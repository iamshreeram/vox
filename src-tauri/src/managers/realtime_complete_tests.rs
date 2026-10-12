//! Authoritative tests for `realtime_complete` routing and the preflight change
//! (design R9, stage 1). Do not edit to make code pass.

use super::*;
use crate::managers::converse_platform::preflight_check;
use crate::managers::realtime_config::{MSG_ENDPOINT_NEEDS_HTTPS, MSG_PROXY_INVALID, MSG_TOKEN_MISSING};
use crate::managers::composer_config::TtsComposerMode;
use crate::settings::get_default_settings;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

fn base_settings() -> AppSettings {
    let mut s = get_default_settings();
    s.tts_enabled = true;
    s
}

fn direct(url: &str) -> AppSettings {
    let mut s = base_settings();
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_endpoint_url = url.into();
    s.exp_rt_model = "some-model".into();
    s.exp_rt_token = "tok-123".into();
    s
}

fn composer(model: &str) -> AppSettings {
    let mut s = base_settings();
    s.exp_rt_enabled = true;
    s.tts_composer_mode = TtsComposerMode::Model;
    s.tts_composer_provider_id = "openai".into();
    s.tts_composer_model = model.into();
    s.post_process_api_keys.insert("openai".into(), "sk-composer".into());
    s
}

// ---- prepare -----------------------------------------------------------------

#[test]
fn master_off_is_always_legacy() {
    let mut s = direct("https://gw.example.com/anthropic");
    s.exp_rt_enabled = false;
    assert_eq!(prepare(&s), Ok(None));
    let mut c = composer("gpt-x");
    c.exp_rt_enabled = false;
    assert_eq!(prepare(&c), Ok(None));
    assert_eq!(prepare(&get_default_settings()), Ok(None));
}

#[test]
fn direct_endpoint_route() {
    let p = prepare(&direct(" https://gw.example.com/anthropic/ ")).unwrap().unwrap();
    assert_eq!(p.protocol, Protocol::AnthropicMessages);
    assert_eq!(p.base_url, "https://gw.example.com/anthropic");
    assert_eq!(p.api_key, "tok-123");
    assert_eq!(p.model, "some-model");
    assert!(!p.json_mode);
    assert_eq!(p.max_tokens, PLANNER_MAX_TOKENS);
}

#[test]
fn misconfigured_direct_endpoint_is_an_error_never_a_fallback() {
    // even with a perfectly good composer configured
    for mutate in [
        (|s: &mut AppSettings| s.exp_rt_token = "".into()) as fn(&mut AppSettings),
        |s| s.exp_rt_model = "".into(),
        |s| s.exp_rt_endpoint_url = "".into(),
        |s| s.exp_rt_endpoint_url = "ftp://x.example.com".into(),
        |s| s.exp_rt_endpoint_url = "http://gw.example.com".into(),
    ] {
        let mut s = composer("gpt-x");
        s.exp_rt_direct_endpoint = true;
        s.exp_rt_endpoint_url = "https://gw.example.com".into();
        s.exp_rt_model = "m".into();
        s.exp_rt_token = "t".into();
        mutate(&mut s);
        assert_eq!(prepare(&s), Err(ComposeError::NotConfigured));
    }
}

#[test]
fn prepared_debug_never_prints_the_key() {
    let p = prepare(&direct("https://gw.example.com")).unwrap().unwrap();
    let d = format!("{p:?}");
    assert!(!d.contains("tok-123"));
    assert!(d.contains("[REDACTED]"));
}

#[test]
fn composer_route_streams_openai_compatible() {
    let s = composer("gpt-x");
    let provider = s
        .post_process_providers
        .iter()
        .find(|p| p.id == "openai")
        .expect("default openai provider");
    let p = prepare(&s).unwrap().unwrap();
    assert_eq!(p.protocol, Protocol::OpenAiChat);
    assert_eq!(p.base_url, provider.base_url.trim_end_matches('/'));
    assert_eq!(p.api_key, "sk-composer");
    assert_eq!(p.model, "gpt-x");
    assert_eq!(p.json_mode, provider.supports_structured_output);
    assert_eq!(p.max_tokens, PLANNER_MAX_TOKENS);
}

#[test]
fn composer_route_falls_back_to_legacy_when_not_streamable() {
    let mut s = composer("gpt-x");
    s.exp_rt_stream_reply = false;
    assert_eq!(prepare(&s), Ok(None), "stream_reply off");
    assert_eq!(prepare(&composer("")), Ok(None), "empty model");
    assert_eq!(prepare(&composer("   ")), Ok(None), "blank model");
    let mut s = composer("gpt-x");
    s.tts_composer_mode = TtsComposerMode::Off;
    assert_eq!(prepare(&s), Ok(None), "composer off");
    s.tts_composer_mode = TtsComposerMode::Command;
    assert_eq!(prepare(&s), Ok(None), "command mode");
    let mut s = composer("gpt-x");
    s.tts_composer_provider_id = "does-not-exist".into();
    assert_eq!(prepare(&s), Ok(None), "unknown provider");
}

