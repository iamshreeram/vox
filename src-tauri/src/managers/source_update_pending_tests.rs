use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

use super::*;

const RUN: &str = "0123456789abcdef";
const INSTALL: &str = "fedcba9876543210";
const TARGET: &str = "0123456789abcdef0123456789abcdef01234567";
const SHA: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

fn state(dir: &Path) -> PathBuf {
    dir.to_path_buf()
}

fn built(dir: &Path) -> PendingRun {
    PendingRun {
        run_id: RUN.to_string(),
        target_full: TARGET.to_string(),
        staged_app: staged_app_path(dir, RUN),
        exe_name: "vox-bin".to_string(),
        exe_sha256: SHA.to_string(),
        info_plist_sha256: SHA.to_string(),
        origin: PendingOrigin::Auto,
        ts: 1_700_000_000,
        state: PendingState::Built,
    }
}

fn built_json(dir: &Path) -> serde_json::Value {
    serde_json::json!({
        "v": 1,
        "run_id": RUN,
        "target_full": TARGET,
        "staged_app": staged_app_path(dir, RUN).to_str().unwrap(),
        "exe_name": "vox-bin",
        "exe_sha256": SHA,
        "info_plist_sha256": SHA,
        "state": "built",
        "origin": "auto",
        "ts": 1_700_000_000u64
    })
}

fn committing_json(dir: &Path) -> serde_json::Value {
    let mut value = built_json(dir);
    let map = value.as_object_mut().unwrap();
    map.insert("state".into(), "committing".into());
    map.insert("install_run_id".into(), INSTALL.into());
    map.insert(
        "install_label".into(),
        format!("com.iamshreeram.vox.update.{INSTALL}").into(),
    );
    map.insert(
        "install_plist".into(),
        plist_path(dir, INSTALL).to_str().unwrap().into(),
    );
    map.insert("committing_ts".into(), 1_700_000_100u64.into());
    value
}

fn bytes_of(value: &serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(value).unwrap()
}

fn with(mut value: serde_json::Value, key: &str, new: serde_json::Value) -> Vec<u8> {
    value.as_object_mut().unwrap().insert(key.into(), new);
    bytes_of(&value)
}

fn parse_err(bytes: &[u8], dir: &Path) -> String {
    match parse_pending(bytes, dir) {
        Ok(_) => panic!("expected rejection"),
        Err(error) => format!("{error:?}"),
    }
}

#[test]
fn valid_built_record_parses() {
    let dir = tempfile::tempdir().unwrap();
    let run = parse_pending(&bytes_of(&built_json(dir.path())), dir.path()).unwrap();
    assert_eq!(run, built(dir.path()));
}

#[test]
fn valid_committing_record_parses_with_commit_info() {
    let dir = tempfile::tempdir().unwrap();
    let run = parse_pending(&bytes_of(&committing_json(dir.path())), dir.path()).unwrap();
    match run.state {
        PendingState::Committing(info) => {
            assert_eq!(info.install_run_id, INSTALL);
            assert_eq!(info.committing_ts, 1_700_000_100);
            assert_eq!(info.install_plist, plist_path(dir.path(), INSTALL));
        }
        PendingState::Built => panic!("expected committing"),
    }
}

#[test]
fn encode_then_parse_round_trips_both_states() {
    let dir = tempfile::tempdir().unwrap();
    let b = built(dir.path());
    let bytes = encode_pending(&b, dir.path()).unwrap();
    assert_eq!(parse_pending(&bytes, dir.path()).unwrap(), b);
    let c = PendingRun {
        state: PendingState::Committing(CommitInfo {
            install_run_id: INSTALL.into(),
            install_label: job_label(INSTALL),
            install_plist: plist_path(dir.path(), INSTALL),
            committing_ts: 5,
        }),
        ..b
    };
    let bytes = encode_pending(&c, dir.path()).unwrap();
    assert_eq!(parse_pending(&bytes, dir.path()).unwrap(), c);
}

#[test]
fn rejects_oversize_bad_utf8_bad_json_and_non_object() {
    let dir = tempfile::tempdir().unwrap();
    let big = vec![b' '; PENDING_MAX_BYTES + 1];
    assert!(parse_err(&big, dir.path()).contains("TooLarge"));
    assert!(parse_err(&[0xff, 0xfe], dir.path()).contains("InvalidUtf8"));
    assert!(parse_err(b"{not json", dir.path()).contains("InvalidJson"));
    assert!(parse_err(b"[]", dir.path()).contains("NotAnObject"));
}

#[test]
fn rejects_wrong_version_and_unknown_state() {
    let dir = tempfile::tempdir().unwrap();
    let v2 = with(built_json(dir.path()), "v", 2.into());
    assert!(parse_err(&v2, dir.path()).contains("BadVersion"));
    let odd = with(built_json(dir.path()), "state", "installing".into());
    assert!(parse_err(&odd, dir.path()).contains("BadState"));
}

