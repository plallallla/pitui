use std::collections::HashSet;

use crate::domain::GitPath;

use super::{
    Action, AppState, ChangeGroup, ChangesTreeNode, ConfirmDialog, FocusPanel, GlobalMode,
    PendingJobKind, RemoteEditKind, Screen,
};

/// A stable, normal-mode operation id. The enum is deliberately `repr(usize)`:
/// each value indexes [`COMMAND_SPECS`], which is the Rust equivalent of a C++
/// function-pointer jump table.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(usize)]
pub enum CommandId {
    AppQuit,
    AppShortcutHelp,
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
    FileCopyName,
    FileCopyAbsolutePath,
    FileCopyRelativePath,
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

/// The exact normal-mode column to which a command table is mounted.
///
/// Screen alone is intentionally insufficient: `CommitList`, `FileList`, and
/// `DiffView` on adjacent hierarchy levels expose different operations even
/// when they display data from the same commit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum ShortcutContext {
    BranchTree,
    OverviewCommits,
    DetailCommits,
    CommitFiles,
    DiffFiles,
    DiffView,
    Reflog,
    ChangesTree,
    ChangesDiff,
    Remotes,
}

impl ShortcutContext {
    pub const ALL: &'static [Self] = &[
        Self::BranchTree,
        Self::OverviewCommits,
        Self::DetailCommits,
        Self::CommitFiles,
        Self::DiffFiles,
        Self::DiffView,
        Self::Reflog,
        Self::ChangesTree,
        Self::ChangesDiff,
        Self::Remotes,
    ];
    pub const ALL_MASK: u16 = (1 << Self::ALL.len()) - 1;

    pub const fn mask(self) -> u16 {
        1 << self as u8
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::BranchTree => "branch.tree",
            Self::OverviewCommits => "overview.commits",
            Self::DetailCommits => "detail.commits",
            Self::CommitFiles => "commit.files",
            Self::DiffFiles => "diff.files",
            Self::DiffView => "diff.view",
            Self::Reflog => "reflog.list",
            Self::ChangesTree => "changes.tree",
            Self::ChangesDiff => "changes.diff",
            Self::Remotes => "remotes.list",
        }
    }

    pub const fn title(self) -> &'static str {
        match self {
            Self::BranchTree => "Branches · repository / branch column",
            Self::OverviewCommits => "Commits · overview right column",
            Self::DetailCommits => "Commits · detail left column",
            Self::CommitFiles => "Commit · changed-files column",
            Self::DiffFiles => "Commit · file column beside diff",
            Self::DiffView => "Diff · file content column",
            Self::Reflog => "Reflog · entries column",
            Self::ChangesTree => "Changes · staged / unstaged tree",
            Self::ChangesDiff => "Changes · diff column",
            Self::Remotes => "Remote Management · remote list",
        }
    }

    pub fn from_view(screen: Screen, focus: FocusPanel) -> Option<Self> {
        match (screen, focus) {
            (Screen::BranchOverview, FocusPanel::BranchList) => Some(Self::BranchTree),
            (Screen::BranchOverview, FocusPanel::CommitList) => Some(Self::OverviewCommits),
            (Screen::CommitDetail, FocusPanel::CommitList) => Some(Self::DetailCommits),
            (Screen::CommitDetail, FocusPanel::CommitFileList) => Some(Self::CommitFiles),
            (Screen::FileDiffDetail, FocusPanel::FileList) => Some(Self::DiffFiles),
            (Screen::FileDiffDetail, FocusPanel::DiffView) => Some(Self::DiffView),
            (Screen::Reflog, FocusPanel::ReflogList) => Some(Self::Reflog),
            (Screen::Changes, FocusPanel::ChangesTree) => Some(Self::ChangesTree),
            (Screen::Changes, FocusPanel::ChangesDiff) => Some(Self::ChangesDiff),
            (Screen::Remotes, FocusPanel::RemoteList) => Some(Self::Remotes),
            _ => None,
        }
    }

    pub fn from_app(app: &AppState) -> Option<Self> {
        Self::from_view(app.screen, app.focus)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandMount {
    Global,
    Focus,
}

pub type CommandHandler = fn(&AppState) -> Option<Action>;

/// Metadata and callable behavior for one command. Input resolution, footer
/// hints, configuration validation, and the help popup all consume this same
/// table; none of them carries a second screen/focus key map.
#[derive(Clone, Copy)]
pub struct CommandSpec {
    pub id: CommandId,
    pub name: &'static str,
    pub default_bindings: &'static [&'static str],
    pub default_label: &'static str,
    pub default_visible: bool,
    pub footer_group: FooterGroup,
    pub chord_group: Option<&'static str>,
    pub mount: CommandMount,
    pub contexts: u16,
    pub invoke: CommandHandler,
}

