use super::ws::{TimelineSyncService, BRIDGE_BROADCAST};
use crate::conversation::{resolve_tree_route_key, ConversationManager, NodeMetadata, NodeType};
use crate::mcp::types::ImageAttachment;
use crate::utils::append_timeline_debug_log;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager};

pub(super) async fn record_mcp_action_response_node(
    app_handle: &AppHandle,
    action: &str,
    response: &serde_json::Value,
    project_path: &str,
    request_id: Option<&str>,
    timeline_route_id: Option<&str>,
    source: &str,
) -> Result<(), String> {
    let route_id = timeline_route_id.or(request_id);
    if matches!(action, "submit" | "continue" | "enhance") {
        if source == "bridge_serve_response_file" {
            append_timeline_debug_log(
                "rust/bridge::record_mcp_action_response_node:use_persistent_manager",
                serde_json::json!({
                    "action": action,
                    "source": source,
                    "request_id": request_id,
                    "timeline_route_id": timeline_route_id,
                    "project_path": project_path,
                }),
            );
            let persistent_manager = ConversationManager::new_with_forced_persistence();
            crate::ui::commands::record_user_response_node(
                Some(app_handle),
                &persistent_manager,
                response,
                Some(project_path.to_string()),
                request_id.map(ToOwned::to_owned),
                route_id.map(ToOwned::to_owned),
                source,
            )
            .await?;
            broadcast_latest_timeline_node(
                &persistent_manager,
                request_id,
                route_id,
                project_path,
                source,
            )
            .await;
            return Ok(());
        }

        let Some(manager) = app_handle.try_state::<Arc<ConversationManager>>() else {
            log::warn!(
                "[Conversation] Bridge 无法获取 ConversationManager，跳过用户节点记录: action={}, source={}",
                action,
                source
            );
            append_timeline_debug_log(
                "rust/bridge::record_mcp_action_response_node:skipped",
                serde_json::json!({
                    "reason": "conversation_manager_unavailable",
                    "action": action,
                    "source": source,
                    "request_id": request_id,
                    "timeline_route_id": timeline_route_id,
                    "project_path": project_path,
                }),
            );
            return Ok(());
        };

        crate::ui::commands::record_user_response_node(
            Some(app_handle),
            manager.as_ref(),
            response,
            Some(project_path.to_string()),
            request_id.map(ToOwned::to_owned),
            route_id.map(ToOwned::to_owned),
            source,
        )
        .await?;
        broadcast_latest_timeline_node(
            manager.as_ref(),
            request_id,
            route_id,
            project_path,
            source,
        )
        .await;
        return Ok(());
    }

    record_bridge_response_node(
        app_handle,
        response,
        project_path,
        request_id,
        route_id,
        source,
    )
    .await
}

pub(super) async fn broadcast_latest_timeline_node(
    manager: &ConversationManager,
    request_id: Option<&str>,
    timeline_route_id: Option<&str>,
    project_path: &str,
    source: &str,
) {
    let route_id = timeline_route_id.or(request_id);
    let request_key = resolve_tree_route_key(route_id, Some(project_path));
    let Some(tree_id) = manager
        .get_tree_for_route(route_id, Some(project_path))
        .await
    else {
        append_timeline_debug_log(
            "rust/bridge::broadcast_latest_timeline_node:skipped",
            serde_json::json!({
                "reason": "tree_not_found",
                "source": source,
                "request_key": request_key,
                "actual_request_id": request_id,
                "project_path": project_path,
            }),
        );
        return;
    };
    let Some(node_id) = manager.get_current_node_id(&tree_id).await else {
        append_timeline_debug_log(
            "rust/bridge::broadcast_latest_timeline_node:skipped",
            serde_json::json!({
                "reason": "current_node_missing",
                "source": source,
                "tree_id": tree_id,
                "request_key": request_key,
            }),
        );
        return;
    };
    let Some(node) = manager.get_node(&tree_id, &node_id).await else {
        append_timeline_debug_log(
            "rust/bridge::broadcast_latest_timeline_node:skipped",
            serde_json::json!({
                "reason": "node_not_found",
                "source": source,
                "tree_id": tree_id,
                "node_id": node_id,
                "request_key": request_key,
            }),
        );
        return;
    };

    if !TimelineSyncService::node_matches_route(&node, &tree_id, route_id, Some(project_path)) {
        append_timeline_debug_log(
            "rust/bridge::broadcast_latest_timeline_node:skipped",
            serde_json::json!({
                "reason": "route_mismatch",
                "source": source,
                "tree_id": tree_id,
                "node_id": node_id,
                "request_key": request_key,
                "actual_request_id": request_id,
                "project_path": project_path,
            }),
        );
        return;
    }

    let Some(delta_msg) = TimelineSyncService::build_delta_message(route_id, Some(project_path), &node) else { return; };
    if let Err(err) = BRIDGE_BROADCAST.send(delta_msg) {
        log::debug!(
            "[TimelineSync] 广播用户增量节点失败（可能无订阅者）: {}",
            err
        );
    }
    append_timeline_debug_log(
        "rust/bridge::broadcast_latest_timeline_node:sent",
        serde_json::json!({
            "source": source,
            "tree_id": tree_id,
            "node_id": node_id,
            "node_type": node.node_type.as_key(),
            "request_key": request_key,
            "project_path": project_path,
        }),
    );
}

