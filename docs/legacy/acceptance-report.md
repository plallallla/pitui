# Pitui 0.1.0 MVP 验收报告

> 本报告只证明当前 0.1.0 Legacy 行为基线，可作为下一代真实 Git 集成测试和安全语义的
> 复用证据，不代表下一代内部架构。Dataset ECS 当前实现证据、缺口和阶段验收见
> [`../next-development-status.md`](../next-development-status.md)。

验收日期：2026-07-17

## 结论

[`git-tui-design.md`](git-tui-design.md) 中 Milestone 1–5 及多仓库安全操作扩展已实现。Pitui 可同时打开多个 Git 仓库，以真正分层的仓库/分支树浏览 commit、独立 Changes 三级树、changed files、两种 file diff、reflog 和 Remote Management，并通过仓库隔离的确认流程执行 fetch、pull --rebase、push、switch、cherry-pick、soft/mixed/hard reset 和 safe rebase。Remote Management 可新增 remote、归一 fetch/push URL 并为当前分支设置双向 upstream；worker 在联系网络前强制校验 URL 和分支路由一致。Changes 支持文件/分组多选、stage、unstage 和 commit。快捷键现由 `OperationId as usize -> OperationSpec` 可调用跳表按九个 focus context 精确挂载：WASD 分别负责 up/left/down/right，原有冲突操作迁移到 `W/S/A`；Commits 的 `Ctrl+C → h/i/m` 只复制 hashes/info/message，CommitFiles/FileList 的 `Ctrl+C → n/a/r` 只复制文件名/绝对路径/仓库相对路径，DiffView 不再误挂载 copy。Filter、确认、quick command、commit submission、Remote editor 等 modal 使用独占可调用输入表；`h` 帮助框只从同一 registry 生成 Global 与来源 focus 的有效 operation。Ctrl+Backtick 打开快捷命令框，`Ctrl+Space` 不绑定，输入 `help` 返回当前-focus 快捷键指南；`Ctrl+P` 从同一 resolved registry 打开可搜索操作面板。Commits 位于 Overview 宽右栏时使用两行 detailed item，补充精确到分钟的日期时间和作者，仅在存在 Git tag 时展示 tags；平移到 Commit Detail 窄左栏后恢复原有单行 compact item。顶部状态栏不再显示 view、viewing、focus。定时 Git 状态轮询已移除，任意主界面通过 `Ctrl+R` 手动刷新；Commit Files、File Diff 和 Changes Diff 统一支持 Home/End/PageUp/PageDown。Branch 列切换分支会自动刷新右侧 commits；Commit Detail 左侧切换 commit 会自动刷新右侧 metadata/files；两者都保持左侧 focus，快速连续移动只接受最新响应。浏览主链使用不回绕的 `Branches | Commits` → `Commits | Commit` → `Commit | Diff` 双栏层级：Right 越过右栏时将其复用为下一界面左栏，Left 完全对称还原。File Diff 左侧因此保留完整 commit metadata + files，而非孤立 Files 列；切换文件仍刷新右侧且不抢走 focus。版本化全局 TOML 配置已接管 command bindings、逐级 chord、action-only footer、共享 diff 默认模式和后台日志策略，并在进入 terminal 前严格校验。所有 Git worker job 另有可配置的持久化 JSONL 生命周期日志。按 Data View 分层的配置已开放基础解析：六个 View 可分别设置双栏宽度、commit density、作者/时间/tag 字段，并可覆盖任意稳定 OperationId 的绑定；输入、footer、帮助和操作面板使用相同的 view-effective keymap。

## 里程碑证据

