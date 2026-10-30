//! Appearance transport and commit rules (UI enterprise S1, DESIGN Revision 3, contracts 2-6).
//!
//! Pure rules and a fake-clock worker. The app-bound functions (commit, emit, window builds) are
//! covered by source contracts that pin their wiring. Nothing here logs colours or paths.

use crate::appearance::{
    commit_in_memory, encode_query_value, plan_deliveries, run_consumer_replay, should_apply,
    snapshot_from_settings, with_ap_param, AppearanceFields, AppearanceHolder, AppearanceSnapshot,
    APPEARANCE_EVENT, CONSUMER_LABELS,
};
use crate::chat_opacity::{replay_lifecycle, OpacityHolder, ReplayGate};
use crate::settings::{
    get_default_settings, Accent, ChatTheme, CustomThemeColors, Theme, VisualStyle,
};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::time::Duration;

const CHAT: &str = "chat";
const ORB: &str = "orb";
const BUBBLES: &str = "orb-bubbles";

fn all_exist(_: &str) -> bool {
    true
}

fn none_exist(_: &str) -> bool {
    false
}

fn only(labels: &'static [&'static str]) -> impl Fn(&str) -> bool {
    move |label| labels.iter().any(|known| *known == label)
}

// ---------------------------------------------------------------------------------------------
// Snapshot shape and build from persisted settings
// ---------------------------------------------------------------------------------------------

#[test]
fn event_name_and_consumer_labels_are_the_existing_windows() {
    assert_eq!(APPEARANCE_EVENT, "appearance");
    assert_eq!(CONSUMER_LABELS, [CHAT, ORB, BUBBLES]);
    assert_eq!(crate::chat_window::CHAT_WINDOW_LABEL, CHAT);
    assert_eq!(crate::orb_window::ORB_WINDOW_LABEL, ORB);
    assert_eq!(crate::window_commands::BUBBLES_WINDOW_LABEL, BUBBLES);
}

#[test]
fn snapshot_payload_has_exactly_the_snake_case_wire_keys() {
    let snap = snapshot_from_settings(4, &get_default_settings());
    let value = serde_json::to_value(&snap).expect("serialize");
    let keys: Vec<&str> = value
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    let mut sorted = keys.clone();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [
            "accent",
            "chat_theme",
            "custom_colors",
            "orb_accent",
            "rev",
            "theme",
            "visual_style"
        ]
    );
    assert_eq!(value["rev"], json!(4));
    assert_eq!(value["theme"], json!("system"));
    assert_eq!(value["accent"], json!("amber"));
    assert_eq!(value["chat_theme"], json!("dark"));
    assert_eq!(value["visual_style"], json!("vibrant"));
}

#[test]
fn custom_colours_travel_only_while_the_theme_is_custom() {
    let mut s = get_default_settings();
    assert_eq!(snapshot_from_settings(1, &s).custom_colors, None);
    s.theme = Theme::Custom;
    let snap = snapshot_from_settings(1, &s);
    assert_eq!(snap.custom_colors, Some(s.custom_theme_colors.clone()));
}

#[test]
fn delivered_snapshot_equals_the_persisted_settings() {
    let mut s = get_default_settings();
    s.theme = Theme::Light;
    s.accent = Accent::Violet;
    s.chat_theme = ChatTheme::FollowApp;
    s.visual_style = VisualStyle::Restrained;
    let snap = snapshot_from_settings(9, &s);
    assert_eq!(snap.rev, 9);
    assert_eq!(snap.theme, s.theme);
    assert_eq!(snap.accent, s.accent);
    assert_eq!(snap.chat_theme, s.chat_theme);
    assert_eq!(snap.visual_style, s.visual_style);
    assert_eq!(snap.custom_colors, None);
}

// ---------------------------------------------------------------------------------------------
// Monotonic rev, commit, seed
// ---------------------------------------------------------------------------------------------

