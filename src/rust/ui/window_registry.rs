use crate::log_important;
#[cfg(target_os = "windows")]
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::Path;
use std::path::PathBuf;
use std::process;

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
pub struct WindowInstance {
    pub pid: u32,
    pub project_path: String,
    pub window_title: String,
    pub registered_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_title: Option<String>,
    /// 消息全文（有上限），只给窗口总览搜索用；标题仍是截短后的 `request_title`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_text: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct WindowRegistry {
    pub instances: Vec<WindowInstance>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) last_focused_at_by_pid: HashMap<u32, String>,
}

/// 测试专用：把注册表重定向到临时文件。默认路径是真实的 `$TMPDIR/iterate_windows.json`，
/// 本机运行中的 iterate 也在读写它，测试直接写会互相干扰（并行偶发失败），还会改坏线上状态。
#[cfg(test)]
pub(crate) static TEST_REGISTRY_PATH_OVERRIDE: std::sync::Mutex<Option<PathBuf>> =
    std::sync::Mutex::new(None);

impl WindowRegistry {
    fn registry_path() -> PathBuf {
        #[cfg(test)]
        if let Some(path) = TEST_REGISTRY_PATH_OVERRIDE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
        {
            return path;
        }
        let tmp_dir = std::env::temp_dir();
        tmp_dir.join("iterate_windows.json")
    }

    pub fn load() -> Self {
        load_registry_from_path(&Self::registry_path())
    }

    pub fn register(
        &mut self,
        project_path: &str,
        request_id: Option<&str>,
        request_title: Option<&str>,
    ) -> Result<(), String> {
        let pid = process::id();
        let normalized_project_path = normalize_project_path(project_path, pid);
        let normalized_request_id = normalize_optional_trimmed(request_id);
        let normalized_request_title = normalize_optional_trimmed(request_title);
        let port = serve_port_for_pid(pid);

        *self = register_instance_at_path(
            &Self::registry_path(),
            pid,
            &normalized_project_path,
            normalized_request_id.as_deref(),
            normalized_request_title.as_deref(),
            port,
        )?;
        log_important!(
            info,
            "窗口已注册: PID={}, 项目={}, request_id={:?}, port={:?}",
            pid,
            normalized_project_path,
            normalized_request_id,
            port
        );
        Ok(())
    }

    /// The serve process already knows the real child PID and immutable request identity.
    /// Register that popup before WebView startup so Bridge can route mobile delivery.
    pub(crate) fn register_spawned_popup(
        pid: u32,
        project_path: &str,
        request_id: &str,
        request_title: &str,
    ) -> Result<(), String> {
        register_spawned_popup_at_path(
            &Self::registry_path(),
            pid,
            project_path,
            request_id,
            request_title,
        )
    }

    pub fn unregister(&mut self) -> Result<(), String> {
        let pid = process::id();
        *self = update_registry_at_path(&Self::registry_path(), false, |registry| {
            registry.instances.retain(|instance| instance.pid != pid);
            registry.last_focused_at_by_pid.remove(&pid);
        })?;
        write_request_text(&Self::registry_path(), pid, None);
        log_important!(info, "窗口已注销: PID={}", pid);
        Ok(())
    }

    pub fn last_focused_at_by_pid(&self) -> &HashMap<u32, String> {
        &self.last_focused_at_by_pid
    }

    pub fn mark_current_window_focused(&mut self) -> Result<bool, String> {
        let pid = process::id();
        let focused_at = chrono::Utc::now().to_rfc3339();
        let (registry, updated) =
            mark_instance_focused_at_path(&Self::registry_path(), pid, focused_at)?;
        *self = registry;
        if updated {
            log_important!(info, "窗口聚焦时间已更新: PID={}", pid);
        }
        Ok(updated)
    }

    pub fn clear_request_binding(&mut self) -> Result<(), String> {
        let pid = process::id();
        *self = update_registry_at_path(&Self::registry_path(), false, |registry| {
            if let Some(instance) = registry
                .instances
                .iter_mut()
                .find(|instance| instance.pid == pid)
            {
                instance.request_id = None;
                instance.request_title = None;
                instance.request_text = None;
            }
        })?;
        write_request_text(&Self::registry_path(), pid, None);
        log_important!(info, "窗口请求绑定已清除: PID={}", pid);
        Ok(())
    }

