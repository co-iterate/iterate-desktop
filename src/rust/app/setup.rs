use crate::bridge::start_bridge_server;
use crate::config::{load_config_and_apply_window_settings, AppState};
use crate::ipc::start_ipc_server;
use crate::log_important;
#[cfg(not(target_os = "windows"))]
use crate::ui::exit_handler::setup_exit_handlers;
#[cfg(not(target_os = "windows"))]
use crate::ui::setup_window_event_listeners;
use crate::ui::{initialize_audio_asset_manager, migrate_legacy_custom_audio};
use chrono::Local;
use once_cell::sync::Lazy;
use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Manager};

/// 全局变量：追踪最后聚焦的窗口 label
pub static LAST_FOCUSED_WINDOW: RwLock<Option<String>> = RwLock::new(None);
static STARTUP_STATUS: Lazy<RwLock<StartupStatus>> =
    Lazy::new(|| RwLock::new(StartupStatus::starting("正在启动后台服务")));
static BACKGROUND_RETRY_IN_FLIGHT: AtomicBool = AtomicBool::new(false);
static LAST_SELF_HEAL_AT: AtomicU64 = AtomicU64::new(0);
static LAST_FAILED_BRIDGE_RECOVER_AT: AtomicU64 = AtomicU64::new(0);
static BRIDGE_ORIGIN_TRACKER: Mutex<BridgeOriginTracker> = Mutex::new(BridgeOriginTracker {
    consecutive_hung_failures: 0,
});
const WATCHDOG_INTERVAL_SECS: u64 = 20;
const WATCHDOG_STARTUP_GRACE_SECS: u64 = 30;
const WATCHDOG_COOLDOWN_SECS: u64 = 300;
const MANUAL_RECOVER_FAILED_RETRY_COOLDOWN_SECS: u64 = 60;
const BRIDGE_HUNG_CONFIRMATION_FAILURES: u32 = 3;
const BRIDGE_RECOVERY_VERIFY_ATTEMPTS: u32 = 12;
const BRIDGE_RECOVERY_VERIFY_INTERVAL_MS: u64 = 500;
const BRIDGE_RECOVERY_STABLE_CHECKS: u32 = 3;
const LOCAL_HEALTH_TIMEOUT_SECS: u64 = 3;
const CONNECTION_STATUS_TIMEOUT_SECS: u64 = 3;
const PUBLIC_FALLBACK_HEALTH_TIMEOUT_SECS: u64 = 3;
const CONNECTION_STATUS_URL: &str = "http://127.0.0.1:8080/api/connection-status";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupStatus {
    pub phase: String,
    pub message: String,
}

impl StartupStatus {
    fn starting(message: impl Into<String>) -> Self {
        Self {
            phase: "starting".to_string(),
            message: message.into(),
        }
    }

    fn ready() -> Self {
        Self {
            phase: "ready".to_string(),
            message: "后台服务已就绪".to_string(),
        }
    }

    fn degraded(message: impl Into<String>) -> Self {
        Self {
            phase: "degraded".to_string(),
            message: message.into(),
        }
    }
}

fn publish_startup_status(app_handle: &AppHandle, status: StartupStatus) {
    if let Ok(mut current) = STARTUP_STATUS.write() {
        *current = status.clone();
    }
    let _ = app_handle.emit("startup-status-changed", status);
}

#[tauri::command]
pub fn get_startup_status() -> StartupStatus {
    STARTUP_STATUS
        .read()
        .map(|status| status.clone())
        .unwrap_or_else(|_| StartupStatus::degraded("无法读取后台服务状态"))
}

fn instance_debug_log(tag: &str, message: impl AsRef<str>) {
    let line = format!(
        "{} [app-setup:{}] {} {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        std::process::id(),
        tag,
        message.as_ref()
    );
    #[cfg(target_os = "windows")]
    let path = std::env::temp_dir().join("iterate-instance-debug.log");
    #[cfg(not(target_os = "windows"))]
    let path = PathBuf::from("/tmp/iterate-instance-debug.log");
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 设置最后聚焦的窗口
pub fn set_last_focused_window(label: &str) {
    if let Ok(mut guard) = LAST_FOCUSED_WINDOW.write() {
        *guard = Some(label.to_string());
    }
}

/// 获取最后聚焦的窗口
pub fn get_last_focused_window() -> Option<String> {
    LAST_FOCUSED_WINDOW
        .read()
        .ok()
        .and_then(|guard| guard.clone())
}

const BRIDGE_DAEMON_LABEL: &str = "com.cunzhi.iterate.bridge";

fn command_stdout(command: &str, args: &[&str]) -> Option<String> {
    let mut child = std::process::Command::new(command);
    child.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        child.creation_flags(CREATE_NO_WINDOW);
    }
    child
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
}

#[cfg(target_os = "windows")]
fn bridge_http_healthy(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{port}/api/version");
    let Ok(client) = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(2))
        .no_proxy()
        .build()
    else {
        return false;
    };

    client
        .get(url)
        .send()
        .and_then(|response| response.error_for_status())
        .and_then(|response| response.text())
        .map(|body| body.contains("iterate"))
        .unwrap_or(false)
}

#[cfg(not(target_os = "windows"))]
fn bridge_http_healthy(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{}/api/version", port);
    command_stdout("curl", &["--noproxy", "*", "-fsS", "-m", "2", &url])
        .map(|body| body.contains("iterate"))
        .unwrap_or(false)
}

fn bridge_args_match(args: &str, port: u16) -> bool {
    let mut has_bridge_only = false;
    let mut has_port = false;
    let port_value = port.to_string();
    let mut parts = args.split_whitespace().peekable();

    while let Some(part) = parts.next() {
        if part == "--bridge-only" {
            has_bridge_only = true;
        } else if part == "--port" {
            has_port = parts.peek().is_some_and(|next| *next == port_value);
        } else if part == format!("--port={}", port_value) {
            has_port = true;
        }
    }

    has_bridge_only && has_port
}

fn port_owner_pid(port: u16) -> Option<u32> {
    let port_arg = format!("-tiTCP:{}", port);
    command_stdout("lsof", &["-nP", &port_arg, "-sTCP:LISTEN"]).and_then(|stdout| {
        stdout
            .lines()
            .next()
            .and_then(|line| line.trim().parse().ok())
    })
}

fn process_args(pid: u32) -> Option<String> {
    command_stdout("ps", &["-p", &pid.to_string(), "-ww", "-o", "args="])
        .map(|args| args.trim().to_string())
        .filter(|args| !args.is_empty())
}

fn current_uid() -> String {
    command_stdout("id", &["-u"])
        .map(|uid| uid.trim().to_string())
        .filter(|uid| !uid.is_empty())
        .unwrap_or_else(|| "501".to_string())
}

