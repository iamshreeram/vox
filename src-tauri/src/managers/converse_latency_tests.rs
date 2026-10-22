//! Authoritative tests for `converse_latency` (design D4, A7, G9). Do not weaken.

use super::*;
use serde_json::Value;
use std::collections::BTreeSet;

// ---------- helpers ----------

fn timing(
    endpoint_wait_ms: Option<u32>,
    transcribe_ms: Option<u32>,
    verify_ms: Option<u32>,
    mode: EndpointMode,
) -> ListenTiming {
    ListenTiming {
        endpoint_wait_ms,
        transcribe_ms,
        verify_ms,
        mode,
    }
}

/// Report with only `turn` and `endpoint_wait_ms` set (everything else None, mode Fixed).
fn ep_only(turn: u32, endpoint_wait_ms: Option<u32>) -> LatencyReport {
    LatencyReport {
        turn,
        endpoint_wait_ms,
        transcribe_ms: None,
        verify_ms: None,
        brain_ms: None,
        to_speech_ms: None,
        total_ms: None,
        mode: EndpointMode::Fixed,
    }
}

/// Report with every numeric field set to `v`.
fn full(turn: u32, v: u32) -> LatencyReport {
    LatencyReport {
        turn,
        endpoint_wait_ms: Some(v),
        transcribe_ms: Some(v),
        verify_ms: Some(v),
        brain_ms: Some(v),
        to_speech_ms: Some(v),
        total_ms: Some(v),
        mode: EndpointMode::Fixed,
    }
}

/// Rolling stats after pushing `full(i, v)` for each value in order.
fn rolling_full(values: &[u32]) -> RollingStats {
    let mut s = RollingStats::new();
    for (i, v) in values.iter().enumerate() {
        s.push(&full(i as u32 + 1, *v));
    }
    s
}

/// Rolling stats after pushing `ep_only(i, v)` for each value in order.
fn rolling_ep(values: &[u32]) -> RollingStats {
    let mut s = RollingStats::new();
    for (i, v) in values.iter().enumerate() {
        s.push(&ep_only(i as u32 + 1, Some(*v)));
    }
    s
}

fn fs(median: Option<u32>, p90: Option<u32>) -> FieldStats {
    FieldStats { median, p90 }
}

/// Tag strings allowed anywhere in the payload (the mode serialization only).
const MODE_TAGS: [&str; 3] = ["fixed", "smart_turn", "smart_turn_fallback"];

/// Walk every leaf: numbers and null are fine, strings must be a mode tag, anything else fails.
/// Returns every string leaf found, as (path, value).
fn check_leaves(v: &Value, path: &str, strings: &mut Vec<(String, String)>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                check_leaves(child, &format!("{path}.{k}"), strings);
            }
        }
        Value::Number(_) | Value::Null => {}
        Value::String(s) => {
            assert!(
                MODE_TAGS.contains(&s.as_str()),
                "{path}: string leaf {s:?} is not a mode tag"
            );
            strings.push((path.to_string(), s.clone()));
        }
        other => panic!("{path}: disallowed leaf {other:?} (only numbers, null, mode tags)"),
    }
}

fn key_set(v: &Value) -> BTreeSet<String> {
    v.as_object()
        .expect("expected a JSON object")
        .keys()
        .cloned()
        .collect()
}

fn set(keys: &[&str]) -> BTreeSet<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

fn assert_specta<T: specta::Type>() {}

// ---------- LatencyReport::build ----------

#[test]
fn build_sums_all_present_instants_exactly() {
    let t = timing(Some(400), Some(300), None, EndpointMode::Fixed);
    let r = LatencyReport::build(7, Some(&t), Some(500), Some(120));
    assert_eq!(r.turn, 7);
    assert_eq!(r.endpoint_wait_ms, Some(400));
    assert_eq!(r.transcribe_ms, Some(300));
    assert_eq!(
        r.verify_ms, None,
        "verify absent when the filter did not run"
    );
    assert_eq!(r.brain_ms, Some(500));
    assert_eq!(r.to_speech_ms, Some(120));
    assert_eq!(r.total_ms, Some(1320), "400 + 300 + 500 + 120");
    assert_eq!(r.mode, EndpointMode::Fixed);
}

