//! Live in-app adapter for the shared desktop speech coordinator.

use super::coordinator::{
    validate_control_identity, MonotonicClock, SpeechCompletionSender, SpeechCoordinatorHandle,
    SpeechEffectExecutor,
};
use super::fn_listener::{self, FnListenerEvent};
#[cfg(target_os = "macos")]
use super::owner::FileOwnerLease;
use super::owner::{
    OwnerMetadata, OwnerSupervisor, OwnerSupervisorCommand, OwnerSupervisorEvent,
    RandomEpochSource, SpeechProcessRole,
};
use super::session::{
    OwnerEpoch, SpeechControlInput, SpeechEffect, SpeechLayerIdentity, SpeechSnapshot,
};
use serde::Serialize;
use std::ffi::{c_char, c_void, CStr, CString};
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

pub const SESSION_SNAPSHOT_EVENT: &str = "speech://session-snapshot";
pub const PROCESS_TRANSCRIPT_EVENT: &str = "speech://process-transcript";

static PHASE1_RUNTIME: OnceLock<Arc<Phase1Runtime>> = OnceLock::new();
static DIAGNOSTIC_OWNER_EPOCH: Mutex<Option<OwnerEpoch>> = Mutex::new(None);

pub(crate) fn diagnostic_owner_epoch() -> Option<OwnerEpoch> {
    DIAGNOSTIC_OWNER_EPOCH.lock().ok().and_then(|value| *value)
}

/// Tracks the existing supervisor's lease lifetime for status display only.
struct DiagnosticOwnerLease;

impl Drop for DiagnosticOwnerLease {
    fn drop(&mut self) {
        if let Ok(mut value) = DIAGNOSTIC_OWNER_EPOCH.lock() {
            *value = None;
        }
    }
}
#[cfg(target_os = "macos")]
static PHASE1_SUPERVISOR_STARTED: AtomicBool = AtomicBool::new(false);
static PENDING_POPUP_INSERT: OnceLock<Mutex<Option<PendingPopupInsert>>> = OnceLock::new();
const POPUP_INSERT_ACK_DEADLINE: Duration = Duration::from_millis(300);

#[derive(Clone)]
struct PendingPopupInsert {
    identity: SpeechLayerIdentity,
    request_id: String,
    window_label: String,
    insert_id: String,
    text_len: usize,
    dispatched: bool,
    acknowledged: bool,
}

#[derive(Clone, Debug)]
struct RecognitionConfiguration {
    identity: Option<SpeechLayerIdentity>,
    contextual_strings: Vec<String>,
    mode: super::NativeSpeechRecognitionMode,
}

impl Default for RecognitionConfiguration {
    fn default() -> Self {
        Self {
            identity: None,
            contextual_strings: Vec::new(),
            mode: super::NativeSpeechRecognitionMode::Quality,
        }
    }
}

struct RecognitionConfigurationStore {
    value: Mutex<RecognitionConfiguration>,
    ready: Condvar,
}

impl RecognitionConfigurationStore {
    fn new() -> Self {
        Self {
            value: Mutex::new(RecognitionConfiguration::default()),
            ready: Condvar::new(),
        }
    }

    fn configure(&self, configuration: RecognitionConfiguration) -> Result<(), String> {
        *self
            .value
            .lock()
            .map_err(|_| "speech recognition configuration is poisoned".to_string())? =
            configuration;
        self.ready.notify_all();
        Ok(())
    }

    fn wait_for_session(&self, identity: SpeechLayerIdentity) {
        let Ok(configuration) = self.value.lock() else {
            return;
        };
        let _ = self.ready.wait_timeout_while(
            configuration,
            Duration::from_millis(200),
            |configuration| !configuration_matches(configuration.identity, identity),
        );
    }

    fn for_session(&self, identity: SpeechLayerIdentity) -> RecognitionConfiguration {
        self.value
            .lock()
            .ok()
            .filter(|configuration| configuration_matches(configuration.identity, identity))
            .map(|configuration| configuration.clone())
            .unwrap_or_default()
    }
}

fn configuration_matches(
    configured: Option<SpeechLayerIdentity>,
    requested: SpeechLayerIdentity,
) -> bool {
    configured.is_some_and(|configured| {
        configured.schema_version == requested.schema_version
            && configured.owner_epoch() == requested.owner_epoch()
            && configured.control_seq == requested.control_seq
            && configured.session_sequence == requested.session_sequence
    })
}

#[derive(Clone)]
struct Phase1Clock {
    origin: Instant,
}

impl Phase1Clock {
    fn new() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}

impl MonotonicClock for Phase1Clock {
    fn now_us(&self) -> u64 {
        self.origin.elapsed().as_micros().min(u64::MAX as u128) as u64
    }
}

