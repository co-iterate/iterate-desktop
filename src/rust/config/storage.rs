use anyhow::Result;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use tauri::{AppHandle, LogicalSize, Manager, State};
use tempfile::NamedTempFile;

use super::cunzhi_config_dir;
use super::settings::{default_shortcuts, AppConfig, AppState};

fn atomic_write_config(path: &std::path::Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("配置路径缺少父目录"))?;
    fs::create_dir_all(parent)?;

    let mut temp = NamedTempFile::new_in(parent)?;
    temp.write_all(bytes)?;
    temp.flush()?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| anyhow::anyhow!("原子替换配置失败: {}", error.error))?;

    Ok(())
}

pub fn get_config_path(_app: &AppHandle) -> Result<PathBuf> {
    // 使用与独立配置相同的路径，确保一致性
    get_standalone_config_path()
}

/// Save a popup preference against the latest file, not a window's old snapshot.
/// A single switch click may enable the item and change its state together.
pub fn patch_conditional_prompt_settings(
    path: &std::path::Path,
    prompt_id: &str,
    current_state: Option<bool>,
    is_active: Option<bool>,
) -> Result<AppConfig> {
    anyhow::ensure!(
        current_state.is_some() || is_active.is_some(),
        "未提供上下文追加设置"
    );
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("配置路径缺少父目录"))?;
    fs::create_dir_all(parent)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("context-prompts.lock"))?;
    lock.lock()?;
    let mut value: serde_json::Value = if path.exists() {
        serde_json::from_slice(&fs::read(path)?)?
    } else {
        serde_json::to_value(AppConfig::default())?
    };
    let prompt = value
        .get_mut("custom_prompt_config")
        .and_then(|config| config.get_mut("prompts"))
        .and_then(serde_json::Value::as_array_mut)
        .and_then(|prompts| prompts.iter_mut().find(|prompt| prompt["id"] == prompt_id))
        .ok_or_else(|| anyhow::anyhow!("未找到ID为 {} 的prompt", prompt_id))?;
    anyhow::ensure!(
        prompt["type"] == "conditional",
        "该prompt不是上下文追加选项"
    );
    if let Some(value) = current_state {
        prompt["current_state"] = value.into();
    }
    if let Some(value) = is_active {
        prompt["is_active"] = value.into();
    }
    prompt["updated_at"] = chrono::Utc::now().to_rfc3339().into();
    // Validate before writing; preserve unrelated and unknown JSON fields.
    let config: AppConfig = serde_json::from_value(value.clone())?;
    atomic_write_config(path, &serde_json::to_vec_pretty(&value)?)?;
    Ok(config)
}

pub async fn save_config(state: &State<'_, AppState>, app: &AppHandle) -> Result<()> {
    let config_path = get_config_path(app)?;

    // 确保目录存在
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // The CLI owns formal-route registration. Ordinary GUI settings must not
    // restore an older route from this window's cached AppState.
    let _route_guard =
        crate::tunnel::formal_transport::try_lock_formal_route().map_err(anyhow::Error::msg)?;
    let latest_route = load_standalone_config()?.mobile_config.formal_route;
    let mut config = state
        .config
        .lock()
        .map_err(|e| anyhow::anyhow!("获取配置失败: {}", e))?;
    config.mobile_config.formal_route = latest_route;
    let config_json = serde_json::to_string_pretty(&*config)?;

    atomic_write_config(&config_path, config_json.as_bytes())?;

    log::debug!("配置已保存到: {:?}", config_path);

    Ok(())
}

/// Tauri应用专用的配置加载函数
pub async fn load_config(state: &State<'_, AppState>, app: &AppHandle) -> Result<()> {
    let config_path = get_config_path(app)?;

    if config_path.exists() {
        let config_json = fs::read_to_string(&config_path)?;
        let mut config: AppConfig = serde_json::from_str(&config_json)?;

        // 合并默认快捷键配置，确保新的默认快捷键被添加
        merge_default_shortcuts(&mut config);

        // 同步快捷键全局启用状态
        state
            .global_shortcut_enabled
            .store(config.shortcut_config.global_enabled, Ordering::Relaxed);

        let mut config_guard = state
            .config
            .lock()
            .map_err(|e| anyhow::anyhow!("获取配置锁失败: {}", e))?;
        *config_guard = config;
    }

    Ok(())
}