#[test]
fn build_adds_verify_when_present() {
    let t = timing(Some(400), Some(300), Some(80), EndpointMode::SmartTurn);
    let r = LatencyReport::build(2, Some(&t), Some(500), Some(120));
    assert_eq!(r.verify_ms, Some(80));
    assert_eq!(r.total_ms, Some(1400), "400 + 300 + 80 + 500 + 120");
}

#[test]
fn build_total_is_none_when_endpoint_wait_missing_and_other_fields_stay() {
    let t = timing(None, Some(300), None, EndpointMode::Fixed);
    let r = LatencyReport::build(1, Some(&t), Some(500), Some(120));
    assert_eq!(r.total_ms, None);
    assert_eq!(r.endpoint_wait_ms, None);
    assert_eq!(r.transcribe_ms, Some(300));
    assert_eq!(r.brain_ms, Some(500));
    assert_eq!(r.to_speech_ms, Some(120));
}

#[test]
fn build_total_is_none_when_transcribe_missing_and_other_fields_stay() {
    let t = timing(Some(400), None, None, EndpointMode::Fixed);
    let r = LatencyReport::build(1, Some(&t), Some(500), Some(120));
    assert_eq!(r.total_ms, None);
    assert_eq!(r.endpoint_wait_ms, Some(400));
    assert_eq!(r.transcribe_ms, None);
    assert_eq!(r.brain_ms, Some(500));
    assert_eq!(r.to_speech_ms, Some(120));
}

#[test]
fn build_total_is_none_when_brain_missing_and_other_fields_stay() {
    let t = timing(Some(400), Some(300), Some(80), EndpointMode::Fixed);
    let r = LatencyReport::build(1, Some(&t), None, Some(120));
    assert_eq!(
        r.total_ms, None,
        "verify present must not rescue a missing brain"
    );
    assert_eq!(r.brain_ms, None);
    assert_eq!(r.endpoint_wait_ms, Some(400));
    assert_eq!(r.transcribe_ms, Some(300));
    assert_eq!(r.verify_ms, Some(80));
    assert_eq!(r.to_speech_ms, Some(120));
}

#[test]
fn build_total_is_none_when_to_speech_missing_and_other_fields_stay() {
    let t = timing(Some(400), Some(300), None, EndpointMode::Fixed);
    let r = LatencyReport::build(1, Some(&t), Some(500), None);
    assert_eq!(r.total_ms, None);
    assert_eq!(r.to_speech_ms, None);
    assert_eq!(r.endpoint_wait_ms, Some(400));
    assert_eq!(r.transcribe_ms, Some(300));
    assert_eq!(r.brain_ms, Some(500));
}

#[test]
fn build_without_timing_has_all_listen_fields_none_and_fixed_mode() {
    let r = LatencyReport::build(3, None, Some(500), Some(120));
    assert_eq!(r.turn, 3);
    assert_eq!(r.endpoint_wait_ms, None);
    assert_eq!(r.transcribe_ms, None);
    assert_eq!(r.verify_ms, None);
    assert_eq!(r.mode, EndpointMode::Fixed);
    assert_eq!(r.total_ms, None);
    assert_eq!(
        r.brain_ms,
        Some(500),
        "brain comes from the argument, not timing"
    );
    assert_eq!(r.to_speech_ms, Some(120));
}

#[test]
fn build_copies_mode_smart_turn() {
    let t = timing(Some(1), Some(1), None, EndpointMode::SmartTurn);
    let r = LatencyReport::build(1, Some(&t), Some(1), Some(1));
    assert_eq!(r.mode, EndpointMode::SmartTurn);
}

#[test]
fn build_copies_mode_smart_turn_fallback() {
    let t = timing(Some(1), Some(1), None, EndpointMode::SmartTurnFallback);
    let r = LatencyReport::build(1, Some(&t), Some(1), Some(1));
    assert_eq!(r.mode, EndpointMode::SmartTurnFallback);
}

