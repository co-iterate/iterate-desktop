use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use tauri::Emitter;

const KEY_CODE_FN: u16 = 63;
const KEY_CODE_CONTROL_LEFT: u16 = 59;
const KEY_CODE_CONTROL_RIGHT: u16 = 62;
pub const CONTROL_TAP_MAX_US: u64 = 800_000;
const HEALTH_STARTING: u8 = 0;
const HEALTH_RUNNING: u8 = 1;
const HEALTH_DISABLED: u8 = 2;
const HEALTH_ENDED: u8 = 3;
const TAP_HEALTH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);
const TAP_RESUME_GAP: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TapRecoveryReason {
    InvalidPort,
    DisabledWithoutCallback,
    ResumeGap,
}

fn tap_recovery_reason(
    valid: bool,
    enabled: bool,
    elapsed: Option<std::time::Duration>,
) -> Option<TapRecoveryReason> {
    if !valid {
        Some(TapRecoveryReason::InvalidPort)
    } else if !enabled {
        Some(TapRecoveryReason::DisabledWithoutCallback)
    } else if elapsed.is_none_or(|gap| gap >= TAP_RESUME_GAP) {
        Some(TapRecoveryReason::ResumeGap)
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TapDisabledReason {
    Timeout,
    UserInput,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnListenerEvent {
    Pressed { observed_monotonic_us: u64 },
    Released { observed_monotonic_us: u64 },
    ControlTapped,
    TapDisabled { reason: TapDisabledReason },
    TapEnded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FnRawEventKind {
    FlagsChanged {
        keycode: u16,
        secondary_fn: bool,
        control: bool,
        other_modifier: bool,
    },
    GestureInterference,
    TapDisabled(TapDisabledReason),
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FnRawEvent {
    kind: FnRawEventKind,
    observed_monotonic_us: u64,
}

impl FnRawEvent {
    pub fn flags_changed(keycode: u16, secondary_fn: bool, observed_monotonic_us: u64) -> Self {
        Self::flags_changed_with_modifiers(
            keycode,
            secondary_fn,
            false,
            false,
            observed_monotonic_us,
        )
    }

    pub fn flags_changed_with_modifiers(
        keycode: u16,
        secondary_fn: bool,
        control: bool,
        other_modifier: bool,
        observed_monotonic_us: u64,
    ) -> Self {
        Self {
            kind: FnRawEventKind::FlagsChanged {
                keycode,
                secondary_fn,
                control,
                other_modifier,
            },
            observed_monotonic_us,
        }
    }

    pub fn gesture_interference(observed_monotonic_us: u64) -> Self {
        Self {
            kind: FnRawEventKind::GestureInterference,
            observed_monotonic_us,
        }
    }

    pub fn tap_disabled(reason: TapDisabledReason, observed_monotonic_us: u64) -> Self {
        Self {
            kind: FnRawEventKind::TapDisabled(reason),
            observed_monotonic_us,
        }
    }

    pub fn other(observed_monotonic_us: u64) -> Self {
        Self {
            kind: FnRawEventKind::Other,
            observed_monotonic_us,
        }
    }
}

#[derive(Debug, Default)]
pub struct FnEdgeDecoder {
    fn_down: bool,
    control_keycode: Option<u16>,
    control_pressed_at_us: Option<u64>,
    control_cancelled: bool,
    recreation_requested: bool,
}

impl FnEdgeDecoder {
    pub fn decode(&mut self, event: FnRawEvent) -> Option<FnListenerEvent> {
        match event.kind {
            FnRawEventKind::FlagsChanged {
                keycode,
                secondary_fn,
                control,
                other_modifier,
            } => {
                if matches!(keycode, KEY_CODE_CONTROL_LEFT | KEY_CODE_CONTROL_RIGHT) {
                    return self.decode_control_flags(
                        keycode,
                        control,
                        other_modifier,
                        event.observed_monotonic_us,
                    );
                }
                if self.control_keycode.is_some() {
                    self.control_cancelled = true;
                }
                if keycode != KEY_CODE_FN {
                    return None;
                }
                if secondary_fn {
                    if !self.fn_down {
                        self.fn_down = true;
                        return Some(FnListenerEvent::Pressed {
                            observed_monotonic_us: event.observed_monotonic_us,
                        });
                    }
                } else if self.fn_down {
                    self.fn_down = false;
                    return Some(FnListenerEvent::Released {
                        observed_monotonic_us: event.observed_monotonic_us,
                    });
                }
                None
            }
            FnRawEventKind::GestureInterference => {
                if self.control_keycode.is_some() {
                    self.control_cancelled = true;
                }
                None
            }
            FnRawEventKind::TapDisabled(reason) => {
                self.fn_down = false;
                self.reset_control();
                self.recreation_requested = true;
                Some(FnListenerEvent::TapDisabled { reason })
            }
            _ => None,
        }
    }

    fn decode_control_flags(
        &mut self,
        keycode: u16,
        control_down: bool,
        other_modifier: bool,
        observed_monotonic_us: u64,
    ) -> Option<FnListenerEvent> {
        if control_down {
            match self.control_keycode {
                None => {
                    self.control_keycode = Some(keycode);
                    self.control_pressed_at_us = Some(observed_monotonic_us);
                    self.control_cancelled = other_modifier;
                }
                Some(active_keycode) if active_keycode == keycode => {
                    self.control_cancelled |= other_modifier;
                }
                Some(_) => {
                    self.control_cancelled = true;
                }
            }
            return None;
        }

        let is_matching_release = self.control_keycode == Some(keycode);
        let duration_us = self
            .control_pressed_at_us
            .map(|pressed_at| observed_monotonic_us.saturating_sub(pressed_at));
        let should_trigger = is_matching_release
            && !self.control_cancelled
            && !other_modifier
            && duration_us.is_some_and(|duration| duration <= CONTROL_TAP_MAX_US);
        self.reset_control();
        should_trigger.then_some(FnListenerEvent::ControlTapped)
    }

    fn reset_control(&mut self) {
        self.control_keycode = None;
        self.control_pressed_at_us = None;
        self.control_cancelled = false;
    }

    pub fn recreation_requested(&self) -> bool {
        self.recreation_requested
    }

    pub fn clear_recreation_request(&mut self) {
        self.recreation_requested = false;
    }
}

pub const FN_LIVE_LONG_PRESS_US: u64 = 5_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnReleaseAction {
    None,
    Short,
    Long,
}

#[derive(Debug, Default)]
pub struct FnHoldGesture {
    pressed_at_us: Option<u64>,
    long_press_triggered: bool,
}

impl FnHoldGesture {
    pub fn press(&mut self, observed_monotonic_us: u64) {
        if self.pressed_at_us.is_none() {
            self.pressed_at_us = Some(observed_monotonic_us);
            self.long_press_triggered = false;
        }
    }

    pub fn remaining_until_long_press(&self, now_us: u64) -> Option<std::time::Duration> {
        if self.long_press_triggered {
            return None;
        }
        let pressed_at_us = self.pressed_at_us?;
        Some(std::time::Duration::from_micros(
            FN_LIVE_LONG_PRESS_US.saturating_sub(now_us.saturating_sub(pressed_at_us)),
        ))
    }

    pub fn trigger_long_press_if_due(&mut self, now_us: u64) -> bool {
        let Some(pressed_at_us) = self.pressed_at_us else {
            return false;
        };
        if self.long_press_triggered || now_us.saturating_sub(pressed_at_us) < FN_LIVE_LONG_PRESS_US
        {
            return false;
        }
        self.long_press_triggered = true;
        true
    }

    pub fn release(&mut self) -> bool {
        let was_pressed = self.pressed_at_us.take().is_some();
        let is_short_press = was_pressed && !self.long_press_triggered;
        self.long_press_triggered = false;
        is_short_press
    }

    pub fn release_at(&mut self, observed_monotonic_us: u64) -> FnReleaseAction {
        let long_press_became_due = self.trigger_long_press_if_due(observed_monotonic_us);
        let is_short_press = self.release();
        if long_press_became_due {
            FnReleaseAction::Long
        } else if is_short_press {
            FnReleaseAction::Short
        } else {
            FnReleaseAction::None
        }
    }

    pub fn reset(&mut self) {
        self.pressed_at_us = None;
        self.long_press_triggered = false;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PressDrain {
    pub press_count: u64,
    pub latest_sequence: u64,
}

#[derive(Debug, Default)]
pub struct PressDelivery {
    observed_sequence: AtomicU64,
    coalesced_wakes: AtomicU64,
    disconnected_wakes: AtomicU64,
}

impl PressDelivery {
    pub fn observe_press(&self, sender: &Sender<FnListenerEvent>, observed_monotonic_us: u64) {
        self.observed_sequence.fetch_add(1, Ordering::AcqRel);
        if sender
            .send(FnListenerEvent::Pressed {
                observed_monotonic_us,
            })
            .is_err()
        {
            self.disconnected_wakes.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn drain_after(&self, consumed_sequence: u64) -> PressDrain {
        let latest_sequence = self.observed_sequence.load(Ordering::Acquire);
        PressDrain {
            press_count: latest_sequence.saturating_sub(consumed_sequence),
            latest_sequence,
        }
    }

    pub fn observed_sequence(&self) -> u64 {
        self.observed_sequence.load(Ordering::Acquire)
    }

    pub fn coalesced_wake_count(&self) -> u64 {
        self.coalesced_wakes.load(Ordering::Relaxed)
    }

    pub fn disconnected_wake_count(&self) -> u64 {
        self.disconnected_wakes.load(Ordering::Relaxed)
    }
}

fn global_press_delivery() -> &'static PressDelivery {
    static DELIVERY: OnceLock<PressDelivery> = OnceLock::new();
    DELIVERY.get_or_init(PressDelivery::default)
}

pub fn drain_observed_presses_after(consumed_sequence: u64) -> PressDrain {
    global_press_delivery().drain_after(consumed_sequence)
}

/// Delivers lifecycle state after the event-tap callback has returned. Blocking here is safe and
/// guarantees that a full wake queue cannot hide listener termination from its supervisor.
pub fn deliver_terminal_event(sender: &Sender<FnListenerEvent>, event: FnListenerEvent) -> bool {
    if matches!(event, FnListenerEvent::Pressed { .. }) {
        return false;
    }
    sender.send(event).is_ok()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FnTapLocation {
    Session,
    HidFallback,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FnListenerError {
    TapCreationFailed,
    StartupTimedOut,
    UnsupportedPlatform,
}

impl std::fmt::Display for FnListenerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for FnListenerError {}

pub struct FnListenerHandle {
    stop_requested: Arc<AtomicBool>,
    health: Arc<AtomicU8>,
    location: Arc<Mutex<Option<FnTapLocation>>>,
    #[cfg(target_os = "macos")]
    run_loop: Arc<Mutex<Option<core_foundation::runloop::CFRunLoop>>>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl FnListenerHandle {
    pub fn stop(&self) -> bool {
        if self.stop_requested.swap(true, Ordering::AcqRel) {
            return false;
        }
        #[cfg(target_os = "macos")]
        if let Ok(run_loop) = self.run_loop.lock() {
            if let Some(run_loop) = run_loop.as_ref() {
                run_loop.stop();
            }
        }
        true
    }

    pub fn is_stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }

    pub fn is_healthy(&self) -> bool {
        self.health.load(Ordering::Acquire) == HEALTH_RUNNING
    }

    pub fn tap_location(&self) -> Option<FnTapLocation> {
        self.location.lock().ok().and_then(|location| *location)
    }

    pub fn join(&self) {
        if let Ok(mut join) = self.join.lock() {
            if let Some(join) = join.take() {
                let _ = join.join();
            }
        }
    }

    #[doc(hidden)]
    pub fn detached_for_test() -> Self {
        Self {
            stop_requested: Arc::new(AtomicBool::new(false)),
            health: Arc::new(AtomicU8::new(HEALTH_STARTING)),
            location: Arc::new(Mutex::new(None)),
            #[cfg(target_os = "macos")]
            run_loop: Arc::new(Mutex::new(None)),
            join: Mutex::new(None),
        }
    }
}

pub(crate) fn monotonic_us() -> u64 {
    static ORIGIN: OnceLock<std::time::Instant> = OnceLock::new();
    ORIGIN
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros()
        .min(u64::MAX as u128) as u64
}

#[cfg(target_os = "macos")]
pub fn start_transport(
    sender: Sender<FnListenerEvent>,
) -> Result<FnListenerHandle, FnListenerError> {
    use core_foundation::base::TCFType;
    use core_foundation::mach_port::{CFMachPortIsValid, CFMachPortRef};
    use core_foundation::runloop::{
        kCFRunLoopCommonModes, kCFRunLoopDefaultMode, CFRunLoop, CFRunLoopRunResult,
    };

    extern "C" {
        fn CGEventTapIsEnabled(tap: CFMachPortRef) -> bool;
    }
    use core_graphics::event::{
        CGEvent, CGEventFlags, CGEventTap, CGEventTapLocation, CGEventTapOptions,
        CGEventTapPlacement, CGEventType, CallbackResult, EventField,
    };
    use std::sync::mpsc::sync_channel;
    use std::time::Duration;

    let stop_requested = Arc::new(AtomicBool::new(false));
    let health = Arc::new(AtomicU8::new(HEALTH_STARTING));
    let location = Arc::new(Mutex::new(None));
    let run_loop = Arc::new(Mutex::new(None));
    let (startup_sender, startup_receiver) = sync_channel(1);

    let thread_stop = stop_requested.clone();
    let thread_health = health.clone();
    let thread_location = location.clone();
    let thread_run_loop = run_loop.clone();
    let join = std::thread::spawn(move || {
        fn create_tap(
            location: CGEventTapLocation,
            sender: Sender<FnListenerEvent>,
            run_loop: CFRunLoop,
            health: Arc<AtomicU8>,
        ) -> Result<CGEventTap<'static>, ()> {
            let decoder = Arc::new(Mutex::new(FnEdgeDecoder::default()));
            CGEventTap::new(
                location,
                CGEventTapPlacement::HeadInsertEventTap,
                CGEventTapOptions::ListenOnly,
                vec![
                    CGEventType::FlagsChanged,
                    CGEventType::KeyDown,
                    CGEventType::LeftMouseDown,
                    CGEventType::RightMouseDown,
                    CGEventType::OtherMouseDown,
                    CGEventType::ScrollWheel,
                ],
                move |_proxy, event_type, event: &CGEvent| {
                    let now = monotonic_us();
                    let raw = match event_type {
                        CGEventType::FlagsChanged => {
                            let flags = event.get_flags();
                            FnRawEvent::flags_changed_with_modifiers(
                                event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE)
                                    as u16,
                                flags.contains(CGEventFlags::CGEventFlagSecondaryFn),
                                flags.contains(CGEventFlags::CGEventFlagControl),
                                flags.intersects(
                                    CGEventFlags::CGEventFlagShift
                                        | CGEventFlags::CGEventFlagAlternate
                                        | CGEventFlags::CGEventFlagCommand
                                        | CGEventFlags::CGEventFlagSecondaryFn,
                                ),
                                now,
                            )
                        }
                        CGEventType::KeyDown
                        | CGEventType::LeftMouseDown
                        | CGEventType::RightMouseDown
                        | CGEventType::OtherMouseDown
                        | CGEventType::ScrollWheel => FnRawEvent::gesture_interference(now),
                        CGEventType::TapDisabledByTimeout => {
                            FnRawEvent::tap_disabled(TapDisabledReason::Timeout, now)
                        }
                        CGEventType::TapDisabledByUserInput => {
                            FnRawEvent::tap_disabled(TapDisabledReason::UserInput, now)
                        }
                        _ => FnRawEvent::other(now),
                    };
                    let decoded = decoder
                        .lock()
                        .ok()
                        .and_then(|mut decoder| decoder.decode(raw));
                    match decoded {
                        Some(FnListenerEvent::Pressed {
                            observed_monotonic_us,
                        }) => global_press_delivery().observe_press(&sender, observed_monotonic_us),
                        Some(event @ FnListenerEvent::Released { .. }) => {
                            let _ = sender.send(event);
                        }
                        Some(event @ FnListenerEvent::ControlTapped) => {
                            let _ = sender.send(event);
                        }
                        Some(event @ FnListenerEvent::TapDisabled { .. }) => {
                            health.store(HEALTH_DISABLED, Ordering::Release);
                            let _ = sender.send(event);
                            run_loop.stop();
                        }
                        _ => {}
                    }
                    CallbackResult::Keep
                },
            )
        }

        let current_run_loop = CFRunLoop::get_current();
        if let Ok(mut shared_run_loop) = thread_run_loop.lock() {
            *shared_run_loop = Some(current_run_loop.clone());
        }

        let session_tap = create_tap(
            CGEventTapLocation::Session,
            sender.clone(),
            current_run_loop.clone(),
            thread_health.clone(),
        );
        let (tap, actual_location) = match session_tap {
            Ok(tap) => (tap, FnTapLocation::Session),
            Err(()) => match create_tap(
                CGEventTapLocation::HID,
                sender.clone(),
                current_run_loop.clone(),
                thread_health.clone(),
            ) {
                Ok(tap) => (tap, FnTapLocation::HidFallback),
                Err(()) => {
                    let _ = startup_sender.send(Err(FnListenerError::TapCreationFailed));
                    thread_health.store(HEALTH_ENDED, Ordering::Release);
                    return;
                }
            },
        };

        if let Ok(mut shared_location) = thread_location.lock() {
            *shared_location = Some(actual_location);
        }
        let source = tap.mach_port().create_runloop_source(0);
        let Ok(source) = source else {
            let _ = startup_sender.send(Err(FnListenerError::TapCreationFailed));
            thread_health.store(HEALTH_ENDED, Ordering::Release);
            return;
        };
        current_run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });
        tap.enable();
        thread_health.store(HEALTH_RUNNING, Ordering::Release);
        let _ = startup_sender.send(Ok(actual_location));

        super::debug_log(
            "[fn-tap-ready]",
            format!(
                "location={actual_location:?} health_interval_ms={}",
                TAP_HEALTH_INTERVAL.as_millis()
            ),
        );
        let mut checked_at = std::time::SystemTime::now();
        while !thread_stop.load(Ordering::Acquire)
            && thread_health.load(Ordering::Acquire) == HEALTH_RUNNING
        {
            // A disabled/invalid tap need not deliver another callback. Keep the run loop
            // bounded so the owner can rebuild the transport without restarting the app.
            let result = CFRunLoop::run_in_mode(
                unsafe { kCFRunLoopDefaultMode },
                TAP_HEALTH_INTERVAL,
                false,
            );
            if thread_stop.load(Ordering::Acquire)
                || thread_health.load(Ordering::Acquire) != HEALTH_RUNNING
            {
                break;
            }
            let now = std::time::SystemTime::now();
            let port = tap.mach_port().as_concrete_TypeRef();
            let valid = unsafe { CFMachPortIsValid(port) != 0 };
            let enabled = valid && unsafe { CGEventTapIsEnabled(port) };
            if let Some(reason) =
                tap_recovery_reason(valid, enabled, now.duration_since(checked_at).ok())
            {
                super::debug_log(
                    "[fn-tap-recovery]",
                    format!("reason={reason:?} location={actual_location:?}"),
                );
                thread_health.store(HEALTH_DISABLED, Ordering::Release);
                break;
            }
            checked_at = now;
            if matches!(
                result,
                CFRunLoopRunResult::Finished | CFRunLoopRunResult::Stopped
            ) {
                super::debug_log("[fn-tap-ended]", format!("run_loop={result:?}"));
                break;
            }
        }
        current_run_loop.remove_source(&source, unsafe { kCFRunLoopCommonModes });
        if let Ok(mut shared_run_loop) = thread_run_loop.lock() {
            *shared_run_loop = None;
        }
        if thread_health.load(Ordering::Acquire) != HEALTH_DISABLED {
            thread_health.store(HEALTH_ENDED, Ordering::Release);
        }
        if thread_stop.load(Ordering::Acquire) {
            let _ = sender.send(FnListenerEvent::TapEnded);
        } else {
            let _ = deliver_terminal_event(&sender, FnListenerEvent::TapEnded);
        }
    });

    match startup_receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(_)) => Ok(FnListenerHandle {
            stop_requested,
            health,
            location,
            run_loop,
            join: Mutex::new(Some(join)),
        }),
        Ok(Err(error)) => {
            let _ = join.join();
            Err(error)
        }
        Err(_) => {
            stop_requested.store(true, Ordering::Release);
            if let Ok(run_loop) = run_loop.lock() {
                if let Some(run_loop) = run_loop.as_ref() {
                    run_loop.stop();
                }
            }
            Err(FnListenerError::StartupTimedOut)
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub fn start_transport(
    _sender: Sender<FnListenerEvent>,
) -> Result<FnListenerHandle, FnListenerError> {
    Err(FnListenerError::UnsupportedPlatform)
}

/// Compatibility supervisor for the current desktop command path. The event tap callback itself
/// remains transport-only; all app work happens on this consumer thread.
pub fn start(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let mut consumed_sequence = global_press_delivery().observed_sequence();
        loop {
            let (sender, receiver) = std::sync::mpsc::channel();
            let handle = match start_transport(sender) {
                Ok(handle) => handle,
                Err(error) => {
                    super::debug_log("[fn-listener-install-failed]", error.to_string());
                    std::thread::sleep(std::time::Duration::from_secs(1));
                    continue;
                }
            };
            super::debug_log(
                "[fn-listener-installed]",
                format!("location={:?}", handle.tap_location()),
            );
            while let Ok(event) = receiver.recv() {
                match event {
                    FnListenerEvent::Pressed { .. } => {
                        let drain = drain_observed_presses_after(consumed_sequence);
                        for _ in 0..drain.press_count {
                            super::trigger_global_toggle(&app);
                        }
                        consumed_sequence = drain.latest_sequence;
                    }
                    FnListenerEvent::Released { .. } => {}
                    FnListenerEvent::ControlTapped => {
                        if super::codex_live_audio_reserved() {
                            let _ = app.emit("desktop-codex-live-fn-action", "mute");
                        }
                    }
                    FnListenerEvent::TapDisabled { reason } => {
                        super::debug_log("[fn-listener-disabled]", format!("reason={reason:?}"));
                        break;
                    }
                    FnListenerEvent::TapEnded => break,
                }
            }
            handle.stop();
            handle.join();
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tap_health_recovers_without_a_disabled_callback() {
        use std::time::Duration;
        assert_eq!(
            tap_recovery_reason(true, false, Some(Duration::ZERO)),
            Some(TapRecoveryReason::DisabledWithoutCallback)
        );
        assert_eq!(
            tap_recovery_reason(false, true, Some(Duration::ZERO)),
            Some(TapRecoveryReason::InvalidPort)
        );
    }

    #[test]
    fn tap_health_rebuilds_after_sleep_or_clock_discontinuity_but_not_idle() {
        assert_eq!(
            tap_recovery_reason(true, true, Some(TAP_RESUME_GAP)),
            Some(TapRecoveryReason::ResumeGap)
        );
        assert_eq!(
            tap_recovery_reason(true, true, None),
            Some(TapRecoveryReason::ResumeGap)
        );
        // No key events for hours is healthy if the run loop keeps checking.
        for _ in 0..10_000 {
            assert_eq!(
                tap_recovery_reason(true, true, Some(TAP_HEALTH_INTERVAL)),
                None
            );
        }
    }

    #[test]
    fn fn_edge_decoder_reports_one_press_and_matching_release() {
        let mut decoder = FnEdgeDecoder::default();
        assert_eq!(
            decoder.decode(FnRawEvent::flags_changed(KEY_CODE_FN, true, 10)),
            Some(FnListenerEvent::Pressed {
                observed_monotonic_us: 10,
            })
        );
        assert_eq!(
            decoder.decode(FnRawEvent::flags_changed(KEY_CODE_FN, true, 20)),
            None
        );
        assert_eq!(
            decoder.decode(FnRawEvent::flags_changed(KEY_CODE_FN, false, 30)),
            Some(FnListenerEvent::Released {
                observed_monotonic_us: 30,
            })
        );
        assert_eq!(
            decoder.decode(FnRawEvent::flags_changed(KEY_CODE_FN, false, 40)),
            None
        );
    }

    #[test]
    fn fn_hold_gesture_separates_short_press_from_five_second_toggle() {
        let mut gesture = FnHoldGesture::default();
        gesture.press(1_000);
        assert_eq!(
            gesture.remaining_until_long_press(2_000),
            Some(std::time::Duration::from_micros(4_999_000))
        );
        assert!(!gesture.trigger_long_press_if_due(5_000_999));
        assert!(
            gesture.release(),
            "release before five seconds is a short press"
        );

        gesture.press(10_000_000);
        assert!(gesture.trigger_long_press_if_due(15_000_000));
        assert!(!gesture.trigger_long_press_if_due(16_000_000));
        assert!(gesture.remaining_until_long_press(16_000_000).is_none());
        assert!(
            !gesture.release(),
            "release after the long action must not emit a second short action"
        );

        gesture.press(20_000_000);
        assert_eq!(
            gesture.release_at(25_000_000),
            FnReleaseAction::Long,
            "a release observed exactly at five seconds must still emit the long action"
        );
    }

    #[test]
    fn fn_press_and_release_edges_are_never_dropped_while_consumer_is_busy() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let delivery = PressDelivery::default();

        for index in 0..64_u64 {
            let pressed_at = index * 2;
            delivery.observe_press(&sender, pressed_at);
            sender
                .send(FnListenerEvent::Released {
                    observed_monotonic_us: pressed_at + 1,
                })
                .unwrap();
        }

        for index in 0..64_u64 {
            let pressed_at = index * 2;
            assert_eq!(
                receiver.recv().unwrap(),
                FnListenerEvent::Pressed {
                    observed_monotonic_us: pressed_at,
                }
            );
            assert_eq!(
                receiver.recv().unwrap(),
                FnListenerEvent::Released {
                    observed_monotonic_us: pressed_at + 1,
                }
            );
        }
        assert_eq!(delivery.disconnected_wake_count(), 0);
    }
}
