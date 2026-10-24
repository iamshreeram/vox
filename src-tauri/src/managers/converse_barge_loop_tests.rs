//! Stage B1b (DESIGN Rev 8, sections 12.2.1-12.2.5 and 12.7.B-C): the barge-in seam inside the
//! conversation loop. Fakes only: no audio device, no UI. Written before the implementation.
//!
//! The fake monitor mirrors the real one: `WatchCore::fire` is the worker's sequence (close the
//! reply gate, set the reply token, stop playback) and does nothing once its watch is dropped
//! (disarmed), like the real monitor's arm check.

use crate::managers::converse::{
    run_session, BoxFuture, Brain, CancelClosed, CancelToken, Delivery, Deps, Ears, GatedMouth,
    Hands, ListenOutcome, Mouth, PhraseOutcome, QuietOutcome, Reporter, SayOutcome, SessionConfig,
    SessionEnd, SessionState, SpeechQueue, TranscriptEvent,
};
use crate::managers::converse_barge::{BargeCapture, BargeIn, BargeWatch, ReplyGate};
use crate::managers::converse_plan::{PlanError, PlanInput, PlanKind, TurnPlan};
use crate::managers::emotion::Emotion;
use std::collections::VecDeque;
use std::future::{pending, Future};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{oneshot, watch};

// ---- shared log and helpers --------------------------------------------------------------------

type Log = Arc<Mutex<Vec<String>>>;

fn new_log() -> Log {
    Arc::new(Mutex::new(Vec::new()))
}

fn note(log: &Log, entry: impl Into<String>) {
    log.lock().unwrap().push(entry.into());
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn has(entries: &[String], entry: &str) -> bool {
    entries.iter().any(|e| e == entry)
}

fn n(entries: &[String], entry: &str) -> usize {
    entries.iter().filter(|e| e.as_str() == entry).count()
}

fn np(entries: &[String], prefix: &str) -> usize {
    entries.iter().filter(|e| e.starts_with(prefix)).count()
}

fn position(entries: &[String], entry: &str) -> Option<usize> {
    entries.iter().position(|e| e == entry)
}

/// Driver step: polls the log (yielding to the session) until `done` holds; panics after 5 s.
async fn until(log: &Log, what: &str, done: impl Fn(&[String]) -> bool) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !done(&entries(log)) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}: {:?}",
            entries(log)
        );
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

fn heard(text: &str) -> ListenOutcome {
    ListenOutcome::Utterance(text.to_string())
}

fn answer(speech: &str) -> Result<TurnPlan, PlanError> {
    Ok(TurnPlan {
        job: None,
        role: None,
        kind: PlanKind::Answer,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
    })
}

fn cfg() -> SessionConfig {
    SessionConfig {
        post_speech_guard: Duration::ZERO,
        quiet_wait: Duration::from_millis(5),
        ..SessionConfig::default()
    }
}

fn streaming_cfg() -> SessionConfig {
    SessionConfig {
        early_speech: true,
        ..cfg()
    }
}

// ---- fake barge-in monitor ---------------------------------------------------------------------

struct WatchCore {
    id: usize,
    log: Log,
    gate: Arc<ReplyGate>,
    reply: watch::Sender<bool>,
    latched: AtomicBool,
    /// False once the watch is dropped: a disarmed watch cannot act (real monitor: arm check).
    active: AtomicBool,
    capture_tx: Mutex<Option<oneshot::Sender<BargeCapture>>>,
    capture_rx: Mutex<Option<oneshot::Receiver<BargeCapture>>>,
}

impl WatchCore {
    fn new(id: usize, log: Log) -> WatchCore {
        let (capture_tx, capture_rx) = oneshot::channel();
        WatchCore {
            id,
            log,
            gate: ReplyGate::new(),
            reply: watch::channel(false).0,
            latched: AtomicBool::new(false),
            active: AtomicBool::new(true),
            capture_tx: Mutex::new(Some(capture_tx)),
            capture_rx: Mutex::new(Some(capture_rx)),
        }
    }

    /// The worker's sustained-speech sequence (DESIGN 12.7.C): close the reply gate, set the
    /// reply token, stop playback. Returns false (and does nothing) when disarmed.
    fn fire(&self) -> bool {
        if !self.active.load(Ordering::SeqCst) {
            return false;
        }
        self.gate.close();
        self.latched.store(true, Ordering::SeqCst);
        self.reply.send_replace(true);
        note(&self.log, "worker-stop");
        true
    }

