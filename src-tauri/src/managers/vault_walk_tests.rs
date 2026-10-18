//! Contract tests for `Vault::walk_notes`, `VaultSource::walk`, `Vault::read_note_checked` and
//! `VaultSource::read` (DESIGN Rev 3, section 3.2).
//!
//! Each test runs the real walk and read code in `vault.rs` against a fresh tempdir with
//! synthetic fixtures only, so a failure means a walk or read contract regressed.

use crate::managers::vault::{
    NoteRead, NoteSource, Vault, VaultError, VaultSource, WalkLimits, WalkResult,
    MAX_READ_NOTE_BYTES,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

#[cfg(unix)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};

/// Fixed modification time used by the metadata tests: 2023-11-14T22:13:20Z.
const FIXED_MTIME_SECS: u64 = 1_700_000_000;

fn expect_ok<T>(result: Result<T, VaultError>, what: &str) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("{what}: unexpected error: {err}"),
    }
}

fn expect_err<T>(result: Result<T, VaultError>, what: &str) -> VaultError {
    match result {
        Ok(_) => panic!("{what}: expected an error, got Ok"),
        Err(err) => err,
    }
}

fn as_str(path: &Path) -> &str {
    path.to_str().expect("temp path is valid UTF-8")
}

fn open_vault(root: &Path) -> Vault {
    expect_ok(Vault::open(as_str(root)), "Vault::open")
}

/// Writes `bytes` at `rel` under `root`, creating parent directories.
fn put(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    fs::write(&path, bytes).expect("write fixture");
}

fn roots(names: &[&str]) -> Vec<String> {
    names.iter().map(|name| (*name).to_owned()).collect()
}

fn limits(names: &[&str]) -> WalkLimits {
    WalkLimits {
        allowed_roots: roots(names),
        ..WalkLimits::default()
    }
}

fn walk(root: &Path, names: &[&str]) -> WalkResult {
    expect_ok(
        open_vault(root).walk_notes(&limits(names)),
        "walk_notes with default limits",
    )
}

fn walk_rels(result: &WalkResult) -> Vec<String> {
    result.notes.iter().map(|note| note.rel.clone()).collect()
}

fn checked(root: &Path, rel: &str, names: &[&str]) -> Result<NoteRead, VaultError> {
    open_vault(root).read_note_checked(rel, &roots(names))
}

/// True when the test process runs as root (chmod 000 does not block root).
#[cfg(unix)]
fn running_as_root() -> bool {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .map(|out| String::from_utf8_lossy(&out.stdout).trim() == "0")
        .unwrap_or(false)
}

/// Restores a directory's mode when dropped, so the tempdir can be removed.
#[cfg(unix)]
struct RestoreMode {
    path: PathBuf,
}

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o755));
    }
}

// ---------------------------------------------------------------------------
// Vault::walk_notes
// ---------------------------------------------------------------------------

#[test]
fn walk_returns_only_entries_under_allowed_roots() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/a.md", b"a");
    put(tmp.path(), "Notes/sub/b.md", b"b");
    put(tmp.path(), "Other/c.md", b"c");
    put(tmp.path(), "top.md", b"root level");

    let result = walk(tmp.path(), &["Notes"]);

    assert!(result.complete, "small vault must be complete");
    assert_eq!(
        walk_rels(&result),
        vec!["Notes/a.md".to_owned(), "Notes/sub/b.md".to_owned()],
        "only Notes/ entries; root-level top.md and Other/ must be excluded"
    );
}

#[test]
fn walk_with_empty_allowed_roots_returns_zero_notes_and_is_complete() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/a.md", b"a");
    put(tmp.path(), "top.md", b"root");

    let result = walk(tmp.path(), &[]);

    assert!(
        result.notes.is_empty(),
        "empty allowed_roots must yield no notes"
    );
    assert!(result.complete, "empty allowed_roots must report complete");
}

