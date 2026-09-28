use super::manager::{self, TunnelStatus};
use crate::config::{
    load_standalone_config, save_config, save_standalone_config, AppConfig, AppState,
    CloudflareConfig, CloudflareVerificationResult, FormalMobileRouteConfig,
    FormalMobileRouteVerification,
};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, State};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error as WsError;

const CLOUDFLARE_VERIFICATION_FRESHNESS_SECS: i64 = 10 * 60;

#[derive(Debug, Serialize, Clone)]
pub struct FormalMobileRouteStatus {
    pub configured: bool,
    pub transport: Option<String>,
    pub base_url: Option<String>,
    pub configured_at: Option<String>,
    pub formal_route_generation: Option<u64>,
    pub health: String,
    pub health_checked_at: Option<String>,
    pub last_verified_at: Option<String>,
    pub endpoint_identity_ok: bool,
    pub repair_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SaveCloudflareGuidedConfigRequest {
    pub public_hostname: String,
    #[serde(default)]
    pub access_expected: bool,
    #[serde(default)]
    pub web_login_console_origin: String,
    #[serde(default)]
    pub tunnel_token: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CloudflareGuidedConfigResponse {
    pub config: CloudflareConfig,
}

#[derive(Debug, Serialize)]
pub struct CloudflareWebLoginPairingResponse {
    pub pairing: crate::bridge::ws::WebLoginPairingIssueResponse,
}

#[derive(Debug, Serialize)]
pub struct CloudflareWebLoginSessionsResponse {
    pub sessions: Vec<crate::bridge::ws::WebLoginSessionSummary>,
}

#[derive(Debug, Serialize)]
pub struct CloudflareWebLoginRevokeSessionsResponse {
    pub ok: bool,
    pub revoked: usize,
}

#[derive(Debug, Deserialize)]
pub struct CreateCloudflareWebLoginAutoSetupRequest {
    pub api_token: String,
    pub zone_name: String,
    pub subdomain: String,
    #[serde(default)]
    pub overwrite_dns: bool,
    #[serde(default)]
    pub access_emails: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CloudflareWebLoginAutoSetupResponse {
    pub public_hostname: String,
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub dns_record_id: Option<String>,
    pub dns_action: String,
    pub access_app_id: Option<String>,
    pub access_policy_id: Option<String>,
    pub access_state: String,
    pub verification: CloudflareVerificationResult,
}

#[derive(Debug, Deserialize)]
pub struct CloudflareWebLoginAutoSetupCoreRequest {
    pub api_token: String,
    pub zone_name: String,
    pub subdomain: String,
    #[serde(default)]
    pub overwrite_dns: bool,
    #[serde(default)]
    pub access_emails: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CloudflareWebLoginHeadlessSmokeResponse {
    pub ok: bool,
    pub public_hostname: String,
    pub tunnel_id: String,
    pub tunnel_name: String,
    pub dns_record_id: Option<String>,
    pub dns_action: String,
    pub access_app_id: Option<String>,
    pub access_policy_id: Option<String>,
    pub access_state: String,
    pub verification: CloudflareVerificationResult,
    pub pairing_gate_ok: bool,
}

#[derive(Debug, Deserialize)]
struct CloudflareApiResponse<T> {
    success: bool,
    result: Option<T>,
    #[serde(default)]
    errors: Vec<CloudflareApiError>,
}

#[derive(Debug, Deserialize)]
struct CloudflareApiError {
    code: Option<i64>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct CloudflareZone {
    id: String,
    account: CloudflareAccount,
}

#[derive(Debug, Deserialize)]
struct CloudflareAccount {
    id: String,
}

#[derive(Debug, Deserialize)]
struct CloudflareTunnel {
    id: String,
    name: String,
    token: String,
}

#[derive(Debug, Deserialize)]
struct CloudflareDnsRecord {
    id: String,
    content: String,
    #[allow(dead_code)]
    name: String,
}

#[derive(Debug, Deserialize)]
struct CloudflareAccessApplication {
    id: String,
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    domain: Option<String>,
    #[serde(rename = "type")]
    #[allow(dead_code)]
    application_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CloudflareAccessPolicy {
    id: String,
    #[allow(dead_code)]
    name: String,
}

#[derive(Debug, Serialize)]
struct CreateCloudflareTunnelBody<'a> {
    name: &'a str,
    config_src: &'a str,
}

#[derive(Debug, Serialize)]
struct ConfigureCloudflareTunnelBody<'a> {
    config: CloudflareTunnelConfig<'a>,
}

#[derive(Debug, Serialize)]
struct CloudflareTunnelConfig<'a> {
    ingress: Vec<CloudflareTunnelIngress<'a>>,
}

#[derive(Debug, Serialize)]
struct CloudflareTunnelIngress<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    hostname: Option<&'a str>,
    service: &'a str,
    #[serde(rename = "originRequest", skip_serializing_if = "Option::is_none")]
    origin_request: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct CreateCloudflareDnsRecordBody<'a> {
    #[serde(rename = "type")]
    record_type: &'a str,
    name: &'a str,
    content: &'a str,
    proxied: bool,
}

fn cloudflare_config_for_response(mut config: CloudflareConfig) -> CloudflareConfig {
    config.tunnel_token_saved = super::secret::cloudflare_tunnel_token_exists();
    config
}

fn verification_result(
    state: &str,
    public_hostname: String,
    health_ok: bool,
    pair_challenge_ok: bool,
    websocket_ok: bool,
    access_state: &str,
    error_code: Option<&str>,
) -> CloudflareVerificationResult {
    CloudflareVerificationResult {
        state: state.to_string(),
        public_hostname,
        health_ok,
        pair_challenge_ok,
        websocket_ok,
        access_state: access_state.to_string(),
        error_code: error_code.map(str::to_string),
        checked_at: chrono::Utc::now().to_rfc3339(),
    }
}

fn update_cloudflare_verification(
    state: &State<'_, AppState>,
    result: CloudflareVerificationResult,
) -> Result<CloudflareConfig, String> {
    let mut config = state
        .config
        .lock()
        .map_err(|error| format!("获取配置失败: {}", error))?;
    config.cloudflare_config.last_verified_at = Some(result.checked_at.clone());
    config.cloudflare_config.last_verification = Some(result);
    config.cloudflare_config.tunnel_token_saved = super::secret::cloudflare_tunnel_token_exists();
    Ok(config.cloudflare_config.clone())
}

fn build_cloudflare_probe_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .redirect(Policy::none())
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())
}

fn build_cloudflare_api_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|error| error.to_string())
}

fn cloudflare_api_error<T>(context: &str, response: &CloudflareApiResponse<T>) -> String {
    let details = response
        .errors
        .iter()
        .map(|error| match error.code {
            Some(code) => format!("{code}:{}", error.message),
            None => error.message.clone(),
        })
        .collect::<Vec<_>>()
        .join("; ");
    if details.is_empty() {
        format!("{context}:cloudflare_api_failed")
    } else {
        format!("{context}:{details}")
    }
}

fn normalize_cloudflare_zone_name(zone_name: &str) -> Result<String, String> {
    let normalized = zone_name
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_matches('/')
        .to_ascii_lowercase();
    if normalized.is_empty() || normalized.contains('/') || !normalized.contains('.') {
        return Err("cloudflare_zone_invalid".to_string());
    }
    Ok(normalized)
}

fn build_cloudflare_hostname(zone_name: &str, subdomain: &str) -> Result<String, String> {
    let zone_name = normalize_cloudflare_zone_name(zone_name)?;
    let mut label = subdomain
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_matches('/')
        .to_ascii_lowercase();
    if label.ends_with(&format!(".{zone_name}")) {
        label.truncate(label.len() - zone_name.len() - 1);
    }
    if label.is_empty()
        || label.contains('/')
        || label.starts_with('.')
        || label.ends_with('.')
        || !label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '.')
        || label.split('.').any(|part| {
            part.is_empty() || part.starts_with('-') || part.ends_with('-') || part.len() > 63
        })
    {
        return Err("cloudflare_subdomain_invalid".to_string());
    }
    Ok(format!("{label}.{zone_name}"))
}

