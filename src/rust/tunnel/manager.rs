use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::RwLock;

fn resolve_cloudflared_path() -> Option<PathBuf> {
    // GUI App (Finder 启动) 往往缺少 Homebrew PATH，因此优先尝试常见绝对路径
    let candidates = [
        "/opt/homebrew/bin/cloudflared",
        "/usr/local/bin/cloudflared",
        "/usr/bin/cloudflared",
    ];

    candidates
        .into_iter()
        .map(PathBuf::from)
        .find(|path| path.is_file())
}

const CLOUDFLARED_TEMP_DIR: &str = "iterate-cloudflared";
const CUSTOMER_TOKEN_FILE_PREFIX: &str = "customer-token-";
const STALE_CUSTOMER_TOKEN_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);
const QUICK_TUNNEL_PREFERENCES_FILE: &str = "quick-tunnel.json";
const QUICK_TUNNEL_OWNER_FILE: &str = "quick-tunnel-owner.json";
const QUICK_TUNNEL_PROOF_ATTEMPTS: usize = 20;
const QUICK_TUNNEL_PROOF_RETRY_DELAY: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuickTunnelPreferences {
    version: u8,
    #[serde(rename = "quick_tunnel_consent_v1", alias = "consent_v1")]
    consent_v1: bool,
    #[serde(rename = "quick_tunnel_enabled", alias = "enabled")]
    enabled: bool,
    endpoint_epoch: u64,
    install_identity: String,
}

