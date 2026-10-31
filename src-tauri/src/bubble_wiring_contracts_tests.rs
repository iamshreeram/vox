//! G4b spec: source contracts for the bubble window, its readiness command and the wiring of the
//! shell core into the converse runtime (DESIGN.md A7-2, A7-4, A8-2, A9-2, A24, A27, A30, A38;
//! Revision 10 contracts 2 and 4). Tolerant substring checks on named identifiers, in the style of
//! orb_native_contracts_tests.rs. Authoritative: do not weaken an assertion; report it instead.

use crate::window_commands::{window_command_allowed, BUBBLES_WINDOW_LABEL};

fn src(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Text of the item whose signature starts with `signature`, up to the closing brace at the same
/// indentation as the signature line (a method's `    }` or a free fn's `}`). Panics when the
/// signature is missing, so a renamed function fails the contract.
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

/// The body of every `loop {` in `text` (brace matched), so each can be checked for a stop path.
fn loop_blocks(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(rel) = text[from..].find("loop {") {
        let open = from + rel + "loop ".len();
        let mut depth = 0usize;
        let mut end = text.len();
        for (i, c) in text[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = open + i + 1;
                        break;
                    }
                }
                _ => {}
            }
        }
        out.push(&text[open..end]);
        from = end.max(open + 1);
    }
    out
}

// ---- (a) the bubble window: native flags, URL, activation and logging ----------------------------

#[test]
fn bubble_native_module_exists_and_names_the_window_and_page() {
    // A24: the bubble window is its own label, page and instance-tagged URL.
    let text = src("src/bubble_native.rs");
    assert!(
        text.contains("BUBBLES_WINDOW_LABEL"),
        "the window label constant is used"
    );
    assert!(
        text.contains("src/bubbles/index.html"),
        "the bubble page entry"
    );
    assert!(
        text.contains("instance="),
        "the URL carries the window instance"
    );
}

#[test]
fn bubble_window_builder_sets_the_click_through_flags() {
    // A24: transparent, undecorated, no shadow, never focusable, topmost, click-through.
    let text = src("src/bubble_native.rs");
    for flag in [
        "transparent(true)",
        "decorations(false)",
        "shadow(false)",
        "focusable(false)",
        "always_on_top(true)",
        "skip_taskbar(true)",
        "set_ignore_cursor_events(true)",
        "visible_on_all_workspaces",
    ] {
        assert!(text.contains(flag), "the bubble window must set `{flag}`");
    }
}

#[test]
fn macos_bubble_window_uses_the_nonactivating_panel_path_like_the_orb() {
    // A24: never steals focus; the same non-activating panel construction as orb_native.rs.
    let text = src("src/bubble_native.rs");
    for needle in [
        "tauri_nspanel",
        "PanelBuilder",
        "nonactivating_panel()",
        "can_join_all_spaces()",
        "full_screen_auxiliary()",
        "target_os = \"macos\"",
    ] {
        assert!(
            text.contains(needle),
            "macOS bubble panel must use `{needle}`"
        );
    }
}

#[test]
fn bubble_native_never_changes_policy_does_io_or_prints() {
    // Activation policy belongs to activation_policy.rs; bubbles never touch files or stdout.
    let text = src("src/bubble_native.rs");
    for banned in [
        "set_activation_policy",
        "std::fs",
        "println!",
        "eprintln!",
        "print!(",
    ] {
        assert!(
            !text.contains(banned),
            "bubble_native.rs must not contain `{banned}`"
        );
    }
}

#[test]
fn bubble_logs_never_name_the_message_text() {
    // Content-free logs: no log:: statement may format the transcript text.
    let text = src("src/bubble_native.rs");
    for (at, _) in text.match_indices("log::") {
        let rest = &text[at..];
        let statement = &rest[..rest.find(';').unwrap_or(rest.len())];
        for banned in [
            "{text}",
            "entry.text",
            "{entry.text}",
            "item.text",
            "{item.text}",
        ] {
            assert!(
                !statement.contains(banned),
                "a log statement must not include message text (`{banned}`): {statement}"
            );
        }
    }
}

