use chrono;
use serde::{Deserialize, Serialize};

/// 向运行中的 PTY 进程写入输入
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WriteToPtyRequest {
    #[schemars(description = "运行中的 PTY 会话 ID")]
    pub run_id: String,
    #[schemars(description = "要写入的输入内容（如 y/n、密码等）")]
    pub input: String,
}

/// 取消运行中的 PTY 命令
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CancelPtyRequest {
    #[schemars(description = "要取消的 PTY 会话 ID")]
    pub run_id: String,
}

/// PTY 命令执行请求
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PtyExecRequest {
    #[schemars(description = "要执行的 shell 命令")]
    pub command: String,
    #[schemars(description = "命令的工作目录（可选）")]
    #[serde(default)]
    pub cwd: Option<String>,
    #[schemars(description = "总体超时时间（毫秒），默认 60000")]
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    #[schemars(description = "无输出超时时间（毫秒），默认 30000")]
    #[serde(default)]
    pub no_output_timeout_ms: Option<u64>,
    #[schemars(description = "会话标识符（可选）")]
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ZhiRequest {
    #[schemars(description = "要显示给用户的消息")]
    pub message: String,
    #[schemars(description = "预定义的选项列表（可选）")]
    #[serde(default)]
    pub predefined_options: Vec<String>,
    #[schemars(description = "消息是否为Markdown格式，默认为true")]
    #[serde(default = "default_is_markdown")]
    pub is_markdown: bool,
    #[schemars(description = "当前项目的绝对路径（可选，用于显示项目信息）")]
    #[serde(default)]
    pub project_path: Option<String>,
    #[schemars(
        description = "调用方 Codex home（可选，只传路径，不传 token；默认从 MCP 进程 CODEX_HOME 读取）"
    )]
    #[serde(default)]
    pub codex_home: Option<String>,
    #[schemars(description = "调用本次 MCP 的 Codex 会话 ID（可选，用于回到原会话）")]
    #[serde(default)]
    pub codex_thread_id: Option<String>,
    /// Set only after the MCP producer resolves the real caller or explicit argument.
    #[serde(skip)]
    #[schemars(skip)]
    pub codex_thread_provenance: Option<String>,
    #[schemars(description = "调用本次 MCP 的 Codex 会话 deep link（可选）")]
    #[serde(default)]
    pub codex_deeplink: Option<String>,
    #[schemars(
        description = "是否在返回值中附带对话记录文件路径（可选）。长对话时传 true，AI 被砍旧消息后可用 read_file 找回上下文"
    )]
    #[serde(default)]
    pub compact: Option<bool>,
}

fn default_is_markdown() -> bool {
    true
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct JiyiRequest {
    #[schemars(
        description = "操作类型：记忆(添加记忆), 回忆(获取项目信息), 沉淀(预览待沉淀内容), 确认沉淀(用户确认后执行), 摘要(添加会话摘要)"
    )]
    pub action: String,
    #[schemars(description = "项目路径（必需）")]
    pub project_path: String,
    #[schemars(description = "记忆内容（记忆操作时必需）")]
    #[serde(default)]
    pub content: String,
    #[schemars(
        description = "记忆分类：rule(规范规则), preference(用户偏好), pattern(最佳实践), context(项目上下文)"
    )]
    #[serde(default = "default_category")]
    pub category: String,
}

