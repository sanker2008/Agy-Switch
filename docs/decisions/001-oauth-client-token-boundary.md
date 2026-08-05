# ADR-001：OAuth 客户端与 Refresh Token 边界

## 状态

Accepted

## 日期

2026-08-05

## 背景

Agy Switch 可以从 Antigravity、Antigravity IDE、CLI 和历史备份中读取 refresh token。Google refresh token 与签发它的 OAuth 客户端绑定；不同应用即使服务同一个 Google 账号，也不一定能使用彼此的 refresh token。

实机诊断确认：同一 refresh token 可以由 Antigravity-Manager 使用其 OAuth 客户端刷新，但 Agy Switch 发布包使用另一 OAuth Client ID 时，Google 返回 HTTP 400。使用 Antigravity-Manager 的 Client ID 而不提供其 Client Secret，Google 明确返回 `invalid_request`。

## 决策

- Agy Switch 继续使用自己的桌面 OAuth Client ID 与 PKCE。
- 不复制、读取或内置 Antigravity-Manager 等第三方应用的 Client Secret。
- 第三方来源的 refresh token 若被 Google 拒绝，界面必须区分 OAuth 错误类型并引导用户在 Agy Switch 中重新授权同一邮箱。
- 同邮箱 OAuth 授权使用现有 upsert 行为更新原账号，不创建重复账号。
- Google 刷新响应若轮换 refresh token，必须持久化新值；未返回新值时保留旧值。

## 备选方案

### 内置 Antigravity-Manager 的 OAuth Client Secret

拒绝。桌面安装包无法安全保存 Client Secret，复制第三方凭据还会把两个产品的发布和撤销生命周期错误耦合。

### 对 HTTP 400 自动重复请求

拒绝。OAuth 客户端不匹配不是网络瞬态错误，重试不会改变 token 的签发客户端，只会增加无效请求并掩盖根因。

### 仅复用第三方应用当前的 Access Token

拒绝作为长期方案。Access token 生命周期短，只能暂时绕过刷新，过期后仍会回到相同的 OAuth 客户端不匹配问题。

## 结果

- 用户首次迁移第三方账号后可能需要在 Agy Switch 中重新授权一次。
- 重新授权后的 token 与 Agy Switch 发布包客户端一致，后续配额刷新可以独立工作。
- OAuth 错误文案会提供可执行恢复步骤，同时不会回显 Google 响应中的敏感细节。
