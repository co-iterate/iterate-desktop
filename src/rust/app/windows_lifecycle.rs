use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const MANUALLY_STOPPED_MESSAGE: &str =
    "iterate 已关闭，请通过桌面或开始菜单的 iterate 快捷方式启动";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstanceRecord {
    pid: u32,
    executable_path: PathBuf,
    role: String,
    port: Option<u16>,
    started_at_ms: i64,
    #[serde(default)]
    creation_time_100ns: u64,
}

#[derive(Debug)]
pub struct InstanceGuard {
    path: Option<PathBuf>,
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TerminationSummary {
    pub terminated: usize,
    pub stale_removed: usize,
    pub rejected: usize,
}

fn runtime_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("iterate")
        .join("runtime")
}

fn manual_stop_path() -> PathBuf {
    runtime_dir().join("manually-stopped")
}

fn instance_dir() -> PathBuf {
    runtime_dir().join("instances")
}

fn instance_path(pid: u32) -> PathBuf {
    instance_dir().join(format!("{pid}.json"))
}

fn canonical_or_original(path: PathBuf) -> PathBuf {
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn atomic_write(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, contents)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error);
    }
    Ok(())
}

pub fn is_manually_stopped() -> bool {
    manual_stop_path().is_file()
}

pub fn register_current_instance(role: &str, port: Option<u16>) -> anyhow::Result<InstanceGuard> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (role, port);
        return Ok(InstanceGuard { path: None });
    }

    #[cfg(target_os = "windows")]
    {
        let directory = instance_dir();
        std::fs::create_dir_all(&directory)?;
        let record = InstanceRecord {
            pid: std::process::id(),
            executable_path: canonical_or_original(std::env::current_exe()?),
            role: role.to_string(),
            port,
            started_at_ms: chrono::Utc::now().timestamp_millis(),
            creation_time_100ns: platform::current_process_creation_time()
                .ok_or_else(|| anyhow::anyhow!("无法读取当前 iterate 进程的 Windows 创建时间"))?,
        };
        let path = instance_path(record.pid);
        let contents = serde_json::to_vec(&record)?;
        let _ = std::fs::remove_file(&path);
        atomic_write(&path, &contents)?;
        Ok(InstanceGuard { path: Some(path) })
    }
}

pub fn activate_manual_launch_if_requested(args: &[String]) -> anyhow::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = args;
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        if args.len() != 1 {
            return Ok(());
        }

        reactivate_after_manual_stop()
    }
}

/// Allow an explicit foreground MCP request to summon iterate again after the
/// user closed every window. Closing still stops the current processes; the
/// next interaction is the new explicit activation signal.
pub fn activate_mcp_launch() -> anyhow::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        reactivate_after_manual_stop()
    }
}

#[cfg(target_os = "windows")]
fn reactivate_after_manual_stop() -> anyhow::Result<()> {
    if is_manually_stopped() {
        let _ = terminate_registered_instances(Duration::ZERO, None);
    }
    reset_shutdown_event()?;
    match std::fs::remove_file(manual_stop_path()) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub fn request_global_shutdown() -> anyhow::Result<()> {
    #[cfg(not(target_os = "windows"))]
    {
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        std::fs::create_dir_all(runtime_dir())?;
        let marker = manual_stop_path();
        if !marker.is_file() {
            atomic_write(&marker, b"manual-close\n")?;
        }
        signal_shutdown_event()
    }
}

pub async fn wait_for_global_shutdown() {
    #[cfg(not(target_os = "windows"))]
    futures::future::pending::<()>().await;

    #[cfg(target_os = "windows")]
    {
        let _ = tokio::task::spawn_blocking(wait_for_shutdown_event).await;
    }
}

#[cfg(target_os = "windows")]
pub fn start_tauri_shutdown_listener(app: tauri::AppHandle) {
    let _ = std::thread::Builder::new()
        .name("iterate-shutdown-listener".to_string())
        .spawn(move || {
            if wait_for_shutdown_event().is_ok() {
                tauri::async_runtime::spawn(async move {
                    let _ = crate::ui::exit::force_exit_app(app).await;
                });
            }
        });
}

#[cfg(not(target_os = "windows"))]
pub fn start_tauri_shutdown_listener(_app: tauri::AppHandle) {}

pub fn terminate_registered_instances(
    grace_period: Duration,
    exclude_pid: Option<u32>,
) -> TerminationSummary {
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (grace_period, exclude_pid);
        TerminationSummary::default()
    }

    #[cfg(target_os = "windows")]
    {
        platform::terminate_registered_instances(grace_period, exclude_pid)
    }
}

#[cfg(target_os = "windows")]
fn event_name() -> Vec<u16> {
    "Local\\com.kexin94yyds.iterate.shutdown-all"
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(target_os = "windows")]
fn create_shutdown_event() -> anyhow::Result<windows_sys::Win32::Foundation::HANDLE> {
    use windows_sys::Win32::System::Threading::CreateEventW;

    let name = event_name();
    let handle = unsafe { CreateEventW(std::ptr::null(), 1, 0, name.as_ptr()) };
    if handle.is_null() {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(handle)
    }
}

#[cfg(target_os = "windows")]
fn signal_shutdown_event() -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::SetEvent;

    let handle = create_shutdown_event()?;
    let result = unsafe { SetEvent(handle) };
    unsafe { CloseHandle(handle) };
    if result == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn reset_shutdown_event() -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::ResetEvent;

    let handle = create_shutdown_event()?;
    let result = unsafe { ResetEvent(handle) };
    unsafe { CloseHandle(handle) };
    if result == 0 {
        Err(std::io::Error::last_os_error().into())
    } else {
        Ok(())
    }
}

