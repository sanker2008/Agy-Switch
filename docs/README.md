# Agy-Switch 技术文档与架构说明

欢迎查阅 **Agy-Switch** 项目的技术设计与变更规划文档。

---

## 1. 项目定位与核心机制

`Agy-Switch` 是专门针对 Google Antigravity 的**轻量化多端凭据与账号切换器**。

### 核心运作机制：
1. **Desktop 桌面端 (`desktop`)**:
   - 自动寻址 `%APPDATA%\Antigravity\User\globalStorage\state.vscdb` 或本地客户端存储；
   - 先关闭进程、备份数据库，再通过 SQLite 事务将 `access_token` 和 `refresh_token` 注入 `.vscdb`，同时写入 Windows Keyring。
2. **IDE 插件端 (`ide`)**:
   - 寻址 Antigravity IDE / VS Code 的全局存储路径，完成 Token 的热注入与启动关联。
3. **CLI 命令行 (`cli`)**:
   - 直接修改系统 Keyring 凭据与 `~/.gemini/antigravity-cli` 共享状态，使命令行 `agy` 无缝使用新账号。

---

## 2. 文档目录规范

```text
docs/
├── README.md                                          # 本说明文档
├── USER_INTERACTION_SPECS.md                          # UI 界面与交互重构规格文档 (Stitch 专用)
├── CHANGELOG.md                                       # 全局变更日志索引
├── plans/                                             # 每次需求与改动的规划文档目录
│   └── 2026-07-27-active-client-surface-tracking.md  # 各端作用账号一览规划
└── changelogs/                                        # 每次发版/功能的具体 Changelog
    └── 2026-07-27-active-client-surface-tracking.md  # 详细变动清单
```

---

## 3. 本地开发与规范约定

- **只在 `Agy-Switch` 项目内修改代码与创建文档**。
- `Antigravity-Manager` 项目仅供对比参考，绝不作修改。
- 本地 `.gemini` 规则维护在 `.git/info/exclude` 中，保持本地私有，不污染 Git 提交。
