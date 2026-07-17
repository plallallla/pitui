use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::Instant,
};

use crate::{
    config::{KeyStroke, ResolvedConfig},
    domain::{
        Branch, BranchName, Commit, CommitDetail, CommitHash, CommitList, FileDiff, GitPath,
        ReflogEntry, RemoteInfo, Repository, WorkingTreeChange,
    },
    git::{GitJobId, ResetMode},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Screen {
    #[default]
    BranchOverview,
    CommitDetail,
    FileDiffDetail,
    Reflog,
    Changes,
    Remotes,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FocusPanel {
    #[default]
    BranchList,
    CommitList,
    CommitFileList,
    FileList,
    DiffView,
    ReflogList,
    ChangesTree,
    ChangesDiff,
    RemoteList,
    Popup,
}

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
    CherryPickQueue {
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

#[derive(Debug)]
pub struct RepositoryState {
    pub requested_path: PathBuf,
    pub repository: Option<Repository>,
    pub branches: Vec<Branch>,
    pub expanded: bool,
    pub last_error: Option<AppError>,
    pub viewing_branch: Option<BranchName>,
    pub latest_status_job: Option<GitJobId>,
    pub latest_branches_job: Option<GitJobId>,
}

impl RepositoryState {
    pub fn new(requested_path: PathBuf) -> Self {
        Self {
            requested_path,
            repository: None,
            branches: Vec::new(),
            expanded: true,
            last_error: None,
            viewing_branch: None,
            latest_status_job: None,
            latest_branches_job: None,
        }
    }

    pub fn git_cwd(&self) -> &Path {
        self.repository
            .as_ref()
            .map_or(self.requested_path.as_path(), |repository| {
                repository.root.as_path()
            })
    }

    pub fn display_name(&self) -> String {
        self.repository.as_ref().map_or_else(
            || {
                self.requested_path
                    .file_name()
                    .filter(|name| !name.is_empty())
                    .map_or_else(
                        || self.requested_path.display().to_string(),
                        |name| name.to_string_lossy().into_owned(),
                    )
            },
            |repository| repository.name.clone(),
        )
    }

    pub fn display_path(&self) -> &Path {
        self.repository
            .as_ref()
            .map_or(self.requested_path.as_path(), |repository| {
                repository.root.as_path()
            })
    }

    /// `git for-each-ref refs/heads` cannot return an unborn branch because
    /// the ref does not exist yet. Repository status still knows its name, so
    /// synthesize that current branch as a real tree child until the first
    /// commit creates the ref.
    pub fn ensure_current_branch_visible(&mut self) {
        let Some(repository) = self.repository.as_ref() else {
            return;
        };
        let Some(current_branch) = repository.current_branch.as_ref() else {
            for branch in &mut self.branches {
                branch.is_current = false;
            }
            return;
        };

        let mut found = false;
        for branch in &mut self.branches {
            branch.is_current = branch.name == *current_branch;
            if branch.is_current && branch.head.0.is_empty() && !repository.head.0.is_empty() {
                branch.head = repository.head.clone();
                branch.short_head = repository.head.short().to_string();
                branch.subject.clear();
            }
            found |= branch.is_current;
        }
        if found {
            return;
        }

        let short_head = if repository.head.0.is_empty() {
            "unborn".to_string()
        } else {
            repository.head.short().to_string()
        };
        self.branches.insert(
            0,
            Branch {
                name: current_branch.clone(),
                full_ref: format!("refs/heads/{}", current_branch.0),
                kind: crate::domain::BranchKind::Local,
                head: repository.head.clone(),
                short_head,
                commit_date: String::new(),
                subject: if repository.head.0.is_empty() {
                    "Unborn branch (no commits yet)".into()
                } else {
                    String::new()
                },
                is_current: true,
            },
        );
    }
}

#[derive(Debug)]
pub struct AppState {
    /// Immutable effective configuration snapshot shared by input and
    /// rendering, so hints can never disagree with the active bindings.
    pub config: Arc<ResolvedConfig>,
    pub repositories: Vec<RepositoryState>,
    /// Persistent JSONL audit trail written by the Git worker. The value is
    /// exposed to the UI so users can discover the effective path, including
    /// a temporary fallback when the platform default cannot be opened.
    pub backend_log_path: Option<PathBuf>,
    pub backend_logging_warning: Option<String>,
    pub active_repository_index: Option<usize>,
    pub branch_commits_repository_index: Option<usize>,
    pub branch_commits: CommitList,
    pub reflog_repository_index: Option<usize>,
    pub reflog_entries: Vec<ReflogEntry>,
    pub remotes_repository_index: Option<usize>,
    pub remotes: Vec<RemoteInfo>,
    pub changes_repository_index: Option<usize>,
    pub changes: Vec<WorkingTreeChange>,
    pub current_changes_diff: Option<FileDiff>,
    pub current_changes_diff_group: Option<ChangeGroup>,
    /// File/group selections used by stage and unstage. The group is part of
    /// the key because an `MM` path intentionally appears twice.
    pub change_selection: HashSet<ChangeSelection>,
    /// Screen/focus to restore when Changes was opened through the global
    /// shortcut. This makes Changes an overlay-like destination rather than a
    /// child of Branch Overview.
    pub changes_return_context: Option<(Screen, FocusPanel)>,
    pub current_commit_detail: Option<CommitDetail>,
    pub current_file_diff: Option<FileDiff>,
    pub screen: Screen,
    pub focus: FocusPanel,
    pub previous_focus: Option<FocusPanel>,
    pub mode: GlobalMode,
    pub selection: SelectionState,
    pub expansion: ExpansionState,
    pub diff_mode: DiffViewMode,
    pub wrap_diff: bool,
    pub cherry_pick_queue: Vec<CommitHash>,
    pub cherry_pick_queue_repository_index: Option<usize>,
    /// Independent selection used only for copying commit hashes. Cherry-pick
    /// queue membership must never implicitly change clipboard selection.
    pub commit_copy_selection: HashSet<CommitHash>,
    pub commit_copy_selection_repository_index: Option<usize>,
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
        let repositories = paths
            .into_iter()
            .map(RepositoryState::new)
            .collect::<Vec<_>>();
        let active_repository_index = (!repositories.is_empty()).then_some(0);
        Self {
            diff_mode: config.diff.default_mode,
            config,
            repositories,
            backend_log_path: None,
            backend_logging_warning: None,
            active_repository_index,
            branch_commits_repository_index: active_repository_index,
            branch_commits: CommitList::empty(),
            reflog_repository_index: None,
            reflog_entries: Vec::new(),
            remotes_repository_index: None,
            remotes: Vec::new(),
            changes_repository_index: None,
            changes: Vec::new(),
            current_changes_diff: None,
            current_changes_diff_group: None,
            change_selection: HashSet::new(),
            changes_return_context: None,
            current_commit_detail: None,
            current_file_diff: None,
            screen: Screen::BranchOverview,
            focus: FocusPanel::BranchList,
            previous_focus: None,
            mode: GlobalMode::Normal,
            selection: SelectionState {
                selected_branch_index: active_repository_index,
                ..SelectionState::default()
            },
            expansion: ExpansionState::default(),
            wrap_diff: false,
            cherry_pick_queue: Vec::new(),
            cherry_pick_queue_repository_index: None,
            commit_copy_selection: HashSet::new(),
            commit_copy_selection_repository_index: None,
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
        }
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

    pub fn active_repository_state(&self) -> Option<&RepositoryState> {
        self.repositories.get(self.active_repository_index?)
    }

    pub fn active_repository_state_mut(&mut self) -> Option<&mut RepositoryState> {
        self.repositories.get_mut(self.active_repository_index?)
    }

    pub fn active_repository(&self) -> Option<&Repository> {
        self.active_repository_state()?.repository.as_ref()
    }

    pub fn repository(&self, index: usize) -> Option<&Repository> {
        self.repositories.get(index)?.repository.as_ref()
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

        for (repository_index, repository) in self.repositories.iter().enumerate() {
            let repository_matches = query.is_empty()
                || repository.display_name().to_lowercase().contains(&query)
                || repository
                    .display_path()
                    .to_string_lossy()
                    .to_lowercase()
                    .contains(&query);
            let matching_branches = repository
                .branches
                .iter()
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
            if repository.expanded || !query.is_empty() {
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
            self.repositories
                .get(repository_index)?
                .branches
                .get(branch_index)?,
        ))
    }

    pub fn selected_branch(&self) -> Option<&Branch> {
        self.selected_branch_with_repository()
            .map(|(_, branch)| branch)
    }

    /// Branches of the active repository, retained as a convenient read-only view.
    pub fn visible_branches(&self) -> Vec<&Branch> {
        self.active_repository_state()
            .map(|repository| repository.branches.iter().collect())
            .unwrap_or_default()
    }

    pub fn visible_commit_indices(&self) -> Vec<usize> {
        let query = self.effective_commit_filter().to_lowercase();
        self.branch_commits
            .items
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

    pub fn visible_commits(&self) -> Vec<&Commit> {
        self.visible_commit_indices()
            .into_iter()
            .filter_map(|index| self.branch_commits.items.get(index))
            .collect()
    }

    pub fn selected_commit(&self) -> Option<&Commit> {
        let source_index = *self
            .visible_commit_indices()
            .get(self.selection.selected_commit_index?)?;
        self.branch_commits.items.get(source_index)
    }

    pub fn selected_file(&self) -> Option<&crate::domain::ChangedFile> {
        self.current_commit_detail
            .as_ref()?
            .file(self.selection.selected_file_index)
    }

    pub fn selected_reflog(&self) -> Option<&ReflogEntry> {
        self.reflog_entries
            .get(self.selection.selected_reflog_index?)
    }

    pub fn selected_remote(&self) -> Option<&RemoteInfo> {
        self.remotes.get(self.selection.selected_remote_index?)
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
        self.changes
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
                    self.changes
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
        Some((group, self.changes.get(change_index)?))
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
        self.changes
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
            .branch_commits
            .items
            .iter()
            .filter(|commit| self.commit_copy_selection.contains(&commit.hash))
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

    pub fn selected_commit_info_for_copy(&self) -> Option<String> {
        let commit = self.selected_commit()?;
        let mut author = commit.author.clone();
        let mut message = commit.subject.clone();
        if let Some(detail) = self
            .current_commit_detail
            .as_ref()
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
        self.current_commit_detail
            .as_ref()
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
        let length = self
            .current_commit_detail
            .as_ref()
            .map_or(0, |detail| detail.files.len());
        ensure_index(&mut self.selection.selected_file_index, length);
    }

    pub fn ensure_valid_reflog_selection(&mut self) {
        ensure_index(
            &mut self.selection.selected_reflog_index,
            self.reflog_entries.len(),
        );
    }

    pub fn ensure_valid_remote_selection(&mut self) {
        ensure_index(
            &mut self.selection.selected_remote_index,
            self.remotes.len(),
        );
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

    pub fn default_focus_for_screen(&self) -> FocusPanel {
        match self.screen {
            Screen::BranchOverview => FocusPanel::BranchList,
            Screen::CommitDetail => FocusPanel::CommitFileList,
            Screen::FileDiffDetail => FocusPanel::DiffView,
            Screen::Reflog => FocusPanel::ReflogList,
            Screen::Changes => FocusPanel::ChangesTree,
            Screen::Remotes => FocusPanel::RemoteList,
        }
    }

    pub fn open_popup(&mut self) {
        if self.focus != FocusPanel::Popup {
            self.previous_focus = Some(self.focus);
        }
        self.focus = FocusPanel::Popup;
    }

    pub fn close_popup(&mut self) {
        self.focus = self
            .previous_focus
            .take()
            .unwrap_or_else(|| self.default_focus_for_screen());
        self.mode = GlobalMode::Normal;
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
        file_diff_line_count(self.current_file_diff.as_ref())
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
        state.repositories[0].branches = vec![branch("main"), branch("feature")];
        state.repositories[1].branches = vec![branch("release")];
        assert_eq!(state.visible_tree_nodes().len(), 5);
        assert_eq!(
            state.visible_tree_nodes()[3],
            BranchTreeNode::Repository {
                repository_index: 1
            }
        );

        state.repositories[0].expanded = false;
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
        state.repositories[0].repository = Some(Repository {
            root: "repo".into(),
            name: "repo".into(),
            current_branch: Some(BranchName("main".into())),
            head: CommitHash(String::new()),
            status: crate::domain::WorkingTreeStatus::default(),
        });
        state.repositories[0].ensure_current_branch_visible();

        assert_eq!(state.repositories[0].branches.len(), 1);
        assert_eq!(state.repositories[0].branches[0].name.0, "main");
        assert_eq!(state.repositories[0].branches[0].short_head, "unborn");
        assert!(state.repositories[0].branches[0].is_current);
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
        let mut state = AppState {
            changes: vec![
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
            ],
            ..AppState::default()
        };

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
        let mut state = AppState::default();
        state.branch_commits.items = vec![
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
        state.ensure_valid_commit_selection();
        state
            .commit_copy_selection
            .insert(CommitHash("bbbbbbbb".into()));
        state
            .commit_copy_selection
            .insert(CommitHash("aaaaaaaa".into()));

        assert_eq!(
            state.selected_commit_hashes_for_copy(),
            vec![CommitHash("aaaaaaaa".into()), CommitHash("bbbbbbbb".into())]
        );
        let info = state.selected_commit_info_for_copy().unwrap();
        assert!(info.contains("commit aaaaaaaa"));
        assert!(info.contains("Author: Ada"));
        assert!(info.contains("Refs: HEAD -> main"));
        assert!(info.ends_with("first"));

        assert_eq!(state.selected_commit_message_for_copy(), None);
        state.current_commit_detail = Some(CommitDetail {
            commit: state.branch_commits.items[0].clone(),
            author_email: "ada@example.invalid".into(),
            committer: "Ada".into(),
            committer_email: "ada@example.invalid".into(),
            committed_at: "2026-07-16".into(),
            message: "first\n\nfull body".into(),
            files: Vec::new(),
        });
        assert_eq!(
            state.selected_commit_message_for_copy().as_deref(),
            Some("first\n\nfull body")
        );
    }
}
