//! Direct, certificate-paired TLS. SSH is deliberately absent from this module.
use super::{atomic_json, directory, lock, read_json, settings, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    net::{Ipv4Addr, SocketAddr},
    process::{Command, Stdio},
    time::Duration,
};

const TLS_NAME: &str = "iterate.local";

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectionConfig {
    pub device_name: String,
    pub listen_ip: String,
    pub listen_port: u16,
    pub peer_host: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub backup_peer_host: String,
    pub peer_port: u16,
    peer_ref: String,
}
impl Default for ConnectionConfig {
    fn default() -> Self {
        Self {
            device_name: std::env::var("ITERATE_CROSS_DEVICE_NAME")
                .unwrap_or_else(|_| std::env::consts::OS.into()),
            listen_ip: std::env::var("ITERATE_CROSS_DEVICE_LISTEN_IP")
                .unwrap_or_else(|_| "127.0.0.1".into()),
            listen_port: 5540,
            peer_host: String::new(),
            backup_peer_host: String::new(),
            peer_port: 5540,
            peer_ref: String::new(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(super) struct Identity {
    pub cert: String,
    pub key: String,
    pub token: String,
}

#[derive(Clone, Serialize, Deserialize)]
struct Pairing {
    version: u32,
    device_id: String,
    name: String,
    ip: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    ips: Vec<String>,
    port: u16,
    cert: String,
    token: String,
}

fn secret_account(kind: &str) -> Result<String> {
    Ok(format!("{}-{kind}", settings()?.device_id))
}

fn read_secret(account: &str) -> Result<Option<Vec<u8>>> {
    let dir = directory()?;
    let _guard = lock(&dir, "credentials.lock")?;
    let path = dir.join("credentials.json");
    if !path.exists() { return Ok(None); }
    let entries: std::collections::BTreeMap<String, Value> = read_json(&path)?;
    entries.get(account).map(serde_json::to_vec).transpose().map_err(|e| e.to_string())
}

fn write_secret(account: &str, value: &[u8]) -> Result<()> {
    let dir = directory()?;
    let _guard = lock(&dir, "credentials.lock")?;
    let path = dir.join("credentials.json");
    let mut entries: std::collections::BTreeMap<String, Value> = if path.exists() {
        read_json(&path)?
    } else { Default::default() };
    entries.insert(account.into(), serde_json::from_slice(value).map_err(|e| e.to_string())?);
    atomic_json(&path, &entries)
}

pub(super) fn identity(create: bool) -> Result<Identity> {
    let dir = directory()?;
    let _guard = lock(&dir, "identity.lock")?;
    let account = secret_account("identity")?;
    if let Some(bytes) = read_secret(&account)? {
        return serde_json::from_slice(&bytes).map_err(|_| "本机跨设备身份损坏".into());
    }
    if !create {
        return Err("尚未生成本机配对码".into());
    }
    let certificate = rcgen::generate_simple_self_signed(vec![TLS_NAME.into()])
        .map_err(|_| "无法生成本机证书")?;
    let mut token = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut token)
        .map_err(|_| "无法生成配对凭据")?;
    let value = Identity {
        cert: BASE64.encode(certificate.cert.der()),
        key: BASE64.encode(certificate.key_pair.serialize_der()),
        token: hex::encode(token),
    };
    write_secret(
        &account,
        &serde_json::to_vec(&value).map_err(|e| e.to_string())?,
    )?;
    Ok(value)
}

fn paired() -> Result<Pairing> {
    let reference = connection_config()?.peer_ref;
    if reference.is_empty() {
        return Err("尚未导入对端配对码".into());
    }
    let bytes =
        read_secret(&secret_account(&format!("peer-{reference}"))?)?.ok_or("尚未导入对端配对码")?;
    serde_json::from_slice(&bytes).map_err(|_| "已保存的配对身份损坏".into())
}

pub(super) fn connection_config() -> Result<ConnectionConfig> {
    let path = directory()?.join("direct-connection.json");
    if path.exists() {
        let mut config: ConnectionConfig = read_json(&path)?;
        merge_legacy_backup(&mut config)?;
        Ok(config)
    } else {
        Ok(ConnectionConfig::default())
    }
}

fn usable_ip(value: &str) -> Result<Ipv4Addr> {
    let ip: Ipv4Addr = value.parse().map_err(|_| "请输入有效的 IPv4 地址")?;
    if ip.is_unspecified() || ip.is_multicast() || ip.is_broadcast() {
        return Err("请选择明确的本机或对端 IP，不使用所有网卡或广播地址".into());
    }
    Ok(ip)
}

pub(super) fn ip_list(value: &str) -> Result<Vec<Ipv4Addr>> {
    let mut ips = Vec::new();
    for part in value.split('/') {
        let ip = usable_ip(part.trim())?;
        if !ips.contains(&ip) { ips.push(ip); }
    }
    Ok(ips)
}

fn ip_text(ips: &[Ipv4Addr]) -> String {
    ips.iter().map(ToString::to_string).collect::<Vec<_>>().join("/")
}

fn peer_ips(config: &ConnectionConfig) -> Result<Vec<Ipv4Addr>> {
    let mut ips = ip_list(&config.peer_host)?;
    if !config.backup_peer_host.trim().is_empty() {
        for ip in ip_list(&config.backup_peer_host)? {
            if !ips.contains(&ip) { ips.push(ip); }
        }
    }
    Ok(ips)
}

fn merge_legacy_backup(config: &mut ConnectionConfig) -> Result<()> {
    if !config.backup_peer_host.trim().is_empty() {
        if config.peer_host.trim().is_empty() {
            config.peer_host = ip_text(&ip_list(&config.backup_peer_host)?);
        } else {
            config.peer_host = ip_text(&peer_ips(config)?);
        }
        config.backup_peer_host.clear();
    }
    Ok(())
}

// Only addresses currently assigned to this machine can be bound. Other entries
// remain advertised for another network or a router's public port mapping.
pub(super) fn local_ips(value: &str) -> Result<Vec<Ipv4Addr>> {
    Ok(ip_list(value)?.into_iter().filter(|ip| {
        std::net::UdpSocket::bind((*ip, 0)).is_ok()
    }).collect())
}

impl Pairing {
    fn addresses(&self) -> Result<String> {
        let value = if self.ips.is_empty() { self.ip.clone() } else { self.ips.join("/") };
        Ok(ip_text(&ip_list(&value)?))
    }
}

fn validate(mut config: ConnectionConfig) -> Result<ConnectionConfig> {
    config.device_name = config.device_name.trim().into();
    config.listen_ip = config.listen_ip.trim().into();
    config.peer_host = config.peer_host.trim().into();
    merge_legacy_backup(&mut config)?;
    if config.device_name.is_empty() || config.device_name.chars().count() > 64 {
        return Err("本机名称须为 1–64 个字符".into());
    }
    config.listen_ip = ip_text(&ip_list(&config.listen_ip)?);
    if !config.peer_host.is_empty() {
        config.peer_host = ip_text(&ip_list(&config.peer_host)?);
    }
    if config.listen_port == 0 || config.peer_port == 0 {
        return Err("端口须为 1–65535".into());
    }
    Ok(config)
}

fn decode_pairing(code: &str) -> Result<Pairing> {
    let code = code
        .trim()
        .strip_prefix("iterate-pair-v2:")
        .ok_or("配对码格式不正确")?;
    if code.len() > 6000 {
        return Err("配对码过长".into());
    }
    let bytes = BASE64.decode(code).map_err(|_| "配对码格式不正确")?;
    let pair: Pairing = serde_json::from_slice(&bytes).map_err(|_| "配对码内容无效")?;
    if pair.version != 2
        || pair.token.len() != 64
        || hex::decode(&pair.token).is_err()
        || pair.port == 0
    {
        return Err("配对码版本或认证材料无效".into());
    }
    uuid::Uuid::parse_str(&pair.device_id).map_err(|_| "配对设备标识无效")?;
    usable_ip(&pair.ip)?;
    pair.addresses()?;
    reqwest::Certificate::from_der(&BASE64.decode(&pair.cert).map_err(|_| "配对证书无效")?)
        .map_err(|_| "配对证书无效")?;
    Ok(pair)
}

fn client(cert: &str, address: SocketAddr, own: &Identity) -> Result<reqwest::Client> {
    let identity = reqwest::Identity::from_pem(
        format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n", own.cert, own.key).as_bytes()
    )
    .map_err(|_| "无法加载本机 TLS 身份")?;
    let cert = reqwest::Certificate::from_der(&BASE64.decode(cert).map_err(|_| "配对证书无效")?)
        .map_err(|_| "配对证书无效")?;
    reqwest::Client::builder()
        .use_rustls_tls()
        .no_proxy()
        .https_only(true)
        .tls_built_in_root_certs(false)
        .add_root_certificate(cert)
        .identity(identity)
        .resolve(TLS_NAME, address)
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| error.to_string())
}

async fn request(
    config: &ConnectionConfig,
    pair: &Pairing,
    path: &str,
    body: Option<&Value>,
) -> Result<Value> {
    request_with_identity(config, pair, path, body, &identity(false)?).await
}

async fn request_with_identity(
    config: &ConnectionConfig,
    pair: &Pairing,
    path: &str,
    body: Option<&Value>,
    own: &Identity,
) -> Result<Value> {
    let hosts = peer_ips(config)?;
    let mut failures = Vec::new();
    'endpoint: for (index, host) in hosts.into_iter().enumerate() {
        let label = if index == 0 { "主 IP" } else { "备用 IP" };
        let address = SocketAddr::new(host.into(), config.peer_port);
        let client = client(&pair.cert, address, own)?;
        let url = format!("https://{TLS_NAME}:{}{path}", config.peer_port);
        let request = if let Some(body) = body {
            client.post(url).json(body)
        } else {
            client.get(url)
        };
        let mut response = match request.bearer_auth(&pair.token).send().await {
            Ok(response) => response,
            Err(error) => {
                #[cfg(test)]
                eprintln!("TLS test connection: {error:?}");
                let _ = error;
                failures.push(format!(
                    "{label} {host}:{} 无法建立已配对的加密连接",
                    config.peer_port
                ));
                continue;
            }
        };
        if !response.status().is_success() {
            return Err(format!(
                "对端拒绝了请求 ({})，请检查配对码",
                response.status()
            ));
        }
        if response
            .content_length()
            .is_some_and(|size| size > 1024 * 1024)
        {
            return Err("对端响应过大".into());
        }
        let mut bytes = Vec::new();
        loop {
            let chunk = match response.chunk().await {
                Ok(Some(chunk)) => chunk,
                Ok(None) => break,
                Err(_) => {
                    failures.push(format!(
                        "{label} {host}:{} 连接中断，未能读取完整回执",
                        config.peer_port
                    ));
                    // /submit retries retain the same key and response; source arbitration
                    // makes an already accepted reply idempotent even if its ACK was lost.
                    continue 'endpoint;
                }
            };
            if bytes.len() + chunk.len() > 1024 * 1024 {
                return Err("对端响应过大".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let mut value: Value = serde_json::from_slice(&bytes).map_err(|_| "对端响应无效")?;
        if value.get("version").and_then(Value::as_u64) != Some(2)
            || value.get("device_id").and_then(Value::as_str) != Some(pair.device_id.as_str())
        {
            return Err("对端协议或设备身份与配对码不符".into());
        }
        // Local observations overwrite any similarly named fields sent by the peer.
        value["connected_ip"] = json!(host);
        value["using_backup"] = json!(index > 0);
        return Ok(value);
    }
    Err(format!(
        "{}。请检查地址、端口、配对信息和对端应用。",
        failures.join("；")
    ))
}

pub(super) fn credential() -> Result<String> {
    Ok(identity(false)?.token)
}
pub(super) fn validate_enable() -> Result<()> {
    let config = validate(connection_config()?)?;
    peer_ips(&config)?;
    paired()?;
    Ok(())
}

pub(super) fn route_key() -> Result<String> {
    let bytes = serde_json::to_vec(&connection_config()?).map_err(|e| e.to_string())?;
    Ok(hex::encode(ring::digest::digest(
        &ring::digest::SHA256,
        &bytes,
    )))
}
pub(super) async fn peer(path: &str, body: Option<&Value>) -> Result<Value> {
    request(&connection_config()?, &paired()?, path, body).await
}

pub(super) async fn local_daemon_ready() -> bool {
    local_daemon_health().await.is_some_and(|value| super::supports_window_sync(&value))
}

async fn local_daemon_health() -> Option<Value> {
    let Ok(config) = connection_config() else {
        return None;
    };
    let Ok(identity) = identity(false) else {
        return None;
    };
    let Ok(local) = settings() else {
        return None;
    };
    let Ok(dir) = directory() else { return None; };
    let Ok(process) = read_json::<Value>(&dir.join("daemon-process.json")) else { return None; };
    let Some(port) = process.get("health_port").and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok()).filter(|port| *port != 0) else { return None; };
    let pair = Pairing {
        version: 2,
        device_id: local.device_id,
        name: config.device_name.clone(),
        ip: Ipv4Addr::LOCALHOST.to_string(),
        ips: Vec::new(),
        port,
        cert: identity.cert,
        token: identity.token,
    };
    let local_config = ConnectionConfig {
        peer_host: Ipv4Addr::LOCALHOST.to_string(),
        backup_peer_host: String::new(),
        peer_port: port,
        ..config
    };
    request(&local_config, &pair, "/health", None).await.ok().filter(|value| {
        value.get("config_revision").and_then(Value::as_str) == route_key().ok().as_deref()
    })
}

fn pending_requests() -> bool {
    let Ok(dir) = directory() else {
        return true;
    };
    if let Ok(entries) = fs::read_dir(dir.join("requests")) {
        if entries.flatten().any(|entry| {
            read_json::<super::Registration>(&entry.path()).is_ok_and(|reg| reg.published && reg.pending())
        }) {
            return true;
        }
    }
    if let Ok(entries) = fs::read_dir(dir.join("mirror-locks")) {
        for entry in entries.flatten() {
            if let Ok(file) = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(entry.path())
            {
                if file.try_lock().is_err() {
                    return true;
                }
            }
        }
    }
    if let Ok(entries) = fs::read_dir(dir.join("mirror-leases")) {
        if entries.flatten().any(|entry| {
            entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age < Duration::from_secs(15))
        }) {
            return true;
        }
    }
    false
}

pub(super) async fn ensure_daemon() -> Result<()> {
    if local_daemon_ready().await {
        return Ok(());
    }
    let dir = directory()?;
    if !dir.join("direct-connection.json").is_file() {
        return Err("请先保存跨设备配置".into());
    }
    identity(false)?;
    // Serialize starts across all popup and service processes. The daemon owns
    // a separate lifetime lock, so a stale PID cannot authorize a process kill.
    let start_guard = lock(&dir, "daemon-start.lock")?;
    if !local_daemon_ready().await {
        if local_daemon_health().await.is_some_and(|value| !super::supports_window_sync(&value)) {
            return Err("本机跨设备服务不支持窗口同步，请更新并重启本机服务后重试".into());
        }
        let executable = std::env::var_os("ITERATE_DIALOG_GUI_EXECUTABLE")
            .map(std::path::PathBuf::from)
            .unwrap_or(std::env::current_exe().map_err(|e| e.to_string())?);
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("daemon.log"))
            .map_err(|e| e.to_string())?;
        let mut command = Command::new(executable);
        command
            .arg("--cross-device-daemon")
            .env("ITERATE_CROSS_DEVICE_DIR", &dir)
            .env_remove("ITERATE_MCP_REQUEST_FILE")
            .env_remove("ITERATE_RESPONSE_FILE")
            .env_remove("ITERATE_DELIVERY_FILE")
            .env_remove("ITERATE_READY_FILE")
            .env_remove("ITERATE_CROSS_DEVICE_MIRROR")
            .env_remove("ITERATE_CROSS_DEVICE_SOURCE")
            .env_remove("ITERATE_STANDALONE_MODE")
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|e| e.to_string())?)
            .stderr(log);
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(|_| "无法启动跨设备接收服务")?;
        for _ in 0..20 {
            if local_daemon_ready().await {
                drop(start_guard);
                return Ok(());
            }
            let _ = child.try_wait().map_err(|e| e.to_string())?;
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        if !local_daemon_ready().await {
            return Err("跨设备接收服务未就绪，请检查本机 IP、端口占用与防火墙".into());
        }
    }
    Ok(())
}