struct Phase1Runtime {
    app: AppHandle,
    coordinator: Mutex<SpeechCoordinatorHandle>,
    recognition: Arc<RecognitionConfigurationStore>,
}

#[derive(Clone, Serialize)]
struct ProcessTranscriptPayload {
    identity: SpeechLayerIdentity,
    text: String,
}

struct Phase1EffectExecutor {
    app: AppHandle,
    clock: Arc<Phase1Clock>,
    recognition: Arc<RecognitionConfigurationStore>,
}

impl SpeechEffectExecutor for Phase1EffectExecutor {
    fn execute(&self, effect: SpeechEffect, completion: SpeechCompletionSender) {
        match effect {
            SpeechEffect::ShowOverlay { .. } => {
                if let Err(error) = super::reveal_overlay(&self.app) {
                    super::debug_log("[phase1-overlay-show-failed]", error);
                }
            }
            SpeechEffect::HideOverlay { .. } => {
                let result = if super::codex_live_audio_reserved() {
                    super::reveal_overlay(&self.app)
                } else {
                    super::hide_overlay(&self.app)
                };
                if let Err(error) = result {
                    super::debug_log("[phase1-overlay-visibility-failed]", error);
                }
            }
            SpeechEffect::PrepareStart { identity } => {
                let recognition = self.recognition.clone();
                std::thread::spawn(move || {
                    let prepared = super::microphone_status()
                        && super::speech_recognition_status()
                        && super::input_monitoring_status()
                        && super::accessibility_status()
                        && super::capture_frontmost_target_app().is_ok();
                    if prepared {
                        recognition.wait_for_session(identity);
                    }
                    let input = if prepared {
                        SpeechControlInput::StartPrepared { identity }
                    } else {
                        SpeechControlInput::StartFailed { identity }
                    };
                    if let Err(error) = completion.send(input) {
                        super::debug_log(
                            "[phase1-prepare-completion-failed]",
                            format!("{error:?}"),
                        );
                    }
                    flush_and_emit_snapshot();
                });
            }
            SpeechEffect::CancelStart { .. } => {}
            SpeechEffect::StartNative { identity } => {
                let configuration = self.recognition.for_session(identity);
                start_native(identity, configuration);
            }
            // finish/cancel 在非主线程会 dispatch_sync 到 main queue。若在 coordinator worker
            // 内联执行，且 main 线程正拿着 phase1 coordinator Mutex 等 send/flush ack（Tauri
            // 同步命令、native 回调都跑在 main），worker→main→coordinator 三方互等即永久死锁。
            // 因此这两个效果必须在独立线程执行，与 DispatchWriteback 同型。
            SpeechEffect::FinishNative { identity } => {
                std::thread::spawn(move || {
                    #[cfg(target_os = "macos")]
                    unsafe {
                        super::native_backend::finish(identity);
                    }
                    #[cfg(not(target_os = "macos"))]
                    let _ = identity;
                });
            }
            SpeechEffect::CancelNative { identity } => {
                std::thread::spawn(move || {
                    #[cfg(target_os = "macos")]
                    unsafe {
                        super::native_backend::cancel(identity);
                    }
                    #[cfg(not(target_os = "macos"))]
                    let _ = identity;
                });
            }
            SpeechEffect::ScheduleFinishDeadline {
                identity,
                deadline_us,
            } => {
                let clock = self.clock.clone();
                std::thread::spawn(move || {
                    let delay = deadline_us.saturating_sub(clock.now_us());
                    std::thread::sleep(Duration::from_micros(delay));
                    let _ = send_control_input(SpeechControlInput::FinishDeadline { identity });
                });
            }
            SpeechEffect::ScheduleProcessingDeadline {
                identity,
                deadline_us,
            } => {
                let clock = self.clock.clone();
                std::thread::spawn(move || {
                    let delay = deadline_us.saturating_sub(clock.now_us());
                    std::thread::sleep(Duration::from_micros(delay));
                    let _ = send_control_input(SpeechControlInput::ProcessingDeadline { identity });
                });
            }
            SpeechEffect::ProcessTranscript { identity, text } => {
                super::debug_log("[transcript-processing-start]", format!(
                    "session={} revision={} text_len={}",
                    identity.session_sequence, identity.revision, text.chars().count()));
                if self
                    .app
                    .emit(
                        PROCESS_TRANSCRIPT_EVENT,
                        ProcessTranscriptPayload { identity, text },
                    )
                    .is_err()
                {
                    super::debug_log("[transcript-processing-emit-failed]", format!(
                        "session={} revision={}", identity.session_sequence, identity.revision));
                    let _ = completion.send(SpeechControlInput::ProcessingFailed { identity });
                }
            }
            SpeechEffect::DispatchWriteback { identity, text } => {
                super::debug_log("[writeback-dispatch-start]", format!(
                    "session={} revision={}", identity.session_sequence, identity.revision));
                let app = self.app.clone();
                std::thread::spawn(move || {
                    let result = super::dispatch_speech_writeback(identity, text);
                    let outcome = match &result {
                        Ok(super::SpeechWritebackDispatch::ExternalAcknowledged) => "acknowledged",
                        Ok(super::SpeechWritebackDispatch::ExternalDispatchedUnverified) => "dispatched-unverified",
                        Ok(super::SpeechWritebackDispatch::ExternalUnknownAfterDispatch) => "unknown-after-dispatch",
                        Ok(super::SpeechWritebackDispatch::Popup(_)) => "popup-awaiting-ack",
                        Err(_) => "dispatch-failed",
                    };
                    // Do not log transcripts or arbitrary downstream error strings.
                    super::debug_log("[writeback-dispatch-result]", format!(
                        "session={} revision={} outcome={outcome}", identity.session_sequence, identity.revision));
                    match result {
                        Ok(super::SpeechWritebackDispatch::ExternalAcknowledged) => {
                            let _ = completion
                                .send(SpeechControlInput::WritebackDispatched { identity });
                            let _ = completion
                                .send(SpeechControlInput::WritebackAcknowledged { identity });
                        }
                        Ok(super::SpeechWritebackDispatch::ExternalDispatchedUnverified) => {
                            let _ = completion
                                .send(SpeechControlInput::WritebackDispatched { identity });
                            let _ = completion
                                .send(SpeechControlInput::WritebackUnverified { identity });
                        }
                        Ok(super::SpeechWritebackDispatch::ExternalUnknownAfterDispatch) => {
                            let _ = completion
                                .send(SpeechControlInput::WritebackDispatched { identity });
                            let _ =
                                completion.send(SpeechControlInput::WritebackFailed { identity });
                        }
                        Ok(super::SpeechWritebackDispatch::Popup(popup)) => {
                            dispatch_popup_insert(app, popup, completion);
                        }
                        Err(_) => {
                            let _ = completion
                                .send(SpeechControlInput::WritebackDispatchFailed { identity });
                        }
                    }
                });
            }
        }
    }
}

