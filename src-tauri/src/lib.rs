use base64::{engine::general_purpose, Engine as _};
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Mutex, OnceLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use sysinfo::{ProcessesToUpdate, System};
use tauri::Emitter;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, watch},
};
use url::Url;
use uuid::Uuid;

const DATA_DIRECTORY: &str = ".agy-switch";
const BACKUP_FORMAT: &str = "agy-switch.accounts.v1";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v2/userinfo";
const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_REFRESH_SKEW_SECONDS: i64 = 900;
// Set only by the release build environment. A Google OAuth desktop client ID is public
// metadata; a client secret must never be embedded in a desktop application.
const BUNDLED_GOOGLE_OAUTH_CLIENT_ID: Option<&str> =
    option_env!("AGY_BUNDLED_GOOGLE_OAUTH_CLIENT_ID");
#[cfg(target_os = "windows")]
const PROTECTED_STORE_FORMAT: &str = "agy-switch.dpapi.v1";
const CLOUD_CODE_LOAD_ASSIST_URL: &str =
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:loadCodeAssist";
const QUOTA_API_ENDPOINTS: [&str; 3] = [
    "https://daily-cloudcode-pa.sandbox.googleapis.com/v1internal:fetchAvailableModels",
    "https://daily-cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
    "https://cloudcode-pa.googleapis.com/v1internal:fetchAvailableModels",
];
// Matches the native Antigravity request shape used by the source project. The quota API is
// undocumented; this identifier is therefore intentionally kept compatible rather than invented.
const QUOTA_USER_AGENT: &str = "vscode/1.X.X (Antigravity/4.3.0)";

static STORE_LOCK: Mutex<()> = Mutex::new(());
static OAUTH_FLOW: OnceLock<Mutex<Option<OAuthFlow>>> = OnceLock::new();

#[derive(Debug, Clone, PartialEq, Eq)]
struct GoogleOAuthClient {
    id: String,
    secret: Option<String>,
}

fn oauth_client_from_values(
    runtime_id: Option<String>,
    bundled_id: Option<&str>,
    secret: Option<String>,
) -> Result<GoogleOAuthClient, String> {
    let id = runtime_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            bundled_id
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
        })
        .ok_or("未配置 Google OAuth 客户端 ID；请设置 AGY_GOOGLE_OAUTH_CLIENT_ID 后重试。")?;
    let secret = secret
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Ok(GoogleOAuthClient { id, secret })
}

