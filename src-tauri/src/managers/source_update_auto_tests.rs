use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::time::Duration;

use super::*;

const TARGET: &str = "0123456789abcdef0123456789abcdef01234567";
const OTHER: &str = "fedcba9876543210fedcba9876543210fedcba98";

/// Facts where every rule passes and the policy starts.
fn good() -> AutoFacts {
    AutoFacts {
        source_mode: true,
        checks_effective: true,
        auto_enabled: true,
        state_available: true,
        install_blocker_none: true,
        run_active: false,
        backoff_clear: true,
        verified: VerifiedState::Verified,
        gate_free: true,
        jobs_idle: true,
        idle_for: AUTO_IDLE,
    }
}

#[test]
fn all_rules_passing_starts() {
    assert_eq!(auto_update_decision(&good()), AutoDecision::Start);
}

#[test]
fn each_single_failing_rule_gives_its_reason() {
    let cases: [(fn(&mut AutoFacts), AutoDecision); 14] = [
        (
            |f| f.checks_effective = false,
            AutoDecision::Skip(SkipReason::Disabled),
        ),
        (
            |f| f.auto_enabled = false,
            AutoDecision::Skip(SkipReason::Disabled),
        ),
        (
            |f| f.source_mode = false,
            AutoDecision::Skip(SkipReason::NotSource),
        ),
        (
            |f| f.state_available = false,
            AutoDecision::Skip(SkipReason::NoUpdate),
        ),
        (
            |f| f.install_blocker_none = false,
            AutoDecision::Skip(SkipReason::Blocked),
        ),
        (
            |f| f.run_active = true,
            AutoDecision::Wait(WaitReason::ActiveRun),
        ),
        (
            |f| f.backoff_clear = false,
            AutoDecision::Skip(SkipReason::Backoff),
        ),
        (
            |f| f.verified = VerifiedState::Unknown,
            AutoDecision::Wait(WaitReason::Verifying),
        ),
        (
            |f| f.verified = VerifiedState::Unverified,
            AutoDecision::Skip(SkipReason::Unverified),
        ),
        (
            |f| f.verified = VerifiedState::Failed,
            AutoDecision::Skip(SkipReason::VerifyFailed),
        ),
        (
            |f| f.gate_free = false,
            AutoDecision::Wait(WaitReason::Busy),
        ),
        (
            |f| f.jobs_idle = false,
            AutoDecision::Wait(WaitReason::NotIdle),
        ),
        (
            |f| f.idle_for = AUTO_IDLE - Duration::from_millis(1),
            AutoDecision::Wait(WaitReason::NotIdle),
        ),
        (
            |f| f.idle_for = Duration::ZERO,
            AutoDecision::Wait(WaitReason::NotIdle),
        ),
    ];
    for (mutate, expected) in cases {
        let mut facts = good();
        mutate(&mut facts);
        assert_eq!(auto_update_decision(&facts), expected, "{facts:?}");
    }
}

// B7 major: an update run of ours holds the update lock, so the blocker is present while the
// run is active. The active run must win, so the attempt is a wait and never a persisted skip.
#[test]
fn active_run_beats_blocked_in_the_decision_order() {
    let mut facts = good();
    facts.run_active = true;
    facts.install_blocker_none = false;
    assert_eq!(
        auto_update_decision(&facts),
        AutoDecision::Wait(WaitReason::ActiveRun)
    );
    facts.run_active = false;
    assert_eq!(
        auto_update_decision(&facts),
        AutoDecision::Skip(SkipReason::Blocked)
    );
}

#[test]
fn consent_is_checked_before_everything_else() {
    let mut facts = good();
    facts.auto_enabled = false;
    facts.source_mode = false;
    facts.verified = VerifiedState::Failed;
    facts.gate_free = false;
    assert_eq!(
        auto_update_decision(&facts),
        AutoDecision::Skip(SkipReason::Disabled)
    );
}

