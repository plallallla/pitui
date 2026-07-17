# Pitui 多仓库六视图 Git TUI 设计文档

> 目标：实现一个安全、克制、以分支 / commit / diff 浏览为核心的 Git TUI。
>
> 技术栈：Rust + ratatui + crossterm + git CLI。
>
> 核心约束：UI 不直接执行 Git；Git 命令不直接改 UI；所有 Git 输出先解析为 Domain Model，再由 Renderer 投影为界面。

---

## 1. 产品范围

Pitui 第一阶段聚焦于以下工作流：

```text
选择仓库
  ↓
选择该仓库中的分支
  ↓
浏览该分支 commits，但不切换真实分支
  ↓
查看 commit 修改文件
  ↓
查看单文件 diff
  ↓
必要时安全执行 pull --rebase / push / switch / cherry-pick / reset / rebase
```

六视图结构：

```text
View 1: Branch / Commit Overview
  左侧 Repositories / Branches 树
  右侧 Commits

View 2: Commit Detail
  左侧 Commits
  右侧完整 Commit 栏（上方 metadata，下方 Files changed in commit）

View 3: File Diff Detail
  左侧完整复用 View 2 的 Commit 栏（metadata + files）
  右侧 Diff Detail

View 4: Reflog
  左侧 Reflog Entries
  右侧 Selected Entry

View 5: Changes
  左侧 Changes -> Staged/Unstaged -> File 三级树
  右侧复用 File Diff 的 unified / side-by-side 组件

View 6: Remote Management
  左侧 Remotes（upstream / fetch / push / policy 标记）
  右侧 Fetch URL(s) / Push URL(s) / 当前分支路由

任意主视图可通过 Ctrl+G 进入 Changes；退出时恢复进入前的 screen 与 focus。
```

---

## 2. 分层架构

```text
Presentation / TUI
  - 渲染 ratatui widgets
  - 读取键盘事件
  - 将事件映射为 Action
  - 不调用 git

Application
  - 处理 Action
  - 维护 AppState / Store
  - 调度 GitRequest
  - 应用 GitResponse

Domain
  - Repository / Branch / Commit / Diff 等纯模型
  - 不依赖 ratatui
  - 不依赖 git CLI

Infrastructure
  - 执行 git CLI
  - 解析 git 输出
  - 持久化记录 Git worker job 生命周期
  - 产出 Domain DTO / GitResponse
```

主数据流：

```text
Keyboard Event
  -> Action
  -> AppController
  -> GitCommandBus
  -> Git Worker
       +-> Backend JSONL Log
  -> Parser
  -> GitResponse
  -> Store / AppState
  -> ViewModel
  -> Renderer
```

---

## 3. 全局 UI 状态模型

### 3.1 Screen

```rust
pub enum Screen {
    BranchOverview,
    CommitDetail,
    FileDiffDetail,
    Reflog,
    Changes,
    Remotes,
}
```

### 3.2 FocusPanel

```rust
pub enum FocusPanel {
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
```

### 3.3 DiffViewMode

```rust
pub enum DiffViewMode {
    Unified,
    SideBySide,
}
```

### 3.4 GlobalMode

全局交互模式用于区分普通浏览、过滤输入、确认弹窗等状态。

```rust
pub enum GlobalMode {
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
    CommandPrompt {
        input: String,
        validation_error: Option<String>,
    },
    Error,
}
```

`Loading` 不作为独占模式；加载状态统一由 `pending_jobs` 派生，避免异步读取期间阻断浏览和返回操作。

### 3.5 AppState

```rust
pub struct AppState {
    pub config: Arc<ResolvedConfig>,
    pub repositories: Vec<RepositoryState>,
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
    pub change_selection: HashSet<ChangeSelection>,
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

    pub commit_selection: HashSet<CommitHash>,
    pub commit_selection_repository_index: Option<usize>,
    pub pending_clipboard: Option<String>,
    pub pending_jobs: HashMap<GitJobId, PendingJobKind>,
    pub latest_commits_job: Option<GitJobId>,
    pub latest_commit_detail_job: Option<GitJobId>,
    pub latest_commit_message_job: Option<GitJobId>,
    pub latest_file_diff_job: Option<GitJobId>,
    pub last_error: Option<AppError>,
}

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
```

`PendingJobKind` 对每类请求都保存 `repository_index`。列表、commit detail 和 file diff 仅应用对应仓库、对应上下文的最新 job response，防止跨仓库或快速切换时旧响应覆盖当前状态。commit 多选集合也绑定单一仓库与当前 viewing branch；切换上下文时清空，不允许将不同仓库的 commits 混入同一次写操作。

### 3.6 SelectionState

```rust
pub struct SelectionState {
    // 扁平化后的可见“仓库 / 分支”树行索引
    pub selected_branch_index: Option<usize>,
    pub selected_commit_index: Option<usize>,
    pub selected_file_index: Option<usize>,
    pub selected_reflog_index: Option<usize>,
    // 扁平化后的 Changes -> Group -> File 三级树行索引
    pub selected_changes_index: Option<usize>,
    pub diff_scroll: u16,
    pub changes_diff_scroll: u16,
    pub file_scroll: u16,
    pub commit_scroll: u16,
    pub branch_scroll: u16,
}
```

树节点不保存引用，使用稳定的仓库/分支来源索引描述：

```rust
pub enum BranchTreeNode {
    Repository { repository_index: usize },
    Branch { repository_index: usize, branch_index: usize },
}
```

### 3.7 ExpansionState

```rust
pub struct ExpansionState {
    pub expanded_files: HashSet<GitPath>,
    pub changes_root_expanded: bool,
    pub staged_changes_expanded: bool,
    pub unstaged_changes_expanded: bool,
}
```

---

## 4. 全局 Action 定义

Action 是 Presentation 与 Application 之间的唯一输入协议。

```rust
pub enum Action {
    Tick,
    Quit,

    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
    PageUp,
    PageDown,
    Home,
    End,

    FocusNext,
    FocusPrev,
    Back,
    Confirm,
    Cancel,

    RefreshRepository,

    StartFilter,
    UpdateFilter(String),
    SubmitFilter,
    CancelFilter,

    SelectBranch(usize),
    LoadCommitsForSelectedBranch,
    OpenFetchRepositoryDialog,
    OpenPullRebaseDialog,
    OpenPushDialog,
    OpenRemotes,
    OpenAddRemoteEditor,
    OpenSetRemoteUrlEditor,
    OpenSetUpstreamRemoteDialog,
    UpdateRemoteName(String),
    UpdateRemoteUrl(String),
    FocusNextRemoteField,
    SubmitRemoteEditor,
    OpenReflog,
    ToggleChanges,
    ActivateSelectedChange,
    ToggleChangeSelection,
    StageSelectedChanges,
    UnstageSelectedChanges,
    OpenCommitDialog,
    UpdateCommitMessage(String),
    SubmitCommit,
    OpenSwitchBranchDialog,
    OpenRebaseDialog,

    SelectCommit(usize),
    OpenCommitDetail,

    ToggleFileExpanded,
    OpenSelectedFileDiff,

    ToggleDiffMode,
    NextFile,
    PrevFile,
    ToggleWrap,

    ToggleCommitSelection,
    BeginChord(Vec<KeyStroke>),
    OpenShortcutHelp,
    OpenCommandPrompt,
    UpdateCommandPrompt(String),
    SubmitCommandPrompt,
    CopySelectedCommitHashes,
    CopyCurrentCommitInfo,
    CopyCurrentCommitMessage,
    CopySelectedFileName,
    CopySelectedFileAbsolutePath,
    CopySelectedFileRelativePath,

    OpenCherryPickSelectedDialog,
    OpenResetDialog,
    UpdateTypedConfirmation(String),
    ConfirmReset,

    DismissError,
}
```

---

## 5. 全局状态转移

### 5.1 Screen 状态机

```text
BranchOverview
  Enter on CommitList
    -> CommitDetail
  Right on focused CommitList
    -> CommitDetail，Commits 右栏平移为左栏并保持 focus

CommitDetail
  Enter on CommitFileList
    -> FileDiffDetail
  Right on focused CommitFileList
    -> FileDiffDetail，完整 Commit 右栏平移为左栏并保持 focus
  Left on focused CommitList
    -> BranchOverview，Commits 左栏还原为右栏并保持 focus
  Esc / Back
    -> BranchOverview

FileDiffDetail
  Left on focused FileList
    -> CommitDetail，完整 Commit 左栏还原为右栏并保持 focus
  Esc / Back
    -> CommitDetail

BranchOverview + BranchList + o
  -> Remotes

Remotes
  Esc / Back
    -> BranchOverview + BranchList

任意主视图
  Ctrl+G
    -> Changes（保存原 screen + focus）

Changes
  Ctrl+G / Esc / Back
    -> 恢复原 screen + focus

任意主视图 + Normal
  Ctrl+R
    -> 手动刷新全部 repository status / branches / active commits
    -> Reflog、Changes 或 Remotes 时同时刷新当前视图数据

任意主视图 + 任意 Normal focus
  h
    -> ShortcutHelp(scroll=0) + focus=Popup
    -> 保存 previous_focus；关闭后恢复原 screen/focus

任意主视图 + 任意 Normal focus
  Ctrl+Backtick
    -> CommandPrompt(input="", validation_error=None) + focus=Popup
    -> 输入 help 并按 Enter 后打开 ShortcutHelp
```