macro_rules! command_spec {
    (
        $id:ident, $name:literal, $bindings:expr, $label:literal,
        $visible:expr, $group:ident, $chord:expr, $mount:ident,
        $contexts:expr, $invoke:expr
    ) => {
        CommandSpec {
            id: CommandId::$id,
            name: $name,
            default_bindings: $bindings,
            default_label: $label,
            default_visible: $visible,
            footer_group: FooterGroup::$group,
            chord_group: $chord,
            mount: CommandMount::$mount,
            contexts: $contexts,
            invoke: $invoke,
        }
    };
}

const BRANCH_TREE: u16 = ShortcutContext::BranchTree.mask();
const BRANCH_COMMITS: u16 = ShortcutContext::OverviewCommits.mask();
const DETAIL_COMMITS: u16 = ShortcutContext::DetailCommits.mask();
const DETAIL_FILES: u16 = ShortcutContext::CommitFiles.mask();
const FILE_LIST: u16 = ShortcutContext::DiffFiles.mask();
const FILE_DIFF: u16 = ShortcutContext::DiffView.mask();
const REFLOG: u16 = ShortcutContext::Reflog.mask();
const CHANGES_TREE: u16 = ShortcutContext::ChangesTree.mask();
const CHANGES_DIFF: u16 = ShortcutContext::ChangesDiff.mask();
const REMOTES: u16 = ShortcutContext::Remotes.mask();
const ALL_CONTEXTS: u16 = ShortcutContext::ALL_MASK;
const TWO_PANEL_CONTEXTS: u16 = BRANCH_TREE
    | BRANCH_COMMITS
    | DETAIL_COMMITS
    | DETAIL_FILES
    | FILE_LIST
    | FILE_DIFF
    | CHANGES_TREE
    | CHANGES_DIFF;

