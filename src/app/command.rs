use std::collections::HashSet;

use crate::domain::GitPath;

use super::{Action, AppState, ChangeGroup, ChangesTreeNode, FocusPanel, PendingJobKind, Screen};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CommandId {
    AppQuit,
    AppRefresh,
    ViewChangesToggle,
    FocusNext,
    FocusPrevious,
    NavigationBack,
    NavigationUp,
    NavigationDown,
    NavigationLeft,
    NavigationRight,
    NavigationPageUp,
    NavigationPageDown,
    NavigationHome,
    NavigationEnd,
    RepositoryActivate,
    RepositoryFetch,
    RepositoryPullRebase,
    RepositoryPush,
    RepositoryRemotesOpen,
    RepositoryReflogOpen,
    BranchSwitch,
    BranchRebase,
    ListFilter,
    CommitOpenDetail,
    CommitToggleSelection,
    CommitQueueCherryPick,
    CommitApplyCherryPickQueue,
    CommitReset,
    CommitFileToggleExpanded,
    CommitFileOpenDiff,
    FileOpenDiff,
    FileNext,
    FilePrevious,
    DiffModeToggle,
    DiffWrapToggle,
    ChangesActivate,
    ChangesToggleSelection,
    ChangesStage,
    ChangesUnstage,
    ChangesCommit,
    RemoteAdd,
    RemoteSetSharedUrl,
    RemoteSetUpstream,
    CommitCopyHash,
    CommitCopyInfo,
    CommitCopyMessage,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum FooterGroup {
    Safety,
    Global,
    Primary,
    Contextual,
    Navigation,
}

impl FooterGroup {
    pub fn rank(self) -> u8 {
        match self {
            Self::Safety => 0,
            Self::Global => 1,
            Self::Primary => 2,
            Self::Contextual => 3,
            Self::Navigation => 4,
        }
    }
}

impl CommandId {
    pub const ALL: &'static [Self] = &[
        Self::AppQuit,
        Self::AppRefresh,
        Self::ViewChangesToggle,
        Self::FocusNext,
        Self::FocusPrevious,
        Self::NavigationBack,
        Self::NavigationUp,
        Self::NavigationDown,
        Self::NavigationLeft,
        Self::NavigationRight,
        Self::NavigationPageUp,
        Self::NavigationPageDown,
        Self::NavigationHome,
        Self::NavigationEnd,
        Self::RepositoryActivate,
        Self::RepositoryFetch,
        Self::RepositoryPullRebase,
        Self::RepositoryPush,
        Self::RepositoryRemotesOpen,
        Self::RepositoryReflogOpen,
        Self::BranchSwitch,
        Self::BranchRebase,
        Self::ListFilter,
        Self::CommitOpenDetail,
        Self::CommitToggleSelection,
        Self::CommitQueueCherryPick,
        Self::CommitApplyCherryPickQueue,
        Self::CommitReset,
        Self::CommitFileToggleExpanded,
        Self::CommitFileOpenDiff,
        Self::FileOpenDiff,
        Self::FileNext,
        Self::FilePrevious,
        Self::DiffModeToggle,
        Self::DiffWrapToggle,
        Self::ChangesActivate,
        Self::ChangesToggleSelection,
        Self::ChangesStage,
        Self::ChangesUnstage,
        Self::ChangesCommit,
        Self::RemoteAdd,
        Self::RemoteSetSharedUrl,
        Self::RemoteSetUpstream,
        Self::CommitCopyHash,
        Self::CommitCopyInfo,
        Self::CommitCopyMessage,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|command| command.as_str() == value)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::AppQuit => "app.quit",
            Self::AppRefresh => "app.refresh",
            Self::ViewChangesToggle => "view.changes.toggle",
            Self::FocusNext => "focus.next",
            Self::FocusPrevious => "focus.previous",
            Self::NavigationBack => "navigation.back",
            Self::NavigationUp => "navigation.up",
            Self::NavigationDown => "navigation.down",
            Self::NavigationLeft => "navigation.left",
            Self::NavigationRight => "navigation.right",
            Self::NavigationPageUp => "navigation.page_up",
            Self::NavigationPageDown => "navigation.page_down",
            Self::NavigationHome => "navigation.home",
            Self::NavigationEnd => "navigation.end",
            Self::RepositoryActivate => "repository.activate",
            Self::RepositoryFetch => "repository.fetch",
            Self::RepositoryPullRebase => "repository.pull_rebase",
            Self::RepositoryPush => "repository.push",
            Self::RepositoryRemotesOpen => "repository.remotes.open",
            Self::RepositoryReflogOpen => "repository.reflog.open",
            Self::BranchSwitch => "branch.switch",
            Self::BranchRebase => "branch.rebase",
            Self::ListFilter => "list.filter",
            Self::CommitOpenDetail => "commit.open_detail",
            Self::CommitToggleSelection => "commit.toggle_selection",
            Self::CommitQueueCherryPick => "commit.cherry_pick.queue",
            Self::CommitApplyCherryPickQueue => "commit.cherry_pick.apply_queue",
            Self::CommitReset => "commit.reset",
            Self::CommitFileToggleExpanded => "commit.file.toggle_expanded",
            Self::CommitFileOpenDiff => "commit.file.open_diff",
            Self::FileOpenDiff => "file.open_diff",
            Self::FileNext => "file.next",
            Self::FilePrevious => "file.previous",
            Self::DiffModeToggle => "diff.mode.toggle",
            Self::DiffWrapToggle => "diff.wrap.toggle",
            Self::ChangesActivate => "changes.activate",
            Self::ChangesToggleSelection => "changes.toggle_selection",
            Self::ChangesStage => "changes.stage",
            Self::ChangesUnstage => "changes.unstage",
            Self::ChangesCommit => "changes.commit",
            Self::RemoteAdd => "remote.add",
            Self::RemoteSetSharedUrl => "remote.set_shared_url",
            Self::RemoteSetUpstream => "remote.set_upstream",
            Self::CommitCopyHash => "commit.copy.hash",
            Self::CommitCopyInfo => "commit.copy.info",
            Self::CommitCopyMessage => "commit.copy.message",
        }
    }

    pub fn default_bindings(self) -> &'static [&'static str] {
        match self {
            Self::AppQuit => &["q", "Ctrl+C"],
            Self::AppRefresh => &["Ctrl+R"],
            Self::ViewChangesToggle => &["Ctrl+G"],
            Self::FocusNext => &["Tab"],
            Self::FocusPrevious => &["BackTab"],
            Self::NavigationBack => &["Esc"],
            Self::NavigationUp => &["Up", "k"],
            Self::NavigationDown => &["Down", "j"],
            Self::NavigationLeft => &["Left", "h"],
            Self::NavigationRight => &["Right", "l"],
            Self::NavigationPageUp => &["PageUp"],
            Self::NavigationPageDown => &["PageDown"],
            Self::NavigationHome => &["Home"],
            Self::NavigationEnd => &["End"],
            Self::RepositoryActivate | Self::CommitOpenDetail | Self::ChangesActivate => &["Enter"],
            Self::RepositoryFetch => &["f"],
            Self::RepositoryPullRebase => &["p"],
            Self::RepositoryPush => &["P"],
            Self::RepositoryRemotesOpen => &["o"],
            Self::RepositoryReflogOpen => &["g"],
            Self::BranchSwitch => &["s"],
            Self::BranchRebase => &["b"],
            Self::ListFilter => &["/"],
            Self::CommitToggleSelection
            | Self::CommitFileToggleExpanded
            | Self::ChangesToggleSelection => &["Space"],
            Self::CommitQueueCherryPick => &["y"],
            Self::CommitApplyCherryPickQueue => &["Y"],
            Self::CommitReset => &["R"],
            Self::CommitFileOpenDiff => &["Enter", "v"],
            Self::FileOpenDiff => &["Enter"],
            Self::FileNext => &["n"],
            Self::FilePrevious => &["p"],
            Self::DiffModeToggle => &["v"],
            Self::DiffWrapToggle => &["w"],
            Self::ChangesStage => &["s"],
            Self::ChangesUnstage => &["u"],
            Self::ChangesCommit => &["c"],
            Self::RemoteAdd => &["a"],
            Self::RemoteSetSharedUrl => &["e"],
            Self::RemoteSetUpstream => &["u"],
            Self::CommitCopyHash => &["Ctrl+C h"],
            Self::CommitCopyInfo => &["Ctrl+C i"],
            Self::CommitCopyMessage => &["Ctrl+C m"],
        }
    }

    pub fn default_label(self) -> &'static str {
        match self {
            Self::AppQuit => "quit",
            Self::AppRefresh => "refresh",
            Self::ViewChangesToggle => "changes",
            Self::FocusNext | Self::FocusPrevious => "focus",
            Self::NavigationBack => "back",
            Self::NavigationUp => "up",
            Self::NavigationDown => "down",
            Self::NavigationLeft => "left",
            Self::NavigationRight => "right",
            Self::NavigationPageUp => "page up",
            Self::NavigationPageDown => "page down",
            Self::NavigationHome => "first/top",
            Self::NavigationEnd => "last/bottom",
            Self::RepositoryActivate => "open",
            Self::RepositoryFetch => "fetch",
            Self::RepositoryPullRebase => "pull --rebase",
            Self::RepositoryPush => "push",
            Self::RepositoryRemotesOpen => "remotes",
            Self::RepositoryReflogOpen => "reflog",
            Self::BranchSwitch => "switch",
            Self::BranchRebase => "rebase",
            Self::ListFilter => "filter",
            Self::CommitOpenDetail => "detail",
            Self::CommitToggleSelection => "select",
            Self::CommitQueueCherryPick => "queue",
            Self::CommitApplyCherryPickQueue => "cherry-pick",
            Self::CommitReset => "reset",
            Self::CommitFileToggleExpanded => "expand",
            Self::CommitFileOpenDiff => "file diff",
            Self::FileOpenDiff => "file diff",
            Self::FileNext => "next",
            Self::FilePrevious => "previous",
            Self::DiffModeToggle => "mode",
            Self::DiffWrapToggle => "wrap",
            Self::ChangesActivate => "open/toggle",
            Self::ChangesToggleSelection => "select",
            Self::ChangesStage => "stage",
            Self::ChangesUnstage => "unstage",
            Self::ChangesCommit => "commit",
            Self::RemoteAdd => "add remote",
            Self::RemoteSetSharedUrl => "set shared URL",
            Self::RemoteSetUpstream => "set upstream",
            Self::CommitCopyHash => "hash",
            Self::CommitCopyInfo => "info",
            Self::CommitCopyMessage => "message",
        }
    }

    pub fn default_visible(self) -> bool {
        !matches!(
            self,
            Self::FocusPrevious
                | Self::NavigationUp
                | Self::NavigationDown
                | Self::NavigationLeft
                | Self::NavigationRight
        )
    }

    pub fn footer_group(self) -> FooterGroup {
        match self {
            Self::AppRefresh | Self::ViewChangesToggle => FooterGroup::Global,
            Self::AppQuit | Self::NavigationBack => FooterGroup::Safety,
            Self::FocusNext
            | Self::FocusPrevious
            | Self::NavigationUp
            | Self::NavigationDown
            | Self::NavigationLeft
            | Self::NavigationRight
            | Self::NavigationPageUp
            | Self::NavigationPageDown
            | Self::NavigationHome
            | Self::NavigationEnd
            | Self::FileNext
            | Self::FilePrevious => FooterGroup::Navigation,
            Self::ListFilter | Self::DiffModeToggle | Self::DiffWrapToggle => {
                FooterGroup::Contextual
            }
            _ => FooterGroup::Primary,
        }
    }

    pub fn default_priority(self) -> u16 {
        match self.footer_group() {
            FooterGroup::Safety => 120,
            FooterGroup::Global => 110,
            FooterGroup::Primary => 90,
            FooterGroup::Contextual => 70,
            FooterGroup::Navigation => 50,
        }
    }

    pub fn chord_group(self) -> Option<&'static str> {
        matches!(
            self,
            Self::CommitCopyHash | Self::CommitCopyInfo | Self::CommitCopyMessage
        )
        .then_some("commit.copy")
    }

    /// Conservative static contexts used while validating configured key
    /// collisions. Runtime actionability applies the finer state predicates.
    pub fn context_mask(self) -> u16 {
        const BRANCH_TREE: u16 = 1 << 0;
        const BRANCH_COMMITS: u16 = 1 << 1;
        const DETAIL_COMMITS: u16 = 1 << 2;
        const DETAIL_FILES: u16 = 1 << 3;
        const FILE_LIST: u16 = 1 << 4;
        const FILE_DIFF: u16 = 1 << 5;
        const REFLOG: u16 = 1 << 6;
        const CHANGES_TREE: u16 = 1 << 7;
        const CHANGES_DIFF: u16 = 1 << 8;
        const REMOTES: u16 = 1 << 9;
        const ALL: u16 = (1 << 10) - 1;

        match self {
            Self::AppQuit
            | Self::AppRefresh
            | Self::ViewChangesToggle
            | Self::NavigationUp
            | Self::NavigationDown
            | Self::NavigationPageUp
            | Self::NavigationPageDown
            | Self::NavigationHome
            | Self::NavigationEnd => ALL,
            Self::FocusNext | Self::FocusPrevious => {
                BRANCH_TREE
                    | BRANCH_COMMITS
                    | DETAIL_COMMITS
                    | DETAIL_FILES
                    | FILE_LIST
                    | FILE_DIFF
                    | CHANGES_TREE
                    | CHANGES_DIFF
            }
            Self::NavigationBack => ALL & !BRANCH_TREE & !BRANCH_COMMITS,
            Self::NavigationLeft | Self::NavigationRight => {
                BRANCH_TREE
                    | BRANCH_COMMITS
                    | DETAIL_COMMITS
                    | DETAIL_FILES
                    | FILE_LIST
                    | FILE_DIFF
                    | CHANGES_TREE
                    | CHANGES_DIFF
            }
            Self::RepositoryActivate
            | Self::RepositoryFetch
            | Self::RepositoryPullRebase
            | Self::RepositoryPush
            | Self::RepositoryRemotesOpen
            | Self::RepositoryReflogOpen
            | Self::BranchSwitch
            | Self::BranchRebase => BRANCH_TREE,
            Self::ListFilter => BRANCH_TREE | BRANCH_COMMITS | DETAIL_COMMITS,
            Self::CommitOpenDetail
            | Self::CommitToggleSelection
            | Self::CommitApplyCherryPickQueue => BRANCH_COMMITS | DETAIL_COMMITS,
            Self::CommitQueueCherryPick => BRANCH_COMMITS | DETAIL_COMMITS | DETAIL_FILES,
            Self::CommitReset => BRANCH_COMMITS | DETAIL_COMMITS | REFLOG,
            Self::CommitFileToggleExpanded => DETAIL_FILES,
            Self::CommitFileOpenDiff => DETAIL_FILES,
            Self::FileOpenDiff => FILE_LIST,
            Self::FileNext | Self::FilePrevious => FILE_LIST | FILE_DIFF,
            Self::DiffModeToggle | Self::DiffWrapToggle => {
                FILE_LIST | FILE_DIFF | CHANGES_TREE | CHANGES_DIFF
            }
            Self::ChangesActivate => CHANGES_TREE,
            Self::ChangesToggleSelection
            | Self::ChangesStage
            | Self::ChangesUnstage
            | Self::ChangesCommit => CHANGES_TREE | CHANGES_DIFF,
            Self::RemoteAdd | Self::RemoteSetSharedUrl | Self::RemoteSetUpstream => REMOTES,
            Self::CommitCopyHash | Self::CommitCopyInfo | Self::CommitCopyMessage => {
                BRANCH_TREE | BRANCH_COMMITS | DETAIL_COMMITS | DETAIL_FILES | FILE_LIST | FILE_DIFF
            }
        }
    }

    pub fn action(self, app: &AppState) -> Option<Action> {
        if !matches!(
            app.mode,
            super::GlobalMode::Normal | super::GlobalMode::Chord { .. }
        ) {
            return None;
        }
        match self {
            Self::AppQuit => Some(Action::Quit),
            Self::AppRefresh if !app.repositories.is_empty() => Some(Action::RefreshRepository),
            Self::ViewChangesToggle
                if app.screen == Screen::Changes || app.active_repository_index.is_some() =>
            {
                Some(Action::ToggleChanges)
            }
            Self::FocusNext if has_multiple_panels(app.screen) => Some(Action::FocusNext),
            Self::FocusPrevious if has_multiple_panels(app.screen) => Some(Action::FocusPrev),
            Self::NavigationBack if app.screen != Screen::BranchOverview => Some(Action::Back),
            Self::NavigationUp if can_move(app, -1) => Some(Action::MoveUp),
            Self::NavigationDown if can_move(app, 1) => Some(Action::MoveDown),
            Self::NavigationLeft if can_move_horizontal(app, false) => Some(Action::MoveLeft),
            Self::NavigationRight if can_move_horizontal(app, true) => Some(Action::MoveRight),
            Self::NavigationPageUp if can_move(app, -1) => Some(Action::PageUp),
            Self::NavigationPageDown if can_move(app, 1) => Some(Action::PageDown),
            Self::NavigationHome if can_move(app, -1) => Some(Action::Home),
            Self::NavigationEnd if can_move(app, 1) => Some(Action::End),
            Self::RepositoryActivate
                if app.screen == Screen::BranchOverview
                    && app.focus == FocusPanel::BranchList
                    && app.selected_tree_node().is_some() =>
            {
                Some(Action::LoadCommitsForSelectedBranch)
            }
            Self::RepositoryFetch
                if selected_repository_ready(app).is_some()
                    && app.selected_repository_node_index().is_some() =>
            {
                Some(Action::OpenFetchRepositoryDialog)
            }
            Self::RepositoryPullRebase
                if selected_repository_ready(app).is_some()
                    && app.selected_repository_node_index().is_some() =>
            {
                Some(Action::OpenPullRebaseDialog)
            }
            Self::RepositoryPush
                if selected_repository_ready(app).is_some()
                    && app.selected_repository_node_index().is_some() =>
            {
                Some(Action::OpenPushDialog)
            }
            Self::RepositoryRemotesOpen
                if app.screen == Screen::BranchOverview
                    && app.focus == FocusPanel::BranchList
                    && app.selected_tree_repository_index().is_some() =>
            {
                Some(Action::OpenRemotes)
            }
            Self::RepositoryReflogOpen
                if selected_repository_ready(app).is_some()
                    && app.selected_repository_node_index().is_some() =>
            {
                Some(Action::OpenReflog)
            }
            Self::BranchSwitch
                if app.screen == Screen::BranchOverview
                    && app.focus == FocusPanel::BranchList
                    && app.selected_branch().is_some() =>
            {
                Some(Action::OpenSwitchBranchDialog)
            }
            Self::BranchRebase
                if app.screen == Screen::BranchOverview
                    && app.focus == FocusPanel::BranchList
                    && app.selected_branch().is_some() =>
            {
                Some(Action::OpenRebaseDialog)
            }
            Self::ListFilter if can_filter(app) => Some(Action::StartFilter),
            Self::CommitOpenDetail
                if commit_list_focused(app) && app.selected_commit().is_some() =>
            {
                Some(Action::OpenCommitDetail)
            }
            Self::CommitToggleSelection
                if commit_list_focused(app) && app.selected_commit().is_some() =>
            {
                Some(Action::ToggleCommitCopySelection)
            }
            Self::CommitQueueCherryPick
                if commit_context(app) && app.selected_commit().is_some() =>
            {
                Some(Action::QueueCherryPickSelectedCommit)
            }
            Self::CommitApplyCherryPickQueue
                if commit_list_focused(app) && !app.cherry_pick_queue.is_empty() =>
            {
                Some(Action::OpenCherryPickQueueDialog)
            }
            Self::CommitReset
                if ((commit_list_focused(app) && app.selected_commit().is_some())
                    || (app.screen == Screen::Reflog
                        && app.focus == FocusPanel::ReflogList
                        && app.selected_reflog().is_some())) =>
            {
                Some(Action::OpenResetDialog)
            }
            Self::CommitFileToggleExpanded
                if app.screen == Screen::CommitDetail
                    && app.focus == FocusPanel::CommitFileList
                    && app.selected_file().is_some() =>
            {
                Some(Action::ToggleFileExpanded)
            }
            Self::CommitFileOpenDiff
                if app.screen == Screen::CommitDetail
                    && app.focus == FocusPanel::CommitFileList
                    && app.selected_file().is_some() =>
            {
                Some(Action::OpenSelectedFileDiff)
            }
            Self::FileOpenDiff
                if app.screen == Screen::FileDiffDetail
                    && app.focus == FocusPanel::FileList
                    && app.selected_file().is_some() =>
            {
                Some(Action::OpenSelectedFileDiff)
            }
            Self::FileNext if app.screen == Screen::FileDiffDetail && can_move_file(app, 1) => {
                Some(Action::NextFile)
            }
            Self::FilePrevious
                if app.screen == Screen::FileDiffDetail && can_move_file(app, -1) =>
            {
                Some(Action::PrevFile)
            }
            Self::DiffModeToggle if active_diff(app) => Some(Action::ToggleDiffMode),
            Self::DiffWrapToggle if active_diff(app) => Some(Action::ToggleWrap),
            Self::ChangesActivate
                if app.screen == Screen::Changes
                    && app.focus == FocusPanel::ChangesTree
                    && app.selected_changes_node().is_some() =>
            {
                Some(Action::ActivateSelectedChange)
            }
            Self::ChangesToggleSelection
                if app.screen == Screen::Changes && change_node_has_targets(app) =>
            {
                Some(Action::ToggleChangeSelection)
            }
            Self::ChangesStage if change_operation_paths(app, ChangeGroup::Unstaged).is_empty() => {
                None
            }
            Self::ChangesStage if app.screen == Screen::Changes => {
                Some(Action::StageSelectedChanges)
            }
            Self::ChangesUnstage if change_operation_paths(app, ChangeGroup::Staged).is_empty() => {
                None
            }
            Self::ChangesUnstage if app.screen == Screen::Changes => {
                Some(Action::UnstageSelectedChanges)
            }
            Self::ChangesCommit
                if app.screen == Screen::Changes
                    && app.change_group_count(ChangeGroup::Staged) > 0 =>
            {
                Some(Action::OpenCommitDialog)
            }
            Self::RemoteAdd
                if app.screen == Screen::Remotes
                    && app.focus == FocusPanel::RemoteList
                    && app.remotes_repository_index.is_some() =>
            {
                Some(Action::OpenAddRemoteEditor)
            }
            Self::RemoteSetSharedUrl
                if app.screen == Screen::Remotes
                    && app.focus == FocusPanel::RemoteList
                    && app.selected_remote().is_some() =>
            {
                Some(Action::OpenSetRemoteUrlEditor)
            }
            Self::RemoteSetUpstream
                if app.screen == Screen::Remotes
                    && app.focus == FocusPanel::RemoteList
                    && app.selected_remote().is_some() =>
            {
                Some(Action::OpenSetUpstreamRemoteDialog)
            }
            Self::CommitCopyHash if copy_context(app) => Some(Action::CopySelectedCommitHashes),
            Self::CommitCopyInfo if copy_context(app) => Some(Action::CopyCurrentCommitInfo),
            Self::CommitCopyMessage if copy_context(app) => Some(Action::CopyCurrentCommitMessage),
            _ => None,
        }
    }
}

