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
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq)]
pub struct WindowRegistry {
    pub instances: Vec<WindowInstance>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub(crate) last_focused_at_by_pid: HashMap<u32, String>,
}

impl WindowRegistry {
    fn registry_path() -> PathBuf {
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
        let normalized_request_title =
            normalize_optional_trimmed(request_title).map(|title| truncate_request_title(&title));
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

    pub fn unregister(&mut self) -> Result<(), String> {
        let pid = process::id();
        *self = update_registry_at_path(&Self::registry_path(), false, |registry| {
            registry.instances.retain(|instance| instance.pid != pid);
            registry.last_focused_at_by_pid.remove(&pid);
        })?;
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
            }
        })?;
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
        self.instances.clone()
    }

    fn cleanup_stale_instances(&mut self) {
        self.instances
            .retain(|instance| is_process_running(instance.pid, &instance.registered_at));
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
    let request_title =
        normalize_optional_trimmed(request_title).map(|title| truncate_request_title(&title));
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
        } else {
            registry.instances.push(WindowInstance {
                pid,
                project_path: project_path.clone(),
                window_title: format!("iterate — {}", project_path),
                registered_at: chrono::Utc::now().to_rfc3339(),
                port,
                request_id,
                request_title,
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

fn is_process_running(pid: u32, registered_at: &str) -> bool {
    #[cfg(not(target_os = "windows"))]
    let _ = registered_at;
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
        use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
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
            // A live PID can belong to a different process after the original
            // window exits. Old registry entries already carry the registration
            // timestamp, so no data migration or executable-name guess is needed.
            let same_process = if running {
                let mut created: FILETIME = std::mem::zeroed();
                let mut exited: FILETIME = std::mem::zeroed();
                let mut kernel: FILETIME = std::mem::zeroed();
                let mut user: FILETIME = std::mem::zeroed();
                if GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) != 0 {
                    let ticks = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
                    match chrono::DateTime::parse_from_rfc3339(registered_at) {
                        Ok(registered) => {
                            let created_unix_micros = i128::from(ticks / 10) - 11_644_473_600_000_000i128;
                            created_unix_micros <= i128::from(registered.timestamp_micros())
                        }
                        // Keep legacy records whose timestamp cannot establish
                        // process identity; do not close an unproven live request.
                        Err(_) => true,
                    }
                } else {
                    true
                }
            } else {
                false
            };
            let _ = CloseHandle(handle);
            running && same_process
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

        if !output.status.success() {
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
            normalize_project_path("/Users/test/project", 42),
            "/Users/test/project"
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
            }],
            ..WindowRegistry::default()
        };
        super::save_registry_atomically(&registry_path, &registry)
            .expect("seed isolated window registry");

        let instances = registry.get_all_instances_at_path(&registry_path);

        assert_eq!(instances.len(), 1);
        assert_eq!(instances[0].port, Some(5311));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn reused_live_pid_cannot_keep_a_registration_from_before_process_creation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("iterate_windows.json");
        let pid = std::process::id();
        let stale = super::WindowInstance {
            pid,
            project_path: "C:/closed-cli".into(),
            window_title: "closed CLI".into(),
            registered_at: "2000-01-01T00:00:00Z".into(),
            port: None,
            request_id: Some("closed-request".into()),
            request_title: Some("closed".into()),
        };
        let mut registry = super::WindowRegistry {
            instances: vec![stale],
            ..Default::default()
        };
        super::save_registry_atomically(&path, &registry).unwrap();
        assert!(registry.get_all_instances_at_path(&path).is_empty());
        assert!(super::load_registry_from_path(&path).instances.is_empty());
        assert!(super::is_process_running(pid, &chrono::Utc::now().to_rfc3339()));
        assert!(super::is_process_running(pid, "legacy-unparseable"));
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
                },
                super::WindowInstance {
                    pid: stale_pid,
                    project_path: "/tmp/stale".to_string(),
                    window_title: "iterate — /tmp/stale".to_string(),
                    registered_at: chrono::Utc::now().to_rfc3339(),
                    port: Some(5312),
                    request_id: Some("req-stale".to_string()),
                    request_title: Some("stale".to_string()),
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
