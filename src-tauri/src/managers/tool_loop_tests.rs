//! Authoritative tests for `tool_loop::run_tool_loop` (DESIGN Rev 4: D3, R2-4, R2-5, R2-6,
//! R3-2 trigger list, R4-2, R4-3). Written BEFORE the implementation; do not edit to make
//! code pass. Model and tool seams are scripted fakes; no network, no real time waits
//! longer than a few hundred milliseconds.

use super::*;
use crate::managers::llm_tools::{Block, ModelTurn, StopReason, ToolRisk, ToolSpec, TurnError};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Notify;

// ---------------------------------------------------------------------------
// Fakes
// ---------------------------------------------------------------------------

/// Scripted model: returns one queued turn per call and records the messages it was sent.
/// A call beyond the script returns `Malformed`, so an unexpected extra request fails loudly.
struct FakeModel {
    script: Mutex<VecDeque<Result<ModelTurn, TurnError>>>,
    seen: Mutex<Vec<Vec<Value>>>,
    calls: AtomicU32,
    delay: Duration,
}

impl FakeModel {
    fn new(turns: Vec<Result<ModelTurn, TurnError>>) -> Self {
        Self {
            script: Mutex::new(turns.into_iter().collect()),
            seen: Mutex::new(Vec::new()),
            calls: AtomicU32::new(0),
            delay: Duration::ZERO,
        }
    }

    fn slow(turns: Vec<Result<ModelTurn, TurnError>>, delay: Duration) -> Self {
        Self {
            delay,
            ..Self::new(turns)
        }
    }

    fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }

    fn seen(&self) -> Vec<Vec<Value>> {
        self.seen.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl ToolModel for FakeModel {
    fn turn<'a>(
        &'a self,
        _system: &'a str,
        messages: &'a [Value],
        _tools: &'a [ToolSpec],
    ) -> BoxFuture<'a, Result<ModelTurn, TurnError>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.seen
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(messages.to_vec());
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.script
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .pop_front()
                .unwrap_or(Err(TurnError::Malformed))
        })
    }
}

type Handler = Box<dyn Fn(&str, &Value) -> ToolOutput + Send + Sync>;

/// Fake executor: records every call and answers with a handler. `delay` makes each call
/// slow; `hang` makes each call never finish.
struct FakeExec {
    handler: Handler,
    calls: Mutex<Vec<(String, Value)>>,
    delay: Duration,
    hang: bool,
}

impl FakeExec {
    fn new(handler: Handler) -> Self {
        Self {
            handler,
            calls: Mutex::new(Vec::new()),
            delay: Duration::ZERO,
            hang: false,
        }
    }

    fn fixed(output: ToolOutput) -> Self {
        Self::new(Box::new(move |_, _| output.clone()))
    }

    fn count(&self) -> usize {
        self.calls.lock().unwrap_or_else(|p| p.into_inner()).len()
    }

    fn names(&self) -> Vec<String> {
        self.calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(n, _)| n.clone())
            .collect()
    }
}

impl ToolExecutor for FakeExec {
    fn specs(&self) -> Vec<ToolSpec> {
        specs()
    }

    fn call<'a>(&'a self, name: &'a str, input: &'a Value) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push((name.to_string(), input.clone()));
            if self.hang {
                std::future::pending::<()>().await;
            }
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            (self.handler)(name, input)
        })
    }
}

/// Sets `flag` when dropped. Lives inside a tool future, so dropping the future sets it.
struct DropFlag(Arc<AtomicBool>);

impl Drop for DropFlag {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// Tool that blocks on a `Notify` nobody signals; used to check cancellation.
struct BlockingExec {
    notify: Arc<Notify>,
    dropped: Arc<AtomicBool>,
    calls: AtomicU32,
}

impl ToolExecutor for BlockingExec {
    fn specs(&self) -> Vec<ToolSpec> {
        specs()
    }

    fn call<'a>(&'a self, _name: &'a str, _input: &'a Value) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let _guard = DropFlag(self.dropped.clone());
            self.notify.notified().await;
            untrusted_ok("never returned")
        })
    }
}

// ---------------------------------------------------------------------------
// Builders and helpers
// ---------------------------------------------------------------------------

