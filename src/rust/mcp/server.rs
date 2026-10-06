use anyhow::Result;
use parking_lot::Mutex;
use rmcp::{
    model::*, service::RequestContext, transport::stdio, Error as McpError, RoleServer,
    ServerHandler, ServiceExt,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::codex_deeplink::{
    codex_thread_deeplink, extract_codex_thread_id_from_metas, extract_codex_thread_id_from_value,
};
use super::tools::browser::BrowserProxy;
use super::tools::cron_manage::CronManageTool;
use super::tools::task::TaskTool;
use super::tools::web_fetch::WebFetchTool;
use super::tools::{
    CiTool, DispatchTool, InteractionTool, MemoryTool, PhoneActionTool, PtyExecTool, SmartTool,
};
use super::types::{
    AskSmartFriendRequest, CiRequest, JiyiRequest, PaiRequest, PtyExecRequest, ZhiRequest,
};
use crate::config::load_standalone_config;
use crate::{log_debug, log_important};

/// 需要 zhi 前置确认的工具（写入/危险操作）
const TOOLS_REQUIRING_ZHI: &[&str] = &["ji", "pai", "cron_manage"];

/// zhi 授权有效期（秒）
const ZHI_AUTH_TIMEOUT_SECS: u64 = 300; // 5 分钟

/// 全局状态：记录最后一次 zhi 调用时间
static ZHI_LAST_CALL: std::sync::OnceLock<Arc<Mutex<Option<Instant>>>> = std::sync::OnceLock::new();

fn get_zhi_last_call() -> &'static Arc<Mutex<Option<Instant>>> {
    ZHI_LAST_CALL.get_or_init(|| Arc::new(Mutex::new(None)))
}

#[derive(Clone)]
pub struct ZhiServer {
    enabled_tools: HashMap<String, bool>,
    reload_tools_from_disk: bool,
}

impl Default for ZhiServer {
    fn default() -> Self {
        Self::new()
    }
}

impl ZhiServer {
    pub fn new() -> Self {
        // 尝试加载配置，如果失败则使用默认配置
        let enabled_tools = match load_standalone_config() {
            Ok(config) => config.mcp_config.tools,
            Err(e) => {
                log_important!(warn, "无法加载配置文件，使用默认工具配置: {}", e);
                crate::config::default_mcp_tools()
            }
        };

        Self {
            enabled_tools,
            reload_tools_from_disk: true,
        }
    }

    #[cfg(test)]
    fn with_enabled_tools(enabled_tools: HashMap<String, bool>) -> Self {
        Self {
            enabled_tools,
            reload_tools_from_disk: false,
        }
    }

    /// 检查工具是否启用 - 动态读取最新配置
    fn is_tool_enabled(&self, tool_name: &str) -> bool {
        let default_tools = crate::config::default_mcp_tools();
        if !self.reload_tools_from_disk {
            return self
                .enabled_tools
                .get(tool_name)
                .copied()
                .or_else(|| default_tools.get(tool_name).copied())
                .unwrap_or(false);
        }

        // 每次都重新读取配置，确保获取最新状态
        match load_standalone_config() {
            Ok(config) => {
                let enabled = config
                    .mcp_config
                    .tools
                    .get(tool_name)
                    .copied()
                    .or_else(|| default_tools.get(tool_name).copied())
                    .unwrap_or(false);
                log_debug!("工具 {} 当前状态: {}", tool_name, enabled);
                enabled
            }
            Err(e) => {
                log_important!(warn, "读取配置失败，使用缓存状态: {}", e);
                // 如果读取失败，使用缓存的配置
                self.enabled_tools
                    .get(tool_name)
                    .copied()
                    .or_else(|| default_tools.get(tool_name).copied())
                    .unwrap_or(false)
            }
        }
    }

    fn tool_enabled_or_error(&self, tool_name: &str) -> Result<(), McpError> {
        if self.is_tool_enabled(tool_name) {
            Ok(())
        } else {
            Err(McpError::internal_error(
                format!("{} 工具已被禁用", tool_name),
                None,
            ))
        }
    }
}

