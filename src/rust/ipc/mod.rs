use crate::config::AppState;
use crate::conversation::{resolve_tree_route_key, ConversationManager, NodeMetadata, NodeType};
use crate::mcp::tools::checkpoint::links::{append_checkpoint_link, build_checkpoint_link_entry};
use crate::utils::append_timeline_debug_log;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use uuid::Uuid;

const IPC_ADDR: &str = "127.0.0.1:37101";
const MAX_FRAME_SIZE: usize = 1024 * 1024;
const SHOW_MAIN_WINDOW_CONTROL: &str = "show_main_window";
const CONVERSATION_ROUTE_ID_KEYS: &[&str] = &[
    "codex_thread_id",
    "codexThreadId",
    "conversation_route_id",
    "conversationRouteId",
    "conversation_id",
    "conversationId",
    "timeline_route_id",
    "timelineRouteId",
    "timelineTreeId",
];
const CONVERSATION_ROUTE_CONTAINER_KEYS: &[&str] =
    &["_meta", "meta", "metadata", "request", "payload"];

fn normalize_non_empty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToOwned::to_owned)
}

fn string_field_from_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .and_then(|value| normalize_non_empty(Some(value)))
}

fn extract_conversation_route_id_from_request(req: &Value) -> Option<String> {
    crate::mcp::codex_deeplink::extract_codex_thread_id_from_value(req)
        .or_else(|| {
            let object = req.as_object()?;
            CONVERSATION_ROUTE_ID_KEYS
                .iter()
                .find_map(|key| string_field_from_value(object.get(*key)))
        })
        .or_else(|| {
            let object = req.as_object()?;
            for container_key in CONVERSATION_ROUTE_CONTAINER_KEYS {
                let Some(container) = object.get(*container_key) else {
                    continue;
                };
                if let Some(route_id) = extract_conversation_route_id_from_request(container) {
                    return Some(route_id);
                }
            }

            None
        })
}

fn parse_predefined_options(req: &Value) -> Option<Vec<String>> {
    req.get("predefined_options")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(ToOwned::to_owned))
                .collect::<Vec<String>>()
        })
        .filter(|items| !items.is_empty())
}

