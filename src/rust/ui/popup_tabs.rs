//! Optional presentation grouping. Request ownership and form state stay in the
//! original popup processes; only the selected native window is visible.
use crate::config::{load_standalone_config, PopupDisplayMode};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

static STARTED: AtomicBool = AtomicBool::new(false);
static REGISTERED: AtomicBool = AtomicBool::new(false);
static SEND_WAS_FOREGROUND: AtomicBool = AtomicBool::new(false);
static SEND_INPUT_TIME: AtomicU32 = AtomicU32::new(0);
static SEND_HANDOFF: std::sync::Mutex<Option<SendHandoff>> = std::sync::Mutex::new(None);

#[derive(Debug, Clone)]
struct SendHandoff { request_id: String, target_pid: u32, input_time: Option<u32> }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tab {
    pid: u32,
    request_id: String,
    title: String,
    project_path: String,
    registered_at: String,
    unread: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Geometry { x: i32, y: i32, width: u32, height: u32 }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct Group {
    mode: PopupDisplayMode,
    active_pid: Option<u32>,
    focus_pid: Option<u32>,
    geometry: Option<Geometry>,
    tabs: Vec<Tab>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Snapshot {
    mode: PopupDisplayMode,
    current_pid: u32,
    active_pid: Option<u32>,
    tabs: Vec<Tab>,
}

impl Group {
    fn reconcile(&mut self, alive: impl Fn(&Tab) -> bool) {
        // Resolve the successor in the original order before removing dead
        // tabs, so completing a middle tab advances right instead of to first.
        let active_index = self.tabs.iter().position(|t| Some(t.pid) == self.active_pid);
        let mut next_pid = None;
        let mut index = 0;
        self.tabs.retain(|tab| {
            let live = alive(tab);
            if live && next_pid.is_none() && active_index.is_some_and(|active| index > active) {
                next_pid = Some(tab.pid);
            }
            index += 1;
            live
        });
        if !self.tabs.iter().any(|t| Some(t.pid) == self.active_pid) {
            self.active_pid = next_pid.or_else(|| self.tabs.first().map(|t| t.pid));
            // Losing a process never steals focus from another application.
            self.focus_pid = None;
        }
        if let Some(tab) = self.tabs.iter_mut().find(|t| Some(t.pid) == self.active_pid) {
            tab.unread = false;
        }
    }

    fn select(&mut self, pid: u32) -> Result<(), String> {
        let tab = self.tabs.iter_mut().find(|t| t.pid == pid)
            .ok_or_else(|| "该标签已关闭".to_string())?;
        tab.unread = false;
        self.active_pid = Some(pid);
        self.focus_pid = Some(pid);
        Ok(())
    }

    fn complete(&mut self, pid: u32, request_id: &str) -> Result<bool, String> {
        let Some(tab) = self.tabs.iter().find(|tab| tab.pid == pid) else { return Ok(false) };
        if tab.request_id != request_id { return Err("标签请求已变化，未切换".into()); }
        let active = self.active_pid == Some(pid);
        self.reconcile(|tab| tab.pid != pid);
        Ok(active)
    }

    fn send_successor(&self, pid: u32) -> Option<u32> {
        if self.mode != PopupDisplayMode::Tabs || self.active_pid != Some(pid) || self.tabs.len() < 2 {
            return None;
        }
        let index = self.tabs.iter().position(|tab| tab.pid == pid)?;
        Some(self.tabs[(index + 1) % self.tabs.len()].pid)
    }

    fn restore_send_handoff(&mut self, pid: u32, handoff: &SendHandoff) -> bool {
        if self.mode != PopupDisplayMode::Tabs || self.active_pid != Some(handoff.target_pid)
            || !self.tabs.iter().any(|tab| tab.pid == pid && tab.request_id == handoff.request_id) {
            return false;
        }
        if self.select(pid).is_err() { return false; }
        self.focus_pid = None;
        true
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot { mode: self.mode, current_pid: std::process::id(),
            active_pid: self.active_pid, tabs: self.tabs.clone() }
    }
}

fn state_path() -> std::path::PathBuf {
    std::env::temp_dir().join("iterate_popup_tabs.json")
}

fn update_at(path: &Path, change: impl FnOnce(&mut Group) -> Result<(), String>) -> Result<Group, String> {
    let lock = OpenOptions::new().read(true).write(true).create(true)
        .open(path.with_extension("lock")).map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    fs2::FileExt::lock_exclusive(&lock).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
    }
    let mut group = match fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<Group>(&bytes).map_err(|e| e.to_string())?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Group {
            mode: load_standalone_config().map_err(|e| e.to_string())?.ui_config.window_config.popup_display_mode,
            active_pid: None, focus_pid: None, geometry: None, tabs: vec![],
        },
        Err(e) => return Err(e.to_string()),
    };
    let before = group.clone();
    group.reconcile(is_live);
    change(&mut group)?;
    group.reconcile(|_| true);
    if group != before || !path.exists() {
        let staging = path.with_extension(format!("{}.tmp", std::process::id()));
        let bytes = serde_json::to_vec(&group).map_err(|e| e.to_string())?;
        fs::write(&staging, bytes).map_err(|e| e.to_string())?;
        fs::rename(&staging, path).map_err(|e| e.to_string())?;
    }
    Ok(group)
}

