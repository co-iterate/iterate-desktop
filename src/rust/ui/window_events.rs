use crate::app::setup::set_last_focused_window;
use crate::config::AppState;
use crate::log_important;
use crate::ui::window_registry::WindowRegistry;
use tauri::{AppHandle, Manager, WindowEvent};

/// 设置窗口事件监听器
pub fn setup_window_event_listeners(app_handle: &AppHandle) {
    // 为所有窗口设置焦点追踪
    for (label, window) in app_handle.webview_windows() {
        let label_clone = label.clone();
        window.on_window_event(move |event| {
            if let WindowEvent::Focused(true) = event {
                set_last_focused_window(&label_clone);
                let mut registry = WindowRegistry::load();
                if let Err(error) = registry.mark_current_window_focused() {
                    log_important!(warn, "记录窗口聚焦时间失败: {}", error);
                }
            }
        });
    }

    if let Some(window) = app_handle.get_webview_window("main") {
        let app_handle_clone = app_handle.clone();

        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 阻止默认的关闭行为
                api.prevent_close();

                let app_handle = app_handle_clone.clone();

                // 异步处理退出请求
                tauri::async_runtime::spawn(async move {
                    let state = app_handle.state::<AppState>();

                    log::debug!("🖱️ 窗口关闭按钮被点击");

                    // 窗口关闭按钮点击应该直接退出，不需要双重确认
                    match crate::ui::exit::handle_system_exit_request(
                        state,
                        &app_handle,
                        true, // 手动点击关闭按钮
                    )
                    .await
                    {
                        Ok(exited) => {
                            if !exited {
                                log_important!(info, "退出被阻止，等待二次确认");
                            } else {
                                log_important!(info, "应用已退出");
                            }
                        }
                        Err(e) => {
                            log_important!(error, "处理退出请求失败: {}", e);
                        }
                    }
                });
            }
        });
    }
}
