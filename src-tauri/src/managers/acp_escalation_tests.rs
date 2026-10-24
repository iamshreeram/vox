//! Contract tests for `acp_escalation` (DESIGN.md rev 3.1, tests T31-T33).
//! Red until step S1 implements the module. Workers must not edit this file.

use crate::managers::acp_agent::{AcpAgentConfig, AcpAgentError, AcpReply, PromptOptions};
use crate::managers::acp_escalation::{
    on_context_setting_changed, on_history_cleared, run_escalation, user_message_for, AcpPrompter,
    EscalationEnv, EscalationSink,
};
use crate::managers::conversation::ConversationLog;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

type Hook = Box<dyn Fn() + Send + Sync>;

struct FakePrompter {
    calls: Mutex<Vec<(String, PromptOptions)>>,
    result: Mutex<Option<Result<AcpReply, AcpAgentError>>>,
    resets: AtomicUsize,
    hook: Option<Hook>,
}

impl FakePrompter {
    fn ok(reply: &str) -> Self {
        Self::with(Ok(AcpReply {
            text: reply.to_string(),
            agent_name: Some("fake".to_string()),
        }))
    }
    fn with(result: Result<AcpReply, AcpAgentError>) -> Self {
        FakePrompter {
            calls: Mutex::new(Vec::new()),
            result: Mutex::new(Some(result)),
            resets: AtomicUsize::new(0),
            hook: None,
        }
    }
    fn sent(&self) -> Vec<(String, PromptOptions)> {
        self.calls.lock().unwrap().clone()
    }
}

impl AcpPrompter for FakePrompter {
    fn prompt(
        &self,
        _cfg: &AcpAgentConfig,
        text: &str,
        opts: PromptOptions,
    ) -> Result<AcpReply, AcpAgentError> {
        self.calls.lock().unwrap().push((text.to_string(), opts));
        if let Some(hook) = &self.hook {
            hook();
        }
        self.result
            .lock()
            .unwrap()
            .take()
            .unwrap_or(Err(AcpAgentError::Protocol("no scripted result".into())))
    }
    fn reset_session(&self) {
        self.resets.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<String>>,
}

impl Recorder {
    fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().clone()
    }
}

impl EscalationSink for Recorder {
    fn reply(&self, text: &str) {
        self.events.lock().unwrap().push(format!("reply:{text}"));
    }
    fn exchange(&self, user: &str, reply: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("exchange:{user}|{reply}"));
    }
    fn error(&self, user_message: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("error:{user_message}"));
    }
    fn failed(&self, transcript: &str, message: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("failed:{transcript}|{message}"));
    }
}

/// Implements only the required methods, so `failed` must use the default.
#[derive(Default)]
struct ErrorOnlySink {
    events: Mutex<Vec<String>>,
}

impl EscalationSink for ErrorOnlySink {
    fn reply(&self, _text: &str) {}
    fn exchange(&self, _user: &str, _reply: &str) {}
    fn error(&self, user_message: &str) {
        self.events
            .lock()
            .unwrap()
            .push(format!("error:{user_message}"));
    }
}

fn cfg() -> AcpAgentConfig {
    AcpAgentConfig {
        command: PathBuf::from("/nonexistent/agent"),
        args: Vec::new(),
        cwd: PathBuf::from("/tmp"),
        turn_timeout: Duration::from_secs(5),
        handshake_timeout: Duration::from_secs(5),
        cancel_grace: Duration::from_secs(1),
    }
}

const TRANSCRIPT: &str = "open the pod bay doors";

fn no_recall(_: &str) -> Vec<String> {
    Vec::new()
}

fn env<'a>(
    log: &'a ConversationLog,
    ctx: bool,
    memory: bool,
    recall: &'a dyn Fn(&str) -> Vec<String>,
    ctx_now: &'a dyn Fn() -> bool,
    clock: &'a dyn Fn() -> u64,
) -> EscalationEnv<'a> {
    EscalationEnv {
        log,
        ctx_enabled: ctx,
        memory_enabled: memory,
        recall,
        ctx_enabled_now: ctx_now,
        now_ms: clock,
    }
}

fn yes() -> bool {
    true
}
fn no() -> bool {
    false
}
fn t0() -> u64 {
    1_000
}

#[test]
fn context_off_sends_raw_transcript_and_a_fresh_session_and_never_recalls() {
    let log = ConversationLog::new();
    let p = FakePrompter::ok("done");
    let sink = Recorder::default();
    let recalls = AtomicUsize::new(0);
    let recall = |_: &str| {
        recalls.fetch_add(1, Ordering::SeqCst);
        vec!["should not be used".to_string()]
    };
    let e = env(&log, false, true, &recall, &no, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    let sent = p.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, TRANSCRIPT, "raw transcript only");
    assert!(
        !sent[0].1.reuse_session,
        "context off => fresh session per turn"
    );
    assert_eq!(
        recalls.load(Ordering::SeqCst),
        0,
        "recall must not run with context off"
    );
    assert_eq!(log.len(), 0, "nothing recorded with context off");
    assert_eq!(
        sink.events().first().map(String::as_str),
        Some("reply:done")
    );
}

