# Pitui 全局配置层设计与实现

> 状态：**第一阶段已实现**（启动时加载；不包含运行时 reload、文件 watcher 或 TUI 编辑器）  
> 目标：提供统一、可验证、可扩展的全局配置基础；除本文明确要求的
> action-only footer 重构外，不改变当前默认绑定、Git 语义和安全边界。

实现入口为 `src/config.rs` 与 `src/app/command.rs`。当前 `buffer_capacity` 是后台日志
`BufWriter` 的字节容量；独立异步日志队列和事务式手动 reload 保留为后续扩展。

---

## 1. 背景

此前快捷键映射位于 `src/tui/input.rs`，底部 hotkey bar 位于
`src/tui/render.rs`，两者分别维护。后台日志只支持平台默认路径、
`PITUI_LOG`、JSONL 和固定单文件轮转。第一阶段实现已经把这些入口收敛到同一有效配置。

这会产生三个长期问题：

1. 修改绑定后，输入处理与底部提示容易不一致。
2. 新增 screen、focus 或二级快捷键时，需要同时修改多个 `match`。
3. 日志策略只能依赖编译期常量或单个环境变量，无法满足不同使用环境。

全局配置层必须解决这些问题，但不能削弱危险 Git 操作的确认边界。

---

## 2. 设计目标

### 2.1 必须实现

- 使用一个版本化的全局 TOML 文件承载配置。
- 没有配置文件时保持当前默认绑定、diff 默认模式、日志策略和 Git 行为；footer 则按
  本文升级为 action-only、逐级 chord 提示。
- 快捷键绑定和底部提示来自同一个命令注册表与同一个有效 keymap。
- footer 只显示在当前完整状态下按下后会产生有效动作或进入有效 chord 的“下一键”。
- 多段快捷键必须逐级揭示，未按下前缀时不得提前显示后续按键。
- 支持普通单键、修饰键和多段组合键，例如 `Ctrl+C` 后再按 `h`。
- 支持按 command/chord group 配置哪些提示可见，以及提示文字、优先级和显示上限；
  隐藏提示不能解除实际绑定。
- 支持配置共享 diff 组件的默认显示模式。
- 支持日志开关、路径、级别、格式、刷新间隔、单文件大小和保留数量。
- 配置必须在进入 terminal raw mode 前完成语法与语义校验。
- 配置加载失败必须可诊断；若未来增加运行时重载，失败时必须保留最后一份有效配置。
- 所有日志等级继续遵守现有脱敏规则。

### 2.2 明确不做

- 第一阶段不做 TUI 内配置编辑器。
- 第一阶段不读取仓库内 `.pitui.toml`，避免打开不可信仓库时被修改快捷键或日志位置。
- 不自动监视配置文件，不引入周期轮询或界面闪烁。
- 不允许配置跳过确认弹窗、hard reset 二次确认或 rebase 安全检查。
- 不允许配置 shell 命令、Git 参数模板或任意脚本。
- 本文不包含主题、颜色和 diff 算法配置；它们可复用同一配置层后续扩展。

---

## 3. 配置文件发现与优先级

### 3.1 默认位置

| 平台 | 默认配置文件 |
|---|---|
| macOS | `~/Library/Application Support/pitui/config.toml` |
| Linux / Unix | `${XDG_CONFIG_HOME:-~/.config}/pitui/config.toml` |
| Windows | `%APPDATA%\\pitui\\config.toml` |

只加载一个全局配置文件。默认文件不存在属于正常情况，不产生警告。

### 3.2 指定配置文件

发现顺序：

1. CLI `--config <path>`；
2. 环境变量 `PITUI_CONFIG`；
3. 平台默认路径。

`--no-config` 完全跳过文件加载，只使用内置默认值和允许的 CLI/环境变量覆盖。

### 3.3 值合并优先级

从低到高：

```text
compiled defaults
  < global config.toml
  < supported environment overrides
  < explicit CLI overrides
```

为保持兼容，现有 `PITUI_LOG` 继续覆盖 `logging.path`。不读取 Git config，
也不读取当前仓库中的配置文件。

### 3.4 路径规则

- 绝对路径按原值使用。
- `~` 只允许出现在路径开头并展开为用户主目录。
- 相对路径相对于 `config.toml` 所在目录解析，不相对于当前仓库或启动目录。
- 不执行 shell expansion，不解析命令替换。
- `--print-effective-config` 输出解析后的绝对路径，但不输出环境变量值或敏感内容。

---

## 4. 配置模型

配置文件必须包含版本：

```toml
schema_version = 1
```

