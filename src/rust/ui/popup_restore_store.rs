//! Small process-shared handoff record, separate from popup contents and preferences.
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub started: String,
}

impl ProcessIdentity {
    pub fn current() -> io::Result<Self> {
        let pid = std::process::id();
        process_started(pid)
            .map(|started| Self { pid, started })
            .ok_or_else(|| io::Error::other("cannot identify popup process"))
    }
    pub fn alive(&self) -> bool {
        process_started(self.pid).as_deref() == Some(self.started.as_str())
    }
}

#[cfg(target_os = "macos")]
fn process_started(pid: u32) -> Option<String> {
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of_val(&info) as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    (read == size && info.pbi_uid == unsafe { libc::geteuid() })
        .then(|| format!("{}:{}", info.pbi_start_tvsec, info.pbi_start_tvusec))
}

#[cfg(target_os = "linux")]
fn process_started(pid: u32) -> Option<String> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    text.rsplit_once(')')?
        .1
        .split_whitespace()
        .nth(19)
        .map(str::to_owned)
}

#[cfg(target_os = "windows")]
fn process_started(pid: u32) -> Option<String> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut std::ffi::c_void;
        fn GetProcessTimes(
            process: *mut std::ffi::c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn CloseHandle(handle: *mut std::ffi::c_void) -> i32;
    }
    unsafe {
        let handle = OpenProcess(0x1000, 0, pid);
        if handle.is_null() {
            return None;
        }
        let (mut creation, mut exit, mut kernel, mut user) = (
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
            FileTime::default(),
        );
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user);
        CloseHandle(handle);
        (ok != 0).then(|| format!("{}:{}", creation.high, creation.low))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn process_started(_pid: u32) -> Option<String> {
    None
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Owner {
    pub process: ProcessIdentity,
    pub runtime_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Target {
    pub owner: Owner,
    pub window_label: String,
    pub ticket: String,
    pub requested: bool,
    #[serde(default)]
    pub minimized: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub version: u8,
    pub target: Option<Target>,
    pub shortcut_owner: Option<Owner>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: 1,
            target: None,
            shortcut_owner: None,
        }
    }
}

pub fn read(path: &Path) -> io::Result<Snapshot> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Snapshot::default()),
        Err(error) => return Err(error),
    };
    if bytes.len() > 8192 {
        return Err(io::Error::other("oversized popup restore state"));
    }
    let snapshot: Snapshot = serde_json::from_slice(&bytes)?;
    if snapshot.version != 1 {
        return Err(io::Error::other("unsupported popup restore state"));
    }
    Ok(snapshot)
}

pub fn edit<T>(path: &Path, action: impl FnOnce(&mut Snapshot) -> T) -> io::Result<T> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("missing parent"))?,
    )?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options.open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut snapshot = read(path)?;
    let value = action(&mut snapshot);
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    staged.write_all(&serde_json::to_vec(&snapshot)?)?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    Ok(value)
}

pub fn clear_ticket(path: &Path, ticket: &str) -> io::Result<()> {
    edit(path, |state| {
        if state
            .target
            .as_ref()
            .is_some_and(|target| target.ticket == ticket)
        {
            state.target = None;
        }
    })
}

pub fn clear_window(path: &Path, owner: &Owner, label: &str) -> io::Result<()> {
    edit(path, |state| {
        if state
            .target
            .as_ref()
            .is_some_and(|target| &target.owner == owner && target.window_label == label)
        {
            state.target = None;
        }
    })
}

pub fn request_latest(path: &Path) -> io::Result<Option<Target>> {
    edit(path, |state| {
        if state
            .target
            .as_ref()
            .is_some_and(|target| !target.owner.process.alive())
        {
            state.target = None;
        }
        if let Some(target) = &mut state.target {
            target.requested = true;
        }
        state.target.clone()
    })
}

pub fn targets_window(target: &Target, owner: &Owner, enabled: bool) -> bool {
    target.requested && target.minimized && &target.owner == owner && enabled
}