async fn record_request_node(app_handle: &AppHandle, req: &Value, project_path: &str) {
    let content = req
        .get("message")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .unwrap_or("");
    let raw_request_id = req
        .get("id")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    // Claude Code 调用方（P-2026-9373）：只认 `claude-code:<sessionId>`，不借 Codex 线程 / live goal。
    let conversation_route_id = if crate::mcp::caller_identity::is_claude_code_caller_request(req) {
        crate::mcp::caller_identity::claude_timeline_route_from_request(req)
    } else {
        extract_conversation_route_id_from_request(req).or_else(|| {
            crate::ui::live_goal::live_goal_codex_thread_id_for_project_with_app(
                Some(app_handle),
                Some(project_path),
            )
        })
    };
    let route_id = conversation_route_id.as_deref().or(raw_request_id);
    let request_key = resolve_tree_route_key(route_id, Some(project_path));
    eprintln!(
        "[Conversation][IPC] record_request_node start: request_key={:?}, raw_request_id={:?}, conversation_route_id={:?}, project_path={}, content_len={}",
        request_key,
        raw_request_id,
        conversation_route_id,
        project_path,
        content.chars().count()
    );
    append_timeline_debug_log(
        "rust/ipc::record_request_node:start",
        serde_json::json!({
            "caller_client": req.get("caller_client"),
            "request_key": request_key,
            "raw_request_id": raw_request_id,
            "conversation_route_id": conversation_route_id,
            "project_path": project_path,
            "content_len": content.chars().count(),
        }),
    );
    if content.is_empty() {
        log::warn!("[Conversation] IPC 请求 message 为空，跳过 assistant 节点记录");
        eprintln!(
            "[Conversation][IPC] record_request_node skipped: empty message (request_key={:?})",
            request_key
        );
        append_timeline_debug_log(
            "rust/ipc::record_request_node:skipped",
            serde_json::json!({
                "reason": "empty_message",
                "request_key": request_key,
                "project_path": project_path,
            }),
        );
        return;
    }
    log::info!(
        "[Conversation] IPC 开始记录 assistant 节点: request_key={:?}, project_path={}, message_len={}",
        request_key,
        project_path,
        content.chars().count()
    );

    let Some(manager) = app_handle.try_state::<Arc<ConversationManager>>() else {
        log::warn!("[Conversation] IPC 无法获取 ConversationManager，跳过 assistant 节点记录");
        eprintln!(
            "[Conversation][IPC] record_request_node skipped: ConversationManager unavailable"
        );
        append_timeline_debug_log(
            "rust/ipc::record_request_node:skipped",
            serde_json::json!({
                "reason": "conversation_manager_unavailable",
                "request_key": request_key,
                "project_path": project_path,
            }),
        );
        return;
    };

    let tree_id = manager
        .get_or_create_tree_for_route(route_id, Some(project_path))
        .await;
    let parent_id = manager.get_current_node_id(&tree_id).await;
    append_timeline_debug_log(
        "rust/ipc::record_request_node:resolved_tree_context",
        serde_json::json!({
            "tree_id": tree_id,
            "parent_id": parent_id,
            "request_key": request_key,
            "raw_request_id": raw_request_id,
            "conversation_route_id": conversation_route_id,
            "project_path": project_path,
        }),
    );
    let metadata = NodeMetadata {
        conversation_id: Some(tree_id.clone()),
        project_path: Some(project_path.to_string()),
        predefined_options: parse_predefined_options(req),
        selected_option: None,
        images: None,
        link_url: req
            .get("link_url")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
        link_title: req
            .get("link_title")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
        request_id: request_key.clone(),
        run_id: None,
        generation: None,
        stale_of: None,
        superseded_by: None,
        checkpoint_id: req
            .get("checkpoint_id")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
        checkpoint_commit: req
            .get("checkpoint_commit")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
        checkpoint_message: req
            .get("checkpoint_message")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
        source: Some("ipc_request".to_string()),
    };

    match manager
        .add_node(
            &tree_id,
            parent_id.clone(),
            NodeType::Assistant,
            content.to_string(),
            req.get("is_markdown")
                .and_then(|value| value.as_bool())
                .unwrap_or(true),
            metadata,
        )
        .await
    {
        Ok(node_id) => {
            log::info!(
                "[Conversation] IPC assistant 节点记录成功: tree_id={}, node_id={}, request_key={:?}",
                tree_id,
                node_id,
                request_key
            );
            let request_id_for_link = request_key.as_deref().unwrap_or("");
            if let Some(link) = build_checkpoint_link_entry(
                project_path,
                request_id_for_link,
                None,
                req.get("checkpoint_id").and_then(|value| value.as_str()),
                req.get("checkpoint_commit")
                    .and_then(|value| value.as_str()),
                req.get("checkpoint_message")
                    .and_then(|value| value.as_str()),
                &tree_id,
                &node_id,
                NodeType::Assistant.as_key(),
                "ipc_request",
            ) {
                append_checkpoint_link(link);
            }
            eprintln!(
                "[Conversation][IPC] record_request_node success: tree_id={}, node_id={}, request_key={:?}",
                tree_id, node_id, request_key
            );
            append_timeline_debug_log(
                "rust/ipc::record_request_node:success",
                serde_json::json!({
                    "tree_id": tree_id,
                    "node_id": node_id,
                    "parent_id": parent_id,
                    "request_key": request_key,
                    "project_path": project_path,
                }),
            );
            if let Err(err) = app_handle.emit(
                "conversation-node-recorded",
                serde_json::json!({
                    "tree_id": tree_id,
                    "conversation_id": tree_id,
                    "node_id": node_id,
                    "parent_id": parent_id,
                    "node_type": "assistant",
                    "request_key": request_key.clone(),
                    "request_id": route_id,
                    "actual_request_id": raw_request_id,
                    "conversation_route_id": conversation_route_id,
                    "project_path": project_path,
                    "source": "ipc_request",
                }),
            ) {
                log::warn!(
                    "[Conversation] IPC assistant 节点事件广播失败: tree_id={}, error={}",
                    tree_id,
                    err
                );
            }
        }
        Err(err) => {
            log::warn!("[Conversation] IPC 记录 assistant 节点失败: {}", err);
            eprintln!("[Conversation][IPC] record_request_node failed: {}", err);
            append_timeline_debug_log(
                "rust/ipc::record_request_node:failed",
                serde_json::json!({
                    "reason": "add_node_failed",
                    "tree_id": tree_id,
                    "parent_id": parent_id,
                    "request_key": request_key,
                    "project_path": project_path,
                    "error": err.to_string(),
                }),
            );
        }
    }
}