第一阶段的有效数据结构：

```rust
struct GlobalConfigFile {
    schema_version: u32,
    ui: UiConfigFile,
    keybindings: KeybindingConfigFile,
    diff: DiffConfigFile,
    logging: LoggingConfigFile,
}

struct ResolvedConfig {
    source_path: Option<PathBuf>,
    footer: ResolvedFooterConfig,
    keymap: ResolvedKeymap,
    diff: ResolvedDiffConfig,
    logging: ResolvedLoggingConfig,
}
```

`GlobalConfigFile` 保留“字段是否出现”的信息；`ResolvedConfig` 是完成默认值填充、
路径展开、按键规范化和冲突检查后的不可变快照。

配置解析采用严格模式：

- 未知顶层字段报错；
- 未知 command id 报错并给出相近候选；
- 数值越界、非法枚举和非法按键表达式报错；
- 同一上下文内的绑定冲突报错；
- 更新 schema 时通过显式迁移处理，不静默猜测。

---

## 5. 命令注册表

### 5.1 以命令语义为中心

配置不得直接引用 Rust enum variant，也不得把 screen/focus 判断写进 TOML。
程序内部维护稳定的 `CommandSpec`。`CommandId` 使用 `#[repr(usize)]`，其 discriminant
直接索引 `COMMAND_SPECS`；`invoke` 是可调用函数指针，因此它是 Rust 中对应 C++
function-pointer jump table 的实现，而不是散落在 Input/Renderer 中的多份 `match`：

```rust
type CommandHandler = fn(&AppState) -> Option<Action>;

struct CommandSpec {
    id: CommandId,
    name: &'static str,
    default_bindings: &'static [&'static str],
    default_label: &'static str,
    default_visible: bool,
    footer_group: FooterGroup,
    chord_group: Option<&'static str>,
    mount: CommandMount,       // Global | Focus
    contexts: u16,             // ShortcutContext bit set
    invoke: CommandHandler,
}
```

正常模式的 `ShortcutContext` 与文档中的列操作集一一对应：

```text
branch.tree          -> §6.4 BranchList 操作集
overview.commits     -> §6.5 CommitList 操作集
detail.commits       -> §7.4 CommitList 操作集
commit.files         -> §7.5 CommitFileList 操作集
diff.files           -> §8.4 FileList 操作集
diff.view            -> §8.5 DiffView 操作集
reflog.list          -> Reflog 操作集
changes.tree         -> ChangesTree 操作集
changes.diff         -> ChangesDiff 操作集
remotes.list         -> Remote Management 操作集
```

`CommandMount::Global` 始终挂载；`CommandMount::Focus` 必须先命中当前 context bit，再调用
`invoke` 检查 selection、pending job、scroll boundary 等动态条件。这样 screen 相同但 focus
不同，甚至同一份 Commit 数据作为右栏或左栏显示时，也只能响应各自操作表。

示例 command id：

```text
app.quit
app.shortcuts
app.refresh
view.changes.toggle
navigation.up
navigation.page_down
repository.fetch
repository.pull_rebase
repository.push
repository.remotes.open
commit.copy.hash
commit.copy.info
commit.copy.message
file.copy.name
file.copy.absolute_path
file.copy.relative_path
changes.stage
changes.unstage
changes.commit
remote.add
remote.set_shared_url
remote.set_upstream
```

`invoke` 仍由程序定义，并同时接收 `mode + screen + focus + selection + pending state +
viewport state`。例如 `repository.push` 只在 Branch Overview 的有效仓库节点且没有互斥 job
时可用；空列表不提供移动命令，已经位于内容底部时不提供继续向下滚动命令。
配置只改变绑定和显示方式，不能扩大命令作用域。

### 5.2 单一事实来源

输入和提示必须使用同一个 registry：

```text
KeyEvent
  -> ResolvedKeymap.resolve(current input state, event/chord)
  -> CommandId
  -> COMMAND_SPECS[CommandId as usize].invoke
  -> Action

CurrentInputState(context + chord prefix)
  -> ResolvedKeymap.next_transitions(input state)
  -> CommandRegistry.actionable(transitions, AppState)
  -> footer visibility policy
  -> footer items for this level only
```

禁止继续在 Renderer 中手写 `"P push"` 一类提示。绑定变化后，底部提示、
二级快捷键提示和实际输入处理必须在同一帧内一致。

### 5.3 Modal 可调用表