#[test]
fn walk_skips_hidden_dirs_and_hidden_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/.obsidian/x.md", b"hidden dir");
    put(tmp.path(), "Notes/.git/y.md", b"hidden git dir");
    put(tmp.path(), "Notes/.hidden.md", b"hidden file");
    put(tmp.path(), "Notes/visible.md", b"visible");

    let result = walk(tmp.path(), &["Notes"]);

    assert_eq!(walk_rels(&result), vec!["Notes/visible.md".to_owned()]);
}

#[test]
fn walk_skips_non_markdown_files() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/pic.png", b"\x89PNG");
    put(tmp.path(), "Notes/plain.txt", b"text");
    put(tmp.path(), "Notes/real.md", b"note");

    let result = walk(tmp.path(), &["Notes"]);

    assert_eq!(walk_rels(&result), vec!["Notes/real.md".to_owned()]);
}

#[cfg(unix)]
#[test]
fn walk_never_follows_symlinked_file_or_dir_and_counts_them_as_other() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    put(tmp.path(), "Notes/real/inner.md", b"inner");
    put(tmp.path(), "Notes/real_note.md", b"real");
    put(outside.path(), "leak.md", b"outside note");

    // Symlinked file and symlinked directory inside the vault, pointing at real notes.
    symlink(
        tmp.path().join("Notes/real_note.md"),
        tmp.path().join("Notes/link_note.md"),
    )
    .expect("symlink file");
    symlink(
        tmp.path().join("Notes/real"),
        tmp.path().join("Notes/link_dir"),
    )
    .expect("symlink dir");
    // Symlinked directory pointing outside the vault.
    symlink(outside.path(), tmp.path().join("Notes/leak_dir")).expect("symlink outside");

    let result = walk(tmp.path(), &["Notes"]);

    assert_eq!(
        walk_rels(&result),
        vec![
            "Notes/real/inner.md".to_owned(),
            "Notes/real_note.md".to_owned()
        ],
        "symlinks must never be followed; only the real entries are returned"
    );
    assert!(
        result.skipped_other >= 3,
        "three symlinks must be counted in skipped_other, got {}",
        result.skipped_other
    );
    assert!(result.complete, "skipping symlinks is not a truncation");
}

#[test]
fn walk_skips_notes_larger_than_max_file_bytes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/exact.md", &[b'a'; 10]);
    put(tmp.path(), "Notes/over.md", &[b'b'; 11]);

    let mut walk_limits = limits(&["Notes"]);
    walk_limits.max_file_bytes = 10;
    let result = expect_ok(
        open_vault(tmp.path()).walk_notes(&walk_limits),
        "walk_notes with max_file_bytes 10",
    );

    assert_eq!(
        walk_rels(&result),
        vec!["Notes/exact.md".to_owned()],
        "exactly max_file_bytes is allowed, max+1 is skipped"
    );
    assert_eq!(result.skipped_large, 1);
}

#[test]
fn walk_applies_default_depth_cap_of_eight_directories() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let eight: String = (1..=8)
        .map(|i| format!("d{i}"))
        .collect::<Vec<_>>()
        .join("/");
    let nine = format!("{eight}/d9");
    put(tmp.path(), &format!("Notes/{eight}/ok.md"), b"eight deep");
    put(tmp.path(), &format!("Notes/{nine}/deep.md"), b"nine deep");

    let result = walk(tmp.path(), &["Notes"]);

    assert_eq!(
        walk_rels(&result),
        vec![format!("Notes/{eight}/ok.md")],
        "a note nine directories below Notes must not be returned"
    );
    assert!(
        result.complete,
        "the depth cap is not a truncation; DESIGN 3.2 sets complete=false only for the file cap or enumeration errors"
    );
}

#[test]
fn walk_sets_incomplete_when_file_cap_is_hit() {
    let tmp = tempfile::tempdir().expect("tempdir");
    for i in 1..=5 {
        put(tmp.path(), &format!("Notes/n{i}.md"), b"note");
    }

    let mut walk_limits = limits(&["Notes"]);
    walk_limits.max_files = 3;
    let result = expect_ok(
        open_vault(tmp.path()).walk_notes(&walk_limits),
        "walk_notes with max_files 3",
    );

    assert!(
        !result.complete,
        "hitting the file cap must set complete=false"
    );
    assert_eq!(result.notes.len(), 3, "exactly max_files notes returned");
}

