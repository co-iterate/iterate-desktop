use anyhow::Result;
use rmcp::{model::*, Error as McpError};

use super::logger::{append_conversation_log, get_conversation_log_path, ConversationEntry};
use crate::mcp::codex_deeplink::{
    codex_thread_deeplink, normalize_codex_thread_deeplink, normalize_codex_thread_id,
};
use crate::mcp::codex_home::codex_home_from_process_or_parent_env;
use crate::mcp::handlers::{create_tauri_popup, parse_mcp_response};
use crate::mcp::tools::checkpoint;
use crate::mcp::utils::{generate_request_id, popup_error};
use crate::mcp::{McpResponse, PopupRequest, ResponseMetadata, ZhiRequest};

const EMPTY_ZHI_MESSAGE_FALLBACK: &str = "已暂停，等待你的下一步指令。";

/// 智能代码审查交互工具
///
/// 支持预定义选项、自由文本输入和图片上传
#[derive(Clone)]
pub struct InteractionTool;

impl InteractionTool {
    pub async fn zhi(request: ZhiRequest) -> Result<CallToolResult, McpError> {
        #[cfg(target_os = "windows")]
        crate::app::windows_lifecycle::activate_mcp_launch().map_err(|error| {
            McpError::internal_error(format!("重新启动 iterate 失败: {error}"), None)
        })?;

        let ai_message = normalize_zhi_message(&request.message);
        let project_path = request.project_path.clone();
        let request_id = generate_request_id();
        if let Some(path) = project_path.as_deref() {
            checkpoint::touch_auto_checkpoint_monitor(path, Some(&request_id));
        }

        // 自动创建检查点（如果有项目路径且有未提交的更改）；subject 稍后写入对话 md，便于 che / rg 对齐
        let workspace_checkpoint = project_path
            .as_ref()
            .and_then(|path| auto_create_checkpoint(path, Some(&request_id)));
        let request_codex_thread_id = request
            .codex_thread_id
            .as_deref()
            .and_then(normalize_codex_thread_id);
        let live_goal_codex_thread_id = if request_codex_thread_id.is_none() {
            crate::ui::live_goal::live_goal_codex_thread_id_for_project(project_path.as_deref())
                .as_deref()
                .and_then(normalize_codex_thread_id)
        } else {
            None
        };
        let codex_thread_id = request_codex_thread_id
            .clone()
            .or_else(|| live_goal_codex_thread_id.clone());
        let codex_deeplink = request
            .codex_deeplink
            .as_deref()
            .and_then(normalize_codex_thread_deeplink)
            .or_else(|| codex_thread_id.as_deref().and_then(codex_thread_deeplink));
        crate::utils::append_timeline_debug_log(
            "rust/mcp::interaction/zhi_route_context",
            serde_json::json!({
                "request_id": request_id,
                "project_path": project_path,
                "request_codex_thread_id": request_codex_thread_id,
                "live_goal_codex_thread_id": live_goal_codex_thread_id,
                "effective_codex_thread_id": codex_thread_id,
            }),
        );

        let popup_request = PopupRequest {
            id: request_id.clone(),
            message: ai_message.clone(),
            predefined_options: if request.predefined_options.is_empty() {
                None
            } else {
                Some(request.predefined_options)
            },
            is_markdown: request.is_markdown,
            project_path: request.project_path,
            codex_home: request.codex_home.or_else(codex_home_from_env),
            codex_thread_id,
            codex_deeplink,
            checkpoint_id: workspace_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.checkpoint_id.clone()),
            checkpoint_commit: workspace_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.commit_hash.clone()),
            checkpoint_message: workspace_checkpoint
                .as_ref()
                .map(|checkpoint| checkpoint.commit_subject.clone()),
            link_url: None,
            link_title: None,
            browser_ai_response: None,
        };

        mark_live_goal_waiting_for_user(project_path.as_deref(), &request_id);

        match create_tauri_popup(&popup_request) {
            Ok(response) => {
                mark_live_goal_user_response_received(project_path.as_deref(), &request_id);
                let (response, end_source) = normalize_terminal_response(&response);

                // 记录对话日志
                log_conversation(
                    &ai_message,
                    &response,
                    project_path.clone(),
                    Some(request_id),
                    workspace_checkpoint,
                );

                // 解析响应内容，支持文本和图片
                let mut content = parse_mcp_response(&response)?;
                if let Some(end_source) = end_source {
                    content.push(Content::text(format!(
                        "继续对话: false\n响应来源: {}",
                        end_source
                    )));
                }

                // compact 模式：附带对话日志文件路径，方便 AI 在旧消息被砍后用 read_file 找回上下文
                if request.compact.unwrap_or(false) {
                    if let Some(log_path) = get_conversation_log_path(project_path.as_deref()) {
                        content.push(Content::text(format!(
                            "📦 对话记录: {}\n如需回顾旧上下文，请 read_file 该路径。",
                            log_path
                        )));
                    }
                }

                // 状态文件提示：如果 .cunzhi-memory/progress.md 存在，提醒 AI 更新
                if let Some(ref path) = project_path {
                    let progress_file = std::path::Path::new(path)
                        .join(".cunzhi-memory")
                        .join("progress.md");
                    if progress_file.exists() {
                        content.push(Content::text(
                            "📋 记得更新 .cunzhi-memory/progress.md（目标/已完成/下一步/阻塞项）"
                                .to_string(),
                        ));
                    }
                }

                Ok(CallToolResult::success(content))
            }
            Err(e) => {
                mark_live_goal_user_interaction_failed(project_path.as_deref(), &request_id);
                Err(popup_error(e.to_string()).into())
            }
        }
    }
}

