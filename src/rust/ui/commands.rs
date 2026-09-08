use crate::bridge::ws::{
    broadcast_custom_prompt_config_changed, broadcast_ghost_suggestions_changed,
};
use crate::config::{
    load_config, save_config, AppConfig, AppState, CustomPrompt, CustomPromptConfig, ReplyConfig,
    ShortcutBinding, ShortcutConfig, WindowConfig,
};
use crate::constants::{ui, validation, window};
use crate::conversation::{resolve_tree_route_key, ConversationManager, NodeMetadata, NodeType};
use crate::mcp::codex_deeplink::codex_thread_deeplink;
use crate::mcp::handlers::create_tauri_popup;
use crate::mcp::tools::checkpoint::links::{append_checkpoint_link, build_checkpoint_link_entry};
use crate::mcp::types::PopupRequest;
use crate::mcp::types::{build_continue_response, build_send_response, ImageAttachment};
use crate::speech_memory;
use crate::utils::append_timeline_debug_log;
use percent_encoding::{percent_decode_str, utf8_percent_encode, NON_ALPHANUMERIC};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, Position, State};

#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

trait BackgroundCommandExt {
    fn without_console_window(&mut self) -> &mut Self;
}

impl BackgroundCommandExt for std::process::Command {
    fn without_console_window(&mut self) -> &mut Self {
        #[cfg(target_os = "windows")]
        {
            use std::os::windows::process::CommandExt;
            self.creation_flags(CREATE_NO_WINDOW);
        }
        self
    }
}

#[cfg(target_os = "macos")]
static STANDALONE_PREVIOUS_FRONTMOST_APPLICATION: OnceLock<
    std::sync::Mutex<Option<crate::native_speech::target::FrontmostApplication>>,
> = OnceLock::new();

#[cfg(target_os = "macos")]
pub(crate) fn remember_standalone_previous_frontmost_application() {
    let Ok(application) = crate::native_speech::target::capture_frontmost_application() else {
        return;
    };
    if application.pid == std::process::id() as i32 {
        return;
    }

    if let Ok(mut guard) = STANDALONE_PREVIOUS_FRONTMOST_APPLICATION
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
    {
        *guard = Some(application);
    }
}

#[cfg(target_os = "macos")]
fn restore_standalone_previous_frontmost_application() -> Result<bool, String> {
    let application = STANDALONE_PREVIOUS_FRONTMOST_APPLICATION
        .get_or_init(|| std::sync::Mutex::new(None))
        .lock()
        .map_err(|_| "failed to lock standalone previous application".to_string())?
        .clone();
    let Some(application) = application else {
        return Ok(false);
    };

    crate::native_speech::target::activate_application(&application)?;
    Ok(true)
}

#[tauri::command]
pub fn dismiss_standalone_mcp_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "standalone MCP window not found".to_string())?;
    window
        .hide()
        .map_err(|error| format!("failed to hide standalone MCP window: {error}"))?;

    #[cfg(target_os = "macos")]
    if let Err(error) = restore_standalone_previous_frontmost_application() {
        log::warn!("failed to restore application after MCP dismissal: {error}");
    }

    Ok(())
}

fn bridge_base_url() -> String {
    std::env::var("ITERATE_BRIDGE_BASE_URL")
        .ok()
        .map(|value| value.trim().trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexAutomationProbeResult {
    pub status: String,
    pub details: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenCodexChatResult {
    pub ok: bool,
    pub sent: bool,
    pub mode: String,
    pub message: String,
}

#[tauri::command]
pub async fn get_app_info() -> Result<String, String> {
    Ok(format!("iterate v{}", env!("CARGO_PKG_VERSION")))
}

#[tauri::command]
pub async fn requires_activation_gate() -> bool {
    activation_gate_required_for_current_build(is_mcp_interaction_process())
}

pub(crate) fn activation_gate_required_for_current_build(is_mcp_shell: bool) -> bool {
    activation_gate_required_for_build(
        cfg!(not(any(target_os = "android", target_os = "ios"))),
        option_env!("ITERATE_REQUIRE_ACTIVATION"),
        is_mcp_shell,
    )
}

fn is_mcp_interaction_process() -> bool {
    std::env::var_os("ITERATE_STANDALONE_MODE").is_some()
        || std::env::var_os("ITERATE_MCP_REQUEST_FILE").is_some()
        || std::env::args().any(|arg| arg == "--mcp-request" || arg == "--ui")
}

fn activation_gate_required_for_build(
    is_desktop: bool,
    build_flag: Option<&str>,
    is_mcp_shell: bool,
) -> bool {
    is_desktop
        && !is_mcp_shell
        && build_flag.is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
}

#[tauri::command]
pub async fn get_always_on_top(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.ui_config.always_on_top)
}

#[tauri::command]
pub async fn set_always_on_top(
    enabled: bool,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.ui_config.always_on_top = enabled;
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 应用到当前窗口
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            window
                .set_always_on_top(enabled)
                .map_err(|e| format!("设置窗口置顶失败: {}", e))?;

            log::info!("用户切换窗口置顶状态为: {} (已保存配置)", enabled);
        }

        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = window;
            log::info!("移动端不支持窗口置顶，配置已保存: {}", enabled);
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn get_auto_checkpoint_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.checkpoint_config.auto_checkpoint_enabled)
}

fn apply_auto_checkpoint_enabled(config: &mut AppConfig, enabled: bool) -> bool {
    let previous = config.checkpoint_config.auto_checkpoint_enabled;
    config.checkpoint_config.auto_checkpoint_enabled = enabled;
    previous
}

fn rollback_auto_checkpoint_enabled(config: &mut AppConfig, previous: bool) {
    config.checkpoint_config.auto_checkpoint_enabled = previous;
}

#[tauri::command]
pub async fn set_auto_checkpoint_enabled(
    enabled: bool,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    let previous = {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        apply_auto_checkpoint_enabled(&mut config, enabled)
    };

    // 保存配置到文件（持久化，重启后保持）
    if let Err(error) = save_config(&state, &app).await {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("保存配置失败: {}; 回滚配置失败: {}", error, e))?;
        rollback_auto_checkpoint_enabled(&mut config, previous);
        return Err(format!("保存配置失败: {}", error));
    }

    log::info!("用户切换自动检查点为: {} (已保存配置)", enabled);

    Ok(())
}

#[tauri::command]
pub async fn sync_window_state(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // 根据配置同步窗口状态
    let always_on_top = {
        let config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.ui_config.always_on_top
    };

    // 应用到当前窗口
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            window
                .set_always_on_top(always_on_top)
                .map_err(|e| format!("同步窗口状态失败: {}", e))?;
        }

        #[cfg(any(target_os = "android", target_os = "ios"))]
        {
            let _ = window;
            log::debug!("移动端跳过窗口置顶同步: {}", always_on_top);
        }
    }

    Ok(())
}