async fn cloudflare_get_zone(
    client: &reqwest::Client,
    api_token: &str,
    zone_name: &str,
) -> Result<CloudflareZone, String> {
    let response = client
        .get("https://api.cloudflare.com/client/v4/zones")
        .bearer_auth(api_token)
        .query(&[("name", zone_name), ("status", "active"), ("per_page", "1")])
        .send()
        .await
        .map_err(|error| format!("cloudflare_zone_lookup_failed:{error}"))?
        .json::<CloudflareApiResponse<Vec<CloudflareZone>>>()
        .await
        .map_err(|error| format!("cloudflare_zone_lookup_decode_failed:{error}"))?;
    if !response.success {
        return Err(cloudflare_api_error(
            "cloudflare_zone_lookup_failed",
            &response,
        ));
    }
    response
        .result
        .and_then(|mut zones| zones.pop())
        .ok_or_else(|| "cloudflare_zone_not_found".to_string())
}

async fn cloudflare_create_tunnel(
    client: &reqwest::Client,
    api_token: &str,
    account_id: &str,
    tunnel_name: &str,
) -> Result<CloudflareTunnel, String> {
    let url = format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/cfd_tunnel");
    let response = client
        .post(url)
        .bearer_auth(api_token)
        .json(&CreateCloudflareTunnelBody {
            name: tunnel_name,
            config_src: "cloudflare",
        })
        .send()
        .await
        .map_err(|error| format!("cloudflare_tunnel_create_failed:{error}"))?
        .json::<CloudflareApiResponse<CloudflareTunnel>>()
        .await
        .map_err(|error| format!("cloudflare_tunnel_create_decode_failed:{error}"))?;
    if !response.success {
        return Err(cloudflare_api_error(
            "cloudflare_tunnel_create_failed",
            &response,
        ));
    }
    response
        .result
        .ok_or_else(|| "cloudflare_tunnel_create_missing_result".to_string())
}

async fn cloudflare_configure_tunnel(
    client: &reqwest::Client,
    api_token: &str,
    account_id: &str,
    tunnel_id: &str,
    hostname: &str,
) -> Result<(), String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{account_id}/cfd_tunnel/{tunnel_id}/configurations"
    );
    let body = ConfigureCloudflareTunnelBody {
        config: CloudflareTunnelConfig {
            ingress: vec![
                CloudflareTunnelIngress {
                    hostname: Some(hostname),
                    service: "http://localhost:8080",
                    origin_request: Some(serde_json::json!({})),
                },
                CloudflareTunnelIngress {
                    hostname: None,
                    service: "http_status:404",
                    origin_request: None,
                },
            ],
        },
    };
    let response = client
        .put(url)
        .bearer_auth(api_token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("cloudflare_tunnel_configure_failed:{error}"))?
        .json::<CloudflareApiResponse<serde_json::Value>>()
        .await
        .map_err(|error| format!("cloudflare_tunnel_configure_decode_failed:{error}"))?;
    if response.success {
        Ok(())
    } else {
        Err(cloudflare_api_error(
            "cloudflare_tunnel_configure_failed",
            &response,
        ))
    }
}

async fn cloudflare_find_dns_record(
    client: &reqwest::Client,
    api_token: &str,
    zone_id: &str,
    hostname: &str,
) -> Result<Option<CloudflareDnsRecord>, String> {
    let url = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");
    let response = client
        .get(url)
        .bearer_auth(api_token)
        .query(&[("type", "CNAME"), ("name", hostname), ("per_page", "1")])
        .send()
        .await
        .map_err(|error| format!("cloudflare_dns_lookup_failed:{error}"))?
        .json::<CloudflareApiResponse<Vec<CloudflareDnsRecord>>>()
        .await
        .map_err(|error| format!("cloudflare_dns_lookup_decode_failed:{error}"))?;
    if !response.success {
        return Err(cloudflare_api_error(
            "cloudflare_dns_lookup_failed",
            &response,
        ));
    }
    Ok(response.result.and_then(|mut records| records.pop()))
}

async fn cloudflare_create_dns_record(
    client: &reqwest::Client,
    api_token: &str,
    zone_id: &str,
    hostname: &str,
    tunnel_id: &str,
) -> Result<CloudflareDnsRecord, String> {
    let url = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");
    let response = client
        .post(url)
        .bearer_auth(api_token)
        .json(&CreateCloudflareDnsRecordBody {
            record_type: "CNAME",
            name: hostname,
            content: &format!("{tunnel_id}.cfargotunnel.com"),
            proxied: true,
        })
        .send()
        .await
        .map_err(|error| format!("cloudflare_dns_create_failed:{error}"))?
        .json::<CloudflareApiResponse<CloudflareDnsRecord>>()
        .await
        .map_err(|error| format!("cloudflare_dns_create_decode_failed:{error}"))?;
    if !response.success {
        return Err(cloudflare_api_error(
            "cloudflare_dns_create_failed",
            &response,
        ));
    }
    response
        .result
        .ok_or_else(|| "cloudflare_dns_create_missing_result".to_string())
}

async fn cloudflare_upsert_dns_record(
    client: &reqwest::Client,
    api_token: &str,
    zone_id: &str,
    hostname: &str,
    tunnel_id: &str,
    overwrite_dns: bool,
) -> Result<(Option<String>, String), String> {
    let expected_content = format!("{tunnel_id}.cfargotunnel.com");
    if let Some(existing) = cloudflare_find_dns_record(client, api_token, zone_id, hostname).await?
    {
        if existing.content == expected_content {
            return Ok((Some(existing.id), "existing".to_string()));
        }
        if !overwrite_dns {
            return Err("cloudflare_dns_record_conflict".to_string());
        }
        let url = format!(
            "https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records/{}",
            existing.id
        );
        let response = client
            .put(url)
            .bearer_auth(api_token)
            .json(&CreateCloudflareDnsRecordBody {
                record_type: "CNAME",
                name: hostname,
                content: &expected_content,
                proxied: true,
            })
            .send()
            .await
            .map_err(|error| format!("cloudflare_dns_update_failed:{error}"))?
            .json::<CloudflareApiResponse<CloudflareDnsRecord>>()
            .await
            .map_err(|error| format!("cloudflare_dns_update_decode_failed:{error}"))?;
        if !response.success {
            return Err(cloudflare_api_error(
                "cloudflare_dns_update_failed",
                &response,
            ));
        }
        return Ok((
            response.result.map(|record| record.id),
            "updated".to_string(),
        ));
    }
    let record =
        cloudflare_create_dns_record(client, api_token, zone_id, hostname, tunnel_id).await?;
    Ok((Some(record.id), "created".to_string()))
}

fn normalize_access_emails(emails: &[String]) -> Result<Vec<String>, String> {
    let mut normalized = Vec::new();
    for email in emails {
        let email = email.trim().to_ascii_lowercase();
        if email.is_empty() {
            continue;
        }
        let valid = email.contains('@')
            && !email.starts_with('@')
            && !email.ends_with('@')
            && !email.contains('/')
            && !email.contains(char::is_whitespace);
        if !valid {
            return Err("cloudflare_access_email_invalid".to_string());
        }
        if !normalized.contains(&email) {
            normalized.push(email);
        }
    }
    Ok(normalized)
}

fn cloudflare_access_application_name(hostname: &str) -> String {
    format!("iterate web login {hostname}")
}

fn cloudflare_access_policy_name(hostname: &str) -> String {
    format!("iterate web login allow emails {hostname}")
}

fn cloudflare_access_application_body(hostname: &str) -> serde_json::Value {
    serde_json::json!({
        "name": cloudflare_access_application_name(hostname),
        "domain": hostname,
        "type": "self_hosted",
        "session_duration": "24h",
        "app_launcher_visible": false,
        "auto_redirect_to_identity": false
    })
}

fn cloudflare_access_application_managed_by_iterate(
    app: &CloudflareAccessApplication,
    hostname: &str,
) -> bool {
    app.name == cloudflare_access_application_name(hostname)
}

