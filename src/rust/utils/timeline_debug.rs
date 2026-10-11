use crate::utils::logger::{append_capped_log_line, DEBUG_LOG_MAX_BYTES};
use chrono::{Local, SecondsFormat};
use serde_json::Value;
use std::path::PathBuf;

fn timeline_debug_log_path() -> PathBuf {
    if let Some(home_dir) = dirs::home_dir() {
        return home_dir
            .join("Library")
            .join("Logs")
            .join("iterate")
            .join("timeline-debug.log");
    }

    std::env::temp_dir()
        .join("iterate")
        .join("timeline-debug.log")
}

pub fn append_timeline_debug_log(location: &str, key_params: Value) {
    let timestamp = Local::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let payload_text = serde_json::to_string(&key_params)
        .unwrap_or_else(|_| "{\"serialize_error\":true}".to_string());
    let line = format!("{} | {} | {}\n", timestamp, location, payload_text);
    let _ = append_capped_log_line(&timeline_debug_log_path(), line.as_bytes(), DEBUG_LOG_MAX_BYTES);
}
