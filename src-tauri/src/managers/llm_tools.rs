// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Native Anthropic tool-use wire format: request builder, SSE block accumulator and the
//! streamed call (DESIGN Rev 4, D2 / R2-5). Signatures are fixed by R4-6.

use serde_json::Value;

use crate::managers::llm_stream::{StreamCall, StreamOpts};
use crate::managers::speech_compose::ComposeError;

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

/// Accumulates Anthropic SSE events (content_block_start/delta/stop, message_delta,
/// message_stop, error) into one `ModelTurn`.
#[derive(Debug, Default)]
pub struct SseAccumulator {
    _private: (),
}

impl SseAccumulator {
    pub fn new() -> Self {
        todo!()
    }

    pub fn feed_event(&mut self, event: &Value) -> Result<(), TurnError> {
        let _ = event;
        todo!()
    }

    pub fn finish(self) -> Result<ModelTurn, TurnError> {
        todo!()
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
    let _ = (model, system, messages, tools, max_tokens);
    todo!()
}

/// Assistant message echoing the turn's blocks verbatim in order (R2-5).
pub fn assistant_message(turn: &ModelTurn) -> Value {
    let _ = turn;
    todo!()
}

/// One user message whose content is the tool_result blocks, in call order.
pub fn tool_results_message(results: &[ToolResultBlock]) -> Value {
    let _ = results;
    todo!()
}

/// Sends `body` through the existing HTTP/SSE primitives and accumulates one turn.
pub async fn stream_blocks(
    call: &StreamCall<'_>,
    body: &Value,
    opts: &StreamOpts,
) -> Result<ModelTurn, TurnError> {
    let _ = (call, body, opts);
    todo!()
}

#[cfg(test)]
#[path = "llm_tools_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "llm_tools_http_tests.rs"]
mod http_tests;