Filter、确认框、hard reset 哈希输入、commit message、remote name/URL 编辑、错误框和快捷键
帮助框不会读取普通 focus 表。`MODE_KEY_TABLES` 保存 `fn(&AppState, KeyEvent) ->
Option<Action>`，通过 `ModalShortcutSetId` 选择唯一 handler；同一个 id 还索引
`MODAL_SHORTCUT_SETS` 的 footer/help 操作说明。因此例如 commit 提交只能响应文本、
Backspace、Enter、Esc，Remote Add 才额外挂载 Tab/BackTab，普通 `c`/`u`/复制 chord
不会穿透 modal。

---

## 6. 快捷键配置

### 6.1 TOML 结构

未出现在配置中的命令继承默认绑定。`bindings = []` 表示显式解除绑定。
数组表示多个可替代绑定，第一项是底部默认展示项。

```toml
[keybindings]
chord_timeout_ms = 0 # 0 表示等待下一键，直到完成或 Esc 取消

[keybindings.commands."app.refresh"]
bindings = ["Ctrl+R"]

[keybindings.commands."app.shortcuts"]
bindings = ["Ctrl+?", "?"]

[keybindings.commands."view.changes.toggle"]
bindings = ["Ctrl+G"]

[keybindings.commands."repository.push"]
bindings = ["P", "Alt+P"]

[keybindings.commands."commit.copy.hash"]
bindings = ["Ctrl+C h"]

[keybindings.commands."commit.copy.info"]
bindings = ["Ctrl+C i"]

[keybindings.commands."commit.copy.message"]
bindings = ["Ctrl+C m"]

[keybindings.commands."file.copy.name"]
bindings = ["Ctrl+C n"]

[keybindings.commands."file.copy.absolute_path"]
bindings = ["Ctrl+C a"]

[keybindings.commands."file.copy.relative_path"]
bindings = ["Ctrl+C r"]

[keybindings.commands."repository.reflog.open"]
bindings = [] # 明确禁用
```

### 6.2 按键语法

支持：

- 字符：`q`、`P`、`/`；字符大小写有意义。
- 命名键：`Enter`、`Esc`、`Space`、`Tab`、`BackTab`、`Up`、`Down`、
  `Left`、`Right`、`Home`、`End`、`PageUp`、`PageDown`、`Backspace`。
- 修饰键：`Ctrl+R`、`Alt+P`、`Shift+Tab`。
- 多段组合键：`Ctrl+C h`、`Ctrl+K r`，段与段之间使用空格。
- 每条 sequence 最多三段，防止形成不可发现的长命令链。
- `Esc` 是 active chord 的固定取消键，不能作为第二段或第三段；它仍可作为普通模式的
  单键或第一段参与冲突校验。

解析后统一规范化，例如 `control+r` 输出为 `Ctrl+R`。需要考虑终端别名：
`Ctrl+I` 与 `Tab`、`Ctrl+M` 与 `Enter` 等在普通终端中可能不可区分，冲突检查按
终端实际事件而不是配置文本执行。不可移植的组合应产生警告或错误。

### 6.3 上下文与冲突

只有在同一 `mode + screen + focus + selection capability` 中可能同时生效的绑定才冲突。
因此 Commits 上下文的 `Ctrl+C → h/i/m`、file-list 上下文的 `Ctrl+C → n/a/r` 与其他
上下文中的 `Ctrl+C` 退出可以共存。三个 scope 必须严格互斥：CommitFiles/FileList 不挂载
commit copy，DiffView 不挂载任何 copy chord。

必须拒绝：

- 同一上下文两个命令拥有相同 sequence；
- 同一上下文一个完整 sequence 同时是另一个 sequence 的前缀；
- 不可配置命令被覆盖；
- `app.quit` 的全部绑定被解除，或仅剩会被其他 chord 前缀遮蔽的绑定；
- 普通按键绑定侵入文本输入模式。

配置错误示例必须同时指出 command、sequence 和冲突上下文：

```text
keybinding conflict: `s`
  changes.stage
  changes.commit
context: screen=Changes focus=ChangesTree mode=Normal
```

### 6.4 Chord 状态

原有 `ShortcutMenu::CommitCopy` 已泛化为：

```rust
GlobalMode::Chord {
    prefix: Vec<KeyStroke>,
    started_at: Instant,
}
```

进入 Chord 后：

- 普通状态只显示可进入该 chord 的第一级按键，例如 `Ctrl+C copy…`；不得提前显示
  `h hash`、`i info`、`m message`，也不得在 root footer 展示 `Ctrl+C h`；
- Commits focus 按下 `Ctrl+C` 后 footer 才切换为
  `h hash | i info | m message | Esc cancel`；
