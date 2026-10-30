//! N1e spec: bubble notices (DESIGN.md A-C3). A notice is a refusal message the orb shows in the
//! existing bubble window. It is admitted without a conversation session and while transcript
//! bubbles are disabled, replaces the previous notice, dismisses after 4 s, and never changes the
//! orb. Time is a fake clock in ms. Authoritative: do not weaken an assertion; report a wrong one.
//!
//! Contract under test:
//!   bubble_notice::NoticeCode: Debug, Clone, Copy, PartialEq, Eq, serde (snake_case, unknown
//!     string -> Unknown), specta Type. Variants: DictationBusy, TtsOff, ComposerOff, Ambient,
//!     ExperimentalOff, TextEnding, Unknown. `as_str()` gives the snake_case code.
//!   bubble_notice::wire_parts(Option<NoticeCode>) -> (kind, notice_code) as sent on the wire.
//!   BubbleHub::on_notice(code, now); ProjItem.notice: Option<NoticeCode>; NOTICE_TTL_MS = 4000.
//!   BubbleShellCore::notice(code, &ShellInputs) -> Effects (no orb change, no session change).

use crate::bubble_hub::{BubbleHub, NOTICE_TTL_MS};
use crate::bubble_notice::{wire_parts, NoticeCode};
use crate::bubble_shell_core::{BubbleShellCore, ShellInputs};
use crate::managers::chat_log::GateState;

const ALL: [NoticeCode; 7] = [
    NoticeCode::DictationBusy,
    NoticeCode::TtsOff,
    NoticeCode::ComposerOff,
    NoticeCode::Ambient,
    NoticeCode::ExperimentalOff,
    NoticeCode::TextEnding,
    NoticeCode::Unknown,
];

/// A hub with the orb visible and a placement, transcript recording OFF, no session armed, and
/// the page of its instance ready.
fn ready_hub_without_transcripts() -> (BubbleHub, u64) {
    let mut hub = BubbleHub::new(false);
    hub.set_orb_visible(true);
    hub.set_classic_visible(false);
    hub.set_placement_ok(true);
    let instance = hub.new_instance();
    hub.set_ready(instance);
    (hub, instance)
}

fn inp(now: u64) -> ShellInputs {
    ShellInputs {
        now,
        gate: GateState {
            session_token: 0,
            rec_epoch: 0,
            recording: false,
        },
        orb_persisted_visible: true,
        supported: true,
        classic_visible: false,
        placement_ok: true,
    }
}

#[test]
fn notice_code_serialises_snake_case_and_round_trips() {
    let expected = [
        "dictation_busy",
        "tts_off",
        "composer_off",
        "ambient",
        "experimental_off",
        "text_ending",
        "unknown",
    ];
    for (code, name) in ALL.iter().zip(expected) {
        assert_eq!(code.as_str(), name);
        let json = serde_json::to_string(code).expect("serialise");
        assert_eq!(json, format!("\"{name}\""));
        let back: NoticeCode = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(back, *code);
    }
}

#[test]
fn unknown_or_malformed_code_maps_to_unknown() {
    let raw = "\"something_new\"";
    let parsed: NoticeCode = serde_json::from_str(raw).expect("unknown strings still parse");
    assert_eq!(parsed, NoticeCode::Unknown);
    let blank: NoticeCode = serde_json::from_str("\"\"").expect("empty string parses");
    assert_eq!(blank, NoticeCode::Unknown);
}

#[test]
fn wire_parts_mark_transcript_and_notice_items() {
    assert_eq!(wire_parts(None), ("transcript", None));
    assert_eq!(
        wire_parts(Some(NoticeCode::DictationBusy)),
        ("notice", Some("dictation_busy"))
    );
    assert_eq!(
        wire_parts(Some(NoticeCode::Unknown)),
        ("notice", Some("unknown"))
    );
}

#[test]
fn notice_is_admitted_without_session_and_without_transcripts() {
    let (mut hub, _instance) = ready_hub_without_transcripts();
    hub.on_notice(NoticeCode::TtsOff, 1_000);
    let items = hub.projection(1_000);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].notice, Some(NoticeCode::TtsOff));
    assert_eq!(items[0].text, "", "a notice carries no free text");
    assert_eq!(items[0].ttl_ms, NOTICE_TTL_MS);
}

#[test]
fn notice_waits_for_the_ready_page() {
    let mut hub = BubbleHub::new(false);
    hub.set_orb_visible(true);
    hub.set_placement_ok(true);
    let _instance = hub.new_instance();
    hub.on_notice(NoticeCode::Ambient, 0);
    assert!(hub.projection(0).is_empty());
}

#[test]
fn second_notice_replaces_the_first() {
    let (mut hub, _instance) = ready_hub_without_transcripts();
    hub.on_notice(NoticeCode::DictationBusy, 0);
    hub.on_notice(NoticeCode::TextEnding, 100);
    let items = hub.projection(100);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].notice, Some(NoticeCode::TextEnding));
}

#[test]
fn notice_is_dismissed_after_four_seconds() {
    let (mut hub, _instance) = ready_hub_without_transcripts();
    assert_eq!(NOTICE_TTL_MS, 4_000);
    hub.on_notice(NoticeCode::ComposerOff, 0);
    assert_eq!(hub.projection(3_999).len(), 1);
    assert!(hub.projection(4_000).is_empty());
    assert!(hub.publish(0).is_some(), "the notice is published first");
    assert!(hub.publish(4_000).is_some(), "the removal is published");
}

#[test]
fn core_notice_keeps_the_orb_and_the_session_untouched() {
    let mut core = BubbleShellCore::new(false);
    let instance = core.window_created(0);
    let _ = core.page_ready(instance, &inp(0));
    let _ = core.orb_visibility_changed(true, &inp(0));
    let before_share = core.orb_share();
    let before_desired = core.desired_orb(&inp(10));
    let effects = core.notice(NoticeCode::ExperimentalOff, &inp(10));
    assert!(effects.orb.is_none(), "a notice never changes the orb");
    assert!(
        !effects.open_classic,
        "a notice never opens the classic window"
    );
    assert_eq!(core.orb_share(), before_share);
    assert_eq!(core.desired_orb(&inp(10)), before_desired);
    let snapshot = effects.snapshot.expect("the notice is published");
    assert_eq!(snapshot.items.len(), 1);
    assert_eq!(snapshot.items[0].notice, Some(NoticeCode::ExperimentalOff));
}

#[test]
fn core_notice_hides_again_after_four_seconds() {
    let mut core = BubbleShellCore::new(false);
    let instance = core.window_created(0);
    let _ = core.page_ready(instance, &inp(0));
    let _ = core.orb_visibility_changed(true, &inp(0));
    let shown = core.notice(NoticeCode::DictationBusy, &inp(100));
    assert_eq!(shown.bubbles_visible, Some(true));
    let gone = core.refresh(&inp(100 + NOTICE_TTL_MS));
    assert_eq!(gone.bubbles_visible, Some(false));
}