#[test]
fn build_saturates_at_u32_max_without_panic() {
    let m = u32::MAX;
    let t = timing(Some(m), Some(m), Some(m), EndpointMode::SmartTurn);
    let r = LatencyReport::build(1, Some(&t), Some(m), Some(m));
    assert_eq!(r.total_ms, Some(u32::MAX));
    assert_eq!(r.verify_ms, Some(u32::MAX));

    let t2 = timing(Some(m - 1), Some(1), None, EndpointMode::Fixed);
    let r2 = LatencyReport::build(2, Some(&t2), Some(0), Some(0));
    assert_eq!(
        r2.total_ms,
        Some(u32::MAX),
        "exact sum equal to MAX stays MAX"
    );
}

// ---------- RollingStats ----------

#[test]
fn empty_stats_are_all_none_and_samples_zero() {
    let s = RollingStats::new();
    assert_eq!(s.len(), 0);
    let st = s.stats();
    assert_eq!(st.endpoint_wait, FieldStats::default());
    assert_eq!(st.transcribe, FieldStats::default());
    assert_eq!(st.verify, FieldStats::default());
    assert_eq!(st.brain, FieldStats::default());
    assert_eq!(st.to_speech, FieldStats::default());
    assert_eq!(st.total, FieldStats::default());
    assert_eq!(st.endpoint_wait, fs(None, None));
    assert_eq!(st.samples, 0);
}

#[test]
fn one_value_gives_median_equal_p90_equal_value() {
    let st = rolling_full(&[42]).stats();
    assert_eq!(st.endpoint_wait, fs(Some(42), Some(42)));
    assert_eq!(st.total, fs(Some(42), Some(42)));
    assert_eq!(st.samples, 1);
}

#[test]
fn two_values_median_is_lower_and_p90_is_second() {
    // sorted = [10, 30]; median = sorted[(2-1)/2] = 10; p90 = sorted[ceil(1.8)-1] = sorted[1] = 30
    let st = rolling_ep(&[30, 10]).stats();
    assert_eq!(st.endpoint_wait, fs(Some(10), Some(30)));
    assert_eq!(st.samples, 2);
}

#[test]
fn ten_values_median_is_index_4_and_p90_is_index_8() {
    // pushed in reverse so sorting is exercised; sorted = 1..=10
    let values: Vec<u32> = (1..=10).rev().collect();
    let st = rolling_ep(&values).stats();
    assert_eq!(st.endpoint_wait, fs(Some(5), Some(9)));
}

#[test]
fn nineteen_values_median_is_index_9_and_p90_is_index_17() {
    // sorted = 1..=19; median = sorted[9] = 10; p90 = sorted[ceil(17.1)-1] = sorted[17] = 18
    let values: Vec<u32> = (1..=19).collect();
    let st = rolling_ep(&values).stats();
    assert_eq!(st.endpoint_wait, fs(Some(10), Some(18)));
    assert_eq!(st.samples, 19);
}

#[test]
fn twenty_values_fill_the_window_exactly() {
    // sorted = 1..=20; median = sorted[9] = 10; p90 = sorted[ceil(18)-1] = sorted[17] = 18
    let values: Vec<u32> = (1..=20).collect();
    let s = rolling_ep(&values);
    assert_eq!(s.len(), 20);
    let st = s.stats();
    assert_eq!(st.endpoint_wait, fs(Some(10), Some(18)));
    assert_eq!(st.samples, 20);
}

#[test]
fn twenty_first_report_evicts_the_first_by_value() {
    // window after 21 pushes = 2..=21; sorted = 2..=21
    // median = sorted[9] = 11; p90 = sorted[17] = 19
    // (if 1 were kept instead of evicted: median 11 but p90 would be 20)
    let values: Vec<u32> = (1..=21).collect();
    let s = rolling_ep(&values);
    assert_eq!(s.len(), 20, "samples caps at WINDOW");
    let st = s.stats();
    assert_eq!(st.endpoint_wait, fs(Some(11), Some(19)));
    assert_eq!(st.samples, 20);
}

#[test]
fn evicted_outlier_no_longer_affects_p90() {
    // push a huge first value, then 1..=20: the 21st push evicts 1000
    let mut values = vec![1000u32];
    values.extend(1..=20);
    let st = rolling_ep(&values).stats();
    assert_eq!(st.endpoint_wait, fs(Some(10), Some(18)));
}

