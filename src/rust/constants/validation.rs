// 验证相关常量和函数

use super::audio;
use super::network;
use super::telegram;
use super::theme;
use super::window;

// 通用验证函数

/// 验证字符串是否为空
pub fn is_empty_string(s: &str) -> bool {
    s.trim().is_empty()
}

/// 验证字符串长度是否在范围内
pub fn is_valid_string_length(s: &str, min: usize, max: usize) -> bool {
    let len = s.len();
    len >= min && len <= max
}

/// 验证数值是否在范围内
pub fn is_in_range<T: PartialOrd>(value: T, min: T, max: T) -> bool {
    value >= min && value <= max
}

/// 验证 URL 格式是否有效
pub fn is_valid_url(url: &str) -> bool {
    if url.is_empty() {
        return true; // 空 URL 被认为是有效的（使用默认值）
    }

    url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")
}

/// 验证文件路径是否有效
pub fn is_valid_file_path(path: &str) -> bool {
    !path.is_empty() && !path.contains('\0')
}

/// 验证端口号是否有效
pub fn is_valid_port(port: u16) -> bool {
    port > 0
}

// 窗口验证函数
pub use window::is_valid_window_size;

/// 验证窗口位置是否有效
pub fn is_valid_window_position(x: i32, y: i32) -> bool {
    // 允许负值，因为多显示器环境下可能有负坐标
    (-10000..=10000).contains(&x) && (-10000..=10000).contains(&y)
}

// 音频验证函数
pub use audio::{is_supported_audio_format, is_valid_audio_file_size};

/// 验证音量是否有效
pub fn is_valid_volume(volume: f32) -> bool {
    is_in_range(volume, 0.0, 1.0)
}

// 主题验证函数
pub use theme::is_valid_theme;

// Telegram 验证函数
pub use telegram::{is_valid_bot_token, is_valid_chat_id};

// 网络验证函数

/// 验证超时时间是否有效
pub fn is_valid_timeout(timeout_ms: u64) -> bool {
    is_in_range(timeout_ms, 100, 300000) // 100ms 到 5分钟
}

/// 验证重试次数是否有效
pub fn is_valid_retry_count(count: u32) -> bool {
    count <= network::MAX_RETRY_COUNT
}

// 配置验证结构体
#[derive(Debug, Clone)]
pub struct ValidationResult {
    pub is_valid: bool,
    pub errors: Vec<String>,
}

impl ValidationResult {
    pub fn new() -> Self {
        Self {
            is_valid: true,
            errors: Vec::new(),
        }
    }

    pub fn add_error(&mut self, error: String) {
        self.is_valid = false;
        self.errors.push(error);
    }

    pub fn merge(&mut self, other: ValidationResult) {
        if !other.is_valid {
            self.is_valid = false;
            self.errors.extend(other.errors);
        }
    }
}

impl Default for ValidationResult {
    fn default() -> Self {
        Self::new()
    }
}

// 综合验证函数

/// 验证窗口配置
pub fn validate_window_config(
    width: f64,
    height: f64,
    x: Option<i32>,
    y: Option<i32>,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    if !is_valid_window_size(width, height) {
        result.add_error(format!("无效的窗口尺寸: {}x{}", width, height));
    }

    if let (Some(x), Some(y)) = (x, y) {
        if !is_valid_window_position(x, y) {
            result.add_error(format!("无效的窗口位置: ({}, {})", x, y));
        }
    }

    result
}

/// 验证音频配置
pub fn validate_audio_config(url: &str, volume: f32, enabled: bool) -> ValidationResult {
    let mut result = ValidationResult::new();

    if enabled && !url.is_empty() && !is_valid_url(url) {
        result.add_error(format!("无效的音频 URL: {}", url));
    }

    if !is_valid_volume(volume) {
        result.add_error(format!("无效的音量值: {}", volume));
    }

    result
}

/// 验证网络配置
pub fn validate_network_config(
    timeout_ms: u64,
    retry_count: u32,
    retry_interval_ms: u64,
) -> ValidationResult {
    let mut result = ValidationResult::new();

    if !is_valid_timeout(timeout_ms) {
        result.add_error(format!("无效的超时时间: {}ms", timeout_ms));
    }

    if !is_valid_retry_count(retry_count) {
        result.add_error(format!("无效的重试次数: {}", retry_count));
    }

    if !is_in_range(
        retry_interval_ms,
        network::MIN_RETRY_INTERVAL_MS,
        network::MAX_RETRY_INTERVAL_MS,
    ) {
        result.add_error(format!("无效的重试间隔: {}ms", retry_interval_ms));
    }

    result
}
