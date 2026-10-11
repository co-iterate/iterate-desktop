use anyhow::Result;
use serde::{Deserialize, Serialize};

use super::config::{match_ai_site, AiSiteConfig};

/// AI 完成状态
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AiStatus {
    Idle,
    Generating,
    Completed,
    Error(String),
}

/// 页面状态信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageState {
    pub url: String,
    pub title: String,
    pub site_name: String,
    pub status: AiStatus,
    pub last_check: chrono::DateTime<chrono::Utc>,
}

impl PageState {
    pub fn new(url: String, title: String) -> Self {
        let site_name = match_ai_site(&url)
            .map(|c| c.name)
            .unwrap_or_else(|| "Unknown".to_string());

        Self {
            url,
            title,
            site_name,
            status: AiStatus::Idle,
            last_check: chrono::Utc::now(),
        }
    }
}

/// AI 完成检测器 - 使用 JavaScript 注入检测页面状态
pub struct AiCompletionDetector {
    config: Option<AiSiteConfig>,
}

impl AiCompletionDetector {
    pub fn new(url: &str) -> Self {
        Self {
            config: match_ai_site(url),
        }
    }

    pub fn is_supported(&self) -> bool {
        self.config.is_some()
    }

    pub fn get_site_name(&self) -> String {
        self.config
            .as_ref()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Unknown".to_string())
    }

    /// 生成检测 AI 是否正在生成的 JavaScript 代码
    pub fn get_is_generating_script(&self) -> Result<String> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("不支持的网站"))?;

        let script = format!(
            r#"
            (function() {{
                // 检查停止按钮是否存在（表示正在生成）
                const stopButton = document.querySelector('{}');
                if (stopButton && stopButton.offsetParent !== null) {{
                    return true;
                }}
                
                // 检查 typing indicator
                {}
                
                return false;
            }})()
        "#,
            config.stop_button_selector,
            config
                .typing_indicator_selector
                .as_ref()
                .map(|s| format!(
                    r#"
                    const typingIndicator = document.querySelector('{}');
                    if (typingIndicator && typingIndicator.offsetParent !== null) {{
                        return true;
                    }}
                "#,
                    s
                ))
                .unwrap_or_default()
        );

        Ok(script)
    }

    /// 生成获取最后一条 AI 消息的 JavaScript 代码
    pub fn get_last_message_script(&self) -> Result<String> {
        let config = self
            .config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("不支持的网站"))?;

        let script = format!(
            r#"
            (function() {{
                const messages = document.querySelectorAll('{}');
                if (messages.length > 0) {{
                    const lastMessage = messages[messages.length - 1];
                    return lastMessage.innerText.substring(0, 200);
                }}
                return '';
            }})()
        "#,
            config.message_container_selector
        );

        Ok(script)
    }
}

/// 通知事件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiCompletionEvent {
    pub url: String,
    pub title: String,
    pub site_name: String,
    pub message_preview: String,
    /// Chrome tab that produced the completion; used to route replies back to
    /// the exact conversation instead of whichever tab is currently active.
    #[serde(default)]
    pub tab_id: Option<u32>,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    /// 运行时间（秒）- AI Studio "Ran for Xs"
    #[serde(default)]
    pub run_time: Option<u32>,
    /// 思考时间（秒）- "Thought for X seconds"
    #[serde(default)]
    pub think_time: Option<u32>,
    /// 是否为图片生成完成
    #[serde(default)]
    pub image_generated: bool,
    /// 新生成的图片数量
    #[serde(default)]
    pub new_images: Option<u32>,
}

impl AiCompletionEvent {
    pub fn new(page_state: &PageState, message_preview: String) -> Self {
        Self {
            url: page_state.url.clone(),
            title: page_state.title.clone(),
            site_name: page_state.site_name.clone(),
            message_preview,
            tab_id: None,
            timestamp: chrono::Utc::now(),
            run_time: None,
            think_time: None,
            image_generated: false,
            new_images: None,
        }
    }
}