    /// Completes the pending capture (endpoint + speech-to-text) of this watch.
    fn deliver(&self, capture: BargeCapture) {
        if let Some(tx) = self.capture_tx.lock().unwrap().take() {
            let _ = tx.send(capture);
        }
    }
}

struct FakeBargeIn {
    log: Log,
    arms: Mutex<Vec<Arc<WatchCore>>>,
}

impl FakeBargeIn {
    /// The watch armed for turn `id` (1-based).
    fn nth(&self, id: usize) -> Arc<WatchCore> {
        self.arms.lock().unwrap()[id - 1].clone()
    }

    fn latest(&self) -> Arc<WatchCore> {
        self.arms
            .lock()
            .unwrap()
            .last()
            .expect("no watch armed yet")
            .clone()
    }
}

impl BargeIn for FakeBargeIn {
    fn arm(&self) -> Option<Box<dyn BargeWatch>> {
        let mut arms = self.arms.lock().unwrap();
        let core = Arc::new(WatchCore::new(arms.len() + 1, self.log.clone()));
        note(&self.log, format!("arm:{}", core.id));
        arms.push(core.clone());
        Some(Box::new(FakeWatch { core }))
    }
}

struct FakeWatch {
    core: Arc<WatchCore>,
}

impl BargeWatch for FakeWatch {
    fn reply_token(&self) -> watch::Receiver<bool> {
        self.core.reply.subscribe()
    }

    fn cancel_latched(&self) -> bool {
        self.core.latched.load(Ordering::SeqCst)
    }

    fn gate(&self) -> Arc<ReplyGate> {
        self.core.gate.clone()
    }

    fn capture(&mut self) -> BoxFuture<'_, BargeCapture> {
        let rx = self.core.capture_rx.lock().unwrap().take();
        Box::pin(async move {
            match rx {
                Some(rx) => rx.await.unwrap_or(BargeCapture::NoSpeech),
                None => pending::<BargeCapture>().await,
            }
        })
    }
}

impl Drop for FakeWatch {
    fn drop(&mut self) {
        self.core.active.store(false, Ordering::SeqCst);
        note(&self.core.log, format!("drop:{}", self.core.id));
    }
}

// ---- fake ears, mouth, queue, brain, hands, reporter -------------------------------------------

struct BargeEars {
    log: Log,
    steps: Mutex<VecDeque<ListenOutcome>>,
    barge: Arc<FakeBargeIn>,
    enabled: bool,
}

impl Ears for BargeEars {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
        note(&self.log, "listen");
        let next = self.steps.lock().unwrap().pop_front();
        Box::pin(async move {
            match next {
                Some(outcome) => outcome,
                // No scripted utterance left: listen until the session is stopped.
                None => pending::<ListenOutcome>().await,
            }
        })
    }

    fn barge_in(&self) -> Option<&dyn BargeIn> {
        let barge: &dyn BargeIn = self.barge.as_ref();
        self.enabled.then_some(barge)
    }
}

/// Mouth whose `say` hangs (playback running) or finishes at once, per call, in order.
struct TestMouth {
    log: Log,
    stream: bool,
    hang: Mutex<VecDeque<bool>>,
    /// Per `wait_quiet` call, in order: `true` = the sustained barge fires right then.
    quiet_fire: Mutex<VecDeque<bool>>,
    barge: Arc<FakeBargeIn>,
}

impl Mouth for TestMouth {
    fn say<'a>(&'a self, text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        note(&self.log, format!("say:{text}"));
        let hangs = self.hang.lock().unwrap().pop_front().unwrap_or(false);
        Box::pin(async move {
            if hangs {
                pending::<SayOutcome>().await
            } else {
                SayOutcome::Done
            }
        })
    }

    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        if self.quiet_fire.lock().unwrap().pop_front().unwrap_or(false) {
            self.barge.latest().fire();
        }
        Box::pin(async { QuietOutcome::Quiet })
    }

    fn stop(&self) {
        note(&self.log, "stop");
    }

    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        self.stream.then(|| {
            Arc::new(TestQueue {
                log: self.log.clone(),
            }) as Arc<dyn SpeechQueue>
        })
    }
}

struct TestQueue {
    log: Log,
}

impl SpeechQueue for TestQueue {
    fn push(&self, text: &str, _emotion: Emotion) -> bool {
        note(&self.log, format!("push:{text}"));
        true
    }

    fn abort(&self) {
        note(&self.log, "abort");
    }

    fn finish(&self) -> BoxFuture<'_, SayOutcome> {
        Box::pin(async { SayOutcome::Done })
    }
}

/// Plain mouth for the GatedMouth unit tests: `say` finishes at once, `stop` is logged.
struct PlainMouth {
    log: Log,
}

