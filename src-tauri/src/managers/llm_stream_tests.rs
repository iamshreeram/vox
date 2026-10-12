//! Authoritative tests for `llm_stream` (design R5/R6.5/R7/R9). Written BEFORE the
//! implementation; do not edit to make code pass.

use super::*;
use serde_json::json;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// validate_endpoint
// ---------------------------------------------------------------------------

#[test]
fn endpoint_accepts_https_and_normalizes() {
    let e = validate_endpoint("  https://api.example.com/anthropic/  ", true).unwrap();
    assert_eq!(e.base, "https://api.example.com/anthropic");
    assert!(!e.loopback);
    let e = validate_endpoint("https://api.example.com", false).unwrap();
    assert_eq!(e.base, "https://api.example.com");
}

#[test]
fn endpoint_rejections() {
    let k = true;
    assert_eq!(validate_endpoint("", k), Err(EndpointError::Invalid));
    assert_eq!(validate_endpoint("not a url", k), Err(EndpointError::Invalid));
    assert_eq!(validate_endpoint("ftp://x.example.com", k), Err(EndpointError::Scheme));
    assert_eq!(validate_endpoint("file:///etc/passwd", k), Err(EndpointError::Scheme));
    assert_eq!(validate_endpoint("https://u:p@x.example.com", k), Err(EndpointError::Userinfo));
    assert_eq!(validate_endpoint("https://u@x.example.com", k), Err(EndpointError::Userinfo));
    assert_eq!(validate_endpoint("https://x.example.com/a#frag", k), Err(EndpointError::Fragment));
    assert_eq!(validate_endpoint("https://x.example.com/a?key=1", k), Err(EndpointError::Query));
    assert_eq!(validate_endpoint("https://x.example.com/a?", k), Err(EndpointError::Query));
}

#[test]
fn plain_http_only_for_loopback_or_keyless() {
    assert_eq!(
        validate_endpoint("http://api.example.com", true),
        Err(EndpointError::InsecureWithKey)
    );
    assert!(validate_endpoint("http://api.example.com", false).is_ok());
    for host in ["http://localhost:8080", "http://127.0.0.1:1", "http://127.9.9.9", "http://[::1]:9"] {
        let e = validate_endpoint(host, true).unwrap_or_else(|_| panic!("{host}"));
        assert!(e.loopback, "{host}");
    }
    assert!(!validate_endpoint("http://127.0.0.1.evil.com", false).unwrap().loopback);
    assert!(!validate_endpoint("https://localhost.evil.com", true).unwrap().loopback);
}

// ---------------------------------------------------------------------------
// build_request
// ---------------------------------------------------------------------------

fn call<'a>(protocol: Protocol, base: &'a str, key: &'a str) -> StreamCall<'a> {
    StreamCall {
        protocol,
        base_url: base,
        api_key: key,
        model: "m-1",
        system: "SYS",
        user: "USER",
        max_tokens: 321,
        json_mode: false,
        completion_tokens_field: false,
    }
}

fn header<'a>(r: &'a BuiltRequest, name: &str) -> Option<&'a str> {
    r.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

#[test]
fn anthropic_request_shape() {
    let r = build_request(&call(Protocol::AnthropicMessages, "https://h.example.com/anthropic", "KEY"));
    assert_eq!(r.url, "https://h.example.com/anthropic/v1/messages");
    assert_eq!(header(&r, "x-api-key"), Some("KEY"));
    assert_eq!(header(&r, "anthropic-version"), Some("2023-06-01"));
    assert_eq!(header(&r, "content-type"), Some("application/json"));
    assert_eq!(header(&r, "authorization"), None);
    assert_eq!(r.body["model"], "m-1");
    assert_eq!(r.body["max_tokens"], 321);
    assert_eq!(r.body["stream"], true);
    assert_eq!(r.body["system"], "SYS");
    assert_eq!(r.body["messages"], json!([{"role": "user", "content": "USER"}]));
    assert!(r.body.get("temperature").is_none());
    // no assistant prefill, exactly one message
    assert_eq!(r.body["messages"].as_array().unwrap().len(), 1);
}

