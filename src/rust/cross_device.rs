//! Paired desktop transport. TLS authenticates each configured device;
//! the peer protocol cannot choose local filenames, executables or arguments.
use axum::{
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};

const VERSION: u32 = 2;
const WINDOW_SYNC_CAPABILITY: &str = "window_sync_v1";

fn supports_window_sync(value: &Value) -> bool {
    value.get("capabilities").and_then(Value::as_array)
        .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(WINDOW_SYNC_CAPABILITY)))
}
type Result<T> = std::result::Result<T, String>;

fn directory() -> Result<PathBuf> {
    match std::env::var_os("ITERATE_CROSS_DEVICE_DIR") {
        Some(path) => Ok(PathBuf::from(path)),
        None => crate::config::cunzhi_config_dir()
            .map(|path| path.join("cross-device"))
            .map_err(|error| error.to_string()),
    }
}

pub mod transport;
pub mod hub_transport;
pub mod settings_sync;
use transport::{connection_config, credential, local_daemon_ready, peer};

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let parent = path.parent().ok_or("缺少目录")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    temp.write_all(&serde_json::to_vec(value).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}

fn lock(dir: &Path, name: &str) -> Result<File> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join(name))
        .map_err(|e| e.to_string())?;
    file.lock().map_err(|e| e.to_string())?;
    Ok(file)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

#[derive(Clone, Serialize, Deserialize)]
struct Settings {
    device_id: String,
    enabled: bool,
}

fn settings() -> Result<Settings> {
    let dir = directory()?;
    let _guard = lock(&dir, "settings.lock")?;
    let path = dir.join("settings.json");
    if path.exists() {
        return read_json(&path);
    }
    let value = Settings {
        device_id: uuid::Uuid::new_v4().to_string(),
        enabled: false,
    };
    atomic_json(&path, &value)?;
    Ok(value)
}

// Display-only computer identity. Reading it never creates settings, credentials,
// or a pairing, and it must never be replaced with the replying phone's identity.
pub(crate) fn local_computer_identity() -> (Option<String>, Option<String>) {
    let Ok(dir) = directory() else { return (None, None); };
    let id = read_json::<Settings>(&dir.join("settings.json")).ok()
        .and_then(|settings| display_identity(Some(&json!(settings.device_id))));
    let name = read_json::<transport::ConnectionConfig>(&dir.join("direct-connection.json")).ok()
        .and_then(|config| display_identity(Some(&json!(config.device_name))));
    (id, name)
}

fn display_identity(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::trim)
        .filter(|value| !value.is_empty()).map(str::to_owned)
}

fn display_platform(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::trim)
        .filter(|value| matches!(*value, "windows" | "macos")).map(str::to_owned)
}

fn deserialize_display_platform<'de, D: serde::Deserializer<'de>>(deserializer: D) -> std::result::Result<Option<String>, D::Error> {
    let value = Value::deserialize(deserializer)?;
    Ok(display_platform(Some(&value)))
}

pub(crate) fn local_computer_platform() -> Option<&'static str> {
    match std::env::consts::OS {
        "windows" => Some("windows"),
        "macos" => Some("macos"),
        _ => None,
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Registration {
    key: String,
    origin_device_id: String,
    origin_name: String,
    #[serde(default, deserialize_with = "deserialize_display_platform")]
    origin_platform: Option<String>,
    request: Value,
    request_file: PathBuf,
    response_file: PathBuf,
    #[serde(default = "published_by_default")]
    published: bool,
}

fn published_by_default() -> bool { true }

impl Registration {
    fn path(&self) -> Result<PathBuf> {
        Ok(directory()?
            .join(if self.published { "requests" } else { "deferred-requests" })
            .join(format!("{}.json", self.key)))
    }
    fn result_path(&self) -> Result<PathBuf> {
        Ok(directory()?
            .join("results")
            .join(format!("{}.json", self.key)))
    }
    fn request_lock(&self) -> Result<File> {
        lock(&directory()?.join("locks"), &format!("{}.lock", self.key))
    }
    fn active(&self) -> bool {
        self.pending() && self.result_path().is_ok_and(|p| !p.exists())
    }
    fn pending(&self) -> bool {
        let fresh = directory()
            .ok()
            .and_then(|d| fs::metadata(d.join("leases").join(&self.key)).ok())
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age < Duration::from_secs(15));
        fresh && self.request_file.is_file()
    }
    pub fn renew(&self) {
        if let Ok(dir) = directory() {
            let path = dir.join("leases").join(&self.key);
            let recent = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(1));
            if !recent {
                let _ = fs::create_dir_all(dir.join("leases"));
                let _ = fs::write(path, b"active");
            }
        }
    }
    pub fn key(&self) -> &str {
        &self.key
    }
    pub fn is_published(&self) -> bool {
        self.published
    }
    fn trusted_timeline_route(&self) -> Option<String> {
        (self.request.get("codex_thread_provenance").and_then(Value::as_str) == Some("caller_meta"))
            .then(|| self.request.get("codex_thread_id").and_then(Value::as_str))
            .flatten()
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
    }
    fn ordinary_timeline_route(&self) -> String {
        // This is a recording route only. A legacy/explicit thread may be used
        // by the ordinary MCP timeline, but it never grants Native history.
        ["timeline_route_id", "conversation_route_id", "codex_thread_id", "id"]
            .iter()
            .filter_map(|key| self.request.get(*key).and_then(Value::as_str))
            .map(str::trim)
            .find(|route| !route.is_empty()
                && !route.starts_with("native-cli:")
                && !route.starts_with("native-codex:"))
            .map(str::to_owned)
            .unwrap_or_else(|| self.key.clone())
    }
    fn response_event_id(&self) -> String {
        format!("cross-device-reply:{}", self.key)
    }
    pub fn recover_accepted_response(&self) {
        let Ok(_admission) = hub_transport::submission_guard(false) else { return; };
        // If the source window exits after the peer won, preserve that accepted
        // reply instead of turning it into a cancellation. The receipt remains.
        if let Ok(_guard) = self.request_lock() {
            if !self.response_file.exists() {
                if let Ok(path) = self.result_path() {
                    if let Ok(receipt) = read_json::<Value>(&path) {
                        if let Some(response) = receipt.get("response") {
                            let _ = atomic_json(&self.response_file, response);
                        }
                    } else {
                        // Closing acceptance and inspecting the winner are one
                        // transaction; a late peer cannot win after this point.
                        let _ = atomic_json(&path, &json!({"cancelled":true}));
                    }
                }
            }
        }
    }
    pub fn finish(&self) {
        let Ok(_admission) = hub_transport::submission_guard(false) else { return; };
        if let Ok(_guard) = self.request_lock() {
            if let Ok(path) = self.result_path() {
                if !path.exists() {
                    let _ = atomic_json(&path, &json!({"cancelled": true}));
                }
            }
        }
    }
}

fn has_attachments(value: &Value) -> bool {
    ["images", "image_paths", "file_paths", "attachments"]
        .iter()
        .any(|k| {
            value
                .get(k)
                .is_some_and(|v| v.as_array().is_some_and(|a| !a.is_empty()))
        })
}

pub async fn register(
    request: &Value,
    request_file: &Path,
    response_file: &Path,
) -> Option<Registration> {
    if let Ok(Some(config)) = hub_transport::config() {
        if std::env::var_os("ITERATE_CROSS_DEVICE_MIRROR").is_some() { return None; }
        let (_tracking, may_publish) = hub_transport::registration_guard().ok()?;
        let dir = directory().ok()?;
        let _route = lock(&dir, "connection.lock").ok()?;
        let current = hub_transport::config().ok()??;
        // Credential refreshes do not change a registration's source route.
        // Reject identity/endpoint changes, but retain the popup when only the
        // token environment or CA changed while it waited for this lock.
        if current.device_id != config.device_id || current.endpoint != config.endpoint {
            return None;
        }
        let local=settings().ok()?;
        let registration=Registration{key:uuid::Uuid::new_v4().to_string(),origin_device_id:config.device_id,
            origin_name:connection_config().ok().map(|c|c.device_name).unwrap_or_else(||std::env::consts::OS.into()),
            origin_platform:local_computer_platform().map(str::to_owned),request:request.clone(),
            request_file:request_file.into(),response_file:response_file.into(),published:may_publish&&local.enabled&&hub_transport::ready()};
        atomic_json(&registration.path().ok()?,&registration).ok()?;registration.renew();return Some(registration);
    }
    transport::validate_enable().ok()?;
    let route_key = transport::route_key().ok()?;
    let local = settings().ok()?;
    if std::env::var_os("ITERATE_CROSS_DEVICE_MIRROR").is_some() {
        return None;
    }
    let daemon_ready = transport::ensure_daemon().await.is_ok();
    if !text_mirror_supported(request) {
        return None;
    }
    let published = daemon_ready && local.enabled && peer("/snapshot", None).await.ok()
        .is_some_and(|snapshot| snapshot.get("enabled").and_then(Value::as_bool) == Some(true));
    // Keep paired requests registered while offline or disabled so an already
    // open popup can be synchronized after both devices enable the feature.
    let _route_guard = lock(&directory().ok()?, "connection.lock").ok()?;
    if transport::route_key().ok()? != route_key {
        return None;
    }
    let registration = Registration {
        key: uuid::Uuid::new_v4().to_string(),
        origin_device_id: local.device_id,
        origin_platform: local_computer_platform().map(str::to_owned),
        origin_name: connection_config()
            .ok()
            .map(|c| c.device_name)
            .unwrap_or_else(|| std::env::consts::OS.into()),
        request: request.clone(),
        request_file: request_file.into(),
        response_file: response_file.into(),
        published,
    };
    atomic_json(&registration.path().ok()?, &registration).ok()?;
    registration.renew();
    Some(registration)
}

fn load_registration(key: &str) -> Result<Registration> {
    uuid::Uuid::parse_str(key).map_err(|_| "无效请求标识")?;
    let dir = directory()?;
    let published = dir.join("requests").join(format!("{key}.json"));
    load_registration_files(&published, || read_json(&dir.join("deferred-requests").join(format!("{key}.json"))))
}

fn text_mirror_supported(request: &Value) -> bool {
    let message = request.get("message").and_then(Value::as_str).unwrap_or("");
    !has_attachments(request) && !message.contains("![") && !message.contains("<img")
}

pub async fn record_accepted_response(reg: &Registration, response: &Value) -> Result<()> {
    // The registration and its winner receipt, rather than response metadata
    // supplied by a remote client, own the route and immutable event identity.
    let accepted = read_json::<Value>(&reg.result_path()?)?;
    if !accepted.get("response").is_some_and(|winner| response_contains(winner, response)) {
        return Err("源端记录的回复与获胜回复不一致".into());
    }
    let mut recorded = response.clone();
    let Some(metadata) = recorded.get_mut("metadata").and_then(Value::as_object_mut) else {
        return Ok(());
    };
    metadata.insert("run_id".into(), json!(reg.response_event_id()));
    let project = reg.request.get("project_path").and_then(Value::as_str).map(str::to_owned);
    let request_id = reg.request.get("id").and_then(Value::as_str).map(str::to_owned);
    let manager = crate::conversation::ConversationManager::new_with_forced_persistence();
    crate::ui::commands::record_user_response_node(
        None, &manager, &recorded, project, request_id,
        Some(reg.ordinary_timeline_route()), "cross_device_source_reply",
    ).await.map(|_| ())
}

pub(crate) fn source_reply_identity(response: &Value) -> Option<(String, String)> {
    let key = std::env::var("ITERATE_CROSS_DEVICE_SOURCE").ok().filter(|key| !key.is_empty())?;
    let reg = load_registration(&key).ok()?;
    let receipt = read_json::<Value>(&reg.result_path().ok()?).ok()?;
    receipt.get("response")
        .is_some_and(|winner| response_contains(winner, response))
        .then(|| (reg.response_event_id(), reg.ordinary_timeline_route()))
}

fn load_registration_files(published: &Path, read_deferred: impl FnOnce() -> Result<Registration>) -> Result<Registration> {
    if published.exists() { read_json(&published) }
    // Publication writes the destination before removing the deferred file.
    // If that move raced our first lookup, the destination is now authoritative.
    else { read_deferred().or_else(|_| read_json(published)) }
}

// The same OS file lock is used by the local popup and peer HTTP handler.
// Atomic result persistence makes retry safe even when the ACK is lost.
fn commit(reg: &Registration, response: &Value, remote: bool) -> Result<()> {
    let finishing_winner=reg.result_path().ok().and_then(|p|read_json::<Value>(&p).ok()).is_some_and(|v|v.get("response")==Some(response));
    let _admission = hub_transport::submission_guard(finishing_winner)?;
    commit_inner(reg, response, remote)
}
fn commit_inner(reg: &Registration, response: &Value, remote: bool) -> Result<()> {
    let _guard = reg.request_lock()?;
    let result_path = reg.result_path()?;
    if result_path.exists() {
        let previous: Value = read_json(&result_path)?;
        if previous.get("response") == Some(response) {
            return Ok(());
        }
        return Err("该请求已由另一端完成或关闭".into());
    }
    if !reg.active() {
        return Err("源请求已结束".into());
    }
    if remote && has_attachments(response) {
        return Err("跨设备 spike 仅支持文本和选项，请在源端发送附件".into());
    }
    // Persist the winner before the source GUI performs its existing recording
    // and response-file publication. A killed GUI cannot lose this receipt.
    atomic_json(&result_path, &json!({"response": response}))?;
    Ok(())
}

// Android is paired to the local Bridge, while the source popup can live in a
// different process. Resolve only the original serve request, never a project
// fallback, and use the same receipt lock as desktop/peer submissions.
fn android_registration(request_id: &str, project_path: &str) -> Result<Option<Registration>> {
    let dir = directory()?;
    let mut mismatch = false;
    for folder in ["requests", "deferred-requests"] {
        let Ok(entries) = fs::read_dir(dir.join(folder)) else { continue };
        for entry in entries.flatten() {
            let Ok(reg) = read_json::<Registration>(&entry.path()) else { continue };
            if reg.request.get("id").and_then(Value::as_str) != Some(request_id) {
                continue;
            }
            let registered_project = reg.request.get("project_path").and_then(Value::as_str)
                .map(normalize_project_path);
            if registered_project.as_deref() == Some(project_path) {
                return Ok(Some(reg));
            }
            mismatch = true;
        }
    }
    if mismatch { Err("target_project_mismatch".into()) } else { Ok(None) }
}

pub(crate) fn registered_caller_thread(request_id: &str, project_path: &str) -> Option<String> {
    android_registration(request_id, &normalize_project_path(project_path))
        .ok()
        .flatten()
        .and_then(|reg| reg.trusted_timeline_route())
}

