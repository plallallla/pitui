# Pitui 按 Data View 分层的配置设计

> 状态：**Schema v2 设计稿，尚未实现**。当前可用的 Schema v1、快捷键、footer、diff 与日志
> 配置以 [`global-configuration-design.md`](global-configuration-design.md) 和
> [`config.example.toml`](config.example.toml) 为准。

## 1. 目标

下一阶段配置不再只处理“全局快捷键”，而是把 UI 拆成以下四层：

```text
Global
  └─ Data View（Branch Overview / Commit Detail / File Diff / ...）
       └─ Panel（branch.tree / overview.commits / ...）
            ├─ Fields：显示哪些数据、顺序、格式与详细程度
            └─ Operations：该 panel 可执行哪些操作、绑定、提示与优先级
```

必须满足：

1. 同一种数据在不同 View 中可以采用不同密度。例如 Overview 右侧 Commit 显示两行，
   Commit Detail 左侧 Commit 只显示一行。
2. 操作通过稳定 `CommandId` 配置，不允许 TOML 注入 shell 或 Git 命令。
3. `h` 帮助只读取 Global 与来源 focus 的有效 Operations，不显示其他 View 的快捷键。
4. 字段、绑定、footer 与帮助使用同一份 resolved view model，不能各维护一套字符串。
5. hard reset 双确认、safe rebase 自动 abort、remote 同 fetch/push URL 等安全规则不可配置关闭。

## 2. 标识体系

| 层级 | 稳定标识 | 当前代码映射 |
|---|---|---|
| View | `branch_overview`、`commit_detail`、`file_diff`、`reflog`、`changes`、`remotes` | `Screen` |
| Panel | `branch_tree`、`commits`、`commit_files`、`diff` 等 | `FocusPanel` |
| Shortcut context | `branch.tree`、`overview.commits` 等 | `ShortcutContext` |
| Operation | `branch.switch`、`commit.copy.info` 等 | `CommandId` |
| Field | `subject`、`author`、`tags`、`path` 等 | 各 domain model 的只读字段 |

配置只接受 registry 中存在的标识。重命名必须通过 schema migration，不能静默忽略。

## 3. 全局可配置选项清单

下表区分“当前 v1 已实现”和“v2 计划”。

| 配置组 | 选项 | 状态 | 说明 |
|---|---|---|---|
| 根 | `schema_version` | v1 已实现 | v2 View 配置启用后升级为 `2` |
| `general` | `timezone` | v2 | `commit-offset`、`local`、`utc` |
| `general` | `date_time_format` | v2 | 全局默认时间格式；View 可覆盖 |
| `general` | `empty_value`、`truncate_marker` | v2 | 空值与截断标记 |
| `ui` | `theme`、`unicode`、`borders` | v2 | 主题、树连接符与边框风格 |
| `ui.layout` | `detail_breakpoint`、`side_by_side_breakpoint` | v2 | detailed layout 与双栏 diff 阈值 |
| `ui.status` | `visible`、`fields.*`、`separator` | v2 | 状态栏字段及不同宽度下的顺序 |
| `ui.footer` | mode、行数、替代绑定、overflow、separator | v1 已实现 | action-only footer |
| `ui.footer.commands/groups` | visible、label、priority | v1 已实现 | 全局 operation/chord 展示覆盖 |
| `ui.help` | `show_operation_id`、`show_alternatives`、`height_percent` | v2 | scope 固定为 `current-focus`，不可改为 all views |
| `ui.command_prompt` | `show_available_commands`、`max_input_chars` | v2 | 仍只执行内部 prompt command registry |
| `keybindings` | `chord_timeout_ms` | v1 已实现 | chord 超时；0 表示不自动超时 |
| `keybindings.commands` | `bindings` | v1 已实现 | operation 的全局默认覆盖 |
| `diff` | `default_mode` | v1 已实现 | unified / side-by-side |
| `diff` | line numbers、wrap、context、whitespace、tab width | v2 | View 可覆盖其中的展示项 |
| `data_limits` | commits、reflog、files、message chars | v2 | 读取与渲染上限，必须有安全边界 |
| `logging` | enable、level、path、flush、buffer、detail chars | v1 已实现 | 后台 JSONL 日志 |
| `logging.rotation` | enable、size、保留数量、启动轮转 | v1 已实现 | 日志轮转 |
| `logging.targets` | app、config、git worker level | v1 已实现 | target override |

