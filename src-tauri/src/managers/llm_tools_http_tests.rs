//! Authoritative HTTP tests for `llm_tools::stream_blocks` (DESIGN Rev 4: D2, R2-5,
//! R2-6, R2-8). Written BEFORE the implementation; do not edit to make code pass.
//!
//! A tokio TcpListener on 127.0.0.1 plays the Anthropic Messages endpoint. The
//! `llm_stream_tests.rs` mock is private to that module, so this file carries its own
//! small loopback server instead of editing that file.

use super::*;
use crate::managers::llm_stream::{Protocol, ProxyMode, StreamCall, StreamOpts};
use crate::managers::speech_compose::ComposeError;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Built at runtime so no token-shaped literal appears in source.
const TEST_KEY: &str = concat!("test-", "key-", "value");
const ERROR_BODY_MARKER: &str = "UPSTREAM-BODY-MARKER-DO-NOT-LEAK";

// ---------------------------------------------------------------------------
// Loopback Anthropic-like server
// ---------------------------------------------------------------------------

#[derive(Clone)]
enum Step {
    Write(Vec<u8>),
    Pause(Duration),
    /// Ends the response by closing the connection.
    Close,
    /// Keeps the connection open until the client closes it, then sets the flag.
    HoldUntilClosed(Arc<AtomicBool>),
}

struct Mock {
    base: String,
    /// Raw request bytes (head and body), one entry per received request.
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
    connections: Arc<AtomicUsize>,
}

impl Mock {
    fn requests(&self) -> Vec<Vec<u8>> {
        self.requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

async fn start(steps: Vec<Step>) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let addr = listener.local_addr().expect("loopback local addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let connections = Arc::new(AtomicUsize::new(0));
    let steps = Arc::new(steps);
    {
        let (requests, connections, steps) = (requests.clone(), connections.clone(), steps.clone());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                connections.fetch_add(1, Ordering::SeqCst);
                let (requests, steps) = (requests.clone(), steps.clone());
                tokio::spawn(async move {
                    if let Some(raw) = read_request(&mut socket).await {
                        requests.lock().unwrap_or_else(|p| p.into_inner()).push(raw);
                        run_steps(&mut socket, &steps).await;
                    }
                });
            }
        });
    }
    Mock {
        base: format!("http://{addr}"),
        requests,
        connections,
    }
}

/// Reads one request (head plus Content-Length body) and returns its raw bytes.
async fn read_request(socket: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
        let mut tmp = [0u8; 1024];
        let n = socket.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let len = head
        .split("\r\n")
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let mut tmp = [0u8; 4096];
        let n = socket.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    Some(buf)
}

async fn run_steps(socket: &mut TcpStream, steps: &[Step]) {
    for step in steps {
        match step {
            Step::Write(bytes) => {
                if socket.write_all(bytes).await.is_err() {
                    return;
                }
                let _ = socket.flush().await;
            }
            Step::Pause(d) => tokio::time::sleep(*d).await,
            Step::Close => {
                let _ = socket.shutdown().await;
                return;
            }
            Step::HoldUntilClosed(flag) => {
                let mut b = [0u8; 16];
                loop {
                    match socket.read(&mut b).await {
                        Ok(0) | Err(_) => {
                            flag.store(true, Ordering::SeqCst);
                            return;
                        }
                        Ok(_) => {}
                    }
                }
            }
        }
    }
}

fn sse_head() -> Vec<u8> {
    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n".to_vec()
}

fn sse_bytes(events: &[Value]) -> Vec<u8> {
    let mut out = String::new();
    for ev in events {
        let name = ev["type"].as_str().unwrap_or("message");
        out.push_str(&format!("event: {name}\ndata: {ev}\n\n"));
    }
    out.into_bytes()
}

