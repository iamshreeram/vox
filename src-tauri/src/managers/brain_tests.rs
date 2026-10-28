//! P3a spec: the Brain connection (DESIGN.md Rev 3/4/5). Authoritative: do not weaken an assertion;
//! if a test is wrong, report it instead of editing it. Local servers only (127.0.0.1), no real network.
//!
//! Contract under test (module `crate::managers::brain`):
//!   enum BrainFeature { Composer, RealtimeStream, PostProcess }            (Copy, Debug, PartialEq, serde snake_case, specta Type)
//!   enum BrainMissing { Provider, BaseUrl, Model, Token, Consent }          (Debug, Clone, PartialEq, Eq)
//!   struct BrainConnection { provider_id, base_url, model, token: String }  (Debug must NOT print the token)
//!   enum Resolved { Own, Brain(BrainConnection), NotConfigured(BrainMissing) }
//!   fn resolve(&AppSettings, BrainFeature) -> Resolved
//!   struct EffectiveLlm { provider: PostProcessProvider, api_key: String, model: String }
//!   fn effective_provider(&AppSettings, BrainFeature) -> Result<EffectiveLlm, BrainMissing>
//!   fn destination(provider_id:&str, base_url:&str) -> String
//!   fn validate_base_url(&str) -> Result<(), BrainUrlError>; fn redact_url(&str) -> String
//!   fn missing_message(&BrainMissing) -> &'static str
//!   enum MigrationOutcome { Fresh, Migrated, AlreadyDone, BackupFailed }
//!   fn migrate(&mut AppSettings, store_existed: bool) -> MigrationOutcome          (pure: no backup)
//!   fn migrate_with_backup(&mut AppSettings, store_existed: bool, store_path:&Path, raw:&[u8]) -> MigrationOutcome
//!   fn write_backup_once(store_path:&Path, raw:&[u8]) -> std::io::Result<bool>
//!   fn apply_connection(&mut AppSettings, provider_id:&str, base_url:&str, model:&str) -> Result<(), String>
//!   fn use_brain_for(&mut AppSettings, BrainFeature) / fn use_own_for(&mut AppSettings, BrainFeature)
//!   fn ack_allows_network(&AppSettings) -> bool
//!   async fn run_test(&BrainConnection) -> BrainTestResult { ok, kind: BrainTestKind, latency_ms, model_count }
//!   async fn list_models(&BrainConnection) -> Result<Vec<String>, BrainTestKind>
//! AppSettings fields: brain_provider_id, brain_base_url, brain_model: String; brain_token: SecretString;
//!   brain_token_set, brain_schema_v1: bool; brain_source_composer/realtime/postprocess: BrainSource{Inherit,Own};
//!   brain_test_ack_host, brain_transcript_consent_dest: String.  AppSettings::redacted_for_ipc(&self) -> AppSettings.
use crate::managers::brain::*;
use crate::settings::{get_default_settings, AppSettings, BrainSource, SecretString};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const SENTINEL: &str = "sk-SENTINEL-1234567890";

