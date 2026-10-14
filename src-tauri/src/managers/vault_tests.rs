//! Tests for the confined vault IO contract (DESIGN Rev 3, section 3.2 and T2).
//!
//! Written test-first: the functions in `vault.rs` are `todo!()`, so these tests
//! are expected to fail (panic on `todo!()`) until the implementation lands.
//! All fixtures are synthetic and every test uses a fresh tempdir.

use crate::managers::vault::{
    Vault, VaultError, CONTEXT_PACK_REL, MAX_NOTE_WRITE_BYTES, MAX_PACK_BYTES, NOTES_DIR_REL,
};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Barrier;

#[cfg(unix)]
use std::os::unix::fs::{symlink, PermissionsExt};

/// Number of fresh-tempdir iterations for each race test.
const RACE_ITERATIONS: usize = 50;

/// Synthetic credential-shaped fixtures. None of these are real secrets.
const FAKE_SECRETS: &[&str] = &[
    "eyJhbGciOiJIUzI1NiJ9.eyJzeW50aGV0aWMiOiJmYWtlIn0.ZmFrZXNpZ25hdHVyZQ",
    "password = \"FAKE-synthetic-pass-0001\"",
    "sk-FAKE-SYNTHETIC-0000000000000000",
    "AKIAFAKESYNTHETIC0000",
];

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

fn write_bytes(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture parent");
    }
    fs::write(path, bytes).expect("write fixture");
}

/// Every entry under `dir` (files, dirs and symlinks). Does not follow symlinks.
fn all_entries(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let path = entry.path();
        let is_real_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
        out.push(path.clone());
        if is_real_dir {
            out.extend(all_entries(&path));
        }
    }
    out
}

/// Regular files under `dir`. Symlinks are not followed and not counted.
fn regular_files(dir: &Path) -> Vec<PathBuf> {
    all_entries(dir)
        .into_iter()
        .filter(|p| {
            fs::symlink_metadata(p)
                .map(|m| m.is_file())
                .unwrap_or(false)
        })
        .collect()
}

/// Restores a directory mode on drop so tempdir cleanup can succeed.
#[cfg(unix)]
struct RestoreMode {
    path: PathBuf,
    mode: u32,
}

#[cfg(unix)]
impl Drop for RestoreMode {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(self.mode));
    }
}

// ---------------------------------------------------------------- open

#[test]
fn open_rejects_empty_root() {
    let err = expect_err(Vault::open(""), "open empty root");
    assert!(
        matches!(err, VaultError::Invalid),
        "empty root must be Invalid, got {err:?}"
    );
}

#[test]
fn open_rejects_relative_root() {
    let err = expect_err(Vault::open("relative/vault-root"), "open relative root");
    assert!(
        matches!(err, VaultError::Invalid),
        "relative root must be Invalid, got {err:?}"
    );
}

#[test]
fn open_rejects_missing_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let missing = tmp.path().join("does-not-exist");
    let err = expect_err(Vault::open(as_str(&missing)), "open missing root");
    assert!(
        matches!(err, VaultError::Missing),
        "missing root must be Missing, got {err:?}"
    );
}

#[test]
fn open_rejects_regular_file_root() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("plain.txt");
    write_bytes(&file, b"not a directory");
    let err = expect_err(Vault::open(as_str(&file)), "open regular file");
    assert!(
        matches!(err, VaultError::Invalid),
        "regular-file root must be Invalid, got {err:?}"
    );
}

#[test]
fn open_accepts_existing_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let _vault = expect_ok(Vault::open(as_str(tmp.path())), "open existing dir");
}

// ---------------------------------------------------------------- read_context_pack

#[test]
fn read_missing_when_pack_absent() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let vault = open_vault(tmp.path());
    let err = expect_err(vault.read_context_pack(), "read absent pack");
    assert!(
        matches!(err, VaultError::Missing),
        "absent pack must be Missing, got {err:?}"
    );
}

#[test]
fn read_returns_normal_pack() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_bytes(
        &tmp.path().join(CONTEXT_PACK_REL),
        "# Pack\n- note one\n".as_bytes(),
    );
    let vault = open_vault(tmp.path());
    let text = expect_ok(vault.read_context_pack(), "read normal pack");
    assert_eq!(text, "# Pack\n- note one\n");
}

