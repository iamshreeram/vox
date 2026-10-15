//! M4 S3 RED tests for the summary flow of `JobSpeaker` (DESIGN D6, critic C3 checks
//! (1)-(10)) and for the announcer's guarded finish speech. Red by design: the
//! summary stubs are `todo!()` and the announcer ignores its guard. Do not edit these
//! tests to make code pass.

use std::collections::VecDeque;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{JobManager, JobObserver, JobResult, JobSnapshot, JobState};
use crate::managers::job_announcer::{AnnounceKind, Announcer, LowPriorityMouth};
use crate::managers::job_speaker::{cancel, JobEventSink, JobSpeaker};
use crate::managers::job_summary::{
    ResultStore, ResultSummarizer, SpeechGuard, SummaryError, SummaryHandle, SummaryPolicy,
    SummaryTokens, Ticket, FALLBACK_SPEECH, SPEECH_PREFIX,
};

/// Fixed finish text for project "proj" on success (today's exact wording).
const FIXED_OK: &str = "The coding job in proj finished successfully.";

// ------------------------------------------------------------------ fakes

/// Mouth that records every text it accepted. `busy` makes it refuse everything.
#[derive(Default)]
struct SpeakerMouth {
    texts: Mutex<Vec<String>>,
    busy: AtomicBool,
}

impl SpeakerMouth {
    fn texts(&self) -> Vec<String> {
        self.texts.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl LowPriorityMouth for SpeakerMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        if self.busy.load(Ordering::SeqCst) {
            return false;
        }
        if let Ok(mut g) = self.texts.lock() {
            g.push(text.to_string());
        }
        true
    }
}

/// Records every emitted status DTO.
#[derive(Default)]
struct EventSink {
    events: Mutex<Vec<AgentJobStatusDto>>,
}