fn src(rel: &str) -> String {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn configured() -> AppSettings {
    let mut s = get_default_settings();
    s.brain_provider_id = "custom".into();
    s.brain_base_url = "http://127.0.0.1:11434/v1".into();
    s.brain_model = "llama3".into();
    s.brain_token = SecretString::from(SENTINEL);
    s.brain_token_set = true;
    s.brain_transcript_consent_dest = destination("custom", "http://127.0.0.1:11434/v1");
    s
}

const ALL: [BrainFeature; 3] = [
    BrainFeature::Composer,
    BrainFeature::RealtimeStream,
    BrainFeature::PostProcess,
];

// ---------------------------------------------------------------- defaults and serde

#[test]
fn a_fresh_install_inherits_the_brain_everywhere() {
    let s = get_default_settings();
    assert!(s.brain_schema_v1);
    assert_eq!(s.brain_source_composer, BrainSource::Inherit);
    assert_eq!(s.brain_source_realtime, BrainSource::Inherit);
    assert_eq!(s.brain_source_postprocess, BrainSource::Inherit);
    assert!(
        s.brain_provider_id.is_empty() && s.brain_base_url.is_empty() && s.brain_model.is_empty()
    );
    assert!(s.brain_token.is_empty() && !s.brain_token_set);
    assert!(s.brain_test_ack_host.is_empty() && s.brain_transcript_consent_dest.is_empty());
}

#[test]
fn an_old_store_without_the_keys_loads_as_legacy_own() {
    let mut v = serde_json::to_value(get_default_settings()).unwrap();
    let obj = v.as_object_mut().unwrap();
    obj.retain(|k, _| !k.starts_with("brain_"));
    let s: AppSettings = serde_json::from_value(v).expect("old store still loads");
    assert!(!s.brain_schema_v1, "marker absent");
    assert_eq!(s.brain_source_composer, BrainSource::Own);
    assert_eq!(s.brain_source_realtime, BrainSource::Own);
    assert_eq!(s.brain_source_postprocess, BrainSource::Own);
}

#[test]
fn unknown_extra_keys_never_break_loading() {
    let mut v = serde_json::to_value(get_default_settings()).unwrap();
    v.as_object_mut()
        .unwrap()
        .insert("some_future_key".into(), serde_json::json!(1));
    assert!(serde_json::from_value::<AppSettings>(v).is_ok());
}

// ---------------------------------------------------------------- resolve

#[test]
fn own_sources_keep_the_legacy_path_even_when_the_brain_is_fully_set() {
    let mut s = configured();
    s.brain_source_composer = BrainSource::Own;
    s.brain_source_realtime = BrainSource::Own;
    s.brain_source_postprocess = BrainSource::Own;
    for f in ALL {
        assert_eq!(resolve(&s, f), Resolved::Own, "{f:?}");
    }
}

#[test]
fn without_the_schema_marker_everything_resolves_to_own() {
    let mut s = configured();
    s.brain_schema_v1 = false;
    for f in ALL {
        assert_eq!(resolve(&s, f), Resolved::Own, "{f:?}");
    }
}

#[test]
fn inherit_resolves_to_the_brain_connection() {
    let s = configured();
    for f in ALL {
        match resolve(&s, f) {
            Resolved::Brain(c) => {
                assert_eq!(c.provider_id, "custom");
                assert_eq!(c.base_url, "http://127.0.0.1:11434/v1");
                assert_eq!(c.model, "llama3");
                assert_eq!(c.token, SENTINEL);
            }
            other => panic!("{f:?}: {other:?}"),
        }
    }
}

#[test]
fn missing_pieces_are_reported_in_a_fixed_order() {
    let mut s = configured();
    s.brain_provider_id = "".into();
    assert_eq!(
        resolve(&s, BrainFeature::Composer),
        Resolved::NotConfigured(BrainMissing::Provider)
    );
    s.brain_provider_id = "anthropic".into(); // not a Brain preset
    assert_eq!(
        resolve(&s, BrainFeature::Composer),
        Resolved::NotConfigured(BrainMissing::Provider)
    );
    for bad in ["openai", "openrouter", "groq", "cerebras", "zai", "custom"] {
        s.brain_provider_id = bad.into();
        assert!(
            !matches!(
                resolve(&s, BrainFeature::Composer),
                Resolved::NotConfigured(BrainMissing::Provider)
            ),
            "{bad}"
        );
    }
    let mut s = configured();
    s.brain_base_url = "".into();
    assert_eq!(
        resolve(&s, BrainFeature::Composer),
        Resolved::NotConfigured(BrainMissing::BaseUrl)
    );
    s.brain_base_url = "ftp://x/v1".into();
    assert_eq!(
        resolve(&s, BrainFeature::Composer),
        Resolved::NotConfigured(BrainMissing::BaseUrl)
    );
    let mut s = configured();
    s.brain_model = "   ".into();
    assert_eq!(
        resolve(&s, BrainFeature::Composer),
        Resolved::NotConfigured(BrainMissing::Model)
    );
    // A remote host without a token is missing the token; loopback does not need one.
    let mut s = configured();
    s.brain_base_url = "https://api.example.com/v1".into();
    s.brain_token = SecretString::from("");
    s.brain_transcript_consent_dest = destination("custom", "https://api.example.com/v1");
    assert_eq!(
        resolve(&s, BrainFeature::Composer),
        Resolved::NotConfigured(BrainMissing::Token)
    );
    let mut s = configured();
    s.brain_token = SecretString::from("");
    assert!(
        matches!(resolve(&s, BrainFeature::Composer), Resolved::Brain(_)),
        "loopback needs no token"
    );
}

#[test]
fn transcript_consent_is_bound_to_the_exact_destination() {
    let mut s = configured();
    s.brain_transcript_consent_dest = "".into();
    for f in ALL {
        assert_eq!(
            resolve(&s, f),
            Resolved::NotConfigured(BrainMissing::Consent),
            "{f:?}"
        );
    }
    let mut s = configured();
    s.brain_transcript_consent_dest = destination("custom", "http://127.0.0.1:9999/v1");
    assert_eq!(
        resolve(&s, BrainFeature::Composer),
        Resolved::NotConfigured(BrainMissing::Consent)
    );
    // Own is exempt: no new gate for existing configurations.
    s.brain_source_composer = BrainSource::Own;
    assert_eq!(resolve(&s, BrainFeature::Composer), Resolved::Own);
}

#[test]
fn the_connection_debug_output_never_shows_the_token() {
    let s = configured();
    if let Resolved::Brain(c) = resolve(&s, BrainFeature::Composer) {
        assert!(!format!("{c:?}").contains(SENTINEL));
    } else {
        panic!("expected a connection");
    }
    assert!(!format!("{s:?}").contains(SENTINEL));
}

// ---------------------------------------------------------------- destination and urls

#[test]
fn destination_is_normalised() {
    let a = destination("custom", "HTTP://LocalHost:11434/v1/");
    let b = destination("custom", "http://localhost:11434/v1");
    assert_eq!(a, b);
    assert_ne!(
        destination("custom", "http://localhost:11434/v1"),
        destination("openai", "http://localhost:11434/v1")
    );
    assert_ne!(
        destination("custom", "http://localhost:11434/v1"),
        destination("custom", "http://localhost:11435/v1")
    );
    assert!(!destination("custom", "https://u:p@h.com/v1?key=abc").contains("abc"));
    assert!(!destination("custom", "https://u:p@h.com/v1?key=abc").contains("p@"));
}

#[test]
fn urls_are_validated() {
    for ok in [
        "https://api.openai.com/v1",
        "https://example.com:8443/v1",
        "http://localhost:11434/v1",
        "http://127.0.0.1:1234/v1",
        "http://[::1]:11434/v1",
    ] {
        assert!(validate_base_url(ok).is_ok(), "{ok}");
    }
    for (bad, why) in [
        ("", "empty"),
        ("   ", "blank"),
        ("ftp://x/v1", "scheme"),
        ("javascript:alert(1)", "scheme"),
        ("http://example.com/v1", "plain http to a remote host"),
        ("http://192.168.1.10:11434/v1", "plain http to a LAN host"),
        ("https://user:pw@example.com/v1", "userinfo"),
        ("http://user@127.0.0.1/v1", "userinfo even on loopback"),
        ("not a url", "garbage"),
    ] {
        assert!(validate_base_url(bad).is_err(), "{bad} ({why})");
    }
}

#[test]
fn redaction_removes_userinfo_and_query() {
    assert_eq!(
        redact_url("https://u:p@h.com/v1?key=abc#frag"),
        "https://h.com/v1"
    );
    assert_eq!(
        redact_url("http://127.0.0.1:11434/v1"),
        "http://127.0.0.1:11434/v1"
    );
    assert!(!redact_url("garbage ?key=abc").contains("abc"));
}

#[test]
fn every_missing_message_points_at_the_brain_page() {
    for m in [
        BrainMissing::Provider,
        BrainMissing::BaseUrl,
        BrainMissing::Model,
        BrainMissing::Token,
        BrainMissing::Consent,
    ] {
        assert!(missing_message(&m).contains("Brain page"), "{m:?}");
    }
    assert!(missing_message(&BrainMissing::Consent)
        .to_lowercase()
        .contains("send"));
}

// ---------------------------------------------------------------- migration

#[test]
fn a_fresh_store_initialises_inherit_and_the_marker() {
    let mut s = get_default_settings();
    s.brain_schema_v1 = false;
    s.brain_source_composer = BrainSource::Own;
    s.brain_source_realtime = BrainSource::Own;
    s.brain_source_postprocess = BrainSource::Own;
    assert_eq!(migrate(&mut s, false), MigrationOutcome::Fresh);
    assert!(s.brain_schema_v1);
    for src in [
        s.brain_source_composer,
        s.brain_source_realtime,
        s.brain_source_postprocess,
    ] {
        assert_eq!(src, BrainSource::Inherit);
    }
}

#[test]
fn an_existing_unmarked_store_becomes_own_and_brain_stays_unset() {
    for default_valued in [true, false] {
        let mut s = get_default_settings();
        s.brain_schema_v1 = false;
        s.brain_source_composer = BrainSource::Inherit; // even a stray Inherit must not survive
        if !default_valued {
            s.post_process_enabled = true;
            s.tts_composer_model = "gpt-x".into();
        }
        assert_eq!(migrate(&mut s, true), MigrationOutcome::Migrated);
        assert!(s.brain_schema_v1);
        for src in [
            s.brain_source_composer,
            s.brain_source_realtime,
            s.brain_source_postprocess,
        ] {
            assert_eq!(src, BrainSource::Own);
        }
        assert!(
            s.brain_provider_id.is_empty() && s.brain_model.is_empty(),
            "nothing is seeded"
        );
    }
}

#[test]
fn migration_is_idempotent_and_never_overwrites_edits() {
    let mut s = configured();
    s.brain_source_composer = BrainSource::Own;
    let before = serde_json::to_value(&s).unwrap();
    assert_eq!(migrate(&mut s, true), MigrationOutcome::AlreadyDone);
    assert_eq!(migrate(&mut s, false), MigrationOutcome::AlreadyDone);
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}

#[test]
fn the_backup_is_written_once_and_never_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join("settings_store.json");
    assert_eq!(write_backup_once(&store, b"first").unwrap(), true);
    assert_eq!(write_backup_once(&store, b"second").unwrap(), false);
    let backup = dir.path().join("settings_store.pre-brain-v1.json");
    assert_eq!(std::fs::read(&backup).unwrap(), b"first");
}

