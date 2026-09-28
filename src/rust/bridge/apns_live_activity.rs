use serde::{Deserialize, Serialize};

pub(super) const APNS_LIVE_ACTIVITY_TOKEN_STALE_HOURS: i64 = 36;
pub(super) const LIVE_ACTIVITY_KIND_LIVE_GOAL: &str = "live_goal";
pub(super) const LIVE_ACTIVITY_KIND_QUOTA: &str = "quota";
pub(super) const QUOTA_LIVE_ACTIVITY_KEY: &str = "codex_quota";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ApnsLiveActivityInfo {
    #[serde(skip)]
    pub(super) authorization_generation: String,
    pub(super) activity_token: String,
    pub(super) goal_id: String,
    #[serde(default = "default_live_activity_kind")]
    pub(super) activity_kind: String,
    #[serde(default)]
    pub(super) activity_key: Option<String>,
    #[serde(default)]
    pub(super) activity_id: Option<String>,
    #[serde(default = "default_apns_device_id")]
    pub(super) device_id: String,
    #[serde(default)]
    pub(super) platform: String,
    #[serde(default)]
    pub(super) app_version: String,
    #[serde(default)]
    pub(super) project_path: Option<String>,
    #[serde(default)]
    pub(super) request_id: Option<String>,
    pub(super) registered_at: String,
    #[serde(default)]
    pub(super) last_seen_at: String,
    #[serde(default)]
    pub(super) environment: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct ApnsLiveActivityRegisterRequest {
    pub(super) activity_token: String,
    #[serde(default)]
    pub(super) goal_id: Option<String>,
    #[serde(default)]
    pub(super) activity_kind: Option<String>,
    #[serde(default)]
    pub(super) activity_key: Option<String>,
    #[serde(default)]
    pub(super) activity_id: Option<String>,
    #[serde(default)]
    pub(super) device_id: String,
    #[serde(default)]
    pub(super) platform: String,
    #[serde(default)]
    pub(super) app_version: String,
    #[serde(default)]
    pub(super) project_path: Option<String>,
    #[serde(default)]
    pub(super) request_id: Option<String>,
    #[serde(default)]
    pub(super) environment: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(super) struct ApnsLiveActivityUpdateRequest {
    #[serde(default)]
    pub(super) activity_token: Option<String>,
    #[serde(default)]
    pub(super) goal_id: Option<String>,
    #[serde(default)]
    pub(super) activity_kind: Option<String>,
    #[serde(default)]
    pub(super) activity_key: Option<String>,
    #[serde(default)]
    pub(super) event: Option<String>,
    #[serde(default)]
    pub(super) title: Option<String>,
    #[serde(default)]
    pub(super) status: Option<String>,
    #[serde(default)]
    pub(super) phase: Option<String>,
    #[serde(default)]
    pub(super) status_text: Option<String>,
    #[serde(default)]
    pub(super) progress_percent: Option<f64>,
    #[serde(default)]
    pub(super) progress_label: Option<String>,
    #[serde(default)]
    pub(super) requires_action: Option<bool>,
    #[serde(default)]
    pub(super) elapsed_ms: Option<i64>,
    #[serde(default)]
    pub(super) started_at_ms: Option<i64>,
    #[serde(default)]
    pub(super) updated_at_ms: Option<i64>,
    #[serde(default)]
    pub(super) project_path: Option<String>,
    #[serde(default)]
    pub(super) request_id: Option<String>,
    #[serde(default)]
    pub(super) content_state: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
pub(super) struct ApnsLiveActivitySendStats {
    pub(super) success: bool,
    pub(super) event: String,
    pub(super) matched: usize,
    pub(super) sent: usize,
    pub(super) failed: usize,
    pub(super) invalidated: usize,
    pub(super) message: String,
}

fn default_live_activity_kind() -> String {
    LIVE_ACTIVITY_KIND_LIVE_GOAL.to_string()
}

fn default_apns_device_id() -> String {
    "unknown".to_string()
}

fn apns_now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn parse_rfc3339_utc(value: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&chrono::Utc))
}

pub(super) fn normalized_live_activity_kind(kind: Option<&str>) -> String {
    match kind
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.replace('-', "_").to_ascii_lowercase())
        .as_deref()
    {
        Some(LIVE_ACTIVITY_KIND_QUOTA) => LIVE_ACTIVITY_KIND_QUOTA.to_string(),
        _ => LIVE_ACTIVITY_KIND_LIVE_GOAL.to_string(),
    }
}