fn bridge_daemon_domain() -> String {
    format!("gui/{}", current_uid())
}

fn bridge_daemon_service() -> String {
    format!("{}/{}", bridge_daemon_domain(), BRIDGE_DAEMON_LABEL)
}

fn bridge_daemon_plist_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    home.join("Library")
        .join("LaunchAgents")
        .join(format!("{}.plist", BRIDGE_DAEMON_LABEL))
}

fn bridge_daemon_plist_exists() -> bool {
    bridge_daemon_plist_path().is_file()
}

fn launchctl_bridge_daemon_state() -> Option<(u32, String)> {
    let service = bridge_daemon_service();
    let stdout = command_stdout("launchctl", &["print", &service])?;
    let mut pid = None;
    let mut args = Vec::new();
    let mut in_arguments = false;

    for line in stdout.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("pid = ") {
            pid = value.trim().parse::<u32>().ok();
            continue;
        }

        if trimmed == "arguments = {" {
            in_arguments = true;
            continue;
        }
        if in_arguments {
            if trimmed == "}" {
                in_arguments = false;
            } else if !trimmed.is_empty() {
                args.push(trimmed.to_string());
            }
        }
    }

    pid.map(|pid| (pid, args.join(" ")))
}

fn bridge_daemon_process_owns_port(port: u16) -> bool {
    let Some(owner_pid) = port_owner_pid(port) else {
        instance_debug_log(
            "[bridge-owner-check]",
            format!("port={} owner pid missing", port),
        );
        return false;
    };

    let owner_args = process_args(owner_pid).unwrap_or_default();
    if bridge_args_match(&owner_args, port) {
        return true;
    }

    let Some((launchd_pid, launchd_args)) = launchctl_bridge_daemon_state() else {
        instance_debug_log(
            "[bridge-owner-check]",
            format!(
                "port={} owner_pid={} owner_args={:?} launchd_state=missing",
                port, owner_pid, owner_args
            ),
        );
        return false;
    };

    let launchd_match = launchd_pid == owner_pid && bridge_args_match(&launchd_args, port);
    if !launchd_match {
        instance_debug_log(
            "[bridge-owner-check]",
            format!(
                "port={} owner_pid={} owner_args={:?} launchd_pid={} launchd_args={:?}",
                port, owner_pid, owner_args, launchd_pid, launchd_args
            ),
        );
    }

    launchd_match
}