fn pending_popup_insert() -> &'static Mutex<Option<PendingPopupInsert>> {
    PENDING_POPUP_INSERT.get_or_init(|| Mutex::new(None))
}

fn dispatch_popup_insert(
    app: AppHandle,
    popup: super::PopupSpeechWriteback,
    completion: SpeechCompletionSender,
) {
    let identity = popup.payload.identity;
    let pending = PendingPopupInsert {
        identity,
        request_id: popup.payload.request_id.clone(),
        window_label: popup.payload.window_label.clone(),
        insert_id: popup.payload.insert_id.clone(),
        text_len: popup.payload.text.chars().count(),
        dispatched: false,
        acknowledged: false,
    };
    if let Ok(mut guard) = pending_popup_insert().lock() {
        *guard = Some(pending.clone());
    } else {
        let _ = completion.send(SpeechControlInput::WritebackDispatchFailed { identity });
        return;
    }

    if app
        .emit_to(&popup.window_label, super::INSERT_TEXT_EVENT, popup.payload)
        .is_err()
    {
        clear_pending_popup_insert(identity, &pending.insert_id);
        let _ = completion.send(SpeechControlInput::WritebackDispatchFailed { identity });
        return;
    }

    let expected_insert_id = pending.insert_id.clone();
    let acknowledged = pending_popup_insert()
        .lock()
        .ok()
        .and_then(|mut guard| {
            let current = guard.as_mut()?;
            if current.identity != identity || current.insert_id != expected_insert_id {
                return None;
            }
            current.dispatched = true;
            Some(current.acknowledged)
        })
        .unwrap_or(false);
    let _ = completion.send(SpeechControlInput::WritebackDispatched { identity });
    if acknowledged {
        clear_pending_popup_insert(identity, &pending.insert_id);
        let _ = completion.send(SpeechControlInput::WritebackAcknowledged { identity });
        return;
    }

    let insert_id = pending.insert_id;
    std::thread::spawn(move || {
        std::thread::sleep(POPUP_INSERT_ACK_DEADLINE);
        let expired = pending_popup_insert()
            .lock()
            .ok()
            .and_then(|mut guard| {
                let matches = guard
                    .as_ref()
                    .map(|pending| {
                        pending.identity == identity
                            && pending.insert_id == insert_id
                            && pending.dispatched
                            && !pending.acknowledged
                    })
                    .unwrap_or(false);
                matches.then(|| guard.take()).flatten()
            })
            .is_some();
        if expired {
            let _ = send_control_input(SpeechControlInput::WritebackFailed { identity });
        }
    });
}