pub(super) fn normalized_live_activity_key(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn live_activity_info_key(info: &ApnsLiveActivityInfo) -> String {
    info.activity_key
        .as_deref()
        .and_then(|value| normalized_live_activity_key(Some(value)))
        .unwrap_or_else(|| info.goal_id.clone())
}

pub(super) fn live_activity_info_kind(info: &ApnsLiveActivityInfo) -> String {
    normalized_live_activity_kind(Some(&info.activity_kind))
}

pub(super) fn live_activity_info_matches(
    info: &ApnsLiveActivityInfo,
    requested_kind: &str,
    requested_key: &str,
) -> bool {
    live_activity_info_kind(info) == requested_kind && live_activity_info_key(info) == requested_key
}

fn apns_live_activity_last_active_at(
    info: &ApnsLiveActivityInfo,
) -> Option<chrono::DateTime<chrono::Utc>> {
    if !info.last_seen_at.is_empty() {
        parse_rfc3339_utc(&info.last_seen_at)
    } else {
        parse_rfc3339_utc(&info.registered_at)
    }
}

pub(super) fn is_apns_live_activity_token_stale(info: &ApnsLiveActivityInfo) -> bool {
    let Some(last_active) = apns_live_activity_last_active_at(info) else {
        return true;
    };
    let cutoff = chrono::Utc::now() - chrono::Duration::hours(APNS_LIVE_ACTIVITY_TOKEN_STALE_HOURS);
    last_active < cutoff
}

pub(super) fn quota_live_activity_fingerprint_send_succeeded(
    stats: &ApnsLiveActivitySendStats,
) -> bool {
    stats.sent > 0
}

pub(super) fn quota_live_activity_content_state_from_snapshot(
    quota_snapshot: &serde_json::Value,
) -> Option<serde_json::Value> {
    let primary = quota_snapshot.get("primary")?;
    let status = quota_snapshot_string(quota_snapshot, "status", "status")
        .unwrap_or_else(|| "unknown".to_string());
    let status_label = quota_snapshot_string(quota_snapshot, "statusLabel", "status_label")
        .unwrap_or_else(|| "额度未知".to_string());
    let provider_name = quota_snapshot_string(primary, "providerName", "provider_name")
        .unwrap_or_else(|| "Codex".to_string());
    let provider_summary = quota_snapshot_string(primary, "providerSummary", "provider_summary");
    let account_label = quota_snapshot_string(primary, "accountLabel", "account_label");
    let primary_label =
        quota_snapshot_string(primary, "label", "label").unwrap_or_else(|| "Quota".to_string());
    let primary_remaining = quota_snapshot_u8(primary, "remaining", "remaining").unwrap_or(0);
    let updated_at_ms = quota_snapshot_i64(quota_snapshot, "updatedAtMs", "updated_at_ms")
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis());
    let stale_after_ms = quota_snapshot_i64(quota_snapshot, "staleAfterMs", "stale_after_ms")
        .unwrap_or(updated_at_ms);

    let mut content_state = serde_json::json!({
        "status": status,
        "statusLabel": status_label,
        "providerName": provider_name,
        "providerSummary": provider_summary,
        "accountLabel": account_label,
        "primaryLabel": primary_label,
        "primaryRemaining": primary_remaining,
        "resetLabel": quota_snapshot_string(primary, "resetLabel", "reset_label"),
        "resetAtMs": quota_snapshot_i64(primary, "resetAtMs", "reset_at_ms"),
        "updatedAtMs": updated_at_ms,
        "staleAfterMs": stale_after_ms,
    });

    if let Some(secondary) = quota_snapshot.get("secondary") {
        if let Some(secondary_label) = quota_snapshot_string(secondary, "label", "label") {
            if let Some(secondary_remaining) =
                quota_snapshot_u8(secondary, "remaining", "remaining")
            {
                if let Some(object) = content_state.as_object_mut() {
                    object.insert(
                        "secondaryLabel".to_string(),
                        serde_json::Value::String(secondary_label),
                    );
                    object.insert(
                        "secondaryRemaining".to_string(),
                        serde_json::json!(secondary_remaining),
                    );
                }
            }
        }
    }

    Some(content_state)
}

fn quota_snapshot_string(
    value: &serde_json::Value,
    camel_key: &str,
    snake_key: &str,
) -> Option<String> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn quota_snapshot_i64(
    value: &serde_json::Value,
    camel_key: &str,
    snake_key: &str,
) -> Option<i64> {
    value
        .get(camel_key)
        .or_else(|| value.get(snake_key))
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
                .or_else(|| value.as_f64().map(|number| number as i64))
                .or_else(|| {
                    value
                        .as_str()
                        .and_then(|text| text.trim().parse::<i64>().ok())
                })
        })
}