async fn record_bridge_response_node(
    app_handle: &AppHandle,
    response: &serde_json::Value,
    project_path: &str,
    request_id: Option<&str>,
    timeline_route_id: Option<&str>,
    source: &str,
) -> Result<(), String> {
    let Some(content) = extract_bridge_response_content(response) else {
        log::warn!(
            "[Conversation] Bridge 跳过助手节点记录: 无可用内容 (source={}, request_id={:?}, project_path={})",
            source,
            request_id,
            project_path
        );
        eprintln!(
            "[Conversation] record_bridge_response_node skipped: no content (source={}, request_id={:?}, project_path={})",
            source, request_id, project_path
        );
        append_timeline_debug_log(
            "rust/bridge::record_bridge_response_node:skipped",
            serde_json::json!({
                "reason": "no_content",
                "source": source,
                "request_id": request_id,
                "timeline_route_id": timeline_route_id,
                "project_path": project_path,
            }),
        );
        return Ok(());
    };
    let Some(manager) = app_handle.try_state::<Arc<ConversationManager>>() else {
        log::warn!("[Conversation] Bridge 无法获取 ConversationManager，跳过助手节点记录");
        append_timeline_debug_log(
            "rust/bridge::record_bridge_response_node:skipped",
            serde_json::json!({
                "reason": "conversation_manager_unavailable",
                "source": source,
                "request_id": request_id,
                "timeline_route_id": timeline_route_id,
                "project_path": project_path,
            }),
        );
        return Ok(());
    };

    let route_id = timeline_route_id.or(request_id);
    let request_key = resolve_tree_route_key(route_id, Some(project_path));
    append_timeline_debug_log(
        "rust/bridge::record_bridge_response_node:start",
        serde_json::json!({
            "source": source,
            "request_id": request_id,
            "timeline_route_id": timeline_route_id,
            "project_path": project_path,
            "request_key": request_key,
            "content_len": content.chars().count(),
        }),
    );
    eprintln!(
        "[Conversation] record_bridge_response_node start: source={}, request_id={:?}, project_path={}, request_key={:?}, content_len={}",
        source,
        request_id,
        project_path,
        request_key,
        content.chars().count()
    );
    log::info!(
        "[Conversation] Bridge 开始记录助手节点: source={}, request_key={:?}, content_len={}",
        source,
        request_key,
        content.chars().count()
    );
    let tree_id = manager
        .get_or_create_tree_for_route(route_id, Some(project_path))
        .await;
    let parent_id = manager.get_current_node_id(&tree_id).await;
    append_timeline_debug_log(
        "rust/bridge::record_bridge_response_node:resolved_tree_context",
        serde_json::json!({
            "source": source,
            "tree_id": tree_id,
            "parent_id": parent_id,
            "request_key": request_key,
        }),
    );
    let metadata = NodeMetadata {
        conversation_id: Some(tree_id.clone()),
        project_path: Some(project_path.to_string()),
        predefined_options: None,
        selected_option: extract_bridge_selected_option(response),
        images: manager.prepare_timeline_images(extract_bridge_images(response)),
        link_url: None,
        link_title: None,
        request_id: request_key.clone(),
        run_id: None,
        generation: None,
        stale_of: None,
        superseded_by: None,
        checkpoint_id: None,
        checkpoint_commit: None,
        checkpoint_message: None,
        source: Some(source.to_string()),
    };

    let node_id = match manager
        .add_node(
            &tree_id,
            parent_id.clone(),
            NodeType::Assistant,
            content,
            false,
            metadata,
        )
        .await
    {
        Ok(node_id) => node_id,
        Err(err) => {
            append_timeline_debug_log(
                "rust/bridge::record_bridge_response_node:failed",
                serde_json::json!({
                    "reason": "add_node_failed",
                    "source": source,
                    "request_key": request_key,
                    "tree_id": tree_id,
                    "parent_id": parent_id,
                    "error": err.clone(),
                }),
            );
            return Err(err);
        }
    };

    log::info!(
        "[Conversation] Bridge 助手节点记录成功: tree_id={}, node_id={}, source={}",
        tree_id,
        node_id,
        source
    );
    eprintln!(
        "[Conversation] record_bridge_response_node success: tree_id={}, node_id={}, source={}, request_key={:?}",
        tree_id, node_id, source, request_key
    );
    append_timeline_debug_log(
        "rust/bridge::record_bridge_response_node:success",
        serde_json::json!({
            "source": source,
            "tree_id": tree_id,
            "node_id": node_id,
            "parent_id": parent_id,
            "request_key": request_key,
        }),
    );

    if let Some(new_node) = manager.get_node(&tree_id, &node_id).await {
        if let Some(delta_msg) = TimelineSyncService::build_delta_message(
            request_key.as_deref(),
            Some(project_path),
            &new_node,
        ) {
            if let Err(err) = BRIDGE_BROADCAST.send(delta_msg) {
                log::debug!("[TimelineSync] 广播增量节点失败（可能无订阅者）: {}", err);
            }
        }
    }

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
            "actual_request_id": request_id,
            "project_path": project_path,
            "source": source,
        }),
    ) {
        log::warn!(
            "[Conversation] Bridge 助手节点事件广播失败: source={}, error={}",
            source,
            err
        );
    }

    Ok(())
}

