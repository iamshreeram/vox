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
use std::sync::OnceLock;
use std::time::Duration;

use crate::managers::composer_config::{
    composer_provider_supported, settings_from_app, TtsComposerMode,
};
use crate::managers::llm_stream::{
    self, validate_endpoint, Protocol, ProxyMode, StreamCall, StreamOpts,
};
use crate::managers::realtime_config::{preflight_realtime, RealtimeConfig};
use crate::managers::speech_backends::user_part;
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
pub fn prepare(settings: &AppSettings) -> Result<Option<Prepared>, ComposeError> {
    let cfg = RealtimeConfig::from_settings(settings);
    if !cfg.enabled {
        return Ok(None);
    }
    if cfg.direct_endpoint {
        // A misconfigured direct endpoint never falls back to the composer.
        if preflight_realtime(settings).is_err() {
            return Err(ComposeError::NotConfigured);
        }
        let endpoint = validate_endpoint(&settings.exp_rt_endpoint_url, true)
            .map_err(|_| ComposeError::NotConfigured)?;
        let token = settings.exp_rt_token.as_str();
        let model = settings.exp_rt_model.trim();
        if token.is_empty() || model.is_empty() {
            return Err(ComposeError::NotConfigured);
        }
        return Ok(Some(Prepared {
            protocol: Protocol::AnthropicMessages,
            base_url: endpoint.base,
            api_key: token.to_string(),
            model: model.to_string(),
            json_mode: false,
            completion_tokens_field: false,
            max_tokens: PLANNER_MAX_TOKENS,
        }));
    }
    if !cfg.stream_reply {
        return Ok(None);
    }
    let composer = settings_from_app(settings);
    if composer.mode != TtsComposerMode::Model {
        return Ok(None);
    }
    let model = composer.model.trim();
    if model.is_empty() {
        return Ok(None);
    }
    let Some(provider) = settings
        .post_process_providers
        .iter()
        .find(|p| p.id == composer.provider_id)
    else {
        return Ok(None);
    };
    if !composer_provider_supported(provider) {
        return Ok(None);
    }
    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    Ok(Some(Prepared {
        protocol: Protocol::OpenAiChat,
        base_url: provider.base_url.trim().trim_end_matches('/').to_string(),
        api_key,
        model: model.to_string(),
        json_mode: provider.supports_structured_output,
        completion_tokens_field: false,
        max_tokens: PLANNER_MAX_TOKENS,
    }))
}

/// The base URL to pre-connect to: Some only when the master and the prewarm toggle are on
/// and `prepare` yields a route (never for legacy or a misconfigured direct endpoint).
pub fn prewarm_target(settings: &AppSettings) -> Option<String> {
    if !RealtimeConfig::from_settings(settings).prewarm {
        return None;
    }
    match prepare(settings) {
        Ok(Some(prepared)) => Some(prepared.base_url),
        _ => None,
    }
}

/// `None` => caller uses the legacy path. `combined` is `system + "\n\n" + user` (a bare
/// user part is also accepted, like the legacy `complete`). Proxy resolution:
/// `llm_stream::resolve_proxy(cfg.manual_proxy, cfg.auto_proxy, env, login_shell_probe)`.
/// `deadline` is the overall deadline; first-text = min(FIRST_TEXT_DEADLINE, deadline).
/// Logs only the outcome string. Never touches the network when it returns `None` or a
/// configuration error.
pub async fn try_complete(
    settings: &AppSettings,
    system: &str,
    combined: &str,
    deadline: Duration,
) -> Option<Result<String, ComposeError>> {
    let prepared = match prepare(settings) {
        Ok(None) => return None,
        Ok(Some(prepared)) => prepared,
        Err(err) => {
            log::debug!("converse realtime outcome: {}", err.outcome());
            return Some(Err(err));
        }
    };
    let cfg = RealtimeConfig::from_settings(settings);
    let proxy = resolve_proxy_mode(cfg.manual_proxy, cfg.auto_proxy).await;
    let user = user_part(system, combined);
    let call = StreamCall {
        protocol: prepared.protocol,
        base_url: &prepared.base_url,
        api_key: &prepared.api_key,
        model: &prepared.model,
        system,
        user,
        max_tokens: prepared.max_tokens,
        json_mode: prepared.json_mode,
        completion_tokens_field: prepared.completion_tokens_field,
    };
    let opts = StreamOpts {
        overall: deadline,
        first_text: FIRST_TEXT_DEADLINE.min(deadline),
        proxy,
    };
    let mut on_text = |_: &str| {};
    let result = llm_stream::stream_completion(&call, &opts, &mut on_text).await;
    match &result {
        Ok(_) => log::debug!("converse realtime outcome: ok"),
        Err(err) => log::debug!("converse realtime outcome: {}", err.outcome()),
    }
    Some(result)
}

/// Proxy resolution off the async workers: the login-shell probe may block for up to 2 s
/// (only when no environment proxy exists, and only once per process).
async fn resolve_proxy_mode(manual: Option<String>, auto: bool) -> ProxyMode {
    tauri::async_runtime::spawn_blocking(move || {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        llm_stream::resolve_proxy(manual.as_deref(), auto, &env, &login_shell_probe)
    })
    .await
    .unwrap_or(ProxyMode::SystemDefault)
}

/// Fire-and-forget pre-connect (spawns on the tauri async runtime) when `prewarm_target`
/// is Some. No credentials are sent. Safe to call from any thread; never panics.
pub fn spawn_prewarm(settings: &AppSettings) {
    let Some(base) = prewarm_target(settings) else {
        return;
    };
    let cfg = RealtimeConfig::from_settings(settings);
    tauri::async_runtime::spawn(async move {
        let proxy = resolve_proxy_mode(cfg.manual_proxy, cfg.auto_proxy).await;
        llm_stream::prewarm(&base, &proxy).await;
    });
}

/// macOS/unix only, once per process, cached: asks the user's login shell for its proxy via
/// `probe_shell($SHELL or /bin/zsh, 2 s)`. Elsewhere returns None. Never logs the value.
pub fn login_shell_probe() -> Option<String> {
    static CACHE: OnceLock<Option<String>> = OnceLock::new();
    CACHE.get_or_init(probe_login_shell_once).clone()
}

#[cfg(unix)]
fn probe_login_shell_once() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;

    const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
    const POLL_INTERVAL: Duration = Duration::from_millis(20);
    const MAX_OUTPUT: u64 = 4096;

    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_string());
    let mut child = Command::new(shell)
        .args(["-lc", r#"printf %s "${HTTPS_PROXY:-$https_proxy}""#])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + PROBE_TIMEOUT;
    let succeeded = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status.success(),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !succeeded {
        return None;
    }
    let mut out = String::new();
    child
        .stdout
        .take()?
        .take(MAX_OUTPUT)
        .read_to_string(&mut out)
        .ok()?;
    let trimmed = out.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Runs `{shell} -lc env` (shell-neutral: no `${A:-$b}`, which fish rejects) and returns the
/// first non-empty `HTTPS_PROXY` (preferred) or `https_proxy` value seen within `budget`.
/// Lines are consumed incrementally on a helper thread so a background process that keeps
/// the pipe open cannot block past `budget`; the child is killed when the budget expires.
/// Values longer than `MAX_URL_CHARS` are ignored. Never logs.
#[cfg(unix)]
fn probe_shell(_shell: &str, _budget: Duration) -> Option<String> {
    todo!("WP-probe")
}

#[cfg(not(unix))]
fn probe_login_shell_once() -> Option<String> {
    None
}

#[cfg(test)]
#[path = "realtime_complete_tests.rs"]
mod tests;