impl Mouth for PlainMouth {
    fn say<'a>(&'a self, text: &'a str, _emotion: Emotion) -> BoxFuture<'a, SayOutcome> {
        note(&self.log, format!("say:{text}"));
        Box::pin(async { SayOutcome::Done })
    }

    fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
        Box::pin(async { QuietOutcome::Quiet })
    }

    fn stop(&self) {
        note(&self.log, "stop");
    }

    fn open_queue(&self) -> Option<Arc<dyn SpeechQueue>> {
        Some(Arc::new(TestQueue {
            log: self.log.clone(),
        }))
    }
}

type PlanRecord = (String, Vec<(String, String)>);

struct ScriptBrain {
    log: Log,
    replies: Mutex<VecDeque<Result<TurnPlan, PlanError>>>,
    /// Per streamed turn: the deltas to stream (then hang), or `None` for a plain plan.
    streams: Mutex<VecDeque<Option<Vec<String>>>>,
    inputs: Mutex<Vec<PlanRecord>>,
    /// After this many streamed deltas (1-based) the brain waits for `resume`.
    pause_after: Option<usize>,
    /// After this many streamed deltas (1-based) the barge fires from inside the brain; the next
    /// delta is delivered without yielding, so only the loop's own guard can refuse it.
    fire_after: Option<usize>,
    barge: Arc<FakeBargeIn>,
    resume: Arc<AtomicBool>,
}

impl ScriptBrain {
    fn record(&self, input: &PlanInput<'_>) {
        note(&self.log, format!("plan:{}", input.utterance));
        self.inputs
            .lock()
            .unwrap()
            .push((input.utterance.to_string(), input.history.to_vec()));
    }

    fn next_reply(&self) -> Result<TurnPlan, PlanError> {
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Err(PlanError::Backend))
    }
}

impl Brain for ScriptBrain {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.record(&input);
        let reply = self.next_reply();
        Box::pin(async move { reply })
    }

    fn plan_streaming<'a>(
        &'a self,
        input: PlanInput<'a>,
        on_text: &'a mut (dyn FnMut(&str) + Send),
    ) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        self.record(&input);
        let reply = self.next_reply();
        let script = self.streams.lock().unwrap().pop_front().flatten();
        let log = self.log.clone();
        let pause_after = self.pause_after;
        let fire_after = self.fire_after;
        let barge = self.barge.clone();
        let resume = self.resume.clone();
        Box::pin(async move {
            if let Some(deltas) = script {
                for (index, delta) in deltas.iter().enumerate() {
                    on_text(delta.as_str());
                    note(&log, format!("delta:{}", index + 1));
                    if fire_after == Some(index + 1) {
                        barge.latest().fire();
                        continue;
                    }
                    if pause_after == Some(index + 1) {
                        while !resume.load(Ordering::SeqCst) {
                            tokio::task::yield_now().await;
                        }
                    }
                    tokio::task::yield_now().await;
                }
                // A stream never finishes on its own: only a barge or the session stop ends it.
                pending::<()>().await;
            }
            reply
        })
    }
}

struct NoHands;

impl Hands for NoHands {
    fn paste_generated(&self, _text: &str) -> Result<(), String> {
        Ok(())
    }

    fn run_user_phrase<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }

    fn run_model_command<'a>(&'a self, _phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
        Box::pin(async { PhraseOutcome::NoMatch })
    }
}

#[derive(Default)]
struct Rec {
    events: Mutex<Vec<TranscriptEvent>>,
}

impl Reporter for Rec {
    fn state(&self, _state: SessionState) {}

    fn transcript(&self, ev: TranscriptEvent) {
        self.events.lock().unwrap().push(ev);
    }
}

// ---- rig ---------------------------------------------------------------------------------------

struct Rig {
    log: Log,
    barge: Arc<FakeBargeIn>,
    ears: BargeEars,
    mouth: TestMouth,
    hands: NoHands,
    brain: ScriptBrain,
    reporter: Rec,
    /// The registry's side of the session stop.
    session: watch::Sender<bool>,
    resume: Arc<AtomicBool>,
}

