use super::apns_config::parse_apns_environment;
use super::apns_live_activity::{
    is_apns_live_activity_token_stale, live_activity_info_matches, ApnsLiveActivityInfo,
    APNS_LIVE_ACTIVITY_TOKEN_STALE_HOURS,
};
use super::apns_notification::{is_apns_token_stale, ApnsDeviceInfo, APNS_TOKEN_STALE_DAYS};
use crate::log_important;
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

static APNS_DEVICE_TOKENS: Lazy<Arc<RwLock<HashMap<String, ApnsDeviceInfo>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static APNS_LIVE_ACTIVITY_TOKENS: Lazy<Arc<RwLock<HashMap<String, ApnsLiveActivityInfo>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ApnsNotificationPreferenceUpdate {
    Updated,
    TokenNotFound,
    DeviceMismatch,
}

fn canonical_stored_environment(value: &str, default_environment: &str) -> String {
    parse_apns_environment(value)
        .map(|environment| environment.as_str().to_string())
        .unwrap_or_else(|_| default_environment.to_string())
}

fn migrate_device_environment(info: &mut ApnsDeviceInfo, default_environment: &str) -> bool {
    let canonical = canonical_stored_environment(&info.environment, default_environment);
    if info.environment == canonical {
        return false;
    }
    info.environment = canonical;
    true
}

fn migrate_live_activity_environment(
    info: &mut ApnsLiveActivityInfo,
    default_environment: &str,
) -> bool {
    let canonical = canonical_stored_environment(&info.environment, default_environment);
    if info.environment == canonical {
        return false;
    }
    info.environment = canonical;
    true
}

fn rotated_device_tokens(
    tokens: &HashMap<String, ApnsDeviceInfo>,
    device_token: &str,
    device_id: &str,
    environment: &str,
) -> Vec<String> {
    tokens
        .iter()
        .filter_map(|(token, info)| {
            (info.device_id == device_id
                && info.environment == environment
                && token != device_token)
                .then(|| token.clone())
        })
        .collect()
}

fn rotated_live_activity_tokens(
    tokens: &HashMap<String, ApnsLiveActivityInfo>,
    activity_token: &str,
    activity_kind: &str,
    activity_key: &str,
    device_id: &str,
    environment: &str,
) -> Vec<String> {
    tokens
        .iter()
        .filter_map(|(token, existing)| {
            (live_activity_info_matches(existing, activity_kind, activity_key)
                && existing.device_id == device_id
                && existing.environment == environment
                && token != activity_token)
                .then(|| token.clone())
        })
        .collect()
}

pub(super) async fn apns_device_token_count() -> usize {
    APNS_DEVICE_TOKENS.read().await.len()
}

pub(super) async fn apns_device_tokens_snapshot() -> HashMap<String, ApnsDeviceInfo> {
    let mut snapshot = APNS_DEVICE_TOKENS.read().await.clone();
    let stale_tokens: Vec<String> = snapshot
        .iter()
        .filter_map(|(token, info)| is_apns_token_stale(info).then(|| token.clone()))
        .collect();
    if stale_tokens.is_empty() {
        return snapshot;
    }

    for token in &stale_tokens {
        snapshot.remove(token);
    }

    let mut tokens = APNS_DEVICE_TOKENS.write().await;
    for token in &stale_tokens {
        tokens.remove(token);
    }
    if let Err(err) = save_apns_tokens_to_file(&tokens).await {
        log_important!(warn, "[APNs] 清理过期 token 保存失败: {}", err);
    }
    log::info!(
        "[APNs] 已清理 {} 个超过 {} 天未活跃的 token",
        stale_tokens.len(),
        APNS_TOKEN_STALE_DAYS
    );

    snapshot
}

pub(super) async fn remove_apns_device_tokens(
    invalid_tokens: &[String],
) -> Result<(), std::io::Error> {
    if invalid_tokens.is_empty() {
        return Ok(());
    }

    let mut tokens = APNS_DEVICE_TOKENS.write().await;
    for token in invalid_tokens {
        tokens.remove(token);
    }
    save_apns_tokens_to_file(&tokens).await
}