impl EventSink {
    fn events(&self) -> Vec<AgentJobStatusDto> {
        self.events.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl JobEventSink for EventSink {
    fn emit(&self, dto: &AgentJobStatusDto) {
        if let Ok(mut g) = self.events.lock() {
            g.push(dto.clone());
        }
    }
    fn refresh_tray(&self) {}
}

/// What the fake summarizer does when asked to submit.
#[derive(Clone)]
enum Reply {
    /// Answers at once with this result.
    Now(Result<String, SummaryError>),
    /// Never answers; the sender is kept alive so the handle is not `Failed`.
    Hold,
    /// Panics inside `submit`.
    Panic,
}

/// Summarizer fake: records every submitted text and returns handles per `reply`.
struct FakeSummarizer {
    reply: Mutex<Reply>,
    submitted: Mutex<Vec<String>>,
    held: Mutex<Vec<Sender<Result<String, SummaryError>>>>,
}

impl FakeSummarizer {
    fn new(reply: Reply) -> Arc<Self> {
        Arc::new(Self {
            reply: Mutex::new(reply),
            submitted: Mutex::new(Vec::new()),
            held: Mutex::new(Vec::new()),
        })
    }
    fn submitted(&self) -> Vec<String> {
        self.submitted.lock().map(|g| g.clone()).unwrap_or_default()
    }
    fn submit_count(&self) -> usize {
        self.submitted().len()
    }
    /// Sends `answer` to the `index`-th held (never answered) request.
    fn answer_held(&self, index: usize, answer: Result<String, SummaryError>) {
        if let Ok(g) = self.held.lock() {
            if let Some(tx) = g.get(index) {
                let _ = tx.send(answer);
            }
        }
    }
}

impl ResultSummarizer for FakeSummarizer {
    fn submit(&self, text: &str) -> SummaryHandle {
        if let Ok(mut g) = self.submitted.lock() {
            g.push(text.to_string());
        }
        let reply = self.reply.lock().map(|g| g.clone()).unwrap_or(Reply::Hold);
        let (tx, rx) = mpsc::channel();
        match reply {
            Reply::Now(answer) => {
                let _ = tx.send(answer);
                SummaryHandle::new(rx)
            }
            Reply::Hold => {
                if let Ok(mut g) = self.held.lock() {
                    g.push(tx);
                }
                SummaryHandle::new(rx)
            }
            Reply::Panic => panic!("summarizer submit exploded (test)"),
        }
    }
}

/// Everything a summary test needs, wired the way the manager wires it.
struct Fixture {
    speaker: Arc<JobSpeaker>,
    mouth: Arc<SpeakerMouth>,
    sink: Arc<EventSink>,
    summarizer: Arc<FakeSummarizer>,
    store: Arc<ResultStore>,
    tokens: Arc<SummaryTokens>,
}

fn ask_fixture(reply: Reply, tts: bool, ask: bool, deadline: Duration) -> Fixture {
    let mouth = Arc::new(SpeakerMouth::default());
    let announcer = Arc::new(Announcer::with_timing(
        mouth.clone(),
        Duration::from_millis(10),
        Duration::from_secs(60),
    ));
    let sink = Arc::new(EventSink::default());
    let summarizer = FakeSummarizer::new(reply);
    let store = ResultStore::new();
    let tokens = SummaryTokens::new();
    let policy = SummaryPolicy {
        ask_enabled: Arc::new(move || ask),
        tts_enabled: Arc::new(move || tts),
    };
    let speaker = JobSpeaker::new(
        "proj".to_string(),
        announcer,
        Arc::new(move || tts),
        sink.clone(),
    )
    .with_summary(summarizer.clone(), store.clone(), tokens.clone(), policy)
    .with_summary_deadline(deadline);
    Fixture {
        speaker: Arc::new(speaker),
        mouth,
        sink,
        summarizer,
        store,
        tokens,
    }
}

fn result(id: u64, text: &str) -> JobResult {
    JobResult {
        id,
        project: "proj".to_string(),
        text: text.to_string(),
    }
}

fn snap(id: u64, state: JobState) -> JobSnapshot {
    JobSnapshot {
        id,
        project: "proj".to_string(),
        state,
        elapsed_secs: 3,
        tail: Vec::new(),
    }
}

/// Polls `cond` until true or `limit` elapses.
fn wait_for(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if cond() {
            return true;
        }
        thread::sleep(Duration::from_millis(5));
    }
    cond()
}

/// Runs `observer.on_result(result)` on a helper thread. The receiver yields the
/// instant the call returned.
fn on_result_in_thread(
    speaker: &Arc<JobSpeaker>,
    r: JobResult,
) -> (thread::JoinHandle<()>, Receiver<Instant>) {
    let (tx, rx) = mpsc::channel();
    let sp = speaker.clone();
    let handle = thread::spawn(move || {
        sp.on_result(&r);
        let _ = tx.send(Instant::now());
    });
    (handle, rx)
}

// ------------------------------------------------------- E. JobSpeaker summary flow

#[test]
fn ask_and_tts_on_speaks_one_summary_instead_of_the_fixed_finish_text() {
    let fx = ask_fixture(
        Reply::Now(Ok("All good.".to_string())),
        true,
        true,
        Duration::from_secs(5),
    );
    fx.tokens.admit(1);
    fx.speaker.on_result(&result(1, "raw agent output"));
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(
        fx.mouth.texts(),
        vec!["The agent says: All good.".to_string()],
        "exactly one announcement, carrying the summary"
    );
    assert!(!fx.mouth.texts().iter().any(|t| t == FIXED_OK));
    assert_eq!(
        fx.summarizer.submitted(),
        vec!["raw agent output".to_string()]
    );
}

#[test]
fn summarizer_error_speaks_the_fallback_text() {
    let fx = ask_fixture(
        Reply::Now(Err(SummaryError::Failed)),
        true,
        true,
        Duration::from_secs(5),
    );
    fx.tokens.admit(1);
    fx.speaker.on_result(&result(1, "raw"));
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FALLBACK_SPEECH.to_string()]);
}

#[test]
fn summary_that_never_answers_falls_back_after_the_short_deadline() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_millis(300));
    fx.tokens.admit(1);
    let start = Instant::now();
    fx.speaker.on_result(&result(1, "raw"));
    let waited = start.elapsed();
    assert!(
        waited >= Duration::from_millis(250) && waited < Duration::from_millis(1500),
        "on_result returns about at the 300 ms deadline, took {waited:?}"
    );
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FALLBACK_SPEECH.to_string()]);
}

#[test]
fn summary_with_urls_is_cleaned_before_it_is_spoken() {
    let fx = ask_fixture(
        Reply::Now(Ok("See https://a.b/c now.".to_string())),
        true,
        true,
        Duration::from_secs(5),
    );
    fx.tokens.admit(1);
    fx.speaker.on_result(&result(1, "raw"));
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(
        fx.mouth.texts(),
        vec!["The agent says: See now.".to_string()]
    );
}

