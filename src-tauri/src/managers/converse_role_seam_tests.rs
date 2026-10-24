#![cfg(unix)]
//! Stage 1 roles: production-seam tests (DESIGN-PR2-STAGE1 Revision 3, sections 2.3, 2.7 and T14).
//! Authoritative spec, written before the implementation (step S3). Do not edit to make code pass.
//!
//! Plain route: the planner prompt is sent through `complete` in command mode; the fake composer is
//! a shell script that records its last argument (the joined system + user prompt) and prints the
//! reply. Tool route: a loopback Anthropic SSE mock records the raw request, whose `system` field is
//! the system prompt. No network, no real backend.
#![allow(dead_code)]

#[allow(unused_imports)]
use super::*;
use crate::managers::composer_config::TtsComposerMode;
use crate::managers::composer_rules;
use crate::managers::converse_plan::PlanInput;
use crate::managers::llm_tools::ToolSpec;
use crate::managers::tool_loop::{ToolExecutor, ToolOutput};
use crate::settings::get_default_settings;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const ROLE: &str = "a patient teacher for six year olds";
/// The start of the roles teaching addendum (`ROLE_PLAN_ADDENDUM`), present only with roles enabled.
const ADDENDUM_MARK: &str = "Roles: when the user asks you to act as";
const TEST_KEY: &str = concat!("test-", "key-", "value");

// ---- temp dir and fake composer (command mode) ------------------------------------------------

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "vox-role-seam-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        TempDir(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Command-mode settings whose composer records the prompt it was given into the returned file and
/// prints `reply` on stdout.
fn fake_composer(dir: &TempDir, reply: &str) -> (AppSettings, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let capture = dir.path().join("prompt.txt");
    let script = dir.path().join("fake-composer");
    let body = format!(
        "#!/bin/sh\nfor last; do :; done\nprintf '%s' \"$last\" > '{}'\nprintf '%s' '{}'\n",
        capture.display(),
        reply
    );
    std::fs::write(&script, body).expect("write fake composer");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let mut s = get_default_settings();
    s.tts_composer_mode = TtsComposerMode::Command;
    s.tts_composer_command_path = script.to_string_lossy().to_string();
    s.tts_composer_command_args = String::new();
    s.tts_composer_prompt_flag = "-p".to_string();
    s.tts_composer_command_ack = composer_rules::normalize_command_tuple(
        &s.tts_composer_command_path,
        &s.tts_composer_command_args,
        &s.tts_composer_prompt_flag,
    )
    .expect("normalize command tuple");
    (s, capture)
}

fn plain_input<'a>(role: Option<&'a str>, roles_enabled: bool) -> PlanInput<'a> {
    PlanInput {
        utterance: "teach me fractions",
        history: &[],
        last_draft: None,
        style: "",
        vault_context: None,
        role,
        lesson_paused: false,
        roles_enabled,
        agent_enabled: false,
    }
}

fn answer_json(speech: &str) -> String {
    json!({"kind": "answer", "speech": speech, "emotion": "calm"}).to_string()
}

fn long_speech() -> String {
    "s".repeat(900)
}

// ---- loopback Anthropic mock (tool route) -----------------------------------------------------

/// Minimal blocking loopback server: reads one request, records the raw bytes, writes `response`.
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

/// An SSE reply whose only content is one text block carrying `text` (a final planner JSON).
fn text_reply(text: &str) -> Vec<u8> {
    let events = vec![
        json!({"type": "message_start", "message": {"id": "m", "type": "message",
            "role": "assistant", "content": [], "model": "m", "stop_reason": null}}),
        json!({"type": "content_block_start", "index": 0,
            "content_block": {"type": "text", "text": ""}}),
        json!({"type": "content_block_delta", "index": 0,
            "delta": {"type": "text_delta", "text": text}}),
        json!({"type": "content_block_stop", "index": 0}),
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn",
            "stop_sequence": null}}),
        json!({"type": "message_stop"}),
    ];
    sse_response(&events)
}

fn tool_settings() -> AppSettings {
    let mut s = get_default_settings();
    s.tts_enabled = true;
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_auto_proxy = false;
    s.agent_tools_enabled = true;
    s.agent_tools_guard_actions = true;
    s.exp_rt_endpoint_url = "https://api.example.invalid".into();
    s.exp_rt_model = "claude-test".into();
    s.exp_rt_token = TEST_KEY.into();
    s
}

fn loopback_route(base: &str) -> Prepared {
    Prepared {
        protocol: crate::managers::llm_stream::Protocol::AnthropicMessages,
        base_url: base.to_string(),
        api_key: TEST_KEY.to_string(),
        model: "claude-test".to_string(),
        json_mode: false,
        completion_tokens_field: false,
        max_tokens: 1500,
    }
}

/// The `system` field of the recorded Anthropic request.
fn captured_system(raw: &[u8]) -> String {
    let text = String::from_utf8_lossy(raw);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or("");
    let json: Value = serde_json::from_str(body).expect("request body is JSON");
    match &json["system"] {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Tool executor that is never reached: the scripted replies are final planner JSON.
struct NoTools;

impl ToolExecutor for NoTools {
    fn specs(&self) -> Vec<ToolSpec> {
        Vec::new()
    }

    fn call<'a>(
        &'a self,
        _name: &'a str,
        _input: &'a Value,
    ) -> crate::managers::converse::BoxFuture<'a, ToolOutput> {
        Box::pin(async {
            ToolOutput {
                text: String::new(),
                is_error: false,
                untrusted: false,
            }
        })
    }
}

