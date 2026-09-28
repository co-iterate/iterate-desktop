use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

const LEASE_FILE_NAME: &str = "codex-live-audio.lock";

pub(super) struct AudioLeaseGuard {
    _file: std::fs::File,
}

pub(super) fn try_acquire() -> Result<Option<AudioLeaseGuard>, String> {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    try_acquire_at(&PathBuf::from(home).join(".cunzhi").join(LEASE_FILE_NAME))
}

#[cfg(unix)]
fn try_acquire_at(path: &Path) -> Result<Option<AudioLeaseGuard>, String> {
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::io::AsRawFd;

    let parent = path
        .parent()
        .ok_or_else(|| "Codex GPT-Live 音频锁路径无效".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("创建 Codex GPT-Live 音频锁目录失败: {error}"))?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("打开 Codex GPT-Live 音频锁失败: {error}"))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("设置 Codex GPT-Live 音频锁权限失败: {error}"))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(Some(AudioLeaseGuard { _file: file }));
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(None);
    }
    Err(format!("获取 Codex GPT-Live 跨进程音频锁失败: {error}"))
}

#[cfg(not(unix))]
fn try_acquire_at(_path: &Path) -> Result<Option<AudioLeaseGuard>, String> {
    Err("当前系统不支持 Codex GPT-Live 跨进程音频锁".to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[cfg(unix)]
    #[test]
    fn lease_is_exclusive_until_guard_drops() {
        let dir = tempdir().unwrap();
        let path = dir.path().join(LEASE_FILE_NAME);
        let first = try_acquire_at(&path).unwrap().unwrap();
        assert!(try_acquire_at(&path).unwrap().is_none());
        drop(first);
        assert!(try_acquire_at(&path).unwrap().is_some());
    }
}
