use serde::{Deserialize, Serialize};
#[cfg(target_os = "macos")]
use std::collections::HashMap;
use std::ffi::{c_char, c_void, CString};
#[cfg(target_os = "macos")]
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(target_os = "macos")]
use std::os::unix::fs::MetadataExt;
#[cfg(target_os = "macos")]
use std::os::unix::io::AsRawFd;
#[cfg(target_os = "macos")]
use std::os::unix::net::{UnixListener, UnixStream};
#[cfg(target_os = "macos")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, Size};

pub mod coordinator;
pub mod external_sender;
pub mod fn_listener;
pub mod hud_animation;
pub mod native_backend;
pub mod overlay;
pub mod owner;
pub mod phase1;
#[cfg(target_os = "macos")]
pub mod runtime_paths;
pub mod session;
pub mod target;
pub mod trace;
#[cfg(target_os = "windows")]
pub mod windows;

pub use phase1::{
    ack_speech_overlay_visibility, complete_speech_processing, configure_speech_recognition,
    get_speech_control_snapshot, start_phase1_runtime,
};

pub const SPEECH_OVERLAY_WINDOW_LABEL: &str = "speech-overlay";

const INSERT_TEXT_EVENT: &str = "speech://insert-text";
const OVERLAY_WIDTH: f64 = 96.0;
const OVERLAY_HEIGHT: f64 = 48.0;
const OVERLAY_BOTTOM_MARGIN: f64 = 34.0;
const OWN_BUNDLE_ID: &str = "com.kexin94yyds.iterate";
const LOG_PATH: &str = "/tmp/iterate-native-speech.log";
#[cfg(target_os = "macos")]
const SPEECH_TARGET_REGISTRY_FILE: &str = "iterate_speech_targets.json";

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
static LAST_SPEECH_TARGET: OnceLock<Mutex<Option<SpeechTarget>>> = OnceLock::new();
static ACTIVE_POPUP_SPEECH_TARGET: OnceLock<Mutex<Option<PopupSpeechTarget>>> = OnceLock::new();
static PASTE_IN_PROGRESS: AtomicBool = AtomicBool::new(false);
static CODEX_LIVE_AUDIO_RESERVED: AtomicBool = AtomicBool::new(false);
static PASTE_SEQUENCE_ID: AtomicU64 = AtomicU64::new(1);
static SPEECH_RUNTIME_STATE: OnceLock<Mutex<SpeechRuntimeEventState>> = OnceLock::new();
#[cfg(target_os = "macos")]
static POPUP_SPEECH_IPC: OnceLock<PopupSpeechIpc> = OnceLock::new();
#[cfg(target_os = "macos")]
static POPUP_SPEECH_IPC_INIT: OnceLock<Mutex<()>> = OnceLock::new();
#[cfg(target_os = "macos")]
static FN_OWNER_STATE: OnceLock<Mutex<FnOwnerState>> = OnceLock::new();

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PopupSpeechTarget {
    window_label: String,
    request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ipc_socket_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ipc_token: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct PopupSpeechTargetView {
    window_label: String,
    request_id: String,
    pid: Option<u32>,
    project_path: Option<String>,
    reason: Option<String>,
    updated_at: Option<String>,
}

impl From<PopupSpeechTarget> for PopupSpeechTargetView {
    fn from(target: PopupSpeechTarget) -> Self {
        Self {
            window_label: target.window_label,
            request_id: target.request_id,
            pid: target.pid,
            project_path: target.project_path,
            reason: target.reason,
            updated_at: target.updated_at,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct PopupSpeechTargetRegistry {
    #[serde(default)]
    targets: Vec<PopupSpeechTarget>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SpeechInsertTextPayload {
    pub identity: session::SpeechLayerIdentity,
    pub request_id: String,
    pub window_label: String,
    pub text: String,
    pub mode: String,
    pub insert_id: String,
}

#[cfg(target_os = "macos")]
struct PopupSpeechIpc {
    socket_path: PathBuf,
    token: String,
    acknowledgements: std::sync::Arc<Mutex<HashMap<String, PendingPopupIpcAck>>>,
}

#[cfg(target_os = "macos")]
struct PendingPopupIpcAck {
    identity: session::SpeechLayerIdentity,
    request_id: String,
    window_label: String,
    text_len: usize,
    sender: std::sync::mpsc::Sender<usize>,
}

#[cfg(target_os = "macos")]
fn pending_popup_ipc_insert_matches(
    pending: &PendingPopupIpcAck,
    identity: session::SpeechLayerIdentity,
    request_id: &str,
    window_label: &str,
    text_len: usize,
) -> bool {
    pending.identity == identity
        && pending.request_id == request_id.trim()
        && pending.window_label == window_label.trim()
        && pending.text_len == text_len
}

#[cfg(target_os = "macos")]
#[derive(Serialize, Deserialize)]
struct PopupSpeechIpcRequest {
    token: String,
    payload: SpeechInsertTextPayload,
}

#[cfg(target_os = "macos")]
#[derive(Serialize, Deserialize)]
struct PopupSpeechIpcResponse {
    ok: bool,
    identity: session::SpeechLayerIdentity,
    request_id: String,
    window_label: String,
    insert_id: String,
    text_len: Option<usize>,
    error: Option<String>,
}

pub(crate) struct PopupSpeechWriteback {
    pub window_label: String,
    pub payload: SpeechInsertTextPayload,
}

pub(crate) enum SpeechWritebackDispatch {
    ExternalAcknowledged,
    ExternalDispatchedUnverified,
    ExternalUnknownAfterDispatch,
    Popup(PopupSpeechWriteback),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExternalPasteDispatch {
    DispatchedUnverified,
    UnknownAfterDispatch,
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct PopupSpeechIpcDispatchError {
    dispatched: bool,
    message: String,
}

#[cfg(target_os = "macos")]
impl PopupSpeechIpcDispatchError {
    fn before_dispatch(message: impl Into<String>) -> Self {
        Self {
            dispatched: false,
            message: message.into(),
        }
    }

    fn after_dispatch(message: impl Into<String>) -> Self {
        Self {
            dispatched: true,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug)]
enum SpeechTarget {
    ExternalApp {
        bundle_id: String,
        pid: Option<i32>,
        #[cfg(target_os = "macos")]
        focus_evidence: CapturedFocusEvidence,
    },
    IteratePopupInput {
        window_label: String,
        request_id: String,
        pid: Option<i32>,
    },
}

impl SpeechTarget {
    fn kind(&self) -> &'static str {
        match self {
            SpeechTarget::ExternalApp { .. } => "external-app",
            SpeechTarget::IteratePopupInput { .. } => "iterate-popup-input",
        }
    }

    fn bundle_id(&self) -> Option<&str> {
        match self {
            SpeechTarget::ExternalApp { bundle_id, .. } => Some(bundle_id),
            SpeechTarget::IteratePopupInput { .. } => None,
        }
    }

    fn pid(&self) -> Option<i32> {
        match self {
            SpeechTarget::ExternalApp { pid, .. } => *pid,
            SpeechTarget::IteratePopupInput { pid, .. } => *pid,
        }
    }

    fn summary(&self) -> String {
        match self {
            SpeechTarget::ExternalApp { bundle_id, pid, .. } => format!(
                "kind={} bundle_id={} pid={}",
                self.kind(),
                bundle_id,
                pid.map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string())
            ),
            SpeechTarget::IteratePopupInput {
                window_label,
                request_id,
                pid,
            } => format!(
                "kind={} window_label={} request_id={} pid={}",
                self.kind(),
                window_label,
                request_id,
                pid.map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string())
            ),
        }
    }
}

#[derive(Clone, Default)]
struct SpeechRuntimeEventState {
    last_event: Option<String>,
    last_event_at: Option<String>,
    recognition_mode: Option<String>,
    last_partial_length: Option<usize>,
    last_final_length: Option<usize>,
    last_error: Option<String>,
    last_paste_status: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct SpeechRuntimePermissions {
    pub microphone: bool,
    pub speech_recognition: bool,
    pub input_monitoring: bool,
    pub accessibility: bool,
}

#[derive(Clone, Serialize)]
pub struct SpeechRuntimeOwner {
    pub fn_listener_owner: bool,
    pub owner_pid: Option<u32>,
    pub owner_bundle_id: Option<String>,
    pub owner_path: Option<String>,
    pub owner_team_id: Option<String>,
    pub owner_cdhash: Option<String>,
    pub owner_exe_mtime: Option<String>,
    pub owner_acquired_at: Option<String>,
    pub owner_is_current_process: bool,
    pub owner_matches_current_binary: Option<bool>,
    pub current_pid: u32,
    pub current_path: Option<String>,
    pub current_team_id: Option<String>,
    pub current_cdhash: Option<String>,
    pub current_exe_mtime: Option<String>,
    pub lock_path: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct SpeechRuntimeOverlay {
    pub window_exists: bool,
    pub window_visible: bool,
    pub listener_ready: bool,
    pub pending_toggle: bool,
}

#[derive(Clone, Serialize)]
pub struct SpeechRuntimeSpeech {
    pub active: bool,
    pub recognition_mode: Option<String>,
    pub last_partial_length: Option<usize>,
    pub last_final_length: Option<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NativeSpeechRecognitionMode {
    Quality,
    Privacy,
}

impl NativeSpeechRecognitionMode {
    fn from_option(value: Option<String>) -> Self {
        match value.as_deref().map(str::trim) {
            Some("privacy") => Self::Privacy,
            _ => Self::Quality,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Quality => "quality",
            Self::Privacy => "privacy",
        }
    }

    fn force_on_device(self) -> bool {
        self == Self::Privacy
    }
}

#[cfg(test)]
mod speech_recognition_mode_tests {
    use super::*;

    #[test]
    fn defaults_to_quality_mode() {
        assert_eq!(
            NativeSpeechRecognitionMode::from_option(None),
            NativeSpeechRecognitionMode::Quality
        );
        assert_eq!(
            NativeSpeechRecognitionMode::from_option(Some("".to_string())),
            NativeSpeechRecognitionMode::Quality
        );
    }

    #[test]
    fn preserves_explicit_privacy_mode() {
        let mode = NativeSpeechRecognitionMode::from_option(Some("privacy".to_string()));

        assert_eq!(mode, NativeSpeechRecognitionMode::Privacy);
        assert!(mode.force_on_device());
    }
}

#[derive(Clone, Serialize)]
pub struct SpeechRuntimeWriteback {
    pub last_target_kind: Option<String>,
    pub last_target_bundle_id: Option<String>,
    pub last_target_pid: Option<i32>,
    pub last_target_window_label: Option<String>,
    pub last_target_request_id: Option<String>,
    pub active_popup_window_label: Option<String>,
    pub active_popup_request_id: Option<String>,
    pub registered_popup_target_count: usize,
    pub latest_registered_popup_pid: Option<u32>,
    pub latest_registered_popup_window_label: Option<String>,
    pub latest_registered_popup_request_id: Option<String>,
    pub last_paste_status: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct SpeechRuntimeDiagnostics {
    pub log_path: &'static str,
    pub last_event: Option<String>,
    pub last_event_at: Option<String>,
}

#[derive(Clone, Serialize)]
pub struct SpeechRuntimeStatus {
    pub permissions: SpeechRuntimePermissions,
    pub owner: SpeechRuntimeOwner,
    pub overlay: SpeechRuntimeOverlay,
    pub speech: SpeechRuntimeSpeech,
    pub writeback: SpeechRuntimeWriteback,
    pub diagnostics: SpeechRuntimeDiagnostics,
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct FnOwnerState {
    lock: Option<FnOwnerLock>,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Default)]
struct FnOwnerMetadata {
    pid: Option<u32>,
    bundle_id: Option<String>,
    exe_path: Option<String>,
    exe_mtime: Option<String>,
    team_id: Option<String>,
    cdhash: Option<String>,
    acquired_at: Option<String>,
}

#[cfg(target_os = "macos")]
struct FnOwnerLock {
    _file: File,
}

#[cfg(target_os = "macos")]
impl Drop for FnOwnerLock {
    fn drop(&mut self) {
        let _ = unsafe { flock(self._file.as_raw_fd(), LOCK_UN) };
    }
}

#[cfg(target_os = "macos")]
#[derive(Clone, Default)]
struct FocusedProcessInfo {
    pid: i32,
    bundle_id: Option<String>,
    name: Option<String>,
}

#[cfg(target_os = "macos")]
type AXUIElementRef = *const c_void;

#[cfg(target_os = "macos")]
struct RetainedAxElement {
    raw: AXUIElementRef,
}

#[cfg(target_os = "macos")]
#[derive(Clone, Debug)]
enum CapturedFocusEvidence {
    Exact(RetainedAxElement),
    FrontmostPidFallback,
}

#[cfg(target_os = "macos")]
impl CapturedFocusEvidence {
    fn dispatch_mode(&self) -> FocusDispatchMode {
        match self {
            Self::Exact(_) => FocusDispatchMode::Exact,
            Self::FrontmostPidFallback => FocusDispatchMode::FrontmostPidFallback,
        }
    }
}

#[cfg(target_os = "macos")]
impl RetainedAxElement {
    fn from_owned(raw: AXUIElementRef) -> Result<Self, String> {
        if raw.is_null() {
            Err("AX focused element was null".to_string())
        } else {
            Ok(Self { raw })
        }
    }

    fn pid(&self) -> Result<i32, String> {
        let mut pid = 0;
        let error = unsafe { AXUIElementGetPid(self.raw, &mut pid) };
        if error != AX_ERROR_SUCCESS || pid <= 0 {
            Err(format!("AXUIElementGetPid error={error} pid={pid}"))
        } else {
            Ok(pid)
        }
    }

    fn matches(&self, other: &Self) -> bool {
        use core_foundation::base::{CFEqual, CFTypeRef};

        unsafe { CFEqual(self.raw as CFTypeRef, other.raw as CFTypeRef) != 0 }
    }
}

#[cfg(target_os = "macos")]
impl Clone for RetainedAxElement {
    fn clone(&self) -> Self {
        use core_foundation::base::{CFRetain, CFTypeRef};

        let raw = unsafe { CFRetain(self.raw as CFTypeRef) } as AXUIElementRef;
        Self { raw }
    }
}

#[cfg(target_os = "macos")]
impl std::fmt::Debug for RetainedAxElement {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RetainedAxElement")
            .field("raw", &self.raw)
            .finish()
    }
}

#[cfg(target_os = "macos")]
impl Drop for RetainedAxElement {
    fn drop(&mut self) {
        use core_foundation::base::{CFRelease, CFTypeRef};

        unsafe {
            CFRelease(self.raw as CFTypeRef);
        }
    }
}

// AXUIElementRef is an immutable Core Foundation proxy. The retained reference may be
// passed between the short-lived target-capture and writeback worker threads.
#[cfg(target_os = "macos")]
unsafe impl Send for RetainedAxElement {}
#[cfg(target_os = "macos")]
unsafe impl Sync for RetainedAxElement {}

#[cfg(target_os = "macos")]
type AXError = i32;

#[cfg(target_os = "macos")]
const AX_ERROR_SUCCESS: AXError = 0;
#[cfg(target_os = "macos")]
const AX_ERROR_NO_VALUE: AXError = -25212;

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct FocusedElementCaptureError {
    ax_error: Option<AXError>,
    message: String,
}

#[cfg(target_os = "macos")]
impl FocusedElementCaptureError {
    fn ax_copy(error: AXError) -> Self {
        Self {
            ax_error: Some(error),
            message: format!("AXFocusedUIElement error={error}"),
        }
    }

    fn other(message: impl Into<String>) -> Self {
        Self {
            ax_error: None,
            message: message.into(),
        }
    }

    fn allows_frontmost_pid_fallback(&self) -> bool {
        self.ax_error == Some(AX_ERROR_NO_VALUE)
    }
}

#[cfg(target_os = "macos")]
impl std::fmt::Display for FocusedElementCaptureError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

struct AtomicFlagGuard {
    flag: &'static AtomicBool,
}

impl Drop for AtomicFlagGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

fn mark_paste_in_progress() -> AtomicFlagGuard {
    PASTE_IN_PROGRESS.store(true, Ordering::SeqCst);
    AtomicFlagGuard {
        flag: &PASTE_IN_PROGRESS,
    }
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn speech_bridge_main_bundle_has_usage_description(usage_key: *const c_char) -> bool;
    fn speech_bridge_check_microphone_authorization() -> bool;
    fn speech_bridge_request_microphone_authorization() -> bool;
    fn speech_bridge_check_speech_authorization() -> bool;
    fn speech_bridge_request_speech_authorization() -> bool;
}

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn flock(fd: std::os::raw::c_int, operation: std::os::raw::c_int) -> std::os::raw::c_int;
}

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXUIElementCreateSystemWide() -> AXUIElementRef;
    fn AXUIElementCopyAttributeValue(
        element: AXUIElementRef,
        attribute: core_foundation::string::CFStringRef,
        value: *mut core_foundation::base::CFTypeRef,
    ) -> AXError;
    fn AXUIElementSetAttributeValue(
        element: AXUIElementRef,
        attribute: core_foundation::string::CFStringRef,
        value: core_foundation::base::CFTypeRef,
    ) -> AXError;
    fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> AXError;
}

#[cfg(target_os = "macos")]
const LOCK_EX: std::os::raw::c_int = 2;
#[cfg(target_os = "macos")]
const LOCK_NB: std::os::raw::c_int = 4;
#[cfg(target_os = "macos")]
const LOCK_UN: std::os::raw::c_int = 8;

pub fn set_app_handle(app_handle: AppHandle) {
    let _ = APP_HANDLE.set(app_handle);
}

fn speech_runtime_state() -> &'static Mutex<SpeechRuntimeEventState> {
    SPEECH_RUNTIME_STATE.get_or_init(|| Mutex::new(SpeechRuntimeEventState::default()))
}

fn now_rfc3339() -> String {
    chrono::Local::now().to_rfc3339()
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn current_process_pid_i32() -> Option<i32> {
    i32::try_from(std::process::id()).ok()
}

fn popup_target_pid_i32(target: &PopupSpeechTarget) -> Option<i32> {
    target.pid.and_then(|pid| i32::try_from(pid).ok())
}

#[cfg(target_os = "macos")]
fn speech_target_registry_path() -> PathBuf {
    std::env::temp_dir().join(SPEECH_TARGET_REGISTRY_FILE)
}

fn popup_speech_target_process_is_alive(pid: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        process_is_alive(pid)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = pid;
        true
    }
}

fn cleanup_popup_speech_target_registry(registry: &mut PopupSpeechTargetRegistry) {
    registry.targets.retain(|target| {
        target
            .pid
            .map(popup_speech_target_process_is_alive)
            .unwrap_or(false)
            && !target.window_label.trim().is_empty()
            && !target.request_id.trim().is_empty()
    });
}

#[cfg(target_os = "macos")]
fn read_popup_speech_target_registry() -> PopupSpeechTargetRegistry {
    let path = speech_target_registry_path();
    let uid = unsafe { libc::geteuid() };
    let Ok(mut file) = runtime_paths::open_private_lock_file(&path, uid) else {
        return PopupSpeechTargetRegistry::default();
    };
    let mut content = String::new();
    if file.read_to_string(&mut content).is_err() {
        return PopupSpeechTargetRegistry::default();
    }

    match serde_json::from_str::<PopupSpeechTargetRegistry>(&content) {
        Ok(mut registry) => {
            cleanup_popup_speech_target_registry(&mut registry);
            registry
        }
        Err(error) => {
            debug_log(
                "[speech-target-registry-read-failed]",
                format!("path={} error={error}", path.display()),
            );
            PopupSpeechTargetRegistry::default()
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn read_popup_speech_target_registry() -> PopupSpeechTargetRegistry {
    PopupSpeechTargetRegistry::default()
}

#[cfg(target_os = "macos")]
fn write_popup_speech_target_registry(registry: &PopupSpeechTargetRegistry) -> Result<(), String> {
    let path = speech_target_registry_path();
    let content = serde_json::to_string_pretty(registry)
        .map_err(|error| format!("failed to serialize speech target registry: {error}"))?;
    let uid = unsafe { libc::geteuid() };
    let mut file = runtime_paths::open_private_lock_file(&path, uid).map_err(|error| {
        format!(
            "failed to open speech target registry {}: {error}",
            path.display()
        )
    })?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| format!("failed to seek speech target registry: {error}"))?;
    file.set_len(0)
        .map_err(|error| format!("failed to truncate speech target registry: {error}"))?;
    file.write_all(content.as_bytes()).map_err(|error| {
        format!(
            "failed to write speech target registry {}: {error}",
            path.display()
        )
    })?;
    file.sync_all()
        .map_err(|error| format!("failed to sync speech target registry: {error}"))
}

#[cfg(not(target_os = "macos"))]
fn write_popup_speech_target_registry(_registry: &PopupSpeechTargetRegistry) -> Result<(), String> {
    Ok(())
}

fn upsert_registered_popup_speech_target(target: PopupSpeechTarget) -> Result<(), String> {
    let pid = target
        .pid
        .ok_or_else(|| "popup speech target pid is required".to_string())?;
    let mut registry = read_popup_speech_target_registry();
    registry
        .targets
        .retain(|item| item.pid != Some(pid) || item.request_id != target.request_id);
    registry.targets.push(target);
    cleanup_popup_speech_target_registry(&mut registry);
    write_popup_speech_target_registry(&registry)
}

fn remove_registered_popup_speech_target(request_id: &str) -> Result<(), String> {
    let pid = std::process::id();
    let request_id = request_id.trim();
    let mut registry = read_popup_speech_target_registry();
    registry.targets.retain(|target| {
        if target.pid != Some(pid) {
            return true;
        }
        if request_id.is_empty() {
            return false;
        }
        target.request_id != request_id
    });
    cleanup_popup_speech_target_registry(&mut registry);
    write_popup_speech_target_registry(&registry)
}

fn registered_popup_speech_target_for_pid(pid: i32) -> Option<PopupSpeechTarget> {
    let pid = u32::try_from(pid).ok()?;
    let mut registry = read_popup_speech_target_registry();
    cleanup_popup_speech_target_registry(&mut registry);
    let _ = write_popup_speech_target_registry(&registry);
    registry
        .targets
        .into_iter()
        .filter(|target| target.pid == Some(pid))
        .max_by_key(|target| target.updated_at.clone().unwrap_or_default())
}

#[cfg(target_os = "macos")]
fn bind_private_popup_speech_listener(path: &Path) -> Result<UnixListener, String> {
    use std::os::unix::fs::PermissionsExt;

    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path)
        .map_err(|error| format!("failed to bind popup speech IPC: {error}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("failed to secure popup speech IPC: {error}"))?;
    Ok(listener)
}

#[cfg(target_os = "macos")]
fn ensure_popup_speech_ipc(app: AppHandle) -> Result<&'static PopupSpeechIpc, String> {
    if let Some(ipc) = POPUP_SPEECH_IPC.get() {
        return Ok(ipc);
    }
    let _initialization = POPUP_SPEECH_IPC_INIT
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| "popup speech IPC initialization is poisoned".to_string())?;
    if let Some(ipc) = POPUP_SPEECH_IPC.get() {
        return Ok(ipc);
    }

    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let socket_path = PathBuf::from("/tmp").join(format!(
        "it-sp-{}-{}.sock",
        std::process::id(),
        &nonce[..12]
    ));
    let listener = bind_private_popup_speech_listener(&socket_path)?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    let acknowledgements = std::sync::Arc::new(Mutex::new(HashMap::new()));
    POPUP_SPEECH_IPC
        .set(PopupSpeechIpc {
            socket_path: socket_path.clone(),
            token: token.clone(),
            acknowledgements: acknowledgements.clone(),
        })
        .map_err(|_| "popup speech IPC initialized concurrently".to_string())?;

    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else {
                continue;
            };
            let app = app.clone();
            let token = token.clone();
            let acknowledgements = acknowledgements.clone();
            thread::spawn(move || {
                if let Err(error) = handle_popup_speech_ipc(stream, app, &token, acknowledgements) {
                    debug_log("[popup-ipc-request-failed]", error);
                }
            });
        }
        let _ = std::fs::remove_file(socket_path);
    });

    POPUP_SPEECH_IPC
        .get()
        .ok_or_else(|| "popup speech IPC unavailable".to_string())
}

#[cfg(target_os = "macos")]
fn handle_popup_speech_ipc(
    mut stream: UnixStream,
    app: AppHandle,
    expected_token: &str,
    acknowledgements: std::sync::Arc<Mutex<HashMap<String, PendingPopupIpcAck>>>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| error.to_string())?;

    let mut encoded = String::new();
    stream
        .read_to_string(&mut encoded)
        .map_err(|error| format!("failed to read popup IPC request: {error}"))?;
    let request: PopupSpeechIpcRequest = serde_json::from_str(&encoded)
        .map_err(|error| format!("invalid popup IPC request: {error}"))?;
    if request.token != expected_token {
        return Err("popup IPC token mismatch".to_string());
    }

    let active_target = current_active_popup_speech_target()
        .ok_or_else(|| "popup IPC target is no longer active".to_string())?;
    if active_target.request_id != request.payload.request_id
        || active_target.window_label != request.payload.window_label
        || active_target.pid != Some(std::process::id())
    {
        return Err("popup IPC request does not match the active target".to_string());
    }

    let identity = request.payload.identity;
    let request_id = request.payload.request_id.clone();
    let window_label = request.payload.window_label.clone();
    let insert_id = request.payload.insert_id.clone();
    let text_len = request.payload.text.chars().count();
    let (sender, receiver) = std::sync::mpsc::channel();
    {
        let mut pending = acknowledgements
            .lock()
            .map_err(|_| "popup IPC acknowledgements poisoned".to_string())?;
        if pending.contains_key(&insert_id) {
            return Err("popup IPC insert is already pending".to_string());
        }
        pending.insert(
            insert_id.clone(),
            PendingPopupIpcAck {
                identity,
                request_id: request_id.clone(),
                window_label: window_label.clone(),
                text_len,
                sender,
            },
        );
    }

    if let Err(error) = app.emit_to(&window_label, INSERT_TEXT_EVENT, request.payload) {
        acknowledgements
            .lock()
            .ok()
            .map(|mut pending| pending.remove(&insert_id));
        return Err(format!("failed to emit popup IPC insert: {error}"));
    }

    let response = match receiver.recv_timeout(Duration::from_secs(4)) {
        Ok(acknowledged_text_len) => PopupSpeechIpcResponse {
            ok: acknowledged_text_len == text_len,
            identity,
            request_id,
            window_label,
            insert_id: insert_id.clone(),
            text_len: Some(acknowledged_text_len),
            error: (acknowledged_text_len != text_len)
                .then(|| "popup IPC acknowledgement text length mismatch".to_string()),
        },
        Err(error) => PopupSpeechIpcResponse {
            ok: false,
            identity,
            request_id,
            window_label,
            insert_id: insert_id.clone(),
            text_len: None,
            error: Some(format!("popup IPC acknowledgement timed out: {error}")),
        },
    };
    acknowledgements
        .lock()
        .ok()
        .map(|mut pending| pending.remove(&insert_id));
    stream
        .write_all(&serde_json::to_vec(&response).map_err(|error| error.to_string())?)
        .map_err(|error| format!("failed to write popup IPC response: {error}"))
}

fn update_runtime_state(mut update: impl FnMut(&mut SpeechRuntimeEventState)) {
    if let Ok(mut state) = speech_runtime_state().lock() {
        update(&mut state);
    }
}

fn record_runtime_event(event: &str) {
    update_runtime_state(|state| {
        state.last_event = Some(event.to_string());
        state.last_event_at = Some(now_rfc3339());
    });
}

fn record_partial_length(length: usize) {
    update_runtime_state(|state| {
        state.last_event = Some("native-partial".to_string());
        state.last_event_at = Some(now_rfc3339());
        state.last_partial_length = Some(length);
    });
}

fn record_final_length(length: usize) {
    update_runtime_state(|state| {
        state.last_event = Some("native-final".to_string());
        state.last_event_at = Some(now_rfc3339());
        state.last_final_length = Some(length);
    });
}

fn record_runtime_error(error: impl AsRef<str>) {
    let error = error.as_ref().to_string();
    update_runtime_state(|state| {
        state.last_event = Some("error".to_string());
        state.last_event_at = Some(now_rfc3339());
        state.last_error = Some(error.clone());
    });
}

fn record_paste_status(status: &str) {
    update_runtime_state(|state| {
        state.last_event = Some(status.to_string());
        state.last_event_at = Some(now_rfc3339());
        state.last_paste_status = Some(status.to_string());
    });
}

fn record_paste_error(error: impl AsRef<str>) {
    let error = error.as_ref().to_string();
    update_runtime_state(|state| {
        state.last_event = Some("paste-error".to_string());
        state.last_event_at = Some(now_rfc3339());
        state.last_paste_status = Some("error".to_string());
        state.last_error = Some(error.clone());
    });
}

#[cfg(target_os = "macos")]
static FN_OWNER_SUPERVISOR_STARTED: AtomicBool = AtomicBool::new(false);

pub fn start_fn_listener(app_handle: AppHandle) {
    #[cfg(target_os = "macos")]
    {
        if FN_OWNER_SUPERVISOR_STARTED.swap(true, Ordering::SeqCst) {
            debug_log(
                "[fn-owner-supervisor-existing]",
                "Fn owner supervisor already started in this process",
            );
            return;
        }

        thread::spawn(move || loop {
            let owner_lock = match acquire_fn_owner_lock() {
                Ok(Some(lock)) => lock,
                Ok(None) => {
                    debug_log(
                        "[fn-owner-retry]",
                        "Fn listener owner is busy; retrying in one second",
                    );
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
                Err(error) => {
                    eprintln!("[iterate:speech] failed to acquire Fn owner lock: {error}");
                    debug_log("[fn-owner-lock-failed]", error);
                    thread::sleep(Duration::from_secs(1));
                    continue;
                }
            };

            let mut state = match fn_owner_state().lock() {
                Ok(state) => state,
                Err(_) => {
                    debug_log("[fn-owner-state-poisoned]", "failed to lock Fn owner state");
                    return;
                }
            };
            if state.lock.is_some() {
                return;
            }
            state.lock = Some(owner_lock);
            record_runtime_event("fn-owner-acquired");
            drop(state);

            fn_listener::start(app_handle);
            return;
        });
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = app_handle;
    }
}

pub(crate) fn debug_log(tag: &str, message: impl AsRef<str>) {
    let line = format!(
        "{} [native-speech:{}] {} {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        std::process::id(),
        tag,
        message.as_ref()
    );
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(LOG_PATH) {
        let _ = file.write_all(line.as_bytes());
    }
}

pub fn trigger_global_toggle(_app_handle: &AppHandle) {
    if CODEX_LIVE_AUDIO_RESERVED.load(Ordering::Relaxed) {
        debug_log(
            "[fn-toggle-suppressed]",
            "Codex GPT-Live currently owns the microphone",
        );
        record_runtime_event("fn-toggle-suppressed-codex-live");
        return;
    }
    if let Err(error) = phase1::request_toggle() {
        debug_log("[phase1-toggle-failed]", error);
    }
}

pub(crate) fn codex_live_audio_reserved() -> bool {
    CODEX_LIVE_AUDIO_RESERVED.load(Ordering::Relaxed)
}

fn last_speech_target() -> &'static Mutex<Option<SpeechTarget>> {
    LAST_SPEECH_TARGET.get_or_init(|| Mutex::new(None))
}

fn active_popup_speech_target() -> &'static Mutex<Option<PopupSpeechTarget>> {
    ACTIVE_POPUP_SPEECH_TARGET.get_or_init(|| Mutex::new(None))
}

fn current_speech_target() -> Option<SpeechTarget> {
    last_speech_target()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
}

fn current_active_popup_speech_target() -> Option<PopupSpeechTarget> {
    active_popup_speech_target()
        .lock()
        .ok()
        .and_then(|guard| guard.clone())
}

#[cfg(target_os = "macos")]
fn fn_owner_state() -> &'static Mutex<FnOwnerState> {
    FN_OWNER_STATE.get_or_init(|| Mutex::new(FnOwnerState::default()))
}