impl Rig {
    fn new(steps: Vec<ListenOutcome>, replies: Vec<Result<TurnPlan, PlanError>>) -> Rig {
        let log = new_log();
        let barge = Arc::new(FakeBargeIn {
            log: log.clone(),
            arms: Mutex::new(Vec::new()),
        });
        let resume = Arc::new(AtomicBool::new(false));
        Rig {
            ears: BargeEars {
                log: log.clone(),
                steps: Mutex::new(steps.into()),
                barge: barge.clone(),
                enabled: true,
            },
            mouth: TestMouth {
                log: log.clone(),
                stream: false,
                hang: Mutex::new(VecDeque::new()),
                quiet_fire: Mutex::new(VecDeque::new()),
                barge: barge.clone(),
            },
            hands: NoHands,
            brain: ScriptBrain {
                log: log.clone(),
                replies: Mutex::new(replies.into()),
                streams: Mutex::new(VecDeque::new()),
                inputs: Mutex::new(Vec::new()),
                pause_after: None,
                fire_after: None,
                barge: barge.clone(),
                resume: resume.clone(),
            },
            reporter: Rec::default(),
            session: watch::channel(false).0,
            resume,
            barge,
            log,
        }
    }

    fn hang(mut self, pattern: &[bool]) -> Self {
        self.mouth.hang = Mutex::new(pattern.iter().copied().collect());
        self
    }

    fn quiet_fires(mut self, pattern: &[bool]) -> Self {
        self.mouth.quiet_fire = Mutex::new(pattern.iter().copied().collect());
        self
    }

    fn streaming(mut self, scripts: Vec<Option<Vec<&str>>>) -> Self {
        self.mouth.stream = true;
        self.brain.streams = Mutex::new(
            scripts
                .into_iter()
                .map(|script| script.map(|deltas| deltas.iter().map(|d| d.to_string()).collect()))
                .collect(),
        );
        self
    }

    fn pause_after(mut self, delta: usize) -> Self {
        self.brain.pause_after = Some(delta);
        self
    }

    fn fire_after(mut self, delta: usize) -> Self {
        self.brain.fire_after = Some(delta);
        self
    }

    fn without_barge(mut self) -> Self {
        self.ears.enabled = false;
        self
    }

    fn deps(&self) -> Deps<'_> {
        Deps {
            policy: None,
            announcer: None,
            ears: &self.ears,
            mouth: &self.mouth,
            hands: &self.hands,
            brain: &self.brain,
            reporter: &self.reporter,
        }
    }

    fn plan_inputs(&self) -> Vec<String> {
        self.brain
            .inputs
            .lock()
            .unwrap()
            .iter()
            .map(|(utterance, _)| utterance.clone())
            .collect()
    }

    fn events(&self) -> Vec<TranscriptEvent> {
        self.reporter.events.lock().unwrap().clone()
    }

    fn end_session(&self) {
        self.session.send_replace(true);
    }
}

/// Runs one session next to a driver future (both on this task, so the driver can observe and
/// steer the loop between its awaits).
async fn run(rig: &Rig, cfg: &SessionConfig, driver: impl Future<Output = ()>) -> SessionEnd {
    let deps = rig.deps();
    let session = run_session(&deps, cfg, rig.session.subscribe());
    let (end, ()) = tokio::time::timeout(Duration::from_secs(10), async {
        tokio::join!(session, driver)
    })
    .await
    .expect("run_session hung");
    end
}

// ---- barge-in during a reply: the loop's supervisor (12.2.3) -----------------------------------

#[tokio::test]
async fn barge_stops_playback_while_endpointing_and_stt_are_pending() {
    let rig = Rig::new(
        vec![heard("tell me a story")],
        vec![answer("Once upon a time.")],
    )
    .hang(&[true]);
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the reply to start", |e| {
            has(e, "say:Once upon a time.")
        })
        .await;
        assert!(rig.barge.latest().fire());
        until(&log, "playback stop", |e| has(e, "stop")).await;
        // The capture (endpoint + STT) is still pending: no result, so no second turn.
        tokio::time::sleep(Duration::from_millis(20)).await;
        let seen = entries(&log);
        assert_eq!(
            n(&seen, "listen"),
            1,
            "no listen while the capture is pending"
        );
        assert_eq!(n(&seen, "worker-stop"), 1);
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    let events = rig.events();
    assert_eq!(events.len(), 1, "one event for the interrupted turn");
    assert_eq!(events[0].delivery, Delivery::Interrupted);
}