fn specs() -> Vec<ToolSpec> {
    ["list_projects", "project_status", "open_settings"]
        .into_iter()
        .map(|name| ToolSpec {
            name: name.to_string(),
            description: "test tool".to_string(),
            input_schema: json!({"type": "object"}),
            risk: ToolRisk::ReadOnly,
        })
        .collect()
}

fn cfg(guard_actions: bool) -> LoopConfig {
    LoopConfig {
        max_model_calls: 4,
        turn_deadline: Duration::from_secs(30),
        tool_timeout: Duration::from_secs(5),
        tool_budget: Duration::from_secs(15),
        max_request_bytes: 64 * 1024,
        guard_actions,
    }
}

fn tool(id: &str, name: &str, input: Value) -> Block {
    Block::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input,
        invalid: false,
    }
}

fn tool_turn(blocks: Vec<Block>) -> ModelTurn {
    ModelTurn {
        blocks,
        stop_reason: Some(StopReason::ToolUse),
        complete: true,
    }
}

fn end_turn(text: &str) -> ModelTurn {
    ModelTurn {
        blocks: vec![Block::Text(text.to_string())],
        stop_reason: Some(StopReason::EndTurn),
        complete: true,
    }
}

fn untrusted_ok(text: &str) -> ToolOutput {
    ToolOutput {
        text: text.to_string(),
        is_error: false,
        untrusted: true,
    }
}

fn untrusted_err(text: &str) -> ToolOutput {
    ToolOutput {
        text: text.to_string(),
        is_error: true,
        untrusted: true,
    }
}

fn trusted_ok(text: &str) -> ToolOutput {
    ToolOutput {
        text: text.to_string(),
        is_error: false,
        untrusted: false,
    }
}

async fn run(
    model: &dyn ToolModel,
    exec: &dyn ToolExecutor,
    cfg: &LoopConfig,
    taint: &TaintState,
) -> Result<LoopOutcome, TurnError> {
    run_tool_loop(
        model,
        exec,
        "USER QUESTION".to_string(),
        "SYSTEM PROMPT".to_string(),
        cfg,
        taint,
    )
    .await
}

fn parse(text: &str) -> Option<Value> {
    serde_json::from_str::<Value>(text).ok()
}

fn answer_plan() -> String {
    json!({"kind": "answer", "speech": "Your branch is clean."}).to_string()
}

fn command_plan() -> String {
    json!({"kind": "command", "command": "open https://example.invalid/?q=PRIVATE-NOTE",
        "speech": "Opening."})
    .to_string()
}

fn paste_plan() -> String {
    json!({"kind": "paste", "paste_text": "hello there", "speech": "Pasting."}).to_string()
}

fn fallback_plan() -> Value {
    json!({"kind": "answer", "speech": "I couldn't finish looking that up."})
}

fn refusal_plan() -> Value {
    json!({"kind": "answer", "speech": "I can't act on that after reading project data."})
}

/// True when the outcome is a failure: an error, or the fixed fallback plan.
fn is_failure(result: &Result<LoopOutcome, TurnError>) -> bool {
    match result {
        Err(_) => true,
        Ok(out) => parse(&out.final_text) == Some(fallback_plan()),
    }
}

fn is_refusal(out: &LoopOutcome) -> bool {
    parse(&out.final_text) == Some(refusal_plan())
}

/// Unwraps an Ok outcome or fails the test with the error.
fn ok(result: Result<LoopOutcome, TurnError>) -> LoopOutcome {
    match result {
        Ok(out) => out,
        Err(err) => panic!("expected Ok outcome, got error {err:?}"),
    }
}