fn google_oauth_client() -> Result<GoogleOAuthClient, String> {
    oauth_client_from_values(
        env::var("AGY_GOOGLE_OAUTH_CLIENT_ID").ok(),
        BUNDLED_GOOGLE_OAUTH_CLIENT_ID,
        env::var("AGY_GOOGLE_OAUTH_CLIENT_SECRET").ok(),
    )
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
enum SwitchTarget {
    Desktop,
    Ide,
    Cli,
    #[serde(rename = "win_cli")]
    WinCli,
    #[serde(rename = "wsl_cli")]
    WslCli,
}

impl SwitchTarget {
    fn label(self) -> &'static str {
        match self {
            Self::Desktop => "Antigravity",
            Self::Ide => "Antigravity IDE",
            Self::Cli | Self::WinCli => "Win CLI",
            Self::WslCli => "WSL CLI",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct StoredToken {
    refresh_token: String,
    access_token: String,
    expires_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    id_token: Option<String>,
    #[serde(default)]
    is_gcp_tos: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_id: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct StoredAccount {
    id: String,
    email: String,
    token: StoredToken,
    created_at: i64,
    last_used_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quota: Option<QuotaData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_target: Option<SwitchTarget>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct AccountStore {
    #[serde(default = "default_store_version")]
    version: u8,
    #[serde(default)]
    accounts: Vec<StoredAccount>,
    #[serde(default)]
    current_account_id: Option<String>,
    #[serde(default)]
    current_target: Option<SwitchTarget>,
    #[serde(default)]
    target_accounts: HashMap<SwitchTarget, String>,
}

#[cfg(target_os = "windows")]
#[derive(Deserialize, Serialize)]
struct ProtectedStoreFile {
    format: String,
    data: String,
}

fn default_store_version() -> u8 {
    1
}

#[derive(Debug, Clone, Serialize)]
struct AccountView {
    id: String,
    email: String,
    created_at: i64,
    last_used_at: i64,
    is_current: bool,
    quota: Option<QuotaData>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_target: Option<SwitchTarget>,
}

#[derive(Debug, Serialize)]
struct AccountListResponse {
    accounts: Vec<AccountView>,
    current_target: Option<SwitchTarget>,
    target_accounts: HashMap<SwitchTarget, String>,
}

#[derive(Debug, Deserialize)]
struct BackupAccountInput {
    #[serde(default)]
    email: String,
    refresh_token: String,
}

#[derive(Debug, Serialize)]
struct BackupAccount {
    email: String,
    refresh_token: String,
}

#[derive(Debug, Serialize)]
struct BackupFile {
    format: &'static str,
    exported_at: String,
    accounts: Vec<BackupAccount>,
}

#[derive(Debug, Deserialize)]
struct BackupFileInput {
    format: String,
    accounts: Vec<BackupAccountInput>,
}

#[derive(Debug, Serialize)]
struct ImportResult {
    imported: usize,
    updated: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum DatabaseImportOutcome {
    Added,
    Updated,
    Unchanged,
}

#[derive(Debug, Serialize)]
struct DatabaseImportResult {
    account: AccountView,
    outcome: DatabaseImportOutcome,
}

#[derive(Debug, Deserialize)]
struct GoogleTokenResponse {
    access_token: String,
    expires_in: i64,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GoogleOAuthErrorResponse {
    #[serde(default)]
    error: String,
}

#[derive(Debug, Deserialize)]
struct GoogleUserInfo {
    email: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct ModelQuota {
    name: String,
    percentage: i32,
    reset_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct QuotaData {
    models: Vec<ModelQuota>,
    last_updated: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    subscription_tier: Option<String>,
    #[serde(default)]
    is_forbidden: bool,
}

#[derive(Debug, Deserialize)]
struct FetchAvailableModelsResponse {
    #[serde(default)]
    models: HashMap<String, AvailableModel>,
}

#[derive(Debug, Deserialize)]
struct AvailableModel {
    #[serde(rename = "quotaInfo")]
    quota_info: Option<AvailableQuotaInfo>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AvailableQuotaInfo {
    #[serde(rename = "remainingFraction")]
    remaining_fraction: Option<f64>,
    #[serde(rename = "resetTime")]
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadCodeAssistResponse {
    #[serde(rename = "cloudaicompanionProject")]
    project_id: Option<String>,
    #[serde(rename = "currentTier")]
    current_tier: Option<QuotaTier>,
    #[serde(rename = "paidTier")]
    paid_tier: Option<QuotaTier>,
    #[serde(rename = "allowedTiers")]
    allowed_tiers: Option<Vec<QuotaTier>>,
    #[serde(rename = "ineligibleTiers")]
    ineligible_tiers: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize)]
struct QuotaTier {
    #[serde(rename = "isDefault")]
    is_default: Option<bool>,
    id: Option<String>,
    name: Option<String>,
}

#[derive(Debug, Serialize)]
struct QuotaRefreshSummary {
    refreshed: usize,
    failed: usize,
}

struct OAuthFlow {
    authorization_url: String,
    redirect_uri: String,
    state: String,
    code_verifier: String,
    cancel_tx: watch::Sender<bool>,
    code_tx: mpsc::Sender<Result<String, String>>,
    code_rx: Option<mpsc::Receiver<Result<String, String>>>,
}

struct DatabaseToken {
    refresh_token: String,
    is_gcp_tos: bool,
}

fn now_timestamp() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs() as i64
}

fn data_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("无法定位用户主目录")?;
    let path = home.join(DATA_DIRECTORY);

    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        let mut builder = fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder
            .create(&path)
            .map_err(|error| format!("无法创建 Agy Switch 数据目录：{error}"))?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("无法收紧 Agy Switch 数据目录权限：{error}"))?;
    }

    #[cfg(not(unix))]
    fs::create_dir_all(&path).map_err(|error| format!("无法创建 Agy Switch 数据目录：{error}"))?;

    Ok(path)
}

fn store_path() -> Result<PathBuf, String> {
    Ok(data_dir()?.join("accounts.json"))
}

fn read_store() -> Result<AccountStore, String> {
    let path = store_path()?;
    if !path.exists() {
        return Ok(AccountStore {
            version: default_store_version(),
            ..AccountStore::default()
        });
    }
    let bytes = fs::read(&path).map_err(|error| format!("无法读取账号数据：{error}"))?;
    if bytes.is_empty() {
        return Ok(AccountStore {
            version: default_store_version(),
            ..AccountStore::default()
        });
    }
    let (store, _needs_windows_migration) = decode_store(&bytes)?;
    #[cfg(target_os = "windows")]
    if _needs_windows_migration {
        write_store(&store)?;
    }
    Ok(store)
}

fn decode_store(bytes: &[u8]) -> Result<(AccountStore, bool), String> {
    #[cfg(target_os = "windows")]
    if let Ok(protected) = serde_json::from_slice::<ProtectedStoreFile>(bytes) {
        if protected.format == PROTECTED_STORE_FORMAT {
            let encrypted = general_purpose::STANDARD
                .decode(protected.data)
                .map_err(|error| format!("账号数据加密内容无效：{error}"))?;
            let plaintext = unprotect_store_data(&encrypted)?;
            let store = serde_json::from_slice(&plaintext)
                .map_err(|error| format!("账号数据格式损坏：{error}"))?;
            return Ok((store, false));
        }
    }

    let store =
        serde_json::from_slice(bytes).map_err(|error| format!("账号数据格式损坏：{error}"))?;
    Ok((store, true))
}

fn write_file_atomically(path: &Path, content: &[u8], label: &str) -> Result<(), String> {
    use std::io::Write;
    let temp_path = path.with_extension(format!("tmp-{}", Uuid::new_v4()));
    {
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temp_path)
            .map_err(|error| format!("无法写入临时{label}：{error}"))?;

        file.write_all(content)
            .map_err(|error| format!("无法写入临时{label}：{error}"))?;

        file.sync_all()
            .map_err(|error| format!("无法同步临时{label}：{error}"))?;
    }

    #[cfg(target_os = "windows")]
    replace_file_atomically(&temp_path, path, label)?;

    #[cfg(not(target_os = "windows"))]
    fs::rename(&temp_path, path).map_err(|error| format!("无法保存{label}：{error}"))?;

    #[cfg(unix)]
    restrict_file_permissions(path, label)?;

    Ok(())
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path, label: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("无法收紧{label}文件权限：{error}"))
}

#[cfg(target_os = "windows")]
fn replace_file_atomically(temp_path: &Path, path: &Path, label: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x1;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let source = temp_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    unsafe {
        if MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        ) == 0
        {
            return Err(format!(
                "无法原子替换{label}：{}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
#[repr(C)]
struct DataBlob {
    cb_data: u32,
    pb_data: *mut u8,
}

#[cfg(target_os = "windows")]
fn protect_store_data(plaintext: &[u8]) -> Result<Vec<u8>, String> {
    use std::{ffi::c_void, ptr};

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptProtectData(
            data_in: *const DataBlob,
            description: *const u16,
            optional_entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt_struct: *const c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;
    let byte_len = u32::try_from(plaintext.len())
        .map_err(|_| "账号数据过大，无法使用 Windows 数据保护 API 加密。".to_string())?;
    let input = DataBlob {
        cb_data: byte_len,
        pb_data: plaintext.as_ptr() as *mut u8,
    };
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };

    unsafe {
        if CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        ) == 0
        {
            return Err(format!(
                "无法使用 Windows 数据保护 API 加密账号数据：{}",
                std::io::Error::last_os_error()
            ));
        }
        let encrypted =
            std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec();
        let _ = LocalFree(output.pb_data.cast());
        Ok(encrypted)
    }
}

#[cfg(target_os = "windows")]
fn unprotect_store_data(encrypted: &[u8]) -> Result<Vec<u8>, String> {
    use std::{ffi::c_void, ptr};

    #[link(name = "crypt32")]
    extern "system" {
        fn CryptUnprotectData(
            data_in: *const DataBlob,
            description: *mut *mut u16,
            optional_entropy: *const DataBlob,
            reserved: *mut c_void,
            prompt_struct: *const c_void,
            flags: u32,
            data_out: *mut DataBlob,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    const CRYPTPROTECT_UI_FORBIDDEN: u32 = 0x1;
    let byte_len = u32::try_from(encrypted.len())
        .map_err(|_| "账号数据过大，无法使用 Windows 数据保护 API 解密。".to_string())?;
    let input = DataBlob {
        cb_data: byte_len,
        pb_data: encrypted.as_ptr() as *mut u8,
    };
    let mut description = ptr::null_mut();
    let mut output = DataBlob {
        cb_data: 0,
        pb_data: ptr::null_mut(),
    };

    unsafe {
        if CryptUnprotectData(
            &input,
            &mut description,
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        ) == 0
        {
            return Err(format!(
                "无法使用 Windows 数据保护 API 解密账号数据：{}",
                std::io::Error::last_os_error()
            ));
        }
        let plaintext =
            std::slice::from_raw_parts(output.pb_data, output.cb_data as usize).to_vec();
        let _ = LocalFree(output.pb_data.cast());
        if !description.is_null() {
            let _ = LocalFree(description.cast());
        }
        Ok(plaintext)
    }
}

fn write_store(store: &AccountStore) -> Result<(), String> {
    let path = store_path()?;
    let content = serialize_store(store)?;
    write_file_atomically(&path, &content, "账号数据")
}

fn serialize_store(store: &AccountStore) -> Result<Vec<u8>, String> {
    let plaintext =
        serde_json::to_vec_pretty(store).map_err(|error| format!("无法序列化账号数据：{error}"))?;

    #[cfg(target_os = "windows")]
    {
        let encrypted = protect_store_data(&plaintext)?;
        return serde_json::to_vec(&ProtectedStoreFile {
            format: PROTECTED_STORE_FORMAT.to_string(),
            data: general_purpose::STANDARD.encode(encrypted),
        })
        .map_err(|error| format!("无法封装账号数据：{error}"));
    }

    #[cfg(not(target_os = "windows"))]
    Ok(plaintext)
}

fn account_view(account: &StoredAccount, current_id: Option<&str>) -> AccountView {
    AccountView {
        id: account.id.clone(),
        email: account.email.clone(),
        created_at: account.created_at,
        last_used_at: account.last_used_at,
        is_current: current_id == Some(account.id.as_str()),
        quota: account.quota.clone(),
        last_target: account.last_target,
    }
}

fn token_is_fresh(token: &StoredToken) -> bool {
    token.expires_at > now_timestamp() + TOKEN_REFRESH_SKEW_SECONDS
}

fn google_token_error(status: reqwest::StatusCode, body: &str) -> String {
    let error_code = serde_json::from_str::<GoogleOAuthErrorResponse>(body)
        .ok()
        .map(|response| response.error)
        .filter(|code| !code.trim().is_empty());

    match error_code.as_deref() {
        Some("invalid_grant") => concat!(
            "Google 拒绝该 refresh token（invalid_grant）。它已失效，或由其他 OAuth 客户端签发。",
            "若该账号来自 Antigravity-Manager，请在 Agy Switch 中通过“添加账号 → OAuth 授权”重新登录同一邮箱；原账号会自动更新。"
        )
        .to_string(),
        Some("invalid_client") => concat!(
            "Google 拒绝当前 OAuth 客户端配置（invalid_client）。",
            "请使用 Agy Switch 自己的 OAuth 授权重新登录，不要复用其他应用签发的 refresh token。"
        )
        .to_string(),
        Some("invalid_request") => concat!(
            "Google 拒绝当前 OAuth 刷新请求（invalid_request）。",
            "该 refresh token 可能要求签发它的客户端凭据；请在 Agy Switch 中通过 OAuth 重新授权同一邮箱。"
        )
        .to_string(),
        Some(code) => format!("Google 刷新 Token 失败（HTTP {status}，{code}）。"),
        None => format!("Google 刷新 Token 失败（HTTP {status}）。"),
    }
}

fn apply_refreshed_token(stored: &mut StoredToken, refreshed: GoogleTokenResponse) {
    stored.access_token = refreshed.access_token;
    stored.expires_at = now_timestamp() + refreshed.expires_in.max(0);
    if let Some(refresh_token) = refreshed
        .refresh_token
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        stored.refresh_token = refresh_token;
    }
    if refreshed.id_token.is_some() {
        stored.id_token = refreshed.id_token;
    }
}

async fn refresh_token(refresh_token: &str) -> Result<GoogleTokenResponse, String> {
    let oauth_client = google_oauth_client()?;
    let mut form = vec![
        ("client_id", oauth_client.id.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];
    if let Some(secret) = oauth_client.secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let response = send_google_token_request(&form)
        .await
        .map_err(|error| format!("无法连接 Google 授权服务：{error}"))?;

    if response.status().is_success() {
        return response
            .json::<GoogleTokenResponse>()
            .await
            .map_err(|error| format!("Google 授权响应无法解析：{error}"));
    }

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    Err(google_token_error(status, &body))
}

fn should_retry_google_request_directly(
    used_windows_system_proxy: bool,
    connection_or_timeout_failed: bool,
) -> bool {
    used_windows_system_proxy && connection_or_timeout_failed
}

async fn send_google_token_request(form: &[(&str, &str)]) -> Result<reqwest::Response, String> {
    #[cfg(target_os = "windows")]
    let used_windows_system_proxy = windows_system_proxy_for_url(GOOGLE_TOKEN_URL).is_some();
    #[cfg(not(target_os = "windows"))]
    let used_windows_system_proxy = false;

    let client = google_http_client(GOOGLE_TOKEN_URL, false)?;
    match client.post(GOOGLE_TOKEN_URL).form(form).send().await {
        Ok(response) => Ok(response),
        Err(proxy_error)
            if should_retry_google_request_directly(
                used_windows_system_proxy,
                proxy_error.is_connect() || proxy_error.is_timeout(),
            ) =>
        {
            let direct_client = google_http_client(GOOGLE_TOKEN_URL, true)?;
            direct_client
                .post(GOOGLE_TOKEN_URL)
                .form(form)
                .send()
                .await
                .map_err(|direct_error| {
                    format!(
                        "Windows 系统代理连接失败：{proxy_error}；禁用应用层代理后重试仍失败：{direct_error}"
                    )
                })
        }
        Err(error) => Err(error.to_string()),
    }
}

async fn fetch_email(access_token: &str) -> Result<String, String> {
    let response = google_http_client(GOOGLE_USERINFO_URL, false)?
        .get(GOOGLE_USERINFO_URL)
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|error| format!("无法读取账号邮箱：{error}"))?;
    if !response.status().is_success() {
        return Err("无法自动读取账号邮箱，请在添加账号时填写邮箱。".to_string());
    }
    let info = response
        .json::<GoogleUserInfo>()
        .await
        .map_err(|error| format!("账号邮箱响应无法解析：{error}"))?;
    if info.email.trim().is_empty() {
        return Err("Google 未返回账号邮箱，请手动填写。".to_string());
    }
    Ok(info.email)
}

async fn email_from_refresh_token(refresh_token_value: &str) -> Result<String, String> {
    let token = refresh_token(refresh_token_value).await?;
    fetch_email(&token.access_token).await
}

async fn build_account(
    email: String,
    refresh_token_value: String,
) -> Result<StoredAccount, String> {
    let token = refresh_token(&refresh_token_value).await?;
    let resolved_email = if email.trim().is_empty() {
        fetch_email(&token.access_token).await?
    } else {
        email.trim().to_string()
    };
    let now = now_timestamp();
    let effective_refresh_token = token
        .refresh_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(&refresh_token_value)
        .to_string();
    Ok(StoredAccount {
        id: Uuid::new_v4().to_string(),
        email: resolved_email,
        token: StoredToken {
            refresh_token: effective_refresh_token,
            access_token: token.access_token,
            expires_at: now + token.expires_in.max(0),
            id_token: token.id_token,
            is_gcp_tos: false,
            project_id: None,
        },
        created_at: now,
        last_used_at: 0,
        quota: None,
        last_target: None,
    })
}

async fn make_fresh(mut account: StoredAccount) -> Result<StoredAccount, String> {
    if token_is_fresh(&account.token) {
        return Ok(account);
    }
    let token = refresh_token(&account.token.refresh_token).await?;
    apply_refreshed_token(&mut account.token, token);
    Ok(account)
}

fn oauth_flow_state() -> &'static Mutex<Option<OAuthFlow>> {
    OAUTH_FLOW.get_or_init(|| Mutex::new(None))
}

fn new_pkce_verifier() -> String {
    (0..4)
        .map(|_| Uuid::new_v4().simple().to_string())
        .collect()
}

fn pkce_challenge(code_verifier: &str) -> String {
    general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(code_verifier.as_bytes()))
}

fn build_oauth_authorization_url(
    redirect_uri: &str,
    state: &str,
    code_challenge: &str,
) -> Result<String, String> {
    let oauth_client = google_oauth_client()?;
    let scopes = [
        "openid",
        "https://www.googleapis.com/auth/cloud-platform",
        "https://www.googleapis.com/auth/userinfo.email",
        "https://www.googleapis.com/auth/userinfo.profile",
        "https://www.googleapis.com/auth/cclog",
        "https://www.googleapis.com/auth/experimentsandconfigs",
    ]
    .join(" ");
    Url::parse_with_params(
        GOOGLE_AUTH_URL,
        [
            ("client_id", oauth_client.id.as_str()),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", &scopes),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("include_granted_scopes", "true"),
            ("state", state),
            ("code_challenge", code_challenge),
            ("code_challenge_method", "S256"),
        ],
    )
    .map(|url| url.to_string())
    .map_err(|error| format!("无法创建 OAuth 授权链接：{error}"))
}

fn oauth_success_html() -> &'static str {
    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
    <html><body style='font-family:system-ui;text-align:center;padding:48px'>\
    <h1 style='color:#1d6931'>授权成功</h1><p>可以关闭此页面并返回 Agy Switch。</p>\
    <script>setTimeout(function(){window.close()},1500)</script></body></html>"
}

fn oauth_failure_html() -> &'static str {
    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\n\r\n\
    <html><body style='font-family:system-ui;text-align:center;padding:48px'>\
    <h1 style='color:#b4232b'>授权未完成</h1><p>请返回 Agy Switch 后重试。</p></body></html>"
}

async fn prepare_oauth_flow(app: tauri::AppHandle) -> Result<String, String> {
    {
        let state = oauth_flow_state()
            .lock()
            .map_err(|_| "OAuth 状态锁不可用")?;
        if let Some(flow) = state.as_ref() {
            if flow.code_rx.is_some() {
                return Ok(flow.authorization_url.clone());
            }
        }
    }
    cancel_oauth_flow();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| format!("无法创建 OAuth 本机回调端口：{error}"))?;
    let port = listener
        .local_addr()
        .map_err(|error| format!("无法读取 OAuth 本机回调端口：{error}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth-callback");
    let state = Uuid::new_v4().to_string();
    let code_verifier = new_pkce_verifier();
    let authorization_url =
        build_oauth_authorization_url(&redirect_uri, &state, &pkce_challenge(&code_verifier))?;
    let (cancel_tx, mut cancel_rx) = watch::channel(false);
    let (code_tx, code_rx) = mpsc::channel::<Result<String, String>>(1);
    let callback_tx = code_tx.clone();
    let callback_state = state.clone();
    let callback_app = app.clone();

    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                accepted = listener.accept() => accepted.map_err(|error| format!("OAuth 回调连接失败：{error}")),
                _ = cancel_rx.changed() => break,
            };
            let Ok((mut stream, _)) = accepted else {
                break;
            };
            let mut buffer = [0_u8; 8192];
            let bytes_read = stream.read(&mut buffer).await.unwrap_or(0);
            let request = String::from_utf8_lossy(&buffer[..bytes_read]);
            let callback_path = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1));
            let callback_url =
                callback_path.and_then(|path| Url::parse(&format!("http://127.0.0.1{path}")).ok());
            let code = callback_url.as_ref().and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "code")
                    .map(|(_, value)| value.into_owned())
            });
            let returned_state = callback_url.as_ref().and_then(|url| {
                url.query_pairs()
                    .find(|(key, _)| key == "state")
                    .map(|(_, value)| value.into_owned())
            });
            let is_valid = returned_state.as_deref() == Some(callback_state.as_str());
            let result = match (code, is_valid) {
                (Some(code), true) => Ok(code),
                (Some(_), false) => Err("OAuth state 不匹配，已拒绝本次授权。".to_string()),
                (None, _) => {
                    Err("Google 未返回授权码；请在 Agy Switch 中重新开始授权。".to_string())
                }
            };
            let response = if result.is_ok() {
                oauth_success_html()
            } else {
                oauth_failure_html()
            };
            let _ = stream.write_all(response.as_bytes()).await;
            let _ = stream.flush().await;
            let _ = callback_app.emit("oauth-callback-received", ());
            let _ = callback_tx.send(result).await;
            break;
        }
    });

    let mut flow = oauth_flow_state()
        .lock()
        .map_err(|_| "OAuth 状态锁不可用")?;
    *flow = Some(OAuthFlow {
        authorization_url: authorization_url.clone(),
        redirect_uri,
        state,
        code_verifier,
        cancel_tx,
        code_tx,
        code_rx: Some(code_rx),
    });
    Ok(authorization_url)
}

fn cancel_oauth_flow() {
    if let Ok(mut state) = oauth_flow_state().lock() {
        if let Some(flow) = state.take() {
            let _ = flow.cancel_tx.send(true);
        }
    }
}