#[tokio::test]
async fn barge_during_reply_continues_session_and_barge_text_is_the_next_turn_without_listen() {
    let rig = Rig::new(
        vec![heard("tell me a story")],
        vec![answer("Once upon a time."), answer("Okay, changing it.")],
    )
    .hang(&[true, false]);
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the first reply", |e| has(e, "say:Once upon a time.")).await;
        rig.barge.latest().fire();
        rig.barge
            .latest()
            .deliver(BargeCapture::Utterance("make it a fox".to_string()));
        // Only the listen after the barge turn is answered remains open.
        until(&log, "the listen after the barge turn", |e| {
            n(e, "listen") == 2
        })
        .await;
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    let seen = entries(&log);
    assert_eq!(
        n(&seen, "listen"),
        2,
        "the barge text was not listened for: {seen:?}"
    );
    assert_eq!(rig.plan_inputs(), vec!["tell me a story", "make it a fox"]);
    let answered = position(&seen, "say:Okay, changing it.").expect("barge turn answered");
    let second_listen = seen
        .iter()
        .enumerate()
        .filter(|(_, e)| e.as_str() == "listen")
        .nth(1)
        .expect("second listen")
        .0;
    assert!(
        answered < second_listen,
        "barge turn answered before the next listen: {seen:?}"
    );
    let events = rig.events();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].delivery, Delivery::Interrupted);
    assert_eq!(events[1].user, "make it a fox");
}

#[tokio::test]
async fn no_speech_barge_falls_back_to_a_normal_listen() {
    let rig = Rig::new(
        vec![heard("tell me a story")],
        vec![answer("Once upon a time.")],
    )
    .hang(&[true]);
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the reply to start", |e| {
            has(e, "say:Once upon a time.")
        })
        .await;
        rig.barge.latest().fire();
        rig.barge.latest().deliver(BargeCapture::NoSpeech);
        until(&log, "the normal listen", |e| n(e, "listen") == 2).await;
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert_eq!(
        rig.plan_inputs(),
        vec!["tell me a story"],
        "no turn for silence"
    );
    assert_eq!(n(&entries(&log), "listen"), 2);
}

#[tokio::test]
async fn session_stop_during_endpointing_ends_cancelled_without_an_extra_turn() {
    let rig = Rig::new(
        vec![heard("tell me a story")],
        vec![answer("Once upon a time.")],
    )
    .hang(&[true]);
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the reply to start", |e| {
            has(e, "say:Once upon a time.")
        })
        .await;
        rig.barge.latest().fire();
        until(&log, "playback stop", |e| has(e, "stop")).await;
        // Endpointing is still running; the session is stopped now.
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert_eq!(rig.plan_inputs(), vec!["tell me a story"]);
    assert_eq!(n(&entries(&log), "listen"), 1);
    assert_eq!(rig.events().len(), 1);
}

#[tokio::test]
async fn session_stop_during_stt_ignores_a_transcript_that_arrives_late() {
    let rig = Rig::new(
        vec![heard("tell me a story")],
        vec![answer("Once upon a time.")],
    )
    .hang(&[true]);
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the reply to start", |e| {
            has(e, "say:Once upon a time.")
        })
        .await;
        rig.barge.latest().fire();
        until(&log, "playback stop", |e| has(e, "stop")).await;
        // Speech-to-text is in flight; the session stops, then the transcript lands.
        rig.end_session();
        rig.barge
            .latest()
            .deliver(BargeCapture::Utterance("late words".to_string()));
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    assert_eq!(
        rig.plan_inputs(),
        vec!["tell me a story"],
        "a late transcript is never a turn"
    );
    assert_eq!(n(&entries(&log), "listen"), 1);
}

#[tokio::test]
async fn barge_landing_as_the_reply_completes_is_honoured_exactly_once() {
    let rig = Rig::new(
        vec![heard("tell me a story")],
        vec![answer("Once upon a time."), answer("Sure, again.")],
    )
    .hang(&[false, false])
    // The barge lands inside the first reply's settle, the same instant the reply completes.
    .quiet_fires(&[true, false]);
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the worker stop", |e| has(e, "worker-stop")).await;
        rig.barge
            .latest()
            .deliver(BargeCapture::Utterance("again".to_string()));
        until(&log, "the listen after the barge turn", |e| {
            n(e, "listen") == 2
        })
        .await;
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    let seen = entries(&log);
    assert_eq!(n(&seen, "worker-stop"), 1, "honoured once: {seen:?}");
    assert_eq!(rig.plan_inputs(), vec!["tell me a story", "again"]);
    assert_eq!(rig.events().len(), 2, "one extra turn only");
    assert_eq!(rig.events()[0].delivery, Delivery::Interrupted);
}

