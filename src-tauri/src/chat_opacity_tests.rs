//! Spec tests for the see-through conversation window (Track T, DESIGN Revision 3).
//!
//! Covers A-T1 (clamp table, field-local deserializer through the raw AppSettings JSON, default),
//! A-T2 (command target is exactly the `chat` label, absent window is success), A-T3 (window
//! builder pins are in chat_window_tests.rs), A-T4 (replay worker with a fake clock: starts once,
//! emits the current snapshot each tick, late subscriber converges, stops on destroy) and the
//! effective-opacity rule (compositing unavailable forces 100). Workers must not edit this file.

use super::*;
use crate::chat_presentation::{LinuxBackend, Os, PlatformEnv};
use crate::settings::{get_default_settings, AppSettings};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::time::Duration;

fn env(os: Os, backend: LinuxBackend, composited: bool) -> PlatformEnv {
    PlatformEnv {
        os,
        linux_backend: backend,
        composited,
    }
}

/// Default settings as the raw JSON object a store would hold, with `orb_size_px` set to a
/// non-default value so the test proves unrelated fields survive the field-local deserializer.
fn stored_json_with(chat_opacity: Value) -> Value {
    let mut value = serde_json::to_value(get_default_settings()).expect("defaults serialize");
    let obj = value.as_object_mut().expect("settings is an object");
    obj.insert("orb_size_px".to_string(), json!(140));
    obj.insert("chat_opacity_pct".to_string(), chat_opacity);
    value
}

// ---------------------------------------------------------------------------------------------
// A-T1: constants and clamp
// ---------------------------------------------------------------------------------------------

#[test]
fn range_and_default_are_20_100_70() {
    assert_eq!(CHAT_OPACITY_MIN, 20);
    assert_eq!(CHAT_OPACITY_MAX, 100);
    assert_eq!(CHAT_OPACITY_DEFAULT, 70);
    assert_eq!(default_chat_opacity_pct(), 70);
}

#[test]
fn clamp_table_matches_the_range() {
    let cases: [(i64, u8); 11] = [
        (0, 20),
        (19, 20),
        (20, 20),
        (70, 70),
        (100, 100),
        (101, 100),
        (255, 100),
        (-5, 20),
        (-1, 20),
        (i64::MIN, 20),
        (i64::MAX, 100),
    ];
    for (input, expected) in cases {
        assert_eq!(clamp_chat_opacity_pct(input), expected, "clamp({input})");
    }
}

#[test]
fn default_settings_carry_70() {
    assert_eq!(get_default_settings().chat_opacity_pct, 70);
}

#[test]
fn store_without_the_key_loads_70_and_keeps_unrelated_fields() {
    let mut value = stored_json_with(json!(null));
    value
        .as_object_mut()
        .expect("object")
        .remove("chat_opacity_pct");
    let loaded: AppSettings = serde_json::from_value(value).expect("loads");
    assert_eq!(loaded.chat_opacity_pct, 70, "missing key -> default");
    assert_eq!(loaded.orb_size_px, Some(140), "unrelated field intact");
}

#[test]
fn raw_json_values_are_clamped_or_defaulted_field_locally() {
    let cases: [(Value, u8); 9] = [
        (json!(-1), 20),
        (json!(300), 100),
        (json!(null), 70),
        (json!("bad"), 70),
        (json!(20.5), 70),
        (json!(20), 20),
        (json!(55), 55),
        (json!(true), 70),
        (json!([1, 2]), 70),
    ];
    for (raw, expected) in cases {
        let loaded: AppSettings =
            serde_json::from_value(stored_json_with(raw.clone())).expect("store loads");
        assert_eq!(loaded.chat_opacity_pct, expected, "raw {raw}");
        assert_eq!(
            loaded.orb_size_px,
            Some(140),
            "orb_size_px intact next to raw {raw}"
        );
    }
}

#[test]
fn the_opacity_round_trips_through_serde() {
    let mut settings = get_default_settings();
    settings.chat_opacity_pct = 35;
    let value = serde_json::to_value(&settings).expect("serialize");
    assert_eq!(value["chat_opacity_pct"], json!(35));
    let back: AppSettings = serde_json::from_value(value).expect("load");
    assert_eq!(back.chat_opacity_pct, 35);
}