fn update(change: impl FnOnce(&mut Group) -> Result<(), String>) -> Result<Group, String> {
    update_at(&state_path(), change)
}

fn is_live(tab: &Tab) -> bool {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, tab.pid);
        if handle.is_null() { return false; }
        let mut code = 0;
        let mut created: FILETIME = std::mem::zeroed();
        let mut exited: FILETIME = std::mem::zeroed();
        let mut kernel: FILETIME = std::mem::zeroed();
        let mut user: FILETIME = std::mem::zeroed();
        let running = GetExitCodeProcess(handle, &mut code) != 0 && code == 259;
        let same = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0
            && chrono::DateTime::parse_from_rfc3339(&tab.registered_at).is_ok_and(|registered| {
                let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
                i128::from(ticks / 10) - 11_644_473_600_000_000i128 <= i128::from(registered.timestamp_micros())
            });
        CloseHandle(handle);
        running && same
    }
    #[cfg(unix)]
    { unsafe { libc::kill(tab.pid as i32, 0) == 0 } }
    #[cfg(not(any(target_os = "windows", unix)))]
    { let _ = tab; false }
}

fn geometry(window: &tauri::WebviewWindow) -> Option<Geometry> {
    let position = window.outer_position().ok()?;
    let size = window.inner_size().ok()?;
    Some(Geometry { x: position.x, y: position.y, width: size.width, height: size.height })
}

fn apply_geometry(window: &tauri::WebviewWindow, g: &Geometry) -> Result<(), String> {
    if window.outer_position().map_or(true, |p| p.x != g.x || p.y != g.y) {
        window.set_position(tauri::PhysicalPosition::new(g.x, g.y)).map_err(|e| e.to_string())?;
    }
    if window.inner_size().map_or(true, |s| s.width != g.width || s.height != g.height) {
        window.set_size(tauri::PhysicalSize::new(g.width, g.height)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn is_visible(window: &tauri::WebviewWindow) -> bool {
    #[cfg(target_os = "windows")]
    { window.hwnd().is_ok_and(|handle| unsafe {
        windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible(handle.0) != 0
    }) }
    #[cfg(not(target_os = "windows"))]
    { window.is_visible().unwrap_or(false) }
}

#[cfg(target_os = "windows")]
fn tab_window(pid: u32) -> Option<windows_sys::Win32::Foundation::HWND> {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetClassNameW,
        GetWindowThreadProcessId};
    struct Search { pid: u32, hwnd: Option<HWND> }
    unsafe extern "system" fn inspect(hwnd: HWND, data: LPARAM) -> i32 {
        let search = &mut *(data as *mut Search);
        let mut owner = 0;
        GetWindowThreadProcessId(hwnd, &mut owner);
        if owner == search.pid {
            let mut class = [0u16; 32];
            let len = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
            if len > 0 && class[..len as usize].iter().copied().eq("Tauri Window".encode_utf16()) {
                search.hwnd = Some(hwnd);
                return 0;
            }
        }
        1
    }
    let mut search = Search { pid, hwnd: None };
    unsafe { EnumWindows(Some(inspect), &mut search as *mut Search as LPARAM); }
    search.hwnd
}

#[cfg(target_os = "windows")]
fn tab_window_visible(pid: u32) -> bool {
    tab_window(pid).is_some_and(|hwnd| unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
        IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0
    })
}

fn handoff_ready(active_pid: Option<u32>, destination_visible: bool) -> bool {
    active_pid.is_none() || destination_visible
}

fn hide(window: &tauri::WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())?;
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        ShowWindow(window.hwnd().map_err(|e| e.to_string())?.0, SW_HIDE);
    }
    Ok(())
}

