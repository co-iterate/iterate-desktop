//! 浏览器控制代理
//!
//! 透传 @playwright/mcp 的所有工具，不硬编码 schema。
//! 架构：cunzhi MCP Server → 启动 Playwright MCP Server 子进程 → 代理其工具

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use rmcp::{model::*, serve_client, transport::TokioChildProcess, Error as McpError, RoleClient};
use std::sync::Arc;
use tokio::process::Command;

use crate::{log_debug, log_important};

type McpClientHandle = Arc<rmcp::service::RunningService<RoleClient, ()>>;

static BROWSER_CLIENT: Lazy<Mutex<Option<McpClientHandle>>> = Lazy::new(|| Mutex::new(None));

/// 动态推导 Playwright MCP CLI 路径
fn resolve_playwright_mcp_path() -> Result<std::path::PathBuf, McpError> {
    let current_exe = std::env::current_exe().map_err(|e| {
        McpError::internal_error(format!("无法获取当前可执行文件路径: {}", e), None)
    })?;

    if let Some(parent) = current_exe.parent() {
        let dir_name = parent.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if dir_name == "debug" || dir_name == "release" {
            if let Some(project_root) = parent.parent().and_then(|p| p.parent()) {
                let path = project_root
                    .join("scripts")
                    .join("mcp-browser-server")
                    .join("node_modules")
                    .join("@playwright")
                    .join("mcp")
                    .join("cli.js");
                if path.exists() {
                    return Ok(path);
                }
            }
        }
    }

    Err(McpError::internal_error(
        "无法找到 @playwright/mcp cli.js".to_string(),
        None,
    ))
}

pub struct BrowserProxy;

impl BrowserProxy {
    async fn get_or_create_client() -> Result<McpClientHandle, McpError> {
        {
            let guard = BROWSER_CLIENT.lock();
            if let Some(ref client) = *guard {
                return Ok(client.clone());
            }
        }

        log_important!(info, "[Browser] 启动 Playwright MCP Server...");

        let cli_path = resolve_playwright_mcp_path()?;
        log_debug!("[Browser] Playwright CLI 路径: {:?}", cli_path);

        let mut command = Command::new("node");
        command
            .arg(&cli_path)
            .arg("--headless")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit());

        let transport = TokioChildProcess::new(command)
            .map_err(|e| McpError::internal_error(format!("创建传输层失败: {}", e), None))?;

        let client_handle = Arc::new(
            serve_client((), transport)
                .await
                .map_err(|e| McpError::internal_error(format!("MCP 握手失败: {}", e), None))?,
        );

        {
            let mut guard = BROWSER_CLIENT.lock();
            *guard = Some(client_handle.clone());
        }

        log_important!(info, "[Browser] Playwright MCP Server 连接成功");
        Ok(client_handle)
    }

    /// 获取 Playwright MCP Server 的所有工具列表
    /// 过滤掉 inputSchema.type 不为 "object" 的工具（MCP 规范要求）
    pub async fn list_tools() -> Result<Vec<Tool>, McpError> {
        let client = Self::get_or_create_client().await?;
        let result = client.list_tools(None).await.map_err(|e| {
            McpError::internal_error(format!("获取浏览器工具列表失败: {}", e), None)
        })?;
        // 修复 schema：确保每个工具的 inputSchema.type 都是 "object"（MCP 规范要求）
        let fixed_tools: Vec<Tool> = result
            .tools
            .into_iter()
            .map(|mut tool| {
                let mut schema = (*tool.input_schema).clone();
                match schema.get("type").and_then(|v| v.as_str()) {
                    Some("object") => {} // 已经正确
                    _ => {
                        // 强制设为 object，把原 schema 包装进 properties（如果还没有的话）
                        schema.insert("type".to_string(), serde_json::json!("object"));
                        if !schema.contains_key("properties") {
                            schema.insert("properties".to_string(), serde_json::json!({}));
                        }
                    }
                }
                tool.input_schema = Arc::new(schema);
                tool
            })
            .collect();
        log_debug!("[Browser] 修复后返回 {} 个浏览器工具", fixed_tools.len());
        Ok(fixed_tools)
    }

    /// 透传调用 Playwright MCP Server 的工具
    pub async fn call_tool(
        name: String,
        arguments: Option<serde_json::Map<String, serde_json::Value>>,
    ) -> Result<CallToolResult, McpError> {
        log_debug!("[Browser] 透传调用: {}", name);
        let client = Self::get_or_create_client().await?;
        let call_request = CallToolRequestParam {
            name: name.into(),
            arguments,
        };
        let result = client.call_tool(call_request).await.map_err(|e| {
            log_important!(error, "[Browser] 调用失败: {}", e);
            McpError::internal_error(format!("浏览器工具调用失败: {}", e), None)
        })?;
        Ok(result)
    }
}