#[test]
fn commit_bumps_the_rev_monotonically_and_publishes_the_persisted_values() {
    let holder = AppearanceHolder::new();
    let mut s = get_default_settings();
    let mut revs = Vec::new();
    for (i, accent) in [Accent::Blue, Accent::Rose, Accent::Slate]
        .into_iter()
        .enumerate()
    {
        let snap = commit_in_memory(&mut s, &holder, |f| f.accent = accent);
        assert_eq!(snap.accent, accent, "commit {i} publishes its value");
        assert_eq!(snap.accent, s.accent, "the snapshot equals the settings");
        revs.push(snap.rev);
    }
    assert_eq!(revs, [1, 2, 3]);
    assert_eq!(holder.revision(), 3);
    assert_eq!(holder.current_or_seed(get_default_settings).rev, 3);
}

#[test]
fn seeding_reads_once_then_commits_continue_the_rev() {
    let holder = AppearanceHolder::new();
    let reads = Cell::new(0);
    let seeded = holder.current_or_seed(|| {
        reads.set(reads.get() + 1);
        get_default_settings()
    });
    assert_eq!(seeded.rev, 1);
    let again = holder.current_or_seed(|| {
        reads.set(reads.get() + 1);
        get_default_settings()
    });
    assert_eq!(again, seeded, "a second read returns the same snapshot");
    assert_eq!(reads.get(), 1, "the store is read once");
    let mut s = get_default_settings();
    let next = commit_in_memory(&mut s, &holder, |f| {
        f.visual_style = VisualStyle::Restrained
    });
    assert_eq!(next.rev, 2);
}

#[test]
fn each_of_the_five_writers_is_exactly_one_commit_and_one_snapshot() {
    type Mutation = fn(&mut AppearanceFields);
    let writers: [Mutation; 5] = [
        |f| f.theme = Theme::Light,
        |f| f.custom_theme_colors.accent = "#abcdef".to_string(),
        |f| f.accent = Accent::Emerald,
        |f| f.chat_theme = ChatTheme::Light,
        |f| f.visual_style = VisualStyle::Restrained,
    ];
    let holder = AppearanceHolder::new();
    let mut s = get_default_settings();
    for (index, writer) in writers.into_iter().enumerate() {
        let before = holder.revision();
        let snap = commit_in_memory(&mut s, &holder, writer);
        assert_eq!(snap.rev, before + 1, "writer {index} bumps exactly once");
        assert_eq!(holder.revision(), before + 1);
    }
    assert_eq!(s.theme, Theme::Light);
    assert_eq!(s.accent, Accent::Emerald);
    assert_eq!(s.chat_theme, ChatTheme::Light);
    assert_eq!(s.visual_style, VisualStyle::Restrained);
    assert_eq!(s.custom_theme_colors.accent, "#abcdef");
}

#[test]
fn a_commit_changes_only_the_appearance_fields() {
    let holder = AppearanceHolder::new();
    let mut s = get_default_settings();
    s.selected_model = "keep-me".to_string();
    s.orb_size_px = Some(96);
    commit_in_memory(&mut s, &holder, |f| f.accent = Accent::Blue);
    assert_eq!(s.selected_model, "keep-me");
    assert_eq!(s.orb_size_px, Some(96));
    assert_eq!(s.chat_opacity_pct, get_default_settings().chat_opacity_pct);
}

#[test]
fn appearance_rev_and_opacity_rev_are_independent_counters() {
    let appearance = AppearanceHolder::new();
    let opacity = OpacityHolder::new();
    let mut s = get_default_settings();
    for _ in 0..3 {
        commit_in_memory(&mut s, &appearance, |f| f.accent = Accent::Rose);
    }
    assert_eq!(appearance.revision(), 3);
    assert_eq!(
        opacity.snapshot().rev,
        0,
        "opacity rev untouched by appearance"
    );
    opacity.commit(40);
    assert_eq!(
        appearance.revision(),
        3,
        "appearance rev untouched by opacity"
    );
}

// ---------------------------------------------------------------------------------------------
// Delivery plan: existing labels only, absent windows are success
// ---------------------------------------------------------------------------------------------

