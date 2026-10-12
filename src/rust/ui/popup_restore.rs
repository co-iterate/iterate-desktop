//! Native shortcut ownership and exact-popup restore, including separate GUI processes.
use super::popup_restore_store::{self as store, Owner, ProcessIdentity, Target};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{atomic::Ordering, Mutex};
use tauri::{AppHandle, Manager, WebviewWindow, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

const RESTORE_KEY: &str = "Shift+Tab";

struct Local {
    owner: Option<Owner>,
    enabled: HashSet<String>,
    observed: HashSet<String>,
    owns_shortcut: bool,
}
impl Default for Local {
    fn default() -> Self {
        Self {
            owner: None,
            enabled: HashSet::new(),
            observed: HashSet::new(),
            owns_shortcut: false,
        }
    }
}
#[derive(Default)]
pub struct PopupRestoreState(Mutex<Local>);

fn path() -> Result<PathBuf, String> {
    crate::config::cunzhi_config_dir()
        .map(|root| root.join("popup-restore.state.json"))
        .map_err(|e| e.to_string())
}
fn allowed(app: &AppHandle) -> bool {
    app.try_state::<crate::config::AppState>()
        .is_some_and(|state| state.global_shortcut_enabled.load(Ordering::Relaxed))
        && !super::global_mute::is_globally_muted().unwrap_or(true)
}
fn local_owner(app: &AppHandle) -> Result<Owner, String> {
    let state = app.state::<PopupRestoreState>();
    let mut local = state.0.lock().map_err(|e| e.to_string())?;
    if local.owner.is_none() {
        local.owner = Some(Owner {
            process: ProcessIdentity::current().map_err(|e| e.to_string())?,
            runtime_id: uuid::Uuid::new_v4().to_string(),
        });
    }
    Ok(local.owner.clone().unwrap())
}
async fn on_main<T: Send + 'static>(
    app: AppHandle,
    action: impl FnOnce(&AppHandle) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.send(action(&handle));
    })
    .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())?
}

fn release_shortcut(app: &AppHandle, owner: &Owner) -> Result<(), String> {
    let owned = app
        .state::<PopupRestoreState>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .owns_shortcut;
    if !owned {
        return Ok(());
    }
    app.global_shortcut()
        .unregister(RESTORE_KEY)
        .map_err(|e| e.to_string())?;
    app.state::<PopupRestoreState>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .owns_shortcut = false;
    store::edit(&path()?, |state| {
        if state.shortcut_owner.as_ref() == Some(owner) {
            state.shortcut_owner = None;
        }
    })
    .map_err(|e| e.to_string())
}

fn restore_window(window: &WebviewWindow) -> Result<(), String> {
    window.unminimize().map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        use objc::{class, msg_send, sel, sel_impl};
        let application: *mut objc::runtime::Object =
            unsafe { msg_send![class!(NSApplication), sharedApplication] };
        if !application.is_null() {
            let _: () = unsafe { msg_send![application, activateIgnoringOtherApps: true] };
        }
    }
    window.set_focus().map_err(|e| e.to_string())
}

