use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bevy_ecs::prelude::{Entity, Messages, Res, ResMut, Resource, World};
use pitui_core::{
    Branch, BranchKind, ChangedFile, Commit, CommitDetail, FileDiff, GitPath, ReflogEntry,
    Repository, WorkingTreeChange, WorkingTreeDiff,
};
use pitui_data::{
    ActiveUiContext, BranchMetadata, ChangeBoundary, CommitFieldKind, CommitFieldMetadata,
    CommitMetadata, DatasetChildren, DatasetIdentity, DatasetIndex, DatasetKey, DatasetKind,
    DatasetRevision, DatasetTemplateId, DatasetTemplateRef, DatasetTemplateRegistry, DatasetType,
    DefaultDatasetTemplates, FileChangesMetadata, FileMetadata, FileTreeDirectoryMetadata,
    GitOperationLogEntryMetadata, GitOperationStatus, HasSnapshot, InteractionNoticeRequest,
    ReflogEntryMetadata, RenderBindingId, RepositoryKey, RepositoryMetadata,
    WorkingTreeFileChangesMetadata, WorkingTreeFileMetadata,
};
use pitui_git::{
    CliGitExecutor, GitCommand, GitExecutor, GitFailure, ParsedGitPayload,
    logging::{GitLogStatus, GitOperationLogSink, GitOperationRecord, NoopGitOperationLogSink},
};

use crate::{KernelError, ensure_dataset_in_world, replace_children_in_world, require_dataset};

mod logging;
mod snapshot;

use logging::{
    GitOperationOutcome, payload_summary, record_git_operation, request_interaction_notice,
};
use snapshot::apply_payload;

#[derive(Resource, Clone)]
pub struct GitExecutorResource(pub Arc<dyn GitExecutor>);

impl Default for GitExecutorResource {
    fn default() -> Self {
        Self(Arc::new(CliGitExecutor))
    }
}

#[derive(Resource, Clone)]
pub struct GitOperationLogSinkResource(pub Arc<dyn GitOperationLogSink>);