pub fn start_if_configured() {
    if directory().is_ok_and(|dir| dir.join("direct-connection.json").is_file()) {
        std::thread::spawn(|| {
            if let Ok(runtime) = tokio::runtime::Runtime::new() {
                let _ = runtime.block_on(ensure_daemon());
            }
        });
    }
}

#[tauri::command]
pub async fn get_cross_device_config() -> Result<Value> {
    let config = connection_config()?;
    let peer = paired().ok();
    Ok(
        json!({"config":config,"peer_name":peer.as_ref().map(|p| &p.name),"peer_device_id":peer.as_ref().map(|p| &p.device_id)}),
    )
}

#[tauri::command]
pub async fn generate_cross_device_pairing() -> Result<String> {
    let config = validate(connection_config()?)?;
    if !directory()?.join("direct-connection.json").exists() {
        return Err("请先保存本机 IP 和端口".into());
    }
    let own = identity(true)?;
    ensure_daemon().await?;
    let ips = ip_list(&config.listen_ip)?;
    let pair = Pairing {
        version: 2,
        device_id: settings()?.device_id,
        name: config.device_name,
        ip: ips[0].to_string(),
        ips: ips.iter().map(ToString::to_string).collect(),
        port: config.listen_port,
        cert: own.cert,
        token: own.token,
    };
    Ok(format!(
        "iterate-pair-v2:{}",
        BASE64.encode(serde_json::to_vec(&pair).map_err(|e| e.to_string())?)
    ))
}