#[test]
fn anthropic_base_ending_in_v1_does_not_double_the_segment() {
    let r = build_request(&call(Protocol::AnthropicMessages, "https://h.example.com/v1", "K"));
    assert_eq!(r.url, "https://h.example.com/v1/messages");
    let r = build_request(&call(Protocol::AnthropicMessages, "https://h.example.com/v1/", "K"));
    assert_eq!(r.url, "https://h.example.com/v1/messages");
}

#[test]
fn openai_request_shape_and_optional_pieces() {
    let mut c = call(Protocol::OpenAiChat, "https://api.example.com/v1", "KEY");
    let r = build_request(&c);
    assert_eq!(r.url, "https://api.example.com/v1/chat/completions");
    assert_eq!(header(&r, "authorization"), Some("Bearer KEY"));
    assert_eq!(header(&r, "x-api-key"), None);
    assert_eq!(r.body["stream"], true);
    assert_eq!(r.body["max_tokens"], 321);
    assert!(r.body.get("max_completion_tokens").is_none());
    assert!(r.body.get("temperature").is_none());
    assert!(r.body.get("response_format").is_none());
    assert_eq!(
        r.body["messages"],
        json!([{"role": "system", "content": "SYS"}, {"role": "user", "content": "USER"}])
    );

    c.api_key = "";
    assert_eq!(header(&build_request(&c), "authorization"), None);

    c.json_mode = true;
    c.completion_tokens_field = true;
    let r = build_request(&c);
    assert_eq!(r.body["response_format"], json!({"type": "json_object"}));
    assert_eq!(r.body["max_completion_tokens"], 321);
    assert!(r.body.get("max_tokens").is_none());
}

// ---------------------------------------------------------------------------
// SseParser
// ---------------------------------------------------------------------------

fn parse_all(chunks: &[&[u8]]) -> Vec<SseEvent> {
    let mut p = SseParser::new();
    let mut out = Vec::new();
    for c in chunks {
        out.extend(p.push(c).unwrap());
    }
    out
}

const SAMPLE: &str = "event: content_block_delta\ndata: {\"a\":1}\n\n: keep-alive comment\n\ndata:no-space\n\ndata: l1\ndata: l2\n\nevent: ping\ndata: x\n\n";

fn expected_sample() -> Vec<SseEvent> {
    vec![
        SseEvent { event: Some("content_block_delta".into()), data: "{\"a\":1}".into() },
        SseEvent { event: None, data: "no-space".into() },
        SseEvent { event: None, data: "l1\nl2".into() },
        SseEvent { event: Some("ping".into()), data: "x".into() },
    ]
}

#[test]
fn sse_whole_input() {
    assert_eq!(parse_all(&[SAMPLE.as_bytes()]), expected_sample());
}

#[test]
fn sse_identical_for_every_split_point() {
    let bytes = SAMPLE.as_bytes();
    for i in 0..=bytes.len() {
        let got = parse_all(&[&bytes[..i], &bytes[i..]]);
        assert_eq!(got, expected_sample(), "split at {i}");
    }
}

#[test]
fn sse_identical_when_fed_one_byte_at_a_time() {
    let chunks: Vec<&[u8]> = SAMPLE.as_bytes().chunks(1).collect();
    assert_eq!(parse_all(&chunks), expected_sample());
}

#[test]
fn sse_crlf_and_cr_line_endings() {
    let crlf = SAMPLE.replace('\n', "\r\n");
    assert_eq!(parse_all(&[crlf.as_bytes()]), expected_sample());
    let cr = SAMPLE.replace('\n', "\r");
    assert_eq!(parse_all(&[cr.as_bytes()]), expected_sample());
    // split between CR and LF
    let b = crlf.as_bytes();
    let cut = crlf.find("\r\n").unwrap() + 1;
    assert_eq!(parse_all(&[&b[..cut], &b[cut..]]), expected_sample());
}

#[test]
fn sse_multibyte_character_split_across_chunks() {
    let s = "data: h\u{e9}llo \u{1F600} end\n\n";
    let b = s.as_bytes();
    for i in 0..=b.len() {
        let got = parse_all(&[&b[..i], &b[i..]]);
        assert_eq!(got.len(), 1, "split {i}");
        assert_eq!(got[0].data, "h\u{e9}llo \u{1F600} end", "split {i}");
    }
}