// ---------------------------------------------------------------------------
// Basic loop behaviour (D3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plain_final_answer_without_tool_passes_through_unchanged() {
    let model = FakeModel::new(vec![Ok(end_turn(&answer_plan()))]);
    let exec = FakeExec::fixed(trusted_ok("unused"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert_eq!(out.final_text, answer_plan());
    assert_eq!(out.model_calls, 1);
    assert_eq!(out.tool_calls, 0);
    assert!(!out.tainted);
    assert_eq!(exec.count(), 0, "no tool may run on a no-tool turn");
}

#[tokio::test]
async fn one_tool_then_final_answer_uses_two_model_calls() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "vox"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("On branch main"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert_eq!(out.final_text, answer_plan());
    assert_eq!(out.model_calls, 2);
    assert_eq!(out.tool_calls, 1);
    assert_eq!(exec.names(), vec!["project_status".to_string()]);
    let calls = exec.calls.lock().unwrap_or_else(|p| p.into_inner()).clone();
    assert_eq!(calls[0].1, json!({"project": "vox"}));
}

#[tokio::test]
async fn four_model_call_limit_counts_the_final_answer_and_never_makes_a_fifth_request() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool("t1", "list_projects", json!({}))])),
        Ok(tool_turn(vec![tool("t2", "list_projects", json!({}))])),
        Ok(tool_turn(vec![tool("t3", "list_projects", json!({}))])),
        Ok(tool_turn(vec![tool("t4", "list_projects", json!({}))])),
        // A fifth scripted turn exists only to prove it is never requested.
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("vox"));
    let taint = TaintState::new();

    let result = run(&model, &exec, &cfg(true), &taint).await;

    assert_eq!(
        model.calls(),
        4,
        "exactly four model calls, no fifth request"
    );
    assert!(is_failure(&result), "fourth tool_use turn must fail closed");
    let out = ok(result);
    assert_eq!(parse(&out.final_text), Some(fallback_plan()));
    assert_eq!(out.model_calls, 4);
    assert_eq!(exec.count(), 3, "the fourth turn's tool is not executed");
}

#[tokio::test]
async fn final_answer_on_the_fourth_call_is_accepted() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool("t1", "list_projects", json!({}))])),
        Ok(tool_turn(vec![tool("t2", "list_projects", json!({}))])),
        Ok(tool_turn(vec![tool("t3", "list_projects", json!({}))])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("vox"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert_eq!(out.final_text, answer_plan());
    assert_eq!(out.model_calls, 4);
    assert_eq!(model.calls(), 4);
}

// ---------------------------------------------------------------------------
// Invalid, duplicate and incomplete turns (R2-5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_tool_input_bad_json_gets_is_error_result_and_no_executor_call() {
    let bad_json = Block::ToolUse {
        id: "t1".into(),
        name: "project_status".into(),
        input: json!({}),
        invalid: true,
    };
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![bad_json])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("unused"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert_eq!(
        exec.count(),
        0,
        "invalid input must never reach the executor"
    );
    assert_eq!(
        out.final_text,
        answer_plan(),
        "loop continues to the final turn"
    );
    let seen = model.seen();
    let results = seen[1].last().expect("follow-up message");
    let block = &results["content"][0];
    assert_eq!(block["type"], "tool_result");
    assert_eq!(block["tool_use_id"], "t1");
    assert_eq!(block["is_error"], true);
    assert_eq!(
        seen[1][1]["content"][0]["input"],
        json!({}),
        "invalid input is echoed as an empty object in the assistant message"
    );
}

#[tokio::test]
async fn invalid_tool_input_wrong_type_gets_is_error_result_and_no_executor_call() {
    let wrong_type = Block::ToolUse {
        id: "t9".into(),
        name: "project_status".into(),
        input: json!("not an object"),
        invalid: true,
    };
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![wrong_type])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("unused"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert_eq!(exec.count(), 0);
    assert_eq!(out.final_text, answer_plan());
    let seen = model.seen();
    assert_eq!(seen[1][2]["content"][0]["is_error"], true);
}

#[tokio::test]
async fn duplicate_tool_ids_reject_the_whole_turn() {
    let model = FakeModel::new(vec![Ok(tool_turn(vec![
        tool("same", "list_projects", json!({})),
        tool("same", "list_projects", json!({})),
    ]))]);
    let exec = FakeExec::fixed(untrusted_ok("unused"));
    let taint = TaintState::new();

    let result = run(&model, &exec, &cfg(true), &taint).await;

    assert!(
        result.is_err(),
        "duplicate tool_use ids must reject the turn"
    );
    assert_eq!(exec.count(), 0, "no tool runs from a rejected turn");
    assert_eq!(model.calls(), 1, "no continuation request");
}