#[cfg(unix)]
#[test]
fn walk_unreadable_directory_is_incomplete_and_other_folders_still_listed() {
    if running_as_root() {
        eprintln!("skipped: running as root, chmod 000 does not block root");
        return;
    }
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/locked/x.md", b"locked");
    put(tmp.path(), "Notes/open.md", b"open");
    let locked = tmp.path().join("Notes/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).expect("chmod 000");
    let _restore = RestoreMode {
        path: locked.clone(),
    };

    let result = walk(tmp.path(), &["Notes"]);

    assert!(
        !result.complete,
        "an enumeration error must set complete=false"
    );
    assert!(
        result.skipped_unreadable >= 1,
        "the unreadable directory must be counted in skipped_unreadable"
    );
    assert!(
        walk_rels(&result).contains(&"Notes/open.md".to_owned()),
        "other folders must still be listed"
    );
}

#[cfg(unix)]
#[test]
fn walk_skips_non_utf8_file_names() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/good.md", b"good");
    let bad_name = OsStr::from_bytes(b"bad\xff.md");
    if let Err(err) = fs::write(tmp.path().join("Notes").join(bad_name), b"bad") {
        // Some filesystems (for example APFS on macOS) refuse non-UTF-8 names outright.
        // The fixture cannot exist there, so this test is not exercised on that platform.
        eprintln!("skipped: filesystem refused non-UTF-8 name: {}", err.kind());
        return;
    }

    let result = walk(tmp.path(), &["Notes"]);

    assert_eq!(walk_rels(&result), vec!["Notes/good.md".to_owned()]);
    assert!(
        result.skipped_other >= 1,
        "non-UTF-8 name counted in skipped_other"
    );
}

#[test]
fn walk_of_empty_allowed_folder_is_empty_and_complete() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("Notes")).expect("create empty Notes");

    let result = walk(tmp.path(), &["Notes"]);

    assert!(result.notes.is_empty());
    assert!(result.complete);
    assert_eq!(result.skipped_large, 0);
    assert_eq!(result.skipped_other, 0);
    assert_eq!(result.skipped_unreadable, 0);
}

#[test]
fn walk_results_are_sorted_by_rel() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/z.md", b"z");
    put(tmp.path(), "Notes/a/m.md", b"m");
    put(tmp.path(), "Notes/a.md", b"a");
    put(tmp.path(), "Notes/b/c.md", b"c");

    let result = walk(tmp.path(), &["Notes"]);
    let got = walk_rels(&result);
    let mut sorted = got.clone();
    sorted.sort();

    assert_eq!(got.len(), 4, "all four notes returned");
    assert_eq!(got, sorted, "results must be sorted by rel");
}

#[test]
fn walk_populates_size_and_mtime_ms() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/meta.md", b"hello world");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(tmp.path().join("Notes/meta.md"))
        .expect("open for set_modified");
    file.set_modified(UNIX_EPOCH + Duration::from_secs(FIXED_MTIME_SECS))
        .expect("set mtime");
    drop(file);

    let result = walk(tmp.path(), &["Notes"]);

    assert_eq!(result.notes.len(), 1);
    assert_eq!(result.notes[0].rel, "Notes/meta.md");
    assert_eq!(result.notes[0].size, 11, "size in bytes");
    assert_eq!(
        result.notes[0].mtime_ms,
        (FIXED_MTIME_SECS * 1000) as i64,
        "mtime in milliseconds since the Unix epoch"
    );
}

#[test]
fn vault_source_walk_matches_vault_walk() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/a.md", b"a");
    put(tmp.path(), "Notes/sub/b.md", b"b");
    put(tmp.path(), "Other/c.md", b"c");
    let source = VaultSource::new(tmp.path().to_path_buf(), roots(&["Notes"]));

    let via_source = expect_ok(source.walk(&limits(&["Notes"])), "VaultSource::walk");
    let via_vault = walk(tmp.path(), &["Notes"]);

    assert_eq!(via_source.notes, via_vault.notes);
    assert_eq!(via_source.complete, via_vault.complete);
}

