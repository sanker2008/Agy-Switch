# Agy Switch

专注于 Antigravity 账号切换和模型配额查看的桌面应用。

简单、轻量、高效，专注于提供账号管理、三端切换（Antigravity / Antigravity IDE / Antigravity CLI）、账号备份与模型配额实时查询功能。

## 功能范围

| 功能 | 行为 |
| --- | --- |
| 添加账号 | 默认 Google OAuth 浏览器授权；也支持单个/批量 refresh token、当前/自定义 `state.vscdb` 与 V1 备份导入。 |
| Antigravity | 写入系统凭据；若已存在经典版的 `state.vscdb`，同时更新该库并启动程序。 |
| Antigravity IDE | 备份并更新 IDE 的 `state.vscdb`，然后启动 IDE。 |
| Antigravity CLI（`agy`） | 写入系统凭据，不自动启动 CLI。请在切换后自行运行 `agy`。 |
| 模型配额 | 按账号查询并缓存可用模型的剩余比例、重置时间和订阅等级；支持单账号或全部账号刷新。 |
| 账号备份 | 可导出 Agy Switch 当前账号并在另一台设备导入；备份文件含 refresh token，必须按敏感凭据保管。 |

每次写入状态库前都会在原位置生成带时间戳的备份。Agy Switch 的账号数据独立保存在 `~/.agy-switch/accounts.json`。

### 账号隔离说明

三个端不会在一次操作中同时切换：点击哪个目标，就只向该目标写入登录状态。但 Antigravity 与 Antigravity CLI 共用 Windows 系统凭据，因此两者**不能长期保持不同账号**；切换其中任意一个，都会覆盖另一端下次读取的共享凭据。Antigravity IDE 使用独立的 `state.vscdb`，可以与 Antigravity / CLI 使用不同账号。

可稳定使用的组合是：**Antigravity IDE 使用账号 A，Antigravity 与 CLI 使用账号 B**。不支持让 Antigravity、Antigravity IDE、Antigravity CLI 三者分别保持三个不同账号。界面中的“当前”仅表示最后一次通过 Agy Switch 切换的账号和目标，并不代表三个端的实时登录状态。

## 使用方式

1. 配置 Google OAuth 后，点击“添加账号”，默认选择 **OAuth 授权**。点击“开始 OAuth 授权”后，浏览器会打开 Google 登录页；授权完成后返回应用点击“我已授权，继续”。
2. 如已有 token，切换至 **Refresh Token**，可粘贴单个 token、含 `refresh_token` 的 JSON 数组，或任意包含多个 token 的文本。
3. 如目标程序已经登录，切换至 **从数据库导入**，点击 **自动导入当前登录账号** 会优先读取正在运行的实例；未运行时会选择最近更新的 Antigravity 或 Antigravity IDE 状态库。也可指定 Antigravity、Antigravity IDE，或手动选择 `state.vscdb`；还可扫描 `~/.antigravity-agent` 批量导入 V1 备份。
4. 选中账号后，直接点击 Antigravity、Antigravity IDE 或 Antigravity CLI 即可切换；界面会显示上次成功切换的目标。
5. 在右侧配额区点击“刷新”，查看该账号可用模型的剩余配额和重置时间；工具栏的“刷新全部配额”会顺序刷新全部账号。
6. 点击工具栏“导出备份”保存本地账号；在“添加账号 → 导入账号”中可导入 Agy Switch 备份或 V1 备份。

模型配额直接使用账号的短期 access token 查询 Antigravity Cloud Code 服务；access token 即将过期时会先用本地保存的 refresh token 更新。查询失败不会删除账号或原有的配额缓存。账号无权访问配额时，界面会明确显示“无权读取”。

## 添加账号的数据来源

从 `state.vscdb` 导入只读取其中的 OAuth 状态，再用 refresh token 向 Google 验证账号；不会修改源数据库。自动检测支持启动参数 `--user-data-dir` 与安装目录下的便携数据目录，避免误读过期的默认状态库。V1 导入读取 `~/.antigravity-agent` 中的 `antigravity_accounts.json` 或 `accounts.json`，并兼容关联备份文件内的旧版 OAuth 状态。若 Google 授权页无法自动跳回应用，可复制授权链接，并在应用中粘贴回调链接或授权码提交。

## 开发与构建

### Windows（用于实际切换）

在 PowerShell 中执行：

```powershell
Set-Location D:\dev\san\agy-switch
npm install
npm run tauri dev
```

### Google OAuth 配置

为避免将 OAuth 客户端凭据硬编码进公开仓库，OAuth 登录和 refresh token 刷新从运行环境读取配置。复制 `.env.example` 的变量名，在启动应用的同一个终端中设置自己的桌面端 OAuth 客户端：

