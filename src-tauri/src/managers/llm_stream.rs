//! Streaming chat transport for the realtime conversation (design R5/R6.5/R7/R9).
//!
//! SKELETON (WP1): signatures and doc contracts only; bodies are `todo!()` and are
//! filled in by the implementation step. The tests live in `llm_stream_tests.rs`
//! and are the authoritative spec; they must not be edited to make code pass.
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

use std::time::Duration;

use crate::managers::speech_compose::ComposeError;

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
pub fn validate_endpoint(_url: &str, _key_present: bool) -> Result<Endpoint, EndpointError> {
    todo!("WP1")
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
pub fn build_request(_call: &StreamCall<'_>) -> BuiltRequest {
    todo!("WP1")
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
    _private: (),
}

impl SseParser {
    pub fn new() -> Self {
        todo!("WP1")
    }
    pub fn push(&mut self, _bytes: &[u8]) -> Result<Vec<SseEvent>, SseError> {
        todo!("WP1")
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
pub fn extract_delta(_protocol: Protocol, _json: &serde_json::Value) -> Extracted {
    todo!("WP1")
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
    _manual: Option<&str>,
    _auto: bool,
    _env: &dyn Fn(&str) -> Option<String>,
    _shell_probe: &dyn Fn() -> Option<String>,
) -> ProxyMode {
    todo!("WP1")
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
    _call: &StreamCall<'_>,
    _opts: &StreamOpts,
    _on_text: &mut (dyn FnMut(&str) + Send),
) -> Result<String, ComposeError> {
    todo!("WP1")
}

/// Best-effort connection warm-up: exactly `HEAD {scheme}://{host}[:port]/` (origin root,
/// no path/query/userinfo), no credentials, 3 s timeout, status ignored, same pooled
/// client as `stream_completion` so the later POST reuses the connection.
pub async fn prewarm(_base_url: &str, _proxy: &ProxyMode) {
    todo!("WP1")
}

#[cfg(test)]
#[path = "llm_stream_tests.rs"]
mod tests;