#[test]
fn deliveries_target_exactly_the_existing_consumer_labels() {
    let snap = snapshot_from_settings(2, &get_default_settings());
    let all = plan_deliveries(&snap, all_exist);
    let labels: Vec<&str> = all.iter().map(|(label, _)| *label).collect();
    assert_eq!(labels, [CHAT, ORB, BUBBLES]);
    assert!(all.iter().all(|(_, payload)| *payload == snap));

    static ONLY_ORB_AND_BUBBLES: [&str; 2] = [ORB, BUBBLES];
    let some = plan_deliveries(&snap, only(&ONLY_ORB_AND_BUBBLES));
    let labels: Vec<&str> = some.iter().map(|(label, _)| *label).collect();
    assert_eq!(labels, [ORB, BUBBLES], "an absent chat window is skipped");

    assert!(
        plan_deliveries(&snap, none_exist).is_empty(),
        "no window is success"
    );
}

#[test]
fn only_the_three_consumer_labels_ever_receive_the_event() {
    let snap = snapshot_from_settings(1, &get_default_settings());
    let seen = RefCell::new(Vec::new());
    for label in [
        "main",
        "overlay",
        "recording_overlay",
        "settings",
        "chat",
        "orb",
    ] {
        seen.borrow_mut().clear();
        for (target, _) in plan_deliveries(&snap, |l| l == label) {
            seen.borrow_mut().push(target);
        }
        if CONSUMER_LABELS.contains(&label) {
            assert_eq!(seen.borrow().as_slice(), [label], "{label} is a consumer");
        } else {
            assert!(seen.borrow().is_empty(), "{label} is not a consumer");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Consumer rule: apply only a strictly higher rev (reordered and duplicate deliveries)
// ---------------------------------------------------------------------------------------------

#[test]
fn the_consumer_applies_only_strictly_higher_revisions() {
    assert!(should_apply(0, 1));
    assert!(!should_apply(3, 3), "a duplicate is ignored");
    assert!(!should_apply(3, 2), "an older delivery is ignored");
    assert!(should_apply(3, 4));
}

#[test]
fn reordered_and_duplicate_deliveries_never_regress_the_consumer() {
    let holder = AppearanceHolder::new();
    let mut s = get_default_settings();
    let mut deliveries = Vec::new();
    for accent in [Accent::Blue, Accent::Rose, Accent::Violet, Accent::Slate] {
        deliveries.push(commit_in_memory(&mut s, &holder, |f| f.accent = accent));
    }
    // Arrival order revs: 2, 4, 3, 1, 4 (a late older one, a duplicate of the newest).
    let arrival = [1usize, 3, 2, 0, 3];
    let mut applied: Option<AppearanceSnapshot> = None;
    let mut applied_revs = Vec::new();
    for index in arrival {
        let incoming = deliveries[index].clone();
        let current = applied.as_ref().map_or(0, |snap| snap.rev);
        if should_apply(current, incoming.rev) {
            applied_revs.push(incoming.rev);
            applied = Some(incoming);
        }
    }
    assert_eq!(
        applied_revs,
        [2, 4],
        "older and duplicate deliveries never apply"
    );
    assert_eq!(applied.map(|snap| snap.accent), Some(Accent::Slate));
}

// ---------------------------------------------------------------------------------------------
// URL seed (ap=) and encoding
// ---------------------------------------------------------------------------------------------

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).expect("hex");
            out.push(u8::from_str_radix(hex, 16).expect("hex digits"));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).expect("utf8")
}

fn ap_value(url: &str) -> String {
    let query = url.split_once('?').expect("query").1;
    let pair = query
        .split('&')
        .find(|p| p.starts_with("ap="))
        .expect("ap param");
    percent_decode(pair.trim_start_matches("ap="))
}

#[test]
fn query_encoding_table() {
    let cases = [
        ("abc-_.~", "abc-_.~"),
        ("#", "%23"),
        ("a b", "a%20b"),
        ("&=?", "%26%3D%3F"),
        ("\"{}\"", "%22%7B%7D%22"),
        ("100%", "100%25"),
        ("é", "%C3%A9"),
    ];
    for (input, expected) in cases {
        assert_eq!(encode_query_value(input), expected, "{input:?}");
        assert_eq!(percent_decode(&encode_query_value(input)), input);
    }
}

