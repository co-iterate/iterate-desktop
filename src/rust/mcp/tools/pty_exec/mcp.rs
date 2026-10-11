//! PTY 命令执行工具实现
//!
//! 使用 rmcp client 连接到 Node.js Terminal MCP Server，
//! 代理其 exec_pty_command 工具能力

use anyhow::Result;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::Regex;
use rmcp::{model::*, serve_client, transport::TokioChildProcess, Error as McpError, RoleClient};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;

use crate::mcp::types::PtyExecRequest;
use crate::{log_debug, log_important};

// ============================================================
// 高危命令拦截机制
// 基于关键字和正则的危险命令检测
// ============================================================

/// 危险命令关键字列表
static DANGEROUS_KEYWORDS: &[&str] = &[
    "rm -rf /",
    "rm -rf /*",
    "rm -rf ~",
    "rm -rf $HOME",
    "mkfs",
    "> /dev/sd",
    "> /dev/nvme",
    "> /dev/hd",
    "dd if=",
    ":(){:|:&};:", // fork bomb
    "chmod -R 777 /",
    "chown -R",
    "mv /* ",
    "wget | sh",
    "curl | sh",
    "wget | bash",
    "curl | bash",
];

/// 危险命令正则模式
static DANGEROUS_PATTERNS: Lazy<Vec<Regex>> = Lazy::new(|| {
    vec![
        // rm -rf 根目录或通配符
        Regex::new(r"rm\s+(-[a-zA-Z]*f[a-zA-Z]*\s+)?(-[a-zA-Z]*r[a-zA-Z]*\s+)?/\s*$").unwrap(),
        Regex::new(r"rm\s+(-[a-zA-Z]*r[a-zA-Z]*\s+)?(-[a-zA-Z]*f[a-zA-Z]*\s+)?/\s*$").unwrap(),
        // dd 写入设备
        Regex::new(r"dd\s+.*of=/dev/(sd|nvme|hd|vd)[a-z]").unwrap(),
        // mkfs 格式化
        Regex::new(r"mkfs\.[a-z0-9]+\s+/dev/").unwrap(),
        // 危险的重定向到设备
        Regex::new(r">\s*/dev/(sd|nvme|hd|vd)[a-z]").unwrap(),
        // 危险的 sudo 组合
        Regex::new(r"sudo\s+rm\s+(-[a-zA-Z]*)?/").unwrap(),
        // curl/wget 管道到 sh/bash（中间可能有 URL 等内容）
        Regex::new(r"curl\s+.*\|\s*(ba)?sh").unwrap(),
        Regex::new(r"wget\s+.*\|\s*(ba)?sh").unwrap(),
    ]
});

/// 白名单文件路径
fn allowlist_path() -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join(".cunzhi")
        .join("exec-allowlist.json")
}

/// 检查命令是否在白名单中
fn is_allowlisted(command: &str) -> bool {
    let path = allowlist_path();
    if !path.exists() {
        return false;
    }
    if let Ok(data) = std::fs::read_to_string(&path) {
        if let Ok(list) = serde_json::from_str::<Vec<String>>(&data) {
            return list.iter().any(|entry| command.contains(entry.as_str()));
        }
    }
    false
}

/// 检查命令是否包含危险操作（跳过白名单中的命令）
fn is_dangerous_command(command: &str) -> Option<String> {
    // 白名单优先放行
    if is_allowlisted(command) {
        log_debug!("[PtyExec] 命令在白名单中，放行: {}", command);
        return None;
    }

    let cmd_lower = command.to_lowercase();

    // 关键字检测
    for keyword in DANGEROUS_KEYWORDS {
        if cmd_lower.contains(&keyword.to_lowercase()) {
            return Some(format!("匹配危险关键字: {}", keyword));
        }
    }

    // 正则检测
    for pattern in DANGEROUS_PATTERNS.iter() {
        if pattern.is_match(command) {
            return Some(format!("匹配危险模式: {}", pattern.as_str()));
        }
    }

    None
}

/// MCP Client 连接状态（懒加载单例）
type McpClientHandle = Arc<rmcp::service::RunningService<RoleClient, ()>>;

static MCP_CLIENT: Lazy<Mutex<Option<McpClientHandle>>> = Lazy::new(|| Mutex::new(None));

