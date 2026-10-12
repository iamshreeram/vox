//! Routing of conversation planner calls onto the streaming transport (design R9, stage 1).
//!
//! SKELETON: bodies are `todo!("WP-route")`; tests in `realtime_complete_tests.rs` are the spec.
//!
//! Routes (decided from settings only):
//! * master `exp_rt_enabled` OFF            => legacy path (`Ok(None)`), nothing changes.
//! * master on + `exp_rt_direct_endpoint`   => Anthropic-style endpoint from the Experimental
//!   fields (base/model/token). A misconfigured direct endpoint is `Err(NotConfigured)`; it
//!   NEVER silently falls back to the composer.
//! * master on, direct off, `exp_rt_stream_reply` on, composer mode Model with a supported
//!   provider and non-empty model => OpenAI-compatible streaming through the composer's
//!   provider/key/model (`json_mode` = provider.supports_structured_output).
//! * anything else (composer Off/Command, stream_reply off, unsupported provider, empty
//!   model) => legacy path.

use std::fmt;
use std::time::Duration;

use crate::managers::llm_stream::Protocol;
use crate::managers::speech_compose::ComposeError;
use crate::settings::AppSettings;

/// Planner replies may carry up to 8000 characters of paste text.
pub const PLANNER_MAX_TOKENS: u32 = 1500;
/// First-text deadline (absolute, from request start).
pub const FIRST_TEXT_DEADLINE: Duration = Duration::from_secs(20);

#[derive(Clone, PartialEq, Eq)]
pub struct Prepared {
    pub protocol: Protocol,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub json_mode: bool,
    pub completion_tokens_field: bool,
    pub max_tokens: u32,
}

impl fmt::Debug for Prepared {
    /// Never prints the key.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Prepared")
            .field("protocol", &self.protocol)
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("json_mode", &self.json_mode)
            .field("completion_tokens_field", &self.completion_tokens_field)
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

/// See the module docs. `Ok(None)` = use the legacy path.
pub fn prepare(_settings: &AppSettings) -> Result<Option<Prepared>, ComposeError> {
    todo!("WP-route")
}

/// The base URL to pre-connect to: Some only when the master and the prewarm toggle are on
/// and `prepare` yields a route (never for legacy or a misconfigured direct endpoint).
pub fn prewarm_target(_settings: &AppSettings) -> Option<String> {
    todo!("WP-route")
}

/// `None` => caller uses the legacy path. `combined` is `system + "\n\n" + user` (a bare
/// user part is also accepted, like the legacy `complete`). Proxy resolution:
/// `llm_stream::resolve_proxy(cfg.manual_proxy, cfg.auto_proxy, env, login_shell_probe)`.
/// `deadline` is the overall deadline; first-text = min(FIRST_TEXT_DEADLINE, deadline).
/// Logs only the outcome string. Never touches the network when it returns `None` or a
/// configuration error.
pub async fn try_complete(
    _settings: &AppSettings,
    _system: &str,
    _combined: &str,
    _deadline: Duration,
) -> Option<Result<String, ComposeError>> {
    todo!("WP-route")
}

/// Fire-and-forget pre-connect (spawns on the tauri async runtime) when `prewarm_target`
/// is Some. No credentials are sent. Safe to call from any thread; never panics.
pub fn spawn_prewarm(_settings: &AppSettings) {
    todo!("WP-route")
}

/// macOS/unix only, once per process, cached: asks the user's login shell for its proxy
/// (`$SHELL -lc 'printf %s "${HTTPS_PROXY:-$https_proxy}"'`), 2 s timeout, returns the
/// trimmed output when non-empty. Elsewhere returns None. Never logs the value.
pub fn login_shell_probe() -> Option<String> {
    todo!("WP-route")
}

#[cfg(test)]
#[path = "realtime_complete_tests.rs"]
mod tests;