### 5.2 Focus 状态机

```text
BranchOverview:
  BranchList --Tab--> CommitList
  CommitList --Tab--> BranchList

CommitDetail:
  CommitList --Tab--> CommitFileList
  CommitFileList --Tab--> CommitList

FileDiffDetail:
  FileList --Tab--> DiffView
  DiffView --Tab--> FileList

Changes:
  ChangesTree --Tab--> ChangesDiff
  ChangesDiff --Tab--> ChangesTree

Remotes:
  RemoteList（右侧是所选 remote 的只读详情）
```

左右方向键与 Tab 的语义刻意分离：Tab/Shift-Tab 只在当前 screen 的两个 panel 间循环；
Left/Right 沿以下层级单向移动，绝不从最右端 wrap 回最左端：

```text
Branches | Commits
  Right on Commits
    -> Commits | Commit(metadata + files)
  Right on Commit
    -> Commit(metadata + files) | Diff

反向 Left 完全对称：跨 screen 时当前左栏成为上一级右栏，并保持同一逻辑 column 的 focus。
```

### 5.3 Modal 状态机

```text
Normal
  dangerous action
    -> Confirming / TypingConfirmation

Confirming
  Enter
    -> Normal + submit GitRequest
  Esc
    -> Normal

TypingConfirmation
  typed input == expected && Enter
    -> Normal + submit GitRequest
  Esc
    -> Normal
  other text input
    -> TypingConfirmation(input updated)

Normal + h
  -> ShortcutHelp(scroll=0)

ShortcutHelp
  navigation keys
    -> ShortcutHelp(scroll updated)
  h / Enter / Esc / q
    -> Normal + restore previous_focus

Normal + Ctrl+Backtick
  -> CommandPrompt(input="", validation_error=None)

CommandPrompt
  Char / Backspace
    -> CommandPrompt(input updated, validation_error cleared)
  Enter + input == help
    -> ShortcutHelp(scroll=0)
  Enter + empty/unknown input
    -> CommandPrompt(validation_error set)
  Esc
    -> Normal + restore previous_focus
```

### 5.4 Loading 状态

Loading 不是独占 screen，而是由 `pending_jobs` 表达：

```text
Action triggers GitRequest
  -> pending_jobs += job_id
  -> UI 显示 loading indicator

GitResponse received
  -> pending_jobs -= job_id
  -> Store 更新模型
  -> Renderer 下一 tick 刷新
```

---

## 6. View 1: Branch / Commit Overview

### 6.1 视图目的

View 1 用于：

1. 查看多个仓库，以及各仓库的本地和远程分支。
2. 在不切换真实分支的情况下，浏览任意分支 commits。
3. 从 commit 列表进入 commit detail。
4. 安全执行 fetch、pull --rebase、push、switch 和 rebase。
5. 从仓库节点进入该仓库 reflog。
6. 从仓库或分支节点进入 Remote Management。
7. 通过全局 Changes 查看当前 working tree 状态和 diff。

### 6.2 布局

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ repo=pitui | branch=main | op=NORMAL | S=0 M=1 U=0 C=0 | ↑0 ↓0          │
├──────────────────────────────┬─────────────────────────────────────────────┤
│ Repositories / Branches      │ Commits                                     │
│ ▼ ● pitui /repo/pitui        │ > 3cc6d76 Fix Debug Layer error             │
│   ├─ * main                  │     Date: 2026-07-16 13:20 Author: Ada Tags: v1.2 │
│   └─   feature/x             │   0acaf2e Fix color grading format          │
│ ▶ ○ backend /repo/backend    │     Date: 2026-07-15 09:05 Author: Lin      │
├──────────────────────────────┴─────────────────────────────────────────────┤
│ Ctrl+R refresh | Tab focus | Enter view | S switch | / filter | q quit  │
└────────────────────────────────────────────────────────────────────────────┘
```

View 1 的 Commits 位于宽右栏，使用两行 detailed item：summary 行显示统一多选标记、
short hash 与 subject；metadata 行显示 ISO 日期到分钟（`YYYY-MM-DD HH:MM`）、author，
并在存在 Git tag 时追加从 `%D` decorations 中提取的一个或多个 `tag:`。无 tag 时整个
`Tags:` 字段都不渲染。当同一 Commits 组件在
View 2 成为窄左栏时切换为原有单行 compact item，保持纵向密度与既有 decorations 展示。

### 6.3 FocusPanel

View 1 允许两个 focus：

```text
BranchList
CommitList
```

默认：

```text
screen = BranchOverview
focus = BranchList
```

### 6.4 BranchList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| w / ↑ / k | MoveUp | tree 非空 | 扁平化 tree selection 上移；选中分支时自动加载右侧 commits，焦点保持在 BranchList；跨仓库时更新 active repository |
| s / ↓ / j | MoveDown | tree 非空 | 扁平化 tree selection 下移；选中分支时自动加载右侧 commits，焦点保持在 BranchList；跨仓库时更新 active repository |
| PageUp | PageUp | tree 非空 | 树向上翻页，只为最终选中的分支加载右侧 commits |
| PageDown | PageDown | tree 非空 | 树向下翻页，只为最终选中的分支加载右侧 commits |
| Home | Home | tree 非空 | 选择第一行；若为分支则同步右侧 commits |
| End | End | tree 非空 | 选择最后一行；若为分支则同步右侧 commits |
| d / → / l | MoveRight | 总是 | focus -> CommitList；不改变 screen |
| Enter | LoadCommitsForSelectedBranch | 已选择仓库 | 展开/折叠仓库节点 |
| Enter | LoadCommitsForSelectedBranch | 已选择分支 | 加载该分支 commits，不切换真实分支 |
| f | OpenFetchRepositoryDialog | 已选择仓库 | 确认执行 `git fetch --all --prune` |
| p | OpenPullRebaseDialog | 已选择仓库 | 确认执行 `git pull --rebase` |
| P | OpenPushDialog | 已选择仓库 | 确认执行 `git push` |
| g | OpenReflog | 已选择仓库 | 加载该仓库最近 300 条 reflog |
| o | OpenRemotes | 已选择仓库或分支 | 打开所属仓库的 Remote Management |
| S | OpenSwitchBranchDialog | 已选择分支 | 进入切分支确认弹窗 |
| b | OpenRebaseDialog | 已选择分支 | 将当前分支安全 rebase 到所选分支 |
| / | StartFilter | 总是 | 按仓库名、路径、分支或 subject 过滤树 |
| Ctrl+R | RefreshRepository | 任意主界面的 Normal mode | 手动刷新全部 repo status / branch list / 当前 viewing commits |
| Tab | FocusNext | 总是 | focus -> CommitList |
| q | Quit | 总是 | 退出应用 |

### 6.5 CommitList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| w / ↑ / k | MoveUp | commits 非空 | selected_commit_index 上移 |
| s / ↓ / j | MoveDown | commits 非空 | selected_commit_index 下移 |
| PageUp | PageUp | commits 非空 | commit 列表向上翻页 |
| PageDown | PageDown | commits 非空 | commit 列表向下翻页 |
| Home | Home | commits 非空 | 选择第一个 commit |
| End | End | commits 非空 | 选择最后一个 commit |
| a / ← | MoveLeft | 总是 | focus -> BranchList；不改变 screen；`h` 保留给帮助 |
| d / → / l | MoveRight | 已选择 commit | 平移为 `Commits | Commit`；Commits 成为左栏并保持 focus，异步加载右栏 detail |
| Enter | OpenCommitDetail | 已选择 commit | 加载 commit detail，然后进入 View 2 |
| Space | ToggleCommitSelection | 已选择 commit | 加入/移出统一 commit 多选集合 |
| Ctrl+C | BeginChord([Ctrl+C]) | 已选择 commit | 进入当前有效 keymap 的复制 chord，不改变 focus |
| Ctrl+C → h | CopySelectedCommitHashes | commits 非空 | 按列表顺序复制多选完整 hashes；集合为空则复制当前 hash |
| Ctrl+C → i | CopyCurrentCommitInfo | 已选择 commit | 复制 hash、author、date、refs 与 message |
| Ctrl+C → m | CopyCurrentCommitMessage | 已选择 commit | 复制完整 message；缺少 detail 时后台加载，不切换 screen/focus |
| y | OpenCherryPickSelectedDialog | 多选集合非空 | 按历史顺序打开所选 commits 的 cherry-pick 确认弹窗 |
| R | OpenResetDialog | 已选择 commit | 打开 reset typed confirmation 弹窗 |
| / | StartFilter | commits 非空 | 进入 commit search/filter 模式 |
| Ctrl+R | RefreshRepository | 任意主界面的 Normal mode | 手动刷新 repo status / branch list / commits |
| Tab | FocusNext | 总是 | focus -> BranchList |
| Esc | Back | 总是 | 若无上层视图，则保持当前视图 |
| q | Quit | 总是 | 退出应用 |

`commit_selection` 是复制 hashes 与 cherry-pick 共用的显式多选集合；切换仓库或 viewing branch 时清空，过滤列表不会改变集合。复制 hashes 在集合为空时仍可复制当前 commit，但 cherry-pick 严格要求集合非空，并按日志中的 oldest-to-newest 顺序提交。剪贴板由 TUI 层通过 OSC 52 写入，不引入平台特定 clipboard 命令。message copy 必须返回完整 subject/body；若 `CommitDetail` 尚未缓存，则发送独立 `LoadCommitMessage`，响应只写 clipboard，不得导航到 Commit Detail 或改变当前 focus。

`Ctrl+C` 是可配置的二级快捷键 prefix。进入 `GlobalMode::Chord` 后，底部只显示当前 prefix
直接可接受的 `h hash | i info | m message | Esc cancel`；`q` 或再次按 `Ctrl+C` 也取消。
通用 resolver 同样支持三段 chord，且每次只揭示下一段。普通模式不再长期占用 `c/i/m`
三个单键，chord 完成、超时或取消后恢复 `Normal`，focus 始终不变。

### 6.6 View 1 状态转移

#### 6.6.1 选择分支

```text
BranchOverview + BranchList + MoveUp/MoveDown
  -> selected_branch_index changed
  -> active_repository_index = selected node repository
  -> 选中 branch node 时立即清空不匹配的旧 commits/detail/diff
  -> GitRequest::LoadCommits { selected branch, limit: 300 }
  -> response 更新右侧 Commits，focus 保持 BranchList
  -> 快速连续移动时只接受最新选择对应的 job response
  -> 跨仓库时加载新仓库上下文
