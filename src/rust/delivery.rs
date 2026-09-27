//! Per-call local handoff status. Returned means handed to the HTTP handler,
//! never an acknowledgement from the model. The lock detects service exit.
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::Mutex,
};

pub const FAILURE: &str = "连接已断开，回复未确认送达，请检查 AI 客户端。";

pub struct ResponseFileClaim {
    path: PathBuf,
    committed: bool,
}

impl ResponseFileClaim {
    pub fn acquire(response_file: &Path) -> std::io::Result<Self> {
        let path = response_file.with_extension("response.claim");
        OpenOptions::new().write(true).create_new(true).open(&path)?;
        Ok(Self { path, committed: false })
    }

    pub fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for ResponseFileClaim {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn publish_claimed_response_file(response_file: &Path, content: &str) -> std::io::Result<()> {
    let pending_file = response_file.with_extension(format!("pending-{}", uuid::Uuid::new_v4()));
    let result = fs::write(&pending_file, content)
        .and_then(|_| fs::hard_link(&pending_file, response_file));
    let _ = fs::remove_file(&pending_file);
    result
}

#[derive(Debug)]
pub struct Delivery {
    pub path: PathBuf,
    state: Mutex<u8>,
    _owner: File,
}

impl Drop for Delivery {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        let _ = fs::remove_file(self.path.with_extension("lock"));
    }
}

impl Delivery {
    pub fn new() -> std::io::Result<Self> {
        let path = std::env::temp_dir().join(format!("iterate_delivery_{}", uuid::Uuid::new_v4()));
        let owner = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        owner.lock()?;
        fs::write(&path, b"w")?;
        Ok(Self {
            path,
            state: Mutex::new(b'w'),
            _owner: owner,
        })
    }

    fn finish(&self, next: u8) {
        if let Ok(mut state) = self.state.lock() {
            if *state == b'w' {
                // First terminal state wins, including cancellation vs completion.
                if fs::write(&self.path, [next]).is_ok() {
                    *state = next;
                }
            }
        }
    }
    pub fn failed(&self) {
        self.finish(b'f');
    }
    pub fn returned(&self) {
        self.finish(b'r');
    }
    pub fn disconnected(&self) -> bool {
        self.state.lock().map(|s| *s == b'f').unwrap_or(true)
    }
    pub fn was_returned(&self) -> bool {
        self.state.lock().map(|s| *s == b'r').unwrap_or(false)
    }
}

fn status_at(path: &std::path::Path) -> Result<&'static str, String> {
    match fs::read(path).map_err(|e| e.to_string())?.as_slice() {
        b"r" => return Ok("returned"),
        b"f" => return Ok("disconnected"),
        _ => {}
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))
        .map_err(|e| e.to_string())?;
    match file.try_lock() {
        Ok(()) => Ok("disconnected"),
        Err(std::fs::TryLockError::WouldBlock) => Ok("waiting"),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub fn get_mcp_delivery_status() -> Result<String, String> {
    let Some(path) = std::env::var_os("ITERATE_DELIVERY_FILE").filter(|s| !s.is_empty()) else {
        return Ok("untracked".into());
    };
    status_at(&PathBuf::from(path)).map(String::from)
}

pub fn ensure_connected() -> Result<(), String> {
    if get_mcp_delivery_status()? == "disconnected" {
        return Err(FAILURE.into());
    }
    Ok(())
}

pub async fn wait_for_handoff() -> Result<(), String> {
    loop {
        match get_mcp_delivery_status()?.as_str() {
            "waiting" => tokio::time::sleep(std::time::Duration::from_millis(50)).await,
            "disconnected" => return Err(FAILURE.into()),
            _ => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn response_claim_allows_one_writer_and_publishes_complete_file() {
        let response_file = std::env::temp_dir()
            .join(format!("iterate_claim_test_{}.json", uuid::Uuid::new_v4()));
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut handles = Vec::new();
        for index in 0..2 {
            let path = response_file.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                match ResponseFileClaim::acquire(&path) {
                    Ok(mut claim) => {
                        let value = format!("{{\"winner\":{index}}}");
                        publish_claimed_response_file(&path, &value).unwrap();
                        claim.commit();
                        true
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
                    Err(error) => panic!("claim failed unexpectedly: {error}"),
                }
            }));
        }
        barrier.wait();
        assert_eq!(handles.into_iter().map(|handle| handle.join().unwrap()).filter(|won| *won).count(), 1);
        let content = fs::read_to_string(&response_file).unwrap();
        assert!(content == "{\"winner\":0}" || content == "{\"winner\":1}");
        fs::remove_file(response_file.with_extension("response.claim")).unwrap();
        fs::remove_file(response_file).unwrap();
    }

    #[test]
    fn detects_owner_exit_and_preserves_first_terminal_state() {
        let call = Delivery::new().unwrap();
        assert_eq!(status_at(&call.path).unwrap(), "waiting");
        call.failed();
        call.returned();
        assert_eq!(status_at(&call.path).unwrap(), "disconnected");
        assert!(!call.was_returned());
        // Releasing the OS lock models an abruptly terminated service.
        call._owner.unlock().unwrap();
        let call = Delivery::new().unwrap();
        call._owner.unlock().unwrap();
        assert_eq!(status_at(&call.path).unwrap(), "disconnected");
        let call = Delivery::new().unwrap();
        call.returned();
        call.failed();
        assert_eq!(status_at(&call.path).unwrap(), "returned");
        assert!(call.was_returned());
    }
}