fn clear_pending_popup_insert(identity: SpeechLayerIdentity, insert_id: &str) {
    if let Ok(mut guard) = pending_popup_insert().lock() {
        if guard
            .as_ref()
            .map(|pending| pending.identity == identity && pending.insert_id == insert_id)
            .unwrap_or(false)
        {
            *guard = None;
        }
    }
}

pub fn ack_popup_speech_insert(
    identity: SpeechLayerIdentity,
    request_id: String,
    window_label: String,
    insert_id: String,
    text_len: usize,
) -> Result<(), String> {
    validate(identity)?;
    let should_complete = {
        let mut guard = pending_popup_insert()
            .lock()
            .map_err(|_| "pending popup insert is poisoned".to_string())?;
        let pending = guard
            .as_mut()
            .ok_or_else(|| "no pending popup insert".to_string())?;
        if pending.identity != identity
            || pending.request_id != request_id.trim()
            || pending.window_label != window_label.trim()
            || pending.insert_id != insert_id.trim()
            || pending.text_len != text_len
        {
            return Err("popup insert acknowledgement does not match the pending insert".into());
        }
        if pending.acknowledged {
            return Ok(());
        }
        pending.acknowledged = true;
        if pending.dispatched {
            *guard = None;
            true
        } else {
            false
        }
    };
    if should_complete {
        send_control_input(SpeechControlInput::WritebackAcknowledged { identity })?;
    }
    Ok(())
}

pub fn invalidate_popup_speech_insert(request_id: &str) {
    let pending = pending_popup_insert().lock().ok().and_then(|mut guard| {
        let matches = guard
            .as_ref()
            .map(|pending| request_id.is_empty() || pending.request_id == request_id)
            .unwrap_or(false);
        matches.then(|| guard.take()).flatten()
    });
    if let Some(pending) = pending {
        let input = if pending.dispatched {
            SpeechControlInput::WritebackFailed {
                identity: pending.identity,
            }
        } else {
            SpeechControlInput::WritebackDispatchFailed {
                identity: pending.identity,
            }
        };
        let _ = send_control_input(input);
    }
}

