# Pitui Next 开发状态

更新日期：2026-07-18

本文记录 Next 当前实现证据、已知缺口和下一步，不把局部纵向路径描述成完整 v1。

## 当前组合边界

```text
pitui-core    纯 Git 值类型与 Diff 算法
pitui-data    Dataset/Context/Operation/Render/Git 交互所需的 typed 数据
pitui-git     parser、安全 argv、同步 GitExecutor 与可轮转 JSONL Git 日志 sink
pitui-ecs     World、固定 Schedule、Git/Operation/Reconcile/Projection Systems
pitui-config  内置 Dataset Template、Operation、Proxy、Mode 与严格冲突审计数据
pitui-tui     crossterm 输入/终端边界和只消费 UiFrame 的 ratatui Renderer
pitui-next    不链接 Legacy AppState 的 composition root、可运行 binary 与真实 Git 验收
```

根包 `pitui` 仍作为 Legacy 行为基线。`pitui-next` 已经可以进入真实 alternate-screen TUI，
并且 Reflog、CommitCreation 与安全 cherry-pick 已有完整纵向路径；Remotes、Confirmation、
reset、safe rebase 和网络写操作仍未迁移，因此还不能接管默认产品入口。

## 当前专项目标审计

| 目标 | 当前结论 | 直接证据 |
|---|---|---|
| 可扩展 Dataset/Proxy/Operation 框架 | 已完成当前 kernel 范围 | `DatasetKind::ALL`、identity-kind 映射、Template/Proxy/Operation/Command/System 跨 Registry 启动校验 |
| Repositories + Branches | 按要求保持一个集合 Dataset 和共享 Proxy/Operation Set | `RepositoriesBranches` Template 拥有 tree Proxy 与导航集合；Repository/Branch 条目只提供 typed metadata/detail Proxy，不复制局部操作 |
| Commits/Commit | 已具备独立语义契约 | Commits compact+detailed Proxies、选择/copy/cherry-pick Operations；Commit detail Proxy、copy/scroll/层级 Operations |
| Files/File/FileChanges | 已具备独立语义契约 | list/detail/unified/side-by-side Proxies，以及导航、copy、selection、共享文本滚动 Operations |
| Reflog | 已完成纵向路径 | Repository-scoped Dataset、entry Dataset、同步 Git、事务 snapshot、list/detail Proxies、Command-only 入口、copy hash Operation |
| 创建提交 | 已完成纵向路径 | Repository-scoped CommitCreation Dataset、typed metadata、专用 editor Proxy、独占 help/cancel/submit Operations、真实 Git commit 验收 |
| cherry-pick 归属与实现 | 已明确并实现 | 只属于 Commits Operation Set；只读有序 Selection；oldest-to-newest argv；dirty/pre-existing-state preflight；本次冲突自动 abort |

`pitui-config` 的专项契约测试锁定上述归属；`pitui-next` 与 `pitui-git` 的真实临时仓库测试锁定
Reflog、CommitCreation 和 cherry-pick 的端到端数据流。该结论只表示本轮专项目标已实现，不表示
Next 的完整 v1 已经完成。

## 已有直接证据

### Dataset ECS Kernel

- `DatasetIdentity -> Entity` canonical index。
- 有序 `DatasetChildren` DAG、环检测、共享 Commit、reachability GC。
- `ActiveUiContext`、`ContextStack`、稳定 Render bindings 和成对恢复。
- Collection Dataset 是列表/树的 focus owner；上下移动只修改 Cursor。
- `DatasetNavigationOrder` 将事实所有权与逻辑行分离：
  - Repositories/Branches 暴露 Repository + Branch；
  - Changes 暴露 boundary group + working-tree file；
  - Commit/Files 等普通集合使用直接 children。
- Cursor/Selection 修复和详情 binding 更新不会隐式改变 Active Dataset。
- Left/Right 按逻辑层级移动，不回绕；跨 Mode 时 Push/Pop 完整 Context。
- `DatasetIdentity::kind()` 固定稳定身份的语义类型，调用者不能为同一 identity 任意选择 kind。
- `DatasetKind::ALL`、默认 Template、Render Proxy、Operation、Command、Availability、
  Command System 和 Render Mode 在进入 terminal 前执行跨 Registry 契约校验。
