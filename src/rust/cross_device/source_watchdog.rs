//! An OS-thread watchdog for the supervised, headless Windows bridge only.
//! Progress measures completed local/network steps, not successful connectivity.
use std::{
    fs, io::Write, path::{Path, PathBuf},
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(super) const RESTART_EXIT_CODE: i32 = 75;

pub(super) fn supervised_bridge(windows: bool, args: impl IntoIterator<Item = String>) -> bool {
    let args: Vec<_> = args.into_iter().skip(1).collect();
    windows && args.iter().any(|arg| arg == "--bridge-only")
        && !args.iter().any(|arg| matches!(arg.as_str(),
            "--hub-source" | "--hub-source-configure" | "--serve" | "--relay-server"
            | "--relay-mac-client" | "--cross-device-daemon"))
}

#[derive(Clone)]
pub(super) struct Progress(Arc<Mutex<(Instant, &'static str)>>);

impl Progress {
    pub(super) fn new() -> Self {
        Self(Arc::new(Mutex::new((Instant::now(), "source_start"))))
    }

    pub(super) fn enter(&self, stage: &'static str) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = (Instant::now(), stage);
    }
}

// No request bodies, credentials, URLs or user-supplied errors enter this file.
pub(super) fn record_incident(dir: &Path, stage: &'static str, elapsed: Duration) {
    let at = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    let body = format!(
        "{{\"at\":{at},\"pid\":{},\"stage\":\"{stage}\",\"stalled_seconds\":{},\"exit_code\":{RESTART_EXIT_CODE}}}\n",
        std::process::id(), elapsed.as_secs(),
    );
    let _ = fs::create_dir_all(dir);
    if let Ok(mut file) = fs::File::create(dir.join("hub-source-watchdog.json")) {
        let _ = file.write_all(body.as_bytes());
        let _ = file.sync_all();
    }
}

pub(super) struct Watchdog {
    // Dropping the owner stops the thread, including runtime/task cancellation.
    _stop: mpsc::Sender<()>,
}

impl Watchdog {
    pub(super) fn start(dir: PathBuf, progress: Progress, limit: Duration, interval: Duration) -> std::io::Result<Self> {
        let (stop, receiver) = mpsc::channel();
        std::thread::Builder::new().name("hub-source-watchdog".into()).spawn(move || {
            while let Err(mpsc::RecvTimeoutError::Timeout) = receiver.recv_timeout(interval) {
                let (last, stage) = *progress.0.lock().unwrap_or_else(|e| e.into_inner());
                let elapsed = last.elapsed();
                if elapsed >= limit {
                    record_incident(&dir, stage, elapsed);
                    // This thread does not need the Tokio runtime to be responsive.
                    std::process::exit(RESTART_EXIT_CODE);
                }
            }
        })?;
        Ok(Self { _stop: stop })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_windows_headless_bridge_is_supervised() {
        for (windows, args, expected) in [
            (true, vec!["iterate", "--bridge-only", "--port", "8080"], true),
            (false, vec!["iterate", "--bridge-only"], false),
            (true, vec!["iterate"], false),
            (true, vec!["iterate", "--serve"], false),
            (true, vec!["iterate", "--hub-source"], false),
            (true, vec!["iterate", "--hub-source", "--bridge-only"], false),
            (true, vec!["iterate", "--serve", "--bridge-only"], false),
            (true, vec!["--bridge-only"], false),
        ] {
            assert_eq!(supervised_bridge(windows, args.into_iter().map(String::from)), expected);
        }
    }

    #[test]
    fn watchdog_child_fixture() {
        let Ok(mode) = std::env::var("ITERATE_WATCHDOG_FIXTURE") else { return; };
        let dir = PathBuf::from(std::env::var_os("ITERATE_WATCHDOG_FIXTURE_DIR").unwrap());
        let progress = Progress::new();
        let guard = Watchdog::start(dir, progress.clone(), Duration::from_millis(300), Duration::from_millis(20)).unwrap();
        match mode.as_str() {
            "blocked" => {
                progress.enter("action_commit");
                std::thread::sleep(Duration::from_secs(5));
                panic!("watchdog failed to terminate blocked process");
            }
            "network_errors" => {
                for _ in 0..20 {
                    progress.enter("poll_failed");
                    std::thread::sleep(Duration::from_millis(40));
                }
                drop(guard);
            }
            "cancelled" => { drop(guard); std::thread::sleep(Duration::from_millis(600)); }
            _ => panic!("invalid fixture"),
        }
    }

    #[test]
    fn real_process_stall_exits_but_network_retries_and_cancellation_do_not() {
        let root = std::env::temp_dir().join(format!("iterate-watchdog-test-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        // Works both as a standalone rustc test and within the crate's tests.
        for (mode, code) in [("blocked", RESTART_EXIT_CODE), ("network_errors", 0), ("cancelled", 0)] {
            let dir = root.join(mode);
            let status = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["watchdog_child_fixture", "--nocapture", "--test-threads=1"])
                .env("ITERATE_WATCHDOG_FIXTURE", mode).env("ITERATE_WATCHDOG_FIXTURE_DIR", &dir)
                .status().unwrap();
            assert_eq!(status.code(), Some(code), "mode={mode}");
            let incident = dir.join("hub-source-watchdog.json");
            if mode == "blocked" {
                let record = fs::read_to_string(incident).unwrap();
                assert!(record.contains("\"stage\":\"action_commit\""));
                assert!(record.contains("\"exit_code\":75"));
            } else { assert!(!incident.exists()); }
        }
        fs::remove_dir_all(root).unwrap();
    }
}