```powershell
$env:AGY_GOOGLE_OAUTH_CLIENT_ID = "你的客户端 ID"
# 仅当该客户端在 Google 配置中包含 secret 时才需要：
$env:AGY_GOOGLE_OAUTH_CLIENT_SECRET = "你的客户端 Secret"
npm run tauri dev
```

未设置 Client ID 时，OAuth 授权、token 导入、令牌刷新、当前账号自动侦测和配额查询会明确提示配置缺失；已添加账号仅能在其短期 access token 尚未过期时切换。账号备份导出、删除等本地文件操作不受影响。不要把真实值写入 `.env.example`、源代码或公开仓库。桌面 OAuth 使用 PKCE；通常不需要 Client Secret，只有你的 Google 客户端明确要求时才配置它。

构建安装包：

```powershell
npm run tauri build
```

#### esbuild 版本不匹配

若启动时出现 `Host version "..." does not match binary version "..."` 或 `The service was stopped`，说明 `node_modules` 中的 esbuild 主程序与 Windows 二进制不属于同一个版本。请先在运行 `tauri dev` 的终端按 `Ctrl+C` 停止旧进程，然后在 **Windows PowerShell** 中执行：

```powershell
Set-Location D:\dev\san\agy-switch
Remove-Item -Recurse -Force .\node_modules
npm ci
npm run build
npm run tauri dev
```

这只会重建项目依赖，不会删除账号数据；账号数据位于 `~/.agy-switch/accounts.json`。

### WSL（前端检查）

```bash
cd /mnt/d/dev/san/agy-switch
npm install
npm run build
```

`node_modules` 包含与操作系统绑定的 esbuild 和 Tauri 二进制，不能作为 Windows 与 WSL 共用的依赖目录。若要在 Windows 启动 Agy Switch，请不要在同一份 `D:\dev\san\agy-switch` 中再从 WSL 执行 `npm install`；它可能会替换 Windows 二进制，导致上述版本或平台不匹配。需要在 WSL 做前端检查时，请使用单独的工作副本，或直接调用 Windows 的 `npm run build`。

WSL 中编译 Tauri Linux 桌面程序需要本机 GTK/GLib 开发依赖，且生成的是 Linux 应用，不能验证 Windows Credential Manager 或 Windows 上的 Antigravity 状态库。需要验证 Windows 三端切换时，请使用 Windows 原生 PowerShell 运行 Tauri。

前端通过 Tauri 2 的 `invoke` 调用 Rust 命令，调用方式参考官方文档：<https://v2.tauri.app/develop/calling-rust/>。

## 数据与安全

- refresh token 等同长期账号凭据；Unix/WSL 上的 `~/.agy-switch` 目录及其中账户文件会强制设为仅当前用户可读写。Windows 账户库使用当前用户的 DPAPI 加密，旧版明文账户库会在首次读取时自动迁移。
- Agy Switch 导出的 JSON 备份也包含 refresh token；导出前会再次确认，并会在 Unix/WSL 上以仅当前用户可读写的权限创建。不要通过聊天、邮件或公共仓库传输。
- WSL CLI 切换只会写入 Windows 默认 WSL 发行版中当前 Linux 用户的 `~/.gemini/antigravity-cli/credentials.json`，不会扫描或修改其他发行版及用户目录。
- Google 网络请求只允许 HTTPS 且不会跟随重定向。在 Windows 上会遵从系统代理；如公司代理实施 TLS 解密，请只在你信任该代理和根证书时进行 OAuth、账号导入或配额刷新。
- 删除账号只删除 Agy Switch 的本地记录，不会删除 Google 账号、系统凭据或旧项目数据。
- 首次切换 Antigravity IDE 前，先手动启动一次 IDE，确保已创建 `state.vscdb`。
- 从数据库导入需要读取已登录程序的 `state.vscdb`；请只选择你信任的本地数据库文件。
- 切换 Antigravity 或 IDE 时，应用会关闭同一目标的运行进程，防止旧会话把新凭据覆盖回去。
- 若目标程序关闭超时，切换会取消，避免旧进程与新状态库并发写入。

### 安全处置与审查记录

安全审查的代码修复、已验证范围、平台验证缺口与发布历史处置顺序见 [2026-07-29 安全审查记录](docs/security/2026-07-29-security-review.md)。其中的历史凭据事件必须先在 OAuth 提供方撤销或轮换，再由仓库所有者决定是否协调重写历史；不要仅删除当前源码或未经授权直接强制推送。

## 验收边界

`npm run build` 只能验证前端的 TypeScript 和生产打包。完整验收仍需要在 Windows 上安装目标程序，并使用有效 Google 账号实测：OAuth 浏览器授权与本机回调、`state.vscdb` 导入、系统凭据写入、三种账号切换及模型配额查询。
