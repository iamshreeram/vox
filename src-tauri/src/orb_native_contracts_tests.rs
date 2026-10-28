//! Contracts for the floating orb's native shell (G3b-B fix). Source-level pins for behaviour that
//! only runs against a real window system, plus pure checks of the ticker interval and of the
//! platform offer. Each pin names the rule it protects in its assertion message.

use crate::chat_presentation::{floating_supported, LinuxBackend, Os, PlatformEnv};
use crate::orb_runtime::tick_interval_ms;

const GUARD: &str = "if active || !reduced";

fn src(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Text of the function whose signature line contains `signature`, up to its closing brace at
/// column zero. Panics when the signature is missing, so a renamed function fails the contract.
fn fn_body<'a>(text: &'a str, signature: &str) -> &'a str {
    let start = text
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let rest = &text[start..];
    let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
    &rest[..end]
}

/// Whether every occurrence of `needle` in `body` comes after the first `guard`.
fn only_after_guard(body: &str, guard: &str, needle: &str) -> bool {
    let Some(guard_at) = body.find(guard) else {
        return false;
    };
    body.match_indices(needle).all(|(at, _)| at > guard_at)
}

#[test]
fn linux_button_read_destructures_the_four_tuple_from_device_position() {
    // gdk 0.18 `Window::device_position` returns (Option<Window>, i32, i32, ModifierType).
    let text = src("src/orb_native.rs");
    assert!(
        text.contains("let (_, _, _, mask) = root.device_position(&pointer);"),
        "the Linux button read must take the modifier mask from the 4-tuple"
    );
    assert!(
        !text.contains("let (_, _, mask) = root.device_position"),
        "the 3-tuple destructure does not compile against gdk 0.18"
    );
}

#[test]
fn the_ticker_never_calls_a_panel_or_visibility_getter() {
    // tauri-nspanel requires panel calls on the main thread, and the ticker is a side thread.
    let text = src("src/orb_native.rs");
    let body = fn_body(&text, "fn run_ticker(");
    for banned in ["get_webview_panel(", "is_visible("] {
        assert!(
            !body.contains(banned),
            "the ticker loop must not call {banned}: it runs off the main thread"
        );
    }
}

#[test]
fn the_ticker_reads_only_the_shown_flag_set_on_the_main_thread() {
    let text = src("src/orb_native.rs");
    assert!(
        fn_body(&text, "fn run_ticker(").contains("cell.shown()"),
        "the ticker must read the shown flag"
    );
    for setter in ["fn apply_on_main(", "fn stop_orb("] {
        assert!(
            fn_body(&text, setter).contains("set_shown("),
            "{setter} must keep the shown flag in step"
        );
    }
}

#[test]
fn pointer_down_wakes_the_ticker_so_the_first_gesture_tick_is_not_delayed() {
    let text = src("src/orb_native.rs");
    assert!(
        fn_body(&text, "pub fn pointer_down(").contains(".wake()"),
        "pointer_down must wake the ticker"
    );
    assert!(
        fn_body(&text, "pub fn set_reduced_motion(").contains(".wake()"),
        "turning reduced motion off must wake the ticker"
    );
    let body = fn_body(&text, "fn run_ticker(");
    assert!(
        body.contains(".wait("),
        "the ticker must sleep on its wake signal"
    );
    assert!(
        !body.contains("thread::sleep("),
        "the ticker must not sleep for a fixed interval the wake cannot cut short"
    );
}

#[test]
fn under_reduced_motion_the_ticker_touches_no_cursor_or_window_getter_without_a_gesture() {
    let text = src("src/orb_native.rs");
    let body = fn_body(&text, "fn run_ticker(");
    assert!(
        body.contains(GUARD),
        "the ticker must gate its polling on `{GUARD}`"
    );
    for getter in [
        "read_frame(&app)",
        "monitors_for(&app)",
        "left_button_down(",
    ] {
        assert!(
            only_after_guard(body, GUARD, getter),
            "{getter} must run only when a gesture is active or reduced motion is off"
        );
    }
}

