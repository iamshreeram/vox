//! Authoritative tests for Stage W (wake word opens a conversation), design
//! DESIGN.md Rev 4 (.ram-status/duplex-jarvis-20261022-1316), amended by
//! .ram-status/vox-simplify-20261029-0900/DESIGN.md Rev 3 (Track W: one trigger).
//! Do not edit to make code pass.
//!
//! Contract under test:
//! * `settings::WakeWordAction` has two variants for bindings stability, but every stored
//!   value (missing, "conversation", "voice_request", unknown, non-string) reads as
//!   Conversation, and it always serializes as "conversation".
//! * `preflight_check` never refuses for the wake word; it refuses only while ambient
//!   listening is effective (`settings::ambient_effective`).
//! * `wakeword_listener::route_detection` (pure) decides Ignore / StartConversation.
//! * `wakeword_listener::wake_frames_suppressed` (pure) drops detector input during a
//!   session and for WAKE_SESSION_TAIL_MS after it ended.
//! * `wakeword_listener::start_conversation_from_wake` calls a start-only seam exactly
//!   once, stays silent for busy refusals, and reports every other refusal once.

use crate::managers::converse_hooks;
use crate::managers::converse_platform::{preflight_check, DICTATION_BUSY, ENROLLMENT_BUSY};
use crate::managers::converse_runtime::ALREADY_RUNNING;
use crate::managers::wakeword_listener::{
    route_detection, should_reset_engine_on_suppression_edge, start_conversation_from_wake,
    wake_frames_suppressed, ConversationStarter, WakeDetectionRoute, WAKE_SESSION_TAIL_MS,
};
use crate::settings::{get_default_settings, AppSettings, WakeWordAction};
use std::cell::{Cell, RefCell};

const AMBIENT_MSG: &str = "Turn off ambient listening before starting a conversation.";

/// Settings that pass every preflight rule except possibly the ambient one
/// (same builder shape as realtime_complete_tests::direct, which passes preflight).
fn valid() -> AppSettings {
    let mut s = get_default_settings();
    s.tts_enabled = true;
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_endpoint_url = "https://gw.example.com/anthropic".into();
    s.exp_rt_model = "some-model".into();
    s.exp_rt_token = "tok-123".into();
    s
}

// ---- settings ---------------------------------------------------------------------

#[test]
fn default_action_is_conversation_for_new_and_legacy_settings() {
    assert_eq!(
        get_default_settings().wake_word_action,
        WakeWordAction::Conversation
    );
    let legacy: AppSettings = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(legacy.wake_word_action, WakeWordAction::Conversation);
}

#[test]
fn action_serializes_as_conversation_whatever_the_variant() {
    // Every variant is written as the one value that is still meaningful.
    assert_eq!(
        serde_json::to_value(WakeWordAction::VoiceRequest).unwrap(),
        serde_json::json!("conversation")
    );
    assert_eq!(
        serde_json::to_value(WakeWordAction::Conversation).unwrap(),
        serde_json::json!("conversation")
    );
    let parsed: WakeWordAction = serde_json::from_value(serde_json::json!("conversation")).unwrap();
    assert_eq!(parsed, WakeWordAction::Conversation);
}

#[test]
fn stored_conversation_value_survives_in_app_settings() {
    let s: AppSettings =
        serde_json::from_value(serde_json::json!({ "wake_word_action": "conversation" })).unwrap();
    assert_eq!(s.wake_word_action, WakeWordAction::Conversation);
    let back = serde_json::to_value(&s).unwrap();
    assert_eq!(back["wake_word_action"], serde_json::json!("conversation"));
}

#[test]
fn stored_voice_request_loads_as_conversation() {
    // A-W1: the retired one-shot choice can never be restored by loading settings.
    let s: AppSettings =
        serde_json::from_value(serde_json::json!({ "wake_word_action": "voice_request" })).unwrap();
    assert_eq!(s.wake_word_action, WakeWordAction::Conversation);
}

#[test]
fn unknown_or_non_string_stored_value_loads_as_conversation() {
    for raw in [
        serde_json::json!("banana"),
        serde_json::json!(3),
        serde_json::json!(null),
        serde_json::json!({ "nested": true }),
    ] {
        let s: AppSettings =
            serde_json::from_value(serde_json::json!({ "wake_word_action": raw })).unwrap();
        assert_eq!(s.wake_word_action, WakeWordAction::Conversation, "{raw}");
    }
}

// ---- preflight --------------------------------------------------------------------

#[test]
fn sanity_valid_builder_passes_preflight() {
    assert_eq!(preflight_check(&valid()), Ok(()));
}

#[test]
fn wake_word_enabled_never_blocks_a_conversation() {
    // Wake word on is no longer a refusal, whatever the stored action.
    let mut s = valid();
    s.wake_word_enabled = true;
    s.wake_word_action = WakeWordAction::Conversation;
    assert_eq!(preflight_check(&s), Ok(()));
    s.wake_word_action = WakeWordAction::VoiceRequest;
    assert_eq!(preflight_check(&s), Ok(()));
}

#[test]
fn effective_ambient_listening_is_refused() {
    let mut s = valid();
    s.experimental_enabled = true;
    s.ambient_mode_enabled = true;
    assert_eq!(preflight_check(&s), Err(AMBIENT_MSG.to_string()));
    s.wake_word_enabled = true;
    assert_eq!(preflight_check(&s), Err(AMBIENT_MSG.to_string()));
}