// ---------------------------------------------------------------------------------------------
// Effective opacity: compositing unavailable forces fully opaque
// ---------------------------------------------------------------------------------------------

#[test]
fn effective_opacity_is_the_setting_where_compositing_is_available() {
    for pct in [20u8, 70, 100] {
        assert_eq!(
            effective_chat_opacity_pct(pct, &env(Os::MacOs, LinuxBackend::Unknown, false)),
            pct
        );
        assert_eq!(
            effective_chat_opacity_pct(pct, &env(Os::Windows, LinuxBackend::Unknown, false)),
            pct
        );
        assert_eq!(
            effective_chat_opacity_pct(pct, &env(Os::Linux, LinuxBackend::X11, true)),
            pct
        );
    }
}

#[test]
fn effective_opacity_is_forced_to_100_without_compositing() {
    for pct in [20u8, 70] {
        assert_eq!(
            effective_chat_opacity_pct(pct, &env(Os::Linux, LinuxBackend::X11, false)),
            100,
            "X11 without a compositor"
        );
        assert_eq!(
            effective_chat_opacity_pct(pct, &env(Os::Linux, LinuxBackend::Wayland, true)),
            100,
            "Wayland-blocked transparency"
        );
    }
}

#[test]
fn effective_opacity_never_leaves_the_range() {
    assert_eq!(
        effective_chat_opacity_pct(0, &env(Os::MacOs, LinuxBackend::Unknown, false)),
        CHAT_OPACITY_MIN
    );
}

// ---------------------------------------------------------------------------------------------
// A-T4: snapshot holder, replay worker with a fake clock
// ---------------------------------------------------------------------------------------------

#[test]
fn replay_interval_is_two_seconds() {
    assert_eq!(replay_interval(), Duration::from_secs(2));
}

#[test]
fn seeding_then_committing_bumps_the_rev_monotonically() {
    let holder = OpacityHolder::new();
    let seeded = holder.seed(70);
    assert_eq!(seeded, OpacitySnapshot { rev: 1, pct: 70 });
    // seeding again keeps the committed snapshot
    assert_eq!(holder.seed(20), OpacitySnapshot { rev: 1, pct: 70 });
    let committed = holder.commit(20);
    assert_eq!(committed, OpacitySnapshot { rev: 2, pct: 20 });
    let again = holder.commit(500);
    assert_eq!(again, OpacitySnapshot { rev: 3, pct: 100 });
    assert_eq!(holder.snapshot(), again);
}

#[test]
fn gate_starts_exactly_once_until_finished() {
    let gate = ReplayGate::new();
    assert!(gate.try_start(), "first start wins");
    assert!(!gate.try_start(), "no second worker while one runs");
    assert!(!gate.try_start());
    gate.finish();
    assert!(
        gate.try_start(),
        "a new window may start again after the worker ended"
    );
}

#[test]
fn replay_worker_emits_the_current_snapshot_each_tick_and_stops_on_destroy() {
    let holder = OpacityHolder::new();
    holder.seed(70); // rev 1, pct 70
    let ticks = Cell::new(0usize);
    let emitted: RefCell<Vec<OpacitySnapshot>> = RefCell::new(Vec::new());
    let sleeps: RefCell<Vec<Duration>> = RefCell::new(Vec::new());
    let emits = run_replay(
        || holder.snapshot(),
        || {
            // alive for 4 ticks, then the window is destroyed
            let n = ticks.get();
            ticks.set(n + 1);
            n < 4
        },
        |d| sleeps.borrow_mut().push(d),
        |snap| {
            emitted.borrow_mut().push(snap);
            // a commit happens after the first tick: later ticks must read it fresh
            if snap.rev == 1 {
                holder.commit(20);
            }
        },
    );
    assert_eq!(emits, 4, "one emit per live tick");
    let got = emitted.borrow().clone();
    assert_eq!(got[0], OpacitySnapshot { rev: 1, pct: 70 });
    assert_eq!(
        got[1],
        OpacitySnapshot { rev: 2, pct: 20 },
        "next tick reads the current snapshot, not a captured one"
    );
    assert!(sleeps.borrow().iter().all(|d| *d == replay_interval()));
}