// Read-only restart recovery for Windows sources. Replies still use the existing
// Registration receipt lock and source-consumed acknowledgement, never a new route.
pub(crate) fn load_ready_local_registration_for_mobile(
    request_id: &str, project_path: &str, instances: &[crate::ui::window_registry::WindowInstance],
) -> std::result::Result<Value, &'static str> {
    #[cfg(target_os = "windows")]
    {
        // directory() may create the default config directory. Resolve the same
        // Windows locations without initializing any configuration or state.
        let dir = std::env::var_os("ITERATE_CROSS_DEVICE_DIR").map(PathBuf::from)
            .or_else(|| std::env::var_os("ITERATE_CONFIG_DIR").filter(|path| !path.is_empty())
                .map(|path| PathBuf::from(path).join("cross-device")))
            .or_else(|| dirs::config_dir().map(|path| path.join("cunzhi/cross-device")))
            .filter(|path| !path.as_os_str().is_empty()).ok_or("registration_directory_missing")?;
        load_ready_local_registration_from_dir(&dir, &std::env::temp_dir(), request_id, project_path, instances)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (request_id, project_path, instances);
        Err("registered_recovery_not_windows")
    }
}

#[cfg(target_os = "windows")]
fn read_registration_recovery_json(path: &Path) -> std::result::Result<Value, &'static str> {
    use std::io::Read;
    use std::os::windows::fs::OpenOptionsExt;
    const MAX_BYTES: u64 = 1024 * 1024;
    let metadata = fs::symlink_metadata(path).map_err(|_| "recovery_file_missing")?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_BYTES {
        return Err("recovery_file_invalid");
    }
    // Open the reparse point itself rather than following a swapped symlink.
    let file = OpenOptions::new().read(true).custom_flags(0x00200000).open(path)
        .map_err(|_| "recovery_file_invalid")?;
    let opened = file.metadata().map_err(|_| "recovery_file_invalid")?;
    if !opened.is_file() || opened.file_type().is_symlink() || opened.len() > MAX_BYTES {
        return Err("recovery_file_invalid");
    }
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| "recovery_file_invalid")?;
    if bytes.len() as u64 > MAX_BYTES { return Err("recovery_file_invalid"); }
    serde_json::from_slice(&bytes).map_err(|_| "recovery_file_invalid")
}

#[cfg(target_os = "windows")]
fn recovery_file_absent(path: &Path) -> bool {
    matches!(fs::symlink_metadata(path), Err(error) if error.kind() == std::io::ErrorKind::NotFound)
}

#[cfg(target_os = "windows")]
fn recovery_registration_pending(dir: &Path, temp_dir: &Path, reg: &Registration) -> bool {
    let fresh = fs::symlink_metadata(dir.join("leases").join(&reg.key)).ok()
        .filter(|metadata| metadata.is_file() && !metadata.file_type().is_symlink())
        .and_then(|metadata| metadata.modified().ok()).and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(15));
    fresh && recovery_file_absent(&dir.join("results").join(format!("{}.json", reg.key)))
        && recovery_file_absent(&temp_dir.join(format!("iterate_response_{}.json", reg.request["id"].as_str().unwrap_or(""))))
}

#[cfg(target_os = "windows")]
fn load_ready_local_registration_from_dir(
    dir: &Path, temp_dir: &Path, request_id: &str, project_path: &str,
    instances: &[crate::ui::window_registry::WindowInstance],
) -> std::result::Result<Value, &'static str> {
    use crate::bridge::ws::local_project_paths_match;
    let valid_id = request_id.strip_prefix("serve-").and_then(|suffix| suffix.split_once('-'))
        .is_some_and(|(timestamp, uuid)| timestamp.len() == 13
            && timestamp.bytes().all(|byte| byte.is_ascii_digit()) && uuid::Uuid::parse_str(uuid).is_ok());
    if !valid_id || !Path::new(project_path).is_absolute() { return Err("invalid_registered_route"); }
    if !instances.iter().any(|instance| instance.request_id.as_deref() == Some(request_id)
        && local_project_paths_match(&instance.project_path, project_path)) {
        return Err("no_live_registered_window");
    }
    let created_at_ms = request_id[6..19].parse::<i64>().map_err(|_| "invalid_registered_route")?;
    let request_file = temp_dir.join(format!("iterate_request_{request_id}.json"));
    let response_file = temp_dir.join(format!("iterate_response_{request_id}.json"));
    let ready_file = temp_dir.join(format!("iterate_ready_{request_id}.json"));
    let mut miss = "registration_missing";
    for folder in ["requests", "deferred-requests"] {
        let Ok(entries) = fs::read_dir(dir.join(folder)) else { continue; };
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(key) = path.file_stem().and_then(|name| name.to_str()) else { continue; };
            if path.extension().and_then(|name| name.to_str()) != Some("json") || uuid::Uuid::parse_str(key).is_err() { continue; }
            let Ok(value) = read_registration_recovery_json(&path) else { continue; };
            let Ok(reg) = serde_json::from_value::<Registration>(value) else { continue; };
            if reg.request.get("id").and_then(Value::as_str) != Some(request_id) { continue; }
            if reg.key != key { miss = "registration_key_mismatch"; continue; }
            if !reg.request.get("project_path").and_then(Value::as_str)
                .is_some_and(|path| local_project_paths_match(path, project_path))
                || !local_project_paths_match(&reg.request_file.to_string_lossy(), &request_file.to_string_lossy())
                || !local_project_paths_match(&reg.response_file.to_string_lossy(), &response_file.to_string_lossy()) {
                miss = "registered_project_or_file_mismatch"; continue;
            }
            if !recovery_registration_pending(dir, temp_dir, &reg) { miss = "registered_source_not_pending"; continue; }
            let Ok(actual) = read_registration_recovery_json(&request_file) else { miss = "registered_request_missing"; continue; };
            if actual != reg.request || has_attachments(&actual)
                || !actual.get("message").and_then(Value::as_str).is_some_and(|message| !message.trim().is_empty()) {
                miss = "registered_request_mismatch"; continue;
            }
            let Ok(ready) = read_registration_recovery_json(&ready_file) else { miss = "registered_source_not_ready"; continue; };
            // Ready acknowledges the already-bound request; ordinary Win sources
            // may omit its optional project path or serialize it as null.
            let ready_project_matches = match ready.get("project_path") {
                None | Some(Value::Null) => true,
                Some(Value::String(path)) => local_project_paths_match(path, project_path),
                Some(_) => false,
            };
            if ready.get("request_id").and_then(Value::as_str) != Some(request_id)
                || !ready_project_matches
                || !ready.get("ready_at").and_then(Value::as_str).and_then(|time| chrono::DateTime::parse_from_rfc3339(time).ok())
                    .is_some_and(|time| time.timestamp_millis() >= created_at_ms
                        && time.timestamp_millis() <= chrono::Utc::now().timestamp_millis()) {
                miss = "registered_ready_mismatch"; continue;
            }
            // Recheck after reading assets so completion/lease expiry wins over recovery.
            if !recovery_registration_pending(dir, temp_dir, &reg) { miss = "registered_source_not_pending"; continue; }
            return Ok(json!({"request":reg.request,"showMcpPopup":true,"timelineNodes":[],
                "cache_source":"registered_local_source"}));
        }
    }
    Err(miss)
}

fn normalize_project_path(raw: &str) -> String {
    let path = Path::new(raw);
    if path.is_relative() {
        fs::canonicalize(path).map(|value| value.to_string_lossy().to_string()).unwrap_or_else(|_| {
            std::env::current_dir().map(|cwd| cwd.join(path).to_string_lossy().to_string())
                .unwrap_or_else(|_| raw.to_string())
        })
    } else { raw.to_string() }
}

fn android_action_response(payload: &Value, request_id: &str, project_path: &str) -> Result<Value> {
    if has_attachments(payload) { return Err("attachments_not_supported".into()); }
    let action = payload.get("action").and_then(Value::as_str).unwrap_or("");
    let action_id = payload.get("client_action_id").and_then(Value::as_str)
        .filter(|value| !value.is_empty()).ok_or("missing_client_action_id")?;
    let options = payload.get("selected_options").cloned().unwrap_or_else(|| json!([]));
    let mut metadata = json!({"source":"android_cross_device","request_id":request_id,
        "client_action_id":action_id});
    let input = match action {
        "submit" => payload.get("user_input").cloned().unwrap_or(Value::Null),
        "cancel" => {
            metadata["source"] = json!("popup_closed");
            Value::Null
        },
        "continue" => Value::Null,
        "enhance" => payload.get("user_input").cloned().unwrap_or(Value::Null),
        "goal" | "goal_start" => {
            let (goal, title, _) = crate::bridge::android_goal_payload_parts(payload);
            if goal.is_empty() { return Err("goal_input_missing".into()); }
            metadata["mode"] = json!("goalrun_takeover");
            metadata["goal_title"] = json!(title);
            json!(crate::bridge::android_goal_submit_prompt(&goal))
        }
        _ => return Err("unsupported_mcp_action".into()),
    };
    let options = match action {
        "continue" => json!(["继续"]),
        "enhance" => json!(["增强"]),
        _ => options,
    };
    Ok(json!({"user_input":input,"selected_options":options,"images":[],
        "project_path":project_path,"metadata":metadata}))
}

