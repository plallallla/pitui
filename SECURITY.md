# Security Policy

## Supported versions

安全修复优先应用于默认分支的最新代码。发布版本存在受影响问题时，维护者会在可行范围内说明受影响版本与升级建议。

## Reporting a vulnerability

请优先使用 GitHub 仓库 **Security → Report a vulnerability** 的私密报告入口。不要在公开 Issue、Discussion、日志或截图中披露漏洞细节、凭据、私有仓库路径或敏感 diff 内容。

报告应尽量包含：

- 受影响的版本或 commit；
- 可安全复现的最小步骤；
- 影响范围，尤其是错误仓库写入、命令注入、终端注入或破坏性 Git 操作；
- 建议修复或缓解方式（如有）。

如果私密漏洞报告不可用，请先提交一个不含漏洞细节的普通 Issue，请求维护者提供私密联络渠道。

Pitui 的后台 JSONL 日志可能包含仓库绝对路径、文件路径和 Git stderr。提交日志作为复现材料前请先脱敏；diff/文件内容不会主动写入日志，commit message 也会被隐藏。

维护者会在完成初步评估后回复是否接受报告；修复发布前请避免公开细节。本项目不承诺固定响应 SLA 或漏洞奖励。
