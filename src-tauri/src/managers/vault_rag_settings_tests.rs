//! T2 settings and eligibility tests for the vault RAG (DESIGN Rev 5, 3.1 and 3.2). Step S5:
//! written first (red at S5, green from S6). Plain-data tests (defaults, legacy JSON, round trip)
//! and the eligibility table exercise the real fields and rules.

use crate::managers::composer_config::{composer_provider_supported, TtsComposerMode};
use crate::managers::conversation_save::{answer_only_for, rag_possible};
use crate::settings::{get_default_settings, parse_rag_folders, AppSettings};

const DEFAULT_FOLDERS: &str = "Projects,Daily,Memory,Meetings,Resources";

#[test]
fn cfg_defaults_off_and_default_folders() {
    let s = get_default_settings();
    assert!(!s.vault_rag_enabled, "vault RAG must default to off");
    assert_eq!(s.vault_rag_folders, DEFAULT_FOLDERS);
}

#[test]
fn cfg_legacy_json_without_fields_loads() {
    let mut value = serde_json::to_value(get_default_settings()).expect("serialise defaults");
    let object = value.as_object_mut().expect("settings are a JSON object");
    object.remove("vault_rag_enabled");
    object.remove("vault_rag_folders");
    let loaded: AppSettings = serde_json::from_value(value).expect("legacy JSON must load");
    assert!(!loaded.vault_rag_enabled);
    assert_eq!(loaded.vault_rag_folders, DEFAULT_FOLDERS);
}

#[test]
fn cfg_round_trip() {
    let mut s = get_default_settings();
    s.vault_rag_enabled = true;
    s.vault_rag_folders = "Daily,Notes".to_string();
    let value = serde_json::to_value(&s).expect("serialise");
    let back: AppSettings = serde_json::from_value(value).expect("deserialise");
    assert!(back.vault_rag_enabled);
    assert_eq!(back.vault_rag_folders, "Daily,Notes");
}

#[test]
fn cfg_parse_rag_folders_table() {
    let default_list: Vec<String> = vec!["Projects", "Daily", "Memory", "Meetings", "Resources"]
        .into_iter()
        .map(str::to_string)
        .collect();
    assert_eq!(parse_rag_folders(DEFAULT_FOLDERS), Ok(default_list));

    // Trims, drops empties.
    assert_eq!(
        parse_rag_folders(" Projects , Daily ,, "),
        Ok(vec!["Projects".to_string(), "Daily".to_string()])
    );

    // Dedupes keeping first occurrence order.
    assert_eq!(
        parse_rag_folders("Daily,Projects,Daily, Projects"),
        Ok(vec!["Daily".to_string(), "Projects".to_string()])
    );

    // Rejected entries: separators, parent refs, NUL, leading dot, over 64 chars.
    let too_long = "x".repeat(65);
    for bad in [
        "Pro/jects",
        "Pro\\jects",
        "a..b",
        "..",
        "a\0b",
        ".hidden",
        too_long.as_str(),
    ] {
        assert!(
            parse_rag_folders(bad).is_err(),
            "entry must be rejected: {bad:?}"
        );
    }

    // 64 characters is the limit and is accepted.
    let longest = "x".repeat(64);
    assert_eq!(parse_rag_folders(&longest), Ok(vec![longest.clone()]));

    // At most 16 distinct entries.
    let sixteen: Vec<String> = (0..16).map(|i| format!("f{i}")).collect();
    assert_eq!(parse_rag_folders(&sixteen.join(",")), Ok(sixteen.clone()));
    let seventeen: Vec<String> = (0..17).map(|i| format!("f{i}")).collect();
    assert!(parse_rag_folders(&seventeen.join(",")).is_err());

    // Zero entries is an error with a fixed message.
    assert_eq!(
        parse_rag_folders(" , "),
        Err("choose at least one folder".to_string())
    );
    assert_eq!(
        parse_rag_folders(""),
        Err("choose at least one folder".to_string())
    );
}

/// Model routes need a supported provider; this picks one from the shipped defaults.
fn model_on(s: &mut AppSettings) {
    s.tts_composer_mode = TtsComposerMode::Model;
    s.tts_composer_model = "model-x".to_string();
    let supported = s
        .post_process_providers
        .iter()
        .find(|p| composer_provider_supported(p))
        .expect("defaults contain a supported provider")
        .id
        .clone();
    s.tts_composer_provider_id = supported;
}

#[derive(Debug, Clone, Copy)]
enum Route {
    /// Direct endpoint: realtime master and direct endpoint on.
    Direct,
    /// Model route with the stream route off.
    LegacyHttp,
    /// Model route with the stream route on.
    Stream,
    /// External command composer: no model call.
    Command,
    /// Composer off: no model call.
    Off,
    /// Model mode with a provider that is not supported (no HTTP route): no model call.
    OnDevice,
}

const ROUTES: [Route; 6] = [
    Route::Direct,
    Route::LegacyHttp,
    Route::Stream,
    Route::Command,
    Route::Off,
    Route::OnDevice,
];

fn route_settings(route: Route) -> AppSettings {
    let mut s = get_default_settings();
    match route {
        Route::Direct => {
            model_on(&mut s);
            s.exp_rt_direct_endpoint = true;
        }
        Route::LegacyHttp => model_on(&mut s),
        Route::Stream => {
            model_on(&mut s);
            s.exp_rt_stream_reply = true;
        }
        Route::Command => s.tts_composer_mode = TtsComposerMode::Command,
        Route::Off => s.tts_composer_mode = TtsComposerMode::Off,
        Route::OnDevice => {
            model_on(&mut s);
            s.tts_composer_provider_id = "on-device".to_string();
        }
    }
    s
}

#[test]
fn cfg_rag_possible_truth_table() {
    for realtime in [false, true] {
        for rag in [false, true] {
            for path_set in [false, true] {
                for route in ROUTES {
                    let mut s = route_settings(route);
                    s.exp_rt_enabled = realtime;
                    s.vault_rag_enabled = rag;
                    s.vault_path = if path_set {
                        "/tmp/vault".to_string()
                    } else {
                        "   ".to_string()
                    };
                    let http = matches!(route, Route::Direct | Route::LegacyHttp | Route::Stream);
                    let expected = realtime && rag && path_set && http;
                    assert_eq!(
                        rag_possible(&s),
                        expected,
                        "realtime={realtime} rag={rag} path_set={path_set} route={route:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn cfg_answer_only_for_includes_rag() {
    let mut s = get_default_settings();
    s.vault_rag_enabled = true;
    s.vault_path = "/tmp/vault".to_string();
    s.vault_context_enabled = false;
    s.agent_tools_guard_actions = false;
    assert!(
        answer_only_for(&s),
        "RAG sessions are answer-only even with guard_actions off"
    );

    s.vault_path = "   ".to_string();
    assert!(!answer_only_for(&s), "blank path is not a RAG session");
}