```

#### 6.6.2 加载分支 commits

```text
BranchOverview + BranchList + Enter
  -> Action::LoadCommitsForSelectedBranch
  -> GitRequest::LoadCommits { branch, limit: 300 }, routed by repository_index
  -> pending_jobs += job_id

GitResponse::CommitsLoaded { branch, commits }
  -> branch_commits.viewing_branch = branch
  -> branch_commits.items = commits
  -> selected_commit_index = first item if exists
  -> focus = CommitList
```

#### 6.6.3 进入 Commit Detail

```text
BranchOverview + CommitList + Enter
  -> Action::OpenCommitDetail
  -> GitRequest::LoadCommitDetail { commit }
  -> pending_jobs += job_id

GitResponse::CommitDetailLoaded(detail)
  -> current_commit_detail = Some(detail)
  -> screen = CommitDetail
  -> focus = CommitFileList
  -> selected_file_index = first file if exists
```

Right 使用 column shift intent，而不是 Enter 的直接打开 intent：

```text
BranchOverview + CommitList + Right
  -> screen = CommitDetail（立即）
  -> Commits 从右栏复用为左栏
  -> focus = CommitList
  -> 缺少匹配缓存时异步 LoadCommitDetail
  -> response 只更新右侧 Commit，不改变 focus
  -> 若 response 到达前已 Left 返回，则该 response 作为 stale 丢弃
```

#### 6.6.4 切换分支

```text
BranchOverview + BranchList + s
  -> mode = Confirming(SwitchBranchDialog)
  -> focus = Popup

Confirming + Enter
  -> GitRequest::SwitchBranch { branch }
  -> mode = Normal
  -> focus = BranchList
  -> pending_jobs += job_id

GitResponse::CommandSucceeded
  -> RefreshRepository
  -> current_branch updated

Confirming + Esc
  -> mode = Normal
  -> focus = previous_focus
```

---

## 7. View 2: Commit Detail

### 7.1 视图目的

View 2 用于：

1. 以单行紧凑样式保留左侧 commit 列表上下文。
2. 查看当前 commit 的元信息。
3. 查看当前 commit 修改了哪些文件。
4. 展开文件以查看 hunk summary。
5. 进入单文件 diff 详情。

### 7.2 布局

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ repo=pitui | branch=main | commit=3cc6d76 | op=NORMAL | S=0 M=0 U=0 C=0 │
├──────────────────────────────┬─────────────────────────────────────────────┤
│ Commits                      │ Commit                                      │
│ > 3cc6d76 Fix Debug Layer    │ Commit: 3cc6d76                             │
│   0acaf2e Fix color grading  │ Author: xxx                                 │
│   ab244d6 Add texture        │ Date: 2026-07-16 13:20:45                   │
│                              │ Message: Fix Debug Layer error              │
│                              ├─────────────────────────────────────────────┤
│                              │ Files changed in commit                     │
│                              │ > ▶ M shaders/Collision.hlsl        +20 -5  │
│                              │   ▶ A src/git/parser.rs            +130 -0  │
│                              │   ▶ D old/status.rs                 +0 -80  │
├──────────────────────────────┴─────────────────────────────────────────────┤
│ Tab focus | Space expand | Enter diff | Ctrl+C copy file… | Esc back      │
└────────────────────────────────────────────────────────────────────────────┘
```

### 7.3 FocusPanel

View 2 允许两个 focus：

```text
CommitList
CommitFileList
```

通过 Enter 直接打开 detail 时：

```text
screen = CommitDetail
focus = CommitFileList
```

通过 Commits 右栏再次按 Right 平移进入时：

```text
screen = CommitDetail
focus = CommitList
```

### 7.4 CommitList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| w / ↑ / k | MoveUp | commits 非空 | 选择上一个 commit，自动刷新右侧 detail，focus 保持 CommitList |
| s / ↓ / j | MoveDown | commits 非空 | 选择下一个 commit，自动刷新右侧 detail，focus 保持 CommitList |
| PageUp | PageUp | commits 非空 | commit 列表向上翻页，只加载最终选中 commit detail |
| PageDown | PageDown | commits 非空 | commit 列表向下翻页，只加载最终选中 commit detail |
| a / ← | MoveLeft | 总是 | 平移回 `Branches | Commits`；Commits 成为右栏并保持 focus |
| d / → / l | MoveRight | 总是 | focus -> CommitFileList；不改变 screen |
| Enter | OpenCommitDetail | 已选择 commit | 重新加载所选 commit detail |
| Space | ToggleCommitSelection | 已选择 commit | 加入/移出统一 commit 多选集合 |
| Ctrl+C → h | CopySelectedCommitHashes | commits 非空 | 复制多选完整 hashes；无多选时复制当前 hash |
| Ctrl+C → i | CopyCurrentCommitInfo | 已选择 commit | 复制当前 commit info |
| Ctrl+C → m | CopyCurrentCommitMessage | 已选择 commit | 复制完整 message，必要时后台加载且不改变 focus |
| y | OpenCherryPickSelectedDialog | 多选集合非空 | 按历史顺序打开所选 commits 的 cherry-pick 确认弹窗 |
| R | OpenResetDialog | 已选择 commit | 打开 reset typed confirmation 弹窗 |
| Tab | FocusNext | 总是 | focus -> CommitFileList |
| Esc | Back | 总是 | screen -> BranchOverview |
| q | Quit | 总是 | 退出应用 |

### 7.5 CommitFileList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| w / ↑ / k | MoveUp | files 非空 | selected_file_index 上移 |
| s / ↓ / j | MoveDown | files 非空 | selected_file_index 下移 |
| PageUp | PageUp | files 非空 | 文件树向上翻页 |
| PageDown | PageDown | files 非空 | 文件树向下翻页 |
| Home | Home | files 非空 | 选择第一个文件 |
| End | End | files 非空 | 选择最后一个文件 |
| a / ← | MoveLeft | 总是 | focus -> CommitList；不改变 screen |
| d / → / l | MoveRight | 已选择文件 | 平移为 `Commit | Diff`；完整 Commit 栏成为左栏并保持 focus，异步加载右栏 diff |
| Space | ToggleFileExpanded | 已选择文件 | 展开 / 折叠 hunk summary |
| Enter | OpenSelectedFileDiff | 已选择文件 | 加载文件 diff，然后进入 View 3 |
| v | OpenSelectedFileDiff | 已选择文件 | 等价于打开文件 diff |
| Ctrl+C → n | CopySelectedFileName | 已选择文件 | 复制当前文件 basename |
| Ctrl+C → a | CopySelectedFileAbsolutePath | 已选择文件 | 复制 repository root 与 GitPath 组合后的绝对路径 |
| Ctrl+C → r | CopySelectedFileRelativePath | 已选择文件 | 复制 Git 返回的仓库相对路径 |
| Tab | FocusNext | 总是 | focus -> CommitList |
| Esc | Back | 总是 | screen -> BranchOverview |
| q | Quit | 总是 | 退出应用 |

### 7.6 View 2 状态转移

#### 7.6.1 切换 commit detail

```text
CommitDetail + CommitList + MoveUp/MoveDown
  -> selected_commit_index changed
  -> 清空不匹配的旧 current_commit_detail/current_file_diff
  -> GitRequest::LoadCommitDetail { selected commit }
  -> pending_jobs += job_id

GitResponse::CommitDetailLoaded(detail)
  -> 仅当 job 仍对应最新选择时更新 current_commit_detail
  -> selected_file_index = first file if exists
  -> expansion.expanded_files.clear()
  -> focus 保持 CommitList
```