#[test]
fn stored_ambient_flag_without_experimental_does_not_block() {
    // C1: Experimental off makes the stored flag inert for every gate.
    let mut s = valid();
    s.experimental_enabled = false;
    s.ambient_mode_enabled = true;
    s.wake_word_enabled = false;
    assert_eq!(preflight_check(&s), Ok(()));
}

#[test]
fn earlier_preflight_refusals_still_win_with_wake_word_on() {
    let mut s = valid();
    s.wake_word_enabled = true;
    s.tts_enabled = false;
    assert_eq!(
        preflight_check(&s),
        Err("Turn on spoken replies in Settings > Models > Text-to-speech first.".to_string())
    );
}

// ---- routing ----------------------------------------------------------------------

#[test]
fn route_detection_always_starts_a_conversation_unless_one_is_running() {
    use WakeDetectionRoute as R;
    assert_eq!(route_detection(false), R::StartConversation);
    // A live session always wins: a detection never starts anything.
    assert_eq!(route_detection(true), R::Ignore);
}

#[test]
fn wake_detection_handler_never_requests_a_transcription() {
    // A-W6: the detection handler is the only place a wake detection could reach the
    // dictation path, so pin that its production body never calls it.
    let src = include_str!("wakeword_listener.rs");
    let body = src
        .split("fn handle_detection(")
        .nth(1)
        .and_then(|rest| rest.split("#[cfg(test)]").next())
        .expect("handle_detection is present");
    assert!(
        !body.contains("send_transcription_input"),
        "a wake detection must never request a transcription"
    );
}

// ---- detector suppression ---------------------------------------------------------

#[test]
fn tail_constant_is_one_second() {
    assert_eq!(WAKE_SESSION_TAIL_MS, 1000);
}

#[test]
fn frames_are_suppressed_during_session_and_for_the_tail_only() {
    // Never ran a session: not suppressed.
    assert!(!wake_frames_suppressed(false, None));
    // Active session outranks everything, with or without an end time.
    assert!(wake_frames_suppressed(true, None));
    assert!(wake_frames_suppressed(true, Some(50_000)));
    // Tail window: [0, TAIL) suppressed, TAIL and later not.
    assert!(wake_frames_suppressed(false, Some(0)));
    assert!(wake_frames_suppressed(
        false,
        Some(WAKE_SESSION_TAIL_MS - 1)
    ));
    assert!(!wake_frames_suppressed(false, Some(WAKE_SESSION_TAIL_MS)));
    assert!(!wake_frames_suppressed(false, Some(60_000)));
}

#[test]
fn engine_resets_exactly_once_on_the_rising_suppression_edge() {
    assert!(should_reset_engine_on_suppression_edge(false, true));
    assert!(!should_reset_engine_on_suppression_edge(true, true));
    assert!(!should_reset_engine_on_suppression_edge(true, false));
    assert!(!should_reset_engine_on_suppression_edge(false, false));
}

#[test]
fn session_hooks_expose_time_since_session_end() {
    let _g = converse_hooks::TEST_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    converse_hooks::set_session_active(true);
    assert_eq!(
        converse_hooks::ms_since_session_end(),
        None,
        "none while active"
    );
    converse_hooks::set_session_active(false);
    let ms = converse_hooks::ms_since_session_end().expect("end time recorded");
    assert!(ms < WAKE_SESSION_TAIL_MS, "just ended, got {ms} ms");
}

// ---- start-only seam --------------------------------------------------------------

struct FakeStarter {
    result: Result<u64, String>,
    calls: Cell<u32>,
}

impl FakeStarter {
    fn new(result: Result<u64, String>) -> Self {
        Self {
            result,
            calls: Cell::new(0),
        }
    }
}

impl ConversationStarter for FakeStarter {
    fn start(&self) -> Result<u64, String> {
        self.calls.set(self.calls.get() + 1);
        self.result.clone()
    }
}

#[test]
fn successful_start_calls_once_and_reports_nothing() {
    let starter = FakeStarter::new(Ok(7));
    let notified = RefCell::new(Vec::<String>::new());
    let out =
        start_conversation_from_wake(&starter, &mut |m| notified.borrow_mut().push(m.to_string()));
    assert_eq!(out, Some(7));
    assert_eq!(starter.calls.get(), 1);
    assert!(notified.borrow().is_empty());
}

#[test]
fn setup_refusals_are_reported_exactly_once() {
    let msg = "Turn on spoken replies in the speech settings first.";
    let starter = FakeStarter::new(Err(msg.to_string()));
    let notified = RefCell::new(Vec::<String>::new());
    let out =
        start_conversation_from_wake(&starter, &mut |m| notified.borrow_mut().push(m.to_string()));
    assert_eq!(out, None);
    assert_eq!(starter.calls.get(), 1);
    assert_eq!(*notified.borrow(), vec![msg.to_string()]);
}

#[test]
fn busy_refusals_are_silent_because_a_spurious_wake_must_not_nag() {
    for busy in [ALREADY_RUNNING, DICTATION_BUSY, ENROLLMENT_BUSY] {
        let starter = FakeStarter::new(Err(busy.to_string()));
        let notified = RefCell::new(Vec::<String>::new());
        let out = start_conversation_from_wake(&starter, &mut |m| {
            notified.borrow_mut().push(m.to_string())
        });
        assert_eq!(out, None, "{busy}");
        assert_eq!(starter.calls.get(), 1, "{busy}");
        assert!(notified.borrow().is_empty(), "{busy} must not notify");
    }
}
