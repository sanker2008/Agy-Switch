# 变更日志：Agy-Switch 实时账号侦测校准与两行 UI 看板

- **日期**: 2026-07-27
- **模块**: Agy-Switch 账号切换器 (`Account Switcher`) / 深度侦测校准 (`Token & Email Dual Match`)

---

## 🌟 新功能与修复特性 (Features & Fixes)

- **深度侦测与真实账号纠错**:
  - 采用全路径候选探测（`import_state_db_candidates`）+ Token 比对 + 异步 Google `fetch_email` 双重校验，解决因 Token 后台刷新导致错显旧账号的问题。
- **两行统一 UI**:
  - 4 端卡片结构高度规整（第一行：图标 + 端点名称；第二行：真实邮箱账号/未切换）。

---

## ⚙️ 变更文件列表 (Changed Files)

- `src-tauri/src/lib.rs`: `detect_system_active_accounts` 改为异步，加入多路径探测与 `fetch_email` 邮箱校验
- `src/App.tsx`: 统一 `surface-chip-header` 与 `surface-email` 结构
- `src/styles.css`: 规范弹性盒样式
- `docs/plans/2026-07-27-active-client-surface-tracking.md`: 规划更新
- `docs/CHANGELOG.md`: 索引更新
