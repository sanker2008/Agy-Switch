# Agy Switch - 用户交互操作与 UI 设计规格文档 (Stitch 重构专用)

> **文档用途**: 本文档完整梳理了 **Agy Switch**（Google Antigravity 多端账号切换器）的当前功能架构、所有界面区块、交互逻辑、状态流转与 UI 细节，供在 **Stitch** 中进行全新界面设计与视觉重构时参考使用。

---

## 🧭 1. 产品定位与核心价值

**Agy Switch** 是一款专门针对 Google Antigravity 生态的**轻量化桌面级多端账号快速切换与配额管理工具**（基于 Tauri 2.0 Rust + React 19 构建）。

### 核心用户痛点与解法：
- **多端独立凭据管理**: 用户同时在 **Antigravity 桌面原生版**、**Antigravity IDE (VS Code / Cursor 插件)**、**Windows CLI (`agy`)** 和 **WSL Linux CLI (`agy`)** 等 4 个端点使用不同 Google 账号。
- **无感自动感知 (Auto-Detect)**: 启动或刷新时，自动扫描系统 4 个端点的本地 SQLite 状态库 (`state.vscdb`) 和系统凭据管理器 (Keyring)，一眼查看“各端当前分别登录了谁”。
- **一键安全热切换**: 自动识别并优雅关闭目标客户端、自动安全备份状态库，将 Token 热注入目标端并自动重启程序。
- **模型配额实时查询**: 实时查询所选账号在 Gemini 3.1 / Claude 3.7 / Opus 4.6 等 20+ 个 AI 模型的剩余配额与重置倒计时。

---

## 🎨 2. 界面总体布局层级 (Layout Hierarchy)

整个应用采用顶部固定 Header + 核心 Hero 卡片 + 中间 4 端看板 + 下方双栏 Working Grid + 底部安全提示的单页主视图（Single-Page Application），配合模态弹窗（Modals）。