fn default_category() -> String {
    "context".to_string()
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CiRequest {
    #[schemars(description = "提示词库目录名（如 ci、git、testing），默认为 ci")]
    pub directory: String,
    #[schemars(description = "项目路径（必需）")]
    pub project_path: String,
    #[schemars(description = "搜索关键词（可选，用于过滤模板）")]
    #[serde(default)]
    pub query: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AskSmartFriendRequest {
    #[schemars(description = "要咨询的问题或需要审查的内容")]
    pub question: String,
    #[schemars(description = "当前项目路径（可选）")]
    #[serde(default)]
    pub project_path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct PaiRequest {
    #[schemars(description = "任务类型（如：补录回归检查、批量重命名、代码审查）")]
    pub task_type: String,
    #[schemars(description = "任务范围列表（显式列表，不用模糊表述）")]
    pub items: Vec<String>,
    #[schemars(description = "当前项目路径（可选，用于归档到对应项目的 conversation）")]
    #[serde(default)]
    pub project_path: Option<String>,
    #[schemars(description = "源文件路径（可选）")]
    #[serde(default)]
    pub source_file: Option<String>,
    #[schemars(description = "目标文件路径（可选）")]
    #[serde(default)]
    pub target_file: Option<String>,
    #[schemars(description = "输出格式模板（可选，用于指定 room worker 回包格式）")]
    #[serde(default)]
    pub output_format: Option<String>,
    #[schemars(description = "额外步骤说明（可选）")]
    #[serde(default)]
    pub extra_steps: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PopupRequest {
    pub id: String,
    pub message: String,
    pub predefined_options: Option<Vec<String>>,
    pub is_markdown: bool,
    pub project_path: Option<String>,
    #[serde(default)]
    pub codex_home: Option<String>,
    #[serde(default)]
    pub codex_thread_id: Option<String>,
    #[serde(default)]
    pub codex_thread_provenance: Option<String>,
    #[serde(default)]
    pub codex_deeplink: Option<String>,
    #[serde(default)]
    pub checkpoint_id: Option<String>,
    #[serde(default)]
    pub checkpoint_commit: Option<String>,
    #[serde(default)]
    pub checkpoint_message: Option<String>,
    #[serde(default)]
    pub link_url: Option<String>,
    #[serde(default)]
    pub link_title: Option<String>,
    #[serde(default)]
    pub browser_ai_response: Option<String>,
}

/// 新的结构化响应数据格式
#[derive(Debug, Deserialize)]
pub struct McpResponse {
    pub user_input: Option<String>,
    #[serde(default)]
    pub selected_options: Vec<String>,
    #[serde(default)]
    pub images: Vec<ImageAttachment>,
    #[serde(default)]
    pub file_paths: Vec<String>,
    #[serde(default)]
    pub image_paths: Vec<String>,
    #[serde(default)]
    pub metadata: ResponseMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageAttachment {
    pub data: String,
    pub media_type: String,
    pub filename: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct ResponseMetadata {
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub request_id: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub hui_snapshot: Option<String>,
    #[serde(default)]
    pub conversation_id: Option<String>,
    #[serde(default)]
    pub tree_id: Option<String>,
    #[serde(default)]
    pub current_node_id: Option<String>,
    #[serde(default)]
    pub node_id: Option<String>,
    #[serde(default)]
    pub parent_node_id: Option<String>,
    #[serde(default)]
    pub timeline_route_id: Option<String>,
    #[serde(default)]
    pub conversation_route_id: Option<String>,
    #[serde(default)]
    pub request_key: Option<String>,
    #[serde(default)]
    pub actual_request_id: Option<String>,
    #[serde(default)]
    pub run_id: Option<String>,
    #[serde(default)]
    pub generation: Option<u64>,
    #[serde(default)]
    pub stale_of: Option<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
}

/// 旧格式兼容性支持
#[derive(Debug, Deserialize)]
pub struct McpResponseContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: Option<String>,
    pub source: Option<ImageSource>,
}

#[derive(Debug, Deserialize)]
pub struct ImageSource {
    #[serde(rename = "type")]
    pub source_type: String,
    pub media_type: String,
    pub data: String,
}

/// 统一的响应构建函数
///
/// 用于生成标准的JSON响应格式，确保无GUI和有GUI模式输出一致
pub fn build_mcp_response(
    user_input: Option<String>,
    selected_options: Vec<String>,
    images: Vec<ImageAttachment>,
    request_id: Option<String>,
    source: &str,
) -> serde_json::Value {
    serde_json::json!({
        "user_input": user_input,
        "selected_options": selected_options,
        "images": images,
        "file_paths": [],
        "image_paths": [],
        "metadata": {
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "request_id": request_id,
            "source": source
        }
    })
}

/// 构建发送操作的响应
pub fn build_send_response(
    user_input: Option<String>,
    selected_options: Vec<String>,
    images: Vec<ImageAttachment>,
    request_id: Option<String>,
    source: &str,
) -> String {
    let response = build_mcp_response(user_input, selected_options, images, request_id, source);
    response.to_string()
}

/// 构建继续操作的响应
pub fn build_continue_response(request_id: Option<String>, source: &str) -> String {
    // 动态获取继续提示词
    let continue_prompt = if let Ok(config) = crate::config::load_standalone_config() {
        config.reply_config.continue_prompt
    } else {
        "请按照最佳实践继续".to_string()
    };

    let response = build_mcp_response(Some(continue_prompt), vec![], vec![], request_id, source);
    response.to_string()
}