### 3.1 状态栏默认字段

状态栏不再显示 `view`、`viewing`、`focus`，v2 也不把这三个字段列入默认可选字段。推荐配置：

```toml
[ui.status]
visible = true
separator = " | "
compact_fields = ["repository", "branch", "operation", "changes", "tracking"]
normal_fields = ["repository", "branch", "operation", "changes", "tracking", "selection"]
wide_fields = ["repository", "branch", "head", "commit", "file", "operation", "changes", "tracking", "selection"]
```

字段 allowlist：`repository`、`branch`、`head`、`commit`、`file`、`operation`、`changes`、
`tracking`、`selection`、`loading`。

### 3.2 全局默认快捷键

```toml
[keybindings.commands."navigation.up"]
bindings = ["w", "Up", "k"]

[keybindings.commands."navigation.left"]
bindings = ["a", "Left"]

[keybindings.commands."navigation.down"]
bindings = ["s", "Down", "j"]

[keybindings.commands."navigation.right"]
bindings = ["d", "Right", "l"]

[keybindings.commands."app.shortcuts"]
bindings = ["h"]

[keybindings.commands."app.command_prompt"]
bindings = ["Ctrl+`"]
```

WASD 占用普通模式导航后，冲突操作默认迁移为：`S` branch switch、`S` stage、`W` wrap、
`A` add remote。它们位于互斥 context，因此大写 `S` 可以安全复用。

## 4. View 通用 Schema

每个 View 共享布局与 panel 外壳，但字段集合和 operation allowlist 不同：

```toml
[views.<view_id>]
enabled = true
left_width_percent = 36

[views.<view_id>.panels.<panel_id>]
visible = true
density = "compact" # compact | normal | detailed
fields = ["..."]    # 有序 allowlist
omit_empty = ["..."]
max_rows_per_item = 1

[views.<view_id>.panels.<panel_id>.operations."<command_id>"]
enabled = true
bindings = ["..."]
show_in_footer = true
label = "..."
priority = 90
```

Operation 字段全部可省略。省略时依次继承：

```text
compiled CommandSpec default
  -> [keybindings.commands] / [ui.footer.commands|groups]
  -> views.<view>.panels.<panel>.operations.<command>
  -> runtime actionability（selection / pending job / scroll boundary）
```

`enabled=false` 只在这个 panel 禁用 operation；不会删除 registry，也不会影响其他 View。

## 5. Branch Overview

### 5.1 `branch_tree` panel

可配置数据：

| 类别 | Fields / options |
|---|---|
| Repository | `name`、`path`、`current_branch`、`head`、`working_tree_counts`、`tracking` |
| Branch | `name`、`kind`、`current`、`upstream`、`ahead_behind` |
| Tree | `show_connectors`、`show_remote_branches`、`show_empty_repository`、sort、filter fields |

允许的主要 Operations：

```text
repository.activate
repository.fetch
repository.pull_rebase
repository.push
repository.remotes.open
repository.reflog.open
branch.switch
branch.rebase
list.filter
navigation.*
focus.*
```

### 5.2 `commits` panel

默认 detailed 两行显示：

```toml
[views.branch_overview.panels.commits]
density = "detailed"
fields = ["short_hash", "subject", "authored_at", "author", "tags"]
omit_empty = ["tags"]
date_time_format = "%Y-%m-%d %H:%M"
max_subject_lines = 1
show_decorations = false
```

允许的主要 Operations：`commit.open_detail`、`commit.toggle_selection`、
`commit.cherry_pick.selected`、`commit.reset`、
`commit.copy.hash/info/message`、`list.filter`、`navigation.*`、`focus.*`。

## 6. Commit Detail

### 6.1 `commits` panel

同一 Commit 数据在这里默认 compact：

```toml
[views.commit_detail.panels.commits]
density = "compact"
fields = ["short_hash", "subject", "decorations"]
max_rows_per_item = 1
```

Operations 与 Overview Commits 基本相同，但配置 scope 独立，可使用不同绑定和 footer label。

### 6.2 `commit_files` panel

可配置字段：

```text
Commit metadata: hash, author, author_email, authored_at, committer,
                 committer_email, committed_at, decorations, message