#[tauri::command]
pub async fn save_cross_device_config(
    config: ConnectionConfig,
    pairing_code: Option<String>,
) -> Result<Value> {
    let dir = directory()?;
    let guard = lock(&dir, "connection.lock")?;
    let mut config = validate(config)?;
    let previous = connection_config()?;
    let existed = dir.join("direct-connection.json").is_file();
    config.peer_ref = previous.peer_ref.clone();
    let new_pair = pairing_code
        .filter(|code| !code.trim().is_empty())
        .map(|code| decode_pairing(&code))
        .transpose()?;
    if let Some(pair) = &new_pair {
        if pair.device_id == settings()?.device_id {
            return Err("不能导入本机自己的配对码，请粘贴另一台设备的码".into());
        }
        if config.peer_host.is_empty() {
            config.peer_host = pair.addresses()?;
        }
        config.peer_port = pair.port;
    }
    config = validate(config)?;
    let changed = previous.listen_ip != config.listen_ip
        || previous.listen_port != config.listen_port
        || previous.peer_host != config.peer_host
        || previous.backup_peer_host != config.backup_peer_host
        || previous.peer_port != config.peer_port
        || new_pair.is_some();
    if changed && pending_requests() {
        return Err("还有未完成的跨设备请求，请先处理完再更换地址、端口或配对信息".into());
    }
    if previous.listen_ip != config.listen_ip || previous.listen_port != config.listen_port {
        let available = local_ips(&config.listen_ip)?;
        if available.is_empty() {
            return Err("本机地址列表须包含至少一个当前网卡 IP；路由器公网 IP 需映射到该地址。原配置未更改".into());
        }
        let old_ips = ip_list(&previous.listen_ip)?;
        for ip in available {
            // The existing daemon owns overlapping listeners until revision reload.
            if existed && previous.listen_port == config.listen_port && old_ips.contains(&ip) { continue; }
            std::net::TcpListener::bind((ip, config.listen_port))
                .map_err(|error| match error.kind() {
                    std::io::ErrorKind::AddrInUse => format!("本机地址 {ip}:{} 已被其他进程监听，原配置未更改", config.listen_port),
                    _ => format!("无法监听本机地址 {ip}:{}：{error}，原配置未更改", config.listen_port),
                })?;
        }
    }
    identity(true)?;
    if let Some(pair) = new_pair {
        config.peer_ref = uuid::Uuid::new_v4().to_string();
        write_secret(
            &secret_account(&format!("peer-{}", config.peer_ref))?,
            &serde_json::to_vec(&pair).map_err(|e| e.to_string())?,
        )?;
    }
    atomic_json(&dir.join("direct-connection.json"), &config)?;
    if let Err(error) = ensure_daemon().await {
        if existed {
            atomic_json(&dir.join("direct-connection.json"), &previous)?;
            if let Err(restore) = ensure_daemon().await {
                return Err(format!(
                    "{error}；原配置已恢复，但原接收服务未恢复：{restore}"
                ));
            }
        } else {
            fs::remove_file(dir.join("direct-connection.json")).map_err(|e| e.to_string())?;
        }
        return Err(format!("{error}；已恢复原配置"));
    }
    drop(guard);
    get_cross_device_config().await
}

