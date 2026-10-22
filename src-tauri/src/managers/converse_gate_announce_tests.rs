//! RED tests (part 3) for the M3 S3b gate: announcer conversation lifetime and turn
//! boundaries (R2-6), confirmation speech arbitration (R2-1), transcript hygiene
//! (D2) and source contracts (`start_job` / `cancel_job` call sites). Child module
//! of `converse_gate_tests`, reusing its fakes.

use super::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Ears that announces one background message from inside `listen`, then returns the
/// scripted outcome. Lets a test place an announcement at a precise point in the loop.
struct AnnouncingEars {
    announcer: Arc<Announcer>,
    text: &'static str,
    ev: Arc<Events>,
    answer: ListenOutcome,
}

impl Ears for AnnouncingEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        self.ev.push("listen");
        self.announcer
            .announce(AnnounceKind::Finish, self.text.to_string());
        let answer = self.answer.clone();
        Box::pin(async move { answer })
    }
}

/// Reporter that keeps every transcript event (D2, DESIGN 8.2).
#[derive(Default)]
struct CaptureReporter(Mutex<Vec<TranscriptEvent>>);

impl Reporter for CaptureReporter {
    fn state(&self, _s: SessionState) {}
    fn transcript(&self, ev: TranscriptEvent) {
        self.0.lock().unwrap().push(ev);
    }
}

/// Builds a low-priority mouth and announcer that record into `ev`.
fn announcer_with(ev: &Arc<Events>) -> Arc<Announcer> {
    Arc::new(Announcer::new(Arc::new(RecordingLowMouth {
        ev: ev.clone(),
    })))
}

// ---------------------------------------------------------- conversation lifetime (R2-6)

/// While the conversation loop runs, an announcement is queued and is spoken through the
/// loop's own mouth at the next turn boundary, in order (R2-6). The first listen announces
/// and answers one utterance; the second listen (a second turn) ends the session, so a full
/// boundary happens after the announce. The probe must not appear as a direct speech before
/// the boundary.
#[tokio::test]
async fn announcement_during_conversation_is_spoken_at_the_turn_boundary() {
    struct TwoTurnEars {
        announcer: Arc<Announcer>,
        ev: Arc<Events>,
        turn: Mutex<usize>,
    }
    impl Ears for TwoTurnEars {
        fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
            self.ev.push("listen");
            let mut turn = self.turn.lock().unwrap();
            *turn += 1;
            let outcome = if *turn == 1 {
                self.announcer
                    .announce(AnnounceKind::Finish, "The coding job finished.".to_string());
                ListenOutcome::Utterance("what time is it".to_string())
            } else {
                ListenOutcome::Utterance("stop listening".to_string())
            };
            Box::pin(async move { outcome })
        }
    }
    let ev = Arc::new(Events::default());
    let announcer = announcer_with(&ev);
    let ears = TwoTurnEars {
        announcer: announcer.clone(),
        ev: ev.clone(),
        turn: Mutex::new(0),
    };
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain {
        plan: answer_plan("It is noon."),
    };
    let hands = GateHands::new(ev.clone());
    let (_tx, rx) = watch::channel(false);
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &NoReporter,
        policy: None,
        announcer: Some(&announcer),
    };
    let _ = run_session(&deps, &gate_cfg(), rx).await;
    let all = ev.all();
    let listens: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, e)| *e == "listen")
        .map(|(i, _)| i)
        .collect();
    assert!(
        listens.len() >= 2,
        "a full turn boundary must occur: {all:?}"
    );
    let spoken_at = all.iter().position(|e| e == "say:The coding job finished.");
    let direct_at = all
        .iter()
        .position(|e| e == "announce:The coding job finished.");
    assert!(
        direct_at.is_none(),
        "the announcement must not be spoken through the direct announcer mouth: {all:?}"
    );
    let spoken_at = spoken_at.unwrap_or(usize::MAX);
    assert!(
        spoken_at > listens[0] && spoken_at < listens[1],
        "the probe must be spoken through the loop mouth between turn 1 and turn 2: {all:?}"
    );
}

/// Ending the session early clears the conversation flag (ruling B(i): flag state only).
/// SET during the loop: a probe announced from inside the listen is queued (take_pending
/// returns it). CLEARED after the end: a fresh announcement is not queued (take_pending
/// is empty). No assertion is made about direct speech after the end.
#[tokio::test]
async fn conversation_flag_is_cleared_on_early_return() {
    let ev = Arc::new(Events::default());
    let announcer = announcer_with(&ev);
    let ears = AnnouncingEars {
        announcer: announcer.clone(),
        text: "probe during early return",
        ev: ev.clone(),
        answer: ListenOutcome::Aborted,
    };
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain {
        plan: answer_plan("x"),
    };
    let hands = GateHands::new(ev.clone());
    let (_tx, rx) = watch::channel(false);
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &NoReporter,
        policy: None,
        announcer: Some(&announcer),
    };
    let _ = run_session(&deps, &gate_cfg(), rx).await;
    let all = ev.all();
    let probe_at = all
        .iter()
        .position(|e| e == "announce:probe during early return");
    let stop_at = all.iter().position(|e| e == "stop");
    assert!(
        probe_at.is_some() && probe_at > stop_at,
        "flag SET: the probe must not be spoken during the loop: {all:?}"
    );
    announcer.announce(AnnounceKind::Finish, "after the end".to_string());
    assert!(
        ev.all().iter().any(|e| e == "announce:after the end"),
        "flag CLEARED: a post-end announcement is spoken directly: {:?}",
        ev.all()
    );
}