Changed file:    status, path, old_path, additions, deletions, binary, hunk_summary
```

示例：

```toml
[views.commit_detail.panels.commit_files]
density = "normal"
metadata_fields = ["hash", "author", "authored_at", "message"]
file_fields = ["status", "path", "additions", "deletions"]
show_hunk_summary = true
message_max_lines = 6
```

Operations：`commit.file.toggle_expanded`、`commit.file.open_diff`、
`file.copy.name/absolute_path/relative_path`、`navigation.*`、`focus.*`。

## 7. File Diff

### 7.1 `files` panel

复用完整 Commit + changed files 数据，但可独立配置 metadata/file fields、message 行数与宽度。

### 7.2 `diff` panel

```toml
[views.file_diff.panels.diff]
default_mode = "unified"       # 可覆盖全局 diff.default_mode
show_line_numbers = true
wrap = false
tab_width = 4
context_lines = 3
show_whitespace = "none"       # none | trailing | all
binary_summary = true
```

Operations：`diff.mode.toggle`、`diff.wrap.toggle`、`file.next`、`file.previous`、
`navigation.page_up/page_down/home/end/left`、`focus.*`。Diff 内容 focus 不挂载 copy chord。

## 8. Changes

### 8.1 `changes_tree` panel

```toml
[views.changes.panels.changes_tree]
group_order = ["staged", "unstaged"]
show_empty_groups = true
fields = ["selection", "status", "path", "old_path"]
duplicate_mixed_files = true # MM 必须分别表达 index/worktree 边界；不可设为 false
show_counts = true
```

Operations：`changes.activate`、`changes.toggle_selection`、`changes.stage`、
`changes.unstage`、`changes.commit`、`navigation.*`、`focus.*`。

### 8.2 `diff` panel

继承全局 diff 配置，可单独设置 mode、line numbers、wrap、tab width 与 staged/unstaged 标题字段。
stage/unstage operation 仍根据选中 group 做安全路由，配置不能改变 Git 边界。

## 9. Reflog

```toml
[views.reflog.panels.entries]
density = "normal"
fields = ["short_hash", "selector", "action", "message", "author", "authored_at"]
date_time_format = "%Y-%m-%d %H:%M"
limit = 300
message_max_lines = 2
```

Operations：`commit.reset`、`navigation.up/down/page_up/page_down/home/end`、
`navigation.back`、全局 refresh/help/changes/command prompt。

## 10. Remote Management

```toml
[views.remotes.panels.remotes]
list_fields = ["name", "upstream", "push_target", "url_health"]
detail_fields = ["fetch_urls", "push_urls", "current_branch_routing"]
redact_url_userinfo = true
show_split_url_warning = true
```

Operations：`remote.add`、`remote.set_shared_url`、`remote.set_upstream`、
`navigation.up/down/home/end/back` 与 Global operations。URL 脱敏、fetch/push 一致性和
branch routing 检查不能被配置关闭。

## 11. Overlay Views

### 11.1 Shortcut Help

```toml
[ui.help]
show_operation_id = true
show_alternatives = true
height_percent = 90
```

scope 固定为：

```text
Global operations + originating ShortcutContext operations
```

不提供 `all-views` 开关，避免再次把帮助框变成无法阅读的全量手册。

### 11.2 Command Prompt

```toml
[ui.command_prompt]
show_available_commands = true
max_input_chars = 256
```

只允许 `PROMPT_COMMAND_SPECS` 中的命令。不得配置任意 shell、Git argv 或脚本。

### 11.3 Safety / Editor Modals

可以配置尺寸、颜色、说明文本是否换行，但以下按键与流程保留：确认 Enter、取消 Esc、
hard reset 精确哈希、remote/commit/filter 文本输入、rebase conflict auto-abort。

## 12. View Operation 覆盖示例

```toml
schema_version = 2

