//! Spec tests for the conversation window: toggle decision, command allow-list (A8), capability
//! files (A3), default bindings and settings (A5), tray entry, window construction (A3/A5) and
//! the shortcut registration path. Tests are written first: the implementation step makes them
//! green. Workers must not edit this file.
//!
//! Most checks are source contracts (`include_str!` or runtime reads of repo files) so this file
//! compiles while other workers edit `lib.rs`, `settings.rs` or the tray.

use super::*;
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::path::Path;

/// Commands added by DESIGN-CHAT. Asserted by name, independent of how far `lib.rs` has got.
const NEW_CHAT_COMMANDS: [&str; 5] = [
    "get_chat_snapshot",
    "clear_chat_log",
    "toggle_chat_window_command",
    "change_converse_chat_log_setting",
    // TYPING SPEC (DESIGN-TYPE Rev 1): the one command added on the user's request to type.
    "send_chat_message",
];

/// The only commands the chat webview may call (A8). TYPING SPEC (DESIGN-TYPE Rev 1): the
/// v1 pair plus `send_chat_message`; nothing else, and the new one is refused elsewhere.
const CHAT_ALLOWED: [&str; 3] = ["get_chat_snapshot", "clear_chat_log", "send_chat_message"];

/// Locales that must carry the chat keys (the repo's 26 locale directories).
const LOCALE_COUNT: usize = 26;