fn has_multiple_panels(screen: Screen) -> bool {
    matches!(
        screen,
        Screen::BranchOverview | Screen::CommitDetail | Screen::FileDiffDetail | Screen::Changes
    )
}

fn can_filter(app: &AppState) -> bool {
    matches!(
        (app.screen, app.focus),
        (
            Screen::BranchOverview,
            FocusPanel::BranchList | FocusPanel::CommitList
        ) | (Screen::CommitDetail, FocusPanel::CommitList)
    )
}

fn commit_list_focused(app: &AppState) -> bool {
    matches!(
        (app.screen, app.focus),
        (
            Screen::BranchOverview | Screen::CommitDetail,
            FocusPanel::CommitList
        )
    )
}

fn commit_context(app: &AppState) -> bool {
    matches!(
        app.screen,
        Screen::BranchOverview | Screen::CommitDetail | Screen::FileDiffDetail
    )
}

fn copy_context(app: &AppState) -> bool {
    commit_context(app) && app.selected_commit().is_some()
}

fn active_diff(app: &AppState) -> bool {
    match app.screen {
        Screen::FileDiffDetail => app.current_file_diff.is_some(),
        Screen::Changes => app.current_changes_diff.is_some(),
        _ => false,
    }
}

fn selected_repository_ready(app: &AppState) -> Option<usize> {
    let repository_index = app.selected_repository_node_index()?;
    (!has_pending_repository_command(app, repository_index)).then_some(repository_index)
}