#[test]
fn a_gesture_cancels_when_its_window_is_destroyed_or_the_display_changes() {
    let text = src("src/orb_native.rs");
    assert!(
        fn_body(&text, "fn attach(").contains("WindowEvent::Destroyed"),
        "the orb window's destruction must be handled"
    );
    assert!(
        text.contains("fn cancel_gesture("),
        "a cancel helper must exist"
    );
    assert!(
        fn_body(&text, "fn cancel_gesture(").contains("cancel(&mut calls)"),
        "the cancel helper must cancel through the driver, which never toggles the chat"
    );
    let ticker = fn_body(&text, "fn run_ticker(");
    assert!(
        ticker.contains("cancel_gesture(&app)"),
        "a display change found by the monitor check must cancel the gesture"
    );
    assert!(
        fn_body(&text, "fn attach(").contains("on_destroyed("),
        "a destroyed orb window must reach the destroyed handler"
    );
    assert!(
        fn_body(&text, "fn on_destroyed(").contains("cancel_gesture("),
        "the destroyed handler must cancel the gesture"
    );
}

#[test]
fn the_tick_is_16_ms_while_a_gesture_is_active_whatever_the_gaze_or_motion_state() {
    for gaze in [None, Some(33), Some(500)] {
        for reduced in [false, true] {
            assert_eq!(
                tick_interval_ms(true, gaze, reduced, true),
                16,
                "gesture active: gaze {gaze:?}, reduced {reduced}"
            );
        }
    }
}

#[test]
fn a_hidden_orb_waits_a_long_idle_time_that_a_wake_can_still_cut_short() {
    assert!(tick_interval_ms(false, None, false, false) >= 1_000);
    assert!(tick_interval_ms(false, Some(33), true, false) >= 1_000);
}

#[test]
fn reduced_motion_without_a_gesture_waits_at_least_250_ms() {
    assert!(tick_interval_ms(false, None, true, true) >= 250);
    assert!(tick_interval_ms(false, Some(33), true, true) >= 250);
}

#[test]
fn without_a_gesture_the_gaze_interval_is_honoured_and_defaults_to_250_ms() {
    assert_eq!(tick_interval_ms(false, Some(33), false, true), 33);
    assert_eq!(tick_interval_ms(false, Some(500), false, true), 500);
    assert_eq!(tick_interval_ms(false, None, false, true), 250);
}

#[test]
fn the_tray_orb_item_is_offered_only_where_floating_is_supported() {
    let env = |os, linux_backend, composited| PlatformEnv {
        os,
        linux_backend,
        composited,
    };
    assert!(floating_supported(&env(Os::MacOs, LinuxBackend::Unknown, true)).is_ok());
    assert!(floating_supported(&env(Os::Windows, LinuxBackend::Unknown, true)).is_ok());
    assert!(floating_supported(&env(Os::Linux, LinuxBackend::X11, true)).is_ok());
    assert!(floating_supported(&env(Os::Linux, LinuxBackend::Wayland, true)).is_err());
    assert!(floating_supported(&env(Os::Linux, LinuxBackend::Unknown, true)).is_err());
    assert!(floating_supported(&env(Os::Linux, LinuxBackend::X11, false)).is_err());
    assert!(floating_supported(&env(Os::Other, LinuxBackend::Unknown, true)).is_err());

    let orb = src("src/orb_native.rs");
    assert!(
        fn_body(&orb, "fn record_platform_offer(").contains("sync_tray("),
        "a change in the platform offer must resync the tray"
    );
    let tray = src("src/tray.rs");
    assert!(
        tray.contains("orb_native::floating_offered()"),
        "the tray must read the platform offer"
    );
    assert!(
        tray.contains("menu.remove(&orb_i)?"),
        "the tray must drop the orb item where floating is unsupported"
    );
}

#[test]
fn the_idle_face_doc_matches_the_code() {
    let text = src("src/commands/orb.rs");
    assert!(
        !text.contains("idle shows sleep, or concerned"),
        "the avatar_face doc must not say idle shows sleep without an end"
    );
    assert!(
        text.contains("idle with no end shows idle"),
        "the avatar_face doc must state that idle with no end shows idle"
    );
}
