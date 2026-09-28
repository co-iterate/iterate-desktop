#![cfg(unix)]

use cunzhi::native_speech::owner::{
    read_existing_owner_metadata, OwnerEpochSource, OwnerLeaseAttempt, OwnerLeaseBackend,
    OwnerMetadata, OwnerSupervisor, OwnerSupervisorCommand, OwnerSupervisorEvent,
    SpeechProcessRole,
};
use cunzhi::native_speech::runtime_paths::{
    ensure_private_runtime_dir, open_private_lock_file, RuntimeNamespace,
};
use std::collections::VecDeque;
use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[test]
fn roles_parse_and_only_canonical_or_agent_are_eligible() {
    let cases = [
        (vec!["iterate"], SpeechProcessRole::CanonicalGui, true),
        (
            vec!["iterate", "--standalone"],
            SpeechProcessRole::StandalonePopup,
            false,
        ),
        (vec!["iterate", "serve"], SpeechProcessRole::Serve, false),
        (vec!["iterate", "--serve"], SpeechProcessRole::Serve, false),
        (
            vec!["iterate", "mcp-request"],
            SpeechProcessRole::McpRequest,
            false,
        ),
        (
            vec!["iterate", "--mcp-request"],
            SpeechProcessRole::McpRequest,
            false,
        ),
        (
            vec!["iterate", "--bridge-only"],
            SpeechProcessRole::Bridge,
            false,
        ),
        (vec!["iterate-speech-agent"], SpeechProcessRole::Agent, true),
        (
            vec!["iterate", "--speech-paste-helper"],
            SpeechProcessRole::PasteHelper,
            false,
        ),
        (
            vec!["iterate", "--speech-paste-helper-dry-run"],
            SpeechProcessRole::PasteHelper,
            false,
        ),
    ];
    for (args, expected, eligible) in cases {
        let parsed = SpeechProcessRole::from_args(args.iter().copied());
        assert_eq!(parsed, expected);
        assert_eq!(parsed.is_owner_eligible(), eligible);
    }
}

#[test]
fn bundle_launched_standalone_popup_is_never_owner_eligible_when_argv_is_empty() {
    let parsed = SpeechProcessRole::from_runtime(["iterate"], true);
    assert_eq!(parsed, SpeechProcessRole::StandalonePopup);
    assert!(!parsed.is_owner_eligible());
}

#[test]
fn helper_read_never_creates_a_missing_owner_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing-owner.lock");
    assert!(read_existing_owner_metadata(&path, unsafe { libc::geteuid() }).is_err());
    assert!(!path.exists());
}

#[derive(Clone)]
struct FakeBackend {
    attempts: Arc<Mutex<VecDeque<OwnerLeaseAttempt>>>,
    releases: Arc<Mutex<u64>>,
}

impl OwnerLeaseBackend for FakeBackend {
    fn try_acquire(&mut self, _metadata: &OwnerMetadata) -> OwnerLeaseAttempt {
        self.attempts.lock().unwrap().pop_front().unwrap()
    }

    fn release(&mut self) {
        *self.releases.lock().unwrap() += 1;
    }
}

#[derive(Clone)]
struct FixedEpoch([u8; 16]);

impl OwnerEpochSource for FixedEpoch {
    fn next_epoch(&mut self) -> [u8; 16] {
        self.0
    }
}

fn metadata(role: SpeechProcessRole) -> OwnerMetadata {
    OwnerMetadata::for_test(
        role,
        123,
        "/Applications/iterate.app",
        "Developer ID",
        "abc",
    )
}

#[test]
fn busy_owner_is_not_stolen_and_free_retry_acquires_within_next_reconcile() {
    let busy = metadata(SpeechProcessRole::CanonicalGui);
    let attempts = Arc::new(Mutex::new(VecDeque::from([
        OwnerLeaseAttempt::Busy(busy.clone()),
        OwnerLeaseAttempt::Acquired,
    ])));
    let backend = FakeBackend {
        attempts,
        releases: Arc::new(Mutex::new(0)),
    };
    let mut supervisor = OwnerSupervisor::new(
        metadata(SpeechProcessRole::Agent),
        backend,
        FixedEpoch([7; 16]),
    );
    assert_eq!(
        supervisor.apply(OwnerSupervisorCommand::Reconcile, true),
        vec![OwnerSupervisorEvent::Busy(busy)]
    );
    assert_eq!(supervisor.listener_start_count(), 0);
    assert!(matches!(
        supervisor
            .apply(OwnerSupervisorCommand::Reconcile, true)
            .as_slice(),
        [
            OwnerSupervisorEvent::Acquired(_),
            OwnerSupervisorEvent::ListenerStartRequested
        ]
    ));
    assert_eq!(supervisor.listener_start_count(), 1);
}

