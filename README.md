# Pitui

> [!IMPORTANT]
> **Vibe Coding 声明：当前仓库中的全部代码、测试和文档均通过 vibe coding 生成；维护者负责提出需求、审阅结果并运行验证。**

Pitui 是一个安全、克制、支持多仓库的 Git TUI，以分支、commit 和 diff 浏览为核心。

```text
Repositories + Branches -> Commits -> Commit + Changed Files -> Files + Diff
          |
          +-------------> Reflog
          +-------------> Remotes -> Add / Shared URL / Upstream

Any main view -- Ctrl+G --> Changes -> Staged / Unstaged -> Files + Diff
```

它允许你在**不切换当前工作分支**的情况下浏览任意本地或远程分支的提交；真正修改仓库的操作始终经过确认。

## 功能

- 浏览本地与远程分支。
- 同时打开多个仓库，并以“仓库 → 分支”的树形结构展示。
- 在仓库节点确认后执行 `git fetch --all --prune`。
- 在仓库节点确认执行 pull/push；pull 固定使用 `git pull --rebase`，不会产生隐式 merge commit。
- 独立 Remote 管理界面可新增 remote、将现有 remote 统一为一个 fetch/push URL，并为当前分支显式选择 upstream remote。
- fetch、pull 和 push 前强制检查 remote 的 fetch/push URL 相同，并禁止当前分支从一个 remote 拉取却向另一个 remote 推送。
- 按仓库浏览最近 300 条 reflog，并从 commit 或 reflog 条目选择 reset 目标。
- 独立 Changes 界面使用“Changes → Staged/Unstaged → 文件”三级树展示当前修改。
- Changes 与 commit file diff 复用 unified / side-by-side diff 组件。
- unified / side-by-side 的启动默认模式可通过全局配置设置；窄终端仍安全降级为 unified。
- Changes 支持文件/分组多选，可在文件树或 diff 焦点下 stage、unstage，并通过消息弹窗创建 commit。
- Git worker 的每个后台 job 都写入持久化 JSONL 操作日志，包含排队、开始、完成、耗时与失败结果。
- 不执行 checkout/switch 即可查看任意分支最近 300 个 commits。
- 查看 commit 元信息、文件状态、增删行数和 hunk summary。
- 查看单文件 unified diff。
- 在宽度不小于 140 列时使用 side-by-side diff。
- 分支和 commit 过滤。
- commit 支持多选，并复制一个或多个完整 commit hash；也可单独复制当前 commit info 或完整 message。
- commit 复制使用 `Ctrl-C` 前缀的二级快捷键，减少普通模式的单键占用。
- 不进行定时 Git 状态轮询；所有主界面统一使用 `Ctrl-R` 手动刷新，避免状态栏周期性闪烁。
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

## 全局配置

Pitui 在进入 terminal raw mode 前读取并严格校验一个版本化 TOML 配置。默认文件不存在是
正常情况，此时使用内置默认值；不会读取仓库内配置，也不会自动轮询配置文件。修改配置后
需要重启 Pitui。

| 平台 | 默认配置文件 |
|---|---|
| macOS | `~/Library/Application Support/pitui/config.toml` |
| Linux / Unix | `${XDG_CONFIG_HOME:-~/.config}/pitui/config.toml` |
| Windows | `%APPDATA%\pitui\config.toml` |

配置文件查找优先级为 `--config <path>`、`PITUI_CONFIG`、平台默认路径；
`--no-config` 可跳过文件。相对日志路径以配置文件所在目录为基准。可用以下命令先诊断，
它们不会启动 TUI 或 Git worker：

```bash
pitui --check-config --config /path/to/config.toml
pitui --print-config-path
pitui --print-effective-config
```

最小配置必须声明 schema 版本。下面把共享 diff 组件的默认模式改为 side-by-side：

```toml
schema_version = 1

[diff]
default_mode = "side-by-side" # unified | side-by-side
```

该默认值同时初始化 Commit File Diff 和 Changes Diff。终端宽度小于 140 列时只在渲染时
临时回退为 unified，不会修改配置或当前 session mode；按 `v` 切换只影响当前运行。

同一个配置层还支持命令绑定、分级快捷键、底部提示和日志策略。例如：

```toml
schema_version = 1

[ui.footer]
mode = "contextual"            # contextual | compact | hidden
max_rows = 2                    # 1..3
default_visibility = "registry" # registry | all | allowlist
show_alternative_bindings = false

[ui.footer.groups."commit.copy"]
visible = true
label = "copy…"

[keybindings]
chord_timeout_ms = 0

[keybindings.commands."app.refresh"]
bindings = ["Ctrl+R"]

[keybindings.commands."commit.copy.hash"]
bindings = ["Ctrl+C h"]

[logging]
enabled = true
level = "info"
path = "logs/pitui.jsonl"
flush_interval_ms = 0
buffer_capacity = 1024
max_detail_chars = 4096
fail_on_open_error = false

[logging.rotation]
enabled = true
max_size = "5 MiB"
keep_files = 3
rotate_on_start = false
```

