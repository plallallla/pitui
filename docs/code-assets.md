# Pitui 代码资产清单

更新日期：2026-07-18

本文回答三个问题：每个代码文件负责什么、为什么保留、何时可以删除。当前仓库同时保存
0.1.0 Legacy 行为基线和下一代 Dataset ECS；在 Next 完成完整 v1 验收以前，Legacy 不是死代码。

## 1. 资产分区

| 分区 | 状态 | 作用 |
|---|---|---|
| 根包 `pitui` / `src/` | Legacy，仍是默认 binary | 已完整实现的产品行为、安全策略和真实 Git oracle |
| `crates/pitui-core` | 共享、长期保留 | 纯 Git 值类型与 Diff 算法 |
| `crates/pitui-data` | Next 核心、长期保留 | typed Dataset、Context、Operation、Render 数据 |
| `crates/pitui-git` | 共享、长期保留 | argv-only 同步 Git executor、parser、持久日志 |
| `crates/pitui-ecs` | Next 核心、长期保留 | World、Systems、Schedule、事务 Snapshot 与 Projection |
| `crates/pitui-config` | Next 配置边界、长期保留 | 内置 Template/Proxy/Mode/Operation 数据与严格契约 |
| `crates/pitui-tui` | Next 边界、长期保留 | terminal/input adapter 与只消费 `UiFrame` 的 Renderer |
| `crates/pitui-next` | Next composition root | 新 binary、Registry 组装和端到端验收 |
| `tests/` | Legacy oracle | 默认 binary 的配置和真实 Git 行为测试 |

依赖方向固定为：

```text
pitui-core <- pitui-data <- pitui-ecs
pitui-core <- pitui-git  <- pitui-ecs
pitui-data <- pitui-config
pitui-data <- pitui-tui

pitui-next 只负责组装，不把 World、Git CLI 或 terminal 反向泄漏到下层 crate。
```

## 2. 根包与 Legacy 文件

这些文件目前全部可达并参与默认 `pitui` binary 或其测试，不能仅因为 Next 已存在就删除。

| 文件 | 职责 | 处理 |
|---|---|---|
| `src/main.rs` | 默认 `pitui` binary 入口与退出码 | 保留到 Next 接管默认 binary |
| `src/lib.rs` | Legacy composition API、仓库路径解析 | 保留 |
| `src/config.rs` | 已投入使用的严格 TOML/effective config | 保留并作为 Next 配置迁移 oracle |
| `src/domain/mod.rs` | 对 `pitui-core` 的兼容 facade | 保留到 Legacy 退出 |
| `src/app/mod.rs` | Legacy app 模块出口 | 保留 |
| `src/app/action.rs` | Legacy typed `Action` | 保留 |
| `src/app/command.rs` | Legacy Operation jump table、快捷键/help/footer 共同来源 | 保留为行为 oracle |
| `src/app/controller.rs` | Legacy reducer、Git effect 与安全状态机 | 保留；Next 写操作迁移的主要 oracle |
| `src/app/focus.rs` | Legacy semantic focus/path | 保留 |
| `src/app/model.rs` | Legacy normalized GitModel | 保留 |
| `src/app/requirement.rs` | Legacy 声明式数据依赖 | 保留 |
| `src/app/state.rs` | Legacy session/UI state | 保留 |
| `src/app/view.rs` | Legacy focus → view projection | 保留 |
| `src/git/mod.rs` | Legacy Git API facade | 保留 |
| `src/git/parser.rs` | 对 `pitui-git::parser` 的兼容 facade | 保留，避免 parser 双实现 |
| `src/git/protocol.rs` | Legacy worker request/response 协议 | 保留到异步 Legacy 退出 |
| `src/git/runner.rs` | 尚未全部迁入 Next 的 remote/reset/rebase Git 语义 | 保留 |
| `src/git/worker.rs` | Legacy 后台 worker/channel | 保留；Next v1 明确同步执行，不复用运行时 |
| `src/git/logging.rs` | Legacy job 生命周期日志 | 保留到日志迁移完成 |
| `src/tui/mod.rs` | Legacy terminal session 和主循环 | 保留 |
| `src/tui/input.rs` | Legacy modal/focus 输入适配 | 保留为交互 oracle |
| `src/tui/render.rs` | Legacy完整产品 Renderer | 保留到 Next 达成视觉验收 |
| `tests/config_cli.rs` | Legacy 配置 CLI 集成测试 | 保留 |
| `tests/git_integration.rs` | reset/remote/rebase/pull/push 等真实 Git oracle | 保留，迁移完成前不能删 |

## 3. 共享与 Next 文件

### `pitui-core`

| 文件 | 职责 |
|---|---|
| `crates/pitui-core/src/lib.rs` | 纯数据 crate 出口与依赖边界声明 |
| `crates/pitui-core/src/model.rs` | Repository/Branch/Commit/File/Reflog/Remote 值类型 |
| `crates/pitui-core/src/diff.rs` | unified/side-by-side Diff 数据与对齐算法 |

### `pitui-data`

| 文件 | 职责 |
|---|---|
| `crates/pitui-data/src/lib.rs` | typed ECS data 出口 |
| `crates/pitui-data/src/identity.rs` | `DatasetIdentity`、`DatasetKind`、RepositoryKey |
| `crates/pitui-data/src/dataset.rs` | Dataset Components、Bundle、Index、Roots |
| `crates/pitui-data/src/metadata.rs` | 各语义 Dataset 的 typed metadata |
| `crates/pitui-data/src/context.rs` | Active Context、bindings、overlay/text/notice 数据 |
| `crates/pitui-data/src/operation.rs` | Command/Operation/Key/Availability/Invocation 数据 |
| `crates/pitui-data/src/render.rs` | Proxy、Field、Mode、UiFrame/Projection 数据 |
| `crates/pitui-data/src/template.rs` | 稳定 ID、Template Registry 与默认模板映射 |

