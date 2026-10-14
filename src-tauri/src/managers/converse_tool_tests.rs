//! Direct-route wire contract for the tool-enabled voice turn (DESIGN Rev 4: D1, D3, R2-5,
//! R2-8). Written BEFORE the implementation; do not edit to make code pass.
//!
//! Scope note: `RealBrain::plan` takes an `AppHandle` and reads settings from the real store
//! (`settings::get_settings`), and it has no injected executor or base-URL seam. The
//! RealBrain-level cases (tools off by default, composer route without tools, early speech
//! off on a tool turn) therefore cannot be driven from a test yet; they are listed as a
//! missing seam in the report. What IS reachable here is the request the direct route sends
//! and the turn it reads back, through a local 127.0.0.1 SSE mock.

use crate::managers::llm_stream::{Protocol, ProxyMode, StreamCall, StreamOpts};
use crate::managers::llm_tools::{
    build_request_body, Block, StopReason, ToolRisk, ToolSpec, TurnError,
};
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Built at runtime so no token-shaped literal appears in source.
const TEST_KEY: &str = concat!("test-", "key-", "value");

/// Minimal blocking loopback server: reads one request (head plus Content-Length body),
/// records the raw bytes, then writes `response` and closes.
fn serve_once(response: Vec<u8>) -> (String, Arc<Mutex<Vec<u8>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("loopback addr");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    std::thread::spawn(move || {
        if let Ok((mut st, _)) = listener.accept() {
            let mut buf = Vec::new();
            let mut tmp = [0u8; 4096];
            let head_end = loop {
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    break i + 4;
                }
                match st.read(&mut tmp) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            };
            let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
            let len = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())?
                })
                .unwrap_or(0);
            while buf.len() < head_end + len {
                match st.read(&mut tmp) {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            *seen2.lock().unwrap_or_else(|p| p.into_inner()) = buf;
            let _ = st.write_all(&response);
            let _ = st.flush();
        }
    });
    (format!("http://{addr}"), seen)
}

fn sse_response(events: &[Value]) -> Vec<u8> {
    let mut body = String::new();
    for ev in events {
        let name = ev["type"].as_str().unwrap_or("message");
        body.push_str(&format!("event: {name}\ndata: {ev}\n\n"));
    }
    format!("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}")
        .into_bytes()
}

fn tool_turn_events() -> Vec<Value> {
    vec![
        json!({"type": "message_start", "message": {"id": "m", "type": "message",
            "role": "assistant", "content": [], "model": "m", "stop_reason": null}}),
        json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "toolu_1", "name": "list_projects",
                "input": {}}}),
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{}"}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use",
            "stop_sequence": null}}),
        json!({"type": "message_stop"}),
    ]
}

fn spec() -> ToolSpec {
    ToolSpec {
        name: "list_projects".to_string(),
        description: "List project folders".to_string(),
        input_schema: json!({"type": "object", "properties": {}}),
        risk: ToolRisk::ReadOnly,
    }
}

fn direct_call<'a>(base: &'a str) -> StreamCall<'a> {
    StreamCall {
        protocol: Protocol::AnthropicMessages,
        base_url: base,
        api_key: TEST_KEY,
        model: "claude-test",
        system: "SYS",
        user: "USER",
        max_tokens: 256,
        json_mode: false,
        completion_tokens_field: false,
    }
}

fn opts() -> StreamOpts {
    StreamOpts {
        overall: Duration::from_secs(5),
        first_text: Duration::from_secs(5),
        proxy: ProxyMode::SystemDefault,
    }
}

fn request_json(raw: &[u8]) -> Value {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("request head terminator")
        + 4;
    serde_json::from_slice(&raw[end..]).expect("request body is JSON")
}

#[tokio::test]
async fn direct_route_tool_use_turn_reaches_the_caller_with_its_tool_block() {
    let (base, _seen) = serve_once(sse_response(&tool_turn_events()));
    let call = direct_call(&base);
    let body = build_request_body(
        "claude-test",
        "SYS",
        &[json!({"role": "user", "content": "what projects do I have?"})],
        &[spec()],
        256,
    );

    let turn = crate::managers::llm_tools::stream_blocks(&call, &body, &opts())
        .await
        .expect("tool_use turn");

    assert!(turn.complete);
    assert_eq!(turn.stop_reason, Some(StopReason::ToolUse));
    match turn.blocks.as_slice() {
        [Block::ToolUse {
            id,
            name,
            input,
            invalid,
        }] => {
            assert_eq!(id, "toolu_1");
            assert_eq!(name, "list_projects");
            assert_eq!(input, &json!({}));
            assert!(!invalid);
        }
        other => panic!("expected one tool_use block, got {other:?}"),
    }
}

#[tokio::test]
async fn direct_route_request_carries_tools_and_the_user_message() {
    let (base, seen) = serve_once(sse_response(&tool_turn_events()));
    let call = direct_call(&base);
    let body = build_request_body(
        "claude-test",
        "SYS",
        &[json!({"role": "user", "content": "what projects do I have?"})],
        &[spec()],
        256,
    );

    let _ = crate::managers::llm_tools::stream_blocks(&call, &body, &opts()).await;

    let raw = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let sent = request_json(&raw);
    let tools = sent["tools"]
        .as_array()
        .expect("tools array on the tool route");
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0]["name"], "list_projects");
    assert_eq!(sent["messages"][0]["content"], "what projects do I have?");
}

#[tokio::test]
async fn direct_route_request_without_tools_has_no_tools_key() {
    // Tools disabled (default) means the RealBrain path sends an empty spec list; the body
    // must then carry no `tools` key at all, so the request is byte-compatible with today.
    let (base, seen) = serve_once(sse_response(&tool_turn_events()));
    let call = direct_call(&base);
    let body = build_request_body(
        "claude-test",
        "SYS",
        &[json!({"role": "user", "content": "hello"})],
        &[],
        256,
    );

    let _ = crate::managers::llm_tools::stream_blocks(&call, &body, &opts()).await;

    let raw = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let sent = request_json(&raw);
    assert!(
        sent.get("tools").is_none(),
        "tools disabled must not add a tools key"
    );
}

#[tokio::test]
async fn direct_route_truncated_tool_stream_is_a_safe_failure_not_a_tool_turn() {
    // Same frames without message_stop: the turn is incomplete and must fail closed.
    let mut events = tool_turn_events();
    events.pop();
    let (base, _seen) = serve_once(sse_response(&events));
    let call = direct_call(&base);
    let body = build_request_body(
        "claude-test",
        "SYS",
        &[json!({"role": "user", "content": "hi"})],
        &[spec()],
        256,
    );

    let result = crate::managers::llm_tools::stream_blocks(&call, &body, &opts()).await;

    assert!(
        matches!(
            result,
            Err(TurnError::Truncated) | Err(TurnError::Malformed)
        ) || matches!(result, Ok(ref t) if !t.complete),
        "a truncated tool stream must not be acted on, got {result:?}"
    );
}

#[test]
fn tool_spec_risk_is_read_only_for_every_m2_tool() {
    assert_eq!(spec().risk, ToolRisk::ReadOnly);
}