fn bridge_daemon_owns_port(port: u16) -> bool {
    bridge_http_healthy(port) && bridge_daemon_process_owns_port(port)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BridgeOriginState {
    Healthy,
    OriginDown,
    WrongOwner,
    HungTransient,
    HungConfirmed,
}

impl BridgeOriginState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::OriginDown => "origin_down",
            Self::WrongOwner => "wrong_owner",
            Self::HungTransient => "origin_hung_transient",
            Self::HungConfirmed => "origin_hung_confirmed",
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct BridgeOriginObservation {
    state: BridgeOriginState,
    consecutive_hung_failures: u32,
    daemon_process_owns_port: bool,
    port_has_owner: bool,
}

#[derive(Debug)]
struct BridgeOriginTracker {
    consecutive_hung_failures: u32,
}

#[derive(Debug, Serialize)]
pub struct BridgeOriginRecoveryResponse {
    pub status: String,
    pub origin_state: String,
    pub healthy: bool,
    pub recovered: bool,
    pub cooldown_remaining_secs: u64,
    pub message: String,
}

fn classify_bridge_origin_state(
    local_origin_ok: bool,
    daemon_process_owns_port: bool,
    port_has_owner: bool,
    consecutive_hung_failures: u32,
) -> BridgeOriginState {
    if local_origin_ok {
        BridgeOriginState::Healthy
    } else if daemon_process_owns_port {
        if consecutive_hung_failures >= BRIDGE_HUNG_CONFIRMATION_FAILURES {
            BridgeOriginState::HungConfirmed
        } else {
            BridgeOriginState::HungTransient
        }
    } else if port_has_owner {
        BridgeOriginState::WrongOwner
    } else {
        BridgeOriginState::OriginDown
    }
}

fn observe_bridge_origin_state(port: u16, local_origin_ok: bool) -> BridgeOriginObservation {
    let daemon_process_owns_port = bridge_daemon_process_owns_port(port);
    let port_has_owner = daemon_process_owns_port || port_owner_pid(port).is_some();

    let mut tracker = BRIDGE_ORIGIN_TRACKER
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    if local_origin_ok {
        tracker.consecutive_hung_failures = 0;
    } else if daemon_process_owns_port {
        tracker.consecutive_hung_failures = tracker.consecutive_hung_failures.saturating_add(1);
    } else {
        tracker.consecutive_hung_failures = 0;
    }

    let state = classify_bridge_origin_state(
        local_origin_ok,
        daemon_process_owns_port,
        port_has_owner,
        tracker.consecutive_hung_failures,
    );

    BridgeOriginObservation {
        state,
        consecutive_hung_failures: tracker.consecutive_hung_failures,
        daemon_process_owns_port,
        port_has_owner,
    }
}

// 成功恢复占用完整 300s cooldown；失败的手动恢复只占 60s，
// 避免用户点一次失败的「重试」后被锁 5 分钟只能拿到 cooldown_active。
fn manual_recovery_cooldown_remaining(now: u64, last_success_at: u64, last_failed_at: u64) -> u64 {
    let success_remaining =
        WATCHDOG_COOLDOWN_SECS.saturating_sub(now.saturating_sub(last_success_at));
    let failed_remaining = MANUAL_RECOVER_FAILED_RETRY_COOLDOWN_SECS
        .saturating_sub(now.saturating_sub(last_failed_at));
    success_remaining.max(failed_remaining)
}

fn bridge_recovery_cooldown_remaining_for_state(
    now: u64,
    state: BridgeOriginState,
    last_success_at: u64,
    last_failed_at: u64,
) -> u64 {
    let failed_remaining = MANUAL_RECOVER_FAILED_RETRY_COOLDOWN_SECS
        .saturating_sub(now.saturating_sub(last_failed_at));
    if matches!(
        state,
        BridgeOriginState::OriginDown
            | BridgeOriginState::WrongOwner
            | BridgeOriginState::HungConfirmed
    ) {
        return failed_remaining;
    }

    manual_recovery_cooldown_remaining(now, last_success_at, last_failed_at)
}

fn bridge_recovery_cooldown_remaining(now: u64, state: BridgeOriginState) -> u64 {
    bridge_recovery_cooldown_remaining_for_state(
        now,
        state,
        effective_last_self_heal_at(),
        LAST_FAILED_BRIDGE_RECOVER_AT.load(Ordering::SeqCst),
    )
}

async fn wait_for_stable_bridge_daemon_ownership(port: u16, reason: &str, phase: &str) -> bool {
    let mut stable_checks = 0;
    for attempt in 1..=BRIDGE_RECOVERY_VERIFY_ATTEMPTS {
        tokio::time::sleep(Duration::from_millis(BRIDGE_RECOVERY_VERIFY_INTERVAL_MS)).await;
        if bridge_daemon_owns_port(port) {
            stable_checks += 1;
            if stable_checks < BRIDGE_RECOVERY_STABLE_CHECKS {
                continue;
            }
            instance_debug_log(
                "[bridge-daemon-recover-ok]",
                format!(
                    "reason={}, phase={}, port={}, attempt={}, stable_checks={}",
                    reason, phase, port, attempt, stable_checks
                ),
            );
            return true;
        }

        if stable_checks > 0 {
            instance_debug_log(
                "[bridge-daemon-recover-unstable]",
                format!(
                    "reason={}, phase={}, port={}, attempt={}, stable_checks_reset_from={}",
                    reason, phase, port, attempt, stable_checks
                ),
            );
            stable_checks = 0;
        }
    }

    instance_debug_log(
        "[bridge-daemon-recover-timeout]",
        format!(
            "reason={}, phase={}, port={}, stable_checks={}",
            reason, phase, port, stable_checks
        ),
    );
    false
}

async fn ensure_bridge_daemon_owns_port(port: u16, reason: &str) -> bool {
    if bridge_daemon_owns_port(port) {
        return wait_for_stable_bridge_daemon_ownership(port, reason, "already_running").await;
    }

    if reason == "watchdog_origin_down" && bridge_daemon_process_owns_port(port) {
        instance_debug_log(
            "[bridge-daemon-recover-skip]",
            format!(
                "reason={}, port={}, daemon_process_owns_port=true, local_http_healthy=false",
                reason, port
            ),
        );
        return true;
    }

    let plist_path = bridge_daemon_plist_path();
    if !plist_path.is_file() {
        instance_debug_log(
            "[bridge-daemon-recover-skip]",
            format!(
                "reason={}, port={}, plist_missing={}",
                reason,
                port,
                plist_path.display()
            ),
        );
        return false;
    }

    let domain = bridge_daemon_domain();
    let service = bridge_daemon_service();
    instance_debug_log(
        "[bridge-daemon-recover-begin]",
        format!(
            "reason={}, port={}, service={}, plist={}",
            reason,
            port,
            service,
            plist_path.display()
        ),
    );

    match tokio::process::Command::new("launchctl")
        .arg("bootstrap")
        .arg(&domain)
        .arg(&plist_path)
        .output()
        .await
    {
        Ok(output) if output.status.success() => {
            instance_debug_log(
                "[bridge-daemon-bootstrap-ok]",
                format!("reason={}, service={}", reason, service),
            );
        }
        Ok(output) => {
            instance_debug_log(
                "[bridge-daemon-bootstrap-nonzero]",
                format!(
                    "reason={}, service={}, status={:?}, stderr={}",
                    reason,
                    service,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            );
        }
        Err(err) => {
            instance_debug_log(
                "[bridge-daemon-bootstrap-error]",
                format!("reason={}, service={}, error={}", reason, service, err),
            );
        }
    }

    let kickstart_ok = match tokio::process::Command::new("launchctl")
        .args(["kickstart", "-k", &service])
        .output()
        .await
    {
        Ok(output) if output.status.success() => true,
        Ok(output) => {
            instance_debug_log(
                "[bridge-daemon-kickstart-failed]",
                format!(
                    "reason={}, service={}, status={:?}, stderr={}",
                    reason,
                    service,
                    output.status.code(),
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            );
            false
        }
        Err(err) => {
            instance_debug_log(
                "[bridge-daemon-kickstart-error]",
                format!("reason={}, service={}, error={}", reason, service, err),
            );
            false
        }
    };
    if !kickstart_ok {
        return false;
    }

    wait_for_stable_bridge_daemon_ownership(port, reason, "after_kickstart").await
}

pub async fn recover_bridge_origin() -> BridgeOriginRecoveryResponse {
    let local_origin_ok = bridge_origin_healthy().await;
    let observation = observe_bridge_origin_state(8080, local_origin_ok);

    if observation.state == BridgeOriginState::Healthy {
        return BridgeOriginRecoveryResponse {
            status: "already_healthy".to_string(),
            origin_state: observation.state.as_str().to_string(),
            healthy: true,
            recovered: false,
            cooldown_remaining_secs: 0,
            message: "8080 bridge is already healthy".to_string(),
        };
    }

    let now = unix_now_secs();
    let cooldown_remaining_secs = bridge_recovery_cooldown_remaining(now, observation.state);
    if cooldown_remaining_secs > 0 {
        instance_debug_log(
            "[manual-bridge-recover-cooldown]",
            format!(
                "state={}, cooldown_remaining_secs={}, hung_failures={}, daemon_process_owns_port={}, port_has_owner={}",
                observation.state.as_str(),
                cooldown_remaining_secs,
                observation.consecutive_hung_failures,
                observation.daemon_process_owns_port,
                observation.port_has_owner
            ),
        );
        return BridgeOriginRecoveryResponse {
            status: "cooldown_active".to_string(),
            origin_state: observation.state.as_str().to_string(),
            healthy: false,
            recovered: false,
            cooldown_remaining_secs,
            message: format!(
                "Bridge recovery is cooling down for {}s",
                cooldown_remaining_secs
            ),
        };
    }

    let recovered = ensure_bridge_daemon_owns_port(8080, "manual_recover_bridge_origin").await;
    let completed_at = unix_now_secs();
    if recovered {
        persist_last_self_heal_at(completed_at);
    } else {
        LAST_FAILED_BRIDGE_RECOVER_AT.store(completed_at, Ordering::SeqCst);
    }
    let status = if recovered {
        "recovery_started"
    } else {
        "failed"
    };

    BridgeOriginRecoveryResponse {
        status: status.to_string(),
        origin_state: observation.state.as_str().to_string(),
        healthy: recovered,
        recovered,
        cooldown_remaining_secs: if recovered {
            0
        } else {
            MANUAL_RECOVER_FAILED_RETRY_COOLDOWN_SECS
        },
        message: if recovered {
            "Bridge recovery was started and the origin became healthy".to_string()
        } else {
            "Bridge recovery failed or the origin did not become healthy".to_string()
        },
    }
}

fn unix_now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn last_self_heal_state_path() -> std::path::PathBuf {
    std::env::temp_dir().join("iterate-last-self-heal-at")
}

fn load_persisted_last_self_heal_at() -> u64 {
    std::fs::read_to_string(last_self_heal_state_path())
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(0)
}

fn effective_last_self_heal_at() -> u64 {
    let in_memory = LAST_SELF_HEAL_AT.load(Ordering::SeqCst);
    let persisted = load_persisted_last_self_heal_at();
    let effective = in_memory.max(persisted);
    if effective > in_memory {
        LAST_SELF_HEAL_AT.store(effective, Ordering::SeqCst);
    }
    effective
}

fn persist_last_self_heal_at(timestamp: u64) {
    LAST_SELF_HEAL_AT.store(timestamp, Ordering::SeqCst);
    let _ = std::fs::write(last_self_heal_state_path(), timestamp.to_string());
}

async fn bridge_origin_healthy() -> bool {
    health_endpoint_ok(
        "http://127.0.0.1:8080/api/version",
        Duration::from_secs(LOCAL_HEALTH_TIMEOUT_SECS),
    )
    .await
}

async fn public_tunnel_probe_healthy() -> bool {
    let Some(route) = crate::tunnel::commands::configured_formal_mobile_route() else {
        return false;
    };
    let health_url = format!("{}/api/version", route.base_url.trim_end_matches('/'));
    health_endpoint_ok(
        &health_url,
        Duration::from_secs(PUBLIC_FALLBACK_HEALTH_TIMEOUT_SECS),
    )
    .await
}

async fn health_endpoint_ok(url: &str, timeout: Duration) -> bool {
    let client = match reqwest::Client::builder()
        .timeout(timeout)
        .no_proxy()
        .build()
    {
        Ok(client) => client,
        Err(_) => return false,
    };

    client
        .get(url)
        .send()
        .await
        .map(|resp| resp.status().is_success())
        .unwrap_or(false)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PublicTunnelSnapshot {
    effective_ok: Option<bool>,
    health_source: String,
    diagnosis_code: Option<String>,
    raw_public_probe_ok: Option<bool>,
}

fn unknown_public_snapshot(source: impl Into<String>) -> PublicTunnelSnapshot {
    PublicTunnelSnapshot {
        effective_ok: None,
        health_source: source.into(),
        diagnosis_code: None,
        raw_public_probe_ok: None,
    }
}

fn json_path<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    Some(current)
}

fn json_bool(value: &serde_json::Value, path: &[&str]) -> Option<bool> {
    json_path(value, path).and_then(|value| value.as_bool())
}

fn json_str<'a>(value: &'a serde_json::Value, path: &[&str]) -> Option<&'a str> {
    json_path(value, path).and_then(|value| value.as_str())
}

fn json_f64(value: &serde_json::Value, path: &[&str]) -> Option<f64> {
    json_path(value, path).and_then(|value| {
        value
            .as_f64()
            .or_else(|| value.as_str().and_then(|raw| raw.parse::<f64>().ok()))
    })
}

fn connection_status_root_tunnel_authoritative_up(value: &serde_json::Value) -> bool {
    let metrics_http_ok = json_bool(value, &["root_tunnel", "metrics", "http_ok"]).unwrap_or(false);
    let live_ha_count =
        json_f64(value, &["root_tunnel", "metrics", "ha_connection_count"]).unwrap_or(0.0);
    let status_ha_count = json_f64(
        value,
        &["root_tunnel", "metrics", "status_ha_connection_count"],
    )
    .unwrap_or(0.0);
    let expected_ha_count = json_f64(
        value,
        &["root_tunnel", "metrics", "expected_ha_connections"],
    )
    .filter(|value| *value > 0.0)
    .unwrap_or(4.0);
    let status_fresh = json_bool(value, &["root_tunnel", "status_fresh"]).unwrap_or(false);
    let child_alive =
        json_bool(value, &["root_tunnel", "derived", "child_alive"]).unwrap_or(live_ha_count > 0.0);
    let ha_ready = live_ha_count >= expected_ha_count
        || (status_fresh && status_ha_count >= expected_ha_count);

    metrics_http_ok && child_alive && ha_ready
}

fn parse_connection_status_public_snapshot(value: &serde_json::Value) -> PublicTunnelSnapshot {
    let diagnosis_code = json_str(value, &["diagnosis", "code"]).map(ToOwned::to_owned);
    let diagnosis = diagnosis_code.as_deref();
    let public_healthy = json_bool(value, &["public_tunnel", "healthy"]);
    let public_ws_healthy = json_bool(value, &["public_tunnel", "websocket_healthy"]);
    let public_ws_auth_required =
        json_bool(value, &["public_tunnel", "websocket_auth_required"]).unwrap_or(false);
    let raw_public_probe_ok = json_bool(value, &["public_tunnel", "probe", "healthy"]);
    let root_authoritative = connection_status_root_tunnel_authoritative_up(value);

    let public_fields_ok = match (public_healthy, public_ws_healthy) {
        (Some(true), Some(false)) => Some(false),
        (Some(true), _) => Some(true),
        (Some(false), _) => Some(false),
        _ => None,
    };

    let effective_ok = if diagnosis == Some("ok") {
        Some(true)
    } else if root_authoritative {
        Some(true)
    } else if let Some(ok) = public_fields_ok {
        Some(ok)
    } else if matches!(
        diagnosis,
        Some(
            "public_tunnel_down_local_ok"
                | "public_ws_unavailable"
                | "root_tunnel_child_missing"
                | "root_tunnel_ha_degraded"
                | "root_tunnel_backoff_active"
        )
    ) {
        Some(false)
    } else {
        None
    };

    let health_source = json_str(value, &["public_tunnel", "health_source"])
        .filter(|source| !source.is_empty() && *source != "none")
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            if effective_ok == Some(true) && root_authoritative {
                "root_tunnel_ha".to_string()
            } else if effective_ok == Some(true) && public_ws_auth_required {
                "auth_required".to_string()
            } else if effective_ok.is_some() {
                "connection_status".to_string()
            } else {
                "connection_status_unknown".to_string()
            }
        });

    PublicTunnelSnapshot {
        effective_ok,
        health_source,
        diagnosis_code,
        raw_public_probe_ok,
    }
}

