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

#[derive(Clone, Serialize, Deserialize)]
pub struct Registration {
    key: String,
    origin_device_id: String,
    origin_name: String,
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
    pub fn recover_accepted_response(&self) {
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
    transport::validate_enable().ok()?;
    let route_key = transport::route_key().ok()?;
    let local = settings().ok()?;
    if std::env::var_os("ITERATE_CROSS_DEVICE_MIRROR").is_some() {
        return None;
    }
    let daemon_ready = transport::ensure_daemon().await.is_ok();
    let message = request.get("message").and_then(Value::as_str).unwrap_or("");
    if has_attachments(request) || message.contains("![") || message.contains("<img") {
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

fn load_registration_files(published: &Path, read_deferred: impl FnOnce() -> Result<Registration>) -> Result<Registration> {
    if published.exists() { read_json(&published) }
    // Publication writes the destination before removing the deferred file.
    // If that move raced our first lookup, the destination is now authoritative.
    else { read_deferred().or_else(|_| read_json(published)) }
}

// The same OS file lock is used by the local popup and peer HTTP handler.
// Atomic result persistence makes retry safe even when the ACK is lost.
fn commit(reg: &Registration, response: &Value, remote: bool) -> Result<()> {
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

fn response_contains(accepted: &Value, observed: &Value) -> bool {
    match (accepted, observed) {
        (Value::Object(a), Value::Object(b)) => a.iter().all(|(key, value)|
            b.get(key).is_some_and(|actual| response_contains(value, actual))),
        _ => accepted == observed,
    }
}

pub fn mark_source_prepared(reg: &Registration, response: &Value) -> Result<()> {
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
            .map(|response|json!({"response":response,"request_id":reg.request.get("id"),"project_path":reg.request.get("project_path")})));
    if source_pending.is_some() {
        return json!({"enabled":local.enabled,"device_id":local.device_id,"mirror":mirror,
            "source_pending":source_pending});
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
                if config.enabled && reg.published && reg.active() {
                    requests.push(
                        json!({"key":reg.key,"origin_device_id":reg.origin_device_id,
                    "origin_name":reg.origin_name,"request":reg.request}),
                    );
                }
            }
        }
    }
    Ok(Json(
        json!({"version":VERSION,"capabilities":[WINDOW_SYNC_CAPABILITY],"config_revision":state.revision,"enabled":config.enabled,"device_id":config.device_id,"device_name":connection_config().ok().map(|c|c.device_name),"requests":requests}),
    ))
}

// Only an explicit, authenticated pull publishes requests opened while offline.
async fn sync_open_windows(
    State(state): State<Arc<Broker>>,
    headers: HeaderMap,
) -> std::result::Result<Json<Value>, StatusCode> {
    if !authorized(&headers, &state) { return Err(StatusCode::UNAUTHORIZED); }
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
                    if let Ok(mut reg) = read_json::<Registration>(&entry.path()) {
                        if !reg.published && reg.active() {
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
        "conversation_title":format!("来自 {name} · {}",source.get("conversation_title").and_then(Value::as_str).unwrap_or("跨设备请求"))});
    let dir = directory()?.join("mirrors");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let request_file = dir.join(format!("{key}.json"));
    let ready_file = dir.join(format!("{key}.ready"));
    if ready_file.exists() {
        fs::remove_file(&ready_file).map_err(|e| e.to_string())?;
    }
    atomic_json(&request_file, &request)?;
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

async fn mirror_loop() {
    let mut mirrors: HashMap<String, Mirror> = HashMap::new();
    let mut dismissed: HashSet<String> = HashSet::new();
    loop {
        let poll_started = chrono::Utc::now().timestamp_millis();
        let route_key = transport::route_key().ok();
        if let Ok(snapshot) = peer("/snapshot", None).await {
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
                if !active.contains(key) {
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
    #[test]
    fn registration_lookup_retries_publication_that_races_deferred_read() {
        let temp = tempfile::tempdir().unwrap();
        let published = temp.path().join("requests/request.json");
        let deferred = temp.path().join("deferred-requests/request.json");
        let mut reg = Registration {
            key: uuid::Uuid::new_v4().to_string(), origin_device_id: "source".into(),
            origin_name: "source".into(), request: json!({"message":"pending"}),
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
    async fn offline_windows_are_published_only_by_each_explicit_pull() {
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
            origin_name: "source".into(), request: json!({"message":"offline request"}), request_file,
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
            origin_device_id: "origin".into(), origin_name: "test".into(),
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