#[test]
fn read_accepts_exactly_max_pack_bytes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_bytes(
        &tmp.path().join(CONTEXT_PACK_REL),
        &vec![b'a'; MAX_PACK_BYTES],
    );
    let vault = open_vault(tmp.path());
    let text = expect_ok(vault.read_context_pack(), "read exactly MAX_PACK_BYTES");
    assert_eq!(text.len(), MAX_PACK_BYTES);
    assert_eq!(text, "a".repeat(MAX_PACK_BYTES));
}

#[test]
fn read_rejects_one_byte_over_max_pack_bytes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_bytes(
        &tmp.path().join(CONTEXT_PACK_REL),
        &vec![b'a'; MAX_PACK_BYTES + 1],
    );
    let vault = open_vault(tmp.path());
    let err = expect_err(vault.read_context_pack(), "read MAX_PACK_BYTES + 1");
    assert!(
        matches!(err, VaultError::TooLarge),
        "MAX_PACK_BYTES + 1 must be TooLarge, got {err:?}"
    );
}

#[test]
fn read_rejects_secret_straddling_the_pack_limit() {
    for secret in FAKE_SECRETS {
        // Each cut places the secret start `cut` bytes before the 64 KiB boundary,
        // so the secret straddles it. The pack is over the limit, so it must be
        // rejected whole (TooLarge), never truncated to a partial secret.
        for cut in [1, secret.len() / 2, secret.len() - 1] {
            let tmp = tempfile::tempdir().expect("tempdir");
            let mut body = "a".repeat(MAX_PACK_BYTES - cut);
            body.push_str(secret);
            body.push_str(&"b".repeat(100));
            assert!(body.len() > MAX_PACK_BYTES, "fixture must exceed the limit");
            write_bytes(&tmp.path().join(CONTEXT_PACK_REL), body.as_bytes());
            let vault = open_vault(tmp.path());
            let err = expect_err(
                vault.read_context_pack(),
                &format!("straddling secret cut={cut}"),
            );
            assert!(
                matches!(err, VaultError::TooLarge),
                "straddling secret (cut {cut}) must be TooLarge, got {err:?}"
            );
        }
    }
}

#[test]
fn read_decodes_invalid_utf8_lossily() {
    let tmp = tempfile::tempdir().expect("tempdir");
    write_bytes(
        &tmp.path().join(CONTEXT_PACK_REL),
        b"ok \xff\xfe bad-utf8 \xc3(",
    );
    let vault = open_vault(tmp.path());
    let text = expect_ok(vault.read_context_pack(), "read invalid UTF-8");
    assert!(text.starts_with("ok "), "lossy text kept the valid prefix");
    assert!(
        text.contains('\u{FFFD}'),
        "invalid bytes must decode to U+FFFD"
    );
}

#[test]
fn read_directory_at_pack_path_is_invalid() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join(CONTEXT_PACK_REL)).expect("create dir at pack path");
    let vault = open_vault(tmp.path());
    let err = expect_err(vault.read_context_pack(), "read directory at pack path");
    assert!(
        matches!(err, VaultError::Invalid),
        "a directory at the pack path must be Invalid, got {err:?}"
    );
}

#[cfg(unix)]
#[test]
fn read_rejects_symlinked_pack_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    let outside_pack = outside.path().join("pack.md");
    fs::write(&outside_pack, "outside pack").expect("write outside pack");
    let code_puppy = tmp.path().join("Memory").join("Code-Puppy");
    fs::create_dir_all(&code_puppy).expect("create Code-Puppy");
    symlink(&outside_pack, code_puppy.join("CONTEXT-PACK.md")).expect("symlink pack");
    let vault = open_vault(tmp.path());
    let err = expect_err(vault.read_context_pack(), "read symlinked pack");
    assert!(
        matches!(err, VaultError::Unsafe),
        "symlinked pack file must be Unsafe, got {err:?}"
    );
}