fn last_input_time() -> Option<u32> {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};
        let mut info = LASTINPUTINFO { cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32, dwTime: 0 };
        return (GetLastInputInfo(&mut info) != 0).then_some(info.dwTime);
    }
    #[cfg(not(target_os = "windows"))]
    { None }
}

pub fn remember_send_focus(window: &tauri::WebviewWindow) {
    SEND_WAS_FOREGROUND.store(false, Ordering::SeqCst);
    if !REGISTERED.load(Ordering::SeqCst) { return; }
    #[cfg(target_os = "windows")]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        if window.hwnd().is_ok_and(|handle| GetForegroundWindow() == handle.0) {
            if let Some(input_time) = last_input_time() {
                SEND_INPUT_TIME.store(input_time, Ordering::SeqCst);
                SEND_WAS_FOREGROUND.store(true, Ordering::SeqCst);
            }
        }
    }
}

fn may_handoff_focus(was_foreground: bool, input_time: u32, current_input: Option<u32>) -> bool {
    was_foreground && current_input == Some(input_time)
}

/// Hand the existing peer window over while the sending window still owns
/// foreground permission. The source request remains live until its real ACK.
pub fn prepare_send_handoff(window: &tauri::WebviewWindow) -> Result<(), String> {
    remember_send_focus(window);
    if !REGISTERED.load(Ordering::SeqCst) { return Ok(()); }
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::RECT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{AllowSetForegroundWindow,
            GetClientRect, GetForegroundWindow, GetWindowRect, IsWindowVisible,
            SetForegroundWindow, SetWindowPos, ShowWindow, SWP_NOACTIVATE, SWP_NOZORDER,
            SW_SHOWNOACTIVATE};
        let source = window.hwnd().map_err(|e| e.to_string())?.0;
        let foreground = unsafe { GetForegroundWindow() == source };
        let current_geometry = geometry(window);
        let mut destination = None;
        let mut handoff = None;
        let group = update(|group| {
            let Some(next_pid) = group.send_successor(std::process::id()) else { return Ok(()) };
            let Some(hwnd) = tab_window(next_pid) else { return Err("下一标签窗口尚不可用".into()) };
            let request_id = group.tabs.iter().find(|tab| tab.pid == std::process::id())
                .ok_or("发送标签已关闭")?.request_id.clone();
            if foreground { unsafe { AllowSetForegroundWindow(next_pid); } }
            group.select(next_pid)?;
            // Native activation below is immediate. The later worker must not
            // activate again after the user has moved to another application.
            group.focus_pid = None;
            group.geometry = current_geometry.clone();
            destination = Some(hwnd);
            handoff = Some(SendHandoff { request_id, target_pid: next_pid, input_time: last_input_time() });
            Ok(())
        })?;
        let Some(target) = destination else { return Ok(()) };
        *SEND_HANDOFF.lock().map_err(|_| "发送交接状态锁定失败")? = handoff;
        let presented = (|| -> Result<(), String> {
            if let Some(g) = group.geometry.as_ref() {
                let mut outer: RECT = unsafe { std::mem::zeroed() };
                let mut client: RECT = unsafe { std::mem::zeroed() };
                if unsafe { GetWindowRect(target, &mut outer) } == 0
                    || unsafe { GetClientRect(target, &mut client) } == 0 {
                    return Err("无法读取下一标签窗口尺寸".into());
                }
                let width = g.width as i32 + (outer.right - outer.left) - (client.right - client.left);
                let height = g.height as i32 + (outer.bottom - outer.top) - (client.bottom - client.top);
                if unsafe { SetWindowPos(target, std::ptr::null_mut(), g.x, g.y, width, height,
                    SWP_NOACTIVATE | SWP_NOZORDER) } == 0 {
                    return Err("无法对齐下一标签窗口".into());
                }
            }
            unsafe { ShowWindow(target, SW_SHOWNOACTIVATE); }
            if unsafe { IsWindowVisible(target) } == 0 {
                return Err("下一标签窗口未显示，保留当前窗口".into());
            }
            // Do this before hiding the source, not after a network ACK or a
            // timer tick. Ordinary mouse movement after send cannot cancel it.
            if foreground && (unsafe { GetForegroundWindow() == source }
                || may_handoff_focus(true, SEND_INPUT_TIME.load(Ordering::SeqCst), last_input_time())) {
                unsafe { SetForegroundWindow(target); }
            }
            log::info!("[Tab send handoff] source={} target={} foreground={}",
                std::process::id(), group.active_pid.unwrap_or_default(), foreground);
            Ok(())
        })();
        if presented.is_err() {
            let pending = SEND_HANDOFF.lock().map_err(|_| "发送交接状态锁定失败")?.take();
            if let Some(pending) = pending {
                update(|group| { group.restore_send_handoff(std::process::id(), &pending); Ok(()) })?;
            }
        }
        presented?;
    }
    Ok(())
}

