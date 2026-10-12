//! Pure logic for the Experimental "Realtime conversation" group (design R9).
//!
//! SKELETON: bodies are `todo!()`; tests in `realtime_config_tests.rs` are the spec.
//!
//! * `RealtimeConfig::from_settings` resolves EFFECTIVE flags: every sub-flag is false
//!   unless the master `exp_rt_enabled` is on; `early_speech` additionally needs
//!   `stream_reply`; `manual_proxy` is Some only when the manual proxy toggle is on AND the
//!   URL is a valid proxy URL.
//! * `preflight_realtime` returns static user-facing messages only (never echoes values).
//! * `apply_*` are the single validated write paths used by the Tauri commands.

use specta::Type;

use crate::managers::llm_stream::{is_valid_proxy_url, validate_endpoint, EndpointError};
use crate::settings::AppSettings;

pub const MSG_ENDPOINT_MISSING: &str =
    "Enter the endpoint address in the experimental realtime settings first.";
pub const MSG_ENDPOINT_INVALID: &str = "The realtime endpoint address is not valid.";
pub const MSG_ENDPOINT_NEEDS_HTTPS: &str =
    "The realtime endpoint must use https when a token is set.";
pub const MSG_MODEL_MISSING: &str =
    "Enter the model name in the experimental realtime settings first.";
pub const MSG_TOKEN_MISSING: &str =
    "Enter the access token in the experimental realtime settings first.";
pub const MSG_PROXY_INVALID: &str = "The realtime proxy address is not valid.";
pub const MSG_TEXT_INVALID: &str =
    "The value contains characters that are not allowed in the realtime settings.";
pub const MSG_TEXT_TOO_LONG: &str = "The value is too long for the realtime settings.";

pub const MAX_URL_CHARS: usize = 2048;
pub const MAX_MODEL_CHARS: usize = 128;
pub const MAX_TOKEN_CHARS: usize = 8192;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeConfig {
    pub enabled: bool,
    pub direct_endpoint: bool,
    pub stream_reply: bool,
    pub early_speech: bool,
    pub fast_voice: bool,
    pub ack: bool,
    pub prewarm: bool,
    pub auto_proxy: bool,
    pub manual_proxy: Option<String>,
    pub timing_log: bool,
}

impl RealtimeConfig {
    pub fn from_settings(s: &AppSettings) -> Self {
        let enabled = s.exp_rt_enabled;
        let manual_proxy = if enabled && s.exp_rt_proxy_enabled {
            let url = s.exp_rt_proxy_url.trim();
            if is_valid_proxy_url(url) {
                Some(url.to_string())
            } else {
                None
            }
        } else {
            None
        };
        Self {
            enabled,
            direct_endpoint: enabled && s.exp_rt_direct_endpoint,
            stream_reply: enabled && s.exp_rt_stream_reply,
            early_speech: enabled && s.exp_rt_stream_reply && s.exp_rt_early_speech,
            fast_voice: enabled && s.exp_rt_fast_voice,
            ack: enabled && s.exp_rt_ack,
            prewarm: enabled && s.exp_rt_prewarm,
            auto_proxy: enabled && s.exp_rt_auto_proxy,
            manual_proxy,
            timing_log: enabled && s.exp_rt_timing_log,
        }
    }
}

