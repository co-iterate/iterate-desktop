use crate::config::{load_standalone_config, save_standalone_config, AppConfig};
use anyhow::{bail, Result};
use futures::{SinkExt, StreamExt};
use ring::hmac;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        http::StatusCode,
        Message,
    },
};
use uuid::Uuid;

use super::detector::AiCompletionEvent;

const WS_PORT: u16 = 9333;
const BROWSER_WS_TOKEN_ENV: &str = "ITERATE_BROWSER_WS_TOKEN";
const BROWSER_WS_TOKEN_PREFIX: &str = "bwst_";
const BROWSER_WS_AUTH_CLIENT_HELLO: &str = "auth_client_hello";
const BROWSER_WS_AUTH_SERVER_HELLO: &str = "auth_server_hello";
const BROWSER_WS_AUTH_CLIENT_PROOF: &str = "auth_client_proof";
const BROWSER_WS_AUTH_OK: &str = "auth_ok";
const BROWSER_WS_AUTH_FAILED: &str = "auth_failed";
const BROWSER_WS_AUTH_REQUIRED: &str = "auth_required";
const BROWSER_WS_MIN_NONCE_LEN: usize = 16;
const BROWSER_WS_MAX_NONCE_LEN: usize = 128;
const BROWSER_WS_ALLOWED_EXTENSION_ORIGIN_PREFIXES: &[&str] = &[
    "chrome-extension://",
    "moz-extension://",
    "safari-web-extension://",
];

fn browser_ws_normalized_token(value: &str) -> Option<String> {
    let token = value.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

fn browser_ws_new_pairing_token() -> String {
    format!(
        "{}{}{}",
        BROWSER_WS_TOKEN_PREFIX,
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}

pub(crate) fn browser_ws_pairing_token_in_config(config: &AppConfig) -> Option<String> {
    browser_ws_normalized_token(&config.browser_ws_config.token)
}

pub(crate) fn browser_ws_pairing_token_from_sources(
    env_token: Option<&str>,
    config: &AppConfig,
) -> Result<String> {
    if let Some(token) = env_token.and_then(browser_ws_normalized_token) {
        return Ok(token);
    }
    if let Some(token) = browser_ws_pairing_token_in_config(config) {
        return Ok(token);
    }
    bail!(
        "{} 未配置，且本地 Browser WebSocket 配对密钥为空",
        BROWSER_WS_TOKEN_ENV
    )
}

pub(crate) fn ensure_browser_ws_pairing_token_in_config(config: &mut AppConfig) -> Result<String> {
    if let Some(token) = browser_ws_pairing_token_in_config(config) {
        return Ok(token);
    }

    let token = browser_ws_new_pairing_token();
    config.browser_ws_config.token = token.clone();
    Ok(token)
}

pub fn ensure_browser_ws_pairing_token() -> Result<String> {
    let env_token = std::env::var(BROWSER_WS_TOKEN_ENV).ok();
    let mut config = load_standalone_config()?;
    if let Ok(token) = browser_ws_pairing_token_from_sources(env_token.as_deref(), &config) {
        return Ok(token);
    }

    let token = ensure_browser_ws_pairing_token_in_config(&mut config)?;
    save_standalone_config(&config)?;
    Ok(token)
}

fn browser_ws_token_from_config_or_env() -> Result<String> {
    let env_token = std::env::var(BROWSER_WS_TOKEN_ENV).ok();
    if let Some(token) = env_token.as_deref().and_then(browser_ws_normalized_token) {
        return Ok(token);
    }

    ensure_browser_ws_pairing_token()
}

fn browser_ws_new_nonce() -> String {
    Uuid::new_v4().simple().to_string()
}

fn browser_ws_origin_is_trusted(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(origin) => {
            let origin = origin.trim();
            !origin.is_empty()
                && BROWSER_WS_ALLOWED_EXTENSION_ORIGIN_PREFIXES
                    .iter()
                    .any(|prefix| origin.starts_with(prefix))
        }
    }
}

fn browser_ws_forbidden_origin_response() -> ErrorResponse {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(Some("browser websocket origin rejected".to_string()))
        .expect("build browser websocket forbidden origin response")
}

fn browser_ws_validate_origin_request(
    request: &Request,
    response: Response,
) -> std::result::Result<Response, ErrorResponse> {
    let origin = match request.headers().get("origin") {
        None => None,
        Some(value) => match value.to_str() {
            Ok(origin) => Some(origin),
            Err(_) => return Err(browser_ws_forbidden_origin_response()),
        },
    };

    if browser_ws_origin_is_trusted(origin) {
        Ok(response)
    } else {
        log::warn!(
            "[Browser WS] 拒绝不可信 Origin 的握手: {}",
            origin.unwrap_or("<missing>")
        );
        Err(browser_ws_forbidden_origin_response())
    }
}

fn browser_ws_nonce_from_value<'a>(data: &'a serde_json::Value, field: &str) -> Option<&'a str> {
    let nonce = data.get(field)?.as_str()?;
    let len = nonce.len();
    if !(BROWSER_WS_MIN_NONCE_LEN..=BROWSER_WS_MAX_NONCE_LEN).contains(&len) {
        return None;
    }
    if nonce
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        Some(nonce)
    } else {
        None
    }
}

