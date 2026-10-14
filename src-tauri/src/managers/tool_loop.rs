//! Bounded tool loop around one voice turn (DESIGN Rev 4, D3 / R2-4 / R2-6 / R4-2).
//! Pure over the `ToolModel` and `ToolExecutor` seams so tests can script both.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::managers::converse::BoxFuture;
use crate::managers::llm_tools::{
    assistant_message, build_request_body, tool_results_message, Block, ModelTurn, StopReason,
    ToolResultBlock, ToolSpec, TurnError,
};

/// Tool calls executed or attempted per model turn; calls beyond this are rejected (R2-5).
const MAX_TOOL_CALLS_PER_TURN: usize = 4;
const REFUSAL_SPEECH: &str = "I can't act on that after reading project data.";
const FALLBACK_SPEECH: &str = "I couldn't finish looking that up.";

/// One absolute time budget; every operation receives `min(own limit, remaining)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Deadline {
    start: Instant,
    limit: Duration,
}

impl Deadline {
    pub fn new(start: Instant, limit: Duration) -> Self {
        Self { start, limit }
    }

    pub fn remaining(&self) -> Duration {
        self.limit.saturating_sub(self.start.elapsed())
    }

    pub fn expired(&self) -> bool {
        self.remaining().is_zero()
    }

    /// `min(limit, remaining)`. Kept for the deadline contract; tests exercise it.
    #[allow(dead_code)]
    pub fn cap(&self, limit: Duration) -> Duration {
        limit.min(self.remaining())
    }
}

/// Model seam: one streamed model turn with the current transcript and tool specs.
pub trait ToolModel: Send + Sync {
    fn turn<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [Value],
        tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<ModelTurn, TurnError>>;
}

/// Result of one tool call, already capped and neutralised by the executor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
    /// `true` when the text came from outside Vox (files, git, conversations). `open_settings`
    /// returns `false` (R4-2).
    pub untrusted: bool,
}

/// Tool seam: specs for the model plus one call by name.
pub trait ToolExecutor: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;

    fn call<'a>(&'a self, name: &'a str, input: &'a Value) -> BoxFuture<'a, ToolOutput>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopConfig {
    /// Total model calls per voice turn, including the final answer call (R2-6: 4).
    pub max_model_calls: u32,
    /// Whole-turn deadline (R2-6: 30 s).
    pub turn_deadline: Duration,
    /// Per tool call (R2-6: 5 s).
    pub tool_timeout: Duration,
    /// Total tool wall budget per turn (R2-6: 15 s).
    pub tool_budget: Duration,
    /// Retained request body cap (R2-6: 64 KiB).
    pub max_request_bytes: usize,
    /// Mirrors `agent_tools_guard_actions` (R4-2).
    pub guard_actions: bool,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            max_model_calls: 4,
            turn_deadline: Duration::from_secs(30),
            tool_timeout: Duration::from_secs(5),
            tool_budget: Duration::from_secs(15),
            max_request_bytes: 64 * 1024,
            guard_actions: true,
        }
    }
}

/// Sticky per-session taint flag (R4-2). Cloning shares the same flag.
#[derive(Debug, Clone, Default)]
pub struct TaintState {
    flag: Arc<AtomicBool>,
}

