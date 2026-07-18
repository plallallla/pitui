use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use bevy_ecs::prelude::{
    Entity, Message, MessageReader, MessageWriter, Messages, Res, ResMut, Resource, World,
};
use pitui_core::{
    Branch, BranchKind, ChangedFile, Commit, CommitDetail, FileDiff, ReflogEntry, Repository,
    WorkingTreeChange, WorkingTreeDiff,
};
use pitui_data::{
    ActiveUiContext, BranchMetadata, ChangeBoundary, CommitMetadata, DatasetChildren,
    DatasetIdentity, DatasetIndex, DatasetKey, DatasetKind, DatasetRevision, DatasetTemplateId,
    DatasetTemplateRef, DatasetTemplateRegistry, DatasetType, DefaultDatasetTemplates,
    FileChangesMetadata, FileMetadata, GitOperationLogEntryMetadata, GitOperationStatus,
    HasSnapshot, InteractionNoticeRequest, ReflogEntryMetadata, RenderBindingId, RepositoryKey,
    RepositoryMetadata, WorkingTreeFileChangesMetadata, WorkingTreeFileMetadata,
};
use pitui_git::{
    CliGitExecutor, GitCommand, GitExecutor, GitFailure, ParsedGitPayload,
    logging::{GitLogStatus, GitOperationLogSink, GitOperationRecord, NoopGitOperationLogSink},
};

use crate::{KernelError, ensure_dataset_in_world, replace_children_in_world, require_dataset};

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

#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub struct GitCommandData {
    pub repository_dataset: Entity,
    pub cwd: PathBuf,
    pub command: GitCommand,
}

#[derive(Message, Clone, Debug, Eq, PartialEq)]
pub struct GitResultData {
    pub repository_dataset: Entity,
    pub cwd: PathBuf,
    pub command: GitCommand,
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
        data: GitCommandData,
        failure: GitFailure,
        started_at: SystemTime,
        duration: Duration,
    },
    Dataset {
        data: GitCommandData,
        failure: GitDataError,
        started_at: SystemTime,
        duration: Duration,
    },
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct GitExecutionFailures(pub Vec<GitExecutionFailure>);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GitMutationSuccess {
    pub command: GitCommand,
    pub message: String,
}

#[derive(Resource, Clone, Debug, Default, Eq, PartialEq)]
pub struct GitMutationSuccesses(pub Vec<GitMutationSuccess>);

