use axum::http::{header, HeaderMap};

const PUBLIC_BRIDGE_BASE_URL_ENV: &str = "ITERATE_PUBLIC_BRIDGE_BASE_URL";

fn normalize_public_origin(value: &str) -> Option<String> {
    let trimmed = value.trim().trim_end_matches('/');
    let parsed = reqwest::Url::parse(trimmed).ok()?;
    if parsed.scheme() != "https"
        || parsed.host_str().is_none()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return None;
    }
    Some(trimmed.to_string())
}

fn public_bridge_base_url_override() -> Option<String> {
    std::env::var(PUBLIC_BRIDGE_BASE_URL_ENV)
        .ok()
        .and_then(|value| normalize_public_origin(&value))
}

fn configured_public_bridge_base_url() -> Option<String> {
    let config = crate::config::load_standalone_config().ok()?;
    if let Some(route) = config.mobile_config.formal_route.as_ref() {
        if route.schema_version == 1
            && matches!(
                route.transport.as_str(),
                "cloudflare_named_tunnel" | "aliyun_ssh_reverse_tunnel"
            )
        {
            if let Some(base_url) = normalize_public_origin(&route.base_url).filter(|base_url| {
                route.transport != "aliyun_ssh_reverse_tunnel"
                    || crate::tunnel::commands::aliyun_ip_origin_is_public(base_url)
            }) {
                return Some(base_url);
            }
        }
    }
    config
        .cloudflare_config
        .guided_setup_enabled
        .then(|| normalize_public_origin(&config.cloudflare_config.public_hostname))
        .flatten()
}

pub(super) fn public_bridge_base_url() -> String {
    public_bridge_base_url_override()
        .or_else(configured_public_bridge_base_url)
        .unwrap_or_default()
}

pub(super) fn public_bridge_base_url_is_overridden() -> bool {
    public_bridge_base_url_override().is_some() || configured_public_bridge_base_url().is_some()
}

fn public_bridge_host() -> String {
    let base_url = public_bridge_base_url();
    let without_scheme = base_url
        .strip_prefix("https://")
        .or_else(|| base_url.strip_prefix("http://"))
        .unwrap_or(&base_url);
    without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .trim_end_matches('.')
        .to_ascii_lowercase()
}

pub(super) fn debug_header_value(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
        .to_string()
}

pub(super) fn truncate_audit_value(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(super) fn has_bridge_auth_header(headers: &HeaderMap) -> bool {
    headers.contains_key(header::AUTHORIZATION)
        || headers.contains_key("x-iterate-device-token")
        || headers.contains_key("x-iterate-pairing-token")
}

pub(super) fn is_public_bridge_request(headers: &HeaderMap) -> bool {
    host_is_public_bridge(&debug_header_value(headers, header::HOST.as_str()))
        || host_is_public_bridge(&debug_header_value(headers, "x-forwarded-host"))
        || debug_header_value(headers, "cf-ray") != "-"
        || debug_header_value(headers, "x-forwarded-for") != "-"
}

fn host_is_public_bridge(host: &str) -> bool {
    let normalized = host.trim().trim_end_matches('.').to_ascii_lowercase();
    let host = normalized.as_str();
    let public_host = public_bridge_host();
    let public_host = public_host.as_str();
    if public_host.is_empty() {
        return false;
    }
    host == public_host
        || host
            .strip_suffix(":443")
            .is_some_and(|value| value == public_host)
        || host
            .strip_suffix(":80")
            .is_some_and(|value| value == public_host)
}

pub(super) fn is_public_control_path(path: &str) -> bool {
    path == "/"
        || path == "/index.html"
        || path == "/bridge_test.html"
        || path == "/.well-known/iterate/health"
        || path == "/pair"
        || path == "/pair/challenge"
        || path == "/pair/claim"
        || path == "/session/refresh"
        || path == "/session/revoke"
        || path == "/mobile"
        || path == "/ws"
        || path == "/ws/codex-live"
        || path == "/files"
        || path == "/files/roots"
        || path == "/files/mkdir"
        || path == "/image"
        || path == "/windows"
        || path.starts_with("/bridge/")
        || path.starts_with("/api/ghost-suggestions/")
        || path.starts_with("/api/phone-action-jobs/")
        || path.starts_with("/api/android/pairing/sessions/")
        || matches!(
            path,
            "/api/active-sessions"
                | "/api/android/pairing"
                | "/api/android/pairing/claim"
                | "/api/android/pairing/status"
                | "/api/audio-assets"
                | "/api/bridge/health"
                | "/api/cleanup-session"
                | "/api/config"
                | "/api/connection-diagnostics"
                | "/api/connection-status"
                | "/api/diagnostics"
                | "/api/desktop-codex-live"
                | "/api/desktop-codex-live/lease"
                | "/api/desktop-codex-live/status"
                | "/api/ghost-suggestions"
                | "/api/ghost-suggestion-learning"
                | "/api/import-prompts-dir"
                | "/api/mcp-tools"
                | "/api/mobile/paired-device-file-roots"
                | "/api/open-codex-chat"
                | "/api/phone-action"
                | "/api/phone-action-result"
                | "/api/prompt-library"
                | "/api/promptor-library"
                | "/api/recover-tailscale-funnel"
                | "/api/restart-service"
                | "/api/restart-tunnel"
                | "/api/show-window"
                | "/api/speech-correction-memory"
                | "/api/speech-muscle-memory"
                | "/api/speech-vocabulary"
                | "/api/test-audio"
                | "/push/subscribe"
                | "/push/unsubscribe"
        )
}