#[tokio::test]
async fn stale_watch_capture_and_fire_after_the_next_arm_are_ignored() {
    let rig = Rig::new(
        vec![heard("first"), heard("second")],
        vec![answer("Reply one."), answer("Reply two.")],
    )
    .hang(&[false, true]);
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the second reply to start", |e| {
            has(e, "say:Reply two.")
        })
        .await;
        // Watch 1 belongs to a finished turn: its capture and its fire must have no effect.
        let old = rig.barge.nth(1);
        old.deliver(BargeCapture::Utterance("stale words".to_string()));
        assert!(!old.fire(), "a disarmed watch cannot fire");
        tokio::time::sleep(Duration::from_millis(20)).await;
        let seen = entries(&log);
        assert_eq!(n(&seen, "worker-stop"), 0);
        assert_eq!(n(&seen, "listen"), 2, "no extra listen: {seen:?}");
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    let seen = entries(&log);
    assert_eq!(rig.plan_inputs(), vec!["first", "second"], "no extra turn");
    let dropped = position(&seen, "drop:1").expect("watch 1 dropped");
    let armed = position(&seen, "arm:2").expect("watch 2 armed");
    assert!(
        dropped < armed,
        "watch 1 dropped before watch 2 armed: {seen:?}"
    );
}

#[tokio::test]
async fn two_consecutive_barges_are_each_honoured_once() {
    let rig = Rig::new(
        vec![heard("tell me a story")],
        vec![
            answer("Once upon a time."),
            answer("Changed once."),
            answer("Changed twice."),
        ],
    )
    .hang(&[true, true, false]);
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the first reply", |e| has(e, "say:Once upon a time.")).await;
        rig.barge.nth(1).fire();
        rig.barge
            .nth(1)
            .deliver(BargeCapture::Utterance("first change".to_string()));
        until(&log, "the first changed reply", |e| {
            has(e, "say:Changed once.")
        })
        .await;
        rig.barge.nth(2).fire();
        rig.barge
            .nth(2)
            .deliver(BargeCapture::Utterance("second change".to_string()));
        until(&log, "the second changed reply and listen", |e| {
            has(e, "say:Changed twice.") && n(e, "listen") == 2
        })
        .await;
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    let seen = entries(&log);
    assert_eq!(n(&seen, "worker-stop"), 2);
    assert_eq!(
        rig.plan_inputs(),
        vec!["tell me a story", "first change", "second change"]
    );
    assert_eq!(n(&seen, "listen"), 2);
    assert_eq!(rig.events().len(), 3);
}

#[tokio::test]
async fn barge_while_the_brain_is_still_streaming_speaks_no_later_sentence() {
    let rig = Rig::new(
        vec![heard("tell me a story")],
        vec![answer("unused, the stream decides")],
    )
    .streaming(vec![Some(vec![
        r#"{"kind":"answer","emotion":"calm","speech":"Partial thought with no full stop yet"#,
        " and then more. Late sentence one is long enough here. ",
        "Late sentence two is also long enough here. \"}",
    ])])
    .fire_after(1);
    let log = rig.log.clone();
    let end = run(&rig, &streaming_cfg(), async {
        // The barge fires inside the brain after delta 1; delta 2 reaches the loop with the token
        // already set, and its complete sentences must be refused.
        until(&log, "the delta after the stop", |e| has(e, "delta:2")).await;
        let seen = entries(&log);
        assert_eq!(n(&seen, "worker-stop"), 1);
        assert_eq!(
            np(&seen, "push:"),
            0,
            "no sentence queued after the stop: {seen:?}"
        );
        assert_eq!(np(&seen, "say:"), 0, "no say after the stop: {seen:?}");
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].assistant, None, "nothing was submitted");
    assert_eq!(events[0].delivery, Delivery::NotSpoken);
}

// ---- interrupted-turn history (12.2.5) ---------------------------------------------------------

/// Two sentences are submitted (queued) before the barge; the third must never appear.
fn streamed_turn_rig() -> Rig {
    Rig::new(
        vec![heard("tell me a story")],
        vec![answer("unused, the stream decides"), answer("Okay.")],
    )
    .streaming(vec![
        Some(vec![
            r#"{"kind":"answer","emotion":"calm","speech":"First sentence is long enough here. "#,
            "Second sentence is also long enough here. ",
            "Third sentence must never be submitted.\"}",
        ]),
        None,
    ])
    .pause_after(2)
}

async fn drive_streamed_barge(rig: &Rig, cfg: &SessionConfig) -> SessionEnd {
    let log = rig.log.clone();
    run(rig, cfg, async {
        until(&log, "two submitted sentences", |e| np(e, "push:") == 2).await;
        assert!(rig.barge.latest().fire());
        rig.barge
            .latest()
            .deliver(BargeCapture::Utterance("next question".to_string()));
        rig.resume.store(true, Ordering::SeqCst);
        until(&log, "the listen after the next turn", |e| {
            n(e, "listen") == 2
        })
        .await;
        rig.end_session();
    })
    .await
}