pub async fn load_config_and_apply_window_settings(
    state: &State<'_, AppState>,
    app: &AppHandle,
) -> Result<()> {
    // 先加载配置
    load_config(state, app).await?;

    // 然后应用窗口设置
    let (always_on_top, window_config) = {
        let config = state
            .config
            .lock()
            .map_err(|e| anyhow::anyhow!("获取配置失败: {}", e))?;
        (
            config.ui_config.always_on_top,
            config.ui_config.window_config.clone(),
        )
    };

    // 应用到窗口
    if let Some(window) = app.get_webview_window("main") {
        // 应用置顶设置
        #[cfg(not(any(target_os = "android", target_os = "ios")))]
        {
            if let Err(e) = window.set_always_on_top(always_on_top) {
                log::warn!("设置窗口置顶失败: {}", e);
            } else {
                log::info!("窗口置顶状态已设置为: {} (配置加载时)", always_on_top);
            }
        }

        // 应用窗口大小约束
        if let Err(e) = window.set_min_size(Some(LogicalSize::new(
            window_config.min_width,
            window_config.min_height,
        ))) {
            log::warn!("设置最小窗口大小失败: {}", e);
        }

        if let Err(e) = window.set_max_size(Some(LogicalSize::new(
            window_config.max_width,
            window_config.max_height,
        ))) {
            log::warn!("设置最大窗口大小失败: {}", e);
        }

        // 根据当前模式设置窗口大小
        let (target_width, target_height) = if window_config.fixed {
            // 固定模式：使用固定尺寸
            (window_config.fixed_width, window_config.fixed_height)
        } else {
            // 自由拉伸模式：使用自由拉伸尺寸
            (window_config.free_width, window_config.free_height)
        };

        // 应用窗口大小（移除调试信息）
        if let Err(_e) = window.set_size(LogicalSize::new(target_width, target_height)) {
            // 静默处理窗口大小设置失败
        }
    }

    Ok(())
}

/// 独立加载配置文件（用于MCP服务器等独立进程）
pub fn load_standalone_config() -> Result<AppConfig> {
    let config_path = get_standalone_config_path()?;

    if config_path.exists() {
        let config_json = fs::read_to_string(config_path)?;
        let mut config: AppConfig = serde_json::from_str(&config_json)?;

        // 合并默认快捷键配置
        merge_default_shortcuts(&mut config);

        Ok(config)
    } else {
        // 如果配置文件不存在，返回默认配置
        Ok(AppConfig::default())
    }
}

/// 独立保存配置文件（用于 bridge-only / MCP 等无 Tauri AppState 的进程）
pub fn save_standalone_config(config: &AppConfig) -> Result<()> {
    let config_path = get_standalone_config_path()?;
    let config_json = serde_json::to_string_pretty(config)?;

    atomic_write_config(&config_path, config_json.as_bytes())?;

    Ok(())
}

/// 独立加载Telegram配置（用于MCP模式下的配置检查）
pub fn load_standalone_telegram_config() -> Result<super::settings::TelegramConfig> {
    let config = load_standalone_config()?;
    Ok(config.telegram_config)
}

/// 获取独立配置文件路径（不依赖Tauri）
fn get_standalone_config_path() -> Result<PathBuf> {
    Ok(cunzhi_config_dir()?.join("config.json"))
}

fn matches_legacy_popup_binding(
    binding: &super::settings::ShortcutBinding,
    id: &str,
    name: &str,
    description: &str,
    action: &str,
    ctrl: bool,
    alt: bool,
    shift: bool,
    meta: bool,
) -> bool {
    let key = &binding.key_combination;
    binding.id == id
        && binding.name == name
        && binding.description == description
        && binding.action == action
        && binding.enabled
        && binding.scope == "popup"
        && key.key == "Enter"
        && key.ctrl == ctrl
        && key.alt == alt
        && key.shift == shift
        && key.meta == meta
}

fn has_complete_legacy_popup_defaults(config: &AppConfig) -> bool {
    if cfg!(target_os = "macos") {
        return false;
    }

    let shortcuts = &config.shortcut_config.shortcuts;
    shortcuts.get("quick_submit").is_some_and(|binding| {
        matches_legacy_popup_binding(
            binding,
            "quick_submit",
            "快速发送",
            "快速提交当前输入内容",
            "submit",
            false,
            false,
            false,
            true,
        )
    }) && shortcuts.get("continue").is_some_and(|binding| {
        matches_legacy_popup_binding(
            binding,
            "continue",
            "继续",
            "继续对话",
            "continue",
            false,
            false,
            true,
            false,
        )
    }) && shortcuts.get("enhance").is_some_and(|binding| {
        matches_legacy_popup_binding(
            binding,
            "enhance",
            "增强",
            "增强当前输入内容",
            "enhance",
            false,
            true,
            false,
            false,
        )
    })
}

