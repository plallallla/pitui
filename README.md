# Pitui

> [!IMPORTANT]
> **Vibe Coding 声明：当前仓库中的全部代码、测试和文档均通过 vibe coding 生成；维护者负责提出需求、审阅结果并运行验证。**

Pitui 是一个安全、克制、支持多仓库的 Git TUI，以分支、commit 和 diff 浏览为核心。

```text
Repositories + Branches -> Commits -> Commit + Changed Files -> Files + Diff
          |
          +-------------> Reflog

Any main view -- Ctrl+G --> Changes -> Staged / Unstaged -> Files + Diff
```

它允许你在**不切换当前工作分支**的情况下浏览任意本地或远程分支的提交；真正修改仓库的操作始终经过确认。

## 功能

- 浏览本地与远程分支。
- 同时打开多个仓库，并以“仓库 → 分支”的树形结构展示。
- 在仓库节点确认后执行 `git fetch --all --prune`。
- 按仓库浏览最近 300 条 reflog，并从 commit 或 reflog 条目选择 reset 目标。
- 独立 Changes 界面使用“Changes → Staged/Unstaged → 文件”三级树展示当前修改。
- Changes 与 commit file diff 复用 unified / side-by-side diff 组件。
- Changes 支持文件/分组多选，可在文件树或 diff 焦点下 stage、unstage，并通过消息弹窗创建 commit。
- Git worker 的每个后台 job 都写入持久化 JSONL 操作日志，包含排队、开始、完成、耗时与失败结果。
- 不执行 checkout/switch 即可查看任意分支最近 300 个 commits。
- 查看 commit 元信息、文件状态、增删行数和 hunk summary。
- 查看单文件 unified diff。
- 在宽度不小于 140 列时使用 side-by-side diff。
- 分支和 commit 过滤。
- commit 支持多选，并复制一个或多个完整 commit hash；也可单独复制当前 commit info 或完整 message。
- cherry-pick queue。
- 分支切换确认。
- `reset --soft` / `--mixed` / `--hard`；hard reset 采用警告确认 + 短哈希输入的双重确认。
- 安全 rebase：只允许干净工作区确认执行；发生冲突时自动执行 `git rebase --abort` 并提示结果。
- detached HEAD、unborn repository、rename、root commit、binary file 和非 UTF-8 Git 路径处理。
- staged、unstaged、untracked、conflicted、ahead/behind 状态展示。

## 环境要求

- Rust toolchain，支持 Rust 2024 edition。
- Git 可在 `PATH` 中调用。
- 支持 ANSI alternate screen 的终端；复制功能使用 OSC 52，终端需允许应用写入剪贴板。

## 构建与运行

在项目目录中构建：

```bash
cargo build --release
```

不带参数时浏览当前 Git 仓库：

```bash
/path/to/pitui/target/release/pitui
```

也可以同时指定一个或多个仓库（相对路径按当前目录解析）：

```bash
/path/to/pitui/target/release/pitui /repo/frontend /repo/backend /repo/tools
```

开发时也可以直接指定 manifest：

```bash
cargo run --manifest-path /path/to/pitui/Cargo.toml -- /repo/frontend /repo/backend
```

## 后台操作日志

Pitui 会记录所有提交到 Git worker 的后台操作。每个 job 都至少包含 `queued`、`started`、`completed` 三条 JSONL 记录，以及 `job_id`、仓库 `cwd`、操作名、结果、耗时和错误摘要。后台 channel 意外断开也会写入错误记录。

默认日志位置：

| 平台 | 路径 |
|---|---|
| macOS | `~/Library/Logs/pitui/pitui.jsonl` |
| Linux / Unix | `${XDG_STATE_HOME:-~/.local/state}/pitui/pitui.jsonl` |
| Windows | `%LOCALAPPDATA%\pitui\pitui.jsonl` |

可使用环境变量覆盖路径：

```bash
PITUI_LOG=/path/to/pitui.jsonl pitui /repo
```

日志达到 5 MiB 时轮转为 `pitui.jsonl.1`，保留当前文件和一份备份。若默认位置无法创建，Pitui 会回退到临时目录并在状态栏显示实际路径；`pitui --help` 也会输出当前默认路径。

日志记录操作元数据、仓库路径、所选文件路径和 Git 错误，因此分享前应检查并脱敏。为避免无意泄露，diff/文件内容不会写入日志，commit message 只记录字节数，详情和错误字段最长保留 4096 个字符。

## 快捷键

### 全局