#[test]
fn sse_unterminated_final_event_is_discarded() {
    assert_eq!(parse_all(&[b"data: a\n\ndata: partial"]).len(), 1);
}

#[test]
fn sse_limits() {
    let mut p = SseParser::new();
    let big = vec![b'x'; MAX_FRAME_BYTES + 10];
    let mut line = b"data: ".to_vec();
    line.extend_from_slice(&big);
    line.push(b'\n');
    assert_eq!(p.push(&line), Err(SseError::FrameTooLarge));

    // a frame made of many small lines also counts as one frame
    let mut p = SseParser::new();
    let mut err = None;
    for _ in 0..(MAX_FRAME_BYTES / 4 + 100) {
        if let Err(e) = p.push(b"data: ab\n") {
            err = Some(e);
            break;
        }
    }
    assert_eq!(err, Some(SseError::FrameTooLarge));

    // endless ignorable comments
    let mut p = SseParser::new();
    let mut err = None;
    for _ in 0..(MAX_IGNORED_EVENTS + 50) {
        if let Err(e) = p.push(b": c\n\n") {
            err = Some(e);
            break;
        }
    }
    assert_eq!(err, Some(SseError::TooManyIgnored));
}

// ---------------------------------------------------------------------------
// extract_delta
// ---------------------------------------------------------------------------

fn ex(p: Protocol, v: serde_json::Value) -> Extracted {
    extract_delta(p, &v)
}

fn txt(t: &str, end: End) -> Extracted {
    Extracted { text: Some(t.into()), end }
}

fn none() -> Extracted {
    Extracted { text: None, end: End::None }
}