fn normalize_zhi_message(message: &str) -> String {
    if message.trim().is_empty() {
        EMPTY_ZHI_MESSAGE_FALLBACK.to_string()
    } else {
        message.to_string()
    }
}

fn normalize_terminal_response(response: &str) -> (String, Option<String>) {
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(response) else {
        return (response.to_string(), None);
    };
    let user_input = value
        .get("user_input")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let selected_options = value
        .get("selected_options")
        .and_then(serde_json::Value::as_array)
        .map(|options| {
            options
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let submitted_source = value
        .get("metadata")
        .and_then(|metadata| metadata.get("source"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let end_source = if crate::conversation::is_explicit_conversation_end_response(
        &user_input,
        &selected_options,
    ) {
        Some(crate::conversation::EXPLICIT_CONVERSATION_END_SOURCE)
    } else if crate::conversation::is_popup_closed_response_source(&submitted_source) {
        Some(crate::conversation::POPUP_CLOSED_SOURCE)
    } else {
        None
    };
    let Some(end_source) = end_source else {
        return (response.to_string(), None);
    };

    if let Some(object) = value.as_object_mut() {
        object.insert("selected_options".to_string(), serde_json::json!([]));
        object.insert("images".to_string(), serde_json::json!([]));
        object.insert("file_paths".to_string(), serde_json::json!([]));
        object.insert("image_paths".to_string(), serde_json::json!([]));
        let metadata = object
            .entry("metadata".to_string())
            .or_insert_with(|| serde_json::json!({}));
        if let Some(metadata) = metadata.as_object_mut() {
            metadata.insert(
                "source".to_string(),
                serde_json::json!(end_source),
            );
        }
    }
    (value.to_string(), Some(end_source.to_string()))
}

fn mark_live_goal_waiting_for_user(project_path: Option<&str>, request_id: &str) {
    log_live_goal_interaction_phase_result(
        "waiting_for_user",
        crate::ui::live_goal::mark_live_goal_waiting_for_user(project_path, Some(request_id)),
    );
}

fn mark_live_goal_user_response_received(project_path: Option<&str>, request_id: &str) {
    log_live_goal_interaction_phase_result(
        "running",
        crate::ui::live_goal::mark_live_goal_user_response_received(project_path, Some(request_id)),
    );
}

fn mark_live_goal_user_interaction_failed(project_path: Option<&str>, request_id: &str) {
    log_live_goal_interaction_phase_result(
        "failed",
        crate::ui::live_goal::mark_live_goal_user_interaction_failed(
            project_path,
            Some(request_id),
        ),
    );
}

fn log_live_goal_interaction_phase_result(
    phase: &str,
    result: Result<Option<crate::ui::live_goal::LiveGoalSnapshot>, String>,
) {
    match result {
        Ok(Some(goal)) => crate::utils::append_timeline_debug_log(
            "rust/live_goal::mcp_interaction_phase:applied",
            serde_json::json!({
                "phase": phase,
                "goal_id": goal.id,
                "project_path": goal.project_path,
                "request_id": goal.request_id,
            }),
        ),
        Ok(None) => crate::utils::append_timeline_debug_log(
            "rust/live_goal::mcp_interaction_phase:skipped",
            serde_json::json!({ "phase": phase }),
        ),
        Err(error) => {
            log::warn!(
                "[LiveGoal] failed to update MCP interaction phase {}: {}",
                phase,
                error
            );
            crate::utils::append_timeline_debug_log(
                "rust/live_goal::mcp_interaction_phase:failed",
                serde_json::json!({ "phase": phase, "error": error }),
            );
        }
    }
}

fn codex_home_from_env() -> Option<String> {
    codex_home_from_process_or_parent_env()
}

/// 记录对话到日志
fn log_conversation(
    ai_message: &str,
    response: &str,
    project_path: Option<String>,
    request_id: Option<String>,
    workspace_checkpoint: Option<checkpoint::CheckpointMetadata>,
) {
    // 跳过取消操作
    if response.trim() == "CANCELLED" || response.trim() == "用户取消了操作" {
        return;
    }

    // 解析响应获取用户输入详情
    let parsed = parse_response_for_log(response);

    let entry = ConversationEntry {
        conversation_id: parsed
            .metadata
            .conversation_id
            .clone()
            .or_else(|| parsed.metadata.tree_id.clone()),
        current_node_id: parsed
            .metadata
            .current_node_id
            .clone()
            .or_else(|| parsed.metadata.node_id.clone()),
        timeline_route_id: parsed
            .metadata
            .timeline_route_id
            .clone()
            .or_else(|| parsed.metadata.conversation_route_id.clone()),
        run_id: parsed.metadata.run_id.clone(),
        generation: parsed.metadata.generation,
        stale_of: parsed.metadata.stale_of.clone(),
        superseded_by: parsed.metadata.superseded_by.clone(),
        ai_message: ai_message.to_string(),
        user_response: parsed.user_text,
        project_path,
        image_count: parsed.image_count,
        file_paths: parsed.file_paths,
        image_paths: parsed.image_paths,
        selected_options: parsed.selected_options,
        request_id,
        checkpoint_id: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.checkpoint_id.clone()),
        checkpoint_commit: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.commit_hash.clone()),
        push_status: workspace_checkpoint
            .as_ref()
            .map(|checkpoint| checkpoint.push_status.clone()),
        response_source: parsed.metadata.source,
        workspace_checkpoint_message: workspace_checkpoint
            .map(|checkpoint| checkpoint.commit_subject),
    };

    append_conversation_log(&entry);
}

/// 解析响应用于日志记录
struct ParsedLogResponse {
    user_text: String,
    selected_options: Vec<String>,
    image_count: usize,
    file_paths: Vec<String>,
    image_paths: Vec<String>,
    metadata: ResponseMetadata,
}

fn parse_response_for_log(response: &str) -> ParsedLogResponse {
    // 尝试解析结构化格式
    if let Ok(structured) = serde_json::from_str::<McpResponse>(response) {
        return ParsedLogResponse {
            user_text: structured.user_input.unwrap_or_default(),
            selected_options: structured.selected_options,
            image_count: structured.images.len(),
            file_paths: structured.file_paths,
            image_paths: structured.image_paths,
            metadata: structured.metadata,
        };
    }

    // 回退：直接作为文本
    ParsedLogResponse {
        user_text: response.to_string(),
        selected_options: vec![],
        image_count: 0,
        file_paths: vec![],
        image_paths: vec![],
        metadata: ResponseMetadata::default(),
    }
}

/// 自动创建检查点（如果有未提交的更改）；返回本次 commit 的 subject，未创建则为 `None`。
fn auto_create_checkpoint(
    project_path: &str,
    request_id: Option<&str>,
) -> Option<checkpoint::CheckpointMetadata> {
    checkpoint::maybe_auto_checkpoint(project_path, request_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_zhi_message_preserves_content() {
        assert_eq!(normalize_zhi_message("正常消息"), "正常消息");
    }

    #[test]
    fn explicit_end_response_discards_business_payload_without_touching_ordinary_input() {
        let response = serde_json::json!({
            "user_input": "结束对话。",
            "selected_options": ["继续"],
            "images": [{"data": "abc"}],
            "file_paths": ["a.txt"],
            "image_paths": ["a.png"],
            "metadata": {"source": "popup"}
        })
        .to_string();
        let (normalized, end_source) = normalize_terminal_response(&response);
        assert_eq!(
            end_source.as_deref(),
            Some(crate::conversation::EXPLICIT_CONVERSATION_END_SOURCE)
        );
        let normalized: serde_json::Value = serde_json::from_str(&normalized).unwrap();
        assert_eq!(normalized["selected_options"], serde_json::json!([]));
        assert_eq!(normalized["images"], serde_json::json!([]));
        assert_eq!(normalized["file_paths"], serde_json::json!([]));
        assert_eq!(normalized["image_paths"], serde_json::json!([]));
        assert_eq!(
            normalized["metadata"]["source"],
            crate::conversation::EXPLICIT_CONVERSATION_END_SOURCE
        );

        let ordinary = serde_json::json!({"user_input": "如何结束对话"}).to_string();
        let (unchanged, end_source) = normalize_terminal_response(&ordinary);
        assert!(end_source.is_none());
        assert_eq!(unchanged, ordinary);
    }

    #[test]
    fn terminal_response_accepts_end_options_and_popup_close_source() {
        let option_response = serde_json::json!({
            "user_input": "",
            "selected_options": ["结束对话"],
            "images": [],
            "metadata": {"source": "popup"}
        })
        .to_string();
        let (normalized, end_source) = normalize_terminal_response(&option_response);
        assert_eq!(
            end_source.as_deref(),
            Some(crate::conversation::EXPLICIT_CONVERSATION_END_SOURCE)
        );
        let normalized: serde_json::Value = serde_json::from_str(&normalized).unwrap();
        assert_eq!(normalized["selected_options"], serde_json::json!([]));

        let close_response = serde_json::json!({
            "user_input": "",
            "selected_options": [],
            "images": [],
            "metadata": {"source": "popup_closed"}
        })
        .to_string();
        let (_, end_source) = normalize_terminal_response(&close_response);
        assert_eq!(
            end_source.as_deref(),
            Some(crate::conversation::POPUP_CLOSED_SOURCE)
        );
    }

    #[test]
    fn normalize_zhi_message_uses_fallback_for_blank_input() {
        assert_eq!(normalize_zhi_message(""), EMPTY_ZHI_MESSAGE_FALLBACK);
        assert_eq!(normalize_zhi_message(" \n\t"), EMPTY_ZHI_MESSAGE_FALLBACK);
    }

    #[test]
    fn parse_response_for_log_preserves_attachment_paths() {
        let response = serde_json::json!({
            "user_input": "看附件",
            "selected_options": ["继续"],
            "images": [{
                "data": "AAAA",
                "media_type": "image/png",
                "filename": null
            }],
            "file_paths": ["/tmp/spec.md"],
            "image_paths": ["/Users/test/.cunzhi/images/image_123_0.png"],
            "metadata": {
                "timestamp": "2026-06-17T00:00:00Z",
                "request_id": "req-1",
                "source": "popup_submit"
            }
        })
        .to_string();

        let parsed = parse_response_for_log(&response);

        assert_eq!(parsed.user_text, "看附件");
        assert_eq!(parsed.selected_options, vec!["继续".to_string()]);
        assert_eq!(parsed.image_count, 1);
        assert_eq!(parsed.file_paths, vec!["/tmp/spec.md".to_string()]);
        assert_eq!(
            parsed.image_paths,
            vec!["/Users/test/.cunzhi/images/image_123_0.png".to_string()]
        );
        assert_eq!(parsed.metadata.request_id.as_deref(), Some("req-1"));
    }
}