#[test]
fn a_failed_backup_aborts_the_migration_and_keeps_legacy_resolution() {
    let mut s = get_default_settings();
    s.brain_schema_v1 = false;
    s.brain_source_composer = BrainSource::Own;
    s.brain_source_realtime = BrainSource::Own;
    s.brain_source_postprocess = BrainSource::Own;
    let before = serde_json::to_value(&s).unwrap();
    let bad = PathBuf::from("/nonexistent-vox-dir-xyz/settings_store.json");
    assert_eq!(
        migrate_with_backup(&mut s, true, &bad, b"{}"),
        MigrationOutcome::BackupFailed
    );
    assert_eq!(
        serde_json::to_value(&s).unwrap(),
        before,
        "settings untouched"
    );
    for f in ALL {
        assert_eq!(resolve(&s, f), Resolved::Own);
    }
}

#[test]
fn a_fresh_store_needs_no_backup() {
    let mut s = get_default_settings();
    s.brain_schema_v1 = false;
    let bad = PathBuf::from("/nonexistent-vox-dir-xyz/settings_store.json");
    assert_eq!(
        migrate_with_backup(&mut s, false, &bad, b""),
        MigrationOutcome::Fresh
    );
}

#[test]
fn legacy_effective_configuration_is_identical_before_and_after_migration() {
    for flavour in ["default", "configured", "postprocess_anthropic"] {
        let mut s = get_default_settings();
        s.brain_schema_v1 = false;
        s.brain_source_composer = BrainSource::Own;
        s.brain_source_realtime = BrainSource::Own;
        s.brain_source_postprocess = BrainSource::Own;
        match flavour {
            "configured" => {
                s.tts_composer_provider_id = "openai".into();
                s.tts_composer_model = "gpt-x".into();
                s.post_process_api_keys
                    .insert("openai".into(), "sk-legacy".into());
                s.post_process_models
                    .insert("openai".into(), "gpt-y".into());
            }
            "postprocess_anthropic" => {
                s.post_process_provider_id = "anthropic".into();
                s.post_process_models
                    .insert("anthropic".into(), "claude-x".into());
                s.post_process_api_keys
                    .insert("anthropic".into(), "sk-ant".into());
            }
            _ => {}
        }
        let before: Vec<_> = ALL
            .iter()
            .map(|f| {
                effective_provider(&s, *f)
                    .map(|e| (e.provider.id, e.provider.base_url, e.api_key, e.model))
            })
            .collect();
        migrate(&mut s, true);
        let after: Vec<_> = ALL
            .iter()
            .map(|f| {
                effective_provider(&s, *f)
                    .map(|e| (e.provider.id, e.provider.base_url, e.api_key, e.model))
            })
            .collect();
        assert_eq!(before, after, "{flavour}");
    }
}