/// Restore a failed send without taking over a peer the user has since edited
/// or selected. Returning true means tab presentation has been handled here.
#[tauri::command]
pub fn restore_popup_tab(app: AppHandle) -> Result<bool, String> {
    if !REGISTERED.load(Ordering::SeqCst) { return Ok(false); }
    let pending = SEND_HANDOFF.lock().map_err(|_| "发送交接状态锁定失败")?.take();
    let Some(pending) = pending else { return Ok(false) };
    let mut restored = false;
    update(|group| {
        if pending.input_time.is_some() && pending.input_time == last_input_time() {
            restored = group.restore_send_handoff(std::process::id(), &pending);
        }
        Ok(())
    })?;
    if restored {
        let window = app.get_webview_window("main").ok_or("窗口不存在")?;
        #[cfg(target_os = "windows")]
        let focus = unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
            let foreground = GetForegroundWindow();
            window.hwnd().is_ok_and(|hwnd| hwnd.0 == foreground)
                || tab_window(pending.target_pid) == Some(foreground)
        };
        #[cfg(not(target_os = "windows"))]
        let focus = false;
        show(&window, focus)?;
    }
    Ok(true)
}

/// Called only after the real send ACK, before optional cloud cleanup or exit.
#[tauri::command]
pub fn complete_popup_tab(request_id: String) -> Result<(), String> {
    if !REGISTERED.load(Ordering::SeqCst) { return Ok(()); }
    let focus = may_handoff_focus(SEND_WAS_FOREGROUND.swap(false, Ordering::SeqCst),
        SEND_INPUT_TIME.load(Ordering::SeqCst), last_input_time());
    update(|group| {
        let active = group.complete(std::process::id(), &request_id)?;
        if active && focus && group.mode == PopupDisplayMode::Tabs {
            #[cfg(target_os = "windows")]
            if let Some(next_pid) = group.active_pid {
                // Grant before publishing the group, so the destination worker
                // cannot try foreground activation ahead of the permission.
                if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(next_pid) } != 0 {
                    group.focus_pid = Some(next_pid);
                }
            }
        }
        Ok(())
    })?;
    let mut pending = SEND_HANDOFF.lock().map_err(|_| "发送交接状态锁定失败")?;
    if pending.as_ref().is_some_and(|handoff| handoff.request_id == request_id) { *pending = None; }
    Ok(())
}

