# Hotkey Profile

Pitui 的语义功能、Renderer、Operation System 和 Git argv 都由 Rust 编译实现。外部配置的唯一行为边界是
Hotkey profile：它只能给已经属于某个 Operation Set 的 `OperationId` 替换按键。

## 启用

```bash
PITUI_HOTKEY_CONFIG=/absolute/path/hotkeys.toml pitui /path/to/repository
```

未设置 `PITUI_HOTKEY_CONFIG` 时使用内置默认表。Profile 只在启动时读取，不进行自动刷新或运行时 reload。

## 格式

```toml
version = 1

[global]
"global.help" = ["h"]
"global.refresh" = ["ctrl+r"]

[datasets.commits]
"copy.commit.hash" = ["ctrl+c h"]
"copy.commit.info" = ["ctrl+c i"]
"copy.commit.message" = ["ctrl+c m"]

[datasets.files]
"copy.file.name" = ["ctrl+c n"]
```

- `[global]` 只能引用内置 Global Operation Set 中已有的 ID。
- `[datasets.<template-id>]` 只能引用该 Dataset Template 已声明的 Operation。
- 未出现的 Operation 保留默认绑定。
- `[]` 只解绑 Hotkey，Operation 仍可出现在 Command Palette 中。
- 同一个 Operation 可以配置多个按键字符串。
- 文件采用有意收窄的 TOML 子集：一行一个赋值、字符串数组、不支持多行数组。这样可以保持解析边界小且严格。
- Profile 在副本上完成全部校验后才原子替换默认表；失败不会留下部分覆盖。

## 按键字符串

单次按键示例：

```text
h
ctrl+r
shift+s
home
page-down
```

Chord 使用空格分隔每一级：

```text
ctrl+c h
ctrl+x shift+h
```

支持的 modifier：`ctrl`/`control`、`alt`、`shift`、`super`/`cmd`。

支持的命名键：`up`、`down`、`left`、`right`、`home`、`end`、`pageup`/`page-up`、
`pagedown`/`page-down`、`enter`、`escape`/`esc`、`space`、`backspace`、`tab`。此外可直接使用单个字符。

## 安全与冲突

- 配置不能声明新的 Command/Operation，不能指定函数名、shell 字符串或 Git 参数。
- 未知 Template、越权 Operation、非法 modifier/key、空 sequence、重复 sequence 和 profile 内前缀歧义会返回
  `HotkeyProfileError` 并阻止启动。
- 最终有效 Operation Set 仍由 ECS resolver 按当前 Active Dataset 和 Availability 规则生成；footer、Help、
  Command Palette 和按键执行共用这一份结果。
