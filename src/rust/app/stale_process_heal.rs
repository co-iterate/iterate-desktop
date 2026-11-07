//! Self-heal after an in-place app update (P-2026-9354, P-2026-9365).
//!
//! Processes started before an update keep executing the replaced image. Their
//! code-signature checks then fail against the new files on disk: the old
//! canonical GUI keeps the Fn owner lock but its paste helper is rejected with
//! `cdhash_mismatch`, and old `--serve` processes cannot obtain bridge tokens,
//! so APNs pushes fail. The updater of the previous version cannot fix this,
//! so the new canonical GUI hands over at startup:
//! - the stale canonical GUI that holds the Fn owner lock is asked to exit;
//! - a stale relay client is restarted, only when its LaunchAgent runs that
//!   exact process.
//!
//! Serves are never stopped: "no popup child" does not prove a serve is idle
//! (a request may be registering before its popup starts), and they exit on
//! their own after 30 idle minutes. Popups and the bridge are never touched.
//! A process counts as stale only when its mapped executable is this
//! installation (or its retired copy) with a different device + inode, and it
//! started before this process; another installation is never a target.

#[cfg(target_os = "macos")]
mod imp {
    use std::os::unix::fs::MetadataExt;
    use std::process::Command;
    use std::time::Duration;

    const RELAY_LAUNCH_AGENT: &str = "com.cunzhi.iterate.relay-mac-client";
    const EXECUTABLE_SUFFIX: &str = "/Contents/MacOS/iterate";

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(super) struct ImageId {
        pub dev: u64,
        pub inode: u64,
    }

    /// Parse `lsof -F Din` output and return the mapped app executable image
    /// and its path.
    pub(super) fn parse_lsof_image(output: &str) -> Option<(ImageId, String)> {
        let mut dev = None;
        let mut inode = None;
        for line in output.lines() {
            let (tag, value) = line.split_at(line.len().min(1));
            match tag {
                "f" => {
                    dev = None;
                    inode = None;
                }
                "D" => dev = u64::from_str_radix(value.trim_start_matches("0x"), 16).ok(),
                "i" => inode = value.parse().ok(),
                "n" if value.ends_with(EXECUTABLE_SUFFIX) => {
                    return Some((ImageId { dev: dev?, inode: inode? }, value.to_string()));
                }
                _ => {}
            }
        }
        None
    }

    /// The mapped executable belongs to this installation: its current path or a
    /// retired copy kept beside it by the updater/installer.
    pub(super) fn belongs_to_installation(mapped_path: &str, bundle_executable: &str) -> bool {
        if mapped_path == bundle_executable {
            return true;
        }
        let Some(install_dir) = bundle_executable
            .strip_suffix(EXECUTABLE_SUFFIX)
            .and_then(|bundle| bundle.rsplit_once('/'))
            .map(|(dir, _)| dir)
        else {
            return false;
        };
        mapped_path.starts_with(&format!("{install_dir}/.iterate-retired/"))
            && mapped_path.ends_with(EXECUTABLE_SUFFIX)
    }

    fn process_image(pid: u32) -> Option<(ImageId, String)> {
        let output = Command::new("/usr/sbin/lsof")
            .args(["-a", "-p", &pid.to_string(), "-d", "txt", "-F", "Din"])
            .output()
            .ok()?;
        parse_lsof_image(&String::from_utf8_lossy(&output.stdout))
    }

