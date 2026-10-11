use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeNamespace {
    uid: u32,
    session: String,
}

impl RuntimeNamespace {
    pub fn new(uid: u32, session: impl AsRef<str>) -> Self {
        let session = session
            .as_ref()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect();
        Self { uid, session }
    }

    pub fn runtime_dir_under(&self, root: &Path) -> PathBuf {
        root.join(format!("iterate-{}-{}", self.uid, self.session))
            .join("speech")
    }
}

fn current_security_session() -> io::Result<String> {
    // AuthSession.h: SecuritySessionId / SessionAttributeBits are UInt32.
    #[link(name = "Security", kind = "framework")]
    extern "C" {
        fn SessionGetInfo(session: u32, actual: *mut u32, attributes: *mut u32) -> i32;
    }
    let mut actual = 0_u32;
    let status = unsafe { SessionGetInfo(u32::MAX, &mut actual, std::ptr::null_mut()) };
    if status != 0 || actual == 0 || actual >= u32::MAX - 1 {
        return Err(io::Error::other(format!(
            "cannot resolve Fn owner security session: status={status} session={actual}"
        )));
    }
    Ok(format!("security-{actual:x}"))
}

pub fn production_owner_lock_path() -> io::Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    // LaunchServices gives each app instance a different XPC_SERVICE_NAME, and
    // SECURITYSESSIONID is not consistently exported by GUI/shell launchers.
    // Ask Security.framework for the real login session so all eligible owners
    // (and their one-shot paste helpers) contend on the same kernel file lock.
    let session = current_security_session()?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME is unavailable"))?;
    let root = home
        .join("Library")
        .join("Caches")
        .join("com.kexin94yyds.iterate")
        .join("run");
    let runtime = RuntimeNamespace::new(uid, session).runtime_dir_under(&root);
    ensure_private_runtime_dir(&runtime, uid)?;
    Ok(runtime.join("fn-owner.lock"))
}

pub fn ensure_private_runtime_dir(path: &Path, expected_uid: u32) -> io::Result<()> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "runtime path is a symlink",
            ));
        }
    } else {
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path)?;
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() || metadata.uid() != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe runtime directory",
        ));
    }
    if metadata.mode() & 0o777 != 0o700 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "runtime directory is not 0700",
        ));
    }
    Ok(())
}

pub fn open_private_lock_file(path: &Path, expected_uid: u32) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.uid() != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe owner lock file",
        ));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "owner lock is not 0600",
        ));
    }
    Ok(file)
}

pub fn open_existing_private_lock_file(path: &Path, expected_uid: u32) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() || metadata.uid() != expected_uid {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unsafe existing owner lock file",
        ));
    }
    if metadata.mode() & 0o777 != 0o600 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "existing owner lock is not 0600",
        ));
    }
    Ok(file)
}