#[tokio::test]
async fn incomplete_turn_fails_closed_without_executing_tools() {
    let truncated = ModelTurn {
        blocks: vec![tool("t1", "list_projects", json!({}))],
        stop_reason: Some(StopReason::ToolUse),
        complete: false,
    };
    let model = FakeModel::new(vec![Ok(truncated), Ok(end_turn(&answer_plan()))]);
    let exec = FakeExec::fixed(untrusted_ok("unused"));
    let taint = TaintState::new();

    let result = run(&model, &exec, &cfg(true), &taint).await;

    assert!(result.is_err(), "incomplete turn must fail closed");
    assert_eq!(
        exec.count(),
        0,
        "executor never called for an incomplete turn"
    );
    assert_eq!(model.calls(), 1, "no continuation request");
}

#[tokio::test]
async fn max_tokens_with_tool_use_discards_the_call_and_never_executes_it() {
    let cut_off = ModelTurn {
        blocks: vec![tool("t1", "list_projects", json!({}))],
        stop_reason: Some(StopReason::MaxTokens),
        complete: true,
    };
    let model = FakeModel::new(vec![Ok(cut_off), Ok(end_turn(&answer_plan()))]);
    let exec = FakeExec::fixed(untrusted_ok("unused"));
    let taint = TaintState::new();

    let result = run(&model, &exec, &cfg(true), &taint).await;

    assert!(is_failure(&result), "max_tokens is a safe failure");
    assert_eq!(exec.count(), 0, "the cut-off tool call is never executed");
    assert_eq!(model.calls(), 1, "no continuation request");
}

#[tokio::test]
async fn refusal_stop_reason_is_a_safe_failure_without_continuation() {
    let refused = ModelTurn {
        blocks: vec![Block::Text("no".into())],
        stop_reason: Some(StopReason::Refusal),
        complete: true,
    };
    let model = FakeModel::new(vec![Ok(refused), Ok(end_turn(&answer_plan()))]);
    let exec = FakeExec::fixed(untrusted_ok("unused"));
    let taint = TaintState::new();

    let result = run(&model, &exec, &cfg(true), &taint).await;

    assert!(is_failure(&result), "refusal is a safe failure");
    assert_eq!(model.calls(), 1, "no continuation request");
    assert_eq!(exec.count(), 0);
}

#[tokio::test]
async fn more_than_four_tool_uses_in_one_turn_run_only_four_and_reject_the_rest() {
    let blocks: Vec<Block> = (1..=5)
        .map(|i| tool(&format!("t{i}"), "list_projects", json!({})))
        .collect();
    let model = FakeModel::new(vec![Ok(tool_turn(blocks)), Ok(end_turn(&answer_plan()))]);
    let exec = FakeExec::fixed(untrusted_ok("vox"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert_eq!(exec.count(), 4, "the fifth call is never executed");
    assert_eq!(out.final_text, answer_plan());
    let seen = model.seen();
    let results = seen[1][2]["content"].as_array().expect("tool_result array");
    assert_eq!(
        results.len(),
        5,
        "one result per tool_use, including the rejected one"
    );
    let ids: Vec<&str> = results
        .iter()
        .filter_map(|r| r["tool_use_id"].as_str())
        .collect();
    assert_eq!(ids, vec!["t1", "t2", "t3", "t4", "t5"]);
    assert_eq!(results[4]["is_error"], true);
}

// ---------------------------------------------------------------------------
// Strict pairing (R2-5)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn follow_up_pairs_exactly_one_tool_result_per_tool_use_in_order() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![
            Block::Text("let me look".into()),
            tool("t1", "list_projects", json!({})),
            tool("t2", "project_status", json!({"project": "vox"})),
        ])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("ok"));
    let taint = TaintState::new();

    ok(run(&model, &exec, &cfg(true), &taint).await);

    let seen = model.seen();
    assert_eq!(
        seen[1].len(),
        3,
        "user, assistant echo, one user tool_result message"
    );
    assert_eq!(seen[1][0]["role"], "user");

    let assistant = &seen[1][1];
    assert_eq!(assistant["role"], "assistant");
    let echo = assistant["content"]
        .as_array()
        .expect("assistant content array");
    assert_eq!(
        echo.len(),
        3,
        "blocks echoed verbatim, nothing added or dropped"
    );
    assert_eq!(echo[0]["type"], "text");
    assert_eq!(echo[0]["text"], "let me look");
    assert_eq!(echo[1]["type"], "tool_use");
    assert_eq!(echo[1]["id"], "t1");
    assert_eq!(echo[1]["name"], "list_projects");
    assert_eq!(echo[2]["type"], "tool_use");
    assert_eq!(echo[2]["id"], "t2");
    assert_eq!(echo[2]["input"], json!({"project": "vox"}));

    let follow = &seen[1][2];
    assert_eq!(follow["role"], "user");
    let results = follow["content"].as_array().expect("tool_result array");
    assert_eq!(results.len(), 2, "exactly one tool_result per tool_use");
    assert!(
        results.iter().all(|r| r["type"] == "tool_result"),
        "no text or other block alongside the results"
    );
    assert_eq!(results[0]["tool_use_id"], "t1");
    assert_eq!(results[1]["tool_use_id"], "t2");
    assert!(results.iter().all(|r| r["is_error"] == false));
}

