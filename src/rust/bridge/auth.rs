//! Per-user authentication primitives for the Bridge control plane.
//!
//! The Bridge listener deliberately serves LAN, Tailscale, and tunnel clients.
//! Network location is therefore never a sufficient authorization signal. This
//! module owns the small amount of durable secret material used to mint very
//! short-lived, request-bound capabilities for trusted desktop callers.

use axum::http::{header, HeaderMap};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Duration, Utc};
use once_cell::sync::Lazy;
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, OnceLock,
    },
    time::Duration as StdDuration,
};

#[cfg(target_os = "windows")]
mod windows_broker;

#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, RawFd};
#[cfg(target_os = "macos")]
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
#[cfg(target_os = "macos")]
use std::os::unix::net::UnixStream as StdUnixStream;

pub const AUTH_COOKIE_NAME: &str = "iterate_bridge_auth";

const MASTER_KEY_BYTES: usize = 32;
const MAX_TOKEN_BYTES: usize = 4 * 1024;
const TOKEN_PREFIX: &str = "ibi1";
const DESKTOP_TOKEN_TTL_SECONDS: i64 = 20;
const INTERNAL_TOKEN_TTL_SECONDS: i64 = 45;
const AUTH_BROKER_PROTOCOL_VERSION: u8 = 2;
const AUTH_BROKER_MAX_MESSAGE_BYTES: u64 = 4 * 1024;
const AUTH_BROKER_SOCKET_FILE: &str = "bridge-auth-broker.sock";
#[cfg(target_os = "macos")]
const AUTH_BROKER_LOCK_FILE: &str = "bridge-auth-broker.lock";
const ITERATE_CODE_REQUIREMENT: &str = concat!(
    "(identifier \"com.kexin94yyds.iterate\" or ",
    "identifier \"com.kexin94yyds.iterate.mcp-server\") and anchor apple generic and ",
    "certificate 1[field.1.2.840.113635.100.6.2.6] exists and ",
    "certificate leaf[field.1.2.840.113635.100.6.1.13] exists and ",
    "certificate leaf[subject.OU] = \"UM3Z9G5DNH\""
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BridgeTokenAudience {
    /// Issued only through the native Tauri command.  It is path/method bound
    /// and permits the desktop renderer to make one local Bridge request.
    DesktopRenderer,
    /// Issued inside Rust only for a real process boundary (standalone popup,
    /// relay client, MCP helper, or canonical GUI helper).
    InternalProcess,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BridgeTokenClaims {
    version: u8,
    audience: BridgeTokenAudience,
    method: String,
    path: String,
    issued_at: i64,
    expires_at: i64,
    nonce: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedAuthState {
    version: u8,
    enforcement_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthBrokerRequest {
    version: u8,
    audience: BridgeTokenAudience,
    method: String,
    path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    context: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct AuthBrokerResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

static SERVER_MASTER_KEY: OnceLock<Vec<u8>> = OnceLock::new();
static ENFORCEMENT_ENABLED: Lazy<AtomicBool> =
    Lazy::new(|| AtomicBool::new(load_enforcement_state()));
static USED_CAPABILITY_NONCES: Lazy<Mutex<std::collections::HashMap<String, i64>>> =
    Lazy::new(|| Mutex::new(std::collections::HashMap::new()));

fn auth_directory() -> PathBuf {
    crate::config::iterate_bridge_state_dir()
}

fn auth_state_path() -> PathBuf {
    auth_directory().join("bridge-auth-state.json")
}

fn random_url_safe_token(prefix: &str) -> Result<String, String> {
    let mut bytes = [0_u8; MASTER_KEY_BYTES];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "无法生成 Bridge 随机凭据".to_string())?;
    Ok(format!("{prefix}_{}", URL_SAFE_NO_PAD.encode(bytes)))
}

fn set_private_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("设置 Bridge 私有文件权限失败: {error}"))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn ensure_private_directory(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| format!("创建 Bridge 状态目录失败: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("设置 Bridge 私有目录权限失败: {error}"))?;
    }
    Ok(())
}

/// Creates a private file with 0600 permissions at creation time.  Setting the
/// mode only after open would leave a short umask-dependent exposure window.
fn open_new_private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn allocate_private_temp(parent: &Path, file_name: &str) -> Result<(PathBuf, fs::File), String> {
    for _ in 0..8 {
        let suffix = random_url_safe_token("tmp")?;
        let path = parent.join(format!(".{file_name}.{suffix}"));
        match open_new_private_file(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("创建 Bridge 私有临时文件失败: {error}")),
        }
    }
    Err("创建 Bridge 私有临时文件失败".to_string())
}

/// Atomically replaces a private durable state file.  The temp file is in the
/// same directory, synced before rename, and the directory is synced after it.
fn atomic_write_private(path: &Path, body: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Bridge 私有文件目录无效".to_string())?;
    ensure_private_directory(parent)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "Bridge 私有文件名无效".to_string())?;
    let (temp_path, mut temp_file) = allocate_private_temp(parent, file_name)?;

    let result = (|| -> Result<(), String> {
        temp_file
            .write_all(body)
            .map_err(|error| format!("写入 Bridge 状态文件失败: {error}"))?;
        temp_file
            .sync_all()
            .map_err(|error| format!("同步 Bridge 状态文件失败: {error}"))?;
        drop(temp_file);
        fs::rename(&temp_path, path)
            .map_err(|error| format!("原子更新 Bridge 状态失败: {error}"))?;
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| format!("同步 Bridge 状态目录失败: {error}"))?;
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn parse_enforcement_state(value: &str) -> Result<bool, String> {
    let state = serde_json::from_str::<PersistedAuthState>(value)
        .map_err(|_| "Bridge 鉴权状态文件格式无效".to_string())?;
    if state.version != 1 {
        return Err("Bridge 鉴权状态文件版本无效".to_string());
    }
    Ok(state.enforcement_enabled)
}

fn load_enforcement_state_from_path(path: &Path) -> bool {
    match fs::read_to_string(path) {
        Ok(value) => {
            if let Err(error) = set_private_permissions(path) {
                log::error!("[Bridge] {}，保持鉴权强制开启", error);
                return true;
            }
            match parse_enforcement_state(&value) {
                Ok(enabled) => enabled,
                Err(error) => {
                    log::error!("[Bridge] {}，保持鉴权强制开启", error);
                    true
                }
            }
        }
        // The secure default is enabled even on first launch.  Missing state
        // is not a legacy exemption because the listener binds 0.0.0.0.
        Err(error) if error.kind() == io::ErrorKind::NotFound => true,
        Err(error) => {
            log::error!("[Bridge] 读取鉴权状态失败: {}，保持鉴权强制开启", error);
            true
        }
    }
}

fn load_enforcement_state() -> bool {
    load_enforcement_state_from_path(&auth_state_path())
}

fn save_enforcement_state_to_path(path: &Path, enabled: bool) -> Result<(), String> {
    let body = serde_json::to_vec_pretty(&PersistedAuthState {
        version: 1,
        enforcement_enabled: enabled,
    })
    .map_err(|error| format!("序列化 Bridge 鉴权状态失败: {error}"))?;
    atomic_write_private(path, &body)
}

pub fn is_enforcement_enabled() -> bool {
    ENFORCEMENT_ENABLED.load(Ordering::Acquire)
}

/// Idempotently records the fail-closed policy.  This is called after a
/// successful pairing as an audit/durability point, not as a switch from an
/// insecure bootstrap mode.
pub fn enable_enforcement() -> Result<(), String> {
    save_enforcement_state_to_path(&auth_state_path(), true)?;
    ENFORCEMENT_ENABLED.store(true, Ordering::Release);
    Ok(())
}

fn auth_broker_socket_path() -> PathBuf {
    auth_directory().join(AUTH_BROKER_SOCKET_FILE)
}

#[cfg(target_os = "macos")]
struct AuthBrokerSocketGuard {
    socket_path: PathBuf,
    device: u64,
    inode: u64,
    lock_file: fs::File,
}

#[cfg(target_os = "macos")]
impl Drop for AuthBrokerSocketGuard {
    fn drop(&mut self) {
        if let Ok(metadata) = fs::symlink_metadata(&self.socket_path) {
            if metadata.file_type().is_socket()
                && metadata.dev() == self.device
                && metadata.ino() == self.inode
            {
                let _ = fs::remove_file(&self.socket_path);
            }
        }
        let _ = unsafe { libc::flock(self.lock_file.as_raw_fd(), libc::LOCK_UN) };
    }
}

#[cfg(target_os = "macos")]
fn socket_identity_from_fd(raw_fd: RawFd) -> Result<(u64, u64), String> {
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    if unsafe { libc::fstat(raw_fd, metadata.as_mut_ptr()) } != 0 {
        return Err("读取 Bridge 鉴权代理 socket 标识失败".to_string());
    }
    let metadata = unsafe { metadata.assume_init() };
    Ok((metadata.st_dev as u64, metadata.st_ino as u64))
}

pub(crate) struct InternalAuthBroker {
    task: tokio::task::JoinHandle<()>,
    #[cfg(target_os = "macos")]
    _socket_guard: AuthBrokerSocketGuard,
}

impl InternalAuthBroker {
    pub(crate) fn abort(&self) {
        self.task.abort();
    }
}

impl Drop for InternalAuthBroker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

fn initialize_server_master_key() -> Result<&'static [u8], String> {
    if let Some(key) = SERVER_MASTER_KEY.get() {
        return Ok(key.as_slice());
    }
    let mut key = vec![0_u8; MASTER_KEY_BYTES];
    SystemRandom::new()
        .fill(&mut key)
        .map_err(|_| "无法生成 Bridge 进程内主密钥".to_string())?;
    let _ = SERVER_MASTER_KEY.set(key);
    SERVER_MASTER_KEY
        .get()
        .map(Vec::as_slice)
        .ok_or_else(|| "Bridge 进程内主密钥不可用".to_string())
}

fn server_master_key() -> Result<&'static [u8], String> {
    SERVER_MASTER_KEY
        .get()
        .map(Vec::as_slice)
        .ok_or_else(|| "bridge_auth_broker_unavailable".to_string())
}

#[cfg(target_os = "macos")]
fn peer_pid(raw_fd: std::os::fd::RawFd) -> Result<libc::pid_t, String> {
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            raw_fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERPID,
            (&mut pid as *mut libc::pid_t).cast(),
            &mut length,
        )
    };
    if status != 0 || length as usize != std::mem::size_of::<libc::pid_t>() || pid <= 0 {
        return Err("bridge_auth_peer_pid_unavailable".to_string());
    }
    Ok(pid)
}

