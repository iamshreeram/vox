//! Tool-support probe matrix (DESIGN Rev 5, 3.6 "Probe (frozen)", section 4 T2) and the bindings
//! check for `vault_rag_probe` (T2 bindings contract). Step S7: written first, RED until S8.
//! Loopback mock only; no real provider. Helpers come from `converse_rag_tests`.

use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use specta_typescript::{BigIntExportBehavior, Typescript};

use super::converse::BoxFuture;
use super::converse_platform::{route_key_of, run_probe, RouteKey, ToolSupport};
use super::converse_rag_tests::{
    loopback_route, request_json, request_text, serve_sequence, sse_response, text_reply,
    tool_reply_named,
};
use super::vault_rag::BlockingGate;
use super::vault_rag_runtime::{IndexBackend, VaultRagRuntime};
use crate::commands::vault_rag::probe_inner;
use crate::managers::llm_tools::{ToolRisk, ToolSpec};
use crate::managers::tool_loop::{ToolExecutor, ToolOutput};

/// Probe deadline used by the matrix (the production turn budget is 15 s).
const DEADLINE: Duration = Duration::from_secs(15);
/// Fixed probe system prompt (DESIGN 3.6).
const PROBE_SYSTEM: &str = "You are a connectivity check.";
/// Fixed probe user prompt (DESIGN 3.6).
const PROBE_PROMPT: &str =
    "Call the vox_probe_echo tool with the word \"ping\", then reply with the single word it returns.";
/// Refusal text when tool support cannot be tested (DESIGN 3.6).
const NO_TOOLS_ERR: &str =
    "tool support can only be tested when agent tools are enabled with a direct model route";

/// Probe echo executor: counts calls and records every word it was given.
struct Echo {
    calls: Arc<AtomicUsize>,
    words: Arc<Mutex<Vec<String>>>,
}

impl ToolExecutor for Echo {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "vox_probe_echo".to_string(),
            description: "Echoes the probe word.".to_string(),
            input_schema: json!({"type": "object",
                "properties": {"word": {"type": "string"}}, "required": ["word"]}),
            risk: ToolRisk::ReadOnly,
        }]
    }

    fn call<'a>(&'a self, _name: &'a str, input: &'a Value) -> BoxFuture<'a, ToolOutput> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let word = input["word"].as_str().unwrap_or("").to_string();
            self.words
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(word);
            ToolOutput {
                text: "ping".to_string(),
                is_error: false,
                untrusted: false,
            }
        })
    }
}

/// The probe echo executor with its call and "ping" counters, and the recorded words.
type ProbeEchoParts = (
    Arc<dyn ToolExecutor>,
    Arc<AtomicUsize>,
    Arc<Mutex<Vec<String>>>,
);

/// A probe echo executor plus its counters.
fn echo() -> ProbeEchoParts {
    let calls = Arc::new(AtomicUsize::new(0));
    let words = Arc::new(Mutex::new(Vec::new()));
    let exec: Arc<dyn ToolExecutor> = Arc::new(Echo {
        calls: calls.clone(),
        words: words.clone(),
    });
    (exec, calls, words)
}

/// A bare HTTP error response with an empty body.
fn status_reply(code: u16, reason: &str) -> Vec<u8> {
    format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

/// SSE tool_use whose streamed input is cut off (malformed JSON).
fn malformed_tool_reply() -> Vec<u8> {
    let events = vec![
        json!({"type": "message_start", "message": {"id": "m", "type": "message",
            "role": "assistant", "content": [], "model": "m", "stop_reason": null}}),
        json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "tool_use", "id": "toolu_bad", "name": "vox_probe_echo",
                "input": {}}}),
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "input_json_delta", "partial_json": "{\"word\": "}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "tool_use",
            "stop_sequence": null}}),
        json!({"type": "message_stop"}),
    ];
    sse_response(&events)
}

/// A loopback address whose listener accepts and never answers (for the timeout case).
fn serve_hang() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("loopback addr");
    std::thread::spawn(move || {
        if let Ok((_st, _)) = listener.accept() {
            std::thread::sleep(Duration::from_secs(5));
        }
    });
    format!("http://{addr}")
}

