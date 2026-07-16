# Pitui 多仓库五视图 Git TUI 设计文档

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
必要时安全执行 switch / cherry-pick / reset
```

五视图结构：

```text
View 1: Branch / Commit Overview
  左侧 Repositories / Branches 树
  右侧 Commits

View 2: Commit Detail
  左侧 Commits
  右侧 Files changed in commit

View 3: File Diff Detail
  左侧 Files
  右侧 Diff Detail

View 4: Reflog
  左侧 Reflog Entries
  右侧 Selected Entry

View 5: Changes
  左侧 Changes -> Staged/Unstaged -> File 三级树
  右侧复用 File Diff 的 unified / side-by-side 组件

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
    Error,
}
```

`Loading` 不作为独占模式；加载状态统一由 `pending_jobs` 派生，避免异步读取期间阻断浏览和返回操作。

### 3.5 AppState

```rust
pub struct AppState {
    pub repositories: Vec<RepositoryState>,
    pub backend_log_path: Option<PathBuf>,
    pub backend_logging_warning: Option<String>,
    pub active_repository_index: Option<usize>,
    pub branch_commits_repository_index: Option<usize>,
    pub branch_commits: CommitList,

    pub reflog_repository_index: Option<usize>,
    pub reflog_entries: Vec<ReflogEntry>,
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

    pub cherry_pick_queue: Vec<CommitHash>,
    pub cherry_pick_queue_repository_index: Option<usize>,
    pub commit_copy_selection: HashSet<CommitHash>,
    pub commit_copy_selection_repository_index: Option<usize>,
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

`PendingJobKind` 对每类请求都保存 `repository_index`。列表、commit detail 和 file diff 仅应用对应仓库、对应上下文的最新 job response，防止跨仓库或快速切换时旧响应覆盖当前状态。cherry-pick queue 也绑定单一仓库，不允许将不同仓库的 commit 混入同一次写操作。

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

    ToggleCommitCopySelection,
    CopySelectedCommitHashes,
    CopyCurrentCommitInfo,
    CopyCurrentCommitMessage,

