//! Authoritative tests for `llm_tools` wire format and SSE block accumulation
//! (DESIGN Rev 4: D2, R2-5). Written BEFORE the implementation; do not edit to make
//! code pass. Pure: no network, no filesystem.

use super::*;
use crate::managers::speech_compose::ComposeError;
use serde_json::{json, Value};

// ---------------------------------------------------------------------------
// Event builders (Anthropic Messages SSE payloads)
// ---------------------------------------------------------------------------

fn msg_start() -> Value {
    json!({"type": "message_start", "message": {"id": "msg_1", "type": "message",
        "role": "assistant", "content": [], "model": "m", "stop_reason": null,
        "usage": {"input_tokens": 1, "output_tokens": 1}}})
}

fn ping() -> Value {
    json!({"type": "ping"})
}

fn text_start(index: u64) -> Value {
    json!({"type": "content_block_start", "index": index,
        "content_block": {"type": "text", "text": ""}})
}

fn tool_start(index: u64, id: &str, name: &str) -> Value {
    json!({"type": "content_block_start", "index": index,
        "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}})
}

fn text_delta(index: u64, text: &str) -> Value {
    json!({"type": "content_block_delta", "index": index,
        "delta": {"type": "text_delta", "text": text}})
}

fn json_delta(index: u64, partial: &str) -> Value {
    json!({"type": "content_block_delta", "index": index,
        "delta": {"type": "input_json_delta", "partial_json": partial}})
}

fn block_stop(index: u64) -> Value {
    json!({"type": "content_block_stop", "index": index})
}

fn msg_delta(stop: &str) -> Value {
    json!({"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": null},
        "usage": {"output_tokens": 5}})
}

fn msg_stop() -> Value {
    json!({"type": "message_stop"})
}

/// A complete, well-framed message: message_start, body events, message_delta, message_stop.
fn frame(body: Vec<Value>, stop: &str) -> Vec<Value> {
    let mut events = vec![msg_start()];
    events.extend(body);
    events.push(msg_delta(stop));
    events.push(msg_stop());
    events
}

/// Feeds every event into a fresh accumulator and finishes it; returns the first error.
fn run(events: &[Value]) -> Result<ModelTurn, TurnError> {
    let mut acc = SseAccumulator::new();
    for event in events {
        acc.feed_event(event)?;
    }
    acc.finish()
}

fn expect_turn(events: &[Value]) -> ModelTurn {
    run(events).unwrap_or_else(|e| panic!("expected a complete turn, got {e:?}"))
}

fn tool(id: &str, name: &str, input: Value, invalid: bool) -> Block {
    Block::ToolUse {
        id: id.to_string(),
        name: name.to_string(),
        input,
        invalid,
    }
}

/// A result that does not hand the model a valid tool call: an error, or an Ok turn with
/// no valid (executable) tool_use block. Used where the spec only requires that nothing
/// is acted on.
fn assert_no_valid_tool_use(result: &Result<ModelTurn, TurnError>) {
    if let Ok(turn) = result {
        let valid_calls = turn
            .blocks
            .iter()
            .filter(|b| matches!(b, Block::ToolUse { invalid: false, .. }))
            .count();
        assert_eq!(valid_calls, 0, "a valid tool call survived: {turn:?}");
    }
}

// ---------------------------------------------------------------------------
// SseAccumulator: text turns and framing
// ---------------------------------------------------------------------------

#[test]
fn text_only_turn_is_complete_and_end_turn() {
    let events = frame(
        vec![
            text_start(0),
            text_delta(0, "Hel"),
            text_delta(0, "lo"),
            block_stop(0),
        ],
        "end_turn",
    );
    let turn = expect_turn(&events);
    assert_eq!(turn.blocks, vec![Block::Text("Hello".to_string())]);
    assert_eq!(turn.stop_reason, Some(StopReason::EndTurn));
    assert!(turn.complete);
}

#[test]
fn ping_and_message_start_events_are_ignored() {
    let events = vec![
        ping(),
        msg_start(),
        ping(),
        text_start(0),
        text_delta(0, "ok"),
        block_stop(0),
        ping(),
        msg_delta("end_turn"),
        msg_stop(),
    ];
    let turn = expect_turn(&events);
    assert_eq!(turn.blocks, vec![Block::Text("ok".to_string())]);
    assert!(turn.complete);
}

#[test]
fn stop_sequence_text_turn_reports_stop_sequence() {
    let events = frame(
        vec![text_start(0), text_delta(0, "done"), block_stop(0)],
        "stop_sequence",
    );
    let turn = expect_turn(&events);
    assert_eq!(turn.stop_reason, Some(StopReason::StopSequence));
    assert!(turn
        .blocks
        .iter()
        .all(|b| !matches!(b, Block::ToolUse { .. })));
}

#[test]
fn empty_event_stream_is_truncated() {
    assert_eq!(run(&[]), Err(TurnError::Truncated));
}

#[test]
fn stream_without_message_stop_is_truncated() {
    let events = vec![
        msg_start(),
        text_start(0),
        text_delta(0, "hi"),
        block_stop(0),
        msg_delta("end_turn"),
    ];
    assert_eq!(run(&events), Err(TurnError::Truncated));
}

#[test]
fn unstopped_tool_block_before_message_stop_is_truncated() {
    let events = vec![
        msg_start(),
        tool_start(0, "toolu_u", "git_status"),
        json_delta(0, "{}"),
        msg_delta("tool_use"),
        msg_stop(),
    ];
    assert_eq!(run(&events), Err(TurnError::Truncated));
}

#[test]
fn error_event_mid_stream_is_provider_error() {
    let events = vec![
        msg_start(),
        text_start(0),
        text_delta(0, "partial"),
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "busy"}}),
    ];
    assert_eq!(run(&events), Err(TurnError::ProviderError));
}