#[test]
fn context_on_reuses_the_session_and_injects_no_history() {
    let log = ConversationLog::new();
    let ticket = log.begin();
    assert!(log.record(ticket, "old question", "old answer", 500));
    let p = FakePrompter::ok("done");
    let sink = Recorder::default();
    let e = env(&log, true, false, &no_recall, &yes, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    let sent = p.sent();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].0.contains(TRANSCRIPT));
    assert!(
        !sent[0].0.contains("old question"),
        "history must not be re-sent"
    );
    assert!(!sent[0].0.contains("[Recent conversation]"));
    assert!(sent[0].1.reuse_session, "context on => one reused session");
}

#[test]
fn memory_facts_are_kept_with_context_on_but_history_never() {
    let log = ConversationLog::new();
    let ticket = log.begin();
    assert!(log.record(ticket, "earlier user turn", "earlier assistant turn", 500));
    let p = FakePrompter::ok("done");
    let sink = Recorder::default();
    let recall = |_: &str| vec!["the user likes tea".to_string()];
    let e = env(&log, true, true, &recall, &yes, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    let text = &p.sent()[0].0;
    assert!(text.contains("the user likes tea"));
    assert!(text.contains(TRANSCRIPT));
    assert!(!text.contains("earlier user turn"));
    assert!(!text.contains("[Recent conversation]"));
}

#[test]
fn enabled_context_records_exactly_one_exchange_and_emits_in_order() {
    let log = ConversationLog::new();
    let p = FakePrompter::ok("the reply");
    let sink = Recorder::default();
    let e = env(&log, true, false, &no_recall, &yes, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    assert_eq!(log.len(), 1);
    assert_eq!(
        log.recent(t0()),
        vec![(TRANSCRIPT.to_string(), "the reply".to_string())]
    );
    assert_eq!(
        sink.events(),
        vec![
            "reply:the reply".to_string(),
            format!("exchange:{TRANSCRIPT}|the reply"),
        ]
    );
}

#[test]
fn disabled_context_still_emits_the_reply_but_records_nothing() {
    let log = ConversationLog::new();
    let p = FakePrompter::ok("the reply");
    let sink = Recorder::default();
    let e = env(&log, false, false, &no_recall, &no, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    assert_eq!(log.len(), 0);
    let events = sink.events();
    assert!(events.contains(&"reply:the reply".to_string()));
    assert!(events.iter().all(|ev| !ev.starts_with("error:")));
}

#[test]
fn history_cleared_during_the_request_is_not_resurrected() {
    let log = std::sync::Arc::new(ConversationLog::new());
    let mut p = FakePrompter::ok("the reply");
    let log_for_hook = log.clone();
    p.hook = Some(Box::new(move || log_for_hook.clear()));
    let sink = Recorder::default();
    let e = env(&log, true, false, &no_recall, &yes, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    assert_eq!(log.len(), 0, "a clear during the request must win");
    assert!(sink.events().contains(&"reply:the reply".to_string()));
}

#[test]
fn context_disabled_during_the_request_is_not_recorded() {
    let log = ConversationLog::new();
    let p = FakePrompter::ok("the reply");
    let sink = Recorder::default();
    // enabled at dispatch, disabled when the reply arrives
    let e = env(&log, true, false, &no_recall, &no, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    assert_eq!(log.len(), 0);
}

#[test]
fn stale_result_is_completely_silent() {
    let log = ConversationLog::new();
    let p = FakePrompter::with(Err(AcpAgentError::Stale));
    let sink = Recorder::default();
    let e = env(&log, true, false, &no_recall, &yes, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    assert!(
        sink.events().is_empty(),
        "Stale => no reply, no exchange, no error"
    );
    assert_eq!(log.len(), 0);
}

#[test]
fn closed_result_is_silent() {
    let log = ConversationLog::new();
    let p = FakePrompter::with(Err(AcpAgentError::Closed));
    let sink = Recorder::default();
    let e = env(&log, true, false, &no_recall, &yes, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    assert!(sink.events().is_empty());
}

#[test]
fn an_error_emits_one_error_event_and_nothing_else() {
    let log = ConversationLog::new();
    let p = FakePrompter::with(Err(AcpAgentError::Timeout));
    let sink = Recorder::default();
    let e = env(&log, true, false, &no_recall, &yes, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    let expected = user_message_for(&AcpAgentError::Timeout).expect("timeout has a message");
    assert_eq!(
        sink.events(),
        vec![format!("failed:{TRANSCRIPT}|{expected}")]
    );
    assert_eq!(log.len(), 0);
}

#[test]
fn escalation_failure_reports_transcript_via_failed() {
    let log = ConversationLog::new();
    let p = FakePrompter::with(Err(AcpAgentError::Timeout));
    let sink = Recorder::default();
    let e = env(&log, true, false, &no_recall, &yes, &t0);
    run_escalation(&p, &sink, &e, &cfg(), TRANSCRIPT);
    let expected = user_message_for(&AcpAgentError::Timeout).expect("timeout has a message");
    let events = sink.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert!(
        events[0].starts_with(&format!("failed:{TRANSCRIPT}|")),
        "{events:?}"
    );
    assert!(events[0].ends_with(&expected), "{events:?}");
    assert!(
        events.iter().all(|ev| !ev.starts_with("error:")),
        "plain error line must not be emitted: {events:?}"
    );
}

#[test]
fn sink_default_failed_delegates_to_error() {
    let sink = ErrorOnlySink::default();
    sink.failed(TRANSCRIPT, "boom");
    assert_eq!(
        sink.events.lock().unwrap().clone(),
        vec!["error:boom".to_string()]
    );
}

fn every_error() -> Vec<AcpAgentError> {
    vec![
        AcpAgentError::NotConfigured,
        AcpAgentError::CommandNotFound,
        AcpAgentError::Unsupported,
        AcpAgentError::SpawnFailed("spawn detail".into()),
        AcpAgentError::HandshakeFailed("handshake detail".into()),
        AcpAgentError::Timeout,
        AcpAgentError::Busy,
        AcpAgentError::AgentExited {
            code: Some(3),
            stderr: "stderr detail".into(),
        },
        AcpAgentError::EmptyReply,
        AcpAgentError::Refused,
        AcpAgentError::Protocol("protocol detail".into()),
        AcpAgentError::Io(std::io::Error::other("io detail")),
    ]
}

#[test]
fn every_real_error_has_a_short_non_empty_user_message() {
    for err in every_error() {
        let message =
            user_message_for(&err).unwrap_or_else(|| panic!("{err:?} must produce a user message"));
        assert!(!message.trim().is_empty(), "{err:?}");
        assert!(message.chars().count() <= 400, "{err:?} message too long");
    }
}

#[test]
fn stale_and_closed_have_no_user_message() {
    assert!(user_message_for(&AcpAgentError::Stale).is_none());
    assert!(user_message_for(&AcpAgentError::Closed).is_none());
}

#[test]
fn agent_provided_detail_is_capped_at_200_characters() {
    let huge = "E".repeat(5_000);
    for err in [
        AcpAgentError::AgentExited {
            code: Some(1),
            stderr: huge.clone(),
        },
        AcpAgentError::HandshakeFailed(huge.clone()),
        AcpAgentError::Protocol(huge.clone()),
        AcpAgentError::SpawnFailed(huge.clone()),
    ] {
        let message = user_message_for(&err).expect("message");
        assert!(
            !message.contains(&"E".repeat(201)),
            "{err:?}: more than 200 characters of agent text leaked"
        );
    }
}

#[test]
fn history_cleared_always_resets_the_session() {
    // with a populated log
    let log = ConversationLog::new();
    let ticket = log.begin();
    assert!(log.record(ticket, "q", "a", 1));
    let p = FakePrompter::ok("x");
    on_history_cleared(Some(&log), &p);
    assert_eq!(log.len(), 0);
    assert_eq!(p.resets.load(Ordering::SeqCst), 1);
    // with an empty log
    let empty = ConversationLog::new();
    let p2 = FakePrompter::ok("x");
    on_history_cleared(Some(&empty), &p2);
    assert_eq!(p2.resets.load(Ordering::SeqCst), 1);
    // with no log at all
    let p3 = FakePrompter::ok("x");
    on_history_cleared(None, &p3);
    assert_eq!(p3.resets.load(Ordering::SeqCst), 1);
}

#[test]
fn disabling_context_clears_and_resets_but_enabling_does_nothing() {
    let log = ConversationLog::new();
    let ticket = log.begin();
    assert!(log.record(ticket, "q", "a", 1));
    let p = FakePrompter::ok("x");
    on_context_setting_changed(true, Some(&log), &p);
    assert_eq!(log.len(), 1, "enabling must not clear");
    assert_eq!(p.resets.load(Ordering::SeqCst), 0);
    on_context_setting_changed(false, Some(&log), &p);
    assert_eq!(log.len(), 0);
    assert_eq!(p.resets.load(Ordering::SeqCst), 1);
    on_context_setting_changed(false, None, &p);
    assert_eq!(
        p.resets.load(Ordering::SeqCst),
        2,
        "reset runs even without a log"
    );
}
