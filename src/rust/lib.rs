pub mod app;
pub mod bridge;
pub mod browser;
pub mod config;
pub mod constants;
pub mod conversation;
pub mod cross_device;
pub mod delivery;
pub mod ghost_suggestion_learning;
pub mod ghost_suggestions;
pub mod ipc;
pub mod license;
pub mod mcp;
pub mod native_speech;
pub mod relay;
pub mod server;
pub mod speech_memory;
pub mod telegram;
pub mod tunnel;
pub mod ui;
pub mod utils;

// 避免重名导出，使用限定导出
pub use config::*;
pub use conversation::*;
pub use utils::*;

// 选择性导出常用项，避免冲突
pub use constants::{
    app as app_constants, network, telegram as telegram_constants, theme, validation,
};
pub use mcp::{handlers, server as mcp_server, tools, types, utils as mcp_utils};
pub use ui::{audio as ui_audio, audio_assets, updater, window as ui_window};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    app::run_tauri_app();
}
