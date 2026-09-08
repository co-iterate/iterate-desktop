use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use super::detector::AiCompletionEvent;
use super::websocket::{
    browser_extension_connected, browser_ws_server_running, ensure_browser_ws_pairing_token,
    send_to_browser, start_ws_server, stop_ws_server,
};
use crate::mcp::handlers::create_tauri_popup;
use crate::mcp::types::PopupRequest;
use crate::mcp::utils::generate_request_id;

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn is_closing_text(text: &str) -> bool {
    let t = text.to_lowercase();
    let patterns = [
        "还有什么需要帮助",
        "还有什么可以帮",
        "如果你还有",
        "如有需要",
        "希望能帮到",
        "到这里",
        "就到这",
        "告一段落",
        "wrap up",
        "in summary",
        "hope this helps",
        "anything else",
        "let me know if",
    ];
    patterns.iter().any(|p| t.contains(p))
}

/// 存储最新的 AI 回复
static LATEST_AI_RESPONSE: Lazy<Arc<RwLock<Option<String>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

#[derive(Debug, Serialize, Deserialize)]
pub struct BrowserMonitorStatus {
    pub connected: bool,
    pub monitoring: bool,
}

/// 启动浏览器监控（WebSocket 模式）
#[tauri::command]
pub async fn start_browser_monitoring(
    app: AppHandle,
    _port: Option<u16>,
) -> Result<String, String> {
    match start_ws_server().await {
        Ok(event_tx) => {
            // 启动事件转发到前端
            let app_handle = app.clone();
            let mut receiver = event_tx.subscribe();
            tokio::spawn(async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            log::info!("AI 完成事件: {} - {}", event.site_name, event.url);

                            // 存储最新的 AI 回复
                            if !event.message_preview.is_empty() {
                                let mut latest = LATEST_AI_RESPONSE.write().await;
                                *latest = Some(event.message_preview.clone());
                                log::info!(
                                    "已存储最新 AI 回复，长度: {}",
                                    event.message_preview.len()
                                );
                            }

                            // 发送事件到前端显示在列表中
                            let _ = app_handle.emit("browser-ai-completed", &event);

                            // 构建弹窗消息
                            let mut message = if event.image_generated {
                                format!("## 🖼️ {} 图片生成完成", event.site_name)
                            } else {
                                format!("## {} AI 完成", event.site_name)
                            };

                            message.push_str(&format!("\n\n**标题**: {}", event.title));

                            if let Some(run_time) = event.run_time {
                                message.push_str(&format!("\n**运行时间**: {}秒", run_time));
                            }
                            if let Some(think_time) = event.think_time {
                                message.push_str(&format!("\n**思考时间**: {}秒", think_time));
                            }
                            if let Some(new_images) = event.new_images {
                                message.push_str(&format!("\n**新图片**: {}张", new_images));
                            }

                            // 创建弹窗通知（像 cunzhi 那样）- 在独立线程中运行避免阻塞
                            let popup_request = PopupRequest {
                                id: generate_request_id(),
                                message,
                                predefined_options: Some(vec![
                                    "继续".to_string(),
                                    "循环".to_string(),
                                    "打开页面".to_string(),
                                    "忽略".to_string(),
                                ]),
                                is_markdown: true,
                                project_path: None,
                                codex_home: None,
                                codex_thread_id: None,
                                codex_deeplink: None,
                                checkpoint_id: None,
                                checkpoint_commit: None,
                                checkpoint_message: None,
                                link_url: Some(event.url.clone()),
                                link_title: Some(event.title.clone()),
                                browser_ai_response: if event.message_preview.is_empty() {
                                    None
                                } else {
                                    Some(event.message_preview.clone())
                                },
                            };

                            let url = event.url.clone();
                            let should_offer_continue = is_closing_text(&event.message_preview);
                            tokio::spawn(async move {
                                let popup_request = if should_offer_continue {
                                    popup_request
                                } else {
                                    PopupRequest {
                                        predefined_options: Some(vec![
                                            "打开页面".to_string(),
                                            "忽略".to_string(),
                                        ]),
                                        ..popup_request
                                    }
                                };

                                let popup_result = tokio::task::spawn_blocking(move || {
                                    create_tauri_popup(&popup_request)
                                })
                                .await;
                                let response = match popup_result {
                                    Ok(Ok(r)) => r,
                                    _ => return,
                                };

                                if response.contains("打开") {
                                    #[cfg(target_os = "macos")]
                                    {
                                        let _ =
                                            std::process::Command::new("open").arg(&url).spawn();
                                    }
                                    return;
                                }

                                if should_offer_continue && response.contains("继续") {
                                    let continue_prompt = crate::config::load_standalone_config()
                                        .map(|c| c.reply_config.continue_prompt)
                                        .unwrap_or_else(|_| "请按照最佳实践继续".to_string());
                                    let _ = send_to_browser(continue_prompt).await;
                                } else if should_offer_continue && response.contains("循环") {
                                    let loop_prompt = crate::config::load_standalone_config()
                                        .map(|c| c.reply_config.loop_prompt)
                                        .unwrap_or_else(|_| {
                                            crate::constants::mcp::DEFAULT_LOOP_PROMPT.to_string()
                                        });
                                    let _ = send_to_browser(loop_prompt).await;
                                }
                            });
                        }
                        Err(e) => {
                            log::warn!("事件接收错误: {}, 重新订阅", e);
                            break;
                        }
                    }
                }
            });

            Ok("WebSocket 服务器已启动 (端口 9333)".to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// 停止浏览器监控
#[tauri::command]
pub async fn stop_browser_monitoring() -> Result<String, String> {
    stop_ws_server().await;
    Ok("浏览器监控已停止".to_string())
}

/// 获取浏览器监控状态
#[tauri::command]
pub async fn get_browser_monitor_status() -> Result<BrowserMonitorStatus, String> {
    Ok(BrowserMonitorStatus {
        connected: browser_extension_connected().await,
        monitoring: browser_ws_server_running().await,
    })
}

/// 获取或生成浏览器扩展 WebSocket 配对密钥。
#[tauri::command]
pub async fn get_browser_ws_pairing_token() -> Result<String, String> {
    ensure_browser_ws_pairing_token()
        .map_err(|e| format!("获取 Browser WebSocket 配对密钥失败: {}", e))
}

/// 打开浏览器页面（通过 URL）
#[tauri::command]
pub async fn open_browser_url(url: String) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("打开 URL 失败: {}", e))?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", &url])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("打开 URL 失败: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&url)
            .spawn()
            .map_err(|e| format!("打开 URL 失败: {}", e))?;
    }

    Ok(())
}

