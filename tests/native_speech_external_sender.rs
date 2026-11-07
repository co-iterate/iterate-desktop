use cunzhi::native_speech::external_sender::{
    encode_request, parse_request, ExpectedExecutableIdentity, HelperFocusMode,
    PasteHelperAttemptOutcome, PasteHelperDispatchOutcome, PasteHelperFrame, PasteHelperLauncher,
    PasteHelperPhase, PasteHelperRequest, PhaseFrameValidator, ProtocolError,
    MAX_HELPER_REQUEST_BYTES, PASTE_HELPER_SCHEMA_VERSION,
};
use cunzhi::native_speech::external_sender::{
    validate_helper_boot_environment, validate_helper_target_environment, HelperBootEnvironment,
    HelperTargetEnvironment,
};
use cunzhi::native_speech::owner::{OwnerMetadata, SpeechProcessRole};

fn request() -> PasteHelperRequest {
    PasteHelperRequest {
        schema_version: PASTE_HELPER_SCHEMA_VERSION,
        attempt_id: "2bd9993f-d49f-4ddf-ad35-13638710ff52".into(),
        owner_epoch: "9a29babe-d0e3-4421-a360-4f5d50396cce".into(),
        parent_pid: 123,
        target_pid: 456,
        target_bundle_id: "com.openai.codex".into(),
        focus_mode: HelperFocusMode::FrontmostPidFallback,
        expected_executable_identity: ExpectedExecutableIdentity {
            executable_path: "/Applications/iterate.app/Contents/MacOS/iterate".into(),
            team_id: "UM3Z9G5DNH".into(),
            cdhash: "ABCDEF0123456789".into(),
        },
    }
}

fn frame(
    request: &PasteHelperRequest,
    helper_pid: u32,
    phase: PasteHelperPhase,
) -> PasteHelperFrame {
    PasteHelperFrame {
        schema_version: PASTE_HELPER_SCHEMA_VERSION,
        attempt_id: request.attempt_id.clone(),
        owner_epoch: request.owner_epoch.clone(),
        helper_pid,
        phase,
        reason: (phase == PasteHelperPhase::RejectedBeforePost).then(|| "target_changed".into()),
        elapsed_ms: 1,
    }
}

#[test]
fn request_round_trips_without_text_payload() {
    let request = request();
    let encoded = encode_request(&request).expect("request should encode");
    let wire = String::from_utf8(encoded.clone()).unwrap();
    assert!(!wire.contains("speech text"));
    assert!(!wire.contains("clipboard"));
    assert_eq!(parse_request(&encoded), Ok(request));
}

#[test]
fn parser_fails_closed_on_unknown_version_fields_and_oversize() {
    let mut unsupported = request();
    unsupported.schema_version = 2;
    assert_eq!(
        encode_request(&unsupported),
        Err(ProtocolError::UnsupportedVersion(2))
    );

    let unknown = br#"{"schema_version":1,"attempt_id":"2bd9993f-d49f-4ddf-ad35-13638710ff52","owner_epoch":"9a29babe-d0e3-4421-a360-4f5d50396cce","parent_pid":123,"target_pid":456,"target_bundle_id":"com.openai.codex","focus_mode":"frontmost_pid_fallback","expected_executable_identity":{"executable_path":"/Applications/iterate.app/Contents/MacOS/iterate","team_id":"UM3Z9G5DNH","cdhash":"ABCDEF0123456789"},"text":"must-not-exist"}"#;
    assert_eq!(parse_request(unknown), Err(ProtocolError::InvalidJson));
    assert_eq!(
        parse_request(&vec![b'x'; MAX_HELPER_REQUEST_BYTES + 1]),
        Err(ProtocolError::TooLarge)
    );
}

#[test]
fn request_rejects_invalid_identity_fields() {
    let mut invalid = request();
    invalid.parent_pid = 0;
    assert_eq!(
        invalid.validate(),
        Err(ProtocolError::InvalidField("parent_pid"))
    );
    invalid = request();
    invalid.target_bundle_id = "codex".into();
    assert_eq!(
        invalid.validate(),
        Err(ProtocolError::InvalidField("target_bundle_id"))
    );
}