// ---------------------------------------------------------------- effective provider

#[test]
fn own_returns_exactly_what_the_legacy_readers_used() {
    let mut s = get_default_settings();
    s.brain_source_composer = BrainSource::Own;
    s.brain_source_realtime = BrainSource::Own;
    s.brain_source_postprocess = BrainSource::Own;
    s.tts_composer_provider_id = "openai".into();
    s.tts_composer_model = "gpt-composer".into();
    s.post_process_provider_id = "groq".into();
    s.post_process_api_keys
        .insert("openai".into(), "sk-openai".into());
    s.post_process_api_keys
        .insert("groq".into(), "sk-groq".into());
    s.post_process_models
        .insert("groq".into(), "llama-pp".into());
    for f in [BrainFeature::Composer, BrainFeature::RealtimeStream] {
        let e = effective_provider(&s, f).unwrap();
        assert_eq!(
            (e.provider.id.as_str(), e.api_key.as_str(), e.model.as_str()),
            ("openai", "sk-openai", "gpt-composer"),
            "{f:?}"
        );
    }
    let e = effective_provider(&s, BrainFeature::PostProcess).unwrap();
    assert_eq!(
        (e.provider.id.as_str(), e.api_key.as_str(), e.model.as_str()),
        ("groq", "sk-groq", "llama-pp")
    );
}