    fn is_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    fn terminate(pid: u32, reason: &str) {
        log::info!("[stale-heal] SIGTERM pid={pid} reason={reason}");
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGTERM);
        }
        for _ in 0..20 {
            if !is_alive(pid) {
                return;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        log::warn!("[stale-heal] pid={pid} still running after SIGTERM; left alone");
    }

    fn fn_owner_canonical_pid() -> Option<u32> {
        let path = crate::native_speech::runtime_paths::production_owner_lock_path().ok()?;
        let value: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
        (value.get("role")?.as_str()? == "canonical-gui")
            .then(|| value.get("pid")?.as_u64())
            .flatten()
            .and_then(|pid| u32::try_from(pid).ok())
    }

    fn start_time(pid: u32) -> Option<std::time::SystemTime> {
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let read = unsafe {
            libc::proc_pidinfo(
                pid as libc::c_int,
                libc::PROC_PIDTBSDINFO,
                0,
                &mut info as *mut _ as *mut libc::c_void,
                size,
            )
        };
        (read == size).then(|| {
            std::time::UNIX_EPOCH
                + Duration::from_secs(info.pbi_start_tvsec)
                + Duration::from_micros(info.pbi_start_tvusec)
        })
    }

    pub fn run() {
        let Ok(executable) = std::env::current_exe() else { return };
        let Ok(metadata) = std::fs::metadata(&executable) else { return };
        let current = ImageId { dev: metadata.dev(), inode: metadata.ino() };
        let bundle_executable = executable.to_string_lossy().to_string();
        let own_pid = std::process::id();
        let Some(own_start) = start_time(own_pid) else { return };
        // Stale = this installation, replaced image, and started before us
        // (guards against PID reuse and against other installations).
        let is_stale = |pid: u32| {
            process_image(pid).is_some_and(|(image, path)| {
                image != current && belongs_to_installation(&path, &bundle_executable)
            }) && start_time(pid).is_some_and(|started| started < own_start)
        };

        if let Some(pid) = fn_owner_canonical_pid().filter(|pid| *pid != own_pid) {
            if is_alive(pid) && is_stale(pid) {
                terminate(pid, "stale canonical GUI holds the Fn owner lock");
            }
        }

        if let Some(pid) = relay_agent_pid() {
            if is_stale(pid) {
                restart_relay();
            }
        }
    }

    /// PID of the relay process launchd runs for the LaunchAgent, if loaded.
    fn relay_agent_pid() -> Option<u32> {
        let target = format!("gui/{}/{}", unsafe { libc::geteuid() }, RELAY_LAUNCH_AGENT);
        let output = Command::new("/bin/launchctl").args(["print", &target]).output().ok()?;
        if !output.status.success() {
            return None;
        }
        parse_launchctl_pid(&String::from_utf8_lossy(&output.stdout))
    }

    pub(super) fn parse_launchctl_pid(output: &str) -> Option<u32> {
        output
            .lines()
            .map(str::trim)
            .find_map(|line| line.strip_prefix("pid = "))
            .and_then(|pid| pid.trim().parse().ok())
    }

    fn restart_relay() {
        let target = format!("gui/{}/{}", unsafe { libc::geteuid() }, RELAY_LAUNCH_AGENT);
        log::info!("[stale-heal] restarting relay LaunchAgent {RELAY_LAUNCH_AGENT}");
        let _ = Command::new("/bin/launchctl").args(["kickstart", "-k", &target]).status();
    }
}

/// Run once from the canonical GUI after startup; never blocks the caller.
pub fn spawn_after_startup() {
    #[cfg(target_os = "macos")]
    std::thread::spawn(|| {
        // Let the new GUI finish starting before handing over from old processes.
        std::thread::sleep(std::time::Duration::from_secs(3));
        imp::run();
    });
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::imp::*;

    #[test]
    fn only_this_installation_or_its_retired_copies_are_targets() {
        let exe = "/Applications/iterate.app/Contents/MacOS/iterate";
        assert!(belongs_to_installation(exe, exe));
        assert!(belongs_to_installation("/Applications/.iterate-retired/iterate-20261015-1.app/Contents/MacOS/iterate", exe));
        // Another installation of the same app is never touched.
        assert!(!belongs_to_installation("/Users/example/Applications/iterate.app/Contents/MacOS/iterate", exe));
        assert!(!belongs_to_installation("/Users/example/.cunzhi/runtimes/bridge/iterate", exe));
        let spaced = "/Users/example/My Apps/iterate.app/Contents/MacOS/iterate";
        assert!(belongs_to_installation("/Users/example/My Apps/.iterate-retired/x.app/Contents/MacOS/iterate", spaced));
    }

    #[test]
    fn lsof_image_uses_the_app_executable_record() {
        let output = "p42\nftxt\nD0x1000010\ni111\nn/usr/lib/dyld\nftxt\nD0x1000012\ni359073316\nn/Applications/iterate.app/Contents/MacOS/iterate\n";
        assert_eq!(
            parse_lsof_image(output),
            Some((ImageId { dev: 0x1000012, inode: 359073316 }, "/Applications/iterate.app/Contents/MacOS/iterate".to_string()))
        );
        assert_eq!(parse_lsof_image("p42\nftxt\nD0x1\ni1\nn/usr/lib/dyld\n"), None);
    }

    #[test]
    fn relay_pid_comes_from_the_launch_agent_record() {
        let output = "gui/501/com.cunzhi.iterate.relay-mac-client = {\n\tactive count = 1\n\tstate = running\n\tpid = 6052\n}\n";
        assert_eq!(parse_launchctl_pid(output), Some(6052));
        assert_eq!(parse_launchctl_pid("\tstate = not running\n"), None);
    }
}