未出现的 command 继承默认绑定，`bindings = []` 才会解除绑定。隐藏 footer 提示不会解除
按键；确认弹窗、hard reset 二次确认和文本编辑按键属于安全保留交互，不能通过配置绕过。
完整字段和示例见 [`docs/config.example.toml`](docs/config.example.toml)，全部稳定 command id
可通过 `--print-effective-config` 查看；设计与约束见
[`docs/global-configuration-design.md`](docs/global-configuration-design.md)。

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

默认日志达到 5 MiB 时轮转为 `pitui.jsonl.1`，保留当前文件和一份备份。全局配置可修改
日志开关、路径、level、flush interval、writer buffer、详情长度、轮转大小和 `.1` 到 `.N`
的保留数量。若配置位置无法创建且 `fail_on_open_error=false`，Pitui 会回退到临时目录并在
底部消息中显示实际路径；`pitui --help` 也会输出当前默认路径。

日志记录操作元数据、仓库路径、所选文件路径和 Git 错误，因此分享前应检查并脱敏。为避免无意泄露，diff/文件内容不会写入日志，commit message 只记录字节数，详情和错误字段最长保留 4096 个字符。

## 快捷键

下表是内置默认绑定。底部提示和输入解析共享同一份有效 keymap，只显示当前状态下按下后
确实会有动作的“下一键”；二级/三级快捷键只在按下当前前缀后逐级显示后续键。绑定、提示
是否可见、label、priority、行数和 overflow 均可由全局配置独立控制。

### 全局

| Key | 操作 |
|---|---|
| `q` | 普通模式退出；普通确认/错误弹窗中取消或关闭；哈希输入弹窗中作为输入字符 |
| `Ctrl-G` | 从任意主界面打开独立 Changes；再次按下或按 `Esc` 返回原界面和原焦点 |
| `Ctrl-C` | 有当前 commit 时进入二级复制快捷键；否则退出 |
| `Tab` / `Shift-Tab` | 切换面板焦点 |
| `↑` / `k`, `↓` / `j` | 移动选择或滚动 diff |
| `PageUp`, `PageDown`, `Home`, `End` | 翻页或跳转 |
| `Esc` | 返回上一视图或取消弹窗 |
| `Ctrl-R` | 从任意主界面手动刷新全部仓库、分支、当前 commit 列表以及 reflog/Changes/Remotes 数据 |

### Branch / Commit Overview

| 焦点 | Key | 操作 |
|---|---|---|
| 仓库节点 | `Enter` | 展开/折叠该仓库的分支 |
| 仓库节点 | `f` | 确认后在该仓库执行 `git fetch --all --prune` |
| 仓库节点 | `p` | 确认后对当前分支执行 `git pull --rebase` |
| 仓库节点 | `P` | 确认后对当前分支执行 `git push` |
| 仓库节点 | `g` | 查看该仓库 reflog |
| 仓库/分支节点 | `o` | 打开所属仓库的 Remote 管理界面 |
| 分支节点 | `Enter` | 浏览所选分支 commits，不切换真实分支 |
| 仓库/分支树 | `↑` / `k`, `↓` / `j` | 选择分支时自动刷新右侧 commits，焦点保持在左侧 |
| 分支节点 | `s` | 确认后在所属仓库执行 `git switch` |
| 分支节点 | `b` | 确认后将当前分支 rebase 到所选分支；冲突自动 abort |
| 仓库/分支树 | `/` | 按仓库名、路径、分支或 subject 过滤 |
| Commits | `Enter` | 打开 commit detail |
| Commit Detail 中的 Commits | `↑` / `k`, `↓` / `j` | 选择 commit 时自动刷新右侧 metadata/files，焦点保持在左侧 |
| Commits | `Space` | 加入/移出 commit 复制多选集合 |
| Commits | `Ctrl-C` → `h` | 按当前列表顺序复制所有多选 commit 的完整 hash；无多选时复制当前 hash |
| Commits | `Ctrl-C` → `i` | 复制当前 commit info（hash、author、date、refs 和 message） |
| Commits | `Ctrl-C` → `m` | 复制当前完整 commit message；需要时后台加载 body，但不改变 screen/focus |
| Commits | `/` | 过滤 commits |
| Commits | `y` | 将 commit 加入 cherry-pick queue |
| Commits | `Y` | 打开 queue 确认弹窗 |
| Commits | `R` | 选择 soft/mixed/hard reset 模式并确认 |

Pull 只使用 rebase 策略，不读取 `pull.rebase` 配置来决定是否 merge。执行前 Controller 与 Git worker 都会检查 attached branch、干净的 working tree/index 和既存 rebase；发生冲突时自动执行 `git rebase --abort`。Push 使用 Git 已配置的 upstream/default push target；Pitui 不会在 push 时隐式创建 upstream，但可在 Remote 管理中由用户显式设置。