/// 动态推导 Node.js MCP PTY Server 的路径
fn resolve_mcp_pty_server_path() -> Result<PathBuf, McpError> {
    let current_exe = std::env::current_exe().map_err(|e| {
        McpError::internal_error(format!("无法获取当前可执行文件路径: {}", e), None)
    })?;

    // 开发模式: target/debug/xxx 或 target/release/xxx
    // 项目根目录 = current_exe.parent().parent().parent()
    if let Some(parent) = current_exe.parent() {
        let parent_name = parent.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if parent_name == "debug" || parent_name == "release" {
            // 开发模式: 从 target/{debug,release}/binary 回溯到项目根目录
            if let Some(project_root) = parent.parent().and_then(|p| p.parent()) {
                let mcp_server_path = project_root
                    .join("scripts")
                    .join("mcp-pty-server")
                    .join("dist")
                    .join("mcp-pty-server.cjs");
                if mcp_server_path.exists() {
                    log_debug!("[PtyExec] 开发模式，使用路径: {:?}", mcp_server_path);
                    return Ok(mcp_server_path);
                }
            }
        }
    }

    // 生产模式 fallback: 尝试相对于可执行文件的资源目录
    // macOS app bundle: Contents/MacOS/binary -> Contents/Resources/mcp-pty-server.cjs
    if let Some(parent) = current_exe.parent() {
        // macOS .app bundle 结构
        if parent.ends_with("Contents/MacOS") {
            let resources_path = parent
                .parent()
                .map(|p| p.join("Resources").join("mcp-pty-server.cjs"));
            if let Some(ref path) = resources_path {
                if path.exists() {
                    log_debug!("[PtyExec] macOS app bundle 模式，使用路径: {:?}", path);
                    return Ok(path.clone());
                }
            }
        }

        // 通用 fallback: 可执行文件同目录下的 mcp-pty-server.cjs
        let same_dir_path = parent.join("mcp-pty-server.cjs");
        if same_dir_path.exists() {
            log_debug!("[PtyExec] 同目录 fallback，使用路径: {:?}", same_dir_path);
            return Ok(same_dir_path);
        }
    }

    Err(McpError::internal_error(
        format!(
            "无法找到 mcp-pty-server.cjs，当前可执行文件: {:?}",
            current_exe
        ),
        None,
    ))
}

/// PTY 命令执行工具
#[derive(Clone)]
pub struct PtyExecTool;

impl PtyExecTool {
    /// 获取或创建 MCP Client 连接
    async fn get_or_create_client() -> Result<McpClientHandle, McpError> {
        // 先检查是否已有连接
        {
            let guard = MCP_CLIENT.lock();
            if let Some(ref client) = *guard {
                return Ok(Arc::clone(client));
            }
        }

        // 创建新连接
        log_important!(info, "[PtyExec] 正在启动 Node.js MCP Terminal Server...");

        // 动态推导 MCP PTY Server 路径
        let mcp_server_path = resolve_mcp_pty_server_path()?;
        log_important!(
            info,
            "[PtyExec] 使用 MCP Server 路径: {:?}",
            mcp_server_path
        );

        // 使用 node 运行打包后的 cjs 文件
        let mut command = Command::new("node");
        command
            .arg(&mcp_server_path)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());

        // 创建 child process transport（传入 Command，不是 Child）
        let transport = TokioChildProcess::new(command)
            .map_err(|e| McpError::internal_error(format!("创建 transport 失败: {}", e), None))?;

        log_debug!("[PtyExec] 子进程已启动");

        // 使用 serve_client 创建 MCP client（使用 () 作为默认 handler）
        let client = serve_client((), transport)
            .await
            .map_err(|e| McpError::internal_error(format!("连接 MCP Server 失败: {}", e), None))?;

        let client_handle = Arc::new(client);

        // 存储连接
        {
            let mut guard = MCP_CLIENT.lock();
            *guard = Some(Arc::clone(&client_handle));
        }

        log_important!(info, "[PtyExec] Node.js MCP Terminal Server 连接成功");