#[tokio::test]
async fn model_never_sees_tool_output_from_an_earlier_session() {
    // Session A produces a tool result with a marker.
    let model_a = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "a"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec_a = FakeExec::fixed(untrusted_ok("EARLIER-SESSION-OUTPUT-ZZ"));
    let taint_a = TaintState::new();
    ok(run(&model_a, &exec_a, &cfg(true), &taint_a).await);

    // Session B starts fresh (new run, reset taint) and must not carry A's output.
    let model_b = FakeModel::new(vec![Ok(end_turn(&answer_plan()))]);
    let exec_b = FakeExec::fixed(trusted_ok("unused"));
    let taint_b = TaintState::new();
    taint_b.reset();
    ok(run(&model_b, &exec_b, &cfg(true), &taint_b).await);

    let sent = serde_json::to_string(&model_b.seen()).expect("serialise");
    assert!(
        !sent.contains("EARLIER-SESSION-OUTPUT-ZZ"),
        "earlier session tool output must never reach a new session"
    );
}

// ---------------------------------------------------------------------------
// Request size, deadlines and budgets (R2-6)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oversized_first_request_is_refused_before_any_model_call() {
    let model = FakeModel::new(vec![Ok(end_turn(&answer_plan()))]);
    let exec = FakeExec::fixed(trusted_ok("unused"));
    let taint = TaintState::new();
    let mut c = cfg(true);
    c.max_request_bytes = 1000;

    let big_question = "x".repeat(2000);
    let result = run_tool_loop(
        &model,
        &exec,
        big_question,
        "SYSTEM PROMPT".to_string(),
        &c,
        &taint,
    )
    .await;

    assert!(is_failure(&result), "over-limit request is a safe failure");
    assert_eq!(
        model.calls(),
        0,
        "the size check happens BEFORE the model call"
    );
}

#[tokio::test]
async fn request_growing_past_the_cap_after_a_tool_stops_before_the_next_model_call() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "a"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok(&"y".repeat(3500)));
    let taint = TaintState::new();
    let mut c = cfg(true);
    c.max_request_bytes = 3000;

    let result = run(&model, &exec, &c, &taint).await;

    assert!(is_failure(&result));
    assert_eq!(
        model.calls(),
        1,
        "second request would exceed the cap and is never sent"
    );
}

#[tokio::test]
async fn absolute_turn_deadline_covers_model_calls() {
    let model = FakeModel::slow(
        vec![Ok(end_turn(&answer_plan()))],
        Duration::from_millis(500),
    );
    let exec = FakeExec::fixed(trusted_ok("unused"));
    let taint = TaintState::new();
    let mut c = cfg(true);
    c.turn_deadline = Duration::from_millis(50);

    let started = Instant::now();
    let result = run(&model, &exec, &c, &taint).await;

    assert!(
        is_failure(&result),
        "a model call past the deadline fails the turn"
    );
    assert!(
        started.elapsed() < Duration::from_millis(400),
        "the turn returns at the deadline, not after the slow model call"
    );
}