async fn exchange_oauth_code(
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<GoogleTokenResponse, String> {
    let oauth_client = google_oauth_client()?;
    let mut form = vec![
        ("client_id", oauth_client.id.as_str()),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("code_verifier", code_verifier),
        ("grant_type", "authorization_code"),
    ];
    if let Some(secret) = oauth_client.secret.as_deref() {
        form.push(("client_secret", secret));
    }
    let response = google_http_client(GOOGLE_TOKEN_URL, false)?
        .post(GOOGLE_TOKEN_URL)
        .form(&form)
        .send()
        .await
        .map_err(|error| format!("OAuth 授权码换取 Token 失败：{error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        return Err(format!("OAuth 授权码换取 Token 失败（HTTP {status}）。"));
    }
    response
        .json::<GoogleTokenResponse>()
        .await
        .map_err(|error| format!("OAuth Token 响应无法解析：{error}"))
}

async fn build_account_from_oauth_token(
    token: GoogleTokenResponse,
) -> Result<StoredAccount, String> {
    let refresh_token = token
        .refresh_token
        .ok_or("Google 未返回 refresh token；请先在 Google 账号中撤销 Agy Switch 的授权后重试。")?;
    let email = fetch_email(&token.access_token).await?;
    let now = now_timestamp();
    Ok(StoredAccount {
        id: Uuid::new_v4().to_string(),
        email,
        token: StoredToken {
            refresh_token,
            access_token: token.access_token,
            expires_at: now + token.expires_in.max(0),
            id_token: token.id_token,
            is_gcp_tos: false,
            project_id: None,
        },
        created_at: now,
        last_used_at: 0,
        quota: None,
        last_target: None,
    })
}

async fn add_quota_if_available(account: StoredAccount) -> StoredAccount {
    match refresh_stored_account_quota(account.clone()).await {
        Ok((updated, _)) => updated,
        Err(_) => account,
    }
}

fn database_import_outcome(
    existing_refresh_token: Option<&str>,
    imported_refresh_token: &str,
) -> DatabaseImportOutcome {
    match existing_refresh_token {
        None => DatabaseImportOutcome::Added,
        Some(existing) if existing == imported_refresh_token => DatabaseImportOutcome::Unchanged,
        Some(_) => DatabaseImportOutcome::Updated,
    }
}

fn upsert_imported_account(account: StoredAccount) -> Result<AccountView, String> {
    Ok(upsert_imported_account_with_outcome(account)?.account)
}

fn upsert_imported_account_with_outcome(
    mut account: StoredAccount,
) -> Result<DatabaseImportResult, String> {
    let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
    let mut store = read_store()?;
    let current_account_id = store.current_account_id.clone();
    if let Some(existing) = store
        .accounts
        .iter_mut()
        .find(|item| item.email.eq_ignore_ascii_case(&account.email))
    {
        let outcome = database_import_outcome(
            Some(existing.token.refresh_token.as_str()),
            account.token.refresh_token.as_str(),
        );
        account.id = existing.id.clone();
        account.created_at = existing.created_at;
        account.last_used_at = existing.last_used_at;
        *existing = account;
        let view = account_view(existing, current_account_id.as_deref());
        write_store(&store)?;
        return Ok(DatabaseImportResult {
            account: view,
            outcome,
        });
    }
    let view = account_view(&account, current_account_id.as_deref());
    store.accounts.push(account);
    write_store(&store)?;
    Ok(DatabaseImportResult {
        account: view,
        outcome: DatabaseImportOutcome::Added,
    })
}

fn tier_name(tier: &QuotaTier) -> Option<String> {
    tier.name.clone().or_else(|| tier.id.clone())
}

#[cfg(any(target_os = "windows", test))]
fn proxy_url_from_windows_proxy_list(proxy_list: &str, request_url: &str) -> Option<String> {
    let request_scheme = Url::parse(request_url).ok()?.scheme().to_ascii_lowercase();
    let mut fallback = None;

    for entry in proxy_list
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if entry.eq_ignore_ascii_case("DIRECT") {
            continue;
        }

        let (protocol, endpoint) = entry
            .split_once('=')
            .map(|(protocol, endpoint)| {
                (Some(protocol.trim().to_ascii_lowercase()), endpoint.trim())
            })
            .unwrap_or((None, entry));
        if let Some(protocol) = protocol.as_deref() {
            if matches!(protocol, "http" | "https") && protocol != request_scheme.as_str() {
                continue;
            }
        }

        let (endpoint, proxy_scheme) = endpoint
            .strip_prefix("PROXY ")
            .or_else(|| endpoint.strip_prefix("proxy "))
            .or_else(|| endpoint.strip_prefix("HTTPS "))
            .or_else(|| endpoint.strip_prefix("https "))
            .or_else(|| endpoint.strip_prefix("HTTP "))
            .or_else(|| endpoint.strip_prefix("http "))
            .map(|endpoint| (endpoint, None))
            .or_else(|| {
                endpoint
                    .strip_prefix("SOCKS5 ")
                    .or_else(|| endpoint.strip_prefix("socks5 "))
                    .or_else(|| endpoint.strip_prefix("SOCKS "))
                    .or_else(|| endpoint.strip_prefix("socks "))
                    .map(|endpoint| (endpoint, Some("socks5")))
            })
            .unwrap_or((endpoint, None));
        let endpoint = endpoint.trim();
        if endpoint.is_empty() || endpoint.eq_ignore_ascii_case("DIRECT") {
            continue;
        }

        let url = if endpoint.contains("://") {
            endpoint.to_string()
        } else {
            let scheme = proxy_scheme.or_else(|| {
                matches!(protocol.as_deref(), Some("socks") | Some("socks5")).then_some("socks5")
            });
            format!("{}://{endpoint}", scheme.unwrap_or("http"))
        };
        if Url::parse(&url)
            .ok()
            .filter(|url| {
                matches!(url.scheme(), "http" | "https" | "socks5") && url.host_str().is_some()
            })
            .is_some()
        {
            if protocol.as_deref() == Some(request_scheme.as_str()) {
                return Some(url);
            }
            fallback.get_or_insert(url);
        }
    }

    fallback
}

#[cfg(target_os = "windows")]
fn windows_system_proxy_for_url(request_url: &str) -> Option<String> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};

    type WinHttpHandle = *mut c_void;

    #[repr(C)]
    struct CurrentUserIeProxyConfig {
        auto_detect: i32,
        auto_config_url: *mut u16,
        proxy: *mut u16,
        proxy_bypass: *mut u16,
    }

    #[repr(C)]
    struct AutoProxyOptions {
        flags: u32,
        auto_detect_flags: u32,
        auto_config_url: *const u16,
        reserved: *mut c_void,
        reserved_flags: u32,
        auto_logon_if_challenged: i32,
    }

    #[repr(C)]
    struct ProxyInfo {
        access_type: u32,
        proxy: *mut u16,
        proxy_bypass: *mut u16,
    }

    #[link(name = "winhttp")]
    extern "system" {
        fn WinHttpOpen(
            user_agent: *const u16,
            access_type: u32,
            proxy_name: *const u16,
            proxy_bypass: *const u16,
            flags: u32,
        ) -> WinHttpHandle;
        fn WinHttpCloseHandle(handle: WinHttpHandle) -> i32;
        fn WinHttpGetIEProxyConfigForCurrentUser(config: *mut CurrentUserIeProxyConfig) -> i32;
        fn WinHttpGetProxyForUrl(
            session: WinHttpHandle,
            url: *const u16,
            options: *const AutoProxyOptions,
            proxy_info: *mut ProxyInfo,
        ) -> i32;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalFree(memory: *mut c_void) -> *mut c_void;
    }

    unsafe fn read_and_free_windows_string(value: *mut u16) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let length = (0..).take_while(|&index| *value.add(index) != 0).count();
        let result = String::from_utf16_lossy(std::slice::from_raw_parts(value, length));
        GlobalFree(value.cast());
        (!result.trim().is_empty()).then_some(result)
    }

    const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
    const WINHTTP_AUTOPROXY_AUTO_DETECT: u32 = 0x0000_0001;
    const WINHTTP_AUTOPROXY_CONFIG_URL: u32 = 0x0000_0002;
    const WINHTTP_AUTO_DETECT_TYPE_DHCP: u32 = 0x0000_0001;
    const WINHTTP_AUTO_DETECT_TYPE_DNS_A: u32 = 0x0000_0002;

    let mut config = CurrentUserIeProxyConfig {
        auto_detect: 0,
        auto_config_url: ptr::null_mut(),
        proxy: ptr::null_mut(),
        proxy_bypass: ptr::null_mut(),
    };
    unsafe {
        if WinHttpGetIEProxyConfigForCurrentUser(&mut config) == 0 {
            return None;
        }
        let auto_config_url = read_and_free_windows_string(config.auto_config_url);
        let static_proxy = read_and_free_windows_string(config.proxy);
        let _ = read_and_free_windows_string(config.proxy_bypass);

        let (flags, auto_config_url_wide) = if let Some(url) = auto_config_url {
            (
                WINHTTP_AUTOPROXY_CONFIG_URL,
                std::ffi::OsStr::new(&url)
                    .encode_wide()
                    .chain(Some(0))
                    .collect::<Vec<_>>(),
            )
        } else if config.auto_detect != 0 {
            (WINHTTP_AUTOPROXY_AUTO_DETECT, Vec::new())
        } else {
            return static_proxy
                .and_then(|proxy| proxy_url_from_windows_proxy_list(&proxy, request_url));
        };

        let user_agent = std::ffi::OsStr::new("Agy Switch")
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let request_url_wide = std::ffi::OsStr::new(request_url)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let session = WinHttpOpen(
            user_agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        );
        if session.is_null() {
            return static_proxy
                .and_then(|proxy| proxy_url_from_windows_proxy_list(&proxy, request_url));
        }

        let options = AutoProxyOptions {
            flags,
            auto_detect_flags: WINHTTP_AUTO_DETECT_TYPE_DHCP | WINHTTP_AUTO_DETECT_TYPE_DNS_A,
            auto_config_url: (!auto_config_url_wide.is_empty())
                .then_some(auto_config_url_wide.as_ptr())
                .unwrap_or(ptr::null()),
            reserved: ptr::null_mut(),
            reserved_flags: 0,
            auto_logon_if_challenged: 0,
        };
        let mut proxy_info = ProxyInfo {
            access_type: 0,
            proxy: ptr::null_mut(),
            proxy_bypass: ptr::null_mut(),
        };
        let proxy_lookup_succeeded = WinHttpGetProxyForUrl(
            session,
            request_url_wide.as_ptr(),
            &options,
            &mut proxy_info,
        ) != 0;
        let proxy = read_and_free_windows_string(proxy_info.proxy);
        let _ = read_and_free_windows_string(proxy_info.proxy_bypass);
        let resolved = if proxy_lookup_succeeded {
            proxy.and_then(|proxy| proxy_url_from_windows_proxy_list(&proxy, request_url))
        } else {
            None
        };
        let _ = WinHttpCloseHandle(session);

        if proxy_lookup_succeeded {
            resolved
        } else {
            static_proxy.and_then(|proxy| proxy_url_from_windows_proxy_list(&proxy, request_url))
        }
    }
}

fn google_http_client(
    request_url: &str,
    bypass_windows_system_proxy: bool,
) -> Result<reqwest::Client, String> {
    let builder = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(20));

    #[cfg(target_os = "windows")]
    let builder = if bypass_windows_system_proxy {
        // TUN works below the application proxy layer. Disabling the selected Windows proxy here
        // lets a failed local/PAC proxy retry through the TUN route without weakening HTTPS.
        builder.no_proxy()
    } else if let Some(proxy_url) = windows_system_proxy_for_url(request_url) {
        builder.proxy(
            reqwest::Proxy::all(&proxy_url)
                .map_err(|error| format!("Windows 系统代理配置无效：{error}"))?,
        )
    } else {
        builder
    };

    #[cfg(not(target_os = "windows"))]
    let builder = if bypass_windows_system_proxy {
        builder.no_proxy()
    } else {
        builder
    };

    builder
        .build()
        .map_err(|error| format!("无法创建 Google 网络请求：{error}"))
}

async fn load_code_assist(access_token: &str) -> Result<(Option<String>, Option<String>), String> {
    let response = google_http_client(CLOUD_CODE_LOAD_ASSIST_URL, false)?
        .post(CLOUD_CODE_LOAD_ASSIST_URL)
        .bearer_auth(access_token)
        .header(reqwest::header::USER_AGENT, QUOTA_USER_AGENT)
        .json(&serde_json::json!({ "metadata": { "ideType": "ANTIGRAVITY" } }))
        .send()
        .await
        .map_err(|error| format!("无法连接模型配额服务：{error}"))?;

    if !response.status().is_success() {
        return Ok((None, None));
    }

    let payload = response
        .json::<LoadCodeAssistResponse>()
        .await
        .map_err(|error| format!("模型配额项目信息无法解析：{error}"))?;
    let has_ineligible_tier = payload
        .ineligible_tiers
        .as_ref()
        .is_some_and(|tiers| !tiers.is_empty());
    let subscription_tier = tier_name_opt(payload.paid_tier.as_ref())
        .or_else(|| {
            (!has_ineligible_tier)
                .then(|| tier_name_opt(payload.current_tier.as_ref()))
                .flatten()
        })
        .or_else(|| {
            if has_ineligible_tier {
                payload
                    .allowed_tiers
                    .as_ref()
                    .and_then(|tiers| tiers.iter().find(|tier| tier.is_default == Some(true)))
                    .and_then(tier_name)
                    .map(|tier| format!("{tier} (Restricted)"))
            } else {
                None
            }
        });

    Ok((payload.project_id, subscription_tier))
}

fn tier_name_opt(tier: Option<&QuotaTier>) -> Option<String> {
    tier.and_then(tier_name)
}

fn quota_percentage(fraction: Option<f64>) -> i32 {
    let fraction = fraction.filter(|value| value.is_finite()).unwrap_or(0.0);
    (fraction.clamp(0.0, 1.0) * 100.0).round() as i32
}

