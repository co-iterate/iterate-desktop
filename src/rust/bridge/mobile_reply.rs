//! Durable, device-scoped receipt for native companion replies.
//! A crash after reservation is deliberately uncertain; never execute it again blindly.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

struct ReplayGuard {
    epoch: String,
    seen: Mutex<HashSet<PathBuf>>,
}

impl ReplayGuard {
    fn reserve(
        &self,
        path: &Path,
        operation: &str,
        digest: &str,
        epoch: &str,
    ) -> Result<Reservation, String> {
        if epoch != self.epoch {
            return Err("reply_replay_epoch_changed".into());
        }
        let mut seen = self.seen.lock().map_err(|_| "reply_replay_unavailable")?;
        if seen.contains(path) {
            // A previously admitted operation can only be read. Never call the
            // create-capable reserve path, even if the file vanishes concurrently.
            return Ok(Reservation::Existing(read(path, operation, digest)?));
        }
        if !seen.contains(path) && seen.len() >= 1000 {
            return Err("reply_history_full".into());
        }
        let reservation = reserve(path, operation, digest)?;
        // This is recorded before the caller is allowed to perform any delivery.
        // Deleting/resetting the on-disk receipt in this process cannot admit it again.
        seen.insert(path.to_path_buf());
        Ok(reservation)
    }
}

fn replay_guard() -> Result<&'static ReplayGuard, String> {
    static GUARD: OnceLock<Result<ReplayGuard, String>> = OnceLock::new();
    GUARD
        .get_or_init(|| {
            use ring::rand::SecureRandom;
            let mut bytes = [0u8; 16];
            ring::rand::SystemRandom::new()
                .fill(&mut bytes)
                .map_err(|_| "reply_replay_unavailable".to_string())?;
            Ok(ReplayGuard {
                epoch: hex::encode(bytes),
                seen: Mutex::new(HashSet::new()),
            })
        })
        .as_ref()
        .map_err(Clone::clone)
}

pub(super) fn replay_epoch() -> Result<&'static str, String> {
    Ok(replay_guard()?.epoch.as_str())
}