#[test]
fn summary_containing_nul_speaks_the_fallback_text() {
    let fx = ask_fixture(
        Reply::Now(Ok("bad\0text".to_string())),
        true,
        true,
        Duration::from_secs(5),
    );
    fx.tokens.admit(1);
    fx.speaker.on_result(&result(1, "raw"));
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FALLBACK_SPEECH.to_string()]);
}

#[test]
fn very_long_summary_is_capped_at_400_characters_after_the_prefix() {
    let long = "w".repeat(900);
    let fx = ask_fixture(Reply::Now(Ok(long)), true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    fx.speaker.on_result(&result(1, "raw"));
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    let said = fx.mouth.texts();
    assert_eq!(said.len(), 1, "one announcement");
    let body = said[0]
        .strip_prefix(SPEECH_PREFIX)
        .expect("spoken summary starts with the fixed prefix");
    assert_eq!(body.chars().count(), 400, "summary capped at 400 chars");
}

#[test]
fn task_mode_finish_without_a_result_speaks_the_fixed_text_and_never_summarizes() {
    let fx = ask_fixture(
        Reply::Now(Ok("should not be used".to_string())),
        true,
        true,
        Duration::from_secs(5),
    );
    fx.tokens.admit(1);
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(
        fx.summarizer.submit_count(),
        0,
        "no summarizer call for task jobs"
    );
    assert_eq!(fx.mouth.texts(), vec![FIXED_OK.to_string()]);
}

#[test]
fn failed_cancelled_and_timed_out_states_ignore_a_stray_result() {
    let cases = [
        (
            JobState::Failed { code: Some(3) },
            "The coding job in proj failed with exit code 3.",
        ),
        (JobState::Cancelled, "The coding job in proj was cancelled."),
        (JobState::TimedOut, "The coding job in proj timed out."),
    ];
    for (state, want) in cases {
        let fx = ask_fixture(
            Reply::Now(Ok("stray".to_string())),
            true,
            true,
            Duration::from_secs(5),
        );
        fx.tokens.admit(1);
        fx.speaker.on_result(&result(1, "stray result"));
        fx.speaker.on_finish(&snap(1, state));
        assert_eq!(
            fx.summarizer.submit_count(),
            0,
            "no summarizer call for {state:?}"
        );
        assert_eq!(
            fx.mouth.texts(),
            vec![want.to_string()],
            "fixed text for {state:?}"
        );
    }
}

#[test]
fn tts_off_skips_the_summarizer_but_keeps_the_result_for_the_dto() {
    let fx = ask_fixture(
        Reply::Now(Ok("never used".to_string())),
        false,
        true,
        Duration::from_secs(5),
    );
    fx.tokens.admit(1);
    fx.speaker.on_result(&result(1, "kept output"));
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(
        fx.summarizer.submit_count(),
        0,
        "no summary while tts is off"
    );
    assert!(
        fx.mouth.texts().is_empty(),
        "nothing spoken while tts is off"
    );
    assert_eq!(fx.store.get_for(1), Some("kept output".to_string()));
    let last = fx.sink.events().pop().expect("finish emits a dto");
    assert_eq!(last.result, Some("kept output".to_string()));
}

#[test]
fn ask_off_skips_the_summarizer_keeps_the_result_and_speaks_the_fixed_text() {
    let fx = ask_fixture(
        Reply::Now(Ok("never used".to_string())),
        true,
        false,
        Duration::from_secs(5),
    );
    fx.tokens.admit(1);
    fx.speaker.on_result(&result(1, "kept output"));
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(
        fx.summarizer.submit_count(),
        0,
        "no summary while ask is off"
    );
    assert_eq!(fx.store.get_for(1), Some("kept output".to_string()));
    assert_eq!(fx.mouth.texts(), vec![FIXED_OK.to_string()]);
}

#[test]
fn default_speaker_without_summary_keeps_todays_behavior_and_no_dto_result() {
    let mouth = Arc::new(SpeakerMouth::default());
    let announcer = Arc::new(Announcer::with_timing(
        mouth.clone(),
        Duration::from_millis(10),
        Duration::from_secs(60),
    ));
    let sink = Arc::new(EventSink::default());
    let speaker = JobSpeaker::new(
        "proj".to_string(),
        announcer,
        Arc::new(|| true),
        sink.clone(),
    );
    speaker.on_result(&result(1, "ignored by a plain speaker"));
    speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(mouth.texts(), vec![FIXED_OK.to_string()]);
    let last = sink.events().pop().expect("finish emits a dto");
    assert_eq!(last.result, None, "plain speaker never reports a result");
}

#[test]
fn dto_never_shows_a_result_stored_for_another_job() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_secs(5));
    fx.store.set(9, "belongs to job nine".to_string());
    fx.speaker.on_finish(&snap(7, JobState::Succeeded));
    for dto in fx.sink.events() {
        assert_eq!(dto.result, None, "job 7 must not show job 9's result");
    }
    assert_eq!(fx.store.get_for(7), None);
}