#[test]
fn frames_advance_monotonically_to_posted_terminal() {
    let request = request();
    let helper_pid = 777;
    let mut validator = PhaseFrameValidator::new(&request, helper_pid);
    for phase in [
        PasteHelperPhase::Booted,
        PasteHelperPhase::Validated,
        PasteHelperPhase::PostStarted,
        PasteHelperPhase::PostedUnverified,
    ] {
        validator
            .accept(&frame(&request, helper_pid, phase))
            .unwrap();
    }
    assert!(validator.post_started());
    assert!(validator.terminal());
}

#[test]
fn stale_duplicate_out_of_order_and_cross_attempt_frames_are_rejected() {
    let request = request();
    let helper_pid = 777;
    let mut validator = PhaseFrameValidator::new(&request, helper_pid);
    let booted = frame(&request, helper_pid, PasteHelperPhase::Booted);
    validator.accept(&booted).unwrap();
    assert_eq!(
        validator.accept(&booted),
        Err(ProtocolError::DuplicateOrOutOfOrder)
    );

    let mut cross_attempt = frame(&request, helper_pid, PasteHelperPhase::Validated);
    cross_attempt.attempt_id = "f52f7626-09d7-44bb-965d-fef52e9a7525".into();
    assert_eq!(
        validator.accept(&cross_attempt),
        Err(ProtocolError::IdentityMismatch)
    );

    let post_started = frame(&request, helper_pid, PasteHelperPhase::PostStarted);
    assert_eq!(
        validator.accept(&post_started),
        Err(ProtocolError::DuplicateOrOutOfOrder)
    );
}

#[test]
fn rejection_is_terminal_only_before_post_started() {
    let request = request();
    let helper_pid = 777;
    let mut rejected = PhaseFrameValidator::new(&request, helper_pid);
    rejected
        .accept(&frame(
            &request,
            helper_pid,
            PasteHelperPhase::RejectedBeforePost,
        ))
        .unwrap();
    assert!(rejected.terminal());

    let mut after_post = PhaseFrameValidator::new(&request, helper_pid);
    for phase in [
        PasteHelperPhase::Booted,
        PasteHelperPhase::Validated,
        PasteHelperPhase::PostStarted,
    ] {
        after_post
            .accept(&frame(&request, helper_pid, phase))
            .unwrap();
    }
    assert_eq!(
        after_post.accept(&frame(
            &request,
            helper_pid,
            PasteHelperPhase::RejectedBeforePost,
        )),
        Err(ProtocolError::DuplicateOrOutOfOrder)
    );
}

#[test]
fn helper_boot_requires_live_parent_owner_epoch_and_same_signed_executable() {
    let request = request();
    let environment = HelperBootEnvironment {
        actual_parent_pid: request.parent_pid,
        current_executable_path: request.expected_executable_identity.executable_path.clone(),
        current_team_id: request.expected_executable_identity.team_id.clone(),
        current_cdhash: request.expected_executable_identity.cdhash.clone(),
        owner: OwnerMetadata {
            epoch: Some(request.owner_epoch.clone()),
            pid: request.parent_pid,
            executable: request.expected_executable_identity.executable_path.clone(),
            signing_identity: request.expected_executable_identity.team_id.clone(),
            signing_hash: request.expected_executable_identity.cdhash.clone(),
            acquired_unix_ms: 1,
            role: SpeechProcessRole::CanonicalGui,
        },
    };
    assert_eq!(
        validate_helper_boot_environment(&request, &environment),
        Ok(())
    );

    let mut forged = environment.clone();
    forged.owner.epoch = Some("5cecd58f-dbc1-4a51-b26f-b2887080906e".into());
    assert_eq!(
        validate_helper_boot_environment(&request, &forged),
        Err("owner_epoch_mismatch")
    );
    forged = environment.clone();
    forged.actual_parent_pid += 1;
    assert_eq!(
        validate_helper_boot_environment(&request, &forged),
        Err("parent_pid_mismatch")
    );
    forged = environment;
    forged.current_cdhash = "DIFFERENT".into();
    assert_eq!(
        validate_helper_boot_environment(&request, &forged),
        Err("cdhash_mismatch")
    );
}

