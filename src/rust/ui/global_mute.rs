use super::global_mute_store::{self as store, Snapshot};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

#[derive(Default)]
pub struct GlobalMuteState(Mutex<Option<Snapshot>>);

fn preference_path() -> Result<PathBuf, String> {
    crate::config::cunzhi_config_dir()
        .map(|dir| dir.join("global-mute.state"))
        .map_err(|error| error.to_string())
}

async fn synchronize(app: &AppHandle, snapshot: Snapshot) -> Result<(), String> {
    let handle = app.clone();
    let (tx, rx) = tokio::sync::oneshot::channel();
    // Every access to this cache and all native operations run on the main thread.
    // A worker must never hold the mutex while waiting for a native window getter.
    app.run_on_main_thread(move || {
        let result = (|| {
            let state = handle.state::<GlobalMuteState>();
            let mut previous = state.0.lock().map_err(|e| e.to_string())?;
            if previous.is_some_and(|old| old.0 >= snapshot.0) {
                return Ok(());
            }
            if store::should_minimize(*previous, snapshot) {
                if let Some(audio) = handle.try_state::<super::audio::AudioController>() {
                    audio
                        .should_stop
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                }
                if let Some(window) = handle.get_webview_window("main") {
                    #[cfg(not(any(target_os = "android", target_os = "ios")))]
                    // A new hidden WebView must finish request readiness first.
                    // present_mcp_window handles its initial minimization.
                    if window.is_visible().unwrap_or(false) {
                        window.minimize().map_err(|e| e.to_string())?;
                    }
                }
            }
            // No show/unminimize on unmute: old Dock windows remain minimized.
            *previous = Some(snapshot);
            drop(previous);
            handle
                .emit("global-mute-changed", snapshot)
                .map_err(|e| e.to_string())
        })();
        let _ = tx.send(result);
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

pub fn is_globally_muted() -> Result<bool, String> {
    Ok(store::read(&preference_path()?)
        .map_err(|e| e.to_string())?
        .is_some_and(|snapshot| snapshot.1))
}

#[tauri::command]
pub async fn get_global_mute(
    app: AppHandle,
    legacy_muted: Option<bool>,
) -> Result<Snapshot, String> {
    let snapshot = store::initialize(&preference_path()?, legacy_muted.unwrap_or(false))
        .map_err(|e| e.to_string())?;
    synchronize(&app, snapshot).await?;
    Ok(snapshot)
}

#[tauri::command]
pub async fn set_global_mute(app: AppHandle, muted: bool) -> Result<Snapshot, String> {
    let snapshot = store::set(&preference_path()?, muted).map_err(|e| e.to_string())?;
    synchronize(&app, snapshot).await?;
    Ok(snapshot)
}

/// Native polling continues when a minimized WebView is throttled. No network/extra service.
pub fn start_global_mute_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let Ok(path) = preference_path() else { return };
        let mut timer = tokio::time::interval(Duration::from_millis(200));
        let mut observed = None;
        loop {
            timer.tick().await;
            if let Err(error) = super::popup_restore::synchronize(&app).await {
                log::debug!("同步弹窗恢复快捷键失败: {error}");
            }
            if let Ok(Some(snapshot)) = store::read(&path) {
                if observed != Some(snapshot) {
                    match synchronize(&app, snapshot).await {
                        Ok(()) => observed = Some(snapshot),
                        Err(error) => log::warn!("同步全局静音失败: {error}"),
                    }
                }
            }
        }
    });
}

/// Read the shared preference at the final presentation boundary, even for forced requests.
#[tauri::command]
pub async fn present_mcp_window(app: AppHandle) -> Result<(), String> {
    let snapshot = store::read(&preference_path()?)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "全局静音尚未初始化".to_string())?;
    synchronize(&app, snapshot).await?;
    if snapshot.1 {
        if let Some(window) = app.get_webview_window("main") {
            #[cfg(not(any(target_os = "android", target_os = "ios")))]
            window.minimize().map_err(|e| e.to_string())?;
        }
        Ok(())
    } else {
        super::commands::center_window(app).await
    }
}

/// Input focus is allowed after the user opens a Dock window, including while still muted.
#[tauri::command]
pub fn can_focus_popup_input(app: AppHandle) -> bool {
    app.get_webview_window("main").is_some_and(|window| {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        if window.is_minimized().unwrap_or(true) {
            return false;
        }
        window.is_visible().unwrap_or(false) && window.is_focused().unwrap_or(false)
    })
}
