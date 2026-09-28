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

pub async fn save_config(state: &State<'_, AppState>, app: &AppHandle) -> Result<()> {
    let config_path = get_config_path(app)?;

    // 确保目录存在
    if let Some(parent) = config_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut config = state
        .config
        .lock()
        .map_err(|e| anyhow::anyhow!("获取配置失败: {}", e))?;
    *config = save_config_changes(&config)?;

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
        config.save_baseline = Some(serde_json::to_value(&config)?);

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
        config.save_baseline = Some(serde_json::to_value(&config)?);

        Ok(config)
    } else {
        // 如果配置文件不存在，返回默认配置
        Ok(AppConfig::default())
    }
}

/// 独立保存配置文件（用于 bridge-only / MCP 等无 Tauri AppState 的进程）
pub fn save_standalone_config(config: &AppConfig) -> Result<()> {
    save_config_changes(config)?;
    Ok(())
}

/// All GUI and standalone writers merge only their changes under the same file lock.
/// A concurrent change to the same field is rejected instead of silently overwritten.
pub fn save_config_changes(config: &AppConfig) -> Result<AppConfig> {
    update_config_locked(|latest| {
        let mut current = serde_json::to_value(&*latest)?;
        let desired = serde_json::to_value(config)?;
        let default = serde_json::to_value(AppConfig::default())?;
        merge_config_fields(config.save_baseline.as_ref().unwrap_or(&default), &desired, &mut current, "")?;
        *latest = serde_json::from_value(current)?;
        Ok(())
    })
}

pub fn update_config_locked(change: impl FnOnce(&mut AppConfig) -> Result<()>) -> Result<AppConfig> {
    let path = get_standalone_config_path()?;
    let parent = path.parent().ok_or_else(|| anyhow::anyhow!("配置路径无效"))?;
    fs::create_dir_all(parent)?;
    let lock = fs::OpenOptions::new().create(true).truncate(false).read(true).write(true)
        .open(parent.join("config.lock"))?;
    lock.lock()?;
    let mut merged = load_standalone_config()?;
    change(&mut merged)?;
    atomic_write_config(&path, &serde_json::to_vec_pretty(&merged)?)?;
    merged.save_baseline = Some(serde_json::to_value(&merged)?);
    Ok(merged)
}

fn merge_config_fields(base: &serde_json::Value, desired: &serde_json::Value, current: &mut serde_json::Value, path: &str) -> Result<()> {
    if base == desired { return Ok(()); }
    if let (Some(before), Some(after), Some(now)) = (base.as_object(), desired.as_object(), current.as_object_mut()) {
        for key in before.keys().chain(after.keys()).collect::<std::collections::BTreeSet<_>>() {
            let field = format!("{path}/{key}");
            match (before.get(key), after.get(key)) {
                (Some(b), Some(d)) => {
                    let value = now.entry(key.clone()).or_insert(serde_json::Value::Null);
                    merge_config_fields(b, d, value, &field)?;
                }
                (None, Some(d)) => {
                    anyhow::ensure!(now.get(key).is_none_or(|v| v == d), "设置 {field} 已在另一窗口改变，请重新加载");
                    now.insert(key.clone(), d.clone());
                }
                (Some(b), None) => {
                    anyhow::ensure!(now.get(key).is_none_or(|v| v == b), "设置 {field} 已在另一窗口改变，请重新加载");
                    now.remove(key);
                }
                _ => {}
            }
        }
    } else {
        anyhow::ensure!(current == base || current == desired, "设置 {path} 已在另一窗口改变，请重新加载");
        *current = desired.clone();
    }
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

#[cfg(test)]
mod shortcut_tests {
    use super::merge_default_shortcuts;
    use crate::config::AppConfig;

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