#[test]
fn unsorted_input_is_sorted_before_ranking() {
    // sorted = [1,2,3,4,5]; median = sorted[2] = 3; p90 = sorted[ceil(4.5)-1] = sorted[4] = 5
    let st = rolling_ep(&[5, 1, 4, 2, 3]).stats();
    assert_eq!(st.endpoint_wait, fs(Some(3), Some(5)));
}

#[test]
fn duplicates_are_counted() {
    // sorted = [3,7,7,7]; median = sorted[1] = 7; p90 = sorted[ceil(3.6)-1] = sorted[3] = 7
    let st = rolling_ep(&[7, 7, 7, 3]).stats();
    assert_eq!(st.endpoint_wait, fs(Some(7), Some(7)));
    // duplicates of the low value: sorted = [3,3,3,9]; median = sorted[1] = 3; p90 = sorted[3] = 9
    let st2 = rolling_ep(&[3, 9, 3, 3]).stats();
    assert_eq!(st2.endpoint_wait, fs(Some(3), Some(9)));
}

#[test]
fn none_values_are_ignored_per_field() {
    // endpoint_wait present in reports 1 and 3 only: sorted = [100, 200]
    let mut s = RollingStats::new();
    s.push(&ep_only(1, Some(100)));
    s.push(&ep_only(2, None));
    s.push(&ep_only(3, Some(200)));
    let st = s.stats();
    assert_eq!(st.endpoint_wait, fs(Some(100), Some(200)));
    assert_eq!(st.brain, fs(None, None), "no brain values anywhere");
    assert_eq!(st.samples, 3, "samples counts reports held in the window");
}

#[test]
fn fields_are_independent_when_reports_carry_different_fields() {
    let brain_only = LatencyReport {
        brain_ms: Some(50),
        ..ep_only(1, None)
    };
    let ep_report = ep_only(2, Some(10));
    let mut s = RollingStats::new();
    s.push(&brain_only);
    s.push(&ep_report);
    let st = s.stats();
    assert_eq!(
        st.endpoint_wait,
        fs(Some(10), Some(10)),
        "brain report must not affect endpoint"
    );
    assert_eq!(
        st.brain,
        fs(Some(50), Some(50)),
        "endpoint report must not affect brain"
    );
    assert_eq!(st.total, fs(None, None));
}

#[test]
fn reset_empties_the_window() {
    let mut s = rolling_full(&[1, 2, 3]);
    assert_eq!(s.len(), 3);
    s.reset();
    assert_eq!(s.len(), 0);
    let st = s.stats();
    assert_eq!(st.samples, 0);
    assert_eq!(st.total, fs(None, None));
    assert_eq!(st.endpoint_wait, fs(None, None));
}

#[test]
fn window_of_all_none_totals_gives_total_none() {
    let mut s = RollingStats::new();
    for turn in 1..=5 {
        s.push(&ep_only(turn, Some(turn * 10)));
    }
    let st = s.stats();
    assert_eq!(st.total, fs(None, None), "no total_ms values in any report");
    assert_eq!(
        st.endpoint_wait,
        fs(Some(30), Some(50)),
        "sorted 10..50: median 30, p90 50"
    );
}

// ---------- serialization (bindings / payload contract) ----------

#[test]
fn mode_serializes_to_the_three_snake_case_tags() {
    assert_eq!(
        serde_json::to_value(EndpointMode::Fixed).unwrap(),
        serde_json::json!("fixed")
    );
    assert_eq!(
        serde_json::to_value(EndpointMode::SmartTurn).unwrap(),
        serde_json::json!("smart_turn")
    );
    assert_eq!(
        serde_json::to_value(EndpointMode::SmartTurnFallback).unwrap(),
        serde_json::json!("smart_turn_fallback")
    );
}

fn populated_event() -> LatencyEvent {
    let t = timing(Some(400), Some(300), None, EndpointMode::SmartTurn);
    let report = LatencyReport::build(9, Some(&t), Some(500), Some(120));
    let mut stats = RollingStats::new();
    stats.push(&report);
    stats.push(&LatencyReport::build(10, None, None, None));
    LatencyEvent {
        report,
        stats: stats.stats(),
    }
}