fn synchronize_main(app: &AppHandle) -> Result<(), String> {
    let (enabled, owns) = {
        let state = app.state::<PopupRestoreState>();
        let local = state.0.lock().map_err(|e| e.to_string())?;
        (local.enabled.clone(), local.owns_shortcut)
    };
    if enabled.is_empty() && !owns {
        return Ok(());
    }
    let owner = local_owner(app)?;
    let state_path = path()?;
    let mut snapshot = store::read(&state_path).map_err(|e| e.to_string())?;
    if let Some(target) = snapshot.target.as_ref() {
        if !target.owner.process.alive() {
            store::clear_ticket(&state_path, &target.ticket).map_err(|e| e.to_string())?;
            snapshot.target = None;
        }
    }
    if !allowed(app) {
        if let Some(target) = snapshot
            .target
            .as_ref()
            .filter(|target| target.owner == owner)
        {
            store::clear_ticket(&state_path, &target.ticket).map_err(|e| e.to_string())?;
        }
        return release_shortcut(app, &owner);
    }
    if let Some(target) = snapshot
        .target
        .as_ref()
        .filter(|target| target.owner == owner)
    {
        if !enabled.contains(&target.window_label) {
            store::clear_ticket(&state_path, &target.ticket).map_err(|e| e.to_string())?;
            snapshot.target = None;
        } else if store::targets_window(target, &owner, true) {
            if let Some(window) = app.get_webview_window(&target.window_label) {
                // A user may already have restored this popup through the Dock.
                if window.is_minimized().map_err(|e| e.to_string())? {
                    restore_window(&window)?;
                }
            }
            store::clear_ticket(&state_path, &target.ticket).map_err(|e| e.to_string())?;
            snapshot.target = None;
        }
    }
    if snapshot.target.is_none() {
        return release_shortcut(app, &owner);
    }
    if owns {
        return Ok(());
    }
    // The OS grants the key to one process. Any native owner can route the exact
    // target; a later sync takes ownership after that process closes.
    if app
        .global_shortcut()
        .on_shortcut(RESTORE_KEY, |app, _, event| {
            if event.state != ShortcutState::Pressed || !allowed(app) {
                return;
            }
            let result =
                path().and_then(|path| store::request_latest(&path).map_err(|e| e.to_string()));
            match result {
                Ok(Some(_)) => {
                    let handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = synchronize(&handle).await {
                            log::warn!("恢复指定弹窗失败: {error}");
                        }
                    });
                }
                Ok(None) => {}
                Err(error) => log::warn!("读取待恢复弹窗失败: {error}"),
            }
        })
        .is_ok()
    {
        app.state::<PopupRestoreState>()
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .owns_shortcut = true;
        store::edit(&state_path, |state| state.shortcut_owner = Some(owner))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Reuses the existing native mute tick, which runs while WebViews are minimized.
pub async fn synchronize(app: &AppHandle) -> Result<(), String> {
    let active = app.try_state::<PopupRestoreState>().is_some_and(|state| {
        state
            .0
            .lock()
            .map(|local| !local.enabled.is_empty() || local.owns_shortcut)
            .unwrap_or(false)
    });
    if !active {
        return Ok(());
    }
    on_main(app.clone(), synchronize_main).await
}

#[tauri::command]
pub async fn set_popup_restore_enabled(
    window: WebviewWindow,
    enabled: bool,
) -> Result<bool, String> {
    let app = window.app_handle().clone();
    on_main(app, move |app| {
        let owner = local_owner(app)?;
        let label = window.label().to_string();
        let enabled = enabled && allowed(app);
        let observe = {
            let state = app.state::<PopupRestoreState>();
            let mut local = state.0.lock().map_err(|e| e.to_string())?;
            if enabled {
                local.enabled.insert(label.clone());
            } else {
                local.enabled.remove(&label);
            }
            local.observed.insert(label.clone())
        };
        if !enabled {
            store::clear_window(&path()?, &owner, &label).map_err(|e| e.to_string())?;
        }
        if observe {
            let handle = app.clone();
            window.on_window_event(move |event| {
                // Tao may deliver an earlier focus event after our minimize
                // task. Only current native state can confirm a Dock restore.
                let restored = matches!(event, WindowEvent::Focused(true))
                    && handle.get_webview_window(&label).is_some_and(|window| {
                        window.is_minimized().ok() == Some(false)
                            && window.is_focused().ok() == Some(true)
                    });
                if restored || matches!(event, WindowEvent::Destroyed) {
                    if let Ok(state_path) = path() {
                        let _ = store::clear_window(&state_path, &owner, &label);
                    }
                }
                if matches!(event, WindowEvent::Destroyed) {
                    if let Some(state) = handle.try_state::<PopupRestoreState>() {
                        if let Ok(mut local) = state.0.lock() {
                            local.enabled.remove(&label);
                            local.observed.remove(&label);
                        }
                    }
                }
            });
        }
        synchronize_main(app)?;
        Ok(enabled)
    })
    .await
}

#[tauri::command]
pub async fn minimize_popup_window(window: WebviewWindow) -> Result<(), String> {
    let app = window.app_handle().clone();
    on_main(app, move |app| {
        let enabled = app
            .state::<PopupRestoreState>()
            .0
            .lock()
            .map_err(|e| e.to_string())?
            .enabled
            .contains(window.label());
        if !enabled || !allowed(app) {
            return Err("当前弹窗快捷键未启用".into());
        }
        if window.is_minimized().map_err(|e| e.to_string())? {
            return Ok(());
        }
        let target = Target {
            owner: local_owner(app)?,
            window_label: window.label().into(),
            ticket: uuid::Uuid::new_v4().to_string(),
            requested: false,
            minimized: false,
        };
        let state_path = path()?;
        let previous = store::edit(&state_path, |state| state.target.replace(target.clone()))
            .map_err(|e| e.to_string())?;
        let result = (|| {
            synchronize_main(app)?;
            let ready = store::read(&state_path)
                .map_err(|e| e.to_string())?
                .shortcut_owner
                .is_some_and(|owner| owner.process.alive());
            if !ready {
                return Err("Shift+Tab 被旧版弹窗或其它应用占用，请关闭旧版弹窗后重试".into());
            }
            window.minimize().map_err(|e| e.to_string())?;
            // Keep an early Shift+Tab queued until the native minimize finishes.
            // Completion for an older window cannot arm a newer target.
            if let Err(error) = store::mark_minimized(&state_path, &target.ticket) {
                let _ = restore_window(&window);
                return Err(error.to_string());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = store::edit(&state_path, |state| {
                if state
                    .target
                    .as_ref()
                    .is_some_and(|current| current.ticket == target.ticket)
                {
                    state.target = previous;
                }
            });
            let _ = synchronize_main(app);
        }
        result
    })
    .await
}
