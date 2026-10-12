use anyhow::{anyhow, Result};
use axum::{
    extract::{
        ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use ring::digest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    fs::OpenOptions,
    io::Write,
    net::SocketAddr,
    path::PathBuf,
    process::Command,
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpListener,
    sync::{broadcast, mpsc},
    task::JoinHandle,
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message as TungsteniteMessage},
};
use uuid::Uuid;

const DEFAULT_COMMAND_TTL_SECS: i64 = 60;
const MAX_COMMANDS: usize = 200;
const MAX_AUDIT_EVENTS: usize = 400;
const MAX_REPLAY_ENTRIES: usize = 400;
const MAX_RELAY_STREAM_MESSAGE_BYTES: usize = 16 * 1024 * 1024;
const RELAY_STREAM_CHANNEL_CAPACITY: usize = 256;
const RELAY_MAC_CLIENT_LABEL: &str = "com.cunzhi.iterate.relay-mac-client";
const RELAY_MOBILE_PAIRING_TOKEN_TTL_SECS: i64 = 10 * 60;
const RELAY_MOBILE_DEVICE_TOKEN_TTL_SECS: i64 = 365 * 24 * 60 * 60;
const RELAY_MOBILE_SCOPES: &[&str] = &[
    "status.read",
    "session.read",
    "session.stream",
    "session.respond",
];