| Key | 操作 |
|---|---|
| `q` | 普通模式退出；普通确认/错误弹窗中取消或关闭；哈希输入弹窗中作为输入字符 |
| `Ctrl-G` | 从任意主界面打开独立 Changes；再次按下或按 `Esc` 返回原界面和原焦点 |
| `Ctrl-C` | commit 上下文中复制已多选的完整 hashes（没有多选时复制当前 hash）；其他上下文退出 |
| `Ctrl-Shift-C` | commit 上下文中复制当前 commit info |
| `Ctrl-Alt-C` | commit 上下文中复制当前完整 commit message |
| `Tab` / `Shift-Tab` | 切换面板焦点 |
| `↑` / `k`, `↓` / `j` | 移动选择或滚动 diff |
| `PageUp`, `PageDown`, `Home`, `End` | 翻页或跳转 |
| `Esc` | 返回上一视图或取消弹窗 |
| `r` | 刷新全部仓库、分支以及当前 commit/reflog/Changes 视图 |

### Branch / Commit Overview

| 焦点 | Key | 操作 |
|---|---|---|
| 仓库节点 | `Enter` | 展开/折叠该仓库的分支 |
| 仓库节点 | `f` | 确认后在该仓库执行 `git fetch --all --prune` |
| 仓库节点 | `g` | 查看该仓库 reflog |
| 分支节点 | `Enter` | 浏览所选分支 commits，不切换真实分支 |
| 分支节点 | `s` | 确认后在所属仓库执行 `git switch` |
| 分支节点 | `b` | 确认后将当前分支 rebase 到所选分支；冲突自动 abort |
| 仓库/分支树 | `/` | 按仓库名、路径、分支或 subject 过滤 |
| Commits | `Enter` | 打开 commit detail |
| Commits | `Space` | 加入/移出 commit 复制多选集合 |
| Commits | `c` / `Ctrl-C` | 按当前列表顺序复制所有多选 commit 的完整 hash；无多选时复制当前 hash |
| Commits | `i` / `Ctrl-Shift-C` | 复制当前 commit info（hash、author、date、refs 和 message） |
| Commits | `m` / `Ctrl-Alt-C` | 复制当前完整 commit message；需要时后台加载 body，但不改变 screen/focus |
| Commits | `/` | 过滤 commits |
| Commits | `y` | 将 commit 加入 cherry-pick queue |
| Commits | `Y` | 打开 queue 确认弹窗 |
| Commits | `R` | 选择 soft/mixed/hard reset 模式并确认 |

### Reflog

| Key | 操作 |
|---|---|
| `↑` / `k`, `↓` / `j` | 选择 reflog 条目 |
| `R` | 以该条目的 commit 为目标打开 reset 模式选择 |
| `Esc` | 返回仓库/分支树 |

Reset 模式弹窗使用 `s` 选择 soft、`m` 选择 mixed、`h` 选择 hard。soft/mixed 需要一次命令确认；hard 还要先确认危险警告，再准确输入目标短哈希。

### Changes

Changes 是独立主界面，不挂在仓库树或 Working Tree 子页面下。左侧固定为三级结构：

```text
▼ Changes
  ├─▼ Staged Changes
  │  ├─ M src/indexed.rs
  │  └─ A src/new.rs
  └─▼ Unstaged Changes
     ├─ M src/edited.rs
     └─ ? notes.txt
```

同一文件如果同时存在 index 和 working-tree 修改（porcelain `MM`），会分别出现在 Staged 与 Unstaged 中；选择不同节点时右侧只展示对应边界的 diff，不会把两类修改混在一起。untracked 和 conflict 归入 Unstaged。

| Key | 操作 |
|---|---|
| `Ctrl-G` | 从任意主界面进入；在 Changes 内再次按下返回原界面 |
| `↑` / `k`, `↓` / `j` | 在 Changes、Staged/Unstaged 和文件节点间移动 |
| `←` / `h`, `→` / `l`, `Enter` | 折叠/展开 Changes 或分组；文件节点上 `Enter` 聚焦 diff |
| `Tab` / `Shift-Tab` | 在三级树和右侧 diff 之间切换焦点 |
| `Space` | 文件节点选择/反选当前文件；分组/root 节点选择或反选全部子项；diff 焦点下作用于当前文件 |
| `s` | stage 已选择的 Unstaged 文件；没有显式选择时 stage 当前 Unstaged 文件 |
| `u` | unstage 已选择的 Staged 文件；没有显式选择时 unstage 当前 Staged 文件 |
| `c` | 打开 commit message 弹窗；`Enter` 使用全部 staged 内容创建 commit |
| `PageUp` / `PageDown` | 翻页选择文件或滚动 diff |
| `v` | unified / side-by-side 切换；终端不足 140 列时自动回退 unified |
| `w` | diff 换行开关 |
| `r` | 重新读取当前工作区 |
| `Esc` | 返回进入 Changes 前的界面和焦点 |