- 新增 Dataset kind 时若缺默认 Template、Proxy，或引用悬空/类型不匹配的 Proxy、Operation、
  Command System，会在 composition root 启动阶段失败，而不是等到渲染或按键时才暴露。

证据位于 `crates/pitui-ecs/src/lib.rs`、`binding_reconcile.rs` 及其测试。

### 同步 Git 与事务 Snapshot

- `GitCommandData -> GitExecutor -> GitResultData -> DatasetSnapshotPlan`。
- Repository、Branches、Commits、CommitDetail、FileDiff、WorkingTree、WorkingTreeDiff、Reflog
  已接入同步读取。
- snapshot 在写 World 前完成 identity/template/reference/cycle 校验。
- 失败保留最后成功的 children、metadata 和 revision。
- 同一 Commit 被多个 Branch 引用时复用同一 Entity。
- 从仓库子目录启动时，seed identity 会重绑定到真实 repository root。
- unborn 当前分支是合法 Repository/Branch 状态。
- stage、unstage、commit 使用 argv 和 pathspec，不经过 shell；unstage 不修改工作区内容。
- cherry-pick 使用显式 commit hash argv；执行前拒绝空选择、dirty worktree/index 与已有
  `CHERRY_PICK_HEAD`，只 abort 本次调用新建的冲突状态。
- 写操作后的 Repository/WorkingTree/Commits snapshot 在同一同步消息批次中刷新。

真实 Git 证据位于：

- `crates/pitui-git/tests/read_executor.rs`
- `crates/pitui-git/tests/write_executor.rs`
- `crates/pitui-ecs/tests/read_vertical.rs`
- `crates/pitui-next/src/lib.rs`

### Render 与 Terminal 数据链

- typed `RenderProxySpec`、`FieldSpec`、递归 `RenderLayout`、`RenderModeSpec` registries。
- History、Commit、FileDiff、Changes 的 unified/side-by-side Mode 均为数据，不是页面分支。
- Context/Stable bindings 在进入界面前解析成 `ResolvedRenderLayout`。
- Projection 生成 immutable `UiFrame`；Renderer 不接收 ECS World。
- Commit detailed Proxy 包含分钟精度日期、作者、非空 tag 和 subject。
- Commit/File/WorkingTree Diff 共用 unified/side-by-side 投影与渲染组件。
- Home/End/PageUp/PageDown 通过共享 `DatasetViewport` 覆盖详情和 Diff。
- TUI 对 Git/文件文本进行控制字符和 bidi 清理，并按 Unicode 显示宽度安全裁剪。
- terminal event 被转换成 `InputIntent`；Resize 保留在 terminal adapter 边界。
- OSC 52 clipboard、raw mode 和 alternate screen 使用 RAII。
- 只有 `UiFrame.generation` 改变或 terminal resize 才绘制；poll timeout 不刷新 Git、不重绘。

`pitui-next` 已在真实 PTY 中验证启动、Help、Command Palette 和退出；TestBackend 覆盖普通布局、
Diff、窄文本和保留下层内容的居中 Overlay。

### Operation、Command 与 Interaction Context

- typed Command、Operation、Availability、TargetSource、KeySequence registries。
- Global 与 Active Dataset Template 合并为唯一 `ResolvedOperationSet`。
- 同名 Command 采用 Global 优先；重复 KeySequence 和前缀歧义拒绝解析并保留上一有效集。
- Input key、chord 和零参数 Command Palette 最终统一产生 `CommandInvocation`。
- `CommandSystemId -> bevy SystemId` 是 World 内 callable jump table；Input 不持有函数。
- composition root 对每个 Command 引用的 `CommandSystemId` 做启动前可调用性校验；尚未迁移的
  Command 显式绑定拒绝 System，不以悬空函数引用伪装成已实现。
- Cursor/Selection 隐式目标由 resolver 生成，Selection 按 Dataset 业务顺序规范化。
- chord prefix 只修改数据；footer/help 随后只显示当前第二级有效集合。
- 默认数据包含 WASD + 方向键、`h`、`Ctrl+G`、`Ctrl+R`、`Ctrl+P`，并锁定
  `Ctrl+Space` 未绑定。
- Commits copy chord 只暴露 `h/i/m`；Files/Diff 只暴露 `n/a/r`。
- Help 快照当前有效快捷键；Command Palette 搜索并延迟执行同一份 Invocation。
- 全局 `InteractionContext` 支持 Help、CommandPalette、TextInput 和 Notice Overlay；Esc 成对恢复
  Active、Mode 与 bindings。