- CommitFiles/DiffFiles focus 使用同一个 prefix，但 footer 只切换为
  `n file name | a absolute path | r relative path | Esc cancel`；
- DiffView 和其他没有 copy table 的 focus 不进入 chord，`Ctrl+C` 回退到 `app.quit`；
- 三段 chord 同理，每次只显示当前 prefix 下可接受的下一段；
- `Esc` 取消并回到原 focus；
- 未匹配按键默认取消且不透传，避免误触危险命令；
- `chord_timeout_ms = 0` 不自动超时；大于 0 时超时取消但不执行前缀对应动作；
- Chord 期间不得改变 screen、selection 或 focus。

### 6.5 安全保留键

第一阶段只开放 Normal 与 Chord 命令的绑定。以下交互保持程序保留，不能配置：

- 确认弹窗的 `Enter` 与取消弹窗的 `Esc`；
- hard reset 模式选择和目标哈希准确输入；
- commit message、filter、remote name/URL 编辑时的字符输入；
- 错误弹窗的安全关闭路径。

这保证快捷键配置不能绕过二次确认，也不能将输入的 commit message 或 remote URL
误解释为命令。

### 6.6 全局快捷键参考框

`app.shortcuts` 默认绑定为 `Ctrl+?` 与 plain `?` fallback，在任意 Normal focus 打开
`GlobalMode::ShortcutHelp { scroll }`。弹窗不是手写静态清单，而是依次读取：

1. 当前有效 `ResolvedKeymap` 中的 Global commands；
2. 十个 `ShortcutContext` 的 focus command tables；
3. `MODAL_SHORTCUT_SETS` 中 filter、confirmation、commit submission、remote editor、
   error 与 help 自身的安全保留操作集。

每行同时显示有效 sequence、label 和稳定 operation id；解除绑定的命令显示 `(unbound)`，
当前 focus 表用 `▶` 标记。`↑/↓`、PageUp/PageDown、Home/End 滚动，`Ctrl+?`、`?`、Enter、Esc
或 `q` 关闭并恢复原 focus。帮助框本身使用独占 modal input table，不能触发其背后的命令。

---

## 7. 底部提示配置

### 7.1 配置结构

```toml
[ui.footer]
mode = "contextual" # contextual | compact | hidden
max_rows = 1        # 1..3
show_global = true
show_alternative_bindings = false
default_visibility = "registry" # registry | all | allowlist
separator = " | "
overflow = "count" # count | ellipsis

[ui.footer.groups."commit.copy"]
visible = true
label = "copy…"
priority = 110

[ui.footer.groups."file.copy"]
visible = true
label = "copy file…"
priority = 110

[ui.footer.commands."app.shortcuts"]
visible = true
label = "shortcuts"
priority = 100

[ui.footer.commands."app.refresh"]
visible = true
label = "refresh"
priority = 100

[ui.footer.commands."view.changes.toggle"]
visible = true
label = "changes"
priority = 100

[ui.footer.commands."navigation.up"]
visible = false

[ui.footer.commands."repository.push"]
visible = true
label = "push"
priority = 80

[ui.footer.commands."commit.copy.hash"]
visible = true
label = "hash"

[ui.footer.commands."commit.copy.info"]
visible = true
label = "info"

[ui.footer.commands."commit.copy.message"]
visible = true
label = "message"

[ui.footer.commands."file.copy.name"]
visible = true
label = "file name"

[ui.footer.commands."file.copy.absolute_path"]
visible = true
label = "absolute path"

[ui.footer.commands."file.copy.relative_path"]
visible = true
label = "relative path"
```

含义：

- `contextual`：显示当前上下文所有可用命令，再按优先级裁剪。
- `compact`：只显示 global、primary 和 safety 分组。
- `hidden`：不渲染 footer；实际绑定仍然生效。
- `max_rows`：允许 footer 使用的最大行数，不得挤压主体到不可用尺寸。
- `show_global`：是否在各 screen 重复展示全局命令。
- `show_alternative_bindings=false`：一个 command 有多个当前可用绑定时只展示第一项；
  其他绑定继续生效。
- `default_visibility=registry`：使用 registry 默认可见性。
- `default_visibility=all`：所有当前 actionable 的命令/前缀默认可见，再应用局部覆盖。
- `default_visibility=allowlist`：默认全部隐藏，只显示显式设置 `visible=true` 的
  command 或 chord group。
- `overflow=count`：空间不足时显示 `… +N`。