    QueueCherryPickSelectedCommit,
    OpenCherryPickQueueDialog,
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

CommitDetail
  Enter on CommitFileList
    -> FileDiffDetail
  Esc / Back
    -> BranchOverview

FileDiffDetail
  Esc / Back
    -> CommitDetail

任意主视图
  Ctrl+G
    -> Changes（保存原 screen + focus）

Changes
  Ctrl+G / Esc / Back
    -> 恢复原 screen + focus
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
4. 安全执行 fetch、switch 和 rebase。
5. 从仓库节点进入该仓库 reflog。
6. 从仓库节点查看当前 working tree 状态和 diff。

### 6.2 布局

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ repo=pitui | branch=main | viewing=feature/x | op=NORMAL | S=0 M=1 U=0    │
├──────────────────────────────┬─────────────────────────────────────────────┤
│ Repositories / Branches      │ Commits                                     │
│ ▼ ● pitui /repo/pitui        │ > 3cc6d76 Fix Debug Layer error             │
│   ├─ * main                  │   0acaf2e Fix color grading format          │
│   └─   feature/x             │   ab244d6 Add final_render_result_texture   │
│ ▶ ○ backend /repo/backend    │                                             │
├──────────────────────────────┴─────────────────────────────────────────────┤
│ Tab focus | Enter view | s switch | / filter | r refresh | q quit         │
└────────────────────────────────────────────────────────────────────────────┘
```

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
| ↑ / k | MoveUp | tree 非空 | 扁平化 tree selection 上移；跨仓库时更新 active repository |
| ↓ / j | MoveDown | tree 非空 | 扁平化 tree selection 下移；跨仓库时更新 active repository |
| PageUp | PageUp | tree 非空 | 树向上翻页 |
| PageDown | PageDown | tree 非空 | 树向下翻页 |
| Home | Home | tree 非空 | 选择第一行 |
| End | End | tree 非空 | 选择最后一行 |
| Enter | LoadCommitsForSelectedBranch | 已选择仓库 | 展开/折叠仓库节点 |
| Enter | LoadCommitsForSelectedBranch | 已选择分支 | 加载该分支 commits，不切换真实分支 |
| f | OpenFetchRepositoryDialog | 已选择仓库 | 确认执行 `git fetch --all --prune` |
| g | OpenReflog | 已选择仓库 | 加载该仓库最近 300 条 reflog |
| s | OpenSwitchBranchDialog | 已选择分支 | 进入切分支确认弹窗 |
| b | OpenRebaseDialog | 已选择分支 | 将当前分支安全 rebase 到所选分支 |
| / | StartFilter | 总是 | 按仓库名、路径、分支或 subject 过滤树 |
| r | RefreshRepository | 总是 | 刷新全部 repo status / branch list / 当前 viewing commits |
| Tab | FocusNext | 总是 | focus -> CommitList |
| q | Quit | 总是 | 退出应用 |

### 6.5 CommitList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| ↑ / k | MoveUp | commits 非空 | selected_commit_index 上移 |
| ↓ / j | MoveDown | commits 非空 | selected_commit_index 下移 |
| PageUp | PageUp | commits 非空 | commit 列表向上翻页 |
| PageDown | PageDown | commits 非空 | commit 列表向下翻页 |
| Home | Home | commits 非空 | 选择第一个 commit |
| End | End | commits 非空 | 选择最后一个 commit |
| Enter | OpenCommitDetail | 已选择 commit | 加载 commit detail，然后进入 View 2 |
| Space | ToggleCommitCopySelection | 已选择 commit | 加入/移出独立的复制多选集合 |
| c / Ctrl+C | CopySelectedCommitHashes | commits 非空 | 按列表顺序复制多选完整 hashes；集合为空则复制当前 hash |
| i / Ctrl+Shift+C | CopyCurrentCommitInfo | 已选择 commit | 复制 hash、author、date、refs 与 message |
| m / Ctrl+Alt+C | CopyCurrentCommitMessage | 已选择 commit | 复制完整 message；缺少 detail 时后台加载，不切换 screen/focus |
| y | QueueCherryPickSelectedCommit | 已选择 commit | 加入 cherry-pick queue |
| Y | OpenCherryPickQueueDialog | queue 非空 | 打开 cherry-pick queue 确认弹窗 |
| R | OpenResetDialog | 已选择 commit | 打开 reset typed confirmation 弹窗 |
| / | StartFilter | commits 非空 | 进入 commit search/filter 模式 |
| r | RefreshRepository | 总是 | 刷新 repo status / branch list / commits |
| Tab | FocusNext | 总是 | focus -> BranchList |
| Esc | Back | 总是 | 若无上层视图，则保持当前视图 |
| q | Quit | 总是 | 退出应用 |

`commit_copy_selection` 与 cherry-pick queue 完全独立；切换仓库或 viewing branch 时清空，过滤列表不会改变集合。剪贴板由 TUI 层通过 OSC 52 写入，不引入平台特定 clipboard 命令。message copy 必须返回完整 subject/body；若 `CommitDetail` 尚未缓存，则发送独立 `LoadCommitMessage`，响应只写 clipboard，不得导航到 Commit Detail 或改变当前 focus。

### 6.6 View 1 状态转移

#### 6.6.1 选择分支

```text
BranchOverview + BranchList + MoveUp/MoveDown
  -> selected_branch_index changed
  -> active_repository_index = selected node repository
  -> 跨仓库时清空旧 detail/diff 并加载新仓库 viewing/current branch
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

1. 保留左侧 commit 列表上下文。
2. 查看当前 commit 的元信息。
3. 查看当前 commit 修改了哪些文件。
4. 展开文件以查看 hunk summary。
5. 进入单文件 diff 详情。

### 7.2 布局

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ repo=pitui | branch=main | viewing=feature/x | commit=3cc6d76 | NORMAL     │
├──────────────────────────────┬─────────────────────────────────────────────┤
│ Commits                      │ Files changed in commit                     │
│ > 3cc6d76 Fix Debug Layer    │ > ▶ M shaders/Collision.hlsl        +20 -5  │
│   0acaf2e Fix color grading  │   ▶ A src/git/parser.rs            +130 -0  │
│   ab244d6 Add texture        │   ▶ D old/status.rs                 +0 -80  │
│                              │                                             │
│                              │ Commit: 3cc6d76                             │
│                              │ Author: xxx                                 │
│                              │ Date: 2026-07-16 13:20:45                   │
│                              │ Message: Fix Debug Layer error              │
├──────────────────────────────┴─────────────────────────────────────────────┤
│ Tab focus | Space expand | Enter file diff | y queue | Esc back           │
└────────────────────────────────────────────────────────────────────────────┘
```

### 7.3 FocusPanel

View 2 允许两个 focus：

```text
CommitList
CommitFileList
```

默认进入 View 2 时：

```text
screen = CommitDetail
focus = CommitFileList
```

### 7.4 CommitList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| ↑ / k | MoveUp | commits 非空 | 选择上一个 commit |
| ↓ / j | MoveDown | commits 非空 | 选择下一个 commit |
| PageUp | PageUp | commits 非空 | commit 列表向上翻页 |
| PageDown | PageDown | commits 非空 | commit 列表向下翻页 |
| Enter | OpenCommitDetail | 已选择 commit | 重新加载所选 commit detail |
| y | QueueCherryPickSelectedCommit | 已选择 commit | 加入 cherry-pick queue |
| Y | OpenCherryPickQueueDialog | queue 非空 | 打开 cherry-pick queue 确认弹窗 |
| R | OpenResetDialog | 已选择 commit | 打开 reset typed confirmation 弹窗 |
| Tab | FocusNext | 总是 | focus -> CommitFileList |
| Esc | Back | 总是 | screen -> BranchOverview |
| q | Quit | 总是 | 退出应用 |

### 7.5 CommitFileList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| ↑ / k | MoveUp | files 非空 | selected_file_index 上移 |
| ↓ / j | MoveDown | files 非空 | selected_file_index 下移 |
| PageUp | PageUp | files 非空 | 文件树向上翻页 |
| PageDown | PageDown | files 非空 | 文件树向下翻页 |
| Home | Home | files 非空 | 选择第一个文件 |
| End | End | files 非空 | 选择最后一个文件 |
| Space | ToggleFileExpanded | 已选择文件 | 展开 / 折叠 hunk summary |
| Enter | OpenSelectedFileDiff | 已选择文件 | 加载文件 diff，然后进入 View 3 |
| v | OpenSelectedFileDiff | 已选择文件 | 等价于打开文件 diff |
| y | QueueCherryPickSelectedCommit | 已选择 commit | 加入 cherry-pick queue |
| Tab | FocusNext | 总是 | focus -> CommitList |
| Esc | Back | 总是 | screen -> BranchOverview |
| q | Quit | 总是 | 退出应用 |

### 7.6 View 2 状态转移

#### 7.6.1 切换 commit detail

```text
CommitDetail + CommitList + MoveUp/MoveDown
  -> selected_commit_index changed
  -> current_commit_detail remains old until Enter
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

1. 查看当前 commit 的文件列表。
2. 查看选中文件的完整 diff。
3. 在 unified 和 side-by-side 两种模式之间切换。
4. 在当前 commit 的多个文件之间快速切换。

### 8.2 布局

```text
┌────────────────────────────────────────────────────────────────────────────┐
│ repo=pitui | branch=main | viewing=feature/x | commit=3cc6d76 | file=a.rs │
├──────────────────────────────┬─────────────────────────────────────────────┤
│ Files                        │ Changes                                     │
│ > M CollisionStage.hlsl      │ @@ -70,10 +70,18 @@                         │
│   A parser.rs                │   const bool isAlive = ...                  │
│   D old_status.rs            │ - query.signed_distance = 0.0f;             │
│                              │ + query.uses_cached_contact = false;        │
│                              │ + query.signed_distance = 0.0f;             │
├──────────────────────────────┴─────────────────────────────────────────────┤
│ v mode | n next file | p prev file | w wrap | Tab focus | Esc back        │
└────────────────────────────────────────────────────────────────────────────┘
```

### 8.3 FocusPanel

View 3 允许两个 focus：

```text
FileList
DiffView
```

默认进入 View 3 时：

```text
screen = FileDiffDetail
focus = DiffView
```

### 8.4 FileList 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| ↑ / k | MoveUp | files 非空 | 选择上一个文件并加载 diff，focus 保持 FileList |
| ↓ / j | MoveDown | files 非空 | 选择下一个文件并加载 diff，focus 保持 FileList |
| n | NextFile | files 非空 | 选择下一个文件，并加载该文件 diff |
| p | PrevFile | files 非空 | 选择上一个文件，并加载该文件 diff |
| Enter | OpenSelectedFileDiff | 已选择文件 | 加载该文件 diff |
| v | ToggleDiffMode | 总是 | unified / side-by-side 切换 |
| w | ToggleWrap | 总是 | 开启 / 关闭换行 |
| Tab | FocusNext | 总是 | focus -> DiffView |
| Esc | Back | 总是 | screen -> CommitDetail |
| q | Quit | 总是 | 退出应用 |

### 8.5 DiffView 操作集

| Key | Action | 前置条件 | 结果 |
|---|---|---|---|
| ↑ / k | MoveUp | diff 已加载 | diff_scroll 上移 |
| ↓ / j | MoveDown | diff 已加载 | diff_scroll 下移 |
| PageUp | PageUp | diff 已加载 | diff 向上翻页 |
| PageDown | PageDown | diff 已加载 | diff 向下翻页 |
| Home | Home | diff 已加载 | 滚动到 diff 顶部 |
| End | End | diff 已加载 | 滚动到 diff 底部 |
| n | NextFile | files 非空 | 选择下一个文件，并加载 diff |
| p | PrevFile | files 非空 | 选择上一个文件，并加载 diff |
| v | ToggleDiffMode | 总是 | unified / side-by-side 切换 |
| w | ToggleWrap | 总是 | 开启 / 关闭换行 |
| Tab | FocusNext | 总是 | focus -> FileList |
| Esc | Back | 总是 | screen -> CommitDetail |
| q | Quit | 总是 | 退出应用 |

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
FileDiffDetail + n / p / MoveUp / MoveDown in FileList
  -> selected_file_index changed
  -> GitRequest::LoadFileDiff { commit, path }
  -> pending_jobs += job_id
  -> diff_scroll = 0

GitResponse::FileDiffLoaded(diff)
  -> current_file_diff = Some(diff)
  -> focus unchanged
```

`PendingJobKind::FileDiff` 携带 `focus_diff` intent：只有显式 `Enter/OpenSelectedFileDiff` 的响应可以把 focus 设为 `DiffView`；由 `↑/↓/Home/End/n/p` 触发的 diff refresh 必须保留响应到达时的 focus，异步 response 不得抢走左侧文件列表焦点。

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

### 9.2 Cherry-pick Queue

#### 触发

```text
y: add selected commit to queue
Y: open queue confirmation
```

#### y 状态转移

```text
Normal + y
  -> if selected commit not in queue:
       cherry_pick_queue.push(commit)
  -> screen unchanged
```

#### Y 状态转移

```text
Normal + Y + queue non-empty
  -> Confirming(CherryPickQueue)
```

#### 弹窗内容

```text
About to run:
git cherry-pick <commit1> <commit2> ...

Queue:
1. 3cc6d76 Fix Debug Layer error
2. 0acaf2e Fix color grading format

Enter confirm | Esc cancel
```

#### 确认状态转移

```text
Confirming(CherryPickQueue) + Enter
  -> GitRequest::CherryPick { commits }
  -> mode = Normal
  -> pending_jobs += job_id

GitResponse::CommandSucceeded
  -> cherry_pick_queue.clear()
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
fn map_normal(event: KeyEvent, app: &AppState) -> Option<Action> {
    match (app.screen, app.focus, event.code) {
        (_, _, KeyCode::Char('q')) => Some(Action::Quit),
        (_, _, KeyCode::Tab) => Some(Action::FocusNext),
        (_, _, KeyCode::Esc) => Some(Action::Back),
        (_, _, KeyCode::Char('r')) => Some(Action::RefreshRepository),
        _ => map_screen_specific(event, app),
    }
}
```

### 10.2 Filtering 模式

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

### 13.6 Fetch / Reflog / Reset / Rebase

```bash
git fetch --all --prune
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
viewing_branch
selected_commit
selected_file
operation
staged_count
unstaged_count
untracked_count
conflicted_count
ahead / behind
current_view
focused_panel
```

示例：

```text
repo=pitui | branch=main | viewing=feature/x | commit=3cc6d76 | file=a.rs | op=NORMAL | S=1 M=3 U=0 C=0 | ↑2 ↓0
```

### 14.3 底部 Hotkey Bar

Hotkey bar 根据 `screen + focus + mode + selection` 动态生成。

示例：

```text
BranchOverview + BranchList:
repo: Enter expand/collapse | f fetch | g reflog
branch: Enter view commits | s switch | b rebase

BranchOverview + CommitList:
Space select | c copy hashes | i copy info | m copy message | Enter detail | y queue | Y cherry-pick | R reset

CommitDetail + CommitFileList:
Space expand | Enter file diff | y queue | Esc back | q quit

FileDiffDetail + DiffView:
v mode | n next file | p prev file | m/Ctrl+Alt+C message | w wrap | Tab focus | Esc back

Reflog + ReflogList:
R reset | Esc back | q quit

Changes + ChangesTree / ChangesDiff:
Enter/←/→ expand/collapse | Tab focus | v mode | w wrap | ↑/↓ select or scroll | r refresh | Esc back

Global normal mode:
Ctrl+G changes（所有主 screen 都放在 hotkey bar 最前面）
```

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
    cherry_pick_queue: vec![],
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
status refresh interval: active repository 每 2s
branch list refresh: 手动 r、fetch、switch、reset 或 rebase 后
commit list refresh: 选择分支 Enter 或 r
commit detail refresh: 进入 View 2 或切换 commit 后 Enter
diff refresh: 进入 View 3 或切换文件后
```

Tick 负责：

```text
1. 拉取 GitResponse
2. 更新 pending_jobs
3. 定期刷新轻量 repo status
4. 驱动 loading indicator
```

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

### 9.4 Fetch

仅仓库节点显示 `f`。确认弹窗必须同时展示仓库名、绝对路径和准确命令：

```text
git fetch --all --prune
```

job 使用节点的 `repository_index` 解析 cwd；成功后只对该仓库执行 full refresh。

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

### 9.7 独立 Changes

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
4. root 和 group 支持 `Enter`、`←/h`、`→/l` 折叠/展开；文件节点 `Enter` 聚焦右侧。`Tab` 始终在 tree/diff 间切换。
5. 右侧复用 File Diff Detail 的 `render_diff_panel`、`unified_text`、`side_by_side_text`；`v` 切换模式，`w` 切换 wrap，终端宽度小于 140 时 side-by-side 自动降级。
6. rename/copy 显示 `old → new`；Git argv 始终使用原始 `GitPath` 字节。
7. 空文件或 binary 无 textual diff 时显示明确占位；快速选择文件产生的旧 response 由 latest job id 丢弃。
8. `PendingJobKind::Changes*` 携带 `repository_index` 和 diff group；离开 screen、切换仓库或快速切换文件后，过期 response 不得覆盖当前内容。
9. `Space` 在 file/group/root 上选择或反选对应节点；diff focus 下仍操作左侧当前文件。父节点用 `[ ]`、`[x]`、`[-]` 表示空选、全选和部分选择。
10. `s` 只 stage 已选的 Unstaged 节点，`u` 只 unstage 已选的 Staged 节点；没有显式选择时回退到当前同组文件。混合选择不会越过 group 边界。
11. `c` 只在至少一个 staged 文件存在时打开 commit message 弹窗；`Enter` 提交、`Esc` 取消，空消息留在弹窗并显示 validation error。

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
- 右侧 commit changed files
- 文件可展开 hunk summary
- Enter 进入 file diff
```

### Milestone 4: File Diff Detail

```text
- 左侧 files
- 右侧 unified diff
- v 切换 side-by-side
- n / p 切换文件
- Esc 返回
```

### Milestone 5: 可写操作

```text
- branch switch confirmation
- cherry-pick queue confirmation
- reset typed confirmation
- multi-repository tree + per-repository fetch/reflog
- hierarchical repository/branch connectors + unborn current branch child
- global Changes screen + three-level staged/unstaged/file tree
- shared unified/side-by-side diff component for commit and Changes patches
- file/group multi-select + file-level stage/unstage + commit message dialog
- multi-select commit hash + current info/full-message copy through OSC 52
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
