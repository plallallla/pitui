# Contributing to Pitui

感谢你改进 Pitui。项目接受人工编写和 AI / vibe-coding 辅助的贡献；无论代码如何产生，提交者都必须理解、审阅并验证最终变更。

## 开始之前

1. 对较大功能先提交 Feature request，说明 UI、快捷键、Git 命令和失败恢复策略。
2. 安全问题不要提交公开 Issue；请遵循 [`SECURITY.md`](SECURITY.md)。
3. 保持功能范围克制，避免把 renderer、input mapper 与 Git 执行耦合。

## 本地开发

需要支持 Rust 2024 edition 的 stable Rust，以及可从 `PATH` 调用的 Git。

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets
```

真实 Git 写操作测试必须只使用 `tempfile` 创建的临时仓库，禁止依赖或修改开发者现有仓库。

## 设计与安全规则

- 只有 `src/git/runner.rs` 可以启动 `git` 进程。
- 命令必须使用 argv，不得拼接 shell 命令字符串。
- 每个异步 job 必须携带明确的 repository cwd/context，过期响应不得覆盖当前仓库状态。
- 每个新增的 `GitRequest` 都必须在后台 JSONL 日志中有稳定的 operation 名称，并覆盖 queued/started/completed 生命周期；不得把 diff、文件内容或 commit message 写入日志。
- 新增破坏性操作时，必须给出风险说明、确认流程、失败状态和恢复策略。
- Remote write 测试只能使用临时 bare repository；pull 行为必须显式保持 `--rebase`，push 不得猜测或自动创建 upstream。
- Remote Management 必须在联系 remote 前拒绝拆分的 fetch/push URL 和分支路由；测试 URL 只能指向临时本地仓库，后台 operation metadata 不得记录 URL。
- Git 元信息、路径和 diff 在进入终端前必须经过控制字符清理。
- parser、controller 状态转移和真实临时仓库行为都应有针对性测试。

## Pull Request

- PR 保持单一目的，并说明用户可见行为与安全影响。
- 如果使用了 AI/vibe coding，请说明生成范围以及你完成的审阅和验证。
- 提交 PR 即表示你有权贡献相关内容，并同意其按项目的 [MIT License](LICENSE) 发布。