async fn fetch_connection_status_public_snapshot() -> PublicTunnelSnapshot {
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(CONNECTION_STATUS_TIMEOUT_SECS))
        .no_proxy()
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            return unknown_public_snapshot(format!("connection_status_client_error:{err}"))
        }
    };

    let request = match crate::bridge::auth::authorize_internal_bridge_request(
        client.get(CONNECTION_STATUS_URL),
        "GET",
        CONNECTION_STATUS_URL,
    ) {
        Ok(request) => request,
        Err(err) => return unknown_public_snapshot(format!("connection_status_auth_error:{err}")),
    };
    let response = match request.send().await {
        Ok(response) => response,
        Err(err) => return unknown_public_snapshot(format!("connection_status_error:{err}")),
    };
    let status = response.status();
    if !status.is_success() {
        return unknown_public_snapshot(format!("connection_status_http_{}", status.as_u16()));
    }

    let body = match response.text().await {
        Ok(body) => body,
        Err(err) => return unknown_public_snapshot(format!("connection_status_body_error:{err}")),
    };
    let value = match serde_json::from_str::<serde_json::Value>(&body) {
        Ok(value) => value,
        Err(err) => return unknown_public_snapshot(format!("connection_status_json_error:{err}")),
    };

    parse_connection_status_public_snapshot(&value)
}

