use super::quota::{QuotaMetric, UsageProvider};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Local, SecondsFormat, Utc};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const DEFAULT_CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const CHATGPT_USAGE_PATH: &str = "/wham/usage";
const CODEX_USAGE_PATH: &str = "/api/codex/usage";
const TOKEN_REFRESH_ENDPOINT: &str = "https://auth.openai.com/oauth/token";
const CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";
const TOKEN_REFRESH_AFTER_DAYS: i64 = 8;

#[derive(Debug, Clone, Default)]
pub struct CodexUsageProviderOptions {
    pub provider_id: Option<String>,
    pub summary_label: Option<String>,
    pub codex_home: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct CodexCredentials {
    auth_path: PathBuf,
    raw_json: Value,
    access_token: String,
    refresh_token: String,
    id_token: Option<String>,
    account_id: Option<String>,
    last_refresh: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
struct CodexRateWindow {
    used_percent: f64,
    limit_window_seconds: u64,
    reset_at: i64,
}

#[derive(Debug)]
struct CodexUsageSnapshot {
    plan_type: Option<String>,
    primary: Option<CodexRateWindow>,
    secondary: Option<CodexRateWindow>,
}

#[derive(Debug, serde::Deserialize)]
struct TokenRefreshResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

pub async fn get_native_codex_usage_provider() -> Result<UsageProvider, String> {
    get_native_codex_usage_provider_with_options(CodexUsageProviderOptions::default()).await
}

pub async fn get_native_codex_usage_provider_with_options(
    options: CodexUsageProviderOptions,
) -> Result<UsageProvider, String> {
    let codex_home = options.codex_home.unwrap_or_else(codex_home_from_env);
    let mut credentials = load_credentials_from_home(codex_home.clone())?;
    if credentials.needs_refresh() {
        credentials = refresh_credentials(credentials).await?;
        save_credentials(&credentials)?;
    }

    let usage_url = resolve_usage_url_from_home(codex_home.as_path());
    let usage = fetch_usage_snapshot(&credentials, usage_url.as_str()).await?;
    let provider_id = options
        .provider_id
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("codex");
    let account_label = codex_account_label(&credentials, options.summary_label.as_deref());
    usage
        .to_usage_provider(
            provider_id,
            options.summary_label.as_deref(),
            account_label.as_deref(),
        )
        .ok_or_else(|| "Codex usage API 没有返回可显示的额度窗口".to_string())
}

fn codex_home_from_env() -> PathBuf {
    std::env::var("CODEX_HOME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
        })
}

fn load_credentials_from_home(codex_home: PathBuf) -> Result<CodexCredentials, String> {
    let auth_path = codex_home.join("auth.json");
    let raw = fs::read_to_string(&auth_path)
        .map_err(|_| "Codex auth.json 未找到，请先运行 codex 登录".to_string())?;
    parse_credentials(auth_path, raw.as_str())
}

fn parse_credentials(auth_path: PathBuf, raw: &str) -> Result<CodexCredentials, String> {
    let raw_json: Value =
        serde_json::from_str(raw).map_err(|e| format!("解析 Codex auth.json 失败: {}", e))?;
    let object = raw_json
        .as_object()
        .ok_or_else(|| "Codex auth.json 不是 JSON object".to_string())?;
    let last_refresh = object
        .get("last_refresh")
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_utc);

    if let Some(api_key) = string_field(object, &["OPENAI_API_KEY"]) {
        return Ok(CodexCredentials {
            auth_path,
            raw_json,
            access_token: api_key,
            refresh_token: String::new(),
            id_token: None,
            account_id: None,
            last_refresh: None,
        });
    }

    let tokens = object
        .get("tokens")
        .and_then(Value::as_object)
        .ok_or_else(|| "Codex auth.json 缺少 tokens".to_string())?;
    let access_token = string_field(tokens, &["access_token", "accessToken"])
        .ok_or_else(|| "Codex auth.json 缺少 access_token".to_string())?;
    let refresh_token = string_field(tokens, &["refresh_token", "refreshToken"])
        .ok_or_else(|| "Codex auth.json 缺少 refresh_token".to_string())?;
    let id_token = string_field(tokens, &["id_token", "idToken"]);
    let account_id = string_field(tokens, &["account_id", "accountId"]);