#[cfg(all(target_os = "macos", debug_assertions))]
fn debug_peer_is_same_executable(pid: libc::pid_t) -> bool {
    let mut buffer = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    let length =
        unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    if length <= 0 {
        return false;
    }
    buffer.truncate(length as usize);
    let Ok(peer_path) = std::str::from_utf8(&buffer) else {
        return false;
    };
    let Ok(current) = std::env::current_exe().and_then(fs::canonicalize) else {
        return false;
    };
    fs::canonicalize(peer_path).is_ok_and(|peer| peer == current)
}

#[cfg(target_os = "macos")]
fn trusted_iterate_peer(raw_fd: std::os::fd::RawFd) -> Result<(), String> {
    use core_foundation::{base::TCFType, data::CFData};
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement,
    };
    use std::str::FromStr;

    #[repr(C)]
    struct AuditToken {
        values: [u32; 8],
    }

    let pid = peer_pid(raw_fd)?;
    #[cfg(not(debug_assertions))]
    let _ = pid;
    let mut audit_token = AuditToken { values: [0; 8] };
    let mut length = std::mem::size_of::<AuditToken>() as libc::socklen_t;
    let status = unsafe {
        libc::getsockopt(
            raw_fd,
            libc::SOL_LOCAL,
            libc::LOCAL_PEERTOKEN,
            (&mut audit_token as *mut AuditToken).cast(),
            &mut length,
        )
    };
    if status != 0 || length as usize != std::mem::size_of::<AuditToken>() {
        return Err("bridge_auth_peer_audit_token_unavailable".to_string());
    }
    let bytes = unsafe {
        std::slice::from_raw_parts(
            (&audit_token as *const AuditToken).cast::<u8>(),
            std::mem::size_of::<AuditToken>(),
        )
    };
    let audit_data = CFData::from_buffer(bytes);
    let mut attributes = GuestAttributes::new();
    attributes.set_audit_token(audit_data.as_concrete_TypeRef());
    let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .map_err(|_| "bridge_auth_peer_code_unavailable".to_string())?;
    let requirement = SecRequirement::from_str(ITERATE_CODE_REQUIREMENT)
        .map_err(|_| "bridge_auth_code_requirement_invalid".to_string())?;
    // The requirement itself pins the Apple generic anchor, Developer ID
    // Application certificate OID, Team ID, and executable identifier.  Use
    // the Security.framework default validation mode here: STRICT_VALIDATE is
    // intended for bundle-shape/resource validation and rejects a valid signed
    // standalone helper executable even though its dynamic code identity and
    // designated requirement are valid. The installer keeps replaced bundles
    // alive while their processes are still running, so this validation never
    // has to compare an old process with unrelated bytes at the same path.
    if code.check_validity(Flags::NONE, &requirement).is_ok() {
        return Ok(());
    }

    #[cfg(debug_assertions)]
    if debug_peer_is_same_executable(pid) {
        return Ok(());
    }

    Err("bridge_auth_peer_not_trusted".to_string())
}