#[test]
fn exhaustive_truth_table_starts_only_when_everything_passes() {
    let verified_states = [
        VerifiedState::Unknown,
        VerifiedState::Verified,
        VerifiedState::Unverified,
        VerifiedState::Failed,
    ];
    let idle_states = [
        Duration::ZERO,
        AUTO_IDLE - Duration::from_secs(1),
        AUTO_IDLE,
    ];
    let mut starts = 0;
    for bits in 0..512u32 {
        let flag = |bit: u32| bits & (1 << bit) != 0;
        for verified in verified_states {
            for idle_for in idle_states {
                let facts = AutoFacts {
                    source_mode: flag(0),
                    checks_effective: flag(1),
                    auto_enabled: flag(2),
                    state_available: flag(3),
                    install_blocker_none: flag(4),
                    run_active: flag(5),
                    backoff_clear: flag(6),
                    gate_free: flag(7),
                    jobs_idle: flag(8),
                    verified,
                    idle_for,
                };
                let decision = auto_update_decision(&facts);
                let must_pass = facts.source_mode
                    && facts.checks_effective
                    && facts.auto_enabled
                    && facts.state_available
                    && facts.install_blocker_none
                    && !facts.run_active
                    && facts.backoff_clear
                    && facts.verified == VerifiedState::Verified
                    && facts.gate_free
                    && facts.jobs_idle
                    && facts.idle_for >= AUTO_IDLE;
                assert_eq!(decision == AutoDecision::Start, must_pass, "{facts:?}");
                if decision == AutoDecision::Start {
                    starts += 1;
                }
            }
        }
    }
    assert_eq!(starts, 1, "exactly one combination starts");
}

#[test]
fn idle_boundary_is_exactly_sixty_seconds() {
    let mut facts = good();
    facts.idle_for = Duration::from_secs(59);
    assert_eq!(
        auto_update_decision(&facts),
        AutoDecision::Wait(WaitReason::NotIdle)
    );
    facts.idle_for = Duration::from_secs(60);
    assert_eq!(auto_update_decision(&facts), AutoDecision::Start);
}

#[test]
fn stable_codes_are_exact() {
    assert_eq!(AutoDecision::Start.code(), "start");
    assert_eq!(SkipReason::Disabled.code(), "disabled");
    assert_eq!(SkipReason::NotSource.code(), "not_source");
    assert_eq!(SkipReason::NoUpdate.code(), "no_update");
    assert_eq!(SkipReason::Blocked.code(), "blocked");
    assert_eq!(SkipReason::Backoff.code(), "backoff");
    assert_eq!(SkipReason::Unverified.code(), "unverified");
    assert_eq!(SkipReason::VerifyFailed.code(), "verify_failed");
    assert_eq!(WaitReason::ActiveRun.code(), "active_run");
    assert_eq!(WaitReason::Busy.code(), "busy");
    assert_eq!(WaitReason::NotIdle.code(), "not_idle");
    assert_eq!(WaitReason::Verifying.code(), "verifying");
    assert_eq!(AutoDecision::Wait(WaitReason::Busy).code(), "busy");
}

fn failed_state(now: u64) -> AutoState {
    AutoState::default().with_failure(TARGET, "failed", now)
}

#[test]
fn no_history_always_allows_a_retry() {
    assert!(should_retry(&AutoState::default(), TARGET, 1_000));
}

#[test]
fn failed_target_is_blocked_until_exactly_the_cooldown_passes() {
    let state = failed_state(1_000);
    assert!(!should_retry(&state, TARGET, 1_000));
    assert!(!should_retry(
        &state,
        TARGET,
        1_000 + AUTO_RETRY_COOLDOWN_SECS - 1
    ));
    assert!(should_retry(
        &state,
        TARGET,
        1_000 + AUTO_RETRY_COOLDOWN_SECS
    ));
}

#[test]
fn a_different_target_is_never_blocked_by_an_old_failure() {
    let state = failed_state(1_000);
    assert!(should_retry(&state, OTHER, 1_001));
}

#[test]
fn clock_going_backwards_blocks_a_small_skew_but_not_a_bogus_future_stamp() {
    let state = failed_state(10_000);
    // Small backwards skew: the cooldown still holds (fail closed).
    assert!(!should_retry(&state, TARGET, 9_990));
    // A stamp more than a full cooldown ahead is corrupt: it must not block forever.
    let bogus = failed_state(10_000 + AUTO_RETRY_COOLDOWN_SECS + 5);
    assert!(should_retry(&bogus, TARGET, 10_000));
}