#[test]
fn event_missing_index_is_malformed() {
    let events = vec![
        msg_start(),
        json!({"type": "content_block_start", "content_block": {"type": "text", "text": ""}}),
        msg_stop(),
    ];
    assert_eq!(run(&events), Err(TurnError::Malformed));
}

#[test]
fn delta_for_never_started_index_is_malformed() {
    let events = frame(
        vec![text_start(0), text_delta(3, "stray"), block_stop(0)],
        "end_turn",
    );
    assert_eq!(run(&events), Err(TurnError::Malformed));
}

#[test]
fn block_index_gap_is_malformed() {
    // Judgment call (spec silent on gaps): a skipped index aborts the turn, fail closed.
    let events = frame(
        vec![text_start(0), block_stop(0), text_start(2), block_stop(2)],
        "end_turn",
    );
    assert_eq!(run(&events), Err(TurnError::Malformed));
}

// ---------------------------------------------------------------------------
// SseAccumulator: tool_use input accumulation
// ---------------------------------------------------------------------------

#[test]
fn tool_use_input_split_across_three_chunks_is_reassembled() {
    let events = frame(
        vec![
            tool_start(0, "toolu_1", "read_file"),
            json_delta(0, "{\"path\":"),
            json_delta(0, "\"src/"),
            json_delta(0, "main.rs\"}"),
            block_stop(0),
        ],
        "tool_use",
    );
    let turn = expect_turn(&events);
    assert_eq!(
        turn.blocks,
        vec![tool(
            "toolu_1",
            "read_file",
            json!({"path": "src/main.rs"}),
            false
        )]
    );
    assert_eq!(turn.stop_reason, Some(StopReason::ToolUse));
    assert!(turn.complete);
}

#[test]
fn multibyte_text_split_across_partial_json_deltas_is_reassembled() {
    let events = frame(
        vec![
            tool_start(0, "toolu_mb", "search"),
            json_delta(0, "{\"q\":\"caf"),
            json_delta(0, "\u{e9} \u{2603}"),
            json_delta(0, " \u{65e5}\u{672c}\"}"),
            block_stop(0),
        ],
        "tool_use",
    );
    let turn = expect_turn(&events);
    assert_eq!(
        turn.blocks,
        vec![tool(
            "toolu_mb",
            "search",
            json!({"q": "caf\u{e9} \u{2603} \u{65e5}\u{672c}"}),
            false
        )]
    );
}

