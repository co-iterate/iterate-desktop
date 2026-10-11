// Also runnable with rustc --test and the repository's built libc dependency.
// Child processes let us vary launch environments without racing other tests.
#[path = "../src/rust/native_speech/runtime_paths.rs"]
mod runtime_paths;

use std::path::PathBuf;
use std::process::Command;

struct TestHome(PathBuf);
impl TestHome {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "iterate-owner-launch-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn probe(&self, security_env: Option<&str>, xpc: &str) -> String {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "production_path_child",
                "--ignored",
                "--nocapture",
            ])
            .env("HOME", &self.0)
            .env("XPC_SERVICE_NAME", xpc)
            .env_remove("SECURITYSESSIONID");
        if let Some(value) = security_env {
            command.env("SECURITYSESSIONID", value);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .find_map(|line| line.strip_prefix("OWNER_PATH=").map(str::to_owned))
            .unwrap()
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
#[ignore = "subprocess probe; invoked by launch-environment regression tests"]
fn production_path_child() {
    println!(
        "OWNER_PATH={}",
        runtime_paths::production_owner_lock_path()
            .unwrap()
            .display()
    );
}

#[test]
fn two_launchservices_instances_share_one_owner_lock() {
    let home = TestHome::new();
    assert_eq!(
        home.probe(None, "application.iterate.FIRST"),
        home.probe(None, "application.iterate.SECOND")
    );
}

#[test]
fn shell_and_launchservices_share_one_owner_lock_despite_environment() {
    let home = TestHome::new();
    assert_eq!(
        home.probe(Some("fake-session-from-launcher"), "shell"),
        home.probe(None, "application.iterate.GUI")
    );
}

#[test]
#[ignore = "child contender invoked by the kernel-lock regression"]
fn competing_owner_child() {
    use std::os::fd::AsRawFd;
    let path = runtime_paths::production_owner_lock_path().unwrap();
    let file = runtime_paths::open_private_lock_file(&path, unsafe { libc::geteuid() }).unwrap();
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    assert_eq!(
        result, -1,
        "a differently launched owner acquired a second lock"
    );
    assert_eq!(
        std::io::Error::last_os_error().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn differently_launched_process_cannot_acquire_a_second_owner() {
    use std::os::fd::AsRawFd;
    let home = TestHome::new();
    let first_path = home.probe(None, "application.iterate.FIRST");
    let file = runtime_paths::open_private_lock_file(std::path::Path::new(&first_path), unsafe {
        libc::geteuid()
    })
    .unwrap();
    assert_eq!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) },
        0
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "competing_owner_child",
            "--ignored",
            "--nocapture",
        ])
        .env("HOME", &home.0)
        .env_remove("SECURITYSESSIONID")
        .env("XPC_SERVICE_NAME", "application.iterate.SECOND")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
