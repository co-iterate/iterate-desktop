use rmcp::{model::*, Error as McpError};

use crate::log_debug;
use crate::mcp::tools::interaction::logger::{append_conversation_log, ConversationEntry};
use crate::mcp::types::PaiRequest;
use crate::mcp::{handlers::create_tauri_popup, utils::generate_request_id, PopupRequest};

/// Pai Room 编排工具
///
/// 根据任务参数生成 codex-room 调度草案，供主会话按 room 协议执行。
#[derive(Clone)]
pub struct DispatchTool;

impl DispatchTool {
    /// 生成 room 调度草案
    fn generate_room_dispatch_plan(request: &PaiRequest) -> String {
        let items_list = request
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| format!("{}. {}", i + 1, item))
            .collect::<Vec<_>>()
            .join("\n");

        let mut prompt = format!(
            r#"## Pai Room 调度草案

主会话作为 room hub 执行调度；不要创建新聊天窗口。Codex 内置子代理可在任务边界清晰、主会话能汇总把关时作为可选补充分工；不要绕过 room hub 和主会话汇总。

**任务类型**: {}
**范围**（共 {} 个）：
{}
"#,
            request.task_type,
            request.items.len(),
            items_list
        );

        // 添加源文件和目标文件
        if let Some(ref source) = request.source_file {
            prompt.push_str(&format!("**源文件**: {}\n", source));
        }
        if let Some(ref target) = request.target_file {
            prompt.push_str(&format!("**目标文件**: {}\n", target));
        }

        prompt.push_str(
            r#"
### room 调度命令

```bash
pnpm codex-room refresh-agents --room <room> --project <project_path> --ttl-ms 180000
pnpm codex-room agents --room <room> --project <project_path>
pnpm codex-room validate-target <agent> --room <room> --project <project_path>
pnpm codex-room submit-target <agent> "<worker任务+回包协议>" --room <room> --project <project_path> --run-id <run_id> --step <step> --dedupe-key <dedupe_key>
pnpm codex-room wait --room <room> --project <project_path> --after <target_submit_message_id> --timeout-ms 60000 --types external_message
```

	### pairoom worker 协议

	给 `codex-room` / pairoom worker 的任务必须包含：

```text
	你是 pai room 的可复用 worker，不是主对话代理。
	不要进入无限对话，不要向用户提问，不要向用户做最终汇报。
	默认只读；除非任务明确授权，不要修改文件。
	除非主会话明确授权并给出边界，不要在 worker 内再启动二级子代理。
	完成后按顺序执行两个动作：
	1. 向 room post 一条 external_message：
	pnpm codex-room post "worker_done | from=<agent> | status=<success|partial|failed> | scope=<scope> | findings=<findings> | tests=<tests> | risks=<risks>" --room <room> --project <project_path> --run-id <run_id> --step <step> --from <agent> --dedupe-key <dedupe_key>
	2. post 成功后在物理末尾调用 zhi / call_zhi 归队待命，等待下一次 room 调度；不要向用户做最终汇报。
	```

### 调度规则

1. 只使用当前项目 room 中 `healthy + waiting_user + request_id 未过期 + workspace 匹配` 的目标。
	2. 没有 dispatchable target 时，可在当前 Codex 环境支持时启用 Codex 内置子代理补充分工；如果也不可用，再显式报告暂不可派发。
	3. 默认只读；写入必须用户明确授权，并且每个 worker 文件范围不重叠。
	4. 主会话读取 room 回包、合并冲突、最终通过 `zhi/call_zhi` 汇报给用户。

	### Codex 内置子代理协议

	1. Codex 内置子代理只作为主会话授权的补充分工手段，不走 `codex-room submit-target` 投递链路。
	2. 内置子代理只返回子任务结果给主会话；用户可见汇总和 `zhi/call_zhi` 交互仍由主会话完成。
	3. 仅 pairoom worker 执行 `post -> zhi/call_zhi` 归队协议。
	4. 主会话配置或启用 Codex 内置子代理时，子任务 prompt 的物理末尾必须写明：输出就行，不用调用 zhi/call_zhi。
	"#,
        );

        // 添加输出格式模板
        if let Some(ref format) = request.output_format {
            prompt.push_str(&format!("\n### 输出格式\n{}\n", format));
        } else {
            prompt.push_str("\n### 输出格式\n默认使用 `worker_done | from=... | findings=... | tests=... | risks=...` 回包。\n");
        }

        // 添加额外步骤
        if let Some(ref extra) = request.extra_steps {
            prompt.push_str(&format!("\n### 额外说明\n{}\n", extra));
        }

