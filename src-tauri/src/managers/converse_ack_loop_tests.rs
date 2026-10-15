//! Loop-level acknowledgement tests (WP-looptests). Authoritative spec: do not edit to pass.
//! Included from converse.rs as `mod ack_loop_tests`. Smart acknowledgement = `converse_ack`
//! (`SessionConfig::smart_ack` + `ack_after` as the enable flag).

#[allow(unused_imports)]
use super::*;
use crate::managers::converse_ack::{self as ack, AckKind};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::watch;

type Shared = Arc<Mutex<Vec<String>>>;
/// Set when a brain request starts; pushes are time-stamped relative to it.
type Clock = Arc<Mutex<Option<Instant>>>;

#[derive(Clone, Debug)]
struct Push {
    text: String,
    /// Milliseconds since the brain request started.
    ms: u128,
}

fn log_of(l: &Shared) -> Vec<String> {
    l.lock().unwrap().clone()
}
fn has(l: &Shared, e: &str) -> bool {
    log_of(l).iter().any(|x| x == e)
}
fn count_prefix(l: &Shared, p: &str) -> usize {
    log_of(l).iter().filter(|x| x.starts_with(p)).count()
}

fn ms_since(clock: &Clock) -> u128 {
    match *clock.lock().unwrap() {
        Some(t) => t.elapsed().as_millis(),
        None => 0,
    }
}

fn all_ack_texts() -> Vec<&'static str> {
    [
        AckKind::Revise,
        AckKind::Draft,
        AckKind::Action,
        AckKind::Question,
        AckKind::Other,
    ]
    .into_iter()
    .flat_map(|k| ack::pool(k).iter().copied())
    .collect()
}

fn is_ack_text(text: &str) -> bool {
    all_ack_texts().contains(&text)
}

// ---- fakes -------------------------------------------------------------------------------

struct AckQueue {
    log: Shared,
    clock: Clock,
    all: Arc<Mutex<Vec<Push>>>,
}
impl SpeechQueue for AckQueue {
    fn push(&self, text: &str, _emotion: Emotion) -> bool {
        let ms = ms_since(&self.clock);
        self.all.lock().unwrap().push(Push {
            text: text.to_string(),
            ms,
        });
        self.log.lock().unwrap().push(format!("push:{text}"));
        true
    }
    fn abort(&self) {
        self.log.lock().unwrap().push("abort".into());
    }
    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        self.log.lock().unwrap().push("finish".into());
        Box::pin(async { SayOutcome::Done })
    }
}

struct AckMouth {
    log: Shared,
    clock: Clock,
    all: Arc<Mutex<Vec<Push>>>,
    opens: AtomicUsize,
}
impl Mouth for AckMouth {
    fn say<'a>(&'a self, text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        self.log.lock().unwrap().push(format!("say:{text}"));
        Box::pin(async { SayOutcome::Done })
    }
    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }
    fn stop(&self) {
        self.log.lock().unwrap().push("stop".into());
    }
    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.log.lock().unwrap().push("open".into());
        Some(Arc::new(AckQueue {
            log: self.log.clone(),
            clock: self.clock.clone(),
            all: self.all.clone(),
        }))
    }
}

struct Turn {
    /// (delay before delivering, raw delta)
    deltas: Vec<(u64, String)>,
    /// Delay after the last delta before the plan resolves.
    final_delay: u64,
    plan: Result<TurnPlan, PlanError>,
}

struct AckBrain {
    log: Shared,
    clock: Clock,
    script: Mutex<VecDeque<Turn>>,
    histories: Mutex<Vec<Vec<(String, String)>>>,
    last_drafts: Mutex<Vec<Option<String>>>,
}
impl AckBrain {
    fn record(&self, input: &PlanInput<'_>) {
        *self.clock.lock().unwrap() = Some(Instant::now());
        self.histories.lock().unwrap().push(input.history.to_vec());
        self.last_drafts
            .lock()
            .unwrap()
            .push(input.last_draft.map(str::to_string));
    }
    fn next(&self) -> Option<Turn> {
        self.script.lock().unwrap().pop_front()
    }
}
impl Brain for AckBrain {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.record(&input);
        let t = self.next();
        Box::pin(async move {
            match t {
                Some(t) => {
                    tokio::time::sleep(Duration::from_millis(t.final_delay)).await;
                    t.plan
                }
                None => Err(PlanError::Backend),
            }
        })
    }
    fn plan_streaming<'a>(
        &'a self,
        input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.record(&input);
        let t = self.next();
        let log = self.log.clone();
        Box::pin(async move {
            let Some(t) = t else {
                return Err(PlanError::Backend);
            };
            for (delay, delta) in t.deltas {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                on_text(&delta);
            }
            tokio::time::sleep(Duration::from_millis(t.final_delay)).await;
            log.lock().unwrap().push("plan_done".into());
            t.plan
        })
    }
}