fn has_pending_repository_command(app: &AppState, repository_index: usize) -> bool {
    app.pending_jobs.values().any(|pending| {
        matches!(
            pending,
            PendingJobKind::Command {
                repository_index: index,
                ..
            } if *index == repository_index
        )
    })
}

fn can_move_file(app: &AppState, delta: isize) -> bool {
    let length = app
        .current_commit_detail
        .as_ref()
        .map_or(0, |detail| detail.files.len());
    can_move_index(app.selection.selected_file_index, length, delta)
}

fn can_move(app: &AppState, delta: isize) -> bool {
    match app.focus {
        FocusPanel::BranchList => can_move_index(
            app.selection.selected_branch_index,
            app.visible_tree_nodes().len(),
            delta,
        ),
        FocusPanel::CommitList => can_move_index(
            app.selection.selected_commit_index,
            app.visible_commit_indices().len(),
            delta,
        ),
        FocusPanel::CommitFileList | FocusPanel::FileList => can_move_file(app, delta),
        FocusPanel::DiffView => can_scroll(app.selection.diff_scroll, app.diff_line_count(), delta),
        FocusPanel::ReflogList => can_move_index(
            app.selection.selected_reflog_index,
            app.reflog_entries.len(),
            delta,
        ),
        FocusPanel::RemoteList => can_move_index(
            app.selection.selected_remote_index,
            app.remotes.len(),
            delta,
        ),
        FocusPanel::ChangesTree => can_move_index(
            app.selection.selected_changes_index,
            app.visible_changes_nodes().len(),
            delta,
        ),
        FocusPanel::ChangesDiff => can_scroll(
            app.selection.changes_diff_scroll,
            app.changes_diff_line_count(),
            delta,
        ),
        FocusPanel::Popup => false,
    }
}