impl Default for GitOperationLogSinkResource {
    fn default() -> Self {
        Self(Arc::new(NoopGitOperationLogSink))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitCommandData {
    pub repository_dataset: Entity,
    pub cwd: PathBuf,
    pub command: GitCommand,
}

/// Stable correlation key for one Git effect. Callers that need to react to a
/// specific mutation retain this value instead of indexing an append-only
/// success vector. It is also the stale-result guard used by the future async
/// executor boundary.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct GitRequestId(pub u64);

/// Typed reads that must run after a Git job has completed and its payload has
/// been accepted. Mutation Operations describe their data impact once; the
/// Git runtime owns sequencing and never relies on message insertion order.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum GitRefreshTarget {
    Repository,
    Branches,
    Commits {
        branch: pitui_core::BranchName,
        limit: usize,
    },
    WorkingTree,
    Reflog {
        limit: usize,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitRefreshPlan(pub Vec<GitRefreshTarget>);

impl GitRefreshPlan {
    pub fn new(targets: impl IntoIterator<Item = GitRefreshTarget>) -> Self {
        let mut unique = HashSet::new();
        Self(
            targets
                .into_iter()
                .filter(|target| unique.insert(target.clone()))
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Independent load channels within one repository. Repository metadata and
/// its branch children deliberately use different keys, so a refresh can run
/// both without either result superseding the other.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub enum GitLoadTarget {
    Repository,
    Branches,
    Commits {
        branch: pitui_core::BranchName,
    },
    CommitDetail {
        commit: pitui_core::CommitHash,
    },
    FileDiff {
        commit: pitui_core::CommitHash,
        path: GitPath,
    },
    WorkingTree,
    Reflog,
    WorkingTreeDiff {
        boundary: ChangeBoundary,
        path: GitPath,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct GitLoadKey {
    pub repository_dataset: Entity,
    pub target: GitLoadTarget,
}

impl GitLoadKey {
    fn from_data(data: &GitCommandData) -> Option<Self> {
        let target = match &data.command {
            GitCommand::LoadRepository => GitLoadTarget::Repository,
            GitCommand::LoadBranches => GitLoadTarget::Branches,
            GitCommand::LoadCommits { branch, .. } => GitLoadTarget::Commits {
                branch: branch.clone(),
            },
            GitCommand::LoadCommitDetail { commit } => GitLoadTarget::CommitDetail {
                commit: commit.clone(),
            },
            GitCommand::LoadFileDiff { commit, path, .. } => GitLoadTarget::FileDiff {
                commit: commit.clone(),
                path: path.clone(),
            },
            GitCommand::LoadReflog { .. } => GitLoadTarget::Reflog,
            GitCommand::LoadWorkingTree => GitLoadTarget::WorkingTree,
            GitCommand::LoadWorkingTreeDiff {
                path,
                include_staged,
                include_worktree,
                untracked,
                ..
            } => {
                let boundary = if *include_staged && !*include_worktree && !*untracked {
                    ChangeBoundary::Staged
                } else if !*include_staged && (*include_worktree || *untracked) {
                    ChangeBoundary::Unstaged
                } else {
                    return None;
                };
                GitLoadTarget::WorkingTreeDiff {
                    boundary,
                    path: path.clone(),
                }
            }
            GitCommand::StagePaths { .. }
            | GitCommand::UnstagePaths { .. }
            | GitCommand::Commit { .. }
            | GitCommand::CherryPick { .. }
            | GitCommand::Reset { .. } => return None,
        };
        Some(Self {
            repository_dataset: data.repository_dataset,
            target,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitLoadStatus {
    Queued {
        request_id: GitRequestId,
    },
    Running {
        request_id: GitRequestId,
    },
    Ready {
        request_id: GitRequestId,
    },
    Failed {
        request_id: GitRequestId,
        message: String,
    },
}

impl GitLoadStatus {
    pub const fn request_id(&self) -> GitRequestId {
        match self {
            Self::Queued { request_id }
            | Self::Running { request_id }
            | Self::Ready { request_id }
            | Self::Failed { request_id, .. } => *request_id,
        }
    }
}

/// Latest-wins load state. It prevents a slow, older diff/commit response from
/// replacing a newer request while keeping every transition inspectable data.
#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct GitLoadTracker {
    states: HashMap<GitLoadKey, GitLoadStatus>,
    completed: VecDeque<(GitLoadKey, GitRequestId)>,
}

impl GitLoadTracker {
    pub fn get(&self, key: &GitLoadKey) -> Option<&GitLoadStatus> {
        self.states.get(key)
    }

    pub fn states(&self) -> &HashMap<GitLoadKey, GitLoadStatus> {
        &self.states
    }

    fn queue(&mut self, key: GitLoadKey, request_id: GitRequestId) {
        self.states
            .insert(key, GitLoadStatus::Queued { request_id });
    }

    fn start_if_current(&mut self, key: &GitLoadKey, request_id: GitRequestId) -> bool {
        if self
            .states
            .get(key)
            .is_none_or(|status| status.request_id() != request_id)
        {
            return false;
        }
        self.states
            .insert(key.clone(), GitLoadStatus::Running { request_id });
        true
    }

    fn is_current(&self, key: &GitLoadKey, request_id: GitRequestId) -> bool {
        self.states
            .get(key)
            .is_some_and(|status| status.request_id() == request_id)
    }

    fn complete(&mut self, key: GitLoadKey, status: GitLoadStatus, retention: usize) {
        let request_id = status.request_id();
        self.states.insert(key.clone(), status);
        self.completed.push_back((key, request_id));
        while self.completed.len() > retention {
            let Some((expired_key, expired_request)) = self.completed.pop_front() else {
                break;
            };
            if self
                .states
                .get(&expired_key)
                .is_some_and(|status| status.request_id() == expired_request)
            {
                self.states.remove(&expired_key);
            }
        }
    }
}

/// One executable Git job. `GitCommandData` remains the public effect payload;
/// the runtime adds identity and the success continuation when it is queued.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitJob {
    pub request_id: GitRequestId,
    /// Root request that caused this job. Initial requests correlate to
    /// themselves; success-triggered refresh jobs retain the mutation ID.
    pub correlation_id: GitRequestId,
    pub tracks_outcome: bool,
    pub data: GitCommandData,
    pub load_key: Option<GitLoadKey>,
    pub refresh_after_apply: GitRefreshPlan,
}

/// Deterministic ingress queue for Git effects. It is intentionally ordinary
/// ECS data so a later background executor can drain jobs and return results
/// without changing Operation Systems or their refresh semantics.
#[derive(Resource, Clone, Debug)]
pub struct GitRequestQueue {
    next_request_id: u64,
    pending: VecDeque<GitJob>,
}

impl Default for GitRequestQueue {
    fn default() -> Self {
        Self {
            next_request_id: 1,
            pending: VecDeque::new(),
        }
    }
}

impl GitRequestQueue {
    pub fn enqueue(&mut self, data: GitCommandData) -> GitRequestId {
        self.enqueue_with_refresh(data, GitRefreshPlan::default())
    }

    pub fn enqueue_with_refresh(
        &mut self,
        data: GitCommandData,
        refresh_after_apply: GitRefreshPlan,
    ) -> GitRequestId {
        self.enqueue_job(data, refresh_after_apply, false)
    }

    /// Enqueues a mutation whose logical continuation must not depend on
    /// bounded diagnostic histories. Correlated refresh jobs inherit this
    /// flag and update [`GitRequestOutcomes`] on failure.
    pub fn enqueue_with_refresh_tracked(
        &mut self,
        data: GitCommandData,
        refresh_after_apply: GitRefreshPlan,
    ) -> GitRequestId {
        self.enqueue_job(data, refresh_after_apply, true)
    }

    fn enqueue_job(
        &mut self,
        data: GitCommandData,
        refresh_after_apply: GitRefreshPlan,
        tracks_outcome: bool,
    ) -> GitRequestId {
        let request_id = GitRequestId(self.next_request_id);
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.pending.push_back(GitJob {
            request_id,
            correlation_id: request_id,
            tracks_outcome,
            load_key: GitLoadKey::from_data(&data),
            data,
            refresh_after_apply,
        });
        request_id
    }

    fn enqueue_correlated(
        &mut self,
        data: GitCommandData,
        correlation_id: GitRequestId,
        tracks_outcome: bool,
    ) -> GitRequestId {
        let request_id = GitRequestId(self.next_request_id);
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        self.pending.push_back(GitJob {
            request_id,
            correlation_id,
            tracks_outcome,
            load_key: GitLoadKey::from_data(&data),
            data,
            refresh_after_apply: GitRefreshPlan::default(),
        });
        request_id
    }

    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    fn take_pending(&mut self) -> VecDeque<GitJob> {
        std::mem::take(&mut self.pending)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitResultData {
    pub job: GitJob,
    pub started_at: SystemTime,
    pub duration: Duration,
    pub result: Result<ParsedGitPayload, GitFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitDataError {
    Kernel(KernelError),
    TargetIsNotRepository(Entity),
    RepositoryIdentityCollision(RepositoryKey),
    PayloadDoesNotMatchCommand,
    MissingDefaultTemplate(DatasetKind),
    DuplicatePlannedIdentity(DatasetIdentity),
    MissingPlannedIdentity(DatasetIdentity),
    PlannedCycle(DatasetIdentity),
}

impl From<KernelError> for GitDataError {
    fn from(value: KernelError) -> Self {
        Self::Kernel(value)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitExecutionFailure {
    Git {
        request_id: GitRequestId,
        correlation_id: GitRequestId,
        data: GitCommandData,
        failure: GitFailure,
        started_at: SystemTime,
        duration: Duration,
    },
    Dataset {
        request_id: GitRequestId,
        correlation_id: GitRequestId,
        data: GitCommandData,
        failure: GitDataError,
        started_at: SystemTime,
        duration: Duration,
    },
}

impl GitExecutionFailure {
    pub const fn request_id(&self) -> GitRequestId {
        match self {
            Self::Git { request_id, .. } | Self::Dataset { request_id, .. } => *request_id,
        }
    }

    pub const fn correlation_id(&self) -> GitRequestId {
        match self {
            Self::Git { correlation_id, .. } | Self::Dataset { correlation_id, .. } => {
                *correlation_id
            }
        }
    }
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct GitExecutionFailures(pub VecDeque<GitExecutionFailure>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitMutationSuccess {
    pub request_id: GitRequestId,
    pub command: GitCommand,
    pub message: String,
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct GitMutationSuccesses(pub VecDeque<GitMutationSuccess>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GitRequestOutcome {
    MutationApplied { command: GitCommand },
    Failed,
}

/// Control-plane completion state for explicitly tracked continuations.
/// Unlike diagnostic histories this map is acknowledged by its consumer and
/// is unaffected by logging retention settings.
#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct GitRequestOutcomes(HashMap<GitRequestId, GitRequestOutcome>);

impl GitRequestOutcomes {
    pub fn get(&self, request_id: GitRequestId) -> Option<&GitRequestOutcome> {
        self.0.get(&request_id)
    }

    pub fn acknowledge(&mut self, request_id: GitRequestId) {
        self.0.remove(&request_id);
    }

    fn set(&mut self, request_id: GitRequestId, outcome: GitRequestOutcome) {
        self.0.insert(request_id, outcome);
    }
}

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct PendingGitResults(VecDeque<GitResultData>);

/// Bounds diagnostic/session data independently from the persistent JSONL
/// file rotation policy. Request IDs, rather than retained vector positions,
/// provide effect correlation so old entries can be safely evicted.
#[derive(Resource, Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitRuntimeRetention {
    pub failure_entries: usize,
    pub mutation_entries: usize,
    pub session_log_entries: usize,
    pub completed_load_entries: usize,
}

impl Default for GitRuntimeRetention {
    fn default() -> Self {
        Self {
            failure_entries: 256,
            mutation_entries: 256,
            session_log_entries: 1_000,
            completed_load_entries: 2_048,
        }
    }
}

#[derive(Resource, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct LastDependentReadGeneration(Option<u64>);

#[derive(Resource, Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct NextGitOperationLogSequence(u64);

pub(super) fn initialize_git_runtime(
    world: &mut World,
    executor: Arc<dyn GitExecutor>,
    log_sink: Arc<dyn GitOperationLogSink>,
) {
    world.insert_resource(GitExecutorResource(executor));
    world.insert_resource(GitOperationLogSinkResource(log_sink));
    world.init_resource::<GitRequestQueue>();
    world.init_resource::<GitLoadTracker>();
    world.init_resource::<PendingGitResults>();
    world.init_resource::<GitExecutionFailures>();
    world.init_resource::<GitMutationSuccesses>();
    world.init_resource::<GitRequestOutcomes>();
    world.init_resource::<GitRuntimeRetention>();
    world.init_resource::<LastDependentReadGeneration>();
    world.init_resource::<NextGitOperationLogSequence>();
}

#[allow(clippy::too_many_arguments)]
pub(super) fn enqueue_dependent_reads(
    context: Option<Res<ActiveUiContext>>,
    mut last_generation: ResMut<LastDependentReadGeneration>,
    index: Res<DatasetIndex>,
    keys: bevy_ecs::prelude::Query<&DatasetKey>,
    repositories: bevy_ecs::prelude::Query<&RepositoryMetadata>,
    commits: bevy_ecs::prelude::Query<&CommitMetadata>,
    files: bevy_ecs::prelude::Query<&FileMetadata>,
    working_files: bevy_ecs::prelude::Query<&WorkingTreeFileMetadata>,
    snapshots: bevy_ecs::prelude::Query<&HasSnapshot>,
    mut commands: ResMut<GitRequestQueue>,
) {
    let Some(context) = context else {
        return;
    };
    if last_generation.0 == Some(context.generation) {
        return;
    }
    last_generation.0 = Some(context.generation);
    let Some(repository_entity) = context
        .render_bindings
        .get(&RenderBindingId::CurrentRepository)
    else {
        return;
    };
    let Ok(repository) = repositories.get(repository_entity) else {
        return;
    };
    let cwd = repository.0.root.clone();

    if let Some(commit_entity) = context.render_bindings.get(&RenderBindingId::CurrentCommit)
        && commits
            .get(commit_entity)
            .is_ok_and(|metadata| metadata.message.is_none())
        && let Ok(DatasetKey(DatasetIdentity::Commit { hash, .. })) = keys.get(commit_entity)
    {
        commands.enqueue(GitCommandData {
            repository_dataset: repository_entity,
            cwd: cwd.clone(),
            command: GitCommand::LoadCommitDetail {
                commit: hash.clone(),
            },
        });
    }

    if let Some(diff_entity) = context
        .render_bindings
        .get(&RenderBindingId::CurrentFileChanges)
        && snapshots.get(diff_entity).is_ok_and(|snapshot| !snapshot.0)
        && let Ok(DatasetKey(DatasetIdentity::FileChanges {
            repository,
            commit,
            path,
        })) = keys.get(diff_entity)
    {
        let file = index.get(&DatasetIdentity::File {
            repository: repository.clone(),
            commit: commit.clone(),
            path: path.clone(),
        });
        let old_path = file
            .and_then(|file| files.get(file).ok())
            .and_then(|metadata| metadata.0.old_path.clone());
        commands.enqueue(GitCommandData {
            repository_dataset: repository_entity,
            cwd: cwd.clone(),
            command: GitCommand::LoadFileDiff {
                commit: commit.clone(),
                path: path.clone(),
                old_path,
            },
        });
    }

    if let Some(diff_entity) = context
        .render_bindings
        .get(&RenderBindingId::CurrentChangesFileChanges)
        && snapshots.get(diff_entity).is_ok_and(|snapshot| !snapshot.0)
        && let Ok(DatasetKey(DatasetIdentity::WorkingTreeFileChanges {
            repository,
            boundary,
            path,
        })) = keys.get(diff_entity)
    {
        let file = index.get(&DatasetIdentity::WorkingTreeFile {
            repository: repository.clone(),
            boundary: *boundary,
            path: path.clone(),
        });
        let metadata = file.and_then(|file| working_files.get(file).ok());
        commands.enqueue(GitCommandData {
            repository_dataset: repository_entity,
            cwd,
            command: GitCommand::LoadWorkingTreeDiff {
                path: path.clone(),
                old_path: metadata.and_then(|metadata| metadata.0.old_path.clone()),
                include_staged: *boundary == ChangeBoundary::Staged,
                include_worktree: *boundary == ChangeBoundary::Unstaged
                    && metadata.is_some_and(|metadata| !metadata.0.is_untracked()),
                untracked: *boundary == ChangeBoundary::Unstaged
                    && metadata.is_some_and(|metadata| metadata.0.is_untracked()),
            },
        });
    }
}

pub(super) fn prepare_git_commands(
    commands: Res<GitRequestQueue>,
    mut loads: ResMut<GitLoadTracker>,
) {
    for job in &commands.pending {
        if let Some(key) = &job.load_key {
            loads.queue(key.clone(), job.request_id);
        }
    }
}

pub(super) fn execute_git_commands(
    mut commands: ResMut<GitRequestQueue>,
    executor: Res<GitExecutorResource>,
    mut loads: ResMut<GitLoadTracker>,
    mut outcomes: ResMut<GitRequestOutcomes>,
    mut results: ResMut<PendingGitResults>,
) {
    for job in commands.take_pending() {
        if let Some(key) = &job.load_key
            && !loads.start_if_current(key, job.request_id)
        {
            if job.tracks_outcome {
                outcomes.set(job.correlation_id, GitRequestOutcome::Failed);
            }
            continue;
        }
        let started_at = SystemTime::now();
        let started = std::time::Instant::now();
        let result = executor.0.execute(&job.data.cwd, &job.data.command);
        results.0.push_back(GitResultData {
            job,
            started_at,
            duration: started.elapsed(),
            result,
        });
    }
}

pub(super) fn apply_pending_git_results(world: &mut World) {
    let results = std::mem::take(&mut world.resource_mut::<PendingGitResults>().0);
    for result in results {
        let request_id = result.job.request_id;
        let correlation_id = result.job.correlation_id;
        let tracks_outcome = result.job.tracks_outcome;
        let command_data = result.job.data;
        let load_key = result.job.load_key;
        let refresh_after_apply = result.job.refresh_after_apply;
        if load_key.as_ref().is_some_and(|key| {
            !world
                .resource::<GitLoadTracker>()
                .is_current(key, request_id)
        }) {
            if tracks_outcome {
                world
                    .resource_mut::<GitRequestOutcomes>()
                    .set(correlation_id, GitRequestOutcome::Failed);
            }
            continue;
        }
        match result.result {
            Ok(payload) => {
                let summary = payload_summary(&payload);
                let (status, abort_attempted, abort_result) = match &payload {
                    ParsedGitPayload::ConflictAborted { abort_result, .. } => (
                        GitOperationStatus::ConflictAborted,
                        true,
                        Some(abort_result.clone()),
                    ),
                    _ => (GitOperationStatus::Success, false, None),
                };
                match apply_payload(world, request_id, &command_data, payload) {
                    Ok(()) => {
                        if tracks_outcome
                            && matches!(
                                command_data.command,
                                GitCommand::StagePaths { .. }
                                    | GitCommand::UnstagePaths { .. }
                                    | GitCommand::Commit { .. }
                                    | GitCommand::CherryPick { .. }
                                    | GitCommand::Reset { .. }
                            )
                            && status == GitOperationStatus::Success
                        {
                            world.resource_mut::<GitRequestOutcomes>().set(
                                correlation_id,
                                GitRequestOutcome::MutationApplied {
                                    command: command_data.command.clone(),
                                },
                            );
                        }
                        record_git_operation(
                            world,
                            &command_data,
                            result.started_at,
                            result.duration,
                            GitOperationOutcome {
                                status,
                                message: summary.clone(),
                                abort_attempted,
                                abort_result,
                            },
                        );
                        if status == GitOperationStatus::ConflictAborted {
                            request_interaction_notice(
                                world,
                                "Git conflict aborted",
                                pitui_git::sanitize_log_text(&summary, 4096),
                            );
                        }
                        enqueue_refresh_plan(
                            world,
                            correlation_id,
                            tracks_outcome,
                            &command_data,
                            refresh_after_apply,
                        );
                        complete_git_load(world, load_key, GitLoadStatus::Ready { request_id });
                    }
                    Err(failure) => {
                        if tracks_outcome {
                            world
                                .resource_mut::<GitRequestOutcomes>()
                                .set(correlation_id, GitRequestOutcome::Failed);
                        }
                        let message = format!("dataset update failed: {failure:?}");
                        record_git_operation(
                            world,
                            &command_data,
                            result.started_at,
                            result.duration,
                            GitOperationOutcome {
                                status: GitOperationStatus::Failure,
                                message: message.clone(),
                                abort_attempted: false,
                                abort_result: None,
                            },
                        );
                        request_interaction_notice(
                            world,
                            "Dataset update failed",
                            format!("{}: {message}", command_data.command.operation_name()),
                        );
                        push_git_failure(
                            world,
                            GitExecutionFailure::Dataset {
                                request_id,
                                correlation_id,
                                data: command_data,
                                failure,
                                started_at: result.started_at,
                                duration: result.duration,
                            },
                        );
                        complete_git_load(
                            world,
                            load_key,
                            GitLoadStatus::Failed {
                                request_id,
                                message,
                            },
                        );
                    }
                }
            }
            Err(failure) => {
                if tracks_outcome {
                    world
                        .resource_mut::<GitRequestOutcomes>()
                        .set(correlation_id, GitRequestOutcome::Failed);
                }
                let failure_message = if failure.stderr.is_empty() {
                    "command failed".into()
                } else {
                    pitui_git::sanitize_log_text(&failure.stderr, 4096)
                };
                record_git_operation(
                    world,
                    &command_data,
                    result.started_at,
                    result.duration,
                    GitOperationOutcome {
                        status: GitOperationStatus::Failure,
                        message: failure_message.clone(),
                        abort_attempted: failure.abort_attempted,
                        abort_result: failure.abort_result.clone(),
                    },
                );
                request_interaction_notice(
                    world,
                    "Git operation failed",
                    format!(
                        "{}: {}",
                        command_data.command.operation_name(),
                        failure_message
                    ),
                );
                push_git_failure(
                    world,
                    GitExecutionFailure::Git {
                        request_id,
                        correlation_id,
                        data: command_data,
                        failure,
                        started_at: result.started_at,
                        duration: result.duration,
                    },
                );
                complete_git_load(
                    world,
                    load_key,
                    GitLoadStatus::Failed {
                        request_id,
                        message: failure_message,
                    },
                );
            }
        }
    }
}

fn complete_git_load(world: &mut World, key: Option<GitLoadKey>, status: GitLoadStatus) {
    let Some(key) = key else {
        return;
    };
    let retention = world
        .resource::<GitRuntimeRetention>()
        .completed_load_entries;
    world
        .resource_mut::<GitLoadTracker>()
        .complete(key, status, retention);
}

fn enqueue_refresh_plan(
    world: &mut World,
    correlation_id: GitRequestId,
    tracks_outcome: bool,
    completed: &GitCommandData,
    plan: GitRefreshPlan,
) {
    if plan.is_empty() {
        return;
    }
    let mut queue = world.resource_mut::<GitRequestQueue>();
    for target in plan.0 {
        let command = match target {
            GitRefreshTarget::Repository => GitCommand::LoadRepository,
            GitRefreshTarget::Branches => GitCommand::LoadBranches,
            GitRefreshTarget::Commits { branch, limit } => {
                GitCommand::LoadCommits { branch, limit }
            }
            GitRefreshTarget::WorkingTree => GitCommand::LoadWorkingTree,
            GitRefreshTarget::Reflog { limit } => GitCommand::LoadReflog { limit },
        };
        queue.enqueue_correlated(
            GitCommandData {
                repository_dataset: completed.repository_dataset,
                cwd: completed.cwd.clone(),
                command,
            },
            correlation_id,
            tracks_outcome,
        );
    }
}

fn push_git_failure(world: &mut World, failure: GitExecutionFailure) {
    let limit = world.resource::<GitRuntimeRetention>().failure_entries;
    push_bounded(
        &mut world.resource_mut::<GitExecutionFailures>().0,
        failure,
        limit,
    );
}

fn push_bounded<T>(history: &mut VecDeque<T>, value: T, limit: usize) {
    if limit == 0 {
        return;
    }
    while history.len() >= limit {
        history.pop_front();
    }
    history.push_back(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_queue_assigns_ids_and_keeps_typed_refresh_data() {
        let mut queue = GitRequestQueue::default();
        let repository = Entity::from_bits(42);
        let first = queue.enqueue(GitCommandData {
            repository_dataset: repository,
            cwd: PathBuf::from("/repo"),
            command: GitCommand::LoadRepository,
        });
        let second = queue.enqueue_with_refresh(
            GitCommandData {
                repository_dataset: repository,
                cwd: PathBuf::from("/repo"),
                command: GitCommand::Reset {
                    target: pitui_core::CommitHash("abc".into()),
                    mode: pitui_core::ResetMode::Mixed,
                },
            },
            GitRefreshPlan::new([
                GitRefreshTarget::Repository,
                GitRefreshTarget::Repository,
                GitRefreshTarget::WorkingTree,
            ]),
        );

        assert_eq!(first, GitRequestId(1));
        assert_eq!(second, GitRequestId(2));
        let correlated = queue.enqueue_correlated(
            GitCommandData {
                repository_dataset: repository,
                cwd: PathBuf::from("/repo"),
                command: GitCommand::LoadWorkingTree,
            },
            second,
            true,
        );
        assert_eq!(correlated, GitRequestId(3));
        let jobs = queue.take_pending();
        assert_eq!(jobs[0].correlation_id, first);
        assert_eq!(jobs[1].correlation_id, second);
        assert_eq!(jobs[2].correlation_id, second);
        assert!(jobs[2].tracks_outcome);
        assert_eq!(
            jobs[0].load_key.as_ref().unwrap().target,
            GitLoadTarget::Repository
        );
        assert_eq!(
            jobs[1].refresh_after_apply.0,
            vec![GitRefreshTarget::Repository, GitRefreshTarget::WorkingTree,]
        );
    }

    #[test]
    fn load_tracker_executes_only_the_latest_request_and_bounds_completed_state() {
        let key = GitLoadKey {
            repository_dataset: Entity::from_bits(7),
            target: GitLoadTarget::WorkingTree,
        };
        let other = GitLoadKey {
            repository_dataset: Entity::from_bits(7),
            target: GitLoadTarget::Reflog,
        };
        let mut tracker = GitLoadTracker::default();
        tracker.queue(key.clone(), GitRequestId(1));
        tracker.queue(key.clone(), GitRequestId(2));
        assert!(!tracker.start_if_current(&key, GitRequestId(1)));
        assert!(tracker.start_if_current(&key, GitRequestId(2)));
        tracker.complete(
            key.clone(),
            GitLoadStatus::Ready {
                request_id: GitRequestId(2),
            },
            1,
        );
        tracker.queue(other.clone(), GitRequestId(3));
        tracker.complete(
            other.clone(),
            GitLoadStatus::Ready {
                request_id: GitRequestId(3),
            },
            1,
        );
        assert!(tracker.get(&key).is_none());
        assert_eq!(
            tracker.get(&other),
            Some(&GitLoadStatus::Ready {
                request_id: GitRequestId(3),
            })
        );
    }

    #[test]
    fn formats_log_timestamps_as_stable_utc_rfc3339() {
        assert_eq!(
            logging::format_system_time_utc(UNIX_EPOCH),
            "1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            logging::format_system_time_utc(
                UNIX_EPOCH + Duration::from_secs(946_684_800) + Duration::from_millis(123)
            ),
            "2000-01-01T00:00:00.123Z"
        );
    }
}