fn is_user_facing_model(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["gemini", "claude", "gpt", "image", "imagen"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

async fn fetch_available_models(
    access_token: &str,
    project_id: Option<&str>,
    subscription_tier: Option<String>,
) -> Result<QuotaData, String> {
    let initial_payload = project_id
        .map(|project| serde_json::json!({ "project": project }))
        .unwrap_or_else(|| serde_json::json!({}));
    let mut last_error = "模型配额服务没有可用响应。".to_string();

    for endpoint in QUOTA_API_ENDPOINTS {
        let client = google_http_client(endpoint, false)?;
        let mut payload = initial_payload.clone();
        let mut retried_without_project = false;
        loop {
            let response = client
                .post(endpoint)
                .bearer_auth(access_token)
                .header(reqwest::header::USER_AGENT, QUOTA_USER_AGENT)
                .json(&payload)
                .send()
                .await;
            let response = match response {
                Ok(response) => response,
                Err(error) => {
                    last_error = format!("模型配额服务连接失败：{error}");
                    break;
                }
            };

            let status = response.status();
            if status.is_success() {
                let payload = response
                    .json::<FetchAvailableModelsResponse>()
                    .await
                    .map_err(|error| format!("模型配额响应无法解析：{error}"))?;
                let mut models = payload
                    .models
                    .into_iter()
                    .filter_map(|(name, model)| {
                        if !is_user_facing_model(&name) {
                            return None;
                        }
                        let quota = model.quota_info?;
                        Some(ModelQuota {
                            name,
                            percentage: quota_percentage(quota.remaining_fraction),
                            reset_time: quota.reset_time.unwrap_or_default(),
                            display_name: model.display_name,
                        })
                    })
                    .collect::<Vec<_>>();
                models.sort_by(|left, right| left.name.cmp(&right.name));
                return Ok(QuotaData {
                    models,
                    last_updated: now_timestamp(),
                    subscription_tier,
                    is_forbidden: false,
                });
            }

            if status == reqwest::StatusCode::FORBIDDEN
                && payload.get("project").is_some()
                && !retried_without_project
            {
                payload = serde_json::json!({});
                retried_without_project = true;
                continue;
            }
            if status == reqwest::StatusCode::FORBIDDEN {
                return Ok(QuotaData {
                    models: Vec::new(),
                    last_updated: now_timestamp(),
                    subscription_tier,
                    is_forbidden: true,
                });
            }
            last_error = format!("模型配额服务返回 HTTP {status}。");
            break;
        }
    }

    Err(last_error)
}

async fn refresh_stored_account_quota(
    account: StoredAccount,
) -> Result<(StoredAccount, QuotaData), String> {
    let mut account = make_fresh(account).await?;
    let existing_tier = account
        .quota
        .as_ref()
        .and_then(|quota| quota.subscription_tier.clone());
    let (project_id, detected_tier) = if let Some(project_id) = account.token.project_id.clone() {
        (Some(project_id), None)
    } else {
        load_code_assist(&account.token.access_token).await?
    };
    let quota = fetch_available_models(
        &account.token.access_token,
        project_id.as_deref(),
        detected_tier.or(existing_tier),
    )
    .await?;
    if project_id.is_some() {
        account.token.project_id = project_id;
    }
    account.quota = Some(quota.clone());
    Ok((account, quota))
}

#[tauri::command]
async fn list_accounts() -> Result<AccountListResponse, String> {
    let store = {
        let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
        read_store()?
    };
    let auto_detected = detect_system_active_accounts(&store).await;
    let mut merged_targets = store.target_accounts.clone();
    for (target, email) in auto_detected {
        merged_targets.insert(target, email);
    }

    Ok(AccountListResponse {
        accounts: store
            .accounts
            .iter()
            .map(|account| account_view(account, store.current_account_id.as_deref()))
            .collect(),
        current_target: store.current_target,
        target_accounts: merged_targets,
    })
}

#[tauri::command]
async fn prepare_oauth_url(app: tauri::AppHandle) -> Result<String, String> {
    prepare_oauth_flow(app).await
}

#[tauri::command]
async fn open_oauth_browser(app: tauri::AppHandle) -> Result<(), String> {
    let authorization_url = prepare_oauth_flow(app.clone()).await?;
    app.opener()
        .open_url(&authorization_url, None::<String>)
        .map_err(|error| format!("无法打开默认浏览器：{error}"))
}

#[tauri::command]
async fn complete_oauth_login() -> Result<AccountView, String> {
    let (mut code_rx, redirect_uri, code_verifier) = {
        let mut state = oauth_flow_state()
            .lock()
            .map_err(|_| "OAuth 状态锁不可用")?;
        let flow = state
            .as_mut()
            .ok_or("没有进行中的 OAuth 授权；请先点击“开始 OAuth 授权”。")?;
        let code_rx = flow
            .code_rx
            .take()
            .ok_or("OAuth 授权正在处理中，请勿重复提交。")?;
        (
            code_rx,
            flow.redirect_uri.clone(),
            flow.code_verifier.clone(),
        )
    };
    let code = match code_rx.recv().await {
        Some(Ok(code)) => code,
        Some(Err(error)) => return Err(error),
        None => return Err("OAuth 授权已取消或回调端口已关闭。".to_string()),
    };
    cancel_oauth_flow();
    let account = build_account_from_oauth_token(
        exchange_oauth_code(&code, &redirect_uri, &code_verifier).await?,
    )
    .await?;
    upsert_imported_account(add_quota_if_available(account).await)
}

#[tauri::command]
async fn submit_oauth_code(code_or_callback_url: String) -> Result<(), String> {
    let input = code_or_callback_url.trim();
    if input.is_empty() {
        return Err("授权码不能为空。".to_string());
    }
    let parsed_url = Url::parse(input).ok();
    let code = parsed_url
        .as_ref()
        .and_then(|url| {
            url.query_pairs()
                .find(|(key, _)| key == "code")
                .map(|(_, value)| value.into_owned())
        })
        .unwrap_or_else(|| input.to_string());
    let submitted_state = parsed_url.as_ref().and_then(|url| {
        url.query_pairs()
            .find(|(key, _)| key == "state")
            .map(|(_, value)| value.into_owned())
    });
    let sender = {
        let state = oauth_flow_state()
            .lock()
            .map_err(|_| "OAuth 状态锁不可用")?;
        let flow = state
            .as_ref()
            .ok_or("没有进行中的 OAuth 授权；请先点击“开始 OAuth 授权”。")?;
        if let Some(submitted_state) = submitted_state {
            if submitted_state != flow.state {
                return Err("OAuth state 不匹配，已拒绝本次授权。".to_string());
            }
        }
        flow.code_tx.clone()
    };
    sender
        .send(Ok(code))
        .await
        .map_err(|_| "OAuth 授权码接收器已关闭；请重新开始授权。".to_string())
}

#[tauri::command]
fn cancel_oauth_login() {
    cancel_oauth_flow();
}

#[tauri::command]
async fn import_default_database(
    target: Option<SwitchTarget>,
) -> Result<DatabaseImportResult, String> {
    if target == Some(SwitchTarget::Cli)
        || target == Some(SwitchTarget::WinCli)
        || target == Some(SwitchTarget::WslCli)
    {
        return import_account_from_keyring().await;
    }

    let candidates = import_state_db_candidates(target);
    let mut failures = Vec::new();

    for db_path in candidates {
        match import_account_from_database(&db_path).await {
            Ok(account) => return Ok(account),
            Err(error) => failures.push(error),
        }
    }

    if target.is_none() {
        if let Ok(account) = import_account_from_keyring().await {
            return Ok(account);
        }
    }

    let latest_error = failures
        .into_iter()
        .next()
        .unwrap_or_else(|| "没有可导入的 OAuth 登录状态。".to_string());
    Err(format!("未能从当前登录状态导入账号：{latest_error}"))
}

#[tauri::command]
async fn import_database_file(path: String) -> Result<DatabaseImportResult, String> {
    import_account_from_database(Path::new(&path)).await
}

#[tauri::command]
async fn fetch_account_quota(account_id: String) -> Result<AccountView, String> {
    let stored = {
        let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
        let store = read_store()?;
        store
            .accounts
            .iter()
            .find(|account| account.id == account_id)
            .cloned()
            .ok_or("账号不存在。")?
    };
    let original_refresh_token = stored.token.refresh_token.clone();
    let (updated, _) = refresh_stored_account_quota(stored).await?;

    let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
    let mut store = read_store()?;
    let current_account_id = store.current_account_id.clone();
    let account = store
        .accounts
        .iter_mut()
        .find(|account| account.id == account_id)
        .ok_or("账号在查询配额时被删除。")?;
    if account.token.refresh_token != original_refresh_token {
        return Err("账号凭据在查询期间已更新；请重新刷新模型配额。".to_string());
    }
    account.token = updated.token;
    account.quota = updated.quota;
    let view = account_view(account, current_account_id.as_deref());
    write_store(&store)?;
    Ok(view)
}

#[tauri::command]
async fn refresh_all_quotas() -> Result<QuotaRefreshSummary, String> {
    let account_ids = {
        let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
        read_store()?
            .accounts
            .into_iter()
            .map(|account| account.id)
            .collect::<Vec<_>>()
    };
    let mut summary = QuotaRefreshSummary {
        refreshed: 0,
        failed: 0,
    };
    for account_id in account_ids {
        match fetch_account_quota(account_id).await {
            Ok(_) => summary.refreshed += 1,
            Err(_) => summary.failed += 1,
        }
    }
    Ok(summary)
}

#[tauri::command]
async fn add_account(email: String, refresh_token: String) -> Result<AccountView, String> {
    let refresh_token = refresh_token.trim().to_string();
    if refresh_token.is_empty() {
        return Err("Refresh token 不能为空。".to_string());
    }
    let account = build_account(email, refresh_token).await?;
    let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
    let mut store = read_store()?;
    let duplicate = store
        .accounts
        .iter()
        .any(|item| item.email.eq_ignore_ascii_case(&account.email));
    if duplicate {
        return Err("该邮箱已经存在；请使用导入更新其 refresh token。".to_string());
    }
    let view = account_view(&account, store.current_account_id.as_deref());
    store.accounts.push(account);
    write_store(&store)?;
    Ok(view)
}

#[tauri::command]
async fn import_accounts(accounts: Vec<BackupAccountInput>) -> Result<ImportResult, String> {
    if accounts.is_empty() {
        return Err("没有可导入的账号。".to_string());
    }

    let mut prepared = Vec::new();
    for input in accounts {
        let refresh_token = input.refresh_token.trim().to_string();
        if refresh_token.is_empty() {
            continue;
        }
        prepared.push(build_account(input.email, refresh_token).await?);
    }
    if prepared.is_empty() {
        return Err("没有可导入的有效 refresh token。".to_string());
    }

    store_imported_accounts(prepared)
}

fn store_imported_accounts(prepared: Vec<StoredAccount>) -> Result<ImportResult, String> {
    let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
    let mut store = read_store()?;
    let mut imported = 0;
    let mut updated = 0;
    for mut account in prepared {
        if let Some(existing) = store
            .accounts
            .iter_mut()
            .find(|item| item.email.eq_ignore_ascii_case(&account.email))
        {
            account.id = existing.id.clone();
            account.created_at = existing.created_at;
            account.last_used_at = existing.last_used_at;
            *existing = account;
            updated += 1;
        } else {
            store.accounts.push(account);
            imported += 1;
        }
    }
    write_store(&store)?;
    Ok(ImportResult { imported, updated })
}

fn extract_v1_refresh_token(value: &serde_json::Value) -> Option<String> {
    let direct = value
        .get("refresh_token")
        .and_then(|token| token.as_str())
        .or_else(|| {
            value
                .get("token")
                .and_then(|token| token.get("refresh_token"))
                .and_then(|token| token.as_str())
        });
    if let Some(token) = direct.map(str::trim).filter(|token| !token.is_empty()) {
        return Some(token.to_string());
    }

    let legacy_state = value
        .get("jetskiStateSync.agentManagerInitState")
        .and_then(|state| state.as_str())?;
    let blob = general_purpose::STANDARD.decode(legacy_state).ok()?;
    let oauth_info = find_protobuf_bytes_field(&blob, 6).ok()??;
    let refresh_token = find_protobuf_bytes_field(&oauth_info, 3).ok()??;
    String::from_utf8(refresh_token)
        .ok()
        .filter(|token| !token.trim().is_empty())
}

fn resolve_v1_backup_file(v1_dir: &Path, location: &str) -> Option<PathBuf> {
    let configured = PathBuf::from(location);
    if configured.is_file() {
        return Some(configured);
    }
    let file_name = configured.file_name()?;
    [
        v1_dir.join(file_name),
        v1_dir.join("backups").join(file_name),
        v1_dir.join("accounts").join(file_name),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file())
}

fn scan_v1_refresh_tokens() -> Result<Vec<String>, String> {
    let home = dirs::home_dir().ok_or("无法读取用户主目录。")?;
    let v1_dir = home.join(".antigravity-agent");
    let mut found_index = false;
    let mut tokens = Vec::new();

    for index_name in ["antigravity_accounts.json", "accounts.json"] {
        let index_path = v1_dir.join(index_name);
        if !index_path.is_file() {
            continue;
        }
        found_index = true;
        let text = fs::read_to_string(&index_path)
            .map_err(|error| format!("无法读取 V1 账号索引 {}：{error}", index_path.display()))?;
        let index: serde_json::Value = serde_json::from_str(&text)
            .map_err(|error| format!("V1 账号索引不是有效 JSON：{error}"))?;
        let Some(root) = index.as_object() else {
            continue;
        };
        let accounts = root
            .get("accounts")
            .and_then(|value| value.as_object())
            .unwrap_or(root);

        for account in accounts.values().filter(|value| value.is_object()) {
            if let Some(token) = extract_v1_refresh_token(account) {
                tokens.push(token);
                continue;
            }
            let location = account
                .get("backup_file")
                .and_then(|value| value.as_str())
                .or_else(|| account.get("data_file").and_then(|value| value.as_str()));
            let Some(location) = location else {
                continue;
            };
            let Some(backup_path) = resolve_v1_backup_file(&v1_dir, location) else {
                continue;
            };
            let Ok(content) = fs::read_to_string(backup_path) else {
                continue;
            };
            let Ok(backup) = serde_json::from_str::<serde_json::Value>(&content) else {
                continue;
            };
            if let Some(token) = extract_v1_refresh_token(&backup) {
                tokens.push(token);
            }
        }
    }

    if !found_index {
        return Err(
            "未找到 V1 账号索引：~/.antigravity-agent/antigravity_accounts.json 或 accounts.json。"
                .to_string(),
        );
    }
    tokens.sort();
    tokens.dedup();
    if tokens.is_empty() {
        return Err("V1 备份中没有可导入的 refresh token。".to_string());
    }
    Ok(tokens)
}

#[tauri::command]
async fn import_v1_accounts() -> Result<ImportResult, String> {
    let mut prepared = Vec::new();
    let mut failed = 0;
    for refresh_token in scan_v1_refresh_tokens()? {
        match build_account(String::new(), refresh_token).await {
            Ok(account) => prepared.push(account),
            Err(_) => failed += 1,
        }
    }
    if prepared.is_empty() {
        return Err(if failed > 0 {
            "V1 备份中的 refresh token 均无法通过 Google 验证。".to_string()
        } else {
            "V1 备份中没有可导入的有效账号。".to_string()
        });
    }
    store_imported_accounts(prepared)
}

fn export_accounts() -> Result<BackupFile, String> {
    let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
    let store = read_store()?;
    Ok(BackupFile {
        format: BACKUP_FORMAT,
        exported_at: Utc::now().to_rfc3339(),
        accounts: store
            .accounts
            .into_iter()
            .map(|account| BackupAccount {
                email: account.email,
                refresh_token: account.token.refresh_token,
            })
            .collect(),
    })
}

#[tauri::command]
async fn export_accounts_to_file(app: tauri::AppHandle, path: String) -> Result<String, String> {
    let path = PathBuf::from(path.trim());
    if path.as_os_str().is_empty() {
        return Err("请选择账号备份保存位置。".to_string());
    }

    let confirmed = tokio::task::spawn_blocking(move || {
        app.dialog()
            .message("导出的 JSON 包含长期 refresh token。请仅保存到受信任的私有位置，且不要通过聊天、邮件或公共仓库传输。是否继续？")
            .title("确认导出账号备份")
            .buttons(tauri_plugin_dialog::MessageDialogButtons::YesNo)
            .blocking_show()
    })
    .await
    .map_err(|error| format!("无法显示账号备份确认对话框：{error}"))?;
    if !confirmed {
        return Err("已取消账号备份导出。".to_string());
    }

    let backup = export_accounts()?;
    let content = serde_json::to_vec_pretty(&backup)
        .map_err(|error| format!("无法序列化账号备份：{error}"))?;
    write_file_atomically(&path, &content, "账号备份")?;
    Ok(path.display().to_string())
}

#[tauri::command]
async fn import_backup_file(path: String) -> Result<ImportResult, String> {
    let path = PathBuf::from(path.trim());
    if !path.is_file() {
        return Err("账号备份文件不存在。".to_string());
    }
    let content = fs::read(&path).map_err(|error| format!("无法读取账号备份：{error}"))?;
    let backup = serde_json::from_slice::<BackupFileInput>(&content)
        .map_err(|error| format!("账号备份格式无效：{error}"))?;
    if backup.format != BACKUP_FORMAT {
        return Err(format!("不支持的账号备份格式：{}", backup.format));
    }
    import_accounts(backup.accounts).await
}

#[tauri::command]
fn delete_account(account_id: String) -> Result<(), String> {
    let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
    let mut store = read_store()?;
    let before = store.accounts.len();
    store.accounts.retain(|account| account.id != account_id);
    if before == store.accounts.len() {
        return Err("账号不存在。".to_string());
    }
    if store.current_account_id.as_deref() == Some(account_id.as_str()) {
        store.current_account_id = None;
        store.current_target = None;
    }
    write_store(&store)
}

#[tauri::command]
fn switch_account(account_id: String, target: SwitchTarget) -> Result<String, String> {
    let stored = {
        let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
        let store = read_store()?;
        store
            .accounts
            .into_iter()
            .find(|account| account.id == account_id)
            .ok_or("账号不存在。")?
    };
    // Switching only changes local credentials and must stay available offline. The target
    // application owns refreshing an expired access token after it regains network access.
    let target_needs_network_refresh = !token_is_fresh(&stored.token);
    apply_target_state(&stored, target)?;

    let _guard = STORE_LOCK.lock().map_err(|_| "账号存储锁不可用")?;
    let mut store = read_store()?;
    let email = {
        let account = store
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .ok_or("账号在切换过程中被删除。")?;
        *account = stored;
        account.last_used_at = now_timestamp();
        account.last_target = Some(target);
        account.email.clone()
    };
    store.current_account_id = Some(account_id);
    store.current_target = Some(target);
    store.target_accounts.insert(target, email.clone());
    if let Err(error) = write_store(&store) {
        return Err(format!(
            "已将 {email} 写入 {}，但 Agy Switch 未能保存本次切换记录：{error}",
            target.label()
        ));
    }
    let network_refresh_note = if target_needs_network_refresh {
        " 当前 access token 已过期，目标程序联网后会自行刷新。"
    } else {
        ""
    };
    let success_message = format!(
        "已将 {email} 切换到 {}。{network_refresh_note}",
        target.label()
    );

    match target {
        SwitchTarget::Cli | SwitchTarget::WinCli | SwitchTarget::WslCli => Ok(success_message),
        SwitchTarget::Desktop | SwitchTarget::Ide => match start_target(target) {
            Ok(()) => Ok(success_message),
            Err(error) => Ok(format!("{success_message} 但未能自动启动目标程序：{error}")),
        },
    }
}

fn apply_target_state(account: &StoredAccount, target: SwitchTarget) -> Result<(), String> {
    match target {
        SwitchTarget::Cli | SwitchTarget::WinCli => write_to_system_keyring(account),
        SwitchTarget::WslCli => write_to_wsl_cli(account),
        SwitchTarget::Desktop => {
            close_running_target(SwitchTarget::Desktop)?;
            if let Some(db_path) = find_state_db(SwitchTarget::Desktop) {
                let backup_path = backup_db(&db_path)?;
                inject_token_into_db(&db_path, account)?;
                if let Err(error) = write_to_system_keyring(account) {
                    let rollback = restore_db(&backup_path, &db_path);
                    return Err(match rollback {
                        Ok(()) => format!("系统凭据写入失败，已恢复状态库：{error}"),
                        Err(rollback_error) => format!(
                            "系统凭据写入失败，且无法恢复状态库：{error}；恢复失败：{rollback_error}"
                        ),
                    });
                }
                return Ok(());
            }
            write_to_system_keyring(account)
        }
        SwitchTarget::Ide => {
            close_running_target(SwitchTarget::Ide)?;
            let db_path = find_state_db(SwitchTarget::Ide).ok_or(
                "未找到 Antigravity IDE 的 state.vscdb；请先至少启动一次 Antigravity IDE。",
            )?;
            backup_db(&db_path)?;
            inject_token_into_db(&db_path, account)
        }
    }
}

fn backup_db(db_path: &Path) -> Result<PathBuf, String> {
    if !db_path.is_file() {
        return Err(format!("目标状态库不存在：{}", db_path.display()));
    }
    let stamp = Utc::now().format("%Y%m%d-%H%M%S%.3f");
    let backup = db_path.with_extension(format!(
        "vscdb.agy-switch-{stamp}-{}.backup",
        Uuid::new_v4()
    ));
    copy_database_backup(db_path, &backup)?;
    Ok(backup)
}

fn copy_database_backup(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        let mut input = fs::File::open(source)
            .map_err(|error| format!("无法读取目标状态库以创建备份：{error}"))?;
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true).mode(0o600);
        let mut output = options
            .open(destination)
            .map_err(|error| format!("无法创建受保护的状态库备份：{error}"))?;
        std::io::copy(&mut input, &mut output)
            .map_err(|error| format!("无法备份目标状态库：{error}"))?;
        output
            .sync_all()
            .map_err(|error| format!("无法同步状态库备份：{error}"))?;
        restrict_file_permissions(destination, "状态库备份")
    }

    #[cfg(not(unix))]
    {
        fs::copy(source, destination).map_err(|error| format!("无法备份目标状态库：{error}"))?;
        Ok(())
    }
}