fn browser_ws_hmac_hex(
    token: &str,
    purpose: &str,
    client_nonce: &str,
    server_nonce: &str,
) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, token.as_bytes());
    let payload = format!("{}:{}:{}", purpose, client_nonce, server_nonce);
    hex::encode(hmac::sign(&key, payload.as_bytes()).as_ref())
}

fn browser_ws_server_proof(token: &str, client_nonce: &str, server_nonce: &str) -> String {
    browser_ws_hmac_hex(token, "server", client_nonce, server_nonce)
}

fn browser_ws_client_proof(token: &str, client_nonce: &str, server_nonce: &str) -> String {
    browser_ws_hmac_hex(token, "client", client_nonce, server_nonce)
}

fn browser_ws_constant_time_eq(expected: &str, actual: &str) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    expected
        .bytes()
        .zip(actual.bytes())
        .fold(0_u8, |diff, (left, right)| diff | (left ^ right))
        == 0
}

fn browser_ws_server_proof_matches(
    proof: &str,
    token: &str,
    client_nonce: &str,
    server_nonce: &str,
) -> bool {
    let expected = browser_ws_server_proof(token, client_nonce, server_nonce);
    browser_ws_constant_time_eq(&expected, proof)
}

fn browser_ws_client_proof_matches(
    data: &serde_json::Value,
    token: &str,
    expected_client_nonce: &str,
    expected_server_nonce: &str,
) -> bool {
    if data.get("type").and_then(|v| v.as_str()) != Some(BROWSER_WS_AUTH_CLIENT_PROOF) {
        return false;
    }
    if browser_ws_nonce_from_value(data, "clientNonce") != Some(expected_client_nonce) {
        return false;
    }
    if browser_ws_nonce_from_value(data, "serverNonce") != Some(expected_server_nonce) {
        return false;
    }
    let Some(proof) = data.get("clientProof").and_then(|v| v.as_str()) else {
        return false;
    };
    let expected = browser_ws_client_proof(token, expected_client_nonce, expected_server_nonce);
    browser_ws_constant_time_eq(&expected, proof)
}

fn browser_ws_client_hello_message(client_nonce: &str) -> serde_json::Value {
    serde_json::json!({
        "type": BROWSER_WS_AUTH_CLIENT_HELLO,
        "clientNonce": client_nonce,
    })
}

fn browser_ws_server_hello_message(
    token: &str,
    client_nonce: &str,
    server_nonce: &str,
) -> serde_json::Value {
    serde_json::json!({
        "type": BROWSER_WS_AUTH_SERVER_HELLO,
        "clientNonce": client_nonce,
        "serverNonce": server_nonce,
        "serverProof": browser_ws_server_proof(token, client_nonce, server_nonce),
    })
}

fn browser_ws_client_proof_message(
    token: &str,
    client_nonce: &str,
    server_nonce: &str,
) -> serde_json::Value {
    serde_json::json!({
        "type": BROWSER_WS_AUTH_CLIENT_PROOF,
        "clientNonce": client_nonce,
        "serverNonce": server_nonce,
        "clientProof": browser_ws_client_proof(token, client_nonce, server_nonce),
    })
}

/// 发送到浏览器的消息
#[derive(Debug, Clone)]
pub struct BrowserMessage {
    pub message_type: String,
    pub message: String,
    pub tab_id: Option<u32>,
}

/// 全局消息发送通道
static BROWSER_TX: once_cell::sync::Lazy<Arc<RwLock<Option<mpsc::Sender<BrowserMessage>>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(None)));