fn cloudflare_access_policy_body(hostname: &str, emails: &[String]) -> serde_json::Value {
    let include = emails
        .iter()
        .map(|email| serde_json::json!({ "email": { "email": email } }))
        .collect::<Vec<_>>();
    serde_json::json!({
        "name": cloudflare_access_policy_name(hostname),
        "decision": "allow",
        "precedence": 1,
        "include": include,
        "session_duration": "24h"
    })
}

async fn cloudflare_find_access_application(
    client: &reqwest::Client,
    api_token: &str,
    account_id: &str,
    hostname: &str,
) -> Result<Option<CloudflareAccessApplication>, String> {
    let url = format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/access/apps");
    let response = client
        .get(url)
        .bearer_auth(api_token)
        .query(&[("per_page", "100")])
        .send()
        .await
        .map_err(|error| format!("cloudflare_access_app_lookup_failed:{error}"))?
        .json::<CloudflareApiResponse<Vec<CloudflareAccessApplication>>>()
        .await
        .map_err(|error| format!("cloudflare_access_app_lookup_decode_failed:{error}"))?;
    if !response.success {
        return Err(cloudflare_api_error(
            "cloudflare_access_app_lookup_failed",
            &response,
        ));
    }
    Ok(response.result.and_then(|apps| {
        apps.into_iter().find(|app| {
            app.domain.as_deref() == Some(hostname)
                && app.application_type.as_deref() == Some("self_hosted")
        })
    }))
}

async fn cloudflare_upsert_access_application(
    client: &reqwest::Client,
    api_token: &str,
    account_id: &str,
    hostname: &str,
) -> Result<CloudflareAccessApplication, String> {
    let body = cloudflare_access_application_body(hostname);
    if let Some(existing) =
        cloudflare_find_access_application(client, api_token, account_id, hostname).await?
    {
        if !cloudflare_access_application_managed_by_iterate(&existing, hostname) {
            return Err(format!(
                "cloudflare_access_app_conflict_existing_unmanaged_app:{hostname}"
            ));
        }
        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/access/apps/{}",
            existing.id
        );
        let response = client
            .put(url)
            .bearer_auth(api_token)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("cloudflare_access_app_update_failed:{error}"))?
            .json::<CloudflareApiResponse<CloudflareAccessApplication>>()
            .await
            .map_err(|error| format!("cloudflare_access_app_update_decode_failed:{error}"))?;
        if !response.success {
            return Err(cloudflare_api_error(
                "cloudflare_access_app_update_failed",
                &response,
            ));
        }
        return response
            .result
            .ok_or_else(|| "cloudflare_access_app_update_missing_result".to_string());
    }

    let url = format!("https://api.cloudflare.com/client/v4/accounts/{account_id}/access/apps");
    let response = client
        .post(url)
        .bearer_auth(api_token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("cloudflare_access_app_create_failed:{error}"))?
        .json::<CloudflareApiResponse<CloudflareAccessApplication>>()
        .await
        .map_err(|error| format!("cloudflare_access_app_create_decode_failed:{error}"))?;
    if !response.success {
        return Err(cloudflare_api_error(
            "cloudflare_access_app_create_failed",
            &response,
        ));
    }
    response
        .result
        .ok_or_else(|| "cloudflare_access_app_create_missing_result".to_string())
}

async fn cloudflare_find_access_policy(
    client: &reqwest::Client,
    api_token: &str,
    account_id: &str,
    app_id: &str,
    policy_name: &str,
) -> Result<Option<CloudflareAccessPolicy>, String> {
    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{account_id}/access/apps/{app_id}/policies"
    );
    let response = client
        .get(url)
        .bearer_auth(api_token)
        .query(&[("per_page", "100")])
        .send()
        .await
        .map_err(|error| format!("cloudflare_access_policy_lookup_failed:{error}"))?
        .json::<CloudflareApiResponse<Vec<CloudflareAccessPolicy>>>()
        .await
        .map_err(|error| format!("cloudflare_access_policy_lookup_decode_failed:{error}"))?;
    if !response.success {
        return Err(cloudflare_api_error(
            "cloudflare_access_policy_lookup_failed",
            &response,
        ));
    }
    Ok(response.result.and_then(|policies| {
        policies
            .into_iter()
            .find(|policy| policy.name == policy_name)
    }))
}

async fn cloudflare_upsert_access_policy(
    client: &reqwest::Client,
    api_token: &str,
    account_id: &str,
    app_id: &str,
    hostname: &str,
    emails: &[String],
) -> Result<CloudflareAccessPolicy, String> {
    let policy_name = cloudflare_access_policy_name(hostname);
    let body = cloudflare_access_policy_body(hostname, emails);
    if let Some(existing) =
        cloudflare_find_access_policy(client, api_token, account_id, app_id, &policy_name).await?
    {
        let url = format!(
            "https://api.cloudflare.com/client/v4/accounts/{account_id}/access/apps/{app_id}/policies/{}",
            existing.id
        );
        let response = client
            .put(url)
            .bearer_auth(api_token)
            .json(&body)
            .send()
            .await
            .map_err(|error| format!("cloudflare_access_policy_update_failed:{error}"))?
            .json::<CloudflareApiResponse<CloudflareAccessPolicy>>()
            .await
            .map_err(|error| format!("cloudflare_access_policy_update_decode_failed:{error}"))?;
        if !response.success {
            return Err(cloudflare_api_error(
                "cloudflare_access_policy_update_failed",
                &response,
            ));
        }
        return response
            .result
            .ok_or_else(|| "cloudflare_access_policy_update_missing_result".to_string());
    }

    let url = format!(
        "https://api.cloudflare.com/client/v4/accounts/{account_id}/access/apps/{app_id}/policies"
    );
    let response = client
        .post(url)
        .bearer_auth(api_token)
        .json(&body)
        .send()
        .await
        .map_err(|error| format!("cloudflare_access_policy_create_failed:{error}"))?
        .json::<CloudflareApiResponse<CloudflareAccessPolicy>>()
        .await
        .map_err(|error| format!("cloudflare_access_policy_create_decode_failed:{error}"))?;
    if !response.success {
        return Err(cloudflare_api_error(
            "cloudflare_access_policy_create_failed",
            &response,
        ));
    }
    response
        .result
        .ok_or_else(|| "cloudflare_access_policy_create_missing_result".to_string())
}

async fn local_cloudflare_origin_ready() -> bool {
    let Ok(client) = build_cloudflare_probe_client() else {
        return false;
    };
    let Ok(response) = client
        .get("http://127.0.0.1:8080/.well-known/iterate/health")
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<serde_json::Value>()
        .await
        .map(|payload| cloudflare_health_payload_valid(&payload))
        .unwrap_or(false)
}

fn value_bool(value: &serde_json::Value, key: &str) -> bool {
    value.get(key).and_then(|field| field.as_bool()) == Some(true)
}

fn value_str_eq(value: &serde_json::Value, key: &str, expected: &str) -> bool {
    value.get(key).and_then(|field| field.as_str()) == Some(expected)
}

fn value_str_present(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(|field| field.as_str())
        .is_some_and(|field| !field.trim().is_empty())
}

fn value_rfc3339_present(value: &serde_json::Value, key: &str) -> bool {
    value
        .get(key)
        .and_then(|field| field.as_str())
        .is_some_and(|field| chrono::DateTime::parse_from_rfc3339(field).is_ok())
}

fn cloudflare_health_payload_valid(payload: &serde_json::Value) -> bool {
    let Some(capabilities) = payload.get("capabilities") else {
        return false;
    };
    value_bool(payload, "ok")
        && value_str_eq(payload, "service", "iterate")
        && value_str_present(payload, "version")
        && value_str_eq(payload, "public_surface", "cloudflare_web_login")
        && value_bool(capabilities, "pair_challenge")
        && value_bool(capabilities, "websocket")
}