    pub fn get_all_instances(&mut self) -> Vec<WindowInstance> {
        self.get_all_instances_at_path(&Self::registry_path())
    }

    fn get_all_instances_at_path(&mut self, path: &Path) -> Vec<WindowInstance> {
        match update_registry_at_path(path, true, |_| {}) {
            Ok(registry) => *self = registry,
            Err(error) => log_important!(warn, "读取窗口注册表失败: {}", error),
        }
        let mut instances = self.instances.clone();
        for instance in &mut instances {
            if instance.request_text.is_none() && instance.request_id.is_some() {
                instance.request_text = read_request_text(path, instance.pid);
            }
        }
        prune_request_texts(path, &instances);
        instances
    }

    fn cleanup_stale_instances(&mut self) {
        self.instances
            .retain(|instance| is_process_running(instance.pid));
        let live_pids = self
            .instances
            .iter()
            .map(|instance| instance.pid)
            .collect::<HashSet<_>>();
        self.last_focused_at_by_pid
            .retain(|pid, _| live_pids.contains(pid));
    }
}

fn load_registry_from_path(path: &Path) -> WindowRegistry {
    if path.exists() {
        match fs::read_to_string(&path) {
            Ok(content) => match serde_json::from_str(&content) {
                Ok(registry) => return registry,
                Err(e) => {
                    log_important!(warn, "解析窗口注册表失败: {}", e);
                }
            },
            Err(e) => {
                log_important!(warn, "读取窗口注册表失败: {}", e);
            }
        }
    }
    WindowRegistry::default()
}

fn save_registry_atomically(path: &Path, registry: &WindowRegistry) -> Result<(), String> {
    let content = serde_json::to_vec_pretty(registry)
        .map_err(|error| format!("序列化窗口注册表失败: {}", error))?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("iterate_windows.json");
    let temp_path = path.with_file_name(format!(
        ".{}.{}.{}.tmp",
        file_name,
        process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .map_err(|error| format!("创建窗口注册表临时文件失败: {}", error))?;
        std::io::Write::write_all(&mut file, &content)
            .map_err(|error| format!("写入窗口注册表临时文件失败: {}", error))?;
        file.sync_all()
            .map_err(|error| format!("同步窗口注册表临时文件失败: {}", error))?;
        fs::rename(&temp_path, path).map_err(|error| format!("原子替换窗口注册表失败: {}", error))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp_path);
    }
    result
}

fn update_registry_at_path(
    path: &Path,
    cleanup_stale: bool,
    mutate: impl FnOnce(&mut WindowRegistry),
) -> Result<WindowRegistry, String> {
    #[cfg(not(target_os = "windows"))]
    let _ = cleanup_stale;
    let lock_path = path.with_extension("json.lock");
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .map_err(|error| format!("打开窗口注册表锁失败: {}", error))?;

    #[cfg(target_os = "windows")]
    lock_file
        .lock_exclusive()
        .map_err(|error| format!("锁定窗口注册表失败: {}", error))?;
    #[cfg(unix)]
    if unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(format!(
            "锁定窗口注册表失败: {}",
            std::io::Error::last_os_error()
        ));
    }

    let mut registry = load_registry_from_path(path);
    #[cfg(target_os = "windows")]
    let original = registry.clone();
    #[cfg(target_os = "windows")]
    if cleanup_stale {
        registry.cleanup_stale_instances();
    }
    #[cfg(not(target_os = "windows"))]
    registry.cleanup_stale_instances();
    mutate(&mut registry);
    #[cfg(target_os = "windows")]
    let save_result = if registry == original {
        Ok(())
    } else {
        save_registry_atomically(path, &registry)
    };
    #[cfg(not(target_os = "windows"))]
    let save_result = save_registry_atomically(path, &registry);

    #[cfg(target_os = "windows")]
    let _ = FileExt::unlock(&lock_file);
    #[cfg(unix)]
    let _ = unsafe { libc::flock(lock_file.as_raw_fd(), libc::LOCK_UN) };

    save_result.map(|_| registry)
}