/// 重新加载配置文件到内存
#[tauri::command]
pub async fn reload_config(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // 从文件重新加载配置到内存
    load_config(&state, &app)
        .await
        .map_err(|e| format!("重新加载配置失败: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn get_theme(state: State<'_, AppState>) -> Result<String, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.ui_config.theme.clone())
}

#[tauri::command]
pub async fn set_theme(
    theme: String,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    // 验证主题值
    if !["light", "dark"].contains(&theme.as_str()) {
        return Err("无效的主题值，只支持 light、dark".to_string());
    }

    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.ui_config.theme = theme;
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn get_window_config(state: State<'_, AppState>) -> Result<WindowConfig, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.ui_config.window_config.clone())
}

#[tauri::command]
pub async fn set_window_config(
    window_config: WindowConfig,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.ui_config.window_config = window_config;
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn get_reply_config(state: State<'_, AppState>) -> Result<ReplyConfig, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.reply_config.clone())
}

#[tauri::command]
pub async fn set_reply_config(
    reply_config: ReplyConfig,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.reply_config = reply_config;
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn get_window_settings(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;

    // 返回窗口设置，包含两种模式的独立尺寸
    let window_settings = serde_json::json!({
        "fixed": config.ui_config.window_config.fixed,
        "current_width": config.ui_config.window_config.current_width(),
        "current_height": config.ui_config.window_config.current_height(),
        "fixed_width": config.ui_config.window_config.fixed_width,
        "fixed_height": config.ui_config.window_config.fixed_height,
        "free_width": config.ui_config.window_config.free_width,
        "free_height": config.ui_config.window_config.free_height
    });

    Ok(window_settings)
}

#[tauri::command]
pub async fn get_window_settings_for_mode(
    fixed: bool,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;

    // 返回指定模式的窗口设置
    let (width, height) = if fixed {
        (
            config.ui_config.window_config.fixed_width,
            config.ui_config.window_config.fixed_height,
        )
    } else {
        (
            config.ui_config.window_config.free_width,
            config.ui_config.window_config.free_height,
        )
    };

    let window_settings = serde_json::json!({
        "width": width,
        "height": height,
        "fixed": fixed
    });

    Ok(window_settings)
}

#[tauri::command]
pub async fn get_window_constraints_cmd() -> Result<serde_json::Value, String> {
    let constraints = window::get_default_constraints();
    let ui_timings = ui::get_default_ui_timings();

    let mut result = constraints.to_json();
    if let serde_json::Value::Object(ref mut map) = result {
        if let serde_json::Value::Object(ui_map) = ui_timings.to_json() {
            map.extend(ui_map);
        }
    }

    Ok(result)
}

#[tauri::command]
pub async fn get_current_window_size(app: tauri::AppHandle) -> Result<serde_json::Value, String> {
    if let Some(window) = app.get_webview_window("main") {
        // 检查窗口是否最小化
        if let Ok(is_minimized) = window.is_minimized() {
            if is_minimized {
                return Err("窗口已最小化，跳过尺寸获取".to_string());
            }
        }

        // 获取逻辑尺寸而不是物理尺寸
        if let Ok(logical_size) = window.inner_size().map(|physical_size| {
            // 获取缩放因子
            let scale_factor = window.scale_factor().unwrap_or(1.0);

            // 转换为逻辑尺寸
            let logical_width = physical_size.width as f64 / scale_factor;
            let logical_height = physical_size.height as f64 / scale_factor;

            tauri::LogicalSize::new(logical_width, logical_height)
        }) {
            let width = logical_size.width.round() as u32;
            let height = logical_size.height.round() as u32;

            // 验证并调整尺寸到有效范围
            let (clamped_width, clamped_height) =
                crate::constants::window::clamp_window_size(width as f64, height as f64);
            let final_width = clamped_width as u32;
            let final_height = clamped_height as u32;

            if final_width != width || final_height != height {
                log::info!(
                    "窗口尺寸已调整: {}x{} -> {}x{}",
                    width,
                    height,
                    final_width,
                    final_height
                );
            }

            let window_size = serde_json::json!({
                "width": final_width,
                "height": final_height
            });
            return Ok(window_size);
        }
    }

    Err("无法获取当前窗口大小".to_string())
}

#[tauri::command]
pub async fn set_window_settings(
    window_settings: serde_json::Value,
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 更新窗口配置
        if let Some(fixed) = window_settings.get("fixed").and_then(|v| v.as_bool()) {
            config.ui_config.window_config.fixed = fixed;
        }

        // 更新固定模式尺寸（添加尺寸验证）
        if let Some(width) = window_settings.get("fixed_width").and_then(|v| v.as_f64()) {
            if let Some(height) = window_settings.get("fixed_height").and_then(|v| v.as_f64()) {
                if validation::is_valid_window_size(width, height) {
                    config.ui_config.window_config.fixed_width = width;
                    config.ui_config.window_config.fixed_height = height;
                }
            } else if width >= window::MIN_WIDTH {
                config.ui_config.window_config.fixed_width = width;
            }
        } else if let Some(height) = window_settings.get("fixed_height").and_then(|v| v.as_f64()) {
            if height >= window::MIN_HEIGHT {
                config.ui_config.window_config.fixed_height = height;
            }
        }

        // 更新自由拉伸模式尺寸（添加尺寸验证）
        if let Some(width) = window_settings.get("free_width").and_then(|v| v.as_f64()) {
            if let Some(height) = window_settings.get("free_height").and_then(|v| v.as_f64()) {
                if validation::is_valid_window_size(width, height) {
                    config.ui_config.window_config.free_width = width;
                    config.ui_config.window_config.free_height = height;
                }
            } else if width >= window::MIN_WIDTH {
                config.ui_config.window_config.free_width = width;
            }
        } else if let Some(height) = window_settings.get("free_height").and_then(|v| v.as_f64()) {
            if height >= window::MIN_HEIGHT {
                config.ui_config.window_config.free_height = height;
            }
        }

        // 兼容旧的width/height参数，更新当前模式的尺寸（添加尺寸验证）
        if let (Some(width), Some(height)) = (
            window_settings.get("width").and_then(|v| v.as_f64()),
            window_settings.get("height").and_then(|v| v.as_f64()),
        ) {
            if validation::is_valid_window_size(width, height) {
                config
                    .ui_config
                    .window_config
                    .update_current_size(width, height);
            }
        }
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

#[tauri::command]
pub fn ack_mcp_request_ready(
    project_path: Option<String>,
    request_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let route_key = resolve_tree_route_key(request_id.as_deref(), project_path.as_deref())
        .ok_or_else(|| "缺少 request_id/project_path，无法确认 MCP 请求已接收".to_string())?;

    let sender = {
        let mut channels = state
            .request_ready_channels
            .lock()
            .map_err(|e| format!("获取 ready 通道失败: {}", e))?;
        channels.remove(&route_key)
    };

    if let Some(sender) = sender {
        let _ = sender.send(());
        log::info!(
            "[Conversation] ack_mcp_request_ready 成功: route_key={}",
            route_key
        );
    } else {
        log::info!(
            "[Conversation] ack_mcp_request_ready 忽略: 未找到 route_key={}",
            route_key
        );
    }

    if let Ok(ready_file) = std::env::var("ITERATE_READY_FILE") {
        let ready_payload = serde_json::json!({
            "request_id": request_id,
            "project_path": project_path,
            "ready_at": chrono::Utc::now().to_rfc3339(),
        });
        std::fs::write(
            &ready_file,
            serde_json::to_string(&ready_payload)
                .map_err(|e| format!("序列化 ready 文件失败: {}", e))?,
        )
        .map_err(|e| format!("写入 ready 文件失败: {}", e))?;
        log::info!("[Conversation] standalone ready 文件已写入: {}", ready_file);
    }

    Ok(())
}

fn send_response_to_route_channel(
    sender: tokio::sync::oneshot::Sender<String>,
    response_str: String,
    lookup_key: &str,
) -> Result<(), String> {
    sender
        .send(response_str)
        .map_err(|_| format!("发送响应到 {} 失败", lookup_key))
}

#[derive(Debug, Clone)]
pub(crate) struct RecordedConversationNode {
    tree_id: String,
    node_id: String,
    parent_id: Option<String>,
    request_key: Option<String>,
    conversation_route_id: Option<String>,
    actual_request_id: Option<String>,
}

fn attach_conversation_metadata(
    response: &mut serde_json::Value,
    recorded: &RecordedConversationNode,
) {
    let Some(map) = response.as_object_mut() else {
        return;
    };
    let metadata = map
        .entry("metadata")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    if !metadata.is_object() {
        *metadata = serde_json::Value::Object(serde_json::Map::new());
    }
    let Some(metadata_map) = metadata.as_object_mut() else {
        return;
    };

    metadata_map.insert(
        "conversation_id".to_string(),
        serde_json::Value::String(recorded.tree_id.clone()),
    );
    metadata_map.insert(
        "tree_id".to_string(),
        serde_json::Value::String(recorded.tree_id.clone()),
    );
    metadata_map.insert(
        "current_node_id".to_string(),
        serde_json::Value::String(recorded.node_id.clone()),
    );
    metadata_map.insert(
        "node_id".to_string(),
        serde_json::Value::String(recorded.node_id.clone()),
    );
    if let Some(parent_id) = recorded.parent_id.as_ref() {
        metadata_map.insert(
            "parent_node_id".to_string(),
            serde_json::Value::String(parent_id.clone()),
        );
    }
    if let Some(request_key) = recorded.request_key.as_ref() {
        metadata_map.insert(
            "request_key".to_string(),
            serde_json::Value::String(request_key.clone()),
        );
    }
    if let Some(conversation_route_id) = recorded.conversation_route_id.as_ref() {
        metadata_map.insert(
            "timeline_route_id".to_string(),
            serde_json::Value::String(conversation_route_id.clone()),
        );
        metadata_map.insert(
            "conversation_route_id".to_string(),
            serde_json::Value::String(conversation_route_id.clone()),
        );
    }
    if let Some(actual_request_id) = recorded.actual_request_id.as_ref() {
        metadata_map.insert(
            "actual_request_id".to_string(),
            serde_json::Value::String(actual_request_id.clone()),
        );
        metadata_map
            .entry("request_id")
            .or_insert_with(|| serde_json::Value::String(actual_request_id.clone()));
    }
}

#[tauri::command]
pub async fn send_mcp_response(
    mut response: serde_json::Value,
    project_path: Option<String>,
    request_id: Option<String>,
    timeline_route_id: Option<String>,
    app: AppHandle,
    state: State<'_, AppState>,
    conversation_manager: State<'_, Arc<ConversationManager>>,
) -> Result<(), String> {
    let normalized_project_path = normalize_non_empty(project_path.clone());
    let normalized_request_id = normalize_non_empty(request_id.clone());
    let normalized_timeline_route_id = normalize_non_empty(timeline_route_id.clone());
    let metadata_request_id = extract_response_request_id(&response);
    let effective_request_id = normalized_request_id
        .clone()
        .or_else(|| metadata_request_id.clone());
    let live_goal_route_id = if normalized_timeline_route_id.is_none() {
        crate::ui::live_goal::live_goal_codex_thread_id_for_project_with_app(
            Some(&app),
            normalized_project_path.as_deref(),
        )
    } else {
        None
    };
    let conversation_route_id = normalized_timeline_route_id
        .clone()
        .or_else(|| live_goal_route_id.clone())
        .or_else(|| effective_request_id.clone());

    maybe_attach_hui_snapshot(
        &mut response,
        normalized_project_path.as_deref(),
        conversation_route_id.as_deref(),
        conversation_manager.inner(),
    )
    .await;

    // 检查是否为 standalone 模式（--ui 模式）
    let is_standalone_mode = std::env::var("ITERATE_STANDALONE_MODE").is_ok();

    // 检查是否为MCP模式
    let args: Vec<String> = std::env::args().collect();
    let is_mcp_mode = args.len() >= 3 && args[1] == "--mcp-request";
    let route_key = resolve_tree_route_key(
        effective_request_id.as_deref(),
        normalized_project_path.as_deref(),
    );
    let conversation_route_key = resolve_tree_route_key(
        conversation_route_id.as_deref(),
        normalized_project_path.as_deref(),
    );
    append_timeline_debug_log(
        "rust/ui::send_mcp_response:entry",
        serde_json::json!({
            "request_id_arg": request_id,
            "timeline_route_id_arg": timeline_route_id,
            "metadata_request_id": metadata_request_id,
            "effective_request_id": effective_request_id,
            "live_goal_route_id": live_goal_route_id,
            "conversation_route_id": conversation_route_id,
            "project_path": normalized_project_path,
            "route_key": route_key,
            "conversation_route_key": conversation_route_key,
            "is_standalone_mode": is_standalone_mode,
            "is_mcp_mode": is_mcp_mode,
            "response_len": response.to_string().len(),
        }),
    );

    crate::ui::live_goal::apply_live_goal_intent_from_response(
        Some(&app),
        &response,
        normalized_project_path.as_deref(),
        effective_request_id.as_deref(),
    );

    log::info!(
        "[Conversation] send_mcp_response 开始: request_id={:?}, project_path={:?}, standalone={}, mcp_mode={}, response_len={}",
        effective_request_id,
        normalized_project_path,
        is_standalone_mode,
        is_mcp_mode,
        response.to_string().len()
    );
    eprintln!(
        "[Conversation] send_mcp_response start: request_id_arg={:?}, timeline_route_id_arg={:?}, metadata_request_id={:?}, effective_request_id={:?}, conversation_route_id={:?}, project_path={:?}, route_key={:?}, standalone={}, mcp_mode={}, response_len={}",
        request_id,
        timeline_route_id,
        metadata_request_id,
        effective_request_id,
        conversation_route_id,
        normalized_project_path,
        route_key,
        is_standalone_mode,
        is_mcp_mode,
        response.to_string().len()
    );

    // 先把用户回复写进 conversation tree，避免后续 stdout / 路由分支提前返回时漏记节点。
    eprintln!(
        "[Conversation] send_mcp_response record_user_response_node: source=send_mcp_response, effective_request_id={:?}, conversation_route_id={:?}, project_path={:?}",
        effective_request_id,
        conversation_route_id,
        normalized_project_path
    );
    match record_user_response_node(
        Some(&app),
        conversation_manager.inner(),
        &response,
        normalized_project_path.clone(),
        effective_request_id.clone(),
        conversation_route_id.clone(),
        "send_mcp_response",
    )
    .await
    {
        Ok(Some(recorded)) => {
            attach_conversation_metadata(&mut response, &recorded);
            eprintln!(
                "[Conversation] send_mcp_response record_user_response_node success: tree_id={}, node_id={}",
                recorded.tree_id, recorded.node_id
            );
        }
        Ok(None) => {
            eprintln!("[Conversation] send_mcp_response record_user_response_node skipped");
        }
        Err(e) => {
            log::warn!("[Conversation] 记录用户节点失败: {}", e);
            eprintln!(
                "[Conversation] send_mcp_response record_user_response_node failed: {}",
                e
            );
        }
    }

    // 将响应序列化为JSON字符串；必须在 conversation metadata 注入之后执行。
    let response_str =
        serde_json::to_string(&response).map_err(|e| format!("序列化响应失败: {}", e))?;

    if response_str.trim().is_empty() {
        return Err("响应内容不能为空".to_string());
    }

    let mut route_error: Option<String> = None;

    if is_standalone_mode || is_mcp_mode {
        // Standalone/MCP模式：输出到stdout
        println!("{}", response_str);
        std::io::Write::flush(&mut std::io::stdout())
            .map_err(|e| format!("刷新stdout失败: {}", e))?;

        // 如果设置了 ITERATE_RESPONSE_FILE，同时写入文件（--serve 模式需要）
        if let Ok(response_file) = std::env::var("ITERATE_RESPONSE_FILE") {
            std::fs::write(&response_file, &response_str)
                .map_err(|e| format!("写入响应文件失败: {}", e))?;
        }
        // 对话已结束，通知主进程清除 MCP_STATE_CACHE（子进程内存与主进程隔离）
        // 这里必须等待请求返回；standalone 子进程退出很快，fire-and-forget 会导致清理丢失。
        if let Some(ref rid) = effective_request_id {
            let client = reqwest::Client::new();
            let cleanup_url = format!("{}/api/cleanup-session", bridge_base_url());
            let request = client
                .post(&cleanup_url)
                .json(&serde_json::json!({ "request_id": rid }))
                .timeout(std::time::Duration::from_secs(2));
            let result = match crate::bridge::auth::authorize_internal_bridge_request(
                request,
                "POST",
                &cleanup_url,
            ) {
                Ok(request) => request
                    .send()
                    .await
                    .map(|_| ())
                    .map_err(|error| error.to_string()),
                Err(error) => Err(error),
            };
            if let Err(err) = result {
                log::warn!("[IPC] cleanup-session 调用失败: {}", err);
            }
        }
        let mut registry = crate::ui::window_registry::WindowRegistry::load();
        let _ = registry.clear_request_binding();
    } else {
        // 通过channel发送响应（优先用 request_id 路由，fallback 到 project_path）
        let rid_for_cleanup = effective_request_id.clone();
        let lookup_key = route_key.clone().unwrap_or_else(|| "Unknown".to_string());
        log::info!(
            "[Conversation] send_mcp_response 尝试路由响应: lookup_key={}",
            lookup_key
        );
        eprintln!(
            "[Conversation] send_mcp_response route: lookup_key={}, route_key={:?}, effective_request_id={:?}, project_path={:?}",
            lookup_key,
            route_key,
            effective_request_id,
            normalized_project_path
        );
        let sender = {
            let mut channels = state
                .response_channels
                .lock()
                .map_err(|e| format!("获取响应通道失败: {}", e))?;
            let keys = channels.keys().cloned().collect::<Vec<String>>();
            log::info!(
                "[Conversation] send_mcp_response 当前可用响应通道: {:?}",
                keys
            );
            channels.remove(&lookup_key)
        };

        if let Some(sender) = sender {
            match send_response_to_route_channel(sender, response_str.clone(), &lookup_key) {
                Ok(()) => {
                    log::info!(
                        "[Conversation] send_mcp_response 路由成功: lookup_key={}",
                        lookup_key
                    );
                    eprintln!(
                        "[Conversation] send_mcp_response route success: lookup_key={}",
                        lookup_key
                    );

                    // 对话已确认送达后走统一 cleanup，避免 active popup route 残留。
                    if let Some(rid) = rid_for_cleanup {
                        let (removed_cache, removed_active) =
                            crate::bridge::ws::cleanup_completed_session_by_request_id(
                                &rid,
                                "send-mcp-response-route-success",
                            )
                            .await;
                        log::info!(
                            "[IPC] route success cleanup: request_id={}, removed_cache={}, removed_active={}",
                            rid,
                            removed_cache,
                            removed_active
                        );
                    }
                    let mut registry = crate::ui::window_registry::WindowRegistry::load();
                    let _ = registry.clear_request_binding();
                }
                Err(err) => {
                    log::warn!("[Conversation] {}", err);
                    eprintln!("[Conversation] send_mcp_response route failed: {}", err);
                    route_error = Some(err);
                }
            }
        } else {
            let err = format!("未找到 {} 的响应通道", lookup_key);
            let keys_snapshot = state
                .response_channels
                .lock()
                .map(|channels| channels.keys().cloned().collect::<Vec<String>>())
                .unwrap_or_default();
            append_timeline_debug_log(
                "rust/ui::send_mcp_response:route_miss",
                serde_json::json!({
                    "pid": std::process::id(),
                    "lookup_key": lookup_key,
                    "route_key": route_key,
                    "effective_request_id": effective_request_id,
                    "project_path": normalized_project_path,
                    "is_standalone_mode": is_standalone_mode,
                    "is_mcp_mode": is_mcp_mode,
                    "available_response_channels": keys_snapshot,
                }),
            );
            log::warn!("[Conversation] {}", err);
            eprintln!(
                "[Conversation] send_mcp_response route miss: {}, pid={}, route_key={:?}, effective_request_id={:?}, project_path={:?}",
                err,
                std::process::id(),
                route_key,
                effective_request_id,
                normalized_project_path
            );
            route_error = Some(err);
        }
    }

    if let Some(err) = route_error {
        return Err(err);
    }

    log::info!(
        "[Conversation] send_mcp_response 完成: request_id={:?}, project_path={:?}",
        effective_request_id,
        normalized_project_path
    );
    eprintln!(
        "[Conversation] send_mcp_response done: request_id={:?}, project_path={:?}",
        effective_request_id, normalized_project_path
    );
    Ok(())
}

fn is_hui_trigger(input: &str) -> bool {
    let trimmed = input.trim_start();
    if trimmed.is_empty() {
        return false;
    }

    fn is_separator(ch: char) -> bool {
        ch.is_whitespace()
            || matches!(
                ch,
                ',' | '，'
                    | '.'
                    | '。'
                    | ':'
                    | '：'
                    | '!'
                    | '！'
                    | '?'
                    | '？'
                    | '、'
                    | '-'
                    | '—'
                    | '+'
                    | '＋'
                    | '➕'
            )
    }

    if let Some(rest) = trimmed.strip_prefix('回') {
        return rest.is_empty() || rest.chars().next().is_some_and(is_separator);
    }

    let lower = trimmed.to_lowercase();
    if let Some(rest) = lower.strip_prefix("hui") {
        if let Some(after_mode) = rest.strip_prefix('0').or_else(|| rest.strip_prefix('1')) {
            return after_mode.is_empty() || after_mode.chars().next().is_some_and(is_separator);
        }
        return rest.is_empty() || rest.chars().next().is_some_and(is_separator);
    }

    false
}

fn summarize_hui_user_input(raw: &str) -> String {
    raw.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(120)
        .collect::<String>()
}

#[derive(Debug)]
struct HuiLatestAnchor {
    date: String,
    time: String,
    file_path: String,
    user_input_summary: String,
    conversation_id: Option<String>,
    current_node_id: Option<String>,
    request_id: Option<String>,
    timeline_route_id: Option<String>,
    run_id: Option<String>,
    generation: Option<u64>,
    stale_of: Option<String>,
    superseded_by: Option<String>,
}

#[derive(Debug, Default)]
struct HuiSnapshotQuery<'a> {
    project_path: Option<&'a str>,
    request_id: Option<&'a str>,
    run_id: Option<&'a str>,
    generation: Option<u64>,
    conversations_root: Option<&'a Path>,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct HuiConversationMetadata {
    conversation_id: Option<String>,
    current_node_id: Option<String>,
    request_id: Option<String>,
    timeline_route_id: Option<String>,
    run_id: Option<String>,
    generation: Option<u64>,
    stale_of: Option<String>,
    superseded_by: Option<String>,
}

#[derive(Debug)]
struct HuiAnchorCandidate {
    anchor: HuiLatestAnchor,
    run_match: bool,
    route_match: bool,
    fresh: bool,
    file_score: i32,
    modified_at: std::time::SystemTime,
    block_index: usize,
    day_offset: i64,
}

fn normalize_hui_meta_str(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn hui_meta_value_matches(candidate: Option<&str>, expected: Option<&str>) -> bool {
    let Some(candidate) = normalize_hui_meta_str(candidate) else {
        return false;
    };
    let Some(expected) = normalize_hui_meta_str(expected) else {
        return false;
    };
    candidate == expected
}

fn parse_hui_conversation_metadata(block: &str) -> Option<HuiConversationMetadata> {
    let prefix = "<!-- cunzhi-meta:";
    let start = block.find(prefix)?;
    let rest = &block[start + prefix.len()..];
    let end = rest.find("-->")?;
    let json = rest[..end].trim();
    serde_json::from_str(json).ok()
}

fn hui_anchor_run_matches(
    metadata: &HuiConversationMetadata,
    query: &HuiSnapshotQuery<'_>,
) -> bool {
    if hui_meta_value_matches(metadata.run_id.as_deref(), query.run_id) {
        return true;
    }

    metadata
        .generation
        .zip(query.generation)
        .is_some_and(|(candidate, expected)| candidate == expected)
}

fn hui_anchor_route_matches(
    metadata: &HuiConversationMetadata,
    query: &HuiSnapshotQuery<'_>,
) -> bool {
    hui_meta_value_matches(metadata.timeline_route_id.as_deref(), query.request_id)
        || hui_meta_value_matches(metadata.request_id.as_deref(), query.request_id)
        || hui_meta_value_matches(metadata.conversation_id.as_deref(), query.request_id)
}

fn find_hui_latest_anchor(query: &HuiSnapshotQuery<'_>) -> Option<HuiLatestAnchor> {
    let project_names = collect_project_names(query.project_path);
    let conversations_root = query
        .conversations_root
        .map(Path::to_path_buf)
        .or_else(|| {
            dirs::home_dir().map(|home_dir| home_dir.join(".cunzhi-knowledge/conversations"))
        })?;
    let time_regex = Regex::new(r"(?m)^##\s+(\d{2}:\d{2}:\d{2})").ok()?;
    let mut candidates = Vec::new();

    for day_offset in 0..3 {
        let date = chrono::Local::now() - chrono::Duration::days(day_offset);
        let date_str = date.format("%Y-%m-%d").to_string();
        let day_dir = conversations_root.join(&date_str);
        if !day_dir.exists() {
            continue;
        }

        let mut ranked_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&day_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let file_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_lowercase())
                    .unwrap_or_default();

                let score = if project_names.iter().any(|project_name| {
                    file_name.starts_with(&format!("{project_name}__"))
                        || file_name == format!("{project_name}.md")
                }) {
                    3
                } else if project_names
                    .iter()
                    .any(|project_name| file_name.contains(project_name))
                {
                    2
                } else if file_name == format!("{date_str}.md") {
                    1
                } else {
                    0
                };

                if score == 0 {
                    continue;
                }

                let modified_at = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                ranked_files.push((score, modified_at, path));
            }
        }

        ranked_files.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        for (file_score, modified_at, path) in ranked_files {
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };

            let segments = content.split("\n---").map(str::trim).collect::<Vec<_>>();

            for (block_index, block) in segments.iter().enumerate() {
                if block.is_empty() {
                    continue;
                }

                let Some(time) = time_regex
                    .captures(block)
                    .and_then(|captures| captures.get(1).map(|m| m.as_str().to_string()))
                else {
                    continue;
                };

                let metadata = parse_hui_conversation_metadata(block).unwrap_or_default();

                let user_input_summary = block
                    .split("### 👤 用户")
                    .nth(1)
                    .map(str::trim)
                    .filter(|text| !text.is_empty())
                    .map(summarize_hui_user_input)
                    .unwrap_or_else(|| "无用户输入".to_string());

                candidates.push(HuiAnchorCandidate {
                    run_match: hui_anchor_run_matches(&metadata, query),
                    route_match: hui_anchor_route_matches(&metadata, query),
                    fresh: metadata.stale_of.is_none(),
                    file_score,
                    modified_at,
                    block_index,
                    day_offset,
                    anchor: HuiLatestAnchor {
                        date: date_str.clone(),
                        time,
                        file_path: path.to_string_lossy().to_string(),
                        user_input_summary,
                        conversation_id: metadata.conversation_id,
                        current_node_id: metadata.current_node_id,
                        request_id: metadata.request_id,
                        timeline_route_id: metadata.timeline_route_id,
                        run_id: metadata.run_id,
                        generation: metadata.generation,
                        stale_of: metadata.stale_of,
                        superseded_by: metadata.superseded_by,
                    },
                });
            }
        }
    }

    let has_run_scope =
        normalize_hui_meta_str(query.run_id).is_some() || query.generation.is_some();
    let has_route_scope = normalize_hui_meta_str(query.request_id).is_some();
    if has_run_scope {
        candidates.retain(|candidate| candidate.run_match);
    } else if has_route_scope {
        candidates.retain(|candidate| candidate.route_match);
    }

    candidates.sort_by(|a, b| {
        b.run_match
            .cmp(&a.run_match)
            .then_with(|| b.route_match.cmp(&a.route_match))
            .then_with(|| b.fresh.cmp(&a.fresh))
            .then_with(|| b.file_score.cmp(&a.file_score))
            .then_with(|| a.day_offset.cmp(&b.day_offset))
            .then_with(|| b.modified_at.cmp(&a.modified_at))
            .then_with(|| b.block_index.cmp(&a.block_index))
    });

    candidates
        .into_iter()
        .next()
        .map(|candidate| candidate.anchor)
}

fn hui_optional_label(value: Option<&str>) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("无")
        .to_string()
}

async fn build_hui_snapshot(
    query: HuiSnapshotQuery<'_>,
    manager: &ConversationManager,
) -> Option<String> {
    let latest_anchor = find_hui_latest_anchor(&query)?;
    let mut tree_id = manager
        .get_tree_for_route(query.request_id, query.project_path)
        .await;
    if tree_id.is_none()
        && query
            .request_id
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
    {
        tree_id = manager.get_tree_for_route(None, query.project_path).await;
    }

    let (tree_id_label, current_node_id, latest_node_timestamp) = if let Some(tree_id) = tree_id {
        let current_node_id = manager
            .get_current_node_id(&tree_id)
            .await
            .or_else(|| latest_anchor.current_node_id.clone());
        let latest_node_timestamp = if let Some(node_id) = current_node_id.as_deref() {
            manager
                .get_node(&tree_id, node_id)
                .await
                .map(|node| node.timestamp)
        } else {
            None
        };
        (tree_id, current_node_id, latest_node_timestamp)
    } else {
        (
            latest_anchor
                .conversation_id
                .clone()
                .unwrap_or_else(|| "无".to_string()),
            latest_anchor.current_node_id.clone(),
            None,
        )
    };

    let latest_node_local = latest_node_timestamp
        .as_deref()
        .and_then(|timestamp| chrono::DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| {
            timestamp
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        });

    Some(format!(
        "## Hui Snapshot\n- 日期：`{}`\n- 时间：`{}`\n- 文件：`{}`\n- 最新用户输入：{}\n- tree_id：`{}`\n- 当前节点：`{}`\n- 当前 tree 最新时间：`{}`\n- meta.route：`{}`\n- meta.request_id：`{}`\n- meta.run_id：`{}`\n- meta.generation：`{}`\n- meta.stale_of：`{}`\n- meta.superseded_by：`{}`",
        latest_anchor.date,
        latest_anchor.time,
        latest_anchor.file_path,
        latest_anchor.user_input_summary,
        tree_id_label,
        current_node_id.unwrap_or_else(|| "无".to_string()),
        latest_node_local
            .or(latest_node_timestamp)
            .unwrap_or_else(|| "无".to_string()),
        hui_optional_label(latest_anchor.timeline_route_id.as_deref()),
        hui_optional_label(latest_anchor.request_id.as_deref()),
        hui_optional_label(latest_anchor.run_id.as_deref()),
        latest_anchor
            .generation
            .map(|generation| generation.to_string())
            .unwrap_or_else(|| "无".to_string()),
        hui_optional_label(latest_anchor.stale_of.as_deref()),
        hui_optional_label(latest_anchor.superseded_by.as_deref())
    ))
}

#[tauri::command]
pub async fn get_hui_snapshot(
    project_path: Option<String>,
    request_id: Option<String>,
    run_id: Option<String>,
    generation: Option<u64>,
    conversation_manager: State<'_, Arc<ConversationManager>>,
) -> Result<Option<String>, String> {
    Ok(build_hui_snapshot(
        HuiSnapshotQuery {
            project_path: project_path.as_deref(),
            request_id: request_id.as_deref(),
            run_id: run_id.as_deref(),
            generation,
            conversations_root: None,
        },
        conversation_manager.inner(),
    )
    .await)
}

async fn maybe_attach_hui_snapshot(
    response: &mut serde_json::Value,
    project_path: Option<&str>,
    request_id: Option<&str>,
    manager: &ConversationManager,
) {
    let Some(user_input) = response
        .get("user_input")
        .and_then(|value| value.as_str())
        .map(str::trim)
    else {
        return;
    };

    if !is_hui_trigger(user_input) {
        return;
    }

    let Some(snapshot) = build_hui_snapshot(
        HuiSnapshotQuery {
            project_path,
            request_id,
            ..Default::default()
        },
        manager,
    )
    .await
    else {
        return;
    };

    let Some(map) = response.as_object_mut() else {
        return;
    };
    let metadata = map
        .entry("metadata")
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let Some(metadata_map) = metadata.as_object_mut() else {
        return;
    };

    metadata_map.insert(
        "hui_snapshot".to_string(),
        serde_json::Value::String(snapshot),
    );
}

pub(crate) async fn record_user_response_node(
    app_handle: Option<&AppHandle>,
    manager: &ConversationManager,
    response: &serde_json::Value,
    project_path: Option<String>,
    request_id: Option<String>,
    timeline_route_id: Option<String>,
    source: &str,
) -> Result<Option<RecordedConversationNode>, String> {
    let Some(content) = extract_user_response_content(response) else {
        log::warn!(
            "[Conversation] 跳过用户节点记录: 无可用内容 (source={}, request_id={:?}, project_path={:?})",
            source,
            request_id,
            project_path
        );
        eprintln!(
            "[Conversation] record_user_response_node skipped: no content (source={}, request_id={:?}, project_path={:?})",
            source, request_id, project_path
        );
        append_timeline_debug_log(
            "rust/ui::record_user_response_node:skipped",
            serde_json::json!({
                "reason": "no_content",
                "source": source,
                "request_id": request_id,
                "project_path": project_path,
            }),
        );
        return Ok(None);
    };
    let conversation_route_id = normalize_non_empty(timeline_route_id.clone())
        .or_else(|| {
            app_handle.and_then(|app| {
                crate::ui::live_goal::live_goal_codex_thread_id_for_project_with_app(
                    Some(app),
                    project_path.as_deref(),
                )
            })
        })
        .or_else(|| request_id.clone());
    let request_key =
        resolve_tree_route_key(conversation_route_id.as_deref(), project_path.as_deref());
    append_timeline_debug_log(
        "rust/ui::record_user_response_node:start",
        serde_json::json!({
            "source": source,
            "request_id": request_id,
            "timeline_route_id": timeline_route_id,
            "conversation_route_id": conversation_route_id,
            "project_path": project_path,
            "request_key": request_key,
            "content_len": content.chars().count(),
        }),
    );
    eprintln!(
        "[Conversation] record_user_response_node start: source={}, request_id={:?}, conversation_route_id={:?}, project_path={:?}, request_key={:?}, content_len={}",
        source,
        request_id,
        conversation_route_id,
        project_path,
        request_key,
        content.chars().count()
    );
    log::info!(
        "[Conversation] 开始记录用户节点: source={}, request_key={:?}, content_len={}",
        source,
        request_key,
        content.chars().count()
    );
    let tree_id = manager
        .get_or_create_tree_for_route(conversation_route_id.as_deref(), project_path.as_deref())
        .await;
    let parent_id = manager.get_current_node_id(&tree_id).await;
    let checkpoint_context =
        lookup_checkpoint_context(request_id.as_deref(), project_path.as_deref()).await;
    append_timeline_debug_log(
        "rust/ui::record_user_response_node:resolved_tree_context",
        serde_json::json!({
            "source": source,
            "tree_id": tree_id,
            "parent_id": parent_id,
            "request_key": request_key,
            "checkpoint_id": checkpoint_context
                .as_ref()
                .and_then(|ctx| ctx.checkpoint_id.clone()),
        }),
    );
    let metadata = NodeMetadata {
        conversation_id: Some(tree_id.clone()),
        project_path: project_path.clone(),
        predefined_options: None,
        selected_option: extract_selected_option(response),
        images: manager.prepare_timeline_images(extract_response_images(response)),
        link_url: None,
        link_title: None,
        request_id: request_key.clone(),
        run_id: extract_response_metadata_string(response, &["run_id", "runId"]),
        generation: extract_response_metadata_u64(
            response,
            &["generation", "run_generation", "runGeneration"],
        ),
        stale_of: extract_response_metadata_string(response, &["stale_of", "staleOf"]),
        superseded_by: extract_response_metadata_string(
            response,
            &["superseded_by", "supersededBy"],
        ),
        checkpoint_id: checkpoint_context
            .as_ref()
            .and_then(|ctx| ctx.checkpoint_id.clone()),
        checkpoint_commit: checkpoint_context
            .as_ref()
            .and_then(|ctx| ctx.checkpoint_commit.clone()),
        checkpoint_message: checkpoint_context
            .as_ref()
            .and_then(|ctx| ctx.checkpoint_message.clone()),
        source: Some(source.to_string()),
    };

    let node_id = match manager
        .add_node(
            &tree_id,
            parent_id.clone(),
            NodeType::User,
            content,
            false,
            metadata,
        )
        .await
    {
        Ok(node_id) => node_id,
        Err(err) => {
            append_timeline_debug_log(
                "rust/ui::record_user_response_node:failed",
                serde_json::json!({
                    "reason": "add_node_failed",
                    "source": source,
                    "tree_id": tree_id,
                    "parent_id": parent_id,
                    "request_key": request_key,
                    "error": err.clone(),
                }),
            );
            return Err(err);
        }
    };
    log::info!(
        "[Conversation] 用户节点记录成功: tree_id={}, node_id={}, source={}",
        tree_id,
        node_id,
        source
    );
    let request_id_for_link = request_key
        .as_deref()
        .or(request_id.as_deref())
        .unwrap_or("");
    if let Some(link) = build_checkpoint_link_entry(
        project_path.as_deref().unwrap_or_default(),
        request_id_for_link,
        None,
        checkpoint_context
            .as_ref()
            .and_then(|ctx| ctx.checkpoint_id.as_deref()),
        checkpoint_context
            .as_ref()
            .and_then(|ctx| ctx.checkpoint_commit.as_deref()),
        checkpoint_context
            .as_ref()
            .and_then(|ctx| ctx.checkpoint_message.as_deref()),
        &tree_id,
        &node_id,
        NodeType::User.as_key(),
        source,
    ) {
        append_checkpoint_link(link);
    }
    eprintln!(
        "[Conversation] record_user_response_node success: tree_id={}, node_id={}, source={}, request_key={:?}",
        tree_id, node_id, source, request_key
    );
    append_timeline_debug_log(
        "rust/ui::record_user_response_node:success",
        serde_json::json!({
            "source": source,
            "tree_id": tree_id,
            "node_id": node_id,
            "parent_id": parent_id,
            "request_key": request_key,
        }),
    );

    if let Some(app_handle) = app_handle {
        if let Err(err) = app_handle.emit(
            "conversation-node-recorded",
            serde_json::json!({
                "tree_id": tree_id,
                "conversation_id": tree_id,
                "node_id": node_id,
                "parent_id": parent_id,
                "node_type": "user",
                "request_key": request_key.clone(),
                "request_id": conversation_route_id,
                "actual_request_id": request_id,
                "project_path": project_path,
                "source": source,
            }),
        ) {
            log::warn!(
                "[Conversation] 用户节点事件广播失败: source={}, error={}",
                source,
                err
            );
        }
    }

    Ok(Some(RecordedConversationNode {
        tree_id,
        node_id,
        parent_id,
        request_key,
        conversation_route_id,
        actual_request_id: request_id,
    }))
}

#[derive(Clone)]
struct CheckpointContext {
    checkpoint_id: Option<String>,
    checkpoint_commit: Option<String>,
    checkpoint_message: Option<String>,
}

async fn lookup_checkpoint_context(
    request_id: Option<&str>,
    project_path: Option<&str>,
) -> Option<CheckpointContext> {
    let cache = crate::bridge::ws::MCP_STATE_CACHE.read().await;
    let payload = request_id
        .and_then(|rid| cache.get(rid))
        .or_else(|| project_path.and_then(|path| cache.get(path)))?;
    let request = payload.get("request")?;
    Some(CheckpointContext {
        checkpoint_id: request
            .get("checkpoint_id")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
        checkpoint_commit: request
            .get("checkpoint_commit")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
        checkpoint_message: request
            .get("checkpoint_message")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned),
    })
}