/// The single normal-mode command jump table. Its order must match
/// [`CommandId`]; a unit test enforces that invariant.
pub static COMMAND_SPECS: &[CommandSpec] = &[
    command_spec!(
        AppQuit,
        "app.quit",
        &["q", "Ctrl+C"],
        "quit",
        true,
        Safety,
        None,
        Global,
        ALL_CONTEXTS,
        |_| Some(Action::Quit)
    ),
    command_spec!(
        AppShortcutHelp,
        "app.shortcuts",
        &["Ctrl+?", "?"],
        "shortcuts",
        true,
        Global,
        None,
        Global,
        ALL_CONTEXTS,
        |_| Some(Action::OpenShortcutHelp)
    ),
    command_spec!(
        AppRefresh,
        "app.refresh",
        &["Ctrl+R"],
        "refresh",
        true,
        Global,
        None,
        Global,
        ALL_CONTEXTS,
        |app| (!app.repositories.is_empty()).then_some(Action::RefreshRepository)
    ),
    command_spec!(
        ViewChangesToggle,
        "view.changes.toggle",
        &["Ctrl+G"],
        "changes",
        true,
        Global,
        None,
        Global,
        ALL_CONTEXTS,
        |app| (app.screen == Screen::Changes || app.active_repository_index.is_some())
            .then_some(Action::ToggleChanges)
    ),
    command_spec!(
        FocusNext,
        "focus.next",
        &["Tab"],
        "focus",
        true,
        Navigation,
        None,
        Focus,
        TWO_PANEL_CONTEXTS,
        |app| has_multiple_panels(app.screen).then_some(Action::FocusNext)
    ),
    command_spec!(
        FocusPrevious,
        "focus.previous",
        &["BackTab"],
        "focus",
        false,
        Navigation,
        None,
        Focus,
        TWO_PANEL_CONTEXTS,
        |app| has_multiple_panels(app.screen).then_some(Action::FocusPrev)
    ),
    command_spec!(
        NavigationBack,
        "navigation.back",
        &["Esc"],
        "back",
        true,
        Safety,
        None,
        Focus,
        ALL_CONTEXTS & !BRANCH_TREE & !BRANCH_COMMITS,
        |app| (app.screen != Screen::BranchOverview).then_some(Action::Back)
    ),
    command_spec!(
        NavigationUp,
        "navigation.up",
        &["Up", "k"],
        "up",
        false,
        Navigation,
        None,
        Focus,
        ALL_CONTEXTS,
        |app| can_move(app, -1).then_some(Action::MoveUp)
    ),
    command_spec!(
        NavigationDown,
        "navigation.down",
        &["Down", "j"],
        "down",
        false,
        Navigation,
        None,
        Focus,
        ALL_CONTEXTS,
        |app| can_move(app, 1).then_some(Action::MoveDown)
    ),
    command_spec!(
        NavigationLeft,
        "navigation.left",
        &["Left", "h"],
        "left",
        false,
        Navigation,
        None,
        Focus,
        TWO_PANEL_CONTEXTS,
        |app| can_move_horizontal(app, false).then_some(Action::MoveLeft)
    ),
    command_spec!(
        NavigationRight,
        "navigation.right",
        &["Right", "l"],
        "right",
        false,
        Navigation,
        None,
        Focus,
        TWO_PANEL_CONTEXTS,
        |app| can_move_horizontal(app, true).then_some(Action::MoveRight)
    ),
    command_spec!(
        NavigationPageUp,
        "navigation.page_up",
        &["PageUp"],
        "page up",
        true,
        Navigation,
        None,
        Focus,
        ALL_CONTEXTS,
        |app| can_move(app, -1).then_some(Action::PageUp)
    ),
    command_spec!(
        NavigationPageDown,
        "navigation.page_down",
        &["PageDown"],
        "page down",
        true,
        Navigation,
        None,
        Focus,
        ALL_CONTEXTS,
        |app| can_move(app, 1).then_some(Action::PageDown)
    ),
    command_spec!(
        NavigationHome,
        "navigation.home",
        &["Home"],
        "first/top",
        true,
        Navigation,
        None,
        Focus,
        ALL_CONTEXTS,
        |app| can_move(app, -1).then_some(Action::Home)
    ),
    command_spec!(
        NavigationEnd,
        "navigation.end",
        &["End"],
        "last/bottom",
        true,
        Navigation,
        None,
        Focus,
        ALL_CONTEXTS,
        |app| can_move(app, 1).then_some(Action::End)
    ),
    command_spec!(
        RepositoryActivate,
        "repository.activate",
        &["Enter"],
        "open",
        true,
        Primary,
        None,
        Focus,
        BRANCH_TREE,
        |app| (app.selected_tree_node().is_some()).then_some(Action::LoadCommitsForSelectedBranch)
    ),
    command_spec!(
        RepositoryFetch,
        "repository.fetch",
        &["f"],
        "fetch",
        true,
        Primary,
        None,
        Focus,
        BRANCH_TREE,
        |app| (selected_repository_ready(app).is_some()
            && app.selected_repository_node_index().is_some())
        .then_some(Action::OpenFetchRepositoryDialog)
    ),
    command_spec!(
        RepositoryPullRebase,
        "repository.pull_rebase",
        &["p"],
        "pull --rebase",
        true,
        Primary,
        None,
        Focus,
        BRANCH_TREE,
        |app| (selected_repository_ready(app).is_some()
            && app.selected_repository_node_index().is_some())
        .then_some(Action::OpenPullRebaseDialog)
    ),
    command_spec!(
        RepositoryPush,
        "repository.push",
        &["P"],
        "push",
        true,
        Primary,
        None,
        Focus,
        BRANCH_TREE,
        |app| (selected_repository_ready(app).is_some()
            && app.selected_repository_node_index().is_some())
        .then_some(Action::OpenPushDialog)
    ),
    command_spec!(
        RepositoryRemotesOpen,
        "repository.remotes.open",
        &["o"],
        "remotes",
        true,
        Primary,
        None,
        Focus,
        BRANCH_TREE,
        |app| (app.selected_tree_repository_index().is_some()).then_some(Action::OpenRemotes)
    ),
    command_spec!(
        RepositoryReflogOpen,
        "repository.reflog.open",
        &["g"],
        "reflog",
        true,
        Primary,
        None,
        Focus,
        BRANCH_TREE,
        |app| (selected_repository_ready(app).is_some()
            && app.selected_repository_node_index().is_some())
        .then_some(Action::OpenReflog)
    ),
    command_spec!(
        BranchSwitch,
        "branch.switch",
        &["s"],
        "switch",
        true,
        Primary,
        None,
        Focus,
        BRANCH_TREE,
        |app| app
            .selected_branch()
            .is_some()
            .then_some(Action::OpenSwitchBranchDialog)
    ),
    command_spec!(
        BranchRebase,
        "branch.rebase",
        &["b"],
        "rebase",
        true,
        Primary,
        None,
        Focus,
        BRANCH_TREE,
        |app| app
            .selected_branch()
            .is_some()
            .then_some(Action::OpenRebaseDialog)
    ),
    command_spec!(
        ListFilter,
        "list.filter",
        &["/"],
        "filter",
        true,
        Contextual,
        None,
        Focus,
        BRANCH_TREE | BRANCH_COMMITS | DETAIL_COMMITS,
        |app| can_filter(app).then_some(Action::StartFilter)
    ),
    command_spec!(
        CommitOpenDetail,
        "commit.open_detail",
        &["Enter"],
        "detail",
        true,
        Primary,
        None,
        Focus,
        BRANCH_COMMITS | DETAIL_COMMITS,
        |app| (app.selected_commit().is_some()).then_some(Action::OpenCommitDetail)
    ),
    command_spec!(
        CommitToggleSelection,
        "commit.toggle_selection",
        &["Space"],
        "select",
        true,
        Primary,
        None,
        Focus,
        BRANCH_COMMITS | DETAIL_COMMITS,
        |app| (app.selected_commit().is_some()).then_some(Action::ToggleCommitCopySelection)
    ),
    command_spec!(
        CommitQueueCherryPick,
        "commit.cherry_pick.queue",
        &["y"],
        "queue",
        true,
        Primary,
        None,
        Focus,
        BRANCH_COMMITS | DETAIL_COMMITS,
        |app| (app.selected_commit().is_some()).then_some(Action::QueueCherryPickSelectedCommit)
    ),
    command_spec!(
        CommitApplyCherryPickQueue,
        "commit.cherry_pick.apply_queue",
        &["Y"],
        "cherry-pick",
        true,
        Primary,
        None,
        Focus,
        BRANCH_COMMITS | DETAIL_COMMITS,
        |app| (!app.cherry_pick_queue.is_empty()).then_some(Action::OpenCherryPickQueueDialog)
    ),
    command_spec!(
        CommitReset,
        "commit.reset",
        &["R"],
        "reset",
        true,
        Primary,
        None,
        Focus,
        BRANCH_COMMITS | DETAIL_COMMITS | REFLOG,
        |app| {
            match app.screen {
                Screen::Reflog => app.selected_reflog().is_some(),
                _ => app.selected_commit().is_some(),
            }
            .then_some(Action::OpenResetDialog)
        }
    ),
    command_spec!(
        CommitFileToggleExpanded,
        "commit.file.toggle_expanded",
        &["Space"],
        "expand",
        true,
        Primary,
        None,
        Focus,
        DETAIL_FILES,
        |app| (app.selected_file().is_some()).then_some(Action::ToggleFileExpanded)
    ),
    command_spec!(
        CommitFileOpenDiff,
        "commit.file.open_diff",
        &["Enter", "v"],
        "file diff",
        true,
        Primary,
        None,
        Focus,
        DETAIL_FILES,
        |app| (app.selected_file().is_some()).then_some(Action::OpenSelectedFileDiff)
    ),
    command_spec!(
        FileOpenDiff,
        "file.open_diff",
        &["Enter"],
        "file diff",
        true,
        Primary,
        None,
        Focus,
        FILE_LIST,
        |app| (app.selected_file().is_some()).then_some(Action::OpenSelectedFileDiff)
    ),
    command_spec!(
        FileNext,
        "file.next",
        &["n"],
        "next",
        true,
        Navigation,
        None,
        Focus,
        FILE_LIST | FILE_DIFF,
        |app| can_move_file(app, 1).then_some(Action::NextFile)
    ),
    command_spec!(
        FilePrevious,
        "file.previous",
        &["p"],
        "previous",
        true,
        Navigation,
        None,
        Focus,
        FILE_LIST | FILE_DIFF,
        |app| can_move_file(app, -1).then_some(Action::PrevFile)
    ),
    command_spec!(
        DiffModeToggle,
        "diff.mode.toggle",
        &["v"],
        "mode",
        true,
        Contextual,
        None,
        Focus,
        FILE_LIST | FILE_DIFF | CHANGES_TREE | CHANGES_DIFF,
        |app| active_diff(app).then_some(Action::ToggleDiffMode)
    ),
    command_spec!(
        DiffWrapToggle,
        "diff.wrap.toggle",
        &["w"],
        "wrap",
        true,
        Contextual,
        None,
        Focus,
        FILE_LIST | FILE_DIFF | CHANGES_TREE | CHANGES_DIFF,
        |app| active_diff(app).then_some(Action::ToggleWrap)
    ),
    command_spec!(
        ChangesActivate,
        "changes.activate",
        &["Enter"],
        "open/toggle",
        true,
        Primary,
        None,
        Focus,
        CHANGES_TREE,
        |app| (app.selected_changes_node().is_some()).then_some(Action::ActivateSelectedChange)
    ),
    command_spec!(
        ChangesToggleSelection,
        "changes.toggle_selection",
        &["Space"],
        "select",
        true,
        Primary,
        None,
        Focus,
        CHANGES_TREE | CHANGES_DIFF,
        |app| change_node_has_targets(app).then_some(Action::ToggleChangeSelection)
    ),
    command_spec!(
        ChangesStage,
        "changes.stage",
        &["s"],
        "stage",
        true,
        Primary,
        None,
        Focus,
        CHANGES_TREE | CHANGES_DIFF,
        |app| (!change_operation_paths(app, ChangeGroup::Unstaged).is_empty())
            .then_some(Action::StageSelectedChanges)
    ),
    command_spec!(
        ChangesUnstage,
        "changes.unstage",
        &["u"],
        "unstage",
        true,
        Primary,
        None,
        Focus,
        CHANGES_TREE | CHANGES_DIFF,
        |app| (!change_operation_paths(app, ChangeGroup::Staged).is_empty())
            .then_some(Action::UnstageSelectedChanges)
    ),
    command_spec!(
        ChangesCommit,
        "changes.commit",
        &["c"],
        "commit",
        true,
        Primary,
        None,
        Focus,
        CHANGES_TREE | CHANGES_DIFF,
        |app| (app.change_group_count(ChangeGroup::Staged) > 0).then_some(Action::OpenCommitDialog)
    ),
    command_spec!(
        RemoteAdd,
        "remote.add",
        &["a"],
        "add remote",
        true,
        Primary,
        None,
        Focus,
        REMOTES,
        |app| app
            .remotes_repository_index
            .is_some()
            .then_some(Action::OpenAddRemoteEditor)
    ),
    command_spec!(
        RemoteSetSharedUrl,
        "remote.set_shared_url",
        &["e"],
        "set shared URL",
        true,
        Primary,
        None,
        Focus,
        REMOTES,
        |app| app
            .selected_remote()
            .is_some()
            .then_some(Action::OpenSetRemoteUrlEditor)
    ),
    command_spec!(
        RemoteSetUpstream,
        "remote.set_upstream",
        &["u"],
        "set upstream",
        true,
        Primary,
        None,
        Focus,
        REMOTES,
        |app| app
            .selected_remote()
            .is_some()
            .then_some(Action::OpenSetUpstreamRemoteDialog)
    ),
    command_spec!(
        CommitCopyHash,
        "commit.copy.hash",
        &["Ctrl+C h"],
        "hash",
        true,
        Primary,
        Some("commit.copy"),
        Focus,
        BRANCH_COMMITS | DETAIL_COMMITS,
        |app| (app.selected_commit().is_some()).then_some(Action::CopySelectedCommitHashes)
    ),
    command_spec!(
        CommitCopyInfo,
        "commit.copy.info",
        &["Ctrl+C i"],
        "info",
        true,
        Primary,
        Some("commit.copy"),
        Focus,
        BRANCH_COMMITS | DETAIL_COMMITS,
        |app| (app.selected_commit().is_some()).then_some(Action::CopyCurrentCommitInfo)
    ),
    command_spec!(
        CommitCopyMessage,
        "commit.copy.message",
        &["Ctrl+C m"],
        "message",
        true,
        Primary,
        Some("commit.copy"),
        Focus,
        BRANCH_COMMITS | DETAIL_COMMITS,
        |app| (app.selected_commit().is_some()).then_some(Action::CopyCurrentCommitMessage)
    ),
    command_spec!(
        FileCopyName,
        "file.copy.name",
        &["Ctrl+C n"],
        "file name",
        true,
        Primary,
        Some("file.copy"),
        Focus,
        DETAIL_FILES | FILE_LIST,
        |app| (app.selected_file().is_some()).then_some(Action::CopySelectedFileName)
    ),
    command_spec!(
        FileCopyAbsolutePath,
        "file.copy.absolute_path",
        &["Ctrl+C a"],
        "absolute path",
        true,
        Primary,
        Some("file.copy"),
        Focus,
        DETAIL_FILES | FILE_LIST,
        |app| (app.selected_file().is_some()).then_some(Action::CopySelectedFileAbsolutePath)
    ),
    command_spec!(
        FileCopyRelativePath,
        "file.copy.relative_path",
        &["Ctrl+C r"],
        "relative path",
        true,
        Primary,
        Some("file.copy"),
        Focus,
        DETAIL_FILES | FILE_LIST,
        |app| (app.selected_file().is_some()).then_some(Action::CopySelectedFileRelativePath)
    ),
];

