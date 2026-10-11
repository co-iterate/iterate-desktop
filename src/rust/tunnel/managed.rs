//! Small managed-frp bootstrap. Pairing and business traffic keep the existing Bridge protocol.
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use futures_util::StreamExt;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;
use tauri::Manager;

/// 托管服务地址在编译期注入（官方构建由 CI / 本地构建环境提供），公开源码里不写死。
/// 两项都未设置时托管不可用；只设置一项或格式非法视为配置错误。
const MANAGED_CONNECT_HOST: Option<&str> = option_env!("ITERATE_MANAGED_CONNECT_HOST");
const MANAGED_DEVICE_DOMAIN: Option<&str> = option_env!("ITERATE_MANAGED_DEVICE_DOMAIN");
const KEYCHAIN_SERVICE: &str = "xin.iterate.managed-connection";
const LABEL: &str = "com.kexin94yyds.iterate.managed-connection";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ManagedService {
    connect_host: &'static str,
    device_domain: &'static str,
}

impl ManagedService {
    /// API、frpc serverAddr 与 TLS serverName 都从同一个主机名派生，避免指向不同服务。
    fn api_origin(&self) -> String {
        format!("https://{}", self.connect_host)
    }
}

fn is_dns_hostname(value: &str) -> bool {
    value.len() <= 253
        && value.contains('.')
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
        })
}

fn resolve_managed_service(
    connect_host: Option<&'static str>,
    device_domain: Option<&'static str>,
) -> Result<ManagedService, String> {
    let connect_host = connect_host.map(str::trim).filter(|value| !value.is_empty());
    let device_domain = device_domain.map(str::trim).filter(|value| !value.is_empty());
    match (connect_host, device_domain) {
        (None, None) => Err("managed_not_configured".into()),
        (Some(connect_host), Some(device_domain))
            if is_dns_hostname(connect_host) && is_dns_hostname(device_domain) =>
        {
            Ok(ManagedService { connect_host, device_domain })
        }
        _ => Err("managed_configuration_invalid".into()),
    }
}

fn managed_service() -> Result<ManagedService, String> {
    resolve_managed_service(MANAGED_CONNECT_HOST, MANAGED_DEVICE_DOMAIN)
}

#[derive(Serialize, Deserialize)]
struct Identity {
    version: u8,
    private_key: String,
    credential: String,
}

impl Identity {
    fn generate() -> Result<Self, String> {
        let rng = SystemRandom::new();
        let key = Ed25519KeyPair::generate_pkcs8(&rng).map_err(|_| "managed_key_generation_failed")?;
        let mut credential = [0_u8; 48];
        rng.fill(&mut credential).map_err(|_| "managed_key_generation_failed")?;
        Ok(Self { version: 1, private_key: STANDARD.encode(key.as_ref()), credential: URL_SAFE_NO_PAD.encode(credential) })
    }

    fn key(&self) -> Result<Ed25519KeyPair, String> {
        if self.version != 1 || self.credential.len() != 64 {
            return Err("managed_credentials_invalid".into());
        }
        let credential = URL_SAFE_NO_PAD.decode(&self.credential).map_err(|_| "managed_credentials_invalid")?;
        if credential.len() != 48 || URL_SAFE_NO_PAD.encode(&credential) != self.credential {
            return Err("managed_credentials_invalid".into());
        }
        let bytes = STANDARD.decode(&self.private_key).map_err(|_| "managed_credentials_invalid")?;
        Ed25519KeyPair::from_pkcs8(&bytes).map_err(|_| "managed_credentials_invalid".into())
    }

    fn public_key(&self) -> Result<String, String> {
        Ok(URL_SAFE_NO_PAD.encode(self.key()?.public_key().as_ref()))
    }

    fn id(&self) -> Result<String, String> {
        let key = self.key()?;
        Ok(format!("d{}", &hex::encode(ring::digest::digest(&ring::digest::SHA256, key.public_key().as_ref()))[..30]))
    }