#[test]
fn helper_target_gate_rejects_pid_bundle_focus_or_permission_drift() {
    let request = request();
    let environment = HelperTargetEnvironment {
        target_identity_matches: true,
        frontmost_pid: request.target_pid,
        frontmost_bundle_id: request.target_bundle_id.clone(),
        accessibility_trusted: true,
    };
    assert_eq!(
        validate_helper_target_environment(&request, &environment),
        Ok(())
    );

    let mut changed = environment.clone();
    changed.frontmost_pid += 1;
    assert_eq!(
        validate_helper_target_environment(&request, &changed),
        Err("frontmost_target_mismatch")
    );
    changed = environment.clone();
    changed.target_identity_matches = false;
    assert_eq!(
        validate_helper_target_environment(&request, &changed),
        Err("target_identity_mismatch")
    );
    changed = environment;
    changed.accessibility_trusted = false;
    assert_eq!(
        validate_helper_target_environment(&request, &changed),
        Err("accessibility_not_trusted")
    );
}

#[test]
fn helper_cli_is_handled_before_any_gui_or_service_role() {
    let cli = include_str!("../src/rust/app/cli.rs");
    let handler = cli
        .split("pub fn handle_cli_args()")
        .nth(1)
        .expect("handle_cli_args should exist");
    let helper = handler.find("--speech-paste-helper").unwrap();
    let serve = handler.find("--serve").unwrap();
    let gui = handler.find("run_tauri_app()").unwrap();
    assert!(helper < serve);
    assert!(helper < gui);
    assert!(handler.contains("speech paste helper accepts no payload arguments"));
}

struct FakeLauncher {
    outcomes: std::collections::VecDeque<PasteHelperAttemptOutcome>,
    launches: usize,
}

impl PasteHelperLauncher for FakeLauncher {
    fn launch_once(&mut self, _request: &PasteHelperRequest) -> PasteHelperAttemptOutcome {
        self.launches += 1;
        self.outcomes.pop_front().unwrap()
    }
}

fn dispatch(outcomes: Vec<PasteHelperAttemptOutcome>) -> (PasteHelperDispatchOutcome, usize) {
    let mut launcher = FakeLauncher {
        outcomes: outcomes.into(),
        launches: 0,
    };
    let outcome =
        cunzhi::native_speech::external_sender::dispatch_with_launcher(&mut launcher, &request());
    (outcome, launcher.launches)
}

#[test]
fn pre_spawn_failure_retries_at_most_once() {
    let (outcome, launches) = dispatch(vec![
        PasteHelperAttemptOutcome::SpawnFailed {
            reason: "spawn_1".into(),
        },
        PasteHelperAttemptOutcome::SpawnFailed {
            reason: "spawn_2".into(),
        },
    ]);
    assert_eq!(launches, 2);
    assert_eq!(
        outcome,
        PasteHelperDispatchOutcome::RejectedBeforePost {
            helper_pid: None,
            reason: "spawn_2".into(),
            attempts: 2,
        }
    );
}

#[test]
fn explicit_retryable_pre_post_rejection_gets_one_fresh_helper() {
    let (outcome, launches) = dispatch(vec![
        PasteHelperAttemptOutcome::RejectedBeforePost {
            helper_pid: 10,
            reason: "codesign_start_failed:busy".into(),
        },
        PasteHelperAttemptOutcome::DispatchedUnverified { helper_pid: 11 },
    ]);
    assert_eq!(launches, 2);
    assert_eq!(
        outcome,
        PasteHelperDispatchOutcome::DispatchedUnverified {
            helper_pid: 11,
            attempts: 2,
        }
    );
}

#[test]
fn identity_target_and_permission_rejections_never_retry() {
    for reason in [
        "owner_epoch_mismatch",
        "team_id_mismatch",
        "target_identity_mismatch",
        "frontmost_target_mismatch",
        "accessibility_not_trusted",
    ] {
        let (outcome, launches) = dispatch(vec![PasteHelperAttemptOutcome::RejectedBeforePost {
            helper_pid: 10,
            reason: reason.into(),
        }]);
        assert_eq!(launches, 1);
        assert!(matches!(
            outcome,
            PasteHelperDispatchOutcome::RejectedBeforePost { attempts: 1, .. }
        ));
    }
}

#[test]
fn any_unknown_after_child_creation_never_retries() {
    for reason in [
        "helper_timeout_without_explicit_pre_post_rejection",
        "invalid_helper_frame",
        "helper_ended_after_post_started_without_terminal",
    ] {
        let (outcome, launches) = dispatch(vec![PasteHelperAttemptOutcome::UnknownAfterDispatch {
            helper_pid: Some(10),
            reason: reason.into(),
        }]);
        assert_eq!(launches, 1);
        assert_eq!(
            outcome,
            PasteHelperDispatchOutcome::UnknownAfterDispatch {
                helper_pid: Some(10),
                reason: reason.into(),
                attempts: 1,
            }
        );
    }
}

