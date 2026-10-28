//! G2 spec: the chat surface dispatcher and its pure state machine (DESIGN.md Rev 5: 'Surface
//! state machine and placement', G2 step). Authoritative: do not weaken an assertion; if a test
//! is wrong, report it.
//!
//! Contract (module `crate::chat_surface`, declared in lib.rs):
//!   enum Visibility { Hidden, Showing(u64), Shown(u64), Hiding(u64) }          (Debug, Clone, Copy, PartialEq, Eq)
//!   enum Event { Open(OpenKind), Close, Toggle { focused: bool }, ShownAck(u64), HideTimer(u64), Destroyed }
//!   enum Effect { Show { focus: bool }, StartHide { gen: u64, after_ms: u64 }, HideNow, CancelTimers }   (Debug, Clone, PartialEq, Eq)
//!   struct SurfaceMachine; SurfaceMachine::new(exit_ms: u64); .visibility() -> Visibility; .gen() -> u64;
//!   .apply(Event) -> Vec<Effect>                                              (pure, no Tauri types)
//!   enum Target { Classic, Panel }                                            (Debug, Clone, Copy, PartialEq, Eq)
//!   const PANEL_AVAILABLE: bool = false                                       (G4 flips it, a deliberate tested change)
//!   fn target_for(setting: Option<Presentation>, env: &PlatformEnv) -> Target
//!   pub fn toggle(app: &AppHandle) -> tauri::Result<()>                       // the single entry for hotkey/tray/settings button
//!   pub fn open_on_session_start(app: &AppHandle)                             // the single entry for conversation start (OpenKind::Auto)
//! Semantics of the machine: every visibility transition bumps `gen`; Hiding(g) is left only by
//! HideTimer(g) with the same g; Open while Hiding invalidates the timer (new gen, CancelTimers).
//! Close with exit_ms == 0 hides at once (classic), otherwise Hiding + StartHide.
use crate::activation_policy::{desired, OpenKind, Policy};
use crate::chat_presentation::{Os, LinuxBackend, PlatformEnv, Presentation};
use crate::chat_surface::*;

fn open_user() -> Event {
    Event::Open(OpenKind::User)
}

#[test]
fn a_user_open_from_hidden_shows_with_focus_and_a_new_generation() {
    let mut m = SurfaceMachine::new(140);
    assert_eq!(m.visibility(), Visibility::Hidden);
    let g0 = m.gen();
    let fx = m.apply(open_user());
    assert_eq!(fx, vec![Effect::Show { focus: true }]);
    assert_eq!(m.visibility(), Visibility::Showing(m.gen()));
    assert!(m.gen() > g0);
    m.apply(Event::ShownAck(m.gen()));
    assert_eq!(m.visibility(), Visibility::Shown(m.gen()));
}

#[test]
fn auto_and_peek_opens_never_focus() {
    for kind in [OpenKind::Auto, OpenKind::Peek] {
        let mut m = SurfaceMachine::new(140);
        assert_eq!(m.apply(Event::Open(kind)), vec![Effect::Show { focus: false }]);
    }
}

#[test]
fn opening_an_already_visible_surface_only_refocuses_for_a_user_open() {
    let mut m = SurfaceMachine::new(140);
    m.apply(open_user());
    let gen = m.gen();
    m.apply(Event::ShownAck(gen));
    assert_eq!(m.apply(Event::Open(OpenKind::Auto)), vec![]);
    assert_eq!(m.apply(Event::Open(OpenKind::Peek)), vec![]);
    assert_eq!(m.apply(open_user()), vec![Effect::Show { focus: true }]);
    assert_eq!(m.gen(), gen, "no visibility change, no new generation");
    assert_eq!(m.visibility(), Visibility::Shown(gen));
}

#[test]
fn a_stale_shown_ack_is_ignored() {
    let mut m = SurfaceMachine::new(140);
    m.apply(open_user());
    let stale = m.gen() - 1;
    m.apply(Event::ShownAck(stale));
    assert_eq!(m.visibility(), Visibility::Showing(m.gen()));
}

#[test]
fn close_with_an_exit_animation_hides_only_when_its_own_timer_fires() {
    let mut m = SurfaceMachine::new(140);
    m.apply(open_user());
    m.apply(Event::ShownAck(m.gen()));
    let fx = m.apply(Event::Close);
    let hide_gen = m.gen();
    assert_eq!(fx, vec![Effect::StartHide { gen: hide_gen, after_ms: 140 }]);
    assert_eq!(m.visibility(), Visibility::Hiding(hide_gen));
    assert_eq!(m.apply(Event::HideTimer(hide_gen)), vec![Effect::HideNow]);
    assert_eq!(m.visibility(), Visibility::Hidden);
}

#[test]
fn close_then_reopen_within_the_exit_animation_stays_visible() {
    let mut m = SurfaceMachine::new(140);
    m.apply(open_user());
    m.apply(Event::ShownAck(m.gen()));
    m.apply(Event::Close);
    let old_timer_gen = m.gen();
    let fx = m.apply(open_user());
    assert_eq!(fx, vec![Effect::CancelTimers, Effect::Show { focus: true }]);
    assert!(m.gen() > old_timer_gen);
    // The old timer fires late: it must do nothing.
    assert_eq!(m.apply(Event::HideTimer(old_timer_gen)), vec![]);
    assert_eq!(m.visibility(), Visibility::Showing(m.gen()));
}

#[test]
fn close_without_an_exit_animation_hides_at_once() {
    let mut m = SurfaceMachine::new(0);
    m.apply(open_user());
    m.apply(Event::ShownAck(m.gen()));
    assert_eq!(m.apply(Event::Close), vec![Effect::HideNow]);
    assert_eq!(m.visibility(), Visibility::Hidden);
}