#[test]
fn anthropic_deltas() {
    let p = Protocol::AnthropicMessages;
    assert_eq!(
        ex(p, json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hel"}})),
        txt("Hel", End::None)
    );
    assert_eq!(ex(p, json!({"type":"message_stop"})), Extracted { text: None, end: End::Done });
    for ignorable in [
        json!({"type":"ping"}),
        json!({"type":"message_start","message":{}}),
        json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}),
        json!({"type":"content_block_stop","index":0}),
        json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        json!({"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}}),
        json!({"type":"content_block_delta","delta":{"type":"text_delta","text":""}}),
        json!({"nothing":true}),
    ] {
        assert_eq!(ex(p, ignorable.clone()), none(), "{ignorable}");
    }
    assert_eq!(
        ex(p, json!({"type":"message_delta","delta":{"stop_reason":"max_tokens"}})).end,
        End::Error
    );
    assert_eq!(
        ex(p, json!({"type":"message_delta","delta":{"stop_reason":"refusal"}})).end,
        End::Error
    );
    assert_eq!(ex(p, json!({"type":"error","error":{"message":"x"}})).end, End::Error);
}

#[test]
fn openai_deltas() {
    let p = Protocol::OpenAiChat;
    assert_eq!(ex(p, json!({"choices":[{"delta":{"content":"Hi"}}]})), txt("Hi", End::None));
    assert_eq!(ex(p, json!({"choices":[{"delta":{"role":"assistant","content":""}}]})), none());
    assert_eq!(ex(p, json!({"choices":[{"delta":{}, "finish_reason": null}]})), none());
    assert_eq!(
        ex(p, json!({"choices":[{"delta":{}, "finish_reason": "stop"}]})),
        Extracted { text: None, end: End::Done }
    );
    // final chunk may carry text and the stop together
    assert_eq!(
        ex(p, json!({"choices":[{"delta":{"content":"!"}, "finish_reason": "stop"}]})),
        txt("!", End::Done)
    );
    for bad in ["length", "content_filter", "tool_calls"] {
        assert_eq!(
            ex(p, json!({"choices":[{"delta":{}, "finish_reason": bad}]})).end,
            End::Error,
            "{bad}"
        );
    }
    assert_eq!(ex(p, json!({"choices":[]})), none());
    assert_eq!(ex(p, json!({"error":{"message":"x"}})).end, End::Error);
}

// ---------------------------------------------------------------------------
// resolve_proxy
// ---------------------------------------------------------------------------

fn env_of(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let m: HashMap<String, String> =
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    move |k: &str| m.get(k).cloned()
}

#[test]
fn proxy_resolution_order() {
    let no_env = env_of(&[]);
    let shell_calls = AtomicUsize::new(0);
    let shell = || {
        shell_calls.fetch_add(1, Ordering::SeqCst);
        Some("http://shell.example:8080/".to_string())
    };

    // manual wins and never touches env or shell
    assert_eq!(
        resolve_proxy(Some("http://manual.example:3128"), true, &no_env, &shell),
        ProxyMode::Explicit("http://manual.example:3128".into())
    );
    assert_eq!(shell_calls.load(Ordering::SeqCst), 0);

    // invalid manual is ignored and falls through
    assert_eq!(
        resolve_proxy(Some("not a proxy"), true, &no_env, &shell),
        ProxyMode::Explicit("http://shell.example:8080/".into())
    );
    assert_eq!(shell_calls.load(Ordering::SeqCst), 1);

    // env proxy => library default, shell not consulted
    let with_env = env_of(&[("https_proxy", "http://env.example:8080")]);
    assert_eq!(resolve_proxy(None, true, &with_env, &shell), ProxyMode::SystemDefault);
    let with_upper = env_of(&[("HTTPS_PROXY", "http://env.example:8080")]);
    assert_eq!(resolve_proxy(None, true, &with_upper, &shell), ProxyMode::SystemDefault);
    assert_eq!(shell_calls.load(Ordering::SeqCst), 1);

    // auto off: shell never consulted
    assert_eq!(resolve_proxy(None, false, &no_env, &shell), ProxyMode::SystemDefault);
    assert_eq!(shell_calls.load(Ordering::SeqCst), 1);

    // invalid shell output is rejected
    for bad in ["", "garbage", "ftp://x:1", "http://x:1/p?q=1", "http://x:1/#f"] {
        let s = || Some(bad.to_string());
        assert_eq!(resolve_proxy(None, true, &no_env, &s), ProxyMode::SystemDefault, "{bad:?}");
    }
    let none_shell = || None;
    assert_eq!(resolve_proxy(None, true, &no_env, &none_shell), ProxyMode::SystemDefault);
}

#[test]
fn proxy_url_validity() {
    for ok in ["http://p.example:8080", "https://p.example", "http://u:p@p.example:3128/", " http://p.example:1 "] {
        assert!(is_valid_proxy_url(ok), "{ok:?}");
    }
    for bad in ["", "p.example:8080", "ftp://p.example", "http://", "http://p.example/?a=1", "http://p.example/#x", "garbage"] {
        assert!(!is_valid_proxy_url(bad), "{bad:?}");
    }
}

// ---------------------------------------------------------------------------
// Scripted fake HTTP/1.1 server (std threads; keep-alive; chunked responses)
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Gate(Arc<(Mutex<bool>, Condvar)>);

impl Gate {
    fn new() -> Self {
        Gate(Arc::new((Mutex::new(false), Condvar::new())))
    }
    fn open(&self) {
        *self.0 .0.lock().unwrap() = true;
        self.0 .1.notify_all();
    }
    fn wait(&self, max: Duration) {
        let (m, c) = &*self.0;
        let g = m.lock().unwrap();
        let _ = c.wait_timeout_while(g, max, |open| !*open).unwrap();
    }
}

#[derive(Debug, Clone)]
struct Req {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

enum Step {
    Chunk(Vec<u8>),
    Wait(Gate),
    /// Block until the peer closes the socket (or 5 s), then report whether it closed.
    ExpectClose(std::sync::mpsc::Sender<bool>),
    End,
}

enum Reply {
    Stream(Vec<Step>),
    Status(u16, &'static str, Option<&'static str>),
}

type Handler = Box<dyn Fn(&Req) -> Reply + Send + Sync>;

struct Server {
    addr: std::net::SocketAddr,
    conns: Arc<AtomicUsize>,
    reqs: Arc<Mutex<Vec<Req>>>,
}

impl Server {
    fn base(&self) -> String {
        format!("http://{}", self.addr)
    }
    fn requests(&self) -> Vec<Req> {
        self.reqs.lock().unwrap().clone()
    }
}

fn read_request(s: &mut TcpStream) -> Option<Req> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 1024];
    let head_end;
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            head_end = i;
            break;
        }
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let first = lines.next()?;
    let mut parts = first.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_string(), v.trim().to_string())))
        .collect();
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut tmp).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&tmp[..n]);
    }
    Some(Req { method, target, headers, body })
}