| 里程碑 | 实现证据 | 验证证据 |
|---|---|---|
| M1 框架与状态栏 | `src/tui/mod.rs`、`src/tui/render.rs`、`src/app/state.rs` | TestBackend 渲染测试、非仓库错误测试、实际 PTY 启动/退出冒烟 |
| M2 Branch / Commit | `src/git/runner.rs`、`src/git/parser.rs`、`src/app/controller.rs`、`src/tui/render.rs` | 右栏 detailed/左栏 compact Commit renderer、Branch/Commit 自动预览、左右键无回绕 column shift 与 stale-response 测试 |
| M3 Commit Detail | commit metadata、name-status、numstat、patch hunk 解析和 changed-files renderer | root commit、rename、binary、hunk integration tests |
| M4 File Diff | 完整 Commit 栏复用、unified parser、side-by-side 对齐、文件切换且异步响应保持 focus、wrap、宽度降级 | metadata/files 保留渲染测试、真实层级左右导航与双文件 focus integration test、PTY 导航冒烟 |
| M5 可写操作 | switch/cherry-pick/reset request、确认状态机和错误弹窗 | 临时仓库写操作 integration test、typed confirmation/controller tests |
| 多仓库树 | positional repository paths、规范化 `GitModel`、轻量 `RepositoryUiState`、扁平可见树投影 | 双仓库加载/折叠/跨仓库 commit 导航 integration test |
| 独立 Changes | 全局 `Ctrl+G`；Changes → Staged/Unstaged → File 三级树；进入/返回上下文；按 group 隔离 patch | 真实临时仓库 `MM` 双分组、三类 diff、全局返回上下文 integration test；state/renderer tests |
| Diff 组件复用 | commit 与 Changes 都使用 `FileDiff`、`render_diff_panel`、unified/side-by-side、wrap 与滚动 | Changes staged 渲染测试、既有宽屏 side-by-side 测试、真实 patch integration test |
| Changes 写操作 | file/group/root checkbox 多选；tree/diff focus 均可 stage/unstage；commit message validation | 正常/unborn 仓库 stage→unstage→commit 语义测试、controller 多选端到端测试、input/renderer tests |
| Commit 复制 | Space 独立多选；`Ctrl+C → h/i/m` 二级 palette；hash/info/full-message；OSC 52 | shortcut mode/input/base64 tests；多行 message 与 semantic focus 保持 integration test |
| File 复制 | CommitFiles/FileList 的 `Ctrl+C → n/a/r`；basename、绝对路径、原始 Git 仓库相对路径；与 commit/Diff focus 隔离 | focus table/footer/input tests；真实临时仓库三种 clipboard payload integration assertions |
| 手动刷新 / 文件导航 | 无定时 status polling；主界面全局 `Ctrl+R`；文件列表与两类 diff 的 Home/End/PageUp/PageDown | 等待超过原 2s 周期仍无 job，手动刷新才读取外部修改；15 文件翻页与长 diff 边界 integration test |
| 树层级 / Unborn | `├─/└─` child connector；status 补全尚不存在 ref 的当前分支 | 空仓库 main 两行树 integration/unit/renderer tests |
| Remote sync / Reflog | 仓库节点 fetch、pull --rebase、push、reflog；确认及 job 级 cwd 路由 | 本地 bare remote fetch；真实 divergent rebase pull/push；dirty rejection；conflict auto-abort integration tests |
| Remote Management | `o` 独立界面；add/shared-URL/upstream 确认流程；`fetch/pull/push` 执行时 policy preflight；URL 日志收敛 | 真实 bare remote 新增、无远程分支时设 upstream 并 plain push；split URL 与 split branch routing 均在联系 remote 前拒绝，修复后通过 |
| Reset 安全分级 | soft/mixed/hard mode chooser；hard warning + hash 两阶段确认 | 三种真实 reset 语义、reflog target reset、hard controller end-to-end tests |
| Safe Rebase | controller + worker 双重前置检查、确认、冲突检测、仅自动 abort 本次操作 | clean success、dirty rejection、真实冲突恢复、既存 rebase 不被 abort integration tests |
| 全局配置 / Diff 默认模式 | `src/config.rs` 严格 TOML、CLI/env/path 优先级和诊断命令；`[diff].default_mode` 初始化共享 session mode | 合法/非法模式、effective config、CLI exit code、两个 diff 入口初始化 tests |
| Focus Shortcut Tables / Help | `OPERATION_SPECS` 函数指针跳表、WASD navigation、九个 `FocusKind`、commit/file 互斥 chord；`MODE_KEY_TABLES` modal handler；`h` 当前-focus 帮助框；Ctrl+Backtick prompt，Ctrl+Space unbound | jump-table 顺序/完整性、WASD 与大写冲突迁移、current-focus help 隔离、modal id 对齐、binding/footer/help 一致、prompt validation/help 跳转、scroll/restore tests |
| 后台日志 | 平台默认路径与 `PITUI_LOG`；配置开关/level/target/flush/writer buffer/rotation/retention；queued/started/completed JSONL；敏感 payload 收敛 | JSON escaping、commit message redaction、level、定时 flush、rotate-on-start、多备份和 strict-open tests；success/failure lifecycle integration test |
| GitHub / License | MIT License、跨平台 CI、Dependabot、Issue Forms、PR template、贡献与安全策略 | Cargo license metadata、GitHub YAML 静态检查、完整质量门禁 |

