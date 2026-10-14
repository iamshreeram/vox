//! Native Anthropic tool-use wire format: request builder, SSE block accumulator and the
//! streamed call (DESIGN Rev 4, D2 / R2-5). Signatures are fixed by R4-6.

use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde_json::{Map, Value};

use crate::managers::llm_stream::{
    build_request, client_for, validate_endpoint, SseParser, StreamCall, StreamOpts,
    MAX_TOTAL_BYTES,
};
use crate::managers::speech_compose::ComposeError;

/// At most this many tool_use blocks per model turn; later ones are invalid (never run).
pub const MAX_TOOL_CALLS_PER_TURN: usize = 4;
/// Accumulated tool input JSON cap per call (inclusive).
pub const MAX_TOOL_INPUT_BYTES: usize = 16 * 1024;
/// Serialized request body cap (retained messages plus tool specs), checked before send.
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
/// Upper bound applied to any caller-supplied duration so `Instant` arithmetic cannot overflow.
const MAX_OVERALL: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// Only read-only tools exist in M2; the dispatcher refuses anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolRisk {
    ReadOnly,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub risk: ToolRisk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    ToolUse,
    EndTurn,
    MaxTokens,
    Refusal,
    StopSequence,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Text(String),
    ToolUse {
        id: String,
        name: String,
        input: Value,
        /// Input JSON was missing, malformed, over the cap, or not an object.
        invalid: bool,
    },
}

/// One complete model turn. Acted on only when `complete` and consistent (R2-5).
#[derive(Debug, Clone, PartialEq)]
pub struct ModelTurn {
    pub blocks: Vec<Block>,
    pub stop_reason: Option<StopReason>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TurnError {
    /// Malformed or unexpected SSE framing or event payload.
    Malformed,
    /// Stream ended before `message_stop` or with an unstopped block.
    Truncated,
    /// Tool-use blocks present but stop_reason is not tool_use, or the reverse.
    Inconsistent,
    /// Two tool_use blocks share an id.
    DuplicateToolId,
    /// Provider reported an error event.
    ProviderError,
    Backend(ComposeError),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolResultBlock {
    pub tool_use_id: String,
    pub text: String,
    pub is_error: bool,
}

/// One content block while the stream is still arriving.
#[derive(Debug)]
enum Slot {
    Text { text: String, stopped: bool },
    Tool(ToolSlot),
}

#[derive(Debug)]
struct ToolSlot {
    id: String,
    name: String,
    /// Raw input JSON text accumulated from `input_json_delta` fragments.
    partial: String,
    /// The accumulated input passed `MAX_TOOL_INPUT_BYTES`; the buffer was dropped.
    overflowed: bool,
    /// Zero-based position among the tool_use blocks of this turn.
    ordinal: usize,
    stopped: bool,
    input: Value,
    invalid: bool,
}

impl Slot {
    fn is_stopped(&self) -> bool {
        match self {
            Slot::Text { stopped, .. } => *stopped,
            Slot::Tool(tool) => tool.stopped,
        }
    }

    fn into_block(self) -> Block {
        match self {
            Slot::Text { text, .. } => Block::Text(text),
            Slot::Tool(tool) => Block::ToolUse {
                id: tool.id,
                name: tool.name,
                input: tool.input,
                invalid: tool.invalid,
            },
        }
    }
}

impl ToolSlot {
    fn new(id: String, name: String, ordinal: usize) -> Self {
        Self {
            id,
            name,
            partial: String::new(),
            overflowed: false,
            ordinal,
            stopped: false,
            input: empty_object(),
            invalid: false,
        }
    }

    fn append(&mut self, piece: &str) {
        if self.overflowed {
            return;
        }
        if self.partial.len().saturating_add(piece.len()) > MAX_TOOL_INPUT_BYTES {
            self.overflowed = true;
            self.partial = String::new();
            return;
        }
        self.partial.push_str(piece);
    }

    /// Finalizes the input at `content_block_stop`. Empty input means `{}`; anything that is
    /// not a JSON object, over the cap, or beyond the per-turn call limit is invalid.
    fn close(&mut self) {
        let parsed = if self.ordinal >= MAX_TOOL_CALLS_PER_TURN {
            None
        } else {
            parse_tool_input(&self.partial, self.overflowed)
        };
        self.invalid = parsed.is_none();
        self.input = parsed.unwrap_or_else(empty_object);
        self.partial = String::new();
        self.stopped = true;
    }
}

fn empty_object() -> Value {
    Value::Object(Map::new())
}

fn parse_tool_input(partial: &str, overflowed: bool) -> Option<Value> {
    if overflowed {
        return None;
    }
    if partial.trim().is_empty() {
        return Some(empty_object());
    }
    match serde_json::from_str::<Value>(partial) {
        Ok(value @ Value::Object(_)) => Some(value),
        _ => None,
    }
}

fn parse_stop_reason(raw: &str) -> Result<StopReason, TurnError> {
    match raw {
        "tool_use" => Ok(StopReason::ToolUse),
        "end_turn" => Ok(StopReason::EndTurn),
        "max_tokens" => Ok(StopReason::MaxTokens),
        "refusal" => Ok(StopReason::Refusal),
        "stop_sequence" => Ok(StopReason::StopSequence),
        _ => Err(TurnError::Malformed),
    }
}

fn str_field<'a>(value: &'a Value, key: &str) -> Result<&'a str, TurnError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or(TurnError::Malformed)
}