impl CommandId {
    pub const ALL: &'static [Self] = &[
        Self::AppQuit,
        Self::AppShortcutHelp,
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
        Self::FileCopyName,
        Self::FileCopyAbsolutePath,
        Self::FileCopyRelativePath,
    ];

    pub fn parse(value: &str) -> Option<Self> {
        COMMAND_SPECS
            .iter()
            .find(|spec| spec.name == value)
            .map(|spec| spec.id)
    }

    pub fn as_str(self) -> &'static str {
        self.spec().name
    }

    pub fn default_bindings(self) -> &'static [&'static str] {
        self.spec().default_bindings
    }

    pub fn default_label(self) -> &'static str {
        self.spec().default_label
    }

    pub fn default_visible(self) -> bool {
        self.spec().default_visible
    }

    pub fn footer_group(self) -> FooterGroup {
        self.spec().footer_group
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
        self.spec().chord_group
    }

    pub fn mount(self) -> CommandMount {
        self.spec().mount
    }

    /// Exact focus tables used while validating configured key collisions.
    /// Runtime actionability applies the finer selection/pending predicates.
    pub fn context_mask(self) -> u16 {
        self.spec().contexts
    }

    pub fn action(self, app: &AppState) -> Option<Action> {
        if !matches!(
            app.mode,
            super::GlobalMode::Normal | super::GlobalMode::Chord { .. }
        ) {
            return None;
        }
        let spec = self.spec();
        if spec.mount == CommandMount::Focus {
            let context = ShortcutContext::from_app(app)?;
            if spec.contexts & context.mask() == 0 {
                return None;
            }
        }
        (spec.invoke)(app)
    }

    pub fn spec(self) -> &'static CommandSpec {
        &COMMAND_SPECS[self as usize]
    }
}