pub(super) fn server_tls() -> Result<axum_server::tls_rustls::RustlsConfig> {
    let own = identity(false)?;
    let peer = if connection_config()?.peer_ref.is_empty() {
        None
    } else {
        Some(paired()?)
    };
    tls_config(&own, peer.as_ref().map(|p| p.cert.as_str()))
}

fn tls_config(
    own: &Identity,
    peer_cert: Option<&str>,
) -> Result<axum_server::tls_rustls::RustlsConfig> {
    use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
    let cert = CertificateDer::from(BASE64.decode(&own.cert).map_err(|_| "本机证书无效")?);
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert.clone()).map_err(|_| "本机证书无效")?;
    if let Some(peer) = peer_cert {
        roots
            .add(CertificateDer::from(
                BASE64.decode(peer).map_err(|_| "对端证书无效")?,
            ))
            .map_err(|_| "对端证书无效")?;
    }
    let verifier = rustls::server::WebPkiClientVerifier::builder(std::sync::Arc::new(roots))
        .build()
        .map_err(|e| e.to_string())?;
    let mut config = rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![cert],
            PrivatePkcs8KeyDer::from(BASE64.decode(&own.key).map_err(|_| "本机私钥无效")?).into(),
        )
        .map_err(|e| e.to_string())?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(axum_server::tls_rustls::RustlsConfig::from_config(
        std::sync::Arc::new(config),
    ))
}

