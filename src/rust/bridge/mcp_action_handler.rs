use super::mcp_action_delivery::try_write_serve_response_file;
use super::mcp_action_payload::{
    build_goal_payload_parts, build_goal_submit_prompt, normalize_mcp_action_images,
};
use super::mcp_action_recording::{
    broadcast_latest_timeline_node, record_mcp_action_response_node,
};
use super::room_submit::{RoomDeliveryAttempt, RoomDeliveryResult};
use super::ws::{
    bridge_debug_log, broadcast_custom_prompt_config_changed,
    cleanup_completed_session_by_request_id, MCP_ACTION_CACHE_TTL_SECS,
};
use crate::conversation::ConversationManager;
use tauri::{AppHandle, Emitter, Manager};

pub(super) async fn try_handle_mcp_action_directly(
    app_handle: &AppHandle,
    project_path: &str,
    request_id: Option<&str>,
    timeline_route_id: Option<&str>,
    payload: &serde_json::Value,
) -> RoomDeliveryResult {
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("");
    eprintln!(
        "[Bridge] try_handle_mcp_action_directly start: action={}, project_path={}, request_id={:?}",
        action, project_path, request_id
    );
    bridge_debug_log(&format!(
        "try_handle_mcp_action_directly: action={}, project_path={}, request_id={:?}",
        action, project_path, request_id
    ));

    let response = match action {
        "submit" => {
            let user_input = payload
                .get("user_input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let selected_options = payload
                .get("selected_options")
                .cloned()
                .unwrap_or(serde_json::json!([]));
            let normalized_images = normalize_mcp_action_images(payload);

            serde_json::json!({
                "user_input": user_input,
                "selected_options": selected_options,
                "images": normalized_images,
                "project_path": project_path,
                "metadata": {
                    "timestamp": chrono::Local::now().to_rfc3339(),
                    "request_id": request_id,
                    "timeline_route_id": timeline_route_id,
                    "conversation_route_id": timeline_route_id,
                    "source": "web_bridge_rust_direct",
                }
            })
        }
        "continue" => {
            serde_json::json!({
                "user_input": null,
                "selected_options": ["继续"],
                "images": [],
                "project_path": project_path,
                "metadata": {
                    "timestamp": chrono::Local::now().to_rfc3339(),
                    "request_id": request_id,
                    "timeline_route_id": timeline_route_id,
                    "conversation_route_id": timeline_route_id,
                    "source": "web_bridge_continue",
                }
            })
        }
        "goal" | "goal_start" => {
            let (goal_text, goal_title, selected_options) = build_goal_payload_parts(payload);
            let normalized_images = normalize_mcp_action_images(payload);
            if goal_text.is_empty() {
                log::warn!("[Bridge] goal action missing input and selected options");
                return RoomDeliveryResult::rejected(RoomDeliveryAttempt::internal(
                    "rust_direct_internal",
                    project_path,
                    request_id,
                    false,
                    Some("goal_input_missing"),
                ));
            }

            serde_json::json!({
                "user_input": build_goal_submit_prompt(&goal_text),
                "selected_options": selected_options,
                "images": normalized_images,
                "project_path": project_path,
                "metadata": {
                    "timestamp": chrono::Local::now().to_rfc3339(),
                    "request_id": request_id,
                    "timeline_route_id": timeline_route_id,
                    "conversation_route_id": timeline_route_id,
                    "source": "web_bridge_goal_submit",
                    "mode": "goalrun_takeover",
                    "goal_title": goal_title,
                }
            })
        }
        "loop" | "loop_start" => {
            let user_input = payload
                .get("user_input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let selected_options = payload
                .get("selected_options")
                .cloned()
                .unwrap_or(serde_json::json!([]));
            serde_json::json!({
                "user_input": user_input,
                "selected_options": selected_options,
                "images": [],
                "project_path": project_path,
                "metadata": {
                    "timestamp": chrono::Local::now().to_rfc3339(),
                    "request_id": request_id,
                    "timeline_route_id": timeline_route_id,
                    "conversation_route_id": timeline_route_id,
                    "source": "web_bridge_loop_start",
                }
            })
        }
        "enhance" => {
            let user_input = payload
                .get("user_input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "user_input": user_input,
                "selected_options": ["增强"],
                "images": [],
                "project_path": project_path,
                "metadata": {
                    "timestamp": chrono::Local::now().to_rfc3339(),
                    "request_id": request_id,
                    "timeline_route_id": timeline_route_id,
                    "conversation_route_id": timeline_route_id,
                    "source": "web_bridge_enhance",
                }
            })
        }
        "send_to_browser_ai" => {
            let message = payload
                .get("user_input")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();

            if message.is_empty() {
                log::warn!("[Bridge] send_to_browser_ai ignored empty message");
                return RoomDeliveryResult::delivered(RoomDeliveryAttempt::internal(
                    "rust_direct_internal",
                    project_path,
                    request_id,
                    true,
                    None,
                ));
            }

            match crate::browser::send_to_browser(message).await {
                Ok(_) => log::info!("[Bridge] send_to_browser_ai sent to browser extension"),
                Err(err) => log::error!("[Bridge] send_to_browser_ai failed: {}", err),
            }
            return RoomDeliveryResult::delivered(RoomDeliveryAttempt::internal(
                "rust_direct_internal",
                project_path,
                request_id,
                true,
                None,
            ));
        }
        "cancel" => serde_json::json!("CANCELLED"),
        "update_conditional_state" => {
            let prompt_id = payload
                .get("promptId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let new_state = payload
                .get("newState")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if let Some(state) = app_handle.try_state::<crate::config::AppState>() {
                if let Ok(mut config) = state.config.lock() {
                    if let Some(prompt) = config
                        .custom_prompt_config
                        .prompts
                        .iter_mut()
                        .find(|p| p.id == prompt_id)
                    {
                        prompt.current_state = new_state;
                        prompt.updated_at = chrono::Utc::now().to_rfc3339();
                        log::info!(
                            "[Bridge] update_conditional_state: {} = {}",
                            prompt_id,
                            new_state
                        );
                    }
                }
                let _ = crate::config::storage::save_config(&state, app_handle).await;
                broadcast_custom_prompt_config_changed(app_handle);
            }
            return RoomDeliveryResult::delivered(RoomDeliveryAttempt::internal(
                "rust_direct_internal",
                project_path,
                request_id,
                true,
                None,
            ));
        }
        "update_conditional_active" => {
            let prompt_id = payload
                .get("promptId")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_active = payload
                .get("isActive")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            if let Some(state) = app_handle.try_state::<crate::config::AppState>() {
                if let Ok(mut config) = state.config.lock() {
                    if let Some(prompt) = config
                        .custom_prompt_config
                        .prompts
                        .iter_mut()
                        .find(|p| p.id == prompt_id)
                    {
                        prompt.is_active = is_active;
                        prompt.updated_at = chrono::Utc::now().to_rfc3339();
                        log::info!(
                            "[Bridge] update_conditional_active: {} = {}",
                            prompt_id,
                            is_active
                        );
                    }
                }
                let _ = crate::config::storage::save_config(&state, app_handle).await;
                broadcast_custom_prompt_config_changed(app_handle);
            }
            return RoomDeliveryResult::delivered(RoomDeliveryAttempt::internal(
                "rust_direct_internal",
                project_path,
                request_id,
                true,
                None,
            ));
        }
        _ => {
            log::warn!("[Bridge] 未知的 mcp_action: {}", action);
            return RoomDeliveryResult::rejected(RoomDeliveryAttempt::internal(
                "rust_direct_internal",
                project_path,
                request_id,
                false,
                Some("unsupported_mcp_action"),
            ));
        }
    };

    if let Some(state) = app_handle.try_state::<crate::config::AppState>() {
        let response_str = match serde_json::to_string(&response) {
            Ok(s) => s,
            Err(e) => {
                log::error!("[Bridge] 序列化 mcp_action 响应失败: {}", e);
                eprintln!(
                    "[Bridge] try_handle_mcp_action_directly failed: serialize response error={}",
                    e
                );
                return RoomDeliveryResult::rejected(RoomDeliveryAttempt::internal(
                    "rust_direct_internal",
                    project_path,
                    request_id,
                    false,
                    Some("response_serialize_failed"),
                ));
            }
        };

        let lookup_key = request_id.unwrap_or(project_path).to_string();
        let (sender, available_channel_count) = {
            if let Ok(mut channels) = state.response_channels.lock() {
                let keys: Vec<String> = channels.keys().cloned().collect();
                bridge_debug_log(&format!(
                    "response_channels keys: {:?}, looking for: {} (request_id: {:?})",
                    keys, lookup_key, request_id
                ));
                let available_count = keys.len();
                (channels.remove(&lookup_key), available_count)
            } else {
                log::error!("[Bridge] 获取 response_channels 锁失败");
                return RoomDeliveryResult::rejected(RoomDeliveryAttempt::internal(
                    "response_channel",
                    project_path,
                    request_id,
                    false,
                    Some("response_channels_lock_failed"),
                ));
            }
        };

        if let Some(sender) = sender {
            if sender.send(response_str).is_err() {
                bridge_debug_log(&format!(
                    "❌ sender.send 失败 (request_id: {:?}, project: {})",
                    request_id, project_path
                ));
                eprintln!(
                    "[Bridge] try_handle_mcp_action_directly failed: sender.send (request_id={:?}, project={})",
                    request_id, project_path
                );
                return RoomDeliveryResult::rejected(RoomDeliveryAttempt::response_channel(
                    project_path,
                    request_id,
                    &lookup_key,
                    false,
                    Some("response_channel_send_failed"),
                    available_channel_count,
                ));
            }

            bridge_debug_log(&format!(
                "✅ 响应已发送 (request_id: {:?}, project: {})",
                request_id, project_path
            ));
            eprintln!(
                "[Bridge] try_handle_mcp_action_directly routed: request_id={:?}, project={}",
                request_id, project_path
            );

            crate::ui::live_goal::apply_live_goal_intent_from_response(
                Some(app_handle), &response, Some(project_path), request_id,
            );

            if let Err(err) = record_mcp_action_response_node(
                app_handle,
                action,
                &response,
                project_path,
                request_id,
                timeline_route_id,
                "bridge_ws_fallback",
            )
            .await
            {
                log::warn!("[Conversation] Bridge fallback 记录用户节点失败: {}", err);
                eprintln!(
                    "[Bridge] try_handle_mcp_action_directly record_mcp_action_response_node failed: {}",
                    err
                );
            }
            if let Some(rid) = request_id {
                cleanup_completed_session_by_request_id(rid, "mcp-action-direct-cleanup").await;
            }
            if let Err(e) = app_handle.emit(
                "bridge-mcp-action-handled",
                serde_json::json!({
                    "project_path": project_path,
                    "action": action,
                }),
            ) {
                log::debug!(
                    "[Bridge] 通知前端 action 已处理失败（可能窗口未打开）: {}",
                    e
                );
            }
            RoomDeliveryResult::delivered(RoomDeliveryAttempt::response_channel(
                project_path,
                request_id,
                &lookup_key,
                true,
                None,
                available_channel_count,
            ))
        } else {
            bridge_debug_log(&format!("⚠️ 未找到项目 {} 的响应通道", project_path));
            eprintln!(
                "[Bridge] try_handle_mcp_action_directly route miss: request_id={:?}, project={}",
                request_id, project_path
            );
            let channel_attempt = RoomDeliveryAttempt::response_channel(
                project_path,
                request_id,
                &lookup_key,
                false,
                Some("response_channel_missing"),
                available_channel_count,
            );
            let serve_attempt = try_write_serve_response_file(
                request_id,
                project_path,
                &response_str,
                MCP_ACTION_CACHE_TTL_SECS,
                &bridge_debug_log,
            );
            if serve_attempt.delivered {
                crate::ui::live_goal::apply_live_goal_intent_from_response(
                    Some(app_handle), &response, Some(project_path), request_id,
                );
                if let Err(err) = record_mcp_action_response_node(
                    app_handle,
                    action,
                    &response,
                    project_path,
                    request_id,
                    timeline_route_id,
                    "bridge_serve_response_file",
                )
                .await
                {
                    log::warn!(
                        "[Conversation] Bridge serve 文件响应记录用户节点失败: {}",
                        err
                    );
                    eprintln!(
                        "[Bridge] try_handle_mcp_action_directly record serve response node failed: {}",
                        err
                    );
                }
                if let Some(rid) = request_id {
                    cleanup_completed_session_by_request_id(rid, "mcp-action-serve-file-cleanup")
                        .await;
                }
                if let Err(e) = app_handle.emit(
                    "bridge-mcp-action-handled",
                    serde_json::json!({
                        "project_path": project_path,
                        "action": action,
                    }),
                ) {
                    log::debug!(
                        "[Bridge] 通知前端 action 已处理失败（可能窗口未打开）: {}",
                        e
                    );
                }
                return RoomDeliveryResult::from_attempts(vec![channel_attempt, serve_attempt]);
            }
            RoomDeliveryResult::rejected_with_attempts(vec![channel_attempt, serve_attempt])
        }
    } else {
        log::warn!("[Bridge] AppState 不可用");
        eprintln!("[Bridge] try_handle_mcp_action_directly failed: AppState unavailable");
        RoomDeliveryResult::rejected(RoomDeliveryAttempt::internal(
            "rust_direct_internal",
            project_path,
            request_id,
            false,
            Some("app_state_unavailable"),
        ))
    }
}

pub(super) async fn try_handle_mcp_action_headless(
    project_path: &str,
    request_id: Option<&str>,
    timeline_route_id: Option<&str>,
    payload: &serde_json::Value,
) -> RoomDeliveryResult {
    let action = payload.get("action").and_then(|v| v.as_str()).unwrap_or("");
    bridge_debug_log(&format!(
        "try_handle_mcp_action_headless: action={}, project_path={}, request_id={:?}",
        action, project_path, request_id
    ));

    let response = match action {
        "submit" => {
            let user_input = payload
                .get("user_input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let selected_options = payload
                .get("selected_options")
                .cloned()
                .unwrap_or(serde_json::json!([]));
            let normalized_images = normalize_mcp_action_images(payload);

            serde_json::json!({
                "user_input": user_input,
                "selected_options": selected_options,
                "images": normalized_images,
                "project_path": project_path,
                    "metadata": {
                        "timestamp": chrono::Local::now().to_rfc3339(),
                        "request_id": request_id,
                        "timeline_route_id": timeline_route_id,
                        "conversation_route_id": timeline_route_id,
                        "source": "web_bridge_rust_direct",
                    }
            })
        }
        "continue" => serde_json::json!({
            "user_input": null,
            "selected_options": ["继续"],
            "images": [],
            "project_path": project_path,
                "metadata": {
                    "timestamp": chrono::Local::now().to_rfc3339(),
                    "request_id": request_id,
                    "timeline_route_id": timeline_route_id,
                    "conversation_route_id": timeline_route_id,
                    "source": "web_bridge_continue",
                }
        }),
        "goal" | "goal_start" => {
            let (goal_text, goal_title, selected_options) = build_goal_payload_parts(payload);
            let normalized_images = normalize_mcp_action_images(payload);
            if goal_text.is_empty() {
                log::warn!("[Bridge] bridge-only goal action missing input and selected options");
                return RoomDeliveryResult::rejected(RoomDeliveryAttempt::internal(
                    "bridge_only_internal",
                    project_path,
                    request_id,
                    false,
                    Some("goal_input_missing"),
                ));
            }

            serde_json::json!({
                "user_input": build_goal_submit_prompt(&goal_text),
                "selected_options": selected_options,
                "images": normalized_images,
                "project_path": project_path,
                    "metadata": {
                        "timestamp": chrono::Local::now().to_rfc3339(),
                        "request_id": request_id,
                        "timeline_route_id": timeline_route_id,
                        "conversation_route_id": timeline_route_id,
                        "source": "web_bridge_goal_submit",
                        "mode": "goalrun_takeover",
                    "goal_title": goal_title,
                }
            })
        }
        "loop" | "loop_start" => {
            let user_input = payload
                .get("user_input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let selected_options = payload
                .get("selected_options")
                .cloned()
                .unwrap_or(serde_json::json!([]));
            serde_json::json!({
                "user_input": user_input,
                "selected_options": selected_options,
                "images": [],
                "project_path": project_path,
                    "metadata": {
                        "timestamp": chrono::Local::now().to_rfc3339(),
                        "request_id": request_id,
                        "timeline_route_id": timeline_route_id,
                        "conversation_route_id": timeline_route_id,
                        "source": "web_bridge_loop_start",
                    }
            })
        }
        "enhance" => {
            let user_input = payload
                .get("user_input")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            serde_json::json!({
                "user_input": user_input,
                "selected_options": ["增强"],
                "images": [],
                "project_path": project_path,
                    "metadata": {
                        "timestamp": chrono::Local::now().to_rfc3339(),
                        "request_id": request_id,
                        "timeline_route_id": timeline_route_id,
                        "conversation_route_id": timeline_route_id,
                        "source": "web_bridge_enhance",
                    }
            })
        }
        "cancel" => serde_json::json!("CANCELLED"),
        _ => {
            log::warn!(
                "[Bridge] bridge-only daemon 不支持直接处理 mcp_action: {}",
                action
            );
            return RoomDeliveryResult::rejected(RoomDeliveryAttempt::internal(
                "bridge_only_internal",
                project_path,
                request_id,
                false,
                Some("unsupported_mcp_action"),
            ));
        }
    };

    let response_str = match serde_json::to_string(&response) {
        Ok(value) => value,
        Err(err) => {
            log::error!("[Bridge] bridge-only 序列化 mcp_action 响应失败: {}", err);
            return RoomDeliveryResult::rejected(RoomDeliveryAttempt::internal(
                "bridge_only_internal",
                project_path,
                request_id,
                false,
                Some("response_serialize_failed"),
            ));
        }
    };

    let serve_attempt = try_write_serve_response_file(
        request_id,
        project_path,
        &response_str,
        MCP_ACTION_CACHE_TTL_SECS,
        &bridge_debug_log,
    );
    if !serve_attempt.delivered {
        bridge_debug_log(&format!(
            "try_handle_mcp_action_headless route miss: request_id={:?}, project={}",
            request_id, project_path
        ));
        return RoomDeliveryResult::rejected(serve_attempt);
    }

    crate::ui::live_goal::apply_live_goal_intent_from_response::<tauri::Wry>(
        None, &response, Some(project_path), request_id,
    );

    if matches!(action, "submit" | "continue" | "enhance") {
        let persistent_manager = ConversationManager::new_with_forced_persistence();
        if let Err(err) = crate::ui::commands::record_user_response_node(
            None,
            &persistent_manager,
            &response,
            Some(project_path.to_string()),
            request_id.map(ToOwned::to_owned),
            timeline_route_id.or(request_id).map(ToOwned::to_owned),
            "bridge_serve_response_file",
        )
        .await
        {
            log::warn!(
                "[Conversation] bridge-only 记录用户节点失败: action={}, error={}",
                action,
                err
            );
        }
        broadcast_latest_timeline_node(
            &persistent_manager,
            request_id,
            timeline_route_id.or(request_id),
            project_path,
            "bridge_serve_response_file",
        )
        .await;
    }

    if let Some(rid) = request_id {
        cleanup_completed_session_by_request_id(rid, "mcp-action-headless-serve-file-cleanup")
            .await;
    }

    RoomDeliveryResult::delivered(serve_attempt)
}
