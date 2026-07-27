# 规划文档：Agy-Switch 各端当前作用账号深度侦测校准与统一两行 UI 看板

- **发布日期**: 2026-07-27
- **目标项目**: `Agy-Switch` (Tauri 2.0 Rust 后端 + React 19 + TypeScript 前端)
- **存储路径**: `~/.agy-switch/accounts.json` (通过 `write_file_atomically` 原子写入)

---

## 1. 需求与问题校准

此前系统在识别各端活跃账号时，存在因数据库/凭据 Token 被 Google 后台刷新后导致单纯字符串不匹配、从而落入旧版历史记录显示为旧账号（如 `sthfume@gmail.com`）的问题。

### 核心解决与优化方案
1. **多候选路径深度侦测 (`import_state_db_candidates`)**:
   - 包含正运行进程参数路径（`--user-data-dir`）、默认 AppData 路径以及 Portable 便携版路径全量扫描。
2. **Token + API 邮箱双重匹配与纠错 (Dual Match Protocol)**:
   - **一重比对**: 比对提取到的 `refresh_token` 与 `access_token`；
   - **二重兜底**: 若 Token 因刷新改变，异步调用 Google `fetch_email` 接口拉取底层真实邮箱，并与 `store.accounts` 比对，精准确定当前活度真实账号。
3. **实时系统真实状态覆盖 (Ground Truth Priority)**:
   - 侦测到的实时活度状态优先覆盖旧有历史记录。

---

## 2. 验证凭据与测试

- [x] **精确校准真实账号**: Antigravity 桌面端当前登录的真实账号 `sannnweb@gmail.com` 准确侦测并显示在看板上，不再错显旧账号。
- [x] **0 编译错误**: Rust `cargo check` 2.17s 通过。