pub(super) async fn register_apns_device_token(
    device_token: String,
    device_info: ApnsDeviceInfo,
    normalized_device_id: &str,
) -> Result<(), std::io::Error> {
    let mut tokens = APNS_DEVICE_TOKENS.write().await;
    if !normalized_device_id.is_empty() {
        let rotated_tokens = rotated_device_tokens(
            &tokens,
            &device_token,
            normalized_device_id,
            &device_info.environment,
        );
        for token in rotated_tokens {
            tokens.remove(&token);
        }
    }
    tokens.insert(device_token, device_info);
    save_apns_tokens_to_file(&tokens).await
}

fn apply_apns_notification_preference(
    tokens: &mut HashMap<String, ApnsDeviceInfo>,
    device_token: &str,
    normalized_device_id: &str,
    notifications_enabled: bool,
    last_seen_at: &str,
) -> ApnsNotificationPreferenceUpdate {
    let Some(device_info) = tokens.get_mut(device_token) else {
        return ApnsNotificationPreferenceUpdate::TokenNotFound;
    };
    if !normalized_device_id.is_empty() && device_info.device_id != normalized_device_id {
        return ApnsNotificationPreferenceUpdate::DeviceMismatch;
    }

    device_info.notifications_enabled = notifications_enabled;
    device_info.last_seen_at = last_seen_at.to_string();
    ApnsNotificationPreferenceUpdate::Updated
}

pub(super) async fn update_apns_device_notification_preference(
    device_token: &str,
    normalized_device_id: &str,
    notifications_enabled: bool,
    last_seen_at: &str,
) -> Result<ApnsNotificationPreferenceUpdate, std::io::Error> {
    let mut tokens = APNS_DEVICE_TOKENS.write().await;
    let outcome = apply_apns_notification_preference(
        &mut tokens,
        device_token,
        normalized_device_id,
        notifications_enabled,
        last_seen_at,
    );
    if outcome == ApnsNotificationPreferenceUpdate::Updated {
        save_apns_tokens_to_file(&tokens).await?;
    }
    Ok(outcome)
}

pub(super) async fn apns_live_activity_tokens_snapshot() -> HashMap<String, ApnsLiveActivityInfo> {
    let mut snapshot = APNS_LIVE_ACTIVITY_TOKENS.read().await.clone();
    let stale_tokens: Vec<String> = snapshot
        .iter()
        .filter_map(|(token, info)| is_apns_live_activity_token_stale(info).then(|| token.clone()))
        .collect();

    if stale_tokens.is_empty() {
        return snapshot;
    }

    for token in &stale_tokens {
        snapshot.remove(token);
    }

    let mut tokens = APNS_LIVE_ACTIVITY_TOKENS.write().await;
    for token in &stale_tokens {
        tokens.remove(token);
    }
    if let Err(err) = save_apns_live_activity_tokens_to_file(&tokens).await {
        log_important!(warn, "[APNs LiveActivity] 清理过期 token 保存失败: {}", err);
    }
    log::info!(
        "[APNs LiveActivity] 已清理 {} 个超过 {} 小时未活跃的 activity token",
        stale_tokens.len(),
        APNS_LIVE_ACTIVITY_TOKEN_STALE_HOURS
    );

    snapshot
}

pub(super) async fn remove_apns_live_activity_tokens(
    invalid_tokens: &[String],
) -> Result<(), std::io::Error> {
    if invalid_tokens.is_empty() {
        return Ok(());
    }

    let mut tokens = APNS_LIVE_ACTIVITY_TOKENS.write().await;
    for token in invalid_tokens {
        tokens.remove(token);
    }
    save_apns_live_activity_tokens_to_file(&tokens).await
}

pub(super) async fn register_apns_live_activity_token(
    activity_token: String,
    info: ApnsLiveActivityInfo,
    activity_kind: &str,
    activity_key: &str,
    device_id: &str,
) -> (usize, Result<(), std::io::Error>) {
    let mut tokens = APNS_LIVE_ACTIVITY_TOKENS.write().await;
    let environment = info.environment.clone();
    let rotated_tokens = rotated_live_activity_tokens(
        &tokens,
        &activity_token,
        activity_kind,
        activity_key,
        device_id,
        &environment,
    );
    for token in rotated_tokens {
        tokens.remove(&token);
    }
    tokens.insert(activity_token, info);
    let token_count = tokens.len();
    let result = save_apns_live_activity_tokens_to_file(&tokens).await;
    (token_count, result)
}