#[cfg(target_os = "windows")]
fn wait_for_shutdown_event() -> anyhow::Result<()> {
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};

    let handle = create_shutdown_event()?;
    let result = unsafe { WaitForSingleObject(handle, INFINITE) };
    unsafe { CloseHandle(handle) };
    if result == WAIT_OBJECT_0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error().into())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
    use windows_sys::Win32::Security::{
        GetLengthSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, OpenProcessToken,
        QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };

    unsafe fn process_user_sid(process: windows_sys::Win32::Foundation::HANDLE) -> Option<Vec<u8>> {
        let mut token = std::ptr::null_mut();
        if OpenProcessToken(process, TOKEN_QUERY, &mut token) == 0 {
            return None;
        }

        let mut required = 0u32;
        let _ = GetTokenInformation(token, TokenUser, std::ptr::null_mut(), 0, &mut required);
        if required == 0 {
            CloseHandle(token);
            return None;
        }

        let mut buffer = vec![0u8; required as usize];
        let success = GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            required,
            &mut required,
        );
        if success == 0 {
            CloseHandle(token);
            return None;
        }

        let token_user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
        let sid_length = GetLengthSid(token_user.User.Sid) as usize;
        let sid = if sid_length == 0 {
            None
        } else {
            Some(std::slice::from_raw_parts(token_user.User.Sid.cast::<u8>(), sid_length).to_vec())
        };
        CloseHandle(token);
        sid
    }

    fn belongs_to_current_user(handle: windows_sys::Win32::Foundation::HANDLE) -> bool {
        unsafe { process_user_sid(handle) == process_user_sid(GetCurrentProcess()) }
    }

    unsafe fn process_creation_time(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
        let mut creation = std::mem::zeroed();
        let mut exit = std::mem::zeroed();
        let mut kernel = std::mem::zeroed();
        let mut user = std::mem::zeroed();
        if GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) == 0 {
            return None;
        }
        Some(((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64)
    }

    pub(super) fn current_process_creation_time() -> Option<u64> {
        unsafe { process_creation_time(GetCurrentProcess()) }
    }

    fn normalized_path(path: &Path) -> String {
        let canonical = canonical_or_original(path.to_path_buf());
        canonical
            .to_string_lossy()
            .trim_start_matches(r"\\?\")
            .replace('/', "\\")
            .to_lowercase()
    }

    fn has_iterate_executable_name(path: &Path) -> bool {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("iterate.exe"))
    }

    unsafe fn query_process_path(
        handle: windows_sys::Win32::Foundation::HANDLE,
    ) -> Option<PathBuf> {
        let mut buffer = vec![0u16; 32768];
        let mut size = buffer.len() as u32;
        if QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) == 0 {
            return None;
        }
        buffer.truncate(size as usize);
        Some(PathBuf::from(String::from_utf16_lossy(&buffer)))
    }

    fn read_records() -> Vec<(PathBuf, InstanceRecord)> {
        let Ok(entries) = std::fs::read_dir(instance_dir()) else {
            return Vec::new();
        };

        entries
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let bytes = std::fs::read(&path).ok()?;
                let record = serde_json::from_slice::<InstanceRecord>(&bytes).ok()?;
                Some((path, record))
            })
            .collect()
    }

    fn cleanup_port_file(port: Option<u16>) {
        let Some(port) = port else {
            return;
        };
        if let Some(home) = dirs::home_dir() {
            let _ = std::fs::remove_file(home.join(".cunzhi_ports").join(port.to_string()));
        }
    }

    pub(super) fn terminate_registered_instances(
        grace_period: Duration,
        exclude_pid: Option<u32>,
    ) -> TerminationSummary {
        if !grace_period.is_zero() {
            std::thread::sleep(grace_period);
        }

        let mut summary = TerminationSummary::default();
        for (record_path, record) in read_records() {
            if Some(record.pid) == exclude_pid {
                continue;
            }

            if !has_iterate_executable_name(&record.executable_path)
                || !matches!(
                    record.role.as_str(),
                    "gui" | "popup" | "serve" | "bridge-only"
                )
            {
                summary.rejected += 1;
                continue;
            }

            let access =
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE | PROCESS_TERMINATE;
            let handle = unsafe { OpenProcess(access, 0, record.pid) };
            if handle.is_null() {
                let _ = std::fs::remove_file(&record_path);
                cleanup_port_file(record.port);
                summary.stale_removed += 1;
                continue;
            }

            let actual_path = unsafe { query_process_path(handle) };
            let actual_creation_time = unsafe { process_creation_time(handle) };
            let identity_matches = actual_path.as_ref().is_some_and(|actual| {
                has_iterate_executable_name(actual)
                    && normalized_path(actual) == normalized_path(&record.executable_path)
            }) && record.creation_time_100ns != 0
                && actual_creation_time == Some(record.creation_time_100ns)
                && belongs_to_current_user(handle);

            if !identity_matches {
                unsafe { CloseHandle(handle) };
                summary.rejected += 1;
                continue;
            }

            let running = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
            if running && unsafe { TerminateProcess(handle, 0) } != 0 {
                let _ = unsafe { WaitForSingleObject(handle, 1000) };
                summary.terminated += 1;
            }
            unsafe { CloseHandle(handle) };
            let _ = std::fs::remove_file(&record_path);
            cleanup_port_file(record.port);
        }
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_files_are_kept_outside_the_project_tree() {
        assert!(manual_stop_path().starts_with(runtime_dir()));
        assert!(instance_dir().starts_with(runtime_dir()));
    }

    #[test]
    fn termination_summary_defaults_to_no_mutation() {
        assert_eq!(TerminationSummary::default().terminated, 0);
        assert_eq!(TerminationSummary::default().stale_removed, 0);
        assert_eq!(TerminationSummary::default().rejected, 0);
    }
}