/// Only checks anything when `exp_rt_enabled && exp_rt_direct_endpoint`; the manual proxy
/// is also checked when it is enabled (and the master is on). Order: endpoint missing,
/// model missing, token missing, endpoint invalid (https required: key is present),
/// proxy invalid.
pub fn preflight_realtime(s: &AppSettings) -> Result<(), &'static str> {
    if !s.exp_rt_enabled {
        return Ok(());
    }
    if s.exp_rt_direct_endpoint {
        let url = s.exp_rt_endpoint_url.trim();
        if url.is_empty() {
            return Err(MSG_ENDPOINT_MISSING);
        }
        if s.exp_rt_model.trim().is_empty() {
            return Err(MSG_MODEL_MISSING);
        }
        if s.exp_rt_token.is_empty() {
            return Err(MSG_TOKEN_MISSING);
        }
        match validate_endpoint(url, true) {
            Ok(_) => {}
            Err(EndpointError::InsecureWithKey) => return Err(MSG_ENDPOINT_NEEDS_HTTPS),
            Err(_) => return Err(MSG_ENDPOINT_INVALID),
        }
    }
    if s.exp_rt_proxy_enabled && !is_valid_proxy_url(s.exp_rt_proxy_url.trim()) {
        return Err(MSG_PROXY_INVALID);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ExpRtBool {
    Enabled,
    DirectEndpoint,
    StreamReply,
    EarlySpeech,
    FastVoice,
    Ack,
    Prewarm,
    AutoProxy,
    ProxyEnabled,
    TimingLog,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum ExpRtText {
    EndpointUrl,
    Model,
    Token,
    ProxyUrl,
}

pub fn apply_exp_rt_bool(s: &mut AppSettings, key: ExpRtBool, value: bool) {
    match key {
        ExpRtBool::Enabled => s.exp_rt_enabled = value,
        ExpRtBool::DirectEndpoint => s.exp_rt_direct_endpoint = value,
        ExpRtBool::StreamReply => s.exp_rt_stream_reply = value,
        ExpRtBool::EarlySpeech => s.exp_rt_early_speech = value,
        ExpRtBool::FastVoice => s.exp_rt_fast_voice = value,
        ExpRtBool::Ack => s.exp_rt_ack = value,
        ExpRtBool::Prewarm => s.exp_rt_prewarm = value,
        ExpRtBool::AutoProxy => s.exp_rt_auto_proxy = value,
        ExpRtBool::ProxyEnabled => s.exp_rt_proxy_enabled = value,
        ExpRtBool::TimingLog => s.exp_rt_timing_log = value,
    }
}

/// Trims; rejects control characters; rejects values over the length caps; the token also
/// rejects any whitespace inside; a non-empty endpoint must pass
/// `llm_stream::validate_endpoint(value, true)`; a non-empty proxy must pass
/// `llm_stream::is_valid_proxy_url`. Empty clears the field. On Err the settings are
/// unchanged and the message is static (never echoes the value).
pub fn apply_exp_rt_text(
    s: &mut AppSettings,
    key: ExpRtText,
    value: &str,
) -> Result<(), &'static str> {
    let v = value.trim();
    let cap = match key {
        ExpRtText::EndpointUrl | ExpRtText::ProxyUrl => MAX_URL_CHARS,
        ExpRtText::Model => MAX_MODEL_CHARS,
        ExpRtText::Token => MAX_TOKEN_CHARS,
    };
    if v.chars().count() > cap {
        return Err(MSG_TEXT_TOO_LONG);
    }
    if v.chars().any(char::is_control) {
        return Err(MSG_TEXT_INVALID);
    }
    match key {
        ExpRtText::EndpointUrl => {
            if v.is_empty() {
                s.exp_rt_endpoint_url = String::new();
            } else {
                let ep = validate_endpoint(v, true).map_err(|e| match e {
                    EndpointError::InsecureWithKey => MSG_ENDPOINT_NEEDS_HTTPS,
                    _ => MSG_ENDPOINT_INVALID,
                })?;
                s.exp_rt_endpoint_url = ep.base;
            }
        }
        ExpRtText::Model => s.exp_rt_model = v.to_string(),
        ExpRtText::Token => {
            if v.chars().any(char::is_whitespace) {
                return Err(MSG_TEXT_INVALID);
            }
            s.exp_rt_token = v.into();
        }
        ExpRtText::ProxyUrl => {
            if v.is_empty() {
                s.exp_rt_proxy_url = String::new();
            } else if is_valid_proxy_url(v) {
                s.exp_rt_proxy_url = v.to_string();
            } else {
                return Err(MSG_PROXY_INVALID);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "realtime_config_tests.rs"]
mod tests;