// None means this is not a cross-device source and the existing local route
// may handle it. A committed response must never fall through to that route.
pub async fn submit_android_action(payload: &Value, request_id: &str, project_path: &str)
    -> Option<Result<bool>> {
    let reg = match android_registration(request_id, project_path) {
        Ok(Some(reg)) => reg,
        Ok(None) => return None,
        Err(error) => return Some(Err(error)),
    };
    let response = match android_action_response(payload, request_id, project_path) {
        Ok(value) => value,
        Err(error) => return Some(Err(error)),
    };
    if let Err(error) = commit(&reg, &response, true) { return Some(Err(error)); }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
    loop {
        let delivered = (|| {
            let _guard = reg.request_lock().ok()?;
            let receipt = read_json::<Value>(&reg.result_path().ok()?).ok()?;
            Some(receipt.get("response") == Some(&response)
                && receipt.get("source_consumed").and_then(Value::as_bool) == Some(true))
        })().unwrap_or(false);
        if delivered { return Some(Ok(true)); }
        if tokio::time::Instant::now() >= deadline { return Some(Ok(false)); }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

// A mobile mirror is a text-only capability bound to a real local mirror and
// the current paired connection. It never represents a local project route.
#[derive(Clone, Serialize, Deserialize)]
struct MobileMirrorBinding {
    key: String,
    revision: String,
    request: Value,
    #[serde(default)]
    origin_device_id: Option<String>,
    #[serde(default)]
    origin_name: Option<String>,
    #[serde(default, deserialize_with = "deserialize_display_platform")]
    origin_platform: Option<String>,
}

fn mobile_mirror_id(key: &str, revision: &str) -> String {
    let material = serde_json::to_vec(&(key, revision)).expect("string identity serialization");
    format!("cross-mirror:{}", hex::encode(ring::digest::digest(&ring::digest::SHA256, &material)))
}

fn mobile_mirror_active(dir: &Path, binding: &MobileMirrorBinding) -> bool {
    let fresh = fs::metadata(dir.join("mirror-leases").join(&binding.key)).ok()
        .and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < Duration::from_secs(15));
    fresh && dir.join("mirrors").join(format!("{}.json", binding.key)).is_file()
        && dir.join("mirrors").join(format!("{}.ready", binding.key)).is_file()
        && !dir.join("dismissed").join(&binding.key).exists()
}

fn load_mobile_mirror_bindings(dir: &Path, revision: &str) -> Vec<MobileMirrorBinding> {
    fs::read_dir(dir.join("mobile-mirrors")).into_iter().flatten().flatten()
        .filter_map(|entry| read_json::<MobileMirrorBinding>(&entry.path()).ok())
        .filter(|binding| binding.revision == revision && uuid::Uuid::parse_str(&binding.key).is_ok()
            && binding.request.get("id").and_then(Value::as_str) == Some(format!("cross-{}", binding.key).as_str())
            && binding.request.get("project_path").is_none())
        .collect()
}

fn refresh_mobile_mirror_display(dir: &Path, revision: &str, registration: &Value) -> Result<()> {
    let Some(key) = registration.get("key").and_then(Value::as_str)
        .filter(|key| uuid::Uuid::parse_str(key).is_ok()) else { return Ok(()); };
    let Some(source) = registration.get("request").filter(|request| request.is_object()) else { return Ok(()); };
    let path = dir.join("mobile-mirrors").join(format!("{key}.json"));
    let Ok(mut binding) = read_json::<MobileMirrorBinding>(&path) else { return Ok(()); };
    if binding.key != key || binding.revision != revision || !mobile_mirror_active(dir, &binding)
        || binding.request.get("id").and_then(Value::as_str) != Some(format!("cross-{key}").as_str())
        || binding.request.get("project_path").is_some() { return Ok(()); }
    let before = serde_json::to_value(&binding).map_err(|e| e.to_string())?;
    binding.origin_device_id = display_identity(registration.get("origin_device_id"));
    binding.origin_name = display_identity(registration.get("origin_name"));
    binding.origin_platform = display_platform(registration.get("origin_platform"));
    binding.request["conversation_title"] = json!(mirror_conversation_title(source));
    if serde_json::to_value(&binding).map_err(|e| e.to_string())? != before {
        atomic_json(&path, &binding)?;
    }
    Ok(())
}

async fn adopt_live_mobile_mirrors(dir: &Path, revision: &str) -> Result<()> {
    let registered = load_mobile_mirror_bindings(dir, revision);
    let pending: Vec<_> = fs::read_dir(dir.join("mirrors")).into_iter().flatten().flatten()
        .filter_map(|entry| {
            let key = entry.path().file_stem()?.to_str()?.to_string();
            if uuid::Uuid::parse_str(&key).is_err() || registered.iter().any(|binding| binding.key == key) { return None; }
            let request: Value = read_json(&entry.path()).ok()?;
            let binding = MobileMirrorBinding { key, revision: revision.into(), request,
                origin_device_id: None, origin_name: None, origin_platform: None };
            mobile_mirror_active(dir, &binding).then_some(binding)
        }).collect();
    if pending.is_empty() { return Ok(()); }
    // Compatibility with an already-running desktop daemon: adopt its real,
    // leased mirror only after the currently paired source confirms that UUID.
    let source = peer("/snapshot", None).await?;
    if source.get("enabled").and_then(Value::as_bool) != Some(true) { return Ok(()); }
    let Some(requests) = source.get("requests").and_then(Value::as_array) else { return Ok(()); };
    let lock_dir = dir.to_path_buf();
    let _guard = tokio::task::spawn_blocking(move || lock(&lock_dir, "connection.lock"))
        .await.map_err(|e| e.to_string())??;
    if transport::route_key()? != revision { return Ok(()); }
    for mut binding in pending {
        if mobile_mirror_active(dir, &binding)
            && binding.request.get("id").and_then(Value::as_str) == Some(format!("cross-{}", binding.key).as_str())
            && binding.request.get("project_path").is_none()
            && requests.iter().any(|request| request.get("key").and_then(Value::as_str) == Some(binding.key.as_str())) {
            // This outer registration came from the authenticated paired peer.
            // Never infer origin from its editable request/title or the local host.
            let registration = requests.iter().find(|request| request.get("key").and_then(Value::as_str) == Some(binding.key.as_str())).unwrap();
            binding.origin_device_id = display_identity(registration.get("origin_device_id"));
            binding.origin_name = display_identity(registration.get("origin_name"));
            binding.origin_platform = display_platform(registration.get("origin_platform"));
            if let Some(source) = registration.get("request").filter(|request| request.is_object()) {
                binding.request["conversation_title"] = json!(mirror_conversation_title(source));
            }
            atomic_json(&dir.join("mobile-mirrors").join(format!("{}.json", binding.key)), &binding)?;
        }
    }
    Ok(())
}

pub(crate) async fn mobile_mirror_states() -> Vec<Value> {
    let Ok(dir) = directory() else { return Vec::new() };
    if !dir.join("mirrors").is_dir() || !settings().is_ok_and(|s| s.enabled) {
        return Vec::new();
    }
    let Ok(revision) = transport::route_key() else { return Vec::new() };
    let _ = adopt_live_mobile_mirrors(&dir, &revision).await;
    let states = load_mobile_mirror_bindings(&dir, &revision).into_iter()
        .filter(|binding| mobile_mirror_active(&dir, binding))
        .filter(|binding| {
            let id = mobile_mirror_id(&binding.key, &revision);
            !read_json::<Value>(&dir.join("mobile-mirror-receipts").join(format!("{}.json", id.trim_start_matches("cross-mirror:"))))
                .ok().is_some_and(|receipt| receipt.get("status").and_then(Value::as_str) == Some("mac_accepted"))
        })
        .map(|binding| {
            let id = mobile_mirror_id(&binding.key, &revision);
            // Rebuild an allowlist instead of forwarding persisted request metadata.
            json!({"mirror_id":id,"request_id":id,"source":"cross_device_mirror",
                "origin_device_id":binding.origin_device_id,"origin_name":binding.origin_name,
                "origin_platform":binding.origin_platform,
                "request":{"id":id,"message":binding.request.get("message"),
                    "conversation_title":binding.request.get("conversation_title"),
                    "predefined_options":binding.request.get("predefined_options"),
                    "is_markdown":binding.request.get("is_markdown")}})
        }).collect();
    if transport::route_key().ok().as_deref() != Some(&revision) { Vec::new() } else { states }
}

fn mobile_mirror_response(payload: &Value, binding: &MobileMirrorBinding) -> Result<Value> {
    let input = payload.get("user_input").filter(|v| !v.is_null());
    if input.is_some_and(|v| !v.is_string() || v.as_str().is_some_and(|s| s.len() > 65536)) {
        return Err("mirror_invalid_text".into());
    }
    let options = payload.get("selected_options").cloned().unwrap_or_else(|| json!([]));
    let options_array = options.as_array().ok_or("mirror_invalid_options")?;
    let allowed = binding.request.get("predefined_options").and_then(Value::as_array);
    if options_array.len() > 64 || options_array.iter().any(|option|
        !option.is_string() || !allowed.is_some_and(|allowed| allowed.contains(option))) {
        return Err("mirror_invalid_options".into());
    }
    if input.and_then(Value::as_str).is_none_or(|text| text.trim().is_empty()) && options_array.is_empty() {
        return Err("mirror_empty_reply".into());
    }
    Ok(json!({"user_input":input,"selected_options":options,"images":[],
        "metadata":{"source":"android_cross_device_mirror",
            "request_id":payload.get("request_id"),"client_action_id":payload.get("client_action_id")}}))
}

pub(crate) async fn submit_mobile_mirror_action(payload: &Value) -> Result<String> {
    let id = payload.get("mirror_id").and_then(Value::as_str).ok_or("mirror_identity_missing")?;
    let digest = id.strip_prefix("cross-mirror:").ok_or("mirror_identity_invalid")?;
    if digest.len() != 64 || !digest.bytes().all(|c| c.is_ascii_hexdigit())
        || payload.get("request_id").and_then(Value::as_str) != Some(id) {
        return Err("mirror_identity_invalid".into());
    }
    let fields = ["mirror_id", "request_id", "action", "client_action_id", "user_input", "selected_options"];
    if payload.as_object().is_none_or(|object| object.keys().any(|key| !fields.contains(&key.as_str()))) {
        return Err("mirror_text_only".into());
    }
    let action_id = payload.get("client_action_id").and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty() && id.len() <= 128).ok_or("missing_client_action_id")?;
    let action = payload.get("action").and_then(Value::as_str).ok_or("mirror_action_missing")?;
    if !matches!(action, "submit" | "close_mirror") { return Err("mirror_action_not_supported".into()); }
    let dir = directory()?;
    let lock_dir = dir.clone();
    let lock_id = digest.to_string();
    let (_connection_guard, _action_guard) = tokio::task::spawn_blocking(move || {
        Ok::<_, String>((lock(&lock_dir, "connection.lock")?, lock(&lock_dir.join("mobile-mirror-locks"), &lock_id)?))
    }).await.map_err(|e| e.to_string())??;
    if !settings()?.enabled { return Err("mirror_disabled".into()); }
    let revision = transport::route_key()?;
    let binding = load_mobile_mirror_bindings(&dir, &revision).into_iter()
        .find(|binding| mobile_mirror_id(&binding.key, &revision) == id).ok_or("mirror_not_registered")?;
    let response = if action == "submit" { mobile_mirror_response(payload, &binding)? } else { Value::Null };
    let receipt_path = dir.join("mobile-mirror-receipts").join(format!("{digest}.json"));
    let previous = read_json::<Value>(&receipt_path).ok();
    let same_attempt = previous.as_ref().is_some_and(|previous|
        previous.get("client_action_id").and_then(Value::as_str) == Some(action_id)
            && previous.get("action").and_then(Value::as_str) == Some(action)
            && previous.get("response") == Some(&response));
    if let Some(previous) = &previous {
        match previous.get("status").and_then(Value::as_str) {
            Some("mac_accepted" | "mirror_closed") if same_attempt =>
                return Ok(previous["status"].as_str().unwrap().to_string()),
            Some("mac_accepted" | "mirror_closed" | "pending") if !same_attempt =>
                return Err("mirror_previous_reply_conflict".into()),
            _ => {},
        }
    }
    if !mobile_mirror_active(&dir, &binding) && !same_attempt { return Err("mirror_not_active".into()); }
    let mut receipt = json!({"client_action_id":action_id,"action":action,"response":response,"status":"pending"});
    if action == "close_mirror" {
        record_dismissal(&dir, &binding.key)?;
        receipt["status"] = json!("mirror_closed");
        atomic_json(&receipt_path, &receipt)?;
        return Ok("mirror_closed".into());
    }
    atomic_json(&receipt_path, &receipt)?;
    let remote = peer("/submit", Some(&json!({"key":binding.key,"response":response}))).await
        .map_err(|error| format!("mirror_confirmation_unknown:{error}"))?;
    if remote.get("ok").and_then(Value::as_bool) != Some(true) {
        receipt["status"] = json!("rejected");
        atomic_json(&receipt_path, &receipt)?;
        return Err(remote.get("error").and_then(Value::as_str).unwrap_or("mirror_submit_rejected").into());
    }
    // ok:true means accepted/persisted by Mac. It does not prove source_consumed.
    receipt["status"] = json!("mac_accepted");
    atomic_json(&receipt_path, &receipt).map_err(|error| format!("mirror_confirmation_unknown:{error}"))?;
    Ok("mac_accepted".into())
}

fn response_contains(accepted: &Value, observed: &Value) -> bool {
    match (accepted, observed) {
        (Value::Object(a), Value::Object(b)) => a.iter().all(|(key, value)|
            b.get(key).is_some_and(|actual| response_contains(value, actual))),
        _ => accepted == observed,
    }
}

pub fn mark_source_prepared(reg: &Registration, response: &Value) -> Result<()> {
    let _admission = hub_transport::submission_guard(true)?;
    let _guard = reg.request_lock()?;
    let path = reg.result_path()?;
    let mut receipt: Value = read_json(&path)?;
    if !receipt.get("response").is_some_and(|accepted| response_contains(accepted, response)) {
        return Err("源端消费的回复与获胜回复不一致".into());
    }
    receipt["source_prepared"] = json!(true);
    atomic_json(&path, &receipt)
}

// Called only by the HTTP handler after its response channel delivered the
// prepared reply and a successful response body has been constructed.
pub fn mark_source_handed_off(request_id: &str, project_path: &str) -> Result<()> {
    let _admission = hub_transport::submission_guard(true)?;
    let reg = android_registration(request_id, &normalize_project_path(project_path))?
        .ok_or("源请求不存在")?;
    let _guard = reg.request_lock()?;
    let path = reg.result_path()?;
    let mut receipt: Value = read_json(&path)?;
    if receipt.get("source_prepared").and_then(Value::as_bool) != Some(true) {
        return Err("源回复尚未准备完成".into());
    }
    receipt["source_consumed"] = json!(true);
    atomic_json(&path, &receipt)
}

pub async fn submit(response: &Value) -> Result<bool> {
    // Even a popup created while registration was paused must not bypass the
    // cloud freeze through the ordinary, unregistered local response path.
    let _unregistered_admission = if std::env::var("ITERATE_CROSS_DEVICE_SOURCE")
        .ok().is_some_and(|key| !key.is_empty()) { None }
        else { hub_transport::submission_guard(false)? };
    if let Ok(key) = std::env::var("ITERATE_CROSS_DEVICE_MIRROR") {
        if response.as_str() == Some("CANCELLED")
            || response.pointer("/metadata/source").and_then(Value::as_str) == Some("popup_closed")
        {
            let dir = directory()?;
            let _guard = lock(&dir, "connection.lock")?;
            record_dismissal(&dir, &key)?;
            return Ok(true);
        }
        if has_attachments(response) {
            return Err("镜像仅支持文本和选项，请在源设备发送附件".into());
        }
        let result = peer("/submit", Some(&json!({"key":key,"response":response}))).await?;
        if result.get("ok").and_then(Value::as_bool) != Some(true) {
            return Err(result
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("发送未确认")
                .into());
        }
        return Ok(true);
    }
    if let Ok(key) = std::env::var("ITERATE_CROSS_DEVICE_SOURCE") {
        if key.is_empty() {
            return Ok(false);
        }
        commit(&load_registration(&key)?, response, false)?;
        return Ok(false);
    }
    Ok(false)
}

pub fn source_finalize_guard() -> Result<Option<File>> {
    let _admission = hub_transport::submission_guard(true)?;
    let Ok(key) = std::env::var("ITERATE_CROSS_DEVICE_SOURCE") else {
        return Ok(None);
    };
    if key.is_empty() {
        return Ok(None);
    }
    let reg = load_registration(&key)?;
    let dir = directory()?.join("finalizing");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join(key))
        .map_err(|e| e.to_string())?;
    file.try_lock().map_err(|_| "获胜回复正在处理中")?;
    if reg.response_file.exists() {
        return Err("回复已完成".into());
    }
    Ok(Some(file))
}

pub fn publish_source_response(path: &Path, response: &Value) -> Result<()> {
    let finishing_registered = std::env::var("ITERATE_CROSS_DEVICE_SOURCE")
        .ok().is_some_and(|key| !key.is_empty());
    let _admission = hub_transport::submission_guard(finishing_registered)?;
    atomic_json(path, response)
}

#[tauri::command]
pub async fn get_cross_device_status() -> Value {
    let mirror = std::env::var_os("ITERATE_CROSS_DEVICE_MIRROR").is_some();
    let local = match settings() {
        Ok(s) => s,
        Err(error) => {
            return json!({"enabled":false,"connected":false,"mirror":mirror,"error":error})
        }
    };
    // The source popup polls this every two seconds. Deliver an accepted
    // response immediately, even if its registration was never published or
    // the paired desktop is offline; network health checks can take seconds.
    let source_pending = std::env::var("ITERATE_CROSS_DEVICE_SOURCE").ok().filter(|key|!key.is_empty())
        .and_then(|key|load_registration(&key).ok()).filter(|reg|!reg.response_file.exists())
        .and_then(|reg|read_json::<Value>(&reg.result_path().ok()?).ok()
            .filter(|receipt| receipt.get("source_prepared").and_then(Value::as_bool) != Some(true)
                && receipt.get("source_consumed").and_then(Value::as_bool) != Some(true))
            .and_then(|receipt|receipt.get("response").cloned())
            .map(|response| {
                json!({"response":response,"request_id":reg.request.get("id"),
                    "project_path":reg.request.get("project_path"),
                    "timeline_route_id":reg.ordinary_timeline_route()})
            }));
    if source_pending.is_some() {
        return json!({"enabled":local.enabled,"device_id":local.device_id,"mirror":mirror,
            "source_pending":source_pending});
    }
    if let Ok(Some(config))=hub_transport::config().map(|config|config.filter(|_|!mirror)){
        return json!({"enabled":local.enabled,"device_id":config.device_id,"transport":"cloud_hub","origin_name":connection_config().ok().map(|c|c.device_name),
            "connected":hub_transport::ready(),"peer_enabled":hub_transport::ready(),"mirror":false,"source_pending":source_pending,
            "local_only":false,"text_only":std::env::consts::OS=="macos"});
    }
    let remote = peer("/snapshot", None).await;
    let local_ready = local_daemon_ready().await;
    let mirror_key = std::env::var("ITERATE_CROSS_DEVICE_MIRROR").ok();
    let local_only =
        std::env::var("ITERATE_CROSS_DEVICE_SOURCE").is_ok_and(|key| key.is_empty()) && !mirror;
    let resolved = mirror_key.as_ref().is_some_and(|key| {
        remote
            .as_ref()
            .ok()
            .and_then(|v| v.get("requests"))
            .and_then(Value::as_array)
            .is_some_and(|items| {
                !items
                    .iter()
                    .any(|r| r.get("key").and_then(Value::as_str) == Some(key.as_str()))
            })
    });
    let origin_name = if mirror {
        std::env::var("ITERATE_CROSS_DEVICE_ORIGIN_NAME").unwrap_or_else(|_| "另一设备".into())
    } else {
        connection_config()
            .map(|c| c.device_name)
            .unwrap_or_else(|_| std::env::consts::OS.into())
    };
    json!({"enabled":local.enabled,"device_id":local.device_id,"origin_name":origin_name,"connected":remote.is_ok() && local_ready,"resolved":resolved,"local_only":local_only,"source_pending":source_pending,
        "connected_ip":remote.as_ref().ok().and_then(|v|v.get("connected_ip")),"using_backup":remote.as_ref().ok().and_then(|v|v.get("using_backup")),
        "peer_enabled":remote.as_ref().ok().and_then(|v|v.get("enabled")).and_then(Value::as_bool).unwrap_or(false),
        "mirror":mirror,"error":remote.err(),"text_only":true})
}

