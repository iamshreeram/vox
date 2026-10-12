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
    pub fn from_settings(_s: &AppSettings) -> Self {
        todo!("WP-settings")
    }
}

/// Only checks anything when `exp_rt_enabled && exp_rt_direct_endpoint`; the manual proxy
/// is also checked when it is enabled (and the master is on). Order: endpoint missing,
/// model missing, token missing, endpoint invalid (https required: key is present),
/// proxy invalid.
pub fn preflight_realtime(_s: &AppSettings) -> Result<(), &'static str> {
    todo!("WP-settings")
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

pub fn apply_exp_rt_bool(_s: &mut AppSettings, _key: ExpRtBool, _value: bool) {
    todo!("WP-settings")
}

/// Trims; rejects control characters; rejects values over the length caps; the token also
/// rejects any whitespace inside; a non-empty endpoint must pass
/// `llm_stream::validate_endpoint(value, true)`; a non-empty proxy must pass
/// `llm_stream::is_valid_proxy_url`. Empty clears the field. On Err the settings are
/// unchanged and the message is static (never echoes the value).
pub fn apply_exp_rt_text(
    _s: &mut AppSettings,
    _key: ExpRtText,
    _value: &str,
) -> Result<(), &'static str> {
    todo!("WP-settings")
}

#[cfg(test)]
#[path = "realtime_config_tests.rs"]
mod tests;
