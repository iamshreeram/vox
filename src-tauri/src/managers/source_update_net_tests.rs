//! Spec for the NET (#190) runtime direct-retry fallback in `managers::source_update`
//! (DESIGN.md "### NET (#190)" N1-N2, "Acceptance (NET)", R3-4, R3-5).
//!
//! Authoritative: do not weaken an assertion; if a test is wrong, report it instead of
//! editing it. Local sockets on 127.0.0.1 only (no real network). These tests never read
//! or mutate process environment: `proxy_env_configured` is driven by a closure, and the
//! proxy is either a fixture listener or a bound-then-dropped ("dead") port.

use crate::managers::source_update::{
    describe_failure, fetch_text_inner, proxy_env_configured, should_retry_direct, FailKind,
    ProxyChoice, UpdateCheckError,
};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const TIMEOUT: Duration = Duration::from_secs(1);
const BIG: usize = 1 << 20;

// ---------------------------------------------------------------- fixtures

/// A hand-written loopback HTTP server that replies with fixed bytes to every connection.
struct Fixture {
    port: u16,
    /// Accepted TCP connections.
    conns: Arc<AtomicU32>,
    /// Complete request heads received (ends with CRLFCRLF).
    requests: Arc<AtomicU32>,
    /// Complete requests whose method is CONNECT.
    connects: Arc<AtomicU32>,
}

impl Fixture {
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    fn https_url(&self, path: &str) -> String {
        format!("https://127.0.0.1:{}{}", self.port, path)
    }

    fn explicit_proxy(&self) -> ProxyChoice {
        ProxyChoice::Explicit(format!("http://127.0.0.1:{}", self.port))
    }
}

fn ok_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

fn status_response(code: u16, reason: &str) -> Vec<u8> {
    format!("HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
        .into_bytes()
}

async fn read_head(sock: &mut TcpStream) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") && buf.len() < 16 * 1024 {
        match sock.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    buf
}

async fn serve(reply: Vec<u8>) -> Fixture {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fixture");
    let port = listener.local_addr().expect("fixture addr").port();
    let conns = Arc::new(AtomicU32::new(0));
    let requests = Arc::new(AtomicU32::new(0));
    let connects = Arc::new(AtomicU32::new(0));
    let (c_conns, c_requests, c_connects) = (conns.clone(), requests.clone(), connects.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            c_conns.fetch_add(1, Ordering::SeqCst);
            let (requests, connects, reply) =
                (c_requests.clone(), c_connects.clone(), reply.clone());
            tokio::spawn(async move {
                let head = read_head(&mut sock).await;
                if head.windows(4).any(|w| w == b"\r\n\r\n") {
                    requests.fetch_add(1, Ordering::SeqCst);
                    if head.starts_with(b"CONNECT ") {
                        connects.fetch_add(1, Ordering::SeqCst);
                    }
                }
                let _ = sock.write_all(&reply).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    Fixture {
        port,
        conns,
        requests,
        connects,
    }
}

/// A loopback port that nothing listens on (bound, then dropped).
async fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind for dead port");
    let port = listener.local_addr().expect("dead addr").port();
    drop(listener);
    port
}

fn count(c: &Arc<AtomicU32>) -> u32 {
    c.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------- fetch behaviour

/// (a) Key red test: dead configured proxy + reachable direct target.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_proxy_with_reachable_direct_target_succeeds_after_one_direct_retry() {
    let target = serve(ok_response("commits-json")).await;
    let dead = dead_port().await;
    let proxy = ProxyChoice::Explicit(format!("http://127.0.0.1:{dead}"));

    let (res, attempts) =
        fetch_text_inner(&target.url("/repos/x/commits"), BIG, TIMEOUT, proxy, true).await;

    assert_eq!(res, Ok((200, "commits-json".to_string())));
    assert_eq!(attempts, 2, "one proxied attempt, then one direct retry");
    assert_eq!(
        count(&target.requests),
        1,
        "target must be hit exactly once"
    );
}

/// (b1) Direct to a dead target, nothing configured: no retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn direct_to_dead_target_is_network_error_with_no_retry() {
    let dead = dead_port().await;
    let url = format!("http://127.0.0.1:{dead}/repos/x/commits");

    let (res, attempts) = fetch_text_inner(&url, BIG, TIMEOUT, ProxyChoice::Direct, false).await;

    assert!(
        matches!(res, Err(UpdateCheckError::Network(_))),
        "expected Network, got {res:?}"
    );
    assert_eq!(attempts, 1);
}

/// (b2) Explicit dead proxy but no proxy variable configured: no retry, target untouched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_proxy_without_configured_proxy_does_not_retry() {
    let target = serve(ok_response("unused")).await;
    let dead = dead_port().await;
    let proxy = ProxyChoice::Explicit(format!("http://127.0.0.1:{dead}"));

    let (res, attempts) =
        fetch_text_inner(&target.url("/repos/x/commits"), BIG, TIMEOUT, proxy, false).await;

    assert!(
        matches!(res, Err(UpdateCheckError::Network(_))),
        "expected Network, got {res:?}"
    );
    assert_eq!(attempts, 1, "no retry when no proxy variable is configured");
    assert_eq!(
        count(&target.requests),
        0,
        "direct target must not be contacted"
    );
}