#[cfg(target_os = "macos")]
fn request_token_from_broker(
    audience: BridgeTokenAudience,
    method: &str,
    path: &str,
    context: Option<&str>,
) -> Result<String, String> {
    request_token_from_broker_at(&auth_broker_socket_path(), audience, method, path, context)
}

#[cfg(target_os = "macos")]
fn request_token_from_broker_at(
    socket_path: &Path,
    audience: BridgeTokenAudience,
    method: &str,
    path: &str,
    context: Option<&str>,
) -> Result<String, String> {
    let (method, path) = normalize_method_and_path(method, path)?;
    if !audience_allows_route(audience, &method, &path) {
        return Err("internal_bridge_route_not_allowed".to_string());
    }
    let mut stream = StdUnixStream::connect(socket_path)
        .map_err(|_| "bridge_auth_broker_unavailable".to_string())?;
    stream
        .set_read_timeout(Some(StdDuration::from_secs(2)))
        .map_err(|_| "bridge_auth_broker_unavailable".to_string())?;
    stream
        .set_write_timeout(Some(StdDuration::from_secs(2)))
        .map_err(|_| "bridge_auth_broker_unavailable".to_string())?;
    trusted_iterate_peer(stream.as_raw_fd())?;

    let request = AuthBrokerRequest {
        version: AUTH_BROKER_PROTOCOL_VERSION,
        audience,
        method,
        path,
        context: context.map(ToOwned::to_owned),
    };
    let mut encoded = serde_json::to_vec(&request)
        .map_err(|_| "bridge_auth_broker_request_invalid".to_string())?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .map_err(|_| "bridge_auth_broker_unavailable".to_string())?;
    stream
        .shutdown(std::net::Shutdown::Write)
        .map_err(|_| "bridge_auth_broker_unavailable".to_string())?;

    let mut response_bytes = Vec::new();
    stream
        .take(AUTH_BROKER_MAX_MESSAGE_BYTES + 1)
        .read_to_end(&mut response_bytes)
        .map_err(|_| "bridge_auth_broker_unavailable".to_string())?;
    if response_bytes.len() as u64 > AUTH_BROKER_MAX_MESSAGE_BYTES {
        return Err("bridge_auth_broker_response_too_large".to_string());
    }
    let response = serde_json::from_slice::<AuthBrokerResponse>(&response_bytes)
        .map_err(|_| "bridge_auth_broker_response_invalid".to_string())?;
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "bridge_auth_broker_denied".to_string()));
    }
    response
        .token
        .filter(|token| !token.is_empty() && token.len() <= MAX_TOKEN_BYTES)
        .ok_or_else(|| "bridge_auth_broker_response_invalid".to_string())
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
fn request_token_from_broker(
    _audience: BridgeTokenAudience,
    _method: &str,
    _path: &str,
    _context: Option<&str>,
) -> Result<String, String> {
    Err("bridge_auth_broker_requires_macos_code_identity".to_string())
}

#[cfg(target_os = "macos")]
async fn handle_auth_broker_client(mut stream: tokio::net::UnixStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    if trusted_iterate_peer(stream.as_raw_fd()).is_err() {
        return;
    }
    let mut request_bytes = Vec::new();
    let read_result = tokio::time::timeout(StdDuration::from_secs(2), async {
        (&mut stream)
            .take(AUTH_BROKER_MAX_MESSAGE_BYTES + 1)
            .read_to_end(&mut request_bytes)
            .await
    })
    .await;
    let response = match read_result {
        Ok(Ok(_)) if request_bytes.len() as u64 <= AUTH_BROKER_MAX_MESSAGE_BYTES => {
            match serde_json::from_slice::<AuthBrokerRequest>(&request_bytes) {
                Ok(request) if request.version == AUTH_BROKER_PROTOCOL_VERSION => {
                    match sign_bridge_token_at(
                        match server_master_key() {
                            Ok(key) => key,
                            Err(error) => {
                                let response = AuthBrokerResponse {
                                    ok: false,
                                    token: None,
                                    error: Some(error),
                                };
                                let _ = stream
                                    .write_all(&serde_json::to_vec(&response).unwrap_or_default())
                                    .await;
                                return;
                            }
                        },
                        request.audience,
                        &request.method,
                        &request.path,
                        request.context.as_deref(),
                        Utc::now(),
                    ) {
                        Ok(token) => AuthBrokerResponse {
                            ok: true,
                            token: Some(token),
                            error: None,
                        },
                        Err(error) => AuthBrokerResponse {
                            ok: false,
                            token: None,
                            error: Some(error),
                        },
                    }
                }
                _ => AuthBrokerResponse {
                    ok: false,
                    token: None,
                    error: Some("bridge_auth_broker_request_invalid".to_string()),
                },
            }
        }
        _ => AuthBrokerResponse {
            ok: false,
            token: None,
            error: Some("bridge_auth_broker_request_invalid".to_string()),
        },
    };
    let _ = stream
        .write_all(&serde_json::to_vec(&response).unwrap_or_default())
        .await;
}

#[cfg(target_os = "macos")]
pub(crate) async fn start_internal_auth_broker() -> Result<InternalAuthBroker, String> {
    start_internal_auth_broker_at(&auth_broker_socket_path()).await
}