fn status_response(code: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {code} Error\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

// ---------------------------------------------------------------------------
// Event and request helpers
// ---------------------------------------------------------------------------

fn msg_start() -> Value {
    json!({"type": "message_start", "message": {"id": "msg_1", "type": "message",
        "role": "assistant", "content": [], "model": "m", "stop_reason": null}})
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
    json!({"type": "message_delta", "delta": {"stop_reason": stop, "stop_sequence": null}})
}
fn msg_stop() -> Value {
    json!({"type": "message_stop"})
}

fn anth_call<'a>(base: &'a str, key: &'a str) -> StreamCall<'a> {
    StreamCall {
        protocol: Protocol::AnthropicMessages,
        base_url: base,
        api_key: key,
        model: "claude-test",
        system: "SYS",
        user: "USER",
        max_tokens: 256,
        json_mode: false,
        completion_tokens_field: false,
    }
}

fn opts(overall_ms: u64, first_ms: u64) -> StreamOpts {
    StreamOpts {
        overall: Duration::from_millis(overall_ms),
        first_text: Duration::from_millis(first_ms),
        proxy: ProxyMode::SystemDefault,
    }
}

fn body_with_user(user: &str) -> Value {
    let messages = vec![json!({"role": "user", "content": user})];
    build_request_body("claude-test", "SYS", &messages, &[], 256)
}

fn header_value(raw: &[u8], name: &str) -> Option<String> {
    let head_end = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    head.split("\r\n")
        .skip(1)
        .filter_map(|l| l.split_once(':'))
        .find(|(k, _)| k.trim().eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim().to_string())
}

fn request_line(raw: &[u8]) -> String {
    let end = raw
        .windows(2)
        .position(|w| w == b"\r\n")
        .unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).to_string()
}

fn request_body(raw: &[u8]) -> Value {
    let end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("request head terminator")
        + 4;
    serde_json::from_slice(&raw[end..]).expect("request body is JSON")
}

// ---------------------------------------------------------------------------
// Successful streams
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_text_turn_streams_text_and_completes() {
    let events = vec![
        msg_start(),
        text_start(0),
        text_delta(0, "Hel"),
        text_delta(0, "lo"),
        block_stop(0),
        msg_delta("end_turn"),
        msg_stop(),
    ];
    let mut bytes = sse_head();
    bytes.extend(sse_bytes(&events));
    let mock = start(vec![Step::Write(bytes), Step::Close]).await;

    let turn = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("hi"),
        &opts(5_000, 2_000),
    )
    .await
    .expect("text turn streams");

    assert_eq!(turn.blocks, vec![Block::Text("Hello".to_string())]);
    assert_eq!(turn.stop_reason, Some(StopReason::EndTurn));
    assert!(turn.complete);
    assert_eq!(mock.connections(), 1);
}

#[tokio::test]
async fn http_tool_use_turn_accumulates_input_and_stops_with_tool_use() {
    let events = vec![
        msg_start(),
        text_start(0),
        text_delta(0, "Checking."),
        block_stop(0),
        tool_start(1, "toolu_http_1", "git_status"),
        json_delta(1, "{\"project\":"),
        json_delta(1, "\"vox\"}"),
        block_stop(1),
        msg_delta("tool_use"),
        msg_stop(),
    ];
    let mut bytes = sse_head();
    bytes.extend(sse_bytes(&events));
    let mock = start(vec![Step::Write(bytes), Step::Close]).await;

    let turn = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("status?"),
        &opts(5_000, 2_000),
    )
    .await
    .expect("tool turn streams");

    assert_eq!(
        turn.blocks,
        vec![
            Block::Text("Checking.".to_string()),
            Block::ToolUse {
                id: "toolu_http_1".to_string(),
                name: "git_status".to_string(),
                input: json!({"project": "vox"}),
                invalid: false,
            },
        ]
    );
    assert_eq!(turn.stop_reason, Some(StopReason::ToolUse));
    assert!(turn.complete);
}

#[tokio::test]
async fn http_tool_only_response_is_accepted() {
    let events = vec![
        msg_start(),
        tool_start(0, "toolu_only", "list_projects"),
        json_delta(0, "{}"),
        block_stop(0),
        msg_delta("tool_use"),
        msg_stop(),
    ];
    let mut bytes = sse_head();
    bytes.extend(sse_bytes(&events));
    let mock = start(vec![Step::Write(bytes), Step::Close]).await;

    let turn = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("list"),
        &opts(5_000, 2_000),
    )
    .await
    .expect("tool-only response is legitimate");

    assert_eq!(
        turn.blocks,
        vec![Block::ToolUse {
            id: "toolu_only".to_string(),
            name: "list_projects".to_string(),
            input: json!({}),
            invalid: false,
        }]
    );
    assert!(turn.complete);
}

