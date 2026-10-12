use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
#[cfg(target_os = "windows")]
use std::os::windows::process::CommandExt;
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::RwLock;

use super::detector::AiCompletionEvent;
use super::websocket::{
    ensure_browser_ws_pairing_token, send_to_browser, start_ws_server, stop_ws_server,
};
use crate::mcp::handlers::create_tauri_popup;
use crate::mcp::types::{McpResponse, PopupRequest};
use crate::mcp::utils::generate_request_id;

const BROWSER_PHONE_NOTIFY_URL: &str = "http://127.0.0.1:8080/api/apns/notify";

static BROWSER_PHONE_NOTIFY_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("browser phone notification client should build")
});

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

fn browser_phone_notification_payload(
    request: &PopupRequest,
    event: &AiCompletionEvent,
) -> serde_json::Value {
    let body = if event.message_preview.trim().is_empty() {
        request.message.clone()
    } else {
        format!("{}\n\n{}", request.message, event.message_preview)
    };

    serde_json::json!({
        "title": format!("{} 回复完成", event.site_name),
        "body": body,
        "project_path": null,
        "request_id": null,
        "predefined_options": [],
        "is_markdown": request.is_markdown,
        "codex_thread_id": null,
        "codex_deeplink": null,
        "loop_active": false,
        "force_popup": false,
        "source": "browser_ai_completed",
    })
}

fn schedule_browser_phone_notification(request: &PopupRequest, event: &AiCompletionEvent) {
    let payload = browser_phone_notification_payload(request, event);
    let request_id = request.id.clone();
    tokio::spawn(async move {
        let retry_delays = [0_u64, 200, 600, 1200];
        for (attempt_index, delay_ms) in retry_delays.into_iter().enumerate() {
            if delay_ms > 0 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            let request = BROWSER_PHONE_NOTIFY_CLIENT
                .post(BROWSER_PHONE_NOTIFY_URL)
                .json(&payload);
            let request = match crate::bridge::auth::authorize_internal_bridge_request(
                request,
                "POST",
                BROWSER_PHONE_NOTIFY_URL,
            ) {
                Ok(request) => request,
                Err(error) => {
                    log::warn!(
                        "[Browser Phone] 内部鉴权失败: request_id={}, attempt={}, error={}",
                        request_id,
                        attempt_index + 1,
                        error
                    );
                    continue;
                }
            };
            match request.send().await {
                Ok(response) if response.status().is_success() => {
                    log::info!(
                        "[Browser Phone] 通知已交给 Bridge: request_id={}, attempt={}",
                        request_id,
                        attempt_index + 1
                    );
                    return;
                }
                Ok(response) => log::warn!(
                    "[Browser Phone] Bridge 拒绝通知: request_id={}, attempt={}, status={}",
                    request_id,
                    attempt_index + 1,
                    response.status()
                ),
                Err(error) => log::warn!(
                    "[Browser Phone] 通知请求失败: request_id={}, attempt={}, error={}",
                    request_id,
                    attempt_index + 1,
                    error
                ),
            }
        }
    });
}

#[derive(Debug, PartialEq, Eq)]
enum BrowserPopupAction {
    Open,
    Ignore,
    Continue,
    Loop,
    Send(String),
}

