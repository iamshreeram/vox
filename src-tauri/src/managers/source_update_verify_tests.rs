use std::cell::Cell;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::time::Duration;

use super::*;

const SHA: &str = "0123456789abcdef0123456789abcdef01234567";

struct Fake(Result<bool, VerifyError>, Cell<u32>);

impl CommitVerifier for Fake {
    fn verify(&self, _full_sha: &str) -> Result<bool, VerifyError> {
        self.1.set(self.1.get() + 1);
        self.0
    }
}

#[test]
fn only_a_json_true_counts_as_verified() {
    let yes = br#"{"commit":{"verification":{"verified":true,"reason":"valid"}}}"#;
    assert_eq!(parse_commit_verification(yes), Ok(true));
}

#[test]
fn every_other_shape_is_not_verified() {
    let cases: [&[u8]; 6] = [
        br#"{"commit":{"verification":{"verified":false}}}"#,
        br#"{"commit":{"verification":{}}}"#,
        br#"{"commit":{}}"#,
        br#"{"commit":{"verification":{"verified":"true"}}}"#,
        br#"{"commit":{"verification":{"verified":null}}}"#,
        br#"[]"#,
    ];
    for body in cases {
        assert_eq!(parse_commit_verification(body), Ok(false), "{body:?}");
    }
}

#[test]
fn garbage_and_oversize_bodies_are_errors() {
    assert_eq!(
        parse_commit_verification(b"<html>rate limited</html>"),
        Err(VerifyError::InvalidJson)
    );
    let huge = vec![b' '; VERIFY_MAX_BYTES + 1];
    assert_eq!(parse_commit_verification(&huge), Err(VerifyError::TooLarge));
}

#[test]
fn commit_url_is_built_only_from_a_lowercase_full_sha() {
    assert_eq!(
        commit_url(GITHUB_API_BASE, SHA).unwrap(),
        format!("https://api.github.com/repos/iamshreeram/vox/commits/{SHA}")
    );
    assert!(commit_url(GITHUB_API_BASE, &SHA.to_uppercase()).is_none());
    assert!(commit_url(GITHUB_API_BASE, &SHA[..39]).is_none());
    assert!(commit_url(GITHUB_API_BASE, "../../orgs/x").is_none());
}

#[test]
fn check_commit_maps_every_verifier_answer() {
    let verified = Fake(Ok(true), Cell::new(0));
    assert_eq!(check_commit(&verified, SHA), VerifiedState::Verified);
    let unverified = Fake(Ok(false), Cell::new(0));
    assert_eq!(check_commit(&unverified, SHA), VerifiedState::Unverified);
    let broken = Fake(Err(VerifyError::Status(403)), Cell::new(0));
    assert_eq!(check_commit(&broken, SHA), VerifiedState::Failed);
}

#[test]
fn bad_sha_fails_closed_without_calling_the_verifier() {
    let fake = Fake(Ok(true), Cell::new(0));
    assert_eq!(check_commit(&fake, "main"), VerifiedState::Failed);
    assert_eq!(
        check_commit(&fake, &SHA.to_uppercase()),
        VerifiedState::Failed
    );
    assert_eq!(fake.1.get(), 0);
}

/// Serves one canned HTTP response on localhost and returns the base URL.
fn serve_once(response: Vec<u8>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0u8; 4096];
            let _ = stream.read(&mut request);
            let _ = stream.write_all(&response);
        }
    });
    format!("http://{addr}")
}

fn http(status_line: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

fn verifier_for(base: &str) -> GithubCommitVerifier {
    GithubCommitVerifier::with_api_base(base, Duration::from_secs(5))
}

#[test]
fn live_200_with_verified_true_is_verified() {
    let body = br#"{"commit":{"verification":{"verified":true}}}"#;
    let base = serve_once(http("200 OK", body));
    assert_eq!(verifier_for(&base).verify(SHA), Ok(true));
}

#[test]
fn live_403_rate_limit_fails_closed() {
    let base = serve_once(http("403 Forbidden", b"{}"));
    assert_eq!(
        verifier_for(&base).verify(SHA),
        Err(VerifyError::Status(403))
    );
}

#[test]
fn live_redirect_is_never_followed() {
    let raw = b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec();
    let base = serve_once(raw);
    assert_eq!(
        verifier_for(&base).verify(SHA),
        Err(VerifyError::Status(302))
    );
}

#[test]
fn live_oversize_body_is_rejected() {
    let body = vec![b' '; VERIFY_MAX_BYTES + 16];
    let base = serve_once(http("200 OK", &body));
    assert_eq!(verifier_for(&base).verify(SHA), Err(VerifyError::TooLarge));
}

#[test]
fn live_transport_failure_fails_closed() {
    let base = "http://127.0.0.1:1";
    assert_eq!(verifier_for(base).verify(SHA), Err(VerifyError::Network));
}