fn register_spawned_popup_at_path(
    path: &Path,
    pid: u32,
    project_path: &str,
    request_id: &str,
    request_title: &str,
) -> Result<(), String> {
    if pid == 0 || request_id.trim().is_empty() {
        return Err("invalid spawned popup identity".to_string());
    }
    register_instance_at_path(
        path,
        pid,
        &normalize_project_path(project_path, pid),
        Some(request_id),
        Some(request_title),
        None,
    )?;
    Ok(())
}

fn register_instance_at_path(
    path: &Path,
    pid: u32,
    project_path: &str,
    request_id: Option<&str>,
    request_title: Option<&str>,
    port: Option<u16>,
) -> Result<WindowRegistry, String> {
    let project_path = project_path.to_string();
    let request_id = normalize_optional_trimmed(request_id);
    let request_text = normalize_optional_trimmed(request_title).map(|text| truncate_request_text(&text));
    let request_title =
        normalize_optional_trimmed(request_title).map(|title| truncate_request_title(&title));
    write_request_text(path, pid, request_text.as_deref());
    update_registry_at_path(path, true, move |registry| {
        if let Some(instance) = registry
            .instances
            .iter_mut()
            .find(|instance| instance.pid == pid)
        {
            instance.project_path = project_path.clone();
            instance.window_title = format!("iterate — {}", project_path);
            instance.port = port;
            instance.request_id = request_id.clone();
            instance.request_title = request_title.clone();
            instance.request_text = request_text.clone();
        } else {
            registry.instances.push(WindowInstance {
                pid,
                project_path: project_path.clone(),
                window_title: format!("iterate — {}", project_path),
                registered_at: chrono::Utc::now().to_rfc3339(),
                port,
                request_id,
                request_title,
                request_text,
            });
        }
    })
}

fn mark_instance_focused_at_path(
    path: &Path,
    pid: u32,
    focused_at: String,
) -> Result<(WindowRegistry, bool), String> {
    let mut updated = false;
    let registry = update_registry_at_path(path, false, |registry| {
        if registry
            .instances
            .iter()
            .any(|instance| instance.pid == pid)
        {
            registry.last_focused_at_by_pid.insert(pid, focused_at);
            updated = true;
        }
    })?;
    Ok((registry, updated))
}

pub fn current_window_registration_label() -> String {
    normalize_project_path("Unknown", process::id())
}

fn normalize_project_path(project_path: &str, pid: u32) -> String {
    let trimmed = project_path.trim();
    if !trimmed.is_empty() && trimmed != "." && trimmed != "Unknown" {
        return trimmed.to_string();
    }

    if let Ok(current_dir) = std::env::current_dir() {
        if let Some(current_dir_str) = current_dir.to_str() {
            let trimmed_current_dir = current_dir_str.trim();
            if !trimmed_current_dir.is_empty() && current_dir.is_absolute() {
                return trimmed_current_dir.to_string();
            }
        }
    }

    if let Ok(config_dir) = std::env::var("ITERATE_CONFIG_DIR") {
        let trimmed_config_dir = config_dir.trim();
        if !trimmed_config_dir.is_empty() {
            return format!("standalone:{}", trimmed_config_dir);
        }
    }

    if std::env::var("ITERATE_STANDALONE_MODE").is_ok() {
        return format!("standalone:pid-{}", pid);
    }

    format!("Unknown:pid-{}", pid)
}

fn normalize_optional_trimmed(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn truncate_request_title(title: &str) -> String {
    title.trim().chars().take(80).collect()
}

/// 注册表是各进程共享、频繁读写的文件，全文设上限，免得几十个窗口把它撑大。
fn truncate_request_text(text: &str) -> String {
    text.trim().chars().take(10_000).collect()
}

/// 全文另存一份 `<注册表名>_texts/<pid>.txt`：还在运行的旧版进程改写注册表时
/// 不认识 `request_text`，会把它丢掉；这份旁路文件旧进程不会碰。
fn request_text_dir(registry_path: &Path) -> PathBuf {
    let stem = registry_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("iterate_windows");
    registry_path.with_file_name(format!("{stem}_texts"))
}

fn write_request_text(registry_path: &Path, pid: u32, text: Option<&str>) {
    let dir = request_text_dir(registry_path);
    let file = dir.join(format!("{pid}.txt"));
    let result = match text {
        Some(text) => fs::create_dir_all(&dir).and_then(|_| fs::write(&file, text)),
        None => match fs::remove_file(&file) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        },
    };
    if let Err(error) = result {
        log_important!(warn, "写入窗口全文失败: PID={}, {}", pid, error);
    }
}

