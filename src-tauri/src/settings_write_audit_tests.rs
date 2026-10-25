//! Stage 4 A8 static audit (validator round 1, F1): no production code writes the `settings`
//! key of the store except `settings::write_settings` and `settings::write_settings_gated`.
//!
//! The scan reads every `.rs` file under `src/` (walked with `std::fs`, not an `include_str!`
//! list), removes every `#[cfg(test)] mod` block (brace-matched, with string, char and comment
//! contents skipped), skips `tests.rs` / `*_tests.rs` files, and
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

/// Whether a file is a test file (`tests.rs` or `*_tests.rs`), which is not production code.
fn is_test_file(file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file);
    name == "tests.rs" || name.ends_with("_tests.rs")
}

fn is_ident(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Blanks `out[start..end]` to spaces, keeping line breaks.
fn blank(out: &mut [char], start: usize, end: usize) {
    for c in &mut out[start..end] {
        if *c != '\n' {
            *c = ' ';
        }
    }
}

fn line_comment_end(raw: &[char], start: usize) -> usize {
    raw[start..]
        .iter()
        .position(|&c| c == '\n')
        .map_or(raw.len(), |offset| start + offset)
}

/// End of a (nested) block comment that starts at `start`.
fn block_comment_end(raw: &[char], start: usize) -> usize {
    let mut depth = 0usize;
    let mut j = start;
    while j < raw.len() {
        if raw[j] == '/' && raw.get(j + 1) == Some(&'*') {
            depth += 1;
            j += 2;
        } else if raw[j] == '*' && raw.get(j + 1) == Some(&'/') {
            depth -= 1;
            j += 2;
            if depth == 0 {
                return j;
            }
        } else {
            j += 1;
        }
    }
    raw.len()
}

/// End of a plain or byte string literal whose opening quote is at `start`.
fn string_end(raw: &[char], start: usize) -> usize {
    let mut j = start + 1;
    while j < raw.len() {
        match raw[j] {
            '\\' => j += 2,
            '"' => return j + 1,
            _ => j += 1,
        }
    }
    raw.len()
}

/// End of a raw string (`r".."`, `r#".."#`, `br".."`) whose `r` is at `start`; `None` when the
/// `r` does not open a raw string (an identifier, or a raw identifier such as `r#type`).
fn raw_string_end(raw: &[char], start: usize) -> Option<usize> {
    let prefix_is_free = match start.checked_sub(1).map(|p| raw[p]) {
        None => true,
        Some('b') => !start.checked_sub(2).is_some_and(|p| is_ident(raw[p])),
        Some(c) => !is_ident(c),
    };
    if !prefix_is_free {
        return None;
    }
    let hashes = raw[start + 1..].iter().take_while(|&&c| c == '#').count();
    let quote = start + 1 + hashes;
    if raw.get(quote) != Some(&'"') {
        return None;
    }
    let mut j = quote + 1;
    while j < raw.len() {
        let closing = raw[j] == '"'
            && raw[j + 1..]
                .iter()
                .take(hashes)
                .take_while(|&&c| c == '#')
                .count()
                == hashes;
        if closing {
            return Some(j + 1 + hashes);
        }
        j += 1;
    }
    Some(raw.len())
}

/// End of a char literal opened at `start`; `None` for a lifetime such as `'a`.
fn char_literal_end(raw: &[char], start: usize) -> Option<usize> {
    match raw.get(start + 1)? {
        '\\' => (start + 3..raw.len().min(start + 12))
            .find(|&j| raw[j] == '\'')
            .map(|j| j + 1),
        _ if raw.get(start + 2) == Some(&'\'') => Some(start + 3),
        _ => None,
    }
}

/// The code with the contents of comments and of string and char literals blanked (line breaks
/// kept), so braces and attributes inside them are invisible to the structure scan. Same length.
fn mask(raw: &[char]) -> Vec<char> {
    let mut out = raw.to_vec();
    let mut i = 0;
    while i < raw.len() {
        let span = match raw[i] {
            '/' if raw.get(i + 1) == Some(&'/') => Some(line_comment_end(raw, i)),
            '/' if raw.get(i + 1) == Some(&'*') => Some(block_comment_end(raw, i)),
            'r' => raw_string_end(raw, i),
            '"' => Some(string_end(raw, i)),
            '\'' => char_literal_end(raw, i),
            _ => None,
        };
        match span {
            Some(end) => {
                blank(&mut out, i, end);
                i = end;
            }
            None => i += 1,
        }
    }
    out
}

/// Whether the masked text has the word `word` at `at`, not followed by an identifier character.
fn word_at(masked: &[char], at: usize, word: &str) -> bool {
    let chars: Vec<char> = word.chars().collect();
    masked.get(at..at + chars.len()) == Some(chars.as_slice())
        && !masked.get(at + chars.len()).is_some_and(|&c| is_ident(c))
}

fn skip_ws(masked: &[char], mut j: usize) -> usize {
    while j < masked.len() && masked[j].is_whitespace() {
        j += 1;
    }
    j
}

/// Index of the brace that closes the one at `open` (the masked text has no literal braces).
fn matching_brace(masked: &[char], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, &c) in masked.iter().enumerate().skip(open) {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// End (exclusive) of the `mod name { .. }` or `mod name;` item that follows a `#[cfg(test)]`
/// attribute ending at `from`. `None` when the attributed item is not a module (it is scanned).
fn test_module_end(masked: &[char], from: usize) -> Option<usize> {
    let mut j = skip_ws(masked, from);
    if word_at(masked, j, "pub") {
        j = skip_ws(masked, j + 3);
        if masked.get(j) == Some(&'(') {
            j += masked[j..].iter().position(|&c| c == ')')? + 1;
        }
        j = skip_ws(masked, j);
    }
    if !word_at(masked, j, "mod") {
        return None;
    }
    j = skip_ws(masked, j + 3);
    let name_start = j;
    while j < masked.len() && is_ident(masked[j]) {
        j += 1;
    }
    if j == name_start {
        return None;
    }
    j = skip_ws(masked, j);
    match masked.get(j)? {
        ';' => Some(j + 1),
        '{' => matching_brace(masked, j).map(|close| close + 1),
        _ => None,
    }
}

/// The production code of a file: the text with every `#[cfg(test)] mod` block (and every
/// `#[cfg(test)] mod name;` declaration) blanked. Other `#[cfg(test)]` items are kept and scanned
/// like production code; scanning more is conservative, scanning less would hide a writer.
fn strip_test_modules(text: &str) -> String {
    let raw: Vec<char> = text.chars().collect();
    let masked = mask(&raw);
    let marker: Vec<char> = concat!("#[cfg", "(test)]").chars().collect();
    let mut out = raw.clone();
    let mut i = 0;
    while i < masked.len() {
        let end = if masked[i..].starts_with(&marker) {
            test_module_end(&masked, i + marker.len())
        } else {
            None
        };
        match end {
            Some(end) => {
                blank(&mut out, i, end);
                i = end;
            }
            None => i += 1,
        }
    }
    out.into_iter().collect()
}

/// Whether the text before a literal ends with a store read or write call, so that the literal
/// is that call's key argument.
fn follows_store_call(before: &str) -> bool {
    let before = before.trim_end();
    [".get(", ".set(", ".delete("]
        .iter()
        .any(|call| before.ends_with(call))
}

/// A `"settings"` literal outside settings.rs, in a file that writes through `.set(` or
/// `.delete(`, is an indirect write key (`let key = "settings"; store.set(key, ..)`). The key
/// argument of a store call is excluded: a `.get(` key is a read, and a direct `.set(`/`.delete(`
/// key is already reported by `write_patterns`.
fn settings_literal_hits(file: &str, code: &str) -> Vec<Hit> {
    let writes = Regex::new(r"\.\s*(?:set|delete)\s*\(").expect("pattern");
    if file == "settings.rs" || !writes.is_match(code) {
        return Vec::new();
    }
    code.match_indices("\"settings\"")
        .filter(|(pos, _)| !follows_store_call(&code[..*pos]))
        .map(|(pos, text)| Hit {
            file: file.to_string(),
            function: enclosing_fn(code, pos),
            text: text.to_string(),
        })
        .collect()
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
        if is_test_file(file) {
            continue;
        }
        let code = strip_test_modules(text);
        for pattern in write_patterns() {
            for found in pattern.find_iter(&code) {
                hits.push(Hit {
                    file: file.clone(),
                    function: enclosing_fn(&code, found.start()),
                    text: found.as_str().to_string(),
                });
            }
        }
        hits.extend(settings_literal_hits(file, &code));
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
        "fn ok() {{}}\n{}\nmod tests {{\n    fn test_only() {{\n        {set}\n    }}\n}}\n",
        concat!("#[cfg", "(test)]")
    );
    assert!(violations(&[("managers/x.rs".into(), after_test_marker)]).is_empty());
}

#[test]
fn a_test_use_line_before_an_offending_write_does_not_hide_it() {
    let source = r#"#[cfg(test)]
use crate::x;
fn rogue(store: &Store) {
    store.set("settings", v);
}
"#;
    let bad = violations(&[("managers/rogue.rs".to_string(), source.to_string())]);
    assert_eq!(
        bad.len(),
        1,
        "a cfg(test) use line must not hide a write: {bad:?}"
    );
    assert!(
        bad[0].contains("rogue"),
        "the write is attributed to rogue: {bad:?}"
    );
}

#[test]
fn an_offending_write_inside_a_test_module_is_ignored() {
    let source = r#"fn ok() {}
#[cfg(test)]
mod tests {
    fn t(store: &Store) {
        store.set("settings", v);
    }
}
"#;
    let bad = violations(&[("managers/x.rs".to_string(), source.to_string())]);
    assert!(
        bad.is_empty(),
        "test-module code is not production code: {bad:?}"
    );
}

#[test]
fn an_offending_write_after_a_nested_brace_test_module_is_flagged() {
    let source = r#"fn ok() {}
#[cfg(test)]
mod tests {
    fn t() {
        if true {
            let s = "}";
        }
    }
}
fn rogue(store: &Store) {
    store.set("settings", v);
}
"#;
    let bad = violations(&[("managers/x.rs".to_string(), source.to_string())]);
    assert_eq!(
        bad.len(),
        1,
        "the write after the test module must be flagged: {bad:?}"
    );
    assert!(bad[0].contains("rogue"), "{bad:?}");
}

#[test]
fn an_indirect_settings_key_outside_settings_rs_is_flagged_when_the_file_writes() {
    let source = r#"fn rogue(store: &Store) {
    let key = "settings";
    store.set(key, v);
}
"#;
    let bad = violations(&[("managers/rogue.rs".to_string(), source.to_string())]);
    assert_eq!(
        bad.len(),
        1,
        "an indirect settings key must be flagged: {bad:?}"
    );
}

#[test]
fn a_settings_read_outside_settings_rs_is_not_flagged() {
    let source = "fn read(store: &Store) {\n    let v = store.get(\"settings\");\n}\n";
    let bad = violations(&[("managers/reader.rs".to_string(), source.to_string())]);
    assert!(bad.is_empty(), "reads are fine: {bad:?}");
}