#[derive(Resource, Clone, Debug, Default)]
pub(super) struct PendingGitResults(Vec<GitResultData>);

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
    world.init_resource::<Messages<GitCommandData>>();
    world.init_resource::<Messages<GitResultData>>();
    world.init_resource::<PendingGitResults>();
    world.init_resource::<GitExecutionFailures>();
    world.init_resource::<GitMutationSuccesses>();
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
    mut commands: MessageWriter<GitCommandData>,
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
        commands.write(GitCommandData {
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
        commands.write(GitCommandData {
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
        commands.write(GitCommandData {
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

pub(super) fn update_git_messages(world: &mut World) {
    world.resource_mut::<Messages<GitCommandData>>().update();
    world.resource_mut::<Messages<GitResultData>>().update();
}

pub(super) fn execute_git_commands(
    mut commands: MessageReader<GitCommandData>,
    executor: Res<GitExecutorResource>,
    mut results: MessageWriter<GitResultData>,
) {
    for data in commands.read() {
        let started_at = SystemTime::now();
        let started = std::time::Instant::now();
        let result = executor.0.execute(&data.cwd, &data.command);
        results.write(GitResultData {
            repository_dataset: data.repository_dataset,
            cwd: data.cwd.clone(),
            command: data.command.clone(),
            started_at,
            duration: started.elapsed(),
            result,
        });
    }
}

pub(super) fn collect_git_results(
    mut results: MessageReader<GitResultData>,
    mut pending: ResMut<PendingGitResults>,
) {
    pending.0.extend(results.read().cloned());
}

pub(super) fn apply_pending_git_results(world: &mut World) {
    let results = std::mem::take(&mut world.resource_mut::<PendingGitResults>().0);
    for result in results {
        let command_data = GitCommandData {
            repository_dataset: result.repository_dataset,
            cwd: result.cwd.clone(),
            command: result.command.clone(),
        };
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
                match apply_payload(world, &command_data, payload) {
                    Ok(()) => {
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
                    }
                    Err(failure) => {
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
                        world.resource_mut::<GitExecutionFailures>().0.push(
                            GitExecutionFailure::Dataset {
                                data: command_data,
                                failure,
                                started_at: result.started_at,
                                duration: result.duration,
                            },
                        );
                    }
                }
            }
            Err(failure) => {
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
                world
                    .resource_mut::<GitExecutionFailures>()
                    .0
                    .push(GitExecutionFailure::Git {
                        data: command_data,
                        failure,
                        started_at: result.started_at,
                        duration: result.duration,
                    })
            }
        }
    }
}

fn payload_summary(payload: &ParsedGitPayload) -> String {
    match payload {
        ParsedGitPayload::Repository(repository) => format!(
            "branch={} staged={} modified={} untracked={} conflicted={}",
            repository
                .current_branch
                .as_ref()
                .map_or("detached", |branch| branch.0.as_str()),
            repository.status.staged,
            repository.status.modified,
            repository.status.untracked,
            repository.status.conflicted
        ),
        ParsedGitPayload::Branches(branches) => format!("branches={}", branches.len()),
        ParsedGitPayload::Commits { branch, commits } => {
            format!("branch={} commits={}", branch.0, commits.len())
        }
        ParsedGitPayload::CommitDetail(detail) => format!(
            "commit={} files={}",
            detail.commit.hash.short(),
            detail.files.len()
        ),
        ParsedGitPayload::FileDiff(diff) => format!(
            "path={} hunks={} binary={}",
            diff.path,
            diff.hunks.len(),
            diff.is_binary
        ),
        ParsedGitPayload::Reflog(entries) => format!("entries={}", entries.len()),
        ParsedGitPayload::WorkingTree(changes) => format!("changes={}", changes.len()),
        ParsedGitPayload::WorkingTreeDiff(diff) => {
            format!("path={} sections={}", diff.path, diff.sections.len())
        }
        ParsedGitPayload::CommandSucceeded { message } => message.clone(),
        ParsedGitPayload::ConflictAborted { message, .. } => message.clone(),
    }
}

struct GitOperationOutcome {
    status: GitOperationStatus,
    message: String,
    abort_attempted: bool,
    abort_result: Option<String>,
}

fn record_git_operation(
    world: &mut World,
    data: &GitCommandData,
    started_at: SystemTime,
    duration: Duration,
    outcome: GitOperationOutcome,
) {
    let GitOperationOutcome {
        status,
        message,
        abort_attempted,
        abort_result,
    } = outcome;
    let repository = match world.get::<DatasetKey>(data.repository_dataset) {
        Some(DatasetKey(DatasetIdentity::Repository(repository))) => repository.clone(),
        _ => RepositoryKey::new(data.cwd.clone()),
    };
    world
        .resource::<GitOperationLogSinkResource>()
        .0
        .record(&GitOperationRecord {
            operation: data.command.operation_name().into(),
            repository: repository.as_path().to_path_buf(),
            started_at,
            duration,
            status: match status {
                GitOperationStatus::Success => GitLogStatus::Success,
                GitOperationStatus::Failure => GitLogStatus::Failure,
                GitOperationStatus::ConflictAborted => GitLogStatus::ConflictAborted,
            },
            message: message.clone(),
            abort_attempted,
            abort_result: abort_result.clone(),
        });

    let Some(log) = world
        .resource::<DatasetIndex>()
        .get(&DatasetIdentity::GlobalGitOperationLog)
    else {
        return;
    };
    let Some(template) = world
        .resource::<DefaultDatasetTemplates>()
        .get(DatasetKind::GitOperationLogEntry)
        .cloned()
    else {
        return;
    };
    let sequence = {
        let mut next = world.resource_mut::<NextGitOperationLogSequence>();
        let sequence = next.0;
        next.0 = next.0.wrapping_add(1);
        sequence
    };
    let identity = DatasetIdentity::GitOperationLogEntry(sequence);
    let Ok(entry) =
        ensure_dataset_in_world(world, identity, DatasetKind::GitOperationLogEntry, template)
    else {
        return;
    };
    world
        .entity_mut(entry)
        .insert(GitOperationLogEntryMetadata {
            sequence,
            operation: data.command.operation_name().into(),
            repository,
            started_at_utc: format_system_time_utc(started_at),
            duration_ms: duration.as_millis(),
            status,
            message: pitui_git::sanitize_log_text(&message, 4096),
            abort_attempted,
            abort_result: abort_result.map(|result| pitui_git::sanitize_log_text(&result, 4096)),
        });
    let mut children = world
        .get::<DatasetChildren>(log)
        .map(|children| children.0.clone())
        .unwrap_or_default();
    children.insert(0, entry);
    if replace_children_in_world(world, log, children, true).is_ok() {
        let log_is_active = world
            .get_resource::<ActiveUiContext>()
            .is_some_and(|context| context.active_dataset == log);
        if let Some(mut cursor) = world.get_mut::<pitui_data::DatasetCursor>(log)
            && (!log_is_active || cursor.0.is_none())
        {
            cursor.0 = Some(entry);
        }
    }
}

fn format_system_time_utc(time: SystemTime) -> String {
    let duration = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let millis = duration.subsec_millis();
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = second_of_day / 3_600;
    let minute = second_of_day % 3_600 / 60;
    let second = second_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i64, i64, i64) {
    let days = days_since_unix_epoch + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn request_interaction_notice(world: &mut World, title: &str, message: String) {
    world
        .resource_mut::<Messages<InteractionNoticeRequest>>()
        .write(InteractionNoticeRequest {
            title: title.into(),
            message,
        });
}

#[derive(Clone, Debug)]
struct PlannedNode {
    identity: DatasetIdentity,
    kind: DatasetKind,
    template: DatasetTemplateId,
}

#[derive(Clone, Debug)]
struct PlannedChildren {
    parent: DatasetIdentity,
    children: Vec<DatasetIdentity>,
}

#[derive(Clone, Debug)]
enum MetadataUpdate {
    Branch(DatasetIdentity, Branch),
    Commit(DatasetIdentity, CommitMetadata),
    File(DatasetIdentity, ChangedFile),
    FileChanges(DatasetIdentity, FileDiff),
    WorkingTreeFile(DatasetIdentity, WorkingTreeChange),
    WorkingTreeFileChanges(DatasetIdentity, WorkingTreeDiff),
    ReflogEntry(DatasetIdentity, ReflogEntry),
}

#[derive(Clone, Debug, Default)]
struct DatasetSnapshotPlan {
    nodes: Vec<PlannedNode>,
    children: Vec<PlannedChildren>,
    metadata: Vec<MetadataUpdate>,
    snapshots: Vec<DatasetIdentity>,
    invalidated_snapshots: Vec<DatasetIdentity>,
}

impl DatasetSnapshotPlan {
    fn add_node(
        &mut self,
        identity: DatasetIdentity,
        kind: DatasetKind,
        template: DatasetTemplateId,
    ) {
        self.nodes.push(PlannedNode {
            identity,
            kind,
            template,
        });
    }

    fn replace_children(&mut self, parent: DatasetIdentity, children: Vec<DatasetIdentity>) {
        self.snapshots.push(parent.clone());
        self.children.push(PlannedChildren { parent, children });
    }

    fn mark_snapshot(&mut self, identity: DatasetIdentity) {
        self.snapshots.push(identity);
    }

    fn invalidate_snapshot(&mut self, identity: DatasetIdentity) {
        self.invalidated_snapshots.push(identity);
    }
}

fn apply_payload(
    world: &mut World,
    data: &GitCommandData,
    payload: ParsedGitPayload,
) -> Result<(), GitDataError> {
    require_dataset(world, data.repository_dataset)?;
    let repository = repository_key(world, data.repository_dataset)?;
    let plan = match (&data.command, payload) {
        (
            GitCommand::StagePaths { .. }
            | GitCommand::UnstagePaths { .. }
            | GitCommand::Commit { .. }
            | GitCommand::CherryPick { .. },
            ParsedGitPayload::CommandSucceeded { message },
        ) => {
            world
                .resource_mut::<GitMutationSuccesses>()
                .0
                .push(GitMutationSuccess {
                    command: data.command.clone(),
                    message,
                });
            return Ok(());
        }
        (GitCommand::CherryPick { .. }, ParsedGitPayload::ConflictAborted { .. }) => return Ok(()),
        (GitCommand::LoadRepository, ParsedGitPayload::Repository(metadata)) => {
            return apply_repository_metadata(world, data.repository_dataset, metadata);
        }
        (GitCommand::LoadBranches, ParsedGitPayload::Branches(branches)) => {
            branches_plan(world, repository, branches)?
        }
        (
            GitCommand::LoadCommits { branch, .. },
            ParsedGitPayload::Commits {
                branch: payload_branch,
                commits,
            },
        ) if branch == &payload_branch => commits_plan(world, repository, payload_branch, commits)?,
        (GitCommand::LoadCommitDetail { commit }, ParsedGitPayload::CommitDetail(detail))
            if commit == &detail.commit.hash =>
        {
            commit_detail_plan(world, repository, detail)?
        }
        (GitCommand::LoadFileDiff { commit, path, .. }, ParsedGitPayload::FileDiff(diff))
            if commit == &diff.commit && path == &diff.path =>
        {
            file_diff_plan(world, repository, diff)?
        }
        (GitCommand::LoadWorkingTree, ParsedGitPayload::WorkingTree(changes)) => {
            working_tree_plan(world, repository, changes)?
        }
        (GitCommand::LoadReflog { .. }, ParsedGitPayload::Reflog(entries)) => {
            reflog_plan(world, repository, entries)?
        }
        (
            GitCommand::LoadWorkingTreeDiff {
                path,
                include_staged,
                include_worktree,
                untracked,
                ..
            },
            ParsedGitPayload::WorkingTreeDiff(diff),
        ) if path == &diff.path => {
            let boundary = if *include_staged && !*include_worktree && !*untracked {
                ChangeBoundary::Staged
            } else if !*include_staged && (*include_worktree || *untracked) {
                ChangeBoundary::Unstaged
            } else {
                return Err(GitDataError::PayloadDoesNotMatchCommand);
            };
            working_tree_diff_plan(world, repository, boundary, diff)?
        }
        _ => return Err(GitDataError::PayloadDoesNotMatchCommand),
    };

    validate_plan(world, &plan)?;
    apply_plan(world, plan)
}

fn repository_key(world: &World, entity: Entity) -> Result<RepositoryKey, GitDataError> {
    match world.get::<DatasetKey>(entity).map(|key| &key.0) {
        Some(DatasetIdentity::Repository(repository)) => Ok(repository.clone()),
        _ => Err(GitDataError::TargetIsNotRepository(entity)),
    }
}

fn template_for(world: &World, kind: DatasetKind) -> Result<DatasetTemplateId, GitDataError> {
    world
        .resource::<DefaultDatasetTemplates>()
        .get(kind)
        .cloned()
        .ok_or(GitDataError::MissingDefaultTemplate(kind))
}

fn existing_node(world: &World, entity: Entity) -> Result<PlannedNode, GitDataError> {
    Ok(PlannedNode {
        identity: world
            .get::<DatasetKey>(entity)
            .ok_or(KernelError::MissingDataset(entity))?
            .0
            .clone(),
        kind: world
            .get::<DatasetType>(entity)
            .ok_or(KernelError::MissingDataset(entity))?
            .0,
        template: world
            .get::<DatasetTemplateRef>(entity)
            .ok_or(KernelError::MissingDataset(entity))?
            .0
            .clone(),
    })
}

fn apply_repository_metadata(
    world: &mut World,
    repository_entity: Entity,
    metadata: Repository,
) -> Result<(), GitDataError> {
    let old_identity = world
        .get::<DatasetKey>(repository_entity)
        .ok_or(KernelError::MissingDataset(repository_entity))?
        .0
        .clone();
    let DatasetIdentity::Repository(old_repository) = &old_identity else {
        return Err(GitDataError::TargetIsNotRepository(repository_entity));
    };
    let actual_repository = RepositoryKey::new(metadata.root.clone());
    let actual_identity = DatasetIdentity::Repository(actual_repository.clone());
    if let Some(existing) = world.resource::<DatasetIndex>().get(&actual_identity)
        && existing != repository_entity
    {
        return Err(GitDataError::RepositoryIdentityCollision(actual_repository));
    }

    // All fallible checks finish before identity and metadata are committed.
    if old_repository != &actual_repository {
        let mut index = world.resource_mut::<DatasetIndex>();
        index.by_key.remove(&old_identity);
        index
            .by_key
            .insert(actual_identity.clone(), repository_entity);
        world
            .entity_mut(repository_entity)
            .insert(DatasetKey(actual_identity));
    }
    world
        .entity_mut(repository_entity)
        .insert(RepositoryMetadata(metadata));
    mark_snapshot(world, repository_entity);
    Ok(())
}

fn branches_plan(
    world: &World,
    repository: RepositoryKey,
    mut branches: Vec<Branch>,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let branch_template = template_for(world, DatasetKind::Branch)?;
    let commits_template = template_for(world, DatasetKind::Commits)?;
    let repository_identity = DatasetIdentity::Repository(repository.clone());
    let repository_entity = world
        .resource::<DatasetIndex>()
        .get(&repository_identity)
        .ok_or_else(|| GitDataError::MissingPlannedIdentity(repository_identity.clone()))?;
    let repository_node = existing_node(world, repository_entity)?;
    if let Some(metadata) = world.get::<RepositoryMetadata>(repository_entity)
        && let Some(current) = &metadata.0.current_branch
        && !branches.iter().any(|branch| &branch.name == current)
    {
        branches.insert(
            0,
            Branch {
                name: current.clone(),
                full_ref: format!("refs/heads/{current}"),
                kind: BranchKind::Local,
                head: metadata.0.head.clone(),
                short_head: metadata.0.head.short().to_string(),
                commit_date: String::new(),
                subject: String::new(),
                is_current: true,
            },
        );
    }
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(
        repository_node.identity.clone(),
        repository_node.kind,
        repository_node.template,
    );

    let mut branch_ids = Vec::with_capacity(branches.len());
    for branch in branches {
        let branch_id = DatasetIdentity::Branch {
            repository: repository.clone(),
            name: branch.name.clone(),
        };
        let commits_id = DatasetIdentity::Commits {
            repository: repository.clone(),
            branch: branch.name.clone(),
        };
        plan.add_node(
            branch_id.clone(),
            DatasetKind::Branch,
            branch_template.clone(),
        );
        plan.add_node(
            commits_id.clone(),
            DatasetKind::Commits,
            commits_template.clone(),
        );
        plan.metadata
            .push(MetadataUpdate::Branch(branch_id.clone(), branch));
        plan.replace_children(branch_id.clone(), vec![commits_id]);
        branch_ids.push(branch_id);
    }
    plan.replace_children(repository_identity, branch_ids);
    Ok(plan)
}

fn commits_plan(
    world: &World,
    repository: RepositoryKey,
    branch: pitui_core::BranchName,
    commits: Vec<Commit>,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let commits_template = template_for(world, DatasetKind::Commits)?;
    let commit_template = template_for(world, DatasetKind::Commit)?;
    let files_template = template_for(world, DatasetKind::Files)?;
    let commits_id = DatasetIdentity::Commits {
        repository: repository.clone(),
        branch,
    };
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(commits_id.clone(), DatasetKind::Commits, commits_template);

    let mut commit_ids = Vec::with_capacity(commits.len());
    for commit in commits {
        let commit_id = DatasetIdentity::Commit {
            repository: repository.clone(),
            hash: commit.hash.clone(),
        };
        let files_id = DatasetIdentity::Files {
            repository: repository.clone(),
            commit: commit.hash.clone(),
        };
        let tags = tags_from_decorations(&commit.decorations);
        let message = world
            .resource::<DatasetIndex>()
            .get(&commit_id)
            .and_then(|entity| world.get::<CommitMetadata>(entity))
            .and_then(|metadata| metadata.message.clone());
        plan.add_node(
            commit_id.clone(),
            DatasetKind::Commit,
            commit_template.clone(),
        );
        plan.add_node(files_id.clone(), DatasetKind::Files, files_template.clone());
        plan.metadata.push(MetadataUpdate::Commit(
            commit_id.clone(),
            CommitMetadata {
                summary: commit,
                message,
                tags,
            },
        ));
        plan.replace_children(commit_id.clone(), vec![files_id]);
        commit_ids.push(commit_id);
    }
    plan.replace_children(commits_id, commit_ids);
    Ok(plan)
}

fn commit_detail_plan(
    world: &World,
    repository: RepositoryKey,
    detail: CommitDetail,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let commit_template = template_for(world, DatasetKind::Commit)?;
    let files_template = template_for(world, DatasetKind::Files)?;
    let file_template = template_for(world, DatasetKind::File)?;
    let changes_template = template_for(world, DatasetKind::FileChanges)?;
    let commit_id = DatasetIdentity::Commit {
        repository: repository.clone(),
        hash: detail.commit.hash.clone(),
    };
    let files_id = DatasetIdentity::Files {
        repository: repository.clone(),
        commit: detail.commit.hash.clone(),
    };
    let tags = world
        .resource::<DatasetIndex>()
        .get(&commit_id)
        .and_then(|entity| world.get::<CommitMetadata>(entity))
        .map(|metadata| metadata.tags.clone())
        .unwrap_or_default();

    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(commit_id.clone(), DatasetKind::Commit, commit_template);
    plan.add_node(files_id.clone(), DatasetKind::Files, files_template);
    plan.metadata.push(MetadataUpdate::Commit(
        commit_id.clone(),
        CommitMetadata {
            summary: detail.commit.clone(),
            message: Some(detail.message),
            tags,
        },
    ));

    let mut file_ids = Vec::with_capacity(detail.files.len());
    for file in detail.files {
        let file_id = DatasetIdentity::File {
            repository: repository.clone(),
            commit: detail.commit.hash.clone(),
            path: file.path.clone(),
        };
        let changes_id = DatasetIdentity::FileChanges {
            repository: repository.clone(),
            commit: detail.commit.hash.clone(),
            path: file.path.clone(),
        };
        plan.add_node(file_id.clone(), DatasetKind::File, file_template.clone());
        plan.add_node(
            changes_id.clone(),
            DatasetKind::FileChanges,
            changes_template.clone(),
        );
        plan.metadata
            .push(MetadataUpdate::File(file_id.clone(), file));
        plan.replace_children(file_id.clone(), vec![changes_id]);
        file_ids.push(file_id);
    }
    plan.replace_children(files_id.clone(), file_ids);
    plan.replace_children(commit_id, vec![files_id]);
    Ok(plan)
}

fn file_diff_plan(
    world: &World,
    repository: RepositoryKey,
    diff: FileDiff,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let changes_template = template_for(world, DatasetKind::FileChanges)?;
    let changes_id = DatasetIdentity::FileChanges {
        repository,
        commit: diff.commit.clone(),
        path: diff.path.clone(),
    };
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(
        changes_id.clone(),
        DatasetKind::FileChanges,
        changes_template,
    );
    plan.metadata
        .push(MetadataUpdate::FileChanges(changes_id.clone(), diff));
    plan.mark_snapshot(changes_id);
    Ok(plan)
}

fn working_tree_plan(
    world: &World,
    repository: RepositoryKey,
    changes: Vec<WorkingTreeChange>,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let changes_template = template_for(world, DatasetKind::Changes)?;
    let groups_template = template_for(world, DatasetKind::WorkingTreeFiles)?;
    let file_template = template_for(world, DatasetKind::WorkingTreeFile)?;
    let diff_template = template_for(world, DatasetKind::WorkingTreeFileChanges)?;
    let changes_id = DatasetIdentity::GlobalChanges;
    let staged_group = DatasetIdentity::WorkingTreeFiles {
        repository: repository.clone(),
        boundary: ChangeBoundary::Staged,
    };
    let unstaged_group = DatasetIdentity::WorkingTreeFiles {
        repository: repository.clone(),
        boundary: ChangeBoundary::Unstaged,
    };
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(changes_id.clone(), DatasetKind::Changes, changes_template);
    for group in [&staged_group, &unstaged_group] {
        plan.add_node(
            group.clone(),
            DatasetKind::WorkingTreeFiles,
            groups_template.clone(),
        );
    }

    let mut staged_files = Vec::new();
    let mut unstaged_files = Vec::new();
    for change in changes {
        for boundary in [ChangeBoundary::Staged, ChangeBoundary::Unstaged] {
            let included = match boundary {
                ChangeBoundary::Staged => change.has_staged_changes(),
                ChangeBoundary::Unstaged => change.has_worktree_changes() || change.is_untracked(),
            };
            if !included {
                continue;
            }
            let file_id = DatasetIdentity::WorkingTreeFile {
                repository: repository.clone(),
                boundary,
                path: change.path.clone(),
            };
            let diff_id = DatasetIdentity::WorkingTreeFileChanges {
                repository: repository.clone(),
                boundary,
                path: change.path.clone(),
            };
            plan.add_node(
                file_id.clone(),
                DatasetKind::WorkingTreeFile,
                file_template.clone(),
            );
            plan.add_node(
                diff_id.clone(),
                DatasetKind::WorkingTreeFileChanges,
                diff_template.clone(),
            );
            plan.invalidate_snapshot(diff_id.clone());
            plan.metadata.push(MetadataUpdate::WorkingTreeFile(
                file_id.clone(),
                change.clone(),
            ));
            plan.replace_children(file_id.clone(), vec![diff_id]);
            match boundary {
                ChangeBoundary::Staged => staged_files.push(file_id),
                ChangeBoundary::Unstaged => unstaged_files.push(file_id),
            }
        }
    }
    plan.replace_children(staged_group.clone(), staged_files);
    plan.replace_children(unstaged_group.clone(), unstaged_files);
    plan.replace_children(changes_id, vec![staged_group, unstaged_group]);
    Ok(plan)
}

fn reflog_plan(
    world: &World,
    repository: RepositoryKey,
    entries: Vec<ReflogEntry>,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let reflog_template = template_for(world, DatasetKind::Reflog)?;
    let entry_template = template_for(world, DatasetKind::ReflogEntry)?;
    let reflog_id = DatasetIdentity::Reflog(repository.clone());
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(reflog_id.clone(), DatasetKind::Reflog, reflog_template);

    let mut entry_ids = Vec::with_capacity(entries.len());
    for entry in entries {
        let entry_id = DatasetIdentity::ReflogEntry {
            repository: repository.clone(),
            selector: entry.selector.clone(),
        };
        plan.add_node(
            entry_id.clone(),
            DatasetKind::ReflogEntry,
            entry_template.clone(),
        );
        plan.metadata
            .push(MetadataUpdate::ReflogEntry(entry_id.clone(), entry));
        entry_ids.push(entry_id);
    }
    plan.replace_children(reflog_id, entry_ids);
    Ok(plan)
}

fn working_tree_diff_plan(
    world: &World,
    repository: RepositoryKey,
    boundary: ChangeBoundary,
    diff: WorkingTreeDiff,
) -> Result<DatasetSnapshotPlan, GitDataError> {
    let template = template_for(world, DatasetKind::WorkingTreeFileChanges)?;
    let identity = DatasetIdentity::WorkingTreeFileChanges {
        repository,
        boundary,
        path: diff.path.clone(),
    };
    let mut plan = DatasetSnapshotPlan::default();
    plan.add_node(
        identity.clone(),
        DatasetKind::WorkingTreeFileChanges,
        template,
    );
    plan.metadata.push(MetadataUpdate::WorkingTreeFileChanges(
        identity.clone(),
        diff,
    ));
    plan.mark_snapshot(identity);
    Ok(plan)
}

fn tags_from_decorations(decorations: &str) -> Vec<String> {
    decorations
        .split(',')
        .map(str::trim)
        .filter_map(|decoration| decoration.strip_prefix("tag: "))
        .map(str::to_string)
        .collect()
}

fn validate_plan(world: &World, plan: &DatasetSnapshotPlan) -> Result<(), GitDataError> {
    let mut planned = HashMap::<DatasetIdentity, (&DatasetKind, &DatasetTemplateId)>::new();
    for node in &plan.nodes {
        if let Some((kind, template)) =
            planned.insert(node.identity.clone(), (&node.kind, &node.template))
            && (*kind != node.kind || *template != node.template)
        {
            return Err(GitDataError::DuplicatePlannedIdentity(
                node.identity.clone(),
            ));
        }
        let registered_kind = world
            .resource::<DatasetTemplateRegistry>()
            .get(&node.template)
            .map(|template| template.kind)
            .ok_or_else(|| KernelError::MissingTemplate(node.template.clone()))?;
        if registered_kind != node.kind {
            return Err(KernelError::TemplateKindMismatch {
                template: node.template.clone(),
                expected: node.kind,
                actual: registered_kind,
            }
            .into());
        }
        if let Some(entity) = world.resource::<DatasetIndex>().get(&node.identity) {
            let kind = world
                .get::<DatasetType>(entity)
                .ok_or(KernelError::MissingDataset(entity))?
                .0;
            let template = world
                .get::<DatasetTemplateRef>(entity)
                .ok_or(KernelError::MissingDataset(entity))?
                .0
                .clone();
            if kind != node.kind {
                return Err(KernelError::IdentityKindMismatch {
                    identity: Box::new(node.identity.clone()),
                    expected: node.kind,
                    actual: kind,
                }
                .into());
            }
            if template != node.template {
                return Err(KernelError::IdentityTemplateMismatch {
                    identity: Box::new(node.identity.clone()),
                    expected: node.template.clone(),
                    actual: template,
                }
                .into());
            }
        }
    }

    let mut graph = current_identity_graph(world);
    for relation in &plan.children {
        if !planned.contains_key(&relation.parent)
            && world
                .resource::<DatasetIndex>()
                .get(&relation.parent)
                .is_none()
        {
            return Err(GitDataError::MissingPlannedIdentity(
                relation.parent.clone(),
            ));
        }
        let mut seen = HashSet::new();
        for child in &relation.children {
            if !seen.insert(child) {
                return Err(GitDataError::DuplicatePlannedIdentity(child.clone()));
            }
            if !planned.contains_key(child) && world.resource::<DatasetIndex>().get(child).is_none()
            {
                return Err(GitDataError::MissingPlannedIdentity(child.clone()));
            }
        }
        graph.insert(relation.parent.clone(), relation.children.clone());
    }
    if let Some(identity) = graph_cycle(&graph) {
        return Err(GitDataError::PlannedCycle(identity));
    }
    Ok(())
}

fn current_identity_graph(world: &World) -> HashMap<DatasetIdentity, Vec<DatasetIdentity>> {
    let index = world.resource::<DatasetIndex>();
    let mut graph = HashMap::new();
    for (identity, entity) in &index.by_key {
        let children = world
            .get::<DatasetChildren>(*entity)
            .map(|children| {
                children
                    .0
                    .iter()
                    .filter_map(|child| world.get::<DatasetKey>(*child))
                    .map(|key| key.0.clone())
                    .collect()
            })
            .unwrap_or_default();
        graph.insert(identity.clone(), children);
    }
    graph
}

fn graph_cycle(graph: &HashMap<DatasetIdentity, Vec<DatasetIdentity>>) -> Option<DatasetIdentity> {
    fn visit(
        node: &DatasetIdentity,
        graph: &HashMap<DatasetIdentity, Vec<DatasetIdentity>>,
        visiting: &mut HashSet<DatasetIdentity>,
        visited: &mut HashSet<DatasetIdentity>,
    ) -> Option<DatasetIdentity> {
        if visiting.contains(node) {
            return Some(node.clone());
        }
        if !visited.insert(node.clone()) {
            return None;
        }
        visiting.insert(node.clone());
        if let Some(children) = graph.get(node) {
            for child in children {
                if let Some(cycle) = visit(child, graph, visiting, visited) {
                    return Some(cycle);
                }
            }
        }
        visiting.remove(node);
        None
    }

    let mut visited = HashSet::new();
    for node in graph.keys() {
        if let Some(cycle) = visit(node, graph, &mut HashSet::new(), &mut visited) {
            return Some(cycle);
        }
    }
    None
}

fn apply_plan(world: &mut World, plan: DatasetSnapshotPlan) -> Result<(), GitDataError> {
    for node in &plan.nodes {
        ensure_dataset_in_world(
            world,
            node.identity.clone(),
            node.kind,
            node.template.clone(),
        )?;
    }

    for identity in &plan.invalidated_snapshots {
        let entity = entity_for(world, identity)?;
        world
            .entity_mut(entity)
            .remove::<WorkingTreeFileChangesMetadata>()
            .insert(HasSnapshot(false));
    }

    for update in plan.metadata {
        match update {
            MetadataUpdate::Branch(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world.entity_mut(entity).insert(BranchMetadata(metadata));
            }
            MetadataUpdate::Commit(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world.entity_mut(entity).insert(metadata);
            }
            MetadataUpdate::File(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world.entity_mut(entity).insert(FileMetadata(metadata));
            }
            MetadataUpdate::FileChanges(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(FileChangesMetadata(metadata));
            }
            MetadataUpdate::WorkingTreeFile(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(WorkingTreeFileMetadata(metadata));
            }
            MetadataUpdate::WorkingTreeFileChanges(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(WorkingTreeFileChangesMetadata(metadata));
            }
            MetadataUpdate::ReflogEntry(identity, metadata) => {
                let entity = entity_for(world, &identity)?;
                world
                    .entity_mut(entity)
                    .insert(ReflogEntryMetadata(metadata));
            }
        }
    }

    let relation_parents = plan
        .children
        .iter()
        .map(|relation| relation.parent.clone())
        .collect::<HashSet<_>>();
    for relation in plan.children {
        let parent = entity_for(world, &relation.parent)?;
        let children = relation
            .children
            .iter()
            .map(|identity| entity_for(world, identity))
            .collect::<Result<Vec<_>, _>>()?;
        replace_children_in_world(world, parent, children, true)?;
    }
    for identity in plan.snapshots {
        if !relation_parents.contains(&identity) {
            mark_snapshot(world, entity_for(world, &identity)?);
        }
    }
    Ok(())
}

fn entity_for(world: &World, identity: &DatasetIdentity) -> Result<Entity, GitDataError> {
    world
        .resource::<DatasetIndex>()
        .get(identity)
        .ok_or_else(|| GitDataError::MissingPlannedIdentity(identity.clone()))
}

fn mark_snapshot(world: &mut World, entity: Entity) {
    world.entity_mut(entity).insert(HasSnapshot(true));
    world
        .get_mut::<DatasetRevision>(entity)
        .expect("validated Dataset must have a revision")
        .0 += 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_log_timestamps_as_stable_utc_rfc3339() {
        assert_eq!(
            format_system_time_utc(UNIX_EPOCH),
            "1970-01-01T00:00:00.000Z"
        );
        assert_eq!(
            format_system_time_utc(
                UNIX_EPOCH + Duration::from_secs(946_684_800) + Duration::from_millis(123)
            ),
            "2000-01-01T00:00:00.123Z"
        );
    }
}
