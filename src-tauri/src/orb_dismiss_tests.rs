//! T3 spec: the orb dismissal (DESIGN Rev 4, B5 and B8). The Hide badge, Esc and the native menu's
//! Hide item all reach one helper, `orb_native::dismiss_orb`, through `orb_dismiss` (orb label
//! only). The bubble shell's clearing of bubbles is pinned in `bubble_shell_core_tests`
//! (`orb_hidden_by_the_user_clears_bubbles`). Authoritative: do not weaken an assertion.

use crate::orb_native::hide_orb_setting;
use crate::settings::AppSettings;
use crate::window_commands::{window_command_allowed, CHAT_LABEL, MAIN_LABEL, ORB_LABEL};

const BUBBLES_LABEL: &str = "orb-bubbles";
const RECORDING_OVERLAY_LABEL: &str = "recording_overlay";
const UNKNOWN_LABEL: &str = "some-unknown-window";

fn src(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(path))
        .unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Text of the function whose signature line contains `signature`, up to its closing brace at
/// column zero.
fn fn_body<'a>(text: &'a str, signature: &str) -> &'a str {
    let start = text
        .find(signature)
        .unwrap_or_else(|| panic!("`{signature}` not found"));
    let rest = &text[start..];
    let end = rest.find("\n}\n").map(|i| i + 3).unwrap_or(rest.len());
    &rest[..end]
}

#[test]
fn a_dismissal_persists_the_orb_hidden() {
    let mut settings = AppSettings::default();
    hide_orb_setting(&mut settings);
    assert_eq!(settings.orb_visible, Some(false));
}

#[test]
fn a_dismissal_hides_an_orb_that_was_shown() {
    let mut settings = AppSettings {
        orb_visible: Some(true),
        ..AppSettings::default()
    };
    hide_orb_setting(&mut settings);
    assert_eq!(settings.orb_visible, Some(false));
}

#[test]
fn a_dismissal_is_idempotent_when_the_orb_is_already_hidden() {
    let mut settings = AppSettings {
        orb_visible: Some(false),
        ..AppSettings::default()
    };
    hide_orb_setting(&mut settings);
    let once = settings.orb_visible;
    hide_orb_setting(&mut settings);
    assert_eq!(settings.orb_visible, Some(false));
    assert_eq!(settings.orb_visible, once);
}

#[test]
fn the_menu_hide_item_and_the_command_share_one_helper() {
    let orb = src("src/orb_native.rs");
    assert!(
        fn_body(&orb, "fn handle_menu(").contains("\"orb_hide\" => dismiss_orb(app)"),
        "the menu Hide item must call dismiss_orb"
    );
    let helper = fn_body(&orb, "pub fn dismiss_orb(");
    assert!(
        helper.contains("user_hid_orb(app)"),
        "dismiss reports the hide to the bubble shell"
    );
    assert!(
        helper.contains("write_settings_gated(app, hide_orb_setting)"),
        "dismiss persists orb_visible = false through the gated writer"
    );
    assert!(
        helper.contains("apply_visibility(app)"),
        "dismiss applies visibility"
    );

    let commands = src("src/commands/orb.rs");
    let command = fn_body(&commands, "pub async fn orb_dismiss(");
    assert!(
        command.contains("orb_native::dismiss_orb(&app)"),
        "orb_dismiss must call the same helper as the menu item"
    );
}

#[test]
fn the_command_is_async_and_registered() {
    let commands = src("src/commands/orb.rs");
    assert!(commands.contains("pub async fn orb_dismiss(app: AppHandle)"));
    let lib = src("src/lib.rs");
    assert!(
        lib.contains("commands::orb::orb_dismiss,"),
        "registered in lib.rs"
    );
}

#[test]
fn the_dismiss_gate_matrix_allows_only_the_orb_window() {
    let labels = [
        MAIN_LABEL,
        CHAT_LABEL,
        ORB_LABEL,
        BUBBLES_LABEL,
        RECORDING_OVERLAY_LABEL,
        UNKNOWN_LABEL,
    ];
    let commands = ["orb_dismiss", "orb_menu", "chat_stop_conversation"];
    for label in labels {
        for command in commands {
            let want = match command {
                "orb_dismiss" | "orb_menu" => label == ORB_LABEL,
                "chat_stop_conversation" => label == CHAT_LABEL,
                _ => unreachable!(),
            };
            assert_eq!(
                window_command_allowed(label, command),
                want,
                "{label:?} calling {command}"
            );
        }
    }
}

#[test]
fn the_capability_file_grants_no_app_command() {
    // App commands are gated by window_command_allowed, not by capabilities: orb.json stays
    // event-only (window_policy_tests pins the exact permissions).
    let text = src("capabilities/orb.json");
    assert!(!text.contains("orb_dismiss"));
    assert!(!text.contains("allow-invoke"));
}