        Ok(client_handle)
    }

    /// 执行 PTY 命令
    pub async fn exec_pty(request: PtyExecRequest) -> Result<CallToolResult, McpError> {
        log_debug!("[PtyExec] 执行命令: {}", request.command);

        // 高危命令拦截检查
        if let Some(reason) = is_dangerous_command(&request.command) {
            log_important!(warn, "🚨 拦截到高危命令: {} ({})", request.command, reason);

            let error_message = format!(
                "⛔ 此命令太危险，已被系统拦截保护。\n\n\
                被拦截的命令: {}\n\
                拦截原因: {}\n\n\
                请修改命令或手动在终端中执行。\n\
                如果您确信该命令是安全的，请直接在终端中运行。",
                request.command, reason
            );

            return Ok(CallToolResult {
                content: vec![Content::text(error_message)],
                is_error: Some(true),
            });
        }

        // 获取 MCP client
        let client = Self::get_or_create_client().await?;

        // 构建 exec_pty_command 参数
        let mut params = serde_json::Map::new();
        params.insert("command".to_string(), serde_json::json!(request.command));

        if let Some(ref cwd) = request.cwd {
            params.insert("cwd".to_string(), serde_json::json!(cwd));
        }
        if let Some(timeout_ms) = request.timeout_ms {
            params.insert("timeout_ms".to_string(), serde_json::json!(timeout_ms));
        }
        if let Some(no_output_timeout_ms) = request.no_output_timeout_ms {
            params.insert(
                "no_output_timeout_ms".to_string(),
                serde_json::json!(no_output_timeout_ms),
            );
        }
        if let Some(ref session_id) = request.session_id {
            params.insert("session_id".to_string(), serde_json::json!(session_id));
        }

        // 调用远程 MCP tool
        let call_request = CallToolRequestParam {
            name: "exec_pty_command".into(),
            arguments: Some(params),
        };

        let result = client.call_tool(call_request).await.map_err(|e| {
            log_important!(error, "[PtyExec] 调用远程工具失败: {}", e);
            McpError::internal_error(format!("调用 exec_pty_command 失败: {}", e), None)
        })?;

        log_debug!("[PtyExec] 命令执行完成");

        Ok(result)
    }

    /// 向运行中的 PTY 进程写入输入
    pub async fn write_to_pty(
        request: crate::mcp::types::WriteToPtyRequest,
    ) -> Result<CallToolResult, McpError> {
        log_debug!(
            "[PtyExec] 写入 PTY: run_id={}, input={}",
            request.run_id,
            request.input
        );
        let client = Self::get_or_create_client().await?;
        let mut params = serde_json::Map::new();
        params.insert("run_id".to_string(), serde_json::json!(request.run_id));
        params.insert("input".to_string(), serde_json::json!(request.input));
        let call_request = CallToolRequestParam {
            name: "write_to_pty".into(),
            arguments: Some(params),
        };
        let result = client.call_tool(call_request).await.map_err(|e| {
            log_important!(error, "[PtyExec] write_to_pty 失败: {}", e);
            McpError::internal_error(format!("调用 write_to_pty 失败: {}", e), None)
        })?;
        Ok(result)
    }

    /// 取消运行中的 PTY 命令
    pub async fn cancel_pty(
        request: crate::mcp::types::CancelPtyRequest,
    ) -> Result<CallToolResult, McpError> {
        log_debug!("[PtyExec] 取消 PTY: run_id={}", request.run_id);
        let client = Self::get_or_create_client().await?;
        let mut params = serde_json::Map::new();
        params.insert("run_id".to_string(), serde_json::json!(request.run_id));
        let call_request = CallToolRequestParam {
            name: "cancel_pty_command".into(),
            arguments: Some(params),
        };
        let result = client.call_tool(call_request).await.map_err(|e| {
            log_important!(error, "[PtyExec] cancel_pty 失败: {}", e);
            McpError::internal_error(format!("调用 cancel_pty_command 失败: {}", e), None)
        })?;
        Ok(result)
    }

    /// 列出所有活跃的 PTY 会话
    pub async fn list_active_runs() -> Result<CallToolResult, McpError> {
        log_debug!("[PtyExec] 列出活跃会话");
        let client = Self::get_or_create_client().await?;
        let call_request = CallToolRequestParam {
            name: "list_active_runs".into(),
            arguments: Some(serde_json::Map::new()),
        };
        let result = client.call_tool(call_request).await.map_err(|e| {
            log_important!(error, "[PtyExec] list_active_runs 失败: {}", e);
            McpError::internal_error(format!("调用 list_active_runs 失败: {}", e), None)
        })?;
        Ok(result)
    }

    /// 关闭 MCP Client 连接（可选，用于清理）
    #[allow(dead_code)]
    pub async fn shutdown() {
        let mut guard = MCP_CLIENT.lock();
        if guard.is_some() {
            log_important!(info, "[PtyExec] 关闭 MCP Client 连接");
            *guard = None;
        }
    }
}