- Text edit intent 支持普通按键、Backspace、Paste、控制字符过滤、长度上限和内联校验错误；
  Interaction TextInput 与 CommitCreation 都只消费数据化的 edit intent。
- Git 执行/解析失败通过 typed `InteractionNoticeRequest` 进入 FIFO 队列；普通 Context 恢复后
  一次只显示一个独占 Notice，初次无缓存失败也不会在 UI 初始化前丢失。
- Notice 只显示稳定 operation name 与脱敏、限长的错误文本，不包含 commit message/URL argv。

证据位于：

- `crates/pitui-config/src/lib.rs`
- `crates/pitui-ecs/tests/operations.rs`
- `crates/pitui-next/src/lib.rs`

### Changes 写操作纵向路径

- World 中只有一个 `GlobalChanges` Dataset。
- 三级数据为 Changes -> Staged/Unstaged group -> WorkingTreeFile；MM 文件在两个 boundary
  下拥有不同稳定身份。
- 左侧 Changes Dataset 始终拥有 Cursor/Selection；右侧 Diff 通过 Context binding 复用它们。
- Space 可在左列或 Diff focus 下选择/反选，且只允许 WorkingTreeFile 成为选择目标。
- Stage/Unstage 在执行前验证当前 Repository、目标 Dataset 类型和 ChangeBoundary。
- 整文件 stage/unstage 后，语义 Cursor 会迁移到新 boundary 的同一路径。
- 从 Diff 执行 stage/unstage 时，Active Entity 会迁移到新 Diff，但键盘 focus 仍停留在右列。
- Commit 只在存在 staged file 时可用，先 Push 仓库级 `CommitCreationDataset`；该 Dataset
  固定 staged paths/revision，独立拥有 message、validation error、Render Proxy 与
  help/cancel/submit Operation Set，不再复用通用 TextInput purpose。
- 空消息或 staged revision 不一致时留在 CommitCreation 并写入其 typed validation error；
  Git commit 真正执行后恢复 Changes，失败再打开脱敏 Notice。
- Commit 后刷新 Repository、Branches、当前 Commits 和 Changes；若原 Diff 对应文件消失，
  focus 移到下一个剩余 Diff，没有剩余文件时安全回到 Changes，而不保留 stale Diff。
- 合法的 clean Changes snapshot 可以进入界面，右侧没有对象时保持空白而不是报错文案。
- v1 仍不支持 stash 和 partial hunk staging。

真实 Git 集成覆盖未跟踪文件、stage、unborn unstage、仅提交 staged snapshot、从 Diff 写操作、
最后一个文件提交以及 Context/focus 恢复。

### Reflog 与 cherry-pick 纵向路径

- `Reflog(RepositoryKey) -> ReflogEntry` 是 canonical Dataset 链；`LoadReflog` 先解析临时
  snapshot，再事务替换 children/metadata/revision。
- `reflog` 只通过零参数 Command 进入，没有默认进入快捷键；两栏 Mode 使用 Reflog list 与
  当前 entry detail Proxy，Cursor 更新右侧详情但不转移 Active Dataset。
- Reflog Operation Set 已支持导航与复制当前条目 hash；reset target 已在权威设计中定义为
  Reflog Cursor，但 reset/Confirmation 执行链尚未迁移。
- `commits.cherry-pick` 只挂载在 `CommitsDataset` 的局部 Operation Set，不属于 Global Set，
  没有 Selection 时 Availability 为 false，也不存在 queue mode/entity。
- cherry-pick 从当前 `CommitsDataset.Selection` 取目标，按 DatasetNavigationOrder 规范化后以
  oldest-to-newest 顺序执行，不能由用户按 Space 的先后顺序改变 argv。
- 冲突只在确认是本次调用创建的 `CHERRY_PICK_HEAD` 且存在 unresolved files 时自动 abort；
  预先存在的 cherry-pick 状态保持不动。成功、失败和 conflict-aborted 都写入 typed 日志。

真实 Git 验收覆盖多 commit 顺序回放、成功后的历史刷新、当前请求冲突自动 abort，以及已有
cherry-pick 状态不被 Pitui abort。

### GitOperationLog 数据链

