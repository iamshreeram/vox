//! Transport-seam tests for `complete_plain` (DESIGN A4, answer judge route).
//! Each test runs a local fake HTTP server and inspects what was sent; no real network.

use super::*;
use crate::managers::composer_config::write_to_app;
use crate::settings::get_default_settings;
use std::future::Future;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

const KEY: &str = "PLAIN-KEY-7";
const SYS: &str = "FIXED-SYSTEM-TEXT";
const USR: &str = "Question: Should I do that?\nAnswer: \"go on\"";

fn block_on<F: Future>(f: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(f)
}

#[derive(Clone)]
enum Reply {
    /// 200 with `choices[0].message.content` = this text.
    Content(&'static str),
    /// 200 with this raw `text/event-stream` body (Anthropic streaming frames).
    Sse(String),
    /// 500 with a body that must never surface.
    Status,
    /// Never answers.
    Hang,
}

struct Srv {
    addr: SocketAddr,
    conns: Arc<AtomicUsize>,
    bodies: Arc<Mutex<Vec<String>>>,
}

impl Srv {
    fn connections(&self) -> usize {
        self.conns.load(Ordering::SeqCst)
    }

    /// Full raw requests (head and body) as received, in order.
    fn raw_requests(&self) -> Vec<String> {
        self.bodies.lock().unwrap().clone()
    }

    fn parsed_bodies(&self) -> Vec<serde_json::Value> {
        self.raw_requests()
            .iter()
            .map(|raw| {
                let body = raw.split("\r\n\r\n").nth(1).unwrap_or("");
                serde_json::from_str(body).unwrap()
            })
            .collect()
    }

    fn only_body(&self) -> serde_json::Value {
        let bodies = self.parsed_bodies();
        assert_eq!(bodies.len(), 1, "expected exactly one request");
        bodies[0].clone()
    }
}

fn http(status: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status}\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

fn answer(mut stream: TcpStream, reply: Reply, bodies: &Mutex<Vec<String>>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| {
            l.strip_prefix("content-length:")
                .and_then(|v| v.trim().parse::<usize>().ok())
        })
        .unwrap_or(0);
    while buf.len() < header_end + len {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    bodies
        .lock()
        .unwrap()
        .push(String::from_utf8_lossy(&buf).to_string());
    match reply {
        Reply::Content(text) => {
            let body = serde_json::json!({"choices":[{"message":{"content":text}}]}).to_string();
            let _ = stream.write_all(&http("200 OK", body.as_bytes()));
            let _ = stream.flush();
        }
        Reply::Sse(payload) => {
            let head =
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(payload.as_bytes());
            let _ = stream.flush();
        }
        Reply::Status => {
            let _ = stream.write_all(&http("500 Internal Server Error", b"boom SECRET-BODY"));
            let _ = stream.flush();
        }
        Reply::Hang => {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(4)));
            while let Ok(n) = stream.read(&mut chunk) {
                if n == 0 {
                    break;
                }
            }
        }
    }
}

fn fake(reply: Reply) -> Srv {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let conns = Arc::new(AtomicUsize::new(0));
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let (c, b) = (conns.clone(), bodies.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let Ok(stream) = stream else { break };
            c.fetch_add(1, Ordering::SeqCst);
            let b = b.clone();
            let r = reply.clone();
            std::thread::spawn(move || answer(stream, r, &b));
        }
    });
    Srv {
        addr,
        conns,
        bodies,
    }
}

/// Composer in Model mode against the fake server. `structured` sets the provider's
/// `supports_structured_output` flag (the flag that would force JSON mode elsewhere).
fn model_settings(addr: SocketAddr, structured: bool, stream_reply: bool) -> AppSettings {
    let mut s = get_default_settings();
    s.brain_source_composer = crate::settings::BrainSource::Own;
    s.brain_source_realtime = crate::settings::BrainSource::Own;
    s.brain_source_postprocess = crate::settings::BrainSource::Own;
    s.exp_rt_enabled = true;
    s.exp_rt_stream_reply = stream_reply;
    if let Some(p) = s
        .post_process_providers
        .iter_mut()
        .find(|p| p.id == "custom")
    {
        p.base_url = format!("http://{addr}/v1");
        p.supports_structured_output = structured;
    }
    s.post_process_api_keys
        .insert("custom".to_string(), KEY.to_string());
    let c = TtsComposerSettings {
        mode: TtsComposerMode::Model,
        provider_id: "custom".to_string(),
        model: "judge-model".to_string(),
        ..Default::default()
    };
    write_to_app(&mut s, &c);
    s
}