#[test]
fn event_json_has_exact_snake_case_keys() {
    let v = serde_json::to_value(populated_event()).unwrap();
    assert_eq!(key_set(&v), set(&["report", "stats"]));
    assert_eq!(
        key_set(&v["report"]),
        set(&[
            "turn",
            "endpoint_wait_ms",
            "transcribe_ms",
            "verify_ms",
            "brain_ms",
            "to_speech_ms",
            "total_ms",
            "mode",
        ])
    );
    assert_eq!(
        key_set(&v["stats"]),
        set(&[
            "endpoint_wait",
            "transcribe",
            "verify",
            "brain",
            "to_speech",
            "total",
            "samples",
        ])
    );
    for field in [
        "endpoint_wait",
        "transcribe",
        "verify",
        "brain",
        "to_speech",
        "total",
    ] {
        assert_eq!(
            key_set(&v["stats"][field]),
            set(&["median", "p90"]),
            "{field}"
        );
    }
}

#[test]
fn event_json_leaves_are_only_numbers_null_or_mode_tags() {
    let v = serde_json::to_value(populated_event()).unwrap();
    let mut strings = Vec::new();
    check_leaves(&v, "event", &mut strings);
    assert_eq!(
        strings,
        vec![("event.report.mode".to_string(), "smart_turn".to_string())],
        "the only string leaf must be the report mode tag"
    );
}

#[test]
fn event_json_mode_tag_matches_the_report_mode() {
    let mut event = populated_event();
    for (mode, tag) in [
        (EndpointMode::Fixed, "fixed"),
        (EndpointMode::SmartTurn, "smart_turn"),
        (EndpointMode::SmartTurnFallback, "smart_turn_fallback"),
    ] {
        event.report.mode = mode;
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["report"]["mode"], Value::String(tag.to_string()));
    }
}

#[test]
fn absent_values_serialize_as_null_not_missing_keys() {
    let v = serde_json::to_value(LatencyEvent {
        report: LatencyReport::build(1, None, None, None),
        stats: RollingStats::new().stats(),
    })
    .unwrap();
    assert_eq!(v["report"]["verify_ms"], Value::Null);
    assert_eq!(v["report"]["total_ms"], Value::Null);
    assert_eq!(v["stats"]["total"]["median"], Value::Null);
    assert_eq!(v["stats"]["samples"], serde_json::json!(0));
}

#[test]
fn text_leak_guard_numbers_only_payload_has_no_words() {
    // Built from timing and brain/to_speech numbers only: no text can enter these types.
    let t = timing(
        Some(412),
        Some(287),
        Some(64),
        EndpointMode::SmartTurnFallback,
    );
    let report = LatencyReport::build(4, Some(&t), Some(533), Some(101));
    let mut stats = RollingStats::new();
    stats.push(&report);
    let json = serde_json::to_string(&LatencyEvent {
        report,
        stats: stats.stats(),
    })
    .unwrap();
    let lower = json.to_lowercase();
    for word in [
        "transcript",
        "utterance",
        "hello",
        "reply",
        "password",
        "secret",
        "assistant",
        "prompt",
        "voiceprint",
    ] {
        assert!(!lower.contains(word), "payload leaks word {word:?}: {json}");
    }
    let mut strings = Vec::new();
    check_leaves(
        &serde_json::from_str::<Value>(&json).unwrap(),
        "event",
        &mut strings,
    );
    assert_eq!(
        strings.len(),
        1,
        "exactly one string leaf (mode tag): {json}"
    );
}

// ---------- event gating ----------

#[test]
fn event_enabled_truth_table() {
    assert!(!event_enabled(false, false), "setting off, absent");
    assert!(
        !event_enabled(false, true),
        "setting off always false, even when present"
    );
    assert!(event_enabled(true, true), "setting on and present");
    assert!(!event_enabled(true, false), "setting on but absent");
}

#[test]
fn event_name_is_converse_latency() {
    assert_eq!(EVENT_NAME, "converse-latency");
}

// ---------- bindings: specta::Type must compile for every public type ----------

#[test]
fn every_public_type_derives_specta_type() {
    assert_specta::<EndpointMode>();
    assert_specta::<ListenTiming>();
    assert_specta::<LatencyReport>();
    assert_specta::<FieldStats>();
    assert_specta::<LatencyStats>();
    assert_specta::<LatencyEvent>();
}