struct AckEars {
    script: Mutex<VecDeque<String>>,
    log: Shared,
}
impl Ears for AckEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.log.lock().unwrap().push("listen".into());
        let n = self.script.lock().unwrap().pop_front();
        Box::pin(async move {
            match n {
                Some(u) => ListenOutcome::Utterance(u),
                None => ListenOutcome::Aborted,
            }
        })
    }
}

struct AckHands {
    log: Shared,
}
impl Hands for AckHands {
    fn paste_generated(&self, text: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("paste:{text}"));
        Ok(())
    }
    fn run_user_phrase<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
    fn run_model_command<'a>(&'a self, p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        self.log.lock().unwrap().push(format!("model:{p}"));
        Box::pin(async { PhraseOutcome::Done("ok".into()) })
    }
}

struct AckReporter;
impl Reporter for AckReporter {
    fn state(&self, _s: SessionState) {}
}

struct Rig {
    log: Shared,
    clock: Clock,
    pushes: Arc<Mutex<Vec<Push>>>,
    cancel: Arc<watch::Sender<bool>>,
    ears: AckEars,
    mouth: AckMouth,
    hands: AckHands,
    brain: AckBrain,
    reporter: AckReporter,
}

fn rig(utterances: &[&str], turns: Vec<Turn>) -> Rig {
    let log: Shared = Arc::new(Mutex::new(vec![]));
    let clock: Clock = Arc::new(Mutex::new(None));
    let pushes = Arc::new(Mutex::new(vec![]));
    let (tx, _rx) = watch::channel(false);
    Rig {
        ears: AckEars {
            script: Mutex::new(utterances.iter().map(|s| s.to_string()).collect()),
            log: log.clone(),
        },
        mouth: AckMouth {
            log: log.clone(),
            clock: clock.clone(),
            all: pushes.clone(),
            opens: AtomicUsize::new(0),
        },
        hands: AckHands { log: log.clone() },
        brain: AckBrain {
            log: log.clone(),
            clock: clock.clone(),
            script: Mutex::new(turns.into()),
            histories: Mutex::new(vec![]),
            last_drafts: Mutex::new(vec![]),
        },
        reporter: AckReporter,
        cancel: Arc::new(tx),
        pushes,
        clock,
        log,
    }
}

/// Smart acknowledgement on (the `ack_after` value is only the enable flag).
fn smart(early: bool) -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        early_speech: early,
        ack_after: Some(Duration::from_millis(10)),
        smart_ack: true,
        ..SessionConfig::default()
    }
}

/// Legacy static acknowledgement (`MSG_ACK`).
fn legacy(ack_after_ms: u64) -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        early_speech: false,
        ack_after: Some(Duration::from_millis(ack_after_ms)),
        smart_ack: false,
        ..SessionConfig::default()
    }
}

async fn run(r: &Rig, cfg: &SessionConfig) -> SessionEnd {
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &r.ears,
        mouth: &r.mouth,
        hands: &r.hands,
        brain: &r.brain,
        reporter: &r.reporter,
    };
    let token = r.cancel.subscribe();
    tokio::time::timeout(Duration::from_secs(10), run_session(&deps, cfg, token))
        .await
        .expect("run_session hung")
}

fn plan(kind: PlanKind, speech: &str) -> TurnPlan {
    TurnPlan {
        job: None,
        kind,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
    }
}

/// A brain turn with no streamed deltas that resolves with `p` after `final_delay` ms.
fn slow(final_delay: u64, p: TurnPlan) -> Turn {
    Turn {
        deltas: vec![],
        final_delay,
        plan: Ok(p),
    }
}

fn end_turn() -> Turn {
    slow(0, plan(PlanKind::End, "Goodbye."))
}

/// Every push that is an acknowledgement (pool text), in order.
fn ack_pushes(r: &Rig) -> Vec<Push> {
    r.pushes
        .lock()
        .unwrap()
        .iter()
        .filter(|p| is_ack_text(&p.text))
        .cloned()
        .collect()
}

fn all_pushes(r: &Rig) -> Vec<Push> {
    r.pushes.lock().unwrap().clone()
}

// ---- tests ---------------------------------------------------------------------------------