fn restore_db(backup_path: &Path, db_path: &Path) -> Result<(), String> {
    fs::copy(backup_path, db_path).map_err(|error| format!("无法恢复目标状态库：{error}"))?;
    Ok(())
}

fn close_running_target(target: SwitchTarget) -> Result<(), String> {
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All);

    let matching_pids: Vec<_> = system
        .processes()
        .iter()
        .filter(|(_, p)| target_matches_process_name(target, &p.name().to_string_lossy()))
        .map(|(pid, _)| *pid)
        .collect();

    if matching_pids.is_empty() {
        return Ok(());
    }

    for pid in &matching_pids {
        if let Some(process) = system.process(*pid) {
            let _ = process.kill();
        }
    }

    #[cfg(target_os = "windows")]
    {
        if target == SwitchTarget::Desktop {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/IM", "antigravity.exe"])
                .output();
        } else if target == SwitchTarget::Ide {
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/IM", "Antigravity IDE.exe"])
                .output();
            let _ = Command::new("taskkill")
                .args(["/F", "/T", "/IM", "antigravity-ide.exe"])
                .output();
        }
    }

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        system.refresh_processes(ProcessesToUpdate::All);
        let still_running = system
            .processes()
            .values()
            .any(|process| target_matches_process_name(target, &process.name().to_string_lossy()));
        if !still_running {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "{} 未能在 4 秒内完全退出；为避免旧会话覆盖新账号，请手动确认关闭后重试。",
                target.label()
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}

fn find_state_db(target: SwitchTarget) -> Option<PathBuf> {
    let folder = match target {
        SwitchTarget::Desktop => "Antigravity",
        SwitchTarget::Ide => "Antigravity IDE",
        SwitchTarget::Cli | SwitchTarget::WinCli | SwitchTarget::WslCli => return None,
    };
    let candidates = state_storage_candidates(folder);
    candidates.into_iter().find(|path| path.is_file())
}

fn state_db_path_in(user_data_dir: PathBuf) -> PathBuf {
    user_data_dir
        .join("User")
        .join("globalStorage")
        .join("state.vscdb")
}

fn target_matches_process_name(target: SwitchTarget, name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    let is_ide = name.contains("antigravity ide") || name.contains("antigravity-ide");
    let is_self_or_helper = name.contains("agy-switch")
        || name.contains("agy_switch")
        || name.contains("antigravity-cli")
        || name.contains("antigravity-manager")
        || name.contains("antigravity-proxy");

    if is_self_or_helper {
        return false;
    }

    match target {
        SwitchTarget::Desktop => {
            name == "antigravity.exe"
                || name == "antigravity"
                || (name.contains("antigravity") && !is_ide)
        }
        SwitchTarget::Ide => is_ide,
        SwitchTarget::Cli | SwitchTarget::WinCli | SwitchTarget::WslCli => false,
    }
}

fn user_data_dir_from_command_line(arguments: &[std::ffi::OsString]) -> Option<PathBuf> {
    let mut index = 0;
    while index < arguments.len() {
        let argument = arguments[index].to_string_lossy();
        if let Some(value) = argument.strip_prefix("--user-data-dir=") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(PathBuf::from(value));
            }
        } else if argument == "--user-data-dir" {
            if let Some(value) = arguments.get(index + 1) {
                let value = value.to_string_lossy();
                let value = value.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(PathBuf::from(value));
                }
            }
        }
        index += 1;
    }
    None
}

fn sort_state_db_candidates(paths: &mut Vec<PathBuf>) {
    paths.sort_by(|left, right| {
        let left_modified = fs::metadata(left)
            .and_then(|metadata| metadata.modified())
            .ok();
        let right_modified = fs::metadata(right)
            .and_then(|metadata| metadata.modified())
            .ok();
        right_modified.cmp(&left_modified)
    });
}

fn append_unique_paths(destination: &mut Vec<PathBuf>, candidates: Vec<PathBuf>) {
    let mut seen = destination.iter().cloned().collect::<HashSet<_>>();
    for candidate in candidates {
        if seen.insert(candidate.clone()) {
            destination.push(candidate);
        }
    }
}

fn running_state_db_candidates(targets: &[SwitchTarget]) -> Vec<PathBuf> {
    let mut system = System::new_all();
    system.refresh_processes(ProcessesToUpdate::All);
    let mut candidates = Vec::new();
    for process in system.processes().values() {
        let name = process.name().to_string_lossy();
        let Some(target) = targets
            .iter()
            .copied()
            .find(|target| target_matches_process_name(*target, &name))
        else {
            continue;
        };
        if let Some(user_data_dir) = user_data_dir_from_command_line(process.cmd()) {
            let db_path = state_db_path_in(user_data_dir);
            if db_path.is_file() {
                candidates.push(db_path);
            }
        } else if let Some(db_path) = find_state_db(target) {
            candidates.push(db_path);
        }
    }
    sort_state_db_candidates(&mut candidates);
    candidates
}

fn portable_state_db_candidates(target: SwitchTarget) -> Vec<PathBuf> {
    let Some(executable) = executable_path(target) else {
        return Vec::new();
    };
    let Some(parent) = executable.parent() else {
        return Vec::new();
    };
    let db_path = state_db_path_in(parent.join("data").join("user-data"));
    db_path.is_file().then_some(db_path).into_iter().collect()
}

fn import_state_db_candidates(target: Option<SwitchTarget>) -> Vec<PathBuf> {
    let targets = match target {
        Some(target) => vec![target],
        None => vec![SwitchTarget::Desktop, SwitchTarget::Ide],
    };
    let mut candidates = running_state_db_candidates(&targets);

    let mut fallback = Vec::new();
    for target in targets {
        fallback.extend(portable_state_db_candidates(target));
        if let Some(path) = find_state_db(target) {
            fallback.push(path);
        }
    }
    sort_state_db_candidates(&mut fallback);
    append_unique_paths(&mut candidates, fallback);
    candidates
}