```text
CommitDetail + CommitList + Enter
  -> GitRequest::LoadCommitDetail { commit }
  -> pending_jobs += job_id

GitResponse::CommitDetailLoaded(detail)
  -> current_commit_detail = Some(detail)
  -> selected_file_index = first file if exists
  -> expansion.expanded_files.clear()
  -> focus = CommitFileList
```

#### 7.6.2 展开文件 hunk summary

```text
CommitDetail + CommitFileList + Space
  -> if selected file path in expanded_files:
       remove path
     else:
       insert path
  -> screen unchanged
  -> focus unchanged
```

#### 7.6.3 进入 File Diff Detail

```text
CommitDetail + CommitFileList + Enter
  -> GitRequest::LoadFileDiff { commit, path }
  -> pending_jobs += job_id

GitResponse::FileDiffLoaded(diff)
  -> current_file_diff = Some(diff)
  -> screen = FileDiffDetail
  -> focus = DiffView
  -> diff_scroll = 0
```

Right 使用 column shift intent：

```text
CommitDetail + CommitFileList + Right
  -> screen = FileDiffDetail（立即）
  -> 完整 Commit 栏从右侧复用到左侧
  -> focus = FileList
  -> GitRequest::LoadFileDiff { commit, path }
  -> response 只更新右侧 Diff，不改变 focus
  -> 若 response 到达前已 Left 返回，则该 response 作为 stale 丢弃
```

#### 7.6.4 返回 Branch Overview

```text
CommitDetail + Esc
  -> screen = BranchOverview
  -> focus = CommitList
  -> current_commit_detail preserved
```

---

## 8. View 3: File Diff Detail

### 8.1 视图目的

View 3 用于：

1. 保留当前 commit 的 metadata 与文件列表上下文。
2. 查看选中文件的完整 diff。
3. 在 unified 和 side-by-side 两种模式之间切换。
4. 在当前 commit 的多个文件之间快速切换。

### 8.2 布局

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ repo=pitui | branch=main | commit=3cc6d76 | file=a.rs | op=NORMAL       │
├──────────────────────────────┬─────────────────────────────────────────────┤
│ Commit                       │ Changes                                     │
│ Commit: 3cc6d76              │ @@ -70,10 +70,18 @@                         │
│ Author: xxx                  │   const bool isAlive = ...                  │
│ Message: Fix Debug Layer     │ - query.signed_distance = 0.0f;             │
├──────────────────────────────┤ + query.uses_cached_contact = false;        │
│ Files changed in commit      │ + query.signed_distance = 0.0f;             │
│ > ▶ M CollisionStage.hlsl    │                                             │
│   ▶ A parser.rs              │                                             │
│   ▶ D old_status.rs          │                                             │
├──────────────────────────────┴─────────────────────────────────────────────┤
│ v mode | n next file | p prev file | W wrap | Tab focus | Esc back        │
└────────────────────────────────────────────────────────────────────────────┘
```

### 8.3 FocusPanel

View 3 允许两个 focus：

```text
FileList
DiffView
```

通过 Enter 直接打开 diff 时：

```text
screen = FileDiffDetail
focus = DiffView
```

通过完整 Commit 右栏再次按 Right 平移进入时：

```text
screen = FileDiffDetail
focus = FileList
```

### 8.4 FileList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| w / ↑ / k | MoveUp | files 非空 | 选择上一个文件并加载 diff，focus 保持 FileList |
| s / ↓ / j | MoveDown | files 非空 | 选择下一个文件并加载 diff，focus 保持 FileList |
| PageUp | PageUp | files 非空 | 上翻 10 个文件，只为最终文件提交一次 diff 请求 |
| PageDown | PageDown | files 非空 | 下翻 10 个文件，只为最终文件提交一次 diff 请求 |
| Home | Home | files 非空 | 选择第一个文件并加载 diff，focus 保持 FileList |
| End | End | files 非空 | 选择最后一个文件并加载 diff，focus 保持 FileList |
| a / ← | MoveLeft | 总是 | 平移回 `Commits | Commit`；完整 Commit 栏成为右栏并保持 focus |
| d / → / l | MoveRight | 总是 | focus -> DiffView；不改变 screen |
| n | NextFile | files 非空 | 选择下一个文件，并加载该文件 diff |
| p | PrevFile | files 非空 | 选择上一个文件，并加载该文件 diff |
| Enter | OpenSelectedFileDiff | 已选择文件 | 加载该文件 diff |
| v | ToggleDiffMode | 总是 | unified / side-by-side 切换 |
| W | ToggleWrap | 总是 | 开启 / 关闭换行 |
| Ctrl+C → n | CopySelectedFileName | 已选择文件 | 复制文件 basename |
| Ctrl+C → a | CopySelectedFileAbsolutePath | 已选择文件 | 复制绝对路径 |
| Ctrl+C → r | CopySelectedFileRelativePath | 已选择文件 | 复制仓库相对路径 |
| Tab | FocusNext | 总是 | focus -> DiffView |
| Esc | Back | 总是 | screen -> CommitDetail |
| q | Quit | 总是 | 退出应用 |

### 8.5 DiffView 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| w / ↑ / k | MoveUp | diff 已加载 | diff_scroll 上移 |
| s / ↓ / j | MoveDown | diff 已加载 | diff_scroll 下移 |
| PageUp | PageUp | diff 已加载 | diff 向上翻页 |
| PageDown | PageDown | diff 已加载 | diff 向下翻页 |
| Home | Home | diff 已加载 | 滚动到 diff 顶部 |
| End | End | diff 已加载 | 滚动到 diff 底部 |
| a / ← | MoveLeft | 总是 | focus -> FileList；不改变 screen |
| d / → / l | MoveRight | 最深层 | 无动作，不 wrap 回 FileList |
| n | NextFile | files 非空 | 选择下一个文件，并加载 diff |
| p | PrevFile | files 非空 | 选择上一个文件，并加载 diff |
| v | ToggleDiffMode | 总是 | unified / side-by-side 切换 |
| W | ToggleWrap | 总是 | 开启 / 关闭换行 |
| Tab | FocusNext | 总是 | focus -> FileList |
| Esc | Back | 总是 | screen -> CommitDetail |
| Ctrl+C / q | Quit | 总是 | 此 focus 不挂载 commit/file copy table，执行全局退出 |

`CommitFileList` 与 `FileList` 共享 `file.copy` 二级表，但 `DiffView` 不共享；同一份 commit
数据仍严格按当前 focus 选择操作表。反之 `CommitList` 只挂载 `commit.copy`，不会出现文件
路径命令。

### 8.6 View 3 状态转移

#### 8.6.1 切换 diff 模式

```text
FileDiffDetail + v
  -> if diff_mode == Unified:
       diff_mode = SideBySide
     else:
       diff_mode = Unified

Renderer rule:
  if diff_mode == SideBySide && terminal_width < 140:
       render Unified and show note: "side-by-side requires width >= 140"
```

#### 8.6.2 切换文件

```text
FileDiffDetail + n / p / MoveUp / MoveDown / PageUp / PageDown / Home / End in FileList
  -> selected_file_index changed
  -> GitRequest::LoadFileDiff { commit, path }
  -> pending_jobs += job_id
  -> diff_scroll = 0

GitResponse::FileDiffLoaded(diff)
  -> current_file_diff = Some(diff)
  -> focus unchanged
```

`PendingJobKind::FileDiff` 携带 `focus_diff` intent：只有显式 `Enter/OpenSelectedFileDiff` 的响应可以把 focus 设为 `DiffView`；由 column shift 或 `↑/↓/PageUp/PageDown/Home/End/n/p` 触发的 diff refresh 必须保留响应到达时的 focus，异步 response 不得抢走左侧 Commit/Files 栏焦点。非聚焦请求只允许在 `FileDiffDetail` 接受；用户已 Left 返回时 response 必须作为 stale 丢弃。PageUp/PageDown 直接计算最终 selection，不为中间 9 个文件排队无用 job。

#### 8.6.3 滚动 diff

```text
FileDiffDetail + DiffView + MoveUp/MoveDown/PageUp/PageDown
  -> diff_scroll updated
  -> no GitRequest
  -> screen unchanged
```

#### 8.6.4 返回 Commit Detail

```text
FileDiffDetail + Esc
  -> screen = CommitDetail
  -> focus = CommitFileList
  -> current_file_diff preserved
```

---

## 9. 弹窗与危险操作状态转移

### 9.1 Switch Branch

#### 触发

```text
BranchOverview + BranchList + s
```

#### 弹窗内容

```text
About to run:
git switch <branch>

Working tree:
staged=<n> modified=<n> untracked=<n> conflicted=<n>

Enter confirm | Esc cancel
```

#### 状态转移

```text
Normal
  --s-->
Confirming(SwitchBranch)
  --Enter-->
Normal + GitRequest::SwitchBranch
  --Esc-->
Normal
```

#### GitResponse 处理

```text
CommandSucceeded
  -> RefreshRepository
  -> current_branch updated
  -> selected branch remains visible

CommandFailed
  -> last_error = AppError
  -> mode = Error