#[tokio::test]
async fn http_delayed_tool_only_response_is_accepted_within_first_event_deadline() {
    // R2-6: a valid content_block_start (tool_use) counts as progress, so a slow start
    // that still beats the first-event deadline must be accepted.
    let mut head = sse_head();
    head.extend(sse_bytes(&[msg_start()]));
    let mut tool = sse_bytes(&[
        tool_start(0, "toolu_slow", "list_projects"),
        json_delta(0, "{}"),
        block_stop(0),
        msg_delta("tool_use"),
        msg_stop(),
    ]);
    let mut steps = vec![Step::Write(head), Step::Pause(Duration::from_millis(300))];
    steps.push(Step::Write(std::mem::take(&mut tool)));
    steps.push(Step::Close);
    let mock = start(steps).await;

    let turn = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("list"),
        &opts(5_000, 1_500),
    )
    .await
    .expect("delayed tool-only response within deadline");

    assert_eq!(turn.stop_reason, Some(StopReason::ToolUse));
    assert!(turn.complete);
}

#[tokio::test]
async fn http_sse_split_into_tcp_chunks_mid_event_and_mid_utf8_is_reassembled() {
    let events = vec![
        msg_start(),
        text_start(0),
        text_delta(0, "caf\u{e9} \u{2603} done"),
        block_stop(0),
        msg_delta("end_turn"),
        msg_stop(),
    ];
    let mut full = sse_head();
    full.extend(sse_bytes(&events));

    // Cut inside the two-byte UTF-8 sequence for e-acute, and also every few bytes so
    // event lines and the data: prefix are split across TCP writes.
    let eacute = "\u{e9}".as_bytes();
    let utf8_cut = full
        .windows(eacute.len())
        .position(|w| w == eacute)
        .expect("e-acute present in stream")
        + 1;
    let mut steps = Vec::new();
    let mut offset = 0usize;
    let mut cuts = vec![utf8_cut];
    let mut at = 7;
    while at < full.len() {
        cuts.push(at);
        at += 9;
    }
    cuts.sort_unstable();
    cuts.dedup();
    for cut in cuts {
        if cut > offset {
            steps.push(Step::Write(full[offset..cut].to_vec()));
            steps.push(Step::Pause(Duration::from_millis(2)));
            offset = cut;
        }
    }
    steps.push(Step::Write(full[offset..].to_vec()));
    steps.push(Step::Close);
    let mock = start(steps).await;

    let turn = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("unicode"),
        &opts(5_000, 2_000),
    )
    .await
    .expect("split SSE reassembles");

    assert_eq!(
        turn.blocks,
        vec![Block::Text("caf\u{e9} \u{2603} done".to_string())]
    );
    assert!(turn.complete);
}

#[tokio::test]
async fn http_tool_input_split_across_tcp_writes_is_reassembled() {
    let events = vec![
        msg_start(),
        tool_start(0, "toolu_split", "read_file"),
        json_delta(0, "{\"pa"),
        json_delta(0, "th\":\"src/"),
        json_delta(0, "lib.rs\"}"),
        block_stop(0),
        msg_delta("tool_use"),
        msg_stop(),
    ];
    let full = {
        let mut f = sse_head();
        f.extend(sse_bytes(&events));
        f
    };
    let mid = full.len() / 2;
    let steps = vec![
        Step::Write(full[..mid].to_vec()),
        Step::Pause(Duration::from_millis(2)),
        Step::Write(full[mid..].to_vec()),
        Step::Close,
    ];
    let mock = start(steps).await;

    let turn = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("read"),
        &opts(5_000, 2_000),
    )
    .await
    .expect("split tool input reassembles");

    assert_eq!(
        turn.blocks,
        vec![Block::ToolUse {
            id: "toolu_split".to_string(),
            name: "read_file".to_string(),
            input: json!({"path": "src/lib.rs"}),
            invalid: false,
        }]
    );
}