// ---- prewarm_target ------------------------------------------------------------

#[test]
fn prewarm_target_follows_route_and_toggle() {
    let s = direct("https://gw.example.com/anthropic");
    assert_eq!(prewarm_target(&s).as_deref(), Some("https://gw.example.com/anthropic"));
    let mut off = s.clone();
    off.exp_rt_prewarm = false;
    assert_eq!(prewarm_target(&off), None);
    let mut master = s.clone();
    master.exp_rt_enabled = false;
    assert_eq!(prewarm_target(&master), None);
    let mut bad = s.clone();
    bad.exp_rt_token = "".into();
    assert_eq!(prewarm_target(&bad), None, "misconfigured direct endpoint");
    assert_eq!(prewarm_target(&base_settings()), None, "legacy");
    assert!(prewarm_target(&composer("gpt-x")).is_some());
}

// ---- preflight_check -------------------------------------------------------------

#[test]
fn legacy_preflight_is_unchanged() {
    let s = base_settings();
    assert_eq!(
        preflight_check(&s),
        Err("Set up a reply backend in the speech settings first.".to_string())
    );
    let mut s = base_settings();
    s.tts_enabled = false;
    assert_eq!(
        preflight_check(&s),
        Err("Turn on spoken replies in the speech settings first.".to_string())
    );
    // direct fields set but the master is off => still the legacy answer
    let mut s = direct("https://gw.example.com");
    s.exp_rt_enabled = false;
    assert_eq!(
        preflight_check(&s),
        Err("Set up a reply backend in the speech settings first.".to_string())
    );
}

#[test]
fn direct_endpoint_replaces_the_composer_requirement() {
    let s = direct("https://gw.example.com/anthropic");
    assert_eq!(s.tts_composer_mode, TtsComposerMode::Off);
    assert_eq!(preflight_check(&s), Ok(()));
}

#[test]
fn direct_endpoint_problems_are_reported_with_static_messages() {
    let mut s = direct("https://gw.example.com");
    s.exp_rt_token = "".into();
    assert_eq!(preflight_check(&s), Err(MSG_TOKEN_MISSING.to_string()));
    let mut s = direct("http://gw.example.com");
    assert_eq!(preflight_check(&s), Err(MSG_ENDPOINT_NEEDS_HTTPS.to_string()));
    s = direct("https://gw.example.com");
    s.exp_rt_proxy_enabled = true;
    s.exp_rt_proxy_url = "garbage".into();
    assert_eq!(preflight_check(&s), Err(MSG_PROXY_INVALID.to_string()));
}

#[test]
fn other_preflight_checks_still_apply_in_realtime() {
    let mut s = direct("https://gw.example.com");
    s.tts_enabled = false;
    assert_eq!(
        preflight_check(&s),
        Err("Turn on spoken replies in the speech settings first.".to_string()),
        "spoken replies are still required, and are checked first"
    );
    let mut s = direct("https://gw.example.com");
    s.wake_word_enabled = true;
    assert_eq!(
        preflight_check(&s),
        Err("Turn off wake word and ambient listening before starting a conversation.".to_string())
    );
}

#[test]
fn composer_route_still_needs_the_composer_backend_for_preflight() {
    // master on, no direct endpoint, composer Off => legacy requirement applies
    let mut s = base_settings();
    s.exp_rt_enabled = true;
    assert_eq!(
        preflight_check(&s),
        Err("Set up a reply backend in the speech settings first.".to_string())
    );
    assert_eq!(preflight_check(&composer("gpt-x")), Ok(()));
}

// ---- try_complete against a tiny local server ------------------------------------

struct Mini {
    base: String,
    conns: Arc<AtomicUsize>,
    reqs: Arc<Mutex<Vec<(String, Vec<(String, String)>, Vec<u8>)>>>,
}

/// Replies to every request with `payload` (as an SSE body, connection: close), after `stall`.
fn mini_server(status: u16, payload: String, stall: Duration) -> Mini {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let conns = Arc::new(AtomicUsize::new(0));
    let reqs = Arc::new(Mutex::new(Vec::new()));
    let (c2, r2) = (conns.clone(), reqs.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut st) = stream else { break };
            c2.fetch_add(1, Ordering::SeqCst);
            let (payload, r2) = (payload.clone(), r2.clone());
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 2048];
                loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&buf[..i]).to_string();
                        let len = head
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.trim().eq_ignore_ascii_case("content-length").then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        while buf.len() < i + 4 + len {
                            match st.read(&mut tmp) {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                            }
                        }
                        let mut lines = head.lines();
                        let first = lines.next().unwrap_or("").to_string();
                        let headers = lines
                            .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())))
                            .collect();
                        r2.lock().unwrap().push((first, headers, buf[i + 4..i + 4 + len].to_vec()));
                        break;
                    }
                    match st.read(&mut tmp) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                std::thread::sleep(stall);
                let head = format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                );
                let _ = st.write_all(head.as_bytes());
                let _ = st.write_all(payload.as_bytes());
            });
        }
    });
    Mini { base: format!("http://{addr}"), conns, reqs }
}

