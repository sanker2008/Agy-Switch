# Agy Switch 安全审查与处置记录

- **日期**: 2026-07-29
- **状态**: 代码级修复已完成并经静态检查验证；公开 Git 历史处置与 macOS 原生验证仍待完成。
- **范围**: Rust 后端、React/Tauri 前端与 capability、OAuth/网络路径、本机账户与备份、状态库写入、WSL/系统凭据、CI、文档和公开 Git 引用。

> 本文档不包含真实 refresh token、OAuth client secret、账户信息或本机绝对路径。它用于记录安全决策和验收边界，不是“零风险”认证。

## 背景与目标

Agy Switch 需要在当前用户的本机边界内保存 refresh token、向 Google 完成 OAuth、更新目标应用登录状态，并导出可迁移的账号备份。审查目标是把敏感数据的读写、授权确认和网络发送收敛到可验证的 Rust 与操作系统边界，避免把前端界面、默认文件权限或当前源码删除当作唯一防线。

## 已采纳的决策

### 1. 发布包内置构建期 Client ID，运行期可覆盖

发布 CI 只把公开的 OAuth Client ID 作为 `AGY_BUNDLED_GOOGLE_OAUTH_CLIENT_ID` 传给编译器，二进制将其作为回退值；运行期 `AGY_GOOGLE_OAUTH_CLIENT_ID` 优先，可用于开发、测试或自有客户端覆盖。官方安装包不要求终端用户配置环境变量。