fn pair_challenge_payload_valid(payload: &serde_json::Value) -> bool {
    let challenge_ok = payload
        .get("challenge")
        .and_then(|field| field.as_str())
        .is_some_and(|challenge| challenge.starts_with("pc_") && challenge.len() > 16);
    value_bool(payload, "ok")
        && challenge_ok
        && value_str_eq(payload, "scope", "pair_challenge")
        && payload
            .get("session_issued")
            .and_then(|field| field.as_bool())
            == Some(false)
        && value_rfc3339_present(payload, "issued_at")
        && value_rfc3339_present(payload, "expires_at")
}

fn cloudflare_config_change_invalidates_verification(
    config: &CloudflareConfig,
    public_hostname: &str,
    access_expected: bool,
    token_provided: bool,
) -> bool {
    config.public_hostname != public_hostname
        || config.access_expected != access_expected
        || token_provided
}

fn normalize_optional_web_login_console_origin(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    crate::bridge::ws::normalize_web_origin(trimmed)
}

fn cloudflare_verification_is_fresh(
    verification: &CloudflareVerificationResult,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    chrono::DateTime::parse_from_rfc3339(&verification.checked_at)
        .ok()
        .map(|checked_at| checked_at.with_timezone(&chrono::Utc))
        .filter(|checked_at| *checked_at <= now)
        .is_some_and(|checked_at| {
            now.signed_duration_since(checked_at)
                <= chrono::Duration::seconds(CLOUDFLARE_VERIFICATION_FRESHNESS_SECS)
        })
}

fn cloudflare_pairing_gate_error(
    config: &CloudflareConfig,
    public_hostname: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<&'static str> {
    if config.web_login_console_origin.trim().is_empty() {
        return Some("web_login_console_origin_missing");
    }
    let Some(verification) = config.last_verification.as_ref() else {
        return Some("cloudflare_not_verified");
    };
    if verification.state != "verified" {
        return Some("cloudflare_not_verified");
    }
    if verification.public_hostname != public_hostname {
        return Some("cloudflare_verification_stale");
    }
    if !cloudflare_verification_is_fresh(verification, now) {
        return Some("cloudflare_verification_stale");
    }
    None
}

async fn probe_public_health(public_hostname: &str) -> bool {
    let Ok(client) = build_cloudflare_probe_client() else {
        return false;
    };
    let url = format!("{public_hostname}/.well-known/iterate/health");
    let Ok(response) = client.get(url).send().await else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<serde_json::Value>()
        .await
        .map(|payload| cloudflare_health_payload_valid(&payload))
        .unwrap_or(false)
}

async fn probe_cloudflare_access_gate(public_hostname: &str) -> bool {
    let Ok(client) = build_cloudflare_probe_client() else {
        return false;
    };
    let url = format!("{public_hostname}/.well-known/iterate/health");
    let Ok(response) = client.get(url).send().await else {
        return false;
    };
    let status = response.status();
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    status.is_redirection()
        && (location.contains("/cdn-cgi/access/") || location.contains("cloudflareaccess.com"))
}

async fn probe_pair_challenge(public_hostname: &str) -> bool {
    let Ok(client) = build_cloudflare_probe_client() else {
        return false;
    };
    let url = format!("{public_hostname}/pair/challenge");
    let Ok(response) = client
        .post(url)
        .json(&serde_json::json!({ "source": "settings_verify" }))
        .send()
        .await
    else {
        return false;
    };
    if !response.status().is_success() {
        return false;
    }
    response
        .json::<serde_json::Value>()
        .await
        .map(|payload| pair_challenge_payload_valid(&payload))
        .unwrap_or(false)
}

fn websocket_error_is_iterate_auth_required(error: &WsError) -> bool {
    let WsError::Http(response) = error else {
        return false;
    };
    if response.status().as_u16() != 401 {
        return false;
    }
    let Some(body) = response.body().as_ref() else {
        return false;
    };
    let Ok(body_text) = std::str::from_utf8(body) else {
        return false;
    };
    serde_json::from_str::<serde_json::Value>(body_text)
        .ok()
        .and_then(|payload| {
            payload
                .get("error")
                .and_then(|error| error.as_str())
                .map(str::to_string)
        })
        .is_some_and(|error| error == "mobile_auth_required")
}

async fn probe_websocket(public_hostname: &str) -> bool {
    let ws_url = public_hostname.replacen("https://", "wss://", 1) + "/ws";
    match tokio::time::timeout(std::time::Duration::from_secs(5), connect_async(&ws_url)).await {
        Ok(Ok((stream, _))) => {
            drop(stream);
            true
        }
        Ok(Err(error)) => websocket_error_is_iterate_auth_required(&error),
        Err(_) => false,
    }
}

/// 启动远程隧道
#[tauri::command]
pub async fn start_remote_tunnel() -> Result<String, String> {
    log::info!("启动远程隧道...");
    manager::start_tunnel().await
}

/// 停止远程隧道
#[tauri::command]
pub async fn stop_remote_tunnel() -> Result<String, String> {
    log::info!("停止远程隧道...");
    manager::stop_tunnel().await
}

/// 获取远程隧道状态
#[tauri::command]
pub async fn get_remote_tunnel_status() -> Result<TunnelStatus, String> {
    Ok(manager::get_status().await)
}

async fn quick_tunnel_bridge_request(
    method: reqwest::Method,
    path: &str,
    body: Option<serde_json::Value>,
) -> Result<manager::QuickTunnelStatus, String> {
    let url = format!("http://127.0.0.1:8080{path}");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(35))
        .no_proxy()
        .build()
        .map_err(|_| "bridge_or_mcp_not_ready".to_string())?;
    let method_name = method.as_str().to_string();
    let mut request = client.request(method, &url);
    if let Some(body) = body {
        request = request.json(&body);
    }
    request = crate::bridge::auth::authorize_internal_bridge_request(request, &method_name, &url)
        .map_err(|_| "bridge_or_mcp_not_ready".to_string())?;
    let response = request
        .send()
        .await
        .map_err(|_| "bridge_or_mcp_not_ready".to_string())?;
    let status = response.status();
    let payload = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| "bridge_or_mcp_not_ready".to_string())?;
    if !status.is_success() {
        return Err(payload
            .get("error")
            .and_then(|value| value.as_str())
            .unwrap_or("bridge_or_mcp_not_ready")
            .to_string());
    }
    serde_json::from_value(
        payload
            .get("status")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|_| "bridge_or_mcp_not_ready".to_string())
}

#[tauri::command]
pub async fn start_quick_tunnel(consent_v1: bool) -> Result<manager::QuickTunnelStatus, String> {
    quick_tunnel_bridge_request(
        reqwest::Method::POST,
        "/api/quick-tunnel/start",
        Some(serde_json::json!({ "consent_v1": consent_v1 })),
    )
    .await
}

#[tauri::command]
pub async fn stop_quick_tunnel() -> Result<manager::QuickTunnelStatus, String> {
    quick_tunnel_bridge_request(
        reqwest::Method::POST,
        "/api/quick-tunnel/stop",
        Some(serde_json::json!({})),
    )
    .await
}

#[tauri::command]
pub async fn get_quick_tunnel_status() -> Result<manager::QuickTunnelStatus, String> {
    quick_tunnel_bridge_request(reqwest::Method::GET, "/api/quick-tunnel/status", None).await
}

/// 检查本机 8080 端口健康状态
#[tauri::command]
pub async fn check_origin_health() -> Result<bool, String> {
    manager::check_origin_health().await
}

fn publicly_routable_ipv4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !matches!(a, 0 | 10 | 127 | 224..=255)
        && !(a == 100 && (64..=127).contains(&b))
        && !(a == 169 && b == 254)
        && !(a == 172 && (16..=31).contains(&b))
        && !(a == 192 && ((b == 0 && (c == 0 || c == 2)) || (b == 88 && c == 99) || b == 168))
        && !(a == 198 && ((b == 18 || b == 19) || (b == 51 && c == 100)))
        && !(a == 203 && b == 0 && c == 113)
}