fn no_tools() -> impl Fn(&AppSettings) -> Arc<dyn ToolExecutor> + Sync {
    |_s: &AppSettings| -> Arc<dyn ToolExecutor> { Arc::new(NoTools) }
}

// ---- plain route ------------------------------------------------------------------------------

#[tokio::test]
async fn role_seam_plain_route_prompt_carries_the_role_and_the_addendum() {
    let dir = TempDir::new("plain-role");
    let (s, capture) = fake_composer(&dir, &answer_json("Sure."));
    let input = plain_input(Some(ROLE), true);
    let plan = plan_plain(&s, &input, &TaintState::new(), None)
        .await
        .expect("plain route plans");
    assert_eq!(plan.speech, "Sure.");
    let prompt = std::fs::read_to_string(&capture).expect("composer ran");
    assert!(
        prompt.contains(ROLE),
        "the active role must be in the system prompt sent on the plain route"
    );
    assert!(
        prompt.contains(ADDENDUM_MARK),
        "the roles addendum must be present when roles are enabled"
    );
}

#[tokio::test]
async fn role_seam_plain_route_prompt_has_no_role_text_without_a_role() {
    let dir = TempDir::new("plain-norole");
    let (s, capture) = fake_composer(&dir, &answer_json("Sure."));
    let input = plain_input(None, false);
    let _ = plan_plain(&s, &input, &TaintState::new(), None)
        .await
        .expect("plain route plans");
    let prompt = std::fs::read_to_string(&capture).expect("composer ran");
    assert!(
        !prompt.contains(ROLE),
        "no role text without an active role"
    );
    assert!(
        !prompt.contains(ADDENDUM_MARK),
        "no roles addendum when roles are disabled"
    );
}

#[tokio::test]
async fn role_seam_plain_route_keeps_a_900_char_answer_only_with_a_role() {
    let dir = TempDir::new("plain-cap-role");
    let (s, _capture) = fake_composer(&dir, &answer_json(&long_speech()));
    let input = plain_input(Some(ROLE), true);
    let plan = plan_plain(&s, &input, &TaintState::new(), None)
        .await
        .expect("plain route plans");
    assert_eq!(
        plan.speech.chars().count(),
        900,
        "with a role active the 900-char answer survives parsing"
    );

    let dir = TempDir::new("plain-cap-norole");
    let (s, _capture) = fake_composer(&dir, &answer_json(&long_speech()));
    let input = plain_input(None, false);
    let plan = plan_plain(&s, &input, &TaintState::new(), None)
        .await
        .expect("plain route plans");
    assert_eq!(
        plan.speech.chars().count(),
        400,
        "without a role the answer is cut to 400"
    );
}

// ---- tool route -------------------------------------------------------------------------------

#[tokio::test]
async fn role_seam_tool_route_system_prompt_carries_the_role_and_the_addendum() {
    let (base, seen) = serve_once(text_reply(&answer_json("Sure.")));
    let make = no_tools();
    let input = plain_input(Some(ROLE), true);
    let plan = plan_with_tools(
        &tool_settings(),
        loopback_route(&base),
        &input,
        &TaintState::new(),
        &make,
    )
    .await
    .expect("tool route plans");
    assert_eq!(plan.speech, "Sure.");
    let system = captured_system(&seen.lock().unwrap());
    assert!(
        system.contains(ROLE),
        "the active role must be in the system prompt sent on the tool route"
    );
    assert!(
        system.contains(ADDENDUM_MARK),
        "the roles addendum must be present when roles are enabled"
    );
}

#[tokio::test]
async fn role_seam_tool_route_system_prompt_has_no_role_text_without_a_role() {
    let (base, seen) = serve_once(text_reply(&answer_json("Sure.")));
    let make = no_tools();
    let input = plain_input(None, false);
    let _ = plan_with_tools(
        &tool_settings(),
        loopback_route(&base),
        &input,
        &TaintState::new(),
        &make,
    )
    .await
    .expect("tool route plans");
    let system = captured_system(&seen.lock().unwrap());
    assert!(
        !system.contains(ROLE),
        "no role text without an active role"
    );
    assert!(
        !system.contains(ADDENDUM_MARK),
        "no addendum when roles are disabled"
    );
}

#[tokio::test]
async fn role_seam_tool_route_keeps_a_900_char_answer_only_with_a_role() {
    let make = no_tools();

    let (base, _seen) = serve_once(text_reply(&answer_json(&long_speech())));
    let input = plain_input(Some(ROLE), true);
    let plan = plan_with_tools(
        &tool_settings(),
        loopback_route(&base),
        &input,
        &TaintState::new(),
        &make,
    )
    .await
    .expect("tool route plans");
    assert_eq!(
        plan.speech.chars().count(),
        900,
        "with a role active the 900-char answer survives parsing on the tool route"
    );

    let (base, _seen) = serve_once(text_reply(&answer_json(&long_speech())));
    let input = plain_input(None, false);
    let plan = plan_with_tools(
        &tool_settings(),
        loopback_route(&base),
        &input,
        &TaintState::new(),
        &make,
    )
    .await
    .expect("tool route plans");
    assert_eq!(
        plan.speech.chars().count(),
        400,
        "without a role the tool-route answer is cut to 400"
    );
}

// ---- END ----