fn write_chunk(s: &mut TcpStream, data: &[u8]) -> bool {
    let mut out = format!("{:x}\r\n", data.len()).into_bytes();
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
    s.write_all(&out).and_then(|_| s.flush()).is_ok()
}

fn serve(handler: Handler) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let conns = Arc::new(AtomicUsize::new(0));
    let reqs = Arc::new(Mutex::new(Vec::new()));
    let handler = Arc::new(handler);
    {
        let (conns, reqs) = (conns.clone(), reqs.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                conns.fetch_add(1, Ordering::SeqCst);
                let (handler, reqs) = (handler.clone(), reqs.clone());
                std::thread::spawn(move || {
                    while let Some(req) = read_request(&mut stream) {
                        reqs.lock().unwrap().push(req.clone());
                        match handler(&req) {
                            Reply::Status(code, body, loc) => {
                                let mut h = format!(
                                    "HTTP/1.1 {code} X\r\nContent-Length: {}\r\n",
                                    if req.method == "HEAD" { 0 } else { body.len() }
                                );
                                if let Some(l) = loc {
                                    h.push_str(&format!("Location: {l}\r\n"));
                                }
                                h.push_str("\r\n");
                                let mut out = h.into_bytes();
                                if req.method != "HEAD" {
                                    out.extend_from_slice(body.as_bytes());
                                }
                                if stream.write_all(&out).is_err() {
                                    return;
                                }
                            }
                            Reply::Stream(steps) => {
                                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n";
                                if stream.write_all(head.as_bytes()).is_err() {
                                    return;
                                }
                                for step in steps {
                                    match step {
                                        Step::Chunk(d) => {
                                            if !write_chunk(&mut stream, &d) {
                                                return;
                                            }
                                        }
                                        Step::Wait(g) => g.wait(Duration::from_secs(10)),
                                        Step::ExpectClose(tx) => {
                                            let _ = stream
                                                .set_read_timeout(Some(Duration::from_secs(5)));
                                            let mut b = [0u8; 16];
                                            let closed = matches!(stream.read(&mut b), Ok(0) | Err(_));
                                            let _ = tx.send(closed);
                                            return;
                                        }
                                        Step::End => {
                                            if stream.write_all(b"0\r\n\r\n").is_err() {
                                                return;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                });
            }
        });
    }
    Server { addr, conns, reqs }
}

// SSE payload helpers -------------------------------------------------------

fn a_start() -> Vec<u8> {
    b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{}}\n\n".to_vec()
}
fn a_text(t: &str) -> Vec<u8> {
    format!(
        "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n",
        serde_json::to_string(t).unwrap()
    )
    .into_bytes()
}
fn a_stop() -> Vec<u8> {
    b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n".to_vec()
}
fn o_text(t: &str) -> Vec<u8> {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":{}}}}}]}}\n\n", serde_json::to_string(t).unwrap())
        .into_bytes()
}
fn o_finish(reason: &str) -> Vec<u8> {
    format!("data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"{reason}\"}}]}}\n\n").into_bytes()
}

fn opts(overall_ms: u64, first_ms: u64) -> StreamOpts {
    StreamOpts {
        overall: Duration::from_millis(overall_ms),
        first_text: Duration::from_millis(first_ms),
        proxy: ProxyMode::SystemDefault,
    }
}

fn anthropic_call(base: &str) -> StreamCall<'_> {
    call(Protocol::AnthropicMessages, base, "test-key")
}

async fn run_collect(c: &StreamCall<'_>, o: &StreamOpts) -> (Result<String, ComposeError>, Vec<String>) {
    let mut got = Vec::new();
    let mut cb = |t: &str| got.push(t.to_string());
    let r = stream_completion(c, o, &mut cb).await;
    (r, got)
}

fn is_backend(r: &Result<String, ComposeError>) -> bool {
    matches!(r, Err(ComposeError::Backend))
}

// ---------------------------------------------------------------------------
// stream_completion behaviour
// ---------------------------------------------------------------------------

#[tokio::test]
async fn anthropic_stream_collects_text_and_sends_expected_request() {
    let srv = serve(Box::new(|_| {
        Reply::Stream(vec![
            Step::Chunk(a_start()),
            Step::Chunk(a_text("Hel")),
            Step::Chunk(a_text("lo")),
            Step::Chunk(a_stop()),
            Step::End,
        ])
    }));
    let base = srv.base();
    let (r, got) = run_collect(&anthropic_call(&base), &opts(5000, 5000)).await;
    assert_eq!(r.unwrap(), "Hello");
    assert_eq!(got, vec!["Hel", "lo"]);
    let reqs = srv.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[0].target, "/v1/messages");
    assert_eq!(reqs[0].header("x-api-key"), Some("test-key"));
    let body: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    assert_eq!(body["stream"], true);
    assert!(body.get("temperature").is_none());
}

