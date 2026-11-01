//! Tests for B2b-1 runner behaviour: the `waiting` phase (R7-1), the three-valued
//! launchd probe (R9-2) and the resting reconcile rule (R7-4, R9-3). Wired from
//! `source_update_runner` with `#[path]`.

use super::*;
use crate::managers::source_update_runner_poll::{
    poll_step, Effect, PollInput, PollState, UNREADABLE_AFTER_READS,
};
use crate::managers::source_update_runner_run::{adopt_plan, AdoptPlan};

const RUN16: &str = "0123456789abcdef";
const FULL40: &str = "0123456789abcdef0123456789abcdef01234567";
const NOW_UNIX: u64 = 1_800_000_000;

fn status(phase: RunPhase, percent: u8) -> RunStatus {
    RunStatus {
        run_id: RUN16.to_string(),
        phase,
        percent,
        target: "0123456".to_string(),
        target_full: FULL40.to_string(),
        message: String::new(),
        method: StatusMethod::Exact,
        ts: NOW_UNIX,
    }
}

fn live() -> JobProbe {
    JobProbe::Registered {
        pid: Some(77),
        last_exit: None,
    }
}

fn registered_idle() -> JobProbe {
    JobProbe::Registered {
        pid: None,
        last_exit: Some(0),
    }
}

fn reconcile_with(
    status: Option<RunStatus>,
    age: u64,
    job: JobProbe,
    pending_built: bool,
    published: bool,
) -> Decision {
    reconcile(&ReconcileInput {
        status,
        status_age_secs: age,
        job,
        lock: LockState::Free,
        running_build_sha: FULL40,
        published,
        interrupted_after_secs: INTERRUPTED_AFTER_SECS,
        pending_built,
    })
}

fn waiting_bytes(percent: u32) -> Vec<u8> {
    format!(
        r#"{{"v":1,"run_id":"{RUN16}","phase":"waiting","percent":{percent},"target":"0123456","target_full":"{FULL40}","message":"","method":"exact","ts":{NOW_UNIX}}}"#
    )
    .into_bytes()
}

#[test]
fn waiting_is_resting_not_terminal_and_serialises_lowercase() {
    assert!(!RunPhase::Waiting.is_terminal());
    assert_eq!(
        serde_json::to_string(&RunPhase::Waiting).ok().as_deref(),
        Some("\"waiting\"")
    );
}

#[test]
fn waiting_status_accepts_only_the_band_88_to_91_and_never_100() {
    for percent in [88u32, 91] {
        let parsed = parse_status(&waiting_bytes(percent), RUN16, None, NOW_UNIX);
        assert_eq!(parsed.map(|s| s.phase), Ok(RunPhase::Waiting), "{percent}");
    }
    for percent in [0u32, 87, 92, 99, 100] {
        let parsed = parse_status(&waiting_bytes(percent), RUN16, None, NOW_UNIX);
        assert_eq!(
            parsed.map(|_| ()),
            Err(StatusError::PercentNotAllowed),
            "{percent}"
        );
    }
}

#[test]
fn waiting_sits_between_bundling_and_installing_in_the_forward_order() {
    let compiling = status(RunPhase::Compiling, 60);
    let parsed = parse_status(&waiting_bytes(90), RUN16, Some(&compiling), NOW_UNIX);
    assert_eq!(parsed.map(|s| s.phase), Ok(RunPhase::Waiting));

    let waiting = status(RunPhase::Waiting, 90);
    let installing = format!(
        r#"{{"v":1,"run_id":"{RUN16}","phase":"installing","percent":92,"target":"0123456","target_full":"{FULL40}","message":"","method":"exact","ts":{NOW_UNIX}}}"#
    );
    let forward = parse_status(installing.as_bytes(), RUN16, Some(&waiting), NOW_UNIX);
    assert_eq!(forward.map(|s| s.phase), Ok(RunPhase::Installing));

    let back = parse_status(
        &waiting_bytes(90),
        RUN16,
        Some(&status(RunPhase::Installing, 92)),
        NOW_UNIX,
    );
    assert_eq!(back.map(|_| ()), Err(StatusError::PhaseRegress));
}

#[test]
fn launchctl_output_without_recognised_lines_is_unknown_never_absent() {
    assert_eq!(parse_launchctl_print("hello world"), JobProbe::Unknown);
    assert_eq!(parse_launchctl_print(""), JobProbe::Unknown);
}

#[test]
fn unknown_is_neither_live_nor_absent() {
    assert!(!JobProbe::Unknown.is_live());
    assert!(JobProbe::Unknown.is_unknown());
    assert!(!JobProbe::Absent.is_unknown());
}

#[test]
fn reconcile_unknown_never_interrupts_or_cleans_up_in_any_state() {
    let statuses = [
        None,
        Some(status(RunPhase::Compiling, 50)),
        Some(status(RunPhase::Waiting, 90)),
        Some(status(RunPhase::Failed, 40)),
        Some(status(RunPhase::Done, 100)),
    ];
    for maybe in statuses {
        for age in [0u64, INTERRUPTED_AFTER_SECS * 10] {
            for pending in [false, true] {
                for published in [false, true] {
                    let decision =
                        reconcile_with(maybe.clone(), age, JobProbe::Unknown, pending, published);
                    assert!(
                        !matches!(decision, Decision::MarkInterrupted(_) | Decision::CleanupFinished | Decision::Rest),
                        "{decision:?} for {maybe:?} age {age} pending {pending} published {published}"
                    );
                }
            }
        }
    }
}