#[test]
fn listener_recreation_keeps_one_epoch_and_idle_handoff_is_idempotent() {
    let releases = Arc::new(Mutex::new(0));
    let backend = FakeBackend {
        attempts: Arc::new(Mutex::new(VecDeque::from([OwnerLeaseAttempt::Acquired]))),
        releases: releases.clone(),
    };
    let mut supervisor = OwnerSupervisor::new(
        metadata(SpeechProcessRole::CanonicalGui),
        backend,
        FixedEpoch([8; 16]),
    );
    supervisor.apply(OwnerSupervisorCommand::Reconcile, true);
    let epoch = supervisor.owner_epoch();
    assert_eq!(
        supervisor.apply(OwnerSupervisorCommand::ListenerEnded, true),
        vec![OwnerSupervisorEvent::ListenerStartRequested]
    );
    assert_eq!(supervisor.owner_epoch(), epoch);
    assert!(supervisor
        .apply(OwnerSupervisorCommand::RequestIdleHandoff, false)
        .is_empty());
    assert_eq!(
        supervisor.apply(OwnerSupervisorCommand::RequestIdleHandoff, true),
        vec![OwnerSupervisorEvent::Released]
    );
    assert!(supervisor
        .apply(OwnerSupervisorCommand::Shutdown, true)
        .is_empty());
    assert_eq!(*releases.lock().unwrap(), 1);
}

fn temp_root(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "iterate-owner-{label}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ))
}

#[test]
fn runtime_namespace_is_private_and_two_sessions_do_not_collide() {
    let root = temp_root("private");
    let one = RuntimeNamespace::new(501, "session-a").runtime_dir_under(&root);
    let two = RuntimeNamespace::new(501, "session-b").runtime_dir_under(&root);
    assert_ne!(one, two);
    ensure_private_runtime_dir(&one, 501).unwrap();
    assert_eq!(
        fs::metadata(&one).unwrap().permissions().mode() & 0o777,
        0o700
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lock_open_is_no_follow_regular_current_uid_and_mode_0600() {
    let uid = unsafe { libc::geteuid() };
    let root = temp_root("lock");
    let runtime = RuntimeNamespace::new(uid, "tests").runtime_dir_under(&root);
    ensure_private_runtime_dir(&runtime, uid).unwrap();
    let lock_path = runtime.join("fn-owner.lock");
    let file = open_private_lock_file(&lock_path, uid).unwrap();
    let metadata = file.metadata().unwrap();
    assert_eq!(metadata.uid(), uid);
    assert_eq!(metadata.mode() & 0o777, 0o600);
    assert!(metadata.file_type().is_file());

    let symlink_path = runtime.join("symlink.lock");
    symlink(&lock_path, &symlink_path).unwrap();
    assert!(open_private_lock_file(&symlink_path, uid).is_err());
    drop(file);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn real_flock_contender_is_busy_then_takes_over_after_release() {
    use cunzhi::native_speech::owner::FileOwnerLease;
    let uid = unsafe { libc::geteuid() };
    let root = temp_root("flock");
    let runtime = RuntimeNamespace::new(uid, "flock").runtime_dir_under(&root);
    ensure_private_runtime_dir(&runtime, uid).unwrap();
    let path = runtime.join("fn-owner.lock");
    let mut first = FileOwnerLease::new(path.clone(), uid);
    let mut second = FileOwnerLease::new(path, uid);
    assert_eq!(
        first.try_acquire(&metadata(SpeechProcessRole::CanonicalGui)),
        OwnerLeaseAttempt::Acquired
    );
    assert!(matches!(
        second.try_acquire(&metadata(SpeechProcessRole::Agent)),
        OwnerLeaseAttempt::Busy(_)
    ));
    first.release();
    assert_eq!(
        second.try_acquire(&metadata(SpeechProcessRole::Agent)),
        OwnerLeaseAttempt::Acquired
    );
    second.release();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stored_lock_epoch_matches_the_supervisor_acquired_event() {
    use cunzhi::native_speech::owner::FileOwnerLease;
    let uid = unsafe { libc::geteuid() };
    let root = temp_root("epoch-single-source");
    let runtime = RuntimeNamespace::new(uid, "epoch").runtime_dir_under(&root);
    ensure_private_runtime_dir(&runtime, uid).unwrap();
    let path = runtime.join("fn-owner.lock");
    let backend = FileOwnerLease::new(path.clone(), uid);
    let mut supervisor = OwnerSupervisor::new(
        metadata(SpeechProcessRole::CanonicalGui),
        backend,
        FixedEpoch([9; 16]),
    );

    let acquired = supervisor
        .apply(OwnerSupervisorCommand::Reconcile, true)
        .into_iter()
        .find_map(|event| match event {
            OwnerSupervisorEvent::Acquired(epoch) => Some(epoch),
            _ => None,
        })
        .expect("acquired event");
    let stored: OwnerMetadata =
        serde_json::from_slice(&fs::read(&path).unwrap()).expect("stored owner metadata");

    assert_eq!(
        stored.epoch.as_deref(),
        Some(acquired.to_canonical_string().as_str())
    );
    supervisor.apply(OwnerSupervisorCommand::Shutdown, true);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn production_wiring_uses_role_gate_and_has_no_global_tmp_owner_path() {
    let setup = include_str!("../src/rust/app/setup.rs");
    let speech = include_str!("../src/rust/native_speech/mod.rs");
    assert!(setup.contains("SpeechProcessRole::from_runtime"));
    assert!(setup.contains("is_standalone"));
    assert!(setup.contains("is_owner_eligible"));
    assert!(!speech.contains("temp_dir().join(FN_OWNER_LOCK_FILE)"));
    assert!(!speech.contains("iterate-fn-owner.lock"));
    assert!(speech.contains("open_private_lock_file"));
}