    Ok(CodexCredentials {
        auth_path,
        raw_json,
        access_token,
        refresh_token,
        id_token,
        account_id,
        last_refresh,
    })
}

fn string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .filter_map(|key| object.get(*key))
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn codex_account_label(
    credentials: &CodexCredentials,
    summary_label: Option<&str>,
) -> Option<String> {
    email_from_id_token(credentials.id_token.as_deref()).or_else(|| {
        summary_label
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn email_from_id_token(id_token: Option<&str>) -> Option<String> {
    let payload = id_token?.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let value: Value = serde_json::from_slice(bytes.as_slice()).ok()?;
    let object = value.as_object()?;
    string_field(object, &["email"])
}

fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.with_timezone(&Utc))
}

impl CodexCredentials {
    fn needs_refresh(&self) -> bool {
        if self.refresh_token.is_empty() {
            return false;
        }
        match self.last_refresh {
            Some(last_refresh) => {
                Utc::now().signed_duration_since(last_refresh).num_days() > TOKEN_REFRESH_AFTER_DAYS
            }
            None => true,
        }
    }
}

async fn refresh_credentials(credentials: CodexCredentials) -> Result<CodexCredentials, String> {
    let client = codex_http_client(Duration::from_secs(30), "token 刷新")?;

    let body = serde_json::json!({
        "client_id": CODEX_CLIENT_ID,
        "grant_type": "refresh_token",
        "refresh_token": credentials.refresh_token,
        "scope": "openid profile email",
    });

    let response = client
        .post(TOKEN_REFRESH_ENDPOINT)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Codex token 刷新请求失败: {}", e))?;

    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("读取 Codex token 刷新响应失败: {}", e))?;
    if !status.is_success() {
        return Err(format!("Codex token 刷新失败: HTTP {}", status));
    }

    let refresh: TokenRefreshResponse =
        serde_json::from_str(&text).map_err(|e| format!("解析 Codex token 刷新响应失败: {}", e))?;

    Ok(CodexCredentials {
        access_token: refresh
            .access_token
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(credentials.access_token),
        refresh_token: refresh
            .refresh_token
            .filter(|value| !value.trim().is_empty())
            .unwrap_or(credentials.refresh_token),
        id_token: refresh.id_token.or(credentials.id_token),
        account_id: credentials.account_id,
        last_refresh: Some(Utc::now()),
        auth_path: credentials.auth_path,
        raw_json: credentials.raw_json,
    })
}

fn save_credentials(credentials: &CodexCredentials) -> Result<(), String> {
    let mut root = credentials.raw_json.clone();
    let object = root
        .as_object_mut()
        .ok_or_else(|| "Codex auth.json 不是 JSON object".to_string())?;

    let tokens_value = object
        .entry("tokens".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let tokens = tokens_value
        .as_object_mut()
        .ok_or_else(|| "Codex auth.json tokens 不是 JSON object".to_string())?;

    tokens.insert(
        "access_token".to_string(),
        Value::String(credentials.access_token.clone()),
    );
    tokens.insert(
        "refresh_token".to_string(),
        Value::String(credentials.refresh_token.clone()),
    );
    if let Some(id_token) = credentials.id_token.as_ref() {
        tokens.insert("id_token".to_string(), Value::String(id_token.clone()));
    }
    if let Some(account_id) = credentials.account_id.as_ref() {
        tokens.insert("account_id".to_string(), Value::String(account_id.clone()));
    }
    object.insert(
        "last_refresh".to_string(),
        Value::String(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)),
    );

    let serialized = serde_json::to_vec_pretty(&root)
        .map_err(|e| format!("序列化 Codex auth.json 失败: {}", e))?;
    let parent = credentials
        .auth_path
        .parent()
        .ok_or_else(|| "Codex auth.json 路径无父目录".to_string())?;
    fs::create_dir_all(parent).map_err(|e| format!("创建 Codex auth 目录失败: {}", e))?;

    let temp_path = parent.join(format!(".auth.json.{}.tmp", uuid::Uuid::new_v4()));
    fs::write(&temp_path, serialized)
        .map_err(|e| format!("写入 Codex auth 临时文件失败: {}", e))?;
    set_secret_file_permissions(&temp_path)?;
    fs::rename(&temp_path, &credentials.auth_path)
        .map_err(|e| format!("更新 Codex auth.json 失败: {}", e))?;
    Ok(())
}