/// AL1: an action utterance with a 1000 ms brain gets a fast action acknowledgement first.
#[tokio::test]
async fn al1_slow_action_turn_gets_a_fast_action_acknowledgement() {
    let r = rig(
        &["open safari"],
        vec![slow(1000, plan(PlanKind::Answer, "Opening Safari."))],
    );
    let _ = run(&r, &smart(false)).await;
    let pushes = all_pushes(&r);
    let first = pushes.first().expect("no push at all").clone();
    assert!(
        ack::pool(AckKind::Action).contains(&first.text.as_str()),
        "first push {:?} is not an action acknowledgement",
        first.text
    );
    assert_ne!(first.text, "One moment.");
    assert!(
        first.ms >= 500 && first.ms < 900,
        "ack pushed at {} ms",
        first.ms
    );
}

/// AL2: a question with a 1000 ms brain is under the 1500 ms question delay: no ack.
#[tokio::test]
async fn al2_fast_question_gets_no_acknowledgement() {
    let r = rig(
        &["why is the sky blue"],
        vec![slow(1000, plan(PlanKind::Answer, "Rayleigh scattering."))],
    );
    let _ = run(&r, &smart(false)).await;
    assert!(ack_pushes(&r).is_empty(), "{:?}", all_pushes(&r));
    assert!(has(&r.log, "push:Rayleigh scattering.") || has(&r.log, "say:Rayleigh scattering."));
}

/// AL3: the same question with a 2000 ms brain gets exactly one question acknowledgement.
#[tokio::test]
async fn al3_slow_question_gets_exactly_one_question_acknowledgement() {
    let r = rig(
        &["why is the sky blue"],
        vec![slow(2000, plan(PlanKind::Answer, "Rayleigh scattering."))],
    );
    let _ = run(&r, &smart(false)).await;
    let acks = ack_pushes(&r);
    assert_eq!(acks.len(), 1, "{:?}", all_pushes(&r));
    assert!(
        ack::pool(AckKind::Question).contains(&acks[0].text.as_str()),
        "{:?} is not a question acknowledgement",
        acks[0].text
    );
    assert!(acks[0].ms >= 1400, "ack pushed at {} ms", acks[0].ms);
}

/// AL4: two slow turns with the same utterance: the second acknowledgement differs (last_ack).
#[tokio::test]
async fn al4_consecutive_identical_slow_turns_get_different_acknowledgements() {
    let r = rig(
        &["open safari", "open safari"],
        vec![
            slow(1000, plan(PlanKind::Answer, "Opening Safari.")),
            slow(1000, plan(PlanKind::Answer, "Opening Safari again.")),
        ],
    );
    let _ = run(&r, &smart(false)).await;
    let acks = ack_pushes(&r);
    assert_eq!(acks.len(), 2, "{:?}", all_pushes(&r));
    assert_ne!(
        acks[0].text, acks[1].text,
        "the second acknowledgement repeated the first"
    );
}

/// AL5: once a sentence is queued (~300 ms), no acknowledgement follows, even for an action utterance.
#[tokio::test]
async fn al5_no_acknowledgement_once_a_sentence_is_queued() {
    let json = r#"{"kind":"answer","speech":"First sentence is here. Second one follows."}"#;
    let r = rig(
        &["open safari"],
        vec![Turn {
            deltas: vec![(300, json.to_string())],
            final_delay: 800,
            plan: Ok(plan(
                PlanKind::Answer,
                "First sentence is here. Second one follows.",
            )),
        }],
    );
    let _ = run(&r, &smart(true)).await;
    assert!(
        has(&r.log, "push:First sentence is here."),
        "{:?}",
        log_of(&r.log)
    );
    assert!(ack_pushes(&r).is_empty(), "{:?}", all_pushes(&r));
}

/// AL6: smart_ack off, static ack: exactly one legacy "One moment.".
#[tokio::test]
async fn al6_legacy_static_acknowledgement_is_unchanged() {
    let r = rig(
        &["hi"],
        vec![slow(300, plan(PlanKind::Answer, "Hello there."))],
    );
    let _ = run(&r, &legacy(10)).await;
    let ones: Vec<Push> = all_pushes(&r)
        .into_iter()
        .filter(|p| p.text == "One moment.")
        .collect();
    assert_eq!(ones.len(), 1, "{:?}", log_of(&r.log));
}

