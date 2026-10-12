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