#[test]
fn admitting_a_newer_job_while_on_result_waits_aborts_it_and_keeps_the_fixed_text() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    let (handle, done) = on_result_in_thread(&fx.speaker, result(1, "raw"));
    assert!(
        wait_for(Duration::from_secs(5), || fx.summarizer.submit_count() == 1),
        "summary must be requested before the barrier"
    );
    let admitted = Instant::now();
    fx.tokens.admit(2);
    let returned = done
        .recv_timeout(Duration::from_secs(12))
        .expect("on_result must return after the newer admission");
    assert!(
        returned.saturating_duration_since(admitted) < Duration::from_millis(600),
        "on_result released within ~600 ms of the newer admission"
    );
    handle.join().expect("on_result thread");
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FIXED_OK.to_string()]);
    assert_eq!(fx.summarizer.submit_count(), 1, "no further submission");
}

#[test]
fn delayed_observer_for_an_old_job_never_submits_after_a_newer_admission() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    fx.tokens.admit(2);
    fx.speaker
        .on_result(&result(1, "late delivery for job one"));
    assert_eq!(
        fx.summarizer.submit_count(),
        0,
        "stale observer never submits"
    );
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FIXED_OK.to_string()]);
}

#[test]
fn toggling_ask_or_tts_while_waiting_aborts_the_summary() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    let (handle, done) = on_result_in_thread(&fx.speaker, result(1, "raw"));
    assert!(wait_for(Duration::from_secs(5), || fx
        .summarizer
        .submit_count()
        == 1));
    let toggled = Instant::now();
    fx.tokens.invalidate();
    let returned = done
        .recv_timeout(Duration::from_secs(12))
        .expect("on_result returns after the settings toggle");
    assert!(returned.saturating_duration_since(toggled) < Duration::from_millis(600));
    handle.join().expect("on_result thread");
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FIXED_OK.to_string()]);
}

#[test]
fn off_then_on_before_the_answer_still_invalidates_the_summary() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    let (handle, done) = on_result_in_thread(&fx.speaker, result(1, "raw"));
    assert!(wait_for(Duration::from_secs(5), || fx
        .summarizer
        .submit_count()
        == 1));
    fx.tokens.invalidate(); // toggle off
    fx.tokens.invalidate(); // toggle back on
    fx.summarizer
        .answer_held(0, Ok("arrived too late".to_string()));
    done.recv_timeout(Duration::from_secs(12))
        .expect("on_result returns");
    handle.join().expect("on_result thread");
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FIXED_OK.to_string()]);
}

#[test]
fn closing_the_tokens_on_exit_while_waiting_releases_on_result_with_the_fixed_text() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    let (handle, done) = on_result_in_thread(&fx.speaker, result(1, "raw"));
    assert!(wait_for(Duration::from_secs(5), || fx
        .summarizer
        .submit_count()
        == 1));
    let closed = Instant::now();
    fx.tokens.close();
    let returned = done
        .recv_timeout(Duration::from_secs(12))
        .expect("on_result returns on exit");
    assert!(returned.saturating_duration_since(closed) < Duration::from_millis(600));
    handle.join().expect("on_result thread");
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FIXED_OK.to_string()]);
}