```

### 9.2 Cherry-pick Selected Commits

#### 触发

```text
Space: toggle current commit in the shared selection
y: open cherry-pick confirmation for the selection
```

#### 状态转移

```text
Normal + y + commit_selection non-empty
  -> collect selected commits oldest-to-newest
  -> Confirming(CherryPickSelected)

Normal + y + commit_selection empty
  -> no action; command is absent from the current footer/help action set
```

#### 弹窗内容

```text
About to run:
git cherry-pick <commit1> <commit2> ...

Selected commits (oldest to newest):
1. 3cc6d76 Fix Debug Layer error
2. 0acaf2e Fix color grading format

Enter confirm | Esc cancel
```

#### 确认状态转移

```text
Confirming(CherryPickSelected) + Enter
  -> GitRequest::CherryPick { commits }
  -> mode = Normal
  -> pending_jobs += job_id

GitResponse::CommandSucceeded
  -> commit_selection.clear()
  -> RefreshRepository
```

### 9.3 Reset

Reset 可以从 CommitList 或 Reflog 选择目标，并明确区分三种模式。hard 是最高风险操作，采用两阶段确认。

#### 触发

```text
CommitList + R
Reflog + R
```

#### 弹窗内容

```text
Choose reset mode:
s --soft   # 保留 index 与 working tree
m --mixed  # 重置 index，保留 working tree
h --hard   # 丢弃 tracked index / working tree changes
```

#### 状态转移

```text
Normal + R
  -> Confirming(ResetModeChoice)

ResetModeChoice + s/m
  -> Confirming(Reset { mode: Soft | Mixed })
  -> Enter 后提交 GitRequest::Reset

ResetModeChoice + h
  -> Confirming(HardResetWarning)       # confirmation 1/2

HardResetWarning + Enter
  -> TypingConfirmation(expected=short_hash) # confirmation 2/2

TypingConfirmation + Enter + input == expected
  -> GitRequest::Reset { commit, mode: Hard }
  -> mode = Normal

TypingConfirmation + Enter + input != expected
  -> stay TypingConfirmation
  -> show validation error

TypingConfirmation + Esc
  -> mode = Normal
```

---

## 10. Input Mapping 规则

Input Mapper 只根据当前 `screen + focus + mode` 产生 Action，不修改状态。

### 10.1 Normal 模式

```rust
type CommandHandler = fn(&AppState) -> Option<Action>;

struct CommandSpec {
    id: CommandId,
    mount: CommandMount,  // Global | Focus
    contexts: u16,
    invoke: CommandHandler,
    // stable id / default bindings / footer metadata omitted here
}

fn resolve_command(command: CommandId, app: &AppState) -> Option<Action> {
    let spec = &COMMAND_SPECS[command as usize];
    if spec.mount == CommandMount::Focus {
        let context = ShortcutContext::from_view(app.screen, app.focus)?;
        if spec.contexts & context.mask() == 0 {
            return None;
        }
    }
    (spec.invoke)(app)
}
```

`ShortcutContext` 的十张 focus 表与 §6.4、§6.5、§7.4、§7.5、§8.4、§8.5、Reflog、
ChangesTree、ChangesDiff、Remotes 操作集一一对应。`ResolvedKeymap::resolve` 和 footer 都先
遍历同一 `COMMAND_SPECS`，再调用同一函数指针；Renderer 不维护另一份按键字符串。

关键挂载约束：

```text
OverviewCommits / DetailCommits -> commit.copy.hash/info/message
CommitFiles / DiffFiles         -> file.copy.name/absolute_path/relative_path
DiffView                        -> no copy table; Ctrl+C falls back to app.quit
```

`app.shortcuts` 是 Global command，默认 `h`，打开当前-focus operation-set 参考框。
`navigation.up/left/down/right` 默认首选 `w/a/s/d`，并保留箭头与 `k/j/l` 替代绑定；
chord 中的 `Ctrl+C → h` 仍由第二级表解析。

`app.command_prompt` 是另一个 Global command，只绑定 Ctrl+Backtick；`Ctrl+Space` 不绑定
任何操作。WASD 占用后，branch switch、Changes stage、diff wrap、remote add 分别使用
大写 `S/S/W/A`。

### 10.2 Filtering 模式

Normal/Chord 之外的输入通过 `MODE_KEY_TABLES` 跳表选择可调用 handler；表 id 同时索引
`MODAL_SHORTCUT_SETS`，因此下面的输入、footer 和全局帮助框共用同一操作说明。

```text
Char(c)    -> UpdateFilter(query + c)
Backspace  -> UpdateFilter(query.pop())
Enter      -> SubmitFilter
Esc        -> CancelFilter
```

### 10.3 Confirming 模式

```text
Enter -> Confirm
Esc   -> Cancel
q     -> Cancel
```

### 10.4 TypingConfirmation 模式

```text
Char(c)    -> UpdateTypedConfirmation(input + c)
Backspace  -> UpdateTypedConfirmation(input.pop())
Enter      -> Confirm
Esc        -> Cancel
```

### 10.5 Commit / Remote 编辑模式

```text
EditingCommitMessage:
  Char / Backspace -> UpdateCommitMessage
  Enter            -> SubmitCommit
  Esc              -> Cancel

EditingRemote(Add):
  Char / Backspace -> UpdateRemoteName or UpdateRemoteUrl for active field
  Tab / BackTab    -> FocusNextRemoteField
  Enter            -> SubmitRemoteEditor
  Esc              -> Cancel

EditingRemote(SetUrl):
  Char / Backspace -> UpdateRemoteUrl
  Enter            -> SubmitRemoteEditor
  Esc              -> Cancel
```

### 10.6 CommandPrompt 模式

```text
Ctrl+Backtick from any Normal focus
  -> open_popup(); previous_focus = current focus
  -> GlobalMode::CommandPrompt { input: "", validation_error: None }

Char / Backspace -> UpdateCommandPrompt
Enter            -> SubmitCommandPrompt
Esc              -> close_popup(); restore previous_focus
```

可接受命令由 `PROMPT_COMMAND_SPECS` 可调用表维护，表项包含 name、description、operation id
和 `fn() -> Action`。当前 `help` 返回 `OpenShortcutHelp`；Controller 先恢复来源 focus，再按普通
命令路径打开帮助弹窗。空输入与未知输入留在命令框并显示 validation error。

### 10.7 ShortcutHelp 模式

```text
h from any Normal focus
  -> open_popup(); previous_focus = current focus
  -> GlobalMode::ShortcutHelp { scroll: 0 }

w/s or Up/Down or k/j -> scroll one line
PageUp/PageDown    -> scroll one page
Home/End           -> jump start/end
h/Enter/Esc/q       -> close_popup(); restore previous_focus
```

帮助内容只由有效 keymap 的 Global 表与来源 focus 对应的唯一 `ShortcutContext` 表生成；
其他 View、其他 focus、modal 与 prompt command 表不进入弹窗。每行显示有效 binding、label、
operation id，来源 focus 用 `▶` 标记，`bindings=[]` 显示 `(unbound)`。

---

## 11. Store 更新规则

### 11.1 Store 是唯一状态源

Renderer 不保存业务状态。所有状态必须来自 AppState。

### 11.2 GitResponse 应用规则

```rust
pub fn apply_git_response(&mut self, envelope: GitResponseEnvelope) {
    let Some(kind) = self.pending_jobs.remove(&envelope.id) else { return };
    let repository_index = kind.repository_index();
    if self.is_stale(envelope.id, &kind) {
        return;
    }
    match envelope.response {
        GitResponse::RepositoryStatusLoaded(repo) => {
            self.repositories[repository_index].repository = Some(repo);
        }
        GitResponse::BranchesLoaded(branches) => {
            self.repositories[repository_index].branches = branches;
            self.ensure_valid_branch_selection();
        }
        GitResponse::CommitsLoaded { branch, commits } => {
            self.branch_commits_repository_index = Some(repository_index);
            self.branch_commits = CommitList { viewing_branch: Some(branch), items: commits };
            self.ensure_valid_commit_selection();
        }
        GitResponse::ReflogLoaded(entries) => {
            self.reflog_repository_index = Some(repository_index);
            self.reflog_entries = entries;
        }
        // detail / diff / command responses follow the same repository guard
        // and stale-id rules.
        _ => { /* apply response */ }
    }
}
```

### 11.3 Selection 修正规则

当列表被刷新后，selection 必须保证合法：

```text
if list is empty:
  selected_index = None
else if selected_index is None:
  selected_index = Some(0)
else if selected_index >= list.len():
  selected_index = Some(list.len() - 1)
else:
  keep selected_index