绑定配置与提示配置必须分离：`keybindings.commands` 决定按键是否生效，
`ui.footer.commands` 只决定其提示是否显示。`visible=false` 不会解除 binding；
`bindings=[]` 才会解除。`ui.footer.groups` 配置 chord 前缀节点在上一级 footer 中的
label、可见性和优先级。

group 和 member command 的可见性彼此独立：group 控制 root 中是否显示
`Ctrl+C copy…`，member 控制进入该 prefix 后是否显示 `h hash` 等下一键。
使用 `allowlist` 时两层都需要按需显式开启。未绑定命令永远不进入 live footer，
不存在通过配置显示“不可按”的占位项。

### 7.2 生成规则

1. 使用当前 `AppState + mode + screen + focus + selection + pending jobs + viewport +
   chord prefix` 构造不可变 `InputContextSnapshot`。
2. 查询 keymap trie 在当前 prefix 下的直接子节点；孙节点和更深节点禁止参与本级 footer。
3. 使用与 input resolver 相同的 `actionability` 谓词过滤：如果此刻按下该键不会产生
   `Action`、有效编辑动作或进入至少包含一个可执行后继的 chord，则不显示。
4. root 中遇到 chord prefix 时只生成一个 group item；Commits 显示 `Ctrl+C copy…`，
   文件列显示 `Ctrl+C copy file…`。进入 prefix 后才根据当前 focus 表的直接子节点生成
   `h/i/m` 或 `n/a/r`，禁止合并两个表。
5. 应用 `default_visibility`、group/command 的 `visible`、label 和 priority 配置。
6. 读取有效 keymap 的首选 binding；需要时根据 `show_alternative_bindings` 展示其他
   同层且当前 actionable 的替代键。
7. 按 `safety > global > primary > contextual > navigation` 以及 priority 排序，保证
   `Ctrl+G changes`、`Ctrl+R refresh` 等全局入口在窄终端中仍优先可见。
8. 使用 `unicode-width` 计算终端列宽，按完整 item 裁剪，不能截断按键名称。
9. label 在加载时清理控制字符与 bidi override，并限制显示长度。
10. Chord、Confirming、Editing 和 Error mode 的提示完全替换普通 footer，而不是叠加。

不存在“禁用但灰显”的 footer item。提示要么对应当前可接受的下一键，要么不出现。
若配置隐藏了一个 actionable item，它仍可按下，但这是用户的显式显示策略。
`mode=hidden` 只隐藏普通 footer；确认/取消和 hard reset 等安全保留键仍必须在弹窗正文中
显示，不能由提示可见性配置移除。

典型状态：

```text
CommitList / Normal:
Ctrl+G changes | Ctrl+R refresh | Enter detail | Space select | Ctrl+C copy…

CommitList / chord prefix=Ctrl+C:
h hash | i info | m message | Esc cancel

CommitFileList / Normal:
Ctrl+G changes | Ctrl+R refresh | Enter file diff | Ctrl+C copy file…

CommitFileList / chord prefix=Ctrl+C:
n file name | a absolute path | r relative path | Esc cancel

Confirming push:
Enter confirm | Esc cancel
```

普通命令提示查询有效 keymap；安全保留 modal 则查询固定的 `ModalShortcutSet`，其 id
同时选择可调用输入 handler、footer 和全局帮助内容，不允许出现提示与真实操作不一致。

---

## 8. 日志配置

### 8.1 TOML 结构

```toml
[logging]
enabled = true
level = "info"          # error | warn | info | debug | trace
path = ""               # 空或省略：使用平台默认位置
format = "jsonl"        # 第一阶段只支持 jsonl
flush_interval_ms = 0   # 0：每条刷新；>0：按周期批量刷新
buffer_capacity = 1024  # BufWriter 字节容量
max_detail_chars = 4096
fail_on_open_error = false

[logging.rotation]
enabled = true
max_size = "5 MiB"
keep_files = 1
rotate_on_start = false

[logging.targets]
git_worker = "info"
```

配置缺失时必须复现当前行为：

```text
enabled=true
level=info
format=jsonl
flush_interval_ms=0
rotation.max_size=5 MiB
rotation.keep_files=1
```

### 8.2 日志等级语义

| Level | 记录内容 |
|---|---|
| `error` | Git job 失败、channel 关闭、日志 sink 严重错误 |
| `warn` | 安全 preflight 拒绝、自动 abort、路径 fallback、配置降级 |
| `info` | session、queued、started、completed 及耗时 |
| `debug` | 经过收敛的 response 计数、配置来源和状态转移摘要 |
| `trace` | command id、screen/focus 和 job 调度细节，不记录文本输入内容 |