#[test]
fn seed_url_table_preserves_existing_parameters() {
    let mut s = get_default_settings();
    s.accent = Accent::Violet;
    s.theme = Theme::Custom;
    s.custom_theme_colors.accent = "#a1b2c3".to_string();
    let snap = snapshot_from_settings(7, &s);

    // Orb: no query before, so the parameter opens the query string.
    let orb = with_ap_param("src/orb/index.html", &snap);
    assert!(orb.starts_with("src/orb/index.html?ap="), "{orb}");
    assert_eq!(
        serde_json::from_str::<AppearanceSnapshot>(&ap_value(&orb)).ok(),
        Some(snap.clone())
    );

    // Bubbles: the instance parameter is kept in front of the appearance seed.
    let bubbles = with_ap_param("src/bubbles/index.html?instance=12", &snap);
    assert!(
        bubbles.starts_with("src/bubbles/index.html?instance=12&ap="),
        "{bubbles}"
    );

    // Chat: the opacity and rev parameters are kept, the seed is appended.
    let chat = with_ap_param("src/chat/index.html?opacity=70&rev=5", &snap);
    assert!(
        chat.starts_with("src/chat/index.html?opacity=70&rev=5&ap="),
        "{chat}"
    );
    assert!(
        ap_value(&chat).contains('#'),
        "the decoded value carries the hex colour"
    );
    assert!(!chat.contains('#'), "no raw hash in the URL: {chat}");
}

#[test]
fn seed_value_round_trips_special_characters_in_custom_colours() {
    let mut s = get_default_settings();
    s.theme = Theme::Custom;
    s.custom_theme_colors = CustomThemeColors {
        text: "#0a0B0c".to_string(),
        background: "#FFFFFF".to_string(),
        accent: "#123456".to_string(),
        accent_secondary: "#654321".to_string(),
        warning: "#ffaa00".to_string(),
        error: "#ff0000".to_string(),
    };
    let snap = snapshot_from_settings(3, &s);
    let url = with_ap_param("src/chat/index.html?opacity=20&rev=1", &snap);
    let parsed: AppearanceSnapshot = serde_json::from_str(&ap_value(&url)).expect("valid JSON");
    assert_eq!(parsed, snap);
    assert!(url.is_ascii(), "the URL is plain ASCII");
}

// ---------------------------------------------------------------------------------------------
// Shared replay worker: fake ticker, lifecycle, convergence
// ---------------------------------------------------------------------------------------------

#[test]
fn the_shared_worker_ticks_while_any_consumer_exists_and_stops_when_none_do() {
    let ticks = Cell::new(0usize);
    let sleeps = Cell::new(0usize);
    let alive_for = Cell::new(3usize);
    let runs = run_consumer_replay(
        || {
            let left = alive_for.get();
            alive_for.set(left.saturating_sub(1));
            left > 0
        },
        |d: Duration| {
            assert_eq!(d, Duration::from_secs(2));
            sleeps.set(sleeps.get() + 1);
        },
        || ticks.set(ticks.get() + 1),
    );
    assert_eq!(runs, 3);
    assert_eq!(ticks.get(), 3);
    assert_eq!(sleeps.get(), 3, "one replay interval per tick");
}

#[test]
fn orb_and_bubbles_without_a_chat_window_get_snapshots() {
    let snap = snapshot_from_settings(2, &get_default_settings());
    static ORB_AND_BUBBLES: [&str; 2] = [ORB, BUBBLES];
    let received = RefCell::new(BTreeMap::<&str, usize>::new());
    let exists = only(&ORB_AND_BUBBLES);
    // The fake clock allows exactly one tick, so the loop ends.
    let budget = Cell::new(1usize);
    run_consumer_replay(
        || {
            let left = budget.get();
            budget.set(left.saturating_sub(1));
            left > 0 && CONSUMER_LABELS.iter().any(|l| exists(l))
        },
        |_| {},
        || {
            for (label, _) in plan_deliveries(&snap, &exists) {
                *received.borrow_mut().entry(label).or_insert(0) += 1;
            }
        },
    );
    assert_eq!(received.borrow().get(ORB), Some(&1));
    assert_eq!(received.borrow().get(BUBBLES), Some(&1));
    assert_eq!(
        received.borrow().get(CHAT),
        None,
        "no chat window was ever created"
    );
}

