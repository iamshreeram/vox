//! T2 command tests (DESIGN Rev 5, 3.6): the pure inner functions behind the vault RAG commands,
//! and the generated TypeScript contract. Step S5: written first; the inner functions and the
//! bindings are implemented from S6 on.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use specta_typescript::{BigIntExportBehavior, Typescript};

use crate::commands::vault_rag::{
    change_rag_enabled_inner, change_rag_folders_inner, change_vault_path_inner, preview_inner,
    reindex_inner,
};
use crate::managers::vault_index::{Hit, IndexStatus, RagError, SearchOpts};
use crate::managers::vault_rag::BlockingGate;
use crate::managers::vault_rag_runtime::{IndexBackend, VaultRagRuntime};
use crate::settings::{get_default_settings, AppSettings};

const ALPHA: &str = "/vault/alpha";
const WAIT: Duration = Duration::from_secs(5);

/// Records calls; serves nothing. Enough to observe what the commands asked of the runtime.
#[derive(Default)]
struct RecordingIndex {
    generation: AtomicU64,
    configures: Mutex<Vec<String>>,
    refreshes: AtomicUsize,
    searches: AtomicUsize,
}

impl RecordingIndex {
    fn configured(&self, root: &str) -> bool {
        self.configures.lock().unwrap().iter().any(|c| c == root)
    }
}

impl IndexBackend for RecordingIndex {
    fn configure(&self, root: Option<PathBuf>, folders: Vec<String>) -> u64 {
        let _ = folders;
        let name = root.map(|p| p.display().to_string()).unwrap_or_default();
        self.configures.lock().unwrap().push(name);
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn request_refresh(&self, budget: Duration) {
        let _ = budget;
        self.refreshes.fetch_add(1, Ordering::SeqCst);
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn status(&self) -> IndexStatus {
        IndexStatus {
            notes: 0,
            chunks: 0,
            skipped: 0,
            sensitive_names: 0,
            last_refresh_ms: None,
            refreshing: false,
            last_error_kind: None,
        }
    }

    fn search(&self, gen: u64, query: &str, opts: &SearchOpts) -> Result<Vec<Hit>, RagError> {
        let _ = (gen, query, opts);
        self.searches.fetch_add(1, Ordering::SeqCst);
        Err(RagError::Unavailable)
    }
}

fn live() -> (Arc<RecordingIndex>, Arc<VaultRagRuntime>) {
    let index = Arc::new(RecordingIndex::default());
    let backend: Arc<dyn IndexBackend> = index.clone();
    (index, VaultRagRuntime::new(backend, BlockingGate::new(2)))
}

fn rag_settings(root: &str) -> AppSettings {
    let mut s = get_default_settings();
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.vault_rag_enabled = true;
    s.vault_path = root.to_string();
    s
}

#[test]
fn cmd_change_setting_applies_runtime() {
    let (index, rt) = live();
    let mut s = rag_settings(ALPHA);
    s.vault_rag_enabled = false;
    let before = rt.revision();

    change_rag_enabled_inner(&mut s, Some(&*rt), true).expect("enabling succeeds");
    assert!(s.vault_rag_enabled);
    assert_eq!(rt.revision(), before + 1, "enabling applies exactly once");
    let deadline = Instant::now() + WAIT;
    while !index.configured(ALPHA) {
        assert!(
            Instant::now() < deadline,
            "configure runs with the new snapshot"
        );
        thread::sleep(Duration::from_millis(5));
    }
    rt.shutdown();
}

#[test]
fn cmd_change_folders_parse_error_persists_nothing() {
    let (_index, rt) = live();
    let mut s = rag_settings(ALPHA);
    s.vault_rag_folders = "Projects".to_string();
    let before = rt.revision();

    let rejected = change_rag_folders_inner(&mut s, Some(&*rt), "Pro/jects");
    assert!(rejected.is_err(), "a bad entry is an error");
    assert_eq!(
        s.vault_rag_folders, "Projects",
        "nothing is stored on error"
    );
    assert_eq!(rt.revision(), before, "nothing is applied on error");

    change_rag_folders_inner(&mut s, Some(&*rt), "Daily").expect("valid folders are accepted");
    assert_eq!(s.vault_rag_folders, "Daily");
    assert_eq!(rt.revision(), before + 1);
    rt.shutdown();
}

#[test]
fn cmd_change_vault_path_calls_runtime_apply() {
    let (_index, rt) = live();
    let mut s = rag_settings(ALPHA);
    let before = rt.revision();

    change_vault_path_inner(&mut s, Some(&*rt), "  /vault/bravo  ").expect("valid path");
    assert_eq!(s.vault_path, "/vault/bravo", "the path is trimmed");
    assert_eq!(
        rt.revision(),
        before + 1,
        "a path change applies exactly once"
    );

    assert!(change_vault_path_inner(&mut s, Some(&*rt), "bad\0path").is_err());
    assert_eq!(s.vault_path, "/vault/bravo");
    assert_eq!(rt.revision(), before + 1, "a rejected path applies nothing");
    rt.shutdown();
}

#[test]
fn cmd_reindex_err_when_off() {
    let (index, rt) = live();
    let mut s = rag_settings(ALPHA);
    s.vault_rag_enabled = false;
    let result = reindex_inner(&s, Some(&*rt));
    assert_eq!(result, Err("vault search is not enabled".to_string()));
    thread::sleep(Duration::from_millis(100));
    assert_eq!(
        index.refreshes.load(Ordering::SeqCst),
        0,
        "no refresh when off"
    );
    rt.shutdown();
}

#[test]
fn cmd_preview_requires_enabled() {
    let (index, rt) = live();
    let mut s = rag_settings(ALPHA);
    s.vault_rag_enabled = false;
    let result = preview_inner(&s, Some(&*rt), "status");
    assert!(result.is_err(), "preview requires enabled");
    assert_eq!(
        index.searches.load(Ordering::SeqCst),
        0,
        "no index call when off"
    );
    rt.shutdown();
}

/// Returns the body of `export type NAME = { ... }` in generated TypeScript.
fn type_block<'a>(ts: &'a str, name: &str) -> &'a str {
    let head = format!("export type {name} = {{");
    let start = ts
        .find(&head)
        .unwrap_or_else(|| panic!("bindings lack `export type {name}`"));
    let rest = &ts[start..];
    &rest[..rest.find('}').expect("type body closes")]
}

