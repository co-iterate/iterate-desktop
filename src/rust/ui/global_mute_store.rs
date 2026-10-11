//! Process-shared preference. A separate file avoids overwriting unrelated app settings.
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// (revision, muted). Revisions prevent delayed UI responses from reverting the bell.
pub type Snapshot = (u64, bool);

/// Polling can coalesce off/on into a newer true snapshot. A revision gap must not
/// lose that enable action when the user has opened this window from the Dock.
pub fn should_minimize(previous: Option<Snapshot>, next: Snapshot) -> bool {
    next.1
        && previous.is_none_or(|old| next.0 > old.0 && (!old.1 || next.0 > old.0.saturating_add(1)))
}

pub fn read(path: &Path) -> io::Result<Option<Snapshot>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid global mute preference");
    let mut parts = text.split_whitespace();
    let revision = parts
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let muted = match parts.next() {
        Some("true") => true,
        Some("false") => false,
        _ => return Err(invalid()),
    };
    if parts.next().is_some() {
        return Err(invalid());
    }
    Ok(Some((revision, muted)))
}

fn lock(path: &Path) -> io::Result<File> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| io::Error::other("missing parent"))?,
    )?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    file.lock()?;
    Ok(file) // dropping the handle releases the inter-process lock
}

fn write_locked(path: &Path, snapshot: Snapshot) -> io::Result<()> {
    // The same lock protects initialization, writes and the temporary filename.
    let temporary: PathBuf = path.with_extension("tmp");
    let result = (|| {
        let mut file = File::create(&temporary)?;
        writeln!(file, "{} {}", snapshot.0, snapshot.1)?;
        file.sync_all()?;
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

pub fn initialize(path: &Path, legacy_muted: bool) -> io::Result<Snapshot> {
    let _lock = lock(path)?;
    if let Some(snapshot) = read(path)? {
        return Ok(snapshot);
    }
    let snapshot = (1, legacy_muted);
    write_locked(path, snapshot)?;
    Ok(snapshot)
}

/// Explicit desired state, never a per-process toggle of a stale cached value.
pub fn set(path: &Path, muted: bool) -> io::Result<Snapshot> {
    let _lock = lock(path)?;
    let current = read(path)?.unwrap_or((0, false));
    let revision = current
        .0
        .checked_add(1)
        .ok_or_else(|| io::Error::other("mute revision overflow"))?;
    let snapshot = (revision, muted);
    write_locked(path, snapshot)?;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(
                std::env::temp_dir()
                    .join(format!(
                        "iterate-mute-test-{}-{}",
                        std::process::id(),
                        NEXT.fetch_add(1, Ordering::Relaxed)
                    ))
                    .join("mute"),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0.parent().unwrap());
        }
    }

    #[test]
    fn migrates_once_and_new_windows_inherit_both_on_and_off() {
        let f = Fixture::new();
        assert_eq!(initialize(&f.0, true).unwrap(), (1, true));
        assert_eq!(initialize(&f.0, false).unwrap(), (1, true));
        assert_eq!(set(&f.0, false).unwrap(), (2, false));
        assert_eq!(initialize(&f.0, true).unwrap(), (2, false));
    }

    #[test]
    fn coalesced_off_on_still_minimizes_but_unmute_and_replays_do_not() {
        assert!(should_minimize(Some((1, true)), (3, true)));
        assert!(should_minimize(Some((1, false)), (2, true)));
        assert!(!should_minimize(Some((1, true)), (2, false)));
        assert!(!should_minimize(Some((3, true)), (3, true)));
        assert!(!should_minimize(Some((3, false)), (2, true)));
        assert!(!should_minimize(Some((2, true)), (3, true)));
    }

    #[test]
    fn concurrent_writers_preserve_all_revisions_and_valid_reads() {
        let f = Fixture::new();
        initialize(&f.0, false).unwrap();
        std::thread::scope(|scope| {
            for worker in 0..8 {
                let path = &f.0;
                scope.spawn(move || {
                    for _ in 0..20 {
                        set(path, worker % 2 == 0).unwrap();
                        assert!(read(path).unwrap().is_some());
                    }
                });
            }
        });
        assert_eq!(read(&f.0).unwrap().unwrap().0, 161);
    }

    #[test]
    fn malformed_state_is_not_silently_replaced_by_unmuted() {
        let f = Fixture::new();
        initialize(&f.0, true).unwrap();
        fs::write(&f.0, "broken").unwrap();
        assert!(initialize(&f.0, false).is_err());
        assert!(set(&f.0, false).is_err());
    }

    fn child_writer_test_name() -> String {
        let module = module_path!();
        let without_crate = module.split_once("::").map_or(module, |(_, rest)| rest);
        format!("{without_crate}::child_writer")
    }

    #[test]
    fn child_writer() {
        let Some(path) = std::env::var_os("ITERATE_MUTE_TEST_PATH") else {
            return;
        };
        for _ in 0..10 {
            set(Path::new(&path), true).unwrap();
        }
    }

    #[test]
    fn independent_processes_share_the_same_preference_and_lock() {
        let f = Fixture::new();
        initialize(&f.0, false).unwrap();
        let children: Vec<_> = (0..4)
            .map(|_| {
                std::process::Command::new(std::env::current_exe().unwrap())
                    // libtest 的测试名是完整模块路径（不含 crate 名）；只写 tests::child_writer
                    // 在 lib 测试二进制里一个都匹配不到，子进程什么都不做就成功退出。
                    .args(["--exact", &child_writer_test_name()])
                    .env("ITERATE_MUTE_TEST_PATH", &f.0)
                    .stdout(std::process::Stdio::null())
                    .spawn()
                    .unwrap()
            })
            .collect();
        for mut child in children {
            assert!(child.wait().unwrap().success());
        }
        assert_eq!(read(&f.0).unwrap(), Some((41, true)));
        set(&f.0, false).unwrap();
        assert_eq!(initialize(&f.0, true).unwrap(), (42, false));
    }
}