fn plain(settings: &AppSettings, deadline: Duration) -> Result<String, PlainError> {
    block_on(complete_plain(settings, SYS, USR, deadline))
}

// ---------- supported routes: the outgoing request ----------

#[test]
fn supported_routes_send_only_the_fixed_plain_request() {
    for structured in [false, true] {
        for stream_reply in [false, true] {
            let srv = fake(Reply::Content("APPROVE"));
            let s = model_settings(srv.addr, structured, stream_reply);
            assert_eq!(
                plain(&s, Duration::from_secs(10)),
                Ok("APPROVE".to_string()),
                "structured={structured} stream={stream_reply}"
            );
            let body = srv.only_body();
            assert_eq!(body["model"], "judge-model");
            assert_eq!(body["stream"], false);
            assert!(body.get("response_format").is_none(), "no JSON mode");
            assert!(body.get("tools").is_none(), "no tools");
            assert!(body.get("functions").is_none(), "no functions");
            assert_eq!(body["temperature"], 0, "temperature must be 0");
            let max = body["max_tokens"].as_u64().unwrap_or(u64::MAX);
            assert!(max <= 8, "max_tokens must be <= 8, got {max}");
            let messages = body["messages"].as_array().cloned().unwrap_or_default();
            assert_eq!(messages.len(), 2, "exactly system and user");
            assert_eq!(messages[0]["role"], "system");
            assert_eq!(messages[0]["content"], SYS);
            assert_eq!(messages[1]["role"], "user");
            assert_eq!(messages[1]["content"], USR);
        }
    }
}