async fn save_apns_tokens_to_file(
    tokens: &HashMap<String, ApnsDeviceInfo>,
) -> Result<(), std::io::Error> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::Path::new(&home).join(".cunzhi");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("apns_tokens.json");
    let content = serde_json::to_string_pretty(tokens).unwrap_or_default();
    std::fs::write(path, content)
}

async fn save_apns_live_activity_tokens_to_file(
    tokens: &HashMap<String, ApnsLiveActivityInfo>,
) -> Result<(), std::io::Error> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::Path::new(&home).join(".cunzhi");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("apns_live_activity_tokens.json");
    let content = serde_json::to_string_pretty(tokens).unwrap_or_default();
    std::fs::write(path, content)
}

fn load_apns_tokens_from_file() -> HashMap<String, ApnsDeviceInfo> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::Path::new(&home)
        .join(".cunzhi")
        .join("apns_tokens.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        HashMap::new()
    }
}

fn load_apns_live_activity_tokens_from_file() -> HashMap<String, ApnsLiveActivityInfo> {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let path = std::path::Path::new(&home)
        .join(".cunzhi")
        .join("apns_live_activity_tokens.json");
    if let Ok(content) = std::fs::read_to_string(&path) {
        serde_json::from_str(&content).unwrap_or_default()
    } else {
        HashMap::new()
    }
}

pub(super) async fn init_apns_tokens(default_environment: &str) {
    let mut saved_tokens = load_apns_tokens_from_file();
    let original_len = saved_tokens.len();
    let migrated_count: usize = saved_tokens
        .values_mut()
        .map(|info| usize::from(migrate_device_environment(info, default_environment)))
        .sum();
    saved_tokens.retain(|_, info| !is_apns_token_stale(info));
    if !saved_tokens.is_empty() {
        let mut tokens = APNS_DEVICE_TOKENS.write().await;
        *tokens = saved_tokens;
        log::info!(
            "[APNs] 已加载 {} 个设备 Token（清理过期 {} 个）",
            tokens.len(),
            original_len.saturating_sub(tokens.len())
        );
        if original_len != tokens.len() || migrated_count > 0 {
            if let Err(err) = save_apns_tokens_to_file(&tokens).await {
                log_important!(warn, "[APNs] 持久化过期 token 清理结果失败: {}", err);
            }
        }
    } else if original_len > 0 {
        if let Err(err) = save_apns_tokens_to_file(&saved_tokens).await {
            log_important!(warn, "[APNs] 清空过期 token 文件失败: {}", err);
        }
        log::info!(
            "[APNs] 已清理全部过期 token（{} 个），当前无可用设备",
            original_len
        );
    }

    init_apns_live_activity_tokens(default_environment).await;
}