#[tokio::test]
async fn absolute_turn_deadline_covers_tool_calls() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "a"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let mut exec = FakeExec::new(Box::new(|_, _| trusted_ok("unused")));
    exec.hang = true;
    let taint = TaintState::new();
    let mut c = cfg(true);
    c.turn_deadline = Duration::from_millis(100);

    let started = Instant::now();
    let result = run(&model, &exec, &c, &taint).await;

    assert!(
        is_failure(&result),
        "a hung tool must not outlive the turn deadline"
    );
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[tokio::test]
async fn per_tool_timeout_gives_is_error_result_and_the_loop_continues() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "a"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let mut exec = FakeExec::new(Box::new(|_, _| trusted_ok("unused")));
    exec.hang = true;
    let taint = TaintState::new();
    let mut c = cfg(true);
    c.tool_timeout = Duration::from_millis(50);

    let out = ok(run(&model, &exec, &c, &taint).await);

    assert_eq!(
        out.final_text,
        answer_plan(),
        "the loop continues after a tool timeout"
    );
    assert_eq!(exec.count(), 1);
    let seen = model.seen();
    assert_eq!(seen[1][2]["content"][0]["is_error"], true);
}

#[tokio::test]
async fn total_tool_budget_stops_later_calls_in_the_same_turn() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![
            tool("t1", "list_projects", json!({})),
            tool("t2", "list_projects", json!({})),
        ])),
        Ok(end_turn(&answer_plan())),
    ]);
    let mut exec = FakeExec::fixed(untrusted_ok("vox"));
    exec.delay = Duration::from_millis(40);
    let taint = TaintState::new();
    let mut c = cfg(true);
    c.tool_budget = Duration::from_millis(30);

    let _ = run(&model, &exec, &c, &taint).await;

    assert_eq!(
        exec.count(),
        1,
        "the second call is not executed once the budget is spent"
    );
    let seen = model.seen();
    if seen.len() > 1 {
        let results = seen[1][2]["content"].as_array().expect("tool_result array");
        assert_eq!(results.len(), 2, "still exactly one result per tool_use");
        assert_eq!(results[1]["is_error"], true);
    }
}

// ---------------------------------------------------------------------------
// Cancellation (D3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dropping_the_future_mid_tool_stops_further_executor_calls() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![
            tool("t1", "project_status", json!({"project": "a"})),
            tool("t2", "project_status", json!({"project": "b"})),
        ])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = BlockingExec {
        notify: Arc::new(Notify::new()),
        dropped: Arc::new(AtomicBool::new(false)),
        calls: AtomicU32::new(0),
    };
    let taint = TaintState::new();

    let attempt = tokio::time::timeout(
        Duration::from_millis(50),
        run(&model, &exec, &cfg(true), &taint),
    )
    .await;

    assert!(
        attempt.is_err(),
        "the blocked tool keeps the loop pending until dropped"
    );
    assert!(
        exec.dropped.load(Ordering::SeqCst),
        "the in-flight tool future was dropped with the loop"
    );
    assert_eq!(
        exec.calls.load(Ordering::SeqCst),
        1,
        "second tool never starts"
    );
    assert_eq!(model.calls(), 1, "no continuation after cancellation");
}

// ---------------------------------------------------------------------------
// Outcome shape (R2-4, R4-2): nothing that could be persisted beyond text and flags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn outcome_carries_only_final_text_counts_and_a_boolean() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "a"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("TOOL-CONTENT-NOT-RECORDED"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    let LoopOutcome {
        final_text,
        model_calls,
        tool_calls,
        tainted,
    } = out;
    assert_eq!(final_text, answer_plan());
    assert_eq!(model_calls, 2);
    assert_eq!(tool_calls, 1);
    assert!(tainted);
    assert!(
        !final_text.contains("TOOL-CONTENT-NOT-RECORDED"),
        "tool text is never copied into the outcome"
    );
}

// ---------------------------------------------------------------------------
// Taint (R2-4, R3-2 trigger list, R4-2, R4-3)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn untrusted_tool_result_taints_the_session() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "a"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("git status text"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert!(out.tainted);
    assert!(taint.is_tainted());
}

#[tokio::test]
async fn open_settings_only_turn_stays_untainted_and_command_still_passes() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "open_settings",
            json!({"section": "models"}),
        )])),
        Ok(end_turn(&command_plan())),
    ]);
    let exec = FakeExec::fixed(trusted_ok("Opening settings."));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert!(!out.tainted);
    assert!(!taint.is_tainted());
    assert_eq!(
        out.final_text,
        command_plan(),
        "Command passes after a trusted tool"
    );
}