pub(crate) fn aliyun_ip_origin_is_public(value: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(value.trim().trim_end_matches('/')) else {
        return false;
    };
    parsed.scheme() == "https"
        && parsed.path() == "/"
        && parsed.query().is_none()
        && parsed.fragment().is_none()
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed
            .host_str()
            .and_then(|host| host.parse::<std::net::Ipv4Addr>().ok())
            .is_some_and(publicly_routable_ipv4)
}

fn normalize_formal_mobile_route_base_url(transport: &str, value: &str) -> Result<String, String> {
    if transport == "aliyun_ssh_reverse_tunnel" && !aliyun_ip_origin_is_public(value) {
        return Err("formal_route_ip_origin_invalid".to_string());
    }
    manager::normalize_public_hostname(value)
}

pub fn configured_formal_mobile_route() -> Option<FormalMobileRouteConfig> {
    let config = load_standalone_config().ok()?;
    if let Some(route) = config.mobile_config.formal_route {
        if route.schema_version == 1
            && matches!(
                route.transport.as_str(),
                "cloudflare_named_tunnel" | "aliyun_ssh_reverse_tunnel"
            )
            && normalize_formal_mobile_route_base_url(&route.transport, &route.base_url).is_ok()
        {
            return Some(route);
        }
    }

    if !config.cloudflare_config.guided_setup_enabled {
        return None;
    }
    let base_url =
        manager::normalize_public_hostname(&config.cloudflare_config.public_hostname).ok()?;
    let configured_at = config
        .cloudflare_config
        .last_verified_at
        .clone()
        .unwrap_or_default();
    Some(FormalMobileRouteConfig {
        schema_version: 1,
        transport: "cloudflare_named_tunnel".to_string(),
        base_url,
        configured_at,
        source: "legacy_cloudflare_guided".to_string(),
        formal_route_generation: 1,
        last_verified_at: config.cloudflare_config.last_verified_at,
        last_verification: config
            .cloudflare_config
            .last_verification
            .map(|verification| FormalMobileRouteVerification {
                https_ok: verification.health_ok && verification.pair_challenge_ok,
                websocket_ok: verification.websocket_ok,
                endpoint_identity_ok: verification.health_ok
                    && verification.pair_challenge_ok
                    && verification.websocket_ok,
                checked_at: verification.checked_at,
                error_code: verification.error_code,
            }),
    })
}

fn unconfigured_formal_mobile_route_status() -> FormalMobileRouteStatus {
    FormalMobileRouteStatus {
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
    }
}

async fn probe_formal_mobile_route(route: &FormalMobileRouteConfig) -> FormalMobileRouteStatus {
    let checked_at = chrono::Utc::now().to_rfc3339();
    let origin_healthy = manager::check_origin_health().await.unwrap_or(false);
    let endpoint_identity_ok =
        origin_healthy && manager::public_endpoint_proves_current_install(&route.base_url).await;
    let repair_reason = if endpoint_identity_ok {
        None
    } else if !origin_healthy {
        Some("bridge_unhealthy".to_string())
    } else {
        Some("endpoint_unreachable".to_string())
    };
    FormalMobileRouteStatus {
        configured: true,
        transport: Some(route.transport.clone()),
        base_url: Some(route.base_url.clone()),
        configured_at: Some(route.configured_at.clone()),
        formal_route_generation: Some(route.formal_route_generation.max(1)),
        health: if endpoint_identity_ok {
            "healthy".to_string()
        } else {
            "degraded".to_string()
        },
        health_checked_at: Some(checked_at),
        last_verified_at: route.last_verified_at.clone(),
        endpoint_identity_ok,
        repair_reason,
    }
}

pub async fn get_formal_mobile_route_status() -> FormalMobileRouteStatus {
    match configured_formal_mobile_route() {
        Some(route) => probe_formal_mobile_route(&route).await,
        None => unconfigured_formal_mobile_route_status(),
    }
}

pub async fn register_formal_mobile_route(
    transport: &str,
    base_url: &str,
    source: &str,
) -> Result<FormalMobileRouteStatus, String> {
    if !matches!(
        transport,
        "cloudflare_named_tunnel" | "aliyun_ssh_reverse_tunnel"
    ) {
        return Err("formal_route_transport_not_supported".to_string());
    }
    let source = match source.trim() {
        "ai_configured" => "ai_configured",
        "manual_adopt" => "manual_adopt",
        "legacy_migration" => "legacy_migration",
        _ => return Err("formal_route_source_invalid".to_string()),
    };
    let base_url = normalize_formal_mobile_route_base_url(transport, base_url)?;
    if !manager::check_origin_health().await? {
        return Err("bridge_unhealthy".to_string());
    }
    if !manager::public_endpoint_proves_current_install(&base_url).await {
        return Err("endpoint_identity_mismatch".to_string());
    }

    let checked_at = chrono::Utc::now().to_rfc3339();
    let mut config = load_standalone_config().map_err(|error| error.to_string())?;
    let previous = config.mobile_config.formal_route.as_ref();
    let generation = previous
        .map(|route| {
            if route.transport == transport && route.base_url == base_url {
                route.formal_route_generation.max(1)
            } else {
                route.formal_route_generation.saturating_add(1).max(1)
            }
        })
        .unwrap_or(1);
    let route = FormalMobileRouteConfig {
        schema_version: 1,
        transport: transport.to_string(),
        base_url: base_url.clone(),
        configured_at: previous
            .filter(|route| route.transport == transport && route.base_url == base_url)
            .map(|route| route.configured_at.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| checked_at.clone()),
        source: source.to_string(),
        formal_route_generation: generation,
        last_verified_at: Some(checked_at.clone()),
        last_verification: Some(FormalMobileRouteVerification {
            https_ok: true,
            websocket_ok: true,
            endpoint_identity_ok: true,
            checked_at: checked_at.clone(),
            error_code: None,
        }),
    };
    config.mobile_config.formal_route = Some(route.clone());
    save_standalone_config(&config).map_err(|error| error.to_string())?;
    Ok(FormalMobileRouteStatus {
        configured: true,
        transport: Some(route.transport),
        base_url: Some(route.base_url),
        configured_at: Some(route.configured_at),
        formal_route_generation: Some(route.formal_route_generation.max(1)),
        health: "healthy".to_string(),
        health_checked_at: Some(checked_at.clone()),
        last_verified_at: Some(checked_at),
        endpoint_identity_ok: true,
        repair_reason: None,
    })
}

pub async fn verify_formal_mobile_route() -> Result<FormalMobileRouteStatus, String> {
    let Some(route) = configured_formal_mobile_route() else {
        return Ok(unconfigured_formal_mobile_route_status());
    };
    let status = probe_formal_mobile_route(&route).await;
    let checked_at = status
        .health_checked_at
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    let mut config = load_standalone_config().map_err(|error| error.to_string())?;
    if let Some(stored) = config.mobile_config.formal_route.as_mut() {
        if status.endpoint_identity_ok {
            stored.last_verified_at = Some(checked_at.clone());
        }
        stored.last_verification = Some(FormalMobileRouteVerification {
            https_ok: status.endpoint_identity_ok,
            websocket_ok: status.endpoint_identity_ok,
            endpoint_identity_ok: status.endpoint_identity_ok,
            checked_at,
            error_code: status.repair_reason.clone(),
        });
        save_standalone_config(&config).map_err(|error| error.to_string())?;
    }
    Ok(status)
}

#[tauri::command]
pub async fn recover_bridge_origin(
) -> Result<crate::app::setup::BridgeOriginRecoveryResponse, String> {
    Ok(crate::app::setup::recover_bridge_origin().await)
}

#[tauri::command]
pub async fn get_cloudflare_guided_config(
    state: State<'_, AppState>,
) -> Result<CloudflareGuidedConfigResponse, String> {
    let config = state
        .config
        .lock()
        .map_err(|error| format!("获取配置失败: {}", error))?
        .cloudflare_config
        .clone();
    Ok(CloudflareGuidedConfigResponse {
        config: cloudflare_config_for_response(config),
    })
}