### `pitui-git`

| 文件 | 职责 |
|---|---|
| `crates/pitui-git/src/lib.rs` | 同步 `GitExecutor`、argv、安全写操作和 typed payload |
| `crates/pitui-git/src/parser.rs` | Git 字节输出解析；Legacy 也复用此唯一实现 |
| `crates/pitui-git/src/logging.rs` | 脱敏、限长、轮转 JSONL sink |
| `crates/pitui-git/tests/read_executor.rs` | 真实仓库只读纵向链 |
| `crates/pitui-git/tests/write_executor.rs` | stage/unstage/commit/cherry-pick 安全语义 |

### `pitui-ecs`

| 文件 | 职责 |
|---|---|
| `crates/pitui-ecs/src/lib.rs` | Kernel、Schedule、Registry 组合、公开 runtime 边界 |
| `crates/pitui-ecs/src/binding_reconcile.rs` | Cursor → 依赖 binding、Mode、Context 的一致性修复 |
| `crates/pitui-ecs/src/git_runtime.rs` | Git messages、事务 Snapshot plan、typed log Dataset |
| `crates/pitui-ecs/src/operation_runtime.rs` | Input resolution、Command systems、Changes/Commit/Reflog 操作 |
| `crates/pitui-ecs/src/projection.rs` | Dataset + Proxy → immutable `UiFrame` |
| `crates/pitui-ecs/src/tests.rs` | Kernel 私有边界单元测试；已与生产 `lib.rs` 分离 |
| `crates/pitui-ecs/tests/operations.rs` | Operation/Command/chord 集成测试 |
| `crates/pitui-ecs/tests/projection.rs` | Projection、Diff 和 viewport 集成测试 |
| `crates/pitui-ecs/tests/read_vertical.rs` | 真实 Git snapshot 与缓存事务测试 |

### `pitui-config`

| 文件 | 职责 |
|---|---|
| `crates/pitui-config/src/lib.rs` | built-in Template/Proxy/Mode/Operation/Logging profile |
| `crates/pitui-config/src/tests.rs` | 全 kind 覆盖、引用解析、快捷键冲突和专项契约测试 |

### `pitui-tui`

| 文件 | 职责 |
|---|---|
| `crates/pitui-tui/src/lib.rs` | TUI adapter 出口 |
| `crates/pitui-tui/src/input.rs` | crossterm event → `InputIntent` |
| `crates/pitui-tui/src/render.rs` | 只读取 `UiFrame` 的 ratatui Renderer |
| `crates/pitui-tui/src/terminal.rs` | raw mode、alternate screen、OSC 52 与 RAII |

### `pitui-next`

| 文件 | 职责 |
|---|---|
| `crates/pitui-next/src/main.rs` | `pitui-next` binary 入口 |
| `crates/pitui-next/src/lib.rs` | composition root、Registry/System 组装和 terminal loop |
| `crates/pitui-next/src/tests.rs` | Next 私有端到端测试；已与生产 `lib.rs` 分离 |

所有 `Cargo.toml` 和根 `Cargo.lock` 都属于实际 workspace 构建图，没有发现未使用的 workspace
member。各 crate 的依赖方向符合当前 workspace 分层，没有把完整 Bevy、Git CLI 或 ECS World
泄漏到不应依赖它们的生产边界。

## 4. 文档与 GitHub 资产

- `docs/next-development-status.md`：当前 Next 实现证据、缺口与质量门禁。
- `docs/code-assets.md`：代码职责、保留判断与删除条件。
- `docs/legacy/`：Legacy 行为、配置和验收资料；只作 oracle，不作 Next API。
- `.github/workflows/ci.yml`：必须使用 `--workspace` 覆盖 Legacy 与所有 Next crates。
- `.github/ISSUE_TEMPLATE/`、PR 模板、Dependabot、`LICENSE`、`SECURITY.md`、
  `CONTRIBUTING.md`：仍是有效发布/协作资产。

当前仓库不保留设计图片、临时截图或仅用于辅助理解的设计草稿。`target/` 是 `.gitignore`
管理的构建缓存，不是版本库代码资产，也不作为源码清理对象。

## 5. Legacy 最终删除条件

只有同时满足以下条件，才可以删除根 `src/app`、Legacy TUI/worker/config 和对应测试：

1. Next 完成 Confirmation、reset、Remotes、fetch/pull/push/sync、switch、safe rebase。
2. 每项 Legacy 真实 Git 测试都已有 Next 对应测试，而不是仅有相似单元测试。
3. strict effective config 和日志行为完成迁移。
4. `pitui-next` 完整验收后接管默认 `pitui` binary。
5. Linux/macOS/Windows 的 workspace CI 全部通过。

在这之前，Legacy 应冻结功能、只接受安全修复，避免两套运行时继续平行演进。

## 6. 后续文件拆分优先级

以下文件都在使用，不能删除，但继续扩展前应按语义拆分：

1. `crates/pitui-ecs/src/operation_runtime.rs`：拆为 input/resolution、navigation、changes、
   commit-creation、reflog、clipboard/viewport。
2. `crates/pitui-ecs/src/git_runtime.rs`：拆为 lifecycle/log、snapshot plan 和各 payload adapter。
3. `crates/pitui-config/src/lib.rs`：拆为 templates、proxies/modes、commands/operations、logging。
4. `crates/pitui-next/src/tests.rs`：将超长 composition 场景拆成共享 fixture 与语义验收模块。

拆分必须保持 crate 依赖方向和现有测试语义，不使用 `include!` 或重新引入页面式 Controller。