#[test]
fn bubble_hide_path_orders_out_and_only_an_explicit_destroy_fn_closes() {
    // A7-2: hide the window (never destroy it per show); destruction only in destroy_bubbles.
    let text = src("src/bubble_native.rs");
    assert!(text.contains(".hide("), "the hide path hides the window");
    let destroy = fn_body(&text, "fn destroy_bubbles(");
    assert!(
        destroy.contains(".close(") || destroy.contains(".destroy("),
        "destroy_bubbles is the place that destroys the window"
    );
    let rest = text.replacen(destroy, "", 1);
    assert!(
        !rest.contains(".destroy(") && !rest.contains(".close("),
        "no other function may destroy or close the bubble window"
    );
}

// ---- (b) capability: event listening only, no emit, no other permission -------------------------

#[test]
fn bubbles_capability_grants_only_event_listen_for_the_bubble_window() {
    // A27 / A7-2: the page can listen; it cannot emit, invoke other commands or touch windows.
    let text = src("capabilities/bubbles.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("bubbles.json is valid JSON");
    assert_eq!(json["windows"], serde_json::json!(["orb-bubbles"]));
    assert_eq!(
        json["permissions"],
        serde_json::json!(["core:event:allow-listen"]),
        "exactly one permission: event listen"
    );
}

#[test]
fn other_capabilities_never_mention_the_bubble_window() {
    // The bubble window is granted only by its own capability file.
    for file in [
        "capabilities/default.json",
        "capabilities/chat.json",
        "capabilities/orb.json",
        "capabilities/desktop.json",
    ] {
        assert!(
            !src(file).contains("orb-bubbles"),
            "{file} must not mention orb-bubbles"
        );
    }
}

// ---- (c) the readiness command and the command matrix -------------------------------------------

#[test]
fn bubbles_ready_is_registered_in_the_command_list() {
    // A7-2: the handshake command is registered with the invoke handler.
    let lib = src("src/lib.rs");
    let start = lib
        .find("collect_commands![")
        .expect("lib.rs builds its command list with collect_commands!");
    assert!(
        lib[start..].contains("bubbles_ready"),
        "bubbles_ready must be registered in collect_commands!"
    );
}

#[test]
fn bubbles_ready_checks_the_caller_and_the_instance_then_calls_page_ready() {
    // A7-2 handshake: only the bubble window, only its current instance, then the core.
    let text = src("src/commands/bubbles.rs");
    let body = fn_body(&text, "async fn bubbles_ready(");
    assert!(body.contains("BUBBLES_WINDOW_LABEL"), "caller label check");
    assert!(body.contains("instance"), "instance is part of the request");
    assert!(body.contains("=="), "instance equality check");
    assert!(body.contains("page_ready("), "calls the shell's page_ready");
}

#[test]
fn window_matrix_allows_bubbles_ready_only_for_the_bubble_window() {
    // A27: re-assert through the single choke point.
    assert!(window_command_allowed(
        BUBBLES_WINDOW_LABEL,
        "bubbles_ready"
    ));
    for label in ["main", "chat", "orb", "recording_overlay", "settings", ""] {
        assert!(
            !window_command_allowed(label, "bubbles_ready"),
            "{label:?} must not call bubbles_ready"
        );
    }
    assert!(
        src("src/lib.rs").contains("window_command_allowed("),
        "lib.rs keeps the window_command_allowed choke point"
    );
}

// ---- (d) wiring of the shell into the converse runtime ------------------------------------------

#[test]
fn session_start_reaches_the_surface_dispatcher_which_decides_on_session_surface() {
    // A20 / R6-2: on_session_start -> chat_surface::open_on_session_start -> session_surface.
    let conv = src("src/managers/converse_platform.rs");
    assert!(
        fn_body(&conv, "fn on_session_start(").contains("chat_surface::open_on_session_start("),
        "the session start hook goes through the surface dispatcher"
    );
    let surface = src("src/chat_surface.rs");
    assert!(
        fn_body(&surface, "pub fn open_on_session_start(").contains("session_surface("),
        "open_on_session_start dispatches on session_surface("
    );
}

#[test]
fn voice_and_typed_terminal_paths_both_end_the_session_in_the_shell() {
    // A7-1 / A9-2: voice (on_state idle) and typed (on_state_text terminal) both call session_ended.
    let conv = src("src/managers/converse_platform.rs");
    for signature in ["fn on_state(", "fn on_state_text("] {
        assert!(
            fn_body(&conv, signature).contains("session_ended("),
            "{signature} must call the shell's session_ended"
        );
    }
}

#[test]
fn chat_log_fan_out_feeds_the_shell_for_entries() {
    // A8-1: the ChatLog emitter hands every Entry to the shell (no emit to the bubble window).
    let text = src("src/commands/chat.rs");
    let body = fn_body(&text, "fn emit(&self, event: &ChatEvent)");
    assert!(
        body.contains("chat_entry("),
        "the emitter calls the shell's chat_entry"
    );
    assert!(
        body.contains("ChatEventKind::Entry") || text.contains("ChatEventKind::Entry"),
        "only Entry events are fed to the bubbles"
    );
}

#[test]
fn recording_setting_updates_the_chat_log_before_the_shell() {
    // Rev 10 contract 1 / A9-1: the ChatLog gate changes first, then the shell is told.
    let text = src("src/commands/chat.rs");
    let body = fn_body(&text, "pub fn change_converse_chat_log_setting(");
    let log_at = body
        .find("set_recording(")
        .expect("the ChatLog set_recording call");
    let shell_at = body
        .find("recording_changed(")
        .expect("the shell's recording_changed call");
    assert!(
        log_at < shell_at,
        "ChatLog::set_recording must come before recording_changed"
    );
}

#[test]
fn orb_tray_hide_and_show_report_to_the_shell() {
    // A9-2: tray Hide sets user_hidden; tray Show clears it through user_showed_orb.
    let orb = src("src/orb_native.rs");
    assert!(
        fn_body(&orb, "pub fn dismiss_orb(").contains("user_hid_orb("),
        "the menu Hide reports user_hid_orb through dismiss_orb"
    );
    let toggle = fn_body(&orb, "pub fn toggle_visibility(");
    assert!(
        toggle.contains("user_showed_orb("),
        "the tray show reports user_showed_orb"
    );
    assert!(
        toggle.contains("user_hid_orb("),
        "the tray hide reports user_hid_orb"
    );
}

#[test]
fn classic_window_show_and_hide_report_their_visibility() {
    // A8-2: the classic window's show and hide both update the projection.
    let text = src("src/chat_window.rs");
    assert!(
        text.matches("classic_visibility_changed(").count() >= 2,
        "chat_window show and hide both call classic_visibility_changed"
    );
}

#[test]
fn bubble_window_is_destroyed_on_exit_and_on_presentation_switch() {
    // A7-2: destroyed on app exit and when the presentation switches to Classic.
    let lib = src("src/lib.rs");
    assert!(
        lib.contains("destroy_bubbles("),
        "app exit destroys the bubble window"
    );
    let orb_cmd = src("src/commands/orb.rs");
    let orb_native = src("src/orb_native.rs");
    assert!(
        orb_cmd.contains("destroy_bubbles(") || orb_native.contains("destroy_bubbles("),
        "a presentation switch destroys the bubble window"
    );
}

// ---- (e) the ready timeout runs on one stoppable timer ------------------------------------------

#[test]
fn ready_timeout_is_checked_by_a_timer_that_stops_when_idle() {
    // A7-4 / Rev 10 contract 5: READY_TIMEOUT_MS is driven by a tick; the timer thread has a stop
    // flag, exits when the core is Idle, and no loop runs without a break that references it.
    let text = src("src/bubble_native.rs");
    assert!(
        text.contains("READY_TIMEOUT_MS"),
        "the timeout constant drives the tick"
    );
    assert!(
        text.contains("stop"),
        "the timer thread carries a stop flag"
    );
    assert!(
        text.to_ascii_lowercase().contains("idle"),
        "the timer exits when the core is Idle"
    );
    for block in loop_blocks(&text) {
        assert!(
            block.contains("break") && block.contains("stop"),
            "every loop must break on the stop flag: {block}"
        );
    }
}