右侧直接复用 Commit File Diff 的解析、行号、配色、滚动、换行和两种 diff mode 组件。

Stage 使用 path-limited `git add --all -- <paths>`；unstage 使用 path-limited `git reset -- <paths>`，只修改 index，不丢弃 working-tree 文件，并支持 unborn repository。创建 commit 前必须至少存在一个 staged 文件且 message 不能为空。

### Commit Detail

| Key | 操作 |
|---|---|
| `Space` | 展开或折叠文件 hunk summary |
| `Enter` / `v` | 打开所选文件 diff |
| `m` / `Ctrl-Alt-C` | 复制当前完整 commit message |
| `Esc` | 返回 Branch / Commit Overview |

### File Diff Detail

| Key | 操作 |
|---|---|
| 文件列表 `↑` / `k`, `↓` / `j` | 选择文件并刷新右侧 diff，焦点保持在左侧文件列表 |
| `n` / `p` | 下一个/上一个文件 |
| `v` | unified / side-by-side 切换 |
| `w` | diff 换行开关 |
| `Ctrl-C` | 复制当前/已多选 commit hashes |
| `Ctrl-Shift-C` | 复制当前 commit info |
| `m` / `Ctrl-Alt-C` | 复制当前完整 commit message |
| `Esc` | 返回 Commit Detail |

## 安全边界

- Renderer 只读取 `&AppState`，不会修改业务状态或执行 Git。
- Input Mapper 只把终端事件转换为 `Action`。
- Git Worker 是唯一允许执行 `git` 的组件；每个 job 显式携带仓库路径，并使用参数数组而非 shell 字符串。
- Git Worker 为每个 job 写入 queued/started/completed JSONL 审计记录；日志失败不会阻塞 Git 操作。
- 读请求带 job id；快速切换时，过期响应不会覆盖当前选择。
- Git 输出中的控制字符在渲染前会被清理，避免终端转义注入。
- `fetch`、`switch`、rebase 与 cherry-pick 必须在执行前确认。
- stage/unstage 只操作 Changes 中显式多选的文件；未多选时只操作当前文件。commit 必须先进入消息编辑弹窗。
- `reset --hard` 必须准确输入所选 commit 的短哈希。
- rebase 前要求工作区和 index 干净；检测到冲突后 worker 会立即尝试 `git rebase --abort`，成功后 UI 弹窗说明已经恢复。
- 集成测试中的所有写操作只发生在临时 Git 仓库。

> Pitui 只负责确认并提交 Git 命令。若 cherry-pick 产生冲突，请退出 Pitui 后使用标准 Git 命令完成或中止该操作。

## 架构

```text
Keyboard Event
  -> Action
  -> App Controller / Store
  -> GitRequest + Job ID + Repository cwd
  -> Git Worker
  -> Backend JSONL Log (queued / started / completed)
  -> Parser / Domain Model
  -> GitResponse
  -> AppState
  -> Renderer
```

实现严格分为：

- `src/tui`：终端生命周期、输入和渲染。
- `src/app`：Action、状态、选择修正、请求调度和状态转移。
- `src/domain`：仓库、分支、commit、reflog、文件和 diff 纯模型。
- `src/git`：Git 协议、Worker、命令执行、后台操作日志和解析器。

完整产品设计见 [`docs/git-tui-design.md`](docs/git-tui-design.md)。

## 开发验证

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## 参与贡献与安全问题

- 贡献流程与安全设计约束见 [`CONTRIBUTING.md`](CONTRIBUTING.md)。
- 漏洞请按 [`SECURITY.md`](SECURITY.md) 使用 GitHub 私密漏洞报告，不要公开敏感细节。
- Bug report、Feature request、Pull Request 模板以及跨平台 CI 已放在 [`.github`](.github) 目录。

## License

Pitui 采用 [MIT License](LICENSE)，Copyright (c) 2026 Pitui contributors。

当前范围不包括逐行/逐 hunk partial staging、stash、交互式 rebase todo、merge conflict editor、blame 和内置文件编辑器。
