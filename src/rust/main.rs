#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use anyhow::Result;
use cunzhi::app::{handle_cli_args, handle_early_cli_args, run_tauri_app};
use cunzhi::utils::auto_init_logger;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_tauri_app();
}

fn main() -> Result<()> {
    #[cfg(target_os = "macos")]
    load_installed_profile()?;
    #[cfg(target_os = "windows")]
    cunzhi::app::windows_lifecycle::activate_manual_launch_if_requested(
        &std::env::args().collect::<Vec<_>>(),
    )?;

    if handle_early_cli_args() {
        return Ok(());
    }

    // 初始化日志系统
    if let Err(e) = auto_init_logger() {
        eprintln!("初始化日志系统失败: {}", e);
    }

    // 处理命令行参数
    handle_cli_args()
}

/// Installer-owned local profile preserves existing pairing when replacing a
/// legacy shell-launched bundle with the native desktop executable.
#[cfg(target_os = "macos")]
fn load_installed_profile() -> Result<()> {
    let executable = std::env::current_exe()?;
    let Some(contents) = executable.parent().and_then(std::path::Path::parent) else { return Ok(()); };
    let path = contents.join("Resources/iterate-profile.json");
    if !path.is_file() { return Ok(()); }
    let values: std::collections::BTreeMap<String, String> = serde_json::from_slice(&std::fs::read(path)?)?;
    for (key, value) in values {
        anyhow::ensure!(matches!(key.as_str(), "ITERATE_CONFIG_DIR" | "ITERATE_CROSS_DEVICE_DIR" | "ITERATE_CROSS_DEVICE_NAME" | "ITERATE_CONVERSATION_STATE_FILE"), "无效的本机配置字段");
        if std::env::var_os(&key).is_none() { std::env::set_var(key, value); }
    }
    // A legacy launcher may still supply the old binary path. Always use the
    // installed native executable for future GUI children.
    std::env::set_var("ITERATE_DIALOG_GUI_EXECUTABLE", executable);
    Ok(())
}