#[test]
fn a_summary_invalidated_right_after_it_arrives_is_dropped_for_the_fixed_text() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    let (handle, done) = on_result_in_thread(&fx.speaker, result(1, "raw"));
    assert!(wait_for(Duration::from_secs(5), || fx
        .summarizer
        .submit_count()
        == 1));
    fx.summarizer.answer_held(0, Ok("arrived".to_string()));
    fx.tokens.invalidate();
    done.recv_timeout(Duration::from_secs(12))
        .expect("on_result returns");
    handle.join().expect("on_result thread");
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    let said = fx.mouth.texts();
    assert_eq!(
        said,
        vec![FIXED_OK.to_string()],
        "stale summary never spoken"
    );
    assert!(!said.iter().any(|t| t.starts_with(SPEECH_PREFIX)));
}

#[test]
fn a_panicking_summarizer_is_isolated_and_on_finish_still_speaks_the_fallback() {
    let fx = ask_fixture(Reply::Panic, true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        fx.speaker.on_result(&result(1, "raw"));
    }));
    assert!(outcome.is_ok(), "on_result must not propagate the panic");
    fx.speaker.on_finish(&snap(1, JobState::Succeeded));
    assert_eq!(fx.mouth.texts(), vec![FALLBACK_SPEECH.to_string()]);
}

#[test]
fn cancel_and_close_return_immediately_while_on_result_is_blocked() {
    let fx = ask_fixture(Reply::Hold, true, true, Duration::from_secs(5));
    fx.tokens.admit(1);
    let (handle, done) = on_result_in_thread(&fx.speaker, result(1, "raw"));
    assert!(wait_for(Duration::from_secs(5), || fx
        .summarizer
        .submit_count()
        == 1));

    let manager = JobManager::new();
    let t0 = Instant::now();
    let _ = cancel(&manager);
    assert!(
        t0.elapsed() < Duration::from_millis(50),
        "cancel must not wait for delivery"
    );

    let t1 = Instant::now();
    fx.tokens.close();
    assert!(
        t1.elapsed() < Duration::from_millis(50),
        "exit close must not wait for delivery"
    );

    done.recv_timeout(Duration::from_secs(12))
        .expect("on_result eventually returns");
    handle.join().expect("on_result thread");
}

// ------------------------------------------------------- F. announcer guard

/// Mouth with a scripted sequence of busy/idle answers that records what it accepted
/// and how many times it was offered text.
struct GuardMouth {
    script: Mutex<VecDeque<bool>>,
    default: AtomicBool,
    calls: AtomicUsize,
    texts: Mutex<Vec<String>>,
}

impl GuardMouth {
    fn new(script: &[bool], default: bool) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.iter().copied().collect()),
            default: AtomicBool::new(default),
            calls: AtomicUsize::new(0),
            texts: Mutex::new(Vec::new()),
        })
    }
    fn texts(&self) -> Vec<String> {
        self.texts.lock().map(|g| g.clone()).unwrap_or_default()
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl LowPriorityMouth for GuardMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let scripted = self.script.lock().ok().and_then(|mut g| g.pop_front());
        let answer = scripted.unwrap_or_else(|| self.default.load(Ordering::SeqCst));
        if answer {
            if let Ok(mut g) = self.texts.lock() {
                g.push(text.to_string());
            }
        }
        answer
    }
}

const SUMMARY: &str = "The agent says: summary text";
const FALLBACK: &str = "The coding job in proj finished successfully.";

fn guard_for(tokens: &Arc<SummaryTokens>, ticket: Ticket, fallback: &str) -> SpeechGuard {
    SpeechGuard {
        tokens: tokens.clone(),
        ticket,
        fallback: fallback.to_string(),
    }
}

/// Retry interval long enough that only explicit `notify_idle` calls retry in a test.
fn quiet_announcer(mouth: Arc<GuardMouth>) -> Arc<Announcer> {
    Arc::new(Announcer::with_timing(
        mouth,
        Duration::from_secs(30),
        Duration::from_secs(120),
    ))
}

#[test]
fn f1_guarded_finish_with_a_valid_ticket_and_idle_mouth_speaks_the_summary_once() {
    let mouth = GuardMouth::new(&[], true);
    let announcer = quiet_announcer(mouth.clone());
    let tokens = SummaryTokens::new();
    tokens.admit(1);
    let ticket = tokens.begin(1).expect("ticket for the latest job");
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard_for(&tokens, ticket, FALLBACK),
    );
    assert_eq!(
        mouth.texts(),
        vec![SUMMARY.to_string()],
        "summary spoken once"
    );
    assert_eq!(mouth.calls(), 1);
}