fn read_request_text(registry_path: &Path, pid: u32) -> Option<String> {
    fs::read_to_string(request_text_dir(registry_path).join(format!("{pid}.txt")))
        .ok()
        .filter(|text| !text.trim().is_empty())
}

/// 窗口已不在注册表里（进程退出、被清理）的旁路文件顺手删掉。
fn prune_request_texts(registry_path: &Path, instances: &[WindowInstance]) {
    let Ok(entries) = fs::read_dir(request_text_dir(registry_path)) else {
        return;
    };
    let live = instances.iter().map(|instance| instance.pid).collect::<HashSet<_>>();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid = name
            .to_str()
            .and_then(|name| name.strip_suffix(".txt"))
            .and_then(|pid| pid.parse::<u32>().ok());
        if pid.is_some_and(|pid| !live.contains(&pid)) {
            let _ = fs::remove_file(entry.path());
        }
    }
}

fn is_process_running(pid: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::CloseHandle;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };
        const STILL_ACTIVE_EXIT_CODE: u32 = 259;

        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() {
                return false;
            }

            let mut exit_code = 0u32;
            let running = GetExitCodeProcess(handle, &mut exit_code) != 0
                && exit_code == STILL_ACTIVE_EXIT_CODE;
            let _ = CloseHandle(handle);
            running
        }
    }

    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{}", pid)).exists()
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = pid;
        false
    }
}

pub(crate) fn command_serve_port(command: &str) -> Option<u16> {
    let mut has_serve = false;
    let mut port = None;
    let mut parts = command.split_whitespace().peekable();

    while let Some(part) = parts.next() {
        if part == "--serve" {
            has_serve = true;
        } else if part == "--port" {
            port = parts.next().and_then(|value| value.parse::<u16>().ok());
        } else if let Some(value) = part.strip_prefix("--port=") {
            port = value.parse::<u16>().ok();
        }
    }

    has_serve.then_some(port).flatten()
}

pub(crate) fn command_chain_serve_port<'a>(
    commands: impl IntoIterator<Item = &'a str>,
) -> Option<u16> {
    commands.into_iter().find_map(command_serve_port)
}

pub(crate) fn serve_port_for_pid(pid: u32) -> Option<u16> {
    let mut current_pid = Some(pid);
    for _ in 0..4 {
        let pid = current_pid?;
        if let Some(port) = process_command_line_for_pid(pid)
            .and_then(|command| command_chain_serve_port([command.as_str()]))
        {
            return Some(port);
        }
        current_pid = process_parent_pid(pid).filter(|parent_pid| *parent_pid != pid);
    }
    None
}

#[cfg(target_os = "macos")]
fn process_command_line_for_pid(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "command="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let command = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!command.is_empty()).then_some(command)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn process_parent_pid(pid: u32) -> Option<u32> {
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "ppid="])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

#[cfg(target_os = "linux")]
fn process_command_line_for_pid(pid: u32) -> Option<String> {
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let command = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .filter_map(|part| std::str::from_utf8(part).ok())
        .collect::<Vec<_>>()
        .join(" ");
    (!command.is_empty()).then_some(command)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_command_line_for_pid(_pid: u32) -> Option<String> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn process_parent_pid(_pid: u32) -> Option<u32> {
    None
}

const FOCUS_REQUEST_TTL_MS: u128 = 5_000;

fn focus_request_path() -> PathBuf {
    std::env::temp_dir().join("iterate_focus_request.json")
}

fn unix_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis())
        .unwrap_or_default()
}

/// 每个弹窗进程都在 200ms 的原生 tick 里查看这份请求，命中自己就把窗口拉到前台。
fn write_focus_request(pid: u32) -> Result<(), String> {
    let content = serde_json::json!({ "pid": pid, "requested_at_ms": unix_millis() as u64 });
    fs::write(focus_request_path(), content.to_string())
        .map_err(|error| format!("写入聚焦请求失败: {}", error))
}

