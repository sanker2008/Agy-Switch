# Agy-Switch 项目变更日志 (Changelog)

本文档汇总记录 Agy-Switch 项目的每次功能规划与变更演进凭据。

---

## [0.1.3] - 2026-08-04

- **[修复] Windows SOCKS 系统代理**: 模型配额刷新会保留 Windows PAC 与系统代理返回的 `SOCKS`/`SOCKS5` 传输协议，不再错误地把 SOCKS 地址当作 HTTP 代理。
- **[兼容性] 代理客户端**: 启用 HTTP 客户端的 SOCKS 支持，兼容 `SOCKS5 host:port` 与 `socks=host:port` 两种 Windows 代理格式。
- **[防回归] 代理解析测试**: 覆盖 PAC 指令和手动 SOCKS 系统代理设置，防止后续更新再次降级传输协议。

---

## [0.1.2] - 2026-07-29

- **[修复] 发布包 OAuth 可用性**: GitHub Release 构建在编译期注入公开的 Google OAuth Client ID；安装后的普通用户无需自行设置环境变量。
- **[安全] 凭据边界**: 运行期 Client ID 可覆盖内置值；Client Secret 不会进入 CI、二进制、安装包或发布资产。
- **[防回归] 构建保护**: CI 在 Client ID 未配置时中止构建，避免再次发布无法完成登录和配额刷新的安装包。

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