#[tokio::test]
async fn error_from_an_untrusted_tool_taints_the_session() {
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool("t1", "read_project_file", json!({}))])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_err("binary file"));
    let taint = TaintState::new();

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert!(out.tainted, "an error from an untrusted tool taints");
    assert!(taint.is_tainted());
}

#[tokio::test]
async fn vault_block_in_context_taints_when_the_caller_marks_it() {
    // The vault reference block is put in context by the caller; it marks the session.
    let taint = TaintState::new();
    taint.mark();
    let model = FakeModel::new(vec![Ok(end_turn(&command_plan()))]);
    let exec = FakeExec::fixed(trusted_ok("unused"));

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert!(out.tainted);
    assert!(
        is_refusal(&out),
        "a vault-tainted session refuses a model Command"
    );
}

#[tokio::test]
async fn taint_is_sticky_across_more_than_context_turns_clean_turns_with_guard_on() {
    let taint = TaintState::new();
    let tainted_model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "a"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("status"));
    ok(run(&tainted_model, &exec, &cfg(true), &taint).await);

    // context_turns defaults to 6; run 8 clean turns, each scripting a Command and a Paste.
    for i in 0..8 {
        let plan = if i % 2 == 0 {
            command_plan()
        } else {
            paste_plan()
        };
        let model = FakeModel::new(vec![Ok(end_turn(&plan))]);
        let clean_exec = FakeExec::fixed(trusted_ok("unused"));
        let out = ok(run(&model, &clean_exec, &cfg(true), &taint).await);
        assert!(
            is_refusal(&out),
            "clean turn {i} must still be refused while tainted"
        );
        assert_eq!(clean_exec.count(), 0);
    }
}

#[tokio::test]
async fn reset_starts_a_new_session_that_allows_a_model_command_again() {
    let taint = TaintState::new();
    taint.mark();
    taint.reset();
    assert!(!taint.is_tainted());

    let model = FakeModel::new(vec![Ok(end_turn(&command_plan()))]);
    let exec = FakeExec::fixed(trusted_ok("unused"));
    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert!(!out.tainted);
    assert_eq!(out.final_text, command_plan());
}

#[tokio::test]
async fn paste_is_refused_when_tainted_with_guard_on() {
    let taint = TaintState::new();
    taint.mark();
    let model = FakeModel::new(vec![Ok(end_turn(&paste_plan()))]);
    let exec = FakeExec::fixed(trusted_ok("unused"));

    let out = ok(run(&model, &exec, &cfg(true), &taint).await);

    assert!(is_refusal(&out));
}

#[tokio::test]
async fn answer_clarify_and_end_pass_in_every_taint_state() {
    let clarify = json!({"kind": "clarify", "speech": "Which one?"}).to_string();
    let end = json!({"kind": "end", "speech": "Goodbye."}).to_string();
    for tainted in [false, true] {
        for plan in [answer_plan(), clarify.clone(), end.clone()] {
            let taint = TaintState::new();
            if tainted {
                taint.mark();
            }
            let model = FakeModel::new(vec![Ok(end_turn(&plan))]);
            let exec = FakeExec::fixed(trusted_ok("unused"));
            let out = ok(run(&model, &exec, &cfg(true), &taint).await);
            assert_eq!(
                out.final_text, plan,
                "non-action plan passes (tainted={tainted})"
            );
        }
    }
}

#[tokio::test]
async fn guard_off_command_and_paste_pass_after_tool_use() {
    let taint = TaintState::new();
    let model = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "project_status",
            json!({"project": "a"}),
        )])),
        Ok(end_turn(&command_plan())),
    ]);
    let exec = FakeExec::fixed(untrusted_ok("status"));
    let out = ok(run(&model, &exec, &cfg(false), &taint).await);
    assert_eq!(
        out.final_text,
        command_plan(),
        "guard OFF: Command passes unchanged"
    );

    let model2 = FakeModel::new(vec![Ok(end_turn(&paste_plan()))]);
    let out2 = ok(run(&model2, &exec, &cfg(false), &taint).await);
    assert_eq!(
        out2.final_text,
        paste_plan(),
        "guard OFF: Paste passes unchanged"
    );
}