    fn request(&self, challenge: &str) -> Result<Value, String> {
        let decoded = URL_SAFE_NO_PAD.decode(challenge).map_err(|_| "managed_challenge_invalid")?;
        if decoded.len() != 32 || URL_SAFE_NO_PAD.encode(&decoded) != challenge {
            return Err("managed_challenge_invalid".into());
        }
        let mut bytes = b"iterate-device-secret-v1\0".to_vec();
        bytes.extend_from_slice(self.credential.as_bytes());
        let credential_hash = hex::encode(ring::digest::digest(&ring::digest::SHA256, &bytes));
        let public_key = self.public_key()?;
        // The three values are validated base64url/hex, so no escaping ambiguity.
        let canonical = format!("{{\"challenge\":\"{challenge}\",\"credential_hash\":\"{credential_hash}\",\"public_key\":\"{public_key}\"}}");
        let mut message = b"iterate-managed-enrollment-v1\0".to_vec();
        message.extend_from_slice(canonical.as_bytes());
        Ok(json!({"challenge":challenge,"credential_hash":credential_hash,"public_key":public_key,
                  "signature":URL_SAFE_NO_PAD.encode(self.key()?.sign(&message).as_ref())}))
    }
}

#[derive(Serialize, Deserialize)]
struct Enrollment {
    device_id: String,
    credential_version: u64,
    base_url: String,
    token_endpoint: String,
}

fn owned_device_base_url(service: &ManagedService, id: &str, base_url: &str) -> bool {
    [
        format!("https://{id}.{}", service.device_domain),
        format!("https://{id}.{}", service.connect_host),
    ]
    .contains(&base_url.to_string())
}

fn validate_enrollment(service: &ManagedService, identity: &Identity, result: &Enrollment) -> Result<(), String> {
    let id = identity.id()?;
    if result.device_id != id || result.credential_version == 0
        || !owned_device_base_url(service, &id, &result.base_url)
        || result.token_endpoint != format!("{}/token", service.api_origin()) {
        return Err("managed_service_response_invalid".into());
    }
    Ok(())
}

fn proxy_from_native(entries: &Value) -> Result<Option<String>, String> {
    let entries = entries.as_array().filter(|v| !v.is_empty() && v.len() <= 16).ok_or("managed_proxy_invalid")?;
    let first = &entries[0];
    if first.get("hasAuthentication").and_then(Value::as_bool) != Some(false) {
        return Err("managed_proxy_auth_unsupported".into());
    }
    match first.get("type").and_then(Value::as_str) {
        Some("kCFProxyTypeNone") => return Ok(None),
        Some("kCFProxyTypeHTTP" | "kCFProxyTypeHTTPS") => {},
        _ => return Err("managed_proxy_type_unsupported".into()),
    }
    let host = first.get("host").and_then(Value::as_str).ok_or("managed_proxy_invalid")?;
    let port = first.get("port").and_then(Value::as_u64).filter(|p| *p > 0 && *p <= 65535).ok_or("managed_proxy_invalid")?;
    if host.is_empty() || host.len() > 255 || host.chars().any(|c| c.is_whitespace() || c.is_control() || matches!(c, '/' | '@' | '?' | '#' | '%')) {
        return Err("managed_proxy_invalid".into());
    }
    let host = if host.contains(':') {
        let address: std::net::Ipv6Addr = host.trim_matches(['[', ']']).parse().map_err(|_| "managed_proxy_invalid")?;
        format!("[{address}]")
    } else {
        if !host.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'-' | b'_')) { return Err("managed_proxy_invalid".into()); }
        host.to_string()
    };
    Ok(Some(format!("http://{host}:{port}")))
}

async fn system_proxy(service: &ManagedService, resources: &Path) -> Result<Option<String>, String> {
    use std::process::Stdio;
    use tokio::io::AsyncWriteExt;
    let mut child = tokio::process::Command::new(resources.join("managed-proxy"))
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true)
        .spawn().map_err(|_| "managed_proxy_unavailable")?;
    let mut input = child.stdin.take().ok_or("managed_proxy_unavailable")?;
    input.write_all(service.api_origin().as_bytes()).await.map_err(|_| "managed_proxy_unavailable")?;
    drop(input);
    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output()).await
        .map_err(|_| "managed_proxy_unavailable")?.map_err(|_| "managed_proxy_unavailable")?;
    if !output.status.success() || output.stdout.len() > 16384 { return Err("managed_proxy_unavailable".into()); }
    proxy_from_native(&serde_json::from_slice(&output.stdout).map_err(|_| "managed_proxy_invalid")?)
}