/// Walk errors are never produced for an ordinary vault: `walk_notes` returns `Ok` even when it
/// skips hidden, non-markdown and oversize entries (DESIGN 3.2).
#[test]
fn walk_does_not_report_forbidden_invalid_or_unsafe() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/.hidden.md", b"x");
    put(tmp.path(), "Notes/pic.png", b"x");
    put(tmp.path(), "Notes/ok.md", b"ok");

    let outcome = open_vault(tmp.path()).walk_notes(&limits(&["Notes"]));

    assert!(
        outcome.is_ok(),
        "walk must skip bad entries without returning an error"
    );
}

/// Display strings never carry a path, a file name or note text (DESIGN 3.2 error hygiene).
#[test]
fn vault_error_display_never_contains_paths_or_note_text() {
    use std::io::ErrorKind;
    let variants = [
        VaultError::Invalid,
        VaultError::Missing,
        VaultError::TooLarge,
        VaultError::Unsafe,
        VaultError::Exists,
        VaultError::Io(ErrorKind::PermissionDenied),
        VaultError::Forbidden,
        VaultError::Unstable,
    ];
    for err in &variants {
        let text = err.to_string();
        assert!(!text.is_empty(), "Display must not be empty");
        assert!(
            !text.contains(".md") && !text.contains('\\') && !text.contains("//"),
            "Display must not carry a path or file name: {text:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Vault::read_note_checked and VaultSource::read
// ---------------------------------------------------------------------------

#[test]
fn read_valid_note_returns_text_size_and_mtime() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/a.md", b"hello\nworld");
    let file = fs::OpenOptions::new()
        .write(true)
        .open(tmp.path().join("Notes/a.md"))
        .expect("open for set_modified");
    file.set_modified(UNIX_EPOCH + Duration::from_secs(FIXED_MTIME_SECS))
        .expect("set mtime");
    drop(file);

    let note = expect_ok(
        checked(tmp.path(), "Notes/a.md", &["Notes"]),
        "read valid note",
    );

    assert_eq!(note.text, "hello\nworld");
    assert_eq!(note.size, 11);
    assert_eq!(note.mtime_ms, (FIXED_MTIME_SECS * 1000) as i64);
}

#[test]
fn read_outside_allowed_roots_is_forbidden() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Other/x.md", b"other");
    put(tmp.path(), "top.md", b"root level");
    put(tmp.path(), "Notes/a.md", b"notes");

    let other = expect_err(checked(tmp.path(), "Other/x.md", &["Notes"]), "Other/x.md");
    assert!(
        matches!(other, VaultError::Forbidden),
        "Other/: got {other}"
    );

    let root_level = expect_err(checked(tmp.path(), "top.md", &["Notes"]), "top.md");
    assert!(
        matches!(root_level, VaultError::Forbidden),
        "root-level note: got {root_level}"
    );

    let no_roots = expect_err(checked(tmp.path(), "Notes/a.md", &[]), "empty roots");
    assert!(
        matches!(no_roots, VaultError::Forbidden),
        "empty allowed_roots: got {no_roots}"
    );
}

#[test]
fn read_rejects_malformed_rel_as_invalid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/a.md", b"notes");
    let bad = [
        "",
        "Notes/../a.md",
        "/Notes/a.md",
        "Notes/.hidden/x.md",
        "Notes/.x.md",
        "Notes/a.txt",
        "Notes/a\0.md",
        "Notes\\a.md",
        "C:/Notes/a.md",
        "Notes//a.md",
    ];
    for rel in bad {
        let err = expect_err(checked(tmp.path(), rel, &["Notes"]), rel);
        assert!(
            matches!(err, VaultError::Invalid),
            "{rel:?}: expected Invalid, got {err}"
        );
    }
}

