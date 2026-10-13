//! Routing of conversation planner calls onto the streaming transport (design R9, stage 1).
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
use std::time::{Duration, Instant};

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
/// Upper bound on proxy resolution on the request path and in the prewarm task.
const PROXY_RESOLVE_BUDGET: Duration = Duration::from_secs(3);

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
    let mut noop = |_: &str| {};
    try_complete_streaming(settings, system, combined, deadline, &mut noop).await
}

/// Same as `try_complete`, additionally passing every raw text delta of the reply to
/// `on_text` as it arrives (never called on the legacy `None` path or on a configuration
/// error). The callback must be quick and non-blocking.
pub async fn try_complete_streaming(
    settings: &AppSettings,
    system: &str,
    combined: &str,
    deadline: Duration,
    on_text: &mut (dyn FnMut(&str) + Send),
) -> Option<Result<String, ComposeError>> {
    let prepared = match prepare(settings) {
        Ok(None) => return None,
        Ok(Some(prepared)) => prepared,
        Err(err) => {
            log::debug!("converse realtime outcome: {}", err.outcome());
            return Some(Err(err));
        }
    };
    let started = Instant::now();
    let cfg = RealtimeConfig::from_settings(settings);
    let proxy = resolve_proxy_mode(
        cfg.manual_proxy,
        cfg.auto_proxy,
        PROXY_RESOLVE_BUDGET.min(deadline),
    )
    .await;
    // Proxy resolution counts against the overall deadline.
    let remaining = deadline.saturating_sub(started.elapsed());
    if remaining.is_zero() {
        log::debug!(
            "converse realtime outcome: {}",
            ComposeError::Timeout.outcome()
        );
        return Some(Err(ComposeError::Timeout));
    }
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
        overall: remaining,
        first_text: FIRST_TEXT_DEADLINE.min(remaining),
        proxy,
    };
    let result = llm_stream::stream_completion(&call, &opts, on_text).await;
    match &result {
        Ok(_) => log::debug!("converse realtime outcome: ok"),
        Err(err) => log::debug!("converse realtime outcome: {}", err.outcome()),
    }
    Some(result)
}

/// Proxy resolution off the async workers, bounded by `budget`. The login-shell probe may
/// block for up to 2 s (only when no environment proxy exists, and only once per process).
/// On timeout the caller gets `SystemDefault`; the blocking task is detached and keeps
/// running so the once-per-process cache still gets filled.
async fn resolve_proxy_mode(manual: Option<String>, auto: bool, budget: Duration) -> ProxyMode {
    let task = tauri::async_runtime::spawn_blocking(move || {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        llm_stream::resolve_proxy(manual.as_deref(), auto, &env, &login_shell_probe)
    });
    match tokio::time::timeout(budget, task).await {
        Ok(Ok(mode)) => mode,
        _ => ProxyMode::SystemDefault,
    }
}

/// Fire-and-forget pre-connect (spawns on the tauri async runtime) when `prewarm_target`
/// is Some. No credentials are sent. Safe to call from any thread; never panics.
pub fn spawn_prewarm(settings: &AppSettings) {
    let Some(base) = prewarm_target(settings) else {
        return;
    };
    let cfg = RealtimeConfig::from_settings(settings);
    tauri::async_runtime::spawn(async move {
        let proxy =
            resolve_proxy_mode(cfg.manual_proxy, cfg.auto_proxy, PROXY_RESOLVE_BUDGET).await;
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
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "/bin/zsh".to_string());
    probe_shell(&shell, Duration::from_secs(2))
}

/// Runs `{shell} -lc env` (shell-neutral: no `${A:-$b}`, which fish rejects) and returns the
/// first non-empty `HTTPS_PROXY` (preferred) or `https_proxy` value seen within `budget`.
/// Lines are consumed incrementally on a helper thread so a background process that keeps
/// the pipe open cannot block past `budget`; the child is killed when the budget expires.
/// Values longer than `MAX_URL_CHARS` are ignored. Never logs.
#[cfg(unix)]
fn probe_shell(shell: &str, budget: Duration) -> Option<String> {
    use std::io::{BufRead, BufReader, Read};
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{self, RecvTimeoutError};

    /// Longest line considered; longer lines are discarded.
    const MAX_LINE: usize = 8192;

    let mut child = Command::new(shell)
        .args(["-lc", "env"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .ok()?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return None;
    };

    let (tx, rx) = mpsc::channel::<String>();
    // Detached: exits at EOF, or when the child group is killed below, or when `rx` is dropped.
    std::thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            let mut line = Vec::new();
            let limit = (MAX_LINE + 1) as u64;
            match reader.by_ref().take(limit).read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            if line.last() != Some(&b'\n') && line.len() as u64 >= limit {
                // Overlong line: discard through its newline.
                if !discard_through_newline(&mut reader) {
                    break;
                }
                continue;
            }
            if line.len() > MAX_LINE {
                continue;
            }
            let text = String::from_utf8_lossy(&line);
            let text = text.trim_end_matches(['\r', '\n']).to_string();
            if tx.send(text).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + budget;
    let mut upper: Option<String> = None;
    let mut lower: Option<String> = None;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let line = match rx.recv_timeout(remaining) {
            Ok(line) => line,
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
        };
        if let Some(v) = line.strip_prefix("HTTPS_PROXY=") {
            if upper.is_none() {
                upper = accept_probe_value(v);
            }
            if upper.is_some() {
                break;
            }
        } else if let Some(v) = line.strip_prefix("https_proxy=") {
            if lower.is_none() {
                lower = accept_probe_value(v);
            }
        }
    }

    // Kill the whole group while the leader is still unreaped (its pgid stays reserved),
    // then reap it on every path so no zombie is left.
    if let Ok(pid) = libc::pid_t::try_from(child.id()) {
        // SAFETY: plain signal delivery to our own child's process group; no memory is touched.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    upper.or(lower)
}

/// Drops a trimmed, non-empty value that exceeds the URL cap. Never logs the value.
#[cfg(unix)]
fn accept_probe_value(raw: &str) -> Option<String> {
    let v = raw.trim();
    if v.is_empty() || v.chars().count() > crate::managers::realtime_config::MAX_URL_CHARS {
        None
    } else {
        Some(v.to_string())
    }
}

/// Reads and drops bytes up to and including the next newline. False on EOF or error.
#[cfg(unix)]
fn discard_through_newline<R: std::io::BufRead>(reader: &mut R) -> bool {
    loop {
        let (found_newline, used) = match reader.fill_buf() {
            Ok(buf) if !buf.is_empty() => match buf.iter().position(|&b| b == b'\n') {
                Some(i) => (true, i + 1),
                None => (false, buf.len()),
            },
            _ => return false,
        };
        reader.consume(used);
        if found_newline {
            return true;
        }
    }
}

#[cfg(not(unix))]
fn probe_login_shell_once() -> Option<String> {
    None
}

#[cfg(test)]
#[path = "realtime_complete_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "realtime_complete_stream_tests.rs"]
mod stream_tests;

#[cfg(test)]
#[path = "realtime_live_tests.rs"]
mod live_tests;