fn show(window: &tauri::WebviewWindow, focus: bool) -> Result<(), String> {
    if focus {
        window.unminimize().map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
    } else {
        #[cfg(target_os = "windows")]
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_SHOWNOACTIVATE};
            ShowWindow(window.hwnd().map_err(|e| e.to_string())?.0, SW_SHOWNOACTIVATE);
        }
        #[cfg(not(target_os = "windows"))]
        window.show().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Native polling remains live when WebView timers are suspended in hidden tabs.
fn start_worker(app: AppHandle, initial: Group) {
    if STARTED.swap(true, Ordering::SeqCst) { return; }
    std::thread::spawn(move || {
        let pid = std::process::id();
        let mut previous = initial;
        let mut split_geometry = None;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            let Some(window) = app.get_webview_window("main") else { break };
            // Native queries can marshal onto the UI thread. Never hold the
            // inter-process file lock while waiting for that thread: a UI
            // command may itself be waiting to update the tab group.
            let current_geometry = if previous.mode == PopupDisplayMode::Tabs
                && previous.active_pid == Some(pid) && is_visible(&window)
                && !window.is_minimized().unwrap_or(false) {
                geometry(&window)
            } else { None };
            let result = update(|group| {
                if group.mode == PopupDisplayMode::Tabs && group.active_pid == Some(pid)
                    && previous.active_pid == Some(pid) && current_geometry.is_some() {
                    group.geometry = current_geometry;
                }
                Ok(())
            });
            let Ok(group) = result else { continue };
            let was_active = previous.active_pid == Some(pid);
            let active = group.active_pid == Some(pid);
            let tabs = group.mode == PopupDisplayMode::Tabs;
            let was_tabs = previous.mode == PopupDisplayMode::Tabs;
            let visibility_result = if tabs && !active {
                if !was_tabs { split_geometry = geometry(&window); }
                // The destination runs in another process with an independent
                // polling phase. Keep covering the desktop until its native
                // window is actually shown, including older popup versions.
                #[cfg(target_os = "windows")]
                let ready = handoff_ready(group.active_pid,
                    group.active_pid.is_some_and(tab_window_visible));
                #[cfg(not(target_os = "windows"))]
                let ready = true;
                if ready && is_visible(&window) { hide(&window) } else { Ok(()) }
            } else if tabs && active && (!was_active || !was_tabs) {
                if !was_tabs { split_geometry = geometry(&window); }
                group.geometry.as_ref().map(|g| apply_geometry(&window, g)).transpose()
                    .and_then(|_| show(&window, group.focus_pid == Some(pid)))
            } else if !tabs && was_tabs {
                split_geometry.take().as_ref().map(|g| apply_geometry(&window, g)).transpose()
                    .and_then(|_| show(&window, false))
            } else { Ok(()) };
            if let Err(error) = visibility_result {
                log::warn!("标签窗口显示失败: {error}");
                continue;
            }
            if group.snapshot() != previous.snapshot() {
                let _ = window.emit("popup-tabs-changed", group.snapshot());
            }
            previous = group;
        }
    });
}

#[tauri::command]
pub fn register_popup_tab(app: AppHandle, request_id: String, title: String, project_path: String) -> Result<Snapshot, String> {
    if request_id.trim().is_empty() { return Err("标签缺少请求标识".into()); }
    let pid = std::process::id();
    let group = update(|group| {
        if let Some(tab) = group.tabs.iter_mut().find(|t| t.pid == pid) {
            if tab.request_id != request_id { tab.unread = group.active_pid != Some(pid); }
            tab.request_id = request_id;
            tab.title = title;
            tab.project_path = project_path;
        } else {
            group.tabs.push(Tab { pid, request_id, title, project_path,
                registered_at: chrono::Utc::now().to_rfc3339(), unread: true });
        }
        Ok(())
    })?;
    REGISTERED.store(true, Ordering::SeqCst);
    start_worker(app, group.clone());
    Ok(group.snapshot())
}

#[tauri::command]
pub fn get_popup_tabs() -> Result<Snapshot, String> { update(|_| Ok(())).map(|g| g.snapshot()) }