/// The probe echo answered with word "ping": Supported, the echo ran exactly once with "ping".
#[tokio::test]
async fn probe_valid_echo_is_supported_and_runs_once_with_ping() {
    let (base, _seen) = serve_sequence(vec![
        tool_reply_named("vox_probe_echo", "toolu_p", &json!({"word": "ping"})),
        text_reply("ping"),
    ]);
    let (exec, calls, words) = echo();

    let result = run_probe(Some(loopback_route(&base)), DEADLINE, exec).await;

    assert_eq!(result, Ok(ToolSupport::Supported));
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the echo ran exactly once");
    assert_eq!(
        words.lock().unwrap_or_else(|p| p.into_inner()).clone(),
        vec!["ping".to_string()]
    );
}

/// A text-only success with no tool call: Unsupported, the echo never ran.
#[tokio::test]
async fn probe_text_only_success_is_unsupported() {
    let (base, _seen) = serve_sequence(vec![text_reply("ping")]);
    let (exec, calls, _words) = echo();

    let result = run_probe(Some(loopback_route(&base)), DEADLINE, exec).await;

    assert_eq!(result, Ok(ToolSupport::Unsupported));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "the echo never ran");
}

/// A malformed tool call is Unknown, not Unsupported.
#[tokio::test]
async fn probe_malformed_tool_call_is_unknown() {
    let (base, _seen) = serve_sequence(vec![malformed_tool_reply()]);
    let (exec, _calls, _words) = echo();

    let result = run_probe(Some(loopback_route(&base)), DEADLINE, exec).await;

    assert_eq!(result, Ok(ToolSupport::Unknown));
}

/// HTTP 400, 401 and 429 are each Unknown.
#[tokio::test]
async fn probe_http_400_401_429_are_unknown() {
    for (code, reason) in [
        (400, "Bad Request"),
        (401, "Unauthorized"),
        (429, "Too Many Requests"),
    ] {
        let (base, _seen) = serve_sequence(vec![status_reply(code, reason)]);
        let (exec, _calls, _words) = echo();

        let result = run_probe(Some(loopback_route(&base)), DEADLINE, exec).await;

        assert_eq!(result, Ok(ToolSupport::Unknown), "HTTP {code} is Unknown");
    }
}

/// A refused connection is Unknown.
#[tokio::test]
async fn probe_connection_refused_is_unknown() {
    let (exec, _calls, _words) = echo();

    let result = run_probe(Some(loopback_route("http://127.0.0.1:1")), DEADLINE, exec).await;

    assert_eq!(result, Ok(ToolSupport::Unknown));
}

/// A probe that never answers is Unknown once the injected deadline passes.
#[tokio::test]
async fn probe_timeout_is_unknown() {
    let base = serve_hang();
    let (exec, _calls, _words) = echo();

    let result = run_probe(
        Some(loopback_route(&base)),
        Duration::from_millis(300),
        exec,
    )
    .await;

    assert_eq!(result, Ok(ToolSupport::Unknown));
}

/// No tool route: the exact Err text, and nothing is sent.
#[tokio::test]
async fn probe_without_tool_route_is_an_error_and_sends_nothing() {
    let (exec, calls, _words) = echo();

    let result = run_probe(None, DEADLINE, exec).await;

    assert_eq!(result, Err(NO_TOOLS_ERR.to_string()));
    assert_eq!(calls.load(Ordering::SeqCst), 0, "no echo ran");
}

