//! Contract tests for `managers::redact::redact`.
//!
//! Ported case-for-case from the reference scrubber's suite
//! (cpvault `tests/test_scrub.py`, 54 test methods / 756 subtest executions)
//! against `cpvault/scrub.py`. Every fixture is synthetic: fake tokens in the
//! real format, and neutral `example.com` / `demo` names in place of any
//! employer or personal identifiers.
//!
//! Red-first: until `redact` is implemented, every test that calls it panics
//! on `todo!()`. Each table case prints its own index and input on failure.

use crate::managers::redact::redact;
use std::time::Instant;

const MARKER: &str = "<redacted:";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// JSON-encode a string the way Python's json.dumps does for ASCII content.
fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn assert_absent(label: &str, input: &str, out: &str, secret: &str) {
    assert!(
        !out.contains(secret),
        "{label}: secret {secret:?} survived. input={input:?} output={out:?}"
    );
}

/// Secret must be gone, and no quote/paren/bracket fragment may stay glued to it.
fn assert_no_fragment(label: &str, input: &str, out: &str, secret: &str, glue: &[char]) {
    assert_absent(label, input, out, secret);
    for &c in glue {
        let after = format!("{secret}{c}");
        let before = format!("{c}{secret}");
        assert!(
            !out.contains(&after) && !out.contains(&before),
            "{label}: fragment {c:?} left around secret {secret:?}. input={input:?} output={out:?}"
        );
    }
}

/// Context wrappers for line-shaped cases: bare, inside a sentence, inside a block.
fn line_contexts(line: &str) -> Vec<(&'static str, String)> {
    vec![
        ("bare", line.to_string()),
        (
            "sentence",
            format!("Here is the line {line} inside a sentence, thanks."),
        ),
        ("block", format!("first line\n{line}\nlast line")),
    ]
}

/// Context wrappers for seeded secrets: six surroundings, as in the Python suite.
fn seeded_contexts(value: &str) -> Vec<(&'static str, String)> {
    vec![
        ("bare", value.to_string()),
        (
            "prose",
            format!("Here is the value {value} inside a sentence, thanks."),
        ),
        (
            "json_string",
            format!("{{\"msg\": {}}}", json_str(&format!("debug {value}"))),
        ),
        ("parens", format!("call(({value})) done")),
        (
            "quoted_punct",
            format!("\"{value}\", [{value}]. ({value}!)"),
        ),
        ("multiline", format!("first line\n{value}\nlast line")),
    ]
}

/// Mirrors the Python `_assert_gone` helper, kind by kind.
fn assert_seeded_gone(label: &str, kind: &str, value: &str, input: &str, out: &str) {
    match kind {
        "pem" => {
            let lines: Vec<&str> = value.lines().collect();
            for line in &lines[1..lines.len() - 1] {
                assert_absent(label, input, out, line);
            }
        }
        "jwt" => {
            for part in value.split('.') {
                assert_absent(label, input, out, part);
            }
        }
        "url_userinfo" => {
            assert_absent(label, input, out, "pa55w0rd");
            assert_absent(label, input, out, "user:");
            assert!(
                out.contains("example.com"),
                "{label}: host dropped. output={out:?}"
            );
        }
        "email" => {
            assert_absent(label, input, out, "someone@example.com");
            assert!(
                out.contains("<email>"),
                "{label}: no <email> marker. output={out:?}"
            );
        }
        "password_json" => {
            for word in ["hunter", "two", "three"] {
                assert_absent(label, input, out, word);
            }
        }
        "basic_auth" => assert_absent(label, input, out, "dXNlcjpwYXNz"),
        _ => {}
    }
    assert_absent(label, input, out, value);
}

/// Time one redact call against a hard limit.
fn timed_redact(label: &str, text: &str, limit_secs: f64) -> String {
    let start = Instant::now();
    let out = redact(text);
    let elapsed = start.elapsed().as_secs_f64();
    assert!(
        elapsed < limit_secs,
        "{label}: took {elapsed:.3}s, limit {limit_secs}s"
    );
    out
}

// ---------------------------------------------------------------------------
// Fixtures (all FAKE; values mirror the Python fixture file exactly)
// ---------------------------------------------------------------------------

/// SEEDED_SECRETS from the Python helpers, same keys and order.
const SEEDED: &[(&str, &str)] = &[
    ("jwt", "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c"),
    ("bearer", "Bearer a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5"),
    ("password_json", "\"password\": \"hunter two three\""),
    ("api_key_assign", "api_key=abcd1234efgh5678ijkl"),
    ("aws", concat!("AKIA", "IOSFODNN7EXAMPLE")),
    ("github", concat!("ghp_", "a1b2c3d4e5f6g7h8i9j0K1L2M3N4O5P6Q7R8")),
    ("slack", concat!("xoxb-", "1234567890-1234567890123-AbCdEfGhIjKlMnOpQrStUvWx")),
    ("pem", "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCZmFrZWtleW1hdGVyaWFsZm9ydGVzdHM=\nZmFrZWZha2VmYWtlZmFrZWZha2VmYWtlZmFrZWZha2VmYWtlZmFrZWZha2U=\n-----END PRIVATE KEY-----"),
    ("url_userinfo", "https://user:pa55w0rd@example.com/x"),
    ("hex64", "9c2f4e1ab7d05836e21fc4a9b8d37560e4a1f29b8c7d03e5a6b9f2c1d4e8a073"),
    ("hex40_sha", "commit 3f786850e387550fdab836ed7e6dc881de23001b"),
    ("b64_blob", "k3Jx9Lm2Pq8Rt4Vw7Yz1Bc6Df0Gh5Kj3Nn8Qs2Tu9Xa4Ec7Ib1Oe6Mr0Sl5WpAb3Z"),
    ("email", "someone@example.com"),
    ("basic_auth", "Authorization: Basic dXNlcjpwYXNz"),
    ("sk_ant", concat!("sk-ant-", "api03-a1B2c3D4e5F6g7H8i9J0k1L2m3N4o5P6q7R8s9T0")),
];

