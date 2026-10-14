//! Authoritative tests for `project` (DESIGN Rev 4: R2-3, D4). Do not weaken.
//! RED on purpose: the stubs are `todo!()`, so these fail at runtime.
//! Symlink-dependent tests live in `unix_only` (Windows is not gated here).

use super::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tempfile::TempDir;

fn canon(path: &Path) -> PathBuf {
    fs::canonicalize(path).expect("canonicalize test directory")
}

fn make_dir(path: &Path) {
    fs::create_dir_all(path).expect("create test directory");
}

fn as_root_strings(paths: &[&Path]) -> Vec<String> {
    paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

// ---- roots (R2-3: absolute, existing, directory; invalid roots are skipped) ----

#[test]
fn no_configured_roots_resolves_to_no_roots() {
    let resolver = ProjectResolver::new(&[]);
    assert_eq!(resolver.resolve("proj"), Err(ResolveError::NoRoots));
    assert!(resolver.list_projects().is_empty());
}

#[test]
fn relative_tilde_and_empty_roots_are_skipped() {
    let roots = vec![
        "relative/root".to_string(),
        "~/vox-tester-no-such-dir".to_string(),
        String::new(),
    ];
    let resolver = ProjectResolver::new(&roots);
    assert!(
        matches!(resolver.resolve("proj"), Err(ResolveError::NoRoots)),
        "all-invalid roots must behave as no roots"
    );
    assert!(resolver.list_projects().is_empty());
}

#[test]
fn nonexistent_absolute_root_is_skipped() {
    let tmp = TempDir::new().expect("tempdir");
    let missing = tmp.path().join("missing");
    let resolver = ProjectResolver::new(&[missing.to_string_lossy().into_owned()]);
    assert!(matches!(
        resolver.resolve("proj"),
        Err(ResolveError::NoRoots)
    ));
}

#[test]
fn root_that_is_a_regular_file_is_skipped() {
    let tmp = TempDir::new().expect("tempdir");
    let file = tmp.path().join("not-a-dir");
    fs::write(&file, b"x").expect("write file");
    let resolver = ProjectResolver::new(&[file.to_string_lossy().into_owned()]);
    assert!(matches!(
        resolver.resolve("proj"),
        Err(ResolveError::NoRoots)
    ));
}

#[test]
fn valid_root_still_works_next_to_invalid_roots() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    let proj = root.join("proj");
    make_dir(&proj);
    let roots = vec![
        "relative/root".to_string(),
        tmp.path().join("missing").to_string_lossy().into_owned(),
        root.to_string_lossy().into_owned(),
    ];
    let resolver = ProjectResolver::new(&roots);
    assert_eq!(resolver.resolve("proj"), Ok(canon(&proj)));
}

#[test]
fn resolves_plain_project_folder_to_its_canonical_path() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    let proj = root.join("proj");
    make_dir(&proj);
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    assert_eq!(resolver.resolve("proj"), Ok(canon(&proj)));
}

#[test]
fn missing_project_name_is_not_found() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    assert_eq!(resolver.resolve("nope"), Err(ResolveError::NotFound));
}

#[test]
fn regular_file_with_the_project_name_is_not_a_project() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    fs::write(root.join("notdir"), b"x").expect("write file");
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    assert!(resolver.resolve("notdir").is_err());
}

#[test]
fn project_name_match_is_case_insensitive() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    make_dir(&root.join("Proj"));
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    let resolved = resolver.resolve("proj").expect("case-insensitive match");
    let name = resolved
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    assert_eq!(name, "proj");
}

// ---- ambiguity (R2-3) ----

#[test]
fn same_name_in_two_roots_is_ambiguous_and_never_picks_one() {
    let a = TempDir::new().expect("tempdir a");
    let b = TempDir::new().expect("tempdir b");
    make_dir(&canon(a.path()).join("proj"));
    make_dir(&canon(b.path()).join("proj"));
    let resolver = ProjectResolver::new(&as_root_strings(&[a.path(), b.path()]));
    assert_eq!(resolver.resolve("proj"), Err(ResolveError::Ambiguous(2)));
}

#[test]
fn ambiguity_error_does_not_leak_sibling_root_paths() {
    let a = TempDir::new().expect("tempdir a");
    let b = TempDir::new().expect("tempdir b");
    let a_root = canon(a.path());
    let b_root = canon(b.path());
    make_dir(&a_root.join("proj"));
    make_dir(&b_root.join("proj"));
    let resolver = ProjectResolver::new(&as_root_strings(&[a.path(), b.path()]));
    let err = resolver.resolve("proj").expect_err("ambiguous name");
    let shown = format!("{err:?}");
    assert!(
        !shown.contains(&*a_root.to_string_lossy()),
        "error leaked root A: {shown}"
    );
    assert!(
        !shown.contains(&*b_root.to_string_lossy()),
        "error leaked root B: {shown}"
    );
}

// ---- name validation (R2-3: bare NAME, no separators, no dot-dot, no NUL, <= 255 bytes) ----

#[test]
fn names_with_separators_dot_dot_nul_or_empty_are_invalid() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    make_dir(&root.join("proj"));
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    for name in ["..", ".", "a/b", "a/../proj", "/abs", "proj/", "", "a\0b"] {
        assert_eq!(
            resolver.resolve(name),
            Err(ResolveError::InvalidName),
            "name {name:?} must be InvalidName"
        );
    }
}