fn non_empty_field(value: &Value, key: &str) -> Result<String, TurnError> {
    let text = str_field(value, key)?;
    if text.is_empty() {
        return Err(TurnError::Malformed);
    }
    Ok(text.to_string())
}

fn block_index(event: &Value) -> Result<usize, TurnError> {
    event
        .get("index")
        .and_then(Value::as_u64)
        .and_then(|i| usize::try_from(i).ok())
        .ok_or(TurnError::Malformed)
}

/// Accumulates Anthropic SSE events (content_block_start/delta/stop, message_delta,
/// message_stop, error) into one `ModelTurn`.
#[derive(Debug, Default)]
pub struct SseAccumulator {
    /// Index-addressed: block `i` is `slots[i]`; indices must arrive in order without gaps.
    slots: Vec<Slot>,
    stop_reason: Option<StopReason>,
    message_stopped: bool,
}

impl SseAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// True once any content block has started (valid content_block_start seen).
    fn has_started_block(&self) -> bool {
        !self.slots.is_empty()
    }

    fn tool_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|s| matches!(s, Slot::Tool(_)))
            .count()
    }

    pub fn feed_event(&mut self, event: &Value) -> Result<(), TurnError> {
        let kind = str_field(event, "type")?;
        match kind {
            "ping" => Ok(()),
            "error" => Err(TurnError::ProviderError),
            "message_start"
            | "content_block_start"
            | "content_block_delta"
            | "content_block_stop"
            | "message_delta"
            | "message_stop" => {
                if self.message_stopped {
                    return Err(TurnError::Malformed);
                }
                self.handle_known(kind, event)
            }
            _ => Ok(()),
        }
    }

    fn handle_known(&mut self, kind: &str, event: &Value) -> Result<(), TurnError> {
        match kind {
            "content_block_start" => self.block_start(event),
            "content_block_delta" => self.block_delta(event),
            "content_block_stop" => self.block_stop(event),
            "message_delta" => self.message_delta(event),
            "message_stop" => {
                self.message_stopped = true;
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn block_start(&mut self, event: &Value) -> Result<(), TurnError> {
        let index = block_index(event)?;
        if index != self.slots.len() {
            return Err(TurnError::Malformed);
        }
        let block = event.get("content_block").ok_or(TurnError::Malformed)?;
        match str_field(block, "type")? {
            "text" => {
                let text = block
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.slots.push(Slot::Text {
                    text,
                    stopped: false,
                });
            }
            "tool_use" => {
                let id = non_empty_field(block, "id")?;
                let name = non_empty_field(block, "name")?;
                let duplicate = self
                    .slots
                    .iter()
                    .any(|s| matches!(s, Slot::Tool(t) if t.id == id));
                if duplicate {
                    return Err(TurnError::DuplicateToolId);
                }
                let ordinal = self.tool_count();
                self.slots
                    .push(Slot::Tool(ToolSlot::new(id, name, ordinal)));
            }
            _ => return Err(TurnError::Malformed),
        }
        Ok(())
    }

    fn block_delta(&mut self, event: &Value) -> Result<(), TurnError> {
        let index = block_index(event)?;
        let delta = event.get("delta").ok_or(TurnError::Malformed)?;
        let kind = str_field(delta, "type")?;
        let slot = self.slots.get_mut(index).ok_or(TurnError::Malformed)?;
        match (slot, kind) {
            (
                Slot::Text {
                    text,
                    stopped: false,
                },
                "text_delta",
            ) => {
                text.push_str(str_field(delta, "text")?);
            }
            (Slot::Tool(tool), "input_json_delta") if !tool.stopped => {
                tool.append(str_field(delta, "partial_json")?);
            }
            _ => return Err(TurnError::Malformed),
        }
        Ok(())
    }

    fn block_stop(&mut self, event: &Value) -> Result<(), TurnError> {
        let index = block_index(event)?;
        let slot = self.slots.get_mut(index).ok_or(TurnError::Malformed)?;
        if slot.is_stopped() {
            return Err(TurnError::Malformed);
        }
        match slot {
            Slot::Text { stopped, .. } => *stopped = true,
            Slot::Tool(tool) => tool.close(),
        }
        Ok(())
    }

    fn message_delta(&mut self, event: &Value) -> Result<(), TurnError> {
        let delta = event.get("delta").ok_or(TurnError::Malformed)?;
        match delta.get("stop_reason") {
            None | Some(Value::Null) => Ok(()),
            Some(Value::String(raw)) => {
                if self.stop_reason.is_some() {
                    return Err(TurnError::Malformed);
                }
                self.stop_reason = Some(parse_stop_reason(raw)?);
                Ok(())
            }
            Some(_) => Err(TurnError::Malformed),
        }
    }

    pub fn finish(self) -> Result<ModelTurn, TurnError> {
        if !self.message_stopped {
            return Err(TurnError::Truncated);
        }
        if self.slots.iter().any(|s| !s.is_stopped()) {
            return Err(TurnError::Truncated);
        }
        let stop_reason = self.stop_reason.ok_or(TurnError::Malformed)?;
        let has_tool = self.slots.iter().any(|s| matches!(s, Slot::Tool(_)));
        if has_tool != (stop_reason == StopReason::ToolUse) {
            return Err(TurnError::Inconsistent);
        }
        let blocks = self.slots.into_iter().map(Slot::into_block).collect();
        Ok(ModelTurn {
            blocks,
            stop_reason: Some(stop_reason),
            complete: true,
        })
    }
}

/// Builds the Anthropic Messages request body, including `tools` when non-empty.
pub fn build_request_body(
    model: &str,
    system: &str,
    messages: &[Value],
    tools: &[ToolSpec],
    max_tokens: u32,
) -> Value {
    let mut body = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "system": system,
        "messages": Value::Array(messages.to_vec()),
    });
    if !tools.is_empty() {
        let specs: Vec<Value> = tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.input_schema,
                })
            })
            .collect();
        body["tools"] = Value::Array(specs);
    }
    body
}

