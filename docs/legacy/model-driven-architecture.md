# Pitui Model-Driven Architecture

> 状态：**当前 0.1.0 Legacy 实现说明**。本文用于解释和审计现有代码资产，不再是下一代
> 开发目标。下一代使用独立 `bevy_ecs` crate 按 Dataset ECS 重新开发；当前实现证据、复用
> 边界与缺口见 [`../next-development-status.md`](../next-development-status.md)。历史产品设计中的 `Screen`、
> `FocusPanel`、`RepositoryState` 和详情缓存代码片段也不代表当前实现。

## 1. 结论

当前 0.1.0 Legacy 实现在自身边界内把 Git 数据、语义焦点、视图投影和可调用操作分成
四个独立层，主浏览链由以下模型驱动，而不是由页面枚举驱动。这里描述的是可复用的行为
与分层经验，不表示其 Model/View 核心等同于下一代 Dataset ECS：

```text
Repository -> Branch -> Commit -> File
```

其中 Commit 会被多个 Branch 共同引用，因此代码没有机械地复制成嵌套 `Vec`，而是以
typed ID 规范化存储。Changes、Reflog、Remotes 是 Repository facet；Diff 是 File 或
working-tree change 的展示资源，不伪装成新的 Git 实体。

用户提出的对应关系如下：

| 目标 | 当前实现 |
|---|---|
| Repository→Branch→Commit→File 核心结构 | `GitModel` + `RepositoryId/BranchId/CommitId/FileId` |
| focus 决定当前层级 | `FocusContext { kind, role, path }` |
| focus 决定左右栏内容 | `ViewProjection::from_focus` |
| 每个 focus 有不同操作集 | `OperationSpec.mount + contexts` |
| 操作可以绑定快捷键 | 全局与 `views.<view>.operations` resolved keymap |
| 操作可以从面板选择 | `Ctrl+P` 操作面板，仍执行同一 `OperationId` |
| 选择变化自动更新详情 | `DataRequirement` reconcile |

## 2. 数据流

```text
Terminal key / Ctrl+P palette
        -> OperationId
        -> OperationSpec::invoke(&AppState) -> Option<Action>
        -> Controller
        -> state/focus mutation + explicit safety state machine
        -> DataRequirement reconcile
        -> GitRequest

Git Worker
        -> GitResponse
        -> model reducer
        -> GitModel Resource<T>
        -> focus + requirement reconcile

GitModel + FocusContext + ResolvedViewConfig
        -> ViewProjection
        -> reusable panels
        -> Renderer(&AppState)
```

Input 不执行 Git，Renderer 不修改状态，只有 Git Worker 可以启动 `git` 进程。

## 3. 当前实现的 Model

`src/app/model.rs` 持有唯一核心 Git 数据：

```rust
struct GitModel {
    repository_order: Vec<RepositoryId>,
    repositories: HashMap<RepositoryId, RepositoryNode>,
}

struct RepositoryNode {
    id: RepositoryId,
    requested_path: PathBuf,
    summary: Option<Repository>,
    branch_order: Vec<BranchId>,
    branches: HashMap<BranchId, BranchNode>,
    commits: HashMap<CommitId, CommitNode>,
    working_tree: Resource<Vec<WorkingTreeChange>>,
    reflog: Resource<Vec<ReflogEntry>>,
    remotes: Resource<Vec<RemoteInfo>>,
}

struct BranchNode {
    id: BranchId,
    summary: Option<Branch>,
    commits: Resource<Vec<CommitId>>,
}

struct CommitNode {
    id: CommitId,
    summary: Commit,
    metadata: Resource<CommitMetadata>,
    file_order: Vec<FileId>,
    files: HashMap<FileId, FileNode>,
}

struct FileNode {
    id: FileId,
    summary: ChangedFile,
    diff: Resource<FileDiff>,
}
```

`Resource<T>` 明确区分 `NotLoaded`、`Loading`、`Ready(T)` 和 `Failed(String)`。加载、
空集合和错误因此是数据状态，不由 renderer 根据 job 字段猜测。过期响应会被丢弃；如果
它是某资源最后一个在途请求，Controller 会把孤立的 `Loading` 恢复为 `NotLoaded`，让
reconcile 可以重新提交请求。

`AppState` 不再保存 `branch_commits`、`current_commit_detail`、`current_file_diff`、
`reflog_entries`、`remotes` 或 `changes` 等第二份核心数据。`RepositoryUiState` 只保存展开、
错误、查看分支和异步 job 之类的 UI/调度状态。

## 4. Navigation 与 Focus

语义焦点、列表光标和显式多选是不同概念：

```rust
struct NavigationState {
    current: FocusContext,
    cursors: HashMap<CollectionId, EntityId>,
    history: Vec<FocusContext>,
}

struct FocusContext {
    kind: FocusKind,
    role: FocusRole,
    path: Option<FocusPath>,
}
```

- `FocusKind`：Repository、Branch、Commit、File、Diff、Reflog、Changes、ChangesDiff、Remote。
- `FocusRole`：`Collection`、`Entity`、`Content`，表达同一实体处于父栏、成为下一视图左栏，
  或进入详细内容。
- `FocusPath`：携带 typed target 与完整有效祖先；构造时拒绝跨仓库、target 不匹配和多余
  descendant。
- `path: None`：只用于 empty/loading collection。此时 `kind + role` 仍然足以解析视图和
  操作，不需要另一套 page state。
- `NavigationState.cursors`：按稳定 `EntityId` 保存光标身份；`SelectionState` 中的 index
  只是当前过滤/扁平列表的渲染坐标。
