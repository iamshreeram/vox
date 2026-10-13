//! Streaming chat transport for the realtime conversation (design R5/R6.5/R7/R9).
//!
//! The tests live in `llm_stream_tests.rs` and are the authoritative spec; they must
//! not be edited to make code pass.
//!
//! Contract summary:
//! * Two wire protocols: OpenAI chat-completions SSE and Anthropic Messages SSE.
//! * Never sends `temperature` (some models reject it) and never uses assistant prefill.
//! * One pooled client per (proxy mode, loopback) so consecutive calls and the prewarm
//!   share a TCP connection; loopback targets never use a proxy; redirects never followed.
//! * HTTP non-200, `error` events, truncated streams and refusal/length stops are all
//!   `ComposeError::Backend`; response bodies are never surfaced or logged.
//! * Deadlines are absolute from the start of the request.
//! * Credentials are never logged. Only static strings are logged.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

use crate::managers::speech_compose::ComposeError;

/// Upper bound for any configured deadline; keeps `start + d` from overflowing.
const MAX_DEADLINE: Duration = Duration::from_secs(60 * 60 * 24 * 365);
/// Connect timeout for pooled clients.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Idle keep-alive lifetime for pooled connections.
const POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(300);
/// Upper bound on reading the rest of a body after the terminal event.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(1);
/// Hard cap for the best-effort prewarm HEAD.
const PREWARM_TIMEOUT: Duration = Duration::from_secs(3);

/// Pooled clients keyed by (proxy mode key, loopback).
static CLIENTS: OnceLock<Mutex<HashMap<(String, bool), reqwest::Client>>> = OnceLock::new();

/// Hard limits (design R6.5).
pub const MAX_TOTAL_BYTES: usize = 256 * 1024;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;
pub const MAX_TEXT_BYTES: usize = 64 * 1024;
pub const MAX_IGNORED_EVENTS: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    OpenAiChat,
    AnthropicMessages,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    Invalid,
    Scheme,
    Userinfo,
    Fragment,
    Query,
    NoHost,
    /// Plain http to a non-loopback host while a key is configured.
    InsecureWithKey,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// Normalized base URL: trimmed, no trailing slash, path preserved.
    pub base: String,
    pub loopback: bool,
}

/// scheme http/https only; no userinfo, fragment or query; host required; plain `http`
/// is allowed only for loopback hosts (`localhost`, 127.0.0.0/8, `::1`) or when no key
/// is configured.
pub fn validate_endpoint(url: &str, key_present: bool) -> Result<Endpoint, EndpointError> {
    let trimmed = url.trim();
    let parsed = url::Url::parse(trimmed).map_err(|_| EndpointError::Invalid)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(EndpointError::Scheme);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(EndpointError::Userinfo);
    }
    if parsed.fragment().is_some() {
        return Err(EndpointError::Fragment);
    }
    if parsed.query().is_some() {
        return Err(EndpointError::Query);
    }
    let loopback = match parsed.host().ok_or(EndpointError::NoHost)? {
        url::Host::Domain(d) => d.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(a) => a.is_loopback(),
        url::Host::Ipv6(a) => a.is_loopback(),
    };
    if parsed.scheme() == "http" && !loopback && key_present {
        return Err(EndpointError::InsecureWithKey);
    }
    Ok(Endpoint {
        base: trimmed.trim_end_matches('/').to_string(),
        loopback,
    })
}

#[derive(Debug, Clone)]
pub struct StreamCall<'a> {
    pub protocol: Protocol,
    pub base_url: &'a str,
    /// May be empty (OpenAI-compatible local servers); then no auth header is sent.
    pub api_key: &'a str,
    pub model: &'a str,
    pub system: &'a str,
    pub user: &'a str,
    pub max_tokens: u32,
    /// OpenAI protocol only: send `response_format: json_object`.
    pub json_mode: bool,
    /// OpenAI protocol only: name the limit `max_completion_tokens` instead of `max_tokens`.
    pub completion_tokens_field: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BuiltRequest {
    pub url: String,
    /// Header names are lowercase.
    pub headers: Vec<(String, String)>,
    pub body: serde_json::Value,
}