fn can_move_index(index: Option<usize>, length: usize, delta: isize) -> bool {
    let Some(index) = index else {
        return length > 0;
    };
    if delta < 0 {
        index > 0
    } else {
        index.saturating_add(1) < length
    }
}

fn can_scroll(offset: u16, line_count: usize, delta: isize) -> bool {
    if delta < 0 {
        offset > 0
    } else {
        usize::from(offset).saturating_add(1) < line_count
    }
}

fn can_move_horizontal(app: &AppState, expand: bool) -> bool {
    if app.screen == Screen::Changes && app.focus == FocusPanel::ChangesTree {
        return match app.selected_changes_node() {
            Some(ChangesTreeNode::Root) => app.expansion.changes_root_expanded != expand,
            Some(ChangesTreeNode::Group(ChangeGroup::Staged)) => {
                app.expansion.staged_changes_expanded != expand
            }
            Some(ChangesTreeNode::Group(ChangeGroup::Unstaged)) => {
                app.expansion.unstaged_changes_expanded != expand
            }
            Some(ChangesTreeNode::File { .. }) | None => false,
        };
    }
    has_multiple_panels(app.screen)
}

fn change_node_has_targets(app: &AppState) -> bool {
    match app.selected_changes_node() {
        Some(ChangesTreeNode::Root) => !app.changes.is_empty(),
        Some(ChangesTreeNode::Group(group)) => app.change_group_count(group) > 0,
        Some(ChangesTreeNode::File { .. }) => true,
        None => false,
    }
}

fn change_operation_paths(app: &AppState, group: ChangeGroup) -> Vec<GitPath> {
    if app.screen != Screen::Changes {
        return Vec::new();
    }
    let wanted = if app.change_selection.is_empty() {
        app.selected_change()
            .filter(|(selected_group, _)| *selected_group == group)
            .map(|(_, change)| vec![AppState::change_selection_key(group, change)])
            .unwrap_or_default()
    } else {
        app.change_selection
            .iter()
            .filter(|selection| selection.group == group)
            .cloned()
            .collect()
    };
    let wanted = wanted.into_iter().collect::<HashSet<_>>();
    let mut seen = HashSet::new();
    let mut paths = Vec::new();
    for change in &app.changes {
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

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::CommandId;

    #[test]
    fn registry_lists_every_command_once_and_ids_round_trip() {
        assert_eq!(CommandId::ALL.len(), 46);
        let mut ids = HashSet::new();
        for command in CommandId::ALL.iter().copied() {
            assert!(ids.insert(command.as_str()), "duplicate command id");
            assert_eq!(CommandId::parse(command.as_str()), Some(command));
            assert!(!command.default_bindings().is_empty());
        }
    }
}