/// Assistant message echoing the turn's blocks verbatim in order (R2-5). Invalid tool input
/// is echoed as `{}`.
pub fn assistant_message(turn: &ModelTurn) -> Value {
    let content: Vec<Value> = turn
        .blocks
        .iter()
        .map(|block| match block {
            Block::Text(text) => serde_json::json!({"type": "text", "text": text}),
            Block::ToolUse {
                id,
                name,
                input,
                invalid,
            } => {
                let input = if *invalid {
                    empty_object()
                } else {
                    input.clone()
                };
                serde_json::json!({
                    "type": "tool_use",
                    "id": id,
                    "name": name,
                    "input": input,
                })
            }
        })
        .collect();
    serde_json::json!({"role": "assistant", "content": content})
}

/// One user message whose content is the tool_result blocks, in call order.
pub fn tool_results_message(results: &[ToolResultBlock]) -> Value {
    let content: Vec<Value> = results
        .iter()
        .map(|r| {
            serde_json::json!({
                "type": "tool_result",
                "tool_use_id": r.tool_use_id,
                "content": r.text,
                "is_error": r.is_error,
            })
        })
        .collect();
    serde_json::json!({"role": "user", "content": content})
}

fn backend_error() -> TurnError {
    TurnError::Backend(ComposeError::Backend)
}

