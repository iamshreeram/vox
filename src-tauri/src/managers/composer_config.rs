//! Settings types and validation for the speech composer. PURE: no I/O.
//! See DESIGN.md sections 0.4, 0.5, 3.4. All string validation goes through
//! `composer_rules` so every path agrees about what is valid.

#![allow(dead_code)]

use crate::managers::composer_rules as rules;
use crate::settings::{AppSettings, PostProcessProvider};
use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Type)]
#[serde(rename_all = "snake_case")]
pub enum TtsComposerMode {
    /// Legacy behavior: speak the reply text as-is.
    #[default]
    Off,
    /// Ask an OpenAI-compatible provider already configured in settings.
    Model,
    /// Run a user-configured local command.
    Command,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Type)]
#[serde(rename_all = "snake_case")]
pub enum TtsComposerFallback {
    /// Speak a fixed short sentence when composition fails.
    #[default]
    ShortNotice,
    /// Speak the (capped) reply verbatim when composition fails.
    Verbatim,
    /// Say nothing when composition fails.
    Silent,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct TtsComposerSettings {
    pub mode: TtsComposerMode,
    pub provider_id: String,
    pub model: String,
    pub command_path: String,
    pub command_args: String,
    pub prompt_flag: String,
    pub command_ack: String,
    pub timeout_secs: u64,
    pub style: String,
    pub send_user_utterance: bool,
    pub fallback: TtsComposerFallback,
}

/// Partial update. `None` = untouched. For strings `Some("")` CLEARS (for
/// `prompt_flag` it resets to the default `-p`; for `provider_id` an empty
/// string is invalid).
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq, Type)]
pub struct TtsComposerSettingsPatch {
    pub mode: Option<TtsComposerMode>,
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub command_path: Option<String>,
    pub command_args: Option<String>,
    pub prompt_flag: Option<String>,
    pub timeout_secs: Option<u64>,
    pub style: Option<String>,
    pub send_user_utterance: Option<bool>,
    pub fallback: Option<TtsComposerFallback>,
    /// Honored ONLY when the same patch also supplies command_path,
    /// command_args and prompt_flag.
    pub acknowledge_command: Option<bool>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct ComposerProviderInfo {
    pub id: String,
    pub label: String,
    pub base_url: String,
}

impl Default for TtsComposerSettings {
    fn default() -> Self {
        TtsComposerSettings {
            mode: TtsComposerMode::Off,
            provider_id: "custom".to_string(),
            model: String::new(),
            command_path: String::new(),
            command_args: String::new(),
            prompt_flag: rules::DEFAULT_PROMPT_FLAG.to_string(),
            command_ack: String::new(),
            timeout_secs: rules::TIMEOUT_DEFAULT_SECS,
            style: rules::DEFAULT_STYLE.to_string(),
            send_user_utterance: true,
            fallback: TtsComposerFallback::ShortNotice,
        }
    }
}

/// Ids that speak plain OpenAI-style `/chat/completions` (see DESIGN 3.4).
pub const SUPPORTED_PROVIDER_IDS: [&str; 6] =
    ["openai", "openrouter", "groq", "cerebras", "zai", "custom"];

/// Whitelisted id AND an http(s) base URL.
pub fn composer_provider_supported(provider: &PostProcessProvider) -> bool {
    if !SUPPORTED_PROVIDER_IDS.contains(&provider.id.as_str()) {
        return false;
    }
    let url = provider.base_url.trim();
    // ASCII lowercasing keeps byte offsets, so the length checks below are exact.
    let lower = url.to_ascii_lowercase();
    let rest_len = if lower.starts_with("https://") {
        url.len() - "https://".len()
    } else if lower.starts_with("http://") {
        url.len() - "http://".len()
    } else {
        return false;
    };
    rest_len > 0
}

/// Supported providers only (the UI shows the endpoint).
pub fn list_composer_providers(providers: &[PostProcessProvider]) -> Vec<ComposerProviderInfo> {
    providers
        .iter()
        .filter(|p| composer_provider_supported(p))
        .map(|p| ComposerProviderInfo {
            id: p.id.clone(),
            label: p.label.clone(),
            base_url: p.base_url.clone(),
        })
        .collect()
}

/// Read the 11 flat `tts_composer_*` fields of AppSettings.
pub fn settings_from_app(app: &AppSettings) -> TtsComposerSettings {
    TtsComposerSettings {
        mode: app.tts_composer_mode,
        provider_id: app.tts_composer_provider_id.clone(),
        model: app.tts_composer_model.clone(),
        command_path: app.tts_composer_command_path.clone(),
        command_args: app.tts_composer_command_args.clone(),
        prompt_flag: app.tts_composer_prompt_flag.clone(),
        command_ack: app.tts_composer_command_ack.clone(),
        timeout_secs: rules::clamp_timeout(app.tts_composer_timeout_secs),
        style: app.tts_composer_style.clone(),
        send_user_utterance: app.tts_composer_send_user_utterance,
        fallback: app.tts_composer_fallback,
    }
}

/// Write all 11 fields back into AppSettings.
pub fn write_to_app(app: &mut AppSettings, s: &TtsComposerSettings) {
    app.tts_composer_mode = s.mode;
    app.tts_composer_provider_id = s.provider_id.clone();
    app.tts_composer_model = s.model.clone();
    app.tts_composer_command_path = s.command_path.clone();
    app.tts_composer_command_args = s.command_args.clone();
    app.tts_composer_prompt_flag = s.prompt_flag.clone();
    app.tts_composer_command_ack = s.command_ack.clone();
    app.tts_composer_timeout_secs = rules::clamp_timeout(s.timeout_secs);
    app.tts_composer_style = s.style.clone();
    app.tts_composer_send_user_utterance = s.send_user_utterance;
    app.tts_composer_fallback = s.fallback;
}

/// True iff `command_ack` is non-empty AND equals the normalized tuple of the
/// CURRENT path/args/flag (`composer_rules::normalize_command_tuple`).
pub fn command_ack_matches(s: &TtsComposerSettings) -> bool {
    if s.command_ack.is_empty() {
        return false;
    }
    match rules::normalize_command_tuple(&s.command_path, &s.command_args, &s.prompt_flag) {
        Ok(tuple) => tuple == s.command_ack,
        Err(_) => false,
    }
}

/// Validate EVERYTHING first, then return the normalized result. Any error =>
/// `Err(message)` and nothing is applied (pure function). Rules:
/// - provider_id: if Some it must be non-empty, exist in `providers` and
///   satisfy `composer_provider_supported`.
/// - model: `validate_model` (Some("") clears). style: `sanitize_style`.
/// - command_path: "" (clear) or `validate_binary_path`. command_args:
///   `validate_extra_args`, stored re-joined with single spaces. prompt_flag:
///   Some("") resets to "-p", else `validate_prompt_flag`.
/// - timeout_secs clamped with `clamp_timeout`.
/// - ACK: acknowledge_command Some(true) is honored only if the same patch
///   supplies command_path, command_args AND prompt_flag (else Err) and the
///   resulting tuple is valid; it stores `normalize_command_tuple(..)`.
///   Some(false) clears the ack. None leaves `command_ack` untouched (so a
///   changed tuple no longer matches). Selecting mode Command never grants it.
pub fn apply_patch(
    current: &TtsComposerSettings,
    patch: &TtsComposerSettingsPatch,
    providers: &[PostProcessProvider],
) -> Result<TtsComposerSettings, String> {
    // Build the whole result in a copy; `current` is never mutated and an
    // early `?` return leaves nothing applied.
    let mut next = current.clone();

    if let Some(mode) = patch.mode {
        next.mode = mode;
    }
    if let Some(id) = &patch.provider_id {
        if id.is_empty() {
            return Err("Pick a provider".to_string());
        }
        let provider = providers
            .iter()
            .find(|p| p.id == *id)
            .ok_or_else(|| "Provider is not configured".to_string())?;
        if !composer_provider_supported(provider) {
            return Err("Provider is not supported for speech composition".to_string());
        }
        next.provider_id = id.clone();
    }
    if let Some(model) = &patch.model {
        next.model = rules::validate_model(model)?;
    }
    if let Some(style) = &patch.style {
        next.style = rules::sanitize_style(style)?;
    }
    if let Some(path) = &patch.command_path {
        next.command_path = if path.is_empty() {
            String::new()
        } else {
            rules::validate_binary_path(path)?
        };
    }
    if let Some(args) = &patch.command_args {
        next.command_args = rules::validate_extra_args(args)?.join(" ");
    }
    if let Some(flag) = &patch.prompt_flag {
        next.prompt_flag = if flag.is_empty() {
            rules::DEFAULT_PROMPT_FLAG.to_string()
        } else {
            rules::validate_prompt_flag(flag)?
        };
    }
    if let Some(timeout) = patch.timeout_secs {
        next.timeout_secs = rules::clamp_timeout(timeout);
    }
    if let Some(send) = patch.send_user_utterance {
        next.send_user_utterance = send;
    }
    if let Some(fallback) = patch.fallback {
        next.fallback = fallback;
    }

    // Acknowledgement is bound to the tuple the same patch supplies.
    match patch.acknowledge_command {
        Some(true) => {
            if patch.command_path.is_none()
                || patch.command_args.is_none()
                || patch.prompt_flag.is_none()
            {
                return Err(
                    "Acknowledging the command needs its path, arguments and prompt flag in the same save"
                        .to_string(),
                );
            }
            next.command_ack = rules::normalize_command_tuple(
                &next.command_path,
                &next.command_args,
                &next.prompt_flag,
            )?;
        }
        Some(false) => next.command_ack.clear(),
        None => {}
    }

    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::get_default_settings;

    fn providers() -> Vec<PostProcessProvider> {
        get_default_settings().post_process_providers
    }

    fn provider(id: &str, url: &str) -> PostProcessProvider {
        PostProcessProvider {
            id: id.to_string(),
            label: id.to_string(),
            base_url: url.to_string(),
            allow_base_url_edit: false,
            models_endpoint: None,
            supports_structured_output: false,
        }
    }

    fn apply(p: TtsComposerSettingsPatch) -> Result<TtsComposerSettings, String> {
        apply_patch(&TtsComposerSettings::default(), &p, &providers())
    }

    fn full_command_patch(
        path: &str,
        args: &str,
        flag: &str,
        ack: Option<bool>,
    ) -> TtsComposerSettingsPatch {
        TtsComposerSettingsPatch {
            command_path: Some(path.to_string()),
            command_args: Some(args.to_string()),
            prompt_flag: Some(flag.to_string()),
            acknowledge_command: ack,
            ..Default::default()
        }
    }

    // ---- defaults and legacy loading ----

    #[test]
    fn defaults_match_the_design() {
        let d = TtsComposerSettings::default();
        assert_eq!(d.mode, TtsComposerMode::Off);
        assert_eq!(d.provider_id, "custom");
        assert_eq!(d.model, "");
        assert_eq!(d.command_path, "");
        assert_eq!(d.command_args, "");
        assert_eq!(d.prompt_flag, "-p");
        assert_eq!(d.command_ack, "");
        assert_eq!(d.timeout_secs, 45);
        assert_eq!(d.style, "warm, upbeat and concise");
        assert!(d.send_user_utterance);
        assert_eq!(d.fallback, TtsComposerFallback::ShortNotice);
    }

    #[test]
    fn app_defaults_equal_composer_defaults() {
        assert_eq!(
            settings_from_app(&get_default_settings()),
            TtsComposerSettings::default()
        );
    }

    #[test]
    fn legacy_settings_json_without_new_keys_loads_with_defaults() {
        let mut v = serde_json::to_value(get_default_settings()).unwrap();
        let obj = v.as_object_mut().unwrap();
        let keys: Vec<String> = obj
            .keys()
            .filter(|k| k.starts_with("tts_composer_"))
            .cloned()
            .collect();
        assert_eq!(
            keys.len(),
            11,
            "expected exactly 11 flat tts_composer_* fields, got {keys:?}"
        );
        for k in keys {
            obj.remove(&k);
        }
        let loaded: AppSettings = serde_json::from_value(v).unwrap();
        assert_eq!(settings_from_app(&loaded), TtsComposerSettings::default());
    }

    #[test]
    fn write_then_read_round_trips_every_field() {
        let s = TtsComposerSettings {
            mode: TtsComposerMode::Command,
            provider_id: "groq".into(),
            model: "m".into(),
            command_path: "/bin/agent".into(),
            command_args: "-m fast".into(),
            prompt_flag: "--prompt".into(),
            command_ack: "tuple".into(),
            timeout_secs: 12,
            style: "calm".into(),
            send_user_utterance: false,
            fallback: TtsComposerFallback::Silent,
        };
        let mut app = get_default_settings();
        write_to_app(&mut app, &s);
        assert_eq!(settings_from_app(&app), s);
        let json = serde_json::to_value(&app).unwrap();
        assert_eq!(json["tts_composer_mode"], "command");
        assert_eq!(json["tts_composer_fallback"], "silent");
        assert_eq!(json["tts_composer_timeout_secs"], 12);
    }

    // ---- provider predicate ----

    #[test]
    fn provider_whitelist_and_url_scheme() {
        for id in SUPPORTED_PROVIDER_IDS {
            assert!(
                composer_provider_supported(&provider(id, "https://x.example/v1")),
                "{id}"
            );
        }
        assert!(composer_provider_supported(&provider(
            "custom",
            "http://localhost:11434/v1"
        )));
        for id in [
            "anthropic",
            "bedrock_mantle",
            "apple_intelligence",
            "apple",
            "unknown",
            "",
        ] {
            assert!(
                !composer_provider_supported(&provider(id, "https://x.example/v1")),
                "{id:?}"
            );
        }
        for url in [
            "",
            "ftp://x",
            "file:///etc",
            "localhost:11434",
            "javascript:alert(1)",
        ] {
            assert!(
                !composer_provider_supported(&provider("openai", url)),
                "{url:?}"
            );
        }
    }

    #[test]
    fn listing_contains_only_supported_providers() {
        let list = list_composer_providers(&providers());
        assert!(!list.is_empty());
        for info in &list {
            assert!(
                SUPPORTED_PROVIDER_IDS.contains(&info.id.as_str()),
                "{}",
                info.id
            );
            assert!(info.base_url.starts_with("http"));
        }
        assert!(!list
            .iter()
            .any(|i| i.id == "anthropic" || i.id == "bedrock_mantle"));
    }

    // ---- apply_patch: semantics ----

    #[test]
    fn empty_patch_changes_nothing() {
        assert_eq!(
            apply(Default::default()).unwrap(),
            TtsComposerSettings::default()
        );
    }

    #[test]
    fn partial_patch_changes_only_the_given_fields() {
        let out = apply(TtsComposerSettingsPatch {
            mode: Some(TtsComposerMode::Model),
            model: Some("  gpt-x ".into()),
            send_user_utterance: Some(false),
            fallback: Some(TtsComposerFallback::Verbatim),
            ..Default::default()
        })
        .unwrap();
        let mut expected = TtsComposerSettings::default();
        expected.mode = TtsComposerMode::Model;
        expected.model = "gpt-x".into();
        expected.send_user_utterance = false;
        expected.fallback = TtsComposerFallback::Verbatim;
        assert_eq!(out, expected);
    }

    #[test]
    fn empty_string_clears_where_clearing_is_meaningful() {
        let mut cur = TtsComposerSettings::default();
        cur.model = "m".into();
        cur.command_path = "/bin/agent".into();
        cur.command_args = "-m x".into();
        cur.style = "calm".into();
        cur.prompt_flag = "--prompt".into();
        let patch = TtsComposerSettingsPatch {
            model: Some(String::new()),
            command_path: Some(String::new()),
            command_args: Some(String::new()),
            style: Some(String::new()),
            prompt_flag: Some(String::new()),
            ..Default::default()
        };
        let out = apply_patch(&cur, &patch, &providers()).unwrap();
        assert_eq!(out.model, "");
        assert_eq!(out.command_path, "");
        assert_eq!(out.command_args, "");
        assert_eq!(out.style, "");
        assert_eq!(out.prompt_flag, "-p");
    }

    #[test]
    fn one_bad_field_rejects_the_whole_patch() {
        let cur = TtsComposerSettings::default();
        let patch = TtsComposerSettingsPatch {
            mode: Some(TtsComposerMode::Model),
            model: Some("fine".into()),
            command_path: Some("relative/path".into()),
            ..Default::default()
        };
        assert!(apply_patch(&cur, &patch, &providers()).is_err());
        // The pure function never mutates its input.
        assert_eq!(cur, TtsComposerSettings::default());
    }

    #[test]
    fn provider_must_exist_be_supported_and_non_empty() {
        assert!(apply(TtsComposerSettingsPatch {
            provider_id: Some("groq".into()),
            ..Default::default()
        })
        .is_ok());
        for bad in ["anthropic", "bedrock_mantle", "nope", ""] {
            let r = apply(TtsComposerSettingsPatch {
                provider_id: Some(bad.into()),
                ..Default::default()
            });
            assert!(r.is_err(), "{bad:?}");
        }
        // A supported id that is missing from the configured providers list.
        let only_openai = vec![provider("openai", "https://x/v1")];
        let r = apply_patch(
            &TtsComposerSettings::default(),
            &TtsComposerSettingsPatch {
                provider_id: Some("groq".into()),
                ..Default::default()
            },
            &only_openai,
        );
        assert!(r.is_err());
    }

    #[test]
    fn timeout_is_clamped_not_rejected() {
        let t = |v: u64| {
            apply(TtsComposerSettingsPatch {
                timeout_secs: Some(v),
                ..Default::default()
            })
            .unwrap()
            .timeout_secs
        };
        assert_eq!(
            (t(0), t(1), t(2), t(45), t(60), t(99), t(u64::MAX)),
            (2, 2, 2, 45, 60, 60, 60)
        );
    }

    #[test]
    fn style_model_path_args_and_flag_validation() {
        let s = |style: &str| {
            apply(TtsComposerSettingsPatch {
                style: Some(style.into()),
                ..Default::default()
            })
        };
        assert_eq!(s("calm\u{1b}[0m kind").unwrap().style, "calm[0m kind");
        assert!(s(&"s".repeat(301)).is_err());
        assert!(s(&"s".repeat(300)).is_ok());
        let m = |model: &str| {
            apply(TtsComposerSettingsPatch {
                model: Some(model.into()),
                ..Default::default()
            })
        };
        assert!(m(&"m".repeat(101)).is_err());
        assert!(m("a\nb").is_err());
        let p = |path: &str| {
            apply(TtsComposerSettingsPatch {
                command_path: Some(path.into()),
                ..Default::default()
            })
        };
        assert!(p("relative").is_err());
        assert!(p("/a\u{0}b").is_err());
        assert_eq!(p(" /bin/agent ").unwrap().command_path, "/bin/agent");
        let a = |args: &str| {
            apply(TtsComposerSettingsPatch {
                command_args: Some(args.into()),
                ..Default::default()
            })
        };
        assert!(a("1 2 3 4 5 6 7 8 9").is_err());
        assert_eq!(
            a("--no-tools   -m  fast").unwrap().command_args,
            "--no-tools -m fast"
        );
        let f = |flag: &str| {
            apply(TtsComposerSettingsPatch {
                prompt_flag: Some(flag.into()),
                ..Default::default()
            })
        };
        assert!(f("-p; rm").is_err());
        assert!(f("p").is_err());
        assert_eq!(f("--prompt").unwrap().prompt_flag, "--prompt");
    }

    // ---- acknowledgement bound to the tuple ----

    #[test]
    fn ack_with_full_tuple_in_one_patch_is_stored_and_matches() {
        let out = apply(full_command_patch(
            "/bin/agent",
            "--no-tools -m fast",
            "-p",
            Some(true),
        ))
        .unwrap();
        assert!(command_ack_matches(&out));
        assert_eq!(out.command_ack, "/bin/agent\0--no-tools -m fast\0-p");
    }

    #[test]
    fn ack_true_without_the_full_tuple_is_an_error() {
        for patch in [
            TtsComposerSettingsPatch {
                acknowledge_command: Some(true),
                ..Default::default()
            },
            TtsComposerSettingsPatch {
                command_path: Some("/bin/agent".into()),
                acknowledge_command: Some(true),
                ..Default::default()
            },
            TtsComposerSettingsPatch {
                command_path: Some("/bin/agent".into()),
                command_args: Some("".into()),
                acknowledge_command: Some(true),
                ..Default::default()
            },
        ] {
            assert!(apply(patch).is_err());
        }
    }

    #[test]
    fn ack_true_with_an_invalid_or_empty_tuple_is_an_error() {
        assert!(apply(full_command_patch("", "", "-p", Some(true))).is_err());
        assert!(apply(full_command_patch("/bin/agent", "", "bad flag", Some(true))).is_err());
    }

    #[test]
    fn changing_path_args_or_flag_without_ack_leaves_the_old_ack_non_matching() {
        let base = apply(full_command_patch(
            "/bin/agent",
            "-m fast",
            "-p",
            Some(true),
        ))
        .unwrap();
        assert!(command_ack_matches(&base));
        let p = |patch: TtsComposerSettingsPatch| apply_patch(&base, &patch, &providers()).unwrap();
        let by_path = p(TtsComposerSettingsPatch {
            command_path: Some("/bin/other".into()),
            ..Default::default()
        });
        let by_args = p(TtsComposerSettingsPatch {
            command_args: Some("-m slow".into()),
            ..Default::default()
        });
        let by_flag = p(TtsComposerSettingsPatch {
            prompt_flag: Some("--prompt".into()),
            ..Default::default()
        });
        for s in [by_path, by_args, by_flag] {
            assert!(!command_ack_matches(&s));
            assert_eq!(
                s.command_ack, base.command_ack,
                "ack text is untouched, it just no longer matches"
            );
        }
    }

    #[test]
    fn restoring_the_acknowledged_tuple_matches_again_and_cosmetic_spacing_does_not_revoke() {
        let base = apply(full_command_patch(
            "/bin/agent",
            "-m fast",
            "-p",
            Some(true),
        ))
        .unwrap();
        let spaced = apply_patch(
            &base,
            &TtsComposerSettingsPatch {
                command_args: Some("-m    fast ".into()),
                ..Default::default()
            },
            &providers(),
        )
        .unwrap();
        assert!(command_ack_matches(&spaced));
    }

    #[test]
    fn ack_false_clears_and_none_leaves_untouched() {
        let base = apply(full_command_patch("/bin/agent", "", "-p", Some(true))).unwrap();
        let cleared = apply_patch(
            &base,
            &TtsComposerSettingsPatch {
                acknowledge_command: Some(false),
                ..Default::default()
            },
            &providers(),
        )
        .unwrap();
        assert_eq!(cleared.command_ack, "");
        assert!(!command_ack_matches(&cleared));
        let kept = apply_patch(&base, &TtsComposerSettingsPatch::default(), &providers()).unwrap();
        assert_eq!(kept.command_ack, base.command_ack);
    }

    #[test]
    fn selecting_command_mode_never_grants_acknowledgement() {
        let out = apply(TtsComposerSettingsPatch {
            mode: Some(TtsComposerMode::Command),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(out.mode, TtsComposerMode::Command);
        assert!(!command_ack_matches(&out));
        let with_cmd = apply(TtsComposerSettingsPatch {
            mode: Some(TtsComposerMode::Command),
            command_path: Some("/bin/agent".into()),
            command_args: Some("".into()),
            prompt_flag: Some("-p".into()),
            ..Default::default()
        })
        .unwrap();
        assert!(!command_ack_matches(&with_cmd));
    }

    #[test]
    fn ack_matches_is_false_for_empty_or_invalid_stored_values() {
        let mut s = TtsComposerSettings::default();
        assert!(!command_ack_matches(&s));
        s.command_ack = "\0\0-p".into();
        assert!(!command_ack_matches(&s));
        s.command_path = "relative".into();
        s.command_ack = "relative\0\0-p".into();
        assert!(!command_ack_matches(&s));
    }
}
