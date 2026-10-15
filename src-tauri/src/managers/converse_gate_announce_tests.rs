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

/// While the conversation loop runs, an announcement is queued (not spoken directly),
/// then it is spoken through the mouth at the turn boundary, in order.
#[tokio::test]
async fn announcement_during_conversation_is_spoken_at_the_turn_boundary() {
    let ev = Arc::new(Events::default());
    let announcer = announcer_with(&ev);
    let ears = AnnouncingEars {
        announcer: announcer.clone(),
        text: "The coding job finished.",
        ev: ev.clone(),
        answer: ListenOutcome::Utterance("what time is it".to_string()),
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
    assert!(
        ev.all()
            .iter()
            .all(|e| e != "announce:The coding job finished."),
        "announcement must not be spoken directly during a conversation: {:?}",
        ev.all()
    );
    let spoken_at = ev
        .all()
        .iter()
        .position(|e| e == "say:The coding job finished.");
    let listened_at = ev.all().iter().position(|e| e == "listen");
    assert!(
        spoken_at.is_some() && spoken_at > listened_at,
        "announcement must be spoken through the mouth at a turn boundary: {:?}",
        ev.all()
    );
}

/// Ending the session early (cancelled listen) clears the conversation flag.
/// Precondition proven first: the flag is SET during the loop (a probe announced from
/// inside the listen is queued, not spoken), so the post-return check is meaningful.
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
    assert!(
        ev.all()
            .iter()
            .all(|e| e != "announce:probe during early return"),
        "flag must be set during the loop: {:?}",
        ev.all()
    );
    announcer.announce(AnnounceKind::Finish, "after the session".to_string());
    assert!(
        ev.all().iter().any(|e| e == "announce:after the session"),
        "flag must be cleared when the session returns: {:?}",
        ev.all()
    );
}

/// A panic inside the conversation loop still clears the conversation flag (RAII guard).
/// Precondition proven first: the flag is SET while the loop runs (an announcement from
/// inside the panicking listen is queued, not spoken), so clearing it on unwind is real.
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
    assert!(
        ev.all().iter().all(|e| e != "announce:probe before panic"),
        "flag must be set during the loop before the panic: {:?}",
        ev.all()
    );
    announcer.announce(AnnounceKind::Finish, "after the panic".to_string());
    assert!(
        ev.all().iter().any(|e| e == "announce:after the panic"),
        "flag must be cleared on unwind: {:?}",
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
    let mut cancel: CancelToken = rx;
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

/// The conversation flag is SET for the whole loop: an announcement made from inside
/// `listen` is queued and must not be spoken directly (R2-6). Fails today because the
/// loop never calls `set_conversation_active(true)`.
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
    assert!(
        ev.all().iter().all(|e| e != "announce:flag probe"),
        "during the loop the announcement must be queued, not spoken directly: {:?}",
        ev.all()
    );
}