#[tokio::test]
async fn next_plan_input_after_a_barge_keeps_only_submitted_sentences_marked_interrupted() {
    let rig = streamed_turn_rig();
    let end = drive_streamed_barge(&rig, &streaming_cfg()).await;
    assert_eq!(end, SessionEnd::Cancelled);
    let inputs = rig.brain.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 2, "one barge turn, one next turn");
    assert_eq!(inputs[1].0, "next question");
    assert_eq!(
        inputs[1].1.len(),
        1,
        "exactly the interrupted turn is in history"
    );
    let (request, reply) = &inputs[1].1[0];
    assert_eq!(request, "tell me a story");
    assert!(
        reply.starts_with(
            "First sentence is long enough here. Second sentence is also long enough here."
        ),
        "history must hold the submitted sentences: {reply:?}"
    );
    assert!(
        reply.ends_with(&crate::managers::converse_context::note_interrupted()),
        "history must end with the structured interrupted note: {reply:?}"
    );
    assert!(
        !reply.contains("Third"),
        "an unsubmitted sentence must never reach history: {reply:?}"
    );
    let events = rig.events();
    assert_eq!(events[0].delivery, Delivery::Interrupted);
    assert_eq!(
        events[0].assistant.as_deref(),
        Some("First sentence is long enough here. Second sentence is also long enough here.")
    );
}

#[tokio::test]
async fn zero_context_turns_retains_no_interrupted_history() {
    let rig = streamed_turn_rig();
    let cfg = SessionConfig {
        context_turns: 0,
        ..streaming_cfg()
    };
    let end = drive_streamed_barge(&rig, &cfg).await;
    assert_eq!(end, SessionEnd::Cancelled);
    let inputs = rig.brain.inputs.lock().unwrap().clone();
    assert_eq!(inputs.len(), 2);
    assert!(
        inputs[1].1.is_empty(),
        "converse_context_turns = 0 keeps nothing"
    );
}

// ---- no monitor: the loop is unchanged ---------------------------------------------------------

#[tokio::test]
async fn without_a_barge_in_monitor_the_loop_arms_nothing_and_records_as_before() {
    let rig = Rig::new(vec![heard("hi there")], vec![answer("Hello.")]).without_barge();
    let log = rig.log.clone();
    let end = run(&rig, &cfg(), async {
        until(&log, "the second listen", |e| n(e, "listen") == 2).await;
        rig.end_session();
    })
    .await;
    assert_eq!(end, SessionEnd::Cancelled);
    let seen = entries(&log);
    assert_eq!(np(&seen, "arm:"), 0);
    assert_eq!(np(&seen, "worker-stop"), 0);
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].delivery, Delivery::Submitted);
}

// ---- GatedMouth (12.7.C) -----------------------------------------------------------------------

#[tokio::test]
async fn gated_say_admits_and_reaches_the_backend_while_the_gate_is_open() {
    let log = new_log();
    let mouth = PlainMouth { log: log.clone() };
    let gated = GatedMouth {
        inner: &mouth,
        gate: ReplyGate::new(),
    };
    assert_eq!(gated.say("hello", Emotion::Neutral).await, SayOutcome::Done);
    assert_eq!(entries(&log), vec!["say:hello"]);
}

#[tokio::test]
async fn gated_say_taken_between_the_token_check_and_admission_never_reaches_the_backend() {
    let log = new_log();
    let mouth = PlainMouth { log: log.clone() };
    let gate = ReplyGate::new();
    let (session_tx, session_rx) = watch::channel(false);
    let token = CancelToken::from(session_rx);
    let gated = GatedMouth {
        inner: &mouth,
        gate: gate.clone(),
    };
    let (paused_tx, paused_rx) = std::sync::mpsc::channel::<()>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    let inner: &PlainMouth = &mouth;
    std::thread::scope(|scope| {
        scope.spawn(move || {
            // The cancel runs on another thread while the reply is paused.
            paused_rx.recv().unwrap();
            gate.close();
            session_tx.send_replace(true);
            inner.stop();
            done_tx.send(()).unwrap();
        });
        assert!(!token.is_cancelled(), "the token check passes first");
        paused_tx.send(()).unwrap();
        done_rx.recv().unwrap();
    });
    let outcome = gated.say("late sentence", Emotion::Neutral).await;
    assert_eq!(outcome, SayOutcome::Interrupted);
    assert_eq!(entries(&log), vec!["stop"], "no say reached the backend");
}

