use super::active_session::{
    build_active_session_summaries, build_active_session_summaries_with_focus,
    is_registered_mcp_port_request_id, lookup_active_session_entry, remove_active_session_entry,
    resolve_mcp_action_timeline_route_id, update_active_session_registry, ActiveSessionEntry,
};
#[cfg(test)]
use super::active_session::{
    is_inactive_session_message, lookup_active_session_payload, prune_active_session_registry,
};
use super::apns_config::{
    apns_endpoint, build_apns_bearer_token, configured_apns_environment, load_apns_config,
    resolve_apns_environment, ApnsConfig, ApnsEnvironment,
};
use super::apns_live_activity::{
    direct_live_activity_info_from_request, live_activity_content_state_from_update,
    live_activity_info_key, live_activity_info_kind, live_activity_info_matches,
    normalized_live_activity_event, normalized_live_activity_key, normalized_live_activity_kind,
    quota_live_activity_content_state_from_snapshot,
    quota_live_activity_fingerprint_send_succeeded, quota_snapshot_i64,
    trimmed_live_activity_string, ApnsLiveActivityInfo, ApnsLiveActivityRegisterRequest,
    ApnsLiveActivitySendStats, ApnsLiveActivityUpdateRequest, LIVE_ACTIVITY_KIND_LIVE_GOAL,
    LIVE_ACTIVITY_KIND_QUOTA, QUOTA_LIVE_ACTIVITY_KEY,
};
use super::apns_notification::{
    apns_collapse_id, apns_dedupe_key, apns_dedupe_ttl_secs, apns_now_rfc3339, ApnsDeviceInfo,
    ApnsNotifyRequest, ApnsRegisterRequest, APNS_NOTIFICATION_EXPIRATION_SECS,
};
#[cfg(test)]
use super::apns_notification::{
    APNS_NOTIFICATION_DEDUPE_SECS, APNS_NOTIFICATION_REQUEST_DEDUPE_SECS,
};
use super::apns_token_store::{
    apns_device_token_count, apns_device_tokens_snapshot, apns_live_activity_tokens_snapshot,
    init_apns_tokens, register_apns_device_token, register_apns_live_activity_token,
    remove_apns_device_tokens, remove_apns_live_activity_tokens,
    update_apns_device_notification_preference, ApnsNotificationPreferenceUpdate,
};
#[cfg(test)]
use super::file_list_guard::FILE_LIST_MAX_DEPTH;
use super::file_list_guard::{
    bounded_file_list_depth, canonical_file_list_roots, canonical_path_is_within_allowed_roots,
    file_list_browser_root_for_path, file_list_browser_roots_for_known_root,
    sanitize_created_directory_name,
};
#[cfg(test)]
use super::json_cache::CacheMetrics;
use super::json_cache::{
    mark_json_cache_keys, prune_json_cache, record_cache_write_count, CacheLookupRoute,
    MCP_ACTION_CACHE_METRICS, MCP_STATE_CACHE_METRICS,
};
use super::json_fields::{json_string_field, nested_metadata_string_field};
use super::markdown_images::{
    register_markdown_images_for_mcp_state_payload, registered_markdown_image_path,
};
#[cfg(test)]
use super::mcp_action_delivery::try_write_serve_response_file;
use super::mcp_action_handler::{try_handle_mcp_action_directly, try_handle_mcp_action_headless};
#[cfg(test)]
use super::mcp_action_payload::{
    build_goal_payload_parts, build_goal_submit_prompt, normalize_mcp_action_images,
    render_goal_submit_prompt,
};
use super::mcp_state_extract::{
    extract_conversation_id_from_mcp_state, extract_project_path_from_mcp_state,
    extract_request_id_from_mcp_state, extract_timeline_route_id_from_mcp_state,
};
#[cfg(test)]
use super::network_parse::is_tailscale_ipv4;
use super::network_parse::{
    is_valid_ipv4, parse_first_ipv4_line, parse_first_tailscale_ipv4_from_ifconfig,
};
use super::notification_payload::{
    bridge_payload_suppresses_remote_notification, extract_notification_body,
    trim_notification_body,
};
#[cfg(test)]
use super::phone_action::PHONE_ACTION_RESULT_TTL_SECS;
use super::phone_action::{
    attach_phone_action_job_metadata, build_phone_action_bridge_message,
    phone_action_job_is_expired, phone_action_job_payload_from_message,
    phone_action_job_payload_size, phone_action_result_entry_from_message,
    phone_action_target_device_id, prune_phone_action_jobs, prune_phone_action_results,
    PhoneActionJobEntry, PhoneActionJobResponse, PhoneActionResultQuery,
    PHONE_ACTION_INLINE_PAYLOAD_MAX_BYTES, PHONE_ACTION_JOB_PAYLOAD_MAX_BYTES,
    PHONE_ACTION_JOB_TTL_SECS,
};
pub use super::phone_action::{
    PhoneActionPublishResponse, PhoneActionRequest, PhoneActionResultEntry,
    PhoneActionResultResponse,
};
use super::promptor_library::read_promptor_library;
use super::public_control::{
    debug_header_value, has_bridge_auth_header, is_public_bridge_request, is_public_control_path,
    public_bridge_base_url, public_bridge_base_url_is_overridden, truncate_audit_value,
};
use super::public_probe_helpers::{
    format_http_status, http_url_to_ws_url, probe_error_summary, websocket_probe_auth_required,
    websocket_probe_ok_or_auth_required,
};
#[cfg(test)]
use super::room_submit::{
    cached_room_submit_outcome, clear_room_submit_outcome_cache_for_tests,
    remember_room_submit_outcome, RoomSubmitRequest,
};
use super::room_submit::{
    has_room_submit_metadata, payload_string_field, room_submit_outcome, RoomSubmitOutcome,
};
use super::room_submit_handler::handle_room_submit_action;
#[cfg(test)]
use super::route_debug::{build_route_debug_snapshot, reset_active_desktop_popup_route_for_tests};
use super::route_debug::{
    clear_active_desktop_popup_route, last_active_route, record_active_desktop_popup_route,
    record_last_active_route, record_last_completed_route, record_last_notification_route,
    route_debug_status_value,
};
use super::route_part::normalize_route_part;
use super::serve_request_fallback::load_live_serve_request_fallback;
use super::tailscale_diagnostics::{
    tailscale_dns_name, tailscale_funnel_config_matches,
    tailscale_host_from_public_bridge_base_url, tailscale_status_summary, TAILSCALE_FUNNEL_PORT,
};
use super::time_parse::parse_rfc3339;
use super::vapid_config::{load_vapid_config, VapidConfig};
use crate::conversation::{
    resolve_tree_route_key, ConversationManager, ConversationNode, NodeMetadata, NodeType,
};
use crate::log_important;
use crate::speech_memory;
use axum::{
    body::Bytes,
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        ConnectInfo, Path, Query, Request, State,
    },
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode, Uri},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, patch, post},
    Json, Router,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::Local;
use futures_util::{SinkExt, StreamExt};
use once_cell::sync::Lazy;
use ring::rand::SecureRandom;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::net::SocketAddr;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path as FilePath, PathBuf};
use std::process::Command;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower_http::cors::{AllowOrigin, CorsLayer};
use web_push_native::p256::ecdsa::{signature::Signer as _, Signature, SigningKey};
use web_push_native::{Auth as WebPushAuth, WebPushBuilder};

static APPLE_TOUCH_ICON_PNG: &[u8] = include_bytes!("../../../icons/icon-512.png");
static WEB_APP_MANIFEST: &str = "{\n  \"name\": \"iterate\",\n  \"short_name\": \"iterate\",\n  \"start_url\": \"/\",\n  \"scope\": \"/\",\n  \"display\": \"standalone\",\n  \"background_color\": \"#ffffff\",\n  \"theme_color\": \"#ffffff\",\n  \"icons\": [\n    {\n      \"src\": \"/apple-touch-icon.png\",\n      \"sizes\": \"512x512\",\n      \"type\": \"image/png\"\n    }\n  ]\n}\n";
static SERVICE_WORKER_JS: &str = include_str!("../../../sw.js");

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BridgeMessage {
    pub message_type: String,
    pub payload: serde_json::Value,
}

#[cfg(test)]
mod pairing_session_state_tests {
    use super::*;

    static TEST_LOCK: Lazy<tokio::sync::Mutex<()>> = Lazy::new(|| tokio::sync::Mutex::new(()));

    async fn reset_state() {
        MOBILE_PAIRING_TOKENS.write().await.clear();
        MOBILE_PAIRING_SESSIONS.write().await.clear();
        MOBILE_PAIRING_CLAIM_RECEIPTS.write().await.clear();
        WS_CLIENT_REGISTRY.write().await.clear();
    }

    #[tokio::test]
    async fn stopping_quick_tunnel_expires_only_bound_unclaimed_grants() {
        let _guard = TEST_LOCK.lock().await;
        reset_state().await;
        let now = chrono::Utc::now();
        let expires_at = (now + chrono::Duration::minutes(10)).to_rfc3339();
        let bound = PairingTokenInfo {
            session_id: "quick-session".to_string(),
            issued_at: now.to_rfc3339(),
            expires_at: expires_at.clone(),
            state: "pending".to_string(),
            failure_count: 0,
            first_failed_at: None,
            transport_mode: "cloudflare_tunnel".to_string(),
            formal_route_generation: None,
            endpoint_binding: Some(crate::tunnel::manager::QuickTunnelPairingBinding {
                endpoint: "https://example.trycloudflare.com".to_string(),
                install_identity: "install-test".to_string(),
                endpoint_epoch: 7,
            }),
        };
        let unbound = PairingTokenInfo {
            session_id: "local-session".to_string(),
            transport_mode: "lan_fallback".to_string(),
            endpoint_binding: None,
            ..bound.clone()
        };
        let fixed = PairingTokenInfo {
            session_id: "fixed-session".to_string(),
            transport_mode: "public_tunnel".to_string(),
            formal_route_generation: Some(3),
            endpoint_binding: Some(crate::tunnel::manager::QuickTunnelPairingBinding {
                endpoint: "https://iterate.example.com".to_string(),
                install_identity: "install-test".to_string(),
                endpoint_epoch: 7,
            }),
            ..bound.clone()
        };
        MOBILE_PAIRING_TOKENS
            .write()
            .await
            .insert("quick-token".to_string(), bound.clone());
        MOBILE_PAIRING_TOKENS
            .write()
            .await
            .insert("local-token".to_string(), unbound.clone());
        MOBILE_PAIRING_TOKENS
            .write()
            .await
            .insert("fixed-token".to_string(), fixed.clone());
        MOBILE_PAIRING_SESSIONS
            .write()
            .await
            .insert(bound.session_id.clone(), bound);
        MOBILE_PAIRING_SESSIONS
            .write()
            .await
            .insert(unbound.session_id.clone(), unbound);
        MOBILE_PAIRING_SESSIONS
            .write()
            .await
            .insert(fixed.session_id.clone(), fixed);

        invalidate_quick_tunnel_pairing_tokens().await;

        let tokens = MOBILE_PAIRING_TOKENS.read().await;
        assert!(!tokens.contains_key("quick-token"));
        assert!(tokens.contains_key("local-token"));
        assert!(tokens.contains_key("fixed-token"));
        drop(tokens);
        assert_eq!(
            MOBILE_PAIRING_SESSIONS
                .read()
                .await
                .get("quick-session")
                .map(|session| session.state.as_str()),
            Some("expired")
        );
        assert_eq!(
            MOBILE_PAIRING_SESSIONS
                .read()
                .await
                .get("fixed-session")
                .map(|session| session.state.as_str()),
            Some("pending")
        );
    }

    #[tokio::test]
    async fn issued_pairing_session_moves_from_pending_to_claimed() {
        let _guard = TEST_LOCK.lock().await;
        reset_state().await;

        let payload = build_mobile_pairing_payload(8080, false)
            .await
            .expect("test pairing payload");
        assert_eq!(payload.version, 2);
        assert!(!payload.pairing_session_id.is_empty());

        let pending = mobile_pairing_session_snapshot(&payload.pairing_session_id)
            .await
            .expect("pending session");
        assert_eq!(pending.state, "pending");

        let mut store = PairedDeviceStore::default();
        let response = claim_mobile_pairing_core(
            &payload.pairing_token,
            "ios-device-1",
            Some("Kexin iPhone"),
            Some("ios"),
            false,
            Some(&mut store),
            None,
        )
        .await
        .expect("claim succeeds");
        assert_eq!(response.pairing_session_id, payload.pairing_session_id);

        let claimed = mobile_pairing_session_snapshot(&payload.pairing_session_id)
            .await
            .expect("claimed session");
        assert_eq!(claimed.state, "claimed");
        assert_eq!(claimed.device_id.as_deref(), Some("ios-device-1"));
    }

    #[tokio::test]
    async fn replacement_issue_keeps_previous_unclaimed_qr_valid() {
        let _guard = TEST_LOCK.lock().await;
        reset_state().await;

        let first = build_mobile_pairing_payload(8080, false)
            .await
            .expect("first pairing payload");
        let second = build_mobile_pairing_payload(8080, false)
            .await
            .expect("replacement pairing payload");

        let tokens = MOBILE_PAIRING_TOKENS.read().await;
        assert!(tokens.contains_key(&first.pairing_token));
        assert!(tokens.contains_key(&second.pairing_token));
        assert_ne!(first.pairing_token, second.pairing_token);
        drop(tokens);

        let sessions = MOBILE_PAIRING_SESSIONS.read().await;
        assert_eq!(
            sessions
                .get(&first.pairing_session_id)
                .map(|session| session.state.as_str()),
            Some("pending")
        );
        assert_eq!(
            sessions
                .get(&second.pairing_session_id)
                .map(|session| session.state.as_str()),
            Some("pending")
        );
    }

    #[tokio::test]
    async fn claimed_pairing_token_is_idempotent_only_for_same_device() {
        let _guard = TEST_LOCK.lock().await;
        reset_state().await;

        let payload = build_mobile_pairing_payload(8080, false)
            .await
            .expect("test pairing payload");
        let mut store = PairedDeviceStore::default();
        claim_mobile_pairing_core(
            &payload.pairing_token,
            "ios-device-1",
            Some("iPhone"),
            Some("ios"),
            false,
            Some(&mut store),
            None,
        )
        .await
        .expect("first claim succeeds");

        let retry = claim_mobile_pairing_core(
            &payload.pairing_token,
            "ios-device-1",
            Some("iPhone"),
            Some("ios"),
            false,
            Some(&mut store),
            None,
        )
        .await
        .expect("same-device retry succeeds");
        assert_eq!(retry.pairing_session_id, payload.pairing_session_id);

        let replay = claim_mobile_pairing_core(
            &payload.pairing_token,
            "ios-device-2",
            Some("Other iPhone"),
            Some("ios"),
            false,
            Some(&mut store),
            None,
        )
        .await
        .expect_err("different-device replay is rejected");
        assert_eq!(replay, "pairing_token_already_claimed");
    }

    #[tokio::test]
    async fn only_authenticated_matching_ios_socket_completes_pairing() {
        let _guard = TEST_LOCK.lock().await;
        reset_state().await;

        let payload = build_mobile_pairing_payload(8080, false)
            .await
            .expect("test pairing payload");
        let mut store = PairedDeviceStore::default();
        claim_mobile_pairing_core(
            &payload.pairing_token,
            "ios-device-1",
            Some("iPhone"),
            Some("ios"),
            false,
            Some(&mut store),
            None,
        )
        .await
        .expect("claim succeeds");

        let now = chrono::Utc::now().to_rfc3339();
        let mut spoofed = WsClientInfo {
            client_id: "spoofed".to_string(),
            connected_at: now.clone(),
            last_seen_at: now.clone(),
            last_message_type: Some("client_hello".to_string()),
            remote_addr: None,
            host: String::new(),
            x_forwarded_for: String::new(),
            x_forwarded_proto: String::new(),
            cf_ray: String::new(),
            user_agent: String::new(),
            authenticated: false,
            authenticated_device_id: None,
            authenticated_client_kind: None,
            client_kind: "ios".to_string(),
            device_id: Some("ios-device-1".to_string()),
            selected_transport_mode: Some("public_tunnel".to_string()),
            selected_ws_url: None,
            project_path: None,
            request_id: None,
        };
        register_ws_client_after_upgrade(spoofed.clone()).await;
        let claimed = mobile_pairing_session_snapshot(&payload.pairing_session_id)
            .await
            .expect("claimed session");
        assert_eq!(claimed.state, "claimed");

        spoofed.client_id = "authenticated".to_string();
        spoofed.authenticated = true;
        spoofed.authenticated_device_id = Some("ios-device-1".to_string());
        spoofed.authenticated_client_kind = Some("ios".to_string());
        register_ws_client_after_upgrade(spoofed).await;

        let connected = mobile_pairing_session_snapshot(&payload.pairing_session_id)
            .await
            .expect("connected session");
        assert_eq!(connected.state, "connected");
        assert_eq!(
            connected.selected_transport_mode.as_deref(),
            Some("public_tunnel")
        );
    }
}

fn custom_prompts_value_for_mcp_state(app_handle: Option<&AppHandle>) -> Option<serde_json::Value> {
    if let Some(app_handle) = app_handle {
        if let Some(state) = app_handle.try_state::<crate::config::AppState>() {
            if let Ok(config) = state.config.lock() {
                return serde_json::to_value(&config.custom_prompt_config).ok();
            }
        }
    }

    match crate::config::storage::load_standalone_config() {
        Ok(config) => serde_json::to_value(&config.custom_prompt_config).ok(),
        Err(err) => {
            log::warn!(
                "[Bridge] load custom_prompt_config fallback failed for mcp_state: {}",
                err
            );
            None
        }
    }
}

fn ensure_custom_prompts_value_in_mcp_state(
    payload: &mut serde_json::Value,
    custom_prompts: serde_json::Value,
) {
    let Some(object) = payload.as_object_mut() else {
        return;
    };

    object.insert("customPrompts".to_string(), custom_prompts);
}

fn ensure_custom_prompts_in_mcp_state(
    app_handle: Option<&AppHandle>,
    payload: &mut serde_json::Value,
) {
    let needs_custom_prompts = payload
        .get("customPrompts")
        .map(|value| value.is_null())
        .unwrap_or(true);

    if !needs_custom_prompts {
        return;
    }

    let Some(custom_prompts) = custom_prompts_value_for_mcp_state(app_handle) else {
        return;
    };

    ensure_custom_prompts_value_in_mcp_state(payload, custom_prompts);
}

fn ensure_ghost_suggestions_in_mcp_state(payload: &mut serde_json::Value) {
    let needs_ghost_suggestions = payload
        .get("ghostSuggestions")
        .map(|value| value.is_null())
        .unwrap_or(true);

    if !needs_ghost_suggestions {
        return;
    }

    let Some(object) = payload.as_object_mut() else {
        return;
    };

    object.insert("ghostSuggestions".to_string(), read_ghost_suggestions());
}

const REQUEST_TIMELINE_SYNC_MESSAGE_TYPE: &str = "request_timeline_sync";
const TIMELINE_SYNC_SNAPSHOT_MESSAGE_TYPE: &str = "timeline_sync_snapshot";
const TIMELINE_SYNC_DELTA_MESSAGE_TYPE: &str = "timeline_sync_delta";
pub(super) const MCP_ACTION_CACHE_TTL_SECS: i64 = 30 * 60;
const MCP_STATE_CACHE_TTL_SECS: i64 = 6 * 60 * 60;
const MCP_ACTION_CACHE_MAX_ENTRIES: usize = 256;
const MCP_STATE_CACHE_MAX_ENTRIES: usize = 512;
const ROOT_TUNNEL_STATUS_FILE: &str = "/tmp/iterate-root-tunnel-status.json";
const ROOT_TUNNEL_METRICS_URL: &str = "http://127.0.0.1:60123/metrics";
const ROOT_TUNNEL_EXPECTED_HA_CONNECTIONS: f64 = 4.0;
const ROOT_TUNNEL_STATUS_MAX_AGE_SECS: i64 = 30;
const LOCAL_PROBE_TIMEOUT_SECS: u64 = 3;
const PUBLIC_PROBE_TIMEOUT_SECS: u64 = 8;
const LOCAL_WS_PROBE_TIMEOUT_SECS: u64 = 5;
const PUBLIC_WS_PROBE_TIMEOUT_SECS: u64 = 8;
const PUBLIC_HEALTH_RETRY_ATTEMPTS: usize = 3;
const PUBLIC_HEALTH_RETRY_DELAY_MS: u64 = 200;
const PUBLIC_HEALTH_SUCCESS_CACHE_SECS: u64 = 30;
const MOBILE_PAIRING_COMMAND_TIMEOUT_SECS: u64 = 2;
const MOBILE_PAIRING_CANDIDATES_TIMEOUT_SECS: u64 = 8;
const MOBILE_PAIRING_SESSION_RETENTION_SECS: i64 = 60 * 60;
const MOBILE_PAIRING_PERSIST_RETRY_GRACE_SECS: i64 = 30;
// 公网探针后台刷新间隔与缓存最大有效期：connection-status 读缓存秒回，
// 不再每次请求都同步阻塞跑公网探针。
const PUBLIC_PROBE_CACHE_REFRESH_SECS: u64 = 15;
const PUBLIC_PROBE_CACHE_MAX_AGE_SECS: u64 = 45;
const QUOTA_SNAPSHOT_REFRESH_ACTIVE_SECS: u64 = 60;
const QUOTA_SNAPSHOT_REFRESH_IDLE_SECS: u64 = 5 * 60;
const QUOTA_SNAPSHOT_REFRESH_START_DELAY_SECS: u64 = 5;
const QUOTA_SNAPSHOT_REFRESH_TRIGGER_COOLDOWN_SECS: u64 = 15;
const WS_CLIENT_REGISTRY_MAX_ENTRIES: usize = 32;

const SCOPE_STATUS_READ: &str = "status.read";
const SCOPE_SESSION_READ: &str = "session.read";
const SCOPE_SESSION_RESPOND: &str = "session.respond";
const SCOPE_WINDOW_SHOW: &str = "window.show";
const SCOPE_CONFIG_READ: &str = "config.read";
const SCOPE_CONFIG_WRITE: &str = "config.write";
const SCOPE_FILE_LIST: &str = "file.list";
const SCOPE_PAIRING_ISSUE: &str = "pairing.issue";
const SCOPE_GHOST_SUGGESTIONS_READ: &str = "ghost_suggestions.read";
const SCOPE_PROMPT_LIBRARY_READ: &str = "prompt_library.read";
const SCOPE_PROMPT_LIBRARY_WRITE: &str = "prompt_library.write";
const SCOPE_SPEECH_MEMORY_READ: &str = "speech_memory.read";
const SCOPE_SPEECH_MEMORY_WRITE: &str = "speech_memory.write";
const SCOPE_TUNNEL_RECOVER: &str = "tunnel.recover";
const SCOPE_SERVICE_RECOVER: &str = "service.recover";
const SCOPE_NOTIFICATION_SUBSCRIBE: &str = "notification.subscribe";
const SCOPE_NOTIFICATION_SEND: &str = "notification.send";
const SCOPE_BRIDGE_PUBLISH: &str = "bridge.publish";
const SCOPE_PHONE_ACTION_JOB_READ: &str = "phone_action.job.read";
const SCOPE_GHOST_SUGGESTIONS_WRITE: &str = "ghost_suggestions.write";
const WEB_LOGIN_PAIRING_TTL_SECS: i64 = 5 * 60;
const WEB_LOGIN_SESSION_TTL_SECS: i64 = 60 * 60;
const WEB_LOGIN_MAX_PAIRING_NONCES: usize = 128;
const WEB_LOGIN_MAX_SESSIONS: usize = 128;
static PUBLIC_HEALTH_LAST_SUCCESS_AT: Lazy<RwLock<Option<std::time::Instant>>> =
    Lazy::new(|| RwLock::new(None));
static APNS_NOTIFICATION_DEDUPE: Lazy<RwLock<HashMap<String, std::time::Instant>>> =
    Lazy::new(|| RwLock::new(HashMap::new()));
// 公网探针后台缓存：后台任务每 PUBLIC_PROBE_CACHE_REFRESH_SECS 刷新一次，
// connection-status 直接读这里，避免每次请求都同步阻塞跑公网 HTTP/WS 探针。
static PUBLIC_PROBE_CACHE: Lazy<RwLock<Option<CachedPublicProbe>>> =
    Lazy::new(|| RwLock::new(None));
static PUBLIC_PROBE_REFRESH_IN_FLIGHT: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));
static QUOTA_SNAPSHOT_REFRESHER_STARTED: Lazy<AtomicBool> = Lazy::new(|| AtomicBool::new(false));
static QUOTA_SNAPSHOT_REFRESH_ONCE_GATE: Lazy<Mutex<QuotaSnapshotRefreshGate>> =
    Lazy::new(|| Mutex::new(QuotaSnapshotRefreshGate::default()));

#[derive(Default)]
struct QuotaSnapshotRefreshGate {
    in_flight: HashSet<String>,
    last_started_at: HashMap<String, std::time::Instant>,
}

impl QuotaSnapshotRefreshGate {
    fn should_spawn(
        &mut self,
        key: &str,
        now: std::time::Instant,
        cooldown: std::time::Duration,
    ) -> bool {
        if self.in_flight.contains(key) {
            return false;
        }
        if self
            .last_started_at
            .get(key)
            .is_some_and(|started_at| now.duration_since(*started_at) < cooldown)
        {
            return false;
        }
        self.in_flight.insert(key.to_string());
        self.last_started_at.insert(key.to_string(), now);
        true
    }

    fn finish(&mut self, key: &str) {
        self.in_flight.remove(key);
    }
}

#[derive(Clone)]
struct CachedPublicProbe {
    http_value: serde_json::Value,
    http_ok: bool,
    ws_value: serde_json::Value,
    ws_ok: bool,
    refreshed_at: std::time::Instant,
}

pub(super) struct TimelineSyncService;

impl TimelineSyncService {
    fn normalize_route_key(value: Option<&str>) -> Option<String> {
        value
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(ToOwned::to_owned)
    }

    pub(super) fn node_matches_route(
        node: &ConversationNode,
        tree_id: &str,
        request_key: Option<&str>,
        project_key: Option<&str>,
    ) -> bool {
        if let Some(conversation_id) =
            Self::normalize_route_key(node.metadata.conversation_id.as_deref())
        {
            if conversation_id != tree_id {
                return false;
            }
        }

        if let Some(project_key) = project_key {
            if let Some(node_project) =
                Self::normalize_route_key(node.metadata.project_path.as_deref())
            {
                if node_project != project_key {
                    return false;
                }
            }
        }

        if let Some(request_key) = request_key {
            if let Some(node_request) =
                Self::normalize_route_key(node.metadata.request_id.as_deref())
            {
                if node_request != request_key {
                    return false;
                }
            }
        }

        true
    }

    fn strip_heavy_metadata(node: &ConversationNode) -> serde_json::Value {
        let mut node_value = match serde_json::to_value(node) {
            Ok(value) => value,
            Err(err) => {
                log::warn!("[TimelineSync] 节点序列化失败，使用精简降级格式: {}", err);
                serde_json::json!({
                    "id": node.id.clone(),
                    "parent_id": node.parent_id.clone(),
                    "timestamp": node.timestamp.clone(),
                    "node_type": node.node_type.clone(),
                    "content": node.content.clone(),
                    "is_markdown": node.is_markdown,
                    "metadata": node.metadata.clone(),
                })
            }
        };
        if let Some(images) = node_value
            .get_mut("metadata")
            .and_then(|metadata| metadata.get_mut("images"))
            .and_then(|images| images.as_array_mut())
        {
            for image in images {
                if let Some(image_obj) = image.as_object_mut() {
                    image_obj.remove("data");
                }
            }
        }
        node_value
    }

    fn strip_heavy_metadata_value(mut node_value: serde_json::Value) -> serde_json::Value {
        if let Some(images) = node_value
            .get_mut("metadata")
            .and_then(|metadata| metadata.get_mut("images"))
            .and_then(|images| images.as_array_mut())
        {
            for image in images {
                if let Some(image_obj) = image.as_object_mut() {
                    image_obj.remove("data");
                }
            }
        }
        node_value
    }

    fn metadata_string_value(node: &serde_json::Value, keys: &[&str]) -> Option<String> {
        keys.iter().find_map(|key| {
            node.get("metadata")
                .and_then(|metadata| metadata.get(*key))
                .and_then(|value| value.as_str())
                .and_then(|value| Self::normalize_route_key(Some(value)))
        })
    }

    fn timeline_value_matches_route(
        node: &serde_json::Value,
        request_key: Option<&str>,
        project_key: Option<&str>,
        conversation_id: Option<&str>,
    ) -> bool {
        if let Some(expected_conversation_id) = conversation_id {
            if let Some(node_conversation_id) =
                Self::metadata_string_value(node, &["conversation_id", "conversationId"])
            {
                if node_conversation_id != expected_conversation_id {
                    return false;
                }
            }
        }

        if let Some(project_key) = project_key {
            if let Some(node_project_path) =
                Self::metadata_string_value(node, &["project_path", "projectPath"])
            {
                if node_project_path != project_key {
                    return false;
                }
            }
        }

        if let Some(request_key) = request_key {
            if let Some(node_request_id) =
                Self::metadata_string_value(node, &["request_id", "requestId"])
            {
                if node_request_id != request_key {
                    return false;
                }
            }
        }

        true
    }

    fn sanitize_timeline_values(
        nodes: &[serde_json::Value],
        request_key: Option<&str>,
        project_key: Option<&str>,
        conversation_id: Option<&str>,
    ) -> Vec<serde_json::Value> {
        let request_key = Self::normalize_route_key(request_key);
        let project_key = Self::normalize_route_key(project_key);
        let conversation_id = Self::normalize_route_key(conversation_id);

        nodes
            .iter()
            .filter(|node| {
                Self::timeline_value_matches_route(
                    node,
                    request_key.as_deref(),
                    project_key.as_deref(),
                    conversation_id.as_deref(),
                )
            })
            .cloned()
            .map(Self::strip_heavy_metadata_value)
            .collect()
    }

    fn strip_and_filter_nodes(
        nodes: &[ConversationNode],
        tree_id: &str,
        request_key: Option<&str>,
        project_key: Option<&str>,
    ) -> Vec<serde_json::Value> {
        let request_key = Self::normalize_route_key(request_key);
        let project_key = Self::normalize_route_key(project_key);
        nodes
            .iter()
            .filter(|node| {
                Self::node_matches_route(
                    node,
                    tree_id,
                    request_key.as_deref(),
                    project_key.as_deref(),
                )
            })
            .map(Self::strip_heavy_metadata)
            .collect()
    }

    fn sanitize_payload_timeline_nodes(payload: &mut serde_json::Value) {
        let request_key = extract_timeline_route_id_from_mcp_state(payload)
            .or_else(|| extract_request_id_from_mcp_state(payload));
        let project_key = extract_project_path_from_mcp_state(payload);
        let conversation_id = extract_conversation_id_from_mcp_state(payload);
        let Some(nodes) = payload
            .get("timelineNodes")
            .and_then(|value| value.as_array())
        else {
            return;
        };

        let sanitized = Self::sanitize_timeline_values(
            nodes,
            request_key.as_deref(),
            project_key.as_deref(),
            conversation_id.as_deref(),
        );
        if let Some(obj) = payload.as_object_mut() {
            obj.insert("timelineNodes".to_string(), serde_json::json!(sanitized));
        }
    }

    fn empty_snapshot_message(
        request_key: Option<String>,
        project_key: Option<String>,
    ) -> BridgeMessage {
        BridgeMessage {
            message_type: TIMELINE_SYNC_SNAPSHOT_MESSAGE_TYPE.to_string(),
            payload: serde_json::json!({
                "request_id": request_key,
                "project_path": project_key,
                "conversation_id": serde_json::Value::Null,
                "timelineNodes": Vec::<serde_json::Value>::new(),
            }),
        }
    }

    async fn build_snapshot_message(
        app_handle: &AppHandle,
        request_id: Option<&str>,
        project_path: Option<&str>,
    ) -> Option<BridgeMessage> {
        let request_key = Self::normalize_route_key(request_id);
        let project_key = Self::normalize_route_key(project_path);
        let fallback_route = if request_key.is_none() && project_key.is_none() {
            last_active_route().await
        } else {
            None
        };
        let lookup_request_key = request_key.clone().or_else(|| fallback_route.clone());
        let lookup_project_key = project_key.clone().or_else(|| fallback_route);

        let Some(manager) = app_handle.try_state::<Arc<ConversationManager>>() else {
            log::warn!("[TimelineSync] ConversationManager 不可用，返回空快照");
            return Some(Self::empty_snapshot_message(request_key, project_key));
        };

        let Some(tree_id) = manager
            .get_tree_for_route(lookup_request_key.as_deref(), lookup_project_key.as_deref())
            .await
        else {
            return Some(Self::empty_snapshot_message(request_key, project_key));
        };
        let nodes = if let Some(current_node_id) = manager.get_current_node_id(&tree_id).await {
            match manager.get_node_path(&tree_id, &current_node_id).await {
                Ok(path) => path,
                Err(err) => {
                    log::warn!(
                        "[TimelineSync] 获取时间线路径失败: tree_id={}, node_id={}, error={}",
                        tree_id,
                        current_node_id,
                        err
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };
        let timeline_nodes = Self::strip_and_filter_nodes(
            &nodes,
            &tree_id,
            request_key.as_deref(),
            project_key.as_deref(),
        );

        Some(BridgeMessage {
            message_type: TIMELINE_SYNC_SNAPSHOT_MESSAGE_TYPE.to_string(),
            payload: serde_json::json!({
                "request_id": request_key,
                "project_path": project_key,
                "conversation_id": tree_id,
                "timelineNodes": timeline_nodes,
            }),
        })
    }

    pub(super) fn build_delta_message(
        request_id: Option<&str>,
        project_path: Option<&str>,
        node: &ConversationNode,
    ) -> BridgeMessage {
        let request_key = Self::normalize_route_key(request_id);
        let project_key = Self::normalize_route_key(project_path);
        BridgeMessage {
            message_type: TIMELINE_SYNC_DELTA_MESSAGE_TYPE.to_string(),
            payload: serde_json::json!({
                "request_id": request_key,
                "timeline_route_id": request_key,
                "project_path": project_key,
                "conversation_id": node.metadata.conversation_id.clone(),
                "timelineNode": Self::strip_heavy_metadata(node),
            }),
        }
    }
}

pub(super) static BRIDGE_BROADCAST: Lazy<broadcast::Sender<BridgeMessage>> = Lazy::new(|| {
    let (tx, _) = broadcast::channel(100);
    tx
});

/// 广播防止睡眠状态给所有 WebSocket 客户端
pub fn broadcast_prevent_sleep_status(enabled: bool) {
    let broadcast_msg = BridgeMessage {
        message_type: "prevent_sleep_status".to_string(),
        payload: serde_json::json!({
            "enabled": enabled
        }),
    };
    let _ = BRIDGE_BROADCAST.send(broadcast_msg);
    log::info!("[Bridge] 广播防止睡眠状态: {}", enabled);
}

/// 广播防止睡眠状态给所有客户端（包括 Tauri 前端和 WebSocket 客户端）
pub fn broadcast_prevent_sleep_status_with_app(app: &tauri::AppHandle, enabled: bool) {
    // 广播给 WebSocket 客户端
    broadcast_prevent_sleep_status(enabled);

    // 发送 Tauri 事件给前端
    use tauri::Emitter;
    let _ = app.emit(
        "prevent_sleep_status",
        serde_json::json!({ "enabled": enabled }),
    );
}

fn broadcast_prevent_sleep_status_for_app(app: Option<&tauri::AppHandle>, enabled: bool) {
    if let Some(app) = app {
        broadcast_prevent_sleep_status_with_app(app, enabled);
    } else {
        broadcast_prevent_sleep_status(enabled);
    }
}

/// 广播自定义prompt配置变更给所有客户端
/// 用于跨进程同步上下文追加状态
pub fn broadcast_custom_prompt_config_changed(app: &tauri::AppHandle) {
    // 广播给 WebSocket 客户端（iOS/Web）
    let broadcast_msg = BridgeMessage {
        message_type: "custom_prompt_config_changed".to_string(),
        payload: serde_json::json!({
            "timestamp": chrono::Utc::now().to_rfc3339()
        }),
    };
    let _ = BRIDGE_BROADCAST.send(broadcast_msg);

    // 发送 Tauri 事件给同进程前端
    use tauri::Emitter;
    let _ = app.emit(
        "custom-prompt-config-changed",
        serde_json::json!({ "timestamp": chrono::Utc::now().to_rfc3339() }),
    );

    log::info!("[Bridge] 广播自定义prompt配置变更");
}

/// 广播幽灵补全词表变更给所有客户端
pub fn broadcast_ghost_suggestions_changed(
    app: &tauri::AppHandle,
    ghost_suggestions: serde_json::Value,
) {
    broadcast_ghost_suggestions_changed_to_bridge(ghost_suggestions.clone());

    use tauri::Emitter;
    let _ = app.emit("ghost-suggestions-changed", ghost_suggestions);

    log::info!("[Bridge] 广播幽灵补全词表变更");
}

fn broadcast_ghost_suggestions_changed_to_bridge(ghost_suggestions: serde_json::Value) {
    let timestamp = chrono::Utc::now().to_rfc3339();
    let broadcast_msg = BridgeMessage {
        message_type: "ghost_suggestions_changed".to_string(),
        payload: serde_json::json!({
            "timestamp": timestamp,
            "ghostSuggestions": ghost_suggestions,
        }),
    };
    let _ = BRIDGE_BROADCAST.send(broadcast_msg);
}

pub static MCP_STATE_CACHE: Lazy<Arc<RwLock<HashMap<String, serde_json::Value>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static MCP_STATE_CACHE_TOUCHED_AT: Lazy<
    Arc<RwLock<HashMap<String, chrono::DateTime<chrono::Utc>>>>,
> = Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

static ACTIVE_SESSION_REGISTRY: Lazy<Arc<RwLock<HashMap<String, ActiveSessionEntry>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

static MCP_ACTION_CACHE: Lazy<Arc<RwLock<HashMap<String, serde_json::Value>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static MCP_ACTION_CACHE_TOUCHED_AT: Lazy<
    Arc<RwLock<HashMap<String, chrono::DateTime<chrono::Utc>>>>,
> = Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

static PUSH_SUBSCRIPTIONS: Lazy<Arc<RwLock<HashMap<String, WebPushSubscriptionInfo>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
const MAX_WEB_PUSH_SUBSCRIPTIONS: usize = 32;
const MAX_WEB_PUSH_ENDPOINT_LENGTH: usize = 2048;
const MAX_WEB_PUSH_P256DH_LENGTH: usize = 256;
const MAX_WEB_PUSH_AUTH_LENGTH: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebPushSubscriptionKeys {
    p256dh: String,
    auth: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WebPushSubscriptionInfo {
    endpoint: String,
    keys: WebPushSubscriptionKeys,
}

impl WebPushSubscriptionInfo {
    #[cfg(test)]
    fn new(endpoint: &str, p256dh: &str, auth: &str) -> Self {
        Self {
            endpoint: endpoint.to_string(),
            keys: WebPushSubscriptionKeys {
                p256dh: p256dh.to_string(),
                auth: auth.to_string(),
            },
        }
    }
}

fn is_allowed_web_push_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "fcm.googleapis.com"
        || host == "web.push.apple.com"
        || host == "push.services.mozilla.com"
        || host.ends_with(".push.services.mozilla.com")
        || host.ends_with(".notify.windows.com")
}

fn validate_web_push_endpoint(endpoint: &str) -> Result<reqwest::Url, String> {
    if endpoint.is_empty() || endpoint.len() > MAX_WEB_PUSH_ENDPOINT_LENGTH {
        return Err("Web Push endpoint 长度无效".to_string());
    }

    let endpoint_url =
        reqwest::Url::parse(endpoint).map_err(|_| "Web Push endpoint 无效".to_string())?;
    if endpoint_url.scheme() != "https" {
        return Err("Web Push endpoint 必须使用 HTTPS".to_string());
    }
    if !endpoint_url.username().is_empty() || endpoint_url.password().is_some() {
        return Err("Web Push endpoint 不允许包含凭据".to_string());
    }
    if endpoint_url.port_or_known_default() != Some(443) {
        return Err("Web Push endpoint 只允许 HTTPS 443 端口".to_string());
    }
    let host = endpoint_url
        .host_str()
        .ok_or_else(|| "Web Push endpoint 缺少主机名".to_string())?;
    if host.parse::<std::net::IpAddr>().is_ok() || !is_allowed_web_push_host(host) {
        return Err("Web Push endpoint 不属于受支持的 Push 服务".to_string());
    }

    Ok(endpoint_url)
}

fn validate_web_push_subscription(subscription: &WebPushSubscriptionInfo) -> Result<(), String> {
    validate_web_push_endpoint(&subscription.endpoint)?;
    if subscription.keys.p256dh.is_empty()
        || subscription.keys.p256dh.len() > MAX_WEB_PUSH_P256DH_LENGTH
    {
        return Err("Web Push p256dh 长度无效".to_string());
    }
    if subscription.keys.auth.is_empty() || subscription.keys.auth.len() > MAX_WEB_PUSH_AUTH_LENGTH
    {
        return Err("Web Push auth 长度无效".to_string());
    }
    Ok(())
}

fn web_push_subscription_capacity_available(current: usize, replaces_existing: bool) -> bool {
    replaces_existing || current < MAX_WEB_PUSH_SUBSCRIPTIONS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MobilePairingCandidate {
    transport_mode: String,
    base_url: String,
    ws_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay_device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay_pairing_token: Option<String>,
    #[serde(default)]
    health: String,
    #[serde(default)]
    disabled: bool,
    warning: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MobilePairingPayload {
    version: u8,
    pairing_session_id: String,
    device_id: String,
    device_name: String,
    transport_mode: String,
    base_url: String,
    ws_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay_device_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    relay_pairing_token: Option<String>,
    #[serde(default)]
    candidates: Vec<MobilePairingCandidate>,
    pairing_token: String,
    issued_at: String,
    expires_at: String,
    warning: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QuickTunnelStartRequest {
    #[serde(default)]
    consent_v1: bool,
}

#[derive(Debug, Clone)]
struct PairingTokenInfo {
    session_id: String,
    issued_at: String,
    expires_at: String,
    #[allow(dead_code)]
    state: String,
    failure_count: u32,
    first_failed_at: Option<String>,
    transport_mode: String,
    formal_route_generation: Option<u64>,
    endpoint_binding: Option<crate::tunnel::manager::QuickTunnelPairingBinding>,
}

#[derive(Debug, Clone)]
struct MobilePairingClaimReceipt {
    session_id: String,
    device_id: String,
    device_name: String,
    client_kind: String,
    device_token: String,
    scopes: Vec<String>,
    claimed_at: String,
    expires_at: String,
}

#[derive(Debug, Clone, Serialize)]
struct MobilePairingSessionSnapshot {
    session_id: String,
    state: String,
    expires_at: String,
    device_id: Option<String>,
    device_name: Option<String>,
    client_kind: Option<String>,
    claimed_at: Option<String>,
    connected_at: Option<String>,
    selected_transport_mode: Option<String>,
}

#[derive(Debug, Clone)]
struct WebLoginPairingNonce {
    device_id: String,
    cf_origin: String,
    console_origin: String,
    scopes: Vec<String>,
    issued_at: String,
    expires_at: String,
}

#[derive(Debug, Clone)]
struct WebLoginSession {
    session_id: String,
    device_id: String,
    cf_origin: String,
    console_origin: String,
    scopes: Vec<String>,
    issued_at: String,
    expires_at: String,
    last_seen_at: String,
    revoked_at: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebLoginSessionSummary {
    pub session_id: String,
    pub device_id: String,
    pub cf_origin: String,
    pub console_origin: String,
    pub scopes: Vec<String>,
    pub issued_at: String,
    pub expires_at: String,
    pub last_seen_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct WebLoginPairingIssueResponse {
    pub ok: bool,
    pub device_id: String,
    pub cf_origin: String,
    pub console_origin: String,
    pub pair_url: String,
    pub nonce: String,
    pub scopes: Vec<String>,
    pub issued_at: String,
    pub expires_at: String,
}

#[derive(Debug, Deserialize)]
struct WebLoginPairClaimRequest {
    nonce: String,
    device_id: String,
    cf_origin: String,
    #[serde(default)]
    requested_scopes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WebLoginPairPageQuery {
    nonce: String,
    device_id: String,
    cf_origin: String,
}

#[derive(Debug, Deserialize)]
struct MobilePairingClaimRequest {
    pairing_token: String,
    device_id: String,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    client_kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct MobilePairingClaimResponse {
    ok: bool,
    device_id: String,
    device_token: String,
    scopes: Vec<String>,
    pairing_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PairedDeviceRecord {
    device_id: String,
    device_name: String,
    client_kind: String,
    token_hash: String,
    scopes: Vec<String>,
    created_at: String,
    last_seen_at: String,
    #[serde(default)]
    file_browser_roots: Vec<String>,
    #[serde(default)]
    revoked_at: Option<String>,
}

#[derive(Debug, Serialize)]
struct PairedDeviceFileRootsSummary {
    device_id: String,
    device_name: String,
    client_kind: String,
    created_at: String,
    last_seen_at: String,
    file_browser_roots: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct UpdatePairedDeviceFileRootsRequest {
    device_id: String,
    roots: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct PairedDeviceStore {
    #[serde(default)]
    devices: Vec<PairedDeviceRecord>,
}

#[derive(Debug, Clone)]
struct AuthPrincipal {
    principal_id: String,
    device_id: String,
    client_kind: String,
    scopes: Vec<String>,
}

struct WebsocketAuthentication {
    principal: AuthPrincipal,
    allowed_browser_origins: Option<Vec<String>>,
}

struct HttpAuthentication {
    principal: AuthPrincipal,
    allowed_browser_origins: Option<Vec<String>>,
}

impl AuthPrincipal {
    fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|value| value == scope) || self.has_legacy_default_scope(scope)
    }

    fn has_legacy_default_scope(&self, scope: &str) -> bool {
        scope == SCOPE_NOTIFICATION_SUBSCRIBE
            && self.client_kind.eq_ignore_ascii_case("ios")
            && self.scopes.iter().any(|value| value == SCOPE_SESSION_READ)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WsClientInfo {
    client_id: String,
    connected_at: String,
    last_seen_at: String,
    last_message_type: Option<String>,
    remote_addr: Option<String>,
    host: String,
    x_forwarded_for: String,
    x_forwarded_proto: String,
    cf_ray: String,
    user_agent: String,
    authenticated: bool,
    /// Immutable identity captured from the credential used at WS upgrade. Mutable
    /// hello metadata remains diagnostic-only and cannot complete pairing.
    authenticated_device_id: Option<String>,
    authenticated_client_kind: Option<String>,
    client_kind: String,
    device_id: Option<String>,
    selected_transport_mode: Option<String>,
    selected_ws_url: Option<String>,
    project_path: Option<String>,
    request_id: Option<String>,
}

static VAPID_CONFIG: Lazy<VapidConfig> = Lazy::new(load_vapid_config);
static APNS_CONFIG: Lazy<Option<ApnsConfig>> = Lazy::new(load_apns_config);

fn bridge_apns_default_environment() -> ApnsEnvironment {
    APNS_CONFIG
        .as_ref()
        .map(|config| config.default_environment)
        .unwrap_or_else(configured_apns_environment)
}

fn persisted_apns_environment(value: &str, config: &ApnsConfig) -> ApnsEnvironment {
    match resolve_apns_environment(Some(value), config.default_environment) {
        Ok(environment) => environment,
        Err(error) => {
            log::warn!(
                "[APNs] 已保存 environment={} 无效（{}），回退到 {}",
                value,
                error,
                config.default_environment.as_str()
            );
            config.default_environment
        }
    }
}
static APNS_HTTP_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .pool_max_idle_per_host(2)
        .tcp_keepalive(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(12))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
});
static MOBILE_PAIRING_TOKENS: Lazy<Arc<RwLock<HashMap<String, PairingTokenInfo>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
const MOBILE_PAIRING_MAX_PENDING_TOKENS: usize = 4;
static MOBILE_PAIRING_SESSIONS: Lazy<Arc<RwLock<HashMap<String, PairingTokenInfo>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static MOBILE_PAIRING_CLAIM_RECEIPTS: Lazy<
    Arc<RwLock<HashMap<String, MobilePairingClaimReceipt>>>,
> = Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static MOBILE_PAIRING_CLAIM_LOCK: Lazy<tokio::sync::Mutex<()>> =
    Lazy::new(|| tokio::sync::Mutex::new(()));
static MOBILE_CONFIG_WRITE_LOCK: Lazy<tokio::sync::Mutex<()>> =
    Lazy::new(|| tokio::sync::Mutex::new(()));
static WEB_LOGIN_PAIRING_NONCES: Lazy<Arc<RwLock<HashMap<String, WebLoginPairingNonce>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static WEB_LOGIN_SESSIONS: Lazy<Arc<RwLock<HashMap<String, WebLoginSession>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static WS_CLIENT_REGISTRY: Lazy<Arc<RwLock<HashMap<String, WsClientInfo>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static PHONE_ACTION_RESULTS: Lazy<Arc<RwLock<HashMap<String, PhoneActionResultEntry>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));
static PHONE_ACTION_JOBS: Lazy<Arc<RwLock<HashMap<String, PhoneActionJobEntry>>>> =
    Lazy::new(|| Arc::new(RwLock::new(HashMap::new())));

fn mobile_auth_required() -> bool {
    // macOS additionally requires authentication for loopback requests because
    // the signed-process broker supplies a bounded local capability. Other
    // platforms may keep unauthenticated loopback compatibility, but public,
    // forwarded, and non-loopback requests are always authenticated below.
    cfg!(target_os = "macos")
}

fn bridge_auth_required_for_request(headers: &HeaderMap, remote_addr: SocketAddr) -> bool {
    mobile_auth_required()
        || is_public_bridge_request(headers)
        || direct_network_peer_requires_auth(remote_addr)
}

/// The Bridge terminates plain HTTP locally, while Cloudflare or another
/// trusted tunnel may terminate TLS upstream. Only an unambiguous HTTPS
/// declaration is accepted; malformed or multi-hop forwarding falls back to
/// HTTP and therefore cannot broaden the origin match.
fn effective_bridge_http_scheme(headers: &HeaderMap) -> &'static str {
    let x_forwarded_https = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("https"));
    let forwarded_https = headers
        .get("forwarded")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            !value.contains(',')
                && value
                    .split(';')
                    .any(|field| field.trim().eq_ignore_ascii_case("proto=https"))
        });
    let cloudflare_https = headers
        .get("cf-visitor")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| serde_json::from_str::<serde_json::Value>(value).ok())
        .and_then(|value| {
            value
                .get("scheme")
                .and_then(|scheme| scheme.as_str())
                .map(str::to_owned)
        })
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https"));
    if x_forwarded_https || forwarded_https || cloudflare_https {
        "https"
    } else {
        "http"
    }
}

fn canonical_bridge_authority(value: &str, scheme: &str) -> Option<String> {
    let authority = value.parse::<axum::http::uri::Authority>().ok()?;
    if authority.as_str().contains('@') {
        return None;
    }
    let host = authority.host().trim();
    if host.is_empty() || host.chars().any(|character| character.is_control()) {
        return None;
    }
    let host = host.to_ascii_lowercase();
    let host = if host.contains(':') && !(host.starts_with('[') && host.ends_with(']')) {
        format!("[{host}]")
    } else {
        host
    };
    match authority.port_u16() {
        Some(443) if scheme == "https" => Some(host),
        Some(80) if scheme == "http" => Some(host),
        Some(port) => Some(format!("{host}:{port}")),
        None => Some(host),
    }
}

fn canonical_browser_origin(value: &str) -> Option<String> {
    let uri = value.parse::<axum::http::Uri>().ok()?;
    if !matches!(uri.path(), "" | "/") || uri.query().is_some() {
        return None;
    }
    let scheme = uri.scheme_str()?.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = canonical_bridge_authority(uri.authority()?.as_str(), &scheme)?;
    Some(format!("{scheme}://{authority}"))
}

fn expected_bridge_browser_origin(headers: &HeaderMap) -> Option<String> {
    let scheme = effective_bridge_http_scheme(headers);
    let host = headers.get(header::HOST)?.to_str().ok()?;
    let authority = canonical_bridge_authority(host, scheme)?;
    Some(format!("{scheme}://{authority}"))
}

/// WebSocket handshakes bypass CORS. Browsers must therefore send the exact
/// Bridge origin. Native URLSession clients normally omit Origin and continue
/// through bearer/capability authentication unchanged.
fn browser_websocket_origin_is_allowed(
    headers: &HeaderMap,
    allowed_browser_origins: Option<&[String]>,
    auth_required: bool,
) -> bool {
    if !auth_required {
        return true;
    }
    let Some(origin) = headers.get(header::ORIGIN) else {
        // A web-login cookie is only valid from the console origins captured
        // during pairing. Native bearer/capability clients normally omit it.
        return allowed_browser_origins.is_none();
    };
    let Some(origin) = origin.to_str().ok().and_then(canonical_browser_origin) else {
        return false;
    };
    if let Some(allowed_origins) = allowed_browser_origins {
        return allowed_origins.iter().any(|allowed| {
            canonical_browser_origin(allowed).is_some_and(|allowed| allowed == origin)
        });
    }
    expected_bridge_browser_origin(headers).is_some_and(|expected| expected == origin)
}

/// Cookie-authenticated HTTP requests are browser credentials and must stay
/// bound to the exact console origins captured when that session was paired.
/// Bearer/capability clients do not use this check.
fn browser_http_origin_is_allowed(headers: &HeaderMap, allowed_origins: &[String]) -> bool {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .and_then(canonical_browser_origin)
    else {
        return false;
    };
    allowed_origins
        .iter()
        .any(|allowed| canonical_browser_origin(allowed).is_some_and(|allowed| allowed == origin))
}

const WS_DEVICE_TOKEN_PROTOCOL_PREFIX: &str = "iterate.device-token.";
const WS_DESKTOP_TOKEN_PROTOCOL_PREFIX: &str = "iterate.desktop-token.";

fn mobile_device_scopes(allow_ghost_suggestions_write: bool) -> Vec<String> {
    let mut scopes = vec![
        SCOPE_STATUS_READ.to_string(),
        SCOPE_SESSION_READ.to_string(),
        SCOPE_SESSION_RESPOND.to_string(),
        SCOPE_WINDOW_SHOW.to_string(),
        SCOPE_CONFIG_READ.to_string(),
        SCOPE_CONFIG_WRITE.to_string(),
        SCOPE_PROMPT_LIBRARY_READ.to_string(),
        SCOPE_PROMPT_LIBRARY_WRITE.to_string(),
        SCOPE_NOTIFICATION_SUBSCRIBE.to_string(),
        SCOPE_SPEECH_MEMORY_READ.to_string(),
        SCOPE_SPEECH_MEMORY_WRITE.to_string(),
        SCOPE_GHOST_SUGGESTIONS_READ.to_string(),
        SCOPE_PHONE_ACTION_JOB_READ.to_string(),
        SCOPE_TUNNEL_RECOVER.to_string(),
        SCOPE_FILE_LIST.to_string(),
        SCOPE_PAIRING_ISSUE.to_string(),
    ];
    if allow_ghost_suggestions_write {
        scopes.push(SCOPE_GHOST_SUGGESTIONS_WRITE.to_string());
    }
    scopes
}

fn normalize_mobile_device_scopes(device: &mut PairedDeviceRecord) -> bool {
    if !device.client_kind.eq_ignore_ascii_case("ios") {
        return false;
    }

    let allow_ghost_suggestions_write = device
        .scopes
        .iter()
        .any(|scope| scope == SCOPE_GHOST_SUGGESTIONS_WRITE);
    let expected_scopes = mobile_device_scopes(allow_ghost_suggestions_write);
    let mut changed = false;

    for scope in expected_scopes {
        if !device.scopes.iter().any(|existing| existing == &scope) {
            device.scopes.push(scope);
            changed = true;
        }
    }

    changed
}

fn normalize_paired_device_store(store: &mut PairedDeviceStore) -> bool {
    let mut changed = false;

    for device in &mut store.devices {
        if device.revoked_at.is_none() && normalize_mobile_device_scopes(device) {
            changed = true;
        }
    }

    changed
}

fn mobile_ghost_suggestions_write_enabled(app_handle: Option<&AppHandle>) -> bool {
    if let Some(app_handle) = app_handle {
        if let Some(state) = app_handle.try_state::<crate::config::AppState>() {
            if let Ok(config) = state.config.lock() {
                return config.mobile_config.allow_ghost_suggestions_write;
            }
        }
    }

    crate::config::storage::load_standalone_config()
        .map(|config| config.mobile_config.allow_ghost_suggestions_write)
        .unwrap_or(false)
}

fn generate_bridge_token(prefix: &str) -> String {
    let rng = ring::rand::SystemRandom::new();
    let mut bytes = [0_u8; 32];
    if rng.fill(&mut bytes).is_ok() {
        return format!("{}_{}", prefix, URL_SAFE_NO_PAD.encode(bytes));
    }

    format!("{}_{}", prefix, uuid::Uuid::new_v4())
}

fn bridge_token_hash(token: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, token.as_bytes());
    format!("sha256:{}", hex::encode(digest.as_ref()))
}

fn bridge_token_hash_matches(token: &str, stored_hash: &str) -> bool {
    let actual = bridge_token_hash(token);
    actual == stored_hash
}

fn paired_devices_path() -> PathBuf {
    crate::config::iterate_bridge_state_dir().join("paired-devices.json")
}

const PAIRED_DEVICE_LAST_SEEN_WRITE_INTERVAL_SECS: i64 = 60;
const MAX_FILE_BROWSER_ROOTS_PER_DEVICE: usize = 8;

struct PairedDeviceStoreLock {
    file: File,
}

impl Drop for PairedDeviceStoreLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
    }
}

fn paired_devices_backup_path(path: &FilePath) -> PathBuf {
    path.with_file_name(format!(
        "{}.bak",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("paired-devices.json")
    ))
}

fn paired_devices_lock_path(path: &FilePath) -> PathBuf {
    path.with_file_name(format!(
        "{}.lock",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("paired-devices.json")
    ))
}

fn lock_paired_device_store(path: &FilePath) -> Result<PairedDeviceStoreLock, String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建设备目录失败: {}", err))?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(paired_devices_lock_path(path))
        .map_err(|err| format!("打开设备授权锁失败: {}", err))?;

    #[cfg(unix)]
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(format!(
            "锁定设备授权失败: {}",
            std::io::Error::last_os_error()
        ));
    }

    Ok(PairedDeviceStoreLock { file })
}

fn atomic_write_private_file(path: &FilePath, content: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| format!("创建设备目录失败: {}", err))?;
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("paired-devices.json");
    let temp_path = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        file_name,
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|err| format!("创建设备授权临时文件失败: {}", err))?;
        file.write_all(content)
            .map_err(|err| format!("写入设备授权临时文件失败: {}", err))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|err| format!("设置设备授权权限失败: {}", err))?;
        }
        file.sync_all()
            .map_err(|err| format!("同步设备授权临时文件失败: {}", err))?;
        std::fs::rename(&temp_path, path)
            .map_err(|err| format!("原子替换设备授权失败: {}", err))?;
        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            File::open(parent)
                .and_then(|directory| directory.sync_all())
                .map_err(|err| format!("同步设备授权目录失败: {}", err))?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    result
}

fn read_paired_device_store_candidate(
    path: &FilePath,
) -> Result<Option<(PairedDeviceStore, Vec<u8>)>, String> {
    let content = match std::fs::read(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("读取设备授权失败: {}", err)),
    };
    let store = serde_json::from_slice::<PairedDeviceStore>(&content)
        .map_err(|err| format!("解析设备授权失败: {}", err))?;
    Ok(Some((store, content)))
}

fn load_paired_device_store_with_recovery_at(
    path: &FilePath,
) -> Result<(PairedDeviceStore, bool), String> {
    match read_paired_device_store_candidate(path) {
        Ok(Some((store, _))) => Ok((store, false)),
        Ok(None) => match read_paired_device_store_candidate(&paired_devices_backup_path(path)) {
            Ok(Some((store, _))) => Ok((store, true)),
            Ok(None) => Ok((PairedDeviceStore::default(), false)),
            Err(backup_error) => Err(backup_error),
        },
        Err(primary_error) => {
            match read_paired_device_store_candidate(&paired_devices_backup_path(path)) {
                Ok(Some((store, _))) => Ok((store, true)),
                Ok(None) => Err(primary_error),
                Err(backup_error) => Err(format!(
                    "{}; 备份设备授权也不可用: {}",
                    primary_error, backup_error
                )),
            }
        }
    }
}

fn load_paired_device_store_at(path: &FilePath) -> Result<PairedDeviceStore, String> {
    load_paired_device_store_with_recovery_at(path).map(|(store, _)| store)
}

fn save_paired_device_store_at(path: &FilePath, store: &PairedDeviceStore) -> Result<(), String> {
    let content =
        serde_json::to_vec_pretty(store).map_err(|err| format!("序列化设备授权失败: {}", err))?;
    if let Ok(Some((_, current_content))) = read_paired_device_store_candidate(path) {
        atomic_write_private_file(&paired_devices_backup_path(path), &current_content)?;
    }
    atomic_write_private_file(path, &content)
}

fn mutate_paired_device_store_at<T>(
    path: &FilePath,
    mutate: impl FnOnce(&mut PairedDeviceStore) -> (T, bool),
) -> Result<T, String> {
    let _lock = lock_paired_device_store(path)?;
    let (mut store, recovered_from_backup) = load_paired_device_store_with_recovery_at(path)?;
    let normalized = normalize_paired_device_store(&mut store);
    let (result, changed) = mutate(&mut store);
    if recovered_from_backup || normalized || changed {
        save_paired_device_store_at(path, &store)?;
    }
    Ok(result)
}

fn should_persist_paired_device_last_seen(
    previous: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    parse_rfc3339(previous)
        .map(|last_seen| {
            now.signed_duration_since(last_seen).num_seconds()
                >= PAIRED_DEVICE_LAST_SEEN_WRITE_INTERVAL_SECS
        })
        .unwrap_or(true)
}

fn normalize_file_browser_roots(roots: &[String]) -> Result<Vec<String>, &'static str> {
    if roots.len() > MAX_FILE_BROWSER_ROOTS_PER_DEVICE {
        return Err("too_many_file_browser_roots");
    }

    let mut normalized = Vec::new();
    for raw_root in roots {
        let raw_root = raw_root.trim();
        if raw_root.is_empty() || !std::path::Path::new(raw_root).is_absolute() {
            return Err("invalid_file_browser_root");
        }
        let canonical_root = std::path::Path::new(raw_root)
            .canonicalize()
            .map_err(|_| "invalid_file_browser_root")?;
        if !canonical_root.is_dir() {
            return Err("invalid_file_browser_root");
        }
        if canonical_root.parent().is_none() {
            return Err("filesystem_root_not_allowed");
        }
        let canonical_root = canonical_root
            .to_str()
            .ok_or("invalid_file_browser_root_encoding")?
            .to_string();
        if !normalized.contains(&canonical_root) {
            normalized.push(canonical_root);
        }
    }
    Ok(normalized)
}

fn replace_paired_device_record(store: &mut PairedDeviceStore, mut record: PairedDeviceRecord) {
    if let Some(existing) = store
        .devices
        .iter()
        .find(|device| device.device_id == record.device_id && device.revoked_at.is_none())
    {
        record.file_browser_roots = existing.file_browser_roots.clone();
    }
    store
        .devices
        .retain(|device| device.device_id != record.device_id);
    store.devices.push(record);
}

fn authenticate_paired_device_at(
    path: &FilePath,
    token: &str,
    requested_device_id: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<(Option<AuthPrincipal>, bool, bool), String> {
    mutate_paired_device_store_at(path, |store| {
        let mut principal = None;
        let mut persisted_last_seen = false;
        let mut explicitly_revoked = false;
        for device in &mut store.devices {
            if requested_device_id.is_some_and(|value| value != device.device_id.as_str())
                || !bridge_token_hash_matches(token, &device.token_hash)
            {
                continue;
            }

            if device.revoked_at.is_some() {
                explicitly_revoked = true;
                break;
            }

            if should_persist_paired_device_last_seen(&device.last_seen_at, now) {
                device.last_seen_at = now.to_rfc3339();
                persisted_last_seen = true;
            }
            principal = Some(AuthPrincipal {
                principal_id: format!("device:{}", device.device_id),
                device_id: device.device_id.clone(),
                client_kind: device.client_kind.clone(),
                scopes: device.scopes.clone(),
            });
            break;
        }
        (
            (principal, persisted_last_seen, explicitly_revoked),
            persisted_last_seen,
        )
    })
}

fn bearer_token_from_headers(headers: &HeaderMap) -> Option<String> {
    if let Some(value) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
    {
        let mut parts = value.splitn(2, char::is_whitespace);
        let scheme = parts.next().unwrap_or_default();
        let token = parts.next().unwrap_or_default().trim();
        if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
            return Some(token.to_string());
        }
    }

    headers
        .get("x-iterate-device-token")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn device_id_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get("x-iterate-device-id")
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn websocket_device_token_from_protocols(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(',').find_map(|protocol| {
                protocol
                    .trim()
                    .strip_prefix(WS_DEVICE_TOKEN_PROTOCOL_PREFIX)
                    .map(str::trim)
                    .filter(|token| !token.is_empty())
                    .map(ToOwned::to_owned)
            })
        })
}

fn websocket_desktop_token_from_protocols(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value.split(',').find_map(|protocol| {
                protocol
                    .trim()
                    .strip_prefix(WS_DESKTOP_TOKEN_PROTOCOL_PREFIX)
                    .map(str::trim)
                    .filter(|token| !token.is_empty())
                    .map(ToOwned::to_owned)
            })
        })
}

fn websocket_device_token_from_uri(uri: &Uri) -> Option<String> {
    uri.query().and_then(|query| {
        query.split('&').find_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let raw_key = parts.next().unwrap_or_default();
            let key = percent_encoding::percent_decode_str(raw_key).decode_utf8_lossy();
            if key == "token" || key == "device_token" {
                let raw_value = parts.next().unwrap_or_default();
                let value = percent_encoding::percent_decode_str(raw_value).decode_utf8_lossy();
                let token = value.trim();
                if !token.is_empty() {
                    return Some(token.to_string());
                }
            }
            None
        })
    })
}

fn websocket_device_token_from_message(message: &BridgeMessage) -> Option<String> {
    if message.message_type != "client_hello" {
        return None;
    }

    json_string_field(&message.payload, &["device_token", "deviceToken", "token"])
}

fn websocket_device_id_from_message(message: &BridgeMessage) -> Option<String> {
    json_string_field(&message.payload, &["device_id", "deviceId"])
}

fn redact_bridge_message_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(object) => {
            for (key, child) in object.iter_mut() {
                if matches!(key.as_str(), "token" | "device_token" | "deviceToken") {
                    *child = serde_json::Value::String("[redacted]".to_string());
                } else {
                    redact_bridge_message_value(child);
                }
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_bridge_message_value(item);
            }
        }
        _ => {}
    }
}

fn redact_bridge_message_text(text: &str) -> String {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(text) else {
        return text.to_string();
    };
    redact_bridge_message_value(&mut value);
    serde_json::to_string(&value).unwrap_or_else(|_| text.to_string())
}

const AUTH_STORE_UNAVAILABLE_ERROR: &str = "auth_store_unavailable";
const REVOKED_DEVICE_AUTH_ERROR: &str = "revoked_device_auth";

async fn authenticate_bridge_headers_result(
    headers: &HeaderMap,
) -> Result<Option<AuthPrincipal>, String> {
    Ok(authenticate_bridge_http_result(headers)
        .await?
        .map(|authentication| authentication.principal))
}

async fn authenticate_bridge_http_result(
    headers: &HeaderMap,
) -> Result<Option<HttpAuthentication>, String> {
    let requested_device_id = device_id_from_headers(headers);
    if let Some(token) = bearer_token_from_headers(headers) {
        return Ok(authenticate_bridge_token_result(token, requested_device_id)
            .await?
            .map(|principal| HttpAuthentication {
                principal,
                allowed_browser_origins: None,
            }));
    }
    let Some(token) = crate::bridge::auth::cookie_token_from_headers(headers) else {
        return Ok(None);
    };
    Ok(
        authenticate_web_login_session_token_with_origins(&token, requested_device_id.as_ref())
            .await
            .map(|(principal, allowed_browser_origins)| HttpAuthentication {
                principal,
                allowed_browser_origins: Some(allowed_browser_origins),
            }),
    )
}

async fn authenticate_bridge_websocket_result(
    headers: &HeaderMap,
    uri: &Uri,
) -> Result<Option<WebsocketAuthentication>, String> {
    let requested_device_id = device_id_from_headers(headers);
    let Some(token) = bearer_token_from_headers(headers)
        .or_else(|| websocket_device_token_from_protocols(headers))
        .or_else(|| websocket_device_token_from_uri(uri))
    else {
        let Some(cookie_token) = crate::bridge::auth::cookie_token_from_headers(headers) else {
            return Ok(None);
        };
        return Ok(authenticate_web_login_session_token_with_origins(
            &cookie_token,
            requested_device_id.as_ref(),
        )
        .await
        .map(
            |(principal, allowed_browser_origins)| WebsocketAuthentication {
                principal,
                allowed_browser_origins: Some(allowed_browser_origins),
            },
        ));
    };
    Ok(authenticate_bridge_token_result(token, requested_device_id)
        .await?
        .map(|principal| WebsocketAuthentication {
            principal,
            allowed_browser_origins: None,
        }))
}

async fn authenticate_bridge_token_result(
    token: String,
    requested_device_id: Option<String>,
) -> Result<Option<AuthPrincipal>, String> {
    let paired_device_error = match authenticate_paired_device_at(
        &paired_devices_path(),
        &token,
        requested_device_id.as_deref(),
        chrono::Utc::now(),
    ) {
        Ok((Some(principal), _, _)) => return Ok(Some(principal)),
        Ok((None, _, true)) => return Err(REVOKED_DEVICE_AUTH_ERROR.to_string()),
        Ok((None, _, false)) => None,
        Err(err) => Some(err),
    };

    if let Some(principal) =
        authenticate_web_login_session_token(&token, requested_device_id.as_ref()).await
    {
        return Ok(Some(principal));
    }

    if let Some(err) = paired_device_error {
        log::warn!(
            "[Bridge][MobileAuth] paired device store unavailable: {}",
            err
        );
        Err(AUTH_STORE_UNAVAILABLE_ERROR.to_string())
    } else {
        Ok(None)
    }
}

async fn authenticate_bridge_headers(headers: &HeaderMap) -> Option<AuthPrincipal> {
    authenticate_bridge_headers_result(headers)
        .await
        .ok()
        .flatten()
}

fn auth_store_unavailable_response() -> Response {
    json_error_response(
        StatusCode::SERVICE_UNAVAILABLE,
        AUTH_STORE_UNAVAILABLE_ERROR,
    )
}

fn bridge_auth_error_response(error: &str) -> Response {
    if error == REVOKED_DEVICE_AUTH_ERROR {
        json_error_response(StatusCode::UNAUTHORIZED, REVOKED_DEVICE_AUTH_ERROR)
    } else {
        auth_store_unavailable_response()
    }
}

fn public_anonymous_path_allowed(method: &axum::http::Method, path: &str) -> bool {
    matches!(
        (method, path),
        (&axum::http::Method::GET, "/")
            | (&axum::http::Method::GET, "/index.html")
            | (&axum::http::Method::GET, "/bridge_test.html")
            | (&axum::http::Method::GET, "/mobile")
            | (&axum::http::Method::GET, "/apple-touch-icon.png")
            | (&axum::http::Method::GET, "/manifest.webmanifest")
            | (&axum::http::Method::GET, "/sw.js")
            | (&axum::http::Method::GET, "/push/vapid_public_key")
            | (&axum::http::Method::GET, "/.well-known/iterate/health")
            | (&axum::http::Method::GET, "/pair")
            | (&axum::http::Method::POST, "/pair/challenge")
            | (&axum::http::Method::POST, "/pair/claim")
            | (&axum::http::Method::GET, "/api/version")
            | (&axum::http::Method::GET, "/ws")
            | (&axum::http::Method::GET, "/ws/codex-live")
            | (&axum::http::Method::POST, "/api/mobile/pairing/claim")
    )
}

/// This endpoint authenticates only after Axum has buffered the exact body
/// bytes, because the one-shot capability is bound to that body's SHA-256.
/// Keep it separate from the genuinely anonymous route allowlist.
fn body_bound_auth_deferred(method: &axum::http::Method, path: &str) -> bool {
    method == axum::http::Method::POST && path == "/api/room-submit"
}

fn normalize_bridge_project_path(raw_path: &str) -> String {
    let path = std::path::Path::new(raw_path);
    if path.is_relative() {
        std::fs::canonicalize(path)
            .map(|absolute| absolute.to_string_lossy().to_string())
            .unwrap_or_else(|_| {
                std::env::current_dir()
                    .map(|cwd| cwd.join(path).to_string_lossy().to_string())
                    .unwrap_or_else(|_| raw_path.to_string())
            })
    } else {
        raw_path.to_string()
    }
}

fn broadcast_room_submit_outcome(
    tx: &broadcast::Sender<BridgeMessage>,
    outcome: &RoomSubmitOutcome,
) {
    let payload = match serde_json::to_value(outcome) {
        Ok(value) => value,
        Err(error) => {
            log::warn!("[Bridge] room submit outcome serialize failed: {}", error);
            return;
        }
    };
    let _ = tx.send(BridgeMessage {
        message_type: "room_submit_outcome".to_string(),
        payload,
    });
}

fn action_cache_key_for_pull(project_path: &str, request_id: Option<&str>) -> Option<String> {
    normalize_route_part(request_id).or_else(|| normalize_route_part(Some(project_path)))
}

fn request_id_is_stale_for_live_window_instances(
    instances: &[crate::ui::window_registry::WindowInstance],
    request_id: Option<&str>,
    project_path: Option<&str>,
) -> bool {
    let Some(target_request_id) = normalize_route_part(request_id) else {
        return false;
    };
    let Some(target_project_path) = normalize_route_part(project_path) else {
        return false;
    };

    let mut has_bound_request_for_project = false;
    let mut has_matching_request_for_project = false;

    for instance in instances {
        if normalize_route_part(Some(&instance.project_path)).as_deref()
            != Some(target_project_path.as_str())
        {
            continue;
        }

        let Some(bound_request_id) = normalize_route_part(instance.request_id.as_deref()) else {
            continue;
        };
        has_bound_request_for_project = true;
        if bound_request_id == target_request_id {
            has_matching_request_for_project = true;
            break;
        }
    }

    has_bound_request_for_project && !has_matching_request_for_project
}

fn request_id_is_stale_for_current_window_binding(
    request_id: Option<&str>,
    project_path: Option<&str>,
) -> bool {
    if normalize_route_part(request_id).is_none() || normalize_route_part(project_path).is_none() {
        return false;
    }

    let mut window_registry = crate::ui::window_registry::WindowRegistry::load();
    let instances = window_registry.get_all_instances();
    request_id_is_stale_for_live_window_instances(&instances, request_id, project_path)
}

fn request_id_is_stale_for_bridge_project_binding(
    request_id: Option<&str>,
    project_path: Option<&str>,
) -> bool {
    let normalized_project_path = project_path.map(normalize_bridge_project_path);
    request_id_is_stale_for_current_window_binding(request_id, normalized_project_path.as_deref())
}

fn take_cached_action_for_pull(
    cache: &mut HashMap<String, serde_json::Value>,
    project_path: &str,
    request_id: Option<&str>,
) -> Option<serde_json::Value> {
    match normalize_route_part(request_id) {
        Some(rid) => cache.remove(&rid),
        None => normalize_route_part(Some(project_path)).and_then(|path| cache.remove(&path)),
    }
}

fn take_cached_action_for_pull_with_window_bindings(
    cache: &mut HashMap<String, serde_json::Value>,
    project_path: &str,
    request_id: Option<&str>,
    instances: &[crate::ui::window_registry::WindowInstance],
) -> Option<serde_json::Value> {
    if request_id_is_stale_for_live_window_instances(instances, request_id, Some(project_path)) {
        if let Some(request_key) = normalize_route_part(request_id) {
            cache.remove(&request_key);
        }
        return None;
    }

    take_cached_action_for_pull(cache, project_path, request_id)
}

pub async fn remove_active_session_entry_by_request_id(request_id: &str) -> bool {
    let mut registry = ACTIVE_SESSION_REGISTRY.write().await;
    remove_active_session_entry(&mut registry, request_id)
}

pub(crate) async fn cleanup_completed_session_by_request_id(
    request_id: &str,
    source: &str,
) -> (bool, bool) {
    let mut cache = MCP_STATE_CACHE.write().await;
    let mut touched_at = MCP_STATE_CACHE_TOUCHED_AT.write().await;
    let removed_cache = remove_cached_session_entries(&mut cache, request_id);
    touched_at.remove(request_id);
    drop(cache);
    drop(touched_at);

    let removed_active = remove_active_session_entry_by_request_id(request_id).await;
    record_last_completed_route(Some(request_id), None, source).await;
    clear_active_desktop_popup_route(Some(request_id), None, source).await;
    log::info!(
        "[Bridge] {}: request_id={}, removed_cache={}, removed_active={}",
        source,
        request_id,
        removed_cache,
        removed_active
    );
    (removed_cache, removed_active)
}

fn remove_stale_mcp_state_cache_entries(
    cache: &mut HashMap<String, serde_json::Value>,
    request_id: &str,
    project_path: Option<&str>,
) -> Vec<String> {
    let Some(request_key) = normalize_route_part(Some(request_id)) else {
        return Vec::new();
    };

    let removed_payload = cache.remove(&request_key);
    let mut removed_keys = Vec::new();
    if removed_payload.is_some() {
        removed_keys.push(request_key.clone());
    }

    let project_key = normalize_route_part(project_path).or_else(|| {
        removed_payload.as_ref().and_then(|payload| {
            extract_project_path_from_mcp_state(payload)
                .and_then(|path| normalize_route_part(Some(path.as_str())))
        })
    });
    let Some(project_key) = project_key else {
        return removed_keys;
    };

    let project_payload_matches_request = cache
        .get(&project_key)
        .and_then(extract_request_id_from_mcp_state)
        .and_then(|rid| normalize_route_part(Some(rid.as_str())))
        .as_deref()
        == Some(request_key.as_str());

    if project_payload_matches_request {
        cache.remove(&project_key);
        removed_keys.push(project_key);
    }

    removed_keys
}

async fn cleanup_stale_request_sync_route(
    request_id: Option<&str>,
    project_path: Option<&str>,
    source: &str,
) -> (bool, bool) {
    let Some(request_id) = normalize_route_part(request_id) else {
        return (false, false);
    };
    let normalized_project_path = normalize_route_part(project_path);

    let removed_cache_keys = {
        let mut cache = MCP_STATE_CACHE.write().await;
        let mut touched_at = MCP_STATE_CACHE_TOUCHED_AT.write().await;
        let removed_keys = remove_stale_mcp_state_cache_entries(
            &mut cache,
            &request_id,
            normalized_project_path.as_deref(),
        );
        for key in &removed_keys {
            touched_at.remove(key);
        }
        removed_keys
    };

    let removed_active = remove_active_session_entry_by_request_id(&request_id).await;
    bridge_debug_log(&format!(
        "[Bridge Timing] stale request_sync cleanup: request_id={}, project_path={:?}, removed_cache_keys={}, removed_active={}, source={}",
        request_id,
        normalized_project_path,
        removed_cache_keys.len(),
        removed_active,
        source
    ));

    (!removed_cache_keys.is_empty(), removed_active)
}

fn resolve_host_label() -> String {
    Command::new("hostname")
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| value.split('.').next().unwrap_or("unknown").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn resolve_mobile_device_id() -> String {
    format!("mac-{}", resolve_host_label())
}

async fn mobile_pairing_command_output(
    command: &str,
    args: &[&str],
) -> Result<Option<std::process::Output>, String> {
    let mut command_builder = tokio::process::Command::new(command);
    command_builder
        .args(args)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    match tokio::time::timeout(
        std::time::Duration::from_secs(MOBILE_PAIRING_COMMAND_TIMEOUT_SECS),
        command_builder.output(),
    )
    .await
    {
        Ok(Ok(output)) => Ok(Some(output)),
        Ok(Err(err)) => Err(err.to_string()),
        Err(_) => Ok(None),
    }
}

async fn detect_tailscale_ipv4_with_source() -> Option<(String, String)> {
    let started_at = std::time::Instant::now();
    if let Ok(env_ip) = std::env::var("ITERATE_TAILSCALE_IP") {
        let trimmed = env_ip.trim();
        if is_valid_ipv4(trimmed) {
            log::info!(
                "[Bridge][MobilePairing] tailscale_detect result=env elapsed_ms={}",
                started_at.elapsed().as_millis()
            );
            return Some((trimmed.to_string(), "env:ITERATE_TAILSCALE_IP".to_string()));
        }
    }

    const TAILSCALE_COMMANDS: [&str; 2] = [
        "tailscale",
        "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    ];

    for command in TAILSCALE_COMMANDS {
        let status_started_at = std::time::Instant::now();
        let status_output = match mobile_pairing_command_output(command, &["status"]).await {
            Ok(Some(output)) => output,
            Ok(None) => {
                log::warn!(
                    "[Bridge][MobilePairing] tailscale_status command={} result=timeout timeout_secs={} elapsed_ms={}",
                    command,
                    MOBILE_PAIRING_COMMAND_TIMEOUT_SECS,
                    status_started_at.elapsed().as_millis()
                );
                continue;
            }
            Err(err) => {
                log::info!(
                    "[Bridge][MobilePairing] tailscale_status command={} result=spawn_failed error={} elapsed_ms={}",
                    command,
                    err,
                    status_started_at.elapsed().as_millis()
                );
                continue;
            }
        };
        log::info!(
            "[Bridge][MobilePairing] tailscale_status command={} success={} elapsed_ms={}",
            command,
            status_output.status.success(),
            status_started_at.elapsed().as_millis()
        );
        let status_text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&status_output.stdout),
            String::from_utf8_lossy(&status_output.stderr)
        )
        .to_lowercase();
        if !status_output.status.success()
            || status_text.contains("tailscale is stopped")
            || status_text.contains("logged out")
        {
            continue;
        }

        let ip_started_at = std::time::Instant::now();
        let output = match mobile_pairing_command_output(command, &["ip", "-4"]).await {
            Ok(Some(output)) => output,
            Ok(None) => {
                log::warn!(
                    "[Bridge][MobilePairing] tailscale_ip command={} result=timeout timeout_secs={} elapsed_ms={}",
                    command,
                    MOBILE_PAIRING_COMMAND_TIMEOUT_SECS,
                    ip_started_at.elapsed().as_millis()
                );
                continue;
            }
            Err(err) => {
                log::info!(
                    "[Bridge][MobilePairing] tailscale_ip command={} result=spawn_failed error={} elapsed_ms={}",
                    command,
                    err,
                    ip_started_at.elapsed().as_millis()
                );
                continue;
            }
        };
        log::info!(
            "[Bridge][MobilePairing] tailscale_ip command={} success={} elapsed_ms={}",
            command,
            output.status.success(),
            ip_started_at.elapsed().as_millis()
        );
        if !output.status.success() {
            continue;
        }
        if let Some(ip) = parse_first_ipv4_line(&String::from_utf8_lossy(&output.stdout)) {
            log::info!(
                "[Bridge][MobilePairing] tailscale_detect result=cli source={} elapsed_ms={}",
                command,
                started_at.elapsed().as_millis()
            );
            return Some((ip, "cli:tailscale ip -4".to_string()));
        }
    }

    let ifconfig_started_at = std::time::Instant::now();
    match mobile_pairing_command_output("/sbin/ifconfig", &[]).await {
        Ok(Some(output)) => {
            log::info!(
                "[Bridge][MobilePairing] tailscale_ifconfig success={} elapsed_ms={}",
                output.status.success(),
                ifconfig_started_at.elapsed().as_millis()
            );
            if output.status.success() {
                if let Some(ip) = parse_first_tailscale_ipv4_from_ifconfig(
                    &String::from_utf8_lossy(&output.stdout),
                ) {
                    log::info!(
                        "[Bridge][MobilePairing] tailscale_detect result=ifconfig elapsed_ms={}",
                        started_at.elapsed().as_millis()
                    );
                    return Some((ip, "ifconfig:100.64.0.0/10".to_string()));
                }
            }
        }
        Ok(None) => {
            log::warn!(
                "[Bridge][MobilePairing] tailscale_ifconfig result=timeout timeout_secs={} elapsed_ms={}",
                MOBILE_PAIRING_COMMAND_TIMEOUT_SECS,
                ifconfig_started_at.elapsed().as_millis()
            );
        }
        Err(err) => {
            log::info!(
                "[Bridge][MobilePairing] tailscale_ifconfig result=spawn_failed error={} elapsed_ms={}",
                err,
                ifconfig_started_at.elapsed().as_millis()
            );
        }
    }

    log::info!(
        "[Bridge][MobilePairing] tailscale_detect result=none elapsed_ms={}",
        started_at.elapsed().as_millis()
    );
    None
}

fn detect_lan_ipv4() -> Option<String> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    if ip.is_loopback() || !ip.is_ipv4() {
        return None;
    }
    Some(ip.to_string())
}

struct PairingCandidatesResult {
    candidates: Vec<MobilePairingCandidate>,
    primary: MobilePairingCandidate,
    tailscale_source: Option<String>,
    public_endpoint_binding: Option<crate::tunnel::manager::QuickTunnelPairingBinding>,
}

fn mobile_pairing_candidate_from_relay_config(
    config: crate::relay::RelayMobilePairingConfig,
) -> MobilePairingCandidate {
    let has_relay_pairing_token = config.relay_pairing_token.is_some();
    let disabled = config.token_present && !has_relay_pairing_token;
    let health = if disabled {
        "auth_required"
    } else if config.process_running {
        "healthy"
    } else if config.launchctl_loaded {
        "configured"
    } else {
        "configured"
    };
    let warning = if disabled {
        Some(
            "Relay Mac Client 已配置静态 token，但 relay_pairing_token 签发失败；二维码不会携带静态 token。"
                .to_string(),
        )
    } else if !config.process_running {
        Some(
            "Relay Mac Client 已配置但当前未检测到运行进程；扫码可导入 Relay route，连接前请启动 Mac client。"
                .to_string(),
        )
    } else {
        None
    };

    MobilePairingCandidate {
        transport_mode: "relay".to_string(),
        base_url: config.base_url,
        ws_url: config.ws_url,
        relay_device_id: Some(config.relay_device_id),
        relay_pairing_token: config.relay_pairing_token,
        health: health.to_string(),
        disabled,
        warning,
    }
}

fn mobile_pairing_candidate_is_usable_public_tunnel(candidate: &MobilePairingCandidate) -> bool {
    let health = candidate.health.trim().to_ascii_lowercase();
    matches!(
        candidate.transport_mode.as_str(),
        "public_tunnel" | "cloudflare_tunnel"
    ) && !candidate.disabled
        && !matches!(health.as_str(), "degraded" | "unhealthy" | "down")
}

fn mobile_pairing_candidate_is_secure_for_qr(candidate: &MobilePairingCandidate) -> bool {
    if candidate.disabled
        || !matches!(
            candidate.transport_mode.as_str(),
            "public_tunnel" | "cloudflare_tunnel" | "relay"
        )
        || !matches!(
            candidate.health.trim().to_ascii_lowercase().as_str(),
            "healthy" | "ok"
        )
    {
        return false;
    }
    let Ok(base_url) = reqwest::Url::parse(&candidate.base_url) else {
        return false;
    };
    let Ok(ws_url) = reqwest::Url::parse(&candidate.ws_url) else {
        return false;
    };
    base_url.scheme() == "https" && ws_url.scheme() == "wss"
}

fn quick_tunnel_test_capability_enabled() -> bool {
    crate::tunnel::manager::quick_tunnel_test_capability_enabled()
}

fn formal_mobile_route_status_from_candidates(
    result: &PairingCandidatesResult,
) -> crate::tunnel::commands::FormalMobileRouteStatus {
    let Some(route) = crate::tunnel::commands::configured_formal_mobile_route() else {
        return crate::tunnel::commands::FormalMobileRouteStatus {
            configured: false,
            transport: None,
            base_url: None,
            configured_at: None,
            formal_route_generation: None,
            health: "unknown".to_string(),
            health_checked_at: None,
            last_verified_at: None,
            endpoint_identity_ok: false,
            repair_reason: None,
        };
    };
    let route_base_url = route.base_url.trim_end_matches('/');
    let healthy = result.candidates.iter().any(|candidate| {
        candidate.transport_mode == "public_tunnel"
            && candidate.base_url.trim_end_matches('/') == route_base_url
            && mobile_pairing_candidate_is_secure_for_qr(candidate)
            && mobile_pairing_candidate_has_endpoint_proof(
                candidate,
                result.public_endpoint_binding.as_ref(),
            )
    });
    crate::tunnel::commands::FormalMobileRouteStatus {
        configured: true,
        transport: Some(route.transport),
        base_url: Some(route.base_url),
        configured_at: Some(route.configured_at),
        formal_route_generation: Some(route.formal_route_generation.max(1)),
        health: if healthy { "healthy" } else { "degraded" }.to_string(),
        health_checked_at: Some(chrono::Utc::now().to_rfc3339()),
        last_verified_at: route.last_verified_at,
        endpoint_identity_ok: healthy,
        repair_reason: (!healthy).then(|| "endpoint_unreachable".to_string()),
    }
}

async fn pairing_token_endpoint_binding_is_current(token: &PairingTokenInfo) -> bool {
    let Some(binding) = token.endpoint_binding.as_ref() else {
        return true;
    };
    match token.transport_mode.as_str() {
        "cloudflare_tunnel" => crate::tunnel::manager::pairing_binding_is_current(binding).await,
        "public_tunnel" => {
            let Some(route) = crate::tunnel::commands::configured_formal_mobile_route() else {
                return false;
            };
            route.formal_route_generation.max(1) == token.formal_route_generation.unwrap_or(0)
                && route.base_url.trim_end_matches('/') == binding.endpoint.trim_end_matches('/')
                && crate::tunnel::manager::pairing_binding_matches_current_install(binding)
                && crate::tunnel::manager::public_endpoint_proves_current_install(&binding.endpoint)
                    .await
        }
        _ => false,
    }
}

fn mobile_pairing_candidate_is_ready_relay(candidate: &MobilePairingCandidate) -> bool {
    let health = candidate.health.trim().to_ascii_lowercase();
    candidate.transport_mode == "relay"
        && !candidate.disabled
        && candidate
            .relay_pairing_token
            .as_deref()
            .map(str::trim)
            .is_some_and(|token| !token.is_empty())
        && matches!(health.as_str(), "healthy" | "ok")
}

fn mobile_pairing_candidate_has_endpoint_proof(
    candidate: &MobilePairingCandidate,
    endpoint_binding: Option<&crate::tunnel::manager::QuickTunnelPairingBinding>,
) -> bool {
    match candidate.transport_mode.as_str() {
        "public_tunnel" | "cloudflare_tunnel" => endpoint_binding.is_some_and(|binding| {
            binding.endpoint.trim_end_matches('/') == candidate.base_url.trim_end_matches('/')
        }),
        "relay" => mobile_pairing_candidate_is_ready_relay(candidate),
        _ => false,
    }
}

fn mobile_pairing_primary_selection_reason(
    candidates: &[MobilePairingCandidate],
    primary: &MobilePairingCandidate,
) -> &'static str {
    if primary.transport_mode == "cloudflare_tunnel" {
        "verified_quick_tunnel"
    } else if primary.transport_mode == "public_tunnel" {
        if candidates
            .iter()
            .any(mobile_pairing_candidate_is_ready_relay)
        {
            "public_tunnel_precedence_over_ready_relay"
        } else {
            "usable_public_tunnel"
        }
    } else if mobile_pairing_candidate_is_ready_relay(primary) {
        "ready_relay"
    } else if !primary.disabled {
        "first_enabled_candidate"
    } else {
        "first_candidate_fallback"
    }
}

fn mobile_pairing_candidate_diagnostic_summary(candidates: &[MobilePairingCandidate]) -> String {
    candidates
        .iter()
        .map(|candidate| {
            format!(
                "{}:health={},disabled={},relay_token={},relay_device={}",
                candidate.transport_mode,
                candidate.health,
                candidate.disabled,
                candidate.relay_pairing_token.is_some(),
                candidate.relay_device_id.is_some()
            )
        })
        .collect::<Vec<_>>()
        .join("|")
}

fn select_mobile_pairing_primary_candidate(
    candidates: &[MobilePairingCandidate],
) -> Option<MobilePairingCandidate> {
    let primary = candidates
        .iter()
        .find(|candidate| mobile_pairing_candidate_is_usable_public_tunnel(candidate))
        .or_else(|| candidates.iter().find(|candidate| !candidate.disabled))
        .or_else(|| candidates.first())
        .cloned();
    if let Some(primary) = primary.as_ref() {
        let ready_relay_count = candidates
            .iter()
            .filter(|candidate| mobile_pairing_candidate_is_ready_relay(candidate))
            .count();
        let usable_public_count = candidates
            .iter()
            .filter(|candidate| mobile_pairing_candidate_is_usable_public_tunnel(candidate))
            .count();
        log::info!(
            "[Bridge][MobilePairing] primary_selection reason={} primary={} ready_relay_count={} usable_public_count={} candidates={}",
            mobile_pairing_primary_selection_reason(candidates, primary),
            primary.transport_mode,
            ready_relay_count,
            usable_public_count,
            mobile_pairing_candidate_diagnostic_summary(candidates)
        );
    }
    primary
}

fn public_bridge_candidate_ws_url(base_url: &str) -> String {
    http_url_to_ws_url(base_url)
        .map(|url| format!("{}/ws", url.trim_end_matches('/')))
        .unwrap_or_else(|| format!("{}/ws", base_url.trim_end_matches('/')))
}

async fn configured_relay_pairing_candidate() -> Option<MobilePairingCandidate> {
    let config = match crate::relay::relay_mobile_pairing_config_from_mac_client_for_qr().await {
        Ok(config) => config,
        Err(error) => {
            log::warn!(
                "[Bridge][MobilePairing] relay_config_unavailable error={}",
                error
            );
            return None;
        }
    }?;

    Some(mobile_pairing_candidate_from_relay_config(config))
}

fn configured_relay_pairing_candidate_without_issue() -> Option<MobilePairingCandidate> {
    match crate::relay::relay_mobile_pairing_config_from_mac_client() {
        Ok(Some(config)) => Some(mobile_pairing_candidate_from_relay_config(config)),
        Ok(None) => None,
        Err(error) => {
            log::warn!(
                "[Bridge][MobilePairing] relay_config_unavailable error={}",
                error
            );
            None
        }
    }
}

fn fixed_public_pairing_candidate_from_probe(
    public_base_url: &str,
    public_http_ok: bool,
    public_ws_ready: bool,
    root_tunnel_authoritative_up: bool,
    endpoint_proved: bool,
    public_ws_auth_required: bool,
) -> Option<MobilePairingCandidate> {
    if !((public_http_ok && public_ws_ready) || root_tunnel_authoritative_up) || !endpoint_proved {
        return None;
    }

    let candidate = MobilePairingCandidate {
        transport_mode: "public_tunnel".to_string(),
        base_url: public_base_url.trim_end_matches('/').to_string(),
        ws_url: public_bridge_candidate_ws_url(public_base_url),
        relay_device_id: None,
        relay_pairing_token: None,
        health: "healthy".to_string(),
        disabled: false,
        warning: public_ws_auth_required
            .then(|| "公网通道当前健康，WebSocket 已启用移动端鉴权保护。".to_string()),
    };

    mobile_pairing_candidate_is_secure_for_qr(&candidate).then_some(candidate)
}

async fn healthy_fixed_public_pairing_result() -> Option<PairingCandidatesResult> {
    let public_base_url = public_bridge_base_url();
    if public_base_url.is_empty() {
        return None;
    }
    let ((_, public_http_ok, public_ws_probe, public_ws_ok), root_tunnel, endpoint_binding) = tokio::join!(
        get_public_probe_snapshot(),
        inspect_root_tunnel_runtime(),
        crate::tunnel::manager::proven_binding_for_public_endpoint(&public_base_url),
    );
    let public_ws_auth_required = websocket_probe_auth_required(&public_ws_probe);
    let public_ws_ready = websocket_probe_ok_or_auth_required(public_ws_ok, &public_ws_probe, true);
    let root_tunnel_authoritative_up = root_tunnel_is_authoritative_up(&root_tunnel);

    let public_candidate = fixed_public_pairing_candidate_from_probe(
        &public_base_url,
        public_http_ok,
        public_ws_ready,
        root_tunnel_authoritative_up,
        endpoint_binding.is_some(),
        public_ws_auth_required,
    )?;

    Some(PairingCandidatesResult {
        candidates: vec![public_candidate.clone()],
        primary: public_candidate,
        tailscale_source: None,
        public_endpoint_binding: endpoint_binding,
    })
}

async fn has_healthy_supported_route_without_quick(_bridge_port: u16) -> bool {
    if healthy_fixed_public_pairing_result().await.is_some() {
        return true;
    }

    if configured_relay_pairing_candidate_without_issue()
        .as_ref()
        .is_some_and(mobile_pairing_candidate_is_ready_relay)
    {
        return true;
    }

    false
}

async fn build_pairing_candidates(bridge_port: u16) -> PairingCandidatesResult {
    let started_at = std::time::Instant::now();
    log::info!(
        "[Bridge][MobilePairing] candidates_start bridge_port={}",
        bridge_port
    );

    // The user's proven fixed public endpoint is authoritative. Returning it
    // here avoids blocking normal pairing on optional Tailscale discovery,
    // Relay token issuance, or Quick Tunnel state. Those fallbacks are only
    // evaluated when the fixed endpoint is unavailable.
    if let Some(result) = healthy_fixed_public_pairing_result().await {
        log::info!(
            "[Bridge][MobilePairing] candidates_done primary=public_tunnel candidate_count=1 reason=healthy_fixed_public total_elapsed_ms={}",
            started_at.elapsed().as_millis()
        );
        return result;
    }

    let tunnel_status_started_at = std::time::Instant::now();
    let tunnel_status = crate::tunnel::manager::get_status().await;
    log::info!(
        "[Bridge][MobilePairing] tunnel_status state={:?} has_domain={} elapsed_ms={}",
        tunnel_status.state,
        tunnel_status.domain.is_some(),
        tunnel_status_started_at.elapsed().as_millis()
    );
    let quick_tunnel_base_url = if tunnel_status.state == crate::tunnel::TunnelState::Running
        && tunnel_status.verified
        && tunnel_status.enabled
    {
        tunnel_status
            .domain
            .as_deref()
            .map(str::trim)
            .filter(|value| value.starts_with("https://"))
            .map(|value| value.trim_end_matches('/').to_string())
    } else {
        None
    };

    let mut candidates = Vec::new();
    let mut tailscale_source = None;

    let tailscale_started_at = std::time::Instant::now();
    if let Some((tailscale_ip, detected_tailscale_source)) =
        detect_tailscale_ipv4_with_source().await
    {
        let base_url = format!("http://{}:{}", tailscale_ip, bridge_port);
        let health_url = format!("{}/api/version", base_url);
        let health_started_at = std::time::Instant::now();
        let health_ok = health_endpoint_success(&health_url).await;
        log::info!(
            "[Bridge][MobilePairing] tailscale_health source={} ok={} elapsed_ms={}",
            detected_tailscale_source,
            health_ok,
            health_started_at.elapsed().as_millis()
        );
        if health_ok {
            tailscale_source = Some(detected_tailscale_source);
            candidates.push(MobilePairingCandidate {
                transport_mode: "tailscale".to_string(),
                base_url,
                ws_url: format!("ws://{}:{}/ws", tailscale_ip, bridge_port),
                relay_device_id: None,
                relay_pairing_token: None,
                health: "healthy".to_string(),
                disabled: false,
                warning: None,
            });
        }
    }
    log::info!(
        "[Bridge][MobilePairing] tailscale_stage candidate_count={} elapsed_ms={}",
        candidates.len(),
        tailscale_started_at.elapsed().as_millis()
    );

    if let Some(relay_candidate) = configured_relay_pairing_candidate().await {
        log::info!(
            "[Bridge][MobilePairing] relay_stage health={} disabled={}",
            relay_candidate.health,
            relay_candidate.disabled
        );
        candidates.push(relay_candidate);
    }

    let public_base_url = public_bridge_base_url();
    let has_configured_public_base_url = !public_base_url.is_empty();
    let public_base_url_is_overridden = public_bridge_base_url_is_overridden();
    let public_ws_url = public_bridge_candidate_ws_url(&public_base_url);
    let has_tailscale_candidate = candidates
        .iter()
        .any(|candidate| candidate.transport_mode == "tailscale");
    let public_started_at = std::time::Instant::now();
    let ((_, public_http_ok, public_ws_probe, public_ws_ok), root_tunnel, public_endpoint_binding) =
        if has_configured_public_base_url {
            tokio::join!(
                get_public_probe_snapshot(),
                inspect_root_tunnel_runtime(),
                crate::tunnel::manager::proven_binding_for_public_endpoint(&public_base_url),
            )
        } else {
            (
                (serde_json::json!({}), false, serde_json::json!({}), false),
                serde_json::json!({}),
                None,
            )
        };
    let public_ws_auth_required = websocket_probe_auth_required(&public_ws_probe);
    let public_ws_ready = websocket_probe_ok_or_auth_required(public_ws_ok, &public_ws_probe, true);
    let root_tunnel_authoritative_up = root_tunnel_is_authoritative_up(&root_tunnel);
    let public_transport_ready = has_configured_public_base_url
        && ((public_http_ok && public_ws_ready)
            || root_tunnel_authoritative_up
            || public_base_url_is_overridden);
    let public_endpoint_proved = public_transport_ready && public_endpoint_binding.is_some();
    let public_ready = public_transport_ready && public_endpoint_proved;
    let public_ws_error = probe_error_summary(&public_ws_probe);
    log::info!(
        "[Bridge][MobilePairing] public_stage http_ok={} ws_ready={} ws_auth_required={} root_authoritative_up={} endpoint_proved={} ready={} elapsed_ms={}",
        public_http_ok,
        public_ws_ready,
        public_ws_auth_required,
        root_tunnel_authoritative_up,
        public_endpoint_proved,
        public_ready,
        public_started_at.elapsed().as_millis()
    );
    let public_warning = if public_transport_ready && !public_endpoint_proved {
        Some("公网通道未能证明端点属于这台 Mac，不会用于生成二维码。".to_string())
    } else if public_ready {
        if public_base_url_is_overridden && !(public_http_ok && public_ws_ready) {
            Some(
                "公网通道使用手动覆盖地址，本机探针可能受发卡弯或代理 DNS 影响；已按配置作为首选。"
                    .to_string(),
            )
        } else if root_tunnel_authoritative_up && !public_http_ok {
            Some(
                "公网通道由 root tunnel HA 满连接确认可用，当前本机发卡弯探针仅作参考。"
                    .to_string(),
            )
        } else if public_ws_auth_required && has_tailscale_candidate {
            Some("公网通道当前健康且已启用移动端鉴权保护，Tailscale 保留为备用候选。".to_string())
        } else if public_ws_auth_required {
            Some("公网通道当前健康，WebSocket 已启用移动端鉴权保护。".to_string())
        } else if has_tailscale_candidate {
            Some("公网通道当前健康，Tailscale 保留为备用候选。".to_string())
        } else {
            None
        }
    } else {
        let ws_error_detail = public_ws_error
            .as_deref()
            .map(|error| format!("，WebSocket 错误={error}"))
            .unwrap_or_default();
        Some(format!(
            "公网通道当前降级：HTTP 健康={}，WebSocket 健康={}{}，不作为普通 fallback。",
            public_http_ok, public_ws_ready, ws_error_detail
        ))
    };
    let public_candidate = MobilePairingCandidate {
        transport_mode: "public_tunnel".to_string(),
        base_url: public_base_url,
        ws_url: public_ws_url,
        relay_device_id: None,
        relay_pairing_token: None,
        health: if public_ready { "healthy" } else { "degraded" }.to_string(),
        disabled: !public_ready,
        warning: public_warning,
    };

    if public_ready {
        if public_base_url_is_overridden {
            candidates.insert(0, public_candidate.clone());
        } else {
            candidates.push(public_candidate.clone());
        }
    }

    let quick_tunnel_added = quick_tunnel_base_url.is_some();
    if let Some(quick_tunnel_base_url) = quick_tunnel_base_url {
        let ws_host = quick_tunnel_base_url
            .trim_start_matches("https://")
            .to_string();
        candidates.push(MobilePairingCandidate {
            transport_mode: "cloudflare_tunnel".to_string(),
            base_url: quick_tunnel_base_url,
            ws_url: format!("wss://{}/ws", ws_host),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "healthy".to_string(),
            disabled: false,
            warning: None,
        });
    }

    let lan_started_at = std::time::Instant::now();
    if let Some(lan_ip) = detect_lan_ipv4() {
        let has_tailscale_candidate = candidates
            .iter()
            .any(|candidate| candidate.transport_mode == "tailscale");
        let warning = if has_tailscale_candidate {
            "局域网备用地址，仅适合同一 Wi-Fi/热点实验；跨网请优先使用 Tailscale 或公网通道。"
        } else {
            "未检测到 Tailscale，当前返回的是局域网地址，仅适合同一 Wi-Fi 实验。"
        };
        candidates.push(MobilePairingCandidate {
            transport_mode: "lan_fallback".to_string(),
            base_url: format!("http://{}:{}", lan_ip, bridge_port),
            ws_url: format!("ws://{}:{}/ws", lan_ip, bridge_port),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "fallback".to_string(),
            disabled: false,
            warning: Some(warning.to_string()),
        });
    }
    log::info!(
        "[Bridge][MobilePairing] lan_stage elapsed_ms={} candidate_count={}",
        lan_started_at.elapsed().as_millis(),
        candidates.len()
    );

    if has_configured_public_base_url && !public_ready {
        candidates.push(public_candidate);
    }

    if candidates.is_empty() {
        candidates.push(MobilePairingCandidate {
            transport_mode: "loopback_fallback".to_string(),
            base_url: format!("http://127.0.0.1:{}", bridge_port),
            ws_url: format!("ws://127.0.0.1:{}/ws", bridge_port),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "fallback".to_string(),
            disabled: false,
            warning: Some(
                "未检测到 Tailscale 或局域网地址，当前返回的是回环地址，仅适合本机调试。"
                    .to_string(),
            ),
        });
    }

    let primary = select_mobile_pairing_primary_candidate(&candidates)
        .expect("mobile pairing candidates must not be empty");

    log::info!(
        "[Bridge][MobilePairing] candidates_done primary={} candidate_count={} quick_tunnel_added={} total_elapsed_ms={}",
        primary.transport_mode,
        candidates.len(),
        quick_tunnel_added,
        started_at.elapsed().as_millis()
    );

    PairingCandidatesResult {
        candidates,
        primary,
        tailscale_source,
        public_endpoint_binding,
    }
}

fn fallback_pairing_candidates(bridge_port: u16, warning: String) -> PairingCandidatesResult {
    fallback_pairing_candidates_with_public_base_url(bridge_port, warning, public_bridge_base_url())
}

fn fallback_pairing_candidates_with_public_base_url(
    bridge_port: u16,
    warning: String,
    public_base_url: String,
) -> PairingCandidatesResult {
    let public_ws_url = public_bridge_candidate_ws_url(&public_base_url);
    let mut candidates = Vec::new();
    if let Some(relay_candidate) = configured_relay_pairing_candidate_without_issue() {
        candidates.push(relay_candidate);
    }
    if !public_base_url.is_empty() {
        candidates.push(MobilePairingCandidate {
            transport_mode: "public_tunnel".to_string(),
            base_url: public_base_url,
            ws_url: public_ws_url,
            relay_device_id: None,
            relay_pairing_token: None,
            health: "degraded".to_string(),
            disabled: true,
            warning: Some(warning.clone()),
        });
    }

    if let Some(lan_ip) = detect_lan_ipv4() {
        candidates.push(MobilePairingCandidate {
            transport_mode: "lan_fallback".to_string(),
            base_url: format!("http://{}:{}", lan_ip, bridge_port),
            ws_url: format!("ws://{}:{}/ws", lan_ip, bridge_port),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "fallback".to_string(),
            disabled: false,
            warning: Some(
                "配对候选探测超时，临时返回局域网备用地址；跨网请优先等待 Tailscale 候选恢复。"
                    .to_string(),
            ),
        });
    }

    candidates.push(MobilePairingCandidate {
        transport_mode: "loopback_fallback".to_string(),
        base_url: format!("http://127.0.0.1:{}", bridge_port),
        ws_url: format!("ws://127.0.0.1:{}/ws", bridge_port),
        relay_device_id: None,
        relay_pairing_token: None,
        health: "fallback".to_string(),
        disabled: false,
        warning: Some("配对候选探测超时，回环地址仅适合本机调试。".to_string()),
    });

    let primary = select_mobile_pairing_primary_candidate(&candidates)
        .expect("fallback pairing candidates must not be empty");

    PairingCandidatesResult {
        candidates,
        primary,
        tailscale_source: None,
        public_endpoint_binding: None,
    }
}

async fn build_pairing_candidates_bounded(
    bridge_port: u16,
    context: &str,
) -> PairingCandidatesResult {
    match tokio::time::timeout(
        std::time::Duration::from_secs(MOBILE_PAIRING_CANDIDATES_TIMEOUT_SECS),
        build_pairing_candidates(bridge_port),
    )
    .await
    {
        Ok(result) => result,
        Err(_) => {
            let warning = format!(
                "配对候选探测超过 {} 秒，已返回降级候选；请稍后刷新以重新检测 Tailscale。",
                MOBILE_PAIRING_CANDIDATES_TIMEOUT_SECS
            );
            log::warn!(
                "[Bridge][MobilePairing] candidates_timeout context={} bridge_port={} timeout_secs={}",
                context,
                bridge_port,
                MOBILE_PAIRING_CANDIDATES_TIMEOUT_SECS
            );
            fallback_pairing_candidates(bridge_port, warning)
        }
    }
}

async fn build_mobile_pairing_payload(
    bridge_port: u16,
    require_secure_route: bool,
) -> Result<MobilePairingPayload, String> {
    let device_name = resolve_host_label();
    let device_id = resolve_mobile_device_id();
    let issued_at = chrono::Utc::now();
    let expires_at = issued_at + chrono::Duration::minutes(10);
    let pairing_session_id = generate_bridge_token("ps");
    let pairing_token = generate_bridge_token("pt");

    let mut result = build_pairing_candidates_bounded(bridge_port, "issue").await;
    let mut endpoint_binding = match result.primary.transport_mode.as_str() {
        "cloudflare_tunnel" => crate::tunnel::manager::pairing_binding().await,
        "public_tunnel" => result.public_endpoint_binding.clone(),
        _ => None,
    };
    if result.primary.transport_mode == "cloudflare_tunnel" && endpoint_binding.is_none() {
        for candidate in &mut result.candidates {
            if candidate.transport_mode == "cloudflare_tunnel" {
                candidate.disabled = true;
                candidate.health = "degraded".to_string();
                candidate.warning = Some("Quick Tunnel 端点证明已失效，请重新检测。".to_string());
            }
        }
        result.primary = select_mobile_pairing_primary_candidate(&result.candidates)
            .expect("mobile pairing candidates must not be empty after quick proof invalidation");
        endpoint_binding = (result.primary.transport_mode == "public_tunnel")
            .then(|| result.public_endpoint_binding.clone())
            .flatten();
    }
    let formal_route = crate::tunnel::commands::configured_formal_mobile_route();
    let selected_formal_route = formal_route.as_ref().filter(|route| {
        result.primary.transport_mode == "public_tunnel"
            && route.base_url.trim_end_matches('/') == result.primary.base_url.trim_end_matches('/')
    });
    if require_secure_route {
        if selected_formal_route.is_none() && !quick_tunnel_test_capability_enabled() {
            return Err(if formal_route.is_some() {
                "formal_route_unhealthy".to_string()
            } else {
                "formal_route_not_configured".to_string()
            });
        }
        if !mobile_pairing_candidate_is_secure_for_qr(&result.primary) {
            return Err("endpoint_proof_failed".to_string());
        }
        if !mobile_pairing_candidate_has_endpoint_proof(&result.primary, endpoint_binding.as_ref())
        {
            return Err("endpoint_proof_failed".to_string());
        }
    }
    log::info!(
        "[Bridge][MobilePairing] issue_endpoint_proof transport={} reused_from_candidate_build={}",
        result.primary.transport_mode,
        result.primary.transport_mode == "public_tunnel" && endpoint_binding.is_some()
    );
    let _claim_guard = MOBILE_PAIRING_CLAIM_LOCK.lock().await;
    let transport_mode = result.primary.transport_mode.clone();
    let formal_route_generation =
        selected_formal_route.map(|route| route.formal_route_generation.max(1));

    {
        let mut tokens = MOBILE_PAIRING_TOKENS.write().await;
        tokens.retain(|_, info| info.expires_at > chrono::Utc::now().to_rfc3339());
        let mut sessions = MOBILE_PAIRING_SESSIONS.write().await;
        let now = chrono::Utc::now();
        sessions.retain(|_, info| {
            parse_rfc3339(&info.expires_at)
                .map(|expires_at| {
                    expires_at + chrono::Duration::seconds(MOBILE_PAIRING_SESSION_RETENTION_SECS)
                        > now
                })
                .unwrap_or(false)
        });
        let mut receipts = MOBILE_PAIRING_CLAIM_RECEIPTS.write().await;
        receipts.retain(|_, receipt| receipt.expires_at > chrono::Utc::now().to_rfc3339());
        let token_info = PairingTokenInfo {
            session_id: pairing_session_id.clone(),
            issued_at: issued_at.to_rfc3339(),
            expires_at: expires_at.to_rfc3339(),
            state: "pending".to_string(),
            failure_count: 0,
            first_failed_at: None,
            transport_mode,
            formal_route_generation,
            endpoint_binding,
        };
        tokens.insert(pairing_token.clone(), token_info.clone());
        sessions.insert(pairing_session_id.clone(), token_info);

        // Keep the previously rendered QR valid until the caller has actually
        // received a replacement. There is no response ACK on this endpoint,
        // so eagerly clearing the old token makes a lost refresh response turn
        // the still-visible QR into a dead code. Bound the overlap and protect
        // the token being returned by this response from pruning.
        if tokens.len() > MOBILE_PAIRING_MAX_PENDING_TOKENS {
            let mut removable = tokens
                .iter()
                .filter(|(token, _)| token.as_str() != pairing_token)
                .map(|(token, info)| {
                    (
                        token.clone(),
                        info.session_id.clone(),
                        info.issued_at.clone(),
                    )
                })
                .collect::<Vec<_>>();
            removable
                .sort_by(|left, right| left.2.cmp(&right.2).then_with(|| left.0.cmp(&right.0)));
            let overflow = tokens.len() - MOBILE_PAIRING_MAX_PENDING_TOKENS;
            for (token, session_id, _) in removable.into_iter().take(overflow) {
                tokens.remove(&token);
                if let Some(session) = sessions.get_mut(&session_id) {
                    session.state = "expired".to_string();
                }
            }
        }
    }

    Ok(MobilePairingPayload {
        version: 2,
        pairing_session_id,
        device_id,
        device_name,
        transport_mode: result.primary.transport_mode,
        base_url: result.primary.base_url,
        ws_url: result.primary.ws_url,
        relay_device_id: result.primary.relay_device_id,
        relay_pairing_token: result.primary.relay_pairing_token,
        candidates: result.candidates,
        pairing_token,
        issued_at: issued_at.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
        warning: result.primary.warning,
    })
}

pub async fn invalidate_quick_tunnel_pairing_tokens() {
    let _claim_guard = MOBILE_PAIRING_CLAIM_LOCK.lock().await;
    let invalidated_sessions = {
        let mut tokens = MOBILE_PAIRING_TOKENS.write().await;
        let session_ids = tokens
            .values()
            .filter(|token| token.transport_mode == "cloudflare_tunnel")
            .map(|token| token.session_id.clone())
            .collect::<std::collections::HashSet<_>>();
        tokens.retain(|_, token| token.transport_mode != "cloudflare_tunnel");
        session_ids
    };
    if invalidated_sessions.is_empty() {
        return;
    }
    let mut sessions = MOBILE_PAIRING_SESSIONS.write().await;
    for session_id in invalidated_sessions {
        if let Some(session) = sessions.get_mut(&session_id) {
            session.state = "expired".to_string();
        }
    }
}

fn default_web_login_scopes() -> Vec<String> {
    vec![
        SCOPE_STATUS_READ.to_string(),
        SCOPE_SESSION_READ.to_string(),
        SCOPE_SESSION_RESPOND.to_string(),
    ]
}

pub(crate) fn normalize_web_origin(value: &str) -> Result<String, String> {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err("cf_origin_missing".to_string());
    }
    let parsed = reqwest::Url::parse(trimmed).map_err(|_| "cf_origin_invalid".to_string())?;
    if parsed.scheme() != "https" {
        return Err("cf_origin_must_be_https".to_string());
    }
    if parsed.host_str().is_none() {
        return Err("cf_origin_invalid".to_string());
    }
    if parsed.path() != "/" || parsed.query().is_some() || parsed.fragment().is_some() {
        return Err("cf_origin_origin_only".to_string());
    }
    Ok(trimmed.to_string())
}

fn normalize_requested_web_scopes(requested: &[String]) -> Result<Vec<String>, String> {
    let allowed = default_web_login_scopes();
    if requested.is_empty() {
        return Ok(allowed);
    }

    let mut scopes = Vec::new();
    for scope in requested {
        let normalized = scope.trim();
        if normalized.is_empty() {
            continue;
        }
        if !allowed
            .iter()
            .any(|allowed_scope| allowed_scope == normalized)
        {
            return Err("invalid_scope".to_string());
        }
        if !scopes.iter().any(|existing| existing == normalized) {
            scopes.push(normalized.to_string());
        }
    }

    if scopes.is_empty() {
        Ok(allowed)
    } else {
        Ok(scopes)
    }
}

fn prune_web_login_pairing_nonces(
    nonces: &mut HashMap<String, WebLoginPairingNonce>,
    now: chrono::DateTime<chrono::Utc>,
) {
    nonces.retain(|_, info| {
        parse_rfc3339(&info.expires_at)
            .map(|expires_at| expires_at > now)
            .unwrap_or(false)
    });
    if nonces.len() > WEB_LOGIN_MAX_PAIRING_NONCES {
        let mut oldest: Vec<(String, String)> = nonces
            .iter()
            .map(|(nonce, info)| (nonce.clone(), info.issued_at.clone()))
            .collect();
        oldest.sort_by(|a, b| a.1.cmp(&b.1));
        for (nonce, _) in oldest
            .into_iter()
            .take(nonces.len().saturating_sub(WEB_LOGIN_MAX_PAIRING_NONCES))
        {
            nonces.remove(&nonce);
        }
    }
}

fn prune_web_login_sessions(
    sessions: &mut HashMap<String, WebLoginSession>,
    now: chrono::DateTime<chrono::Utc>,
) {
    sessions.retain(|_, session| {
        session.revoked_at.is_none()
            && parse_rfc3339(&session.expires_at)
                .map(|expires_at| expires_at > now)
                .unwrap_or(false)
    });
    if sessions.len() > WEB_LOGIN_MAX_SESSIONS {
        let mut oldest: Vec<(String, String)> = sessions
            .iter()
            .map(|(token_hash, session)| (token_hash.clone(), session.issued_at.clone()))
            .collect();
        oldest.sort_by(|a, b| a.1.cmp(&b.1));
        for (token_hash, _) in oldest
            .into_iter()
            .take(sessions.len().saturating_sub(WEB_LOGIN_MAX_SESSIONS))
        {
            sessions.remove(&token_hash);
        }
    }
}

fn build_web_pair_url(issue: &WebLoginPairingIssueResponse) -> String {
    let mut url =
        reqwest::Url::parse(&issue.console_origin).expect("validated web origin is valid");
    url.set_path("/pair");
    url.set_query(None);
    url.set_fragment(None);
    url.query_pairs_mut()
        .append_pair("device_id", &issue.device_id)
        .append_pair("cf_origin", &issue.cf_origin)
        .append_pair("nonce", &issue.nonce);
    url.to_string()
}

fn web_login_session_summary(session: &WebLoginSession) -> WebLoginSessionSummary {
    WebLoginSessionSummary {
        session_id: session.session_id.clone(),
        device_id: session.device_id.clone(),
        cf_origin: session.cf_origin.clone(),
        console_origin: session.console_origin.clone(),
        scopes: session.scopes.clone(),
        issued_at: session.issued_at.clone(),
        expires_at: session.expires_at.clone(),
        last_seen_at: session.last_seen_at.clone(),
    }
}

pub async fn list_web_login_sessions() -> Vec<WebLoginSessionSummary> {
    let now = chrono::Utc::now();
    let mut sessions = WEB_LOGIN_SESSIONS.write().await;
    prune_web_login_sessions(&mut sessions, now);
    let mut summaries: Vec<_> = sessions.values().map(web_login_session_summary).collect();
    summaries.sort_by(|a, b| b.last_seen_at.cmp(&a.last_seen_at));
    summaries
}

pub async fn revoke_all_web_login_sessions() -> usize {
    let mut sessions = WEB_LOGIN_SESSIONS.write().await;
    let revoked = sessions.len();
    sessions.clear();
    revoked
}

pub async fn issue_cloudflare_web_login_pairing(
    cf_origin: String,
    console_origin: String,
) -> Result<WebLoginPairingIssueResponse, String> {
    let cf_origin = normalize_web_origin(&cf_origin)?;
    let console_origin = normalize_web_origin(&console_origin)?;
    let issued_at = chrono::Utc::now();
    let expires_at = issued_at + chrono::Duration::seconds(WEB_LOGIN_PAIRING_TTL_SECS);
    let nonce = generate_bridge_token("wp");
    let device_id = resolve_mobile_device_id();
    let scopes = default_web_login_scopes();

    let mut issue = WebLoginPairingIssueResponse {
        ok: true,
        device_id: device_id.clone(),
        cf_origin: cf_origin.clone(),
        console_origin: console_origin.clone(),
        pair_url: String::new(),
        nonce: nonce.clone(),
        scopes: scopes.clone(),
        issued_at: issued_at.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };
    issue.pair_url = build_web_pair_url(&issue);

    {
        let mut nonces = WEB_LOGIN_PAIRING_NONCES.write().await;
        prune_web_login_pairing_nonces(&mut nonces, issued_at);
        nonces.insert(
            nonce,
            WebLoginPairingNonce {
                device_id,
                cf_origin,
                console_origin,
                scopes,
                issued_at: issue.issued_at.clone(),
                expires_at: issue.expires_at.clone(),
            },
        );
    }

    Ok(issue)
}

async fn authenticate_web_login_session_token(
    token: &str,
    requested_device_id: Option<&String>,
) -> Option<AuthPrincipal> {
    authenticate_web_login_session_token_with_origins(token, requested_device_id)
        .await
        .map(|(principal, _)| principal)
}

async fn authenticate_web_login_session_token_with_origins(
    token: &str,
    requested_device_id: Option<&String>,
) -> Option<(AuthPrincipal, Vec<String>)> {
    let token_hash = bridge_token_hash(token);
    let now = chrono::Utc::now();
    let now_string = now.to_rfc3339();
    let mut sessions = WEB_LOGIN_SESSIONS.write().await;
    prune_web_login_sessions(&mut sessions, now);
    let session = sessions.get_mut(&token_hash)?;
    if requested_device_id.is_some_and(|value| value != &session.device_id) {
        return None;
    }
    session.last_seen_at = now_string;
    let mut allowed_browser_origins =
        vec![session.cf_origin.clone(), session.console_origin.clone()];
    allowed_browser_origins.sort();
    allowed_browser_origins.dedup();
    Some((
        AuthPrincipal {
            principal_id: format!("web:{}", session.session_id),
            device_id: session.device_id.clone(),
            client_kind: "web".to_string(),
            scopes: session.scopes.clone(),
        },
        allowed_browser_origins,
    ))
}

async fn mark_apns_notification_for_send(
    request_id: Option<&str>,
    project_path: Option<&str>,
    body: &str,
    source: &str,
) -> bool {
    let now = std::time::Instant::now();
    let key = apns_dedupe_key(request_id, project_path, body);
    let mut sent = APNS_NOTIFICATION_DEDUPE.write().await;
    sent.retain(|dedupe_key, last_sent| {
        now.duration_since(*last_sent).as_secs() < apns_dedupe_ttl_secs(dedupe_key)
    });

    if sent.contains_key(&key) {
        bridge_debug_log(&format!(
            "[APNs Timing] skipped duplicate: key={}, request_id={:?}, project_path={:?}, source={}",
            key, request_id, project_path, source
        ));
        return false;
    }

    sent.insert(key.clone(), now);
    bridge_debug_log(&format!(
        "[APNs Timing] accepted notification: key={}, request_id={:?}, project_path={:?}, source={}",
        key, request_id, project_path, source
    ));
    true
}

async fn send_apns_notification_once(
    title: &str,
    body: &str,
    project_path: Option<String>,
    request_id: Option<String>,
    source: &str,
) {
    if !mark_apns_notification_for_send(
        request_id.as_deref(),
        project_path.as_deref(),
        body,
        source,
    )
    .await
    {
        return;
    }

    record_last_notification_route(request_id.as_deref(), project_path.as_deref(), source).await;
    send_apns_notification(title, body, project_path, request_id).await;
}

async fn send_web_push_for_bridge_message(message: BridgeMessage) {
    let Some(body) = extract_notification_body(&message.payload) else {
        return;
    };
    send_web_push_notification("iterate", &body).await;
}

async fn send_web_push_notification(title: &str, body: &str) {
    let Some(private_key) = VAPID_CONFIG.private_key.as_ref() else {
        return;
    };

    let subscriptions = PUSH_SUBSCRIPTIONS.read().await;
    if subscriptions.is_empty() {
        return;
    }

    let payload = serde_json::json!({ "title": title, "body": body }).to_string();
    let client = match reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(client) => client,
        Err(err) => {
            log_important!(warn, "[Bridge] Web Push HTTP 客户端创建失败: {}", err);
            return;
        }
    };

    for subscription in subscriptions.values() {
        let request = match build_web_push_request(
            subscription,
            private_key,
            VAPID_CONFIG.subject.as_deref(),
            payload.as_bytes(),
        ) {
            Ok(request) => request,
            Err(err) => {
                log_important!(warn, "[Bridge] Web Push 消息构建失败: {}", err);
                continue;
            }
        };

        if let Err(err) = send_web_push_request(&client, request).await {
            log_important!(warn, "[Bridge] Web Push 发送失败: {}", err);
        }
    }
}

fn build_web_push_request(
    subscription: &WebPushSubscriptionInfo,
    private_key: &str,
    subject: Option<&str>,
    payload: &[u8],
) -> Result<axum::http::Request<Vec<u8>>, String> {
    validate_web_push_subscription(subscription)?;
    let endpoint_url = validate_web_push_endpoint(&subscription.endpoint)?;

    let endpoint = subscription
        .endpoint
        .parse()
        .map_err(|_| "Web Push endpoint 无效".to_string())?;
    let ua_public_bytes = URL_SAFE_NO_PAD
        .decode(&subscription.keys.p256dh)
        .map_err(|_| "Web Push p256dh 无效".to_string())?;
    let ua_public = web_push_native::p256::PublicKey::from_sec1_bytes(&ua_public_bytes)
        .map_err(|_| "Web Push p256dh 无效".to_string())?;
    let ua_auth_bytes = URL_SAFE_NO_PAD
        .decode(&subscription.keys.auth)
        .map_err(|_| "Web Push auth 无效".to_string())?;
    if ua_auth_bytes.len() != 16 {
        return Err("Web Push auth 长度无效".to_string());
    }
    let ua_auth = WebPushAuth::clone_from_slice(&ua_auth_bytes);

    let mut request = WebPushBuilder::new(endpoint, ua_public, ua_auth)
        .with_valid_duration(std::time::Duration::from_secs(28 * 24 * 60 * 60))
        .build(payload.to_vec())
        .map_err(|err| format!("Web Push 加密失败: {}", err))?;
    let authorization = build_vapid_authorization(private_key, subject, &endpoint_url)?;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        authorization
            .parse()
            .map_err(|_| "VAPID Authorization header 无效".to_string())?,
    );
    Ok(request)
}

fn build_vapid_authorization(
    private_key: &str,
    subject: Option<&str>,
    endpoint: &reqwest::Url,
) -> Result<String, String> {
    let private_key_bytes = URL_SAFE_NO_PAD
        .decode(private_key)
        .map_err(|_| "VAPID 私钥不是有效的 base64url".to_string())?;
    let signing_key = SigningKey::from_slice(&private_key_bytes)
        .map_err(|_| "VAPID 私钥不是有效的 P-256 私钥".to_string())?;
    let audience = endpoint.origin().ascii_serialization();
    let expires_at = chrono::Utc::now().timestamp().saturating_add(12 * 60 * 60);
    let mut claims = serde_json::json!({
        "aud": audience,
        "exp": expires_at,
    });
    if let Some(subject) = subject {
        claims["sub"] = serde_json::Value::String(subject.to_string());
    }
    let header_part = URL_SAFE_NO_PAD.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims_part = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).map_err(|_| "VAPID claims 序列化失败".to_string())?);
    let signing_input = format!("{}.{}", header_part, claims_part);
    let signature: Signature = signing_key.sign(signing_input.as_bytes());
    let signature_part = URL_SAFE_NO_PAD.encode(signature.to_bytes());
    let token = format!("{}.{}", signing_input, signature_part);
    let public_key = URL_SAFE_NO_PAD.encode(
        signing_key
            .verifying_key()
            .to_encoded_point(false)
            .as_bytes(),
    );
    Ok(format!("vapid t={}, k={}", token, public_key))
}

async fn send_web_push_request(
    client: &reqwest::Client,
    request: axum::http::Request<Vec<u8>>,
) -> Result<(), String> {
    const MAX_RESPONSE_SIZE: usize = 64 * 1024;

    let (parts, body) = request.into_parts();
    let endpoint = parts.uri.to_string();
    let parsed_endpoint = validate_web_push_endpoint(&endpoint)?;

    let mut request_builder = client.post(parsed_endpoint).body(body);
    for (name, value) in &parts.headers {
        request_builder = request_builder.header(name.as_str(), value.as_bytes());
    }

    let response = request_builder
        .send()
        .await
        .map_err(|err| format!("Web Push 请求失败: {}", err))?;
    let status = response.status();
    let mut response_body = Vec::new();
    let mut chunks = response.bytes_stream();
    while let Some(chunk) = chunks.next().await {
        let chunk = chunk.map_err(|err| format!("Web Push 响应读取失败: {}", err))?;
        if response_body.len().saturating_add(chunk.len()) > MAX_RESPONSE_SIZE {
            return Err("Web Push 响应超过 64 KiB".to_string());
        }
        response_body.extend_from_slice(&chunk);
    }

    if status.is_success() {
        return Ok(());
    }

    let detail = String::from_utf8_lossy(&response_body);
    let detail = detail.trim();
    if detail.is_empty() {
        Err(format!("Web Push 服务返回 HTTP {}", status))
    } else {
        Err(format!("Web Push 服务返回 HTTP {}: {}", status, detail))
    }
}

#[cfg(test)]
mod web_push_security_tests {
    use super::*;

    fn valid_subscription(endpoint: &str) -> WebPushSubscriptionInfo {
        let ua_key = SigningKey::from_slice(&[9_u8; 32]).expect("valid test P-256 key");
        WebPushSubscriptionInfo::new(
            endpoint,
            &URL_SAFE_NO_PAD.encode(ua_key.verifying_key().to_encoded_point(false).as_bytes()),
            &URL_SAFE_NO_PAD.encode([3_u8; 16]),
        )
    }

    #[test]
    fn web_push_request_requires_https() {
        let subscription = valid_subscription("http://fcm.googleapis.com/send");
        let private_key = URL_SAFE_NO_PAD.encode([7_u8; 32]);

        let error = build_web_push_request(
            &subscription,
            &private_key,
            Some("mailto:security@example.test"),
            b"test",
        )
        .expect_err("HTTP Web Push endpoint must be rejected");

        assert!(error.contains("HTTPS"));
    }

    #[test]
    fn web_push_request_has_vapid_and_encrypted_payload() {
        let subscription = valid_subscription("https://fcm.googleapis.com/fcm/send/test-token");
        let private_key = URL_SAFE_NO_PAD.encode([7_u8; 32]);

        let request = build_web_push_request(
            &subscription,
            &private_key,
            Some("mailto:security@example.test"),
            b"test",
        )
        .expect("valid Web Push request");

        assert_eq!(request.uri().scheme_str(), Some("https"));
        assert_eq!(
            request
                .headers()
                .get(header::CONTENT_ENCODING)
                .and_then(|value| value.to_str().ok()),
            Some("aes128gcm")
        );
        assert!(request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.starts_with("vapid t=") && value.contains(", k=")));
        assert!(!request.body().is_empty());
    }

    #[test]
    fn web_push_endpoint_allows_only_known_https_push_services() {
        for endpoint in [
            "https://fcm.googleapis.com/fcm/send/token",
            "https://updates.push.services.mozilla.com/wpush/v2/token",
            "https://web.push.apple.com/QD-token",
            "https://wns2-by3p.notify.windows.com/?token=test",
        ] {
            assert!(
                validate_web_push_endpoint(endpoint).is_ok(),
                "expected supported endpoint: {endpoint}"
            );
        }
    }

    #[test]
    fn web_push_endpoint_rejects_private_unknown_or_credentialed_targets() {
        for endpoint in [
            "https://127.0.0.1/push",
            "https://[::1]/push",
            "https://localhost/push",
            "https://10.0.0.1/push",
            "https://push.example.test/send",
            "https://user:secret@fcm.googleapis.com/send",
            "https://fcm.googleapis.com:8443/send",
        ] {
            assert!(
                validate_web_push_endpoint(endpoint).is_err(),
                "expected rejected endpoint: {endpoint}"
            );
        }
    }

    #[test]
    fn web_push_subscription_rejects_oversized_fields() {
        let oversized_endpoint = format!(
            "https://fcm.googleapis.com/fcm/send/{}",
            "x".repeat(MAX_WEB_PUSH_ENDPOINT_LENGTH)
        );
        assert!(validate_web_push_endpoint(&oversized_endpoint).is_err());

        let mut subscription = valid_subscription("https://fcm.googleapis.com/fcm/send/test-token");
        subscription.keys.p256dh = "x".repeat(MAX_WEB_PUSH_P256DH_LENGTH + 1);
        assert!(validate_web_push_subscription(&subscription).is_err());
    }

    #[test]
    fn web_push_subscription_capacity_allows_replacement_but_bounds_growth() {
        assert!(web_push_subscription_capacity_available(
            MAX_WEB_PUSH_SUBSCRIPTIONS,
            true
        ));
        assert!(!web_push_subscription_capacity_available(
            MAX_WEB_PUSH_SUBSCRIPTIONS,
            false
        ));
        assert!(web_push_subscription_capacity_available(
            MAX_WEB_PUSH_SUBSCRIPTIONS - 1,
            false
        ));
    }
}

async fn send_apns_notification(
    title: &str,
    body: &str,
    project_path: Option<String>,
    request_id: Option<String>,
) {
    let apns_started_at = std::time::Instant::now();
    let Some(config) = APNS_CONFIG.as_ref() else {
        bridge_debug_log("APNs 跳过: APNS_CONFIG 为 None（未配置）");
        return;
    };

    let tokens_snapshot = apns_device_tokens_snapshot().await;
    if tokens_snapshot.is_empty() {
        bridge_debug_log("APNs 跳过: 没有已注册的 device token");
        return;
    }
    bridge_debug_log(&format!(
        "[APNs Timing] send begin: request_id={:?}, project_path={:?}, tokens={}, body_len={}",
        request_id,
        project_path,
        tokens_snapshot.len(),
        body.len()
    ));

    let bearer = match build_apns_bearer_token(config) {
        Ok(token) => token,
        Err(err) => {
            log_important!(warn, "[APNs] {}", err);
            return;
        }
    };

    let body = trim_notification_body(body, 140);
    let sent_at = chrono::Utc::now();
    let expires_at = sent_at + chrono::Duration::seconds(APNS_NOTIFICATION_EXPIRATION_SECS);
    let sent_at_rfc3339 = sent_at.to_rfc3339();
    let expires_at_rfc3339 = expires_at.to_rfc3339();
    let apns_expiration = expires_at.timestamp().to_string();
    let apns_collapse_id = apns_collapse_id(request_id.as_deref(), project_path.as_deref(), &body);

    let mut invalid_tokens = Vec::new();

    for (record_index, (device_token, device_info)) in tokens_snapshot.into_iter().enumerate() {
        let record_number = record_index + 1;
        let environment = persisted_apns_environment(&device_info.environment, config);
        let url = format!("{}/3/device/{}", apns_endpoint(environment), device_token);
        let notifications_enabled = device_info.notifications_enabled;
        let apns_push_type = if notifications_enabled {
            "alert"
        } else {
            "background"
        };
        let payload = if notifications_enabled {
            serde_json::json!({
                "aps": {
                    "alert": {
                        "title": title,
                        "body": body,
                    },
                    "sound": "default",
                    "content-available": 1,
                },
                "project_path": project_path,
                "request_id": request_id,
                "source": "iterate_bridge",
                "sent_at": sent_at_rfc3339,
                "expires_at": expires_at_rfc3339,
            })
        } else {
            serde_json::json!({
                "aps": {
                    "content-available": 1,
                },
                "project_path": project_path,
                "request_id": request_id,
                "source": "iterate_bridge",
                "sent_at": sent_at_rfc3339,
                "expires_at": expires_at_rfc3339,
            })
        };
        let response = APNS_HTTP_CLIENT
            .post(&url)
            .header("authorization", format!("bearer {}", bearer))
            .header("apns-topic", &config.topic)
            .header("apns-push-type", apns_push_type)
            .header("apns-expiration", &apns_expiration)
            .header("apns-collapse-id", &apns_collapse_id)
            .header(
                "apns-priority",
                if notifications_enabled { "10" } else { "5" },
            )
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                bridge_debug_log(&format!(
                    "[APNs Timing] send success: request_id={:?}, project_path={:?}, record={}, mode={}, environment={}, elapsed_ms={}",
                    request_id,
                    project_path,
                    record_number,
                    apns_push_type,
                    environment.as_str(),
                    apns_started_at.elapsed().as_millis()
                ));
                log::info!(
                    "[APNs] 远程通知已发送: record={}, mode={}, environment={}",
                    record_number,
                    apns_push_type,
                    environment.as_str()
                );
            }
            Ok(resp) => {
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                bridge_debug_log(&format!(
                    "[APNs Timing] send failed: request_id={:?}, project_path={:?}, status={}, record={}, mode={}, environment={}, elapsed_ms={}, body={}",
                    request_id,
                    project_path,
                    status,
                    record_number,
                    apns_push_type,
                    environment.as_str(),
                    apns_started_at.elapsed().as_millis(),
                    text
                ));
                log_important!(
                    warn,
                    "[APNs] 远程通知发送失败: status={}, record={}, mode={}, environment={}, body={}",
                    status,
                    record_number,
                    apns_push_type,
                    environment.as_str(),
                    text
                );
                if status.as_u16() == 410 || status.as_u16() == 400 {
                    invalid_tokens.push(device_token);
                }
            }
            Err(err) => {
                bridge_debug_log(&format!(
                    "[APNs Timing] request error: request_id={:?}, project_path={:?}, record={}, mode={}, environment={}, elapsed_ms={}, error={}",
                    request_id,
                    project_path,
                    record_number,
                    apns_push_type,
                    environment.as_str(),
                    apns_started_at.elapsed().as_millis(),
                    err
                ));
                log_important!(
                    warn,
                    "[APNs] 请求发送失败: record={}, mode={}, environment={}, error={}",
                    record_number,
                    apns_push_type,
                    environment.as_str(),
                    err
                );
            }
        }
    }

    if !invalid_tokens.is_empty() {
        if let Err(err) = remove_apns_device_tokens(&invalid_tokens).await {
            log_important!(warn, "[APNs] 清理失效 token 保存失败: {}", err);
        }
    }
}

pub async fn send_live_goal_live_activity_apns(live_goal: serde_json::Value, event: &str) {
    let goal_id = live_goal
        .get("goal_id")
        .or_else(|| live_goal.get("id"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);

    let Some(goal_id) = goal_id else {
        bridge_debug_log("[APNs LiveActivity] 跳过: live_goal 缺少 goal_id");
        return;
    };

    let request = ApnsLiveActivityUpdateRequest {
        activity_token: None,
        goal_id: Some(goal_id.clone()),
        activity_kind: Some(LIVE_ACTIVITY_KIND_LIVE_GOAL.to_string()),
        activity_key: Some(goal_id),
        event: Some(event.to_string()),
        title: live_goal
            .get("title")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        status: live_goal
            .get("status")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        phase: live_goal
            .get("phase")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        status_text: live_goal
            .get("status_text")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        progress_percent: live_goal
            .get("progress_percent")
            .and_then(|value| value.as_f64()),
        progress_label: live_goal
            .get("progress_label")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        requires_action: Some(false),
        elapsed_ms: live_goal.get("elapsed_ms").and_then(|value| value.as_i64()),
        started_at_ms: live_goal
            .get("started_at_ms")
            .and_then(|value| value.as_i64()),
        updated_at_ms: live_goal
            .get("updated_at_ms")
            .and_then(|value| value.as_i64()),
        project_path: live_goal
            .get("project_path")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        request_id: live_goal
            .get("request_id")
            .and_then(|value| value.as_str())
            .map(str::to_string),
        content_state: None,
    };

    // The local caller already persisted the snapshot with its real source
    // (goal_intent, zhi_call, codex_quota, etc.). Only the public APNs update
    // handler should write APNs-originated progress back into the true source.
    let stats = send_apns_live_activity_update_inner(request).await;
    if stats.sent > 0 || stats.failed > 0 {
        log::info!(
            "[APNs LiveActivity] goal update event={} sent={} failed={} matched={}",
            stats.event,
            stats.sent,
            stats.failed,
            stats.matched
        );
    } else {
        bridge_debug_log(&format!(
            "[APNs LiveActivity] goal update skipped: event={}, matched={}, message={}",
            stats.event, stats.matched, stats.message
        ));
    }
}

pub async fn send_quota_live_activity_apns(quota_snapshot: serde_json::Value, event: &str) -> bool {
    let Some(content_state) = quota_live_activity_content_state_from_snapshot(&quota_snapshot)
    else {
        bridge_debug_log("[APNs QuotaActivity] 跳过: quota snapshot 缺少 primary metric");
        return false;
    };

    let request = ApnsLiveActivityUpdateRequest {
        activity_token: None,
        goal_id: Some(QUOTA_LIVE_ACTIVITY_KEY.to_string()),
        activity_kind: Some(LIVE_ACTIVITY_KIND_QUOTA.to_string()),
        activity_key: Some(QUOTA_LIVE_ACTIVITY_KEY.to_string()),
        event: Some(event.to_string()),
        title: None,
        status: None,
        phase: None,
        status_text: None,
        progress_percent: None,
        progress_label: None,
        requires_action: None,
        elapsed_ms: None,
        started_at_ms: None,
        updated_at_ms: quota_snapshot_i64(&quota_snapshot, "updatedAtMs", "updated_at_ms"),
        project_path: None,
        request_id: None,
        content_state: Some(content_state),
    };

    let stats = send_apns_live_activity_update_inner(request).await;
    if stats.sent > 0 || stats.failed > 0 {
        log::info!(
            "[APNs QuotaActivity] update event={} sent={} failed={} matched={}",
            stats.event,
            stats.sent,
            stats.failed,
            stats.matched
        );
    } else {
        bridge_debug_log(&format!(
            "[APNs QuotaActivity] update skipped: event={}, matched={}, message={}",
            stats.event, stats.matched, stats.message
        ));
    }
    quota_live_activity_fingerprint_send_succeeded(&stats)
}

fn apns_live_activity_topic(config: &ApnsConfig) -> String {
    if config.topic.ends_with(".push-type.liveactivity") {
        config.topic.clone()
    } else {
        format!("{}.push-type.liveactivity", config.topic)
    }
}

async fn send_apns_live_activity_update_inner(
    request: ApnsLiveActivityUpdateRequest,
) -> ApnsLiveActivitySendStats {
    let event = normalized_live_activity_event(request.event.as_deref());
    let requested_activity_kind = normalized_live_activity_kind(request.activity_kind.as_deref());
    let Some(config) = APNS_CONFIG.as_ref() else {
        return ApnsLiveActivitySendStats {
            success: false,
            event,
            matched: 0,
            sent: 0,
            failed: 0,
            invalidated: 0,
            message: "APNs is not configured".to_string(),
        };
    };

    let snapshot = apns_live_activity_tokens_snapshot().await;
    let direct_token = request
        .activity_token
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let requested_goal_id = request
        .goal_id
        .as_ref()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let requested_activity_key = normalized_live_activity_key(request.activity_key.as_deref())
        .or_else(|| requested_goal_id.clone());

    let candidates: Vec<(String, ApnsLiveActivityInfo)> =
        if let Some(activity_token) = direct_token {
            let info = snapshot.get(&activity_token).cloned().unwrap_or_else(|| {
                direct_live_activity_info_from_request(&activity_token, &request)
            });
            vec![(activity_token, info)]
        } else if let Some(activity_key) = requested_activity_key.as_ref() {
            snapshot
                .into_iter()
                .filter(|(_, info)| {
                    live_activity_info_matches(info, requested_activity_kind.as_str(), activity_key)
                })
                .collect()
        } else {
            Vec::new()
        };

    if candidates.is_empty() {
        return ApnsLiveActivitySendStats {
            success: false,
            event,
            matched: 0,
            sent: 0,
            failed: 0,
            invalidated: 0,
            message: "No Live Activity token matched".to_string(),
        };
    }

    let bearer = match build_apns_bearer_token(config) {
        Ok(token) => token,
        Err(err) => {
            return ApnsLiveActivitySendStats {
                success: false,
                event,
                matched: candidates.len(),
                sent: 0,
                failed: 0,
                invalidated: 0,
                message: err,
            };
        }
    };

    let topic = apns_live_activity_topic(config);
    let now = chrono::Utc::now();
    let apns_expiration = (now + chrono::Duration::seconds(APNS_NOTIFICATION_EXPIRATION_SECS))
        .timestamp()
        .to_string();
    let content_state =
        live_activity_content_state_from_update(&request, &event, requested_activity_kind.as_str());
    let mut sent = 0usize;
    let mut failed = 0usize;
    let mut invalid_tokens = Vec::new();

    for (record_index, (activity_token, info)) in candidates.iter().enumerate() {
        let record_number = record_index + 1;
        let environment = persisted_apns_environment(&info.environment, config);
        let url = format!("{}/3/device/{}", apns_endpoint(environment), activity_token);
        let payload = serde_json::json!({
            "aps": {
                "timestamp": now.timestamp(),
                "event": event.as_str(),
                "content-state": content_state.clone(),
            },
            "goal_id": info.goal_id,
            "activity_kind": live_activity_info_kind(info),
            "activity_key": live_activity_info_key(info),
            "project_path": request.project_path.clone().or_else(|| info.project_path.clone()),
            "request_id": request.request_id.clone().or_else(|| info.request_id.clone()),
            "source": "iterate_bridge",
            "sent_at": now.to_rfc3339(),
        });

        let response = APNS_HTTP_CLIENT
            .post(&url)
            .header("authorization", format!("bearer {}", bearer))
            .header("apns-topic", &topic)
            .header("apns-push-type", "liveactivity")
            .header("apns-expiration", &apns_expiration)
            .header("apns-priority", "10")
            .header("content-type", "application/json")
            .json(&payload)
            .send()
            .await;

        match response {
            Ok(resp) if resp.status().is_success() => {
                sent += 1;
                log::info!(
                    "[APNs LiveActivity] 发送成功: goal_id={}, record={}, event={}, environment={}",
                    info.goal_id,
                    record_number,
                    event,
                    environment.as_str()
                );
                if event == "end" {
                    invalid_tokens.push(activity_token.clone());
                }
            }
            Ok(resp) => {
                failed += 1;
                let status = resp.status();
                let text = resp.text().await.unwrap_or_default();
                log_important!(
                    warn,
                    "[APNs LiveActivity] 发送失败: status={}, goal_id={}, record={}, event={}, environment={}, body={}",
                    status,
                    info.goal_id,
                    record_number,
                    event,
                    environment.as_str(),
                    text
                );
                if status.as_u16() == 410 || status.as_u16() == 400 {
                    invalid_tokens.push(activity_token.clone());
                }
            }
            Err(err) => {
                failed += 1;
                log_important!(
                    warn,
                    "[APNs LiveActivity] 请求失败: goal_id={}, record={}, event={}, environment={}, error={}",
                    info.goal_id,
                    record_number,
                    event,
                    environment.as_str(),
                    err
                );
            }
        }
    }

    invalid_tokens.sort();
    invalid_tokens.dedup();
    if !invalid_tokens.is_empty() {
        if let Err(err) = remove_apns_live_activity_tokens(&invalid_tokens).await {
            log_important!(
                warn,
                "[APNs LiveActivity] 清理失效 activity token 保存失败: {}",
                err
            );
        }
    }

    ApnsLiveActivitySendStats {
        success: sent > 0,
        event,
        matched: candidates.len(),
        sent,
        failed,
        invalidated: invalid_tokens.len(),
        message: if sent > 0 {
            "Live Activity update sent".to_string()
        } else {
            "Live Activity update failed".to_string()
        },
    }
}

#[derive(Clone)]
struct BridgeHttpState {
    app_handle: Option<AppHandle>,
    tx: broadcast::Sender<BridgeMessage>,
    port: u16,
    desktop_codex_live: Arc<RwLock<DesktopCodexLiveState>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct DesktopCodexLiveCommand {
    action: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    project_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    microphone_muted: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
struct DesktopCodexLiveSnapshot {
    server_epoch: String,
    revision: u64,
    command: Option<DesktopCodexLiveCommand>,
    phase: String,
    status_text: String,
    active_project_path: Option<String>,
    active_thread_id: Option<String>,
    microphone_muted: bool,
    updated_at_ms: i64,
}

#[derive(Debug)]
struct DesktopCodexLiveState {
    server_epoch: String,
    revision: u64,
    command: Option<DesktopCodexLiveCommand>,
    phase: String,
    status_text: String,
    active_project_path: Option<String>,
    last_project_path: Option<String>,
    active_thread_id: Option<String>,
    microphone_muted: bool,
    updated_at_ms: i64,
    host_id: Option<String>,
    host_lease_updated_at_ms: i64,
    pending_mute_after_lifecycle: bool,
}

impl Default for DesktopCodexLiveState {
    fn default() -> Self {
        Self::with_last_project_path(None)
    }
}

impl DesktopCodexLiveState {
    fn with_last_project_path(last_project_path: Option<String>) -> Self {
        Self {
            server_epoch: uuid::Uuid::new_v4().to_string(),
            revision: 0,
            command: None,
            phase: "idle".to_string(),
            status_text: "启动全局 GPT-Live 主代理".to_string(),
            active_project_path: None,
            last_project_path,
            active_thread_id: None,
            microphone_muted: false,
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
            host_id: None,
            host_lease_updated_at_ms: 0,
            pending_mute_after_lifecycle: false,
        }
    }

    fn snapshot(&self) -> DesktopCodexLiveSnapshot {
        DesktopCodexLiveSnapshot {
            server_epoch: self.server_epoch.clone(),
            revision: self.revision,
            command: self.command.clone(),
            phase: self.phase.clone(),
            status_text: self.status_text.clone(),
            active_project_path: self.active_project_path.clone(),
            active_thread_id: self.active_thread_id.clone(),
            microphone_muted: self.microphone_muted,
            updated_at_ms: self.updated_at_ms,
        }
    }
}

#[derive(Debug, Deserialize)]
struct DesktopCodexLiveControlRequest {
    action: String,
    project_path: Option<String>,
    microphone_muted: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct DesktopCodexLiveStatusRequest {
    server_epoch: String,
    host_id: String,
    revision: u64,
    phase: String,
    status_text: String,
    active_project_path: Option<String>,
    active_thread_id: Option<String>,
    microphone_muted: bool,
}

#[derive(Debug, Deserialize)]
struct DesktopCodexLiveLeaseRequest {
    server_epoch: String,
    host_id: String,
}

#[derive(Debug, Serialize)]
struct DesktopCodexLiveLeaseResponse {
    snapshot: DesktopCodexLiveSnapshot,
    granted: bool,
}

const DESKTOP_CODEX_LIVE_HOST_LEASE_MS: i64 = 5_000;

fn desktop_codex_live_host_lease_expired(live: &DesktopCodexLiveState, now_ms: i64) -> bool {
    live.host_id.is_some()
        && now_ms.saturating_sub(live.host_lease_updated_at_ms) > DESKTOP_CODEX_LIVE_HOST_LEASE_MS
}

fn desktop_codex_live_host_lease_available(live: &DesktopCodexLiveState, now_ms: i64) -> bool {
    live.host_id.is_none() || desktop_codex_live_host_lease_expired(live, now_ms)
}

fn expire_stale_desktop_codex_live_host(live: &mut DesktopCodexLiveState, now_ms: i64) {
    if desktop_codex_live_host_lease_expired(live, now_ms)
        && matches!(
            live.phase.as_str(),
            "preparing" | "connecting" | "active" | "reconnecting"
        )
    {
        live.phase = "failed".to_string();
        live.status_text = "全局 GPT-Live 宿主已离线，点击按钮重试".to_string();
        live.active_thread_id = None;
        live.updated_at_ms = now_ms;
        live.host_id = None;
        live.host_lease_updated_at_ms = 0;
    }
}

fn valid_desktop_codex_live_phase(phase: &str) -> bool {
    matches!(
        phase,
        "idle" | "preparing" | "connecting" | "active" | "reconnecting" | "failed"
    )
}

fn normalize_desktop_codex_live_project_path(path: Option<String>) -> Option<String> {
    path.map(|value| value.trim().to_string())
        .filter(|value| value.starts_with('/') && value.len() <= 4096 && !value.contains('\0'))
}

fn desktop_codex_live_capability_required(headers: &HeaderMap) -> Option<Response> {
    (!trusted_internal_capability(headers)).then(|| {
        json_error_response(
            StatusCode::UNAUTHORIZED,
            "desktop_codex_live_capability_required",
        )
    })
}

async fn handle_desktop_codex_live_get(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) = desktop_codex_live_capability_required(&headers) {
        return response;
    }
    let mut live = state.desktop_codex_live.write().await;
    expire_stale_desktop_codex_live_host(&mut live, chrono::Utc::now().timestamp_millis());
    Json(live.snapshot()).into_response()
}

async fn handle_desktop_codex_live_control(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(request): Json<DesktopCodexLiveControlRequest>,
) -> Response {
    if let Some(response) = desktop_codex_live_capability_required(&headers) {
        return response;
    }

    let requested_action = request.action.trim().to_ascii_lowercase();
    let requested_project_path = normalize_desktop_codex_live_project_path(request.project_path);
    if !matches!(
        requested_action.as_str(),
        "start" | "stop" | "toggle" | "mute" | "short" | "interrupt"
    ) {
        return json_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_desktop_codex_live_command",
        );
    }

    let mut live = state.desktop_codex_live.write().await;
    expire_stale_desktop_codex_live_host(&mut live, chrono::Utc::now().timestamp_millis());
    if requested_action == "start"
        && matches!(
            live.phase.as_str(),
            "preparing" | "connecting" | "active" | "reconnecting"
        )
    {
        return Json(live.snapshot()).into_response();
    }
    let action = if requested_action == "toggle" {
        if matches!(
            live.phase.as_str(),
            "preparing" | "connecting" | "active" | "reconnecting"
        ) {
            "stop".to_string()
        } else {
            "start".to_string()
        }
    } else if requested_action == "short" {
        if matches!(
            live.phase.as_str(),
            "preparing" | "connecting" | "active" | "reconnecting"
        ) {
            "stop".to_string()
        } else {
            return StatusCode::NO_CONTENT.into_response();
        }
    } else {
        requested_action
    };
    let project_path = if action == "start" {
        requested_project_path.or_else(|| live.last_project_path.clone())
    } else {
        requested_project_path
    };
    if action == "start" && project_path.is_none() {
        return json_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_desktop_codex_live_command",
        );
    }
    if action == "mute"
        && !matches!(
            live.phase.as_str(),
            "preparing" | "connecting" | "active" | "reconnecting"
        )
    {
        return json_error_response(StatusCode::CONFLICT, "desktop_codex_live_not_active");
    }
    let lifecycle_pending = live
        .command
        .as_ref()
        .is_some_and(|command| matches!(command.action.as_str(), "start" | "stop"));
    if action == "mute" && lifecycle_pending {
        live.microphone_muted = request.microphone_muted.unwrap_or(!live.microphone_muted);
        live.pending_mute_after_lifecycle = true;
        return Json(live.snapshot()).into_response();
    }
    if action == "mute" {
        live.microphone_muted = request.microphone_muted.unwrap_or(!live.microphone_muted);
    }
    live.revision = live.revision.saturating_add(1);
    live.command = Some(DesktopCodexLiveCommand {
        action: action.clone(),
        project_path: project_path.clone(),
        microphone_muted: (action == "mute").then_some(live.microphone_muted),
    });
    live.updated_at_ms = chrono::Utc::now().timestamp_millis();
    if matches!(action.as_str(), "start" | "stop") {
        live.active_thread_id = None;
    }
    if action == "start" {
        live.microphone_muted = false;
        live.pending_mute_after_lifecycle = false;
        live.phase = "preparing".to_string();
        live.status_text = "正在唤醒全局 GPT-Live 主代理".to_string();
        live.active_project_path = project_path.clone();
        live.last_project_path = project_path;
    } else if action == "stop" {
        live.pending_mute_after_lifecycle = false;
        live.status_text = "正在结束全局 GPT-Live 主代理".to_string();
    } else if action == "interrupt" {
        live.status_text = "正在取消当前对话，GPT-Live 保持连接".to_string();
    }
    Json(live.snapshot()).into_response()
}

async fn handle_desktop_codex_live_lease(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(request): Json<DesktopCodexLiveLeaseRequest>,
) -> Response {
    if let Some(response) = desktop_codex_live_capability_required(&headers) {
        return response;
    }
    let host_id = request.host_id.trim();
    if request.server_epoch.len() > 128 || host_id.is_empty() || host_id.len() > 128 {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_desktop_codex_live_lease");
    }

    let mut live = state.desktop_codex_live.write().await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    expire_stale_desktop_codex_live_host(&mut live, now_ms);
    let granted = request.server_epoch == live.server_epoch
        && (live.host_id.as_deref() == Some(host_id)
            || desktop_codex_live_host_lease_available(&live, now_ms));
    if granted {
        live.host_id = Some(host_id.to_string());
        live.host_lease_updated_at_ms = now_ms;
    }
    Json(DesktopCodexLiveLeaseResponse {
        snapshot: live.snapshot(),
        granted,
    })
    .into_response()
}

async fn handle_desktop_codex_live_status(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(request): Json<DesktopCodexLiveStatusRequest>,
) -> Response {
    if let Some(response) = desktop_codex_live_capability_required(&headers) {
        return response;
    }
    if request.server_epoch.len() > 128
        || request.host_id.trim().is_empty()
        || request.host_id.len() > 128
        || !valid_desktop_codex_live_phase(request.phase.trim())
        || request.status_text.chars().count() > 320
        || request
            .active_thread_id
            .as_ref()
            .is_some_and(|value| value.len() > 256)
    {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_desktop_codex_live_status");
    }

    let mut live = state.desktop_codex_live.write().await;
    let now_ms = chrono::Utc::now().timestamp_millis();
    let host_can_publish = live.host_id.as_deref() == Some(request.host_id.trim())
        && !desktop_codex_live_host_lease_expired(&live, now_ms);
    // A late heartbeat from an older command must never overwrite a newer
    // start/stop request that the canonical host has not consumed yet.
    if request.server_epoch == live.server_epoch
        && request.revision == live.revision
        && host_can_publish
    {
        let acknowledged_lifecycle = live
            .command
            .as_ref()
            .is_some_and(|command| matches!(command.action.as_str(), "start" | "stop"));
        let acknowledged_mute = live
            .command
            .as_ref()
            .is_some_and(|command| command.action == "mute");
        live.host_id = Some(request.host_id.trim().to_string());
        live.phase = request.phase.trim().to_string();
        live.status_text = request.status_text.trim().to_string();
        live.active_project_path =
            normalize_desktop_codex_live_project_path(request.active_project_path);
        if live.active_project_path.is_some() {
            live.last_project_path = live.active_project_path.clone();
        }
        live.active_thread_id = request
            .active_thread_id
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        if !acknowledged_lifecycle || !live.pending_mute_after_lifecycle {
            live.microphone_muted = request.microphone_muted;
        }
        live.updated_at_ms = now_ms;
        live.command = None;
        if acknowledged_lifecycle && live.pending_mute_after_lifecycle {
            live.pending_mute_after_lifecycle = false;
            if matches!(
                live.phase.as_str(),
                "preparing" | "connecting" | "active" | "reconnecting"
            ) {
                live.revision = live.revision.saturating_add(1);
                live.command = Some(DesktopCodexLiveCommand {
                    action: "mute".to_string(),
                    project_path: None,
                    microphone_muted: Some(live.microphone_muted),
                });
            }
        }
        if live.phase == "idle" {
            live.microphone_muted = false;
        } else if acknowledged_mute {
            live.microphone_muted = request.microphone_muted;
        }
    }
    Json(live.snapshot()).into_response()
}

fn bridge_cors_origin_strings(
    configured_console_origin: Option<String>,
    include_development_origins: bool,
) -> Vec<String> {
    let mut origins = vec![
        "tauri://localhost".to_string(),
        "http://tauri.localhost".to_string(),
        "https://tauri.localhost".to_string(),
    ];
    if include_development_origins {
        origins.extend([1420_u16, 5173, 5174].into_iter().flat_map(|port| {
            [
                format!("http://localhost:{port}"),
                format!("http://127.0.0.1:{port}"),
            ]
        }));
    }
    if let Some(origin) = configured_console_origin {
        origins.push(origin);
    }
    origins.sort();
    origins.dedup();
    origins
}

pub async fn start_bridge_daemon(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    start_bridge_server_inner(None, port).await
}

pub async fn start_bridge_server(
    app_handle: AppHandle,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    start_bridge_server_inner(Some(app_handle), port).await
}

async fn start_bridge_server_inner(
    app_handle: Option<AppHandle>,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    // 生产 tunnel 由系统 LaunchDaemon 常驻托管。bridge 启动时只在 daemon
    // 缺席时尝试拉起，避免每次 app 启动都无条件打断现有 tunnel 连接。
    tokio::spawn(async {
        let preferred_labels = [
            "system/xin.tobooks.cunzhi.cloudflared-proxied.root",
            "system/com.cloudflare.cloudflared",
        ];

        for label in preferred_labels {
            match tokio::process::Command::new("launchctl")
                .args(["print", label])
                .output()
                .await
            {
                Ok(output) => {
                    let summary = format!(
                        "{}\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                    if output.status.success() && summary.contains("state = running") {
                        log::info!("[Bridge] {} 已在运行，跳过启动时重启", label);
                        return;
                    }
                }
                Err(err) => log::debug!("[Bridge] 检查 {} 状态失败: {}", label, err),
            }
        }

        for label in preferred_labels {
            match tokio::process::Command::new("launchctl")
                .args(["kickstart", "-k", label])
                .output()
                .await
            {
                Ok(kickstart) if kickstart.status.success() => {
                    log::info!("[Bridge] {} 不在运行，已执行启动恢复", label);
                    return;
                }
                Ok(kickstart) => {
                    log::warn!(
                        "[Bridge] {} 启动恢复失败: status={:?}, stderr={}",
                        label,
                        kickstart.status.code(),
                        String::from_utf8_lossy(&kickstart.stderr)
                    );
                    if label == "system/xin.tobooks.cunzhi.cloudflared-proxied.root" {
                        let _ =
                            signal_root_tunnel_recovery_request("startup_recovery_failed").await;
                    }
                }
                Err(err) => log::warn!("[Bridge] {} 启动恢复失败: {}", label, err),
            }
        }
    });

    // 加载已保存的 APNs Token
    init_apns_tokens(bridge_apns_default_environment().as_str()).await;

    // 启动公网探针后台刷新循环：connection-status 读缓存秒回，不再每请求阻塞探针。
    spawn_public_probe_cache_refresher();

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let tx = BRIDGE_BROADCAST.clone();
    spawn_quota_snapshot_refresher(app_handle.clone(), tx.clone());

    let configured_console_origin = app_handle
        .as_ref()
        .and_then(|handle| handle.try_state::<crate::config::AppState>())
        .and_then(|state| {
            state
                .config
                .lock()
                .ok()
                .map(|config| config.cloudflare_config.web_login_console_origin.clone())
        })
        .or_else(|| {
            crate::config::storage::load_standalone_config()
                .ok()
                .map(|config| config.cloudflare_config.web_login_console_origin)
        })
        .and_then(|origin| normalize_web_origin(&origin).ok());
    let allowed_origins =
        bridge_cors_origin_strings(configured_console_origin, cfg!(debug_assertions))
            .into_iter()
            .filter_map(|origin| HeaderValue::from_str(&origin).ok())
            .collect::<Vec<_>>();

    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list(allowed_origins))
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::PATCH,
            axum::http::Method::PUT,
            axum::http::Method::DELETE,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            HeaderName::from_static("x-iterate-device-id"),
            HeaderName::from_static("x-iterate-device-token"),
        ])
        .allow_credentials(true);

    let app = Router::new()
        .route("/", get(handle_index))
        .route("/index.html", get(handle_index))
        .route("/bridge_test.html", get(handle_index))
        .route(
            "/.well-known/iterate/health",
            get(handle_well_known_iterate_health),
        )
        .route("/pair", get(handle_pair_page))
        .route("/pair/challenge", post(handle_pair_challenge))
        .route("/pair/claim", post(handle_pair_claim))
        .route("/session/refresh", post(handle_session_refresh))
        .route("/session/revoke", post(handle_session_revoke))
        .route("/apple-touch-icon.png", get(handle_apple_touch_icon_png))
        .route("/manifest.webmanifest", get(handle_web_app_manifest))
        .route("/sw.js", get(handle_sw_js))
        .route("/push/vapid_public_key", get(handle_push_vapid_public_key))
        .route("/push/subscribe", post(handle_push_subscribe))
        .route("/push/unsubscribe", post(handle_push_unsubscribe))
        .route("/api/apns/register", post(handle_apns_register))
        .route("/api/apns/notify", post(handle_apns_notify))
        .route(
            "/api/apns/live-activity/register",
            post(handle_apns_live_activity_register),
        )
        .route(
            "/api/apns/live-activity/update",
            post(handle_apns_live_activity_update),
        )
        .route("/api/phone-action", post(handle_api_phone_action))
        .route(
            "/api/phone-action-result",
            get(handle_api_phone_action_result),
        )
        .route(
            "/api/phone-action-jobs/:id",
            get(handle_api_phone_action_job),
        )
        .route("/api/mobile/pairing", get(handle_api_mobile_pairing))
        .route(
            "/api/mobile/pairing/claim",
            post(handle_api_mobile_pairing_claim),
        )
        .route(
            "/api/mobile/pairing/sessions/:session_id",
            get(handle_api_mobile_pairing_session),
        )
        .route(
            "/api/mobile/pairing/status",
            get(handle_api_mobile_pairing_status),
        )
        .route(
            "/api/quick-tunnel/status",
            get(handle_api_quick_tunnel_status),
        )
        .route(
            "/api/quick-tunnel/start",
            post(handle_api_quick_tunnel_start),
        )
        .route("/api/quick-tunnel/stop", post(handle_api_quick_tunnel_stop))
        .route(
            "/api/mobile/paired-device-file-roots",
            get(handle_get_paired_device_file_roots).post(handle_update_paired_device_file_roots),
        )
        .route(
            "/api/speech-muscle-memory",
            get(handle_api_speech_muscle_memory_get).post(handle_api_speech_muscle_memory_post),
        )
        .route(
            "/api/speech-correction-memory",
            get(handle_api_speech_correction_memory_get)
                .post(handle_api_speech_correction_memory_post),
        )
        .route(
            "/api/speech-vocabulary",
            get(handle_api_speech_vocabulary_get).post(handle_api_speech_vocabulary_post),
        )
        .route(
            "/api/prevent-sleep",
            get(handle_api_prevent_sleep_get).post(handle_api_prevent_sleep_post),
        )
        .route("/bridge/publish", post(handle_bridge_publish))
        .route("/api/room-submit", post(handle_local_room_submit))
        .route("/image", get(handle_serve_image))
        .route("/bridge/pull_action", post(handle_bridge_pull_action))
        .route("/files", get(handle_get_files))
        .route("/files/roots", get(handle_get_file_roots))
        .route("/files/mkdir", post(handle_create_directory))
        .route("/windows", get(handle_get_windows))
        .route(
            "/api/mcp-tools",
            get(handle_api_mcp_tools).post(handle_api_mcp_tools_post),
        )
        .route(
            "/api/prompt-library",
            get(handle_api_prompt_library_get)
                .post(handle_api_prompt_library_post)
                .delete(handle_api_prompt_library_delete),
        )
        .route(
            "/api/promptor-library",
            get(handle_api_promptor_library_get),
        )
        .route(
            "/api/ghost-suggestions",
            get(handle_api_ghost_suggestions_get)
                .post(handle_api_ghost_suggestions_post)
                .put(handle_api_ghost_suggestions_put),
        )
        .route(
            "/api/ghost-suggestions/reorder",
            post(handle_api_ghost_suggestions_reorder),
        )
        .route(
            "/api/ghost-suggestion-learning",
            get(handle_api_ghost_suggestion_learning_get)
                .post(handle_api_ghost_suggestion_learning_post),
        )
        .route(
            "/api/ghost-suggestions/:id",
            patch(handle_api_ghost_suggestions_patch).delete(handle_api_ghost_suggestions_delete),
        )
        .route(
            "/api/import-prompts-dir",
            post(handle_api_import_prompts_dir),
        )
        .route("/api/audio-assets", get(handle_api_audio_assets))
        .route("/api/test-audio", post(handle_api_test_audio))
        .route("/api/version", get(handle_api_version))
        .route("/api/connection-status", get(handle_api_connection_status))
        .route(
            "/api/connection-diagnostics",
            get(handle_api_connection_status),
        )
        .route("/api/diagnostics", get(handle_api_connection_status))
        .route("/api/bridge/health", get(handle_api_connection_status))
        .route(
            "/api/desktop-codex-live",
            get(handle_desktop_codex_live_get).post(handle_desktop_codex_live_control),
        )
        .route(
            "/api/desktop-codex-live/status",
            post(handle_desktop_codex_live_status),
        )
        .route(
            "/api/desktop-codex-live/lease",
            post(handle_desktop_codex_live_lease),
        )
        .route(
            "/api/config",
            get(handle_api_config_get).post(handle_api_config_post),
        )
        .route("/mobile", get(handle_mobile_page))
        .route("/api/active-sessions", get(handle_api_active_sessions))
        .route("/api/cleanup-session", post(handle_api_cleanup_session))
        .route("/api/show-window", get(handle_api_show_window))
        .route("/api/open-codex-chat", post(handle_api_open_codex_chat))
        .route(
            "/api/recover-tailscale-funnel",
            post(handle_api_recover_tailscale_funnel),
        )
        .route("/api/restart-tunnel", post(handle_api_restart_tunnel))
        .route("/api/restart-service", post(handle_api_restart_service))
        .route("/ws/codex-live", get(handle_codex_live_ws_upgrade))
        .route("/ws", get(handle_ws_upgrade))
        .layer(middleware::from_fn(audit_public_control_request))
        .layer(middleware::from_fn(enforce_bridge_control_auth))
        .layer(cors)
        .with_state(BridgeHttpState {
            app_handle,
            tx,
            port,
            desktop_codex_live: Arc::new(RwLock::new(
                match super::codex_live::last_continuity_project_path() {
                    Ok(last_project_path) => {
                        DesktopCodexLiveState::with_last_project_path(last_project_path)
                    }
                    Err(error) => {
                        bridge_debug_log(&format!(
                            "恢复 GPT-Live 最近目标项目失败，保持未选择状态: {error}"
                        ));
                        DesktopCodexLiveState::default()
                    }
                },
            )),
        });

    // 端口绑定重试（旧进程退出后端口可能还没释放）
    let listener = {
        let mut result: Option<TcpListener> = None;
        for attempt in 1..=10 {
            match TcpListener::bind(addr).await {
                Ok(l) => {
                    bridge_debug_log(&format!("端口 {} 绑定成功（第 {} 次尝试）", port, attempt));
                    result = Some(l);
                    break;
                }
                Err(e) => {
                    bridge_debug_log(&format!(
                        "端口 {} 绑定失败（第 {} 次）: {}",
                        port, attempt, e
                    ));
                    if attempt == 10 {
                        bridge_debug_log(&format!("端口 {} 绑定最终失败: {}", port, e));
                        return Err(Box::new(e));
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
        result.unwrap()
    };
    let auth_broker = crate::bridge::auth::start_internal_auth_broker()
        .await
        .map_err(|error| std::io::Error::other(format!("Bridge auth broker failed: {error}")))?;
    tokio::spawn(async move {
        // Release builds must reconcile legacy Quick Tunnel state even when a
        // healthy formal route is already configured. Otherwise an old saved
        // preference can leave a stale Quick process/proof alive.
        if !crate::tunnel::manager::quick_tunnel_test_capability_enabled() {
            crate::tunnel::manager::autostart_quick_tunnel().await;
            return;
        }

        if has_healthy_supported_route_without_quick(port).await {
            log::info!("[Bridge][QuickTunnel] existing healthy route suppresses autostart");
            return;
        }
        crate::tunnel::manager::autostart_quick_tunnel().await;
    });
    bridge_debug_log(&format!("Bridge Server 启动成功: http://{}", addr));
    log_important!(
        info,
        "[Bridge] Bridge Server 正在监听 (HTTP+WS): http://{}",
        addr
    );

    let serve_result = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await;
    auth_broker.abort();
    serve_result?;

    Ok(())
}

fn authenticate_internal_websocket_once(
    headers: &HeaderMap,
    remote_addr: SocketAddr,
) -> Result<bool, String> {
    if trusted_internal_capability(headers) {
        return Ok(true);
    }
    if !crate::bridge::auth::has_internal_bridge_bearer(headers) {
        return Ok(false);
    }
    if !remote_addr.ip().is_loopback() {
        return Err("internal_auth_requires_loopback".to_string());
    }
    crate::bridge::auth::authenticate_internal_bridge_bearer(headers, "GET", "/ws")?
        .map(|_| true)
        .ok_or_else(|| "invalid_internal_bridge_auth".to_string())
}

async fn handle_ws_upgrade(
    State(state): State<BridgeHttpState>,
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    uri: Uri,
) -> axum::response::Response {
    let client_id = format!("ws_{}", uuid::Uuid::new_v4());
    let connected_at = chrono::Utc::now().to_rfc3339();
    let user_agent = debug_header_value(&headers, header::USER_AGENT.as_str());
    let forwarded_for = debug_header_value(&headers, "x-forwarded-for");
    let forwarded_proto = debug_header_value(&headers, "x-forwarded-proto");
    let cf_ray = debug_header_value(&headers, "cf-ray");
    let host = debug_header_value(&headers, header::HOST.as_str());
    let internal_authenticated = match authenticate_internal_websocket_once(&headers, remote_addr) {
        Ok(authenticated) => authenticated,
        Err(error) => {
            let response_error = if error == "internal_auth_requires_loopback" {
                "internal_auth_requires_loopback"
            } else {
                bridge_debug_log(&format!(
                    "[Bridge Auth] internal websocket bearer rejected: {}",
                    error
                ));
                log::warn!(
                    "[Bridge][Auth] internal websocket bearer rejected: {}",
                    error
                );
                "invalid_internal_bridge_auth"
            };
            return json_error_response(StatusCode::UNAUTHORIZED, response_error);
        }
    };
    let websocket_authentication = if internal_authenticated {
        None
    } else {
        match authenticate_bridge_websocket_result(&headers, &uri).await {
            Ok(authentication) => authentication,
            Err(error) => return bridge_auth_error_response(&error),
        }
    };
    let allowed_browser_origins = websocket_authentication
        .as_ref()
        .and_then(|authentication| authentication.allowed_browser_origins.as_deref());
    let auth_enforced = bridge_auth_required_for_request(&headers, remote_addr);
    if !browser_websocket_origin_is_allowed(&headers, allowed_browser_origins, auth_enforced) {
        return json_error_response(StatusCode::FORBIDDEN, "invalid_websocket_origin");
    }
    let auth_principal = websocket_authentication.map(|authentication| authentication.principal);
    if !internal_authenticated {
        if let Some((status, error)) = websocket_auth_denial(auth_enforced, auth_principal.as_ref())
        {
            log::warn!(
                "[Bridge][MobileAuth] rejecting anonymous public websocket upgrade host={} xff_present={} cf_ray_present={}",
                host,
                forwarded_for != "-",
                cf_ray != "-",
            );
            return (
                status,
                Json(serde_json::json!({
                    "ok": false,
                    "error": error,
                })),
            )
                .into_response();
        }
    }
    let scope_enforced = !internal_authenticated && websocket_scope_enforced(auth_enforced);
    log::info!(
        "[Bridge][WS Debug] upgrade ua={} host={} proto={} xff={} cf_ray={} authenticated={} auth_enforce={} scope_enforce={}",
        user_agent,
        host,
        forwarded_proto,
        forwarded_for,
        cf_ray,
        auth_principal.is_some() || internal_authenticated,
        auth_enforced,
        scope_enforced,
    );
    let client_kind = auth_principal
        .as_ref()
        .map(|principal| principal.client_kind.clone())
        .unwrap_or_else(|| {
            if internal_authenticated {
                "relay_mac".to_string()
            } else {
                classify_ws_client_kind(&user_agent, &host, &forwarded_for)
            }
        });
    let ws_client_info = WsClientInfo {
        client_id: client_id.clone(),
        connected_at: connected_at.clone(),
        last_seen_at: connected_at,
        last_message_type: None,
        remote_addr: Some(remote_addr.to_string()),
        host: host.clone(),
        x_forwarded_for: forwarded_for.clone(),
        x_forwarded_proto: forwarded_proto.clone(),
        cf_ray: cf_ray.clone(),
        user_agent: user_agent.clone(),
        authenticated: auth_principal.is_some() || internal_authenticated,
        authenticated_device_id: auth_principal
            .as_ref()
            .map(|principal| principal.device_id.clone()),
        authenticated_client_kind: auth_principal
            .as_ref()
            .map(|principal| principal.client_kind.clone()),
        client_kind,
        device_id: auth_principal
            .as_ref()
            .map(|principal| principal.device_id.clone()),
        selected_transport_mode: None,
        selected_ws_url: None,
        project_path: None,
        request_id: None,
    };
    let app_handle = state.app_handle.clone();
    let tx = state.tx.clone();
    ws.protocols(["iterate.mobile.v1"])
        .on_upgrade(move |socket| async move {
            register_ws_client_after_upgrade(ws_client_info).await;
            handle_axum_connection(
                socket,
                app_handle,
                tx,
                client_id,
                auth_principal,
                scope_enforced,
            )
            .await
        })
}

async fn handle_codex_live_ws_upgrade(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    uri: Uri,
) -> axum::response::Response {
    let desktop_authenticated = if let Some(token) =
        websocket_desktop_token_from_protocols(&headers)
    {
        if !remote_addr.ip().is_loopback() {
            return json_error_response(
                StatusCode::UNAUTHORIZED,
                "desktop_live_auth_requires_loopback",
            );
        }
        match crate::bridge::auth::authenticate_internal_bridge_token(
            &token,
            "GET",
            "/ws/codex-live",
        ) {
            Ok(crate::bridge::auth::BridgeTokenAudience::DesktopRenderer) => true,
            Ok(_) | Err(_) => {
                return json_error_response(StatusCode::UNAUTHORIZED, "invalid_desktop_live_auth");
            }
        }
    } else {
        false
    };

    if desktop_authenticated {
        return ws
            .protocols(["iterate.codex-live.v1"])
            .max_message_size(96 * 1024)
            .max_frame_size(96 * 1024)
            .on_upgrade(crate::bridge::codex_live::serve);
    }

    let authentication = match authenticate_bridge_websocket_result(&headers, &uri).await {
        Ok(Some(authentication)) => authentication,
        Ok(None) => return json_error_response(StatusCode::UNAUTHORIZED, "mobile_auth_required"),
        Err(error) => return bridge_auth_error_response(&error),
    };
    if !browser_websocket_origin_is_allowed(
        &headers,
        authentication.allowed_browser_origins.as_deref(),
        true,
    ) {
        return json_error_response(StatusCode::FORBIDDEN, "invalid_websocket_origin");
    }
    if !authentication.principal.has_scope(SCOPE_SESSION_RESPOND) {
        return json_error_response(StatusCode::FORBIDDEN, "missing_scope_session_respond");
    }

    ws.protocols(["iterate.codex-live.v1"])
        .max_message_size(96 * 1024)
        .max_frame_size(96 * 1024)
        .on_upgrade(crate::bridge::codex_live::serve)
}

fn websocket_auth_denial(
    auth_required: bool,
    principal: Option<&AuthPrincipal>,
) -> Option<(StatusCode, &'static str)> {
    if auth_required && principal.is_none() {
        Some((StatusCode::UNAUTHORIZED, "mobile_auth_required"))
    } else {
        None
    }
}

fn websocket_scope_enforced(auth_required: bool) -> bool {
    auth_required
}

async fn enforce_bridge_control_auth(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    mut req: Request,
    next: Next,
) -> Response {
    // Never trust a marker supplied by the network. Only this middleware may
    // add it after validating a loopback-only, method/path-bound capability.
    req.headers_mut().remove(TRUSTED_INTERNAL_CAPABILITY_HEADER);

    let method = req.method().as_str().to_string();
    let path = req.uri().path().to_string();
    if crate::bridge::auth::has_internal_bridge_bearer(req.headers()) {
        if !remote_addr.ip().is_loopback() {
            return json_error_response(
                StatusCode::UNAUTHORIZED,
                "internal_auth_requires_loopback",
            );
        }
        return match crate::bridge::auth::authenticate_internal_bridge_bearer(
            req.headers(),
            &method,
            &path,
        ) {
            Ok(Some(_)) => {
                req.headers_mut().insert(
                    HeaderName::from_static(TRUSTED_INTERNAL_CAPABILITY_HEADER),
                    HeaderValue::from_static("1"),
                );
                next.run(req).await
            }
            Ok(None) => {
                bridge_debug_log("[Bridge Auth] middleware internal bearer was not recognized");
                json_error_response(StatusCode::UNAUTHORIZED, "invalid_internal_bridge_auth")
            }
            Err(error) => {
                bridge_debug_log(&format!(
                    "[Bridge Auth] middleware internal bearer rejected: {}",
                    error
                ));
                json_error_response(StatusCode::UNAUTHORIZED, "invalid_internal_bridge_auth")
            }
        };
    }

    let auth_required = bridge_auth_required_for_request(req.headers(), remote_addr);
    if !auth_required {
        return next.run(req).await;
    }

    if req.method() == axum::http::Method::OPTIONS
        || public_anonymous_path_allowed(req.method(), req.uri().path())
        || body_bound_auth_deferred(req.method(), req.uri().path())
    {
        return next.run(req).await;
    }

    let credential_present = has_bridge_auth_header(req.headers())
        || crate::bridge::auth::cookie_token_from_headers(req.headers()).is_some();
    match authenticate_bridge_http_result(req.headers()).await {
        Ok(Some(authentication)) => {
            if authentication
                .allowed_browser_origins
                .as_deref()
                .is_some_and(|origins| !browser_http_origin_is_allowed(req.headers(), origins))
            {
                return json_error_response(StatusCode::FORBIDDEN, "invalid_http_origin");
            }
            next.run(req).await
        }
        Ok(None) => json_error_response(
            StatusCode::UNAUTHORIZED,
            if credential_present {
                "invalid_device_auth"
            } else {
                "mobile_auth_required"
            },
        ),
        Err(error) => bridge_auth_error_response(&error),
    }
}

async fn audit_public_control_request(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    if method != axum::http::Method::OPTIONS {
        let headers = req.headers();
        let path = req.uri().path();
        if is_public_bridge_request(headers) && is_public_control_path(path) {
            let public_auth_required = mobile_auth_required() || is_public_bridge_request(headers);
            let principal = match authenticate_bridge_headers_result(headers).await {
                Ok(principal) => principal,
                Err(error) => return bridge_auth_error_response(&error),
            };
            let host = debug_header_value(headers, header::HOST.as_str());
            let forwarded_for = debug_header_value(headers, "x-forwarded-for");
            let cf_ray = debug_header_value(headers, "cf-ray");
            let user_agent = debug_header_value(headers, header::USER_AGENT.as_str());
            log::warn!(
                "[Bridge][SecurityAudit] public control surface reached: method={} path={} host={} xff_present={} cf_ray_present={} auth_header_present={} authenticated={} enforce={} ua={}",
                method,
                path,
                host,
                forwarded_for != "-",
                cf_ray != "-",
                has_bridge_auth_header(headers),
                principal.is_some(),
                public_auth_required,
                truncate_audit_value(&user_agent, 160),
            );
            if public_auth_required
                && principal.is_none()
                && !public_anonymous_path_allowed(&method, path)
            {
                return (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "ok": false,
                        "error": "mobile_auth_required",
                    })),
                )
                    .into_response();
            }
        }
    }

    next.run(req).await
}

fn classify_ws_client_kind(user_agent: &str, host: &str, forwarded_for: &str) -> String {
    let ua = user_agent.to_ascii_lowercase();
    if ua.contains("curl/") {
        "curl_probe".to_string()
    } else if ua.contains("iphone") || ua.contains("ipad") || ua.contains("ios") {
        "ios".to_string()
    } else if ua.contains("webkit") && (host == "127.0.0.1:8080" || host == "localhost:8080") {
        "desktop_webview".to_string()
    } else if forwarded_for != "-" {
        "public_tunnel_client".to_string()
    } else {
        "unknown".to_string()
    }
}

fn prune_ws_client_registry(registry: &mut HashMap<String, WsClientInfo>) {
    if registry.len() < WS_CLIENT_REGISTRY_MAX_ENTRIES {
        return;
    }
    let mut entries = registry
        .iter()
        .map(|(client_id, info)| (client_id.clone(), info.last_seen_at.clone()))
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.1.cmp(&right.1));
    let remove_count = registry
        .len()
        .saturating_sub(WS_CLIENT_REGISTRY_MAX_ENTRIES - 1);
    for (client_id, _) in entries.into_iter().take(remove_count) {
        registry.remove(&client_id);
    }
}

async fn register_ws_client_after_upgrade(info: WsClientInfo) {
    let mut registry = WS_CLIENT_REGISTRY.write().await;
    prune_ws_client_registry(&mut registry);
    registry.insert(info.client_id.clone(), info);
}

async fn update_ws_client_from_message(
    client_id: &str,
    message: &BridgeMessage,
    auth_principal: Option<&AuthPrincipal>,
) {
    let mut registry = WS_CLIENT_REGISTRY.write().await;
    let Some(info) = registry.get_mut(client_id) else {
        return;
    };
    info.last_seen_at = chrono::Utc::now().to_rfc3339();
    info.last_message_type = Some(message.message_type.clone());

    if let Some(principal) = auth_principal {
        info.authenticated = true;
        info.authenticated_device_id = Some(principal.device_id.clone());
        info.authenticated_client_kind = Some(principal.client_kind.clone());
        info.client_kind = principal.client_kind.clone();
        info.device_id = Some(principal.device_id.clone());
    } else {
        if let Some(value) = json_string_field(
            &message.payload,
            &["client_kind", "clientKind", "source_client", "sourceClient"],
        ) {
            info.client_kind = value;
        } else if message.message_type == "client_hello" && info.client_kind == "unknown" {
            info.client_kind = "ios".to_string();
        }
        if let Some(value) = json_string_field(&message.payload, &["device_id", "deviceId"]) {
            info.device_id = Some(value);
        }
    }
    if let Some(value) = json_string_field(
        &message.payload,
        &[
            "selected_transport_mode",
            "selectedTransportMode",
            "transport_mode",
            "transportMode",
        ],
    ) {
        info.selected_transport_mode = Some(value);
    }
    if let Some(value) = json_string_field(
        &message.payload,
        &["selected_ws_url", "selectedWsUrl", "ws_url", "wsUrl"],
    ) {
        info.selected_ws_url = Some(value);
    }
    if let Some(value) = json_string_field(&message.payload, &["project_path", "projectPath"])
        .or_else(|| {
            nested_metadata_string_field(&message.payload, &["project_path", "projectPath"])
        })
    {
        info.project_path = Some(value);
    }
    if let Some(value) = json_string_field(&message.payload, &["request_id", "requestId"])
        .or_else(|| nested_metadata_string_field(&message.payload, &["request_id", "requestId"]))
    {
        info.request_id = Some(value);
    }
}

async fn snapshot_ws_clients() -> Vec<WsClientInfo> {
    let mut clients = WS_CLIENT_REGISTRY
        .read()
        .await
        .values()
        .cloned()
        .collect::<Vec<_>>();
    clients.sort_by(|left, right| right.last_seen_at.cmp(&left.last_seen_at));
    clients
}

#[cfg(test)]
fn bridge_message_targets_device(message: &BridgeMessage, device_id: Option<&str>) -> bool {
    let Some(target_device_id) = phone_action_target_device_id(message) else {
        return true;
    };

    device_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_some_and(|value| value == target_device_id)
}

async fn bridge_message_should_send_to_ws_client(client_id: &str, message: &BridgeMessage) -> bool {
    if message.message_type != "phone_action_request" {
        return true;
    }

    let delivery_client_ids =
        phone_action_delivery_client_ids(phone_action_target_device_id(message).as_deref()).await;

    delivery_client_ids.contains(client_id)
}

fn phone_action_client_is_newer(left: &WsClientInfo, right: &WsClientInfo) -> bool {
    (
        left.last_seen_at.as_str(),
        left.connected_at.as_str(),
        left.client_id.as_str(),
    ) > (
        right.last_seen_at.as_str(),
        right.connected_at.as_str(),
        right.client_id.as_str(),
    )
}

async fn phone_action_delivery_client_ids(target_device_id: Option<&str>) -> HashSet<String> {
    let target_device_id = target_device_id
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let registry = WS_CLIENT_REGISTRY.read().await;
    let mut legacy_client_ids = HashSet::new();
    let mut latest_by_device_id: HashMap<String, WsClientInfo> = HashMap::new();

    for info in registry.values() {
        if !info.client_kind.eq_ignore_ascii_case("ios") {
            continue;
        }

        let device_id = info
            .device_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty());

        if let Some(target_device_id) = target_device_id {
            if device_id != Some(target_device_id) {
                continue;
            }
        }

        let Some(device_id) = device_id else {
            if target_device_id.is_none() {
                legacy_client_ids.insert(info.client_id.clone());
            }
            continue;
        };

        latest_by_device_id
            .entry(device_id.to_string())
            .and_modify(|existing| {
                if phone_action_client_is_newer(info, existing) {
                    *existing = info.clone();
                }
            })
            .or_insert_with(|| info.clone());
    }

    let mut client_ids = legacy_client_ids;
    client_ids.extend(latest_by_device_id.into_values().map(|info| info.client_id));
    client_ids
}

async fn record_phone_action_result(
    client_id: &str,
    message: &BridgeMessage,
) -> Option<PhoneActionResultEntry> {
    let source_device_id = {
        let registry = WS_CLIENT_REGISTRY.read().await;
        registry
            .get(client_id)
            .and_then(|info| info.device_id.clone())
    };
    let entry = phone_action_result_entry_from_message(
        message,
        Some(client_id.to_string()),
        source_device_id,
    )?;

    let mut results = PHONE_ACTION_RESULTS.write().await;
    prune_phone_action_results(&mut results, chrono::Utc::now());
    results.insert(entry.id.clone(), entry.clone());
    Some(entry)
}

fn remote_action_denial_reason(
    principal: &AuthPrincipal,
    message: &BridgeMessage,
) -> Option<String> {
    match message.message_type.as_str() {
        "client_hello" => None,
        "request_sync" | REQUEST_TIMELINE_SYNC_MESSAGE_TYPE => {
            if principal.has_scope(SCOPE_SESSION_READ) {
                None
            } else {
                Some(format!("missing scope {}", SCOPE_SESSION_READ))
            }
        }
        "request_main_page" => {
            let tab = message
                .payload
                .get("tab")
                .and_then(|value| value.as_str())
                .unwrap_or("intro");
            match tab {
                "intro" | "settings" => None,
                "prompts" => {
                    if principal.has_scope(SCOPE_PROMPT_LIBRARY_READ) {
                        None
                    } else {
                        Some(format!("missing scope {}", SCOPE_PROMPT_LIBRARY_READ))
                    }
                }
                "tools" => {
                    if principal.has_scope(SCOPE_CONFIG_READ) {
                        None
                    } else {
                        Some(format!("missing scope {}", SCOPE_CONFIG_READ))
                    }
                }
                _ => Some(format!("unknown main page tab denied: {}", tab)),
            }
        }
        "phone_action_result" => {
            if principal.has_scope(SCOPE_SESSION_RESPOND) {
                None
            } else {
                Some(format!("missing scope {}", SCOPE_SESSION_RESPOND))
            }
        }
        "system_command" => {
            let command = message
                .payload
                .get("command")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            match command {
                "show_main_window" => {
                    if principal.has_scope(SCOPE_WINDOW_SHOW) {
                        None
                    } else {
                        Some(format!("missing scope {}", SCOPE_WINDOW_SHOW))
                    }
                }
                "toggle_prevent_sleep" => {
                    if principal.has_scope(SCOPE_SESSION_RESPOND) {
                        None
                    } else {
                        Some(format!("missing scope {}", SCOPE_SESSION_RESPOND))
                    }
                }
                _ => Some(format!("unknown system command denied: {}", command)),
            }
        }
        "mcp_action" => {
            let action = message
                .payload
                .get("action")
                .and_then(|value| value.as_str())
                .unwrap_or_default();
            match action {
                "submit"
                | "continue"
                | "cancel"
                | "enhance"
                | "goal"
                | "goal_start"
                | "update_window_conditional_state"
                | "update_window_conditional_active" => {
                    if principal.has_scope(SCOPE_SESSION_RESPOND) {
                        None
                    } else {
                        Some(format!("missing scope {}", SCOPE_SESSION_RESPOND))
                    }
                }
                "send_to_browser_ai" => {
                    Some("browser.write not allowed for MVP paired iOS".to_string())
                }
                "update_conditional_state"
                | "update_conditional_active"
                | "update_custom_prompt_order" => {
                    Some("config.write not allowed for MVP paired iOS".to_string())
                }
                _ => Some(format!("unknown mcp action denied: {}", action)),
            }
        }
        message_type => Some(format!("unknown remote message denied: {}", message_type)),
    }
}

async fn handle_axum_connection(
    socket: WebSocket,
    app_handle: Option<AppHandle>,
    tx: broadcast::Sender<BridgeMessage>,
    client_id: String,
    mut auth_principal: Option<AuthPrincipal>,
    auth_enforced: bool,
) {
    log_important!(info, "[Bridge] Web 端已连接 (Axum WS)");
    if let Some(info) = ws_client_info_snapshot(&client_id).await {
        bridge_debug_log(&format!(
            "WS 客户端已连接: client_id={} client_kind={} device_id={} host={} transport={} request_id={} ua={}",
            info.client_id,
            info.client_kind,
            info.device_id.as_deref().unwrap_or("-"),
            info.host,
            info.selected_transport_mode.as_deref().unwrap_or("-"),
            info.request_id.as_deref().unwrap_or("-"),
            info.user_agent,
        ));
    } else {
        bridge_debug_log(&format!("WS 客户端已连接: client_id={}", client_id));
    }
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut rx = tx.subscribe();
    // 每 30 秒发送 ping 保持连接（防止 Cloudflare Tunnel 空闲超时断连）
    let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(30));

    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                if let Err(e) = ws_sender.send(Message::Ping(vec![b'p', b'i', b'n', b'g'])).await {
                    log::debug!("[Bridge] Ping 发送失败: {}", e);
                    break;
                }
            }
            result = rx.recv() => {
                let bridge_msg = match result {
                    Ok(msg) => msg,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        bridge_debug_log(&format!("broadcast lagged, skipped {} messages", n));
                        continue;
                    }
                    Err(_) => {
                        bridge_debug_log("broadcast channel closed");
                        break;
                    }
                };
                if !bridge_message_should_send_to_ws_client(&client_id, &bridge_msg).await {
                    log::debug!(
                        "[Bridge] skip targeted phone_action_request for client_id={}",
                        client_id
                    );
                    continue;
                }
                if let Ok(text) = serde_json::to_string(&bridge_msg) {
                    if let Err(e) = ws_sender.send(Message::Text(text)).await {
                        log_important!(error, "[Bridge] 发送消息到 Web 端失败: {}", e);
                        break;
                    }
                }
            }
            msg = ws_receiver.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        let redacted_text = redact_bridge_message_text(&text);
                        log_important!(info, "[Bridge] 收到 Web 端消息: {}", redacted_text);
                        bridge_debug_log(&format!("WS 收到消息: {}", redacted_text.chars().take(100).collect::<String>()));
                        if let Ok(bridge_msg) = serde_json::from_str::<BridgeMessage>(&text) {
                            bridge_debug_log(&format!("消息类型: {}", bridge_msg.message_type));
                            if auth_enforced && auth_principal.is_none() {
                                if let Some(token) = websocket_device_token_from_message(&bridge_msg) {
                                    match authenticate_bridge_token_result(
                                        token,
                                        websocket_device_id_from_message(&bridge_msg),
                                    )
                                    .await
                                    {
                                        Ok(principal) => auth_principal = principal,
                                        Err(_) => {
                                            let _ = ws_sender
                                                .send(Message::Text(
                                                    serde_json::json!({
                                                        "message_type": "auth_error",
                                                        "payload": {
                                                            "error": AUTH_STORE_UNAVAILABLE_ERROR
                                                        }
                                                    })
                                                    .to_string(),
                                                ))
                                                .await;
                                            break;
                                        }
                                    }
                                }

                                if auth_principal.is_none() {
                                    log::warn!(
                                        "[Bridge][MobileAuth] closing unauthenticated WS message type={} client_id={}",
                                        bridge_msg.message_type,
                                        client_id
                                    );
                                    let _ = ws_sender
                                        .send(Message::Text(
                                            serde_json::json!({
                                                "message_type": "auth_error",
                                                "payload": {
                                                    "error": "mobile_auth_required"
                                                }
                                            })
                                            .to_string(),
                                        ))
                                        .await;
                                    break;
                                }
                            }
                            update_ws_client_from_message(
                                &client_id,
                                &bridge_msg,
                                auth_principal.as_ref(),
                            )
                            .await;
                            if auth_enforced {
                                if let Some(principal) = auth_principal.as_ref() {
                                    if let Some(reason) =
                                        remote_action_denial_reason(principal, &bridge_msg)
                                    {
                                        log::warn!(
                                            "[Bridge][MobileAuth] denied remote WS message principal={} type={} reason={}",
                                            principal.principal_id,
                                            bridge_msg.message_type,
                                            reason
                                        );
                                        continue;
                                    }
                                }
                            }
                            if let Some(phone_action_result) =
                                record_phone_action_result(&client_id, &bridge_msg).await
                            {
                                if let Some(app_handle) = app_handle.as_ref() {
                                    if let Err(error) =
                                        app_handle.emit("phone-action-result", phone_action_result)
                                    {
                                        log::warn!(
                                            "[Bridge] emit phone-action-result failed: {}",
                                            error
                                        );
                                    }
                                }
                            }
                            let mut mcp_action_handled_by_rust = false;
                            if bridge_msg.message_type == "mcp_action" {
                                if let Some(raw_path) = bridge_msg
                                    .payload
                                    .get("project_path")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string())
                                {
                                    // 规范化 project_path（与 IPC 端一致）
                                    let target_project_path =
                                        normalize_bridge_project_path(&raw_path);
                                    bridge_debug_log(&format!("project_path 规范化: {} -> {}", raw_path, target_project_path));
                                        let request_id = bridge_msg.payload
                                            .get("request_id")
                                            .or_else(|| bridge_msg.payload.get("requestId"))
                                        .or_else(|| {
                                            bridge_msg
                                                .payload
                                                .get("metadata")
                                                .and_then(|meta| {
                                                    meta.get("request_id")
                                                        .or_else(|| meta.get("requestId"))
                                                })
                                            })
                                            .and_then(|v| v.as_str())
                                            .map(|s| s.to_string());
                                        let target_request_is_stale =
                                            request_id_is_stale_for_current_window_binding(
                                                request_id.as_deref(),
                                                Some(&target_project_path),
                                            );
                                        if target_request_is_stale {
                                            log::info!(
                                                "[Bridge] stale mcp_action dropped: request_id={:?}, project_path={}",
                                                request_id,
                                                target_project_path
                                            );
                                            bridge_debug_log(&format!(
                                                "[Bridge Route] stale mcp_action dropped: project_path={}, request_id={:?}",
                                                target_project_path,
                                                request_id
                                            ));
                                            mcp_action_handled_by_rust = true;
                                        } else {
                                        let fallback_route = last_active_route().await;
                                        let timeline_route_resolution = {
                                            let registry = ACTIVE_SESSION_REGISTRY.read().await;
                                            resolve_mcp_action_timeline_route_id(
                                                &bridge_msg.payload,
                                                request_id.as_deref(),
                                                Some(&target_project_path),
                                                fallback_route.as_deref(),
                                                &registry,
                                            )
                                        };
                                        let timeline_route_id =
                                            timeline_route_resolution.as_ref().map(|resolution| {
                                                resolution.route_id.clone()
                                            });
                                        if let Some(resolution) = &timeline_route_resolution {
                                            bridge_debug_log(&format!(
                                                "[Bridge Route] mcp_action timeline route resolved: source={}, project_path={}, request_id={:?}, timeline_route_id={}, fallback_route={:?}",
                                                resolution.source,
                                                target_project_path,
                                                request_id,
                                                resolution.route_id,
                                                fallback_route
                                            ));
                                        } else {
                                            bridge_debug_log(&format!(
                                                "[Bridge Route] mcp_action timeline route unresolved: project_path={}, request_id={:?}, fallback_route={:?}",
                                                target_project_path,
                                                request_id,
                                                fallback_route
                                            ));
                                        }
                                        eprintln!(
                                            "[Bridge] mcp_action received: action={:?}, project_path={}, request_id={:?}, timeline_route_id={:?}",
                                            bridge_msg.payload.get("action"),
                                            target_project_path,
                                            request_id,
                                            timeline_route_id
                                        );
                                        if has_room_submit_metadata(&bridge_msg.payload) {
                                            let outcome = handle_room_submit_action(
                                                app_handle.as_ref(),
                                                &target_project_path,
                                                request_id.as_deref(),
                                                timeline_route_id.as_deref(),
                                                &bridge_msg.payload,
                                            )
                                        .await;
                                        broadcast_room_submit_outcome(&tx, &outcome);
                                        mcp_action_handled_by_rust = true;
                                    // 尝试直接在 Rust 端处理 mcp_action（不依赖前端 WebView）
                                    } else {
                                        let handled = if let Some(app_handle) = app_handle.as_ref() {
                                            try_handle_mcp_action_directly(
                                                    app_handle,
                                                    &target_project_path,
                                                    request_id.as_deref(),
                                                    timeline_route_id.as_deref(),
                                                    &bridge_msg.payload,
                                                )
                                                .await
                                            } else {
                                                try_handle_mcp_action_headless(
                                                    &target_project_path,
                                                    request_id.as_deref(),
                                                    timeline_route_id.as_deref(),
                                                    &bridge_msg.payload,
                                                )
                                            .await
                                        };
                                        if handled.delivered {
                                            log::info!("[Bridge] mcp_action 已在 Rust 端直接处理 (project: {})", target_project_path);
                                            mcp_action_handled_by_rust = true;
                                        } else {
                                            // 回退：缓存 action，等前端轮询消费
                                            let mut cache = MCP_ACTION_CACHE.write().await;
                                            let mut touched_at =
                                                MCP_ACTION_CACHE_TOUCHED_AT.write().await;
                                            prune_json_cache(
                                                "mcp_action",
                                                &mut cache,
                                                &mut touched_at,
                                                MCP_ACTION_CACHE_TTL_SECS,
                                                MCP_ACTION_CACHE_MAX_ENTRIES,
                                            );
                                            // 优先用 request_id 单键缓存，避免同一消息被多个窗口消费。
                                            // 只有在没有 request_id 时才用 project_path 作为 fallback key。
                                            let mut cache_keys = Vec::<String>::new();
                                            let rid_opt = request_id
                                                .clone()
                                                .map(|value| value.trim().to_string())
                                                .filter(|value| !value.is_empty());
                                            if let Some(rid) = rid_opt {
                                                cache.insert(rid.clone(), bridge_msg.payload.clone());
                                                cache_keys.push(rid);
                                            } else {
                                                // 没有 request_id 时才用 project_path
                                                let normalized_project_key = target_project_path.trim().to_string();
                                                if !normalized_project_key.is_empty() {
                                                    cache.insert(
                                                        normalized_project_key.clone(),
                                                        bridge_msg.payload.clone(),
                                                    );
                                                    cache_keys.push(normalized_project_key);
                                                }
                                            }
                                            mark_json_cache_keys(&mut touched_at, &cache_keys);
                                            record_cache_write_count("mcp_action", cache_keys.len());
                                            log::info!(
                                                "[Bridge] mcp_action 已缓存 (keys: {:?}, project: {})",
                                                cache_keys,
                                                target_project_path
                                            );
                                        }
                                    }
                                    }
                                }
                            }

                            // 处理系统命令（如防止睡眠）
                            if bridge_msg.message_type == "system_command" {
                                if let Some(command) = bridge_msg.payload.get("command").and_then(|v| v.as_str()) {
                                    match command {
                                        "toggle_prevent_sleep" => {
                                            match crate::ui::commands::toggle_prevent_sleep_local() {
                                                Ok(status) => {
                                                    broadcast_prevent_sleep_status_for_app(
                                                        app_handle.as_ref(),
                                                        status,
                                                    );
                                                    log::info!("[Bridge] 合盖运行状态切换并广播: {}", status);
                                                }
                                                Err(error) => {
                                                    log::warn!("[Bridge] 合盖运行状态切换失败: {}", error);
                                                }
                                            }
                                        }
                                        "show_main_window" => {
                                            if let Some(app_handle) = app_handle.as_ref() {
                                                if let Err(error) =
                                                    crate::ui::commands::activate_app_window(
                                                        app_handle.clone(),
                                                    )
                                                    .await
                                                {
                                                    log::warn!(
                                                        "[Bridge] 显示主窗口失败: {}",
                                                        error
                                                    );
                                                } else {
                                                    log::info!("[Bridge] 已激活并聚焦主窗口");
                                                }
                                            } else {
                                                log::warn!("[Bridge] bridge-only 模式无法直接显示主窗口");
                                            }
                                        }
                                        _ => {
                                            log::warn!("[Bridge] 未知系统命令: {}", command);
                                        }
                                    }
                                }
                            }

                            // 处理主页面请求：组装主页内容为 Markdown，以 mcp_state 格式推送
                            if bridge_msg.message_type == "request_main_page" {
                                let tab = bridge_msg.payload.get("tab").and_then(|v| v.as_str()).unwrap_or("intro");
                                let content = if let Some(app_handle) = app_handle.as_ref() {
                                    build_main_page_markdown(app_handle, tab).await
                                } else {
                                    "bridge-only daemon 当前不承载主页面 UI。请打开 iterate GUI 查看主页面。".to_string()
                                };
                                let main_page_msg = BridgeMessage {
                                    message_type: "mcp_state".to_string(),
                                    payload: serde_json::json!({
                                        "request": {
                                            "message": content,
                                            "project_path": "main_page",
                                            "predefined_options": ["📋 介绍", "🔧 MCP 工具", "📝 提示词库", "⚙️ 设置", "💬 返回消息"]
                                        }
                                    }),
                                };
                                if let Ok(text) = serde_json::to_string(&main_page_msg) {
                                    let _ = ws_sender.send(Message::Text(text)).await;
                                }
                            }

                            let is_request_sync = bridge_msg.message_type == "request_sync";
                            let is_timeline_sync_request =
                                bridge_msg.message_type == REQUEST_TIMELINE_SYNC_MESSAGE_TYPE;

                            // 多进程弹窗模式下，手机端切换项目时桌面窗口可能不会响应。
                            // 这里直接用主进程缓存的 mcp_state 回应，保证 ∞ 菜单切换有效。
                            // 另外：request_sync 与 request_timeline_sync 都返回独立 timeline 快照。
                            if is_request_sync || is_timeline_sync_request {
                                let target_request_id = bridge_msg
                                    .payload
                                    .get("request_id")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                let target_project_path = bridge_msg
                                    .payload
                                    .get("project_path")
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                let sync_reason = bridge_msg
                                    .payload
                                    .get("sync_reason")
                                    .and_then(|v| v.as_str())
                                    .map(str::trim)
                                    .filter(|reason| !reason.is_empty())
                                    .unwrap_or("manual")
                                    .to_string();
                                let requested_codex_home = bridge_msg
                                    .payload
                                    .get("codex_home")
                                    .or_else(|| bridge_msg.payload.get("codexHome"))
                                    .and_then(|value| value.as_str())
                                    .map(str::trim)
                                    .filter(|value| !value.is_empty())
                                    .map(ToOwned::to_owned);
                                let mut timeline_route_hint: Option<serde_json::Value> = None;
                                let normalized_project_path = target_project_path
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|path| !path.is_empty() && *path != ".")
                                    .map(ToOwned::to_owned);
                                let effective_target_request_id = effective_request_sync_request_id(
                                    target_request_id.as_deref(),
                                    normalized_project_path.as_deref(),
                                );
                                let fallback_route = if effective_target_request_id.is_none()
                                    && normalized_project_path.is_none()
                                {
                                    last_active_route().await
                                } else {
                                    None
                                };
                                let target_request_is_stale =
                                    request_id_is_stale_for_current_window_binding(
                                        effective_target_request_id.as_deref(),
                                        normalized_project_path.as_deref(),
                                    );

                                if is_request_sync {
                                    if target_request_is_stale {
                                        cleanup_stale_request_sync_route(
                                            effective_target_request_id.as_deref(),
                                            normalized_project_path.as_deref(),
                                            "request_sync_stale_window_binding",
                                        )
                                        .await;
                                    }

                                    let (
                                        cached_payload,
                                        local_cache_hit,
                                        cache_lookup_key,
                                        cache_touched_at,
                                    ) = {
                                        let mut cache = MCP_STATE_CACHE.write().await;
                                        let mut touched_at =
                                            MCP_STATE_CACHE_TOUCHED_AT.write().await;
                                        prune_json_cache(
                                            "mcp_state",
                                            &mut cache,
                                            &mut touched_at,
                                            MCP_STATE_CACHE_TTL_SECS,
                                            MCP_STATE_CACHE_MAX_ENTRIES,
                                        );
                                        let (route, key, payload) = if target_request_is_stale {
                                            (CacheLookupRoute::RequestId, None, None)
                                        } else if let Some(ref rid) = effective_target_request_id {
                                            (
                                                CacheLookupRoute::RequestId,
                                                Some(rid.clone()),
                                                cache.get(rid).cloned(),
                                            )
                                        } else if let Some(ref path) = normalized_project_path {
                                            (
                                                CacheLookupRoute::ProjectPath,
                                                Some(path.clone()),
                                                cache.get(path).cloned(),
                                            )
                                        } else if let Some(ref route_key) = fallback_route {
                                            (
                                                CacheLookupRoute::FallbackRoute,
                                                Some(route_key.clone()),
                                                cache.get(route_key).cloned(),
                                            )
                                        } else {
                                            (CacheLookupRoute::FallbackRoute, None, None)
                                        };
                                        let touched = key
                                            .as_ref()
                                            .and_then(|key| touched_at.get(key).cloned());
                                        let hit = payload.is_some();
                                        MCP_STATE_CACHE_METRICS.record_lookup(route, hit);
                                        (payload, hit, key, touched)
                                    };
                                    let local_cache_age_ms = cache_touched_at.map(|touched| {
                                        (chrono::Utc::now() - touched).num_milliseconds().max(0)
                                    });
                                    let mut cache_source: Option<&'static str> =
                                        local_cache_hit.then_some("mcp_state_cache");
                                    let mut cache_age_ms = local_cache_age_ms;
                                    let mut registry_fallback_hit = false;
                                    let mut serve_request_file_fallback_hit = false;
                                    let mut serve_request_file_fallback_miss: Option<&'static str> =
                                        None;
                                    let cached_payload = if target_request_is_stale {
                                        None
                                    } else if local_cache_hit {
                                        cached_payload
                                    } else {
                                        let registry_entry = {
                                            let registry = ACTIVE_SESSION_REGISTRY.read().await;
                                            lookup_active_session_entry(
                                                &registry,
                                                effective_target_request_id.as_deref(),
                                                normalized_project_path.as_deref(),
                                                fallback_route.as_deref(),
                                            )
                                        };
                                        if let Some(entry) = registry_entry {
                                            registry_fallback_hit = true;
                                            cache_source = Some("active_session_registry");
                                            cache_age_ms = parse_rfc3339(&entry.last_active_at)
                                                .map(|last_active_at| {
                                                    (chrono::Utc::now() - last_active_at)
                                                        .num_milliseconds()
                                                        .max(0)
                                                });
                                            MCP_STATE_CACHE_METRICS
                                                .record_active_registry_fallback_hit();
                                            Some(entry.payload)
                                        } else {
                                            let serve_request_fallback = match (
                                                effective_target_request_id.clone(),
                                                normalized_project_path.clone(),
                                            ) {
                                                (Some(request_id), Some(project_path)) => {
                                                    match tokio::task::spawn_blocking(move || {
                                                        load_live_serve_request_fallback(
                                                            &request_id,
                                                            &project_path,
                                                        )
                                                    })
                                                    .await
                                                    {
                                                        Ok(Ok(fallback)) => Some(fallback),
                                                        Ok(Err(miss)) => {
                                                            serve_request_file_fallback_miss =
                                                                Some(miss.as_str());
                                                            None
                                                        }
                                                        Err(error) => {
                                                            serve_request_file_fallback_miss =
                                                                Some("worker_join_failed");
                                                            log::warn!(
                                                                "[Bridge] serve request fallback worker failed: {}",
                                                                error
                                                            );
                                                            None
                                                        }
                                                    }
                                                }
                                                _ => None,
                                            };
                                            if let Some(fallback) = serve_request_fallback {
                                                serve_request_file_fallback_hit = true;
                                                cache_source = Some("serve_request_file");
                                                cache_age_ms = Some(fallback.age_ms);
                                                Some(fallback.payload)
                                            } else {
                                                None
                                            }
                                        }
                                    };

                                    bridge_debug_log(&format!(
                                        "[Bridge Timing] request_sync lookup: request_id={:?}, effective_request_id={:?}, project_path={:?}, fallback_route={:?}, stale_request={}, cache_hit={}, registry_fallback_hit={}, serve_request_file_fallback_hit={}, serve_request_file_fallback_miss={:?}, cache_source={:?}, cache_age_ms={:?}, sync_reason={}",
                                        target_request_id,
                                        effective_target_request_id,
                                        normalized_project_path,
                                        fallback_route,
                                        target_request_is_stale,
                                        local_cache_hit,
                                        registry_fallback_hit,
                                        serve_request_file_fallback_hit,
                                        serve_request_file_fallback_miss,
                                        cache_source,
                                        cache_age_ms,
                                        sync_reason
                                    ));
                                    timeline_route_hint = cached_payload.clone();

                                    if let Some(mut payload) = cached_payload {
                                        ensure_custom_prompts_in_mcp_state(
                                            app_handle.as_ref(),
                                            &mut payload,
                                        );
                                        ensure_ghost_suggestions_in_mcp_state(&mut payload);
                                        let payload_request_id =
                                            extract_request_id_from_mcp_state(&payload);
                                        let payload_project_path =
                                            extract_project_path_from_mcp_state(&payload);
                                        crate::ui::live_goal::ensure_live_goal_in_mcp_state(
                                            app_handle.as_ref(),
                                            &mut payload,
                                            payload_project_path
                                                .as_deref()
                                                .or(normalized_project_path.as_deref()),
                                        );
                                        let payload_codex_home =
                                            crate::ui::quota_snapshot::codex_home_from_mcp_state(
                                                &payload,
                                            )
                                            .or_else(|| requested_codex_home.clone());
                                        inject_cached_quota_snapshot_and_refresh_async(
                                            app_handle.as_ref(),
                                            &tx,
                                            &mut payload,
                                            payload_codex_home,
                                            "quota_request_sync",
                                        );
                                        let route_key = resolve_tree_route_key(
                                            payload_request_id.as_deref(),
                                            payload_project_path.as_deref(),
                                        )
                                        .or_else(|| cache_lookup_key.clone());
                                        if let Some(object) = payload.as_object_mut() {
                                            object.insert(
                                                "sync_response".to_string(),
                                                serde_json::Value::Bool(true),
                                            );
                                            object.insert(
                                                "suppress_remote_notification".to_string(),
                                                serde_json::Value::Bool(true),
                                            );
                                            if let Some(cache_age_ms) = cache_age_ms {
                                                object.insert(
                                                    "cache_age_ms".to_string(),
                                                    serde_json::json!(cache_age_ms),
                                                );
                                            }
                                            if let Some(cache_source) = cache_source {
                                                object.insert(
                                                    "cache_source".to_string(),
                                                    serde_json::json!(cache_source),
                                                );
                                            }
                                            if let Some(route_key) = route_key {
                                                object.insert(
                                                    "route_key".to_string(),
                                                    serde_json::json!(route_key),
                                                );
                                            }
                                            object.insert(
                                                "sync_reason".to_string(),
                                                serde_json::json!(sync_reason),
                                            );
                                        }
                                        TimelineSyncService::sanitize_payload_timeline_nodes(
                                            &mut payload,
                                        );
                                        let cached_msg = BridgeMessage {
                                            message_type: "mcp_state".to_string(),
                                            payload,
                                        };
                                        if let Ok(text) = serde_json::to_string(&cached_msg) {
                                            let _ = ws_sender.send(Message::Text(text)).await;
                                        }
                                    } else if let Some(live_goal) =
                                        crate::ui::live_goal::live_goal_payload_for_project(
                                            app_handle.as_ref(),
                                            normalized_project_path.as_deref(),
                                        )
                                    {
                                        let mut payload = serde_json::json!({
                                            "sync_response": true,
                                            "suppress_remote_notification": true,
                                            "cache_source": "live_goal_store",
                                            "sync_reason": sync_reason,
                                            "live_goal": live_goal,
                                        });
                                        inject_cached_quota_snapshot_and_refresh_async(
                                            app_handle.as_ref(),
                                            &tx,
                                            &mut payload,
                                            requested_codex_home.clone(),
                                            "quota_request_sync",
                                        );
                                        let cached_msg = BridgeMessage {
                                            message_type: "mcp_state".to_string(),
                                            payload,
                                        };
                                        if let Ok(text) = serde_json::to_string(&cached_msg) {
                                            let _ = ws_sender.send(Message::Text(text)).await;
                                        }
                                    } else {
                                        let mut payload = serde_json::json!({
                                            "sync_response": true,
                                            "suppress_remote_notification": true,
                                            "cache_source": "quota_snapshot",
                                            "sync_reason": sync_reason,
                                        });
                                        inject_cached_quota_snapshot_and_refresh_async(
                                            app_handle.as_ref(),
                                            &tx,
                                            &mut payload,
                                            requested_codex_home.clone(),
                                            "quota_request_sync",
                                        );
                                        let cached_msg = BridgeMessage {
                                            message_type: "mcp_state".to_string(),
                                            payload,
                                        };
                                        if let Ok(text) = serde_json::to_string(&cached_msg) {
                                            let _ = ws_sender.send(Message::Text(text)).await;
                                        }
                                    }

                                    // 同步防止睡眠状态给新连接的客户端
                                    let prevent_sleep_status =
                                        crate::ui::commands::get_prevent_sleep_status_local();
                                    let status_msg = BridgeMessage {
                                        message_type: "prevent_sleep_status".to_string(),
                                        payload: serde_json::json!({
                                            "enabled": prevent_sleep_status
                                        }),
                                    };
                                    if let Ok(text) = serde_json::to_string(&status_msg) {
                                        let _ = ws_sender.send(Message::Text(text)).await;
                                    }
                                }

                                let mut timeline_request_id = effective_target_request_id;
                                let mut timeline_project_path = target_project_path;

                                if timeline_request_id.is_none() {
                                    if let Some(payload) = timeline_route_hint.as_ref() {
                                        timeline_request_id = extract_request_id_from_mcp_state(payload);
                                        if timeline_project_path.is_none() {
                                            timeline_project_path =
                                                extract_project_path_from_mcp_state(payload);
                                        }
                                    }
                                }

                                if let Some(app_handle) = app_handle.as_ref() {
                                    if let Some(snapshot_msg) = TimelineSyncService::build_snapshot_message(
                                        app_handle,
                                        timeline_request_id.as_deref(),
                                        timeline_project_path.as_deref(),
                                    )
                                    .await
                                    {
                                        if let Ok(text) = serde_json::to_string(&snapshot_msg) {
                                            if let Err(err) = ws_sender.send(Message::Text(text)).await {
                                                log::warn!("[Bridge] 发送时间线快照失败: {}", err);
                                            }
                                        }
                                    }
                                }
                            }

                            // 如果 Rust 端已处理 mcp_action，不再转发给前端（避免竞态双重消费）
                            if !mcp_action_handled_by_rust {
                                if let Some(app_handle) = app_handle.as_ref() {
                                    if let Err(e) = app_handle.emit("bridge-message", bridge_msg) {
                                        log_important!(error, "[Bridge] 推送消息到 Tauri 前端失败: {}", e);
                                    }
                                } else {
                                    log::debug!("[Bridge] bridge-only 模式跳过 Tauri 前端 emit");
                                }
                            }
                        }
                    }
                    Some(Ok(Message::Close(frame))) => {
                        bridge_debug_log(&format!("WS 客户端发送 Close: {:?}", frame));
                        log_important!(info, "[Bridge] Web 端已断开连接");
                        break;
                    }
                    None => {
                        bridge_debug_log("WS stream ended (None)");
                        log_important!(info, "[Bridge] Web 端已断开连接");
                        break;
                    }
                    Some(Ok(Message::Pong(_))) => {
                        // Pong 响应，正常
                    }
                    Some(Ok(_)) => {
                        // 其他消息类型（Binary 等），忽略
                    }
                    Some(Err(e)) => {
                        bridge_debug_log(&format!("WS 接收错误: {}", e));
                        log_important!(error, "[Bridge] WS 接收错误: {}", e);
                        break;
                    }
                }
            }
        }
    }
    let disconnected_at = chrono::Utc::now();
    let removed_info = {
        let mut registry = WS_CLIENT_REGISTRY.write().await;
        registry.remove(&client_id)
    };
    if let Some(info) = removed_info {
        let duration_ms = ws_connection_duration_ms(&info, disconnected_at)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string());
        bridge_debug_log(&format!(
            "WS client registry removed: client_id={} client_kind={} device_id={} host={} duration_ms={} last_message_type={} transport={} request_id={}",
            info.client_id,
            info.client_kind,
            info.device_id.as_deref().unwrap_or("-"),
            info.host,
            duration_ms,
            info.last_message_type.as_deref().unwrap_or("-"),
            info.selected_transport_mode.as_deref().unwrap_or("-"),
            info.request_id.as_deref().unwrap_or("-"),
        ));
    } else {
        bridge_debug_log(&format!("WS client registry removed: {}", client_id));
    }
}

async fn ws_client_info_snapshot(client_id: &str) -> Option<WsClientInfo> {
    WS_CLIENT_REGISTRY.read().await.get(client_id).cloned()
}

fn ws_connection_duration_ms(
    info: &WsClientInfo,
    disconnected_at: chrono::DateTime<chrono::Utc>,
) -> Option<i64> {
    let connected_at = chrono::DateTime::parse_from_rfc3339(&info.connected_at)
        .ok()?
        .with_timezone(&chrono::Utc);
    Some((disconnected_at - connected_at).num_milliseconds().max(0))
}

/// 写调试日志到文件（macOS GUI app 的 stdout/stderr 不可见）
/// 使用 spawn_blocking 避免阻塞 async worker
pub(super) fn bridge_debug_log(msg: &str) {
    let line = {
        let elapsed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        format!("[{}] {}\n", elapsed, msg)
    };
    // 在后台线程写文件，不阻塞 async runtime
    let _ = tokio::task::spawn_blocking(move || {
        use std::io::Write;
        let path = std::env::temp_dir().join("iterate_bridge_debug.log");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = f.write_all(line.as_bytes());
            let _ = f.flush();
            return;
        }
        eprintln!("[bridge_debug] {}", line.trim());
    });
}

fn instance_debug_log(tag: &str, message: impl AsRef<str>) {
    let line = format!(
        "{} [bridge:{}] {} {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        std::process::id(),
        tag,
        message.as_ref()
    );
    if let Ok(mut file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open("/tmp/iterate-instance-debug.log")
    {
        let _ = file.write_all(line.as_bytes());
    }
}

async fn handle_index() -> Html<String> {
    Html(include_str!("bridge_test.html").to_string())
}

async fn handle_apple_touch_icon_png() -> ([(header::HeaderName, &'static str); 1], Bytes) {
    (
        [(header::CONTENT_TYPE, "image/png")],
        Bytes::from_static(APPLE_TOUCH_ICON_PNG),
    )
}

async fn handle_web_app_manifest() -> ([(header::HeaderName, &'static str); 1], Bytes) {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        Bytes::from_static(WEB_APP_MANIFEST.as_bytes()),
    )
}

async fn handle_sw_js() -> ([(header::HeaderName, &'static str); 1], Bytes) {
    (
        [(header::CONTENT_TYPE, "application/javascript")],
        Bytes::from_static(SERVICE_WORKER_JS.as_bytes()),
    )
}

async fn handle_push_vapid_public_key() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "public_key": VAPID_CONFIG.public_key.clone() }))
}

async fn handle_push_subscribe(
    headers: HeaderMap,
    Json(subscription): Json<WebPushSubscriptionInfo>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_NOTIFICATION_SUBSCRIBE,
        "missing_scope_notification_subscribe",
    )
    .await
    {
        return response;
    }

    if let Err(error) = validate_web_push_subscription(&subscription) {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": error })),
        )
            .into_response();
    }

    let mut subscriptions = PUSH_SUBSCRIPTIONS.write().await;
    if !web_push_subscription_capacity_available(
        subscriptions.len(),
        subscriptions.contains_key(&subscription.endpoint),
    ) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "ok": false,
                "error": "Web Push 订阅数量已达上限"
            })),
        )
            .into_response();
    }
    subscriptions.insert(subscription.endpoint.clone(), subscription);
    Json(serde_json::json!({ "ok": true, "count": subscriptions.len() })).into_response()
}

#[derive(Debug, Deserialize)]
struct PushUnsubscribeRequest {
    endpoint: String,
}

async fn handle_push_unsubscribe(
    headers: HeaderMap,
    Json(request): Json<PushUnsubscribeRequest>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_NOTIFICATION_SUBSCRIBE,
        "missing_scope_notification_subscribe",
    )
    .await
    {
        return response;
    }

    if request.endpoint.is_empty() || request.endpoint.len() > MAX_WEB_PUSH_ENDPOINT_LENGTH {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "ok": false, "error": "Web Push endpoint 长度无效" })),
        )
            .into_response();
    }

    let mut subscriptions = PUSH_SUBSCRIPTIONS.write().await;
    let removed = subscriptions.remove(&request.endpoint).is_some();
    Json(serde_json::json!({ "ok": removed, "count": subscriptions.len() })).into_response()
}

/// GET /api/mobile/pairing — 返回 iOS companion 实验阶段的临时配对信息
async fn handle_api_mobile_pairing(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    let started_at = std::time::Instant::now();
    let public_request = is_public_bridge_request(&headers);
    let requires_auth = public_route_requires_auth(&headers);
    log::info!(
        "[Bridge][MobilePairing] issue_start port={} public_request={} requires_auth={}",
        state.port,
        public_request,
        requires_auth
    );
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_PAIRING_ISSUE, "missing_scope_pairing_issue")
            .await
    {
        log::warn!(
            "[Bridge][MobilePairing] issue_denied elapsed_ms={}",
            started_at.elapsed().as_millis()
        );
        return response;
    }

    let payload = match build_mobile_pairing_payload(state.port, true).await {
        Ok(payload) => payload,
        Err(error) => {
            log::warn!(
                "[Bridge][MobilePairing] issue_blocked error={} elapsed_ms={}",
                error,
                started_at.elapsed().as_millis()
            );
            return json_error_response(StatusCode::SERVICE_UNAVAILABLE, &error);
        }
    };
    let token_count = MOBILE_PAIRING_TOKENS.read().await.len();
    log::info!(
        "[Bridge][MobilePairing] issue_done transport={} candidate_count={} token_count={} elapsed_ms={}",
        payload.transport_mode,
        payload.candidates.len(),
        token_count,
        started_at.elapsed().as_millis()
    );
    Json(serde_json::json!({
        "ok": true,
        "pairing": payload,
        "token_count": token_count
    }))
    .into_response()
}

fn quick_tunnel_internal_auth_denial(
    remote_addr: SocketAddr,
    headers: &HeaderMap,
    _method: &str,
    _path: &str,
) -> Option<Response> {
    if !remote_addr.ip().is_loopback() || is_public_bridge_request(headers) {
        return Some(json_error_response(
            StatusCode::FORBIDDEN,
            "quick_tunnel_local_control_only",
        ));
    }
    #[cfg(target_os = "macos")]
    {
        // The control-auth middleware has already consumed the method/path-bound,
        // one-shot bearer before adding this private marker. Re-verifying the
        // original bearer here would always reject it as a replay.
        (!trusted_internal_capability(headers)).then(|| {
            json_error_response(StatusCode::UNAUTHORIZED, "invalid_quick_tunnel_capability")
        })
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

async fn handle_api_quick_tunnel_status(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) =
        quick_tunnel_internal_auth_denial(remote_addr, &headers, "GET", "/api/quick-tunnel/status")
    {
        return response;
    }
    Json(serde_json::json!({
        "ok": true,
        "status": crate::tunnel::manager::get_quick_status().await,
    }))
    .into_response()
}

async fn handle_api_quick_tunnel_start(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(request): Json<QuickTunnelStartRequest>,
) -> Response {
    if let Some(response) =
        quick_tunnel_internal_auth_denial(remote_addr, &headers, "POST", "/api/quick-tunnel/start")
    {
        return response;
    }
    if !crate::tunnel::manager::check_origin_health()
        .await
        .unwrap_or(false)
    {
        return json_error_response(StatusCode::SERVICE_UNAVAILABLE, "bridge_or_mcp_not_ready");
    }
    if has_healthy_supported_route_without_quick(state.port).await {
        return json_error_response(StatusCode::CONFLICT, "healthy_route_already_available");
    }
    if let Err(error) = crate::tunnel::manager::start_quick_tunnel(request.consent_v1).await {
        let status = if error == "quick_tunnel_consent_required" {
            StatusCode::CONFLICT
        } else if error == "cloudflared_missing" {
            StatusCode::FAILED_DEPENDENCY
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        return json_error_response(status, &error);
    }
    Json(serde_json::json!({
        "ok": true,
        "status": crate::tunnel::manager::get_quick_status().await,
    }))
    .into_response()
}

async fn handle_api_quick_tunnel_stop(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
) -> Response {
    if let Some(response) =
        quick_tunnel_internal_auth_denial(remote_addr, &headers, "POST", "/api/quick-tunnel/stop")
    {
        return response;
    }
    if let Err(error) = crate::tunnel::manager::stop_tunnel().await {
        return json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &error);
    }
    Json(serde_json::json!({
        "ok": true,
        "status": crate::tunnel::manager::get_quick_status().await,
    }))
    .into_response()
}

fn json_error_response(status: StatusCode, error: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "ok": false, "error": error })),
    )
        .into_response()
}

fn stale_request_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({
            "ok": false,
            "status": "rejected",
            "reason": "stale_request",
        })),
    )
        .into_response()
}

fn public_route_requires_auth(headers: &HeaderMap) -> bool {
    trusted_internal_capability(headers)
        || is_public_bridge_request(headers)
        || has_bridge_auth_header(headers)
        || crate::bridge::auth::cookie_token_from_headers(headers).is_some()
}

const TRUSTED_INTERNAL_CAPABILITY_HEADER: &str = "x-iterate-trusted-capability";

fn trusted_internal_capability(headers: &HeaderMap) -> bool {
    headers
        .get(TRUSTED_INTERNAL_CAPABILITY_HEADER)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == "1")
}

fn direct_network_peer_requires_auth(remote_addr: SocketAddr) -> bool {
    !remote_addr.ip().is_loopback()
}

fn direct_network_bridge_auth_path(path: &str) -> bool {
    path == "/image"
        || path == "/files"
        || path == "/files/roots"
        || path == "/files/mkdir"
        || path == "/windows"
        || path == "/bridge/publish"
        || path == "/bridge/pull_action"
        || path.starts_with("/api/ghost-suggestions/")
        || path.starts_with("/api/phone-action-jobs/")
        || matches!(
            path,
            "/api/active-sessions"
                | "/api/apns/register"
                | "/api/apns/live-activity/register"
                | "/api/apns/live-activity/update"
                | "/api/apns/notify"
                | "/api/audio-assets"
                | "/api/cleanup-session"
                | "/api/config"
                | "/api/ghost-suggestions"
                | "/api/import-prompts-dir"
                | "/api/mcp-tools"
                | "/api/mobile/pairing"
                | "/api/mobile/paired-device-file-roots"
                | "/api/open-codex-chat"
                | "/api/phone-action"
                | "/api/phone-action-result"
                | "/api/prevent-sleep"
                | "/api/prompt-library"
                | "/api/promptor-library"
                | "/api/recover-tailscale-funnel"
                | "/api/restart-service"
                | "/api/restart-tunnel"
                | "/api/show-window"
                | "/api/speech-correction-memory"
                | "/api/speech-muscle-memory"
                | "/api/test-audio"
                | "/push/subscribe"
                | "/push/unsubscribe"
        )
}

async fn direct_network_bridge_auth_denial(
    headers: &HeaderMap,
    path: &str,
    remote_addr: SocketAddr,
) -> Option<Response> {
    if !direct_network_peer_requires_auth(remote_addr) || !direct_network_bridge_auth_path(path) {
        return None;
    }
    match authenticate_bridge_headers_result(headers).await {
        Ok(Some(_)) => return None,
        Ok(None) => {}
        Err(error) => return Some(bridge_auth_error_response(&error)),
    }

    let error = if has_bridge_auth_header(headers) {
        "invalid_device_auth"
    } else {
        "mobile_auth_required"
    };
    Some(json_error_response(StatusCode::UNAUTHORIZED, error))
}

fn scoped_public_route_denial(
    principal: Option<&AuthPrincipal>,
    requires_device_auth: bool,
    scope: &str,
    missing_scope_error: &'static str,
) -> Option<(StatusCode, &'static str)> {
    if !requires_device_auth {
        return None;
    }

    let Some(principal) = principal else {
        return Some((StatusCode::UNAUTHORIZED, "invalid_device_auth"));
    };
    if principal.has_scope(scope) {
        None
    } else {
        Some((StatusCode::FORBIDDEN, missing_scope_error))
    }
}

async fn authorize_public_route_scope(
    headers: &HeaderMap,
    scope: &str,
    missing_scope_error: &'static str,
) -> Result<Option<AuthPrincipal>, Response> {
    if trusted_internal_capability(headers) {
        return Ok(None);
    }
    let requires_device_auth = public_route_requires_auth(headers);
    let principal = authenticate_bridge_headers_result(headers)
        .await
        .map_err(|error| bridge_auth_error_response(&error))?;
    if let Some((status, error)) = scoped_public_route_denial(
        principal.as_ref(),
        requires_device_auth,
        scope,
        missing_scope_error,
    ) {
        return Err(json_error_response(status, error));
    }

    Ok(principal)
}

async fn authorize_public_route_any_scope(
    headers: &HeaderMap,
    scopes: &[&str],
    missing_scope_error: &'static str,
) -> Result<Option<AuthPrincipal>, Response> {
    if trusted_internal_capability(headers) {
        return Ok(None);
    }
    let requires_device_auth = public_route_requires_auth(headers);
    let principal = authenticate_bridge_headers_result(headers)
        .await
        .map_err(|error| bridge_auth_error_response(&error))?;
    if !requires_device_auth {
        return Ok(principal);
    }

    let Some(principal) = principal else {
        return Err(json_error_response(
            StatusCode::UNAUTHORIZED,
            "invalid_device_auth",
        ));
    };
    if principal_has_any_scope(&principal, scopes) {
        Ok(Some(principal))
    } else {
        Err(json_error_response(
            StatusCode::FORBIDDEN,
            missing_scope_error,
        ))
    }
}

#[derive(Debug, Deserialize)]
struct PreventSleepControlRequest {
    action: String,
}

fn prevent_sleep_control(action: &str) -> Result<bool, String> {
    match action {
        "enable" => crate::ui::commands::enable_prevent_sleep_local(),
        "disable" => crate::ui::commands::disable_prevent_sleep_local(),
        "toggle" => crate::ui::commands::toggle_prevent_sleep_local(),
        _ => Err(format!("未知合盖运行操作: {}", action)),
    }
}

async fn handle_api_prevent_sleep_get(headers: HeaderMap) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_SESSION_READ, "missing_scope_session_read")
            .await
    {
        return response;
    }

    Json(serde_json::json!({
        "ok": true,
        "enabled": crate::ui::commands::get_prevent_sleep_status_local(),
    }))
    .into_response()
}

async fn handle_api_prevent_sleep_post(
    headers: HeaderMap,
    State(state): State<BridgeHttpState>,
    Json(request): Json<PreventSleepControlRequest>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_SESSION_RESPOND,
        "missing_scope_session_respond",
    )
    .await
    {
        return response;
    }

    match prevent_sleep_control(request.action.trim()) {
        Ok(enabled) => {
            broadcast_prevent_sleep_status_for_app(state.app_handle.as_ref(), enabled);
            Json(serde_json::json!({ "ok": true, "enabled": enabled })).into_response()
        }
        Err(error) => json_error_response(StatusCode::BAD_REQUEST, &error),
    }
}

fn principal_has_any_scope(principal: &AuthPrincipal, scopes: &[&str]) -> bool {
    scopes.iter().any(|scope| principal.has_scope(scope))
}

fn ghost_suggestions_write_scope_denial(
    principal: Option<&AuthPrincipal>,
    requires_device_auth: bool,
) -> Option<(StatusCode, &'static str)> {
    scoped_public_route_denial(
        principal,
        requires_device_auth,
        SCOPE_GHOST_SUGGESTIONS_WRITE,
        "missing_scope_ghost_suggestions_write",
    )
}

fn status_read_full_diagnostics_denial(
    principal: Option<&AuthPrincipal>,
    public_request: bool,
    requires_device_auth: bool,
) -> Option<(StatusCode, &'static str)> {
    if public_request && principal.is_none() {
        return None;
    }
    scoped_public_route_denial(
        principal,
        requires_device_auth,
        SCOPE_STATUS_READ,
        "missing_scope_status_read",
    )
}

async fn authorize_ghost_suggestions_read(headers: &HeaderMap) -> Result<(), Response> {
    authorize_public_route_scope(
        headers,
        SCOPE_GHOST_SUGGESTIONS_READ,
        "missing_scope_ghost_suggestions_read",
    )
    .await
    .map(|_| ())
}

async fn authorize_ghost_suggestions_write(
    headers: &HeaderMap,
) -> Result<Option<AuthPrincipal>, Response> {
    let requires_device_auth = public_route_requires_auth(headers);
    let principal = authenticate_bridge_headers_result(headers)
        .await
        .map_err(|error| bridge_auth_error_response(&error))?;
    if let Some((status, error)) =
        ghost_suggestions_write_scope_denial(principal.as_ref(), requires_device_auth)
    {
        return Err(json_error_response(status, error));
    }

    Ok(principal)
}

fn speech_memory_auth_denial(
    principal: Option<&AuthPrincipal>,
    public_request: bool,
    scope: &str,
    missing_scope_error: &'static str,
) -> Option<(StatusCode, &'static str)> {
    scoped_public_route_denial(principal, public_request, scope, missing_scope_error)
}

async fn authorize_speech_memory_access(
    headers: &HeaderMap,
    scope: &str,
    missing_scope_error: &'static str,
) -> Result<(), Response> {
    if trusted_internal_capability(headers) {
        return Ok(());
    }
    let requires_device_auth = public_route_requires_auth(headers);
    let principal = authenticate_bridge_headers_result(headers)
        .await
        .map_err(|error| bridge_auth_error_response(&error))?;
    if let Some((status, error)) = speech_memory_auth_denial(
        principal.as_ref(),
        requires_device_auth,
        scope,
        missing_scope_error,
    ) {
        return Err(json_error_response(status, error));
    }

    Ok(())
}

fn ghost_suggestions_write_error_response(error: String) -> Response {
    let status = match error.as_str() {
        crate::ghost_suggestions::CONFLICT_ERROR_CODE => StatusCode::CONFLICT,
        crate::ghost_suggestions::NOT_FOUND_ERROR_CODE => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    json_error_response(status, &error)
}

fn ghost_suggestions_write_success(state: &BridgeHttpState, store: serde_json::Value) -> Response {
    if let Some(app) = state.app_handle.as_ref() {
        broadcast_ghost_suggestions_changed(app, store.clone());
    } else {
        broadcast_ghost_suggestions_changed_to_bridge(store.clone());
    }

    Json(serde_json::json!({
        "ok": true,
        "ghostSuggestions": store,
    }))
    .into_response()
}

fn log_ghost_suggestions_write(principal: Option<&AuthPrincipal>, action: &str, target: &str) {
    log::info!(
        "[Bridge][GhostSuggestions] write principal={} device_id={} action={} target={}",
        principal
            .map(|value| value.principal_id.as_str())
            .unwrap_or("local_desktop"),
        principal
            .map(|value| value.device_id.as_str())
            .unwrap_or("local_desktop"),
        action,
        truncate_audit_value(target, 96),
    );
}

/// POST /api/mobile/pairing/claim — iOS 使用一次性 pairing token 换长期 device token
async fn claim_mobile_pairing_core(
    pairing_token_raw: &str,
    device_id_raw: &str,
    device_name_raw: Option<&str>,
    client_kind_raw: Option<&str>,
    allow_ghost_suggestions_write: bool,
    test_store: Option<&mut PairedDeviceStore>,
    injected_persist_error: Option<&str>,
) -> Result<MobilePairingClaimResponse, String> {
    let pairing_token = pairing_token_raw.trim();
    if pairing_token.is_empty() {
        return Err("invalid_pairing_token".to_string());
    }
    let device_id = device_id_raw.trim();
    if device_id.is_empty() {
        return Err("missing_device_id".to_string());
    }
    let device_name = device_name_raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("iPhone")
        .to_string();
    let client_kind = client_kind_raw
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("ios")
        .to_ascii_lowercase();
    if client_kind != "ios" {
        return Err("invalid_client_kind".to_string());
    }

    // Claims must be serialized so a concurrent retry cannot mint two credentials before the
    // durable paired-device record is written.
    let _claim_guard = MOBILE_PAIRING_CLAIM_LOCK.lock().await;
    let now = chrono::Utc::now();
    let now_string = now.to_rfc3339();
    let token_hash = bridge_token_hash(pairing_token);

    {
        let mut receipts = MOBILE_PAIRING_CLAIM_RECEIPTS.write().await;
        receipts.retain(|_, receipt| {
            parse_rfc3339(&receipt.expires_at)
                .map(|expires_at| expires_at > now)
                .unwrap_or(false)
        });
        if let Some(receipt) = receipts.get(&token_hash) {
            if receipt.device_id != device_id {
                return Err("pairing_token_already_claimed".to_string());
            }
            return Ok(MobilePairingClaimResponse {
                ok: true,
                device_id: receipt.device_id.clone(),
                device_token: receipt.device_token.clone(),
                scopes: receipt.scopes.clone(),
                pairing_session_id: receipt.session_id.clone(),
            });
        }
    }

    let mut persist_retry_grace_expired = false;
    let token_info = {
        let mut tokens = MOBILE_PAIRING_TOKENS.write().await;
        tokens.retain(|_, info| {
            parse_rfc3339(&info.expires_at)
                .map(|expires_at| expires_at > now)
                .unwrap_or(false)
        });
        let token_info = tokens.get(pairing_token).cloned();
        if token_info.as_ref().is_some_and(|info| {
            info.state == "failed" && !mobile_pairing_persist_retry_grace_active(info, now)
        }) {
            tokens.remove(pairing_token);
            persist_retry_grace_expired = true;
            None
        } else {
            token_info
        }
    };

    let Some(token_info) = token_info else {
        return Err(if persist_retry_grace_expired {
            "expired_pairing_token".to_string()
        } else {
            "invalid_pairing_token".to_string()
        });
    };
    if parse_rfc3339(&token_info.expires_at)
        .map(|expires_at| expires_at <= now)
        .unwrap_or(true)
    {
        return Err("expired_pairing_token".to_string());
    }

    if token_info.endpoint_binding.is_some() {
        if !pairing_token_endpoint_binding_is_current(&token_info).await {
            MOBILE_PAIRING_TOKENS.write().await.remove(pairing_token);
            if let Some(session) = MOBILE_PAIRING_SESSIONS
                .write()
                .await
                .get_mut(&token_info.session_id)
            {
                session.state = "expired".to_string();
            }
            return Err("endpoint_proof_failed".to_string());
        }
    }

    let device_token = generate_bridge_token("dt");
    let scopes = mobile_device_scopes(allow_ghost_suggestions_write);
    let record = PairedDeviceRecord {
        device_id: device_id.to_string(),
        device_name: device_name.clone(),
        client_kind: client_kind.clone(),
        token_hash: bridge_token_hash(&device_token),
        scopes: scopes.clone(),
        created_at: now_string.clone(),
        last_seen_at: now_string.clone(),
        file_browser_roots: Vec::new(),
        revoked_at: None,
    };

    if let Some(error) = injected_persist_error {
        mark_mobile_pairing_session_failed(&token_info.session_id).await;
        return Err(error.to_string());
    }

    if let Some(store) = test_store {
        replace_paired_device_record(store, record);
    } else {
        let path = paired_devices_path();
        if let Err(err) = mutate_paired_device_store_at(&path, |store| {
            replace_paired_device_record(store, record);
            ((), true)
        }) {
            log::warn!("[Bridge][MobileAuth] failed to save paired device: {}", err);
            mark_mobile_pairing_session_failed(&token_info.session_id).await;
            return Err("save_paired_device_failed".to_string());
        }
    }

    let receipt = MobilePairingClaimReceipt {
        session_id: token_info.session_id.clone(),
        device_id: device_id.to_string(),
        device_name,
        client_kind,
        device_token: device_token.clone(),
        scopes: scopes.clone(),
        claimed_at: now_string,
        expires_at: token_info.expires_at.clone(),
    };
    MOBILE_PAIRING_CLAIM_RECEIPTS
        .write()
        .await
        .insert(token_hash, receipt);
    mark_mobile_pairing_session_claimed(&token_info.session_id).await;
    MOBILE_PAIRING_TOKENS.write().await.remove(pairing_token);

    Ok(MobilePairingClaimResponse {
        ok: true,
        device_id: device_id.to_string(),
        device_token,
        scopes,
        pairing_session_id: token_info.session_id,
    })
}

async fn mark_mobile_pairing_session_claimed(session_id: &str) {
    let mut sessions = MOBILE_PAIRING_SESSIONS.write().await;
    if let Some(session) = sessions.get_mut(session_id) {
        session.state = "claimed".to_string();
        session.failure_count = 0;
        session.first_failed_at = None;
    }
}

async fn mark_mobile_pairing_session_failed(session_id: &str) {
    let failed_at = chrono::Utc::now().to_rfc3339();
    {
        let mut sessions = MOBILE_PAIRING_SESSIONS.write().await;
        if let Some(session) = sessions.get_mut(session_id) {
            record_mobile_pairing_persist_failure(session, &failed_at);
        }
    }
    {
        let mut tokens = MOBILE_PAIRING_TOKENS.write().await;
        for token in tokens
            .values_mut()
            .filter(|token| token.session_id == session_id)
        {
            record_mobile_pairing_persist_failure(token, &failed_at);
        }
    }
}

fn record_mobile_pairing_persist_failure(session: &mut PairingTokenInfo, failed_at: &str) {
    session.state = "failed".to_string();
    session.failure_count = session.failure_count.saturating_add(1);
    if session.first_failed_at.is_none() {
        session.first_failed_at = Some(failed_at.to_string());
    }
}

fn mobile_pairing_persist_retry_grace_active(
    session: &PairingTokenInfo,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if session.state != "failed" || session.failure_count == 0 {
        return false;
    }
    parse_rfc3339(session.first_failed_at.as_deref().unwrap_or_default())
        .filter(|failed_at| *failed_at <= now)
        .is_some_and(|failed_at| {
            now.signed_duration_since(failed_at)
                <= chrono::Duration::seconds(MOBILE_PAIRING_PERSIST_RETRY_GRACE_SECS)
        })
}

async fn handle_api_mobile_pairing_claim(
    State(state): State<BridgeHttpState>,
    Json(request): Json<MobilePairingClaimRequest>,
) -> Response {
    match claim_mobile_pairing_core(
        &request.pairing_token,
        &request.device_id,
        request.device_name.as_deref(),
        request.client_kind.as_deref(),
        mobile_ghost_suggestions_write_enabled(state.app_handle.as_ref()),
        None,
        None,
    )
    .await
    {
        Ok(response) => {
            log::info!(
                "[Bridge][MobileAuth] paired mobile device device_id={} scopes={:?} session_id={}",
                response.device_id,
                response.scopes,
                response.pairing_session_id,
            );
            Json(response).into_response()
        }
        Err(error) => match error.as_str() {
            "invalid_pairing_token" | "expired_pairing_token" => {
                json_error_response(StatusCode::UNAUTHORIZED, &error)
            }
            "missing_device_id" | "invalid_client_kind" => {
                json_error_response(StatusCode::BAD_REQUEST, &error)
            }
            "pairing_token_already_claimed" => json_error_response(StatusCode::CONFLICT, &error),
            _ => json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
        },
    }
}

async fn mobile_pairing_session_snapshot(session_id: &str) -> Option<MobilePairingSessionSnapshot> {
    let session_id = session_id.trim();
    if session_id.is_empty() {
        return None;
    }
    // Keep session failure metadata and its issue-token revocation in the same
    // serialization domain as issue/claim, so no observer can see a half-updated retry state.
    let _claim_guard = MOBILE_PAIRING_CLAIM_LOCK.lock().await;
    let now = chrono::Utc::now();
    let session_info = {
        let mut sessions = MOBILE_PAIRING_SESSIONS.write().await;
        sessions.retain(|_, info| {
            parse_rfc3339(&info.expires_at)
                .map(|expires_at| {
                    expires_at + chrono::Duration::seconds(MOBILE_PAIRING_SESSION_RETENTION_SECS)
                        > now
                })
                .unwrap_or(false)
        });
        sessions.get(session_id).cloned()
    }?;

    let receipt = {
        let mut receipts = MOBILE_PAIRING_CLAIM_RECEIPTS.write().await;
        receipts.retain(|_, receipt| {
            parse_rfc3339(&receipt.expires_at)
                .map(|expires_at| expires_at > now)
                .unwrap_or(false)
        });
        receipts
            .values()
            .find(|receipt| receipt.session_id == session_id)
            .cloned()
    };
    let retry_grace_active = {
        let mut tokens = MOBILE_PAIRING_TOKENS.write().await;
        let token_available = tokens.values().any(|token| {
            token.session_id == session_id
                && parse_rfc3339(&token.expires_at).is_some_and(|expires_at| expires_at > now)
        });
        let active =
            token_available && mobile_pairing_persist_retry_grace_active(&session_info, now);
        if session_info.state == "failed" && !active {
            // Expiring the server-side retry grace also revokes the issue token before
            // the terminal state is observable, so `failed` can never remain claimable.
            tokens.retain(|_, token| token.session_id != session_id);
        }
        active
    };

    let connected = if let Some(receipt) = receipt.as_ref() {
        let claimed_at = parse_rfc3339(&receipt.claimed_at);
        let registry = WS_CLIENT_REGISTRY.read().await;
        registry
            .values()
            .filter(|info| info.authenticated)
            .filter(|info| {
                info.authenticated_client_kind
                    .as_deref()
                    .is_some_and(|client_kind| client_kind.eq_ignore_ascii_case("ios"))
            })
            .filter(|info| {
                info.authenticated_device_id.as_deref() == Some(receipt.device_id.as_str())
            })
            .filter(|info| {
                claimed_at
                    .zip(parse_rfc3339(&info.connected_at))
                    .is_some_and(|(claimed_at, connected_at)| connected_at >= claimed_at)
            })
            .max_by(|left, right| {
                left.last_seen_at
                    .cmp(&right.last_seen_at)
                    .then_with(|| left.connected_at.cmp(&right.connected_at))
            })
            .cloned()
    } else {
        None
    };

    Some(MobilePairingSessionSnapshot {
        session_id: session_id.to_string(),
        state: if session_info.state == "failed" && !retry_grace_active {
            "failed".to_string()
        } else if session_info.state == "expired" {
            "expired".to_string()
        } else if parse_rfc3339(&session_info.expires_at)
            .map(|expires_at| expires_at <= now)
            .unwrap_or(true)
        {
            "expired".to_string()
        } else if connected.is_some() {
            "connected".to_string()
        } else if receipt.is_some() {
            "claimed".to_string()
        } else {
            "pending".to_string()
        },
        expires_at: session_info.expires_at,
        device_id: receipt.as_ref().map(|value| value.device_id.clone()),
        device_name: receipt.as_ref().map(|value| value.device_name.clone()),
        client_kind: receipt.as_ref().map(|value| value.client_kind.clone()),
        claimed_at: receipt.as_ref().map(|value| value.claimed_at.clone()),
        connected_at: connected.as_ref().map(|value| value.connected_at.clone()),
        selected_transport_mode: connected
            .as_ref()
            .and_then(|value| value.selected_transport_mode.clone()),
    })
}

/// GET /api/mobile/pairing/sessions/:session_id — authenticated, redacted session progress.
async fn handle_api_mobile_pairing_session(
    Path(session_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_PAIRING_ISSUE, "missing_scope_pairing_issue")
            .await
    {
        return response;
    }

    let Some(snapshot) = mobile_pairing_session_snapshot(&session_id).await else {
        return json_error_response(StatusCode::NOT_FOUND, "pairing_session_not_found");
    };
    Json(serde_json::json!({
        "ok": true,
        "session": snapshot,
    }))
    .into_response()
}

/// GET /api/mobile/pairing/status — read-only pairing diagnostics (no token generation)
async fn handle_api_mobile_pairing_status(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    let started_at = std::time::Instant::now();
    let public_request = is_public_bridge_request(&headers);
    let requires_device_auth = public_route_requires_auth(&headers);
    log::info!(
        "[Bridge][MobilePairing] status_start port={} public_request={} requires_auth={}",
        state.port,
        public_request,
        requires_device_auth
    );
    let trusted_internal = trusted_internal_capability(&headers);
    let auth_principal = if trusted_internal {
        None
    } else {
        match authenticate_bridge_headers_result(&headers).await {
            Ok(principal) => principal,
            Err(error) => return bridge_auth_error_response(&error),
        }
    };
    if !trusted_internal {
        if let Some((status, error)) = status_read_full_diagnostics_denial(
            auth_principal.as_ref(),
            public_request,
            requires_device_auth,
        ) {
            log::warn!(
                "[Bridge][MobilePairing] status_denied status={} error={} elapsed_ms={}",
                status,
                error,
                started_at.elapsed().as_millis()
            );
            return json_error_response(status, error);
        }
    }
    let redact_public_anonymous = public_request && auth_principal.is_none();

    let result = build_pairing_candidates_bounded(state.port, "status").await;
    if redact_public_anonymous {
        log::info!(
            "[Bridge][MobilePairing] status_done redacted=true transport={} candidate_count={} elapsed_ms={}",
            result.primary.transport_mode,
            result.candidates.len(),
            started_at.elapsed().as_millis()
        );
        return Json(build_redacted_pairing_status_value(&result)).into_response();
    }

    let token_count = MOBILE_PAIRING_TOKENS.read().await.len();
    let formal_route = formal_mobile_route_status_from_candidates(&result);
    log::info!(
        "[Bridge][MobilePairing] status_done redacted=false transport={} candidate_count={} token_count={} elapsed_ms={}",
        result.primary.transport_mode,
        result.candidates.len(),
        token_count,
        started_at.elapsed().as_millis()
    );
    Json(serde_json::json!({
        "ok": true,
        "transport_mode": result.primary.transport_mode,
        "base_url": result.primary.base_url,
        "ws_url": result.primary.ws_url,
        "tailscale_source": result.tailscale_source,
        "candidates": result.candidates,
        "warning": result.primary.warning,
        "token_count": token_count,
        "formal_route": formal_route,
        "capabilities": {
            "quick_tunnel_test": quick_tunnel_test_capability_enabled(),
        },
    }))
    .into_response()
}

fn build_redacted_pairing_status_value(result: &PairingCandidatesResult) -> serde_json::Value {
    let public_tunnel_healthy = result
        .candidates
        .iter()
        .find(|candidate| candidate.transport_mode == "public_tunnel")
        .map(|candidate| candidate.health == "healthy" && !candidate.disabled)
        .unwrap_or(false);
    serde_json::json!({
        "ok": true,
        "transport_mode": result.primary.transport_mode,
        "public_tunnel": {
            "healthy": public_tunnel_healthy,
        },
        "warning": result.primary.warning,
    })
}

#[derive(Debug, Deserialize)]
struct FilesQuery {
    project_path: String,
    #[serde(default)]
    max_depth: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct CreateDirectoryRequest {
    parent_path: String,
    name: String,
}

fn explicit_file_list_roots_for_principal_at(
    path: &FilePath,
    principal: Option<&AuthPrincipal>,
) -> Vec<PathBuf> {
    let Some(principal) = principal.filter(|principal| {
        principal.client_kind.eq_ignore_ascii_case("ios") && !principal.device_id.trim().is_empty()
    }) else {
        return Vec::new();
    };

    match load_paired_device_store_at(path) {
        Ok(store) => store
            .devices
            .into_iter()
            .find(|device| device.revoked_at.is_none() && device.device_id == principal.device_id)
            .map(|device| {
                device
                    .file_browser_roots
                    .into_iter()
                    .map(PathBuf::from)
                    .flat_map(file_list_browser_roots_for_known_root)
                    .collect()
            })
            .unwrap_or_default(),
        Err(error) => {
            log::warn!(
                "[Bridge][Files] failed to load explicit roots device_id={} error={}",
                principal.device_id,
                error
            );
            Vec::new()
        }
    }
}

fn explicit_file_list_roots_for_principal(principal: Option<&AuthPrincipal>) -> Vec<PathBuf> {
    explicit_file_list_roots_for_principal_at(&paired_devices_path(), principal)
}

fn update_paired_device_file_roots_at(
    path: &FilePath,
    device_id: &str,
    roots: Vec<String>,
) -> Result<bool, String> {
    mutate_paired_device_store_at(path, |store| {
        let Some(device) = store
            .devices
            .iter_mut()
            .find(|device| device.device_id == device_id && device.revoked_at.is_none())
        else {
            return (false, false);
        };
        let changed = device.file_browser_roots != roots;
        device.file_browser_roots = roots;
        (true, changed)
    })
}

async fn known_file_list_roots(principal: Option<&AuthPrincipal>) -> Vec<PathBuf> {
    let mut roots = explicit_file_list_roots_for_principal(principal);
    {
        let registry = ACTIVE_SESSION_REGISTRY.read().await;
        roots.extend(
            registry
                .values()
                .map(|entry| entry.project_path.trim())
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
                .flat_map(file_list_browser_roots_for_known_root),
        );
    }

    let mut window_registry = crate::ui::window_registry::WindowRegistry::load();
    roots.extend(
        window_registry
            .get_all_instances()
            .into_iter()
            .map(|instance| instance.project_path.trim().to_string())
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
            .flat_map(file_list_browser_roots_for_known_root),
    );
    roots
}

async fn allowed_file_list_roots(principal: Option<&AuthPrincipal>) -> Vec<PathBuf> {
    canonical_file_list_roots(&known_file_list_roots(principal).await)
}

async fn public_file_list_browser_root(
    canonical_project_path: &std::path::Path,
    principal: Option<&AuthPrincipal>,
) -> Option<PathBuf> {
    file_list_browser_root_for_path(
        canonical_project_path,
        &allowed_file_list_roots(principal).await,
    )
}

async fn handle_get_paired_device_file_roots(headers: HeaderMap) -> Response {
    if !trusted_internal_capability(&headers) {
        return json_error_response(
            StatusCode::FORBIDDEN,
            "desktop_file_root_management_required",
        );
    }

    let store = match load_paired_device_store_at(&paired_devices_path()) {
        Ok(store) => store,
        Err(error) => {
            log::warn!("[Bridge][Files] failed to list paired devices: {}", error);
            return json_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "paired_device_store_unavailable",
            );
        }
    };
    let mut devices = store
        .devices
        .into_iter()
        .filter(|device| device.revoked_at.is_none())
        .map(|device| PairedDeviceFileRootsSummary {
            device_id: device.device_id,
            device_name: device.device_name,
            client_kind: device.client_kind,
            created_at: device.created_at,
            last_seen_at: device.last_seen_at,
            file_browser_roots: device.file_browser_roots,
        })
        .collect::<Vec<_>>();
    devices.sort_by(|left, right| right.last_seen_at.cmp(&left.last_seen_at));

    Json(serde_json::json!({ "ok": true, "devices": devices })).into_response()
}

async fn handle_update_paired_device_file_roots(
    headers: HeaderMap,
    Json(request): Json<UpdatePairedDeviceFileRootsRequest>,
) -> Response {
    if !trusted_internal_capability(&headers) {
        return json_error_response(
            StatusCode::FORBIDDEN,
            "desktop_file_root_management_required",
        );
    }

    let device_id = request.device_id.trim();
    if device_id.is_empty() {
        return json_error_response(StatusCode::BAD_REQUEST, "missing_device_id");
    }
    let roots = match normalize_file_browser_roots(&request.roots) {
        Ok(roots) => roots,
        Err(error) => return json_error_response(StatusCode::BAD_REQUEST, error),
    };
    let updated = match update_paired_device_file_roots_at(
        &paired_devices_path(),
        device_id,
        roots.clone(),
    ) {
        Ok(updated) => updated,
        Err(error) => {
            log::warn!(
                "[Bridge][Files] failed to update roots device_id={} error={}",
                device_id,
                error
            );
            return json_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "paired_device_store_unavailable",
            );
        }
    };
    if !updated {
        return json_error_response(StatusCode::NOT_FOUND, "paired_device_not_found");
    }

    Json(serde_json::json!({
        "ok": true,
        "device_id": device_id,
        "roots": roots,
    }))
    .into_response()
}

async fn handle_get_file_roots(headers: HeaderMap) -> Response {
    let principal =
        match authorize_public_route_scope(&headers, SCOPE_FILE_LIST, "missing_scope_file_list")
            .await
        {
            Ok(principal) => principal,
            Err(response) => return response,
        };
    let roots = allowed_file_list_roots(principal.as_ref()).await;
    let roots = roots
        .into_iter()
        .map(|root| root.to_string_lossy().to_string())
        .collect::<Vec<_>>();

    Json(serde_json::json!({
        "ok": true,
        "roots": roots,
        "preferred_root": roots.first(),
    }))
    .into_response()
}

async fn handle_get_files(
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<FilesQuery>,
) -> Response {
    let principal =
        match authorize_public_route_scope(&headers, SCOPE_FILE_LIST, "missing_scope_file_list")
            .await
        {
            Ok(principal) => principal,
            Err(response) => return response,
        };

    let project_path = std::path::Path::new(&query.project_path);
    let Ok(canonical_project_path) = project_path.canonicalize() else {
        return Json(serde_json::json!({ "error": "Invalid project path", "files": [] }))
            .into_response();
    };
    if !canonical_project_path.is_dir() {
        return Json(serde_json::json!({ "error": "Invalid project path", "files": [] }))
            .into_response();
    }
    let Some(browser_root) =
        public_file_list_browser_root(&canonical_project_path, principal.as_ref()).await
    else {
        return json_error_response(StatusCode::FORBIDDEN, "file_list_root_not_allowed");
    };

    let max_depth = bounded_file_list_depth(query.max_depth);
    let mut files = Vec::new();

    fn collect_files(
        dir: &std::path::Path,
        base: &std::path::Path,
        depth: usize,
        max_depth: usize,
        files: &mut Vec<String>,
    ) {
        if depth > max_depth {
            return;
        }

        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };

        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // 跳过隐藏文件和常见忽略目录
            if name.starts_with('.')
                || name == "node_modules"
                || name == "target"
                || name == "dist"
                || name == "__pycache__"
            {
                continue;
            }

            if let Ok(rel_path) = path.strip_prefix(base) {
                let rel_str = rel_path.to_string_lossy().to_string();
                if file_type.is_dir() {
                    files.push(format!("{}/", rel_str));
                    collect_files(&path, base, depth + 1, max_depth, files);
                } else {
                    files.push(rel_str);
                }
            }
        }
    }

    collect_files(
        &canonical_project_path,
        &canonical_project_path,
        0,
        max_depth,
        &mut files,
    );
    files.sort();

    Json(serde_json::json!({
        "files": files,
        "project_path": query.project_path,
        "browser_root": browser_root.to_string_lossy(),
    }))
    .into_response()
}

async fn handle_create_directory(
    headers: HeaderMap,
    Json(request): Json<CreateDirectoryRequest>,
) -> Response {
    let principal =
        match authorize_public_route_scope(&headers, SCOPE_FILE_LIST, "missing_scope_file_list")
            .await
        {
            Ok(principal) => principal,
            Err(response) => return response,
        };

    let Ok(directory_name) = sanitize_created_directory_name(&request.name) else {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_directory_name");
    };

    let parent_path = std::path::Path::new(&request.parent_path);
    let Ok(canonical_parent_path) = parent_path.canonicalize() else {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_parent_path");
    };
    if !canonical_parent_path.is_dir() {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_parent_path");
    }
    let allowed_roots = allowed_file_list_roots(principal.as_ref()).await;
    if !canonical_path_is_within_allowed_roots(&canonical_parent_path, &allowed_roots) {
        return json_error_response(StatusCode::FORBIDDEN, "file_list_root_not_allowed");
    }

    let target_path = canonical_parent_path.join(&directory_name);
    if target_path.exists() {
        return json_error_response(StatusCode::CONFLICT, "directory_already_exists");
    }

    if let Err(error) = std::fs::create_dir(&target_path) {
        log::warn!(
            "[Bridge][Files] create directory failed parent={} name={} error={}",
            canonical_parent_path.display(),
            directory_name,
            error
        );
        return json_error_response(StatusCode::INTERNAL_SERVER_ERROR, "create_directory_failed");
    }

    let created_path = target_path
        .canonicalize()
        .unwrap_or_else(|_| target_path.clone());
    if !canonical_path_is_within_allowed_roots(&created_path, &allowed_roots) {
        let _ = std::fs::remove_dir(&created_path);
        return json_error_response(StatusCode::FORBIDDEN, "file_list_root_not_allowed");
    }
    Json(serde_json::json!({
        "ok": true,
        "path": created_path.to_string_lossy(),
    }))
    .into_response()
}

async fn handle_get_windows(headers: HeaderMap) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_SESSION_READ, "missing_scope_session_read")
            .await
    {
        return response;
    }

    let mut registry = crate::ui::window_registry::WindowRegistry::load();
    let instances = registry.get_all_instances();
    Json(serde_json::json!({ "instances": instances })).into_response()
}

// ============================================================
// Mobile API endpoints
// ============================================================

fn prompt_library_path() -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    std::path::Path::new(&home)
        .join(".cunzhi")
        .join("prompt-library.json")
}

fn read_prompt_library() -> Result<serde_json::Value, String> {
    let path = prompt_library_path();
    if !path.exists() {
        return Ok(serde_json::json!({ "version": 1, "items": [] }));
    }
    let content =
        std::fs::read_to_string(&path).map_err(|error| format!("读取提示词库失败: {}", error))?;
    let value: serde_json::Value =
        serde_json::from_str(&content).map_err(|error| format!("解析提示词库失败: {}", error))?;
    if !value.get("items").is_some_and(serde_json::Value::is_array) {
        return Err("提示词库格式无效: items 不是数组".to_string());
    }
    Ok(value)
}

fn read_ghost_suggestions() -> serde_json::Value {
    crate::ghost_suggestions::load_store_value()
}

fn write_prompt_library(data: &serde_json::Value) -> Result<(), String> {
    let path = prompt_library_path();
    if let Ok(existing) = std::fs::read(&path) {
        let backup_path = path.with_extension("json.bak");
        atomic_write_private_file(&backup_path, &existing)
            .map_err(|error| format!("备份提示词库失败: {}", error))?;
    }
    let json = serde_json::to_string_pretty(data).map_err(|e| format!("序列化失败: {}", e))?;
    atomic_write_private_file(&path, json.as_bytes())
        .map_err(|error| format!("原子写入提示词库失败: {}", error))
}

/// GET /api/mcp-tools — 返回 MCP 工具列表
async fn handle_api_mcp_tools(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_CONFIG_READ, "missing_scope_config_read").await
    {
        return response;
    }

    use crate::config::AppState;
    if let Some(app_handle) = state.app_handle.as_ref() {
        let state = app_handle.state::<AppState>();
        match crate::mcp::commands::get_mcp_tools_config(state).await {
            Ok(tools) => Json(serde_json::json!({ "tools": tools })).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response(),
        }
    } else {
        match crate::config::load_standalone_config() {
            Ok(config) => {
                let tools = crate::mcp::commands::build_mcp_tools_config(&config);
                Json(serde_json::json!({ "tools": tools })).into_response()
            }
            Err(error) => json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("读取配置失败: {error}"),
            ),
        }
    }
}

#[derive(Debug, Deserialize)]
struct McpToolUpdateRequest {
    tool_id: String,
    enabled: bool,
}

/// POST /api/mcp-tools — 更新单个 MCP 工具的真实启用状态。
async fn handle_api_mcp_tools_post(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(request): Json<McpToolUpdateRequest>,
) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_CONFIG_WRITE, "missing_scope_config_write")
            .await
    {
        return response;
    }

    if !crate::constants::mcp::is_valid_tool_id(&request.tool_id) {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_mcp_tool_id");
    }

    let Some(app_handle) = state.app_handle.as_ref() else {
        if request.tool_id == crate::constants::mcp::TOOL_ZHI && !request.enabled {
            return json_error_response(StatusCode::BAD_REQUEST, "iterate_tool_required");
        }
        let mut config = match crate::config::load_standalone_config() {
            Ok(config) => config,
            Err(error) => {
                return json_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("读取配置失败: {error}"),
                );
            }
        };
        config
            .mcp_config
            .tools
            .insert(request.tool_id, request.enabled);
        return match crate::config::save_standalone_config(&config) {
            Ok(()) => Json(serde_json::json!({ "success": true })).into_response(),
            Err(error) => json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                &format!("保存配置失败: {error}"),
            ),
        };
    };

    let previous_enabled = {
        let app_state = app_handle.state::<crate::config::AppState>();
        let config = match app_state.config.lock() {
            Ok(config) => config,
            Err(error) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("获取配置失败: {}", error) })),
                )
                    .into_response();
            }
        };
        config.mcp_config.tools.get(&request.tool_id).copied()
    };
    let tool_id = request.tool_id;
    match crate::mcp::commands::set_mcp_tool_enabled(
        tool_id.clone(),
        request.enabled,
        app_handle.state::<crate::config::AppState>(),
        app_handle.clone(),
    )
    .await
    {
        Ok(()) => {
            let _ = app_handle.emit("config_reloaded", ());
            Json(serde_json::json!({ "success": true })).into_response()
        }
        Err(error) => {
            let app_state = app_handle.state::<crate::config::AppState>();
            if let Ok(mut config) = app_state.config.lock() {
                match previous_enabled {
                    Some(enabled) => {
                        config.mcp_config.tools.insert(tool_id, enabled);
                    }
                    None => {
                        config.mcp_config.tools.remove(&tool_id);
                    }
                }
            }
            let status = if error.contains("无法禁用") {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (status, Json(serde_json::json!({ "error": error }))).into_response()
        }
    }
}

/// GET /api/prompt-library — 返回提示词库
async fn handle_api_prompt_library_get(headers: HeaderMap) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_PROMPT_LIBRARY_READ,
        "missing_scope_prompt_library_read",
    )
    .await
    {
        return response;
    }

    match read_prompt_library() {
        Ok(library) => Json(library).into_response(),
        Err(error) => json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    }
}

/// GET /api/promptor-library — 返回 Promptor 的提示词与模式安全投影。
async fn handle_api_promptor_library_get(headers: HeaderMap) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_PROMPT_LIBRARY_READ,
        "missing_scope_prompt_library_read",
    )
    .await
    {
        return response;
    }

    match read_promptor_library() {
        Ok(library) => Json(library).into_response(),
        Err(error) => {
            log::warn!("[Bridge][Promptor] library unavailable: {error}");
            json_error_response(
                StatusCode::SERVICE_UNAVAILABLE,
                "promptor_library_unavailable",
            )
        }
    }
}

/// GET /api/ghost-suggestions — 返回幽灵补全词表
async fn handle_api_ghost_suggestions_get(headers: HeaderMap) -> Response {
    if let Err(response) = authorize_ghost_suggestions_read(&headers).await {
        return response;
    }

    Json(read_ghost_suggestions()).into_response()
}

/// POST /api/ghost-suggestions — 按当前 schema 追加或更新一个幽灵补全词
async fn handle_api_ghost_suggestions_post(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(body): Json<crate::ghost_suggestions::UpsertGhostSuggestionRequest>,
) -> Response {
    let principal = match authorize_ghost_suggestions_write(&headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let key = body.key.clone();
    match crate::ghost_suggestions::upsert_ghost_suggestion(body) {
        Ok(store) => {
            log_ghost_suggestions_write(principal.as_ref(), "upsert", &key);
            ghost_suggestions_write_success(&state, store)
        }
        Err(error) => ghost_suggestions_write_error_response(error),
    }
}

/// PUT /api/ghost-suggestions — 原子替换完整词表，用于批量变更与快照撤销
async fn handle_api_ghost_suggestions_put(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(body): Json<crate::ghost_suggestions::ReplaceGhostSuggestionsRequest>,
) -> Response {
    let principal = match authorize_ghost_suggestions_write(&headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let target = format!("{} items", body.suggestions.len());
    match crate::ghost_suggestions::replace_ghost_suggestions(body) {
        Ok(store) => {
            log_ghost_suggestions_write(principal.as_ref(), "replace", &target);
            ghost_suggestions_write_success(&state, store)
        }
        Err(error) => ghost_suggestions_write_error_response(error),
    }
}

/// PATCH /api/ghost-suggestions/:id — 按 id 编辑幽灵补全词
async fn handle_api_ghost_suggestions_patch(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<crate::ghost_suggestions::UpdateGhostSuggestionRequest>,
) -> Response {
    let principal = match authorize_ghost_suggestions_write(&headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let target = body.key.clone().unwrap_or_else(|| id.clone());
    match crate::ghost_suggestions::update_ghost_suggestion(&id, body) {
        Ok(store) => {
            log_ghost_suggestions_write(principal.as_ref(), "patch", &target);
            ghost_suggestions_write_success(&state, store)
        }
        Err(error) => ghost_suggestions_write_error_response(error),
    }
}

/// DELETE /api/ghost-suggestions/:id — 按 id 删除幽灵补全词
async fn handle_api_ghost_suggestions_delete(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<crate::ghost_suggestions::RemoveGhostSuggestionRequest>,
) -> Response {
    let principal = match authorize_ghost_suggestions_write(&headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    match crate::ghost_suggestions::remove_ghost_suggestion(&id, body) {
        Ok(store) => {
            log_ghost_suggestions_write(principal.as_ref(), "delete", &id);
            ghost_suggestions_write_success(&state, store)
        }
        Err(error) => ghost_suggestions_write_error_response(error),
    }
}

/// POST /api/ghost-suggestions/reorder — 按完整 id 顺序重排幽灵补全词
async fn handle_api_ghost_suggestions_reorder(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(body): Json<crate::ghost_suggestions::ReorderGhostSuggestionsRequest>,
) -> Response {
    let principal = match authorize_ghost_suggestions_write(&headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let target = format!("{} ids", body.ids.len());
    match crate::ghost_suggestions::reorder_ghost_suggestions(body) {
        Ok(store) => {
            log_ghost_suggestions_write(principal.as_ref(), "reorder", &target);
            ghost_suggestions_write_success(&state, store)
        }
        Err(error) => ghost_suggestions_write_error_response(error),
    }
}

/// GET /api/ghost-suggestion-learning — 返回磁盘权威的自动学习计数。
async fn handle_api_ghost_suggestion_learning_get(headers: HeaderMap) -> Response {
    if let Err(response) = authorize_ghost_suggestions_read(&headers).await {
        return response;
    }

    match crate::ghost_suggestion_learning::load_store() {
        Ok(state) => Json(serde_json::json!({ "state": state })).into_response(),
        Err(error) => json_error_response(StatusCode::SERVICE_UNAVAILABLE, &error),
    }
}

/// POST /api/ghost-suggestion-learning — 只记录过滤后的词条和计数。
async fn handle_api_ghost_suggestion_learning_post(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(body): Json<crate::ghost_suggestion_learning::RecordGhostSuggestionLearningRequest>,
) -> Response {
    let principal = match authorize_ghost_suggestions_write(&headers).await {
        Ok(principal) => principal,
        Err(response) => return response,
    };
    let term_count = body.terms.len();
    let event = body.event.clone();
    match crate::ghost_suggestion_learning::record_learning(body) {
        Ok(result) => {
            if !result.promoted_keys.is_empty() {
                if let Some(app) = state.app_handle.as_ref() {
                    broadcast_ghost_suggestions_changed(app, result.ghost_suggestions.clone());
                } else {
                    broadcast_ghost_suggestions_changed_to_bridge(result.ghost_suggestions.clone());
                }
            }
            log_ghost_suggestions_write(
                principal.as_ref(),
                "learn",
                &format!("event={event} terms={term_count}"),
            );
            Json(result).into_response()
        }
        Err(error) => json_error_response(StatusCode::BAD_REQUEST, &error),
    }
}

/// GET /api/speech-muscle-memory — 返回共享语音肌肉记忆库
async fn handle_api_speech_muscle_memory_get(headers: HeaderMap) -> Response {
    if let Err(response) = authorize_speech_memory_access(
        &headers,
        SCOPE_SPEECH_MEMORY_READ,
        "missing_scope_speech_memory_read",
    )
    .await
    {
        return response;
    }

    match speech_memory::load_entries() {
        Ok(entries) => Json(serde_json::json!({ "entries": entries })).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error, "entries": [] })),
        )
            .into_response(),
    }
}

/// POST /api/speech-muscle-memory — 保存共享语音肌肉记忆库
async fn handle_api_speech_muscle_memory_post(
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(response) = authorize_speech_memory_access(
        &headers,
        SCOPE_SPEECH_MEMORY_WRITE,
        "missing_scope_speech_memory_write",
    )
    .await
    {
        return response;
    }

    let entries = body
        .get("entries")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    match speech_memory::save_entries(entries) {
        Ok(saved) => Json(serde_json::json!({ "ok": true, "entries": saved })).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": error, "entries": [] })),
        )
            .into_response(),
    }
}

/// GET /api/speech-correction-memory — 返回共享语音纠错记忆库
async fn handle_api_speech_correction_memory_get(headers: HeaderMap) -> Response {
    if let Err(response) = authorize_speech_memory_access(
        &headers,
        SCOPE_SPEECH_MEMORY_READ,
        "missing_scope_speech_memory_read",
    )
    .await
    {
        return response;
    }

    match speech_memory::load_correction_entries() {
        Ok(entries) => Json(serde_json::json!({ "entries": entries })).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": error, "entries": [] })),
        )
            .into_response(),
    }
}

/// POST /api/speech-correction-memory — 保存共享语音纠错记忆库
async fn handle_api_speech_correction_memory_post(
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(response) = authorize_speech_memory_access(
        &headers,
        SCOPE_SPEECH_MEMORY_WRITE,
        "missing_scope_speech_memory_write",
    )
    .await
    {
        return response;
    }

    let entries = body
        .get("entries")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();

    match speech_memory::save_correction_entries(entries) {
        Ok(saved) => Json(serde_json::json!({ "ok": true, "entries": saved })).into_response(),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "ok": false, "error": error, "entries": [] })),
        )
            .into_response(),
    }
}

/// GET /api/speech-vocabulary — 返回磁盘权威的个人语音词典。
async fn handle_api_speech_vocabulary_get(headers: HeaderMap) -> Response {
    if let Err(response) = authorize_speech_memory_access(
        &headers,
        SCOPE_SPEECH_MEMORY_READ,
        "missing_scope_speech_memory_read",
    )
    .await
    {
        return response;
    }

    match speech_memory::load_vocabulary_store() {
        Ok(store) => Json(serde_json::json!({
            "version": store.version,
            "updated_at": store.updated_at,
            "entries": store.entries,
        }))
        .into_response(),
        Err(error) => Json(serde_json::json!({
            "error": error,
            "version": 1,
            "entries": [],
        }))
        .into_response(),
    }
}

/// POST /api/speech-vocabulary — 只接收过滤后的词条，不接收或保存整段转写。
async fn handle_api_speech_vocabulary_post(
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(response) = authorize_speech_memory_access(
        &headers,
        SCOPE_SPEECH_MEMORY_WRITE,
        "missing_scope_speech_memory_write",
    )
    .await
    {
        return response;
    }

    let terms = body
        .get("terms")
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let merge_only = body.get("mode").and_then(|value| value.as_str()) == Some("merge");
    let result = if merge_only {
        speech_memory::merge_vocabulary_terms(terms)
    } else {
        speech_memory::record_vocabulary_terms(terms)
    };

    match result {
        Ok(store) => Json(serde_json::json!({
            "ok": true,
            "version": store.version,
            "updated_at": store.updated_at,
            "entries": store.entries,
        }))
        .into_response(),
        Err(error) => Json(serde_json::json!({
            "ok": false,
            "error": error,
            "entries": [],
        }))
        .into_response(),
    }
}

/// POST /api/prompt-library — 添加或更新提示词
async fn handle_api_prompt_library_post(
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_PROMPT_LIBRARY_WRITE,
        "missing_scope_prompt_library_write",
    )
    .await
    {
        return response;
    }

    let mut library = match read_prompt_library() {
        Ok(library) => library,
        Err(error) => return json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let Some(new_item) = body.as_object() else {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_prompt_item");
    };
    let id = new_item
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let name = new_item
        .get("name")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    let content = new_item
        .get("content")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim();
    let category = new_item
        .get("category")
        .and_then(|value| value.as_str())
        .unwrap_or("未分类");
    if id.is_empty()
        || name.is_empty()
        || content.is_empty()
        || id.chars().count() > 256
        || name.chars().count() > 512
        || content.chars().count() > 262_144
        || category.chars().count() > 512
    {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_prompt_item");
    }
    let items = library.get_mut("items").and_then(|v| v.as_array_mut());

    match items {
        Some(items) => {
            if let Some(pos) = items
                .iter()
                .position(|item| item.get("id").and_then(|value| value.as_str()) == Some(id))
            {
                items[pos] = body.clone();
            } else {
                items.push(body.clone());
            }
            match write_prompt_library(&library) {
                Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e })),
                )
                    .into_response(),
            }
        }
        None => json_error_response(StatusCode::INTERNAL_SERVER_ERROR, "invalid_library_format"),
    }
}

#[derive(Debug, Deserialize)]
struct DeleteQuery {
    id: Option<String>,
    all: Option<bool>,
}

/// DELETE /api/prompt-library?id=xxx — 删除提示词
async fn handle_api_prompt_library_delete(
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<DeleteQuery>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_PROMPT_LIBRARY_WRITE,
        "missing_scope_prompt_library_write",
    )
    .await
    {
        return response;
    }

    let mut library = match read_prompt_library() {
        Ok(library) => library,
        Err(error) => return json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    if let Some(items) = library.get_mut("items").and_then(|v| v.as_array_mut()) {
        if query.all == Some(true) {
            items.clear();
        } else if let Some(id) = query.id.as_deref() {
            items.retain(|i| i.get("id").and_then(|v| v.as_str()) != Some(id));
        } else {
            return json_error_response(StatusCode::BAD_REQUEST, "prompt_id_is_required");
        }
        match write_prompt_library(&library) {
            Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response(),
        }
    } else {
        json_error_response(StatusCode::INTERNAL_SERVER_ERROR, "invalid_library_format")
    }
}

/// POST /api/import-prompts-dir — 从目录导入提示词
async fn handle_api_import_prompts_dir(
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    let principal = match authorize_public_route_scope(
        &headers,
        SCOPE_PROMPT_LIBRARY_WRITE,
        "missing_scope_prompt_library_write",
    )
    .await
    {
        Ok(principal) => principal,
        Err(response) => return response,
    };

    let path = body.get("path").and_then(|v| v.as_str()).unwrap_or("");
    if path.is_empty() {
        return json_error_response(StatusCode::BAD_REQUEST, "path_is_required");
    }
    let dir = std::path::Path::new(path);
    let Ok(canonical_dir) = dir.canonicalize() else {
        return json_error_response(StatusCode::NOT_FOUND, "directory_not_found");
    };
    if !canonical_dir.is_dir() {
        return json_error_response(StatusCode::NOT_FOUND, "directory_not_found");
    }
    if public_file_list_browser_root(&canonical_dir, principal.as_ref())
        .await
        .is_none()
    {
        return json_error_response(StatusCode::FORBIDDEN, "file_list_root_not_allowed");
    }

    let mut library = match read_prompt_library() {
        Ok(library) => library,
        Err(error) => return json_error_response(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let items = library.get_mut("items").and_then(|v| v.as_array_mut());
    let items = match items {
        Some(i) => i,
        None => {
            return json_error_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                "invalid_library_format",
            );
        }
    };

    let existing_names: std::collections::HashSet<String> = items
        .iter()
        .filter_map(|i| {
            i.get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect();

    let mut imported = 0u32;
    let mut skipped = 0u32;
    let mut failed_files: Vec<String> = Vec::new();

    if let Ok(entries) = std::fs::read_dir(&canonical_dir) {
        for entry in entries.flatten() {
            let file_path = entry.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("txt") {
                continue;
            }
            let Ok(metadata) = std::fs::symlink_metadata(&file_path) else {
                failed_files.push(file_path.display().to_string());
                continue;
            };
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                failed_files.push(file_path.display().to_string());
                continue;
            }
            let Ok(canonical_file) = file_path.canonicalize() else {
                failed_files.push(file_path.display().to_string());
                continue;
            };
            if canonical_file.parent() != Some(canonical_dir.as_path()) {
                failed_files.push(file_path.display().to_string());
                continue;
            }
            let category = file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("未分类")
                .to_string();

            match std::fs::read_to_string(&canonical_file) {
                Ok(content) => {
                    let lines: Vec<&str> = content
                        .lines()
                        .map(|l| l.trim())
                        .filter(|l| !l.is_empty())
                        .collect();
                    let mut i = 0;
                    while i < lines.len() {
                        let name = lines[i].to_string();
                        let prompt_content = if i + 1 < lines.len() {
                            lines[i + 1].to_string()
                        } else {
                            name.clone()
                        };
                        i += 2;
                        // Skip blank separator lines
                        while i < lines.len() && lines[i].is_empty() {
                            i += 1;
                        }

                        if existing_names.contains(&name) {
                            skipped += 1;
                            continue;
                        }

                        let id = format!(
                            "prompt_import_{}",
                            chrono::Utc::now().timestamp_millis() as u64 + imported as u64
                        );
                        items.push(serde_json::json!({
                            "id": id,
                            "name": name,
                            "content": prompt_content,
                            "category": category,
                        }));
                        imported += 1;
                    }
                }
                Err(_) => {
                    failed_files.push(file_path.display().to_string());
                }
            }
        }
    }

    match write_prompt_library(&library) {
        Ok(_) => Json(serde_json::json!({
            "ok": true,
            "imported": imported,
            "skipped": skipped,
            "failed_files": failed_files,
        }))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

/// GET /mobile — 移动端优化主页面
async fn handle_mobile_page() -> Response {
    // Normal and release runs must serve the source embedded in this Bridge.
    // An explicit override keeps local HTML iteration possible without letting
    // a developer's absolute path silently change production behavior.
    let content = std::env::var("ITERATE_MOBILE_HTML_DEV_PATH")
        .ok()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .and_then(|path| match std::fs::read_to_string(&path) {
            Ok(content) => Some(content),
            Err(error) => {
                log::warn!(
                    "[Bridge] 无法读取 ITERATE_MOBILE_HTML_DEV_PATH={}：{}；回退内嵌 Home",
                    path,
                    error
                );
                None
            }
        })
        .unwrap_or_else(|| include_str!("../../../mobile.html").to_string());

    (
        [
            (
                header::CACHE_CONTROL,
                "no-store, no-cache, must-revalidate, max-age=0",
            ),
            (header::PRAGMA, "no-cache"),
            (header::EXPIRES, "0"),
        ],
        Html(content),
    )
        .into_response()
}

/// 组装主页面 Markdown 内容
async fn build_main_page_markdown(app_handle: &AppHandle, tab: &str) -> String {
    use crate::config::AppState;
    match tab {
        "tools" => {
            let state: tauri::State<'_, AppState> = app_handle.state::<AppState>();
            let tool_info = {
                let tool_names: &[(&str, &str, &str)] = &[
                    ("zhi", "iterate", "智能代码审查交互工具（L0 协调者）"),
                    (
                        "ji",
                        "记忆管理",
                        "全局记忆管理工具，支持回忆/记忆/沉淀/摘要",
                    ),
                    ("sou", "代码搜索", "语义代码搜索工具，支持增量索引"),
                    ("acemcp", "AceMCP", "MCP 代理转发工具"),
                ];
                if let Ok(config) = state.config.lock() {
                    tool_names
                        .iter()
                        .map(|(id, name, desc)| {
                            let enabled = config.mcp_config.tools.get(*id).copied().unwrap_or(true);
                            (enabled, name.to_string(), desc.to_string())
                        })
                        .collect::<Vec<_>>()
                } else {
                    vec![]
                }
            };
            if tool_info.is_empty() {
                "# 🔧 MCP 工具\n\n无法获取工具配置".to_string()
            } else {
                let mut md = String::from("# 🔧 MCP 工具\n\n");
                for (enabled, name, desc) in &tool_info {
                    let status = if *enabled { "🟢" } else { "⚪" };
                    md.push_str(&format!("### {} {}\n{}\n\n", status, name, desc));
                }
                md
            }
        }
        "prompts" => {
            let prompt_path = dirs::home_dir()
                .map(|h| h.join(".cunzhi/prompt-library.json"))
                .unwrap_or_default();
            if let Ok(content) = std::fs::read_to_string(&prompt_path) {
                if let Ok(items) = serde_json::from_str::<Vec<serde_json::Value>>(&content) {
                    if items.is_empty() {
                        return "# 📝 提示词库\n\n提示词库为空，请在桌面端设置中导入提示词。"
                            .to_string();
                    }
                    let mut md = format!("# 📝 提示词库（{} 条）\n\n", items.len());
                    // Group by category
                    let mut categories: std::collections::BTreeMap<
                        String,
                        Vec<&serde_json::Value>,
                    > = std::collections::BTreeMap::new();
                    for item in &items {
                        let cat = item
                            .get("category")
                            .and_then(|v| v.as_str())
                            .unwrap_or("未分类")
                            .to_string();
                        categories.entry(cat).or_default().push(item);
                    }
                    for (cat, prompts) in &categories {
                        md.push_str(&format!("## 📂 {} ({} 条)\n\n", cat, prompts.len()));
                        for p in prompts {
                            let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("未命名");
                            let content = p.get("content").and_then(|v| v.as_str()).unwrap_or("");
                            let preview: String = content.chars().take(80).collect();
                            md.push_str(&format!("**{}**\n> {}\n\n", name, preview));
                        }
                    }
                    md
                } else {
                    "# 📝 提示词库\n\n文件格式错误".to_string()
                }
            } else {
                "# 📝 提示词库\n\n提示词库为空，请在桌面端设置中导入提示词。".to_string()
            }
        }
        "settings" => "# ⚙️ 设置\n\n\
- 🎨 **主题设置** — 选择界面主题\n\
- 🔤 **字体设置** — 自定义字体系列和大小\n\
- ▶️ **继续回复设置** — 配置AI继续回复的行为\n\
- 📝 **提示词模板** — 管理快捷模板和上下文追加\n\
- 📚 **提示词库** — 管理和导入提示词集合\n\
- ⌨️ **快捷键设置** — 自定义应用快捷键绑定\n\
- 🪟 **窗口设置** — 调整窗口显示和行为\n\
- 🔔 **音频设置** — 配置音频通知和提示音\n\
- 🌐 **浏览器监控** — 监控 ChatGPT/Gemini 等 AI 完成通知\n\
- � **iterate** — 手机远程控制 / 公网访问一键开通\n\
- 🔌 **寸止端口监听** — 类似 Infinite WF 的无限对话服务\n\
- ⚙️ **配置管理** — 重新加载配置文件和管理设置\n\
- 🔄 **版本检查** — 检查应用更新\n\n\
*设置项需在桌面端修改*"
            .to_string(),
        _ => {
            // intro tab (default)
            let version = env!("CARGO_PKG_VERSION");
            format!(
                "# ∞ iterate v{version}\n\n\
🟢 **MCP 服务已启动** · 智能代码审查工具\n\n\
---\n\n\
### 💬 Zhi 智能审查工具\n\
iterate 交互系统\n\
- 智能代码审查交互\n\
- 支持文本和图片输入\n\
- 预定义选项支持\n\
- Markdown 渲染\n\
- 跨平台快捷键支持\n\n\
### 🔍 代码搜索工具\n\
智能代码检索\n\
- 语义代码搜索\n\
- 增量索引支持\n\
- 多文件类型支持\n\
- 自动索引更新\n\n\
### 🔊 音频通知系统\n\
智能音效管理\n\
- 多种内置音效\n\
- 自定义音频支持\n\n\
### ⚙️ 个性化设置\n\
全面配置选项\n\
- 深色/浅色主题\n\
- 窗口大小控制\n\
- MCP工具管理"
            )
        }
    }
}

fn remove_cached_session_entries(
    cache: &mut HashMap<String, serde_json::Value>,
    request_id: &str,
) -> bool {
    let removed_payload = cache.remove(request_id);
    let Some(payload) = removed_payload else {
        return false;
    };

    let project_path = payload
        .get("request")
        .and_then(|r| r.get("project_path"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    if let Some(path) = project_path {
        let should_remove_aux = cache
            .get(&path)
            .and_then(|value| value.get("request"))
            .and_then(|request| request.get("id"))
            .and_then(|id| id.as_str())
            .map(|cached_request_id| cached_request_id == request_id)
            .unwrap_or(false);
        if should_remove_aux {
            cache.remove(&path);
        }
    }

    true
}

fn effective_request_sync_request_id(
    request_id: Option<&str>,
    project_path: Option<&str>,
) -> Option<String> {
    let request_id = normalize_route_part(request_id)?;
    if is_registered_mcp_port_request_id(&request_id)
        && normalize_route_part(project_path).is_some()
    {
        None
    } else {
        Some(request_id)
    }
}

/// GET /api/active-sessions — 返回所有活跃的 MCP 对话（用于 iOS 活跃项目列表）
async fn handle_api_active_sessions(headers: HeaderMap) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_SESSION_READ, "missing_scope_session_read")
            .await
    {
        return response;
    }

    let mut window_registry = crate::ui::window_registry::WindowRegistry::load();
    let instances = window_registry.get_all_instances();
    let live_window_count = instances.len();
    let registry = ACTIVE_SESSION_REGISTRY.read().await;
    let sessions = build_active_session_summaries_with_focus(
        &registry,
        instances,
        window_registry.last_focused_at_by_pid(),
    );

    log::info!(
        "[Bridge] active-sessions: registry_size={}, live_windows={}, returned={}",
        registry.len(),
        live_window_count,
        sessions.len()
    );

    Json(serde_json::json!({ "sessions": sessions })).into_response()
}

/// POST /api/cleanup-session — 子进程通知主进程清除已结束对话的缓存
async fn handle_api_cleanup_session(
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_SESSION_RESPOND,
        "missing_scope_session_respond",
    )
    .await
    {
        return response;
    }

    let request_id = body.get("request_id").and_then(|v| v.as_str());
    if let Some(rid) = request_id {
        let (removed, active_removed) =
            cleanup_completed_session_by_request_id(rid, "cleanup-session").await;
        Json(serde_json::json!({ "ok": true, "removed": removed || active_removed }))
            .into_response()
    } else {
        Json(serde_json::json!({ "ok": false, "error": "missing request_id" })).into_response()
    }
}

/// GET /api/show-window — 显示并聚焦桌面端主窗口（HTTP 版本，方便测试和 iOS 调用）
#[cfg(target_os = "macos")]
async fn activate_installed_desktop_app() -> Result<&'static str, String> {
    static ACTIVATION_LOCK: Lazy<tokio::sync::Mutex<()>> =
        Lazy::new(|| tokio::sync::Mutex::new(()));
    let _guard = ACTIVATION_LOCK.lock().await;

    if crate::ipc::request_show_main_window().await.is_ok() {
        return Ok("attached_gui");
    }

    let output = tokio::process::Command::new("/usr/bin/open")
        .args([
            "-n",
            "-b",
            crate::constants::app::APP_IDENTIFIER,
            "--args",
            "--show-main-window",
        ])
        .output()
        .await
        .map_err(|error| format!("请求 LaunchServices 打开 iterate 失败: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "LaunchServices 打开 iterate 失败: status={:?}, stderr={}",
            output.status.code(),
            if stderr.is_empty() {
                "<empty>"
            } else {
                &stderr
            }
        ));
    }

    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if crate::ipc::request_show_main_window().await.is_ok() {
            return Ok("launched_gui");
        }
    }

    Err("iterate 已启动，但主 GUI 未在 5 秒内建立本地激活通道".to_string())
}

#[cfg(not(target_os = "macos"))]
async fn activate_installed_desktop_app() -> Result<&'static str, String> {
    Err("bridge-only 主窗口激活目前仅支持 macOS".to_string())
}

async fn handle_api_show_window(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_WINDOW_SHOW, "missing_scope_window_show").await
    {
        return response;
    }

    let activation = if let Some(app_handle) = state.app_handle.as_ref() {
        crate::ui::commands::activate_app_window(app_handle.clone())
            .await
            .map(|()| "embedded_gui")
    } else {
        activate_installed_desktop_app().await
    };
    match activation {
        Ok(mode) => {
            log_important!(info, "[Bridge] HTTP: 已激活并聚焦主窗口");
            Json(serde_json::json!({
                "ok": true,
                "message": "主窗口已显示",
                "mode": mode
            }))
            .into_response()
        }
        Err(error) => {
            log_important!(warn, "[Bridge] HTTP: 激活主窗口失败: {}", error);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "ok": false, "error": error })),
            )
                .into_response()
        }
    }
}

/// POST /api/open-codex-chat — 免权限打开桌面 Codex（优先按项目）
async fn handle_api_open_codex_chat(
    State(_state): State<BridgeHttpState>,
    headers: HeaderMap,
    body: Option<Json<serde_json::Value>>,
) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_WINDOW_SHOW, "missing_scope_window_show").await
    {
        return response;
    }

    let project_path = body
        .as_ref()
        .and_then(|Json(body)| body.get("project_path"))
        .and_then(|value| value.as_str())
        .map(ToOwned::to_owned);

    match crate::ui::commands::open_new_codex_chat_with_text("zhi".to_string(), project_path).await
    {
        Ok(result) => Json(serde_json::json!({
            "ok": result.ok,
            "sent": result.sent,
            "mode": result.mode,
            "message": result.message
        }))
        .into_response(),
        Err(error) => Json(serde_json::json!({
            "ok": false,
            "error": error
        }))
        .into_response(),
    }
}

fn patch_app_config(
    config: &crate::config::settings::AppConfig,
    body: &serde_json::Value,
) -> Option<crate::config::settings::AppConfig> {
    let body_obj = body.as_object()?;
    if body_obj.len() != 1 {
        return None;
    }
    let (section, section_value) = body_obj.iter().next()?;
    let mut patched = config.clone();
    match section.as_str() {
        "mobile_config" => {
            let mobile_config = section_value.as_object()?;
            if mobile_config.len() != 1
                || !mobile_config
                    .get("allow_ghost_suggestions_write")
                    .is_some_and(serde_json::Value::is_boolean)
            {
                return None;
            }
            patched.mobile_config.allow_ghost_suggestions_write = mobile_config
                .get("allow_ghost_suggestions_write")?
                .as_bool()?;
        }
        "ui_config" => {
            let values = section_value.as_object()?;
            if values.is_empty()
                || values.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "theme" | "font_config" | "always_on_top" | "window_config"
                    )
                })
            {
                return None;
            }
            if let Some(theme) = values.get("theme") {
                let theme = theme.as_str()?;
                if !crate::constants::validation::is_valid_theme(theme) {
                    return None;
                }
                patched.ui_config.theme = theme.to_string();
            }
            if let Some(font_config) = values.get("font_config") {
                let font_values = font_config.as_object()?;
                if font_values.is_empty()
                    || font_values.keys().any(|key| {
                        !matches!(
                            key.as_str(),
                            "font_family" | "font_size" | "custom_font_family"
                        )
                    })
                {
                    return None;
                }
                if let Some(font_family) = font_values.get("font_family") {
                    let font_family = font_family.as_str()?;
                    if !crate::constants::font::FONT_FAMILIES
                        .iter()
                        .any(|(id, _, _)| *id == font_family)
                    {
                        return None;
                    }
                    patched.ui_config.font_config.font_family = font_family.to_string();
                }
                if let Some(font_size) = font_values.get("font_size") {
                    let font_size = font_size.as_str()?;
                    if !crate::constants::font::FONT_SIZES
                        .iter()
                        .any(|(id, _, _)| *id == font_size)
                    {
                        return None;
                    }
                    patched.ui_config.font_config.font_size = font_size.to_string();
                }
                if let Some(custom_font_family) = font_values.get("custom_font_family") {
                    let custom_font_family = custom_font_family.as_str()?;
                    if custom_font_family.chars().count() > 256 {
                        return None;
                    }
                    patched.ui_config.font_config.custom_font_family =
                        custom_font_family.to_string();
                }
            }
            if let Some(always_on_top) = values.get("always_on_top") {
                patched.ui_config.always_on_top = always_on_top.as_bool()?;
            }
            if let Some(window_config) = values.get("window_config") {
                let window_values = window_config.as_object()?;
                if window_values.len() != 1 || !window_values.contains_key("fixed") {
                    return None;
                }
                patched.ui_config.window_config.fixed = window_values.get("fixed")?.as_bool()?;
            }
        }
        "audio_config" => {
            let values = section_value.as_object()?;
            if values.len() != 1 || !values.contains_key("notification_enabled") {
                return None;
            }
            if let Some(enabled) = values.get("notification_enabled") {
                patched.audio_config.notification_enabled = enabled.as_bool()?;
            }
        }
        "reply_config" => {
            let values = section_value.as_object()?;
            if values.is_empty()
                || values.keys().any(|key| {
                    !matches!(
                        key.as_str(),
                        "enable_continue_reply" | "auto_continue_threshold" | "continue_prompt"
                    )
                })
            {
                return None;
            }
            if let Some(enabled) = values.get("enable_continue_reply") {
                patched.reply_config.enable_continue_reply = enabled.as_bool()?;
            }
            if let Some(threshold) = values.get("auto_continue_threshold") {
                let threshold = u32::try_from(threshold.as_u64()?).ok()?;
                if !(500..=5000).contains(&threshold) {
                    return None;
                }
                patched.reply_config.auto_continue_threshold = threshold;
            }
            if let Some(prompt) = values.get("continue_prompt") {
                let prompt = prompt.as_str()?;
                if prompt.chars().count() > 16_384 {
                    return None;
                }
                patched.reply_config.continue_prompt = prompt.to_string();
            }
        }
        "custom_prompt_config" => {
            let values = section_value.as_object()?;
            if values.is_empty()
                || values
                    .keys()
                    .any(|key| !matches!(key.as_str(), "enabled" | "prompts"))
            {
                return None;
            }
            let max_prompts = config.custom_prompt_config.max_prompts;
            if let Some(enabled) = values.get("enabled") {
                patched.custom_prompt_config.enabled = enabled.as_bool()?;
            }
            if let Some(prompts) = values.get("prompts") {
                let prompts: Vec<crate::config::settings::CustomPrompt> =
                    serde_json::from_value(prompts.clone()).ok()?;
                if prompts.len() > max_prompts as usize {
                    return None;
                }
                patched.custom_prompt_config.prompts = prompts;
            }
        }
        "shortcut_config" => {
            let values = section_value.as_object()?;
            if values.len() != 1 || !values.contains_key("global_enabled") {
                return None;
            }
            patched.shortcut_config.global_enabled = values.get("global_enabled")?.as_bool()?;
        }
        _ => return None,
    }

    Some(patched)
}

fn redact_config_value(mut value: serde_json::Value) -> serde_json::Value {
    fn redact(value: &mut serde_json::Value) {
        match value {
            serde_json::Value::Object(object) => {
                for (key, child) in object {
                    let key = key.to_ascii_lowercase();
                    if matches!(
                        key.as_str(),
                        "token"
                            | "bot_token"
                            | "acemcp_token"
                            | "manual_cookie"
                            | "api_token"
                            | "api_key"
                            | "secret"
                            | "password"
                            | "authorization"
                            | "tunnel_token"
                            | "custom_url"
                    ) {
                        *child = serde_json::Value::String("[redacted]".to_string());
                    } else {
                        redact(child);
                    }
                }
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    redact(item);
                }
            }
            _ => {}
        }
    }
    redact(&mut value);
    value
}

/// GET /api/config — 返回当前配置
async fn handle_api_config_get(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_CONFIG_READ, "missing_scope_config_read").await
    {
        return response;
    }

    use crate::config::AppState;
    if let Some(app_handle) = state.app_handle.as_ref() {
        if let Some(state) = app_handle.try_state::<AppState>() {
            if let Ok(config) = state.config.lock() {
                return match serde_json::to_value(&*config) {
                    Ok(value) => Json(redact_config_value(value)).into_response(),
                    Err(error) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({"error": format!("序列化失败: {}", error)})),
                    )
                        .into_response(),
                };
            }
        }
    }

    match crate::config::storage::load_standalone_config() {
        Ok(config) => match serde_json::to_value(config) {
            Ok(value) => Json(redact_config_value(value)).into_response(),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("序列化失败: {}", error)})),
            )
                .into_response(),
        },
        Err(error) => {
            log::warn!("[Bridge] 读取 standalone 配置失败: {}", error);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("无法获取配置: {}", error)})),
            )
                .into_response()
        }
    }
}

/// POST /api/config — 更新配置（部分更新）
async fn handle_api_config_post(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_CONFIG_WRITE, "missing_scope_config_write")
            .await
    {
        return response;
    }

    // Keep the whole read → patch → persist → apply/rollback transaction serialized.
    // Individual file writes are atomic, but without this guard two concurrent Home
    // requests could both patch the same snapshot and the later save would lose fields.
    let _config_write_guard = MOBILE_CONFIG_WRITE_LOCK.lock().await;

    use crate::config::AppState;
    let updated_section = body
        .as_object()
        .and_then(|object| object.keys().next())
        .cloned();
    if let Some(app_handle) = state.app_handle.as_ref() {
        if let Some(app_state) = app_handle.try_state::<AppState>() {
            let config_pair = app_state.config.lock().ok().and_then(|config| {
                patch_app_config(&config, &body).map(|new_config| (config.clone(), new_config))
            });

            let Some((previous_config, new_config)) = config_pair else {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({"error": "invalid_config_patch"})),
                )
                    .into_response();
            };

            if let Err(error) = crate::config::storage::save_standalone_config(&new_config) {
                log::warn!("[Bridge] 保存配置失败: {}", error);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("保存失败: {}", error)})),
                )
                    .into_response();
            }

            match app_state.config.lock() {
                Ok(mut config) => *config = new_config,
                Err(_) => {
                    let _ = crate::config::storage::save_standalone_config(&previous_config);
                    return json_error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "config_lock_failed",
                    );
                }
            }

            if let Err(error) = crate::config::storage::load_config_and_apply_window_settings(
                &app_state, app_handle,
            )
            .await
            {
                log::warn!("[Bridge] 应用移动端配置失败: {}", error);
                let restored_in_memory = match app_state.config.lock() {
                    Ok(mut config) => {
                        *config = previous_config.clone();
                        true
                    }
                    Err(_) => false,
                };
                if !restored_in_memory {
                    let _ = crate::config::storage::load_config_and_apply_window_settings(
                        &app_state, app_handle,
                    )
                    .await;
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(serde_json::json!({
                            "error": format!("应用失败: {}；配置锁损坏，已尝试从磁盘重新对齐", error)
                        })),
                    )
                        .into_response();
                }
                let rollback_result =
                    match crate::config::storage::save_config(&app_state, app_handle).await {
                        Ok(()) => crate::config::storage::load_config_and_apply_window_settings(
                            &app_state, app_handle,
                        )
                        .await
                        .map_err(|rollback_error| {
                            format!("回滚配置已落盘，但重新应用失败: {}", rollback_error)
                        }),
                        Err(rollback_error) => {
                            let realign_result =
                                crate::config::storage::load_config_and_apply_window_settings(
                                    &app_state, app_handle,
                                )
                                .await;
                            Err(match realign_result {
                                Ok(()) => format!(
                                    "回滚落盘失败: {}；已重新加载磁盘中的已保存版本",
                                    rollback_error
                                ),
                                Err(realign_error) => format!(
                                    "回滚落盘失败: {}；重新对齐磁盘版本也失败: {}",
                                    rollback_error, realign_error
                                ),
                            })
                        }
                    };
                let rollback_detail = rollback_result
                    .err()
                    .map(|detail| format!("；{}", detail))
                    .unwrap_or_default();
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": format!("应用失败: {}{}", error, rollback_detail)
                    })),
                )
                    .into_response();
            }

            let _ = app_handle.emit("config_reloaded", ());
            if updated_section.as_deref() == Some("custom_prompt_config") {
                broadcast_custom_prompt_config_changed(app_handle);
            }

            return Json(serde_json::json!({"success": true})).into_response();
        }
    }

    let current = match crate::config::storage::load_standalone_config() {
        Ok(config) => config,
        Err(error) => {
            log::warn!("[Bridge] 读取 standalone 配置失败: {}", error);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("无法获取配置: {}", error)})),
            )
                .into_response();
        }
    };
    let Some(new_config) = patch_app_config(&current, &body) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "invalid_config_patch"})),
        )
            .into_response();
    };
    if let Err(error) = crate::config::storage::save_standalone_config(&new_config) {
        log::warn!("[Bridge] 保存 standalone 配置失败: {}", error);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": format!("保存失败: {}", error)})),
        )
            .into_response();
    }

    Json(serde_json::json!({"success": true})).into_response()
}

#[cfg(test)]
mod bridge_config_tests {
    use super::{patch_app_config, redact_config_value};

    #[test]
    fn config_patch_enables_mobile_ghost_suggestion_writeback() {
        let config = crate::config::settings::AppConfig::default();
        let patched = patch_app_config(
            &config,
            &serde_json::json!({
                "mobile_config": {
                    "allow_ghost_suggestions_write": true
                }
            }),
        )
        .expect("mobile config patch should deserialize");

        assert!(patched.mobile_config.allow_ghost_suggestions_write);
        assert_eq!(
            patched.reply_config.enable_continue_reply,
            config.reply_config.enable_continue_reply
        );
    }

    #[test]
    fn config_patch_updates_allowed_section_without_touching_secrets() {
        let mut config = crate::config::settings::AppConfig::default();
        config.telegram_config.bot_token = "keep-secret".to_string();
        let original_loop_prompt = config.reply_config.loop_prompt.clone();
        let original_goal_template = config.reply_config.goal_prompt_template.clone();
        let patched = patch_app_config(
            &config,
            &serde_json::json!({
                "reply_config": {
                    "enable_continue_reply": true
                }
            }),
        )
        .expect("reply config should be writable");

        assert!(patched.reply_config.enable_continue_reply);
        assert_eq!(patched.reply_config.loop_prompt, original_loop_prompt);
        assert_eq!(
            patched.reply_config.goal_prompt_template,
            original_goal_template
        );
        assert_eq!(patched.telegram_config.bot_token, "keep-secret");
    }

    #[test]
    fn ui_patch_only_changes_home_exposed_fields() {
        let config = crate::config::settings::AppConfig::default();
        let original_window = config.ui_config.window_config.clone();
        let patched = patch_app_config(
            &config,
            &serde_json::json!({
                "ui_config": {
                    "always_on_top": !config.ui_config.always_on_top,
                    "window_config": {
                        "fixed": !original_window.fixed
                    }
                }
            }),
        )
        .expect("ui config should be writable");

        assert_eq!(
            patched.ui_config.always_on_top,
            !config.ui_config.always_on_top
        );
        assert_eq!(
            patched.ui_config.window_config.fixed,
            !original_window.fixed
        );
        assert_eq!(
            patched.ui_config.window_config.fixed_width,
            original_window.fixed_width
        );
        assert_eq!(
            patched.ui_config.window_config.min_width,
            original_window.min_width
        );
    }

    #[test]
    fn config_patch_rejects_sensitive_or_multi_section_writes() {
        let config = crate::config::settings::AppConfig::default();
        assert!(patch_app_config(
            &config,
            &serde_json::json!({ "telegram_config": { "bot_token": "replace" } }),
        )
        .is_none());
        assert!(patch_app_config(
            &config,
            &serde_json::json!({
                "audio_config": {
                    "custom_url": "/Users/example/private-sound.mp3"
                }
            }),
        )
        .is_none());
        assert!(patch_app_config(
            &config,
            &serde_json::json!({
                "audio_config": config.audio_config.clone(),
                "reply_config": config.reply_config.clone(),
            }),
        )
        .is_none());
    }

    #[test]
    fn config_redaction_hides_custom_audio_references() {
        let redacted = redact_config_value(serde_json::json!({
            "audio_config": {
                "notification_enabled": true,
                "custom_url": "/Users/example/private-sound.mp3"
            }
        }));

        assert_eq!(
            redacted["audio_config"]["custom_url"],
            serde_json::Value::String("[redacted]".to_string())
        );
    }

    #[test]
    fn shortcut_patch_cannot_replace_shortcut_bindings() {
        let config = crate::config::settings::AppConfig::default();
        assert!(patch_app_config(
            &config,
            &serde_json::json!({
                "shortcut_config": {
                    "global_enabled": false,
                    "shortcuts": {}
                }
            }),
        )
        .is_none());

        let patched = patch_app_config(
            &config,
            &serde_json::json!({
                "shortcut_config": {
                    "global_enabled": false
                }
            }),
        )
        .expect("shortcut enable state should be writable");

        assert!(!patched.shortcut_config.global_enabled);
        assert_eq!(
            patched.shortcut_config.shortcuts.len(),
            config.shortcut_config.shortcuts.len()
        );
    }
}

/// GET /api/audio-assets — 返回可用音频资源列表
async fn handle_api_audio_assets(headers: HeaderMap) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_CONFIG_READ, "missing_scope_config_read").await
    {
        return response;
    }

    let manager = crate::audio_assets::get_audio_asset_manager();
    match manager.lock() {
        Ok(mgr) => {
            let assets: Vec<_> = mgr.get_all_assets().into_iter().cloned().collect();
            Json(serde_json::json!({ "assets": assets })).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("获取音频资源失败: {}", e) })),
        )
            .into_response(),
    }
}

/// POST /api/test-audio — 测试播放音频（3秒防抖）
async fn handle_api_test_audio(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) =
        authorize_public_route_scope(&headers, SCOPE_CONFIG_WRITE, "missing_scope_config_write")
            .await
    {
        return response;
    }

    use std::sync::Mutex;
    use std::time::Instant;
    static LAST_PLAY: Mutex<Option<Instant>> = Mutex::new(None);

    if let Ok(mut last) = LAST_PLAY.lock() {
        if let Some(t) = *last {
            if t.elapsed().as_secs_f64() < 3.0 {
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({ "error": "请稍后再试（3秒冷却）" })),
                )
                    .into_response();
            }
        }
        *last = Some(Instant::now());
    }

    let Some(app_handle) = state.app_handle.as_ref() else {
        return json_error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            "bridge_only_daemon_no_app_state",
        );
    };
    match app_handle.try_state::<crate::config::AppState>() {
        Some(state) => {
            let audio_url = if let Ok(config) = state.config.lock() {
                config.audio_config.custom_url.clone()
            } else {
                return json_error_response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "config_unavailable",
                );
            };
            match crate::ui_audio::play_audio_file(&app_handle, &audio_url).await {
                Ok(_) => Json(serde_json::json!({ "ok": true })).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": format!("{}", e) })),
                )
                    .into_response(),
            }
        }
        None => json_error_response(StatusCode::SERVICE_UNAVAILABLE, "app_state_unavailable"),
    }
}

async fn restart_cloudflared_for_recovery(reason: &str) -> serde_json::Value {
    log_tunnel_recovery_snapshot("before_recovery").await;
    log::info!(
        "[Bridge][Tunnel Debug] 请求 root supervisor 恢复 tunnel: reason={}",
        reason
    );

    let requested = signal_root_tunnel_recovery_request(reason).await;
    log_tunnel_recovery_snapshot("after_root_recovery_request").await;

    if requested {
        serde_json::json!({
            "action": "requested_root_recovery",
            "reason": reason,
            "message": "已请求 root tunnel supervisor 自愈"
        })
    } else {
        serde_json::json!({
            "action": "root_recovery_request_failed",
            "reason": reason,
            "message": "写入 root tunnel 恢复请求失败"
        })
    }
}

const TAILSCALE_APP_CLI_PATH: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";
const TAILSCALE_FUNNEL_TARGET: &str = "http://127.0.0.1:8080";

fn truncate_for_diagnostics(value: &str, max_chars: usize) -> String {
    let mut output = String::new();
    for (index, ch) in value.chars().enumerate() {
        if index >= max_chars {
            output.push_str("...");
            break;
        }
        output.push(ch);
    }
    output
}

fn tailscale_cli_path() -> Option<String> {
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let candidate = dir.join("tailscale");
            if candidate.is_file() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }
    }

    if std::path::Path::new(TAILSCALE_APP_CLI_PATH).is_file() {
        return Some(TAILSCALE_APP_CLI_PATH.to_string());
    }

    None
}

async fn run_tailscale_command(
    cli_path: &str,
    args: &[&str],
    timeout_secs: u64,
) -> serde_json::Value {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        tokio::process::Command::new(cli_path).args(args).output(),
    )
    .await;

    match output {
        Ok(Ok(output)) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let parsed = serde_json::from_slice::<serde_json::Value>(&output.stdout).ok();
            serde_json::json!({
                "ok": output.status.success(),
                "status_code": output.status.code(),
                "args": args,
                "stdout": truncate_for_diagnostics(&stdout, 1200),
                "stderr": truncate_for_diagnostics(&stderr, 1200),
                "json": parsed.unwrap_or(serde_json::Value::Null),
            })
        }
        Ok(Err(error)) => serde_json::json!({
            "ok": false,
            "args": args,
            "error": error.to_string(),
        }),
        Err(_) => serde_json::json!({
            "ok": false,
            "args": args,
            "error": "timeout",
            "timeout_secs": timeout_secs,
        }),
    }
}

fn command_diagnostics_without_json(command: &serde_json::Value) -> serde_json::Value {
    let mut value = command.clone();
    if let Some(object) = value.as_object_mut() {
        object.remove("json");
    }
    value
}

async fn launch_tailscale_app_for_recovery() -> serde_json::Value {
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(8),
        tokio::process::Command::new("open")
            .args(["-ga", "Tailscale"])
            .output(),
    )
    .await;

    match output {
        Ok(Ok(output)) => serde_json::json!({
            "ok": output.status.success(),
            "status_code": output.status.code(),
            "stdout": truncate_for_diagnostics(&String::from_utf8_lossy(&output.stdout), 400),
            "stderr": truncate_for_diagnostics(&String::from_utf8_lossy(&output.stderr), 400),
        }),
        Ok(Err(error)) => serde_json::json!({
            "ok": false,
            "error": error.to_string(),
        }),
        Err(_) => serde_json::json!({
            "ok": false,
            "error": "timeout",
        }),
    }
}

async fn inspect_tailscale_funnel_runtime() -> serde_json::Value {
    let Some(cli_path) = tailscale_cli_path() else {
        return serde_json::json!({
            "healthy": false,
            "cli_available": false,
            "reason": "tailscale_cli_missing",
        });
    };

    let status_command = run_tailscale_command(&cli_path, &["status", "--json"], 8).await;
    let status_json = status_command
        .get("json")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let status_json_available = !status_json.is_null();
    let status_summary = tailscale_status_summary(&status_json);
    let dns_name =
        tailscale_dns_name(&status_json).or_else(tailscale_host_from_public_bridge_base_url);
    let backend_running = status_json
        .get("BackendState")
        .and_then(|value| value.as_str())
        == Some("Running");
    let self_online = status_json
        .pointer("/Self/Online")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    let funnel_command = run_tailscale_command(&cli_path, &["funnel", "status", "--json"], 8).await;
    let funnel_json = funnel_command
        .get("json")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let funnel_json_available = !funnel_json.is_null();
    let config_ok = dns_name
        .as_deref()
        .map(|host| tailscale_funnel_config_matches(&funnel_json, host, TAILSCALE_FUNNEL_TARGET))
        .unwrap_or(false);

    let (public_url, public_http, public_http_ok, public_ws, public_ws_ok) =
        if let Some(host) = dns_name.as_deref() {
            let public_url = format!("https://{host}");
            let (public_http, public_http_ok) =
                probe_http_endpoint(&format!("{public_url}/api/version")).await;
            let (public_ws, public_ws_ok) =
                probe_websocket_upgrade_endpoint(&format!("{public_url}/ws")).await;
            (
                serde_json::Value::String(public_url),
                public_http,
                public_http_ok,
                public_ws,
                public_ws_ok,
            )
        } else {
            (
                serde_json::Value::Null,
                serde_json::json!({"error": "tailscale_dns_missing"}),
                false,
                serde_json::json!({"error": "tailscale_dns_missing"}),
                false,
            )
        };

    let public_ws_auth_required = websocket_probe_auth_required(&public_ws);
    let public_ws_ready = public_ws_ok || public_ws_auth_required;
    let public_probe_healthy = public_http_ok && public_ws_ready;
    let runtime_verified = status_json_available
        && funnel_json_available
        && backend_running
        && self_online
        && config_ok;
    let healthy = public_probe_healthy && (runtime_verified || !funnel_json_available);
    let health_source = if runtime_verified {
        "tailscale_cli_and_public_probe"
    } else if public_probe_healthy {
        "public_probe"
    } else {
        "unhealthy"
    };

    serde_json::json!({
        "healthy": healthy,
        "health_source": health_source,
        "cli_available": true,
        "cli_path": cli_path,
        "status_json_available": status_json_available,
        "funnel_json_available": funnel_json_available,
        "status": status_summary,
        "status_command": command_diagnostics_without_json(&status_command),
        "funnel_config_ok": config_ok,
        "funnel_status": funnel_json,
        "funnel_command": command_diagnostics_without_json(&funnel_command),
        "public_url": public_url,
        "public_http": public_http,
        "public_ws": public_ws,
        "public_ws_auth_required": public_ws_auth_required,
        "public_ws_ready": public_ws_ready,
    })
}

async fn recover_tailscale_funnel_value(recovery_transport: &str) -> serde_json::Value {
    let (local_origin, local_healthy) =
        probe_http_endpoint("http://127.0.0.1:8080/api/version").await;
    let (local_ws, local_ws_ok) =
        probe_websocket_upgrade_endpoint("http://127.0.0.1:8080/ws").await;
    if !local_healthy || !local_ws_ok {
        return serde_json::json!({
            "ok": false,
            "action": "local_origin_unhealthy",
            "message": "本地 8080 bridge 不健康，暂不重配 Tailscale Funnel",
            "recovery_transport": recovery_transport,
            "recovered_transport": "tailscale_funnel",
            "checks": {
                "local_origin": local_origin,
                "local_ws": local_ws,
            }
        });
    }

    let before = inspect_tailscale_funnel_runtime().await;
    if before
        .get("healthy")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return serde_json::json!({
            "ok": true,
            "action": "skipped_healthy",
            "message": "Tailscale Funnel 当前健康，跳过恢复",
            "recovery_transport": recovery_transport,
            "recovered_transport": "tailscale_funnel",
            "checks": {
                "local_origin": local_origin,
                "local_ws": local_ws,
                "tailscale_funnel": before,
            }
        });
    }

    if !before
        .get("cli_available")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return serde_json::json!({
            "ok": false,
            "action": "tailscale_cli_missing",
            "message": "未找到 tailscale CLI 或 Tailscale.app binary",
            "recovery_transport": recovery_transport,
            "recovered_transport": "tailscale_funnel",
            "checks": {
                "local_origin": local_origin,
                "local_ws": local_ws,
                "tailscale_funnel": before,
            }
        });
    }

    let launch = launch_tailscale_app_for_recovery().await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let after_launch = inspect_tailscale_funnel_runtime().await;
    if after_launch
        .get("healthy")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
    {
        return serde_json::json!({
            "ok": true,
            "action": "started_tailscale_app",
            "message": "Tailscale App 启动后 Funnel 已恢复",
            "recovery_transport": recovery_transport,
            "recovered_transport": "tailscale_funnel",
            "launch": launch,
            "checks": {
                "local_origin": local_origin,
                "local_ws": local_ws,
                "tailscale_funnel_before": before,
                "tailscale_funnel_after": after_launch,
            }
        });
    }

    let cli_json_ready = after_launch
        .get("status_json_available")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
        && after_launch
            .get("funnel_json_available")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
    if !cli_json_ready {
        return serde_json::json!({
            "ok": false,
            "action": "tailscale_cli_unusable",
            "message": "Tailscale CLI 未返回可解析 JSON，无法安全重放 Funnel 配置",
            "recovery_transport": recovery_transport,
            "recovered_transport": "tailscale_funnel",
            "launch": launch,
            "checks": {
                "local_origin": local_origin,
                "local_ws": local_ws,
                "tailscale_funnel_before": before,
                "tailscale_funnel_after_launch": after_launch,
            }
        });
    }

    let cli_path = after_launch
        .get("cli_path")
        .and_then(|value| value.as_str())
        .or_else(|| before.get("cli_path").and_then(|value| value.as_str()))
        .unwrap_or(TAILSCALE_APP_CLI_PATH);
    let https_arg = format!("--https={TAILSCALE_FUNNEL_PORT}");
    let reapply = run_tailscale_command(
        cli_path,
        &[
            "funnel",
            "--bg",
            https_arg.as_str(),
            "--yes",
            TAILSCALE_FUNNEL_TARGET,
        ],
        20,
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let after = inspect_tailscale_funnel_runtime().await;
    let healthy_after = after
        .get("healthy")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);

    serde_json::json!({
        "ok": healthy_after,
        "action": if healthy_after { "recovered_tailscale_funnel" } else { "tailscale_funnel_reapply_failed" },
        "message": if healthy_after { "Tailscale Funnel 已恢复" } else { "已尝试重放 Tailscale Funnel 配置，但健康检查仍未通过" },
        "recovery_transport": recovery_transport,
        "recovered_transport": "tailscale_funnel",
        "launch": launch,
        "reapply": reapply,
        "checks": {
            "local_origin": local_origin,
            "local_ws": local_ws,
            "tailscale_funnel_before": before,
            "tailscale_funnel_after_launch": after_launch,
            "tailscale_funnel_after": after,
        }
    })
}

/// POST /api/recover-tailscale-funnel — 恢复当前 Mac 的 Tailscale Funnel 443 -> 8080
async fn handle_api_recover_tailscale_funnel(headers: HeaderMap) -> Response {
    if let Err(response) = authorize_public_route_any_scope(
        &headers,
        &[SCOPE_TUNNEL_RECOVER, SCOPE_SERVICE_RECOVER],
        "missing_scope_tunnel_recover",
    )
    .await
    {
        return response;
    }

    let recovery_transport = recovery_transport_from_headers(&headers);
    log::info!(
        "[Bridge][Tunnel Debug] 收到 /api/recover-tailscale-funnel 请求 recovery_transport={}",
        recovery_transport
    );
    Json(recover_tailscale_funnel_value(&recovery_transport).await).into_response()
}

/// POST /api/restart-tunnel — 重启 cloudflared tunnel（保持向后兼容）
async fn handle_api_restart_tunnel(
    State(_state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authorize_public_route_any_scope(
        &headers,
        &[SCOPE_TUNNEL_RECOVER, SCOPE_SERVICE_RECOVER],
        "missing_scope_tunnel_recover",
    )
    .await
    {
        return response;
    }
    let recovery_transport = recovery_transport_from_headers(&headers);

    use std::sync::Mutex;
    use std::time::Instant;
    static LAST_RESTART: Mutex<Option<Instant>> = Mutex::new(None);

    log::info!(
        "[Bridge][Tunnel Debug] 收到 /api/restart-tunnel 请求 recovery_transport={}",
        recovery_transport
    );
    log_tunnel_recovery_snapshot("api_restart_tunnel_received").await;

    let (
        (local_origin, local_healthy),
        (local_ws, local_ws_ok),
        (public_tunnel, public_probe_healthy, public_ws, public_ws_ok),
        root_tunnel,
    ) = tokio::join!(
        probe_http_endpoint("http://127.0.0.1:8080/api/version"),
        probe_websocket_upgrade_endpoint("http://127.0.0.1:8080/ws"),
        get_public_probe_snapshot(),
        inspect_root_tunnel_runtime(),
    );
    let public_ws_auth_required = websocket_probe_auth_required(&public_ws);
    let public_ws_effective_ok =
        websocket_probe_ok_or_auth_required(public_ws_ok, &public_ws, true);
    let root_child_alive = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("child_alive"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let root_ha_ready = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("ha_ready"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let root_backoff_active = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("backoff_active"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let root_tunnel_authoritative_up = root_tunnel_is_authoritative_up(&root_tunnel);
    let public_healthy = public_probe_healthy || root_tunnel_authoritative_up;
    let public_ws_effective_ok = public_ws_effective_ok || root_tunnel_authoritative_up;

    if local_healthy
        && local_ws_ok
        && public_healthy
        && public_ws_effective_ok
        && root_child_alive
        && root_ha_ready
    {
        return Json(serde_json::json!({
            "ok": true,
            "action": "skipped_healthy",
            "message": "tunnel 与 bridge 当前健康，跳过重启",
            "recovery_transport": recovery_transport,
            "checks": {
                "local_origin": local_origin,
                "local_ws": local_ws,
                "public_tunnel": public_tunnel,
                "public_ws": public_ws,
                "public_ws_auth_required": public_ws_auth_required,
                "root_tunnel": root_tunnel,
            }
        }))
        .into_response();
    }

    if root_backoff_active {
        return Json(serde_json::json!({
            "ok": false,
            "action": "backoff_active",
            "message": "root tunnel supervisor 正在 backoff，暂不重复恢复",
            "recovery_transport": recovery_transport,
            "checks": {
                "local_origin": local_origin,
                "local_ws": local_ws,
                "public_tunnel": public_tunnel,
                "public_ws": public_ws,
                "public_ws_auth_required": public_ws_auth_required,
                "root_tunnel": root_tunnel,
            }
        }))
        .into_response();
    }

    // 30秒冷却防止频繁重启
    if let Ok(mut last) = LAST_RESTART.lock() {
        if let Some(t) = *last {
            if t.elapsed().as_secs() < 30 {
                return Json(serde_json::json!({
                    "ok": false,
                    "action": "cooldown_active",
                    "message": "请等待30秒后再试",
                    "recovery_transport": recovery_transport
                }))
                .into_response();
            }
        }
        *last = Some(Instant::now());
    }

    let reason = if !root_child_alive || !root_ha_ready {
        "root_tunnel_unhealthy"
    } else if !public_healthy || !public_ws_effective_ok {
        "public_tunnel_unhealthy"
    } else if !local_healthy || !local_ws_ok {
        "local_origin_unhealthy"
    } else {
        "manual_request"
    };

    let tunnel_result = restart_cloudflared_for_recovery(reason).await;
    log::info!(
        "[Bridge][Tunnel Debug] /api/restart-tunnel 恢复结果: {:?}",
        tunnel_result
    );
    log_tunnel_recovery_snapshot("api_restart_tunnel_completed").await;

    Json(serde_json::json!({
        "ok": true,
        "recovery_transport": recovery_transport,
        "result": tunnel_result,
        "checks": {
            "local_origin": local_origin,
            "local_ws": local_ws,
            "public_tunnel": public_tunnel,
            "public_ws": public_ws,
            "root_tunnel": root_tunnel,
        }
    }))
    .into_response()
}

fn recovery_transport_from_headers(headers: &HeaderMap) -> String {
    headers
        .get("x-iterate-recovery-transport")
        .and_then(|value| value.to_str().ok())
        .map(sanitize_recovery_transport)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn sanitize_recovery_transport(value: &str) -> String {
    value
        .trim()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_' || *ch == '-')
        .take(48)
        .collect()
}

async fn log_tunnel_recovery_snapshot(stage: &str) {
    let local_api = debug_health_status("http://127.0.0.1:8080/api/version").await;
    let public_base_url = public_bridge_base_url();
    let public_api = if public_base_url.is_empty() {
        "not_configured".to_string()
    } else {
        debug_health_status(&format!("{public_base_url}/api/version")).await
    };
    let legacy = debug_launchctl_label("system/com.cloudflare.cloudflared").await;

    log::info!(
        "[Bridge][Tunnel Debug] snapshot stage={} local_api={} public_api={} legacy={}",
        stage,
        local_api,
        public_api,
        legacy
    );
}

async fn signal_root_tunnel_recovery_request(reason: &str) -> bool {
    let payload = format!(
        "requested_at={}\nreason={}\nsource=bridge_restart_tunnel\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        reason
    );
    match std::fs::write("/tmp/iterate-root-tunnel-recover.request", payload) {
        Ok(_) => {
            log::info!(
                "[Bridge][Tunnel Debug] 已写入 root tunnel 恢复请求: reason={}",
                reason
            );
            true
        }
        Err(err) => {
            log::warn!(
                "[Bridge][Tunnel Debug] 写入 root tunnel 恢复请求失败: reason={} err={}",
                reason,
                err
            );
            false
        }
    }
}

fn is_public_bridge_url(url: &str) -> bool {
    let base_url = public_bridge_base_url();
    !base_url.is_empty() && url.starts_with(&base_url)
}

fn http_probe_timeout_secs(url: &str) -> u64 {
    if is_public_bridge_url(url) {
        PUBLIC_PROBE_TIMEOUT_SECS
    } else {
        LOCAL_PROBE_TIMEOUT_SECS
    }
}

fn ws_probe_timeout_secs(url: &str) -> u64 {
    if is_public_bridge_url(url) {
        PUBLIC_WS_PROBE_TIMEOUT_SECS
    } else {
        LOCAL_WS_PROBE_TIMEOUT_SECS
    }
}

fn build_probe_client(timeout_secs: u64) -> Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .no_proxy()
        .build()
}

async fn debug_health_status(url: &str) -> String {
    let timeout_secs = http_probe_timeout_secs(url);
    let client = match build_probe_client(timeout_secs) {
        Ok(client) => client,
        Err(err) => return format!("client_error:{err}"),
    };

    match client.get(url).send().await {
        Ok(response) => format!("http_{}", response.status().as_u16()),
        Err(err) => format!("error:{}", err),
    }
}

async fn health_endpoint_success(url: &str) -> bool {
    if !is_public_bridge_url(url) {
        return health_endpoint_success_once(url).await;
    }

    if let Some(last_success_at) = *PUBLIC_HEALTH_LAST_SUCCESS_AT.read().await {
        if last_success_at.elapsed()
            < std::time::Duration::from_secs(PUBLIC_HEALTH_SUCCESS_CACHE_SECS)
        {
            return true;
        }
    }

    for attempt in 0..PUBLIC_HEALTH_RETRY_ATTEMPTS {
        if health_endpoint_success_once(url).await {
            *PUBLIC_HEALTH_LAST_SUCCESS_AT.write().await = Some(std::time::Instant::now());
            return true;
        }

        if attempt + 1 < PUBLIC_HEALTH_RETRY_ATTEMPTS {
            tokio::time::sleep(std::time::Duration::from_millis(
                PUBLIC_HEALTH_RETRY_DELAY_MS,
            ))
            .await;
        }
    }

    false
}

async fn health_endpoint_success_once(url: &str) -> bool {
    let timeout_secs = http_probe_timeout_secs(url);
    let client = match build_probe_client(timeout_secs) {
        Ok(client) => client,
        Err(_) => return false,
    };

    client
        .get(url)
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

async fn probe_http_endpoint(url: &str) -> (serde_json::Value, bool) {
    let started_at = std::time::Instant::now();
    let timeout_secs = http_probe_timeout_secs(url);
    let client = match build_probe_client(timeout_secs) {
        Ok(client) => client,
        Err(err) => {
            return (
                serde_json::json!({
                    "url": url,
                    "healthy": false,
                    "status_code": serde_json::Value::Null,
                    "latency_ms": started_at.elapsed().as_millis(),
                    "timeout_secs": timeout_secs,
                    "error": format!("client_error:{err}"),
                }),
                false,
            );
        }
    };

    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status();
            let healthy = status.is_success();
            (
                serde_json::json!({
                    "url": url,
                    "healthy": healthy,
                    "status_code": status.as_u16(),
                    "latency_ms": started_at.elapsed().as_millis(),
                    "timeout_secs": timeout_secs,
                    "probe_mode": "direct",
                    "error": serde_json::Value::Null,
                }),
                healthy,
            )
        }
        Err(err) => (
            serde_json::json!({
                "url": url,
                "healthy": false,
                "status_code": serde_json::Value::Null,
                "latency_ms": started_at.elapsed().as_millis(),
                "timeout_secs": timeout_secs,
                "probe_mode": "direct",
                "error": err.to_string(),
            }),
            false,
        ),
    }
}

type WebsocketProbeStream =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>;
type WebsocketProbeResponse = tokio_tungstenite::tungstenite::handshake::client::Response;
type WebsocketProbeConnectResult =
    Result<(WebsocketProbeStream, WebsocketProbeResponse), WebsocketProbeError>;

enum WebsocketProbeError {
    Handshake(tokio_tungstenite::tungstenite::Error),
    TlsConnectorBuild(String),
}

async fn connect_websocket_probe(ws_url: &str) -> WebsocketProbeConnectResult {
    let mut request = ws_url
        .into_client_request()
        .map_err(WebsocketProbeError::Handshake)?;
    if let Ok(token) = crate::bridge::auth::issue_internal_bridge_websocket_token(ws_url) {
        let authorization = format!("Bearer {token}")
            .parse()
            .map_err(|_| WebsocketProbeError::TlsConnectorBuild("invalid_auth_header".into()))?;
        request.headers_mut().insert("authorization", authorization);
    }
    if ws_url.starts_with("wss://") {
        let tls_connector = native_tls::TlsConnector::new()
            .map_err(|err| WebsocketProbeError::TlsConnectorBuild(err.to_string()))?;
        return tokio_tungstenite::connect_async_tls_with_config(
            request,
            None,
            false,
            Some(tokio_tungstenite::Connector::NativeTls(tls_connector)),
        )
        .await
        .map_err(WebsocketProbeError::Handshake);
    }

    tokio_tungstenite::connect_async(request)
        .await
        .map_err(WebsocketProbeError::Handshake)
}

fn websocket_probe_error_code(error: &tokio_tungstenite::tungstenite::Error) -> &'static str {
    let message = error.to_string();
    if message.contains("TLS support not compiled in") {
        "ws_tls_connector_unavailable"
    } else if message.starts_with("TLS error:") {
        "ws_tls_error"
    } else if message.starts_with("URL error:") {
        "ws_url_error"
    } else {
        "ws_handshake_error"
    }
}

fn websocket_probe_error_code_from_error(error: &WebsocketProbeError) -> &'static str {
    match error {
        WebsocketProbeError::Handshake(err) => websocket_probe_error_code(err),
        WebsocketProbeError::TlsConnectorBuild(_) => "ws_tls_connector_build_failed",
    }
}

fn websocket_probe_error_message(error: &WebsocketProbeError) -> String {
    match error {
        WebsocketProbeError::Handshake(err) => err.to_string(),
        WebsocketProbeError::TlsConnectorBuild(err) => err.clone(),
    }
}

fn attach_direct_error(value: &mut serde_json::Value, direct_error: Option<&str>) {
    let Some(direct_error) = direct_error else {
        return;
    };
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "direct_error".to_string(),
            serde_json::Value::String(direct_error.to_string()),
        );
    }
}

fn websocket_probe_value_from_result(
    url: &str,
    ws_url: &str,
    probe_mode: &str,
    started_at: std::time::Instant,
    timeout_secs: u64,
    result: Result<WebsocketProbeConnectResult, tokio::time::error::Elapsed>,
    direct_error: Option<&str>,
) -> (serde_json::Value, bool) {
    match result {
        Ok(Ok((_stream, response))) => {
            let status = response.status();
            let status_code = status.as_u16();
            let upgrade_ok = status_code == 101;
            let mut value = serde_json::json!({
                "url": url,
                "ws_url": ws_url,
                "upgrade_ok": upgrade_ok,
                "status_code": status_code,
                "status_line": format!("{:?}", status),
                "latency_ms": started_at.elapsed().as_millis(),
                "timeout_secs": timeout_secs,
                "probe_mode": probe_mode,
                "error_code": if upgrade_ok {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String("unexpected_status".to_string())
                },
                "error": if upgrade_ok {
                    serde_json::Value::Null
                } else {
                    serde_json::Value::String(format!("unexpected_status:{status_code}"))
                },
            });
            attach_direct_error(&mut value, direct_error);
            (value, upgrade_ok)
        }
        Ok(Err(WebsocketProbeError::Handshake(tokio_tungstenite::tungstenite::Error::Http(
            response,
        )))) => {
            let status = response.status();
            let status_code = status.as_u16();
            let mut value = serde_json::json!({
                "url": url,
                "ws_url": ws_url,
                "upgrade_ok": false,
                "status_code": status_code,
                "status_line": format_http_status(status),
                "latency_ms": started_at.elapsed().as_millis(),
                "timeout_secs": timeout_secs,
                "probe_mode": probe_mode,
                "error_code": "http_status",
                "error": format!("http_status:{status_code}"),
            });
            attach_direct_error(&mut value, direct_error);
            (value, false)
        }
        Ok(Err(err)) => {
            let error_code = websocket_probe_error_code_from_error(&err);
            let mut value = serde_json::json!({
                "url": url,
                "ws_url": ws_url,
                "upgrade_ok": false,
                "status_code": serde_json::Value::Null,
                "status_line": serde_json::Value::Null,
                "latency_ms": started_at.elapsed().as_millis(),
                "timeout_secs": timeout_secs,
                "probe_mode": probe_mode,
                "error_code": error_code,
                "error": websocket_probe_error_message(&err),
            });
            attach_direct_error(&mut value, direct_error);
            (value, false)
        }
        Err(_) => {
            let mut value = serde_json::json!({
                "url": url,
                "ws_url": ws_url,
                "upgrade_ok": false,
                "status_code": serde_json::Value::Null,
                "status_line": serde_json::Value::Null,
                "latency_ms": started_at.elapsed().as_millis(),
                "timeout_secs": timeout_secs,
                "probe_mode": probe_mode,
                "error_code": "timeout",
                "error": "timeout",
            });
            attach_direct_error(&mut value, direct_error);
            (value, false)
        }
    }
}

async fn probe_websocket_upgrade_endpoint(url: &str) -> (serde_json::Value, bool) {
    let started_at = std::time::Instant::now();
    let timeout_secs = ws_probe_timeout_secs(url);
    let Some(ws_url) = http_url_to_ws_url(url) else {
        return (
            serde_json::json!({
                "url": url,
                "upgrade_ok": false,
                "status_code": serde_json::Value::Null,
                "status_line": serde_json::Value::Null,
                "latency_ms": started_at.elapsed().as_millis(),
                "timeout_secs": timeout_secs,
                "probe_mode": "tokio_tungstenite_handshake",
                "error_code": "unsupported_url_scheme",
                "error": "unsupported_url_scheme",
            }),
            false,
        );
    };

    let direct_result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        connect_websocket_probe(ws_url.as_str()),
    )
    .await;

    websocket_probe_value_from_result(
        url,
        ws_url.as_str(),
        "tokio_tungstenite_handshake",
        started_at,
        timeout_secs,
        direct_result,
        None,
    )
}

async fn refresh_public_probe_cache() {
    let public_base_url = public_bridge_base_url();
    if public_base_url.is_empty() {
        *PUBLIC_PROBE_CACHE.write().await = Some(CachedPublicProbe {
            http_value: missing_public_probe_cache_value("", "http"),
            http_ok: false,
            ws_value: missing_public_probe_cache_value("", "websocket"),
            ws_ok: false,
            refreshed_at: std::time::Instant::now(),
        });
        return;
    }
    let public_version_url = format!("{}/api/version", public_base_url);
    let public_ws_url = format!("{}/ws", public_base_url);
    let ((http_value, http_ok), (ws_value, ws_ok)) = tokio::join!(
        probe_http_endpoint(&public_version_url),
        probe_websocket_upgrade_endpoint(&public_ws_url),
    );
    *PUBLIC_PROBE_CACHE.write().await = Some(CachedPublicProbe {
        http_value,
        http_ok,
        ws_value,
        ws_ok,
        refreshed_at: std::time::Instant::now(),
    });
}

async fn refresh_public_probe_cache_guarded() {
    if PUBLIC_PROBE_REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    refresh_public_probe_cache().await;
    PUBLIC_PROBE_REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
}

fn request_public_probe_cache_refresh() {
    if PUBLIC_PROBE_REFRESH_IN_FLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }

    tokio::spawn(async {
        refresh_public_probe_cache().await;
        PUBLIC_PROBE_REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
    });
}

fn annotate_public_probe_cache_state(
    mut value: serde_json::Value,
    age_secs: u64,
    stale: bool,
) -> serde_json::Value {
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "cache_age_secs".to_string(),
            serde_json::Value::Number(age_secs.into()),
        );
        object.insert("cache_stale".to_string(), serde_json::Value::Bool(stale));
        object.insert(
            "cache_max_age_secs".to_string(),
            serde_json::Value::Number(PUBLIC_PROBE_CACHE_MAX_AGE_SECS.into()),
        );
    }
    value
}

fn missing_public_probe_cache_value(url: &str, probe_kind: &str) -> serde_json::Value {
    serde_json::json!({
        "url": url,
        "healthy": false,
        "upgrade_ok": false,
        "status_code": serde_json::Value::Null,
        "latency_ms": 0,
        "probe_kind": probe_kind,
        "probe_mode": "cache",
        "cache_stale": true,
        "cache_state": "missing",
        "error_code": "cache_missing",
        "error": "public_probe_cache_missing",
    })
}

/// 读取公网探针快照：只读缓存并秒回；缓存缺失/过期只触发后台刷新。
async fn get_public_probe_snapshot() -> (serde_json::Value, bool, serde_json::Value, bool) {
    let cached = { PUBLIC_PROBE_CACHE.read().await.clone() };
    if let Some(cached) = cached {
        let age_secs = cached.refreshed_at.elapsed().as_secs();
        let stale = age_secs >= PUBLIC_PROBE_CACHE_MAX_AGE_SECS;
        if stale {
            request_public_probe_cache_refresh();
        }
        let effective_http_ok = cached.http_ok && !stale;
        let effective_ws_ok = cached.ws_ok && !stale;
        return (
            annotate_public_probe_cache_state(cached.http_value, age_secs, stale),
            effective_http_ok,
            annotate_public_probe_cache_state(cached.ws_value, age_secs, stale),
            effective_ws_ok,
        );
    }

    let public_base_url = public_bridge_base_url();
    let public_version_url = format!("{}/api/version", public_base_url);
    let public_ws_url = format!("{}/ws", public_base_url);
    request_public_probe_cache_refresh();
    (
        missing_public_probe_cache_value(&public_version_url, "http"),
        false,
        missing_public_probe_cache_value(&public_ws_url, "websocket"),
        false,
    )
}

/// 启动公网探针后台刷新循环，让 connection-status 始终读到近实时缓存。
fn spawn_public_probe_cache_refresher() {
    tokio::spawn(async {
        loop {
            refresh_public_probe_cache_guarded().await;
            tokio::time::sleep(std::time::Duration::from_secs(
                PUBLIC_PROBE_CACHE_REFRESH_SECS,
            ))
            .await;
        }
    });
}

fn spawn_quota_snapshot_refresher(
    app_handle: Option<AppHandle>,
    tx: broadcast::Sender<BridgeMessage>,
) {
    if QUOTA_SNAPSHOT_REFRESHER_STARTED.swap(true, Ordering::Relaxed) {
        return;
    }

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(
            QUOTA_SNAPSHOT_REFRESH_START_DELAY_SECS,
        ))
        .await;

        loop {
            let has_ws_subscribers = tx.receiver_count() > 0;
            let has_quota_live_activity = has_quota_live_activity_tokens().await;
            let active_targets = has_ws_subscribers || has_quota_live_activity;

            refresh_quota_snapshot_and_broadcast(
                app_handle.as_ref(),
                &tx,
                "quota_refresh_scheduler",
                None,
            )
            .await;

            let refresh_secs = if active_targets {
                QUOTA_SNAPSHOT_REFRESH_ACTIVE_SECS
            } else {
                QUOTA_SNAPSHOT_REFRESH_IDLE_SECS
            };
            tokio::time::sleep(std::time::Duration::from_secs(refresh_secs)).await;
        }
    });
}

fn spawn_quota_snapshot_refresh_once(
    app_handle: Option<AppHandle>,
    tx: broadcast::Sender<BridgeMessage>,
    reason: &'static str,
) {
    spawn_quota_snapshot_refresh_once_for_codex_home(app_handle, tx, reason, None);
}

fn spawn_quota_snapshot_refresh_once_for_codex_home(
    app_handle: Option<AppHandle>,
    tx: broadcast::Sender<BridgeMessage>,
    reason: &'static str,
    codex_home: Option<String>,
) {
    let key = quota_snapshot_refresh_key(codex_home.as_deref());
    let cooldown = std::time::Duration::from_secs(QUOTA_SNAPSHOT_REFRESH_TRIGGER_COOLDOWN_SECS);
    let should_spawn = QUOTA_SNAPSHOT_REFRESH_ONCE_GATE
        .lock()
        .map(|mut gate| gate.should_spawn(&key, std::time::Instant::now(), cooldown))
        .unwrap_or(true);
    if !should_spawn {
        bridge_debug_log(&format!(
            "[QuotaSnapshot] refresh coalesced: reason={}, key={}",
            reason, key
        ));
        return;
    }

    tokio::spawn(async move {
        refresh_quota_snapshot_and_broadcast(
            app_handle.as_ref(),
            &tx,
            reason,
            codex_home.as_deref(),
        )
        .await;
        if let Ok(mut gate) = QUOTA_SNAPSHOT_REFRESH_ONCE_GATE.lock() {
            gate.finish(&key);
        }
    });
}

fn quota_snapshot_refresh_key(codex_home: Option<&str>) -> String {
    codex_home
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("default")
        .to_string()
}

async fn refresh_quota_snapshot_and_broadcast(
    app_handle: Option<&AppHandle>,
    tx: &broadcast::Sender<BridgeMessage>,
    reason: &str,
    codex_home: Option<&str>,
) {
    let has_ws_subscribers = tx.receiver_count() > 0;
    let Some(snapshot) =
        crate::ui::quota_snapshot::refresh_quota_snapshot_for_app(app_handle, codex_home).await
    else {
        return;
    };

    if !has_ws_subscribers {
        return;
    }

    let mut payload = serde_json::json!({
        "sync_response": true,
        "suppress_remote_notification": true,
        "cache_source": reason,
        "sync_reason": reason,
    });
    crate::ui::quota_snapshot::inject_quota_snapshot_in_mcp_state(&mut payload, &snapshot);
    match tx.send(BridgeMessage {
        message_type: "mcp_state".to_string(),
        payload,
    }) {
        Ok(sent_count) => bridge_debug_log(&format!(
            "[QuotaSnapshot] refresh broadcast sent={} reason={}",
            sent_count, reason
        )),
        Err(err) => bridge_debug_log(&format!(
            "[QuotaSnapshot] refresh broadcast skipped: reason={}, error={}",
            reason, err
        )),
    }
}

async fn has_quota_live_activity_tokens() -> bool {
    let tokens = apns_live_activity_tokens_snapshot().await;
    tokens.values().any(|info| {
        live_activity_info_matches(info, LIVE_ACTIVITY_KIND_QUOTA, QUOTA_LIVE_ACTIVITY_KEY)
    })
}

fn inject_cached_quota_snapshot_and_refresh_async(
    app_handle: Option<&AppHandle>,
    tx: &broadcast::Sender<BridgeMessage>,
    payload: &mut serde_json::Value,
    codex_home: Option<String>,
    reason: &'static str,
) {
    crate::ui::quota_snapshot::inject_current_quota_snapshot_in_mcp_state(app_handle, payload);
    spawn_quota_snapshot_refresh_once_for_codex_home(
        app_handle.cloned(),
        tx.clone(),
        reason,
        codex_home,
    );
}

async fn probe_tcp_port(port: u16) -> (serde_json::Value, bool) {
    let started_at = std::time::Instant::now();
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    {
        Ok(Ok(_stream)) => (
            serde_json::json!({
                "port": port,
                "tcp_reachable": true,
                "latency_ms": started_at.elapsed().as_millis(),
                "error": serde_json::Value::Null,
            }),
            true,
        ),
        Ok(Err(err)) => (
            serde_json::json!({
                "port": port,
                "tcp_reachable": false,
                "latency_ms": started_at.elapsed().as_millis(),
                "error": err.to_string(),
            }),
            false,
        ),
        Err(_) => (
            serde_json::json!({
                "port": port,
                "tcp_reachable": false,
                "latency_ms": started_at.elapsed().as_millis(),
                "error": "timeout",
            }),
            false,
        ),
    }
}

const MCP_REGISTERED_PORT_LIMIT: usize = 32;
const MCP_REGISTERED_PORT_PROBE_TIMEOUT_MS: u64 = 500;

fn registered_mcp_ports_from_dir(port_dir: &std::path::Path) -> Vec<u16> {
    let mut ports = Vec::new();
    if let Ok(entries) = std::fs::read_dir(port_dir) {
        for entry in entries.flatten() {
            let Some(name) = entry.file_name().to_str().map(ToOwned::to_owned) else {
                continue;
            };
            let Ok(port) = name.parse::<u16>() else {
                continue;
            };
            ports.push(port);
        }
    }
    ports.sort_unstable();
    ports.dedup();
    ports
}

fn registered_mcp_ports() -> Vec<u16> {
    dirs::home_dir()
        .map(|home| registered_mcp_ports_from_dir(&home.join(".cunzhi_ports")))
        .unwrap_or_default()
}

async fn probe_registered_mcp_ports() -> (Vec<serde_json::Value>, bool) {
    let mut probes = Vec::new();
    let mut any_healthy = false;

    for port in registered_mcp_ports()
        .into_iter()
        .take(MCP_REGISTERED_PORT_LIMIT)
    {
        let url = format!("http://127.0.0.1:{port}/health");
        let probe = match tokio::time::timeout(
            std::time::Duration::from_millis(MCP_REGISTERED_PORT_PROBE_TIMEOUT_MS),
            probe_http_endpoint(&url),
        )
        .await
        {
            Ok((probe, healthy)) => {
                any_healthy |= healthy;
                probe
            }
            Err(_) => serde_json::json!({
                "url": url,
                "healthy": false,
                "status_code": serde_json::Value::Null,
                "latency_ms": MCP_REGISTERED_PORT_PROBE_TIMEOUT_MS,
                "timeout_secs": 0.5,
                "probe_mode": "direct",
                "error": "timeout",
            }),
        };

        probes.push(serde_json::json!({
            "port": port,
            "probe": probe,
        }));
    }

    (probes, any_healthy)
}

async fn probe_mcp_response_runtime(default_port: u16) -> (serde_json::Value, bool) {
    let (default_probe, default_reachable) = probe_tcp_port(default_port).await;
    let (registered_ports, registered_reachable) = probe_registered_mcp_ports().await;
    let effective_reachable = default_reachable || registered_reachable;
    let effective_source = if default_reachable {
        "default_port"
    } else if registered_reachable {
        "registered_port"
    } else {
        "none"
    };

    let mut probe = default_probe;
    if let Some(object) = probe.as_object_mut() {
        object.insert("default_port".to_string(), serde_json::json!(default_port));
        object.insert(
            "effective_reachable".to_string(),
            serde_json::json!(effective_reachable),
        );
        object.insert(
            "effective_source".to_string(),
            serde_json::json!(effective_source),
        );
        object.insert(
            "registered_port_count".to_string(),
            serde_json::json!(registered_ports.len()),
        );
        object.insert(
            "registered_ports".to_string(),
            serde_json::json!(registered_ports),
        );
    }

    (probe, effective_reachable)
}

fn root_tunnel_status_age_secs(status_file: Option<&serde_json::Value>) -> Option<i64> {
    let updated_at = status_file?
        .get("updated_at")
        .and_then(|value| value.as_str())?;
    let updated_at = chrono::DateTime::parse_from_rfc3339(updated_at)
        .ok()?
        .with_timezone(&chrono::Utc);
    Some(
        chrono::Utc::now()
            .signed_duration_since(updated_at)
            .num_seconds()
            .max(0),
    )
}

fn root_tunnel_is_authoritative_up(root_tunnel: &serde_json::Value) -> bool {
    let metrics_http_ok = root_tunnel
        .get("metrics")
        .and_then(|metrics| metrics.get("http_ok"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let live_ha_count = root_tunnel
        .get("metrics")
        .and_then(|metrics| metrics.get("ha_connection_count"))
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let status_ha_count = root_tunnel
        .get("metrics")
        .and_then(|metrics| metrics.get("status_ha_connection_count"))
        .and_then(|value| value.as_f64())
        .unwrap_or(0.0);
    let expected_ha_count = root_tunnel
        .get("metrics")
        .and_then(|metrics| metrics.get("expected_ha_connections"))
        .and_then(|value| value.as_f64())
        .filter(|value| *value > 0.0)
        .unwrap_or(ROOT_TUNNEL_EXPECTED_HA_CONNECTIONS);
    let status_fresh = root_tunnel
        .get("status_fresh")
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let child_alive = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("child_alive"))
        .and_then(|value| value.as_bool())
        .unwrap_or(live_ha_count > 0.0);
    let structural_block = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("structural_block"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let edge_7844_suspected = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("edge_7844_suspected"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let needs_edge_path_fix = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("tunnel_health_class"))
        .and_then(|value| value.as_str())
        .map(|value| value == "needs_edge_path_fix")
        .unwrap_or(false);
    let ha_ready = live_ha_count >= expected_ha_count
        || (status_fresh && status_ha_count >= expected_ha_count);

    metrics_http_ok
        && child_alive
        && ha_ready
        && !structural_block
        && !edge_7844_suspected
        && !needs_edge_path_fix
}

fn root_tunnel_supervisor_fields_from_status(
    status_file: Option<&serde_json::Value>,
) -> serde_json::Value {
    let tunnel_health_class = status_file
        .and_then(|status| status.get("tunnel_health_class"))
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_string();
    let last_skip_reason = status_file
        .and_then(|status| status.get("last_skip_reason"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let structural_block = status_file
        .and_then(|status| status.get("structural_block"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let edge_7844_suspected = status_file
        .and_then(|status| status.get("edge_7844_suspected"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let edge_7844_probe_ok = status_file
        .and_then(|status| status.get("edge_7844_probe_ok"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let edge_7844_checked_at = status_file
        .and_then(|status| status.get("edge_7844_checked_at"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let edge_7844_failure_reason = status_file
        .and_then(|status| status.get("edge_7844_failure_reason"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let edge_7844_last_url = status_file
        .and_then(|status| status.get("edge_7844_last_url"))
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .to_string();
    let escalation_count_hour = status_file
        .and_then(|status| status.get("escalation_count_hour"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let max_escalations_per_hour = status_file
        .and_then(|status| status.get("max_escalations_per_hour"))
        .and_then(|value| value.as_i64())
        .unwrap_or(3);
    let next_action_at = status_file
        .and_then(|status| status.get("next_action_at"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let observe_only_until = status_file
        .and_then(|status| status.get("observe_only_until"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let backoff_remaining_secs = status_file
        .and_then(|status| status.get("backoff_remaining_secs"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);

    serde_json::json!({
        "backoff_active": backoff_remaining_secs > 0,
        "backoff_remaining_secs": backoff_remaining_secs,
        "tunnel_health_class": tunnel_health_class,
        "last_skip_reason": last_skip_reason,
        "structural_block": structural_block,
        "edge_7844_suspected": edge_7844_suspected,
        "edge_7844_probe_ok": edge_7844_probe_ok,
        "edge_7844_checked_at": edge_7844_checked_at,
        "edge_7844_failure_reason": edge_7844_failure_reason,
        "edge_7844_last_url": edge_7844_last_url,
        "escalation_count_hour": escalation_count_hour,
        "max_escalations_per_hour": max_escalations_per_hour,
        "next_action_at": next_action_at,
        "observe_only_until": observe_only_until,
    })
}

fn parse_root_tunnel_ha_connections(metrics: &str) -> Option<f64> {
    const METRIC_NAME: &str = concat!("cloudflared_", "tunnel_ha_connections");
    metrics.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        if fields.next()? != METRIC_NAME {
            return None;
        }
        fields.next()?.parse::<f64>().ok()
    })
}

async fn probe_root_tunnel_ha_connections(url: &str) -> Option<f64> {
    let timeout_secs = http_probe_timeout_secs(url);
    let client = build_probe_client(timeout_secs).ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let body = response.text().await.ok()?;
    parse_root_tunnel_ha_connections(&body)
}

async fn inspect_root_tunnel_runtime() -> serde_json::Value {
    let status_file = std::fs::read_to_string(ROOT_TUNNEL_STATUS_FILE)
        .ok()
        .and_then(|content| serde_json::from_str::<serde_json::Value>(&content).ok());
    let (metrics_probe, metrics_http_ok) = probe_http_endpoint(ROOT_TUNNEL_METRICS_URL).await;
    let live_ha_connection_count = probe_root_tunnel_ha_connections(ROOT_TUNNEL_METRICS_URL).await;
    let ha_connections = live_ha_connection_count.map(|value| value.to_string());
    let ha_connection_count = live_ha_connection_count.unwrap_or(0.0);
    let root_launchctl =
        debug_launchctl_label("system/xin.tobooks.cunzhi.cloudflared-proxied.root").await;
    let status_age_secs = root_tunnel_status_age_secs(status_file.as_ref());
    let status_fresh = status_age_secs
        .map(|age| age <= ROOT_TUNNEL_STATUS_MAX_AGE_SECS)
        .unwrap_or(false);
    let status_expected_ha_connections = status_file
        .as_ref()
        .filter(|_| status_fresh)
        .and_then(|status| status.get("expected_ha_connections"))
        .and_then(|value| value.as_f64())
        .unwrap_or(ROOT_TUNNEL_EXPECTED_HA_CONNECTIONS);
    let status_ha_connection_count = status_file
        .as_ref()
        .filter(|_| status_fresh)
        .and_then(|status| status.get("ha_connection_count"))
        .and_then(|value| value.as_f64())
        .unwrap_or(ha_connection_count);
    let effective_ha_connection_count = ha_connection_count.max(status_ha_connection_count);
    let effective_expected_ha_connections = if status_expected_ha_connections > 0.0 {
        status_expected_ha_connections
    } else {
        ROOT_TUNNEL_EXPECTED_HA_CONNECTIONS
    };
    let ha_active = effective_ha_connection_count > 0.0;
    let ha_ready = effective_ha_connection_count >= effective_expected_ha_connections;
    let ha_degraded = ha_active && !ha_ready;
    let child_alive = status_file
        .as_ref()
        .filter(|_| status_fresh)
        .and_then(|status| status.get("child_alive"))
        .and_then(|value| value.as_bool())
        .unwrap_or(ha_active);
    let backoff_remaining_secs = status_file
        .as_ref()
        .and_then(|status| status.get("backoff_remaining_secs"))
        .and_then(|value| value.as_i64())
        .unwrap_or(0);
    let supervisor_fields = root_tunnel_supervisor_fields_from_status(status_file.as_ref());

    serde_json::json!({
        "launchctl": root_launchctl,
        "status_file_path": ROOT_TUNNEL_STATUS_FILE,
        "status_age_secs": status_age_secs,
        "status_fresh": status_fresh,
        "status_max_age_secs": ROOT_TUNNEL_STATUS_MAX_AGE_SECS,
        "status": status_file,
        "metrics": {
            "http_ok": metrics_http_ok,
            "probe": metrics_probe,
            "ha_connections": ha_connections,
            "ha_connection_count": ha_connection_count,
            "status_ha_connection_count": status_ha_connection_count,
            "effective_ha_connection_count": effective_ha_connection_count,
            "expected_ha_connections": effective_expected_ha_connections,
            "ha_active": ha_active,
            "ha_ready": ha_ready,
            "ha_degraded": ha_degraded,
        },
        "derived": {
            "child_alive": child_alive,
            "ha_active": ha_active,
            "ha_ready": ha_ready,
            "ha_degraded": ha_degraded,
            "backoff_active": supervisor_fields
                .get("backoff_active")
                .and_then(|value| value.as_bool())
                .unwrap_or(backoff_remaining_secs > 0),
            "backoff_remaining_secs": supervisor_fields
                .get("backoff_remaining_secs")
                .and_then(|value| value.as_i64())
                .unwrap_or(backoff_remaining_secs),
            "tunnel_health_class": supervisor_fields
                .get("tunnel_health_class")
                .and_then(|value| value.as_str())
                .unwrap_or("unknown"),
            "last_skip_reason": supervisor_fields
                .get("last_skip_reason")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
            "structural_block": supervisor_fields
                .get("structural_block")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            "edge_7844_suspected": supervisor_fields
                .get("edge_7844_suspected")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            "edge_7844_probe_ok": supervisor_fields
                .get("edge_7844_probe_ok")
                .and_then(|value| value.as_bool())
                .unwrap_or(false),
            "edge_7844_checked_at": supervisor_fields
                .get("edge_7844_checked_at")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
            "edge_7844_failure_reason": supervisor_fields
                .get("edge_7844_failure_reason")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
            "edge_7844_last_url": supervisor_fields
                .get("edge_7844_last_url")
                .and_then(|value| value.as_str())
                .unwrap_or(""),
            "escalation_count_hour": supervisor_fields
                .get("escalation_count_hour")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            "max_escalations_per_hour": supervisor_fields
                .get("max_escalations_per_hour")
                .and_then(|value| value.as_i64())
                .unwrap_or(3),
            "next_action_at": supervisor_fields
                .get("next_action_at")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
            "observe_only_until": supervisor_fields
                .get("observe_only_until")
                .and_then(|value| value.as_i64())
                .unwrap_or(0),
        }
    })
}

async fn diagnostic_command_stdout(program: &str, args: &[String]) -> Option<String> {
    let timeout_secs = match program {
        "lsof" | "ps" => 5,
        _ => 2,
    };
    let mut command = tokio::process::Command::new(program);
    command.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.as_std_mut().creation_flags(CREATE_NO_WINDOW);
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        command.output(),
    )
    .await
    .ok()?
    .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
}

fn workspace_from_command(command: &str) -> Option<String> {
    let parts = command.split_whitespace().collect::<Vec<_>>();
    for (index, part) in parts.iter().enumerate() {
        if *part == "--workspace" {
            return parts.get(index + 1).map(|value| value.to_string());
        }
        if let Some(value) = part.strip_prefix("--workspace=") {
            if !value.trim().is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

async fn inspect_tcp_listener_owner(port: u16) -> serde_json::Value {
    let lsof_args = vec![
        "-nP".to_string(),
        format!("-iTCP:{port}"),
        "-sTCP:LISTEN".to_string(),
        "-Fpc".to_string(),
    ];
    let Some(lsof_output) = diagnostic_command_stdout("lsof", &lsof_args).await else {
        return serde_json::Value::Null;
    };

    let mut pid: Option<u32> = None;
    let mut command_name: Option<String> = None;
    for line in lsof_output.lines() {
        if let Some(raw_pid) = line.strip_prefix('p') {
            pid = raw_pid.parse::<u32>().ok();
        } else if let Some(raw_command) = line.strip_prefix('c') {
            command_name = Some(raw_command.to_string());
        }
        if pid.is_some() && command_name.is_some() {
            break;
        }
    }

    let Some(pid) = pid else {
        return serde_json::Value::Null;
    };
    let command_line = diagnostic_command_stdout(
        "ps",
        &[
            "-p".to_string(),
            pid.to_string(),
            "-o".to_string(),
            "command=".to_string(),
        ],
    )
    .await;
    let command = command_line
        .as_deref()
        .and_then(|line| line.split_whitespace().next())
        .map(ToOwned::to_owned);

    serde_json::json!({
        "pid": pid,
        "command": command_name,
        "exe": command,
        "args": command_line,
        "workspace": command_line.as_deref().and_then(workspace_from_command),
    })
}

fn status_hint(level: &str, code: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "level": level,
        "code": code,
        "message": message,
    })
}

fn build_redacted_connection_status_value(
    generated_at: &str,
    diagnosis_code: &str,
    public_healthy: bool,
    public_ws_effective_ok: bool,
    public_ws_auth_required: bool,
    hints: Vec<serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "ok": true,
        "generated_at": generated_at,
        "version": env!("CARGO_PKG_VERSION"),
        "diagnosis": {
            "code": diagnosis_code,
        },
        "public_tunnel": {
            "healthy": public_healthy,
            "websocket_healthy": public_ws_effective_ok,
            "websocket_auth_required": public_ws_auth_required,
        },
        "hints": hints,
    })
}

async fn debug_launchctl_label(label: &str) -> String {
    match tokio::process::Command::new("launchctl")
        .args(["print", label])
        .output()
        .await
    {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            let combined = format!("{stdout}\n{stderr}");
            let summary = combined
                .lines()
                .map(str::trim)
                .find(|line| {
                    line.contains("state =")
                        || line.contains("pid =")
                        || line.contains("last exit code =")
                })
                .unwrap_or("-");
            format!(
                "status={:?}, summary={}",
                output.status.code(),
                summary.replace(' ', "_")
            )
        }
        Err(err) => format!("command_error:{err}"),
    }
}

/// POST /api/restart-service — 完整服务重启（HTTP + tunnel）
async fn handle_api_restart_service(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_SERVICE_RECOVER,
        "missing_scope_service_recover",
    )
    .await
    {
        return response;
    }

    let Some(app_handle) = state.app_handle.clone() else {
        return Json(serde_json::json!({
            "ok": false,
            "error": "bridge_only_daemon_cannot_restart_gui"
        }))
        .into_response();
    };
    instance_debug_log(
        "[api-restart-service]",
        format!(
            "caller_pid={}, scheduling_restart=true, cloudflared_action=skip",
            std::process::id()
        ),
    );

    // 系统级 cloudflared daemon 作为生产 tunnel 常驻托管。服务重启入口只重启
    // iterate 应用本身，避免把本地 bridge 恢复和 tunnel 重启耦合在一起。
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        log::info!("[Bridge] 收到服务重启请求，重启 iterate 应用...");
        instance_debug_log(
            "[api-restart-service-restart-app]",
            "calling app_handle.restart()",
        );
        app_handle.restart();
    });

    Json(serde_json::json!({
        "ok": true,
        "message": "iterate 应用将在 0.5 秒后重启（不会重启 cloudflared）"
    }))
    .into_response()
}

/// GET /api/connection-status — 返回只读连接诊断信息，不触发恢复动作
async fn handle_api_connection_status(
    State(state): State<BridgeHttpState>,
    headers: HeaderMap,
) -> Response {
    let public_request = is_public_bridge_request(&headers);
    let requires_device_auth = public_route_requires_auth(&headers);
    let trusted_internal = trusted_internal_capability(&headers);
    let auth_principal = if trusted_internal {
        None
    } else {
        match authenticate_bridge_headers_result(&headers).await {
            Ok(principal) => principal,
            Err(error) => return bridge_auth_error_response(&error),
        }
    };
    if !trusted_internal {
        if let Some((status, error)) = status_read_full_diagnostics_denial(
            auth_principal.as_ref(),
            public_request,
            requires_device_auth,
        ) {
            return json_error_response(status, error);
        }
    }
    let redact_public_anonymous = public_request && auth_principal.is_none();

    let port = state.port;
    let tx = state.tx;
    let local_version_url = format!("http://127.0.0.1:{}/api/version", port);
    let local_ws_url = format!("http://127.0.0.1:{}/ws", port);
    let generated_at = chrono::Utc::now().to_rfc3339();
    let (
        (local_origin, local_healthy),
        (local_ws, local_ws_ok),
        local_owner,
        (public_tunnel, public_probe_healthy, public_ws, public_ws_ok),
        root_tunnel,
        tunnel_manager,
        (mcp_probe, mcp_reachable),
        mcp_owner,
    ) = tokio::join!(
        probe_http_endpoint(&local_version_url),
        probe_websocket_upgrade_endpoint(&local_ws_url),
        inspect_tcp_listener_owner(port),
        get_public_probe_snapshot(),
        inspect_root_tunnel_runtime(),
        crate::tunnel::manager::get_status(),
        probe_mcp_response_runtime(5311),
        inspect_tcp_listener_owner(5311),
    );
    let public_ws_auth_required = websocket_probe_auth_required(&public_ws);
    let public_ws_effective_ok =
        websocket_probe_ok_or_auth_required(public_ws_ok, &public_ws, true);
    let root_child_alive = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("child_alive"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let root_backoff_active = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("backoff_active"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let root_ha_degraded = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("ha_degraded"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    let root_structural_block = root_tunnel
        .get("derived")
        .and_then(|derived| derived.get("structural_block"))
        .and_then(|value| value.as_bool())
        .unwrap_or(false);
    // root cloudflared HA 满连接且 metrics 健康 = 公网隧道实际可用（手机直连走的就是这条路），
    // 这个信号比电脑本机发卡弯探针更权威；探针偶发超时不再被误判为“公网 down”。
    let root_tunnel_authoritative_up = root_tunnel_is_authoritative_up(&root_tunnel);
    let public_healthy = public_probe_healthy || root_tunnel_authoritative_up;
    let public_ws_effective_ok = public_ws_effective_ok || root_tunnel_authoritative_up;
    let public_health_source = if public_probe_healthy {
        "probe"
    } else if root_tunnel_authoritative_up {
        "root_tunnel_ha"
    } else {
        "none"
    };
    let active_registry_count = ACTIVE_SESSION_REGISTRY.read().await.len();
    let mut window_registry = crate::ui::window_registry::WindowRegistry::load();
    let live_window_count = window_registry.get_all_instances().len();
    let mcp_state_count = MCP_STATE_CACHE.read().await.len();
    let mcp_action_count = MCP_ACTION_CACHE.read().await.len();
    let pairing_token_count = MOBILE_PAIRING_TOKENS.read().await.len();
    let apns_device_count = apns_device_token_count().await;
    let push_subscription_count = PUSH_SUBSCRIPTIONS.read().await.len();
    let ws_clients = snapshot_ws_clients().await;
    let routes = route_debug_status_value().await;

    let mut hints = Vec::new();
    if !local_healthy {
        hints.push(status_hint(
            "error",
            "local_origin_down",
            &format!("Local {} bridge health check failed.", port),
        ));
    } else if !local_ws_ok {
        hints.push(status_hint(
            "error",
            "local_ws_unavailable",
            &format!(
                "Local {} HTTP is healthy but WebSocket upgrade failed.",
                port
            ),
        ));
    } else if !root_child_alive {
        hints.push(status_hint(
            "error",
            "root_tunnel_child_missing",
            "Root cloudflared supervisor is present but no healthy child/HA metrics were observed.",
        ));
    } else if root_structural_block && !public_healthy {
        hints.push(status_hint(
            "warning",
            "root_tunnel_structural_block",
            "Root cloudflared auto-recovery is paused because the Cloudflare edge path looks structurally unreachable; Tailscale/local bridge may still work.",
        ));
    } else if root_ha_degraded && !public_healthy {
        hints.push(status_hint(
            "warning",
            "root_tunnel_ha_degraded",
            "Root cloudflared has fewer than 4 HA connections while the public probe is unhealthy.",
        ));
    } else if root_backoff_active && !public_healthy {
        hints.push(status_hint(
            "warning",
            "root_tunnel_backoff_active",
            "Root cloudflared recovery is in backoff while the public probe is unhealthy.",
        ));
    } else if !public_healthy {
        hints.push(status_hint(
            "warning",
            "public_tunnel_down_local_ok",
            "Local bridge is healthy but the public tunnel health check failed.",
        ));
    } else if !public_ws_effective_ok {
        hints.push(status_hint(
            "info",
            "public_ws_auth_required",
            "Public HTTP is healthy and public WebSocket is protected by mobile auth.",
        ));
    }
    if !mcp_reachable {
        hints.push(status_hint(
            "warning",
            "mcp_response_unreachable",
            "MCP response path probe failed; UI may connect while the MCP response path is unavailable.",
        ));
    }
    if active_registry_count == 0 && live_window_count == 0 {
        hints.push(status_hint(
            "info",
            "no_active_sessions",
            "No active desktop windows are currently registered.",
        ));
    }
    if mcp_action_count >= MCP_ACTION_CACHE_MAX_ENTRIES
        || mcp_state_count >= MCP_STATE_CACHE_MAX_ENTRIES
    {
        hints.push(status_hint(
            "warning",
            "cache_near_capacity",
            "MCP state/action cache count is at or above configured capacity.",
        ));
    }

    let diagnosis_code = if !local_healthy {
        "local_origin_down"
    } else if !local_ws_ok {
        "local_ws_unavailable"
    } else if !root_child_alive {
        "root_tunnel_child_missing"
    } else if root_structural_block && !public_healthy {
        "root_tunnel_structural_block"
    } else if root_ha_degraded && !public_healthy {
        "root_tunnel_ha_degraded"
    } else if root_backoff_active && !public_healthy {
        "root_tunnel_backoff_active"
    } else if !public_healthy {
        "public_tunnel_down_local_ok"
    } else if !public_ws_effective_ok {
        "public_ws_unavailable"
    } else {
        "ok"
    };
    let websocket_client_count = tx.receiver_count();

    if redact_public_anonymous {
        return Json(build_redacted_connection_status_value(
            &generated_at,
            diagnosis_code,
            public_healthy,
            public_ws_effective_ok,
            public_ws_auth_required,
            hints,
        ))
        .into_response();
    }

    Json(serde_json::json!({
        "ok": true,
        "generated_at": generated_at,
        "version": env!("CARGO_PKG_VERSION"),
        "diagnosis": {
            "code": diagnosis_code,
        },
        "local_origin": {
            "healthy": local_healthy,
            "probe": local_origin,
            "websocket": local_ws,
            "owner": local_owner,
        },
        "public_tunnel": {
            "healthy": public_healthy,
            "health_source": public_health_source,
            "probe": public_tunnel,
            "websocket": public_ws,
            "websocket_healthy": public_ws_effective_ok,
            "websocket_auth_required": public_ws_auth_required,
            "manager": tunnel_manager,
        },
        "root_tunnel": root_tunnel,
        "mcp": {
            "probe": mcp_probe,
            "owner": mcp_owner,
        },
        "sessions": {
            "active_registry_count": active_registry_count,
            "live_window_count": live_window_count,
        },
        "caches": {
            "mcp_state_count": mcp_state_count,
            "mcp_action_count": mcp_action_count,
            "mcp_state_ttl_secs": MCP_STATE_CACHE_TTL_SECS,
            "mcp_action_ttl_secs": MCP_ACTION_CACHE_TTL_SECS,
            "mcp_state_max_entries": MCP_STATE_CACHE_MAX_ENTRIES,
            "mcp_action_max_entries": MCP_ACTION_CACHE_MAX_ENTRIES,
            "mcp_state": {
                "count": mcp_state_count,
                "ttl_secs": MCP_STATE_CACHE_TTL_SECS,
                "max_entries": MCP_STATE_CACHE_MAX_ENTRIES,
                "metrics": MCP_STATE_CACHE_METRICS.snapshot(),
            },
            "mcp_action": {
                "count": mcp_action_count,
                "ttl_secs": MCP_ACTION_CACHE_TTL_SECS,
                "max_entries": MCP_ACTION_CACHE_MAX_ENTRIES,
                "metrics": MCP_ACTION_CACHE_METRICS.snapshot(),
            },
            "pairing_token_count": pairing_token_count,
            "apns_device_count": apns_device_count,
            "push_subscription_count": push_subscription_count,
        },
        "websocket": {
            "client_count": websocket_client_count,
            "subscriber_count": websocket_client_count,
            "registry_count": ws_clients.len(),
            "clients": ws_clients,
        },
        "routes": routes,
        "hints": hints,
    }))
    .into_response()
}

/// GET /api/version — 返回版本信息
async fn handle_api_version() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "name": "iterate",
        "description": "智能代码审查工具，支持MCP协议集成"
    }))
}

/// GET /.well-known/iterate/health — Cloudflare Web Login public health probe
async fn handle_well_known_iterate_health() -> Json<serde_json::Value> {
    let (endpoint_proof, endpoint_epoch) = crate::tunnel::manager::quick_tunnel_public_proof();
    let installation_proof = crate::tunnel::manager::installation_public_proof();
    Json(serde_json::json!({
        "ok": true,
        "service": "iterate",
        "version": env!("CARGO_PKG_VERSION"),
        "public_surface": "cloudflare_web_login",
        "installation_proof": installation_proof,
        "endpoint_proof": endpoint_proof,
        "endpoint_epoch": endpoint_epoch,
        "capabilities": {
            "pair_challenge": true,
            "websocket": true
        }
    }))
}

/// POST /pair/challenge — public route smoke test, not a login/session claim
async fn handle_pair_challenge() -> Json<serde_json::Value> {
    let issued_at = chrono::Utc::now();
    Json(serde_json::json!({
        "ok": true,
        "challenge": generate_bridge_token("pc"),
        "issued_at": issued_at.to_rfc3339(),
        "expires_at": (issued_at + chrono::Duration::minutes(2)).to_rfc3339(),
        "scope": "pair_challenge",
        "session_issued": false
    }))
}

async fn handle_pair_page(Query(query): Query<WebLoginPairPageQuery>) -> Response {
    let page_state = serde_json::json!({
        "nonce": query.nonce,
        "device_id": query.device_id,
        "cf_origin": query.cf_origin,
        "scopes": default_web_login_scopes(),
    });
    let page_state_json = serde_json::to_string(&page_state)
        .unwrap_or_else(|_| "{}".to_string())
        .replace("</", "<\\/");
    let html = format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>iterate Web Login</title>
  <style>
    :root {{
      color-scheme: light dark;
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
      background: #0f172a;
      color: #e5e7eb;
    }}
    body {{
      margin: 0;
      min-height: 100vh;
      display: grid;
      place-items: center;
      padding: 24px;
      box-sizing: border-box;
    }}
    main {{
      width: min(420px, 100%);
      border: 1px solid rgba(148, 163, 184, 0.28);
      border-radius: 8px;
      padding: 20px;
      background: #111827;
      box-shadow: 0 18px 42px rgba(0, 0, 0, 0.28);
    }}
    h1 {{
      margin: 0 0 8px;
      font-size: 20px;
      line-height: 1.2;
      letter-spacing: 0;
    }}
    p {{
      margin: 0 0 16px;
      color: #9ca3af;
      font-size: 13px;
      line-height: 1.6;
    }}
    dl {{
      margin: 0 0 16px;
      display: grid;
      grid-template-columns: 88px 1fr;
      gap: 8px;
      font-size: 12px;
      line-height: 1.4;
    }}
    dt {{
      color: #9ca3af;
    }}
    dd {{
      margin: 0;
      min-width: 0;
      overflow-wrap: anywhere;
      color: #f9fafb;
    }}
    button {{
      width: 100%;
      border: 0;
      border-radius: 6px;
      padding: 11px 12px;
      background: #2563eb;
      color: white;
      font-weight: 600;
      cursor: pointer;
      font-size: 14px;
    }}
    button:disabled {{
      cursor: default;
      opacity: 0.65;
    }}
    .status {{
      margin-top: 12px;
      min-height: 18px;
      font-size: 12px;
      color: #9ca3af;
      overflow-wrap: anywhere;
    }}
    .ok {{ color: #86efac; }}
    .error {{ color: #fca5a5; }}
  </style>
</head>
<body>
  <main>
    <h1>iterate Web Login</h1>
    <p>Authorize this browser for the current iterate Cloudflare session.</p>
    <dl>
      <dt>Origin</dt><dd id="origin"></dd>
      <dt>Device</dt><dd id="device"></dd>
      <dt>Scopes</dt><dd id="scopes"></dd>
    </dl>
    <button id="claim" type="button">Authorize Browser</button>
    <div id="status" class="status"></div>
  </main>
  <script>
    const pairing = {page_state_json};
    const statusEl = document.getElementById('status');
    const button = document.getElementById('claim');
    document.getElementById('origin').textContent = pairing.cf_origin || '';
    document.getElementById('device').textContent = pairing.device_id || '';
    document.getElementById('scopes').textContent = (pairing.scopes || []).join(', ');
    function setStatus(text, className) {{
      statusEl.textContent = text;
      statusEl.className = 'status ' + (className || '');
    }}
    button.addEventListener('click', async () => {{
      button.disabled = true;
      setStatus('Authorizing...', '');
      try {{
        const response = await fetch(new URL('/pair/claim', pairing.cf_origin).toString(), {{
          method: 'POST',
          credentials: 'include',
          headers: {{ 'Content-Type': 'application/json' }},
          body: JSON.stringify({{
            nonce: pairing.nonce,
            device_id: pairing.device_id,
            cf_origin: pairing.cf_origin,
            requested_scopes: pairing.scopes || []
          }})
        }});
        const body = await response.json().catch(() => ({{}}));
        if (!response.ok || !body.ok) {{
          throw new Error(body.error || ('HTTP ' + response.status));
        }}
        localStorage.setItem('iterate.webSessionId', body.session_id);
        localStorage.setItem('iterate.webSessionExpiresAt', body.expires_at);
        setStatus('Authorized. This browser can now connect to iterate until ' + new Date(body.expires_at).toLocaleString() + '.', 'ok');
      }} catch (error) {{
        button.disabled = false;
        setStatus(String(error && error.message ? error.message : error), 'error');
      }}
    }});
  </script>
</body>
</html>"#
    );
    Html(html).into_response()
}

fn authorization_bearer_token_from_headers(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .and_then(|value| {
            let mut parts = value.splitn(2, char::is_whitespace);
            let scheme = parts.next().unwrap_or_default();
            let token = parts.next().unwrap_or_default().trim();
            if scheme.eq_ignore_ascii_case("bearer") && !token.is_empty() {
                Some(token.to_string())
            } else {
                None
            }
        })
}

async fn authorize_web_login_session(
    headers: &HeaderMap,
) -> Result<(AuthPrincipal, String), Response> {
    let token = authorization_bearer_token_from_headers(headers)
        .or_else(|| crate::bridge::auth::cookie_token_from_headers(headers))
        .ok_or_else(|| json_error_response(StatusCode::UNAUTHORIZED, "invalid_web_session"))?;
    let principal = authenticate_bridge_headers_result(headers)
        .await
        .map_err(|error| bridge_auth_error_response(&error))?
        .ok_or_else(|| json_error_response(StatusCode::UNAUTHORIZED, "invalid_web_session"))?;
    if principal.client_kind != "web" {
        return Err(json_error_response(
            StatusCode::UNAUTHORIZED,
            "invalid_web_session",
        ));
    }
    Ok((principal, token))
}

fn validated_pair_claim_origin(
    headers: &HeaderMap,
    expected_origin: &str,
) -> Result<(), &'static str> {
    let Some(origin) = headers
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "null")
    else {
        return Err("origin_missing");
    };
    let Ok(origin) = normalize_web_origin(origin) else {
        return Err("origin_invalid");
    };
    if origin != expected_origin {
        return Err("origin_not_allowed");
    }
    Ok(())
}

async fn handle_pair_claim(
    headers: HeaderMap,
    Json(request): Json<WebLoginPairClaimRequest>,
) -> Response {
    let nonce = request.nonce.trim();
    if nonce.is_empty() {
        return json_error_response(StatusCode::UNAUTHORIZED, "invalid_pairing_nonce");
    }
    let device_id = request.device_id.trim();
    if device_id.is_empty() {
        return json_error_response(StatusCode::BAD_REQUEST, "missing_device_id");
    }
    let cf_origin = match normalize_web_origin(&request.cf_origin) {
        Ok(value) => value,
        Err(error) => return json_error_response(StatusCode::BAD_REQUEST, &error),
    };
    let requested_scopes = match normalize_requested_web_scopes(&request.requested_scopes) {
        Ok(value) => value,
        Err(error) => return json_error_response(StatusCode::FORBIDDEN, &error),
    };
    let now = chrono::Utc::now();
    let nonce_info = {
        let mut nonces = WEB_LOGIN_PAIRING_NONCES.write().await;
        prune_web_login_pairing_nonces(&mut nonces, now);
        nonces.get(nonce).cloned()
    };
    let Some(nonce_info) = nonce_info else {
        return json_error_response(StatusCode::UNAUTHORIZED, "invalid_pairing_nonce");
    };
    if parse_rfc3339(&nonce_info.expires_at)
        .map(|expires_at| expires_at <= now)
        .unwrap_or(true)
    {
        return json_error_response(StatusCode::UNAUTHORIZED, "expired_pairing_nonce");
    }
    if nonce_info.device_id != device_id || nonce_info.cf_origin != cf_origin {
        return json_error_response(StatusCode::UNAUTHORIZED, "pairing_context_mismatch");
    }
    if let Err(error) = validated_pair_claim_origin(&headers, &nonce_info.console_origin) {
        let status = if error == "origin_not_allowed" {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::BAD_REQUEST
        };
        return json_error_response(status, error);
    }
    if requested_scopes
        .iter()
        .any(|scope| !nonce_info.scopes.iter().any(|allowed| allowed == scope))
    {
        return json_error_response(StatusCode::FORBIDDEN, "invalid_scope");
    }
    {
        let mut nonces = WEB_LOGIN_PAIRING_NONCES.write().await;
        if nonces.remove(nonce).is_none() {
            return json_error_response(StatusCode::UNAUTHORIZED, "invalid_pairing_nonce");
        }
    }

    let session_token = generate_bridge_token("wsess");
    let session_id = format!("web_{}", uuid::Uuid::new_v4());
    let issued_at = now.to_rfc3339();
    let expires_at = (now + chrono::Duration::seconds(WEB_LOGIN_SESSION_TTL_SECS)).to_rfc3339();
    {
        let mut sessions = WEB_LOGIN_SESSIONS.write().await;
        prune_web_login_sessions(&mut sessions, now);
        sessions.insert(
            bridge_token_hash(&session_token),
            WebLoginSession {
                session_id: session_id.clone(),
                device_id: device_id.to_string(),
                cf_origin,
                console_origin: nonce_info.console_origin,
                scopes: requested_scopes.clone(),
                issued_at: issued_at.clone(),
                expires_at: expires_at.clone(),
                last_seen_at: issued_at.clone(),
                revoked_at: None,
            },
        );
    }

    let cookie = crate::bridge::auth::build_auth_cookie(
        &session_token,
        crate::bridge::auth::should_use_secure_cookie(&headers),
        WEB_LOGIN_SESSION_TTL_SECS,
    );
    let mut response = Json(serde_json::json!({
        "ok": true,
        "device_id": device_id,
        "session_id": session_id,
        "scopes": requested_scopes,
        "issued_at": issued_at,
        "expires_at": expires_at,
    }))
    .into_response();
    if let Ok(cookie) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

async fn handle_session_refresh(headers: HeaderMap) -> Response {
    let (principal, token) = match authorize_web_login_session(&headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let token_hash = bridge_token_hash(&token);
    let now = chrono::Utc::now();
    let expires_at = (now + chrono::Duration::seconds(WEB_LOGIN_SESSION_TTL_SECS)).to_rfc3339();
    let mut sessions = WEB_LOGIN_SESSIONS.write().await;
    prune_web_login_sessions(&mut sessions, now);
    let Some(session) = sessions.get_mut(&token_hash) else {
        return json_error_response(StatusCode::UNAUTHORIZED, "invalid_web_session");
    };
    if session.session_id != principal.principal_id.trim_start_matches("web:") {
        return json_error_response(StatusCode::UNAUTHORIZED, "invalid_web_session");
    }
    session.expires_at = expires_at.clone();
    session.last_seen_at = now.to_rfc3339();
    let response_session_id = session.session_id.clone();
    let response_device_id = session.device_id.clone();
    let response_cf_origin = session.cf_origin.clone();
    let response_scopes = session.scopes.clone();
    drop(sessions);
    let mut response = Json(serde_json::json!({
        "ok": true,
        "session_id": response_session_id,
        "device_id": response_device_id,
        "cf_origin": response_cf_origin,
        "scopes": response_scopes,
        "expires_at": expires_at
    }))
    .into_response();
    let cookie = crate::bridge::auth::build_auth_cookie(
        &token,
        crate::bridge::auth::should_use_secure_cookie(&headers),
        WEB_LOGIN_SESSION_TTL_SECS,
    );
    if let Ok(cookie) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

async fn handle_session_revoke(headers: HeaderMap) -> Response {
    let (_principal, token) = match authorize_web_login_session(&headers).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let token_hash = bridge_token_hash(&token);
    let mut sessions = WEB_LOGIN_SESSIONS.write().await;
    sessions.remove(&token_hash);
    drop(sessions);
    let mut response = Json(serde_json::json!({
        "ok": true,
        "revoked": true
    }))
    .into_response();
    let cookie = crate::bridge::auth::clear_auth_cookie(
        crate::bridge::auth::should_use_secure_cookie(&headers),
    );
    if let Ok(cookie) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(header::SET_COOKIE, cookie);
    }
    response
}

// APNs 设备 Token 注册接口
async fn handle_apns_register(
    headers: HeaderMap,
    Json(request): Json<ApnsRegisterRequest>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_NOTIFICATION_SUBSCRIBE,
        "missing_scope_notification_subscribe",
    )
    .await
    {
        return response;
    }

    let now = apns_now_rfc3339();
    let normalized_device_id = request.device_id.trim().to_string();
    let explicit_environment = request
        .environment
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if explicit_environment.is_none() {
        let Some(notifications_enabled) = request.notifications_enabled else {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "message": "environment is required for new APNs device tokens",
                })),
            )
                .into_response();
        };
        return match update_apns_device_notification_preference(
            &request.device_token,
            &normalized_device_id,
            notifications_enabled,
            &now,
        )
        .await
        {
            Ok(ApnsNotificationPreferenceUpdate::Updated) => {
                log::info!("[APNs] 通知偏好已更新，保留既有 token environment");
                Json(serde_json::json!({
                    "success": true,
                    "message": "Notification preference updated",
                    "environment_preserved": true,
                }))
                .into_response()
            }
            Ok(ApnsNotificationPreferenceUpdate::TokenNotFound) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "message": "environment is required for new APNs device tokens",
                })),
            )
                .into_response(),
            Ok(ApnsNotificationPreferenceUpdate::DeviceMismatch) => (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "message": "device_id does not match the existing APNs token",
                })),
            )
                .into_response(),
            Err(error) => {
                log::warn!("[APNs] 保存通知偏好失败: {}", error);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "success": false,
                        "message": "failed to persist APNs notification preference",
                    })),
                )
                    .into_response()
            }
        };
    }

    let environment =
        match resolve_apns_environment(explicit_environment, bridge_apns_default_environment()) {
            Ok(environment) => environment,
            Err(message) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "success": false,
                        "message": message,
                    })),
                )
                    .into_response();
            }
        };

    let device_info = ApnsDeviceInfo {
        device_token: request.device_token.clone(),
        platform: if request.platform.is_empty() {
            "ios".to_string()
        } else {
            request.platform
        },
        app_version: if request.app_version.is_empty() {
            "1.0".to_string()
        } else {
            request.app_version
        },
        device_id: if normalized_device_id.is_empty() {
            format!(
                "legacy-{}",
                &request.device_token[..12.min(request.device_token.len())]
            )
        } else {
            normalized_device_id.clone()
        },
        registered_at: now.clone(),
        last_seen_at: now,
        notifications_enabled: request.notifications_enabled.unwrap_or(true),
        environment: environment.as_str().to_string(),
    };

    if let Err(e) = register_apns_device_token(
        request.device_token.clone(),
        device_info,
        &normalized_device_id,
    )
    .await
    {
        log::warn!("[APNs] 保存 Token 到文件失败: {}", e);
    }

    log::info!(
        "[APNs] 设备 Token 注册成功: environment={}",
        environment.as_str()
    );

    Json(serde_json::json!({
        "success": true,
        "message": "Device token registered successfully"
    }))
    .into_response()
}

async fn handle_apns_live_activity_register(
    headers: HeaderMap,
    State(state): State<BridgeHttpState>,
    Json(request): Json<ApnsLiveActivityRegisterRequest>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_NOTIFICATION_SUBSCRIBE,
        "missing_scope_notification_subscribe",
    )
    .await
    {
        return response;
    }

    let environment = match resolve_apns_environment(
        request.environment.as_deref(),
        bridge_apns_default_environment(),
    ) {
        Ok(environment) => environment,
        Err(message) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "success": false,
                    "message": message,
                })),
            )
                .into_response();
        }
    };

    let activity_token = request.activity_token.trim().to_string();
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
        .unwrap_or_default();
    let Some(activity_key) = activity_key else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "message": "activity_token and activity_key or goal_id are required"
            })),
        )
            .into_response();
    };
    if activity_token.is_empty() || goal_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "success": false,
                "message": "activity_token and activity_key or goal_id are required"
            })),
        )
            .into_response();
    }

    let now = apns_now_rfc3339();
    let normalized_device_id = request.device_id.trim().to_string();
    let device_id = if normalized_device_id.is_empty() {
        format!(
            "legacy-live-{}",
            &activity_token[..12.min(activity_token.len())]
        )
    } else {
        normalized_device_id.clone()
    };

    let info = ApnsLiveActivityInfo {
        activity_token: activity_token.clone(),
        goal_id: goal_id.clone(),
        activity_kind: activity_kind.clone(),
        activity_key: Some(activity_key.clone()),
        activity_id: request
            .activity_id
            .and_then(|value| trimmed_live_activity_string(Some(&value))),
        device_id: device_id.clone(),
        platform: trimmed_live_activity_string(Some(&request.platform))
            .unwrap_or_else(|| "ios".to_string()),
        app_version: trimmed_live_activity_string(Some(&request.app_version))
            .unwrap_or_else(|| "1.0".to_string()),
        project_path: request
            .project_path
            .and_then(|value| trimmed_live_activity_string(Some(&value))),
        request_id: request
            .request_id
            .and_then(|value| trimmed_live_activity_string(Some(&value))),
        registered_at: now.clone(),
        last_seen_at: now,
        environment: environment.as_str().to_string(),
    };

    let (token_count, save_result) = register_apns_live_activity_token(
        activity_token.clone(),
        info,
        activity_kind.as_str(),
        activity_key.as_str(),
        device_id.as_str(),
    )
    .await;
    if let Err(err) = save_result {
        log::warn!("[APNs LiveActivity] 保存 Token 到文件失败: {}", err);
    }

    log::info!(
        "[APNs LiveActivity] Token 注册成功: kind={}, key={}, goal_id={}, environment={}",
        activity_kind,
        activity_key,
        goal_id,
        environment.as_str()
    );

    if activity_kind == LIVE_ACTIVITY_KIND_QUOTA && activity_key == QUOTA_LIVE_ACTIVITY_KEY {
        spawn_quota_snapshot_refresh_once(
            state.app_handle.clone(),
            state.tx.clone(),
            "quota_live_activity_register",
        );
    }

    Json(serde_json::json!({
        "success": true,
        "message": "Live Activity token registered successfully",
        "activity_kind": activity_kind,
        "activity_key": activity_key,
        "goal_id": goal_id,
        "tokens": token_count
    }))
    .into_response()
}

async fn handle_apns_live_activity_update(
    headers: HeaderMap,
    Json(request): Json<ApnsLiveActivityUpdateRequest>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_NOTIFICATION_SEND,
        "missing_scope_notification_send",
    )
    .await
    {
        return response;
    }

    if normalized_live_activity_kind(request.activity_kind.as_deref())
        == LIVE_ACTIVITY_KIND_LIVE_GOAL
    {
        persist_live_goal_progress_from_apns_update(&request);
    }
    let stats = send_apns_live_activity_update_inner(request).await;
    let status = if stats.success {
        StatusCode::OK
    } else if stats.matched == 0 {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::BAD_GATEWAY
    };

    (
        status,
        Json(serde_json::to_value(stats).unwrap_or_default()),
    )
        .into_response()
}

fn persist_live_goal_progress_from_apns_update(request: &ApnsLiveActivityUpdateRequest) {
    let Some(goal_id) = request
        .goal_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return;
    };

    let event = normalized_live_activity_event(request.event.as_deref());
    let completed = event == "end";
    let update = crate::ui::live_goal::LiveGoalProgressUpdate {
        progress_percent: request
            .progress_percent
            .or_else(|| completed.then_some(100.0)),
        progress_source: Some("apns_live_activity_update".to_string()),
        progress_label: request
            .progress_label
            .clone()
            .or_else(|| completed.then(|| "100%".to_string())),
        phase: request
            .phase
            .clone()
            .or_else(|| completed.then(|| "completed".to_string())),
        status: request
            .status
            .clone()
            .or_else(|| completed.then(|| "completed".to_string())),
        status_text: request
            .status_text
            .clone()
            .or_else(|| completed.then(|| "已完成".to_string())),
        project_path: request.project_path.clone(),
        request_id: request.request_id.clone(),
        source: Some("apns_live_activity_update".to_string()),
        ..Default::default()
    };

    match crate::ui::live_goal::update_live_goal_progress_persistent_only(Some(goal_id), update) {
        Ok(Some(goal)) => {
            bridge_debug_log(&format!(
                "[APNs LiveActivity] synced live goal source: goal_id={}, progress={:?}, label={:?}",
                goal.id, goal.progress_percent, goal.progress_label
            ));
        }
        Ok(None) => {
            bridge_debug_log(&format!(
                "[APNs LiveActivity] skipped live goal source sync: no current goal matched goal_id={}",
                goal_id
            ));
        }
        Err(err) => {
            log_important!(
                warn,
                "[APNs LiveActivity] 持久化 Live Goal 进度失败: goal_id={}, error={}",
                goal_id,
                err
            );
        }
    }
}

async fn cache_early_mcp_state(payload: serde_json::Value, source: &str) {
    let request_id = extract_request_id_from_mcp_state(&payload);
    let project_path = extract_project_path_from_mcp_state(&payload);

    record_last_active_route(request_id.as_deref(), project_path.as_deref()).await;

    let mut cache = MCP_STATE_CACHE.write().await;
    let mut touched_at = MCP_STATE_CACHE_TOUCHED_AT.write().await;
    prune_json_cache(
        "mcp_state",
        &mut cache,
        &mut touched_at,
        MCP_STATE_CACHE_TTL_SECS,
        MCP_STATE_CACHE_MAX_ENTRIES,
    );

    let mut cache_keys = Vec::<String>::new();
    if let Some(rid) = &request_id {
        cache.insert(rid.clone(), payload.clone());
        cache_keys.push(rid.clone());
    }
    if let Some(path) = &project_path {
        cache.insert(path.clone(), payload.clone());
        cache_keys.push(path.clone());
    }
    mark_json_cache_keys(&mut touched_at, &cache_keys);
    record_cache_write_count("mcp_state", cache_keys.len());
    drop(cache);
    drop(touched_at);

    let mut active_registry = ACTIVE_SESSION_REGISTRY.write().await;
    update_active_session_registry(&mut active_registry, &payload);

    bridge_debug_log(&format!(
        "[Bridge Timing] early mcp_state cached: request_id={:?}, project_path={:?}, cache_keys={}, source={}",
        request_id,
        project_path,
        cache_keys.len(),
        source
    ));
}

async fn handle_apns_notify(
    headers: HeaderMap,
    Json(request): Json<ApnsNotifyRequest>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_NOTIFICATION_SEND,
        "missing_scope_notification_send",
    )
    .await
    {
        return response;
    }

    let source = request
        .source
        .clone()
        .unwrap_or_else(|| "early_request".to_string());
    let body = request.body.trim().to_string();
    if body.is_empty() {
        return Json(serde_json::json!({
            "ok": false,
            "error": "empty body",
        }))
        .into_response();
    }

    let request_id = request.request_id.clone();
    let project_path = request.project_path.clone();
    if request_id_is_stale_for_bridge_project_binding(
        request_id.as_deref(),
        project_path.as_deref(),
    ) {
        log::info!(
            "[Bridge] stale APNs notify dropped: request_id={:?}, project_path={:?}, source={}",
            request_id,
            project_path,
            source
        );
        bridge_debug_log(&format!(
            "[Bridge Route] stale APNs notify dropped: request_id={:?}, project_path={:?}, source={}",
            request_id, project_path, source
        ));
        return stale_request_response();
    }
    if source == "desktop_popup_ready" {
        record_active_desktop_popup_route(request_id.as_deref(), project_path.as_deref(), &source)
            .await;
    }
    if request.request_id.is_some() || request.project_path.is_some() {
        let payload = serde_json::json!({
            "request": {
                "id": request_id,
                "message": body.clone(),
                "predefined_options": request.predefined_options,
                "is_markdown": request.is_markdown,
                "project_path": project_path,
                "codex_thread_id": request.codex_thread_id,
                "codex_deeplink": request.codex_deeplink,
                "loop_active": request.loop_active,
                "force_popup": request.force_popup,
            },
            "showMcpPopup": true,
            "timelineNodes": [],
            "earlyNotification": true,
        });
        cache_early_mcp_state(payload, &source).await;
    }

    let title = request.title.unwrap_or_else(|| "iterate".to_string());
    send_apns_notification_once(&title, &body, project_path, request_id, &source).await;

    Json(serde_json::json!({
        "ok": true,
        "source": source,
    }))
    .into_response()
}

async fn handle_bridge_publish(
    headers: HeaderMap,
    State(state): State<BridgeHttpState>,
    Json(mut message): Json<BridgeMessage>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_BRIDGE_PUBLISH,
        "missing_scope_bridge_publish",
    )
    .await
    {
        return response;
    }

    let tx = state.tx.clone();
    let publish_received_at = std::time::Instant::now();
    let publish_request_id = if message.message_type == "mcp_state" {
        extract_request_id_from_mcp_state(&message.payload)
    } else {
        None
    };
    let publish_project_path = if message.message_type == "mcp_state" {
        extract_project_path_from_mcp_state(&message.payload)
    } else {
        None
    };
    bridge_debug_log(&format!(
        "[Bridge Timing] publish received: type={}, request_id={:?}, project_path={:?}, subscribers={}",
        message.message_type,
        publish_request_id,
        publish_project_path,
        tx.receiver_count()
    ));

    if message.message_type == "mcp_state"
        && request_id_is_stale_for_bridge_project_binding(
            publish_request_id.as_deref(),
            publish_project_path.as_deref(),
        )
    {
        log::info!(
            "[Bridge] stale mcp_state publish dropped: request_id={:?}, project_path={:?}",
            publish_request_id,
            publish_project_path
        );
        bridge_debug_log(&format!(
            "[Bridge Route] stale mcp_state publish dropped: request_id={:?}, project_path={:?}",
            publish_request_id, publish_project_path
        ));
        return stale_request_response();
    }

    if message.message_type == "mcp_action" && has_room_submit_metadata(&message.payload) {
        let action = message
            .payload
            .get("action")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let Some(raw_project_path) =
            payload_string_field(&message.payload, &["project_path", "projectPath"])
        else {
            let outcome = room_submit_outcome(
                None,
                action,
                "",
                None,
                "rejected",
                Some("missing_project_path"),
                false,
            );
            broadcast_room_submit_outcome(&tx, &outcome);
            return (StatusCode::BAD_REQUEST, Json(outcome)).into_response();
        };
        let project_path = normalize_bridge_project_path(&raw_project_path);
        let request_id = payload_string_field(&message.payload, &["request_id", "requestId"]);
        if request_id_is_stale_for_current_window_binding(
            request_id.as_deref(),
            Some(&project_path),
        ) {
            log::info!(
                "[Bridge] stale room mcp_action dropped: request_id={:?}, project_path={}",
                request_id,
                project_path
            );
            bridge_debug_log(&format!(
                "[Bridge Route] stale room mcp_action dropped: project_path={}, request_id={:?}",
                project_path, request_id
            ));
            let outcome = room_submit_outcome(
                None,
                action,
                &project_path,
                request_id.as_deref(),
                "rejected",
                Some("stale_request"),
                false,
            );
            broadcast_room_submit_outcome(&tx, &outcome);
            return (StatusCode::BAD_REQUEST, Json(outcome)).into_response();
        }
        let fallback_route = last_active_route().await;
        let timeline_route_resolution = {
            let registry = ACTIVE_SESSION_REGISTRY.read().await;
            resolve_mcp_action_timeline_route_id(
                &message.payload,
                request_id.as_deref(),
                Some(&project_path),
                fallback_route.as_deref(),
                &registry,
            )
        };
        let timeline_route_id = timeline_route_resolution
            .as_ref()
            .map(|resolution| resolution.route_id.clone());
        if let Some(resolution) = &timeline_route_resolution {
            bridge_debug_log(&format!(
                "[Bridge Route] room mcp_action timeline route resolved: source={}, project_path={}, request_id={:?}, timeline_route_id={}, fallback_route={:?}",
                resolution.source, project_path, request_id, resolution.route_id, fallback_route
            ));
        } else {
            bridge_debug_log(&format!(
                "[Bridge Route] room mcp_action timeline route unresolved: project_path={}, request_id={:?}, fallback_route={:?}",
                project_path, request_id, fallback_route
            ));
        }
        let outcome = handle_room_submit_action(
            state.app_handle.as_ref(),
            &project_path,
            request_id.as_deref(),
            timeline_route_id.as_deref(),
            &message.payload,
        )
        .await;
        broadcast_room_submit_outcome(&tx, &outcome);
        let status = if outcome.ok {
            StatusCode::OK
        } else {
            StatusCode::BAD_REQUEST
        };
        return (status, Json(outcome)).into_response();
    }

    if message.message_type == "mcp_state" {
        // 服务端注入 timelineNodes：从 ConversationManager 查找当前对话路径
        let has_frontend_timeline = message
            .payload
            .get("timelineNodes")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);

        // P-2026-118: 无论 frontend 是否提供了 timelineNodes，都要清洗 heavy metadata；
        // P-2026-1879: frontend 传来的历史路径还要按当前 route 过滤，避免旧混线数据继续扩散。
        if has_frontend_timeline {
            TimelineSyncService::sanitize_payload_timeline_nodes(&mut message.payload);
        }

        if !has_frontend_timeline {
            if let Some(manager) = state
                .app_handle
                .as_ref()
                .and_then(|app_handle| app_handle.try_state::<Arc<ConversationManager>>())
            {
                let request_id = extract_request_id_from_mcp_state(&message.payload);
                let timeline_route_id = extract_timeline_route_id_from_mcp_state(&message.payload)
                    .or_else(|| request_id.clone());
                let project_path = extract_project_path_from_mcp_state(&message.payload);

                // 诊断日志：记录所有 mcp_state 推送
                if let Ok(mut f) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open("/tmp/bridge_timeline_debug.log")
                {
                    use std::io::Write;
                    let _ = writeln!(
                        f,
                        "{}: mcp_state received: request_id={:?}, timeline_route_id={:?}, project_path={:?}, has_frontend_timeline={}",
                        chrono::Local::now().format("%H:%M:%S"),
                        request_id,
                        timeline_route_id,
                        project_path,
                        has_frontend_timeline
                    );
                }

                if let (Some(route_id), Some(pp)) = (timeline_route_id, project_path) {
                    // 查找或创建对话树：timeline 使用稳定会话 key，响应路由仍使用真实 request_id。
                    let tree_id = manager
                        .create_tree_for_route(Some(route_id.clone()), Some(pp.clone()))
                        .await;
                    let current_node = manager.get_current_node_id(&tree_id).await;

                    // 诊断日志
                    if let Ok(mut f) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open("/tmp/bridge_timeline_debug.log")
                    {
                        use std::io::Write;
                        let _ = writeln!(
                            f,
                            "{}: tree_id={}, current_node={:?}, project={}",
                            chrono::Local::now().format("%H:%M:%S"),
                            tree_id,
                            current_node,
                            pp
                        );
                    }

                    // 如果树为空（app 重启后），自动创建 bootstrap assistant 节点
                    let effective_node_id = if current_node.is_some() {
                        current_node
                    } else {
                        let msg_content = message
                            .payload
                            .pointer("/request/message")
                            .and_then(|v| v.as_str())
                            .unwrap_or("[对话继续]")
                            .to_string();
                        let metadata = NodeMetadata {
                            conversation_id: Some(tree_id.clone()),
                            project_path: Some(pp.clone()),
                            predefined_options: None,
                            selected_option: None,
                            images: None,
                            link_url: None,
                            link_title: None,
                            request_id: Some(route_id.clone()),
                            run_id: None,
                            generation: None,
                            stale_of: None,
                            superseded_by: None,
                            checkpoint_id: message
                                .payload
                                .pointer("/request/checkpoint_id")
                                .and_then(|v| v.as_str())
                                .map(ToOwned::to_owned),
                            checkpoint_commit: message
                                .payload
                                .pointer("/request/checkpoint_commit")
                                .and_then(|v| v.as_str())
                                .map(ToOwned::to_owned),
                            checkpoint_message: message
                                .payload
                                .pointer("/request/checkpoint_message")
                                .and_then(|v| v.as_str())
                                .map(ToOwned::to_owned),
                            source: Some("server_injection_bootstrap".to_string()),
                        };
                        match manager
                            .add_node(
                                &tree_id,
                                None,
                                NodeType::Assistant,
                                msg_content,
                                true,
                                metadata,
                            )
                            .await
                        {
                            Ok(node_id) => {
                                if let Ok(mut f) = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open("/tmp/bridge_timeline_debug.log")
                                {
                                    use std::io::Write;
                                    let _ = writeln!(
                                        f,
                                        "{}: auto-created bootstrap node={} tree={} project={}",
                                        chrono::Local::now().format("%H:%M:%S"),
                                        node_id,
                                        tree_id,
                                        pp
                                    );
                                }
                                Some(node_id)
                            }
                            Err(e) => {
                                log::warn!("[Bridge] 自动创建 bootstrap 节点失败: {}", e);
                                None
                            }
                        }
                    };

                    if let Some(node_id) = effective_node_id {
                        match manager.get_node_path(&tree_id, &node_id).await {
                            Ok(nodes) => {
                                let lightweight_nodes = TimelineSyncService::strip_and_filter_nodes(
                                    &nodes,
                                    &tree_id,
                                    Some(&route_id),
                                    Some(&pp),
                                );
                                if let Ok(serialized) = serde_json::to_value(&lightweight_nodes) {
                                    if let Some(obj) = message.payload.as_object_mut() {
                                        obj.insert("timelineNodes".to_string(), serialized);
                                        obj.insert(
                                            "timeline_route_id".to_string(),
                                            serde_json::json!(route_id),
                                        );
                                        obj.insert(
                                            "conversation_id".to_string(),
                                            serde_json::json!(tree_id),
                                        );
                                    }
                                }
                                // 调试日志
                                if let Ok(mut f) = std::fs::OpenOptions::new()
                                    .create(true)
                                    .append(true)
                                    .open("/tmp/bridge_timeline_debug.log")
                                {
                                    use std::io::Write;
                                    let _ = writeln!(
                                        f,
                                        "{}: server-injected timelineNodes={} project={}",
                                        chrono::Local::now().format("%H:%M:%S"),
                                        nodes.len(),
                                        pp
                                    );
                                }
                            }
                            Err(e) => {
                                log::warn!("[Bridge] 服务端注入时间线失败: {}", e);
                            }
                        }
                    }
                }
            }
        }

        ensure_custom_prompts_in_mcp_state(state.app_handle.as_ref(), &mut message.payload);
        ensure_ghost_suggestions_in_mcp_state(&mut message.payload);
        let project_path_for_live_goal = extract_project_path_from_mcp_state(&message.payload);
        crate::ui::live_goal::ensure_live_goal_in_mcp_state(
            state.app_handle.as_ref(),
            &mut message.payload,
            project_path_for_live_goal.as_deref(),
        );
        let codex_home_for_quota =
            crate::ui::quota_snapshot::codex_home_from_mcp_state(&message.payload);
        inject_cached_quota_snapshot_and_refresh_async(
            state.app_handle.as_ref(),
            &state.tx,
            &mut message.payload,
            codex_home_for_quota,
            "quota_bridge_publish",
        );
        register_markdown_images_for_mcp_state_payload(&mut message.payload);

        let request_id = extract_request_id_from_mcp_state(&message.payload);
        let project_path = extract_project_path_from_mcp_state(&message.payload);
        record_last_active_route(request_id.as_deref(), project_path.as_deref()).await;
        let mut cache = MCP_STATE_CACHE.write().await;
        let mut touched_at = MCP_STATE_CACHE_TOUCHED_AT.write().await;
        prune_json_cache(
            "mcp_state",
            &mut cache,
            &mut touched_at,
            MCP_STATE_CACHE_TTL_SECS,
            MCP_STATE_CACHE_MAX_ENTRIES,
        );
        // 以 request_id 为主 key（支持同项目多对话），project_path 为辅助 key（兼容旧客户端）
        let mut cache_keys = Vec::<String>::new();
        if let Some(rid) = &request_id {
            cache.insert(rid.clone(), message.payload.clone());
            cache_keys.push(rid.clone());
        }
        if let Some(path) = &project_path {
            cache.insert(path.clone(), message.payload.clone());
            cache_keys.push(path.clone());
        }
        mark_json_cache_keys(&mut touched_at, &cache_keys);
        record_cache_write_count("mcp_state", cache_keys.len());
        drop(cache);
        drop(touched_at);

        let mut active_registry = ACTIVE_SESSION_REGISTRY.write().await;
        update_active_session_registry(&mut active_registry, &message.payload);
    }
    let web_push_message = if message.message_type == "mcp_state"
        && !bridge_payload_suppresses_remote_notification(&message.payload)
    {
        Some(message.clone())
    } else {
        None
    };
    if message.message_type == "mcp_state" {
        bridge_debug_log(&format!(
            "[Bridge Timing] APNs skipped: request_id={:?}, project_path={:?}, reason=mcp_state_sync_only",
            publish_request_id, publish_project_path
        ));
    }

    // 广播消息到所有 WebSocket 客户端
    let subscriber_count = tx.receiver_count();
    let msg_type = message.message_type.clone();
    match tx.send(message) {
        Ok(sent_count) => {
            log::info!(
                "[Bridge] 广播成功: {} 个订阅者收到消息 (总订阅者: {})",
                sent_count,
                subscriber_count
            );
            bridge_debug_log(&format!(
                "[Bridge Timing] broadcast success: type={}, request_id={:?}, project_path={:?}, sent={}, subscribers={}, elapsed_ms={}",
                msg_type,
                publish_request_id,
                publish_project_path,
                sent_count,
                subscriber_count,
                publish_received_at.elapsed().as_millis()
            ));
        }
        Err(e) => {
            log::warn!(
                "[Bridge] 广播失败 (无订阅者): {:?}, 订阅者数量: {}",
                e,
                subscriber_count
            );
            bridge_debug_log(&format!(
                "[Bridge Timing] broadcast failed: type={}, request_id={:?}, project_path={:?}, subscribers={}, elapsed_ms={}, err={:?}",
                msg_type,
                publish_request_id,
                publish_project_path,
                subscriber_count,
                publish_received_at.elapsed().as_millis(),
                e
            ));
        }
    }

    if let Some(web_push_message) = web_push_message {
        tokio::spawn(async move {
            send_web_push_for_bridge_message(web_push_message).await;
        });
    }

    StatusCode::NO_CONTENT.into_response()
}

/// Narrow loopback transport for codex-room. Every request must carry a
/// one-shot capability minted through the signed iterate broker and bound to
/// the exact body bytes. Room state still validates room/request/workspace and
/// target metadata, but its same-user-readable token is no longer the HTTP
/// authentication boundary.
async fn handle_local_room_submit(
    ConnectInfo(remote_addr): ConnectInfo<SocketAddr>,
    State(state): State<BridgeHttpState>,
    mut headers: HeaderMap,
    body: Bytes,
) -> Response {
    if !remote_addr.ip().is_loopback() || is_public_bridge_request(&headers) {
        return json_error_response(StatusCode::FORBIDDEN, "local_room_submit_only");
    }
    let body_sha256 = crate::bridge::auth::bridge_body_sha256(&body);
    match crate::bridge::auth::authenticate_internal_room_submit_bearer(&headers, &body_sha256) {
        Ok(Some(crate::bridge::auth::BridgeTokenAudience::InternalProcess)) => {}
        Ok(_) | Err(_) => {
            return json_error_response(StatusCode::UNAUTHORIZED, "invalid_room_submit_capability");
        }
    }
    let Ok(message) = serde_json::from_slice::<BridgeMessage>(&body) else {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_room_submit_json");
    };
    if message.message_type != "mcp_action" || !has_room_submit_metadata(&message.payload) {
        return json_error_response(StatusCode::BAD_REQUEST, "invalid_room_submit");
    }
    headers.remove(header::AUTHORIZATION);
    headers.insert(
        HeaderName::from_static(TRUSTED_INTERNAL_CAPABILITY_HEADER),
        HeaderValue::from_static("1"),
    );
    handle_bridge_publish(headers, State(state), Json(message)).await
}

async fn maybe_attach_phone_action_job(
    action_id: &str,
    message: &mut BridgeMessage,
) -> Result<(), String> {
    let Some(payload) = phone_action_job_payload_from_message(message) else {
        return Ok(());
    };
    let payload_size_bytes = phone_action_job_payload_size(&payload)?;
    if payload_size_bytes <= PHONE_ACTION_INLINE_PAYLOAD_MAX_BYTES {
        return Ok(());
    }
    if payload_size_bytes > PHONE_ACTION_JOB_PAYLOAD_MAX_BYTES {
        return Err(format!(
            "phone action payload is too large: {} bytes (max {} bytes)",
            payload_size_bytes, PHONE_ACTION_JOB_PAYLOAD_MAX_BYTES
        ));
    }

    let action = message
        .payload
        .get("action")
        .and_then(|value| value.as_str())
        .unwrap_or("unknown")
        .to_string();
    let now = chrono::Utc::now();
    let expires_at = now + chrono::Duration::seconds(PHONE_ACTION_JOB_TTL_SECS);
    let job = PhoneActionJobEntry {
        id: uuid::Uuid::new_v4().to_string(),
        action_id: action_id.to_string(),
        action,
        payload,
        payload_size_bytes,
        created_at: now.to_rfc3339(),
        expires_at: expires_at.to_rfc3339(),
    };

    {
        let mut jobs = PHONE_ACTION_JOBS.write().await;
        prune_phone_action_jobs(&mut jobs, now);
        jobs.insert(job.id.clone(), job.clone());
        prune_phone_action_jobs(&mut jobs, now);
    }

    log::info!(
        "[Bridge] phone_action job created action_id={} job_id={} bytes={} expires_at={}",
        action_id,
        job.id,
        job.payload_size_bytes,
        job.expires_at
    );
    attach_phone_action_job_metadata(message, &job);
    Ok(())
}

async fn publish_phone_action_request(
    tx: &broadcast::Sender<BridgeMessage>,
    request: PhoneActionRequest,
    default_source: &str,
) -> Result<PhoneActionPublishResponse, String> {
    let (id, mut message) = build_phone_action_bridge_message(request, default_source)?;
    let subscribers = tx.receiver_count();
    let target_device_id = phone_action_target_device_id(&message);
    let delivery_client_ids = phone_action_delivery_client_ids(target_device_id.as_deref()).await;
    let delivery_client_count = delivery_client_ids.len();

    if delivery_client_count == 0 {
        return Ok(PhoneActionPublishResponse {
            ok: false,
            id,
            sent: 0,
            subscribers,
        });
    }

    maybe_attach_phone_action_job(&id, &mut message).await?;

    let sent = match tx.send(message) {
        Ok(sent) => delivery_client_count.min(sent),
        Err(err) => {
            log::warn!(
                "[Bridge] phone_action_request broadcast had no subscribers: {}",
                err
            );
            0
        }
    };

    Ok(PhoneActionPublishResponse {
        ok: sent > 0,
        id,
        sent,
        subscribers,
    })
}

async fn handle_api_phone_action(
    headers: HeaderMap,
    State(state): State<BridgeHttpState>,
    Json(request): Json<PhoneActionRequest>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_BRIDGE_PUBLISH,
        "missing_scope_bridge_publish",
    )
    .await
    {
        return response;
    }

    match publish_phone_action_request(&state.tx, request, "desktop_http").await {
        Ok(response) => Json(response).into_response(),
        Err(error) => json_error_response(StatusCode::BAD_REQUEST, &error),
    }
}

async fn handle_api_phone_action_result(
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<PhoneActionResultQuery>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_BRIDGE_PUBLISH,
        "missing_scope_bridge_publish",
    )
    .await
    {
        return response;
    }

    let id = query.id.trim();
    if id.is_empty() {
        return json_error_response(StatusCode::BAD_REQUEST, "phone action id is required");
    }

    let result = {
        let mut results = PHONE_ACTION_RESULTS.write().await;
        prune_phone_action_results(&mut results, chrono::Utc::now());
        results.get(id).cloned()
    };
    Json(PhoneActionResultResponse {
        ok: result.is_some(),
        result,
    })
    .into_response()
}

async fn handle_api_phone_action_job(headers: HeaderMap, Path(job_id): Path<String>) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_PHONE_ACTION_JOB_READ,
        "missing_scope_phone_action_job_read",
    )
    .await
    {
        return response;
    }

    let job_id = job_id.trim();
    if job_id.is_empty() {
        return json_error_response(StatusCode::BAD_REQUEST, "phone action job id is required");
    }

    let now = chrono::Utc::now();
    let job = {
        let mut jobs = PHONE_ACTION_JOBS.write().await;
        prune_phone_action_jobs(&mut jobs, now);
        jobs.get(job_id).cloned()
    };

    let Some(job) = job else {
        return json_error_response(StatusCode::NOT_FOUND, "phone action job not found");
    };
    if phone_action_job_is_expired(&job, now) {
        let mut jobs = PHONE_ACTION_JOBS.write().await;
        jobs.remove(job_id);
        return json_error_response(StatusCode::NOT_FOUND, "phone action job expired");
    }

    Json(PhoneActionJobResponse { ok: true, job }).into_response()
}

#[derive(Debug, Deserialize)]
struct PullActionQuery {
    project_path: String,
    request_id: Option<String>,
}

async fn handle_bridge_pull_action(
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<PullActionQuery>,
) -> Response {
    if let Err(response) = authorize_public_route_scope(
        &headers,
        SCOPE_SESSION_RESPOND,
        "missing_scope_session_respond",
    )
    .await
    {
        return response;
    }

    // 先获取并释放 MCP_ACTION_CACHE 锁
    let action = {
        let mut cache = MCP_ACTION_CACHE.write().await;
        let mut touched_at = MCP_ACTION_CACHE_TOUCHED_AT.write().await;
        let has_request_id = normalize_route_part(query.request_id.as_deref()).is_some();
        let window_instances = if has_request_id {
            let mut window_registry = crate::ui::window_registry::WindowRegistry::load();
            window_registry.get_all_instances()
        } else {
            Vec::new()
        };
        prune_json_cache(
            "mcp_action",
            &mut cache,
            &mut touched_at,
            MCP_ACTION_CACHE_TTL_SECS,
            MCP_ACTION_CACHE_MAX_ENTRIES,
        );
        let cache_key = action_cache_key_for_pull(&query.project_path, query.request_id.as_deref());
        let lookup_route = if has_request_id {
            CacheLookupRoute::RequestId
        } else {
            CacheLookupRoute::ProjectPath
        };
        let action = take_cached_action_for_pull_with_window_bindings(
            &mut cache,
            &query.project_path,
            query.request_id.as_deref(),
            &window_instances,
        );
        MCP_ACTION_CACHE_METRICS.record_lookup(lookup_route, action.is_some());
        if action.is_some() {
            if let Some(key) = cache_key {
                touched_at.remove(&key);
            }
        } else if let Some(key) = cache_key {
            if request_id_is_stale_for_live_window_instances(
                &window_instances,
                query.request_id.as_deref(),
                Some(&query.project_path),
            ) {
                touched_at.remove(&key);
                log::info!(
                    "[Bridge] stale mcp_action dropped: request_id={:?}, project_path={}",
                    query.request_id,
                    query.project_path
                );
            }
        }
        action
    };
    // MCP_ACTION_CACHE 锁已释放。对话已结束（action 被消费）后，
    // 清除 MCP_STATE_CACHE 与 active-session 中对应条目。
    if action.is_some() {
        if let Some(ref rid) = query.request_id {
            cleanup_completed_session_by_request_id(rid, "pull-action-cleanup").await;
        }
    }
    Json(serde_json::json!({ "action": action })).into_response()
}

#[derive(Debug, Deserialize)]
struct ImageQuery {
    path: Option<String>,
    id: Option<String>,
}

async fn image_path_read_denial(headers: &HeaderMap) -> Option<Response> {
    if !public_route_requires_auth(headers) {
        return None;
    }
    match authenticate_bridge_headers_result(headers).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Some(json_error_response(
                StatusCode::UNAUTHORIZED,
                "mobile_auth_required",
            ));
        }
        Err(error) => return Some(bridge_auth_error_response(&error)),
    }

    Some(json_error_response(
        StatusCode::FORBIDDEN,
        "image_path_not_allowed",
    ))
}

/// HTTP 端点：读取本地图片并返回压缩后的图片（让手机端通过 HTTP 下载而非 WebSocket 传输）
async fn handle_serve_image(
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<ImageQuery>,
) -> Response {
    if let Some(image_id) = query
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if public_route_requires_auth(&headers) {
            match authenticate_bridge_headers_result(&headers).await {
                Ok(Some(_)) => {}
                Ok(None) => {
                    return json_error_response(StatusCode::UNAUTHORIZED, "mobile_auth_required");
                }
                Err(error) => return bridge_auth_error_response(&error),
            }
        }
        let Some(file_path) = registered_markdown_image_path(image_id) else {
            return json_error_response(StatusCode::NOT_FOUND, "image_id_not_found");
        };
        return serve_image_file(&file_path);
    }

    let Some(path) = query.path.as_deref() else {
        return json_error_response(StatusCode::BAD_REQUEST, "image_path_or_id_required");
    };

    if let Some(response) = image_path_read_denial(&headers).await {
        return response;
    }

    let file_path = std::path::Path::new(path);
    serve_image_file(file_path)
}

fn serve_image_file(file_path: &std::path::Path) -> Response {
    if !file_path.exists() {
        return (
            [
                ("content-type".to_string(), "text/plain".to_string()),
                ("cache-control".to_string(), "no-cache".to_string()),
            ],
            Bytes::from("File not found"),
        )
            .into_response();
    }

    let metadata = std::fs::metadata(file_path).ok();
    let file_size = metadata.map(|m| m.len()).unwrap_or(0);
    let original_content_type = image_content_type_for_path(file_path);

    // 大于 150KB 的非 GIF 图片用 sips 压缩；GIF 保留原始 bytes 才能保持动画。
    let (data, content_type) = if file_size > 150_000 && original_content_type != "image/gif" {
        let temp = std::env::temp_dir().join(format!("iterate_img_{}.jpg", std::process::id()));
        let _ = std::process::Command::new("sips")
            .args([
                "-Z",
                "1600",
                "-s",
                "format",
                "jpeg",
                "-s",
                "formatOptions",
                "80",
            ])
            .arg(file_path)
            .arg("--out")
            .arg(&temp)
            .output();
        if temp.exists() {
            let d = std::fs::read(&temp).unwrap_or_default();
            let _ = std::fs::remove_file(&temp);
            (d, "image/jpeg".to_string())
        } else {
            (
                std::fs::read(file_path).unwrap_or_default(),
                original_content_type.to_string(),
            )
        }
    } else {
        (
            std::fs::read(file_path).unwrap_or_default(),
            original_content_type.to_string(),
        )
    };

    (
        [
            ("content-type".to_string(), content_type),
            (
                "cache-control".to_string(),
                "public, max-age=3600".to_string(),
            ),
        ],
        Bytes::from(data),
    )
        .into_response()
}

fn image_content_type_for_path(file_path: &std::path::Path) -> &'static str {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "image/png",
    }
}

#[tauri::command]
pub async fn send_to_web_bridge(message: BridgeMessage) -> Result<(), String> {
    // 只通过 HTTP POST 转发到 Bridge Server（避免重复推送）
    // handle_bridge_publish 会负责缓存写入和广播到所有 WebSocket 客户端
    tauri::async_runtime::spawn(async move {
        let client = reqwest::Client::builder()
            .no_proxy() // 绕过代理
            .build()
            .unwrap_or_default();
        let url = "http://127.0.0.1:8080/bridge/publish";
        let request = client
            .post(url)
            .json(&message)
            .timeout(std::time::Duration::from_secs(2));
        let result =
            match crate::bridge::auth::authorize_internal_bridge_request(request, "POST", url) {
                Ok(request) => request
                    .send()
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error),
            };
        if let Err(e) = result {
            log::debug!("[Bridge] HTTP publish failed: {}", e);
        }
    });

    Ok(())
}

#[tauri::command]
pub async fn send_phone_action_request(
    request: PhoneActionRequest,
) -> Result<PhoneActionPublishResponse, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|error| format!("创建 HTTP 客户端失败: {}", error))?;
    let url = "http://127.0.0.1:8080/api/phone-action";
    let request = client
        .post(url)
        .json(&request)
        .timeout(std::time::Duration::from_secs(2));
    let request = crate::bridge::auth::authorize_internal_bridge_request(request, "POST", url)
        .map_err(|error| format!("签发 phone_action_request 凭据失败: {}", error))?;
    let response = request
        .send()
        .await
        .map_err(|error| format!("发送 phone_action_request 失败: {}", error))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "phone_action_request 被 Bridge 拒绝: status={} body={}",
            status, body
        ));
    }

    response
        .json::<PhoneActionPublishResponse>()
        .await
        .map_err(|error| format!("解析 phone_action_request 响应失败: {}", error))
}

#[cfg(test)]
mod tests {
    use super::super::mcp_action_delivery::serve_response_route_file;
    use super::{
        apns_dedupe_ttl_secs, attach_phone_action_job_metadata, body_bound_auth_deferred,
        bounded_file_list_depth, bridge_auth_required_for_request, bridge_cors_origin_strings,
        bridge_message_targets_device, bridge_token_hash, bridge_token_hash_matches,
        browser_http_origin_is_allowed, browser_websocket_origin_is_allowed,
        build_active_session_summaries, build_active_session_summaries_with_focus,
        build_goal_payload_parts, build_goal_submit_prompt, build_phone_action_bridge_message,
        build_redacted_connection_status_value, build_redacted_pairing_status_value,
        build_route_debug_snapshot, cached_room_submit_outcome,
        canonical_path_is_within_allowed_roots, cleanup_completed_session_by_request_id,
        cleanup_stale_request_sync_route, clear_room_submit_outcome_cache_for_tests,
        direct_network_bridge_auth_denial, direct_network_peer_requires_auth,
        effective_request_sync_request_id, ensure_custom_prompts_value_in_mcp_state,
        expire_stale_desktop_codex_live_host, explicit_file_list_roots_for_principal_at,
        extract_conversation_id_from_mcp_state, extract_project_path_from_mcp_state,
        extract_request_id_from_mcp_state, extract_timeline_route_id_from_mcp_state,
        fallback_pairing_candidates_with_public_base_url, get_public_probe_snapshot,
        ghost_suggestions_write_scope_denial, handle_api_active_sessions, handle_api_audio_assets,
        handle_api_cleanup_session, handle_api_config_get, handle_api_config_post,
        handle_api_ghost_suggestions_get, handle_api_import_prompts_dir, handle_api_mcp_tools,
        handle_api_mobile_pairing, handle_api_open_codex_chat, handle_api_prompt_library_delete,
        handle_api_prompt_library_get, handle_api_prompt_library_post,
        handle_api_promptor_library_get, handle_api_restart_service, handle_api_restart_tunnel,
        handle_api_show_window, handle_api_speech_correction_memory_get,
        handle_api_speech_correction_memory_post, handle_api_speech_muscle_memory_get,
        handle_api_speech_muscle_memory_post, handle_api_test_audio, handle_apns_notify,
        handle_apns_register, handle_bridge_publish, handle_bridge_pull_action,
        handle_desktop_codex_live_control, handle_desktop_codex_live_lease,
        handle_desktop_codex_live_status, handle_get_files, handle_get_windows, handle_pair_claim,
        handle_push_subscribe, handle_push_unsubscribe, handle_room_submit_action,
        handle_serve_image, handle_session_refresh, handle_session_revoke, image_path_read_denial,
        is_inactive_session_message, is_public_control_path, is_tailscale_ipv4, is_valid_ipv4,
        issue_cloudflare_web_login_pairing, list_web_login_sessions,
        live_activity_content_state_from_update, live_activity_info_matches, load_apns_config,
        lookup_active_session_entry, lookup_active_session_payload, mobile_auth_required,
        mobile_device_scopes, mobile_pairing_candidate_has_endpoint_proof,
        mobile_pairing_candidate_is_ready_relay, mobile_pairing_candidate_is_secure_for_qr,
        mobile_pairing_primary_selection_reason, normalize_file_browser_roots,
        normalize_mcp_action_images, normalize_paired_device_store, parse_first_ipv4_line,
        parse_first_tailscale_ipv4_from_ifconfig, parse_rfc3339, phone_action_delivery_client_ids,
        phone_action_job_payload_from_message, phone_action_job_payload_size,
        phone_action_result_entry_from_message, phone_action_target_device_id,
        principal_has_any_scope, prune_active_session_registry, prune_json_cache,
        prune_phone_action_results, public_anonymous_path_allowed,
        quota_live_activity_content_state_from_snapshot,
        quota_live_activity_fingerprint_send_succeeded, record_active_desktop_popup_route,
        recovery_transport_from_headers, redact_bridge_message_text, registered_mcp_ports_from_dir,
        remember_room_submit_outcome, remote_action_denial_reason, render_goal_submit_prompt,
        replace_paired_device_record, request_id_is_stale_for_live_window_instances,
        reset_active_desktop_popup_route_for_tests, resolve_mcp_action_timeline_route_id,
        revoke_all_web_login_sessions, room_submit_outcome, root_tunnel_is_authoritative_up,
        root_tunnel_supervisor_fields_from_status, route_debug_status_value,
        sanitize_recovery_transport, scoped_public_route_denial,
        select_mobile_pairing_primary_candidate, serve_image_file, speech_memory_auth_denial,
        status_read_full_diagnostics_denial, tailscale_dns_name, tailscale_funnel_config_matches,
        take_cached_action_for_pull, take_cached_action_for_pull_with_window_bindings,
        try_write_serve_response_file, update_active_session_registry,
        update_paired_device_file_roots_at, websocket_auth_denial,
        websocket_desktop_token_from_protocols, websocket_device_id_from_message,
        websocket_device_token_from_message, websocket_device_token_from_protocols,
        websocket_device_token_from_uri, websocket_probe_auth_required,
        websocket_probe_ok_or_auth_required, websocket_scope_enforced, ActiveSessionEntry,
        ApnsLiveActivityInfo, ApnsLiveActivitySendStats, ApnsLiveActivityUpdateRequest,
        ApnsNotifyRequest, ApnsRegisterRequest, AuthPrincipal, BridgeHttpState, BridgeMessage,
        CacheLookupRoute, CacheMetrics, CachedPublicProbe, DeleteQuery, DesktopCodexLiveCommand,
        DesktopCodexLiveControlRequest, DesktopCodexLiveLeaseRequest, DesktopCodexLiveState,
        DesktopCodexLiveStatusRequest, FilesQuery, ImageQuery, MobilePairingCandidate,
        PairedDeviceRecord, PairedDeviceStore, PairingCandidatesResult, PhoneActionJobEntry,
        PhoneActionRequest, PhoneActionResultEntry, PullActionQuery, PushUnsubscribeRequest,
        QuotaSnapshotRefreshGate, RoomSubmitRequest, TimelineSyncService, WebLoginPairClaimRequest,
        WebPushSubscriptionInfo, WsClientInfo, ACTIVE_SESSION_REGISTRY,
        APNS_NOTIFICATION_DEDUPE_SECS, APNS_NOTIFICATION_REQUEST_DEDUPE_SECS, FILE_LIST_MAX_DEPTH,
        LIVE_ACTIVITY_KIND_LIVE_GOAL, LIVE_ACTIVITY_KIND_QUOTA, MCP_ACTION_CACHE_TTL_SECS,
        MCP_STATE_CACHE, MCP_STATE_CACHE_TOUCHED_AT, PHONE_ACTION_INLINE_PAYLOAD_MAX_BYTES,
        PHONE_ACTION_RESULT_TTL_SECS, PUBLIC_PROBE_CACHE, PUBLIC_PROBE_CACHE_MAX_AGE_SECS,
        PUBLIC_PROBE_REFRESH_IN_FLIGHT, SCOPE_BRIDGE_PUBLISH, SCOPE_CONFIG_READ,
        SCOPE_CONFIG_WRITE, SCOPE_FILE_LIST, SCOPE_GHOST_SUGGESTIONS_READ,
        SCOPE_GHOST_SUGGESTIONS_WRITE, SCOPE_NOTIFICATION_SEND, SCOPE_NOTIFICATION_SUBSCRIBE,
        SCOPE_PAIRING_ISSUE, SCOPE_PHONE_ACTION_JOB_READ, SCOPE_PROMPT_LIBRARY_READ,
        SCOPE_PROMPT_LIBRARY_WRITE, SCOPE_SERVICE_RECOVER, SCOPE_SESSION_READ,
        SCOPE_SESSION_RESPOND, SCOPE_SPEECH_MEMORY_READ, SCOPE_SPEECH_MEMORY_WRITE,
        SCOPE_STATUS_READ, SCOPE_TUNNEL_RECOVER, SCOPE_WINDOW_SHOW, WEB_LOGIN_PAIRING_NONCES,
        WEB_LOGIN_SESSIONS, WS_CLIENT_REGISTRY,
    };
    use crate::bridge::markdown_images::{
        clear_markdown_image_registry_for_tests, rewrite_markdown_local_images,
    };
    use crate::conversation::{ConversationNode, NodeMetadata, NodeType};
    use axum::{
        extract::{Query, State},
        http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
        Json,
    };
    use once_cell::sync::Lazy;
    use std::collections::HashMap;
    use std::ffi::{OsStr, OsString};
    use std::net::SocketAddr;
    use std::path::PathBuf;
    use std::sync::{atomic::Ordering, Arc, Barrier, Mutex};
    use std::time::{Duration, Instant};
    use tokio::sync::{broadcast, RwLock};

    static PUBLIC_PROBE_TEST_LOCK: Lazy<tokio::sync::Mutex<()>> =
        Lazy::new(|| tokio::sync::Mutex::new(()));

    #[test]
    fn desktop_codex_live_bridge_restart_gets_a_new_epoch() {
        let first = DesktopCodexLiveState::default();
        let second = DesktopCodexLiveState::default();
        assert!(!first.server_epoch.is_empty());
        assert_ne!(first.server_epoch, second.server_epoch);
        assert_eq!(first.revision, 0);
    }

    #[test]
    fn desktop_codex_live_stale_host_cannot_remain_falsely_active() {
        let mut live = DesktopCodexLiveState::default();
        live.phase = "active".to_string();
        live.host_id = Some("old-host".to_string());
        live.host_lease_updated_at_ms = 1_000;
        expire_stale_desktop_codex_live_host(&mut live, 7_000);
        assert_eq!(live.phase, "failed");
        assert_eq!(live.host_id, None);
        assert!(live.status_text.contains("宿主已离线"));
    }

    #[tokio::test]
    async fn desktop_codex_live_idle_host_lease_excludes_a_second_host() {
        let state = test_bridge_state();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );
        let epoch = state.desktop_codex_live.read().await.server_epoch.clone();

        let claim = |host_id: &str| DesktopCodexLiveLeaseRequest {
            server_epoch: epoch.clone(),
            host_id: host_id.to_string(),
        };

        let _ = handle_desktop_codex_live_lease(
            State(state.clone()),
            headers.clone(),
            Json(claim("host-a")),
        )
        .await;
        assert_eq!(
            state.desktop_codex_live.read().await.host_id.as_deref(),
            Some("host-a")
        );

        let _ = handle_desktop_codex_live_lease(
            State(state.clone()),
            headers.clone(),
            Json(claim("host-b")),
        )
        .await;
        assert_eq!(
            state.desktop_codex_live.read().await.host_id.as_deref(),
            Some("host-a"),
            "a live idle host must retain the single-host lease"
        );

        state
            .desktop_codex_live
            .write()
            .await
            .host_lease_updated_at_ms = 0;
        let _ =
            handle_desktop_codex_live_lease(State(state.clone()), headers, Json(claim("host-b")))
                .await;
        assert_eq!(
            state.desktop_codex_live.read().await.host_id.as_deref(),
            Some("host-b"),
            "an expired idle lease must be reclaimable"
        );
    }

    #[tokio::test]
    async fn desktop_codex_live_toggle_is_resolved_atomically_by_bridge_state() {
        let state = test_bridge_state();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );

        let toggle = |project_path: Option<&str>| DesktopCodexLiveControlRequest {
            action: "toggle".to_string(),
            project_path: project_path.map(ToString::to_string),
            microphone_muted: None,
        };

        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(toggle(Some("/tmp/project"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        {
            let live = state.desktop_codex_live.read().await;
            assert_eq!(live.revision, 1);
            assert_eq!(
                live.command.as_ref().map(|command| command.action.as_str()),
                Some("start")
            );
            assert_eq!(live.phase, "preparing");
        }

        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(toggle(Some("/tmp/project"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        {
            let live = state.desktop_codex_live.read().await;
            assert_eq!(live.revision, 2);
            assert_eq!(
                live.command.as_ref().map(|command| command.action.as_str()),
                Some("stop")
            );
            assert_eq!(
                live.phase, "preparing",
                "stop still waits for the host acknowledgement"
            );
        }

        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(toggle(Some("/tmp/project"))),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let live = state.desktop_codex_live.read().await;
        assert_eq!(live.revision, 3);
        assert_eq!(
            live.command.as_ref().map(|command| command.action.as_str()),
            Some("stop")
        );
    }

    #[tokio::test]
    async fn desktop_codex_live_mute_preserves_the_active_session() {
        let state = test_bridge_state();
        {
            let mut live = state.desktop_codex_live.write().await;
            live.phase = "active".to_string();
            live.status_text = "GPT-Live 已连接".to_string();
            live.active_project_path = Some("/tmp/project".to_string());
            live.active_thread_id = Some("thread-a".to_string());
        }
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );

        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "mute".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        let live = state.desktop_codex_live.read().await;
        assert_eq!(live.revision, 1);
        assert_eq!(live.phase, "active");
        assert_eq!(live.status_text, "GPT-Live 已连接");
        assert_eq!(live.active_project_path.as_deref(), Some("/tmp/project"));
        assert_eq!(live.active_thread_id.as_deref(), Some("thread-a"));
        assert!(live.microphone_muted);
        assert_eq!(
            live.command.as_ref().map(|command| command.action.as_str()),
            Some("mute")
        );
        assert_eq!(
            live.command
                .as_ref()
                .and_then(|command| command.microphone_muted),
            Some(true)
        );

        drop(live);
        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "mute".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let live = state.desktop_codex_live.read().await;
        assert_eq!(live.revision, 2);
        assert!(!live.microphone_muted);
        assert_eq!(
            live.command
                .as_ref()
                .and_then(|command| command.microphone_muted),
            Some(false),
            "two unacknowledged mute requests must converge to the final desired state"
        );

        drop(live);
        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers,
            Json(DesktopCodexLiveControlRequest {
                action: "mute".to_string(),
                project_path: None,
                microphone_muted: Some(false),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let live = state.desktop_codex_live.read().await;
        assert!(!live.microphone_muted);
        assert_eq!(
            live.command
                .as_ref()
                .and_then(|command| command.microphone_muted),
            Some(false),
            "an absolute mute request must be idempotent instead of flipping stale state"
        );
    }

    #[tokio::test]
    async fn desktop_codex_live_toggle_reuses_the_last_project_after_stop() {
        let state = test_bridge_state();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );

        let _ = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "start".to_string(),
                project_path: Some("/tmp/remembered".to_string()),
                microphone_muted: None,
            }),
        )
        .await;
        {
            let mut live = state.desktop_codex_live.write().await;
            live.phase = "idle".to_string();
            live.active_project_path = None;
        }

        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers,
            Json(DesktopCodexLiveControlRequest {
                action: "toggle".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let live = state.desktop_codex_live.read().await;
        assert_eq!(live.phase, "preparing");
        assert_eq!(live.active_project_path.as_deref(), Some("/tmp/remembered"));
        assert_eq!(
            live.command
                .as_ref()
                .and_then(|command| command.project_path.as_deref()),
            Some("/tmp/remembered")
        );
    }

    #[tokio::test]
    async fn desktop_codex_live_toggle_reuses_the_persisted_project_after_bridge_restart() {
        let state = test_bridge_state();
        *state.desktop_codex_live.write().await =
            DesktopCodexLiveState::with_last_project_path(Some("/tmp/persisted".to_string()));
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );

        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers,
            Json(DesktopCodexLiveControlRequest {
                action: "toggle".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::OK);
        let live = state.desktop_codex_live.read().await;
        assert_eq!(live.phase, "preparing");
        assert_eq!(live.active_project_path.as_deref(), Some("/tmp/persisted"));
        assert_eq!(
            live.command
                .as_ref()
                .and_then(|command| command.project_path.as_deref()),
            Some("/tmp/persisted")
        );
    }

    #[tokio::test]
    async fn desktop_codex_live_start_is_idempotent_while_session_is_live() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );

        for phase in ["preparing", "connecting", "active", "reconnecting"] {
            let state = test_bridge_state();
            {
                let mut live = state.desktop_codex_live.write().await;
                live.revision = 7;
                live.phase = phase.to_string();
                live.status_text = format!("existing-{phase}");
                live.active_project_path = Some("/tmp/existing".to_string());
                live.active_thread_id = Some("thread-existing".to_string());
            }

            let response = handle_desktop_codex_live_control(
                State(state.clone()),
                headers.clone(),
                Json(DesktopCodexLiveControlRequest {
                    action: "start".to_string(),
                    project_path: Some("/tmp/replacement-must-not-win".to_string()),
                    microphone_muted: None,
                }),
            )
            .await;

            assert_eq!(response.status(), StatusCode::OK, "phase={phase}");
            let live = state.desktop_codex_live.read().await;
            assert_eq!(live.revision, 7, "phase={phase}");
            assert_eq!(live.phase, phase, "phase={phase}");
            assert_eq!(live.status_text, format!("existing-{phase}"));
            assert_eq!(
                live.active_project_path.as_deref(),
                Some("/tmp/existing"),
                "phase={phase}"
            );
            assert_eq!(
                live.active_thread_id.as_deref(),
                Some("thread-existing"),
                "phase={phase}"
            );
            assert!(live.command.is_none(), "phase={phase}");
        }
    }

    #[tokio::test]
    async fn desktop_codex_live_short_press_stops_every_live_phase_and_idle_falls_back() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );
        let short = || DesktopCodexLiveControlRequest {
            action: "short".to_string(),
            project_path: None,
            microphone_muted: None,
        };

        let state = test_bridge_state();
        let idle_response =
            handle_desktop_codex_live_control(State(state.clone()), headers.clone(), Json(short()))
                .await;
        assert_eq!(idle_response.status(), StatusCode::NO_CONTENT);
        assert_eq!(state.desktop_codex_live.read().await.revision, 0);

        for phase in ["preparing", "connecting", "active", "reconnecting"] {
            let state = test_bridge_state();
            {
                let mut live = state.desktop_codex_live.write().await;
                live.phase = phase.to_string();
                live.revision = 7;
                live.active_thread_id = Some("thread-active".to_string());
            }

            let response = handle_desktop_codex_live_control(
                State(state.clone()),
                headers.clone(),
                Json(short()),
            )
            .await;
            assert_eq!(response.status(), StatusCode::OK, "phase={phase}");
            let live = state.desktop_codex_live.read().await;
            assert_eq!(live.revision, 8, "phase={phase}");
            assert_eq!(
                live.command.as_ref().map(|command| command.action.as_str()),
                Some("stop"),
                "phase={phase}"
            );
            assert_eq!(
                live.status_text, "正在结束全局 GPT-Live 主代理",
                "phase={phase}"
            );
            assert_eq!(live.phase, phase, "phase={phase}");
            assert!(live.active_thread_id.is_none(), "phase={phase}");
        }
    }

    #[tokio::test]
    async fn desktop_codex_live_short_press_replaces_a_pending_start_with_stop() {
        let state = test_bridge_state();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );
        {
            let mut live = state.desktop_codex_live.write().await;
            live.phase = "preparing".to_string();
            live.revision = 7;
            live.command = Some(DesktopCodexLiveCommand {
                action: "start".to_string(),
                project_path: Some("/tmp/project".to_string()),
                microphone_muted: None,
            });
        }

        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers,
            Json(DesktopCodexLiveControlRequest {
                action: "short".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let live = state.desktop_codex_live.read().await;
        assert_eq!(live.revision, 8);
        assert_eq!(
            live.command.as_ref().map(|command| command.action.as_str()),
            Some("stop")
        );
        assert_eq!(live.status_text, "正在结束全局 GPT-Live 主代理");
    }

    #[tokio::test]
    async fn desktop_codex_live_mute_cannot_overwrite_pending_lifecycle_commands() {
        let state = test_bridge_state();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );
        let epoch = state.desktop_codex_live.read().await.server_epoch.clone();
        {
            let mut live = state.desktop_codex_live.write().await;
            live.phase = "active".to_string();
            live.active_project_path = Some("/tmp/project".to_string());
            live.last_project_path = live.active_project_path.clone();
            live.host_id = Some("host-a".to_string());
            live.host_lease_updated_at_ms = chrono::Utc::now().timestamp_millis();
        }

        let _ = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "stop".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;
        let _ = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "mute".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;
        {
            let live = state.desktop_codex_live.read().await;
            assert_eq!(live.revision, 1);
            assert_eq!(
                live.command.as_ref().map(|command| command.action.as_str()),
                Some("stop")
            );
            assert!(live.pending_mute_after_lifecycle);
        }

        let _ = handle_desktop_codex_live_status(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveStatusRequest {
                server_epoch: epoch.clone(),
                host_id: "host-a".to_string(),
                revision: 1,
                phase: "idle".to_string(),
                status_text: "已结束".to_string(),
                active_project_path: None,
                active_thread_id: None,
                microphone_muted: false,
            }),
        )
        .await;
        {
            let live = state.desktop_codex_live.read().await;
            assert_eq!(live.phase, "idle");
            assert!(live.command.is_none());
            assert!(!live.pending_mute_after_lifecycle);
        }

        let _ = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "start".to_string(),
                project_path: Some("/tmp/project".to_string()),
                microphone_muted: None,
            }),
        )
        .await;
        let _ = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "mute".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;
        let _ = handle_desktop_codex_live_status(
            State(state.clone()),
            headers,
            Json(DesktopCodexLiveStatusRequest {
                server_epoch: epoch,
                host_id: "host-a".to_string(),
                revision: 2,
                phase: "preparing".to_string(),
                status_text: "正在准备".to_string(),
                active_project_path: Some("/tmp/project".to_string()),
                active_thread_id: None,
                microphone_muted: false,
            }),
        )
        .await;
        let live = state.desktop_codex_live.read().await;
        assert_eq!(live.revision, 3);
        assert_eq!(
            live.command.as_ref().map(|command| command.action.as_str()),
            Some("mute")
        );
        assert_eq!(
            live.command
                .as_ref()
                .and_then(|command| command.microphone_muted),
            Some(true)
        );
    }

    #[tokio::test]
    async fn desktop_codex_live_control_rejects_stale_status_and_waits_for_stop_ack() {
        let state = test_bridge_state();
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-trusted-capability",
            HeaderValue::from_static("1"),
        );

        let response = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "start".to_string(),
                project_path: Some("/tmp/project".to_string()),
                microphone_muted: None,
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let epoch = state.desktop_codex_live.read().await.server_epoch.clone();

        let _ = handle_desktop_codex_live_lease(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveLeaseRequest {
                server_epoch: epoch.clone(),
                host_id: "host-a".to_string(),
            }),
        )
        .await;

        let _ = handle_desktop_codex_live_status(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveStatusRequest {
                server_epoch: "stale-bridge".to_string(),
                host_id: "host-a".to_string(),
                revision: 1,
                phase: "active".to_string(),
                status_text: "stale".to_string(),
                active_project_path: Some("/tmp/project".to_string()),
                active_thread_id: None,
                microphone_muted: false,
            }),
        )
        .await;
        assert_eq!(state.desktop_codex_live.read().await.phase, "preparing");

        let _ = handle_desktop_codex_live_status(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveStatusRequest {
                server_epoch: epoch.clone(),
                host_id: "host-a".to_string(),
                revision: 1,
                phase: "active".to_string(),
                status_text: "connected".to_string(),
                active_project_path: Some("/tmp/project".to_string()),
                active_thread_id: Some("thread-a".to_string()),
                microphone_muted: false,
            }),
        )
        .await;
        assert_eq!(state.desktop_codex_live.read().await.phase, "active");

        let _ = handle_desktop_codex_live_control(
            State(state.clone()),
            headers.clone(),
            Json(DesktopCodexLiveControlRequest {
                action: "stop".to_string(),
                project_path: None,
                microphone_muted: None,
            }),
        )
        .await;
        {
            let live = state.desktop_codex_live.read().await;
            assert_eq!(live.revision, 2);
            assert_eq!(live.phase, "active", "stop must wait for the host ack");
        }

        let _ = handle_desktop_codex_live_status(
            State(state.clone()),
            headers,
            Json(DesktopCodexLiveStatusRequest {
                server_epoch: epoch,
                host_id: "host-a".to_string(),
                revision: 1,
                phase: "idle".to_string(),
                status_text: "late stop".to_string(),
                active_project_path: None,
                active_thread_id: None,
                microphone_muted: false,
            }),
        )
        .await;
        let live = state.desktop_codex_live.read().await;
        assert_eq!(live.revision, 2);
        assert_eq!(live.phase, "active");
    }

    static ROUTE_DEBUG_TEST_LOCK: Lazy<tokio::sync::Mutex<()>> =
        Lazy::new(|| tokio::sync::Mutex::new(()));
    static ROOM_SUBMIT_TEST_LOCK: Lazy<tokio::sync::Mutex<()>> =
        Lazy::new(|| tokio::sync::Mutex::new(()));
    static WS_CLIENT_REGISTRY_TEST_LOCK: Lazy<tokio::sync::Mutex<()>> =
        Lazy::new(|| tokio::sync::Mutex::new(()));
    static WINDOW_REGISTRY_TEST_LOCK: Lazy<tokio::sync::Mutex<()>> =
        Lazy::new(|| tokio::sync::Mutex::new(()));
    static WEB_LOGIN_TEST_LOCK: Lazy<tokio::sync::Mutex<()>> =
        Lazy::new(|| tokio::sync::Mutex::new(()));
    static APNS_ENV_TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    struct WindowRegistryFileGuard {
        path: PathBuf,
        previous: Option<String>,
    }

    impl WindowRegistryFileGuard {
        fn with_instances(instances: Vec<crate::ui::window_registry::WindowInstance>) -> Self {
            let path = std::env::temp_dir().join("iterate_windows.json");
            let previous = std::fs::read_to_string(&path).ok();
            let registry = crate::ui::window_registry::WindowRegistry {
                instances,
                ..Default::default()
            };
            std::fs::write(
                &path,
                serde_json::to_string_pretty(&registry).expect("serialize window registry"),
            )
            .expect("write window registry");

            Self { path, previous }
        }
    }

    impl Drop for WindowRegistryFileGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                let _ = std::fs::write(&self.path, previous);
            } else {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    fn window_instance_for_test(
        project_path: &str,
        request_id: &str,
    ) -> crate::ui::window_registry::WindowInstance {
        crate::ui::window_registry::WindowInstance {
            pid: std::process::id(),
            project_path: project_path.to_string(),
            window_title: format!("iterate - {}", project_path),
            registered_at: chrono::Utc::now().to_rfc3339(),
            port: Some(5311),
            request_id: Some(request_id.to_string()),
            request_title: Some(request_id.to_string()),
        }
    }

    async fn clear_mcp_state_route_for_test(request_id: &str, project_path: &str) {
        {
            let mut cache = MCP_STATE_CACHE.write().await;
            cache.remove(request_id);
            cache.remove(project_path);
        }
        {
            let mut touched_at = MCP_STATE_CACHE_TOUCHED_AT.write().await;
            touched_at.remove(request_id);
            touched_at.remove(project_path);
        }
        {
            let mut registry = ACTIVE_SESSION_REGISTRY.write().await;
            registry.retain(|_, entry| {
                entry.request_id != request_id && entry.project_path != project_path
            });
        }
    }

    struct EnvGuard {
        key: &'static str,
        previous: Option<OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
            let guard = Self::capture(key);
            std::env::set_var(key, value);
            guard
        }

        fn remove(key: &'static str) -> Self {
            let guard = Self::capture(key);
            std::env::remove_var(key);
            guard
        }

        fn capture(key: &'static str) -> Self {
            Self {
                key,
                previous: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = &self.previous {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn goal_payload_keeps_images_in_payload_without_target_internals() {
        let payload = serde_json::json!({
            "user_input": "",
            "selected_options": [],
            "images": [{
                "data": "data:image/jpeg;base64,abc123",
                "media_type": "image/jpeg",
                "filename": "photo.jpg"
            }]
        });

        let (goal_text, goal_title, selected_options) = build_goal_payload_parts(&payload);
        assert_eq!(goal_text, "");
        assert_eq!(goal_title, "图片目标: 1 张");
        assert_eq!(selected_options, serde_json::json!([]));

        let prompt = render_goal_submit_prompt(
            &goal_text,
            crate::constants::mcp::DEFAULT_GOAL_PROMPT_TEMPLATE,
        );
        assert!(prompt.contains("目标：\n《\n"));
        assert!(prompt.contains("先把这句话整理成可执行目标"));
        assert!(prompt.contains("执行任何实现动作前"));
        assert!(prompt.contains("get_goal"));
        assert!(prompt.contains("create_goal"));
        assert!(prompt.contains("update_goal 为 complete"));
        assert!(prompt.contains("Codex 正式 Goal 是唯一状态源"));
        assert!(prompt.contains("绝不能伪造完成或在未同步状态下继续"));
        assert!(prompt.contains("任何实现动作前必须执行 xi"));
        assert!(prompt.contains("只有已有证据证明它确实完成"));
        assert!(!prompt.contains("先建立 Goal Spec"));
        assert!(!prompt.contains("success_criteria"));
        assert!(!prompt.contains("stop_conditions"));
        assert!(!prompt.contains("images[0]"));
        assert!(!prompt.contains("见 images 附件"));

        let images = normalize_mcp_action_images(&payload);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0]["data"], "abc123");
        assert_eq!(images[0]["media_type"], "image/jpeg");
        assert_eq!(images[0]["filename"], "photo.jpg");
    }

    #[test]
    fn goal_payload_includes_selected_options_with_user_input() {
        let payload = serde_json::json!({
            "user_input": "修复 GoalRun 目标",
            "selected_options": ["桌面一起做", "手机一起做"],
            "images": [{
                "data": "data:image/png;base64,abc123",
                "media_type": "image/png",
                "filename": null
            }]
        });

        let (goal_text, goal_title, selected_options) = build_goal_payload_parts(&payload);
        assert_eq!(goal_title, "修复 GoalRun 目标");
        assert_eq!(
            selected_options,
            serde_json::json!(["桌面一起做", "手机一起做"])
        );
        assert_eq!(
            goal_text,
            "修复 GoalRun 目标\n\n选中的选项：\n- 桌面一起做\n- 手机一起做"
        );

        let prompt = build_goal_submit_prompt(&goal_text);
        assert!(prompt.contains("目标：\n《\n修复 GoalRun 目标"));
        assert!(prompt.contains("选中的选项：\n- 桌面一起做\n- 手机一起做"));
    }

    #[test]
    fn goal_payload_does_not_duplicate_selected_options_already_in_goal() {
        let payload = serde_json::json!({
            "user_input": "修复 GoalRun 目标\n\n选中的选项：\n- 桌面一起做",
            "selected_options": ["桌面一起做"],
            "images": []
        });

        let (goal_text, _, _) = build_goal_payload_parts(&payload);
        assert_eq!(goal_text.matches("桌面一起做").count(), 1);
        assert_eq!(goal_text.matches("选中的选项：").count(), 1);
    }

    #[test]
    fn goal_payload_strips_existing_image_reference_context() {
        let payload = serde_json::json!({
            "user_input": "修这个问题\n\n附加图片：1 张（见 images 附件）",
            "selected_options": [],
            "images": [{
                "data": "raw-base64",
                "media_type": "image/png",
                "filename": null
            }]
        });

        let (goal_text, _, _) = build_goal_payload_parts(&payload);
        assert_eq!(goal_text, "修这个问题");
        assert!(!goal_text.contains("附加图片："));
        assert!(!goal_text.contains("见 images 附件"));
        assert_eq!(
            normalize_mcp_action_images(&payload)[0]["data"],
            "raw-base64"
        );
    }

    #[test]
    fn goal_payload_ignores_malformed_images_when_building_target() {
        let payload = serde_json::json!({
            "user_input": "",
            "selected_options": [],
            "images": [{}]
        });

        let (goal_text, goal_title, _) = build_goal_payload_parts(&payload);
        assert_eq!(goal_text, "");
        assert_eq!(goal_title, "");
        assert!(normalize_mcp_action_images(&payload).is_empty());
    }

    #[test]
    fn quota_snapshot_refresh_gate_coalesces_in_flight_and_recent_refreshes() {
        let mut gate = QuotaSnapshotRefreshGate::default();
        let started_at = Instant::now();
        let cooldown = Duration::from_secs(15);

        assert!(gate.should_spawn("default", started_at, cooldown));
        assert!(!gate.should_spawn("default", started_at + Duration::from_secs(1), cooldown));
        assert!(gate.should_spawn("other", started_at + Duration::from_secs(1), cooldown));

        gate.finish("default");
        assert!(!gate.should_spawn("default", started_at + Duration::from_secs(2), cooldown));
        assert!(gate.should_spawn("default", started_at + cooldown, cooldown));
    }

    #[test]
    fn quota_live_activity_fingerprint_commits_on_any_sent_token() {
        let partial_success = ApnsLiveActivitySendStats {
            success: true,
            event: "update".to_string(),
            matched: 2,
            sent: 1,
            failed: 1,
            invalidated: 0,
            message: "partial".to_string(),
        };
        assert!(quota_live_activity_fingerprint_send_succeeded(
            &partial_success
        ));

        let complete_failure = ApnsLiveActivitySendStats {
            success: false,
            event: "update".to_string(),
            matched: 2,
            sent: 0,
            failed: 2,
            invalidated: 0,
            message: "failed".to_string(),
        };
        assert!(!quota_live_activity_fingerprint_send_succeeded(
            &complete_failure
        ));

        let no_match = ApnsLiveActivitySendStats {
            success: false,
            event: "update".to_string(),
            matched: 0,
            sent: 0,
            failed: 0,
            invalidated: 0,
            message: "no match".to_string(),
        };
        assert!(!quota_live_activity_fingerprint_send_succeeded(&no_match));
    }

    fn build_payload(request_id: &str, project_path: &str, message: &str) -> serde_json::Value {
        serde_json::json!({
            "request": {
                "id": request_id,
                "project_path": project_path,
                "message": message,
            }
        })
    }

    fn build_payload_with_timeline_route(
        request_id: &str,
        project_path: &str,
        message: &str,
        timeline_route_id: &str,
    ) -> serde_json::Value {
        let mut payload = build_payload(request_id, project_path, message);
        payload["timeline_route_id"] = serde_json::Value::String(timeline_route_id.to_string());
        payload
    }

    fn timeline_node(
        id: &str,
        conversation_id: Option<&str>,
        request_id: Option<&str>,
        project_path: Option<&str>,
    ) -> ConversationNode {
        ConversationNode {
            id: id.to_string(),
            parent_id: None,
            timestamp: "2026-06-15T00:00:00Z".to_string(),
            node_type: NodeType::Assistant,
            content: id.to_string(),
            is_markdown: true,
            metadata: NodeMetadata {
                conversation_id: conversation_id.map(ToOwned::to_owned),
                request_id: request_id.map(ToOwned::to_owned),
                project_path: project_path.map(ToOwned::to_owned),
                ..NodeMetadata::default()
            },
        }
    }

    #[test]
    fn timeline_sync_sanitizes_frontend_nodes_for_requested_route() {
        let nodes = vec![
            serde_json::json!({
                "id": "match",
                "metadata": {
                    "conversation_id": "tree-1",
                    "request_id": "req-1",
                    "project_path": "/tmp/project",
                    "images": [{ "id": "img-1", "data": "base64" }]
                }
            }),
            serde_json::json!({
                "id": "other-request",
                "metadata": {
                    "conversation_id": "tree-1",
                    "request_id": "req-2",
                    "project_path": "/tmp/project"
                }
            }),
            serde_json::json!({
                "id": "other-tree",
                "metadata": {
                    "conversation_id": "tree-2",
                    "request_id": "req-1",
                    "project_path": "/tmp/project"
                }
            }),
            serde_json::json!({
                "id": "legacy",
                "metadata": {
                    "project_path": "/tmp/project"
                }
            }),
        ];

        let sanitized = TimelineSyncService::sanitize_timeline_values(
            &nodes,
            Some("req-1"),
            Some("/tmp/project"),
            Some("tree-1"),
        );
        let ids: Vec<_> = sanitized
            .iter()
            .filter_map(|node| node.get("id").and_then(|value| value.as_str()))
            .collect();

        assert_eq!(ids, vec!["match", "legacy"]);
        assert!(sanitized[0].pointer("/metadata/images/0/data").is_none());
    }

    #[test]
    fn timeline_sync_filters_manager_nodes_for_requested_route() {
        let nodes = vec![
            timeline_node("match", Some("tree-1"), Some("req-1"), Some("/tmp/project")),
            timeline_node(
                "other-request",
                Some("tree-1"),
                Some("req-2"),
                Some("/tmp/project"),
            ),
            timeline_node(
                "other-tree",
                Some("tree-2"),
                Some("req-1"),
                Some("/tmp/project"),
            ),
            timeline_node("legacy", None, None, Some("/tmp/project")),
        ];

        let filtered = TimelineSyncService::strip_and_filter_nodes(
            &nodes,
            "tree-1",
            Some("req-1"),
            Some("/tmp/project"),
        );
        let ids: Vec<_> = filtered
            .iter()
            .filter_map(|node| node.get("id").and_then(|value| value.as_str()))
            .collect();

        assert_eq!(ids, vec!["match", "legacy"]);
    }

    #[test]
    fn mcp_state_route_extractors_accept_aliases() {
        let payload = serde_json::json!({
            "conversationId": "tree-1",
            "timelineRouteId": "thread-1",
            "request": {
                "request_id": "req-1",
                "projectPath": "/tmp/project"
            }
        });

        assert_eq!(
            extract_conversation_id_from_mcp_state(&payload).as_deref(),
            Some("tree-1")
        );
        assert_eq!(
            extract_request_id_from_mcp_state(&payload).as_deref(),
            Some("req-1")
        );
        assert_eq!(
            extract_timeline_route_id_from_mcp_state(&payload).as_deref(),
            Some("thread-1")
        );
        assert_eq!(
            extract_project_path_from_mcp_state(&payload).as_deref(),
            Some("/tmp/project")
        );
    }

    #[tokio::test]
    async fn room_submit_dedupe_replays_accepted_outcome_with_current_correlation() {
        let _guard = ROOM_SUBMIT_TEST_LOCK.lock().await;
        clear_room_submit_outcome_cache_for_tests();

        let first_request = RoomSubmitRequest {
            action: "submit".to_string(),
            project_path: "/tmp/project".to_string(),
            request_id: Some("serve-1".to_string()),
            room_id: "room-a".to_string(),
            room_token: "room_secret".to_string(),
            room_storage: None,
            target_agent: Some("ai-1".to_string()),
            correlation_id: "corr-1".to_string(),
            run_id: Some("run-1".to_string()),
            dedupe_key: Some("dedupe-1".to_string()),
        };
        let accepted = room_submit_outcome(
            Some(&first_request),
            "submit",
            "/tmp/project",
            Some("serve-1"),
            "accepted",
            None,
            true,
        );
        remember_room_submit_outcome(&first_request, &accepted);

        let retry_request = RoomSubmitRequest {
            correlation_id: "corr-2".to_string(),
            ..first_request.clone()
        };
        let replay = cached_room_submit_outcome(&retry_request).unwrap();
        assert!(replay.ok);
        assert_eq!(replay.status, "accepted");
        assert_eq!(replay.correlation_id.as_deref(), Some("corr-2"));
        assert_eq!(replay.dedupe_key.as_deref(), Some("dedupe-1"));
        assert_eq!(replay.target_agent.as_deref(), Some("ai-1"));
        assert!(replay.delivered);

        let different_target = RoomSubmitRequest {
            target_agent: Some("ai-2".to_string()),
            correlation_id: "corr-3".to_string(),
            ..first_request.clone()
        };
        assert!(cached_room_submit_outcome(&different_target).is_none());

        clear_room_submit_outcome_cache_for_tests();
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn bridge_publish_room_submit_rejects_stale_window_request() {
        let _room_guard = ROOM_SUBMIT_TEST_LOCK.lock().await;
        let _window_guard = WINDOW_REGISTRY_TEST_LOCK.lock().await;
        clear_room_submit_outcome_cache_for_tests();
        let _registry_guard = WindowRegistryFileGuard::with_instances(vec![
            crate::ui::window_registry::WindowInstance {
                pid: std::process::id(),
                project_path: "/Users/test/project".to_string(),
                window_title: "iterate — /Users/test/project".to_string(),
                registered_at: chrono::Utc::now().to_rfc3339(),
                port: Some(5311),
                request_id: Some("req-new".to_string()),
                request_title: Some("new".to_string()),
            },
        ]);

        let response = handle_bridge_publish(
            HeaderMap::new(),
            State(test_bridge_state()),
            Json(BridgeMessage {
                message_type: "mcp_action".to_string(),
                payload: serde_json::json!({
                    "action": "submit",
                    "project_path": "/Users/test/project",
                    "request_id": "req-old",
                    "room_id": "room-a",
                    "room_token": "room_secret",
                    "target_agent": "ai-1",
                    "correlation_id": "corr-stale",
                    "user_input": "hello",
                    "selected_options": [],
                    "images": []
                }),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["reason"].as_str(), Some("stale_request"));
        assert_eq!(body["status"].as_str(), Some("rejected"));
        assert_eq!(body["delivered"].as_bool(), Some(false));

        clear_room_submit_outcome_cache_for_tests();
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn bridge_publish_mcp_state_rejects_stale_window_request_without_side_effects() {
        let _route_guard = ROUTE_DEBUG_TEST_LOCK.lock().await;
        let _window_guard = WINDOW_REGISTRY_TEST_LOCK.lock().await;
        let project_path = format!("/tmp/cunzhi-stale-publish-{}", uuid::Uuid::new_v4());
        let old_request_id = format!("req-old-{}", uuid::Uuid::new_v4());
        let new_request_id = format!("req-new-{}", uuid::Uuid::new_v4());
        clear_mcp_state_route_for_test(&old_request_id, &project_path).await;
        clear_mcp_state_route_for_test(&new_request_id, &project_path).await;
        let _registry_guard =
            WindowRegistryFileGuard::with_instances(vec![window_instance_for_test(
                &project_path,
                &new_request_id,
            )]);
        let baseline_routes = route_debug_status_value().await;
        let state = test_bridge_state();
        let mut receiver = state.tx.subscribe();

        let response = handle_bridge_publish(
            HeaderMap::new(),
            State(state),
            Json(BridgeMessage {
                message_type: "mcp_state".to_string(),
                payload: build_payload(&old_request_id, &project_path, "old state"),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["ok"].as_bool(), Some(false));
        assert_eq!(body["reason"].as_str(), Some("stale_request"));
        assert_eq!(body["status"].as_str(), Some("rejected"));
        assert!(receiver.try_recv().is_err());
        assert!(!MCP_STATE_CACHE.read().await.contains_key(&old_request_id));
        assert!(!MCP_STATE_CACHE.read().await.contains_key(&project_path));
        assert!(!ACTIVE_SESSION_REGISTRY
            .read()
            .await
            .contains_key(&old_request_id));
        let routes_after = route_debug_status_value().await;
        assert_eq!(
            routes_after["last_active_route"],
            baseline_routes["last_active_route"]
        );

        clear_mcp_state_route_for_test(&old_request_id, &project_path).await;
        clear_mcp_state_route_for_test(&new_request_id, &project_path).await;
    }

    #[cfg(not(target_os = "windows"))]
    #[tokio::test]
    async fn apns_notify_rejects_stale_window_request_without_early_cache_or_notification_route() {
        let _route_guard = ROUTE_DEBUG_TEST_LOCK.lock().await;
        let _window_guard = WINDOW_REGISTRY_TEST_LOCK.lock().await;
        let project_path = format!("/tmp/cunzhi-stale-apns-{}", uuid::Uuid::new_v4());
        let old_request_id = format!("req-old-{}", uuid::Uuid::new_v4());
        let new_request_id = format!("req-new-{}", uuid::Uuid::new_v4());
        let body = format!("old notification {}", uuid::Uuid::new_v4());
        clear_mcp_state_route_for_test(&old_request_id, &project_path).await;
        clear_mcp_state_route_for_test(&new_request_id, &project_path).await;
        let _registry_guard =
            WindowRegistryFileGuard::with_instances(vec![window_instance_for_test(
                &project_path,
                &new_request_id,
            )]);
        let baseline_routes = route_debug_status_value().await;

        let response = handle_apns_notify(
            HeaderMap::new(),
            Json(ApnsNotifyRequest {
                body,
                title: Some("iterate".to_string()),
                project_path: Some(project_path.clone()),
                request_id: Some(old_request_id.clone()),
                predefined_options: vec![],
                is_markdown: true,
                codex_thread_id: None,
                codex_deeplink: None,
                loop_active: true,
                force_popup: false,
                source: Some("early_request_test".to_string()),
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_json(response).await;
        assert_eq!(body["ok"].as_bool(), Some(false));
        assert_eq!(body["reason"].as_str(), Some("stale_request"));
        assert_eq!(body["status"].as_str(), Some("rejected"));
        assert!(!MCP_STATE_CACHE.read().await.contains_key(&old_request_id));
        assert!(!MCP_STATE_CACHE.read().await.contains_key(&project_path));
        assert!(!ACTIVE_SESSION_REGISTRY
            .read()
            .await
            .contains_key(&old_request_id));
        let routes_after = route_debug_status_value().await;
        assert_eq!(
            routes_after["last_active_route"],
            baseline_routes["last_active_route"]
        );
        assert_eq!(
            routes_after["last_notification_route"],
            baseline_routes["last_notification_route"]
        );

        clear_mcp_state_route_for_test(&old_request_id, &project_path).await;
        clear_mcp_state_route_for_test(&new_request_id, &project_path).await;
    }

    #[tokio::test]
    async fn serve_response_file_attempt_reports_route_failures_and_success() {
        let request_id = format!("serve-route-test-{}", uuid::Uuid::new_v4());
        let route_file = serve_response_route_file(&request_id);
        let _ = std::fs::remove_file(&route_file);
        let debug_log = |_: &str| {};

        let missing = try_write_serve_response_file(
            Some(&request_id),
            "/tmp/project-a",
            "{\"ok\":true}",
            MCP_ACTION_CACHE_TTL_SECS,
            &debug_log,
        );
        assert!(!missing.delivered);
        assert_eq!(missing.method, "serve_response_file");
        assert_eq!(missing.reason.as_deref(), Some("response_route_missing"));
        let route_file_display = route_file.display().to_string();
        assert_eq!(
            missing.route_file.as_deref(),
            Some(route_file_display.as_str())
        );

        let response_file = std::env::temp_dir().join(format!(
            "iterate-response-test-{}.json",
            uuid::Uuid::new_v4()
        ));
        let route = serde_json::json!({
            "project_path": "/tmp/project-b",
            "response_file": response_file,
            "created_at": chrono::Utc::now().timestamp()
        });
        std::fs::write(&route_file, format!("{route}\n")).unwrap();
        let mismatch = try_write_serve_response_file(
            Some(&request_id),
            "/tmp/project-a",
            "{\"ok\":true}",
            MCP_ACTION_CACHE_TTL_SECS,
            &debug_log,
        );
        assert!(!mismatch.delivered);
        assert_eq!(
            mismatch.reason.as_deref(),
            Some("response_route_project_mismatch")
        );
        let response_file_display = response_file.display().to_string();
        assert_eq!(
            mismatch.response_file.as_deref(),
            Some(response_file_display.as_str())
        );

        let route = serde_json::json!({
            "project_path": "/tmp/project-a",
            "response_file": response_file,
            "created_at": chrono::Utc::now().timestamp()
        });
        std::fs::write(&route_file, format!("{route}\n")).unwrap();
        let delivered = try_write_serve_response_file(
            Some(&request_id),
            "/tmp/project-a",
            "{\"ok\":true}",
            MCP_ACTION_CACHE_TTL_SECS,
            &debug_log,
        );
        assert!(delivered.delivered);
        assert_eq!(delivered.reason, None);
        assert_eq!(
            std::fs::read_to_string(&response_file).unwrap(),
            "{\"ok\":true}"
        );
        assert!(!route_file.exists());

        let _ = std::fs::remove_file(&route_file);
        let _ = std::fs::remove_file(&response_file);
    }

    #[tokio::test]
    async fn room_submit_preflight_rejects_missing_serve_response_route() {
        let _guard = ROOM_SUBMIT_TEST_LOCK.lock().await;
        clear_room_submit_outcome_cache_for_tests();

        let project = std::env::temp_dir().join(format!(
            "iterate-room-submit-preflight-test-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = project.join(".cunzhi-memory/codex-room");
        let rooms = storage.join("rooms");
        std::fs::create_dir_all(&rooms).unwrap();

        let request_id = format!("serve-preflight-{}", uuid::Uuid::new_v4());
        let route_file = serve_response_route_file(&request_id);
        let _ = std::fs::remove_file(&route_file);
        let future = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        std::fs::write(
            rooms.join("room-a.json"),
            serde_json::json!({
                "room_id": "room-a",
                "room_token": "room_secret",
                "agent_registry": {
                    "ai-1": {
                        "agent_id": "ai-1",
                        "workspace": project.to_string_lossy(),
                        "port": "5344",
                        "request_id": request_id,
                        "status": "waiting_user",
                        "health": "healthy",
                        "expires_at": future,
                    }
                },
            })
            .to_string(),
        )
        .unwrap();

        let project_str = project.to_string_lossy().to_string();
        let storage_str = storage.to_string_lossy().to_string();
        let payload = serde_json::json!({
            "action": "submit",
            "room_id": "room-a",
            "room_token": "room_secret",
            "room_storage": storage_str,
            "target_agent": "ai-1",
            "correlation_id": "corr-preflight",
            "user_input": "hello",
            "selected_options": [],
            "images": [],
        });

        let outcome =
            handle_room_submit_action(None, &project_str, Some(&request_id), None, &payload).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.status, "rejected");
        assert_eq!(outcome.reason.as_deref(), Some("response_route_missing"));
        assert!(!outcome.delivered);
        assert_eq!(outcome.delivery_attempts.len(), 1);
        let attempt = &outcome.delivery_attempts[0];
        assert_eq!(attempt.method, "serve_response_file_preflight");
        assert!(!attempt.delivered);
        assert_eq!(attempt.reason.as_deref(), Some("response_route_missing"));
        let route_file_display = route_file.display().to_string();
        assert_eq!(
            attempt.route_file.as_deref(),
            Some(route_file_display.as_str())
        );

        let _ = std::fs::remove_file(&route_file);
        let _ = std::fs::remove_dir_all(project);
        clear_room_submit_outcome_cache_for_tests();
    }

    #[tokio::test]
    async fn room_submit_preflight_cleans_expired_serve_response_route() {
        let _guard = ROOM_SUBMIT_TEST_LOCK.lock().await;
        clear_room_submit_outcome_cache_for_tests();

        let project = std::env::temp_dir().join(format!(
            "iterate-room-submit-preflight-expired-test-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = project.join(".cunzhi-memory/codex-room");
        let rooms = storage.join("rooms");
        std::fs::create_dir_all(&rooms).unwrap();

        let request_id = format!("serve-preflight-expired-{}", uuid::Uuid::new_v4());
        let route_file = serve_response_route_file(&request_id);
        let response_file = std::env::temp_dir().join(format!(
            "iterate-response-preflight-expired-{}.json",
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_file(&route_file);
        let _ = std::fs::remove_file(&response_file);

        let future = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        std::fs::write(
            rooms.join("room-a.json"),
            serde_json::json!({
                "room_id": "room-a",
                "room_token": "room_secret",
                "agent_registry": {
                    "ai-1": {
                        "agent_id": "ai-1",
                        "workspace": project.to_string_lossy(),
                        "port": "5344",
                        "request_id": request_id,
                        "status": "waiting_user",
                        "health": "healthy",
                        "expires_at": future,
                    }
                },
            })
            .to_string(),
        )
        .unwrap();

        let project_str = project.to_string_lossy().to_string();
        std::fs::write(
            &route_file,
            serde_json::json!({
                "project_path": project_str,
                "response_file": response_file,
                "created_at": chrono::Utc::now()
                    .timestamp()
                    .saturating_sub(MCP_ACTION_CACHE_TTL_SECS + 10)
            })
            .to_string(),
        )
        .unwrap();

        let storage_str = storage.to_string_lossy().to_string();
        let payload = serde_json::json!({
            "action": "submit",
            "room_id": "room-a",
            "room_token": "room_secret",
            "room_storage": storage_str,
            "target_agent": "ai-1",
            "correlation_id": "corr-preflight-expired",
            "user_input": "hello",
            "selected_options": [],
            "images": [],
        });

        let outcome =
            handle_room_submit_action(None, &project_str, Some(&request_id), None, &payload).await;
        assert!(!outcome.ok);
        assert_eq!(outcome.status, "rejected");
        assert_eq!(outcome.reason.as_deref(), Some("response_route_expired"));
        assert_eq!(outcome.delivery_attempts.len(), 1);
        assert_eq!(
            outcome.delivery_attempts[0].method,
            "serve_response_file_preflight"
        );
        assert_eq!(
            outcome.delivery_attempts[0].reason.as_deref(),
            Some("response_route_expired")
        );
        assert!(!route_file.exists());

        let _ = std::fs::remove_file(&route_file);
        let _ = std::fs::remove_file(&response_file);
        let _ = std::fs::remove_dir_all(project);
        clear_room_submit_outcome_cache_for_tests();
    }

    #[tokio::test]
    async fn room_submit_preflight_accepts_valid_serve_response_route() {
        let _guard = ROOM_SUBMIT_TEST_LOCK.lock().await;
        clear_room_submit_outcome_cache_for_tests();

        let project = std::env::temp_dir().join(format!(
            "iterate-room-submit-preflight-ok-test-{}",
            uuid::Uuid::new_v4()
        ));
        let storage = project.join(".cunzhi-memory/codex-room");
        let rooms = storage.join("rooms");
        std::fs::create_dir_all(&rooms).unwrap();

        let request_id = format!("serve-preflight-ok-{}", uuid::Uuid::new_v4());
        let route_file = serve_response_route_file(&request_id);
        let response_file = std::env::temp_dir().join(format!(
            "iterate-response-preflight-ok-{}.json",
            uuid::Uuid::new_v4()
        ));
        let _ = std::fs::remove_file(&route_file);
        let _ = std::fs::remove_file(&response_file);

        let future = (chrono::Utc::now() + chrono::Duration::minutes(5)).to_rfc3339();
        std::fs::write(
            rooms.join("room-a.json"),
            serde_json::json!({
                "room_id": "room-a",
                "room_token": "room_secret",
                "agent_registry": {
                    "ai-1": {
                        "agent_id": "ai-1",
                        "workspace": project.to_string_lossy(),
                        "port": "5344",
                        "request_id": request_id,
                        "status": "waiting_user",
                        "health": "healthy",
                        "expires_at": future,
                    }
                },
            })
            .to_string(),
        )
        .unwrap();

        let project_str = project.to_string_lossy().to_string();
        std::fs::write(
            &route_file,
            serde_json::json!({
                "project_path": project_str,
                "response_file": response_file,
                "created_at": chrono::Utc::now().timestamp()
            })
            .to_string(),
        )
        .unwrap();

        let storage_str = storage.to_string_lossy().to_string();
        let payload = serde_json::json!({
            "action": "submit",
            "room_id": "room-a",
            "room_token": "room_secret",
            "room_storage": storage_str,
            "target_agent": "ai-1",
            "correlation_id": "corr-preflight-ok",
            "user_input": "hello",
            "selected_options": [],
            "images": [],
        });

        let outcome =
            handle_room_submit_action(None, &project_str, Some(&request_id), None, &payload).await;
        assert!(outcome.ok);
        assert_eq!(outcome.status, "accepted");
        assert_eq!(outcome.reason, None);
        assert!(outcome.delivered);
        assert_eq!(outcome.delivery_attempts.len(), 2);
        assert_eq!(
            outcome.delivery_attempts[0].method,
            "serve_response_file_preflight"
        );
        assert!(outcome.delivery_attempts[0].delivered);
        assert_eq!(outcome.delivery_attempts[0].reason, None);
        assert_eq!(outcome.delivery_attempts[1].method, "serve_response_file");
        assert!(outcome.delivery_attempts[1].delivered);
        assert_eq!(outcome.delivery_attempts[1].reason, None);
        assert!(!route_file.exists());

        let response: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&response_file).unwrap()).unwrap();
        assert_eq!(response["user_input"].as_str(), Some("hello"));
        assert_eq!(
            response["project_path"].as_str(),
            Some(project_str.as_str())
        );
        assert_eq!(
            response["metadata"]["request_id"].as_str(),
            Some(request_id.as_str())
        );

        let _ = std::fs::remove_file(&route_file);
        let _ = std::fs::remove_file(&response_file);
        let _ = std::fs::remove_dir_all(project);
        clear_room_submit_outcome_cache_for_tests();
    }

    fn public_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("cf-ray", HeaderValue::from_static("test-ray"));
        headers
    }

    fn test_bridge_state() -> BridgeHttpState {
        let (tx, _) = broadcast::channel(1);
        BridgeHttpState {
            app_handle: None,
            tx,
            port: 8080,
            desktop_codex_live: Arc::new(RwLock::new(DesktopCodexLiveState::default())),
        }
    }

    fn test_ws_client(
        client_id: &str,
        device_id: Option<&str>,
        connected_at: &str,
        last_seen_at: &str,
    ) -> WsClientInfo {
        WsClientInfo {
            client_id: client_id.to_string(),
            connected_at: connected_at.to_string(),
            last_seen_at: last_seen_at.to_string(),
            last_message_type: None,
            remote_addr: None,
            host: String::new(),
            x_forwarded_for: String::new(),
            x_forwarded_proto: String::new(),
            cf_ray: String::new(),
            user_agent: String::new(),
            authenticated: device_id.is_some(),
            authenticated_device_id: device_id.map(str::to_string),
            authenticated_client_kind: device_id.map(|_| "ios".to_string()),
            client_kind: "ios".to_string(),
            device_id: device_id.map(str::to_string),
            selected_transport_mode: None,
            selected_ws_url: None,
            project_path: None,
            request_id: None,
        }
    }

    fn test_live_activity_info(
        goal_id: &str,
        activity_kind: &str,
        activity_key: Option<&str>,
    ) -> ApnsLiveActivityInfo {
        ApnsLiveActivityInfo {
            activity_token: "token".to_string(),
            goal_id: goal_id.to_string(),
            activity_kind: activity_kind.to_string(),
            activity_key: activity_key.map(str::to_string),
            activity_id: None,
            device_id: "device".to_string(),
            platform: "ios".to_string(),
            app_version: "1.0".to_string(),
            project_path: None,
            request_id: None,
            registered_at: "2026-06-04T00:00:00Z".to_string(),
            last_seen_at: "2026-06-04T00:00:00Z".to_string(),
            environment: "sandbox".to_string(),
        }
    }

    fn empty_live_activity_update_request() -> ApnsLiveActivityUpdateRequest {
        ApnsLiveActivityUpdateRequest {
            activity_token: None,
            goal_id: None,
            activity_kind: None,
            activity_key: None,
            event: None,
            title: None,
            status: None,
            phase: None,
            status_text: None,
            progress_percent: None,
            progress_label: None,
            requires_action: None,
            elapsed_ms: None,
            started_at_ms: None,
            updated_at_ms: None,
            project_path: None,
            request_id: None,
            content_state: None,
        }
    }

    #[test]
    fn live_activity_info_matches_legacy_goal_id_as_live_goal_key() {
        let info = test_live_activity_info("goal-1", LIVE_ACTIVITY_KIND_LIVE_GOAL, None);

        assert!(live_activity_info_matches(
            &info,
            LIVE_ACTIVITY_KIND_LIVE_GOAL,
            "goal-1"
        ));
        assert!(!live_activity_info_matches(
            &info,
            LIVE_ACTIVITY_KIND_QUOTA,
            "goal-1"
        ));
    }

    #[test]
    fn quota_live_activity_content_state_uses_override_directly() {
        let mut request = empty_live_activity_update_request();
        request.activity_kind = Some(LIVE_ACTIVITY_KIND_QUOTA.to_string());
        request.content_state = Some(serde_json::json!({
            "status": "limited",
            "statusLabel": "受限 · 14:01 重置",
            "providerName": "Codex",
            "primaryLabel": "Session",
            "primaryRemaining": 0,
            "updatedAtMs": 1_780_560_000_000i64,
            "staleAfterMs": 1_780_560_360_000i64,
        }));

        let state =
            live_activity_content_state_from_update(&request, "update", LIVE_ACTIVITY_KIND_QUOTA);

        assert_eq!(
            state
                .get("primaryRemaining")
                .and_then(|value| value.as_i64()),
            Some(0)
        );
        assert!(state.get("title").is_none());
        assert!(state.get("progressPercent").is_none());
    }

    #[test]
    fn quota_snapshot_maps_to_quota_activity_content_state() {
        let snapshot = serde_json::json!({
            "status": "limited",
            "statusLabel": "受限 · 14:01 重置",
            "primary": {
                "providerId": "codex",
                "providerName": "Codex",
                "providerSummary": "Pro OAuth",
                "label": "Session",
                "remaining": 0,
                "resetLabel": "14:01 重置",
                "resetAtMs": 1_780_560_060_000i64
            },
            "secondary": {
                "providerId": "codex",
                "providerName": "Codex",
                "label": "Weekly",
                "remaining": 61
            },
            "updatedAtMs": 1_780_560_000_000i64,
            "staleAfterMs": 1_780_560_360_000i64
        });

        let state = quota_live_activity_content_state_from_snapshot(&snapshot).unwrap();

        assert_eq!(state["status"], "limited");
        assert_eq!(state["providerName"], "Codex");
        assert_eq!(state["providerSummary"], "Pro OAuth");
        assert_eq!(state["primaryRemaining"], 0);
        assert_eq!(state["secondaryLabel"], "Weekly");
        assert_eq!(state["secondaryRemaining"], 61);
    }

    #[test]
    fn treats_terminal_messages_as_inactive() {
        assert!(is_inactive_session_message("任务已结束，请查看最终状态。"));
        assert!(is_inactive_session_message("已停止分析，已 push 完成。"));
    }

    #[test]
    fn public_control_path_marks_remote_control_surface() {
        assert!(is_public_control_path("/"));
        assert!(is_public_control_path("/index.html"));
        assert!(is_public_control_path("/mobile"));
        assert!(is_public_control_path("/files"));
        assert!(is_public_control_path("/files/roots"));
        assert!(is_public_control_path("/files/mkdir"));
        assert!(is_public_control_path("/windows"));
        assert!(is_public_control_path("/image"));
        assert!(is_public_control_path("/ws"));
        assert!(is_public_control_path("/ws/codex-live"));
        assert!(is_public_control_path("/bridge/publish"));
        assert!(is_public_control_path("/bridge/pull_action"));
        assert!(is_public_control_path("/api/active-sessions"));
        assert!(is_public_control_path("/api/phone-action"));
        assert!(is_public_control_path("/api/phone-action-result"));
        assert!(is_public_control_path("/api/phone-action-jobs/job-1"));
        assert!(is_public_control_path("/api/mobile/pairing"));
        assert!(is_public_control_path("/api/mobile/pairing/claim"));
        assert!(is_public_control_path("/api/mobile/pairing/status"));
        assert!(is_public_control_path(
            "/api/mobile/paired-device-file-roots"
        ));
        assert!(is_public_control_path("/api/ghost-suggestions/reorder"));
        assert!(is_public_control_path("/api/ghost-suggestions/item-1"));
        assert!(is_public_control_path("/api/ghost-suggestion-learning"));
        assert!(is_public_control_path("/api/speech-muscle-memory"));
        assert!(is_public_control_path("/api/speech-correction-memory"));
        assert!(is_public_control_path("/api/speech-vocabulary"));
        assert!(is_public_control_path("/api/connection-status"));
        assert!(is_public_control_path("/api/recover-tailscale-funnel"));
        assert!(is_public_control_path("/api/restart-tunnel"));
        assert!(is_public_control_path("/push/subscribe"));
    }

    #[test]
    fn public_websocket_path_reaches_websocket_specific_auth() {
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/ws"
        ));
    }

    #[test]
    fn websocket_handler_does_not_consume_a_middleware_verified_nonce_twice() {
        let token =
            crate::bridge::auth::issue_internal_bridge_websocket_token("ws://127.0.0.1:8080/ws")
                .expect("mint internal websocket capability");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .expect("valid authorization header"),
        );

        assert!(
            crate::bridge::auth::authenticate_internal_bridge_bearer(&headers, "GET", "/ws")
                .expect("middleware verifies capability")
                .is_some()
        );
        headers.insert(
            HeaderName::from_static(super::TRUSTED_INTERNAL_CAPABILITY_HEADER),
            HeaderValue::from_static("1"),
        );

        assert!(super::authenticate_internal_websocket_once(
            &headers,
            "127.0.0.1:8080".parse().unwrap(),
        )
        .expect("handler trusts middleware verification"));
        assert!(
            crate::bridge::auth::authenticate_internal_bridge_bearer(&headers, "GET", "/ws")
                .is_err()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quick_tunnel_handler_trusts_middleware_without_consuming_nonce_twice() {
        let token =
            crate::bridge::auth::issue_internal_bridge_token("GET", "/api/quick-tunnel/status")
                .expect("mint internal Quick Tunnel capability");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .expect("valid authorization header"),
        );

        assert!(crate::bridge::auth::authenticate_internal_bridge_bearer(
            &headers,
            "GET",
            "/api/quick-tunnel/status",
        )
        .expect("middleware verifies capability")
        .is_some());
        headers.insert(
            HeaderName::from_static(super::TRUSTED_INTERNAL_CAPABILITY_HEADER),
            HeaderValue::from_static("1"),
        );

        assert!(super::quick_tunnel_internal_auth_denial(
            "127.0.0.1:8080".parse().unwrap(),
            &headers,
            "GET",
            "/api/quick-tunnel/status",
        )
        .is_none());
        assert!(
            crate::bridge::auth::authenticate_internal_bridge_bearer(
                &headers,
                "GET",
                "/api/quick-tunnel/status",
            )
            .is_err(),
            "the original bearer remains one-shot",
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn quick_tunnel_handler_rejects_an_unmarked_loopback_request() {
        let denial = super::quick_tunnel_internal_auth_denial(
            "127.0.0.1:8080".parse().unwrap(),
            &HeaderMap::new(),
            "GET",
            "/api/quick-tunnel/status",
        )
        .expect("unmarked request must be denied");

        assert_eq!(denial.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn fixed_public_pairing_candidate_is_the_secure_fast_path() {
        let candidate = super::fixed_public_pairing_candidate_from_probe(
            "https://iterate.example.com/",
            true,
            true,
            false,
            true,
            true,
        )
        .expect("healthy proven fixed public route");

        assert_eq!(candidate.transport_mode, "public_tunnel");
        assert_eq!(candidate.base_url, "https://iterate.example.com");
        assert_eq!(candidate.ws_url, "wss://iterate.example.com/ws");
        assert_eq!(candidate.health, "healthy");
        assert!(!candidate.disabled);
        assert!(super::mobile_pairing_candidate_is_secure_for_qr(&candidate));
    }

    #[test]
    fn fixed_public_pairing_candidate_keeps_endpoint_proof_and_tls_mandatory() {
        assert!(super::fixed_public_pairing_candidate_from_probe(
            "https://iterate.example.com",
            true,
            true,
            false,
            false,
            true,
        )
        .is_none());
        assert!(super::fixed_public_pairing_candidate_from_probe(
            "http://127.0.0.1:8080",
            true,
            true,
            false,
            true,
            false,
        )
        .is_none());
        assert!(super::fixed_public_pairing_candidate_from_probe(
            "https://iterate.example.com",
            false,
            false,
            false,
            true,
            false,
        )
        .is_none());
    }

    #[test]
    fn tailscale_dns_name_trims_magic_dns_dot() {
        let status = serde_json::json!({
            "Self": {
                "DNSName": "macbook-air.tail5b0fb3.ts.net."
            },
            "CertDomains": ["fallback.tail5b0fb3.ts.net"]
        });

        assert_eq!(
            tailscale_dns_name(&status).as_deref(),
            Some("macbook-air.tail5b0fb3.ts.net")
        );
    }

    #[test]
    fn tailscale_funnel_config_requires_443_proxy_and_allow_funnel() {
        let status = serde_json::json!({
            "Web": {
                "macbook-air.tail5b0fb3.ts.net:443": {
                    "Handlers": {
                        "/": {
                            "Proxy": "http://127.0.0.1:8080"
                        }
                    }
                }
            },
            "AllowFunnel": {
                "macbook-air.tail5b0fb3.ts.net:443": true
            }
        });

        assert!(tailscale_funnel_config_matches(
            &status,
            "macbook-air.tail5b0fb3.ts.net",
            "http://127.0.0.1:8080",
        ));
        assert!(!tailscale_funnel_config_matches(
            &status,
            "macbook-air.tail5b0fb3.ts.net",
            "http://127.0.0.1:9090",
        ));
    }

    #[test]
    fn public_control_path_leaves_health_surface_unmarked() {
        assert!(!is_public_control_path("/api/version"));
        assert!(!is_public_control_path("/apple-touch-icon.png"));
        assert!(!is_public_control_path("/manifest.webmanifest"));
        assert!(!is_public_control_path("/sw.js"));
    }

    #[test]
    fn phone_action_bridge_message_normalizes_payload() {
        let (id, message) = build_phone_action_bridge_message(
            PhoneActionRequest {
                id: Some(" action-1 ".to_string()),
                action: " set_clipboard ".to_string(),
                title: None,
                text: Some(" hello ".to_string()),
                url: None,
                browser: None,
                shortcut_name: None,
                requires_confirmation: false,
                source: None,
                target_device_id: Some(" iphone-1 ".to_string()),
            },
            "desktop_test",
        )
        .expect("valid phone action");

        assert_eq!(id, "action-1");
        assert_eq!(message.message_type, "phone_action_request");
        assert_eq!(message.payload["id"], "action-1");
        assert_eq!(message.payload["action"], "set_clipboard");
        assert_eq!(message.payload["text"], "hello");
        assert_eq!(message.payload["source"], "desktop_test");
        assert_eq!(message.payload["target_device_id"], "iphone-1");
        assert_eq!(message.payload["requires_confirmation"], false);
    }

    #[test]
    fn phone_action_bridge_message_preserves_shortcut_name() {
        let (id, message) = build_phone_action_bridge_message(
            PhoneActionRequest {
                id: Some("shortcut-1".to_string()),
                action: " run_shortcut ".to_string(),
                title: None,
                text: Some(" shortcut input ".to_string()),
                url: None,
                browser: None,
                shortcut_name: Some(" iterate ".to_string()),
                requires_confirmation: false,
                source: Some(" desktop_http ".to_string()),
                target_device_id: None,
            },
            "desktop_test",
        )
        .expect("valid shortcut phone action");

        assert_eq!(id, "shortcut-1");
        assert_eq!(message.message_type, "phone_action_request");
        assert_eq!(message.payload["action"], "run_shortcut");
        assert_eq!(message.payload["text"], "shortcut input");
        assert_eq!(message.payload["shortcut_name"], "iterate");
        assert_eq!(message.payload["source"], "desktop_http");
    }

    #[test]
    fn phone_action_job_metadata_removes_payload_from_websocket_message() {
        let mut message = BridgeMessage {
            message_type: "phone_action_request".to_string(),
            payload: serde_json::json!({
                "id": "action-job-1",
                "action": "show_message",
                "title": "Large",
                "text": "x".repeat(PHONE_ACTION_INLINE_PAYLOAD_MAX_BYTES + 1),
                "source": "desktop_test",
            }),
        };
        let payload =
            phone_action_job_payload_from_message(&message).expect("job payload is present");
        let payload_size = phone_action_job_payload_size(&payload).expect("payload serializes");
        assert!(payload_size > PHONE_ACTION_INLINE_PAYLOAD_MAX_BYTES);

        let job = PhoneActionJobEntry {
            id: "job-1".to_string(),
            action_id: "action-job-1".to_string(),
            action: "show_message".to_string(),
            payload,
            payload_size_bytes: payload_size,
            created_at: "2026-05-31T00:00:00Z".to_string(),
            expires_at: "2026-05-31T00:10:00Z".to_string(),
        };

        attach_phone_action_job_metadata(&mut message, &job);

        assert!(message.payload.get("text").is_none());
        assert!(message.payload.get("title").is_none());
        assert_eq!(message.payload["job_id"], "job-1");
        assert_eq!(message.payload["job_payload_size_bytes"], payload_size);
        assert_eq!(message.payload["source"], "desktop_test");
    }

    #[test]
    fn phone_action_bridge_message_rejects_unsafe_open_url_scheme() {
        let result = build_phone_action_bridge_message(
            PhoneActionRequest {
                id: None,
                action: "open_url".to_string(),
                title: None,
                text: None,
                url: Some("shortcuts://run-shortcut?name=anything".to_string()),
                browser: None,
                shortcut_name: None,
                requires_confirmation: false,
                source: None,
                target_device_id: None,
            },
            "desktop_test",
        );

        assert!(result
            .expect_err("unsafe open_url scheme should be rejected")
            .contains("open_url does not allow"));
    }

    #[test]
    fn phone_action_bridge_message_rejects_non_iterate_shortcut_name() {
        let result = build_phone_action_bridge_message(
            PhoneActionRequest {
                id: None,
                action: "run_shortcut".to_string(),
                title: None,
                text: None,
                url: None,
                browser: None,
                shortcut_name: Some("Open URLs".to_string()),
                requires_confirmation: false,
                source: None,
                target_device_id: None,
            },
            "desktop_test",
        );

        assert!(result
            .expect_err("unsafe shortcut name should be rejected")
            .contains("starting with iterate"));
    }

    #[test]
    fn phone_action_bridge_message_rejects_empty_action() {
        let result = build_phone_action_bridge_message(
            PhoneActionRequest {
                id: None,
                action: " ".to_string(),
                title: None,
                text: None,
                url: None,
                browser: None,
                shortcut_name: None,
                requires_confirmation: false,
                source: None,
                target_device_id: None,
            },
            "desktop_test",
        );

        assert!(result.is_err());
    }

    #[test]
    fn phone_action_target_device_id_accepts_snake_and_camel_case() {
        let snake = BridgeMessage {
            message_type: "phone_action_request".to_string(),
            payload: serde_json::json!({
                "target_device_id": "iphone-1",
            }),
        };
        let camel = BridgeMessage {
            message_type: "phone_action_request".to_string(),
            payload: serde_json::json!({
                "targetDeviceId": "iphone-2",
            }),
        };
        let other = BridgeMessage {
            message_type: "mcp_state".to_string(),
            payload: serde_json::json!({
                "target_device_id": "iphone-3",
            }),
        };

        assert_eq!(
            phone_action_target_device_id(&snake).as_deref(),
            Some("iphone-1")
        );
        assert_eq!(
            phone_action_target_device_id(&camel).as_deref(),
            Some("iphone-2")
        );
        assert!(phone_action_target_device_id(&other).is_none());
    }

    #[test]
    fn bridge_message_targets_only_matching_phone_device() {
        let targeted = BridgeMessage {
            message_type: "phone_action_request".to_string(),
            payload: serde_json::json!({
                "target_device_id": "iphone-1",
            }),
        };
        let untargeted = BridgeMessage {
            message_type: "phone_action_request".to_string(),
            payload: serde_json::json!({}),
        };

        assert!(bridge_message_targets_device(&targeted, Some("iphone-1")));
        assert!(!bridge_message_targets_device(&targeted, Some("iphone-2")));
        assert!(!bridge_message_targets_device(&targeted, None));
        assert!(bridge_message_targets_device(&untargeted, None));
    }

    #[tokio::test]
    async fn phone_action_delivery_dedupes_latest_client_per_device() {
        let _guard = WS_CLIENT_REGISTRY_TEST_LOCK.lock().await;

        {
            let mut registry = WS_CLIENT_REGISTRY.write().await;
            registry.clear();
            registry.insert(
                "old".to_string(),
                test_ws_client(
                    "old",
                    Some("iphone-1"),
                    "2026-05-30T09:00:00Z",
                    "2026-05-30T09:01:00Z",
                ),
            );
            registry.insert(
                "new".to_string(),
                test_ws_client(
                    "new",
                    Some("iphone-1"),
                    "2026-05-30T09:02:00Z",
                    "2026-05-30T09:03:00Z",
                ),
            );
            let mut browser_client = test_ws_client(
                "browser-client",
                Some("iphone-1"),
                "2026-05-30T09:08:00Z",
                "2026-05-30T09:09:00Z",
            );
            browser_client.client_kind = "browser".to_string();
            registry.insert("browser-client".to_string(), browser_client);
            registry.insert(
                "other-device".to_string(),
                test_ws_client(
                    "other-device",
                    Some("iphone-2"),
                    "2026-05-30T09:04:00Z",
                    "2026-05-30T09:05:00Z",
                ),
            );
            registry.insert(
                "legacy".to_string(),
                test_ws_client(
                    "legacy",
                    None,
                    "2026-05-30T09:06:00Z",
                    "2026-05-30T09:07:00Z",
                ),
            );
            let mut unknown_legacy = test_ws_client(
                "unknown-legacy",
                None,
                "2026-05-30T09:10:00Z",
                "2026-05-30T09:11:00Z",
            );
            unknown_legacy.client_kind = "unknown".to_string();
            registry.insert("unknown-legacy".to_string(), unknown_legacy);
        }

        let targeted = phone_action_delivery_client_ids(Some("iphone-1")).await;
        assert_eq!(targeted.len(), 1);
        assert!(targeted.contains("new"));
        assert!(!targeted.contains("old"));
        assert!(!targeted.contains("browser-client"));

        let untargeted = phone_action_delivery_client_ids(None).await;
        assert_eq!(untargeted.len(), 3);
        assert!(untargeted.contains("new"));
        assert!(untargeted.contains("other-device"));
        assert!(untargeted.contains("legacy"));
        assert!(!untargeted.contains("old"));
        assert!(!untargeted.contains("browser-client"));
        assert!(!untargeted.contains("unknown-legacy"));

        WS_CLIENT_REGISTRY.write().await.clear();
    }

    #[test]
    fn phone_action_result_entry_parses_payload() {
        let message = BridgeMessage {
            message_type: "phone_action_result".to_string(),
            payload: serde_json::json!({
                "id": "action-1",
                "status": "success",
                "message": "clipboard updated",
            }),
        };

        let entry = phone_action_result_entry_from_message(
            &message,
            Some("client-1".to_string()),
            Some("iphone-1".to_string()),
        )
        .expect("phone action result");

        assert_eq!(entry.id, "action-1");
        assert_eq!(entry.status, "success");
        assert_eq!(entry.message.as_deref(), Some("clipboard updated"));
        assert_eq!(entry.source_client_id.as_deref(), Some("client-1"));
        assert_eq!(entry.source_device_id.as_deref(), Some("iphone-1"));
        assert!(!entry.received_at.is_empty());
    }

    #[test]
    fn phone_action_results_prune_expired_entries() {
        let now = chrono::Utc::now();
        let expired_at =
            (now - chrono::Duration::seconds(PHONE_ACTION_RESULT_TTL_SECS + 1)).to_rfc3339();
        let fresh_at = now.to_rfc3339();
        let mut results = HashMap::from([
            (
                "expired".to_string(),
                PhoneActionResultEntry {
                    id: "expired".to_string(),
                    status: "success".to_string(),
                    message: None,
                    received_at: expired_at,
                    source_client_id: None,
                    source_device_id: None,
                },
            ),
            (
                "fresh".to_string(),
                PhoneActionResultEntry {
                    id: "fresh".to_string(),
                    status: "pending".to_string(),
                    message: None,
                    received_at: fresh_at,
                    source_client_id: None,
                    source_device_id: None,
                },
            ),
        ]);

        prune_phone_action_results(&mut results, now);

        assert!(!results.contains_key("expired"));
        assert!(results.contains_key("fresh"));
    }

    #[test]
    fn public_anonymous_allowlist_keeps_only_safe_mobile_auth_paths() {
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/.well-known/iterate/health"
        ));
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/pair"
        ));
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::POST,
            "/pair/challenge"
        ));
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::POST,
            "/pair/claim"
        ));
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/api/version"
        ));
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/ws/codex-live"
        ));
        assert!(!public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/api/connection-status"
        ));
        assert!(!public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/api/mobile/pairing/status"
        ));
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::POST,
            "/api/mobile/pairing/claim"
        ));

        assert!(!public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/api/mobile/pairing"
        ));
        assert!(!public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/api/active-sessions"
        ));
        assert!(!public_anonymous_path_allowed(
            &axum::http::Method::POST,
            "/api/room-submit"
        ));
        assert!(body_bound_auth_deferred(
            &axum::http::Method::POST,
            "/api/room-submit"
        ));
        assert!(!body_bound_auth_deferred(
            &axum::http::Method::GET,
            "/api/room-submit"
        ));
        // The socket upgrade is public so a newly paired mobile client can reach
        // the bridge. Authentication is enforced by the WebSocket handshake and
        // only an authenticated, matching device may complete pairing.
        assert!(public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/ws"
        ));
        assert!(!public_anonymous_path_allowed(
            &axum::http::Method::GET,
            "/image"
        ));
    }

    #[test]
    fn websocket_origin_accepts_exact_bridge_origin_and_rejects_sibling() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("iterate.example.com"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://iterate.example.com"),
        );
        assert!(browser_websocket_origin_is_allowed(&headers, None, true));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://sibling.tobooks.xin"),
        );
        assert!(!browser_websocket_origin_is_allowed(&headers, None, true));

        let console_origins = vec!["https://sibling.tobooks.xin".to_string()];
        assert!(browser_websocket_origin_is_allowed(
            &headers,
            Some(&console_origins),
            true,
        ));

        headers.remove(header::ORIGIN);
        assert!(browser_websocket_origin_is_allowed(&headers, None, true));
        assert!(!browser_websocket_origin_is_allowed(
            &headers,
            Some(&console_origins),
            true,
        ));
    }

    #[test]
    fn cookie_http_origin_requires_an_exact_paired_console_origin() {
        let allowed = vec![
            "https://iterate.example.com".to_string(),
            "https://console.tobooks.xin".to_string(),
        ];
        let mut headers = HeaderMap::new();
        assert!(!browser_http_origin_is_allowed(&headers, &allowed));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://console.tobooks.xin"),
        );
        assert!(browser_http_origin_is_allowed(&headers, &allowed));

        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.tobooks.xin"),
        );
        assert!(!browser_http_origin_is_allowed(&headers, &allowed));
    }

    #[test]
    fn production_cors_excludes_vite_origins_but_keeps_configured_console() {
        let production =
            bridge_cors_origin_strings(Some("https://console.tobooks.xin".to_string()), false);
        assert!(production.contains(&"https://console.tobooks.xin".to_string()));
        assert!(!production.iter().any(|origin| origin.contains(":5173")));
        assert!(!production.iter().any(|origin| origin.contains(":5174")));

        let development = bridge_cors_origin_strings(None, true);
        assert!(development.contains(&"http://127.0.0.1:5174".to_string()));
    }

    async fn response_json(response: axum::response::Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body bytes");
        serde_json::from_slice(&bytes).expect("json response")
    }

    fn web_login_origin_headers(origin: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, HeaderValue::from_str(origin).unwrap());
        headers
    }

    #[tokio::test]
    async fn web_login_pair_claim_issues_replay_safe_web_session() {
        let _guard = WEB_LOGIN_TEST_LOCK.lock().await;
        WEB_LOGIN_PAIRING_NONCES.write().await.clear();
        WEB_LOGIN_SESSIONS.write().await.clear();

        let issue = issue_cloudflare_web_login_pairing(
            "https://iterate.example.com".to_string(),
            "https://app.iterate.example.com".to_string(),
        )
        .await
        .expect("issue web pairing");
        assert!(issue
            .pair_url
            .starts_with("https://app.iterate.example.com/pair?"));
        assert!(issue.pair_url.contains("nonce="));
        assert!(issue.pair_url.contains("cf_origin="));

        let claim = WebLoginPairClaimRequest {
            nonce: issue.nonce.clone(),
            device_id: issue.device_id.clone(),
            cf_origin: issue.cf_origin.clone(),
            requested_scopes: vec![
                SCOPE_STATUS_READ.to_string(),
                SCOPE_SESSION_READ.to_string(),
            ],
        };
        let response =
            handle_pair_claim(web_login_origin_headers(&issue.console_origin), Json(claim)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let set_cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("HttpOnly web session cookie")
            .to_string();
        assert!(set_cookie.contains("HttpOnly"));
        assert!(set_cookie.contains("SameSite=Strict"));
        let body = response_json(response).await;
        assert!(body.get("session_token").is_none());
        assert_eq!(body["device_id"].as_str(), Some(issue.device_id.as_str()));

        let replay = WebLoginPairClaimRequest {
            nonce: issue.nonce.clone(),
            device_id: issue.device_id.clone(),
            cf_origin: issue.cf_origin.clone(),
            requested_scopes: Vec::new(),
        };
        let replay_response = handle_pair_claim(
            web_login_origin_headers(&issue.console_origin),
            Json(replay),
        )
        .await;
        assert_eq!(replay_response.status(), StatusCode::UNAUTHORIZED);

        let mut web_headers = HeaderMap::new();
        web_headers.insert(
            header::COOKIE,
            HeaderValue::from_str(set_cookie.split(';').next().expect("cookie pair")).unwrap(),
        );
        web_headers.insert(
            "x-iterate-device-id",
            HeaderValue::from_str(&issue.device_id).unwrap(),
        );
        let principal = super::authenticate_bridge_headers(&web_headers)
            .await
            .expect("web session authenticates");
        assert_eq!(principal.client_kind, "web");
        assert!(principal.has_scope(SCOPE_STATUS_READ));
        assert!(!principal.has_scope(SCOPE_FILE_LIST));

        let refresh = handle_session_refresh(web_headers.clone()).await;
        assert_eq!(refresh.status(), StatusCode::OK);
        let refreshed_cookie = refresh
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("refresh renews browser cookie");
        assert!(refreshed_cookie.contains("Max-Age="));
        let sessions = list_web_login_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].device_id, issue.device_id);
        let revoke = handle_session_revoke(web_headers.clone()).await;
        assert_eq!(revoke.status(), StatusCode::OK);
        let cleared_cookie = revoke
            .headers()
            .get(header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .expect("revoke clears browser cookie");
        assert!(cleared_cookie.contains("Max-Age=0"));
        assert!(super::authenticate_bridge_headers(&web_headers)
            .await
            .is_none());
    }

    #[tokio::test]
    async fn web_login_pair_claim_rejects_missing_or_mismatched_origin() {
        let _guard = WEB_LOGIN_TEST_LOCK.lock().await;
        WEB_LOGIN_PAIRING_NONCES.write().await.clear();
        WEB_LOGIN_SESSIONS.write().await.clear();

        let issue = issue_cloudflare_web_login_pairing(
            "https://iterate.example.com".to_string(),
            "https://app.iterate.example.com".to_string(),
        )
        .await
        .expect("issue web pairing");
        let claim = WebLoginPairClaimRequest {
            nonce: issue.nonce.clone(),
            device_id: issue.device_id.clone(),
            cf_origin: issue.cf_origin.clone(),
            requested_scopes: Vec::new(),
        };
        let missing_origin = handle_pair_claim(HeaderMap::new(), Json(claim)).await;
        assert_eq!(missing_origin.status(), StatusCode::BAD_REQUEST);

        let issue = issue_cloudflare_web_login_pairing(
            "https://iterate.example.com".to_string(),
            "https://app.iterate.example.com".to_string(),
        )
        .await
        .expect("issue web pairing");
        let claim = WebLoginPairClaimRequest {
            nonce: issue.nonce.clone(),
            device_id: issue.device_id.clone(),
            cf_origin: issue.cf_origin.clone(),
            requested_scopes: Vec::new(),
        };
        let wrong_origin = handle_pair_claim(
            web_login_origin_headers("https://evil.example.com"),
            Json(claim),
        )
        .await;
        assert_eq!(wrong_origin.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn mobile_device_token_cannot_refresh_web_session() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer dt_not_a_web_session"),
        );
        let response = handle_session_refresh(headers).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        let body = response_json(response).await;
        assert_eq!(body["error"].as_str(), Some("invalid_web_session"));
    }

    #[tokio::test]
    async fn web_login_sessions_can_be_revoked_all_without_exposing_tokens() {
        let _guard = WEB_LOGIN_TEST_LOCK.lock().await;
        WEB_LOGIN_PAIRING_NONCES.write().await.clear();
        WEB_LOGIN_SESSIONS.write().await.clear();

        let issue = issue_cloudflare_web_login_pairing(
            "https://iterate.example.com".to_string(),
            "https://app.iterate.example.com".to_string(),
        )
        .await
        .expect("issue web pairing");
        let claim = WebLoginPairClaimRequest {
            nonce: issue.nonce.clone(),
            device_id: issue.device_id.clone(),
            cf_origin: issue.cf_origin.clone(),
            requested_scopes: Vec::new(),
        };
        let response =
            handle_pair_claim(web_login_origin_headers(&issue.console_origin), Json(claim)).await;
        assert_eq!(response.status(), StatusCode::OK);

        let sessions = list_web_login_sessions().await;
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].cf_origin, "https://iterate.example.com");

        assert_eq!(revoke_all_web_login_sessions().await, 1);
        assert!(list_web_login_sessions().await.is_empty());
    }

    #[test]
    fn websocket_probe_auth_required_counts_as_healthy_only_when_mobile_auth_is_enforced() {
        let unauthorized_probe = serde_json::json!({
            "upgrade_ok": false,
            "status_code": 401,
            "error_code": "http_status",
            "error": "http_status:401",
        });
        let forbidden_probe = serde_json::json!({
            "upgrade_ok": false,
            "status_code": 403,
            "error_code": "http_status",
            "error": "http_status:403",
        });
        let unavailable_probe = serde_json::json!({
            "upgrade_ok": false,
            "status_code": 502,
            "error_code": "http_status",
            "error": "http_status:502",
        });

        assert!(websocket_probe_auth_required(&unauthorized_probe));
        assert!(websocket_probe_auth_required(&forbidden_probe));
        assert!(!websocket_probe_auth_required(&unavailable_probe));
        assert!(websocket_probe_ok_or_auth_required(
            false,
            &unauthorized_probe,
            true
        ));
        assert!(!websocket_probe_ok_or_auth_required(
            false,
            &unauthorized_probe,
            false
        ));
        assert!(!websocket_probe_ok_or_auth_required(
            false,
            &unavailable_probe,
            true
        ));
    }

    #[test]
    fn root_tunnel_authority_rejects_stale_status_ha_without_live_metrics() {
        let stale_status_only = serde_json::json!({
            "status_fresh": false,
            "metrics": {
                "http_ok": true,
                "ha_connection_count": 0.0,
                "status_ha_connection_count": 4.0,
                "expected_ha_connections": 4.0,
            },
            "derived": {
                "child_alive": true,
            }
        });
        let fresh_status_fallback = serde_json::json!({
            "status_fresh": true,
            "metrics": {
                "http_ok": true,
                "ha_connection_count": 0.0,
                "status_ha_connection_count": 4.0,
                "expected_ha_connections": 4.0,
            },
            "derived": {
                "child_alive": true,
            }
        });
        let live_metrics_ready = serde_json::json!({
            "status_fresh": false,
            "metrics": {
                "http_ok": true,
                "ha_connection_count": 4.0,
                "status_ha_connection_count": 0.0,
                "expected_ha_connections": 4.0,
            }
        });

        assert!(!root_tunnel_is_authoritative_up(&stale_status_only));
        assert!(root_tunnel_is_authoritative_up(&fresh_status_fallback));
        assert!(root_tunnel_is_authoritative_up(&live_metrics_ready));
    }

    #[test]
    fn root_tunnel_authority_rejects_structural_block_even_with_ready_ha() {
        let structurally_blocked = serde_json::json!({
            "status_fresh": true,
            "metrics": {
                "http_ok": true,
                "ha_connection_count": 4.0,
                "status_ha_connection_count": 4.0,
                "expected_ha_connections": 4.0,
            },
            "derived": {
                "child_alive": true,
                "structural_block": true,
                "edge_7844_suspected": true,
                "tunnel_health_class": "needs_edge_path_fix",
            }
        });

        assert!(!root_tunnel_is_authoritative_up(&structurally_blocked));
    }

    #[test]
    fn root_tunnel_supervisor_fields_from_status_exposes_a_package_diagnostics() {
        let status = serde_json::json!({
            "tunnel_health_class": "needs_edge_path_fix",
            "last_skip_reason": "structural_block edge_7844_suspected",
            "structural_block": true,
            "edge_7844_suspected": true,
            "edge_7844_probe_ok": false,
            "edge_7844_checked_at": "2026-06-14T06:30:00Z",
            "edge_7844_failure_reason": "curl_28_SSL_timeout",
            "edge_7844_last_url": "https://region1.v2.argotunnel.com:7844",
            "escalation_count_hour": 2,
            "max_escalations_per_hour": 3,
            "next_action_at": 1_700_000_000_i64,
            "observe_only_until": 1_700_000_900_i64,
            "backoff_remaining_secs": 120,
        });
        let derived = root_tunnel_supervisor_fields_from_status(Some(&status));

        assert_eq!(
            derived["tunnel_health_class"].as_str(),
            Some("needs_edge_path_fix")
        );
        assert_eq!(
            derived["last_skip_reason"].as_str(),
            Some("structural_block edge_7844_suspected")
        );
        assert_eq!(derived["structural_block"].as_bool(), Some(true));
        assert_eq!(derived["edge_7844_suspected"].as_bool(), Some(true));
        assert_eq!(derived["edge_7844_probe_ok"].as_bool(), Some(false));
        assert_eq!(
            derived["edge_7844_checked_at"].as_str(),
            Some("2026-06-14T06:30:00Z")
        );
        assert_eq!(
            derived["edge_7844_failure_reason"].as_str(),
            Some("curl_28_SSL_timeout")
        );
        assert_eq!(
            derived["edge_7844_last_url"].as_str(),
            Some("https://region1.v2.argotunnel.com:7844")
        );
        assert_eq!(derived["escalation_count_hour"].as_i64(), Some(2));
        assert_eq!(derived["max_escalations_per_hour"].as_i64(), Some(3));
        assert_eq!(derived["next_action_at"].as_i64(), Some(1_700_000_000));
        assert_eq!(derived["observe_only_until"].as_i64(), Some(1_700_000_900));
        assert_eq!(derived["backoff_active"].as_bool(), Some(true));
        assert_eq!(derived["backoff_remaining_secs"].as_i64(), Some(120));
    }

    #[tokio::test]
    async fn public_probe_snapshot_returns_missing_cache_without_inline_probe() {
        let _guard = PUBLIC_PROBE_TEST_LOCK.lock().await;
        *PUBLIC_PROBE_CACHE.write().await = None;
        PUBLIC_PROBE_REFRESH_IN_FLIGHT.store(true, Ordering::SeqCst);

        let started = Instant::now();
        let (http_probe, http_ok, ws_probe, ws_ok) = get_public_probe_snapshot().await;

        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(!http_ok);
        assert!(!ws_ok);
        assert_eq!(
            http_probe
                .get("error_code")
                .and_then(|value| value.as_str()),
            Some("cache_missing")
        );
        assert_eq!(
            ws_probe.get("error_code").and_then(|value| value.as_str()),
            Some("cache_missing")
        );

        PUBLIC_PROBE_REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
    }

    #[tokio::test]
    async fn public_probe_snapshot_does_not_treat_stale_cache_as_healthy() {
        let _guard = PUBLIC_PROBE_TEST_LOCK.lock().await;
        PUBLIC_PROBE_REFRESH_IN_FLIGHT.store(true, Ordering::SeqCst);
        *PUBLIC_PROBE_CACHE.write().await = Some(CachedPublicProbe {
            http_value: serde_json::json!({
                "url": "https://iterate.example.com/api/version",
                "healthy": true,
                "status_code": 200,
            }),
            http_ok: true,
            ws_value: serde_json::json!({
                "url": "https://iterate.example.com/ws",
                "upgrade_ok": true,
                "status_code": 101,
            }),
            ws_ok: true,
            refreshed_at: Instant::now()
                .checked_sub(Duration::from_secs(PUBLIC_PROBE_CACHE_MAX_AGE_SECS + 1))
                .unwrap(),
        });

        let started = Instant::now();
        let (http_probe, http_ok, ws_probe, ws_ok) = get_public_probe_snapshot().await;

        assert!(started.elapsed() < Duration::from_millis(100));
        assert!(!http_ok);
        assert!(!ws_ok);
        assert_eq!(
            http_probe
                .get("cache_stale")
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            ws_probe
                .get("cache_stale")
                .and_then(|value| value.as_bool()),
            Some(true)
        );

        *PUBLIC_PROBE_CACHE.write().await = None;
        PUBLIC_PROBE_REFRESH_IN_FLIGHT.store(false, Ordering::SeqCst);
    }

    #[test]
    fn websocket_auth_guard_is_request_scoped_and_platform_independent() {
        let principal = AuthPrincipal {
            principal_id: "device:test".to_string(),
            device_id: "test".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_STATUS_READ.to_string()],
        };

        assert!(websocket_auth_denial(false, None).is_none());
        assert_eq!(
            websocket_auth_denial(true, None),
            Some((StatusCode::UNAUTHORIZED, "mobile_auth_required"))
        );
        assert!(websocket_auth_denial(true, Some(&principal)).is_none());
        assert!(!websocket_scope_enforced(false));
        assert!(websocket_scope_enforced(true));
    }

    #[test]
    fn bridge_auth_is_required_for_forwarded_or_non_loopback_requests_on_every_platform() {
        let local_peer = SocketAddr::from(([127, 0, 0, 1], 45678));
        let remote_peer = SocketAddr::from(([192, 168, 1, 50], 45678));
        let mut public_headers = HeaderMap::new();
        public_headers.insert("cf-ray", HeaderValue::from_static("test-ray"));

        assert!(bridge_auth_required_for_request(
            &HeaderMap::new(),
            remote_peer
        ));
        assert!(bridge_auth_required_for_request(
            &public_headers,
            local_peer
        ));
        assert_eq!(
            bridge_auth_required_for_request(&HeaderMap::new(), local_peer),
            mobile_auth_required()
        );
    }

    #[test]
    fn websocket_device_token_can_be_carried_by_browser_subprotocol() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static("iterate.mobile.v1, iterate.device-token.dt_test-token_1"),
        );

        assert_eq!(
            websocket_device_token_from_protocols(&headers).as_deref(),
            Some("dt_test-token_1")
        );
    }

    #[test]
    fn websocket_desktop_token_uses_a_distinct_subprotocol_namespace() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            HeaderValue::from_static(
                "iterate.codex-live.v1, iterate.desktop-token.ibi1.payload.signature",
            ),
        );

        assert_eq!(
            websocket_desktop_token_from_protocols(&headers).as_deref(),
            Some("ibi1.payload.signature")
        );
        assert!(websocket_device_token_from_protocols(&headers).is_none());
    }

    #[test]
    fn websocket_device_token_can_be_carried_by_query_for_public_tunnels() {
        let uri: axum::http::Uri = "/ws?transport=public_tunnel&token=dt_query-token_1"
            .parse()
            .unwrap();

        assert_eq!(
            websocket_device_token_from_uri(&uri).as_deref(),
            Some("dt_query-token_1")
        );
    }

    #[test]
    fn websocket_device_token_can_be_carried_by_client_hello_payload() {
        let message = BridgeMessage {
            message_type: "client_hello".to_string(),
            payload: serde_json::json!({
                "device_id": "android-web-test",
                "device_token": "dt_message-token_1",
            }),
        };

        assert_eq!(
            websocket_device_token_from_message(&message).as_deref(),
            Some("dt_message-token_1")
        );
        assert_eq!(
            websocket_device_id_from_message(&message).as_deref(),
            Some("android-web-test")
        );
    }

    #[test]
    fn bridge_message_log_redacts_device_tokens() {
        let redacted = redact_bridge_message_text(
            r#"{"message_type":"client_hello","payload":{"device_token":"dt_secret","deviceToken":"dt_camel","token":"dt_plain","device_id":"android-web-test"}}"#,
        );

        assert!(!redacted.contains("dt_secret"));
        assert!(!redacted.contains("dt_camel"));
        assert!(!redacted.contains("dt_plain"));
        assert!(redacted.contains("\"device_token\":\"[redacted]\""));
        assert!(redacted.contains("\"deviceToken\":\"[redacted]\""));
        assert!(redacted.contains("\"token\":\"[redacted]\""));
        assert!(redacted.contains("android-web-test"));
    }

    #[tokio::test]
    async fn direct_network_clients_must_authenticate_for_scoped_bridge_routes() {
        let remote_peer = SocketAddr::from(([192, 168, 1, 50], 45678));
        let local_peer = SocketAddr::from(([127, 0, 0, 1], 45678));

        assert!(direct_network_peer_requires_auth(remote_peer));
        assert!(!direct_network_peer_requires_auth(local_peer));
        assert!(
            direct_network_bridge_auth_denial(&HeaderMap::new(), "/ws", remote_peer)
                .await
                .is_none()
        );

        let response = direct_network_bridge_auth_denial(&HeaderMap::new(), "/image", remote_peer)
            .await
            .expect("direct LAN image reads must require bridge auth");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        let response =
            direct_network_bridge_auth_denial(&HeaderMap::new(), "/api/prevent-sleep", remote_peer)
                .await
                .expect("direct LAN prevent-sleep control must require bridge auth");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        assert!(direct_network_bridge_auth_denial(
            &HeaderMap::new(),
            "/pair/challenge",
            remote_peer
        )
        .await
        .is_none());
        assert!(
            direct_network_bridge_auth_denial(&HeaderMap::new(), "/image", local_peer)
                .await
                .is_none()
        );
        assert!(direct_network_bridge_auth_denial(
            &HeaderMap::new(),
            "/api/prevent-sleep",
            local_peer
        )
        .await
        .is_none());
    }

    #[test]
    fn bridge_token_hash_never_matches_the_raw_token_by_accident() {
        let hash = bridge_token_hash("dt_secret");
        assert!(hash.starts_with("sha256:"));
        assert!(bridge_token_hash_matches("dt_secret", &hash));
        assert!(!bridge_token_hash_matches("dt_other", &hash));
        assert!(!bridge_token_hash_matches("dt_secret", "dt_secret"));
    }

    #[test]
    fn paired_ios_guard_denies_dangerous_remote_actions() {
        let principal = AuthPrincipal {
            principal_id: "device:test".to_string(),
            device_id: "test".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![
                SCOPE_SESSION_RESPOND.to_string(),
                SCOPE_WINDOW_SHOW.to_string(),
            ],
        };

        let submit = BridgeMessage {
            message_type: "mcp_action".to_string(),
            payload: serde_json::json!({ "action": "submit" }),
        };
        assert!(remote_action_denial_reason(&principal, &submit).is_none());

        let goal = BridgeMessage {
            message_type: "mcp_action".to_string(),
            payload: serde_json::json!({ "action": "goal" }),
        };
        assert!(remote_action_denial_reason(&principal, &goal).is_none());

        let config_write = BridgeMessage {
            message_type: "mcp_action".to_string(),
            payload: serde_json::json!({ "action": "update_custom_prompt_order" }),
        };
        assert!(remote_action_denial_reason(&principal, &config_write).is_some());

        let prevent_sleep = BridgeMessage {
            message_type: "system_command".to_string(),
            payload: serde_json::json!({ "command": "toggle_prevent_sleep" }),
        };
        assert!(remote_action_denial_reason(&principal, &prevent_sleep).is_none());

        let show_window = BridgeMessage {
            message_type: "system_command".to_string(),
            payload: serde_json::json!({ "command": "show_main_window" }),
        };
        assert!(remote_action_denial_reason(&principal, &show_window).is_none());

        let phone_action_result = BridgeMessage {
            message_type: "phone_action_result".to_string(),
            payload: serde_json::json!({ "id": "action-1", "status": "completed" }),
        };
        assert!(remote_action_denial_reason(&principal, &phone_action_result).is_none());
        let read_only_principal = AuthPrincipal {
            scopes: vec![SCOPE_SESSION_READ.to_string()],
            ..principal.clone()
        };
        assert_eq!(
            remote_action_denial_reason(&read_only_principal, &phone_action_result),
            Some(format!("missing scope {}", SCOPE_SESSION_RESPOND))
        );

        let unknown_action = BridgeMessage {
            message_type: "mcp_action".to_string(),
            payload: serde_json::json!({ "action": "future_admin_action" }),
        };
        assert_eq!(
            remote_action_denial_reason(&principal, &unknown_action),
            Some("unknown mcp action denied: future_admin_action".to_string())
        );

        let unknown_message = BridgeMessage {
            message_type: "future_remote_message".to_string(),
            payload: serde_json::json!({}),
        };
        assert_eq!(
            remote_action_denial_reason(&principal, &unknown_message),
            Some("unknown remote message denied: future_remote_message".to_string())
        );
    }

    #[test]
    fn main_page_ws_guard_requires_tab_specific_read_scopes() {
        let base_principal = AuthPrincipal {
            principal_id: "device:test".to_string(),
            device_id: "test".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_SESSION_READ.to_string()],
        };
        let prompt_principal = AuthPrincipal {
            scopes: vec![SCOPE_PROMPT_LIBRARY_READ.to_string()],
            ..base_principal.clone()
        };
        let config_principal = AuthPrincipal {
            scopes: vec![SCOPE_CONFIG_READ.to_string()],
            ..base_principal.clone()
        };
        let intro = BridgeMessage {
            message_type: "request_main_page".to_string(),
            payload: serde_json::json!({ "tab": "intro" }),
        };
        let prompts = BridgeMessage {
            message_type: "request_main_page".to_string(),
            payload: serde_json::json!({ "tab": "prompts" }),
        };
        let tools = BridgeMessage {
            message_type: "request_main_page".to_string(),
            payload: serde_json::json!({ "tab": "tools" }),
        };
        let unknown = BridgeMessage {
            message_type: "request_main_page".to_string(),
            payload: serde_json::json!({ "tab": "admin" }),
        };

        assert!(remote_action_denial_reason(&base_principal, &intro).is_none());
        assert_eq!(
            remote_action_denial_reason(&base_principal, &prompts),
            Some(format!("missing scope {}", SCOPE_PROMPT_LIBRARY_READ))
        );
        assert!(remote_action_denial_reason(&prompt_principal, &prompts).is_none());
        assert_eq!(
            remote_action_denial_reason(&base_principal, &tools),
            Some(format!("missing scope {}", SCOPE_CONFIG_READ))
        );
        assert!(remote_action_denial_reason(&config_principal, &tools).is_none());
        assert_eq!(
            remote_action_denial_reason(&base_principal, &unknown),
            Some("unknown main page tab denied: admin".to_string())
        );
    }

    #[test]
    fn paired_ios_scope_set_allows_ghost_suggestion_writeback() {
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_CONFIG_READ));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_CONFIG_WRITE));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_PROMPT_LIBRARY_READ));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_PROMPT_LIBRARY_WRITE));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_GHOST_SUGGESTIONS_READ));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_PHONE_ACTION_JOB_READ));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_TUNNEL_RECOVER));
        assert!(!mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_SERVICE_RECOVER));
        assert!(!mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_GHOST_SUGGESTIONS_WRITE));
        assert!(mobile_device_scopes(true)
            .iter()
            .any(|scope| scope == SCOPE_GHOST_SUGGESTIONS_WRITE));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_NOTIFICATION_SUBSCRIBE));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_SPEECH_MEMORY_READ));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_SPEECH_MEMORY_WRITE));
        assert!(!mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_NOTIFICATION_SEND));
        assert!(!mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_BRIDGE_PUBLISH));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_PAIRING_ISSUE));
        assert!(mobile_device_scopes(false)
            .iter()
            .any(|scope| scope == SCOPE_FILE_LIST));
    }

    #[test]
    fn legacy_ios_paired_devices_receive_current_mobile_scopes() {
        let mut store = PairedDeviceStore {
            devices: vec![PairedDeviceRecord {
                device_id: "legacy-ios".to_string(),
                device_name: "iPhone".to_string(),
                client_kind: "ios".to_string(),
                token_hash: "sha256:test".to_string(),
                scopes: vec![
                    SCOPE_STATUS_READ.to_string(),
                    SCOPE_SESSION_READ.to_string(),
                    SCOPE_SESSION_RESPOND.to_string(),
                    SCOPE_WINDOW_SHOW.to_string(),
                    SCOPE_GHOST_SUGGESTIONS_WRITE.to_string(),
                    "speech_text.refine".to_string(),
                ],
                created_at: "2026-01-01T00:00:00Z".to_string(),
                last_seen_at: "2026-01-01T00:00:00Z".to_string(),
                file_browser_roots: Vec::new(),
                revoked_at: None,
            }],
        };

        assert!(normalize_paired_device_store(&mut store));

        let scopes = &store.devices[0].scopes;
        assert!(scopes.iter().any(|scope| scope == SCOPE_CONFIG_READ));
        assert!(scopes.iter().any(|scope| scope == SCOPE_CONFIG_WRITE));
        assert!(scopes
            .iter()
            .any(|scope| scope == SCOPE_PROMPT_LIBRARY_READ));
        assert!(scopes
            .iter()
            .any(|scope| scope == SCOPE_PROMPT_LIBRARY_WRITE));
        assert!(scopes
            .iter()
            .any(|scope| scope == SCOPE_GHOST_SUGGESTIONS_READ));
        assert!(scopes
            .iter()
            .any(|scope| scope == SCOPE_PHONE_ACTION_JOB_READ));
        assert!(scopes.iter().any(|scope| scope == SCOPE_TUNNEL_RECOVER));
        assert!(scopes.iter().any(|scope| scope == SCOPE_FILE_LIST));
        assert!(!scopes.iter().any(|scope| scope == SCOPE_SERVICE_RECOVER));
        assert!(scopes
            .iter()
            .any(|scope| scope == SCOPE_GHOST_SUGGESTIONS_WRITE));
        assert!(scopes.iter().any(|scope| scope == "speech_text.refine"));
    }

    #[test]
    fn custom_prompts_injection_replaces_null_payload_value() {
        let mut payload = serde_json::json!({
            "customPrompts": null,
            "request": {
                "project_path": "/tmp/project"
            }
        });
        let custom_prompts = serde_json::json!({
            "enabled": true,
            "prompts": [
                {
                    "id": "normal-template",
                    "type": "normal",
                    "name": "Normal"
                },
                {
                    "id": "conditional-template",
                    "type": "conditional",
                    "name": "Conditional"
                }
            ]
        });

        ensure_custom_prompts_value_in_mcp_state(&mut payload, custom_prompts);

        let prompts = payload["customPrompts"]["prompts"]
            .as_array()
            .expect("prompts array");
        assert_eq!(prompts.len(), 2);
        assert_eq!(prompts[1]["type"], "conditional");
    }

    #[test]
    fn ghost_suggestion_read_scope_guard_requires_authenticated_scope_when_remote() {
        let read_only_principal = AuthPrincipal {
            principal_id: "device:read-only".to_string(),
            device_id: "read-only".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_SESSION_RESPOND.to_string()],
        };
        let read_principal = AuthPrincipal {
            scopes: vec![SCOPE_GHOST_SUGGESTIONS_READ.to_string()],
            ..read_only_principal.clone()
        };

        assert_eq!(
            scoped_public_route_denial(
                None,
                true,
                SCOPE_GHOST_SUGGESTIONS_READ,
                "missing_scope_ghost_suggestions_read"
            ),
            Some((StatusCode::UNAUTHORIZED, "invalid_device_auth"))
        );
        assert_eq!(
            scoped_public_route_denial(
                Some(&read_only_principal),
                true,
                SCOPE_GHOST_SUGGESTIONS_READ,
                "missing_scope_ghost_suggestions_read"
            ),
            Some((
                StatusCode::FORBIDDEN,
                "missing_scope_ghost_suggestions_read"
            ))
        );
        assert!(scoped_public_route_denial(
            Some(&read_principal),
            true,
            SCOPE_GHOST_SUGGESTIONS_READ,
            "missing_scope_ghost_suggestions_read"
        )
        .is_none());
    }

    #[test]
    fn ghost_suggestion_write_scope_guard_requires_authenticated_scope_when_remote() {
        let read_only_principal = AuthPrincipal {
            principal_id: "device:read-only".to_string(),
            device_id: "read-only".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_SESSION_RESPOND.to_string()],
        };
        let write_principal = AuthPrincipal {
            scopes: vec![SCOPE_GHOST_SUGGESTIONS_WRITE.to_string()],
            ..read_only_principal.clone()
        };

        assert!(ghost_suggestions_write_scope_denial(None, false).is_none());
        assert_eq!(
            ghost_suggestions_write_scope_denial(None, true),
            Some((StatusCode::UNAUTHORIZED, "invalid_device_auth"))
        );
        assert_eq!(
            ghost_suggestions_write_scope_denial(Some(&read_only_principal), true),
            Some((
                StatusCode::FORBIDDEN,
                "missing_scope_ghost_suggestions_write"
            ))
        );
        assert!(ghost_suggestions_write_scope_denial(Some(&write_principal), true).is_none());
    }

    #[test]
    fn public_route_scope_guard_requires_specific_scope_when_remote() {
        let base_principal = AuthPrincipal {
            principal_id: "device:read-only".to_string(),
            device_id: "read-only".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_SESSION_RESPOND.to_string()],
        };
        let config_principal = AuthPrincipal {
            scopes: vec![SCOPE_CONFIG_WRITE.to_string()],
            ..base_principal.clone()
        };
        let legacy_ios_principal = AuthPrincipal {
            scopes: vec![SCOPE_SESSION_READ.to_string()],
            ..base_principal.clone()
        };

        assert!(scoped_public_route_denial(
            None,
            false,
            SCOPE_CONFIG_WRITE,
            "missing_scope_config_write"
        )
        .is_none());
        assert_eq!(
            scoped_public_route_denial(
                None,
                true,
                SCOPE_CONFIG_WRITE,
                "missing_scope_config_write"
            ),
            Some((StatusCode::UNAUTHORIZED, "invalid_device_auth"))
        );
        assert_eq!(
            scoped_public_route_denial(
                Some(&base_principal),
                true,
                SCOPE_CONFIG_WRITE,
                "missing_scope_config_write"
            ),
            Some((StatusCode::FORBIDDEN, "missing_scope_config_write"))
        );
        assert!(scoped_public_route_denial(
            Some(&config_principal),
            true,
            SCOPE_CONFIG_WRITE,
            "missing_scope_config_write"
        )
        .is_none());
        assert!(scoped_public_route_denial(
            Some(&legacy_ios_principal),
            true,
            SCOPE_NOTIFICATION_SUBSCRIBE,
            "missing_scope_notification_subscribe"
        )
        .is_none());
        assert_eq!(
            scoped_public_route_denial(
                Some(&legacy_ios_principal),
                true,
                SCOPE_NOTIFICATION_SEND,
                "missing_scope_notification_send"
            ),
            Some((StatusCode::FORBIDDEN, "missing_scope_notification_send"))
        );
    }

    #[test]
    fn restart_tunnel_scope_accepts_tunnel_recover_without_service_recover() {
        let tunnel_principal = AuthPrincipal {
            principal_id: "device:tunnel".to_string(),
            device_id: "tunnel".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_TUNNEL_RECOVER.to_string()],
        };

        assert!(principal_has_any_scope(
            &tunnel_principal,
            &[SCOPE_TUNNEL_RECOVER, SCOPE_SERVICE_RECOVER]
        ));
        assert_eq!(
            scoped_public_route_denial(
                Some(&tunnel_principal),
                true,
                SCOPE_SERVICE_RECOVER,
                "missing_scope_service_recover"
            ),
            Some((StatusCode::FORBIDDEN, "missing_scope_service_recover"))
        );
    }

    #[test]
    fn recovery_transport_header_is_sanitized() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-iterate-recovery-transport",
            HeaderValue::from_static(" tailscale; rm -rf / "),
        );

        assert_eq!(recovery_transport_from_headers(&headers), "tailscalerm-rf");
        assert_eq!(
            sanitize_recovery_transport("public_tunnel"),
            "public_tunnel"
        );
        assert_eq!(
            recovery_transport_from_headers(&HeaderMap::new()),
            "unknown"
        );
    }

    #[test]
    fn status_read_full_diagnostics_require_status_read_when_authenticated_remote() {
        let base_principal = AuthPrincipal {
            principal_id: "device:limited".to_string(),
            device_id: "limited".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_SESSION_RESPOND.to_string()],
        };
        let status_principal = AuthPrincipal {
            scopes: vec![SCOPE_STATUS_READ.to_string()],
            ..base_principal.clone()
        };

        assert!(status_read_full_diagnostics_denial(None, false, false).is_none());
        assert!(status_read_full_diagnostics_denial(None, true, true).is_none());
        assert_eq!(
            status_read_full_diagnostics_denial(None, false, true),
            Some((StatusCode::UNAUTHORIZED, "invalid_device_auth"))
        );
        assert_eq!(
            status_read_full_diagnostics_denial(Some(&base_principal), true, true),
            Some((StatusCode::FORBIDDEN, "missing_scope_status_read"))
        );
        assert!(status_read_full_diagnostics_denial(Some(&status_principal), true, true).is_none());
    }

    #[test]
    fn pairing_and_file_scopes_are_not_covered_by_read_only_scopes() {
        let read_only_principal = AuthPrincipal {
            principal_id: "device:read-only".to_string(),
            device_id: "read-only".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![
                SCOPE_STATUS_READ.to_string(),
                SCOPE_SESSION_READ.to_string(),
            ],
        };
        let pairing_principal = AuthPrincipal {
            scopes: vec![SCOPE_PAIRING_ISSUE.to_string()],
            ..read_only_principal.clone()
        };
        let file_principal = AuthPrincipal {
            scopes: vec![SCOPE_FILE_LIST.to_string()],
            ..read_only_principal.clone()
        };

        assert_eq!(
            scoped_public_route_denial(
                Some(&read_only_principal),
                true,
                SCOPE_PAIRING_ISSUE,
                "missing_scope_pairing_issue"
            ),
            Some((StatusCode::FORBIDDEN, "missing_scope_pairing_issue"))
        );
        assert_eq!(
            scoped_public_route_denial(
                Some(&read_only_principal),
                true,
                SCOPE_FILE_LIST,
                "missing_scope_file_list"
            ),
            Some((StatusCode::FORBIDDEN, "missing_scope_file_list"))
        );
        assert!(scoped_public_route_denial(
            Some(&pairing_principal),
            true,
            SCOPE_PAIRING_ISSUE,
            "missing_scope_pairing_issue"
        )
        .is_none());
        assert!(scoped_public_route_denial(
            Some(&file_principal),
            true,
            SCOPE_FILE_LIST,
            "missing_scope_file_list"
        )
        .is_none());
        assert!(scoped_public_route_denial(
            Some(&read_only_principal),
            true,
            SCOPE_CONFIG_READ,
            "missing_scope_config_read"
        )
        .is_some());
    }

    #[test]
    fn file_list_depth_is_bounded() {
        assert_eq!(bounded_file_list_depth(None), 3);
        assert_eq!(bounded_file_list_depth(Some(2)), 2);
        assert_eq!(
            bounded_file_list_depth(Some(FILE_LIST_MAX_DEPTH + 100)),
            FILE_LIST_MAX_DEPTH
        );
    }

    #[test]
    fn file_list_root_guard_allows_only_known_roots() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let project_root = temp_dir.path().join("project");
        let project_child = project_root.join("src");
        let outside_root = temp_dir.path().join("outside");
        std::fs::create_dir_all(&project_child).expect("project child");
        std::fs::create_dir_all(&outside_root).expect("outside root");

        let canonical_project_child = project_child.canonicalize().expect("canonical child");
        let canonical_outside = outside_root.canonicalize().expect("canonical outside");
        let allowed_roots = vec![project_root];

        assert!(canonical_path_is_within_allowed_roots(
            &canonical_project_child,
            &allowed_roots
        ));
        assert!(!canonical_path_is_within_allowed_roots(
            &canonical_outside,
            &allowed_roots
        ));
    }

    #[test]
    fn redacted_pairing_status_response_omits_private_transport_fields() {
        let result = PairingCandidatesResult {
            primary: MobilePairingCandidate {
                transport_mode: "tailscale".to_string(),
                base_url: "http://100.64.0.1:8080".to_string(),
                ws_url: "ws://100.64.0.1:8080/ws".to_string(),
                relay_device_id: None,
                relay_pairing_token: None,
                health: "healthy".to_string(),
                disabled: false,
                warning: Some("tailscale primary".to_string()),
            },
            candidates: vec![MobilePairingCandidate {
                transport_mode: "public_tunnel".to_string(),
                base_url: "https://iterate.example.com".to_string(),
                ws_url: "wss://iterate.example.com/ws".to_string(),
                relay_device_id: None,
                relay_pairing_token: None,
                health: "healthy".to_string(),
                disabled: false,
                warning: None,
            }],
            tailscale_source: Some("cli:tailscale ip -4".to_string()),
            public_endpoint_binding: None,
        };

        let response = build_redacted_pairing_status_value(&result);
        assert!(response.get("base_url").is_none());
        assert!(response.get("ws_url").is_none());
        assert!(response.get("tailscale_source").is_none());
        assert!(response.get("candidates").is_none());
        assert!(response.get("token_count").is_none());
        assert_eq!(
            response
                .get("public_tunnel")
                .and_then(|value| value.get("healthy"))
                .and_then(|value| value.as_bool()),
            Some(true)
        );
    }

    #[test]
    fn relay_pairing_candidate_serialization_keeps_static_token_out() {
        let candidate = MobilePairingCandidate {
            transport_mode: "relay".to_string(),
            base_url: "https://relay.example.com".to_string(),
            ws_url: "wss://relay.example.com/api/devices/local%2Dmac/stream".to_string(),
            relay_device_id: Some("local-mac".to_string()),
            relay_pairing_token: None,
            health: "auth_required".to_string(),
            disabled: true,
            warning: Some(
                "Relay requires short-lived pairing before App Store release".to_string(),
            ),
        };

        let serialized = serde_json::to_string(&candidate).expect("serialize relay candidate");
        assert!(serialized.contains("\"transport_mode\":\"relay\""));
        assert!(serialized.contains("\"relay_device_id\":\"local-mac\""));
        assert!(!serialized.contains("relay_token"));
        assert!(!serialized.contains("ITERATE_RELAY_TOKEN"));
        assert!(!serialized.contains("token="));
    }

    #[test]
    fn mobile_pairing_primary_prefers_public_tunnel_over_ready_relay_candidate() {
        let tailscale = MobilePairingCandidate {
            transport_mode: "tailscale".to_string(),
            base_url: "http://100.64.0.1:8080".to_string(),
            ws_url: "ws://100.64.0.1:8080/ws".to_string(),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "healthy".to_string(),
            disabled: false,
            warning: None,
        };
        let relay = MobilePairingCandidate {
            transport_mode: "relay".to_string(),
            base_url: "https://relay.example.com".to_string(),
            ws_url: "wss://relay.example.com/api/devices/local%2Dmac/stream".to_string(),
            relay_device_id: Some("local-mac".to_string()),
            relay_pairing_token: Some("rp_short_lived".to_string()),
            health: "healthy".to_string(),
            disabled: false,
            warning: None,
        };
        let public_tunnel = MobilePairingCandidate {
            transport_mode: "public_tunnel".to_string(),
            base_url: "https://iterate.example.com".to_string(),
            ws_url: "wss://iterate.example.com/ws".to_string(),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "healthy".to_string(),
            disabled: false,
            warning: None,
        };

        let candidates = vec![tailscale, relay, public_tunnel];
        let primary = select_mobile_pairing_primary_candidate(&candidates).expect("primary");

        assert_eq!(primary.transport_mode, "public_tunnel");
        assert!(primary.relay_pairing_token.is_none());
        assert_eq!(
            mobile_pairing_primary_selection_reason(&candidates, &primary),
            "public_tunnel_precedence_over_ready_relay"
        );
    }

    #[test]
    fn verified_quick_tunnel_beats_configured_but_not_running_relay() {
        let configured_relay = MobilePairingCandidate {
            transport_mode: "relay".to_string(),
            base_url: "https://relay.example.com".to_string(),
            ws_url: "wss://relay.example.com/api/devices/local%2Dmac/stream".to_string(),
            relay_device_id: Some("local-mac".to_string()),
            relay_pairing_token: Some("rp_short_lived".to_string()),
            health: "configured".to_string(),
            disabled: false,
            warning: Some("Relay client is not running".to_string()),
        };
        let quick = MobilePairingCandidate {
            transport_mode: "cloudflare_tunnel".to_string(),
            base_url: "https://quick.trycloudflare.com".to_string(),
            ws_url: "wss://quick.trycloudflare.com/ws".to_string(),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "healthy".to_string(),
            disabled: false,
            warning: None,
        };
        let candidates = vec![configured_relay, quick];

        let primary = select_mobile_pairing_primary_candidate(&candidates).expect("primary");

        assert_eq!(primary.transport_mode, "cloudflare_tunnel");
        assert_eq!(
            mobile_pairing_primary_selection_reason(&candidates, &primary),
            "verified_quick_tunnel"
        );
    }

    #[test]
    fn qr_security_gate_rejects_lan_and_insecure_websocket_candidates() {
        let quick = MobilePairingCandidate {
            transport_mode: "cloudflare_tunnel".to_string(),
            base_url: "https://quick.trycloudflare.com".to_string(),
            ws_url: "wss://quick.trycloudflare.com/ws".to_string(),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "healthy".to_string(),
            disabled: false,
            warning: None,
        };
        assert!(mobile_pairing_candidate_is_secure_for_qr(&quick));
        assert!(!mobile_pairing_candidate_has_endpoint_proof(&quick, None));

        let binding = crate::tunnel::manager::QuickTunnelPairingBinding {
            endpoint: quick.base_url.clone(),
            install_identity: "install-test".to_string(),
            endpoint_epoch: 7,
        };
        assert!(mobile_pairing_candidate_has_endpoint_proof(
            &quick,
            Some(&binding),
        ));

        let mut insecure_ws = quick.clone();
        insecure_ws.ws_url = "ws://quick.trycloudflare.com/ws".to_string();
        assert!(!mobile_pairing_candidate_is_secure_for_qr(&insecure_ws));

        let mut lan = quick;
        lan.transport_mode = "lan_fallback".to_string();
        lan.base_url = "http://192.168.1.2:8080".to_string();
        lan.ws_url = "ws://192.168.1.2:8080/ws".to_string();
        assert!(!mobile_pairing_candidate_is_secure_for_qr(&lan));

        let configured_relay = MobilePairingCandidate {
            transport_mode: "relay".to_string(),
            base_url: "https://relay.example.com".to_string(),
            ws_url: "wss://relay.example.com/api/devices/mac/stream".to_string(),
            relay_device_id: Some("mac".to_string()),
            relay_pairing_token: Some("rp_short_lived".to_string()),
            health: "configured".to_string(),
            disabled: false,
            warning: None,
        };
        assert!(!mobile_pairing_candidate_is_ready_relay(&configured_relay));
        assert!(!mobile_pairing_candidate_has_endpoint_proof(
            &configured_relay,
            None,
        ));
    }

    #[test]
    fn mobile_pairing_primary_ignores_relay_candidate_without_pairing_token() {
        let tailscale = MobilePairingCandidate {
            transport_mode: "tailscale".to_string(),
            base_url: "http://100.64.0.1:8080".to_string(),
            ws_url: "ws://100.64.0.1:8080/ws".to_string(),
            relay_device_id: None,
            relay_pairing_token: None,
            health: "healthy".to_string(),
            disabled: false,
            warning: None,
        };
        let relay = MobilePairingCandidate {
            transport_mode: "relay".to_string(),
            base_url: "https://relay.example.com".to_string(),
            ws_url: "wss://relay.example.com/api/devices/local%2Dmac/stream".to_string(),
            relay_device_id: Some("local-mac".to_string()),
            relay_pairing_token: None,
            health: "auth_required".to_string(),
            disabled: true,
            warning: None,
        };

        let primary =
            select_mobile_pairing_primary_candidate(&[relay, tailscale]).expect("primary");

        assert_eq!(primary.transport_mode, "tailscale");
        assert!(primary.relay_pairing_token.is_none());
    }

    #[test]
    fn fallback_pairing_candidates_return_public_and_loopback_candidates() {
        let result = fallback_pairing_candidates_with_public_base_url(
            8080,
            "probe timeout".to_string(),
            "https://public.example.test".to_string(),
        );

        assert_ne!(result.primary.transport_mode, "public_tunnel");
        assert!(result.tailscale_source.is_none());
        assert!(result.candidates.iter().any(|candidate| {
            candidate.transport_mode == "public_tunnel"
                && candidate.health == "degraded"
                && candidate.disabled
                && candidate.warning.as_deref() == Some("probe timeout")
        }));
        assert!(result
            .candidates
            .iter()
            .any(|candidate| candidate.transport_mode == "loopback_fallback"
                && candidate.base_url == "http://127.0.0.1:8080"));
    }

    #[test]
    fn fallback_pairing_candidates_omit_public_candidate_without_secure_origin() {
        let result = fallback_pairing_candidates_with_public_base_url(
            8080,
            "probe timeout".to_string(),
            String::new(),
        );

        assert!(!result
            .candidates
            .iter()
            .any(|candidate| candidate.transport_mode == "public_tunnel"));
        assert_ne!(result.primary.transport_mode, "public_tunnel");
        assert!(result.candidates.iter().any(|candidate| {
            candidate.transport_mode == "loopback_fallback"
                && candidate.base_url == "http://127.0.0.1:8080"
        }));
    }

    #[test]
    fn redacted_connection_status_response_omits_private_diagnostics() {
        let response = build_redacted_connection_status_value(
            "2026-05-26T12:00:00Z",
            "ok",
            true,
            true,
            true,
            vec![serde_json::json!({
                "level": "info",
                "code": "test",
                "message": "safe public hint",
            })],
        );

        assert_eq!(
            response.get("ok").and_then(|value| value.as_bool()),
            Some(true)
        );
        assert_eq!(
            response
                .get("diagnosis")
                .and_then(|value| value.get("code"))
                .and_then(|value| value.as_str()),
            Some("ok")
        );
        assert_eq!(
            response
                .get("public_tunnel")
                .and_then(|value| value.get("websocket_auth_required"))
                .and_then(|value| value.as_bool()),
            Some(true)
        );
        for sensitive_key in [
            "local_origin",
            "mcp",
            "root_tunnel",
            "sessions",
            "caches",
            "websocket",
        ] {
            assert!(
                response.get(sensitive_key).is_none(),
                "redacted response leaked {sensitive_key}"
            );
        }
    }

    #[tokio::test]
    async fn sensitive_http_handlers_reject_anonymous_public_requests() {
        let state = test_bridge_state();
        let responses = vec![
            handle_api_config_get(State(state.clone()), public_headers()).await,
            handle_api_config_post(
                State(state.clone()),
                public_headers(),
                Json(serde_json::json!({})),
            )
            .await,
            handle_api_prompt_library_get(public_headers()).await,
            handle_api_promptor_library_get(public_headers()).await,
            handle_api_prompt_library_post(
                public_headers(),
                Json(serde_json::json!({
                    "id": "test",
                    "name": "test",
                    "content": "test",
                    "category": "test"
                })),
            )
            .await,
            handle_api_prompt_library_delete(
                public_headers(),
                Query(DeleteQuery {
                    id: Some("test".to_string()),
                    all: None,
                }),
            )
            .await,
            handle_api_import_prompts_dir(
                public_headers(),
                Json(serde_json::json!({ "path": "/tmp" })),
            )
            .await,
            handle_api_ghost_suggestions_get(public_headers()).await,
            handle_api_mobile_pairing(State(state.clone()), public_headers()).await,
            handle_api_cleanup_session(
                public_headers(),
                Json(serde_json::json!({ "request_id": "test-request" })),
            )
            .await,
            handle_api_show_window(State(state.clone()), public_headers()).await,
            handle_api_open_codex_chat(
                State(state.clone()),
                public_headers(),
                None::<Json<serde_json::Value>>,
            )
            .await,
            handle_api_test_audio(State(state.clone()), public_headers()).await,
            handle_api_restart_service(State(state.clone()), public_headers()).await,
            handle_api_restart_tunnel(State(state), public_headers()).await,
        ];

        for response in responses {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn notification_and_publish_handlers_reject_anonymous_public_requests() {
        let state = test_bridge_state();
        let responses = vec![
            handle_push_subscribe(
                public_headers(),
                Json(WebPushSubscriptionInfo::new(
                    "https://example.com/push",
                    "p256dh",
                    "auth",
                )),
            )
            .await,
            handle_push_unsubscribe(
                public_headers(),
                Json(PushUnsubscribeRequest {
                    endpoint: "https://example.com/push".to_string(),
                }),
            )
            .await,
            handle_apns_register(
                public_headers(),
                Json(ApnsRegisterRequest {
                    device_token: "test-device-token".to_string(),
                    platform: "ios".to_string(),
                    app_version: "1.0".to_string(),
                    device_id: "test-device".to_string(),
                    notifications_enabled: Some(true),
                    environment: Some("sandbox".to_string()),
                }),
            )
            .await,
            handle_apns_notify(
                public_headers(),
                Json(ApnsNotifyRequest {
                    body: "test notification".to_string(),
                    title: Some("test".to_string()),
                    project_path: Some("/tmp/test".to_string()),
                    request_id: Some("test-request".to_string()),
                    predefined_options: vec![],
                    is_markdown: true,
                    codex_thread_id: None,
                    codex_deeplink: None,
                    loop_active: true,
                    force_popup: false,
                    source: Some("test".to_string()),
                }),
            )
            .await,
            handle_bridge_publish(
                public_headers(),
                State(state),
                Json(BridgeMessage {
                    message_type: "mcp_state".to_string(),
                    payload: build_payload("test-request", "/tmp/test", "test notification"),
                }),
            )
            .await,
        ];

        for response in responses {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn read_inventory_and_pull_handlers_reject_anonymous_public_requests() {
        let state = test_bridge_state();
        let responses = vec![
            handle_get_files(
                public_headers(),
                Query(FilesQuery {
                    project_path: "/tmp".to_string(),
                    max_depth: Some(1),
                }),
            )
            .await,
            handle_get_windows(public_headers()).await,
            handle_api_mcp_tools(State(state.clone()), public_headers()).await,
            handle_api_active_sessions(public_headers()).await,
            handle_api_audio_assets(public_headers()).await,
            handle_bridge_pull_action(
                public_headers(),
                Query(PullActionQuery {
                    project_path: "/tmp/test".to_string(),
                    request_id: Some("test-request".to_string()),
                }),
            )
            .await,
            handle_serve_image(
                public_headers(),
                Query(ImageQuery {
                    path: Some("/tmp/test-image.png".to_string()),
                    id: None,
                }),
            )
            .await,
        ];

        for response in responses {
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn image_path_reads_are_rejected_for_public_requests() {
        assert!(image_path_read_denial(&HeaderMap::new()).await.is_none());
        let response = image_path_read_denial(&public_headers())
            .await
            .expect("public image path should be rejected");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn registered_image_ids_are_served_without_public_path_reads() {
        let file_path =
            std::env::temp_dir().join(format!("iterate-md-image-{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&file_path, b"test image").expect("write test image");
        clear_markdown_image_registry_for_tests();

        let encoded_path = percent_encoding::utf8_percent_encode(
            &file_path.to_string_lossy(),
            percent_encoding::NON_ALPHANUMERIC,
        )
        .to_string();
        let message = format!("![截图](/image?path={encoded_path})");
        let rewritten = rewrite_markdown_local_images(&message).expect("image should rewrite");
        let image_id = rewritten
            .split("/image?id=")
            .nth(1)
            .and_then(|tail| tail.split(')').next())
            .expect("rewritten image id")
            .to_string();

        let public_response = handle_serve_image(
            public_headers(),
            Query(ImageQuery {
                path: None,
                id: Some(image_id.clone()),
            }),
        )
        .await;
        assert_eq!(public_response.status(), StatusCode::UNAUTHORIZED);

        let local_response = handle_serve_image(
            HeaderMap::new(),
            Query(ImageQuery {
                path: None,
                id: Some(image_id),
            }),
        )
        .await;
        assert_eq!(local_response.status(), StatusCode::OK);
        assert_eq!(
            local_response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/png")
        );

        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn large_gif_images_are_served_without_jpeg_compression() {
        let file_path =
            std::env::temp_dir().join(format!("iterate-large-gif-{}.gif", uuid::Uuid::new_v4()));
        let mut bytes = b"GIF89a".to_vec();
        bytes.resize(151_000, 0);
        std::fs::write(&file_path, bytes).expect("write test gif");

        let response = serve_image_file(&file_path);

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("image/gif")
        );

        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn speech_memory_auth_guard_requires_scoped_authenticated_public_requests() {
        let session_principal = AuthPrincipal {
            principal_id: "device:test".to_string(),
            device_id: "test".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_SESSION_RESPOND.to_string()],
        };
        let read_principal = AuthPrincipal {
            scopes: vec![SCOPE_SPEECH_MEMORY_READ.to_string()],
            ..session_principal.clone()
        };
        let write_principal = AuthPrincipal {
            scopes: vec![SCOPE_SPEECH_MEMORY_WRITE.to_string()],
            ..session_principal.clone()
        };

        assert!(speech_memory_auth_denial(
            None,
            false,
            SCOPE_SPEECH_MEMORY_READ,
            "missing_scope_speech_memory_read",
        )
        .is_none());
        assert_eq!(
            speech_memory_auth_denial(
                None,
                true,
                SCOPE_SPEECH_MEMORY_READ,
                "missing_scope_speech_memory_read",
            ),
            Some((StatusCode::UNAUTHORIZED, "invalid_device_auth"))
        );
        assert_eq!(
            speech_memory_auth_denial(
                Some(&session_principal),
                true,
                SCOPE_SPEECH_MEMORY_READ,
                "missing_scope_speech_memory_read",
            ),
            Some((StatusCode::FORBIDDEN, "missing_scope_speech_memory_read"))
        );
        assert!(speech_memory_auth_denial(
            Some(&read_principal),
            true,
            SCOPE_SPEECH_MEMORY_READ,
            "missing_scope_speech_memory_read",
        )
        .is_none());
        assert_eq!(
            speech_memory_auth_denial(
                Some(&read_principal),
                true,
                SCOPE_SPEECH_MEMORY_WRITE,
                "missing_scope_speech_memory_write",
            ),
            Some((StatusCode::FORBIDDEN, "missing_scope_speech_memory_write"))
        );
        assert!(speech_memory_auth_denial(
            Some(&write_principal),
            true,
            SCOPE_SPEECH_MEMORY_WRITE,
            "missing_scope_speech_memory_write",
        )
        .is_none());
    }

    #[tokio::test]
    async fn speech_memory_handlers_reject_anonymous_public_requests() {
        let get_response = handle_api_speech_muscle_memory_get(public_headers()).await;
        assert_eq!(get_response.status(), StatusCode::UNAUTHORIZED);

        let post_response = handle_api_speech_muscle_memory_post(
            public_headers(),
            Json(serde_json::json!({ "entries": [] })),
        )
        .await;
        assert_eq!(post_response.status(), StatusCode::UNAUTHORIZED);

        let correction_get_response =
            handle_api_speech_correction_memory_get(public_headers()).await;
        assert_eq!(correction_get_response.status(), StatusCode::UNAUTHORIZED);

        let correction_post_response = handle_api_speech_correction_memory_post(
            public_headers(),
            Json(serde_json::json!({ "entries": [] })),
        )
        .await;
        assert_eq!(correction_post_response.status(), StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn keeps_handoff_messages_active() {
        assert!(!is_inactive_session_message(
            "已暂停当前工作。请直接输入下一步指令。"
        ));
        assert!(!is_inactive_session_message(
            "已暂停当前处理，等待你的下一步指令。"
        ));
        assert!(!is_inactive_session_message(
            "已停止当前操作，等待你的下一步指令。"
        ));
        assert!(!is_inactive_session_message(
            "已调用 zhi，等待你的下一步指令。"
        ));
    }

    #[test]
    fn keeps_real_working_message_active() {
        assert!(!is_inactive_session_message(
            "正在修复 active sessions 残留并验证 8080 健康。"
        ));
    }

    #[test]
    fn active_session_registry_keeps_latest_handoff_payload() {
        let mut registry = HashMap::new();
        update_active_session_registry(
            &mut registry,
            &build_payload("serve-1", "/Users/test/project", "正在修复 active sessions"),
        );
        update_active_session_registry(
            &mut registry,
            &build_payload(
                "serve-1",
                "/Users/test/project",
                "已调用 zhi，等待你的下一步指令。",
            ),
        );

        let payload = lookup_active_session_payload(
            &registry,
            Some("serve-1"),
            Some("/Users/test/project"),
            None,
        )
        .expect("expected active payload");
        let message = payload
            .get("request")
            .and_then(|request| request.get("message"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        assert_eq!(message, "已调用 zhi，等待你的下一步指令。");
    }

    #[test]
    fn active_session_registry_keeps_handoff_project_visible() {
        let mut registry = HashMap::new();
        update_active_session_registry(
            &mut registry,
            &build_payload(
                "serve-2",
                "/Users/test/other project",
                "已调用 zhi，等待你的下一步指令。",
            ),
        );

        let payload =
            lookup_active_session_payload(&registry, None, Some("/Users/test/other project"), None);

        assert!(payload.is_some());
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn active_session_registry_keeps_same_project_sessions_independent() {
        let mut registry = HashMap::new();
        update_active_session_registry(
            &mut registry,
            &build_payload("serve-1", "/Users/test/project", "第一条会话"),
        );
        update_active_session_registry(
            &mut registry,
            &build_payload("serve-2", "/Users/test/project", "第二条会话"),
        );

        assert_eq!(registry.len(), 2);

        let first = lookup_active_session_payload(
            &registry,
            Some("serve-1"),
            Some("/Users/test/project"),
            None,
        )
        .expect("expected first payload");
        let second = lookup_active_session_payload(
            &registry,
            Some("serve-2"),
            Some("/Users/test/project"),
            None,
        )
        .expect("expected second payload");

        let first_message = first
            .get("request")
            .and_then(|request| request.get("message"))
            .and_then(|value| value.as_str())
            .unwrap_or("");
        let second_message = second
            .get("request")
            .and_then(|request| request.get("message"))
            .and_then(|value| value.as_str())
            .unwrap_or("");

        assert_eq!(first_message, "第一条会话");
        assert_eq!(second_message, "第二条会话");
    }

    #[test]
    fn active_session_lookup_with_missing_request_id_does_not_fallback_to_project() {
        let mut registry = HashMap::new();
        update_active_session_registry(
            &mut registry,
            &build_payload("serve-2", "/Users/test/project", "第二条会话"),
        );

        let payload = lookup_active_session_payload(
            &registry,
            Some("serve-missing"),
            Some("/Users/test/project"),
            None,
        );

        assert!(payload.is_none());
    }

    #[test]
    fn mcp_action_route_resolves_from_recent_active_session_payload() {
        let mut registry = HashMap::new();
        update_active_session_registry(
            &mut registry,
            &build_payload_with_timeline_route(
                "serve-desktop",
                "/Users/test/project",
                "当前桌面会话",
                "codex-thread-stable",
            ),
        );
        let action_payload = serde_json::json!({
            "action": "submit",
            "project_path": "/Users/test/project",
            "request_id": "serve-one-shot",
            "user_input": "手机语音"
        });

        let resolved = resolve_mcp_action_timeline_route_id(
            &action_payload,
            Some("serve-one-shot"),
            Some("/Users/test/project"),
            Some("serve-desktop"),
            &registry,
        )
        .expect("expected route from active session payload");

        assert_eq!(resolved.route_id, "codex-thread-stable");
        assert_eq!(resolved.source, "active_session_fallback_route");
    }

    #[test]
    fn mcp_action_route_uses_unique_project_route_when_request_id_is_one_shot() {
        let mut registry = HashMap::new();
        update_active_session_registry(
            &mut registry,
            &build_payload_with_timeline_route(
                "serve-desktop-1",
                "/Users/test/project",
                "桌面会话一",
                "codex-thread-stable",
            ),
        );
        update_active_session_registry(
            &mut registry,
            &build_payload_with_timeline_route(
                "serve-desktop-2",
                "/Users/test/project",
                "桌面会话二",
                "codex-thread-stable",
            ),
        );
        let action_payload = serde_json::json!({
            "action": "submit",
            "project_path": "/Users/test/project",
            "request_id": "serve-one-shot",
            "user_input": "手机语音"
        });

        let resolved = resolve_mcp_action_timeline_route_id(
            &action_payload,
            Some("serve-one-shot"),
            Some("/Users/test/project"),
            None,
            &registry,
        )
        .expect("expected unique project route");

        assert_eq!(resolved.route_id, "codex-thread-stable");
        assert_eq!(resolved.source, "active_session_unique_project_route");
    }

    #[test]
    fn mcp_action_route_refuses_ambiguous_project_routes() {
        let mut registry = HashMap::new();
        update_active_session_registry(
            &mut registry,
            &build_payload_with_timeline_route(
                "serve-desktop-1",
                "/Users/test/project",
                "第一条会话",
                "codex-thread-a",
            ),
        );
        update_active_session_registry(
            &mut registry,
            &build_payload_with_timeline_route(
                "serve-desktop-2",
                "/Users/test/project",
                "第二条会话",
                "codex-thread-b",
            ),
        );
        let action_payload = serde_json::json!({
            "action": "submit",
            "project_path": "/Users/test/project",
            "request_id": "serve-one-shot",
            "user_input": "手机语音"
        });

        let resolved = resolve_mcp_action_timeline_route_id(
            &action_payload,
            Some("serve-one-shot"),
            Some("/Users/test/project"),
            None,
            &registry,
        );

        assert!(resolved.is_none());
    }

    #[test]
    fn pull_action_with_request_id_does_not_fallback_to_project_path() {
        let mut cache = HashMap::new();
        cache.insert(
            "/Users/test/project".to_string(),
            serde_json::json!({ "action": "wrong-project-fallback" }),
        );

        let action =
            take_cached_action_for_pull(&mut cache, "/Users/test/project", Some("serve-1"));

        assert!(action.is_none());
        assert!(cache.contains_key("/Users/test/project"));
    }

    #[test]
    fn pull_action_without_request_id_uses_project_path_fallback() {
        let mut cache = HashMap::new();
        cache.insert(
            "/Users/test/project".to_string(),
            serde_json::json!({ "action": "legacy-project-fallback" }),
        );

        let action = take_cached_action_for_pull(&mut cache, "/Users/test/project", None)
            .expect("expected legacy project action");

        assert_eq!(action["action"], "legacy-project-fallback");
        assert!(!cache.contains_key("/Users/test/project"));
    }

    #[test]
    fn request_id_binding_detects_replaced_same_project_request() {
        let instances = vec![crate::ui::window_registry::WindowInstance {
            pid: 1,
            project_path: "/Users/test/project".to_string(),
            window_title: "iterate — /Users/test/project".to_string(),
            registered_at: chrono::Utc::now().to_rfc3339(),
            port: Some(5311),
            request_id: Some("req-new".to_string()),
            request_title: Some("new".to_string()),
        }];

        assert!(request_id_is_stale_for_live_window_instances(
            &instances,
            Some("req-old"),
            Some("/Users/test/project"),
        ));
        assert!(!request_id_is_stale_for_live_window_instances(
            &instances,
            Some("req-new"),
            Some("/Users/test/project"),
        ));
        assert!(!request_id_is_stale_for_live_window_instances(
            &instances,
            Some("req-old"),
            Some("/Users/test/other"),
        ));
    }

    #[test]
    fn pull_action_with_stale_window_request_drops_cached_action() {
        let instances = vec![crate::ui::window_registry::WindowInstance {
            pid: 1,
            project_path: "/Users/test/project".to_string(),
            window_title: "iterate — /Users/test/project".to_string(),
            registered_at: chrono::Utc::now().to_rfc3339(),
            port: Some(5311),
            request_id: Some("req-new".to_string()),
            request_title: Some("new".to_string()),
        }];
        let mut cache = HashMap::new();
        cache.insert(
            "req-old".to_string(),
            serde_json::json!({ "action": "stale-old-request" }),
        );

        let action = take_cached_action_for_pull_with_window_bindings(
            &mut cache,
            "/Users/test/project",
            Some("req-old"),
            &instances,
        );

        assert!(action.is_none());
        assert!(!cache.contains_key("req-old"));
    }

    #[tokio::test]
    async fn cleanup_completed_session_removes_state_cache_and_active_registry() {
        let _guard = ROUTE_DEBUG_TEST_LOCK.lock().await;
        let request_id = "serve-cleanup-test";
        let project_path = "/Users/test/cleanup-test";
        let payload = build_payload(request_id, project_path, "cleanup test");

        {
            let mut cache = MCP_STATE_CACHE.write().await;
            cache.insert(request_id.to_string(), payload.clone());
            cache.insert(project_path.to_string(), payload.clone());

            let mut touched_at = MCP_STATE_CACHE_TOUCHED_AT.write().await;
            touched_at.insert(request_id.to_string(), chrono::Utc::now());
            touched_at.insert(project_path.to_string(), chrono::Utc::now());
        }

        {
            let mut registry = ACTIVE_SESSION_REGISTRY.write().await;
            update_active_session_registry(&mut registry, &payload);
            assert!(registry.contains_key(request_id));
        }
        record_active_desktop_popup_route(Some(request_id), Some(project_path), "test-ready").await;

        let (removed_cache, removed_active) =
            cleanup_completed_session_by_request_id(request_id, "test-cleanup").await;

        assert!(removed_cache);
        assert!(removed_active);
        assert!(!MCP_STATE_CACHE.read().await.contains_key(request_id));
        assert!(!MCP_STATE_CACHE.read().await.contains_key(project_path));
        assert!(!ACTIVE_SESSION_REGISTRY
            .read()
            .await
            .contains_key(request_id));
        let routes = route_debug_status_value().await;
        assert!(routes["active_desktop_popup_route"].is_null());
        assert_eq!(
            routes["last_completed_route"]["request_id"].as_str(),
            Some(request_id)
        );

        MCP_STATE_CACHE_TOUCHED_AT
            .write()
            .await
            .remove(project_path);
        reset_active_desktop_popup_route_for_tests().await;
    }

    #[tokio::test]
    async fn stale_request_sync_cleanup_removes_project_cache_and_active_registry() {
        let request_id = format!("serve-stale-sync-{}", uuid::Uuid::new_v4());
        let project_path = format!("/tmp/stale-sync-{}", uuid::Uuid::new_v4());
        let payload = build_payload(&request_id, &project_path, "stale sync state");

        clear_mcp_state_route_for_test(&request_id, &project_path).await;
        {
            let mut cache = MCP_STATE_CACHE.write().await;
            cache.insert(request_id.clone(), payload.clone());
            cache.insert(project_path.clone(), payload.clone());

            let mut touched_at = MCP_STATE_CACHE_TOUCHED_AT.write().await;
            touched_at.insert(request_id.clone(), chrono::Utc::now());
            touched_at.insert(project_path.clone(), chrono::Utc::now());
        }
        {
            let mut registry = ACTIVE_SESSION_REGISTRY.write().await;
            update_active_session_registry(&mut registry, &payload);
            assert!(registry.contains_key(&request_id));
        }

        let (removed_cache, removed_active) = cleanup_stale_request_sync_route(
            Some(&request_id),
            Some(&project_path),
            "test-stale-request-sync",
        )
        .await;

        assert!(removed_cache);
        assert!(removed_active);
        assert!(!MCP_STATE_CACHE.read().await.contains_key(&request_id));
        assert!(!MCP_STATE_CACHE.read().await.contains_key(&project_path));
        assert!(!MCP_STATE_CACHE_TOUCHED_AT
            .read()
            .await
            .contains_key(&request_id));
        assert!(!MCP_STATE_CACHE_TOUCHED_AT
            .read()
            .await
            .contains_key(&project_path));
        assert!(!ACTIVE_SESSION_REGISTRY
            .read()
            .await
            .contains_key(&request_id));

        clear_mcp_state_route_for_test(&request_id, &project_path).await;
    }

    #[test]
    fn active_session_summaries_exclude_registry_entries_without_live_windows() {
        let now = chrono::Utc::now();
        let older = (now - chrono::Duration::seconds(5)).to_rfc3339();
        let recent = now.to_rfc3339();
        let mut registry = HashMap::new();
        registry.insert(
            "req-a".to_string(),
            ActiveSessionEntry {
                request_id: "req-a".to_string(),
                project_path: "/tmp/a-registry".to_string(),
                project_name: "a".to_string(),
                title: "".to_string(),
                payload: build_payload("req-a", "/tmp/a-registry", "a"),
                last_active_at: older,
            },
        );
        registry.insert(
            "req-b".to_string(),
            ActiveSessionEntry {
                request_id: "req-b".to_string(),
                project_path: "/tmp/b".to_string(),
                project_name: "b".to_string(),
                title: "registry b".to_string(),
                payload: build_payload("req-b", "/tmp/b", "b"),
                last_active_at: recent.clone(),
            },
        );

        let sessions = build_active_session_summaries(
            &registry,
            vec![crate::ui::window_registry::WindowInstance {
                pid: 1,
                project_path: "/tmp/a-window".to_string(),
                window_title: "iterate — /tmp/a-window".to_string(),
                registered_at: recent,
                request_id: Some("req-a".to_string()),
                request_title: Some("window a".to_string()),
                port: None,
            }],
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0]
                .get("request_id")
                .and_then(|value| value.as_str()),
            Some("req-a")
        );
        assert!(
            sessions.iter().all(|session| session
                .get("request_id")
                .and_then(|value| value.as_str())
                != Some("req-b")),
            "registry-only sessions must not appear in iOS active projects"
        );
        assert_eq!(
            sessions[0]
                .get("project_path")
                .and_then(|value| value.as_str()),
            Some("/tmp/a-window")
        );
        assert_eq!(
            sessions[0].get("title").and_then(|value| value.as_str()),
            Some("window a")
        );
    }

    #[test]
    fn active_session_summaries_include_live_windows_without_registry_entries() {
        let recent = chrono::Utc::now().to_rfc3339();
        let sessions = build_active_session_summaries(
            &HashMap::new(),
            vec![crate::ui::window_registry::WindowInstance {
                pid: 1,
                project_path: "/tmp/live".to_string(),
                window_title: "iterate — /tmp/live".to_string(),
                registered_at: recent.clone(),
                request_id: Some("req-live".to_string()),
                request_title: Some("window live".to_string()),
                port: Some(5311),
            }],
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0]
                .get("request_id")
                .and_then(|value| value.as_str()),
            Some("req-live")
        );
        assert_eq!(
            sessions[0]
                .get("project_path")
                .and_then(|value| value.as_str()),
            Some("/tmp/live")
        );
        assert_eq!(
            sessions[0]
                .get("project_name")
                .and_then(|value| value.as_str()),
            Some("live")
        );
        assert_eq!(
            sessions[0].get("title").and_then(|value| value.as_str()),
            Some("window live")
        );
        assert_eq!(
            sessions[0]
                .get("last_active_at")
                .and_then(|value| value.as_str()),
            Some(recent.as_str())
        );
    }

    #[test]
    fn active_session_summaries_exclude_registered_projects() {
        let recent = chrono::Utc::now().to_rfc3339();
        let sessions = build_active_session_summaries(
            &HashMap::new(),
            vec![crate::ui::window_registry::WindowInstance {
                pid: 1,
                project_path: "/tmp/live".to_string(),
                window_title: "iterate — /tmp/live".to_string(),
                registered_at: recent.clone(),
                request_id: Some("req-live".to_string()),
                request_title: Some("window live".to_string()),
                port: Some(5311),
            }],
        );

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0]
                .get("project_path")
                .and_then(|value| value.as_str()),
            Some("/tmp/live")
        );
        assert_eq!(
            sessions[0]
                .get("request_id")
                .and_then(|value| value.as_str()),
            Some("req-live")
        );
        assert_eq!(
            sessions[0].get("source").and_then(|value| value.as_str()),
            Some("window_registry")
        );
        assert_eq!(
            sessions[0].get("port").and_then(|value| value.as_u64()),
            Some(5311)
        );
    }

    #[test]
    fn active_session_summaries_keep_distinct_requests_for_same_project() {
        let older = (chrono::Utc::now() - chrono::Duration::seconds(5)).to_rfc3339();
        let recent = chrono::Utc::now().to_rfc3339();
        let sessions = build_active_session_summaries(
            &HashMap::new(),
            vec![
                crate::ui::window_registry::WindowInstance {
                    pid: 1,
                    project_path: "/tmp/duplicate".to_string(),
                    window_title: "iterate — /tmp/duplicate".to_string(),
                    registered_at: older,
                    request_id: Some("req-old".to_string()),
                    request_title: Some("old duplicate".to_string()),
                    port: Some(5311),
                },
                crate::ui::window_registry::WindowInstance {
                    pid: 2,
                    project_path: "/tmp/duplicate".to_string(),
                    window_title: "iterate — /tmp/duplicate".to_string(),
                    registered_at: recent,
                    request_id: Some("req-new".to_string()),
                    request_title: Some("new duplicate".to_string()),
                    port: Some(5312),
                },
            ],
        );

        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0]
                .get("request_id")
                .and_then(|value| value.as_str()),
            Some("req-new")
        );
        assert_eq!(
            sessions[1]
                .get("request_id")
                .and_then(|value| value.as_str()),
            Some("req-old")
        );
    }

    #[test]
    fn active_session_summaries_order_latest_window_before_stale_registry_touch() {
        let now = chrono::Utc::now();
        let old_window_registered_at = (now - chrono::Duration::minutes(10)).to_rfc3339();
        let new_window_registered_at = (now - chrono::Duration::minutes(1)).to_rfc3339();
        let stale_registry_touch = now.to_rfc3339();

        let mut registry = HashMap::new();
        registry.insert(
            "req-old".to_string(),
            ActiveSessionEntry {
                request_id: "req-old".to_string(),
                project_path: "/tmp/duplicate".to_string(),
                project_name: "duplicate".to_string(),
                title: "old registry touched recently".to_string(),
                payload: build_payload("req-old", "/tmp/duplicate", "old registry touched"),
                last_active_at: stale_registry_touch,
            },
        );

        let sessions = build_active_session_summaries(
            &registry,
            vec![
                crate::ui::window_registry::WindowInstance {
                    pid: 1,
                    project_path: "/tmp/duplicate".to_string(),
                    window_title: "iterate — /tmp/duplicate".to_string(),
                    registered_at: old_window_registered_at,
                    request_id: Some("req-old".to_string()),
                    request_title: Some("old window".to_string()),
                    port: Some(5311),
                },
                crate::ui::window_registry::WindowInstance {
                    pid: 2,
                    project_path: "/tmp/duplicate".to_string(),
                    window_title: "iterate — /tmp/duplicate".to_string(),
                    registered_at: new_window_registered_at,
                    request_id: Some("req-new".to_string()),
                    request_title: Some("new window".to_string()),
                    port: Some(5312),
                },
            ],
        );

        assert_eq!(sessions.len(), 2);
        assert_eq!(
            sessions[0]
                .get("request_id")
                .and_then(|value| value.as_str()),
            Some("req-new")
        );
    }

    #[test]
    fn active_session_summaries_display_window_focus_time_across_registry_retouches() {
        let now = chrono::Utc::now();
        let registered_at = (now - chrono::Duration::minutes(20)).to_rfc3339();
        let focused_at = (now - chrono::Duration::minutes(3)).to_rfc3339();
        let pid = 41;
        let instance = crate::ui::window_registry::WindowInstance {
            pid,
            project_path: "/tmp/focus-authority".to_string(),
            window_title: "iterate — /tmp/focus-authority".to_string(),
            registered_at,
            request_id: Some("req-focus-authority".to_string()),
            request_title: Some("focus authority".to_string()),
            port: Some(5311),
        };
        let focus_by_pid = HashMap::from([(pid, focused_at.clone())]);
        let mut registry = HashMap::new();

        update_active_session_registry(
            &mut registry,
            &build_payload(
                "req-focus-authority",
                "/tmp/focus-authority",
                "first repeated state",
            ),
        );
        let first = build_active_session_summaries_with_focus(
            &registry,
            vec![instance.clone()],
            &focus_by_pid,
        );

        update_active_session_registry(
            &mut registry,
            &build_payload(
                "req-focus-authority",
                "/tmp/focus-authority",
                "second repeated state",
            ),
        );
        let second =
            build_active_session_summaries_with_focus(&registry, vec![instance], &focus_by_pid);

        for sessions in [&first, &second] {
            assert_eq!(sessions.len(), 1);
            assert_eq!(
                sessions[0]
                    .get("last_active_at")
                    .and_then(|value| value.as_str()),
                Some(focused_at.as_str())
            );
        }
    }

    #[test]
    fn active_session_summaries_keep_distinct_focus_times_for_same_project_windows() {
        let now = chrono::Utc::now();
        let old_registered_at = (now - chrono::Duration::minutes(30)).to_rfc3339();
        let new_registered_at = (now - chrono::Duration::minutes(20)).to_rfc3339();
        let old_focus = (now - chrono::Duration::minutes(5)).to_rfc3339();
        let new_focus = (now - chrono::Duration::minutes(1)).to_rfc3339();
        let focus_by_pid = HashMap::from([(41, old_focus.clone()), (42, new_focus.clone())]);
        let sessions = build_active_session_summaries_with_focus(
            &HashMap::new(),
            vec![
                crate::ui::window_registry::WindowInstance {
                    pid: 41,
                    project_path: "/tmp/shared".to_string(),
                    window_title: "iterate — /tmp/shared".to_string(),
                    registered_at: old_registered_at,
                    request_id: Some("req-shared-a".to_string()),
                    request_title: Some("shared a".to_string()),
                    port: Some(5311),
                },
                crate::ui::window_registry::WindowInstance {
                    pid: 42,
                    project_path: "/tmp/shared".to_string(),
                    window_title: "iterate — /tmp/shared".to_string(),
                    registered_at: new_registered_at,
                    request_id: Some("req-shared-b".to_string()),
                    request_title: Some("shared b".to_string()),
                    port: Some(5312),
                },
            ],
            &focus_by_pid,
        );
        let displayed_times = sessions
            .iter()
            .filter_map(|session| {
                Some((
                    session.get("request_id")?.as_str()?.to_string(),
                    session.get("last_active_at")?.as_str()?.to_string(),
                ))
            })
            .collect::<HashMap<_, _>>();

        assert_eq!(displayed_times.get("req-shared-a"), Some(&old_focus));
        assert_eq!(displayed_times.get("req-shared-b"), Some(&new_focus));
    }

    #[test]
    fn active_session_summaries_exclude_registered_projects_without_live_windows() {
        let sessions = build_active_session_summaries(&HashMap::new(), Vec::new());

        assert!(sessions.is_empty());
    }

    #[test]
    fn active_session_lookup_ignores_registered_port_request_id_for_project_fallback() {
        let mut registry = HashMap::new();
        registry.insert(
            "req-real".to_string(),
            ActiveSessionEntry {
                request_id: "req-real".to_string(),
                project_path: "/tmp/port-project".to_string(),
                project_name: "port-project".to_string(),
                title: "real state".to_string(),
                payload: build_payload("req-real", "/tmp/port-project", "real state"),
                last_active_at: chrono::Utc::now().to_rfc3339(),
            },
        );

        let entry = lookup_active_session_entry(
            &registry,
            Some("registered-port-5311"),
            Some("/tmp/port-project"),
            None,
        )
        .expect("project fallback should still work for registered port ids");

        assert_eq!(entry.request_id, "req-real");
    }

    #[test]
    fn request_sync_effective_request_id_ignores_registered_port_with_project_path() {
        assert_eq!(
            effective_request_sync_request_id(
                Some("registered-port-5311"),
                Some("/tmp/port-project")
            ),
            None
        );
        assert_eq!(
            effective_request_sync_request_id(Some("registered-port-5311"), None).as_deref(),
            Some("registered-port-5311")
        );
        assert_eq!(
            effective_request_sync_request_id(Some("req-real"), Some("/tmp/port-project"))
                .as_deref(),
            Some("req-real")
        );
        assert_eq!(
            effective_request_sync_request_id(
                Some("registered-port-not-a-port"),
                Some("/tmp/port-project")
            )
            .as_deref(),
            Some("registered-port-not-a-port")
        );
    }

    #[test]
    fn registered_mcp_ports_from_dir_sorts_dedups_and_ignores_non_ports() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp_dir.path().join("5330"), "/Users/test/project").expect("write port");
        std::fs::write(temp_dir.path().join("5323"), "/Users/test/project").expect("write port");
        std::fs::write(temp_dir.path().join("not-a-port"), "/Users/test/project")
            .expect("write invalid");
        std::fs::create_dir(temp_dir.path().join("5330.copy")).expect("mkdir invalid");

        assert_eq!(
            registered_mcp_ports_from_dir(temp_dir.path()),
            vec![5323, 5330]
        );
    }

    #[test]
    fn apns_request_id_dedupe_keeps_longer_ttl_than_body_fallback() {
        assert_eq!(
            apns_dedupe_ttl_secs("request:serve-1"),
            APNS_NOTIFICATION_REQUEST_DEDUPE_SECS
        );
        assert_eq!(
            apns_dedupe_ttl_secs("fallback:/Users/test/project:123"),
            APNS_NOTIFICATION_DEDUPE_SECS
        );
    }

    #[test]
    fn apns_config_loads_file_when_process_env_is_partial() {
        let _lock = APNS_ENV_TEST_LOCK.lock().expect("lock APNS env");
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_dir = temp_dir.path().join(".config/iterate");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let key_path = temp_dir.path().join("AuthKey_TEST.p8");
        let key_pem = "synthetic-apns-test-key\n";
        std::fs::write(&key_path, key_pem).expect("write key");
        std::fs::write(
            config_dir.join("apns-env.sh"),
            format!(
                "export APNS_KEY_ID=\"FILE_KEY\"\n\
                 export APNS_TEAM_ID=\"FILE_TEAM\"\n\
                 export APNS_TOPIC=\"com.iterate.notify\"\n\
                 export APNS_AUTH_KEY_PATH=\"{}\"\n\
                 export APNS_ENV=\"sandbox\"\n",
                key_path.display()
            ),
        )
        .expect("write apns env");

        let _home_guard = EnvGuard::set("HOME", temp_dir.path().as_os_str());
        let _key_guard = EnvGuard::set("APNS_KEY_ID", OsStr::new("PARTIAL_KEY"));
        let _team_guard = EnvGuard::remove("APNS_TEAM_ID");
        let _topic_guard = EnvGuard::remove("APNS_TOPIC");
        let _path_guard = EnvGuard::remove("APNS_AUTH_KEY_PATH");
        let _p8_guard = EnvGuard::remove("APNS_AUTH_KEY_P8");
        let _env_guard = EnvGuard::remove("APNS_ENV");

        let config = load_apns_config().expect("APNS config should load from file");

        assert_eq!(config.key_id, "FILE_KEY");
        assert_eq!(config.team_id, "FILE_TEAM");
        assert_eq!(config.topic, "com.iterate.notify");
        assert_eq!(config.key_pem, key_pem);
        assert_eq!(config.endpoint, "https://api.sandbox.push.apple.com");
    }

    #[test]
    fn apns_config_loads_optional_file_env_when_required_env_exists() {
        let _lock = APNS_ENV_TEST_LOCK.lock().expect("lock APNS env");
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let config_dir = temp_dir.path().join(".config/iterate");
        std::fs::create_dir_all(&config_dir).expect("create config dir");
        let key_path = temp_dir.path().join("AuthKey_TEST.p8");
        let key_pem = "synthetic-apns-test-key\n";
        std::fs::write(&key_path, key_pem).expect("write key");
        std::fs::write(
            config_dir.join("apns-env.sh"),
            "export APNS_TOPIC=\"com.iterate.notify.file\"\n\
             export APNS_ENV=\"sandbox\"\n",
        )
        .expect("write apns env");

        let _home_guard = EnvGuard::set("HOME", temp_dir.path().as_os_str());
        let _key_guard = EnvGuard::set("APNS_KEY_ID", OsStr::new("ENV_KEY"));
        let _team_guard = EnvGuard::set("APNS_TEAM_ID", OsStr::new("ENV_TEAM"));
        let _path_guard = EnvGuard::set("APNS_AUTH_KEY_PATH", key_path.as_os_str());
        let _p8_guard = EnvGuard::remove("APNS_AUTH_KEY_P8");
        let _topic_guard = EnvGuard::remove("APNS_TOPIC");
        let _env_guard = EnvGuard::remove("APNS_ENV");

        let config = load_apns_config().expect("APNS config should load from env plus file");

        assert_eq!(config.key_id, "ENV_KEY");
        assert_eq!(config.team_id, "ENV_TEAM");
        assert_eq!(config.topic, "com.iterate.notify.file");
        assert_eq!(config.key_pem, key_pem);
        assert_eq!(config.endpoint, "https://api.sandbox.push.apple.com");
    }

    #[test]
    fn route_debug_snapshot_prefers_request_id() {
        let snapshot = build_route_debug_snapshot(
            Some("serve-1"),
            Some("/Users/test/project"),
            "desktop_popup_ready",
        )
        .expect("route snapshot");

        assert_eq!(snapshot.route_key, "serve-1");
        assert_eq!(snapshot.request_id.as_deref(), Some("serve-1"));
        assert_eq!(
            snapshot.project_path.as_deref(),
            Some("/Users/test/project")
        );
        assert_eq!(snapshot.source, "desktop_popup_ready");
        assert!(!snapshot.updated_at.is_empty());
    }

    #[test]
    fn route_debug_snapshot_falls_back_to_project_path() {
        let snapshot =
            build_route_debug_snapshot(None, Some("/Users/test/project"), "cleanup-session")
                .expect("route snapshot");

        assert_eq!(snapshot.route_key, "/Users/test/project");
        assert_eq!(snapshot.request_id, None);
        assert_eq!(
            snapshot.project_path.as_deref(),
            Some("/Users/test/project")
        );
        assert_eq!(snapshot.source, "cleanup-session");
    }

    #[test]
    fn route_debug_snapshot_requires_any_route_part() {
        assert!(build_route_debug_snapshot(None, None, "missing").is_none());
    }

    #[tokio::test]
    async fn route_debug_status_tracks_active_desktop_popup_route() {
        let _guard = ROUTE_DEBUG_TEST_LOCK.lock().await;
        reset_active_desktop_popup_route_for_tests().await;

        record_active_desktop_popup_route(
            Some("serve-ready"),
            Some("/Users/test/project"),
            "desktop_popup_ready",
        )
        .await;

        let routes = route_debug_status_value().await;
        assert_eq!(
            routes["active_desktop_popup_route"]["route_key"].as_str(),
            Some("serve-ready")
        );
        assert_eq!(
            routes["active_desktop_popup_route"]["source"].as_str(),
            Some("desktop_popup_ready")
        );

        reset_active_desktop_popup_route_for_tests().await;
    }

    #[test]
    fn json_cache_prune_removes_expired_entries_and_keeps_fresh_entries() {
        let mut cache = HashMap::new();
        let mut touched_at = HashMap::new();
        cache.insert("old".to_string(), serde_json::json!({ "state": "old" }));
        cache.insert("fresh".to_string(), serde_json::json!({ "state": "fresh" }));
        touched_at.insert(
            "old".to_string(),
            chrono::DateTime::parse_from_rfc3339("2000-01-01T00:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        touched_at.insert("fresh".to_string(), chrono::Utc::now());

        let removed = prune_json_cache("test", &mut cache, &mut touched_at, 60, 10);

        assert_eq!(removed, 1);
        assert!(!cache.contains_key("old"));
        assert!(cache.contains_key("fresh"));
        assert!(!touched_at.contains_key("old"));
        assert!(touched_at.contains_key("fresh"));
    }

    #[test]
    fn cache_metrics_snapshot_reports_route_hit_rates() {
        let metrics = CacheMetrics::default();

        metrics.record_lookup(CacheLookupRoute::RequestId, true);
        metrics.record_lookup(CacheLookupRoute::ProjectPath, false);
        metrics.record_write_count(2);
        metrics.record_pruned_count(1);
        metrics.record_active_registry_fallback_hit();

        let snapshot = metrics.snapshot();

        assert_eq!(snapshot["lookups"], 2);
        assert_eq!(snapshot["hits"], 1);
        assert_eq!(snapshot["misses"], 1);
        assert_eq!(snapshot["hit_rate_percent"], 50.0);
        assert_eq!(snapshot["writes"], 2);
        assert_eq!(snapshot["pruned"], 1);
        assert_eq!(snapshot["active_registry_fallback_hits"], 1);
        assert_eq!(snapshot["routes"]["request_id"]["hit_rate_percent"], 100.0);
        assert_eq!(snapshot["routes"]["project_path"]["hit_rate_percent"], 0.0);
    }

    #[test]
    fn active_session_registry_prunes_expired_entries() {
        let mut registry = HashMap::new();
        registry.insert(
            "serve-old".to_string(),
            ActiveSessionEntry {
                request_id: "serve-old".to_string(),
                project_path: "/Users/test/old".to_string(),
                project_name: "old".to_string(),
                title: "old".to_string(),
                payload: build_payload("serve-old", "/Users/test/old", "old"),
                last_active_at: "2000-01-01T00:00:00Z".to_string(),
            },
        );
        registry.insert(
            "serve-new".to_string(),
            ActiveSessionEntry {
                request_id: "serve-new".to_string(),
                project_path: "/Users/test/new".to_string(),
                project_name: "new".to_string(),
                title: "new".to_string(),
                payload: build_payload("serve-new", "/Users/test/new", "new"),
                last_active_at: chrono::Utc::now().to_rfc3339(),
            },
        );

        prune_active_session_registry(&mut registry);

        assert!(registry.contains_key("serve-new"));
        assert!(!registry.contains_key("serve-old"));
        assert!(parse_rfc3339(
            &registry
                .get("serve-new")
                .expect("expected fresh entry")
                .last_active_at
        )
        .is_some());
    }

    #[test]
    fn inactive_message_removes_existing_active_session() {
        let mut registry = HashMap::new();
        update_active_session_registry(
            &mut registry,
            &build_payload("serve-1", "/Users/test/project", "第一条会话"),
        );
        assert!(registry.contains_key("serve-1"));

        update_active_session_registry(
            &mut registry,
            &build_payload(
                "serve-1",
                "/Users/test/project",
                "任务已结束，请查看最终状态。",
            ),
        );

        assert!(!registry.contains_key("serve-1"));
    }

    #[test]
    fn is_valid_ipv4_accepts_valid_addresses() {
        assert!(is_valid_ipv4("100.117.101.49"));
        assert!(is_valid_ipv4("192.168.1.1"));
        assert!(is_valid_ipv4("0.0.0.0"));
        assert!(is_valid_ipv4("255.255.255.255"));
        assert!(is_valid_ipv4("10.0.0.1"));
    }

    #[test]
    fn is_valid_ipv4_rejects_invalid_input() {
        assert!(!is_valid_ipv4(""));
        assert!(!is_valid_ipv4("not an ip"));
        assert!(!is_valid_ipv4("The Tailscale GUI failed to start..."));
        assert!(!is_valid_ipv4("256.1.1.1"));
        assert!(!is_valid_ipv4("1.2.3"));
        assert!(!is_valid_ipv4("1.2.3.4.5"));
        assert!(!is_valid_ipv4("abc.def.ghi.jkl"));
        assert!(!is_valid_ipv4("100.117.101.49:8080"));
        assert!(!is_valid_ipv4("tailscale is stopped"));
    }

    #[test]
    fn parse_first_ipv4_line_extracts_valid_ip() {
        assert_eq!(
            parse_first_ipv4_line("100.117.101.49\n"),
            Some("100.117.101.49".to_string())
        );
        assert_eq!(
            parse_first_ipv4_line("  100.117.101.49  \n"),
            Some("100.117.101.49".to_string())
        );
        assert_eq!(
            parse_first_ipv4_line("some error text\n100.117.101.49\n"),
            Some("100.117.101.49".to_string())
        );
    }

    #[test]
    fn parse_first_ipv4_line_returns_none_for_invalid() {
        assert_eq!(parse_first_ipv4_line(""), None);
        assert_eq!(
            parse_first_ipv4_line("The Tailscale GUI failed to start because the engine\n"),
            None
        );
        assert_eq!(parse_first_ipv4_line("logged out\n"), None);
        assert_eq!(parse_first_ipv4_line("not-an-ip\nhostname.local\n"), None);
    }

    #[test]
    fn is_tailscale_ipv4_matches_cgnat_range() {
        assert!(is_tailscale_ipv4("100.64.0.1"));
        assert!(is_tailscale_ipv4("100.117.101.49"));
        assert!(is_tailscale_ipv4("100.127.255.254"));
        assert!(!is_tailscale_ipv4("100.63.255.255"));
        assert!(!is_tailscale_ipv4("100.128.0.1"));
        assert!(!is_tailscale_ipv4("10.101.57.139"));
        assert!(!is_tailscale_ipv4("192.168.1.2"));
        assert!(!is_tailscale_ipv4("not-an-ip"));
    }

    #[test]
    fn parse_first_tailscale_ipv4_from_ifconfig_extracts_utun_address() {
        let ifconfig = r#"
utun3: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1380
    inet6 fe80::ce81:b1c:bd2c:69e%utun3 prefixlen 64 scopeid 0x10
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
    inet 10.101.57.139 netmask 0xffffff00 broadcast 10.101.57.255
utun4: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1280
    inet 100.117.101.49 --> 100.117.101.49 netmask 0xffffffff
    inet6 fd7a:115c:a1e0::e73b:6531 prefixlen 48
"#;
        assert_eq!(
            parse_first_tailscale_ipv4_from_ifconfig(ifconfig),
            Some("100.117.101.49".to_string())
        );
    }

    #[test]
    fn parse_first_tailscale_ipv4_from_ifconfig_rejects_non_tailscale() {
        let ifconfig = r#"
en0: flags=8863<UP,BROADCAST,SMART,RUNNING,SIMPLEX,MULTICAST> mtu 1500
    inet 10.101.57.139 netmask 0xffffff00 broadcast 10.101.57.255
utun0: flags=8051<UP,POINTOPOINT,RUNNING,MULTICAST> mtu 1380
    inet 100.128.0.1 --> 100.128.0.1 netmask 0xffffffff
"#;
        assert_eq!(parse_first_tailscale_ipv4_from_ifconfig(ifconfig), None);
    }

    fn paired_device_record_for_test(device_id: &str, token: &str) -> PairedDeviceRecord {
        PairedDeviceRecord {
            device_id: device_id.to_string(),
            device_name: device_id.to_string(),
            client_kind: "ios".to_string(),
            token_hash: bridge_token_hash(token),
            scopes: mobile_device_scopes(false),
            created_at: "2026-07-13T00:00:00Z".to_string(),
            last_seen_at: "2026-07-13T00:00:00Z".to_string(),
            file_browser_roots: Vec::new(),
            revoked_at: None,
        }
    }

    #[test]
    fn explicit_file_browser_roots_are_canonical_deduplicated_and_never_filesystem_root() {
        let directory = tempfile::tempdir().expect("temporary root directory");
        let home = directory.path().join("home");
        let file = home.join("note.txt");
        std::fs::create_dir_all(&home).expect("create home root");
        std::fs::write(&file, b"note").expect("create file");

        let roots = normalize_file_browser_roots(&[
            home.to_string_lossy().to_string(),
            format!("{}/", home.display()),
        ])
        .expect("normalize roots");
        assert_eq!(
            roots,
            vec![home.canonicalize().unwrap().display().to_string()]
        );
        assert_eq!(
            normalize_file_browser_roots(&[directory
                .path()
                .ancestors()
                .last()
                .expect("filesystem root")
                .display()
                .to_string(),]),
            Err("filesystem_root_not_allowed")
        );
        assert_eq!(
            normalize_file_browser_roots(&[file.display().to_string()]),
            Err("invalid_file_browser_root")
        );
        assert_eq!(
            normalize_file_browser_roots(&[directory.path().join("missing").display().to_string()]),
            Err("invalid_file_browser_root")
        );
    }

    #[test]
    fn file_browser_root_updates_are_isolated_to_the_selected_device() {
        let directory = tempfile::tempdir().expect("temporary paired-device directory");
        let path = directory.path().join("paired-devices.json");
        let granted_root = directory.path().join("home");
        std::fs::create_dir_all(&granted_root).expect("create granted root");
        let store = PairedDeviceStore {
            devices: vec![
                paired_device_record_for_test("iphone-a", "token-a"),
                paired_device_record_for_test("iphone-b", "token-b"),
            ],
        };
        super::save_paired_device_store_at(&path, &store).expect("save paired devices");

        let normalized_roots =
            normalize_file_browser_roots(&[granted_root.to_string_lossy().to_string()])
                .expect("normalize granted root");
        assert!(
            update_paired_device_file_roots_at(&path, "iphone-a", normalized_roots.clone(),)
                .expect("update selected device")
        );

        let principal_a = AuthPrincipal {
            principal_id: "device:iphone-a".to_string(),
            device_id: "iphone-a".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_FILE_LIST.to_string()],
        };
        let principal_b = AuthPrincipal {
            principal_id: "device:iphone-b".to_string(),
            device_id: "iphone-b".to_string(),
            client_kind: "ios".to_string(),
            scopes: vec![SCOPE_FILE_LIST.to_string()],
        };
        assert_eq!(
            explicit_file_list_roots_for_principal_at(&path, Some(&principal_a)),
            vec![granted_root.canonicalize().unwrap()]
        );
        assert!(explicit_file_list_roots_for_principal_at(&path, Some(&principal_b)).is_empty());

        let restored = super::load_paired_device_store_at(&path).expect("reload paired devices");
        assert_eq!(restored.devices[0].file_browser_roots, normalized_roots);
        assert!(restored.devices[1].file_browser_roots.is_empty());
    }

    #[test]
    fn active_repairing_preserves_roots_but_revoked_repairing_does_not() {
        let directory = tempfile::tempdir().expect("temporary root directory");
        let home = directory.path().join("home");
        std::fs::create_dir_all(&home).expect("create home root");
        let root = home.canonicalize().unwrap().display().to_string();

        let mut active = paired_device_record_for_test("iphone", "old-token");
        active.file_browser_roots = vec![root.clone()];
        let mut store = PairedDeviceStore {
            devices: vec![active],
        };
        replace_paired_device_record(
            &mut store,
            paired_device_record_for_test("iphone", "new-token"),
        );
        assert_eq!(store.devices[0].file_browser_roots, vec![root.clone()]);

        store.devices[0].revoked_at = Some("2026-08-10T00:00:00Z".to_string());
        replace_paired_device_record(
            &mut store,
            paired_device_record_for_test("iphone", "third-token"),
        );
        assert!(store.devices[0].file_browser_roots.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn paired_device_store_concurrent_updates_preserve_both_devices() {
        let directory = tempfile::tempdir().expect("temporary paired-device directory");
        let path = directory.path().join("paired-devices.json");
        let barrier = Arc::new(Barrier::new(2));

        let handles = [("iphone-a", "token-a"), ("iphone-b", "token-b")]
            .into_iter()
            .map(|(device_id, token)| {
                let path = path.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    super::mutate_paired_device_store_at(&path, |store| {
                        store.devices.retain(|device| device.device_id != device_id);
                        store
                            .devices
                            .push(paired_device_record_for_test(device_id, token));
                        ((), true)
                    })
                    .expect("concurrent paired-device update");
                })
            })
            .collect::<Vec<_>>();

        for handle in handles {
            handle.join().expect("paired-device writer thread");
        }

        let store = super::load_paired_device_store_at(&path).expect("load paired devices");
        let mut device_ids = store
            .devices
            .into_iter()
            .map(|device| device.device_id)
            .collect::<Vec<_>>();
        device_ids.sort();
        assert_eq!(device_ids, vec!["iphone-a", "iphone-b"]);
    }

    #[test]
    fn paired_device_store_recovers_backup_and_throttles_last_seen_writes() {
        let directory = tempfile::tempdir().expect("temporary paired-device directory");
        let path = directory.path().join("paired-devices.json");
        let token = "durable-device-token";
        let store = PairedDeviceStore {
            devices: vec![paired_device_record_for_test("iphone", token)],
        };

        super::save_paired_device_store_at(&path, &store).expect("initial paired-device save");
        super::save_paired_device_store_at(&path, &store).expect("paired-device backup save");
        std::fs::write(&path, b"{corrupt").expect("corrupt primary paired-device store");

        let first_seen = chrono::DateTime::parse_from_rfc3339("2026-07-13T00:00:30Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let (principal, persisted, revoked) =
            super::authenticate_paired_device_at(&path, token, None, first_seen)
                .expect("recover paired-device backup");
        assert_eq!(
            principal.expect("paired-device principal").device_id,
            "iphone"
        );
        assert!(!persisted, "30-second auth should not rewrite last_seen_at");
        assert!(!revoked);
        serde_json::from_str::<PairedDeviceStore>(
            &std::fs::read_to_string(&path).expect("restored primary paired-device store"),
        )
        .expect("restored primary is valid JSON");

        let later_seen = chrono::DateTime::parse_from_rfc3339("2026-07-13T00:01:01Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let (_, persisted, revoked) =
            super::authenticate_paired_device_at(&path, token, None, later_seen)
                .expect("update paired-device last_seen_at");
        assert!(persisted, "61-second auth should persist last_seen_at");
        assert!(!revoked);

        let restored = super::load_paired_device_store_at(&path).expect("reload paired devices");
        assert_eq!(
            restored.devices[0].last_seen_at,
            "2026-07-13T00:01:01+00:00"
        );
    }

    #[test]
    fn revoked_paired_device_auth_is_distinct_from_unknown_credentials() {
        let directory = tempfile::tempdir().expect("temporary paired-device directory");
        let path = directory.path().join("paired-devices.json");
        let token = "revoked-device-token";
        let mut record = paired_device_record_for_test("iphone", token);
        record.revoked_at = Some("2026-08-14T00:00:00Z".to_string());
        super::save_paired_device_store_at(
            &path,
            &PairedDeviceStore {
                devices: vec![record],
            },
        )
        .expect("save revoked paired device");

        let (principal, persisted, revoked) =
            super::authenticate_paired_device_at(&path, token, Some("iphone"), chrono::Utc::now())
                .expect("read revoked paired device");
        assert!(principal.is_none());
        assert!(!persisted);
        assert!(revoked);

        let (principal, _, revoked) = super::authenticate_paired_device_at(
            &path,
            "unknown-token",
            Some("iphone"),
            chrono::Utc::now(),
        )
        .expect("read unknown paired device");
        assert!(principal.is_none());
        assert!(!revoked);
    }
}