```

---

## 12. GitRequest 与 GitResponse

### 12.1 GitRequest

```rust
pub enum GitRequest {
    LoadRepositoryStatus,
    LoadBranches,
    LoadRemotes,
    LoadCommits {
        branch: BranchName,
        limit: usize,
    },
    LoadCommitDetail {
        commit: CommitHash,
    },
    LoadCommitMessage {
        commit: CommitHash,
    },
    LoadFileDiff {
        commit: CommitHash,
        path: GitPath,
        old_path: Option<GitPath>,
    },
    LoadReflog {
        limit: usize,
    },
    LoadWorkingTree,
    LoadWorkingTreeDiff {
        path: GitPath,
        old_path: Option<GitPath>,
        include_staged: bool,
        include_worktree: bool,
        untracked: bool,
    },
    StagePaths {
        paths: Vec<GitPath>,
    },
    UnstagePaths {
        paths: Vec<GitPath>,
    },
    Commit {
        message: String,
    },
    Fetch,
    PullRebase,
    Push,
    AddRemote { name: String, url: String },
    SetRemoteUrl { name: String, url: String },
    SetUpstreamRemote { name: String },
    SwitchBranch {
        branch: BranchName,
    },
    CherryPick {
        commits: Vec<CommitHash>,
    },
    Reset {
        commit: CommitHash,
        mode: ResetMode,
    },
    Rebase {
        upstream: BranchName,
    },
}
```

`old_path` 用于 rename/copy 文件的 pathspec 回退。`GitPath` 必须保留 Git 输出的原始字节；显示层可以使用 lossy 文本，但再次调用 Git 时不能丢失非 UTF-8 路径。

### 12.2 GitResponse

```rust
pub enum GitResponse {
    RepositoryStatusLoaded(Repository),
    BranchesLoaded(Vec<Branch>),
    RemotesLoaded(Vec<RemoteInfo>),
    CommitsLoaded {
        branch: BranchName,
        commits: Vec<Commit>,
    },
    CommitDetailLoaded(CommitDetail),
    CommitMessageLoaded {
        commit: CommitHash,
        message: String,
    },
    FileDiffLoaded(FileDiff),
    ReflogLoaded(Vec<ReflogEntry>),
    WorkingTreeLoaded(Vec<WorkingTreeChange>),
    WorkingTreeDiffLoaded(WorkingTreeDiff),
    CommandSucceeded {
        message: String,
    },
    CommandFailed {
        command: String,
        stderr: String,
    },
    RebaseConflictAborted {
        command: String,
        stderr: String,
    },
}

pub struct GitResponseEnvelope {
    pub id: GitJobId,
    pub response: GitResponse,
}
```

---

## 13. Git 命令规划

### 13.1 Repository Status

```bash
git rev-parse --show-toplevel
git rev-parse --verify --short=8 HEAD  # unborn repository 时允许为空
git branch --show-current
git status --porcelain=v1 -z -b
```

### 13.2 Branch List

```bash
git for-each-ref \
  --format="%(refname)%00%(refname:short)%00%(objectname)%00%(objectname:short)%00%(committerdate:iso8601-strict)%00%(subject)%00%(HEAD)" \
  refs/heads refs/remotes
```

### 13.3 Load Commits

```bash
git log <branch> \
  --max-count=300 \
  --date=iso-strict \
  --decorate=short \
  --format="%x1e%H%x1f%h%x1f%an%x1f%aI%x1f%D%x1f%s" \
  --
```

### 13.4 Commit Detail

```bash
git show --no-patch --date=iso-strict --format=<record-separated-metadata> <commit>
git show --no-patch --format=%B <commit>  # clipboard-only full message request
git diff-tree --root -m --first-parent --no-commit-id --name-status -r -M -z <commit>
git show --first-parent --numstat -z --format= --find-renames <commit>
git show --first-parent --format= --patch --find-renames --no-ext-diff --no-color <commit>
```

### 13.5 File Diff

```bash
git show --first-parent --format= --patch --find-renames --no-ext-diff --no-color <commit> -- <path> [<old_path>]
```

### 13.6 Fetch / Pull / Push / Reflog / Reset / Rebase

```bash
git fetch --all --prune
git pull --rebase
git push
git remote
git remote get-url --all <name>
git remote get-url --push --all <name>
git config --local --null --get-all remote.<name>.url
git config --local --null --get-all remote.<name>.pushurl
git remote add -- <name> <shared-url>
git config --local branch.<branch>.remote <name>
git config --local branch.<branch>.merge refs/heads/<branch>
git config --local branch.<branch>.pushRemote <name>
git reflog show --max-count=300 --date=iso-strict --format=<record-separated-fields>
git reset --soft  <commit>
git reset --mixed <commit>
git reset --hard  <commit>
git rebase <selected-upstream>
git diff --name-only --diff-filter=U  # failed rebase conflict detection
git rebase --abort                    # automatic conflict rollback
```

### 13.7 Changes 数据源

```bash
git status --porcelain=v1 -z --untracked-files=all
git diff --cached --patch --find-renames --no-ext-diff --no-color -- <path> [<old-path>]
git diff          --patch --find-renames --no-ext-diff --no-color -- <path> [<old-path>]
git diff --no-index --patch --no-ext-diff --no-color -- /dev/null <untracked-path>
```

`WorkingTreeChange` 是 Changes 的 Git-facing 数据源，保留 porcelain `XY`、raw `GitPath` 和 rename old path。一个 `MM` 文件会在三级树的 Staged 与 Unstaged 分组各出现一次；选择其中一项时只请求该边界的 patch。冲突文件保留 `UU/AA/...` 并归入 Unstaged，untracked 使用 `??`。`--no-index` 的退出码 1 表示“发现差异”，在该请求中属于成功结果而不是命令失败。

Worker 仍返回 raw patch section；Controller 通过同一个 `parse_file_diff` 将其解析为 `FileDiff`，Renderer 再将 commit diff 与 Changes diff 都交给同一个 `render_diff_panel`。因此 unified/side-by-side、行号、配色、wrap、窄终端降级和滚动行为只有一套实现。

### 13.8 Changes 写操作

```bash
git add --all -- <selected-paths...>
git reset -- <selected-paths...>
git commit -m <validated-message>
```

Stage/unstage 只接受 Controller 从 Changes 选择集合生成的原始 `GitPath` argv。`git reset -- <paths>` 是 path-limited mixed reset，只改变 index，不覆盖 working-tree 内容，并可用于尚无 commit 的 unborn repository。rename 节点同时带上 old/new path，防止只更新一侧。

选择集合的 key 是 `(ChangeGroup, GitPath)`，所以 `MM` 文件的 Staged 与 Unstaged 节点可以独立选择。操作完成后清空选择，重新加载 repository status、Changes tree 和当前 diff；执行期间禁止并发提交第二个写 job。

Commit 弹窗使用独立的 `EditingCommitMessage` mode；空白消息在 Controller 和 runner 两侧都拒绝。提交使用当前 index 的全部 staged 内容，不隐式 stage 其他文件。当前只支持文件级 stage/unstage，不支持逐行或逐 hunk partial staging。

### 13.9 后台操作日志

GitCommandBus 对每个 request 写入持久化 JSONL 生命周期记录：

```text
session_started
queued    { job_id, cwd, operation, details }
started   { job_id, cwd, operation, details }
completed { job_id, cwd, operation, status, duration_ms, summary }
```

日志默认使用平台 state/log 目录，可由 `PITUI_LOG` 覆盖；初始化失败时回退到临时目录。当前文件达到 5 MiB 后轮转为 `.1`，保留一份备份。日志运行时 I/O 失败不得阻塞 Git worker。

每个 `GitRequest` 必须由穷尽 match 映射到稳定 operation 名称，因此后台刷新与所有读写操作都可按 job id 追踪。日志包含 repository/file path 和截断后的 Git error，但不写 diff、文件内容或解析后的 commit message；commit request 只记录 message byte length，失败命令中的 message 也必须 redacted。

`for-each-ref refs/heads` 无法返回尚未产生第一个 commit 的 unborn branch。`RepositoryState::ensure_current_branch_visible` 必须用 `git branch --show-current` 的结果补出该 branch child，因此空仓库也必须渲染为两行：repository root + `└─ * main  unborn`。

`GitJob` 包含 `{ id, cwd, request }`。所有 read/write request 都在提交时绑定仓库路径；worker 不持有全局单仓库 cwd，避免多仓库请求串线。

所有命令必须通过 argv 调用，不允许拼接 shell 字符串。Git 元信息、路径和 diff 内容在进入 Renderer 后必须清理终端控制字符和 bidi override，避免终端转义注入。

---

## 14. Renderer 规则

### 14.1 Renderer 输入

Renderer 只接受：

```text
&AppState
```

禁止：

```text
Renderer 调用 git
Renderer 修改 AppState
Renderer 保存业务状态
```

### 14.2 顶部 Status Bar

状态栏字段：

```text
repo
current_branch
selected_commit
selected_file
operation
staged_count
unstaged_count
untracked_count
conflicted_count
ahead / behind
```

示例：

```text
repo=pitui | branch=main | commit=3cc6d76 | file=a.rs | op=NORMAL | S=1 M=3 U=0 C=0 | ↑2 ↓0
```

### 14.3 底部 Hotkey Bar

Hotkey bar 根据 `screen + focus + mode + selection` 动态生成。

示例：

```text
BranchOverview + BranchList:
repo: Enter expand/collapse | f fetch | p pull --rebase | P push | g reflog | o remotes
branch: Enter view commits | S switch | b rebase | o remotes

BranchOverview + CommitList:
Space select | Ctrl+C copy… | Enter detail | y cherry-pick selected | R reset