#[tokio::test]
async fn openai_stream_completes_on_finish_reason_without_done_marker() {
    let srv = serve(Box::new(|_| {
        Reply::Stream(vec![Step::Chunk(o_text("a")), Step::Chunk(o_text("b")), Step::Chunk(o_finish("stop")), Step::End])
    }));
    let base = format!("{}/v1", srv.base());
    let c = call(Protocol::OpenAiChat, &base, "");
    let (r, _) = run_collect(&c, &opts(5000, 5000)).await;
    assert_eq!(r.unwrap(), "ab");
    let reqs = srv.requests();
    assert_eq!(reqs[0].target, "/v1/chat/completions");
    assert_eq!(reqs[0].header("authorization"), None);
}

#[tokio::test]
async fn first_text_callback_fires_before_the_server_finishes() {
    let gate = Gate::new();
    let g2 = gate.clone();
    let srv = serve(Box::new(move |_| {
        Reply::Stream(vec![
            Step::Chunk(a_start()),
            Step::Chunk(a_text("Hel")),
            Step::Wait(g2.clone()),
            Step::Chunk(a_text("lo")),
            Step::Chunk(a_stop()),
            Step::End,
        ])
    }));
    let base = srv.base();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        let c = anthropic_call(&base);
        let mut cb = |t: &str| {
            let _ = tx.send(t.to_string());
        };
        stream_completion(&c, &opts(8000, 8000), &mut cb).await
    });
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("first text must arrive while the server is still blocked")
        .unwrap();
    assert_eq!(first, "Hel");
    assert!(!task.is_finished(), "must not have finished before the gate opens");
    gate.open();
    assert_eq!(task.await.unwrap().unwrap(), "Hello");
}

#[tokio::test]
async fn consecutive_calls_and_prewarm_share_one_connection() {
    let srv = serve(Box::new(|req| {
        if req.method == "HEAD" {
            Reply::Status(404, "", None)
        } else {
            Reply::Stream(vec![Step::Chunk(a_text("ok")), Step::Chunk(a_stop()), Step::End])
        }
    }));
    let base = srv.base();
    prewarm(&base, &ProxyMode::SystemDefault).await;
    for _ in 0..2 {
        let (r, _) = run_collect(&anthropic_call(&base), &opts(5000, 5000)).await;
        assert_eq!(r.unwrap(), "ok");
    }
    assert_eq!(srv.conns.load(Ordering::SeqCst), 1, "HEAD + 2 POSTs must reuse one TCP connection");
    let reqs = srv.requests();
    assert_eq!(reqs[0].method, "HEAD");
    assert_eq!(reqs[0].target, "/");
    assert_eq!(reqs[0].header("x-api-key"), None);
    assert_eq!(reqs[0].header("authorization"), None);
}

#[tokio::test]
async fn prewarm_uses_only_the_origin_root_and_survives_failures() {
    let srv = serve(Box::new(|_| Reply::Status(500, "boom", None)));
    let base = format!("{}/some/path/v1", srv.base());
    prewarm(&base, &ProxyMode::SystemDefault).await; // must not panic or error
    let reqs = srv.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "HEAD");
    assert_eq!(reqs[0].target, "/");
    // unreachable / invalid endpoints are silent no-ops
    prewarm("http://127.0.0.1:1", &ProxyMode::SystemDefault).await;
    prewarm("ftp://nope", &ProxyMode::SystemDefault).await;
}

