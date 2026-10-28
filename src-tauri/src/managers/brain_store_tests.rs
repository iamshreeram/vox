//! P3a corrections spec (validator round 1: store path, salvage and stale-write merge).
//! Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::settings`, `pub(crate)`):
//!   fn absolute_store_path(app_data_dir: &Path, relative: &Path) -> PathBuf
//!       relative joined under app_data_dir; an already-absolute path is returned unchanged.
//!   fn existing_store_defaults() -> AppSettings      brain_schema_v1 == false, all three sources Own
//!   fn salvage_settings(&serde_json::Value) -> AppSettings
//!   fn merge_for_write(incoming: AppSettings, committed: &AppSettings) -> AppSettings
//!       committed.brain_schema_v1  -> preserve_launch_fields(incoming, committed) (every brain_* from committed)
//!       otherwise                  -> same, EXCEPT brain_schema_v1 and the three brain_source_* are taken from
//!                                     incoming when incoming.brain_schema_v1 is true; every other brain_* field
//!                                     always comes from committed.
use crate::settings::{
    absolute_store_path, existing_store_defaults, get_default_settings, merge_for_write,
    salvage_settings, AppSettings, BrainSource, SecretString,
};
use std::path::{Path, PathBuf};

fn brain_fields(s: &AppSettings) -> serde_json::Map<String, serde_json::Value> {
    serde_json::to_value(s)
        .unwrap()
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, _)| k.starts_with("brain_"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

fn configured_committed(marker: bool) -> AppSettings {
    let mut c = get_default_settings();
    c.brain_provider_id = "custom".into();
    c.brain_base_url = "http://127.0.0.1:11434/v1".into();
    c.brain_model = "llama3".into();
    c.brain_token = SecretString::from("sk-committed");
    c.brain_token_set = true;
    c.brain_test_ack_host = "ack".into();
    c.brain_transcript_consent_dest = "consent".into();
    c.brain_schema_v1 = marker;
    c
}

fn stale() -> AppSettings {
    let mut s = get_default_settings();
    s.brain_provider_id = "openai".into();
    s.brain_base_url = "https://stale.example/v1".into();
    s.brain_model = "stale".into();
    s.brain_token = SecretString::from("sk-stale");
    s.brain_token_set = false;
    s.brain_test_ack_host = "".into();
    s.brain_transcript_consent_dest = "".into();
    s
}

#[test]
fn a_relative_store_path_is_resolved_under_the_app_data_dir() {
    let dir = Path::new("/Users/x/Library/Application Support/com.example.vox");
    assert_eq!(
        absolute_store_path(dir, Path::new("settings_store.json")),
        dir.join("settings_store.json")
    );
    assert_eq!(
        absolute_store_path(dir, Path::new("/abs/portable/settings_store.json")),
        PathBuf::from("/abs/portable/settings_store.json")
    );
    assert!(absolute_store_path(dir, Path::new("settings_store.json")).is_absolute());
}

#[test]
fn the_migration_reads_and_backs_up_the_absolute_store_path() {
    let src =
        std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/settings.rs"))
            .unwrap();
    assert!(
        src.contains("absolute_store_path("),
        "get_settings must resolve the store file"
    );
    assert!(
        !src.contains("std::fs::read(store_path"),
        "never read the relative store path against the process working directory"
    );
}

#[test]
fn a_salvaged_old_store_is_legacy_own_not_fresh() {
    let d = existing_store_defaults();
    assert!(!d.brain_schema_v1);
    for s in [
        d.brain_source_composer,
        d.brain_source_realtime,
        d.brain_source_postprocess,
    ] {
        assert_eq!(s, BrainSource::Own);
    }
    // Old store (no brain keys) with one field of the wrong type: salvage must not mark it migrated.
    let mut v = serde_json::to_value(get_default_settings()).unwrap();
    let obj = v.as_object_mut().unwrap();
    obj.retain(|k, _| !k.starts_with("brain_"));
    let bool_key = obj
        .iter()
        .find(|(_, val)| val.is_boolean())
        .map(|(k, _)| k.clone())
        .expect("some boolean setting");
    obj.insert(bool_key, serde_json::json!("not a bool"));
    assert!(
        serde_json::from_value::<AppSettings>(v.clone()).is_err(),
        "the fixture really is unparsable"
    );
    let salvaged = salvage_settings(&v);
    assert!(
        !salvaged.brain_schema_v1,
        "the migration still has to run for this store"
    );
    for s in [
        salvaged.brain_source_composer,
        salvaged.brain_source_realtime,
        salvaged.brain_source_postprocess,
    ] {
        assert_eq!(s, BrainSource::Own);
    }
}

#[test]
fn a_marked_store_keeps_every_brain_field_against_a_stale_write() {
    let committed = configured_committed(true);
    let mut incoming = stale();
    incoming.brain_schema_v1 = false;
    incoming.brain_source_composer = BrainSource::Own;
    incoming.brain_source_realtime = BrainSource::Own;
    incoming.brain_source_postprocess = BrainSource::Own;
    let merged = merge_for_write(incoming, &committed);
    assert_eq!(brain_fields(&merged), brain_fields(&committed));
}

#[test]
fn an_unmarked_store_only_accepts_the_marker_and_sources_from_the_migration() {
    let committed = configured_committed(false);
    // A stale write that is itself unmarked changes nothing.
    let mut incoming = stale();
    incoming.brain_schema_v1 = false;
    incoming.brain_source_composer = BrainSource::Inherit;
    let merged = merge_for_write(incoming, &committed);
    assert_eq!(brain_fields(&merged), brain_fields(&committed));
    // The migration write carries the marker and the sources; nothing else of incoming is taken.
    let mut mig = stale();
    mig.brain_schema_v1 = true;
    mig.brain_source_composer = BrainSource::Own;
    mig.brain_source_realtime = BrainSource::Own;
    mig.brain_source_postprocess = BrainSource::Own;
    let merged = merge_for_write(mig, &committed);
    assert!(merged.brain_schema_v1);
    assert_eq!(merged.brain_source_composer, BrainSource::Own);
    assert_eq!(merged.brain_source_realtime, BrainSource::Own);
    assert_eq!(merged.brain_source_postprocess, BrainSource::Own);
    assert_eq!(merged.brain_token.as_str(), "sk-committed");
    assert_eq!(merged.brain_base_url, committed.brain_base_url);
    assert_eq!(merged.brain_provider_id, committed.brain_provider_id);
    assert_eq!(merged.brain_model, committed.brain_model);
    assert_eq!(merged.brain_test_ack_host, "ack");
    assert_eq!(merged.brain_transcript_consent_dest, "consent");
    assert!(merged.brain_token_set);
}