Shortcut + CommitCopy:
h hash | i info | m message | Esc cancel

CommitDetail + CommitFileList:
Space expand | Enter file diff | Home/End | PgUp/PgDn | Ctrl+C copy file… | Esc back

Shortcut + FileCopy:
n file name | a absolute path | r relative path | Esc cancel

FileDiffDetail + DiffView:
v mode | n next file | p prev file | Home/End | PgUp/PgDn | W wrap | Tab focus | Esc back
(no copy chord in DiffView)

Reflog + ReflogList:
R reset | Esc back | q quit

Remotes + RemoteList:
A add remote | e set shared URL | u set upstream | Ctrl+R refresh | Esc back

Changes + ChangesTree / ChangesDiff:
Enter/a/d/←/→ expand/collapse | w/s/↑/↓ select/scroll | Home/End | PgUp/PgDn | Ctrl+R refresh | Esc back

Global normal mode:
h help | Ctrl+Backtick command | Ctrl+G changes | Ctrl+R refresh（所有主 screen 都放在 hotkey bar 最前面）
```

Normal/Chord footer 来自 `ResolvedKeymap::footer_items`；quick command、filter、confirm、
commit submission、remote editor、error 与 shortcut help footer 来自相应 `ModalShortcutSet.footer`，不再由
Renderer 复制操作字符串。

---

## 15. 启动流程

```text
main
  -> parse `pitui [REPOSITORY ...]`; 无参数时使用 cwd
  -> initialize terminal
  -> 为每个路径创建 RepositoryState
  -> 并行语义地提交每个仓库的 LoadRepositoryStatus（单 worker 串行执行）
  -> 每个成功仓库加载 branches；active repository 加载 commits
  -> enter event loop
```

初始状态：

```rust
AppState {
    repositories: repository_paths.map(RepositoryState::new),
    active_repository_index: first_repository,
    branch_commits: CommitList::empty(),
    reflog_entries: vec![],
    current_commit_detail: None,
    current_file_diff: None,
    screen: Screen::BranchOverview,
    focus: FocusPanel::BranchList,
    previous_focus: None,
    mode: GlobalMode::Normal,
    diff_mode: DiffViewMode::Unified,
    commit_selection: HashSet::new(),
    pending_jobs: vec![],
    last_error: None,
}
```

启动后的自动加载：

```text
1. 每个 repository: LoadRepositoryStatus(repository_index)
2. 每个成功 repository: LoadBranches(repository_index)
3. active repository 有 current_branch：LoadCommits(current_branch)
4. active repository detached HEAD：LoadCommits(HEAD)
```

---

## 16. Tick / Refresh 策略

```text
tick interval: 200ms
periodic repository polling: disabled
repository status / branch list refresh: 启动初始化、Ctrl+R、Git 写操作完成后
commit list refresh: 选择分支 Enter、Ctrl+R 或 Git 写操作完成后
commit detail refresh: 进入 View 2 或切换 commit 后 Enter
diff refresh: 进入 View 3 或切换文件后
remote config refresh: 进入 Remote Management、Ctrl+R 或 remote 写操作完成后
```

Event loop 持续拉取 `GitResponse` 并更新 `pending_jobs`。Tick 只在已经存在 pending job 时驱动 loading indicator：

```text
Tick -> tick_count += 1
Tick -/-> GitRequest
```

不存在 2 秒或其他固定周期的 repository status 请求。外部 Git 命令或编辑器改动由用户在任意主界面按 `Ctrl+R` 同步；应用启动和 Pitui 自身写操作完成后的事件驱动刷新仍保留。

---

## 17. Side-by-side Diff 转换规则

输入为 parsed unified diff：

```text
DiffHunk -> Vec<DiffLine>
```

输出：

```rust
pub struct SideBySideRow {
    pub left_line_no: Option<u32>,
    pub left_text: Option<String>,
    pub left_kind: DiffCellKind,

    pub right_line_no: Option<u32>,
    pub right_text: Option<String>,
    pub right_kind: DiffCellKind,
}
```

转换规则：

```text
Context:
  left = context
  right = context

Delete followed by Add:
  pair into one Modified row

Delete not followed by Add:
  left = deleted
  right = empty

Add not paired:
  left = empty
  right = added

HunkHeader:
  render as full-width separator row
```

宽度规则：

```text
terminal_width >= 140:
  allow SideBySide

terminal_width < 140:
  force Unified render
  keep app.diff_mode unchanged
```

---

## 18. 错误处理

### 18.1 Git command failure

```text
GitResponse::CommandFailed
  -> stay TypingConfirmation
  -> validation_error = Some(...)
```

### 9.4 Remote sync：Fetch / Pull --rebase / Push

仅仓库节点显示 `f`。确认弹窗必须同时展示仓库名、绝对路径和准确命令：

```text
git fetch --all --prune
```

job 使用节点的 `repository_index` 解析 cwd；成功后只对该仓库执行 full refresh。

同一 repository 节点提供：

```text
p -> git pull --rebase
P -> git push
```

两者都必须显示仓库、绝对路径、当前分支和准确命令，并经过确认。Pull 的策略固定为 rebase，不允许根据 `pull.rebase` 配置退化成 merge；Controller 与 worker 双重检查 attached branch、干净的 working tree/index 和既存 rebase。若本次 pull 产生冲突，复用 safe rebase 的冲突检测与 `git rebase --abort` 回滚。

Push 使用 Git 已配置的 upstream/default push target，不在 push 时自动执行 `--set-upstream`，避免猜测 remote 或发布错误分支；upstream 只能由用户在 Remote Management 中显式选择。所有 remote 操作都继承 `GIT_TERMINAL_PROMPT=0`，不会在 TUI 后台等待交互式认证。

Worker 在真正执行 fetch/pull/push 前每次重新读取 local Git config，强制两层策略：

1. 每个 remote 的有效 `remote.<name>.url` 与 `remote.<name>.pushurl` 列表必须完全相同；未配置 `pushurl` 时 Git 有效 push URL 等于 fetch URL，因此合法。
2. 当前分支的 `branch.<branch>.remote` 与 Git 解析出的 push remote 必须是同一个 remote；`branch.<branch>.pushRemote` 或 `remote.pushDefault` 将路由拆分到另一 remote 时拒绝。

校验只执行本地 `git config` / `git remote` 读取；不合法时在联系网络前返回可修复错误。

### 9.5 Safe Rebase

分支节点 `b` 的语义固定为：**把该仓库当前分支 rebase 到所选分支**，即执行：

```text
git rebase <selected-upstream>
```

安全约束：

1. 必须处于 attached branch。
2. working tree 与 index 必须干净。
3. 弹窗展示 current branch、selected upstream 和准确命令。
4. controller 提交前检查一次；worker 真正执行前再次检查 attached branch、clean status 和 `rebase-merge` / `rebase-apply`，避免状态刷新与执行之间的竞态。
5. 如果请求前已经存在 rebase，worker 必须拒绝新请求并保持原 rebase 完全不变，绝不能代替用户 abort。
6. worker 串行执行 rebase。
7. 仅当本次请求创建了 rebase state，且 `git diff --name-only --diff-filter=U` 检测到冲突时，worker 立即执行 `git rebase --abort`。
8. abort 成功后返回 `RebaseConflictAborted`；UI 弹窗说明已恢复原分支与工作区，并刷新仓库。
9. abort 本身失败时保留原错误与 abort 错误，禁止声称已恢复。

### 9.6 Reflog

仓库节点 `g` 加载该仓库最近 300 条 reflog：

```text
git reflog show --max-count=300 --date=iso-strict --format=<record-separated-fields>
```

Reflog view 左侧展示 short hash、selector、action 和 message，右侧展示完整 hash、author 与 commit date。`R` 复用 9.3 的 reset 流程，实际目标始终使用完整 object id，而不是解析 selector 文本。

### 9.7 Remote Management

仓库或分支节点按 `o` 加载 `GitRequest::LoadRemotes`。`RemoteInfo` 保留 remote name、fetch URL 列表、Git 有效 push URL 列表、当前分支 fetch upstream 和 push target 标记。Renderer 用 `★/F/P/!` 区分双向 upstream、仅 fetch、仅 push 以及 URL policy 违规。

```text
a -> EditingRemote(Add: name + shared URL) -> Confirming(AddRemote)
e -> EditingRemote(SetUrl: shared URL) -> Confirming(SetRemoteUrl)
u -> Confirming(SetUpstreamRemote for current branch)
```

- Add 只接受一个 URL，`git remote add` 自然使 fetch/push 使用同一地址。
- SetUrl 用 local config transaction 将 `remote.<name>.url` 归一为输入 URL，并移除全部显式 `pushurl`；任一步失败都尝试恢复两个 key 的原值。
- SetUpstreamRemote 仅接受 URL 已一致的 remote，事务式更新 `branch.<branch>.remote`、`merge=refs/heads/<branch>` 和 `pushRemote`，保证 pull/push 路由到同一 remote。远程分支可尚未存在，下一次经确认的 push 可创建它。
- URL 不写入 operation log details；remote 写请求的失败 command 也使用 `<redacted-url>`。

### 9.8 独立 Changes

`Ctrl+G` 从任意主 screen 进入 Changes，不要求焦点停在仓库节点。它保存进入前的 `(Screen, FocusPanel)`，因此 `Ctrl+G`、`Esc` 或 Back 都返回原位置，而不是固定跳回 Branch Overview。

```text
┌──────────────────────────────────┬──────────────────────────────────────────┐
│ Changes  S:2 U:3 · selected: 2  │ Staged Changes — both.txt (unified)      │
│ ▼ [-] Changes                   │ @@ -1 +1,2 @@                            │
│   ├─▼ [x] Staged Changes (2)     │      1     1  base                      │
│   │  ├─ [x] M both.txt           │            2 +index line                │
│   │  └─ [x] A staged.txt         │                                          │
│   └─▼ [ ] Unstaged Changes (3)   │                                          │
│      ├─ [ ] M both.txt           │                                          │
│      ├─ [ ] M modified.txt       │                                          │
│      └─ [ ] ? untracked.txt      │                                          │
└──────────────────────────────────┴──────────────────────────────────────────┘
```

三级树与交互规则：

1. 第一级始终是 `Changes` root；第二级固定为 `Staged Changes` 与 `Unstaged Changes`；第三级才是文件。
2. `MM` 文件同时出现在两个分组。Staged 节点只加载 `git diff --cached`，Unstaged 节点只加载 `git diff`，绝不在右侧混合两个边界。
3. untracked 与 conflicted 文件归入 Unstaged；untracked 使用 `git diff --no-index` 预览，冲突保留 Git 的 unmerged patch。
4. root 和 group 支持 `Enter`、`a/←`、`d/→/l` 折叠/展开；文件节点 `Enter` 聚焦右侧。`Tab` 始终在 tree/diff 间切换。
5. 右侧复用 File Diff Detail 的 `render_diff_panel`、`unified_text`、`side_by_side_text`；`v` 切换模式，`W` 切换 wrap，终端宽度小于 140 时 side-by-side 自动降级。
6. rename/copy 显示 `old → new`；Git argv 始终使用原始 `GitPath` 字节。
7. 空文件或 binary 无 textual diff 时显示明确占位；快速选择文件产生的旧 response 由 latest job id 丢弃。
8. `PendingJobKind::Changes*` 携带 `repository_index` 和 diff group；离开 screen、切换仓库或快速切换文件后，过期 response 不得覆盖当前内容。
9. `Space` 在 file/group/root 上选择或反选对应节点；diff focus 下仍操作左侧当前文件。父节点用 `[ ]`、`[x]`、`[-]` 表示空选、全选和部分选择。
10. `S` 只 stage 已选的 Unstaged 节点，`u` 只 unstage 已选的 Staged 节点；没有显式选择时回退到当前同组文件。混合选择不会越过 group 边界。
11. `c` 只在至少一个 staged 文件存在时打开 commit message 弹窗；`Enter` 提交、`Esc` 取消，空消息留在弹窗并显示 validation error。
12. `Home/End/PageUp/PageDown` 在 ChangesTree 中跳转/翻页选择并只加载最终文件；在 ChangesDiff 中跳到内容首尾或按 10 行翻页，不改变 focus。

错误弹窗：

```text
Command failed:
<command>