#[cfg(unix)]
#[test]
fn read_rejects_symlinked_memory_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    write_bytes(
        &outside.path().join("Code-Puppy").join("CONTEXT-PACK.md"),
        b"outside pack",
    );
    symlink(outside.path(), tmp.path().join("Memory")).expect("symlink Memory");
    let vault = open_vault(tmp.path());
    let err = expect_err(vault.read_context_pack(), "read via symlinked Memory");
    assert!(
        matches!(err, VaultError::Unsafe),
        "symlinked Memory dir must be Unsafe, got {err:?}"
    );
}

#[cfg(unix)]
#[test]
fn read_rejects_symlinked_code_puppy_dir() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    write_bytes(&outside.path().join("CONTEXT-PACK.md"), b"outside pack");
    fs::create_dir_all(tmp.path().join("Memory")).expect("create Memory");
    symlink(outside.path(), tmp.path().join("Memory").join("Code-Puppy"))
        .expect("symlink Code-Puppy");
    let vault = open_vault(tmp.path());
    let err = expect_err(vault.read_context_pack(), "read via symlinked Code-Puppy");
    assert!(
        matches!(err, VaultError::Unsafe),
        "symlinked Code-Puppy dir must be Unsafe, got {err:?}"
    );
}

// ---------------------------------------------------------------- write_note

#[test]
fn write_creates_nested_dirs_and_returns_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let vault = open_vault(tmp.path());
    let path = expect_ok(
        vault.write_note("2026-10/note.md", "hello note\n"),
        "write nested note",
    );
    assert!(
        path.ends_with(Path::new(NOTES_DIR_REL).join("2026-10").join("note.md")),
        "returned path must be the note under the notes dir, got {path:?}"
    );
    let on_disk = tmp
        .path()
        .join(NOTES_DIR_REL)
        .join("2026-10")
        .join("note.md");
    assert!(on_disk.is_file(), "note file must exist");
    assert_eq!(
        fs::read_to_string(&on_disk).expect("read note"),
        "hello note\n"
    );
}

#[test]
fn write_rejects_unsafe_relative_paths() {
    // The absolute case points inside a separate outside tempdir. A naive
    // implementation that joins it would write there, which the check catches.
    let outside = tempfile::tempdir().expect("outside tempdir");
    let absolute = format!("{}/escape.md", as_str(outside.path()));
    let cases: Vec<&str> = vec![
        "",
        "..",
        "a/../b",
        absolute.as_str(),
        "nul\0byte.md",
        "back\\slash.md",
        "C:drive-escape.md",
        "C:/drive-escape.md",
        ".",
        "./x",
        "../Code-Puppy/x",
    ];
    for rel in cases {
        let tmp = tempfile::tempdir().expect("tempdir");
        let vault = open_vault(tmp.path());
        let err = expect_err(
            vault.write_note(rel, "payload"),
            &format!("write rel {rel:?}"),
        );
        assert!(
            matches!(err, VaultError::Invalid),
            "rel {rel:?} must be Invalid, got {err:?}"
        );
        assert!(
            regular_files(tmp.path()).is_empty(),
            "rel {rel:?} must not create files under the vault root"
        );
    }
    assert!(
        all_entries(outside.path()).is_empty(),
        "absolute rel must not write to the outside directory"
    );
}

#[test]
fn write_rejects_escape_into_code_puppy() {
    let tmp = tempfile::tempdir().expect("tempdir");
    fs::create_dir_all(tmp.path().join("Memory").join("Code-Puppy")).expect("create Code-Puppy");
    let vault = open_vault(tmp.path());
    let err = expect_err(
        vault.write_note("../Code-Puppy/x", "payload"),
        "write escape into Code-Puppy",
    );
    assert!(
        matches!(err, VaultError::Invalid),
        "../Code-Puppy/x must be Invalid, got {err:?}"
    );
    assert!(
        !tmp.path()
            .join("Memory")
            .join("Code-Puppy")
            .join("x")
            .exists(),
        "no file may be created in Code-Puppy"
    );
    assert!(
        regular_files(tmp.path()).is_empty(),
        "no files may be created"
    );
}