impl ServerHandler for ZhiServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(ProtocolVersion::V_2024_11_05)
            .with_server_info(Implementation::new("Zhi-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions("Zhi 智能代码审查工具，支持交互式对话和记忆管理")
    }

    async fn initialize(
        &self,
        _request: InitializeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> Result<ServerInfo, McpError> {
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        use std::borrow::Cow;
        use std::sync::Arc;

        let mut tools = Vec::new();

        // iterate 工具始终可用（必需工具）
        let zhi_schema = serde_json::json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "要显示给用户的消息"
                },
                "predefined_options": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "预定义的选项列表（可选）"
                },
                "is_markdown": {
                    "type": "boolean",
                    "description": "消息是否为Markdown格式，默认为true"
                },
                "project_path": {
                    "type": "string",
                    "description": "当前项目的绝对路径（强烈建议传递，用于在弹窗中显示项目路径）"
                }
            },
            "required": ["message"]
        });

        if let serde_json::Value::Object(schema_map) = zhi_schema {
            tools.push(Tool::new(
                Cow::Borrowed("zhi"),
                Cow::Borrowed("iterate 交互工具（L0 协调者）。用户发送 'zhi' 时必须立即调用，停止一切其他操作。所有对话必经，控制任务流程。支持预定义选项、自由文本输入和图片上传。"),
                Arc::new(schema_map),
            ));
        }

        // 记忆管理工具 - 仅在启用时添加
        if self.is_tool_enabled("ji") {
            let ji_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "操作类型：记忆(添加记忆), 回忆(获取项目信息), 沉淀(写入knowledge), 摘要(添加会话摘要)"
                    },
                    "project_path": {
                        "type": "string",
                        "description": "项目路径（必需）"
                    },
                    "content": {
                        "type": "string",
                        "description": "记忆内容（记忆操作时必需）"
                    },
                    "category": {
                        "type": "string",
                        "description": "记忆分类：rule(规范规则), preference(用户偏好), pattern(最佳实践), context(项目上下文)"
                    }
                },
                "required": ["action", "project_path"]
            });

            if let serde_json::Value::Object(schema_map) = ji_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("ji"),
                    Cow::Borrowed("全局记忆管理工具。支持 4 种 action：回忆/记忆/沉淀/摘要。必须绑定 git 根目录。用于存储开发规范、用户偏好和最佳实践。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // 提示词库搜索工具 - 仅在启用时添加
        if self.is_tool_enabled("ci") {
            let ku_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "directory": {
                        "type": "string",
                        "description": "提示词库目录名（如 ci、git、testing）"
                    },
                    "project_path": {
                        "type": "string",
                        "description": "项目路径（必需）"
                    },
                    "query": {
                        "type": "string",
                        "description": "搜索关键词（可选，用于过滤模板）"
                    }
                },
                "required": ["directory", "project_path"]
            });

            if let serde_json::Value::Object(schema_map) = ku_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("ci"),
                    Cow::Borrowed("提示词库搜索工具。在 .cunzhi-knowledge/prompts/ 中搜索相关模板。触发：用户输入目录名（如 ci、git、testing）。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // 智能助手工具 - 仅在启用时添加
        if self.is_tool_enabled("ask_smart_friend") {
            let smart_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "question": {
                        "type": "string",
                        "description": "要咨询的问题、需要审查的代码方案、或遇到的 Bug 描述"
                    },
                    "project_path": {
                        "type": "string",
                        "description": "当前项目路径（可选）"
                    }
                },
                "required": ["question"]
            });

            if let serde_json::Value::Object(schema_map) = smart_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("ask_smart_friend"),
                    Cow::Borrowed("向更强大的 AI 模型咨询建议。使用场景：1. 代码探索后、执行前 - 请求计划审查；2. 遇到新错误/Bug - 请求调试建议；3. 完成编码后 - 请求代码审查。它能看到整个对话上下文。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // Pai Room 编排工具 - 仅在启用时添加
        if self.is_tool_enabled("pai") {
            let pai_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "task_type": {
                        "type": "string",
                        "description": "任务类型（如：补录回归检查、批量重命名、代码审查）"
                    },
                    "items": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "任务范围列表（显式列表，不用模糊表述）"
                    },
                    "project_path": {
                        "type": "string",
                        "description": "当前项目路径（可选）"
                    },
                    "source_file": {
                        "type": "string",
                        "description": "源文件路径（可选）"
                    },
                    "target_file": {
                        "type": "string",
                        "description": "目标文件路径（可选）"
                    },
                    "output_format": {
                        "type": "string",
                        "description": "输出格式模板（可选，用于指定 room worker 回包格式）"
                    },
                    "extra_steps": {
                        "type": "string",
                        "description": "额外步骤说明（可选）"
                    }
                },
                "required": ["task_type", "items"]
            });

            if let serde_json::Value::Object(schema_map) = pai_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("pai"),
                    Cow::Borrowed("Pai Room 编排工具。生成 pnpm codex-room 调度草案和 worker 回包协议，不创建或派发子代理。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // PTY 执行工具 - 仅在启用时添加
        if self.is_tool_enabled("exec_pty") {
            let pty_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "要执行的 shell 命令"
                    },
                    "cwd": {
                        "type": "string",
                        "description": "命令的工作目录（可选）"
                    },
                    "timeout_ms": {
                        "type": "number",
                        "description": "总体超时时间（毫秒），默认 60000"
                    },
                    "no_output_timeout_ms": {
                        "type": "number",
                        "description": "无输出超时时间（毫秒），默认 30000"
                    },
                    "session_id": {
                        "type": "string",
                        "description": "会话标识符（可选）"
                    }
                },
                "required": ["command"]
            });

            if let serde_json::Value::Object(schema_map) = pty_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("exec_pty"),
                    Cow::Borrowed("在伪终端 (PTY) 中执行 shell 命令。通过代理 Node.js Terminal MCP Server 实现，支持超时控制。"),
                    Arc::new(schema_map),
                ));
            }

            // write_to_pty 工具
            let write_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "run_id": {
                        "type": "string",
                        "description": "运行中的 PTY 会话 ID"
                    },
                    "input": {
                        "type": "string",
                        "description": "要写入的输入内容（如 y/n、密码等）"
                    }
                },
                "required": ["run_id", "input"]
            });
            if let serde_json::Value::Object(schema_map) = write_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("write_to_pty"),
                    Cow::Borrowed("向运行中的 PTY 进程写入输入。用于处理交互式命令（如确认提示、密码输入等）。"),
                    Arc::new(schema_map),
                ));
            }

            // cancel_pty 工具
            let cancel_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "run_id": {
                        "type": "string",
                        "description": "要取消的 PTY 会话 ID"
                    }
                },
                "required": ["run_id"]
            });
            if let serde_json::Value::Object(schema_map) = cancel_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("cancel_pty"),
                    Cow::Borrowed("终止运行中的 PTY 命令。用于停止长时间运行的进程或卡住的命令。"),
                    Arc::new(schema_map),
                ));
            }

            // list_active_runs 工具
            let list_schema = serde_json::json!({
                "type": "object",
                "properties": {},
                "required": []
            });
            if let serde_json::Value::Object(schema_map) = list_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("list_active_runs"),
                    Cow::Borrowed("列出所有活跃的 PTY 会话。返回正在运行的命令列表及其状态。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // web_fetch 工具 - 可通过配置禁用
        if self.is_tool_enabled("web_fetch") {
            let fetch_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "要抓取的网页 URL"
                    },
                    "timeout_secs": {
                        "type": "number",
                        "description": "超时时间（秒），默认 15"
                    },
                    "max_chars": {
                        "type": "number",
                        "description": "最大返回字符数，默认 50000"
                    }
                },
                "required": ["url"]
            });
            if let serde_json::Value::Object(schema_map) = fetch_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("web_fetch"),
                    Cow::Borrowed("抓取网页内容并提取纯文本。支持 HTML 自动转文本、超时控制和内容截断。用于查阅文档、调研信息。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // cron_manage 工具 - 可通过配置禁用
        if self.is_tool_enabled("cron_manage") {
            let cron_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "操作类型: list(列出所有定时任务), add(添加定时任务), remove(移除定时任务)"
                    },
                    "schedule": {
                        "type": "string",
                        "description": "cron 表达式（add 时必填），如 '0 6 * * *' 表示每天6点"
                    },
                    "command": {
                        "type": "string",
                        "description": "要定时执行的 shell 命令（add 时必填）"
                    },
                    "label": {
                        "type": "string",
                        "description": "任务标签，用于标识和删除（add/remove 时使用）"
                    }
                },
                "required": ["action"]
            });
            if let serde_json::Value::Object(schema_map) = cron_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("cron_manage"),
                    Cow::Borrowed("管理系统定时任务（crontab）。支持列出、添加、移除定时任务。可用于设置闹钟、定时脚本、定期清理等。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // task 工具 - 文件持久化任务系统
        if self.is_tool_enabled("task") {
            let task_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "description": "操作类型: list(列出任务), add(添加), update(更新), done(完成), remove(删除)"
                    },
                    "project_path": {
                        "type": "string",
                        "description": "项目路径（必需）"
                    },
                    "task_id": {
                        "type": "string",
                        "description": "任务ID（update/done/remove时必需）"
                    },
                    "subject": {
                        "type": "string",
                        "description": "任务主题（add时必需）"
                    },
                    "status": {
                        "type": "string",
                        "description": "状态: pending/in_progress/done/blocked"
                    },
                    "priority": {
                        "type": "string",
                        "description": "优先级: high/medium/low"
                    },
                    "blocked_by": {
                        "type": "string",
                        "description": "阻塞原因（可选）"
                    }
                },
                "required": ["action", "project_path"]
            });
            if let serde_json::Value::Object(schema_map) = task_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("task"),
                    Cow::Borrowed("文件持久化任务系统。任务存储在 .cunzhi-memory/tasks.json，跨会话持久。支持 list/add/update/done/remove。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // iPhone 合法动作路由工具 - 可通过配置禁用
        if self.is_tool_enabled("phone_action") {
            let phone_action_schema = serde_json::json!({
                "type": "object",
                "properties": {
                    "action": {
                        "type": "string",
                        "enum": ["set_input", "append_input", "set_clipboard", "show_message", "start_voice", "open_url", "open_browser", "share_text", "run_shortcut"],
                        "description": "手机动作。仅支持公开可执行的安全动作：set_input/append_input/set_clipboard/show_message/start_voice/open_url/open_browser/share_text/run_shortcut"
                    },
                    "id": {
                        "type": "string",
                        "description": "动作 ID（可选，不传则 Bridge 自动生成）"
                    },
                    "title": {
                        "type": "string",
                        "description": "动作标题（可选）"
                    },
                    "text": {
                        "type": "string",
                        "description": "文本内容。append_input/set_clipboard/show_message 必填；set_input 可用于设置输入框"
                    },
                    "url": {
                        "type": "string",
                        "description": "open_url/open_browser/share_text 的目标 URL；open_url 允许 http/https/iterate，open_browser/share_text 仅允许 http/https"
                    },
                    "browser": {
                        "type": "string",
                        "enum": ["default", "safari", "chrome", "google"],
                        "description": "open_browser 的目标浏览器，默认 default；chrome 不可用时 iOS 端回退默认浏览器"
                    },
                    "shortcut_name": {
                        "type": "string",
                        "description": "run_shortcut 的快捷指令名称；为降低风险，仅允许名称以 iterate 开头"
                    },
                    "target_device_id": {
                        "type": "string",
                        "description": "目标 iPhone device_id（可选，不传则广播给全部在线 iPhone）"
                    },
                    "wait_for_result_ms": {
                        "type": "number",
                        "description": "等待 iPhone 回执的毫秒数，默认 3000，最大 10000；传 0 表示只投递不等待"
                    }
                },
                "required": ["action"]
            });
            if let serde_json::Value::Object(schema_map) = phone_action_schema {
                tools.push(Tool::new(
                    Cow::Borrowed("phone_action"),
                    Cow::Borrowed("iPhone 合法动作路由工具。把 AI 请求路由成 iOS 允许的动作并返回投递/回执状态；不能控制其他 App、不能接管侧边按钮。"),
                    Arc::new(schema_map),
                ));
            }
        }

        // 浏览器工具 - 动态从 Playwright MCP Server 获取
        if self.is_tool_enabled("browser") {
            match BrowserProxy::list_tools().await {
                Ok(browser_tools) => {
                    log_debug!("[Browser] 获取到 {} 个浏览器工具", browser_tools.len());
                    tools.extend(browser_tools);
                }
                Err(e) => {
                    log_important!(warn, "[Browser] 获取浏览器工具列表失败（跳过）: {}", e);
                }
            }
        }

        log_debug!(
            "返回给客户端的工具列表: {:?}",
            tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );

        Ok(ListToolsResult::with_all_items(tools))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        log_debug!("收到工具调用请求: {}", request.name);

        // 守卫检查：需要 zhi 前置确认的工具
        let tool_name = request.name.as_ref();
        if TOOLS_REQUIRING_ZHI.contains(&tool_name) {
            let last_call = get_zhi_last_call().lock();
            let needs_zhi = match *last_call {
                None => true, // 从未调用过 zhi
                Some(instant) => instant.elapsed() > Duration::from_secs(ZHI_AUTH_TIMEOUT_SECS),
            };
            drop(last_call); // 释放锁

            if needs_zhi {
                log_important!(warn, "工具 {} 需要先调用 zhi 确认", tool_name);
                return Err(McpError::invalid_request(
                    format!(
                        "⚠️ 操作需要确认：请先调用 zhi 工具向用户确认后再执行 {} 操作",
                        tool_name
                    ),
                    None,
                ));
            }
        }

        match request.name.as_ref() {
            "zhi" => {
                let start = Instant::now();
                log_debug!("zhi 工具开始执行");

                // 解析请求参数
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let argument_codex_thread_id = extract_codex_thread_id_from_value(&arguments_value);

                let mut zhi_request: ZhiRequest = serde_json::from_value(arguments_value)
                    .map_err(|e| McpError::invalid_params(format!("参数解析失败: {}", e), None))?;
                let caller_codex_thread_id =
                    extract_codex_thread_id_from_metas(std::iter::once(&context.meta));
                if zhi_request.codex_thread_id.is_none() {
                    zhi_request.codex_thread_id = caller_codex_thread_id
                        .clone()
                        .or_else(|| argument_codex_thread_id.clone())
                        .or_else(|| {
                            crate::ui::live_goal::live_goal_codex_thread_id_for_project(
                                zhi_request.project_path.as_deref(),
                            )
                        });
                }
                // Tool arguments cannot assert a Native history binding. Even when a
                // caller exists, an explicit thread must agree with that caller.
                zhi_request.codex_thread_provenance = if caller_codex_thread_id.as_deref()
                    == zhi_request.codex_thread_id.as_deref()
                    && caller_codex_thread_id.is_some() {
                    Some("caller_meta".to_string())
                } else {
                    None
                };
                if zhi_request.codex_deeplink.is_none() {
                    zhi_request.codex_deeplink = zhi_request
                        .codex_thread_id
                        .as_deref()
                        .and_then(codex_thread_deeplink);
                }
                crate::utils::append_timeline_debug_log(
                    "rust/mcp::server/zhi_context",
                    serde_json::json!({
                        "caller_codex_thread_id": caller_codex_thread_id,
                        "argument_codex_thread_id": argument_codex_thread_id,
                        "effective_codex_thread_id": zhi_request.codex_thread_id,
                        "project_path": zhi_request.project_path,
                    }),
                );

                // 调用 zhi 工具
                let result = InteractionTool::zhi(zhi_request).await;

                if let Err(ref e) = result {
                    log_important!(error, "zhi 工具执行失败: {}", e);
                }

                log_debug!(
                    "zhi 工具执行结束, 耗时={:.2}s",
                    start.elapsed().as_secs_f64()
                );

                // 成功调用后更新时间戳
                if result.is_ok() {
                    let mut last_call = get_zhi_last_call().lock();
                    *last_call = Some(Instant::now());
                    log_debug!("zhi 授权时间戳已更新");
                }

                result
            }
            "ji" => {
                // 检查记忆管理工具是否启用
                if !self.is_tool_enabled("ji") {
                    return Err(McpError::internal_error(
                        "记忆管理工具已被禁用".to_string(),
                        None,
                    ));
                }

                // 解析请求参数
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let ji_request: JiyiRequest = serde_json::from_value(arguments_value)
                    .map_err(|e| McpError::invalid_params(format!("参数解析失败: {}", e), None))?;

                // 调用记忆工具
                MemoryTool::jiyi(ji_request).await
            }
            "pai" => {
                // 检查 Pai Room 编排工具是否启用
                if !self.is_tool_enabled("pai") {
                    return Err(McpError::internal_error(
                        "Pai Room 编排工具已被禁用".to_string(),
                        None,
                    ));
                }

                // 解析请求参数
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let pai_request: PaiRequest = serde_json::from_value(arguments_value)
                    .map_err(|e| McpError::invalid_params(format!("参数解析失败: {}", e), None))?;

                // 调用 Pai Room 编排工具
                DispatchTool::pai(pai_request).await
            }
            "ci" => {
                // 检查提示词库搜索工具是否启用
                if !self.is_tool_enabled("ci") {
                    return Err(McpError::internal_error(
                        "ci 工具已被禁用".to_string(),
                        None,
                    ));
                }

                // 解析请求参数
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let ci_request: CiRequest = serde_json::from_value(arguments_value)
                    .map_err(|e| McpError::invalid_params(format!("参数解析失败: {}", e), None))?;

                // 调用提示词库搜索工具
                CiTool::search_prompts(ci_request).await
            }
            "ask_smart_friend" => {
                // 解析请求参数
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let smart_request: AskSmartFriendRequest = serde_json::from_value(arguments_value)
                    .map_err(|e| McpError::invalid_params(format!("参数解析失败: {}", e), None))?;

                // 调用智能助手工具（需要传递 context 以使用 sampling）
                SmartTool::ask_smart_friend(smart_request, context).await
            }
            "exec_pty" => {
                if !self.is_tool_enabled("exec_pty") {
                    return Err(McpError::internal_error(
                        "exec_pty 工具已被禁用".to_string(),
                        None,
                    ));
                }
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let pty_request: PtyExecRequest = serde_json::from_value(arguments_value)
                    .map_err(|e| McpError::invalid_params(format!("参数解析失败: {}", e), None))?;
                PtyExecTool::exec_pty(pty_request).await
            }
            "write_to_pty" => {
                if !self.is_tool_enabled("exec_pty") {
                    return Err(McpError::internal_error(
                        "PTY 工具已被禁用".to_string(),
                        None,
                    ));
                }
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let req: crate::mcp::types::WriteToPtyRequest =
                    serde_json::from_value(arguments_value).map_err(|e| {
                        McpError::invalid_params(format!("参数解析失败: {}", e), None)
                    })?;
                PtyExecTool::write_to_pty(req).await
            }
            "cancel_pty" => {
                if !self.is_tool_enabled("exec_pty") {
                    return Err(McpError::internal_error(
                        "PTY 工具已被禁用".to_string(),
                        None,
                    ));
                }
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let req: crate::mcp::types::CancelPtyRequest =
                    serde_json::from_value(arguments_value).map_err(|e| {
                        McpError::invalid_params(format!("参数解析失败: {}", e), None)
                    })?;
                PtyExecTool::cancel_pty(req).await
            }
            "list_active_runs" => {
                if !self.is_tool_enabled("exec_pty") {
                    return Err(McpError::internal_error(
                        "PTY 工具已被禁用".to_string(),
                        None,
                    ));
                }
                PtyExecTool::list_active_runs().await
            }
            "web_fetch" => {
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let req: super::tools::web_fetch::fetch::WebFetchRequest =
                    serde_json::from_value(arguments_value).map_err(|e| {
                        McpError::invalid_params(format!("参数解析失败: {}", e), None)
                    })?;
                WebFetchTool::fetch(req).await
            }
            "cron_manage" => {
                self.tool_enabled_or_error("cron_manage")?;
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let req: super::tools::cron_manage::cron::CronManageRequest =
                    serde_json::from_value(arguments_value).map_err(|e| {
                        McpError::invalid_params(format!("参数解析失败: {}", e), None)
                    })?;
                CronManageTool::manage(req).await
            }
            "task" => {
                if !self.is_tool_enabled("task") {
                    return Err(McpError::internal_error(
                        "task 工具已被禁用".to_string(),
                        None,
                    ));
                }
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let task_request: super::tools::task::task::TaskRequest =
                    serde_json::from_value(arguments_value).map_err(|e| {
                        McpError::invalid_params(format!("参数解析失败: {}", e), None)
                    })?;
                TaskTool::handle(task_request).await
            }
            "phone_action" => {
                if !self.is_tool_enabled("phone_action") {
                    return Err(McpError::internal_error(
                        "phone_action 工具已被禁用".to_string(),
                        None,
                    ));
                }
                let arguments_value = request
                    .arguments
                    .map(serde_json::Value::Object)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
                let phone_action_request: super::tools::phone_action::PhoneActionToolRequest =
                    serde_json::from_value(arguments_value).map_err(|e| {
                        McpError::invalid_params(format!("参数解析失败: {}", e), None)
                    })?;
                PhoneActionTool::send(phone_action_request).await
            }
            // 浏览器工具透传（browser_ 前缀的工具都转发给 Playwright）
            name if name.starts_with("browser_") => {
                if !self.is_tool_enabled("browser") {
                    return Err(McpError::internal_error(
                        "浏览器工具已被禁用".to_string(),
                        None,
                    ));
                }
                BrowserProxy::call_tool(name.to_string(), request.arguments).await
            }
            _ => Err(McpError::invalid_request(
                format!("未知的工具: {}", request.name),
                None,
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::ServiceExt;

    #[test]
    fn cron_manage_is_classified_as_zhi_gated_dangerous_tool() {
        assert!(
            TOOLS_REQUIRING_ZHI.contains(&"cron_manage"),
            "cron_manage writes persistent crontab commands and must require zhi approval"
        );
    }

    #[test]
    fn cron_manage_is_disabled_by_default() {
        let tools = crate::config::default_mcp_tools();

        assert_eq!(
            tools.get("cron_manage").copied(),
            Some(false),
            "cron_manage must fail closed unless the user explicitly enables it"
        );
    }

    #[test]
    fn disabled_cron_manage_is_rejected_before_execution() {
        let server = ZhiServer::with_enabled_tools(crate::config::default_mcp_tools());

        assert!(
            server.tool_enabled_or_error("cron_manage").is_err(),
            "cron_manage direct calls must be rejected when the tool is disabled"
        );
    }

    #[test]
    fn enabled_tools_are_allowed_by_execution_guard() {
        let server = ZhiServer::with_enabled_tools(crate::config::default_mcp_tools());

        assert!(
            server.tool_enabled_or_error("task").is_ok(),
            "enabled tools should continue through the execution guard"
        );
    }

    #[tokio::test]
    async fn disabled_cron_manage_direct_mcp_call_is_rejected_after_zhi_auth() {
        let (server_transport, client_transport) = tokio::io::duplex(4096);
        {
            let mut last_call = get_zhi_last_call().lock();
            *last_call = Some(Instant::now());
        }

        let server_handle = tokio::spawn(async move {
            let service = ZhiServer::with_enabled_tools(crate::config::default_mcp_tools())
                .serve(server_transport)
                .await
                .expect("start in-memory zhi server");
            service
                .waiting()
                .await
                .expect("server should close cleanly");
        });
        let client = ().serve(client_transport).await.expect("start test client");
        let result = client
            .call_tool(CallToolRequestParam::new("cron_manage").with_arguments(
                serde_json::Map::from_iter([(
                    "action".to_string(),
                    serde_json::Value::String("list".to_string()),
                )]),
            ))
            .await;

        assert!(
            result.is_err(),
            "cron_manage direct MCP calls must be rejected while disabled, even inside a fresh zhi auth window"
        );

        drop(client);
        server_handle.await.expect("server task should join");
    }
}

/// 启动MCP服务器
pub async fn run_server() -> Result<(), Box<dyn std::error::Error>> {
    // 记录启动信息
    let pid = std::process::id();
    log_important!(info, "[MCP] 服务器启动 PID={}", pid);

    // 创建并运行服务器
    let service = ZhiServer::new().serve(stdio()).await.inspect_err(|e| {
        log_important!(error, "[MCP] 启动服务器失败 PID={}: {}", pid, e);
    })?;

    log_important!(info, "[MCP] 服务器开始监听 PID={}", pid);

    // 等待服务器关闭
    let result = service.waiting().await;

    match &result {
        Ok(_) => {
            log_important!(info, "[MCP] 服务器退出 PID={} result=ok", pid);
        }
        Err(e) => {
            log_important!(error, "[MCP] 服务器退出 PID={} result=err error={}", pid, e);
        }
    }

    result
        .map(|_| ())
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
}