/// A non-configurable operation table used while a modal editor or safety
/// dialog owns keyboard input. These tables are also rendered in the global
/// shortcut reference, so commit submission and remote editing cannot drift
/// away from their documented operation sets.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(usize)]
pub enum ModalShortcutSetId {
    Filter,
    Confirmation,
    ResetMode,
    TypedConfirmation,
    CommitMessage,
    RemoteAdd,
    RemoteUrl,
    Error,
    ShortcutHelp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModalShortcutHint {
    pub key: &'static str,
    pub label: &'static str,
    pub operation: &'static str,
}

#[derive(Clone, Copy, Debug)]
pub struct ModalShortcutSet {
    pub id: ModalShortcutSetId,
    pub title: &'static str,
    pub footer: &'static str,
    pub hints: &'static [ModalShortcutHint],
}

impl ModalShortcutSetId {
    pub const ALL: &'static [Self] = &[
        Self::Filter,
        Self::Confirmation,
        Self::ResetMode,
        Self::TypedConfirmation,
        Self::CommitMessage,
        Self::RemoteAdd,
        Self::RemoteUrl,
        Self::Error,
        Self::ShortcutHelp,
    ];

    pub fn spec(self) -> &'static ModalShortcutSet {
        &MODAL_SHORTCUT_SETS[self as usize]
    }
}

