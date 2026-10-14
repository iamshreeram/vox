//! Secret redaction: Rust port of the proven Python scrubber.
//!
//! Rules run in the reference order, each as one pass over the text. The
//! reference uses look-behind, look-ahead and back-references, which the
//! `regex` crate does not support. Those checks are emulated: the pattern
//! matches a slightly wider shape and the replacement closure inspects the
//! original text around the match (byte offsets, sliced only on char
//! boundaries). Back-references become one pattern per quote style.

use regex::{Captures, NoExpand, Regex};
use std::collections::HashMap;
use std::sync::LazyLock;

const B64_MIN_ENTROPY: f64 = 3.5;

const KEYWORDS: &str = r"x-api-key|client[_-]?secret|private[_-]?key|access[_-]?key|secret[_-]?key|api[_-]?key|apikey|authorization|credentials?|token|secret|password|passwd|pwd|auth|pass";

/// Value alternatives shared by the key=value rule and the long-flag rule.
/// The `(?!<redacted:)` guard is checked in code on the captured value.
const VALUE: &str =
    r#"(?P<val>\\"[^"\\\n]*\\?"?|"[^"\n]*"?|'[^'\n]*'?|`[^`\n]*`?|[^\s"'\\,;)\]}>]+)"#;

fn marker(kind: &str) -> String {
    format!("<redacted:{kind}>")
}

/// Length cap of a value in the reference (`*` / `+`). The
/// patterns below are unbounded; the cap is enforced in code so that the
/// compiled automaton stays small.
const VALUE_CAP: usize = 4096;

/// Byte offset after the first `n` chars of `s` (or `s.len()`).
fn cut_chars(s: &str, n: usize) -> usize {
    s.char_indices().nth(n).map_or(s.len(), |(i, _)| i)
}

/// Length in bytes of the part of a matched value that the reference's
/// bounded alternatives would have consumed. Over-cap content stops after
/// `VALUE_CAP` chars (plus the opening quote); the rest stays as text.
fn value_keep_len(val: &str) -> usize {
    let (open, body, quote, json) = if let Some(rest) = val.strip_prefix("\\\"") {
        (2, rest, '"', true)
    } else {
        match val.chars().next() {
            Some(q) if q == '"' || q == '\'' || q == '`' => (1, &val[1..], q, false),
            _ => return cut_chars(val, VALUE_CAP),
        }
    };
    let content = body
        .chars()
        .take_while(|&c| c != '\n' && c != quote && !(json && c == '\\'))
        .count();
    if content > VALUE_CAP {
        open + cut_chars(body, VALUE_CAP)
    } else {
        val.len()
    }
}

/// Replaces every match for which `f` returns `Some`; `None` keeps the text.
fn replace_with<F>(text: &str, re: &Regex, mut f: F) -> String
where
    F: FnMut(&str, &Captures<'_>) -> Option<String>,
{
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for caps in re.captures_iter(text) {
        let Some(whole) = caps.get(0) else {
            continue;
        };
        if let Some(rep) = f(text, &caps) {
            out.push_str(&text[last..whole.start()]);
            out.push_str(&rep);
            last = whole.end();
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Like `replace_with`, but `f` returns the byte offset where the consumed
/// text ends. When a value is cut at its length cap, the rest of the match
/// stays as text and scanning resumes at that offset, as the bounded
/// reference pattern would. `None` keeps the match and resumes after it.
fn replace_capped<F>(text: &str, re: &Regex, mut f: F) -> String
where
    F: FnMut(&str, &Captures<'_>) -> Option<(String, usize)>,
{
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    let mut pos = 0;
    while pos <= text.len() {
        let Some(caps) = re.captures_at(text, pos) else {
            break;
        };
        let Some(whole) = caps.get(0) else {
            break;
        };
        match f(text, &caps) {
            Some((rep, end)) if end > whole.start() => {
                out.push_str(&text[last..whole.start()]);
                out.push_str(&rep);
                last = end;
                pos = end;
            }
            _ => pos = whole.end(),
        }
    }
    out.push_str(&text[last..]);
    out
}

fn replace_fixed(text: &str, re: &Regex, rep: &str) -> String {
    re.replace_all(text, NoExpand(rep)).into_owned()
}

/// Character immediately before byte offset `start`, if any.
fn prev_char(text: &str, start: usize) -> Option<char> {
    text[..start].chars().next_back()
}

/// Python `(?<![A-Za-z0-9])` at `start`.
fn not_after_ascii_alnum(text: &str, start: usize) -> bool {
    !prev_char(text, start).is_some_and(|c| c.is_ascii_alphanumeric())
}

/// Python `(?<![\w-])` at `start`.
fn not_after_word_or_dash(text: &str, start: usize) -> bool {
    !prev_char(text, start).is_some_and(|c| c.is_alphanumeric() || c == '_' || c == '-')
}

fn entropy(s: &str) -> f64 {
    let n = s.chars().count();
    if n == 0 {
        return 0.0;
    }
    let mut counts: HashMap<char, usize> = HashMap::new();
    for c in s.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }
    let nf = n as f64;
    -counts
        .values()
        .map(|&c| {
            let p = c as f64 / nf;
            p * p.log2()
        })
        .sum::<f64>()
}

// -- multi-line PEM blocks; an unterminated BEGIN redacts to end of text -----
static PEM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN [A-Z0-9 ]{1,40}-----[\s\S]*?(?:-----END [A-Z0-9 ]{1,40}-----|\z)")
        .unwrap()
});

// -- JSON Web Tokens ---------------------------------------------------------
static JWT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"eyJ[A-Za-z0-9_\-]{8,2048}\.[A-Za-z0-9_\-]{8,2048}\.[A-Za-z0-9_\-]{8,2048}")
        .unwrap()
});

