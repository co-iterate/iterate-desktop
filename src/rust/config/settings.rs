use crate::constants::{audio, font, mcp, telegram, theme, window};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AppConfig {
    #[serde(default = "default_ui_config")]
    pub ui_config: UiConfig, // UI相关配置（主题、窗口、置顶等）
    #[serde(default = "default_audio_config")]
    pub audio_config: AudioConfig, // 音频相关配置
    #[serde(default = "default_reply_config")]
    pub reply_config: ReplyConfig, // 继续回复配置
    #[serde(default = "default_mobile_config")]
    pub mobile_config: MobileConfig, // 移动端远程能力配置
    #[serde(default = "default_cloudflare_config")]
    pub cloudflare_config: CloudflareConfig, // Cloudflare Web Login 配置（不含 secret）
    #[serde(default = "default_browser_ws_config")]
    pub browser_ws_config: BrowserWsConfig, // Browser WebSocket 扩展配对配置
    #[serde(default = "default_mcp_config")]
    pub mcp_config: McpConfig, // MCP工具配置
    #[serde(default = "default_telegram_config")]
    pub telegram_config: TelegramConfig, // Telegram Bot配置
    #[serde(default = "default_custom_prompt_config")]
    pub custom_prompt_config: CustomPromptConfig, // 自定义prompt配置
    #[serde(default = "default_shortcut_config")]
    pub shortcut_config: ShortcutConfig, // 自定义快捷键配置
    #[serde(default = "default_usage_config")]
    pub usage_config: UsageConfig, // AI 用量 Provider 配置
    #[serde(default = "default_checkpoint_config")]
    pub checkpoint_config: CheckpointConfig, // 自动检查点配置
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CheckpointConfig {
    // 是否启用自动检查点（每次 zhi 自动提交 + 后台文件监控）
    #[serde(default = "default_auto_checkpoint_enabled")]
    pub auto_checkpoint_enabled: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UiConfig {
    // 主题设置
    #[serde(default = "default_theme")]
    pub theme: String, // "light", "dark"

    // 字体设置
    #[serde(default = "default_font_config")]
    pub font_config: FontConfig,

    // 窗口设置
    #[serde(default = "default_window_config")]
    pub window_config: WindowConfig,

    // 置顶设置
    #[serde(default = "default_always_on_top")]
    pub always_on_top: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FontConfig {
    // 字体系列
    #[serde(default = "default_font_family")]
    pub font_family: String, // "inter", "jetbrains-mono", "system", "custom"

    // 字体大小
    #[serde(default = "default_font_size")]
    pub font_size: String, // "small", "medium", "large"

    // 自定义字体系列（当 font_family 为 "custom" 时使用）
    #[serde(default = "default_custom_font_family")]
    pub custom_font_family: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct WindowConfig {
    // 窗口约束设置
    #[serde(default = "default_auto_resize")]
    pub auto_resize: bool,
    #[serde(default = "default_max_width")]
    pub max_width: f64,
    #[serde(default = "default_max_height")]
    pub max_height: f64,
    #[serde(default = "default_min_width")]
    pub min_width: f64,
    #[serde(default = "default_min_height")]
    pub min_height: f64,

    // 当前模式
    #[serde(default = "default_window_fixed")]
    pub fixed: bool,

    // 固定模式的尺寸设置
    #[serde(default = "default_fixed_width")]
    pub fixed_width: f64,
    #[serde(default = "default_fixed_height")]
    pub fixed_height: f64,

    // 自由拉伸模式的尺寸设置
    #[serde(default = "default_free_width")]
    pub free_width: f64,
    #[serde(default = "default_free_height")]
    pub free_height: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AudioConfig {
    #[serde(default = "default_audio_notification_enabled")]
    pub notification_enabled: bool,
    #[serde(default = "default_audio_url")]
    pub custom_url: String, // 自定义音效URL
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ReplyConfig {
    #[serde(default = "default_enable_continue_reply")]
    pub enable_continue_reply: bool,
    #[serde(default = "default_auto_continue_threshold")]
    pub auto_continue_threshold: u32, // 字符数阈值
    #[serde(default = "default_continue_prompt")]
    pub continue_prompt: String, // 继续回复的提示词
    #[serde(default = "default_loop_prompt")]
    pub loop_prompt: String, // 循环模式的提示词
    #[serde(default = "default_goal_prompt_template")]
    pub goal_prompt_template: String, // Goal 提交时附加的执行规则
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MobileConfig {
    #[serde(default = "default_allow_mobile_ghost_suggestions_write")]
    pub allow_ghost_suggestions_write: bool,
    #[serde(default)]
    pub formal_route: Option<FormalMobileRouteConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FormalMobileRouteConfig {
    #[serde(default = "default_formal_mobile_route_schema_version")]
    pub schema_version: u32,
    pub transport: String,
    pub base_url: String,
    pub configured_at: String,
    pub source: String,
    #[serde(default = "default_formal_mobile_route_generation")]
    pub formal_route_generation: u64,
    #[serde(default)]
    pub last_verified_at: Option<String>,
    #[serde(default)]
    pub last_verification: Option<FormalMobileRouteVerification>,
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct FormalMobileRouteVerification {
    pub https_ok: bool,
    pub websocket_ok: bool,
    pub endpoint_identity_ok: bool,
    pub checked_at: String,
    pub error_code: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CloudflareConfig {
    #[serde(default)]
    pub guided_setup_enabled: bool,
    #[serde(default)]
    pub public_hostname: String,
    #[serde(default)]
    pub access_expected: bool,
    #[serde(default)]
    pub web_login_console_origin: String,
    #[serde(skip, default)]
    pub tunnel_token_saved: bool,
    #[serde(default)]
    pub last_verified_at: Option<String>,
    #[serde(default)]
    pub last_verification: Option<CloudflareVerificationResult>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CloudflareVerificationResult {
    pub state: String,
    pub public_hostname: String,
    pub health_ok: bool,
    pub pair_challenge_ok: bool,
    pub websocket_ok: bool,
    pub access_state: String,
    pub error_code: Option<String>,
    pub checked_at: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct BrowserWsConfig {
    #[serde(default)]
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct McpConfig {
    #[serde(default = "default_mcp_tools")]
    pub tools: HashMap<String, bool>, // MCP工具启用状态
    pub acemcp_base_url: Option<String>, // acemcp API端点URL
    pub acemcp_token: Option<String>,    // acemcp认证令牌
    pub acemcp_batch_size: Option<u32>,  // acemcp批处理大小
    pub acemcp_max_lines_per_blob: Option<u32>, // acemcp最大行数/块
    pub acemcp_text_extensions: Option<Vec<String>>, // acemcp文件扩展名
    pub acemcp_exclude_patterns: Option<Vec<String>>, // acemcp排除模式
}

// 自定义prompt结构
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CustomPrompt {
    pub id: String,
    pub name: String,
    pub content: String,
    pub description: Option<String>,
    pub sort_order: i32,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default = "default_prompt_type")]
    pub r#type: String, // "normal" | "conditional"
    // 条件性prompt专用字段
    pub condition_text: Option<String>, // 条件描述文本
    pub template_true: Option<String>,  // 开关为true时的模板
    pub template_false: Option<String>, // 开关为false时的模板
    #[serde(default = "default_prompt_state")]
    pub current_state: bool, // 当前开关状态（原default_state）
    #[serde(default = "default_prompt_active")]
    pub is_active: bool, // 是否启用该项（即是否参与追加）
}

// 自定义prompt配置
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CustomPromptConfig {
    #[serde(default = "default_custom_prompts")]
    pub prompts: Vec<CustomPrompt>,
    #[serde(default = "default_custom_prompt_enabled")]
    pub enabled: bool,
    #[serde(default = "default_custom_prompt_max_prompts")]
    pub max_prompts: u32,
}

// 快捷键配置
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShortcutConfig {
    #[serde(default = "default_shortcuts")]
    pub shortcuts: HashMap<String, ShortcutBinding>,
    #[serde(default = "default_global_shortcut_enabled")]
    pub global_enabled: bool,
}

pub fn default_global_shortcut_enabled() -> bool {
    true
}

// 快捷键绑定
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShortcutBinding {
    pub id: String,
    pub name: String,
    pub description: String,
    pub action: String, // "submit", "exit", "custom"
    pub key_combination: ShortcutKey,
    pub enabled: bool,
    pub scope: String, // "global", "popup", "input"
}

// 快捷键组合
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct ShortcutKey {
    pub key: String, // 主键，如 "Enter", "Q", "F4"
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool, // macOS的Cmd键
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TelegramConfig {
    #[serde(default = "default_telegram_enabled")]
    pub enabled: bool, // 是否启用Telegram Bot
    #[serde(default = "default_telegram_bot_token")]
    pub bot_token: String, // Bot Token
    #[serde(default = "default_telegram_chat_id")]
    pub chat_id: String, // Chat ID
    #[serde(default = "default_telegram_hide_frontend_popup")]
    pub hide_frontend_popup: bool, // 是否隐藏前端弹窗，仅使用Telegram交互
    #[serde(default = "default_telegram_api_base_url")]
    pub api_base_url: String, // Telegram API基础URL
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UsageConfig {
    #[serde(default = "default_usage_enabled")]
    pub enabled: bool,
    #[serde(default = "default_usage_refresh_interval_seconds")]
    pub refresh_interval_seconds: u64,
    #[serde(default = "default_usage_providers")]
    pub providers: HashMap<String, UsageProviderConfig>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UsageProviderConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_usage_provider_source")]
    pub source: String,
    #[serde(default)]
    pub manual_cookie: Option<String>,
    #[serde(default)]
    pub accounts: Vec<UsageProviderAccountConfig>,
    #[serde(default)]
    pub auto_discover_accounts: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UsageProviderAccountConfig {
    pub id: String,
    #[serde(default = "default_usage_provider_account_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub codex_home: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug)]
pub struct AppState {
    pub config: Mutex<AppConfig>,
    pub response_channels: Mutex<HashMap<String, tokio::sync::oneshot::Sender<String>>>,
    pub request_ready_channels: Mutex<HashMap<String, tokio::sync::oneshot::Sender<()>>>,
    // 快捷键全局启用状态原子变量，用于高性能检查
    pub global_shortcut_enabled: Arc<AtomicBool>,
    #[cfg(target_os = "windows")]
    // 防止 Windows 窗口关闭事件重复进入退出流程
    pub exit_in_progress: AtomicBool,
    // 防误触退出机制
    pub exit_attempt_count: Mutex<u32>,
    pub last_exit_attempt: Mutex<Option<std::time::Instant>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            config: Mutex::new(AppConfig::default()),
            response_channels: Mutex::new(HashMap::new()),
            request_ready_channels: Mutex::new(HashMap::new()),
            global_shortcut_enabled: Arc::new(AtomicBool::new(true)),
            #[cfg(target_os = "windows")]
            exit_in_progress: AtomicBool::new(false),
            exit_attempt_count: Mutex::new(0),
            last_exit_attempt: Mutex::new(None),
        }
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ui_config: default_ui_config(),
            audio_config: default_audio_config(),
            reply_config: default_reply_config(),
            mobile_config: default_mobile_config(),
            cloudflare_config: default_cloudflare_config(),
            browser_ws_config: default_browser_ws_config(),
            mcp_config: default_mcp_config(),
            telegram_config: default_telegram_config(),
            custom_prompt_config: default_custom_prompt_config(),
            shortcut_config: default_shortcut_config(),
            usage_config: default_usage_config(),
            checkpoint_config: default_checkpoint_config(),
        }
    }
}

pub fn default_checkpoint_config() -> CheckpointConfig {
    CheckpointConfig {
        auto_checkpoint_enabled: default_auto_checkpoint_enabled(),
    }
}

pub fn default_auto_checkpoint_enabled() -> bool {
    true
}

// 默认值函数
pub fn default_ui_config() -> UiConfig {
    UiConfig {
        theme: default_theme(),
        font_config: default_font_config(),
        window_config: default_window_config(),
        always_on_top: default_always_on_top(),
    }
}

pub fn default_audio_config() -> AudioConfig {
    AudioConfig {
        notification_enabled: default_audio_notification_enabled(),
        custom_url: default_audio_url(),
    }
}

pub fn default_mcp_config() -> McpConfig {
    McpConfig {
        tools: default_mcp_tools(),
        acemcp_base_url: None,
        acemcp_token: None,
        acemcp_batch_size: None,
        acemcp_max_lines_per_blob: None,
        acemcp_text_extensions: None,
        acemcp_exclude_patterns: None,
    }
}

pub fn default_mobile_config() -> MobileConfig {
    MobileConfig {
        allow_ghost_suggestions_write: default_allow_mobile_ghost_suggestions_write(),
        formal_route: None,
    }
}

pub fn default_formal_mobile_route_schema_version() -> u32 {
    1
}

pub fn default_formal_mobile_route_generation() -> u64 {
    1
}

pub fn default_cloudflare_config() -> CloudflareConfig {
    CloudflareConfig {
        guided_setup_enabled: false,
        public_hostname: String::new(),
        access_expected: false,
        web_login_console_origin: String::new(),
        tunnel_token_saved: false,
        last_verified_at: None,
        last_verification: None,
    }
}

pub fn default_browser_ws_config() -> BrowserWsConfig {
    BrowserWsConfig {
        token: String::new(),
    }
}

pub fn default_allow_mobile_ghost_suggestions_write() -> bool {
    false
}

pub fn default_telegram_config() -> TelegramConfig {
    TelegramConfig {
        enabled: default_telegram_enabled(),
        bot_token: default_telegram_bot_token(),
        chat_id: default_telegram_chat_id(),
        hide_frontend_popup: default_telegram_hide_frontend_popup(),
        api_base_url: default_telegram_api_base_url(),
    }
}

pub fn default_custom_prompt_config() -> CustomPromptConfig {
    CustomPromptConfig {
        prompts: default_custom_prompts(),
        enabled: default_custom_prompt_enabled(),
        max_prompts: default_custom_prompt_max_prompts(),
    }
}

pub fn default_usage_config() -> UsageConfig {
    UsageConfig {
        enabled: default_usage_enabled(),
        refresh_interval_seconds: default_usage_refresh_interval_seconds(),
        providers: default_usage_providers(),
    }
}

pub fn default_usage_enabled() -> bool {
    true
}

pub fn default_usage_refresh_interval_seconds() -> u64 {
    300
}

pub fn default_usage_provider_source() -> String {
    "auto".to_string()
}

pub fn default_usage_provider_account_enabled() -> bool {
    true
}

pub fn default_usage_providers() -> HashMap<String, UsageProviderConfig> {
    let mut providers = HashMap::new();
    providers.insert(
        "codex".to_string(),
        UsageProviderConfig {
            enabled: true,
            source: "auto".to_string(),
            manual_cookie: None,
            accounts: Vec::new(),
            auto_discover_accounts: false,
        },
    );
    providers.insert(
        "antigravity".to_string(),
        UsageProviderConfig {
            enabled: true,
            source: "local".to_string(),
            manual_cookie: None,
            accounts: Vec::new(),
            auto_discover_accounts: false,
        },
    );
    providers
}

pub fn default_always_on_top() -> bool {
    window::DEFAULT_ALWAYS_ON_TOP
}

pub fn default_audio_notification_enabled() -> bool {
    audio::DEFAULT_NOTIFICATION_ENABLED
}

pub fn default_theme() -> String {
    theme::DEFAULT.to_string()
}

pub fn default_audio_url() -> String {
    audio::DEFAULT_URL.to_string()
}

pub fn default_window_config() -> WindowConfig {
    WindowConfig {
        auto_resize: window::DEFAULT_AUTO_RESIZE,
        max_width: window::MAX_WIDTH,
        max_height: window::MAX_HEIGHT,
        min_width: window::MIN_WIDTH,
        min_height: window::MIN_HEIGHT,
        fixed: window::DEFAULT_FIXED_MODE,
        fixed_width: window::DEFAULT_WIDTH,
        fixed_height: window::DEFAULT_HEIGHT,
        free_width: window::DEFAULT_WIDTH,
        free_height: window::DEFAULT_HEIGHT,
    }
}

pub fn default_reply_config() -> ReplyConfig {
    ReplyConfig {
        enable_continue_reply: mcp::DEFAULT_CONTINUE_REPLY_ENABLED,
        auto_continue_threshold: mcp::DEFAULT_AUTO_CONTINUE_THRESHOLD,
        continue_prompt: mcp::DEFAULT_CONTINUE_PROMPT.to_string(),
        loop_prompt: mcp::DEFAULT_LOOP_PROMPT.to_string(),
        goal_prompt_template: mcp::DEFAULT_GOAL_PROMPT_TEMPLATE.to_string(),
    }
}

pub fn default_auto_resize() -> bool {
    true
}

pub fn default_max_width() -> f64 {
    window::MAX_WIDTH
}

pub fn default_max_height() -> f64 {
    window::MAX_HEIGHT
}

pub fn default_min_width() -> f64 {
    window::MIN_WIDTH
}

pub fn default_min_height() -> f64 {
    window::MIN_HEIGHT
}

pub fn default_enable_continue_reply() -> bool {
    mcp::DEFAULT_CONTINUE_REPLY_ENABLED
}

pub fn default_auto_continue_threshold() -> u32 {
    mcp::DEFAULT_AUTO_CONTINUE_THRESHOLD
}

pub fn default_continue_prompt() -> String {
    mcp::DEFAULT_CONTINUE_PROMPT.to_string()
}

pub fn default_loop_prompt() -> String {
    mcp::DEFAULT_LOOP_PROMPT.to_string()
}

pub fn default_goal_prompt_template() -> String {
    mcp::DEFAULT_GOAL_PROMPT_TEMPLATE.to_string()
}

pub fn default_mcp_tools() -> HashMap<String, bool> {
    let mut tools = HashMap::new();
    tools.insert(mcp::TOOL_ZHI.to_string(), true); // iterate 工具默认启用
    tools.insert(mcp::TOOL_JI.to_string(), false); // 记忆管理工具默认关闭
    tools.insert(mcp::TOOL_SOU.to_string(), false); // 代码搜索工具默认关闭
    tools.insert(mcp::TOOL_PAI.to_string(), false); // room 编排工具默认关闭
    tools.insert(mcp::TOOL_XI.to_string(), false); // 经验查找工具默认关闭
    tools.insert(mcp::TOOL_CI.to_string(), false); // 提示词库搜索工具默认关闭
    tools.insert(mcp::TOOL_EXEC_PTY.to_string(), false); // PTY终端执行工具可手动启用
    tools.insert(mcp::TOOL_TASK.to_string(), true); // 任务系统默认开启
    tools.insert(mcp::TOOL_PHONE_ACTION.to_string(), true); // iPhone 合法动作路由默认开启
    tools.insert(mcp::TOOL_CRON_MANAGE.to_string(), false); // crontab 持久命令工具默认关闭
    tools
}

pub fn default_window_width() -> f64 {
    window::DEFAULT_WIDTH
}

pub fn default_window_height() -> f64 {
    window::DEFAULT_HEIGHT
}

pub fn default_window_fixed() -> bool {
    window::DEFAULT_FIXED_MODE
}

pub fn default_fixed_width() -> f64 {
    window::DEFAULT_WIDTH
}

pub fn default_fixed_height() -> f64 {
    window::DEFAULT_HEIGHT
}

pub fn default_free_width() -> f64 {
    window::DEFAULT_WIDTH
}

pub fn default_free_height() -> f64 {
    window::DEFAULT_HEIGHT
}

pub fn default_telegram_enabled() -> bool {
    telegram::DEFAULT_ENABLED
}

pub fn default_telegram_bot_token() -> String {
    telegram::DEFAULT_BOT_TOKEN.to_string()
}

pub fn default_telegram_chat_id() -> String {
    telegram::DEFAULT_CHAT_ID.to_string()
}

pub fn default_telegram_hide_frontend_popup() -> bool {
    telegram::DEFAULT_HIDE_FRONTEND_POPUP
}

pub fn default_telegram_api_base_url() -> String {
    telegram::API_BASE_URL.to_string()
}

impl WindowConfig {
    // 获取当前模式的宽度
    pub fn current_width(&self) -> f64 {
        if self.fixed {
            self.fixed_width
        } else {
            self.free_width
        }
    }

    // 获取当前模式的高度
    pub fn current_height(&self) -> f64 {
        if self.fixed {
            self.fixed_height
        } else {
            self.free_height
        }
    }

    // 更新当前模式的尺寸
    pub fn update_current_size(&mut self, width: f64, height: f64) {
        if self.fixed {
            self.fixed_width = width;
            self.fixed_height = height;
        } else {
            self.free_width = width;
            self.free_height = height;
        }
    }
}

// 字体配置默认值函数
pub fn default_font_config() -> FontConfig {
    FontConfig {
        font_family: default_font_family(),
        font_size: default_font_size(),
        custom_font_family: default_custom_font_family(),
    }
}

pub fn default_font_family() -> String {
    font::DEFAULT_FONT_FAMILY.to_string()
}

pub fn default_font_size() -> String {
    font::DEFAULT_FONT_SIZE.to_string()
}

pub fn default_custom_font_family() -> String {
    font::DEFAULT_CUSTOM_FONT_FAMILY.to_string()
}

pub fn default_prompt_type() -> String {
    "normal".to_string()
}

pub fn default_prompt_state() -> bool {
    false
}

pub fn default_prompt_active() -> bool {
    true
}

// 自定义prompt默认值函数
pub fn default_custom_prompts() -> Vec<CustomPrompt> {
    vec![
        CustomPrompt {
            id: "default_1".to_string(),
            name: "✅Done".to_string(),
            content: "结束当前对话".to_string(),
            description: Some("请求AI结束工作".to_string()),
            sort_order: 1,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "normal".to_string(),
            condition_text: None,
            template_true: None,
            template_false: None,
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_2".to_string(),
            name: "🧹Clear".to_string(),
            content: "".to_string(),
            description: Some("清空输入框内容".to_string()),
            sort_order: 2,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "normal".to_string(),
            condition_text: None,
            template_true: None,
            template_false: None,
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_3".to_string(),
            name: "✨New Issue".to_string(),
            content: "ok，完美，新的需求or问题，".to_string(),
            description: Some("准备新的需求or问题".to_string()),
            sort_order: 3,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "normal".to_string(),
            condition_text: None,
            template_true: None,
            template_false: None,
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_4".to_string(),
            name: "🧠Remember".to_string(),
            content: "请记住，".to_string(),
            description: Some("iterate 的另一个工具，请记住".to_string()),
            sort_order: 4,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "normal".to_string(),
            condition_text: None,
            template_true: None,
            template_false: None,
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_5".to_string(),
            name: "📝Summary And Restart".to_string(),
            content: "本次对话的上下文已经太长了，我打算关掉并重新开一个新的会话。你有什么想对你的继任者说的，以便它能更好的理解你当前的工作并顺利继续？".to_string(),
            description: Some("总结-开新会话".to_string()),
            sort_order: 5,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "normal".to_string(),
            condition_text: None,
            template_true: None,
            template_false: None,
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_6".to_string(),
            name: "🔍Review And Plan".to_string(),
            content: "请执行以下项目进度检查和规划任务：\n\n1. **项目进度分析**：\n   - 查看当前代码库状态，分析已完成的功能模块\n   - 识别已完成、进行中和待开始的功能点\n\n2. **里程碑确定**：\n   - 基于当前进度和剩余工作量，定义清晰的里程碑节点\n   - 为每个里程碑设定具体的完成标准和时间预期\n   - 优先考虑核心任务管理功能的里程碑\n\n3. **文档更新**（注意：仅更新现有文档，不创建新文档）：\n   - 更新项目规划文档中的进度状态\n   - 修正任何与实际实现不符的技术方案描述\n   - 确保文档反映当前的技术栈和架构决策\n\n4. **下一步工作规划**：\n   - 基于用户偏好（系统化开发方法、前端优先、分步骤反馈）制定具体的下一阶段工作计划\n   - 识别关键路径上的阻塞点和依赖关系\n   - 提供3-5个具体的下一步行动项，按优先级排序\n\n5. **反馈收集**：\n   - 在完成分析后，使用 iterate 工具收集用户对进度评估和下一步计划的反馈\n   - 提供多个可选的发展方向供用户选择".to_string(),
            description: Some("项目进度检查和规划任务".to_string()),
            sort_order: 6,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "normal".to_string(),
            condition_text: None,
            template_true: None,
            template_false: None,
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_7".to_string(),
            name: "是否生成总结性Markdown文档".to_string(),
            content: "".to_string(),
            description: Some("是否生成总结性Markdown文档".to_string()),
            sort_order: 7,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "conditional".to_string(),
            condition_text: Some("是否生成总结性Markdown文档".to_string()),
            template_true: Some("✔️请记住，帮我生成总结性Markdown文档".to_string()),
            template_false: Some("❌请记住，不要生成总结性Markdown文档".to_string()),
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_8".to_string(),
            name: "是否生成测试脚本".to_string(),
            content: "".to_string(),
            description: Some("是否生成测试脚本".to_string()),
            sort_order: 8,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "conditional".to_string(),
            condition_text: Some("是否生成测试脚本".to_string()),
            template_true: Some("✔️请记住，帮我生成测试脚本".to_string()),
            template_false: Some("❌请记住，不要生成测试脚本".to_string()),
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_9".to_string(),
            name: "是否主动编译".to_string(),
            content: "".to_string(),
            description: Some("是否主动编译".to_string()),
            sort_order: 9,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "conditional".to_string(),
            condition_text: Some("是否主动编译".to_string()),
            template_true: Some("✔️请记住，帮我编译".to_string()),
            template_false: Some("❌请记住，不要编译，用户自己编译".to_string()),
            current_state: false,
            is_active: true,
        },
        CustomPrompt {
            id: "default_10".to_string(),
            name: "是否主动运行".to_string(),
            content: "".to_string(),
            description: Some("是否主动运行".to_string()),
            sort_order: 10,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
            r#type: "conditional".to_string(),
            condition_text: Some("是否主动运行".to_string()),
            template_true: Some("✔️请记住，帮我运行".to_string()),
            template_false: Some("❌请记住，不要运行，用户自己运行".to_string()),
            current_state: false,
            is_active: true,
        },
    ]
}

pub fn default_custom_prompt_enabled() -> bool {
    true
}

pub fn default_custom_prompt_max_prompts() -> u32 {
    50
}

// 快捷键默认值函数
pub fn default_shortcut_config() -> ShortcutConfig {
    ShortcutConfig {
        shortcuts: default_shortcuts(),
        global_enabled: true,
    }
}

pub fn default_shortcuts() -> HashMap<String, ShortcutBinding> {
    let mut shortcuts = HashMap::new();
    let is_macos = cfg!(target_os = "macos");

    // 快速发送快捷键
    shortcuts.insert(
        "quick_submit".to_string(),
        ShortcutBinding {
            id: "quick_submit".to_string(),
            name: "快速发送".to_string(),
            description: "快速提交当前输入内容".to_string(),
            action: "submit".to_string(),
            key_combination: ShortcutKey {
                key: "Enter".to_string(),
                ctrl: false,
                alt: false,
                shift: !is_macos,
                meta: is_macos,
            },
            enabled: true,
            scope: "popup".to_string(),
        },
    );

    // 增强快捷键
    shortcuts.insert(
        "enhance".to_string(),
        ShortcutBinding {
            id: "enhance".to_string(),
            name: "增强".to_string(),
            description: "增强当前输入内容".to_string(),
            action: "enhance".to_string(),
            key_combination: ShortcutKey {
                key: "Enter".to_string(),
                ctrl: !is_macos,
                alt: is_macos,
                shift: !is_macos,
                meta: false,
            },
            enabled: true,
            scope: "popup".to_string(),
        },
    );

    // 继续快捷键
    shortcuts.insert(
        "continue".to_string(),
        ShortcutBinding {
            id: "continue".to_string(),
            name: "继续".to_string(),
            description: "继续对话".to_string(),
            action: "continue".to_string(),
            key_combination: ShortcutKey {
                key: "Enter".to_string(),
                ctrl: !is_macos,
                alt: false,
                shift: is_macos,
                meta: false,
            },
            enabled: true,
            scope: "popup".to_string(),
        },
    );

    // 引用选区快捷键
    shortcuts.insert(
        "quote_selection".to_string(),
        ShortcutBinding {
            id: "quote_selection".to_string(),
            name: "引用选区".to_string(),
            description: "将当前选区作为引用插入输入框".to_string(),
            action: "quote_selection_to_input".to_string(),
            key_combination: ShortcutKey {
                key: "Y".to_string(),
                ctrl: false,
                alt: false,
                shift: true,
                meta: true,
            },
            enabled: true,
            scope: "popup".to_string(),
        },
    );

    shortcuts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_provider_config_defaults_to_no_accounts() {
        let config: UsageProviderConfig =
            serde_json::from_str(r#"{"enabled":true}"#).expect("usage provider config");

        assert!(config.accounts.is_empty());
        assert!(!config.auto_discover_accounts);
        assert_eq!(config.source, "auto");
    }

    #[test]
    fn usage_account_defaults_to_enabled() {
        let account: UsageProviderAccountConfig =
            serde_json::from_str(r#"{"id":"plus"}"#).expect("usage account config");

        assert!(account.enabled);
        assert_eq!(account.id, "plus");
    }

    #[test]
    fn popup_shortcut_defaults_match_current_platform() {
        let shortcuts = default_shortcuts();
        let quick_submit = &shortcuts["quick_submit"].key_combination;
        let continue_key = &shortcuts["continue"].key_combination;
        let enhance = &shortcuts["enhance"].key_combination;

        if cfg!(target_os = "macos") {
            assert!(quick_submit.meta);
            assert!(!quick_submit.ctrl && !quick_submit.alt && !quick_submit.shift);
            assert!(continue_key.shift);
            assert!(!continue_key.ctrl && !continue_key.alt && !continue_key.meta);
            assert!(enhance.alt);
            assert!(!enhance.ctrl && !enhance.shift && !enhance.meta);
        } else {
            assert!(quick_submit.shift);
            assert!(!quick_submit.ctrl && !quick_submit.alt && !quick_submit.meta);
            assert!(continue_key.ctrl);
            assert!(!continue_key.alt && !continue_key.shift && !continue_key.meta);
            assert!(enhance.ctrl && enhance.shift);
            assert!(!enhance.alt && !enhance.meta);
        }
    }
}