#[test]
fn a_late_subscriber_converges_on_the_latest_commit() {
    let holder = OpacityHolder::new();
    holder.seed(70);
    holder.commit(20); // rev 2 committed long before anyone listens
    let ticks = Cell::new(0usize);
    let last: Cell<Option<OpacitySnapshot>> = Cell::new(None);
    // the subscriber only listens after three replay ticks with no other event
    run_replay(
        || holder.snapshot(),
        || {
            let n = ticks.get();
            ticks.set(n + 1);
            n < 4
        },
        |_| {},
        |snap| {
            if ticks.get() >= 3 {
                last.set(Some(snap));
            }
        },
    );
    assert_eq!(last.get(), Some(OpacitySnapshot { rev: 2, pct: 20 }));
}

#[test]
fn the_replay_worker_is_started_from_the_window_builder() {
    let src = include_str!("chat_window.rs");
    assert!(
        src.contains("start_replay_worker(app)") || src.contains("start_replay_worker(&app)"),
        "build_chat_window must start the replay worker"
    );
    assert!(
        src.contains("CHAT_OPACITY_REPLAY") && src.contains("try_start"),
        "the worker is guarded by the single-instance gate"
    );
}

// ---------------------------------------------------------------------------------------------
// A-T2: the command's emit plan targets only the chat label; an absent window is success
// ---------------------------------------------------------------------------------------------

#[test]
fn the_emit_target_is_exactly_the_chat_label() {
    assert_eq!(CHAT_OPACITY_TARGET, "chat");
    assert_eq!(CHAT_OPACITY_TARGET, crate::chat_window::CHAT_WINDOW_LABEL);
    assert_eq!(CHAT_OPACITY_EVENT, "chat-opacity");
}

#[test]
fn an_absent_window_plans_no_emit_and_is_not_an_error() {
    let snap = OpacitySnapshot { rev: 5, pct: 20 };
    assert_eq!(
        plan_emit(false, snap, &env(Os::MacOs, LinuxBackend::Unknown, false)),
        None
    );
}

#[test]
fn a_present_window_gets_the_effective_snapshot_on_the_chat_label() {
    let snap = OpacitySnapshot { rev: 5, pct: 20 };
    assert_eq!(
        plan_emit(true, snap, &env(Os::MacOs, LinuxBackend::Unknown, false)),
        Some(("chat", OpacitySnapshot { rev: 5, pct: 20 }))
    );
    // forced opaque without compositing; the rev is still the committed one
    assert_eq!(
        plan_emit(true, snap, &env(Os::Linux, LinuxBackend::Wayland, true)),
        Some(("chat", OpacitySnapshot { rev: 5, pct: 100 }))
    );
}

#[test]
fn the_command_source_emits_only_to_the_chat_target() {
    let src = include_str!("commands/chat_opacity.rs");
    assert!(
        src.contains("emit_to(") && src.contains("CHAT_OPACITY_TARGET"),
        "the command emits with emit_to(CHAT_OPACITY_TARGET, ..)"
    );
    assert!(
        !src.contains(".emit(") && !src.contains("emit_all("),
        "no broadcast emit"
    );
    assert!(src.contains("change_chat_opacity_setting"));
    assert!(
        src.contains("write_settings_gated("),
        "writes through the gated writer"
    );
    assert!(
        !src.contains("write_settings(") && !src.contains("store"),
        "no direct store write"
    );
}

#[test]
fn the_command_is_registered_and_refused_from_the_chat_window() {
    let lib = include_str!("lib.rs");
    assert!(
        lib.contains("commands::chat_opacity::change_chat_opacity_setting,"),
        "registered in lib.rs"
    );
    assert!(
        !crate::window_commands::window_command_allowed("chat", "change_chat_opacity_setting"),
        "the chat window must not be allowed to invoke it"
    );
    assert!(
        crate::window_commands::window_command_allowed("main", "change_chat_opacity_setting"),
        "main may invoke it"
    );
    assert!(
        !crate::window_commands::window_command_allowed(
            "recording_overlay",
            "change_chat_opacity_setting"
        ),
        "the overlay is refused too: the command is main-window-only"
    );
    assert!(!crate::chat_window::chat_command_allowed(
        "chat",
        "change_chat_opacity_setting"
    ));
}

// ---------------------------------------------------------------------------------------------
// Review fixes (validator round 1)
// ---------------------------------------------------------------------------------------------