// -- Authorization headers (scheme kept out of the value) --------------------
static AUTH_HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?i)(\bauthorization\\?["']?[ \t]*[:=][ \t]*(?:(?:basic|bearer|token)[ \t]+)?)(?P<val>[^\s"',;\\)\]}>]+)"#,
    )
    .unwrap()
});

static BEARER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\bbearer[ \t]+[A-Za-z0-9\-._~+/]{16,512}=*").unwrap());

// -- known token prefixes (Stripe is handled separately, see STRIPE) ---------
static PREFIX_RULES: LazyLock<Vec<(Regex, String)>> = LazyLock::new(|| {
    let rules: [(&str, &str); 11] = [
        (r"sk-ant-[A-Za-z0-9_\-]{10,512}", "anthropic_key"),
        (r"sk-[A-Za-z0-9_\-]{20,512}", "openai_key"),
        (r"AIza[0-9A-Za-z_\-]{30,512}", "google_key"),
        (r"ya29\.[A-Za-z0-9_\-.]{20,512}", "google_oauth"),
        (r"glpat-[A-Za-z0-9_\-]{16,512}", "gitlab_token"),
        (r"npm_[A-Za-z0-9]{20,512}", "npm_token"),
        (r"pypi-[A-Za-z0-9_\-]{20,1024}", "pypi_token"),
        (r"xox[abprs]-[A-Za-z0-9\-]{10,512}", "slack_token"),
        (r"github_pat_[A-Za-z0-9_]{20,512}", "github_pat"),
        (r"gh[pousr]_[A-Za-z0-9]{16,512}", "github_token"),
        (r"(?:AKIA|ASIA)[0-9A-Z]{16}", "aws_key"),
    ];
    rules
        .iter()
        .map(|(p, k)| (Regex::new(p).unwrap(), marker(k)))
        .collect()
});

static STRIPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:(?:sk|rk)_(?:live|test)|pk_live)_[A-Za-z0-9]{16,512}").unwrap()
});

// -- curl credentials: ``-u user:password`` / ``--user user:password`` -----
static CURL_USER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?P<pre>(?:-u|--user)(?:[ \t]+|=))(?P<user>[^\s:"'<>]{1,256}):[^\s"'<>]{1,512}"#)
        .unwrap()
});

// -- tuple credentials: one pattern per quote style (no back-references) ----
static AUTH_TUPLE: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    let styles: [(char, char); 4] = [('\'', '\''), ('\'', '"'), ('"', '\''), ('"', '"')];
    styles
        .iter()
        .map(|(q, q2)| {
            let pattern = format!(
                r"(?i)(?P<pre>auth[ \t]{{0,4}}=[ \t]{{0,4}}|HTTPBasicAuth[ \t]{{0,4}})\([ \t]{{0,4}}{q}[^{q}\n]{{0,200}}{q}[ \t]{{0,4}},[ \t]{{0,4}}{q2}[^{q2}\n]{{0,200}}{q2}[ \t]{{0,4}}\)"
            );
            Regex::new(&pattern).unwrap()
        })
        .collect()
});