- commit 与 change 多选分别保存在独立 selection set，不等同于 focus。

`set_focus_layer` 是层级切换入口。弹窗、确认框、编辑器、帮助和操作面板属于
`GlobalMode`，不会篡改 domain focus；关闭后仍回到原语义焦点。

## 5. ViewProjection

`src/app/view.rs` 只根据 `FocusKind + FocusRole` 选择可复用 panel：

```text
Branch/Repository            -> RepositoryBranches | Commits
Commit(Collection)           -> RepositoryBranches | Commits
Commit(Entity/Content)       -> Commits             | Commit
File(Collection)             -> Commits             | Commit
File(Entity)                 -> Commit               | FileDiff
File(Content) / Diff         -> Commit               | FileDiff
Reflog                       -> Reflog               | ReflogDetail
Changes / ChangesDiff        -> Changes              | ChangesDiff
Remote                       -> Remotes              | RemoteDetail
```

因此向右钻取时，前一屏右栏会成为下一屏左栏；向左完全对称。视图不会反向写入 focus，
Renderer 只读取 `&AppState`、`ViewProjection`、model 查询结果和 resolved view config。
当前没有额外复制一份长期存活的 `ViewModel` cache。

## 6. OperationRegistry

`src/app/command.rs` 的 `OPERATION_SPECS` 是唯一 normal/chord 操作跳表：

```rust
type OperationHandler = fn(&AppState) -> Option<Action>;

struct OperationSpec {
    id: OperationId,
    name: &'static str,
    default_bindings: &'static [&'static str],
    default_label: &'static str,
    default_visible: bool,
    footer_group: FooterGroup,
    chord_group: Option<&'static str>,
    mount: OperationMount, // Global | Focus
    contexts: u16,         // FocusKind mask
    invoke: OperationHandler,
}
```

`OperationId` 使用 `#[repr(usize)]`，可直接索引跳表。Focus-mounted operation 先按当前
`FocusKind` 过滤，再调用 handler；`None` 表示此刻不可执行，`Some(Action)` 表示可执行。
输入解析、footer、`h` 帮助和 `Ctrl+P` 面板都读取同一 resolved operation set，面板不会
另造 Git 命令。确认、hard reset 双确认、safe rebase abort 和 remote URL policy 仍由
Controller/Worker 的安全状态机强制执行，不能被配置绕过。

Modal 拥有独立且独占的静态 callable table，因为文本编辑、确认和错误框不是 domain
focus；普通操作不会穿透 modal。

## 7. 配置驱动

有效配置是启动时完整校验后的 `Arc<ResolvedConfig>`。绑定继承顺序为：

```text
compiled OperationSpec defaults
  -> global [keybindings.commands]
  -> [views.<view>.operations]
  -> runtime focus mask + actionability
```

六个 View 当前可以独立配置左栏宽度、commit density、作者/日期/tag 字段和 operation
bindings。冲突校验按每个有效 View/Focus context 执行；footer、帮助和操作面板使用同一
view-effective keymap。

## 8. 声明式数据需求

`src/app/requirement.rs` 定义：

```text
BranchCommits(BranchId)
CommitDetail(CommitId)
FileDiff(FileId)
Reflog(RepositoryId)
WorkingTree(RepositoryId)
Remotes(RepositoryId)
```

每次 Action 和 GitResponse 处理完成后，Controller 都会 reconcile 当前 projection 的缺失
资源。方向键、WASD、Home/End、PageUp/PageDown 和异步响应不再各自维护一份“应该刷新
哪个右栏”的规则。显式 Enter/open 可以附带“钻入下一层”的 focus intent，但仍写入同一个
Model，并经过同一 stale-response 规则。

## 9. 状态所有权

| 状态 | 所有者 | 是否业务真值 |
|---|---|---|
| Repository/Branch/Commit/File 与仓库 facets | `GitModel` | 是 |
| 当前语义层级与实体路径 | `NavigationState.current` | 是 |
| 左右栏组合 | `ViewProjection` | 否，纯派生 |
| 操作与默认绑定 | `OPERATION_SPECS` | 是 |
| 有效用户配置 | `ResolvedConfig` | 是 |
| 过滤后列表 index、scroll、expand | `SelectionState/ExpansionState` | 否，UI 状态 |
| pending/latest job id | Controller/AppState | 否，异步调度状态 |
| popup/editor/help/palette | `GlobalMode` | 否，临时交互状态 |
| working-tree 当前 diff | AppState 临时预览资源 | 否，不属于 commit 文件层级 |

## 10. 架构不变量与审计

1. Core Git entity/collection 只存于 `GitModel`。
2. Child `FocusPath` 必须与 target 和 ancestor typed ID 完全一致。
3. `ViewProjection` 只由 focus 派生，不拥有导航状态。
4. Focus operation 不得挂载到未声明的 `FocusKind`。
5. 快捷键、footer、帮助和 palette 不维护第二份 operation 表。
6. GitResponse 先 reducer 到 Model，再 reconcile focus/requirements。
7. 过期 response 不得覆盖新选择，也不得留下孤立 `Loading`。
8. `std::process::Command::new("git")` 只能出现在 Git runner。

当前仍可继续演进但不阻塞上述结构的点：把少量异步 routing `usize` 全部替换为 typed
`RepositoryId`、为 working-tree diff 增加与 commit file diff 对称的 typed resource，以及
在需要更复杂 memoization 时引入短生命周期 ViewModel。它们都不能重新引入 Screen 状态机
或核心数据副本。