[views.branch_overview.panels.branch_tree.operations."branch.switch"]
bindings = ["S"]
show_in_footer = true
label = "switch"

[views.changes.panels.changes_tree.operations."changes.stage"]
bindings = ["S"]
label = "stage"

[views.file_diff.panels.diff.operations."diff.wrap.toggle"]
bindings = ["W"]
label = "wrap"

[views.commit_detail.panels.commits.operations."commit.copy.info"]
bindings = ["Ctrl+C i"]
label = "info"

[views.branch_overview.panels.commits.operations."commit.copy.info"]
enabled = false
```

最后一项只关闭 Overview Commits 的 copy-info，不影响 Commit Detail Commits。

## 13. 校验与安全规则

启动进入 raw mode 前必须完成：

1. View、Panel、Field、CommandId 全部存在且层级匹配。
2. `fields` 顺序去重；format、行数、百分比、limit 在合法范围内。
3. Operation 必须已挂载到该 `ShortcutContext`，不能跨 panel 注入。
4. 合并全局与 View override 后，按 exact context 检测单键/chord 冲突。
5. Global `app.quit` 至少保留一个不被 chord 遮蔽的路径。
6. `Ctrl+Space` 默认无绑定；用户显式配置时仍执行正常冲突校验。
7. 安全 modal 的保留键和 Git 安全策略拒绝 override。
8. `--print-effective-config` 输出合并后的每个 View/Panel/Field/Operation，便于诊断继承来源。

配置错误必须带完整路径，例如：

```text
views.changes.panels.changes_tree.operations."changes.stage".bindings:
binding conflict `s` with `navigation.down` in context changes.tree
```

## 14. 代码落点

```text
src/config.rs
  RawViewConfig / ResolvedViewConfig / validation / schema migration

src/app/command.rs
  ViewId + PanelId + CommandId/ShortcutContext registry（handler 仍由程序定义）

src/tui/render.rs
  只消费 typed ResolvedPanelConfig，不解析 TOML 字符串

src/tui/input.rs
  继续通过 resolved command table 解析，不新增手写 View key match
```

建议 resolved 结构：

```rust
struct ResolvedViewConfig {
    layout: ResolvedViewLayout,
    panels: HashMap<PanelId, ResolvedPanelConfig>,
}

struct ResolvedPanelConfig {
    density: Density,
    fields: Vec<ResolvedField>,
    operations: HashMap<CommandId, ResolvedOperationPresentation>,
}
```

## 15. 分阶段实现

1. **Schema v2 + typed resolver**：只解析、校验、打印 effective config，不改变 UI。
2. **Status / Branch Overview / Commit Detail**：先实现字段顺序、density、时间格式和布局比例。
3. **File Diff / Changes / Reflog / Remotes**：补齐每个数据 View 的专属字段。
4. **Scoped Operations**：实现 View/Panel 绑定、footer label/priority 覆盖和冲突诊断。
5. **运行时 reload（可选）**：仅在完整校验成功后原子替换 resolved generation。

每阶段都必须保证无配置时保持当前默认 UI、WASD、当前-focus 帮助和 Git 安全语义。

## 16. 验收矩阵

- 同一 Commit 在 Overview/Detail 使用不同 fields 与 density，互不串配置。
- 无 tag 且 `omit_empty=["tags"]` 时不出现 `Tags:`；时间格式可覆盖且默认精确到分钟。
- `h` 只显示 Global + 来源 focus，切 focus 后立即使用另一 operation 表。
- 同一 CommandId 可在两个互斥 panel 使用不同绑定；同 context 冲突启动前报错。
- 禁用某 panel operation 后 input、footer、help 三处同时消失。
- status 的 compact/normal/wide 字段顺序正确，默认永不显示 view/viewing/focus。
- unknown View/Panel/Field/Operation、非法比例/limit/format 均被拒绝。
- hard reset、safe rebase、remote URL/routing 等安全不变量无法通过配置关闭。
