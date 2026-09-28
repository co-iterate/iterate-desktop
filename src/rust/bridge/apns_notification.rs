use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

pub(super) const APNS_TOKEN_STALE_DAYS: i64 = 30;
pub(super) const APNS_NOTIFICATION_DEDUPE_SECS: u64 = 30;
pub(super) const APNS_NOTIFICATION_REQUEST_DEDUPE_SECS: u64 = 6 * 60 * 60;
pub(super) const APNS_NOTIFICATION_EXPIRATION_SECS: i64 = 5 * 60;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ApnsDeviceInfo {
    #[serde(skip)]
    pub(super) authorization_generation: String,
    pub(super) device_token: String,
    pub(super) platform: String,
    pub(super) app_version: String,
    #[serde(default = "default_apns_device_id")]
    pub(super) device_id: String,
    pub(super) registered_at: String,
    #[serde(default)]
    pub(super) last_seen_at: String,
    #[serde(default = "default_apns_notifications_enabled")]
    pub(super) notifications_enabled: bool,
    #[serde(default)]
    pub(super) environment: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApnsNotifyRequest {
    pub(super) body: String,
    pub(super) title: Option<String>,
    pub(super) project_path: Option<String>,
    pub(super) request_id: Option<String>,
    #[serde(default)]
    pub(super) predefined_options: Vec<String>,
    #[serde(default = "default_true_bool")]
    pub(super) is_markdown: bool,
    #[serde(default)]
    pub(super) codex_thread_id: Option<String>,
    #[serde(default)]
    pub(super) codex_deeplink: Option<String>,
    #[serde(default)]
    pub(super) loop_active: bool,
    #[serde(default)]
    pub(super) force_popup: bool,
    pub(super) source: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApnsRegisterRequest {
    pub(super) device_token: String,
    #[serde(default)]
    pub(super) platform: String,
    #[serde(default)]
    pub(super) app_version: String,
    #[serde(default)]
    pub(super) device_id: String,
    #[serde(default)]
    pub(super) notifications_enabled: Option<bool>,
    #[serde(default)]
    pub(super) environment: Option<String>,
}

fn default_true_bool() -> bool {
    true
}

fn default_apns_notifications_enabled() -> bool {
    true
}

fn default_apns_device_id() -> String {
    "unknown".to_string()
}

pub(super) fn apns_now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn parse_rfc3339_utc(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

fn normalize_route_part(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn apns_last_active_at(device_info: &ApnsDeviceInfo) -> Option<chrono::DateTime<chrono::Utc>> {
    if !device_info.last_seen_at.is_empty() {
        parse_rfc3339_utc(&device_info.last_seen_at)
    } else {
        parse_rfc3339_utc(&device_info.registered_at)
    }
}

pub(super) fn is_apns_token_stale(device_info: &ApnsDeviceInfo) -> bool {
    let Some(last_active) = apns_last_active_at(device_info) else {
        return true;
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::days(APNS_TOKEN_STALE_DAYS);
    last_active < cutoff
}

pub(super) fn apns_dedupe_key(
    request_id: Option<&str>,
    project_path: Option<&str>,
    body: &str,
) -> String {
    if let Some(request_id) = normalize_route_part(request_id) {
        return format!("request:{}", request_id);
    }

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    body.hash(&mut hasher);
    format!(
        "fallback:{}:{}",
        project_path.unwrap_or("unknown"),
        hasher.finish()
    )
}

pub(super) fn apns_dedupe_ttl_secs(key: &str) -> u64 {
    if key.starts_with("request:") {
        APNS_NOTIFICATION_REQUEST_DEDUPE_SECS
    } else {
        APNS_NOTIFICATION_DEDUPE_SECS
    }
}

pub(super) fn apns_collapse_id(
    request_id: Option<&str>,
    project_path: Option<&str>,
    body: &str,
) -> String {
    let key = apns_dedupe_key(request_id, project_path, body);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    format!("iterate-{:x}", hasher.finish())
}