#[test]
fn inherit_returns_the_brain_and_never_touches_legacy_values() {
    let mut s = configured();
    s.post_process_api_keys
        .insert("custom".into(), "sk-legacy-custom".into());
    for f in ALL {
        let e = effective_provider(&s, f).unwrap();
        assert_eq!(e.provider.id, "custom");
        assert_eq!(e.provider.base_url, "http://127.0.0.1:11434/v1");
        assert_eq!(
            e.api_key, SENTINEL,
            "{f:?} uses the Brain token, not the legacy key"
        );
        assert_eq!(e.model, "llama3");
        assert!(!e.provider.allow_base_url_edit);
    }
}

#[test]
fn inherit_reports_what_is_missing() {
    let mut s = configured();
    s.brain_model = "".into();
    for f in ALL {
        assert_eq!(effective_provider(&s, f).err(), Some(BrainMissing::Model));
    }
}

#[test]
fn editing_the_brain_changes_only_inheriting_features() {
    let mut s = configured();
    s.brain_source_postprocess = BrainSource::Own;
    s.post_process_provider_id = "openai".into();
    s.post_process_models
        .insert("openai".into(), "gpt-pp".into());
    let own_before = effective_provider(&s, BrainFeature::PostProcess)
        .map(|e| (e.provider.id, e.api_key, e.model));
    let inherit_before = effective_provider(&s, BrainFeature::Composer).map(|e| e.model);
    apply_connection(&mut s, "custom", "http://127.0.0.1:11434/v1", "mistral").unwrap();
    s.brain_token = SecretString::from("sk-NEW");
    assert_eq!(
        effective_provider(&s, BrainFeature::PostProcess).map(|e| (
            e.provider.id,
            e.api_key,
            e.model
        )),
        own_before
    );
    assert_ne!(
        effective_provider(&s, BrainFeature::Composer).map(|e| e.model),
        inherit_before
    );
}

// ---------------------------------------------------------------- settings plumbing

#[test]
fn apply_connection_validates_and_resets_consent_on_a_new_destination() {
    let mut s = configured();
    s.brain_test_ack_host = destination("custom", "http://127.0.0.1:11434/v1");
    assert!(apply_connection(&mut s, "anthropic", "https://api.anthropic.com/v1", "m").is_err());
    assert!(apply_connection(&mut s, "custom", "http://example.com/v1", "m").is_err());
    assert!(apply_connection(&mut s, "custom", "https://u:p@example.com/v1", "m").is_err());
    assert_eq!(
        s.brain_base_url, "http://127.0.0.1:11434/v1",
        "a rejected call changes nothing"
    );
    // Model-only change keeps the consent.
    apply_connection(&mut s, "custom", "http://127.0.0.1:11434/v1/", " other ").unwrap();
    assert_eq!(s.brain_model, "other");
    assert!(!s.brain_transcript_consent_dest.is_empty());
    assert!(!s.brain_test_ack_host.is_empty());
    // A new destination invalidates both.
    apply_connection(&mut s, "custom", "http://127.0.0.1:9999/v1", "other").unwrap();
    assert!(s.brain_transcript_consent_dest.is_empty());
    assert!(s.brain_test_ack_host.is_empty());
}