/// AL7: smart_ack on but ack_after None: no acknowledgement and no queue opened (early off).
#[tokio::test]
async fn al7_smart_ack_without_ack_after_is_silent_and_opens_no_queue() {
    let r = rig(
        &["open safari"],
        vec![slow(1000, plan(PlanKind::Answer, "Opening Safari."))],
    );
    let cfg = SessionConfig {
        post_speech_guard: Duration::from_millis(0),
        quiet_wait: Duration::from_millis(5),
        early_speech: false,
        ack_after: None,
        smart_ack: true,
        ..SessionConfig::default()
    };
    let _ = run(&r, &cfg).await;
    assert!(all_pushes(&r).is_empty(), "{:?}", all_pushes(&r));
    assert_eq!(
        r.mouth.opens.load(Ordering::SeqCst),
        0,
        "no queue without a reason"
    );
    assert!(has(&r.log, "say:Opening Safari."));
}

/// AL8: after a paste sets the draft, "make it shorter" with a slow brain gets a Revise acknowledgement.
#[tokio::test]
async fn al8_revision_with_a_draft_gets_a_revise_acknowledgement() {
    let paste = TurnPlan {
        job: None,
        paste_text: Some("Dogs are loyal companions.".into()),
        ..plan(PlanKind::Paste, "")
    };
    let r = rig(
        &["write a note about dogs", "make it shorter"],
        vec![
            slow(0, paste),
            slow(1000, plan(PlanKind::Answer, "Dogs are loyal.")),
        ],
    );
    let _ = run(&r, &smart(false)).await;
    let drafts = r.brain.last_drafts.lock().unwrap().clone();
    assert_eq!(
        drafts.get(1).cloned().flatten().as_deref(),
        Some("Dogs are loyal companions.")
    );
    let acks = ack_pushes(&r);
    assert_eq!(acks.len(), 1, "{:?}", all_pushes(&r));
    assert!(
        ack::pool(AckKind::Revise).contains(&acks[0].text.as_str()),
        "{:?} is not a revise acknowledgement",
        acks[0].text
    );
}

/// AL9: the acknowledgement is never spoken text: a slow action turn whose plan is a normal answer
/// still reconciles (nothing aborted), and the history records the plan speech only.
#[tokio::test]
async fn al9_acknowledgement_is_not_history_nor_spoken_answer_text() {
    let r = rig(
        &["open safari", "thanks"],
        vec![
            slow(1000, plan(PlanKind::Answer, "Opening Safari now.")),
            end_turn(),
        ],
    );
    let end = run(&r, &smart(false)).await;
    assert_eq!(end, SessionEnd::UserEnded);
    assert!(!has(&r.log, "abort"), "{:?}", log_of(&r.log));
    assert!(
        has(&r.log, "push:Opening Safari now."),
        "{:?}",
        log_of(&r.log)
    );
    let h = r.brain.histories.lock().unwrap().clone();
    assert_eq!(h.len(), 2);
    assert_eq!(h[1].len(), 1, "{:?}", h[1]);
    assert_eq!(h[1][0].0, "open safari");
    assert_eq!(h[1][0].1, "Opening Safari now.");
    assert!(!h[1].iter().any(|(u, s)| is_ack_text(u) || is_ack_text(s)));
}

/// AL10: from_settings maps the realtime master + ack toggle to smart_ack / ack_after.
#[test]
fn al10_from_settings_enables_smart_ack_only_with_master_and_ack() {
    let mut s = crate::settings::get_default_settings();
    s.exp_rt_enabled = true;
    s.exp_rt_ack = true;
    let c = SessionConfig::from_settings(&s);
    assert!(c.smart_ack, "master + ack on => smart_ack");
    assert!(c.ack_after.is_some(), "master + ack on => ack_after");

    s.exp_rt_enabled = false;
    let c = SessionConfig::from_settings(&s);
    assert!(!c.smart_ack, "master off => no smart_ack");
    assert!(c.ack_after.is_none(), "master off => no ack_after");
}