async fn read_frame(stream: &mut TcpStream) -> Result<Vec<u8>, String> {
    let mut len_buf = [0u8; 4];
    stream
        .read_exact(&mut len_buf)
        .await
        .map_err(|e| format!("读取长度失败: {}", e))?;

    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 {
        return Err("空帧".to_string());
    }
    if len > MAX_FRAME_SIZE {
        return Err(format!("帧过大: {}", len));
    }

    let mut buf = vec![0u8; len];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|e| format!("读取帧失败: {}", e))?;
    Ok(buf)
}

async fn write_frame(stream: &mut TcpStream, bytes: &[u8]) -> Result<(), String> {
    let len: u32 = bytes.len().try_into().map_err(|_| "帧过大".to_string())?;

    stream
        .write_all(&len.to_be_bytes())
        .await
        .map_err(|e| format!("写入长度失败: {}", e))?;
    stream
        .write_all(bytes)
        .await
        .map_err(|e| format!("写入帧失败: {}", e))?;
    stream
        .flush()
        .await
        .map_err(|e| format!("flush 失败: {}", e))?;
    Ok(())
}

pub async fn start_ipc_server(app_handle: AppHandle) -> Result<(), String> {
    let listener = TcpListener::bind(IPC_ADDR)
        .await
        .map_err(|e| format!("IPC 监听失败 ({}): {}", IPC_ADDR, e))?;
    log::info!("[IPC] 服务已启动: {}", IPC_ADDR);

    loop {
        let (stream, _addr) = listener
            .accept()
            .await
            .map_err(|e| format!("IPC accept 失败: {}", e))?;

        if let Err(e) = handle_connection(app_handle.clone(), stream).await {
            log::warn!("[IPC] connection error: {}", e);
        }
    }
}