#[test]
fn rejects_unknown_keys_in_either_state() {
    let dir = tempfile::tempdir().unwrap();
    let extra = with(built_json(dir.path()), "bin_sha256", SHA.into());
    assert!(parse_err(&extra, dir.path()).contains("unexpected field"));
    // Install keys are not allowed while the record is still built.
    let mut value = built_json(dir.path());
    value
        .as_object_mut()
        .unwrap()
        .insert("committing_ts".into(), 1.into());
    assert!(parse_err(&bytes_of(&value), dir.path()).contains("unexpected field"));
}

#[test]
fn rejects_missing_keys_per_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut value = built_json(dir.path());
    value.as_object_mut().unwrap().remove("exe_sha256");
    assert!(parse_err(&bytes_of(&value), dir.path()).contains("exe_sha256"));
    let mut commit = committing_json(dir.path());
    commit.as_object_mut().unwrap().remove("committing_ts");
    assert!(parse_err(&bytes_of(&commit), dir.path()).contains("committing_ts"));
}

#[test]
fn rejects_malformed_run_and_target_ids() {
    let dir = tempfile::tempdir().unwrap();
    let upper = with(built_json(dir.path()), "run_id", "0123456789ABCDEF".into());
    assert!(parse_err(&upper, dir.path()).contains("BadRunId"));
    let short_target = with(built_json(dir.path()), "target_full", "0123".into());
    assert!(parse_err(&short_target, dir.path()).contains("BadTarget"));
    let upper_target = with(
        built_json(dir.path()),
        "target_full",
        TARGET.to_uppercase().into(),
    );
    assert!(parse_err(&upper_target, dir.path()).contains("BadTarget"));
}

#[test]
fn staged_app_must_be_exactly_the_layout_path() {
    let dir = tempfile::tempdir().unwrap();
    let elsewhere = with(built_json(dir.path()), "staged_app", "/tmp/Vox.app".into());
    assert!(parse_err(&elsewhere, dir.path()).contains("BadPath"));
    let traversal = format!("{}/staged/{RUN}/../{RUN}/Vox.app", dir.path().display());
    let bytes = with(built_json(dir.path()), "staged_app", traversal.into());
    assert!(parse_err(&bytes, dir.path()).contains("BadPath"));
}

#[test]
fn rejects_bad_exe_name_and_digests_and_origin() {
    let dir = tempfile::tempdir().unwrap();
    for bad in ["", "../x", "a/b", "..", ".", "sp ace", &"a".repeat(65)] {
        let bytes = with(built_json(dir.path()), "exe_name", bad.into());
        assert!(
            parse_err(&bytes, dir.path()).contains("BadExeName"),
            "{bad}"
        );
    }
    let upper = with(
        built_json(dir.path()),
        "exe_sha256",
        SHA.to_uppercase().into(),
    );
    assert!(parse_err(&upper, dir.path()).contains("BadSha256"));
    let short = with(built_json(dir.path()), "info_plist_sha256", "abc".into());
    assert!(parse_err(&short, dir.path()).contains("BadSha256"));
    let origin = with(built_json(dir.path()), "origin", "robot".into());
    assert!(parse_err(&origin, dir.path()).contains("BadOrigin"));
}

#[test]
fn committing_checks_install_identity() {
    let dir = tempfile::tempdir().unwrap();
    let same = with(committing_json(dir.path()), "install_run_id", RUN.into());
    assert!(parse_err(&same, dir.path()).contains("BadRunId"));
    let label = with(
        committing_json(dir.path()),
        "install_label",
        "com.evil".into(),
    );
    assert!(parse_err(&label, dir.path()).contains("install_label"));
    let plist = with(
        committing_json(dir.path()),
        "install_plist",
        "/tmp/x.plist".into(),
    );
    assert!(parse_err(&plist, dir.path()).contains("BadPath"));
}

#[test]
fn parse_errors_never_echo_input() {
    let dir = tempfile::tempdir().unwrap();
    let hostile = with(built_json(dir.path()), "run_id", "<script>\u{7}".into());
    let error = parse_pending(&hostile, dir.path()).unwrap_err();
    let text = error.to_string();
    assert!(!text.contains("script"));
    assert!(!text.contains('\u{7}'));
}

#[test]
fn load_returns_none_when_missing_and_record_when_valid() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(&state(dir.path()));
    assert!(store.load().unwrap().is_none());
    fs::write(store.path(), bytes_of(&built_json(dir.path()))).unwrap();
    assert_eq!(store.load().unwrap(), Some(built(dir.path())));
}