impl Default for QuickTunnelPreferences {
    fn default() -> Self {
        Self {
            version: 1,
            consent_v1: false,
            enabled: false,
            endpoint_epoch: 0,
            install_identity: format!("install-{}", uuid::Uuid::new_v4()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct QuickTunnelOwnerMarker {
    version: u8,
    uid: u32,
    install_identity: String,
    endpoint_epoch: u64,
    pid: u32,
    started_at_unix_ms: u128,
    process_start_time: String,
    executable_path: String,
    config_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProcessSnapshot {
    uid: u32,
    process_start_time: String,
    command: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuickTunnelStatus {
    pub state: TunnelState,
    pub phase: String,
    pub progress: u8,
    pub endpoint: Option<String>,
    pub verified: bool,
    pub error_code: Option<String>,
    pub consent_given: bool,
    pub enabled: bool,
    pub endpoint_epoch: u64,
    pub proof_checked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickTunnelPairingBinding {
    pub endpoint: String,
    pub install_identity: String,
    pub endpoint_epoch: u64,
}

/// Tunnel 状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TunnelState {
    Stopped,
    Starting,
    Running,
    Error,
}

/// Tunnel 状态详情
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelStatus {
    pub state: TunnelState,
    pub mode: String,
    pub domain: Option<String>,
    pub pid: Option<u32>,
    pub last_error: Option<String>,
    pub recent_logs: Vec<String>,
    pub origin_healthy: bool,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub progress: u8,
    #[serde(default)]
    pub error_code: Option<String>,
    #[serde(default)]
    pub consent_given: bool,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub endpoint_epoch: u64,
    #[serde(default)]
    pub proof_checked_at: Option<String>,
}

impl Default for TunnelStatus {
    fn default() -> Self {
        Self {
            state: TunnelState::Stopped,
            mode: "quick_tunnel".to_string(),
            domain: None,
            pid: None,
            last_error: None,
            recent_logs: Vec::new(),
            origin_healthy: false,
            verified: false,
            phase: "idle".to_string(),
            progress: 0,
            error_code: None,
            consent_given: false,
            enabled: false,
            endpoint_epoch: 0,
            proof_checked_at: None,
        }
    }
}

/// Tunnel 管理器
pub struct TunnelManager {
    child: Option<Child>,
    quick_tunnel_config: Option<PathBuf>,
    customer_token_file: Option<PathBuf>,
    status: TunnelStatus,
}

impl Default for TunnelManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TunnelManager {
    pub fn new() -> Self {
        let preferences = load_quick_tunnel_preferences().unwrap_or_default();
        let mut status = TunnelStatus::default();
        status.consent_given = preferences.consent_v1;
        status.enabled = preferences.enabled;
        status.endpoint_epoch = preferences.endpoint_epoch;
        Self {
            child: None,
            quick_tunnel_config: None,
            customer_token_file: None,
            status,
        }
    }
}

fn quick_tunnel_state_dir() -> PathBuf {
    crate::config::iterate_bridge_state_dir()
}

fn quick_tunnel_preferences_path() -> PathBuf {
    quick_tunnel_state_dir().join(QUICK_TUNNEL_PREFERENCES_FILE)
}

fn quick_tunnel_owner_path() -> PathBuf {
    quick_tunnel_state_dir().join(QUICK_TUNNEL_OWNER_FILE)
}

fn ensure_private_state_directory(path: &Path) -> Result<(), String> {
    std::fs::create_dir_all(path)
        .map_err(|error| format!("quick_tunnel_state_write_failed:{error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("quick_tunnel_state_write_failed:{error}"))?;
    }
    Ok(())
}

fn atomic_write_private_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "quick_tunnel_state_write_failed".to_string())?;
    ensure_private_state_directory(parent)?;
    let body = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("quick_tunnel_state_write_failed:{error}"))?;
    let temp_path = parent.join(format!(".quick-tunnel-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temp_path)
        .map_err(|error| format!("quick_tunnel_state_write_failed:{error}"))?;
    let result = (|| -> Result<(), String> {
        file.write_all(&body)
            .map_err(|error| format!("quick_tunnel_state_write_failed:{error}"))?;
        file.sync_all()
            .map_err(|error| format!("quick_tunnel_state_write_failed:{error}"))?;
        drop(file);
        std::fs::rename(&temp_path, path)
            .map_err(|error| format!("quick_tunnel_state_write_failed:{error}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn load_quick_tunnel_preferences() -> Result<QuickTunnelPreferences, String> {
    let path = quick_tunnel_preferences_path();
    match std::fs::read_to_string(&path) {
        Ok(raw) => serde_json::from_str(&raw)
            .map_err(|error| format!("quick_tunnel_state_invalid:{error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let preferences = QuickTunnelPreferences::default();
            atomic_write_private_json(&path, &preferences)?;
            Ok(preferences)
        }
        Err(error) => Err(format!("quick_tunnel_state_read_failed:{error}")),
    }
}

fn save_quick_tunnel_preferences(preferences: &QuickTunnelPreferences) -> Result<(), String> {
    atomic_write_private_json(&quick_tunnel_preferences_path(), preferences)
}

fn load_owner_marker() -> Result<Option<QuickTunnelOwnerMarker>, String> {
    match std::fs::read_to_string(quick_tunnel_owner_path()) {
        Ok(raw) => serde_json::from_str(&raw)
            .map(Some)
            .map_err(|error| format!("quick_tunnel_owner_invalid:{error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("quick_tunnel_owner_read_failed:{error}")),
    }
}

fn save_owner_marker(marker: &QuickTunnelOwnerMarker) -> Result<(), String> {
    atomic_write_private_json(&quick_tunnel_owner_path(), marker)
}

fn remove_owner_marker_if_matches(pid: u32, endpoint_epoch: u64) {
    let Ok(Some(marker)) = load_owner_marker() else {
        return;
    };
    if marker.pid == pid && marker.endpoint_epoch == endpoint_epoch {
        let _ = std::fs::remove_file(quick_tunnel_owner_path());
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

fn quick_tunnel_config_body(preferences: &QuickTunnelPreferences) -> String {
    format!(
        "# iterate_install_identity: {}\n# iterate_endpoint_epoch: {}\nloglevel: info\n",
        preferences.install_identity, preferences.endpoint_epoch
    )
}

fn create_quick_tunnel_config(preferences: &QuickTunnelPreferences) -> Result<PathBuf, String> {
    let base_dir = ensure_cloudflared_temp_dir()?;

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("读取系统时间失败: {}", e))?
        .as_millis();
    let path = base_dir.join(format!("quick-{}-{}.yml", std::process::id(), nonce));

    // 显式指定一份最小配置，避免 quick tunnel 读取用户已有的 ~/.cloudflared/config.yml。
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&path)
        .map_err(|e| format!("写入 cloudflared 临时配置失败: {}", e))?;
    if let Err(error) = file.write_all(quick_tunnel_config_body(preferences).as_bytes()) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("写入 cloudflared 临时配置失败: {}", error));
    }

    Ok(path)
}

fn cleanup_quick_tunnel_config(mgr: &mut TunnelManager) {
    if let Some(path) = mgr.quick_tunnel_config.take() {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("清理 cloudflared 临时配置失败 {}: {}", path.display(), e);
            }
        }
    }
}

fn cleanup_customer_token_file(mgr: &mut TunnelManager) {
    if let Some(path) = mgr.customer_token_file.take() {
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("清理 cloudflared token 文件失败 {}: {}", path.display(), e);
            }
        }
    }
}

fn create_customer_token_file(token: &str) -> Result<PathBuf, String> {
    use std::io::Write;

    let base_dir = ensure_cloudflared_temp_dir()?;
    cleanup_stale_customer_token_files(&base_dir);

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| format!("读取系统时间失败: {}", e))?
        .as_millis();
    let path = base_dir.join(format!(
        "{}{}-{}",
        CUSTOMER_TOKEN_FILE_PREFIX,
        std::process::id(),
        nonce
    ));

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }

    let mut file = options
        .open(&path)
        .map_err(|e| format!("创建 cloudflared token 文件失败: {}", e))?;
    if let Err(e) = file.write_all(token.trim().as_bytes()) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("写入 cloudflared token 文件失败: {}", e));
    }
    if let Err(e) = file.flush() {
        let _ = std::fs::remove_file(&path);
        return Err(format!("刷新 cloudflared token 文件失败: {}", e));
    }

    Ok(path)
}

fn ensure_cloudflared_temp_dir() -> Result<PathBuf, String> {
    let base_dir = std::env::temp_dir().join(CLOUDFLARED_TEMP_DIR);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true);
        builder.mode(0o700);
        builder
            .create(&base_dir)
            .map_err(|e| format!("创建 cloudflared 临时目录失败: {}", e))?;
        std::fs::set_permissions(&base_dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("设置 cloudflared 临时目录权限失败: {}", e))?;
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(&base_dir)
            .map_err(|e| format!("创建 cloudflared 临时目录失败: {}", e))?;
    }
    Ok(base_dir)
}

fn cleanup_stale_customer_token_files(base_dir: &Path) {
    let Ok(entries) = std::fs::read_dir(base_dir) else {
        return;
    };
    let now = SystemTime::now();

    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !file_name.starts_with(CUSTOMER_TOKEN_FILE_PREFIX) {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= STALE_CUSTOMER_TOKEN_MAX_AGE);
        if stale {
            let path = entry.path();
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!(
                    "清理过期 cloudflared token 文件失败 {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
}

/// 全局 Tunnel 管理器实例
static TUNNEL_MANAGER: Lazy<Arc<RwLock<TunnelManager>>> =
    Lazy::new(|| Arc::new(RwLock::new(TunnelManager::new())));

/// 域名提取正则
static DOMAIN_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"https://[a-zA-Z0-9-]+\.trycloudflare\.com").unwrap());

/// 获取全局 Tunnel 管理器
pub fn get_tunnel_manager() -> Arc<RwLock<TunnelManager>> {
    TUNNEL_MANAGER.clone()
}

fn read_process_field(pid: u32, field: &str) -> Option<String> {
    let output = std::process::Command::new("/bin/ps")
        .args(["-p", &pid.to_string(), "-o", field])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn process_snapshot(pid: u32) -> Option<ProcessSnapshot> {
    Some(ProcessSnapshot {
        uid: read_process_field(pid, "uid=")?.parse().ok()?,
        process_start_time: read_process_field(pid, "lstart=")?,
        command: read_process_field(pid, "command=")?,
    })
}

async fn wait_for_process_snapshot(pid: u32) -> Option<ProcessSnapshot> {
    for _ in 0..10 {
        if let Some(snapshot) = process_snapshot(pid) {
            return Some(snapshot);
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    None
}

fn expected_owner_command(marker: &QuickTunnelOwnerMarker) -> String {
    format!(
        "{} tunnel --config {} --url http://127.0.0.1:8080",
        marker.executable_path, marker.config_path
    )
}

fn owner_marker_matches_snapshot(
    marker: &QuickTunnelOwnerMarker,
    snapshot: &ProcessSnapshot,
    config_body: &str,
) -> bool {
    marker.version == 2
        && snapshot.uid == marker.uid
        && snapshot.process_start_time == marker.process_start_time
        && snapshot.command == expected_owner_command(marker)
        && config_body
            == format!(
                "# iterate_install_identity: {}\n# iterate_endpoint_epoch: {}\nloglevel: info\n",
                marker.install_identity, marker.endpoint_epoch
            )
}

fn owner_marker_command_matches(marker: &QuickTunnelOwnerMarker) -> bool {
    if marker.pid == std::process::id()
        || marker.uid != current_uid()
        || marker.install_identity.trim().is_empty()
        || marker.executable_path.trim().is_empty()
        || marker.config_path.trim().is_empty()
    {
        return false;
    }
    let config_path = PathBuf::from(&marker.config_path);
    let Ok(temp_dir) = ensure_cloudflared_temp_dir() else {
        return false;
    };
    if config_path.parent() != Some(temp_dir.as_path())
        || !config_path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.starts_with("quick-"))
    {
        return false;
    }

    let Some(snapshot) = process_snapshot(marker.pid) else {
        return false;
    };
    let Ok(config_body) = std::fs::read_to_string(config_path) else {
        return false;
    };
    owner_marker_matches_snapshot(marker, &snapshot, &config_body)
}

#[cfg(unix)]
fn process_exists(pid: u32) -> bool {
    (unsafe { libc::kill(pid as i32, 0) }) == 0
}

#[cfg(not(unix))]
fn process_exists(_pid: u32) -> bool {
    false
}

#[cfg(unix)]
async fn recover_stale_quick_tunnel_owner(
    preferences: &QuickTunnelPreferences,
) -> Result<(), String> {
    let Some(marker) = load_owner_marker()? else {
        return Ok(());
    };
    if marker.install_identity != preferences.install_identity {
        return Err("quick_tunnel_owner_identity_mismatch".to_string());
    }
    if !owner_marker_command_matches(&marker) {
        if process_exists(marker.pid) {
            return Err("quick_tunnel_owner_validation_failed".to_string());
        }
        let _ = std::fs::remove_file(quick_tunnel_owner_path());
        return Ok(());
    }

    if unsafe { libc::kill(marker.pid as i32, libc::SIGTERM) } != 0 {
        return Err("quick_tunnel_stale_child_stop_failed".to_string());
    }
    for _ in 0..20 {
        if unsafe { libc::kill(marker.pid as i32, 0) } != 0 {
            let _ = std::fs::remove_file(quick_tunnel_owner_path());
            let _ = std::fs::remove_file(&marker.config_path);
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err("quick_tunnel_stale_child_stop_failed".to_string())
}

#[cfg(not(unix))]
async fn recover_stale_quick_tunnel_owner(
    _preferences: &QuickTunnelPreferences,
) -> Result<(), String> {
    if load_owner_marker()?.is_some() {
        return Err("quick_tunnel_owner_recovery_unsupported".to_string());
    }
    Ok(())
}

fn websocket_probe_status_is_acceptable(status: reqwest::StatusCode) -> bool {
    matches!(
        status,
        reqwest::StatusCode::SWITCHING_PROTOCOLS | reqwest::StatusCode::UNAUTHORIZED
    )
}

fn public_route_proof_client(proxy_url: Option<&str>) -> Result<reqwest::Client, reqwest::Error> {
    let mut builder = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .http1_only();
    // LaunchServices need not inherit shell proxy variables. Use the user's
    // current system HTTPS proxy for both proof requests; otherwise stay direct.
    if let Some(url) = proxy_url {
        builder = builder.proxy(reqwest::Proxy::https(url)?);
    }
    builder.build()
}

async fn public_route_endpoint_proof(
    endpoint: &str,
    install_identity: &str,
    endpoint_epoch: Option<u64>,
) -> bool {
    let proxy_url = crate::ui::codex_quota::macos_system_https_proxy_url();
    let Ok(client) = public_route_proof_client(proxy_url.as_deref()) else {
        return false;
    };
    let health_url = format!(
        "{}/.well-known/iterate/health",
        endpoint.trim_end_matches('/')
    );
    let websocket_url = format!("{}/ws", endpoint.trim_end_matches('/'));
    for attempt in 0..3 {
        let health_ok = match client.get(&health_url).send().await {
            Ok(response) if response.status().is_success() => response
                .json::<serde_json::Value>()
                .await
                .ok()
                .is_some_and(|payload| {
                    let common_identity_matches = payload
                        .get("installation_proof")
                        .and_then(|value| value.as_str())
                        == Some(public_installation_proof(install_identity).as_str());
                    let route_generation_matches = endpoint_epoch.is_none_or(|endpoint_epoch| {
                        payload
                            .get("endpoint_proof")
                            .and_then(|value| value.as_str())
                            == Some(
                                public_endpoint_proof(install_identity, endpoint_epoch).as_str(),
                            )
                            && payload
                                .get("endpoint_epoch")
                                .and_then(|value| value.as_u64())
                                == Some(endpoint_epoch)
                    });
                    payload.get("ok").and_then(|value| value.as_bool()) == Some(true)
                        && payload.get("service").and_then(|value| value.as_str())
                            == Some("iterate")
                        && common_identity_matches
                        && route_generation_matches
                }),
            _ => false,
        };

        // Probe the exact WebSocket upgrade path over the same proxy-aware
        // HTTP/1.1 client used for the cryptographic health proof. A protected
        // Bridge intentionally answers 401 before upgrade; 101 is also valid.
        let websocket_ok = if health_ok {
            client
                .get(&websocket_url)
                .header("Connection", "Upgrade")
                .header("Upgrade", "websocket")
                .header("Sec-WebSocket-Version", "13")
                .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
                .send()
                .await
                .is_ok_and(|response| websocket_probe_status_is_acceptable(response.status()))
        } else {
            false
        };
        if websocket_ok {
            return true;
        }

        // A single edge timeout or route-reload response must not tear down the
        // QR entry. Three bounded attempts still fail closed for a genuinely
        // unreachable or identity-mismatched endpoint.
        if attempt < 2 {
            tokio::time::sleep(Duration::from_millis(250 * (attempt + 1))).await;
        }
    }

    false
}

async fn quick_tunnel_endpoint_proof(
    endpoint: &str,
    install_identity: &str,
    endpoint_epoch: u64,
) -> bool {
    public_route_endpoint_proof(endpoint, install_identity, Some(endpoint_epoch)).await
}

pub async fn public_endpoint_proves_current_install(endpoint: &str) -> bool {
    if !endpoint.trim().starts_with("https://") {
        return false;
    }
    let Ok(preferences) = load_quick_tunnel_preferences() else {
        return false;
    };
    public_route_endpoint_proof(
        endpoint.trim().trim_end_matches('/'),
        &preferences.install_identity,
        None,
    )
    .await
}

pub async fn proven_binding_for_public_endpoint(
    endpoint: &str,
) -> Option<QuickTunnelPairingBinding> {
    if !endpoint.trim().starts_with("https://") {
        return None;
    }
    let preferences = load_quick_tunnel_preferences().ok()?;
    let binding = QuickTunnelPairingBinding {
        endpoint: endpoint.trim().trim_end_matches('/').to_string(),
        install_identity: preferences.install_identity,
        endpoint_epoch: preferences.endpoint_epoch,
    };
    public_route_endpoint_proof(&binding.endpoint, &binding.install_identity, None)
        .await
        .then_some(binding)
}

async fn verify_quick_tunnel_endpoint(
    endpoint: String,
    install_identity: String,
    endpoint_epoch: u64,
) {
    for _ in 0..QUICK_TUNNEL_PROOF_ATTEMPTS {
        if quick_tunnel_endpoint_proof(&endpoint, &install_identity, endpoint_epoch).await {
            let manager = get_tunnel_manager();
            let mut mgr = manager.write().await;
            if mgr.status.domain.as_deref() == Some(endpoint.as_str())
                && mgr.status.endpoint_epoch == endpoint_epoch
            {
                mgr.status.state = TunnelState::Running;
                mgr.status.verified = true;
                mgr.status.phase = "ready".to_string();
                mgr.status.progress = 100;
                mgr.status.error_code = None;
                mgr.status.last_error = None;
                mgr.status.proof_checked_at = Some(chrono::Utc::now().to_rfc3339());
            }
            return;
        }
        tokio::time::sleep(QUICK_TUNNEL_PROOF_RETRY_DELAY).await;
    }

    let manager = get_tunnel_manager();
    let mut mgr = manager.write().await;
    if mgr.status.domain.as_deref() == Some(endpoint.as_str())
        && mgr.status.endpoint_epoch == endpoint_epoch
    {
        mgr.status.state = TunnelState::Error;
        mgr.status.verified = false;
        mgr.status.phase = "proof_failed".to_string();
        mgr.status.progress = 75;
        mgr.status.error_code = Some("endpoint_proof_failed".to_string());
        mgr.status.last_error = Some("endpoint_proof_failed".to_string());
    }
}

fn public_endpoint_proof(install_identity: &str, endpoint_epoch: u64) -> String {
    let material = format!("iterate-quick-tunnel-proof-v1:{install_identity}:{endpoint_epoch}");
    let digest = ring::digest::digest(&ring::digest::SHA256, material.as_bytes());
    format!("sha256:{}", hex::encode(digest.as_ref()))
}

fn public_installation_proof(install_identity: &str) -> String {
    let material = format!("iterate-installation-proof-v1:{install_identity}");
    let digest = ring::digest::digest(&ring::digest::SHA256, material.as_bytes());
    format!("sha256:{}", hex::encode(digest.as_ref()))
}

pub fn installation_public_proof() -> String {
    let preferences = load_quick_tunnel_preferences().unwrap_or_default();
    public_installation_proof(&preferences.install_identity)
}

fn quick_tunnel_test_capability_enabled_for(
    debug_assertions: bool,
    environment_value: Option<&str>,
) -> bool {
    debug_assertions
        && environment_value.is_some_and(|value| matches!(value.trim(), "1" | "true" | "TRUE"))
}

pub fn quick_tunnel_test_capability_enabled() -> bool {
    quick_tunnel_test_capability_enabled_for(
        cfg!(debug_assertions),
        std::env::var("ITERATE_ENABLE_QUICK_TUNNEL_TEST")
            .ok()
            .as_deref(),
    )
}

pub fn quick_tunnel_public_proof() -> (String, u64) {
    let preferences = load_quick_tunnel_preferences().unwrap_or_default();
    (
        public_endpoint_proof(&preferences.install_identity, preferences.endpoint_epoch),
        preferences.endpoint_epoch,
    )
}

pub async fn pairing_binding() -> Option<QuickTunnelPairingBinding> {
    let preferences = load_quick_tunnel_preferences().ok()?;
    let status = get_status().await;
    if status.state != TunnelState::Running || !status.verified || !status.enabled {
        return None;
    }
    Some(QuickTunnelPairingBinding {
        endpoint: status.domain?,
        install_identity: preferences.install_identity,
        endpoint_epoch: status.endpoint_epoch,
    })
}

pub async fn pairing_binding_is_current(binding: &QuickTunnelPairingBinding) -> bool {
    let Ok(preferences) = load_quick_tunnel_preferences() else {
        return false;
    };
    if preferences.install_identity != binding.install_identity
        || preferences.endpoint_epoch != binding.endpoint_epoch
    {
        return false;
    }
    quick_tunnel_endpoint_proof(
        &binding.endpoint,
        &binding.install_identity,
        binding.endpoint_epoch,
    )
    .await
}

pub fn pairing_binding_matches_current_install(binding: &QuickTunnelPairingBinding) -> bool {
    load_quick_tunnel_preferences()
        .is_ok_and(|preferences| preferences.install_identity == binding.install_identity)
}

fn spawn_log_reader<R>(stream: Option<R>, manager_clone: Arc<RwLock<TunnelManager>>)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    if let Some(stream) = stream {
        tokio::spawn(async move {
            let reader = BufReader::new(stream);
            let mut lines = reader.lines();

            while let Ok(Some(line)) = lines.next_line().await {
                let mut mgr = manager_clone.write().await;
                let lower = line.to_lowercase();

                // 保留最近 50 条日志
                mgr.status.recent_logs.push(line.clone());
                if mgr.status.recent_logs.len() > 50 {
                    mgr.status.recent_logs.remove(0);
                }

                // 尝试提取域名
                if mgr.status.domain.is_none() {
                    if let Some(captures) = DOMAIN_REGEX.find(&line) {
                        let domain = captures.as_str().to_string();
                        log::info!("Tunnel 域名已获取: {}", domain);
                        mgr.status.domain = Some(domain.clone());
                        mgr.status.phase = "verifying_endpoint".to_string();
                        mgr.status.progress = 70;
                        let preferences = load_quick_tunnel_preferences().unwrap_or_default();
                        let endpoint_epoch = mgr.status.endpoint_epoch;
                        tokio::spawn(verify_quick_tunnel_endpoint(
                            domain,
                            preferences.install_identity,
                            endpoint_epoch,
                        ));
                    }
                }

                // 检测错误（cloudflared 常见是大写 ERR）
                let is_error = lower.contains("error")
                    || lower.contains("failed")
                    || line.contains(" ERR")
                    || line.contains("ERR ");

                if is_error {
                    // 不打断已 Running 的隧道，只记录 last_error
                    mgr.status.last_error = Some(line.clone());
                    if mgr.status.state != TunnelState::Running {
                        mgr.status.state = TunnelState::Error;
                        mgr.status.error_code = Some("cloudflared_start_failed".to_string());
                    }
                }
            }

            // 日志流结束（通常意味着进程退出）
            let mut mgr = manager_clone.write().await;
            let exited = mgr
                .child
                .as_mut()
                .and_then(|child| child.try_wait().ok().flatten())
                .is_some();
            if exited {
                if let (Some(pid), endpoint_epoch) = (mgr.status.pid, mgr.status.endpoint_epoch) {
                    remove_owner_marker_if_matches(pid, endpoint_epoch);
                }
                mgr.status.state = TunnelState::Stopped;
                mgr.child = None;
                cleanup_quick_tunnel_config(&mut mgr);
                cleanup_customer_token_file(&mut mgr);
            }
        });
    }
}

/// 启动 cloudflared tunnel
pub async fn start_tunnel() -> Result<String, String> {
    start_quick_tunnel(false).await
}

pub async fn start_quick_tunnel(consent_v1: bool) -> Result<String, String> {
    if !quick_tunnel_test_capability_enabled() {
        return Err("quick_tunnel_test_capability_disabled".to_string());
    }
    let manager = get_tunnel_manager();
    let mut mgr = manager.write().await;

    let mut preferences = load_quick_tunnel_preferences()?;
    if consent_v1 {
        preferences.consent_v1 = true;
    }
    if !preferences.consent_v1 {
        mgr.status.error_code = Some("quick_tunnel_consent_required".to_string());
        return Err("quick_tunnel_consent_required".to_string());
    }
    if mgr.child.is_some()
        && mgr.status.mode == "quick_tunnel"
        && matches!(
            mgr.status.state,
            TunnelState::Starting | TunnelState::Running
        )
    {
        if !preferences.enabled {
            preferences.enabled = true;
            save_quick_tunnel_preferences(&preferences)?;
        }
        mgr.status.consent_given = true;
        mgr.status.enabled = true;
        return Ok("quick_tunnel_already_active".to_string());
    }
    preferences.enabled = true;
    preferences.endpoint_epoch = preferences.endpoint_epoch.saturating_add(1);
    save_quick_tunnel_preferences(&preferences)?;

    // 如果已经在运行，先停止
    if mgr.child.is_some() {
        drop(mgr);
        stop_tunnel_internal().await?;
        mgr = manager.write().await;
    }

    drop(mgr);
    recover_stale_quick_tunnel_owner(&preferences).await?;
    let mut mgr = manager.write().await;

    mgr.status.state = TunnelState::Starting;
    mgr.status.mode = "quick_tunnel".to_string();
    mgr.status.last_error = None;
    mgr.status.domain = None;
    mgr.status.recent_logs.clear();
    mgr.status.verified = false;
    mgr.status.phase = "starting_cloudflared".to_string();
    mgr.status.progress = 35;
    mgr.status.error_code = None;
    mgr.status.consent_given = preferences.consent_v1;
    mgr.status.enabled = preferences.enabled;
    mgr.status.endpoint_epoch = preferences.endpoint_epoch;
    mgr.status.proof_checked_at = None;

    let cloudflared_bin = resolve_cloudflared_path().ok_or_else(|| {
        mgr.status.state = TunnelState::Error;
        mgr.status.phase = "cloudflared_missing".to_string();
        mgr.status.progress = 15;
        mgr.status.error_code = Some("cloudflared_missing".to_string());
        let msg = "cloudflared_missing".to_string();
        mgr.status.last_error = Some(msg.clone());
        msg
    })?;

    let quick_tunnel_config = create_quick_tunnel_config(&preferences).map_err(|e| {
        mgr.status.state = TunnelState::Error;
        mgr.status.last_error = Some(e.clone());
        e
    })?;

    // 启动 cloudflared 进程
    let mut cmd = Command::new(&cloudflared_bin);
    // 兜底补齐 PATH，避免 Finder 启动应用时环境变量缺失
    cmd.env(
        "PATH",
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    );

    let mut child = cmd
        .arg("tunnel")
        .arg("--config")
        .arg(&quick_tunnel_config)
        .args(["--url", "http://127.0.0.1:8080"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            let _ = std::fs::remove_file(&quick_tunnel_config);
            mgr.status.state = TunnelState::Error;
            mgr.status.last_error = Some(format!("启动 cloudflared 失败: {}", e));
            format!("启动 cloudflared 失败: {}。请确保已安装 cloudflared", e)
        })?;

    let pid = child.id();
    let pid = pid.ok_or_else(|| "cloudflared_pid_unavailable".to_string())?;
    let Some(process_start_time) = wait_for_process_snapshot(pid)
        .await
        .map(|snapshot| snapshot.process_start_time)
    else {
        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = std::fs::remove_file(&quick_tunnel_config);
        mgr.status.state = TunnelState::Error;
        mgr.status.error_code = Some("cloudflared_process_identity_unavailable".to_string());
        return Err("cloudflared_process_identity_unavailable".to_string());
    };
    mgr.status.pid = Some(pid);
    let marker = QuickTunnelOwnerMarker {
        version: 2,
        uid: current_uid(),
        install_identity: preferences.install_identity.clone(),
        endpoint_epoch: preferences.endpoint_epoch,
        pid,
        started_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
        process_start_time,
        executable_path: cloudflared_bin.to_string_lossy().to_string(),
        config_path: quick_tunnel_config.to_string_lossy().to_string(),
    };
    if let Err(error) = save_owner_marker(&marker) {
        let _ = child.kill().await;
        let _ = child.wait().await;
        let _ = std::fs::remove_file(&quick_tunnel_config);
        mgr.status.state = TunnelState::Error;
        mgr.status.error_code = Some("quick_tunnel_owner_write_failed".to_string());
        return Err(error);
    }
    mgr.quick_tunnel_config = Some(quick_tunnel_config);
    mgr.customer_token_file = None;

    // 同时读取 stdout/stderr（cloudflared 的域名输出可能在 stdout）
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    mgr.child = Some(child);

    // 释放锁，开始异步读取日志
    drop(mgr);

    // 分别读取 stdout / stderr
    spawn_log_reader(stdout, manager.clone());
    spawn_log_reader(stderr, manager.clone());

    Ok("quick_tunnel_starting".to_string())
}

/// 启动客户自有 Cloudflare Tunnel
pub async fn start_customer_tunnel(
    public_hostname: String,
    tunnel_token: String,
) -> Result<String, String> {
    let public_hostname = normalize_public_hostname(&public_hostname)?;
    let trimmed_token = tunnel_token.trim();
    if trimmed_token.is_empty() {
        return Err("token_missing".to_string());
    }

    let manager = get_tunnel_manager();
    let mut mgr = manager.write().await;

    if mgr.child.is_some() {
        drop(mgr);
        stop_tunnel_internal().await?;
        mgr = manager.write().await;
    }

    mgr.status.state = TunnelState::Starting;
    mgr.status.mode = "customer_tunnel".to_string();
    mgr.status.last_error = None;
    mgr.status.domain = Some(public_hostname);
    mgr.status.recent_logs.clear();

    let cloudflared_bin = resolve_cloudflared_path().ok_or_else(|| {
        mgr.status.state = TunnelState::Error;
        let msg = "启动 cloudflared 失败: No such file or directory (os error 2)。请确保已安装 cloudflared".to_string();
        mgr.status.last_error = Some(msg.clone());
        msg
    })?;

    let token_file = create_customer_token_file(trimmed_token).map_err(|e| {
        mgr.status.state = TunnelState::Error;
        mgr.status.last_error = Some(e.clone());
        e
    })?;

    let mut cmd = Command::new(cloudflared_bin);
    cmd.env(
        "PATH",
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
    );

    let mut child = cmd
        .arg("tunnel")
        .arg("run")
        .arg("--token-file")
        .arg(&token_file)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| {
            let _ = std::fs::remove_file(&token_file);
            mgr.status.state = TunnelState::Error;
            mgr.status.last_error = Some(format!("启动 customer cloudflared 失败: {}", e));
            format!(
                "启动 customer cloudflared 失败: {}。请确保已安装 cloudflared",
                e
            )
        })?;

    let pid = child.id();
    mgr.status.pid = pid;
    mgr.customer_token_file = Some(token_file);
    mgr.quick_tunnel_config = None;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    mgr.child = Some(child);

    drop(mgr);

    spawn_log_reader(stdout, manager.clone());
    spawn_log_reader(stderr, manager.clone());

    Ok("Customer tunnel 启动中...".to_string())
}

/// 内部停止函数
async fn stop_tunnel_internal() -> Result<(), String> {
    let manager = get_tunnel_manager();
    let (child, endpoint_epoch) = {
        let mut mgr = manager.write().await;
        (mgr.child.take(), mgr.status.endpoint_epoch)
    };

    if let Some(mut child) = child {
        let pid = child.id();
        let _ = child.kill().await;
        let _ = child.wait().await;
        if let Some(pid) = pid {
            remove_owner_marker_if_matches(pid, endpoint_epoch);
        }
    } else {
        let preferences = load_quick_tunnel_preferences()?;
        recover_stale_quick_tunnel_owner(&preferences).await?;
    }

    let mut mgr = manager.write().await;
    cleanup_quick_tunnel_config(&mut mgr);
    cleanup_customer_token_file(&mut mgr);

    mgr.status.state = TunnelState::Stopped;
    mgr.status.domain = None;
    mgr.status.pid = None;
    mgr.status.recent_logs.clear();
    mgr.status.verified = false;
    mgr.status.phase = "idle".to_string();
    mgr.status.progress = 0;
    mgr.status.proof_checked_at = None;

    Ok(())
}

pub fn normalize_public_hostname(value: &str) -> Result<String, String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("hostname_missing".to_string());
    }
    let parsed = reqwest::Url::parse(trimmed).map_err(|_| "hostname_invalid".to_string())?;
    if parsed.scheme() != "https" {
        return Err("hostname_must_be_https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("hostname_invalid".to_string());
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("hostname_origin_only".to_string());
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| "hostname_invalid".to_string())?;
    let authority = match parsed.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    };
    Ok(format!("{}://{}", parsed.scheme(), authority))
}

/// 停止 tunnel
pub async fn stop_tunnel() -> Result<String, String> {
    let mut preferences = load_quick_tunnel_preferences()?;
    preferences.enabled = false;
    preferences.endpoint_epoch = preferences.endpoint_epoch.saturating_add(1);
    save_quick_tunnel_preferences(&preferences)?;
    stop_tunnel_internal().await?;
    let manager = get_tunnel_manager();
    let mut mgr = manager.write().await;
    mgr.status.enabled = false;
    mgr.status.endpoint_epoch = preferences.endpoint_epoch;
    mgr.status.error_code = None;
    drop(mgr);
    crate::bridge::ws::invalidate_quick_tunnel_pairing_tokens().await;
    Ok("Tunnel 已停止".to_string())
}

/// 获取 tunnel 状态
pub async fn get_status() -> TunnelStatus {
    let manager = get_tunnel_manager();
    let mgr = manager.read().await;

    let mut status = mgr.status.clone();

    // 检查进程是否还在运行
    if (status.state == TunnelState::Running || status.state == TunnelState::Starting)
        && mgr.child.is_none()
    {
        status.state = TunnelState::Stopped;
    }

    status
}

pub async fn get_quick_status() -> QuickTunnelStatus {
    let status = get_status().await;
    QuickTunnelStatus {
        state: status.state,
        phase: status.phase,
        progress: status.progress,
        endpoint: status.domain,
        verified: status.verified,
        error_code: status.error_code,
        consent_given: status.consent_given,
        enabled: status.enabled,
        endpoint_epoch: status.endpoint_epoch,
        proof_checked_at: status.proof_checked_at,
    }
}

pub async fn autostart_quick_tunnel() {
    let Ok(mut preferences) = load_quick_tunnel_preferences() else {
        return;
    };
    if !quick_tunnel_test_capability_enabled() {
        if preferences.enabled {
            preferences.enabled = false;
            if let Err(error) = save_quick_tunnel_preferences(&preferences) {
                log::warn!("禁用生产 Quick Tunnel 自动启动失败: {}", error);
            }
        }
        if let Err(error) = recover_stale_quick_tunnel_owner(&preferences).await {
            log::warn!("清理生产 Quick Tunnel 遗留进程失败: {}", error);
        }
        let manager = get_tunnel_manager();
        let mut mgr = manager.write().await;
        mgr.status.enabled = false;
        mgr.status.state = TunnelState::Stopped;
        mgr.status.phase = "test_capability_disabled".to_string();
        mgr.status.error_code = None;
        return;
    }
    if !preferences.enabled || !preferences.consent_v1 {
        return;
    }
    if let Err(error) = start_quick_tunnel(false).await {
        log::warn!("Quick Tunnel 自动启动失败: {}", error);
    }
}

/// 检查本机 8080 端口是否可达
pub async fn check_origin_health() -> Result<bool, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    match client.get("http://127.0.0.1:8080").send().await {
        Ok(_) => {
            let manager = get_tunnel_manager();
            let mut mgr = manager.write().await;
            mgr.status.origin_healthy = true;
            Ok(true)
        }
        Err(e) => {
            let manager = get_tunnel_manager();
            let mut mgr = manager.write().await;
            mgr.status.origin_healthy = false;
            mgr.status.last_error = Some(format!("8080 端口不可达: {}", e));
            Ok(false)
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn test_preferences() -> QuickTunnelPreferences {
        QuickTunnelPreferences {
            version: 1,
            consent_v1: true,
            enabled: true,
            endpoint_epoch: 7,
            install_identity: "install-test".to_string(),
        }
    }

    fn test_owner_marker() -> QuickTunnelOwnerMarker {
        QuickTunnelOwnerMarker {
            version: 2,
            uid: 501,
            install_identity: "install-test".to_string(),
            endpoint_epoch: 7,
            pid: 4242,
            started_at_unix_ms: 1,
            process_start_time: "Fri Aug 14 12:00:00 2026".to_string(),
            executable_path: "/opt/homebrew/bin/cloudflared".to_string(),
            config_path: "/tmp/iterate-cloudflared/quick-1-2.yml".to_string(),
        }
    }

    #[test]
    fn preferences_persist_the_frozen_non_secret_keys() {
        let value = serde_json::to_value(test_preferences()).expect("serialize preferences");
        assert_eq!(value["quick_tunnel_consent_v1"], true);
        assert_eq!(value["quick_tunnel_enabled"], true);
        assert!(value.get("consent_v1").is_none());
        assert!(value.get("enabled").is_none());
    }

    #[test]
    fn endpoint_proof_changes_with_install_or_epoch_without_disclosing_identity() {
        let proof = public_endpoint_proof("install-secret", 7);
        assert!(proof.starts_with("sha256:"));
        assert!(!proof.contains("install-secret"));
        assert_ne!(proof, public_endpoint_proof("install-secret", 8));
        assert_ne!(proof, public_endpoint_proof("other-install", 7));
    }

    #[test]
    fn formal_route_installation_proof_is_stable_across_quick_epochs() {
        let proof = public_installation_proof("install-secret");
        assert!(proof.starts_with("sha256:"));
        assert!(!proof.contains("install-secret"));
        assert_eq!(proof, public_installation_proof("install-secret"));
        assert_ne!(proof, public_installation_proof("other-install"));
    }

    #[test]
    fn quick_tunnel_test_capability_requires_debug_build_and_explicit_opt_in() {
        assert!(!quick_tunnel_test_capability_enabled_for(false, Some("1")));
        assert!(!quick_tunnel_test_capability_enabled_for(true, None));
        assert!(!quick_tunnel_test_capability_enabled_for(
            true,
            Some("false")
        ));
        assert!(quick_tunnel_test_capability_enabled_for(true, Some("1")));
    }

    #[test]
    fn websocket_route_proof_accepts_upgrade_or_auth_gate_only() {
        assert!(websocket_probe_status_is_acceptable(
            reqwest::StatusCode::SWITCHING_PROTOCOLS
        ));
        assert!(websocket_probe_status_is_acceptable(
            reqwest::StatusCode::UNAUTHORIZED
        ));
        assert!(!websocket_probe_status_is_acceptable(
            reqwest::StatusCode::OK
        ));
        assert!(!websocket_probe_status_is_acceptable(
            reqwest::StatusCode::FOUND
        ));
    }

    #[test]
    fn stale_owner_requires_exact_process_and_config_identity() {
        let marker = test_owner_marker();
        let snapshot = ProcessSnapshot {
            uid: marker.uid,
            process_start_time: marker.process_start_time.clone(),
            command: expected_owner_command(&marker),
        };
        let config_body = quick_tunnel_config_body(&test_preferences());
        assert!(owner_marker_matches_snapshot(
            &marker,
            &snapshot,
            &config_body
        ));

        let mut wrong_start = snapshot.clone();
        wrong_start.process_start_time = "Fri Aug 14 12:00:01 2026".to_string();
        assert!(!owner_marker_matches_snapshot(
            &marker,
            &wrong_start,
            &config_body
        ));

        let mut wrong_command = snapshot.clone();
        wrong_command.command.push_str(" --metrics 127.0.0.1:9999");
        assert!(!owner_marker_matches_snapshot(
            &marker,
            &wrong_command,
            &config_body
        ));

        let wrong_epoch = config_body.replace("endpoint_epoch: 7", "endpoint_epoch: 8");
        assert!(!owner_marker_matches_snapshot(
            &marker,
            &snapshot,
            &wrong_epoch
        ));
    }

    #[test]
    fn customer_token_file_is_created_private_on_unix() {
        let path = create_customer_token_file("  test-secret-token  ")
            .expect("customer token file should be created");
        let metadata = std::fs::metadata(&path).expect("token metadata should be readable");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::read_to_string(&path).expect("token file should be readable by owner"),
            "test-secret-token"
        );

        let dir = path.parent().expect("token file should have parent dir");
        let dir_metadata = std::fs::metadata(dir).expect("temp dir metadata should be readable");
        assert_eq!(dir_metadata.permissions().mode() & 0o777, 0o700);

        std::fs::remove_file(&path).expect("token file should be removable");
    }

    #[test]
    fn quick_tunnel_config_is_private_and_identity_bound() {
        let preferences = test_preferences();
        let path =
            create_quick_tunnel_config(&preferences).expect("quick config should be created");
        let metadata = std::fs::metadata(&path).expect("quick config metadata");
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::read_to_string(&path).expect("quick config contents"),
            quick_tunnel_config_body(&preferences)
        );
        std::fs::remove_file(path).expect("quick config should be removable");
    }
}