fn frpc_config(service: &ManagedService, identity: &Identity, enrollment: &Enrollment, proxy: Option<&str>) -> Result<Value, String> {
    validate_enrollment(service, identity, enrollment)?;
    // Validation pins both the new namespace and the existing pilot namespace
    // to this exact installation. Preserve a previously assigned legacy route.
    let domain = enrollment.base_url.strip_prefix("https://").ok_or("managed_service_response_invalid")?;
    let mut config = json!({
        "serverAddr": service.connect_host, "serverPort":443, "user":enrollment.device_id, "loginFailExit":false,
        "auth":{"method":"oidc","additionalScopes":["HeartBeats","NewWorkConns"],"oidc":{
            "clientID":enrollment.device_id,"clientSecret":identity.credential,
            "audience":"iterate-managed-frp-v1","scope":"frp","tokenEndpointURL":enrollment.token_endpoint}},
        "transport":{"protocol":"wss","wireProtocol":"v1","tcpMux":true,
            "heartbeatInterval":2,"heartbeatTimeout":15,
            "tls":{"enable":true,"serverName":service.connect_host,"trustedCaFile":"/etc/ssl/cert.pem"}},
        "proxies":[{"name":"bridge","type":"http","localIP":"127.0.0.1","localPort":8080,
            "customDomains":[domain],"locations":["/"]}],
        "log":{"to":"console","level":"warn","disablePrintColor":true}
    });
    config["transport"]["proxyURL"] = proxy.unwrap_or("").into();
    config["auth"]["oidc"]["proxyURL"] = proxy.unwrap_or("").into();
    Ok(config)
}

async fn post_json(service: &ManagedService, client: &reqwest::Client, suffix: &str, body: Value) -> Result<Value, String> {
    let response = client.post(format!("{}{suffix}", service.api_origin())).json(&body).send().await
        .map_err(|_| "managed_service_unavailable")?;
    if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS { return Err("managed_service_busy".into()); }
    if !response.status().is_success() { return Err("managed_service_unavailable".into()); }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| "managed_service_unavailable")?;
        if bytes.len() + chunk.len() > 8192 { return Err("managed_service_response_invalid".into()); }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| "managed_service_response_invalid".into())
}

#[cfg(target_os = "macos")]
fn save_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().ok_or("managed_storage_invalid")?)
        .map_err(|_| "managed_storage_unavailable")?;
    file.as_file().set_permissions(std::fs::Permissions::from_mode(0o600)).map_err(|_| "managed_storage_unavailable")?;
    file.write_all(bytes).map_err(|_| "managed_storage_unavailable")?;
    file.as_file().sync_all().map_err(|_| "managed_storage_unavailable")?;
    file.persist(path).map_err(|_| "managed_storage_unavailable")?;
    std::fs::File::open(path.parent().ok_or("managed_storage_invalid")?)
        .and_then(|folder| folder.sync_all()).map_err(|_| "managed_storage_unavailable")?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct JobTransaction {
    previous: Option<Vec<u8>>,
    next: Vec<u8>,
}

fn read_job_file(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("managed_job_unavailable".into()),
    };
    if !metadata.file_type().is_file() || metadata.len() > 131072 {
        return Err("managed_job_ownership_conflict".into());
    }
    Ok(Some(std::fs::read(path).map_err(|_| "managed_job_unavailable")?))
}

fn job_is_owned(actual: Option<&[u8]>, receipt: Option<&[u8]>, pending: Option<&JobTransaction>) -> bool {
    if actual.is_none() || actual == receipt { return true; }
    pending.is_some_and(|transaction| {
        let next = Some(transaction.next.as_slice());
        (receipt == transaction.previous.as_deref() || receipt == next)
            && (actual == transaction.previous.as_deref() || actual == next)
    })
}

