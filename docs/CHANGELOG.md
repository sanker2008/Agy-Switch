# Agy-Switch 项目变更日志 (Changelog)

本文档汇总记录 Agy-Switch 项目的每次功能规划与变更演进凭据。

---

## [2026-07-29] OAuth 与本机凭据安全加固审查

- **安全审查记录**: [docs/security/2026-07-29-security-review.md](security/2026-07-29-security-review.md)

### 变更摘要

- **[安全] OAuth 与发布边界**: 运行时显式读取 OAuth 配置，使用 PKCE，并移除 renderer 中不必要的 opener 权限和 CI 中的 OAuth 配置注入。
- **[安全] 本机凭据与导出**: 将账户数据保护、数据库备份权限和备份导出确认收敛到 Rust/系统边界，避免把前端 UI 当作授权边界。
- **[处置] 公开历史凭据**: 记录“先在 OAuth 提供方撤销或轮换、再经授权处理公开 Git 历史与标签”的顺序；未执行任何外部凭据变更、历史重写或强制推送。
- **[验证边界] 原生平台**: Windows/macOS 原生对话框、OAuth 浏览器启动、DPAPI 与 macOS Keychain 参数可见性仍须在对应平台验收。

---

## [2026-07-27] 桌面端与各端真实账号深度校准、CLI 细分与统一两行 UI 看板

- **规划文档**: [docs/plans/2026-07-27-active-client-surface-tracking.md](file:///d:/dev/san/agy-switch/docs/plans/2026-07-27-active-client-surface-tracking.md)
- **变更日志**: [docs/changelogs/2026-07-27-active-client-surface-tracking.md](file:///d:/dev/san/agy-switch/docs/changelogs/2026-07-27-active-client-surface-tracking.md)

### 变更摘要
- **[BUG 修复与校准] 真实账号深度检测**: 修复了由于 Token 在底层被更新导致侦测落入旧历史记录错显 `sthfume@gmail.com` 的问题。现引入多候选路径扫描与 Google 用户邮箱双重校验（`fetch_email`），精准匹配实时活度账号 `sannnweb@gmail.com`。
- **[UI 优化] 统一两行卡片布局**: 看板 4 个端点统一调整为两行结构。
