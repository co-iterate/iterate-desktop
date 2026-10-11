//! Web Fetch 工具实现
//!
//! 原生 Rust 实现的网页抓取工具，支持 HTML→纯文本提取、超时控制和内容截断。

use rmcp::{model::*, Error as McpError};

use crate::{log_debug, log_important};

/// Web Fetch 请求
#[derive(Debug, serde::Deserialize)]
pub struct WebFetchRequest {
    pub url: String,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_chars")]
    pub max_chars: usize,
}

fn default_timeout() -> u64 {
    15
}
fn default_max_chars() -> usize {
    50000
}

pub struct WebFetchTool;

impl WebFetchTool {
    pub async fn fetch(request: WebFetchRequest) -> Result<CallToolResult, McpError> {
        log_debug!("[WebFetch] 抓取 URL: {}", request.url);

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(request.timeout_secs))
            .user_agent("Mozilla/5.0 (compatible; CunZhi/1.0; +https://cunzhi.ai)")
            .build()
            .map_err(|e| McpError::internal_error(format!("创建 HTTP 客户端失败: {}", e), None))?;

        let response = client.get(&request.url).send().await.map_err(|e| {
            log_important!(warn, "[WebFetch] 请求失败: {}", e);
            McpError::internal_error(format!("HTTP 请求失败: {}", e), None)
        })?;

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("unknown")
            .to_string();

        if !status.is_success() {
            return Ok(CallToolResult {
                content: vec![Content::text(format!(
                    "HTTP 请求失败: {} {}\nURL: {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("Unknown"),
                    request.url
                ))],
                is_error: Some(true),
            });
        }

        let body = response
            .text()
            .await
            .map_err(|e| McpError::internal_error(format!("读取响应体失败: {}", e), None))?;

        // 简单的 HTML→纯文本提取
        let text = if content_type.contains("html") {
            strip_html_tags(&body)
        } else {
            body
        };

        // 截断过长内容
        let truncated = if text.len() > request.max_chars {
            format!(
                "{}\n\n--- 内容已截断（共 {} 字符，显示前 {} 字符）---",
                &text[..request.max_chars],
                text.len(),
                request.max_chars
            )
        } else {
            text
        };

        let result_json = serde_json::json!({
            "url": request.url,
            "status": status.as_u16(),
            "content_type": content_type,
            "content_length": truncated.len(),
            "content": truncated
        });

        log_debug!("[WebFetch] 抓取完成: {} 字符", truncated.len());

        Ok(CallToolResult {
            content: vec![Content::text(result_json.to_string())],
            is_error: None,
        })
    }
}

/// 简单的 HTML 标签剥离（移除标签、脚本、样式，保留文本）
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut tag_buf = String::new();

    for ch in html.chars() {
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
            continue;
        }
        if ch == '>' && in_tag {
            in_tag = false;
            let tag_lower = tag_buf.to_lowercase();
            if tag_lower.starts_with("script") {
                in_script = true;
            } else if tag_lower.starts_with("/script") {
                in_script = false;
            } else if tag_lower.starts_with("style") {
                in_style = true;
            } else if tag_lower.starts_with("/style") {
                in_style = false;
            }
            // 在块级标签后插入换行
            if tag_lower.starts_with("br")
                || tag_lower.starts_with("p")
                || tag_lower.starts_with("/p")
                || tag_lower.starts_with("/div")
                || tag_lower.starts_with("/h")
                || tag_lower.starts_with("/li")
                || tag_lower.starts_with("/tr")
            {
                result.push('\n');
            }
            continue;
        }
        if in_tag {
            tag_buf.push(ch);
            continue;
        }
        if in_script || in_style {
            continue;
        }
        result.push(ch);
    }

    // 压缩连续空白行
    let mut cleaned = String::with_capacity(result.len());
    let mut prev_blank = false;
    for line in result.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank {
                cleaned.push('\n');
                prev_blank = true;
            }
        } else {
            cleaned.push_str(trimmed);
            cleaned.push('\n');
            prev_blank = false;
        }
    }

    cleaned.trim().to_string()
}