pub fn start_phase1_runtime(app: AppHandle, role: SpeechProcessRole) {
    #[cfg(target_os = "macos")]
    {
        if !matches!(role, SpeechProcessRole::CanonicalGui)
            || PHASE1_SUPERVISOR_STARTED.swap(true, Ordering::SeqCst)
        {
            return;
        }
        std::thread::spawn(move || run_owner_supervisor(app, role));
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (app, role);
}

#[cfg(target_os = "macos")]
fn run_owner_supervisor(app: AppHandle, role: SpeechProcessRole) {
    let path = match super::runtime_paths::production_owner_lock_path() {
        Ok(path) => path,
        Err(error) => {
            super::debug_log("[phase1-owner-path-failed]", error.to_string());
            return;
        }
    };
    let executable = std::env::current_exe()
        .and_then(|path| path.canonicalize())
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "unknown".into());
    #[cfg(target_os = "macos")]
    let signing = super::current_fn_owner_metadata();
    #[cfg(target_os = "macos")]
    let metadata = OwnerMetadata::new(
        role,
        std::process::id(),
        &executable,
        signing.team_id.as_deref().unwrap_or("unknown"),
        signing.cdhash.as_deref().unwrap_or("unknown"),
    );
    #[cfg(not(target_os = "macos"))]
    let metadata = OwnerMetadata::new(role, std::process::id(), &executable, "unknown", "unknown");
    let backend = FileOwnerLease::new(path, unsafe { libc::geteuid() });
    let mut supervisor = OwnerSupervisor::new(metadata, backend, RandomEpochSource);

    loop {
        let events = supervisor.apply(OwnerSupervisorCommand::Reconcile, true);
        let acquired = events.iter().find_map(|event| match event {
            OwnerSupervisorEvent::Acquired(epoch) => Some(*epoch),
            _ => None,
        });
        if let Some(epoch) = acquired {
            if initialize_runtime(&app, role, epoch).is_ok() {
                if let Ok(mut value) = DIAGNOSTIC_OWNER_EPOCH.lock() {
                    *value = Some(epoch);
                }
                let _diagnostic_lease = DiagnosticOwnerLease;
                run_fn_transport(&mut supervisor);
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn initialize_runtime(
    app: &AppHandle,
    role: SpeechProcessRole,
    epoch: OwnerEpoch,
) -> Result<(), String> {
    super::overlay::ensure_owner_overlay(app, role, Some(epoch))?;
    let clock = Arc::new(Phase1Clock::new());
    let recognition = Arc::new(RecognitionConfigurationStore::new());
    let executor = Arc::new(Phase1EffectExecutor {
        app: app.clone(),
        clock: clock.clone(),
        recognition: recognition.clone(),
    });
    let coordinator = SpeechCoordinatorHandle::new(executor, clock, 64);
    let runtime = Arc::new(Phase1Runtime {
        app: app.clone(),
        coordinator: Mutex::new(coordinator),
        recognition,
    });
    PHASE1_RUNTIME
        .set(runtime)
        .map_err(|_| "phase1 speech runtime already initialized".to_string())?;
    send_control_input(SpeechControlInput::OwnerAcquired { epoch })
}

fn run_fn_transport<B, E>(supervisor: &mut OwnerSupervisor<B, E>)
where
    B: super::owner::OwnerLeaseBackend,
    E: super::owner::OwnerEpochSource,
{
    let mut consumed_sequence = fn_listener::drain_observed_presses_after(0).latest_sequence;
    let mut gesture = fn_listener::FnHoldGesture::default();
    loop {
        let (sender, receiver) = std::sync::mpsc::channel();
        let handle = match fn_listener::start_transport(sender) {
            Ok(handle) => handle,
            Err(error) => {
                super::debug_log("[phase1-fn-install-failed]", error.to_string());
                std::thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        loop {
            let event = match receive_fn_gesture_event(&receiver, &gesture) {
                Ok(event) => event,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if gesture.trigger_long_press_if_due(fn_listener::monotonic_us()) {
                        dispatch_desktop_codex_live_fn_action("start");
                    }
                    continue;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            };
            match event {
                FnListenerEvent::Pressed {
                    observed_monotonic_us,
                } => {
                    let drain = fn_listener::drain_observed_presses_after(consumed_sequence);
                    if drain.press_count > 0 {
                        gesture.press(observed_monotonic_us);
                    }
                    consumed_sequence = drain.latest_sequence;
                }
                FnListenerEvent::Released {
                    observed_monotonic_us,
                } => match gesture.release_at(observed_monotonic_us) {
                    fn_listener::FnReleaseAction::Long => {
                        dispatch_desktop_codex_live_fn_action("start");
                    }
                    fn_listener::FnReleaseAction::Short => {
                        dispatch_desktop_codex_live_fn_action("short");
                    }
                    fn_listener::FnReleaseAction::None => {}
                },
                FnListenerEvent::ControlTapped => {
                    if super::codex_live_audio_reserved() {
                        dispatch_desktop_codex_live_fn_action("mute");
                    } else {
                        super::debug_log(
                            "[desktop-codex-live-control-tap-ignored]",
                            "GPT-Live does not own the microphone",
                        );
                    }
                }
                FnListenerEvent::TapDisabled { .. } | FnListenerEvent::TapEnded => {
                    gesture.reset();
                    break;
                }
            }
        }
        gesture.reset();
        handle.stop();
        handle.join();
        let _ = supervisor.apply(OwnerSupervisorCommand::ListenerEnded, true);
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn receive_fn_gesture_event(
    receiver: &std::sync::mpsc::Receiver<FnListenerEvent>,
    gesture: &fn_listener::FnHoldGesture,
) -> Result<FnListenerEvent, std::sync::mpsc::RecvTimeoutError> {
    // Always drain an already-observed edge before consulting wall-clock time.
    // A short press that was released while this thread was descheduled must
    // not be reclassified as a five-second hold.
    match receiver.try_recv() {
        Ok(event) => return Ok(event),
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            return Err(std::sync::mpsc::RecvTimeoutError::Disconnected);
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
    }
    match gesture.remaining_until_long_press(fn_listener::monotonic_us()) {
        Some(timeout) => receiver.recv_timeout(timeout),
        None => receiver
            .recv()
            .map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected),
    }
}

fn dispatch_desktop_codex_live_fn_action(action: &'static str) {
    if let Some(sender) = desktop_codex_live_fn_dispatcher() {
        if sender.send(action).is_ok() {
            return;
        }
    }
    fallback_desktop_codex_live_fn_action(action);
}

fn desktop_codex_live_fn_dispatcher() -> Option<&'static std::sync::mpsc::Sender<&'static str>> {
    static DISPATCHER: std::sync::OnceLock<Option<std::sync::mpsc::Sender<&'static str>>> =
        std::sync::OnceLock::new();
    DISPATCHER
        .get_or_init(|| {
            let (sender, receiver) = std::sync::mpsc::channel::<&'static str>();
            std::thread::Builder::new()
                .name("iterate-fn-live-dispatch".to_string())
                .spawn(move || {
                    while let Ok(action) = receiver.recv() {
                        process_desktop_codex_live_fn_action(action);
                    }
                })
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FnBridgeFailureClass {
    SafeBeforeCommit,
    SafeRejected,
    AmbiguousAfterDispatch,
}

#[derive(Debug)]
struct FnBridgeFailure {
    class: FnBridgeFailureClass,
    details: String,
}

fn fn_bridge_failure(class: FnBridgeFailureClass, details: impl Into<String>) -> FnBridgeFailure {
    FnBridgeFailure {
        class,
        details: details.into(),
    }
}

fn should_fallback_desktop_codex_live_fn_action(failure: &FnBridgeFailure) -> bool {
    failure.class != FnBridgeFailureClass::AmbiguousAfterDispatch
}

fn process_desktop_codex_live_fn_action(action: &'static str) {
    let result = tauri::async_runtime::block_on(async move {
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(250))
            .build()
            .map_err(|error| {
                fn_bridge_failure(
                    FnBridgeFailureClass::SafeBeforeCommit,
                    format!("client={error}"),
                )
            })?;
        let url = "http://127.0.0.1:8080/api/desktop-codex-live";
        let token =
            crate::bridge::auth::issue_desktop_bridge_token("POST", "/api/desktop-codex-live")
                .map_err(|error| {
                    fn_bridge_failure(
                        FnBridgeFailureClass::SafeBeforeCommit,
                        format!("token={error}"),
                    )
                })?;
        let response = client
            .post(url)
            .bearer_auth(token)
            .json(&serde_json::json!({
                "action": action,
                "project_path": null,
            }))
            .send()
            .await
            .map_err(|error| {
                let class = if error.is_connect() {
                    FnBridgeFailureClass::SafeBeforeCommit
                } else {
                    // A timeout or body/transport failure may happen after the
                    // bridge committed the non-idempotent toggle. Never emit a
                    // second fallback action when commit state is unknown.
                    FnBridgeFailureClass::AmbiguousAfterDispatch
                };
                fn_bridge_failure(class, format!("request={error}"))
            })?;
        let status = response.status();
        if status == reqwest::StatusCode::NO_CONTENT && action == "short" {
            return Ok(false);
        }
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(fn_bridge_failure(
                FnBridgeFailureClass::SafeRejected,
                format!("status={status} body={body}"),
            ));
        }
        Ok(true)
    });

    match result {
        Ok(true) => super::debug_log("[desktop-codex-live-fn-action]", format!("action={action}")),
        Ok(false) => {
            let _ = send_control_input(SpeechControlInput::FnPressed);
        }
        Err(failure) => {
            super::debug_log(
                "[desktop-codex-live-fn-action-failed]",
                format!(
                    "action={action} class={:?} error={}",
                    failure.class, failure.details
                ),
            );
            if should_fallback_desktop_codex_live_fn_action(&failure) {
                fallback_desktop_codex_live_fn_action(action);
            }
        }
    }
}

fn fallback_desktop_codex_live_fn_action(action: &str) {
    if action == "short" && !super::codex_live_audio_reserved() {
        let _ = send_control_input(SpeechControlInput::FnPressed);
    } else if action == "toggle" {
        emit_desktop_codex_live_fn_fallback(action);
    }
}

fn emit_desktop_codex_live_fn_fallback(action: &str) {
    let Ok(runtime) = runtime() else {
        return;
    };
    let _ = runtime.app.emit("desktop-codex-live-fn-action", action);
}

fn runtime() -> Result<&'static Arc<Phase1Runtime>, String> {
    PHASE1_RUNTIME
        .get()
        .ok_or_else(|| "phase1 speech runtime is not the active owner".to_string())
}

fn send_control_input(input: SpeechControlInput) -> Result<(), String> {
    let runtime = runtime()?;
    let coordinator = runtime
        .coordinator
        .lock()
        .map_err(|_| "phase1 coordinator is poisoned".to_string())?;
    coordinator
        .send(input)
        .map_err(|error| format!("{error:?}"))?;
    coordinator.flush().map_err(|error| format!("{error:?}"))?;
    let snapshot = coordinator.snapshot();
    runtime
        .app
        .emit(SESSION_SNAPSHOT_EVENT, snapshot)
        .map_err(|error| format!("failed to emit speech snapshot: {error}"))
}

pub fn request_toggle() -> Result<(), String> {
    send_control_input(SpeechControlInput::FnPressed)
}

pub fn request_desired_state(desired: super::session::DesiredState) -> Result<(), String> {
    let snapshot = get_speech_control_snapshot()?;
    if snapshot.desired_state == desired {
        Ok(())
    } else {
        request_toggle()
    }
}

pub fn configure_current_recognition(
    contextual_strings: Option<Vec<String>>,
    recognition_mode: Option<String>,
) -> Result<(), String> {
    let identity = get_speech_control_snapshot()?
        .identity
        .ok_or_else(|| "speech session is not active".to_string())?;
    configure_speech_recognition(identity, contextual_strings, recognition_mode)
}

fn flush_and_emit_snapshot() {
    let Ok(runtime) = runtime() else {
        return;
    };
    let Ok(coordinator) = runtime.coordinator.lock() else {
        return;
    };
    let _ = coordinator.flush();
    let _ = runtime
        .app
        .emit(SESSION_SNAPSHOT_EVENT, coordinator.snapshot());
}

fn validate(identity: SpeechLayerIdentity) -> Result<(), String> {
    let snapshot = get_speech_control_snapshot()?;
    validate_control_identity(&snapshot, identity).map_err(|error| format!("{error:?}"))
}

#[tauri::command]
pub fn get_speech_control_snapshot() -> Result<SpeechSnapshot, String> {
    let runtime = runtime()?;
    runtime
        .coordinator
        .lock()
        .map_err(|_| "phase1 coordinator is poisoned".to_string())
        .map(|coordinator| coordinator.snapshot())
}

#[tauri::command]
pub fn ack_speech_overlay_visibility(
    identity: SpeechLayerIdentity,
    visible: bool,
) -> Result<(), String> {
    validate(identity)?;
    send_control_input(SpeechControlInput::OverlayVisibilityAcknowledged { identity, visible })
}

#[tauri::command]
pub fn configure_speech_recognition(
    identity: SpeechLayerIdentity,
    contextual_strings: Option<Vec<String>>,
    recognition_mode: Option<String>,
) -> Result<(), String> {
    validate(identity)?;
    let runtime = runtime()?;
    runtime.recognition.configure(RecognitionConfiguration {
        identity: Some(identity),
        contextual_strings: contextual_strings
            .unwrap_or_default()
            .into_iter()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty() && value.len() <= 80)
            .take(100)
            .collect(),
        mode: super::NativeSpeechRecognitionMode::from_option(recognition_mode),
    })
}

#[tauri::command]
pub fn complete_speech_processing(
    identity: SpeechLayerIdentity,
    text: String,
) -> Result<(), String> {
    super::debug_log("[transcript-processing-complete-received]", format!(
        "session={} revision={} text_len={}",
        identity.session_sequence, identity.revision, text.chars().count()));
    if let Err(error) = validate(identity) {
        super::debug_log("[transcript-processing-complete-result]", format!(
            "session={} revision={} outcome=identity-rejected", identity.session_sequence, identity.revision));
        return Err(error);
    }
    let result = send_control_input(SpeechControlInput::TranscriptProcessed { identity, text });
    super::debug_log("[transcript-processing-complete-result]", format!(
        "session={} revision={} outcome={}", identity.session_sequence, identity.revision,
        if result.is_ok() { "queued" } else { "queue-failed" }));
    result
}

fn start_native(identity: SpeechLayerIdentity, configuration: RecognitionConfiguration) {
    #[cfg(target_os = "macos")]
    unsafe {
        let contextual: Vec<CString> = configuration
            .contextual_strings
            .into_iter()
            .filter_map(|value| CString::new(value).ok())
            .collect();
        let pointers: Vec<*const c_char> = contextual.iter().map(|value| value.as_ptr()).collect();
        let pointer = if pointers.is_empty() {
            std::ptr::null()
        } else {
            pointers.as_ptr()
        };
        super::native_backend::start(
            identity,
            phase1_native_callback,
            std::ptr::null_mut(),
            pointer,
            pointers.len(),
            configuration.mode.force_on_device(),
        );
    }

    #[cfg(not(target_os = "macos"))]
    let _ = (identity, configuration);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_diagnostics_never_include_transcript_or_arbitrary_errors() {
        let secret = "private dictated text";
        for event in ["started", "partial", "final", "error"] {
            assert!(!native_callback_diagnostic(event, secret).unwrap().contains(secret));
        }
        assert_eq!(native_callback_diagnostic("diag-error",
            "stage=recognition-task domain=kAFAssistantErrorDomain code=1101").as_deref(),
            Some("event=error-detail stage=recognition-task domain=kAFAssistantErrorDomain code=1101"));
        for text in ["stage=private domain=none code=0",
            "stage=recognition-task domain=private code=0",
            "stage=recognition-task domain=none code=private",
            "stage=recognition-task domain=none code=0 private"] {
            assert!(native_callback_diagnostic("diag-error", text).is_none());
        }
    }

    #[test]
    fn recognition_configuration_follows_the_session_across_revision_changes() {
        let store = RecognitionConfigurationStore::new();
        let epoch = OwnerEpoch([9; 16]);
        let arming = SpeechLayerIdentity::new(epoch, 3, 4, 1);
        let native = SpeechLayerIdentity::new(epoch, 3, 4, 2);
        store
            .configure(RecognitionConfiguration {
                identity: Some(arming),
                contextual_strings: vec!["privacy".into()],
                mode: super::super::NativeSpeechRecognitionMode::Privacy,
            })
            .expect("configure");

        let configuration = store.for_session(native);
        assert_eq!(configuration.contextual_strings, vec!["privacy"]);
        assert_eq!(
            configuration.mode,
            super::super::NativeSpeechRecognitionMode::Privacy
        );
    }

    #[test]
    fn queued_release_wins_over_an_expired_wall_clock_hold_deadline() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let mut gesture = fn_listener::FnHoldGesture::default();
        let stale_press =
            fn_listener::monotonic_us().saturating_sub(fn_listener::FN_LIVE_LONG_PRESS_US + 1_000);
        gesture.press(stale_press);
        sender
            .send(FnListenerEvent::Released {
                observed_monotonic_us: stale_press + 1_000,
            })
            .unwrap();

        let release = receive_fn_gesture_event(&receiver, &gesture).expect("queued release");
        let FnListenerEvent::Released {
            observed_monotonic_us,
        } = release
        else {
            panic!("expected release event");
        };
        assert_eq!(
            gesture.release_at(observed_monotonic_us),
            fn_listener::FnReleaseAction::Short,
            "a release observed before the deadline stays short even if consumed late"
        );
    }

    #[test]
    fn release_observed_at_hold_deadline_is_a_long_action() {
        let mut gesture = fn_listener::FnHoldGesture::default();
        let pressed_at = fn_listener::monotonic_us();
        gesture.press(pressed_at);

        assert_eq!(
            gesture.release_at(pressed_at + fn_listener::FN_LIVE_LONG_PRESS_US),
            fn_listener::FnReleaseAction::Long
        );
    }

    #[test]
    fn committed_toggle_with_lost_response_never_emits_a_second_fallback() {
        let failure = fn_bridge_failure(
            FnBridgeFailureClass::AmbiguousAfterDispatch,
            "bridge committed, response timed out",
        );
        assert!(!should_fallback_desktop_codex_live_fn_action(&failure));
        assert!(should_fallback_desktop_codex_live_fn_action(
            &fn_bridge_failure(FnBridgeFailureClass::SafeBeforeCommit, "connection refused")
        ));
        assert!(should_fallback_desktop_codex_live_fn_action(
            &fn_bridge_failure(FnBridgeFailureClass::SafeRejected, "HTTP 400")
        ));
    }
}

fn native_callback_diagnostic(event: &str, text: &str) -> Option<String> {
    match event {
        "started" | "partial" | "final" | "error" => {
            Some(format!("event={event} text_len={}", text.chars().count()))
        }
        "diag-error" => {
            let fields: Vec<_> = text.split_whitespace().collect();
            if fields.len() != 3 { return None; }
            let stage = fields[0].strip_prefix("stage=")?;
            let domain = fields[1].strip_prefix("domain=")?;
            let code = fields[2].strip_prefix("code=")?.parse::<i64>().ok()?;
            if !["recognizer-unavailable", "recognition-task", "audio-engine-start",
                "speech-authorization", "microphone-authorization"].contains(&stage)
                || !["kAFAssistantErrorDomain", "SFSpeechErrorDomain", "NSOSStatusErrorDomain",
                    "AVFoundationErrorDomain", "NSCocoaErrorDomain", "none", "other"].contains(&domain) {
                return None;
            }
            Some(format!("event=error-detail stage={stage} domain={domain} code={code}"))
        }
        _ => None,
    }
}

extern "C" fn phase1_native_callback(
    identity: SpeechLayerIdentity,
    event_type: *const c_char,
    text: *const c_char,
    _user_data: *mut c_void,
) {
    if event_type.is_null() || text.is_null() {
        return;
    }
    let event_type = unsafe { CStr::from_ptr(event_type) }.to_string_lossy();
    let text = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    if let Some(diagnostic) = native_callback_diagnostic(&event_type, &text) {
        super::debug_log("[native-callback]", format!(
            "session={} revision={} {diagnostic}", identity.session_sequence, identity.revision));
    }
    match event_type.as_ref() {
        "started" => super::update_runtime_state(|state| {
            state.recognition_mode = Some(text.to_string());
        }),
        "partial" => super::record_partial_length(text.chars().count()),
        "final" => super::record_final_length(text.chars().count()),
        "error" => super::record_runtime_error(text.as_ref()),
        _ => {}
    }
    if let Some(input) =
        super::native_backend::map_bridge_callback(identity, identity, &event_type, &text)
    {
        if send_control_input(input).is_err() {
            super::debug_log("[native-callback-queue-failed]", format!(
                "session={} revision={}", identity.session_sequence, identity.revision));
        } else {
            if let Ok(snapshot) = get_speech_control_snapshot() {
                super::debug_log("[native-callback-state]", format!(
                    "session={} revision={} phase={:?} outcome={:?}",
                    identity.session_sequence, identity.revision, snapshot.phase, snapshot.writeback_outcome));
            }
        }
    }
}
