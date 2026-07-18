# Contributing to Pitui

感谢你改进 Pitui。项目接受人工编写和 AI / vibe-coding 辅助的贡献；无论代码如何产生，提交者
都必须理解、审阅并验证最终变更。

## 本地开发

需要支持 Rust 2024 edition 的 Rust 1.95+，以及可从 `PATH` 调用的 Git。

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets
cargo test --workspace --doc
```

真实 Git 写操作测试必须只使用 `tempfile` 创建的临时仓库，禁止依赖或修改开发者现有仓库。

## 架构规则

- `pitui-core` 只保存纯值类型和算法。
- `pitui-data` 保存 Dataset、Context、Operation 和 Render 的类型化数据协议。
- 生产代码只有 `pitui-git` 可以启动 `git` 进程；其他包只能产生 typed `GitCommandData`。
  测试代码可以调用 Git，但只能用于构造和检查 `tempfile` 临时仓库。
- `pitui-ecs` 负责系统执行、Dataset 生命周期、不变量、Context reconcile 和 Projection。
- `pitui-tui` 只能把终端事件转换为 `InputIntent`，并渲染不可变 `UiFrame`。
- Input Listener 不得查询 ECS World；快捷键必须由 Operation Executor 在当前 Active Dataset 的
  `ResolvedOperationSet` 缓存中解析。
- Dataset Template 绑定 `OperationId` 及自己的 `OperationHotkeyTable`；可执行函数只能注册到
  `OperationManager`，不得通过 Command 名称、Renderer callback 或输入分支直接调用；Command 仅保存
  命令面板元数据，Operation 语义不得内嵌快捷键。
- 根 `src/` 只负责组合各边界、初始化 World、运行终端循环和端到端验收。
- 新增 `DatasetKind` 时必须同时补齐默认 Template、Render Proxy、Operation/Command、可用性规则
  和注册契约测试，禁止依靠字符串猜测或 renderer 特判补洞。
- 同一时间只允许一份 `ActiveUiContext`、`ActiveRenderMode` 和 `ResolvedOperationSet`。
- 快捷键、footer、Help 和 Command Palette 必须共同读取已解析 Operation Set。

## Git 与安全规则

- 命令必须使用 argv，不得拼接 shell 命令字符串。
- 每个 Git 操作必须有稳定、无敏感参数的 operation name。
- 日志不得记录 diff、文件内容、URL、凭据或 commit message。
- Git 元信息、路径和错误进入终端前必须清理控制字符；持久日志还必须执行长度限制和 URL 脱敏。
- 新增破坏性操作时，必须同时实现风险说明、确认数据、失败状态、恢复策略和真实临时仓库测试。
- Remote 写操作测试只能使用临时 bare repository，禁止联系真实网络地址。
- 同步 Git executor 若新增长耗时操作，必须先明确 UI 阻塞预算或引入保持数据边界的任务执行方案。

## Pull Request

- PR 保持单一目的，并说明用户可见行为与安全影响。
- 新行为应有对应单元测试、ECS 契约测试或真实临时仓库测试。
- 如果使用了 AI/vibe coding，请说明生成范围以及完成的人工审阅和验证。
- 提交 PR 即表示你有权贡献相关内容，并同意其按项目的 [MIT License](LICENSE) 发布。
