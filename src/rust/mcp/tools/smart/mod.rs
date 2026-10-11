//! ask_smart_friend 工具模块
//!
//! 通过 MCP Sampling 协议，让 IDE 代为调用更强的 AI 模型进行咨询

use rmcp::{
    model::{
        CallToolResult, Content, ContextInclusion, CreateMessageRequestParam, Role, SamplingMessage,
    },
    service::RequestContext,
    RoleServer,
};

use crate::mcp::types::AskSmartFriendRequest;
use crate::{log_debug, log_important};

pub struct SmartTool;

impl SmartTool {
    pub async fn ask_smart_friend(
        request: AskSmartFriendRequest,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, rmcp::Error> {
        log_debug!("ask_smart_friend 开始执行, question={}", request.question);

        // 构建发送给 IDE 的 sampling 请求
        // IDE 会用自己的 API key 调用更强的模型（如 Claude Opus）
        let params = CreateMessageRequestParam {
            messages: vec![SamplingMessage {
                role: Role::User,
                content: Content::text(request.question.clone()),
            }],
            // 不指定模型偏好，让 IDE 自己决定使用哪个模型
            model_preferences: None,
            // 系统提示：告诉模型它的角色
            system_prompt: Some(
                "你是一个经验丰富的高级工程师，正在帮助另一个 AI 助手解决问题。\
                请提供简洁、准确、可操作的建议。"
                    .to_string(),
            ),
            // 包含当前对话上下文（IDE 会自动注入）
            include_context: Some(ContextInclusion::AllServers),
            temperature: None,
            max_tokens: 4096,
            stop_sequences: None,
            metadata: None,
        };

        // 通过 MCP Sampling 协议发送请求给 IDE
        let result = context.peer.create_message(params).await.map_err(|e| {
            log_important!(error, "ask_smart_friend sampling 请求失败: {}", e);
            rmcp::Error::internal_error(format!("调用智能助手失败: {}", e), None)
        })?;

        log_debug!("ask_smart_friend 收到响应, model={}", result.model);

        // 提取响应文本
        let response_text = result
            .message
            .content
            .as_text()
            .map(|t| t.text.clone())
            .unwrap_or_else(|| "[智能助手返回了非文本内容]".to_string());

        // 格式化输出
        let output = format!(
            "## 智能助手的建议\n\n{}\n\n---\n*模型: {}*",
            response_text, result.model
        );

        Ok(CallToolResult {
            content: vec![Content::text(output)],
            is_error: Some(false),
        })
    }
}