fn browser_popup_action(response: &str, should_offer_continue: bool) -> BrowserPopupAction {
    let trimmed = response.trim();
    if trimmed.is_empty() || matches!(trimmed, "CANCELLED" | "用户取消了操作") {
        return BrowserPopupAction::Ignore;
    }

    if let Ok(parsed) = serde_json::from_str::<McpResponse>(trimmed) {
        if parsed
            .selected_options
            .iter()
            .any(|option| option.contains("打开"))
        {
            return BrowserPopupAction::Open;
        }
        if parsed
            .selected_options
            .iter()
            .any(|option| option.contains("忽略") || option.contains("取消"))
        {
            return BrowserPopupAction::Ignore;
        }
        if should_offer_continue
            && parsed
                .selected_options
                .iter()
                .any(|option| option.contains("循环"))
        {
            return BrowserPopupAction::Loop;
        }
        if should_offer_continue
            && (parsed
                .metadata
                .source
                .as_deref()
                .is_some_and(|source| source.contains("continue"))
                || parsed
                    .selected_options
                    .iter()
                    .any(|option| option.contains("继续")))
        {
            return BrowserPopupAction::Continue;
        }

        let mut parts = Vec::new();
        if let Some(input) = parsed.user_input.map(|input| input.trim().to_string()) {
            if !input.is_empty() {
                parts.push(input);
            }
        }
        let other_options: Vec<_> = parsed
            .selected_options
            .into_iter()
            .filter(|option| {
                !["打开", "忽略", "取消", "继续", "循环"]
                    .iter()
                    .any(|control| option.contains(control))
            })
            .collect();
        if !other_options.is_empty() {
            parts.push(other_options.join("，"));
        }
        return if parts.is_empty() {
            BrowserPopupAction::Ignore
        } else {
            BrowserPopupAction::Send(parts.join("\n\n"))
        };
    }

    if trimmed.contains("打开") {
        BrowserPopupAction::Open
    } else if trimmed.contains("忽略") || trimmed.contains("取消") {
        BrowserPopupAction::Ignore
    } else if should_offer_continue && trimmed.contains("循环") {
        BrowserPopupAction::Loop
    } else if should_offer_continue && trimmed.contains("继续") {
        BrowserPopupAction::Continue
    } else {
        BrowserPopupAction::Send(trimmed.to_string())
    }
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

                            let should_offer_continue = is_closing_text(&event.message_preview);
                            let predefined_options = if should_offer_continue {
                                vec![
                                    "继续".to_string(),
                                    "循环".to_string(),
                                    "打开页面".to_string(),
                                    "忽略".to_string(),
                                ]
                            } else {
                                vec!["打开页面".to_string(), "忽略".to_string()]
                            };

                            // 创建弹窗通知（像 cunzhi 那样）- 在独立线程中运行避免阻塞
                            let popup_request = PopupRequest {
                                id: generate_request_id(),
                                message,
                                predefined_options: Some(predefined_options),
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
                                browser_tab_id: event.tab_id,
                            };

                            schedule_browser_phone_notification(&popup_request, &event);

                            let url = event.url.clone();
                            let browser_tab_id = event.tab_id;
                            let expected_url = event.url.clone();
                            tokio::spawn(async move {
                                let popup_result = tokio::task::spawn_blocking(move || {
                                    create_tauri_popup(&popup_request)
                                })
                                .await;
                                let response = match popup_result {
                                    Ok(Ok(r)) => r,
                                    _ => return,
                                };

                                let message =
                                    match browser_popup_action(&response, should_offer_continue) {
                                        BrowserPopupAction::Open => {
                                            #[cfg(target_os = "macos")]
                                            {
                                                let _ = std::process::Command::new("open")
                                                    .arg(&url)
                                                    .spawn();
                                            }
                                            return;
                                        }
                                        BrowserPopupAction::Ignore => return,
                                        BrowserPopupAction::Continue => {
                                            let continue_prompt =
                                                crate::config::load_standalone_config()
                                                    .map(|c| c.reply_config.continue_prompt)
                                                    .unwrap_or_else(|_| {
                                                        "请按照最佳实践继续".to_string()
                                                    });
                                            continue_prompt
                                        }
                                        BrowserPopupAction::Loop => {
                                            let loop_prompt =
                                                crate::config::load_standalone_config()
                                                    .map(|c| c.reply_config.loop_prompt)
                                                    .unwrap_or_else(|_| {
                                                        crate::constants::mcp::DEFAULT_LOOP_PROMPT
                                                            .to_string()
                                                    });
                                            loop_prompt
                                        }
                                        BrowserPopupAction::Send(message) => message,
                                    };
                                let _ =
                                    send_to_browser(message, browser_tab_id, Some(expected_url))
                                        .await;
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
    // WebSocket 模式下简化状态返回
    Ok(BrowserMonitorStatus {
        connected: true,
        monitoring: true,
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
pub async fn send_message_to_browser_ai(
    message: String,
    tab_id: Option<u32>,
    expected_url: Option<String>,
) -> Result<String, String> {
    log::info!(
        "[DEBUG] send_message_to_browser_ai 命令被调用, message: {}",
        message
    );
    match send_to_browser(message, tab_id, expected_url).await {
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

#[cfg(test)]
mod tests {
    use super::{
        browser_phone_notification_payload, browser_popup_action, AiCompletionEvent,
        BrowserPopupAction, PopupRequest,
    };

    #[test]
    fn browser_completion_phone_payload_keeps_reply_and_request_identity() {
        let request = PopupRequest {
            id: "browser-request-1".to_string(),
            message: "## ChatGPT AI 完成\n\n**标题**: 原 Chat".to_string(),
            predefined_options: Some(vec!["打开页面".to_string(), "忽略".to_string()]),
            is_markdown: true,
            project_path: Some("/Users/example/cunzhi".to_string()),
            codex_home: None,
            codex_thread_id: None,
            codex_deeplink: None,
            checkpoint_id: None,
            checkpoint_commit: None,
            checkpoint_message: None,
            link_url: Some("https://chatgpt.com/c/original".to_string()),
            link_title: Some("原 Chat".to_string()),
            browser_ai_response: Some("完整回答".to_string()),
            browser_tab_id: Some(42),
        };
        let event = AiCompletionEvent {
            url: "https://chatgpt.com/c/original".to_string(),
            title: "原 Chat".to_string(),
            site_name: "ChatGPT".to_string(),
            message_preview: "完整回答".to_string(),
            tab_id: Some(42),
            timestamp: chrono::Utc::now(),
            run_time: None,
            think_time: None,
            image_generated: false,
            new_images: None,
        };

        let payload = browser_phone_notification_payload(&request, &event);
        assert!(payload["request_id"].is_null());
        assert!(payload["project_path"].is_null());
        assert_eq!(payload["source"], "browser_ai_completed");
        assert_eq!(payload["predefined_options"].as_array().unwrap().len(), 0);
        assert!(payload["body"].as_str().unwrap().contains("完整回答"));
    }

    #[test]
    fn structured_popup_text_is_sent_back_to_the_bound_chat() {
        let response = serde_json::json!({
            "user_input": "继续检查同一个问题",
            "selected_options": [],
            "images": [],
            "file_paths": [],
            "image_paths": [],
            "metadata": { "source": "popup" }
        })
        .to_string();

        assert_eq!(
            browser_popup_action(&response, true),
            BrowserPopupAction::Send("继续检查同一个问题".to_string())
        );
    }

    #[test]
    fn continue_and_stop_controls_do_not_become_literal_chat_messages() {
        let continue_response = serde_json::json!({
            "user_input": "请按照最佳实践继续",
            "selected_options": [],
            "images": [],
            "file_paths": [],
            "image_paths": [],
            "metadata": { "source": "popup_continue" }
        })
        .to_string();
        let stop_response = serde_json::json!({
            "user_input": null,
            "selected_options": ["忽略"],
            "images": [],
            "file_paths": [],
            "image_paths": [],
            "metadata": { "source": "popup" }
        })
        .to_string();

        assert_eq!(
            browser_popup_action(&continue_response, true),
            BrowserPopupAction::Continue
        );
        assert_eq!(
            browser_popup_action(&stop_response, true),
            BrowserPopupAction::Ignore
        );
    }
}