fn optional_bool_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "true",
        Some(false) => "false",
        None => "unknown",
    }
}

#[derive(Clone)]
struct ConnectivitySnapshot {
    local_origin_ok: bool,
    public_tunnel_effective_ok: Option<bool>,
    public_health_source: String,
    diagnosis_code: Option<String>,
    raw_public_probe_ok: Option<bool>,
}

impl ConnectivitySnapshot {
    fn public_tunnel_effectively_healthy(&self) -> bool {
        self.public_tunnel_effective_ok == Some(true)
    }

    fn public_tunnel_unhealthy(&self) -> bool {
        self.public_tunnel_effective_ok == Some(false)
    }

    fn public_tunnel_unknown(&self) -> bool {
        self.public_tunnel_effective_ok.is_none()
    }

    fn summary(&self) -> String {
        format!(
            "local_origin_ok={} public_tunnel_effective_ok={} health_source={} diagnosis={} raw_public_probe_ok={}",
            self.local_origin_ok,
            optional_bool_label(self.public_tunnel_effective_ok),
            self.public_health_source,
            self.diagnosis_code.as_deref().unwrap_or("n/a"),
            optional_bool_label(self.raw_public_probe_ok)
        )
    }
}

async fn collect_connectivity_snapshot() -> ConnectivitySnapshot {
    let local_origin_ok = bridge_origin_healthy().await;
    if !local_origin_ok {
        return ConnectivitySnapshot {
            local_origin_ok,
            public_tunnel_effective_ok: None,
            public_health_source: "local_origin_down".to_string(),
            diagnosis_code: None,
            raw_public_probe_ok: None,
        };
    }

    // Treat the bridge daemon's connection-status endpoint as the health
    // contract across the GUI/bridge process boundary. Raw public probes are
    // only a fallback observation when the central status is unavailable.
    let mut public_snapshot = fetch_connection_status_public_snapshot().await;
    if public_snapshot.effective_ok.is_none() {
        let raw_public_probe_ok = public_tunnel_probe_healthy().await;
        public_snapshot.raw_public_probe_ok = Some(raw_public_probe_ok);
        if raw_public_probe_ok {
            public_snapshot.effective_ok = Some(true);
            public_snapshot.health_source = "probe_fallback".to_string();
        } else {
            public_snapshot.health_source =
                format!("{}+probe_fallback_failed", public_snapshot.health_source);
        }
    }

    ConnectivitySnapshot {
        local_origin_ok,
        public_tunnel_effective_ok: public_snapshot.effective_ok,
        public_health_source: public_snapshot.health_source,
        diagnosis_code: public_snapshot.diagnosis_code,
        raw_public_probe_ok: public_snapshot.raw_public_probe_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_connection_status_ok_with_root_ha_source() {
        let snapshot = parse_connection_status_public_snapshot(&json!({
            "diagnosis": { "code": "ok" },
            "public_tunnel": {
                "healthy": true,
                "health_source": "root_tunnel_ha",
                "probe": { "healthy": false },
                "websocket_healthy": true,
                "websocket_auth_required": true
            },
            "root_tunnel": {
                "status_fresh": true,
                "metrics": {
                    "http_ok": true,
                    "ha_connection_count": 4,
                    "expected_ha_connections": 4
                },
                "derived": { "child_alive": true }
            }
        }));

        assert_eq!(snapshot.effective_ok, Some(true));
        assert_eq!(snapshot.health_source, "root_tunnel_ha");
        assert_eq!(snapshot.diagnosis_code.as_deref(), Some("ok"));
        assert_eq!(snapshot.raw_public_probe_ok, Some(false));
    }

    #[test]
    fn treats_auth_required_websocket_as_protected_when_status_is_ok() {
        let snapshot = parse_connection_status_public_snapshot(&json!({
            "diagnosis": { "code": "ok" },
            "public_tunnel": {
                "healthy": true,
                "probe": { "healthy": true },
                "websocket_healthy": true,
                "websocket_auth_required": true
            }
        }));

        assert_eq!(snapshot.effective_ok, Some(true));
        assert_eq!(snapshot.health_source, "auth_required");
    }

    #[test]
    fn falls_back_to_root_ha_when_public_fields_are_missing() {
        let snapshot = parse_connection_status_public_snapshot(&json!({
            "root_tunnel": {
                "status_fresh": true,
                "metrics": {
                    "http_ok": true,
                    "ha_connection_count": 0,
                    "status_ha_connection_count": 4,
                    "expected_ha_connections": 4
                },
                "derived": { "child_alive": true }
            }
        }));

        assert_eq!(snapshot.effective_ok, Some(true));
        assert_eq!(snapshot.health_source, "root_tunnel_ha");
    }

    #[test]
    fn marks_public_down_from_connection_status_without_using_unknown() {
        let snapshot = parse_connection_status_public_snapshot(&json!({
            "diagnosis": { "code": "public_tunnel_down_local_ok" },
            "public_tunnel": {
                "healthy": false,
                "health_source": "none",
                "probe": { "healthy": false },
                "websocket_healthy": false,
                "websocket_auth_required": false
            }
        }));

        assert_eq!(snapshot.effective_ok, Some(false));
        assert_eq!(snapshot.health_source, "connection_status");
        assert_eq!(snapshot.raw_public_probe_ok, Some(false));
    }

    #[test]
    fn missing_connection_status_is_unknown_not_false() {
        let snapshot = unknown_public_snapshot("connection_status_timeout");

        assert_eq!(snapshot.effective_ok, None);
        assert_eq!(snapshot.health_source, "connection_status_timeout");
        assert_eq!(optional_bool_label(snapshot.effective_ok), "unknown");
    }

    #[test]
    fn classifies_healthy_bridge_origin() {
        assert_eq!(
            classify_bridge_origin_state(true, true, true, 0),
            BridgeOriginState::Healthy
        );
    }

    #[test]
    fn classifies_transient_and_confirmed_hung_bridge_origin() {
        assert_eq!(
            classify_bridge_origin_state(false, true, true, 1),
            BridgeOriginState::HungTransient
        );
        assert_eq!(
            classify_bridge_origin_state(false, true, true, BRIDGE_HUNG_CONFIRMATION_FAILURES),
            BridgeOriginState::HungConfirmed
        );
    }

    #[test]
    fn classifies_wrong_owner_and_origin_down() {
        assert_eq!(
            classify_bridge_origin_state(false, false, true, 0),
            BridgeOriginState::WrongOwner
        );
        assert_eq!(
            classify_bridge_origin_state(false, false, false, 0),
            BridgeOriginState::OriginDown
        );
    }

    #[test]
    fn failed_manual_recovery_uses_short_retry_cooldown() {
        // 成功恢复后 300s 内仍处于 cooldown
        assert_eq!(manual_recovery_cooldown_remaining(1000, 900, 0), 200);
        // 失败的手动恢复只锁 60s，而不是 300s
        assert_eq!(manual_recovery_cooldown_remaining(1000, 0, 990), 50);
        assert_eq!(manual_recovery_cooldown_remaining(1000, 0, 930), 0);
        // 两者同时存在时取剩余更长的那个
        assert_eq!(manual_recovery_cooldown_remaining(1000, 950, 990), 250);
        // 无任何历史记录时不应有 cooldown
        assert_eq!(manual_recovery_cooldown_remaining(1000, 0, 0), 0);
    }

    #[test]
    fn confirmed_bridge_origin_failures_bypass_success_cooldown() {
        assert_eq!(
            bridge_recovery_cooldown_remaining_for_state(
                1000,
                BridgeOriginState::HungConfirmed,
                950,
                0
            ),
            0
        );
        assert_eq!(
            bridge_recovery_cooldown_remaining_for_state(
                1000,
                BridgeOriginState::OriginDown,
                950,
                0
            ),
            0
        );
        assert_eq!(
            bridge_recovery_cooldown_remaining_for_state(
                1000,
                BridgeOriginState::WrongOwner,
                950,
                0
            ),
            0
        );
    }

    #[test]
    fn transient_bridge_origin_failure_keeps_success_cooldown() {
        assert_eq!(
            bridge_recovery_cooldown_remaining_for_state(
                1000,
                BridgeOriginState::HungTransient,
                950,
                0
            ),
            250
        );
    }

    #[test]
    fn confirmed_bridge_origin_failures_keep_failed_retry_cooldown() {
        assert_eq!(
            bridge_recovery_cooldown_remaining_for_state(
                1000,
                BridgeOriginState::HungConfirmed,
                950,
                990
            ),
            50
        );
    }

    #[test]
    fn bridge_args_match_accepts_split_or_equals_port() {
        assert!(bridge_args_match(
            "/Applications/iterate.app/Contents/MacOS/iterate --bridge-only --port 8080",
            8080
        ));
        assert!(bridge_args_match(
            "/Applications/iterate.app/Contents/MacOS/iterate --bridge-only --port=8080",
            8080
        ));
    }

    #[test]
    fn bridge_args_match_rejects_gui_or_wrong_port() {
        assert!(!bridge_args_match(
            "/Applications/iterate.app/Contents/MacOS/iterate",
            8080
        ));
        assert!(!bridge_args_match(
            "/Applications/iterate.app/Contents/MacOS/iterate --bridge-only --port 8099",
            8080
        ));
    }
}

#[cfg(not(target_os = "windows"))]
fn start_connectivity_watchdog(app_handle: AppHandle) {
    let startup_grace_until = unix_now_secs().saturating_add(WATCHDOG_STARTUP_GRACE_SECS);
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(WATCHDOG_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            interval.tick().await;

            let now = unix_now_secs();
            let snapshot = collect_connectivity_snapshot().await;

            if now < startup_grace_until {
                if !snapshot.local_origin_ok || !snapshot.public_tunnel_effectively_healthy() {
                    instance_debug_log(
                        "[watchdog-skip-startup-grace]",
                        format!(
                            "{}, now={}, startup_grace_until={}",
                            snapshot.summary(),
                            now,
                            startup_grace_until
                        ),
                    );
                }
                continue;
            }

            if snapshot.local_origin_ok {
                let _ = observe_bridge_origin_state(8080, true);
                if bridge_daemon_plist_exists() && !bridge_daemon_owns_port(8080) {
                    let last = effective_last_self_heal_at();
                    if now.saturating_sub(last) < WATCHDOG_COOLDOWN_SECS {
                        instance_debug_log(
                            "[watchdog-skip-owner-drift-cooldown]",
                            format!(
                                "{}, last_self_heal_at={}, now={}",
                                snapshot.summary(),
                                last,
                                now
                            ),
                        );
                        continue;
                    }

                    instance_debug_log(
                        "[watchdog-owner-drift]",
                        format!("{}, now={}", snapshot.summary(), now),
                    );

                    if ensure_bridge_daemon_owns_port(8080, "watchdog_owner_drift").await {
                        persist_last_self_heal_at(unix_now_secs());
                        log_important!(
                            info,
                            "[Watchdog] bridge daemon ownership recovered, skipping GUI restart"
                        );
                        continue;
                    }

                    LAST_FAILED_BRIDGE_RECOVER_AT.store(unix_now_secs(), Ordering::SeqCst);

                    log_important!(
                        warn,
                        "[Watchdog] 8080 owner is not bridge daemon; restarting GUI to release port"
                    );
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    instance_debug_log(
                        "[watchdog-restart-app-owner-drift]",
                        "calling app_handle.restart()",
                    );
                    app_handle.restart();
                }

                if snapshot.public_tunnel_unhealthy() {
                    instance_debug_log(
                        "[watchdog-observe-public-unhealthy]",
                        format!("{}, now={}", snapshot.summary(), now),
                    );
                } else if snapshot.public_tunnel_unknown() {
                    instance_debug_log(
                        "[watchdog-observe-public-unknown]",
                        format!("{}, now={}", snapshot.summary(), now),
                    );
                }
                continue;
            }

            // Double-check after a short delay so transient startup jitter does not
            // immediately escalate into an app restart loop.
            tokio::time::sleep(Duration::from_secs(2)).await;
            let recheck = collect_connectivity_snapshot().await;
            if recheck.local_origin_ok {
                let _ = observe_bridge_origin_state(8080, true);
                instance_debug_log(
                    "[watchdog-recovered-before-heal]",
                    format!("{}, now={}", recheck.summary(), unix_now_secs()),
                );
                continue;
            }

            let observation = observe_bridge_origin_state(8080, false);
            instance_debug_log(
                "[watchdog-origin-state]",
                format!(
                    "{}, state={}, hung_failures={}, daemon_process_owns_port={}, port_has_owner={}",
                    recheck.summary(),
                    observation.state.as_str(),
                    observation.consecutive_hung_failures,
                    observation.daemon_process_owns_port,
                    observation.port_has_owner
                ),
            );
            if observation.state == BridgeOriginState::HungTransient {
                continue;
            }

            let last = effective_last_self_heal_at();
            let cooldown_remaining_secs =
                bridge_recovery_cooldown_remaining(now, observation.state);
            if cooldown_remaining_secs > 0 {
                instance_debug_log(
                    "[watchdog-skip-cooldown]",
                    format!(
                        "{}, state={}, cooldown_remaining_secs={}, last_self_heal_at={}, now={}",
                        recheck.summary(),
                        observation.state.as_str(),
                        cooldown_remaining_secs,
                        last,
                        now
                    ),
                );
                continue;
            }

            instance_debug_log(
                "[watchdog-trigger]",
                format!(
                    "{}, now={}, last={}, startup_grace_until={}",
                    recheck.summary(),
                    now,
                    last,
                    startup_grace_until
                ),
            );

            // Try to recover the bridge daemon first (owned by launchd).
            let recover_reason = match observation.state {
                BridgeOriginState::HungConfirmed => "watchdog_origin_hung_confirmed",
                BridgeOriginState::WrongOwner => "watchdog_origin_wrong_owner",
                _ => "watchdog_origin_down",
            };
            if ensure_bridge_daemon_owns_port(8080, recover_reason).await {
                persist_last_self_heal_at(unix_now_secs());
                log_important!(
                    info,
                    "[Watchdog] bridge daemon available or recovered, skipping GUI restart"
                );
                continue;
            }

            LAST_FAILED_BRIDGE_RECOVER_AT.store(unix_now_secs(), Ordering::SeqCst);

            if observation.state == BridgeOriginState::HungConfirmed {
                log_important!(
                    warn,
                    "[Watchdog] 8080 origin hung confirmed but bridge daemon recovery failed"
                );
                continue;
            }

            log_important!(
                warn,
                "[Watchdog] 检测到 8080 origin 异常，重启 iterate 应用"
            );
            tokio::time::sleep(Duration::from_secs(1)).await;
            instance_debug_log("[watchdog-restart-app]", "calling app_handle.restart()");
            app_handle.restart();
        }
    });
}

