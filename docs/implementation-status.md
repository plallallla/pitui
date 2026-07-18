# Pitui 当前实现状态

更新日期：2026-07-18

## 结论

根包 `pitui` 是唯一正式运行时，使用 Data Driven Development + `bevy_ecs`。旧状态机、Controller、
后台 worker、兼容 facade 和对应测试已经从工作树删除。根 `src/` 负责组合内部 crate，不再承载另一套
领域模型或 Renderer。

## 当前运行时边界

```text
pitui-core    纯 Git 值类型与 Diff 算法
pitui-data    Dataset/Context/Operation/Render typed 数据
pitui-config  内置 Template/Proxy/Mode/Command/Operation 配置
pitui-git     parser、argv-only GitExecutor 与 JSONL 日志
pitui-ecs     World、固定 Schedule、Systems、Reconcile 与 Projection
pitui-tui     crossterm 输入、终端边界和只消费 UiFrame 的 Renderer
pitui         composition root、binary 与端到端验收
```

## 已实现

- Repository、Branch、Commits、Commit、CommitField、Files、FileTreeDirectory、FileChanges Dataset
  纵向链路；Commit 的 hash/author/date/tags/subject/message 是独立字段 Dataset，可 Active、多选并复制值。
- 稳定 `DatasetIdentity -> Entity` canonical index。
- Dataset DAG、显式 roots、Manager 生成的 Collection Element/depth、Active Element、selection、viewport 和
  reachability GC。
- Dataset Template 配置驱动的 Collection Manager：Repositories/Branches、Files、Changes 和
  WorkingTreeFiles 共用 `TreeManager`；其他 Dataset 默认使用 `ListManager`。Tree 的可见/可选类型、
  sibling order 和 selection mode 都是数据，结构行不会误入操作目标。
- Dataset Template 可声明多个 `DatasetViewSpec`；`DatasetViewState` 在相同 ownership DAG 上选择
  Collection Manager 与 Render Proxy。Files 默认 Tree View，可按 `v` 切换为只显示 File 后代的
  flat List View，再切回 Tree，期间不会重建或改写目录/文件实体关系。
- 单一 `ActiveUiContext`、`ActiveRenderMode`、`ResolvedOperationSet`。
- `pitui-tui` Input Listener 只解析终端事件；Operation Executor 从当前 Active Dataset 的
  `ResolvedOperationSet` 缓存查询绑定并构造调用上下文，未绑定输入直接忽略。
- `OperationManager` 维护直接的 `OperationId -> Bevy SystemId` 编译期函数表；Dataset Template 绑定
  Operation，并拥有独立的 `OperationHotkeyTable`。Operation 语义不再携带按键；Command 仅保留命令
  名称/作用域元数据，System 通过 typed ECS 参数修改 Active、全局状态或发出 Git/Context/Clipboard 等数据。
- Template/Proxy/Mode/Operation/Command/Availability 跨 Registry 启动校验。
- History、Commit、File Diff、Changes、Reflog、Git Operation Log Render Mode。
- Commit 下的 Files、Changes 的 staged/unstaged 边界和 WorkingTreeFiles 使用共享 `PathTree`
  Proxy：Snapshot 按 Git 原始路径构建真实目录 Dataset DAG，`TreeManager` 再稳定展平并生成深度；
  边界分组、文件与目录均保留 Dataset Active Element/selection 语义，目录的 diff 绑定到首个后代文件。
- unified 与 side-by-side diff projection；Commits 进入 Commit RenderMode 后依次把 Active 接力给
  Commit 详情和 Files；Files 向右先切换到 unified 模式并保持当前 File，
  再次向右才把 Active Dataset 接力给 FileChanges。
- 当前 Active Dataset 操作解析、WASD/方向接力、二级 copy chord、动态 footer/help/palette。
- Changes staged/unstaged 树、边界分组/目录/文件的父子级联多选、分组或目录递归 stage/unstage 和
  commit creation。
- Reflog 加载与 hash 复制。
- Commits/Reflog 当前条目的 soft、mixed、hard reset；hard reset 先进入默认 `Cancel` 的 Confirmation
  Context，确认层 Pop 后才释放内部写 Operation，随后刷新 Repository、Branches、Commits、Changes 和 Reflog。
- commits 多选和安全 cherry-pick；本次冲突自动尝试 abort。
- session Git operation log Dataset 与可轮转持久 JSONL sink。
- `UiFrame` generation 驱动重绘，不进行定时 Git 自动刷新。

## 尚未实现

- Remote 数据加载与管理。
- fetch、pull、push、sync。
- safe rebase。
- stash 浏览和操作。
- 外部 Hotkey 配置文件加载、严格覆盖和运行时 reload；当前每个 Dataset 的 Hotkey 表使用编译期默认值。
- 异步/后台 Git executor；当前同步执行可能阻塞 terminal event loop。
- 用户可操作的 unified/side-by-side 模式切换。
- Table Collection Manager；扩展位置已经收敛到 `CollectionManagerSpec`，本次不实现 Table。

`remotes/fetch/pull/push/sync` 已有稳定 Command/Operation ID，但当前系统明确返回 unimplemented；
safe rebase 尚未进入 `GitCommand` 数据协议。

## 固定数据流

```text
InputIntent
  -> query Active Dataset Operation Set cache
  -> OperationInvocation
  -> OperationManager[OperationId]
  -> registered ECS Operation System
  -> Dataset mutation / ContextTransitionRequest / GitCommandData
  -> GitResultData
  -> transactional Dataset snapshot application
  -> binding/layout/operation reconcile
  -> immutable UiFrame projection
  -> terminal presentation
```

Schedule 顺序固定为：

```text
Ingress -> Resolve -> Execute -> Reconcile -> Projection -> Present
```

终端呈现在 ECS Schedule 外消费 `UiFrame`；`Present` 目前作为顺序边界保留。

## 质量门禁

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --doc
```

真实 Git 写测试只能在 `tempfile` 仓库执行；网络操作完成前不得在测试中联系真实 remote。

## 后续优先级

1. 在不改变 Operation Manager 边界的前提下，将 `operation/systems.rs` 再按 interaction、active、changes、
   copy/scroll 拆为领域 System 模块。
2. 将 `git_runtime.rs` 按 lifecycle/log、snapshot planning 和 payload adapter 拆分。
3. 将根 `src/tests.rs` 拆为共享 fixture 与按语义分类的集成测试模块。
4. 设计保持 typed data 边界的异步 Git task/result 通道。
5. 依次实现 Remote、网络操作和 safe rebase，并补齐真实临时仓库测试。
6. 只为各 Dataset 的 `OperationHotkeyTable` 增加严格外部覆盖，不把语义功能、Renderer 或 Git argv
   暴露为运行时配置。