/// 发送消息到浏览器
pub async fn send_to_browser(message: String) -> Result<()> {
    log::info!("尝试发送消息到浏览器: {}", message);

    // 先尝试通过 channel 发送（主应用进程）
    {
        let tx = BROWSER_TX.read().await;
        if let Some(ref sender) = *tx {
            let msg = BrowserMessage {
                message_type: "send_message".to_string(),
                message: message.clone(),
                tab_id: None,
            };
            log::info!("发送消息通过 channel...");
            match sender.send(msg).await {
                Ok(_) => {
                    log::info!("消息已发送到 channel");
                    return Ok(());
                }
                Err(e) => {
                    log::warn!("Channel 发送失败: {}，连接可能已断开", e);
                    // 清理失效的发送器
                    drop(tx);
                    let mut tx_write = BROWSER_TX.write().await;
                    *tx_write = None;
                }
            }
        }
    }

    // 如果 channel 不可用，作为客户端直接连接发送（弹窗进程）
    log::info!("Channel 不可用，尝试作为客户端发送...");
    send_as_client(message).await
}

/// 作为 WebSocket 客户端发送消息（用于弹窗进程）
async fn send_as_client(message: String) -> Result<()> {
    use tokio_tungstenite::connect_async;

    let token = browser_ws_token_from_config_or_env()?;
    let url = format!("ws://127.0.0.1:{}", WS_PORT);
    log::info!("连接到 WebSocket 服务器: {}", url);

    let (ws_stream, _) = connect_async(&url)
        .await
        .map_err(|e| anyhow::anyhow!("连接 WebSocket 失败: {}", e))?;

    let (mut write, mut read) = ws_stream.split();
    let client_nonce = browser_ws_new_nonce();

    write
        .send(Message::Text(
            browser_ws_client_hello_message(&client_nonce).to_string(),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("发送 WebSocket 认证 hello 失败: {}", e))?;

    let server_hello = read
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("WebSocket 服务器在认证前关闭连接"))?
        .map_err(|e| anyhow::anyhow!("读取 WebSocket 认证响应失败: {}", e))?;
    let Message::Text(server_hello_text) = server_hello else {
        bail!("WebSocket 服务器返回了非文本认证响应");
    };
    let server_hello_data: serde_json::Value = serde_json::from_str(&server_hello_text)
        .map_err(|e| anyhow::anyhow!("解析 WebSocket 认证响应失败: {}", e))?;
    if server_hello_data.get("type").and_then(|v| v.as_str()) != Some(BROWSER_WS_AUTH_SERVER_HELLO)
    {
        bail!("WebSocket 服务器未返回认证 challenge");
    }
    if browser_ws_nonce_from_value(&server_hello_data, "clientNonce") != Some(client_nonce.as_str())
    {
        bail!("WebSocket 认证 challenge 的 clientNonce 不匹配");
    }
    let server_nonce = browser_ws_nonce_from_value(&server_hello_data, "serverNonce")
        .ok_or_else(|| anyhow::anyhow!("WebSocket 认证 challenge 缺少 serverNonce"))?;
    let server_proof = server_hello_data
        .get("serverProof")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if !browser_ws_server_proof_matches(server_proof, &token, &client_nonce, server_nonce) {
        bail!("WebSocket 服务器身份认证失败");
    }

    write
        .send(Message::Text(
            browser_ws_client_proof_message(&token, &client_nonce, server_nonce).to_string(),
        ))
        .await
        .map_err(|e| anyhow::anyhow!("发送 WebSocket 认证 proof 失败: {}", e))?;

    let auth_ok = read
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("WebSocket 服务器在认证确认前关闭连接"))?
        .map_err(|e| anyhow::anyhow!("读取 WebSocket 认证确认失败: {}", e))?;
    let Message::Text(auth_ok_text) = auth_ok else {
        bail!("WebSocket 服务器返回了非文本认证确认");
    };
    let auth_ok_data: serde_json::Value = serde_json::from_str(&auth_ok_text)
        .map_err(|e| anyhow::anyhow!("解析 WebSocket 认证确认失败: {}", e))?;
    if auth_ok_data.get("type").and_then(|v| v.as_str()) != Some(BROWSER_WS_AUTH_OK) {
        bail!("WebSocket 服务器拒绝认证");
    }

    let json = serde_json::json!({
        "type": "send_message",
        "message": message,
    });

    log::info!("发送消息: {}", json);
    write
        .send(Message::Text(json.to_string()))
        .await
        .map_err(|e| anyhow::anyhow!("发送消息失败: {}", e))?;

    log::info!("消息已发送");
    Ok(())
}