fn extract_bridge_response_content(response: &serde_json::Value) -> Option<String> {
    match response {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        serde_json::Value::Object(map) => {
            let user_input = map
                .get("user_input")
                .and_then(|value| value.as_str())
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned);
            if user_input.is_some() {
                return user_input;
            }

            if let Some(selected_options) = map
                .get("selected_options")
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::trim))
                        .filter(|item| !item.is_empty())
                        .map(ToOwned::to_owned)
                        .collect::<Vec<String>>()
                })
                .filter(|items| !items.is_empty())
            {
                return Some(selected_options.join(" / "));
            }

            if let Some(image_count) = map
                .get("images")
                .and_then(|value| value.as_array())
                .map(Vec::len)
                .filter(|count| *count > 0)
            {
                return Some(format!("[{} image(s)]", image_count));
            }

            let serialized = response.to_string();
            if serialized == "null" {
                None
            } else {
                Some(serialized)
            }
        }
        _ => {
            let serialized = response.to_string();
            if serialized == "null" {
                None
            } else {
                Some(serialized)
            }
        }
    }
}

fn extract_bridge_selected_option(response: &serde_json::Value) -> Option<String> {
    response
        .get("selected_options")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_bridge_images(response: &serde_json::Value) -> Option<Vec<ImageAttachment>> {
    let parsed_images = response
        .get("images")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<ImageAttachment>(item.clone()).ok())
                .collect::<Vec<ImageAttachment>>()
        })
        .unwrap_or_default();

    if parsed_images.is_empty() {
        None
    } else {
        Some(parsed_images)
    }
}