```text
┌───────────────────────────────────────────────────────────────────────────────────────────┐
│ [LOGO] Agy Switch                   [语言下拉] [🌓] [🎨主题色] [导出备份] [🔄刷新] [+添加账号] │  <-- 1. App Header
├───────────────────────────────────────────────────────────────────────────────────────────┤
│ 🟢 已成功将 sthfume@gmail.com 切换到 Antigravity IDE。                                      [X]│  <-- 2. Notice Toast
├───────────────────────────────────────────────────────────────────────────────────────────┤
│ QUICK SWITCH                                                                              │
│ sthfume@gmail.com                                ┌──────────┬──────────┬──────────┬──────────┐│  <-- 3. Hero Quick Switch
│ 选择目标即可切换。切换前会自动关闭目标并备份状态。   │ 🖥️ Desktop│ 🔁 IDE   │ 💻 WinCLI│ 🐧 WSLCLI││
│ 🛡️ 上次成功切换：Antigravity IDE                  └──────────┴──────────┴──────────┴──────────┘│
├───────────────────────────────────────────────────────────────────────────────────────────┤
│ 🛡️ 各端当前作用账号一览 (SURFACE OVERVIEW BAR - 4 列两行规整卡片)                                │  <-- 4. Active Surface Bar
│ ┌───────────────────┬───────────────────┬───────────────────┬───────────────────┐ │
│ │ 🖥️ Antigravity     │ 🔁 Antigravity IDE│ 💻 Win CLI        │ 🐧 WSL CLI        │ │
│ │ sthfume@gmail.com │ sthfume@gmail.com │ sannnweb@gmail.com│ 未切换             │ │
│ └───────────────────┴───────────────────┴───────────────────┴───────────────────┘ │
├──────────────────────────────────────────────────┬────────────────────────────────────────┤
│ LOCAL ACCOUNTS                                   │ MODEL QUOTA                            │  <-- 5. Main Workspace
│ 选择要切换的账号 [2]                 [🔄刷新配额] │ 所选账号的模型配额               [🔄刷新] │      (Left: Account List
│ ┌──────────────────────────────────────────────┐ │ 订阅: Google AI Pro · 2026-07-27 15:51│ │       Right: Model Quota)
│ │ (S) sthfume@gmail.com          [上次使用] [🗑️] │ Claude Opus 4.6 (Thinking) [100% 🟩]  │ │
│ ├──────────────────────────────────────────────┤ │ 重置: 2 小时 46 分钟后                 │ │
│ │ (S) sannnweb@gmail.com                    [🗑️] │ Gemini 3.1 Flash Lite      [100% 🟩]  │ │
│ └──────────────────────────────────────────────┘ │                            [↗️查看全部] │ │
├──────────────────────────────────────────────────┴────────────────────────────────────────┤
│ 🛡️ 切换提示：Antigravity 与 Antigravity CLI 使用同一份系统凭据；Antigravity IDE 可保持独立账号。    │  <-- 6. Safety Footer Note
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

---

## 📑 3. 八大功能模块交互规格 (Detailed Interactive Specs)

### 3.1 顶部导航栏 (App Header)
- **品牌 Logo 区**:
  - 图标: 包含 Antigravity 水晶/渐变 Icon。
  - 主标题: `Agy Switch`
  - 副标题/眉题: `ACCOUNT SWITCHER`
- **控制组件区 (右侧)**:
  - **语言切换器 (Language Picker)**: 下拉选择框，支持 `简体中文` (`zh-CN`)、`繁體中文` (`zh-TW`)、`English` (`en-US`)，切换后实时更新全局 i18n 文案。
  - **深浅主题切换 (Theme Toggle)**: 按钮 Icon（太阳 ☀️ / 月亮 🌙），支持 `dark` 深色模式与 `light` 浅色模式无缝切换。
  - **主题调色盘 (Accent Picker)**: 4 个颜色小圆点（`teal` 鼠尾草绿、`violet` 柔和堇紫、`cyan` 海盐青蓝、`amber` 柔暖琥珀），点击可切换主色调与 Glow 光效。
  - **导出备份按钮 (Export Backup)**: 带 Icon `Download`，点击触发系统保存文件对话框，将所有账号凭据加密导出为 JSON 备份文件。
  - **刷新账号按钮 (Refresh Accounts)**: 带 Icon `RefreshCw`（刷新时自带旋转动画），重新扫瞄并读取本地账号列表与各端动态检测状态。
  - **+ 添加账号按钮 (+ Add Account)**: 凸起的 Primary 高亮按钮，带 Icon `Plus`，点击打开「添加账号 Modal 弹窗」。

---

### 3.2 全局通知 Banner (Notice Toast)
- **位置**: 位于 Header 下方，跨满宽度。
- **状态分类**:
  - **成功 (Success)**: 绿底/青色光效，用于提示账号切换成功、导入完成、配额刷新成功。
  - **错误/警告 (Error/Warning)**: 红底/橙色光效，用于提示“无法关闭正在运行的 Antigravity”、“OAuth 授权失败”、“配额拉取失败”等。
- **交互**: 右侧带 `X` 按钮可手动关闭，切换新操作时自动更新文案。

---

### 3.3 快速切换 Hero 区域 (Quick Switch Hero)
- **左侧: 选中账号预览区**:
  - 眉题: `QUICK SWITCH`
  - 账号主邮箱: `h2` 级大字体（如 `sthfume@gmail.com`）。若无账号显示 `暂无保存的账号`。
  - 说明文案: “选择目标即可立即切换。切换前会自动安全关闭目标程序并备份状态库。”
  - 状态标识: 显示盾牌 Icon 🛡️ + 上次成功切换的目标（如 `上次成功切换: Antigravity IDE`）。
- **右侧: 4 端切换按钮组 (Target Switcher Grid)**:
  - 4 个平铺网格按钮（1 行 4 列），分别代表：
    1. 🖥️ **Antigravity** (桌面端) - 描述: `写入系统凭据与本地状态库`
    2. 🔁 **Antigravity IDE** (IDE 插件) - 描述: `写入独立的 IDE 状态库`
    3. 💻 **Win CLI** (Windows 命令行) - 描述: `写入共享的系统凭据`
    4. 🐧 **WSL CLI** (Linux 子系统) - 描述: `写入 WSL Linux 凭据`
  - **按钮交互**:
    - 点击任意目标按钮，触发 `switch_account(account_id, target)` 操作。
    - 按钮展现 Loading 旋转 Icon，完成切换后更新背景高亮态 (`is-recorded`)，并在顶部 Banner 提示成功。

---

### 3.4 各端当前作用账号一览看板 (Surface Overview Bar) ⭐ **核心重点**
- **定位**: 位于 Hero 下方、主 Working 区域上方，作为多端状态感知的心脏。
- **布局格式 (两行规整卡片 x 4 列)**:
  ```text
  ┌───────────────────┬───────────────────┬───────────────────┬───────────────────┐
  │ 🖥️ Antigravity     │ 🔁 Antigravity IDE│ 💻 Win CLI        │ 🐧 WSL CLI        │
  │ sthfume@gmail.com │ sthfume@gmail.com │ sannnweb@gmail.com│ 未切换             │
  └───────────────────┴───────────────────┴───────────────────┴───────────────────┘
  ```
  - **第一行 (Card Header)**: Icon + 目标端名称（`Antigravity` | `Antigravity IDE` | `Win CLI` | `WSL CLI`）。
  - **第二行 (Card Value)**: 自动侦测/最新绑定的账号邮箱（如 `sthfume@gmail.com`），若无则显示灰字 `未切换`。
- **自动侦测机制 (Auto-Detect Ground Truth)**:
  - 后端在启动/刷新时会自动解析 4 个端点实际的 `.vscdb` 数据库与 Keyring，直接输出目前真正在作用的邮箱，防止呈现陈旧记录。
- **视觉样式**:
  - 激活卡片带有 Primary 边框高亮与 Subtle 背景 Glow；未激活卡片为柔和暗色。

---

### 3.5 本地账号列表区 (Local Accounts Workspace)
- **标题栏**:
  - 眉题: `LOCAL ACCOUNTS`
  - 标题: `选择要切换的账号` + 数字 Badge（如 `2`）。
  - 右上角按钮: `刷新全部配额` (带 Icon `RefreshCw`)，批量拉取所有保存账号的模型配额。
- **账号列表项 (Account Item / Ledger Row)**:
  - **选择态 (Selected State)**: 点击整行选中该账号，左侧带主色渐变条，设置为 Quick Switch 的活动目标。
  - **Avatar**: 圆形首字母图标（如 `S`）。
  - **账号信息**: 显示 Email 及创建/上次切换时间（如 `上次切换 2026年7月27日 16:50`）。
  - **配额摘要**: 快速显示模型平均剩余百分比（如 `平均剩余 100%`）。
  - **状态 Tag**: 上次使用的账号显示高亮绿色 Tag `上次使用`。
  - **操作按钮**:
    - `ChevronRight`: 标识可点击选中。
    - 🗑️ `Trash2` (删除按钮): 红色 Hover 态，点击弹出确认框删除账号。

---

### 3.6 模型配额区域 (Model Quota Sidebar)
- **标题栏**:
  - 眉题: `MODEL QUOTA`
  - 标题: `模型配额` + 刷新 Icon 按钮。
- **订阅与更新时间**:
  - 显示订阅层级（如 `订阅: Google AI Pro` 或 `Standard`）。
  - 显示刷新时间（如 `更新于 2026年7月27日 15:51`）。
- **配额进度条 (Quota Progress Bars)**:
  - 针对所选账号，展示核心模型的配额：
    - **模型名称**: 如 `Claude Opus 4.6 (Thinking)`、`Gemini 3.1 Flash Lite`。
    - **配额百分比**: 右侧显示 `100%` / `65%` / `0%`（带颜色变化：绿/黄/红）。
    - **进度条 (Progress Track)**: 自定义填充条。
    - **重置倒计时**: 下方显示 `重置: 2 小时 46 分钟后`。
- **查看全部按钮**: 底部带 `↗️ 查看全部模型 (20)` 按钮，点击弹窗展示全部 20+ 个模型的完整配额面板。

---

### 3.7 添加账号 Modal 弹窗 (Add Account Modal)
点击 Header `+ 添加账号` 触发。分为 3 个 Tab 选项卡：

1. **Tab 1: OAuth 授权 (`oauth`) - 推荐**:
   - 说明: 通过 Google 官方 OAuth 2.0 PKCE 流程登录。
   - 交互: 点击 `开始 OAuth 授权` 自动打开默认浏览器访问 Google 登录页；提供回调链接/授权码输入框，粘贴后点击 `提交` 或 `我已授权，继续` 自动获取 Token 并保存。
2. **Tab 2: Refresh Token (`token`)**:
   - 说明: 适合高级用户批量导入已有 Token。
   - 交互: 大文本框支持粘贴单个 Token、JSON 数组或包含 Token 的长文本；点击 `确认添加` 逐一校验有效性并保存。
3. **Tab 3: 从当前登录状态导入 / 备份恢复 (`database`)**:
   - **自动检测**: 点击 `自动检测` 按钮，直接扫描当前 Antigravity / IDE / CLI 的登录状态并导入。
   - **选择 DB 文件**: 选择本地 `.vscdb` 数据库文件提取登录凭据。
   - **备份恢复**: 点击导入先前导出的 `agy-switch.accounts.v1.json` 备份文件。

---

### 3.8 全部模型配额 Modal 弹窗 (All Models Quota Modal)
- **触发**: 点击配额侧栏底部的 `查看全部模型 (20)`。
- **展现形式**: 居中大弹窗，网格/列表展示所有可用的 20+ 款 Gemini、Claude、Codex 模型配额进度条与重置倒计时，提供搜索框方便过滤特定模型。

---

## ⚡ 4. 核心交互流转逻辑 (Core User Workflows)

### 🔄 流程 A：账号一键切换流
```text
[用户在列表选中账号 A] 
       │