#[cfg(target_os = "macos")]
async fn start_internal_auth_broker_at(socket_path: &Path) -> Result<InternalAuthBroker, String> {
    initialize_server_master_key()?;
    let directory = socket_path
        .parent()
        .ok_or_else(|| "Bridge 鉴权代理 socket 目录无效".to_string())?;
    ensure_private_directory(&directory)?;
    // Delete the retired same-UID-readable prototype key. It is never used by
    // this implementation; all signing material now lives only in the Bridge
    // server process.
    let _ = fs::remove_file(directory.join("bridge-auth-master.key"));

    let lock_path = directory.join(AUTH_BROKER_LOCK_FILE);
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .open(&lock_path)
        .map_err(|error| format!("打开 Bridge 鉴权代理锁失败: {error}"))?;
    set_private_permissions(&lock_path)?;
    if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("bridge_auth_broker_already_running".to_string());
    }

    match fs::symlink_metadata(socket_path) {
        Ok(metadata) if metadata.file_type().is_socket() => {
            let stale_device = metadata.dev();
            let stale_inode = metadata.ino();
            if StdUnixStream::connect(socket_path).is_ok() {
                return Err("bridge_auth_broker_already_running".to_string());
            }
            let current_metadata = fs::symlink_metadata(socket_path)
                .map_err(|error| format!("重新检查 Bridge 鉴权代理 socket 失败: {error}"))?;
            if !current_metadata.file_type().is_socket()
                || current_metadata.dev() != stale_device
                || current_metadata.ino() != stale_inode
            {
                return Err("bridge_auth_broker_path_changed".to_string());
            }
            fs::remove_file(socket_path)
                .map_err(|error| format!("清理 Bridge 鉴权代理 socket 失败: {error}"))?;
        }
        Ok(_) => return Err("bridge_auth_broker_path_occupied".to_string()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("检查 Bridge 鉴权代理 socket 失败: {error}")),
    }
    let listener = tokio::net::UnixListener::bind(socket_path)
        .map_err(|error| format!("启动 Bridge 鉴权代理失败: {error}"))?;
    let (device, inode) = socket_identity_from_fd(listener.as_raw_fd())?;
    let socket_guard = AuthBrokerSocketGuard {
        socket_path: socket_path.to_path_buf(),
        device,
        inode,
        lock_file,
    };
    fs::set_permissions(socket_path, fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("设置 Bridge 鉴权代理权限失败: {error}"))?;
    let task = tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, _)) => {
                    tokio::spawn(handle_auth_broker_client(stream));
                }
                Err(error) => {
                    log::error!("[Bridge] 鉴权代理 accept 失败: {error}");
                    break;
                }
            }
        }
    });
    Ok(InternalAuthBroker {
        task,
        _socket_guard: socket_guard,
    })
}

#[cfg(target_os = "windows")]
pub(crate) async fn start_internal_auth_broker() -> Result<InternalAuthBroker, String> {
    windows_broker::start().await
}

#[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
pub(crate) async fn start_internal_auth_broker() -> Result<InternalAuthBroker, String> {
    // The signed-code broker is a macOS hardening boundary. Keep other
    // platforms on the 0.5.8 transport behavior until they have an equivalent
    // OS-authenticated IPC implementation instead of making Bridge startup
    // fail everywhere outside macOS.
    initialize_server_master_key()?;
    Ok(InternalAuthBroker {
        task: tokio::spawn(async {}),
    })
}

fn normalize_method_and_path(method: &str, path: &str) -> Result<(String, String), String> {
    let method = method.trim().to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PATCH" | "PUT" | "DELETE") {
        return Err("invalid_internal_bridge_auth".to_string());
    }
    let path = path.trim();
    if path.is_empty()
        || path.len() > 512
        || !path.starts_with('/')
        || path.contains('?')
        || path.contains('#')
        || path.contains("//")
        || path
            .split('/')
            .any(|segment| segment == "." || segment == "..")
    {
        return Err("invalid_internal_bridge_auth".to_string());
    }
    Ok((method, path.to_string()))
}

fn desktop_route_allowed(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("GET", "/api/connection-status")
            | ("GET", "/api/android/pairing")
            | ("GET", "/api/android/pairing/status")
            | ("GET", "/api/mobile/paired-device-file-roots")
            | ("POST", "/api/mobile/paired-device-file-roots")
            | ("GET", "/api/config")
            | ("POST", "/api/config")
            | ("GET", "/api/phone-action-result")
            | ("POST", "/bridge/pull_action")
            | ("GET", "/api/desktop-codex-live")
            | ("POST", "/api/desktop-codex-live")
            | ("POST", "/api/desktop-codex-live/lease")
            | ("POST", "/api/desktop-codex-live/status")
            | ("GET", "/ws/codex-live")
    ) || (method == "GET" && path_has_one_nonempty_child(path, "/api/android/pairing/sessions/"))
}

fn internal_route_allowed(method: &str, path: &str) -> bool {
    matches!(
        (method, path),
        ("POST", "/bridge/publish")
            | ("GET", "/ws")
            | ("POST", "/bridge/pull_action")
            | ("POST", "/api/room-submit")
            | ("POST", "/api/cleanup-session")
            | ("POST", "/api/phone-action")
            | ("GET", "/api/phone-action-result")
            | ("GET", "/api/connection-status")
            | ("GET", "/api/active-sessions")
            | ("GET", "/api/android/pairing/status")
            | ("POST", "/api/apns/notify")
            | ("POST", "/api/recover-tailscale-funnel")
            | ("POST", "/api/restart-service")
            | ("POST", "/api/restart-tunnel")
            | ("GET", "/api/quick-tunnel/status")
            | ("POST", "/api/quick-tunnel/start")
            | ("POST", "/api/quick-tunnel/stop")
            | ("GET", "/api/prevent-sleep")
            | ("POST", "/api/prevent-sleep")
    ) || (method == "GET"
        && (path_has_one_nonempty_child(path, "/api/android/pairing/sessions/")
            || path_has_one_nonempty_child(path, "/api/phone-action-jobs/")))
}

fn path_has_one_nonempty_child(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .is_some_and(|child| !child.is_empty() && !child.contains('/'))
}

fn audience_allows_route(audience: BridgeTokenAudience, method: &str, path: &str) -> bool {
    match audience {
        BridgeTokenAudience::DesktopRenderer => desktop_route_allowed(method, path),
        BridgeTokenAudience::InternalProcess => internal_route_allowed(method, path),
    }
}