/// A panic inside the conversation loop still clears the conversation flag (ruling B(i)).
/// SET before the panic: the probe announced inside the panicking listen is queued
/// (take_pending returns it). CLEARED after the unwind: a new announcement is not queued.
#[test]
fn conversation_flag_is_cleared_when_the_loop_panics() {
    struct AnnounceThenPanic {
        announcer: Arc<Announcer>,
        ev: Arc<Events>,
    }
    impl Ears for AnnounceThenPanic {
        fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
            self.ev.push("listen");
            self.announcer
                .announce(AnnounceKind::Finish, "probe before panic".to_string());
            Box::pin(async { panic!("simulated listen panic") })
        }
    }
    let ev = Arc::new(Events::default());
    let announcer = announcer_with(&ev);
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain {
        plan: answer_plan("x"),
    };
    let hands = GateHands::new(ev.clone());
    let ears = AnnounceThenPanic {
        announcer: announcer.clone(),
        ev: ev.clone(),
    };
    let (_tx, rx) = watch::channel(false);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let deps = Deps {
            ears: &ears,
            mouth: &mouth,
            hands: &hands,
            brain: &brain,
            reporter: &NoReporter,
            policy: None,
            announcer: Some(&*announcer),
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(run_session(&deps, &gate_cfg(), rx))
    }));
    assert!(result.is_err(), "the simulated panic must propagate");
    let all = ev.all();
    // The probe was announced inside the panicking listen. Flag SET means it was queued and
    // not spoken directly, so it can only appear after the loop ended (flush at end).
    let probe_at = all.iter().position(|e| e == "announce:probe before panic");
    assert!(
        probe_at.is_some(),
        "the probe must be flushed after the panic (it was queued while the flag was set): {all:?}"
    );
    assert!(
        probe_at > all.iter().position(|e| e == "listen"),
        "flag SET before the panic: the probe must not be spoken directly during the loop: {all:?}"
    );
    announcer.announce(AnnounceKind::Finish, "after the panic".to_string());
    assert!(
        ev.all().iter().any(|e| e == "announce:after the panic"),
        "flag CLEARED on unwind: a post-unwind announcement is spoken directly: {:?}",
        ev.all()
    );
}

// ---------------------------------------------------------- confirmation arbitration (R2-1)

/// An announcement made while a confirmation question is open is never spoken until the
/// confirmation finishes; it is then flushed.
#[tokio::test]
async fn announcement_during_confirmation_is_held_until_the_guard_drops() {
    let ev = Arc::new(Events::default());
    let announcer = announcer_with(&ev);
    let ears = AnnouncingEars {
        announcer: announcer.clone(),
        text: "Job done during the question.",
        ev: ev.clone(),
        answer: ListenOutcome::Utterance("no".to_string()),
    };
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain {
        plan: paste_plan("x"),
    };
    let hands = GateHands::new(ev.clone());
    let (_tx, rx) = watch::channel(false);
    let mut cancel: CancelToken = rx.into();
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &NoReporter,
        policy: Some(&*policy(true, false)),
        announcer: Some(&announcer),
    };
    let mut carry = Carry::default();
    let _ = gate_and_run(&deps, &gate_cfg(), &mut cancel, paste_plan("x"), &mut carry).await;
    let listened = ev.all().iter().position(|e| e == "listen");
    let announced = ev
        .all()
        .iter()
        .position(|e| e == "announce:Job done during the question.");
    assert!(
        announced.is_some() && announced > listened,
        "held announcement must be flushed only after the question: {:?}",
        ev.all()
    );
    assert!(
        ev.all()
            .iter()
            .take(announced.unwrap_or(0))
            .all(|e| e != "announce:Job done during the question."),
        "announcement must not be played while confirming"
    );
}

// ---------------------------------------------------------- transcript hygiene (D2)

/// The confirmation answer is never stored in the transcript: records show only the
/// original utterance and the approved action's outcome.
#[tokio::test]
async fn confirmation_answer_never_appears_in_transcript_events() {
    let ev = Arc::new(Events::default());
    let hands = GateHands::new(ev.clone());
    let reporter = CaptureReporter::default();
    let ears = GateEars {
        script: Mutex::new(vec![utt("what can you type"), utt("yes")].into()),
        ev: ev.clone(),
        policy: Some(Arc::new(GatePolicy {
            policy: Mutex::new(ActionPolicy {
                gate_active: true,
                jobs_allowed: false,
            }),
            launch: Mutex::new(None),
        })),
        flip: None,
    };
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain {
        plan: paste_plan("draft text"),
    };
    let pol = policy(true, false);
    let (_tx, rx) = watch::channel(false);
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &reporter,
        policy: Some(&*pol),
        announcer: None,
    };
    let _ = run_session(&deps, &gate_cfg(), rx).await;
    let events = reporter.0.lock().unwrap().clone();
    assert!(
        events.iter().all(|e| e.user != "yes"),
        "the confirmation answer must not be recorded as the user turn: {events:?}"
    );
    assert!(
        events
            .iter()
            .all(|e| !e.assistant.as_deref().unwrap_or("").contains("yes")),
        "the confirmation answer must not appear in assistant text: {events:?}"
    );
}