## 架构约束

- `std::process::Command::new("git")` 只存在于 `src/git/runner.rs`。
- Renderer 的入口和子函数只接收 `&AppState`，不保存或修改业务状态。
- `GitModel` 以 typed ID 规范化 Repository→Branch→Commit→File；GitResponse 先写 Model，不存在第二份 branch/commit/file 详情 cache。
- `FocusContext/FocusPath` 是操作、视图与数据需求真值；`ViewProjection` 完全由 `FocusKind + FocusRole` 派生。
- Focus/View 改变统一派生 `DataRequirement` 并依据 `Resource<T>` 去重加载。
- Input Mapper 只生成 `Action`。
- Normal/Chord 输入、底部提示和全局帮助读取同一个 `OPERATION_SPECS` 可调用 registry 与同一份 `Arc<ResolvedConfig>`；focus context 先过滤挂载表，再调用动态 actionability 函数。
- modal 输入通过 `MODE_KEY_TABLES` 独占；其 footer/help 文字来自对应 `MODAL_SHORTCUT_SETS`，普通 focus 命令不会穿透。
- Clipboard payload 由 Application 生成，TUI 只负责 OSC 52 编码与终端写入；Commits 的统一多选集合同时服务 hash 复制与 cherry-pick，后者在集合为空时不可执行。
- Git Worker 通过 request/response channel 通信，不依赖 TUI；每个 job 显式携带 cwd，不存在全局单仓库路径。
- GitCommandBus 在发送和执行边界记录每个 job 的 queued/started/completed；响应发布前 flush completion，channel 断开也有错误记录。
- pending job 保存请求上下文，latest job id 防止过期响应覆盖当前视图。
- 所有 Git 命令使用 argv，不通过 shell 拼接。
- Git 输出进入终端前会清理控制字符和 bidi override。

## 自动验证