impl TaintState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_tainted(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub fn mark(&self) {
        self.flag.store(true, Ordering::SeqCst);
    }

    /// Called when a new session starts. Sessions get a fresh `TaintState` today, so the
    /// production path does not call this; kept for the reset contract.
    #[allow(dead_code)]
    pub fn reset(&self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopOutcome {
    /// Final planner text, or the fixed refusal or fallback plan JSON.
    pub final_text: String,
    pub model_calls: u32,
    pub tool_calls: u32,
    /// Boolean only; never content (R4-2).
    pub tainted: bool,
}

/// Fixed plan JSON with an answer speech.
fn fixed_plan(speech: &str) -> String {
    json!({"kind": "answer", "speech": speech}).to_string()
}

/// Failure outcome: the fixed fallback plan, with the counts so far.
fn fallback_outcome(model_calls: u32, tool_calls: u32, taint: &TaintState) -> LoopOutcome {
    LoopOutcome {
        final_text: fixed_plan(FALLBACK_SPEECH),
        model_calls,
        tool_calls,
        tainted: taint.is_tainted(),
    }
}

/// Serialised request size of the messages plus tool specs, as the model would send them.
fn request_bytes(system: &str, messages: &[Value], specs: &[ToolSpec]) -> usize {
    serde_json::to_vec(&build_request_body("", system, messages, specs, 0))
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

/// Structural checks on a model turn before any of its tool calls may run (R2-5).
fn check_framing(turn: &ModelTurn) -> Result<(), TurnError> {
    if !turn.complete {
        return Err(TurnError::Truncated);
    }
    let mut seen = HashSet::new();
    for block in &turn.blocks {
        if let Block::ToolUse { id, .. } = block {
            if !seen.insert(id.as_str()) {
                return Err(TurnError::DuplicateToolId);
            }
        }
    }
    let has_tools = turn
        .blocks
        .iter()
        .any(|block| matches!(block, Block::ToolUse { .. }));
    let expects_tools = turn.stop_reason == Some(StopReason::ToolUse);
    if has_tools != expects_tools {
        return Err(TurnError::Inconsistent);
    }
    Ok(())
}

/// Concatenated text of a final turn's blocks.
fn final_text_of(turn: &ModelTurn) -> String {
    turn.blocks
        .iter()
        .filter_map(|block| match block {
            Block::Text(text) => Some(text.as_str()),
            Block::ToolUse { .. } => None,
        })
        .collect()
}

/// Replaces a Paste or Command final plan with the fixed refusal when the session is tainted
/// and the guard is on (R4-2). Text that does not parse is passed through untouched.
fn apply_guard(text: String, guard_actions: bool, tainted: bool) -> String {
    if !guard_actions || !tainted {
        return text;
    }
    let is_action = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|plan| {
            plan.get("kind")
                .and_then(Value::as_str)
                .map(|kind| kind == "paste" || kind == "command")
        })
        .unwrap_or(false);
    if is_action {
        fixed_plan(REFUSAL_SPEECH)
    } else {
        text
    }
}

/// Result of one tool_use block: the paired tool_result plus bookkeeping.
struct ToolStep {
    result: ToolResultBlock,
    executed: bool,
    tainted: bool,
}

/// Limits shared by every tool call in one turn.
struct ToolCtx<'a> {
    exec: &'a dyn ToolExecutor,
    specs: &'a [ToolSpec],
    budget: Deadline,
    turn: Deadline,
    tool_timeout: Duration,
}

/// A rejected call: never executed, paired with an error result, and tainting (the rejection
/// text is an error message, and the session was already exposed to tool traffic).
fn rejected(tool_use_id: &str, text: &str) -> ToolStep {
    ToolStep {
        result: ToolResultBlock {
            tool_use_id: tool_use_id.to_string(),
            text: text.to_string(),
            is_error: true,
        },
        executed: false,
        tainted: true,
    }
}

/// Runs one tool_use block, or rejects it without execution. Every call yields exactly one
/// tool_result, so the transcript stays paired (R2-5).
async fn run_one_tool(
    ctx: &ToolCtx<'_>,
    tool_use_id: &str,
    name: &str,
    input: &Value,
    invalid: bool,
    index: usize,
) -> ToolStep {
    if invalid || !input.is_object() {
        return rejected(tool_use_id, "invalid tool input");
    }
    if index >= MAX_TOOL_CALLS_PER_TURN {
        return rejected(tool_use_id, "too many tool calls in one turn");
    }
    if !ctx.specs.iter().any(|spec| spec.name == name) {
        return rejected(tool_use_id, "unknown tool");
    }
    let limit = ctx
        .tool_timeout
        .min(ctx.budget.remaining())
        .min(ctx.turn.remaining());
    if limit.is_zero() {
        return rejected(tool_use_id, "tool budget exhausted");
    }
    match tokio::time::timeout(limit, ctx.exec.call(name, input)).await {
        Ok(output) => ToolStep {
            result: ToolResultBlock {
                tool_use_id: tool_use_id.to_string(),
                text: output.text,
                is_error: output.is_error,
            },
            executed: true,
            tainted: output.untrusted,
        },
        // The timeout drops the call future, which kills any in-flight work it owns.
        Err(_) => ToolStep {
            result: ToolResultBlock {
                tool_use_id: tool_use_id.to_string(),
                text: "tool timed out".to_string(),
                is_error: true,
            },
            executed: true,
            tainted: true,
        },
    }
}

/// Runs the bounded loop for one voice turn (DESIGN D3, R2-4..R2-6, R4-2).
pub async fn run_tool_loop(
    model: &dyn ToolModel,
    exec: &dyn ToolExecutor,
    first_user: String,
    system: String,
    cfg: &LoopConfig,
    taint: &TaintState,
) -> Result<LoopOutcome, TurnError> {
    let turn = Deadline::new(Instant::now(), cfg.turn_deadline);
    let specs = exec.specs();
    let ctx = ToolCtx {
        exec,
        specs: &specs,
        budget: Deadline::new(Instant::now(), cfg.tool_budget),
        turn,
        tool_timeout: cfg.tool_timeout,
    };
    let mut messages = vec![json!({"role": "user", "content": first_user})];
    let mut model_calls: u32 = 0;
    let mut tool_calls: u32 = 0;

    loop {
        if turn.expired() || request_bytes(&system, &messages, &specs) > cfg.max_request_bytes {
            return Ok(fallback_outcome(model_calls, tool_calls, taint));
        }
        model_calls += 1;
        let reply =
            match tokio::time::timeout(turn.remaining(), model.turn(&system, &messages, &specs))
                .await
            {
                Ok(result) => result?,
                Err(_) => return Ok(fallback_outcome(model_calls, tool_calls, taint)),
            };

        if matches!(
            reply.stop_reason,
            Some(StopReason::MaxTokens) | Some(StopReason::Refusal)
        ) {
            return Ok(fallback_outcome(model_calls, tool_calls, taint));
        }
        check_framing(&reply)?;

        let has_tools = reply
            .blocks
            .iter()
            .any(|block| matches!(block, Block::ToolUse { .. }));
        if !has_tools {
            let text = final_text_of(&reply);
            return Ok(LoopOutcome {
                final_text: apply_guard(text, cfg.guard_actions, taint.is_tainted()),
                model_calls,
                tool_calls,
                tainted: taint.is_tainted(),
            });
        }
        if model_calls >= cfg.max_model_calls {
            return Ok(fallback_outcome(model_calls, tool_calls, taint));
        }

        messages.push(assistant_message(&reply));
        let mut results = Vec::new();
        let mut index = 0usize;
        for block in &reply.blocks {
            if let Block::ToolUse {
                id,
                name,
                input,
                invalid,
            } = block
            {
                let step = run_one_tool(&ctx, id, name, input, *invalid, index).await;
                index += 1;
                if step.executed {
                    tool_calls += 1;
                }
                if step.tainted {
                    taint.mark();
                }
                results.push(step.result);
            }
        }
        messages.push(tool_results_message(&results));
    }
}

#[cfg(test)]
#[path = "tool_loop_tests.rs"]
mod tests;
