# Pitui

> [!IMPORTANT]
> **Vibe Coding 声明：当前仓库中的全部代码、测试和文档均通过 vibe coding 生成；维护者负责提出需求、审阅结果并运行验证。**

Pitui 是一个使用 Rust、`bevy_ecs`、ratatui 和 Git CLI 实现的 Data Driven Git TUI。
仓库只有一套正式运行时：根包 `pitui`。所有界面状态、焦点、操作、Git 请求与渲染结果都通过
类型化数据表达。

## 当前能力

- 同时打开一个或多个本地 Git 仓库。
- 以仓库/分支树浏览分支，并查看对应 commit 列表。
- 查看 commit 作者、时间、tag、message、changed files 和文件 diff。
- 查看 staged/unstaged changes，支持文件多选、stage、unstage 和创建 commit。
- 查看 reflog，并复制 reflog hash。
- 多选 commits 后安全执行 cherry-pick；冲突时自动尝试 abort。
- 复制 commit hash/info/message，以及文件名、绝对路径和仓库相对路径。
- 查看会话 Git 操作日志，并可持久化为自动轮转的 JSONL 日志。
- 快捷键、底部提示、Help 和 Command Palette 共用当前唯一有效 Operation Set。
- 只在数据发生变化或终端 resize 时重绘；Git 数据使用 `Ctrl+R` 手动刷新。

当前尚未实现 remote 管理、fetch、pull、push、sync、reset 和 rebase。相关未实现命令会明确拒绝，
不会静默执行。外部 TOML 配置也尚未接入，当前使用 `pitui-config` 提供的严格内置配置数据。

## 运行

要求：

- Rust 1.95 或更新版本
- `git` 可从 `PATH` 调用

```bash
cargo run -- /path/to/repository
```

同时打开多个仓库：

```bash
cargo run -- /repo/one /repo/two
```

不传路径时默认打开当前目录。

## 主要快捷键

底部只显示当前焦点下真正可执行的快捷键；按 `h` 可查看当前上下文帮助。

| 快捷键 | 操作 |
|---|---|
| `W/A/S/D` 或方向键 | 上/左/下/右导航 |
| `Space` | 选择或反选当前条目 |
| `Esc` | 返回或关闭交互层 |
| `h` | 当前上下文帮助 |
| `Ctrl+P` | Command Palette |
| `Ctrl+R` | 手动刷新当前上下文 |
| `Ctrl+G` | 打开 Changes |
| `Ctrl+C`, `h/i/m` | 复制 commit hash/info/message |
| `Ctrl+C`, `n/a/r` | 复制文件名/绝对路径/相对路径 |
| `Shift+S` | Stage 选中的 unstaged 文件 |
| `Shift+U` | Unstage 选中的 staged 文件 |
| `Shift+C` | 使用 staged 文件创建 commit |
| `Home/End/PageUp/PageDown` | 详细内容和 diff 滚动 |
| `q` | 退出 |

二级快捷键只会在输入第一级后展示第二级提示。

## Data Driven + ECS 架构

```text
Terminal Event
    -> InputIntent
    -> ResolvedOperationSet
    -> CommandInvocation
    -> ECS Command System
         -> Dataset/Cursor/Selection/ContextTransition
         -> GitCommandData -> GitExecutor -> GitResultData
    -> Reconcile bindings/layout/operations
    -> immutable UiFrame
    -> ratatui Renderer
```

核心约束：

- `DatasetIdentity` 是稳定业务身份，Bevy `Entity` 只是运行时句柄。
- `ActiveUiContext.active_dataset` 是唯一语义焦点。
- 同一时间只有一个 `ActiveRenderMode` 和一个 `ResolvedOperationSet`。
- Dataset Template 声明某类数据可用的 Render Proxy 与 Operation。
- Renderer 只能读取 `UiFrame`，不能访问或修改 ECS `World`。
- Git effect 只能通过类型化 `GitCommand` 进入 `pitui-git`，不拼接 shell 命令字符串。
- Help、footer、输入响应和 Command Palette 读取同一份已解析操作数据。

## 源码布局

```text
src/                    pitui binary、composition root、端到端测试
crates/pitui-core/      纯 Git 值类型与 diff 算法
crates/pitui-data/      Dataset、Context、Operation、Render 数据协议
crates/pitui-config/    Template、Proxy、Mode、Command 和快捷键配置数据
crates/pitui-git/       Git executor、parser 与 JSONL 日志
crates/pitui-ecs/       World、Schedule、Systems、Reconcile 与 Projection
crates/pitui-tui/       crossterm/ratatui 终端适配器
docs/                   当前实现状态和代码资产说明
```

详细说明见：

- [`docs/implementation-status.md`](docs/implementation-status.md)
- [`docs/code-assets.md`](docs/code-assets.md)

## 安全边界

- Git 命令使用 argv 调用，不通过 shell 执行。
- Git 错误和日志进入 UI/持久化前会截断并隐藏 URL 类敏感内容。
- cherry-pick 前检查工作区状态；发生本次冲突时自动尝试 `git cherry-pick --abort`。
- stage/unstage 只作用于当前光标或显式选择的文件。
- 所有真实 Git 写操作测试只使用 `tempfile` 创建的临时仓库。

## 开发验证

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --doc
```

## License

Pitui 使用 [MIT License](LICENSE)。