fn clear_last_target_app_bundle_id() {
    if let Ok(mut guard) = last_speech_target().lock() {
        *guard = None;
    }
}

#[cfg(target_os = "macos")]
fn fn_owner_lock_path() -> PathBuf {
    runtime_paths::production_owner_lock_path().unwrap_or_else(|error| {
        debug_log("[fn-owner-runtime-path-failed]", error.to_string());
        PathBuf::from("/dev/null/iterate-speech-owner-unavailable")
    })
}

#[cfg(target_os = "macos")]
fn acquire_fn_owner_lock() -> Result<Option<FnOwnerLock>, String> {
    let lock_path = fn_owner_lock_path();
    acquire_fn_owner_lock_at(&lock_path)
}

#[cfg(target_os = "macos")]
fn acquire_fn_owner_lock_at(lock_path: &Path) -> Result<Option<FnOwnerLock>, String> {
    let uid = unsafe { libc::geteuid() };
    let mut file = runtime_paths::open_private_lock_file(lock_path, uid).map_err(|error| {
        format!(
            "failed to open Fn owner lock file {}: {error}",
            lock_path.display()
        )
    })?;

    let lock_result = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
    if lock_result != 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::WouldBlock {
            debug_log(
                "[fn-owner-busy]",
                format!("lock_path={} error={}", lock_path.display(), error),
            );
            return Ok(None);
        }

        return Err(format!(
            "failed to acquire Fn owner lock {}: {}",
            lock_path.display(),
            error
        ));
    }

    if !fn_owner_lock_matches_path(&file, lock_path) {
        debug_log(
            "[fn-owner-inode-replaced]",
            format!("lock_path={}", lock_path.display()),
        );
        return Ok(None);
    }

    let previous_metadata = read_fn_owner_metadata_from_file(&mut file);
    let metadata = current_fn_owner_metadata();
    log_fn_owner_replacement(&lock_path, previous_metadata.as_ref(), &metadata);
    write_fn_owner_metadata(&mut file, &metadata);

    debug_log(
        "[fn-owner-acquired]",
        format!(
            "lock_path={} pid={} exe_path={} cdhash={}",
            lock_path.display(),
            metadata
                .pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            metadata.exe_path.as_deref().unwrap_or("unknown"),
            metadata.cdhash.as_deref().unwrap_or("unknown"),
        ),
    );

    Ok(Some(FnOwnerLock { _file: file }))
}