#[test]
fn f2_invalidation_while_the_gate_is_closed_speaks_only_the_fallback_once() {
    for which in ["admit", "invalidate", "close"] {
        let mouth = GuardMouth::new(&[], true);
        let announcer = quiet_announcer(mouth.clone());
        let tokens = SummaryTokens::new();
        tokens.admit(1);
        let ticket = tokens.begin(1).expect("ticket");
        announcer.set_conversation_active(true);
        announcer.announce_guarded(
            AnnounceKind::Finish,
            SUMMARY.to_string(),
            guard_for(&tokens, ticket, FALLBACK),
        );
        match which {
            "admit" => tokens.admit(2),
            "invalidate" => tokens.invalidate(),
            _ => tokens.close(),
        }
        announcer.set_conversation_active(false);
        assert!(
            wait_for(Duration::from_secs(2), || !mouth.texts().is_empty()),
            "fallback spoken after the gate opens ({which})"
        );
        thread::sleep(Duration::from_millis(30));
        assert_eq!(
            mouth.texts(),
            vec![FALLBACK.to_string()],
            "only the fallback, exactly once ({which})"
        );
    }
}

#[test]
fn f3_busy_mouth_then_invalidation_speaks_the_fallback_and_never_the_summary() {
    let mouth = GuardMouth::new(&[false], true);
    let announcer = quiet_announcer(mouth.clone());
    let tokens = SummaryTokens::new();
    tokens.admit(1);
    let ticket = tokens.begin(1).expect("ticket");
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard_for(&tokens, ticket, FALLBACK),
    );
    assert!(mouth.texts().is_empty(), "first attempt was busy");
    tokens.invalidate();
    announcer.notify_idle();
    assert_eq!(mouth.texts(), vec![FALLBACK.to_string()], "fallback only");
    assert!(
        !mouth.texts().iter().any(|t| t == SUMMARY),
        "summary never spoken"
    );
}

#[test]
fn f4_invalidation_after_acceptance_neither_retracts_nor_repeats_the_summary() {
    let mouth = GuardMouth::new(&[], true);
    let announcer = quiet_announcer(mouth.clone());
    let tokens = SummaryTokens::new();
    tokens.admit(1);
    let ticket = tokens.begin(1).expect("ticket");
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard_for(&tokens, ticket, FALLBACK),
    );
    tokens.invalidate();
    announcer.notify_idle();
    thread::sleep(Duration::from_millis(30));
    assert_eq!(
        mouth.texts(),
        vec![SUMMARY.to_string()],
        "spoken exactly once"
    );
    assert_eq!(mouth.calls(), 1, "never offered again");
}

#[test]
fn f5_guard_is_evaluated_on_every_retry_attempt_so_a_valid_retry_speaks_the_summary() {
    let mouth = GuardMouth::new(&[false], true);
    let announcer = quiet_announcer(mouth.clone());
    let tokens = SummaryTokens::new();
    tokens.admit(1);
    let ticket = tokens.begin(1).expect("ticket");
    announcer.announce_guarded(
        AnnounceKind::Finish,
        SUMMARY.to_string(),
        guard_for(&tokens, ticket, FALLBACK),
    );
    assert!(mouth.texts().is_empty(), "attempt one busy");
    announcer.notify_idle();
    assert_eq!(
        mouth.texts(),
        vec![SUMMARY.to_string()],
        "attempt two spoke the summary"
    );
    assert_eq!(mouth.calls(), 2, "two attempts, each guarded");
}

#[test]
fn f6_unguarded_finish_and_progress_keep_m3_order_and_eviction() {
    let mouth = GuardMouth::new(&[], true);
    let announcer = quiet_announcer(mouth.clone());
    announcer.set_conversation_active(true);
    announcer.announce(AnnounceKind::Progress, "p1".to_string());
    announcer.announce(AnnounceKind::Finish, "f".to_string());
    assert_eq!(
        announcer.take_pending(),
        vec!["p1".to_string(), "f".to_string()]
    );
    assert_eq!(mouth.calls(), 0);
}

