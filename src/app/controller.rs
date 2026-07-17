use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, mpsc::TryRecvError},
    time::Instant,
};

use crate::{
    config::ResolvedConfig,
    domain::{BranchName, CommitHash, GitPath, WorkingTreeDiffKind},
    git::{
        GitCommandBus, GitJobId, GitRequest, GitResponse, GitResponseEnvelope, ResetMode,
        parse_file_diff,
    },
};

use super::{
    Action, AppError, AppState, BranchId, BranchTreeNode, ChangeGroup, ChangeSelection,
    ChangesTreeNode, CommandKind, CommitId, ConfirmDialog, DataRequirement, DiffViewMode, FileId,
    FilterTarget, FocusKind, FocusRole, GlobalMode, OperationId, PendingJobKind, RemoteEditKind,
    RemoteInputField, RepositoryId, ViewId, prompt_command,
};

const PAGE_SIZE: usize = 10;
const COMMIT_LIMIT: usize = 300;
const REFLOG_LIMIT: usize = 300;

pub struct App {
    pub state: AppState,
    bus: GitCommandBus,
    should_quit: bool,
}

impl App {
    pub fn new(bus: GitCommandBus, repository_paths: Vec<PathBuf>) -> Self {
        Self::new_with_config(bus, repository_paths, ResolvedConfig::shared_default())
    }

    pub fn new_with_config(
        bus: GitCommandBus,
        repository_paths: Vec<PathBuf>,
        config: Arc<ResolvedConfig>,
    ) -> Self {
        let backend_log_path = bus.log_path().map(Path::to_path_buf);
        let backend_logging_warning = bus.logging_warning().map(str::to_string);
        let mut state = AppState::with_config(repository_paths, config);
        state.backend_log_path = backend_log_path.clone();
        state.backend_logging_warning = backend_logging_warning.clone();
        state.last_message = backend_logging_warning
            .or_else(|| backend_log_path.map(|path| format!("Backend log: {}", path.display())));
        let mut app = Self {
            state,
            bus,
            should_quit: false,
        };
        app.refresh_all_repositories();
        app
    }

    pub fn should_quit(&self) -> bool {
        self.should_quit
    }

    pub fn take_clipboard_request(&mut self) -> Option<String> {
        self.state.pending_clipboard.take()
    }