#[test]
fn use_brain_and_use_own_flip_one_feature_atomically() {
    let mut s = configured();
    s.brain_transcript_consent_dest = "".into();
    s.brain_source_composer = BrainSource::Own;
    s.brain_source_postprocess = BrainSource::Own;
    s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Off;
    use_brain_for(&mut s, BrainFeature::Composer);
    assert_eq!(s.brain_source_composer, BrainSource::Inherit);
    assert_eq!(
        s.brain_source_postprocess,
        BrainSource::Own,
        "other features untouched"
    );
    assert_eq!(
        s.brain_transcript_consent_dest,
        destination("custom", "http://127.0.0.1:11434/v1")
    );
    assert_eq!(
        s.tts_composer_mode,
        crate::managers::composer_config::TtsComposerMode::Model,
        "Off becomes Model"
    );
    use_own_for(&mut s, BrainFeature::Composer);
    assert_eq!(s.brain_source_composer, BrainSource::Own);
    // Command mode is never overridden by use_brain_for.
    let mut c = configured();
    c.brain_source_composer = BrainSource::Own;
    c.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Command;
    use_brain_for(&mut c, BrainFeature::Composer);
    assert_eq!(
        c.tts_composer_mode,
        crate::managers::composer_config::TtsComposerMode::Command
    );
}

#[test]
fn the_network_ack_is_bound_to_the_destination() {
    let mut s = configured();
    assert!(!ack_allows_network(&s));
    s.brain_test_ack_host = destination("custom", "http://127.0.0.1:11434/v1");
    assert!(ack_allows_network(&s));
    s.brain_base_url = "http://127.0.0.1:2/v1".into();
    assert!(!ack_allows_network(&s));
}

#[test]
fn ipc_copies_never_carry_the_token_but_the_store_does() {
    let s = configured();
    assert!(
        serde_json::to_string(&s).unwrap().contains(SENTINEL),
        "the store keeps it"
    );
    let ipc = s.redacted_for_ipc();
    assert!(!serde_json::to_string(&ipc).unwrap().contains(SENTINEL));
    assert!(
        ipc.brain_token_set,
        "the UI can still see that a token is set"
    );
}

#[test]
fn stale_whole_object_writes_cannot_change_any_brain_field() {
    let committed = configured();
    let mut stale = get_default_settings();
    stale.brain_provider_id = "openai".into();
    stale.brain_base_url = "https://evil.example/v1".into();
    stale.brain_model = "x".into();
    stale.brain_token = SecretString::from("sk-stale");
    stale.brain_token_set = false;
    stale.brain_source_composer = BrainSource::Own;
    stale.brain_source_realtime = BrainSource::Own;
    stale.brain_source_postprocess = BrainSource::Own;
    stale.brain_schema_v1 = false;
    stale.brain_test_ack_host = "x".into();
    stale.brain_transcript_consent_dest = "y".into();
    let merged = crate::settings::preserve_launch_fields(stale, &committed);
    assert_eq!(
        serde_json::to_value(&merged)
            .unwrap()
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with("brain_"))
            .count(),
        serde_json::to_value(&committed)
            .unwrap()
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with("brain_"))
            .count()
    );
    let (m, c) = (
        serde_json::to_value(&merged).unwrap(),
        serde_json::to_value(&committed).unwrap(),
    );
    for (k, v) in c.as_object().unwrap() {
        if k.starts_with("brain_") {
            assert_eq!(&m[k], v, "{k} must come from the committed store");
        }
    }
}

// ---------------------------------------------------------------- network (mock servers)

#[derive(Default, Clone)]
struct Seen(Arc<Mutex<Vec<String>>>);

fn serve(
    status_line: &'static str,
    extra_headers: &'static str,
    body: &'static str,
    seen: Seen,
) -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in l.incoming() {
            let Ok(mut st) = stream else { break };
            st.set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .ok();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = st.read(&mut chunk).unwrap_or(0);
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
                if let Some(h) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..h]).to_lowercase();
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if buf.len() >= h + 4 + len {
                        break;
                    }
                }
            }
            seen.0
                .lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf).to_string());
            let resp = format!(
                "{status_line}\r\n{extra_headers}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = st.write_all(resp.as_bytes());
        }
    });
    port
}