#[test]
fn tool_use_without_any_input_delta_has_empty_object_input() {
    let events = frame(
        vec![tool_start(0, "toolu_e", "list_projects"), block_stop(0)],
        "tool_use",
    );
    let turn = expect_turn(&events);
    assert_eq!(
        turn.blocks,
        vec![tool("toolu_e", "list_projects", json!({}), false)]
    );
}

#[test]
fn empty_accumulated_partial_json_means_empty_object_and_is_valid() {
    let events = frame(
        vec![
            tool_start(0, "toolu_z", "list_projects"),
            json_delta(0, ""),
            block_stop(0),
        ],
        "tool_use",
    );
    let turn = expect_turn(&events);
    assert_eq!(
        turn.blocks,
        vec![tool("toolu_z", "list_projects", json!({}), false)]
    );
}

#[test]
fn invalid_accumulated_json_marks_the_call_invalid() {
    for partial in ["{\"path\":", "{\"a\":1}}", "{'single': 'quotes'}"] {
        let events = frame(
            vec![
                tool_start(0, "toolu_bad", "read_file"),
                json_delta(0, partial),
                block_stop(0),
            ],
            "tool_use",
        );
        let turn = expect_turn(&events);
        match &turn.blocks[..] {
            [Block::ToolUse { id, invalid, .. }] => {
                assert_eq!(id, "toolu_bad", "partial {partial:?}");
                assert!(*invalid, "partial {partial:?} must be invalid");
            }
            other => panic!("partial {partial:?}: unexpected blocks {other:?}"),
        }
    }
}

#[test]
fn non_object_tool_input_is_invalid() {
    for partial in ["[1,2]", "42", "\"text\"", "null"] {
        let events = frame(
            vec![
                tool_start(0, "toolu_nonobj", "read_file"),
                json_delta(0, partial),
                block_stop(0),
            ],
            "tool_use",
        );
        let turn = expect_turn(&events);
        match &turn.blocks[..] {
            [Block::ToolUse { invalid, .. }] => assert!(*invalid, "partial {partial:?}"),
            other => panic!("partial {partial:?}: unexpected blocks {other:?}"),
        }
    }
}

#[test]
fn tool_input_exactly_at_16_kib_is_valid() {
    // `{"q":"` (6) + n x 'a' + `"}` (2) = 16384 bytes, the inclusive cap.
    let n = 16_384 - 8;
    let value = "a".repeat(n);
    let whole = format!("{{\"q\":\"{value}\"}}");
    assert_eq!(whole.len(), 16_384);
    let (first, second) = whole.split_at(whole.len() / 2);
    let events = frame(
        vec![
            tool_start(0, "toolu_cap", "search"),
            json_delta(0, first),
            json_delta(0, second),
            block_stop(0),
        ],
        "tool_use",
    );
    let turn = expect_turn(&events);
    match &turn.blocks[..] {
        [Block::ToolUse { invalid, input, .. }] => {
            assert!(!*invalid, "16384-byte input must be accepted");
            assert_eq!(input["q"].as_str().map(str::len), Some(n));
        }
        other => panic!("unexpected blocks {other:?}"),
    }
}

#[test]
fn tool_input_one_byte_over_16_kib_is_invalid() {
    let n = 16_384 - 7;
    let whole = format!("{{\"q\":\"{}\"}}", "a".repeat(n));
    assert_eq!(whole.len(), 16_385);
    let (first, second) = whole.split_at(whole.len() / 2);
    let events = frame(
        vec![
            tool_start(0, "toolu_big", "search"),
            json_delta(0, first),
            json_delta(0, second),
            block_stop(0),
        ],
        "tool_use",
    );
    let turn = expect_turn(&events);
    match &turn.blocks[..] {
        [Block::ToolUse { invalid, .. }] => assert!(*invalid, "16385-byte input must be rejected"),
        other => panic!("unexpected blocks {other:?}"),
    }
}