#[cfg(target_os = "windows")]
fn start_connectivity_watchdog(app_handle: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(WATCHDOG_INTERVAL_SECS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut consecutive_failures = 0u32;
        let mut was_unhealthy = false;

        loop {
            interval.tick().await;
            let healthy = tauri::async_runtime::spawn_blocking(|| bridge_http_healthy(8080))
                .await
                .unwrap_or(false);

            if healthy {
                consecutive_failures = 0;
                if was_unhealthy {
                    log_important!(info, "[Watchdog] Windows Bridge 已恢复");
                    publish_startup_status(&app_handle, StartupStatus::ready());
                }
                was_unhealthy = false;
                continue;
            }

            consecutive_failures = consecutive_failures.saturating_add(1);
            if consecutive_failures < 3 {
                continue;
            }

            consecutive_failures = 0;
            was_unhealthy = true;
            log_important!(
                warn,
                "[Watchdog] Windows Bridge 连续三次不可用，尝试仅恢复 Bridge"
            );
            publish_startup_status(&app_handle, StartupStatus::starting("Bridge 正在自动恢复"));

            let bridge_app = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(error) = start_bridge_server(bridge_app, 8080).await {
                    log_important!(warn, "[Watchdog] Bridge 恢复启动失败: {}", error);
                }
            });

            for delay_secs in [1u64, 2, 4] {
                tokio::time::sleep(Duration::from_secs(delay_secs)).await;
                let recovered = tauri::async_runtime::spawn_blocking(|| bridge_http_healthy(8080))
                    .await
                    .unwrap_or(false);
                if recovered {
                    break;
                }
            }

            let recovered = tauri::async_runtime::spawn_blocking(|| bridge_http_healthy(8080))
                .await
                .unwrap_or(false);
            if !recovered {
                publish_startup_status(
                    &app_handle,
                    StartupStatus::degraded("Bridge 暂不可用，可点击重试"),
                );
            }
        }
    });
}