#[cfg(target_os = "macos")]
fn publish_job_files(
    path: &Path, receipt_path: &Path, pending_path: &Path, plist: &[u8],
    stop: &dyn Fn() -> Result<(), String>, start: &dyn Fn() -> Result<(), String>,
) -> Result<(), String> {
    let actual = read_job_file(path)?;
    let mut receipt = read_job_file(receipt_path)?;
    let pending = read_job_file(pending_path)?.map(|bytes| serde_json::from_slice::<JobTransaction>(&bytes)
        .map_err(|_| "managed_job_ownership_conflict".to_string())).transpose()?;
    if !job_is_owned(actual.as_deref(), receipt.as_deref(), pending.as_ref()) {
        return Err("managed_job_ownership_conflict".into());
    }
    // Recover a prior interrupted commit before creating the next transaction.
    // Failure here leaves the existing process and its pending ownership intact.
    if actual.is_some() && actual != receipt {
        save_private(receipt_path, actual.as_deref().unwrap())?;
        receipt = actual;
    }
    let transaction = JobTransaction { previous: receipt, next: plist.to_vec() };
    save_private(pending_path, &serde_json::to_vec(&transaction).map_err(|_| "managed_job_unavailable")?)?;
    stop()?;
    save_private(path, plist)?;
    save_private(receipt_path, plist)?;
    start()?;
    // A leftover complete transaction is safe to reconcile on the next attempt.
    let _ = std::fs::remove_file(pending_path);
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_job(binary: &Path, root: &Path) -> Result<(), String> {
    let escape = |value: &str| value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;");
    let plist = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Label</key><string>{LABEL}</string>
<key>ProgramArguments</key><array><string>/bin/sh</string><string>-c</string>
<string>if test -x "$1" &amp;&amp; test -f "$2"; then exec "$1" -c "$2"; fi; exit 0</string>
<string>iterate-managed</string><string>{}</string><string>{}</string></array>
<key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
<key>ThrottleInterval</key><integer>10</integer><key>EnvironmentVariables</key><dict>
<key>HTTP_PROXY</key><string></string><key>HTTPS_PROXY</key><string></string><key>ALL_PROXY</key><string></string>
<key>http_proxy</key><string></string><key>https_proxy</key><string></string><key>all_proxy</key><string></string>
<key>GOMAXPROCS</key><string>1</string></dict>
<key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string>
</dict></plist>"#, escape(&binary.to_string_lossy()), escape(&root.join("frpc.json").to_string_lossy()),
        escape(&root.join("frpc.log").to_string_lossy()), escape(&root.join("frpc-error.log").to_string_lossy()));
    let folder = dirs::home_dir().ok_or("managed_home_unavailable")?.join("Library/LaunchAgents");
    std::fs::create_dir_all(&folder).map_err(|_| "managed_job_unavailable")?;
    let path = folder.join(format!("{LABEL}.plist"));
    let expected = root.join("installed-job.plist");
    let service = format!("gui/{}/{LABEL}", unsafe { libc::getuid() });
    let loaded = std::process::Command::new("/bin/launchctl").args(["print", &service]).output()
        .map_err(|_| "managed_job_unavailable")?;
    if loaded.status.success() && !path.exists() { return Err("managed_job_ownership_conflict".into()); }
    if loaded.status.success() && !String::from_utf8_lossy(&loaded.stdout).lines()
        .any(|line| line.trim().strip_prefix("path = ") == Some(path.to_string_lossy().as_ref())) {
        return Err("managed_job_ownership_conflict".into());
    }
    publish_job_files(&path, &expected, &root.join("job-transaction.json"), plist.as_bytes(), &|| {
        let _ = std::process::Command::new("/bin/launchctl").args(["bootout", &service]).output();
        if std::process::Command::new("/bin/launchctl").args(["print", &service]).output()
            .map_err(|_| "managed_job_unavailable")?.status.success() {
            return Err("managed_job_stop_failed".into());
        }
        Ok(())
    }, &|| {
        let result = std::process::Command::new("/bin/launchctl")
            .args(["bootstrap", &format!("gui/{}", unsafe { libc::getuid() })]).arg(&path)
            .output().map_err(|_| "managed_job_unavailable")?;
        if !result.status.success() { return Err("managed_job_start_failed".into()); }
        Ok(())
    })?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn stop_owned_job(root: &Path) -> Result<(), String> {
    let path = dirs::home_dir().ok_or("managed_home_unavailable")?.join("Library/LaunchAgents").join(format!("{LABEL}.plist"));
    if !path.exists() { return Ok(()); }
    let installed = std::fs::read(&path).map_err(|_| "managed_job_unavailable")?;
    let expected = std::fs::read(root.join("installed-job.plist")).map_err(|_| "managed_job_ownership_conflict")?;
    if installed != expected {
        return Err("managed_job_ownership_conflict".into());
    }
    let service = format!("gui/{}/{LABEL}", unsafe { libc::getuid() });
    let _ = std::process::Command::new("/bin/launchctl").args(["bootout", &service]).output();
    if std::process::Command::new("/bin/launchctl").args(["print", &service]).output()
        .map_err(|_| "managed_job_unavailable")?.status.success() { return Err("managed_job_stop_failed".into()); }
    std::fs::rename(path, root.join("disabled-job.plist")).map_err(|_| "managed_job_unavailable")?;
    Ok(())
}

#[tauri::command]
pub async fn ensure_managed_mobile_route(app: tauri::AppHandle) -> Result<super::commands::FormalMobileRouteStatus, String> {
    #[cfg(not(target_os = "macos"))]
    { let _ = app; return Err("managed_platform_unsupported".into()); }
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        // 未配置托管服务的构建（如社区构建）在任何副作用之前返回。
        let service = managed_service()?;
        let root = crate::config::cunzhi_config_dir().map_err(|_| "managed_storage_unavailable")?.join("managed-connection");
        if root.is_symlink() { return Err("managed_storage_invalid".into()); }
        std::fs::create_dir_all(&root).map_err(|_| "managed_storage_unavailable")?;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).map_err(|_| "managed_storage_unavailable")?;
        let lock = std::fs::OpenOptions::new().read(true).write(true).create(true).mode(0o600)
            .custom_flags(libc::O_NOFOLLOW).open(root.join("connect.lock")).map_err(|_| "managed_storage_unavailable")?;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err("managed_connection_busy".into());
        }
        let current = super::commands::get_formal_mobile_route_status().await;
        if current.configured && current.health == "healthy" { return Ok(current); }
        let resources = app.path().resource_dir().map_err(|_| "managed_connector_missing")?;
        let binary = resources.join("managed-frpc");
        if !binary.is_file() { return Err("managed_connector_missing".into()); }
        use security_framework::passwords::{get_generic_password, set_generic_password};
        let identity: Identity = match get_generic_password(KEYCHAIN_SERVICE, "installation-v1") {
            Ok(value) => serde_json::from_slice(&value).map_err(|_| "managed_credentials_invalid")?,
            Err(error) if error.code() == -25300 && !current.configured => {
                let identity = Identity::generate()?;
                set_generic_password(KEYCHAIN_SERVICE, "installation-v1", &serde_json::to_vec(&identity).map_err(|_| "managed_credentials_invalid")?)
                    .map_err(|_| "managed_keychain_unavailable")?;
                identity
            }
            Err(error) if error.code() == -25300 => return Err("managed_route_not_owned".into()),
            Err(_) => return Err("managed_keychain_unavailable".into()),
        };
        if current.configured && (current.transport.as_deref() != Some("self_hosted_frp")
            || !owned_device_base_url(&service, &identity.id()?, current.base_url.as_deref().unwrap_or(""))) {
            return Err("managed_route_not_owned".into());
        }
        let proxy = system_proxy(&service, &resources).await?;
        let mut builder = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(proxy) = &proxy { builder = builder.proxy(reqwest::Proxy::all(proxy).map_err(|_| "managed_proxy_invalid")?); }
        let client = builder.build().map_err(|_| "managed_service_unavailable")?;
        let metadata_path = root.join("enrollment.json");
        let enrollment: Enrollment = if metadata_path.exists() {
            serde_json::from_slice(&std::fs::read(&metadata_path).map_err(|_| "managed_storage_unavailable")?)
                .map_err(|_| "managed_credentials_invalid")?
        } else {
            let challenge = post_json(&service, &client, "/v1/enrollment/challenge", json!({"public_key":identity.public_key()?})).await?;
            let challenge = challenge.get("challenge").and_then(Value::as_str).ok_or("managed_service_response_invalid")?;
            let response = post_json(&service, &client, "/v1/enrollment", identity.request(challenge)?).await?;
            let enrollment: Enrollment = serde_json::from_value(response).map_err(|_| "managed_service_response_invalid")?;
            validate_enrollment(&service, &identity, &enrollment)?;
            save_private(&metadata_path, &serde_json::to_vec(&enrollment).map_err(|_| "managed_storage_unavailable")?)?;
            enrollment
        };
        validate_enrollment(&service, &identity, &enrollment)?;
        save_private(&root.join("frpc.json"), &serde_json::to_vec_pretty(&frpc_config(&service, &identity, &enrollment, proxy.as_deref())?).map_err(|_| "managed_config_invalid")?)?;
        let verify = tokio::process::Command::new(&binary).args(["verify", "-c"]).arg(root.join("frpc.json"))
            .output().await.map_err(|_| "managed_connector_missing")?;
        if !verify.status.success() { return Err("managed_config_invalid".into()); }
        install_job(&binary, &root)?;
        let mut last = "managed_route_not_ready".to_string();
        for _ in 0..8 {
            match super::commands::register_managed_mobile_route(&enrollment.base_url, &current).await {
                Ok(status) => return Ok(status),
                Err(error) => {
                    if error == "formal_route_changed" { stop_owned_job(&root)?; return Err(error); }
                    last = error;
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        let _ = last;
        Err("managed_route_not_ready".into())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_dns_hostname, managed_service, owned_device_base_url, resolve_managed_service,
        ManagedService, MANAGED_CONNECT_HOST, MANAGED_DEVICE_DOMAIN,
    };

    #[test]
    fn injected_build_configuration_is_valid() {
        // 官方构建注入了地址时必须能解析；社区构建两项都为空，得到 not_configured。
        match (MANAGED_CONNECT_HOST, MANAGED_DEVICE_DOMAIN) {
            (None, None) => assert_eq!(managed_service(), Err("managed_not_configured".into())),
            _ => assert!(managed_service().is_ok(), "{:?}", managed_service()),
        }
    }

    const SERVICE: ManagedService = ManagedService {
        connect_host: "connect.example.com",
        device_domain: "example.com",
    };

    #[test]
    fn unconfigured_build_reports_not_configured() {
        assert_eq!(resolve_managed_service(None, None), Err("managed_not_configured".into()));
        assert_eq!(resolve_managed_service(Some(" "), Some("")), Err("managed_not_configured".into()));
    }

    #[test]
    fn partial_or_malformed_configuration_is_rejected() {
        let invalid = Err("managed_configuration_invalid".into());
        assert_eq!(resolve_managed_service(Some("connect.example.com"), None), invalid);
        assert_eq!(resolve_managed_service(None, Some("example.com")), invalid);
        for bad in ["https://connect.example.com", "connect.example.com:443", "connect.example.com/x",
            "user@connect.example.com", "*.example.com", "Connect.Example.com", "localhost", "-a.example.com", "a..b"] {
            assert!(!is_dns_hostname(bad), "{bad}");
            assert_eq!(resolve_managed_service(Some(bad), Some("example.com")), invalid, "{bad}");
        }
        assert_eq!(resolve_managed_service(Some("connect.example.com"), Some("example.com")), Ok(SERVICE));
    }

    #[test]
    fn device_base_url_must_match_exactly() {
        assert_eq!(SERVICE.api_origin(), "https://connect.example.com");
        assert!(owned_device_base_url(&SERVICE, "dabc", "https://dabc.example.com"));
        assert!(owned_device_base_url(&SERVICE, "dabc", "https://dabc.connect.example.com"));
        for bad in ["https://other.example.com", "http://dabc.example.com", "https://dabc.example.com/",
            "https://dabc.example.com.evil.com", "https://dabc.example.com:8443", "https://x@dabc.example.com"] {
            assert!(!owned_device_base_url(&SERVICE, "dabc", bad), "{bad}");
        }
    }
}