以下命令全部通过：

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
cargo test --doc
```

测试结果：

```text
unit tests:             89 passed
config CLI tests:        3 passed
Git integration tests:  36 passed
doc tests:               0 failed
```

集成测试使用真实 Git 和独立临时仓库，覆盖：

- clean/staged/modified/untracked/conflicted 状态；
- 普通、unborn 和 detached HEAD 仓库；
- branch、commit、root commit、rename、binary file；
- commit detail、numstat、hunk 和 file diff；
- File Diff 左侧上下切换双文件时响应刷新右侧但 focus 持续保持 FileList；
- Commit Files、File Diff 左侧文件列表、File Diff 右侧内容和 Changes Diff 对 Home/End/PageUp/PageDown 的首尾/翻页语义；文件列表翻页只产生一个最终 diff job；
- 超过原 2 秒轮询间隔后 Tick 仍不提交 Git 请求，只有 `Ctrl+R`/`RefreshRepository` 才读取外部工作区变化；
- 异步 controller 主浏览三视图状态转移；
- 快速切换分支时的 stale response 丢弃；
- Branch 列用上下键选择分支时自动刷新右侧 commits 且保持 BranchList focus；Commit Detail 左侧 Commits 上下切换时自动刷新右侧详情且保持 CommitList focus；
- Overview 右侧 Commits 显示 date/author/tags 的两行 detailed item，Commit Detail 左侧继续使用单行 compact item；
- `Branches | Commits`、`Commits | Commit(metadata + files)`、`Commit(metadata + files) | Diff` 间的双向 column shift；最左/最右边界不回绕，异步加载完成前 Left 返回也不会被旧 response 重新打开；
- 非 Git 目录错误显示与关闭；
- switch、cherry-pick、reset 写操作；
- 多仓库树加载、折叠、切换及所有请求的仓库隔离；
- 选中仓库通过本地 bare remote 执行 fetch 并刷新 remote tracking branch；
- repository 节点确认执行 `git pull --rebase` 后重放本地 commit、plain push 到已配置 upstream、dirty pull 前置拒绝及 pull conflict 自动 abort；
- Remote Management 从空配置新增共享 URL remote、设置当前分支 fetch/push upstream，并使用尚不存在的远程分支完成 plain push；
- 显式 `pushurl` 与 fetch URL 不同时 fetch/pull/push 均被本地 preflight 拒绝；`e`/SetRemoteUrl 移除拆分 URL，分支 fetch remote 与 push remote 不同时同样拒绝，`u`/SetUpstreamRemote 修复后可推送；
- staged、unstaged、untracked 文件发现、Changes 三级树、`MM` 跨组双节点和按边界加载 diff，以及空仓库 `main` 子节点补全；
- Changes file/group 多选、tree/diff focus 下 stage/unstage、正常与 unborn repository 安全 unstage、commit message validation 和真实 commit；
- 从 file diff 全局进入 Changes 后恢复原 semantic focus；Commits 的 `Ctrl+C → h/i/m` 与文件列的 `Ctrl+C → n/a/r` 二级表严格隔离，复制 commit 多选 hashes/info/完整 message 以及文件 basename/绝对路径/相对路径；Diff focus 使用退出 fallback；
- reflog 加载、渲染、返回时 stale response 丢弃及以 reflog entry 为 reset target；
- soft/mixed/hard reset 语义和 hard 双阶段确认；
- safe rebase 成功、worker 执行时脏工作区拒绝、真实冲突提示与自动 `git rebase --abort` 恢复；既存 rebase 会被拒绝且保持不变。
- 后台 JSONL 对成功/失败 job 的完整生命周期、cwd/operation/status/duration 记录，以及日志轮转和 commit message redaction。

配置专项测试另覆盖严格 schema/未知字段、非法 command 与按键冲突、配置相对日志路径、
默认 unified 与 side-by-side 初始化、input/footer/help actionability 一致、focus-mounted chord
逐级提示、modal operation-set 对齐、全局快捷键帮助框、footer 多行布局、日志
level/flush/rotation/retention，以及 `--check-config` / `--print-effective-config`
不会进入 alternate screen；非法配置以退出码 `2` 结束。

仓库发布配套包括 MIT `LICENSE`、README vibe-coding 声明、Linux/macOS/Windows CI、Cargo/GitHub Actions Dependabot、结构化 Bug/Feature Issue Forms、PR 模板、贡献指南和私密漏洞报告策略。

解析器单元测试另外验证了原始非 UTF-8 Git path 字节和 quoted path 的保留；支持此类文件名的 Unix 文件系统还会启用完整 argv round-trip integration test。

## 终端冒烟验证

在独立临时 Git 仓库中通过真实 PTY 完成：

1. 使用两个 repository positional arguments 启动；树中同时显示 alpha/beta 仓库及各自分支。
2. active 仓库自动加载 HEAD 和 commits。
3. 在 alpha 仓库节点按 `g` 进入真实 reflog，显示 selector/action/message/full hash。
4. 从 reflog 按 `R` 打开 reset mode chooser，依次验证 `h` warning confirmation 1/2 和 short-hash confirmation 2/2，随后取消而未执行写操作。
5. `q` 退出并恢复 alternate screen、鼠标模式和光标。

## 明确不在 0.1.0 范围内

- 逐行 / 逐 hunk partial staging
- stash 管理
- interactive rebase todo 编辑
- merge conflict editor
- blame
- 内置文件编辑器

cherry-pick 产生冲突后，Pitui 会展示 Git 错误和 conflicted 状态；继续或中止冲突操作由用户在标准 Git CLI 中完成。