[点击 Quick Switch 中目标端 (如 Antigravity IDE)]
       │
[后端检查并终止已运行的目标进程 (taskkill / kill)]
       │
[安全备份原 state.vscdb 状态库]
       │
[注入账号 A 的 Access & Refresh Token 至目标 SQLite / Keyring]
       │
[自动重新启动目标程序] ──> [看板 Surface Overview 自动高亮更新账号 A]
```

### 🔍 流程 B：多端无感自动感知流
```text
[启动 Agy Switch / 点击刷新 🔄]
       │
[后端并发扫描 4 端 (Desktop DB, IDE DB, Win Keyring, WSL UNC Path)]
       │
[提取各端 Token ──> 如果 Token 已刷新，异步调用 Google fetch_email 校验]
       │
[匹配 store.accounts 中的目标账号] ──> [Surface Overview 看板 4 端精准列出真实邮箱]
```

---

## 🎨 5. Stitch 设计参考指南 (Design Tokens & Aesthetics)

针对 Stitch 进行 UI 模版制作与 Visual Design 时，请遵循以下规范：

- **设计风格**: 现代化 Glassmorphic / Sleek Dark Dashboard 风格，搭配精致边框与 Subtle Micro-Glow。
- **色彩 Token (Color Tokens)**:
  - **App Background**: 深蓝暗色 `#0f172a` (Dark) / 高雅浅灰 `#f8fafc` (Light)
  - **Card Background**: `#1e293b` (Dark Card) / `#ffffff` (Light Card)
  - **Accent Colors (支持 4 套预设)**:
    - 鼠尾草绿 (Teal): `#14b8a6` / `#2dd4bf`
    - 柔和堇紫 (Violet): `#8b5cf6` / `#a78bfa`
    - 海盐青蓝 (Cyan): `#06b6d4` / `#38bdf8`
    - 柔暖琥珀 (Amber): `#f59e0b` / `#fbbf24`
- **字体标准**: 现代无衬线字体（Inter, Segoe UI, system-ui），眉题（Eyebrow）统一大写 `uppercase` + 字母间距 `letter-spacing: 0.08em`。
- **组件形态**:
  - 圆角统一：卡片 `border-radius: 16px`，按钮 `10px - 13px`，Chip `12px`。
  - Surface Overview Bar 看板统一为**两行结构**，视觉对齐极度舒适。