/// WebSocket 服务器状态
pub struct WsServer {
    event_tx: broadcast::Sender<AiCompletionEvent>,
    running: Arc<RwLock<bool>>,
    server_task: RwLock<Option<tokio::task::JoinHandle<()>>>,
}

impl Default for WsServer {
    fn default() -> Self {
        Self::new()
    }
}

impl WsServer {
    pub fn new() -> Self {
        let (event_tx, _) = broadcast::channel(100);
        Self {
            event_tx,
            running: Arc::new(RwLock::new(false)),
            server_task: RwLock::new(None),
        }
    }

    /// 获取事件发送器
    pub fn get_event_sender(&self) -> broadcast::Sender<AiCompletionEvent> {
        self.event_tx.clone()
    }

    /// 启动 WebSocket 服务器
    pub async fn start(&self) -> Result<()> {
        let addr = format!("127.0.0.1:{}", WS_PORT);
        let listener = TcpListener::bind(&addr).await?;

        log::info!("WebSocket 服务器已启动: {}", addr);

        *self.running.write().await = true;
        let running = self.running.clone();
        let event_tx = self.event_tx.clone();

        let server_task = tokio::spawn(async move {
            while *running.read().await {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        log::info!("新的 WebSocket 连接: {}", addr);
                        let event_tx = event_tx.clone();
                        // 为每个连接创建消息通道
                        let (new_tx, new_rx) = mpsc::channel::<BrowserMessage>(100);
                        // 传递发送器，连接处理器会在确认是浏览器扩展后更新 BROWSER_TX
                        tokio::spawn(handle_connection(stream, event_tx, new_rx, new_tx));
                    }
                    Err(e) => {
                        log::error!("接受连接失败: {}", e);
                    }
                }
            }
        });
        *self.server_task.write().await = Some(server_task);

        Ok(())
    }

    /// 停止服务器
    pub async fn stop(&self) {
        *self.running.write().await = false;
        if let Some(server_task) = self.server_task.write().await.take() {
            server_task.abort();
            let _ = server_task.await;
        }
    }
}