stderr:
<stderr>

Enter / Esc dismiss
```

### 18.2 Error 状态转移

```text
Error + Enter/Esc
  -> last_error = None
  -> mode = Normal
  -> focus = previous_focus
```

---

## 19. 第一版里程碑

### Milestone 1: 框架与状态栏

```text
- 启动 TUI
- 检测 git repo
- 显示 repo / branch / head / status
- 显示动态 hotkeys
- q 退出
```

### Milestone 2: Branch / Commit Overview

```text
- 加载 branch list
- 选择 branch
- 不切换分支加载 commit list
- Enter 进入 commit detail
```

### Milestone 3: Commit Detail

```text
- 左侧 commits
- 右侧完整 Commit 栏（metadata + changed files）
- 文件可展开 hunk summary
- Enter 进入 file diff
```

### Milestone 4: File Diff Detail

```text
- 左侧完整复用 Commit 栏（metadata + files）
- 右侧 unified diff
- v 切换 side-by-side
- n / p 切换文件
- Esc 返回
```

### Milestone 5: 可写操作

```text
- branch switch confirmation
- repository pull --rebase / push confirmation
- explicitly selected commits cherry-pick confirmation
- reset typed confirmation
- multi-repository tree + per-repository fetch/reflog/remote management
- shared fetch/push URL policy + explicit per-branch upstream remote
- hierarchical repository/branch connectors + unborn current branch child
- global Changes screen + three-level staged/unstaged/file tree
- shared unified/side-by-side diff component for commit and Changes patches
- file/group multi-select + file-level stage/unstage + commit message dialog
- multi-select commit hash + current info/full-message copy through OSC 52
- focus-mounted Ctrl+C palettes: Commits h/i/m; file columns n/a/r; none in DiffView
- callable CommandSpec / modal input jump tables, `h` current-focus shortcut reference, and Ctrl+Backtick command prompt
- File Diff file selection refresh without asynchronous focus stealing
- soft/mixed reset + two-stage hard reset
- safe rebase + conflict auto-abort
- persistent rotated JSONL lifecycle log for every Git worker job
```

---

## 20. 不做事项

当前暂不实现：

```text
- 逐行 / 逐 hunk partial staging
- stash 管理
- interactive rebase todo 编辑
- merge conflict editor
- blame view
- 内置文件编辑器
```

Pitui 的定位是：

```text
安全、克制、以查看和选择 commit 为核心的 Git TUI。
```

---

## 21. License 与 GitHub 协作

- 项目使用 MIT License，版权标识为 `Copyright (c) 2026 Pitui contributors`。
- README 明确披露当前全部代码、测试和文档由 vibe coding 生成；维护者仍对需求、审阅和验证负责。
- `.github/workflows/ci.yml` 在 Linux 执行 format/clippy，并在 Linux、macOS、Windows 执行完整测试。
- Dependabot 同时跟踪 Cargo 与 GitHub Actions 依赖。
- Bug / Feature Issue Forms、PR template、`CONTRIBUTING.md` 和 `SECURITY.md` 规范复现信息、危险 Git 操作审查、AI 生成披露与私密漏洞报告。

---

## 22. 全局配置层（第一阶段已实现）

全局配置层采用版本化 TOML，并在进入 terminal raw mode 前完成加载、合并与严格校验。
它只读取受信任的用户全局配置，不读取仓库内配置；默认配置保持原有绑定、Git 与日志
语义，同时把 footer 升级为 action-only 和逐级 chord 提示。

核心设计约束：

1. 建立稳定的 `CommandId + CommandSpec` 可调用跳表；`CommandId as usize` 直接索引包含
   `fn(&AppState) -> Option<Action>` 的 spec。输入映射、二级快捷键、底部 hotkey bar 和
   全局帮助框共同读取同一份有效 keymap，禁止分别维护绑定字符串。
2. footer 只展示当前状态下 input resolver 会接受的下一键；chord 在 root 只展示第一级
   prefix，按下后才展示第二级，三段 chord 继续逐级揭示，不提前泄露后续提示。
3. 配置可覆盖单键、修饰键及多段 chord，并独立配置哪些 command/chord group 提示可见、
   footer label、优先级、显示模式与最大行数；隐藏提示不解除绑定。
4. `[diff].default_mode` 配置 Commit File Diff 与 Changes Diff 共用的启动默认模式；
   side-by-side 在窄终端仍保持安全的临时 unified 降级。
5. 日志配置覆盖路径、level、target level、flush interval、writer buffer、单文件大小、
   备份数量与打开失败策略，同时继续强制脱敏和运行时 I/O best-effort 语义。
6. 不自动监视配置文件；后续手动 reload 必须完整校验后原子切换，失败时保留最后一份
   有效配置，避免输入与提示跨 generation 不一致。
7. 第一阶段不提供 TUI 配置编辑器，不配置 shell/Git 命令模板，也不改变当前 Git 安全策略。
8. Normal 命令按十个 `ShortcutContext` 精确挂载：Commits 使用 commit copy，文件列使用
   file copy，DiffView 不挂载 copy；modal 输入另由 `MODE_KEY_TABLES` 独占。
9. `h` 打开当前-focus 快捷键参考框，只展示 Global 与来源 `ShortcutContext` 的有效
   binding/operation id，并在关闭时恢复来源 focus；Ctrl+Backtick 打开快捷命令框，
   `Ctrl+Space` 不绑定，`help` 返回同一指南。

第一阶段不包含运行时 reload、文件 watcher、TUI 配置编辑器或独立异步日志事件队列。
完整 schema、快捷键冲突规则、footer 生成算法、日志轮转/刷新策略、模块边界、测试矩阵与
后续计划见 [`global-configuration-design.md`](global-configuration-design.md)。
按 Branch Overview、Commit Detail、File Diff、Changes、Reflog、Remotes 拆分 Fields 与
Operations 的 Schema v2 设计见
[`view-configuration-design.md`](view-configuration-design.md)；该 schema 尚未实现。