- World 中只有一个 rooted `GlobalGitOperationLog` Dataset；每个同步 Git result 生成一个 typed
  `GitOperationLogEntryMetadata` Dataset。
- 条目包含 operation、Repository、UTC 开始时间、duration、success/failure/conflict-aborted、
  message、abort attempted/result；cherry-pick 冲突路径已经产生并验证 conflict-aborted 条目。
- 日志列表按最新优先排列；不在日志界面时新条目更新默认 Cursor，在界面内不会抢走用户 Cursor。
- `logs` Command Push 可配置的两栏 Mode：左侧 session 条目，右侧随 Cursor 更新 typed detail，
  上下移动不会改变 Active Dataset。
- 同一结果同时写入 session Dataset 和注入的 `GitOperationLogSink`。
- `JsonlGitOperationLogSink` 支持路径、level、单文件大小、保留文件数、启动轮转、buffer、flush
  interval 和 message 上限；URL token 被脱敏，运行期 I/O 失败不打断 Git/Dataset 更新。
- `NextApp::open` 使用平台默认持久化路径，也接受 `PITUI_GIT_LOG_PATH`；打开失败默认降级为
  Noop sink 并显示 Notice。`open_from` 测试边界默认不写用户目录。

## 已知缺口

- mutation 与刷新目前是同一个有序同步消息批次；后续安全状态机需要显式建模成功后的
  follow-up、部分成功和失败 Notice/Log。
- Interaction Context 尚未实现 Confirmation、选项移动和 pending invocation 恢复策略。
- Remotes 只有部分类型/解析资产，尚无完整纵向路径。
- 下一代日志配置目前只有 typed defaults/环境路径覆盖，尚未接入 strict TOML/effective-config；
  session Dataset 的容量策略也尚未配置化。
- reset 双确认、fetch/pull-rebase/push/sync、switch、safe rebase/auto-abort 尚未接入 Next
  runtime；当前只完成 cherry-pick 的安全 auto-abort。
- 下一代 strict TOML/effective-config 尚未取代当前内置 profile。
- Legacy 仍是默认 binary；发布、迁移和性能/阻塞时间验收未开始。

## Phase 对照

| Phase | 状态 | 尚缺的关键交付物 |
|---|---|---|
| Phase 0 | 大部分完成 | logging 尚未完整提取；Legacy 仍是默认 binary |
| Phase 1 | 接近完成 | 进一步收紧所有生产 Dataset 更新只能经过 Systems 的公开边界 |
| Phase 2 | 核心纵向链完成 | 补全全部验收边界、性能记录和更多失败/空仓库组合 |
| Phase 3 | 核心完成 | Confirmation 独占集合及尚未迁移 Command 的真实 Systems |
| Phase 4 | 核心完成 | strict 用户配置、样式配置和更完整的极窄布局验收 |
| Phase 5 | 进行中 | Changes、CommitCreation、Reflog、Notice、GitOperationLog 已有纵向路径；Remotes/Confirmation 未完成 |
| Phase 6 | 进行中 | stage/unstage/commit/cherry-pick 已迁移；其余写操作和安全状态机未完成 |
| Phase 7 | 未开始 | strict 配置迁移、Legacy 退出、发布验收 |

## 当前下一步

1. 完成 Confirmation Context、选项移动、pending invocation 和 hard-reset 二阶段输入基础。
2. 用同一套 Dataset/Operation/Projection 组件完成 Remotes 只读/管理纵向路径。
3. 把 mutation follow-up、sync 部分成功和 rebase/pull abort outcome 显式建模进 Git operation
   lifecycle；cherry-pick 的 conflict-aborted typed outcome 已可作为参考纵向路径。
4. 在上述基础上实现 reset 双确认、safe rebase、pull-rebase/sync 及冲突 auto-abort，全部使用
   真实临时仓库测试。

## 当前质量门禁

2026-07-17 已完整通过：

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --doc
```

本轮直接结果包括 Legacy 79 个单元测试、3 个配置 CLI 测试、36 个当前平台真实 Git 测试，
以及所有 Next workspace 单元、集成和文档测试。其中专项证据包括 8 个配置契约测试、
10 个 ECS kernel 测试、4 个 `pitui-git` writer 测试与 10 个 Next composition/真实仓库测试；Reflog、
CommitCreation、cherry-pick 顺序回放和冲突 abort 均在本次全量门禁内执行。