fn sanitize_artifact_title(title: Option<&str>) -> String {
    let raw = title
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("html-artifact");
    let without_ext = raw
        .strip_suffix(".html")
        .or_else(|| raw.strip_suffix(".htm"))
        .unwrap_or(raw);
    let mut filename = String::new();

    for ch in without_ext.chars() {
        if ch.is_ascii_alphanumeric() {
            filename.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '-' | '_' | '.') {
            filename.push(ch);
        } else if ch.is_whitespace() {
            filename.push('-');
        }

        if filename.len() >= 80 {
            break;
        }
    }

    let filename = filename.trim_matches(['-', '_', '.']);
    if filename.is_empty() {
        "html-artifact".to_string()
    } else {
        filename.to_string()
    }
}

fn open_file_in_default_browser(path: &std::path::Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("打开 HTML Artifact 失败: {}", e))?;
    }

    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("打开 HTML Artifact 失败: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| format!("打开 HTML Artifact 失败: {}", e))?;
    }

    Ok(())
}

/// 将 HTML Artifact 写入临时文件并用默认浏览器打开。
#[tauri::command]
pub async fn open_html_artifact_in_browser(
    content: String,
    title: Option<String>,
) -> Result<String, String> {
    if content.trim().is_empty() {
        return Err("HTML Artifact 内容为空".to_string());
    }

    let artifacts_dir = std::env::temp_dir().join("iterate-html-artifacts");
    std::fs::create_dir_all(&artifacts_dir)
        .map_err(|e| format!("创建 HTML Artifact 临时目录失败: {}", e))?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| format!("获取当前时间失败: {}", e))?
        .as_millis();
    let filename = format!(
        "{}-{}.html",
        sanitize_artifact_title(title.as_deref()),
        timestamp
    );
    let file_path = artifacts_dir.join(filename);

    std::fs::write(&file_path, content)
        .map_err(|e| format!("写入 HTML Artifact 临时文件失败: {}", e))?;
    open_file_in_default_browser(&file_path)?;

    Ok(file_path.to_string_lossy().to_string())
}

/// 显示 AI 完成通知弹窗
#[tauri::command]
pub async fn show_ai_completion_popup(
    app: AppHandle,
    event: AiCompletionEvent,
) -> Result<(), String> {
    // 发送到前端显示弹窗
    app.emit("show-ai-completion-popup", &event)
        .map_err(|e| format!("发送弹窗事件失败: {}", e))
}

/// 发送消息到浏览器 AI
#[tauri::command]
pub async fn send_message_to_browser_ai(message: String) -> Result<String, String> {
    log::info!(
        "[DEBUG] send_message_to_browser_ai 命令被调用, message: {}",
        message
    );
    match send_to_browser(message).await {
        Ok(_) => {
            log::info!("[DEBUG] 消息发送成功");
            Ok("消息已发送".to_string())
        }
        Err(e) => {
            log::error!("[DEBUG] 消息发送失败: {}", e);
            Err(format!("发送失败: {}", e))
        }
    }
}

/// 获取最新的浏览器 AI 回复
#[tauri::command]
pub async fn get_latest_ai_response() -> Result<Option<String>, String> {
    log::info!("[get_latest_ai_response] 命令被调用");
    let latest = LATEST_AI_RESPONSE.read().await;
    log::info!(
        "[get_latest_ai_response] 返回值: {:?}",
        latest.as_ref().map(|s| s.len())
    );
    Ok(latest.clone())
}