fn sign_bridge_token_at(
    key: &[u8],
    audience: BridgeTokenAudience,
    method: &str,
    path: &str,
    context: Option<&str>,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let (method, path) = normalize_method_and_path(method, path)?;
    if !audience_allows_route(audience, &method, &path) {
        return Err("internal_bridge_route_not_allowed".to_string());
    }
    if context.is_some()
        && (audience != BridgeTokenAudience::InternalProcess
            || method != "POST"
            || path != "/api/room-submit")
    {
        return Err("internal_bridge_context_not_allowed".to_string());
    }
    let context = match context {
        Some(value) => {
            let value = value.trim().to_ascii_lowercase();
            if value.len() > 256 || !valid_sha256_hex(&value) {
                return Err("invalid_internal_bridge_context".to_string());
            }
            Some(value)
        }
        None => None,
    };
    let ttl = match audience {
        BridgeTokenAudience::DesktopRenderer => DESKTOP_TOKEN_TTL_SECONDS,
        BridgeTokenAudience::InternalProcess => INTERNAL_TOKEN_TTL_SECONDS,
    };
    let claims = BridgeTokenClaims {
        version: 1,
        audience,
        method,
        path,
        issued_at: now.timestamp(),
        expires_at: (now + Duration::seconds(ttl)).timestamp(),
        nonce: random_url_safe_token("nonce")?,
        context,
    };
    let payload = URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&claims)
            .map_err(|error| format!("序列化 Bridge 内部凭据失败: {error}"))?,
    );
    let unsigned = format!("{TOKEN_PREFIX}.{payload}");
    let signature = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), unsigned.as_bytes());
    Ok(format!(
        "{unsigned}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    ))
}

fn verify_bridge_token_at(
    key: &[u8],
    token: &str,
    expected_method: &str,
    expected_path: &str,
    expected_context: Option<&str>,
    now: DateTime<Utc>,
) -> Result<BridgeTokenAudience, String> {
    if token.len() > MAX_TOKEN_BYTES {
        return Err("invalid_internal_bridge_auth".to_string());
    }
    let mut parts = token.split('.');
    let (Some(prefix), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err("invalid_internal_bridge_auth".to_string());
    };
    if prefix != TOKEN_PREFIX || payload.is_empty() || signature.is_empty() {
        return Err("invalid_internal_bridge_auth".to_string());
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "invalid_internal_bridge_auth".to_string())?;
    let unsigned = format!("{prefix}.{payload}");
    hmac::verify(
        &hmac::Key::new(hmac::HMAC_SHA256, key),
        unsigned.as_bytes(),
        &signature,
    )
    .map_err(|_| "invalid_internal_bridge_auth".to_string())?;
    let claims = serde_json::from_slice::<BridgeTokenClaims>(
        &URL_SAFE_NO_PAD
            .decode(payload)
            .map_err(|_| "invalid_internal_bridge_auth".to_string())?,
    )
    .map_err(|_| "invalid_internal_bridge_auth".to_string())?;
    let (method, path) = normalize_method_and_path(expected_method, expected_path)?;
    let ttl = match claims.audience {
        BridgeTokenAudience::DesktopRenderer => DESKTOP_TOKEN_TTL_SECONDS,
        BridgeTokenAudience::InternalProcess => INTERNAL_TOKEN_TTL_SECONDS,
    };
    let timestamp = now.timestamp();
    if claims.version != 1
        || claims.method != method
        || claims.path != path
        || !audience_allows_route(claims.audience, &method, &path)
        || claims.nonce.is_empty()
        || claims.nonce.len() > 256
        || claims.issued_at > timestamp + 30
        || claims.expires_at <= timestamp
        || claims.expires_at > claims.issued_at + ttl
        || claims.context.as_deref() != expected_context
    {
        return Err("invalid_internal_bridge_auth".to_string());
    }
    let mut used_nonces = USED_CAPABILITY_NONCES
        .lock()
        .map_err(|_| "invalid_internal_bridge_auth".to_string())?;
    used_nonces.retain(|_, expires_at| *expires_at > timestamp);
    if used_nonces.contains_key(&claims.nonce) {
        return Err("invalid_internal_bridge_auth".to_string());
    }
    used_nonces.insert(claims.nonce, claims.expires_at);
    Ok(claims.audience)
}

#[cfg(target_os = "windows")]
fn issue_windows_desktop_bridge_token(method: &str, path: &str) -> Result<String, String> {
    // A GUI that owns the Bridge already has its process-local key and must
    // retain the full, existing DesktopRenderer route allowlist. Only the
    // separate GUI process needs the narrower pairing-only pipe broker.
    match server_master_key() {
        Ok(key) => sign_bridge_token_at(
            key,
            BridgeTokenAudience::DesktopRenderer,
            method,
            path,
            None,
            Utc::now(),
        ),
        Err(_) => windows_broker::request_desktop_token(method, path),
    }
}

pub fn issue_desktop_bridge_token(method: &str, path: &str) -> Result<String, String> {
    #[cfg(test)]
    {
        return sign_bridge_token_at(
            &[9_u8; MASTER_KEY_BYTES],
            BridgeTokenAudience::DesktopRenderer,
            method,
            path,
            None,
            Utc::now(),
        );
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        request_token_from_broker(BridgeTokenAudience::DesktopRenderer, method, path, None)
    }
    #[cfg(all(not(test), target_os = "windows"))]
    {
        issue_windows_desktop_bridge_token(method, path)
    }
    #[cfg(all(not(test), not(target_os = "macos"), not(target_os = "windows")))]
    {
        sign_bridge_token_at(
            server_master_key()?,
            BridgeTokenAudience::DesktopRenderer,
            method,
            path,
            None,
            Utc::now(),
        )
    }
}

/// Native-only entry point used by the trusted Tauri renderer. Web companions
/// served by the Bridge do not have access to Tauri IPC and therefore cannot
/// mint this local capability.
#[tauri::command]
pub fn get_bridge_desktop_token(method: String, path: String) -> Result<String, String> {
    issue_desktop_bridge_token(&method, &path)
}

pub(crate) fn issue_internal_bridge_token(method: &str, path: &str) -> Result<String, String> {
    #[cfg(test)]
    {
        return sign_bridge_token_at(
            &[9_u8; MASTER_KEY_BYTES],
            BridgeTokenAudience::InternalProcess,
            method,
            path,
            None,
            Utc::now(),
        );
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        request_token_from_broker(BridgeTokenAudience::InternalProcess, method, path, None)
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        sign_bridge_token_at(
            server_master_key()?,
            BridgeTokenAudience::InternalProcess,
            method,
            path,
            None,
            Utc::now(),
        )
    }
}

fn valid_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub(crate) fn bridge_body_sha256(body: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, body);
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Mint the only body-bound capability exposed to an external room worker.
/// The broker still authenticates the signed iterate helper, while the body
/// digest prevents the one-shot bearer from being replayed for another room,
/// request, workspace, target, or message payload.
pub(crate) fn issue_internal_room_submit_token(body_sha256: &str) -> Result<String, String> {
    let body_sha256 = body_sha256.trim().to_ascii_lowercase();
    if !valid_sha256_hex(&body_sha256) {
        return Err("invalid_room_submit_body_digest".to_string());
    }
    #[cfg(test)]
    {
        return sign_bridge_token_at(
            &[9_u8; MASTER_KEY_BYTES],
            BridgeTokenAudience::InternalProcess,
            "POST",
            "/api/room-submit",
            Some(&body_sha256),
            Utc::now(),
        );
    }
    #[cfg(all(not(test), target_os = "macos"))]
    {
        request_token_from_broker(
            BridgeTokenAudience::InternalProcess,
            "POST",
            "/api/room-submit",
            Some(&body_sha256),
        )
    }
    #[cfg(all(not(test), not(target_os = "macos")))]
    {
        sign_bridge_token_at(
            server_master_key()?,
            BridgeTokenAudience::InternalProcess,
            "POST",
            "/api/room-submit",
            Some(&body_sha256),
            Utc::now(),
        )
    }
}

pub(crate) fn authorize_internal_bridge_request(
    request: reqwest::RequestBuilder,
    method: &str,
    url: &str,
) -> Result<reqwest::RequestBuilder, String> {
    let parsed =
        reqwest::Url::parse(url).map_err(|_| "internal_bridge_url_not_allowed".to_string())?;
    let host_is_loopback = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    let port_allowed =
        parsed.port_or_known_default() == Some(8080) || (cfg!(test) && parsed.port().is_some());
    if parsed.scheme() != "http"
        || !host_is_loopback
        || !port_allowed
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("internal_bridge_url_not_allowed".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        let token = issue_internal_bridge_token(method, parsed.path())?;
        Ok(request.bearer_auth(token))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = method;
        Ok(request)
    }
}

pub(crate) fn issue_internal_bridge_websocket_token(url: &str) -> Result<String, String> {
    let parsed =
        reqwest::Url::parse(url).map_err(|_| "internal_bridge_url_not_allowed".to_string())?;
    let host_is_loopback = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|address| address.is_loopback())
    });
    if parsed.scheme() != "ws"
        || !host_is_loopback
        || parsed.port_or_known_default() != Some(8080)
        || parsed.path() != "/ws"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        return Err("internal_bridge_url_not_allowed".to_string());
    }
    issue_internal_bridge_token("GET", "/ws")
}

