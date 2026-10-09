use super::{ConversationManager, ConversationNode, NodeMetadata, NodeType};
use crate::utils::append_timeline_debug_log;
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub async fn list_native_conversation_histories() -> Result<Vec<super::manager::NativeHistorySession>, String> {
    let manager = ConversationManager::new_with_forced_persistence();
    Ok(manager.list_native_history_sessions().await)
}

#[tauri::command]
pub async fn migrate_timeline_image_storage(
    confirmation: String,
) -> Result<super::manager::TimelineImageMigrationReport, String> {
    if confirmation != "MIGRATE_TIMELINE_IMAGES" {
        return Err("迁移确认口令不匹配，未修改时间线数据".to_string());
    }
    tokio::task::spawn_blocking(|| {
        let manager = ConversationManager::new_with_forced_persistence();
        manager.migrate_inline_timeline_images()
    })
    .await
    .map_err(|error| format!("时间线图片迁移任务异常退出: {}", error))?
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsureConversationNodeResult {
    pub node_id: String,
    pub reused: bool,
}

#[tauri::command]
pub async fn create_conversation_tree(
    manager: State<'_, Arc<ConversationManager>>,
    request_id: Option<String>,
    project_path: Option<String>,
) -> Result<String, String> {
    append_timeline_debug_log(
        "rust/conversation::create_conversation_tree:start",
        serde_json::json!({
            "request_id": request_id,
            "project_path": project_path,
        }),
    );
    let tree_id = manager
        .create_tree_for_route(request_id, project_path)
        .await;
    append_timeline_debug_log(
        "rust/conversation::create_conversation_tree:success",
        serde_json::json!({
            "tree_id": tree_id,
        }),
    );
    Ok(tree_id)
}

#[tauri::command]
pub async fn add_conversation_node(
    app: AppHandle,
    manager: State<'_, Arc<ConversationManager>>,
    tree_id: String,
    parent_id: Option<String>,
    node_type: String,
    content: String,
    is_markdown: bool,
    metadata: NodeMetadata,
) -> Result<String, String> {
    let normalized_node_type = node_type.trim().to_ascii_lowercase();
    let node_type = match normalized_node_type.as_str() {
        "user" => NodeType::User,
        "assistant" => NodeType::Assistant,
        _ => return Err("Invalid node type".to_string()),
    };
    let request_key = metadata.request_id.clone();
    append_timeline_debug_log(
        "rust/conversation::add_conversation_node:start",
        serde_json::json!({
            "tree_id": tree_id,
            "parent_id": parent_id.clone(),
            "node_type": normalized_node_type,
            "request_key": request_key.clone(),
        }),
    );

    let node_id = manager
        .add_node(
            &tree_id,
            parent_id,
            node_type,
            content,
            is_markdown,
            metadata,
        )
        .await?;
    let recorded_node = manager.get_node(&tree_id, &node_id).await;
    let resolved_parent_id = recorded_node
        .as_ref()
        .and_then(|node| node.parent_id.clone());
    append_timeline_debug_log(
        "rust/conversation::add_conversation_node:success",
        serde_json::json!({
            "tree_id": tree_id,
            "node_id": node_id,
            "parent_id": resolved_parent_id.clone(),
            "node_type": normalized_node_type,
            "request_key": request_key.clone(),
        }),
    );

    if let Err(err) = app.emit(
        "conversation-node-recorded",
        serde_json::json!({
            "tree_id": tree_id,
            "conversation_id": tree_id,
            "node_id": node_id,
            "parent_id": resolved_parent_id,
            "node_type": normalized_node_type,
            "request_key": request_key.clone(),
            "request_id": request_key,
            "project_path": recorded_node
                .as_ref()
                .and_then(|node| node.metadata.project_path.clone()),
            "source": "tauri_add_conversation_node",
        }),
    ) {
        log::warn!(
            "[Conversation] add_conversation_node 事件广播失败: node_id={}, error={}",
            node_id,
            err
        );
    }

    Ok(node_id)
}

#[tauri::command]
pub async fn ensure_conversation_assistant_node(
    app: AppHandle,
    manager: State<'_, Arc<ConversationManager>>,
    tree_id: String,
    content: String,
    is_markdown: bool,
    metadata: NodeMetadata,
) -> Result<EnsureConversationNodeResult, String> {
    let request_key = metadata.request_id.clone();
    append_timeline_debug_log(
        "rust/conversation::ensure_conversation_assistant_node:start",
        serde_json::json!({
            "tree_id": tree_id,
            "request_key": request_key.clone(),
        }),
    );

    let outcome = manager
        .ensure_assistant_request_node(&tree_id, content, is_markdown, metadata)
        .await?;
    let recorded_node = manager.get_node(&tree_id, &outcome.node_id).await;
    let resolved_parent_id = recorded_node
        .as_ref()
        .and_then(|node| node.parent_id.clone());
    append_timeline_debug_log(
        "rust/conversation::ensure_conversation_assistant_node:success",
        serde_json::json!({
            "tree_id": tree_id,
            "node_id": outcome.node_id,
            "parent_id": resolved_parent_id.clone(),
            "request_key": request_key.clone(),
            "reused": outcome.reused,
        }),
    );

    if !outcome.reused {
        if let Err(err) = app.emit(
            "conversation-node-recorded",
            serde_json::json!({
                "tree_id": tree_id,
                "conversation_id": tree_id,
                "node_id": outcome.node_id,
                "parent_id": resolved_parent_id,
                "node_type": "assistant",
                "request_key": request_key.clone(),
                "request_id": request_key,
                "project_path": recorded_node
                    .as_ref()
                    .and_then(|node| node.metadata.project_path.clone()),
                "source": "tauri_ensure_conversation_assistant_node",
            }),
        ) {
            log::warn!(
                "[Conversation] ensure_conversation_assistant_node 事件广播失败: node_id={}, error={}",
                outcome.node_id,
                err
            );
        }
    }

    Ok(EnsureConversationNodeResult {
        node_id: outcome.node_id,
        reused: outcome.reused,
    })
}

#[tauri::command]
pub async fn switch_conversation_node(
    manager: State<'_, Arc<ConversationManager>>,
    tree_id: String,
    node_id: String,
) -> Result<ConversationNode, String> {
    manager.switch_to_node(&tree_id, &node_id).await
}

#[tauri::command]
pub async fn get_conversation_path(
    manager: State<'_, Arc<ConversationManager>>,
    tree_id: String,
    node_id: String,
) -> Result<Vec<ConversationNode>, String> {
    append_timeline_debug_log(
        "rust/conversation::get_conversation_path:start",
        serde_json::json!({
            "tree_id": tree_id,
            "node_id": node_id,
        }),
    );
    let mut path = manager.get_node_path(&tree_id, &node_id).await;
    if path.is_err() {
        let persisted = ConversationManager::new_with_forced_persistence();
        if persisted.list_native_history_sessions().await.iter().any(|session| session.tree_id == tree_id) {
            path = persisted.get_node_path(&tree_id, &node_id).await;
        }
    }
    match &path {
        Ok(nodes) => {
            append_timeline_debug_log(
                "rust/conversation::get_conversation_path:success",
                serde_json::json!({
                    "tree_id": tree_id,
                    "node_id": node_id,
                    "node_count": nodes.len(),
                    "nodes": nodes
                        .iter()
                        .map(|node| serde_json::json!({
                            "id": node.id.clone(),
                            "parent_id": node.parent_id.clone(),
                            "node_type": node.node_type.as_key(),
                        }))
                        .collect::<Vec<_>>(),
                }),
            );
        }
        Err(err) => {
            append_timeline_debug_log(
                "rust/conversation::get_conversation_path:failed",
                serde_json::json!({
                    "tree_id": tree_id,
                    "node_id": node_id,
                    "error": err,
                }),
            );
        }
    }
    path
}

#[tauri::command]
pub async fn clear_conversation_tree(
    manager: State<'_, Arc<ConversationManager>>,
    tree_id: String,
) -> Result<usize, String> {
    append_timeline_debug_log(
        "rust/conversation::clear_conversation_tree:start",
        serde_json::json!({ "tree_id": tree_id }),
    );
    let count = manager.clear_tree(&tree_id).await?;
    append_timeline_debug_log(
        "rust/conversation::clear_conversation_tree:success",
        serde_json::json!({ "tree_id": tree_id, "cleared_count": count }),
    );
    Ok(count)
}

#[tauri::command]
pub async fn get_current_conversation_node_id(
    manager: State<'_, Arc<ConversationManager>>,
    tree_id: String,
) -> Result<Option<String>, String> {
    Ok(manager.get_current_node_id(&tree_id).await)
}