#[test]
fn a_destroyed_and_recreated_chat_keeps_one_worker_while_another_consumer_lives() {
    // The gate allows one worker. The orb lives throughout; the chat window is destroyed and
    // recreated. The single worker runs the whole time and never releases the gate.
    let gate = ReplayGate::new();
    assert!(gate.try_start(), "first claim");
    let snap = snapshot_from_settings(1, &get_default_settings());
    let step = Cell::new(0usize);
    let chat_present = Cell::new(true);
    let orb_deliveries = Cell::new(0usize);
    let ended = Cell::new(false);
    let runs = replay_lifecycle(
        &gate,
        // The worker ends only when no consumer is left; here it ends after its run.
        || !ended.get(),
        || {
            run_consumer_replay(
                || {
                    step.set(step.get() + 1);
                    step.get() <= 6
                },
                |_| {},
                || {
                    // chat is destroyed on ticks 2-3 and recreated on tick 4
                    chat_present.set(!matches!(step.get(), 2 | 3));
                    for (label, _) in
                        plan_deliveries(&snap, |l| l == ORB || (l == CHAT && chat_present.get()))
                    {
                        if label == ORB {
                            orb_deliveries.set(orb_deliveries.get() + 1);
                        }
                    }
                },
            );
            ended.set(true);
        },
    );
    assert_eq!(runs, 1, "one worker run, never a second worker");
    assert_eq!(orb_deliveries.get(), 6, "the orb heard every tick");
    assert!(
        gate.try_start(),
        "the worker released its claim when it ended"
    );
}

#[test]
fn no_consumer_stops_the_worker_and_releases_the_claim() {
    let gate = ReplayGate::new();
    assert!(gate.try_start());
    let ticks = Cell::new(0usize);
    let runs = replay_lifecycle(
        &gate,
        || false,
        || {
            run_consumer_replay(|| false, |_| {}, || ticks.set(ticks.get() + 1));
        },
    );
    assert_eq!(runs, 1);
    assert_eq!(ticks.get(), 0, "no consumer, no tick");
    assert!(
        gate.try_start(),
        "the claim is released for the next consumer"
    );
}

#[test]
fn a_late_listener_converges_on_the_next_tick() {
    let snap = snapshot_from_settings(5, &get_default_settings());
    let listening = Cell::new(false);
    let applied = Cell::new(0u64);
    let tick_of_convergence = Cell::new(None::<usize>);
    let tick = Cell::new(0usize);
    run_consumer_replay(
        || tick.get() < 4,
        |_| {},
        || {
            tick.set(tick.get() + 1);
            if tick.get() == 2 {
                listening.set(true); // the page subscribes late
            }
            // Each tick re-emits the current snapshot to the existing window.
            if listening.get() && should_apply(applied.get(), snap.rev) {
                applied.set(snap.rev);
                tick_of_convergence.set(Some(tick.get()));
            }
        },
    );
    assert_eq!(applied.get(), 5, "the late listener converged");
    assert_eq!(
        tick_of_convergence.get(),
        Some(2),
        "on the first tick after it listened"
    );
}

// ---------------------------------------------------------------------------------------------
// Window-command matrix: the three new commands are main-window only
// ---------------------------------------------------------------------------------------------