// -- URL userinfo: keep scheme and host, drop user[:password] ----------------
static USERINFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"([A-Za-z][A-Za-z0-9+.\-]{0,20}://)[^\s/:@"'<>\\]{0,256}(?::[^\s/@"'<>\\]{0,256})?@"#,
    )
    .unwrap()
});

// -- key=value / "key": "value" / key: value for sensitive keys --------------
static KV: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = format!(
        r"(?i)(?P<pre>(?P<kw>{KEYWORDS})\\?[\x22']?\]?(?:[ \t]+(?:\*|\[\])*[A-Za-z_][A-Za-z0-9_.\[\]*]{{0,64}})?[ \t]*(?::=|:|=)[ \t]*){VALUE}"
    );
    Regex::new(&pattern).unwrap()
});

// Long flags with a space-separated value: ``--password hunter2``.
static FLAG: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = format!(r"(?i)(?P<pre>--[A-Za-z0-9_\-]{{0,80}}?(?P<kw>{KEYWORDS})[ \t]+){VALUE}");
    Regex::new(&pattern).unwrap()
});

static CODE_ATTR_FULL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)+)$").unwrap()
});
static SNAKE_IDENT_FULL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:[a-z]+(?:_[a-z]+)+)$").unwrap());

const CODE_LITERALS: [&str; 7] = [
    "None",
    "True",
    "False",
    "null",
    "true",
    "false",
    "undefined",
];

/// True when a matched value is code or a non-secret rather than a credential.
fn is_code_value(kw: &str, val: &str) -> bool {
    if matches!(val, "\"\"" | "''" | "``") {
        return true; // empty literal: there is no secret to hide
    }
    if let Some(first) = val.chars().next() {
        if first == '\'' || first == '"' {
            // e.g. input("Password: "): the value opens with the literal's own quote.
            return matches!(val.chars().nth(1), Some(')') | Some(','));
        }
    }
    if kw == "auth" {
        return true; // bare auth values are never redacted; quoted/tuple forms are
    }
    if val.contains(['(', ')', '[', ']']) {
        return true; // call or subscript syntax
    }
    if CODE_LITERALS.contains(&val) {
        return true;
    }
    CODE_ATTR_FULL.is_match(val) || SNAKE_IDENT_FULL.is_match(val)
}

/// Python `\\?["']?\]?[ \t]*[:=]`: the text after a `pass` keyword must be a separator.
fn pass_separator_follows(rest: &str) -> bool {
    let mut chars = rest.chars().peekable();
    if chars.peek() == Some(&'\\') {
        chars.next();
    }
    if matches!(chars.peek(), Some('"') | Some('\'')) {
        chars.next();
    }
    if chars.peek() == Some(&']') {
        chars.next();
    }
    while matches!(chars.peek(), Some(' ') | Some('\t')) {
        chars.next();
    }
    matches!(chars.next(), Some(':') | Some('='))
}

fn scan_kv(text: &str) -> String {
    replace_capped(text, &KV, kv_repl)
}

fn kv_repl(text: &str, caps: &Captures<'_>) -> Option<(String, usize)> {
    let val_m = caps.name("val")?;
    let full = val_m.as_str();
    if full.starts_with("<redacted:") {
        return None; // (?!<redacted:)
    }
    let pre = caps.name("pre")?.as_str();
    // Python `:(?!=)`: a bare ':' separator cannot be followed by '='.
    if pre.ends_with(':') && full.starts_with('=') {
        return None;
    }
    // Over-cap values: only the first VALUE_CAP chars are consumed.
    let keep = value_keep_len(full);
    let val = &full[..keep];
    let end = val_m.start() + keep;
    let kw_m = caps.name("kw")?;
    if kw_m.as_str().eq_ignore_ascii_case("pass") {
        // (?<![A-Za-z0-9])pass(?=\\?["']?\]?[ \t]*[:=])
        if !not_after_ascii_alnum(text, kw_m.start()) {
            return None;
        }
        if !pass_separator_follows(&text[kw_m.end()..]) {
            return None;
        }
    }
    let kw = kw_m.as_str().to_ascii_lowercase();
    if is_code_value(&kw, val) {
        // Keep the code; still scan inside it for key=value pairs.
        return Some((format!("{pre}{}", scan_kv(val)), end));
    }
    Some((format!("{pre}{}", marker(&kw.replace('-', "_"))), end))
}