#[test]
fn name_longer_than_255_bytes_is_invalid_and_255_bytes_is_allowed() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    assert_eq!(
        resolver.resolve(&"x".repeat(256)),
        Err(ResolveError::InvalidName)
    );
    // Boundary: exactly 255 bytes is a valid name, merely not present here.
    assert_eq!(
        resolver.resolve(&"x".repeat(255)),
        Err(ResolveError::NotFound)
    );
}

#[test]
fn byte_length_limit_counts_utf8_bytes_not_characters() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    // U+00E9 is two bytes: 128 of them is 256 bytes.
    assert_eq!(
        resolver.resolve(&"\u{e9}".repeat(128)),
        Err(ResolveError::InvalidName)
    );
    // 127 * 2 + 1 = 255 bytes.
    let boundary = format!("{}a", "\u{e9}".repeat(127));
    assert_eq!(resolver.resolve(&boundary), Err(ResolveError::NotFound));
}

#[test]
fn unicode_project_name_resolves() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    let name = "r\u{e9}sum\u{e9}-\u{65e5}\u{672c}";
    make_dir(&root.join(name));
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    assert_eq!(resolver.resolve(name), Ok(canon(&root.join(name))));
}

#[test]
fn unicode_lookalike_names_do_not_match_an_ascii_project() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    make_dir(&root.join("proj"));
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    // Fullwidth letters and a zero-width space are different names.
    let fullwidth = "\u{ff50}\u{ff52}\u{ff4f}\u{ff4a}";
    let zero_width = "pro\u{200b}j";
    assert_eq!(resolver.resolve(fullwidth), Err(ResolveError::NotFound));
    assert_eq!(resolver.resolve(zero_width), Err(ResolveError::NotFound));
}

// ---- list_projects (R2-3: plain directories, <= 50 names, bounded reads) ----

#[test]
fn list_projects_is_sorted_and_lists_plain_directories_only() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    make_dir(&root.join("beta"));
    make_dir(&root.join("alpha"));
    fs::write(root.join("gamma.txt"), b"x").expect("write file");
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    assert_eq!(resolver.list_projects(), vec!["alpha", "beta"]);
}

#[test]
fn list_projects_caps_the_result_at_fifty_names() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    for i in 0..60 {
        make_dir(&root.join(format!("d{i:02}")));
    }
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    let expected: Vec<String> = (0..50).map(|i| format!("d{i:02}")).collect();
    assert_eq!(resolver.list_projects(), expected);
}

#[test]
fn list_projects_merges_roots_in_sorted_order_without_duplicates() {
    let a = TempDir::new().expect("tempdir a");
    let b = TempDir::new().expect("tempdir b");
    let a_root = canon(a.path());
    let b_root = canon(b.path());
    make_dir(&a_root.join("m1"));
    make_dir(&a_root.join("z1"));
    make_dir(&b_root.join("a1"));
    make_dir(&b_root.join("m2"));
    let resolver = ProjectResolver::new(&as_root_strings(&[a.path(), b.path()]));
    assert_eq!(resolver.list_projects(), vec!["a1", "m1", "m2", "z1"]);
}

#[test]
fn list_projects_on_a_20000_entry_root_returns_promptly() {
    let tmp = TempDir::new().expect("tempdir");
    let root = canon(tmp.path());
    for i in 0..20_000 {
        fs::write(root.join(format!("f{i:05}")), b"").expect("write entry");
    }
    let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
    let started = Instant::now();
    let names = resolver.list_projects();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_secs(2),
        "list_projects took {elapsed:?} on a 20000-entry root"
    );
    assert!(names.len() <= 50, "returned {} names", names.len());
}

#[cfg(unix)]
mod unix_only {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn symlinked_project_child_is_refused_as_not_plain_folder() {
        let tmp = TempDir::new().expect("tempdir");
        let root = canon(tmp.path());
        let external = TempDir::new().expect("external tempdir");
        symlink(canon(external.path()), root.join("linked")).expect("create symlink");
        let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
        assert_eq!(
            resolver.resolve("linked"),
            Err(ResolveError::NotPlainFolder)
        );
    }

    #[test]
    fn symlinked_project_child_is_not_listed() {
        let tmp = TempDir::new().expect("tempdir");
        let root = canon(tmp.path());
        let external = TempDir::new().expect("external tempdir");
        make_dir(&root.join("real"));
        symlink(canon(external.path()), root.join("linked")).expect("create symlink");
        let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
        assert_eq!(resolver.list_projects(), vec!["real"]);
    }

    #[test]
    fn symlinked_root_is_canonicalized_once_and_still_resolves() {
        let real = TempDir::new().expect("real tempdir");
        let real_root = canon(real.path());
        make_dir(&real_root.join("proj"));
        let holder = TempDir::new().expect("holder tempdir");
        let link_root = canon(holder.path()).join("root-link");
        symlink(&real_root, &link_root).expect("create root symlink");
        let resolver = ProjectResolver::new(&[link_root.to_string_lossy().into_owned()]);
        let resolved = resolver.resolve("proj").expect("symlinked root resolves");
        assert!(
            resolved.starts_with(&real_root),
            "resolved path {resolved:?} must be under the canonical root {real_root:?}"
        );
    }

    #[test]
    fn list_projects_skips_symlinked_root_entries_and_files() {
        let tmp = TempDir::new().expect("tempdir");
        let root = canon(tmp.path());
        let external = TempDir::new().expect("external tempdir");
        make_dir(&root.join("alpha"));
        fs::write(root.join("beta.txt"), b"x").expect("write file");
        symlink(canon(external.path()), root.join("gamma")).expect("create symlink");
        let resolver = ProjectResolver::new(&[root.to_string_lossy().into_owned()]);
        assert_eq!(resolver.list_projects(), vec!["alpha"]);
    }
}
