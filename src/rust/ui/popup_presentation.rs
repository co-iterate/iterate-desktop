use std::collections::HashSet;
use windows_sys::Win32::Foundation::{HWND, LPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowLongPtrW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetWindowPos, GWL_EXSTYLE, HWND_BOTTOM, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_SHOWWINDOW, WS_EX_TOPMOST,
};

struct ExistingWindows {
    current: HWND,
    pids: HashSet<u32>,
    last: HWND,
}

unsafe extern "system" fn find_last_iterate_window(hwnd: HWND, data: LPARAM) -> i32 {
    let windows = &mut *(data as *mut ExistingWindows);
    let mut pid = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if hwnd != windows.current && windows.pids.contains(&pid)
        && IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0
        && GetWindowLongPtrW(hwnd, GWL_EXSTYLE) & WS_EX_TOPMOST as isize == 0 {
        windows.last = hwnd;
    }
    1
}

/// Show without a foreground flash and put only the new popup behind the other
/// Iterate windows. Existing windows retain their focus, contents and ordering.
pub fn show_behind_existing(window: &tauri::WebviewWindow) -> Result<(), String> {
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
    let mut registry = super::window_registry::WindowRegistry::load();
    let mut windows = ExistingWindows {
        current: hwnd,
        pids: registry.get_all_instances().into_iter().map(|instance| instance.pid).collect(),
        // With no visible peers, a do-not-disturb popup must still not cover
        // whichever other application the user is currently working in.
        last: HWND_BOTTOM,
    };
    unsafe {
        if EnumWindows(Some(find_last_iterate_window), &mut windows as *mut _ as LPARAM) == 0 {
            return Err(format!("枚举弹窗层级失败: {}", std::io::Error::last_os_error()));
        }
        if SetWindowPos(hwnd, windows.last, 0, 0, 0, 0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW) == 0 {
            return Err(format!("在已有弹窗后显示失败: {}", std::io::Error::last_os_error()));
        }
    }
    Ok(())
}
