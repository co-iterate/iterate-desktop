// 音频相关常量

/// 默认音频 URL（空字符串表示使用内置音效）
pub const DEFAULT_URL: &str = "";

/// 默认音频通知启用状态
pub const DEFAULT_NOTIFICATION_ENABLED: bool = false;

/// 音频文件支持的格式
pub const SUPPORTED_FORMATS: &[&str] = &["mp3", "wav", "ogg", "m4a"];

/// 默认音量 (0.0 - 1.0)
pub const DEFAULT_VOLUME: f32 = 0.8;

/// 最大音频文件大小 (bytes) - 10MB
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

// 音频配置结构体
#[derive(Debug, Clone)]
pub struct AudioConfig {
    pub default_url: String,
    pub notification_enabled: bool,
    pub supported_formats: Vec<String>,
    pub default_volume: f32,
    pub max_file_size: u64,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self {
            default_url: DEFAULT_URL.to_string(),
            notification_enabled: DEFAULT_NOTIFICATION_ENABLED,
            supported_formats: SUPPORTED_FORMATS.iter().map(|s| s.to_string()).collect(),
            default_volume: DEFAULT_VOLUME,
            max_file_size: MAX_FILE_SIZE,
        }
    }
}

impl AudioConfig {
    /// 验证音频格式是否支持
    pub fn is_supported_format(&self, format: &str) -> bool {
        self.supported_formats.contains(&format.to_lowercase())
    }

    /// 验证音频文件大小是否有效
    pub fn is_valid_file_size(&self, size: u64) -> bool {
        size <= self.max_file_size
    }

    /// 验证音量是否有效
    pub fn is_valid_volume(&self, volume: f32) -> bool {
        (0.0..=1.0).contains(&volume)
    }

    /// 转换为 JSON 格式
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "default_url": self.default_url,
            "notification_enabled": self.notification_enabled,
            "supported_formats": self.supported_formats,
            "default_volume": self.default_volume,
            "max_file_size": self.max_file_size
        })
    }
}

// 便捷函数
/// 获取默认音频配置
pub fn get_default_audio_config() -> AudioConfig {
    AudioConfig::default()
}

/// 验证音频格式是否支持
pub fn is_supported_audio_format(format: &str) -> bool {
    SUPPORTED_FORMATS.contains(&format.to_lowercase().as_str())
}

/// 验证音频文件大小是否有效
pub fn is_valid_audio_file_size(size: u64) -> bool {
    size <= MAX_FILE_SIZE
}