#[cfg(target_os = "macos")]
fn fn_owner_lock_matches_path(file: &File, lock_path: &Path) -> bool {
    let Ok(file_metadata) = file.metadata() else {
        return false;
    };
    let Ok(path_metadata) = std::fs::metadata(lock_path) else {
        return false;
    };
    file_metadata.dev() == path_metadata.dev() && file_metadata.ino() == path_metadata.ino()
}

#[cfg(target_os = "macos")]
fn is_own_bundle_id(bundle_id: &str) -> bool {
    bundle_id == OWN_BUNDLE_ID
}

#[cfg(target_os = "macos")]
fn process_command_line(pid: i32) -> Option<String> {
    let output = Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }

    let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if command.is_empty() {
        None
    } else {
        Some(command)
    }
}

#[cfg(target_os = "macos")]
fn process_is_alive(pid: u32) -> bool {
    if pid == std::process::id() {
        return true;
    }

    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };

    process_command_line(pid).is_some()
}

#[cfg(target_os = "macos")]
fn is_mcp_request_process(pid: i32) -> bool {
    process_command_line(pid)
        .map(|command| command.contains(" --mcp-request ") || command.contains("\t--mcp-request "))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn should_preserve_retained_target_for_own_main_process(
    retained_target: Option<&SpeechTarget>,
    frontmost_pid: i32,
    is_mcp_request: bool,
) -> bool {
    !is_mcp_request
        && retained_target
            .and_then(SpeechTarget::pid)
            .is_some_and(|retained_pid| retained_pid != frontmost_pid)
}

#[cfg(target_os = "macos")]
fn frontmost_app_identity() -> Result<(String, Option<i32>), String> {
    let application = target::capture_frontmost_application()?;
    Ok((application.bundle_id, Some(application.pid)))
}

#[cfg(target_os = "macos")]
fn app_process_identity_by_pid(pid: i32) -> (Option<String>, Option<String>) {
    let script = format!(
        r#"tell application "System Events"
  try
    set targetProc to first application process whose unix id is {pid}
    set procBundle to ""
    set procName to ""
    try
      set procBundle to bundle identifier of targetProc as text
    end try
    try
      set procName to name of targetProc as text
    end try
    return procBundle & "\t" & procName
  on error
    return "\t"
  end try
end tell"#
    );

    let Ok(output) = Command::new("osascript").arg("-e").arg(script).output() else {
        return (None, None);
    };
    if !output.status.success() {
        return (None, None);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut parts = stdout.trim_end().splitn(2, '\t');
    let bundle_id = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    let name = parts
        .next()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);

    (bundle_id, name)
}

#[cfg(target_os = "macos")]
fn copy_system_focused_element() -> Result<RetainedAxElement, FocusedElementCaptureError> {
    use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
    use core_foundation::string::CFString;

    let system_wide = unsafe { AXUIElementCreateSystemWide() };
    if system_wide.is_null() {
        return Err(FocusedElementCaptureError::other(
            "AXUIElementCreateSystemWide returned null",
        ));
    }

    let attribute = CFString::new("AXFocusedUIElement");
    let mut focused_value: CFTypeRef = std::ptr::null();
    let copy_error = unsafe {
        AXUIElementCopyAttributeValue(
            system_wide,
            attribute.as_concrete_TypeRef(),
            &mut focused_value,
        )
    };
    unsafe {
        CFRelease(system_wide as CFTypeRef);
    }

    if copy_error != AX_ERROR_SUCCESS || focused_value.is_null() {
        return Err(FocusedElementCaptureError::ax_copy(copy_error));
    }

    RetainedAxElement::from_owned(focused_value as AXUIElementRef)
        .map_err(FocusedElementCaptureError::other)
}

#[cfg(target_os = "macos")]
fn capture_focused_element_for_pid(
    expected_pid: i32,
) -> Result<RetainedAxElement, FocusedElementCaptureError> {
    let focused_element = copy_system_focused_element()?;
    let actual_pid = focused_element
        .pid()
        .map_err(FocusedElementCaptureError::other)?;
    if actual_pid != expected_pid {
        return Err(FocusedElementCaptureError::other(format!(
            "focused element pid mismatch expected={expected_pid} actual={actual_pid}"
        )));
    }

    Ok(focused_element)
}

#[cfg(target_os = "macos")]
fn verify_captured_focused_element(
    focused_element: &RetainedAxElement,
    expected_pid: i32,
) -> Result<(), String> {
    let current_element = copy_system_focused_element().map_err(|error| error.to_string())?;
    let current_pid = current_element.pid()?;
    if current_pid != expected_pid || !focused_element.matches(&current_element) {
        return Err(format!(
            "focused element verification failed expected_pid={expected_pid} actual_pid={current_pid} exact_match={}",
            focused_element.matches(&current_element)
        ));
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn restore_captured_focused_element(
    focused_element: &RetainedAxElement,
    expected_pid: i32,
    paste_id: u64,
) -> Result<(), String> {
    use core_foundation::base::TCFType;
    use core_foundation::boolean::CFBoolean;
    use core_foundation::string::CFString;

    let actual_pid = focused_element.pid()?;
    if actual_pid != expected_pid {
        return Err(format!(
            "captured focused element is stale expected_pid={expected_pid} actual_pid={actual_pid}"
        ));
    }

    let focused_attribute = CFString::new("AXFocused");
    let focused_value = CFBoolean::true_value();
    let set_error = unsafe {
        AXUIElementSetAttributeValue(
            focused_element.raw,
            focused_attribute.as_concrete_TypeRef(),
            focused_value.as_CFTypeRef(),
        )
    };
    if set_error != AX_ERROR_SUCCESS {
        return Err(format!("AXFocused set error={set_error}"));
    }

    thread::sleep(Duration::from_millis(60));
    verify_captured_focused_element(focused_element, expected_pid)?;

    debug_log(
        "[paste-focused-element-restored]",
        format!("id={paste_id} pid={expected_pid} exact_match=true"),
    );
    Ok(())
}

#[cfg(target_os = "macos")]
fn system_focused_process_info() -> Result<FocusedProcessInfo, String> {
    let focused_element = copy_system_focused_element().map_err(|error| error.to_string())?;
    let pid = focused_element.pid()?;

    let (bundle_id, name) = app_process_identity_by_pid(pid);
    Ok(FocusedProcessInfo {
        pid,
        bundle_id,
        name,
    })
}

#[cfg(target_os = "macos")]
fn focused_process_summary(info: &FocusedProcessInfo) -> String {
    format!(
        "pid={} bundle_id={} name={}",
        info.pid,
        info.bundle_id.as_deref().unwrap_or("<unknown>"),
        info.name.as_deref().unwrap_or("<unknown>")
    )
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PasteDispatchRoute {
    AnnotatedSession,
    Abort(&'static str),
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusDispatchMode {
    Exact,
    FrontmostPidFallback,
}

#[cfg(target_os = "macos")]
fn select_paste_dispatch_route(
    frontmost_pid: Option<i32>,
    focused_process: Option<&FocusedProcessInfo>,
    target_bundle_id: &str,
    focus_mode: FocusDispatchMode,
) -> PasteDispatchRoute {
    if frontmost_pid.is_none() {
        return PasteDispatchRoute::Abort("missing-frontmost-pid");
    }

    if let Some(info) = focused_process {
        if info
            .bundle_id
            .as_deref()
            .map(is_own_bundle_id)
            .unwrap_or(false)
        {
            return PasteDispatchRoute::Abort("focused-own-app");
        }
        if let Some(focused_bundle_id) = info.bundle_id.as_deref() {
            if focused_bundle_id != target_bundle_id {
                return PasteDispatchRoute::Abort("focused-bundle-mismatch");
            }
        }
    }

    match focus_mode {
        // Both paths have already confirmed that the captured target is still
        // the frontmost application immediately before dispatch.  Posting on
        // the annotated session tap reaches WebView controls that ignore the
        // per-PID event route when AX does not expose their focused element.
        FocusDispatchMode::Exact | FocusDispatchMode::FrontmostPidFallback => {
            PasteDispatchRoute::AnnotatedSession
        }
    }
}

#[cfg(target_os = "macos")]
fn log_paste_dispatch_route(
    route: PasteDispatchRoute,
    frontmost_pid: Option<i32>,
    focused_process: Option<&FocusedProcessInfo>,
    target_bundle_id: &str,
    focus_mode: FocusDispatchMode,
    paste_id: u64,
) {
    let focused_pid = focused_process
        .map(|info| info.pid.to_string())
        .unwrap_or_else(|| "<none>".to_string());
    let focused_bundle_id = focused_process
        .and_then(|info| info.bundle_id.as_deref())
        .unwrap_or("<unknown>");
    debug_log(
        "[paste-dispatch-route]",
        format!(
            "id={} route={:?} focus_mode={:?} frontmost_pid={} focused_pid={} focused_bundle_id={} target_bundle_id={}",
            paste_id,
            route,
            focus_mode,
            frontmost_pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "<none>".to_string()),
            focused_pid,
            focused_bundle_id,
            target_bundle_id,
        ),
    );
}

pub fn capture_frontmost_target_app() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let (bundle_id, frontmost_pid) = frontmost_app_identity()?;
        let mut guard = last_speech_target()
            .lock()
            .map_err(|_| "failed to lock speech target store".to_string())?;
        debug_log(
            "[target-capture-frontmost]",
            format!(
                "bundle_id={} pid={} retained_target={}",
                bundle_id,
                frontmost_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string()),
                guard
                    .as_ref()
                    .map(SpeechTarget::summary)
                    .unwrap_or_else(|| "<none>".to_string())
            ),
        );
        if is_own_bundle_id(&bundle_id) {
            if let Some(pid) = frontmost_pid {
                if let Some(target) = current_active_popup_speech_target()
                    .filter(|target| popup_target_pid_i32(target) == Some(pid))
                    .or_else(|| registered_popup_speech_target_for_pid(pid))
                {
                    *guard = Some(SpeechTarget::IteratePopupInput {
                        window_label: target.window_label.clone(),
                        request_id: target.request_id.clone(),
                        pid: Some(pid),
                    });
                    debug_log(
                        "[target-captured]",
                        format!(
                            "kind=iterate-popup-input source=registry window_label={} request_id={} pid={}",
                            target.window_label, target.request_id, pid
                        ),
                    );
                    return Ok(());
                }

                let is_mcp_request = is_mcp_request_process(pid);
                if should_preserve_retained_target_for_own_main_process(
                    guard.as_ref(),
                    pid,
                    is_mcp_request,
                ) {
                    debug_log(
                        "[target-capture-preserved]",
                        format!(
                            "reason=own-main-process-overlay frontmost_pid={} retained_target={}",
                            pid,
                            guard
                                .as_ref()
                                .map(SpeechTarget::summary)
                                .unwrap_or_else(|| "<none>".to_string())
                        ),
                    );
                    return Ok(());
                }
                let focused_element = capture_focused_element_for_pid(pid).map_err(|error| {
                    debug_log(
                        "[target-capture-rejected-focus]",
                        format!("bundle_id={bundle_id} pid={pid} error={error}"),
                    );
                    format!("failed to capture the focused input for pid {pid}: {error}")
                })?;
                *guard = Some(SpeechTarget::ExternalApp {
                    bundle_id: bundle_id.clone(),
                    pid: Some(pid),
                    focus_evidence: CapturedFocusEvidence::Exact(focused_element),
                });
                debug_log(
                    "[target-captured]",
                    format!(
                        "kind=own-app-paste-target bundle_id={bundle_id} pid={pid} is_mcp_request={is_mcp_request}"
                    ),
                );
                return Ok(());
            }

            if let Some(target) = current_active_popup_speech_target() {
                *guard = Some(SpeechTarget::IteratePopupInput {
                    window_label: target.window_label.clone(),
                    request_id: target.request_id.clone(),
                    pid: popup_target_pid_i32(&target),
                });
                debug_log(
                    "[target-captured]",
                    format!(
                        "kind=iterate-popup-input source=local window_label={} request_id={} pid={}",
                        target.window_label,
                        target.request_id,
                        popup_target_pid_i32(&target)
                            .map(|pid| pid.to_string())
                            .unwrap_or_else(|| "<unknown>".to_string())
                    ),
                );
                return Ok(());
            }

            *guard = None;
            debug_log(
                "[target-capture-rejected-self]",
                format!("bundle_id={bundle_id} reason=own-app-without-pid"),
            );
            return Err("frontmost app is iterate but no target pid was found".to_string());
        }

        let focus_evidence = frontmost_pid
            .map(|pid| match capture_focused_element_for_pid(pid) {
                Ok(focused_element) => Ok(CapturedFocusEvidence::Exact(focused_element)),
                Err(error) if error.allows_frontmost_pid_fallback() => {
                    debug_log(
                        "[target-capture-focus-fallback]",
                        format!("bundle_id={bundle_id} pid={pid} mode=frontmost-pid error={error}"),
                    );
                    Ok(CapturedFocusEvidence::FrontmostPidFallback)
                }
                Err(error) => {
                    debug_log(
                        "[target-capture-rejected-focus]",
                        format!("bundle_id={bundle_id} pid={pid} error={error}"),
                    );
                    Err(format!(
                        "failed to capture the focused input for pid {pid}: {error}"
                    ))
                }
            })
            .transpose()?;
        let focus_evidence = focus_evidence.ok_or_else(|| {
            *guard = None;
            "frontmost external app did not provide a PID".to_string()
        })?;
        *guard = Some(SpeechTarget::ExternalApp {
            bundle_id: bundle_id.clone(),
            pid: frontmost_pid,
            focus_evidence: focus_evidence.clone(),
        });
        eprintln!("[iterate:speech] captured target app={bundle_id}");
        debug_log(
            "[target-captured]",
            format!(
                "kind=external-app bundle_id={} pid={} focus_mode={:?}",
                bundle_id,
                frontmost_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string()),
                focus_evidence.dispatch_mode(),
            ),
        );
        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn activate_app(bundle_id: &str) -> Result<(), String> {
    let pid = current_speech_target()
        .filter(|target| target.bundle_id() == Some(bundle_id))
        .and_then(|target| target.pid())
        .unwrap_or_default();
    target::activate_application(&target::FrontmostApplication {
        bundle_id: bundle_id.to_owned(),
        pid,
    })
}

#[cfg(target_os = "macos")]
fn simulate_paste(route: PasteDispatchRoute, paste_id: u64) -> Result<(), String> {
    use core_graphics::event::{CGEvent, CGEventFlags, CGEventTapLocation, CGKeyCode};
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    const KEY_CODE_V: CGKeyCode = 9;

    let source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
        .map_err(|_| "failed to create macOS event source".to_string())?;

    let key_down = CGEvent::new_keyboard_event(source.clone(), KEY_CODE_V, true)
        .map_err(|_| "failed to create Cmd+V key down event".to_string())?;
    key_down.set_flags(CGEventFlags::CGEventFlagCommand);

    let key_up = CGEvent::new_keyboard_event(source, KEY_CODE_V, false)
        .map_err(|_| "failed to create Cmd+V key up event".to_string())?;
    key_up.set_flags(CGEventFlags::CGEventFlagCommand);

    match route {
        PasteDispatchRoute::AnnotatedSession => {
            debug_log(
                "[paste-post-method]",
                format!("id={paste_id} method=annotated-session"),
            );
            key_down.post(CGEventTapLocation::AnnotatedSession);
            key_up.post(CGEventTapLocation::AnnotatedSession);
        }
        PasteDispatchRoute::Abort(reason) => {
            return Err(format!("paste dispatch aborted: {reason}"));
        }
    }
    thread::sleep(Duration::from_millis(60));

    Ok(())
}

#[cfg(target_os = "macos")]
fn read_clipboard_text() -> Option<String> {
    let output = Command::new("pbpaste").output().ok()?;
    if output.status.success() {
        Some(String::from_utf8_lossy(&output.stdout).to_string())
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn write_clipboard_text(text: &str) -> Result<(), String> {
    let mut child = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| format!("failed to start pbcopy: {error}"))?;

    let Some(mut stdin) = child.stdin.take() else {
        return Err("failed to open pbcopy stdin".to_string());
    };
    stdin
        .write_all(text.as_bytes())
        .map_err(|error| format!("failed to write clipboard text: {error}"))?;
    drop(stdin);

    let status = child
        .wait()
        .map_err(|error| format!("failed to wait for pbcopy: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("pbcopy returned non-zero status".to_string())
    }
}

#[cfg(target_os = "macos")]
fn check_accessibility_permission() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
    }

    unsafe { AXIsProcessTrusted() }
}

#[cfg(target_os = "macos")]
fn open_accessibility_settings() -> Result<(), String> {
    const CANDIDATES: &[&str] = &[
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
        "x-apple.systempreferences:com.apple.settings.extensions.PrivacySecurity.extension?Privacy_Accessibility",
    ];

    for url in CANDIDATES {
        if Command::new("open")
            .arg(url)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
    }

    Command::new("open")
        .args(["-a", "System Settings"])
        .status()
        .map_err(|error| format!("failed to open System Settings: {error}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_speech_recognition_settings() -> Result<(), String> {
    const CANDIDATES: &[&str] = &[
        "x-apple.systempreferences:com.apple.preference.security?Privacy_SpeechRecognition",
        "x-apple.systempreferences:com.apple.settings.extensions.PrivacySecurity.extension?Privacy_SpeechRecognition",
    ];

    for url in CANDIDATES {
        if Command::new("open")
            .arg(url)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
    }

    Command::new("open")
        .args(["-a", "System Settings"])
        .status()
        .map_err(|error| format!("failed to open System Settings: {error}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_microphone_settings() -> Result<(), String> {
    const CANDIDATES: &[&str] = &[
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone",
        "x-apple.systempreferences:com.apple.settings.extensions.PrivacySecurity.extension?Privacy_Microphone",
    ];

    for url in CANDIDATES {
        if Command::new("open")
            .arg(url)
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
        {
            return Ok(());
        }
    }

    Command::new("open")
        .args(["-a", "System Settings"])
        .status()
        .map_err(|error| format!("failed to open System Settings: {error}"))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_app_info_plist_for_executable(executable_path: &Path) -> Option<PathBuf> {
    let macos_dir = executable_path.parent()?;
    if macos_dir.file_name().and_then(|name| name.to_str()) != Some("MacOS") {
        return None;
    }

    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name().and_then(|name| name.to_str()) != Some("Contents") {
        return None;
    }

    let app_dir = contents_dir.parent()?;
    if app_dir.extension().and_then(|extension| extension.to_str()) != Some("app") {
        return None;
    }

    let info_plist = contents_dir.join("Info.plist");
    if info_plist.is_file() {
        Some(info_plist)
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
fn current_macos_app_info_plist() -> Option<PathBuf> {
    let executable_path = std::env::current_exe().ok()?;
    macos_app_info_plist_for_executable(&executable_path)
}

#[cfg(target_os = "macos")]
fn ensure_privacy_usage_description_available(
    usage_key: &str,
    permission_label: &str,
) -> Result<(), String> {
    let current_path = current_exe_path_string().unwrap_or_else(|| "unknown".to_string());
    let Some(info_plist) = current_macos_app_info_plist() else {
        let message = format!(
            "当前 iterate 进程不是完整 macOS .app bundle，不能直接请求{permission_label}权限；macOS 会因为缺少 {usage_key} 上下文终止进程。请从 /Applications/iterate.app 启动主 App 后再请求，或在系统设置中手动授权。current_exe={current_path}"
        );
        debug_log("[privacy-request-blocked]", &message);
        return Err(message);
    };

    let contents = std::fs::read_to_string(&info_plist).map_err(|error| {
        format!(
            "读取 {} 失败，不能请求{permission_label}权限: {error}",
            info_plist.display()
        )
    })?;
    let needle = format!("<key>{usage_key}</key>");
    if contents.contains(&needle) {
        let usage_key_c = CString::new(usage_key).map_err(|error| {
            format!("无效的 {usage_key} usage key，不能请求{permission_label}权限: {error}")
        })?;
        let main_bundle_has_usage =
            unsafe { speech_bridge_main_bundle_has_usage_description(usage_key_c.as_ptr()) };
        if main_bundle_has_usage {
            return Ok(());
        }

        let message = format!(
            "当前 iterate 进程的 main bundle 没有暴露 {usage_key}，不能请求{permission_label}权限；macOS TCC 可能会终止进程。current_exe={current_path}, info_plist={}",
            info_plist.display()
        );
        debug_log("[privacy-request-blocked]", &message);
        return Err(message);
    }

    let message = format!(
        "{} 缺少 {usage_key}，不能请求{permission_label}权限；macOS 可能会终止进程。",
        info_plist.display()
    );
    debug_log("[privacy-request-blocked]", &message);
    Err(message)
}

#[cfg(target_os = "macos")]
fn check_input_monitoring_permission() -> bool {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightListenEventAccess() -> bool;
    }

    unsafe { CGPreflightListenEventAccess() }
}

#[cfg(target_os = "macos")]
fn request_input_monitoring_access() -> Result<(), String> {
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn CGRequestListenEventAccess() -> bool;
    }

    let _ = unsafe { CGRequestListenEventAccess() };
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_bump_overlay_window_level(window: &tauri::WebviewWindow) {
    use cocoa::appkit::NSWindow;
    use cocoa::base::id;

    let Ok(ptr) = window.ns_window() else {
        return;
    };
    if ptr.is_null() {
        return;
    }

    unsafe {
        let win = ptr as id;
        NSWindow::setLevel_(win, 101);
    }
}

fn reveal_overlay(app_handle: &AppHandle) -> Result<(), String> {
    let Some(window) = app_handle.get_webview_window(SPEECH_OVERLAY_WINDOW_LABEL) else {
        return Err("speech overlay window not found".to_string());
    };

    let window_for_main_thread = window.clone();
    window
        .run_on_main_thread(move || {
            let already_visible = window_for_main_thread.is_visible().unwrap_or(false);

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            {
                let _ = window_for_main_thread.set_always_on_top(true);
                #[cfg(target_os = "macos")]
                macos_bump_overlay_window_level(&window_for_main_thread);
                let _ = window_for_main_thread.set_visible_on_all_workspaces(true);
                let _ = window_for_main_thread.set_skip_taskbar(true);
            }
            let _ = window_for_main_thread.set_size(Size::Logical(LogicalSize::new(
                OVERLAY_WIDTH,
                OVERLAY_HEIGHT,
            )));

            if !already_visible {
                if let Ok(Some(monitor)) = window_for_main_thread.current_monitor() {
                    let monitor_size = monitor.size();
                    let scale_factor = monitor.scale_factor();
                    let x = ((monitor_size.width as f64 - OVERLAY_WIDTH * scale_factor) / 2.0)
                        .round() as i32;
                    let y = (monitor_size.height as f64
                        - OVERLAY_HEIGHT * scale_factor
                        - OVERLAY_BOTTOM_MARGIN * scale_factor)
                        .round() as i32;
                    let _ = window_for_main_thread
                        .set_position(Position::Physical(PhysicalPosition::new(x, y)));
                }
            }

            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            let _ = window_for_main_thread.unminimize();
            let _ = window_for_main_thread.show();
        })
        .map_err(|error| format!("failed to reveal speech overlay: {error}"))?;

    Ok(())
}

fn hide_overlay(app_handle: &AppHandle) -> Result<(), String> {
    let Some(window) = app_handle.get_webview_window(SPEECH_OVERLAY_WINDOW_LABEL) else {
        return Ok(());
    };
    window
        .hide()
        .map_err(|error| format!("failed to hide speech overlay: {error}"))
}

#[tauri::command]
pub fn reveal_speech_overlay_window(_app: AppHandle) -> Result<(), String> {
    phase1::request_desired_state(session::DesiredState::On)
}

#[tauri::command]
pub fn hide_speech_overlay_window(_app: AppHandle) -> Result<(), String> {
    phase1::request_desired_state(session::DesiredState::Off)
}

#[tauri::command]
pub fn remember_frontmost_app() -> Result<(), String> {
    capture_frontmost_target_app()
}

#[tauri::command]
pub fn get_captured_target_app_bundle_id() -> Option<String> {
    last_speech_target().lock().ok().and_then(|guard| {
        guard
            .as_ref()
            .and_then(|target| target.bundle_id().map(ToString::to_string))
    })
}

fn get_captured_target_kind() -> Option<String> {
    last_speech_target()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().map(|target| target.kind().to_string()))
}

fn get_captured_target_pid() -> Option<i32> {
    last_speech_target()
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(SpeechTarget::pid))
}

fn get_captured_target_window_label() -> Option<String> {
    last_speech_target().lock().ok().and_then(|guard| {
        guard.as_ref().and_then(|target| match target {
            SpeechTarget::IteratePopupInput { window_label, .. } => Some(window_label.clone()),
            SpeechTarget::ExternalApp { .. } => None,
        })
    })
}

fn get_captured_target_request_id() -> Option<String> {
    last_speech_target().lock().ok().and_then(|guard| {
        guard.as_ref().and_then(|target| match target {
            SpeechTarget::IteratePopupInput { request_id, .. } => Some(request_id.clone()),
            SpeechTarget::ExternalApp { .. } => None,
        })
    })
}

fn current_exe_path_string() -> Option<String> {
    std::env::current_exe()
        .ok()
        .map(|path| path.display().to_string())
}

#[cfg(target_os = "macos")]
fn file_mtime_unix_string(path: &str) -> Option<String> {
    std::fs::metadata(path)
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs().to_string())
}

#[cfg(target_os = "macos")]
fn codesign_identity(path: &str) -> (Option<String>, Option<String>) {
    let output = Command::new("codesign")
        .args(["-dv", "--verbose=4", path])
        .stdout(Stdio::null())
        .output();

    let Ok(output) = output else {
        return (None, None);
    };

    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut team_id = None;
    let mut cdhash = None;

    for line in stderr.lines() {
        if let Some(value) = line.strip_prefix("TeamIdentifier=") {
            team_id = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("CDHash=") {
            cdhash = Some(value.trim().to_string());
        }
    }

    (team_id, cdhash)
}

#[cfg(target_os = "macos")]
fn current_fn_owner_metadata() -> FnOwnerMetadata {
    let exe_path = current_exe_path_string();
    let exe_mtime = exe_path.as_deref().and_then(file_mtime_unix_string);
    let (team_id, cdhash) = exe_path
        .as_deref()
        .map(codesign_identity)
        .unwrap_or((None, None));

    FnOwnerMetadata {
        pid: Some(std::process::id()),
        bundle_id: Some(OWN_BUNDLE_ID.to_string()),
        exe_path,
        exe_mtime,
        team_id,
        cdhash,
        acquired_at: Some(chrono::Local::now().to_rfc3339()),
    }
}

#[cfg(target_os = "macos")]
fn write_fn_owner_metadata(file: &mut File, metadata: &FnOwnerMetadata) {
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.set_len(0);
    let fields = [
        ("pid", metadata.pid.map(|pid| pid.to_string())),
        ("bundle_id", metadata.bundle_id.clone()),
        ("exe_path", metadata.exe_path.clone()),
        ("exe_mtime", metadata.exe_mtime.clone()),
        ("team_id", metadata.team_id.clone()),
        ("cdhash", metadata.cdhash.clone()),
        ("acquired_at", metadata.acquired_at.clone()),
    ];

    for (key, value) in fields {
        if let Some(value) = value {
            let _ = writeln!(file, "{}={}", key, value);
        }
    }
    let _ = file.sync_all();
}

#[cfg(target_os = "macos")]
fn read_fn_owner_metadata() -> Option<FnOwnerMetadata> {
    let content = std::fs::read_to_string(fn_owner_lock_path()).ok()?;
    Some(parse_fn_owner_metadata(&content))
}

#[cfg(target_os = "macos")]
fn read_fn_owner_metadata_from_file(file: &mut File) -> Option<FnOwnerMetadata> {
    let mut content = String::new();
    file.seek(SeekFrom::Start(0)).ok()?;
    file.read_to_string(&mut content).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    Some(parse_fn_owner_metadata(&content))
}

#[cfg(target_os = "macos")]
fn parse_fn_owner_metadata(content: &str) -> FnOwnerMetadata {
    let mut metadata = FnOwnerMetadata::default();

    for token in content.lines().map(str::trim) {
        let Some((key, value)) = token.split_once('=') else {
            continue;
        };

        match key {
            "pid" => metadata.pid = value.parse::<u32>().ok(),
            "bundle_id" => metadata.bundle_id = Some(value.to_string()),
            "exe_path" => metadata.exe_path = Some(value.to_string()),
            "exe_mtime" => metadata.exe_mtime = Some(value.to_string()),
            "team_id" => metadata.team_id = Some(value.to_string()),
            "cdhash" => metadata.cdhash = Some(value.to_string()),
            "acquired_at" => metadata.acquired_at = Some(value.to_string()),
            _ => {}
        }
    }

    metadata
}

#[cfg(target_os = "macos")]
fn has_fn_owner_metadata(metadata: &FnOwnerMetadata) -> bool {
    metadata.pid.is_some()
        || metadata.bundle_id.is_some()
        || metadata.exe_path.is_some()
        || metadata.exe_mtime.is_some()
        || metadata.team_id.is_some()
        || metadata.cdhash.is_some()
        || metadata.acquired_at.is_some()
}

#[cfg(target_os = "macos")]
fn log_fn_owner_replacement(
    lock_path: &Path,
    previous: Option<&FnOwnerMetadata>,
    current: &FnOwnerMetadata,
) {
    let Some(previous) = previous else {
        return;
    };

    if !has_fn_owner_metadata(previous) {
        return;
    }

    if previous.pid == current.pid && previous.cdhash == current.cdhash {
        return;
    }

    let previous_pid = previous
        .pid
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let previous_exe_path = previous.exe_path.as_deref().unwrap_or("unknown");
    let previous_cdhash = previous.cdhash.as_deref().unwrap_or("unknown");
    let current_pid = current
        .pid
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let current_cdhash = current.cdhash.as_deref().unwrap_or("unknown");

    match previous.pid {
        Some(pid) if !process_is_alive(pid) => debug_log(
            "[fn-owner-stale-reaped]",
            format!(
                "lock_path={} previous_pid={} previous_exe_path={} previous_cdhash={} current_pid={} current_cdhash={}",
                lock_path.display(),
                previous_pid,
                previous_exe_path,
                previous_cdhash,
                current_pid,
                current_cdhash,
            ),
        ),
        Some(_) => debug_log(
            "[fn-owner-unlocked-replaced]",
            format!(
                "lock_path={} previous_pid={} previous_exe_path={} previous_cdhash={} current_pid={} current_cdhash={}",
                lock_path.display(),
                previous_pid,
                previous_exe_path,
                previous_cdhash,
                current_pid,
                current_cdhash,
            ),
        ),
        None => debug_log(
            "[fn-owner-metadata-replaced]",
            format!(
                "lock_path={} previous_exe_path={} previous_cdhash={} current_pid={} current_cdhash={}",
                lock_path.display(),
                previous_exe_path,
                previous_cdhash,
                current_pid,
                current_cdhash,
            ),
        ),
    }
}

#[cfg(target_os = "macos")]
fn owner_matches_current_binary(
    owner: &FnOwnerMetadata,
    current: &FnOwnerMetadata,
) -> Option<bool> {
    match (&owner.cdhash, &current.cdhash) {
        (Some(owner_cdhash), Some(current_cdhash)) => Some(owner_cdhash == current_cdhash),
        _ => match (&owner.exe_path, &current.exe_path) {
            (Some(owner_path), Some(current_path)) => Some(owner_path == current_path),
            _ => None,
        },
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn test_speech_identity() -> session::SpeechLayerIdentity {
        session::SpeechLayerIdentity::new(session::OwnerEpoch([7; 16]), 11, 13, 17)
    }

    fn focused_process(pid: i32, bundle_id: Option<&str>) -> FocusedProcessInfo {
        FocusedProcessInfo {
            pid,
            bundle_id: bundle_id.map(str::to_string),
            name: Some("focused-test-process".to_string()),
        }
    }

    #[test]
    fn external_paste_routes_to_the_verified_frontmost_first_responder() {
        let focused = focused_process(76225, Some("com.google.Chrome"));

        assert_eq!(
            select_paste_dispatch_route(
                Some(76052),
                Some(&focused),
                "com.google.Chrome",
                FocusDispatchMode::Exact,
            ),
            PasteDispatchRoute::AnnotatedSession,
        );
    }

    #[test]
    fn codex_paste_uses_hid_for_the_verified_webview_first_responder() {
        let focused = focused_process(3244, Some("com.openai.codex"));

        assert_eq!(
            select_paste_dispatch_route(
                Some(3244),
                Some(&focused),
                "com.openai.codex",
                FocusDispatchMode::Exact,
            ),
            PasteDispatchRoute::AnnotatedSession,
        );
    }

    #[test]
    fn external_paste_uses_hid_when_target_is_frontmost_but_ax_focus_is_unavailable() {
        assert_eq!(
            select_paste_dispatch_route(
                Some(76052),
                None,
                "com.openai.codex",
                FocusDispatchMode::Exact,
            ),
            PasteDispatchRoute::AnnotatedSession,
        );
    }

    #[test]
    fn codex_no_value_fallback_uses_annotated_session_after_frontmost_guard() {
        assert_eq!(
            select_paste_dispatch_route(
                Some(11268),
                None,
                "com.openai.codex",
                FocusDispatchMode::FrontmostPidFallback,
            ),
            PasteDispatchRoute::AnnotatedSession,
        );
    }

    #[test]
    fn only_ax_no_value_allows_frontmost_pid_fallback() {
        assert!(
            FocusedElementCaptureError::ax_copy(AX_ERROR_NO_VALUE).allows_frontmost_pid_fallback()
        );
        assert!(!FocusedElementCaptureError::ax_copy(-25211).allows_frontmost_pid_fallback());
        assert!(!FocusedElementCaptureError::other("focused pid mismatch")
            .allows_frontmost_pid_fallback());
    }

    #[test]
    fn external_paste_aborts_when_focus_belongs_to_iterate() {
        let focused = focused_process(94984, Some("com.kexin94yyds.iterate"));

        assert_eq!(
            select_paste_dispatch_route(
                Some(76052),
                Some(&focused),
                "com.openai.codex",
                FocusDispatchMode::FrontmostPidFallback,
            ),
            PasteDispatchRoute::Abort("focused-own-app"),
        );
    }

    #[test]
    fn external_paste_aborts_when_known_focus_belongs_to_another_app() {
        let focused = focused_process(44747, Some("com.google.Chrome"));

        assert_eq!(
            select_paste_dispatch_route(
                Some(76052),
                Some(&focused),
                "com.openai.codex",
                FocusDispatchMode::FrontmostPidFallback,
            ),
            PasteDispatchRoute::Abort("focused-bundle-mismatch"),
        );
    }

    #[test]
    fn external_paste_uses_hid_for_an_ax_owned_process_without_a_bundle_identity() {
        let focused = focused_process(76225, None);

        assert_eq!(
            select_paste_dispatch_route(
                Some(76052),
                Some(&focused),
                "com.google.Chrome",
                FocusDispatchMode::Exact,
            ),
            PasteDispatchRoute::AnnotatedSession,
        );
    }

    #[test]
    fn external_paste_aborts_without_an_exact_frontmost_target_pid() {
        assert_eq!(
            select_paste_dispatch_route(
                None,
                None,
                "com.openai.codex",
                FocusDispatchMode::FrontmostPidFallback,
            ),
            PasteDispatchRoute::Abort("missing-frontmost-pid"),
        );
    }

    #[test]
    fn external_paste_uses_hid_when_ax_returns_an_invalid_pid() {
        let focused = focused_process(0, Some("com.openai.codex"));

        assert_eq!(
            select_paste_dispatch_route(
                Some(76052),
                Some(&focused),
                "com.openai.codex",
                FocusDispatchMode::Exact,
            ),
            PasteDispatchRoute::AnnotatedSession,
        );
    }

    #[test]
    fn popup_speech_registry_round_trip_preserves_private_ipc_endpoint() {
        let target = PopupSpeechTarget {
            window_label: "main".to_string(),
            request_id: "serve-test".to_string(),
            pid: Some(std::process::id()),
            project_path: None,
            reason: Some("focus-input".to_string()),
            updated_at: Some("2026-07-11T23:20:00+08:00".to_string()),
            ipc_socket_path: Some("/tmp/iterate-popup-test.sock".to_string()),
            ipc_token: Some("private-token".to_string()),
        };

        let encoded = serde_json::to_string(&target).expect("serialize popup target");
        let decoded: PopupSpeechTarget =
            serde_json::from_str(&encoded).expect("deserialize popup target");

        assert_eq!(
            decoded.ipc_socket_path.as_deref(),
            Some("/tmp/iterate-popup-test.sock")
        );
        assert_eq!(decoded.ipc_token.as_deref(), Some("private-token"));

        let public_view = serde_json::to_string(&PopupSpeechTargetView::from(decoded))
            .expect("serialize public popup target view");
        assert!(!public_view.contains("private-token"));
        assert!(!public_view.contains("ipc_socket_path"));
        assert!(!public_view.contains("ipc_token"));
    }

    #[test]
    fn popup_speech_ipc_socket_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let path = PathBuf::from("/tmp").join(format!(
            "it-sp-mode-{}-{}.sock",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let listener = bind_private_popup_speech_listener(&path).expect("bind private listener");
        let mode = std::fs::metadata(&path)
            .expect("socket metadata")
            .permissions()
            .mode()
            & 0o777;

        assert_eq!(mode, 0o600);
        drop(listener);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cross_process_popup_dispatch_requires_matching_typed_ack() {
        use std::net::Shutdown;
        let path = PathBuf::from("/tmp").join(format!(
            "it-sp-ack-{}-{}.sock",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let listener = bind_private_popup_speech_listener(&path).expect("bind private listener");
        let identity = test_speech_identity();
        let payload = SpeechInsertTextPayload {
            identity,
            request_id: "serve-test".to_string(),
            window_label: "main".to_string(),
            text: "写入 iterate".to_string(),
            mode: "final".to_string(),
            insert_id: "insert-test".to_string(),
        };
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept owner connection");
            let mut encoded = String::new();
            stream
                .read_to_string(&mut encoded)
                .expect("read IPC request");
            let request: PopupSpeechIpcRequest =
                serde_json::from_str(&encoded).expect("decode IPC request");
            assert_eq!(request.token, "private-token");
            assert_eq!(request.payload.identity, identity);
            assert_eq!(request.payload.request_id, "serve-test");
            assert_eq!(request.payload.insert_id, "insert-test");
            let response = PopupSpeechIpcResponse {
                ok: true,
                identity,
                request_id: request.payload.request_id,
                window_label: request.payload.window_label,
                insert_id: request.payload.insert_id,
                text_len: Some(request.payload.text.chars().count()),
                error: None,
            };
            stream
                .write_all(&serde_json::to_vec(&response).expect("encode ACK"))
                .expect("write ACK");
            stream.shutdown(Shutdown::Write).expect("finish ACK");
        });

        dispatch_cross_process_popup(
            path.to_str().expect("socket path"),
            "private-token",
            payload,
        )
        .expect("matching typed ACK completes dispatch");
        server.join().expect("server join");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn popup_ipc_rejects_ack_for_a_different_insert() {
        let identity = test_speech_identity();
        let response = PopupSpeechIpcResponse {
            ok: true,
            identity,
            request_id: "serve-test".to_string(),
            window_label: "main".to_string(),
            insert_id: "wrong-insert".to_string(),
            text_len: Some(3),
            error: None,
        };

        let error = validate_popup_ipc_response(
            response,
            identity,
            "serve-test",
            "main",
            "expected-insert",
            3,
        )
        .expect_err("a mismatched insert id must not acknowledge writeback");

        assert!(error.contains("mismatched typed acknowledgement"));
    }

    #[test]
    fn popup_ipc_authority_requires_the_exact_pending_identity_and_lease() {
        let identity = test_speech_identity();
        let (sender, _receiver) = std::sync::mpsc::channel();
        let pending = PendingPopupIpcAck {
            identity,
            request_id: "serve-test".to_string(),
            window_label: "main".to_string(),
            text_len: 10,
            sender,
        };

        assert!(pending_popup_ipc_insert_matches(
            &pending,
            identity,
            "serve-test",
            "main",
            10,
        ));
        assert!(!pending_popup_ipc_insert_matches(
            &pending,
            session::SpeechLayerIdentity::new(session::OwnerEpoch([9; 16]), 11, 13, 17),
            "serve-test",
            "main",
            10,
        ));
        assert!(!pending_popup_ipc_insert_matches(
            &pending,
            identity,
            "serve-other",
            "main",
            10,
        ));
        assert!(!pending_popup_ipc_insert_matches(
            &pending,
            identity,
            "serve-test",
            "main",
            9,
        ));
    }

    #[test]
    fn popup_ipc_preserves_remote_timeout_error() {
        let identity = test_speech_identity();
        let response = PopupSpeechIpcResponse {
            ok: false,
            identity,
            request_id: "serve-test".to_string(),
            window_label: "main".to_string(),
            insert_id: "insert-test".to_string(),
            text_len: None,
            error: Some("popup ACK timeout".to_string()),
        };

        let error =
            validate_popup_ipc_response(response, identity, "serve-test", "main", "insert-test", 3)
                .expect_err("a negative ACK must fail dispatch");

        assert_eq!(error, "popup ACK timeout");
    }

    #[test]
    fn own_main_process_overlay_preserves_a_different_captured_target() {
        let retained = SpeechTarget::ExternalApp {
            bundle_id: "com.openai.codex".to_string(),
            pid: Some(5402),
            focus_evidence: CapturedFocusEvidence::FrontmostPidFallback,
        };

        assert!(should_preserve_retained_target_for_own_main_process(
            Some(&retained),
            788,
            false,
        ));
    }

    #[test]
    fn real_mcp_request_process_can_replace_the_retained_target() {
        let retained = SpeechTarget::ExternalApp {
            bundle_id: "com.openai.codex".to_string(),
            pid: Some(5402),
            focus_evidence: CapturedFocusEvidence::FrontmostPidFallback,
        };

        assert!(!should_preserve_retained_target_for_own_main_process(
            Some(&retained),
            90995,
            true,
        ));
    }

    #[test]
    fn own_main_process_without_a_retained_target_still_captures_normally() {
        assert!(!should_preserve_retained_target_for_own_main_process(
            None, 788, false,
        ));
    }

    #[test]
    fn desktop_fn_owner_lock_can_be_reacquired_after_release() {
        let lock_path = std::env::temp_dir().join(format!(
            "iterate-fn-owner-reacquire-test-{}-{}.lock",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));

        let first = acquire_fn_owner_lock_at(&lock_path)
            .expect("first lock attempt")
            .expect("first owner acquires lock");
        assert!(
            acquire_fn_owner_lock_at(&lock_path)
                .expect("contending lock attempt")
                .is_none(),
            "a live owner must keep contenders waiting"
        );

        drop(first);

        let replacement = acquire_fn_owner_lock_at(&lock_path)
            .expect("replacement lock attempt")
            .expect("waiting owner takes over after release");
        drop(replacement);
        let _ = std::fs::remove_file(lock_path);
    }

    #[test]
    fn desktop_fn_owner_rejects_a_replaced_lock_inode() {
        let lock_path = std::env::temp_dir().join(format!(
            "iterate-fn-owner-inode-test-{}-{}.lock",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let original = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open original inode");
        assert!(fn_owner_lock_matches_path(&original, &lock_path));

        std::fs::remove_file(&lock_path).expect("unlink original inode");
        let replacement = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open replacement inode");

        assert!(!fn_owner_lock_matches_path(&original, &lock_path));
        assert!(fn_owner_lock_matches_path(&replacement, &lock_path));
        drop(original);
        drop(replacement);
        let _ = std::fs::remove_file(lock_path);
    }

    #[test]
    fn parse_fn_owner_metadata_preserves_values_with_spaces() {
        let metadata = parse_fn_owner_metadata(
            "pid=123\n\
             bundle_id=com.kexin94yyds.iterate\n\
             exe_path=/Applications/iterate beta.app/Contents/MacOS/iterate\n\
             cdhash=abc123\n",
        );

        assert_eq!(metadata.pid, Some(123));
        assert_eq!(
            metadata.exe_path.as_deref(),
            Some("/Applications/iterate beta.app/Contents/MacOS/iterate")
        );
        assert_eq!(metadata.cdhash.as_deref(), Some("abc123"));
    }

    #[test]
    fn read_then_write_fn_owner_metadata_replaces_existing_content() {
        let lock_path = std::env::temp_dir().join(format!(
            "iterate-fn-owner-test-{}-{}.lock",
            std::process::id(),
            chrono::Local::now()
                .timestamp_nanos_opt()
                .unwrap_or_default()
        ));
        let mut file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(&lock_path)
            .expect("open test lock file");

        file.write_all(b"pid=123\ncdhash=old\n")
            .expect("seed lock file");

        let previous = read_fn_owner_metadata_from_file(&mut file).expect("read metadata");
        assert_eq!(previous.pid, Some(123));

        write_fn_owner_metadata(
            &mut file,
            &FnOwnerMetadata {
                pid: Some(456),
                cdhash: Some("new".to_string()),
                ..FnOwnerMetadata::default()
            },
        );

        let content = std::fs::read_to_string(&lock_path).expect("read rewritten lock file");
        let _ = std::fs::remove_file(lock_path);

        assert!(content.contains("pid=456"));
        assert!(content.contains("cdhash=new"));
        assert!(!content.contains("pid=123"));
        assert!(!content.contains("cdhash=old"));
    }
}

fn build_owner_status() -> SpeechRuntimeOwner {
    #[cfg(target_os = "macos")]
    {
        let fn_listener_owner = fn_owner_state()
            .lock()
            .map(|state| state.lock.is_some())
            .unwrap_or(false);
        let current = current_fn_owner_metadata();
        let owner = read_fn_owner_metadata().unwrap_or_else(|| {
            if fn_listener_owner {
                current.clone()
            } else {
                FnOwnerMetadata::default()
            }
        });
        let owner_is_current_process = owner.pid == Some(std::process::id());
        let owner_matches_current_binary = owner_matches_current_binary(&owner, &current);

        SpeechRuntimeOwner {
            fn_listener_owner,
            owner_pid: owner.pid,
            owner_bundle_id: owner.bundle_id,
            owner_path: owner.exe_path,
            owner_team_id: owner.team_id,
            owner_cdhash: owner.cdhash,
            owner_exe_mtime: owner.exe_mtime,
            owner_acquired_at: owner.acquired_at,
            owner_is_current_process,
            owner_matches_current_binary,
            current_pid: std::process::id(),
            current_path: current.exe_path,
            current_team_id: current.team_id,
            current_cdhash: current.cdhash,
            current_exe_mtime: current.exe_mtime,
            lock_path: Some(fn_owner_lock_path().display().to_string()),
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        SpeechRuntimeOwner {
            fn_listener_owner: true,
            owner_pid: Some(std::process::id()),
            owner_bundle_id: None,
            owner_path: current_exe_path_string(),
            owner_team_id: None,
            owner_cdhash: None,
            owner_exe_mtime: None,
            owner_acquired_at: None,
            owner_is_current_process: true,
            owner_matches_current_binary: Some(true),
            current_pid: std::process::id(),
            current_path: current_exe_path_string(),
            current_team_id: None,
            current_cdhash: None,
            current_exe_mtime: None,
            lock_path: None,
        }
    }
}

#[tauri::command]
pub fn mark_speech_overlay_ready(_app: AppHandle) -> Result<(), String> {
    debug_log("[overlay-ready]", "speech overlay listener is ready");
    record_runtime_event("overlay-ready");
    let snapshot = phase1::get_speech_control_snapshot()?;
    if let Some(identity) = snapshot.identity {
        phase1::ack_speech_overlay_visibility(identity, snapshot.visible)?;
    }
    Ok(())
}

#[tauri::command]
pub fn mark_speech_overlay_unready() -> Result<(), String> {
    debug_log("[overlay-unready]", "speech overlay listener disposed");
    record_runtime_event("overlay-unready");
    Ok(())
}

#[tauri::command]
pub fn get_speech_runtime_status(app: AppHandle) -> SpeechRuntimeStatus {
    let runtime = speech_runtime_state()
        .lock()
        .map(|state| state.clone())
        .unwrap_or_default();
    let target_kind = get_captured_target_kind();
    let target_bundle_id = get_captured_target_app_bundle_id();
    let target_pid = get_captured_target_pid();
    let target_window_label = get_captured_target_window_label();
    let target_request_id = get_captured_target_request_id();
    let active_popup_target = current_active_popup_speech_target();
    let registered_popup_registry = read_popup_speech_target_registry();
    let registered_popup_target_count = registered_popup_registry.targets.len();
    let latest_registered_popup_target = registered_popup_registry
        .targets
        .into_iter()
        .max_by_key(|target| target.updated_at.clone().unwrap_or_default());
    let overlay_window = app.get_webview_window(SPEECH_OVERLAY_WINDOW_LABEL);
    let window_exists = overlay_window.is_some();
    let window_visible = overlay_window
        .as_ref()
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false);
    let control = phase1::get_speech_control_snapshot().ok();

    SpeechRuntimeStatus {
        permissions: SpeechRuntimePermissions {
            microphone: microphone_status(),
            speech_recognition: speech_recognition_status(),
            input_monitoring: input_monitoring_status(),
            accessibility: accessibility_status(),
        },
        owner: build_owner_status(),
        overlay: SpeechRuntimeOverlay {
            window_exists,
            window_visible,
            listener_ready: control.is_some(),
            pending_toggle: false,
        },
        speech: SpeechRuntimeSpeech {
            active: control
                .as_ref()
                .map(|snapshot| snapshot.desired_state == session::DesiredState::On)
                .unwrap_or(false),
            recognition_mode: runtime.recognition_mode,
            last_partial_length: runtime.last_partial_length,
            last_final_length: runtime.last_final_length,
        },
        writeback: SpeechRuntimeWriteback {
            last_target_kind: target_kind,
            last_target_bundle_id: target_bundle_id,
            last_target_pid: target_pid,
            last_target_window_label: target_window_label,
            last_target_request_id: target_request_id,
            active_popup_window_label: active_popup_target
                .as_ref()
                .map(|target| target.window_label.clone()),
            active_popup_request_id: active_popup_target.map(|target| target.request_id),
            registered_popup_target_count,
            latest_registered_popup_pid: latest_registered_popup_target
                .as_ref()
                .and_then(|target| target.pid),
            latest_registered_popup_window_label: latest_registered_popup_target
                .as_ref()
                .map(|target| target.window_label.clone()),
            latest_registered_popup_request_id: latest_registered_popup_target
                .as_ref()
                .map(|target| target.request_id.clone()),
            last_paste_status: runtime.last_paste_status,
            last_error: runtime.last_error,
        },
        diagnostics: SpeechRuntimeDiagnostics {
            log_path: LOG_PATH,
            last_event: runtime.last_event,
            last_event_at: runtime.last_event_at,
        },
    }
}

#[tauri::command]
pub fn accessibility_status() -> bool {
    #[cfg(target_os = "macos")]
    {
        check_accessibility_permission()
    }

    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[tauri::command]
pub fn input_monitoring_status() -> bool {
    #[cfg(target_os = "macos")]
    {
        check_input_monitoring_permission()
    }

    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[tauri::command]
pub fn microphone_status() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        speech_bridge_check_microphone_authorization()
    }

    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[tauri::command]
pub fn speech_recognition_status() -> bool {
    #[cfg(target_os = "macos")]
    unsafe {
        speech_bridge_check_speech_authorization()
    }

    #[cfg(not(target_os = "macos"))]
    {
        true
    }
}

#[tauri::command]
pub fn request_accessibility_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        open_accessibility_settings()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[tauri::command]
pub fn request_input_monitoring_permission() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        request_input_monitoring_access()
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(())
    }
}

#[tauri::command]
pub async fn request_microphone_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        if let Err(error) =
            ensure_privacy_usage_description_available("NSMicrophoneUsageDescription", "麦克风")
        {
            let _ = open_microphone_settings();
            return Err(error);
        }

        tauri::async_runtime::spawn_blocking(|| unsafe {
            speech_bridge_request_microphone_authorization()
        })
        .await
        .map_err(|error| format!("microphone permission request failed: {error}"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

#[tauri::command]
pub async fn request_speech_recognition_permission() -> Result<bool, String> {
    #[cfg(target_os = "macos")]
    {
        if let Err(error) = ensure_privacy_usage_description_available(
            "NSSpeechRecognitionUsageDescription",
            "语音识别",
        ) {
            let _ = open_speech_recognition_settings();
            return Err(error);
        }

        let granted = tauri::async_runtime::spawn_blocking(|| unsafe {
            speech_bridge_request_speech_authorization()
        })
        .await
        .map_err(|error| format!("speech recognition permission request failed: {error}"))?;

        if !granted {
            let _ = open_speech_recognition_settings();
        }

        Ok(granted)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Ok(true)
    }
}

#[tauri::command]
pub fn register_popup_speech_target(
    app: AppHandle,
    window_label: String,
    request_id: String,
    reason: Option<String>,
    project_path: Option<String>,
) -> Result<(), String> {
    let window_label = window_label.trim().to_string();
    let request_id = request_id.trim().to_string();
    if window_label.is_empty() || request_id.is_empty() {
        return Err("window_label and request_id are required".to_string());
    }

    #[cfg(target_os = "macos")]
    let ipc = ensure_popup_speech_ipc(app)?;
    #[cfg(not(target_os = "macos"))]
    let _ = app;

    let target = PopupSpeechTarget {
        window_label,
        request_id,
        pid: Some(std::process::id()),
        project_path: normalize_optional_string(project_path),
        reason: normalize_optional_string(reason),
        updated_at: Some(now_rfc3339()),
        #[cfg(target_os = "macos")]
        ipc_socket_path: Some(ipc.socket_path.display().to_string()),
        #[cfg(not(target_os = "macos"))]
        ipc_socket_path: None,
        #[cfg(target_os = "macos")]
        ipc_token: Some(ipc.token.clone()),
        #[cfg(not(target_os = "macos"))]
        ipc_token: None,
    };
    debug_log(
        "[speech-target-registered]",
        format!(
            "kind=iterate-popup-input window_label={} request_id={} pid={} reason={}",
            target.window_label,
            target.request_id,
            target
                .pid
                .map(|pid| pid.to_string())
                .unwrap_or_else(|| "<unknown>".to_string()),
            target.reason.as_deref().unwrap_or("<unknown>")
        ),
    );
    if let Err(error) = upsert_registered_popup_speech_target(target.clone()) {
        debug_log("[speech-target-registry-upsert-failed]", &error);
        return Err(error);
    }
    let mut guard = active_popup_speech_target()
        .lock()
        .map_err(|_| "failed to lock active popup speech target".to_string())?;
    *guard = Some(target);
    record_runtime_event("speech-target-registered");
    Ok(())
}

#[tauri::command]
pub fn unregister_popup_speech_target(request_id: String) -> Result<(), String> {
    let request_id = request_id.trim().to_string();
    phase1::invalidate_popup_speech_insert(&request_id);
    let mut guard = active_popup_speech_target()
        .lock()
        .map_err(|_| "failed to lock active popup speech target".to_string())?;
    let should_clear = guard
        .as_ref()
        .map(|target| request_id.is_empty() || target.request_id == request_id)
        .unwrap_or(false);
    if should_clear {
        debug_log(
            "[speech-target-unregistered]",
            format!("kind=iterate-popup-input request_id={request_id}"),
        );
        *guard = None;
        record_runtime_event("speech-target-unregistered");
    }
    if let Err(error) = remove_registered_popup_speech_target(&request_id) {
        debug_log("[speech-target-registry-remove-failed]", &error);
        return Err(error);
    }
    Ok(())
}

#[tauri::command]
pub fn get_active_popup_speech_target() -> Option<PopupSpeechTargetView> {
    current_active_popup_speech_target().map(Into::into)
}

#[tauri::command]
pub fn authorize_popup_speech_insert(
    identity: session::SpeechLayerIdentity,
    request_id: String,
    window_label: String,
    insert_id: String,
    text_len: usize,
) -> bool {
    #[cfg(target_os = "macos")]
    {
        let authorized = POPUP_SPEECH_IPC
            .get()
            .and_then(|ipc| ipc.acknowledgements.lock().ok())
            .and_then(|pending| {
                pending.get(insert_id.trim()).map(|entry| {
                    pending_popup_ipc_insert_matches(
                        entry,
                        identity,
                        &request_id,
                        &window_label,
                        text_len,
                    )
                })
            })
            .unwrap_or(false);
        debug_log(
            if authorized {
                "[popup-ipc-insert-authorized]"
            } else {
                "[popup-ipc-insert-rejected]"
            },
            format!(
                "request_id={} window_label={} insert_id={} text_len={}",
                request_id.trim(),
                window_label.trim(),
                insert_id.trim(),
                text_len
            ),
        );
        authorized
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (identity, request_id, window_label, insert_id, text_len);
        false
    }
}

#[tauri::command]
pub fn record_popup_speech_insert_result(
    identity: session::SpeechLayerIdentity,
    request_id: String,
    window_label: String,
    insert_id: String,
    text_len: usize,
) -> Result<(), String> {
    let owner_ack = phase1::ack_popup_speech_insert(
        identity,
        request_id.clone(),
        window_label.clone(),
        insert_id.clone(),
        text_len,
    );
    #[cfg(target_os = "macos")]
    if owner_ack.is_err() {
        acknowledge_popup_ipc_insert(identity, &request_id, &window_label, &insert_id, text_len)?;
    }
    #[cfg(not(target_os = "macos"))]
    owner_ack?;
    debug_log(
        "[popup-insert-applied]",
        format!(
            "request_id={} window_label={} insert_id={} text_len={}",
            request_id.trim(),
            window_label.trim(),
            insert_id.trim(),
            text_len
        ),
    );
    record_paste_status("popup-insert-applied");
    Ok(())
}

#[cfg(target_os = "macos")]
fn acknowledge_popup_ipc_insert(
    identity: session::SpeechLayerIdentity,
    request_id: &str,
    window_label: &str,
    insert_id: &str,
    text_len: usize,
) -> Result<(), String> {
    let ipc = POPUP_SPEECH_IPC
        .get()
        .ok_or_else(|| "popup speech IPC is not initialized".to_string())?;
    let mut acknowledgements = ipc
        .acknowledgements
        .lock()
        .map_err(|_| "popup IPC acknowledgements poisoned".to_string())?;
    let pending = acknowledgements
        .get(insert_id.trim())
        .ok_or_else(|| "no matching popup IPC insert is pending".to_string())?;
    if !pending_popup_ipc_insert_matches(pending, identity, request_id, window_label, text_len) {
        return Err("popup IPC acknowledgement does not match the pending insert".to_string());
    }
    let pending = acknowledgements
        .remove(insert_id.trim())
        .ok_or_else(|| "no matching popup IPC insert is pending".to_string())?;
    drop(acknowledgements);
    pending
        .sender
        .send(text_len)
        .map_err(|_| "popup IPC acknowledgement receiver closed".to_string())
}

#[tauri::command]
pub fn start_native_speech(
    contextual_strings: Option<Vec<String>>,
    recognition_mode: Option<String>,
) -> Result<(), String> {
    if CODEX_LIVE_AUDIO_RESERVED.load(Ordering::Relaxed) {
        return Err("Codex GPT-Live 正在使用麦克风".to_string());
    }
    let contextual_count = contextual_strings
        .as_ref()
        .map(Vec::len)
        .unwrap_or_default();
    let mode = NativeSpeechRecognitionMode::from_option(recognition_mode.clone());
    debug_log(
        "[speech-start-command]",
        format!(
            "contextual_count={contextual_count} recognition_mode={}",
            mode.as_str()
        ),
    );
    record_runtime_event("speech-start-command");
    update_runtime_state(|state| {
        state.recognition_mode = Some(mode.as_str().to_string());
    });
    phase1::configure_current_recognition(contextual_strings, recognition_mode)?;
    phase1::request_desired_state(session::DesiredState::On)
}

#[tauri::command]
pub fn stop_native_speech() -> Result<(), String> {
    debug_log("[speech-stop-command]", "stop requested");
    record_runtime_event("speech-stop-command");
    phase1::request_desired_state(session::DesiredState::Off)
}

#[tauri::command]
pub fn set_codex_live_audio_reserved(app: AppHandle, reserved: bool) -> Result<bool, String> {
    CODEX_LIVE_AUDIO_RESERVED.store(reserved, Ordering::Relaxed);
    if reserved {
        if let Err(error) = phase1::request_desired_state(session::DesiredState::Off) {
            debug_log(
                "[codex-live-native-speech-stop-skipped]",
                format!("native speech runtime was unavailable: {error}"),
            );
        }
        if let Err(error) = reveal_overlay(&app) {
            debug_log("[codex-live-overlay-show-failed]", error);
        }
    } else {
        if let Err(error) = hide_overlay(&app) {
            debug_log("[codex-live-overlay-hide-failed]", error);
        }
    }
    debug_log(
        "[codex-live-audio-reservation]",
        if reserved { "acquired" } else { "released" },
    );
    record_runtime_event(if reserved {
        "codex-live-audio-reservation-acquired"
    } else {
        "codex-live-audio-reservation-released"
    });
    Ok(reserved)
}

#[tauri::command]
pub fn commit_speech_text(_app: AppHandle, text: String) -> Result<(), String> {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        debug_log("[speech-commit-rejected]", "text is empty");
        record_paste_error("text is empty");
        return Err("text is empty".to_string());
    }

    let Some(target) = current_speech_target() else {
        debug_log("[speech-commit-aborted]", "reason=missing-target");
        record_paste_error("missing target");
        return Err("没有可写回的目标。请先聚焦输入框再按 Fn。".to_string());
    };

    match target {
        SpeechTarget::ExternalApp { .. } => {
            debug_log(
                "[speech-commit]",
                format!(
                    "target_kind=external-app writeback_path=paste text_len={}",
                    trimmed.chars().count()
                ),
            );
            let identity = phase1::get_speech_control_snapshot()?
                .identity
                .ok_or_else(|| "speech writeback identity is unavailable".to_string())?;
            match paste_text_with_identity(trimmed, identity)? {
                ExternalPasteDispatch::DispatchedUnverified => Ok(()),
                ExternalPasteDispatch::UnknownAfterDispatch => Err(
                    "写回已经发出，但无法确认最终投递结果；为避免重复输入，本轮不会重试。".into(),
                ),
            }
        }
        SpeechTarget::IteratePopupInput { .. } => {
            Err("popup speech insertion requires a tagged coordinator identity".to_string())
        }
    }
}

#[cfg(target_os = "macos")]
fn validate_popup_ipc_response(
    response: PopupSpeechIpcResponse,
    expected_identity: session::SpeechLayerIdentity,
    expected_request_id: &str,
    expected_window_label: &str,
    expected_insert_id: &str,
    expected_text_len: usize,
) -> Result<(), String> {
    if response.identity != expected_identity
        || response.request_id != expected_request_id
        || response.window_label != expected_window_label
        || response.insert_id != expected_insert_id
    {
        return Err("popup speech IPC returned a mismatched typed acknowledgement".to_string());
    }
    if !response.ok {
        return Err(response
            .error
            .unwrap_or_else(|| "popup speech IPC rejected insert".to_string()));
    }
    if response.text_len != Some(expected_text_len) {
        return Err("popup speech IPC acknowledgement text length mismatch".to_string());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn dispatch_cross_process_popup(
    socket_path: &str,
    token: &str,
    payload: SpeechInsertTextPayload,
) -> Result<(), PopupSpeechIpcDispatchError> {
    use std::net::Shutdown;

    let expected_identity = payload.identity;
    let expected_request_id = payload.request_id.clone();
    let expected_window_label = payload.window_label.clone();
    let expected_insert_id = payload.insert_id.clone();
    let expected_text_len = payload.text.chars().count();
    let mut stream = UnixStream::connect(socket_path).map_err(|error| {
        PopupSpeechIpcDispatchError::before_dispatch(format!(
            "failed to connect popup speech IPC: {error}"
        ))
    })?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| PopupSpeechIpcDispatchError::before_dispatch(error.to_string()))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|error| PopupSpeechIpcDispatchError::before_dispatch(error.to_string()))?;
    let request = PopupSpeechIpcRequest {
        token: token.to_string(),
        payload,
    };
    let encoded = serde_json::to_vec(&request)
        .map_err(|error| PopupSpeechIpcDispatchError::before_dispatch(error.to_string()))?;
    stream.write_all(&encoded).map_err(|error| {
        PopupSpeechIpcDispatchError::before_dispatch(format!(
            "failed to write popup speech IPC: {error}"
        ))
    })?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|error| PopupSpeechIpcDispatchError::after_dispatch(error.to_string()))?;

    let mut response = String::new();
    stream.read_to_string(&mut response).map_err(|error| {
        PopupSpeechIpcDispatchError::after_dispatch(format!(
            "failed to read popup speech IPC response: {error}"
        ))
    })?;
    let response: PopupSpeechIpcResponse = serde_json::from_str(&response).map_err(|error| {
        PopupSpeechIpcDispatchError::after_dispatch(format!(
            "invalid popup speech IPC response: {error}"
        ))
    })?;
    validate_popup_ipc_response(
        response,
        expected_identity,
        &expected_request_id,
        &expected_window_label,
        &expected_insert_id,
        expected_text_len,
    )
    .map_err(PopupSpeechIpcDispatchError::after_dispatch)?;

    debug_log(
        "[popup-ipc-acknowledged]",
        format!(
            "request_id={} window_label={} insert_id={} text_len={}",
            expected_request_id, expected_window_label, expected_insert_id, expected_text_len
        ),
    );
    Ok(())
}

pub(crate) fn dispatch_speech_writeback(
    identity: session::SpeechLayerIdentity,
    text: String,
) -> Result<SpeechWritebackDispatch, String> {
    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        return Err("text is empty".to_string());
    }
    let target = current_speech_target().ok_or_else(|| "missing speech target".to_string())?;
    match target {
        SpeechTarget::ExternalApp { .. } => match paste_text_with_identity(trimmed, identity)? {
            ExternalPasteDispatch::DispatchedUnverified => {
                Ok(SpeechWritebackDispatch::ExternalDispatchedUnverified)
            }
            ExternalPasteDispatch::UnknownAfterDispatch => {
                Ok(SpeechWritebackDispatch::ExternalUnknownAfterDispatch)
            }
        },
        SpeechTarget::IteratePopupInput {
            window_label,
            request_id,
            pid,
        } => {
            if pid != current_process_pid_i32() {
                #[cfg(target_os = "macos")]
                {
                    let registered =
                        registered_popup_speech_target_for_pid(pid.ok_or_else(|| {
                            "cross-process popup target pid is missing".to_string()
                        })?)
                        .ok_or_else(|| {
                            "cross-process popup target is no longer registered".to_string()
                        })?;
                    if registered.request_id != request_id
                        || registered.window_label != window_label
                    {
                        return Err(
                            "cross-process popup endpoint does not match the captured target"
                                .to_string(),
                        );
                    }
                    let socket_path = registered
                        .ipc_socket_path
                        .ok_or_else(|| "cross-process popup IPC socket is missing".to_string())?;
                    let token = registered
                        .ipc_token
                        .ok_or_else(|| "cross-process popup IPC token is missing".to_string())?;
                    let insert_id = format!(
                        "v{}-{:016x}{:016x}-{}-{}-{}",
                        identity.schema_version,
                        identity.owner_epoch_hi,
                        identity.owner_epoch_lo,
                        identity.control_seq,
                        identity.session_sequence,
                        identity.revision
                    );
                    let payload = SpeechInsertTextPayload {
                        identity,
                        request_id,
                        window_label,
                        text: trimmed,
                        mode: "final".to_string(),
                        insert_id,
                    };
                    return match dispatch_cross_process_popup(&socket_path, &token, payload) {
                        Ok(()) => Ok(SpeechWritebackDispatch::ExternalAcknowledged),
                        Err(error) if error.dispatched => {
                            debug_log("[popup-ipc-ack-unknown]", &error.message);
                            Ok(SpeechWritebackDispatch::ExternalUnknownAfterDispatch)
                        }
                        Err(error) => Err(error.message),
                    };
                }
                #[cfg(not(target_os = "macos"))]
                return Err("cross-process popup speech IPC is unavailable".to_string());
            }
            let insert_id = format!(
                "v{}-{:016x}{:016x}-{}-{}-{}",
                identity.schema_version,
                identity.owner_epoch_hi,
                identity.owner_epoch_lo,
                identity.control_seq,
                identity.session_sequence,
                identity.revision
            );
            Ok(SpeechWritebackDispatch::Popup(PopupSpeechWriteback {
                window_label: window_label.clone(),
                payload: SpeechInsertTextPayload {
                    identity,
                    request_id,
                    window_label,
                    text: trimmed,
                    mode: "final".to_string(),
                    insert_id,
                },
            }))
        }
    }
}

#[tauri::command]
pub fn paste_text(text: String) -> Result<(), String> {
    let identity = phase1::get_speech_control_snapshot()?
        .identity
        .ok_or_else(|| "speech writeback identity is unavailable".to_string())?;
    match paste_text_with_identity(text, identity)? {
        ExternalPasteDispatch::DispatchedUnverified => Ok(()),
        ExternalPasteDispatch::UnknownAfterDispatch => {
            Err("写回已经发出，但无法确认最终投递结果；为避免重复输入，本轮不会重试。".into())
        }
    }
}

fn paste_text_with_identity(
    text: String,
    identity: session::SpeechLayerIdentity,
) -> Result<ExternalPasteDispatch, String> {
    let paste_id = PASTE_SEQUENCE_ID.fetch_add(1, Ordering::SeqCst);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        debug_log("[paste-rejected]", "text is empty");
        record_paste_error("text is empty");
        return Err("text is empty".to_string());
    }
    debug_log(
        "[paste-command]",
        format!("id={paste_id} text_len={}", trimmed.chars().count()),
    );
    record_paste_status("paste-command");

    #[cfg(target_os = "macos")]
    {
        let Some(target) = current_speech_target() else {
            debug_log("[paste-aborted]", "reason=missing-target");
            record_paste_error("missing target");
            clear_last_target_app_bundle_id();
            return Err("没有可写回的目标应用。请先聚焦外部输入框再按 Fn。".to_string());
        };
        let SpeechTarget::ExternalApp {
            bundle_id,
            pid: captured_target_pid,
            focus_evidence: captured_focus_evidence,
        } = target
        else {
            debug_log(
                "[paste-aborted]",
                format!(
                    "id={paste_id} reason=target-not-external target={}",
                    target.summary()
                ),
            );
            record_paste_error("target is not an external app");
            clear_last_target_app_bundle_id();
            return Err("弹窗输入不能走剪贴板写回，请使用 commit_speech_text。".to_string());
        };
        let own_app_target = is_own_bundle_id(&bundle_id);
        let _paste_guard = mark_paste_in_progress();
        let control_phase = phase1::get_speech_control_snapshot()
            .map(|snapshot| format!("{:?}", snapshot.phase))
            .unwrap_or_else(|_| "unavailable".to_string());
        debug_log(
            "[paste-begin]",
            format!(
                "id={} target_bundle_id={} target_pid={} own_app_target={} control_phase={}",
                paste_id,
                bundle_id,
                captured_target_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string()),
                own_app_target,
                control_phase
            ),
        );
        if own_app_target && captured_target_pid.is_none() {
            debug_log(
                "[paste-aborted]",
                format!("id={paste_id} reason=own-app-target-without-pid bundle_id={bundle_id}"),
            );
            record_paste_error(format!("own app target without pid: {bundle_id}"));
            clear_last_target_app_bundle_id();
            return Err("没有可写回的 iterate 窗口 PID。请先聚焦输入框再按 Fn。".to_string());
        }
        let captured_target_pid = captured_target_pid.ok_or_else(|| {
            record_paste_error("external target without pid");
            "没有可复核的目标应用 PID，已取消写回。".to_string()
        })?;
        let focus_mode = captured_focus_evidence.dispatch_mode();
        let captured_application = target::FrontmostApplication {
            bundle_id: bundle_id.clone(),
            pid: captured_target_pid,
        };
        if !target::application_matches_identity(&captured_application) {
            record_paste_error("captured target pid no longer matches bundle");
            clear_last_target_app_bundle_id();
            return Err("目标应用进程已变化，已取消写回。".to_string());
        }

        let ax_trusted = check_accessibility_permission();
        debug_log(
            "[paste-accessibility-check]",
            format!("AXIsProcessTrusted={ax_trusted}"),
        );
        if !ax_trusted {
            record_paste_error("missing accessibility permission");
            return Err("缺少辅助功能(Accessibility)权限，无法写回文本。请在 系统设置 → 隐私与安全性 → 辅助功能 中允许 iterate。".to_string());
        }

        let sender_mode = selected_external_sender_mode();
        let helper_request = match sender_mode {
            ExternalSenderMode::OneShotHelper => Some(build_paste_helper_request(
                identity,
                captured_target_pid,
                &bundle_id,
                focus_mode,
            )?),
            ExternalSenderMode::InProcess => None,
        };

        let previous = read_clipboard_text();
        let restore_previous_clipboard = |previous: Option<&String>| {
            if let Some(previous) = previous {
                let _ = write_clipboard_text(previous);
                debug_log(
                    "[paste-clipboard-restored]",
                    format!("id={paste_id} previous clipboard restored"),
                );
            }
        };
        debug_log(
            "[paste-clipboard-before]",
            format!(
                "id={} previous_len={}",
                paste_id,
                previous
                    .as_ref()
                    .map(|text| text.chars().count().to_string())
                    .unwrap_or_else(|| "<unavailable>".to_string())
            ),
        );
        if let Err(error) = write_clipboard_text(trimmed) {
            record_paste_error(&error);
            return Err(error);
        }
        debug_log(
            "[paste-clipboard-written]",
            format!("id={paste_id} new text copied to clipboard"),
        );

        let frontmost_before_paste = frontmost_app_identity().ok();
        let frontmost_before_bundle_id = frontmost_before_paste
            .as_ref()
            .map(|(frontmost_bundle_id, _)| frontmost_bundle_id.as_str());
        let frontmost_before_pid = frontmost_before_paste.as_ref().and_then(|(_, pid)| *pid);
        debug_log(
            "[paste-frontmost-before-activate]",
            format!(
                "id={} bundle_id={} pid={}",
                paste_id,
                frontmost_before_bundle_id.unwrap_or("<unknown>"),
                frontmost_before_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string())
            ),
        );

        if own_app_target {
            debug_log(
                "[paste-target-activate]",
                format!(
                    "id={} method=skip-own-app-pid bundle_id={} pid={}",
                    paste_id, bundle_id, captured_target_pid
                ),
            );
        } else {
            debug_log(
                "[paste-target-activate]",
                format!("id={paste_id} method=bundle bundle_id={bundle_id}"),
            );
            if let Err(error) = activate_app(&bundle_id) {
                record_paste_error(&error);
                restore_previous_clipboard(previous.as_ref());
                clear_last_target_app_bundle_id();
                return Err(error);
            }
            thread::sleep(Duration::from_millis(180));
        }

        let frontmost_after_activate = frontmost_app_identity().ok();
        let frontmost_after_bundle_id = frontmost_after_activate
            .as_ref()
            .map(|(frontmost_bundle_id, _)| frontmost_bundle_id.as_str());
        let frontmost_after_pid = frontmost_after_activate.as_ref().and_then(|(_, pid)| *pid);
        let target_pid = Some(captured_target_pid);
        debug_log(
            "[paste-frontmost-after-activate]",
            format!(
                "id={} bundle_id={} pid={}",
                paste_id,
                frontmost_after_bundle_id.unwrap_or("<unknown>"),
                frontmost_after_pid
                    .map(|pid| pid.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string())
            ),
        );
        let target_is_frontmost = frontmost_after_bundle_id == Some(bundle_id.as_str())
            && frontmost_after_pid == Some(captured_target_pid);
        if !target_is_frontmost {
            debug_log(
                "[paste-aborted]",
                format!(
                    "id={} reason=target-not-frontmost target={} target_pid={} frontmost={} frontmost_pid={}",
                    paste_id,
                    bundle_id,
                    target_pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string()),
                    frontmost_after_bundle_id.unwrap_or("<unknown>"),
                    frontmost_after_pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string())
                ),
            );
            restore_previous_clipboard(previous.as_ref());
            record_paste_error("target not frontmost after activate");
            clear_last_target_app_bundle_id();
            return Err("目标应用未成功切回，已取消写回。".to_string());
        }

        if let CapturedFocusEvidence::Exact(captured_focused_element) = &captured_focus_evidence {
            if let Err(error) = restore_captured_focused_element(
                captured_focused_element,
                captured_target_pid,
                paste_id,
            ) {
                debug_log(
                    "[paste-aborted]",
                    format!(
                        "id={paste_id} reason=restore-captured-focused-element-failed error={error}"
                    ),
                );
                restore_previous_clipboard(previous.as_ref());
                record_paste_error(format!(
                    "failed to restore captured focused element: {error}"
                ));
                clear_last_target_app_bundle_id();
                return Err(
                    "原输入框焦点已经失效，已取消写回。请点回输入框后重新按 Fn。".to_string(),
                );
            }
        } else {
            debug_log(
                "[paste-focus-restore-skipped]",
                format!(
                    "id={paste_id} reason=ax-no-value mode=frontmost-pid target_pid={captured_target_pid}"
                ),
            );
        }

        thread::sleep(Duration::from_millis(80));
        let focused_process = match system_focused_process_info() {
            Ok(info) => {
                debug_log(
                    "[paste-focused-process-before-post]",
                    format!("id={} {}", paste_id, focused_process_summary(&info)),
                );
                Some(info)
            }
            Err(error) => {
                debug_log(
                    "[paste-focused-process-before-post]",
                    format!("id={paste_id} error={error}"),
                );
                None
            }
        };
        if let CapturedFocusEvidence::Exact(captured_focused_element) = &captured_focus_evidence {
            if let Err(error) =
                verify_captured_focused_element(captured_focused_element, captured_target_pid)
            {
                debug_log(
                    "[paste-aborted]",
                    format!(
                        "id={paste_id} reason=focused-element-changed-before-dispatch error={error}"
                    ),
                );
                restore_previous_clipboard(previous.as_ref());
                record_paste_error(format!("focused element changed before dispatch: {error}"));
                clear_last_target_app_bundle_id();
                return Err(
                    "原输入框在写回前再次失焦，已取消写回。请点回输入框后重试。".to_string()
                );
            }
        }

        let frontmost_before_dispatch = frontmost_app_identity().ok();
        let frontmost_before_dispatch_bundle_id = frontmost_before_dispatch
            .as_ref()
            .map(|(frontmost_bundle_id, _)| frontmost_bundle_id.as_str());
        let frontmost_before_dispatch_pid =
            frontmost_before_dispatch.as_ref().and_then(|(_, pid)| *pid);
        if frontmost_before_dispatch_bundle_id != Some(bundle_id.as_str())
            || frontmost_before_dispatch_pid != Some(captured_target_pid)
        {
            debug_log(
                "[paste-aborted]",
                format!(
                    "id={} reason=target-changed-before-dispatch target={} target_pid={} frontmost={} frontmost_pid={}",
                    paste_id,
                    bundle_id,
                    captured_target_pid,
                    frontmost_before_dispatch_bundle_id.unwrap_or("<unknown>"),
                    frontmost_before_dispatch_pid
                        .map(|pid| pid.to_string())
                        .unwrap_or_else(|| "<unknown>".to_string())
                ),
            );
            restore_previous_clipboard(previous.as_ref());
            record_paste_error("target changed before dispatch");
            clear_last_target_app_bundle_id();
            return Err("目标应用在写回前发生变化，已取消写回。".to_string());
        }

        let paste_route = select_paste_dispatch_route(
            frontmost_before_dispatch_pid,
            focused_process.as_ref(),
            &bundle_id,
            focus_mode,
        );
        log_paste_dispatch_route(
            paste_route,
            frontmost_before_dispatch_pid,
            focused_process.as_ref(),
            &bundle_id,
            focus_mode,
            paste_id,
        );
        if let PasteDispatchRoute::Abort(reason) = paste_route {
            let error = format!("paste dispatch aborted: {reason}");
            record_paste_error(&error);
            restore_previous_clipboard(previous.as_ref());
            clear_last_target_app_bundle_id();
            return Err("目标输入焦点已经变化，已取消写回以避免输入到错误窗口。".to_string());
        }
        let dispatch = match sender_mode {
            ExternalSenderMode::InProcess => {
                if let Err(error) = simulate_paste(paste_route, paste_id) {
                    record_paste_error(&error);
                    restore_previous_clipboard(previous.as_ref());
                    clear_last_target_app_bundle_id();
                    return Err(error);
                }
                debug_log(
                    "[paste-posted]",
                    format!("id={paste_id} sender=in-process verification=unverified"),
                );
                ExternalPasteDispatch::DispatchedUnverified
            }
            ExternalSenderMode::OneShotHelper => {
                let request = helper_request.expect("one-shot sender request must be prepared");
                let mut launcher = external_sender::SystemPasteHelperLauncher::default();
                match external_sender::dispatch_with_launcher(&mut launcher, &request) {
                    external_sender::PasteHelperDispatchOutcome::DispatchedUnverified {
                        helper_pid,
                        attempts,
                    } => {
                        debug_log(
                            "[paste-helper-terminal]",
                            format!(
                                "id={} attempt_id={} helper_pid={} attempts={} outcome=posted-unverified",
                                paste_id, request.attempt_id, helper_pid, attempts
                            ),
                        );
                        ExternalPasteDispatch::DispatchedUnverified
                    }
                    external_sender::PasteHelperDispatchOutcome::RejectedBeforePost {
                        helper_pid,
                        reason,
                        attempts,
                    } => {
                        debug_log(
                            "[paste-helper-terminal]",
                            format!(
                                "id={} attempt_id={} helper_pid={} attempts={} outcome=rejected-before-post reason={}",
                                paste_id,
                                request.attempt_id,
                                helper_pid
                                    .map(|pid| pid.to_string())
                                    .unwrap_or_else(|| "<none>".into()),
                                attempts,
                                reason
                            ),
                        );
                        restore_previous_clipboard(previous.as_ref());
                        clear_last_target_app_bundle_id();
                        record_paste_error(format!("paste helper rejected before post: {reason}"));
                        return Err("目标或写回身份在发送前发生变化，已取消写回。".into());
                    }
                    external_sender::PasteHelperDispatchOutcome::UnknownAfterDispatch {
                        helper_pid,
                        reason,
                        attempts,
                    } => {
                        debug_log(
                            "[paste-helper-terminal]",
                            format!(
                                "id={} attempt_id={} helper_pid={} attempts={} outcome=unknown-after-dispatch reason={}",
                                paste_id,
                                request.attempt_id,
                                helper_pid
                                    .map(|pid| pid.to_string())
                                    .unwrap_or_else(|| "<none>".into()),
                                attempts,
                                reason
                            ),
                        );
                        ExternalPasteDispatch::UnknownAfterDispatch
                    }
                }
            }
        };
        match dispatch {
            ExternalPasteDispatch::DispatchedUnverified => {
                record_paste_status("paste-dispatched-unverified")
            }
            ExternalPasteDispatch::UnknownAfterDispatch => {
                record_paste_status("paste-unknown-after-dispatch")
            }
        }

        if previous.is_some() {
            thread::sleep(Duration::from_millis(1200));
            restore_previous_clipboard(previous.as_ref());
        }

        clear_last_target_app_bundle_id();
        debug_log("[paste-end]", format!("id={paste_id}"));

        Ok(dispatch)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = identity;
        Err("global paste is only implemented on macOS".to_string())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExternalSenderMode {
    OneShotHelper,
    InProcess,
}

fn selected_external_sender_mode() -> ExternalSenderMode {
    match std::env::var("ITERATE_SPEECH_EXTERNAL_SENDER")
        .ok()
        .as_deref()
    {
        Some("in_process") => ExternalSenderMode::InProcess,
        _ => ExternalSenderMode::OneShotHelper,
    }
}

#[cfg(target_os = "macos")]
fn build_paste_helper_request(
    identity: session::SpeechLayerIdentity,
    target_pid: i32,
    target_bundle_id: &str,
    focus_mode: FocusDispatchMode,
) -> Result<external_sender::PasteHelperRequest, String> {
    let executable_path = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map_err(|error| format!("failed to resolve iterate executable: {error}"))?
        .display()
        .to_string();
    let signing = current_fn_owner_metadata();
    let team_id = signing
        .team_id
        .ok_or_else(|| "iterate Team ID is unavailable".to_string())?;
    let cdhash = signing
        .cdhash
        .ok_or_else(|| "iterate CDHash is unavailable".to_string())?;
    Ok(external_sender::PasteHelperRequest {
        schema_version: external_sender::PASTE_HELPER_SCHEMA_VERSION,
        attempt_id: uuid::Uuid::new_v4().to_string(),
        owner_epoch: identity.owner_epoch().to_canonical_string(),
        parent_pid: std::process::id(),
        target_pid,
        target_bundle_id: target_bundle_id.to_string(),
        focus_mode: match focus_mode {
            FocusDispatchMode::Exact => external_sender::HelperFocusMode::Exact,
            FocusDispatchMode::FrontmostPidFallback => {
                external_sender::HelperFocusMode::FrontmostPidFallback
            }
        },
        expected_executable_identity: external_sender::ExpectedExecutableIdentity {
            executable_path,
            team_id,
            cdhash,
        },
    })
}