fn normalize_non_empty(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty() && item != "Unknown")
}

fn extract_response_request_id(response: &serde_json::Value) -> Option<String> {
    response
        .get("metadata")
        .and_then(|metadata| {
            metadata
                .get("request_id")
                .or_else(|| metadata.get("requestId"))
        })
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_response_metadata_string(response: &serde_json::Value, keys: &[&str]) -> Option<String> {
    let metadata = response.get("metadata")?;
    keys.iter()
        .find_map(|key| metadata.get(*key))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_response_metadata_u64(response: &serde_json::Value, keys: &[&str]) -> Option<u64> {
    let metadata = response.get("metadata")?;
    keys.iter().find_map(|key| {
        let value = metadata.get(*key)?;
        value
            .as_u64()
            .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
    })
}

fn extract_user_response_content(response: &serde_json::Value) -> Option<String> {
    match response {
        serde_json::Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        serde_json::Value::Object(map) => {
            let selected_options = extract_selected_options_summary(map);
            let explicit_text = ["user_input", "message", "text", "content"]
                .iter()
                .find_map(|key| {
                    map.get(*key)
                        .and_then(|value| value.as_str())
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                });

            if let Some(options) = selected_options {
                return Some(match explicit_text {
                    Some(text) if text == options => format!("选中的选项: {}", options),
                    Some(text) => format!("选中的选项: {}\n\n{}", options, text),
                    None => format!("选中的选项: {}", options),
                });
            }

            // Only accept explicit user-facing text fields; do not stringify the whole
            // response object, or tool envelopes/metadata will leak into the UI.
            if explicit_text.is_some() {
                return explicit_text;
            }

            if let Some(image_count) = map
                .get("images")
                .and_then(|value| value.as_array())
                .map(Vec::len)
                .filter(|count| *count > 0)
            {
                return Some(format!("[{} image(s)]", image_count));
            }

            None
        }
        _ => None,
    }
}

fn extract_selected_options_summary(
    map: &serde_json::Map<String, serde_json::Value>,
) -> Option<String> {
    map.get("selected_options")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::trim))
                .filter(|item| !item.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<String>>()
        })
        .filter(|items| !items.is_empty())
        .map(|items| items.join(" / "))
}

fn extract_selected_option(response: &serde_json::Value) -> Option<String> {
    response
        .get("selected_options")
        .and_then(|value| value.as_array())
        .and_then(|items| items.first())
        .and_then(|item| item.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_response_images(response: &serde_json::Value) -> Option<Vec<ImageAttachment>> {
    let parsed_images = response
        .get("images")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<ImageAttachment>(item.clone()).ok())
                .collect::<Vec<ImageAttachment>>()
        })
        .unwrap_or_default();

    if parsed_images.is_empty() {
        None
    } else {
        Some(parsed_images)
    }
}

#[tauri::command]
pub fn get_cli_args() -> Result<serde_json::Value, String> {
    let args: Vec<String> = std::env::args().collect();
    let mut result = serde_json::Map::new();

    // 优先检查环境变量（--ui 模式）
    if let Ok(request_file) = std::env::var("ITERATE_MCP_REQUEST_FILE") {
        result.insert(
            "mcp_request".to_string(),
            serde_json::Value::String(request_file.clone()),
        );
        result.insert("standalone_mode".to_string(), serde_json::Value::Bool(true));
        // 调试日志（仅在非 standalone 模式下输出）
        if std::env::var("ITERATE_STANDALONE_MODE").is_err() {
            eprintln!(
                "[get_cli_args] 检测到环境变量 ITERATE_MCP_REQUEST_FILE: {}",
                request_file
            );
        }
        return Ok(serde_json::Value::Object(result));
    }

    // 检查是否有 --mcp-request 参数
    if args.len() >= 3 && args[1] == "--mcp-request" {
        result.insert(
            "mcp_request".to_string(),
            serde_json::Value::String(args[2].clone()),
        );
    }

    // 检查是否有 --ui 参数（但环境变量未设置的情况）
    if args.iter().any(|arg| arg == "--ui") {
        // --ui 模式但环境变量未设置，说明是直接启动的情况
        // 需要从命令行参数中解析请求
        let mut message = String::from("请确认是否继续？");
        let mut options_str = String::new();
        let mut workspace = String::from(".");

        let mut i = 1;
        while i < args.len() {
            match args[i].as_str() {
                "--message" if i + 1 < args.len() => {
                    message = args[i + 1].clone();
                    i += 2;
                }
                "--options" if i + 1 < args.len() => {
                    options_str = args[i + 1].clone();
                    i += 2;
                }
                "--workspace" if i + 1 < args.len() => {
                    workspace = args[i + 1].clone();
                    i += 2;
                }
                _ => i += 1,
            }
        }

        // 解析选项
        let predefined_options: Vec<serde_json::Value> = if options_str.is_empty() {
            vec![]
        } else {
            options_str
                .split(',')
                .map(|s| serde_json::Value::String(s.trim().to_string()))
                .collect()
        };

        // 创建内联请求（不需要临时文件）
        let request_id = format!("standalone-{}", chrono::Utc::now().timestamp_millis());
        let inline_request = serde_json::json!({
            "id": request_id,
            "message": message,
            "predefined_options": predefined_options,
            "is_markdown": true,
            "project_path": workspace
        });

        result.insert("mcp_request_inline".to_string(), inline_request);
        result.insert("standalone_mode".to_string(), serde_json::Value::Bool(true));
    }

    Ok(serde_json::Value::Object(result))
}

#[tauri::command]
pub fn read_mcp_request(file_path: String) -> Result<serde_json::Value, String> {
    if !std::path::Path::new(&file_path).exists() {
        return Err(format!("文件不存在: {}", file_path));
    }

    match std::fs::read_to_string(&file_path) {
        Ok(content) => {
            if content.trim().is_empty() {
                return Err("文件内容为空".to_string());
            }
            match serde_json::from_str(&content) {
                Ok(json) => Ok(json),
                Err(e) => Err(format!("解析JSON失败: {}", e)),
            }
        }
        Err(e) => Err(format!("读取文件失败: {}", e)),
    }
}

/// 列出项目目录中的文件（用于 @文件 选择菜单）
#[tauri::command]
pub fn list_project_files(
    project_path: String,
    max_depth: Option<u32>,
) -> Result<Vec<serde_json::Value>, String> {
    use std::path::Path;

    let path = Path::new(&project_path);
    if !path.exists() || !path.is_dir() {
        return Err(format!("项目路径不存在或不是目录: {}", project_path));
    }

    let max_depth = max_depth.unwrap_or(3);
    let mut files = Vec::new();

    fn collect_files(
        dir: &std::path::Path,
        base_path: &std::path::Path,
        files: &mut Vec<serde_json::Value>,
        current_depth: u32,
        max_depth: u32,
    ) {
        if current_depth > max_depth {
            return;
        }

        // 忽略的目录
        let ignored_dirs = [
            "node_modules",
            ".git",
            "target",
            "dist",
            ".next",
            "__pycache__",
            ".venv",
            "venv",
            ".idea",
            ".vscode",
            "build",
            ".cache",
            ".turbo",
        ];

        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

                // 跳过隐藏文件和忽略的目录
                if file_name.starts_with('.') && file_name != ".env" {
                    continue;
                }

                if path.is_dir() {
                    if ignored_dirs.contains(&file_name) {
                        continue;
                    }
                    // 递归处理子目录
                    collect_files(&path, base_path, files, current_depth + 1, max_depth);
                } else {
                    // 计算相对路径
                    let relative_path = path
                        .strip_prefix(base_path)
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|_| path.to_string_lossy().to_string());

                    // 获取文件扩展名
                    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");

                    files.push(serde_json::json!({
                        "name": file_name,
                        "path": relative_path,
                        "full_path": path.to_string_lossy().to_string(),
                        "extension": extension,
                        "is_dir": false
                    }));
                }
            }
        }
    }

    collect_files(path, path, &mut files, 0, max_depth);

    // 按路径排序
    files.sort_by(|a, b| {
        let path_a = a.get("path").and_then(|v| v.as_str()).unwrap_or("");
        let path_b = b.get("path").and_then(|v| v.as_str()).unwrap_or("");
        path_a.cmp(path_b)
    });

    Ok(files)
}

#[tauri::command]
pub async fn select_image_files() -> Result<Vec<String>, String> {
    // 简化版本：返回测试图片数据
    // 在实际应用中，这里应该调用系统文件对话框
    let test_image_base64 = "data:image/svg+xml;base64,PHN2ZyB3aWR0aD0iMTAwIiBoZWlnaHQ9IjEwMCIgeG1sbnM9Imh0dHA6Ly93d3cudzMub3JnLzIwMDAvc3ZnIj4KICA8cmVjdCB3aWR0aD0iMTAwIiBoZWlnaHQ9IjEwMCIgZmlsbD0iIzMzNzNkYyIvPgogIDx0ZXh0IHg9IjUwIiB5PSI1NSIgZm9udC1mYW1pbHk9IkFyaWFsIiBmb250LXNpemU9IjE0IiBmaWxsPSJ3aGl0ZSIgdGV4dC1hbmNob3I9Im1pZGRsZSI+VGF1cmk8L3RleHQ+Cjwvc3ZnPg==";

    Ok(vec![test_image_base64.to_string()])
}

#[tauri::command]
pub async fn open_external_url(url: String) -> Result<(), String> {
    use std::process::Command;

    // 移除不重要的调试信息

    // 根据操作系统选择合适的命令
    let result = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", &url])
            .without_console_window()
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(&url).spawn()
    } else {
        // Linux 和其他 Unix 系统
        Command::new("xdg-open").arg(&url).spawn()
    };

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("无法打开链接: {}", e)),
    }
}

#[tauri::command]
pub async fn open_local_path(
    path: String,
    project_path: String,
    prefer_editor: Option<bool>,
) -> Result<(), String> {
    use std::process::Command;

    let target = resolve_local_open_target(&path, &project_path)?;
    let metadata = std::fs::metadata(&target.path).map_err(|e| format!("读取文件失败: {}", e))?;
    let resolved_path_string = target.path.to_string_lossy().to_string();

    if prefer_editor.unwrap_or(false) && metadata.is_file() {
        if open_local_target_in_editor(&target).is_ok() {
            return Ok(());
        }
    }

    let result = if cfg!(target_os = "windows") {
        Command::new("cmd")
            .args(["/C", "start", "", &resolved_path_string])
            .without_console_window()
            .spawn()
    } else if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        if metadata.is_file() {
            command.arg("-R");
        }
        command.arg(&target.path).spawn()
    } else {
        Command::new("xdg-open").arg(&target.path).spawn()
    };

    match result {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("无法打开本地文件: {}", e)),
    }
}

#[tauri::command]
pub async fn open_confirmed_external_file(path: String) -> Result<(), String> {
    use std::process::Command;

    let target = resolve_confirmed_external_file_target(&path)?;
    let resolved_path_string = target.to_string_lossy().to_string();

    let result = if cfg!(target_os = "windows") {
        Command::new("explorer").arg(&resolved_path_string).spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open")
            .args(["-R", &resolved_path_string])
            .spawn()
    } else {
        Command::new("xdg-open").arg(&target).spawn()
    };

    result
        .map(|_| ())
        .map_err(|e| format!("无法在 Finder 中定位跨项目文件: {}", e))
}