// Reject excess sockets before a TLS handshake and expire slow connections.
#[derive(Clone)]
pub(super) struct LimitedAcceptor(std::sync::Arc<tokio::sync::Semaphore>);
impl LimitedAcceptor {
    pub(super) fn new() -> Self {
        Self(std::sync::Arc::new(tokio::sync::Semaphore::new(32)))
    }
}
pub(super) struct LimitedStream {
    stream: tokio::net::TcpStream,
    _permit: tokio::sync::OwnedSemaphorePermit,
    deadline: std::pin::Pin<Box<tokio::time::Sleep>>,
}
impl<S> axum_server::accept::Accept<tokio::net::TcpStream, S> for LimitedAcceptor {
    type Stream = LimitedStream;
    type Service = S;
    type Future = std::future::Ready<std::io::Result<(LimitedStream, S)>>;
    fn accept(&self, stream: tokio::net::TcpStream, service: S) -> Self::Future {
        std::future::ready(
            self.0
                .clone()
                .try_acquire_owned()
                .map(|permit| {
                    (
                        LimitedStream {
                            stream,
                            _permit: permit,
                            deadline: Box::pin(tokio::time::sleep(Duration::from_secs(10))),
                        },
                        service,
                    )
                })
                .map_err(|_| std::io::Error::other("connection limit")),
        )
    }
}
impl tokio::io::AsyncRead for LimitedStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::future::Future;
        if self.deadline.as_mut().poll(cx).is_ready() {
            return std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connection timeout",
            )));
        }
        std::pin::Pin::new(&mut self.stream).poll_read(cx, buffer)
    }
}
impl tokio::io::AsyncWrite for LimitedStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buffer: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.stream).poll_write(cx, buffer)
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

