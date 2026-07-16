# Pitui 0.1.0 MVP 验收报告

验收日期：2026-07-16

## 结论

`docs/git-tui-design.md` 中 Milestone 1–5 及多仓库安全操作扩展已实现。Pitui 可同时打开多个 Git 仓库，以真正分层的仓库/分支树浏览 commit、独立 Changes 三级树、changed files、两种 file diff 和 reflog，并通过仓库隔离的确认流程执行 fetch、switch、cherry-pick、soft/mixed/hard reset 和 safe rebase。Changes 支持文件/分组多选、stage、unstage 和 commit；Commits 支持独立多选后复制完整 hashes 及复制当前 commit info。所有 Git worker job 另有持久化 JSONL 生命周期日志。

## 里程碑证据

| 里程碑 | 实现证据 | 验证证据 |
|---|---|---|
| M1 框架与状态栏 | `src/tui/mod.rs`、`src/tui/render.rs`、`src/app/state.rs` | TestBackend 渲染测试、非仓库错误测试、实际 PTY 启动/退出冒烟 |
| M2 Branch / Commit | `src/git/runner.rs`、`src/git/parser.rs`、`src/app/controller.rs` | `loads_repository_branches_commits_details_and_diffs`、controller 主浏览三视图测试 |
| M3 Commit Detail | commit metadata、name-status、numstat、patch hunk 解析和 changed-files renderer | root commit、rename、binary、hunk integration tests |
| M4 File Diff | unified parser、side-by-side 对齐、文件切换、wrap、宽度降级 | side-by-side 单元/渲染测试、真实 file diff integration test、PTY 导航冒烟 |
| M5 可写操作 | switch/cherry-pick/reset request、确认状态机和错误弹窗 | 临时仓库写操作 integration test、typed confirmation/controller tests |
| 多仓库树 | positional repository paths、`RepositoryState`、扁平可见树节点、active repository context | 双仓库加载/折叠/跨仓库 commit 导航 integration test |
| 独立 Changes | 全局 `Ctrl+G`；Changes → Staged/Unstaged → File 三级树；进入/返回上下文；按 group 隔离 patch | 真实临时仓库 `MM` 双分组、三类 diff、全局返回上下文 integration test；state/renderer tests |
| Diff 组件复用 | commit 与 Changes 都使用 `FileDiff`、`render_diff_panel`、unified/side-by-side、wrap 与滚动 | Changes staged 渲染测试、既有宽屏 side-by-side 测试、真实 patch integration test |
| Changes 写操作 | file/group/root checkbox 多选；tree/diff focus 均可 stage/unstage；commit message validation | 正常/unborn 仓库 stage→unstage→commit 语义测试、controller 多选端到端测试、input/renderer tests |
| Commit 复制 | Space 独立多选；hash/info 格式化；OSC 52 terminal clipboard | selection/state、base64、controller clipboard payload tests |
| 树层级 / Unborn | `├─/└─` child connector；status 补全尚不存在 ref 的当前分支 | 空仓库 main 两行树 integration/unit/renderer tests |
| Fetch / Reflog | job 级 cwd 路由、仓库节点 fetch/reflog、Reflog screen | 本地 bare remote fetch、reflog parser/renderer/真实仓库 integration tests |
| Reset 安全分级 | soft/mixed/hard mode chooser；hard warning + hash 两阶段确认 | 三种真实 reset 语义、reflog target reset、hard controller end-to-end tests |
| Safe Rebase | controller + worker 双重前置检查、确认、冲突检测、仅自动 abort 本次操作 | clean success、dirty rejection、真实冲突恢复、既存 rebase 不被 abort integration tests |
| 后台日志 | 平台默认路径与 `PITUI_LOG`；queued/started/completed JSONL；5 MiB 轮转；敏感 payload 收敛 | JSON escaping、commit message redaction、rotation unit tests；success/failure lifecycle integration test |
| GitHub / License | MIT License、跨平台 CI、Dependabot、Issue Forms、PR template、贡献与安全策略 | Cargo license metadata、GitHub YAML 静态检查、完整质量门禁 |

## 架构约束

- `std::process::Command::new("git")` 只存在于 `src/git/runner.rs`。
- Renderer 的入口和子函数只接收 `&AppState`，不保存或修改业务状态。
- Input Mapper 只生成 `Action`。
- Clipboard payload 由 Application 生成，TUI 只负责 OSC 52 编码与终端写入；复制集合与 cherry-pick queue 隔离。
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
unit tests:        37 passed
integration tests: 24 passed
doc tests:          0 failed
```

集成测试使用真实 Git 和独立临时仓库，覆盖：

- clean/staged/modified/untracked/conflicted 状态；
- 普通、unborn 和 detached HEAD 仓库；
- branch、commit、root commit、rename、binary file；
- commit detail、numstat、hunk 和 file diff；
- 异步 controller 主浏览三视图状态转移；
- 快速切换分支时的 stale response 丢弃；
- 非 Git 目录错误显示与关闭；
- switch、cherry-pick、reset 写操作；
- 多仓库树加载、折叠、切换及所有请求的仓库隔离；
- 选中仓库通过本地 bare remote 执行 fetch 并刷新 remote tracking branch；
- staged、unstaged、untracked 文件发现、Changes 三级树、`MM` 跨组双节点和按边界加载 diff，以及空仓库 `main` 子节点补全；
- Changes file/group 多选、tree/diff focus 下 stage/unstage、正常与 unborn repository 安全 unstage、commit message validation 和真实 commit；
- 从 file diff 全局进入 Changes 后恢复原 screen/focus；commit 多选 hashes 与当前 commit info clipboard payload；
- reflog 加载、渲染、返回时 stale response 丢弃及以 reflog entry 为 reset target；
- soft/mixed/hard reset 语义和 hard 双阶段确认；
- safe rebase 成功、worker 执行时脏工作区拒绝、真实冲突提示与自动 `git rebase --abort` 恢复；既存 rebase 会被拒绝且保持不变。
- 后台 JSONL 对成功/失败 job 的完整生命周期、cwd/operation/status/duration 记录，以及日志轮转和 commit message redaction。

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