fn conn(port: u16) -> BrainConnection {
    BrainConnection {
        provider_id: "custom".into(),
        base_url: format!("http://127.0.0.1:{port}/v1"),
        model: "llama3".into(),
        token: SENTINEL.into(),
    }
}

#[tokio::test]
async fn the_test_sends_one_fixed_prompt_with_the_token_to_the_disclosed_host_only() {
    let seen = Seen::default();
    let port = serve(
        "HTTP/1.1 200 OK",
        "",
        r#"{"choices":[{"message":{"role":"assistant","content":"OK"}}]}"#,
        seen.clone(),
    );
    let r = run_test(&conn(port)).await;
    assert!(r.ok, "{r:?}");
    assert_eq!(r.kind, BrainTestKind::Ok);
    let reqs = seen.0.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1, "exactly one request");
    let req = &reqs[0];
    assert!(req.starts_with("POST /v1/chat/completions "), "{req}");
    assert!(req.to_lowercase().contains(&format!(
        "authorization: bearer {}",
        SENTINEL.to_lowercase()
    )));
    assert!(req.contains("Reply with OK."), "fixed synthetic prompt");
    assert!(req.contains("\"model\":\"llama3\""));
    // Nothing else leaves: one user message, no history or transcript fields.
    assert_eq!(req.matches("\"role\"").count(), 1, "single message: {req}");
    assert!(!format!("{r:?}").contains(SENTINEL));
}

#[tokio::test]
async fn redirects_are_an_error_and_never_forward_the_token() {
    let elsewhere = Seen::default();
    let other = serve("HTTP/1.1 200 OK", "", "{}", elsewhere.clone());
    let location: &'static str = Box::leak(
        format!("Location: http://127.0.0.1:{other}/v1/chat/completions\r\n").into_boxed_str(),
    );
    let seen = Seen::default();
    let port = serve("HTTP/1.1 302 Found", location, "", seen.clone());
    let r = run_test(&conn(port)).await;
    assert!(!r.ok);
    assert_eq!(r.kind, BrainTestKind::Redirect);
    assert_eq!(
        elsewhere.0.lock().unwrap().len(),
        0,
        "the redirect target is never contacted"
    );
}

#[tokio::test]
async fn http_failures_are_classified_without_leaking_details() {
    for (line, kind) in [
        ("HTTP/1.1 401 Unauthorized", BrainTestKind::Unauthorized),
        ("HTTP/1.1 403 Forbidden", BrainTestKind::Unauthorized),
        ("HTTP/1.1 404 Not Found", BrainTestKind::NotFound),
    ] {
        let port = serve(
            line,
            "",
            r#"{"error":"sk-SENTINEL-1234567890 echoed"}"#,
            Seen::default(),
        );
        let r = run_test(&conn(port)).await;
        assert!(!r.ok);
        assert_eq!(r.kind, kind, "{line}");
        assert!(
            !format!("{r:?}").contains("SENTINEL"),
            "server text is never echoed"
        );
    }
    let port = serve("HTTP/1.1 200 OK", "", "not json", Seen::default());
    assert_eq!(run_test(&conn(port)).await.kind, BrainTestKind::BadResponse);
}

#[tokio::test]
async fn a_closed_port_is_unreachable() {
    let port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let r = run_test(&conn(port)).await;
    assert!(!r.ok);
    assert_eq!(r.kind, BrainTestKind::Unreachable);
}

#[tokio::test]
async fn remote_plain_http_is_refused_before_any_connection() {
    let mut c = conn(1);
    c.base_url = "http://203.0.113.9/v1".into();
    let started = std::time::Instant::now();
    let r = run_test(&c).await;
    assert!(!r.ok);
    assert_eq!(r.kind, BrainTestKind::InvalidUrl);
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "no connection attempt"
    );
    assert_eq!(list_models(&c).await.err(), Some(BrainTestKind::InvalidUrl));
}

#[tokio::test]
async fn list_models_parses_the_openai_shape_and_sends_the_token() {
    let seen = Seen::default();
    let port = serve(
        "HTTP/1.1 200 OK",
        "",
        r#"{"data":[{"id":"llama3"},{"id":"mistral"},{"id":"llama3"}]}"#,
        seen.clone(),
    );
    let models = list_models(&conn(port)).await.unwrap();
    assert_eq!(
        models,
        vec!["llama3".to_string(), "mistral".to_string()],
        "deduplicated, order kept"
    );
    let req = seen.0.lock().unwrap()[0].clone();
    assert!(req.starts_with("GET /v1/models "));
    assert!(req.to_lowercase().contains(&format!(
        "authorization: bearer {}",
        SENTINEL.to_lowercase()
    )));
}

