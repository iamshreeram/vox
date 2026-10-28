//! Bubble anchor contracts (issue #113). When the bubble stack sits Below the orb it must hug the
//! top of its window; for Above, Left and Right it keeps the bottom alignment. The anchor travels
//! as a content-free `anchor` field on `bubbles-state`, and the M5 exit path never builds the orb.
//! Authoritative: do not weaken an assertion; report it instead.

use crate::bubble_native::anchor_changed;
use crate::orb_geometry::{anchor_for_side, Side};

fn src(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Text of the item whose signature starts with `signature`, up to the closing brace at the same
/// indentation as the signature line.
fn fn_body<'a>(text: &'a str, signature: &str) -> &'a str {
    let start = text
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..start];
    let closing = format!("\n{indent}}}\n");
    let rest = &text[start..];
    let end = rest
        .find(closing.as_str())
        .map(|i| i + closing.len())
        .unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn anchor_for_side_is_top_only_when_the_stack_is_below_the_orb() {
    assert_eq!(anchor_for_side(Side::Below), "top");
    assert_eq!(anchor_for_side(Side::Above), "bottom");
    assert_eq!(anchor_for_side(Side::Left), "bottom");
    assert_eq!(anchor_for_side(Side::Right), "bottom");
}

#[test]
fn a_below_placement_yields_the_top_anchor() {
    let side = Side::Below;
    assert_eq!(anchor_for_side(side), "top");
}

#[test]
fn a_side_change_between_top_and_bottom_requires_a_re_emit() {
    assert!(anchor_changed("bottom", "top"));
    assert!(anchor_changed("top", "bottom"));
    assert!(!anchor_changed("top", "top"));
    assert!(!anchor_changed("bottom", "bottom"));
}

#[test]
fn show_bubbles_stores_the_anchor_and_re_emits_when_it_changes() {
    let text = src("src/bubble_native.rs");
    let show = fn_body(&text, "fn show_bubbles(");
    let derive = show
        .find("anchor_for_side(placement.side)")
        .expect("show_bubbles derives the anchor from the placement side");
    let store = show
        .find("anchor_top.swap(")
        .expect("show_bubbles stores the anchor");
    let apply = show
        .find("apply_rect(")
        .expect("show_bubbles places the window");
    let show_call = show
        .find("show_bubble_window(app)")
        .expect("show_bubbles shows the window");
    assert!(derive < store, "the anchor is derived before it is stored");
    assert!(
        store < show_call,
        "the anchor is stored before the window shows"
    );
    assert!(apply < show_call, "the window is placed before it shows");
    assert!(
        show.contains("anchor_changed("),
        "a changed anchor is detected"
    );
    assert!(
        show.contains("resend_snapshot("),
        "a changed anchor re-sends the current snapshot"
    );
}

#[test]
fn the_re_send_uses_the_current_revision_and_never_publishes() {
    let text = src("src/bubble_native.rs");
    let resend = fn_body(&text, "fn resend_snapshot(");
    assert!(
        resend.contains("current_snapshot("),
        "the re-send reads the current projection"
    );
    assert!(
        !resend.contains("publish("),
        "the re-send must not bump the revision"
    );
    let core = src("src/bubble_shell_core.rs");
    assert!(
        core.contains("pub fn current_snapshot("),
        "the core exposes the current snapshot"
    );
}

#[test]
fn every_snapshot_emit_carries_the_anchor_field() {
    let text = src("src/bubble_native.rs");
    let emit = fn_body(&text, "fn emit_snapshot(");
    assert!(emit.contains("anchor"), "emit_snapshot reads the anchor");
    let payload = fn_body(&text, "struct BubblesPayload");
    assert!(
        payload.contains("anchor: &'static str"),
        "the payload gains a content-free anchor field"
    );
}

#[test]
fn exit_arm_marks_exit_then_destroys_without_the_core_or_executor() {
    let lib = src("src/lib.rs");
    let arm_start = lib
        .find("tauri::RunEvent::Exit => {")
        .expect("the Exit arm exists");
    let arm = &lib[arm_start..arm_start + 200.min(lib.len() - arm_start)];
    let mark = arm
        .find("bubble_native::mark_exiting(app);")
        .expect("the Exit arm marks the exit first");
    let destroy = arm
        .find("bubble_native::destroy_bubbles(app);")
        .expect("the Exit arm destroys the bubble window");
    assert!(mark < destroy, "the exit mark comes before the destroy");
}

#[test]
fn destroy_bubbles_returns_before_the_core_and_executor_when_exiting() {
    let text = src("src/bubble_native.rs");
    let body = fn_body(&text, "pub fn destroy_bubbles(");
    let guard = body
        .find("shell.exiting.load(")
        .expect("destroy_bubbles checks the exit flag");
    let core_step = body
        .find("update(app, |core, inp| core.reset(inp))")
        .expect("the fallback and switch paths still reset the core");
    assert!(
        guard < core_step,
        "the exit flag is checked before the core is touched"
    );
    let before_guard = &body[..guard];
    for forbidden in [
        "update(",
        "schedule(",
        "execute_on_main",
        "orb_native::",
        "show_bubbles(",
    ] {
        assert!(
            !before_guard.contains(forbidden),
            "nothing before the exit guard may call `{forbidden}`"
        );
    }
}

#[test]
fn mark_exiting_sets_only_the_exit_flag() {
    let text = src("src/bubble_native.rs");
    let body = fn_body(&text, "pub fn mark_exiting(");
    assert!(body.contains("exiting.store(true"));
    for forbidden in ["update(", "schedule(", "orb_native::", "show_bubbles("] {
        assert!(
            !body.contains(forbidden),
            "mark_exiting does not call `{forbidden}`"
        );
    }
}
#[test]
fn destroy_bubbles_keeps_its_fallback_and_switch_behaviour() {
    let text = src("src/bubble_native.rs");
    let body = fn_body(&text, "pub fn destroy_bubbles(");
    assert!(
        body.contains("core.reset(inp)") || body.contains("|core, inp| core.reset(inp)"),
        "destroy_bubbles still resets the core for the fallback and switch paths"
    );
    assert!(
        body.contains(".destroy()"),
        "destroy_bubbles still destroys"
    );
}