fn anthropic_payload(text: &str) -> String {
    format!(
        "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\nevent: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
        serde_json::to_string(text).unwrap()
    )
}

#[tokio::test]
async fn direct_endpoint_request_carries_token_and_split_prompt() {
    let srv = mini_server(200, anthropic_payload("{\"kind\":\"answer\"}"), Duration::ZERO);
    let s = direct(&srv.base);
    let r = try_complete(&s, "SYSTEM RULES", "SYSTEM RULES\n\nuser said hi", Duration::from_secs(5)).await;
    assert_eq!(r, Some(Ok("{\"kind\":\"answer\"}".to_string())));
    let reqs = srv.reqs.lock().unwrap().clone();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].0.starts_with("POST /v1/messages"), "{}", reqs[0].0);
    assert!(reqs[0].1.iter().any(|(k, v)| k == "x-api-key" && v == "tok-123"));
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].2).unwrap();
    assert_eq!(body["system"], "SYSTEM RULES");
    assert_eq!(body["messages"][0]["content"], "user said hi");
    assert_eq!(body["model"], "some-model");
    assert_eq!(body["stream"], true);
    assert!(body.get("temperature").is_none());
}

#[tokio::test]
async fn bare_user_part_is_accepted() {
    let srv = mini_server(200, anthropic_payload("ok"), Duration::ZERO);
    let r = try_complete(&direct(&srv.base), "SYS", "just the user part", Duration::from_secs(5)).await;
    assert_eq!(r, Some(Ok("ok".to_string())));
    let reqs = srv.reqs.lock().unwrap().clone();
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].2).unwrap();
    assert_eq!(body["messages"][0]["content"], "just the user part");
}

#[tokio::test]
async fn master_off_and_misconfiguration_never_touch_the_network() {
    let srv = mini_server(200, anthropic_payload("x"), Duration::ZERO);
    let mut off = direct(&srv.base);
    off.exp_rt_enabled = false;
    assert_eq!(try_complete(&off, "S", "S\n\nu", Duration::from_secs(2)).await, None);
    let mut bad = direct(&srv.base);
    bad.exp_rt_token = "".into();
    assert_eq!(
        try_complete(&bad, "S", "S\n\nu", Duration::from_secs(2)).await,
        Some(Err(ComposeError::NotConfigured))
    );
    assert_eq!(srv.conns.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn backend_errors_and_deadlines_surface() {
    let srv = mini_server(401, "SECRET".into(), Duration::ZERO);
    let r = try_complete(&direct(&srv.base), "S", "S\n\nu", Duration::from_secs(5)).await;
    assert_eq!(r, Some(Err(ComposeError::Backend)));

    let slow = mini_server(200, anthropic_payload("late"), Duration::from_secs(4));
    let t0 = Instant::now();
    let r = try_complete(&direct(&slow.base), "S", "S\n\nu", Duration::from_millis(500)).await;
    assert_eq!(r, Some(Err(ComposeError::Timeout)));
    assert!(t0.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn composer_route_streams_openai_compatible_to_the_provider() {
    // point the composer's provider at the local server
    let payload = "data: {\"choices\":[{\"delta\":{\"content\":\"hi\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n".to_string();
    let srv = mini_server(200, payload, Duration::ZERO);
    let mut s = composer("gpt-x");
    for p in s.post_process_providers.iter_mut() {
        if p.id == "openai" {
            p.base_url = format!("{}/v1", srv.base);
        }
    }
    let r = try_complete(&s, "S", "S\n\nu", Duration::from_secs(5)).await;
    assert_eq!(r, Some(Ok("hi".to_string())));
    let reqs = srv.reqs.lock().unwrap().clone();
    assert!(reqs[0].0.starts_with("POST /v1/chat/completions"), "{}", reqs[0].0);
    assert!(reqs[0].1.iter().any(|(k, v)| k == "authorization" && v == "Bearer sk-composer"));
}

#[tokio::test]
async fn legacy_configuration_returns_none_without_network() {
    let srv = mini_server(200, anthropic_payload("x"), Duration::ZERO);
    let mut s = base_settings();
    s.exp_rt_endpoint_url = srv.base.clone();
    assert_eq!(try_complete(&s, "S", "S\n\nu", Duration::from_secs(2)).await, None);
    assert_eq!(srv.conns.load(Ordering::SeqCst), 0);
}