fn bearer_token_from_headers(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?.trim();
    let mut parts = value.splitn(2, char::is_whitespace);
    let scheme = parts.next()?;
    let token = parts.next()?.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then(|| token.to_string())
}

pub fn has_internal_bridge_bearer(headers: &HeaderMap) -> bool {
    bearer_token_from_headers(headers)
        .is_some_and(|token| token.starts_with(&format!("{TOKEN_PREFIX}.")))
}

/// Returns `Ok(None)` when the request has no internal bearer, and rejects an
/// internal-looking bearer that fails verification.  A device credential is
/// intentionally not handled here; the paired-device store verifies it.
pub fn authenticate_internal_bridge_bearer(
    headers: &HeaderMap,
    method: &str,
    path: &str,
) -> Result<Option<BridgeTokenAudience>, String> {
    let Some(token) = bearer_token_from_headers(headers) else {
        return Ok(None);
    };
    if !token.starts_with(&format!("{TOKEN_PREFIX}.")) {
        return Ok(None);
    }
    authenticate_internal_bridge_token(&token, method, path).map(Some)
}

pub(crate) fn authenticate_internal_bridge_token(
    token: &str,
    method: &str,
    path: &str,
) -> Result<BridgeTokenAudience, String> {
    if !token.starts_with(&format!("{TOKEN_PREFIX}.")) {
        return Err("invalid_internal_bridge_auth".to_string());
    }
    #[cfg(test)]
    let key: &[u8] = &[9_u8; MASTER_KEY_BYTES];
    #[cfg(not(test))]
    let key = server_master_key()?;
    verify_bridge_token_at(key, token, method, path, None, Utc::now())
}

pub(crate) fn authenticate_internal_room_submit_bearer(
    headers: &HeaderMap,
    body_sha256: &str,
) -> Result<Option<BridgeTokenAudience>, String> {
    let Some(token) = bearer_token_from_headers(headers) else {
        return Ok(None);
    };
    if !token.starts_with(&format!("{TOKEN_PREFIX}.")) {
        return Ok(None);
    }
    let body_sha256 = body_sha256.trim().to_ascii_lowercase();
    if !valid_sha256_hex(&body_sha256) {
        return Err("invalid_room_submit_body_digest".to_string());
    }
    #[cfg(test)]
    let key: &[u8] = &[9_u8; MASTER_KEY_BYTES];
    #[cfg(not(test))]
    let key = server_master_key()?;
    verify_bridge_token_at(
        key,
        &token,
        "POST",
        "/api/room-submit",
        Some(&body_sha256),
        Utc::now(),
    )
    .map(Some)
}

pub fn cookie_token_from_headers(headers: &HeaderMap) -> Option<String> {
    let cookies = headers.get(header::COOKIE)?.to_str().ok()?;
    cookies.split(';').find_map(|part| {
        let (name, value) = part.trim().split_once('=')?;
        (name == AUTH_COOKIE_NAME && !value.trim().is_empty()).then(|| value.trim().to_string())
    })
}

pub fn build_auth_cookie(token: &str, secure: bool, max_age_seconds: i64) -> String {
    let attributes = if secure {
        "; HttpOnly; SameSite=None; Secure"
    } else {
        "; HttpOnly; SameSite=Strict"
    };
    format!("{AUTH_COOKIE_NAME}={token}; Path=/; Max-Age={max_age_seconds}{attributes}",)
}

pub fn clear_auth_cookie(secure: bool) -> String {
    let attributes = if secure {
        "; HttpOnly; SameSite=None; Secure"
    } else {
        "; HttpOnly; SameSite=Strict"
    };
    format!(
        "{AUTH_COOKIE_NAME}=; Path=/; Max-Age=0; Expires=Thu, 01 Jan 1970 00:00:00 GMT{attributes}",
    )
}

pub fn should_use_secure_cookie(headers: &HeaderMap) -> bool {
    headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("https"))
        })
        .unwrap_or(false)
        || headers
            .get("forwarded")
            .and_then(|value| value.to_str().ok())
            .map(|value| value.to_ascii_lowercase().contains("proto=https"))
            .unwrap_or(false)
}

#[cfg(test)]
pub(crate) fn issue_bridge_token_for_test(
    key: &[u8],
    audience: BridgeTokenAudience,
    method: &str,
    path: &str,
    now: DateTime<Utc>,
) -> Result<String, String> {
    sign_bridge_token_at(key, audience, method, path, None, now)
}