/// PREFIX_TOKENS from the Python suite (19 entries).
const PREFIX_TOKENS: &[(&str, &str)] = &[
    ("openai_sk", "sk-proj-AbCdEf0123GhIjKl4567MnOpQr89StUvWx"),
    (
        "anthropic_sk_ant",
        concat!("sk-ant-", "api03-Zx9Qw8Er7Ty6Ui5Op4As3Df2Gh1Jk0Lz"),
    ),
    (
        "google_aiza",
        concat!("AIza", "SyD4f8G2h6J0kL3mN7pQ1rS5tU9vW2xY4zA"),
    ),
    (
        "google_ya29",
        "ya29.a0AfH6SMBx7Qk2Lp9Vw4Zt1Rn8Ms3Dg5Yc0Ub6Hj",
    ),
    ("gitlab_glpat", concat!("glpat-", "x9Kq2Lm7Rt4Vw8Zy1Bn6")),
    ("npm", "npm_a7Kd9Fg2Hj4Kl6Zx8Cv1Bn3Mq5Wp0Rt2Ys4Uj6"),
    (
        "pypi",
        "pypi-AgEIcHlwaS5vcmcCJGFiY2RlZjAxMjM0NTY3ODlhYmNkZWZnaGlqa2xtbm9w",
    ),
    (
        "slack_xoxa",
        concat!(
            "xoxa-",
            "2345678901-9876543210-4433221100-a1b2c3d4e5f6a7b8c9d0e1f2"
        ),
    ),
    (
        "slack_xoxp",
        concat!(
            "xoxp-",
            "2345678901-9876543210-4433221100-a1b2c3d4e5f6a7b8c9d0e1f2"
        ),
    ),
    (
        "slack_xoxr",
        concat!(
            "xoxr-",
            "2345678901-9876543210-4433221100-a1b2c3d4e5f6a7b8c9d0e1f2"
        ),
    ),
    (
        "slack_xoxs",
        concat!(
            "xoxs-",
            "2345678901-9876543210-4433221100-a1b2c3d4e5f6a7b8c9d0e1f2"
        ),
    ),
    (
        "github_ghp",
        concat!("ghp_", "A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
    ),
    ("github_gho", "gho_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
    ("github_ghu", "ghu_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
    ("github_ghs", "ghs_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
    ("github_ghr", "ghr_A1b2C3d4E5f6G7h8I9j0K1l2M3n4O5p6Q7r8"),
    (
        "github_pat",
        "github_pat_11AB7cD9eF2gH4iJ6kL8mN_oP1qR3sT5uV7wX9yZ0aB2cD4eF6gH8iJ",
    ),
    ("aws_akia", concat!("AKIA", "J7QXYB3RNPK2WHTD")),
    ("aws_asia", "ASIAY34FZKBOKMUTVV7A"),
];

/// sha256(b"vault-fixture-{0,1,2}"), precomputed.
const NONUNIFORM_HEX64: [&str; 3] = [
    "408cfc1e1febbb3662055aafbfceca72f6514636e445a3a1831f3d79282ba2f4",
    "169746a644c9d60c59857b770502abd67d86a06977a6eab707c4c0c113c3c7cf",
    "16ade717863a5d292920ba6e4ffa8dac3052d60c0b85dc7527428667e017ceb2",
];
/// md5(b"vault-fixture-4").
const HEX32: &str = "ee64667ba2f0ddbf9c7e2b9595b07187";
/// sha384(b"hello")[:48].
const HEX48: &str = "59e1748777448c69de6b800d7a33bbfb9ff1b463e44354c3";
const GIT_SHA40: &str = "3f786850e387550fdab836ed7e6dc881de23001b";

/// FALSE_POSITIVES from the Python suite: each must come back byte-identical.
const FALSE_POSITIVES: &[&str] = &[
    "The build finished without errors, so we can ship the release tomorrow.",
    "Please open /Users/x/proj/src/main.rs and check the parser.",
    "short hash 3f78685 and 3f786850e387 are fine",
    "uuid 123e4567-e89b-12d3-a456-426614174000 is an id",
    "def parse_session_summary_v2(messages): return messages",
    "fn build_digest_from_messages() -> Result<Digest, Error>",
    "parse_session_summary_for_the_export_xxxxxxxx",
    "release version 1.2.3 is out",
    "see [the docs](https://example.com/docs) for details",
    "let token_count = 5;",
    "reset your password",
];

const PROSE_SENTENCE: &str = "The quick brown fox jumps over the lazy dog while the build runs. ";

/// PATH_CORPUS, anonymised: usernames, employer names and repo names replaced.
const PATH_CORPUS: &[&str] = &[
    "/Users/dev42/work/projects/rust/demo/src/managers/converse_platform.rs",
    "/Users/dev42/Library/CloudStorage/OneDrive-AcmeInc/work/projects/agent/brain/brain/memory/loader.py",
    "~/work/projects/rust/demo/.worktrees/overlay-ux2/src-tauri/src/lib.rs",
    "src-tauri/src/managers/converse_platform_v2/handlers/2026-10-13/session.rs",
    "https://example.com/a/very/long/path/v2/with/many/segments/and-more-words/index.html",
    "/Users/dev42/work/projects/rust/demo/src/tests/fixtures",
    r"C:\Users\dev42\Documents\project\src\managers\main.rs",
    "/var/folders/7k/9xq2m1_w0rn4tz8p6hd3c5vb0000gn/T/cpvault-run-20261013/session.json",
    "/Users/dev42/work/projects/demo-vault/cpvault/scrub.py",
];

/// IDENT_CORPUS, anonymised.
const IDENT_CORPUS: &[&str] = &[
    "com.acme.work.demo.converse.platform.managers.v2.OverlayController",
    "demo-overlay-ux-worktree-2026-10-13-converse-platform-build-042",
];

/// Standard base64 64-char secrets (one or two '/', a '+', '=' padding).
const B64_SECRETS_64: &[&str] = &[
    "Qz7kP2mRf9Lw4TxB8nYc1Vh6Ds3Jg0Ka5Ue+Wi/Nq2Om8Xr4Zt6Ab1Cy7Ef9Gh==",
    "Xr9vT2kLq8Wm4Zp1Hd6Nb3Gs7Yc0Fe5Ju/Pa8Rk2+Mw9Ql4VTn7Ei1Zo6Ub3K/==",
    "Mq3Lz8Yt1Ra6Ws0Ke4Pn9Dv2Hf7Gx5BjCu+8Ok1Tm/3Ye6IwNs2Qb9Rl4Zh7Ea0=",
];

/// 48-char base64url secret using '-' and '_'.
const B64URL_SECRET_48: &str = "Hw3_Kz8-Qm1Vt6RxLd9Ny4-Ps0Jc7_UfAb2Gi5Ev8Oq3-Zh6";

const SEED_TOKEN: &str = "abcd1234efgh5678ijkl";

// ---------------------------------------------------------------------------
// Python: TestSeededSecrets (3 methods, 84 + 0 + 0 subtests)
// ---------------------------------------------------------------------------

#[test]
fn every_seeded_secret_does_not_survive_in_any_context() {
    let mut checked = 0;
    for (i, (kind, value)) in SEEDED.iter().enumerate() {
        if *kind == "hex40_sha" {
            continue; // git sha: PRESERVED, covered by the git_sha_* tests
        }
        for (ctx, text) in seeded_contexts(value) {
            let label = format!("seeded[{i}] kind={kind} ctx={ctx}");
            let out = redact(&text);
            assert_seeded_gone(&label, kind, value, &text, &out);
            checked += 1;
        }
    }
    assert_eq!(
        checked, 84,
        "Python subtest count is 84 (14 kinds x 6 contexts)"
    );
}

#[test]
fn email_becomes_email_marker() {
    assert_eq!(redact("someone@example.com"), "<email>");
}

#[test]
fn jwt_is_replaced_with_a_marker() {
    let out = redact(&format!("token {} end", SEEDED[0].1));
    assert!(out.contains(MARKER), "no marker. output={out:?}");
}

// ---------------------------------------------------------------------------
// Python: TestQuotedAndAssignmentStyles (9 methods)
// ---------------------------------------------------------------------------

#[test]
fn mixed_case_json_password_is_removed() {
    let out = redact("{\"Password\": \"short-secret\"}");
    assert!(!out.contains("short-secret"), "output={out:?}");
}

#[test]
fn json_api_key_without_space_is_removed() {
    let out = redact("{\"API_KEY\":\"xyz\"}");
    assert!(!out.contains("xyz"), "output={out:?}");
}

#[test]
fn single_quoted_token_with_spaces_is_removed() {
    let out = redact("{'Token': 'a b c'}");
    assert!(!out.contains("a b c"), "output={out:?}");
    assert!(!out.contains("b c"), "output={out:?}");
}

#[test]
fn quoted_password_with_spaces_is_removed_entirely() {
    let out = redact("config {\"password\": \"correct horse battery\"} done");
    for word in ["correct", "horse", "battery"] {
        assert!(
            !out.contains(word),
            "word {word:?} survived. output={out:?}"
        );
    }
}

#[test]
fn yaml_password_is_removed() {
    let out = redact("password: abc");
    assert!(!out.contains("abc"), "output={out:?}");
}

#[test]
fn env_secret_key_export_is_removed() {
    let out = redact("export SECRET_KEY=abc");
    assert!(!out.contains("abc"), "output={out:?}");
}

#[test]
fn authorization_basic_header_is_removed() {
    let out = redact("Authorization: Basic dXNlcjpwYXNz");
    assert!(!out.contains("dXNlcjpwYXNz"), "output={out:?}");
}

#[test]
fn authorization_bearer_header_is_removed() {
    let out = redact("Authorization: Bearer abcdefghijklmnopqrstu");
    assert!(!out.contains("abcdefghijklmnopqrstu"), "output={out:?}");
}

#[test]
fn authorization_token_header_is_removed() {
    let secret = "ghtkn_9f8e7d6c5b4a39281706f5e4d3c2b1a0";
    let out = redact(&format!("Authorization: Token {secret}"));
    assert!(!out.contains(secret), "output={out:?}");
}

// ---------------------------------------------------------------------------
// Python: TestKnownPrefixes (1 method, 38 subtests = 19 prefixes x 2 contexts)
// ---------------------------------------------------------------------------

#[test]
fn every_known_prefix_token_is_removed() {
    let mut checked = 0;
    for (i, (name, token)) in PREFIX_TOKENS.iter().enumerate() {
        let contexts = [
            (
                "prose",
                format!("The credential {token} was pasted in the log."),
            ),
            ("json", format!("{{\"auth\": {}}}", json_str(token))),
        ];
        for (ctx, text) in contexts {
            let out = redact(&text);
            assert_absent(&format!("prefix[{i}] {name} ctx={ctx}"), &text, &out, token);
            checked += 1;
        }
    }
    assert_eq!(checked, 38);
}

// ---------------------------------------------------------------------------
// Python: TestHexRuns (3 methods)
// ---------------------------------------------------------------------------

#[test]
fn nonuniform_64_hex_runs_are_redacted() {
    for (i, digest) in NONUNIFORM_HEX64.iter().enumerate() {
        let text = format!("digest {digest} end");
        let out = redact(&text);
        assert_absent(&format!("hex64[{i}]"), &text, &out, digest);
    }
}

#[test]
fn hex_32_run_is_redacted() {
    let text = format!("key {HEX32} end");
    let out = redact(&text);
    assert_absent("hex32", &text, &out, HEX32);
}

#[test]
fn hex_48_run_is_redacted() {
    let text = format!("key {HEX48} end");
    let out = redact(&text);
    assert_absent("hex48", &text, &out, HEX48);
}

// ---------------------------------------------------------------------------
// Python: TestGitShaPreserved (3 methods)
// ---------------------------------------------------------------------------

#[test]
fn sha_after_commit_word_is_preserved() {
    let text = format!("commit {GIT_SHA40}");
    assert_eq!(redact(&text), text);
}

#[test]
fn sha_after_sha_word_is_preserved() {
    let text = format!("sha {GIT_SHA40}");
    assert_eq!(redact(&text), text);
}

#[test]
fn sha_in_git_log_oneline_style_is_preserved() {
    let text = format!("{GIT_SHA40} Fix the parser bug");
    assert_eq!(redact(&text), text);
}

// ---------------------------------------------------------------------------
// Python: TestFalsePositives (4 methods, 11 table rows)
// ---------------------------------------------------------------------------

#[test]
fn false_positive_table_is_unchanged() {
    for (i, text) in FALSE_POSITIVES.iter().enumerate() {
        assert_eq!(
            redact(text),
            *text,
            "false positive[{i}] changed. input={text:?}"
        );
    }
}

#[test]
fn snake_case_identifier_is_45_chars_and_kept() {
    let ident = "parse_session_summary_for_the_export_xxxxxxxx";
    assert_eq!(ident.len(), 45);
    assert_eq!(redact(ident), ident);
}

#[test]
fn token_word_without_secret_value_survives() {
    assert!(redact("let token_count = 5;").contains("token_count"));
}

#[test]
fn password_word_in_prose_survives() {
    assert_eq!(redact("reset your password"), "reset your password");
}

// ---------------------------------------------------------------------------
// Python: TestIdempotence (2 methods)
// ---------------------------------------------------------------------------

#[test]
fn scrub_is_idempotent_on_mixed_corpus() {
    let mut parts: Vec<String> = SEEDED.iter().map(|(_, v)| v.to_string()).collect();
    parts.extend(PREFIX_TOKENS.iter().map(|(_, v)| v.to_string()));
    parts.extend(NONUNIFORM_HEX64.iter().map(|s| s.to_string()));
    parts.extend([HEX32, HEX48, GIT_SHA40].iter().map(|s| s.to_string()));
    parts.extend(FALSE_POSITIVES.iter().map(|s| s.to_string()));
    let corpus = parts.join("\n");
    let once = redact(&corpus);
    assert_eq!(redact(&once), once, "not idempotent on mixed corpus");
}

#[test]
fn scrub_is_idempotent_per_seeded_value() {
    for (i, (kind, value)) in SEEDED.iter().enumerate() {
        let once = redact(value);
        assert_eq!(
            redact(&once),
            once,
            "seeded[{i}] kind={kind} not idempotent"
        );
    }
}

// ---------------------------------------------------------------------------
// Python: TestScrubBeforeTruncate (2 methods)
// ---------------------------------------------------------------------------

/// The first 600 characters of the redacted text must not contain any
/// 6-character window of the secret (no fragment straddling the cut).
fn assert_no_secret_window_in_head(label: &str, secret: &str, text: &str) {
    let head: String = redact(text).chars().take(600).collect();
    let bytes = secret.as_bytes();
    for window in bytes.windows(6) {
        let w = std::str::from_utf8(window).expect("ASCII fixture");
        assert!(
            !head.contains(w),
            "{label}: window {w:?} leaked into head. head={head:?}"
        );
    }
}

#[test]
fn secret_straddling_index_600_leaves_no_fragment() {
    let secret = SEEDED.iter().find(|(k, _)| *k == "hex64").unwrap().1;
    let prose: String = PROSE_SENTENCE.repeat(30).chars().take(590).collect();
    assert_eq!(prose.chars().count(), 590);
    let text = format!("{prose}{secret} trailing words follow");
    assert_no_secret_window_in_head("hex64 straddle", secret, &text);
}

#[test]
fn straddling_nonhex_secret_leaves_no_fragment() {
    let secret = PREFIX_TOKENS[0].1;
    let prose: String = PROSE_SENTENCE.repeat(30).chars().take(590).collect();
    let text = format!("{prose}{secret} trailing words follow");
    assert_no_secret_window_in_head("openai straddle", secret, &text);
}

// ---------------------------------------------------------------------------
// Python: TestRobustnessAndPerformance (5 methods; design adds the extra inputs)
// ---------------------------------------------------------------------------

#[test]
fn prose_200kb_scrubs_under_1_5_seconds() {
    let prose: String = PROSE_SENTENCE.repeat(4000).chars().take(200_000).collect();
    let out = timed_redact("200KB prose", &prose, 1.5);
    assert_eq!(out, prose);
}

#[test]
fn adversarial_long_run_of_a() {
    timed_redact("a*100000", &"a".repeat(100_000), 2.0);
}

#[test]
fn adversarial_unterminated_password_json() {
    let text = format!("\"password\": \"{}", "x".repeat(100_000));
    timed_redact("password json", &text, 2.0);
}

#[test]
fn adversarial_long_a_then_equals() {
    let text = format!("{}=", "A".repeat(50_000));
    timed_redact("A*50000 =", &text, 2.0);
}

#[test]
fn adversarial_pem_without_end() {
    let text = format!(
        concat!("-----BEGIN ", "PRIVATE KEY-----{}"),
        "x".repeat(100_000)
    );
    timed_redact("pem no end", &text, 2.0);
}

#[test]
fn adversarial_200kb_key_value_pairs() {
    timed_redact("a= x100000", &"a=".repeat(100_000), 2.0);
}

#[test]
fn adversarial_200kb_quote_soup() {
    timed_redact("quotes", &"\"'`".repeat(66_667), 2.0);
}

#[test]
fn adversarial_200kb_hex_run() {
    timed_redact("hex", &"0123456789abcdef".repeat(12_500), 2.0);
}

#[test]
fn adversarial_200kb_base64_like() {
    timed_redact("b64ish", &"Ab3+/Xy9".repeat(25_000), 2.0);
}

#[test]
fn adversarial_very_long_keyword_line() {
    let text = format!("password={}", "x".repeat(199_991));
    timed_redact("long keyword line", &text, 2.0);
}

#[test]
fn adversarial_10k_nested_quote_chars() {
    let text = format!("{}secret{}", "\"".repeat(10_000), "\"".repeat(10_000));
    timed_redact("nested quotes", &text, 2.0);
}

#[test]
fn adversarial_10k_nested_json_openers() {
    timed_redact("json nested", &"{\"".repeat(10_000), 2.0);
}

// ---------------------------------------------------------------------------
// Python: TestInputContract (4 methods; the non-str TypeError test has no
// Rust equivalent because `&str` is enforced at compile time)
// ---------------------------------------------------------------------------

#[test]
fn empty_string_returns_empty_string() {
    assert_eq!(redact(""), "");
}

#[test]
fn unicode_text_is_unchanged() {
    let text = "café naïve 日本語 ünïcödé";
    assert_eq!(redact(text), text);
}

#[test]
fn rtl_text_is_unchanged() {
    let text = "مرحبا بالعالم שלום עולם \u{202e}foo\u{202c}";
    assert_eq!(redact(text), text);
}

// ---------------------------------------------------------------------------
// Design extra: UTF-8 validity and no panic over multibyte, emoji, NUL, RTL.
// Expected outputs are the reference scrubber's actual outputs on these inputs.
// ---------------------------------------------------------------------------

#[test]
fn emoji_before_password_redacts_and_keeps_emoji() {
    assert_eq!(
        redact("\u{1F512} password=hunter2 ok"),
        "\u{1F512} password=<redacted:password> ok"
    );
}

#[test]
fn nul_inside_secret_is_redacted_without_panic() {
    assert_eq!(
        redact("a\0b password=hu\0nter2 end"),
        "a\0b password=<redacted:password> end"
    );
}

#[test]
fn emoji_and_nul_around_token_redacts() {
    assert_eq!(
        redact("\u{1F511} with NUL \0 and token=abcd1234efgh5678ijkl "),
        "\u{1F511} with NUL \0 and token=<redacted:token> "
    );
}

#[test]
fn cjk_before_password_redacts() {
    assert_eq!(
        redact("密码 password=hunter2 ok"),
        "密码 password=<redacted:password> ok"
    );
}

#[test]
fn long_multibyte_tail_before_password_redacts() {
    let text = format!("{}password=hunter2", "é".repeat(50_000));
    let out = redact(&text);
    assert!(!out.contains("hunter2"), "secret survived");
    // Char-based tail: a byte-offset slice could split a 2-byte char and panic.
    let tail: String = {
        let mut t: Vec<char> = out.chars().rev().take(40).collect();
        t.reverse();
        t.into_iter().collect()
    };
    assert!(
        out.ends_with("password=<redacted:password>"),
        "tail={tail:?}"
    );
}

// ---------------------------------------------------------------------------
// Python: ScrubPathFalsePositiveTests (8 methods)
// ---------------------------------------------------------------------------

fn is_path_like(s: &str) -> bool {
    s.matches('/').count() >= 2 || s.matches('\\').count() >= 3 || s.starts_with("~/")
}

#[test]
fn corpus_fixtures_meet_stated_shape() {
    for s in PATH_CORPUS.iter().chain(IDENT_CORPUS.iter()) {
        assert!(s.len() >= 45, "too short: {s:?}");
        assert!(s.chars().any(|c| c.is_ascii_digit()), "no digit: {s:?}");
        assert!(
            s.chars().any(|c| c.is_ascii_alphabetic()),
            "no letter: {s:?}"
        );
    }
    for s in PATH_CORPUS {
        assert!(is_path_like(s), "not path-like: {s:?}");
    }
    for s in B64_SECRETS_64 {
        assert_eq!(s.len(), 64);
        let slashes = s.matches('/').count();
        assert!(slashes == 1 || slashes == 2, "slash count: {s:?}");
        assert!(s.contains('+'));
        assert!(s.ends_with('='));
        assert!(!s.starts_with('/') && !s.starts_with('~') && !s.starts_with("./"));
    }
    assert_eq!(B64URL_SECRET_48.len(), 48);
}

#[test]
fn paths_unchanged_alone() {
    for s in PATH_CORPUS.iter().chain(IDENT_CORPUS.iter()) {
        assert_eq!(redact(s), *s, "changed alone: {s:?}");
    }
}

#[test]
fn paths_unchanged_in_sentence() {
    for s in PATH_CORPUS.iter().chain(IDENT_CORPUS.iter()) {
        let text = format!("Opened {s} in the editor, then reran the build.");
        assert_eq!(redact(&text), text, "changed in sentence: {s:?}");
    }
}

#[test]
fn paths_unchanged_inside_json_string() {
    for s in PATH_CORPUS.iter().chain(IDENT_CORPUS.iter()) {
        let text = format!(
            "{{\"path\": {}, \"note\": {}}}",
            json_str(s),
            json_str(&format!("see {s}"))
        );
        assert_eq!(redact(&text), text, "changed in json: {s:?}");
    }
}

#[test]
fn standard_base64_64_char_secrets_still_redacted() {
    for secret in B64_SECRETS_64 {
        let contexts = [
            ("bare", secret.to_string()),
            ("prose", format!("the blob {secret} was logged")),
            ("json", format!("{{\"blob\": {}}}", json_str(secret))),
        ];
        for (ctx, text) in contexts {
            let out = redact(&text);
            assert_absent(&format!("b64 ctx={ctx}"), &text, &out, secret);
            assert_absent(&format!("b64 prefix ctx={ctx}"), &text, &out, &secret[..20]);
            assert!(
                out.contains("<redacted:base64>"),
                "ctx={ctx} output={out:?}"
            );
        }
    }
}

#[test]
fn base64url_48_char_secret_still_redacted() {
    let contexts = [
        ("bare", B64URL_SECRET_48.to_string()),
        ("prose", format!("id {B64URL_SECRET_48} end")),
    ];
    for (ctx, text) in contexts {
        let out = redact(&text);
        assert_absent(&format!("b64url ctx={ctx}"), &text, &out, B64URL_SECRET_48);
        assert_absent(
            &format!("b64url prefix ctx={ctx}"),
            &text,
            &out,
            &B64URL_SECRET_48[..20],
        );
        assert!(
            out.contains("<redacted:base64>"),
            "ctx={ctx} output={out:?}"
        );
    }
}

#[test]
fn path_then_seeded_secret_same_line() {
    let text = format!("/Users/x/proj/src/main.rs: token={SEED_TOKEN}");
    let out = redact(&text);
    assert!(out.contains("/Users/x/proj/src/main.rs:"), "output={out:?}");
    assert_absent("path+token", &text, &out, SEED_TOKEN);
    assert!(out.contains("<redacted:token>"), "output={out:?}");
}

#[test]
fn scrub_is_idempotent_on_path_corpus() {
    let corpus = PATH_CORPUS
        .iter()
        .chain(IDENT_CORPUS.iter())
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    let once = redact(&corpus);
    assert_eq!(once, corpus, "path corpus changed");
    assert_eq!(redact(&once), once);
}

// ---------------------------------------------------------------------------
// Python: ScrubKeyNamePrefixTests (4 methods)
// ---------------------------------------------------------------------------

/// (line, value that must be gone). Key names may remain.
const PREFIXED_KEY_SECRETS: &[(&str, &str)] = &[
    ("export OPENAI_API_KEY=Xk29fhQ0pLm", "Xk29fhQ0pLm"),
    ("GITHUB_TOKEN=myT0kenValue99", "myT0kenValue99"),
    ("DB_PASSWORD=hunter2hunter", "hunter2hunter"),
    ("MY_SECRET: s3cr3tv4lue", "s3cr3tv4lue"),
    ("ANTHROPIC_API_KEY: Xk29fhQ0pLm", "Xk29fhQ0pLm"),
    ("\"dbPassword\": \"hunter2\"", "hunter2"),
    ("\"clientSecret\":\"abc123def456\"", "abc123def456"),
    (
        "AWS_SECRET_ACCESS_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
        "wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    ),
    ("--password hunter2", "hunter2"),
    ("--token=abc123def", "abc123def"),
    (
        "--api-key sk_live_abcdef123456",
        concat!("sk_live_", "abcdef123456"),
    ),
    ("SECRET_KEY=abc", "abc"),
];

/// Must come back byte-identical: short flags are ambiguous; these keys are
/// either non-secret words or numeric settings.
const PREFIXED_UNCHANGED: &[&str] = &[
    "-p hunter2",
    "password_policy: strong",
    "token_count = 5",
    "max_tokens: 700",
];

/// Bare, inside a multi-line env block, and inside a JSON object.
fn prefixed_contexts(line: &str) -> Vec<(&'static str, String)> {
    vec![
        ("bare", line.to_string()),
        ("block", format!("env:\n{line}\nDEBUG: on\n")),
        (
            "json",
            format!("{{\"cmd\": {}, \"ok\": true}}", json_str(line)),
        ),
    ]
}

#[test]
fn prefixed_and_camel_case_secret_values_are_removed() {
    let mut checked = 0;
    for (i, (line, value)) in PREFIXED_KEY_SECRETS.iter().enumerate() {
        for (ctx, text) in prefixed_contexts(line) {
            let out = redact(&text);
            assert_absent(&format!("prefixed[{i}] ctx={ctx}"), &text, &out, value);
            checked += 1;
        }
    }
    assert_eq!(checked, 36);
}

#[test]
fn short_flag_and_non_secret_keys_are_unchanged() {
    let mut checked = 0;
    for (i, line) in PREFIXED_UNCHANGED.iter().enumerate() {
        for (ctx, text) in prefixed_contexts(line) {
            assert_eq!(redact(&text), text, "unchanged[{i}] ctx={ctx}");
            checked += 1;
        }
    }
    assert_eq!(checked, 12);
}

#[test]
fn plain_secret_key_still_redacted() {
    let out = redact("SECRET_KEY=abc");
    assert!(out.contains("<redacted:"), "output={out:?}");
    assert!(!out.contains("=abc"), "output={out:?}");
}

#[test]
fn idempotent_on_prefixed_key_corpus() {
    let mut corpus: Vec<String> = Vec::new();
    for (line, _) in PREFIXED_KEY_SECRETS {
        corpus.extend(prefixed_contexts(line).into_iter().map(|(_, t)| t));
    }
    for line in PREFIXED_UNCHANGED {
        corpus.extend(prefixed_contexts(line).into_iter().map(|(_, t)| t));
    }
    for text in &corpus {
        let once = redact(text);
        assert_eq!(redact(&once), once, "not idempotent: {text:?}");
    }
    let joined = corpus.join("\n");
    assert_eq!(redact(&redact(&joined)), redact(&joined));
}

// ---------------------------------------------------------------------------
// Python: ScrubValidatorRound2Tests (3 methods)
// ---------------------------------------------------------------------------

/// (line, secret values that must be gone)
const ROUND2_MUST_REDACT: &[(&str, &[&str])] = &[
    (
        "curl -u admin:Sup3rPass https://api.example.com/v1",
        &["Sup3rPass"],
    ),
    (
        "curl --user admin:Sup3rPass https://api.example.com",
        &["Sup3rPass"],
    ),
    (
        "wget --http-user=admin --http-password=Sup3rPass http://x",
        &["Sup3rPass"],
    ),
    ("os.environ['X_SECRET']='topsecret1'", &["topsecret1"]),
    ("config['password'] = 'hunter2'", &["hunter2"]),
    ("config[\"api_key\"]=\"abc123xyz456\"", &["abc123xyz456"]),
    (
        "settings[\"dbPassword\"] = 'hunter2hunter'",
        &["hunter2hunter"],
    ),
    ("DB_PASS=hunter2", &["hunter2"]),
    ("pass=hunter2", &["hunter2"]),
    ("PASS: hunter2", &["hunter2"]),
    ("MYSQL_PASS=\"p4ss w0rd\"", &["p4ss w0rd", "p4ss", "w0rd"]),
    ("auth=('admin', 'hunter2')", &["hunter2"]),
    ("auth=(\"user\", \"p4ssw0rd9\")", &["p4ssw0rd9"]),
    ("requests.get(url, auth=('admin','hunter2'))", &["hunter2"]),
    ("HTTPBasicAuth('admin', 'hunter2')", &["hunter2"]),
    ("redis://:onlypass@localhost:6379", &["onlypass"]),
    ("https://:onlypass77@h.example.com/x", &["onlypass77"]),
    (
        "STRIPE_KEY=rk_live_51Habcdef1234567890",
        &[concat!("rk_live_", "51Habcdef1234567890")],
    ),
    (
        concat!("sk_live_", "abcdef1234567890ABCD"),
        &[concat!("sk_live_", "abcdef1234567890ABCD")],
    ),
    (
        "sk_test_abcdef1234567890ABCD",
        &["sk_test_abcdef1234567890ABCD"],
    ),
];

/// Must come back byte-identical in every context.
const ROUND2_MUST_STAY: &[&str] = &[
    "token = getenv('TOKEN')",
    "api_key = os.environ[\"API_KEY\"]",
    "password = input(\"Password: \")",
    "secret = settings.SECRET",
    "password_policy=strict max_tokens=100 token_count=5",
    "pass_through: true",
    "bypass=1",
    "compass: north",
    "if x:\n    pass",
    "def f():\n    pass",
    "os.environ['HOME']",
    "config['timeout'] = 30",
    "auth=None",
    "auth=auth_handler",
    "ssh -i ~/.ssh/id_rsa user@host",
    "curl -X POST https://api.example.com/v1",
    "ls -u",
    "sort -u file.txt",
    "the user:admin role",
];

#[test]
fn round2_secret_values_are_removed() {
    let mut checked = 0;
    for (i, (line, secrets)) in ROUND2_MUST_REDACT.iter().enumerate() {
        for (ctx, text) in line_contexts(line) {
            let out = redact(&text);
            for secret in *secrets {
                let label = format!("round2[{i}] ctx={ctx}");
                assert_no_fragment(
                    &label,
                    &text,
                    &out,
                    secret,
                    &['\'', '"', '(', ')', '[', ']'],
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 60);
}

#[test]
fn round2_lookalike_lines_are_unchanged() {
    let mut checked = 0;
    for (i, line) in ROUND2_MUST_STAY.iter().enumerate() {
        for (ctx, text) in line_contexts(line) {
            assert_eq!(redact(&text), text, "lookalike[{i}] ctx={ctx}");
            checked += 1;
        }
    }
    assert_eq!(checked, 57);
}

#[test]
fn round2_corpus_is_idempotent() {
    let lines: Vec<&str> = ROUND2_MUST_REDACT
        .iter()
        .map(|(l, _)| *l)
        .chain(ROUND2_MUST_STAY.iter().copied())
        .collect();
    let mut texts: Vec<String> = Vec::new();
    for line in &lines {
        for (_, text) in line_contexts(line) {
            let once = redact(&text);
            assert_eq!(redact(&once), once, "not idempotent: {text:?}");
            texts.push(text);
        }
    }
    let joined = texts.join("\n");
    let once = redact(&joined);
    assert_eq!(redact(&once), once);
}

// ---------------------------------------------------------------------------
// Python: ScrubGoAndShortFlagTests (3 methods)
// ---------------------------------------------------------------------------

const GO_MUST_REDACT: &[(&str, &[&str])] = &[
    ("password := \"S3cure!Pass2024\"", &["S3cure!Pass2024"]),
    (
        "apiKey := \"Xk9fQ2mLp7Rz4TbW8vNc\"",
        &["Xk9fQ2mLp7Rz4TbW8vNc"],
    ),
    ("token:=\"abcDEF123ghi456\"", &["abcDEF123ghi456"]),
    ("dbPassword := `raw-secret-9`", &["raw-secret-9"]),
    ("secret = \"s3cr3t-v4lue\"", &["s3cr3t-v4lue"]),
    ("const password string = \"hunter2\"", &["hunter2"]),
    (
        "var apiKey string = \"Xk9fQ2mLp7Rz4TbW8vNc\"",
        &["Xk9fQ2mLp7Rz4TbW8vNc"],
    ),
    (
        "docker login -u bob -p hunter2 registry.example.com",
        &["hunter2"],
    ),
    (
        "podman login --password hunter2 -u bob registry.example.com",
        &["hunter2"],
    ),
    (
        "docker login registry.example.com -u bob -p hunter2xyz",
        &["hunter2xyz"],
    ),
    ("mysql -u root -phunter2 db", &["hunter2"]),
    ("mysqldump -uroot -phunter2 db", &["hunter2"]),
    ("http -a admin:hunter2 GET api.example.com", &["hunter2"]),
    (
        "http --auth admin:hunter2 GET api.example.com",
        &["hunter2"],
    ),
    (
        "http --auth=admin:hunter2 GET api.example.com",
        &["hunter2"],
    ),
];

const GO_MUST_STAY: &[&str] = &[
    "secret := os.Getenv(\"SECRET\")",
    "password := os.Getenv(\"DB_PASSWORD\")",
    "token := getToken()",
    "apiKey := cfg.APIKey",
    "password := \"\"",
    "ssh -p 22 host",
    "nc -p 8080 localhost",
    "ls -a",
    "python -m http.server -p 8000",
    "curl -p http://x",
    "mysql -u root db",
    "mysql -p",
    "docker login registry.example.com",
    "http GET api.example.com",
    "http -a",
    "gcc -a main.c",
    "x := 5",
    "err := run()",
    "name := \"bob\"",
];

#[test]
fn go_and_short_flag_secret_values_are_removed() {
    let mut checked = 0;
    for (i, (line, secrets)) in GO_MUST_REDACT.iter().enumerate() {
        for (ctx, text) in line_contexts(line) {
            let out = redact(&text);
            for secret in *secrets {
                let label = format!("go[{i}] ctx={ctx}");
                assert_no_fragment(
                    &label,
                    &text,
                    &out,
                    secret,
                    &['\'', '"', '`', '(', ')', '[', ']'],
                );
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 45);
}

#[test]
fn go_and_short_flag_lookalikes_are_unchanged() {
    let mut checked = 0;
    for (i, line) in GO_MUST_STAY.iter().enumerate() {
        for (ctx, text) in line_contexts(line) {
            assert_eq!(redact(&text), text, "go lookalike[{i}] ctx={ctx}");
            checked += 1;
        }
    }
    assert_eq!(checked, 57);
}

#[test]
fn go_and_short_flag_corpus_is_idempotent() {
    let lines: Vec<&str> = GO_MUST_REDACT
        .iter()
        .map(|(l, _)| *l)
        .chain(GO_MUST_STAY.iter().copied())
        .collect();
    let mut texts: Vec<String> = Vec::new();
    for line in &lines {
        for (_, text) in line_contexts(line) {
            let once = redact(&text);
            assert_eq!(redact(&once), once, "not idempotent: {text:?}");
            texts.push(text);
        }
    }
    let joined = texts.join("\n");
    let once = redact(&joined);
    assert_eq!(redact(&once), once);
}

// ---------------------------------------------------------------------------
// Design: email policy must equal the Python reference (scrub.py _EMAIL).
// Expected values are the reference's actual outputs.
// ---------------------------------------------------------------------------

#[test]
fn email_policy_equals_python_reference() {
    let cases: Vec<(String, String)> = vec![
        ("someone@example.com".into(), "<email>".into()),
        ("a.b+c_d%e-f@sub.example.co".into(), "<email>".into()),
        ("x@mail.example.museum".into(), "<email>".into()),
        (
            "u@example.abcdefghijklmnopqrstuvwx".into(),
            "<email>".into(),
        ),
        (format!("{}@example.com", "a".repeat(64)), "<email>".into()),
        (
            format!("{}@example.com", "a".repeat(65)),
            format!("{}<email>", "a"),
        ),
        (
            "u@example.abcdefghijklmnopqrstuvwxy".into(),
            "<email>y".into(),
        ),
        ("u@example.c".into(), "u@example.c".into()),
        (
            "Mail me at a@b.io today".into(),
            "Mail me at <email> today".into(),
        ),
        ("a@b.io and c@d.io".into(), "<email> and <email>".into()),
        ("user@localhost".into(), "user@localhost".into()),
        ("@example.com".into(), "@example.com".into()),
        (
            "name at example dot com".into(),
            "name at example dot com".into(),
        ),
        ("x_y-z@my-host.example.org".into(), "<email>".into()),
    ];
    assert_eq!(cases.len(), 14);
    for (i, (input, expected)) in cases.iter().enumerate() {
        assert_eq!(redact(input), *expected, "email case {i}: input={input:?}");
    }
}

// ---- Over-cap differential tests (expected values generated from the Python reference by the lead) ----
#[test]
fn overcap_inputs_match_python_reference() {
    // overcap_password_equals
    {
        let input = String::new()
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &"="
            + &"a".repeat(5000);
        let expected = String::new()
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &"="
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &">"
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"h"
            + &"e"
            + &"x"
            + &">";
        assert_eq!(redact(&input), expected, "case overcap_password_equals");
    }
    // overcap_token_colon
    {
        let input =
            String::new() + &"t" + &"o" + &"k" + &"e" + &"n" + &":" + &" " + &"x".repeat(4097);
        let expected = String::new()
            + &"t"
            + &"o"
            + &"k"
            + &"e"
            + &"n"
            + &":"
            + &" "
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"t"
            + &"o"
            + &"k"
            + &"e"
            + &"n"
            + &">"
            + &"x";
        assert_eq!(redact(&input), expected, "case overcap_token_colon");
    }
    // overcap_secret_quoted
    {
        let input = String::new()
            + &"s"
            + &"e"
            + &"c"
            + &"r"
            + &"e"
            + &"t"
            + &"="
            + &"\""
            + &"b".repeat(5000)
            + &"\"";
        let expected = String::new()
            + &"s"
            + &"e"
            + &"c"
            + &"r"
            + &"e"
            + &"t"
            + &"="
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"s"
            + &"e"
            + &"c"
            + &"r"
            + &"e"
            + &"t"
            + &">"
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"h"
            + &"e"
            + &"x"
            + &">"
            + &"\"";
        assert_eq!(redact(&input), expected, "case overcap_secret_quoted");
    }
    // overcap_bearer_long
    {
        let input = String::new()
            + &"A"
            + &"u"
            + &"t"
            + &"h"
            + &"o"
            + &"r"
            + &"i"
            + &"z"
            + &"a"
            + &"t"
            + &"i"
            + &"o"
            + &"n"
            + &":"
            + &" "
            + &"B"
            + &"e"
            + &"a"
            + &"r"
            + &"e"
            + &"r"
            + &" "
            + &"A".repeat(600);
        let expected = String::new()
            + &"A"
            + &"u"
            + &"t"
            + &"h"
            + &"o"
            + &"r"
            + &"i"
            + &"z"
            + &"a"
            + &"t"
            + &"i"
            + &"o"
            + &"n"
            + &":"
            + &" "
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"a"
            + &"u"
            + &"t"
            + &"h"
            + &"o"
            + &"r"
            + &"i"
            + &"z"
            + &"a"
            + &"t"
            + &"i"
            + &"o"
            + &"n"
            + &">"
            + &" "
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"a"
            + &"u"
            + &"t"
            + &"h"
            + &"o"
            + &"r"
            + &"i"
            + &"z"
            + &"a"
            + &"t"
            + &"i"
            + &"o"
            + &"n"
            + &">";
        assert_eq!(redact(&input), expected, "case overcap_bearer_long");
    }
    // overcap_flag_password
    {
        let input = String::new()
            + &"--"
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &" "
            + &"p".repeat(5000);
        let expected = String::new()
            + &"--"
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &" "
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &">"
            + &"p".repeat(904);
        assert_eq!(redact(&input), expected, "case overcap_flag_password");
    }
    // overcap_multibyte_value
    {
        let input = String::new()
            + &"a"
            + &"p"
            + &"i"
            + &"_"
            + &"k"
            + &"e"
            + &"y"
            + &"="
            + &"é".repeat(5000);
        let expected = String::new()
            + &"a"
            + &"p"
            + &"i"
            + &"_"
            + &"k"
            + &"e"
            + &"y"
            + &"="
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"a"
            + &"p"
            + &"i"
            + &"_"
            + &"k"
            + &"e"
            + &"y"
            + &">"
            + &"é".repeat(904);
        assert_eq!(redact(&input), expected, "case overcap_multibyte_value");
    }
    // overcap_exact_cap
    {
        let input = String::new()
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &"="
            + &"k".repeat(4096);
        let expected = String::new()
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &"="
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &">";
        assert_eq!(redact(&input), expected, "case overcap_exact_cap");
    }
    // overcap_cap_plus_one
    {
        let input = String::new()
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &"="
            + &"k".repeat(4097);
        let expected = String::new()
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &"="
            + &"<"
            + &"r"
            + &"e"
            + &"d"
            + &"a"
            + &"c"
            + &"t"
            + &"e"
            + &"d"
            + &":"
            + &"p"
            + &"a"
            + &"ss"
            + &"w"
            + &"o"
            + &"r"
            + &"d"
            + &">"
            + &"k";
        assert_eq!(redact(&input), expected, "case overcap_cap_plus_one");
    }
}