#[test]
fn text_then_tool_use_keeps_block_order() {
    let events = frame(
        vec![
            text_start(0),
            text_delta(0, "Let me check."),
            block_stop(0),
            tool_start(1, "toolu_a", "git_status"),
            json_delta(1, "{\"project\":\"vox\"}"),
            block_stop(1),
        ],
        "tool_use",
    );
    let turn = expect_turn(&events);
    assert_eq!(
        turn.blocks,
        vec![
            Block::Text("Let me check.".to_string()),
            tool("toolu_a", "git_status", json!({"project": "vox"}), false),
        ]
    );
}

#[test]
fn two_tool_blocks_interleaved_with_text_accumulate_per_index() {
    let events = frame(
        vec![
            text_start(0),
            text_delta(0, "One"),
            tool_start(1, "toolu_a", "read_file"),
            tool_start(2, "toolu_b", "list_projects"),
            json_delta(2, "{\"b\":"),
            json_delta(1, "{\"a\":1"),
            json_delta(2, "2}"),
            json_delta(1, "}"),
            block_stop(1),
            block_stop(2),
            block_stop(0),
        ],
        "tool_use",
    );
    let turn = expect_turn(&events);
    assert_eq!(
        turn.blocks,
        vec![
            Block::Text("One".to_string()),
            tool("toolu_a", "read_file", json!({"a": 1}), false),
            tool("toolu_b", "list_projects", json!({"b": 2}), false),
        ]
    );
}

#[test]
fn fifth_tool_block_is_never_a_valid_executable_call() {
    // Spec (R2-5): calls beyond 4 get is_error results and are NOT executed. The spec does
    // not say whether the accumulator reports this as a turn error or as an invalid block,
    // so both are accepted. In either case the first four must stay valid and the fifth
    // must never be a valid call.
    let mut body = Vec::new();
    for i in 0..5u64 {
        body.push(tool_start(i, &format!("toolu_{i}"), "list_projects"));
        body.push(json_delta(i, "{}"));
        body.push(block_stop(i));
    }
    let result = run(&frame(body, "tool_use"));
    match &result {
        Err(_) => {}
        Ok(turn) => {
            let tools: Vec<&Block> = turn
                .blocks
                .iter()
                .filter(|b| matches!(b, Block::ToolUse { .. }))
                .collect();
            assert_eq!(tools.len(), 5, "all five blocks kept in order: {turn:?}");
            for (i, block) in tools.iter().enumerate() {
                match block {
                    Block::ToolUse { id, invalid, .. } => {
                        assert_eq!(id, &format!("toolu_{i}"));
                        if i == 4 {
                            assert!(*invalid, "fifth call must be invalid");
                        } else {
                            assert!(!*invalid, "call {i} must stay valid");
                        }
                    }
                    other => panic!("block {i}: unexpected {other:?}"),
                }
            }
        }
    }
}

#[test]
fn duplicate_tool_use_ids_are_rejected() {
    let events = frame(
        vec![
            tool_start(0, "toolu_dup", "read_file"),
            json_delta(0, "{}"),
            block_stop(0),
            tool_start(1, "toolu_dup", "read_file"),
            json_delta(1, "{}"),
            block_stop(1),
        ],
        "tool_use",
    );
    assert_eq!(run(&events), Err(TurnError::DuplicateToolId));
}

#[test]
fn tool_use_with_end_turn_is_inconsistent() {
    let events = frame(
        vec![
            tool_start(0, "toolu_i", "read_file"),
            json_delta(0, "{}"),
            block_stop(0),
        ],
        "end_turn",
    );
    assert_eq!(run(&events), Err(TurnError::Inconsistent));
}

#[test]
fn tool_use_stop_reason_without_tool_blocks_is_inconsistent() {
    let events = frame(
        vec![text_start(0), text_delta(0, "hi"), block_stop(0)],
        "tool_use",
    );
    assert_eq!(run(&events), Err(TurnError::Inconsistent));
}