#[test]
fn the_new_appearance_commands_are_main_window_only() {
    use crate::window_commands::window_command_allowed;
    let commands = [
        "change_accent_setting",
        "change_chat_theme_setting",
        "change_visual_style_setting",
    ];
    for command in commands {
        assert!(
            window_command_allowed("main", command),
            "{command} from main"
        );
        for label in [
            "chat",
            "orb",
            "orb-bubbles",
            "overlay",
            "recording_overlay",
            "settings",
        ] {
            assert!(
                !window_command_allowed(label, command),
                "{command} refused from {label}"
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Capability pins (contract 5): listen-only, nothing added
// ---------------------------------------------------------------------------------------------

fn capability(src: &str) -> Value {
    serde_json::from_str(src).expect("capability json parses")
}

fn permissions(value: &Value) -> Vec<String> {
    value["permissions"]
        .as_array()
        .expect("permissions array")
        .iter()
        .map(|p| p.as_str().expect("string").to_string())
        .collect()
}

#[test]
fn chat_capability_is_pinned_to_listen_and_unlisten() {
    let value = capability(include_str!("../capabilities/chat.json"));
    assert_eq!(value["windows"], json!(["chat"]));
    assert_eq!(
        permissions(&value),
        ["core:event:allow-listen", "core:event:allow-unlisten"]
    );
}

#[test]
fn orb_capability_is_pinned_to_listen_and_unlisten() {
    let value = capability(include_str!("../capabilities/orb.json"));
    assert_eq!(value["windows"], json!(["orb"]));
    assert_eq!(
        permissions(&value),
        ["core:event:allow-listen", "core:event:allow-unlisten"]
    );
}

#[test]
fn bubbles_capability_is_pinned_to_listen_only() {
    let value = capability(include_str!("../capabilities/bubbles.json"));
    assert_eq!(value["windows"], json!(["orb-bubbles"]));
    assert_eq!(permissions(&value), ["core:event:allow-listen"]);
}

// ---------------------------------------------------------------------------------------------
// Source contracts: wiring of the writers, the seeds and the worker
// ---------------------------------------------------------------------------------------------

fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn all_five_writers_go_through_commit_appearance_once_each() {
    let shortcut = code_only(include_str!("shortcut/mod.rs"));
    let commands = code_only(include_str!("commands/appearance.rs"));
    assert_eq!(
        shortcut.matches("commit_appearance(").count(),
        2,
        "two existing writers"
    );
    assert_eq!(
        commands.matches("commit_appearance(").count(),
        4,
        "four new writers"
    );
    for command in [
        "change_accent_setting",
        "change_chat_theme_setting",
        "change_visual_style_setting",
    ] {
        assert!(
            commands.contains(&format!("pub async fn {command}(")),
            "{command} is async"
        );
    }
    assert!(shortcut.contains("pub async fn change_theme_setting("));
    assert!(shortcut.contains("pub async fn update_custom_theme_colors("));
}

#[test]
fn the_appearance_module_never_prints_or_touches_the_file_system() {
    let src = code_only(include_str!("appearance.rs"));
    for banned in ["println!", "eprintln!", "dbg!", "std::fs"] {
        assert!(!src.contains(banned), "appearance.rs must not use {banned}");
    }
}

#[test]
fn every_window_seeds_its_first_paint_from_the_ap_parameter() {
    let chat = code_only(include_str!("chat_window.rs"));
    assert!(chat.contains("with_ap_param("), "chat URL carries ap=");
    assert!(
        chat.contains("opacity={}&rev={}"),
        "chat keeps its opacity and rev parameters"
    );
    let orb = code_only(include_str!("orb_native.rs"));
    assert!(orb.contains("with_ap_param("), "orb URL carries ap=");
    let bubbles = code_only(include_str!("bubble_native.rs"));
    assert!(bubbles.contains("with_ap_param("), "bubble URL carries ap=");
    assert!(
        bubbles.contains("?instance={instance}"),
        "bubble instance parameter kept"
    );
}

#[test]
fn the_worker_is_started_by_every_consumer_build() {
    let chat = code_only(include_str!("chat_window.rs"));
    assert!(chat.contains("start_replay_worker(app)"));
    let orb = code_only(include_str!("orb_native.rs"));
    assert!(
        orb.contains("start_replay_worker(app)"),
        "orb starts the shared worker"
    );
    let bubbles = code_only(include_str!("bubble_native.rs"));
    assert!(
        bubbles.contains("start_replay_worker(app)"),
        "bubbles start the shared worker"
    );
}

#[test]
fn appearance_snapshot_is_exported_to_the_typescript_bindings() {
    let lib = code_only(include_str!("lib.rs"));
    assert!(lib.contains(".typ::<appearance::AppearanceSnapshot>()"));
    assert!(lib.contains("commands::appearance::change_accent_setting,"));
    assert!(lib.contains("commands::appearance::change_chat_theme_setting,"));
    assert!(lib.contains("commands::appearance::change_visual_style_setting,"));
}