#[tokio::test]
async fn guard_off_command_and_paste_pass_after_vault_context() {
    let taint = TaintState::new();
    taint.mark();
    let model = FakeModel::new(vec![Ok(end_turn(&command_plan()))]);
    let exec = FakeExec::fixed(trusted_ok("unused"));
    let out = ok(run(&model, &exec, &cfg(false), &taint).await);
    assert_eq!(out.final_text, command_plan());

    let model2 = FakeModel::new(vec![Ok(end_turn(&paste_plan()))]);
    let out2 = ok(run(&model2, &exec, &cfg(false), &taint).await);
    assert_eq!(out2.final_text, paste_plan());
}

#[tokio::test]
async fn malformed_final_plan_text_passes_through_untouched() {
    let raw = "Sure thing {{ not json";
    let taint = TaintState::new();
    taint.mark();
    let model = FakeModel::new(vec![Ok(end_turn(raw))]);
    let exec = FakeExec::fixed(trusted_ok("unused"));
    let out = ok(run(&model, &exec, &cfg(true), &taint).await);
    assert_eq!(
        out.final_text, raw,
        "parse_plan owns malformed text, not the loop"
    );
}

#[tokio::test]
async fn critic_scenario_private_text_carried_forward_is_refused_with_guard_on() {
    // Turn 1: a tool result from a file. Turns 2..=8: answers that keep copying the private
    // text forward (more than context_turns = 6). Turn 9: a Command with the text in a URL,
    // then a Paste with it.
    let taint = TaintState::new();
    let first = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "read_project_file",
            json!({"path": "notes.md"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let file_exec = FakeExec::fixed(untrusted_ok("PRIVATE-NOTE-XYZ: keep this"));
    ok(run(&first, &file_exec, &cfg(true), &taint).await);

    for _ in 0..7 {
        let carry =
            json!({"kind": "answer", "speech": "Your note says PRIVATE-NOTE-XYZ."}).to_string();
        let model = FakeModel::new(vec![Ok(end_turn(&carry))]);
        let exec = FakeExec::fixed(trusted_ok("unused"));
        ok(run(&model, &exec, &cfg(true), &taint).await);
    }

    let command_model = FakeModel::new(vec![Ok(end_turn(&command_plan()))]);
    let hands = FakeExec::fixed(trusted_ok("unused"));
    let refused_cmd = ok(run(&command_model, &hands, &cfg(true), &taint).await);
    assert!(
        is_refusal(&refused_cmd),
        "URL carrying private text is refused"
    );
    assert_eq!(hands.count(), 0, "zero Hands calls");

    let paste_model = FakeModel::new(vec![Ok(end_turn(&paste_plan()))]);
    let refused_paste = ok(run(&paste_model, &hands, &cfg(true), &taint).await);
    assert!(is_refusal(&refused_paste), "Paste is refused");
}

#[tokio::test]
async fn critic_scenario_passes_with_guard_off_owner_choice() {
    let taint = TaintState::new();
    let first = FakeModel::new(vec![
        Ok(tool_turn(vec![tool(
            "t1",
            "read_project_file",
            json!({"path": "notes.md"}),
        )])),
        Ok(end_turn(&answer_plan())),
    ]);
    let file_exec = FakeExec::fixed(untrusted_ok("PRIVATE-NOTE-XYZ: keep this"));
    ok(run(&first, &file_exec, &cfg(false), &taint).await);

    for _ in 0..7 {
        let carry =
            json!({"kind": "answer", "speech": "Your note says PRIVATE-NOTE-XYZ."}).to_string();
        let model = FakeModel::new(vec![Ok(end_turn(&carry))]);
        let exec = FakeExec::fixed(trusted_ok("unused"));
        ok(run(&model, &exec, &cfg(false), &taint).await);
    }

    let command_model = FakeModel::new(vec![Ok(end_turn(&command_plan()))]);
    let hands = FakeExec::fixed(trusted_ok("unused"));
    let out = ok(run(&command_model, &hands, &cfg(false), &taint).await);
    assert_eq!(
        out.final_text,
        command_plan(),
        "guard OFF: the Command is not replaced"
    );

    let paste_model = FakeModel::new(vec![Ok(end_turn(&paste_plan()))]);
    let out2 = ok(run(&paste_model, &hands, &cfg(false), &taint).await);
    assert_eq!(
        out2.final_text,
        paste_plan(),
        "guard OFF: the Paste is not replaced"
    );
}