#[tauri::command]
pub async fn exit_app(app: AppHandle) -> Result<(), String> {
    // 直接调用强制退出，用于程序内部的退出操作（如MCP响应后退出）
    crate::ui::exit::force_exit_app(app).await
}

/// 打开新的 Windsurf 聊天标签页
#[tauri::command]
pub async fn open_new_windsurf_chat() -> Result<(), String> {
    use std::process::Command;

    // Windsurf 新聊天标签页逻辑：
    // 使用 Cmd+T 触发新聊天标签
    let script = r#"
tell application "Windsurf" to activate
delay 0.5
tell application "System Events"
    keystroke "t" using command down
end tell
"#;

    let output = Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("执行 AppleScript 失败: {}", e))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("打开新聊天窗口失败: {}", stderr))
    }
}

#[cfg(target_os = "macos")]
fn escape_applescript_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "macos")]
fn run_applescript(script: &str) -> Result<std::process::Output, String> {
    use std::process::Command;

    Command::new("osascript")
        .arg("-e")
        .arg(script)
        .output()
        .map_err(|e| format!("执行 AppleScript 失败: {}", e))
}

#[cfg(target_os = "macos")]
fn is_automation_permission_error(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("not authorized to send apple events")
        || lower.contains("not permitted to send keystrokes")
        || lower.contains("access not allowed")
        || lower.contains("发送 return 键失败")
        || lower.contains("(-1743)")
        || lower.contains("1002")
}

#[cfg(target_os = "macos")]
type CGKeyCode = u16;

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn CGPostKeyboardEvent(key_char: u16, virtual_key: CGKeyCode, key_down: bool) -> i32;
}

#[cfg(target_os = "macos")]
fn post_return_keypress() -> Result<(), String> {
    const RETURN_KEY: CGKeyCode = 36;

    let down = unsafe { CGPostKeyboardEvent(0, RETURN_KEY, true) };
    let up = unsafe { CGPostKeyboardEvent(0, RETURN_KEY, false) };

    if down == 0 && up == 0 {
        Ok(())
    } else {
        Err(format!("发送 Return 键失败: down={}, up={}", down, up))
    }
}

#[cfg(target_os = "macos")]
fn post_return_keypress_with_applescript() -> Result<(), String> {
    let script = r#"
tell application "Codex" to activate
delay 0.2
tell application "System Events"
    key code 36
end tell
"#;

    let output = run_applescript(script)?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("AppleScript Return 发送失败: {}", stderr))
    }
}

#[cfg(target_os = "macos")]
fn post_return_keypress_to_codex() -> Result<(), String> {
    match post_return_keypress() {
        Ok(()) => Ok(()),
        Err(cg_error) => {
            log::warn!(
                "Codex CGEvent Return 发送失败，尝试 AppleScript Return: {}",
                cg_error
            );
            post_return_keypress_with_applescript()
                .map_err(|apple_error| format!("{}；{}", cg_error, apple_error))
        }
    }
}

#[cfg(target_os = "macos")]
fn open_accessibility_settings() {
    let _ = Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .output();
}

#[cfg(target_os = "macos")]
fn build_codex_new_thread_deeplink(content: &str, project_path: Option<&str>) -> Option<String> {
    let prompt = content.trim();
    if prompt.is_empty() {
        return None;
    }

    let mut query = vec![format!(
        "prompt={}",
        utf8_percent_encode(prompt, NON_ALPHANUMERIC)
    )];

    if let Some(path) = project_path.filter(|path| !path.is_empty()) {
        query.push(format!(
            "path={}",
            utf8_percent_encode(path, NON_ALPHANUMERIC)
        ));
    }

    Some(format!("codex://new?{}", query.join("&")))
}

#[cfg(target_os = "macos")]
const CODEX_DESKTOP_BUNDLE_ID: &str = "com.openai.codex";

#[cfg(target_os = "macos")]
fn codex_deeplink_open_args(url: &str) -> [&str; 3] {
    ["-b", CODEX_DESKTOP_BUNDLE_ID, url]
}

#[cfg(target_os = "macos")]
fn codex_app_open_args() -> [&'static str; 2] {
    ["-b", CODEX_DESKTOP_BUNDLE_ID]
}

#[cfg(target_os = "macos")]
fn launch_codex_desktop_deeplink(url: &str) -> Result<(), String> {
    use std::process::Command;

    let open_result = Command::new("open")
        .args(codex_deeplink_open_args(url))
        .output()
        .map_err(|e| format!("调用 Codex deeplink 失败: {}", e))?;

    if open_result.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&open_result.stderr);
        Err(format!("调用 Codex deeplink 失败: {}", stderr))
    }
}

#[cfg(target_os = "macos")]
fn codex_desktop_cli_candidates() -> Vec<PathBuf> {
    let mut candidates = vec![
        PathBuf::from("/Applications/ChatGPT.app/Contents/Resources/codex"),
        PathBuf::from("/Applications/Codex.app/Contents/Resources/codex"),
        PathBuf::from("/opt/homebrew/bin/codex"),
        PathBuf::from("/usr/local/bin/codex"),
    ];

    if let Some(path_env) = std::env::var_os("PATH") {
        candidates.extend(std::env::split_paths(&path_env).map(|dir| dir.join("codex")));
    }

    candidates.dedup();
    candidates
}

#[cfg(target_os = "macos")]
fn resolve_codex_desktop_cli() -> Result<PathBuf, String> {
    let candidates = codex_desktop_cli_candidates();
    candidates
        .iter()
        .find(|candidate| candidate.is_file())
        .cloned()
        .ok_or_else(|| {
            format!(
                "未找到 Codex Desktop CLI；已检查：{}",
                candidates
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        })
}

#[cfg(target_os = "macos")]
fn codex_project_cli_args(project_path: &str) -> [&str; 2] {
    ["app", project_path]
}

#[cfg(target_os = "macos")]
fn launch_codex_desktop_project(project_path: &str) -> Result<(), String> {
    use std::process::Command;

    let codex_cli = resolve_codex_desktop_cli()?;
    let output = Command::new(&codex_cli)
        .args(codex_project_cli_args(project_path))
        .output()
        .map_err(|error| format!("调用 Codex Desktop CLI 失败: {}", error))?;

    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "Codex Desktop CLI 打开项目失败（{}）：{}",
            codex_cli.display(),
            if stderr.is_empty() {
                format!("退出码 {:?}", output.status.code())
            } else {
                stderr
            }
        ))
    }
}

#[cfg(target_os = "macos")]
fn launch_codex_desktop(project_path: Option<&str>) -> Result<(), String> {
    use std::process::Command;

    if let Some(path) = project_path {
        return launch_codex_desktop_project(path);
    }

    let open_result = Command::new("open")
        .args(codex_app_open_args())
        .output()
        .map_err(|e| format!("启动 Codex App 失败: {}", e))?;

    if open_result.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&open_result.stderr);
        Err(format!("启动 Codex App 失败: {}", stderr))
    }
}

#[cfg(target_os = "macos")]
fn open_new_codex_chat_with_applescript(
    content: &str,
    project_path: Option<&str>,
) -> Result<(), String> {
    launch_codex_desktop(project_path)?;

    let escaped_content = escape_applescript_string(content);
    let script = format!(
        r#"
tell application "Codex" to activate
delay 0.8
tell application "System Events"
    keystroke "n" using command down
    delay 0.8
    set the clipboard to "{escaped_content}"
    keystroke "v" using command down
    delay 0.15
    key code 36
end tell
"#,
    );

    let output = run_applescript(&script)?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("AppleScript 自动发送失败: {}", stderr))
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexNewChatRoute {
    ProjectDeeplinkFirst,
    ApplescriptNewChat,
}

#[cfg(target_os = "macos")]
fn choose_codex_new_chat_route(project_path: Option<&str>) -> CodexNewChatRoute {
    if project_path.is_some() {
        CodexNewChatRoute::ProjectDeeplinkFirst
    } else {
        CodexNewChatRoute::ApplescriptNewChat
    }
}

#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn probe_codex_automation_permission() -> Result<CodexAutomationProbeResult, String> {
    launch_codex_desktop(None)?;

    let script = r#"
tell application "Codex" to activate
delay 0.5
tell application "System Events"
    if exists process "Codex" then
        tell process "Codex"
            count windows
        end tell
    else
        count processes
    end if
end tell
"#;

    let output = run_applescript(script)?;
    if output.status.success() {
        return Ok(CodexAutomationProbeResult {
            status: "granted".to_string(),
            details: "已确认当前这台 Mac 的 Codex / System Events 自动化链路可用。".to_string(),
        });
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let (status, details) = if is_automation_permission_error(&stderr) {
        (
            "permission_required",
            "我已主动触发一次系统自动化权限检查；如果系统弹出允许框，请点允许。若没有弹框或仍失败，请到“系统设置 > 隐私与安全性 > 自动化”检查 iterate 对 Codex / System Events 的授权。".to_string(),
        )
    } else {
        ("error", format!("自动化探测未通过：{}", stderr))
    };

    Ok(CodexAutomationProbeResult {
        status: status.to_string(),
        details,
    })
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn probe_codex_automation_permission() -> Result<CodexAutomationProbeResult, String> {
    Ok(CodexAutomationProbeResult {
        status: "unsupported".to_string(),
        details: "仅 macOS 支持 Codex 自动化权限探测。".to_string(),
    })
}

/// 按项目路径打开 Codex Desktop
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn open_codex_project(project_path: String) -> Result<(), String> {
    let normalized_project_path = project_path.trim();
    if normalized_project_path.is_empty() || normalized_project_path == "main_page" {
        return Err("项目路径无效".to_string());
    }
    if !std::path::Path::new(normalized_project_path).is_absolute() {
        return Err("仅支持绝对路径项目".to_string());
    }

    launch_codex_desktop(Some(normalized_project_path))
}

/// 打开指定 Codex 会话
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn open_codex_thread(thread_id: String) -> Result<(), String> {
    let deeplink =
        codex_thread_deeplink(&thread_id).ok_or_else(|| "Codex 会话 ID 无效".to_string())?;
    launch_codex_desktop_deeplink(&deeplink)
}

/// 非 macOS 平台暂不支持按项目打开 Codex
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn open_codex_project(_project_path: String) -> Result<(), String> {
    Err("仅 macOS 暂支持 Codex Desktop 项目跳转".to_string())
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn open_codex_thread(_thread_id: String) -> Result<(), String> {
    Err("仅 macOS 暂支持 Codex Desktop 会话跳转".to_string())
}

/// 在 Codex 中打开项目或唤起应用。
/// 该链路优先恢复老的 AppleScript 发送体验；若自动化权限缺失，再回退到 deeplink / 打开项目。
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn open_new_codex_chat_with_text(
    content: String,
    project_path: Option<String>,
) -> Result<OpenCodexChatResult, String> {
    let normalized_project_path = project_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty() && *path != "main_page")
        .filter(|path| std::path::Path::new(path).is_absolute());

    if choose_codex_new_chat_route(normalized_project_path)
        == CodexNewChatRoute::ProjectDeeplinkFirst
    {
        if let Some(deeplink) =
            build_codex_new_thread_deeplink(content.as_str(), normalized_project_path)
        {
            return match launch_codex_desktop_deeplink(&deeplink) {
                Ok(()) => {
                    std::thread::sleep(std::time::Duration::from_millis(700));

                    match post_return_keypress_to_codex() {
                        Ok(()) => Ok(OpenCodexChatResult {
                            ok: true,
                            sent: true,
                            mode: "fallback_enter_sent".to_string(),
                            message: format!(
                                "已在当前项目打开 Codex，并自动发送 {}",
                                content.trim()
                            ),
                        }),
                        Err(post_error) => {
                            log::warn!(
                                "Codex 项目级 deeplink 打开后 Return 发送失败: {}",
                                post_error
                            );
                            if is_automation_permission_error(&post_error) {
                                open_accessibility_settings();
                                return Ok(OpenCodexChatResult {
                                    ok: true,
                                    sent: false,
                                    mode: "accessibility_required".to_string(),
                                    message: "已按当前项目打开 Codex，但 macOS 当前拦截了自动按回车。系统已为你打开“隐私与安全性 > 辅助功能”，请给 iterate 授权后再点一次 +。授权完成后，这条 fallback 会自动发送，不再只预填 zhi。".to_string(),
                                });
                            }
                            Ok(OpenCodexChatResult {
                                ok: true,
                                sent: false,
                                mode: "fallback_prefilled".to_string(),
                                message: format!(
                                    "已按当前项目打开 Codex，但本次未自动发送，只预填了 {}",
                                    content.trim()
                                ),
                            })
                        }
                    }
                }
                Err(deeplink_error) => {
                    log::warn!(
                        "Codex 项目级 deeplink 打开失败，回退到项目打开: {}",
                        deeplink_error
                    );
                    launch_codex_desktop(normalized_project_path)
                        .map(|_| OpenCodexChatResult {
                            ok: true,
                            sent: false,
                            mode: "open_only".to_string(),
                            message: "已打开当前项目的 Codex，但未能自动发送或预填 zhi".to_string(),
                        })
                        .map_err(|fallback_error| {
                            format!(
                                "Codex 项目级 deeplink 打开失败: {}；回退到项目打开也失败: {}",
                                deeplink_error, fallback_error
                            )
                        })
                }
            };
        }
    }

    match open_new_codex_chat_with_applescript(content.as_str(), normalized_project_path) {
        Ok(()) => {
            return Ok(OpenCodexChatResult {
                ok: true,
                sent: true,
                mode: "applescript_sent".to_string(),
                message: format!("已打开 Codex 并自动发送 {}", content.trim()),
            });
        }
        Err(apple_script_error) => {
            log::warn!(
                "Codex AppleScript 自动发送失败，回退到 deeplink/项目打开: {}",
                apple_script_error
            );
        }
    }

    if let Some(deeplink) =
        build_codex_new_thread_deeplink(content.as_str(), normalized_project_path)
    {
        return match launch_codex_desktop_deeplink(&deeplink) {
            Ok(()) => {
                std::thread::sleep(std::time::Duration::from_millis(700));

                match post_return_keypress_to_codex() {
                    Ok(()) => Ok(OpenCodexChatResult {
                        ok: true,
                        sent: true,
                        mode: "fallback_enter_sent".to_string(),
                        message: format!(
                            "已打开 Codex，并通过 fallback 自动发送 {}",
                            content.trim()
                        ),
                    }),
                    Err(post_error) => {
                        log::warn!("Codex fallback 回车发送失败: {}", post_error);
                        if is_automation_permission_error(&post_error) {
                            open_accessibility_settings();
                            return Ok(OpenCodexChatResult {
                                ok: true,
                                sent: false,
                                mode: "accessibility_required".to_string(),
                                message: "已打开 Codex，但 macOS 当前拦截了自动按回车。系统已为你打开“隐私与安全性 > 辅助功能”，请给 iterate 授权后再点一次 +。授权完成后，这条 fallback 会自动发送，不再只预填 zhi。".to_string(),
                            });
                        }
                        Ok(OpenCodexChatResult {
                            ok: true,
                            sent: false,
                            mode: "fallback_prefilled".to_string(),
                            message: format!(
                                "已打开 Codex，但本次未自动发送，只预填了 {}",
                                content.trim()
                            ),
                        })
                    }
                }
            }
            Err(deeplink_error) => {
                log::warn!(
                    "Codex deeplink 打开失败，回退到项目打开: {}",
                    deeplink_error
                );
                launch_codex_desktop(normalized_project_path)
                    .map(|_| OpenCodexChatResult {
                        ok: true,
                        sent: false,
                        mode: "open_only".to_string(),
                        message: "已打开 Codex，但未能自动发送或预填 zhi".to_string(),
                    })
                    .map_err(|fallback_error| {
                        format!(
                            "Codex deeplink 打开失败: {}；回退到项目打开也失败: {}",
                            deeplink_error, fallback_error
                        )
                    })
            }
        };
    }

    launch_codex_desktop(normalized_project_path).map(|_| OpenCodexChatResult {
        ok: true,
        sent: false,
        mode: "open_only".to_string(),
        message: "已打开 Codex，但未能自动发送或预填 zhi".to_string(),
    })
}

/// 非 macOS 平台暂不支持 Codex 自动化
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn open_new_codex_chat_with_text(
    _content: String,
    _project_path: Option<String>,
) -> Result<OpenCodexChatResult, String> {
    Err("仅 macOS 暂支持 Codex 自动化".to_string())
}

/// 打开新的 Windsurf 聊天标签页并发送内容
#[tauri::command]
pub async fn open_new_windsurf_chat_with_content(content: String) -> Result<(), String> {
    use std::process::Command;

    // 使用 windsurf chat [prompt] 直接发送内容
    let result = Command::new("windsurf")
        .arg("chat")
        .arg(&content)
        .arg("--reuse-window")
        .spawn();

    match result {
        Ok(_) => Ok(()),
        Err(e) => {
            // 回退到 AppleScript 方案
            let escaped_content = content.replace('\\', "\\\\").replace('"', "\\\"");

            let script = format!(
                r#"
tell application "Windsurf" to activate
delay 0.5
tell application "System Events"
    keystroke "t" using command down
    delay 1.0
    keystroke "l" using command down
    delay 0.5
    keystroke "{}"
    delay 0.3
    keystroke return using command down
end tell
"#,
                escaped_content
            );

            let output = Command::new("osascript")
                .arg("-e")
                .arg(script)
                .output()
                .map_err(|_| format!("执行命令及 AppleScript 均失败: {}", e))?;

            if output.status.success() {
                Ok(())
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                Err(format!("打开新聊天窗口失败: {}", stderr))
            }
        }
    }
}

/// 处理应用退出请求（用于前端退出快捷键）
#[tauri::command]
pub async fn handle_app_exit_request(app: AppHandle) -> Result<bool, String> {
    crate::ui::exit_handler::handle_exit_request_internal(app).await
}

/// 构建发送操作的MCP响应
#[tauri::command]
pub fn build_mcp_send_response(
    user_input: Option<String>,
    selected_options: Vec<String>,
    images: Vec<ImageAttachment>,
    request_id: Option<String>,
    source: String,
) -> Result<String, String> {
    Ok(build_send_response(
        user_input,
        selected_options,
        images,
        request_id,
        &source,
    ))
}

/// 构建继续操作的MCP响应
#[tauri::command]
pub fn build_mcp_continue_response(
    request_id: Option<String>,
    source: String,
) -> Result<String, String> {
    Ok(build_continue_response(request_id, &source))
}

/// 创建测试popup窗口
#[tauri::command]
pub async fn create_test_popup(request: serde_json::Value) -> Result<String, String> {
    // 将JSON值转换为PopupRequest
    let popup_request: PopupRequest =
        serde_json::from_value(request).map_err(|e| format!("解析请求参数失败: {}", e))?;

    // 调用现有的popup创建函数
    match create_tauri_popup(&popup_request) {
        Ok(response) => Ok(response),
        Err(e) => Err(format!("创建测试popup失败: {}", e)),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HuiSuggestionTerm {
    pub key: String,
    pub description: String,
}

fn collect_project_names(project_path: Option<&str>) -> Vec<String> {
    let mut project_names = Vec::new();

    if let Some(project_path) = project_path {
        if let Some(name) = Path::new(project_path)
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.trim().to_lowercase())
        {
            if !name.is_empty() {
                project_names.push(name);
            }
        }
    }

    if project_names.is_empty() {
        if let Ok(current_dir) = std::env::current_dir() {
            if let Some(name) = current_dir
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.trim().to_lowercase())
            {
                if !name.is_empty() {
                    project_names.push(name);
                }
            }
        }
    }

    project_names
}

fn is_current_project_conversation_file(file_name: &str, project_name: &str) -> bool {
    let file_name = file_name.trim().to_lowercase();
    let project_name = project_name.trim().to_lowercase();

    if file_name == format!("{project_name}.md") {
        return true;
    }

    file_name.starts_with(&format!("{project_name}__")) && file_name.ends_with(".md")
}

fn collect_hui_source_paths(project_path: Option<&str>) -> Vec<PathBuf> {
    let Some(home_dir) = dirs::home_dir() else {
        return Vec::new();
    };
    collect_hui_source_paths_from(project_path, &home_dir, chrono::Local::now())
}

fn collect_hui_source_paths_from(
    project_path: Option<&str>,
    home_dir: &Path,
    now: chrono::DateTime<chrono::Local>,
) -> Vec<PathBuf> {
    let mut source_paths = Vec::new();
    let mut seen = HashSet::new();
    let project_root = project_path.map(PathBuf::from);

    if let Some(project_root) = &project_root {
        for relative_path in [".cunzhi-memory/context.md", ".cunzhi-memory/progress.md"] {
            let path = project_root.join(relative_path);
            if path.exists() && seen.insert(path.clone()) {
                source_paths.push(path);
            }
        }
    }

    let project_names = collect_project_names(project_path);
    let conversations_root = home_dir.join(".cunzhi-knowledge/conversations");

    for day_offset in 0..3 {
        let date = now - chrono::Duration::days(day_offset);
        let day_dir = conversations_root.join(date.format("%Y-%m-%d").to_string());
        if !day_dir.exists() {
            continue;
        }

        let mut ranked_files = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&day_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let file_name = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(|name| name.to_lowercase())
                    .unwrap_or_default();
                let score = if project_names.iter().any(|project_name| {
                    is_current_project_conversation_file(&file_name, project_name)
                }) {
                    2
                } else {
                    0
                };

                if score == 0 {
                    continue;
                }

                let modified_at = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                ranked_files.push((score, modified_at, path));
            }
        }

        ranked_files.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.1.cmp(&a.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        for (_, _, path) in ranked_files.into_iter().take(2) {
            if seen.insert(path.clone()) {
                source_paths.push(path);
            }
        }
    }

    source_paths
}