#[test]
fn write_existing_file_is_exists_and_bytes_unchanged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let vault = open_vault(tmp.path());
    let first = expect_ok(vault.write_note("dup.md", "original bytes"), "first write");
    let err = expect_err(
        vault.write_note("dup.md", "replacement bytes"),
        "second write",
    );
    assert!(
        matches!(err, VaultError::Exists),
        "existing note must be Exists, got {err:?}"
    );
    assert_eq!(
        fs::read(&first).expect("read first note"),
        b"original bytes",
        "existing note bytes must be unchanged"
    );
}

#[test]
fn write_accepts_exactly_max_note_bytes() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let vault = open_vault(tmp.path());
    let body = "a".repeat(MAX_NOTE_WRITE_BYTES);
    let path = expect_ok(
        vault.write_note("2026-10/max.md", &body),
        "write MAX_NOTE_WRITE_BYTES",
    );
    assert_eq!(
        fs::metadata(&path).expect("stat note").len() as usize,
        MAX_NOTE_WRITE_BYTES
    );
}

#[test]
fn write_rejects_one_byte_over_max_without_creating_file() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let vault = open_vault(tmp.path());
    let body = "a".repeat(MAX_NOTE_WRITE_BYTES + 1);
    let err = expect_err(vault.write_note("2026-10/over.md", &body), "write over max");
    assert!(
        matches!(err, VaultError::TooLarge),
        "MAX_NOTE_WRITE_BYTES + 1 must be TooLarge, got {err:?}"
    );
    assert!(
        regular_files(tmp.path()).is_empty(),
        "no file may be created for an oversized note"
    );
}

#[cfg(unix)]
#[test]
fn write_note_file_mode_is_0600() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let vault = open_vault(tmp.path());
    let path = expect_ok(
        vault.write_note("2026-10/mode.md", "mode body"),
        "write mode note",
    );
    let mode = fs::metadata(&path).expect("stat note").permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "note must be created with mode 0600");
}

#[cfg(unix)]
#[test]
fn write_unwritable_dir_is_io_error_without_leftovers() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let leaf = tmp.path().join(NOTES_DIR_REL).join("2026-10");
    fs::create_dir_all(&leaf).expect("create leaf");
    fs::set_permissions(&leaf, fs::Permissions::from_mode(0o500)).expect("make leaf read-only");
    // Declared after `tmp`, so it drops first and restores the mode before cleanup.
    let _restore = RestoreMode {
        path: leaf.clone(),
        mode: 0o700,
    };
    let vault = open_vault(tmp.path());
    let result = vault.write_note("2026-10/blocked.md", "partial body");
    assert!(
        matches!(result, Err(VaultError::Io(_))),
        "unwritable target dir must be Io(..), got {result:?}"
    );
    assert!(
        all_entries(&leaf).is_empty(),
        "no partial or temp files may be left in the target dir"
    );
}

#[cfg(unix)]
#[test]
fn write_symlinked_notes_dir_is_unsafe_and_creates_nothing_outside() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    fs::create_dir_all(tmp.path().join("Memory")).expect("create Memory");
    symlink(outside.path(), tmp.path().join(NOTES_DIR_REL)).expect("symlink notes dir");
    let vault = open_vault(tmp.path());
    let result = vault.write_note("2026-10/note.md", "outside body");
    assert!(
        matches!(result, Err(VaultError::Unsafe)),
        "symlinked notes dir must be Unsafe, got {result:?}"
    );
    assert!(
        all_entries(outside.path()).is_empty(),
        "zero files or dirs may be created in the outside directory"
    );
}

// ---------------------------------------------------------------- error text

#[test]
fn error_text_never_contains_contents_or_root_path() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = as_str(tmp.path()).to_string();
    let vault = open_vault(tmp.path());
    const BODY: &str = "SECRET-BODY-7f3a-do-not-leak";
    const OTHER: &str = "SECRET-OTHER-9c1e-do-not-leak";
    expect_ok(vault.write_note("dup.md", BODY), "seed note");
    let exists = expect_err(vault.write_note("dup.md", OTHER), "duplicate write");
    let too_large = expect_err(
        vault.write_note(
            "big.md",
            &format!("{BODY}{}", "x".repeat(MAX_NOTE_WRITE_BYTES)),
        ),
        "oversized write",
    );
    let invalid = expect_err(vault.write_note("../escape.md", BODY), "invalid rel");
    let missing_root = expect_err(
        Vault::open(&format!("{root}/does-not-exist")),
        "missing root",
    );
    let mut samples = vec![exists, too_large, invalid, missing_root];
    samples.extend([
        VaultError::Invalid,
        VaultError::Missing,
        VaultError::TooLarge,
        VaultError::Unsafe,
        VaultError::Exists,
        VaultError::Io(std::io::ErrorKind::PermissionDenied),
    ]);
    let forbidden = [BODY, OTHER, root.as_str()];
    for err in &samples {
        let rendered = format!("{err} | {err:?}");
        for text in forbidden {
            assert!(
                !rendered.contains(text),
                "error {err:?} leaked a forbidden fragment (contents or root path)"
            );
        }
    }
}

