// 主题相关常量

/// 默认主题（新用户首次打开为浅色）
pub const DEFAULT: &str = "light";

/// 浅色主题
pub const LIGHT: &str = "light";

/// 深色主题
pub const DARK: &str = "dark";

/// 可用主题列表
pub const AVAILABLE_THEMES: &[&str] = &[LIGHT, DARK];

// 主题配置结构体
#[derive(Debug, Clone)]
pub struct ThemeConfig {
    pub default_theme: String,
    pub available_themes: Vec<String>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            default_theme: DEFAULT.to_string(),
            available_themes: AVAILABLE_THEMES.iter().map(|s| s.to_string()).collect(),
        }
    }
}

impl ThemeConfig {
    /// 验证主题是否有效
    pub fn is_valid_theme(&self, theme: &str) -> bool {
        self.available_themes.contains(&theme.to_string())
    }

    /// 获取默认主题
    pub fn get_default(&self) -> &str {
        &self.default_theme
    }

    /// 转换为 JSON 格式
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "default": self.default_theme,
            "available": self.available_themes
        })
    }
}

// 便捷函数
/// 获取默认主题配置
pub fn get_default_theme_config() -> ThemeConfig {
    ThemeConfig::default()
}

/// 验证主题是否有效
pub fn is_valid_theme(theme: &str) -> bool {
    AVAILABLE_THEMES.contains(&theme)
}

/// 获取默认主题
pub fn get_default_theme() -> &'static str {
    DEFAULT
}