#[tokio::test]
async fn truncated_stream_is_a_backend_error() {
    let srv = serve(Box::new(|_| {
        Reply::Stream(vec![Step::Chunk(a_start()), Step::Chunk(a_text("partial")), Step::End])
    }));
    let base = srv.base();
    let (r, got) = run_collect(&anthropic_call(&base), &opts(5000, 5000)).await;
    assert!(is_backend(&r), "{r:?}");
    assert_eq!(got, vec!["partial"], "text seen before the truncation is still delivered");
}

#[tokio::test]
async fn error_event_and_bad_stop_reasons_are_backend_errors() {
    let srv = serve(Box::new(|req| {
        let which = String::from_utf8_lossy(&req.body).to_string();
        if which.contains("\"model\":\"err-event\"") {
            Reply::Stream(vec![
                Step::Chunk(a_text("x")),
                Step::Chunk(b"data: {\"type\":\"error\",\"error\":{\"message\":\"overloaded\"}}\n\n".to_vec()),
                Step::End,
            ])
        } else if which.contains("\"model\":\"max-tokens\"") {
            Reply::Stream(vec![
                Step::Chunk(a_text("x")),
                Step::Chunk(b"data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"}}\n\n".to_vec()),
                Step::Chunk(a_stop()),
                Step::End,
            ])
        } else {
            Reply::Stream(vec![Step::Chunk(o_text("x")), Step::Chunk(o_finish("length")), Step::End])
        }
    }));
    let base = srv.base();
    for model in ["err-event", "max-tokens"] {
        let mut c = anthropic_call(&base);
        c.model = model;
        let (r, _) = run_collect(&c, &opts(5000, 5000)).await;
        assert!(is_backend(&r), "{model}: {r:?}");
    }
    let mut c = call(Protocol::OpenAiChat, &base, "k");
    c.model = "openai-length";
    let (r, _) = run_collect(&c, &opts(5000, 5000)).await;
    assert!(is_backend(&r), "{r:?}");
}

#[tokio::test]
async fn non_200_is_a_backend_error_and_the_body_never_leaks() {
    let srv = serve(Box::new(|_| Reply::Status(401, "SECRET-BODY-token-123", None)));
    let base = srv.base();
    let (r, got) = run_collect(&anthropic_call(&base), &opts(5000, 5000)).await;
    assert!(is_backend(&r));
    assert!(got.is_empty());
    assert!(!format!("{r:?}").contains("SECRET"));
}

#[tokio::test]
async fn redirects_are_never_followed() {
    let srv = serve(Box::new(|req| {
        if req.target == "/v1/messages" {
            Reply::Status(302, "", Some("/elsewhere"))
        } else {
            Reply::Stream(vec![Step::Chunk(a_text("evil")), Step::Chunk(a_stop()), Step::End])
        }
    }));
    let base = srv.base();
    let (r, got) = run_collect(&anthropic_call(&base), &opts(5000, 5000)).await;
    assert!(is_backend(&r), "{r:?}");
    assert!(got.is_empty());
    assert_eq!(srv.requests().len(), 1, "the Location target must never be requested");
}

#[tokio::test]
async fn first_text_deadline_is_absolute_and_reports_timeout() {
    let gate = Gate::new();
    let g2 = gate.clone();
    let srv = serve(Box::new(move |_| Reply::Stream(vec![Step::Chunk(a_start()), Step::Wait(g2.clone()), Step::End])));
    let base = srv.base();
    let t0 = Instant::now();
    let (r, _) = run_collect(&anthropic_call(&base), &opts(10_000, 300)).await;
    assert!(matches!(r, Err(ComposeError::Timeout)), "{r:?}");
    assert!(t0.elapsed() < Duration::from_secs(3), "{:?}", t0.elapsed());
    gate.open();
}

#[tokio::test]
async fn overall_deadline_applies_even_after_text_started() {
    let gate = Gate::new();
    let g2 = gate.clone();
    let srv = serve(Box::new(move |_| {
        Reply::Stream(vec![Step::Chunk(a_text("one")), Step::Wait(g2.clone()), Step::Chunk(a_stop()), Step::End])
    }));
    let base = srv.base();
    let t0 = Instant::now();
    let (r, got) = run_collect(&anthropic_call(&base), &opts(400, 5000)).await;
    assert!(matches!(r, Err(ComposeError::Timeout)), "{r:?}");
    assert_eq!(got, vec!["one"]);
    assert!(t0.elapsed() < Duration::from_secs(3));
    gate.open();
}