#[tauri::command]
pub fn select_popup_tab(app: AppHandle, pid: u32) -> Result<Snapshot, String> {
    let window = app.get_webview_window("main").ok_or("窗口不存在")?;
    let current_geometry = geometry(&window);
    let group = update(|group| {
        if group.mode != PopupDisplayMode::Tabs { return Err("当前未使用标签页显示".into()); }
        if group.active_pid != Some(std::process::id()) { return Err("请从当前标签切换".into()); }
        if !group.tabs.iter().any(|tab| tab.pid == pid) { return Err("该标签已关闭".into()); }
        // The destination must have permission before its worker observes the
        // new selection and attempts foreground activation.
        #[cfg(target_os = "windows")]
        unsafe { windows_sys::Win32::UI::WindowsAndMessaging::AllowSetForegroundWindow(pid); }
        group.select(pid)?;
        group.geometry = current_geometry;
        Ok(())
    })?;
    // Keep the source window until the destination's native worker presents it.
    // No form remount, clipboard transfer, or response route mutation occurs.
    Ok(group.snapshot())
}

pub fn set_mode(app: &AppHandle, mode: PopupDisplayMode) -> Result<(), String> {
    update(|group| {
        group.mode = mode;
        if mode == PopupDisplayMode::Tabs {
            let pid = std::process::id();
            if group.tabs.iter().any(|t| t.pid == pid) {
                group.select(pid)?;
                group.geometry = app.get_webview_window("main").as_ref().and_then(geometry);
            }
        }
        Ok(())
    })?;
    Ok(())
}