fn normalize_hui_term(raw: &str, stopwords: &HashSet<&'static str>) -> Option<String> {
    let normalized = raw
        .trim()
        .trim_matches(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '`' | '"' | '\'' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';'
                )
        })
        .to_lowercase();

    if normalized.len() < 2 || normalized.len() > 24 {
        return None;
    }

    if stopwords.contains(normalized.as_str()) {
        return None;
    }

    if normalized.starts_with("http")
        || normalized.contains('/')
        || normalized.contains('\\')
        || normalized.contains("__")
    {
        return None;
    }

    if !normalized.chars().any(|ch| ch.is_ascii_alphabetic()) {
        return None;
    }

    if normalized
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, '.' | ':' | '_' | '-'))
    {
        return None;
    }

    Some(normalized)
}

fn canonicalize_hui_term(term: String) -> String {
    match term.as_str() {
        "ji1" | "ji2" | "ji3" => "ji".to_string(),
        "cunzhi" | "cunzhi-knowledge" => "cunzhiknowledge".to_string(),
        _ => term,
    }
}

fn extract_hui_terms(content: &str, weight: usize, counts: &mut HashMap<String, usize>) {
    let stopwords: HashSet<&'static str> = [
        "true",
        "false",
        "null",
        "none",
        "void",
        "return",
        "const",
        "function",
        "async",
        "await",
        "input",
        "output",
        "message",
        "messages",
        "apple",
        "users",
        "macbook-air",
        "debug",
        "popup",
        "textarea",
    ]
    .into_iter()
    .collect();
    let term_regex =
        Regex::new(r"`([^`\n]{2,32})`|\b([A-Za-z][A-Za-z0-9._:-]{1,31})\b").expect("valid regex");

    for captures in term_regex.captures_iter(content) {
        let raw = captures
            .get(1)
            .or_else(|| captures.get(2))
            .map(|capture| capture.as_str())
            .unwrap_or_default();

        if let Some(term) = normalize_hui_term(raw, &stopwords) {
            let term = canonicalize_hui_term(term);
            *counts.entry(term).or_insert(0) += weight;
        }
    }
}

fn seed_hui_terms(project_path: Option<&str>, counts: &mut HashMap<String, usize>) {
    let seeded_terms = [
        ("ji", 96usize),
        ("hui", 72),
        ("cunzhiknowledge", 128),
        ("global_rules.md", 80),
        ("index.md", 64),
        ("context.md", 48),
        ("progress.md", 48),
        ("skills", 60),
        ("prompts", 56),
        ("memories", 56),
        ("localhost", 40),
    ];

    for (term, weight) in seeded_terms {
        *counts.entry(term.to_string()).or_insert(0) += weight;
    }

    let stopwords: HashSet<&'static str> = [
        "true",
        "false",
        "null",
        "none",
        "void",
        "return",
        "const",
        "function",
        "async",
        "await",
        "input",
        "output",
        "message",
        "messages",
        "apple",
        "users",
        "macbook-air",
        "debug",
        "popup",
        "textarea",
    ]
    .into_iter()
    .collect();

    for project_name in collect_project_names(project_path) {
        if let Some(term) = normalize_hui_term(&project_name, &stopwords) {
            let term = canonicalize_hui_term(term);
            *counts.entry(term).or_insert(0) += 32;
        }
    }
}

/// 获取基于 hui 最近活跃链的高频短词，用于输入补全
#[tauri::command]
pub async fn get_hui_suggestion_terms(
    project_path: Option<String>,
) -> Result<Vec<HuiSuggestionTerm>, String> {
    let source_paths = collect_hui_source_paths(project_path.as_deref());
    let mut counts = HashMap::new();

    for (index, path) in source_paths.iter().enumerate() {
        if let Ok(content) = std::fs::read_to_string(path) {
            let weight = source_paths.len().saturating_sub(index).max(1);
            extract_hui_terms(&content, weight, &mut counts);
        }
    }

    seed_hui_terms(project_path.as_deref(), &mut counts);

    let mut ranked_terms: Vec<(String, usize)> = counts.into_iter().collect();
    ranked_terms.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.len().cmp(&b.0.len()))
            .then_with(|| a.0.cmp(&b.0))
    });

    Ok(ranked_terms
        .into_iter()
        .take(12)
        .map(|(key, _)| HuiSuggestionTerm {
            key,
            description: "hui 高频词".to_string(),
        })
        .collect())
}

// 自定义prompt相关命令

/// 获取自定义prompt配置
/// 每次调用都从磁盘重新加载，确保跨进程状态同步
#[tauri::command]
pub async fn get_custom_prompt_config(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<CustomPromptConfig, String> {
    // 强制从磁盘重新加载配置，确保获取最新状态
    if let Err(e) = crate::config::storage::load_config(&state, &app).await {
        log::warn!("[CustomPrompt] 从磁盘重新加载配置失败，使用内存缓存: {}", e);
    }

    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.custom_prompt_config.clone())
}

/// 添加自定义prompt
#[tauri::command]
pub async fn add_custom_prompt(
    prompt: CustomPrompt,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 检查是否超过最大数量限制
        if config.custom_prompt_config.prompts.len()
            >= config.custom_prompt_config.max_prompts as usize
        {
            return Err(format!(
                "自定义prompt数量已达到上限: {}",
                config.custom_prompt_config.max_prompts
            ));
        }

        // 检查ID是否已存在
        if config
            .custom_prompt_config
            .prompts
            .iter()
            .any(|p| p.id == prompt.id)
        {
            return Err("prompt ID已存在".to_string());
        }

        config.custom_prompt_config.prompts.push(prompt);
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

/// 更新自定义prompt
#[tauri::command]
pub async fn update_custom_prompt(
    prompt: CustomPrompt,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 查找并更新prompt
        if let Some(existing_prompt) = config
            .custom_prompt_config
            .prompts
            .iter_mut()
            .find(|p| p.id == prompt.id)
        {
            *existing_prompt = prompt;
        } else {
            return Err("未找到指定的prompt".to_string());
        }
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

/// 删除自定义prompt
#[tauri::command]
pub async fn delete_custom_prompt(
    prompt_id: String,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 查找并删除prompt
        let initial_len = config.custom_prompt_config.prompts.len();
        config
            .custom_prompt_config
            .prompts
            .retain(|p| p.id != prompt_id);

        if config.custom_prompt_config.prompts.len() == initial_len {
            return Err("未找到指定的prompt".to_string());
        }
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

/// 设置自定义prompt启用状态
#[tauri::command]
pub async fn set_custom_prompt_enabled(
    enabled: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.custom_prompt_config.enabled = enabled;
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 广播配置变更，通知所有客户端刷新
    broadcast_custom_prompt_config_changed(&app);

    Ok(())
}

/// 更新自定义prompt排序
#[tauri::command]
pub async fn update_custom_prompt_order(
    prompt_ids: Vec<String>,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    log::debug!("开始更新prompt排序，接收到的IDs: {:?}", prompt_ids);

    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        log::debug!("更新前的prompt顺序:");
        for prompt in &config.custom_prompt_config.prompts {
            log::debug!("  {} (sort_order: {})", prompt.name, prompt.sort_order);
        }

        // 根据新的顺序更新sort_order
        for (index, prompt_id) in prompt_ids.iter().enumerate() {
            if let Some(prompt) = config
                .custom_prompt_config
                .prompts
                .iter_mut()
                .find(|p| p.id == *prompt_id)
            {
                let old_order = prompt.sort_order;
                prompt.sort_order = (index + 1) as i32;
                prompt.updated_at = chrono::Utc::now().to_rfc3339();
                log::debug!(
                    "更新prompt '{}': {} -> {}",
                    prompt.name,
                    old_order,
                    prompt.sort_order
                );
            }
        }

        // 按sort_order排序
        config
            .custom_prompt_config
            .prompts
            .sort_by_key(|p| p.sort_order);

        log::debug!("更新后的prompt顺序:");
        for prompt in &config.custom_prompt_config.prompts {
            log::debug!("  {} (sort_order: {})", prompt.name, prompt.sort_order);
        }
    }

    log::debug!("开始保存配置文件...");
    let save_start = std::time::Instant::now();

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    let save_duration = save_start.elapsed();
    log::debug!("配置保存完成，耗时: {:?}", save_duration);

    Ok(())
}

/// 更新条件性prompt状态
#[tauri::command]
pub async fn update_conditional_prompt_state(
    prompt_id: String,
    new_state: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 查找并更新指定prompt的current_state
        if let Some(prompt) = config
            .custom_prompt_config
            .prompts
            .iter_mut()
            .find(|p| p.id == prompt_id)
        {
            prompt.current_state = new_state;
            prompt.updated_at = chrono::Utc::now().to_rfc3339();
        } else {
            return Err(format!("未找到ID为 {} 的prompt", prompt_id));
        }
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 广播配置变更，通知所有客户端刷新
    broadcast_custom_prompt_config_changed(&app);

    Ok(())
}

/// 更新条件性prompt启用状态
#[tauri::command]
pub async fn update_conditional_prompt_active(
    prompt_id: String,
    is_active: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 查找并更新指定prompt的is_active状态
        if let Some(prompt) = config
            .custom_prompt_config
            .prompts
            .iter_mut()
            .find(|p| p.id == prompt_id)
        {
            prompt.is_active = is_active;
            prompt.updated_at = chrono::Utc::now().to_rfc3339();
        } else {
            return Err(format!("未找到ID为 {} 的prompt", prompt_id));
        }
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 广播配置变更，通知所有客户端刷新
    broadcast_custom_prompt_config_changed(&app);

    Ok(())
}

/// 更新快捷键启用状态
#[tauri::command]
pub async fn set_global_shortcut_enabled(
    enabled: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.shortcut_config.global_enabled = enabled;
    }

    // 更新原子变量
    state
        .global_shortcut_enabled
        .store(enabled, std::sync::atomic::Ordering::Relaxed);

    // 保存配置
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 发送事件通知前端
    app.emit("global-shortcut-state-changed", enabled)
        .map_err(|e| format!("发送事件失败: {}", e))?;

    Ok(())
}

/// 获取快捷键启用状态
#[tauri::command]
pub async fn get_global_shortcut_enabled(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state
        .global_shortcut_enabled
        .load(std::sync::atomic::Ordering::Relaxed))
}

/// 获取配置文件的真实路径
#[tauri::command]
pub async fn get_config_file_path(app: AppHandle) -> Result<String, String> {
    let config_path =
        crate::config::get_config_path(&app).map_err(|e| format!("获取配置文件路径失败: {}", e))?;

    // 获取绝对路径
    let absolute_path = if config_path.is_absolute() {
        config_path
    } else {
        // 如果是相对路径，获取当前工作目录并拼接
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(&config_path)
    };

    // 跨平台路径规范化
    let normalized_path = normalize_path_display(&absolute_path);

    Ok(normalized_path)
}

/// 跨平台路径显示规范化
fn normalize_path_display(path: &std::path::Path) -> String {
    // 如果文件存在，尝试获取规范路径
    let canonical_path = if path.exists() {
        path.canonicalize().ok()
    } else {
        None
    };

    let display_path = canonical_path.as_deref().unwrap_or(path);
    let path_str = display_path.to_string_lossy();

    // 处理不同平台的路径格式
    #[cfg(target_os = "windows")]
    {
        // Windows: 移除长路径前缀 \\?\
        if path_str.starts_with(r"\\?\") {
            path_str[4..].to_string()
        } else {
            path_str.to_string()
        }
    }

    #[cfg(target_os = "macos")]
    {
        // macOS: 处理可能的符号链接和特殊路径
        path_str.to_string()
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: 标准Unix路径处理
        path_str.to_string()
    }

    #[cfg(target_os = "ios")]
    {
        // iOS: 类似macOS的处理
        path_str.to_string()
    }

    #[cfg(target_os = "android")]
    {
        // Android: 类似Linux的处理
        path_str.to_string()
    }

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "linux",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        // 其他平台: 通用处理
        path_str.to_string()
    }
}

// 快捷键相关命令

/// 获取快捷键配置
#[tauri::command]
pub async fn get_shortcut_config(state: State<'_, AppState>) -> Result<ShortcutConfig, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.shortcut_config.clone())
}

/// 更新快捷键绑定
#[tauri::command]
pub async fn update_shortcut_binding(
    shortcut_id: String,
    binding: ShortcutBinding,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        // 更新指定的快捷键绑定
        config
            .shortcut_config
            .shortcuts
            .insert(shortcut_id, binding);
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

/// 重置快捷键为默认值
#[tauri::command]
pub async fn reset_shortcuts_to_default(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.shortcut_config = crate::config::default_shortcut_config();
    }

    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    Ok(())
}

// 窗口注册相关命令

/// 注册当前窗口实例
#[tauri::command]
pub async fn register_window_instance(
    project_path: String,
    request_id: Option<String>,
    title: Option<String>,
) -> Result<(), String> {
    let mut registry = crate::ui::window_registry::WindowRegistry::load();
    registry.register(&project_path, request_id.as_deref(), title.as_deref())
}

#[tauri::command]
pub async fn get_default_window_registration_label() -> Result<String, String> {
    Ok(crate::ui::window_registry::current_window_registration_label())
}

/// 注销当前窗口实例
#[tauri::command]
pub async fn unregister_window_instance() -> Result<(), String> {
    let mut registry = crate::ui::window_registry::WindowRegistry::load();
    registry.unregister()
}

/// 获取所有窗口实例
#[tauri::command]
pub async fn get_all_window_instances(
) -> Result<Vec<crate::ui::window_registry::WindowInstance>, String> {
    let mut registry = crate::ui::window_registry::WindowRegistry::load();
    Ok(registry.get_all_instances())
}

/// 激活指定窗口
#[tauri::command]
pub async fn activate_window_instance(pid: u32) -> Result<(), String> {
    crate::ui::window_registry::activate_window(pid)
}

/// 调试日志 - 输出到终端
#[tauri::command]
pub fn debug_log(message: String) {
    log::info!("[Frontend] {}", message);
}

/// 时间线调试日志 - 直接写独立日志文件
#[tauri::command]
pub fn timeline_debug_log(location: String, payload: Option<serde_json::Value>) {
    let key_params = payload
        .filter(|value| !value.is_null())
        .unwrap_or_else(|| serde_json::json!({}));
    append_timeline_debug_log(&location, key_params);
}

/// 打开系统终端
#[tauri::command]
pub async fn open_terminal(cwd: Option<String>) -> Result<(), String> {
    use std::process::Command;

    #[cfg(target_os = "macos")]
    {
        // macOS: 打开 Terminal.app 并进入指定路径
        let mut cmd = Command::new("open");
        cmd.arg("-a").arg("Terminal");
        if let Some(path) = cwd {
            cmd.arg(path);
        }
        cmd.spawn().map_err(|e| format!("打开终端失败: {}", e))?;
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: 打开 cmd 并进入指定路径
        let mut cmd = Command::new("cmd");
        cmd.args(["/C", "start", "cmd"]);
        if let Some(path) = cwd {
            cmd.current_dir(path);
        }
        cmd.spawn().map_err(|e| format!("打开终端失败: {}", e))?;
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: 尝试打开常见终端
        let terminals = ["gnome-terminal", "konsole", "xterm", "xfce4-terminal"];
        let mut opened = false;
        for terminal in terminals {
            if Command::new(terminal).spawn().is_ok() {
                opened = true;
                break;
            }
        }
        if !opened {
            return Err("无法找到可用的终端程序".to_string());
        }
    }

    Ok(())
}