/// (c) A working proxy that answers 500 is a real HTTP answer: returned, never retried.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn working_proxy_http_500_is_returned_without_direct_request() {
    let proxy = serve(status_response(500, "Internal Server Error")).await;
    let target = serve(ok_response("direct")).await;

    let (res, attempts) = fetch_text_inner(
        &target.url("/repos/x/commits"),
        BIG,
        TIMEOUT,
        proxy.explicit_proxy(),
        true,
    )
    .await;

    assert_eq!(res, Ok((500, String::new())));
    assert_eq!(
        attempts, 1,
        "an HTTP status from a live proxy is not retried"
    );
    assert_eq!(
        count(&target.requests),
        0,
        "direct target must never be contacted"
    );
    assert_eq!(count(&proxy.requests), 1);
}

/// (d) Both proxy and target dead: exactly two attempts, bounded time.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn both_dead_fails_after_exactly_two_attempts_within_bounded_time() {
    let dead_proxy = dead_port().await;
    let dead_target = dead_port().await;
    let proxy = ProxyChoice::Explicit(format!("http://127.0.0.1:{dead_proxy}"));
    let url = format!("http://127.0.0.1:{dead_target}/repos/x/commits");

    let started = Instant::now();
    let (res, attempts) = fetch_text_inner(&url, BIG, TIMEOUT, proxy, true).await;
    let elapsed = started.elapsed();

    assert!(
        matches!(res, Err(UpdateCheckError::Network(_))),
        "expected Network, got {res:?}"
    );
    assert_eq!(attempts, 2);
    assert!(
        elapsed < TIMEOUT * 2 + Duration::from_secs(2),
        "took {elapsed:?}, expected under 2*timeout + 2s"
    );
}

/// (e1) Body larger than max_bytes: TooLarge, never retried.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn body_larger_than_limit_is_too_large_without_retry() {
    let target = serve(ok_response(&"x".repeat(100))).await;

    let (res, attempts) = fetch_text_inner(
        &target.url("/repos/x/commits"),
        10,
        TIMEOUT,
        ProxyChoice::Direct,
        true,
    )
    .await;

    assert_eq!(res, Err(UpdateCheckError::TooLarge));
    assert_eq!(attempts, 1);
}

/// (e2) Headers arrive, then the connection closes mid-body: no retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connection_closed_mid_body_is_not_retried() {
    let truncated =
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n0123456789".to_vec();
    let target = serve(truncated).await;

    let (res, attempts) = fetch_text_inner(
        &target.url("/repos/x/commits"),
        BIG,
        TIMEOUT,
        ProxyChoice::Direct,
        true,
    )
    .await;

    assert!(
        matches!(res, Err(UpdateCheckError::Network(_))),
        "expected a transport error for the truncated body, got {res:?}"
    );
    assert_eq!(
        attempts, 1,
        "a failure after headers is not a connect/timeout failure"
    );
}

/// (f) Proxy refuses the CONNECT tunnel (403) for an https target: one retry direct.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refused_connect_tunnel_retries_direct_once() {
    let proxy = serve(status_response(403, "Forbidden")).await;
    let direct = serve(ok_response("direct")).await;

    let (res, attempts) = fetch_text_inner(
        &direct.https_url("/repos/x/commits"),
        BIG,
        TIMEOUT,
        proxy.explicit_proxy(),
        true,
    )
    .await;

    assert!(
        res.is_err(),
        "the direct leg is plain TCP, so the TLS call cannot succeed"
    );
    assert_eq!(
        attempts, 2,
        "tunnel refusal is a connect failure: one direct retry"
    );
    assert_eq!(
        count(&proxy.connects),
        1,
        "proxy must see exactly one CONNECT"
    );
    assert_eq!(
        count(&direct.conns),
        1,
        "direct listener must see exactly one connection"
    );
}