// -- context-limited short flags ---------------------------------------------
static DOCKER_LOGIN_P: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?P<pre>\b(?:docker|podman|helm|skopeo)[ \t][^\n]{0,300}?\blogin\b[^\n]{0,300}?[ \t](?:-p|--password)[ \t]+)(?P<val>[^\s\-"'<>][^\s"'<>]{0,512})"#,
    )
    .unwrap()
});

static MYSQL_P: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?P<pre>(?:mysqldump|mysqladmin|mysqlimport|mysql)[ \t][^\n]{0,300}?[ \t]-p)(?P<val>[^\s\-"'<>][^\s"'<>]{0,512})"#,
    )
    .unwrap()
});

static HTTP_AUTH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?P<pre>https?[ \t](?:[^\n]{0,300}?[ \t])?(?:-a[ \t]+|--auth(?:[ \t]+|=)))(?P<user>[^\s:"'<>\-][^\s:"'<>]{0,256}):(?P<pw>[^\s"'<>]{1,512})"#,
    )
    .unwrap()
});

// -- emails ----------------------------------------------------------------------
static EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[A-Za-z0-9._%+\-]{1,64}@[A-Za-z0-9.\-]{1,255}\.[A-Za-z]{2,24}").unwrap()
});

// -- hex runs >= 32, except a git sha in a commit/sha/rev/HEAD or log context -
static HEX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9a-fA-F]{32,}").unwrap());
static SHA_CONTEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(?:commit|sha|rev|head)\b[\s:=]*\z").unwrap());
static HEX_ONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[0-9a-fA-F]+$").unwrap());

fn sha_exempt(text: &str, start: usize, end: usize) -> bool {
    // The 40 characters before the match, as the reference slices them.
    let tail_start = text[..start]
        .char_indices()
        .rev()
        .nth(39)
        .map(|(i, _)| i)
        .unwrap_or(0);
    if SHA_CONTEXT.is_match(&text[tail_start..start]) {
        return true;
    }
    // git log --oneline style: sha at line start followed by a space.
    let at_line_start = start == 0 || text.as_bytes()[start - 1] == b'\n';
    at_line_start && end < text.len() && text.as_bytes()[end] == b' '
}

// -- base64 / base64url runs >= 40 with digit, letter and entropy >= 3.5 -----
static B64: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z0-9+/_\-]{40,}={0,2}").unwrap());

// Separators that split a candidate into word-like segments.
static SEG_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[/\-._]+").unwrap());

/// True when most segments of `s` are ordinary words or numbers.
fn is_wordy(s: &str) -> bool {
    let segs: Vec<&str> = SEG_SPLIT.split(s).filter(|x| !x.is_empty()).collect();
    if segs.len() < 2 {
        return false;
    }
    let words = segs
        .iter()
        .filter(|x| {
            x.len() >= 2
                && (x.chars().all(|c| c.is_ascii_alphabetic())
                    || x.chars().all(|c| c.is_ascii_digit()))
        })
        .count();
    words as f64 >= 0.7 * segs.len() as f64
}

/// Decide whether a base64-shaped candidate is an ordinary path, URL or name.
fn is_path_or_identifier(text: &str, s: &str, start: usize) -> bool {
    if s.contains('+') || s.contains('=') {
        return false;
    }
    // A long mixed letter+digit segment glued onto a path is a secret: fail closed.
    for seg in SEG_SPLIT.split(s) {
        if seg.len() >= 32
            && seg.chars().any(|c| c.is_ascii_digit())
            && seg.chars().any(|c| c.is_ascii_alphabetic())
        {
            return false;
        }
    }
    if start >= 3 && &text.as_bytes()[start - 3..start] == b"://" {
        return true;
    }
    if s.starts_with('/') && s.matches('/').count() >= 2 {
        return true;
    }
    is_wordy(s)
}