async fn import_account_from_database(db_path: &Path) -> Result<DatabaseImportResult, String> {
    let database_token = extract_database_token(db_path)?;
    let mut account = build_account(String::new(), database_token.refresh_token).await?;
    account.token.is_gcp_tos = database_token.is_gcp_tos;
    upsert_imported_account_with_outcome(add_quota_if_available(account).await)
}

fn extract_database_token(db_path: &Path) -> Result<DatabaseToken, String> {
    if !db_path.is_file() {
        return Err(format!("数据库文件不存在：{}", db_path.display()));
    }
    let conn = Connection::open_with_flags(db_path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("无法读取 state.vscdb：{error}"))?;
    let unified_state: Option<String> = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?",
            ["antigravityUnifiedStateSync.oauthToken"],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取 IDE OAuth 状态：{error}"))?;
    if let Some(unified_state) = unified_state {
        return extract_unified_database_token(&unified_state);
    }

    let legacy_state: String = conn
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?",
            ["jetskiStateSync.agentManagerInitState"],
            |row| row.get(0),
        )
        .map_err(|_| "此数据库没有可导入的 Antigravity OAuth 登录状态。".to_string())?;
    let blob = general_purpose::STANDARD
        .decode(legacy_state)
        .map_err(|error| format!("旧版 OAuth 状态 Base64 无法解析：{error}"))?;
    let oauth_info =
        find_protobuf_bytes_field(&blob, 6)?.ok_or("旧版 OAuth 状态中未找到登录凭据。")?;
    let refresh_token = String::from_utf8(
        find_protobuf_bytes_field(&oauth_info, 3)?
            .ok_or("旧版 OAuth 状态中未找到 refresh token。")?,
    )
    .map_err(|_| "旧版 refresh token 不是有效的 UTF-8 文本。")?;
    Ok(DatabaseToken {
        refresh_token,
        is_gcp_tos: true,
    })
}

fn extract_unified_database_token(value: &str) -> Result<DatabaseToken, String> {
    let topic = general_purpose::STANDARD
        .decode(value)
        .map_err(|error| format!("OAuth 状态 Base64 无法解析：{error}"))?;
    let mut offset = 0;
    while offset < topic.len() {
        let (tag, content_start) = read_varint(&topic, offset)?;
        let field = (tag >> 3) as u32;
        let wire_type = (tag & 7) as u8;
        if field == 1 && wire_type == 2 {
            let (length, entry_start) = read_varint(&topic, content_start)?;
            let entry_end = entry_start.saturating_add(length as usize);
            let entry = topic
                .get(entry_start..entry_end)
                .ok_or("OAuth 状态数据不完整。")?;
            let key = find_protobuf_bytes_field(entry, 1)?
                .and_then(|bytes| String::from_utf8(bytes).ok());
            if key.as_deref() == Some("oauthTokenInfoSentinelKey") {
                let row = find_protobuf_bytes_field(entry, 2)?
                    .ok_or("OAuth 状态中未找到 Token 数据行。")?;
                let encoded_oauth_info = String::from_utf8(
                    find_protobuf_bytes_field(&row, 1)?.ok_or("OAuth 状态中未找到 Token 数据。")?,
                )
                .map_err(|_| "OAuth Token 数据不是有效的 UTF-8 文本。")?;
                let oauth_info = general_purpose::STANDARD
                    .decode(encoded_oauth_info)
                    .map_err(|error| format!("OAuth Token 数据 Base64 无法解析：{error}"))?;
                let refresh_token = String::from_utf8(
                    find_protobuf_bytes_field(&oauth_info, 3)?
                        .ok_or("OAuth 状态中未找到 refresh token。")?,
                )
                .map_err(|_| "refresh token 不是有效的 UTF-8 文本。")?;
                let is_gcp_tos = find_protobuf_varint_field(&oauth_info, 6)?.unwrap_or(0) != 0;
                return Ok(DatabaseToken {
                    refresh_token,
                    is_gcp_tos,
                });
            }
        }
        offset = skip_field(&topic, content_start, wire_type)?;
    }
    Err("OAuth 状态中未找到可导入的登录凭据。".to_string())
}

fn find_protobuf_bytes_field(data: &[u8], target_field: u32) -> Result<Option<Vec<u8>>, String> {
    let mut offset = 0;
    while offset < data.len() {
        let (tag, content_start) = read_varint(data, offset)?;
        let field = (tag >> 3) as u32;
        let wire_type = (tag & 7) as u8;
        if field == target_field && wire_type == 2 {
            let (length, value_start) = read_varint(data, content_start)?;
            let value_end = value_start.saturating_add(length as usize);
            return data
                .get(value_start..value_end)
                .map(|value| Some(value.to_vec()))
                .ok_or("Protobuf 数据不完整。".to_string());
        }
        offset = skip_field(data, content_start, wire_type)?;
    }
    Ok(None)
}

fn find_protobuf_varint_field(data: &[u8], target_field: u32) -> Result<Option<u64>, String> {
    let mut offset = 0;
    while offset < data.len() {
        let (tag, content_start) = read_varint(data, offset)?;
        let field = (tag >> 3) as u32;
        let wire_type = (tag & 7) as u8;
        if field == target_field && wire_type == 0 {
            return read_varint(data, content_start).map(|(value, _)| Some(value));
        }
        offset = skip_field(data, content_start, wire_type)?;
    }
    Ok(None)
}

fn state_storage_candidates(folder: &str) -> Vec<PathBuf> {
    let relative = PathBuf::from("User")
        .join("globalStorage")
        .join("state.vscdb");
    let mut candidates = Vec::new();
    #[cfg(target_os = "windows")]
    if let Ok(appdata) = std::env::var("APPDATA") {
        candidates.push(PathBuf::from(appdata).join(folder).join(&relative));
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = dirs::home_dir() {
        candidates.push(
            home.join("Library/Application Support")
                .join(folder)
                .join(&relative),
        );
    }
    #[cfg(target_os = "linux")]
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".config").join(folder).join(&relative));
    }
    candidates
}