fn quota_snapshot_u8(value: &serde_json::Value, camel_key: &str, snake_key: &str) -> Option<u8> {
    quota_snapshot_i64(value, camel_key, snake_key).map(|value| value.clamp(0, 100) as u8)
}

pub(super) fn normalized_live_activity_event(event: Option<&str>) -> String {
    match event
        .map(str::trim)
        .unwrap_or("update")
        .to_ascii_lowercase()
        .as_str()
    {
        "end" => "end".to_string(),
        _ => "update".to_string(),
    }
}

fn normalized_live_activity_progress(value: Option<f64>, event: &str) -> f64 {
    value
        .filter(|progress| progress.is_finite())
        .map(|progress| progress.clamp(0.0, 100.0))
        .unwrap_or(if event == "end" { 100.0 } else { 0.0 })
}

pub(super) fn trimmed_live_activity_string(value: Option<&String>) -> Option<String> {
    value
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(super) fn live_activity_content_state_from_update(
    request: &ApnsLiveActivityUpdateRequest,
    event: &str,
    activity_kind: &str,
) -> serde_json::Value {
    if activity_kind == LIVE_ACTIVITY_KIND_QUOTA {
        if let Some(serde_json::Value::Object(content_state)) = request.content_state.as_ref() {
            return serde_json::Value::Object(content_state.clone());
        }
    }

    let now_ms = chrono::Utc::now().timestamp_millis();
    let elapsed_ms = request.elapsed_ms.unwrap_or(0).max(0);
    let started_at_ms = request
        .started_at_ms
        .unwrap_or_else(|| now_ms.saturating_sub(elapsed_ms));
    let progress_percent = normalized_live_activity_progress(request.progress_percent, event);
    let status = trimmed_live_activity_string(request.status.as_ref()).unwrap_or_else(|| {
        if event == "end" {
            "completed".to_string()
        } else {
            "running".to_string()
        }
    });
    let phase = trimmed_live_activity_string(request.phase.as_ref()).unwrap_or_else(|| {
        if event == "end" {
            "completed".to_string()
        } else {
            "running".to_string()
        }
    });
    let status_text =
        trimmed_live_activity_string(request.status_text.as_ref()).unwrap_or_else(|| {
            if event == "end" {
                "已完成".to_string()
            } else {
                "执行中".to_string()
            }
        });

    let mut content_state = serde_json::json!({
        "title": trimmed_live_activity_string(request.title.as_ref())
            .unwrap_or_else(|| "正在执行目标".to_string()),
        "status": status,
        "phase": phase,
        "statusText": status_text,
        "progressPercent": progress_percent,
        "progressLabel": request.progress_label.as_ref().and_then(|value| {
            let trimmed = value.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }),
        "requiresAction": request.requires_action.unwrap_or(false),
        "startedAtMs": started_at_ms,
        "elapsedMs": elapsed_ms,
        "updatedAtMs": request.updated_at_ms.unwrap_or(now_ms),
    });

    if let Some(serde_json::Value::Object(overrides)) = request.content_state.as_ref() {
        if let Some(base) = content_state.as_object_mut() {
            for (key, value) in overrides {
                base.insert(key.clone(), value.clone());
            }
        }
    }

    content_state
}

pub(super) fn direct_live_activity_info_from_request(
    activity_token: &str,
    request: &ApnsLiveActivityUpdateRequest,
) -> ApnsLiveActivityInfo {
    let now = apns_now_rfc3339();
    let activity_kind = normalized_live_activity_kind(request.activity_kind.as_deref());
    let activity_key =
        normalized_live_activity_key(request.activity_key.as_deref()).or_else(|| {
            request
                .goal_id
                .as_deref()
                .and_then(|goal_id| normalized_live_activity_key(Some(goal_id)))
        });
    let goal_id = request
        .goal_id
        .as_deref()
        .and_then(|goal_id| normalized_live_activity_key(Some(goal_id)))
        .or_else(|| activity_key.clone())
        .unwrap_or_else(|| "direct".to_string());
    ApnsLiveActivityInfo {
        authorization_generation: String::new(),
        activity_token: activity_token.to_string(),
        goal_id,
        activity_kind,
        activity_key,
        activity_id: None,
        device_id: "direct".to_string(),
        platform: "ios".to_string(),
        app_version: String::new(),
        project_path: request.project_path.clone(),
        request_id: request.request_id.clone(),
        registered_at: now.clone(),
        last_seen_at: now,
        environment: String::new(),
    }
}