fn set_secret_file_permissions(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("设置 Codex auth 文件权限失败: {}", e))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

fn resolve_usage_url_from_home(codex_home: &Path) -> String {
    let config_path = codex_home.join("config.toml");
    let contents = fs::read_to_string(config_path).ok();
    let base = contents
        .as_deref()
        .and_then(parse_chatgpt_base_url)
        .unwrap_or(DEFAULT_CHATGPT_BASE_URL);
    resolve_usage_url(base)
}

fn parse_chatgpt_base_url(contents: &str) -> Option<&str> {
    for raw_line in contents.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "chatgpt_base_url" {
            continue;
        }
        let value = value.trim().trim_matches('"').trim_matches('\'').trim();
        if !value.is_empty() {
            return Some(value);
        }
    }
    None
}

fn resolve_usage_url(base_url: &str) -> String {
    let mut normalized = base_url.trim().trim_end_matches('/').to_string();
    if normalized.is_empty() {
        normalized = DEFAULT_CHATGPT_BASE_URL.to_string();
    }
    if (normalized.starts_with("https://chatgpt.com")
        || normalized.starts_with("https://chat.openai.com"))
        && !normalized.contains("/backend-api")
    {
        normalized.push_str("/backend-api");
    }
    let path = if normalized.contains("/backend-api") {
        CHATGPT_USAGE_PATH
    } else {
        CODEX_USAGE_PATH
    };
    format!("{}{}", normalized, path)
}

async fn fetch_usage_snapshot(
    credentials: &CodexCredentials,
    usage_url: &str,
) -> Result<CodexUsageSnapshot, String> {
    let client = codex_http_client(Duration::from_secs(30), "usage")?;

    let mut request = client
        .get(usage_url)
        .bearer_auth(credentials.access_token.as_str())
        .header("Accept", "application/json")
        .header("User-Agent", "iterate");
    if let Some(account_id) = credentials.account_id.as_ref() {
        request = request.header("ChatGPT-Account-Id", account_id.as_str());
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("Codex usage 请求失败: {}", e))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|e| format!("读取 Codex usage 响应失败: {}", e))?;
    if !status.is_success() {
        return Err(format!("Codex usage API 返回 HTTP {}", status));
    }
    parse_usage_snapshot(text.as_str())
}

fn codex_http_client(timeout: Duration, label: &str) -> Result<reqwest::Client, String> {
    let mut builder = reqwest::Client::builder().timeout(timeout);
    if let Some(proxy_url) = macos_system_https_proxy_url() {
        let proxy = reqwest::Proxy::all(proxy_url.as_str())
            .map_err(|e| format!("创建 Codex {} 系统代理失败: {}", label, e))?;
        builder = builder.proxy(proxy);
    }

    builder
        .build()
        .map_err(|e| format!("创建 Codex {} client 失败: {}", label, e))
}