macro_rules! modal_hint {
    ($key:literal, $label:literal, $operation:literal) => {
        ModalShortcutHint {
            key: $key,
            label: $label,
            operation: $operation,
        }
    };
}

pub static MODAL_SHORTCUT_SETS: &[ModalShortcutSet] = &[
    ModalShortcutSet {
        id: ModalShortcutSetId::Filter,
        title: "Mode · filter editor",
        footer: "Text input | Backspace delete | Enter apply | Esc cancel",
        hints: &[
            modal_hint!("text", "append query", "UpdateFilter"),
            modal_hint!("Backspace", "delete character", "UpdateFilter"),
            modal_hint!("Enter", "apply filter", "SubmitFilter"),
            modal_hint!("Esc", "cancel filter", "CancelFilter"),
        ],
    },
    ModalShortcutSet {
        id: ModalShortcutSetId::Confirmation,
        title: "Mode · safety confirmation",
        footer: "Enter confirm | Esc / q cancel",
        hints: &[
            modal_hint!("Enter", "confirm operation", "Confirm"),
            modal_hint!("Esc / q", "cancel operation", "Cancel"),
        ],
    },
    ModalShortcutSet {
        id: ModalShortcutSetId::ResetMode,
        title: "Mode · reset mode selection",
        footer: "s soft | m mixed | h hard | Esc / q cancel",
        hints: &[
            modal_hint!("s", "choose soft reset", "ChooseResetSoft"),
            modal_hint!("m", "choose mixed reset", "ChooseResetMixed"),
            modal_hint!("h", "choose hard reset", "ChooseResetHard"),
            modal_hint!("Esc / q", "cancel reset", "Cancel"),
        ],
    },
    ModalShortcutSet {
        id: ModalShortcutSetId::TypedConfirmation,
        title: "Mode · hard-reset hash confirmation",
        footer: "Text input | Backspace delete | Enter final confirm | Esc cancel",
        hints: &[
            modal_hint!("text", "append short hash", "UpdateTypedConfirmation"),
            modal_hint!("Backspace", "delete character", "UpdateTypedConfirmation"),
            modal_hint!("Enter", "confirm exact hash", "ConfirmReset"),
            modal_hint!("Esc", "cancel reset", "Cancel"),
        ],
    },
    ModalShortcutSet {
        id: ModalShortcutSetId::CommitMessage,
        title: "Mode · commit submission",
        footer: "Text input | Backspace delete | Enter create commit | Esc cancel",
        hints: &[
            modal_hint!("text", "append commit message", "UpdateCommitMessage"),
            modal_hint!("Backspace", "delete character", "UpdateCommitMessage"),
            modal_hint!("Enter", "create commit", "SubmitCommit"),
            modal_hint!("Esc", "cancel commit", "Cancel"),
        ],
    },
    ModalShortcutSet {
        id: ModalShortcutSetId::RemoteAdd,
        title: "Mode · add remote editor",
        footer: "Text input | Backspace delete | Tab switch field | Enter continue | Esc cancel",
        hints: &[
            modal_hint!("text", "edit active field", "UpdateRemoteName/Url"),
            modal_hint!("Backspace", "delete character", "UpdateRemoteName/Url"),
            modal_hint!("Tab / BackTab", "switch field", "FocusNextRemoteField"),
            modal_hint!("Enter", "validate / continue", "SubmitRemoteEditor"),
            modal_hint!("Esc", "cancel remote edit", "Cancel"),
        ],
    },
    ModalShortcutSet {
        id: ModalShortcutSetId::RemoteUrl,
        title: "Mode · shared remote URL editor",
        footer: "Text input | Backspace delete | Enter continue | Esc cancel",
        hints: &[
            modal_hint!("text", "edit shared URL", "UpdateRemoteUrl"),
            modal_hint!("Backspace", "delete character", "UpdateRemoteUrl"),
            modal_hint!("Enter", "validate / continue", "SubmitRemoteEditor"),
            modal_hint!("Esc", "cancel remote edit", "Cancel"),
        ],
    },
    ModalShortcutSet {
        id: ModalShortcutSetId::Error,
        title: "Mode · command error",
        footer: "Enter / Esc / q dismiss",
        hints: &[modal_hint!(
            "Enter / Esc / q",
            "dismiss error",
            "DismissError"
        )],
    },
    ModalShortcutSet {
        id: ModalShortcutSetId::ShortcutHelp,
        title: "Mode · shortcut reference",
        footer: "↑/↓ scroll | PageUp/PageDown page | Home/End jump | Esc close",
        hints: &[
            modal_hint!("Up / k", "scroll up", "MoveUp"),
            modal_hint!("Down / j", "scroll down", "MoveDown"),
            modal_hint!("PageUp / PageDown", "scroll page", "PageUp/PageDown"),
            modal_hint!("Home / End", "jump start/end", "Home/End"),
            modal_hint!("Ctrl+? / ? / Enter / Esc / q", "close reference", "Cancel"),
        ],
    },
];