#[derive(Debug, Clone)]
pub struct RelayServerConfig {
    pub host: String,
    pub port: u16,
    pub token: Option<String>,
    pub audit_log_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct RelayMacClientConfig {
    pub relay_url: String,
    pub device_id: String,
    pub token: Option<String>,
    pub local_base_url: String,
    pub heartbeat_secs: u64,
    pub allow_recover: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct RelayMobilePairingConfig {
    pub base_url: String,
    pub ws_url: String,
    pub relay_device_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_pairing_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_pairing_expires_at: Option<String>,
    pub token_present: bool,
    pub process_running: bool,
    pub launchctl_loaded: bool,
}

#[derive(Clone)]
struct RelayServerState {
    inner: Arc<Mutex<RelayInner>>,
    token: Option<String>,
}

struct RelayInner {
    started_at: String,
    devices: HashMap<String, RelayDevice>,
    statuses: HashMap<String, Value>,
    commands: HashMap<String, RelayCommand>,
    command_order: VecDeque<String>,
    audit: VecDeque<RelayAuditEvent>,
    mac_senders: HashMap<String, MacSenderEntry>,
    stream_senders: HashMap<String, broadcast::Sender<String>>,
    mobile_stream_revocations: HashMap<String, broadcast::Sender<String>>,
    cooldown_until: HashMap<String, String>,
    mobile_pairing_tokens: HashMap<String, RelayMobilePairingToken>,
    mobile_credentials: HashMap<String, RelayMobileCredential>,
    audit_log_path: Option<PathBuf>,
}

struct MacSenderEntry {
    connection_id: String,
    sender: mpsc::UnboundedSender<ServerToMacMessage>,
}

struct AbortOnDrop(JoinHandle<()>);

#[derive(Debug, Clone, Serialize)]
struct LocalBridgeHealth {
    status: String,
    updated_at: String,
    retry_attempt: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_error: Option<String>,
}

impl LocalBridgeHealth {
    fn new() -> Self {
        Self {
            status: "starting".to_string(),
            updated_at: Utc::now().to_rfc3339(),
            retry_attempt: 0,
            last_error: None,
        }
    }

    fn record(&mut self, status: &str, retry_attempt: u32, last_error: Option<String>) {
        self.status = status.to_string();
        self.updated_at = Utc::now().to_rfc3339();
        self.retry_attempt = retry_attempt;
        self.last_error = last_error;
    }
}

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl RelayInner {
    fn new_with_audit_log_path(audit_log_path: Option<PathBuf>) -> Self {
        Self {
            started_at: Utc::now().to_rfc3339(),
            devices: HashMap::new(),
            statuses: HashMap::new(),
            commands: HashMap::new(),
            command_order: VecDeque::new(),
            audit: VecDeque::new(),
            mac_senders: HashMap::new(),
            stream_senders: HashMap::new(),
            mobile_stream_revocations: HashMap::new(),
            cooldown_until: HashMap::new(),
            mobile_pairing_tokens: HashMap::new(),
            mobile_credentials: HashMap::new(),
            audit_log_path,
        }
    }

    fn push_audit(
        &mut self,
        kind: &str,
        device_id: Option<&str>,
        command_id: Option<&str>,
        metadata: Value,
    ) {
        let event = RelayAuditEvent {
            event_id: format!("evt_{}", Uuid::new_v4()),
            kind: kind.to_string(),
            device_id: device_id.map(str::to_string),
            command_id: command_id.map(str::to_string),
            at: Utc::now().to_rfc3339(),
            metadata,
        };
        self.persist_audit_event(&event);
        self.audit.push_front(event);
        while self.audit.len() > MAX_AUDIT_EVENTS {
            self.audit.pop_back();
        }
    }

    fn persist_audit_event(&self, event: &RelayAuditEvent) {
        let Some(path) = &self.audit_log_path else {
            return;
        };
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "relay audit log parent create failed path={} error={error}",
                    parent.display()
                );
                return;
            }
        }
        let line = match serde_json::to_string(event) {
            Ok(line) => line,
            Err(error) => {
                eprintln!("relay audit event serialize failed: {error}");
                return;
            }
        };
        match OpenOptions::new().create(true).append(true).open(path) {
            Ok(mut file) => {
                if let Err(error) = writeln!(file, "{line}") {
                    eprintln!(
                        "relay audit log write failed path={} error={error}",
                        path.display()
                    );
                }
            }
            Err(error) => {
                eprintln!(
                    "relay audit log open failed path={} error={error}",
                    path.display()
                );
            }
        }
    }

    fn insert_command(&mut self, command: RelayCommand) {
        self.command_order.push_front(command.command_id.clone());
        self.commands.insert(command.command_id.clone(), command);
        while self.command_order.len() > MAX_COMMANDS {
            if let Some(old_id) = self.command_order.pop_back() {
                self.commands.remove(&old_id);
            }
        }
    }

    fn register_mac_sender(
        &mut self,
        device_id: String,
        connection_id: String,
        sender: mpsc::UnboundedSender<ServerToMacMessage>,
    ) {
        self.mac_senders.insert(
            device_id,
            MacSenderEntry {
                connection_id,
                sender,
            },
        );
    }

    fn remove_mac_sender_if_current(&mut self, device_id: &str, connection_id: &str) -> bool {
        let is_current = self
            .mac_senders
            .get(device_id)
            .map(|entry| entry.connection_id == connection_id)
            .unwrap_or(false);
        if is_current {
            self.mac_senders.remove(device_id);
        }
        is_current
    }

    fn stream_sender(&mut self, device_id: &str) -> broadcast::Sender<String> {
        self.stream_senders
            .entry(device_id.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(RELAY_STREAM_CHANNEL_CAPACITY);
                tx
            })
            .clone()
    }

    fn mobile_stream_revocation_sender(
        &mut self,
        credential_hash: &str,
    ) -> broadcast::Sender<String> {
        self.mobile_stream_revocations
            .entry(credential_hash.to_string())
            .or_insert_with(|| {
                let (tx, _) = broadcast::channel(16);
                tx
            })
            .clone()
    }

    fn revoke_mobile_streams(&mut self, credential_hash: &str) -> usize {
        let Some(sender) = self.mobile_stream_revocations.remove(credential_hash) else {
            return 0;
        };
        let receivers = sender.receiver_count();
        let _ = sender.send("mobile_credential_revoked".to_string());
        receivers
    }

    fn prune_mobile_pairing_tokens(&mut self, now: DateTime<Utc>) {
        self.mobile_pairing_tokens.retain(|_, token| {
            token.consumed_at.is_none()
                && parse_utc_datetime(&token.expires_at)
                    .map(|expires_at| expires_at > now)
                    .unwrap_or(false)
        });
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayDevice {
    pub device_id: String,
    pub workspace_id: Option<String>,
    pub app_version: Option<String>,
    pub status: String,
    pub last_seen_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayCommand {
    pub command_id: String,
    pub device_id: String,
    #[serde(rename = "type")]
    pub command_type: String,
    pub scope: String,
    pub status: String,
    pub nonce: String,
    pub created_at: String,
    pub expires_at: String,
    pub delivered_at: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub result: Option<Value>,
    #[serde(default)]
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RelayAuditEvent {
    event_id: String,
    kind: String,
    device_id: Option<String>,
    command_id: Option<String>,
    at: String,
    metadata: Value,
}

#[derive(Debug, Clone)]
struct RelayMobilePairingToken {
    relay_device_id: String,
    expires_at: String,
    created_at: String,
    consumed_at: Option<String>,
}

#[derive(Debug, Clone)]
struct RelayMobileCredential {
    mobile_device_id: String,
    mobile_device_name: String,
    client_kind: String,
    relay_device_id: String,
    scopes: Vec<String>,
    created_at: String,
    expires_at: String,
    revoked_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateRelayCommandRequest {
    #[serde(rename = "type")]
    command_type: String,
    #[serde(default)]
    payload: Value,
}

#[derive(Debug, Deserialize)]
struct RelayMobilePairingClaimRequest {
    pairing_token: String,
    #[serde(default)]
    device_id: String,
    #[serde(default)]
    device_name: Option<String>,
    #[serde(default)]
    client_kind: Option<String>,
    #[serde(default)]
    platform: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelayMobileCredentialRevokeRequest {
    #[serde(default)]
    device_token: Option<String>,
    #[serde(default)]
    mobile_token: Option<String>,
    #[serde(default)]
    device_id: Option<String>,
    #[serde(default)]
    mobile_device_id: Option<String>,
    #[serde(default)]
    relay_device_id: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelayMobilePairingIssueRequest {
    #[serde(default)]
    device_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelayMobilePairingIssueResponse {
    ok: bool,
    relay_device_id: String,
    relay_pairing_token: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
struct MacSocketQuery {
    device_id: Option<String>,
    token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RelaySocketQuery {
    token: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct ServerToMacMessage {
    kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<RelayCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<Value>,
}

#[derive(Clone, Copy)]
struct CommandSpec {
    scope: &'static str,
    cooldown_secs: i64,
}

#[derive(Default)]
struct DeviceAuthorization {
    mobile_credential_hash: Option<String>,
}

#[derive(Default)]
struct RelayClientReplayGuard {
    command_ids: VecDeque<(String, DateTime<Utc>)>,
    nonces: VecDeque<(String, DateTime<Utc>)>,
}

impl RelayClientReplayGuard {
    fn register(&mut self, command: &RelayCommand, expires_at: DateTime<Utc>) -> Result<()> {
        if command.command_id.trim().is_empty() {
            return Err(anyhow!("command missing command_id"));
        }
        if command.nonce.trim().is_empty() {
            return Err(anyhow!("command missing nonce"));
        }

        self.prune(Utc::now());

        if self
            .command_ids
            .iter()
            .any(|(command_id, _)| command_id == &command.command_id)
        {
            return Err(anyhow!("command replay rejected: duplicate command_id"));
        }
        if self.nonces.iter().any(|(nonce, _)| nonce == &command.nonce) {
            return Err(anyhow!("command replay rejected: duplicate nonce"));
        }

        self.command_ids
            .push_front((command.command_id.clone(), expires_at));
        self.nonces.push_front((command.nonce.clone(), expires_at));

        while self.command_ids.len() > MAX_REPLAY_ENTRIES {
            self.command_ids.pop_back();
        }
        while self.nonces.len() > MAX_REPLAY_ENTRIES {
            self.nonces.pop_back();
        }

        Ok(())
    }

    fn prune(&mut self, now: DateTime<Utc>) {
        self.command_ids.retain(|(_, expires_at)| *expires_at > now);
        self.nonces.retain(|(_, expires_at)| *expires_at > now);
    }
}

pub async fn start_relay_server(config: RelayServerConfig) -> Result<()> {
    if config.token.is_none() && !is_loopback_host(&config.host) {
        return Err(anyhow!(
            "refusing to start relay on non-loopback host without --relay-token-env"
        ));
    }

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let state = RelayServerState {
        inner: Arc::new(Mutex::new(RelayInner::new_with_audit_log_path(
            config.audit_log_path,
        ))),
        token: config.token,
    };

    let app = Router::new()
        .route("/health", get(handle_health))
        .route("/mac/ws", get(handle_mac_ws))
        .route("/api/devices", get(handle_devices))
        .route("/api/devices/:device_id/status", get(handle_device_status))
        .route(
            "/api/devices/:device_id/sessions",
            get(handle_device_sessions),
        )
        .route("/api/devices/:device_id/stream", get(handle_device_stream))
        .route(
            "/api/devices/:device_id/mobile-pairing",
            post(handle_issue_mobile_pairing),
        )
        .route(
            "/api/devices/:device_id/commands",
            post(handle_create_command),
        )
        .route(
            "/api/mobile/pairing/claim",
            post(handle_claim_mobile_pairing),
        )
        .route(
            "/api/mobile/credentials/revoke",
            post(handle_revoke_mobile_credential),
        )
        .route("/api/commands/:command_id", get(handle_command))
        .route("/api/audit", get(handle_audit))
        .with_state(state);

    let listener = TcpListener::bind(addr).await?;
    println!(
        "iterate relay server listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, app).await?;
    Ok(())
}

pub async fn start_relay_mac_client(config: RelayMacClientConfig) -> Result<()> {
    let mut replay_guard = RelayClientReplayGuard::default();
    let mut reconnect_attempt: u32 = 0;

    loop {
        let session_started_at = tokio::time::Instant::now();
        let result = run_relay_mac_client_once(&config, &mut replay_guard).await;
        let session_duration = session_started_at.elapsed();
        let reset_backoff =
            relay_should_reset_reconnect_attempt(session_duration, config.heartbeat_secs);

        match result {
            Ok(()) => {
                eprintln!("relay mac client disconnected; reconnecting");
            }
            Err(error) => {
                eprintln!("relay mac client connection failed: {error}");
            }
        }
        if reset_backoff {
            reconnect_attempt = 0;
            eprintln!(
                "relay mac client reconnect backoff reset after {}s session",
                session_duration.as_secs()
            );
        } else {
            reconnect_attempt = reconnect_attempt.saturating_add(1);
        }

        let delay = relay_reconnect_delay(reconnect_attempt);
        eprintln!(
            "relay mac client reconnect scheduled in {}s (attempt={})",
            delay.as_secs(),
            reconnect_attempt
        );
        tokio::time::sleep(delay).await;
    }
}

async fn run_relay_mac_client_once(
    config: &RelayMacClientConfig,
    replay_guard: &mut RelayClientReplayGuard,
) -> Result<()> {
    let relay_ws_url = relay_mac_ws_url(&config.relay_url, &config.device_id);
    let mut request = relay_ws_url.clone().into_client_request()?;
    if let Some(token) = &config.token {
        request.headers_mut().insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}"))?,
        );
    }

    println!("iterate relay mac client connecting to {}", relay_ws_url);
    println!("device_id={}", config.device_id);
    println!("local_base_url={}", config.local_base_url);
    println!("allow_recover={}", config.allow_recover);

    let (ws_stream, _) = connect_async(request).await?;
    let (mut writer, mut reader) = ws_stream.split();
    let (relay_out_tx, mut relay_out_rx) = mpsc::unbounded_channel::<Value>();
    let (local_bridge_tx, local_bridge_rx) = mpsc::unbounded_channel::<Value>();
    let local_bridge_health = Arc::new(Mutex::new(LocalBridgeHealth::new()));
    let _local_bridge_task = AbortOnDrop(tokio::spawn(run_local_bridge_ws_loop(
        config.local_base_url.clone(),
        config.device_id.clone(),
        relay_out_tx.clone(),
        local_bridge_rx,
        local_bridge_health.clone(),
    )));

    send_tungstenite_json(
        &mut writer,
        &json!({
            "kind": "hello",
            "device_id": config.device_id,
            "workspace_id": current_workspace_id(),
            "app_version": env!("CARGO_PKG_VERSION"),
            "capabilities": [
                "status.read",
                "session.read",
                "session.respond",
                "session.stream",
                "bridge.recover",
                "tunnel.recover"
            ]
        }),
    )
    .await?;

    let heartbeat_secs = config.heartbeat_secs.max(5);
    let mut interval = tokio::time::interval(Duration::from_secs(heartbeat_secs));

    loop {
        tokio::select! {
            _ = interval.tick() => {
                let summary = relay_status_snapshot(
                    &config.local_base_url,
                    &local_bridge_health,
                ).await;
                send_tungstenite_json(
                    &mut writer,
                    &json!({
                        "kind": "heartbeat",
                        "device_id": config.device_id,
                        "sent_at": Utc::now().to_rfc3339(),
                        "status": summary
                    }),
                ).await?;
            }
            outbound = relay_out_rx.recv() => {
                if let Some(outbound) = outbound {
                    send_tungstenite_json(&mut writer, &outbound).await?;
                }
            }
            message = reader.next() => {
                let Some(message) = message else {
                    return Ok(());
                };
                let message = message?;
                match message {
                    TungsteniteMessage::Text(text) => {
                        if let Err(error) = handle_relay_client_text(
                            config,
                            &mut writer,
                            &text,
                            replay_guard,
                            &local_bridge_tx,
                        )
                        .await
                        {
                            eprintln!("relay client command handling failed: {error}");
                        }
                    }
                    TungsteniteMessage::Ping(payload) => {
                        writer.send(TungsteniteMessage::Pong(payload)).await?;
                    }
                    TungsteniteMessage::Close(_) => {
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }
    }
}

fn relay_reconnect_delay(attempt: u32) -> Duration {
    let clamped = attempt.min(6);
    Duration::from_secs((2_u64).saturating_pow(clamped).min(60))
}

fn relay_reconnect_reset_after(heartbeat_secs: u64) -> Duration {
    Duration::from_secs(heartbeat_secs.max(5).saturating_mul(2))
}

fn relay_should_reset_reconnect_attempt(session_duration: Duration, heartbeat_secs: u64) -> bool {
    session_duration >= relay_reconnect_reset_after(heartbeat_secs)
}

async fn run_local_bridge_ws_loop(
    local_base_url: String,
    device_id: String,
    relay_out_tx: mpsc::UnboundedSender<Value>,
    mut local_bridge_rx: mpsc::UnboundedReceiver<Value>,
    health: Arc<Mutex<LocalBridgeHealth>>,
) {
    let local_ws_url = local_bridge_ws_url(&local_base_url);
    let mut reconnect_attempt: u32 = 0;

    loop {
        let mut request = match local_ws_url.clone().into_client_request() {
            Ok(request) => request,
            Err(error) => {
                health
                    .lock()
                    .record("invalid_url", reconnect_attempt, Some(error.to_string()));
                eprintln!("relay local bridge websocket URL invalid: {error}");
                return;
            }
        };
        let token = match crate::bridge::auth::issue_internal_bridge_websocket_token(&local_ws_url)
        {
            Ok(token) => token,
            Err(error) => {
                reconnect_attempt = reconnect_attempt.saturating_add(1);
                health
                    .lock()
                    .record("auth_unavailable", reconnect_attempt, Some(error.clone()));
                let delay = relay_reconnect_delay(reconnect_attempt);
                eprintln!(
                    "relay local bridge websocket auth unavailable: {error}; retrying in {}s (attempt={})",
                    delay.as_secs(),
                    reconnect_attempt
                );
                tokio::time::sleep(delay).await;
                continue;
            }
        };
        let Ok(authorization) = HeaderValue::from_str(&format!("Bearer {token}")) else {
            health.lock().record(
                "invalid_auth_header",
                reconnect_attempt,
                Some("invalid authorization header".to_string()),
            );
            eprintln!("relay local bridge websocket auth header invalid");
            return;
        };
        request.headers_mut().insert("authorization", authorization);

        match connect_async(request).await {
            Ok((ws_stream, _)) => {
                eprintln!(
                    "relay mac client connected to local bridge {}",
                    local_ws_url
                );
                reconnect_attempt = 0;
                health.lock().record("healthy", 0, None);
                let (mut local_writer, mut local_reader) = ws_stream.split();

                loop {
                    tokio::select! {
                        outbound = local_bridge_rx.recv() => {
                            let Some(message) = outbound else {
                                return;
                            };
                            let Ok(text) = serde_json::to_string(&message) else {
                                continue;
                            };
                            if local_writer.send(TungsteniteMessage::Text(text)).await.is_err() {
                                break;
                            }
                        }
                        inbound = local_reader.next() => {
                            match inbound {
                                Some(Ok(TungsteniteMessage::Text(text))) => {
                                    match relay_bridge_stream_message_from_text(&text) {
                                        Ok(message) => {
                                            let _ = relay_out_tx.send(json!({
                                                "kind": "bridge_message",
                                                "device_id": device_id,
                                                "sent_at": Utc::now().to_rfc3339(),
                                                "message": message,
                                            }));
                                        }
                                        Err(error) => {
                                            eprintln!("relay local bridge message ignored: {error}");
                                        }
                                    }
                                }
                                Some(Ok(TungsteniteMessage::Ping(payload))) => {
                                    if local_writer
                                        .send(TungsteniteMessage::Pong(payload))
                                        .await
                                        .is_err()
                                    {
                                        break;
                                    }
                                }
                                Some(Ok(TungsteniteMessage::Close(_))) | None => break,
                                Some(Ok(_)) => {}
                                Some(Err(error)) => {
                                    eprintln!("relay local bridge websocket error: {error}");
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            Err(error) => {
                health.lock().record(
                    "connection_failed",
                    reconnect_attempt.saturating_add(1),
                    Some(error.to_string()),
                );
                eprintln!(
                    "relay mac client local bridge connection failed url={} error={error}",
                    local_ws_url
                );
            }
        }

        reconnect_attempt = reconnect_attempt.saturating_add(1);
        if health.lock().status == "healthy" {
            health.lock().record(
                "disconnected",
                reconnect_attempt,
                Some("local bridge websocket disconnected".to_string()),
            );
        }
        let delay = relay_reconnect_delay(reconnect_attempt);
        tokio::time::sleep(delay).await;
    }
}

async fn handle_health(State(state): State<RelayServerState>) -> Response {
    let inner = state.inner.lock();
    Json(json!({
        "ok": true,
        "started_at": inner.started_at,
        "devices": inner.devices.len(),
        "commands": inner.commands.len(),
        "auth_required": state.token.is_some(),
    }))
    .into_response()
}

async fn handle_devices(State(state): State<RelayServerState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize_http(&state, &headers) {
        return response;
    }
    let inner = state.inner.lock();
    let devices: Vec<_> = inner.devices.values().cloned().collect();
    Json(json!({ "devices": devices })).into_response()
}

async fn handle_device_status(
    State(state): State<RelayServerState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
) -> Response {
    if let Err(response) = authorize_device_http(&state, &headers, &device_id, "status.read") {
        return response;
    }
    let inner = state.inner.lock();
    let device = inner.devices.get(&device_id).cloned();
    let status = inner.statuses.get(&device_id).cloned();
    Json(json!({
        "device": device,
        "status": status,
    }))
    .into_response()
}

async fn handle_device_sessions(
    State(state): State<RelayServerState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
) -> Response {
    if let Err(response) = authorize_device_http(&state, &headers, &device_id, "session.read") {
        return response;
    }
    let inner = state.inner.lock();
    let device = inner.devices.get(&device_id).cloned();
    let status = inner.statuses.get(&device_id).cloned();
    let sessions = status
        .as_ref()
        .map(relay_active_sessions_from_status)
        .unwrap_or_default();
    Json(json!({
        "device": device,
        "sessions": sessions,
        "source": "last_heartbeat",
    }))
    .into_response()
}

async fn handle_device_stream(
    State(state): State<RelayServerState>,
    headers: HeaderMap,
    Query(query): Query<RelaySocketQuery>,
    Path(device_id): Path<String>,
    ws: WebSocketUpgrade,
) -> Response {
    if device_id.trim().is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "missing_device_id");
    }
    let authorization = match authorize_device_ws(
        &state,
        &headers,
        query.token.as_deref(),
        &device_id,
        "session.stream",
    ) {
        Ok(authorization) => authorization,
        Err(response) => return response,
    };
    let revocation_rx = authorization.mobile_credential_hash.as_deref().map(|hash| {
        let mut inner = state.inner.lock();
        inner.mobile_stream_revocation_sender(hash).subscribe()
    });
    ws.on_upgrade(move |socket| {
        handle_device_stream_socket(state, device_id, socket, revocation_rx)
    })
}

async fn handle_device_stream_socket(
    state: RelayServerState,
    device_id: String,
    socket: WebSocket,
    mut revocation_rx: Option<broadcast::Receiver<String>>,
) {
    let stream_id = format!("stream_{}", Uuid::new_v4());
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let mut rx = {
        let mut inner = state.inner.lock();
        let sender = inner.stream_sender(&device_id);
        inner.push_audit(
            "mobile_stream_connected",
            Some(&device_id),
            None,
            json!({ "stream_id": stream_id.clone() }),
        );
        sender.subscribe()
    };
    let mut ping_interval = tokio::time::interval(Duration::from_secs(30));

    loop {
        tokio::select! {
            _ = ping_interval.tick() => {
                if ws_sender
                    .send(AxumMessage::Ping(vec![b'p', b'i', b'n', b'g']))
                    .await
                    .is_err()
                {
                    break;
                }
            }
            result = rx.recv() => {
                match result {
                    Ok(text) => {
                        if ws_sender.send(AxumMessage::Text(text)).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        continue;
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = async {
                if let Some(rx) = revocation_rx.as_mut() {
                    let _ = rx.recv().await;
                }
                else {
                    std::future::pending::<()>().await;
                }
            } => {
                let _ = ws_sender
                    .send(AxumMessage::Text(relay_error_message("mobile_credential_revoked")))
                    .await;
                let _ = ws_sender.send(AxumMessage::Close(None)).await;
                break;
            }
            message = ws_receiver.next() => {
                match message {
                    Some(Ok(AxumMessage::Text(text))) => {
                        match relay_mobile_bridge_message_from_text(&text) {
                            Ok(message) => {
                                if let Err(error) = forward_mobile_bridge_message_to_mac(
                                    &state,
                                    &device_id,
                                    message,
                                ) {
                                    let _ = ws_sender
                                        .send(AxumMessage::Text(relay_error_message(&error)))
                                        .await;
                                }
                            }
                            Err(error) => {
                                let _ = ws_sender
                                    .send(AxumMessage::Text(relay_error_message(&error.to_string())))
                                    .await;
                            }
                        }
                    }
                    Some(Ok(AxumMessage::Ping(payload))) => {
                        if ws_sender.send(AxumMessage::Pong(payload)).await.is_err() {
                            break;
                        }
                    }
                    Some(Ok(AxumMessage::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }

    let mut inner = state.inner.lock();
    inner.push_audit(
        "mobile_stream_disconnected",
        Some(&device_id),
        None,
        json!({ "stream_id": stream_id }),
    );
}

fn forward_mobile_bridge_message_to_mac(
    state: &RelayServerState,
    device_id: &str,
    message: Value,
) -> Result<(), String> {
    let message_id = format!("msg_{}", Uuid::new_v4());
    let mut inner = state.inner.lock();
    let Some(sender) = inner
        .mac_senders
        .get(device_id)
        .map(|entry| entry.sender.clone())
    else {
        inner.push_audit(
            "mobile_stream_message_rejected",
            Some(device_id),
            None,
            json!({
                "message_id": message_id,
                "reason": "mac_offline",
            }),
        );
        return Err("mac_offline".to_string());
    };

    sender
        .send(ServerToMacMessage {
            kind: "bridge_client_message",
            command: None,
            message_id: Some(message_id.clone()),
            message: Some(message),
        })
        .map_err(|_| "mac_sender_closed".to_string())?;
    inner.push_audit(
        "mobile_stream_message_forwarded",
        Some(device_id),
        None,
        json!({ "message_id": message_id }),
    );
    Ok(())
}

async fn handle_issue_mobile_pairing(
    State(state): State<RelayServerState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<RelayMobilePairingIssueRequest>,
) -> Response {
    let relay_device_id = device_id.trim();
    if relay_device_id.is_empty() {
        return json_error(StatusCode::BAD_REQUEST, "missing_device_id");
    }
    if let Err(response) = authorize_http(&state, &headers) {
        return response;
    }

    let now = Utc::now();
    let expires_at = now + ChronoDuration::seconds(RELAY_MOBILE_PAIRING_TOKEN_TTL_SECS);
    let pairing_token = generate_relay_token("rpt");
    let token_hash = relay_token_hash(&pairing_token);
    let scopes = relay_mobile_scopes();
    {
        let mut inner = state.inner.lock();
        inner.prune_mobile_pairing_tokens(now);
        inner.mobile_pairing_tokens.insert(
            token_hash,
            RelayMobilePairingToken {
                relay_device_id: relay_device_id.to_string(),
                expires_at: expires_at.to_rfc3339(),
                created_at: now.to_rfc3339(),
                consumed_at: None,
            },
        );
        inner.push_audit(
            "mobile_pairing_issued",
            Some(relay_device_id),
            None,
            json!({
                "expires_at": expires_at.to_rfc3339(),
                "device_name": request.device_name,
            }),
        );
    }

    Json(json!({
        "ok": true,
        "relay_device_id": relay_device_id,
        "relay_pairing_token": pairing_token,
        "expires_at": expires_at.to_rfc3339(),
        "scopes": scopes,
    }))
    .into_response()
}

async fn handle_claim_mobile_pairing(
    State(state): State<RelayServerState>,
    Json(request): Json<RelayMobilePairingClaimRequest>,
) -> Response {
    let pairing_token = request.pairing_token.trim();
    if pairing_token.is_empty() {
        return json_error(StatusCode::UNAUTHORIZED, "invalid_relay_pairing_token");
    }

    let now = Utc::now();
    let pairing_hash = relay_token_hash(pairing_token);
    let Some(pairing) = ({
        let mut inner = state.inner.lock();
        inner.prune_mobile_pairing_tokens(now);
        inner.mobile_pairing_tokens.remove(&pairing_hash)
    }) else {
        return json_error(StatusCode::UNAUTHORIZED, "invalid_relay_pairing_token");
    };

    let expires_at = parse_utc_datetime(&pairing.expires_at);
    if expires_at.map(|value| value <= now).unwrap_or(true) {
        return json_error(StatusCode::UNAUTHORIZED, "expired_relay_pairing_token");
    }

    let mobile_device_id = request
        .device_id
        .trim()
        .to_string()
        .is_empty()
        .then(|| format!("mdev_{}", Uuid::new_v4().simple()))
        .unwrap_or_else(|| request.device_id.trim().to_string());
    let mobile_device_name = request
        .device_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("iPhone")
        .to_string();
    let client_kind = request
        .client_kind
        .as_deref()
        .or(request.platform.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("ios")
        .to_string();
    let scopes = relay_mobile_scopes();
    let device_token = generate_relay_token("rmt");
    let device_token_hash = relay_token_hash(&device_token);
    let credential_expires_at =
        (now + ChronoDuration::seconds(RELAY_MOBILE_DEVICE_TOKEN_TTL_SECS)).to_rfc3339();
    {
        let mut inner = state.inner.lock();
        inner.mobile_credentials.insert(
            device_token_hash,
            RelayMobileCredential {
                mobile_device_id: mobile_device_id.clone(),
                mobile_device_name,
                client_kind,
                relay_device_id: pairing.relay_device_id.clone(),
                scopes: scopes.clone(),
                created_at: now.to_rfc3339(),
                expires_at: credential_expires_at.clone(),
                revoked_at: None,
            },
        );
        inner.push_audit(
            "mobile_pairing_claimed",
            Some(&pairing.relay_device_id),
            None,
            json!({
                "mobile_device_id": mobile_device_id,
                "pairing_created_at": pairing.created_at,
                "scopes": scopes,
            }),
        );
    }

    Json(json!({
        "ok": true,
        "device_id": mobile_device_id,
        "device_token": device_token,
        "relay_device_id": pairing.relay_device_id,
        "scopes": scopes,
        "expires_at": credential_expires_at,
    }))
    .into_response()
}

async fn handle_revoke_mobile_credential(
    State(state): State<RelayServerState>,
    headers: HeaderMap,
    Json(request): Json<RelayMobileCredentialRevokeRequest>,
) -> Response {
    if let Err(response) = authorize_http(&state, &headers) {
        return response;
    }

    let device_token = request
        .device_token
        .as_deref()
        .or(request.mobile_token.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let mobile_device_id = request
        .device_id
        .as_deref()
        .or(request.mobile_device_id.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let relay_device_id = request
        .relay_device_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if device_token.is_none() && mobile_device_id.is_none() {
        return json_error(StatusCode::BAD_REQUEST, "missing_revoke_target");
    }

    let revoked_at = Utc::now().to_rfc3339();
    let mut matched = 0usize;
    let mut revoked = 0usize;
    let mut closed_streams = 0usize;
    let mut audit_events = Vec::new();

    let mut inner = state.inner.lock();
    if let Some(token) = device_token {
        let hash = relay_token_hash(token);
        if let Some(credential) = inner.mobile_credentials.get_mut(&hash) {
            if relay_device_id
                .map(|value| value == credential.relay_device_id)
                .unwrap_or(true)
            {
                matched += 1;
                if credential.revoked_at.is_none() {
                    credential.revoked_at = Some(revoked_at.clone());
                    revoked += 1;
                    audit_events.push((
                        hash.clone(),
                        credential.relay_device_id.clone(),
                        credential.mobile_device_id.clone(),
                        credential.client_kind.clone(),
                    ));
                }
            }
        }
    } else if let Some(device_id) = mobile_device_id {
        for (hash, credential) in inner.mobile_credentials.iter_mut() {
            if credential.mobile_device_id != device_id {
                continue;
            }
            if relay_device_id
                .map(|value| value != credential.relay_device_id)
                .unwrap_or(false)
            {
                continue;
            }
            matched += 1;
            if credential.revoked_at.is_none() {
                credential.revoked_at = Some(revoked_at.clone());
                revoked += 1;
                audit_events.push((
                    hash.clone(),
                    credential.relay_device_id.clone(),
                    credential.mobile_device_id.clone(),
                    credential.client_kind.clone(),
                ));
            }
        }
    }

    if matched == 0 {
        return json_error(StatusCode::NOT_FOUND, "mobile_credential_not_found");
    }

    for (credential_hash, relay_device_id, mobile_device_id, client_kind) in audit_events {
        let credential_closed_streams = inner.revoke_mobile_streams(&credential_hash);
        closed_streams += credential_closed_streams;
        inner.push_audit(
            "mobile_credential_revoked",
            Some(&relay_device_id),
            None,
            json!({
                "mobile_device_id": mobile_device_id,
                "client_kind": client_kind,
                "revoked_at": revoked_at,
                "closed_streams": credential_closed_streams,
            }),
        );
    }

    Json(json!({
        "ok": true,
        "revoked": revoked,
        "closed_streams": closed_streams,
        "revoked_at": revoked_at,
        "device_id": mobile_device_id,
        "relay_device_id": relay_device_id,
    }))
    .into_response()
}

fn relay_error_message(error: &str) -> String {
    json!({
        "message_type": "relay_error",
        "payload": {
            "error": error,
        }
    })
    .to_string()
}

async fn handle_command(
    State(state): State<RelayServerState>,
    headers: HeaderMap,
    Path(command_id): Path<String>,
) -> Response {
    let inner = state.inner.lock();
    let Some(command) = inner.commands.get(&command_id).cloned() else {
        return json_error(StatusCode::NOT_FOUND, "command_not_found");
    };
    drop(inner);

    if let Err(response) =
        authorize_device_http(&state, &headers, &command.device_id, &command.scope)
    {
        return response;
    }
    Json(json!({ "command": command })).into_response()
}

async fn handle_audit(State(state): State<RelayServerState>, headers: HeaderMap) -> Response {
    if let Err(response) = authorize_http(&state, &headers) {
        return response;
    }
    let inner = state.inner.lock();
    let events: Vec<_> = inner.audit.iter().cloned().collect();
    Json(json!({ "events": events })).into_response()
}

async fn handle_create_command(
    State(state): State<RelayServerState>,
    headers: HeaderMap,
    Path(device_id): Path<String>,
    Json(request): Json<CreateRelayCommandRequest>,
) -> Response {
    let Some(spec) = command_spec(&request.command_type) else {
        return json_error(StatusCode::BAD_REQUEST, "unsupported_command");
    };
    if let Err(response) = authorize_device_http(&state, &headers, &device_id, spec.scope) {
        return response;
    }

    let now = Utc::now();
    let mut inner = state.inner.lock();
    let cooldown_key = format!("{}:{}", device_id, request.command_type);
    if spec.cooldown_secs > 0 {
        if let Some(until) = inner
            .cooldown_until
            .get(&cooldown_key)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        {
            if until.with_timezone(&Utc) > now {
                inner.push_audit(
                    "command_cooldown_blocked",
                    Some(&device_id),
                    None,
                    json!({
                        "type": request.command_type,
                        "cooldown_until": until.to_rfc3339(),
                    }),
                );
                return (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({
                        "error": "cooldown_active",
                        "cooldown_until": until.to_rfc3339(),
                    })),
                )
                    .into_response();
            }
        }
        inner.cooldown_until.insert(
            cooldown_key,
            (now + ChronoDuration::seconds(spec.cooldown_secs)).to_rfc3339(),
        );
    }

    let mut command = RelayCommand {
        command_id: format!("cmd_{}", Uuid::new_v4()),
        device_id: device_id.clone(),
        command_type: request.command_type,
        scope: spec.scope.to_string(),
        status: "pending".to_string(),
        nonce: Uuid::new_v4().to_string(),
        created_at: now.to_rfc3339(),
        expires_at: (now + ChronoDuration::seconds(DEFAULT_COMMAND_TTL_SECS)).to_rfc3339(),
        delivered_at: None,
        started_at: None,
        finished_at: None,
        result: None,
        payload: request.payload,
    };

    let delivered = if let Some(entry) = inner.mac_senders.get(&device_id) {
        if entry
            .sender
            .send(ServerToMacMessage {
                kind: "command",
                command: Some(command.clone()),
                message_id: None,
                message: None,
            })
            .is_ok()
        {
            command.status = "delivered".to_string();
            command.delivered_at = Some(Utc::now().to_rfc3339());
            true
        } else {
            false
        }
    } else {
        false
    };

    inner.push_audit(
        "command_created",
        Some(&device_id),
        Some(&command.command_id),
        json!({
            "type": command.command_type,
            "delivered": delivered,
        }),
    );
    inner.insert_command(command.clone());

    (StatusCode::ACCEPTED, Json(json!({ "command": command }))).into_response()
}

async fn handle_mac_ws(
    State(state): State<RelayServerState>,
    headers: HeaderMap,
    Query(query): Query<MacSocketQuery>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(device_id) = query.device_id.filter(|value| !value.trim().is_empty()) else {
        return json_error(StatusCode::BAD_REQUEST, "missing_device_id");
    };
    if let Err(response) = authorize_ws(&state, &headers, query.token.as_deref()) {
        return response;
    }
    ws.on_upgrade(move |socket| handle_mac_socket(state, device_id, socket))
}

async fn handle_mac_socket(state: RelayServerState, device_id: String, socket: WebSocket) {
    let connection_id = format!("conn_{}", Uuid::new_v4());
    let (mut ws_sender, mut ws_receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<ServerToMacMessage>();
    let pending = {
        let mut inner = state.inner.lock();
        inner.register_mac_sender(device_id.clone(), connection_id.clone(), tx.clone());
        let device = inner
            .devices
            .entry(device_id.clone())
            .or_insert(RelayDevice {
                device_id: device_id.clone(),
                workspace_id: None,
                app_version: None,
                status: "online".to_string(),
                last_seen_at: Some(Utc::now().to_rfc3339()),
            });
        device.status = "online".to_string();
        device.last_seen_at = Some(Utc::now().to_rfc3339());
        inner.push_audit(
            "mac_connected",
            Some(&device_id),
            None,
            json!({
                "connection_id": connection_id.clone(),
            }),
        );

        let now = Utc::now();
        let mut pending = Vec::new();
        for command in inner.commands.values_mut() {
            if command.device_id != device_id || command.status != "pending" {
                continue;
            }
            let expired = chrono::DateTime::parse_from_rfc3339(&command.expires_at)
                .map(|expires_at| expires_at.with_timezone(&Utc) <= now)
                .unwrap_or(true);
            if expired {
                command.status = "expired".to_string();
                command.finished_at = Some(Utc::now().to_rfc3339());
                continue;
            }
            command.status = "delivered".to_string();
            command.delivered_at = Some(Utc::now().to_rfc3339());
            pending.push(command.clone());
        }
        pending
    };

    for command in pending {
        let _ = tx.send(ServerToMacMessage {
            kind: "command",
            command: Some(command),
            message_id: None,
            message: None,
        });
    }

    let send_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            let Ok(payload) = serde_json::to_string(&message) else {
                continue;
            };
            if ws_sender.send(AxumMessage::Text(payload)).await.is_err() {
                break;
            }
        }
    });

    while let Some(message) = ws_receiver.next().await {
        match message {
            Ok(AxumMessage::Text(text)) => handle_mac_text(&state, &device_id, &text),
            Ok(AxumMessage::Close(_)) => break,
            Ok(_) => {}
            Err(_) => break,
        }
    }

    send_task.abort();
    let mut inner = state.inner.lock();
    let removed_current_sender = inner.remove_mac_sender_if_current(&device_id, &connection_id);
    if removed_current_sender {
        if let Some(device) = inner.devices.get_mut(&device_id) {
            device.status = "offline".to_string();
            device.last_seen_at = Some(Utc::now().to_rfc3339());
        }
    }
    inner.push_audit(
        "mac_disconnected",
        Some(&device_id),
        None,
        json!({
            "connection_id": connection_id,
            "removed_current_sender": removed_current_sender,
        }),
    );
}

fn handle_mac_text(state: &RelayServerState, device_id: &str, text: &str) {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return;
    };
    let Some(kind) = value.get("kind").and_then(Value::as_str) else {
        return;
    };

    let mut inner = state.inner.lock();
    match kind {
        "hello" => {
            let device = inner
                .devices
                .entry(device_id.to_string())
                .or_insert(RelayDevice {
                    device_id: device_id.to_string(),
                    workspace_id: None,
                    app_version: None,
                    status: "online".to_string(),
                    last_seen_at: None,
                });
            device.workspace_id = value
                .get("workspace_id")
                .and_then(Value::as_str)
                .map(str::to_string);
            device.app_version = value
                .get("app_version")
                .and_then(Value::as_str)
                .map(str::to_string);
            device.status = "online".to_string();
            device.last_seen_at = Some(Utc::now().to_rfc3339());
            let workspace_id = device.workspace_id.clone();
            let app_version = device.app_version.clone();
            inner.push_audit(
                "mac_hello",
                Some(device_id),
                None,
                json!({
                    "workspace_id": workspace_id,
                    "app_version": app_version,
                }),
            );
        }
        "heartbeat" => {
            let status = value.get("status").cloned().unwrap_or_else(|| json!({}));
            inner.statuses.insert(device_id.to_string(), status);
            let device = inner
                .devices
                .entry(device_id.to_string())
                .or_insert(RelayDevice {
                    device_id: device_id.to_string(),
                    workspace_id: None,
                    app_version: None,
                    status: "online".to_string(),
                    last_seen_at: None,
                });
            device.status = "online".to_string();
            device.last_seen_at = Some(Utc::now().to_rfc3339());
        }
        "bridge_message" => {
            let Some(message) = value.get("message") else {
                return;
            };
            let Ok(text) = serde_json::to_string(message) else {
                return;
            };
            let sender = inner.stream_sender(device_id);
            let subscriber_count = sender.send(text).unwrap_or(0);
            inner.push_audit(
                "mac_bridge_message_broadcast",
                Some(device_id),
                None,
                json!({
                    "subscribers": subscriber_count,
                }),
            );
            let device = inner
                .devices
                .entry(device_id.to_string())
                .or_insert(RelayDevice {
                    device_id: device_id.to_string(),
                    workspace_id: None,
                    app_version: None,
                    status: "online".to_string(),
                    last_seen_at: None,
                });
            device.status = "online".to_string();
            device.last_seen_at = Some(Utc::now().to_rfc3339());
        }
        "command_result" => {
            let Some(command_id) = value.get("command_id").and_then(Value::as_str) else {
                return;
            };
            let status = value
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("failed")
                .to_string();
            let result = value.get("result").cloned().unwrap_or_else(|| json!({}));
            let started_at = value
                .get("started_at")
                .and_then(Value::as_str)
                .map(str::to_string);
            let finished_at = value
                .get("finished_at")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| Utc::now().to_rfc3339());
            if let Some(command_device_id) = inner
                .commands
                .get(command_id)
                .map(|command| command.device_id.clone())
            {
                if command_device_id != device_id {
                    inner.push_audit(
                        "command_result_rejected",
                        Some(device_id),
                        Some(command_id),
                        json!({
                            "reason": "device_mismatch",
                            "command_device_id": command_device_id,
                            "sender_device_id": device_id,
                        }),
                    );
                    return;
                }
            }
            if let Some(command) = inner.commands.get_mut(command_id) {
                command.status = status.clone();
                if let Some(started_at) = started_at {
                    command.started_at = Some(started_at);
                }
                command.finished_at = Some(finished_at);
                command.result = Some(result.clone());
            }
            inner.push_audit(
                "command_result",
                Some(device_id),
                Some(command_id),
                json!({
                    "status": status,
                    "result": result,
                }),
            );
        }
        _ => {}
    }
}

async fn handle_relay_client_text<S>(
    config: &RelayMacClientConfig,
    writer: &mut S,
    text: &str,
    replay_guard: &mut RelayClientReplayGuard,
    local_bridge_tx: &mpsc::UnboundedSender<Value>,
) -> Result<()>
where
    S: SinkExt<TungsteniteMessage> + Unpin,
    <S as futures_util::Sink<TungsteniteMessage>>::Error: std::error::Error + Send + Sync + 'static,
{
    let value: Value = serde_json::from_str(text)?;
    let kind = value
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if kind == "bridge_client_message" {
        let message = value
            .get("message")
            .cloned()
            .ok_or_else(|| anyhow!("missing bridge client message"))?;
        local_bridge_tx
            .send(message)
            .map_err(|_| anyhow!("local bridge sender unavailable"))?;
        return Ok(());
    }
    if kind != "command" {
        return Ok(());
    }
    let command_value = value
        .get("command")
        .cloned()
        .ok_or_else(|| anyhow!("missing command"))?;
    let command: RelayCommand = serde_json::from_value(command_value)?;

    let started_at = Utc::now().to_rfc3339();
    let result = execute_relay_command(config, &command, replay_guard).await;
    let finished_at = Utc::now().to_rfc3339();
    let (status, result) = match result {
        Ok(value) => ("succeeded", value),
        Err(error) => ("failed", json!({ "error": error.to_string() })),
    };

    send_tungstenite_json(
        writer,
        &json!({
            "kind": "command_result",
            "command_id": command.command_id,
            "status": status,
            "started_at": started_at,
            "finished_at": finished_at,
            "result": result,
        }),
    )
    .await?;
    Ok(())
}

async fn execute_relay_command(
    config: &RelayMacClientConfig,
    command: &RelayCommand,
    replay_guard: &mut RelayClientReplayGuard,
) -> Result<Value> {
    let expires_at = validate_relay_command_for_execution(command, &config.device_id)?;
    replay_guard.register(command, expires_at)?;

    match command.command_type.as_str() {
        "get_status" => {
            let status = fetch_connection_status(&config.local_base_url).await?;
            Ok(json!({
                "summary": summarize_connection_status(&status),
            }))
        }
        "collect_diagnostics" => {
            let status = fetch_connection_status(&config.local_base_url).await?;
            Ok(json!({
                "summary": summarize_connection_status(&status),
                "root_tunnel": status.get("root_tunnel").cloned().unwrap_or(Value::Null),
                "public_tunnel": status.get("public_tunnel").cloned().unwrap_or(Value::Null),
                "local_origin": status.get("local_origin").cloned().unwrap_or(Value::Null),
            }))
        }
        "get_sessions" => fetch_active_sessions(&config.local_base_url).await,
        "mcp_action" => post_local_bridge_publish(config, &command.payload).await,
        "recover_bridge_origin" => {
            if !config.allow_recover {
                let status = fetch_connection_status(&config.local_base_url).await.ok();
                return Ok(json!({
                    "simulated": true,
                    "note": "Set --allow-recover to invoke recover_bridge_origin.",
                    "summary": status.as_ref().map(summarize_connection_status),
                }));
            }
            let response = crate::app::setup::recover_bridge_origin().await;
            Ok(serde_json::to_value(response)?)
        }
        "recover_public_tunnel" => {
            if !config.allow_recover {
                let status = fetch_connection_status(&config.local_base_url).await.ok();
                return Ok(json!({
                    "simulated": true,
                    "note": "Set --allow-recover to call local /api/restart-tunnel.",
                    "summary": status.as_ref().map(summarize_connection_status),
                }));
            }
            post_local_recovery_endpoint(config, "/api/restart-tunnel", "relay").await
        }
        "recover_tailscale_funnel" => {
            if !config.allow_recover {
                let status = fetch_connection_status(&config.local_base_url).await.ok();
                return Ok(json!({
                    "simulated": true,
                    "note": "Set --allow-recover to call local /api/recover-tailscale-funnel.",
                    "summary": status.as_ref().map(summarize_connection_status),
                }));
            }
            post_local_recovery_endpoint(config, "/api/recover-tailscale-funnel", "relay").await
        }
        "recover_public_transport_auto" => {
            if !config.allow_recover {
                let status = fetch_connection_status(&config.local_base_url).await.ok();
                let pairing = fetch_mobile_pairing_status(&config.local_base_url)
                    .await
                    .ok();
                return Ok(json!({
                    "simulated": true,
                    "note": "Set --allow-recover to recover the active public transport.",
                    "summary": status.as_ref().map(summarize_connection_status),
                    "pairing": pairing.as_ref().map(pairing_primary_summary),
                }));
            }
            let pairing = fetch_mobile_pairing_status(&config.local_base_url).await?;
            let primary = pairing_primary_summary(&pairing);
            let selected_transport = if pairing_primary_is_tailscale_funnel(&pairing) {
                "tailscale_funnel"
            } else {
                "root_tunnel"
            };
            let endpoint = if selected_transport == "tailscale_funnel" {
                "/api/recover-tailscale-funnel"
            } else {
                "/api/restart-tunnel"
            };
            let recovery = post_local_recovery_endpoint(config, endpoint, "relay_auto").await?;
            Ok(json!({
                "selected_transport": selected_transport,
                "endpoint": endpoint,
                "primary_before": primary,
                "recovery": recovery,
            }))
        }
        _ => Err(anyhow!("unsupported command: {}", command.command_type)),
    }
}

async fn post_local_recovery_endpoint(
    config: &RelayMacClientConfig,
    path: &str,
    recovery_transport: &str,
) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .no_proxy()
        .build()?;
    let url = format!("{}{}", config.local_base_url.trim_end_matches('/'), path);
    let request = client
        .post(&url)
        .header("x-iterate-recovery-transport", recovery_transport);
    let request = crate::bridge::auth::authorize_internal_bridge_request(request, "POST", &url)
        .map_err(anyhow::Error::msg)?;
    let response = request.send().await?;
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or_else(|error| {
        json!({
            "error": format!("failed to parse recovery response: {error}")
        })
    });
    Ok(json!({
        "http_status": status.as_u16(),
        "body": body,
    }))
}

async fn fetch_mobile_pairing_status(local_base_url: &str) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()?;
    let url = format!(
        "{}/api/mobile/pairing/status",
        local_base_url.trim_end_matches('/')
    );
    let request =
        crate::bridge::auth::authorize_internal_bridge_request(client.get(&url), "GET", &url)
            .map_err(anyhow::Error::msg)?;
    let response = request.send().await?;
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or_else(|error| {
        json!({
            "error": format!("failed to parse pairing status response: {error}")
        })
    });
    if !status.is_success() {
        return Err(anyhow!(
            "pairing status request failed: http_status={} body={}",
            status.as_u16(),
            body
        ));
    }
    Ok(body)
}

async fn fetch_active_sessions(local_base_url: &str) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()?;
    let url = format!(
        "{}/api/active-sessions",
        local_base_url.trim_end_matches('/')
    );
    let request =
        crate::bridge::auth::authorize_internal_bridge_request(client.get(&url), "GET", &url)
            .map_err(anyhow::Error::msg)?;
    let response = request.send().await?;
    let status = response.status();
    let body = response.json::<Value>().await.unwrap_or_else(|error| {
        json!({
            "error": format!("failed to parse active sessions response: {error}")
        })
    });
    if !status.is_success() {
        return Err(anyhow!(
            "active sessions request failed: http_status={} body={}",
            status.as_u16(),
            body
        ));
    }
    Ok(body)
}

fn pairing_primary_summary(pairing: &Value) -> Value {
    json!({
        "transport_mode": pairing.get("transport_mode").cloned().unwrap_or(Value::Null),
        "base_url": pairing.get("base_url").cloned().unwrap_or(Value::Null),
        "ws_url": pairing.get("ws_url").cloned().unwrap_or(Value::Null),
    })
}

fn pairing_primary_is_tailscale_funnel(pairing: &Value) -> bool {
    pairing
        .get("base_url")
        .and_then(Value::as_str)
        .and_then(https_host_from_base_url)
        .map(|host| host.ends_with(".ts.net"))
        .unwrap_or(false)
}

fn https_host_from_base_url(value: &str) -> Option<String> {
    let rest = value.trim().strip_prefix("https://")?;
    let host = rest.split('/').next()?.trim().trim_end_matches('.');
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn validate_relay_command_for_execution(
    command: &RelayCommand,
    expected_device_id: &str,
) -> Result<DateTime<Utc>> {
    if command.device_id != expected_device_id {
        return Err(anyhow!(
            "command device mismatch: expected={} got={}",
            expected_device_id,
            command.device_id
        ));
    }
    let Some(spec) = command_spec(&command.command_type) else {
        return Err(anyhow!("unsupported command: {}", command.command_type));
    };
    if command.scope != spec.scope {
        return Err(anyhow!(
            "command scope mismatch: type={} expected={} got={}",
            command.command_type,
            spec.scope,
            command.scope
        ));
    }
    let expires_at = chrono::DateTime::parse_from_rfc3339(&command.expires_at)
        .map_err(|_| anyhow!("invalid command expires_at"))?
        .with_timezone(&Utc);
    if expires_at <= Utc::now() {
        return Err(anyhow!("command expired"));
    }
    Ok(expires_at)
}

async fn relay_status_snapshot(
    local_base_url: &str,
    local_bridge_health: &Arc<Mutex<LocalBridgeHealth>>,
) -> Value {
    let connection = relay_status_summary(local_base_url).await;
    let active_sessions = fetch_active_sessions(local_base_url)
        .await
        .unwrap_or_else(|error| {
            json!({
                "sessions": [],
                "error": error.to_string(),
            })
        });

    json!({
        "connection": connection,
        "local_bridge_stream": local_bridge_health.lock().clone(),
        "active_sessions": active_sessions,
    })
}

async fn relay_status_summary(local_base_url: &str) -> Value {
    match fetch_connection_status(local_base_url).await {
        Ok(status) => summarize_connection_status(&status),
        Err(error) => json!({
            "local_origin": "unreachable",
            "public_tunnel": "unknown",
            "root_tunnel_health_class": "unknown",
            "last_error": error.to_string(),
        }),
    }
}

async fn fetch_connection_status(local_base_url: &str) -> Result<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()?;
    let url = format!(
        "{}/api/connection-status",
        local_base_url.trim_end_matches('/')
    );
    let request =
        crate::bridge::auth::authorize_internal_bridge_request(client.get(&url), "GET", &url)
            .map_err(anyhow::Error::msg)?;
    let response = request.send().await?;
    if !response.status().is_success() {
        return Err(anyhow!("connection-status http {}", response.status()));
    }
    Ok(response.json::<Value>().await?)
}

fn summarize_connection_status(status: &Value) -> Value {
    let local_origin_ok = status
        .pointer("/local_origin/healthy")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let public_tunnel_ok = status
        .pointer("/public_tunnel/healthy")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let root_tunnel_health_class = status
        .pointer("/root_tunnel/derived/tunnel_health_class")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let ha_count = status
        .pointer("/root_tunnel/metrics/effective_ha_connection_count")
        .and_then(Value::as_i64)
        .or_else(|| {
            status
                .pointer("/root_tunnel/status/ha_connection_count")
                .and_then(Value::as_i64)
        });
    let edge_7844_suspected = status
        .pointer("/root_tunnel/derived/edge_7844_suspected")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let backoff_remaining_secs = status
        .pointer("/root_tunnel/derived/backoff_remaining_secs")
        .and_then(Value::as_i64)
        .unwrap_or(0);

    json!({
        "local_origin": if local_origin_ok { "healthy" } else { "failed" },
        "public_tunnel": if public_tunnel_ok { "healthy" } else { "failed" },
        "root_tunnel_health_class": root_tunnel_health_class,
        "ha_count": ha_count,
        "edge_7844_suspected": edge_7844_suspected,
        "backoff_remaining_secs": backoff_remaining_secs,
        "diagnosis": status.pointer("/diagnosis/code").cloned().unwrap_or(Value::Null),
    })
}

fn relay_active_sessions_from_status(status: &Value) -> Vec<Value> {
    status
        .pointer("/active_sessions/sessions")
        .and_then(Value::as_array)
        .cloned()
        .or_else(|| status.get("sessions").and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

fn relay_bridge_stream_message_from_text(text: &str) -> Result<Value> {
    if text.len() > MAX_RELAY_STREAM_MESSAGE_BYTES {
        return Err(anyhow!("relay stream message too large"));
    }
    let message: Value = serde_json::from_str(text)?;
    let message_type = message
        .get("message_type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("relay stream message missing message_type"))?;
    if message_type == "relay_client_message" {
        return Err(anyhow!("unsupported relay stream message_type"));
    }
    Ok(message)
}

fn relay_mobile_bridge_message_from_text(text: &str) -> Result<Value> {
    if text.len() > MAX_RELAY_STREAM_MESSAGE_BYTES {
        return Err(anyhow!("relay mobile message too large"));
    }
    let message: Value = serde_json::from_str(text)?;
    let message_type = message
        .get("message_type")
        .and_then(Value::as_str)
        .unwrap_or_default();

    match message_type {
        "request_sync" | "request_timeline_sync" | "request_main_page" => {
            ensure_optional_object_payload(&message)?;
            Ok(message)
        }
        "phone_action_result" => {
            ensure_string_payload_field(&message, "id")?;
            ensure_string_payload_field(&message, "status")?;
            Ok(message)
        }
        "system_command" => {
            let command = message
                .pointer("/payload/command")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !matches!(command, "toggle_prevent_sleep" | "show_main_window") {
                return Err(anyhow!("unsupported relay system_command: {}", command));
            }
            Ok(message)
        }
        "mcp_action" => relay_bridge_message_from_payload(&message),
        _ => Err(anyhow!(
            "unsupported relay mobile message_type: {}",
            message_type
        )),
    }
}

fn ensure_optional_object_payload(message: &Value) -> Result<()> {
    match message.get("payload") {
        Some(Value::Object(_)) | None => Ok(()),
        _ => Err(anyhow!("relay mobile message payload must be an object")),
    }
}

fn ensure_string_payload_field(message: &Value, field: &str) -> Result<()> {
    message
        .get("payload")
        .and_then(|payload| payload.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|_| ())
        .ok_or_else(|| anyhow!("relay mobile message missing payload.{field}"))
}

fn relay_bridge_message_from_payload(payload: &Value) -> Result<Value> {
    let message = if payload.get("message_type").is_some() {
        payload.clone()
    } else {
        json!({
            "message_type": "mcp_action",
            "payload": payload,
        })
    };

    let message_type = message
        .get("message_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if message_type != "mcp_action" {
        return Err(anyhow!(
            "unsupported relay bridge message_type: {}",
            message_type
        ));
    }

    let action_payload = message
        .get("payload")
        .ok_or_else(|| anyhow!("relay mcp_action missing payload"))?;
    let action = action_payload
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(
        action,
        "submit"
            | "continue"
            | "goal"
            | "goal_start"
            | "loop"
            | "loop_start"
            | "enhance"
            | "cancel"
            | "update_window_conditional_state"
            | "update_window_conditional_active"
    ) {
        return Err(anyhow!("unsupported relay mcp_action: {}", action));
    }
    if action_payload
        .get("project_path")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        return Err(anyhow!("relay mcp_action missing project_path"));
    }

    Ok(message)
}

async fn post_local_bridge_publish(
    config: &RelayMacClientConfig,
    payload: &Value,
) -> Result<Value> {
    let message = relay_bridge_message_from_payload(payload)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy()
        .build()?;
    let url = format!(
        "{}/bridge/publish",
        config.local_base_url.trim_end_matches('/')
    );
    let request = crate::bridge::auth::authorize_internal_bridge_request(
        client.post(&url).json(&message),
        "POST",
        &url,
    )
    .map_err(anyhow::Error::msg)?;
    let response = request.send().await?;
    let status = response.status();
    let bytes = response.bytes().await?;
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|error| {
            json!({
                "error": format!("failed to parse bridge publish response: {error}")
            })
        })
    };
    if !status.is_success() {
        return Err(anyhow!(
            "bridge publish request failed: http_status={} body={}",
            status.as_u16(),
            body
        ));
    }
    Ok(json!({
        "http_status": status.as_u16(),
        "body": body,
    }))
}

async fn send_tungstenite_json<S>(writer: &mut S, value: &Value) -> Result<()>
where
    S: SinkExt<TungsteniteMessage> + Unpin,
    <S as futures_util::Sink<TungsteniteMessage>>::Error: std::error::Error + Send + Sync + 'static,
{
    writer
        .send(TungsteniteMessage::Text(serde_json::to_string(value)?))
        .await?;
    Ok(())
}

fn command_spec(command_type: &str) -> Option<CommandSpec> {
    match command_type {
        "get_status" => Some(CommandSpec {
            scope: "status.read",
            cooldown_secs: 0,
        }),
        "collect_diagnostics" => Some(CommandSpec {
            scope: "status.read",
            cooldown_secs: 30,
        }),
        "get_sessions" => Some(CommandSpec {
            scope: "session.read",
            cooldown_secs: 0,
        }),
        "mcp_action" => Some(CommandSpec {
            scope: "session.respond",
            cooldown_secs: 0,
        }),
        "recover_bridge_origin" => Some(CommandSpec {
            scope: "bridge.recover",
            cooldown_secs: 60,
        }),
        "recover_public_tunnel" => Some(CommandSpec {
            scope: "tunnel.recover",
            cooldown_secs: 300,
        }),
        "recover_tailscale_funnel" => Some(CommandSpec {
            scope: "tunnel.recover",
            cooldown_secs: 300,
        }),
        "recover_public_transport_auto" => Some(CommandSpec {
            scope: "tunnel.recover",
            cooldown_secs: 300,
        }),
        _ => None,
    }
}

fn authorize_http(state: &RelayServerState, headers: &HeaderMap) -> Result<(), Response> {
    if admin_token_authorized(state, bearer_token(headers), None) {
        return Ok(());
    }
    Err(json_error(StatusCode::UNAUTHORIZED, "invalid_relay_token"))
}

fn authorize_ws(
    state: &RelayServerState,
    headers: &HeaderMap,
    query_token: Option<&str>,
) -> Result<(), Response> {
    if admin_token_authorized(state, bearer_token(headers), query_token) {
        return Ok(());
    }
    Err(json_error(StatusCode::UNAUTHORIZED, "invalid_relay_token"))
}

fn authorize_device_http(
    state: &RelayServerState,
    headers: &HeaderMap,
    relay_device_id: &str,
    required_scope: &str,
) -> Result<DeviceAuthorization, Response> {
    authorize_device_token(
        state,
        bearer_token(headers),
        None,
        relay_device_id,
        required_scope,
    )
}

fn authorize_device_ws(
    state: &RelayServerState,
    headers: &HeaderMap,
    query_token: Option<&str>,
    relay_device_id: &str,
    required_scope: &str,
) -> Result<DeviceAuthorization, Response> {
    authorize_device_token(
        state,
        bearer_token(headers),
        query_token,
        relay_device_id,
        required_scope,
    )
}

fn authorize_device_token(
    state: &RelayServerState,
    header_token: Option<&str>,
    query_token: Option<&str>,
    relay_device_id: &str,
    required_scope: &str,
) -> Result<DeviceAuthorization, Response> {
    if admin_token_authorized(state, header_token, query_token) {
        return Ok(DeviceAuthorization::default());
    }

    let Some(token) = header_token.or(query_token) else {
        return Err(json_error(StatusCode::UNAUTHORIZED, "invalid_relay_token"));
    };
    let (credential_hash, credential) = mobile_credential_for_token(state, token)
        .map_err(|error| json_error(StatusCode::UNAUTHORIZED, error))?;
    if credential.relay_device_id != relay_device_id {
        return Err(json_error(StatusCode::FORBIDDEN, "relay_device_forbidden"));
    }
    if !credential
        .scopes
        .iter()
        .any(|scope| scope == required_scope)
    {
        return Err(json_error(StatusCode::FORBIDDEN, "relay_scope_forbidden"));
    }
    Ok(DeviceAuthorization {
        mobile_credential_hash: Some(credential_hash),
    })
}

fn admin_token_authorized(
    state: &RelayServerState,
    header_token: Option<&str>,
    query_token: Option<&str>,
) -> bool {
    let Some(expected) = state.token.as_deref() else {
        return true;
    };
    header_token == Some(expected) || query_token == Some(expected)
}

fn mobile_credential_for_token(
    state: &RelayServerState,
    token: &str,
) -> Result<(String, RelayMobileCredential), &'static str> {
    let hash = relay_token_hash(token);
    let now = Utc::now();
    let mut inner = state.inner.lock();
    let Some(credential) = inner.mobile_credentials.get(&hash).cloned() else {
        return Err("invalid_relay_token");
    };
    if credential.revoked_at.is_some() {
        return Err("revoked_device_auth");
    }
    if parse_utc_datetime(&credential.expires_at)
        .map(|expires_at| expires_at <= now)
        .unwrap_or(true)
    {
        inner.mobile_credentials.remove(&hash);
        return Err("expired_device_auth");
    }
    Ok((hash, credential))
}

fn parse_utc_datetime(value: &str) -> Option<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn relay_token_hash(token: &str) -> String {
    let digest = digest::digest(&digest::SHA256, token.as_bytes());
    digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn generate_relay_token(prefix: &str) -> String {
    format!("{}_{}", prefix, Uuid::new_v4().simple())
}

fn relay_mobile_scopes() -> Vec<String> {
    RELAY_MOBILE_SCOPES
        .iter()
        .map(|scope| (*scope).to_string())
        .collect()
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
}

fn json_error(status: StatusCode, error: &str) -> Response {
    (status, Json(json!({ "error": error }))).into_response()
}

fn is_loopback_host(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "::1")
}

fn relay_mac_ws_url(base: &str, device_id: &str) -> String {
    let mut url = normalize_relay_ws_url(base);
    let sep = if url.contains('?') { '&' } else { '?' };
    url.push(sep);
    url.push_str("device_id=");
    url.push_str(&percent_encode(device_id));
    url
}

pub fn relay_mobile_stream_ws_url(base: &str, device_id: &str) -> String {
    let http_base = relay_http_base_from_url(base);
    let mut ws_base = http_base.trim().trim_end_matches('/').to_string();
    if let Some(rest) = ws_base.strip_prefix("http://") {
        ws_base = format!("ws://{rest}");
    } else if let Some(rest) = ws_base.strip_prefix("https://") {
        ws_base = format!("wss://{rest}");
    }
    format!(
        "{}/api/devices/{}/stream",
        ws_base,
        percent_encode(device_id)
    )
}

fn normalize_relay_ws_url(input: &str) -> String {
    let trimmed = input.trim().trim_end_matches('/');
    let mut url = if trimmed.contains("/mac/ws") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/mac/ws")
    };
    if let Some(rest) = url.strip_prefix("http://") {
        url = format!("ws://{rest}");
    } else if let Some(rest) = url.strip_prefix("https://") {
        url = format!("wss://{rest}");
    }
    url
}

fn local_bridge_ws_url(base: &str) -> String {
    let trimmed = base.trim().trim_end_matches('/');
    let mut url = if trimmed.ends_with("/ws") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/ws")
    };
    if let Some(rest) = url.strip_prefix("http://") {
        url = format!("ws://{rest}");
    } else if let Some(rest) = url.strip_prefix("https://") {
        url = format!("wss://{rest}");
    }
    url
}

fn percent_encode(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

fn current_workspace_id() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayMacClientSettings {
    pub relay_url: String,
    pub device_id: String,
    pub local_base_url: String,
    pub heartbeat_secs: u64,
    pub allow_recover: bool,
    #[serde(default)]
    pub relay_token: String,
    #[serde(default)]
    pub clear_relay_token: bool,
    pub token_present: bool,
    pub config_path: String,
    pub plist_path: String,
    pub runner_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelayMacClientControlResult {
    pub action: String,
    pub ok: bool,
    pub message: String,
    pub configured: bool,
    pub runner_present: bool,
    pub plist_present: bool,
    pub launchctl_loaded: bool,
    pub process_running: bool,
    pub pid: Option<u32>,
    pub config_path: String,
    pub plist_path: String,
    pub runner_path: String,
    pub stdout: String,
    pub stderr: String,
}

fn relay_support_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("unable to resolve home directory"))?;
    Ok(home
        .join("Library")
        .join("Application Support")
        .join("iterate"))
}

fn relay_mac_client_config_path() -> Result<PathBuf> {
    Ok(relay_support_dir()?.join("relay-mac-client.env"))
}

fn relay_mac_client_runner_path() -> Result<PathBuf> {
    Ok(relay_support_dir()?.join("run-relay-mac-client.sh"))
}

fn relay_mac_client_plist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("unable to resolve home directory"))?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{RELAY_MAC_CLIENT_LABEL}.plist")))
}

fn parse_relay_env_file(path: &PathBuf) -> HashMap<String, String> {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return HashMap::new();
    };
    contents
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                return None;
            }
            let without_export = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            let (key, value) = without_export.split_once('=')?;
            Some((key.trim().to_string(), shell_unquote(value.trim())))
        })
        .collect()
}

fn shell_unquote(value: &str) -> String {
    if value.len() >= 2 {
        let bytes = value.as_bytes();
        if (bytes[0] == b'\'' && bytes[value.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[value.len() - 1] == b'"')
        {
            let inner = &value[1..value.len() - 1];
            if bytes[0] == b'\'' {
                return inner.replace("'\\''", "'");
            }
            return inner.replace("\\\"", "\"");
        }
    }
    shell_unescape_unquoted(value)
}

fn shell_unescape_unquoted(value: &str) -> String {
    let mut output = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                output.push(next);
            } else {
                output.push(ch);
            }
        } else {
            output.push(ch);
        }
    }
    output
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn command_stdout(program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).output()?;
    if !output.status.success() {
        return Err(anyhow!(
            "{} failed: {}",
            program,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn relay_launchctl_domain() -> Result<String> {
    Ok(format!("gui/{}", command_stdout("id", &["-u"])?))
}

fn relay_launchctl_service() -> Result<String> {
    Ok(format!(
        "{}/{}",
        relay_launchctl_domain()?,
        RELAY_MAC_CLIENT_LABEL
    ))
}

fn run_command(program: &str, args: &[String]) -> Result<(bool, String, String)> {
    let output = Command::new(program).args(args).output()?;
    Ok((
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
        String::from_utf8_lossy(&output.stderr).trim().to_string(),
    ))
}

fn relay_mac_client_pid() -> Option<u32> {
    let output = Command::new("pgrep")
        .args(["-f", "--", "--relay-mac-client"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.trim().parse::<u32>().ok())
        .next()
}

fn relay_mac_client_launchctl_loaded() -> bool {
    let Ok(service) = relay_launchctl_service() else {
        return false;
    };
    Command::new("launchctl")
        .args(["print", &service])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn relay_mac_client_status_result(
    action: &str,
    ok: bool,
    message: impl Into<String>,
    stdout: impl Into<String>,
    stderr: impl Into<String>,
) -> Result<RelayMacClientControlResult, String> {
    let config_path =
        relay_mac_client_config_path().map_err(|error| format!("配置路径解析失败: {error}"))?;
    let plist_path =
        relay_mac_client_plist_path().map_err(|error| format!("plist 路径解析失败: {error}"))?;
    let runner_path =
        relay_mac_client_runner_path().map_err(|error| format!("runner 路径解析失败: {error}"))?;
    let values = parse_relay_env_file(&config_path);
    let pid = relay_mac_client_pid();
    Ok(RelayMacClientControlResult {
        action: action.to_string(),
        ok,
        message: message.into(),
        configured: values
            .get("ITERATE_RELAY_URL")
            .is_some_and(|value| !value.trim().is_empty()),
        runner_present: runner_path.is_file(),
        plist_present: plist_path.is_file(),
        launchctl_loaded: relay_mac_client_launchctl_loaded(),
        process_running: pid.is_some(),
        pid,
        config_path: config_path.display().to_string(),
        plist_path: plist_path.display().to_string(),
        runner_path: runner_path.display().to_string(),
        stdout: stdout.into(),
        stderr: stderr.into(),
    })
}

fn write_relay_mac_client_runner() -> Result<()> {
    let config_path = relay_mac_client_config_path()?;
    let runner_path = relay_mac_client_runner_path()?;
    let app_bin = std::env::current_exe()?;
    let parent = runner_path
        .parent()
        .ok_or_else(|| anyhow!("relay runner path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let contents = format!(
        r#"#!/usr/bin/env bash
set -euo pipefail

CONFIG_PATH={config_path}
if [ ! -f "$CONFIG_PATH" ]; then
  echo "relay Mac client config missing: $CONFIG_PATH" >&2
  exit 78
fi

set -a
# shellcheck disable=SC1090
source "$CONFIG_PATH"
set +a

APP_BIN=${{ITERATE_RELAY_APP_BIN:-{app_bin}}}
[ -x "$APP_BIN" ] || {{ echo "iterate app binary missing: $APP_BIN" >&2; exit 78; }}

args=(
  "$APP_BIN"
  --relay-mac-client
  --relay-url "${{ITERATE_RELAY_URL:?ITERATE_RELAY_URL missing}}"
  --device-id "${{ITERATE_RELAY_DEVICE_ID:-local-mac}}"
  --local-base-url "${{ITERATE_RELAY_LOCAL_BASE_URL:-http://127.0.0.1:8080}}"
  --heartbeat-secs "${{ITERATE_RELAY_HEARTBEAT_SECS:-15}}"
)

if [ -n "${{ITERATE_RELAY_TOKEN:-}}" ]; then
  args+=(--relay-token-env ITERATE_RELAY_TOKEN)
fi

case "${{ITERATE_RELAY_ALLOW_RECOVER:-0}}" in
  1|true|TRUE|yes|YES|on|ON) args+=(--allow-recover) ;;
esac

exec "${{args[@]}}"
"#,
        config_path = shell_quote(&config_path.display().to_string()),
        app_bin = shell_quote(&app_bin.display().to_string()),
    );
    std::fs::write(&runner_path, contents)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner_path, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

fn render_relay_mac_client_plist() -> Result<String> {
    let runner_path = relay_mac_client_runner_path()?;
    let support_dir = relay_support_dir()?;
    let log_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("unable to resolve home directory"))?
        .join("Library")
        .join("Logs")
        .join("iterate");
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
      <string>{runner_path}</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>ThrottleInterval</key>
    <integer>10</integer>
    <key>WorkingDirectory</key>
    <string>{working_directory}</string>
    <key>EnvironmentVariables</key>
    <dict>
      <key>HOME</key>
      <string>{home}</string>
    </dict>
    <key>StandardOutPath</key>
    <string>{out_log}</string>
    <key>StandardErrorPath</key>
    <string>{err_log}</string>
  </dict>
</plist>
"#,
        label = RELAY_MAC_CLIENT_LABEL,
        runner_path = xml_escape(&runner_path.display().to_string()),
        working_directory = xml_escape(&support_dir.display().to_string()),
        home = xml_escape(
            &dirs::home_dir()
                .ok_or_else(|| anyhow!("unable to resolve home directory"))?
                .display()
                .to_string()
        ),
        out_log = xml_escape(
            &log_dir
                .join("relay-mac-client.out.log")
                .display()
                .to_string()
        ),
        err_log = xml_escape(
            &log_dir
                .join("relay-mac-client.err.log")
                .display()
                .to_string()
        ),
    ))
}

fn install_relay_mac_client_service_files() -> Result<()> {
    let config_path = relay_mac_client_config_path()?;
    if !config_path.is_file() {
        return Err(anyhow!("Relay 配置不存在，请先保存配置"));
    }
    let values = parse_relay_env_file(&config_path);
    if values
        .get("ITERATE_RELAY_URL")
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err(anyhow!("Relay URL 不能为空"));
    }

    write_relay_mac_client_runner()?;
    let plist_path = relay_mac_client_plist_path()?;
    if let Some(parent) = plist_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&plist_path, render_relay_mac_client_plist()?)?;
    Ok(())
}

fn start_relay_mac_client_service() -> Result<(String, String)> {
    install_relay_mac_client_service_files()?;
    let domain = relay_launchctl_domain()?;
    let service = relay_launchctl_service()?;
    let plist_path = relay_mac_client_plist_path()?;

    let bootstrap_args = vec![
        "bootstrap".to_string(),
        domain,
        plist_path.display().to_string(),
    ];
    let (_, bootstrap_stdout, bootstrap_stderr) = run_command("launchctl", &bootstrap_args)?;
    let kickstart_args = vec!["kickstart".to_string(), "-k".to_string(), service];
    let (kickstart_ok, kickstart_stdout, kickstart_stderr) =
        run_command("launchctl", &kickstart_args)?;
    if !kickstart_ok {
        return Err(anyhow!(
            "launchctl kickstart failed: {}",
            kickstart_stderr.trim()
        ));
    }
    Ok((
        [bootstrap_stdout, kickstart_stdout]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        [bootstrap_stderr, kickstart_stderr]
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
    ))
}

fn stop_relay_mac_client_service() -> Result<(String, String)> {
    let service = relay_launchctl_service()?;
    let domain = relay_launchctl_domain()?;
    let plist_path = relay_mac_client_plist_path()?;
    let service_args = vec!["bootout".to_string(), service];
    let (service_ok, service_stdout, service_stderr) = run_command("launchctl", &service_args)?;
    if service_ok {
        return Ok((service_stdout, service_stderr));
    }

    let domain_args = vec![
        "bootout".to_string(),
        domain,
        plist_path.display().to_string(),
    ];
    let (domain_ok, domain_stdout, domain_stderr) = run_command("launchctl", &domain_args)?;
    if domain_ok {
        Ok((domain_stdout, domain_stderr))
    } else {
        Ok((
            String::new(),
            [service_stderr, domain_stderr]
                .into_iter()
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
                .join("\n"),
        ))
    }
}

fn relay_http_base_from_url(input: &str) -> String {
    let without_query = input.trim().split('?').next().unwrap_or("").trim();
    let mut base = without_query.trim_end_matches('/').to_string();
    if let Some(rest) = base.strip_prefix("ws://") {
        base = format!("http://{rest}");
    } else if let Some(rest) = base.strip_prefix("wss://") {
        base = format!("https://{rest}");
    }
    base.strip_suffix("/mac/ws")
        .map(str::to_string)
        .unwrap_or(base)
}

fn relay_mobile_pairing_config_from_values(
    values: &HashMap<String, String>,
) -> Option<RelayMobilePairingConfig> {
    let relay_url = values
        .get("ITERATE_RELAY_URL")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())?;
    let relay_device_id = values
        .get("ITERATE_RELAY_DEVICE_ID")
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .unwrap_or("local-mac")
        .to_string();
    let base_url = relay_http_base_from_url(relay_url);
    if base_url.is_empty() {
        return None;
    }

    let token_present = values
        .get("ITERATE_RELAY_TOKEN")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    Some(RelayMobilePairingConfig {
        ws_url: relay_mobile_stream_ws_url(&base_url, &relay_device_id),
        base_url,
        relay_device_id,
        relay_pairing_token: None,
        relay_pairing_expires_at: None,
        token_present,
        process_running: false,
        launchctl_loaded: false,
    })
}

pub fn relay_mobile_pairing_config_from_mac_client(
) -> Result<Option<RelayMobilePairingConfig>, String> {
    let config_path =
        relay_mac_client_config_path().map_err(|error| format!("配置路径解析失败: {error}"))?;
    let values = parse_relay_env_file(&config_path);
    let Some(mut config) = relay_mobile_pairing_config_from_values(&values) else {
        return Ok(None);
    };
    config.process_running = relay_mac_client_pid().is_some();
    config.launchctl_loaded = relay_mac_client_launchctl_loaded();
    Ok(Some(config))
}

pub async fn relay_mobile_pairing_config_from_mac_client_for_qr(
) -> Result<Option<RelayMobilePairingConfig>, String> {
    let config_path =
        relay_mac_client_config_path().map_err(|error| format!("配置路径解析失败: {error}"))?;
    let values = parse_relay_env_file(&config_path);
    let Some(mut config) = relay_mobile_pairing_config_from_values(&values) else {
        return Ok(None);
    };
    config.process_running = relay_mac_client_pid().is_some();
    config.launchctl_loaded = relay_mac_client_launchctl_loaded();

    let relay_token = values
        .get("ITERATE_RELAY_TOKEN")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    match request_relay_mobile_pairing_token(
        &config.base_url,
        &config.relay_device_id,
        relay_token.as_deref(),
    )
    .await
    {
        Ok(issue) if issue.ok => {
            config.relay_pairing_token = Some(issue.relay_pairing_token);
            config.relay_pairing_expires_at = Some(issue.expires_at);
            config.relay_device_id = issue.relay_device_id;
        }
        Ok(_) => {
            log::warn!("[Relay] mobile pairing issue returned ok=false");
        }
        Err(error) => {
            log::warn!(
                "[Relay] mobile pairing issue failed base_url={} device_id={} error={}",
                config.base_url,
                config.relay_device_id,
                error
            );
        }
    }

    Ok(Some(config))
}

async fn request_relay_mobile_pairing_token(
    base_url: &str,
    relay_device_id: &str,
    relay_token: Option<&str>,
) -> Result<RelayMobilePairingIssueResponse> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .no_proxy()
        .build()?;
    let url = format!(
        "{}/api/devices/{}/mobile-pairing",
        base_url.trim_end_matches('/'),
        percent_encode(relay_device_id)
    );
    let mut request = client
        .post(url)
        .json(&json!({ "device_name": resolve_relay_mobile_pairing_device_name() }));
    if let Some(token) = relay_token {
        request = request.bearer_auth(token);
    }
    let response = request.send().await?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return Err(anyhow!(
            "relay mobile pairing issue failed: http_status={} body={}",
            status.as_u16(),
            body
        ));
    }
    Ok(serde_json::from_str(&body)?)
}

fn resolve_relay_mobile_pairing_device_name() -> String {
    current_workspace_id()
}

async fn relay_mac_client_health_output() -> Result<String> {
    let config_path = relay_mac_client_config_path()?;
    let values = parse_relay_env_file(&config_path);
    let relay_url = values
        .get("ITERATE_RELAY_URL")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("ITERATE_RELAY_URL missing"))?;
    let device_id = values
        .get("ITERATE_RELAY_DEVICE_ID")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "local-mac".to_string());
    let token = values
        .get("ITERATE_RELAY_TOKEN")
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let http_base = relay_http_base_from_url(&relay_url);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .no_proxy()
        .build()?;

    let mut health_request = client.get(format!("{http_base}/health"));
    if let Some(token) = &token {
        health_request = health_request.bearer_auth(token);
    }
    let health_response = health_request.send().await?;
    let health_status = health_response.status();
    let health_body = health_response.text().await.unwrap_or_default();

    let mut status_request = client.get(format!(
        "{}/api/devices/{}/status",
        http_base,
        percent_encode(&device_id)
    ));
    if let Some(token) = &token {
        status_request = status_request.bearer_auth(token);
    }
    let status_response = status_request.send().await?;
    let device_status = status_response.status();
    let device_body = status_response.text().await.unwrap_or_default();

    let output = format!(
        "health HTTP {}\n{}\n\ndevice HTTP {}\n{}",
        health_status.as_u16(),
        health_body,
        device_status.as_u16(),
        device_body
    );
    if !health_status.is_success() || !device_status.is_success() {
        return Err(anyhow!("Relay health check failed\n{output}"));
    }
    Ok(output)
}

fn relay_settings_from_env(
    values: &HashMap<String, String>,
    config_path: PathBuf,
    plist_path: PathBuf,
    runner_path: PathBuf,
) -> RelayMacClientSettings {
    let token = values
        .get("ITERATE_RELAY_TOKEN")
        .map(|value| value.trim().to_string())
        .unwrap_or_default();
    RelayMacClientSettings {
        relay_url: values.get("ITERATE_RELAY_URL").cloned().unwrap_or_default(),
        device_id: values
            .get("ITERATE_RELAY_DEVICE_ID")
            .cloned()
            .unwrap_or_else(|| "local-mac".to_string()),
        local_base_url: values
            .get("ITERATE_RELAY_LOCAL_BASE_URL")
            .cloned()
            .unwrap_or_else(|| "http://127.0.0.1:8080".to_string()),
        heartbeat_secs: values
            .get("ITERATE_RELAY_HEARTBEAT_SECS")
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(15),
        allow_recover: values
            .get("ITERATE_RELAY_ALLOW_RECOVER")
            .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on")),
        relay_token: String::new(),
        clear_relay_token: false,
        token_present: !token.is_empty(),
        config_path: config_path.display().to_string(),
        plist_path: plist_path.display().to_string(),
        runner_path: runner_path.display().to_string(),
    }
}

fn relay_token_for_save(
    request: &RelayMacClientSettings,
    existing: &HashMap<String, String>,
) -> String {
    if request.clear_relay_token {
        return String::new();
    }
    let requested = request.relay_token.trim();
    if requested.is_empty() {
        return existing
            .get("ITERATE_RELAY_TOKEN")
            .map(|value| value.trim().to_string())
            .unwrap_or_default();
    }
    requested.to_string()
}

#[tauri::command]
pub fn get_relay_mac_client_config() -> Result<RelayMacClientSettings, String> {
    let config_path =
        relay_mac_client_config_path().map_err(|error| format!("配置路径解析失败: {error}"))?;
    let plist_path =
        relay_mac_client_plist_path().map_err(|error| format!("plist 路径解析失败: {error}"))?;
    let runner_path =
        relay_mac_client_runner_path().map_err(|error| format!("runner 路径解析失败: {error}"))?;
    let values = parse_relay_env_file(&config_path);
    Ok(relay_settings_from_env(
        &values,
        config_path,
        plist_path,
        runner_path,
    ))
}

#[tauri::command]
pub fn save_relay_mac_client_config(
    request: RelayMacClientSettings,
) -> Result<RelayMacClientSettings, String> {
    let config_path =
        relay_mac_client_config_path().map_err(|error| format!("配置路径解析失败: {error}"))?;
    let plist_path =
        relay_mac_client_plist_path().map_err(|error| format!("plist 路径解析失败: {error}"))?;
    let runner_path =
        relay_mac_client_runner_path().map_err(|error| format!("runner 路径解析失败: {error}"))?;
    let existing = parse_relay_env_file(&config_path);

    let relay_url = request.relay_url.trim().to_string();
    let device_id = request.device_id.trim();
    let local_base_url = request.local_base_url.trim();
    let heartbeat_secs = request.heartbeat_secs.max(5);
    let token = relay_token_for_save(&request, &existing);

    if relay_url.is_empty() {
        return Err("Relay URL 不能为空".to_string());
    }

    let contents = format!(
        concat!(
            "# iterate relay Mac client config\n",
            "# Managed by iterate settings or scripts/install-relay-mac-client.sh\n",
            "export ITERATE_RELAY_URL={}\n",
            "export ITERATE_RELAY_DEVICE_ID={}\n",
            "export ITERATE_RELAY_TOKEN={}\n",
            "export ITERATE_RELAY_LOCAL_BASE_URL={}\n",
            "export ITERATE_RELAY_HEARTBEAT_SECS={}\n",
            "export ITERATE_RELAY_ALLOW_RECOVER={}\n"
        ),
        shell_quote(&relay_url),
        shell_quote(if device_id.is_empty() {
            "local-mac"
        } else {
            device_id
        }),
        shell_quote(&token),
        shell_quote(if local_base_url.is_empty() {
            "http://127.0.0.1:8080"
        } else {
            local_base_url
        }),
        heartbeat_secs,
        if request.allow_recover { "1" } else { "0" }
    );

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建 relay 配置目录失败: {} ({error})", parent.display()))?;
    }
    std::fs::write(&config_path, contents)
        .map_err(|error| format!("写入 relay 配置失败: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&config_path, std::fs::Permissions::from_mode(0o600));
    }

    let values = parse_relay_env_file(&config_path);
    Ok(relay_settings_from_env(
        &values,
        config_path,
        plist_path,
        runner_path,
    ))
}

#[tauri::command]
pub async fn control_relay_mac_client(
    action: String,
) -> Result<RelayMacClientControlResult, String> {
    if !cfg!(target_os = "macos") {
        return relay_mac_client_status_result(
            action.trim(),
            false,
            "Relay Mac Client 目前只支持 macOS LaunchAgent",
            "",
            "",
        );
    }

    let normalized_action = action.trim().to_ascii_lowercase();
    match normalized_action.as_str() {
        "status" => {
            relay_mac_client_status_result("status", true, "Relay Mac Client 状态已刷新", "", "")
        }
        "install" => match install_relay_mac_client_service_files() {
            Ok(()) => relay_mac_client_status_result(
                "install",
                true,
                "Relay Mac Client LaunchAgent 已安装",
                "",
                "",
            ),
            Err(error) => relay_mac_client_status_result(
                "install",
                false,
                format!("安装失败: {error}"),
                "",
                error.to_string(),
            ),
        },
        "start" => match start_relay_mac_client_service() {
            Ok((stdout, stderr)) => relay_mac_client_status_result(
                "start",
                true,
                "Relay Mac Client 已启动",
                stdout,
                stderr,
            ),
            Err(error) => relay_mac_client_status_result(
                "start",
                false,
                format!("启动失败: {error}"),
                "",
                error.to_string(),
            ),
        },
        "stop" => match stop_relay_mac_client_service() {
            Ok((stdout, stderr)) => relay_mac_client_status_result(
                "stop",
                true,
                "Relay Mac Client 已停止",
                stdout,
                stderr,
            ),
            Err(error) => relay_mac_client_status_result(
                "stop",
                false,
                format!("停止失败: {error}"),
                "",
                error.to_string(),
            ),
        },
        "restart" => {
            let _ = stop_relay_mac_client_service();
            match start_relay_mac_client_service() {
                Ok((stdout, stderr)) => relay_mac_client_status_result(
                    "restart",
                    true,
                    "Relay Mac Client 已安装并重启",
                    stdout,
                    stderr,
                ),
                Err(error) => relay_mac_client_status_result(
                    "restart",
                    false,
                    format!("重启失败: {error}"),
                    "",
                    error.to_string(),
                ),
            }
        }
        "health" => match relay_mac_client_health_output().await {
            Ok(output) => relay_mac_client_status_result(
                "health",
                true,
                "Relay Mac Client 体检完成",
                output,
                "",
            ),
            Err(error) => relay_mac_client_status_result(
                "health",
                false,
                format!("体检失败: {error}"),
                "",
                error.to_string(),
            ),
        },
        _ => Err("unsupported_relay_mac_client_action".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::Path;

    fn test_state(token: Option<&str>) -> RelayServerState {
        RelayServerState {
            inner: Arc::new(Mutex::new(RelayInner::new_with_audit_log_path(None))),
            token: token.map(str::to_string),
        }
    }

    fn relay_command(command_type: &str, scope: &str, expires_at: String) -> RelayCommand {
        RelayCommand {
            command_id: "cmd_test".to_string(),
            device_id: "local-mac".to_string(),
            command_type: command_type.to_string(),
            scope: scope.to_string(),
            status: "pending".to_string(),
            nonce: "nonce-test".to_string(),
            created_at: Utc::now().to_rfc3339(),
            expires_at,
            delivered_at: None,
            started_at: None,
            finished_at: None,
            result: None,
            payload: Value::Null,
        }
    }

    async fn response_json(response: Response) -> Value {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("response body bytes");
        serde_json::from_slice(&bytes).expect("json response")
    }

    fn bearer_headers(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).expect("authorization header"),
        );
        headers
    }

    #[test]
    fn mac_ws_url_uses_authorization_header_not_query_token() {
        let url = relay_mac_ws_url("https://relay.example.com", "mac 1");
        assert_eq!(url, "wss://relay.example.com/mac/ws?device_id=mac%201");
        assert!(!url.contains("token="));
    }

    #[test]
    fn mac_ws_url_preserves_existing_query() {
        let url = relay_mac_ws_url("ws://127.0.0.1:8790/mac/ws?env=dev", "local-mac");
        assert_eq!(
            url,
            "ws://127.0.0.1:8790/mac/ws?env=dev&device_id=local%2Dmac"
        );
    }

    #[test]
    fn local_bridge_ws_url_normalizes_http_base() {
        assert_eq!(
            local_bridge_ws_url("http://127.0.0.1:8080"),
            "ws://127.0.0.1:8080/ws"
        );
        assert_eq!(
            local_bridge_ws_url("https://bridge.example.com/ws"),
            "wss://bridge.example.com/ws"
        );
    }

    #[test]
    fn relay_http_base_from_url_accepts_ws_or_http_inputs() {
        assert_eq!(
            relay_http_base_from_url("wss://relay.example.com/mac/ws?env=dev"),
            "https://relay.example.com"
        );
        assert_eq!(
            relay_http_base_from_url("https://relay.example.com/"),
            "https://relay.example.com"
        );
        assert_eq!(
            relay_http_base_from_url("ws://127.0.0.1:8790/mac/ws"),
            "http://127.0.0.1:8790"
        );
    }

    #[test]
    fn relay_mobile_stream_ws_url_uses_device_stream_path() {
        assert_eq!(
            relay_mobile_stream_ws_url("wss://relay.example.com/mac/ws?env=dev", "mac 1"),
            "wss://relay.example.com/api/devices/mac%201/stream"
        );
        assert_eq!(
            relay_mobile_stream_ws_url("https://relay.example.com/", "local-mac"),
            "wss://relay.example.com/api/devices/local%2Dmac/stream"
        );
    }

    #[test]
    fn relay_mobile_pairing_config_omits_static_relay_token() {
        let values = HashMap::from([
            (
                "ITERATE_RELAY_URL".to_string(),
                "wss://relay.example.com/mac/ws".to_string(),
            ),
            (
                "ITERATE_RELAY_DEVICE_ID".to_string(),
                "local-mac".to_string(),
            ),
            (
                "ITERATE_RELAY_TOKEN".to_string(),
                "secret-static-token".to_string(),
            ),
        ]);

        let config =
            relay_mobile_pairing_config_from_values(&values).expect("relay config should exist");
        assert_eq!(config.base_url, "https://relay.example.com");
        assert_eq!(
            config.ws_url,
            "wss://relay.example.com/api/devices/local%2Dmac/stream"
        );
        assert_eq!(config.relay_device_id, "local-mac");
        assert!(config.token_present);
        let serialized = serde_json::to_string(&config).expect("serialize config");
        assert!(!serialized.contains("secret-static-token"));
        assert!(!serialized.contains("ITERATE_RELAY_TOKEN"));
        assert!(!serialized.contains("token="));
    }

    #[test]
    fn relay_auth_requires_matching_bearer_token_when_configured() {
        let state = test_state(Some("secret"));
        let headers = HeaderMap::new();
        let err = authorize_http(&state, &headers).expect_err("missing token rejected");
        assert_eq!(err.status(), StatusCode::UNAUTHORIZED);

        let mut headers = HeaderMap::new();
        headers.insert("authorization", HeaderValue::from_static("Bearer secret"));
        assert!(authorize_http(&state, &headers).is_ok());
    }

    #[tokio::test]
    async fn relay_mobile_pairing_claim_issues_scoped_device_token() {
        let state = test_state(Some("admin-token"));

        let unauthorized_issue = handle_issue_mobile_pairing(
            State(state.clone()),
            HeaderMap::new(),
            Path("local-mac".to_string()),
            Json(RelayMobilePairingIssueRequest { device_name: None }),
        )
        .await;
        assert_eq!(unauthorized_issue.status(), StatusCode::UNAUTHORIZED);

        let issue = handle_issue_mobile_pairing(
            State(state.clone()),
            bearer_headers("admin-token"),
            Path("local-mac".to_string()),
            Json(RelayMobilePairingIssueRequest {
                device_name: Some("MacBook".to_string()),
            }),
        )
        .await;
        assert_eq!(issue.status(), StatusCode::OK);
        let issue_json = response_json(issue).await;
        let pairing_token = issue_json["relay_pairing_token"]
            .as_str()
            .expect("relay pairing token")
            .to_string();
        assert!(pairing_token.starts_with("rpt_"));
        assert_ne!(pairing_token, "admin-token");

        let claim = handle_claim_mobile_pairing(
            State(state.clone()),
            Json(RelayMobilePairingClaimRequest {
                pairing_token: pairing_token.clone(),
                device_id: "ios-device-1".to_string(),
                device_name: Some("Alice iPhone".to_string()),
                client_kind: Some("ios".to_string()),
                platform: None,
            }),
        )
        .await;
        assert_eq!(claim.status(), StatusCode::OK);
        let claim_json = response_json(claim).await;
        assert_eq!(claim_json["device_id"], "ios-device-1");
        assert_eq!(claim_json["relay_device_id"], "local-mac");
        let device_token = claim_json["device_token"]
            .as_str()
            .expect("relay device token")
            .to_string();
        assert!(device_token.starts_with("rmt_"));
        assert!(claim_json["scopes"]
            .as_array()
            .expect("scopes")
            .iter()
            .any(|scope| scope.as_str() == Some("session.stream")));

        let reused = handle_claim_mobile_pairing(
            State(state.clone()),
            Json(RelayMobilePairingClaimRequest {
                pairing_token,
                device_id: "ios-device-1".to_string(),
                device_name: None,
                client_kind: None,
                platform: Some("ios".to_string()),
            }),
        )
        .await;
        assert_eq!(reused.status(), StatusCode::UNAUTHORIZED);

        let status = handle_device_status(
            State(state.clone()),
            bearer_headers(&device_token),
            Path("local-mac".to_string()),
        )
        .await;
        assert_eq!(status.status(), StatusCode::OK);

        let other_device = handle_device_status(
            State(state.clone()),
            bearer_headers(&device_token),
            Path("other-mac".to_string()),
        )
        .await;
        assert_eq!(other_device.status(), StatusCode::FORBIDDEN);

        let recover = handle_create_command(
            State(state.clone()),
            bearer_headers(&device_token),
            Path("local-mac".to_string()),
            Json(CreateRelayCommandRequest {
                command_type: "recover_public_tunnel".to_string(),
                payload: Value::Null,
            }),
        )
        .await;
        assert_eq!(recover.status(), StatusCode::FORBIDDEN);

        let unauthorized_revoke = handle_revoke_mobile_credential(
            State(state.clone()),
            bearer_headers(&device_token),
            Json(RelayMobileCredentialRevokeRequest {
                device_token: Some(device_token.clone()),
                mobile_token: None,
                device_id: None,
                mobile_device_id: None,
                relay_device_id: Some("local-mac".to_string()),
            }),
        )
        .await;
        assert_eq!(unauthorized_revoke.status(), StatusCode::UNAUTHORIZED);

        let revoke = handle_revoke_mobile_credential(
            State(state.clone()),
            bearer_headers("admin-token"),
            Json(RelayMobileCredentialRevokeRequest {
                device_token: Some(device_token.clone()),
                mobile_token: None,
                device_id: None,
                mobile_device_id: None,
                relay_device_id: Some("local-mac".to_string()),
            }),
        )
        .await;
        assert_eq!(revoke.status(), StatusCode::OK);
        let revoke_json = response_json(revoke).await;
        assert_eq!(revoke_json["revoked"], 1);

        let revoked_status = handle_device_status(
            State(state),
            bearer_headers(&device_token),
            Path("local-mac".to_string()),
        )
        .await;
        assert_eq!(revoked_status.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response_json(revoked_status).await["error"],
            "revoked_device_auth"
        );
    }

    #[test]
    fn stale_mac_disconnect_does_not_remove_current_sender() {
        let mut inner = RelayInner::new_with_audit_log_path(None);
        let (old_tx, _old_rx) = mpsc::unbounded_channel();
        let (new_tx, _new_rx) = mpsc::unbounded_channel();

        inner.register_mac_sender("local-mac".to_string(), "old-conn".to_string(), old_tx);
        inner.register_mac_sender("local-mac".to_string(), "new-conn".to_string(), new_tx);

        assert!(!inner.remove_mac_sender_if_current("local-mac", "old-conn"));
        assert_eq!(
            inner
                .mac_senders
                .get("local-mac")
                .map(|entry| entry.connection_id.as_str()),
            Some("new-conn")
        );

        assert!(inner.remove_mac_sender_if_current("local-mac", "new-conn"));
        assert!(!inner.mac_senders.contains_key("local-mac"));
    }

    #[test]
    fn relay_env_parser_handles_bash_percent_q_and_single_quotes() {
        assert_eq!(
            shell_unquote("wss://relay.example.com/mac/ws\\?env=dev\\&x=1"),
            "wss://relay.example.com/mac/ws?env=dev&x=1"
        );
        assert_eq!(shell_unquote("token\\ with\\ spaces"), "token with spaces");
        assert_eq!(shell_unquote("token\\"), "token\\");
        assert_eq!(shell_unquote("'token'\\''with-quote'"), "token'with-quote");
    }

    #[test]
    fn relay_token_save_distinguishes_preserve_clear_and_replace() {
        let mut existing = HashMap::new();
        existing.insert("ITERATE_RELAY_TOKEN".to_string(), "old-token".to_string());

        let mut request = RelayMacClientSettings {
            relay_url: "wss://relay.example.com/mac/ws".to_string(),
            device_id: "local-mac".to_string(),
            local_base_url: "http://127.0.0.1:8080".to_string(),
            heartbeat_secs: 15,
            allow_recover: false,
            relay_token: String::new(),
            clear_relay_token: false,
            token_present: true,
            config_path: String::new(),
            plist_path: String::new(),
            runner_path: String::new(),
        };

        assert_eq!(relay_token_for_save(&request, &existing), "old-token");

        request.clear_relay_token = true;
        assert_eq!(relay_token_for_save(&request, &existing), "");

        request.clear_relay_token = false;
        request.relay_token = "new-token".to_string();
        assert_eq!(relay_token_for_save(&request, &existing), "new-token");
    }

    #[test]
    fn relay_reconnect_backoff_resets_after_healthy_session() {
        assert_eq!(
            relay_reconnect_reset_after(15),
            std::time::Duration::from_secs(30)
        );
        assert_eq!(
            relay_reconnect_reset_after(1),
            std::time::Duration::from_secs(10)
        );
        assert!(!relay_should_reset_reconnect_attempt(
            std::time::Duration::from_secs(29),
            15
        ));
        assert!(relay_should_reset_reconnect_attempt(
            std::time::Duration::from_secs(30),
            15
        ));
    }

    #[test]
    fn local_bridge_health_exposes_broker_retry_without_losing_bounded_backoff() {
        let mut health = LocalBridgeHealth::new();
        health.record(
            "auth_unavailable",
            7,
            Some("bridge_auth_broker_unavailable".to_string()),
        );
        let snapshot = serde_json::to_value(&health).expect("serialize local bridge health");
        assert_eq!(snapshot["status"], "auth_unavailable");
        assert_eq!(snapshot["retry_attempt"], 7);
        assert_eq!(snapshot["last_error"], "bridge_auth_broker_unavailable");
        assert_eq!(relay_reconnect_delay(7), Duration::from_secs(60));
        assert_eq!(relay_reconnect_delay(u32::MAX), Duration::from_secs(60));
    }

    #[tokio::test]
    async fn create_command_rejects_unsupported_command() {
        let response = handle_create_command(
            State(test_state(None)),
            HeaderMap::new(),
            Path("local-mac".to_string()),
            Json(CreateRelayCommandRequest {
                command_type: "shell".to_string(),
                payload: Value::Null,
            }),
        )
        .await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(response).await["error"],
            "unsupported_command"
        );
    }

    #[tokio::test]
    async fn create_command_enforces_recovery_cooldown() {
        let state = test_state(None);
        let request = || CreateRelayCommandRequest {
            command_type: "recover_public_tunnel".to_string(),
            payload: Value::Null,
        };

        let first = handle_create_command(
            State(state.clone()),
            HeaderMap::new(),
            Path("local-mac".to_string()),
            Json(request()),
        )
        .await;
        assert_eq!(first.status(), StatusCode::ACCEPTED);

        let second = handle_create_command(
            State(state),
            HeaderMap::new(),
            Path("local-mac".to_string()),
            Json(request()),
        )
        .await;
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response_json(second).await["error"], "cooldown_active");
    }

    #[test]
    fn command_spec_supports_tailscale_and_auto_public_recovery() {
        let tailscale = command_spec("recover_tailscale_funnel").expect("tailscale spec");
        assert_eq!(tailscale.scope, "tunnel.recover");
        assert_eq!(tailscale.cooldown_secs, 300);

        let auto = command_spec("recover_public_transport_auto").expect("auto spec");
        assert_eq!(auto.scope, "tunnel.recover");
        assert_eq!(auto.cooldown_secs, 300);

        let sessions = command_spec("get_sessions").expect("sessions spec");
        assert_eq!(sessions.scope, "session.read");
        assert_eq!(sessions.cooldown_secs, 0);

        let mcp_action = command_spec("mcp_action").expect("mcp_action spec");
        assert_eq!(mcp_action.scope, "session.respond");
        assert_eq!(mcp_action.cooldown_secs, 0);
    }

    #[test]
    fn pairing_primary_detects_tailscale_funnel_base_url() {
        let pairing = json!({
            "transport_mode": "public_tunnel",
            "base_url": "https://macbook-air.tail5b0fb3.ts.net",
            "ws_url": "wss://macbook-air.tail5b0fb3.ts.net/ws"
        });
        assert!(pairing_primary_is_tailscale_funnel(&pairing));

        let cloudflare = json!({
            "transport_mode": "public_tunnel",
            "base_url": "https://iterate.example.com",
            "ws_url": "wss://iterate.example.com/ws"
        });
        assert!(!pairing_primary_is_tailscale_funnel(&cloudflare));
    }

    #[test]
    fn mac_client_rejects_scope_mismatch_before_execution() {
        let expires_at = (Utc::now() + ChronoDuration::seconds(60)).to_rfc3339();
        let command = relay_command("get_status", "tunnel.recover", expires_at);
        let error = validate_relay_command_for_execution(&command, "local-mac").unwrap_err();
        assert!(error.to_string().contains("scope mismatch"));
    }

    #[test]
    fn mac_client_rejects_device_mismatch_before_execution() {
        let expires_at = (Utc::now() + ChronoDuration::seconds(60)).to_rfc3339();
        let command = relay_command("get_status", "status.read", expires_at);
        let error = validate_relay_command_for_execution(&command, "other-mac").unwrap_err();
        assert!(error.to_string().contains("device mismatch"));
    }

    #[test]
    fn mac_client_rejects_expired_command_before_execution() {
        let expires_at = (Utc::now() - ChronoDuration::seconds(1)).to_rfc3339();
        let command = relay_command("get_status", "status.read", expires_at);
        let error = validate_relay_command_for_execution(&command, "local-mac").unwrap_err();
        assert_eq!(error.to_string(), "command expired");
    }

    #[test]
    fn command_result_preserves_mac_timing_fields() {
        let state = test_state(None);
        let command = relay_command(
            "get_status",
            "status.read",
            (Utc::now() + ChronoDuration::seconds(60)).to_rfc3339(),
        );
        let command_id = command.command_id.clone();
        {
            let mut inner = state.inner.lock();
            inner.insert_command(command);
        }

        handle_mac_text(
            &state,
            "local-mac",
            &json!({
                "kind": "command_result",
                "command_id": command_id,
                "status": "succeeded",
                "started_at": "2026-06-14T00:00:01Z",
                "finished_at": "2026-06-14T00:00:02Z",
                "result": { "ok": true }
            })
            .to_string(),
        );

        let inner = state.inner.lock();
        let command = inner.commands.get("cmd_test").expect("command exists");
        assert_eq!(command.status, "succeeded");
        assert_eq!(command.started_at.as_deref(), Some("2026-06-14T00:00:01Z"));
        assert_eq!(command.finished_at.as_deref(), Some("2026-06-14T00:00:02Z"));
        assert_eq!(command.result.as_ref().unwrap()["ok"], true);
    }

    #[test]
    fn command_result_rejects_device_mismatch() {
        let state = test_state(None);
        let command = relay_command(
            "get_status",
            "status.read",
            (Utc::now() + ChronoDuration::seconds(60)).to_rfc3339(),
        );
        let command_id = command.command_id.clone();
        {
            let mut inner = state.inner.lock();
            inner.insert_command(command);
        }

        handle_mac_text(
            &state,
            "other-mac",
            &json!({
                "kind": "command_result",
                "command_id": command_id,
                "status": "succeeded",
                "result": { "ok": true }
            })
            .to_string(),
        );

        let inner = state.inner.lock();
        let command = inner.commands.get("cmd_test").expect("command exists");
        assert_eq!(command.status, "pending");
        assert!(command.result.is_none());
        assert_eq!(
            inner.audit.front().map(|event| event.kind.as_str()),
            Some("command_result_rejected")
        );
    }

    #[test]
    fn relay_status_sessions_extracts_heartbeat_sessions() {
        let status = json!({
            "connection": { "local_origin": "healthy" },
            "active_sessions": {
                "sessions": [
                    {
                        "request_id": "serve-1",
                        "project_path": "/tmp/project",
                        "project_name": "project",
                        "title": "hello"
                    }
                ]
            }
        });

        let sessions = relay_active_sessions_from_status(&status);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0]["request_id"], "serve-1");
    }

    #[test]
    fn mac_bridge_message_is_broadcast_to_device_stream() {
        let state = test_state(None);
        let mut rx = {
            let mut inner = state.inner.lock();
            inner.stream_sender("local-mac").subscribe()
        };

        handle_mac_text(
            &state,
            "local-mac",
            &json!({
                "kind": "bridge_message",
                "message": {
                    "message_type": "mcp_state",
                    "payload": {
                        "request": {
                            "message": "hello",
                            "project_path": "/tmp/project"
                        }
                    }
                }
            })
            .to_string(),
        );

        let text = rx.try_recv().expect("stream message broadcast");
        let message: Value = serde_json::from_str(&text).expect("bridge message json");
        assert_eq!(message["message_type"], "mcp_state");
        assert_eq!(message["payload"]["request"]["message"], "hello");

        let inner = state.inner.lock();
        let event = inner.audit.front().expect("broadcast audit event");
        assert_eq!(event.kind, "mac_bridge_message_broadcast");
        assert_eq!(event.device_id.as_deref(), Some("local-mac"));
        assert_eq!(event.metadata["subscribers"], 1);
    }

    #[test]
    fn relay_bridge_message_accepts_scoped_mcp_action() {
        let message = relay_bridge_message_from_payload(&json!({
            "action": "submit",
            "project_path": "/Users/example/cunzhi",
            "request_id": "serve-1",
            "user_input": "hello",
            "selected_options": [],
            "images": []
        }))
        .expect("valid mcp action");

        assert_eq!(message["message_type"], "mcp_action");
        assert_eq!(message["payload"]["action"], "submit");

        let window_state = relay_bridge_message_from_payload(&json!({
            "message_type": "mcp_action",
            "payload": {
                "action": "update_window_conditional_state",
                "project_path": "/Users/example/cunzhi",
                "request_id": "serve-1",
                "promptId": "prompt-1",
                "newState": true
            }
        }))
        .expect("window conditional action remains allowed");
        assert_eq!(
            window_state["payload"]["action"],
            "update_window_conditional_state"
        );
    }

    #[test]
    fn relay_mobile_bridge_message_accepts_sync_and_rejects_unsafe_types() {
        let sync = relay_mobile_bridge_message_from_text(
            &json!({
                "message_type": "request_sync",
                "payload": {
                    "project_path": "/Users/example/cunzhi"
                }
            })
            .to_string(),
        )
        .expect("request_sync allowed");
        assert_eq!(sync["message_type"], "request_sync");

        let system = relay_mobile_bridge_message_from_text(
            &json!({
                "message_type": "system_command",
                "payload": { "command": "toggle_prevent_sleep" }
            })
            .to_string(),
        )
        .expect("safe system command allowed");
        assert_eq!(system["payload"]["command"], "toggle_prevent_sleep");

        let unsafe_system = relay_mobile_bridge_message_from_text(
            &json!({
                "message_type": "system_command",
                "payload": { "command": "open_terminal" }
            })
            .to_string(),
        )
        .unwrap_err();
        assert!(unsafe_system
            .to_string()
            .contains("unsupported relay system_command"));

        let unsupported = relay_mobile_bridge_message_from_text(
            &json!({
                "message_type": "shell",
                "payload": {}
            })
            .to_string(),
        )
        .unwrap_err();
        assert!(unsupported
            .to_string()
            .contains("unsupported relay mobile message_type"));
    }

    #[test]
    fn relay_bridge_message_rejects_unscoped_or_unknown_actions() {
        let missing_project = relay_bridge_message_from_payload(&json!({
            "action": "submit",
            "request_id": "serve-1",
            "user_input": "hello"
        }))
        .unwrap_err();
        assert!(missing_project.to_string().contains("missing project_path"));

        let unsupported = relay_bridge_message_from_payload(&json!({
            "message_type": "mcp_action",
            "payload": {
                "action": "send_to_browser_ai",
                "project_path": "/Users/example/cunzhi",
                "user_input": "hello"
            }
        }))
        .unwrap_err();
        assert!(unsupported
            .to_string()
            .contains("unsupported relay mcp_action"));
    }

    #[tokio::test]
    async fn post_local_bridge_publish_accepts_no_content_response() {
        let app = Router::new().route("/bridge/publish", post(|| async { StatusCode::NO_CONTENT }));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test bridge server");
        let addr = listener.local_addr().expect("test bridge server addr");
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("test bridge server");
        });

        let config = RelayMacClientConfig {
            relay_url: "wss://relay.example.com/mac/ws".to_string(),
            device_id: "local-mac".to_string(),
            token: None,
            local_base_url: format!("http://{addr}"),
            heartbeat_secs: 10,
            allow_recover: false,
        };

        let result = post_local_bridge_publish(
            &config,
            &json!({
                "action": "update_window_conditional_state",
                "project_path": "/Users/example/cunzhi",
                "request_id": "relay-no-content-test",
                "promptId": "prompt-1",
                "newState": true
            }),
        )
        .await
        .expect("204 No Content is a successful bridge publish response");

        assert_eq!(result["http_status"], 204);
        assert!(result["body"].is_null());
        server.abort();
    }

    #[test]
    fn summarize_connection_status_maps_bridge_and_tunnel_fields() {
        let summary = summarize_connection_status(&json!({
            "local_origin": { "healthy": true },
            "public_tunnel": { "healthy": false },
            "root_tunnel": {
                "derived": {
                    "tunnel_health_class": "needs_edge_path_fix",
                    "edge_7844_suspected": true,
                    "backoff_remaining_secs": 120
                },
                "metrics": { "effective_ha_connection_count": 1 }
            },
            "diagnosis": { "code": "root_tunnel_ha_degraded" }
        }));

        assert_eq!(summary["local_origin"], "healthy");
        assert_eq!(summary["public_tunnel"], "failed");
        assert_eq!(summary["root_tunnel_health_class"], "needs_edge_path_fix");
        assert_eq!(summary["ha_count"], 1);
        assert_eq!(summary["edge_7844_suspected"], true);
        assert_eq!(summary["backoff_remaining_secs"], 120);
        assert_eq!(summary["diagnosis"], "root_tunnel_ha_degraded");
    }
}
