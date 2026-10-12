//! Cron 定时任务管理工具
//!
//! 通过操作系统 crontab 实现定时任务的增删查。

use rmcp::{model::*, Error as McpError};
use std::process::Command;

use crate::{log_debug, log_important};

#[derive(Debug, serde::Deserialize)]
pub struct CronManageRequest {
    pub action: String, // "list" | "add" | "remove"
    #[serde(default)]
    pub schedule: Option<String>, // cron 表达式，如 "0 6 * * *"
    #[serde(default)]
    pub command: Option<String>, // 要执行的命令
    #[serde(default)]
    pub label: Option<String>, // 任务标签（用于标识和删除）
}

pub struct CronManageTool;

impl CronManageTool {
    pub async fn manage(request: CronManageRequest) -> Result<CallToolResult, McpError> {
        log_debug!("[Cron] 操作: {}", request.action);

        match request.action.as_str() {
            "list" => Self::list_jobs().await,
            "add" => Self::add_job(request).await,
            "remove" => Self::remove_job(request).await,
            _ => Ok(CallToolResult {
                content: vec![Content::text(format!(
                    "未知操作: {}。支持的操作: list, add, remove",
                    request.action
                ))],
                is_error: Some(true),
            }),
        }
    }

    async fn list_jobs() -> Result<CallToolResult, McpError> {
        let output = Command::new("crontab")
            .arg("-l")
            .output()
            .map_err(|e| McpError::internal_error(format!("执行 crontab -l 失败: {}", e), None))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() && stderr.contains("no crontab") {
            return Ok(CallToolResult {
                content: vec![Content::text(
                    serde_json::json!({ "jobs": [], "message": "当前没有定时任务" }).to_string(),
                )],
                is_error: None,
            });
        }

        let jobs: Vec<serde_json::Value> = stdout
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .enumerate()
            .map(|(i, line)| {
                let label = if line.contains("# ") {
                    line.rsplit("# ").next().unwrap_or("").trim().to_string()
                } else {
                    String::new()
                };
                serde_json::json!({
                    "index": i,
                    "entry": line.trim(),
                    "label": label
                })
            })
            .collect();

        Ok(CallToolResult {
            content: vec![Content::text(
                serde_json::json!({ "jobs": jobs, "count": jobs.len() }).to_string(),
            )],
            is_error: None,
        })
    }

    async fn add_job(request: CronManageRequest) -> Result<CallToolResult, McpError> {
        let schedule = request
            .schedule
            .ok_or_else(|| McpError::invalid_params("缺少 schedule 参数".to_string(), None))?;
        let command = request
            .command
            .ok_or_else(|| McpError::invalid_params("缺少 command 参数".to_string(), None))?;
        let label = request.label.unwrap_or_else(|| "cunzhi".to_string());

        let new_entry = format!("{} {} # {}", schedule, command, label);

        // 读取现有 crontab
        let existing = Command::new("crontab").arg("-l").output().ok();
        let mut entries: Vec<String> = existing
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.to_string())
            .collect();

        // 如果已有相同标签，先移除
        entries.retain(|l| !l.contains(&format!("# {}", label)));
        entries.push(new_entry.clone());

        // 写回 crontab
        let joined = entries.join("\n") + "\n";
        let mut child = Command::new("crontab")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| McpError::internal_error(format!("启动 crontab 失败: {}", e), None))?;

        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(joined.as_bytes())
            .map_err(|e| McpError::internal_error(format!("写入 crontab 失败: {}", e), None))?;

        child
            .wait()
            .map_err(|e| McpError::internal_error(format!("等待 crontab 失败: {}", e), None))?;

        log_important!(info, "[Cron] 添加定时任务: {}", new_entry);

        Ok(CallToolResult {
            content: vec![Content::text(
                serde_json::json!({
                    "success": true,
                    "entry": new_entry,
                    "message": format!("已添加定时任务: {}", label)
                })
                .to_string(),
            )],
            is_error: None,
        })
    }

    async fn remove_job(request: CronManageRequest) -> Result<CallToolResult, McpError> {
        let label = request
            .label
            .ok_or_else(|| McpError::invalid_params("缺少 label 参数".to_string(), None))?;

        let existing = Command::new("crontab").arg("-l").output().ok();
        let entries_before: Vec<String> = existing
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| l.to_string())
            .collect();

        let count_before = entries_before.len();
        let entries_after: Vec<String> = entries_before
            .into_iter()
            .filter(|l| !l.contains(&format!("# {}", label)))
            .collect();
        let removed = count_before - entries_after.len();

        if removed == 0 {
            return Ok(CallToolResult {
                content: vec![Content::text(
                    serde_json::json!({
                        "success": false,
                        "message": format!("未找到标签为 '{}' 的定时任务", label)
                    })
                    .to_string(),
                )],
                is_error: Some(true),
            });
        }

        let joined = if entries_after.is_empty() {
            String::new()
        } else {
            entries_after.join("\n") + "\n"
        };

        let mut child = Command::new("crontab")
            .arg("-")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| McpError::internal_error(format!("启动 crontab 失败: {}", e), None))?;

        use std::io::Write;
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(joined.as_bytes())
            .map_err(|e| McpError::internal_error(format!("写入 crontab 失败: {}", e), None))?;

        child
            .wait()
            .map_err(|e| McpError::internal_error(format!("等待 crontab 失败: {}", e), None))?;

        log_important!(info, "[Cron] 移除定时任务: {} ({}条)", label, removed);

        Ok(CallToolResult {
            content: vec![Content::text(
                serde_json::json!({
                    "success": true,
                    "removed": removed,
                    "message": format!("已移除 {} 条标签为 '{}' 的定时任务", removed, label)
                })
                .to_string(),
            )],
            is_error: None,
        })
    }
}