#[test]
fn bare_token_replies_are_returned_trimmed_and_verbatim() {
    let cases: [(&'static str, &str); 3] = [
        ("APPROVE", "APPROVE"),
        ("Approve.", "Approve."),
        ("  Approve.\n", "Approve."),
    ];
    for (raw, expected) in cases {
        let srv = fake(Reply::Content(raw));
        let s = model_settings(srv.addr, false, false);
        assert_eq!(
            plain(&s, Duration::from_secs(10)),
            Ok(expected.to_string()),
            "reply {raw:?}"
        );
    }
}

#[test]
fn json_wrapped_reply_is_returned_as_text_for_the_upstream_parser() {
    let srv = fake(Reply::Content(r#"{"verdict":"approve"}"#));
    let s = model_settings(srv.addr, true, false);
    assert_eq!(
        plain(&s, Duration::from_secs(10)),
        Ok(r#"{"verdict":"approve"}"#.to_string())
    );
}

#[test]
fn backend_status_error_is_backend_and_body_is_not_returned() {
    let srv = fake(Reply::Status);
    let s = model_settings(srv.addr, false, false);
    assert_eq!(plain(&s, Duration::from_secs(10)), Err(PlainError::Backend));
}

#[test]
fn no_answer_within_the_deadline_is_timeout() {
    let srv = fake(Reply::Hang);
    let s = model_settings(srv.addr, false, false);
    let started = Instant::now();
    assert_eq!(
        plain(&s, Duration::from_millis(300)),
        Err(PlainError::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

// ---------- unsupported routes: zero external calls, nothing launched ----------

#[test]
fn off_route_is_unsupported_with_zero_calls() {
    let srv = fake(Reply::Content("APPROVE"));
    let mut s = model_settings(srv.addr, false, false);
    let mut c = crate::managers::composer_config::settings_from_app(&s);
    c.mode = TtsComposerMode::Off;
    write_to_app(&mut s, &c);
    assert_eq!(
        plain(&s, Duration::from_secs(4)),
        Err(PlainError::Unsupported)
    );
    assert_eq!(srv.connections(), 0);
}

#[test]
fn unknown_provider_is_unsupported_with_zero_calls() {
    let srv = fake(Reply::Content("APPROVE"));
    let mut s = model_settings(srv.addr, false, false);
    let mut c = crate::managers::composer_config::settings_from_app(&s);
    c.provider_id = "not-a-supported-provider".to_string();
    write_to_app(&mut s, &c);
    assert_eq!(
        plain(&s, Duration::from_secs(4)),
        Err(PlainError::Unsupported)
    );
    assert_eq!(srv.connections(), 0);
}

#[test]
fn empty_model_is_unsupported_with_zero_calls() {
    let srv = fake(Reply::Content("APPROVE"));
    let mut s = model_settings(srv.addr, false, false);
    let mut c = crate::managers::composer_config::settings_from_app(&s);
    c.model = "   ".to_string();
    write_to_app(&mut s, &c);
    assert_eq!(
        plain(&s, Duration::from_secs(4)),
        Err(PlainError::Unsupported)
    );
    assert_eq!(srv.connections(), 0);
}

#[test]
fn direct_endpoint_without_url_is_unsupported_and_never_falls_back_to_composer() {
    let srv = fake(Reply::Content("APPROVE"));
    let mut s = model_settings(srv.addr, false, true);
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_endpoint_url = String::new();
    assert_eq!(
        plain(&s, Duration::from_secs(4)),
        Err(PlainError::Unsupported)
    );
    assert_eq!(srv.connections(), 0, "no composer fallback");
}

#[cfg(unix)]
fn marker_script(dir_name: &str) -> (PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("vox-plain-{}-{}", std::process::id(), dir_name));
    std::fs::create_dir_all(&dir).unwrap();
    let marker = dir.join("launched.marker");
    let _ = std::fs::remove_file(&marker);
    let script = dir.join("agent.sh");
    std::fs::write(
        &script,
        format!("#!/bin/sh\ntouch '{}'\necho APPROVE\n", marker.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    (script, marker)
}

#[cfg(unix)]
#[test]
fn command_route_is_unsupported_and_never_spawns() {
    let (script, marker) = marker_script("command");
    let mut s = get_default_settings();
    let mut c = TtsComposerSettings::default();
    c.mode = TtsComposerMode::Command;
    c.command_path = script.to_string_lossy().to_string();
    c.command_args = String::new();
    c.prompt_flag = "-p".to_string();
    c.command_ack =
        rules::normalize_command_tuple(&c.command_path, &c.command_args, &c.prompt_flag).unwrap();
    write_to_app(&mut s, &c);
    assert_eq!(
        plain(&s, Duration::from_secs(4)),
        Err(PlainError::Unsupported)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(!marker.exists(), "command route must never launch");
}

#[cfg(unix)]
#[test]
fn acp_agent_is_unsupported_never_launched_and_never_a_fallback() {
    let (script, marker) = marker_script("acp");
    let srv = fake(Reply::Content("APPROVE"));
    let mut s = model_settings(srv.addr, false, false);
    let mut c = crate::managers::composer_config::settings_from_app(&s);
    c.mode = TtsComposerMode::Off;
    write_to_app(&mut s, &c);
    s.acp_agent_enabled = true;
    s.acp_agent_command = Some(script.to_string_lossy().to_string());
    assert_eq!(
        plain(&s, Duration::from_secs(4)),
        Err(PlainError::Unsupported)
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(!marker.exists(), "agent must never launch");
    assert_eq!(srv.connections(), 0, "agent must never be a fallback call");
}

// ---------- DirectAnthropic route (A4b): exp_rt_enabled + exp_rt_direct_endpoint ----------

/// The user's real configuration: direct endpoint on, composer Off (the default).
fn direct_settings(endpoint: &str, model: &str, token: &str) -> AppSettings {
    let mut s = get_default_settings();
    s.tts_enabled = true;
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_endpoint_url = endpoint.to_string();
    s.exp_rt_model = model.to_string();
    s.exp_rt_token = token.into();
    s
}

/// Anthropic streaming frames: one `text_delta` per piece, then `message_stop`.
fn anthropic_frames(pieces: &[&str]) -> String {
    let mut out = String::new();
    for piece in pieces {
        let text = serde_json::to_string(piece).unwrap();
        out.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{text}}}}}\n\n"
        ));
    }
    out.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    out
}

#[test]
fn direct_route_sends_only_the_fixed_plain_anthropic_request() {
    let srv = fake(Reply::Sse(anthropic_frames(&["APPROVE"])));
    let s = direct_settings(&format!("http://{}", srv.addr), "judge-model", "TOK-9");
    assert_eq!(
        plain(&s, Duration::from_secs(10)),
        Ok("APPROVE".to_string())
    );
    let raw = srv.raw_requests();
    assert_eq!(raw.len(), 1, "exactly one external call");
    let head = raw[0]
        .split("\r\n\r\n")
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    assert!(head.starts_with("post /v1/messages "), "endpoint: {head}");
    assert!(head.contains("x-api-key: tok-9"), "auth header");
    assert!(
        head.contains("anthropic-version: 2023-06-01"),
        "version header"
    );
    let body = srv.only_body();
    assert_eq!(body["model"], "judge-model");
    assert_eq!(
        body["stream"], true,
        "streaming reader requires stream:true"
    );
    assert_eq!(body["system"], SYS, "fixed system text");
    let max = body["max_tokens"].as_u64().unwrap_or(u64::MAX);
    assert!(max <= 8, "max_tokens must be <= 8, got {max}");
    assert!(
        body.get("temperature").is_none(),
        "temperature omitted by invariant"
    );
    assert!(body.get("tools").is_none(), "no tools");
    assert!(body.get("functions").is_none(), "no functions");
    assert!(body.get("response_format").is_none(), "no JSON mode");
    let messages = body["messages"].as_array().cloned().unwrap_or_default();
    assert_eq!(messages.len(), 1, "system is top-level; one user message");
    assert_eq!(messages[0]["role"], "user");
    assert_eq!(messages[0]["content"], USR);
}

#[test]
fn direct_route_bare_token_replies_are_returned_trimmed() {
    let cases: [(&str, &str); 3] = [
        ("APPROVE", "APPROVE"),
        ("Approve.", "Approve."),
        ("  Approve.\n", "Approve."),
    ];
    for (raw, expected) in cases {
        let srv = fake(Reply::Sse(anthropic_frames(&[raw])));
        let s = direct_settings(&format!("http://{}", srv.addr), "judge-model", "TOK-9");
        assert_eq!(
            plain(&s, Duration::from_secs(10)),
            Ok(expected.to_string()),
            "reply {raw:?}"
        );
    }
}

#[test]
fn direct_route_sse_deltas_concatenate_in_order() {
    let srv = fake(Reply::Sse(anthropic_frames(&["APPRO", "VE"])));
    let s = direct_settings(&format!("http://{}", srv.addr), "judge-model", "TOK-9");
    assert_eq!(
        plain(&s, Duration::from_secs(10)),
        Ok("APPROVE".to_string())
    );
}

#[test]
fn direct_route_silent_endpoint_times_out_within_the_deadline() {
    let srv = fake(Reply::Hang);
    let s = direct_settings(&format!("http://{}", srv.addr), "judge-model", "TOK-9");
    let started = Instant::now();
    assert_eq!(
        plain(&s, Duration::from_millis(300)),
        Err(PlainError::Timeout)
    );
    assert!(started.elapsed() < Duration::from_secs(3));
}

#[test]
fn direct_route_unreachable_endpoint_is_backend_within_the_deadline() {
    // Bind then drop: the port is closed, so the connection is refused.
    let addr = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap();
    let s = direct_settings(&format!("http://{addr}"), "judge-model", "TOK-9");
    let started = Instant::now();
    assert_eq!(plain(&s, Duration::from_secs(4)), Err(PlainError::Backend));
    assert!(started.elapsed() < Duration::from_secs(4));
}

#[test]
fn direct_route_empty_url_model_or_token_is_unsupported_with_zero_calls() {
    let srv = fake(Reply::Sse(anthropic_frames(&["APPROVE"])));
    let url = format!("http://{}", srv.addr);
    let cases = [
        direct_settings("", "judge-model", "TOK-9"),
        direct_settings(&url, "  ", "TOK-9"),
        direct_settings(&url, "judge-model", ""),
    ];
    for s in &cases {
        assert_eq!(
            plain(s, Duration::from_secs(4)),
            Err(PlainError::Unsupported)
        );
    }
    assert_eq!(
        srv.connections(),
        0,
        "misconfigured direct route never calls out"
    );
}