/// 处理单个 WebSocket 连接
async fn handle_connection(
    stream: TcpStream,
    event_tx: broadcast::Sender<AiCompletionEvent>,
    mut browser_rx: mpsc::Receiver<BrowserMessage>,
    browser_tx: mpsc::Sender<BrowserMessage>,
) {
    let token = match browser_ws_token_from_config_or_env() {
        Ok(token) => token,
        Err(error) => {
            log::warn!("[Browser WS] 拒绝连接: {}", error);
            return;
        }
    };
    let mut is_browser_extension = false;
    let mut connection_authenticated = false;
    let mut auth_client_nonce: Option<String> = None;
    let mut auth_server_nonce: Option<String> = None;
    let ws_stream = match accept_hdr_async(stream, browser_ws_validate_origin_request).await {
        Ok(ws) => ws,
        Err(e) => {
            log::error!("WebSocket 握手失败: {}", e);
            return;
        }
    };

    let (mut write, mut read) = ws_stream.split();

    loop {
        tokio::select! {
            // 接收浏览器扩展的消息
            msg = read.next() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        log::debug!("收到消息: {}", text);

                        // 解析收到的消息
                        if let Ok(data) = serde_json::from_str::<serde_json::Value>(&text) {
                            let msg_type = data.get("type").and_then(|t| t.as_str()).unwrap_or("");

                            if !connection_authenticated {
                                match msg_type {
                                    BROWSER_WS_AUTH_CLIENT_HELLO => {
                                        let Some(client_nonce) = browser_ws_nonce_from_value(&data, "clientNonce") else {
                                            log::warn!("[Browser WS] auth_client_hello 缺少有效 clientNonce");
                                            let _ = write.send(Message::Text(serde_json::json!({
                                                "type": BROWSER_WS_AUTH_FAILED,
                                                "reason": "invalid_client_nonce",
                                            }).to_string())).await;
                                            break;
                                        };
                                        let server_nonce = browser_ws_new_nonce();
                                        let response = browser_ws_server_hello_message(&token, client_nonce, &server_nonce);
                                        auth_client_nonce = Some(client_nonce.to_string());
                                        auth_server_nonce = Some(server_nonce);
                                        if let Err(e) = write.send(Message::Text(response.to_string())).await {
                                            log::error!("发送 WebSocket 认证 challenge 失败: {}", e);
                                            break;
                                        }
                                    }
                                    BROWSER_WS_AUTH_CLIENT_PROOF => {
                                        let Some(client_nonce) = auth_client_nonce.as_deref() else {
                                            log::warn!("[Browser WS] 未发送 challenge 就收到 client proof");
                                            let _ = write.send(Message::Text(serde_json::json!({
                                                "type": BROWSER_WS_AUTH_FAILED,
                                                "reason": "missing_challenge",
                                            }).to_string())).await;
                                            break;
                                        };
                                        let Some(server_nonce) = auth_server_nonce.as_deref() else {
                                            log::warn!("[Browser WS] 缺少 server nonce，无法校验 client proof");
                                            let _ = write.send(Message::Text(serde_json::json!({
                                                "type": BROWSER_WS_AUTH_FAILED,
                                                "reason": "missing_challenge",
                                            }).to_string())).await;
                                            break;
                                        };
                                        if !browser_ws_client_proof_matches(&data, &token, client_nonce, server_nonce) {
                                            log::warn!("[Browser WS] client proof 校验失败");
                                            let _ = write.send(Message::Text(serde_json::json!({
                                                "type": BROWSER_WS_AUTH_FAILED,
                                                "reason": "invalid_client_proof",
                                            }).to_string())).await;
                                            break;
                                        }
                                        connection_authenticated = true;
                                        log::info!("[Browser WS] 连接认证成功");
                                        if let Err(e) = write.send(Message::Text(serde_json::json!({
                                            "type": BROWSER_WS_AUTH_OK,
                                        }).to_string())).await {
                                            log::error!("发送 WebSocket 认证确认失败: {}", e);
                                            break;
                                        }
                                    }
                                    _ => {
                                        log::warn!("[Browser WS] 忽略未认证消息类型: {}", msg_type);
                                        if let Err(e) = write.send(Message::Text(serde_json::json!({
                                            "type": BROWSER_WS_AUTH_REQUIRED,
                                        }).to_string())).await {
                                            log::error!("发送 WebSocket 认证要求失败: {}", e);
                                            break;
                                        }
                                    }
                                }
                                continue;
                            }

                            // 任何来自浏览器扩展的消息（包括 ping、ai_completed 等）都应该更新 BROWSER_TX
                            // 这样确保 channel 始终指向最新的活跃连接
                            // 注意：每次收到 ping 都更新，确保重连后能正确刷新 sender
                            if msg_type == "ai_completed" || msg_type == "ping" {
                                if !is_browser_extension {
                                    is_browser_extension = true;
                                    log::info!("确认为浏览器扩展连接（消息类型: {}），更新 BROWSER_TX", msg_type);
                                }
                                // 每次心跳都刷新 sender，确保指向当前活跃连接
                                let mut tx = BROWSER_TX.write().await;
                                *tx = Some(browser_tx.clone());
                            }

                            match msg_type {
                                "ai_completed" => {
                                    // 浏览器扩展发来的 AI 完成事件

                                    // 获取 AI 回复内容
                                    log::info!("收到的完整数据: {:?}", data);
                                    let ai_response = data.get("aiResponse").and_then(|v| v.as_str()).unwrap_or("").to_string();
                                    log::info!("AI 回复内容长度: {}, 内容前100字符: {}", ai_response.len(), ai_response.chars().take(100).collect::<String>());

                                    let event = AiCompletionEvent {
                                        url: data.get("url").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                        title: data.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                                        site_name: data.get("siteName").and_then(|v| v.as_str()).unwrap_or("Unknown").to_string(),
                                        message_preview: ai_response, // 存储 AI 回复内容
                                        timestamp: chrono::Utc::now(),
                                        run_time: data.get("runTime").and_then(|v| v.as_u64()).map(|v| v as u32),
                                        think_time: data.get("thinkTime").and_then(|v| v.as_u64()).map(|v| v as u32),
                                        image_generated: data.get("imageGenerated").and_then(|v| v.as_bool()).unwrap_or(false),
                                        new_images: data.get("newImages").and_then(|v| v.as_u64()).map(|v| v as u32),
                                    };

                                    log::info!("AI 完成事件: {} - {}", event.site_name, event.url);
                                    let _ = event_tx.send(event);
                                }
                                "send_message" => {
                                    // 弹窗进程发来的消息，需要转发给浏览器扩展
                                    if let Some(message) = data.get("message").and_then(|v| v.as_str()) {
                                        log::info!("收到 send_message 请求，转发给浏览器扩展: {}", message);
                                        let msg = BrowserMessage {
                                            message_type: "send_message".to_string(),
                                            message: message.to_string(),
                                            tab_id: None,
                                        };
                                        // 通过 channel 转发给浏览器扩展连接
                                        let tx = BROWSER_TX.read().await;
                                        if let Some(ref sender) = *tx {
                                            if let Err(e) = sender.send(msg).await {
                                                log::error!("转发消息失败: {}", e);
                                            } else {
                                                log::info!("消息已转发到浏览器扩展");
                                            }
                                        } else {
                                            log::warn!("没有浏览器扩展连接，无法转发消息");
                                        }
                                    }
                                }
                                "ping" => {
                                    // 心跳消息，记录活跃状态（BROWSER_TX 已在上面更新）
                                    log::debug!("收到浏览器扩展心跳");
                                }
                                _ => {
                                    log::debug!("未知消息类型: {}", msg_type);
                                }
                            }
                        }

                        // 回复确认
                        if let Err(e) = write.send(Message::Text(r#"{"status":"ok"}"#.to_string())).await {
                            log::error!("发送回复失败: {}", e);
                            break;
                        }
                    }
                    Some(Ok(Message::Close(_))) => {
                        log::info!("WebSocket 连接关闭");
                        break;
                    }
                    Some(Ok(Message::Ping(data))) => {
                        let _ = write.send(Message::Pong(data)).await;
                    }
                    Some(Err(e)) => {
                        log::error!("WebSocket 错误: {}", e);
                        break;
                    }
                    None => break,
                    _ => {}
                }
            }
            // 发送消息到浏览器扩展
            browser_msg = browser_rx.recv() => {
                if let Some(msg) = browser_msg {
                    if !connection_authenticated {
                        log::warn!("[Browser WS] 跳过未认证连接上的浏览器消息发送");
                        continue;
                    }
                    let json = serde_json::json!({
                        "type": msg.message_type,
                        "message": msg.message,
                        "tabId": msg.tab_id,
                    });
                    log::info!("发送消息到浏览器: {}", json);
                    if let Err(e) = write.send(Message::Text(json.to_string())).await {
                        log::error!("发送消息到浏览器失败: {}", e);
                        break;
                    }
                }
            }
        }
    }

    // 连接关闭时，如果是浏览器扩展连接，清理 BROWSER_TX
    if is_browser_extension {
        let mut tx = BROWSER_TX.write().await;
        *tx = None;
        log::info!("浏览器扩展连接关闭，已清理 BROWSER_TX");
    }
}