#[tokio::test]
async fn an_empty_token_sends_no_authorization_header() {
    let seen = Seen::default();
    let port = serve("HTTP/1.1 200 OK", "", r#"{"data":[]}"#, seen.clone());
    let mut c = conn(port);
    c.token = String::new();
    assert_eq!(list_models(&c).await.unwrap(), Vec::<String>::new());
    assert!(!seen.0.lock().unwrap()[0]
        .to_lowercase()
        .contains("authorization:"));
}

// ---------------------------------------------------------------- wiring (source level)

#[test]
fn the_three_readers_resolve_through_the_brain_module() {
    for (file, needle) in [
        (
            "src/managers/speech_backends.rs",
            "brain::effective_provider",
        ),
        (
            "src/managers/realtime_complete.rs",
            "brain::effective_provider",
        ),
        ("src/actions.rs", "brain::effective_provider"),
    ] {
        assert!(src(file).contains(needle), "{file} must call {needle}");
    }
}

#[test]
fn composer_whitelist_accepts_every_brain_preset() {
    let cfg = src("src/managers/composer_config.rs");
    for id in ["openai", "openrouter", "groq", "cerebras", "zai", "custom"] {
        assert!(cfg.contains(&format!("\"{id}\"")), "{id}");
    }
}

#[test]
fn settings_load_runs_the_migration_with_a_backup() {
    let s = src("src/settings.rs");
    assert!(
        s.contains("migrate_with_backup"),
        "get_settings must call brain::migrate_with_backup"
    );
    assert!(s.contains("brain_"), "fields exist");
}

#[test]
fn commands_are_registered_main_window_only() {
    let lib = src("src/lib.rs");
    for name in [
        "change_brain_connection_setting",
        "set_brain_token",
        "brain_acknowledge_test",
        "brain_use_for_feature",
        "brain_test_connection",
        "brain_fetch_models",
    ] {
        assert!(lib.contains(name), "{name} registered in collect_commands!");
    }
    let chat_caps = src("capabilities/chat.json");
    assert!(
        !chat_caps.contains("brain"),
        "the chat window never gets brain commands"
    );
}

#[test]
fn the_test_and_fetch_commands_require_the_acknowledgement() {
    let cmds = src("src/commands/brain.rs");
    assert!(cmds.contains("ack_allows_network"));
    assert!(
        !cmds.contains("brain_token.as_str()") || cmds.contains("redact"),
        "tokens never go into messages"
    );
}

// ---------------------------------------------------------------- preflight

#[test]
fn preflight_names_the_missing_brain_piece() {
    use crate::managers::converse_platform::{preflight_check, preflight_text_check};
    let mut s = configured();
    s.tts_enabled = true;
    s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Model;
    s.brain_model = "".into();
    for r in [preflight_check(&s), preflight_text_check(&s)] {
        let e = r.expect_err("empty Brain model must block the conversation");
        assert!(e.contains("Brain page"), "{e}");
    }
    let mut ok = configured();
    ok.tts_enabled = true;
    ok.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Model;
    assert!(preflight_check(&ok).is_ok());
    let mut no_consent = ok.clone();
    no_consent.brain_transcript_consent_dest = "".into();
    let e = preflight_check(&no_consent).unwrap_err();
    assert!(
        e.contains("Brain page") && e.to_lowercase().contains("send"),
        "{e}"
    );
}

#[test]
fn preflight_catches_an_empty_own_model_instead_of_failing_silently_later() {
    use crate::managers::converse_platform::preflight_check;
    let mut s = get_default_settings();
    s.brain_source_composer = BrainSource::Own;
    s.tts_enabled = true;
    s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Model;
    s.tts_composer_model = "   ".into();
    let e = preflight_check(&s).expect_err("empty model with a model-mode composer");
    assert!(e.to_lowercase().contains("model"), "{e}");
    // Legacy behaviour otherwise unchanged: Off still reports the reply backend.
    s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Off;
    assert!(preflight_check(&s).unwrap_err().contains("reply backend"));
}