#[test]
fn load_refuses_symlink_directory_and_oversize() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let target = dir.path().join("real.json");
    fs::write(&target, bytes_of(&built_json(dir.path()))).unwrap();
    symlink(&target, store.path()).unwrap();
    assert!(matches!(store.load(), Err(PendingError::NotRegularFile)));
    fs::remove_file(store.path()).unwrap();
    fs::create_dir(store.path()).unwrap();
    assert!(matches!(store.load(), Err(PendingError::NotRegularFile)));
    fs::remove_dir(store.path()).unwrap();
    fs::write(store.path(), vec![b' '; PENDING_MAX_BYTES + 10]).unwrap();
    assert!(matches!(store.load(), Err(PendingError::TooLarge)));
}

#[test]
fn begin_commit_writes_private_committing_record_and_reloads() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let base = built(dir.path());
    fs::write(store.path(), encode_pending(&base, dir.path()).unwrap()).unwrap();
    let committing = store.begin_commit(&base, INSTALL, 1_700_000_100).unwrap();
    let mode = fs::metadata(store.path()).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(store.load().unwrap(), Some(committing));
}

#[test]
fn begin_commit_refuses_stale_or_same_run_or_built_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let base = built(dir.path());
    assert!(store.begin_commit(&base, INSTALL, 1).is_err()); // no file on disk
    fs::write(store.path(), encode_pending(&base, dir.path()).unwrap()).unwrap();
    assert!(store.begin_commit(&base, RUN, 1).is_err()); // same run id
    assert!(store.begin_commit(&base, "not-hex", 1).is_err());
    let mut other = base.clone();
    other.ts += 1;
    assert!(store.begin_commit(&other, INSTALL, 1).is_err()); // differs from disk
}

#[test]
fn revert_moves_committing_back_to_built() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let base = built(dir.path());
    fs::write(store.path(), encode_pending(&base, dir.path()).unwrap()).unwrap();
    let committing = store.begin_commit(&base, INSTALL, 9).unwrap();
    let reverted = store.revert(&committing).unwrap();
    assert_eq!(reverted, base);
    assert_eq!(store.load().unwrap(), Some(base));
    assert!(store.revert(&reverted).is_err());
}

#[test]
fn remove_all_removes_record_and_its_staged_dir_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let base = built(dir.path());
    let staged = dir.path().join("staged").join(RUN);
    fs::create_dir_all(staged.join("Vox.app")).unwrap();
    fs::write(store.path(), encode_pending(&base, dir.path()).unwrap()).unwrap();
    store.remove_all().unwrap();
    assert!(!store.path().exists());
    assert!(!staged.exists());
    store.remove_all().unwrap();
}

#[test]
fn remove_all_refuses_symlinked_record_without_following_it() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let victim = dir.path().join("victim.txt");
    fs::write(&victim, "keep").unwrap();
    symlink(&victim, store.path()).unwrap();
    assert!(store.remove_all().is_err());
    assert_eq!(fs::read_to_string(&victim).unwrap(), "keep");
}

#[test]
fn remove_all_drops_unreadable_record_but_keeps_other_staged_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    fs::write(store.path(), "garbage").unwrap();
    let other = dir.path().join("staged").join(INSTALL);
    fs::create_dir_all(&other).unwrap();
    store.remove_all().unwrap();
    assert!(!store.path().exists());
    assert!(other.exists());
}

#[test]
fn remove_staged_refuses_symlink_and_never_touches_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("keep"), "x").unwrap();
    fs::create_dir_all(dir.path().join("staged")).unwrap();
    symlink(&outside, dir.path().join("staged").join(RUN)).unwrap();
    assert!(store.remove_staged(RUN).is_err());
    assert!(outside.join("keep").exists());
    assert!(store.remove_staged("bad").is_err());
}

#[test]
fn sweep_orphans_keeps_referenced_run_and_skips_symlinks_and_odd_names() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    let root = dir.path().join("staged");
    fs::create_dir_all(root.join(RUN)).unwrap();
    fs::create_dir_all(root.join(INSTALL)).unwrap();
    fs::create_dir_all(root.join("not-a-run")).unwrap();
    let outside = dir.path().join("outside");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, root.join("aaaaaaaaaaaaaaaa")).unwrap();
    assert_eq!(store.sweep_orphans(Some(RUN)), 1);
    assert!(root.join(RUN).exists());
    assert!(!root.join(INSTALL).exists());
    assert!(root.join("not-a-run").exists());
    assert!(outside.exists());
    assert_eq!(
        PendingStore::new(&dir.path().join("none")).sweep_orphans(None),
        0
    );
}

#[test]
fn pending_built_for_is_true_only_for_this_built_run() {
    let dir = tempfile::tempdir().unwrap();
    let store = PendingStore::new(dir.path());
    assert!(!pending_built_for(dir.path(), RUN));
    fs::write(
        store.path(),
        encode_pending(&built(dir.path()), dir.path()).unwrap(),
    )
    .unwrap();
    assert!(pending_built_for(dir.path(), RUN));
    assert!(!pending_built_for(dir.path(), INSTALL));
    let commit = store.begin_commit(&built(dir.path()), INSTALL, 3).unwrap();
    assert!(!pending_built_for(dir.path(), RUN), "{commit:?}");
}