pub fn should_present_current() -> Result<bool, String> {
    if !REGISTERED.load(Ordering::SeqCst) { return Ok(true); }
    update(|_| Ok(())).map(|g| g.mode != PopupDisplayMode::Tabs || g.active_pid == Some(std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn group() -> Group {
        Group { mode: PopupDisplayMode::Tabs, active_pid: Some(1), focus_pid: None, geometry: None,
            tabs: (1..=3).map(|pid| Tab { pid, request_id: format!("req-{pid}"), title: "same title".into(),
                project_path: "same project".into(), registered_at: "now".into(), unread: pid != 1 }).collect() }
    }
    #[test]
    fn arrival_keeps_active_and_close_selects_only_live_tab_without_focus() {
        let mut g = group();
        g.reconcile(|_| true);
        assert_eq!(g.active_pid, Some(1));
        assert!(g.tabs[2].unread);
        g.reconcile(|t| t.pid != 1);
        assert_eq!(g.active_pid, Some(2));
        assert_eq!(g.focus_pid, None);
        assert!(!g.tabs[0].unread);
    }
    #[test]
    fn same_project_title_does_not_merge_requests_and_stale_click_cannot_reroute() {
        let mut g = group();
        let ids: Vec<_> = g.tabs.iter().map(|t| t.request_id.clone()).collect();
        g.select(3).unwrap();
        assert_eq!(g.active_pid, Some(3));
        assert_eq!(g.tabs.iter().map(|t| t.request_id.clone()).collect::<Vec<_>>(), ids);
        assert!(!g.tabs[2].unread);
        assert!(g.select(99).is_err());
        assert_eq!(g.active_pid, Some(3));
    }
    #[test]
    fn completing_middle_tab_advances_right_and_last_wraps() {
        let mut g = group();
        g.select(2).unwrap();
        g.reconcile(|t| t.pid != 2);
        assert_eq!(g.active_pid, Some(3));
        assert_eq!(g.focus_pid, None);
        assert!(!g.tabs.iter().find(|t| t.pid == 3).unwrap().unread);
        g.reconcile(|t| t.pid != 3);
        assert_eq!(g.active_pid, Some(1));
        assert_eq!(g.focus_pid, None);
        g.reconcile(|_| false);
        assert_eq!(g.active_pid, None);
    }
    #[test]
    fn successor_skips_dead_tabs_and_background_close_keeps_active() {
        let mut g = group();
        let mut fourth = g.tabs[2].clone();
        fourth.pid = 4;
        fourth.request_id = "req-4".into();
        g.tabs.push(fourth);
        g.select(2).unwrap();
        g.reconcile(|t| t.pid != 1);
        assert_eq!(g.active_pid, Some(2));
        g.reconcile(|t| t.pid != 2 && t.pid != 3);
        assert_eq!(g.active_pid, Some(4));
        assert_eq!(g.focus_pid, None);
    }
    #[test]
    fn legacy_config_defaults_to_separate_windows() {
        let mut config = serde_json::to_value(crate::config::default_window_config()).unwrap();
        config.as_object_mut().unwrap().remove("popup_display_mode");
        let decoded: crate::config::WindowConfig = serde_json::from_value(config).unwrap();
        assert_eq!(decoded.popup_display_mode, PopupDisplayMode::Windows);
    }
    #[test]
    fn confirmed_completion_requires_matching_request_and_preserves_other_tabs() {
        let mut g = group();
        g.select(2).unwrap();
        let before = g.clone();
        assert!(g.complete(2, "wrong-request").is_err());
        assert_eq!(g, before);
        assert!(g.complete(2, "req-2").unwrap());
        assert_eq!(g.active_pid, Some(3));
        assert_eq!(g.tabs.iter().map(|tab| tab.request_id.as_str()).collect::<Vec<_>>(), vec!["req-1", "req-3"]);
        assert!(!g.complete(1, "req-1").unwrap());
        assert_eq!(g.active_pid, Some(3));
        assert!(!g.complete(99, "missing").unwrap());
    }
    #[test]
    fn background_send_or_new_user_input_never_requests_foreground() {
        assert!(may_handoff_focus(true, 123, Some(123)));
        assert!(!may_handoff_focus(false, 123, Some(123)));
        assert!(!may_handoff_focus(true, 123, Some(124)));
        assert!(!may_handoff_focus(true, 123, None));
    }

    #[test]
    fn handoff_keeps_source_until_destination_is_presented() {
        assert!(!handoff_ready(Some(2), false));
        assert!(handoff_ready(Some(2), true));
        // If a failed destination disappears, reconciliation can select
        // another live tab; that window must also be shown before hiding.
        assert!(!handoff_ready(Some(3), false));
        assert!(handoff_ready(Some(3), true));
        assert!(handoff_ready(None, false));
    }

    #[test]
    fn send_handoff_keeps_request_until_ack_and_does_not_skip_successor() {
        let mut g = group();
        g.select(2).unwrap();
        let next = g.send_successor(2).unwrap();
        assert_eq!(next, 3);
        g.select(next).unwrap();
        assert_eq!(g.tabs.len(), 3); // Still pending, not an acknowledged completion.
        assert_eq!(g.tabs[1].request_id, "req-2");
        assert!(!g.complete(2, "req-2").unwrap());
        assert_eq!(g.active_pid, Some(3));
        assert_eq!(g.send_successor(3), Some(1));
        assert_eq!(g.tabs.len(), 2);
        assert_eq!(g.send_successor(1), None); // Background sends never take over.
    }

    #[test]
    fn failed_handoff_restores_only_original_request_and_unchanged_selection() {
        let mut g = group();
        let pending = SendHandoff { request_id: "req-1".into(), target_pid: 2, input_time: Some(123) };
        g.select(2).unwrap();
        assert!(g.restore_send_handoff(1, &pending));
        assert_eq!(g.active_pid, Some(1));
        assert_eq!(g.focus_pid, None);
        assert_eq!(g.tabs.len(), 3);
        g.select(3).unwrap();
        assert!(!g.restore_send_handoff(1, &pending));
        assert_eq!(g.active_pid, Some(3));
        g.select(2).unwrap();
        g.tabs[0].request_id = "new-request".into();
        assert!(!g.restore_send_handoff(1, &pending));
        assert_eq!(g.active_pid, Some(2));
    }

    #[test]
    fn separate_windows_and_single_tab_do_not_prepare_peer_handoff() {
        let mut g = group();
        g.mode = PopupDisplayMode::Windows;
        assert_eq!(g.send_successor(1), None);
        g.mode = PopupDisplayMode::Tabs;
        g.tabs.truncate(1);
        assert_eq!(g.send_successor(1), None);
    }
}