/// 在 IDE（Windsurf 或 Cursor）中打开项目路径
/// 智能检测：优先使用 Windsurf
#[tauri::command]
pub async fn open_in_ide(project_path: String) -> Result<(), String> {
    use std::process::Command;

    #[cfg(target_os = "macos")]
    {
        // 优先尝试 Windsurf，然后是 Cursor
        let ides = vec![("windsurf", "Windsurf"), ("cursor", "Cursor")];

        let mut opened = false;
        let _last_error = String::new();

        for (cmd, app_name) in ides {
            // 先尝试命令行工具
            if Command::new(cmd).arg(&project_path).spawn().is_ok() {
                opened = true;
                break;
            }
            // 再尝试 open -a
            if Command::new("open")
                .args(["-a", app_name, &project_path])
                .spawn()
                .is_ok()
            {
                opened = true;
                break;
            }
        }

        if !opened {
            return Err("无法找到 Windsurf 或 Cursor IDE".to_string());
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Windows: 先尝试 windsurf，再尝试 cursor
        let result = Command::new("cmd")
            .args(["/C", "windsurf", &project_path])
            .without_console_window()
            .spawn();

        if result.is_err() {
            Command::new("cmd")
                .args(["/C", "cursor", &project_path])
                .without_console_window()
                .spawn()
                .map_err(|e| format!("打开 IDE 失败: {}", e))?;
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Linux: 先尝试 windsurf，再尝试 cursor
        let result = Command::new("windsurf").arg(&project_path).spawn();

        if result.is_err() {
            Command::new("cursor")
                .arg(&project_path)
                .spawn()
                .map_err(|e| format!("打开 IDE 失败: {}", e))?;
        }
    }

    Ok(())
}

/// 在当前活动工作区显示窗口（跟随当前页面）
/// 配合 tauri.conf.json 中的 visibleOnAllWorkspaces: true，窗口会在所有工作区可见
#[tauri::command]
pub async fn center_window(app: AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "未找到主窗口".to_string())?;

    let cursor_position = app
        .cursor_position()
        .map_err(|e| format!("获取鼠标位置失败: {}", e))?;

    let target_monitor = window
        .monitor_from_point(cursor_position.x, cursor_position.y)
        .map_err(|e| format!("获取目标屏幕失败: {}", e))?
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten())
        .ok_or_else(|| "未找到可用屏幕".to_string())?;

    let window_size = window
        .outer_size()
        .map_err(|e| format!("获取窗口尺寸失败: {}", e))?;
    let work_area = target_monitor.work_area();

    let centered_x =
        work_area.position.x + ((work_area.size.width as i32 - window_size.width as i32) / 2);
    let centered_y =
        work_area.position.y + ((work_area.size.height as i32 - window_size.height as i32) / 2);

    window
        .set_position(Position::Physical(PhysicalPosition::new(
            centered_x, centered_y,
        )))
        .map_err(|e| format!("设置窗口位置失败: {}", e))?;
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    window
        .unminimize()
        .map_err(|e| format!("恢复窗口失败: {}", e))?;
    window.show().map_err(|e| format!("显示窗口失败: {}", e))?;
    window
        .set_focus()
        .map_err(|e| format!("聚焦窗口失败: {}", e))?;

    Ok(())
}

#[tauri::command]
pub async fn activate_app_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        #[cfg(target_os = "macos")]
        {
            use objc::runtime::Object;
            use objc::{class, msg_send, sel, sel_impl};

            let (tx, rx) = std::sync::mpsc::channel();
            app.run_on_main_thread(move || {
                let result = (|| -> Result<(), String> {
                    let ns_app: *mut Object =
                        unsafe { msg_send![class!(NSApplication), sharedApplication] };
                    if ns_app.is_null() {
                        return Err("获取 NSApplication 失败".to_string());
                    }

                    unsafe {
                        let _: () = msg_send![ns_app, activateIgnoringOtherApps: true];
                    }

                    Ok(())
                })();

                let _ = tx.send(result);
            })
            .map_err(|e| format!("切回主线程失败: {}", e))?;

            rx.recv()
                .map_err(|e| format!("等待 App 激活结果失败: {}", e))??;
        }

        // 显示并聚焦窗口
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        window
            .unminimize()
            .map_err(|e| format!("恢复窗口失败: {}", e))?;
        window.show().map_err(|e| format!("显示窗口失败: {}", e))?;
        window
            .set_focus()
            .map_err(|e| format!("聚焦窗口失败: {}", e))?;
    }
    Ok(())
}

/// 使用原生 NSOpenPanel 打开路径选择器（macOS），支持文件和文件夹
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn select_files_and_folders(
    app: tauri::AppHandle,
    default_path: Option<String>,
    directories_only: Option<bool>,
) -> Result<Vec<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    let directories_only = directories_only.unwrap_or(false);

    app.run_on_main_thread(move || {
        use objc::msg_send;
        use objc::runtime::Object;
        use objc::{class, sel, sel_impl};

        let panel_class = class!(NSOpenPanel);
        let panel: *mut Object = unsafe { msg_send![panel_class, openPanel] };

        if panel.is_null() {
            let _ = tx.send(Err("Failed to create NSOpenPanel".to_string()));
            return;
        }

        let _: () = unsafe { msg_send![panel, setCanChooseFiles: !directories_only] };
        let _: () = unsafe { msg_send![panel, setCanChooseDirectories: true] };
        let _: () = unsafe { msg_send![panel, setAllowsMultipleSelection: !directories_only] };
        let _: () = unsafe { msg_send![panel, setResolvesAliases: true] };

        if let Some(path) = default_path.clone() {
            // 使用 ok() 避免路径包含 \0 时 panic
            let Some(ns_string) = std::ffi::CString::new(path).ok() else {
                return;
            };
            let ns_path: *mut Object = unsafe {
                let ns_string_class = class!(NSString);
                msg_send![ns_string_class, stringWithUTF8String: ns_string.as_ptr()]
            };
            let url_class = class!(NSURL);
            let ns_url: *mut Object = unsafe { msg_send![url_class, fileURLWithPath: ns_path] };
            let _: () = unsafe { msg_send![panel, setDirectoryURL: ns_url] };
        }

        // 静态字符串不会包含 \0，unwrap 安全
        let title = std::ffi::CString::new(if directories_only {
            "选择授权目录"
        } else {
            "选择文件或文件夹"
        })
        .expect("static string");
        let ns_title: *mut Object = unsafe {
            let ns_string_class = class!(NSString);
            msg_send![ns_string_class, stringWithUTF8String: title.as_ptr()]
        };
        let _: () = unsafe { msg_send![panel, setTitle: ns_title] };

        let response: i64 = unsafe { msg_send![panel, runModal] };

        if response == 1 {
            let urls: *mut Object = unsafe { msg_send![panel, URLs] };
            let count: usize = unsafe { msg_send![urls, count] };

            let mut selected_paths = Vec::new();

            for i in 0..count {
                let url: *mut Object = unsafe { msg_send![urls, objectAtIndex: i] };
                let path: *mut Object = unsafe { msg_send![url, path] };
                let path_str: *const i8 = unsafe { msg_send![path, UTF8String] };

                if !path_str.is_null() {
                    let rust_string = unsafe {
                        std::ffi::CStr::from_ptr(path_str)
                            .to_string_lossy()
                            .into_owned()
                    };
                    selected_paths.push(rust_string);
                }
            }

            let _ = tx.send(Ok(selected_paths));
        } else {
            let _ = tx.send(Ok(vec![]));
        }
    })
    .map_err(|e| e.to_string())?;

    rx.recv().map_err(|e| e.to_string())?
}

/// 非 macOS 平台的文件选择器 - 返回空数组（功能暂不支持）
#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn select_files_and_folders(
    _default_path: Option<String>,
    _directories_only: Option<bool>,
) -> Result<Vec<String>, String> {
    // 非 macOS 平台暂不支持，返回空数组让前端静默处理
    Ok(vec![])
}

/// 读取 macOS 剪贴板里的文件路径（用于支持 Finder 复制文件后直接粘贴为附件）
#[cfg(target_os = "macos")]
#[tauri::command]
pub async fn read_clipboard_file_paths(app: tauri::AppHandle) -> Result<Vec<String>, String> {
    let (tx, rx) = std::sync::mpsc::channel();

    app.run_on_main_thread(move || {
        use objc::msg_send;
        use objc::runtime::Object;
        use objc::{class, sel, sel_impl};

        let result = (|| -> Result<Vec<String>, String> {
            let pasteboard: *mut Object =
                unsafe { msg_send![class!(NSPasteboard), generalPasteboard] };
            if pasteboard.is_null() {
                return Err("获取 NSPasteboard 失败".to_string());
            }

            let url_class = class!(NSURL);
            let classes: *mut Object =
                unsafe { msg_send![class!(NSArray), arrayWithObject: url_class] };
            let options: *mut Object = std::ptr::null_mut();
            let urls: *mut Object =
                unsafe { msg_send![pasteboard, readObjectsForClasses: classes options: options] };

            if urls.is_null() {
                return Ok(vec![]);
            }

            let count: usize = unsafe { msg_send![urls, count] };
            let mut unique_paths = HashSet::new();
            let mut selected_paths = Vec::new();

            for i in 0..count {
                let url: *mut Object = unsafe { msg_send![urls, objectAtIndex: i] };
                if url.is_null() {
                    continue;
                }

                let is_file_url: bool = unsafe { msg_send![url, isFileURL] };
                if !is_file_url {
                    continue;
                }

                let path: *mut Object = unsafe { msg_send![url, path] };
                if path.is_null() {
                    continue;
                }

                let path_str: *const i8 = unsafe { msg_send![path, UTF8String] };
                if path_str.is_null() {
                    continue;
                }

                let rust_string = unsafe {
                    std::ffi::CStr::from_ptr(path_str)
                        .to_string_lossy()
                        .into_owned()
                };

                if !rust_string.is_empty() && unique_paths.insert(rust_string.clone()) {
                    selected_paths.push(rust_string);
                }
            }

            Ok(selected_paths)
        })();

        let _ = tx.send(result);
    })
    .map_err(|e| format!("切回主线程失败: {}", e))?;

    rx.recv()
        .map_err(|e| format!("等待剪贴板读取结果失败: {}", e))?
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub async fn read_clipboard_file_paths() -> Result<Vec<String>, String> {
    Ok(vec![])
}

// ============ 防止睡眠功能 ============

use once_cell::sync::Lazy;
use std::process::{Child, Command};
use std::sync::Mutex;

/// 存储 caffeinate 进程
static CAFFEINATE_PROCESS: Lazy<Mutex<Option<Child>>> = Lazy::new(|| Mutex::new(None));

fn reconcile_prevent_sleep_process(process_guard: &mut Option<Child>) -> bool {
    let Some(child) = process_guard.as_mut() else {
        return false;
    };

    match child.try_wait() {
        Ok(None) => true,
        Ok(Some(status)) => {
            log::warn!("[PreventSleep] caffeinate 已退出: {}", status);
            *process_guard = None;
            false
        }
        Err(error) => {
            log::warn!("[PreventSleep] 无法确认 caffeinate 状态: {}", error);
            let _ = child.kill();
            let _ = child.wait();
            *process_guard = None;
            false
        }
    }
}

/// 在当前进程开启合盖运行。仅供持有 8080 的 bridge 进程调用。
pub(crate) fn enable_prevent_sleep_local() -> Result<bool, String> {
    let mut process_guard = CAFFEINATE_PROCESS.lock().map_err(|e| e.to_string())?;

    // 如果已经在运行，直接返回
    if reconcile_prevent_sleep_process(&mut process_guard) {
        return Ok(true);
    }

    // -s: 接电时阻止系统睡眠；屏幕仍可正常熄灭。
    // -w: iterate 主进程退出时同步释放断言，避免遗留孤儿进程。
    let owner_pid = std::process::id().to_string();
    let child = Command::new("caffeinate")
        .args(["-s", "-w", owner_pid.as_str()])
        .spawn()
        .map_err(|e| format!("启动 caffeinate 失败: {}", e))?;

    *process_guard = Some(child);

    log::info!(
        "[PreventSleep] 已开启合盖运行模式 (owner_pid={})",
        owner_pid
    );
    Ok(true)
}

/// 在当前进程关闭合盖运行。仅供持有 8080 的 bridge 进程调用。
pub(crate) fn disable_prevent_sleep_local() -> Result<bool, String> {
    let mut process_guard = CAFFEINATE_PROCESS.lock().map_err(|e| e.to_string())?;

    if let Some(mut child) = process_guard.take() {
        let _ = child.kill();
        let _ = child.wait();
    }

    log::info!("[PreventSleep] 已关闭合盖运行模式");
    Ok(false)
}

/// 在当前 bridge 进程切换合盖运行。
pub(crate) fn toggle_prevent_sleep_local() -> Result<bool, String> {
    if get_prevent_sleep_status_local() {
        disable_prevent_sleep_local()
    } else {
        enable_prevent_sleep_local()
    }
}

/// 获取当前 bridge 进程持有的合盖运行状态。
pub(crate) fn get_prevent_sleep_status_local() -> bool {
    let Ok(mut process_guard) = CAFFEINATE_PROCESS.lock() else {
        return false;
    };
    reconcile_prevent_sleep_process(&mut process_guard)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreventSleepBridgeResponse {
    enabled: bool,
}

async fn request_prevent_sleep_bridge(action: Option<&str>) -> Result<bool, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .map_err(|error| format!("创建合盖运行请求失败: {}", error))?;
    let url = format!("{}/api/prevent-sleep", bridge_base_url());
    let (method, request) = if let Some(action) = action {
        (
            "POST",
            client
                .post(&url)
                .json(&serde_json::json!({ "action": action })),
        )
    } else {
        ("GET", client.get(&url))
    };
    let request = crate::bridge::auth::authorize_internal_bridge_request(request, method, &url)
        .map_err(|error| format!("合盖运行鉴权失败: {}", error))?;
    let response = request
        .timeout(std::time::Duration::from_secs(2))
        .send()
        .await
        .map_err(|error| format!("连接合盖运行服务失败: {}", error))?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "合盖运行服务拒绝请求: status={} body={}",
            status, body
        ));
    }
    response
        .json::<PreventSleepBridgeResponse>()
        .await
        .map(|payload| payload.enabled)
        .map_err(|error| format!("解析合盖运行状态失败: {}", error))
}

/// 通过常驻 bridge 开启合盖运行，保证所有弹窗和手机共享同一状态。
#[tauri::command]
pub async fn enable_prevent_sleep() -> Result<bool, String> {
    request_prevent_sleep_bridge(Some("enable")).await
}

/// 通过常驻 bridge 关闭合盖运行。
#[tauri::command]
pub async fn disable_prevent_sleep() -> Result<bool, String> {
    request_prevent_sleep_bridge(Some("disable")).await
}

/// 通过常驻 bridge 切换合盖运行。
#[tauri::command]
pub async fn toggle_prevent_sleep() -> Result<bool, String> {
    request_prevent_sleep_bridge(Some("toggle")).await
}

/// 从常驻 bridge 读取全局合盖运行状态。
#[tauri::command]
pub async fn get_prevent_sleep_status() -> Result<bool, String> {
    request_prevent_sleep_bridge(None).await
}

/// 读取本地文件并返回 base64 编码（用于 markdown 图片渲染）
/// 非 GIF 大图片自动通过 sips 压缩到 max 800px，转 JPEG 减小体积
#[tauri::command]
pub async fn read_file_base64(path: String) -> Result<String, String> {
    let normalized_path = normalize_local_file_path(&path);
    let file_path = std::path::Path::new(&normalized_path);
    if !file_path.exists() {
        return Err(format!("文件不存在: {}", normalized_path));
    }

    let metadata = std::fs::metadata(file_path).map_err(|e| format!("读取元数据失败: {}", e))?;
    let original_mime = local_image_mime_type(file_path);

    // 大于 150KB 的非 GIF 图片自动压缩；GIF 必须保留原始 bytes 才能保持动画。
    let (data, mime) = if metadata.len() > 150_000 && original_mime != "image/gif" {
        let temp = std::env::temp_dir().join(format!("iterate_thumb_{}.jpg", std::process::id()));
        // 用 sips 缩到 800px 并转 JPEG
        let _ = std::process::Command::new("sips")
            .args([
                "-Z",
                "800",
                "-s",
                "format",
                "jpeg",
                "-s",
                "formatOptions",
                "60",
            ])
            .arg(&normalized_path)
            .arg("--out")
            .arg(temp.to_string_lossy().to_string())
            .output();

        if temp.exists() {
            let d = std::fs::read(&temp).map_err(|e| format!("读取缩略图失败: {}", e))?;
            let _ = std::fs::remove_file(&temp);
            (d, "image/jpeg")
        } else {
            let d = std::fs::read(file_path).map_err(|e| format!("读取文件失败: {}", e))?;
            (d, original_mime)
        }
    } else {
        let d = std::fs::read(file_path).map_err(|e| format!("读取文件失败: {}", e))?;
        (d, original_mime)
    };

    let b64 = base64_013::encode(&data);
    Ok(format!("data:{};base64,{}", mime, b64))
}

fn local_image_mime_type(file_path: &std::path::Path) -> &'static str {
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("png")
        .to_lowercase();

    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        _ => "image/png",
    }
}

fn normalize_local_file_path(path: &str) -> String {
    let trimmed = path.trim();
    let mut decoded = trimmed
        .strip_prefix("file://")
        .unwrap_or(trimmed)
        .to_string();

    for _ in 0..4 {
        let next = percent_decode_str(&decoded)
            .decode_utf8_lossy()
            .into_owned();
        if next == decoded {
            break;
        }
        decoded = next;
    }

    decoded
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalOpenTarget {
    path: PathBuf,
    line: Option<u32>,
    column: Option<u32>,
}

fn parse_editor_location_number(value: &str) -> Option<u32> {
    if value.is_empty() || !value.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }

    value.parse::<u32>().ok().filter(|number| *number > 0)
}

fn split_editor_location_suffix_if_present(path: &Path) -> LocalOpenTarget {
    if path.exists() {
        return LocalOpenTarget {
            path: path.to_path_buf(),
            line: None,
            column: None,
        };
    }

    let candidate = path.to_string_lossy().to_string();
    let Some((line_candidate, trailing_number)) = candidate.rsplit_once(':') else {
        return LocalOpenTarget {
            path: path.to_path_buf(),
            line: None,
            column: None,
        };
    };
    let Some(trailing_number) = parse_editor_location_number(trailing_number) else {
        return LocalOpenTarget {
            path: path.to_path_buf(),
            line: None,
            column: None,
        };
    };

    let stripped_once = PathBuf::from(line_candidate);
    if stripped_once.exists() {
        return LocalOpenTarget {
            path: stripped_once,
            line: Some(trailing_number),
            column: None,
        };
    }

    let Some((path_candidate, line_number)) = line_candidate.rsplit_once(':') else {
        return LocalOpenTarget {
            path: path.to_path_buf(),
            line: None,
            column: None,
        };
    };
    let Some(line_number) = parse_editor_location_number(line_number) else {
        return LocalOpenTarget {
            path: path.to_path_buf(),
            line: None,
            column: None,
        };
    };

    let stripped_twice = PathBuf::from(path_candidate);
    if stripped_twice.exists() {
        return LocalOpenTarget {
            path: stripped_twice,
            line: Some(line_number),
            column: Some(trailing_number),
        };
    }

    LocalOpenTarget {
        path: path.to_path_buf(),
        line: None,
        column: None,
    }
}

fn has_dangerous_local_open_extension(path: &Path) -> bool {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();

    matches!(
        extension.as_str(),
        "app" | "command" | "terminal" | "exe" | "bat" | "cmd" | "com" | "scr" | "ps1"
    )
}