#[test]
fn cmd_bindings_contract_for_vault_rag() {
    let path =
        std::env::temp_dir().join(format!("vox-vault-rag-bindings-{}.ts", std::process::id()));
    crate::specta_builder()
        .export(
            Typescript::default().bigint(BigIntExportBehavior::Number),
            path.to_str().expect("utf-8 temp path"),
        )
        .expect("bindings export");
    let ts = std::fs::read_to_string(&path).expect("read exported bindings");
    let _ = std::fs::remove_file(&path);

    let status = type_block(&ts, "VaultIndexStatusDto");
    for field in [
        "notes",
        "chunks",
        "skipped",
        "last_refresh_ms",
        "refreshing",
        "last_error_kind",
        "tool_support",
    ] {
        assert!(status.contains(field), "VaultIndexStatusDto lacks {field}");
    }
    let hit = type_block(&ts, "VaultHitDto");
    for field in ["rel", "heading", "snippet", "note_date"] {
        assert!(hit.contains(field), "VaultHitDto lacks {field}");
    }

    let commands = [
        ("change_vault_rag_setting", Some("enabled")),
        ("change_vault_rag_folders_setting", Some("folders")),
        ("vault_index_status", None),
        ("vault_reindex", None),
        ("vault_search_preview", Some("query")),
    ];
    for (name, args) in commands {
        let needle = match args {
            Some(arg) => format!("TAURI_INVOKE(\"{name}\", {{ {arg} }})"),
            None => format!("TAURI_INVOKE(\"{name}\")"),
        };
        assert!(ts.contains(&needle), "bindings lack `{needle}`");
    }
}

// Setter rules when the runtime failed to initialise (DESIGN Rev 5, 3.6; lead decision in S6).

#[test]
fn setter_enable_without_runtime_is_refused_and_not_stored() {
    let mut s = rag_settings(ALPHA);
    s.vault_rag_enabled = false;
    let refused = change_rag_enabled_inner(&mut s, None, true);
    assert_eq!(refused, Err("vault search is unavailable".to_string()));
    assert!(!s.vault_rag_enabled, "nothing is persisted on refusal");
}

#[test]
fn setter_disable_without_runtime_persists_and_succeeds() {
    let mut s = rag_settings(ALPHA);
    assert!(s.vault_rag_enabled);
    change_rag_enabled_inner(&mut s, None, false).expect("disabling always succeeds");
    assert!(!s.vault_rag_enabled);
}

#[test]
fn setter_folders_without_runtime_is_refused_and_not_stored() {
    let mut s = rag_settings(ALPHA);
    s.vault_rag_folders = "Projects".to_string();
    let refused = change_rag_folders_inner(&mut s, None, "Daily");
    assert_eq!(refused, Err("vault search is unavailable".to_string()));
    assert_eq!(s.vault_rag_folders, "Projects", "nothing is persisted");
}

#[test]
fn setter_folder_parse_error_without_runtime_is_not_stored() {
    let mut s = rag_settings(ALPHA);
    s.vault_rag_folders = "Projects".to_string();
    assert!(change_rag_folders_inner(&mut s, None, "Pro/jects").is_err());
    assert_eq!(s.vault_rag_folders, "Projects");
}

#[test]
fn setter_vault_path_without_runtime_persists_as_before() {
    let mut s = rag_settings(ALPHA);
    change_vault_path_inner(&mut s, None, " /vault/charlie ")
        .expect("the M1 path setter keeps working without a runtime");
    assert_eq!(s.vault_path, "/vault/charlie");
}