#[test]
fn f6_full_queue_of_unguarded_finishes_drops_an_incoming_finish() {
    let mouth = GuardMouth::new(&[], true);
    let announcer = quiet_announcer(mouth.clone());
    announcer.set_conversation_active(true);
    for label in ["f1", "f2", "f3", "f4"] {
        announcer.announce(AnnounceKind::Finish, label.to_string());
    }
    announcer.announce(AnnounceKind::Finish, "f5".to_string());
    assert_eq!(
        announcer.take_pending(),
        vec![
            "f1".to_string(),
            "f2".to_string(),
            "f3".to_string(),
            "f4".to_string()
        ]
    );
}

#[test]
fn f6_queue_of_four_progress_evicts_the_oldest_when_a_fifth_arrives() {
    let mouth = GuardMouth::new(&[], true);
    let announcer = quiet_announcer(mouth.clone());
    announcer.set_conversation_active(true);
    for label in ["p1", "p2", "p3", "p4", "p5"] {
        announcer.announce(AnnounceKind::Progress, label.to_string());
    }
    assert_eq!(
        announcer.take_pending(),
        vec![
            "p2".to_string(),
            "p3".to_string(),
            "p4".to_string(),
            "p5".to_string()
        ]
    );
}

#[test]
fn f7_guarded_finishes_obey_the_same_capacity_rules_as_finishes() {
    let mouth = GuardMouth::new(&[], true);
    let announcer = quiet_announcer(mouth.clone());
    let tokens = SummaryTokens::new();
    tokens.admit(1);
    let ticket = tokens.begin(1).expect("ticket");
    announcer.set_conversation_active(true);
    for n in 1..=4 {
        announcer.announce_guarded(
            AnnounceKind::Finish,
            format!("s{n}"),
            guard_for(&tokens, ticket, FALLBACK),
        );
    }
    announcer.announce(AnnounceKind::Finish, "plain5".to_string());
    announcer.announce_guarded(
        AnnounceKind::Finish,
        "s5".to_string(),
        guard_for(&tokens, ticket, FALLBACK),
    );
    assert_eq!(
        announcer.take_pending(),
        vec![
            "s1".to_string(),
            "s2".to_string(),
            "s3".to_string(),
            "s4".to_string()
        ],
        "a full queue of finishes drops every incoming finish"
    );
}

#[test]
fn f8_lock_order_admit_waits_for_a_slow_guarded_mouth_call_and_does_not_deadlock() {
    struct SlowMouth {
        entered: Mutex<Sender<()>>,
        exited_at: Mutex<Option<Instant>>,
    }
    impl LowPriorityMouth for SlowMouth {
        fn try_speak_if_idle(&self, _text: &str) -> bool {
            if let Ok(tx) = self.entered.lock() {
                let _ = tx.send(());
            }
            thread::sleep(Duration::from_millis(50));
            if let Ok(mut g) = self.exited_at.lock() {
                *g = Some(Instant::now());
            }
            true
        }
    }

    let (entered_tx, entered_rx) = mpsc::channel();
    let mouth = Arc::new(SlowMouth {
        entered: Mutex::new(entered_tx),
        exited_at: Mutex::new(None),
    });
    let announcer = Arc::new(Announcer::with_timing(
        mouth.clone(),
        Duration::from_secs(30),
        Duration::from_secs(120),
    ));
    let tokens = SummaryTokens::new();
    tokens.admit(1);
    let ticket = tokens.begin(1).expect("ticket");

    let speaker_thread = {
        let announcer = announcer.clone();
        let guard = guard_for(&tokens, ticket, FALLBACK);
        thread::spawn(move || {
            announcer.announce_guarded(AnnounceKind::Finish, SUMMARY.to_string(), guard);
        })
    };
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the slow mouth call must start");

    let (admit_tx, admit_rx) = mpsc::channel();
    let admitter = {
        let tokens = tokens.clone();
        thread::spawn(move || {
            tokens.admit(2);
            let _ = admit_tx.send(Instant::now());
        })
    };
    let admit_done = admit_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("admit must complete within 2 s (no deadlock)");
    let mouth_returned = mouth
        .exited_at
        .lock()
        .ok()
        .and_then(|g| *g)
        .expect("the slow mouth call returned");
    assert!(
        admit_done > mouth_returned,
        "admit completes strictly after the guarded mouth call returned"
    );
    speaker_thread.join().expect("speaker thread");
    admitter.join().expect("admit thread");
}