/// 应用设置和初始化
fn is_standalone_process() -> bool {
    let args: Vec<String> = std::env::args().collect();
    std::env::var("ITERATE_STANDALONE_MODE").is_ok()
        || std::env::var("ITERATE_MCP_REQUEST_FILE").is_ok()
        || args.get(1).is_some_and(|arg| arg == "--mcp-request")
}

async fn wait_for_bridge_ready() -> bool {
    for _ in 0..20 {
        let healthy = tauri::async_runtime::spawn_blocking(|| bridge_http_healthy(8080))
            .await
            .unwrap_or(false);
        if healthy {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    false
}

pub fn start_application_setup(app_handle: AppHandle) {
    publish_startup_status(
        &app_handle,
        StartupStatus::starting("正在初始化应用和后台服务"),
    );
    tauri::async_runtime::spawn(async move {
        match setup_application(&app_handle).await {
            Ok(()) if is_standalone_process() || wait_for_bridge_ready().await => {
                publish_startup_status(&app_handle, StartupStatus::ready());
            }
            Ok(()) => {
                publish_startup_status(
                    &app_handle,
                    StartupStatus::degraded("界面已就绪，Bridge 暂不可用，可点击重试"),
                );
            }
            Err(error) => {
                log_important!(error, "应用初始化失败: {}", error);
                publish_startup_status(
                    &app_handle,
                    StartupStatus::degraded(format!("后台服务启动失败: {}", error)),
                );
            }
        }
    });
}

#[tauri::command]
pub async fn retry_background_services(app_handle: AppHandle) -> Result<StartupStatus, String> {
    if BACKGROUND_RETRY_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        return Ok(get_startup_status());
    }

    publish_startup_status(
        &app_handle,
        StartupStatus::starting("正在重试 Bridge 后台服务"),
    );

    if !bridge_http_healthy(8080) {
        let bridge_app = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(error) = start_bridge_server(bridge_app, 8080).await {
                log_important!(warn, "手动重试 Bridge 启动失败: {}", error);
            }
        });
    }

    let ready = wait_for_bridge_ready().await;
    BACKGROUND_RETRY_IN_FLIGHT.store(false, Ordering::SeqCst);
    let status = if ready {
        StartupStatus::ready()
    } else {
        StartupStatus::degraded("Bridge 仍不可用，请检查 8080 端口占用")
    };
    publish_startup_status(&app_handle, status.clone());
    Ok(status)
}

