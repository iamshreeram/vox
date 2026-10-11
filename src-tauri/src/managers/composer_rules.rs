//! Shared, PURE validators for the speech-composer configuration.
//!
//! One implementation is used by settings-patch validation, run-time
//! re-validation of stored settings, argv construction and the command
//! acknowledgement tuple, so they can never disagree about what is valid.

#![allow(dead_code)]

pub const MAX_BINARY_PATH_CHARS: usize = 1024;
pub const MAX_EXTRA_ARGS: usize = 8;
pub const MAX_ARG_CHARS: usize = 64;
pub const MAX_STYLE_CHARS: usize = 300;
pub const MAX_MODEL_CHARS: usize = 100;
pub const MAX_PROMPT_FLAG_CHARS: usize = 18;
pub const DEFAULT_PROMPT_FLAG: &str = "-p";
pub const TIMEOUT_MIN_SECS: u64 = 2;
pub const TIMEOUT_MAX_SECS: u64 = 60;
pub const TIMEOUT_DEFAULT_SECS: u64 = 45;
pub const DEFAULT_STYLE: &str = "warm, upbeat and concise";

/// Clamp a timeout to the supported range (applied on write AND at use).
pub fn clamp_timeout(secs: u64) -> u64 {
    secs.clamp(TIMEOUT_MIN_SECS, TIMEOUT_MAX_SECS)
}

fn has_control(s: &str) -> bool {
    s.chars().any(|c| c.is_control())
}

/// Absolute path, non-empty, bounded, no control characters. Returns the
/// trimmed path.
pub fn validate_binary_path(path: &str) -> Result<String, String> {
    let p = path.trim();
    if p.is_empty() {
        return Err("Command path is empty".to_string());
    }
    if p.chars().count() > MAX_BINARY_PATH_CHARS {
        return Err("Command path is too long".to_string());
    }
    if has_control(p) {
        return Err("Command path contains control characters".to_string());
    }
    if !std::path::Path::new(p).is_absolute() {
        return Err("Command path must be absolute".to_string());
    }
    Ok(p.to_string())
}

/// Whitespace-separated extra arguments placed BEFORE the prompt flag.
/// At most [`MAX_EXTRA_ARGS`] tokens of at most [`MAX_ARG_CHARS`] characters,
/// no control characters. Empty input is valid (no extra arguments).
pub fn validate_extra_args(args: &str) -> Result<Vec<String>, String> {
    let tokens: Vec<String> = args.split_whitespace().map(str::to_string).collect();
    if tokens.len() > MAX_EXTRA_ARGS {
        return Err("Too many extra arguments".to_string());
    }
    for t in &tokens {
        if t.chars().count() > MAX_ARG_CHARS {
            return Err("An extra argument is too long".to_string());
        }
        if has_control(t) {
            return Err("An extra argument contains control characters".to_string());
        }
    }
    Ok(tokens)
}

/// `^-{1,2}[A-Za-z0-9][A-Za-z0-9_-]{0,15}$`, hand-rolled (no regex crate).
pub fn validate_prompt_flag(flag: &str) -> Result<String, String> {
    let f = flag.trim();
    let bad = || Err("Prompt flag must look like -p or --prompt".to_string());
    if f.is_empty() || f.chars().count() > MAX_PROMPT_FLAG_CHARS {
        return bad();
    }
    let dashes = f.chars().take_while(|c| *c == '-').count();
    if !(1..=2).contains(&dashes) {
        return bad();
    }
    let rest = &f[dashes..];
    let mut chars = rest.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() => {}
        _ => return bad(),
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return bad();
    }
    if rest.chars().count() > 16 {
        return bad();
    }
    Ok(f.to_string())
}

/// Style hint: control characters are STRIPPED, result trimmed, and an
/// over-long result (after stripping) is rejected.
pub fn sanitize_style(style: &str) -> Result<String, String> {
    let cleaned: String = style.chars().filter(|c| !c.is_control()).collect();
    let cleaned = cleaned.trim().to_string();
    if cleaned.chars().count() > MAX_STYLE_CHARS {
        return Err("Style text is too long".to_string());
    }
    Ok(cleaned)
}

/// Model id: trimmed, bounded, no control characters. Empty is valid here
/// (it means "not picked yet"); callers that need a model check emptiness.
pub fn validate_model(model: &str) -> Result<String, String> {
    let m = model.trim();
    if m.chars().count() > MAX_MODEL_CHARS {
        return Err("Model name is too long".to_string());
    }
    if has_control(m) {
        return Err("Model name contains control characters".to_string());
    }
    Ok(m.to_string())
}