#[tokio::test]
async fn text_over_the_cap_is_rejected() {
    let big = "x".repeat(8 * 1024);
    let srv = serve(Box::new(move |_| {
        let mut steps = vec![];
        for _ in 0..12 {
            steps.push(Step::Chunk(a_text(&big)));
        }
        steps.push(Step::Chunk(a_stop()));
        steps.push(Step::End);
        Reply::Stream(steps)
    }));
    let base = srv.base();
    let (r, _) = run_collect(&anthropic_call(&base), &opts(5000, 5000)).await;
    assert!(matches!(r, Err(ComposeError::OutputTooLarge)), "{r:?}");
}

#[tokio::test]
async fn dropping_the_future_closes_the_connection() {
    let (ctx, crx) = std::sync::mpsc::channel::<bool>();
    let ctx = Mutex::new(Some(ctx));
    let srv = serve(Box::new(move |_| {
        let tx = ctx.lock().unwrap().take().expect("single request");
        Reply::Stream(vec![Step::Chunk(a_text("first")), Step::ExpectClose(tx)])
    }));
    let base = srv.base();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let task = tokio::spawn(async move {
        let c = anthropic_call(&base);
        let mut cb = |t: &str| {
            let _ = tx.send(t.to_string());
        };
        stream_completion(&c, &opts(20_000, 20_000), &mut cb).await
    });
    assert_eq!(tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap(), "first");
    task.abort();
    let _ = task.await;
    let closed = tokio::task::spawn_blocking(move || crx.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap();
    assert_eq!(closed, Ok(true), "server must observe the client closing the connection");
}

#[tokio::test]
async fn invalid_endpoint_never_touches_the_network() {
    let srv = serve(Box::new(|_| Reply::Status(200, "", None)));
    let mut c = anthropic_call("https://user:pw@example.com");
    let (r, _) = run_collect(&c, &opts(2000, 2000)).await;
    assert!(is_backend(&r));
    c.base_url = "http://example.com"; // plain http, non-loopback, key present
    let (r, _) = run_collect(&c, &opts(2000, 2000)).await;
    assert!(is_backend(&r));
    assert_eq!(srv.requests().len(), 0);
}

// ---------------------------------------------------------------------------
// proxy behaviour (explicit proxy; loopback bypass)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn explicit_proxy_receives_absolute_form_requests_for_remote_targets() {
    let proxy = serve(Box::new(|_| {
        Reply::Stream(vec![Step::Chunk(a_text("via-proxy")), Step::Chunk(a_stop()), Step::End])
    }));
    let o = StreamOpts {
        overall: Duration::from_secs(5),
        first_text: Duration::from_secs(5),
        proxy: ProxyMode::Explicit(proxy.base()),
    };
    // plain http to a non-loopback name is allowed only without a key
    let c = StreamCall { api_key: "", ..call(Protocol::AnthropicMessages, "http://upstream.example.test", "") };
    let (r, _) = run_collect(&c, &o).await;
    assert_eq!(r.unwrap(), "via-proxy");
    let reqs = proxy.requests();
    assert_eq!(reqs.len(), 1);
    assert!(reqs[0].target.starts_with("http://upstream.example.test/"), "{}", reqs[0].target);
}

#[tokio::test]
async fn loopback_targets_never_use_the_proxy() {
    let proxy = serve(Box::new(|_| Reply::Status(500, "", None)));
    let target = serve(Box::new(|_| {
        Reply::Stream(vec![Step::Chunk(a_text("direct")), Step::Chunk(a_stop()), Step::End])
    }));
    let o = StreamOpts {
        overall: Duration::from_secs(5),
        first_text: Duration::from_secs(5),
        proxy: ProxyMode::Explicit(proxy.base()),
    };
    let base = target.base();
    let (r, _) = run_collect(&anthropic_call(&base), &o).await;
    assert_eq!(r.unwrap(), "direct");
    assert_eq!(proxy.conns.load(Ordering::SeqCst), 0, "loopback must bypass the proxy");
    assert_eq!(target.requests().len(), 1);
}