桌面端采用 PKCE，Google 的 installed app 模型也假定客户端无法保存 secret。因此 Client ID 可随安装包分发，Client Secret 绝不进入源码、CI 构建环境、二进制、安装包或发布资产。只有自定义客户端在本机运行时明确要求时，才可读取可选的 `AGY_GOOGLE_OAUTH_CLIENT_SECRET`。具体启动方式见 [根 README 的 Google OAuth 配置](../../README.md#google-oauth-配置)。

### 2. OAuth 授权码使用 PKCE 与单次状态绑定

OAuth 授权使用 PKCE S256、一次性 state 和受控回调；refresh token 先交换为短期 access token 后才访问用户资料。出站 Google 请求限定为 HTTPS，且不接受重定向到未知地址。

这将授权码替换、长期 token 误用和重定向外传的风险降到受控边界，但仍需要在 Windows 与 macOS 实机完成浏览器回调验收。

### 3. 备份和导出在后端确认并以私有权限创建

导出账户不再通过 IPC 返回全部 refresh token；保留的导出命令在 Rust 端等待系统原生确认。Unix/WSL 的敏感目录和文件使用当前用户权限，数据库备份先以私有权限创建再复制内容，避免先复制、后收紧权限的窗口。

这项选择把“用户确认”和文件创建权限放在前端 JavaScript 不能单独绕过的位置。代价是 Windows/macOS 原生确认框仍需要真实点击验收。

### 4. Tauri renderer 保持最小权限

`main` 窗口只保留实际需要的 core 与 dialog capability；移除了 renderer 的 `opener:default`。OAuth 打开浏览器仍由 Rust 后端使用受控授权 URL 完成。

这样即使未来 renderer 代码受影响，也不会额外获得任意 HTTP(S) opener IPC 权限。

### 5. 发布流程只携带公开 Client ID，并拒绝无配置构建

GitHub Actions 使用不可变 action SHA；release job 从 `AGY_GOOGLE_OAUTH_CLIENT_ID` 读取公开标识并映射为编译期变量 `AGY_BUNDLED_GOOGLE_OAUTH_CLIENT_ID`。构建前会检查该值非空，缺失即失败，避免再次生成安装后无法登录或刷新配额的包。

release job 只保留 `GITHUB_TOKEN` 和上述 Client ID，不注入 Client Secret。该选择不替代发布前的 secret 扫描。

## 风险状态与验收结果

| 风险面 | 当前结论 | 依据与下一步 |
| --- | --- | --- |
| OAuth 回调、PKCE 与 Google 出站请求 | 已完成静态审查 | 代码使用 state/PKCE、HTTPS、受控重定向及构建期 Client ID 回退；在 Windows/macOS 实机完成 OAuth 浏览器流验收。 |
| 本地账户库、导出和数据库备份 | 已完成代码修复 | 账户文件权限、DPAPI 迁移、原生导出确认和私有备份创建均已实现；在原生系统验证导出确认与目标写入。 |
| WSL/Windows 凭据边界 | 已完成静态审查 | WSL 仅写入默认发行版的当前 Linux 用户；Windows Credential Manager 行为须在 Windows 验收。 |
| Tauri IPC/CSP/capability | 已完成代码修复 | 删除 token-returning export handler 与 renderer opener 权限；继续保持前端不渲染不可信 HTML。 |
| CI 供应链 | 已完成代码修复 | action 已固定 SHA，构建 job 只接收公开 Client ID，并在缺失时失败；发布前仍需检查新增依赖、GitHub 配置与 secret 扫描结果。 |
| 公开 Git 历史中的旧 OAuth 凭据 | **待外部处置** | 当前源码删除不能删除已发布历史对象；必须先由 OAuth 提供方撤销或轮换，再经仓库所有者授权处理分支、标签、fork/mirror 和发布资产。 |
| macOS Keychain 写入 | **待原生验证** | 现有 `security -w` 路径需要在 macOS 测量进程参数可见性，并验证替代方案与目标应用兼容。 |

## 公开历史凭据的处置顺序

此项是本次唯一仍可确认的报告风险。旧 OAuth client secret 曾进入公开 Git 历史；即使当前分支已移除文本，读取保留分支、标签、镜像或旧 clone 的人员仍可能恢复历史对象。

1. 由授权的 OAuth 提供方管理员撤销或轮换受影响的 client secret，并保存不含 secret 的处置证据。
2. 确认构建、部署和本机开发环境已经切换到新配置；移除不再需要的 CI secret。
3. 获得仓库所有者对历史重写、标签重建和强制推送的明确授权后，再制定备份、下游通知、fork/mirror 与发布资产清理计划。
4. 重写完成后，对每个仍公开的分支和标签执行只读祖先可达性检查，确认旧引入提交不再可达。

不得反转此顺序：先重写历史并不能使已经复制出去的仍有效凭据失效；未经授权的强制推送也会破坏下游 clone 和发布追溯。

## 验证记录与边界

已通过的本地检查：

- `cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check`
- `cargo metadata --manifest-path src-tauri/Cargo.toml --locked --no-deps --format-version 1`
- `npm exec tsc -- --noEmit`
- `npm audit --omit=dev --audit-level=high --package-lock-only`（无高危生产依赖漏洞）
- `git diff --check`、Tauri capability JSON 解析和公开 Git 引用可达性检查

以下验证不能由 WSL 替代，必须保留为平台验收项：

- Windows：DPAPI 迁移、系统凭据写入、原生导出确认、三个目标的实际切换。
- macOS：OAuth 浏览器启动、原生确认与 Keychain 参数可见性/兼容性。
- Linux/WSL：完整 Rust 运行测试需要安装 GLib、GIO 与 GObject 开发库；该环境阻断不代表代码测试通过。

## 发布前安全检查清单

- [ ] 确认 `.env.example`、README、源码、构建日志和 release asset 中没有真实 token 或 client secret。
- [ ] 检查每个新增或修改的 GitHub Action 是否使用不可变 SHA，并确认 release job 仅注入公开 Client ID、未注入 Client Secret，且缺少 Client ID 时会失败。
- [ ] 在 Windows 完成 OAuth、导入、导出、三端切换和配额查询的实机回归。
- [ ] 在 macOS 完成 OAuth/Keychain 原生验证，关闭参数可见性问题或记录经验证的剩余风险。
- [ ] 如处理过公开历史，先确认 OAuth 提供方的撤销/轮换，再执行经授权的分支/标签处置并复核可达性。

## 后续维护

涉及 OAuth、凭据存储、导入导出、目标写入、Tauri capability 或发布工作流的改动，应同时更新本文档的风险状态和验收清单。若安全决策被替代，新增一份带日期的记录并引用本文件，不删除历史记录。