/// The canonical `path \0 args \0 flag` tuple the user acknowledges. `args`
/// is re-joined with single spaces so cosmetic whitespace differences never
/// change the tuple. Fails if any component is invalid.
pub fn normalize_command_tuple(path: &str, args: &str, flag: &str) -> Result<String, String> {
    let p = validate_binary_path(path)?;
    let a = validate_extra_args(args)?.join(" ");
    let f = validate_prompt_flag(flag)?;
    Ok(format!("{p}\0{a}\0{f}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_is_clamped_to_two_through_sixty() {
        assert_eq!(clamp_timeout(0), 2);
        assert_eq!(clamp_timeout(1), 2);
        assert_eq!(clamp_timeout(2), 2);
        assert_eq!(clamp_timeout(30), 30);
        assert_eq!(clamp_timeout(60), 60);
        assert_eq!(clamp_timeout(61), 60);
        assert_eq!(clamp_timeout(u64::MAX), 60);
        assert_eq!(clamp_timeout(TIMEOUT_DEFAULT_SECS), 45);
    }

    #[test]
    fn binary_path_accepts_absolute_and_trims() {
        assert_eq!(
            validate_binary_path("  /usr/local/bin/agent ").unwrap(),
            "/usr/local/bin/agent"
        );
        assert!(validate_binary_path("/a").is_ok());
    }

    #[test]
    fn binary_path_rejects_bad_values() {
        for bad in [
            "",
            "   ",
            "agent",
            "./agent",
            "bin/agent",
            "~/agent",
            "/a\u{0}b",
            "/a\nb",
            "/a\u{1b}b",
        ] {
            assert!(validate_binary_path(bad).is_err(), "should reject {bad:?}");
        }
        let long = format!("/{}", "a".repeat(MAX_BINARY_PATH_CHARS));
        assert!(validate_binary_path(&long).is_err());
        let ok = format!("/{}", "a".repeat(MAX_BINARY_PATH_CHARS - 1));
        assert!(validate_binary_path(&ok).is_ok());
    }

    #[test]
    fn extra_args_split_on_whitespace_and_allow_empty() {
        assert_eq!(validate_extra_args("").unwrap(), Vec::<String>::new());
        assert_eq!(validate_extra_args("   ").unwrap(), Vec::<String>::new());
        assert_eq!(
            validate_extra_args("--no-tools   -m  fast-model").unwrap(),
            vec!["--no-tools", "-m", "fast-model"]
        );
    }

    #[test]
    fn extra_args_limits() {
        assert!(validate_extra_args("a b c d e f g h").is_ok());
        assert!(validate_extra_args("a b c d e f g h i").is_err());
        assert!(validate_extra_args(&"x".repeat(MAX_ARG_CHARS)).is_ok());
        assert!(validate_extra_args(&"x".repeat(MAX_ARG_CHARS + 1)).is_err());
        assert!(validate_extra_args("ok \u{1b}[31m").is_err());
        assert!(validate_extra_args("ok a\u{7}b").is_err());
        // Multibyte characters count as characters, not bytes.
        assert!(validate_extra_args(&"\u{e9}".repeat(MAX_ARG_CHARS)).is_ok());
    }

    #[test]
    fn prompt_flag_accepts_short_and_long_forms() {
        for ok in [
            "-p",
            "--prompt",
            "-P",
            "--p",
            "-p1",
            "--a-b_c",
            "--abcdefghijklmnop",
        ] {
            assert!(validate_prompt_flag(ok).is_ok(), "should accept {ok:?}");
        }
        assert_eq!(validate_prompt_flag(" -p ").unwrap(), "-p");
    }

    #[test]
    fn prompt_flag_rejects_everything_else() {
        for bad in [
            "",
            "p",
            "-",
            "--",
            "---p",
            "-p; rm",
            "-p x",
            "-p$(x)",
            "--a b",
            "-_p",
            "--_p",
            "-p=",
            "-p\nx",
            "--abcdefghijklmnopq",
            "-é",
        ] {
            assert!(validate_prompt_flag(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn style_strips_control_chars_and_bounds_length() {
        assert_eq!(
            sanitize_style("  warm\u{1b}[0m \n and calm ").unwrap(),
            "warm[0m  and calm"
        );
        assert_eq!(sanitize_style("").unwrap(), "");
        assert!(sanitize_style(&"s".repeat(MAX_STYLE_CHARS)).is_ok());
        assert!(sanitize_style(&"s".repeat(MAX_STYLE_CHARS + 1)).is_err());
        // Length is measured AFTER stripping control characters.
        let padded = format!("{}{}", "s".repeat(MAX_STYLE_CHARS), "\u{1}".repeat(50));
        assert!(sanitize_style(&padded).is_ok());
    }

    #[test]
    fn model_is_trimmed_bounded_and_control_free() {
        assert_eq!(validate_model("  gpt-x ").unwrap(), "gpt-x");
        assert_eq!(validate_model("").unwrap(), "");
        assert!(validate_model(&"m".repeat(MAX_MODEL_CHARS)).is_ok());
        assert!(validate_model(&"m".repeat(MAX_MODEL_CHARS + 1)).is_err());
        assert!(validate_model("a\u{0}b").is_err());
        assert!(validate_model("a\nb").is_err());
    }

    #[test]
    fn tuple_is_canonical_and_ignores_cosmetic_whitespace() {
        let a = normalize_command_tuple("/bin/agent", "--no-tools  -m   fast", "-p").unwrap();
        let b = normalize_command_tuple(" /bin/agent ", "--no-tools -m fast", " -p ").unwrap();
        assert_eq!(a, b);
        assert_eq!(a, "/bin/agent\0--no-tools -m fast\0-p");
    }

    #[test]
    fn tuple_differs_when_any_component_differs() {
        let base = normalize_command_tuple("/bin/agent", "-m fast", "-p").unwrap();
        assert_ne!(
            base,
            normalize_command_tuple("/bin/other", "-m fast", "-p").unwrap()
        );
        assert_ne!(
            base,
            normalize_command_tuple("/bin/agent", "-m slow", "-p").unwrap()
        );
        assert_ne!(
            base,
            normalize_command_tuple("/bin/agent", "-m fast", "--prompt").unwrap()
        );
        // Field boundaries cannot be forged by moving text between fields.
        assert_ne!(
            normalize_command_tuple("/bin/agent", "x y", "-p").unwrap(),
            normalize_command_tuple("/bin/agent x", "y", "-p").unwrap()
        );
    }

    #[test]
    fn tuple_fails_if_any_component_is_invalid() {
        assert!(normalize_command_tuple("", "", "-p").is_err());
        assert!(normalize_command_tuple("relative", "", "-p").is_err());
        assert!(normalize_command_tuple("/bin/agent", "a b c d e f g h i", "-p").is_err());
        assert!(normalize_command_tuple("/bin/agent", "", "p").is_err());
    }
}