#[cfg(unix)]
fn is_executable_file(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable_file(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn resolve_local_open_target(path: &str, project_path: &str) -> Result<LocalOpenTarget, String> {
    let normalized_project_path = normalize_local_file_path(project_path);
    if normalized_project_path.is_empty() {
        return Err("缺少项目路径，无法打开本地文件".to_string());
    }

    let project_root = PathBuf::from(&normalized_project_path);
    if !project_root.is_absolute() {
        return Err(format!("项目路径必须是绝对路径: {}", project_path));
    }

    let canonical_project_root = project_root
        .canonicalize()
        .map_err(|e| format!("无法解析项目路径: {}", e))?;

    if !canonical_project_root.is_dir() {
        return Err("项目路径不是目录，无法打开本地文件".to_string());
    }

    let normalized_path = normalize_local_file_path(path);
    if normalized_path.is_empty() {
        return Err("缺少本地文件路径".to_string());
    }

    let candidate = PathBuf::from(&normalized_path);
    let candidate = if candidate.is_absolute() {
        candidate
    } else {
        canonical_project_root.join(candidate)
    };
    let target = split_editor_location_suffix_if_present(&candidate);

    let canonical_path = target
        .path
        .canonicalize()
        .map_err(|e| format!("无法解析本地文件路径: {}", e))?;

    if !canonical_path.starts_with(&canonical_project_root) {
        return Err("仅支持打开当前项目内的文件".to_string());
    }

    let metadata =
        std::fs::metadata(&canonical_path).map_err(|e| format!("读取文件失败: {}", e))?;
    if has_dangerous_local_open_extension(&canonical_path) || is_executable_file(&metadata) {
        return Err("为安全起见，不直接打开可执行文件".to_string());
    }

    Ok(LocalOpenTarget {
        path: canonical_path,
        line: target.line,
        column: target.column,
    })
}

fn resolve_confirmed_external_file_target(path: &str) -> Result<PathBuf, String> {
    let normalized_path = normalize_local_file_path(path);
    if normalized_path.is_empty() {
        return Err("缺少本地文件路径".to_string());
    }

    let candidate = PathBuf::from(&normalized_path);
    if !candidate.is_absolute() {
        return Err("跨项目文件路径必须是绝对路径".to_string());
    }

    let target = split_editor_location_suffix_if_present(&candidate);
    let canonical_path = target
        .path
        .canonicalize()
        .map_err(|e| format!("无法解析本地文件路径: {}", e))?;
    let metadata =
        std::fs::metadata(&canonical_path).map_err(|e| format!("读取文件失败: {}", e))?;

    if !metadata.is_file() {
        return Err("跨项目仅支持在 Finder 中定位普通文件".to_string());
    }

    if has_dangerous_local_open_extension(&canonical_path) || is_executable_file(&metadata) {
        return Err("为安全起见，不直接定位可执行文件".to_string());
    }

    Ok(canonical_path)
}

fn resolve_local_open_path(path: &str, project_path: &str) -> Result<PathBuf, String> {
    resolve_local_open_target(path, project_path).map(|target| target.path)
}

fn editor_location_arg(target: &LocalOpenTarget) -> String {
    let mut value = target.path.to_string_lossy().to_string();
    if let Some(line) = target.line {
        value.push(':');
        value.push_str(&line.to_string());
        if let Some(column) = target.column {
            value.push(':');
            value.push_str(&column.to_string());
        }
    }
    value
}

fn open_local_target_in_editor(target: &LocalOpenTarget) -> Result<(), String> {
    use std::process::Command;

    let location_arg = editor_location_arg(target);
    let ides = ["windsurf", "cursor", "code"];

    for cmd in ides {
        let mut command = Command::new(cmd);
        if target.line.is_some() {
            command.arg("-g");
        }
        if command.arg(&location_arg).spawn().is_ok() {
            return Ok(());
        }
    }

    #[cfg(target_os = "macos")]
    {
        let apps = ["Windsurf", "Cursor", "Visual Studio Code"];
        for app_name in apps {
            if Command::new("open")
                .args(["-a", app_name])
                .arg(&target.path)
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
    }

    Err("无法找到可用 IDE".to_string())
}

#[cfg(test)]
mod local_file_path_tests {
    use super::{
        normalize_local_file_path, read_file_base64, resolve_confirmed_external_file_target,
        resolve_local_open_path, resolve_local_open_target,
    };

    #[test]
    fn normalizes_encoded_local_image_paths() {
        assert_eq!(
            normalize_local_file_path("/Users/test/.cunzhi/images/%E4%B8%AD%E6%96%87.png"),
            "/Users/test/.cunzhi/images/中文.png"
        );
    }

    #[test]
    fn normalizes_double_encoded_local_image_paths() {
        assert_eq!(
            normalize_local_file_path(
                "/Users/test/.cunzhi/images/%25E4%25B8%25AD%25E6%2596%2587%2520%25E5%259B%25BE%25E7%2589%2587.png"
            ),
            "/Users/test/.cunzhi/images/中文 图片.png"
        );
    }

    #[test]
    fn strips_file_scheme_before_normalizing() {
        assert_eq!(
            normalize_local_file_path("file:///Users/test/.cunzhi/images/%E4%B8%AD%E6%96%87.png"),
            "/Users/test/.cunzhi/images/中文.png"
        );
    }

    #[tokio::test]
    async fn preserves_large_gif_mime_for_markdown_images() {
        let file_path =
            std::env::temp_dir().join(format!("iterate-large-gif-{}.gif", uuid::Uuid::new_v4()));
        let mut bytes = b"GIF89a".to_vec();
        bytes.resize(151_000, 0);
        std::fs::write(&file_path, bytes).expect("write test gif");

        let data_url = read_file_base64(file_path.to_string_lossy().to_string())
            .await
            .expect("read gif as data url");

        assert!(data_url.starts_with("data:image/gif;base64,"));
        assert!(!data_url.starts_with("data:image/jpeg;base64,"));

        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn resolves_project_local_absolute_paths() {
        let root =
            std::env::temp_dir().join(format!("iterate-open-local-{}", uuid::Uuid::new_v4()));
        let file_path = root.join("README.md");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(&file_path, "ok").expect("write file");

        let resolved =
            resolve_local_open_path(&file_path.to_string_lossy(), &root.to_string_lossy())
                .expect("resolve path");

        assert_eq!(resolved, file_path.canonicalize().expect("canonical file"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn resolves_project_local_relative_paths() {
        let root =
            std::env::temp_dir().join(format!("iterate-open-local-{}", uuid::Uuid::new_v4()));
        let file_path = root.join("src/main.rs");
        std::fs::create_dir_all(file_path.parent().expect("file parent")).expect("create dir");
        std::fs::write(&file_path, "ok").expect("write file");

        let resolved = resolve_local_open_path("src/main.rs", &root.to_string_lossy())
            .expect("resolve relative path");

        assert_eq!(resolved, file_path.canonicalize().expect("canonical file"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn strips_markdown_line_suffix_before_resolving() {
        let root =
            std::env::temp_dir().join(format!("iterate-open-local-{}", uuid::Uuid::new_v4()));
        let file_path = root.join("src/main.rs");
        std::fs::create_dir_all(file_path.parent().expect("file parent")).expect("create dir");
        std::fs::write(&file_path, "ok").expect("write file");

        let resolved = resolve_local_open_path("src/main.rs:12", &root.to_string_lossy())
            .expect("resolve path with line suffix");

        assert_eq!(resolved, file_path.canonicalize().expect("canonical file"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn preserves_editor_location_suffix_when_resolving_target() {
        let root =
            std::env::temp_dir().join(format!("iterate-open-local-{}", uuid::Uuid::new_v4()));
        let file_path = root.join("src/main.rs");
        std::fs::create_dir_all(file_path.parent().expect("file parent")).expect("create dir");
        std::fs::write(&file_path, "ok").expect("write file");

        let target = resolve_local_open_target("src/main.rs:12:3", &root.to_string_lossy())
            .expect("resolve path with editor location suffix");

        assert_eq!(
            target.path,
            file_path.canonicalize().expect("canonical file")
        );
        assert_eq!(target.line, Some(12));
        assert_eq!(target.column, Some(3));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_paths_outside_project() {
        let root =
            std::env::temp_dir().join(format!("iterate-open-local-{}", uuid::Uuid::new_v4()));
        let outside = std::env::temp_dir().join(format!(
            "iterate-open-local-outside-{}.md",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(&outside, "outside").expect("write outside");

        let error = resolve_local_open_path(&outside.to_string_lossy(), &root.to_string_lossy())
            .expect_err("outside path should be rejected");

        assert!(error.contains("当前项目内"));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn resolves_confirmed_external_regular_file() {
        let file_path = std::env::temp_dir().join(format!(
            "iterate-confirmed-external-{}.md",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&file_path, "outside").expect("write external file");

        let resolved = resolve_confirmed_external_file_target(&file_path.to_string_lossy())
            .expect("external regular file should resolve");

        assert_eq!(resolved, file_path.canonicalize().expect("canonical file"));
        let _ = std::fs::remove_file(&file_path);
    }

    #[test]
    fn rejects_confirmed_external_directory() {
        let directory = std::env::temp_dir().join(format!(
            "iterate-confirmed-external-dir-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).expect("create external directory");

        let error = resolve_confirmed_external_file_target(&directory.to_string_lossy())
            .expect_err("external directory should be rejected");

        assert!(error.contains("普通文件"));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escape() {
        let root =
            std::env::temp_dir().join(format!("iterate-open-local-{}", uuid::Uuid::new_v4()));
        let outside = std::env::temp_dir().join(format!(
            "iterate-open-local-outside-{}.md",
            uuid::Uuid::new_v4()
        ));
        let link_path = root.join("outside.md");
        std::fs::create_dir_all(&root).expect("create root");
        std::fs::write(&outside, "outside").expect("write outside");
        std::os::unix::fs::symlink(&outside, &link_path).expect("create symlink");

        let error = resolve_local_open_path("outside.md", &root.to_string_lossy())
            .expect_err("symlink escape should be rejected");

        assert!(error.contains("当前项目内"));

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }
}

/// 保存提示词库到文件（供 Bridge Server API 读取）
#[tauri::command]
pub fn save_prompt_library_file(content: String) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let dir = std::path::Path::new(&home).join(".cunzhi");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建目录失败: {}", e))?;
    let path = dir.join("prompt-library.json");
    std::fs::write(&path, &content).map_err(|e| format!("写入失败: {}", e))
}

/// 读取提示词库文件（跨进程共享源）
#[tauri::command]
pub fn load_prompt_library_file() -> Result<String, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    let path = std::path::Path::new(&home)
        .join(".cunzhi")
        .join("prompt-library.json");
    if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| format!("读取失败: {}", e))
    } else {
        Ok(r#"{"version":1,"items":[]}"#.to_string())
    }
}

/// 保存幽灵补全词表到共享文件（供 Bridge Server 和 iOS 读取）
#[tauri::command]
pub fn save_ghost_suggestions_file(content: String, app: AppHandle) -> Result<(), String> {
    let store = crate::ghost_suggestions::save_store_from_content(content)?;
    broadcast_ghost_suggestions_changed(&app, store);
    Ok(())
}

/// 读取幽灵补全词表共享文件
#[tauri::command]
pub fn load_ghost_suggestions_file() -> Result<String, String> {
    crate::ghost_suggestions::load_store_content()
}

/// 按当前 schema 追加或更新一个幽灵补全词，避免外部脚本直接改 JSON。
#[tauri::command]
pub fn upsert_ghost_suggestion(
    input: crate::ghost_suggestions::UpsertGhostSuggestionRequest,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let store = crate::ghost_suggestions::upsert_ghost_suggestion(input)?;
    broadcast_ghost_suggestions_changed(&app, store.clone());
    Ok(store)
}

#[tauri::command]
pub fn get_ghost_suggestion_learning_state() -> Result<serde_json::Value, String> {
    let store = crate::ghost_suggestion_learning::load_store()?;
    serde_json::to_value(store).map_err(|error| format!("序列化幽灵补全学习账本失败: {error}"))
}

#[tauri::command]
pub fn record_ghost_suggestion_learning(
    request: crate::ghost_suggestion_learning::RecordGhostSuggestionLearningRequest,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let result = crate::ghost_suggestion_learning::record_learning(request)?;
    if !result.promoted_keys.is_empty() {
        broadcast_ghost_suggestions_changed(&app, result.ghost_suggestions.clone());
    }
    serde_json::to_value(result).map_err(|error| format!("序列化幽灵补全学习结果失败: {error}"))
}

#[tauri::command]
pub fn merge_ghost_suggestion_learning_state(
    state: serde_json::Value,
    app: AppHandle,
) -> Result<serde_json::Value, String> {
    let incoming = serde_json::from_value(state)
        .map_err(|error| format!("解析旧幽灵补全学习缓存失败: {error}"))?;
    let result = crate::ghost_suggestion_learning::merge_legacy_store(incoming)?;
    if !result.promoted_keys.is_empty() {
        broadcast_ghost_suggestions_changed(&app, result.ghost_suggestions.clone());
    }
    serde_json::to_value(result).map_err(|error| format!("序列化幽灵补全学习合并结果失败: {error}"))
}

#[tauri::command]
pub fn get_speech_muscle_memory_entries() -> Result<serde_json::Value, String> {
    let entries = speech_memory::load_entries()?;
    Ok(serde_json::Value::Array(entries))
}

#[tauri::command]
pub fn save_speech_muscle_memory_entries(
    entries: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let array = entries
        .as_array()
        .cloned()
        .ok_or_else(|| "entries 必须是数组".to_string())?;
    let saved = speech_memory::save_entries(array)?;
    Ok(serde_json::Value::Array(saved))
}

#[tauri::command]
pub fn get_speech_correction_memory_entries() -> Result<serde_json::Value, String> {
    let entries = speech_memory::load_correction_entries()?;
    Ok(serde_json::Value::Array(entries))
}

#[tauri::command]
pub fn save_speech_correction_memory_entries(
    entries: serde_json::Value,
) -> Result<serde_json::Value, String> {
    let array = entries
        .as_array()
        .cloned()
        .ok_or_else(|| "entries 必须是数组".to_string())?;
    let saved = speech_memory::save_correction_entries(array)?;
    Ok(serde_json::Value::Array(saved))
}

#[tauri::command]
pub fn record_speech_muscle_memory_hit(
    id: Option<String>,
    spoken_phrase: Option<String>,
) -> Result<serde_json::Value, String> {
    let entries = speech_memory::record_muscle_memory_hit(id, spoken_phrase)?;
    Ok(serde_json::Value::Array(entries))
}

#[tauri::command]
pub fn record_speech_correction_memory_hit(
    id: Option<String>,
    observed_text: Option<String>,
    intended_text: Option<String>,
) -> Result<serde_json::Value, String> {
    let entries = speech_memory::record_correction_memory_hit(id, observed_text, intended_text)?;
    Ok(serde_json::Value::Array(entries))
}

#[tauri::command]
pub fn record_speech_correction_memory_feedback(
    id: Option<String>,
    observed_text: Option<String>,
    intended_text: Option<String>,
    feedback: String,
) -> Result<serde_json::Value, String> {
    let entries = speech_memory::record_correction_memory_feedback(
        id,
        observed_text,
        intended_text,
        feedback,
    )?;
    Ok(serde_json::Value::Array(entries))
}

#[tauri::command]
pub fn get_speech_vocabulary_entries() -> Result<serde_json::Value, String> {
    let store = speech_memory::load_vocabulary_store()?;
    serde_json::to_value(store.entries).map_err(|error| format!("序列化语音词典失败: {error}"))
}

#[tauri::command]
pub fn record_speech_vocabulary_terms(terms: Vec<String>) -> Result<serde_json::Value, String> {
    let store = speech_memory::record_vocabulary_terms(terms)?;
    serde_json::to_value(store.entries).map_err(|error| format!("序列化语音词典失败: {error}"))
}

#[tauri::command]
pub fn merge_speech_vocabulary_terms(terms: Vec<String>) -> Result<serde_json::Value, String> {
    let store = speech_memory::merge_vocabulary_terms(terms)?;
    serde_json::to_value(store.entries).map_err(|error| format!("序列化语音词典失败: {error}"))
}

#[tauri::command]
pub fn append_speech_history_markdown(text: String) -> Result<serde_json::Value, String> {
    let path = speech_memory::append_speech_history_markdown(text)?;
    Ok(serde_json::json!({
        "ok": true,
        "path": path.to_string_lossy(),
    }))
}

/// 列出目录下的 .txt 提示词导出文件
#[tauri::command]
pub fn list_prompt_files(dir_path: String) -> Result<Vec<String>, String> {
    let path = std::path::Path::new(&dir_path);
    if !path.is_dir() {
        return Err(format!("目录不存在: {}", dir_path));
    }

    let mut files = Vec::new();
    let entries = std::fs::read_dir(path).map_err(|e| format!("读取目录失败: {}", e))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("读取目录项失败: {}", e))?;
        let file_path = entry.path();
        if file_path.is_file() {
            if let Some(ext) = file_path.extension() {
                if ext == "txt" {
                    if let Some(s) = file_path.to_str() {
                        files.push(s.to_string());
                    }
                }
            }
        }
    }

    files.sort();
    Ok(files)
}

/// 读取文本文件内容
#[tauri::command]
pub fn read_text_file(file_path: String) -> Result<String, String> {
    std::fs::read_to_string(&file_path).map_err(|e| format!("读取文件失败 {}: {}", file_path, e))
}

/// 截取全屏并返回 base64 图片。
#[tauri::command]
pub async fn capture_screenshot() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;

        let temp_path = "/tmp/iterate_screenshot.png";
        let output = Command::new("screencapture")
            .arg("-x")
            .arg(temp_path)
            .output()
            .map_err(|e| format!("执行 screencapture 失败: {}", e))?;

        if !output.status.success() {
            return Err(format!("screencapture 命令失败: {:?}", output.stderr));
        }

        let data = std::fs::read(temp_path).map_err(|e| format!("读取截图文件失败: {}", e))?;
        let _ = std::fs::remove_file(temp_path);
        return Ok(format!(
            "data:image/png;base64,{}",
            base64_013::encode(&data)
        ));
    }

    #[cfg(target_os = "windows")]
    {
        use image::codecs::png::PngEncoder;
        use image::{ExtendedColorType, ImageEncoder};
        use std::ffi::c_void;
        use windows_sys::Win32::Graphics::Gdi::{
            BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetDC, ReleaseDC,
            SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, DIB_RGB_COLORS,
            SRCCOPY,
        };
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
            SM_YVIRTUALSCREEN,
        };

        let left = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
        let top = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
        let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
        let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
        if width <= 0 || height <= 0 {
            return Err(format!("Windows 虚拟桌面尺寸无效: {}x{}", width, height));
        }

        let screen_dc = unsafe { GetDC(std::ptr::null_mut()) };
        if screen_dc.is_null() {
            return Err("获取 Windows 屏幕 DC 失败".to_string());
        }

        let memory_dc = unsafe { CreateCompatibleDC(screen_dc) };
        if memory_dc.is_null() {
            unsafe {
                ReleaseDC(std::ptr::null_mut(), screen_dc);
            }
            return Err("创建 Windows 截图内存 DC 失败".to_string());
        }

        let mut bitmap_info = BITMAPINFO::default();
        bitmap_info.bmiHeader = BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height,
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB,
            ..Default::default()
        };
        let mut bits: *mut c_void = std::ptr::null_mut();
        let bitmap = unsafe {
            CreateDIBSection(
                screen_dc,
                &bitmap_info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            )
        };
        if bitmap.is_null() || bits.is_null() {
            unsafe {
                DeleteDC(memory_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
            }
            return Err("创建 Windows 截图 DIB 失败".to_string());
        }

        let previous_object = unsafe { SelectObject(memory_dc, bitmap) };
        if previous_object.is_null() {
            unsafe {
                DeleteObject(bitmap);
                DeleteDC(memory_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
            }
            return Err("选择 Windows 截图位图失败".to_string());
        }

        let copied = unsafe {
            BitBlt(
                memory_dc,
                0,
                0,
                width,
                height,
                screen_dc,
                left,
                top,
                SRCCOPY | CAPTUREBLT,
            )
        };
        if copied == 0 {
            unsafe {
                SelectObject(memory_dc, previous_object);
                DeleteObject(bitmap);
                DeleteDC(memory_dc);
                ReleaseDC(std::ptr::null_mut(), screen_dc);
            }
            return Err("Windows BitBlt 截图失败".to_string());
        }

        let byte_len = width as usize * height as usize * 4;
        let bgra = unsafe { std::slice::from_raw_parts(bits as *const u8, byte_len) };
        let mut rgba = vec![0u8; byte_len];
        for (source, target) in bgra.chunks_exact(4).zip(rgba.chunks_exact_mut(4)) {
            target[0] = source[2];
            target[1] = source[1];
            target[2] = source[0];
            target[3] = 255;
        }

        unsafe {
            SelectObject(memory_dc, previous_object);
            DeleteObject(bitmap);
            DeleteDC(memory_dc);
            ReleaseDC(std::ptr::null_mut(), screen_dc);
        }

        let mut png = Vec::new();
        PngEncoder::new(&mut png)
            .write_image(&rgba, width as u32, height as u32, ExtendedColorType::Rgba8)
            .map_err(|e| format!("编码 Windows 截图 PNG 失败: {}", e))?;

        return Ok(format!(
            "data:image/png;base64,{}",
            base64_013::encode(&png)
        ));
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        Err("当前平台暂不支持截图".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        activation_gate_required_for_build, apply_auto_checkpoint_enabled,
        attach_conversation_metadata, build_hui_snapshot, collect_hui_source_paths_from,
        extract_user_response_content, find_hui_latest_anchor,
        is_current_project_conversation_file, is_hui_trigger, lookup_checkpoint_context,
        record_user_response_node, rollback_auto_checkpoint_enabled,
        send_response_to_route_channel, HuiSnapshotQuery, RecordedConversationNode,
    };
    use crate::bridge::ws::MCP_STATE_CACHE;
    use crate::config::AppConfig;
    use crate::conversation::ConversationManager;
    use serde_json::json;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn community_builds_skip_activation_unless_official_build_opts_in() {
        assert!(!activation_gate_required_for_build(true, None, false));
        assert!(!activation_gate_required_for_build(true, Some("0"), false));
        assert!(activation_gate_required_for_build(true, Some("1"), false));
        assert!(activation_gate_required_for_build(
            true,
            Some("TRUE"),
            false
        ));
        assert!(!activation_gate_required_for_build(false, Some("1"), false));
        assert!(!activation_gate_required_for_build(true, Some("1"), true));
    }

    #[test]
    fn auto_checkpoint_enabled_update_can_be_rolled_back_after_failed_save() {
        let mut config = AppConfig::default();
        config.checkpoint_config.auto_checkpoint_enabled = true;

        let previous = apply_auto_checkpoint_enabled(&mut config, false);

        assert!(previous);
        assert!(!config.checkpoint_config.auto_checkpoint_enabled);

        rollback_auto_checkpoint_enabled(&mut config, previous);

        assert!(config.checkpoint_config.auto_checkpoint_enabled);
    }

    #[test]
    fn extract_user_response_content_prefers_explicit_text_fields() {
        let response = json!({
            "message": "  hello world  ",
            "project_path": "/Users/test/project",
        });

        assert_eq!(
            extract_user_response_content(&response),
            Some("hello world".to_string())
        );
    }

    #[test]
    fn extract_user_response_content_keeps_selected_options_fallback() {
        let response = json!({
            "selected_options": ["继续", "忽略"],
        });

        assert_eq!(
            extract_user_response_content(&response),
            Some("选中的选项: 继续 / 忽略".to_string())
        );
    }

    #[test]
    fn extract_user_response_content_keeps_selected_options_before_auto_context() {
        let response = json!({
            "user_input": "✔️不明白的地方反问我，先不着急编码",
            "selected_options": ["先做 T7"],
        });

        assert_eq!(
            extract_user_response_content(&response),
            Some("选中的选项: 先做 T7\n\n✔️不明白的地方反问我，先不着急编码".to_string())
        );
    }

    #[test]
    fn extract_user_response_content_does_not_leak_structured_envelope() {
        let response = json!({
            "project_path": "/Users/test/project",
            "metadata": {
                "source": "tool_call",
            },
        });

        assert_eq!(extract_user_response_content(&response), None);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn codex_new_thread_deeplink_sends_zhi_to_project_path() {
        let deeplink = super::build_codex_new_thread_deeplink("zhi", Some("/Users/test/project"))
            .expect("zhi should create a Codex deeplink");

        assert!(deeplink.starts_with("codex://new?"));
        assert!(deeplink.contains("prompt=zhi"));
        assert!(deeplink.contains("path=%2FUsers%2Ftest%2Fproject"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn codex_new_thread_deeplink_allows_default_window_without_project_path() {
        let deeplink = super::build_codex_new_thread_deeplink("zhi", None)
            .expect("zhi should create a Codex deeplink");

        assert_eq!(deeplink, "codex://new?prompt=zhi");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn codex_deeplink_open_is_pinned_to_the_official_bundle() {
        assert_eq!(
            super::codex_deeplink_open_args("codex://threads/thread-123"),
            ["-b", "com.openai.codex", "codex://threads/thread-123"]
        );
        assert_eq!(super::codex_app_open_args(), ["-b", "com.openai.codex"]);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn codex_project_open_prefers_the_desktop_cli_bundle() {
        let candidates = super::codex_desktop_cli_candidates();

        assert_eq!(
            candidates.first(),
            Some(&std::path::PathBuf::from(
                "/Applications/ChatGPT.app/Contents/Resources/codex"
            ))
        );
        assert!(candidates.contains(&std::path::PathBuf::from(
            "/Applications/Codex.app/Contents/Resources/codex"
        )));
        assert!(candidates.contains(&std::path::PathBuf::from("/opt/homebrew/bin/codex")));
        assert_eq!(
            super::codex_project_cli_args("/Users/test/示例项目/Agents-Anywhere"),
            ["app", "/Users/test/示例项目/Agents-Anywhere"]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn codex_new_chat_route_prefers_project_deeplink_when_project_path_exists() {
        assert_eq!(
            super::choose_codex_new_chat_route(Some("/Users/test/project")),
            super::CodexNewChatRoute::ProjectDeeplinkFirst
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn codex_new_chat_route_uses_applescript_only_without_project_path() {
        assert_eq!(
            super::choose_codex_new_chat_route(None),
            super::CodexNewChatRoute::ApplescriptNewChat
        );
    }

    #[test]
    fn attach_conversation_metadata_writes_tree_node_and_route() {
        let mut response = json!({
            "user_input": "继续",
            "metadata": {
                "request_id": "req-1",
                "source": "popup_submit"
            }
        });

        attach_conversation_metadata(
            &mut response,
            &RecordedConversationNode {
                tree_id: "tree-1".to_string(),
                node_id: "node-1".to_string(),
                parent_id: Some("node-0".to_string()),
                request_key: Some("thread-1".to_string()),
                conversation_route_id: Some("thread-1".to_string()),
                actual_request_id: Some("req-1".to_string()),
            },
        );

        let metadata = response
            .get("metadata")
            .and_then(|value| value.as_object())
            .expect("metadata should be an object");
        assert_eq!(
            metadata
                .get("conversation_id")
                .and_then(|value| value.as_str()),
            Some("tree-1")
        );
        assert_eq!(
            metadata
                .get("current_node_id")
                .and_then(|value| value.as_str()),
            Some("node-1")
        );
        assert_eq!(
            metadata
                .get("timeline_route_id")
                .and_then(|value| value.as_str()),
            Some("thread-1")
        );
        assert_eq!(
            metadata
                .get("actual_request_id")
                .and_then(|value| value.as_str()),
            Some("req-1")
        );
        assert_eq!(
            metadata.get("request_id").and_then(|value| value.as_str()),
            Some("req-1")
        );
    }

    #[test]
    fn hui_trigger_accepts_explicit_depth_modes() {
        assert!(is_hui_trigger("hui"));
        assert!(is_hui_trigger("hui xi"));
        assert!(is_hui_trigger("hui0"));
        assert!(is_hui_trigger("hui1"));
        assert!(is_hui_trigger("hui0➕xi"));
        assert!(is_hui_trigger("hui1 + xi"));
        assert!(is_hui_trigger("回"));
        assert!(!is_hui_trigger("huish"));
        assert!(!is_hui_trigger("hui10"));
    }

    #[test]
    fn current_project_conversation_file_requires_exact_project_prefix() {
        assert!(is_current_project_conversation_file("cunzhi.md", "cunzhi"));
        assert!(is_current_project_conversation_file(
            "cunzhi__macbook-air.md",
            "cunzhi"
        ));
        assert!(!is_current_project_conversation_file(
            "cat-haven__macbook-air.md",
            "cunzhi"
        ));
        assert!(!is_current_project_conversation_file(
            "软件工程__macbook-air.md",
            "cunzhi"
        ));
    }

    #[test]
    fn collect_hui_source_paths_stays_on_current_project_only() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("hui-source-paths-{unique}"));
        let workspace = root.join("cunzhi");
        let home_dir = root.join("home");
        let conversations_dir = home_dir.join(".cunzhi-knowledge/conversations");
        let day_dir = conversations_dir.join(chrono::Local::now().format("%Y-%m-%d").to_string());

        fs::create_dir_all(&day_dir).expect("should create conversation dir");
        fs::create_dir_all(workspace.join(".cunzhi-memory")).expect("should create workspace dir");

        fs::write(day_dir.join("cunzhi__MacBook-Air.md"), "current project")
            .expect("write current project file");
        fs::write(day_dir.join("cunzhi.md"), "current project plain")
            .expect("write current project plain file");
        fs::write(day_dir.join("cat-haven__MacBook-Air.md"), "other project")
            .expect("write other project file");
        fs::write(day_dir.join("软件工程__MacBook-Air.md"), "other project 2")
            .expect("write other project file");

        let source_paths = collect_hui_source_paths_from(
            Some(
                workspace
                    .to_str()
                    .expect("workspace path should be valid utf-8"),
            ),
            &home_dir,
            chrono::Local::now(),
        );

        assert_eq!(source_paths.len(), 2);
        assert!(source_paths.iter().all(|path| {
            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default()
                .to_lowercase();
            file_name.starts_with("cunzhi")
        }));
        assert!(source_paths
            .iter()
            .all(|path| !path.to_string_lossy().contains("cat-haven")));
        assert!(source_paths
            .iter()
            .all(|path| !path.to_string_lossy().contains("软件工程")));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hui_latest_anchor_rejects_route_match_when_run_scope_misses() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("hui-run-scope-{unique}"));
        let workspace = root.join("cunzhi");
        let conversations_root = root.join(".cunzhi-knowledge/conversations");
        let day_dir = conversations_root.join(chrono::Local::now().format("%Y-%m-%d").to_string());
        fs::create_dir_all(&day_dir).expect("should create conversation dir");
        fs::create_dir_all(&workspace).expect("should create workspace dir");

        fs::write(
            day_dir.join("cunzhi__MacBook-Air.md"),
            r#"## 10:00:00  @ cunzhi
<!-- cunzhi-meta: {"schema":"cunzhi.conversation.v1","conversation_id":"tree-current","current_node_id":"node-current","request_id":"req-current","timeline_route_id":"thread-current","run_id":"run-current","generation":200,"project_path":"/tmp/cunzhi"} -->

### 🤖 AI
current route, old run

### 👤 用户
当前 route 的旧内容
"#,
        )
        .expect("should write conversation file");

        let anchor = find_hui_latest_anchor(&HuiSnapshotQuery {
            project_path: workspace.to_str(),
            request_id: Some("thread-current"),
            run_id: Some("run-new"),
            generation: Some(300),
            conversations_root: Some(&conversations_root),
        });

        assert!(
            anchor.is_none(),
            "new GoalRun run should not prefill hui snapshot from same route old run"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hui_latest_anchor_prefers_current_run_meta_over_newer_stale_block() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("hui-meta-anchor-{unique}"));
        let workspace = root.join("cunzhi");
        let conversations_root = root.join(".cunzhi-knowledge/conversations");
        let day_dir = conversations_root.join(chrono::Local::now().format("%Y-%m-%d").to_string());
        fs::create_dir_all(&day_dir).expect("should create conversation dir");
        fs::create_dir_all(&workspace).expect("should create workspace dir");

        fs::write(
            day_dir.join("cunzhi__MacBook-Air.md"),
            r#"## 10:00:00  @ cunzhi
<!-- cunzhi-meta: {"schema":"cunzhi.conversation.v1","conversation_id":"tree-current","current_node_id":"node-current","request_id":"req-current","timeline_route_id":"thread-current","run_id":"run-current","generation":200,"project_path":"/tmp/cunzhi"} -->

### 🤖 AI
current run

### 👤 用户
继续当前 run

---
## 10:05:00  @ cunzhi
<!-- cunzhi-meta: {"schema":"cunzhi.conversation.v1","conversation_id":"tree-old","current_node_id":"node-old","request_id":"req-old","timeline_route_id":"thread-old","run_id":"run-old","generation":100,"stale_of":"run-old","superseded_by":"run-current","project_path":"/tmp/cunzhi"} -->

### 🤖 AI
old run

### 👤 用户
旧 run 回流
"#,
        )
        .expect("should write conversation file");

        let anchor = find_hui_latest_anchor(&HuiSnapshotQuery {
            project_path: workspace.to_str(),
            request_id: Some("thread-current"),
            run_id: Some("run-current"),
            generation: Some(200),
            conversations_root: Some(&conversations_root),
        })
        .expect("current run anchor should be found");

        assert_eq!(anchor.time, "10:00:00");
        assert_eq!(anchor.conversation_id.as_deref(), Some("tree-current"));
        assert_eq!(anchor.current_node_id.as_deref(), Some("node-current"));
        assert_eq!(anchor.timeline_route_id.as_deref(), Some("thread-current"));
        assert_eq!(anchor.run_id.as_deref(), Some("run-current"));
        assert_eq!(anchor.generation, Some(200));
        assert_eq!(anchor.stale_of, None);

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn hui_snapshot_keeps_current_run_after_late_stale_goal_response() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should be after epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("hui-live-overlap-{unique}"));
        let conversations_root = root.join(".cunzhi-knowledge/conversations");
        let day_dir = conversations_root.join(chrono::Local::now().format("%Y-%m-%d").to_string());
        let workspace = root.join("cunzhi");
        fs::create_dir_all(&day_dir).expect("should create conversation dir");
        fs::create_dir_all(&workspace).expect("should create workspace dir");
        let project_path = workspace.to_string_lossy().to_string();
        let manager = ConversationManager::new();

        let current_response = json!({
            "user_input": "当前 run 回包",
            "metadata": {
                "request_id": "req-current",
                "timeline_route_id": "thread-current",
                "conversation_route_id": "thread-current",
                "source": "popup_goal_submit",
                "run_id": "run-current",
                "generation": 200
            }
        });
        let current_record = record_user_response_node(
            None,
            &manager,
            &current_response,
            Some(project_path.clone()),
            Some("req-current".to_string()),
            Some("thread-current".to_string()),
            "test_current_goal_response",
        )
        .await
        .expect("current response should record")
        .expect("current response should create a node");

        let stale_response = json!({
            "user_input": "旧 run 晚返回",
            "metadata": {
                "request_id": "req-old",
                "timeline_route_id": "thread-old",
                "conversation_route_id": "thread-old",
                "source": "popup_goal_submit",
                "run_id": "run-old",
                "generation": 100,
                "stale_of": "run-old",
                "superseded_by": "run-current"
            }
        });
        let stale_record = record_user_response_node(
            None,
            &manager,
            &stale_response,
            Some(project_path.clone()),
            Some("req-old".to_string()),
            Some("thread-old".to_string()),
            "test_late_stale_goal_response",
        )
        .await
        .expect("stale response should record")
        .expect("stale response should create a node");

        let current_meta = json!({
            "schema": "cunzhi.conversation.v1",
            "conversation_id": current_record.tree_id,
            "current_node_id": current_record.node_id,
            "request_id": "req-current",
            "timeline_route_id": "thread-current",
            "run_id": "run-current",
            "generation": 200,
            "project_path": project_path.clone(),
        });
        let stale_meta = json!({
            "schema": "cunzhi.conversation.v1",
            "conversation_id": stale_record.tree_id,
            "current_node_id": stale_record.node_id,
            "request_id": "req-old",
            "timeline_route_id": "thread-old",
            "run_id": "run-old",
            "generation": 100,
            "stale_of": "run-old",
            "superseded_by": "run-current",
            "project_path": project_path.clone(),
        });
        fs::write(
            day_dir.join("cunzhi__MacBook-Air.md"),
            format!(
                r#"## 10:00:00  @ cunzhi
<!-- cunzhi-meta: {} -->

### 🤖 AI
current run

### 👤 用户
当前 run 回包

---
## 10:05:00  @ cunzhi
<!-- cunzhi-meta: {} -->

### 🤖 AI
old run

### 👤 用户
旧 run 晚返回
"#,
                current_meta, stale_meta
            ),
        )
        .expect("should write conversation file");

        let snapshot = build_hui_snapshot(
            HuiSnapshotQuery {
                project_path: Some(&project_path),
                request_id: Some("thread-current"),
                run_id: Some("run-current"),
                generation: Some(200),
                conversations_root: Some(&conversations_root),
            },
            &manager,
        )
        .await
        .expect("hui snapshot should select an anchor");

        assert!(snapshot.contains("最新用户输入：当前 run 回包"));
        assert!(snapshot.contains("meta.route：`thread-current`"));
        assert!(snapshot.contains("meta.run_id：`run-current`"));
        assert!(snapshot.contains("meta.generation：`200`"));
        assert!(snapshot.contains("meta.stale_of：`无`"));
        assert!(!snapshot.contains("最新用户输入：旧 run 晚返回"));

        let _ = fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn lookup_checkpoint_context_reads_request_metadata_from_cache() {
        let request_id = "req-checkpoint-test";
        {
            let mut cache = MCP_STATE_CACHE.write().await;
            cache.insert(
                request_id.to_string(),
                json!({
                    "request": {
                        "id": request_id,
                        "project_path": "/tmp/demo",
                        "checkpoint_id": "cp_demo_123",
                        "checkpoint_commit": "abc123def",
                        "checkpoint_message": "iterate-checkpoint:2099-01-01T00:00:00Z | 自动检查点 08:00:00"
                    }
                }),
            );
        }

        let context = lookup_checkpoint_context(Some(request_id), Some("/tmp/demo"))
            .await
            .expect("checkpoint context should exist");

        assert_eq!(context.checkpoint_id.as_deref(), Some("cp_demo_123"));
        assert_eq!(context.checkpoint_commit.as_deref(), Some("abc123def"));
        assert!(context
            .checkpoint_message
            .as_deref()
            .unwrap_or_default()
            .contains("iterate-checkpoint:"));

        let mut cache = MCP_STATE_CACHE.write().await;
        cache.remove(request_id);
    }

    #[tokio::test]
    async fn route_channel_send_success_delivers_response() {
        let (sender, receiver) = tokio::sync::oneshot::channel();

        send_response_to_route_channel(sender, "ok".to_string(), "serve-1")
            .expect("send should succeed while receiver is alive");

        assert_eq!(receiver.await.expect("receiver should get response"), "ok");
    }

    #[test]
    fn route_channel_send_failure_returns_error_without_ack() {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        drop(receiver);

        let err = send_response_to_route_channel(sender, "ok".to_string(), "serve-1")
            .expect_err("send should fail after receiver is dropped");

        assert!(err.contains("发送响应到 serve-1 失败"));
    }
}