/// Reads a file under the crate root (`src-tauri/`) at runtime.
fn read_crate_file(rel: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// Reads and parses a JSON file under the crate root.
fn read_crate_json(rel: &str) -> Value {
    let text = read_crate_file(rel);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{rel} is not valid JSON: {e}"))
}

/// Source text with `//` comment lines removed (code only).
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every command name in the `collect_commands![ ... ]` list of `lib.rs`, last path segment only.
fn registered_commands() -> Vec<String> {
    let src = include_str!("lib.rs");
    let marker = "collect_commands![";
    assert_eq!(
        src.matches(marker).count(),
        1,
        "lib.rs must contain exactly one collect_commands! list"
    );
    let start = src.find(marker).expect("marker present") + marker.len();
    let mut depth = 1usize;
    let mut end = None;
    for (offset, ch) in src[start..].char_indices() {
        match ch {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth == 0 {
                    end = Some(start + offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let body = &src[start..end.expect("collect_commands! list is closed")];
    body.lines()
        .map(|line| line.split("//").next().unwrap_or(""))
        .flat_map(|line| line.split(','))
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(|item| item.rsplit("::").next().unwrap_or(item).trim().to_string())
        .collect()
}

/// Every command the table tests must cover: the registered list plus the four new names.
fn all_commands() -> BTreeSet<String> {
    let mut set: BTreeSet<String> = registered_commands().into_iter().collect();
    set.extend(NEW_CHAT_COMMANDS.iter().map(|c| c.to_string()));
    set
}

/// Formats a list of failures for one assertion, so one run reports every offender.
fn assert_no_failures(what: &str, failures: Vec<String>) {
    assert!(
        failures.is_empty(),
        "{what}: {} failure(s):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

// ---------------------------------------------------------------------------------------------
// 1. Toggle decision truth table
// ---------------------------------------------------------------------------------------------

#[test]
fn toggle_hidden_always_shows_whatever_focus_says() {
    assert_eq!(toggle_decision(false, false), ToggleAction::Show);
    assert_eq!(toggle_decision(false, true), ToggleAction::Show);
}

#[test]
fn toggle_visible_but_unfocused_brings_to_front() {
    assert_eq!(toggle_decision(true, false), ToggleAction::Show);
}

#[test]
fn toggle_visible_and_focused_hides() {
    assert_eq!(toggle_decision(true, true), ToggleAction::Hide);
}

// ---------------------------------------------------------------------------------------------
// 2. Command allow-list (A8), every command in lib.rs plus the four new names
// ---------------------------------------------------------------------------------------------

#[test]
fn chat_label_is_allowed_exactly_three_commands() {
    let mut failures = Vec::new();
    for command in all_commands() {
        let expected = CHAT_ALLOWED.contains(&command.as_str());
        if chat_command_allowed("chat", &command) != expected {
            failures.push(format!(
                "label=chat command={command} expected allowed={expected}"
            ));
        }
    }
    assert_no_failures("chat label allow-list", failures);
}

#[test]
fn chat_label_allows_get_chat_snapshot_and_clear_chat_log_by_name() {
    assert!(chat_command_allowed("chat", "get_chat_snapshot"));
    assert!(chat_command_allowed("chat", "clear_chat_log"));
}

#[test]
fn chat_snapshot_and_clear_are_denied_from_main_overlay_and_unknown_labels() {
    let mut failures = Vec::new();
    for label in ["main", "recording_overlay", "no_such_window"] {
        for command in ["get_chat_snapshot", "clear_chat_log"] {
            if chat_command_allowed(label, command) {
                failures.push(format!("label={label} command={command} must be denied"));
            }
        }
    }
    assert_no_failures("chat-only commands outside the chat window", failures);
}

#[test]
fn chat_label_denies_toggle_and_converse_chat_log_setter() {
    assert!(!chat_command_allowed("chat", "toggle_chat_window_command"));
    assert!(!chat_command_allowed(
        "chat",
        "change_converse_chat_log_setting"
    ));
}

#[test]
fn main_and_overlay_keep_every_other_command_allowed() {
    let new: HashSet<&str> = NEW_CHAT_COMMANDS.iter().copied().collect();
    let mut failures = Vec::new();
    for command in all_commands() {
        if new.contains(command.as_str()) {
            continue;
        }
        for label in ["main", "recording_overlay"] {
            if !chat_command_allowed(label, &command) {
                failures.push(format!("label={label} command={command} must stay allowed"));
            }
        }
    }
    assert_no_failures("existing commands from main and overlay", failures);
}

#[test]
fn main_may_call_toggle_and_converse_chat_log_setter() {
    assert!(chat_command_allowed("main", "toggle_chat_window_command"));
    assert!(chat_command_allowed(
        "main",
        "change_converse_chat_log_setting"
    ));
}

#[test]
fn lib_command_list_registers_all_chat_commands() {
    let registered: HashSet<String> = registered_commands().into_iter().collect();
    let missing: Vec<&str> = NEW_CHAT_COMMANDS
        .iter()
        .copied()
        .filter(|c| !registered.contains(*c))
        .collect();
    assert!(
        missing.is_empty(),
        "lib.rs collect_commands! is missing: {missing:?}"
    );
}

#[test]
fn lib_installs_the_single_command_guard_wrapper() {
    let src = code_only(include_str!("lib.rs"));
    assert!(
        src.contains("window_command_allowed("),
        "lib.rs must call window_command_allowed in the invoke handler wrapper"
    );
    assert!(
        src.contains("command not allowed from this window"),
        "the rejection message from A8 must be used"
    );
}

// ---------------------------------------------------------------------------------------------
// 3. Capability files (A3)
// ---------------------------------------------------------------------------------------------

/// The permission identifiers of a capability file (string entries or `{ "identifier": .. }`).
fn permission_ids(capability: &Value) -> Vec<String> {
    capability["permissions"]
        .as_array()
        .expect("permissions is an array")
        .iter()
        .map(|entry| match entry {
            Value::String(s) => s.clone(),
            Value::Object(obj) => obj["identifier"]
                .as_str()
                .expect("permission object has an identifier")
                .to_string(),
            other => panic!("unexpected permission entry: {other}"),
        })
        .collect()
}

#[test]
fn chat_capability_file_exists() {
    assert!(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("capabilities/chat.json")
            .is_file(),
        "src-tauri/capabilities/chat.json must exist (A3)"
    );
}

#[test]
fn chat_capability_is_scoped_to_chat_window_with_exact_event_permissions() {
    let chat = read_crate_json("capabilities/chat.json");
    let windows: Vec<String> = chat["windows"]
        .as_array()
        .expect("windows is an array")
        .iter()
        .map(|w| w.as_str().expect("window is a string").to_string())
        .collect();
    assert_eq!(windows, vec!["chat".to_string()]);

    let actual: BTreeSet<String> = permission_ids(&chat).into_iter().collect();
    let expected: BTreeSet<String> = ["core:event:allow-listen", "core:event:allow-unlisten"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        actual, expected,
        "chat.json permissions must be exactly the two event permissions"
    );
}

#[test]
fn chat_capability_grants_no_plugin_or_filesystem_permission() {
    let forbidden = [
        "fs:",
        "opener:",
        "store:",
        "shell:",
        "updater:",
        "global-shortcut:",
        "autostart:",
        "dialog:",
        "http:",
        "process:",
        "os:",
    ];
    let chat = read_crate_json("capabilities/chat.json");
    let mut failures = Vec::new();
    for permission in permission_ids(&chat) {
        if forbidden.iter().any(|p| permission.starts_with(p)) {
            failures.push(permission);
        }
    }
    assert_no_failures("forbidden permission in chat.json", failures);
}

#[test]
fn default_and_desktop_capabilities_do_not_list_chat() {
    for rel in ["capabilities/default.json", "capabilities/desktop.json"] {
        let cap = read_crate_json(rel);
        let windows = cap["windows"].as_array().cloned().unwrap_or_default();
        assert!(
            !windows.iter().any(|w| w.as_str() == Some("chat")),
            "{rel} must not grant the chat window (A3: chat has its own capability file)"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 4. Default bindings, ACTION_MAP, settings deserialization, shortcut registration path
// ---------------------------------------------------------------------------------------------

#[test]
fn chat_window_binding_has_the_per_os_default() {
    let expected = if cfg!(target_os = "macos") {
        "control+option+t"
    } else {
        "ctrl+alt+shift+t"
    };
    let settings = crate::settings::get_default_settings();
    let binding = settings
        .bindings
        .get("chat_window")
        .expect("default bindings contain chat_window");
    assert_eq!(binding.default_binding, expected);
    assert_eq!(binding.current_binding, expected);
}

#[test]
fn all_default_binding_strings_are_unique() {
    let settings = crate::settings::get_default_settings();
    let mut seen = HashSet::new();
    let mut duplicates = Vec::new();
    for (id, binding) in &settings.bindings {
        if binding.default_binding.is_empty() {
            continue;
        }
        if !seen.insert(binding.default_binding.clone()) {
            duplicates.push(format!("{id}={}", binding.default_binding));
        }
    }
    assert!(
        duplicates.is_empty(),
        "duplicate default bindings: {duplicates:?}"
    );
}

#[test]
fn action_map_registers_chat_window() {
    assert!(
        crate::actions::ACTION_MAP.contains_key("chat_window"),
        "ACTION_MAP must contain the chat_window action (ChatWindowAction)"
    );
}

// UI-REFRESH SPEC (DESIGN-UI Rev 2): the v1 decision "no open-on-start" is reversed on the
// user's request; the default is now ON. This test replaces the v1 assertion that the key was absent.
#[test]
fn default_settings_json_has_chat_log_on_and_open_on_start_on() {
    let value = serde_json::to_value(crate::settings::get_default_settings())
        .expect("default settings serialize");
    assert_eq!(
        value["converse_chat_log"],
        Value::Bool(true),
        "converse_chat_log defaults to true (A5)"
    );
    assert_eq!(
        value["converse_chat_open_on_start"],
        Value::Bool(true),
        "converse_chat_open_on_start defaults to true (DESIGN-UI)"
    );
}

#[test]
fn stored_settings_without_new_keys_deserialize_with_defaults() {
    let mut stored = serde_json::to_value(crate::settings::get_default_settings())
        .expect("default settings serialize");
    let obj = stored.as_object_mut().expect("settings is an object");
    obj.remove("converse_chat_log");
    obj.remove("converse_chat_open_on_start");
    obj.get_mut("bindings")
        .and_then(Value::as_object_mut)
        .expect("bindings is an object")
        .remove("chat_window");

    let loaded: crate::settings::AppSettings = serde_json::from_value(stored)
        .expect("a store written before this feature must still deserialize");
    let back = serde_json::to_value(&loaded).expect("loaded settings serialize");
    assert_eq!(back["converse_chat_log"], Value::Bool(true));
    assert_eq!(
        back["converse_chat_open_on_start"],
        Value::Bool(true),
        "a store without the key gets open-on-start on"
    );
    if let Some(binding) = loaded.bindings.get("chat_window") {
        assert_eq!(binding.current_binding, binding.default_binding);
    }
}

#[test]
fn shortcut_registration_has_no_hard_coded_exclusion_for_chat_window() {
    let src = code_only(&read_crate_file("src/shortcut/mod.rs"));
    assert!(
        src.contains("for (id, default_binding) in &default_bindings"),
        "registration must iterate the default bindings map"
    );
    // Every bare `<...id> == "x"` or `!= "x"` comparison in shortcut/mod.rs (field accesses such
    // as `provider.id` are not binding ids and are skipped). The only binding ids that may be
    // special-cased are the two existing exclusions.
    let re = Regex::new(r#"(?:^|[^.\w])(\w*id)\s*(?:==|!=)\s*"([a-z_]+)""#).expect("valid regex");
    let special: BTreeSet<String> = re.captures_iter(&src).map(|c| c[2].to_string()).collect();
    let allowed: BTreeSet<String> = ["cancel", "transcribe_with_post_process"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(
        special, allowed,
        "shortcut/mod.rs must not special-case binding ids beyond the existing two"
    );
    assert!(!src.contains("\"chat_window\""));
}

// ---------------------------------------------------------------------------------------------
// 5. Tray: menu item, menu-event arm, localized label
// ---------------------------------------------------------------------------------------------

#[test]
fn tray_menu_has_a_chat_item_built_with_with_id() {
    let tray = read_crate_file("src/tray.rs");
    let re = Regex::new(r#"MenuItem::with_id\(\s*app\s*,\s*"chat"\s*,"#).expect("valid regex");
    assert!(
        re.is_match(&tray),
        "tray.rs must build MenuItem::with_id(app, \"chat\", ..)"
    );
    assert!(
        tray.contains("strings.chat"),
        "the chat label must come from the localized TrayStrings (same as converse)"
    );
}

#[test]
fn menu_event_chat_arm_calls_the_chat_window() {
    let lib = read_crate_file("src/lib.rs");
    let arm = lib
        .find("\"chat\" => {")
        .expect("lib.rs on_menu_event must have a \"chat\" arm");
    let window = &lib[arm..(arm + 300).min(lib.len())];
    assert!(
        window.contains("chat_surface::toggle"),
        "the \"chat\" arm must call into the dispatcher (toggle)"
    );
}

#[test]
fn every_locale_has_a_non_empty_tray_chat_label() {
    let locales_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/i18n/locales");
    let mut locales = Vec::new();
    for entry in std::fs::read_dir(&locales_dir).expect("locales dir readable") {
        let entry = entry.expect("dir entry");
        if entry.path().is_dir() {
            locales.push(entry.path().join("translation.json"));
        }
    }
    assert_eq!(
        locales.len(),
        LOCALE_COUNT,
        "expected {LOCALE_COUNT} locale directories"
    );
    let mut failures = Vec::new();
    for path in locales {
        let text = std::fs::read_to_string(&path).expect("locale readable");
        let json: Value = serde_json::from_str(&text).expect("locale is JSON");
        let label = json["tray"]["chat"].as_str().unwrap_or("");
        if label.trim().is_empty() {
            failures.push(path.display().to_string());
        }
    }
    assert_no_failures("tray.chat missing or empty", failures);
}

// ---------------------------------------------------------------------------------------------
// 6. Window construction (A3, A5, A8): source contract on non-comment lines
// ---------------------------------------------------------------------------------------------

fn window_code() -> String {
    code_only(include_str!("chat_window.rs"))
}

#[test]
fn window_label_constant_is_chat() {
    assert_eq!(CHAT_WINDOW_LABEL, "chat");
    assert!(window_code().contains("pub const CHAT_WINDOW_LABEL: &str = \"chat\";"));
}

#[test]
fn window_is_built_from_the_bundled_entry_page_hidden_and_not_always_on_top() {
    let code = window_code();
    assert!(
        code.contains("WebviewWindowBuilder"),
        "must build with WebviewWindowBuilder"
    );
    assert!(
        code.contains("WebviewUrl::App(") && code.contains("CHAT_ENTRY_URL"),
        "must load WebviewUrl::App(CHAT_ENTRY_URL ...)"
    );
    assert!(
        code.contains(".visible(false)"),
        "window must be created hidden"
    );
    assert!(
        !code.contains("always_on_top"),
        "window must not be always-on-top (A5)"
    );
}

#[test]
fn window_close_hides_instead_of_destroying() {
    let code = window_code();
    assert!(
        code.contains("prevent_close"),
        "close request must call prevent_close"
    );
    assert!(
        code.contains(".hide()"),
        "close request must hide the window"
    );
}

#[test]
fn window_denies_navigation_and_new_windows() {
    let code = window_code();
    assert!(
        code.contains(".on_navigation("),
        "must install an on_navigation closure"
    );
    assert!(
        code.contains(".on_new_window("),
        "must deny new windows via on_new_window"
    );
}

#[test]
fn window_show_path_calls_show_and_set_focus() {
    let code = window_code();
    assert!(code.contains(".show()"), "show path must call show()");
    assert!(
        code.contains(".set_focus()"),
        "show path must call set_focus()"
    );
}

#[test]
fn window_code_has_no_filesystem_or_console_output() {
    // DESIGN-A Rev 3: the window module may now log window operations and failures through the
    // `log` crate (a failed show was previously invisible). It still never touches the file
    // system or prints to the console, and `chat_window_logging_never_prints_conversation_content`
    // keeps conversation content out of every log line.
    let re =
        Regex::new(r"\bstd::fs\b|\bfs::|\bFile::|\b(?:e)?println!|\bdbg!").expect("valid regex");
    for (file, code) in [
        ("chat_window.rs", window_code()),
        ("chat_chrome.rs", chrome_code()),
    ] {
        let hits: Vec<&str> = code.lines().filter(|l| re.is_match(l)).collect();
        assert!(
            hits.is_empty(),
            "{file} must not touch fs or print to the console: {hits:?}"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// 7. Toggle wiring from the tray and hotkey already points at the window module
// ---------------------------------------------------------------------------------------------

#[test]
fn chat_window_module_is_wired_in_lib_rs() {
    let lib = read_crate_file("src/lib.rs");
    assert!(
        lib.contains("mod chat_window;"),
        "lib.rs must declare mod chat_window"
    );
}

// ---------------------------------------------------------------------------------------------
// UI-REFRESH SPEC (DESIGN-UI Rev 2, written by ram-ai; workers must not edit): open on start
// ---------------------------------------------------------------------------------------------

#[test]
fn open_plan_truth_table() {
    // (exists, visible, minimized) -> plan
    let cases = [
        (false, false, false, OpenPlan::Build),
        (false, true, false, OpenPlan::Build),
        (false, true, true, OpenPlan::Build),
        (true, false, false, OpenPlan::Show),
        (true, false, true, OpenPlan::Show),
        // a minimized window can still report visible: it must be shown (unminimized)
        (true, true, true, OpenPlan::Show),
        (true, true, false, OpenPlan::Nothing),
    ];
    for (exists, visible, minimized, expected) in cases {
        assert_eq!(
            open_plan(exists, visible, minimized),
            expected,
            "exists={exists} visible={visible} minimized={minimized}"
        );
    }
}

#[test]
fn should_open_on_start_needs_both_settings() {
    let mut s = crate::settings::get_default_settings();
    for (log, open, expected) in [
        (true, true, true),
        (true, false, false),
        (false, true, false),
        (false, false, false),
    ] {
        s.converse_chat_log = log;
        s.converse_chat_open_on_start = open;
        assert_eq!(
            crate::settings::should_open_chat_on_start(&s),
            expected,
            "log={log} open={open}"
        );
    }
}

#[test]
fn queue_open_on_start_returns_before_anything_runs() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    let eligible_calls = Arc::new(AtomicUsize::new(0));
    let open_calls = Arc::new(AtomicUsize::new(0));
    let queued: Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>> = Arc::new(Mutex::new(Vec::new()));

    let (e, o, q) = (eligible_calls.clone(), open_calls.clone(), queued.clone());
    queue_open_on_start(
        move |job| q.lock().unwrap().push(job),
        move || {
            e.fetch_add(1, Ordering::SeqCst);
            true
        },
        move || {
            o.fetch_add(1, Ordering::SeqCst);
        },
    );
    // The hook returned; the dispatcher withheld the job: no settings read, no window work yet.
    assert_eq!(
        eligible_calls.load(Ordering::SeqCst),
        0,
        "no settings read in the hook"
    );
    assert_eq!(
        open_calls.load(Ordering::SeqCst),
        0,
        "no window work in the hook"
    );
    let job = queued.lock().unwrap().pop().expect("one job was queued");
    job();
    assert_eq!(eligible_calls.load(Ordering::SeqCst), 1);
    assert_eq!(open_calls.load(Ordering::SeqCst), 1);
}

#[test]
fn queue_open_on_start_does_not_open_when_not_eligible() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    let open_calls = Arc::new(AtomicUsize::new(0));
    let queued: Arc<Mutex<Vec<Box<dyn FnOnce() + Send>>>> = Arc::new(Mutex::new(Vec::new()));
    let (o, q) = (open_calls.clone(), queued.clone());
    queue_open_on_start(
        move |job| q.lock().unwrap().push(job),
        || false,
        move || {
            o.fetch_add(1, Ordering::SeqCst);
        },
    );
    queued.lock().unwrap().pop().expect("job queued")();
    assert_eq!(open_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn open_on_start_command_is_main_only_and_registered() {
    let cmd = "change_converse_chat_open_on_start_setting";
    assert!(
        !chat_command_allowed("chat", cmd),
        "chat webview must not call it"
    );
    assert!(chat_command_allowed("main", cmd));
    let lib = code_only(&read_crate_file("src/lib.rs"));
    assert!(
        lib.contains(&format!("commands::chat::{cmd}")),
        "{cmd} must be registered in lib.rs"
    );
    let chat_cmds = code_only(&read_crate_file("src/commands/chat.rs"));
    assert!(chat_cmds.contains(&format!("fn {cmd}")), "{cmd} defined");
}

#[test]
fn session_start_hook_only_enqueues_and_never_reads_settings_itself() {
    let src = code_only(&read_crate_file("src/managers/converse_platform.rs"));
    let start = src
        .find("fn on_session_start(&self)")
        .expect("on_session_start exists");
    let body: String = src[start..].chars().take(900).collect();
    let end = body.find("\n    }\n").unwrap_or(body.len());
    let body = &body[..end];
    assert!(
        body.contains("chat_surface::open_on_session_start"),
        "hook must call chat_surface::open_on_session_start"
    );
    assert!(
        !body.contains("get_settings"),
        "the hook must not read the settings store (C1); the queued closure does"
    );
}

#[test]
fn auto_open_never_focuses_and_window_is_dark_and_sized() {
    let src = code_only(&read_crate_file("src/chat_window.rs"));
    // Every function on the auto-open path: none may call set_focus (steals the foreground).
    for name in [
        "open_chat_window_on_session_start",
        "open_on_main_thread",
        "show_without_focus",
    ] {
        let start = src
            .find(&format!("fn {name}"))
            .unwrap_or_else(|| panic!("fn {name} must exist"));
        let tail = &src[start + 3..];
        let end = tail
            .find("\nfn ")
            .or_else(|| tail.find("\npub fn "))
            .unwrap_or(tail.len());
        assert!(
            !tail[..end].contains("set_focus"),
            "{name} must never call set_focus"
        );
    }
    assert!(src.contains("Theme::Dark"), "window uses the dark theme");
    assert!(
        src.contains("inner_size(480.0, 720.0)") && src.contains("min_inner_size(360.0, 420.0)"),
        "window size 480x720, min 360x420"
    );
}

// ---------------------------------------------------------------------------------------------
// TYPING SPEC (DESIGN-TYPE Rev 1, written by ram-ai; workers must not edit)
// ---------------------------------------------------------------------------------------------

#[test]
fn send_chat_message_is_chat_only_and_never_offered_to_other_windows() {
    assert!(chat_command_allowed("chat", "send_chat_message"));
    assert!(!chat_command_allowed("main", "send_chat_message"));
    assert!(!chat_command_allowed(
        "recording_overlay",
        "send_chat_message"
    ));
}

#[test]
fn the_send_command_validates_and_never_logs_the_text() {
    let src = code_only(&read_crate_file("src/commands/chat.rs"));
    let start = src
        .find("fn send_chat_message")
        .expect("send_chat_message command exists");
    let body: String = src[start..].chars().take(1500).collect();
    assert!(
        body.contains("send_typed"),
        "must go through ConverseController::send_typed"
    );
    for banned in ["log::", "println!", "eprintln!", "dbg!", "tracing::"] {
        assert!(
            !body.contains(banned),
            "must not log the typed text ({banned})"
        );
    }
    // returns a stable code, never the text
    assert!(
        body.contains(".code()"),
        "errors are returned as TypedError::code()"
    );
}

#[test]
fn typed_text_modules_never_log() {
    for rel in ["src/managers/converse_text.rs"] {
        let src = code_only(&read_crate_file(rel));
        for banned in ["log::", "println!", "eprintln!", "dbg!", "tracing::"] {
            assert!(!src.contains(banned), "{rel} must not use {banned}");
        }
    }
}

/// Body of `fn name` (brace matched), so tests do not depend on function order or length.
fn fn_body(src: &str, name: &str) -> String {
    let start = src
        .find(&format!("fn {name}"))
        .unwrap_or_else(|| panic!("fn {name} must exist"));
    let open = start + src[start..].find('{').expect("body brace");
    let mut depth = 0usize;
    for (i, ch) in src[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return src[open..open + i + 1].to_string();
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces in fn {name}");
}

#[test]
fn the_text_session_never_arms_the_microphone_or_speaks_in_the_real_platform() {
    let src = code_only(&read_crate_file("src/managers/converse_platform.rs"));
    let body = fn_body(&src, "parts_text");
    assert!(body.contains("PendingEars"), "text ears are PendingEars");
    assert!(body.contains("SilentMouth"), "text mouth is SilentMouth");
    assert!(
        body.contains("TextHands"),
        "text hands refuse paste and jobs"
    );
    for banned in ["RealEars", "RealMouth", "BargeMonitor", "SpeakerGate"] {
        assert!(!body.contains(banned), "parts_text must not build {banned}");
    }
}

#[test]
fn text_sessions_persist_nothing_and_run_no_job_announcer() {
    let src = code_only(&read_crate_file("src/managers/converse_platform.rs"));
    let body = fn_body(&src, "parts_text");
    assert!(
        body.contains("transcript: None"),
        "text sessions collect no transcript"
    );
    assert!(
        body.contains("on_complete: None"),
        "text sessions save nothing at the end"
    );
    assert!(
        body.contains("announcer: None"),
        "no job announcer (it would speak)"
    );
}

#[test]
fn text_hands_refuse_background_jobs_so_nothing_can_speak_later() {
    let src = code_only(&read_crate_file("src/managers/converse_text.rs"));
    let body = fn_body(&src, "start_job");
    assert!(
        body.contains("Unsupported"),
        "TextHands::start_job must be Unsupported"
    );
    let has = fn_body(&src, "has_active_job");
    assert!(
        has.contains("false"),
        "TextHands::has_active_job must be false"
    );
}

#[test]
fn text_mode_text_sessions_use_the_long_confirmation_window_and_a_high_turn_limit() {
    let src = code_only(&read_crate_file("src/managers/converse_platform.rs"));
    let body = fn_body(&src, "parts_text");
    assert!(
        body.contains("confirm_listen_timeout") && body.contains("from_secs(60)"),
        "text sessions confirm with a 60 s window"
    );
    assert!(body.contains("max_turns"), "text sessions raise max_turns");
}

#[test]
fn send_typed_binds_every_enqueue_to_the_session_generation() {
    let src = code_only(&read_crate_file("src/managers/converse_runtime.rs"));
    let body = fn_body(&src, "send_typed");
    assert!(
        body.contains("push_for"),
        "send_typed must enqueue with push_for(generation, ..)"
    );
    assert!(
        !body.contains(".push("),
        "send_typed must never use the unbound push"
    );
}

#[test]
fn the_cancel_button_only_stops_voice_sessions_not_a_text_chat() {
    let src = code_only(&read_crate_file("src/commands/mod.rs"));
    let body = fn_body(&src, "cancel_routed");
    assert!(
        body.contains("is_voice_active"),
        "cancel_routed must route to the conversation only for a voice session"
    );
}

#[test]
fn text_state_changes_never_reach_the_floating_overlay() {
    let src = code_only(&read_crate_file("src/managers/converse_platform.rs"));
    let body = fn_body(&src, "on_state_text");
    assert!(
        !body.contains("overlay_dispatcher"),
        "on_state_text must not drive the recording overlay"
    );
}

// ---------------------------------------------------------------------------------------------
// FRONT SPEC (DESIGN-A Rev 1, written by ram-ai; workers must not weaken or delete these)
// The window must come to the front when the user toggles it, even from tray-only (accessory)
// mode, and every show/build failure must leave a log line (no window content, no text).
// ---------------------------------------------------------------------------------------------

#[test]
fn toggling_the_window_promotes_the_app_before_showing_and_focusing() {
    let src = code_only(&read_crate_file("src/chat_window.rs"));
    let body = fn_body(&src, "show_existing");
    let policy = body
        .find("set_activation_policy")
        .expect("show_existing must set the macOS activation policy");
    let show = body
        .find(".show()")
        .expect("show_existing must show the window");
    let focus = body
        .find(".set_focus()")
        .expect("show_existing must focus the window");
    assert!(policy < show, "activation policy must be set before show()");
    assert!(show < focus, "show() must come before set_focus()");
}

#[test]
fn window_operations_are_never_silently_discarded() {
    let src = code_only(&read_crate_file("src/chat_window.rs"));
    for banned in [
        "let _ = window.show()",
        "let _ = window.set_focus()",
        "let _ = window.unminimize()",
        "let _ = window.hide()",
        "let _ = chat.hide()",
    ] {
        assert!(
            !src.contains(banned),
            "chat_window.rs must log failures instead of `{banned}`"
        );
    }
}

#[test]
fn toggling_and_opening_log_what_they_decided_and_what_failed() {
    let src = code_only(&read_crate_file("src/chat_window.rs"));
    let toggle = fn_body(&src, "toggle_on_main_thread");
    assert!(
        toggle.contains("info!") || toggle.contains("debug!"),
        "toggle_on_main_thread must log exists/visible/focused and the chosen action"
    );
    let open = fn_body(&src, "open_on_main_thread");
    assert!(
        open.contains("warn!") || open.contains("error!"),
        "open_on_main_thread must log a failed build"
    );
    let build = fn_body(&src, "build_chat_window");
    assert!(
        build.contains("warn!") || build.contains("error!") || build.contains("info!"),
        "build_chat_window must log its result"
    );
}

#[test]
fn a_failed_toggle_is_logged_not_dropped() {
    let src = code_only(&read_crate_file("src/chat_window.rs"));
    let body = fn_body(&src, "toggle_chat_window");
    assert!(
        !body.contains("let _ = toggle_on_main_thread"),
        "the toggle result must be logged"
    );
}

#[test]
fn chat_window_logging_never_prints_conversation_content() {
    let src = code_only(&read_crate_file("src/chat_window.rs"));
    // Check every complete `log::` call (the whole macro up to its `;`), not just its first line.
    let mut checked = 0;
    let mut rest = src.as_str();
    while let Some(i) = rest.find("log::") {
        let call_end = rest[i..].find(';').map(|e| i + e).unwrap_or(rest.len());
        let call = rest[i..call_end].to_lowercase();
        for banned in ["text", "message", "transcript", "entry", "snapshot"] {
            assert!(
                !call.contains(banned),
                "chat_window.rs log call must not mention `{banned}`: {call}"
            );
        }
        checked += 1;
        rest = &rest[call_end..];
    }
    assert!(
        checked >= 5,
        "expected the window logging to exist, found {checked} calls"
    );
}

#[test]
fn closing_the_window_goes_through_hide_existing() {
    let src = code_only(&read_crate_file("src/chat_window.rs"));
    let body = fn_body(&src, "build_chat_window");
    assert!(
        body.contains("hide_existing"),
        "the close-requested handler must hide through hide_existing so the Dock policy is restored"
    );
    assert!(
        !body.contains("chat.hide()"),
        "no direct hide in the close handler"
    );
}

#[test]
fn the_dock_policy_is_demoted_only_after_a_successful_hide() {
    let src = code_only(&read_crate_file("src/chat_window.rs"));
    let body = fn_body(&src, "hide_existing");
    let hide = body
        .find(".hide()")
        .expect("hide_existing hides the window");
    let policy = body
        .find("set_activation_policy")
        .expect("hide_existing restores the policy");
    assert!(hide < policy, "hide first, then the policy");
    assert!(
        body.contains("is_ok()") || body.contains("Ok(()) =>") || body.contains("if let Err"),
        "the policy change must depend on the hide result"
    );
}

// ---------------------------------------------------------------------------------------------
// TRACK T SPEC (DESIGN Revision 3, A-T3): see-through window. Builder flags are pinned; every
// other flag above (hidden, not always on top, 480x720, min 360x420, dark theme) is unchanged.
// ---------------------------------------------------------------------------------------------

#[test]
fn window_builder_is_transparent_and_keeps_the_other_flags() {
    let code = window_code();
    assert!(
        code.contains(".transparent(true)"),
        "the window is transparent (A-T3)"
    );
    assert!(code.contains(".decorations(true)"), "decorations stay on");
    assert!(code.contains(".resizable(true)"), "still resizable");
    assert!(code.contains(".visible(false)"), "still created hidden");
    assert!(
        code.contains(".focused(false)"),
        "still not focused on build"
    );
    assert!(
        code.contains(".theme(Some(tauri::Theme::Dark))"),
        "still the dark theme"
    );
    assert!(!code.contains("always_on_top"), "never always on top");
    assert!(code.contains("inner_size(480.0, 720.0)"));
    assert!(code.contains("min_inner_size(360.0, 420.0)"));
}

#[test]
fn macos_window_has_no_shadow_and_a_transparent_title_bar_behind_the_macos_cfg() {
    let code = window_code();
    let cfg = code
        .find("#[cfg(target_os = \"macos\")]")
        .expect("the macOS-only window settings are cfg gated");
    let rest = &code[cfg..];
    assert!(
        rest.contains(".shadow(false)"),
        "macOS builder sets shadow(false)"
    );
    assert!(
        rest.contains("TitleBarStyle::Transparent"),
        "macOS builder sets title_bar_style(TitleBarStyle::Transparent)"
    );
    assert_eq!(
        code.matches("TitleBarStyle::").count(),
        1,
        "TitleBarStyle is referenced once, inside the macOS block"
    );
}

#[test]
fn the_initial_opacity_travels_in_the_page_url_and_the_worker_starts_with_the_window() {
    let code = window_code();
    assert!(
        code.contains("WebviewUrl::App(") && code.contains("opacity=") && code.contains("rev="),
        "the page URL carries ?opacity=<pct>&rev=<n> (A-T3)"
    );
    let build = fn_body(&code, "build_chat_window");
    assert!(
        build.contains("start_replay_worker("),
        "the single replay worker starts at window creation (A-T4)"
    );
}

#[test]
fn the_snapshot_is_re_emitted_on_show_and_on_focus() {
    let code = window_code();
    for name in ["show_existing", "show_without_focus"] {
        let body = fn_body(&code, name);
        assert!(
            body.contains("emit_current_opacity("),
            "{name} re-emits the latest opacity snapshot"
        );
    }
    let build = fn_body(&code, "build_chat_window");
    assert!(
        build.contains("WindowEvent::Focused(true)") && build.contains("emit_current_opacity("),
        "gaining focus re-emits the snapshot"
    );
}

#[test]
fn the_capability_file_is_unchanged_listen_only() {
    let chat = read_crate_json("capabilities/chat.json");
    let actual: BTreeSet<String> = permission_ids(&chat).into_iter().collect();
    let expected: BTreeSet<String> = ["core:event:allow-listen", "core:event:allow-unlisten"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(actual, expected);
}

// ---------------------------------------------------------------------------------------------
// 9. Native chrome (orb-click D1, A-C5) and chat_hide_window (D1b)
// ---------------------------------------------------------------------------------------------

fn chrome_code() -> String {
    code_only(&read_crate_file("src/chat_chrome.rs"))
}

#[test]
fn window_keeps_native_decorations_and_hides_the_title_text_on_macos() {
    let code = window_code();
    assert!(code.contains(".decorations(true)"), "decorations stay on");
    let cfg = code
        .find("#[cfg(target_os = \"macos\")]")
        .expect("macOS block present");
    let rest = &code[cfg..];
    assert!(
        rest.contains(".hidden_title(true)"),
        "macOS builder hides the title text"
    );
    assert_eq!(
        code.matches("TitleBarStyle::Transparent").count(),
        1,
        "TitleBarStyle::Transparent stays exactly once"
    );
}

#[test]
fn chrome_helper_runs_on_creation_on_every_show_and_on_window_events() {
    let code = window_code();
    for name in ["build_chat_window", "show_existing", "show_without_focus"] {
        assert!(
            fn_body(&code, name).contains("hide_window_buttons("),
            "{name} applies the hidden window buttons"
        );
    }
    let build = fn_body(&code, "build_chat_window");
    for kind in [
        "WindowEvent::Focused(",
        "WindowEvent::Resized(",
        "WindowEvent::ScaleFactorChanged",
        "WindowEvent::ThemeChanged(",
    ] {
        assert!(build.contains(kind), "event handler reacts to {kind}");
    }
    assert!(
        build.contains("CloseRequested"),
        "the close handling stays in place"
    );
    assert!(
        build.contains("hide_existing(&close_app"),
        "CloseRequested still goes through hide_existing"
    );
}

#[test]
fn chrome_helper_hides_exactly_the_three_standard_buttons_and_changes_nothing_else() {
    let code = chrome_code();
    for button in [
        "NSWindowButton::CloseButton",
        "NSWindowButton::MiniaturizeButton",
        "NSWindowButton::ZoomButton",
    ] {
        assert!(code.contains(button), "{button} hidden");
    }
    assert!(code.contains("setHidden(true)"), "buttons are hidden");
    assert!(
        !code.contains("setHidden(false)"),
        "the helper never shows a button"
    );
    for forbidden in [
        "setStyleMask",
        "setTitlebarAppearsTransparent",
        "setTitleVisibility",
        "setLevel",
        "setFrame",
        "setCollectionBehavior",
        "setOpaque",
        "setBackgroundColor",
        "setHasShadow",
        "setHidesOnDeactivate",
        "unwrap()",
        "expect(",
        "panic!",
    ] {
        assert!(
            !code.contains(forbidden),
            "chat_chrome.rs must not use {forbidden}"
        );
    }
}

#[test]
fn chat_chrome_is_macos_only_and_prints_or_writes_nothing() {
    let lib = read_crate_file("src/lib.rs");
    assert!(
        lib.contains("#[cfg(target_os = \"macos\")]\nmod chat_chrome;"),
        "lib.rs declares chat_chrome only on macOS"
    );
    let re =
        Regex::new(r"\bstd::fs\b|\bfs::|\bFile::|\b(?:e)?println!|\bdbg!").expect("valid regex");
    let hits: Vec<String> = chrome_code()
        .lines()
        .filter(|l| re.is_match(l))
        .map(str::to_string)
        .collect();
    assert!(
        hits.is_empty(),
        "chat_chrome.rs must not touch fs or print: {hits:?}"
    );
}

#[test]
fn chat_hide_window_applies_only_to_the_chat_label() {
    assert!(hide_request_applies("chat"));
    for label in [
        "main",
        "orb",
        "orb-bubbles",
        "recording_overlay",
        "",
        "Chat",
    ] {
        assert!(!hide_request_applies(label), "{label} must be a no-op");
    }
}

#[test]
fn chat_hide_window_is_listed_once_in_chat_commands_and_registered() {
    use crate::window_commands::CHAT_COMMANDS;
    assert_eq!(CHAT_COMMANDS.len(), 11, "CHAT_COMMANDS grows 10 -> 11");
    assert_eq!(
        CHAT_COMMANDS
            .iter()
            .filter(|c| **c == "chat_hide_window")
            .count(),
        1,
        "chat_hide_window listed exactly once"
    );
    let registered = registered_commands();
    assert!(
        registered.iter().any(|c| c == "chat_hide_window"),
        "lib.rs registers chat_hide_window"
    );
}

#[test]
fn chat_hide_window_command_checks_the_label_and_hides_through_hide_existing() {
    let src = code_only(&read_crate_file("src/commands/chat.rs"));
    let body = fn_body(&src, "chat_hide_window");
    let check = body
        .find("hide_request_applies(")
        .expect("command checks the calling label");
    let hide = body
        .find("hide_existing(")
        .expect("command hides through hide_existing");
    assert!(check < hide, "the label check runs before any hide");
    assert!(body.contains("Ok(())"), "the command returns Ok");
}

#[test]
fn chat_hide_window_is_in_bindings_ts() {
    let bindings = read_crate_file("../src/bindings.ts");
    assert!(
        bindings.contains("TAURI_INVOKE(\"chat_hide_window\")"),
        "bindings.ts exports the chat_hide_window invoke"
    );
}