    pub fn poll_git(&mut self) {
        loop {
            match self.bus.try_recv() {
                Ok(response) => self.apply_git_response(response),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.state.pending_jobs.clear();
                    if !self.should_quit
                        && !self
                            .state
                            .last_error
                            .as_ref()
                            .is_some_and(|error| error.command == "Git worker")
                    {
                        self.state.set_error(
                            "Git worker".into(),
                            "The Git worker stopped unexpectedly".into(),
                        );
                    }
                    break;
                }
            }
        }
    }

    fn submit(
        &mut self,
        repository_index: usize,
        request: GitRequest,
        kind: PendingJobKind,
    ) -> Option<GitJobId> {
        let cwd = self
            .state
            .repository_node(repository_index)?
            .git_cwd()
            .to_path_buf();
        let id = self.bus.submit(cwd, request);
        self.state.pending_jobs.insert(id, kind);
        Some(id)
    }

    fn submit_status(&mut self, repository_index: usize, full_refresh: bool) {
        if let Some(kind) = self.state.pending_jobs.values_mut().find(|kind| {
            matches!(
                kind,
                PendingJobKind::RepositoryStatus {
                    repository_index: index,
                    ..
                } if *index == repository_index
            )
        }) {
            if full_refresh {
                *kind = PendingJobKind::RepositoryStatus {
                    repository_index,
                    full_refresh: true,
                };
            }
            return;
        }

        let kind = PendingJobKind::RepositoryStatus {
            repository_index,
            full_refresh,
        };
        if let Some(id) = self.submit(repository_index, GitRequest::LoadRepositoryStatus, kind)
            && let Some(repository) = self.state.repository_ui.get_mut(repository_index)
        {
            repository.latest_status_job = Some(id);
        }
    }

    fn refresh_repository(&mut self, repository_index: usize) {
        self.submit_status(repository_index, true);
    }

    fn refresh_all_repositories(&mut self) {
        for repository_index in 0..self.state.repository_ui.len() {
            self.submit_status(repository_index, true);
        }
    }

    fn submit_branches(&mut self, repository_index: usize) {
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadBranches,
            PendingJobKind::Branches { repository_index },
        ) && let Some(repository) = self.state.repository_ui.get_mut(repository_index)
        {
            repository.latest_branches_job = Some(id);
        }
    }

    fn submit_remotes(&mut self, repository_index: usize) {
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadRemotes,
            PendingJobKind::Remotes { repository_index },
        ) {
            self.state.latest_remotes_job = Some(id);
            self.state
                .model
                .mark_remotes_loading(RepositoryId(repository_index));
        }
    }

    fn submit_commits(&mut self, repository_index: usize, branch: BranchName) {
        let branch_id = BranchId {
            repository: RepositoryId(repository_index),
            name: branch.clone(),
        };
        let kind = PendingJobKind::Commits {
            repository_index,
            branch: branch.clone(),
        };
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadCommits {
                branch,
                limit: COMMIT_LIMIT,
            },
            kind,
        ) {
            self.state.latest_commits_job = Some(id);
            self.state.model.mark_branch_commits_loading(&branch_id);
        }
    }

    fn submit_commit_detail(
        &mut self,
        repository_index: usize,
        commit: CommitHash,
        focus_files: bool,
    ) {
        let commit_id = CommitId {
            repository: RepositoryId(repository_index),
            hash: commit.clone(),
        };
        let kind = PendingJobKind::CommitDetail {
            repository_index,
            commit: commit.clone(),
            focus_files,
        };
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadCommitDetail { commit },
            kind,
        ) {
            self.state.latest_commit_detail_job = Some(id);
            self.state.model.mark_commit_loading(&commit_id);
        }
    }

    fn submit_commit_message_for_copy(&mut self, repository_index: usize, commit: CommitHash) {
        let kind = PendingJobKind::CommitMessage {
            repository_index,
            commit: commit.clone(),
        };
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadCommitMessage { commit },
            kind,
        ) {
            self.state.latest_commit_message_job = Some(id);
        }
    }

    fn submit_reflog(&mut self, repository_index: usize) {
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadReflog {
                limit: REFLOG_LIMIT,
            },
            PendingJobKind::Reflog { repository_index },
        ) {
            self.state.latest_reflog_job = Some(id);
            self.state
                .model
                .mark_reflog_loading(RepositoryId(repository_index));
        }
    }

    fn submit_changes(&mut self, repository_index: usize) {
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadWorkingTree,
            PendingJobKind::Changes { repository_index },
        ) {
            self.state.latest_changes_job = Some(id);
            self.state
                .model
                .mark_working_tree_loading(RepositoryId(repository_index));
        }
    }

    fn submit_selected_change_diff(&mut self) {
        let Some(repository_index) = self.state.changes_repository_index else {
            return;
        };
        let Some((group, change)) = self.state.selected_change() else {
            self.state.current_changes_diff = None;
            self.state.current_changes_diff_group = None;
            return;
        };
        let path = change.path.clone();
        let old_path = change.old_path.clone();
        let include_staged = group == ChangeGroup::Staged;
        let untracked = group == ChangeGroup::Unstaged && change.is_untracked();
        let include_worktree = group == ChangeGroup::Unstaged && !untracked;
        let kind = PendingJobKind::ChangesDiff {
            repository_index,
            path: path.clone(),
            group,
        };
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadWorkingTreeDiff {
                path,
                old_path,
                include_staged,
                include_worktree,
                untracked,
            },
            kind,
        ) {
            self.state.latest_changes_diff_job = Some(id);
        }
        self.state.current_changes_diff = None;
        self.state.current_changes_diff_group = Some(group);
        self.state.selection.changes_diff_scroll = 0;
    }

    fn submit_selected_file_diff(&mut self, focus_diff: bool) {
        let Some(file) = self.state.selected_file_id() else {
            return;
        };
        self.submit_file_diff(file, focus_diff);
    }

    fn submit_file_diff(&mut self, file_id: FileId, focus_diff: bool) {
        let repository_index = file_id.commit.repository.0;
        let commit = file_id.commit.hash.clone();
        let path = file_id.path.clone();
        let old_path = self
            .state
            .model
            .file(&file_id)
            .and_then(|file| file.summary.old_path.clone());
        let kind = PendingJobKind::FileDiff {
            repository_index,
            commit: commit.clone(),
            path: path.clone(),
            focus_diff,
        };
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadFileDiff {
                commit,
                path,
                old_path,
            },
            kind,
        ) {
            self.state.latest_file_diff_job = Some(id);
            self.state.model.mark_file_diff_loading(&file_id);
        }
        self.state.selection.diff_scroll = 0;
    }

    fn is_stale(&self, id: GitJobId, kind: &PendingJobKind) -> bool {
        let repository_index = kind.repository_index();
        match kind {
            PendingJobKind::RepositoryStatus { .. } => self
                .state
                .repository_ui
                .get(repository_index)
                .is_none_or(|repository| repository.latest_status_job != Some(id)),
            PendingJobKind::Branches { .. } => self
                .state
                .repository_ui
                .get(repository_index)
                .is_none_or(|repository| repository.latest_branches_job != Some(id)),
            PendingJobKind::Remotes { .. } => {
                self.state.latest_remotes_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
                    || self.state.remotes_repository_index != Some(repository_index)
                    || self.state.view_projection().view != ViewId::Remotes
            }
            PendingJobKind::Commits { .. } => {
                self.state.latest_commits_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
            }
            PendingJobKind::CommitDetail {
                commit,
                focus_files,
                ..
            } => {
                self.state.latest_commit_detail_job != Some(id)
                    || self.state.viewing_repository_index() != Some(repository_index)
                    || self
                        .state
                        .selected_commit()
                        .is_none_or(|selected| &selected.hash != commit)
                    || (!focus_files && self.state.view_projection().view != ViewId::Commit)
            }
            PendingJobKind::CommitMessage { commit, .. } => {
                self.state.latest_commit_message_job != Some(id)
                    || self.state.viewing_repository_index() != Some(repository_index)
                    || self
                        .state
                        .selected_commit()
                        .is_none_or(|selected| &selected.hash != commit)
            }
            PendingJobKind::FileDiff {
                commit,
                path,
                focus_diff,
                ..
            } => {
                self.state.latest_file_diff_job != Some(id)
                    || self.state.viewing_repository_index() != Some(repository_index)
                    || self
                        .state
                        .current_commit_id()
                        .is_none_or(|selected| &selected.hash != commit)
                    || self
                        .state
                        .selected_file()
                        .is_none_or(|file| &file.path != path)
                    || if *focus_diff {
                        !matches!(
                            self.state.view_projection().view,
                            ViewId::Commit | ViewId::FileDiff
                        )
                    } else {
                        self.state.view_projection().view != ViewId::FileDiff
                    }
            }
            PendingJobKind::Reflog { .. } => {
                self.state.latest_reflog_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
                    || self.state.reflog_repository_index != Some(repository_index)
                    || self.state.view_projection().view != ViewId::Reflog
            }
            PendingJobKind::Changes { .. } => {
                self.state.latest_changes_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
                    || self.state.changes_repository_index != Some(repository_index)
                    || self.state.view_projection().view != ViewId::Changes
            }
            PendingJobKind::ChangesDiff { .. } => {
                self.state.latest_changes_diff_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
                    || self.state.changes_repository_index != Some(repository_index)
                    || self.state.view_projection().view != ViewId::Changes
            }
            PendingJobKind::Command { .. } => false,
        }
    }

    fn same_data_resource(left: &PendingJobKind, right: &PendingJobKind) -> bool {
        match (left, right) {
            (
                PendingJobKind::Commits {
                    repository_index: left_repository,
                    branch: left_branch,
                },
                PendingJobKind::Commits {
                    repository_index: right_repository,
                    branch: right_branch,
                },
            ) => left_repository == right_repository && left_branch == right_branch,
            (
                PendingJobKind::CommitDetail {
                    repository_index: left_repository,
                    commit: left_commit,
                    ..
                },
                PendingJobKind::CommitDetail {
                    repository_index: right_repository,
                    commit: right_commit,
                    ..
                },
            ) => left_repository == right_repository && left_commit == right_commit,
            (
                PendingJobKind::FileDiff {
                    repository_index: left_repository,
                    commit: left_commit,
                    path: left_path,
                    ..
                },
                PendingJobKind::FileDiff {
                    repository_index: right_repository,
                    commit: right_commit,
                    path: right_path,
                    ..
                },
            ) => {
                left_repository == right_repository
                    && left_commit == right_commit
                    && left_path == right_path
            }
            (
                PendingJobKind::Reflog {
                    repository_index: left,
                },
                PendingJobKind::Reflog {
                    repository_index: right,
                },
            )
            | (
                PendingJobKind::Changes {
                    repository_index: left,
                },
                PendingJobKind::Changes {
                    repository_index: right,
                },
            )
            | (
                PendingJobKind::Remotes {
                    repository_index: left,
                },
                PendingJobKind::Remotes {
                    repository_index: right,
                },
            ) => left == right,
            _ => false,
        }
    }

    fn reset_stale_resource_if_orphaned(&mut self, kind: &PendingJobKind) {
        if self
            .state
            .pending_jobs
            .values()
            .any(|pending| Self::same_data_resource(kind, pending))
        {
            return;
        }
        match kind {
            PendingJobKind::Commits {
                repository_index,
                branch,
            } => self.state.model.reset_branch_commits_loading(&BranchId {
                repository: RepositoryId(*repository_index),
                name: branch.clone(),
            }),
            PendingJobKind::CommitDetail {
                repository_index,
                commit,
                ..
            } => self.state.model.reset_commit_loading(&CommitId {
                repository: RepositoryId(*repository_index),
                hash: commit.clone(),
            }),
            PendingJobKind::FileDiff {
                repository_index,
                commit,
                path,
                ..
            } => self.state.model.reset_file_diff_loading(&FileId {
                commit: CommitId {
                    repository: RepositoryId(*repository_index),
                    hash: commit.clone(),
                },
                path: path.clone(),
            }),
            PendingJobKind::Reflog { repository_index } => self
                .state
                .model
                .reset_reflog_loading(RepositoryId(*repository_index)),
            PendingJobKind::Changes { repository_index } => self
                .state
                .model
                .reset_working_tree_loading(RepositoryId(*repository_index)),
            PendingJobKind::Remotes { repository_index } => self
                .state
                .model
                .reset_remotes_loading(RepositoryId(*repository_index)),
            PendingJobKind::RepositoryStatus { .. }
            | PendingJobKind::Branches { .. }
            | PendingJobKind::CommitMessage { .. }
            | PendingJobKind::ChangesDiff { .. }
            | PendingJobKind::Command { .. } => {}
        }
    }

    fn clear_latest(&mut self, id: GitJobId, kind: &PendingJobKind) {
        let repository_index = kind.repository_index();
        match kind {
            PendingJobKind::RepositoryStatus { .. } => {
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index)
                    && repository.latest_status_job == Some(id)
                {
                    repository.latest_status_job = None;
                }
            }
            PendingJobKind::Branches { .. } => {
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index)
                    && repository.latest_branches_job == Some(id)
                {
                    repository.latest_branches_job = None;
                }
            }
            PendingJobKind::Remotes { .. } if self.state.latest_remotes_job == Some(id) => {
                self.state.latest_remotes_job = None;
            }
            PendingJobKind::Commits { .. } if self.state.latest_commits_job == Some(id) => {
                self.state.latest_commits_job = None;
            }
            PendingJobKind::CommitDetail { .. }
                if self.state.latest_commit_detail_job == Some(id) =>
            {
                self.state.latest_commit_detail_job = None;
            }
            PendingJobKind::CommitMessage { .. }
                if self.state.latest_commit_message_job == Some(id) =>
            {
                self.state.latest_commit_message_job = None;
            }
            PendingJobKind::FileDiff { .. } if self.state.latest_file_diff_job == Some(id) => {
                self.state.latest_file_diff_job = None;
            }
            PendingJobKind::Reflog { .. } if self.state.latest_reflog_job == Some(id) => {
                self.state.latest_reflog_job = None;
            }
            PendingJobKind::Changes { .. } if self.state.latest_changes_job == Some(id) => {
                self.state.latest_changes_job = None;
            }
            PendingJobKind::ChangesDiff { .. }
                if self.state.latest_changes_diff_job == Some(id) =>
            {
                self.state.latest_changes_diff_job = None;
            }
            _ => {}
        }
    }

    fn selected_tree_identity(&self) -> Option<(usize, Option<BranchName>)> {
        match self.state.selected_tree_node()? {
            BranchTreeNode::Repository { repository_index } => Some((repository_index, None)),
            BranchTreeNode::Branch {
                repository_index,
                branch_index,
            } => Some((
                repository_index,
                self.state
                    .repository_branch(repository_index, branch_index)
                    .map(|branch| branch.name.clone()),
            )),
        }
    }

    fn restore_tree_selection(&mut self, identity: Option<(usize, Option<BranchName>)>) {
        let Some((wanted_repository, wanted_branch)) = identity else {
            self.state.ensure_valid_branch_selection();
            return;
        };
        let nodes = self.state.visible_tree_nodes();
        let found = nodes.iter().position(|node| match (*node, &wanted_branch) {
            (BranchTreeNode::Repository { repository_index }, None) => {
                repository_index == wanted_repository
            }
            (
                BranchTreeNode::Branch {
                    repository_index,
                    branch_index,
                },
                Some(branch_name),
            ) => {
                repository_index == wanted_repository
                    && self
                        .state
                        .repository_branch(repository_index, branch_index)
                        .is_some_and(|branch| branch.name == *branch_name)
            }
            _ => false,
        });
        self.state.selection.selected_branch_index = found.or_else(|| {
            nodes.iter().position(|node| {
                matches!(
                    node,
                    BranchTreeNode::Repository { repository_index }
                        if *repository_index == wanted_repository
                )
            })
        });
        self.state.ensure_valid_branch_selection();
    }

    fn desired_branch(&self, repository_index: usize) -> Option<BranchName> {
        self.state
            .repository_ui
            .get(repository_index)?
            .viewing_branch
            .clone()
            .or_else(|| {
                self.state
                    .repository(repository_index)
                    .and_then(|value| value.current_branch.clone())
            })
            .or_else(|| Some(BranchName("HEAD".into())))
    }

    fn load_active_repository_commits(&mut self, repository_index: usize) {
        if self.state.active_repository_index != Some(repository_index) {
            return;
        }
        let has_head = self
            .state
            .repository(repository_index)
            .is_some_and(|repository| !repository.head.0.is_empty());
        if has_head {
            if let Some(branch) = self.desired_branch(repository_index) {
                self.submit_commits(repository_index, branch);
            }
        } else {
            self.state.viewing_branch = self
                .state
                .repository(repository_index)
                .and_then(|repository| repository.current_branch.clone())
                .map(|name| BranchId {
                    repository: RepositoryId(repository_index),
                    name,
                });
            self.state.ensure_valid_commit_selection();
        }
    }

    fn apply_git_response(&mut self, envelope: GitResponseEnvelope) {
        let Some(kind) = self.state.pending_jobs.remove(&envelope.id) else {
            return;
        };
        let repository_index = kind.repository_index();
        let stale = self.is_stale(envelope.id, &kind);
        self.clear_latest(envelope.id, &kind);
        if stale {
            self.reset_stale_resource_if_orphaned(&kind);
            return;
        }

        match envelope.response {
            GitResponse::RepositoryStatusLoaded(repository) => {
                self.state
                    .model
                    .set_repository_summary(RepositoryId(repository_index), repository);
                let identity = self.selected_tree_identity();
                let full_refresh = matches!(
                    kind,
                    PendingJobKind::RepositoryStatus {
                        full_refresh: true,
                        ..
                    }
                );
                if let Some(state) = self.state.repository_ui.get_mut(repository_index) {
                    state.last_error = None;
                }
                self.restore_tree_selection(identity);
                if full_refresh {
                    self.submit_branches(repository_index);
                    self.load_active_repository_commits(repository_index);
                }
            }
            GitResponse::BranchesLoaded(branches) => {
                let identity = self.selected_tree_identity();
                self.state
                    .model
                    .replace_branches(RepositoryId(repository_index), branches);
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
                    repository.last_error = None;
                }
                self.restore_tree_selection(identity);
            }
            GitResponse::RemotesLoaded(remotes) => {
                self.state
                    .model
                    .set_remotes(RepositoryId(repository_index), remotes);
                let selected_name = self
                    .state
                    .selected_remote()
                    .map(|remote| remote.name.clone());
                self.state.remotes_repository_index = Some(repository_index);
                self.state.selection.selected_remote_index = selected_name.and_then(|name| {
                    self.state
                        .remotes()
                        .iter()
                        .position(|remote| remote.name == name)
                });
                self.state.ensure_valid_remote_selection();
                self.state
                    .set_focus_layer(FocusKind::Remote, FocusRole::Entity);
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
                    repository.last_error = None;
                }
            }
            GitResponse::CommitsLoaded { branch, commits } => {
                let branch_id = BranchId {
                    repository: RepositoryId(repository_index),
                    name: branch.clone(),
                };
                let available_hashes = commits
                    .iter()
                    .map(|commit| commit.hash.clone())
                    .collect::<HashSet<_>>();
                let changed_branch = self.state.viewing_branch.as_ref() != Some(&branch_id);
                self.state.model.replace_branch_commits(&branch_id, commits);
                self.state.viewing_branch = Some(branch_id);
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
                    repository.viewing_branch = Some(branch.clone());
                    repository.last_error = None;
                }
                if changed_branch {
                    self.state.commit_filter.clear();
                    self.state.selection.selected_commit_index = None;
                    self.state.commit_selection.clear();
                    self.state.commit_selection_repository_index = Some(repository_index);
                } else {
                    self.state
                        .commit_selection
                        .retain(|hash| available_hashes.contains(hash));
                }
                self.state.ensure_valid_commit_selection();
            }
            GitResponse::CommitDetailLoaded(detail) => {
                let focus_files = match &kind {
                    PendingJobKind::CommitDetail { focus_files, .. } => *focus_files,
                    _ => return,
                };
                self.state
                    .model
                    .set_commit_detail(RepositoryId(repository_index), detail);
                self.state.selection.selected_file_index = None;
                self.state.ensure_valid_file_selection();
                self.state.expansion.expanded_files.clear();
                self.state.set_focus_layer(
                    if focus_files {
                        FocusKind::File
                    } else {
                        FocusKind::Commit
                    },
                    if focus_files {
                        FocusRole::Collection
                    } else {
                        FocusRole::Entity
                    },
                );
            }
            GitResponse::CommitMessageLoaded { commit, message } => {
                let requested_commit = match &kind {
                    PendingJobKind::CommitMessage { commit, .. } => commit,
                    _ => return,
                };
                if &commit != requested_commit {
                    return;
                }
                self.state.pending_clipboard = Some(message);
                self.state.last_message = Some("Copied current commit message".into());
            }
            GitResponse::FileDiffLoaded(diff) => {
                let focus_diff = match &kind {
                    PendingJobKind::FileDiff { focus_diff, .. } => *focus_diff,
                    _ => return,
                };
                self.state
                    .model
                    .set_file_diff(RepositoryId(repository_index), diff);
                self.state.selection.diff_scroll = 0;
                // Enter/open deliberately focuses the diff. Up/Down/Home/End
                // and n/p only refresh it and preserve the current panel.
                self.state.set_focus_layer(
                    if focus_diff {
                        FocusKind::Diff
                    } else {
                        FocusKind::File
                    },
                    if focus_diff {
                        FocusRole::Content
                    } else {
                        FocusRole::Entity
                    },
                );
            }
            GitResponse::ReflogLoaded(entries) => {
                self.state
                    .model
                    .set_reflog(RepositoryId(repository_index), entries);
                self.state.reflog_repository_index = Some(repository_index);
                self.state.selection.selected_reflog_index = None;
                self.state.ensure_valid_reflog_selection();
                self.state
                    .set_focus_layer(FocusKind::Reflog, FocusRole::Entity);
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
                    repository.last_error = None;
                }
            }
            GitResponse::WorkingTreeLoaded(changes) => {
                let selected_container = self.state.selected_changes_node().and_then(|node| {
                    matches!(node, ChangesTreeNode::Root | ChangesTreeNode::Group(_))
                        .then_some(node)
                });
                let selected_identity = self.state.selected_change_identity();
                self.state
                    .model
                    .set_working_tree(RepositoryId(repository_index), changes);
                self.state.changes_repository_index = Some(repository_index);
                self.state.retain_available_change_selection();
                self.state.current_changes_diff = None;
                self.state.current_changes_diff_group = None;
                self.state.selection.selected_changes_index = selected_container
                    .and_then(|wanted| {
                        self.state
                            .visible_changes_nodes()
                            .iter()
                            .position(|node| *node == wanted)
                    })
                    .or_else(|| {
                        selected_identity.and_then(|(wanted_group, wanted_path)| {
                            self.state.visible_changes_nodes().iter().position(|node| {
                                let ChangesTreeNode::File {
                                    group,
                                    change_index,
                                } = *node
                                else {
                                    return false;
                                };
                                group == wanted_group
                                    && self
                                        .state
                                        .working_tree_changes()
                                        .get(change_index)
                                        .is_some_and(|change| change.path == wanted_path)
                            })
                        })
                    });
                self.state.ensure_valid_changes_selection();
                if !matches!(
                    self.state.focus_context().kind,
                    FocusKind::Changes | FocusKind::ChangesDiff
                ) {
                    self.state
                        .set_focus_layer(FocusKind::Changes, FocusRole::Entity);
                }
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
                    repository.last_error = None;
                }
                self.submit_selected_change_diff();
            }
            GitResponse::WorkingTreeDiffLoaded(diff) => {
                let group = match kind {
                    PendingJobKind::ChangesDiff { group, .. } => group,
                    _ => return,
                };
                let expected_kind = match group {
                    ChangeGroup::Staged => WorkingTreeDiffKind::Staged,
                    ChangeGroup::Unstaged => {
                        if diff
                            .sections
                            .iter()
                            .any(|section| section.kind == WorkingTreeDiffKind::Untracked)
                        {
                            WorkingTreeDiffKind::Untracked
                        } else {
                            WorkingTreeDiffKind::Worktree
                        }
                    }
                };
                let patch = diff
                    .sections
                    .iter()
                    .find(|section| section.kind == expected_kind)
                    .map(|section| section.lines.join("\n"))
                    .unwrap_or_default();
                let marker = match expected_kind {
                    WorkingTreeDiffKind::Staged => "INDEX",
                    WorkingTreeDiffKind::Worktree => "WORKTREE",
                    WorkingTreeDiffKind::Untracked => "UNTRACKED",
                };
                self.state.current_changes_diff = Some(parse_file_diff(
                    patch.as_bytes(),
                    CommitHash(marker.into()),
                    diff.path,
                    None,
                ));
                self.state.current_changes_diff_group = Some(group);
                self.state.selection.changes_diff_scroll = 0;
            }
            GitResponse::CommandSucceeded { message } => {
                let command_kind = match kind {
                    PendingJobKind::Command { kind, .. } => Some(kind),
                    _ => None,
                };
                if command_kind == Some(CommandKind::CherryPick) {
                    self.state.commit_selection.clear();
                }
                if command_kind == Some(CommandKind::Reset) {
                    self.state.selection.selected_file_index = None;
                    self.state
                        .set_focus_layer(FocusKind::Commit, FocusRole::Collection);
                }
                if matches!(
                    command_kind,
                    Some(CommandKind::Stage | CommandKind::Unstage | CommandKind::Commit)
                ) {
                    self.state.change_selection.clear();
                    self.state.current_changes_diff = None;
                    self.state.current_changes_diff_group = None;
                }
                let remote_command = matches!(
                    command_kind,
                    Some(
                        CommandKind::AddRemote
                            | CommandKind::SetRemoteUrl
                            | CommandKind::SetUpstreamRemote
                    )
                );
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
                    repository.last_error = None;
                }
                self.state.last_error = None;
                self.state.last_message = Some(message);
                self.refresh_repository(repository_index);
                if self.state.view_projection().view == ViewId::Changes
                    && self.state.changes_repository_index == Some(repository_index)
                {
                    self.submit_changes(repository_index);
                }
                if remote_command
                    && self.state.view_projection().view == ViewId::Remotes
                    && self.state.remotes_repository_index == Some(repository_index)
                {
                    self.submit_remotes(repository_index);
                }
            }
            GitResponse::RebaseConflictAborted { command, stderr } => {
                let operation = if matches!(
                    kind,
                    PendingJobKind::Command {
                        kind: CommandKind::PullRebase,
                        ..
                    }
                ) {
                    "Pull --rebase"
                } else {
                    "Rebase"
                };
                self.state.set_error(
                    command,
                    format!(
                        "{operation} stopped because of conflicts. Pitui automatically ran `git rebase --abort`; the original branch and working tree were restored.\n\n{stderr}"
                    ),
                );
                self.refresh_repository(repository_index);
            }
            GitResponse::CommandFailed { command, stderr } => {
                self.mark_model_resource_failed(&kind, stderr.clone());
                let error = AppError {
                    command: command.clone(),
                    message: stderr.clone(),
                };
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
                    repository.last_error = Some(error);
                }
                let command_job = matches!(kind, PendingJobKind::Command { .. });
                if command_job || self.state.active_repository_index == Some(repository_index) {
                    self.state.set_error(command, stderr);
                }
                if command_job {
                    self.submit_status(repository_index, false);
                }
            }
        }
        self.state.reconcile_focus();
        self.reconcile_data_requirements();
    }

    fn mark_model_resource_failed(&mut self, kind: &PendingJobKind, error: String) {
        match kind {
            PendingJobKind::Commits {
                repository_index,
                branch,
            } => self.state.model.mark_branch_commits_failed(
                &BranchId {
                    repository: RepositoryId(*repository_index),
                    name: branch.clone(),
                },
                error,
            ),
            PendingJobKind::CommitDetail {
                repository_index,
                commit,
                ..
            } => self.state.model.mark_commit_failed(
                &CommitId {
                    repository: RepositoryId(*repository_index),
                    hash: commit.clone(),
                },
                error,
            ),
            PendingJobKind::FileDiff {
                repository_index,
                commit,
                path,
                ..
            } => self.state.model.mark_file_diff_failed(
                &FileId {
                    commit: CommitId {
                        repository: RepositoryId(*repository_index),
                        hash: commit.clone(),
                    },
                    path: path.clone(),
                },
                error,
            ),
            PendingJobKind::Reflog { repository_index } => self
                .state
                .model
                .mark_reflog_failed(RepositoryId(*repository_index), error),
            PendingJobKind::Changes { repository_index } => self
                .state
                .model
                .mark_working_tree_failed(RepositoryId(*repository_index), error),
            PendingJobKind::Remotes { repository_index } => self
                .state
                .model
                .mark_remotes_failed(RepositoryId(*repository_index), error),
            PendingJobKind::RepositoryStatus { .. }
            | PendingJobKind::Branches { .. }
            | PendingJobKind::CommitMessage { .. }
            | PendingJobKind::ChangesDiff { .. }
            | PendingJobKind::Command { .. } => {}
        }
    }

    pub fn dispatch(&mut self, action: Action) {
        if action == Action::Tick {
            self.on_tick();
            return;
        }
        if action == Action::Quit {
            self.should_quit = true;
            return;
        }

        match self.state.mode.clone() {
            GlobalMode::Filtering { .. } => self.dispatch_filtering(action),
            GlobalMode::Confirming { .. } | GlobalMode::TypingConfirmation { .. } => {
                self.dispatch_dialog(action)
            }
            GlobalMode::EditingCommitMessage { .. } => self.dispatch_commit_message(action),
            GlobalMode::EditingRemote { .. } => self.dispatch_remote_editor(action),
            GlobalMode::Chord { .. } => self.dispatch_chord(action),
            GlobalMode::CommandPrompt { .. } => self.dispatch_command_prompt(action),
            GlobalMode::ShortcutHelp { .. } => self.dispatch_shortcut_help(action),
            GlobalMode::OperationPalette { .. } => self.dispatch_operation_palette(action),
            GlobalMode::Error => match action {
                Action::DismissError | Action::Cancel | Action::Back | Action::Confirm => {
                    self.state.dismiss_error();
                }
                _ => {}
            },
            GlobalMode::Normal => self.dispatch_normal(action),
        }
        self.state.reconcile_focus();
        self.reconcile_data_requirements();
    }

    fn reconcile_data_requirements(&mut self) {
        for requirement in self.state.missing_data_requirements() {
            match requirement {
                DataRequirement::BranchCommits(branch) => {
                    self.submit_commits(branch.repository.0, branch.name)
                }
                DataRequirement::CommitDetail(commit) => {
                    self.submit_commit_detail(commit.repository.0, commit.hash, false)
                }
                DataRequirement::FileDiff(file) => self.submit_file_diff(file, false),
                DataRequirement::Reflog(repository) => self.submit_reflog(repository.0),
                DataRequirement::WorkingTree(repository) => self.submit_changes(repository.0),
                DataRequirement::Remotes(repository) => self.submit_remotes(repository.0),
            }
        }
    }

    fn on_tick(&mut self) {
        self.state.tick_count = self.state.tick_count.wrapping_add(1);
    }

    fn dispatch_shortcut_help(&mut self, action: Action) {
        if matches!(
            action,
            Action::Cancel | Action::Back | Action::Confirm | Action::OpenShortcutHelp
        ) {
            self.state.close_popup();
            return;
        }
        let context = Some(self.state.focus_context().kind);
        let maximum = self
            .state
            .config
            .shortcut_help_line_count_for_view(self.state.view_projection().view, context)
            .saturating_sub(1)
            .min(usize::from(u16::MAX)) as u16;
        let GlobalMode::ShortcutHelp { scroll } = &mut self.state.mode else {
            return;
        };
        *scroll = match action {
            Action::MoveUp => scroll.saturating_sub(1),
            Action::MoveDown => scroll.saturating_add(1).min(maximum),
            Action::PageUp => scroll.saturating_sub(PAGE_SIZE as u16),
            Action::PageDown => scroll.saturating_add(PAGE_SIZE as u16).min(maximum),
            Action::Home => 0,
            Action::End => maximum,
            _ => *scroll,
        };
    }

    fn dispatch_command_prompt(&mut self, action: Action) {
        match action {
            Action::UpdateCommandPrompt(input) => {
                if let GlobalMode::CommandPrompt {
                    input: current_input,
                    validation_error,
                } = &mut self.state.mode
                {
                    *current_input = input;
                    *validation_error = None;
                }
            }
            Action::SubmitCommandPrompt => {
                let GlobalMode::CommandPrompt { input, .. } = self.state.mode.clone() else {
                    return;
                };
                let input = input.trim();
                if input.is_empty() {
                    if let GlobalMode::CommandPrompt {
                        validation_error, ..
                    } = &mut self.state.mode
                    {
                        *validation_error = Some(
                            "Command cannot be empty. Type `help` for the shortcut guide.".into(),
                        );
                    }
                    return;
                }
                let Some(command) = prompt_command(input) else {
                    if let GlobalMode::CommandPrompt {
                        validation_error, ..
                    } = &mut self.state.mode
                    {
                        *validation_error = Some(format!(
                            "Unknown command `{input}`. Type `help` for the shortcut guide."
                        ));
                    }
                    return;
                };
                let next_action = (command.invoke)();
                self.state.close_popup();
                self.dispatch_normal(next_action);
            }
            Action::Cancel | Action::Back => self.state.close_popup(),
            _ => {}
        }
    }

    fn dispatch_operation_palette(&mut self, action: Action) {
        match action {
            Action::UpdateOperationPalette(query) => {
                if let GlobalMode::OperationPalette {
                    query: current,
                    selected,
                    ..
                } = &mut self.state.mode
                {
                    *current = query;
                    *selected = 0;
                }
            }
            Action::MoveUp
            | Action::MoveDown
            | Action::PageUp
            | Action::PageDown
            | Action::Home
            | Action::End => {
                let count = self.state.operation_palette_matches().len();
                let GlobalMode::OperationPalette { selected, .. } = &mut self.state.mode else {
                    return;
                };
                if count == 0 {
                    *selected = 0;
                    return;
                }
                *selected = match action {
                    Action::MoveUp => selected.saturating_sub(1),
                    Action::MoveDown => selected.saturating_add(1).min(count - 1),
                    Action::PageUp => selected.saturating_sub(PAGE_SIZE),
                    Action::PageDown => selected.saturating_add(PAGE_SIZE).min(count - 1),
                    Action::Home => 0,
                    Action::End => count - 1,
                    _ => *selected,
                };
            }
            Action::SubmitOperationPalette | Action::Confirm => {
                let selected = match &self.state.mode {
                    GlobalMode::OperationPalette { selected, .. } => *selected,
                    _ => return,
                };
                let Some(operation) = self
                    .state
                    .operation_palette_matches()
                    .get(selected)
                    .copied()
                else {
                    return;
                };
                self.state.close_popup();
                if let Some(next_action) = operation.action(&self.state) {
                    self.dispatch_normal(next_action);
                }
            }
            Action::Cancel | Action::Back => self.state.close_popup(),
            _ => {}
        }
    }

    fn dispatch_filtering(&mut self, action: Action) {
        match action {
            Action::UpdateFilter(query) => {
                if let GlobalMode::Filtering {
                    query: current_query,
                    ..
                } = &mut self.state.mode
                {
                    *current_query = query;
                }
                self.state.ensure_valid_branch_selection();
                self.state.ensure_valid_commit_selection();
            }
            Action::SubmitFilter => {
                if let GlobalMode::Filtering { target, query } = self.state.mode.clone() {
                    match target {
                        FilterTarget::Branches => self.state.branch_filter = query,
                        FilterTarget::Commits => self.state.commit_filter = query,
                    }
                }
                self.state.mode = GlobalMode::Normal;
                self.state.ensure_valid_branch_selection();
                self.state.ensure_valid_commit_selection();
                match self.state.focus_context().kind {
                    FocusKind::Repository | FocusKind::Branch => {
                        self.preview_selected_branch_commits()
                    }
                    FocusKind::Commit => self.preview_selected_commit_detail(),
                    _ => {}
                }
            }
            Action::CancelFilter | Action::Cancel | Action::Back => {
                self.state.mode = GlobalMode::Normal;
                self.state.ensure_valid_branch_selection();
                self.state.ensure_valid_commit_selection();
                match self.state.focus_context().kind {
                    FocusKind::Repository | FocusKind::Branch => {
                        self.preview_selected_branch_commits()
                    }
                    FocusKind::Commit => self.preview_selected_commit_detail(),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn dispatch_dialog(&mut self, action: Action) {
        match action {
            Action::UpdateTypedConfirmation(input) => {
                if let GlobalMode::TypingConfirmation {
                    input: current_input,
                    validation_error,
                    ..
                } = &mut self.state.mode
                {
                    *current_input = input;
                    *validation_error = None;
                }
            }
            Action::Confirm | Action::ConfirmReset => self.confirm_dialog(),
            Action::ChooseResetSoft => self.choose_reset_mode(ResetMode::Soft),
            Action::ChooseResetMixed => self.choose_reset_mode(ResetMode::Mixed),
            Action::ChooseResetHard => self.choose_reset_mode(ResetMode::Hard),
            Action::Cancel | Action::Back => self.state.close_popup(),
            _ => {}
        }
    }

    fn dispatch_commit_message(&mut self, action: Action) {
        match action {
            Action::UpdateCommitMessage(input) => {
                if let GlobalMode::EditingCommitMessage {
                    input: current_input,
                    validation_error,
                } = &mut self.state.mode
                {
                    *current_input = input;
                    *validation_error = None;
                }
            }
            Action::SubmitCommit | Action::Confirm => self.submit_commit_message(),
            Action::Cancel | Action::Back => self.state.close_popup(),
            _ => {}
        }
    }

    fn dispatch_remote_editor(&mut self, action: Action) {
        match action {
            Action::UpdateRemoteName(input) => {
                if let GlobalMode::EditingRemote {
                    kind: RemoteEditKind::Add,
                    name,
                    validation_error,
                    ..
                } = &mut self.state.mode
                {
                    *name = input;
                    *validation_error = None;
                }
            }
            Action::UpdateRemoteUrl(input) => {
                if let GlobalMode::EditingRemote {
                    url,
                    validation_error,
                    ..
                } = &mut self.state.mode
                {
                    *url = input;
                    *validation_error = None;
                }
            }
            Action::FocusNextRemoteField => {
                if let GlobalMode::EditingRemote {
                    kind: RemoteEditKind::Add,
                    field,
                    ..
                } = &mut self.state.mode
                {
                    *field = match field {
                        RemoteInputField::Name => RemoteInputField::Url,
                        RemoteInputField::Url => RemoteInputField::Name,
                    };
                }
            }
            Action::SubmitRemoteEditor | Action::Confirm => self.submit_remote_editor(),
            Action::Cancel | Action::Back => self.state.close_popup(),
            _ => {}
        }
    }

    fn submit_remote_editor(&mut self) {
        let GlobalMode::EditingRemote {
            kind,
            field,
            name,
            url,
            ..
        } = self.state.mode.clone()
        else {
            return;
        };
        let Some(repository_index) = self.state.remotes_repository_index else {
            self.state.close_popup();
            return;
        };

        if matches!(kind, RemoteEditKind::Add) && field == RemoteInputField::Name {
            if let Some(error) = Self::remote_name_error(name.trim()) {
                if let GlobalMode::EditingRemote {
                    validation_error, ..
                } = &mut self.state.mode
                {
                    *validation_error = Some(error);
                }
            } else if let GlobalMode::EditingRemote { field, .. } = &mut self.state.mode {
                *field = RemoteInputField::Url;
            }
            return;
        }

        let name = name.trim().to_string();
        let url = url.trim().to_string();
        let error = Self::remote_name_error(&name).or_else(|| Self::remote_url_error(&url));
        if let Some(error) = error {
            if let GlobalMode::EditingRemote {
                validation_error, ..
            } = &mut self.state.mode
            {
                *validation_error = Some(error);
            }
            return;
        }
        if matches!(kind, RemoteEditKind::Add)
            && self
                .state
                .remotes()
                .iter()
                .any(|remote| remote.name == name)
        {
            if let GlobalMode::EditingRemote {
                validation_error, ..
            } = &mut self.state.mode
            {
                *validation_error = Some(format!("Remote `{name}` already exists"));
            }
            return;
        }

        self.state.mode = GlobalMode::Confirming {
            dialog: match kind {
                RemoteEditKind::Add => ConfirmDialog::AddRemote {
                    repository_index,
                    name,
                    url,
                },
                RemoteEditKind::SetUrl { .. } => ConfirmDialog::SetRemoteUrl {
                    repository_index,
                    name,
                    url,
                },
            },
        };
    }

    fn remote_name_error(name: &str) -> Option<String> {
        (name.is_empty()
            || name.starts_with('-')
            || name
                .chars()
                .any(|character| character.is_control() || character.is_whitespace()))
        .then(|| {
            "Remote name cannot be empty, start with `-`, or contain whitespace/control characters"
                .into()
        })
    }

    fn remote_url_error(url: &str) -> Option<String> {
        (url.is_empty() || url.chars().any(|character| character.is_control()))
            .then(|| "Remote URL cannot be empty or contain control characters".into())
    }

    fn dispatch_chord(&mut self, action: Action) {
        match action {
            Action::BeginChord(prefix) => {
                self.state.mode = GlobalMode::Chord {
                    prefix,
                    started_at: Instant::now(),
                };
            }
            Action::Cancel | Action::Back => self.state.mode = GlobalMode::Normal,
            action => {
                self.state.mode = GlobalMode::Normal;
                self.dispatch_normal(action);
            }
        }
    }

    fn submit_commit_message(&mut self) {
        let GlobalMode::EditingCommitMessage { input, .. } = self.state.mode.clone() else {
            return;
        };
        let message = input.trim().to_string();
        if message.is_empty() {
            if let GlobalMode::EditingCommitMessage {
                validation_error, ..
            } = &mut self.state.mode
            {
                *validation_error = Some("Commit message cannot be empty".into());
            }
            return;
        }
        let Some(repository_index) = self.state.changes_repository_index else {
            self.state.close_popup();
            return;
        };
        self.state.close_popup();
        self.submit(
            repository_index,
            GitRequest::Commit { message },
            PendingJobKind::Command {
                repository_index,
                kind: CommandKind::Commit,
            },
        );
    }

    fn choose_reset_mode(&mut self, mode: ResetMode) {
        let GlobalMode::Confirming {
            dialog:
                ConfirmDialog::ResetModeChoice {
                    repository_index,
                    commit,
                    short_hash,
                },
        } = self.state.mode.clone()
        else {
            return;
        };
        self.state.mode = GlobalMode::Confirming {
            dialog: if mode == ResetMode::Hard {
                ConfirmDialog::HardResetWarning {
                    repository_index,
                    commit,
                    expected: short_hash,
                }
            } else {
                ConfirmDialog::Reset {
                    repository_index,
                    commit,
                    mode,
                }
            },
        };
    }

    fn confirm_dialog(&mut self) {
        match self.state.mode.clone() {
            GlobalMode::Confirming { dialog } => {
                if let ConfirmDialog::HardResetWarning {
                    expected,
                    repository_index,
                    commit,
                } = dialog
                {
                    self.state.mode = GlobalMode::TypingConfirmation {
                        dialog: ConfirmDialog::HardResetWarning {
                            repository_index,
                            commit,
                            expected: expected.clone(),
                        },
                        expected,
                        input: String::new(),
                        validation_error: None,
                    };
                    return;
                }
                if matches!(dialog, ConfirmDialog::ResetModeChoice { .. }) {
                    return;
                }
                self.state.close_popup();
                match dialog {
                    ConfirmDialog::FetchRepository { repository_index } => {
                        self.submit(
                            repository_index,
                            GitRequest::Fetch,
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::Fetch,
                            },
                        );
                    }
                    ConfirmDialog::PullRebaseRepository {
                        repository_index, ..
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::PullRebase,
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::PullRebase,
                            },
                        );
                    }
                    ConfirmDialog::PushRepository {
                        repository_index, ..
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::Push,
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::Push,
                            },
                        );
                    }
                    ConfirmDialog::AddRemote {
                        repository_index,
                        name,
                        url,
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::AddRemote { name, url },
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::AddRemote,
                            },
                        );
                    }
                    ConfirmDialog::SetRemoteUrl {
                        repository_index,
                        name,
                        url,
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::SetRemoteUrl { name, url },
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::SetRemoteUrl,
                            },
                        );
                    }
                    ConfirmDialog::SetUpstreamRemote {
                        repository_index,
                        name,
                        ..
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::SetUpstreamRemote { name },
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::SetUpstreamRemote,
                            },
                        );
                    }
                    ConfirmDialog::SwitchBranch {
                        repository_index,
                        branch,
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::SwitchBranch { branch },
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::SwitchBranch,
                            },
                        );
                    }
                    ConfirmDialog::CherryPickSelected {
                        repository_index,
                        commits,
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::CherryPick { commits },
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::CherryPick,
                            },
                        );
                    }
                    ConfirmDialog::Reset {
                        repository_index,
                        commit,
                        mode,
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::Reset { commit, mode },
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::Reset,
                            },
                        );
                    }
                    ConfirmDialog::Rebase {
                        repository_index,
                        upstream,
                        ..
                    } => {
                        self.submit(
                            repository_index,
                            GitRequest::Rebase { upstream },
                            PendingJobKind::Command {
                                repository_index,
                                kind: CommandKind::Rebase,
                            },
                        );
                    }
                    ConfirmDialog::ResetModeChoice { .. }
                    | ConfirmDialog::HardResetWarning { .. } => {}
                }
            }
            GlobalMode::TypingConfirmation {
                dialog,
                expected,
                input,
                ..
            } => {
                if input != expected {
                    if let GlobalMode::TypingConfirmation {
                        validation_error, ..
                    } = &mut self.state.mode
                    {
                        *validation_error = Some(format!("Type {expected} exactly to continue"));
                    }
                    return;
                }
                self.state.close_popup();
                if let ConfirmDialog::HardResetWarning {
                    repository_index,
                    commit,
                    ..
                } = dialog
                {
                    self.submit(
                        repository_index,
                        GitRequest::Reset {
                            commit,
                            mode: ResetMode::Hard,
                        },
                        PendingJobKind::Command {
                            repository_index,
                            kind: CommandKind::Reset,
                        },
                    );
                }
            }
            _ => {}
        }
    }

    fn dispatch_normal(&mut self, action: Action) {
        match action {
            Action::MoveUp => self.move_up(),
            Action::MoveDown => self.move_down(),
            Action::PageUp => self.page_up(),
            Action::PageDown => self.page_down(),
            Action::Home => self.move_home(),
            Action::End => self.move_end(),
            Action::MoveLeft if self.state.focus_context().kind == FocusKind::Changes => {
                self.collapse_selected_change_node()
            }
            Action::MoveRight if self.state.focus_context().kind == FocusKind::Changes => {
                self.expand_selected_change_node()
            }
            Action::MoveLeft => self.navigate_left(),
            Action::MoveRight => self.navigate_right(),
            Action::FocusPrev => self.focus_previous(),
            Action::FocusNext => self.focus_next(),
            Action::Back => self.back(),
            Action::RefreshRepository => {
                self.refresh_all_repositories();
                if self.state.view_projection().view == ViewId::Reflog
                    && let Some(repository_index) = self.state.reflog_repository_index
                {
                    self.submit_reflog(repository_index);
                }
                if self.state.view_projection().view == ViewId::Changes
                    && let Some(repository_index) = self.state.changes_repository_index
                {
                    self.submit_changes(repository_index);
                }
                if self.state.view_projection().view == ViewId::Remotes
                    && let Some(repository_index) = self.state.remotes_repository_index
                {
                    self.submit_remotes(repository_index);
                }
            }
            Action::StartFilter => self.start_filter(),
            Action::SelectBranch(index) => {
                self.state.selection.selected_branch_index = Some(index);
                self.state.ensure_valid_branch_selection();
                self.preview_selected_branch_commits();
            }
            Action::LoadCommitsForSelectedBranch => self.activate_selected_tree_node(),
            Action::OpenFetchRepositoryDialog => self.open_fetch_dialog(),
            Action::OpenPullRebaseDialog => self.open_pull_rebase_dialog(),
            Action::OpenPushDialog => self.open_push_dialog(),
            Action::OpenRemotes => self.open_remotes(),
            Action::OpenAddRemoteEditor => self.open_add_remote_editor(),
            Action::OpenSetRemoteUrlEditor => self.open_set_remote_url_editor(),
            Action::OpenSetUpstreamRemoteDialog => self.open_set_upstream_remote_dialog(),
            Action::OpenReflog => self.open_reflog(),
            Action::ToggleChanges => self.toggle_changes(),
            Action::ActivateSelectedChange => self.activate_selected_change(),
            Action::ToggleChangeSelection => self.toggle_change_selection(),
            Action::StageSelectedChanges => self.stage_selected_changes(),
            Action::UnstageSelectedChanges => self.unstage_selected_changes(),
            Action::OpenCommitDialog => self.open_commit_dialog(),
            Action::OpenSwitchBranchDialog => self.open_switch_dialog(),
            Action::OpenRebaseDialog => self.open_rebase_dialog(),
            Action::SelectCommit(index) => {
                self.state.selection.selected_commit_index = Some(index);
                self.state.ensure_valid_commit_selection();
                self.preview_selected_commit_detail();
            }
            Action::OpenCommitDetail => self.open_commit_detail(),
            Action::ToggleFileExpanded => self.toggle_file_expanded(),
            Action::OpenSelectedFileDiff => self.submit_selected_file_diff(true),
            Action::ToggleDiffMode => {
                self.state.diff_mode = match self.state.diff_mode {
                    DiffViewMode::Unified => DiffViewMode::SideBySide,
                    DiffViewMode::SideBySide => DiffViewMode::Unified,
                };
                if self.state.view_projection().view == ViewId::Changes {
                    self.state.selection.changes_diff_scroll = 0;
                } else {
                    self.state.selection.diff_scroll = 0;
                }
            }
            Action::NextFile => self.move_file(1),
            Action::PrevFile => self.move_file(-1),
            Action::ToggleWrap => self.state.wrap_diff = !self.state.wrap_diff,
            Action::ToggleCommitSelection => self.toggle_commit_selection(),
            Action::BeginChord(prefix) => {
                self.state.mode = GlobalMode::Chord {
                    prefix,
                    started_at: Instant::now(),
                };
            }
            Action::OpenShortcutHelp => {
                self.state.open_popup();
                self.state.mode = GlobalMode::ShortcutHelp { scroll: 0 };
            }
            Action::OpenOperationPalette => {
                let operations = OperationId::ALL
                    .iter()
                    .copied()
                    .filter(|operation| *operation != OperationId::AppOperationPalette)
                    .filter(|operation| operation.action(&self.state).is_some())
                    .collect();
                self.state.open_popup();
                self.state.mode = GlobalMode::OperationPalette {
                    query: String::new(),
                    selected: 0,
                    operations,
                };
            }
            Action::OpenCommandPrompt => {
                self.state.open_popup();
                self.state.mode = GlobalMode::CommandPrompt {
                    input: String::new(),
                    validation_error: None,
                };
            }
            Action::CopySelectedCommitHashes => self.copy_selected_commit_hashes(),
            Action::CopyCurrentCommitInfo => self.copy_current_commit_info(),
            Action::CopyCurrentCommitMessage => self.copy_current_commit_message(),
            Action::CopySelectedFileName => self.copy_selected_file_name(),
            Action::CopySelectedFileAbsolutePath => self.copy_selected_file_absolute_path(),
            Action::CopySelectedFileRelativePath => self.copy_selected_file_relative_path(),
            Action::OpenCherryPickSelectedDialog => self.open_cherry_pick_selected_dialog(),
            Action::OpenResetDialog => self.open_reset_dialog(),
            Action::DismissError => self.state.dismiss_error(),
            Action::Confirm
            | Action::Cancel
            | Action::UpdateFilter(_)
            | Action::SubmitFilter
            | Action::CancelFilter
            | Action::ChooseResetSoft
            | Action::ChooseResetMixed
            | Action::ChooseResetHard
            | Action::UpdateTypedConfirmation(_)
            | Action::ConfirmReset
            | Action::UpdateCommitMessage(_)
            | Action::SubmitCommit
            | Action::UpdateCommandPrompt(_)
            | Action::SubmitCommandPrompt
            | Action::UpdateOperationPalette(_)
            | Action::SubmitOperationPalette
            | Action::UpdateRemoteName(_)
            | Action::UpdateRemoteUrl(_)
            | Action::FocusNextRemoteField
            | Action::SubmitRemoteEditor
            | Action::Tick
            | Action::Quit => {}
        }
    }

    fn move_selection(index: &mut Option<usize>, length: usize, delta: isize) {
        if length == 0 {
            *index = None;
            return;
        }
        let current = index.unwrap_or(0);
        *index = Some(current.saturating_add_signed(delta).min(length - 1));
    }

    fn move_branch_selection(&mut self, delta: isize) {
        let before = self.state.selection.selected_branch_index;
        let length = self.state.visible_tree_nodes().len();
        Self::move_selection(
            &mut self.state.selection.selected_branch_index,
            length,
            delta,
        );
        if self.state.selection.selected_branch_index != before {
            self.preview_selected_branch_commits();
        }
    }

    fn move_commit_selection(&mut self, delta: isize) {
        let before = self.state.selection.selected_commit_index;
        let length = self.state.visible_commit_indices().len();
        Self::move_selection(
            &mut self.state.selection.selected_commit_index,
            length,
            delta,
        );
        if self.state.selection.selected_commit_index != before {
            self.preview_selected_commit_detail();
        }
    }

    fn move_file_selection(&mut self, delta: isize) {
        let length = self.state.current_file_count();
        Self::move_selection(&mut self.state.selection.selected_file_index, length, delta);
    }

    fn move_reflog_selection(&mut self, delta: isize) {
        let length = self.state.reflog_entries().len();
        Self::move_selection(
            &mut self.state.selection.selected_reflog_index,
            length,
            delta,
        );
    }

    fn move_remote_selection(&mut self, delta: isize) {
        let length = self.state.remotes().len();
        Self::move_selection(
            &mut self.state.selection.selected_remote_index,
            length,
            delta,
        );
    }

    fn move_up(&mut self) {
        let focus = self.state.focus_context();
        match (focus.kind, focus.role) {
            (FocusKind::Repository | FocusKind::Branch, _) => self.move_branch_selection(-1),
            (FocusKind::Commit, _) => self.move_commit_selection(-1),
            (FocusKind::File, FocusRole::Collection) => self.move_file_selection(-1),
            (FocusKind::File, _) => self.move_file(-1),
            (FocusKind::Diff, _) => {
                self.state.selection.diff_scroll =
                    self.state.selection.diff_scroll.saturating_sub(1);
            }
            (FocusKind::Reflog, _) => self.move_reflog_selection(-1),
            (FocusKind::Remote, _) => self.move_remote_selection(-1),
            (FocusKind::Changes, _) => self.move_change_node(-1),
            (FocusKind::ChangesDiff, _) => {
                self.state.selection.changes_diff_scroll =
                    self.state.selection.changes_diff_scroll.saturating_sub(1);
            }
        }
    }

    fn move_down(&mut self) {
        let focus = self.state.focus_context();
        match (focus.kind, focus.role) {
            (FocusKind::Repository | FocusKind::Branch, _) => self.move_branch_selection(1),
            (FocusKind::Commit, _) => self.move_commit_selection(1),
            (FocusKind::File, FocusRole::Collection) => self.move_file_selection(1),
            (FocusKind::File, _) => self.move_file(1),
            (FocusKind::Diff, _) => {
                let maximum = Self::maximum_scroll(self.state.diff_line_count());
                self.state.selection.diff_scroll = self
                    .state
                    .selection
                    .diff_scroll
                    .saturating_add(1)
                    .min(maximum);
            }
            (FocusKind::Reflog, _) => self.move_reflog_selection(1),
            (FocusKind::Remote, _) => self.move_remote_selection(1),
            (FocusKind::Changes, _) => self.move_change_node(1),
            (FocusKind::ChangesDiff, _) => {
                let maximum = Self::maximum_scroll(self.state.changes_diff_line_count());
                self.state.selection.changes_diff_scroll = self
                    .state
                    .selection
                    .changes_diff_scroll
                    .saturating_add(1)
                    .min(maximum);
            }
        }
    }

    fn page_up(&mut self) {
        let focus = self.state.focus_context();
        match (focus.kind, focus.role) {
            (FocusKind::Repository | FocusKind::Branch, _) => {
                self.move_branch_selection(-(PAGE_SIZE as isize))
            }
            (FocusKind::Commit, _) => self.move_commit_selection(-(PAGE_SIZE as isize)),
            (FocusKind::File, FocusRole::Collection) => {
                self.move_file_selection(-(PAGE_SIZE as isize))
            }
            (FocusKind::File, _) => self.move_file(-(PAGE_SIZE as isize)),
            (FocusKind::Diff, _) => {
                self.state.selection.diff_scroll = self
                    .state
                    .selection
                    .diff_scroll
                    .saturating_sub(PAGE_SIZE as u16);
            }
            (FocusKind::ChangesDiff, _) => {
                self.state.selection.changes_diff_scroll = self
                    .state
                    .selection
                    .changes_diff_scroll
                    .saturating_sub(PAGE_SIZE as u16);
            }
            (FocusKind::Changes, _) => self.move_change_node(-(PAGE_SIZE as isize)),
            _ => {
                for _ in 0..PAGE_SIZE {
                    self.move_up();
                }
            }
        }
    }

    fn page_down(&mut self) {
        let focus = self.state.focus_context();
        match (focus.kind, focus.role) {
            (FocusKind::Repository | FocusKind::Branch, _) => {
                self.move_branch_selection(PAGE_SIZE as isize)
            }
            (FocusKind::Commit, _) => self.move_commit_selection(PAGE_SIZE as isize),
            (FocusKind::File, FocusRole::Collection) => {
                self.move_file_selection(PAGE_SIZE as isize)
            }
            (FocusKind::File, _) => self.move_file(PAGE_SIZE as isize),
            (FocusKind::Diff, _) => {
                let maximum = Self::maximum_scroll(self.state.diff_line_count());
                self.state.selection.diff_scroll = self
                    .state
                    .selection
                    .diff_scroll
                    .saturating_add(PAGE_SIZE as u16)
                    .min(maximum);
            }
            (FocusKind::ChangesDiff, _) => {
                let maximum = Self::maximum_scroll(self.state.changes_diff_line_count());
                self.state.selection.changes_diff_scroll = self
                    .state
                    .selection
                    .changes_diff_scroll
                    .saturating_add(PAGE_SIZE as u16)
                    .min(maximum);
            }
            (FocusKind::Changes, _) => self.move_change_node(PAGE_SIZE as isize),
            _ => {
                for _ in 0..PAGE_SIZE {
                    self.move_down();
                }
            }
        }
    }

    fn move_home(&mut self) {
        let focus = self.state.focus_context();
        match (focus.kind, focus.role) {
            (FocusKind::Repository | FocusKind::Branch, _) => {
                let before = self.state.selection.selected_branch_index;
                self.state.selection.selected_branch_index = Some(0);
                self.state.ensure_valid_branch_selection();
                if self.state.selection.selected_branch_index != before {
                    self.preview_selected_branch_commits();
                }
            }
            (FocusKind::Commit, _) => {
                let before = self.state.selection.selected_commit_index;
                self.state.selection.selected_commit_index = Some(0);
                self.state.ensure_valid_commit_selection();
                if self.state.selection.selected_commit_index != before {
                    self.preview_selected_commit_detail();
                }
            }
            (FocusKind::File, _) => {
                self.state.selection.selected_file_index = Some(0);
                self.state.selection.diff_scroll = 0;
            }
            (FocusKind::Diff, _) => self.state.selection.diff_scroll = 0,
            (FocusKind::Reflog, _) => self.state.selection.selected_reflog_index = Some(0),
            (FocusKind::Remote, _) => self.state.selection.selected_remote_index = Some(0),
            (FocusKind::Changes, _) => {
                self.state.selection.selected_changes_index = Some(0);
                self.state.ensure_valid_changes_selection();
                self.submit_selected_change_diff();
            }
            (FocusKind::ChangesDiff, _) => self.state.selection.changes_diff_scroll = 0,
        }
        self.state.ensure_valid_commit_selection();
        self.state.ensure_valid_file_selection();
        self.state.ensure_valid_reflog_selection();
        self.state.ensure_valid_remote_selection();
        self.state.ensure_valid_changes_selection();
    }

    fn move_end(&mut self) {
        let focus = self.state.focus_context();
        match (focus.kind, focus.role) {
            (FocusKind::Repository | FocusKind::Branch, _) => {
                let before = self.state.selection.selected_branch_index;
                self.state.selection.selected_branch_index =
                    self.state.visible_tree_nodes().len().checked_sub(1);
                if self.state.selection.selected_branch_index != before {
                    self.preview_selected_branch_commits();
                }
            }
            (FocusKind::Commit, _) => {
                let before = self.state.selection.selected_commit_index;
                self.state.selection.selected_commit_index =
                    self.state.visible_commit_indices().len().checked_sub(1);
                if self.state.selection.selected_commit_index != before {
                    self.preview_selected_commit_detail();
                }
            }
            (FocusKind::File, _) => {
                self.state.selection.selected_file_index =
                    self.state.current_file_count().checked_sub(1);
                self.state.selection.diff_scroll = 0;
            }
            (FocusKind::Diff, _) => {
                self.state.selection.diff_scroll =
                    Self::maximum_scroll(self.state.diff_line_count());
            }
            (FocusKind::Reflog, _) => {
                self.state.selection.selected_reflog_index =
                    self.state.reflog_entries().len().checked_sub(1);
            }
            (FocusKind::Remote, _) => {
                self.state.selection.selected_remote_index =
                    self.state.remotes().len().checked_sub(1);
            }
            (FocusKind::Changes, _) => {
                self.state.selection.selected_changes_index =
                    self.state.visible_changes_nodes().len().checked_sub(1);
                self.submit_selected_change_diff();
            }
            (FocusKind::ChangesDiff, _) => {
                self.state.selection.changes_diff_scroll =
                    Self::maximum_scroll(self.state.changes_diff_line_count());
            }
        }
    }

    fn maximum_scroll(line_count: usize) -> u16 {
        u16::try_from(line_count.saturating_sub(1)).unwrap_or(u16::MAX)
    }

    fn focus_next(&mut self) {
        let focus = self.state.focus_context();
        let (kind, role) = match (focus.kind, focus.role) {
            (FocusKind::Repository | FocusKind::Branch, _) => {
                (FocusKind::Commit, FocusRole::Collection)
            }
            (FocusKind::Commit, FocusRole::Collection) => {
                (self.history_tree_focus(), FocusRole::Entity)
            }
            (FocusKind::Commit, _) => (FocusKind::File, FocusRole::Collection),
            (FocusKind::File, FocusRole::Collection) => (FocusKind::Commit, FocusRole::Entity),
            (FocusKind::File, _) => (FocusKind::Diff, FocusRole::Content),
            (FocusKind::Diff, _) => (FocusKind::File, FocusRole::Entity),
            (FocusKind::Reflog, _) => (FocusKind::Reflog, FocusRole::Entity),
            (FocusKind::Remote, _) => (FocusKind::Remote, FocusRole::Entity),
            (FocusKind::Changes, _) => (FocusKind::ChangesDiff, FocusRole::Content),
            (FocusKind::ChangesDiff, _) => (FocusKind::Changes, FocusRole::Entity),
        };
        self.state.set_focus_layer(kind, role);
    }

    fn focus_previous(&mut self) {
        self.focus_next();
    }

    /// Navigate one column to the right. When the focused column is already
    /// the right-hand column, slide the two-column window one level deeper:
    /// the old right column becomes the new left column and keeps focus.
    fn navigate_right(&mut self) {
        let focus = self.state.focus_context();
        match (focus.kind, focus.role) {
            (FocusKind::Repository | FocusKind::Branch, _) => {
                self.state
                    .set_focus_layer(FocusKind::Commit, FocusRole::Collection);
            }
            (FocusKind::Commit, FocusRole::Collection) => {
                self.shift_to_commit_detail();
            }
            (FocusKind::Commit, _) => {
                self.state
                    .set_focus_layer(FocusKind::File, FocusRole::Collection);
            }
            (FocusKind::File, FocusRole::Collection) => {
                self.shift_to_file_diff();
            }
            (FocusKind::File, _) => {
                self.state
                    .set_focus_layer(FocusKind::Diff, FocusRole::Content);
            }
            _ => {}
        }
    }

    /// Navigate one column to the left. Crossing a screen boundary is the
    /// exact inverse of `navigate_right`: the current left column becomes the
    /// previous screen's right column and retains focus.
    fn navigate_left(&mut self) {
        let focus = self.state.focus_context();
        match (focus.kind, focus.role) {
            (FocusKind::Commit, FocusRole::Collection) => {
                self.state
                    .set_focus_layer(self.history_tree_focus(), FocusRole::Entity);
            }
            (FocusKind::File, FocusRole::Collection) => {
                self.state
                    .set_focus_layer(FocusKind::Commit, FocusRole::Entity);
            }
            (FocusKind::Commit, _) => {
                self.state
                    .set_focus_layer(FocusKind::Commit, FocusRole::Collection);
            }
            (FocusKind::Diff, _) => {
                self.state
                    .set_focus_layer(FocusKind::File, FocusRole::Entity);
            }
            (FocusKind::File, _) => {
                self.state
                    .set_focus_layer(FocusKind::File, FocusRole::Collection);
            }
            (FocusKind::ChangesDiff, _) => {
                self.state
                    .set_focus_layer(FocusKind::Changes, FocusRole::Entity);
            }
            _ => {}
        }
    }

    fn back(&mut self) {
        match self.state.view_projection().view {
            ViewId::History => {}
            ViewId::Commit => {
                self.state
                    .set_focus_layer(FocusKind::Commit, FocusRole::Collection);
            }
            ViewId::FileDiff => {
                self.state
                    .set_focus_layer(FocusKind::File, FocusRole::Collection);
            }
            ViewId::Reflog => {
                self.state
                    .set_focus_layer(self.history_tree_focus(), FocusRole::Entity);
            }
            ViewId::Remotes => {
                self.state
                    .set_focus_layer(self.history_tree_focus(), FocusRole::Entity);
            }
            ViewId::Changes => {
                if let Some(context) = self.state.changes_return_context.take() {
                    self.state.restore_focus_context(context);
                } else {
                    self.state
                        .set_focus_layer(self.history_tree_focus(), FocusRole::Entity);
                }
            }
        }
    }

    fn history_tree_focus(&self) -> FocusKind {
        if self.state.selected_branch_id().is_some() {
            FocusKind::Branch
        } else {
            FocusKind::Repository
        }
    }

    fn start_filter(&mut self) {
        let (target, query) = match self.state.focus_context().kind {
            FocusKind::Repository | FocusKind::Branch => {
                (FilterTarget::Branches, self.state.branch_filter.clone())
            }
            FocusKind::Commit => (FilterTarget::Commits, self.state.commit_filter.clone()),
            _ => return,
        };
        self.state.mode = GlobalMode::Filtering { target, query };
    }

    fn activate_repository(&mut self, repository_index: usize) {
        if self.state.repository_ui.get(repository_index).is_none()
            || self.state.active_repository_index == Some(repository_index)
        {
            return;
        }
        self.state.active_repository_index = Some(repository_index);
        self.state.viewing_branch = None;
        self.state.reflog_repository_index = None;
        self.state.remotes_repository_index = None;
        self.state.changes_repository_index = None;
        self.state.current_changes_diff = None;
        self.state.current_changes_diff_group = None;
        self.state.change_selection.clear();
        self.state.changes_return_context = None;
        self.state.latest_commits_job = None;
        self.state.latest_commit_detail_job = None;
        self.state.latest_commit_message_job = None;
        self.state.latest_file_diff_job = None;
        self.state.latest_reflog_job = None;
        self.state.latest_remotes_job = None;
        self.state.latest_changes_job = None;
        self.state.latest_changes_diff_job = None;
        self.state.commit_filter.clear();
        self.state.selection.selected_commit_index = None;
        self.state.selection.selected_file_index = None;
        self.state.selection.selected_reflog_index = None;
        self.state.selection.selected_remote_index = None;
        self.state.selection.selected_changes_index = None;
        if matches!(
            self.state.focus_context().kind,
            FocusKind::Repository | FocusKind::Branch
        ) {
            self.state
                .set_focus_layer(self.history_tree_focus(), FocusRole::Entity);
        } else {
            self.state
                .set_focus_layer(FocusKind::Commit, FocusRole::Collection);
        }
        if self.state.commit_selection_repository_index != Some(repository_index) {
            self.state.commit_selection.clear();
            self.state.commit_selection_repository_index = None;
        }
        self.load_active_repository_commits(repository_index);
    }

    /// Selection-driven branch preview. Unlike Enter, this never moves focus
    /// into the commit list. It changes only the semantic branch scope; the
    /// post-action DataRequirement reconciliation owns any required load.
    fn preview_selected_branch_commits(&mut self) {
        let selected = match self.state.selected_tree_node() {
            Some(BranchTreeNode::Repository { repository_index }) => {
                self.activate_repository(repository_index);
                return;
            }
            Some(BranchTreeNode::Branch {
                repository_index,
                branch_index,
            }) => self
                .state
                .repository_branch(repository_index, branch_index)
                .map(|branch| {
                    (
                        repository_index,
                        branch.name.clone(),
                        !branch.head.0.is_empty(),
                    )
                }),
            None => None,
        };
        let Some((repository_index, branch, has_commit)) = selected else {
            return;
        };

        self.activate_repository(repository_index);
        let branch_id = BranchId {
            repository: RepositoryId(repository_index),
            name: branch.clone(),
        };
        if self.state.viewing_branch.as_ref() == Some(&branch_id) {
            if !has_commit {
                self.state
                    .model
                    .replace_branch_commits(&branch_id, Vec::new());
            }
            self.state.ensure_valid_commit_selection();
            return;
        }

        if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
            repository.viewing_branch = Some(branch.clone());
        }
        self.state.viewing_branch = Some(branch_id.clone());
        self.state.commit_filter.clear();
        self.state.selection.selected_commit_index = None;
        self.state.selection.selected_file_index = None;
        self.state.commit_selection.clear();
        self.state.commit_selection_repository_index = Some(repository_index);

        if !has_commit {
            // Invalidate any older in-flight commit-list response before
            // showing an unborn branch as an empty list.
            self.state.latest_commits_job = None;
            self.state
                .model
                .replace_branch_commits(&branch_id, Vec::new());
            self.state.ensure_valid_commit_selection();
        }
    }

    fn activate_selected_tree_node(&mut self) {
        match self.state.selected_tree_node() {
            Some(BranchTreeNode::Repository { repository_index }) => {
                self.activate_repository(repository_index);
                if let Some(repository) = self.state.repository_ui.get_mut(repository_index) {
                    repository.expanded = !repository.expanded;
                }
                self.state.ensure_valid_branch_selection();
            }
            Some(BranchTreeNode::Branch { .. }) => {
                self.preview_selected_branch_commits();
                self.state
                    .set_focus_layer(FocusKind::Commit, FocusRole::Collection);
            }
            None => {}
        }
    }

    fn open_fetch_dialog(&mut self) {
        let Some(repository_index) = self.state.selected_repository_node_index() else {
            return;
        };
        self.activate_repository(repository_index);
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::FetchRepository { repository_index },
        };
    }

    fn open_remotes(&mut self) {
        let Some(repository_index) = self.state.selected_tree_repository_index() else {
            return;
        };
        self.activate_repository(repository_index);
        self.state.remotes_repository_index = Some(repository_index);
        self.state
            .set_focus_layer(FocusKind::Remote, FocusRole::Entity);
        self.state.ensure_valid_remote_selection();
        self.submit_remotes(repository_index);
    }

    fn open_add_remote_editor(&mut self) {
        let Some(repository_index) = self.state.remotes_repository_index else {
            return;
        };
        if self.has_pending_repository_command(repository_index) {
            self.state.last_message = Some("Another Git operation is still running".into());
            return;
        }
        self.state.open_popup();
        self.state.mode = GlobalMode::EditingRemote {
            kind: RemoteEditKind::Add,
            field: RemoteInputField::Name,
            name: String::new(),
            url: String::new(),
            validation_error: None,
        };
    }

    fn open_set_remote_url_editor(&mut self) {
        let Some(repository_index) = self.state.remotes_repository_index else {
            return;
        };
        if self.has_pending_repository_command(repository_index) {
            self.state.last_message = Some("Another Git operation is still running".into());
            return;
        }
        let Some(remote) = self.state.selected_remote() else {
            return;
        };
        let name = remote.name.clone();
        let url = remote.fetch_urls.first().cloned().unwrap_or_default();
        self.state.open_popup();
        self.state.mode = GlobalMode::EditingRemote {
            kind: RemoteEditKind::SetUrl {
                remote_name: name.clone(),
            },
            field: RemoteInputField::Url,
            name,
            url,
            validation_error: None,
        };
    }

    fn open_set_upstream_remote_dialog(&mut self) {
        let Some(repository_index) = self.state.remotes_repository_index else {
            return;
        };
        if self.has_pending_repository_command(repository_index) {
            self.state.last_message = Some("Another Git operation is still running".into());
            return;
        }
        let Some(remote) = self.state.selected_remote() else {
            return;
        };
        if !remote.urls_match() {
            self.state.set_error(
                "set upstream remote".into(),
                format!(
                    "Remote `{}` has different fetch and push URLs. Press `e` in Remote Management to set one shared URL first.",
                    remote.name
                ),
            );
            return;
        }
        let Some(branch) = self
            .state
            .repository(repository_index)
            .and_then(|repository| repository.current_branch.clone())
        else {
            self.state.set_error(
                "set upstream remote".into(),
                "Setting upstream requires an attached current branch; detached HEAD is not supported."
                    .into(),
            );
            return;
        };
        let name = remote.name.clone();
        if remote.is_upstream && remote.is_push_target {
            self.state.last_message = Some(format!(
                "Remote `{name}` is already upstream for both fetch and push"
            ));
            return;
        }
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::SetUpstreamRemote {
                repository_index,
                name,
                branch,
            },
        };
    }

    fn open_pull_rebase_dialog(&mut self) {
        let Some(repository_index) = self.state.selected_repository_node_index() else {
            return;
        };
        self.activate_repository(repository_index);
        if self.has_pending_repository_command(repository_index) {
            self.state.last_message = Some("Another Git operation is still running".into());
            return;
        }
        let Some(repository) = self.state.repository(repository_index) else {
            return;
        };
        let Some(branch) = repository.current_branch.clone() else {
            self.state.set_error(
                "git pull --rebase".into(),
                "Pull --rebase requires an attached current branch; detached HEAD is not supported."
                    .into(),
            );
            return;
        };
        if !repository.status.is_clean() {
            self.state.set_error(
                "git pull --rebase".into(),
                "Pull --rebase requires a clean working tree and index. Commit, stash, or discard all changes first."
                    .into(),
            );
            return;
        }
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::PullRebaseRepository {
                repository_index,
                branch,
            },
        };
    }

    fn open_push_dialog(&mut self) {
        let Some(repository_index) = self.state.selected_repository_node_index() else {
            return;
        };
        self.activate_repository(repository_index);
        if self.has_pending_repository_command(repository_index) {
            self.state.last_message = Some("Another Git operation is still running".into());
            return;
        }
        let Some(branch) = self
            .state
            .repository(repository_index)
            .and_then(|repository| repository.current_branch.clone())
        else {
            self.state.set_error(
                "git push".into(),
                "Push requires an attached current branch; detached HEAD is not supported.".into(),
            );
            return;
        };
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::PushRepository {
                repository_index,
                branch,
            },
        };
    }

    fn open_reflog(&mut self) {
        let Some(repository_index) = self.state.selected_repository_node_index() else {
            return;
        };
        self.activate_repository(repository_index);
        self.state.reflog_repository_index = Some(repository_index);
        self.state.selection.selected_reflog_index = None;
        self.state
            .set_focus_layer(FocusKind::Reflog, FocusRole::Entity);
        self.submit_reflog(repository_index);
    }

    fn toggle_changes(&mut self) {
        if self.state.view_projection().view == ViewId::Changes {
            self.back();
            return;
        }
        let Some(repository_index) = self.state.active_repository_index else {
            return;
        };
        self.state.changes_return_context = Some(self.state.focus_context());
        self.state.changes_repository_index = Some(repository_index);
        self.state.current_changes_diff = None;
        self.state.current_changes_diff_group = None;
        self.state.selection.changes_diff_scroll = 0;
        self.state
            .set_focus_layer(FocusKind::Changes, FocusRole::Entity);
        if self.state.working_tree_changes().is_empty() {
            // Do not turn the implicit pre-load root into an intentional user
            // selection. Once data arrives, selection should land on the
            // first file so its diff is immediately useful.
            self.state.selection.selected_changes_index = None;
        } else {
            self.state.ensure_valid_changes_selection();
        }
        self.submit_changes(repository_index);
    }

    fn select_changes_node(&mut self, node: ChangesTreeNode) {
        self.state.selection.selected_changes_index = self
            .state
            .visible_changes_nodes()
            .iter()
            .position(|candidate| *candidate == node);
        self.state.ensure_valid_changes_selection();
        self.submit_selected_change_diff();
    }

    fn toggle_change_node_expansion(&mut self, node: ChangesTreeNode) {
        match node {
            ChangesTreeNode::Root => {
                self.state.expansion.changes_root_expanded =
                    !self.state.expansion.changes_root_expanded;
            }
            ChangesTreeNode::Group(ChangeGroup::Staged) => {
                self.state.expansion.staged_changes_expanded =
                    !self.state.expansion.staged_changes_expanded;
            }
            ChangesTreeNode::Group(ChangeGroup::Unstaged) => {
                self.state.expansion.unstaged_changes_expanded =
                    !self.state.expansion.unstaged_changes_expanded;
            }
            ChangesTreeNode::File { .. } => return,
        }
        self.select_changes_node(node);
    }

    fn activate_selected_change(&mut self) {
        let Some(node) = self.state.selected_changes_node() else {
            return;
        };
        match node {
            ChangesTreeNode::Root | ChangesTreeNode::Group(_) => {
                self.toggle_change_node_expansion(node)
            }
            ChangesTreeNode::File { .. } => {
                self.submit_selected_change_diff();
                self.state
                    .set_focus_layer(FocusKind::ChangesDiff, FocusRole::Content);
            }
        }
    }

    fn toggle_change_selection(&mut self) {
        let Some(node) = self.state.selected_changes_node() else {
            return;
        };
        let targets = match node {
            ChangesTreeNode::Root => [ChangeGroup::Staged, ChangeGroup::Unstaged]
                .into_iter()
                .flat_map(|group| self.state.change_selections_in_group(group))
                .collect::<Vec<_>>(),
            ChangesTreeNode::Group(group) => self.state.change_selections_in_group(group),
            ChangesTreeNode::File {
                group,
                change_index,
            } => self
                .state
                .working_tree_changes()
                .get(change_index)
                .map(|change| vec![AppState::change_selection_key(group, change)])
                .unwrap_or_default(),
        };
        if targets.is_empty() {
            return;
        }
        let all_selected = targets
            .iter()
            .all(|selection| self.state.change_selection.contains(selection));
        if all_selected {
            for selection in targets {
                self.state.change_selection.remove(&selection);
            }
        } else {
            self.state.change_selection.extend(targets);
        }
        self.state.last_message = Some(format!(
            "{} change entr{} selected",
            self.state.change_selection.len(),
            if self.state.change_selection.len() == 1 {
                "y"
            } else {
                "ies"
            }
        ));
    }

    fn selected_paths_for_change_operation(&self, group: ChangeGroup) -> Vec<GitPath> {
        let wanted = if self.state.change_selection.is_empty() {
            self.state
                .selected_change()
                .filter(|(selected_group, _)| *selected_group == group)
                .map(|(_, change)| vec![AppState::change_selection_key(group, change)])
                .unwrap_or_default()
        } else {
            self.state
                .change_selection
                .iter()
                .filter(|selection| selection.group == group)
                .cloned()
                .collect::<Vec<ChangeSelection>>()
        };
        let wanted = wanted.into_iter().collect::<HashSet<_>>();
        let mut seen = HashSet::new();
        let mut paths = Vec::new();
        for change in self.state.working_tree_changes() {
            if !wanted.contains(&AppState::change_selection_key(group, change)) {
                continue;
            }
            for path in std::iter::once(&change.path).chain(change.old_path.as_ref()) {
                if seen.insert(path.clone()) {
                    paths.push(path.clone());
                }
            }
        }
        paths
    }

    fn has_pending_repository_command(&self, repository_index: usize) -> bool {
        self.state.pending_jobs.values().any(|pending| {
            matches!(
                pending,
                PendingJobKind::Command {
                    repository_index: index,
                    ..
                } if *index == repository_index
            )
        })
    }

    fn stage_selected_changes(&mut self) {
        let Some(repository_index) = self.state.changes_repository_index else {
            return;
        };
        if self.has_pending_repository_command(repository_index) {
            self.state.last_message = Some("Another Git operation is still running".into());
            return;
        }
        let paths = self.selected_paths_for_change_operation(ChangeGroup::Unstaged);
        if paths.is_empty() {
            self.state.last_message = Some("Select an unstaged file before running stage".into());
            return;
        }
        self.submit(
            repository_index,
            GitRequest::StagePaths { paths },
            PendingJobKind::Command {
                repository_index,
                kind: CommandKind::Stage,
            },
        );
    }

    fn unstage_selected_changes(&mut self) {
        let Some(repository_index) = self.state.changes_repository_index else {
            return;
        };
        if self.has_pending_repository_command(repository_index) {
            self.state.last_message = Some("Another Git operation is still running".into());
            return;
        }
        let paths = self.selected_paths_for_change_operation(ChangeGroup::Staged);
        if paths.is_empty() {
            self.state.last_message = Some("Select a staged file before running unstage".into());
            return;
        }
        self.submit(
            repository_index,
            GitRequest::UnstagePaths { paths },
            PendingJobKind::Command {
                repository_index,
                kind: CommandKind::Unstage,
            },
        );
    }

    fn open_commit_dialog(&mut self) {
        let Some(repository_index) = self.state.changes_repository_index else {
            return;
        };
        if self.has_pending_repository_command(repository_index) {
            self.state.last_message = Some("Another Git operation is still running".into());
            return;
        }
        if self.state.change_group_count(ChangeGroup::Staged) == 0 {
            self.state.last_message = Some("Stage at least one file before committing".into());
            return;
        }
        self.state.open_popup();
        self.state.mode = GlobalMode::EditingCommitMessage {
            input: String::new(),
            validation_error: None,
        };
    }

    fn collapse_selected_change_node(&mut self) {
        let Some(node) = self.state.selected_changes_node() else {
            return;
        };
        match node {
            ChangesTreeNode::Root if self.state.expansion.changes_root_expanded => {
                self.state.expansion.changes_root_expanded = false;
                self.select_changes_node(ChangesTreeNode::Root);
            }
            ChangesTreeNode::Group(group) => {
                let expanded = match group {
                    ChangeGroup::Staged => &mut self.state.expansion.staged_changes_expanded,
                    ChangeGroup::Unstaged => &mut self.state.expansion.unstaged_changes_expanded,
                };
                if *expanded {
                    *expanded = false;
                    self.select_changes_node(node);
                } else {
                    self.select_changes_node(ChangesTreeNode::Root);
                }
            }
            ChangesTreeNode::File { group, .. } => {
                self.select_changes_node(ChangesTreeNode::Group(group));
            }
            ChangesTreeNode::Root => {}
        }
    }

    fn expand_selected_change_node(&mut self) {
        let Some(node) = self.state.selected_changes_node() else {
            return;
        };
        match node {
            ChangesTreeNode::Root if !self.state.expansion.changes_root_expanded => {
                self.state.expansion.changes_root_expanded = true;
                self.select_changes_node(node);
            }
            ChangesTreeNode::Group(group) => {
                let expanded = match group {
                    ChangeGroup::Staged => &mut self.state.expansion.staged_changes_expanded,
                    ChangeGroup::Unstaged => &mut self.state.expansion.unstaged_changes_expanded,
                };
                if !*expanded {
                    *expanded = true;
                    self.select_changes_node(node);
                }
            }
            _ => {}
        }
    }

    fn open_switch_dialog(&mut self) {
        let Some((repository_index, branch)) = self
            .state
            .selected_branch_with_repository()
            .map(|(index, branch)| (index, branch.name.clone()))
        else {
            return;
        };
        self.activate_repository(repository_index);
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::SwitchBranch {
                repository_index,
                branch,
            },
        };
    }

    fn open_rebase_dialog(&mut self) {
        let Some((repository_index, upstream)) = self
            .state
            .selected_branch_with_repository()
            .map(|(index, branch)| (index, branch.name.clone()))
        else {
            return;
        };
        self.activate_repository(repository_index);
        let Some(repository) = self.state.repository(repository_index) else {
            return;
        };
        let Some(current_branch) = repository.current_branch.clone() else {
            self.state.set_error(
                "git rebase".into(),
                "Safe rebase requires an attached current branch; detached HEAD is not supported."
                    .into(),
            );
            return;
        };
        if current_branch == upstream {
            self.state.set_error(
                format!("git rebase {}", upstream.0),
                "The selected upstream is already the current branch.".into(),
            );
            return;
        }
        if !repository.status.is_clean() {
            self.state.set_error(
                format!("git rebase {}", upstream.0),
                "Safe rebase requires a clean working tree and index. Commit, stash, or discard all changes first."
                    .into(),
            );
            return;
        }
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::Rebase {
                repository_index,
                current_branch,
                upstream,
            },
        };
    }

    fn open_commit_detail(&mut self) {
        let Some(repository_index) = self.state.viewing_repository_index() else {
            return;
        };
        let Some(commit) = self
            .state
            .selected_commit()
            .map(|commit| commit.hash.clone())
        else {
            return;
        };
        self.submit_commit_detail(repository_index, commit, true);
    }

    /// Slide `Branches | Commits` to `Commits | Commit` without advancing
    /// focus into the new detail column. The selected Commits column is reused
    /// as the new left column, so keyboard position remains visually stable.
    fn shift_to_commit_detail(&mut self) {
        let Some((repository_index, commit)) = self.state.viewing_repository_index().zip(
            self.state
                .selected_commit()
                .map(|commit| commit.hash.clone()),
        ) else {
            return;
        };

        self.state
            .set_focus_layer(FocusKind::Commit, FocusRole::Entity);

        // If Enter already queued the same detail request, turn it into a
        // column-shift request so its response cannot steal focus to Files.
        if let Some(id) = self.state.latest_commit_detail_job
            && let Some(PendingJobKind::CommitDetail {
                repository_index: pending_repository,
                commit: pending_commit,
                focus_files,
            }) = self.state.pending_jobs.get_mut(&id)
            && *pending_repository == repository_index
            && pending_commit == &commit
        {
            *focus_files = false;
        }

        self.preview_selected_commit_detail();
    }

    fn latest_commit_detail_request_matches(
        &self,
        repository_index: usize,
        commit: &CommitHash,
    ) -> bool {
        self.state
            .latest_commit_detail_job
            .and_then(|id| self.state.pending_jobs.get(&id))
            .is_some_and(|kind| {
                matches!(
                    kind,
                    PendingJobKind::CommitDetail {
                        repository_index: pending_repository,
                        commit: pending_commit,
                        ..
                    } if *pending_repository == repository_index && pending_commit == commit
                )
            })
    }

    /// Keep Commit Detail's right pane synchronized with the highlighted
    /// commit without transferring focus away from the left commit list.
    fn preview_selected_commit_detail(&mut self) {
        if self.state.view_projection().view != ViewId::Commit {
            return;
        }
        let selected = self.state.viewing_repository_index().zip(
            self.state
                .selected_commit()
                .map(|commit| commit.hash.clone()),
        );
        let Some((repository_index, commit)) = selected else {
            self.state.latest_commit_detail_job = None;
            self.state.selection.selected_file_index = None;
            self.state.expansion.expanded_files.clear();
            return;
        };

        if self.latest_commit_detail_request_matches(repository_index, &commit) {
            return;
        }

        self.state.selection.selected_file_index = None;
        self.state.expansion.expanded_files.clear();
    }

    /// Slide `Commits | Commit` to `Commit | Diff`. The complete Commit
    /// column (metadata plus changed files) remains selected and becomes the
    /// new left column; the diff is loaded on the right without stealing focus.
    fn shift_to_file_diff(&mut self) {
        let Some(repository_index) = self.state.viewing_repository_index() else {
            return;
        };
        let Some(commit_id) = self.state.current_commit_id() else {
            return;
        };
        let Some(file) = self.state.selected_file() else {
            return;
        };
        let commit = commit_id.hash;
        let path = file.path.clone();

        self.state
            .set_focus_layer(FocusKind::File, FocusRole::Entity);

        if self
            .state
            .current_file_diff()
            .is_some_and(|diff| diff.commit == commit && diff.path == path)
        {
            // A different older request must not replace the reusable cached
            // diff after this column shift.
            self.state.latest_file_diff_job = None;
            return;
        }

        self.state.selection.diff_scroll = 0;

        // Preserve column focus if an explicit open for the same file was
        // already queued before the second Right key arrived.
        if let Some(id) = self.state.latest_file_diff_job
            && let Some(PendingJobKind::FileDiff {
                repository_index: pending_repository,
                commit: pending_commit,
                path: pending_path,
                focus_diff,
            }) = self.state.pending_jobs.get_mut(&id)
            && *pending_repository == repository_index
            && pending_commit == &commit
            && pending_path == &path
        {
            *focus_diff = false;
        }

        // DataRequirement reconciliation after this action loads the selected
        // file if its Resource is not ready.
    }

    fn toggle_file_expanded(&mut self) {
        let Some(path) = self.state.selected_file().map(|file| file.path.clone()) else {
            return;
        };
        if !self.state.expansion.expanded_files.remove(&path) {
            self.state.expansion.expanded_files.insert(path);
        }
    }

    fn move_file(&mut self, delta: isize) {
        let length = self.state.current_file_count();
        let before = self.state.selection.selected_file_index;
        Self::move_selection(&mut self.state.selection.selected_file_index, length, delta);
        if self.state.view_projection().view == ViewId::FileDiff
            && self.state.selection.selected_file_index != before
        {
            self.state.selection.diff_scroll = 0;
        }
    }

    fn move_change_node(&mut self, delta: isize) {
        let before = self.state.selection.selected_changes_index;
        let length = self.state.visible_changes_nodes().len();
        Self::move_selection(
            &mut self.state.selection.selected_changes_index,
            length,
            delta,
        );
        if self.state.selection.selected_changes_index != before {
            self.submit_selected_change_diff();
        }
    }

    fn toggle_commit_selection(&mut self) {
        let Some(repository_index) = self.state.viewing_repository_index() else {
            return;
        };
        let Some(hash) = self
            .state
            .selected_commit()
            .map(|commit| commit.hash.clone())
        else {
            return;
        };
        if self.state.commit_selection_repository_index != Some(repository_index) {
            self.state.commit_selection.clear();
            self.state.commit_selection_repository_index = Some(repository_index);
        }
        if !self.state.commit_selection.remove(&hash) {
            self.state.commit_selection.insert(hash);
        }
    }

    fn copy_selected_commit_hashes(&mut self) {
        let hashes = self.state.selected_commit_hashes_for_copy();
        if hashes.is_empty() {
            return;
        }
        self.state.pending_clipboard = Some(
            hashes
                .iter()
                .map(|hash| hash.0.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
        self.state.last_message = Some(format!(
            "Copied {} commit hash{}",
            hashes.len(),
            if hashes.len() == 1 { "" } else { "es" }
        ));
    }

    fn copy_current_commit_info(&mut self) {
        let Some(info) = self.state.selected_commit_info_for_copy() else {
            return;
        };
        self.state.pending_clipboard = Some(info);
        self.state.last_message = Some("Copied current commit info".into());
    }

    fn copy_current_commit_message(&mut self) {
        if let Some(message) = self.state.selected_commit_message_for_copy() {
            self.state.pending_clipboard = Some(message);
            self.state.last_message = Some("Copied current commit message".into());
            return;
        }

        let Some(repository_index) = self.state.viewing_repository_index() else {
            return;
        };
        let Some(commit) = self
            .state
            .selected_commit()
            .map(|commit| commit.hash.clone())
        else {
            return;
        };
        self.submit_commit_message_for_copy(repository_index, commit);
        self.state.last_message = Some("Loading full commit message…".into());
    }

    fn selected_file_path_for_copy(&self) -> Option<(usize, GitPath)> {
        Some((
            self.state.viewing_repository_index()?,
            self.state.selected_file()?.path.clone(),
        ))
    }

    fn copy_selected_file_name(&mut self) {
        let Some((_, path)) = self.selected_file_path_for_copy() else {
            return;
        };
        let path = PathBuf::from(path.to_os_string());
        let Some(name) = path.file_name() else {
            return;
        };
        self.state.pending_clipboard = Some(name.to_string_lossy().into_owned());
        self.state.last_message = Some("Copied selected file name".into());
    }

    fn copy_selected_file_absolute_path(&mut self) {
        let Some((repository_index, path)) = self.selected_file_path_for_copy() else {
            return;
        };
        let Some(repository) = self.state.repository_node(repository_index) else {
            return;
        };
        let root = if repository.git_cwd().is_absolute() {
            repository.git_cwd().to_path_buf()
        } else {
            let Ok(current_directory) = std::env::current_dir() else {
                return;
            };
            current_directory.join(repository.git_cwd())
        };
        let absolute = root.join(PathBuf::from(path.to_os_string()));
        self.state.pending_clipboard = Some(absolute.to_string_lossy().into_owned());
        self.state.last_message = Some("Copied selected file absolute path".into());
    }

    fn copy_selected_file_relative_path(&mut self) {
        let Some((_, path)) = self.selected_file_path_for_copy() else {
            return;
        };
        self.state.pending_clipboard = Some(path.as_str().to_string());
        self.state.last_message = Some("Copied selected file repository-relative path".into());
    }

    fn open_cherry_pick_selected_dialog(&mut self) {
        let Some(repository_index) = self.state.commit_selection_repository_index else {
            return;
        };
        if self.state.viewing_repository_index() != Some(repository_index) {
            return;
        }
        let commits = self.state.selected_commit_hashes_for_cherry_pick();
        if commits.is_empty() {
            return;
        }
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::CherryPickSelected {
                repository_index,
                commits,
            },
        };
    }

    fn open_reset_dialog(&mut self) {
        let target = if self.state.focus_context().kind == FocusKind::Reflog {
            self.state.selected_reflog().map(|entry| {
                (
                    self.state.reflog_repository_index,
                    entry.hash.clone(),
                    entry.short_hash.clone(),
                )
            })
        } else {
            self.state.selected_commit().map(|commit| {
                (
                    self.state.viewing_repository_index(),
                    commit.hash.clone(),
                    commit.short_hash.clone(),
                )
            })
        };
        let Some((Some(repository_index), commit, short_hash)) = target else {
            return;
        };
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::ResetModeChoice {
                repository_index,
                commit,
                short_hash,
            },
        };
    }
}