async fn handle_connection(app_handle: AppHandle, mut stream: TcpStream) -> Result<(), String> {
    let frame = read_frame(&mut stream).await?;
    let req: Value = serde_json::from_slice(&frame).map_err(|e| format!("解析请求失败: {}", e))?;
    log::info!("[IPC] 收到原始请求帧: bytes={}", frame.len());

    if is_show_main_window_control(&req) {
        crate::ui::commands::activate_app_window(app_handle).await?;
        write_frame(
            &mut stream,
            serde_json::json!({ "ok": true, "action": SHOW_MAIN_WINDOW_CONTROL })
                .to_string()
                .as_bytes(),
        )
        .await?;
        log::info!("[IPC] 已显示并聚焦主窗口");
        return Ok(());
    }

    // 尝试提取 project_path，用于多项目路由（规范化为绝对路径）
    let project_path = {
        let raw = req
            .get("project_path")
            .and_then(|v| v.as_str())
            .unwrap_or("Unknown")
            .to_string();
        if raw != "Unknown" {
            let p = std::path::Path::new(&raw);
            if p.is_relative() {
                match std::fs::canonicalize(p) {
                    Ok(abs) => abs.to_string_lossy().to_string(),
                    Err(_) => {
                        // canonicalize 失败时尝试用当前工作目录拼接
                        if let Ok(cwd) = std::env::current_dir() {
                            cwd.join(p).to_string_lossy().to_string()
                        } else {
                            raw
                        }
                    }
                }
            } else {
                raw
            }
        } else {
            raw
        }
    };

    let normalized_request_id = normalize_non_empty(req.get("id").and_then(|v| v.as_str()));
    let request_id = normalized_request_id.unwrap_or_else(|| {
        let generated = format!("ipc-{}", Uuid::new_v4());
        log::warn!(
            "[IPC] 请求未携带有效 id，已生成兜底 request_id: {} (project_path={})",
            generated,
            project_path
        );
        generated
    });

    // 将规范化后的 project_path/request_id 更新回请求 JSON
    let req = {
        let mut r = req;
        if let Some(obj) = r.as_object_mut() {
            obj.insert(
                "project_path".to_string(),
                serde_json::Value::String(project_path.clone()),
            );
            obj.insert(
                "id".to_string(),
                serde_json::Value::String(request_id.clone()),
            );
        }
        r
    };

    let channel_key = request_id.clone();
    log::info!(
        "[IPC] 收到请求并建立响应通道: channel_key={}, project_path={}",
        channel_key,
        project_path
    );

    record_request_node(&app_handle, &req, &project_path).await;

    let state = app_handle.state::<AppState>();

    // 先注册 ready/response channel，再 emit 给前端（避免竞态：action 先到但通道还没建好）
    let (ready_rx, response_rx) = {
        let mut ready_channels = state
            .request_ready_channels
            .lock()
            .map_err(|e| format!("获取 ready 通道失败: {}", e))?;
        let mut response_channels = state
            .response_channels
            .lock()
            .map_err(|e| format!("获取响应通道失败: {}", e))?;

        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
        let (response_tx, response_rx) = tokio::sync::oneshot::channel::<String>();
        ready_channels.insert(channel_key.clone(), ready_tx);
        response_channels.insert(channel_key.clone(), response_tx);
        (ready_rx, response_rx)
    };

    // 发送请求到所有窗口，由前端根据 project_path 决定谁来处理
    if let Err(err) = app_handle.emit("mcp-request", req) {
        log::warn!("[IPC] 发送 mcp-request 事件失败: {}", err);
    } else {
        log::info!("[IPC] mcp-request 事件发送成功: {}", channel_key);
    }

    let ready_timeout = Duration::from_millis(1500);
    match timeout(ready_timeout, ready_rx).await {
        Ok(Ok(())) => {
            log::info!("[IPC] 前端已确认接收请求: channel_key={}", channel_key);
        }
        Ok(Err(_)) => {
            log::warn!("[IPC] 前端 ready 通道提前关闭: channel_key={}", channel_key);
            let mut ready_channels = state
                .request_ready_channels
                .lock()
                .map_err(|e| format!("获取 ready 通道失败: {}", e))?;
            ready_channels.remove(&channel_key);
            let mut response_channels = state
                .response_channels
                .lock()
                .map_err(|e| format!("获取响应通道失败: {}", e))?;
            response_channels.remove(&channel_key);
            return Err("前端未确认接收 MCP 请求".to_string());
        }
        Err(_) => {
            log::warn!(
                "[IPC] 等待前端确认超时: channel_key={}, timeout_ms={}",
                channel_key,
                ready_timeout.as_millis()
            );
            let mut ready_channels = state
                .request_ready_channels
                .lock()
                .map_err(|e| format!("获取 ready 通道失败: {}", e))?;
            ready_channels.remove(&channel_key);
            let mut response_channels = state
                .response_channels
                .lock()
                .map_err(|e| format!("获取响应通道失败: {}", e))?;
            response_channels.remove(&channel_key);
            return Err("前端未在超时内确认接收 MCP 请求".to_string());
        }
    }

    let response_result = response_rx.await.map_err(|_| "响应通道已关闭".to_string());
    let resp = response_result
        .as_ref()
        .cloned()
        .unwrap_or_else(|e| e.clone());
    log::info!(
        "[IPC] 响应通道完成: channel_key={}, response_len={}",
        channel_key,
        resp.len()
    );

    // 清理已完成的通道
    {
        let mut ready_channels = state
            .request_ready_channels
            .lock()
            .map_err(|e| format!("获取 ready 通道失败: {}", e))?;
        ready_channels.remove(&channel_key);
        let mut channels = state
            .response_channels
            .lock()
            .map_err(|e| format!("获取响应通道失败: {}", e))?;
        channels.remove(&channel_key);
    }

    write_frame(&mut stream, resp.as_bytes()).await?;
    log::info!("[IPC] 响应已回写: channel_key={}", channel_key);

    Ok(())
}

