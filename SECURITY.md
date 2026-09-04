# 安全策略 / Security Policy

## 支持的版本 / Supported Versions

只对最新稳定版提供安全修复 / Security fixes are only provided for the latest stable release:

| 版本 / Version | 支持 / Supported |
| :--- | :--- |
| 最新 Release（见 [Releases](https://github.com/yxpil/bit/releases)） | ✅ |
| 更早版本 / Earlier versions | ❌ 请升级 / Please upgrade |

## 报告漏洞 / Reporting a Vulnerability

**请勿在公开 Issue 中披露安全问题 / Do not disclose security issues in public issues.**

首选方式：使用 GitHub 私密漏洞报告 / Preferred: GitHub private vulnerability reporting:

> https://github.com/yxpil/bit/security/advisories/new

也可以通过 / Also via:

- QQ 群 / QQ group: [点击加入](https://qm.qq.com/q/qlFr8ct0ps)（ fastest，联系作者转私密渠道 / fastest, will move to a private channel）
- 邮件 / Email: 见 [GitHub 主页](https://github.com/yxpil/bit) 资料 / see profile on the GitHub profile

请在报告中包含 / Please include:

1. 影响的版本与平台 / affected version and platform
2. 复现步骤或 PoC / reproduction steps or PoC
3. 影响评估 / impact assessment

## 响应目标 / Response Targets

- 48 小时内确认收到 / acknowledgement within 48 hours
- 高危问题力争 7 天内发布修复 / high-severity fixes targeted within 7 days
- 修复发布后在 Release 说明中致谢（可要求匿名）/ credit in release notes (anonymous on request)

本项目为个人维护的永久免费开源项目，响应速度依赖业余时间，请谅解 / This is a personally maintained free-forever open-source project; responses depend on spare time.

## 范围 / Scope

**在范围内 / In scope:**

- 桌面应用本体 / the desktop app itself
- 自动更新链路 / auto-update chain（latest.json 源、资产下载与白名单 / latest.json sources, asset download & host whitelist）
- 本地 API 服务与远程访问 / local API server and remote access（访问密码、Client Key 认证 / access password, Client Key auth）
- MCP 与工具执行边界 / MCP and tool execution boundaries（路径逃逸、越权执行 / path escape, unauthorized execution）

**不在范围内 / Out of scope:**

- 对第三方 AI 提供方服务端的攻击 / attacks against third-party AI provider backends
- 需要物理接触设备或社会工程学的攻击 / attacks requiring physical access or social engineering
- 对 GitHub / Cloudflare 等基础设施自身的攻击 / attacks against infrastructure (GitHub, Cloudflare, etc.)
- 由用户主动关闭安全特性造成的问题 / issues caused by deliberately disabling security features

## 安全设计要点 / Security Design Notes

- 自动更新资产只接受白名单主机（github.com、yxpil.github.io、osbt.space 等），拒绝其他来源 / update assets are only accepted from an allow-list of hosts
- 本地服务默认只绑定环回地址；开启远程访问需要显式设置访问密码 / local server binds loopback by default; remote access requires an explicitly configured access password
- AI 工具调用记录完整审计日志 / all AI tool invocations are written to an audit log

## 安全护栏 / Safe Harbor

对以善意方式研究并按本页流程负责任披露的安全研究，作者不会采取法律行动 / We will not pursue legal action against good-faith research and responsible disclosure following this policy.