#[cfg(target_os = "macos")]
pub(crate) fn macos_system_https_proxy_url() -> Option<String> {
    let output = std::process::Command::new("scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    parse_macos_scutil_proxy(stdout.as_str())
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn macos_system_https_proxy_url() -> Option<String> {
    None
}

fn parse_macos_scutil_proxy(contents: &str) -> Option<String> {
    let entries: HashMap<String, String> = contents
        .lines()
        .filter_map(|line| {
            let (key, value) = line.split_once(':')?;
            Some((key.trim().to_string(), value.trim().to_string()))
        })
        .collect();

    proxy_url_from_entries(&entries, "HTTPS").or_else(|| proxy_url_from_entries(&entries, "HTTP"))
}

fn proxy_url_from_entries(entries: &HashMap<String, String>, prefix: &str) -> Option<String> {
    if entries.get(format!("{}Enable", prefix).as_str())? != "1" {
        return None;
    }
    let host = entries.get(format!("{}Proxy", prefix).as_str())?.trim();
    let port = entries
        .get(format!("{}Port", prefix).as_str())?
        .trim()
        .parse::<u16>()
        .ok()?;
    if host.is_empty() || port == 0 {
        return None;
    }

    Some(format!("http://{}:{}", bracket_ipv6_host(host), port))
}

fn bracket_ipv6_host(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{}]", host)
    } else {
        host.to_string()
    }
}

fn parse_usage_snapshot(raw: &str) -> Result<CodexUsageSnapshot, String> {
    let value: Value =
        serde_json::from_str(raw).map_err(|e| format!("解析 Codex usage JSON 失败: {}", e))?;
    let plan_type = value
        .get("plan_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    let rate_limit = value.get("rate_limit").and_then(Value::as_object);
    let primary = rate_limit
        .and_then(|object| object.get("primary_window"))
        .and_then(parse_rate_window);
    let secondary = rate_limit
        .and_then(|object| object.get("secondary_window"))
        .and_then(parse_rate_window);

    Ok(CodexUsageSnapshot {
        plan_type,
        primary,
        secondary,
    })
}

fn parse_rate_window(value: &Value) -> Option<CodexRateWindow> {
    if value.is_null() {
        return None;
    }
    let object = value.as_object()?;
    Some(CodexRateWindow {
        used_percent: number_field(object, "used_percent")?,
        limit_window_seconds: number_field(object, "limit_window_seconds")? as u64,
        reset_at: number_field(object, "reset_at")? as i64,
    })
}

fn number_field(object: &Map<String, Value>, key: &str) -> Option<f64> {
    match object.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(value) => value.parse::<f64>().ok(),
        _ => None,
    }
}

impl CodexUsageSnapshot {
    fn to_usage_provider(
        &self,
        provider_id: &str,
        summary_label: Option<&str>,
        account_label: Option<&str>,
    ) -> Option<UsageProvider> {
        let mut metrics = Vec::new();
        if let Some(metric) = self.primary.as_ref().map(rate_window_to_metric) {
            metrics.push(metric);
        }
        if let Some(metric) = self.secondary.as_ref().map(rate_window_to_metric) {
            metrics.push(metric);
        }
        if metrics.is_empty() {
            return None;
        }

        Some(UsageProvider {
            id: provider_id.to_string(),
            name: format_codex_provider_name(summary_label),
            account_label: account_label.map(ToOwned::to_owned),
            color: "#f8fafc".to_string(),
            icon_url: Some("./icons/ai-providers/codex.svg".to_string()),
            summary: format_codex_summary(self.plan_type.as_deref(), summary_label),
            updated_at: Some(Local::now().format("%H:%M").to_string()),
            metrics,
        })
    }
}

fn format_codex_provider_name(summary_label: Option<&str>) -> String {
    match summary_label
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(label) => format!("Codex · {}", label),
        None => "Codex".to_string(),
    }
}

fn format_codex_summary(plan_type: Option<&str>, summary_label: Option<&str>) -> String {
    let plan = format_plan_type(plan_type);
    let label = summary_label
        .map(str::trim)
        .filter(|value| !value.is_empty());
    match label {
        Some(label) if !label.eq_ignore_ascii_case(plan.as_str()) => {
            format!("{} · {} OAuth", label, plan)
        }
        _ => format!("{} OAuth", plan),
    }
}