#[test]
fn repeated_logical_attempts_each_use_one_fresh_helper_pid() {
    let mut helper_pids = std::collections::BTreeSet::new();
    for index in 0..20_u32 {
        let helper_pid = 10_000 + index;
        let (outcome, launches) = dispatch(vec![PasteHelperAttemptOutcome::DispatchedUnverified {
            helper_pid,
        }]);
        assert_eq!(launches, 1);
        assert_eq!(
            outcome,
            PasteHelperDispatchOutcome::DispatchedUnverified {
                helper_pid,
                attempts: 1,
            }
        );
        assert!(helper_pids.insert(helper_pid), "helper PID must be fresh");
    }
    assert_eq!(helper_pids.len(), 20);
}

#[test]
fn production_wiring_defaults_to_helper_and_keeps_one_sender_per_attempt() {
    let speech = include_str!("../src/rust/native_speech/mod.rs");
    let selector = speech
        .split("fn selected_external_sender_mode()")
        .nth(1)
        .expect("external sender selector should exist");
    assert!(selector.contains("Some(\"in_process\") => ExternalSenderMode::InProcess"));
    assert!(selector.contains("_ => ExternalSenderMode::OneShotHelper"));

    let paste = speech
        .split("fn paste_text_with_identity(")
        .nth(1)
        .expect("tagged paste implementation should exist");
    let dispatch_match = paste
        .split("let dispatch = match sender_mode")
        .nth(1)
        .expect("exactly one selected sender should dispatch");
    assert!(dispatch_match.contains("ExternalSenderMode::InProcess =>"));
    assert!(dispatch_match.contains("ExternalSenderMode::OneShotHelper =>"));
    assert!(dispatch_match.contains("dispatch_with_launcher"));
}

#[test]
fn authenticated_popup_writeback_bypasses_external_helper() {
    let speech = include_str!("../src/rust/native_speech/mod.rs");
    let dispatch = speech
        .split("pub(crate) fn dispatch_speech_writeback(")
        .nth(1)
        .expect("speech writeback dispatcher should exist");
    let popup_branch = dispatch
        .split("SpeechTarget::IteratePopupInput {")
        .nth(1)
        .expect("popup writeback branch should exist");
    let popup_branch = popup_branch
        .split("pub fn paste_text")
        .next()
        .expect("popup branch should end before external paste command");
    assert!(popup_branch.contains("SpeechWritebackDispatch::Popup"));
    assert!(popup_branch.contains("SpeechInsertTextPayload"));
    assert!(popup_branch.contains("ExternalAcknowledged"));
    assert!(!popup_branch.contains("dispatch_with_launcher"));
    assert!(!popup_branch.contains("simulate_paste"));
}

#[test]
fn external_paste_is_verified_by_a_transient_pasteboard_promise() {
    let speech = include_str!("../src/rust/native_speech/mod.rs");
    let probe = include_str!("../src/rust/native_speech/paste_probe.rs");
    let phase1 = include_str!("../src/rust/native_speech/phase1.rs");

    let paste = speech
        .split("fn paste_text_with_identity(")
        .nth(1)
        .expect("external paste flow should exist");
    let publish = paste
        .find("paste_probe::publish(trimmed)")
        .expect("promise publish");
    let dispatch = paste
        .find("dispatch_with_launcher")
        .expect("helper dispatch");
    let wait = paste
        .find("wait_consumed(PASTE_CONSUME_TIMEOUT)")
        .expect("consume wait");
    assert!(publish < dispatch && dispatch < wait);
    assert!(paste.contains("ExternalPasteDispatch::NotConsumed"));
    assert!(paste.contains("dispatch != ExternalPasteDispatch::NotConsumed"));

    assert!(probe.contains("org.nspasteboard.TransientType"));
    assert!(probe.contains("setDataProvider"));

    let not_consumed = phase1
        .split("SpeechWritebackDispatch::ExternalNotConsumed) => {")
        .nth(1)
        .expect("phase1 handles unread pastes");
    let branch = not_consumed.split("Ok(super::").next().unwrap();
    assert!(branch.contains("preserve_unwritten_transcript(&recovery_text, \"not-consumed\")"));
    assert!(branch.contains("WritebackFailed"));
}