#[tokio::test]
async fn gated_queue_push_taken_between_the_token_check_and_admission_never_reaches_the_backend() {
    let log = new_log();
    let mouth = PlainMouth { log: log.clone() };
    let gate = ReplyGate::new();
    let (session_tx, session_rx) = watch::channel(false);
    let token = CancelToken::from(session_rx);
    let gated = GatedMouth {
        inner: &mouth,
        gate: gate.clone(),
    };
    let queue = gated
        .open_queue()
        .expect("queue opens while the gate is open");
    let (paused_tx, paused_rx) = std::sync::mpsc::channel::<()>();
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    std::thread::scope(|scope| {
        scope.spawn(move || {
            paused_rx.recv().unwrap();
            gate.close();
            session_tx.send_replace(true);
            done_tx.send(()).unwrap();
        });
        assert!(!token.is_cancelled(), "the token check passes first");
        paused_tx.send(()).unwrap();
        done_rx.recv().unwrap();
    });
    assert!(
        !queue.push("late sentence", Emotion::Neutral),
        "a closed gate refuses the push"
    );
    assert!(entries(&log).is_empty(), "no push reached the backend");
}

// ---- CancelToken (12.2.1, 12.7.B) --------------------------------------------------------------

#[tokio::test]
async fn closed_session_sender_never_fires_and_a_later_reply_cancel_wakes_the_wait() {
    let (session_tx, session_rx) = watch::channel(false);
    let (reply_tx, reply_rx) = watch::channel(false);
    let mut token = CancelToken::from(session_rx).with_reply(reply_rx);
    drop(session_tx);
    assert!(
        !token.is_cancelled(),
        "a closed session sender is not a cancel"
    );
    let (waited, ()) = tokio::join!(
        tokio::time::timeout(
            Duration::from_secs(2),
            token.wait_for(|cancelled| *cancelled)
        ),
        async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            reply_tx.send_replace(true);
        }
    );
    assert_eq!(
        waited.expect("the wait must wake on the reply token"),
        Ok(())
    );
    assert!(token.is_cancelled());
    assert!(!token.session_cancelled());
}

#[tokio::test]
async fn closed_session_and_closed_reply_report_closed_instead_of_waiting_forever() {
    let (session_tx, session_rx) = watch::channel(false);
    let (reply_tx, reply_rx) = watch::channel(false);
    let mut token = CancelToken::from(session_rx).with_reply(reply_rx);
    drop(session_tx);
    drop(reply_tx);
    let waited = tokio::time::timeout(Duration::from_secs(2), token.wait_for(|c| *c))
        .await
        .expect("nothing can cancel: must not hang");
    assert_eq!(waited, Err(CancelClosed));
}

#[test]
fn session_cancel_is_seen_synchronously_by_every_clone() {
    let (tx, rx) = watch::channel(false);
    let token = CancelToken::from(rx);
    let copy = token.clone();
    tx.send_replace(true);
    assert!(token.is_cancelled());
    assert!(copy.is_cancelled());
    assert!(token.session_cancelled());
}

#[test]
fn reply_token_cancels_the_composite_but_not_the_session_side() {
    let (_session_tx, session_rx) = watch::channel(false);
    let (reply_tx, reply_rx) = watch::channel(false);
    let token = CancelToken::from(session_rx).with_reply(reply_rx);
    reply_tx.send_replace(true);
    assert!(token.is_cancelled());
    assert!(!token.session_cancelled());
}

#[test]
fn derived_reply_token_keeps_the_session_side() {
    let (session_tx, session_rx) = watch::channel(false);
    let base = CancelToken::from(session_rx);
    let (_reply_tx, reply_rx) = watch::channel(false);
    let derived = base.with_reply(reply_rx);
    session_tx.send_replace(true);
    assert!(derived.is_cancelled());
    assert!(derived.session_cancelled());
    assert!(base.is_cancelled());
}

#[test]
fn closed_session_sender_keeps_its_last_cancel_value() {
    let (tx, rx) = watch::channel(false);
    let token = CancelToken::from(rx);
    tx.send_replace(true);
    drop(tx);
    assert!(token.is_cancelled());
}

// ---- compile-time assertion (12.2.2) -----------------------------------------------------------

#[test]
fn run_session_future_stays_send_with_a_barge_monitor() {
    fn assert_send<T: Send>(_: &T) {}
    let rig = Rig::new(vec![heard("hi")], vec![answer("Hello.")]);
    let deps = rig.deps();
    let (_session_tx, session_rx) = watch::channel(false);
    let cfg = cfg();
    let session = run_session(&deps, &cfg, session_rx);
    assert_send(&session);
}