#[tauri::command]
pub async fn test_cross_device_connection(
    config: ConnectionConfig,
    pairing_code: Option<String>,
) -> Result<Value> {
    let mut config = validate(config)?;
    let pair = match pairing_code.filter(|code| !code.trim().is_empty()) {
        Some(code) => decode_pairing(&code)?,
        None => paired()?,
    };
    if config.peer_host.is_empty() {
        config.peer_host = pair.addresses()?;
        config.peer_port = pair.port;
    }
    config = validate(config)?;
    let response = request(&config, &pair, "/snapshot", None).await?;
    Ok(
        json!({"message":format!("加密连接成功，已验证对端：{}；当前使用{} {}", response.get("device_name").and_then(Value::as_str).unwrap_or(&pair.name), if response["using_backup"] == true { "备用 IP" } else { "主 IP" }, response["connected_ip"].as_str().unwrap_or("")), "device_id":pair.device_id}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_specific_unicast_addresses_are_accepted() {
        assert!(usable_ip("10.8.0.3").is_ok());
        for bad in [
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "http://10.8.0.3",
            "--option",
        ] {
            assert!(usable_ip(bad).is_err());
        }
        assert!(decode_pairing("123456").is_err());
    }

    #[test]
    fn backup_address_is_optional_and_validated() {
        let legacy: ConnectionConfig = serde_json::from_value(json!({"device_name":"test", "listen_ip":"127.0.0.1", "peer_host":"10.8.0.3", "peer_port":5540})).unwrap();
        assert!(legacy.backup_peer_host.is_empty());
        assert!(validate(legacy.clone()).is_ok());
        for address in ["0.0.0.0", "invalid"] {
            assert!(validate(ConnectionConfig { backup_peer_host: address.into(), ..legacy.clone() }).is_err());
        }
        let valid = validate(ConnectionConfig { backup_peer_host: " 10.8.0.4 ".into(), ..legacy }).unwrap();
        assert_eq!(valid.peer_host, "10.8.0.3/10.8.0.4");
        assert!(valid.backup_peer_host.is_empty());
    }

    #[test]
    fn multiple_addresses_preserve_priority_and_legacy_pairing() {
        assert_eq!(ip_text(&ip_list(" 192.168.1.10 / 10.8.0.4 / 203.0.113.10 /10.8.0.4").unwrap()),
            "192.168.1.10/10.8.0.4/203.0.113.10");
        for bad in ["", "127.0.0.1/", "/127.0.0.1", "127.0.0.1//127.0.0.2", "127.0.0.1/0.0.0.0", "127.0.0.1/host"] {
            assert!(ip_list(bad).is_err(), "{bad}");
        }
        let mut config: ConnectionConfig = serde_json::from_value(json!({"listen_ip":"127.0.0.1", "peer_host":"10.0.0.1", "backup_peer_host":"10.0.0.2"})).unwrap();
        merge_legacy_backup(&mut config).unwrap();
        assert_eq!(config.peer_host, "10.0.0.1/10.0.0.2");
        assert!(serde_json::to_value(config).unwrap().get("backup_peer_host").is_none());
        let mut pair: Pairing = serde_json::from_value(json!({"version":2,"device_id":"test", "name":"test", "ip":"10.0.0.1", "port":5540,"cert":"test","token":"test"})).unwrap();
        assert_eq!(pair.addresses().unwrap(), "10.0.0.1");
        pair.ips = vec!["10.0.0.1".into(), "192.168.1.2".into(), "203.0.113.1".into()];
        assert_eq!(pair.addresses().unwrap(), "10.0.0.1/192.168.1.2/203.0.113.1");
        let available = local_ips("203.0.113.254/127.0.0.1").unwrap();
        assert!(available.contains(&Ipv4Addr::LOCALHOST));
        assert!(!available.contains(&"203.0.113.254".parse().unwrap()));
    }

    #[tokio::test]
    async fn tls_requires_the_paired_certificate_token_and_device_id() {
        use axum::{
            http::{HeaderMap, StatusCode},
            routing::get,
            Json, Router,
        };
        let _ = rustls::crypto::ring::default_provider().install_default();
        let own = rcgen::generate_simple_self_signed(vec![TLS_NAME.into()]).unwrap();
        let wrong = rcgen::generate_simple_self_signed(vec![TLS_NAME.into()]).unwrap();
        let cert = BASE64.encode(own.cert.der());
        let server_identity = Identity {
            cert: cert.clone(),
            key: BASE64.encode(own.key_pair.serialize_der()),
            token: "test-token".into(),
        };
        let client_cert = rcgen::generate_simple_self_signed(vec![TLS_NAME.into()]).unwrap();
        let client_identity = Identity {
            cert: BASE64.encode(client_cert.cert.der()),
            key: BASE64.encode(client_cert.key_pair.serialize_der()),
            token: String::new(),
        };
        let stranger = Identity {
            cert: BASE64.encode(wrong.cert.der()),
            key: BASE64.encode(wrong.key_pair.serialize_der()),
            token: String::new(),
        };
        let tls = tls_config(&server_identity, Some(&client_identity.cert)).unwrap();
        let reload_tls = tls.clone();
        let handle = axum_server::Handle::new();
        let server_handle = handle.clone();
        let app = Router::new().route(
            "/snapshot",
            get(|headers: HeaderMap| async move {
                if headers.get("authorization").and_then(|v| v.to_str().ok())
                    != Some("Bearer test-token")
                {
                    return Err(StatusCode::UNAUTHORIZED);
                }
                Ok(Json(json!({"version":2,"device_id":"test-device"})))
            }),
        );
        let server = tokio::spawn(async move {
            axum_server::bind_rustls("127.0.0.1:0".parse().unwrap(), tls)
                .handle(server_handle)
                .serve(app.into_make_service())
                .await
                .unwrap();
        });
        let address = handle.listening().await.unwrap();
        let config = ConnectionConfig {
            peer_host: "127.0.0.1".into(),
            peer_port: address.port(),
            ..Default::default()
        };
        let pair = Pairing {
            version: 2,
            device_id: "test-device".into(),
            name: "test".into(),
            ip: "127.0.0.1".into(),
            ips: Vec::new(),
            port: address.port(),
            cert,
            token: "test-token".into(),
        };
        request_with_identity(&config, &pair, "/snapshot", None, &client_identity)
            .await
            .unwrap();
        let fallback = ConnectionConfig {
            peer_host: "127.0.0.2".into(),
            backup_peer_host: "127.0.0.1".into(),
            ..config.clone()
        };
        let response = request_with_identity(&fallback, &pair, "/snapshot", None, &client_identity).await.unwrap();
        assert_eq!(response["connected_ip"], "127.0.0.1");
        assert_eq!(response["using_backup"], true);
        let multiple = ConnectionConfig { peer_host: "127.0.0.2/127.0.0.3/127.0.0.1".into(), backup_peer_host: String::new(), ..config.clone() };
        let response = request_with_identity(&multiple, &pair, "/snapshot", None, &client_identity).await.unwrap();
        assert_eq!(response["connected_ip"], "127.0.0.1");
        assert_eq!(response["using_backup"], true);
        let primary = ConnectionConfig { backup_peer_host: "127.0.0.2".into(), ..config.clone() };
        let response = request_with_identity(&primary, &pair, "/snapshot", None, &client_identity).await.unwrap();
        assert_eq!(response["using_backup"], false);
        let unavailable = ConnectionConfig { backup_peer_host: "127.0.0.3".into(), ..fallback.clone() };
        let error = request_with_identity(&unavailable, &pair, "/snapshot", None, &client_identity).await.unwrap_err();
        assert!(error.contains("127.0.0.2") && error.contains("127.0.0.3"));
        // A reachable but untrusted backup never becomes a successful connection.
        assert!(request_with_identity(&fallback, &Pairing { cert: BASE64.encode(wrong.cert.der()), ..pair.clone() }, "/snapshot", None, &client_identity).await.is_err());
        let rejected = request_with_identity(&primary, &Pairing { token: "wrong".into(), ..pair.clone() }, "/snapshot", None, &client_identity).await.unwrap_err();
        assert!(rejected.contains("401"));
        assert!(request_with_identity(
            &config,
            &Pairing {
                cert: BASE64.encode(wrong.cert.der()),
                ..pair.clone()
            },
            "/snapshot",
            None,
            &client_identity
        )
        .await
        .is_err());
        assert!(request_with_identity(
            &config,
            &Pairing {
                token: "wrong".into(),
                ..pair.clone()
            },
            "/snapshot",
            None,
            &client_identity
        )
        .await
        .is_err());
        assert!(request_with_identity(
            &config,
            &Pairing {
                device_id: "wrong-device".into(),
                ..pair.clone()
            },
            "/snapshot",
            None,
            &client_identity
        )
        .await
        .is_err());
        assert!(
            request_with_identity(&config, &pair, "/snapshot", None, &stranger)
                .await
                .is_err()
        );
        reload_tls.reload_from_config(
            tls_config(&server_identity, Some(&stranger.cert))
                .unwrap()
                .get_inner(),
        );
        assert!(
            request_with_identity(&config, &pair, "/snapshot", None, &client_identity)
                .await
                .is_err(),
            "replaced peer must lose inbound access"
        );
        request_with_identity(&config, &pair, "/snapshot", None, &stranger)
            .await
            .unwrap();
        handle.shutdown();
        server.await.unwrap();
    }
}