#[test]
fn close_when_hidden_or_hiding_does_nothing() {
    let mut m = SurfaceMachine::new(140);
    assert_eq!(m.apply(Event::Close), vec![]);
    m.apply(open_user());
    m.apply(Event::ShownAck(m.gen()));
    m.apply(Event::Close);
    let g = m.gen();
    assert_eq!(m.apply(Event::Close), vec![]);
    assert_eq!(m.gen(), g);
}

#[test]
fn toggle_hides_only_a_visible_focused_surface() {
    // Visible and focused: close.
    let mut m = SurfaceMachine::new(0);
    m.apply(open_user());
    m.apply(Event::ShownAck(m.gen()));
    assert_eq!(m.apply(Event::Toggle { focused: true }), vec![Effect::HideNow]);
    // Visible but not focused: bring it forward with focus.
    let mut m = SurfaceMachine::new(0);
    m.apply(open_user());
    m.apply(Event::ShownAck(m.gen()));
    assert_eq!(m.apply(Event::Toggle { focused: false }), vec![Effect::Show { focus: true }]);
    // Hidden: open.
    let mut m = SurfaceMachine::new(0);
    assert_eq!(m.apply(Event::Toggle { focused: false }), vec![Effect::Show { focus: true }]);
    // Hiding: a toggle reopens.
    let mut m = SurfaceMachine::new(140);
    m.apply(open_user());
    m.apply(Event::ShownAck(m.gen()));
    m.apply(Event::Close);
    let fx = m.apply(Event::Toggle { focused: true });
    assert_eq!(fx, vec![Effect::CancelTimers, Effect::Show { focus: true }]);
}

#[test]
fn destruction_cancels_timers_and_resets_to_hidden() {
    let mut m = SurfaceMachine::new(140);
    m.apply(open_user());
    m.apply(Event::ShownAck(m.gen()));
    m.apply(Event::Close);
    let before = m.gen();
    assert_eq!(m.apply(Event::Destroyed), vec![Effect::CancelTimers]);
    assert_eq!(m.visibility(), Visibility::Hidden);
    assert!(m.gen() > before);
    assert_eq!(m.apply(Event::HideTimer(before)), vec![], "a timer from the destroyed page is stale");
}

#[test]
fn generations_only_grow_over_a_long_random_sequence() {
    let mut m = SurfaceMachine::new(140);
    let mut last = m.gen();
    let mut seed = 12345u64;
    for _ in 0..2000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let ev = match (seed >> 33) % 7 {
            0 => Event::Open(OpenKind::User),
            1 => Event::Open(OpenKind::Auto),
            2 => Event::Close,
            3 => Event::Toggle { focused: seed & 1 == 0 },
            4 => Event::ShownAck(m.gen()),
            5 => Event::HideTimer(m.gen()),
            _ => Event::Destroyed,
        };
        m.apply(ev);
        assert!(m.gen() >= last);
        last = m.gen();
    }
}

// ---- Target selection: G2 keeps every entry point on the classic window ----

fn mac() -> PlatformEnv {
    PlatformEnv { os: Os::MacOs, linux_backend: LinuxBackend::Unknown, composited: false }
}

#[test]
fn until_the_panel_exists_every_setting_and_platform_resolves_to_classic() {
    assert!(!PANEL_AVAILABLE, "G4 flips this together with the default and its tests");
    for setting in [None, Some(Presentation::Floating), Some(Presentation::Window)] {
        assert_eq!(target_for(setting, &mac()), Target::Classic);
    }
}

// ---- Source contracts: one dispatcher, one policy owner ----

fn src(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn the_four_entry_points_go_through_the_dispatcher() {
    for file in ["src/actions.rs", "src/commands/chat.rs"] {
        let text = src(file);
        assert!(!text.contains("chat_window::toggle_chat_window"), "{file} must call chat_surface::toggle");
        assert!(text.contains("chat_surface::toggle"), "{file}");
    }
    let lib = src("src/lib.rs");
    assert!(!lib.contains("chat_window::toggle_chat_window("), "tray handler must call the dispatcher");
    assert!(lib.contains("chat_surface::toggle"));
    let converse = src("src/managers/converse_platform.rs");
    assert!(!converse.contains("chat_window::open_chat_window_on_session_start"));
    assert!(converse.contains("chat_surface::open_on_session_start"));
}

#[test]
fn the_dispatcher_does_not_log_content_or_touch_the_filesystem() {
    let text = src("src/chat_surface.rs");
    for banned in ["println!", "eprintln!", "dbg!", "std::fs", "File::"] {
        assert!(!text.contains(banned), "chat_surface.rs must not use {banned}");
    }
}

#[test]
fn chat_window_uses_the_policy_owner_and_no_other_policy_rule() {
    let text = src("src/chat_window.rs");
    assert!(text.contains("activation_policy::desired"), "hide path must ask desired()");
    // The old inline rule `!main_visible && tray_visible` is gone.
    assert!(!text.contains("!main_visible && tray_visible"));
}

/// The rule the code used before this change, kept here as the reference the owner must match
/// for the classic window after it has been hidden.
fn old_rule(main_visible: bool, tray_visible: bool) -> Policy {
    if !main_visible && tray_visible {
        Policy::Accessory
    } else {
        Policy::Regular
    }
}

#[test]
fn the_policy_owner_matches_the_old_hide_rule_for_the_classic_window() {
    for main in [false, true] {
        for tray in [false, true] {
            let inputs = crate::activation_policy::Inputs {
                main_visible: main,
                classic_chat_visible: false, // the chat window was just hidden
                panel_needs_focus: false,
                tray_present: tray,
            };
            assert_eq!(desired(&inputs), old_rule(main, tray), "main={main} tray={tray}");
        }
    }
}