复制操作采用二级快捷键，避免 hash/info/message 各自长期占用普通模式按键。按下 `Ctrl-C`
后底部才显示 `h hash | i info | m message`；`Esc`、`q` 或再次按 `Ctrl-C` 取消，且整个
过程不会改变当前 focus。

Pitui 不会每隔固定时间轮询仓库。启动时会完成一次初始加载，Pitui 自己完成 Git 写操作后会刷新受影响的数据；外部命令或编辑器造成的变化由用户在任意主界面按 `Ctrl-R` 主动同步。

### Remote Management

在左侧仓库/分支树选中任意仓库或其分支后按 `o` 进入。左侧是 remote 列表，右侧同时显示 fetch URL、有效 push URL 以及当前分支路由。

| Key | 操作 |
|---|---|
| `a` | 输入 remote name 和一个共享 URL，确认后执行 `git remote add` |
| `e` | 将所选 remote 的所有 fetch URL 归一为输入值，并删除独立 `pushurl` |
| `u` | 将所选 remote 设为当前分支的 fetch/pull upstream 和 push target；远程分支不存在时可由下一次 push 创建 |
| `Ctrl-R` | 重新读取 remote 配置 |
| `Esc` | 返回仓库/分支树 |

标记 `★` 表示该 remote 同时是当前分支的拉取 upstream 与推送目标；`F` / `P` 表示两个方向被拆分；`!` 表示 fetch/push URL 不一致。后两种配置会阻止 fetch/pull/push，直到使用 `e` 或 `u` 修复。

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
| `PageUp` / `PageDown` | 文件树翻页；diff 焦点下按 10 行滚动 |
| `Home` / `End` | 跳到文件树首尾；diff 焦点下跳到内容顶部/底部 |
| `v` | unified / side-by-side 切换；终端不足 140 列时自动回退 unified |
| `w` | diff 换行开关 |
| `Ctrl-R` | 重新读取当前工作区（与全局手动刷新相同） |
| `Esc` | 返回进入 Changes 前的界面和焦点 |

右侧直接复用 Commit File Diff 的解析、行号、配色、滚动、换行和两种 diff mode 组件。

Stage 使用 path-limited `git add --all -- <paths>`；unstage 使用 path-limited `git reset -- <paths>`，只修改 index，不丢弃 working-tree 文件，并支持 unborn repository。创建 commit 前必须至少存在一个 staged 文件且 message 不能为空。

### Commit Detail

| Key | 操作 |
|---|---|
| Commits 列 `↑` / `k`, `↓` / `j` | 选择 commit 并自动刷新右侧详情，焦点保持在左侧 Commits |
| `Space` | 展开或折叠文件 hunk summary |
| `Enter` / `v` | 打开所选文件 diff |
| `PageUp` / `PageDown`, `Home` / `End` | 文件列表翻页或跳到首尾 |
| `Ctrl-C` → `h` / `i` / `m` | 复制当前 hash / info / 完整 message |
| `Esc` | 返回 Branch / Commit Overview |

### File Diff Detail

| Key | 操作 |
|---|---|
| 文件列表 `↑` / `k`, `↓` / `j` | 选择文件并刷新右侧 diff，焦点保持在左侧文件列表 |
| `PageUp` / `PageDown`, `Home` / `End` | 左侧焦点时翻页/跳到文件首尾且只加载最终文件；右侧焦点时滚动/跳到 diff 首尾 |
| `n` / `p` | 下一个/上一个文件 |
| `v` | unified / side-by-side 切换 |
| `w` | diff 换行开关 |
| `Ctrl-C` → `h` / `i` / `m` | 复制当前或多选 hashes / 当前 info / 完整 message |
| `Esc` | 返回 Commit Detail |

## 安全边界

- Renderer 只读取 `&AppState`，不会修改业务状态或执行 Git。
- Input Mapper 只把终端事件转换为 `Action`。
- Git Worker 是唯一允许执行 `git` 的组件；每个 job 显式携带仓库路径，并使用参数数组而非 shell 字符串。
- Git Worker 为每个 job 写入 queued/started/completed JSONL 审计记录；日志失败不会阻塞 Git 操作。
- 读请求带 job id；快速切换时，过期响应不会覆盖当前选择。
- Git 输出中的控制字符在渲染前会被清理，避免终端转义注入。
- `fetch`、`pull --rebase`、`push`、`switch`、rebase 与 cherry-pick 必须在执行前确认。
- pull 永远显式传入 `--rebase`；要求干净工作区，冲突时自动 abort。push 只使用已配置目标，且终端认证提示被禁用以避免 TUI 卡住。
- 每次 fetch/pull/push 前 worker 都重新读取 Git config；任意 remote 的有效 fetch/push URL 不相同，或当前分支的 upstream/push remote 被拆分时，操作会在联系网络前失败。
- 新增 remote、统一 URL 和设置 upstream 都必须经过确认；URL 更新和分支路由更新在失败时尝试恢复原 Git config。
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