#[test]
fn record_helpers_set_the_expected_fields() {
    let base = AutoState::default();
    let failed = base.with_failure(TARGET, "boom", 5);
    assert_eq!(failed.last_result, Some(AutoResult::Failed));
    assert_eq!(failed.failed_target.as_deref(), Some(TARGET));
    assert_eq!(failed.failed_ts, Some(5));
    let skipped = failed.with_skipped(OTHER, "disabled", 9);
    assert_eq!(skipped.last_result, Some(AutoResult::Skipped));
    assert_eq!(
        skipped.failed_target.as_deref(),
        Some(TARGET),
        "failure memory kept"
    );
    let updated = skipped.with_updated(OTHER, 11);
    assert_eq!(updated.last_result, Some(AutoResult::Updated));
    assert_eq!(updated.reason_code, None);
}

fn valid_state_json() -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "last_target": TARGET,
        "last_result": "failed",
        "reason_code": "verify_failed",
        "ts": 100,
        "failed_target": TARGET,
        "failed_ts": 100
    })
}

fn with_key(key: &str, value: serde_json::Value) -> Vec<u8> {
    let mut json = valid_state_json();
    json.as_object_mut().unwrap().insert(key.into(), value);
    serde_json::to_vec(&json).unwrap()
}

#[test]
fn valid_state_parses_and_round_trips() {
    let bytes = serde_json::to_vec(&valid_state_json()).unwrap();
    let state = parse_auto_state(&bytes).unwrap();
    assert_eq!(state.last_result, Some(AutoResult::Failed));
    let encoded = encode_auto_state(&state).unwrap();
    assert_eq!(parse_auto_state(&encoded).unwrap(), state);
}

#[test]
fn strict_parser_rejects_bad_shapes() {
    let bad: Vec<Vec<u8>> = vec![
        with_key("extra", 1.into()),
        with_key("v", 2.into()),
        with_key("last_result", "exploded".into()),
        with_key("last_target", "ABC".into()),
        with_key("failed_target", TARGET.to_uppercase().into()),
        with_key("reason_code", "Bad Code".into()),
        with_key("reason_code", "x".repeat(33).into()),
        with_key("ts", "100".into()),
        with_key("failed_ts", serde_json::Value::Null),
        with_key("failed_target", serde_json::Value::Null),
        b"[]".to_vec(),
        b"not json".to_vec(),
        vec![b' '; AUTO_STATE_MAX_BYTES + 1],
    ];
    for bytes in bad {
        assert!(
            parse_auto_state(&bytes).is_err(),
            "{}",
            String::from_utf8_lossy(&bytes)
        );
    }
    let mut missing = valid_state_json();
    missing.as_object_mut().unwrap().remove("ts");
    assert!(parse_auto_state(&serde_json::to_vec(&missing).unwrap()).is_err());
}

#[test]
fn absent_optionals_are_null_and_valid() {
    let state = AutoState {
        ts: 7,
        ..AutoState::default()
    };
    let bytes = encode_auto_state(&state).unwrap();
    assert_eq!(parse_auto_state(&bytes).unwrap(), state);
}

#[test]
fn load_is_none_when_absent_and_save_writes_private_file() {
    let dir = tempfile::tempdir().unwrap();
    assert!(load_auto_state(dir.path()).unwrap().is_none());
    let state = failed_state(42);
    save_auto_state(dir.path(), &state).unwrap();
    let path = dir.path().join(AUTO_STATE_FILE_NAME);
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(load_auto_state(dir.path()).unwrap(), Some(state));
}

#[test]
fn load_refuses_a_symlinked_state_file() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.json");
    fs::write(&real, serde_json::to_vec(&valid_state_json()).unwrap()).unwrap();
    symlink(&real, dir.path().join(AUTO_STATE_FILE_NAME)).unwrap();
    assert!(matches!(
        load_auto_state(dir.path()),
        Err(AutoStateError::NotRegularFile)
    ));
}

#[test]
fn coordinator_guard_recovers_from_a_poisoned_lock() {
    let coordinator = UpdateCoordinator::new();
    std::thread::scope(|scope| {
        let handle = scope.spawn(|| {
            let _held = coordinator.guard();
            panic!("holder dies while holding the coordinator");
        });
        assert!(handle.join().is_err());
    });
    // Still usable after the holder panicked.
    let _again = coordinator.guard();
}