fn b64_repl(text: &str, caps: &Captures<'_>) -> Option<String> {
    let whole = caps.get(0)?;
    let s = whole.as_str();
    if HEX_ONLY.is_match(s) {
        return None; // hex runs are owned by the hex rule
    }
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let has_letter = s.chars().any(|c| c.is_ascii_alphabetic());
    if !(has_digit && has_letter) {
        return None;
    }
    if entropy(s) < B64_MIN_ENTROPY {
        return None;
    }
    if is_path_or_identifier(text, s, whole.start()) {
        return None;
    }
    Some(marker("base64"))
}

/// Replaces recognisable secrets in `text` with `<redacted:KIND>` markers
/// (email addresses become `<email>`). Best-effort, not a guarantee.
/// Idempotent: `redact(&redact(x)) == redact(x)`. Never panics on UTF-8 input.
pub fn redact(text: &str) -> String {
    let mut out = replace_fixed(text, &PEM, &marker("pem"));
    out = replace_fixed(&out, &JWT, &marker("jwt"));
    out = replace_capped(&out, &AUTH_HEADER, |_, caps| {
        let val_m = caps.name("val")?;
        if val_m.as_str().starts_with("<redacted:") {
            return None; // (?!<redacted:)
        }
        let pre = caps.get(1)?.as_str();
        let end = val_m.start() + value_keep_len(val_m.as_str());
        Some((format!("{pre}{}", marker("authorization")), end))
    });
    out = replace_fixed(&out, &BEARER, &marker("bearer"));
    for (re, rep) in PREFIX_RULES.iter() {
        out = replace_fixed(&out, re, rep);
    }
    out = replace_with(&out, &STRIPE, |t, caps| {
        let whole = caps.get(0)?;
        if !not_after_ascii_alnum(t, whole.start()) {
            return None;
        }
        Some(marker("stripe_key"))
    });
    for re in AUTH_TUPLE.iter() {
        out = replace_with(&out, re, |t, caps| {
            let whole = caps.get(0)?;
            let pre = caps.name("pre")?.as_str();
            if !not_after_ascii_alnum(t, whole.start()) {
                return None;
            }
            Some(format!("{pre}({})", marker("basic_auth")))
        });
    }
    out = replace_with(&out, &CURL_USER, |t, caps| {
        let whole = caps.get(0)?;
        if !not_after_word_or_dash(t, whole.start()) {
            return None;
        }
        let user = caps.name("user")?.as_str();
        let pre = caps.name("pre")?.as_str();
        Some(format!("{pre}{user}:{}", marker("basic_auth")))
    });
    out = replace_with(&out, &USERINFO, |_, caps| {
        let scheme = caps.get(1)?.as_str();
        Some(format!("{scheme}{}@", marker("userinfo")))
    });
    out = scan_kv(&out);
    out = replace_capped(&out, &FLAG, kv_repl);
    out = replace_with(&out, &DOCKER_LOGIN_P, |_, caps| {
        Some(format!(
            "{}{}",
            caps.name("pre")?.as_str(),
            marker("password")
        ))
    });
    out = replace_with(&out, &MYSQL_P, |t, caps| {
        let whole = caps.get(0)?;
        if !not_after_word_or_dash(t, whole.start()) {
            return None;
        }
        Some(format!(
            "{}{}",
            caps.name("pre")?.as_str(),
            marker("password")
        ))
    });
    out = replace_with(&out, &HTTP_AUTH, |t, caps| {
        let whole = caps.get(0)?;
        if !not_after_word_or_dash(t, whole.start()) {
            return None;
        }
        let pre = caps.name("pre")?.as_str();
        let user = caps.name("user")?.as_str();
        Some(format!("{pre}{user}:{}", marker("basic_auth")))
    });
    out = replace_fixed(&out, &EMAIL, "<email>");
    out = replace_with(&out, &HEX, |t, caps| {
        let whole = caps.get(0)?;
        if whole.len() == 40 && sha_exempt(t, whole.start(), whole.end()) {
            return None;
        }
        Some(marker("hex"))
    });
    out = replace_with(&out, &B64, b64_repl);
    out
}