#[test]
fn reconcile_waiting_with_valid_built_pending_and_no_process_rests() {
    let waiting = Some(status(RunPhase::Waiting, 90));
    assert_eq!(
        reconcile_with(waiting.clone(), 0, JobProbe::Absent, true, false),
        Decision::Rest
    );
    assert_eq!(
        reconcile_with(waiting.clone(), 99_999, registered_idle(), true, false),
        Decision::Rest,
        "resting ignores age"
    );
    assert_eq!(
        reconcile_with(waiting, 0, live(), true, false),
        Decision::Adopt,
        "a live process is always adopted"
    );
}

#[test]
fn reconcile_waiting_without_pending_is_stale_only_after_fifteen_minutes() {
    let waiting = Some(status(RunPhase::Waiting, 90));
    assert_eq!(
        reconcile_with(
            waiting.clone(),
            INTERRUPTED_AFTER_SECS - 1,
            JobProbe::Absent,
            false,
            false
        ),
        Decision::None
    );
    assert!(matches!(
        reconcile_with(
            waiting.clone(),
            INTERRUPTED_AFTER_SECS,
            JobProbe::Absent,
            false,
            false
        ),
        Decision::MarkInterrupted(_)
    ));
    assert_eq!(
        reconcile_with(waiting, 99_999, live(), false, false),
        Decision::Adopt
    );
}

#[test]
fn reconcile_rest_only_applies_to_waiting() {
    let installing = Some(status(RunPhase::Installing, 92));
    assert!(matches!(
        reconcile_with(
            installing,
            INTERRUPTED_AFTER_SECS,
            JobProbe::Absent,
            true,
            false
        ),
        Decision::MarkInterrupted(_)
    ));
}

#[test]
fn adoption_of_a_resting_run_watches_nothing_and_cleans_nothing() {
    let input = ReconcileInput {
        status: Some(status(RunPhase::Waiting, 90)),
        status_age_secs: 5,
        job: JobProbe::Absent,
        lock: LockState::Free,
        running_build_sha: FULL40,
        published: false,
        interrupted_after_secs: INTERRUPTED_AFTER_SECS,
        pending_built: true,
    };
    assert_eq!(adopt_plan(&input, true), AdoptPlan::Nothing);
}

fn poll_input<'a>(
    bytes: Option<&'a [u8]>,
    age: u64,
    job: JobProbe,
    pending: bool,
    now_ms: u64,
) -> PollInput<'a> {
    PollInput {
        status_bytes: bytes,
        status_age_secs: age,
        job,
        lock: LockState::Free,
        running_build_sha: FULL40,
        pending_built: pending,
        now_unix: NOW_UNIX,
        now_ms,
    }
}

#[test]
fn poll_resting_build_cleans_its_job_then_stops_without_outcome() {
    let bytes = waiting_bytes(90);
    let mut state = PollState::new(RUN16);
    let effects = poll_step(
        &mut state,
        &poll_input(Some(&bytes), 3, JobProbe::Absent, true, 0),
    );
    let cleanup = effects.iter().position(|e| *e == Effect::Cleanup);
    let stop = effects.iter().position(|e| *e == Effect::Stop);
    assert!(
        cleanup.is_some(),
        "the finished build job is booted out and its plist removed"
    );
    assert!(
        stop.is_some() && cleanup < stop,
        "cleanup comes before the stop"
    );
    assert!(!effects.iter().any(|e| matches!(
        e,
        Effect::WriteStatus(_) | Effect::PublishFailed(_) | Effect::PublishDone(_)
    )));
}

#[test]
fn poll_unknown_probe_never_cleans_up_stops_or_marks_interrupted() {
    let bytes = b"not a status".to_vec();
    let mut state = PollState::new(RUN16);
    for tick in 0..40u64 {
        let effects = poll_step(
            &mut state,
            &poll_input(
                Some(&bytes),
                INTERRUPTED_AFTER_SECS * 10,
                JobProbe::Unknown,
                false,
                tick * 500,
            ),
        );
        assert!(
            !effects.iter().any(|e| matches!(
                e,
                Effect::Cleanup | Effect::Stop | Effect::WriteStatus(_) | Effect::PublishFailed(_)
            )),
            "tick {tick}: {effects:?}"
        );
    }
}

#[test]
fn poll_unknown_probe_does_not_fail_a_run_as_unreadable() {
    let mut state = PollState::new(RUN16);
    let bytes: Option<&[u8]> = None;
    for tick in 0..(UNREADABLE_AFTER_READS as u64 + 10) {
        let effects = poll_step(
            &mut state,
            &poll_input(bytes, 0, JobProbe::Unknown, false, tick * 500),
        );
        assert!(!effects
            .iter()
            .any(|e| matches!(e, Effect::PublishFailed(_) | Effect::Cleanup)));
    }
}

#[test]
fn pure_match_on_decision_is_exhaustive_with_rest() {
    fn audit(decision: Decision) -> bool {
        match decision {
            Decision::None
            | Decision::Adopt
            | Decision::PublishDone
            | Decision::PublishFailed(_)
            | Decision::MarkInterrupted(_)
            | Decision::CleanupFinished
            | Decision::Rest => true,
        }
    }
    assert!(audit(Decision::Rest));
}