// ---------------------------------------------------------------------------
// Request headers and body
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_request_carries_api_key_anthropic_version_and_the_passed_body() {
    let mut bytes = sse_head();
    bytes.extend(sse_bytes(&[
        msg_start(),
        text_start(0),
        text_delta(0, "ok"),
        block_stop(0),
        msg_delta("end_turn"),
        msg_stop(),
    ]));
    let mock = start(vec![Step::Write(bytes), Step::Close]).await;
    let body = body_with_user("headers please");

    stream_blocks(&anth_call(&mock.base, TEST_KEY), &body, &opts(5_000, 2_000))
        .await
        .expect("turn streams");

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1, "exactly one request");
    let raw = &reqs[0];
    assert!(
        request_line(raw).starts_with("POST /v1/messages "),
        "request line: {}",
        request_line(raw)
    );
    assert_eq!(header_value(raw, "x-api-key").as_deref(), Some(TEST_KEY));
    assert_eq!(
        header_value(raw, "anthropic-version").as_deref(),
        Some("2023-06-01")
    );
    assert!(header_value(raw, "content-type").is_some_and(|v| v.contains("application/json")));
    assert!(
        header_value(raw, "authorization").is_none(),
        "Anthropic path must not send a bearer header"
    );
    assert_eq!(request_body(raw), body, "the passed body is what is sent");
}

#[tokio::test]
async fn http_request_omits_x_api_key_when_no_key_is_configured() {
    let mut bytes = sse_head();
    bytes.extend(sse_bytes(&[
        msg_start(),
        text_start(0),
        text_delta(0, "ok"),
        block_stop(0),
        msg_delta("end_turn"),
        msg_stop(),
    ]));
    let mock = start(vec![Step::Write(bytes), Step::Close]).await;

    stream_blocks(
        &anth_call(&mock.base, ""),
        &body_with_user("no key"),
        &opts(5_000, 2_000),
    )
    .await
    .expect("keyless loopback turn streams");

    let reqs = mock.requests();
    assert_eq!(reqs.len(), 1);
    assert!(header_value(&reqs[0], "x-api-key").is_none());
}

#[tokio::test]
async fn http_request_body_of_exactly_64_kib_is_sent() {
    // R2-6: retained body <= 64 KiB. The boundary value itself must be accepted.
    let base_len = serde_json::to_vec(&body_with_user(""))
        .expect("serialize")
        .len();
    let pad = 65_536 - base_len;
    let body = body_with_user(&"a".repeat(pad));
    assert_eq!(serde_json::to_vec(&body).expect("serialize").len(), 65_536);

    let mut bytes = sse_head();
    bytes.extend(sse_bytes(&[
        msg_start(),
        text_start(0),
        text_delta(0, "ok"),
        block_stop(0),
        msg_delta("end_turn"),
        msg_stop(),
    ]));
    let mock = start(vec![Step::Write(bytes), Step::Close]).await;

    stream_blocks(&anth_call(&mock.base, TEST_KEY), &body, &opts(5_000, 2_000))
        .await
        .expect("64 KiB body is within the cap");
    assert_eq!(mock.requests().len(), 1);
}

#[tokio::test]
async fn http_request_body_over_64_kib_is_refused_before_any_connection() {
    let base_len = serde_json::to_vec(&body_with_user(""))
        .expect("serialize")
        .len();
    let pad = 65_537 - base_len;
    let body = body_with_user(&"a".repeat(pad));
    assert_eq!(serde_json::to_vec(&body).expect("serialize").len(), 65_537);

    let mock = start(vec![Step::Close]).await;

    let result = stream_blocks(&anth_call(&mock.base, TEST_KEY), &body, &opts(5_000, 2_000)).await;

    assert!(result.is_err(), "oversized history must be refused");
    assert_eq!(mock.connections(), 0, "no connection may be attempted");
    assert!(mock.requests().is_empty());
}

#[tokio::test]
async fn http_deadline_already_expired_is_refused_before_sending() {
    let mock = start(vec![Step::Close]).await;

    let result = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("late"),
        &opts(0, 0),
    )
    .await;

    assert!(result.is_err(), "an expired deadline must fail the turn");
    assert!(
        mock.requests().is_empty(),
        "nothing may be sent after expiry"
    );
}