#[cfg(test)]
mod public_route_proxy_tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn requests(listener: TcpListener, responses: Vec<&'static [u8]>) -> Vec<String> {
        listener.set_nonblocking(true).unwrap();
        responses.into_iter().map(|response| {
            let deadline = std::time::Instant::now() + Duration::from_secs(15); // 冷启动 + 全量并行时客户端初始化可能超过 3 秒
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(std::time::Instant::now() < deadline, "probe did not reach expected endpoint");
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Err(e) => panic!("{e}"),
                }
            };
            // macOS 上从非阻塞 listener accept 出来的连接会继承 O_NONBLOCK，数据未到时 read_exact 报 WouldBlock。
            stream.set_nonblocking(false).unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).unwrap();
                header.push(byte[0]);
                assert!(header.len() <= 8192);
            }
            stream.write_all(response).unwrap();
            String::from_utf8(header).unwrap()
        }).collect()
    }

    #[test]
    fn system_proxy_handles_health_and_websocket_https_authority() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || requests(listener, vec![
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"; 2
        ]));
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let client = public_route_proof_client(Some(&proxy)).unwrap();
            for path in ["/.well-known/iterate/health", "/ws"] {
                assert!(client.get(format!("https://public-proof.invalid{path}")).send().await.is_err());
            }
        });
        for request in worker.join().unwrap() {
            assert!(request.starts_with("CONNECT public-proof.invalid:443 HTTP/1.1\r\n"));
            assert!(request.to_ascii_lowercase().contains("\r\nhost: public-proof.invalid:443\r\n"));
        }
    }

    #[test]
    fn absent_system_proxy_stays_direct_and_redirects_remain_disabled() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let worker = std::thread::spawn(move || requests(listener, vec![
            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            b"HTTP/1.1 302 Found\r\nLocation: https://must-not-follow.invalid/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ]));
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let client = public_route_proof_client(None).unwrap();
            assert!(client.get(format!("{origin}/.well-known/iterate/health")).send().await.unwrap().status().is_success());
            assert!(websocket_probe_status_is_acceptable(client.get(format!("{origin}/ws")).send().await.unwrap().status()));
            assert_eq!(client.get(format!("{origin}/redirect")).send().await.unwrap().status(), reqwest::StatusCode::FOUND);
        });
        let seen = worker.join().unwrap();
        assert!(seen[0].starts_with("GET /.well-known/iterate/health "));
        assert!(seen[1].starts_with("GET /ws "));
    }
}