/// Anthropic: url = base + `/v1/messages` (or `/messages` when base already ends with
/// `/v1`), headers `x-api-key`, `anthropic-version: 2023-06-01`, `content-type`.
/// OpenAI: url = base + `/chat/completions`, `authorization: Bearer` only when a key is set.
/// Bodies always contain `stream: true`; neither ever contains `temperature`.
pub fn build_request(call: &StreamCall<'_>) -> BuiltRequest {
    let base = call.base_url.trim().trim_end_matches('/');
    match call.protocol {
        Protocol::AnthropicMessages => {
            let url = if base.ends_with("/v1") {
                format!("{base}/messages")
            } else {
                format!("{base}/v1/messages")
            };
            let mut headers = Vec::new();
            if !call.api_key.is_empty() {
                headers.push(("x-api-key".to_string(), call.api_key.to_string()));
            }
            headers.push(("anthropic-version".to_string(), "2023-06-01".to_string()));
            headers.push(("content-type".to_string(), "application/json".to_string()));
            let body = serde_json::json!({
                "model": call.model,
                "max_tokens": call.max_tokens,
                "stream": true,
                "system": call.system,
                "messages": [{"role": "user", "content": call.user}],
            });
            BuiltRequest { url, headers, body }
        }
        Protocol::OpenAiChat => {
            let url = format!("{base}/chat/completions");
            let mut headers = Vec::new();
            if !call.api_key.is_empty() {
                headers.push((
                    "authorization".to_string(),
                    format!("Bearer {}", call.api_key),
                ));
            }
            headers.push(("content-type".to_string(), "application/json".to_string()));
            let mut body = serde_json::json!({
                "model": call.model,
                "stream": true,
                "messages": [
                    {"role": "system", "content": call.system},
                    {"role": "user", "content": call.user},
                ],
            });
            let limit_key = if call.completion_tokens_field {
                "max_completion_tokens"
            } else {
                "max_tokens"
            };
            body[limit_key] = serde_json::json!(call.max_tokens);
            if call.json_mode {
                body["response_format"] = serde_json::json!({"type": "json_object"});
            }
            BuiltRequest { url, headers, body }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    /// Multi-line `data:` fields joined with `\n`.
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseError {
    FrameTooLarge,
    TooManyIgnored,
}

/// Incremental SSE parser: chunk-boundary safe (any split point, including inside a
/// multi-byte UTF-8 character or between CR and LF), LF/CRLF/CR line endings, comment
/// lines ignored, `data:` with or without the space. An event is dispatched at its blank
/// line; an unterminated final event is discarded.
pub struct SseParser {
    /// Bytes of the line being assembled; decoded only once the line is complete.
    line: Vec<u8>,
    /// The previous line ended at a CR, so a directly following LF belongs to it.
    skip_lf: bool,
    /// Bytes consumed since the last dispatch (the current frame).
    frame_bytes: usize,
    event: Option<String>,
    data: Vec<String>,
    /// Comment lines plus dispatches without data, cumulative.
    ignored: usize,
}

impl SseParser {
    pub fn new() -> Self {
        Self {
            line: Vec::new(),
            skip_lf: false,
            frame_bytes: 0,
            event: None,
            data: Vec::new(),
            ignored: 0,
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        let mut out = Vec::new();
        for &b in bytes {
            self.frame_bytes += 1;
            if self.frame_bytes > MAX_FRAME_BYTES {
                return Err(SseError::FrameTooLarge);
            }
            if self.skip_lf {
                self.skip_lf = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\r' => {
                    self.end_line(&mut out)?;
                    self.skip_lf = true;
                }
                b'\n' => self.end_line(&mut out)?,
                _ => self.line.push(b),
            }
        }
        Ok(out)
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) -> Result<(), SseError> {
        let raw = std::mem::take(&mut self.line);
        let line = String::from_utf8_lossy(&raw);
        if line.is_empty() {
            return self.dispatch(out);
        }
        if line.starts_with(':') {
            return self.count_ignored();
        }
        let (name, value) = match line.split_once(':') {
            Some((name, value)) => (name, value.strip_prefix(' ').unwrap_or(value)),
            None => (&*line, ""),
        };
        match name {
            "data" => self.data.push(value.to_string()),
            "event" => self.event = Some(value.to_string()),
            _ => {}
        }
        Ok(())
    }

    fn dispatch(&mut self, out: &mut Vec<SseEvent>) -> Result<(), SseError> {
        self.frame_bytes = 0;
        let event = self.event.take();
        if self.data.is_empty() {
            return self.count_ignored();
        }
        let data = self.data.join("\n");
        self.data.clear();
        out.push(SseEvent { event, data });
        Ok(())
    }

    fn count_ignored(&mut self) -> Result<(), SseError> {
        self.ignored += 1;
        if self.ignored > MAX_IGNORED_EVENTS {
            Err(SseError::TooManyIgnored)
        } else {
            Ok(())
        }
    }
}

impl Default for SseParser {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// Not terminal.
    None,
    /// Provider says the answer is complete.
    Done,
    /// Error event, refusal, length or content-filter stop.
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub text: Option<String>,
    pub end: End,
}

/// OpenAI: `choices[0].delta.content` (non-empty) is text; `finish_reason` `stop` => Done,
/// any other non-null finish reason => Error. Anthropic: `content_block_delta` +
/// `text_delta` is text; `message_stop` => Done; `error` or `message_delta` with
/// `stop_reason` `max_tokens`/`refusal` => Error; everything else is ignorable.
/// A chunk may carry text AND an end (OpenAI final chunk).
pub fn extract_delta(protocol: Protocol, json: &serde_json::Value) -> Extracted {
    match protocol {
        Protocol::OpenAiChat => extract_openai(json),
        Protocol::AnthropicMessages => extract_anthropic(json),
    }
}

fn extract_openai(json: &serde_json::Value) -> Extracted {
    if json.get("error").is_some_and(|e| !e.is_null()) {
        return Extracted {
            text: None,
            end: End::Error,
        };
    }
    let Some(choice) = json.get("choices").and_then(|c| c.get(0)) else {
        return Extracted {
            text: None,
            end: End::None,
        };
    };
    let text = choice
        .pointer("/delta/content")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let end = match choice.get("finish_reason") {
        None | Some(serde_json::Value::Null) => End::None,
        Some(serde_json::Value::String(r)) if r == "stop" => End::Done,
        Some(_) => End::Error,
    };
    Extracted { text, end }
}

fn extract_anthropic(json: &serde_json::Value) -> Extracted {
    let plain = |end| Extracted { text: None, end };
    match json.get("type").and_then(serde_json::Value::as_str) {
        Some("content_block_delta") => {
            let delta = json.get("delta");
            let is_text = delta
                .and_then(|d| d.get("type"))
                .and_then(serde_json::Value::as_str)
                == Some("text_delta");
            let text = if is_text {
                delta
                    .and_then(|d| d.get("text"))
                    .and_then(serde_json::Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            } else {
                None
            };
            Extracted {
                text,
                end: End::None,
            }
        }
        Some("message_stop") => plain(End::Done),
        Some("message_delta") => {
            match json
                .pointer("/delta/stop_reason")
                .and_then(serde_json::Value::as_str)
            {
                Some("max_tokens") | Some("refusal") => plain(End::Error),
                _ => plain(End::None),
            }
        }
        Some("error") => plain(End::Error),
        _ => plain(End::None),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyMode {
    /// The HTTP library's own resolution (environment variables and static system proxies).
    SystemDefault,
    /// An explicit proxy URL.
    Explicit(String),
}

/// Resolution order: valid manual URL => Explicit; else when `auto`: a proxy in the
/// environment (`HTTPS_PROXY`, then `https_proxy`) => SystemDefault (the library reads
/// it); else a valid URL from the login-shell probe => Explicit; else SystemDefault.
/// A valid proxy URL is http(s) with a host and no query/fragment (userinfo allowed).
/// Invalid manual values are ignored. The shell probe is only called when needed.
pub fn resolve_proxy(
    manual: Option<&str>,
    auto: bool,
    env: &dyn Fn(&str) -> Option<String>,
    shell_probe: &dyn Fn() -> Option<String>,
) -> ProxyMode {
    if let Some(m) = manual.map(str::trim).filter(|m| is_valid_proxy_url(m)) {
        return ProxyMode::Explicit(m.to_string());
    }
    if auto {
        let env_set = ["HTTPS_PROXY", "https_proxy"]
            .iter()
            .any(|k| env(k).is_some_and(|v| !v.trim().is_empty()));
        if env_set {
            return ProxyMode::SystemDefault;
        }
        if let Some(s) = shell_probe() {
            let s = s.trim();
            if is_valid_proxy_url(s) {
                return ProxyMode::Explicit(s.to_string());
            }
        }
    }
    ProxyMode::SystemDefault
}

/// A valid proxy URL: http(s) scheme, a host, no query and no fragment (userinfo allowed).
pub fn is_valid_proxy_url(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url.trim()) else {
        return false;
    };
    matches!(parsed.scheme(), "http" | "https")
        && parsed.host().is_some()
        && parsed.query().is_none()
        && parsed.fragment().is_none()
}

#[derive(Debug, Clone)]
pub struct StreamOpts {
    /// Absolute from the start of the request (connect included).
    pub overall: Duration,
    /// Absolute from the start of the request; effective = min(first_text, overall).
    pub first_text: Duration,
    pub proxy: ProxyMode,
}

/// Streams one completion. `on_text` receives each text delta as it arrives. Returns the
/// full text once the provider's terminal event arrived. Errors: non-200, redirect, error
/// event, truncated stream, refusal/length stop, malformed framing, over-limit => Backend;
/// too much text => OutputTooLarge; deadlines => Timeout. Dropping the future cancels the
/// request. The endpoint is validated with `validate_endpoint` first (Backend on failure).
pub async fn stream_completion(
    call: &StreamCall<'_>,
    opts: &StreamOpts,
    on_text: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ComposeError> {
    let endpoint = validate_endpoint(call.base_url, !call.api_key.is_empty())
        .map_err(|_| ComposeError::Backend)?;
    let client = client_for(&opts.proxy, endpoint.loopback).ok_or(ComposeError::Backend)?;
    let built = build_request(call);
    let body = serde_json::to_vec(&built.body).map_err(|_| ComposeError::Backend)?;

    let start = Instant::now();
    let overall = start + opts.overall.min(MAX_DEADLINE);
    let first = start + opts.first_text.min(opts.overall).min(MAX_DEADLINE);

    let mut req = client.post(built.url.as_str()).body(body);
    for (k, v) in &built.headers {
        req = req.header(k.as_str(), v.as_str());
    }
    let resp = match tokio::time::timeout(remaining(first), req.send()).await {
        Err(_) => return Err(ComposeError::Timeout),
        Ok(Err(_)) => return Err(ComposeError::Backend),
        Ok(Ok(resp)) => resp,
    };
    // Any status other than 200 (including redirects, which are never followed) is a
    // failure; the body is dropped unread so it cannot leak into errors or logs.
    if resp.status() != reqwest::StatusCode::OK {
        return Err(ComposeError::Backend);
    }

    let mut stream = Box::pin(resp.bytes_stream());
    let result = pump(&mut stream, call, first, overall, on_text).await;
    if result.is_ok() {
        // Read the body to its end (bounded) so hyper can return the connection to the pool.
        drain(&mut stream, overall.min(Instant::now() + DRAIN_TIMEOUT)).await;
    }
    result
}

/// Reads and parses SSE events until a terminal event, an error, a deadline or a limit.
async fn pump<S, T>(
    stream: &mut S,
    call: &StreamCall<'_>,
    first: Instant,
    overall: Instant,
    on_text: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ComposeError>
where
    S: futures_util::Stream<Item = Result<T, reqwest::Error>> + Unpin,
    T: AsRef<[u8]>,
{
    let mut parser = SseParser::new();
    let mut text = String::new();
    let mut total: usize = 0;
    let mut bad_json: usize = 0;
    loop {
        // Before any text the first-text deadline applies; afterwards only the overall one.
        let deadline = if text.is_empty() { first } else { overall };
        let next = match tokio::time::timeout(remaining(deadline), stream.next()).await {
            Err(_) => return Err(ComposeError::Timeout),
            Ok(next) => next,
        };
        let chunk = match next {
            None | Some(Err(_)) => return Err(ComposeError::Backend),
            Some(Ok(chunk)) => chunk,
        };
        let chunk = chunk.as_ref();
        total = total.saturating_add(chunk.len());
        if total > MAX_TOTAL_BYTES {
            return Err(ComposeError::Backend);
        }
        let events = parser.push(chunk).map_err(|_| ComposeError::Backend)?;
        for ev in events {
            if ev.data.is_empty() {
                continue;
            }
            if call.protocol == Protocol::OpenAiChat && ev.data.trim() == "[DONE]" {
                return Ok(text);
            }
            let json: serde_json::Value = match serde_json::from_str(&ev.data) {
                Ok(v) => v,
                Err(_) => {
                    bad_json += 1;
                    if bad_json > MAX_IGNORED_EVENTS {
                        return Err(ComposeError::Backend);
                    }
                    continue;
                }
            };
            let extracted = extract_delta(call.protocol, &json);
            if let Some(delta) = extracted.text.filter(|t| !t.is_empty()) {
                if text.len().saturating_add(delta.len()) > MAX_TEXT_BYTES {
                    return Err(ComposeError::OutputTooLarge);
                }
                text.push_str(&delta);
                on_text(&delta);
            }
            match extracted.end {
                End::Done => return Ok(text),
                End::Error => return Err(ComposeError::Backend),
                End::None => {}
            }
        }
    }
}

/// Best-effort connection warm-up: exactly `HEAD {scheme}://{host}[:port]/` (origin root,
/// no path/query/userinfo), no credentials, 3 s timeout, status ignored, same pooled
/// client as `stream_completion` so the later POST reuses the connection.
pub async fn prewarm(base_url: &str, proxy: &ProxyMode) {
    let Ok(endpoint) = validate_endpoint(base_url, false) else {
        return;
    };
    let Ok(parsed) = url::Url::parse(&endpoint.base) else {
        return;
    };
    let target = format!("{}/", parsed.origin().ascii_serialization());
    let Some(client) = client_for(proxy, endpoint.loopback) else {
        return;
    };
    let _ = tokio::time::timeout(PREWARM_TIMEOUT, client.head(target).send()).await;
}

/// Discards the remaining body until it ends, errors, or `deadline` passes.
async fn drain<S, T>(stream: &mut S, deadline: Instant)
where
    S: futures_util::Stream<Item = Result<T, reqwest::Error>> + Unpin,
{
    while let Ok(Some(Ok(_))) = tokio::time::timeout(remaining(deadline), stream.next()).await {}
}

/// Time left until `deadline`, zero once it has passed.
fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Cache key for a proxy mode (in memory only; never logged).
fn proxy_key(mode: &ProxyMode) -> String {
    match mode {
        ProxyMode::SystemDefault => "system".to_string(),
        ProxyMode::Explicit(url) => format!("explicit:{url}"),
    }
}

/// The pooled client for this proxy mode and loopback flag, built once and reused so
/// consecutive requests share one TCP connection. Loopback targets never use a proxy.
fn client_for(mode: &ProxyMode, loopback: bool) -> Option<reqwest::Client> {
    let cache = CLIENTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = match cache.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let key = (proxy_key(mode), loopback);
    if let Some(client) = guard.get(&key) {
        return Some(client.clone());
    }
    let client = build_client(mode, loopback)?;
    guard.insert(key, client.clone());
    Some(client)
}

fn build_client(mode: &ProxyMode, loopback: bool) -> Option<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .tcp_nodelay(true)
        .pool_idle_timeout(POOL_IDLE_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT);
    if loopback {
        builder = builder.no_proxy();
    } else if let ProxyMode::Explicit(url) = mode {
        let proxy = reqwest::Proxy::all(url.as_str()).ok()?;
        builder = builder.proxy(proxy);
    }
    builder.build().ok()
}

#[cfg(test)]
#[path = "llm_stream_tests.rs"]
mod tests;
