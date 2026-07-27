# Agy-Switch 项目变更日志 (Changelog)

本文档汇总记录 Agy-Switch 项目的每次功能规划与变更演进凭据。

---

## [2026-07-27] 桌面端与各端真实账号深度校准、CLI 细分与统一两行 UI 看板

- **规划文档**: [docs/plans/2026-07-27-active-client-surface-tracking.md](file:///d:/dev/san/agy-switch/docs/plans/2026-07-27-active-client-surface-tracking.md)
- **变更日志**: [docs/changelogs/2026-07-27-active-client-surface-tracking.md](file:///d:/dev/san/agy-switch/docs/changelogs/2026-07-27-active-client-surface-tracking.md)

### 变更摘要
- **[BUG 修复与校准] 真实账号深度检测**: 修复了由于 Token 在底层被更新导致侦测落入旧历史记录错显 `sthfume@gmail.com` 的问题。现引入多候选路径扫描与 Google 用户邮箱双重校验（`fetch_email`），精准匹配实时活度账号 `sannnweb@gmail.com`。
- **[UI 优化] 统一两行卡片布局**: 看板 4 个端点统一调整为两行结构。