target level 覆盖全局 level；未配置的 target 继承全局值。第一阶段由 `git_worker`
产生持久化事件，`app` 与 `config` target id 已保留给后续事件扩展。

无论 level 多高，以下内容始终禁止写入：

- diff 与文件正文；
- commit message 和剪贴板正文；
- remote URL、凭据、Authorization header、token；
- remote 编辑器、commit 编辑器、filter 和 typed confirmation 的原始键入；
- 完整环境变量内容。

### 8.3 刷新与缓冲

- `flush_interval_ms = 0` 保持当前每条 JSONL 写入后 `flush()` 的语义。
- 非零值允许日志 writer 批量刷新，范围建议为 `50..=60_000` ms。
- 第一阶段使用共享 `BufWriter`；`buffer_capacity` 控制 writer 的字节缓冲区，而不是事件数。
- 独立有界事件队列、按等级丢弃和 `events_dropped` 汇总尚未实现，属于后续性能扩展；
  当前事件量仅来自串行 Git worker 生命周期。
- 正常退出、日志轮转与 sink 切换前必须 flush；不默认对每条执行 `fsync`。
- `buffer_capacity` 必须设置上下限，防止配置造成无限内存增长。

### 8.4 大小与轮转

`max_size` 接受 `KiB`、`MiB`、`GiB`，统一按 1024 进制解析。轮转在写入完整 JSONL
记录前检查，因此不会拆开一行。

文件命名：

```text
pitui.jsonl       current
pitui.jsonl.1     newest backup
pitui.jsonl.2
...
pitui.jsonl.N     oldest backup
```

轮转步骤：

1. flush 当前文件；
2. 删除超过 `keep_files` 的最老文件；
3. 从大到小重命名 `.N-1 -> .N`；
4. 当前文件重命名为 `.1`；
5. 创建新的 current 文件；
6. 下一条完整 JSONL 事件写入新的 current 文件。

`keep_files = 0` 表示轮转时不保留备份；`rotation.enabled = false` 必须明确配置，
因为它可能导致文件无限增长。文件已超过上限时，默认在下一条事件前轮转；
`rotate_on_start=true` 则在 `session_started` 前主动轮转。

### 8.5 路径、权限与失败策略

- 父目录按需创建。
- 文件权限遵循平台 API 与进程 umask；Pitui 不主动放宽已有目录或文件权限。
- 显式路径打开失败时：
  - `fail_on_open_error=true`：在进入 TUI 前报错退出；
  - 默认 `false`：回退到临时目录，并在状态栏显示持久警告。
- 轮转失败时继续尝试 append 当前文件，不得让 Git 操作失败。
- 实际生效路径写入 `session_started`，并可由 `--print-effective-config` 查看。

---

## 9. Diff 默认模式配置

Commit File Diff 与 Changes Diff 继续复用同一个 diff renderer，并共享同一个 session mode。

```toml
[diff]
default_mode = "unified" # unified | side-by-side
```

规则：

- 配置缺失时默认 `unified`，与当前行为一致。
- `side-by-side` 只表示期望模式；终端宽度小于现有 140 列安全阈值时仍临时降级为
  unified 并显示说明，但不得篡改配置值或 session mode。
- 启动时用 `default_mode` 初始化 `AppState.diff_mode`，同时作用于 commit diff 和 Changes diff。
- 用户按有效 keymap 中绑定到 `diff.mode.toggle` 的按键后，只改变当前 session mode，
  不回写 `config.toml`。
- 手动 reload 不强制覆盖用户在当前 session 中已经切换的 mode；新的默认值从下一次启动
  生效，避免查看 diff 时界面突然变化。
- 非法值在进入 terminal raw mode 前报错，不静默回退。

后续可在 `[diff]` 下扩展默认 wrap、context lines 等字段，但不属于本次默认模式设计。

---

## 10. 启动与运行时生命周期

### 10.1 启动顺序

```text
parse minimal CLI
  -> locate config
  -> parse TOML
  -> merge defaults / file / env / CLI
  -> validate schema + keymap + diff + logging policy
  -> build immutable ResolvedConfig
  -> initialize BackendLogger from logging snapshot
  -> build AppState with Arc<ResolvedConfig> and runtime viewport/session state
  -> initialize terminal
  -> start Git worker and event loop
```

配置错误必须发生在 terminal raw mode 之前，输出配置文件、行列号、字段和修复建议，
退出码为 `2`。日志打开错误按 `fail_on_open_error` 处理。

### 10.2 手动重载