fn start_target(target: SwitchTarget) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let app = match target {
            SwitchTarget::Desktop => "Antigravity",
            SwitchTarget::Ide => "Antigravity IDE",
            SwitchTarget::Cli | SwitchTarget::WinCli | SwitchTarget::WslCli => return Ok(()),
        };
        let output = Command::new("open")
            .args(["-a", app])
            .output()
            .map_err(|error| format!("无法启动 {}：{error}", target.label()))?;
        if output.status.success() {
            return Ok(());
        }
        return Err(format!(
            "无法启动 {}：{}",
            target.label(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    #[cfg(not(target_os = "macos"))]
    {
        let path = executable_path(target)
            .ok_or_else(|| format!("未找到 {} 可执行文件。", target.label()))?;
        Command::new(&path).spawn().map_err(|error| {
            format!("无法启动 {}（{}）：{error}", target.label(), path.display())
        })?;
        Ok(())
    }
}

fn executable_path(target: SwitchTarget) -> Option<PathBuf> {
    let (folder, executable) = match target {
        SwitchTarget::Desktop => ("Antigravity", "Antigravity"),
        SwitchTarget::Ide => ("Antigravity IDE", "Antigravity IDE"),
        SwitchTarget::Cli | SwitchTarget::WinCli | SwitchTarget::WslCli => return None,
    };
    let mut candidates: Vec<PathBuf> = Vec::new();
    #[cfg(target_os = "windows")]
    {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates.push(
                PathBuf::from(local)
                    .join("Programs")
                    .join(folder)
                    .join(format!("{executable}.exe")),
            );
        }
        for root in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Ok(value) = std::env::var(root) {
                candidates.push(
                    PathBuf::from(value)
                        .join(folder)
                        .join(format!("{executable}.exe")),
                );
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let binary = if target == SwitchTarget::Ide {
            "antigravity-ide"
        } else {
            "antigravity"
        };
        candidates.extend([
            PathBuf::from("/usr/bin").join(binary),
            PathBuf::from("/opt").join(folder).join(binary),
            PathBuf::from("/usr/share").join(folder).join(binary),
        ]);
        if let Some(home) = dirs::home_dir() {
            candidates.push(home.join(".local/bin").join(binary));
        }
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(any(target_os = "windows", test))]
const WSL_CREDENTIAL_WRITE_COMMAND: &str = "umask 077; mkdir -p \"$HOME/.gemini/antigravity-cli\"; cat > \"$HOME/.gemini/antigravity-cli/credentials.json\"; chmod 600 \"$HOME/.gemini/antigravity-cli/credentials.json\"";
#[cfg(any(target_os = "windows", test))]
const WSL_CREDENTIAL_READ_COMMAND: &str = "if [ -r \"$HOME/.gemini/antigravity-cli/credentials.json\" ]; then cat \"$HOME/.gemini/antigravity-cli/credentials.json\"; else secret-tool lookup service gemini username antigravity; fi";

fn write_to_wsl_cli(account: &StoredAccount) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use std::io::Write;

        let expiry = chrono::DateTime::from_timestamp(account.token.expires_at, 0)
            .unwrap_or_else(Utc::now)
            .to_rfc3339_opts(SecondsFormat::Micros, true);
        let payload = serde_json::to_string_pretty(&serde_json::json!({
            "token": {
                "access_token": account.token.access_token,
                "token_type": "Bearer",
                "refresh_token": account.token.refresh_token,
                "expiry": expiry
            },
            "auth_method": "consumer"
        }))
        .map_err(|error| format!("无法序列化 WSL 凭据：{error}"))?;

        let mut child = Command::new("wsl.exe")
            .args(["--", "sh", "-c", WSL_CREDENTIAL_WRITE_COMMAND])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("无法启动默认 WSL 发行版：{error}"))?;
        child
            .stdin
            .take()
            .ok_or("无法写入默认 WSL 发行版的凭据。")?
            .write_all(payload.as_bytes())
            .map_err(|error| format!("无法写入默认 WSL 发行版的凭据：{error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("无法等待 WSL 凭据写入完成：{error}"))?;
        if output.status.success() {
            return Ok(());
        }
        return Err(format!(
            "默认 WSL 发行版的凭据写入失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    #[cfg(not(target_os = "windows"))]
    {
        let _ = account;
        Err("WSL CLI 切换只能在 Windows 上执行。".to_string())
    }
}

fn write_to_system_keyring(account: &StoredAccount) -> Result<(), String> {
    #[derive(Serialize)]
    struct KeyringToken<'a> {
        access_token: &'a str,
        token_type: &'static str,
        refresh_token: &'a str,
        expiry: String,
    }
    #[derive(Serialize)]
    struct KeyringPayload<'a> {
        token: KeyringToken<'a>,
        auth_method: &'static str,
    }

    let expiry = chrono::DateTime::from_timestamp(account.token.expires_at, 0)
        .unwrap_or_else(Utc::now)
        .to_rfc3339_opts(SecondsFormat::Micros, true);
    let payload = serde_json::to_string(&KeyringPayload {
        token: KeyringToken {
            access_token: &account.token.access_token,
            token_type: "Bearer",
            refresh_token: &account.token.refresh_token,
            expiry,
        },
        auth_method: "consumer",
    })
    .map_err(|error| format!("无法序列化系统凭据：{error}"))?;

    #[cfg(target_os = "windows")]
    return write_windows_credential(&payload);

    #[cfg(target_os = "macos")]
    {
        let value = format!(
            "go-keyring-base64:{}",
            general_purpose::STANDARD.encode(payload)
        );
        let output = Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-s",
                "gemini",
                "-a",
                "antigravity",
                "-w",
                &value,
                "-A",
            ])
            .output()
            .map_err(|error| format!("无法调用 macOS Keychain：{error}"))?;
        if output.status.success() {
            return Ok(());
        }
        return Err(format!(
            "macOS Keychain 写入失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    #[cfg(target_os = "linux")]
    {
        use std::io::Write;
        let mut child = Command::new("secret-tool")
            .args([
                "store",
                "--label=gemini",
                "service",
                "gemini",
                "username",
                "antigravity",
            ])
            .stdin(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("无法调用 Linux Secret Service：{error}"))?;
        child
            .stdin
            .take()
            .ok_or("无法写入 Linux Secret Service")?
            .write_all(payload.as_bytes())
            .map_err(|error| format!("无法写入 Linux Secret Service：{error}"))?;
        let output = child
            .wait_with_output()
            .map_err(|error| format!("无法等待 Linux Secret Service：{error}"))?;
        if output.status.success() {
            return Ok(());
        }
        return Err(format!(
            "Linux Secret Service 写入失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    #[allow(unreachable_code)]
    Err("当前系统不支持写入 Antigravity 凭据。".to_string())
}

#[cfg(target_os = "windows")]
fn write_windows_credential(payload: &str) -> Result<(), String> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    struct CredentialW {
        flags: u32,
        credential_type: u32,
        target_name: *const u16,
        comment: *const u16,
        last_written: FileTime,
        credential_blob_size: u32,
        credential_blob: *const u8,
        persist: u32,
        attribute_count: u32,
        attributes: *const c_void,
        target_alias: *const u16,
        user_name: *const u16,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn CredWriteW(credential: *const CredentialW, flags: u32) -> i32;
    }

    let target: Vec<u16> = std::ffi::OsStr::new("gemini:antigravity")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let user: Vec<u16> = std::ffi::OsStr::new("antigravity")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let secret = payload.as_bytes();
    let credential = CredentialW {
        flags: 0,
        credential_type: 1,
        target_name: target.as_ptr(),
        comment: ptr::null(),
        last_written: FileTime { low: 0, high: 0 },
        credential_blob_size: secret.len() as u32,
        credential_blob: secret.as_ptr(),
        persist: 2,
        attribute_count: 0,
        attributes: ptr::null(),
        target_alias: ptr::null(),
        user_name: user.as_ptr(),
    };
    unsafe {
        if CredWriteW(&credential, 0) == 0 {
            return Err(format!(
                "Windows 凭据管理器写入失败：{}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

async fn import_account_from_keyring() -> Result<DatabaseImportResult, String> {
    let raw_payloads = read_all_system_keyrings();
    if raw_payloads.is_empty() {
        return Err(
            "系统凭据管理器或 WSL 中未找到 Antigravity CLI 登录凭据（gemini:antigravity）。"
                .to_string(),
        );
    }

    let mut last_result = None;
    let mut errors = Vec::new();

    for raw_payload in raw_payloads {
        match extract_refresh_token_from_keyring(&raw_payload) {
            Ok(refresh_token) => match build_account(String::new(), refresh_token).await {
                Ok(account) => {
                    match upsert_imported_account_with_outcome(
                        add_quota_if_available(account).await,
                    ) {
                        Ok(outcome) => last_result = Some(outcome),
                        Err(err) => errors.push(err),
                    }
                }
                Err(err) => errors.push(err),
            },
            Err(err) => errors.push(err),
        }
    }

    if let Some(res) = last_result {
        Ok(res)
    } else {
        Err(errors
            .into_iter()
            .next()
            .unwrap_or_else(|| "未能从 Antigravity CLI 导入账号。".to_string()))
    }
}

fn extract_refresh_token_from_keyring(raw_payload: &str) -> Result<String, String> {
    let payload = raw_payload.trim();
    let decoded_str = if let Some(b64) = payload.strip_prefix("go-keyring-base64:") {
        let bytes = general_purpose::STANDARD
            .decode(b64)
            .map_err(|e| format!("Keyring Base64 解密失败：{e}"))?;
        String::from_utf8(bytes).map_err(|_| "Keyring 内容不是有效的 UTF-8 文本。".to_string())?
    } else {
        payload.to_string()
    };

    let trimmed = decoded_str.trim();

    if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(token) = value.get("token") {
            if let Some(rt) = token.get("refresh_token").and_then(|v| v.as_str()) {
                if !rt.trim().is_empty() {
                    return Ok(rt.trim().to_string());
                }
            }
        }
        if let Some(rt) = value.get("refresh_token").and_then(|v| v.as_str()) {
            if !rt.trim().is_empty() {
                return Ok(rt.trim().to_string());
            }
        }
        if let Some(rt) = value.get("refreshToken").and_then(|v| v.as_str()) {
            if !rt.trim().is_empty() {
                return Ok(rt.trim().to_string());
            }
        }
    }

    if trimmed.starts_with("1//") || (!trimmed.starts_with('{') && trimmed.len() > 10) {
        return Ok(trimmed.to_string());
    }

    Err("CLI 系统凭据中未找到有效的 refresh token。".to_string())
}

fn read_all_system_keyrings() -> Vec<String> {
    let mut results = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Ok(win_cred) = read_windows_credential() {
            if !win_cred.trim().is_empty() {
                results.push(win_cred);
            }
        }
        if let Ok(wsl_cred) = read_wsl_credential() {
            if !wsl_cred.trim().is_empty() && !results.contains(&wsl_cred) {
                results.push(wsl_cred);
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        let output = Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "gemini",
                "-a",
                "antigravity",
                "-w",
            ])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !secret.is_empty() {
                    results.push(secret);
                }
            }
        }
    }

    #[cfg(target_os = "linux")]
    {
        let output = Command::new("secret-tool")
            .args(["lookup", "service", "gemini", "username", "antigravity"])
            .output();
        if let Ok(output) = output {
            if output.status.success() {
                let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !secret.is_empty() {
                    results.push(secret);
                }
            }
        }
    }

    results
}

#[cfg(target_os = "windows")]
fn read_windows_credential() -> Result<String, String> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[repr(C)]
    struct CredentialW {
        flags: u32,
        credential_type: u32,
        target_name: *mut u16,
        comment: *mut u16,
        last_written: FileTime,
        credential_blob_size: u32,
        credential_blob: *mut u8,
        persist: u32,
        attribute_count: u32,
        attributes: *mut c_void,
        target_alias: *mut u16,
        user_name: *mut u16,
    }
    #[link(name = "advapi32")]
    extern "system" {
        fn CredReadW(
            target_name: *const u16,
            credential_type: u32,
            flags: u32,
            credential: *mut *mut CredentialW,
        ) -> i32;
        fn CredFree(buffer: *mut c_void);
    }

    let target: Vec<u16> = std::ffi::OsStr::new("gemini:antigravity")
        .encode_wide()
        .chain(Some(0))
        .collect();

    let mut cred_ptr: *mut CredentialW = ptr::null_mut();
    unsafe {
        if CredReadW(target.as_ptr(), 1, 0, &mut cred_ptr) != 0 && !cred_ptr.is_null() {
            let cred = &*cred_ptr;
            let blob_slice = std::slice::from_raw_parts(
                cred.credential_blob,
                cred.credential_blob_size as usize,
            );
            let payload = String::from_utf8_lossy(blob_slice).to_string();
            CredFree(cred_ptr as *mut c_void);
            if !payload.trim().is_empty() {
                return Ok(payload);
            }
        }
    }
    Err("Windows Credential Manager 中未找到凭据。".to_string())
}

#[cfg(target_os = "windows")]
fn read_wsl_credential() -> Result<String, String> {
    let output = Command::new("wsl.exe")
        .args(["--", "sh", "-c", WSL_CREDENTIAL_READ_COMMAND])
        .output()
        .map_err(|e| format!("WSL 无法执行 secret-tool：{e}"))?;
    if output.status.success() {
        let secret = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !secret.is_empty() {
            return Ok(secret);
        }
    }
    Err("WSL 中未找到凭据。".to_string())
}

async fn detect_system_active_accounts(store: &AccountStore) -> HashMap<SwitchTarget, String> {
    let mut detected = HashMap::new();

    // 1. Detect Desktop active account
    for db_path in import_state_db_candidates(Some(SwitchTarget::Desktop)) {
        if let Ok(db_token) = extract_database_token(&db_path) {
            if let Some(account) = store.accounts.iter().find(|a| {
                a.token.refresh_token == db_token.refresh_token
                    || a.token.access_token == db_token.refresh_token
            }) {
                detected.insert(SwitchTarget::Desktop, account.email.clone());
                break;
            }
            if let Ok(email) = email_from_refresh_token(&db_token.refresh_token).await {
                if let Some(account) = store
                    .accounts
                    .iter()
                    .find(|a| a.email.eq_ignore_ascii_case(&email))
                {
                    detected.insert(SwitchTarget::Desktop, account.email.clone());
                    break;
                }
            }
        }
    }

    // 2. Detect IDE active account
    for db_path in import_state_db_candidates(Some(SwitchTarget::Ide)) {
        if let Ok(db_token) = extract_database_token(&db_path) {
            if let Some(account) = store.accounts.iter().find(|a| {
                a.token.refresh_token == db_token.refresh_token
                    || a.token.access_token == db_token.refresh_token
            }) {
                detected.insert(SwitchTarget::Ide, account.email.clone());
                break;
            }
            if let Ok(email) = email_from_refresh_token(&db_token.refresh_token).await {
                if let Some(account) = store
                    .accounts
                    .iter()
                    .find(|a| a.email.eq_ignore_ascii_case(&email))
                {
                    detected.insert(SwitchTarget::Ide, account.email.clone());
                    break;
                }
            }
        }
    }

    // 3. Detect Win CLI active account
    #[cfg(target_os = "windows")]
    {
        if let Ok(raw_cred) = read_windows_credential() {
            if let Ok(rt) = extract_refresh_token_from_keyring(&raw_cred) {
                if let Some(account) = store
                    .accounts
                    .iter()
                    .find(|a| a.token.refresh_token == rt || a.token.access_token == rt)
                {
                    detected.insert(SwitchTarget::WinCli, account.email.clone());
                    detected.insert(SwitchTarget::Cli, account.email.clone());
                } else if let Ok(email) = email_from_refresh_token(&rt).await {
                    if let Some(account) = store
                        .accounts
                        .iter()
                        .find(|a| a.email.eq_ignore_ascii_case(&email))
                    {
                        detected.insert(SwitchTarget::WinCli, account.email.clone());
                        detected.insert(SwitchTarget::Cli, account.email.clone());
                    }
                }
            }
        }
    }

    // 4. Detect WSL CLI active account
    #[cfg(target_os = "windows")]
    {
        if let Ok(raw_cred) = read_wsl_credential() {
            if let Ok(rt) = extract_refresh_token_from_keyring(&raw_cred) {
                if let Some(account) = store
                    .accounts
                    .iter()
                    .find(|a| a.token.refresh_token == rt || a.token.access_token == rt)
                {
                    detected.insert(SwitchTarget::WslCli, account.email.clone());
                } else if let Ok(email) = email_from_refresh_token(&rt).await {
                    if let Some(account) = store
                        .accounts
                        .iter()
                        .find(|a| a.email.eq_ignore_ascii_case(&email))
                    {
                        detected.insert(SwitchTarget::WslCli, account.email.clone());
                    }
                }
            }
        }
    }

    detected
}

fn inject_token_into_db(db_path: &Path, account: &StoredAccount) -> Result<(), String> {
    let mut conn =
        Connection::open(db_path).map_err(|error| format!("无法打开目标状态库：{error}"))?;
    let transaction = conn
        .transaction()
        .map_err(|error| format!("无法开始状态库事务：{error}"))?;
    let current: Option<String> = transaction
        .query_row(
            "SELECT value FROM ItemTable WHERE key = ?",
            ["antigravityUnifiedStateSync.oauthToken"],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| format!("无法读取 IDE OAuth 状态：{error}"))?;
    let current = match current {
        Some(value) => general_purpose::STANDARD
            .decode(value)
            .map_err(|error| format!("现有 IDE OAuth 状态 Base64 无法解析：{error}"))?,
        None => Vec::new(),
    };
    let mut topic = remove_unified_topic_entry(&current, "oauthTokenInfoSentinelKey")?;
    topic.extend(create_unified_topic_entry(
        "oauthTokenInfoSentinelKey",
        &create_oauth_info(account),
    ));
    transaction
        .execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
            [
                "antigravityUnifiedStateSync.oauthToken",
                &general_purpose::STANDARD.encode(topic),
            ],
        )
        .map_err(|error| format!("无法写入 IDE OAuth 状态：{error}"))?;

    let user_status = create_unified_state_entry(
        "userStatusSentinelKey",
        &minimal_user_status(&account.email),
    );
    transaction
        .execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
            ["antigravityUnifiedStateSync.userStatus", &user_status],
        )
        .map_err(|error| format!("无法写入 IDE 用户状态：{error}"))?;
    transaction
        .execute(
            "INSERT OR REPLACE INTO ItemTable (key, value) VALUES (?, ?)",
            ["antigravityOnboarding", "true"],
        )
        .map_err(|error| format!("无法写入 IDE 初始化状态：{error}"))?;
    transaction
        .execute(
            "DELETE FROM ItemTable WHERE key = ?",
            ["jetskiStateSync.agentManagerInitState"],
        )
        .map_err(|error| format!("无法清理旧版 IDE OAuth 状态：{error}"))?;
    transaction
        .commit()
        .map_err(|error| format!("无法提交 IDE OAuth 状态：{error}"))
}

fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut output = Vec::new();
    while value >= 0x80 {
        output.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
    output.push(value as u8);
    output
}

fn read_varint(data: &[u8], mut offset: usize) -> Result<(u64, usize), String> {
    let mut result = 0_u64;
    let mut shift = 0;
    loop {
        let byte = *data.get(offset).ok_or("Protobuf 数据不完整")?;
        result |= u64::from(byte & 0x7f) << shift;
        offset += 1;
        if byte & 0x80 == 0 {
            return Ok((result, offset));
        }
        shift += 7;
        if shift >= 64 {
            return Err("Protobuf varint 过长".to_string());
        }
    }
}

fn skip_field(data: &[u8], offset: usize, wire_type: u8) -> Result<usize, String> {
    let next = match wire_type {
        0 => read_varint(data, offset)?.1,
        1 => offset.saturating_add(8),
        2 => {
            let (length, start) = read_varint(data, offset)?;
            start.saturating_add(length as usize)
        }
        5 => offset.saturating_add(4),
        _ => return Err("未知 Protobuf 字段类型".to_string()),
    };
    if next > data.len() {
        return Err("Protobuf 字段长度越界".to_string());
    }
    Ok(next)
}

fn len_field(field: u32, value: &[u8]) -> Vec<u8> {
    let mut output = encode_varint(u64::from((field << 3) | 2));
    output.extend(encode_varint(value.len() as u64));
    output.extend(value);
    output
}

fn string_field(field: u32, value: &str) -> Vec<u8> {
    len_field(field, value.as_bytes())
}

fn varint_field(field: u32, value: u64) -> Vec<u8> {
    let mut output = encode_varint(u64::from(field << 3));
    output.extend(encode_varint(value));
    output
}

fn create_oauth_info(account: &StoredAccount) -> Vec<u8> {
    let mut timestamp = varint_field(1, account.token.expires_at as u64);
    timestamp.extend(varint_field(2, 0));
    let mut output = Vec::new();
    output.extend(string_field(1, &account.token.access_token));
    output.extend(string_field(2, "Bearer"));
    output.extend(string_field(3, &account.token.refresh_token));
    output.extend(len_field(4, &timestamp));
    if let Some(id_token) = &account.token.id_token {
        output.extend(string_field(5, id_token));
    }
    if account.token.is_gcp_tos && !account.email.to_ascii_lowercase().ends_with("@gmail.com") {
        output.extend(varint_field(6, 1));
    }
    output
}

fn minimal_user_status(email: &str) -> Vec<u8> {
    [string_field(3, email), string_field(7, email)].concat()
}

fn create_unified_state_entry(sentinel_key: &str, payload: &[u8]) -> String {
    general_purpose::STANDARD.encode(create_unified_topic_entry(sentinel_key, payload))
}

fn create_unified_topic_entry(sentinel_key: &str, payload: &[u8]) -> Vec<u8> {
    let row = string_field(1, &general_purpose::STANDARD.encode(payload));
    let entry = [string_field(1, sentinel_key), len_field(2, &row)].concat();
    len_field(1, &entry)
}

fn topic_entry_key(entry: &[u8]) -> Option<&str> {
    let mut offset = 0;
    while offset < entry.len() {
        let (tag, content_start) = read_varint(entry, offset).ok()?;
        let wire_type = (tag & 7) as u8;
        let field = (tag >> 3) as u32;
        if field == 1 && wire_type == 2 {
            let (length, text_start) = read_varint(entry, content_start).ok()?;
            return std::str::from_utf8(entry.get(text_start..text_start + length as usize)?).ok();
        }
        offset = skip_field(entry, content_start, wire_type).ok()?;
    }
    None
}

fn remove_unified_topic_entry(data: &[u8], target_key: &str) -> Result<Vec<u8>, String> {
    let mut output = Vec::new();
    let mut offset = 0;
    while offset < data.len() {
        let start = offset;
        let (tag, content_start) = read_varint(data, offset)?;
        let wire_type = (tag & 7) as u8;
        let field = (tag >> 3) as u32;
        let next = skip_field(data, content_start, wire_type)?;
        let should_remove = if field == 1 && wire_type == 2 {
            let (length, entry_start) = read_varint(data, content_start)?;
            let entry_end = entry_start.saturating_add(length as usize);
            if entry_end > data.len() {
                return Err("IDE OAuth Topic 数据不完整".to_string());
            }
            topic_entry_key(&data[entry_start..entry_end]) == Some(target_key)
        } else {
            false
        };
        if !should_remove {
            output.extend_from_slice(&data[start..next]);
        }
        offset = next;
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::{
        apply_refreshed_token, backup_db, database_import_outcome, google_token_error,
        is_oauth_config_key, oauth_client_from_values, pkce_challenge,
        proxy_url_from_windows_proxy_list, should_retry_google_request_directly,
        sort_state_db_candidates, user_data_dir_from_command_line, write_file_atomically,
        DatabaseImportOutcome, GoogleTokenResponse, StoredToken, WSL_CREDENTIAL_READ_COMMAND,
        WSL_CREDENTIAL_WRITE_COMMAND,
    };
    use std::{
        ffi::OsString,
        fs,
        path::PathBuf,
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn finds_equals_form_user_data_dir() {
        let arguments = vec![
            OsString::from("Antigravity.exe"),
            OsString::from("--user-data-dir=C:\\Agy Data"),
        ];

        assert_eq!(
            user_data_dir_from_command_line(&arguments),
            Some(PathBuf::from("C:\\Agy Data"))
        );
    }

    #[test]
    fn finds_separate_form_user_data_dir() {
        let arguments = vec![
            OsString::from("Antigravity.exe"),
            OsString::from("--user-data-dir"),
            OsString::from("C:\\Agy Data"),
        ];

        assert_eq!(
            user_data_dir_from_command_line(&arguments),
            Some(PathBuf::from("C:\\Agy Data"))
        );
    }

    #[test]
    fn ignores_missing_user_data_dir_value() {
        let arguments = vec![
            OsString::from("Antigravity.exe"),
            OsString::from("--user-data-dir"),
        ];

        assert_eq!(user_data_dir_from_command_line(&arguments), None);
    }

    #[test]
    fn reports_duplicate_database_imports_without_claiming_a_new_account() {
        assert_eq!(
            database_import_outcome(None, "new-token"),
            DatabaseImportOutcome::Added
        );
        assert_eq!(
            database_import_outcome(Some("old-token"), "new-token"),
            DatabaseImportOutcome::Updated
        );
        assert_eq!(
            database_import_outcome(Some("same-token"), "same-token"),
            DatabaseImportOutcome::Unchanged
        );
    }

    #[test]
    fn reads_pac_proxy_directives() {
        assert_eq!(
            proxy_url_from_windows_proxy_list(
                "PROXY 127.0.0.1:10808; DIRECT",
                "https://oauth2.googleapis.com/token",
            ),
            Some("http://127.0.0.1:10808".to_string())
        );
    }

    #[test]
    fn preserves_socks_transport_from_windows_proxy_settings() {
        assert_eq!(
            proxy_url_from_windows_proxy_list(
                "SOCKS5 127.0.0.1:10808; DIRECT",
                "https://oauth2.googleapis.com/token",
            ),
            Some("socks5://127.0.0.1:10808".to_string())
        );
        assert_eq!(
            proxy_url_from_windows_proxy_list(
                "socks=127.0.0.1:10808",
                "https://oauth2.googleapis.com/token",
            ),
            Some("socks5://127.0.0.1:10808".to_string())
        );
    }

    #[test]
    fn prefers_the_matching_scheme_from_a_manual_proxy_setting() {
        assert_eq!(
            proxy_url_from_windows_proxy_list(
                "http=127.0.0.1:8080;https=127.0.0.1:10808",
                "https://oauth2.googleapis.com/token",
            ),
            Some("http://127.0.0.1:10808".to_string())
        );
    }

    #[test]
    fn ignores_direct_only_proxy_results() {
        assert_eq!(
            proxy_url_from_windows_proxy_list("DIRECT", "https://oauth2.googleapis.com/token"),
            None
        );
    }

    #[test]
    fn retries_directly_only_after_a_system_proxy_connection_failure() {
        assert!(should_retry_google_request_directly(true, true));
        assert!(should_retry_google_request_directly(true, false));
        assert!(!should_retry_google_request_directly(false, true));
        assert!(!should_retry_google_request_directly(false, false));
    }

    #[test]
    fn pkce_challenge_uses_the_rfc_7636_s256_encoding() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn oauth_prefers_runtime_client_id_then_uses_bundled_id() {
        assert!(oauth_client_from_values(None, None, None).is_err());

        let bundled = oauth_client_from_values(None, Some("bundled-client-id"), None)
            .expect("a bundled installed-app client id should be sufficient for PKCE");
        assert_eq!(bundled.id, "bundled-client-id");
        assert_eq!(bundled.secret, None);

        let runtime = oauth_client_from_values(
            Some("runtime-client-id".to_string()),
            Some("bundled-client-id"),
            None,
        )
        .expect("a runtime client id should override the bundled client id");
        assert_eq!(runtime.id, "runtime-client-id");
        assert_eq!(runtime.secret, None);

        let custom_secret = oauth_client_from_values(
            Some("custom-client-id".to_string()),
            None,
            Some("custom-client-secret".to_string()),
        )
        .expect("a custom runtime client may still provide an optional secret");
        assert_eq!(custom_secret.id, "custom-client-id");
        assert_eq!(
            custom_secret.secret.as_deref(),
            Some("custom-client-secret")
        );
    }

    #[test]
    fn reports_actionable_google_oauth_errors_without_echoing_the_response() {
        let invalid_grant = google_token_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"invalid_grant","error_description":"provider detail"}"#,
        );
        assert!(invalid_grant.contains("invalid_grant"));
        assert!(invalid_grant.contains("添加账号 → OAuth 授权"));
        assert!(!invalid_grant.contains("provider detail"));

        let invalid_request = google_token_error(
            reqwest::StatusCode::BAD_REQUEST,
            r#"{"error":"invalid_request","error_description":"client_secret is missing"}"#,
        );
        assert!(invalid_request.contains("invalid_request"));
        assert!(invalid_request.contains("OAuth 重新授权"));
        assert!(!invalid_request.contains("client_secret"));
    }

    #[test]
    fn persists_rotated_refresh_tokens_and_preserves_them_when_absent() {
        let mut stored = StoredToken {
            refresh_token: "old-refresh".to_string(),
            access_token: "old-access".to_string(),
            expires_at: 0,
            id_token: Some("old-id".to_string()),
            is_gcp_tos: false,
            project_id: None,
        };

        apply_refreshed_token(
            &mut stored,
            GoogleTokenResponse {
                access_token: "new-access".to_string(),
                expires_in: 3_600,
                refresh_token: Some("new-refresh".to_string()),
                id_token: None,
            },
        );
        assert_eq!(stored.access_token, "new-access");
        assert_eq!(stored.refresh_token, "new-refresh");
        assert_eq!(stored.id_token.as_deref(), Some("old-id"));

        apply_refreshed_token(
            &mut stored,
            GoogleTokenResponse {
                access_token: "latest-access".to_string(),
                expires_in: 3_600,
                refresh_token: None,
                id_token: Some("latest-id".to_string()),
            },
        );
        assert_eq!(stored.access_token, "latest-access");
        assert_eq!(stored.refresh_token, "new-refresh");
        assert_eq!(stored.id_token.as_deref(), Some("latest-id"));
    }

    #[test]
    fn dotenv_loader_only_accepts_the_documented_oauth_keys() {
        assert!(is_oauth_config_key("AGY_GOOGLE_OAUTH_CLIENT_ID"));
        assert!(is_oauth_config_key("AGY_GOOGLE_OAUTH_CLIENT_SECRET"));
        assert!(!is_oauth_config_key("PATH"));
        assert!(!is_oauth_config_key("RUST_LOG"));
    }

    #[test]
    fn wsl_credential_commands_are_scoped_to_the_default_linux_user() {
        for command in [WSL_CREDENTIAL_WRITE_COMMAND, WSL_CREDENTIAL_READ_COMMAND] {
            assert!(command.contains("$HOME/.gemini/antigravity-cli/credentials.json"));
            assert!(!command.contains("wsl.localhost"));
            assert!(!command.contains("wsl$"));
            assert!(!command.contains("/home/*"));
        }
        assert!(WSL_CREDENTIAL_WRITE_COMMAND.contains("umask 077"));
        assert!(WSL_CREDENTIAL_WRITE_COMMAND.contains("chmod 600"));
    }

    #[test]
    fn newest_state_database_is_preferred_when_no_instance_is_running() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after the Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agy-switch-state-db-sort-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("temporary directory should be created");
        let old = directory.join("old-state.vscdb");
        let newest = directory.join("new-state.vscdb");
        fs::write(&old, []).expect("old state database should be created");
        thread::sleep(Duration::from_millis(25));
        fs::write(&newest, []).expect("new state database should be created");

        let mut candidates = vec![old, newest.clone()];
        sort_state_db_candidates(&mut candidates);

        let _ = fs::remove_dir_all(&directory);
        assert_eq!(candidates.first(), Some(&newest));
    }

    #[test]
    fn atomic_write_replaces_existing_file_without_removing_it_first() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after the Unix epoch")
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "agy-switch-atomic-write-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("temporary directory should be created");
        let path = directory.join("accounts.json");
        fs::write(&path, b"old").expect("existing file should be written");

        write_file_atomically(&path, b"new", "test data")
            .expect("atomic replacement should succeed");

        assert_eq!(
            fs::read(&path).expect("replacement should be readable"),
            b"new"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path)
                    .expect("replacement metadata should be readable")
                    .permissions()
                    .mode()
                    & 0o077,
                0,
                "credential-bearing files must not be group or world readable"
            );
        }
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn state_database_backups_do_not_collide_within_one_second() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after the Unix epoch")
            .as_nanos();
        let directory =
            std::env::temp_dir().join(format!("agy-switch-backup-{}-{unique}", std::process::id()));
        fs::create_dir_all(&directory).expect("temporary directory should be created");
        let database = directory.join("state.vscdb");
        fs::write(&database, b"database").expect("database fixture should be written");

        let first = backup_db(&database).expect("first backup should succeed");
        let second = backup_db(&database).expect("second backup should succeed");

        assert_ne!(first, second);
        assert!(first.is_file());
        assert!(second.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            for backup in [&first, &second] {
                assert_eq!(
                    fs::metadata(backup)
                        .expect("backup metadata should be readable")
                        .permissions()
                        .mode()
                        & 0o077,
                    0,
                    "state database backups must be private at creation"
                );
            }
        }
        let _ = fs::remove_dir_all(&directory);
    }
}

