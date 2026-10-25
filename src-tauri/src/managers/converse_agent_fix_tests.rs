//! Regression tests for two validator MAJOR findings on the agent hand-off (DESIGN-PR2-STAGE2
//! Revision 3, sections 2.3 and 2.5). Written before the production fix (step C2c): they must
//! compile now and fail on behaviour until the fix lands.
//!
//! (1) `max_turns` counts USER turns only (design 2.3). Today `turns += 1` runs before every
//!     listen, and only the Ignored branch undoes it, so silence while a run is occupied (or
//!     TooLong outcomes) exhausts the budget and ends the session with TurnLimit.
//! (2) A panic must not leave an agent run going. The slot has no Drop and `end_agent_session`
//!     runs only on a normal return. `AgentSlot::new` is private to `converse_agent_loop`, so a
//!     unit test cannot build a slot without a production change; both checks are black-box.
//!
//! Reuses the rig, fakes and helpers of converse_agent_loop_tests.rs. Included from converse.rs.
//! Do not edit to make code pass.
#![allow(dead_code)]

use super::converse_agent_loop_tests::*;
#[allow(unused_imports)]
use super::*;
use crate::managers::converse_agent::{AgentEvent, AgentPoll};
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicUsize, Ordering};

/// `n` scripted silences.
fn silences(n: usize) -> Vec<Step> {
    (0..n).map(|_| silent()).collect()
}

/// A step that, when its listen starts, makes the pending run finish with `RESULT`.
fn finish_run_here(run_ctl: &Arc<RunCtl>) -> Step {
    let c = run_ctl.clone();
    with_hook(silent(), move || {
        c.push(AgentPoll::Event(AgentEvent::Done(RESULT.to_string())));
    })
}

/// (1a) Six silent listens while the run is pending do not use the turn budget (max_turns = 3).
/// The result arrives on the sixth; the user then says thanks and stop. The session ends with the
/// normal End, not TurnLimit, and the result is spoken.
#[tokio::test]
async fn fix_silence_while_a_run_is_pending_does_not_use_the_turn_budget() {
    let run_ctl = RunCtl::new();
    let mut steps = vec![heard(REQ)];
    steps.extend(silences(5));
    steps.push(finish_run_here(&run_ctl));
    steps.push(heard("thanks"));
    steps.push(heard("stop"));
    let r = rig_with(
        steps,
        vec![agent_turn(TASK, ACK_TEXT), answer("You're welcome.")],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    let cfg = SessionConfig {
        max_turns: 3,
        ..acfg()
    };
    let end = run(&r, &cfg).await;
    assert_eq!(
        end,
        SessionEnd::UserEnded,
        "silence must not end the session by turn limit: {:?}",
        entries(&r)
    );
    let said = spoken(&r);
    assert!(
        said.iter().any(|s| s == RESULT),
        "the result was spoken: {said:?}"
    );
    assert_eq!(published(&r), vec![RESULT.to_string()]);
}

/// (1b) Alternating TooLong and silence while the run is pending does not use the turn budget
/// either, and the two-in-a-row TooLong shutdown is not triggered by the alternation.
#[tokio::test]
async fn fix_too_long_while_a_run_is_pending_does_not_use_the_turn_budget() {
    let run_ctl = RunCtl::new();
    let steps = vec![
        heard(REQ),
        outcome(ListenOutcome::TooLong),
        silent(),
        outcome(ListenOutcome::TooLong),
        silent(),
        outcome(ListenOutcome::TooLong),
        finish_run_here(&run_ctl),
        heard("thanks"),
        heard("stop"),
    ];
    let r = rig_with(
        steps,
        vec![agent_turn(TASK, ACK_TEXT), answer("You're welcome.")],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    let cfg = SessionConfig {
        max_turns: 3,
        ..acfg()
    };
    let end = run(&r, &cfg).await;
    assert_eq!(
        end,
        SessionEnd::UserEnded,
        "too-long outcomes must not end the session by turn limit: {:?}",
        entries(&r)
    );
    assert!(
        spoken(&r).iter().any(|s| s == RESULT),
        "the result was spoken"
    );
}

/// (1c) Guard for the old behaviour: with NO run pending, silence still uses the budget and the
/// session ends by TurnLimit after exactly max_turns listens (idle is set high so it cannot win).
#[tokio::test]
async fn fix_silence_without_a_run_still_ends_by_turn_limit() {
    let r = rig_with(silences(10), vec![]);
    let cfg = SessionConfig {
        max_turns: 3,
        max_idle_turns: 5,
        ..acfg()
    };
    let end = run(&r, &cfg).await;
    assert_eq!(end, SessionEnd::TurnLimit);
    assert_eq!(listens(&r), 3, "three listens, then the limit");
}

/// (1d) User utterances still count against the budget while a run is pending: with max_turns = 2,
/// the second user turn is the last one, and the third listen is never made.
#[tokio::test]
async fn fix_user_turns_still_count_while_a_run_is_pending() {
    let run_ctl = RunCtl::new();
    let r = rig_with(
        vec![
            heard(REQ),
            heard("thanks"),
            heard("hello there"),
            heard("more"),
        ],
        vec![agent_turn(TASK, ACK_TEXT), answer("You're welcome.")],
    );
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    let cfg = SessionConfig {
        max_turns: 2,
        ..acfg()
    };
    let end = run(&r, &cfg).await;
    assert_eq!(end, SessionEnd::TurnLimit);
    assert_eq!(listens(&r), 2, "the third user turn is never taken");
}

/// A planner that plans the agent once and panics on the next call.
struct PanicOnSecondPlan {
    calls: AtomicUsize,
}

impl Brain for PanicOnSecondPlan {
    fn plan<'a>(&'a self, _input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            Box::pin(async { Ok(agent_plan(TASK, ACK_TEXT)) })
        } else {
            panic!("planner exploded");
        }
    }
}