/// (g) An explicit proxy intercepts a 127.0.0.1 target: the proxy serves it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn explicit_proxy_intercepts_loopback_target() {
    let proxy = serve(ok_response("via-proxy")).await;
    let target = serve(ok_response("direct")).await;

    let (res, attempts) = fetch_text_inner(
        &target.url("/repos/x/commits"),
        BIG,
        TIMEOUT,
        proxy.explicit_proxy(),
        true,
    )
    .await;

    assert_eq!(res, Ok((200, "via-proxy".to_string())));
    assert_eq!(attempts, 1);
    assert_eq!(
        count(&proxy.requests),
        1,
        "proxy listener must see the request"
    );
    assert_eq!(count(&target.requests), 0);
}

// ---------------------------------------------------------------- env detection

#[test]
fn proxy_env_none_set_is_not_configured() {
    assert!(!proxy_env_configured(|_| None));
}

#[test]
fn proxy_env_blank_values_are_not_configured() {
    let names = ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"];
    for blank in ["", "   ", " \t "] {
        for name in names {
            let get = |k: &str| (k == name).then(|| blank.to_string());
            assert!(
                !proxy_env_configured(get),
                "{name}={blank:?} must not count as configured"
            );
        }
    }
}

#[test]
fn proxy_env_each_of_the_four_names_alone_is_configured() {
    for name in ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"] {
        let get = |k: &str| (k == name).then(|| "http://proxy.invalid:8080".to_string());
        assert!(
            proxy_env_configured(get),
            "{name} alone must count as configured"
        );
    }
}

#[test]
fn proxy_env_mixed_case_names_are_not_among_the_four() {
    // Only the four exact names listed in N1 count; other spellings do not.
    for name in ["Https_Proxy", "HTTPS_proxy", "All_Proxy"] {
        let get = |k: &str| (k == name).then(|| "http://proxy.invalid:8080".to_string());
        assert!(
            !proxy_env_configured(get),
            "{name} is not one of the four names"
        );
    }
}

// ---------------------------------------------------------------- retry decision

#[test]
fn should_retry_direct_truth_table() {
    let kinds = || {
        [
            FailKind::Connect,
            FailKind::Timeout,
            FailKind::Status,
            FailKind::TooLarge,
            FailKind::Body,
            FailKind::Other,
        ]
    };
    for (i, _) in kinds().iter().enumerate() {
        for configured in [false, true] {
            for already in [false, true] {
                let kind = kinds().into_iter().nth(i).expect("kind");
                let retriable = matches!(i, 0 | 1);
                let want = retriable && configured && !already;
                let got = should_retry_direct(kind, configured, already);
                assert_eq!(
                    got, want,
                    "kind#{i} configured={configured} already_retried={already}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------- diagnostics (N2, R3-4)

#[test]
fn describe_failure_is_exact_and_content_free_for_every_class() {
    let cases: Vec<(UpdateCheckError, &str)> = vec![
        (UpdateCheckError::Network("x".into()), "network"),
        (UpdateCheckError::RateLimited, "rate_limited"),
        (UpdateCheckError::Http(500), "http"),
        (UpdateCheckError::Malformed("bad".into()), "malformed"),
        (UpdateCheckError::EmptyHistory, "empty"),
        (UpdateCheckError::BadBuildCommit, "bad_build"),
        (UpdateCheckError::TooLarge, "too_large"),
    ];
    for (err, class) in cases {
        assert_eq!(
            describe_failure(&err, true, false),
            format!("source update check failed: class={class} proxy_configured=true retried_direct=false")
        );
        assert_eq!(
            describe_failure(&err, false, true),
            format!("source update check failed: class={class} proxy_configured=false retried_direct=true")
        );
    }
}

#[test]
fn describe_failure_never_leaks_payloads_or_hosts() {
    let network =
        UpdateCheckError::Network("http://user:pw@secret-proxy.invalid 127.0.0.1:9 secret".into());
    let malformed = UpdateCheckError::Malformed("...secret... api.github.com".into());
    for err in [network, malformed] {
        let line = describe_failure(&err, true, true);
        for leak in [
            "secret",
            "user",
            "pw",
            "proxy.invalid",
            "127.0.0.1",
            "http",
            "github",
        ] {
            assert!(!line.contains(leak), "log line leaked {leak:?}: {line}");
        }
    }
}