/// The probe request: exact system and user prompts, one ReadOnly echo tool, no vault text.
#[tokio::test]
async fn probe_request_has_exact_wording_one_tool_and_no_vault_text() {
    let (base, seen) = serve_sequence(vec![
        tool_reply_named("vox_probe_echo", "toolu_p", &json!({"word": "ping"})),
        text_reply("ping"),
    ]);
    let (exec, _calls, _words) = echo();

    run_probe(Some(loopback_route(&base)), DEADLINE, exec)
        .await
        .expect("probe runs");

    let reqs = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
    let first = request_json(&reqs[0]);
    assert_eq!(
        first["system"].as_str(),
        Some(PROBE_SYSTEM),
        "exact system prompt"
    );
    assert_eq!(
        first["messages"][0]["content"].as_str(),
        Some(PROBE_PROMPT),
        "exact user prompt"
    );
    let tools = first["tools"].as_array().expect("one tool array");
    assert_eq!(tools.len(), 1, "exactly one tool");
    assert_eq!(tools[0]["name"].as_str(), Some("vox_probe_echo"));
    assert_eq!(
        tools[0]["input_schema"]["properties"]["word"]["type"].as_str(),
        Some("string")
    );
    assert!(
        !request_text(&reqs[0]).contains("NOTES FROM THE USER'S VAULT"),
        "no vault text in the probe request"
    );
}

/// The route key is in memory only: its Debug output shows no endpoint and no model name.
#[test]
fn route_key_debug_prints_no_field_values() {
    let key = RouteKey::new(
        "anthropic",
        "https://probe-endpoint.invalid",
        "probe-model-zz",
    );
    let shown = format!("{key:?}");
    assert!(
        !shown.contains("probe-endpoint"),
        "Debug shows no endpoint: {shown}"
    );
    assert!(
        !shown.contains("probe-model-zz"),
        "Debug shows no model: {shown}"
    );
}

/// The key of a route is stable for the same route and changes with the endpoint.
#[test]
fn route_key_of_follows_endpoint_identity() {
    let a = route_key_of(&loopback_route("http://127.0.0.1:9"));
    let b = route_key_of(&loopback_route("http://127.0.0.1:9"));
    let c = route_key_of(&loopback_route("http://127.0.0.1:10"));
    assert_eq!(a, b, "same route, same key");
    assert_ne!(a, c, "another endpoint, another key");
}

/// The command logic records the result against the route it probed and returns its label.
#[tokio::test]
async fn probe_inner_records_supported_for_its_route_and_returns_the_label() {
    let (base, _seen) = serve_sequence(vec![
        tool_reply_named("vox_probe_echo", "toolu_p", &json!({"word": "ping"})),
        text_reply("ping"),
    ]);
    let route = loopback_route(&base);
    let (exec, _calls, _words) = echo();
    let tmp = tempfile::tempdir().expect("tempdir");
    let index = Arc::new(crate::managers::vault_index::IndexManager::new(
        tmp.path().to_path_buf(),
        Arc::new(crate::managers::vault_io::VaultIoWorker::start()),
    ));
    let backend: Arc<dyn IndexBackend> = index;
    let runtime = VaultRagRuntime::new(backend, BlockingGate::new(2));

    let label = probe_inner(Some(&runtime), Some(route.clone()), DEADLINE, exec).await;

    assert_eq!(label, Ok("supported".to_string()));
    assert_eq!(
        runtime.probe_state_for(&route_key_of(&route)),
        ToolSupport::Supported
    );
}

/// Without a tool route the command refuses with the exact text and records nothing.
#[tokio::test]
async fn probe_inner_without_tool_route_is_refused() {
    let (exec, calls, _words) = echo();

    let label = probe_inner(None, None, DEADLINE, exec).await;

    assert_eq!(label, Err(NO_TOOLS_ERR.to_string()));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}

/// Bindings contract for the new command (stays red until S8 registers it and regenerates).
#[test]
fn bindings_contain_vault_rag_probe_command() {
    let path =
        std::env::temp_dir().join(format!("vox-s7-probe-bindings-{}.ts", std::process::id()));
    crate::specta_builder()
        .export(
            Typescript::default().bigint(BigIntExportBehavior::Number),
            path.to_str().expect("utf-8 temp path"),
        )
        .expect("bindings export");
    let ts = std::fs::read_to_string(&path).expect("read exported bindings");
    let _ = std::fs::remove_file(&path);
    assert!(
        ts.contains("TAURI_INVOKE(\"vault_rag_probe\""),
        "bindings lack the vault_rag_probe command"
    );
}