pub fn modal_shortcut_set_id(mode: &GlobalMode) -> Option<ModalShortcutSetId> {
    match mode {
        GlobalMode::Filtering { .. } => Some(ModalShortcutSetId::Filter),
        GlobalMode::Confirming {
            dialog: ConfirmDialog::ResetModeChoice { .. },
        } => Some(ModalShortcutSetId::ResetMode),
        GlobalMode::Confirming { .. } => Some(ModalShortcutSetId::Confirmation),
        GlobalMode::TypingConfirmation { .. } => Some(ModalShortcutSetId::TypedConfirmation),
        GlobalMode::EditingCommitMessage { .. } => Some(ModalShortcutSetId::CommitMessage),
        GlobalMode::EditingRemote {
            kind: RemoteEditKind::Add,
            ..
        } => Some(ModalShortcutSetId::RemoteAdd),
        GlobalMode::EditingRemote { .. } => Some(ModalShortcutSetId::RemoteUrl),
        GlobalMode::Error => Some(ModalShortcutSetId::Error),
        GlobalMode::ShortcutHelp { .. } => Some(ModalShortcutSetId::ShortcutHelp),
        GlobalMode::Normal | GlobalMode::Chord { .. } => None,
    }
}

pub fn modal_shortcut_set(mode: &GlobalMode) -> Option<&'static ModalShortcutSet> {
    modal_shortcut_set_id(mode).map(ModalShortcutSetId::spec)
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

