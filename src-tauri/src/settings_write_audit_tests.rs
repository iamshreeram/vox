//! Stage 4 A8 static audit (validator round 1, F1): no production code writes the `settings`
//! key of the store except `settings::write_settings` and `settings::write_settings_gated`.
//!
//! The scan reads every `.rs` file under `src/` (walked with `std::fs`, not an `include_str!`
//! list), cuts each file at its first `#[cfg(test)]` as the other source-contract tests do, and
//! allow-lists by file AND function name, never by line number. The needles are written with
//! `concat!` so that this file does not contain them literally and cannot flag itself.

use regex::Regex;
use std::path::{Path, PathBuf};

/// The only (file, function) pairs allowed to write the settings key.
const ALLOWED: [(&str, &str); 2] = [
    ("settings.rs", "write_settings"),
    ("settings.rs", "write_settings_gated"),
];

struct Hit {
    file: String,
    function: Option<String>,
    text: String,
}

/// Patterns that write the settings key: `.set("settings"`, `.delete("settings"` (any receiver,
/// whitespace and line breaks allowed) and `clear()` / `reset()` on a store binding.
fn write_patterns() -> Vec<Regex> {
    vec![
        Regex::new(r#"\.\s*(?:set|delete)\s*\(\s*"settings""#).expect("pattern"),
        Regex::new(r"\bstore\w*\s*\.\s*(?:clear|reset)\s*\(\s*\)").expect("pattern"),
    ]
}

/// The code before the first `#[cfg(test)]`, which is the production part of a file.
fn production_part(text: &str) -> &str {
    match text.find(concat!("#[cfg", "(test)]")) {
        Some(cut) => &text[..cut],
        None => text,
    }
}

/// Name of the last `fn` declared before `pos`, i.e. the function that encloses `pos`.
fn enclosing_fn(code: &str, pos: usize) -> Option<String> {
    let fn_decl = Regex::new(r"\bfn\s+(\w+)").expect("pattern");
    fn_decl
        .captures_iter(&code[..pos])
        .last()
        .map(|caps| caps[1].to_string())
}

/// Every write of the settings key in the production part of `files` (`(path, text)` pairs).
fn scan(files: &[(String, String)]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (file, text) in files {
        let code = production_part(text);
        for pattern in write_patterns() {
            for found in pattern.find_iter(code) {
                hits.push(Hit {
                    file: file.clone(),
                    function: enclosing_fn(code, found.start()),
                    text: found.as_str().to_string(),
                });
            }
        }
    }
    hits
}

fn is_allowed(hit: &Hit) -> bool {
    hit.function.as_deref().is_some_and(|function| {
        ALLOWED
            .iter()
            .any(|(file, allowed)| *file == hit.file && *allowed == function)
    })
}

/// One line per write that is not in the allow-list.
fn violations(files: &[(String, String)]) -> Vec<String> {
    scan(files)
        .iter()
        .filter(|hit| !is_allowed(hit))
        .map(|hit| {
            format!(
                "{}::{} `{}`",
                hit.file,
                hit.function.as_deref().unwrap_or("<module>"),
                hit.text
            )
        })
        .collect()
}

fn collect_sources(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {dir:?}: {e}"));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            collect_sources(root, &path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            let relative = path
                .strip_prefix(root)
                .expect("path under src")
                .to_string_lossy()
                .replace('\\', "/");
            let text =
                std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path:?}: {e}"));
            out.push((relative, text));
        }
    }
}

fn real_sources() -> Vec<(String, String)> {
    let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    collect_sources(&root, &root, &mut out);
    assert!(!out.is_empty(), "the source walk found no files");
    out
}

#[test]
fn only_the_two_gated_writers_write_the_settings_key() {
    let files = real_sources();
    let found = scan(&files);
    let bad = violations(&files);
    assert!(
        bad.is_empty(),
        "the settings key is written outside write_settings / write_settings_gated:\n{}",
        bad.join("\n")
    );
    for (file, function) in ALLOWED {
        assert!(
            found
                .iter()
                .any(|hit| hit.file == file && hit.function.as_deref() == Some(function)),
            "the scan must see the write in {function} (guards against a vacuous pass)"
        );
    }
}

#[test]
fn the_scanner_flags_a_synthetic_offending_write() {
    let set = concat!("store", ".se", "t(\"settings\", v);");
    let delete = concat!("store", ".de", "lete(\"settings\");");
    let clear = concat!("sto", "re.clear();");
    let multi_line = "store\n        .set(\n            \"settings\",\n            v,\n        );";
    let cases = [
        (
            "managers/rogue.rs",
            format!("fn rogue_writer(store: &Store) {{\n    {set}\n}}\n"),
        ),
        (
            "managers/rogue.rs",
            format!("fn rogue_delete(store: &Store) {{\n    {delete}\n}}\n"),
        ),
        (
            "managers/rogue.rs",
            format!("fn rogue_clear(store: &Store) {{\n    {clear}\n}}\n"),
        ),
        (
            "managers/rogue.rs",
            format!("fn rogue_multi(store: &Store) {{\n    {multi_line}\n}}\n"),
        ),
        (
            "settings.rs",
            format!("fn get_settings(store: &Store) {{\n    {set}\n}}\n"),
        ),
    ];
    for (file, source) in cases {
        let bad = violations(&[(file.to_string(), source.clone())]);
        assert_eq!(bad.len(), 1, "one violation expected in {source}: {bad:?}");
    }
}

#[test]
fn the_allowed_writers_and_code_after_the_test_marker_are_not_flagged() {
    let set = concat!("store", ".se", "t(\"settings\", v);");
    let allowed = format!("fn write_settings(app: &AppHandle) {{\n    {set}\n}}\n");
    assert!(violations(&[("settings.rs".into(), allowed)]).is_empty());
    let gated = format!("fn write_settings_gated(app: &AppHandle) {{\n    {set}\n}}\n");
    assert!(violations(&[("settings.rs".into(), gated)]).is_empty());
    let after_test_marker = format!(
        "fn ok() {{}}\n{}\nfn test_only() {{\n    {set}\n}}\n",
        concat!("#[cfg", "(test)]")
    );
    assert!(violations(&[("managers/x.rs".into(), after_test_marker)]).is_empty());
}
