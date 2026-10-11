//! Installation evidence from the running main WebView, never from a listening Bridge.
use serde_json::json;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn build_info() -> serde_json::Value {
    json!({"schema_version": 1, "production": !tauri::is_dev(), "ui_readiness_schema": 1})
}

#[tauri::command]
pub fn mark_desktop_ui_ready(window: tauri::WebviewWindow, surface: String) -> Result<(), String> {
    if tauri::is_dev() || window.label() != "main" {
        return Ok(());
    }
    if !matches!(surface.as_str(), "application" | "activation") {
        return Err("invalid desktop surface".into());
    }
    let url = window.url().map_err(|e| e.to_string())?;
    if !((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (url.scheme() == "https" && url.host_str() == Some("tauri.localhost")))
    {
        return Err("desktop UI did not load from the production origin".into());
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let bytes = fs::read(&executable).map_err(|e| e.to_string())?;
    let sha256 = hex::encode(ring::digest::digest(&ring::digest::SHA256, &bytes));
    let args: Vec<_> = std::env::args().collect();
    let popup = std::env::var_os("ITERATE_STANDALONE_MODE").is_some()
        || std::env::var_os("ITERATE_MCP_REQUEST_FILE").is_some()
        || args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--mcp-request" | "--ui"));
    let pid = std::process::id();
    let directory = dirs::home_dir()
        .ok_or("home directory unavailable")?
        .join(".cunzhi/desktop-health");
    fs::create_dir_all(&directory).map_err(|e| e.to_string())?;
    let receipt = json!({
        "schema_version": 1, "pid": pid, "production": true,
        "role": if popup { "mcp-popup" } else { "canonical-gui" },
        "window": "main", "surface": surface, "ready": true,
        "origin": format!("{}://{}", url.scheme(), url.host_str().unwrap_or_default()),
        "executable": executable, "executable_sha256": sha256,
        "ready_at_ms": SystemTime::now().duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?.as_millis() as u64
    });
    let temporary = directory.join(format!("{pid}.tmp"));
    fs::write(
        &temporary,
        serde_json::to_vec(&receipt).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    fs::rename(temporary, directory.join(format!("{pid}.json"))).map_err(|e| e.to_string())
}