#[tauri::command]
pub async fn save_cloudflare_guided_config(
    request: SaveCloudflareGuidedConfigRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<CloudflareGuidedConfigResponse, String> {
    let public_hostname = manager::normalize_public_hostname(&request.public_hostname)?;
    let web_login_console_origin =
        normalize_optional_web_login_console_origin(&request.web_login_console_origin)?;
    let token_provided = request
        .tunnel_token
        .as_deref()
        .is_some_and(|token| !token.trim().is_empty());
    if token_provided {
        if let Some(token) = request.tunnel_token.as_deref() {
            super::secret::save_cloudflare_tunnel_token(token)?;
        }
    }

    {
        let mut config = state
            .config
            .lock()
            .map_err(|error| format!("获取配置失败: {}", error))?;
        let invalidate_verification = cloudflare_config_change_invalidates_verification(
            &config.cloudflare_config,
            &public_hostname,
            request.access_expected,
            token_provided,
        );
        config.cloudflare_config.guided_setup_enabled = true;
        config.cloudflare_config.public_hostname = public_hostname;
        config.cloudflare_config.access_expected = request.access_expected;
        config.cloudflare_config.web_login_console_origin = web_login_console_origin;
        config.cloudflare_config.tunnel_token_saved =
            super::secret::cloudflare_tunnel_token_exists();
        if invalidate_verification {
            config.cloudflare_config.last_verified_at = None;
            config.cloudflare_config.last_verification = None;
        }
    }

    save_config(&state, &app)
        .await
        .map_err(|error| format!("保存 Cloudflare 配置失败: {}", error))?;
    get_cloudflare_guided_config(state).await
}

#[tauri::command]
pub async fn clear_cloudflare_guided_config(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<CloudflareGuidedConfigResponse, String> {
    super::secret::delete_cloudflare_tunnel_token()?;
    {
        let mut config = state
            .config
            .lock()
            .map_err(|error| format!("获取配置失败: {}", error))?;
        config.cloudflare_config = crate::config::default_cloudflare_config();
    }
    save_config(&state, &app)
        .await
        .map_err(|error| format!("保存 Cloudflare 配置失败: {}", error))?;
    get_cloudflare_guided_config(state).await
}

#[tauri::command]
pub async fn start_cloudflare_customer_tunnel(
    state: State<'_, AppState>,
) -> Result<String, String> {
    let public_hostname = state
        .config
        .lock()
        .map_err(|error| format!("获取配置失败: {}", error))?
        .cloudflare_config
        .public_hostname
        .clone();
    let public_hostname = manager::normalize_public_hostname(&public_hostname)?;
    let token =
        super::secret::read_cloudflare_tunnel_token().map_err(|_| "token_missing".to_string())?;
    manager::start_customer_tunnel(public_hostname, token).await
}

#[tauri::command]
pub async fn stop_cloudflare_customer_tunnel() -> Result<String, String> {
    manager::stop_tunnel().await
}

#[tauri::command]
pub async fn get_cloudflare_customer_tunnel_status() -> Result<TunnelStatus, String> {
    Ok(manager::get_status().await)
}

#[tauri::command]
pub async fn create_cloudflare_web_login_auto_setup(
    request: CreateCloudflareWebLoginAutoSetupRequest,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<CloudflareWebLoginAutoSetupResponse, String> {
    let result =
        create_cloudflare_web_login_auto_setup_core(CloudflareWebLoginAutoSetupCoreRequest {
            api_token: request.api_token,
            zone_name: request.zone_name,
            subdomain: request.subdomain,
            overwrite_dns: request.overwrite_dns,
            access_emails: request.access_emails,
        })
        .await?;

    {
        let mut config = state
            .config
            .lock()
            .map_err(|error| format!("获取配置失败: {}", error))?;
        apply_cloudflare_auto_setup_result(&mut config, &result);
    }
    save_config(&state, &app)
        .await
        .map_err(|error| format!("保存 Cloudflare 自动验证结果失败: {}", error))?;

    Ok(result)
}

pub async fn create_cloudflare_web_login_auto_setup_headless(
    request: CloudflareWebLoginAutoSetupCoreRequest,
) -> Result<CloudflareWebLoginHeadlessSmokeResponse, String> {
    let result = create_cloudflare_web_login_auto_setup_core(request).await?;
    let mut config = load_standalone_config()
        .map_err(|error| format!("读取 Cloudflare standalone 配置失败: {error}"))?;
    apply_cloudflare_auto_setup_result(&mut config, &result);
    save_standalone_config(&config)
        .map_err(|error| format!("保存 Cloudflare standalone 配置失败: {error}"))?;

    let pairing_gate_ok = cloudflare_pairing_gate_error(
        &config.cloudflare_config,
        &result.public_hostname,
        chrono::Utc::now(),
    )
    .is_none();
    let ok = result.verification.state == "verified" && pairing_gate_ok;

    Ok(CloudflareWebLoginHeadlessSmokeResponse {
        ok,
        public_hostname: result.public_hostname,
        tunnel_id: result.tunnel_id,
        tunnel_name: result.tunnel_name,
        dns_record_id: result.dns_record_id,
        dns_action: result.dns_action,
        access_app_id: result.access_app_id,
        access_policy_id: result.access_policy_id,
        access_state: result.access_state,
        verification: result.verification,
        pairing_gate_ok,
    })
}

fn apply_cloudflare_auto_setup_result(
    config: &mut AppConfig,
    result: &CloudflareWebLoginAutoSetupResponse,
) {
    config.cloudflare_config.guided_setup_enabled = true;
    config.cloudflare_config.public_hostname = result.public_hostname.clone();
    config.cloudflare_config.access_expected = result.access_state == "configured";
    if config
        .cloudflare_config
        .web_login_console_origin
        .trim()
        .is_empty()
    {
        config.cloudflare_config.web_login_console_origin = result.public_hostname.clone();
    }
    config.cloudflare_config.tunnel_token_saved = super::secret::cloudflare_tunnel_token_exists();
    config.cloudflare_config.last_verified_at = Some(result.verification.checked_at.clone());
    config.cloudflare_config.last_verification = Some(result.verification.clone());
}

async fn create_cloudflare_web_login_auto_setup_core(
    request: CloudflareWebLoginAutoSetupCoreRequest,
) -> Result<CloudflareWebLoginAutoSetupResponse, String> {
    let api_token = request.api_token.trim();
    if api_token.is_empty() {
        return Err("cloudflare_api_token_missing".to_string());
    }
    let access_emails = normalize_access_emails(&request.access_emails)?;
    if !local_cloudflare_origin_ready().await {
        return Err("cloudflare_origin_health_failed".to_string());
    }

    let zone_name = normalize_cloudflare_zone_name(&request.zone_name)?;
    let hostname = build_cloudflare_hostname(&zone_name, &request.subdomain)?;
    let public_hostname = format!("https://{hostname}");
    let client = build_cloudflare_api_client()?;
    let zone = cloudflare_get_zone(&client, api_token, &zone_name).await?;
    let tunnel_name = format!(
        "iterate-web-login-{}-{}",
        hostname.replace('.', "-"),
        chrono::Utc::now().format("%Y%m%d%H%M%S")
    );
    let tunnel =
        cloudflare_create_tunnel(&client, api_token, &zone.account.id, &tunnel_name).await?;
    cloudflare_configure_tunnel(&client, api_token, &zone.account.id, &tunnel.id, &hostname)
        .await?;
    let (dns_record_id, dns_action) = cloudflare_upsert_dns_record(
        &client,
        api_token,
        &zone.id,
        &hostname,
        &tunnel.id,
        request.overwrite_dns,
    )
    .await?;

    super::secret::save_cloudflare_tunnel_token(&tunnel.token)?;

    manager::start_customer_tunnel(public_hostname.clone(), tunnel.token.clone()).await?;

    let mut verification = verification_result(
        "verification_pending",
        public_hostname.clone(),
        false,
        false,
        false,
        "not_required",
        Some("verification_pending"),
    );
    for _ in 0..12 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let health_ok = probe_public_health(&public_hostname).await;
        if !health_ok {
            verification = verification_result(
                "health_failed",
                public_hostname.clone(),
                false,
                false,
                false,
                "not_required",
                Some("health_failed"),
            );
            continue;
        }
        let pair_challenge_ok = probe_pair_challenge(&public_hostname).await;
        if !pair_challenge_ok {
            verification = verification_result(
                "pair_challenge_failed",
                public_hostname.clone(),
                true,
                false,
                false,
                "not_required",
                Some("pair_challenge_failed"),
            );
            continue;
        }
        let websocket_ok = probe_websocket(&public_hostname).await;
        verification = if websocket_ok {
            verification_result(
                "verified",
                public_hostname.clone(),
                true,
                true,
                true,
                "not_required",
                None,
            )
        } else {
            verification_result(
                "websocket_failed",
                public_hostname.clone(),
                true,
                true,
                false,
                "not_required",
                Some("websocket_failed"),
            )
        };
        if verification.state == "verified" {
            break;
        }
    }

    let mut access_app_id = None;
    let mut access_policy_id = None;
    let mut access_state = "not_configured_by_iterate".to_string();
    if !access_emails.is_empty() {
        if verification.state != "verified" {
            return Err("cloudflare_access_requires_verified_tunnel".to_string());
        }
        let access_app =
            cloudflare_upsert_access_application(&client, api_token, &zone.account.id, &hostname)
                .await?;
        let access_policy = cloudflare_upsert_access_policy(
            &client,
            api_token,
            &zone.account.id,
            &access_app.id,
            &hostname,
            &access_emails,
        )
        .await?;
        access_app_id = Some(access_app.id);
        access_policy_id = Some(access_policy.id);
        access_state = "configured".to_string();
        verification.access_state = access_state.clone();
    }

    Ok(CloudflareWebLoginAutoSetupResponse {
        public_hostname,
        tunnel_id: tunnel.id,
        tunnel_name: tunnel.name,
        dns_record_id,
        dns_action,
        access_app_id,
        access_policy_id,
        access_state,
        verification,
    })
}

#[tauri::command]
pub async fn create_cloudflare_web_login_pairing(
    state: State<'_, AppState>,
) -> Result<CloudflareWebLoginPairingResponse, String> {
    let config = state
        .config
        .lock()
        .map_err(|error| format!("获取配置失败: {}", error))?
        .cloudflare_config
        .clone();
    let public_hostname = manager::normalize_public_hostname(&config.public_hostname)?;
    if !super::secret::cloudflare_tunnel_token_exists() {
        return Err("token_missing".to_string());
    }
    if let Some(error) =
        cloudflare_pairing_gate_error(&config, &public_hostname, chrono::Utc::now())
    {
        return Err(error.to_string());
    }
    let console_origin = crate::bridge::ws::normalize_web_origin(&config.web_login_console_origin)?;
    let pairing =
        crate::bridge::ws::issue_cloudflare_web_login_pairing(public_hostname, console_origin)
            .await?;
    Ok(CloudflareWebLoginPairingResponse { pairing })
}

#[tauri::command]
pub async fn list_cloudflare_web_login_sessions(
) -> Result<CloudflareWebLoginSessionsResponse, String> {
    Ok(CloudflareWebLoginSessionsResponse {
        sessions: crate::bridge::ws::list_web_login_sessions().await,
    })
}

#[tauri::command]
pub async fn revoke_cloudflare_web_login_sessions(
) -> Result<CloudflareWebLoginRevokeSessionsResponse, String> {
    Ok(CloudflareWebLoginRevokeSessionsResponse {
        ok: true,
        revoked: crate::bridge::ws::revoke_all_web_login_sessions().await,
    })
}

#[tauri::command]
pub async fn verify_cloudflare_guided_config(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<CloudflareVerificationResult, String> {
    let config = state
        .config
        .lock()
        .map_err(|error| format!("获取配置失败: {}", error))?
        .cloudflare_config
        .clone();
    let public_hostname = match manager::normalize_public_hostname(&config.public_hostname) {
        Ok(value) => value,
        Err(error_code) => {
            let result = verification_result(
                &error_code,
                config.public_hostname,
                false,
                false,
                false,
                "unknown",
                Some(&error_code),
            );
            let _ = update_cloudflare_verification(&state, result.clone())?;
            let _ = save_config(&state, &app).await;
            return Ok(result);
        }
    };

    if !super::secret::cloudflare_tunnel_token_exists() {
        let result = verification_result(
            "token_missing",
            public_hostname,
            false,
            false,
            false,
            "unknown",
            Some("token_missing"),
        );
        let _ = update_cloudflare_verification(&state, result.clone())?;
        let _ = save_config(&state, &app).await;
        return Ok(result);
    }

    if config.access_expected && probe_cloudflare_access_gate(&public_hostname).await {
        let result = verification_result(
            "verified",
            public_hostname,
            false,
            false,
            false,
            "access_enabled",
            None,
        );
        let _ = update_cloudflare_verification(&state, result.clone())?;
        save_config(&state, &app)
            .await
            .map_err(|error| format!("保存 Cloudflare 验证结果失败: {}", error))?;
        return Ok(result);
    }

    let health_ok = probe_public_health(&public_hostname).await;
    if !health_ok {
        let result = verification_result(
            "health_failed",
            public_hostname,
            false,
            false,
            false,
            "unknown",
            Some("health_failed"),
        );
        let _ = update_cloudflare_verification(&state, result.clone())?;
        let _ = save_config(&state, &app).await;
        return Ok(result);
    }

    let pair_challenge_ok = probe_pair_challenge(&public_hostname).await;
    if !pair_challenge_ok {
        let result = verification_result(
            "pair_challenge_failed",
            public_hostname,
            true,
            false,
            false,
            "unknown",
            Some("pair_challenge_failed"),
        );
        let _ = update_cloudflare_verification(&state, result.clone())?;
        let _ = save_config(&state, &app).await;
        return Ok(result);
    }

    let websocket_ok = probe_websocket(&public_hostname).await;
    let (state_name, access_state, error_code) = if config.access_expected {
        (
            "access_expected_missing",
            "not_detected",
            Some("access_expected_missing"),
        )
    } else if websocket_ok {
        ("verified", "not_required", None)
    } else {
        ("websocket_failed", "not_required", Some("websocket_failed"))
    };
    let result = verification_result(
        state_name,
        public_hostname,
        true,
        true,
        websocket_ok,
        access_state,
        error_code,
    );
    let _ = update_cloudflare_verification(&state, result.clone())?;
    save_config(&state, &app)
        .await
        .map_err(|error| format!("保存 Cloudflare 验证结果失败: {}", error))?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        response::{IntoResponse, Redirect},
        routing::get,
        Router,
    };
    use tokio::net::TcpListener;

    fn verified_config_for(hostname: &str, checked_at: String) -> CloudflareConfig {
        let mut config = crate::config::default_cloudflare_config();
        config.guided_setup_enabled = true;
        config.public_hostname = hostname.to_string();
        config.web_login_console_origin = "https://app.iterate.example.com".to_string();
        config.tunnel_token_saved = true;
        config.last_verified_at = Some(checked_at.clone());
        config.last_verification = Some(CloudflareVerificationResult {
            state: "verified".to_string(),
            public_hostname: hostname.to_string(),
            health_ok: true,
            pair_challenge_ok: true,
            websocket_ok: true,
            access_state: "not_required".to_string(),
            error_code: None,
            checked_at,
        });
        config
    }

    #[test]
    fn cloudflare_probe_payloads_require_iterate_json_semantics() {
        let health = serde_json::json!({
            "ok": true,
            "service": "iterate",
            "version": "0.5.8",
            "public_surface": "cloudflare_web_login",
            "capabilities": {
                "pair_challenge": true,
                "websocket": true
            }
        });
        assert!(cloudflare_health_payload_valid(&health));

        let access_page = serde_json::json!({
            "ok": true,
            "service": "cloudflare_access"
        });
        assert!(!cloudflare_health_payload_valid(&access_page));

        let pair = serde_json::json!({
            "ok": true,
            "challenge": "pc_abcdefghijklmnopqrstuvwxyz",
            "issued_at": "2026-06-06T00:00:00Z",
            "expires_at": "2026-06-06T00:02:00Z",
            "scope": "pair_challenge",
            "session_issued": false
        });
        assert!(pair_challenge_payload_valid(&pair));

        let session_issuing_pair = serde_json::json!({
            "ok": true,
            "challenge": "pc_abcdefghijklmnopqrstuvwxyz",
            "issued_at": "2026-06-06T00:00:00Z",
            "expires_at": "2026-06-06T00:02:00Z",
            "scope": "pair_challenge",
            "session_issued": true
        });
        assert!(!pair_challenge_payload_valid(&session_issuing_pair));
    }

    #[test]
    fn cloudflare_auto_setup_normalizes_zone_and_hostname_inputs() {
        assert_eq!(
            normalize_cloudflare_zone_name("https://Example.COM/").unwrap(),
            "example.com"
        );
        assert_eq!(
            build_cloudflare_hostname("example.com", "Login").unwrap(),
            "login.example.com"
        );
        assert_eq!(
            build_cloudflare_hostname("example.com", "https://login.example.com").unwrap(),
            "login.example.com"
        );
    }

    #[test]
    fn cloudflare_auto_setup_rejects_invalid_hostname_inputs() {
        assert!(normalize_cloudflare_zone_name("localhost").is_err());
        assert!(build_cloudflare_hostname("example.com", "-login").is_err());
        assert!(build_cloudflare_hostname("example.com", "login/next").is_err());
        assert!(build_cloudflare_hostname("example.com", ".login").is_err());
    }

    #[test]
    fn cloudflare_access_email_rules_are_normalized_and_deduped() {
        let emails = normalize_access_emails(&[
            " User@Example.COM ".to_string(),
            "user@example.com".to_string(),
            "".to_string(),
        ])
        .unwrap();
        assert_eq!(emails, vec!["user@example.com"]);
        assert!(normalize_access_emails(&["missing-at".to_string()]).is_err());
        assert!(normalize_access_emails(&["a b@example.com".to_string()]).is_err());
    }

    #[test]
    fn cloudflare_access_policy_body_uses_email_include_rules() {
        let body = cloudflare_access_policy_body(
            "iterate.example.com",
            &[
                "user@example.com".to_string(),
                "ops@example.com".to_string(),
            ],
        );
        assert_eq!(body["decision"], "allow");
        assert_eq!(body["precedence"], 1);
        assert_eq!(body["include"][0]["email"]["email"], "user@example.com");
        assert_eq!(body["include"][1]["email"]["email"], "ops@example.com");
    }

    #[test]
    fn cloudflare_access_application_management_boundary_uses_iterate_name() {
        let hostname = "iterate.example.com";
        let managed = CloudflareAccessApplication {
            id: "app-managed".to_string(),
            name: cloudflare_access_application_name(hostname),
            domain: Some(hostname.to_string()),
            application_type: Some("self_hosted".to_string()),
        };
        let unmanaged = CloudflareAccessApplication {
            id: "app-user".to_string(),
            name: "existing user access app".to_string(),
            domain: Some(hostname.to_string()),
            application_type: Some("self_hosted".to_string()),
        };

        assert!(cloudflare_access_application_managed_by_iterate(
            &managed, hostname
        ));
        assert!(!cloudflare_access_application_managed_by_iterate(
            &unmanaged, hostname
        ));
    }

    #[tokio::test]
    async fn health_probe_does_not_follow_redirect_to_success_page() {
        let app = Router::new()
            .route(
                "/.well-known/iterate/health",
                get(|| async { Redirect::temporary("/login") }),
            )
            .route(
                "/login",
                get(|| async {
                    axum::Json(serde_json::json!({
                        "ok": true,
                        "service": "iterate",
                        "version": "0.5.8",
                        "public_surface": "cloudflare_web_login",
                        "capabilities": {
                            "pair_challenge": true,
                            "websocket": true
                        }
                    }))
                    .into_response()
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        assert!(!probe_public_health(&format!("http://{addr}")).await);
    }

    #[tokio::test]
    async fn access_gate_probe_detects_cloudflare_access_redirect() {
        let app = Router::new().route(
            "/.well-known/iterate/health",
            get(|| async {
                Redirect::temporary(
                    "https://team.cloudflareaccess.com/cdn-cgi/access/login/example",
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        assert!(probe_cloudflare_access_gate(&format!("http://{addr}")).await);
        assert!(!probe_public_health(&format!("http://{addr}")).await);
    }

    #[test]
    fn cloudflare_pairing_gate_rejects_stale_or_mismatched_verification() {
        let now = chrono::Utc::now();
        let fresh = now.to_rfc3339();
        let stale = (now - chrono::Duration::seconds(CLOUDFLARE_VERIFICATION_FRESHNESS_SECS + 1))
            .to_rfc3339();

        let config = verified_config_for("https://iterate.example.com", fresh);
        assert_eq!(
            cloudflare_pairing_gate_error(&config, "https://other.example.com", now),
            Some("cloudflare_verification_stale")
        );
        assert_eq!(
            cloudflare_pairing_gate_error(&config, "https://iterate.example.com", now),
            None
        );

        let stale_config = verified_config_for("https://iterate.example.com", stale);
        assert_eq!(
            cloudflare_pairing_gate_error(&stale_config, "https://iterate.example.com", now),
            Some("cloudflare_verification_stale")
        );

        let unverified = crate::config::default_cloudflare_config();
        assert_eq!(
            cloudflare_pairing_gate_error(&unverified, "https://iterate.example.com", now),
            Some("web_login_console_origin_missing")
        );
    }

    #[test]
    fn cloudflare_config_changes_invalidate_verification() {
        let config = verified_config_for(
            "https://iterate.example.com",
            chrono::Utc::now().to_rfc3339(),
        );

        assert!(cloudflare_config_change_invalidates_verification(
            &config,
            "https://other.example.com",
            false,
            false
        ));
        assert!(cloudflare_config_change_invalidates_verification(
            &config,
            "https://iterate.example.com",
            true,
            false
        ));
        assert!(cloudflare_config_change_invalidates_verification(
            &config,
            "https://iterate.example.com",
            false,
            true
        ));
        assert!(!cloudflare_config_change_invalidates_verification(
            &config,
            "https://iterate.example.com",
            false,
            false
        ));
    }

    #[tokio::test]
    async fn formal_mobile_route_rejects_unknown_provenance_before_any_probe() {
        let error = register_formal_mobile_route(
            "cloudflare_named_tunnel",
            "https://iterate.example.com",
            "copied_from_untrusted_prompt",
        )
        .await
        .expect_err("unknown route provenance must be rejected");

        assert_eq!(error, "formal_route_source_invalid");
    }

    #[test]
    fn aliyun_formal_route_accepts_https_ip_origin_on_configured_port() {
        for port in [8443, 9443] {
            let origin = format!("https://8.8.8.8:{port}");
            assert_eq!(
                normalize_formal_mobile_route_base_url("aliyun_ssh_reverse_tunnel", &origin),
                Ok(origin)
            );
        }
        for origin in [
            "https://example.com:8443",
            "https://user:secret@8.8.8.8:8443",
            "https://8.8.8.8:8443/path",
            "http://8.8.8.8:8443",
            "https://127.0.0.1:8443",
            "https://0.1.2.3:8443",
            "https://10.1.2.3:8443",
            "https://100.64.1.2:8443",
            "https://169.254.1.2:8443",
            "https://172.16.1.2:8443",
            "https://192.168.1.2:8443",
            "https://192.0.0.1:8443",
            "https://192.0.2.1:8443",
            "https://192.88.99.1:8443",
            "https://198.18.1.2:8443",
            "https://198.51.100.2:8443",
            "https://203.0.113.10:8443",
            "https://224.0.0.1:8443",
            "https://240.0.0.1:8443",
            "https://255.255.255.255:8443",
        ] {
            assert!(
                normalize_formal_mobile_route_base_url("aliyun_ssh_reverse_tunnel", origin)
                    .is_err()
            );
        }
    }
}