// ---------------------------------------------------------------------------
// SseAccumulator: stop reasons that must never execute a call
// ---------------------------------------------------------------------------

#[test]
fn max_tokens_with_partial_tool_use_discards_the_call() {
    let events = frame(
        vec![
            text_start(0),
            text_delta(0, "Let me look"),
            block_stop(0),
            tool_start(1, "toolu_m", "read_file"),
            json_delta(1, "{\"path\":\"/et"),
            block_stop(1),
        ],
        "max_tokens",
    );
    let result = run(&events);
    assert_no_valid_tool_use(&result);
    if let Ok(turn) = &result {
        assert_eq!(turn.stop_reason, Some(StopReason::MaxTokens));
    }
}

#[test]
fn refusal_stop_produces_no_executable_tool_call() {
    let events = frame(
        vec![
            text_start(0),
            text_delta(0, "I cannot"),
            block_stop(0),
            tool_start(1, "toolu_r", "read_file"),
            json_delta(1, "{}"),
            block_stop(1),
        ],
        "refusal",
    );
    let result = run(&events);
    assert_no_valid_tool_use(&result);
    if let Ok(turn) = &result {
        assert_eq!(turn.stop_reason, Some(StopReason::Refusal));
    }
}

// ---------------------------------------------------------------------------
// build_request_body
// ---------------------------------------------------------------------------

fn git_status_spec() -> ToolSpec {
    ToolSpec {
        name: "git_status".to_string(),
        description: "Summarise git status for a project".to_string(),
        input_schema: json!({"type": "object",
            "properties": {"project": {"type": "string"}},
            "required": ["project"]}),
        risk: ToolRisk::ReadOnly,
    }
}

#[test]
fn request_body_includes_model_limits_stream_system_messages_and_tools() {
    let tools = vec![git_status_spec()];
    let messages = vec![json!({"role": "user", "content": "status of vox?"})];
    let body = build_request_body("claude-test", "SYS", &messages, &tools, 512);
    assert_eq!(body["model"], json!("claude-test"));
    assert_eq!(body["max_tokens"], json!(512));
    assert_eq!(body["stream"], json!(true));
    assert_eq!(body["system"], json!("SYS"));
    assert_eq!(body["messages"], json!(messages));
    // Exactly name/description/input_schema: the internal `risk` field is never sent.
    assert_eq!(
        body["tools"],
        json!([{
            "name": "git_status",
            "description": "Summarise git status for a project",
            "input_schema": {"type": "object",
                "properties": {"project": {"type": "string"}},
                "required": ["project"]}
        }])
    );
    assert!(
        body.get("temperature").is_none(),
        "temperature must never be sent"
    );
}

#[test]
fn request_body_omits_tools_key_when_no_tools() {
    let messages = vec![json!({"role": "user", "content": "hi"})];
    let body = build_request_body("claude-test", "SYS", &messages, &[], 64);
    assert!(
        body.get("tools").is_none(),
        "no tools key for an empty tool list"
    );
    assert_eq!(body["messages"], json!(messages));
}

#[test]
fn request_body_carries_assistant_echo_and_tool_result_round_trip() {
    let turn = ModelTurn {
        blocks: vec![
            Block::Text("Checking.".to_string()),
            tool("toolu_a", "git_status", json!({"project": "vox"}), false),
        ],
        stop_reason: Some(StopReason::ToolUse),
        complete: true,
    };
    let results = vec![ToolResultBlock {
        tool_use_id: "toolu_a".to_string(),
        text: "clean".to_string(),
        is_error: false,
    }];
    let messages = vec![
        json!({"role": "user", "content": "status?"}),
        assistant_message(&turn),
        tool_results_message(&results),
    ];
    let body = build_request_body("claude-test", "SYS", &messages, &[git_status_spec()], 512);
    assert_eq!(
        body["messages"],
        json!([
            {"role": "user", "content": "status?"},
            {"role": "assistant", "content": [
                {"type": "text", "text": "Checking."},
                {"type": "tool_use", "id": "toolu_a", "name": "git_status",
                 "input": {"project": "vox"}}
            ]},
            {"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "toolu_a", "content": "clean", "is_error": false}
            ]}
        ])
    );
}

