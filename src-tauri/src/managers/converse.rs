//! The conversation session loop: listen -> route -> act/answer -> speak ->
//! listen. Pure and trait-generic (no tauri types); all hardware sits behind
//! the traits below. Contract: DESIGN.md section 3.3 (Revision 4).

use crate::managers::converse_plan::{PlanError, PlanInput, TurnPlan};
use crate::managers::emotion::Emotion;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::time::Duration;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
/// `true` means the session was cancelled.
pub type CancelToken = tokio::sync::watch::Receiver<bool>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListenFailure {
    Busy,
    NoMicrophone,
    Permission,
    ModelUnavailable,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListenOutcome {
    Utterance(String),
    NoSpeech,
    TooLong,
    Aborted,
    Failed(ListenFailure),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SayOutcome {
    Done,
    Interrupted,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuietOutcome {
    Quiet,
    StillSpeaking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhraseOutcome {
    /// Executed; the string is a short spoken confirmation.
    Done(String),
    NoMatch,
    Refused,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    Listening,
    Thinking,
    Speaking,
    Acting,
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEnd {
    UserEnded,
    Idle,
    Cancelled,
    NotConfigured,
    AudioUnavailable,
    SpeechStuck,
    TurnLimit,
}

pub trait Ears: Send + Sync {
    fn listen(&self) -> BoxFuture<'_, ListenOutcome>;
}

pub trait Mouth: Send + Sync {
    fn say<'a>(&'a self, text: &'a str, emotion: Emotion) -> BoxFuture<'a, SayOutcome>;
    fn wait_quiet(&self, max: Duration) -> BoxFuture<'_, QuietOutcome>;
    fn stop(&self);
}

pub trait Hands: Send + Sync {
    /// Paste generated/dictated text into the active window (never submits).
    fn paste_generated(&self, text: &str) -> Result<(), String>;
    /// Exact tier: what the USER said (shortcuts, media, router).
    fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome>;
    /// Restricted tier: a command PROPOSED BY THE MODEL.
    fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome>;
}

pub trait Brain: Send + Sync {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>>;
}

pub trait Reporter: Send + Sync {
    fn state(&self, state: SessionState);
}

pub struct Deps<'a> {
    pub ears: &'a dyn Ears,
    pub mouth: &'a dyn Mouth,
    pub hands: &'a dyn Hands,
    pub brain: &'a dyn Brain,
    pub reporter: &'a dyn Reporter,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub max_idle_turns: u8,
    pub context_turns: usize,
    pub max_turns: usize,
    pub post_speech_guard: Duration,
    pub quiet_wait: Duration,
    pub style: String,
}

impl Default for SessionConfig {
    fn default() -> Self {
        SessionConfig {
            max_idle_turns: 2,
            context_turns: 6,
            max_turns: 50,
            post_speech_guard: Duration::from_millis(350),
            quiet_wait: Duration::from_secs(2),
            style: String::new(),
        }
    }
}

/// Run one conversation session to its end. Reports `Idle` exactly once on
/// every exit path and calls `mouth.stop()` when the end is `Cancelled`.
pub async fn run_session(
    _deps: &Deps<'_>,
    _cfg: &SessionConfig,
    _cancel: CancelToken,
) -> SessionEnd {
    todo!("S3")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistryState {
    Idle,
    Running(u64),
    Draining(u64),
}

/// Single-active-session lifecycle: Idle -> Running -> Draining -> Idle.
pub struct SessionRegistry {
    #[allow(dead_code)]
    inner: Mutex<RegistryInner>,
}

#[allow(dead_code)]
struct RegistryInner {
    state: RegistryState,
    next_id: u64,
    cancel_tx: Option<tokio::sync::watch::Sender<bool>>,
}

impl Default for SessionRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionRegistry {
    pub fn new() -> Self {
        SessionRegistry {
            inner: Mutex::new(RegistryInner {
                state: RegistryState::Idle,
                next_id: 1,
                cancel_tx: None,
            }),
        }
    }
    /// Only from Idle: becomes Running(id) and returns the cancel receiver.
    pub fn try_begin(&self) -> Option<(u64, CancelToken)> {
        todo!("S3")
    }
    /// Running -> Draining and send cancel (use `send_replace`: `send` drops the
    /// value when no receiver is alive). False when Idle or already Draining.
    pub fn request_stop(&self) -> bool {
        todo!("S3")
    }
    /// Frees the registry only when `id` matches the current session.
    pub fn finish(&self, _id: u64) -> bool {
        todo!("S3")
    }
    /// True for Running and Draining.
    pub fn is_active(&self) -> bool {
        todo!("S3")
    }
    pub fn state(&self) -> RegistryState {
        todo!("S3")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::converse_plan::PlanKind;
    use std::collections::VecDeque;
    use std::sync::Arc;
    use tokio::sync::watch;

    // ------------------------------------------------------------------
    // fakes
    // ------------------------------------------------------------------

    #[derive(Default)]
    struct Log(Mutex<Vec<String>>);
    impl Log {
        fn push(&self, s: impl Into<String>) {
            self.0.lock().unwrap().push(s.into());
        }
        fn all(&self) -> Vec<String> {
            self.0.lock().unwrap().clone()
        }
        fn count(&self, prefix: &str) -> usize {
            self.all().iter().filter(|e| e.starts_with(prefix)).count()
        }
        fn has(&self, exact: &str) -> bool {
            self.all().iter().any(|e| e == exact)
        }
        fn says(&self) -> Vec<String> {
            self.all()
                .iter()
                .filter_map(|e| e.strip_prefix("say:").map(|s| s.to_string()))
                .collect()
        }
        fn index_of(&self, exact: &str) -> Option<usize> {
            self.all().iter().position(|e| e == exact)
        }
    }

    enum L {
        Out(ListenOutcome),
        Hang,
        /// Cancel the session, then return the outcome (both "ready at once").
        CancelThen(ListenOutcome),
    }
    struct FakeEars {
        script: Mutex<VecDeque<L>>,
        log: Arc<Log>,
        cancel: Arc<watch::Sender<bool>>,
    }
    impl Ears for FakeEars {
        fn listen(&self) -> BoxFuture<'_, ListenOutcome> {
            self.log.push("listen");
            let item = self.script.lock().unwrap().pop_front();
            let cancel = self.cancel.clone();
            Box::pin(async move {
                match item {
                    Some(L::Out(o)) => o,
                    Some(L::CancelThen(o)) => {
                        cancel.send_replace(true);
                        o
                    }
                    Some(L::Hang) => std::future::pending().await,
                    None => ListenOutcome::Aborted,
                }
            })
        }
    }

    enum S {
        Out(SayOutcome),
        Hang,
    }
    struct FakeMouth {
        say_script: Mutex<VecDeque<S>>,
        quiet_script: Mutex<VecDeque<QuietOutcome>>,
        log: Arc<Log>,
    }
    impl Mouth for FakeMouth {
        fn say<'a>(&'a self, text: &'a str, _e: Emotion) -> BoxFuture<'a, SayOutcome> {
            self.log.push(format!("say:{text}"));
            let item = self.say_script.lock().unwrap().pop_front();
            Box::pin(async move {
                match item {
                    Some(S::Out(o)) => o,
                    Some(S::Hang) => std::future::pending().await,
                    None => SayOutcome::Done,
                }
            })
        }
        fn wait_quiet(&self, _max: Duration) -> BoxFuture<'_, QuietOutcome> {
            self.log.push("quiet");
            let o = self
                .quiet_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(QuietOutcome::Quiet);
            Box::pin(async move { o })
        }
        fn stop(&self) {
            self.log.push("stop");
        }
    }

    struct FakeHands {
        paste_script: Mutex<VecDeque<Result<(), String>>>,
        user_script: Mutex<VecDeque<PhraseOutcome>>,
        model_script: Mutex<VecDeque<PhraseOutcome>>,
        log: Arc<Log>,
    }
    impl Hands for FakeHands {
        fn paste_generated(&self, text: &str) -> Result<(), String> {
            self.log.push(format!("paste:{text}"));
            self.paste_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()))
        }
        fn run_user_phrase<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            self.log.push(format!("user:{phrase}"));
            let o = self
                .user_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(PhraseOutcome::NoMatch);
            Box::pin(async move { o })
        }
        fn run_model_command<'a>(&'a self, phrase: &'a str) -> BoxFuture<'a, PhraseOutcome> {
            self.log.push(format!("model:{phrase}"));
            let o = self
                .model_script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(PhraseOutcome::NoMatch);
            Box::pin(async move { o })
        }
    }

    enum B {
        Plan(Result<TurnPlan, PlanError>),
        /// Cancel the session, then return the plan (ready at the same instant).
        CancelThen(Result<TurnPlan, PlanError>),
    }
    struct FakeBrain {
        script: Mutex<VecDeque<B>>,
        log: Arc<Log>,
        cancel: Arc<watch::Sender<bool>>,
        histories: Mutex<Vec<Vec<(String, String)>>>,
        drafts: Mutex<Vec<Option<String>>>,
    }
    impl Brain for FakeBrain {
        fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
            self.log.push(format!("plan:{}", input.utterance));
            self.histories.lock().unwrap().push(input.history.to_vec());
            self.drafts
                .lock()
                .unwrap()
                .push(input.last_draft.map(|s| s.to_string()));
            let item = self.script.lock().unwrap().pop_front();
            let cancel = self.cancel.clone();
            Box::pin(async move {
                match item {
                    Some(B::Plan(r)) => r,
                    Some(B::CancelThen(r)) => {
                        cancel.send_replace(true);
                        r
                    }
                    None => Err(PlanError::Backend),
                }
            })
        }
    }

    struct FakeReporter(Mutex<Vec<SessionState>>);
    impl Reporter for FakeReporter {
        fn state(&self, s: SessionState) {
            self.0.lock().unwrap().push(s);
        }
    }

    struct Rig {
        log: Arc<Log>,
        cancel: Arc<watch::Sender<bool>>,
        ears: FakeEars,
        mouth: FakeMouth,
        hands: FakeHands,
        brain: FakeBrain,
        reporter: FakeReporter,
    }

    fn rig(listens: Vec<L>, plans: Vec<B>) -> Rig {
        let log = Arc::new(Log::default());
        let (tx, _rx) = watch::channel(false);
        let cancel = Arc::new(tx);
        Rig {
            ears: FakeEars {
                script: Mutex::new(listens.into()),
                log: log.clone(),
                cancel: cancel.clone(),
            },
            mouth: FakeMouth {
                say_script: Mutex::new(VecDeque::new()),
                quiet_script: Mutex::new(VecDeque::new()),
                log: log.clone(),
            },
            hands: FakeHands {
                paste_script: Mutex::new(VecDeque::new()),
                user_script: Mutex::new(VecDeque::new()),
                model_script: Mutex::new(VecDeque::new()),
                log: log.clone(),
            },
            brain: FakeBrain {
                script: Mutex::new(plans.into()),
                log: log.clone(),
                cancel: cancel.clone(),
                histories: Mutex::new(vec![]),
                drafts: Mutex::new(vec![]),
            },
            reporter: FakeReporter(Mutex::new(vec![])),
            log,
            cancel,
        }
    }

    fn cfg() -> SessionConfig {
        SessionConfig {
            post_speech_guard: Duration::from_millis(0),
            quiet_wait: Duration::from_millis(5),
            ..SessionConfig::default()
        }
    }

    async fn run(r: &Rig, cfg: &SessionConfig) -> SessionEnd {
        let deps = Deps {
            ears: &r.ears,
            mouth: &r.mouth,
            hands: &r.hands,
            brain: &r.brain,
            reporter: &r.reporter,
        };
        let token = r.cancel.subscribe();
        // Hard test timeout so a regression hangs loudly, not forever.
        tokio::time::timeout(Duration::from_secs(5), run_session(&deps, cfg, token))
            .await
            .expect("run_session hung")
    }

    fn utt(s: &str) -> L {
        L::Out(ListenOutcome::Utterance(s.to_string()))
    }
    fn plan(kind: PlanKind, speech: &str) -> TurnPlan {
        TurnPlan {
            kind,
            speech: speech.to_string(),
            emotion: Emotion::Neutral,
            paste_text: None,
            command: None,
        }
    }
    fn answer(speech: &str) -> B {
        B::Plan(Ok(plan(PlanKind::Answer, speech)))
    }
    fn paste_plan(text: &str, speech: &str) -> TurnPlan {
        TurnPlan {
            paste_text: Some(text.to_string()),
            ..plan(PlanKind::Paste, speech)
        }
    }
    fn command_plan(cmd: &str, speech: &str) -> TurnPlan {
        TurnPlan {
            command: Some(cmd.to_string()),
            ..plan(PlanKind::Command, speech)
        }
    }
    fn idle_count(r: &Rig) -> usize {
        r.reporter
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|s| **s == SessionState::Idle)
            .count()
    }

    // ------------------------------------------------------------------
    // listening outcomes
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn no_speech_streak_ends_the_session_with_a_spoken_notice() {
        let r = rig(
            vec![
                L::Out(ListenOutcome::NoSpeech),
                L::Out(ListenOutcome::NoSpeech),
            ],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Idle);
        assert_eq!(r.log.count("listen"), 2);
        assert_eq!(r.log.says(), vec!["I'll stop listening now."]);
        assert_eq!(r.log.count("plan:"), 0);
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn speech_resets_the_idle_streak() {
        let r = rig(
            vec![
                L::Out(ListenOutcome::NoSpeech),
                utt("hello"),
                L::Out(ListenOutcome::NoSpeech),
                L::Out(ListenOutcome::NoSpeech),
            ],
            vec![answer("Hi!")],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Idle);
        assert_eq!(r.log.count("listen"), 4);
        assert_eq!(r.log.count("plan:"), 1);
    }

    #[tokio::test]
    async fn whitespace_only_utterances_count_as_no_speech() {
        let r = rig(vec![utt("   "), utt("\n\t")], vec![]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Idle);
        assert_eq!(r.log.count("plan:"), 0);
        assert_eq!(r.log.count("user:"), 0);
    }

    #[tokio::test]
    async fn max_idle_turns_is_honoured() {
        let mut c = cfg();
        c.max_idle_turns = 3;
        let r = rig(
            (0..3).map(|_| L::Out(ListenOutcome::NoSpeech)).collect(),
            vec![],
        );
        assert_eq!(run(&r, &c).await, SessionEnd::Idle);
        assert_eq!(r.log.count("listen"), 3);
    }

    #[tokio::test]
    async fn aborted_listen_cancels_the_session() {
        let r = rig(vec![L::Out(ListenOutcome::Aborted)], vec![]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert!(r.log.has("stop"));
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn too_long_listen_is_rejected_without_routing() {
        let r = rig(
            vec![
                L::Out(ListenOutcome::TooLong),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(
            r.log.says(),
            vec!["That was too long for me. Please say it in parts."]
        );
        assert_eq!(
            r.log.count("plan:") + r.log.count("user:") + r.log.count("paste:"),
            0
        );
    }

    #[tokio::test]
    async fn over_long_utterance_is_rejected_not_truncated() {
        let long = "a".repeat(1001);
        let r = rig(vec![utt(&long), L::Out(ListenOutcome::Aborted)], vec![]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(
            r.log.says(),
            vec!["That was too long for me. Please say it in parts."]
        );
        assert_eq!(
            r.log.count("plan:") + r.log.count("user:") + r.log.count("paste:"),
            0
        );
        // exactly 1000 chars is accepted
        let ok = "a".repeat(1000);
        let r2 = rig(
            vec![utt(&ok), L::Out(ListenOutcome::Aborted)],
            vec![answer("ok")],
        );
        run(&r2, &cfg()).await;
        assert_eq!(r2.log.count("plan:"), 1);
    }

    #[tokio::test]
    async fn failed_listen_says_a_hint_and_ends_with_audio_unavailable() {
        for f in [
            ListenFailure::Busy,
            ListenFailure::NoMicrophone,
            ListenFailure::Permission,
            ListenFailure::ModelUnavailable,
            ListenFailure::Other,
        ] {
            let r = rig(vec![L::Out(ListenOutcome::Failed(f))], vec![]);
            assert_eq!(run(&r, &cfg()).await, SessionEnd::AudioUnavailable);
            assert_eq!(r.log.says(), vec!["I can't use the microphone right now."]);
            assert_eq!(idle_count(&r), 1);
        }
    }

    // ------------------------------------------------------------------
    // local tier
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn end_phrase_says_okay_and_ends() {
        let r = rig(vec![utt("goodbye")], vec![]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::UserEnded);
        assert_eq!(r.log.says(), vec!["Okay."]);
        assert_eq!(r.log.count("user:") + r.log.count("plan:"), 0);
        assert!(!r.log.has("stop"));
    }

    #[tokio::test]
    async fn dictation_pastes_silently_and_keeps_listening() {
        let r = rig(
            vec![utt("dictate Hello World"), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert!(r.log.has("paste:Hello World"));
        assert!(r.log.says().is_empty());
        assert_eq!(r.log.count("plan:") + r.log.count("user:"), 0);
        assert_eq!(r.log.count("listen"), 2);
    }

    #[tokio::test]
    async fn dictation_paste_failure_is_reported_without_the_error_text() {
        let r = rig(
            vec![utt("dictate secret thing"), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        r.hands
            .paste_script
            .lock()
            .unwrap()
            .push_back(Err("terminal refused: secret thing".into()));
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["I couldn't paste that."]);
    }

    // ------------------------------------------------------------------
    // exact tier
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn exact_tier_done_speaks_the_confirmation_and_skips_the_brain() {
        let r = rig(
            vec![utt("open Safari"), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        r.hands
            .user_script
            .lock()
            .unwrap()
            .push_back(PhraseOutcome::Done("Opening Safari.".into()));
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["Opening Safari."]);
        assert_eq!(r.log.count("plan:"), 0);
        assert!(r.reporter.0.lock().unwrap().contains(&SessionState::Acting));
    }

    #[tokio::test]
    async fn exact_tier_failure_and_refusal_have_fixed_messages() {
        let r = rig(
            vec![utt("a"), utt("b"), L::Out(ListenOutcome::Aborted)],
            vec![],
        );
        r.hands
            .user_script
            .lock()
            .unwrap()
            .extend([PhraseOutcome::Failed, PhraseOutcome::Refused]);
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["That didn't work.", "I can't do that."]);
    }

    #[tokio::test]
    async fn exact_tier_actions_do_not_enter_the_history() {
        let r = rig(
            vec![
                utt("open Safari"),
                utt("tell me a joke"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![answer("Knock knock.")],
        );
        r.hands
            .user_script
            .lock()
            .unwrap()
            .push_back(PhraseOutcome::Done("Opening Safari.".into()));
        run(&r, &cfg()).await;
        let h = r.brain.histories.lock().unwrap().clone();
        assert_eq!(h.len(), 1);
        assert!(h[0].is_empty());
    }

    // ------------------------------------------------------------------
    // brain tier
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn answer_is_spoken_recorded_and_visible_to_the_next_plan() {
        let r = rig(
            vec![
                utt("what is rust"),
                utt("and go?"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![answer("A language."), answer("Another language.")],
        );
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["A language.", "Another language."]);
        let h = r.brain.histories.lock().unwrap().clone();
        assert!(h[0].is_empty());
        assert_eq!(
            h[1],
            vec![("what is rust".to_string(), "A language.".to_string())]
        );
    }

    #[tokio::test]
    async fn state_order_for_an_answered_turn() {
        let r = rig(
            vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
            vec![answer("Hello.")],
        );
        run(&r, &cfg()).await;
        let states = r.reporter.0.lock().unwrap().clone();
        let pos = |s: SessionState| states.iter().position(|x| *x == s).unwrap();
        assert!(pos(SessionState::Listening) < pos(SessionState::Thinking));
        assert!(pos(SessionState::Thinking) < pos(SessionState::Speaking));
        assert_eq!(*states.last().unwrap(), SessionState::Idle);
    }

    #[tokio::test]
    async fn plan_errors_have_fixed_behaviour() {
        // NotConfigured ends the session with a hint
        let r = rig(
            vec![utt("hi")],
            vec![B::Plan(Err(PlanError::NotConfigured))],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::NotConfigured);
        assert_eq!(
            r.log.says(),
            vec!["Set up a reply backend in the speech settings first."]
        );
        // other errors: apologise, keep going, nothing recorded
        for e in [
            PlanError::Timeout,
            PlanError::Backend,
            PlanError::Unparseable,
            PlanError::Empty,
            PlanError::TooLong,
        ] {
            let r = rig(
                vec![utt("hi"), utt("again"), L::Out(ListenOutcome::Aborted)],
                vec![B::Plan(Err(e)), answer("ok")],
            );
            assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
            assert_eq!(r.log.says()[0], "Sorry, I couldn't work that out.");
            let h = r.brain.histories.lock().unwrap().clone();
            assert!(
                h[1].is_empty(),
                "failed turn must not enter history ({e:?})"
            );
        }
        // Superseded: silent continue
        let r = rig(
            vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
            vec![B::Plan(Err(PlanError::Superseded))],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert!(r.log.says().is_empty());
    }

    #[tokio::test]
    async fn paste_plan_pastes_speaks_and_remembers_the_draft() {
        let r = rig(
            vec![
                utt("write a thank you note"),
                utt("make it shorter"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![
                B::Plan(Ok(paste_plan("Dear Sam, thank you so much.", "Pasted."))),
                B::Plan(Ok(paste_plan("Thanks Sam.", ""))),
            ],
        );
        run(&r, &cfg()).await;
        assert!(r.log.has("paste:Dear Sam, thank you so much."));
        assert!(r.log.has("paste:Thanks Sam."));
        assert_eq!(r.log.says(), vec!["Pasted.", "Done."]);
        let d = r.brain.drafts.lock().unwrap().clone();
        assert_eq!(d[0], None);
        assert_eq!(d[1].as_deref(), Some("Dear Sam, thank you so much."));
        let h = r.brain.histories.lock().unwrap().clone();
        assert_eq!(h[1][0].1, "Pasted. (pasted text)");
    }

    #[tokio::test]
    async fn paste_plan_failure_speaks_fixed_text_and_keeps_no_draft() {
        let r = rig(
            vec![
                utt("write something"),
                utt("next"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![
                B::Plan(Ok(paste_plan("secret body", "Done."))),
                answer("ok"),
            ],
        );
        r.hands
            .paste_script
            .lock()
            .unwrap()
            .push_back(Err("refused: secret body".into()));
        run(&r, &cfg()).await;
        assert_eq!(r.log.says()[0], "I couldn't paste that.");
        assert!(!r.log.says().iter().any(|s| s.contains("secret")));
        let d = r.brain.drafts.lock().unwrap().clone();
        assert_eq!(d[1], None);
    }

    #[tokio::test]
    async fn command_plan_uses_the_restricted_tier_only() {
        let r = rig(
            vec![utt("get me safari"), L::Out(ListenOutcome::Aborted)],
            vec![B::Plan(Ok(command_plan("open Safari", "")))],
        );
        r.hands
            .model_script
            .lock()
            .unwrap()
            .push_back(PhraseOutcome::Done("Opening Safari.".into()));
        run(&r, &cfg()).await;
        assert!(r.log.has("model:open Safari"));
        // the exact tier ran once for the raw utterance (NoMatch), never for the model's phrase
        assert!(!r.log.has("user:open Safari"));
        assert_eq!(r.log.says(), vec!["Opening Safari."]);
    }

    #[tokio::test]
    async fn command_plan_speech_wins_over_the_executor_message() {
        let r = rig(
            vec![utt("x"), L::Out(ListenOutcome::Aborted)],
            vec![B::Plan(Ok(command_plan(
                "pause the music",
                "Pausing for you.",
            )))],
        );
        r.hands
            .model_script
            .lock()
            .unwrap()
            .push_back(PhraseOutcome::Done("Paused.".into()));
        run(&r, &cfg()).await;
        assert_eq!(r.log.says(), vec!["Pausing for you."]);
    }

    #[tokio::test]
    async fn command_plan_failure_messages() {
        let r = rig(
            vec![utt("a"), utt("b"), utt("c"), L::Out(ListenOutcome::Aborted)],
            vec![
                B::Plan(Ok(command_plan("open nothing", ""))),
                B::Plan(Ok(command_plan("open /etc/hosts", ""))),
                B::Plan(Ok(command_plan("open broken", ""))),
            ],
        );
        r.hands.model_script.lock().unwrap().extend([
            PhraseOutcome::NoMatch,
            PhraseOutcome::Refused,
            PhraseOutcome::Failed,
        ]);
        run(&r, &cfg()).await;
        assert_eq!(
            r.log.says(),
            vec![
                "I can't do that yet.",
                "I can't do that.",
                "That didn't work."
            ]
        );
    }

    #[tokio::test]
    async fn end_plan_speaks_then_ends() {
        let r = rig(
            vec![utt("we are finished here, thanks")],
            vec![B::Plan(Ok(plan(PlanKind::End, "Talk soon.")))],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::UserEnded);
        assert_eq!(r.log.says(), vec!["Talk soon."]);
    }

    #[tokio::test]
    async fn clarify_is_spoken_and_recorded() {
        let r = rig(
            vec![utt("email him"), utt("Sam"), L::Out(ListenOutcome::Aborted)],
            vec![
                B::Plan(Ok(plan(PlanKind::Clarify, "Which Sam?"))),
                answer("Got it."),
            ],
        );
        run(&r, &cfg()).await;
        let h = r.brain.histories.lock().unwrap().clone();
        assert_eq!(
            h[1],
            vec![("email him".to_string(), "Which Sam?".to_string())]
        );
    }

    // ------------------------------------------------------------------
    // history
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn history_is_capped_to_context_turns_newest_kept() {
        let mut c = cfg();
        c.context_turns = 2;
        let r = rig(
            vec![
                utt("q1"),
                utt("q2"),
                utt("q3"),
                utt("q4"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![answer("a1"), answer("a2"), answer("a3"), answer("a4")],
        );
        run(&r, &c).await;
        let h = r.brain.histories.lock().unwrap().clone();
        assert_eq!(
            h[3],
            vec![
                ("q2".to_string(), "a2".to_string()),
                ("q3".to_string(), "a3".to_string())
            ]
        );
    }

    #[tokio::test]
    async fn zero_context_turns_keeps_nothing_including_the_draft() {
        let mut c = cfg();
        c.context_turns = 0;
        let r = rig(
            vec![
                utt("write x"),
                utt("shorter"),
                L::Out(ListenOutcome::Aborted),
            ],
            vec![B::Plan(Ok(paste_plan("draft body", "Done."))), answer("ok")],
        );
        run(&r, &c).await;
        assert!(r.brain.histories.lock().unwrap()[1].is_empty());
        assert_eq!(r.brain.drafts.lock().unwrap()[1], None);
    }

    #[tokio::test]
    async fn history_sides_are_truncated_to_600_chars() {
        let q = "q".repeat(600);
        let a = "a".repeat(399);
        let r = rig(
            vec![utt(&q), utt("next"), L::Out(ListenOutcome::Aborted)],
            vec![answer(&a), answer("ok")],
        );
        run(&r, &cfg()).await;
        let h = r.brain.histories.lock().unwrap().clone();
        assert!(h[1][0].0.chars().count() <= 600);
        assert!(h[1][0].1.chars().count() <= 600);
    }

    // ------------------------------------------------------------------
    // speaking
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn every_say_is_followed_by_wait_quiet_before_the_next_listen() {
        let r = rig(
            vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
            vec![answer("Hello.")],
        );
        run(&r, &cfg()).await;
        let log = r.log.all();
        let say = log.iter().position(|e| e == "say:Hello.").unwrap();
        let quiet = log.iter().position(|e| e == "quiet").unwrap();
        let second_listen = log.iter().rposition(|e| e == "listen").unwrap();
        assert!(say < quiet && quiet < second_listen);
    }

    #[tokio::test]
    async fn interrupted_and_failed_say_do_not_end_the_session() {
        for o in [SayOutcome::Interrupted, SayOutcome::Failed] {
            let r = rig(
                vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
                vec![answer("Hello.")],
            );
            r.mouth.say_script.lock().unwrap().push_back(S::Out(o));
            assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
            assert_eq!(r.log.count("listen"), 2, "must keep listening after {o:?}");
            assert!(r.log.has("quiet"));
        }
    }

    #[tokio::test]
    async fn still_speaking_triggers_stop_then_one_more_wait() {
        let r = rig(
            vec![utt("hi"), L::Out(ListenOutcome::Aborted)],
            vec![answer("Hello.")],
        );
        r.mouth
            .quiet_script
            .lock()
            .unwrap()
            .extend([QuietOutcome::StillSpeaking, QuietOutcome::Quiet]);
        run(&r, &cfg()).await;
        let log = r.log.all();
        let say = log.iter().position(|e| e == "say:Hello.").unwrap();
        let stop = log.iter().position(|e| e == "stop").unwrap();
        assert!(say < stop);
        assert_eq!(
            r.log.count("quiet"),
            2 + 0,
            "one wait before stop, one after"
        );
        assert_eq!(r.log.count("listen"), 2);
    }

    #[tokio::test]
    async fn persistent_speech_ends_with_speech_stuck_and_never_reopens_the_mic() {
        let r = rig(
            vec![utt("hi"), utt("never reached")],
            vec![answer("Hello.")],
        );
        r.mouth
            .quiet_script
            .lock()
            .unwrap()
            .extend([QuietOutcome::StillSpeaking, QuietOutcome::StillSpeaking]);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::SpeechStuck);
        assert_eq!(r.log.count("listen"), 1);
        assert_eq!(idle_count(&r), 1);
    }

    // ------------------------------------------------------------------
    // bounds, exactly-once Idle
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn max_turns_ends_with_turn_limit() {
        let mut c = cfg();
        c.max_turns = 3;
        let r = rig(
            vec![utt("a"), utt("b"), utt("c"), utt("d"), utt("e")],
            vec![
                answer("1"),
                answer("2"),
                answer("3"),
                answer("4"),
                answer("5"),
            ],
        );
        assert_eq!(run(&r, &c).await, SessionEnd::TurnLimit);
        assert_eq!(r.log.count("listen"), 3);
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn idle_is_reported_exactly_once_for_every_end() {
        // UserEnded
        let r = rig(vec![utt("stop")], vec![]);
        run(&r, &cfg()).await;
        assert_eq!(idle_count(&r), 1);
        assert_eq!(
            *r.reporter.0.lock().unwrap().last().unwrap(),
            SessionState::Idle
        );
        // NotConfigured
        let r = rig(vec![utt("x")], vec![B::Plan(Err(PlanError::NotConfigured))]);
        run(&r, &cfg()).await;
        assert_eq!(idle_count(&r), 1);
        // Cancelled
        let r = rig(vec![L::Out(ListenOutcome::Aborted)], vec![]);
        run(&r, &cfg()).await;
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn mouth_stop_is_only_called_on_cancel_in_simple_ends() {
        let r = rig(vec![utt("goodbye")], vec![]);
        run(&r, &cfg()).await;
        assert!(!r.log.has("stop"));
    }

    // ------------------------------------------------------------------
    // cancellation
    // ------------------------------------------------------------------

    #[tokio::test]
    async fn cancel_while_listening_ends_cancelled() {
        let r = rig(vec![L::Hang], vec![]);
        let cancel = r.cancel.clone();
        let canceller = async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.send_replace(true);
        };
        let c = cfg();
        let (end, _) = tokio::join!(run(&r, &c), canceller);
        assert_eq!(end, SessionEnd::Cancelled);
        assert!(r.log.has("stop"));
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn cancel_ready_at_the_same_instant_as_an_utterance_wins() {
        let r = rig(
            vec![L::CancelThen(ListenOutcome::Utterance("hello".into()))],
            vec![answer("never")],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(
            r.log.count("plan:") + r.log.count("user:") + r.log.count("paste:"),
            0
        );
        assert!(r.log.says().is_empty());
    }

    #[tokio::test]
    async fn cancel_ready_with_a_dictation_never_pastes() {
        let r = rig(
            vec![L::CancelThen(ListenOutcome::Utterance(
                "dictate hello there".into(),
            ))],
            vec![],
        );
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(r.log.count("paste:"), 0);
    }

    #[tokio::test]
    async fn cancel_ready_with_the_plan_drops_the_plan() {
        for plan_b in [
            B::CancelThen(Ok(paste_plan("body", "Done."))),
            B::CancelThen(Ok(command_plan("open Safari", ""))),
            B::CancelThen(Ok(plan(PlanKind::Answer, "never spoken"))),
        ] {
            let r = rig(vec![utt("do something")], vec![plan_b]);
            assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
            assert_eq!(r.log.count("paste:") + r.log.count("model:"), 0);
            assert!(r.log.says().is_empty());
            assert_eq!(idle_count(&r), 1);
        }
    }

    #[tokio::test]
    async fn cancel_during_say_stops_the_mouth_and_ends() {
        let r = rig(vec![utt("hi")], vec![answer("A very long answer.")]);
        r.mouth.say_script.lock().unwrap().push_back(S::Hang);
        let cancel = r.cancel.clone();
        let canceller = async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.send_replace(true);
        };
        let c = cfg();
        let (end, _) = tokio::join!(run(&r, &c), canceller);
        assert_eq!(end, SessionEnd::Cancelled);
        assert!(r.log.has("stop"));
        assert_eq!(r.log.count("listen"), 1);
    }

    #[tokio::test]
    async fn cancel_before_start_never_listens_or_acts() {
        let r = rig(vec![utt("hi")], vec![answer("x")]);
        r.cancel.send_replace(true);
        assert_eq!(run(&r, &cfg()).await, SessionEnd::Cancelled);
        assert_eq!(r.log.count("listen"), 0);
        assert_eq!(idle_count(&r), 1);
    }

    #[tokio::test]
    async fn cancel_during_terminal_farewell_speech_is_prompt() {
        let r = rig(vec![utt("goodbye")], vec![]);
        r.mouth.say_script.lock().unwrap().push_back(S::Hang);
        let cancel = r.cancel.clone();
        let canceller = async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            cancel.send_replace(true);
        };
        let c = cfg();
        let (end, _) = tokio::join!(run(&r, &c), canceller);
        assert_eq!(end, SessionEnd::Cancelled);
    }

    // ------------------------------------------------------------------
    // registry
    // ------------------------------------------------------------------

    #[test]
    fn registry_begin_stop_finish_lifecycle() {
        let reg = SessionRegistry::new();
        assert!(!reg.is_active());
        assert!(!reg.request_stop(), "stop on Idle is a no-op");
        let (id, token) = reg.try_begin().expect("begin from idle");
        assert_eq!(reg.state(), RegistryState::Running(id));
        assert!(reg.is_active());
        assert!(reg.try_begin().is_none(), "double begin refused");
        assert!(!*token.borrow());
        assert!(reg.request_stop());
        assert!(*token.borrow(), "stop must cancel the token");
        assert_eq!(reg.state(), RegistryState::Draining(id));
        assert!(!reg.request_stop(), "second stop is idempotent no-op");
        assert!(reg.is_active(), "draining still counts as active");
        assert!(reg.try_begin().is_none(), "draining blocks a new session");
        assert!(reg.finish(id));
        assert_eq!(reg.state(), RegistryState::Idle);
        let (id2, _t) = reg.try_begin().expect("begin after finish");
        assert!(id2 > id);
    }

    #[test]
    fn registry_stale_finish_cannot_free_a_newer_session() {
        let reg = SessionRegistry::new();
        let (old, _t) = reg.try_begin().unwrap();
        reg.request_stop();
        assert!(reg.finish(old));
        let (new, _t2) = reg.try_begin().unwrap();
        assert!(!reg.finish(old), "stale id must not free the new session");
        assert_eq!(reg.state(), RegistryState::Running(new));
        assert!(!reg.finish(new + 100));
        assert!(reg.finish(new), "finish of a Running session frees it too");
    }

    #[test]
    fn registry_concurrent_begin_yields_exactly_one_winner() {
        let reg = Arc::new(SessionRegistry::new());
        let wins = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let reg = reg.clone();
                let wins = wins.clone();
                std::thread::spawn(move || {
                    if reg.try_begin().is_some() {
                        wins.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(wins.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[test]
    fn registry_concurrent_stop_and_finish_never_wedge() {
        for _ in 0..50 {
            let reg = Arc::new(SessionRegistry::new());
            let (id, _t) = reg.try_begin().unwrap();
            let a = {
                let reg = reg.clone();
                std::thread::spawn(move || reg.request_stop())
            };
            let b = {
                let reg = reg.clone();
                std::thread::spawn(move || reg.finish(id))
            };
            a.join().unwrap();
            b.join().unwrap();
            // Whatever the interleaving, finishing once more must leave Idle.
            reg.finish(id);
            assert_eq!(reg.state(), RegistryState::Idle);
            assert!(reg.try_begin().is_some());
        }
    }
}
