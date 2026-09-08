//! Owner-gated dynamic Tauri overlay for the Phase 1 desktop speech runtime.

use super::owner::SpeechProcessRole;
use super::session::OwnerEpoch;
use super::SPEECH_OVERLAY_WINDOW_LABEL;
use tauri::utils::config::BackgroundThrottlingPolicy;
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const SPEECH_OVERLAY_URL: &str = "index.html?view=speech-overlay";

pub fn overlay_creation_allowed(role: SpeechProcessRole, owner_epoch: Option<OwnerEpoch>) -> bool {
    matches!(role, SpeechProcessRole::CanonicalGui) && owner_epoch.is_some()
}

pub fn ensure_owner_overlay(
    app: &AppHandle,
    role: SpeechProcessRole,
    owner_epoch: Option<OwnerEpoch>,
) -> Result<WebviewWindow, String> {
    if !overlay_creation_allowed(role, owner_epoch) {
        return Err("speech overlay creation requires the acquired canonical GUI owner".into());
    }
    if let Some(window) = app.get_webview_window(SPEECH_OVERLAY_WINDOW_LABEL) {
        return Ok(window);
    }
    WebviewWindowBuilder::new(
        app,
        SPEECH_OVERLAY_WINDOW_LABEL,
        WebviewUrl::App(SPEECH_OVERLAY_URL.into()),
    )
    .title("iterate-speech-overlay")
    .inner_size(96.0, 48.0)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .visible(false)
    .focused(false)
    .focusable(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .background_throttling(BackgroundThrottlingPolicy::Disabled)
    .build()
    .map_err(|error| format!("failed to create owner speech overlay: {error}"))
}

#[cfg(target_os = "windows")]
pub fn ensure_windows_overlay(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(window) = app.get_webview_window(SPEECH_OVERLAY_WINDOW_LABEL) {
        return Ok(window);
    }
    WebviewWindowBuilder::new(
        app,
        SPEECH_OVERLAY_WINDOW_LABEL,
        WebviewUrl::App(SPEECH_OVERLAY_URL.into()),
    )
    .title("iterate-speech-overlay")
    .inner_size(96.0, 48.0)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .visible(false)
    .focused(false)
    .focusable(false)
    .always_on_top(true)
    .visible_on_all_workspaces(true)
    .skip_taskbar(true)
    .background_throttling(BackgroundThrottlingPolicy::Disabled)
    .build()
    .map_err(|error| format!("failed to create Windows speech overlay: {error}"))
}