/// AL11: a Draft acknowledgement is spoken, then the final plan is a paste: the queue is aborted
/// after the acknowledgement and before the paste, the paste happens once, "Done." is spoken via
/// `say`, and the next turn's history records the paste note.
#[tokio::test]
async fn al11_ack_then_paste_plan_aborts_the_queue_pastes_and_records_the_pasted_note() {
    use crate::managers::converse_context::{compose, note_paste};
    let pasted = "Hi team, the release is delayed.";
    let paste = TurnPlan {
        job: None,
        paste_text: Some(pasted.into()),
        ..plan(PlanKind::Paste, "Done.")
    };
    let r = rig(
        &["write a short note", "thanks"],
        vec![slow(1000, paste), end_turn()],
    );
    let end = run(&r, &smart(true)).await;
    assert_eq!(end, SessionEnd::UserEnded);

    let acks = ack_pushes(&r);
    assert_eq!(acks.len(), 1, "{:?}", all_pushes(&r));
    assert!(
        ack::pool(AckKind::Draft).contains(&acks[0].text.as_str()),
        "{:?} is not a draft acknowledgement",
        acks[0].text
    );

    let log = log_of(&r.log);
    let ack_idx = log
        .iter()
        .position(|x| *x == format!("push:{}", acks[0].text))
        .expect("acknowledgement not in log");
    let abort_idx = log
        .iter()
        .position(|x| x == "abort")
        .expect("queue was not aborted");
    let paste_idx = log
        .iter()
        .position(|x| x.starts_with("paste:"))
        .expect("no paste");
    let done_idx = log
        .iter()
        .position(|x| x == "say:Done.")
        .expect("confirmation not spoken via say");
    assert!(
        ack_idx < abort_idx && abort_idx < paste_idx && paste_idx < done_idx,
        "{:?}",
        log
    );
    assert_eq!(count_prefix(&r.log, "paste:"), 1, "{:?}", log);
    assert!(has(&r.log, &format!("paste:{pasted}")), "{:?}", log);

    let h = r.brain.histories.lock().unwrap().clone();
    assert_eq!(h.len(), 2);
    assert_eq!(h[1].len(), 1, "{:?}", h[1]);
    assert_eq!(
        h[1][0],
        (
            "write a short note".to_string(),
            compose("Done.", &note_paste(pasted))
        )
    );
}

/// AL12: a Draft-less action utterance acknowledged with an Action phrase, then the final plan is a
/// command: the command runs once, the queue is aborted before it, the spoken plan speech is used,
/// and the next turn's history records the command note with the `Done` outcome.
#[tokio::test]
async fn al12_ack_then_command_plan_runs_the_command_and_records_the_action_note() {
    use crate::managers::converse_context::{compose, note_command, CommandNote};
    /// Hands whose model-command path reports `Done("Opened Safari")`.
    struct CmdHands {
        log: Shared,
    }
    impl Hands for CmdHands {
        fn paste_generated(&self, text: &str) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("paste:{text}"));
            Ok(())
        }
        fn run_user_phrase<'a>(&'a self, _p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            Box::pin(async { PhraseOutcome::NoMatch })
        }
        fn run_model_command<'a>(&'a self, p: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            self.log.lock().unwrap().push(format!("model:{p}"));
            Box::pin(async { PhraseOutcome::Done("Opened Safari".into()) })
        }
    }

    let cmd = "open -a Safari";
    let speech = "Opening Safari.";
    let command_plan = TurnPlan {
        job: None,
        command: Some(cmd.into()),
        ..plan(PlanKind::Command, speech)
    };
    let r = rig(
        &["open safari", "thanks"],
        vec![slow(1000, command_plan), end_turn()],
    );
    let hands = CmdHands { log: r.log.clone() };
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &r.ears,
        mouth: &r.mouth,
        hands: &hands,
        brain: &r.brain,
        reporter: &r.reporter,
    };
    let token = r.cancel.subscribe();
    let end = tokio::time::timeout(
        Duration::from_secs(10),
        run_session(&deps, &smart(true), token),
    )
    .await
    .expect("run_session hung");
    assert_eq!(end, SessionEnd::UserEnded);

    let acks = ack_pushes(&r);
    assert_eq!(acks.len(), 1, "{:?}", all_pushes(&r));
    assert!(
        ack::pool(AckKind::Action).contains(&acks[0].text.as_str()),
        "{:?} is not an action acknowledgement",
        acks[0].text
    );

    let log = log_of(&r.log);
    let ack_idx = log
        .iter()
        .position(|x| *x == format!("push:{}", acks[0].text))
        .expect("acknowledgement not in log");
    let abort_idx = log
        .iter()
        .position(|x| x == "abort")
        .expect("queue was not aborted");
    let cmd_idx = log
        .iter()
        .position(|x| x.starts_with("model:"))
        .expect("command did not run");
    assert!(ack_idx < abort_idx && abort_idx < cmd_idx, "{:?}", log);
    assert_eq!(count_prefix(&r.log, "model:"), 1, "{:?}", log);
    assert!(has(&r.log, &format!("model:{cmd}")), "{:?}", log);
    assert!(has(&r.log, &format!("say:{speech}")), "{:?}", log);

    let h = r.brain.histories.lock().unwrap().clone();
    assert_eq!(h.len(), 2);
    assert_eq!(h[1].len(), 1, "{:?}", h[1]);
    assert_eq!(
        h[1][0],
        (
            "open safari".to_string(),
            compose(
                speech,
                &note_command(cmd, &CommandNote::Done("Opened Safari".into()))
            )
        )
    );
}

// ---- END ----