        prompt
    }

    pub async fn pai(request: PaiRequest) -> Result<CallToolResult, McpError> {
        log_debug!(
            "生成 Pai Room 调度草案，任务类型: {}, 条目数: {}",
            request.task_type,
            request.items.len()
        );

        if request.items.is_empty() {
            return Err(McpError::invalid_params("任务范围列表不能为空", None));
        }

        let prompt = Self::generate_room_dispatch_plan(&request);

        // 通过寸止窗口显示 room 调度草案，方便主会话按 room 协议执行。
        let popup_message = format!(
            r#"## Pai Room 调度草案

**任务类型**: {}
**条目数量**: {} 个

---

按以下草案在当前主会话执行 room 调度；不要复制到新聊天窗口：

```
{}
```

---
💡 先执行 `pnpm codex-room refresh-agents` 和 `agents`，确认存在 dispatchable target 后再 submit。"#,
            request.task_type,
            request.items.len(),
            prompt
        );
        let conversation_message = popup_message.clone();

        let request_id = generate_request_id();
        let popup_request = PopupRequest {
            id: request_id.clone(),
            message: popup_message,
            predefined_options: Some(vec!["已复制，开始执行".to_string(), "取消".to_string()]),
            is_markdown: true,
            project_path: request.project_path.clone(),
            codex_home: None,
            codex_thread_id: None,
            codex_thread_provenance: None,
            codex_deeplink: None,
            checkpoint_id: None,
            checkpoint_commit: None,
            checkpoint_message: None,
            link_url: None,
            link_title: None,
            browser_ai_response: None,
        };

        match create_tauri_popup(&popup_request) {
            Ok(response) => {
                if response.trim() != "CANCELLED" && response.trim() != "用户取消了操作" {
                    append_conversation_log(&ConversationEntry {
                        conversation_id: None,
                        current_node_id: None,
                        timeline_route_id: None,
                        run_id: None,
                        generation: None,
                        stale_of: None,
                        superseded_by: None,
                        ai_message: conversation_message,
                        user_response: response.clone(),
                        project_path: request.project_path.clone(),
                        image_count: 0,
                        file_paths: vec![],
                        image_paths: vec![],
                        selected_options: vec![],
                        request_id: Some(request_id),
                        checkpoint_id: None,
                        checkpoint_commit: None,
                        push_status: None,
                        response_source: Some("dispatch_popup".to_string()),
                        workspace_checkpoint_message: None,
                    });
                }

                let result = format!(
                    "Pai Room 调度草案已显示在寸止窗口\n\n用户响应: {}\n\n草案长度: {} 字符",
                    response,
                    prompt.len()
                );
                Ok(CallToolResult::success(vec![Content::text(result)]))
            }
            Err(e) => {
                // 降级：直接返回提示词
                log_debug!("寸止窗口显示失败，降级返回文本: {}", e);
                let result = format!(
                    r#"**Pai Room 调度草案**（寸止窗口不可用，直接显示）

```markdown
{}
```

**草案长度**: {} 字符"#,
                    prompt,
                    prompt.len()
                );
                Ok(CallToolResult::success(vec![Content::text(result)]))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn room_dispatch_plan_requires_worker_post_then_zhi_return() {
        let request = PaiRequest {
            task_type: "代码审查".to_string(),
            items: vec!["scripts/codex-room.mjs".to_string()],
            project_path: Some("/Users/test/project".to_string()),
            source_file: None,
            target_file: None,
            output_format: None,
            extra_steps: None,
        };

        let plan = DispatchTool::generate_room_dispatch_plan(&request);

        assert!(plan.contains("你是 pai room 的可复用 worker"));
        assert!(plan.contains("完成后按顺序执行两个动作"));
        assert!(plan.contains("pnpm codex-room post"));
        assert!(plan.contains("--project <project_path>"));
        assert!(plan.contains("post 成功后在物理末尾调用 zhi / call_zhi 归队待命"));
        assert!(plan.contains("Codex 内置子代理可在任务边界清晰"));
        assert!(plan.contains("Codex 内置子代理只作为主会话授权的补充分工手段"));
        assert!(plan.contains("输出就行，不用调用 zhi/call_zhi"));
        assert!(plan.contains("仅 pairoom worker 执行 `post -> zhi/call_zhi` 归队协议"));
        assert!(plan.contains("不要绕过 room hub 和主会话汇总"));
        assert!(!plan.contains("post 成功后停止，不要调用 zhi / call_zhi"));
        assert!(!plan.contains("一次性 worker"));
        assert!(!plan.contains("完成后只向 room post"));
    }
}