fn rate_window_to_metric(window: &CodexRateWindow) -> QuotaMetric {
    let used = window.used_percent.clamp(0.0, 100.0).round() as u8;
    QuotaMetric {
        label: metric_label(window.limit_window_seconds),
        remaining: 100u8.saturating_sub(used),
        reset_label: reset_label(window.reset_at),
        reset_at_ms: Some(window.reset_at.saturating_mul(1000)),
    }
}

fn metric_label(window_seconds: u64) -> String {
    match window_seconds / 60 {
        300 => "Session".to_string(),
        1_440 => "Daily".to_string(),
        10_080 => "Weekly".to_string(),
        minutes if (43_000..=45_000).contains(&minutes) => "Monthly".to_string(),
        _ => "Quota".to_string(),
    }
}

fn reset_label(reset_at: i64) -> Option<String> {
    DateTime::from_timestamp(reset_at, 0)
        .map(|date| date.with_timezone(&Local))
        .map(|date| format!("{} 重置", date.format("%Y年%-m月%-d日 %-H:%M")))
}

fn format_plan_type(value: Option<&str>) -> String {
    match value.unwrap_or("codex").to_ascii_lowercase().as_str() {
        "pro" => "Pro".to_string(),
        "plus" => "Plus".to_string(),
        "team" => "Team".to_string(),
        "enterprise" => "Enterprise".to_string(),
        "free" => "Free".to_string(),
        "go" => "Go".to_string(),
        other => other
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_auth_json_tokens() {
        let raw = r#"{
          "OPENAI_API_KEY": null,
          "tokens": {
            "access_token": "access",
            "refresh_token": "refresh",
            "id_token": "id",
            "account_id": "account"
          },
          "last_refresh": "2026-04-27T06:47:58.473345Z"
        }"#;
        let credentials = parse_credentials(PathBuf::from("/tmp/auth.json"), raw).unwrap();

        assert_eq!(credentials.access_token, "access");
        assert_eq!(credentials.refresh_token, "refresh");
        assert_eq!(credentials.id_token.as_deref(), Some("id"));
        assert_eq!(credentials.account_id.as_deref(), Some("account"));
        assert!(credentials.last_refresh.is_some());
    }

    #[test]
    fn nonempty_api_key_is_supported() {
        let raw = r#"{"OPENAI_API_KEY":"sk-test"}"#;
        let credentials = parse_credentials(PathBuf::from("/tmp/auth.json"), raw).unwrap();

        assert_eq!(credentials.access_token, "sk-test");
        assert!(credentials.refresh_token.is_empty());
        assert!(!credentials.needs_refresh());
    }

    #[test]
    fn resolves_usage_urls() {
        assert_eq!(
            resolve_usage_url("https://chatgpt.com"),
            "https://chatgpt.com/backend-api/wham/usage"
        );
        assert_eq!(
            resolve_usage_url("https://chat.openai.com/backend-api/"),
            "https://chat.openai.com/backend-api/wham/usage"
        );
        assert_eq!(
            resolve_usage_url("https://api.openai.com"),
            "https://api.openai.com/api/codex/usage"
        );
    }

    #[test]
    fn parses_config_base_url() {
        let config = r#"
            model = "gpt-5.4"
            chatgpt_base_url = "https://api.openai.com"
        "#;

        assert_eq!(
            parse_chatgpt_base_url(config),
            Some("https://api.openai.com")
        );
    }

    #[test]
    fn parses_macos_https_proxy_from_scutil() {
        let output = r#"
<dictionary> {
  HTTPEnable : 1
  HTTPPort : 8888
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : 7897
  HTTPSProxy : 127.0.0.1
}
"#;

        assert_eq!(
            parse_macos_scutil_proxy(output),
            Some("http://127.0.0.1:7897".to_string())
        );
    }

    #[test]
    fn falls_back_to_http_proxy_when_https_disabled() {
        let output = r#"
<dictionary> {
  HTTPEnable : 1
  HTTPPort : 8888
  HTTPProxy : localhost
  HTTPSEnable : 0
}
"#;

        assert_eq!(
            parse_macos_scutil_proxy(output),
            Some("http://localhost:8888".to_string())
        );
    }

    #[test]
    fn ignores_disabled_or_incomplete_macos_proxy() {
        assert_eq!(
            parse_macos_scutil_proxy(
                r#"
<dictionary> {
  HTTPEnable : 0
  HTTPPort : 8888
  HTTPProxy : 127.0.0.1
  HTTPSEnable : 1
  HTTPSPort : bad
  HTTPSProxy : 127.0.0.1
}
"#
            ),
            None
        );
    }

    #[test]
    fn maps_usage_snapshot_to_provider() {
        let raw = r#"{
          "plan_type": "pro",
          "rate_limit": {
            "primary_window": {
              "used_percent": 12,
              "limit_window_seconds": 18000,
              "reset_at": 1766948068
            },
            "secondary_window": {
              "used_percent": 43,
              "limit_window_seconds": 604800,
              "reset_at": 1767407914
            }
          }
        }"#;

        let snapshot = parse_usage_snapshot(raw).unwrap();
        let provider = snapshot.to_usage_provider("codex", None, None).unwrap();

        assert_eq!(provider.id, "codex");
        assert_eq!(provider.summary, "Pro OAuth");
        assert_eq!(provider.metrics.len(), 2);
        assert_eq!(provider.metrics[0].label, "Session");
        assert_eq!(provider.metrics[0].remaining, 88);
        assert_eq!(provider.metrics[1].label, "Weekly");
        assert_eq!(provider.metrics[1].remaining, 57);
    }

    #[test]
    fn ignores_malformed_window_while_keeping_valid_one() {
        let raw = r#"{
          "rate_limit": {
            "primary_window": {
              "used_percent": "bad",
              "limit_window_seconds": 18000,
              "reset_at": 1766948068
            },
            "secondary_window": {
              "used_percent": 43,
              "limit_window_seconds": 604800,
              "reset_at": 1767407914
            }
          }
        }"#;

        let snapshot = parse_usage_snapshot(raw).unwrap();
        assert!(snapshot.primary.is_none());
        assert!(snapshot.secondary.is_some());
        assert_eq!(
            snapshot
                .to_usage_provider("codex", None, None)
                .unwrap()
                .metrics
                .first()
                .unwrap()
                .label,
            "Weekly"
        );
    }

    #[test]
    fn maps_usage_snapshot_to_named_provider() {
        let raw = r#"{
          "plan_type": "pro",
          "rate_limit": {
            "primary_window": {
              "used_percent": 12,
              "limit_window_seconds": 18000,
              "reset_at": 1766948068
            }
          }
        }"#;

        let snapshot = parse_usage_snapshot(raw).unwrap();
        let provider = snapshot
            .to_usage_provider("codex:plus", Some("Plus account"), Some("user@example.com"))
            .unwrap();

        assert_eq!(provider.id, "codex:plus");
        assert_eq!(provider.name, "Codex · Plus account");
        assert_eq!(provider.account_label.as_deref(), Some("user@example.com"));
        assert_eq!(provider.summary, "Plus account · Pro OAuth");
    }

    #[test]
    fn omits_duplicate_summary_label() {
        assert_eq!(format_codex_summary(Some("pro"), Some("Pro")), "Pro OAuth");
    }

    #[test]
    fn names_codex_provider_with_account_label() {
        assert_eq!(
            format_codex_provider_name(Some("Plus")),
            "Codex · Plus".to_string()
        );
        assert_eq!(format_codex_provider_name(None), "Codex".to_string());
    }

    #[test]
    fn extracts_email_from_id_token_for_account_label() {
        let raw = r#"{
          "tokens": {
            "access_token": "access",
            "refresh_token": "refresh",
            "id_token": "e30.eyJlbWFpbCI6InVzZXJAZXhhbXBsZS5jb20ifQ.signature"
          }
        }"#;
        let credentials = parse_credentials(PathBuf::from("/tmp/auth.json"), raw).unwrap();

        assert_eq!(
            codex_account_label(&credentials, Some("Main")).as_deref(),
            Some("user@example.com")
        );
    }
}
