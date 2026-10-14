//! A supervised popup can be recreated without completing its pending request.
use tauri::WebviewWindow;
use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
    },
    ProcessFailedEventHandler,
};

pub const RECOVERY_EXIT_CODE: i32 = 75;

/// Controllers sharing a profile must not race to recreate a crashed browser.
/// The parent holds this only until the frontend acknowledges readiness.
pub async fn startup_lock() -> Result<std::fs::File, String> {
    tauri::async_runtime::spawn_blocking(|| {
        use std::hash::{Hash, Hasher};
        let mut key = std::collections::hash_map::DefaultHasher::new();
        std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").hash(&mut key);
        let path = std::env::temp_dir().join(format!("iterate-webview-startup-{:x}.lock", key.finish()));
        let file = std::fs::OpenOptions::new().create(true).read(true).write(true)
            .open(path).map_err(|e| e.to_string())?;
        file.lock().map_err(|e| e.to_string())?;
        Ok(file)
    }).await.map_err(|e| e.to_string())?
}

pub fn install(window: &WebviewWindow) {
    let label = window.label().to_owned();
    let supervised = std::env::var_os("ITERATE_WEBVIEW_SUPERVISED").is_some();
    if let Err(error) = window.with_webview(move |webview| unsafe {
        let result = (|| {
            let core = webview.controller().CoreWebView2()?;
            let mut token = 0;
            core.add_ProcessFailed(
                &ProcessFailedEventHandler::create(Box::new(move |_, args| {
                    let Some(args) = args else { return Ok(()) };
                    let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED;
                    args.ProcessFailedKind(&mut kind)?;
                    log::error!("[WebViewRecovery] pid={} window={} kind={:?} supervised={}",
                        std::process::id(), label, kind, supervised);
                    if supervised && (kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED
                        || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED)
                    {
                        // The parent retains the request/response files and owns retries.
                        // Do not invoke the user-close path, which completes the request.
                        // The Windows event-loop shutdown returned status 0 in
                        // the real browser-exit test. Use an unambiguous process
                        // status so the parent cannot mistake failure for X/close.
                        std::process::exit(RECOVERY_EXIT_CODE);
                    }
                    Ok(())
                })),
                &mut token,
            )
        })();
        if let Err(error) = result {
            log::error!("[WebViewRecovery] cannot install handler: {error}");
        }
    }) {
        log::error!("[WebViewRecovery] cannot access webview: {error}");
    }
}