/// 全局 WebSocket 服务器实例
static WS_SERVER: once_cell::sync::Lazy<Arc<RwLock<Option<WsServer>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(None)));

/// 返回 Browser WebSocket 服务器当前真实运行状态。
pub async fn browser_ws_server_running() -> bool {
    let global = WS_SERVER.read().await;
    match global.as_ref() {
        Some(server) => *server.running.read().await,
        None => false,
    }
}

/// 返回浏览器扩展当前是否存在已认证的活跃连接。
pub async fn browser_extension_connected() -> bool {
    BROWSER_TX.read().await.is_some()
}

/// 启动 WebSocket 服务器（如果已运行则返回现有的 sender）
pub async fn start_ws_server() -> Result<broadcast::Sender<AiCompletionEvent>> {
    let mut global = WS_SERVER.write().await;

    // 如果服务器已经在运行，返回现有的 sender
    if let Some(ref server) = *global {
        if *server.running.read().await {
            log::info!("WebSocket 服务器已在运行，返回现有 sender");
            return Ok(server.get_event_sender());
        }
    }

    // 创建新的服务器
    let server = WsServer::new();
    let event_tx = server.get_event_sender();
    server.start().await?;
    *global = Some(server);

    Ok(event_tx)
}

/// 停止 WebSocket 服务器
pub async fn stop_ws_server() {
    let mut global = WS_SERVER.write().await;
    if let Some(server) = global.take() {
        server.stop().await;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        browser_ws_client_hello_message, browser_ws_client_proof, browser_ws_client_proof_matches,
        browser_ws_client_proof_message, browser_ws_nonce_from_value,
        browser_ws_pairing_token_from_sources, browser_ws_pairing_token_in_config,
        browser_ws_server_proof, browser_ws_server_proof_matches,
        ensure_browser_ws_pairing_token_in_config, handle_connection, BROWSER_TX,
        BROWSER_WS_AUTH_OK, BROWSER_WS_AUTH_REQUIRED, BROWSER_WS_AUTH_SERVER_HELLO,
        BROWSER_WS_TOKEN_ENV,
    };
    use crate::config::AppConfig;
    use futures::{SinkExt, StreamExt};
    use serde_json::json;
    use tokio::net::TcpListener;
    use tokio::sync::{broadcast, mpsc};
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, http::HeaderValue, Message},
    };

    async fn spawn_test_browser_ws_server() -> std::net::SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test browser ws listener");
        let addr = listener.local_addr().expect("test listener address");
        let (event_tx, _) = broadcast::channel(10);
        let (browser_tx, browser_rx) = mpsc::channel(10);

        {
            let mut tx = BROWSER_TX.write().await;
            *tx = None;
        }

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept test ws client");
            handle_connection(stream, event_tx, browser_rx, browser_tx).await;
        });

        addr
    }

    #[test]
    fn browser_ws_token_source_prefers_env_and_falls_back_to_config() {
        let mut config = AppConfig::default();
        config.browser_ws_config.token = "stored-browser-token".to_string();

        assert_eq!(
            browser_ws_pairing_token_from_sources(Some(" env-browser-token "), &config)
                .expect("env token should be accepted"),
            "env-browser-token"
        );
        assert_eq!(
            browser_ws_pairing_token_from_sources(None, &config)
                .expect("config token should be accepted"),
            "stored-browser-token"
        );

        config.browser_ws_config.token.clear();
        assert!(
            browser_ws_pairing_token_from_sources(None, &config).is_err(),
            "Browser WS must remain fail-closed when no env or config token exists"
        );
    }

    #[test]
    fn browser_ws_pairing_token_generation_persists_in_config() {
        let mut config = AppConfig::default();
        assert!(browser_ws_pairing_token_in_config(&config).is_none());

        let first = ensure_browser_ws_pairing_token_in_config(&mut config)
            .expect("token generation should succeed");
        assert!(first.starts_with("bwst_"));
        assert_eq!(
            browser_ws_pairing_token_in_config(&config).as_deref(),
            Some(first.as_str())
        );

        let second = ensure_browser_ws_pairing_token_in_config(&mut config)
            .expect("existing token should be reused");
        assert_eq!(second, first);
    }

    #[test]
    fn browser_ws_client_proof_rejects_missing_and_wrong_values() {
        let token = "shared-secret";
        let client_nonce = "client-nonce-123456";
        let server_nonce = "server-nonce-123456";

        assert!(!browser_ws_client_proof_matches(
            &json!({
                "type": "auth_client_proof",
                "clientNonce": client_nonce,
                "serverNonce": server_nonce,
            }),
            token,
            client_nonce,
            server_nonce,
        ));

        assert!(!browser_ws_client_proof_matches(
            &json!({
                "type": "auth_client_proof",
                "clientNonce": client_nonce,
                "serverNonce": server_nonce,
                "clientProof": "not-the-proof",
            }),
            token,
            client_nonce,
            server_nonce,
        ));
    }

    #[test]
    fn browser_ws_client_proof_accepts_matching_hmac() {
        let token = "shared-secret";
        let client_nonce = "client-nonce-123456";
        let server_nonce = "server-nonce-123456";
        let proof = browser_ws_client_proof(token, client_nonce, server_nonce);

        assert!(browser_ws_client_proof_matches(
            &json!({
                "type": "auth_client_proof",
                "clientNonce": client_nonce,
                "serverNonce": server_nonce,
                "clientProof": proof,
            }),
            token,
            client_nonce,
            server_nonce,
        ));
    }

    #[test]
    fn browser_ws_server_proof_binds_both_nonces() {
        let token = "shared-secret";
        let client_nonce = "client-nonce-123456";
        let server_nonce = "server-nonce-123456";
        let proof = browser_ws_server_proof(token, client_nonce, server_nonce);

        assert!(browser_ws_server_proof_matches(
            &proof,
            token,
            client_nonce,
            server_nonce,
        ));
        assert!(!browser_ws_server_proof_matches(
            &proof,
            token,
            "other-client-nonce",
            server_nonce,
        ));
        assert!(!browser_ws_server_proof_matches(
            &proof,
            token,
            client_nonce,
            "other-server-nonce",
        ));
    }

    #[test]
    fn browser_ws_origin_policy_rejects_web_page_origins() {
        assert!(super::browser_ws_origin_is_trusted(None));
        assert!(super::browser_ws_origin_is_trusted(Some(
            "chrome-extension://abcdefghijklmnop"
        )));
        assert!(super::browser_ws_origin_is_trusted(Some(
            "moz-extension://12345678-1234-1234-1234-123456789abc"
        )));
        assert!(!super::browser_ws_origin_is_trusted(Some(
            "https://chatgpt.com"
        )));
        assert!(!super::browser_ws_origin_is_trusted(Some(
            "http://127.0.0.1:3000"
        )));
        assert!(!super::browser_ws_origin_is_trusted(Some("null")));
    }

    #[tokio::test]
    async fn browser_ws_rejects_web_page_origin_during_handshake() {
        std::env::set_var(BROWSER_WS_TOKEN_ENV, "test-browser-ws-token");
        let addr = spawn_test_browser_ws_server().await;
        let url = format!("ws://{}", addr);
        let mut request = url.into_client_request().expect("client request");
        request
            .headers_mut()
            .insert("Origin", HeaderValue::from_static("https://chatgpt.com"));

        let err = connect_async(request)
            .await
            .expect_err("web page origin should be rejected before websocket auth");

        assert!(
            err.to_string().contains("403") || err.to_string().contains("Forbidden"),
            "unexpected handshake error: {err}"
        );
    }

    #[tokio::test]
    async fn browser_ws_unauthenticated_ping_does_not_install_browser_sender() {
        std::env::set_var(BROWSER_WS_TOKEN_ENV, "test-browser-ws-token");
        let addr = spawn_test_browser_ws_server().await;
        let url = format!("ws://{}", addr);
        let (mut ws, _) = connect_async(&url).await.expect("connect test ws");

        ws.send(Message::Text(json!({ "type": "ping" }).to_string()))
            .await
            .expect("send unauthenticated ping");
        let response = ws
            .next()
            .await
            .expect("auth required response")
            .expect("valid ws frame");
        let Message::Text(response_text) = response else {
            panic!("expected text auth response");
        };
        let response_json: serde_json::Value =
            serde_json::from_str(&response_text).expect("auth response json");

        assert_eq!(
            response_json.get("type").and_then(|value| value.as_str()),
            Some(BROWSER_WS_AUTH_REQUIRED)
        );
        assert!(BROWSER_TX.read().await.is_none());
    }

    #[tokio::test]
    async fn browser_ws_authenticated_ping_installs_browser_sender() {
        let token = "test-browser-ws-token";
        let client_nonce = "client-nonce-123456";
        std::env::set_var(BROWSER_WS_TOKEN_ENV, token);
        let addr = spawn_test_browser_ws_server().await;
        let url = format!("ws://{}", addr);
        let (mut ws, _) = connect_async(&url).await.expect("connect test ws");

        ws.send(Message::Text(
            browser_ws_client_hello_message(client_nonce).to_string(),
        ))
        .await
        .expect("send auth hello");
        let server_hello = ws
            .next()
            .await
            .expect("server hello")
            .expect("valid server hello");
        let Message::Text(server_hello_text) = server_hello else {
            panic!("expected text server hello");
        };
        let server_hello_json: serde_json::Value =
            serde_json::from_str(&server_hello_text).expect("server hello json");
        assert_eq!(
            server_hello_json
                .get("type")
                .and_then(|value| value.as_str()),
            Some(BROWSER_WS_AUTH_SERVER_HELLO)
        );
        let server_nonce =
            browser_ws_nonce_from_value(&server_hello_json, "serverNonce").expect("server nonce");
        let server_proof = server_hello_json
            .get("serverProof")
            .and_then(|value| value.as_str())
            .expect("server proof");
        assert!(browser_ws_server_proof_matches(
            server_proof,
            token,
            client_nonce,
            server_nonce
        ));

        ws.send(Message::Text(
            browser_ws_client_proof_message(token, client_nonce, server_nonce).to_string(),
        ))
        .await
        .expect("send client proof");
        let auth_ok = ws.next().await.expect("auth ok").expect("valid auth ok");
        let Message::Text(auth_ok_text) = auth_ok else {
            panic!("expected text auth ok");
        };
        let auth_ok_json: serde_json::Value =
            serde_json::from_str(&auth_ok_text).expect("auth ok json");
        assert_eq!(
            auth_ok_json.get("type").and_then(|value| value.as_str()),
            Some(BROWSER_WS_AUTH_OK)
        );

        ws.send(Message::Text(json!({ "type": "ping" }).to_string()))
            .await
            .expect("send authenticated ping");
        let _ = ws.next().await.expect("ping ack").expect("valid ping ack");

        assert!(BROWSER_TX.read().await.is_some());
    }
}