pub fn take_focus_request_for_current_process() -> bool {
    let path = focus_request_path();
    let Ok(content) = fs::read_to_string(&path) else {
        return false;
    };
    let Ok(request) = serde_json::from_str::<serde_json::Value>(&content) else {
        let _ = fs::remove_file(&path);
        return false;
    };
    let pid = request.get("pid").and_then(|value| value.as_u64());
    let requested_at = request
        .get("requested_at_ms")
        .and_then(|value| value.as_u64())
        .unwrap_or_default() as u128;
    if unix_millis().saturating_sub(requested_at) > FOCUS_REQUEST_TTL_MS {
        let _ = fs::remove_file(&path);
        return false;
    }
    if pid != Some(process::id() as u64) {
        return false;
    }
    let _ = fs::remove_file(&path);
    true
}

/// macOS 14 起激活是协作式的：前台进程要先把激活权让给目标，目标的自我激活才会生效。
/// 必须在主线程调用。
#[cfg(target_os = "macos")]
pub fn yield_activation_to(pid: u32) {
    use objc::runtime::{Object, BOOL, NO};
    use objc::{class, msg_send, sel, sel_impl};
    unsafe {
        let application: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let target: *mut Object = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid as i32
        ];
        if application.is_null() || target.is_null() {
            return;
        }
        let supported: BOOL =
            msg_send![application, respondsToSelector: sel!(yieldActivationToApplication:)];
        if supported != NO {
            let _: () = msg_send![application, yieldActivationToApplication: target];
        }
    }
}

#[cfg(target_os = "macos")]
fn activate_iterate_process_natively(pid: u32) -> bool {
    use crate::native_speech::target::{
        activate_application, current_bundle_identifier, FrontmostApplication,
    };
    let Some(bundle_id) = current_bundle_identifier() else {
        log_important!(warn, "[DEBUG] 无法读取当前 bundle id，跳过原生激活");
        return false;
    };
    let target = FrontmostApplication {
        bundle_id,
        pid: pid as i32,
    };
    match activate_application(&target) {
        Ok(()) => true,
        Err(error) => {
            log_important!(warn, "[DEBUG] 原生激活失败: {}", error);
            false
        }
    }
}