async fn init_apns_live_activity_tokens(default_environment: &str) {
    let mut saved_tokens = load_apns_live_activity_tokens_from_file();
    let original_len = saved_tokens.len();
    let migrated_count: usize = saved_tokens
        .values_mut()
        .map(|info| usize::from(migrate_live_activity_environment(info, default_environment)))
        .sum();
    saved_tokens.retain(|_, info| !is_apns_live_activity_token_stale(info));
    if !saved_tokens.is_empty() {
        let mut tokens = APNS_LIVE_ACTIVITY_TOKENS.write().await;
        *tokens = saved_tokens;
        log::info!(
            "[APNs LiveActivity] 已加载 {} 个 activity token（清理过期 {} 个）",
            tokens.len(),
            original_len.saturating_sub(tokens.len())
        );
        if original_len != tokens.len() || migrated_count > 0 {
            if let Err(err) = save_apns_live_activity_tokens_to_file(&tokens).await {
                log_important!(
                    warn,
                    "[APNs LiveActivity] 持久化过期 token 清理结果失败: {}",
                    err
                );
            }
        }
    } else if original_len > 0 {
        if let Err(err) = save_apns_live_activity_tokens_to_file(&saved_tokens).await {
            log_important!(warn, "[APNs LiveActivity] 清空过期 token 文件失败: {}", err);
        }
        log::info!(
            "[APNs LiveActivity] 已清理全部过期 activity token（{} 个）",
            original_len
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_info(device_id: &str, environment: &str) -> ApnsDeviceInfo {
        ApnsDeviceInfo {
            authorization_generation: String::new(),
            device_token: format!("{device_id}-{environment}"),
            platform: "ios".to_string(),
            app_version: "1.0".to_string(),
            device_id: device_id.to_string(),
            registered_at: "2026-08-10T00:00:00Z".to_string(),
            last_seen_at: "2026-08-10T00:00:00Z".to_string(),
            notifications_enabled: true,
            environment: environment.to_string(),
        }
    }

    #[test]
    fn same_device_keeps_sandbox_and_production_tokens() {
        let tokens = HashMap::from([
            (
                "sandbox-old".to_string(),
                device_info("device-a", "sandbox"),
            ),
            (
                "production-old".to_string(),
                device_info("device-a", "production"),
            ),
        ]);

        assert_eq!(
            rotated_device_tokens(&tokens, "sandbox-new", "device-a", "sandbox"),
            vec!["sandbox-old".to_string()]
        );
    }

    #[test]
    fn legacy_device_environment_is_migrated_to_server_default() {
        let mut info = device_info("device-a", "");
        assert!(migrate_device_environment(&mut info, "sandbox"));
        assert_eq!(info.environment, "sandbox");
        assert!(!migrate_device_environment(&mut info, "production"));
        assert_eq!(info.environment, "sandbox");
    }

    #[test]
    fn notification_preference_update_preserves_production_environment_and_registration() {
        let mut tokens = HashMap::from([(
            "production-token".to_string(),
            device_info("device-a", "production"),
        )]);

        let outcome = apply_apns_notification_preference(
            &mut tokens,
            "production-token",
            "device-a",
            false,
            "2026-08-23T00:00:00Z",
        );

        assert_eq!(outcome, ApnsNotificationPreferenceUpdate::Updated);
        let updated = tokens.get("production-token").expect("updated token");
        assert_eq!(updated.environment, "production");
        assert_eq!(updated.registered_at, "2026-08-10T00:00:00Z");
        assert_eq!(updated.last_seen_at, "2026-08-23T00:00:00Z");
        assert!(!updated.notifications_enabled);
    }

    #[test]
    fn notification_preference_update_rejects_unknown_or_mismatched_device() {
        let mut tokens = HashMap::from([(
            "production-token".to_string(),
            device_info("device-a", "production"),
        )]);

        assert_eq!(
            apply_apns_notification_preference(
                &mut tokens,
                "missing-token",
                "device-a",
                false,
                "2026-08-23T00:00:00Z",
            ),
            ApnsNotificationPreferenceUpdate::TokenNotFound
        );
        assert_eq!(
            apply_apns_notification_preference(
                &mut tokens,
                "production-token",
                "device-b",
                false,
                "2026-08-23T00:00:00Z",
            ),
            ApnsNotificationPreferenceUpdate::DeviceMismatch
        );
        assert_eq!(tokens["production-token"].environment, "production");
        assert!(tokens["production-token"].notifications_enabled);
    }

    fn live_info(token: &str, environment: &str) -> ApnsLiveActivityInfo {
        ApnsLiveActivityInfo {
            authorization_generation: String::new(),
            activity_token: token.to_string(),
            goal_id: "goal-a".to_string(),
            activity_kind: "live_goal".to_string(),
            activity_key: Some("goal-a".to_string()),
            activity_id: None,
            device_id: "device-a".to_string(),
            platform: "ios".to_string(),
            app_version: "1.0".to_string(),
            project_path: None,
            request_id: None,
            registered_at: "2026-08-10T00:00:00Z".to_string(),
            last_seen_at: "2026-08-10T00:00:00Z".to_string(),
            environment: environment.to_string(),
        }
    }

    #[test]
    fn live_activity_rotation_is_scoped_to_environment() {
        let tokens = HashMap::from([
            (
                "sandbox-old".to_string(),
                live_info("sandbox-old", "sandbox"),
            ),
            (
                "production-old".to_string(),
                live_info("production-old", "production"),
            ),
        ]);

        assert_eq!(
            rotated_live_activity_tokens(
                &tokens,
                "production-new",
                "live_goal",
                "goal-a",
                "device-a",
                "production",
            ),
            vec!["production-old".to_string()]
        );
    }
}