pub fn mark_minimized(path: &Path, ticket: &str) -> io::Result<()> {
    edit(path, |state| {
        if let Some(target) = state
            .target
            .as_mut()
            .filter(|target| target.ticket == ticket)
        {
            target.minimized = true;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owner(token: &str) -> Owner {
        Owner {
            process: ProcessIdentity::current().unwrap(),
            runtime_id: token.into(),
        }
    }
    fn target(owner: Owner, ticket: &str) -> Target {
        Target {
            owner,
            window_label: "main".into(),
            ticket: ticket.into(),
            requested: false,
            minimized: true,
        }
    }
    #[test]
    fn early_restore_waits_for_minimize_and_keeps_the_request() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let owner = owner("popup");
        let mut pending = target(owner.clone(), "pending");
        pending.minimized = false;
        edit(&path, |s| s.target = Some(pending)).unwrap();
        let early = request_latest(&path).unwrap().unwrap();
        assert!(early.requested);
        assert!(!targets_window(&early, &owner, true));
        mark_minimized(&path, "pending").unwrap();
        let ready = read(&path).unwrap().target.unwrap();
        assert!(targets_window(&ready, &owner, true));
        assert_eq!(ready.ticket, "pending");
    }

    #[test]
    fn late_minimize_completion_cannot_arm_a_newer_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let mut newer = target(owner("new"), "new");
        newer.minimized = false;
        edit(&path, |s| s.target = Some(newer)).unwrap();
        mark_minimized(&path, "old").unwrap();
        assert!(!read(&path).unwrap().target.unwrap().minimized);
    }
    #[test]
    fn shortcut_owner_does_not_determine_which_popup_is_restored() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let (a, b) = (owner("a"), owner("b"));
        edit(&path, |s| {
            s.shortcut_owner = Some(a.clone());
            s.target = Some(target(a.clone(), "a1"));
        })
        .unwrap();
        edit(&path, |s| s.target = Some(target(b.clone(), "b1"))).unwrap();
        let selected = request_latest(&path).unwrap().unwrap();
        assert!(!targets_window(&selected, &a, true));
        assert!(targets_window(&selected, &b, true));
        assert!(!targets_window(&selected, &b, false));
    }
    #[test]
    fn delayed_restore_or_close_cannot_erase_a_newer_minimized_popup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        let (a, b) = (owner("a"), owner("b"));
        edit(&path, |s| s.target = Some(target(b.clone(), "new"))).unwrap();
        clear_ticket(&path, "old").unwrap();
        clear_window(&path, &a, "main").unwrap();
        assert_eq!(read(&path).unwrap().target.unwrap().ticket, "new");
        clear_window(&path, &b, "main").unwrap();
        assert!(read(&path).unwrap().target.is_none());
    }
    #[test]
    fn recycled_process_and_runtime_id_never_receive_an_old_restore() {
        let current = ProcessIdentity::current().unwrap();
        assert!(current.alive());
        let stale = ProcessIdentity {
            started: "stale-generation".into(),
            ..current
        };
        assert!(!stale.alive());
        let mut t = target(owner("old-runtime"), "old");
        t.requested = true;
        assert!(!targets_window(&t, &owner("new-runtime"), true));
    }
    #[test]
    fn malformed_state_fails_without_overwriting_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        fs::write(&path, "broken").unwrap();
        assert!(edit(&path, |_| ()).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "broken");
    }
    #[test]
    fn repeat_restore_requests_are_idempotent_and_do_not_change_the_target() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        edit(&path, |s| s.target = Some(target(owner("a"), "a1"))).unwrap();
        let first = request_latest(&path).unwrap();
        assert_eq!(request_latest(&path).unwrap(), first);
    }
    #[test]
    fn child_requests_shared_target() {
        let Some(path) = std::env::var_os("ITERATE_POPUP_RESTORE_TEST_PATH") else {
            return;
        };
        let selected = request_latest(Path::new(&path)).unwrap().unwrap();
        assert_eq!(selected.owner.runtime_id, "target-process-runtime");
    }
    #[test]
    fn another_process_can_request_the_exact_last_minimized_window() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state");
        edit(&path, |s| {
            s.shortcut_owner = Some(owner("different-shortcut-owner"));
            s.target = Some(target(owner("target-process-runtime"), "latest-popup"));
        })
        .unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "ui::popup_restore_store::tests::child_requests_shared_target",
            ])
            .env("ITERATE_POPUP_RESTORE_TEST_PATH", &path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let selected = read(&path).unwrap().target.unwrap();
        assert!(selected.requested);
        assert_eq!(selected.ticket, "latest-popup");
        assert!(targets_window(
            &selected,
            &owner("target-process-runtime"),
            true
        ));
        assert!(!targets_window(
            &selected,
            &owner("different-shortcut-owner"),
            true
        ));
    }
}