pub fn activate_window(pid: u32) -> Result<(), String> {
    log_important!(info, "[DEBUG] activate_window 被调用，目标 PID: {}", pid);
    log_important!(info, "[DEBUG] 当前进程 PID: {}", process::id());

    #[cfg(target_os = "macos")]
    {
        use std::process::Command;
        log_important!(info, "[DEBUG] 开始执行 macOS 窗口激活");

        // 方法1: 使用 open 命令通过 bundle ID 和 PID 激活
        // 先尝试使用 kill -0 确认进程存在
        let check = Command::new("kill").args(["-0", &pid.to_string()]).output();

        if check.is_err() || !check.unwrap().status.success() {
            return Err(format!("进程 {} 不存在", pid));
        }

        // 先按 PID 原生激活：NSRunningApplication 不需要辅助功能权限，
        // 也不会像 `open -a iterate` 那样落到任意一个 iterate 进程上。
        if let Err(error) = write_focus_request(pid) {
            log_important!(warn, "[DEBUG] {}", error);
        }
        let natively_activated = activate_iterate_process_natively(pid);

        // AppleScript 负责取消最小化并 AXRaise；没有辅助功能权限时会失败，
        // 原生激活已成功就不再走 `open -a` 兜底。
        // 使用 AppleScript 通过 process id 激活（更精确）
        let script = format!(
            r#"
            tell application "System Events"
                set allProcs to every process whose unix id is {}
                if (count of allProcs) > 0 then
                    set targetProc to item 1 of allProcs
                    
                    -- 取消最小化所有窗口
                    tell targetProc
                        repeat with w in windows
                            try
                                set miniaturized of w to false
                            end try
                        end repeat
                    end tell
                    
                    -- 使用 AXRaise 激活窗口
                    tell targetProc
                        try
                            perform action "AXRaise" of window 1
                        end try
                    end tell
                    
                    -- 设置为前台
                    set frontmost of targetProc to true
                end if
            end tell
            "#,
            pid
        );

        log_important!(info, "[DEBUG] 执行 AppleScript，目标 PID: {}", pid);
        let output = Command::new("osascript")
            .args(["-e", &script])
            .output()
            .map_err(|e| format!("激活窗口失败: {}", e))?;

        if !output.status.success() && natively_activated {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log_important!(warn, "[DEBUG] AppleScript 未能取消最小化（已原生激活）: {}", stderr);
        } else if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log_important!(warn, "[DEBUG] AppleScript 警告: {}", stderr);

            // 备用方案: 使用 open 命令
            log_important!(
                warn,
                "[DEBUG] AppleScript 失败后触发 fallback: open -a iterate, current_pid={}, target_pid={}",
                process::id(),
                pid
            );
            let fallback = Command::new("open").args(["-a", "iterate"]).output();
            match fallback {
                Ok(output) => {
                    log_important!(
                        warn,
                        "[DEBUG] fallback open -a iterate 完成: status={:?}, stdout_len={}, stderr_len={}",
                        output.status.code(),
                        output.stdout.len(),
                        output.stderr.len()
                    );
                }
                Err(error) => {
                    log_important!(warn, "[DEBUG] fallback open -a iterate 失败: {}", error);
                }
            }
        } else {
            log_important!(info, "[DEBUG] AppleScript 执行成功");
        }

        Ok(())
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err("暂不支持此平台的窗口激活".to_string())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn registry_keeps_full_message_for_search_while_title_stays_short() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("windows.json");
        let message = format!("第一行标题\n{}\n结尾关键字", "正文".repeat(100));
        super::register_spawned_popup_at_path(
            &path,
            std::process::id(),
            "/tmp/overview-search",
            "serve-search",
            &message,
        )
        .unwrap();
        let instance = &super::load_registry_from_path(&path).instances[0];
        assert_eq!(instance.request_title.as_deref().unwrap().chars().count(), 80);
        assert!(instance.request_text.as_deref().unwrap().ends_with("结尾关键字"));

        // 旧版进程改写注册表时会丢掉 request_text；列表仍能从旁路文件补回全文。
        let mut stripped = super::load_registry_from_path(&path);
        stripped.instances[0].request_text = None;
        std::fs::write(&path, serde_json::to_vec(&stripped).unwrap()).unwrap();
        let listed = super::WindowRegistry::default().get_all_instances_at_path(&path);
        assert!(listed[0].request_text.as_deref().unwrap().ends_with("结尾关键字"));
    }

    #[test]
    fn native_delivery_registers_real_child_identity_without_a_webview() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("windows.json");
        let pid = std::process::id();
        super::register_spawned_popup_at_path(
            &path,
            pid,
            "/tmp/native-delivery",
            "serve-12345",
            "new request",
        )
        .unwrap();
        let registry = super::load_registry_from_path(&path);
        assert_eq!(registry.instances.len(), 1);
        assert_eq!(registry.instances[0].pid, pid);
        assert_eq!(
            registry.instances[0].request_id.as_deref(),
            Some("serve-12345")
        );
        assert_eq!(registry.instances[0].project_path, "/tmp/native-delivery");
        assert!(registry.instances[0].port.is_none());
        assert!(super::register_spawned_popup_at_path(
            &path,
            0,
            "/tmp/native-delivery",
            "serve-12345",
            "new request"
        )
        .is_err());
    }

    use super::{
        command_chain_serve_port, command_serve_port, normalize_project_path,
        register_instance_at_path, WindowRegistry,
    };

    struct EnvGuard {
        key: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, previous }
        }

        fn remove(key: &'static str) -> Self {
            let previous = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, previous }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            if let Some(previous) = self.previous.as_ref() {
                std::env::set_var(self.key, previous);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn keeps_real_project_path() {
        assert_eq!(
            normalize_project_path("/Users/example/cunzhi", 42),
            "/Users/example/cunzhi"
        );
    }

    #[test]
    fn uses_config_dir_for_unknown_instance() {
        let _standalone_guard = EnvGuard::set("ITERATE_STANDALONE_MODE", "1");
        let _config_guard = EnvGuard::set("ITERATE_CONFIG_DIR", "/tmp/iterate-gui-fixed");

        let normalized = normalize_project_path("Unknown", 42);
        if let Ok(current_dir) = std::env::current_dir() {
            if current_dir.is_absolute() {
                assert_eq!(normalized, current_dir.to_string_lossy());
                return;
            }
        }

        assert_eq!(normalized, "standalone:/tmp/iterate-gui-fixed");
    }

    #[test]
    fn avoids_plain_unknown_label() {
        let _standalone_guard = EnvGuard::remove("ITERATE_STANDALONE_MODE");
        let _config_guard = EnvGuard::remove("ITERATE_CONFIG_DIR");

        let normalized = normalize_project_path("Unknown", 42);
        assert!(std::path::Path::new(&normalized).is_absolute() || normalized == "Unknown:pid-42");
    }

    #[test]
    fn prefers_current_dir_before_standalone_label() {
        let _standalone_guard = EnvGuard::set("ITERATE_STANDALONE_MODE", "1");
        let _config_guard = EnvGuard::set("ITERATE_CONFIG_DIR", "/tmp/iterate-gui-fixed");

        let expected_prefix = std::env::current_dir().ok();
        let normalized = normalize_project_path("Unknown", 42);

        if let Some(current_dir) = expected_prefix {
            if current_dir.is_absolute() {
                assert_eq!(normalized, current_dir.to_string_lossy());
                return;
            }
        }

        assert_eq!(normalized, "standalone:/tmp/iterate-gui-fixed");
    }

    #[test]
    fn command_serve_port_reads_space_and_equals_forms() {
        assert_eq!(
            command_serve_port(
                "/Applications/iterate.app/Contents/MacOS/iterate --serve --port 5311 --workspace /tmp/a"
            ),
            Some(5311)
        );
        assert_eq!(
            command_serve_port(
                "/Applications/iterate.app/Contents/MacOS/iterate --serve --port=5312 --workspace /tmp/a"
            ),
            Some(5312)
        );
    }

    #[test]
    fn command_serve_port_ignores_non_serve_commands() {
        assert_eq!(
            command_serve_port(
                "/Applications/iterate.app/Contents/MacOS/iterate --bridge-only --port 8080"
            ),
            None
        );
    }

    #[test]
    fn command_chain_serve_port_reads_parent_serve_command() {
        assert_eq!(
            command_chain_serve_port([
                "/Applications/iterate.app/Contents/MacOS/iterate",
                "/Applications/iterate.app/Contents/MacOS/iterate --serve --port 5313 --workspace /tmp/a",
            ]),
            Some(5313)
        );
    }

    #[test]
    fn get_all_instances_preserves_cached_port_on_read_path() {
        let temp_dir = tempfile::tempdir().expect("temp registry directory");
        let registry_path = temp_dir.path().join("iterate_windows.json");
        let pid = std::process::id();
        let mut registry = super::WindowRegistry {
            instances: vec![super::WindowInstance {
                pid,
                project_path: "/tmp/cunzhi".to_string(),
                window_title: "iterate - /tmp/cunzhi".to_string(),
                registered_at: chrono::Utc::now().to_rfc3339(),
                port: Some(5311),
                request_id: Some("req-active".to_string()),
                request_title: Some("active".to_string()),
                request_text: None,
            }],
            ..WindowRegistry::default()
        };
        super::save_registry_atomically(&registry_path, &registry)
            .expect("seed isolated window registry");

        let instances = registry.get_all_instances_at_path(&registry_path);

        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].port, Some(5311));
    }

    #[test]
    fn old_registry_json_defaults_focus_timestamps() {
        let registry: WindowRegistry =
            serde_json::from_str(r#"{"instances":[]}"#).expect("parse old registry format");

        assert!(registry.last_focused_at_by_pid().is_empty());
    }

    #[test]
    fn focused_timestamp_updates_only_a_registered_pid() {
        let temp_dir = tempfile::tempdir().expect("temp registry directory");
        let registry_path = temp_dir.path().join("iterate_windows.json");
        let pid = std::process::id();
        register_instance_at_path(
            &registry_path,
            pid,
            "/tmp/focused",
            Some("req-focused"),
            Some("focused"),
            Some(5311),
        )
        .expect("register focused window");

        let focused_at = "2040-08-11T16:40:00Z".to_string();
        let (registry, updated) =
            super::mark_instance_focused_at_path(&registry_path, pid, focused_at.clone())
                .expect("mark registered window focused");
        assert!(updated);
        assert_eq!(
            registry.last_focused_at_by_pid().get(&pid),
            Some(&focused_at)
        );

        let (registry, updated) = super::mark_instance_focused_at_path(
            &registry_path,
            u32::MAX,
            "2040-08-11T16:41:00Z".to_string(),
        )
        .expect("ignore missing window");
        assert!(!updated);
        assert!(!registry.last_focused_at_by_pid().contains_key(&u32::MAX));
    }

    #[test]
    fn stale_window_cleanup_removes_its_focus_timestamp() {
        let live_pid = std::process::id();
        let stale_pid = u32::MAX;
        let mut registry = WindowRegistry {
            instances: vec![
                super::WindowInstance {
                    pid: live_pid,
                    project_path: "/tmp/live".to_string(),
                    window_title: "iterate — /tmp/live".to_string(),
                    registered_at: chrono::Utc::now().to_rfc3339(),
                    port: Some(5311),
                    request_id: Some("req-live".to_string()),
                    request_title: Some("live".to_string()),
                    request_text: None,
                },
                super::WindowInstance {
                    pid: stale_pid,
                    project_path: "/tmp/stale".to_string(),
                    window_title: "iterate — /tmp/stale".to_string(),
                    registered_at: chrono::Utc::now().to_rfc3339(),
                    port: Some(5312),
                    request_id: Some("req-stale".to_string()),
                    request_title: Some("stale".to_string()),
                    request_text: None,
                },
            ],
            last_focused_at_by_pid: [
                (live_pid, "2040-08-11T16:40:00Z".to_string()),
                (stale_pid, "2040-08-11T16:41:00Z".to_string()),
            ]
            .into_iter()
            .collect(),
        };

        registry.cleanup_stale_instances();

        assert!(registry.last_focused_at_by_pid().contains_key(&live_pid));
        assert!(!registry.last_focused_at_by_pid().contains_key(&stale_pid));
    }

    #[test]
    fn locked_registrations_preserve_other_live_processes() {
        let temp_dir = tempfile::tempdir().expect("temp registry directory");
        let registry_path = std::sync::Arc::new(temp_dir.path().join("iterate_windows.json"));
        let mut child = std::process::Command::new(
            std::env::current_exe().expect("resolve current test executable"),
        )
        .args([
            "--ignored",
            "--exact",
            "window_registry::tests::window_registry_sleep_helper",
        ])
        .spawn()
        .expect("spawn second live popup process");

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let first_path = std::sync::Arc::clone(&registry_path);
        let first_barrier = std::sync::Arc::clone(&barrier);
        let first = std::thread::spawn(move || {
            first_barrier.wait();
            register_instance_at_path(
                &first_path,
                std::process::id(),
                "/tmp/project-a",
                Some("req-a"),
                Some("project a"),
                Some(5311),
            )
        });
        let second_path = std::sync::Arc::clone(&registry_path);
        let second_barrier = std::sync::Arc::clone(&barrier);
        let second_pid = child.id();
        let second = std::thread::spawn(move || {
            second_barrier.wait();
            register_instance_at_path(
                &second_path,
                second_pid,
                "/tmp/project-b",
                Some("req-b"),
                Some("project b"),
                Some(5312),
            )
        });
        barrier.wait();
        first
            .join()
            .expect("join first popup")
            .expect("register first popup");
        second
            .join()
            .expect("join second popup")
            .expect("register second popup");

        let registry: WindowRegistry = serde_json::from_str(
            &std::fs::read_to_string(registry_path.as_ref()).expect("read registry"),
        )
        .expect("parse registry");
        let mut paths = registry
            .instances
            .iter()
            .map(|instance| instance.project_path.as_str())
            .collect::<Vec<_>>();
        paths.sort_unstable();

        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(paths, vec!["/tmp/project-a", "/tmp/project-b"]);
    }

    #[test]
    #[ignore = "subprocess helper for the cross-process registry test"]
    fn window_registry_sleep_helper() {
        std::thread::sleep(std::time::Duration::from_secs(5));
    }
}