/// 合并默认快捷键配置，确保新的默认快捷键被添加到现有配置中
fn merge_default_shortcuts(config: &mut AppConfig) {
    let default_shortcuts = default_shortcuts();

    // Windows/Linux 历史版本会同时写入 Meta+Enter、Shift+Enter、Alt+Enter。
    // 只有三项的全部字段都仍是这套完整旧默认值时才迁移；任意一项被用户
    // 编辑、禁用或改名，都视为自定义配置并完整保留。
    if has_complete_legacy_popup_defaults(config) {
        for key in ["quick_submit", "continue", "enhance"] {
            if let Some(default_binding) = default_shortcuts.get(key) {
                config
                    .shortcut_config
                    .shortcuts
                    .insert(key.to_string(), default_binding.clone());
            }
        }
    }

    // macOS 保留回流前的逐键迁移（旧增强/继续默认值 → 当前默认值），行为不变。
    #[cfg(target_os = "macos")]
    migrate_macos_legacy_popup_bindings(config, &default_shortcuts);

    // 只补充新版本新增但用户配置中尚不存在的快捷键。
    // 除上面的完整历史默认组合外，不按单个按键值猜测或覆盖已有绑定。
    for (key, default_binding) in default_shortcuts {
        if !config.shortcut_config.shortcuts.contains_key(&key) {
            config
                .shortcut_config
                .shortcuts
                .insert(key, default_binding);
        }
    }
}

#[cfg(target_os = "macos")]
fn migrate_macos_legacy_popup_bindings(
    config: &mut AppConfig,
    default_shortcuts: &std::collections::HashMap<String, super::settings::ShortcutBinding>,
) {
    for key in ["enhance", "continue"] {
        let (Some(existing), Some(default_binding)) = (
            config.shortcut_config.shortcuts.get(key),
            default_shortcuts.get(key),
        ) else {
            continue;
        };
        let kc = &existing.key_combination;
        let enter_only = |ctrl: bool, shift: bool, alt: bool| {
            kc.key == "Enter" && kc.ctrl == ctrl && kc.shift == shift && kc.alt == alt && !kc.meta
        };
        let legacy = match key {
            // 旧增强默认值：Ctrl+Shift+Enter、Ctrl+Enter、Shift+Enter
            "enhance" => {
                enter_only(true, true, false)
                    || enter_only(true, false, false)
                    || enter_only(false, true, false)
            }
            // 旧继续默认值：Alt+Enter
            _ => enter_only(false, false, true),
        };
        if legacy {
            config
                .shortcut_config
                .shortcuts
                .insert(key.to_string(), default_binding.clone());
        }
    }
}

#[cfg(test)]
mod context_prompt_tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let mut value = serde_json::to_value(AppConfig::default()).unwrap();
        value["custom_prompt_config"]["prompts"] = json!([
            {"id":"a", "name":"A", "content":"", "sort_order":0,
             "created_at":"old", "updated_at":"old", "type":"conditional",
             "current_state":false, "is_active":false},
            {"id":"b", "name":"B", "content":"", "sort_order":1,
             "created_at":"old", "updated_at":"old", "type":"conditional",
             "current_state":false, "is_active":false}
        ]);
        value["unrelated_future_setting"] = json!({"keep":true});
        fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        (dir, path)
    }

    #[test]
    fn fresh_reads_keep_enabled_and_disabled_preferences_and_other_settings() {
        let (_dir, path) = fixture();
        let mut expected: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        patch_conditional_prompt_settings(&path, "a", Some(true), Some(true)).unwrap();
        let fresh: AppConfig = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(fresh.custom_prompt_config.prompts[0].current_state);
        assert!(fresh.custom_prompt_config.prompts[0].is_active);
        patch_conditional_prompt_settings(&path, "a", None, Some(false)).unwrap();
        let mut actual: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        expected["custom_prompt_config"]["prompts"][0]["current_state"] = true.into();
        actual["custom_prompt_config"]["prompts"][0]["updated_at"] = "old".into();
        assert_eq!(actual, expected);
    }

    #[test]
    fn independent_window_writers_do_not_drop_each_others_changes() {
        let (_dir, path) = fixture();
        std::thread::scope(|scope| {
            for id in ["a", "b"] {
                let path = &path;
                scope.spawn(move || {
                    for _ in 0..20 {
                        patch_conditional_prompt_settings(path, id, Some(true), Some(true))
                            .unwrap();
                    }
                });
            }
        });
        let fresh: AppConfig = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert!(fresh
            .custom_prompt_config
            .prompts
            .iter()
            .all(|p| p.current_state && p.is_active));
    }

    #[test]
    fn invalid_preference_does_not_change_the_file() {
        let (_dir, path) = fixture();
        let before = fs::read(&path).unwrap();
        assert!(patch_conditional_prompt_settings(&path, "missing", Some(true), None).is_err());
        assert!(patch_conditional_prompt_settings(&path, "a", None, None).is_err());
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}