pub async fn setup_application(app_handle: &AppHandle) -> Result<(), String> {
    let state = app_handle.state::<AppState>();

    // Standalone 子进程（--serve 启动的 GUI 弹窗）不启动 bridge/browser/IPC server，
    // 这些服务由主 app 进程管理。子进程抢占 8080 端口会导致 iOS WS 断连。
    let args: Vec<String> = std::env::args().collect();
    let is_standalone = is_standalone_process();
    instance_debug_log(
        "[setup-begin]",
        format!("is_standalone={}, args={:?}", is_standalone, args),
    );

    let speech_role = crate::native_speech::owner::SpeechProcessRole::from_runtime(
        args.iter().map(String::as_str),
        is_standalone,
    );
    #[cfg(target_os = "macos")]
    if speech_role.is_owner_eligible() {
        crate::native_speech::start_phase1_runtime(app_handle.clone(), speech_role);
    }

    if !is_standalone {
        // 检查 daemon 是否已在 8080 运行（--bridge-only 模式）
        #[cfg(target_os = "windows")]
        let daemon_owns_port = false;
        #[cfg(not(target_os = "windows"))]
        let daemon_owns_port = ensure_bridge_daemon_owns_port(8080, "setup_application").await;
        if daemon_owns_port {
            log_important!(
                info,
                "[Bridge] daemon already owns 8080; GUI attaching without binding"
            );
            instance_debug_log(
                "[setup-bridge]",
                "bridge_runtime_mode=attached_to_daemon".to_string(),
            );
        } else {
            // 启动 Bridge Server (HTTP 8080)
            let app_handle_clone = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = start_bridge_server(app_handle_clone, 8080).await {
                    log_important!(error, "Bridge Server 启动失败: {}", e);
                }
            });
        }
        start_connectivity_watchdog(app_handle.clone());

        // 启动 Browser AI Monitor (WebSocket 9333)
        let app_handle_browser = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            let _ = crate::browser::start_browser_monitoring(app_handle_browser, None).await;
        });

        // 启动 IPC Server
        let app_handle_clone = app_handle.clone();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = start_ipc_server(app_handle_clone).await {
                log_important!(warn, "IPC Server 启动失败: {}", e);
            }
        });
    } else {
        log_important!(info, "Standalone 模式：跳过 Bridge/Browser/IPC Server 启动");
    }

    // 加载配置并应用窗口设置
    if let Err(e) = load_config_and_apply_window_settings(&state, app_handle).await {
        log_important!(warn, "加载配置失败: {}", e);
    }

    if migrate_legacy_custom_audio(&state, app_handle)
        .await
        .is_err()
    {
        log_important!(warn, "迁移旧版自定义提示音失败，已保持安全静音回退");
    }

    // 初始化音频资源管理器
    if let Err(e) = initialize_audio_asset_manager(app_handle) {
        log_important!(warn, "初始化音频资源管理器失败: {}", e);
    }

    #[cfg(not(target_os = "windows"))]
    {
        // Preserve the established macOS/Linux initialization order. Windows
        // installs these listeners before its asynchronous setup begins.
        setup_window_event_listeners(app_handle);
        if let Err(error) = setup_exit_handlers(app_handle) {
            log_important!(warn, "设置退出处理器失败: {}", error);
        }
    }

    // 注册全局快捷键：Shift+Cmd+K 截图
    #[cfg(target_os = "macos")]
    {
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};
        // 防抖：记录上次截图时间，500ms 内不重复触发
        static LAST_SCREENSHOT_TIME: AtomicU64 = AtomicU64::new(0);

        let app_handle_clone = app_handle.clone();
        if let Ok(shortcut) = "Shift+Cmd+K".parse::<Shortcut>() {
            let _ = app_handle.global_shortcut().on_shortcut(
                shortcut,
                move |_app, _shortcut, event| {
                    // 只在按键按下时触发，避免按下和释放时重复触发
                    if event.state != ShortcutState::Pressed {
                        return;
                    }

                    // 防抖检查：500ms 内不重复触发
                    let now = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0);
                    let last = LAST_SCREENSHOT_TIME.load(Ordering::SeqCst);
                    if now - last < 500 {
                        return;
                    }
                    LAST_SCREENSHOT_TIME.store(now, Ordering::SeqCst);

                    let handle = app_handle_clone.clone();
                    tauri::async_runtime::spawn(async move {
                        // 获取最后聚焦的窗口，只操作该窗口
                        let target_label = get_last_focused_window();
                        let target_window = target_label
                            .as_ref()
                            .and_then(|label| handle.get_webview_window(label));

                        // 检查目标窗口是否聚焦，只有聚焦的窗口才响应截图
                        let is_focused = target_window
                            .as_ref()
                            .and_then(|w| w.is_focused().ok())
                            .unwrap_or(false);
                        if !is_focused {
                            return; // 不是当前聚焦的窗口，忽略截图请求
                        }

                        // 截图前隐藏目标窗口
                        if let Some(ref window) = target_window {
                            let _ = window.hide();
                        }

                        // 等待窗口隐藏动画完成
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

                        // 调用截图命令
                        let screenshot_result = crate::ui::commands::capture_screenshot().await;

                        // 截图后显示目标窗口
                        if let Some(ref window) = target_window {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }

                        // 发送截图到前端（只发给目标窗口）
                        if let Ok(screenshot_data) = screenshot_result {
                            if let Some(ref label) = target_label {
                                let _ =
                                    handle.emit_to(label, "screenshot-captured", screenshot_data);
                            } else {
                                let _ = handle.emit("screenshot-captured", screenshot_data);
                            }
                        }
                    });
                },
            );
            log_important!(info, "全局快捷键 Shift+Cmd+K 已注册");
        }
    }

    #[cfg(target_os = "windows")]
    {
        use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

        if let Err(error) = crate::native_speech::overlay::ensure_windows_overlay(app_handle) {
            log::warn!("Windows 语音浮层初始化失败: {}", error);
        }

        let app_handle_clone = app_handle.clone();
        if let Ok(shortcut) =
            crate::native_speech::windows::WINDOWS_SPEECH_SHORTCUT.parse::<Shortcut>()
        {
            let _ =
                app_handle
                    .global_shortcut()
                    .on_shortcut(shortcut, move |app, _shortcut, event| {
                        if event.state != ShortcutState::Pressed {
                            return;
                        }
                        let enabled = app
                            .state::<AppState>()
                            .global_shortcut_enabled
                            .load(Ordering::Relaxed);
                        if !enabled {
                            return;
                        }

                        if let Err(error) = crate::native_speech::windows::start_windows_dictation(
                            app_handle_clone.clone(),
                        ) {
                            log::warn!("Windows 全局语音启动失败: {}", error);
                        }
                    });
            log_important!(
                info,
                "Windows 全局语音快捷键 {} 已注册",
                crate::native_speech::windows::WINDOWS_SPEECH_SHORTCUT
            );
        }
    }

    Ok(())
}