第一阶段只要求启动时加载。后续可以注册 `app.config.reload` 命令，但不默认占用新单键；
用户可为它配置 chord，或由未来的 Settings/Command Palette 调用。

重载必须是事务性的：

1. 在后台读取并完整校验新配置；
2. 新日志路径先成功打开；
3. Controller 在单个 action 边界交换 `Arc<ResolvedConfig>`；
4. footer 与 input 从下一帧同时使用新 generation；
5. 失败时继续使用旧 generation，并显示错误弹窗。

不使用文件 watcher，不在 tick 中检查配置 mtime。

---

## 11. 模块边界

第一阶段实际模块：

```text
src/config.rs            load/resolve、TOML model、路径、keymap 与严格校验
src/app/command.rs       CommandId registry、actionability 与默认 presentation
```

现有模块调整方向：

- `main.rs`：在 terminal 初始化前加载配置。
- `AppState`：通过 `Arc<ResolvedConfig>` 保存有效 footer/keymap 快照，并单独保存 diff
  session mode；日志实际路径和 fallback warning 也保存在 state 中供 UI 展示。
- event loop：在启动与 terminal resize 时更新 AppState 的 viewport capability，保证 input
  resolver 与 footer 对滚动边界和可执行下一键使用同一份状态。
- `tui/input.rs`：从硬编码按键 `match` 迁移为 registry + keymap resolve；文本/modal 保留专用映射。
- `tui/render.rs`：hotkey bar 从 registry 生成，不再维护按键字符串副本。
- `git/logging.rs`：接受 `ResolvedLoggingConfig`，保留 best-effort 和脱敏约束。
- `controller.rs`：仅负责 command 产生的 `Action`，不解析按键文本。

Renderer 仍保持纯函数，不读取磁盘、不加载 TOML、不修改配置。

---

## 12. 诊断命令

已实现以下启动诊断命令：

```text
pitui --config <path> [REPOSITORY ...]
pitui --no-config [REPOSITORY ...]
pitui --check-config [--config <path>]
pitui --print-config-path
pitui --print-effective-config [--config <path>]
```

`--check-config` 不初始化 terminal、不启动 Git worker。`--print-effective-config` 使用规范化的
按键和绝对日志路径；不得打印 secret。第一阶段不在输出中逐字段标记来源。

---

## 13. 示例完整配置

```toml
schema_version = 1

[ui.footer]
mode = "contextual"
max_rows = 1
show_global = true
show_alternative_bindings = false
default_visibility = "registry"
separator = " | "
overflow = "count"

[ui.footer.groups."commit.copy"]
visible = true
label = "copy…"
priority = 110

[ui.footer.groups."file.copy"]
visible = true
label = "copy file…"
priority = 110

[ui.footer.commands."app.shortcuts"]
visible = true
label = "shortcuts"
priority = 100

[ui.footer.commands."app.refresh"]
visible = true
label = "refresh"
priority = 100

[ui.footer.commands."view.changes.toggle"]
visible = true
label = "changes"
priority = 100

[ui.footer.commands."commit.copy.hash"]
visible = true
label = "hash"

[ui.footer.commands."commit.copy.info"]
visible = true
label = "info"

[ui.footer.commands."commit.copy.message"]
visible = true
label = "message"

[ui.footer.commands."file.copy.name"]
visible = true
label = "file name"

[ui.footer.commands."file.copy.absolute_path"]
visible = true
label = "absolute path"

[ui.footer.commands."file.copy.relative_path"]
visible = true
label = "relative path"

[keybindings]
chord_timeout_ms = 0

[keybindings.commands."app.refresh"]
bindings = ["Ctrl+R"]

[keybindings.commands."app.shortcuts"]
bindings = ["Ctrl+?", "?"]

[keybindings.commands."view.changes.toggle"]
bindings = ["Ctrl+G"]

[keybindings.commands."repository.pull_rebase"]
bindings = ["p"]

[keybindings.commands."repository.push"]
bindings = ["P"]

[keybindings.commands."commit.copy.hash"]
bindings = ["Ctrl+C h"]

[keybindings.commands."commit.copy.info"]
bindings = ["Ctrl+C i"]

[keybindings.commands."commit.copy.message"]
bindings = ["Ctrl+C m"]

[keybindings.commands."file.copy.name"]
bindings = ["Ctrl+C n"]

[keybindings.commands."file.copy.absolute_path"]
bindings = ["Ctrl+C a"]

[keybindings.commands."file.copy.relative_path"]
bindings = ["Ctrl+C r"]

[diff]
default_mode = "unified"

[logging]
enabled = true
level = "info"
format = "jsonl"
flush_interval_ms = 0
buffer_capacity = 1024
max_detail_chars = 4096
fail_on_open_error = false

[logging.rotation]
enabled = true
max_size = "5 MiB"
keep_files = 3
rotate_on_start = false

[logging.targets]
git_worker = "info"
```