fn is_show_main_window_control(request: &Value) -> bool {
    request.get("action").and_then(Value::as_str) == Some(SHOW_MAIN_WINDOW_CONTROL)
}

pub async fn request_show_main_window() -> Result<(), String> {
    let mut stream = timeout(Duration::from_millis(500), TcpStream::connect(IPC_ADDR))
        .await
        .map_err(|_| format!("连接常驻 iterate 超时 ({IPC_ADDR})"))?
        .map_err(|e| format!("连接常驻 iterate 失败 ({IPC_ADDR}): {e}"))?;
    let request = serde_json::json!({ "action": SHOW_MAIN_WINDOW_CONTROL });
    let bytes = serde_json::to_vec(&request).map_err(|e| format!("序列化显示窗口请求失败: {e}"))?;
    write_frame(&mut stream, &bytes).await?;

    let response = timeout(Duration::from_secs(2), read_frame(&mut stream))
        .await
        .map_err(|_| "等待主窗口激活响应超时".to_string())??;
    let response: Value =
        serde_json::from_slice(&response).map_err(|e| format!("解析主窗口激活响应失败: {e}"))?;
    if response.get("ok").and_then(Value::as_bool) == Some(true) {
        Ok(())
    } else {
        Err(format!("主窗口拒绝激活请求: {response}"))
    }
}

pub async fn forward_request_to_running_app(request: Value) -> Result<String, String> {
    let mut stream = TcpStream::connect(IPC_ADDR)
        .await
        .map_err(|e| format!("连接常驻 iterate 失败 ({}): {}", IPC_ADDR, e))?;

    let bytes = serde_json::to_vec(&request).map_err(|e| format!("序列化请求失败: {}", e))?;
    write_frame(&mut stream, &bytes).await?;

    let resp_bytes = read_frame(&mut stream).await?;
    let resp = String::from_utf8(resp_bytes).map_err(|e| format!("响应不是 UTF-8: {}", e))?;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_codex_thread_route_from_nested_meta() {
        let req = serde_json::json!({
            "id": "serve-1",
            "metadata": {
                "x-codex-turn-metadata": {
                    "thread_id": "019eca23-a6a1-7683-9674-4d37b577033c"
                }
            }
        });

        assert_eq!(
            extract_conversation_route_id_from_request(&req).as_deref(),
            Some("019eca23-a6a1-7683-9674-4d37b577033c")
        );
    }

    #[test]
    fn extracts_explicit_timeline_route_from_request() {
        let req = serde_json::json!({
            "id": "serve-1",
            "timeline_route_id": "thread-1"
        });

        assert_eq!(
            extract_conversation_route_id_from_request(&req).as_deref(),
            Some("thread-1")
        );
    }

    #[test]
    fn claude_caller_request_does_not_resolve_codex_route() {
        let req = serde_json::json!({
            "id": "serve-1",
            "caller_client": "claude_code",
            "caller_session_id": "7dbca76e-c67a-4cee-ad28-52ad4d90bd28",
            "timeline_route_id": "claude-code:7dbca76e-c67a-4cee-ad28-52ad4d90bd28",
            "codex_thread_id": null
        });

        assert!(crate::mcp::caller_identity::is_claude_code_caller_request(
            &req
        ));
        assert_eq!(
            crate::mcp::caller_identity::claude_timeline_route_from_request(&req).as_deref(),
            Some("claude-code:7dbca76e-c67a-4cee-ad28-52ad4d90bd28")
        );
        // 旧的通用提取也只会拿到 Claude route，不会落到 Codex 线程。
        assert_eq!(
            extract_conversation_route_id_from_request(&req).as_deref(),
            Some("claude-code:7dbca76e-c67a-4cee-ad28-52ad4d90bd28")
        );
    }

    #[test]
    fn recognizes_only_the_explicit_show_main_window_control_frame() {
        assert!(is_show_main_window_control(
            &serde_json::json!({ "action": "show_main_window" })
        ));
        assert!(!is_show_main_window_control(
            &serde_json::json!({ "action": "show_window" })
        ));
        assert!(!is_show_main_window_control(
            &serde_json::json!({ "message": "show_main_window" })
        ));
    }
}