fn timeout_error() -> TurnError {
    TurnError::Backend(ComposeError::Timeout)
}

/// Time left until `deadline`, zero once it has passed.
fn time_left(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

/// Sends `body` through the existing HTTP/SSE primitives and accumulates one turn.
///
/// The body is size-checked before any connection. The first-event deadline applies until
/// the first content block starts; the overall deadline applies after that and to every
/// chunk. Non-200 responses are never read, so their bodies cannot leak into errors.
pub async fn stream_blocks(
    call: &StreamCall<'_>,
    body: &Value,
    opts: &StreamOpts,
) -> Result<ModelTurn, TurnError> {
    let bytes = serde_json::to_vec(body).map_err(|_| backend_error())?;
    if bytes.len() > MAX_REQUEST_BODY_BYTES {
        return Err(backend_error());
    }
    let start = Instant::now();
    let overall_limit = opts.overall.min(MAX_OVERALL);
    let overall = start + overall_limit;
    let first = start + opts.first_text.min(overall_limit);
    if Instant::now() >= overall {
        return Err(timeout_error());
    }

    let endpoint =
        validate_endpoint(call.base_url, !call.api_key.is_empty()).map_err(|_| backend_error())?;
    let client = client_for(&opts.proxy, endpoint.loopback).ok_or_else(backend_error)?;
    let built = build_request(call);

    let mut req = client.post(built.url.as_str()).body(bytes);
    for (name, value) in &built.headers {
        req = req.header(name.as_str(), value.as_str());
    }
    let resp = match tokio::time::timeout(time_left(first), req.send()).await {
        Err(_) => return Err(timeout_error()),
        Ok(Err(_)) => return Err(backend_error()),
        Ok(Ok(resp)) => resp,
    };
    if resp.status() != reqwest::StatusCode::OK {
        return Err(backend_error());
    }
    read_turn(resp, first, overall).await
}

/// Reads the SSE body chunk by chunk until `message_stop`, an error, a deadline or a limit.
async fn read_turn(
    resp: reqwest::Response,
    first: Instant,
    overall: Instant,
) -> Result<ModelTurn, TurnError> {
    let mut stream = Box::pin(resp.bytes_stream());
    let mut parser = SseParser::new();
    let mut acc = SseAccumulator::new();
    let mut total: usize = 0;
    loop {
        let deadline = if acc.has_started_block() {
            overall
        } else {
            first
        };
        let next = match tokio::time::timeout(time_left(deadline), stream.next()).await {
            Err(_) => return Err(timeout_error()),
            Ok(next) => next,
        };
        let chunk = match next {
            None => break,
            Some(Err(_)) => return Err(backend_error()),
            Some(Ok(chunk)) => chunk,
        };
        total = total.saturating_add(chunk.len());
        if total > MAX_TOTAL_BYTES {
            return Err(backend_error());
        }
        let events = parser.push(&chunk).map_err(|_| backend_error())?;
        for event in events {
            if event.data.is_empty() {
                continue;
            }
            let json: Value =
                serde_json::from_str(&event.data).map_err(|_| TurnError::Malformed)?;
            acc.feed_event(&json)?;
            if acc.message_stopped {
                return acc.finish();
            }
        }
    }
    acc.finish()
}

#[cfg(test)]
#[path = "llm_tools_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "llm_tools_http_tests.rs"]
mod http_tests;