// ---------------------------------------------------------------------------
// assistant_message
// ---------------------------------------------------------------------------

#[test]
fn assistant_message_echoes_text_only_turn() {
    let turn = ModelTurn {
        blocks: vec![Block::Text("Hi".to_string())],
        stop_reason: Some(StopReason::EndTurn),
        complete: true,
    };
    assert_eq!(
        assistant_message(&turn),
        json!({"role": "assistant", "content": [{"type": "text", "text": "Hi"}]})
    );
}

#[test]
fn assistant_message_echoes_blocks_verbatim_in_order() {
    let turn = ModelTurn {
        blocks: vec![
            Block::Text("Let me check.".to_string()),
            tool("toolu_a", "git_status", json!({"project": "vox"}), false),
            tool("toolu_b", "read_file", json!({"path": "x"}), false),
        ],
        stop_reason: Some(StopReason::ToolUse),
        complete: true,
    };
    assert_eq!(
        assistant_message(&turn),
        json!({"role": "assistant", "content": [
            {"type": "text", "text": "Let me check."},
            {"type": "tool_use", "id": "toolu_a", "name": "git_status", "input": {"project": "vox"}},
            {"type": "tool_use", "id": "toolu_b", "name": "read_file", "input": {"path": "x"}}
        ]})
    );
}

#[test]
fn assistant_message_echoes_invalid_tool_input_as_empty_object() {
    let turn = ModelTurn {
        blocks: vec![tool("toolu_bad", "read_file", json!("garbage"), true)],
        stop_reason: Some(StopReason::ToolUse),
        complete: true,
    };
    assert_eq!(
        assistant_message(&turn),
        json!({"role": "assistant", "content": [
            {"type": "tool_use", "id": "toolu_bad", "name": "read_file", "input": {}}
        ]})
    );
}

// ---------------------------------------------------------------------------
// tool_results_message
// ---------------------------------------------------------------------------

#[test]
fn tool_results_message_is_one_user_message_in_call_order_with_is_error() {
    let results = vec![
        ToolResultBlock {
            tool_use_id: "toolu_a".to_string(),
            text: "clean".to_string(),
            is_error: false,
        },
        ToolResultBlock {
            tool_use_id: "toolu_b".to_string(),
            text: "unknown tool".to_string(),
            is_error: true,
        },
    ];
    assert_eq!(
        tool_results_message(&results),
        json!({"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_a", "content": "clean", "is_error": false},
            {"type": "tool_result", "tool_use_id": "toolu_b", "content": "unknown tool", "is_error": true}
        ]})
    );
}

#[test]
fn tool_result_text_with_control_chars_and_long_lines_is_passed_verbatim() {
    let text = format!("line1\n\u{1b}[0m\tend\u{0}{}", "x".repeat(5_000));
    let results = vec![ToolResultBlock {
        tool_use_id: "toolu_c".to_string(),
        text: text.clone(),
        is_error: false,
    }];
    let message = tool_results_message(&results);
    assert_eq!(message["content"][0]["content"], json!(text));
}

#[test]
fn tool_results_message_has_no_text_block_before_results() {
    let results = vec![ToolResultBlock {
        tool_use_id: "toolu_a".to_string(),
        text: "ok".to_string(),
        is_error: false,
    }];
    let message = tool_results_message(&results);
    let content = message["content"].as_array();
    assert!(content.is_some_and(|c| c.iter().all(|b| b["type"] == json!("tool_result"))));
}

// ---------------------------------------------------------------------------
// Compile-time sanity: the error type carries the Backend variant used by stream_blocks.
// ---------------------------------------------------------------------------

#[test]
fn turn_error_backend_variant_wraps_compose_error() {
    let err = TurnError::Backend(ComposeError::Timeout);
    assert_eq!(err, TurnError::Backend(ComposeError::Timeout));
}