// ---------------------------------------------------------------- races

/// One swap-then-write race. `swap_memory` selects which component is swapped.
#[cfg(unix)]
fn race_swap_once(iteration: usize, swap_memory: bool) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let outside = tempfile::tempdir().expect("outside tempdir");
    fs::create_dir_all(tmp.path().join(NOTES_DIR_REL)).expect("create notes dir");
    let vault = open_vault(tmp.path());
    let swap_at = if swap_memory {
        tmp.path().join("Memory")
    } else {
        tmp.path().join(NOTES_DIR_REL)
    };
    fs::remove_dir_all(&swap_at).expect("remove real dir before swap");
    symlink(outside.path(), &swap_at).expect("create swap symlink");
    let barrier = Barrier::new(2);
    let result = std::thread::scope(|scope| {
        let writer = scope.spawn(|| {
            barrier.wait();
            vault.write_note("2026-10/race.md", "racing body")
        });
        // The swap above happens-before this wait, which happens-before the
        // writer's release, so the writer always sees the symlink.
        barrier.wait();
        writer.join().expect("writer thread panicked")
    });
    assert!(
        matches!(result, Err(VaultError::Unsafe)),
        "iteration {iteration} swap_memory={swap_memory}: expected Unsafe, got {result:?}"
    );
    assert!(
        all_entries(outside.path()).is_empty(),
        "iteration {iteration} swap_memory={swap_memory}: outside dir must stay empty"
    );
}

#[cfg(unix)]
#[test]
fn race_symlink_swap_before_write_is_refused() {
    for iteration in 0..RACE_ITERATIONS {
        race_swap_once(iteration, true);
        race_swap_once(iteration, false);
    }
}

#[test]
fn race_two_writers_same_rel_exactly_one_wins() {
    for iteration in 0..RACE_ITERATIONS {
        let tmp = tempfile::tempdir().expect("tempdir");
        let vault = open_vault(tmp.path());
        let body_a = format!("writer-A iteration {iteration} {}", "A".repeat(4096));
        let body_b = format!("writer-B iteration {iteration} {}", "B".repeat(4096));
        let barrier = Barrier::new(2);
        let (result_a, result_b) = std::thread::scope(|scope| {
            let a = scope.spawn(|| {
                barrier.wait();
                vault.write_note("2026-10/race.md", &body_a)
            });
            let b = scope.spawn(|| {
                barrier.wait();
                vault.write_note("2026-10/race.md", &body_b)
            });
            (
                a.join().expect("writer A panicked"),
                b.join().expect("writer B panicked"),
            )
        });
        let (winner, loser) = match (result_a, result_b) {
            (Ok(_), Err(err)) => (&body_a, err),
            (Err(err), Ok(_)) => (&body_b, err),
            (Ok(_), Ok(_)) => panic!("iteration {iteration}: both writers reported Ok"),
            (Err(a), Err(b)) => panic!("iteration {iteration}: both writers failed: {a} / {b}"),
        };
        assert!(
            matches!(loser, VaultError::Exists),
            "iteration {iteration}: loser must be Exists, got {loser:?}"
        );
        let target = tmp
            .path()
            .join(NOTES_DIR_REL)
            .join("2026-10")
            .join("race.md");
        let on_disk = fs::read(&target).expect("read winner note");
        assert_eq!(
            on_disk,
            winner.as_bytes(),
            "iteration {iteration}: final bytes must equal the winner's"
        );
    }
}