// ---------------------------------------------------------------------------
// Failures
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_stall_after_headers_hits_the_first_event_deadline() {
    let steps = vec![
        Step::Write(sse_head()),
        Step::Pause(Duration::from_secs(5)),
        Step::Close,
    ];
    let mock = start(steps).await;
    let began = Instant::now();

    let result = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("stall"),
        &opts(10_000, 200),
    )
    .await;

    assert_eq!(result, Err(TurnError::Backend(ComposeError::Timeout)));
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "deadline must fire well before the server stall ends"
    );
}

#[tokio::test]
async fn http_429_error_body_is_a_backend_error_and_never_leaks() {
    let body = format!("{{\"error\":\"{ERROR_BODY_MARKER}\"}}");
    let mock = start(vec![Step::Write(status_response(429, &body)), Step::Close]).await;

    let result = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("rate"),
        &opts(5_000, 2_000),
    )
    .await;

    assert_eq!(result, Err(TurnError::Backend(ComposeError::Backend)));
    let shown = format!("{result:?}");
    assert!(
        !shown.contains(ERROR_BODY_MARKER),
        "error body must not leak"
    );
}

#[tokio::test]
async fn http_500_error_body_is_a_backend_error_and_never_leaks() {
    let body = format!("{{\"error\":\"{ERROR_BODY_MARKER}\"}}");
    let mock = start(vec![Step::Write(status_response(500, &body)), Step::Close]).await;

    let result = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("boom"),
        &opts(5_000, 2_000),
    )
    .await;

    assert_eq!(result, Err(TurnError::Backend(ComposeError::Backend)));
    assert!(!format!("{result:?}").contains(ERROR_BODY_MARKER));
}

#[tokio::test]
async fn http_truncated_stream_without_message_stop_is_a_safe_failure() {
    let events = vec![
        msg_start(),
        tool_start(0, "toolu_cut", "read_file"),
        json_delta(0, "{\"path\":\"a\"}"),
        block_stop(0),
        msg_delta("tool_use"),
    ];
    let mut bytes = sse_head();
    bytes.extend(sse_bytes(&events));
    let mock = start(vec![Step::Write(bytes), Step::Close]).await;

    let result = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("cut"),
        &opts(5_000, 2_000),
    )
    .await;

    match result {
        Err(TurnError::Truncated) | Err(TurnError::Backend(_)) => {}
        other => panic!("truncated framing must not produce a turn: {other:?}"),
    }
}

#[tokio::test]
async fn http_mid_stream_error_event_is_a_provider_error() {
    let events = vec![
        msg_start(),
        text_start(0),
        text_delta(0, "partial"),
        json!({"type": "error", "error": {"type": "overloaded_error", "message": "busy"}}),
    ];
    let mut bytes = sse_head();
    bytes.extend(sse_bytes(&events));
    let mock = start(vec![Step::Write(bytes), Step::Close]).await;

    let result = stream_blocks(
        &anth_call(&mock.base, TEST_KEY),
        &body_with_user("err"),
        &opts(5_000, 2_000),
    )
    .await;

    assert_eq!(result, Err(TurnError::ProviderError));
}

#[tokio::test]
async fn http_client_drop_mid_stream_closes_the_server_connection() {
    // R2-8 cancellation: dropping the future must close the socket the server holds.
    let closed = Arc::new(AtomicBool::new(false));
    let mut partial = sse_head();
    partial.extend_from_slice(b"event: content_block_delta\ndata: {\"type\":");
    let mock = start(vec![
        Step::Write(partial),
        Step::HoldUntilClosed(closed.clone()),
    ])
    .await;

    let outcome = tokio::time::timeout(
        Duration::from_millis(300),
        stream_blocks(
            &anth_call(&mock.base, TEST_KEY),
            &body_with_user("drop"),
            &opts(10_000, 10_000),
        ),
    )
    .await;
    assert!(outcome.is_err(), "the stream was expected to still be open");

    let began = Instant::now();
    while !closed.load(Ordering::SeqCst) && began.elapsed() < Duration::from_secs(3) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(
        closed.load(Ordering::SeqCst),
        "server must observe the dropped client as a closed connection"
    );
}