#[test]
fn the_page_is_built_with_the_effective_opacity_and_its_rev() {
    // (env, committed pct, rev) -> (pct in the URL, rev in the URL)
    let cases: [(PlatformEnv, u8, u64, u8); 4] = [
        (env(Os::Linux, LinuxBackend::X11, true), 70, 7, 70),
        (env(Os::Linux, LinuxBackend::X11, false), 70, 7, 100),
        (env(Os::Linux, LinuxBackend::Wayland, true), 70, 7, 100),
        (env(Os::MacOs, LinuxBackend::Unknown, false), 70, 7, 70),
    ];
    for (platform, pct, rev, expected_pct) in cases {
        let page = page_snapshot(OpacitySnapshot { rev, pct }, &platform);
        assert_eq!(
            page,
            OpacitySnapshot {
                rev,
                pct: expected_pct
            },
            "{platform:?}"
        );
    }
}

#[test]
fn the_build_reads_the_effective_page_value_on_the_main_thread() {
    let src = include_str!("chat_window.rs");
    assert!(
        src.contains("page_snapshot(") && src.contains("orb_native::current_env()"),
        "the URL value comes from page_snapshot with the platform facts"
    );
}

#[test]
fn the_command_is_async_like_the_other_settings_commands() {
    let src = include_str!("commands/chat_opacity.rs");
    assert!(
        src.contains("pub async fn change_chat_opacity_setting("),
        "async, so the main thread never blocks on the settings gate"
    );
}

#[test]
fn a_stale_write_cannot_change_the_chat_opacity() {
    let mut committed = get_default_settings();
    committed.chat_opacity_pct = 35;
    let stale = get_default_settings(); // an old snapshot at the default (70)
    let merged = crate::settings::preserve_launch_fields(stale, &committed);
    assert_eq!(merged.chat_opacity_pct, 35, "committed value kept");

    let mut committed_default = get_default_settings();
    committed_default.chat_opacity_pct = 70;
    let mut stale_low = get_default_settings();
    stale_low.chat_opacity_pct = 20;
    let merged = crate::settings::preserve_launch_fields(stale_low, &committed_default);
    assert_eq!(merged.chat_opacity_pct, 70, "a stale low value cannot win");
}

#[test]
fn the_replay_worker_is_restarted_when_a_window_is_rebuilt_in_the_gap() {
    // Worker running; the window is destroyed; the worker releases the gate; a rebuild claims the
    // gate in the gap. The old loop must then stand down: exactly one worker, the rebuild's.
    let gate = ReplayGate::new();
    assert!(gate.try_start(), "first worker");
    let mut destroyed_once = false;
    let mut rebuild_claimed = false;
    let runs = replay_lifecycle(
        &gate,
        || {
            // the gap: the window is rebuilt and its start claims the released gate
            rebuild_claimed = gate.try_start();
            true
        },
        || destroyed_once = true,
    );
    assert!(destroyed_once);
    assert_eq!(runs, 1, "the old loop stood down");
    assert!(rebuild_claimed, "the rebuild got the gate in the gap");
    assert!(
        !gate.try_start(),
        "exactly one worker is running afterwards"
    );
}

#[test]
fn the_replay_worker_goes_on_when_the_window_is_still_there_and_the_gate_is_free() {
    let gate = ReplayGate::new();
    assert!(gate.try_start());
    // alive after the first run (the loop goes on), gone after the second
    let checks = Cell::new(0usize);
    let runs = replay_lifecycle(
        &gate,
        || {
            let n = checks.get();
            checks.set(n + 1);
            n == 0
        },
        || {},
    );
    assert_eq!(runs, 2, "one more run while the window existed");
    assert!(gate.try_start(), "the gate is free after the worker ends");
    gate.finish();
}

#[test]
fn a_worker_exits_and_frees_the_gate_when_no_window_is_rebuilt() {
    let gate = ReplayGate::new();
    assert!(gate.try_start());
    let runs = replay_lifecycle(&gate, || false, || {});
    assert_eq!(runs, 1);
    assert!(gate.try_start(), "the gate is free for a later window");
    gate.finish();
}

#[test]
fn the_opacity_module_never_logs_or_prints() {
    let src = include_str!("chat_opacity.rs");
    for banned in ["println!", "eprintln!", "dbg!", "std::fs"] {
        assert!(
            !src.contains(banned),
            "chat_opacity.rs must not use {banned}"
        );
    }
}