fn can_move_horizontal(app: &AppState, right: bool) -> bool {
    if app.screen == Screen::Changes && app.focus == FocusPanel::ChangesTree {
        return match app.selected_changes_node() {
            Some(ChangesTreeNode::Root) => app.expansion.changes_root_expanded != right,
            Some(ChangesTreeNode::Group(ChangeGroup::Staged)) => {
                app.expansion.staged_changes_expanded != right
            }
            Some(ChangesTreeNode::Group(ChangeGroup::Unstaged)) => {
                app.expansion.unstaged_changes_expanded != right
            }
            Some(ChangesTreeNode::File { .. }) | None => false,
        };
    }

    match (app.screen, app.focus, right) {
        // Branches | Commits
        (Screen::BranchOverview, FocusPanel::BranchList, true) => true,
        (Screen::BranchOverview, FocusPanel::CommitList, false) => true,
        (Screen::BranchOverview, FocusPanel::CommitList, true) => app.selected_commit().is_some(),

        // Commits | Commit (metadata + files). Crossing either outer edge
        // slides the two-column hierarchy instead of wrapping focus.
        (Screen::CommitDetail, FocusPanel::CommitList, false) => true,
        (Screen::CommitDetail, FocusPanel::CommitList, true) => true,
        (Screen::CommitDetail, FocusPanel::CommitFileList, false) => true,
        (Screen::CommitDetail, FocusPanel::CommitFileList, true) => app.selected_file().is_some(),

        // Commit (metadata + files) | Diff
        (Screen::FileDiffDetail, FocusPanel::FileList, false) => true,
        (Screen::FileDiffDetail, FocusPanel::FileList, true) => true,
        (Screen::FileDiffDetail, FocusPanel::DiffView, false) => true,

        // Changes keeps its existing tree expand/collapse semantics; only a
        // focused diff can move back to the tree with Left.
        (Screen::Changes, FocusPanel::ChangesDiff, false) => true,
        _ => false,
    }
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

    use super::{COMMAND_SPECS, CommandId, MODAL_SHORTCUT_SETS, ModalShortcutSetId};

    #[test]
    fn registry_lists_every_command_once_and_ids_round_trip() {
        assert_eq!(CommandId::ALL.len(), 50);
        assert_eq!(COMMAND_SPECS.len(), CommandId::ALL.len());
        let mut ids = HashSet::new();
        for (index, command) in CommandId::ALL.iter().copied().enumerate() {
            assert_eq!(COMMAND_SPECS[index].id, command, "jump-table order drift");
            assert!(ids.insert(command.as_str()), "duplicate command id");
            assert_eq!(CommandId::parse(command.as_str()), Some(command));
            assert!(!command.default_bindings().is_empty());
        }
    }

    #[test]
    fn modal_operation_sets_are_index_aligned() {
        assert_eq!(MODAL_SHORTCUT_SETS.len(), ModalShortcutSetId::ALL.len());
        for (index, id) in ModalShortcutSetId::ALL.iter().copied().enumerate() {
            assert_eq!(MODAL_SHORTCUT_SETS[index].id, id);
            assert!(!id.spec().hints.is_empty());
        }
    }
}