---

## 14. 测试与验收标准

### 14.1 配置加载

- 无配置文件时默认 binding、diff、logging 与 Git 行为不变，footer 使用新的 action-only
  和 chord 逐级提示规则。
- 各平台默认路径和 `--config` / `PITUI_CONFIG` 优先级正确。
- 相对路径只相对于配置目录解析。
- 未知字段、旧 schema、非法尺寸和越界数值提供精确错误。
- diff 默认模式缺失、合法值与非法值处理正确。
- `--check-config` 不进入 alternate screen。

### 14.2 快捷键与提示

- 所有当前默认快捷键通过 registry 回归测试。
- 单键、修饰键、二段/三段 chord、替代绑定和解除绑定均可解析。
- 重叠上下文冲突被拒绝，互斥上下文可复用同一按键。
- `COMMAND_SPECS[CommandId as usize]` 与 enum 全量、顺序一致，command id 唯一且可 round-trip。
- Commits 只挂载 `Ctrl+C → h/i/m`；CommitFiles/DiffFiles 只挂载
  `Ctrl+C → n/a/r`；DiffView 不挂载 copy chord。
- 修改 binding 后 input 与当前层级 footer、popup hint 同时变化。
- root footer 只显示 chord prefix，按下第一级后才显示第二级；三段 chord 逐级揭示。
- 任何未被当前 input resolver 接受的按键都不会出现在 footer。
- `registry/all/allowlist`、command/group 可见性以及 label/priority 覆盖正确。
- 隐藏提示不解除 binding，解除 binding 后提示必然消失。
- footer 在窄终端、宽字符 label、1..3 行限制下不越界。
- modal、编辑器和 hard reset 安全键不能被配置绕过。
- `MODE_KEY_TABLES` 与 `MODAL_SHORTCUT_SETS` id 对齐；commit submission 与 Remote Add/URL
  显示并响应不同的操作集。
- `Ctrl+?` 从各 Normal focus 打开全局参考框，列出有效 binding/operation id、标记原 focus，
  滚动和关闭后恢复原 focus。
- Chord 取消、超时和未匹配输入不改变原 focus/selection。

### 14.3 Diff 配置

- 无配置时两个 diff 入口均从 unified 启动。
- `default_mode=side-by-side` 同时初始化 Commit File Diff 与 Changes Diff。
- 窄终端只做渲染降级，不改变 session mode。
- 用户切换 mode 不写配置，reload 不覆盖当前 session 选择。

### 14.4 日志

- level 与 target override 正确过滤事件。
- 自定义路径、目录创建和失败 fallback 正确。
- 每条刷新与定时刷新在退出、轮转前都不会丢失已接收事件。
- 多备份按 `.1` 到 `.N` 顺序滚动，永不拆分 JSONL 行。
- writer buffer、定时 flush 与进程退出 flush 均按配置生效。
- 从 `error` 到 `trace` 都不泄露 commit message、remote URL、凭据、diff 或编辑输入。

### 14.5 第一阶段完成定义

只有同时满足以下条件才可把第一阶段标记为已实现：

1. 默认配置下现有全部单元与集成测试继续通过；
2. input 和 footer 不再分别硬编码同一绑定；
3. footer 只展示当前可接受的下一键，所有 chord 都逐级展示；
4. 默认 diff 模式由配置初始化且两个 diff 入口行为一致；
5. 配置错误在 terminal 初始化前可定位；
6. 日志路径、级别、刷新与轮转全部由有效配置驱动；
7. README、`--help`、示例配置和验收报告同步更新；
8. 不引入自动配置轮询，不降低 Git 安全确认等级。

---

## 15. 实施顺序

```text
Phase 1  config path + TOML models + strict validation + diagnostic CLI
Phase 2  CommandRegistry + default keymap（保持行为不变）
Phase 3  configurable bindings + generic chord resolver
Phase 4  next-action-only footer + progressive chord hints + visibility policy
Phase 5  configurable diff default mode
Phase 6  configurable logging path/level/flush/rotation/retention
Phase 7  transactional manual reload（可选，不做 watcher）
Phase 8  focus-mounted callable tables + global shortcut reference
```

每个 phase 都应保持可独立回退；尤其 Phase 2 必须先证明默认 keymap 与当前行为等价，
再开放用户覆盖。