#[tauri::command]
pub async fn set_cross_device_enabled(enabled: bool) -> Result<Value> {
    if enabled {
        transport::validate_enable()?;
        transport::ensure_daemon().await?;
    }
    let mut current = settings()?;
    let dir = directory()?;
    {
        let _guard = lock(&dir, "settings.lock")?;
        current.enabled = enabled;
        atomic_json(&dir.join("settings.json"), &current)?;
    }
    Ok(get_cross_device_status().await)
}

#[tauri::command]
pub async fn sync_cross_device_windows() -> Result<Value> {
    if !settings()?.enabled {
        return Err("请先开启本机跨设备开关".into());
    }
    transport::ensure_daemon().await?;
    let revision = transport::route_key()?;
    // Only dismissals observed before this action may be restored.
    let restore = {
        let dir = directory()?;
        let _guard = lock(&dir, "connection.lock")?;
        capture_dismissals(&dir)
    };
    let remote = peer("/snapshot", None).await?;
    if !supports_window_sync(&remote) {
        return Err("配对端跨设备服务不支持窗口同步，请更新并重启配对端服务后重试".into());
    }
    if remote.get("enabled").and_then(Value::as_bool) != Some(true) {
        return Err("请先开启配对端跨设备开关".into());
    }
    let remote = peer("/sync-windows", Some(&json!({}))).await
        .map_err(|e| format!("读取对端窗口失败，请确认对端已更新并重启：{e}"))?;
    let keys: Vec<String> = remote.get("requests").and_then(Value::as_array)
        .ok_or("对端返回了无效窗口列表")?.iter()
        .map(|item| {
            let key = item.get("key").and_then(Value::as_str).ok_or("缺少窗口标识")?;
            uuid::Uuid::parse_str(key).map_err(|_| "无效窗口标识")?;
            Ok(key.to_string())
        }).collect::<Result<_>>()?;
    let queue = directory()?.join("window-sync");
    fs::create_dir_all(&queue).map_err(|e| e.to_string())?;
    let job = tempfile::tempdir_in(queue).map_err(|e| e.to_string())?;
    atomic_json(&job.path().join("request.json"), &json!({
        "revision": revision, "keys": keys, "restore": restore, "created_at": chrono::Utc::now().timestamp_millis(),
        "expires_at": chrono::Utc::now().timestamp() + 30
    }))?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        if !settings()?.enabled || transport::route_key()? != revision {
            return Err("跨设备开关或配对已改变，请重新同步".into());
        }
        if let Ok(result) = read_json::<Value>(&job.path().join("result.json")) {
            if let Some(error) = result.get("error").and_then(Value::as_str) {
                return Err(error.into());
            }
            return Ok(result);
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("等待窗口同步超时，请检查两端连接后重试".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct Broker {
    token: String,
    revision: String,
    device_id: String,
    network_ready: std::sync::atomic::AtomicBool,
}
fn authorized(headers: &HeaderMap, state: &Broker) -> bool {
    headers.get("authorization").is_some_and(|header| {
        ring::constant_time::verify_slices_are_equal(
            header.as_bytes(),
            format!("Bearer {}", state.token).as_bytes(),
        )
        .is_ok()
    })
}

async fn settings_snapshot(
    State(state): State<Arc<Broker>>,
    headers: HeaderMap,
    Json(request): Json<settings_sync::ExportRequest>,
) -> std::result::Result<Json<Value>, StatusCode> {
    if !authorized(&headers, &state) { return Err(StatusCode::UNAUTHORIZED); }
    if transport::route_key().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? != state.revision {
        return Err(StatusCode::CONFLICT);
    }
    let snapshot = settings_sync::settings_sync_export(request.categories)
        .map_err(|_| StatusCode::UNPROCESSABLE_ENTITY)?;
    let bytes = settings_sync::bounded_bytes(&snapshot).map_err(|_| StatusCode::PAYLOAD_TOO_LARGE)?;
    serde_json::from_slice(&bytes).map(Json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn snapshot(
    State(state): State<Arc<Broker>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, StatusCode> {
    if !authorized(&headers, &state) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let config = settings().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut requests = Vec::new();
    if let Ok(entries) = fs::read_dir(
        directory()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .join("requests"),
    ) {
        for entry in entries.flatten() {
            if let Ok(reg) = read_json::<Registration>(&entry.path()) {
                if config.enabled && reg.published && reg.active() && !reg.response_file.exists()
                    && text_mirror_supported(&reg.request) {
                    requests.push(
                        json!({"key":reg.key,"origin_device_id":reg.origin_device_id,
                    "origin_name":reg.origin_name,"origin_platform":reg.origin_platform,"request":reg.request}),
                    );
                }
            }
        }
    }
    Ok(Json(
        json!({"version":VERSION,"capabilities":[WINDOW_SYNC_CAPABILITY],"config_revision":state.revision,"enabled":config.enabled,"device_id":config.device_id,"device_name":connection_config().ok().map(|c|c.device_name),"requests":requests}),
    ))
}

// Authenticated pulls publish requests opened while offline; automatic pulls
// never restore a mirror's local dismissal.
async fn sync_open_windows(
    State(state): State<Arc<Broker>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, StatusCode> {
    if !authorized(&headers, &state) { return Err(StatusCode::UNAUTHORIZED); }
    let _admission = hub_transport::submission_guard(false).map_err(|_|StatusCode::CONFLICT)?;
    {
        let dir = directory().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let _guard = lock(&dir, "connection.lock").map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        if transport::route_key().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)? != state.revision {
            return Err(StatusCode::CONFLICT);
        }
        if !settings().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?.enabled {
            return Err(StatusCode::CONFLICT);
        }
        // Separate deferred records are invisible to old daemons. Read the old
        // layout as well so already registered source windows remain usable.
        for folder in ["requests", "deferred-requests"] {
            if let Ok(entries) = fs::read_dir(dir.join(folder)) {
                for entry in entries.flatten() {
                    if let Ok(reg) = read_json::<Registration>(&entry.path()) {
                        let _request=reg.request_lock().map_err(|_|StatusCode::INTERNAL_SERVER_ERROR)?;
                        let Ok(mut reg)=load_registration(&reg.key) else {continue};
                        if !reg.published && reg.active() && !reg.response_file.exists()
                            && text_mirror_supported(&reg.request) {
                            reg.published = true;
                            atomic_json(&reg.path().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?, &reg)
                                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                            if folder == "deferred-requests" {
                                fs::remove_file(entry.path()).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
                            }
                        }
                    }
                }
            }
        }
    }
    snapshot(State(state), headers).await
}

// Local readiness must not traverse a VPN interface: peer-only WireGuard routes
// can reach the other device while rejecting traffic to this device's own IP.
async fn health(
    State(state): State<Arc<Broker>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, StatusCode> {
    if !authorized(&headers, &state) { return Err(StatusCode::UNAUTHORIZED); }
    if !state.network_ready.load(std::sync::atomic::Ordering::Acquire) {
        return Err(StatusCode::SERVICE_UNAVAILABLE);
    }
    Ok(Json(json!({"version":VERSION,"capabilities":[WINDOW_SYNC_CAPABILITY],"config_revision":state.revision,"device_id":state.device_id})))
}

pub fn mirror_process_guard() -> Result<Option<File>> {
    let Ok(key) = std::env::var("ITERATE_CROSS_DEVICE_MIRROR") else {
        return Ok(None);
    };
    uuid::Uuid::parse_str(&key).map_err(|_| "无效镜像标识")?;
    let dir = directory()?.join("mirror-locks");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join(key))
        .map_err(|e| e.to_string())?;
    file.try_lock().map_err(|_| "该镜像窗口已存在")?;
    Ok(Some(file))
}

async fn submit_peer(
    State(state): State<Arc<Broker>>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> std::result::Result<Json<Value>, StatusCode> {
    if !authorized(&headers, &state) {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let result = (|| {
        let reg = load_registration(
            body.get("key")
                .and_then(Value::as_str)
                .ok_or("缺少请求标识")?,
        )?;
        let response = body.get("response").ok_or("缺少响应")?;
        if !response.is_object() {
            return Err("无效响应".into());
        }
        commit(&reg, response, true)
    })();
    Ok(Json(match result {
        Ok(()) => {
            json!({"version":VERSION,"device_id":settings().ok().map(|s|s.device_id),"ok":true})
        }
        Err(error) => {
            json!({"version":VERSION,"device_id":settings().ok().map(|s|s.device_id),"ok":false,"error":error})
        }
    }))
}

struct Mirror {
    child: Child,
    request_file: PathBuf,
    ready_file: PathBuf,
}
impl Drop for Mirror {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.request_file);
        let _ = fs::remove_file(&self.ready_file);
        if let (Ok(dir), Some(key)) = (directory(), self.request_file.file_stem()) {
            let _ = fs::remove_file(dir.join("mirror-leases").join(key));
        }
    }
}

fn renew_mirror_lease(key: &str) {
    if let Ok(dir) = directory() {
        let folder = dir.join("mirror-leases");
        let _ = fs::create_dir_all(&folder);
        let _ = fs::write(folder.join(key), b"active");
    }
}

fn mirror_conversation_title(source: &Value) -> &str {
    source.get("conversation_title").and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty()).unwrap_or("未命名会话")
}

fn spawn_mirror(item: &Value) -> Result<Mirror> {
    let key = item
        .get("key")
        .and_then(Value::as_str)
        .ok_or("缺少请求标识")?;
    uuid::Uuid::parse_str(key).map_err(|_| "无效请求标识")?;
    let source = item.get("request").ok_or("缺少请求")?;
    let name = item
        .get("origin_name")
        .and_then(Value::as_str)
        .unwrap_or("另一设备");
    // Explicit allowlist: never import paths, checkpoint IDs, deeplinks or flags.
    let text = source.get("message").and_then(Value::as_str).unwrap_or("");
    let plain_links = regex::Regex::new(r"\[([^\]]*)\]\([^)]*\)")
        .map_err(|e| e.to_string())?
        .replace_all(text, "$1");
    let request = json!({"id":format!("cross-{key}"),"message":format!("{plain_links}\n\n---\n\n注:跨设备来源，仅支持文本和选项"),
        "predefined_options":source.get("predefined_options").cloned().unwrap_or(json!([])),
        "is_markdown":source.get("is_markdown").cloned().unwrap_or(json!(true)),
        "conversation_title":mirror_conversation_title(source)});
    let dir = directory()?.join("mirrors");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let request_file = dir.join(format!("{key}.json"));
    let ready_file = dir.join(format!("{key}.ready"));
    if ready_file.exists() {
        fs::remove_file(&ready_file).map_err(|e| e.to_string())?;
    }
    atomic_json(&request_file, &request)?;
    atomic_json(&directory()?.join("mobile-mirrors").join(format!("{key}.json")),
        &MobileMirrorBinding { key: key.into(), revision: transport::route_key()?, request: request.clone(),
            origin_device_id: display_identity(item.get("origin_device_id")),
            origin_name: display_identity(item.get("origin_name")),
            origin_platform: display_platform(item.get("origin_platform")) })?;
    let executable = std::env::var_os("ITERATE_DIALOG_GUI_EXECUTABLE")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe().map_err(|e| e.to_string())?);
    let mut command = Command::new(executable);
    command
        .env("ITERATE_MCP_REQUEST_FILE", &request_file)
        .env("ITERATE_READY_FILE", &ready_file)
        .env("ITERATE_STANDALONE_MODE", "1")
        .env("ITERATE_CROSS_DEVICE_MIRROR", key)
        .env("ITERATE_CROSS_DEVICE_ORIGIN_NAME", name)
        .env_remove("ITERATE_RESPONSE_FILE")
        .env_remove("ITERATE_DELIVERY_FILE")
        .env_remove("ITERATE_CROSS_DEVICE_SOURCE")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let child = command.spawn().map_err(|e| e.to_string())?;
    renew_mirror_lease(key);
    Ok(Mirror {
        child,
        request_file,
        ready_file,
    })
}

fn record_dismissal(dir: &Path, key: &str) -> Result<()> {
    uuid::Uuid::parse_str(key).map_err(|_| "无效镜像标识")?;
    let path = dir.join("dismissed").join(key);
    // The popup records its close before exiting; the daemon must preserve that
    // marker when it later reaps the process (including across a sync click).
    if !path.exists() { atomic_json(&path, &json!(uuid::Uuid::new_v4().to_string()))?; }
    Ok(())
}

fn capture_dismissals(dir: &Path) -> HashMap<String, Value> {
    fs::read_dir(dir.join("dismissed")).into_iter().flatten().flatten()
        .filter_map(|entry| Some((entry.file_name().to_str()?.to_string(), read_json(&entry.path()).ok()?)))
        .collect()
}

fn restore_dismissals(dir: &Path, keys: &HashSet<String>, restore: &HashMap<String, Value>, dismissed: &mut HashSet<String>) {
    for key in keys {
        if uuid::Uuid::parse_str(key).is_err() { continue; }
        let path = dir.join("dismissed").join(key);
        if let Some(marker) = restore.get(key) {
            if read_json::<Value>(&path).ok().as_ref() == Some(marker) && fs::remove_file(&path).is_ok() {
                dismissed.remove(key);
            }
        }
    }
}

async fn automatic_window_snapshot(
    revision: Option<&str>,
    snapshot: Value,
    pull: impl std::future::Future<Output = Result<Value>>,
) -> Result<Value> {
    if revision.is_none() || transport::route_key().ok().as_deref() != revision {
        return Err("配对已改变，请重新读取窗口".into());
    }
    if settings()?.enabled && snapshot.get("enabled").and_then(Value::as_bool) == Some(true)
        && supports_window_sync(&snapshot) {
        // Pull on every eligible poll so requests registered during reconnection
        // are picked up too. Failures retry on the next poll; do not use a stale
        // pre-pull snapshot to remove existing mirrors.
        pull.await
    } else {
        Ok(snapshot)
    }
}

async fn mirror_loop() {
    let mut mirrors: HashMap<String, Mirror> = HashMap::new();
    let mut dismissed: HashSet<String> = HashSet::new();
    loop {
        let poll_started = chrono::Utc::now().timestamp_millis();
        let route_key = transport::route_key().ok();
        let snapshot = match peer("/snapshot", None).await {
            Ok(snapshot) => automatic_window_snapshot(
                route_key.as_deref(), snapshot, peer("/sync-windows", Some(&json!({}))),
            ).await,
            Err(error) => Err(error),
        };
        if let Ok(snapshot) = snapshot {
            let Ok(dir) = directory() else {
                continue;
            };
            let Ok(_route_guard) = lock(&dir, "connection.lock") else {
                continue;
            };
            if transport::route_key().ok() != route_key {
                continue;
            }
            let requests = snapshot
                .get("requests")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let active: HashSet<String> = requests
                .iter()
                .filter_map(|r| r.get("key").and_then(Value::as_str).map(str::to_string))
                .collect();
            mirrors.retain(|key, mirror| {
                if !active.contains(key) || dir.join("dismissed").join(key).exists() {
                    return false;
                }
                if mirror.child.try_wait().ok().flatten().is_some() {
                    dismissed.insert(key.clone());
                    if let Ok(dir) = directory() {
                        let _ = record_dismissal(&dir, key);
                    }
                    return false;
                }
                renew_mirror_lease(key);
                true
            });
            dismissed.retain(|key| active.contains(key));
            let enabled = settings().is_ok_and(|s| s.enabled)
                && snapshot.get("enabled").and_then(Value::as_bool) == Some(true);
            let mut jobs = Vec::new();
            if let Ok(entries) = fs::read_dir(dir.join("window-sync")) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.join("result.json").exists() { continue; }
                    let Ok(job) = read_json::<Value>(&path.join("request.json")) else { continue; };
                    // A job created during this HTTP poll must use the next snapshot.
                    if job.get("created_at").and_then(Value::as_i64).unwrap_or(i64::MAX) >= poll_started { continue; }
                    if job.get("expires_at").and_then(Value::as_i64).unwrap_or(0) <= chrono::Utc::now().timestamp() {
                        let _ = atomic_json(&path.join("result.json"), &json!({"error":"窗口同步请求已过期，请重试"}));
                        continue;
                    }
                    if !enabled || job.get("revision").and_then(Value::as_str) != route_key.as_deref() {
                        let _ = atomic_json(&path.join("result.json"), &json!({"error":"请确认两端跨设备开关已开启且配对未改变"}));
                        continue;
                    }
                    let Ok(keys) = serde_json::from_value::<Vec<String>>(job["keys"].clone()) else { continue; };
                    let keys: HashSet<String> = keys.into_iter().filter(|key| active.contains(key)).collect();
                    // Restore this fixed batch only once, even if a user closes a
                    // window while other windows in the batch are still loading.
                    if !path.join("started.json").exists() {
                        let restore = serde_json::from_value::<HashMap<String, Value>>(job["restore"].clone()).unwrap_or_default();
                        // A close acknowledged by the popup may still be exiting.
                        // Reap that process before consuming its saved dismissal.
                        if keys.iter().any(|key| mirrors.contains_key(key)
                            && restore.get(key).is_some_and(|marker|
                                read_json::<Value>(&dir.join("dismissed").join(key)).ok().as_ref() == Some(marker))) {
                            continue;
                        }
                        if atomic_json(&path.join("started.json"), &json!(true)).is_err() { continue; }
                        restore_dismissals(&dir, &keys, &restore, &mut dismissed);
                    }
                    jobs.push((path, keys));
                }
            }
            let mut sync_errors = HashMap::new();
            if enabled {
                for item in &requests {
                    let Some(key) = item.get("key").and_then(Value::as_str) else {
                        continue;
                    };
                    if uuid::Uuid::parse_str(key).is_err() {
                        sync_errors.insert(key.to_string(), "对端返回了无效窗口标识".to_string());
                        continue;
                    }
                    if directory().is_ok_and(|dir| dir.join("dismissed").join(key).exists()) {
                        continue;
                    }
                    // Reuse this authenticated snapshot under the existing route
                    // lock. Only refresh display metadata on the exact live binding.
                    if let Some(revision) = route_key.as_deref() {
                        let _ = refresh_mobile_mirror_display(&dir, revision, item);
                    }
                    if !mirrors.contains_key(key) && !dismissed.contains(key) {
                        match spawn_mirror(item) {
                            Ok(mirror) => {
                                mirrors.insert(key.into(), mirror);
                            }
                            Err(e) => {
                                eprintln!("cross-device popup: {e}");
                                sync_errors.insert(key.to_string(), e);
                            }
                        }
                    }
                }
            }
            for (job, keys) in jobs {
                let ready = keys.iter().all(|key| mirrors.get(key).is_some_and(|m| m.ready_file.is_file()));
                let result = if let Some(error) = keys.iter().find_map(|key| sync_errors.get(key)) {
                    Some(json!({"error":format!("打开同步窗口失败：{error}")}))
                } else if keys.iter().any(|key| dismissed.contains(key) || dir.join("dismissed").join(key).exists()) {
                    Some(json!({"error":"本次同步的窗口已在本端关闭；需要恢复时请再次点击同步"}))
                } else if ready {
                    Some(json!({"count": keys.len()}))
                } else { None };
                if let Some(result) = result {
                    let _ = atomic_json(&job.join("result.json"), &result);
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

pub fn run_daemon(_port: u16) -> anyhow::Result<()> {
    let dir = directory().map_err(anyhow::Error::msg)?;
    let owner = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(dir.join("daemon.lock"))?;
    owner
        .try_lock()
        .map_err(|_| anyhow::anyhow!("跨设备服务已经运行"))?;
    atomic_json(
        &dir.join("daemon-process.json"),
        &json!({"pid":std::process::id()}),
    )
    .map_err(anyhow::Error::msg)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    tokio::runtime::Runtime::new()?.block_on(async move {
        let task = tokio::spawn(mirror_loop());
        loop {
            if !dir.join("direct-connection.json").is_file() { task.abort(); return Ok::<(), std::io::Error>(()); }
            let revision = transport::route_key().map_err(std::io::Error::other)?;
            let tls = transport::server_tls().map_err(std::io::Error::other)?;
            let state = Arc::new(Broker {
                token: credential().map_err(std::io::Error::other)?,
                revision: revision.clone(),
                device_id: settings().map_err(std::io::Error::other)?.device_id,
                network_ready: std::sync::atomic::AtomicBool::new(false),
            });
            let config = connection_config().map_err(std::io::Error::other)?;
            let app = Router::new()
                .route("/snapshot", get(snapshot))
                .route("/sync-windows", post(sync_open_windows))
                .route("/submit", post(submit_peer))
                .route("/api/settings-sync/snapshot", post(settings_snapshot)
                    .layer(DefaultBodyLimit::max(settings_sync::MAX_REQUEST_BYTES)))
                .layer(DefaultBodyLimit::max(1024 * 1024))
                .with_state(state.clone());
            // An OS-selected loopback port avoids collisions between local profiles.
            // It exposes only authenticated health, not the request/response routes.
            let health_listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
            let health_port = health_listener.local_addr()?.port();
            let health_handle = axum_server::Handle::new();
            let health_acceptor = axum_server::tls_rustls::RustlsAcceptor::new(tls.clone())
                .handshake_timeout(Duration::from_secs(5)).acceptor(transport::LimitedAcceptor::new());
            let health_app = Router::new().route("/health", get(health)).with_state(state.clone());
            let health_server = tokio::spawn(axum_server::from_tcp(health_listener)
                .acceptor(health_acceptor).handle(health_handle.clone()).serve(health_app.into_make_service()));
            atomic_json(&dir.join("daemon-process.json"), &json!({"pid":std::process::id(),"health_port":health_port}))
                .map_err(std::io::Error::other)?;
            let mut listeners: HashMap<std::net::Ipv4Addr, (axum_server::Handle, tokio::task::JoinHandle<std::io::Result<()>>)> = HashMap::new();
            let mut next_network_check = std::time::Instant::now();
            loop {
                if std::time::Instant::now() >= next_network_check {
                    let available = transport::local_ips(&config.listen_ip).map_err(std::io::Error::other)?;
                    listeners.retain(|ip, (handle, server)| {
                        if !available.contains(ip) || server.is_finished() {
                            handle.shutdown();
                            server.abort();
                            false
                        } else { true }
                    });
                    for ip in available {
                        if listeners.contains_key(&ip) { continue; }
                        // Missing interfaces/public NAT addresses are never bound as
                        // wildcard listeners. Retry new interfaces after network changes.
                        if let Ok(listener) = std::net::TcpListener::bind((ip, config.listen_port)) {
                            let handle = axum_server::Handle::new();
                            let acceptor = axum_server::tls_rustls::RustlsAcceptor::new(tls.clone())
                                .handshake_timeout(Duration::from_secs(5)).acceptor(transport::LimitedAcceptor::new());
                            let server = axum_server::from_tcp(listener).acceptor(acceptor)
                                .handle(handle.clone()).serve(app.clone().into_make_service());
                            let server = tokio::spawn(server);
                            if tokio::time::timeout(Duration::from_secs(1), handle.listening()).await.ok().flatten().is_some() {
                                listeners.insert(ip, (handle, server));
                            } else {
                                handle.shutdown();
                                server.abort();
                            }
                        }
                    }
                    next_network_check = std::time::Instant::now() + Duration::from_secs(2);
                }
                state.network_ready.store(!listeners.is_empty(), std::sync::atomic::Ordering::Release);
                if !dir.join("direct-connection.json").is_file() || transport::route_key().map_err(std::io::Error::other)? != revision {
                    state.network_ready.store(false, std::sync::atomic::Ordering::Release);
                    health_handle.graceful_shutdown(Some(Duration::from_millis(300)));
                    for (handle, _) in listeners.values() {
                        handle.graceful_shutdown(Some(Duration::from_millis(300)));
                    }
                    tokio::time::sleep(Duration::from_millis(350)).await;
                    for (_, server) in listeners.into_values() { server.abort(); }
                    health_server.abort();
                    break;
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "windows")]
    fn registered_recovery_fixture(published: bool) -> (tempfile::TempDir, tempfile::TempDir, Registration, crate::ui::window_registry::WindowInstance) {
        let dir = tempfile::tempdir().unwrap();
        let assets = tempfile::tempdir().unwrap();
        let request_id = "serve-1790916412343-6186ba51-bf52-4ca9-9c5c-b3835cca2e34";
        let request = json!({"id":request_id,"project_path":"E:/Github/iterate-desktop",
            "message":"原始 Win 文本\n保留格式","predefined_options":["手机仍看不到本条","原样选项"],
            "is_markdown":true,"parent_request_id":"original-parent"});
        let reg = Registration { key: "25ee12a1-7d76-47de-b597-d561d8c04337".into(),
            origin_device_id:"local-win".into(),origin_name:"Win".into(),origin_platform:None,request:request.clone(),
            request_file:assets.path().join(format!("iterate_request_{request_id}.json")),
            response_file:assets.path().join(format!("iterate_response_{request_id}.json")),published };
        atomic_json(&reg.request_file, &request).unwrap();
        atomic_json(&dir.path().join(if published {"requests"} else {"deferred-requests"}).join(format!("{}.json",reg.key)), &reg).unwrap();
        atomic_json(&dir.path().join("leases").join(&reg.key), &json!("active")).unwrap();
        atomic_json(&assets.path().join(format!("iterate_ready_{request_id}.json")), &json!({
            "request_id":request_id,"project_path":r"E:\Github\iterate-desktop","ready_at":"2026-10-02T04:46:56.472Z"})).unwrap();
        let window = crate::ui::window_registry::WindowInstance {pid:std::process::id(),
            project_path:r"E:\Github\iterate-desktop".into(),window_title:"unit source".into(),
            registered_at:"2026-10-02T04:46:56.658229200Z".into(),port:None,request_id:Some(request_id.into()),request_title:None};
        (dir,assets,reg,window)
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn ready_registered_recovery_reads_both_layouts_without_response_route_or_writes() {
        for published in [false,true] {
            let (dir,assets,reg,window) = registered_recovery_fixture(published);
            let request_id = reg.request["id"].as_str().unwrap();
            let payload = load_ready_local_registration_from_dir(dir.path(),assets.path(),request_id,&window.project_path,&[window.clone()]).unwrap();
            assert_eq!(payload["request"],reg.request);
            // Source 57 observed ready before syncWindowRegistration completed.
            assert!(chrono::DateTime::parse_from_rfc3339("2026-10-02T04:46:56.472Z").unwrap()
                < chrono::DateTime::parse_from_rfc3339(&window.registered_at).unwrap());
            assert_eq!(payload["cache_source"],"registered_local_source");
            assert_eq!(payload["showMcpPopup"],true);
            assert!(!assets.path().join(format!("iterate_response_route_{request_id}.json")).exists());
            assert!(!reg.response_file.exists());
            for absent in ["settings.json","locks","results"] { assert!(!dir.path().join(absent).exists()); }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn registered_recovery_accepts_captured_ready_null_or_absent_project_only() {
        // Source 75's actual ready JSON, captured before its natural phone reply.
        let captured_ready = json!({"project_path":null,"ready_at":"2026-10-02T06:10:00.626866+00:00",
            "request_id":"serve-1790921396371-a4d3b8f0-3c7b-45ed-bf04-e9ee8b612d2e"});
        for case in ["null","absent","matching","different","empty","number","boolean","array","object"] {
            let (dir,assets,mut reg,mut window) = registered_recovery_fixture(true);
            let request_id = captured_ready["request_id"].as_str().unwrap();
            reg.request["id"] = json!(request_id);
            reg.request_file = assets.path().join(format!("iterate_request_{request_id}.json"));
            reg.response_file = assets.path().join(format!("iterate_response_{request_id}.json"));
            window.request_id = Some(request_id.into());
            window.registered_at = "2026-10-02T06:10:00.909576200+00:00".into();
            atomic_json(&reg.request_file,&reg.request).unwrap();
            atomic_json(&dir.path().join("requests").join(format!("{}.json",reg.key)),&reg).unwrap();
            let mut ready = captured_ready.clone();
            match case {
                "absent" => { ready.as_object_mut().unwrap().remove("project_path"); },
                "matching" => ready["project_path"]=json!(r"E:\Github\iterate-desktop"),
                "different" => ready["project_path"]=json!("E:/Github/other"),
                "empty" => ready["project_path"]=json!(""),
                "number" => ready["project_path"]=json!(1),
                "boolean" => ready["project_path"]=json!(true),
                "array" => ready["project_path"]=json!([]),
                "object" => ready["project_path"]=json!({}),
                _ => {}
            }
            atomic_json(&assets.path().join(format!("iterate_ready_{request_id}.json")),&ready).unwrap();
            let result = load_ready_local_registration_from_dir(dir.path(),assets.path(),request_id,&window.project_path,std::slice::from_ref(&window));
            if matches!(case,"null"|"absent"|"matching") {
                assert_eq!(result.unwrap()["request"],reg.request,"{case}");
            } else {
                assert_eq!(result.unwrap_err(),"registered_ready_mismatch","{case}");
            }
            assert!(!reg.response_file.exists());
            assert!(!assets.path().join(format!("iterate_response_route_{request_id}.json")).exists());
            for absent in ["settings.json","locks","results"] { assert!(!dir.path().join(absent).exists()); }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn ready_registered_recovery_rejects_completed_stale_foreign_and_unready_sources() {
        for optional_ready_path in [false,true] {
        for case in ["result","response","expired_lease","missing_lease","no_live","wrong_window_id","wrong_window_project",
            "wrong_registration_id","wrong_registration_project","foreign_request_file","foreign_response_file","wrong_key",
            "request_changed","request_missing","not_ready","wrong_ready_id","wrong_ready_project","stale_ready","future_ready"] {
            let (dir,assets,mut reg,mut window) = registered_recovery_fixture(false);
            let request_id = reg.request["id"].as_str().unwrap().to_owned();
            let registration_file = dir.path().join("deferred-requests").join(format!("{}.json",reg.key));
            let ready_file = assets.path().join(format!("iterate_ready_{request_id}.json"));
            if optional_ready_path {
                let mut ready:Value = read_json(&ready_file).unwrap();
                ready["project_path"] = Value::Null;
                atomic_json(&ready_file,&ready).unwrap();
            }
            match case {
                "result" => atomic_json(&dir.path().join("results").join(format!("{}.json",reg.key)), &json!({"cancelled":true})).unwrap(),
                "response" => atomic_json(&reg.response_file,&json!({"user_input":"already consumed"})).unwrap(),
                "expired_lease" => File::options().write(true).open(dir.path().join("leases").join(&reg.key)).unwrap()
                    .set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::now()-Duration::from_secs(16))).unwrap(),
                "missing_lease" => fs::remove_file(dir.path().join("leases").join(&reg.key)).unwrap(),
                "wrong_window_id" => window.request_id = Some("other-request".into()),
                "wrong_window_project" => window.project_path = "E:/Github/other".into(),
                "wrong_registration_id" => reg.request["id"] = json!("other-request"),
                "wrong_registration_project" => reg.request["project_path"] = json!("E:/Github/other"),
                "foreign_request_file" => reg.request_file = dir.path().join("foreign.json"),
                "foreign_response_file" => reg.response_file = dir.path().join("foreign-response.json"),
                "wrong_key" => reg.key = uuid::Uuid::new_v4().to_string(),
                "request_changed" => { let mut changed = reg.request.clone(); changed["message"] = json!("different request"); atomic_json(&reg.request_file,&changed).unwrap(); },
                "request_missing" => fs::remove_file(&reg.request_file).unwrap(),
                "not_ready" => fs::remove_file(&ready_file).unwrap(),
                "wrong_ready_id" | "wrong_ready_project" | "stale_ready" | "future_ready" => {
                    let mut ready:Value = read_json(&ready_file).unwrap();
                    match case { "wrong_ready_id" => ready["request_id"]=json!("other-request"),
                        "wrong_ready_project" => ready["project_path"]=json!("E:/Github/other"),
                        "future_ready" => ready["ready_at"]=json!((chrono::Utc::now()+chrono::Duration::seconds(1)).to_rfc3339()),
                        _ => ready["ready_at"]=json!("2026-10-02T04:40:00Z") }
                    atomic_json(&ready_file,&ready).unwrap();
                },
                _ => {}
            }
            atomic_json(&registration_file,&reg).unwrap();
            let windows = if case=="no_live" {vec![]} else {vec![window]};
            assert!(load_ready_local_registration_from_dir(dir.path(),assets.path(),&request_id,r"E:\Github\iterate-desktop",&windows).is_err(),"{case}");
            assert!(!assets.path().join(format!("iterate_response_route_{request_id}.json")).exists());
        }
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn registered_recovery_rejects_oversized_and_nonfile_assets() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_registration_recovery_json(dir.path()).is_err());
        let oversized = dir.path().join("large.json");
        fs::write(&oversized,vec![b' ';1024*1024+1]).unwrap();
        assert!(read_registration_recovery_json(&oversized).is_err());
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn registered_recovery_does_not_initialize_missing_config_directory() {
        let root = tempfile::tempdir().unwrap();
        let missing_config = root.path().join("missing-config");
        let prior_cross = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let prior_config = std::env::var_os("ITERATE_CONFIG_DIR");
        std::env::remove_var("ITERATE_CROSS_DEVICE_DIR");
        std::env::set_var("ITERATE_CONFIG_DIR",&missing_config);
        assert!(load_ready_local_registration_for_mobile(
            "serve-1790916412343-6186ba51-bf52-4ca9-9c5c-b3835cca2e34","E:/Github/iterate-desktop",&[],
        ).is_err());
        assert!(!missing_config.exists());
        match prior_cross {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_DIR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_DIR")}
        match prior_config {Some(value)=>std::env::set_var("ITERATE_CONFIG_DIR",value),None=>std::env::remove_var("ITERATE_CONFIG_DIR")}
    }

    #[test]
    fn old_mobile_mirror_binding_keeps_unknown_computer_origin() {
        let value = json!({"key":"legacy", "revision":"paired", "request":{
            "origin_device_id":"forged-request", "origin_name":"forged title",
            "conversation_title":"来自 Mac"}});
        let binding: MobileMirrorBinding = serde_json::from_value(value).unwrap();
        assert_eq!(binding.origin_device_id, None);
        assert_eq!(binding.origin_name, None);
        assert_eq!(binding.origin_platform, None);
        assert_eq!(display_identity(Some(&Value::Null)), None);
        assert_eq!(display_identity(Some(&json!("null"))), Some("null".into()));
    }

    #[test]
    fn computer_platform_whitelist_keeps_old_or_misnamed_origins_unknown() {
        let legacy = json!({"key":"legacy", "origin_device_id":"windows-computer",
            "origin_name":"macos", "request":{"origin_platform":"macos"},
            "request_file":"request.json", "response_file":"response.json"});
        let old: Registration = serde_json::from_value(legacy.clone()).unwrap();
        assert_eq!(old.origin_platform, None);
        assert_eq!(old.request["origin_platform"], "macos");
        for unknown in [Value::Null, json!("Mac"), json!("windows-host"), json!("linux"),
            json!("null"), json!(12), json!(true), json!([]), json!({"os":"windows"})] {
            let mut registration = legacy.clone();
            registration["origin_platform"] = unknown.clone();
            assert_eq!(serde_json::from_value::<Registration>(registration).unwrap().origin_platform, None);
            let binding: MobileMirrorBinding = serde_json::from_value(json!({"key":"legacy",
                "revision":"paired", "request":{"origin_platform":"macos"},
                "origin_name":"windows", "origin_platform":unknown})).unwrap();
            assert_eq!(binding.origin_platform, None);
        }
        for platform in ["windows", "macos"] {
            let mut registration = legacy.clone();
            registration["origin_platform"] = json!(platform);
            let parsed: Registration = serde_json::from_value(registration).unwrap();
            assert_eq!(parsed.origin_platform.as_deref(), Some(platform));
            assert_eq!(serde_json::to_value(parsed).unwrap()["origin_platform"], platform);
        }
        assert_eq!(local_computer_platform(), match std::env::consts::OS {
            "windows" => Some("windows"), "macos" => Some("macos"), _ => None,
        });
    }

    #[test]
    fn mirror_title_keeps_only_true_source_title_or_neutral_fallback() {
        assert_eq!(mirror_conversation_title(&json!({"conversation_title":"Windows 是用户的真实标题"})), "Windows 是用户的真实标题");
        assert_eq!(mirror_conversation_title(&json!({"conversation_title":"  原始会话标题  ","origin_name":"macos"})), "  原始会话标题  ");
        assert_eq!(mirror_conversation_title(&json!({"conversation_title":"null"})), "null");
        for source in [json!({}),json!({"conversation_title":null}),json!({"conversation_title":"  "})] {
            assert_eq!(mirror_conversation_title(&source), "未命名会话");
        }
    }

    #[test]
    fn mirror_display_refresh_requires_exact_live_binding_and_true_source_title() {
        let prior = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let (dir, binding, _) = mobile_mirror_fixture();
        let path = dir.path().join("mobile-mirrors").join(format!("{}.json", binding.key));
        let original = binding.request.clone();
        let registration = json!({"key":binding.key,"origin_device_id":"source-real",
            "origin_name":"windows misleading name","origin_platform":"macos",
            "request":{"conversation_title":"  真正的会话标题  ","message":"must not replace body"}});
        refresh_mobile_mirror_display(dir.path(), "other-pair", &registration).unwrap();
        assert_eq!(read_json::<MobileMirrorBinding>(&path).unwrap().request, original);
        let mut without_source = registration.clone();
        without_source.as_object_mut().unwrap().remove("request");
        refresh_mobile_mirror_display(dir.path(), &binding.revision, &without_source).unwrap();
        assert_eq!(read_json::<MobileMirrorBinding>(&path).unwrap().request, original);
        refresh_mobile_mirror_display(dir.path(), &binding.revision, &registration).unwrap();
        let refreshed = read_json::<MobileMirrorBinding>(&path).unwrap();
        assert_eq!(refreshed.request["conversation_title"], "  真正的会话标题  ");
        assert_eq!(refreshed.request["id"], original["id"]);
        assert_eq!(refreshed.request["message"], original["message"]);
        assert_eq!(refreshed.request["predefined_options"], original["predefined_options"]);
        assert_eq!(refreshed.origin_platform.as_deref(), Some("macos"));
        assert_eq!(refreshed.key, binding.key);
        assert_eq!(refreshed.revision, binding.revision);
        match prior { Some(value) => std::env::set_var("ITERATE_CROSS_DEVICE_DIR", value), None => std::env::remove_var("ITERATE_CROSS_DEVICE_DIR") }
    }

    #[test]
    fn local_computer_identity_is_display_only_and_does_not_create_settings() {
        let prior = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", dir.path());
        assert_eq!(local_computer_identity(), (None, None));
        assert!(!dir.path().join("settings.json").exists());
        atomic_json(&dir.path().join("settings.json"), &Settings { device_id: "computer-uuid".into(), enabled: false }).unwrap();
        let mut config = transport::ConnectionConfig::default();
        config.device_name = "电脑 A".into();
        atomic_json(&dir.path().join("direct-connection.json"), &config).unwrap();
        assert_eq!(local_computer_identity(), (Some("computer-uuid".into()), Some("电脑 A".into())));
        config.device_name = "电脑 B".into();
        atomic_json(&dir.path().join("direct-connection.json"), &config).unwrap();
        assert_eq!(local_computer_identity().1.as_deref(), Some("电脑 B"));
        assert!(!dir.path().join("credentials.json").exists());
        match prior { Some(value) => std::env::set_var("ITERATE_CROSS_DEVICE_DIR", value), None => std::env::remove_var("ITERATE_CROSS_DEVICE_DIR") }
    }

    fn mobile_mirror_fixture() -> (tempfile::TempDir, MobileMirrorBinding, String) {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", dir.path());
        atomic_json(&dir.path().join("settings.json"), &Settings { device_id: "win-test".into(), enabled: true }).unwrap();
        atomic_json(&dir.path().join("direct-connection.json"), &transport::ConnectionConfig::default()).unwrap();
        let key = uuid::Uuid::new_v4().to_string();
        let revision = transport::route_key().unwrap();
        let binding = MobileMirrorBinding { key: key.clone(), revision: revision.clone(),
            origin_device_id: Some("mac-source-id".into()), origin_name: Some("Mac source".into()), origin_platform: Some("macos".into()),
            request: json!({"id":format!("cross-{key}"),"message":"Mac 真实协议的文本契约测试",
                "predefined_options":["保留选项 原文"],"conversation_title":"来自Mac"}) };
        atomic_json(&dir.path().join("mobile-mirrors").join(format!("{key}.json")), &binding).unwrap();
        atomic_json(&dir.path().join("mirrors").join(format!("{key}.json")), &binding.request).unwrap();
        atomic_json(&dir.path().join("mirrors").join(format!("{key}.ready")), &json!(true)).unwrap();
        atomic_json(&dir.path().join("mirror-leases").join(&key), &json!(true)).unwrap();
        let id = mobile_mirror_id(&key, &revision);
        (dir, binding, id)
    }

    #[tokio::test]
    async fn mobile_mirror_identity_leases_and_local_close_preserve_source_boundary() {
        let prior = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let (dir, binding, id) = mobile_mirror_fixture();
        let states = mobile_mirror_states().await;
        assert_eq!(states.len(), 1);
        assert_eq!(states[0]["mirror_id"], id);
        assert_eq!(states[0]["origin_device_id"], "mac-source-id");
        assert_eq!(states[0]["origin_name"], "Mac source");
        assert_eq!(states[0]["origin_platform"], "macos");
        assert!(states[0]["request"].get("project_path").is_none());
        let close = json!({"mirror_id":id,"request_id":id,"client_action_id":"close-1","action":"close_mirror"});
        assert_eq!(submit_mobile_mirror_action(&close).await.unwrap(), "mirror_closed");
        assert!(dir.path().join("dismissed").join(&binding.key).is_file());
        // The fixture has no TLS credentials or Mac server. Close must stay local.
        assert!(!dir.path().join("results").exists());
        assert_eq!(submit_mobile_mirror_action(&close).await.unwrap(), "mirror_closed");
        assert!(mobile_mirror_states().await.is_empty());
        let different = json!({"mirror_id":id,"request_id":id,"client_action_id":"close-2","action":"close_mirror"});
        assert!(submit_mobile_mirror_action(&different).await.unwrap_err().contains("conflict"));
        let mut config = transport::ConnectionConfig::default();
        config.device_name = "changed pairing revision".into();
        atomic_json(&dir.path().join("direct-connection.json"), &config).unwrap();
        assert_eq!(submit_mobile_mirror_action(&close).await.unwrap_err(), "mirror_not_registered");
        if let Some(prior) = prior { std::env::set_var("ITERATE_CROSS_DEVICE_DIR", prior); }
        else { std::env::remove_var("ITERATE_CROSS_DEVICE_DIR"); }
    }

    #[tokio::test]
    async fn mobile_mirror_expired_lease_rejects_new_action_but_exact_accepted_retry_is_safe() {
        let prior = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let (dir, binding, id) = mobile_mirror_fixture();
        let payload = json!({"mirror_id":id,"request_id":id,"client_action_id":"reply-1","action":"submit",
            "user_input":"原样回复", "selected_options":["保留选项 原文"]});
        let lease = OpenOptions::new().write(true).open(dir.path().join("mirror-leases").join(&binding.key)).unwrap();
        lease.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now() - Duration::from_secs(16))).unwrap();
        assert_eq!(submit_mobile_mirror_action(&payload).await.unwrap_err(), "mirror_not_active");
        let response = mobile_mirror_response(&payload, &binding).unwrap();
        atomic_json(&dir.path().join("mobile-mirror-receipts").join(format!("{}.json", id.strip_prefix("cross-mirror:").unwrap())),
            &json!({"client_action_id":"reply-1","action":"submit","response":response,"status":"mac_accepted"})).unwrap();
        assert_eq!(submit_mobile_mirror_action(&payload).await.unwrap(), "mac_accepted");
        let mut conflict = payload.clone();
        conflict["user_input"] = json!("不同内容");
        assert_eq!(submit_mobile_mirror_action(&conflict).await.unwrap_err(), "mirror_previous_reply_conflict");
        let mut wrong_path = payload.clone();
        wrong_path["project_path"] = json!("C:/unrelated");
        assert_eq!(submit_mobile_mirror_action(&wrong_path).await.unwrap_err(), "mirror_text_only");
        if let Some(prior) = prior { std::env::set_var("ITERATE_CROSS_DEVICE_DIR", prior); }
        else { std::env::remove_var("ITERATE_CROSS_DEVICE_DIR"); }
    }

    #[test]
    fn mobile_mirror_reply_only_accepts_announced_text_options() {
        let binding = MobileMirrorBinding { key: uuid::Uuid::new_v4().to_string(), revision: "pair".into(),
            origin_device_id: None, origin_name: None, origin_platform: None,
            request: json!({"predefined_options":["  原始选项  "]}) };
        let response = mobile_mirror_response(&json!({"user_input":"  原始文本  ","selected_options":["  原始选项  "]}), &binding).unwrap();
        assert_eq!(response["user_input"], "  原始文本  ");
        assert!(response.get("project_path").is_none());
        assert!(mobile_mirror_response(&json!({"selected_options":["未知选项"]}), &binding).is_err());
    }

    #[test]
    fn registered_reply_route_requires_caller_provenance_and_stable_event() {
        let mut reg = Registration {
            key: "b650955f-8e4b-43a7-b1dd-5403e9f71347".into(),
            origin_device_id: "device".into(),
            origin_name: "source".into(),
            origin_platform: None,
            request: json!({"id":"serve-1","codex_thread_id":"thread-1",
                "codex_thread_provenance":"caller_meta"}),
            request_file: PathBuf::new(),
            response_file: PathBuf::new(),
            published: false,
        };
        assert_eq!(reg.trusted_timeline_route().as_deref(), Some("thread-1"));
        assert_eq!(reg.ordinary_timeline_route(), "thread-1");
        assert_eq!(reg.response_event_id(),
            "cross-device-reply:b650955f-8e4b-43a7-b1dd-5403e9f71347");
        reg.request["codex_thread_provenance"] = json!("explicit_argument");
        assert_eq!(reg.trusted_timeline_route(), None);
        assert_eq!(reg.ordinary_timeline_route(), "thread-1");
        reg.request["codex_thread_provenance"] = Value::Null;
        assert_eq!(reg.trusted_timeline_route(), None);
        reg.request["codex_thread_provenance"] = json!("caller_meta");
        reg.request["codex_thread_id"] = json!(" ");
        assert_eq!(reg.trusted_timeline_route(), None);
        assert_eq!(reg.ordinary_timeline_route(), reg.request["id"].as_str().unwrap());
    }

    #[tokio::test]
    async fn legacy_cross_device_gui_and_serve_record_same_event_in_one_tree() {
        let temp = tempfile::tempdir().unwrap();
        let prior_dir = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let prior_state = std::env::var_os("ITERATE_CONVERSATION_STATE_FILE");
        let prior_source = std::env::var_os("ITERATE_CROSS_DEVICE_SOURCE");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", temp.path());
        std::env::set_var("ITERATE_CONVERSATION_STATE_FILE", temp.path().join("state.json"));
        let request_file = temp.path().join("request.json");
        fs::write(&request_file, b"{}").unwrap();
        let reg = Registration {
            key: uuid::Uuid::new_v4().to_string(),
            origin_device_id: "device".into(), origin_name: "source".into(), origin_platform: None,
            request: json!({"id":"serve-1","project_path":temp.path().to_string_lossy(),
                "codex_thread_id":"thread-1"}),
            request_file, response_file: temp.path().join("response.json"), published: true,
        };
        atomic_json(&reg.path().unwrap(), &reg).unwrap();
        reg.renew();
        assert_eq!(registered_caller_thread("serve-1",
            reg.request.get("project_path").and_then(Value::as_str).unwrap()),
            None);
        assert_eq!(reg.ordinary_timeline_route(), "thread-1");
        let response = json!({"selected_options":["A"],"user_input":"追加正文",
            "metadata":{"source":"android_cross_device"}});
        commit(&reg, &response, true).unwrap();
        std::env::set_var("ITERATE_CROSS_DEVICE_SOURCE", &reg.key);
        assert_eq!(source_reply_identity(&response),
            Some((reg.response_event_id(), "thread-1".into())));
        let mut gui_response = response.clone();
        gui_response["metadata"]["run_id"] = json!(reg.response_event_id());
        let gui_manager = crate::conversation::ConversationManager::new_with_forced_persistence();
        crate::ui::commands::record_user_response_node(
            None, &gui_manager, &gui_response,
            Some(temp.path().to_string_lossy().to_string()),
            Some("serve-1".into()), Some(reg.ordinary_timeline_route()),
            "send_mcp_response",
        ).await.unwrap();
        record_accepted_response(&reg, &response).await.unwrap();
        record_accepted_response(&reg, &response).await.unwrap();
        let manager = crate::conversation::ConversationManager::new_with_forced_persistence();
        let tree = manager.get_tree_for_route(Some("thread-1"),
            reg.request.get("project_path").and_then(Value::as_str)).await.unwrap();
        let current = manager.get_current_node_id(&tree).await.unwrap();
        let nodes = manager.get_node_path(&tree, &current).await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].content, "选中的选项: A\n\n追加正文");
        assert_eq!(nodes[0].metadata.run_id.as_deref(), Some(reg.response_event_id().as_str()));
        match prior_dir { Some(v) => std::env::set_var("ITERATE_CROSS_DEVICE_DIR", v),
            None => std::env::remove_var("ITERATE_CROSS_DEVICE_DIR") }
        match prior_state { Some(v) => std::env::set_var("ITERATE_CONVERSATION_STATE_FILE", v),
            None => std::env::remove_var("ITERATE_CONVERSATION_STATE_FILE") }
        match prior_source { Some(v) => std::env::set_var("ITERATE_CROSS_DEVICE_SOURCE", v),
            None => std::env::remove_var("ITERATE_CROSS_DEVICE_SOURCE") }
    }
    #[test]
    fn registration_lookup_retries_publication_that_races_deferred_read() {
        let temp = tempfile::tempdir().unwrap();
        let published = temp.path().join("requests/request.json");
        let deferred = temp.path().join("deferred-requests/request.json");
        let mut reg = Registration {
            key: uuid::Uuid::new_v4().to_string(), origin_device_id: "source".into(),
            origin_name: "source".into(), origin_platform: None, request: json!({"message":"pending"}),
            request_file: temp.path().join("popup.json"), response_file: temp.path().join("response.json"),
            published: false,
        };
        atomic_json(&deferred, &reg).unwrap();
        // Deterministically complete publication after the destination lookup
        // misses, but before the deferred read. No timing or thread sleeps.
        let loaded = load_registration_files(&published, || {
            reg.published = true;
            atomic_json(&published, &reg).unwrap();
            fs::remove_file(&deferred).unwrap();
            read_json(&deferred)
        }).unwrap();
        assert_eq!(loaded.key, reg.key);
        assert!(loaded.published);
        assert_eq!(loaded.response_file, reg.response_file);
    }

    #[test]
    fn sync_capability_must_be_explicit() {
        assert!(!supports_window_sync(&json!({"version":2,"config_revision":"same"})));
        assert!(!supports_window_sync(&json!({"capabilities":["other"]})));
        assert!(supports_window_sync(&json!({"capabilities":[WINDOW_SYNC_CAPABILITY]})));
    }

    #[test]
    fn sync_restores_only_the_dismissal_captured_before_publication() {
        let temp = tempfile::tempdir().unwrap();
        let key = uuid::Uuid::new_v4().to_string();
        let keys = HashSet::from([key.clone()]);
        let mut dismissed = keys.clone();
        // A close during POST, before the queue starts, must never reopen.
        let before_close = capture_dismissals(temp.path());
        record_dismissal(temp.path(), &key).unwrap();
        restore_dismissals(temp.path(), &keys, &before_close, &mut dismissed);
        assert!(dismissed.contains(&key));
        // A normal close is persisted by the popup before daemon reaping.
        let before_reap = capture_dismissals(temp.path());
        record_dismissal(temp.path(), &key).unwrap();
        assert_eq!(before_reap, capture_dismissals(temp.path()));
        restore_dismissals(temp.path(), &keys, &before_reap, &mut dismissed);
        assert!(!dismissed.contains(&key));
        // A second close cannot be cleared by reusing the previous batch.
        record_dismissal(temp.path(), &key).unwrap();
        dismissed.insert(key.clone());
        restore_dismissals(temp.path(), &keys, &before_reap, &mut dismissed);
        assert!(dismissed.contains(&key));
        assert!(temp.path().join("dismissed").join(&key).exists());
    }

    #[tokio::test]
    async fn offline_windows_are_published_only_by_authenticated_pulls() {
        let temp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", temp.path());
        let local = Settings { device_id: "source".into(), enabled: true };
        atomic_json(&temp.path().join("settings.json"), &local).unwrap();
        let state = Arc::new(Broker { token: "test-token".into(), revision: transport::route_key().unwrap(),
            device_id: local.device_id.clone(), network_ready: std::sync::atomic::AtomicBool::new(true) });
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-token".parse().unwrap());
        let request_file = temp.path().join("request.json");
        fs::write(&request_file, b"{}").unwrap();
        let first = Registration { key: uuid::Uuid::new_v4().to_string(), origin_device_id: local.device_id.clone(),
            origin_name: "source".into(), origin_platform: None, request: json!({"message":"offline request"}), request_file,
            response_file: temp.path().join("response.json"), published: false };
        atomic_json(&first.path().unwrap(), &first).unwrap();
        first.renew();
        assert!(snapshot(State(state.clone()), headers.clone()).await.unwrap().0["requests"].as_array().unwrap().is_empty());
        assert_eq!(sync_open_windows(State(state.clone()), HeaderMap::new()).await.unwrap_err(), StatusCode::UNAUTHORIZED);
        assert!(!load_registration(&first.key).unwrap().published);
        // An old daemon scans only requests and ignores the published field.
        assert!(!temp.path().join("requests").join(format!("{}.json", first.key)).exists());
        let pulled = sync_open_windows(State(state.clone()), headers.clone()).await.unwrap().0;
        assert_eq!(pulled["requests"].as_array().unwrap().len(), 1);
        assert!(load_registration(&first.key).unwrap().published);
        assert!(!first.path().unwrap().exists());

        // A later offline window is not swept into a previous one-shot pull.
        let second = Registration { key: uuid::Uuid::new_v4().to_string(), ..first.clone() };
        atomic_json(&second.path().unwrap(), &second).unwrap();
        second.renew();
        assert_eq!(snapshot(State(state.clone()), headers.clone()).await.unwrap().0["requests"].as_array().unwrap().len(), 1);
        assert!(!load_registration(&second.key).unwrap().published);
        atomic_json(&temp.path().join("settings.json"), &Settings { enabled: false, ..local.clone() }).unwrap();
        assert_eq!(sync_open_windows(State(state.clone()), headers.clone()).await.unwrap_err(), StatusCode::CONFLICT);
        assert!(!load_registration(&second.key).unwrap().published);
        atomic_json(&temp.path().join("settings.json"), &local).unwrap();
        for _ in 0..2 {
            assert_eq!(sync_open_windows(State(state.clone()), headers.clone()).await.unwrap().0["requests"].as_array().unwrap().len(), 2);
        }
        // Preserve source registrations made by earlier feature builds.
        let legacy = Registration { key: uuid::Uuid::new_v4().to_string(), ..first.clone() };
        atomic_json(&temp.path().join("requests").join(format!("{}.json", legacy.key)), &legacy).unwrap();
        legacy.renew();
        assert_eq!(sync_open_windows(State(state.clone()), headers.clone()).await.unwrap().0["requests"].as_array().unwrap().len(), 3);
        assert!(load_registration(&legacy.key).unwrap().published);
        // A source response can precede the result receipt; neither an automatic
        // nor a manual pull may publish that already answered source again.
        let answered = Registration { key: uuid::Uuid::new_v4().to_string(),
            response_file: temp.path().join("answered-response.json"), ..first.clone() };
        atomic_json(&answered.path().unwrap(), &answered).unwrap();
        answered.renew();
        atomic_json(&answered.response_file, &json!({"user_input":"already answered"})).unwrap();
        assert_eq!(sync_open_windows(State(state.clone()), headers.clone()).await.unwrap().0["requests"].as_array().unwrap().len(), 3);
        assert!(!load_registration(&answered.key).unwrap().published);
        let answered_published = Registration { key: uuid::Uuid::new_v4().to_string(), published: true, ..answered.clone() };
        atomic_json(&answered_published.path().unwrap(), &answered_published).unwrap();
        answered_published.renew();
        assert_eq!(snapshot(State(state.clone()), headers.clone()).await.unwrap().0["requests"].as_array().unwrap().len(), 3);
        legacy.finish();
        first.finish();
        assert_eq!(sync_open_windows(State(state), headers).await.unwrap().0["requests"].as_array().unwrap().len(), 1);
        match previous {
            Some(value) => std::env::set_var("ITERATE_CROSS_DEVICE_DIR", value),
            None => std::env::remove_var("ITERATE_CROSS_DEVICE_DIR"),
        }
    }

    #[tokio::test]
    async fn local_health_requires_authentication_and_a_network_listener() {
        let state = Arc::new(Broker { token: "test-token".into(), revision: "test-revision".into(),
            device_id: "test-device".into(), network_ready: std::sync::atomic::AtomicBool::new(false) });
        assert_eq!(health(State(state.clone()), HeaderMap::new()).await.unwrap_err(), StatusCode::UNAUTHORIZED);
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-token".parse().unwrap());
        assert_eq!(health(State(state.clone()), headers.clone()).await.unwrap_err(), StatusCode::SERVICE_UNAVAILABLE);
        state.network_ready.store(true, std::sync::atomic::Ordering::Release);
        let value = health(State(state), headers).await.unwrap().0;
        assert_eq!(value["config_revision"], "test-revision");
        assert_eq!(value["device_id"], "test-device");
        assert!(value.get("requests").is_none());
    }
    #[test]
    fn first_submit_is_atomic_retryable_and_cancel_is_final() {
        let temp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", temp.path());
        let request_file = temp.path().join("request.json");
        fs::write(&request_file, b"{}").unwrap();
        let reg = Registration {
            key: uuid::Uuid::new_v4().to_string(),
            origin_device_id: "origin".into(),
            origin_name: "test".into(),
            origin_platform: None,
            request: json!({}),
            request_file,
            response_file: temp.path().join("response.json"),
            published: true,
        };
        reg.renew();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = (0..2)
            .map(|i| {
                let reg = reg.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    let response = json!({"user_input":format!("winner-{i}")});
                    (commit(&reg, &response, true), response)
                })
            })
            .collect();
        barrier.wait();
        let attempts: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(attempts.iter().filter(|(r, _)| r.is_ok()).count(), 1);
        let winner = &attempts.iter().find(|(r, _)| r.is_ok()).unwrap().1;
        assert!(
            reg.pending(),
            "accepted replies still block pairing changes until source delivery"
        );
        assert!(!reg.active());
        assert_eq!(
            read_json::<Value>(&reg.result_path().unwrap())
                .unwrap()
                .get("response"),
            Some(winner)
        );
        assert!(
            !reg.response_file.exists(),
            "only the source GUI may publish after recording"
        );
        assert!(
            commit(&reg, winner, true).is_ok(),
            "lost ACK retry must succeed"
        );
        reg.finish();
        assert!(!reg.active());
        let cancelled = Registration {
            key: uuid::Uuid::new_v4().to_string(),
            response_file: temp.path().join("cancelled-response.json"),
            ..reg.clone()
        };
        cancelled.renew();
        cancelled.finish();
        assert!(commit(&cancelled, &json!({"user_input":"late"}), true).is_err());
        assert!(!cancelled.response_file.exists());
        let exiting = Registration {
            key: uuid::Uuid::new_v4().to_string(),
            response_file: temp.path().join("exit-race.json"),
            ..reg.clone()
        };
        exiting.renew();
        let exit_barrier = Arc::new(std::sync::Barrier::new(2));
        let peer_reg = exiting.clone();
        let peer_barrier = exit_barrier.clone();
        let peer_submit = std::thread::spawn(move || {
            peer_barrier.wait();
            commit(&peer_reg, &json!({"user_input":"exit-race"}), true)
        });
        exit_barrier.wait();
        exiting.recover_accepted_response();
        let accepted = peer_submit.join().unwrap().is_ok();
        assert_eq!(
            accepted,
            exiting.response_file.exists(),
            "an accepted peer reply must survive source exit"
        );
        match previous {
            Some(value) => std::env::set_var("ITERATE_CROSS_DEVICE_DIR", value),
            None => std::env::remove_var("ITERATE_CROSS_DEVICE_DIR"),
        }
    }

    #[tokio::test]
    async fn cloud_direct_sync_respects_freeze_and_text_only_boundaries() {
        let dir=tempfile::tempdir().unwrap();
        let previous=std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR",dir.path());
        atomic_json(&dir.path().join("settings.json"),&Settings{device_id:"desktop-source".into(),enabled:true}).unwrap();
        atomic_json(&dir.path().join("hub-connection.json"),&hub_transport::HubConfig{endpoint:"http://127.0.0.1:18555".into(),device_id:"desktop-source".into(),token_env:"TEST_HUB_TOKEN".into(),ca_certificate:None}).unwrap();
        let state=Arc::new(Broker{token:"paired-token".into(),revision:transport::route_key().unwrap(),device_id:"desktop-source".into(),network_ready:std::sync::atomic::AtomicBool::new(true)});
        let mut headers=HeaderMap::new();headers.insert("authorization","Bearer paired-token".parse().unwrap());
        let make_request=|request:Value,published:bool| {
            let key=uuid::Uuid::new_v4().to_string();let request_file=dir.path().join(format!("{key}-request.json"));
            atomic_json(&request_file,&request).unwrap();
            let reg=Registration{key,origin_device_id:"desktop-source".into(),origin_name:"Mac".into(),origin_platform:Some("macos".into()),request,request_file,response_file:dir.path().join(format!("{}-response.json",uuid::Uuid::new_v4())),published};
            atomic_json(&reg.path().unwrap(),&reg).unwrap();reg.renew();reg
        };
        let text=make_request(json!({"id":"text","message":"pending text"}),false);
        let images=[
            make_request(json!({"id":"image","images":["private.png"]}),false),
            make_request(json!({"id":"markdown","message":"![image](private.png)"}),false),
            make_request(json!({"id":"html","message":"<img src='private.png'>"}),true),
        ];
        for drain in [false,true] {
            atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":true,"drain":drain})).unwrap();
            assert_eq!(sync_open_windows(State(state.clone()),headers.clone()).await.unwrap_err(),StatusCode::CONFLICT);
            assert!(!load_registration(&text.key).unwrap().published);
            // Read-only snapshots stay available; unsafe image windows stay local.
            assert!(snapshot(State(state.clone()),headers.clone()).await.unwrap().0["requests"].as_array().unwrap().is_empty());
        }
        atomic_json(&dir.path().join("hub-control.json"),&json!({"frozen":false,"drain":false})).unwrap();
        let synced=sync_open_windows(State(state),headers).await.unwrap().0;
        assert_eq!(synced["requests"].as_array().unwrap().len(),1);assert_eq!(synced["requests"][0]["key"],text.key);
        assert!(load_registration(&text.key).unwrap().published);
        for reg in &images {assert_eq!(load_registration(&reg.key).unwrap().published,reg.published);assert!(!reg.response_file.exists());}
        match previous {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_DIR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_DIR")}
    }

    #[tokio::test]
    async fn automatic_pulls_retry_and_preserve_dismissals_and_pairing_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", temp.path());
        let local = Settings { device_id: "source".into(), enabled: true };
        atomic_json(&temp.path().join("settings.json"), &local).unwrap();
        let revision = transport::route_key().unwrap();
        let state = Arc::new(Broker { token: "test-token".into(), revision: revision.clone(),
            device_id: local.device_id.clone(), network_ready: std::sync::atomic::AtomicBool::new(true) });
        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer test-token".parse().unwrap());
        let request_file = temp.path().join("request.json");
        fs::write(&request_file, b"{}").unwrap();
        let deferred = Registration { key: uuid::Uuid::new_v4().to_string(), origin_device_id: local.device_id.clone(),
            origin_name: "source".into(), origin_platform: Some("macos".into()),
            request: json!({"message":"opened while Windows was offline"}), request_file,
            response_file: temp.path().join("response.json"), published: false };
        atomic_json(&deferred.path().unwrap(), &deferred).unwrap();
        deferred.renew();
        record_dismissal(temp.path(), &deferred.key).unwrap();
        let marker = capture_dismissals(temp.path());
        let before = snapshot(State(state.clone()), headers.clone()).await.unwrap().0;
        assert!(before["requests"].as_array().unwrap().is_empty());
        assert!(automatic_window_snapshot(Some(&revision), before.clone(), async {
            Err("temporary network outage".into())
        }).await.is_err());
        assert!(!load_registration(&deferred.key).unwrap().published);
        let pulled = automatic_window_snapshot(Some(&revision), before, async {
            sync_open_windows(State(state.clone()), headers.clone()).await
                .map(|value| value.0).map_err(|error| error.to_string())
        }).await.unwrap();
        assert_eq!(pulled["requests"][0]["key"], deferred.key);
        assert!(load_registration(&deferred.key).unwrap().published);
        assert_eq!(capture_dismissals(temp.path()), marker);
        assert!(!temp.path().join("window-sync").exists());
        // Another deferred registration after the first successful pull must
        // be discovered without a new disconnect or an explicit sync click.
        let later = Registration { key: uuid::Uuid::new_v4().to_string(), ..deferred.clone() };
        atomic_json(&later.path().unwrap(), &later).unwrap();
        later.renew();
        let pulled = automatic_window_snapshot(Some(&revision), pulled, async {
            sync_open_windows(State(state.clone()), headers.clone()).await
                .map(|value| value.0).map_err(|error| error.to_string())
        }).await.unwrap();
        assert_eq!(pulled["requests"].as_array().unwrap().len(), 2);
        assert!(load_registration(&later.key).unwrap().published);
        assert_eq!(capture_dismissals(temp.path()), marker);

        // Disabled peers/local switches and older daemons stay on the read-only
        // path. A changed or absent local route must reject the poll entirely.
        for value in [json!({"enabled":false,"capabilities":[WINDOW_SYNC_CAPABILITY]}), json!({"enabled":true})] {
            assert_eq!(automatic_window_snapshot(Some(&revision), value.clone(), async {
                panic!("ineligible snapshot must not pull")
            }).await.unwrap(), value);
        }
        atomic_json(&temp.path().join("settings.json"), &Settings { enabled: false, ..local.clone() }).unwrap();
        assert_eq!(automatic_window_snapshot(Some(&revision), pulled.clone(), async {
            panic!("disabled local switch must not pull")
        }).await.unwrap(), pulled);
        atomic_json(&temp.path().join("settings.json"), &local).unwrap();
        for route in [None, Some("changed-route")] {
            assert!(automatic_window_snapshot(route, pulled.clone(), async {
                panic!("changed pairing must not pull")
            }).await.is_err());
        }
        let stale_state = Arc::new(Broker {
            token: "test-token".into(), revision: "changed-route".into(), device_id: local.device_id,
            network_ready: std::sync::atomic::AtomicBool::new(true),
        });
        assert_eq!(sync_open_windows(State(stale_state), headers).await.unwrap_err(), StatusCode::CONFLICT);
        match previous {
            Some(value) => std::env::set_var("ITERATE_CROSS_DEVICE_DIR", value),
            None => std::env::remove_var("ITERATE_CROSS_DEVICE_DIR"),
        }
    }

    #[tokio::test]
    async fn cloud_cold_registration_survives_unknown_and_frozen_control() {
        let temp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        let previous_mirror = std::env::var_os("ITERATE_CROSS_DEVICE_MIRROR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", temp.path());
        std::env::remove_var("ITERATE_CROSS_DEVICE_MIRROR");
        let config = hub_transport::HubConfig {
            endpoint: "http://127.0.0.1:18556".into(), device_id: "cold-source".into(),
            token_env: "TEST_HUB_TOKEN".into(), ca_certificate: None,
        };
        atomic_json(&temp.path().join("hub-connection.json"), &config).unwrap();
        atomic_json(&temp.path().join("settings.json"), &Settings {
            device_id: config.device_id.clone(), enabled: true,
        }).unwrap();
        // A healthy source alone must not admit work without a known control.
        atomic_json(&temp.path().join("hub-source-health.json"), &json!({
            "at": chrono::Utc::now().timestamp(),
        })).unwrap();
        let control = temp.path().join("hub-control.json");
        for state in [None, Some(json!({})), Some(json!({"frozen":"false"})),
            Some(json!({"frozen":true,"drain":false})), Some(json!({"frozen":true,"drain":true}))] {
            if let Some(state) = state { atomic_json(&control, &state).unwrap(); }
            else { let _ = fs::remove_file(&control); }
            let request = json!({"id": uuid::Uuid::new_v4().to_string(), "message":"pending text"});
            let request_file = temp.path().join(format!("{}-request.json", request["id"].as_str().unwrap()));
            atomic_json(&request_file, &request).unwrap();
            let reg = register(&request, &request_file, &temp.path().join("response.json")).await
                .expect("offline and frozen popups must retain their original registration");
            assert!(!reg.published);
            assert_eq!(load_registration(&reg.key).unwrap().request["id"], request["id"]);
            assert!(reg.path().unwrap().starts_with(temp.path().join("deferred-requests")));
            assert!(reg.active());
            assert!(hub_transport::submission_guard(false).is_err());
            assert!(commit(&reg, &json!({"user_input":"not admitted"}), true).is_err());
            assert!(!reg.response_file.exists());
            assert!(!reg.result_path().unwrap().exists());
        }
        atomic_json(&control, &json!({"frozen":false,"drain":false})).unwrap();
        let request_file = temp.path().join("online-request.json");
        let request = json!({"id":"online","message":"online text"});
        atomic_json(&request_file, &request).unwrap();
        assert!(register(&request, &request_file, &temp.path().join("online-response.json")).await.unwrap().published);
        match previous {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_DIR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_DIR")}
        match previous_mirror {Some(value)=>std::env::set_var("ITERATE_CROSS_DEVICE_MIRROR",value),None=>std::env::remove_var("ITERATE_CROSS_DEVICE_MIRROR")}
    }

    #[tokio::test]
    async fn android_deferred_source_requires_matching_consumed_winner() {
        let temp = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("ITERATE_CROSS_DEVICE_DIR");
        std::env::set_var("ITERATE_CROSS_DEVICE_DIR", temp.path());
        let request_id = format!("serve-test-{}", uuid::Uuid::new_v4());
        let project = temp.path().to_string_lossy().to_string();
        let request_file = temp.path().join("request.json");
        fs::write(&request_file, b"{}").unwrap();
        let reg = Registration {
            key: uuid::Uuid::new_v4().to_string(),
            origin_device_id: "origin".into(), origin_name: "test".into(), origin_platform: None,
            request: json!({"id":request_id,"project_path":project}),
            request_file, response_file: temp.path().join("response.json"), published: false,
        };
        atomic_json(&reg.path().unwrap(), &reg).unwrap();
        reg.renew();
        let payload = json!({"action":"submit","request_id":request_id,"project_path":project,
            "client_action_id":"android-action-1","user_input":"from phone"});
        assert!(android_registration(&request_id, "C:/wrong-project").is_err());
        let task = tokio::spawn({
            let payload = payload.clone(); let request_id = request_id.clone(); let project = project.clone();
            async move { submit_android_action(&payload, &request_id, &project).await }
        });
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while !reg.result_path().unwrap().exists() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let accepted = read_json::<Value>(&reg.result_path().unwrap()).unwrap()["response"].clone();
        assert!(mark_source_handed_off(&request_id, &project).is_err(),
            "HTTP handoff cannot confirm an unprepared reply");
        let previous_source = std::env::var_os("ITERATE_CROSS_DEVICE_SOURCE");
        std::env::set_var("ITERATE_CROSS_DEVICE_SOURCE", &reg.key);
        let source_status = get_cross_device_status().await;
        assert_eq!(source_status["source_pending"]["response"], accepted);
        match previous_source {
            Some(value) => std::env::set_var("ITERATE_CROSS_DEVICE_SOURCE", value),
            None => std::env::remove_var("ITERATE_CROSS_DEVICE_SOURCE"),
        }
        assert!(mark_source_prepared(&reg, &json!({"user_input":"other reply"})).is_err());
        assert!(!task.is_finished());
        let mut observed = accepted.clone();
        observed["metadata"]["conversation_id"] = json!("recorded-by-source");
        mark_source_prepared(&reg, &observed).unwrap();
        assert!(!task.is_finished(), "preparation alone cannot confirm delivery");
        mark_source_handed_off(&request_id, &project).unwrap();
        assert!(task.await.unwrap().unwrap().unwrap());
        assert_eq!(submit_android_action(&payload, &request_id, &project).await.unwrap().unwrap(), true);
        let other = json!({"action":"submit","client_action_id":"android-action-2","user_input":"other"});
        assert!(submit_android_action(&other, &request_id, &project).await.unwrap().is_err());
        match previous {
            Some(value) => std::env::set_var("ITERATE_CROSS_DEVICE_DIR", value),
            None => std::env::remove_var("ITERATE_CROSS_DEVICE_DIR"),
        }
    }
}