#[cfg(unix)]
#[test]
fn read_through_symlink_component_or_final_symlink_is_unsafe() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/real.md", b"real");
    put(tmp.path(), "Notes/realdir/a.md", b"in real dir");
    symlink(
        tmp.path().join("Notes/real.md"),
        tmp.path().join("Notes/link.md"),
    )
    .expect("symlink final");
    symlink(
        tmp.path().join("Notes/realdir"),
        tmp.path().join("Notes/linkdir"),
    )
    .expect("symlink component");

    let final_link = expect_err(
        checked(tmp.path(), "Notes/link.md", &["Notes"]),
        "final symlink",
    );
    assert!(matches!(final_link, VaultError::Unsafe), "got {final_link}");

    let component = expect_err(
        checked(tmp.path(), "Notes/linkdir/a.md", &["Notes"]),
        "symlinked component",
    );
    assert!(matches!(component, VaultError::Unsafe), "got {component}");
}

#[test]
fn read_accepts_exactly_max_bytes_and_rejects_one_more_as_too_large() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let limit = MAX_READ_NOTE_BYTES as usize;
    put(tmp.path(), "Notes/exact.md", &vec![b'a'; limit]);
    put(tmp.path(), "Notes/over.md", &vec![b'b'; limit + 1]);

    let exact = expect_ok(
        checked(tmp.path(), "Notes/exact.md", &["Notes"]),
        "exact limit",
    );
    assert_eq!(exact.size, MAX_READ_NOTE_BYTES);
    assert_eq!(exact.text.len(), limit);

    let over = expect_err(
        checked(tmp.path(), "Notes/over.md", &["Notes"]),
        "limit + 1",
    );
    assert!(matches!(over, VaultError::TooLarge), "got {over}");
}

#[test]
fn read_invalid_utf8_is_decoded_lossily() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/bad.md", b"a\xffb");

    let note = expect_ok(
        checked(tmp.path(), "Notes/bad.md", &["Notes"]),
        "lossy read",
    );

    assert_eq!(note.text, "a\u{FFFD}b");
}

#[test]
fn read_missing_note_is_missing() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("Notes")).expect("create Notes");

    let err = expect_err(
        checked(tmp.path(), "Notes/none.md", &["Notes"]),
        "missing note",
    );

    assert!(matches!(err, VaultError::Missing), "got {err}");
}

#[test]
fn read_directory_named_like_a_note_is_invalid() {
    // Design choice: a directory where the note file is expected is `Invalid`, matching
    // `read_context_pack` ("a directory or other non-regular entry" is Invalid).
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("Notes/adir.md")).expect("create dir named *.md");

    let err = expect_err(
        checked(tmp.path(), "Notes/adir.md", &["Notes"]),
        "directory as note",
    );

    assert!(matches!(err, VaultError::Invalid), "got {err}");
}

#[test]
fn read_reports_unstable_when_file_changes_between_reads() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/a.md", b"one");
    let path = tmp.path().join("Notes/a.md");
    let vault = open_vault(tmp.path());
    let mut mutate = || {
        let _ = fs::write(&path, b"one, now much longer");
    };

    let err = expect_err(
        vault.read_note_checked_with_hook("Notes/a.md", &roots(&["Notes"]), &mut mutate),
        "read with mutation between reads",
    );

    assert!(matches!(err, VaultError::Unstable), "got {err}");
}

#[test]
fn read_with_no_mutation_between_reads_is_ok() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/a.md", b"stable");
    let vault = open_vault(tmp.path());
    let mut noop = || {};

    let note = expect_ok(
        vault.read_note_checked_with_hook("Notes/a.md", &roots(&["Notes"]), &mut noop),
        "read without mutation",
    );

    assert_eq!(note.text, "stable");
}

#[test]
fn vault_source_read_returns_note_and_enforces_roots() {
    let tmp = tempfile::tempdir().expect("tempdir");
    put(tmp.path(), "Notes/a.md", b"via source");
    put(tmp.path(), "Other/x.md", b"other");
    let source = VaultSource::new(tmp.path().to_path_buf(), roots(&["Notes"]));

    let note = expect_ok(source.read("Notes/a.md"), "VaultSource::read");
    assert_eq!(note.text, "via source");

    let err = expect_err(source.read("Other/x.md"), "source read outside roots");
    assert!(matches!(err, VaultError::Forbidden), "got {err}");
}
