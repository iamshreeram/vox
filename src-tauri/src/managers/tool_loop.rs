// M2 stub: bodies arrive in M2-1..M2-3, this allow is removed in M2-3
#![allow(dead_code)]

//! Bounded tool loop around one voice turn (DESIGN Rev 4, D3 / R2-4 / R2-6 / R4-2).
//! Pure over the `ToolModel` and `ToolExecutor` seams so tests can script both.

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::managers::converse::BoxFuture;
use crate::managers::llm_tools::{ModelTurn, ToolSpec, TurnError};

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

    /// `min(limit, remaining)`.
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

/// Sticky per-session taint flag (R4-2). Cloning shares the same flag.
#[derive(Debug, Clone, Default)]
pub struct TaintState {
    flag: Arc<AtomicBool>,
}

impl TaintState {
    pub fn new() -> Self {
        todo!()
    }

    pub fn is_tainted(&self) -> bool {
        todo!()
    }

    pub fn mark(&self) {
        todo!()
    }

    /// Called when a new session starts.
    pub fn reset(&self) {
        todo!()
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

/// Runs the bounded loop for one voice turn (DESIGN D3, R2-4..R2-6, R4-2).
pub async fn run_tool_loop(
    model: &dyn ToolModel,
    exec: &dyn ToolExecutor,
    first_user: String,
    system: String,
    cfg: &LoopConfig,
    taint: &TaintState,
) -> Result<LoopOutcome, TurnError> {
    let _ = (model, exec, first_user, system, cfg, taint);
    todo!()
}

#[cfg(test)]
#[path = "tool_loop_tests.rs"]
mod tests;