/// (2b) A panic in the loop while a run is pending cancels that run exactly once. The panic is
/// caught the way converse_runtime.rs catches a session panic (FutureExt::catch_unwind).
#[tokio::test]
async fn fix_a_panic_while_a_run_is_pending_cancels_the_run_once() {
    let r = rig_with(vec![heard(REQ), heard("thanks")], vec![]);
    let run_ctl = queue_run(&r);
    let brain = PanicOnSecondPlan {
        calls: AtomicUsize::new(0),
    };
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &r.ears,
        mouth: &r.mouth,
        hands: &r.hands,
        brain: &brain,
        reporter: &r.reporter,
    };
    let cfg = acfg();
    let token = r.cancel.subscribe();
    let outcome = tokio::time::timeout(
        Duration::from_secs(10),
        AssertUnwindSafe(run_session(&deps, &cfg, token)).catch_unwind(),
    )
    .await
    .expect("run_session hung");
    assert!(outcome.is_err(), "the planner panicked");
    assert_eq!(
        run_ctl.cancels(),
        1,
        "the panic cancels the pending run exactly once"
    );
}

/// (2a, black-box) After the run delivered its terminal event, the session end does not cancel
/// it again: the count stays 0 (never twice, never after a terminal event).
#[tokio::test]
async fn fix_session_end_after_a_terminal_event_does_not_cancel_again() {
    let r = rig_with(
        vec![heard(REQ), heard("thanks"), heard("stop")],
        vec![agent_turn(TASK, ACK_TEXT), answer("You're welcome.")],
    );
    let run_ctl = queue_run(&r);
    run_ctl.push(AgentPoll::Event(AgentEvent::Done(RESULT.to_string())));
    let end = run(&r, &acfg()).await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert_eq!(run_ctl.cancels(), 0, "a finished run is not cancelled");
}

/// `n` foreign-voice listens (`ListenOutcome::Ignored`).
fn ignored(n: usize) -> Vec<Step> {
    (0..n).map(|_| outcome(ListenOutcome::Ignored)).collect()
}

/// (2d-a) Twelve foreign-voice listens in a row while the run is pending (max_turns = 3) do not
/// consume the user-turn budget, do not panic (debug builds) and do not end the session. The result
/// is delivered; the user's stop ends the session normally.
#[tokio::test]
async fn fix_ignored_listens_while_a_run_is_pending_do_not_use_the_turn_budget() {
    let run_ctl = RunCtl::new();
    let mut steps = vec![heard(REQ)];
    steps.extend(ignored(12));
    steps.push(finish_run_here(&run_ctl));
    steps.push(heard("stop"));
    let r = rig_with(steps, vec![agent_turn(TASK, ACK_TEXT)]);
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    let cfg = SessionConfig {
        max_turns: 3,
        ..acfg()
    };
    let end = run(&r, &cfg).await;
    assert_eq!(
        end,
        SessionEnd::UserEnded,
        "foreign voices must not end the session by turn limit: {:?}",
        entries(&r)
    );
    assert!(
        spoken(&r).iter().any(|s| s == RESULT),
        "the result was spoken"
    );
    assert_eq!(published(&r), vec![RESULT.to_string()], "published once");
}

/// (2d-b) Twenty-three foreign-voice listens in runs of six, separated by silence, while the run
/// is pending: the same guarantee as (a) with the streak limit reached several times.
#[tokio::test]
async fn fix_ignored_listens_interleaved_with_silence_do_not_use_the_turn_budget() {
    let run_ctl = RunCtl::new();
    let mut steps = vec![heard(REQ)];
    for i in 0..23 {
        steps.push(outcome(ListenOutcome::Ignored));
        if (i + 1) % 6 == 0 {
            steps.push(silent());
        }
    }
    steps.push(finish_run_here(&run_ctl));
    steps.push(heard("stop"));
    let r = rig_with(steps, vec![agent_turn(TASK, ACK_TEXT)]);
    r.hands
        .starts
        .lock()
        .unwrap()
        .push_back(Ok(run_ctl.clone()));
    let cfg = SessionConfig {
        max_turns: 3,
        ..acfg()
    };
    let end = run(&r, &cfg).await;
    assert_eq!(
        end,
        SessionEnd::UserEnded,
        "interleaved foreign voices must not end the session by turn limit: {:?}",
        entries(&r)
    );
    assert!(
        spoken(&r).iter().any(|s| s == RESULT),
        "the result was spoken"
    );
    assert_eq!(published(&r), vec![RESULT.to_string()], "published once");
}

/// (2d-c) Guard: with NO run pending, five foreign-voice listens in a row count like one NoSpeech
/// at the idle limit: the session ends Idle with the stop line, and the turn budget is unchanged.
#[tokio::test]
async fn fix_five_ignored_without_a_run_end_idle_like_nospeech() {
    let r = rig_with(ignored(5), vec![]);
    let cfg = SessionConfig {
        max_idle_turns: 1,
        max_turns: 3,
        ..acfg()
    };
    let end = run(&r, &cfg).await;
    assert_eq!(end, SessionEnd::Idle, "{:?}", entries(&r));
    assert_eq!(
        spoken(&r).last().map(String::as_str),
        Some(MSG_STOP_LISTENING)
    );
}
