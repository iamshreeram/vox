//! Tests for `try_complete_streaming` (design R9, stage 1 streaming callback).
//! Same local mock-server setup as `realtime_complete_tests.rs`.

use super::*;
use crate::settings::get_default_settings;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

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

/// Replies to every request with `payload` (as an SSE body, connection: close).
/// Mirrors `mini_server` in `realtime_complete_tests.rs`.
fn sse_server(payload: String) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let conns = Arc::new(AtomicUsize::new(0));
    let c2 = conns.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut st) = stream else { break };
            c2.fetch_add(1, Ordering::SeqCst);
            let payload = payload.clone();
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
                                k.trim()
                                    .eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse::<usize>().ok())?
                            })
                            .unwrap_or(0);
                        while buf.len() < i + 4 + len {
                            match st.read(&mut tmp) {
                                Ok(0) | Err(_) => return,
                                Ok(n) => buf.extend_from_slice(&tmp[..n]),
                            }
                        }
                        break;
                    }
                    match st.read(&mut tmp) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head =
                    "HTTP/1.1 200 X\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";
                let _ = st.write_all(head.as_bytes());
                let _ = st.write_all(payload.as_bytes());
            });
        }
    });
    (format!("http://{addr}"), conns)
}

/// Anthropic-style SSE with one `text_delta` event per piece, then `message_stop`.
fn anthropic_frames(pieces: &[&str]) -> String {
    let mut out = String::new();
    for piece in pieces {
        out.push_str(&format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n",
            serde_json::to_string(piece).unwrap()
        ));
    }
    out.push_str("event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
    out
}

#[tokio::test]
async fn try_complete_streaming_forwards_every_delta_in_order_and_returns_the_concatenation() {
    let pieces = ["{\"kind\":", "\"answer\",", "\"speech\":\"hi\"}"];
    let (base, _conns) = sse_server(anthropic_frames(&pieces));
    let s = direct(&base);

    let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    let mut on_text = move |d: &str| sink.lock().unwrap().push(d.to_string());
    let r = try_complete_streaming(
        &s,
        "SYSTEM RULES",
        "SYSTEM RULES\n\nuser said hi",
        Duration::from_secs(5),
        &mut on_text,
    )
    .await;

    let returned = match r {
        Some(Ok(text)) => text,
        other => panic!("expected Some(Ok(_)), got {other:?}"),
    };
    let deltas = seen.lock().unwrap().clone();
    assert!(deltas.len() > 1, "expected several deltas, got {deltas:?}");
    assert_eq!(deltas.concat(), returned);
    assert_eq!(deltas, pieces.to_vec());
}

#[tokio::test]
async fn try_complete_streaming_returns_none_when_realtime_is_off_and_never_calls_back() {
    let (base, conns) = sse_server(anthropic_frames(&["x", "y", "z"]));
    let mut s = direct(&base);
    s.exp_rt_enabled = false;

    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let mut on_text = move |_: &str| {
        counter.fetch_add(1, Ordering::SeqCst);
    };
    let r = try_complete_streaming(&s, "S", "S\n\nu", Duration::from_secs(2), &mut on_text).await;

    assert_eq!(r, None);
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert_eq!(conns.load(Ordering::SeqCst), 0);
}
