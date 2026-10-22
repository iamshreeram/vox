//! Authoritative tests for Stage W (wake word opens a conversation), design
//! DESIGN.md Rev 4 (.ram-status/duplex-jarvis-20261022-1316). Do not edit to make
//! code pass.
//!
//! Contract under test:
//! * `settings::WakeWordAction` { VoiceRequest (default, "voice_request"),
//!   Conversation ("conversation") }; unknown stored strings fall back to VoiceRequest.
//! * `preflight_check` refuses an enabled wake word ONLY when the action is not
//!   Conversation; ambient listening is always refused.
//! * `wakeword_listener::route_detection` (pure) decides Ignore / VoiceRequest /
//!   StartConversation.
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

const WAKE_OR_AMBIENT_MSG: &str =
    "Turn off wake word and ambient listening before starting a conversation.";

/// Settings that pass every preflight rule except possibly the wake/ambient one
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
fn default_action_is_voice_request_for_new_and_legacy_settings() {
    assert_eq!(
        get_default_settings().wake_word_action,
        WakeWordAction::VoiceRequest
    );
    let legacy: AppSettings = serde_json::from_value(serde_json::json!({})).unwrap();
    assert_eq!(legacy.wake_word_action, WakeWordAction::VoiceRequest);
}

#[test]
fn action_serializes_as_snake_case_strings() {
    assert_eq!(
        serde_json::to_value(WakeWordAction::VoiceRequest).unwrap(),
        serde_json::json!("voice_request")
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
fn unknown_stored_value_falls_back_instead_of_breaking_settings_load() {
    let s: AppSettings =
        serde_json::from_value(serde_json::json!({ "wake_word_action": "banana" })).unwrap();
    assert_eq!(s.wake_word_action, WakeWordAction::VoiceRequest);
}

// ---- preflight --------------------------------------------------------------------

#[test]
fn sanity_valid_builder_passes_preflight() {
    assert_eq!(preflight_check(&valid()), Ok(()));
}

#[test]
fn wake_word_enabled_with_voice_request_is_still_refused() {
    let mut s = valid();
    s.wake_word_enabled = true;
    s.wake_word_action = WakeWordAction::VoiceRequest;
    assert_eq!(preflight_check(&s), Err(WAKE_OR_AMBIENT_MSG.to_string()));
}

#[test]
fn wake_word_enabled_with_conversation_action_is_allowed() {
    let mut s = valid();
    s.wake_word_enabled = true;
    s.wake_word_action = WakeWordAction::Conversation;
    assert_eq!(preflight_check(&s), Ok(()));
}

#[test]
fn ambient_listening_is_refused_even_with_conversation_action() {
    let mut s = valid();
    s.ambient_mode_enabled = true;
    s.wake_word_action = WakeWordAction::Conversation;
    assert_eq!(preflight_check(&s), Err(WAKE_OR_AMBIENT_MSG.to_string()));
    s.wake_word_enabled = true;
    assert_eq!(preflight_check(&s), Err(WAKE_OR_AMBIENT_MSG.to_string()));
}

#[test]
fn earlier_preflight_refusals_still_win_with_conversation_action() {
    let mut s = valid();
    s.wake_word_enabled = true;
    s.wake_word_action = WakeWordAction::Conversation;
    s.tts_enabled = false;
    assert_eq!(
        preflight_check(&s),
        Err("Turn on spoken replies in the speech settings first.".to_string())
    );
}

// ---- routing ----------------------------------------------------------------------

#[test]
fn route_detection_matrix() {
    use WakeDetectionRoute as R;
    use WakeWordAction as A;
    assert_eq!(route_detection(A::VoiceRequest, false), R::VoiceRequest);
    assert_eq!(
        route_detection(A::Conversation, false),
        R::StartConversation
    );
    // A live session always wins: neither action may start anything.
    assert_eq!(route_detection(A::VoiceRequest, true), R::Ignore);
    assert_eq!(route_detection(A::Conversation, true), R::Ignore);
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
