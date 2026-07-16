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
    Action, AppError, AppState, BranchTreeNode, ChangeGroup, ChangeSelection, ChangesTreeNode,
    CommandKind, ConfirmDialog, DiffViewMode, FilterTarget, FocusPanel, GlobalMode, PendingJobKind,
    RemoteEditKind, RemoteInputField, Screen,
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
            .repositories
            .get(repository_index)?
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
            && let Some(repository) = self.state.repositories.get_mut(repository_index)
        {
            repository.latest_status_job = Some(id);
        }
    }

    fn refresh_repository(&mut self, repository_index: usize) {
        self.submit_status(repository_index, true);
    }

    fn refresh_all_repositories(&mut self) {
        for repository_index in 0..self.state.repositories.len() {
            self.submit_status(repository_index, true);
        }
    }

    fn submit_branches(&mut self, repository_index: usize) {
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadBranches,
            PendingJobKind::Branches { repository_index },
        ) && let Some(repository) = self.state.repositories.get_mut(repository_index)
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
        }
    }

    fn submit_commits(&mut self, repository_index: usize, branch: BranchName) {
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
        }
    }

    fn submit_commit_detail(&mut self, repository_index: usize, commit: CommitHash) {
        let kind = PendingJobKind::CommitDetail {
            repository_index,
            commit: commit.clone(),
        };
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadCommitDetail { commit },
            kind,
        ) {
            self.state.latest_commit_detail_job = Some(id);
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
        }
    }

    fn submit_changes(&mut self, repository_index: usize) {
        if let Some(id) = self.submit(
            repository_index,
            GitRequest::LoadWorkingTree,
            PendingJobKind::Changes { repository_index },
        ) {
            self.state.latest_changes_job = Some(id);
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
        let Some(repository_index) = self.state.branch_commits_repository_index else {
            return;
        };
        let Some(detail) = self.state.current_commit_detail.as_ref() else {
            return;
        };
        let Some(file) = self.state.selected_file() else {
            return;
        };
        let commit = detail.commit.hash.clone();
        let path = file.path.clone();
        let old_path = file.old_path.clone();
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
        }
        self.state.selection.diff_scroll = 0;
    }

    fn is_stale(&self, id: GitJobId, kind: &PendingJobKind) -> bool {
        let repository_index = kind.repository_index();
        match kind {
            PendingJobKind::RepositoryStatus { .. } => self
                .state
                .repositories
                .get(repository_index)
                .is_none_or(|repository| repository.latest_status_job != Some(id)),
            PendingJobKind::Branches { .. } => self
                .state
                .repositories
                .get(repository_index)
                .is_none_or(|repository| repository.latest_branches_job != Some(id)),
            PendingJobKind::Remotes { .. } => {
                self.state.latest_remotes_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
                    || self.state.remotes_repository_index != Some(repository_index)
                    || self.state.screen != Screen::Remotes
            }
            PendingJobKind::Commits { .. } => {
                self.state.latest_commits_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
            }
            PendingJobKind::CommitDetail { .. } => {
                self.state.latest_commit_detail_job != Some(id)
                    || self.state.branch_commits_repository_index != Some(repository_index)
            }
            PendingJobKind::CommitMessage { commit, .. } => {
                self.state.latest_commit_message_job != Some(id)
                    || self.state.branch_commits_repository_index != Some(repository_index)
                    || self
                        .state
                        .selected_commit()
                        .is_none_or(|selected| &selected.hash != commit)
            }
            PendingJobKind::FileDiff { .. } => {
                self.state.latest_file_diff_job != Some(id)
                    || self.state.branch_commits_repository_index != Some(repository_index)
                    || !matches!(
                        self.state.screen,
                        Screen::CommitDetail | Screen::FileDiffDetail
                    )
            }
            PendingJobKind::Reflog { .. } => {
                self.state.latest_reflog_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
                    || self.state.reflog_repository_index != Some(repository_index)
                    || self.state.screen != Screen::Reflog
            }
            PendingJobKind::Changes { .. } => {
                self.state.latest_changes_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
                    || self.state.changes_repository_index != Some(repository_index)
                    || self.state.screen != Screen::Changes
            }
            PendingJobKind::ChangesDiff { .. } => {
                self.state.latest_changes_diff_job != Some(id)
                    || self.state.active_repository_index != Some(repository_index)
                    || self.state.changes_repository_index != Some(repository_index)
                    || self.state.screen != Screen::Changes
            }
            PendingJobKind::Command { .. } => false,
        }
    }

    fn clear_latest(&mut self, id: GitJobId, kind: &PendingJobKind) {
        let repository_index = kind.repository_index();
        match kind {
            PendingJobKind::RepositoryStatus { .. } => {
                if let Some(repository) = self.state.repositories.get_mut(repository_index)
                    && repository.latest_status_job == Some(id)
                {
                    repository.latest_status_job = None;
                }
            }
            PendingJobKind::Branches { .. } => {
                if let Some(repository) = self.state.repositories.get_mut(repository_index)
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
                    .repositories
                    .get(repository_index)?
                    .branches
                    .get(branch_index)
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
                        .repositories
                        .get(repository_index)
                        .and_then(|repository| repository.branches.get(branch_index))
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
        let repository = self.state.repositories.get(repository_index)?;
        repository
            .viewing_branch
            .clone()
            .or_else(|| {
                repository
                    .repository
                    .as_ref()
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
            self.state.branch_commits_repository_index = Some(repository_index);
            self.state.branch_commits.viewing_branch = self
                .state
                .repository(repository_index)
                .and_then(|repository| repository.current_branch.clone());
            self.state.branch_commits.items.clear();
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
            return;
        }

        match envelope.response {
            GitResponse::RepositoryStatusLoaded(repository) => {
                let identity = self.selected_tree_identity();
                let full_refresh = matches!(
                    kind,
                    PendingJobKind::RepositoryStatus {
                        full_refresh: true,
                        ..
                    }
                );
                if let Some(state) = self.state.repositories.get_mut(repository_index) {
                    state.repository = Some(repository);
                    state.last_error = None;
                    state.ensure_current_branch_visible();
                }
                self.restore_tree_selection(identity);
                if full_refresh {
                    self.submit_branches(repository_index);
                    self.load_active_repository_commits(repository_index);
                }
            }
            GitResponse::BranchesLoaded(branches) => {
                let identity = self.selected_tree_identity();
                if let Some(repository) = self.state.repositories.get_mut(repository_index) {
                    repository.branches = branches;
                    repository.last_error = None;
                    repository.ensure_current_branch_visible();
                }
                self.restore_tree_selection(identity);
            }
            GitResponse::RemotesLoaded(remotes) => {
                let selected_name = self
                    .state
                    .selected_remote()
                    .map(|remote| remote.name.clone());
                self.state.remotes_repository_index = Some(repository_index);
                self.state.remotes = remotes;
                self.state.selection.selected_remote_index = selected_name.and_then(|name| {
                    self.state
                        .remotes
                        .iter()
                        .position(|remote| remote.name == name)
                });
                self.state.ensure_valid_remote_selection();
                self.state.screen = Screen::Remotes;
                if self.state.focus != FocusPanel::Popup {
                    self.state.focus = FocusPanel::RemoteList;
                }
                if let Some(repository) = self.state.repositories.get_mut(repository_index) {
                    repository.last_error = None;
                }
            }
            GitResponse::CommitsLoaded { branch, commits } => {
                let available_hashes = commits
                    .iter()
                    .map(|commit| commit.hash.clone())
                    .collect::<HashSet<_>>();
                let changed_repository =
                    self.state.branch_commits_repository_index != Some(repository_index);
                let changed_branch =
                    self.state.branch_commits.viewing_branch.as_ref() != Some(&branch);
                if let Some(repository) = self.state.repositories.get_mut(repository_index) {
                    repository.viewing_branch = Some(branch.clone());
                    repository.last_error = None;
                }
                self.state.branch_commits_repository_index = Some(repository_index);
                self.state.branch_commits.viewing_branch = Some(branch);
                self.state.branch_commits.items = commits;
                if changed_repository || changed_branch {
                    self.state.commit_filter.clear();
                    self.state.selection.selected_commit_index = None;
                    self.state.current_commit_detail = None;
                    self.state.current_file_diff = None;
                    self.state.commit_copy_selection.clear();
                    self.state.commit_copy_selection_repository_index = Some(repository_index);
                } else {
                    self.state
                        .commit_copy_selection
                        .retain(|hash| available_hashes.contains(hash));
                }
                self.state.ensure_valid_commit_selection();
            }
            GitResponse::CommitDetailLoaded(detail) => {
                self.state.current_commit_detail = Some(detail);
                self.state.current_file_diff = None;
                self.state.selection.selected_file_index = None;
                self.state.ensure_valid_file_selection();
                self.state.expansion.expanded_files.clear();
                self.state.screen = Screen::CommitDetail;
                self.state.focus = FocusPanel::CommitFileList;
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
                self.state.current_file_diff = Some(diff);
                self.state.selection.diff_scroll = 0;
                self.state.screen = Screen::FileDiffDetail;
                // Enter/open deliberately focuses the diff. Up/Down/Home/End
                // and n/p only refresh it and preserve the current panel.
                if focus_diff {
                    self.state.focus = FocusPanel::DiffView;
                }
            }
            GitResponse::ReflogLoaded(entries) => {
                self.state.reflog_repository_index = Some(repository_index);
                self.state.reflog_entries = entries;
                self.state.selection.selected_reflog_index = None;
                self.state.ensure_valid_reflog_selection();
                self.state.screen = Screen::Reflog;
                self.state.focus = FocusPanel::ReflogList;
                if let Some(repository) = self.state.repositories.get_mut(repository_index) {
                    repository.last_error = None;
                }
            }
            GitResponse::WorkingTreeLoaded(changes) => {
                let selected_container = self.state.selected_changes_node().and_then(|node| {
                    matches!(node, ChangesTreeNode::Root | ChangesTreeNode::Group(_))
                        .then_some(node)
                });
                let selected_identity = self.state.selected_change_identity();
                self.state.changes_repository_index = Some(repository_index);
                self.state.changes = changes;
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
                                        .changes
                                        .get(change_index)
                                        .is_some_and(|change| change.path == wanted_path)
                            })
                        })
                    });
                self.state.ensure_valid_changes_selection();
                self.state.screen = Screen::Changes;
                if !matches!(
                    self.state.focus,
                    FocusPanel::ChangesTree | FocusPanel::ChangesDiff
                ) {
                    self.state.focus = FocusPanel::ChangesTree;
                }
                if let Some(repository) = self.state.repositories.get_mut(repository_index) {
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
                    self.state.cherry_pick_queue.clear();
                    self.state.cherry_pick_queue_repository_index = None;
                }
                if matches!(
                    command_kind,
                    Some(CommandKind::Reset | CommandKind::Rebase | CommandKind::PullRebase)
                ) {
                    self.state.cherry_pick_queue.clear();
                    self.state.cherry_pick_queue_repository_index = None;
                }
                if command_kind == Some(CommandKind::Reset) {
                    self.state.current_commit_detail = None;
                    self.state.current_file_diff = None;
                    self.state.screen = Screen::BranchOverview;
                    self.state.focus = FocusPanel::CommitList;
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
                if let Some(repository) = self.state.repositories.get_mut(repository_index) {
                    repository.last_error = None;
                }
                self.state.last_error = None;
                self.state.last_message = Some(message);
                self.refresh_repository(repository_index);
                if self.state.screen == Screen::Changes
                    && self.state.changes_repository_index == Some(repository_index)
                {
                    self.submit_changes(repository_index);
                }
                if remote_command
                    && self.state.screen == Screen::Remotes
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
                let error = AppError {
                    command: command.clone(),
                    message: stderr.clone(),
                };
                if let Some(repository) = self.state.repositories.get_mut(repository_index) {
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
            GlobalMode::Error => match action {
                Action::DismissError | Action::Cancel | Action::Back | Action::Confirm => {
                    self.state.dismiss_error();
                }
                _ => {}
            },
            GlobalMode::Normal => self.dispatch_normal(action),
        }
    }

    fn on_tick(&mut self) {
        self.state.tick_count = self.state.tick_count.wrapping_add(1);
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
                self.activate_selected_tree_repository();
            }
            Action::CancelFilter | Action::Cancel | Action::Back => {
                self.state.mode = GlobalMode::Normal;
                self.state.ensure_valid_branch_selection();
                self.state.ensure_valid_commit_selection();
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
            && self.state.remotes.iter().any(|remote| remote.name == name)
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
                    ConfirmDialog::CherryPickQueue {
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
            Action::MoveLeft if self.state.focus == FocusPanel::ChangesTree => {
                self.collapse_selected_change_node()
            }
            Action::MoveRight if self.state.focus == FocusPanel::ChangesTree => {
                self.expand_selected_change_node()
            }
            Action::MoveLeft | Action::FocusPrev => self.focus_previous(),
            Action::MoveRight | Action::FocusNext => self.focus_next(),
            Action::Back => self.back(),
            Action::RefreshRepository => {
                self.refresh_all_repositories();
                if self.state.screen == Screen::Reflog
                    && let Some(repository_index) = self.state.reflog_repository_index
                {
                    self.submit_reflog(repository_index);
                }
                if self.state.screen == Screen::Changes
                    && let Some(repository_index) = self.state.changes_repository_index
                {
                    self.submit_changes(repository_index);
                }
                if self.state.screen == Screen::Remotes
                    && let Some(repository_index) = self.state.remotes_repository_index
                {
                    self.submit_remotes(repository_index);
                }
            }
            Action::StartFilter => self.start_filter(),
            Action::SelectBranch(index) => {
                self.state.selection.selected_branch_index = Some(index);
                self.state.ensure_valid_branch_selection();
                self.activate_selected_tree_repository();
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
            }
            Action::OpenCommitDetail => self.open_commit_detail(),
            Action::ToggleFileExpanded => self.toggle_file_expanded(),
            Action::OpenSelectedFileDiff => self.submit_selected_file_diff(true),
            Action::ToggleDiffMode => {
                self.state.diff_mode = match self.state.diff_mode {
                    DiffViewMode::Unified => DiffViewMode::SideBySide,
                    DiffViewMode::SideBySide => DiffViewMode::Unified,
                };
                if self.state.screen == Screen::Changes {
                    self.state.selection.changes_diff_scroll = 0;
                } else {
                    self.state.selection.diff_scroll = 0;
                }
            }
            Action::NextFile => self.move_file(1),
            Action::PrevFile => self.move_file(-1),
            Action::ToggleWrap => self.state.wrap_diff = !self.state.wrap_diff,
            Action::ToggleCommitCopySelection => self.toggle_commit_copy_selection(),
            Action::BeginChord(prefix) => {
                self.state.mode = GlobalMode::Chord {
                    prefix,
                    started_at: Instant::now(),
                };
            }
            Action::CopySelectedCommitHashes => self.copy_selected_commit_hashes(),
            Action::CopyCurrentCommitInfo => self.copy_current_commit_info(),
            Action::CopyCurrentCommitMessage => self.copy_current_commit_message(),
            Action::QueueCherryPickSelectedCommit => self.queue_selected_commit(),
            Action::OpenCherryPickQueueDialog => self.open_cherry_pick_dialog(),
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

    fn move_up(&mut self) {
        match self.state.focus {
            FocusPanel::BranchList => {
                let length = self.state.visible_tree_nodes().len();
                Self::move_selection(&mut self.state.selection.selected_branch_index, length, -1);
                self.activate_selected_tree_repository();
            }
            FocusPanel::CommitList => {
                let length = self.state.visible_commit_indices().len();
                Self::move_selection(&mut self.state.selection.selected_commit_index, length, -1);
            }
            FocusPanel::CommitFileList => Self::move_selection(
                &mut self.state.selection.selected_file_index,
                self.state
                    .current_commit_detail
                    .as_ref()
                    .map_or(0, |detail| detail.files.len()),
                -1,
            ),
            FocusPanel::FileList => self.move_file(-1),
            FocusPanel::DiffView => {
                self.state.selection.diff_scroll =
                    self.state.selection.diff_scroll.saturating_sub(1);
            }
            FocusPanel::ReflogList => Self::move_selection(
                &mut self.state.selection.selected_reflog_index,
                self.state.reflog_entries.len(),
                -1,
            ),
            FocusPanel::RemoteList => Self::move_selection(
                &mut self.state.selection.selected_remote_index,
                self.state.remotes.len(),
                -1,
            ),
            FocusPanel::ChangesTree => self.move_change_node(-1),
            FocusPanel::ChangesDiff => {
                self.state.selection.changes_diff_scroll =
                    self.state.selection.changes_diff_scroll.saturating_sub(1);
            }
            FocusPanel::Popup => {}
        }
    }

    fn move_down(&mut self) {
        match self.state.focus {
            FocusPanel::BranchList => {
                let length = self.state.visible_tree_nodes().len();
                Self::move_selection(&mut self.state.selection.selected_branch_index, length, 1);
                self.activate_selected_tree_repository();
            }
            FocusPanel::CommitList => {
                let length = self.state.visible_commit_indices().len();
                Self::move_selection(&mut self.state.selection.selected_commit_index, length, 1);
            }
            FocusPanel::CommitFileList => Self::move_selection(
                &mut self.state.selection.selected_file_index,
                self.state
                    .current_commit_detail
                    .as_ref()
                    .map_or(0, |detail| detail.files.len()),
                1,
            ),
            FocusPanel::FileList => self.move_file(1),
            FocusPanel::DiffView => {
                let maximum = Self::maximum_scroll(self.state.diff_line_count());
                self.state.selection.diff_scroll = self
                    .state
                    .selection
                    .diff_scroll
                    .saturating_add(1)
                    .min(maximum);
            }
            FocusPanel::ReflogList => Self::move_selection(
                &mut self.state.selection.selected_reflog_index,
                self.state.reflog_entries.len(),
                1,
            ),
            FocusPanel::RemoteList => Self::move_selection(
                &mut self.state.selection.selected_remote_index,
                self.state.remotes.len(),
                1,
            ),
            FocusPanel::ChangesTree => self.move_change_node(1),
            FocusPanel::ChangesDiff => {
                let maximum = Self::maximum_scroll(self.state.changes_diff_line_count());
                self.state.selection.changes_diff_scroll = self
                    .state
                    .selection
                    .changes_diff_scroll
                    .saturating_add(1)
                    .min(maximum);
            }
            FocusPanel::Popup => {}
        }
    }

    fn page_up(&mut self) {
        match self.state.focus {
            FocusPanel::CommitFileList => Self::move_selection(
                &mut self.state.selection.selected_file_index,
                self.state
                    .current_commit_detail
                    .as_ref()
                    .map_or(0, |detail| detail.files.len()),
                -(PAGE_SIZE as isize),
            ),
            FocusPanel::FileList => self.move_file(-(PAGE_SIZE as isize)),
            FocusPanel::DiffView => {
                self.state.selection.diff_scroll = self
                    .state
                    .selection
                    .diff_scroll
                    .saturating_sub(PAGE_SIZE as u16);
            }
            FocusPanel::ChangesDiff => {
                self.state.selection.changes_diff_scroll = self
                    .state
                    .selection
                    .changes_diff_scroll
                    .saturating_sub(PAGE_SIZE as u16);
            }
            FocusPanel::ChangesTree => self.move_change_node(-(PAGE_SIZE as isize)),
            _ => {
                for _ in 0..PAGE_SIZE {
                    self.move_up();
                }
            }
        }
    }

    fn page_down(&mut self) {
        match self.state.focus {
            FocusPanel::CommitFileList => Self::move_selection(
                &mut self.state.selection.selected_file_index,
                self.state
                    .current_commit_detail
                    .as_ref()
                    .map_or(0, |detail| detail.files.len()),
                PAGE_SIZE as isize,
            ),
            FocusPanel::FileList => self.move_file(PAGE_SIZE as isize),
            FocusPanel::DiffView => {
                let maximum = Self::maximum_scroll(self.state.diff_line_count());
                self.state.selection.diff_scroll = self
                    .state
                    .selection
                    .diff_scroll
                    .saturating_add(PAGE_SIZE as u16)
                    .min(maximum);
            }
            FocusPanel::ChangesDiff => {
                let maximum = Self::maximum_scroll(self.state.changes_diff_line_count());
                self.state.selection.changes_diff_scroll = self
                    .state
                    .selection
                    .changes_diff_scroll
                    .saturating_add(PAGE_SIZE as u16)
                    .min(maximum);
            }
            FocusPanel::ChangesTree => self.move_change_node(PAGE_SIZE as isize),
            _ => {
                for _ in 0..PAGE_SIZE {
                    self.move_down();
                }
            }
        }
    }

    fn move_home(&mut self) {
        match self.state.focus {
            FocusPanel::BranchList => {
                self.state.selection.selected_branch_index = Some(0);
                self.state.ensure_valid_branch_selection();
                self.activate_selected_tree_repository();
            }
            FocusPanel::CommitList => self.state.selection.selected_commit_index = Some(0),
            FocusPanel::CommitFileList | FocusPanel::FileList => {
                self.state.selection.selected_file_index = Some(0);
                if self.state.focus == FocusPanel::FileList {
                    self.submit_selected_file_diff(false);
                }
            }
            FocusPanel::DiffView => self.state.selection.diff_scroll = 0,
            FocusPanel::ReflogList => self.state.selection.selected_reflog_index = Some(0),
            FocusPanel::RemoteList => self.state.selection.selected_remote_index = Some(0),
            FocusPanel::ChangesTree => {
                self.state.selection.selected_changes_index = Some(0);
                self.state.ensure_valid_changes_selection();
                self.submit_selected_change_diff();
            }
            FocusPanel::ChangesDiff => self.state.selection.changes_diff_scroll = 0,
            FocusPanel::Popup => {}
        }
        self.state.ensure_valid_commit_selection();
        self.state.ensure_valid_file_selection();
        self.state.ensure_valid_reflog_selection();
        self.state.ensure_valid_remote_selection();
        self.state.ensure_valid_changes_selection();
    }

    fn move_end(&mut self) {
        match self.state.focus {
            FocusPanel::BranchList => {
                self.state.selection.selected_branch_index =
                    self.state.visible_tree_nodes().len().checked_sub(1);
                self.activate_selected_tree_repository();
            }
            FocusPanel::CommitList => {
                self.state.selection.selected_commit_index =
                    self.state.visible_commit_indices().len().checked_sub(1);
            }
            FocusPanel::CommitFileList | FocusPanel::FileList => {
                self.state.selection.selected_file_index = self
                    .state
                    .current_commit_detail
                    .as_ref()
                    .and_then(|detail| detail.files.len().checked_sub(1));
                if self.state.focus == FocusPanel::FileList {
                    self.submit_selected_file_diff(false);
                }
            }
            FocusPanel::DiffView => {
                self.state.selection.diff_scroll =
                    Self::maximum_scroll(self.state.diff_line_count());
            }
            FocusPanel::ReflogList => {
                self.state.selection.selected_reflog_index =
                    self.state.reflog_entries.len().checked_sub(1);
            }
            FocusPanel::RemoteList => {
                self.state.selection.selected_remote_index =
                    self.state.remotes.len().checked_sub(1);
            }
            FocusPanel::ChangesTree => {
                self.state.selection.selected_changes_index =
                    self.state.visible_changes_nodes().len().checked_sub(1);
                self.submit_selected_change_diff();
            }
            FocusPanel::ChangesDiff => {
                self.state.selection.changes_diff_scroll =
                    Self::maximum_scroll(self.state.changes_diff_line_count());
            }
            FocusPanel::Popup => {}
        }
    }

    fn maximum_scroll(line_count: usize) -> u16 {
        u16::try_from(line_count.saturating_sub(1)).unwrap_or(u16::MAX)
    }

    fn focus_next(&mut self) {
        self.state.focus = match (self.state.screen, self.state.focus) {
            (Screen::BranchOverview, FocusPanel::BranchList) => FocusPanel::CommitList,
            (Screen::BranchOverview, _) => FocusPanel::BranchList,
            (Screen::CommitDetail, FocusPanel::CommitList) => FocusPanel::CommitFileList,
            (Screen::CommitDetail, _) => FocusPanel::CommitList,
            (Screen::FileDiffDetail, FocusPanel::FileList) => FocusPanel::DiffView,
            (Screen::FileDiffDetail, _) => FocusPanel::FileList,
            (Screen::Reflog, _) => FocusPanel::ReflogList,
            (Screen::Remotes, _) => FocusPanel::RemoteList,
            (Screen::Changes, FocusPanel::ChangesTree) => FocusPanel::ChangesDiff,
            (Screen::Changes, _) => FocusPanel::ChangesTree,
        };
    }

    fn focus_previous(&mut self) {
        self.focus_next();
    }

    fn back(&mut self) {
        match self.state.screen {
            Screen::BranchOverview => {}
            Screen::CommitDetail => {
                self.state.screen = Screen::BranchOverview;
                self.state.focus = FocusPanel::CommitList;
            }
            Screen::FileDiffDetail => {
                self.state.screen = Screen::CommitDetail;
                self.state.focus = FocusPanel::CommitFileList;
            }
            Screen::Reflog => {
                self.state.screen = Screen::BranchOverview;
                self.state.focus = FocusPanel::BranchList;
            }
            Screen::Remotes => {
                self.state.screen = Screen::BranchOverview;
                self.state.focus = FocusPanel::BranchList;
            }
            Screen::Changes => {
                let (screen, focus) = self
                    .state
                    .changes_return_context
                    .take()
                    .unwrap_or((Screen::BranchOverview, FocusPanel::BranchList));
                self.state.screen = screen;
                self.state.focus = focus;
            }
        }
    }

    fn start_filter(&mut self) {
        let (target, query) = match self.state.focus {
            FocusPanel::BranchList => (FilterTarget::Branches, self.state.branch_filter.clone()),
            FocusPanel::CommitList => (FilterTarget::Commits, self.state.commit_filter.clone()),
            _ => return,
        };
        self.state.mode = GlobalMode::Filtering { target, query };
    }

    fn activate_repository(&mut self, repository_index: usize) {
        if self.state.repositories.get(repository_index).is_none()
            || self.state.active_repository_index == Some(repository_index)
        {
            return;
        }
        self.state.active_repository_index = Some(repository_index);
        self.state.branch_commits_repository_index = Some(repository_index);
        self.state.branch_commits = crate::domain::CommitList::empty();
        self.state.current_commit_detail = None;
        self.state.current_file_diff = None;
        self.state.reflog_repository_index = None;
        self.state.reflog_entries.clear();
        self.state.remotes_repository_index = None;
        self.state.remotes.clear();
        self.state.changes_repository_index = None;
        self.state.changes.clear();
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
        self.state.screen = Screen::BranchOverview;
        if self.state.focus != FocusPanel::BranchList {
            self.state.focus = FocusPanel::CommitList;
        }
        if self.state.cherry_pick_queue_repository_index != Some(repository_index) {
            self.state.cherry_pick_queue.clear();
            self.state.cherry_pick_queue_repository_index = None;
        }
        if self.state.commit_copy_selection_repository_index != Some(repository_index) {
            self.state.commit_copy_selection.clear();
            self.state.commit_copy_selection_repository_index = None;
        }
        self.load_active_repository_commits(repository_index);
    }

    fn activate_selected_tree_repository(&mut self) {
        if let Some(repository_index) = self.state.selected_tree_repository_index() {
            self.activate_repository(repository_index);
        }
    }

    fn activate_selected_tree_node(&mut self) {
        match self.state.selected_tree_node() {
            Some(BranchTreeNode::Repository { repository_index }) => {
                self.activate_repository(repository_index);
                if let Some(repository) = self.state.repositories.get_mut(repository_index) {
                    repository.expanded = !repository.expanded;
                }
                self.state.ensure_valid_branch_selection();
            }
            Some(BranchTreeNode::Branch {
                repository_index,
                branch_index,
            }) => {
                let branch = self
                    .state
                    .repositories
                    .get(repository_index)
                    .and_then(|repository| repository.branches.get(branch_index))
                    .map(|branch| (branch.name.clone(), !branch.head.0.is_empty()));
                self.activate_repository(repository_index);
                if let Some((branch, has_commit)) = branch {
                    if has_commit {
                        self.submit_commits(repository_index, branch);
                    } else {
                        if let Some(repository) = self.state.repositories.get_mut(repository_index)
                        {
                            repository.viewing_branch = Some(branch.clone());
                        }
                        self.state.branch_commits_repository_index = Some(repository_index);
                        self.state.branch_commits.viewing_branch = Some(branch);
                        self.state.branch_commits.items.clear();
                        self.state.selection.selected_commit_index = None;
                        self.state.ensure_valid_commit_selection();
                    }
                    self.state.focus = FocusPanel::CommitList;
                }
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
        self.state.screen = Screen::Remotes;
        self.state.focus = FocusPanel::RemoteList;
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
        self.state.reflog_entries.clear();
        self.state.selection.selected_reflog_index = None;
        self.state.screen = Screen::Reflog;
        self.state.focus = FocusPanel::ReflogList;
        self.submit_reflog(repository_index);
    }

    fn toggle_changes(&mut self) {
        if self.state.screen == Screen::Changes {
            self.back();
            return;
        }
        let Some(repository_index) = self.state.active_repository_index else {
            return;
        };
        self.state.changes_return_context = Some((self.state.screen, self.state.focus));
        self.state.changes_repository_index = Some(repository_index);
        self.state.current_changes_diff = None;
        self.state.current_changes_diff_group = None;
        self.state.selection.changes_diff_scroll = 0;
        self.state.screen = Screen::Changes;
        self.state.focus = FocusPanel::ChangesTree;
        if self.state.changes.is_empty() {
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
                self.state.focus = FocusPanel::ChangesDiff;
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
                .changes
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
        for change in &self.state.changes {
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
        let Some(repository_index) = self.state.branch_commits_repository_index else {
            return;
        };
        let Some(commit) = self
            .state
            .selected_commit()
            .map(|commit| commit.hash.clone())
        else {
            return;
        };
        self.submit_commit_detail(repository_index, commit);
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
        let length = self
            .state
            .current_commit_detail
            .as_ref()
            .map_or(0, |detail| detail.files.len());
        let before = self.state.selection.selected_file_index;
        Self::move_selection(&mut self.state.selection.selected_file_index, length, delta);
        if self.state.screen == Screen::FileDiffDetail
            && self.state.selection.selected_file_index != before
        {
            self.submit_selected_file_diff(false);
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

    fn toggle_commit_copy_selection(&mut self) {
        let Some(repository_index) = self.state.branch_commits_repository_index else {
            return;
        };
        let Some(hash) = self
            .state
            .selected_commit()
            .map(|commit| commit.hash.clone())
        else {
            return;
        };
        if self.state.commit_copy_selection_repository_index != Some(repository_index) {
            self.state.commit_copy_selection.clear();
            self.state.commit_copy_selection_repository_index = Some(repository_index);
        }
        if !self.state.commit_copy_selection.remove(&hash) {
            self.state.commit_copy_selection.insert(hash);
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

        let Some(repository_index) = self.state.branch_commits_repository_index else {
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

    fn queue_selected_commit(&mut self) {
        let Some(repository_index) = self.state.branch_commits_repository_index else {
            return;
        };
        let Some(commit) = self
            .state
            .selected_commit()
            .map(|commit| commit.hash.clone())
        else {
            return;
        };
        if self.state.cherry_pick_queue_repository_index != Some(repository_index) {
            self.state.cherry_pick_queue.clear();
            self.state.cherry_pick_queue_repository_index = Some(repository_index);
        }
        if !self.state.cherry_pick_queue.contains(&commit) {
            self.state.cherry_pick_queue.push(commit);
        }
    }

    fn open_cherry_pick_dialog(&mut self) {
        let Some(repository_index) = self.state.cherry_pick_queue_repository_index else {
            return;
        };
        if self.state.cherry_pick_queue.is_empty()
            || self.state.branch_commits_repository_index != Some(repository_index)
        {
            return;
        }
        self.state.open_popup();
        self.state.mode = GlobalMode::Confirming {
            dialog: ConfirmDialog::CherryPickQueue {
                repository_index,
                commits: self.state.cherry_pick_queue.clone(),
            },
        };
    }

    fn open_reset_dialog(&mut self) {
        let target = if self.state.screen == Screen::Reflog {
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
                    self.state.branch_commits_repository_index,
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