// ---------------------------------------------------------- source contracts (R2-12)

/// Non-test code of every managers converse module (excludes test files and the test
/// module tail of converse.rs).
fn production_converse_sources() -> Vec<(String, String)> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/managers");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}")) {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.starts_with("converse") || !name.ends_with(".rs") || name.ends_with("_tests.rs") {
            continue;
        }
        let text = std::fs::read_to_string(entry.path()).unwrap_or_default();
        let code = match text.find("#[cfg(test)]") {
            Some(cut) => text[..cut].to_string(),
            None => text,
        };
        out.push((name, code));
    }
    out
}

/// Returns the body slice of `async fn gate_and_run` (up to the next top-level fn).
fn gate_and_run_body(code: &str) -> Option<&str> {
    let start = code.find("async fn gate_and_run(")?;
    let rest = &code[start + 1..];
    let end = rest
        .find("\nasync fn ")
        .or_else(|| rest.find("\nfn "))
        .map(|i| i + 1)
        .unwrap_or(rest.len());
    Some(&code[start..start + 1 + end])
}

/// `.start_job(` is called from production code ONLY inside `gate_and_run` (exactly one call).
#[test]
fn start_job_is_called_only_from_gate_and_run() {
    let mut total = 0;
    let mut inside = 0;
    for (name, code) in production_converse_sources() {
        total += code.matches(".start_job(").count();
        if name == "converse.rs" {
            inside = gate_and_run_body(&code)
                .map(|b| b.matches(".start_job(").count())
                .unwrap_or(0);
        }
    }
    assert_eq!(total, 1, "exactly one production start_job call expected");
    assert_eq!(
        inside, 1,
        "the single start_job call must be inside gate_and_run"
    );
}

/// `.cancel_job(` is called from production code ONLY in the CancelJob handler.
#[test]
fn cancel_job_is_called_only_from_the_cancel_job_handler() {
    let mut total = 0;
    for (_name, code) in production_converse_sources() {
        total += code.matches(".cancel_job(").count();
    }
    assert_eq!(total, 1, "exactly one production cancel_job call expected");
    let converse = production_converse_sources()
        .into_iter()
        .find(|(n, _)| n == "converse.rs")
        .map(|(_, c)| c)
        .unwrap_or_default();
    let handler = converse
        .find("LocalIntent::CancelJob")
        .map(|i| &converse[i..i.saturating_add(400).min(converse.len())])
        .unwrap_or("");
    assert!(
        handler.contains(".cancel_job("),
        "the cancel_job call must sit in the LocalIntent::CancelJob arm"
    );
}

/// The CancelJob arm is wired: the placeholder empty arm is gone from dispatch.
#[test]
fn cancel_job_placeholder_arm_is_replaced() {
    let converse = production_converse_sources()
        .into_iter()
        .find(|(n, _)| n == "converse.rs")
        .map(|(_, c)| c)
        .unwrap_or_default();
    assert!(
        !converse.contains("LocalIntent::CancelJob => {}"),
        "the S0 placeholder arm must be replaced by the real handler"
    );
}

/// The conversation flag is SET for the whole loop (ruling B(i)). An announcement made from
/// inside `listen` must not be spoken directly while the loop runs: it is flushed only after
/// the conversation ends, so the direct announcer mouth sees the probe after the loop's stop.
#[tokio::test]
async fn conversation_flag_is_set_during_the_loop() {
    let ev = Arc::new(Events::default());
    let announcer = announcer_with(&ev);
    let ears = AnnouncingEars {
        announcer: announcer.clone(),
        text: "flag probe",
        ev: ev.clone(),
        answer: ListenOutcome::Aborted,
    };
    let mouth = GateMouth {
        ev: ev.clone(),
        say_result: SayOutcome::Done,
    };
    let brain = GateBrain {
        plan: answer_plan("x"),
    };
    let hands = GateHands::new(ev.clone());
    let (_tx, rx) = watch::channel(false);
    let deps = Deps {
        ears: &ears,
        mouth: &mouth,
        hands: &hands,
        brain: &brain,
        reporter: &NoReporter,
        policy: None,
        announcer: Some(&announcer),
    };
    let _ = run_session(&deps, &gate_cfg(), rx).await;
    let all = ev.all();
    let probe_at = all.iter().position(|e| e == "announce:flag probe");
    let stop_at = all.iter().position(|e| e == "stop");
    assert!(
        probe_at.is_some() && probe_at > stop_at,
        "flag SET: the probe must not be spoken during the loop, only flushed after it: {all:?}"
    );
}