pub(super) fn reserve_with_epoch(
    path: &Path,
    operation: &str,
    digest: &str,
    epoch: Option<&str>,
) -> Result<Reservation, String> {
    match epoch {
        Some(epoch) => replay_guard()?.reserve(path, operation, digest, epoch),
        None => reserve(path, operation, digest),
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct Receipt {
    pub operation_id: String,
    pub status: String,
    pub reason: Option<String>,
    digest: String,
    #[serde(default = "receipt_scope")]
    pub receipt_scope: String,
    /// Set while a cross-device source has accepted this reply but not yet
    /// confirmed consumption, so a later lookup can advance the receipt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_device_route: Option<CrossDeviceRoute>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct CrossDeviceRoute {
    pub request_id: String,
    pub project_path: String,
}

pub(super) const CROSS_DEVICE_PENDING_REASON: &str = "source_confirmation_pending";

/// Advance an accepted-but-unconfirmed cross-device reply once the source has
/// consumed the winner. Only the receipt changes; nothing is resubmitted.
pub(super) fn refresh_cross_device_receipt(path: &Path, receipt: Receipt) -> Receipt {
    let pending = receipt.status == "uncertain"
        && receipt.reason.as_deref() == Some(CROSS_DEVICE_PENDING_REASON);
    let Some(route) = receipt.cross_device_route.clone().filter(|_| pending) else {
        return receipt;
    };
    if !crate::cross_device::local_bridge_winner_consumed(&route.request_id, &route.project_path) {
        return receipt;
    }
    let mut delivered = receipt.clone();
    delivered.cross_device_route = None;
    finish(path, delivered, "delivered", None)
        .or_else(|_| lookup(path, &receipt.operation_id).map(|latest| latest.unwrap_or(receipt.clone())))
        .unwrap_or(receipt)
}
fn receipt_scope() -> String {
    "bridge_delivery".into()
}

pub(super) fn lookup(path: &Path, operation_id: &str) -> Result<Option<Receipt>, String> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("reply_receipt_unavailable".into()),
    };
    let mut receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|_| "reply_receipt_uncertain")?;
    if receipt.operation_id != operation_id {
        return Err("operation_identity_conflict".into());
    }
    if receipt.status == "processing" {
        receipt.status = "uncertain".into();
        receipt.reason = Some("previous_attempt_may_have_been_delivered".into());
    }
    Ok(Some(receipt))
}
pub(super) enum Reservation {
    New(Receipt),
    Existing(Receipt),
}
fn read(path: &Path, operation_id: &str, digest: &str) -> Result<Receipt, String> {
    let bytes = fs::read(path).map_err(|_| "reply_receipt_unavailable")?;
    let mut receipt: Receipt =
        serde_json::from_slice(&bytes).map_err(|_| "reply_receipt_uncertain")?;
    if receipt.operation_id != operation_id || receipt.digest != digest {
        return Err("operation_payload_conflict".into());
    }
    if receipt.status == "processing" {
        receipt.status = "uncertain".into();
        receipt.reason = Some("previous_attempt_may_have_been_delivered".into());
    }
    Ok(receipt)
}
pub(super) fn reserve(
    path: &Path,
    operation_id: &str,
    digest: &str,
) -> Result<Reservation, String> {
    fs::create_dir_all(path.parent().ok_or("invalid_receipt_path")?)
        .map_err(|_| "reply_store_unavailable")?;
    if !path.exists()
        && fs::read_dir(path.parent().ok_or("invalid_receipt_path")?)
            .map_err(|_| "reply_store_unavailable")?
            .take(1001)
            .count()
            >= 1000
    {
        return Err("reply_history_full".into());
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            let receipt = Receipt {
                operation_id: operation_id.into(),
                status: "processing".into(),
                reason: None,
                digest: digest.into(),
                receipt_scope: receipt_scope(),
                cross_device_route: None,
            };
            let bytes = serde_json::to_vec(&receipt).map_err(|_| "reply_store_unavailable")?;
            file.write_all(&bytes)
                .and_then(|_| file.sync_all())
                .map_err(|_| "reply_store_unavailable")?;
            #[cfg(unix)]
            fs::File::open(path.parent().ok_or("invalid_receipt_path")?)
                .and_then(|dir| dir.sync_all())
                .map_err(|_| "reply_store_unavailable")?;
            Ok(Reservation::New(receipt))
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            Ok(Reservation::Existing(read(path, operation_id, digest)?))
        }
        Err(_) => Err("reply_store_unavailable".into()),
    }
}
pub(super) fn finish(
    path: &Path,
    mut receipt: Receipt,
    status: &str,
    reason: Option<&str>,
) -> Result<Receipt, String> {
    receipt.status = status.into();
    receipt.reason = reason.map(str::to_owned);
    // Unique per writer: concurrent refreshes of one receipt must not collide.
    let temp: PathBuf =
        path.with_extension(format!("{}.{}.tmp", std::process::id(), uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp).map_err(|_| "reply_receipt_uncertain")?;
    let result = (|| {
        file.write_all(&serde_json::to_vec(&receipt).map_err(|_| "reply_receipt_uncertain")?)
            .and_then(|_| file.sync_all())
            .map_err(|_| "reply_receipt_uncertain")?;
        fs::rename(&temp, path).map_err(|_| "reply_receipt_uncertain")?;
        #[cfg(unix)]
        fs::File::open(path.parent().ok_or("invalid_receipt_path")?)
            .and_then(|dir| dir.sync_all())
            .map_err(|_| "reply_receipt_uncertain")?;
        Ok(receipt)
    })();
    let _ = fs::remove_file(temp);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn replay_guard() -> ReplayGuard {
        ReplayGuard {
            epoch: "test-bridge-epoch".into(),
            seen: std::sync::Mutex::new(std::collections::HashSet::new()),
        }
    }

    #[test]
    fn refresh_leaves_ordinary_uncertain_receipts_unchanged() {
        let path = path();
        let guard = replay_guard();
        let Reservation::New(first) = guard.reserve(&path, "op2", "digest2", &guard.epoch).unwrap()
        else { panic!() };
        let stored = finish(&path, first, "uncertain", Some("delivery_not_confirmed")).unwrap();
        let refreshed = refresh_cross_device_receipt(&path, stored);
        assert_eq!(refreshed.status, "uncertain");
        assert_eq!(refreshed.reason.as_deref(), Some("delivery_not_confirmed"));
        // A pending cross-device receipt whose request no longer exists stays pending.
        let mut pending = refreshed;
        pending.cross_device_route = Some(CrossDeviceRoute {
            request_id: "missing-request".into(), project_path: "/tmp/missing".into(),
        });
        let pending = finish(&path, pending, "uncertain", Some(CROSS_DEVICE_PENDING_REASON)).unwrap();
        assert_eq!(refresh_cross_device_receipt(&path, pending).status, "uncertain");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn bound_retry_recovers_lost_ack_without_second_reservation() {
        let path = path();
        let guard = replay_guard();
        let Reservation::New(first) = guard
            .reserve(&path, "op1", "digest1", &guard.epoch)
            .unwrap()
        else {
            panic!()
        };
        finish(&path, first, "delivered", None).unwrap();
        let Reservation::Existing(again) = guard
            .reserve(&path, "op1", "digest1", &guard.epoch)
            .unwrap()
        else {
            panic!("duplicate side effect admitted")
        };
        assert_eq!(again.status, "delivered");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn changed_epoch_never_creates_a_receipt_or_replays_old_work() {
        let path = path();
        let guard = replay_guard();
        assert!(
            matches!(guard.reserve(&path, "op1", "digest1", "previous-process"), Err(reason) if reason == "reply_replay_epoch_changed")
        );
        assert!(!path.exists());
    }

    #[test]
    fn lost_receipt_store_in_same_epoch_does_not_repeat_delivery() {
        let path = path();
        let guard = replay_guard();
        let Reservation::New(first) = guard
            .reserve(&path, "op1", "digest1", &guard.epoch)
            .unwrap()
        else {
            panic!()
        };
        finish(&path, first, "delivered", None).unwrap();
        fs::remove_file(&path).unwrap();
        assert!(
            matches!(guard.reserve(&path, "op1", "digest1", &guard.epoch), Err(reason) if reason == "reply_receipt_unavailable")
        );
        assert!(!path.exists());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn simultaneous_bound_posts_admit_one_delivery() {
        let path = path();
        let guard = std::sync::Arc::new(replay_guard());
        let mut threads = Vec::new();
        for _ in 0..8 {
            let path = path.clone();
            let guard = guard.clone();
            threads.push(std::thread::spawn(move || {
                matches!(
                    guard
                        .reserve(&path, "op1", "digest1", &guard.epoch)
                        .unwrap(),
                    Reservation::New(_)
                )
            }));
        }
        assert_eq!(
            threads
                .into_iter()
                .map(|thread| thread.join().expect("reservation worker panicked"))
                .filter(|new| *new)
                .count(),
            1
        );
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    static TEST_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    fn path() -> PathBuf {
        std::env::temp_dir()
            .join(format!(
                "iterate-reply-test-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                TEST_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ))
            .join("receipt.json")
    }
    #[test]
    fn retries_return_the_same_receipt_and_changed_payload_is_rejected() {
        let path = path();
        let Reservation::New(first) = reserve(&path, "op1", "digest1").unwrap() else {
            panic!()
        };
        finish(&path, first, "delivered", None).unwrap();
        let Reservation::Existing(retry) = reserve(&path, "op1", "digest1").unwrap() else {
            panic!()
        };
        assert_eq!(retry.status, "delivered");
        assert!(reserve(&path, "op1", "changed").is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn interrupted_attempt_is_uncertain_and_cannot_be_executed_twice() {
        let path = path();
        reserve(&path, "op1", "digest1").unwrap();
        let Reservation::Existing(retry) = reserve(&path, "op1", "digest1").unwrap() else {
            panic!()
        };
        assert_eq!(retry.status, "uncertain");
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn corrupt_reservation_fails_closed() {
        let path = path();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, b"{").unwrap();
        assert!(reserve(&path, "op1", "digest1").is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
    #[test]
    fn status_lookup_never_reserves_or_promotes_a_delivery_to_caller_completion() {
        let path = path();
        assert!(lookup(&path, "op1").unwrap().is_none());
        assert!(!path.exists());
        let Reservation::New(first) = reserve(&path, "op1", "digest1").unwrap() else {
            panic!()
        };
        assert_eq!(lookup(&path, "op1").unwrap().unwrap().status, "uncertain");
        finish(&path, first, "delivered", None).unwrap();
        let readback = lookup(&path, "op1").unwrap().unwrap();
        assert_eq!(readback.status, "delivered");
        assert_eq!(readback.receipt_scope, "bridge_delivery");
        assert!(lookup(&path, "other-operation").is_err());
        fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}