#[cfg(test)]
pub(crate) fn verify_bridge_token_for_test(
    key: &[u8],
    token: &str,
    method: &str,
    path: &str,
    now: DateTime<Utc>,
) -> Result<BridgeTokenAudience, String> {
    verify_bridge_token_at(key, token, method, path, None, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    fn socket_identity_for_test(path: &Path) -> Option<(u64, u64)> {
        let metadata = fs::symlink_metadata(path).ok()?;
        metadata
            .file_type()
            .is_socket()
            .then(|| (metadata.dev(), metadata.ino()))
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "iterate-bridge-auth-{label}-{}",
                random_url_safe_token("test").expect("random test token")
            ));
            fs::create_dir_all(&path).expect("create test directory");
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn test_key() -> [u8; MASTER_KEY_BYTES] {
        [7_u8; MASTER_KEY_BYTES]
    }

    #[test]
    fn request_capability_is_audience_method_and_path_bound() {
        // Keep this nonce inside the same expiry window as concurrently running
        // auth tests. A historical timestamp can otherwise be pruned between the
        // two verifications by another test using the process-wide nonce cache.
        let now = Utc::now();
        let token = sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "POST",
            "/bridge/pull_action",
            None,
            now,
        )
        .expect("issue desktop token");

        assert_eq!(
            verify_bridge_token_at(
                &test_key(),
                &token,
                "POST",
                "/bridge/pull_action",
                None,
                now,
            )
            .expect("verify exact token"),
            BridgeTokenAudience::DesktopRenderer
        );
        assert!(
            verify_bridge_token_at(
                &test_key(),
                &token,
                "POST",
                "/bridge/pull_action",
                None,
                now,
            )
            .is_err(),
            "capability nonce must be single-use"
        );
        assert!(verify_bridge_token_at(
            &test_key(),
            &token,
            "GET",
            "/bridge/pull_action",
            None,
            now,
        )
        .is_err());
        assert!(
            verify_bridge_token_at(&test_key(), &token, "GET", "/api/config", None, now,).is_err()
        );
    }

    #[test]
    fn quick_tunnel_control_routes_are_internal_process_only() {
        for (method, path) in [
            ("GET", "/api/quick-tunnel/status"),
            ("POST", "/api/quick-tunnel/start"),
            ("POST", "/api/quick-tunnel/stop"),
        ] {
            assert!(internal_route_allowed(method, path));
            assert!(!desktop_route_allowed(method, path));
        }
        assert!(!internal_route_allowed("GET", "/api/quick-tunnel/start"));
        assert!(!internal_route_allowed("POST", "/api/quick-tunnel/status"));
    }

    #[test]
    fn desktop_token_cannot_be_minted_for_unapproved_routes() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).expect("fixed time");
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "POST",
            "/api/restart-service",
            None,
            now,
        )
        .is_err());
    }

    #[test]
    fn desktop_renderer_can_mint_only_the_live_websocket_get_capability() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).expect("fixed time");
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "GET",
            "/ws/codex-live",
            None,
            now,
        )
        .is_ok());
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "POST",
            "/ws/codex-live",
            None,
            now,
        )
        .is_err());
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "GET",
            "/ws",
            None,
            now,
        )
        .is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_bridge_owner_keeps_existing_desktop_routes() {
        initialize_server_master_key().expect("Bridge owner initializes its process key");
        for (method, path) in [
            ("GET", "/ws/codex-live"),
            ("POST", "/api/desktop-codex-live/lease"),
            ("GET", "/api/android/pairing"),
        ] {
            let token = issue_windows_desktop_bridge_token(method, path)
                .expect("Bridge owner signs its allowed desktop route locally");
            assert_eq!(
                verify_bridge_token_at(
                    server_master_key().expect("process key"),
                    &token,
                    method,
                    path,
                    None,
                    Utc::now(),
                )
                .expect("local token verifies"),
                BridgeTokenAudience::DesktopRenderer,
            );
        }
        assert!(issue_windows_desktop_bridge_token("POST", "/api/restart-service").is_err());
    }

    #[test]
    fn desktop_live_control_capabilities_are_desktop_only_and_method_bound() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).expect("fixed time");
        for (method, path) in [
            ("GET", "/api/desktop-codex-live"),
            ("POST", "/api/desktop-codex-live"),
            ("POST", "/api/desktop-codex-live/lease"),
            ("POST", "/api/desktop-codex-live/status"),
        ] {
            assert!(sign_bridge_token_at(
                &test_key(),
                BridgeTokenAudience::DesktopRenderer,
                method,
                path,
                None,
                now,
            )
            .is_ok());
            assert!(sign_bridge_token_at(
                &test_key(),
                BridgeTokenAudience::InternalProcess,
                method,
                path,
                None,
                now,
            )
            .is_err());
        }
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "GET",
            "/api/desktop-codex-live/status",
            None,
            now,
        )
        .is_err());
    }

    #[test]
    fn paired_device_file_root_management_is_desktop_only_and_method_bound() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).expect("fixed time");
        for method in ["GET", "POST"] {
            assert!(sign_bridge_token_at(
                &test_key(),
                BridgeTokenAudience::DesktopRenderer,
                method,
                "/api/mobile/paired-device-file-roots",
                None,
                now,
            )
            .is_ok());
            assert!(sign_bridge_token_at(
                &test_key(),
                BridgeTokenAudience::InternalProcess,
                method,
                "/api/mobile/paired-device-file-roots",
                None,
                now,
            )
            .is_err());
        }
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "DELETE",
            "/api/mobile/paired-device-file-roots",
            None,
            now,
        )
        .is_err());
    }

    #[test]
    fn internal_credentials_are_never_attached_to_remote_urls() {
        let client = reqwest::Client::new();
        assert!(authorize_internal_bridge_request(
            client.get("http://example.com:8080/api/connection-status"),
            "GET",
            "http://example.com:8080/api/connection-status",
        )
        .is_err());
        assert!(authorize_internal_bridge_request(
            client.get("https://127.0.0.1:8080/api/connection-status"),
            "GET",
            "https://127.0.0.1:8080/api/connection-status",
        )
        .is_err());
        assert!(issue_internal_bridge_websocket_token("wss://example.com:8080/ws").is_err());
    }

    #[test]
    fn internal_websocket_capability_survives_the_relay_to_ws_auth_path_once() {
        let token = issue_internal_bridge_websocket_token("ws://127.0.0.1:8080/ws")
            .expect("mint loopback websocket capability");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .expect("valid bearer header"),
        );

        assert_eq!(
            authenticate_internal_bridge_bearer(&headers, "GET", "/ws")
                .expect("authenticate exact websocket route"),
            Some(BridgeTokenAudience::InternalProcess),
        );
        assert!(
            authenticate_internal_bridge_bearer(&headers, "GET", "/ws").is_err(),
            "websocket capability must remain single-use"
        );
    }

    #[test]
    fn non_internal_bearer_remains_unhandled_by_internal_auth() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer external-api-token"
                .parse()
                .expect("valid bearer header"),
        );

        assert_eq!(
            authenticate_internal_bridge_bearer(&headers, "GET", "/ws")
                .expect("non-internal bearer must remain outside internal auth"),
            None,
        );
    }

    #[test]
    fn room_submit_capability_is_one_shot_and_exact_body_bound() {
        let body = br#"{"message_type":"mcp_action","payload":{"room_id":"room-a"}}"#;
        let body_digest = bridge_body_sha256(body);
        let token = issue_internal_room_submit_token(&body_digest)
            .expect("mint body-bound room capability");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .expect("valid bearer header"),
        );

        let other_digest = bridge_body_sha256(b"different body");
        assert!(authenticate_internal_room_submit_bearer(&headers, &other_digest).is_err());
        assert_eq!(
            authenticate_internal_room_submit_bearer(&headers, &body_digest)
                .expect("authenticate exact room body"),
            Some(BridgeTokenAudience::InternalProcess),
        );
        assert!(authenticate_internal_room_submit_bearer(&headers, &body_digest).is_err());
    }

    #[test]
    fn browser_cookie_attributes_match_transport_and_support_revocation() {
        let local = build_auth_cookie("token", false, 120);
        assert!(local.contains("HttpOnly"));
        assert!(local.contains("SameSite=Strict"));
        assert!(!local.contains("; Secure"));

        let tunneled = build_auth_cookie("token", true, 120);
        assert!(tunneled.contains("SameSite=None"));
        assert!(tunneled.contains("; Secure"));

        let cleared = clear_auth_cookie(true);
        assert!(cleared.contains("Max-Age=0"));
        assert!(cleared.contains("Expires=Thu, 01 Jan 1970"));
        assert!(cleared.contains("SameSite=None"));
        assert!(cleared.contains("; Secure"));
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn broker_round_trip_authenticates_peer_before_minting() {
        let production_socket = auth_broker_socket_path();
        let production_socket_identity_before = socket_identity_for_test(&production_socket);
        let temp = tempfile::tempdir().expect("temporary auth broker directory");
        let socket_path = temp.path().join(AUTH_BROKER_SOCKET_FILE);
        let broker = start_internal_auth_broker_at(&socket_path)
            .await
            .expect("start auth broker");
        let token = tokio::task::spawn_blocking(move || {
            request_token_from_broker_at(
                &socket_path,
                BridgeTokenAudience::InternalProcess,
                "GET",
                "/api/connection-status",
                None,
            )
        })
        .await
        .expect("broker client task")
        .expect("broker mints capability for trusted peer");
        assert_eq!(
            verify_bridge_token_at(
                server_master_key().expect("server master key"),
                &token,
                "GET",
                "/api/connection-status",
                None,
                Utc::now(),
            )
            .expect("verify broker capability"),
            BridgeTokenAudience::InternalProcess,
        );
        drop(broker);
        let production_socket_identity_after = socket_identity_for_test(&production_socket);
        assert_eq!(
            production_socket_identity_after, production_socket_identity_before,
            "a broker test must not replace or unlink the production socket"
        );
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn broker_refuses_to_replace_a_live_socket() {
        let temp = tempfile::tempdir().expect("temporary auth broker directory");
        let socket_path = temp.path().join(AUTH_BROKER_SOCKET_FILE);
        let broker = start_internal_auth_broker_at(&socket_path)
            .await
            .expect("start first auth broker");

        let error = match start_internal_auth_broker_at(&socket_path).await {
            Ok(_) => panic!("a live broker must retain its socket"),
            Err(error) => error,
        };
        assert_eq!(error, "bridge_auth_broker_already_running");
        assert!(StdUnixStream::connect(&socket_path).is_ok());
        drop(broker);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn broker_guard_preserves_a_replacement_socket() {
        let temp = tempfile::tempdir().expect("temporary auth broker directory");
        let socket_path = temp.path().join(AUTH_BROKER_SOCKET_FILE);
        let broker = start_internal_auth_broker_at(&socket_path)
            .await
            .expect("start auth broker");
        broker.abort();
        fs::remove_file(&socket_path).expect("unlink original broker socket");

        let replacement =
            tokio::net::UnixListener::bind(&socket_path).expect("bind replacement socket");
        let replacement_identity = socket_identity_for_test(&socket_path);
        drop(broker);

        assert_eq!(socket_identity_for_test(&socket_path), replacement_identity);
        drop(replacement);
        fs::remove_file(&socket_path).expect("remove replacement socket");
    }

    #[test]
    fn dynamic_route_allowlist_accepts_exactly_one_path_segment() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).expect("fixed time");
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "GET",
            "/api/android/pairing/sessions/session-1",
            None,
            now,
        )
        .is_ok());
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "GET",
            "/api/android/pairing/sessions/session-1/escape",
            None,
            now,
        )
        .is_err());
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::DesktopRenderer,
            "GET",
            "/api/mobile/pairing/sessions/session-1",
            None,
            now,
        )
        .is_err());
        assert!(sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::InternalProcess,
            "GET",
            "/api/phone-action-jobs/job-1",
            None,
            now,
        )
        .is_ok());
    }

    #[test]
    fn token_expiry_and_tampering_fail_closed() {
        let now = DateTime::from_timestamp(1_700_000_000, 0).expect("fixed time");
        let token = sign_bridge_token_at(
            &test_key(),
            BridgeTokenAudience::InternalProcess,
            "POST",
            "/bridge/publish",
            None,
            now,
        )
        .expect("issue internal token");
        let mut tampered = token.clone();
        tampered.push('x');
        assert!(verify_bridge_token_at(
            &test_key(),
            &tampered,
            "POST",
            "/bridge/publish",
            None,
            now,
        )
        .is_err());
        assert!(verify_bridge_token_at(
            &test_key(),
            &token,
            "POST",
            "/bridge/publish",
            None,
            now + Duration::seconds(INTERNAL_TOKEN_TTL_SECONDS + 1)
        )
        .is_err());
    }

    #[test]
    fn enforcement_state_is_private_atomic_and_corruption_fails_closed() {
        let directory = TestDirectory::new("state");
        let path = directory.path.join("bridge-auth-state.json");
        save_enforcement_state_to_path(&path, true).expect("save state");
        assert!(load_enforcement_state_from_path(&path));
        fs::write(&path, b"not-json").expect("corrupt state");
        assert!(load_enforcement_state_from_path(&path));
        assert!(load_enforcement_state_from_path(
            &directory.path.join("missing.json")
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path)
                    .expect("state metadata")
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }
}
