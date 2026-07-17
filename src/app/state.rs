use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

use crate::{
    config::{KeyStroke, ResolvedConfig},
    domain::{
        Branch, BranchName, Commit, CommitDetail, CommitHash, FileDiff, GitPath, ReflogEntry,
        RemoteInfo, Repository, WorkingTreeChange,
    },
    git::{GitJobId, ResetMode},
};

use super::{
    BranchId, CommitId, FileId, FocusContext, GitModel, NavigationState, OperationId, RepositoryId,
};

/// The two Git boundaries represented in the Changes tree. A path can appear
/// in both groups when it has an indexed change and a newer working-tree
/// change (porcelain `MM`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChangeGroup {
    Staged,
    Unstaged,
}

impl ChangeGroup {
    pub fn title(self) -> &'static str {
        match self {
            Self::Staged => "Staged Changes",
            Self::Unstaged => "Unstaged Changes",
        }
    }
}

/// A flattened node in the three-level Changes tree:
/// `Changes -> Staged/Unstaged -> file`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChangesTreeNode {
    Root,
    Group(ChangeGroup),
    File {
        group: ChangeGroup,
        change_index: usize,
    },
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ChangeSelection {
    pub group: ChangeGroup,
    pub path: GitPath,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DiffViewMode {
    #[default]
    Unified,
    SideBySide,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FilterTarget {
    Branches,
    Commits,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RemoteEditKind {
    Add,
    SetUrl { remote_name: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RemoteInputField {
    Name,
    Url,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmDialog {
    FetchRepository {
        repository_index: usize,
    },
    PullRebaseRepository {
        repository_index: usize,
        branch: BranchName,
    },
    PushRepository {
        repository_index: usize,
        branch: BranchName,
    },
    AddRemote {
        repository_index: usize,
        name: String,
        url: String,
    },
    SetRemoteUrl {
        repository_index: usize,
        name: String,
        url: String,
    },
    SetUpstreamRemote {
        repository_index: usize,
        name: String,
        branch: BranchName,
    },
    SwitchBranch {
        repository_index: usize,
        branch: BranchName,
    },
    CherryPickSelected {
        repository_index: usize,
        commits: Vec<CommitHash>,
    },
    ResetModeChoice {
        repository_index: usize,
        commit: CommitHash,
        short_hash: String,
    },
    Reset {
        repository_index: usize,
        commit: CommitHash,
        mode: ResetMode,
    },
    HardResetWarning {
        repository_index: usize,
        commit: CommitHash,
        expected: String,
    },
    Rebase {
        repository_index: usize,
        current_branch: BranchName,
        upstream: BranchName,
    },
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum GlobalMode {
    #[default]
    Normal,
    Filtering {
        target: FilterTarget,
        query: String,
    },
    Confirming {
        dialog: ConfirmDialog,
    },
    TypingConfirmation {
        dialog: ConfirmDialog,
        expected: String,
        input: String,
        validation_error: Option<String>,
    },
    EditingCommitMessage {
        input: String,
        validation_error: Option<String>,
    },
    EditingRemote {
        kind: RemoteEditKind,
        field: RemoteInputField,
        name: String,
        url: String,
        validation_error: Option<String>,
    },
    Chord {
        prefix: Vec<KeyStroke>,
        started_at: Instant,
    },
    ShortcutHelp {
        scroll: u16,
    },
    /// Searchable projection of the currently resolved normal-mode operation
    /// set. IDs are captured before opening the popup so availability and
    /// invocation come from the same registry/focus snapshot.
    OperationPalette {
        query: String,
        selected: usize,
        operations: Vec<OperationId>,
    },
    CommandPrompt {
        input: String,
        validation_error: Option<String>,
    },
    Error,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectionState {
    /// Index into the flattened, filtered repository/branch tree.
    pub selected_branch_index: Option<usize>,
    /// The remaining indices are into their currently filtered/source views.
    pub selected_commit_index: Option<usize>,
    pub selected_file_index: Option<usize>,
    pub selected_reflog_index: Option<usize>,
    pub selected_remote_index: Option<usize>,
    /// Index into the flattened three-level Changes tree.
    pub selected_changes_index: Option<usize>,
    pub diff_scroll: u16,
    pub changes_diff_scroll: u16,
    pub file_scroll: u16,
    pub commit_scroll: u16,
    pub branch_scroll: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpansionState {
    pub expanded_files: HashSet<GitPath>,
    pub changes_root_expanded: bool,
    pub staged_changes_expanded: bool,
    pub unstaged_changes_expanded: bool,
}

impl Default for ExpansionState {
    fn default() -> Self {
        Self {
            expanded_files: HashSet::new(),
            changes_root_expanded: true,
            staged_changes_expanded: true,
            unstaged_changes_expanded: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppError {
    pub command: String,
    pub message: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKind {
    Fetch,
    PullRebase,
    Push,
    SwitchBranch,
    CherryPick,
    Reset,
    Rebase,
    Stage,
    Unstage,
    Commit,
    AddRemote,
    SetRemoteUrl,
    SetUpstreamRemote,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PendingJobKind {
    RepositoryStatus {
        repository_index: usize,
        full_refresh: bool,
    },
    Branches {
        repository_index: usize,
    },
    Remotes {
        repository_index: usize,
    },
    Commits {
        repository_index: usize,
        branch: BranchName,
    },
    CommitDetail {
        repository_index: usize,
        commit: CommitHash,
        /// True only for an explicit Enter/open action. Selection-driven
        /// previews update the right pane without stealing focus from Commits.
        focus_files: bool,
    },
    CommitMessage {
        repository_index: usize,
        commit: CommitHash,
    },
    FileDiff {
        repository_index: usize,
        commit: CommitHash,
        path: GitPath,
        /// True only for an explicit Enter/open action. Selection-driven
        /// refreshes must not steal focus from the file list.
        focus_diff: bool,
    },
    Reflog {
        repository_index: usize,
    },
    Changes {
        repository_index: usize,
    },
    ChangesDiff {
        repository_index: usize,
        path: GitPath,
        group: ChangeGroup,
    },
    Command {
        repository_index: usize,
        kind: CommandKind,
    },
}

impl PendingJobKind {
    pub fn repository_index(&self) -> usize {
        match self {
            Self::RepositoryStatus {
                repository_index, ..
            }
            | Self::Branches { repository_index }
            | Self::Remotes { repository_index }
            | Self::Commits {
                repository_index, ..
            }
            | Self::CommitDetail {
                repository_index, ..
            }
            | Self::CommitMessage {
                repository_index, ..
            }
            | Self::FileDiff {
                repository_index, ..
            }
            | Self::Reflog { repository_index }
            | Self::Changes { repository_index }
            | Self::ChangesDiff {
                repository_index, ..
            }
            | Self::Command {
                repository_index, ..
            } => *repository_index,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BranchTreeNode {
    Repository {
        repository_index: usize,
    },
    Branch {
        repository_index: usize,
        branch_index: usize,
    },
}

impl BranchTreeNode {
    pub fn repository_index(self) -> usize {
        match self {
            Self::Repository { repository_index }
            | Self::Branch {
                repository_index, ..
            } => repository_index,
        }
    }
}

/// UI/async state keyed by the same numeric `RepositoryId` position as
/// `GitModel::repository_ids`. Core repository and branch data never lives in
/// this structure.
#[derive(Debug, Default)]
pub struct RepositoryUiState {
    pub expanded: bool,
    pub last_error: Option<AppError>,
    pub viewing_branch: Option<BranchName>,
    pub latest_status_job: Option<GitJobId>,
    pub latest_branches_job: Option<GitJobId>,
}

impl RepositoryUiState {
    pub fn new() -> Self {
        Self {
            expanded: true,
            last_error: None,
            viewing_branch: None,
            latest_status_job: None,
            latest_branches_job: None,
        }
    }
}

#[derive(Debug)]
pub struct AppState {
    /// Immutable effective configuration snapshot shared by input and
    /// rendering, so hints can never disagree with the active bindings.
    pub config: Arc<ResolvedConfig>,
    /// Authoritative normalized Repository -> Branch -> Commit -> File model.
    /// Core Git entities and collection contents live nowhere else.
    pub model: GitModel,
    pub navigation: NavigationState,
    pub repository_ui: Vec<RepositoryUiState>,
    /// Persistent JSONL audit trail written by the Git worker. The value is
    /// exposed to the UI so users can discover the effective path, including
    /// a temporary fallback when the platform default cannot be opened.
    pub backend_log_path: Option<PathBuf>,
    pub backend_logging_warning: Option<String>,
    pub active_repository_index: Option<usize>,
    /// Branch whose commit collection is projected into the History/Commit
    /// views. The commits themselves live only in `GitModel`.
    pub viewing_branch: Option<BranchId>,
    pub reflog_repository_index: Option<usize>,
    pub remotes_repository_index: Option<usize>,
    pub changes_repository_index: Option<usize>,
    pub current_changes_diff: Option<FileDiff>,
    pub current_changes_diff_group: Option<ChangeGroup>,
    /// File/group selections used by stage and unstage. The group is part of
    /// the key because an `MM` path intentionally appears twice.
    pub change_selection: HashSet<ChangeSelection>,
    /// Semantic focus to restore when Changes was opened through the global
    /// shortcut. This makes Changes an overlay-like destination rather than a
    /// child of History.
    pub changes_return_context: Option<FocusContext>,
    pub mode: GlobalMode,
    pub selection: SelectionState,
    pub expansion: ExpansionState,
    pub diff_mode: DiffViewMode,
    pub wrap_diff: bool,
    /// Commit selection shared by multi-commit operations such as copying
    /// hashes and cherry-picking. It is scoped to one repository/branch list.
    pub commit_selection: HashSet<CommitHash>,
    pub commit_selection_repository_index: Option<usize>,
    /// Consumed by the terminal layer and written through OSC 52.
    pub pending_clipboard: Option<String>,
    pub pending_jobs: HashMap<GitJobId, PendingJobKind>,
    pub latest_commits_job: Option<GitJobId>,
    pub latest_commit_detail_job: Option<GitJobId>,
    pub latest_commit_message_job: Option<GitJobId>,
    pub latest_file_diff_job: Option<GitJobId>,
    pub latest_reflog_job: Option<GitJobId>,
    pub latest_remotes_job: Option<GitJobId>,
    pub latest_changes_job: Option<GitJobId>,
    pub latest_changes_diff_job: Option<GitJobId>,
    pub last_error: Option<AppError>,
    pub last_message: Option<String>,
    pub branch_filter: String,
    pub commit_filter: String,
    pub tick_count: u64,
}

impl Default for AppState {
    fn default() -> Self {
        Self::with_repository_paths(Vec::new())
    }
}

impl AppState {
    pub fn with_repository_paths(paths: Vec<PathBuf>) -> Self {
        Self::with_config(paths, ResolvedConfig::shared_default())
    }

    pub fn with_config(paths: Vec<PathBuf>, config: Arc<ResolvedConfig>) -> Self {
        let model = GitModel::from_paths(paths.iter().cloned());
        let repository_ui = paths
            .into_iter()
            .map(|_| RepositoryUiState::new())
            .collect::<Vec<_>>();
        let active_repository_index = (!repository_ui.is_empty()).then_some(0);
        let mut state = Self {
            diff_mode: config.diff.default_mode,
            config,
            model,
            navigation: NavigationState::default(),
            repository_ui,
            backend_log_path: None,
            backend_logging_warning: None,
            active_repository_index,
            viewing_branch: None,
            reflog_repository_index: None,
            remotes_repository_index: None,
            changes_repository_index: None,
            current_changes_diff: None,
            current_changes_diff_group: None,
            change_selection: HashSet::new(),
            changes_return_context: None,
            mode: GlobalMode::Normal,
            selection: SelectionState {
                selected_branch_index: active_repository_index,
                ..SelectionState::default()
            },
            expansion: ExpansionState::default(),
            wrap_diff: false,
            commit_selection: HashSet::new(),
            commit_selection_repository_index: None,
            pending_clipboard: None,
            pending_jobs: HashMap::new(),
            latest_commits_job: None,
            latest_commit_detail_job: None,
            latest_commit_message_job: None,
            latest_file_diff_job: None,
            latest_reflog_job: None,
            latest_remotes_job: None,
            latest_changes_job: None,
            latest_changes_diff_job: None,
            last_error: None,
            last_message: None,
            branch_filter: String::new(),
            commit_filter: String::new(),
            tick_count: 0,
        };
        state.reconcile_focus();
        state
    }

    pub fn chord_expired(&self, now: Instant) -> bool {
        let GlobalMode::Chord { started_at, .. } = &self.mode else {
            return false;
        };
        self.config
            .keymap
            .chord_timeout
            .is_some_and(|timeout| now.saturating_duration_since(*started_at) >= timeout)
    }

    pub fn is_loading(&self) -> bool {
        !self.pending_jobs.is_empty()
    }

    pub fn repository_count(&self) -> usize {
        self.model.repository_ids().len()
    }

    pub fn repository_ui(&self, id: RepositoryId) -> Option<&RepositoryUiState> {
        self.repository_ui.get(id.0)
    }

    pub fn repository_ui_mut(&mut self, id: RepositoryId) -> Option<&mut RepositoryUiState> {
        self.repository_ui.get_mut(id.0)
    }

    pub fn active_repository_ui(&self) -> Option<&RepositoryUiState> {
        self.repository_ui(RepositoryId(self.active_repository_index?))
    }

    pub fn active_repository_ui_mut(&mut self) -> Option<&mut RepositoryUiState> {
        self.repository_ui_mut(RepositoryId(self.active_repository_index?))
    }

    pub fn active_repository(&self) -> Option<&Repository> {
        self.repository(self.active_repository_index?)
    }

    pub fn repository(&self, index: usize) -> Option<&Repository> {
        self.model.repository_summary(RepositoryId(index))
    }

    pub fn repository_node(&self, index: usize) -> Option<&super::RepositoryNode> {
        self.model.repository(RepositoryId(index))
    }

    pub fn repository_display_name(&self, index: usize) -> Option<String> {
        Some(self.repository_node(index)?.display_name())
    }

    pub fn repository_display_path(&self, index: usize) -> Option<&std::path::Path> {
        Some(self.repository_node(index)?.display_path())
    }

    pub fn repository_branch(
        &self,
        repository_index: usize,
        branch_index: usize,
    ) -> Option<&Branch> {
        self.repository_node(repository_index)?
            .branch_at(branch_index)
    }

    pub fn effective_branch_filter(&self) -> &str {
        match &self.mode {
            GlobalMode::Filtering {
                target: FilterTarget::Branches,
                query,
            } => query,
            _ => &self.branch_filter,
        }
    }

    pub fn effective_commit_filter(&self) -> &str {
        match &self.mode {
            GlobalMode::Filtering {
                target: FilterTarget::Commits,
                query,
            } => query,
            _ => &self.commit_filter,
        }
    }

    pub fn visible_tree_nodes(&self) -> Vec<BranchTreeNode> {
        let query = self.effective_branch_filter().to_lowercase();
        let mut nodes = Vec::new();

        for repository_id in self.model.repository_ids() {
            let repository_index = repository_id.0;
            let Some(repository) = self.model.repository(*repository_id) else {
                continue;
            };
            let repository_matches = query.is_empty()
                || repository.display_name().to_lowercase().contains(&query)
                || repository
                    .display_path()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&query);
            let matching_branches = self
                .model
                .branch_summaries(*repository_id)
                .into_iter()
                .enumerate()
                .filter(|(_, branch)| {
                    query.is_empty()
                        || repository_matches
                        || branch.name.0.to_lowercase().contains(&query)
                        || branch.subject.to_lowercase().contains(&query)
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();

            if !repository_matches && matching_branches.is_empty() {
                continue;
            }

            nodes.push(BranchTreeNode::Repository { repository_index });
            if self
                .repository_ui(*repository_id)
                .is_some_and(|state| state.expanded)
                || !query.is_empty()
            {
                nodes.extend(matching_branches.into_iter().map(|branch_index| {
                    BranchTreeNode::Branch {
                        repository_index,
                        branch_index,
                    }
                }));
            }
        }

        nodes
    }

    pub fn selected_tree_node(&self) -> Option<BranchTreeNode> {
        self.visible_tree_nodes()
            .get(self.selection.selected_branch_index?)
            .copied()
    }

    pub fn selected_tree_repository_index(&self) -> Option<usize> {
        self.selected_tree_node()
            .map(BranchTreeNode::repository_index)
    }

    pub fn selected_repository_node_index(&self) -> Option<usize> {
        match self.selected_tree_node()? {
            BranchTreeNode::Repository { repository_index } => Some(repository_index),
            BranchTreeNode::Branch { .. } => None,
        }
    }

    pub fn selected_branch_with_repository(&self) -> Option<(usize, &Branch)> {
        let BranchTreeNode::Branch {
            repository_index,
            branch_index,
        } = self.selected_tree_node()?
        else {
            return None;
        };
        Some((
            repository_index,
            self.repository_branch(repository_index, branch_index)?,
        ))
    }

    pub fn selected_branch(&self) -> Option<&Branch> {
        self.selected_branch_with_repository()
            .map(|(_, branch)| branch)
    }

    pub fn selected_branch_id(&self) -> Option<BranchId> {
        let (repository, branch) = self.selected_branch_with_repository()?;
        Some(BranchId {
            repository: RepositoryId(repository),
            name: branch.name.clone(),
        })
    }

    /// Branches of the active repository, retained as a convenient read-only view.
    pub fn visible_branches(&self) -> Vec<&Branch> {
        self.active_repository_index
            .map(RepositoryId)
            .map(|repository| self.model.branch_summaries(repository))
            .unwrap_or_default()
    }

    pub fn visible_commit_indices(&self) -> Vec<usize> {
        let query = self.effective_commit_filter().to_lowercase();
        self.branch_commit_summaries()
            .iter()
            .enumerate()
            .filter(|(_, commit)| {
                query.is_empty()
                    || commit.hash.0.to_lowercase().contains(&query)
                    || commit.short_hash.to_lowercase().contains(&query)
                    || commit.author.to_lowercase().contains(&query)
                    || commit.subject.to_lowercase().contains(&query)
            })
            .map(|(index, _)| index)
            .collect()
    }

    pub fn branch_commit_summaries(&self) -> Vec<&Commit> {
        self.viewing_branch
            .as_ref()
            .and_then(|branch| self.model.branch_commits(branch))
            .unwrap_or_default()
            .into_iter()
            .map(|node| &node.summary)
            .collect()
    }

    pub fn visible_commits(&self) -> Vec<&Commit> {
        let commits = self.branch_commit_summaries();
        self.visible_commit_indices()
            .into_iter()
            .filter_map(|index| commits.get(index).copied())
            .collect()
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        self.visible_commits()
            .get(self.selection.selected_commit_index?)
            .copied()
    }

    pub fn selected_commit_id(&self) -> Option<CommitId> {
        Some(CommitId {
            repository: self.viewing_branch.as_ref()?.repository,
            hash: self.selected_commit()?.hash.clone(),
        })
    }

    pub fn viewing_branch_id(&self) -> Option<BranchId> {
        self.viewing_branch.clone()
    }

    pub fn viewing_repository_index(&self) -> Option<usize> {
        self.viewing_branch
            .as_ref()
            .map(|branch| branch.repository.0)
    }

    pub fn current_commit_id(&self) -> Option<CommitId> {
        self.selected_commit_id()
    }

    pub fn current_commit_detail(&self) -> Option<CommitDetail> {
        self.model.commit_detail(&self.current_commit_id()?)
    }

    pub fn current_file_count(&self) -> usize {
        self.current_commit_id()
            .and_then(|commit| self.model.commit(&commit))
            .map_or(0, |commit| commit.file_order.len())
    }

    pub fn selected_file(&self) -> Option<&crate::domain::ChangedFile> {
        let commit = self.model.commit(&self.current_commit_id()?)?;
        let file = commit.file_order.get(self.selection.selected_file_index?)?;
        commit.files.get(file).map(|file| &file.summary)
    }

    pub fn selected_file_id(&self) -> Option<FileId> {
        let commit_id = self.current_commit_id()?;
        let commit = self.model.commit(&commit_id)?;
        commit
            .file_order
            .get(self.selection.selected_file_index?)
            .cloned()
    }

    pub fn current_file_diff(&self) -> Option<&FileDiff> {
        self.model.file_diff(&self.selected_file_id()?)
    }

    pub fn reflog_entries(&self) -> &[ReflogEntry] {
        self.reflog_repository_index
            .and_then(|index| self.model.reflog(RepositoryId(index)))
            .unwrap_or_default()
    }

    pub fn remotes(&self) -> &[RemoteInfo] {
        self.remotes_repository_index
            .and_then(|index| self.model.remotes(RepositoryId(index)))
            .unwrap_or_default()
    }

    pub fn working_tree_changes(&self) -> &[WorkingTreeChange] {
        self.changes_repository_index
            .and_then(|index| self.model.working_tree(RepositoryId(index)))
            .unwrap_or_default()
    }

    pub fn selected_reflog(&self) -> Option<&ReflogEntry> {
        self.reflog_entries()
            .get(self.selection.selected_reflog_index?)
    }

    pub fn selected_remote(&self) -> Option<&RemoteInfo> {
        self.remotes().get(self.selection.selected_remote_index?)
    }

    pub fn change_belongs_to_group(change: &WorkingTreeChange, group: ChangeGroup) -> bool {
        match group {
            // Unmerged entries are unresolved working-tree state, not a safe
            // staged snapshot, even though porcelain may place a non-space
            // character in both columns.
            ChangeGroup::Staged => !change.is_conflicted() && change.has_staged_changes(),
            ChangeGroup::Unstaged => {
                change.is_conflicted() || change.is_untracked() || change.has_worktree_changes()
            }
        }
    }

    pub fn change_group_count(&self, group: ChangeGroup) -> usize {
        self.working_tree_changes()
            .iter()
            .filter(|change| Self::change_belongs_to_group(change, group))
            .count()
    }

    pub fn visible_changes_nodes(&self) -> Vec<ChangesTreeNode> {
        let mut nodes = vec![ChangesTreeNode::Root];
        if !self.expansion.changes_root_expanded {
            return nodes;
        }

        for group in [ChangeGroup::Staged, ChangeGroup::Unstaged] {
            nodes.push(ChangesTreeNode::Group(group));
            let expanded = match group {
                ChangeGroup::Staged => self.expansion.staged_changes_expanded,
                ChangeGroup::Unstaged => self.expansion.unstaged_changes_expanded,
            };
            if expanded {
                nodes.extend(
                    self.working_tree_changes()
                        .iter()
                        .enumerate()
                        .filter(|(_, change)| Self::change_belongs_to_group(change, group))
                        .map(|(change_index, _)| ChangesTreeNode::File {
                            group,
                            change_index,
                        }),
                );
            }
        }
        nodes
    }

    pub fn selected_changes_node(&self) -> Option<ChangesTreeNode> {
        self.visible_changes_nodes()
            .get(self.selection.selected_changes_index?)
            .copied()
    }

    pub fn selected_change(&self) -> Option<(ChangeGroup, &WorkingTreeChange)> {
        let ChangesTreeNode::File {
            group,
            change_index,
        } = self.selected_changes_node()?
        else {
            return None;
        };
        Some((group, self.working_tree_changes().get(change_index)?))
    }

    pub fn selected_change_identity(&self) -> Option<(ChangeGroup, GitPath)> {
        self.selected_change()
            .map(|(group, change)| (group, change.path.clone()))
    }

    pub fn change_selection_key(group: ChangeGroup, change: &WorkingTreeChange) -> ChangeSelection {
        ChangeSelection {
            group,
            path: change.path.clone(),
        }
    }

    pub fn change_selections_in_group(&self, group: ChangeGroup) -> Vec<ChangeSelection> {
        self.working_tree_changes()
            .iter()
            .filter(|change| Self::change_belongs_to_group(change, group))
            .map(|change| Self::change_selection_key(group, change))
            .collect()
    }

    pub fn selected_change_count(&self, group: ChangeGroup) -> usize {
        self.change_selection
            .iter()
            .filter(|selection| selection.group == group)
            .count()
    }

    pub fn is_change_selected(&self, group: ChangeGroup, change: &WorkingTreeChange) -> bool {
        self.change_selection
            .contains(&Self::change_selection_key(group, change))
    }

    pub fn retain_available_change_selection(&mut self) {
        let available = [ChangeGroup::Staged, ChangeGroup::Unstaged]
            .into_iter()
            .flat_map(|group| self.change_selections_in_group(group))
            .collect::<HashSet<_>>();
        self.change_selection
            .retain(|selection| available.contains(selection));
    }

    pub fn selected_commit_hashes_for_copy(&self) -> Vec<CommitHash> {
        let selected = self
            .branch_commit_summaries()
            .iter()
            .filter(|commit| self.commit_selection.contains(&commit.hash))
            .map(|commit| commit.hash.clone())
            .collect::<Vec<_>>();
        if selected.is_empty() {
            self.selected_commit()
                .map(|commit| vec![commit.hash.clone()])
                .unwrap_or_default()
        } else {
            selected
        }
    }

    /// Returns only explicitly selected commits in replay order. `git log`
    /// lists newest commits first, while cherry-pick should normally replay a
    /// dependent series from oldest to newest.
    pub fn selected_commit_hashes_for_cherry_pick(&self) -> Vec<CommitHash> {
        self.branch_commit_summaries()
            .iter()
            .rev()
            .filter(|commit| self.commit_selection.contains(&commit.hash))
            .map(|commit| commit.hash.clone())
            .collect()
    }

    pub fn selected_commit_info_for_copy(&self) -> Option<String> {
        let commit = self.selected_commit()?;
        let mut author = commit.author.clone();
        let mut message = commit.subject.clone();
        if let Some(detail) = self
            .current_commit_detail()
            .filter(|detail| detail.commit.hash == commit.hash)
        {
            author = format!("{} <{}>", detail.commit.author, detail.author_email);
            message = detail.message.clone();
        }
        let decorations = if commit.decorations.is_empty() {
            String::new()
        } else {
            format!("\nRefs: {}", commit.decorations)
        };
        Some(format!(
            "commit {}\nAuthor: {}\nDate:   {}{}\n\n{}",
            commit.hash.0, author, commit.authored_at, decorations, message
        ))
    }

    /// Returns the full message only when detail for the currently selected
    /// commit is already available. The controller otherwise requests the
    /// message from Git instead of silently degrading to the one-line subject.
    pub fn selected_commit_message_for_copy(&self) -> Option<String> {
        let commit = self.selected_commit()?;
        self.current_commit_detail()
            .filter(|detail| detail.commit.hash == commit.hash)
            .map(|detail| detail.message.clone())
    }

    pub fn ensure_valid_branch_selection(&mut self) {
        let length = self.visible_tree_nodes().len();
        ensure_index(&mut self.selection.selected_branch_index, length);
    }

    pub fn ensure_valid_commit_selection(&mut self) {
        let length = self.visible_commit_indices().len();
        ensure_index(&mut self.selection.selected_commit_index, length);
    }

    pub fn ensure_valid_file_selection(&mut self) {
        let length = self.current_file_count();
        ensure_index(&mut self.selection.selected_file_index, length);
    }

    pub fn ensure_valid_reflog_selection(&mut self) {
        let length = self.reflog_entries().len();
        ensure_index(&mut self.selection.selected_reflog_index, length);
    }

    pub fn ensure_valid_remote_selection(&mut self) {
        let length = self.remotes().len();
        ensure_index(&mut self.selection.selected_remote_index, length);
    }

    pub fn ensure_valid_changes_selection(&mut self) {
        let nodes = self.visible_changes_nodes();
        if nodes.is_empty() {
            self.selection.selected_changes_index = None;
            return;
        }
        self.selection.selected_changes_index =
            Some(self.selection.selected_changes_index.map_or_else(
                || {
                    nodes
                        .iter()
                        .position(|node| matches!(node, ChangesTreeNode::File { .. }))
                        .unwrap_or(0)
                },
                |index| index.min(nodes.len() - 1),
            ));
    }

    /// Opening a modal deliberately leaves semantic focus untouched. The
    /// active `GlobalMode` owns modal input; closing it returns to the exact
    /// same model entity and operation scope without a parallel popup focus.
    pub fn open_popup(&mut self) {}

    pub fn close_popup(&mut self) {
        self.mode = GlobalMode::Normal;
    }

    pub fn operation_palette_matches(&self) -> Vec<OperationId> {
        let GlobalMode::OperationPalette {
            query, operations, ..
        } = &self.mode
        else {
            return Vec::new();
        };
        let query = query.trim().to_lowercase();
        operations
            .iter()
            .copied()
            .filter(|operation| {
                query.is_empty()
                    || operation.as_str().to_lowercase().contains(&query)
                    || self
                        .config
                        .operation_label(*operation)
                        .to_lowercase()
                        .contains(&query)
                    || self
                        .config
                        .keymap
                        .display_bindings_for_view(self.view_projection().view, *operation)
                        .to_lowercase()
                        .contains(&query)
            })
            .collect()
    }

    pub fn set_error(&mut self, command: String, message: String) {
        self.last_error = Some(AppError { command, message });
        self.open_popup();
        self.mode = GlobalMode::Error;
    }

    pub fn dismiss_error(&mut self) {
        self.last_error = None;
        self.close_popup();
    }

    pub fn diff_line_count(&self) -> usize {
        file_diff_line_count(self.current_file_diff())
    }

    pub fn changes_diff_line_count(&self) -> usize {
        file_diff_line_count(self.current_changes_diff.as_ref())
    }
}

fn file_diff_line_count(diff: Option<&FileDiff>) -> usize {
    diff.map_or(0, |diff| {
        diff.header.len()
            + diff
                .hunks
                .iter()
                .map(|hunk| 1 + hunk.lines.len())
                .sum::<usize>()
    })
}

pub fn ensure_index(index: &mut Option<usize>, length: usize) {
    if length == 0 {
        *index = None;
    } else {
        *index = Some(index.unwrap_or(0).min(length - 1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str) -> Branch {
        Branch {
            name: BranchName(name.into()),
            full_ref: format!("refs/heads/{name}"),
            kind: crate::domain::BranchKind::Local,
            head: CommitHash("0123456789".into()),
            short_head: "01234567".into(),
            commit_date: String::new(),
            subject: String::new(),
            is_current: name == "main",
        }
    }

    #[test]
    fn repairs_selection_after_a_list_shrinks() {
        let mut index = Some(8);
        ensure_index(&mut index, 3);
        assert_eq!(index, Some(2));
        ensure_index(&mut index, 0);
        assert_eq!(index, None);
        ensure_index(&mut index, 2);
        assert_eq!(index, Some(0));
    }

    #[test]
    fn builds_and_filters_repository_branch_tree() {
        let mut state = AppState::with_repository_paths(vec!["one".into(), "two".into()]);
        state
            .model
            .replace_branches(RepositoryId(0), vec![branch("main"), branch("feature")]);
        state
            .model
            .replace_branches(RepositoryId(1), vec![branch("release")]);
        assert_eq!(state.visible_tree_nodes().len(), 5);
        assert_eq!(
            state.visible_tree_nodes()[3],
            BranchTreeNode::Repository {
                repository_index: 1
            }
        );

        state.repository_ui[0].expanded = false;
        assert_eq!(state.visible_tree_nodes().len(), 3);

        state.branch_filter = "release".into();
        assert_eq!(
            state.visible_tree_nodes(),
            vec![
                BranchTreeNode::Repository {
                    repository_index: 1
                },
                BranchTreeNode::Branch {
                    repository_index: 1,
                    branch_index: 0
                }
            ]
        );
    }

    #[test]
    fn exposes_an_unborn_current_branch_as_a_tree_child() {
        let mut state = AppState::with_repository_paths(vec!["repo".into()]);
        state.model.set_repository_summary(
            RepositoryId(0),
            Repository {
                root: "repo".into(),
                name: "repo".into(),
                current_branch: Some(BranchName("main".into())),
                head: CommitHash(String::new()),
                status: crate::domain::WorkingTreeStatus::default(),
            },
        );

        assert_eq!(state.model.branch_summaries(RepositoryId(0)).len(), 1);
        let main = state.repository_branch(0, 0).unwrap();
        assert_eq!(main.name.0, "main");
        assert_eq!(main.short_head, "unborn");
        assert!(main.is_current);
        assert_eq!(
            state.visible_tree_nodes(),
            vec![
                BranchTreeNode::Repository {
                    repository_index: 0
                },
                BranchTreeNode::Branch {
                    repository_index: 0,
                    branch_index: 0
                }
            ]
        );
    }

    #[test]
    fn builds_a_three_level_changes_tree_and_duplicates_mm_across_groups() {
        let changes = vec![
            WorkingTreeChange {
                index_status: 'M',
                worktree_status: 'M',
                path: GitPath::from("both.txt"),
                old_path: None,
            },
            WorkingTreeChange {
                index_status: 'A',
                worktree_status: ' ',
                path: GitPath::from("staged.txt"),
                old_path: None,
            },
            WorkingTreeChange {
                index_status: '?',
                worktree_status: '?',
                path: GitPath::from("new.txt"),
                old_path: None,
            },
            WorkingTreeChange {
                index_status: 'U',
                worktree_status: 'U',
                path: GitPath::from("conflict.txt"),
                old_path: None,
            },
        ];
        let mut state = AppState::with_repository_paths(vec![PathBuf::from("/repo")]);
        state.changes_repository_index = Some(0);
        state.model.set_working_tree(RepositoryId(0), changes);

        assert_eq!(state.change_group_count(ChangeGroup::Staged), 2);
        assert_eq!(state.change_group_count(ChangeGroup::Unstaged), 3);
        let nodes = state.visible_changes_nodes();
        assert_eq!(nodes[0], ChangesTreeNode::Root);
        assert_eq!(nodes[1], ChangesTreeNode::Group(ChangeGroup::Staged));
        assert_eq!(
            nodes
                .iter()
                .filter(|node| matches!(node, ChangesTreeNode::File { .. }))
                .count(),
            5
        );
        assert_eq!(
            nodes
                .iter()
                .filter(|node| {
                    matches!(
                        node,
                        ChangesTreeNode::File {
                            change_index: 0,
                            ..
                        }
                    )
                })
                .count(),
            2
        );

        state.ensure_valid_changes_selection();
        assert!(matches!(
            state.selected_changes_node(),
            Some(ChangesTreeNode::File {
                group: ChangeGroup::Staged,
                ..
            })
        ));
        state.expansion.changes_root_expanded = false;
        state.ensure_valid_changes_selection();
        assert_eq!(state.visible_changes_nodes(), vec![ChangesTreeNode::Root]);
        assert_eq!(state.selected_changes_node(), Some(ChangesTreeNode::Root));
    }

    #[test]
    fn formats_selected_commit_hashes_and_current_info_for_clipboard() {
        let mut state = AppState::with_repository_paths(vec![PathBuf::from("/repo")]);
        let commits = vec![
            Commit {
                hash: CommitHash("aaaaaaaa".into()),
                short_hash: "aaaaaaaa".into(),
                author: "Ada".into(),
                authored_at: "2026-07-16".into(),
                decorations: "HEAD -> main".into(),
                subject: "first".into(),
            },
            Commit {
                hash: CommitHash("bbbbbbbb".into()),
                short_hash: "bbbbbbbb".into(),
                author: "Lin".into(),
                authored_at: "2026-07-15".into(),
                decorations: String::new(),
                subject: "second".into(),
            },
        ];
        let branch = BranchId {
            repository: RepositoryId(0),
            name: BranchName("main".into()),
        };
        state.model.replace_branch_commits(&branch, commits.clone());
        state.viewing_branch = Some(branch);
        state.ensure_valid_commit_selection();
        state.commit_selection.insert(CommitHash("bbbbbbbb".into()));
        state.commit_selection.insert(CommitHash("aaaaaaaa".into()));

        assert_eq!(
            state.selected_commit_hashes_for_copy(),
            vec![CommitHash("aaaaaaaa".into()), CommitHash("bbbbbbbb".into())]
        );
        assert_eq!(
            state.selected_commit_hashes_for_cherry_pick(),
            vec![CommitHash("bbbbbbbb".into()), CommitHash("aaaaaaaa".into())]
        );
        let info = state.selected_commit_info_for_copy().unwrap();
        assert!(info.contains("commit aaaaaaaa"));
        assert!(info.contains("Author: Ada"));
        assert!(info.contains("Refs: HEAD -> main"));
        assert!(info.ends_with("first"));

        assert_eq!(state.selected_commit_message_for_copy(), None);
        state.model.set_commit_detail(
            RepositoryId(0),
            CommitDetail {
                commit: commits[0].clone(),
                author_email: "ada@example.invalid".into(),
                committer: "Ada".into(),
                committer_email: "ada@example.invalid".into(),
                committed_at: "2026-07-16".into(),
                message: "first\n\nfull body".into(),
                files: Vec::new(),
            },
        );
        assert_eq!(
            state.selected_commit_message_for_copy().as_deref(),
            Some("first\n\nfull body")
        );
    }
}