#[cfg(test)]
mod shortcut_tests {
    use super::merge_default_shortcuts;
    use crate::config::AppConfig;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_keeps_per_key_legacy_migration() {
        let mut config = AppConfig::default();
        let defaults = config.shortcut_config.shortcuts.clone();
        let continue_key = config
            .shortcut_config
            .shortcuts
            .get_mut("continue")
            .unwrap();
        continue_key.key_combination.shift = false;
        continue_key.key_combination.alt = true;
        let enhance = config.shortcut_config.shortcuts.get_mut("enhance").unwrap();
        enhance.key_combination.alt = false;
        enhance.key_combination.ctrl = true;
        enhance.key_combination.shift = true;
        merge_default_shortcuts(&mut config);
        for key in ["continue", "enhance"] {
            let actual = &config.shortcut_config.shortcuts[key].key_combination;
            let expected = &defaults[key].key_combination;
            assert_eq!(
                (actual.ctrl, actual.alt, actual.shift, actual.meta),
                (expected.ctrl, expected.alt, expected.shift, expected.meta),
                "{key}"
            );
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn merge_migrates_only_the_complete_legacy_popup_default_set() {
        let mut config = AppConfig::default();
        let quick_submit = config
            .shortcut_config
            .shortcuts
            .get_mut("quick_submit")
            .unwrap();
        quick_submit.key_combination.shift = false;
        quick_submit.key_combination.meta = true;

        let continue_key = config
            .shortcut_config
            .shortcuts
            .get_mut("continue")
            .unwrap();
        continue_key.key_combination.ctrl = false;
        continue_key.key_combination.shift = true;

        let enhance = config.shortcut_config.shortcuts.get_mut("enhance").unwrap();
        enhance.key_combination.ctrl = false;
        enhance.key_combination.alt = true;
        enhance.key_combination.shift = false;

        merge_default_shortcuts(&mut config);

        let quick_submit = &config.shortcut_config.shortcuts["quick_submit"].key_combination;
        assert!(quick_submit.shift);
        assert!(!quick_submit.ctrl && !quick_submit.alt && !quick_submit.meta);

        let continue_key = &config.shortcut_config.shortcuts["continue"].key_combination;
        assert!(continue_key.ctrl);
        assert!(!continue_key.alt && !continue_key.shift && !continue_key.meta);

        let enhance = &config.shortcut_config.shortcuts["enhance"].key_combination;
        assert!(enhance.ctrl && enhance.shift);
        assert!(!enhance.alt && !enhance.meta);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn merge_preserves_the_whole_set_when_one_legacy_binding_was_customized() {
        let mut config = AppConfig::default();
        let quick_submit = config
            .shortcut_config
            .shortcuts
            .get_mut("quick_submit")
            .unwrap();
        quick_submit.key_combination.shift = false;
        quick_submit.key_combination.meta = true;

        let continue_key = config
            .shortcut_config
            .shortcuts
            .get_mut("continue")
            .unwrap();
        continue_key.key_combination.ctrl = false;
        continue_key.key_combination.shift = true;

        let enhance = config.shortcut_config.shortcuts.get_mut("enhance").unwrap();
        enhance.key_combination.key = "F8".to_string();
        enhance.key_combination.ctrl = true;
        enhance.key_combination.alt = false;
        enhance.key_combination.shift = false;

        merge_default_shortcuts(&mut config);

        let quick_submit = &config.shortcut_config.shortcuts["quick_submit"].key_combination;
        assert!(quick_submit.meta);
        assert!(!quick_submit.ctrl && !quick_submit.alt && !quick_submit.shift);

        let continue_key = &config.shortcut_config.shortcuts["continue"].key_combination;
        assert!(continue_key.shift);
        assert!(!continue_key.ctrl && !continue_key.alt && !continue_key.meta);

        let enhance = &config.shortcut_config.shortcuts["enhance"].key_combination;
        assert_eq!(enhance.key, "F8");
        assert!(enhance.ctrl);
    }

    #[test]
    fn merge_preserves_existing_user_shortcuts_and_adds_only_missing_actions() {
        let mut config = AppConfig::default();
        let quick_submit = config
            .shortcut_config
            .shortcuts
            .get_mut("quick_submit")
            .expect("quick submit default");
        quick_submit.key_combination.key = "F8".to_string();
        quick_submit.key_combination.ctrl = true;
        quick_submit.key_combination.alt = false;
        quick_submit.key_combination.shift = false;
        quick_submit.key_combination.meta = false;
        config.shortcut_config.shortcuts.remove("continue");

        merge_default_shortcuts(&mut config);

        let preserved = &config.shortcut_config.shortcuts["quick_submit"].key_combination;
        assert_eq!(preserved.key, "F8");
        assert!(preserved.ctrl);
        assert!(!preserved.alt && !preserved.shift && !preserved.meta);
        assert!(config.shortcut_config.shortcuts.contains_key("continue"));
    }
}
