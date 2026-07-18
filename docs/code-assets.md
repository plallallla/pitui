# Pitui 代码资产清单

更新日期：2026-07-18

## 1. Workspace 边界

| 路径 | 职责 | 允许依赖 |
|---|---|---|
| `src/` | 正式 `pitui` composition root、binary 与端到端测试 | 所有内部边界 crate |
| `crates/pitui-core/` | 纯 Git 值类型与 Diff 算法 | 标准库 |
| `crates/pitui-data/` | Dataset、Context、Operation、Render 的 ECS 数据协议 | `pitui-core`、`bevy_ecs` |
| `crates/pitui-config/` | 内置 Template、Proxy、Mode、Command、Operation 和日志配置 | `pitui-data` |
| `crates/pitui-git/` | Git parser、typed executor 与持久日志 | `pitui-core` |
| `crates/pitui-ecs/` | Dataset runtime、Schedule、Systems、Reconcile、Projection | core/data/git、`bevy_ecs` |
| `crates/pitui-tui/` | terminal/input/render adapter | core/data、crossterm、ratatui |

依赖方向：

```text
pitui
├── pitui-config -> pitui-data -> pitui-core
├── pitui-ecs ----> pitui-data + pitui-git + pitui-core
└── pitui-tui ----> pitui-data + pitui-core

pitui-git -> pitui-core
```

## 2. 根源码

| 文件 | 职责 |
|---|---|
| `src/main.rs` | `pitui` binary 入口、参数路径解析调用和错误退出码 |
| `src/lib.rs` | `App` composition root、Registry/System 组装、初始 Git 数据加载和 terminal loop |
| `src/tests.rs` | composition、交互、真实临时仓库与写操作端到端测试 |

## 3. 内部 crate 文件

### `pitui-core`

| 文件 | 职责 |
|---|---|
| `src/model.rs` | Repository/Branch/Commit/File/Reflog/WorkingTree 等值类型 |
| `src/diff.rs` | unified 数据与 side-by-side 对齐算法 |
| `src/lib.rs` | 公共导出 |

### `pitui-data`

| 文件 | 职责 |
|---|---|
| `src/identity.rs` | 稳定 `DatasetIdentity`、`DatasetKind`、CommitFieldKind 和 RepositoryKey |
| `src/dataset.rs` | Dataset Bundle、DAG、Collection Element/depth、Active Element、selection、View state、viewport 和 index components |
| `src/metadata.rs` | 每种语义 Dataset 的 typed metadata components，包括 CommitField 值和文件树目录路径 |
| `src/template.rs` | Template、Dataset View、List/Tree Collection Manager 规格及稳定 ID/Registry；保留未来 Table 扩展边界 |
| `src/context.rs` | Active Context、render bindings、context stack、overlay/text/help 数据 |
| `src/operation.rs` | 快捷键、Operation、Command、可用性、Invocation 和 Clipboard 数据 |
| `src/render.rs` | Render Proxy/Mode/Layout、PathTree 行协议与不可变 `UiFrame` projection 数据 |
| `src/lib.rs` | 公共导出 |

### `pitui-config`

| 文件 | 职责 |
|---|---|
| `src/lib.rs` | 所有内置 Dataset Template、Proxy、Mode、Operation、Command 与日志默认值 |
| `src/tests.rs` | 跨 Registry 严格契约、绑定冲突和语义归属测试 |

### `pitui-git`

| 文件 | 职责 |
|---|---|
| `src/lib.rs` | `GitCommand`、`GitExecutor`、CLI argv 执行和安全写操作 |
| `src/parser.rs` | Git 字节输出到 `pitui-core` 数据的唯一 parser |
| `src/logging.rs` | JSONL operation log、滚动、flush、截断和脱敏 |
| `tests/read_executor.rs` | 真实临时仓库读取测试 |
| `tests/write_executor.rs` | stage/unstage/commit/cherry-pick 与冲突 abort 测试 |

### `pitui-ecs`

| 文件 | 职责 |
|---|---|
| `src/lib.rs` | World/Schedule、注册契约、Dataset 生命周期、DAG、GC 和不变量 |
| `src/collection.rs` | 通用 List/Tree Manager：直接/后代来源、可见节点过滤、排序、展平、深度和父子级联选择 |
| `src/operation_runtime.rs` | 输入解析、唯一 Operation Set、Command System 和 Context 操作 |
| `src/git_runtime.rs` | Git messages、执行结果、事务 Snapshot plan 和 session log Dataset |
| `src/binding_reconcile.rs` | Active handoff、Context transition、render bindings 与 layout reconcile |
| `src/projection.rs` | ECS World 到不可变 `UiFrame` 的单向投影，消费 Manager 生成的 Collection Element 数据 |
| `src/tests.rs` | crate 私有内核测试 |
| `tests/read_vertical.rs` | Repository→Branch→Commit→File 读取纵向测试 |
| `tests/operations.rs` | Operation resolution、target 和 command tests |
| `tests/projection.rs` | Proxy、layout、footer、diff projection tests |

### `pitui-tui`

| 文件 | 职责 |
|---|---|
| `src/input.rs` | crossterm event → `InputIntent` |
| `src/render.rs` | 只消费 `UiFrame` 的 ratatui renderer |
| `src/terminal.rs` | raw mode、alternate screen、OSC 52 与 RAII 清理 |
| `src/lib.rs` | adapter 公共出口 |

## 4. 删除和保留规则

- 当前不存在第二套运行时、兼容 facade 或重复 Git parser。
- 当前不保留设计图片、临时截图、辅助理解草稿和已失效配置示例。
- `target/` 是忽略的构建缓存，不属于源码资产。
- 删除任何 Dataset/Proxy/Operation 前，必须先确认 Registry、Render Mode、真实 Git 测试和文档均无引用。