fn is_oauth_config_key(key: &str) -> bool {
    matches!(
        key,
        "AGY_GOOGLE_OAUTH_CLIENT_ID" | "AGY_GOOGLE_OAUTH_CLIENT_SECRET"
    )
}

fn load_env_file() {
    let mut candidates = vec![PathBuf::from(".env"), PathBuf::from("../.env")];
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(".env"));
        }
    }
    for candidate in candidates {
        if let Ok(content) = fs::read_to_string(&candidate) {
            for line in content.lines() {
                let line = line.trim();
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    let value = value.trim().trim_matches('"').trim_matches('\'');
                    if is_oauth_config_key(key) && env::var(key).is_err() {
                        env::set_var(key, value);
                    }
                }
            }
        }
    }
}

pub fn run() {
    load_env_file();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            list_accounts,
            prepare_oauth_url,
            open_oauth_browser,
            complete_oauth_login,
            submit_oauth_code,
            cancel_oauth_login,
            import_default_database,
            import_database_file,
            fetch_account_quota,
            refresh_all_quotas,
            add_account,
            import_accounts,
            import_v1_accounts,
            export_accounts_to_file,
            import_backup_file,
            delete_account,
            switch_account,
        ])
        .run(tauri::generate_context!())
        .expect("启动 Agy Switch 失败");
}
