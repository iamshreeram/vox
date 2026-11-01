//! Verified-commit gate for automatic updates (R6-5).
//!
//! GitHub reports `commit.verification.verified` for a commit. That is a
//! signature check, not a code review. Every failure fails closed: a missing
//! or false field, a rate limit, a redirect, an oversize body, bad JSON or a
//! transport error all mean "not verified". Nothing logs a body or a token.

#![cfg_attr(not(test), allow(dead_code))]

use std::io::Read;
use std::time::Duration;

use serde_json::Value;

use super::source_update::REPO_SLUG;
use super::source_update_auto::VerifiedState;

/// Largest commit body accepted from GitHub, in bytes.
pub const VERIFY_MAX_BYTES: usize = 1024 * 1024;
/// Request timeout, the same as the update check's fetch timeout.
pub const VERIFY_TIMEOUT: Duration = Duration::from_secs(15);
/// GitHub REST API base used in production.
pub const GITHUB_API_BASE: &str = "https://api.github.com";

/// Why a verification could not be decided. Carries no response text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// The body is larger than `VERIFY_MAX_BYTES`.
    TooLarge,
    /// The body is not valid JSON.
    InvalidJson,
    /// The sha is not 40 lowercase hex characters.
    BadSha,
    /// Transport failure, timeout, or unreadable body.
    Network,
    /// A non-200 status, including rate limits (403/429) and redirects (3xx).
    Status(u16),
}

/// Reads `commit.verification.verified` from a GitHub commit body.
///
/// Only a JSON boolean `true` counts as verified. A missing field, `false`,
/// a string such as `"true"`, or `null` all give `Ok(false)`. An oversize body
/// or invalid JSON is an error.
pub fn parse_commit_verification(body: &[u8]) -> Result<bool, VerifyError> {
    if body.len() > VERIFY_MAX_BYTES {
        return Err(VerifyError::TooLarge);
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| VerifyError::InvalidJson)?;
    let verified = value
        .get("commit")
        .and_then(|commit| commit.get("verification"))
        .and_then(|verification| verification.get("verified"))
        .and_then(Value::as_bool);
    Ok(verified == Some(true))
}

/// Fetches and decides whether one commit is verified.
pub trait CommitVerifier {
    /// `Ok(true)` only when GitHub reports the commit as verified.
    fn verify(&self, full_sha: &str) -> Result<bool, VerifyError>;
}

/// Maps a verification result to the gate's tri-state. A sha that is not
/// 40 lowercase hex is never sent to the verifier and counts as failed.
pub fn check_commit(verifier: &dyn CommitVerifier, full_sha: &str) -> VerifiedState {
    if !is_lower_sha(full_sha) {
        return VerifiedState::Failed;
    }
    match verifier.verify(full_sha) {
        Ok(true) => VerifiedState::Verified,
        Ok(false) => VerifiedState::Unverified,
        Err(_) => VerifiedState::Failed,
    }
}

/// `…/repos/<slug>/commits/<sha>` under `api_base`. `None` unless `full_sha`
/// is exactly 40 lowercase hex characters.
pub fn commit_url(api_base: &str, full_sha: &str) -> Option<String> {
    if !is_lower_sha(full_sha) {
        return None;
    }
    Some(format!("{api_base}/repos/{REPO_SLUG}/commits/{full_sha}"))
}

fn is_lower_sha(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Real verifier: one blocking GET per decision, no redirects, no retries.
#[derive(Debug, Clone)]
pub struct GithubCommitVerifier {
    api_base: String,
    timeout: Duration,
}

impl Default for GithubCommitVerifier {
    fn default() -> Self {
        Self {
            api_base: GITHUB_API_BASE.to_string(),
            timeout: VERIFY_TIMEOUT,
        }
    }
}

impl GithubCommitVerifier {
    /// Verifier against a custom API base (tests point this at a local server).
    pub fn with_api_base(api_base: &str, timeout: Duration) -> Self {
        Self {
            api_base: api_base.to_string(),
            timeout,
        }
    }
}

impl CommitVerifier for GithubCommitVerifier {
    fn verify(&self, full_sha: &str) -> Result<bool, VerifyError> {
        let url = commit_url(&self.api_base, full_sha).ok_or(VerifyError::BadSha)?;
        let client = reqwest::blocking::Client::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(format!("vox-update-check/{}", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|_| VerifyError::Network)?;
        let response = client
            .get(url)
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .send()
            .map_err(|_| VerifyError::Network)?;
        let status = response.status().as_u16();
        if status != 200 {
            return Err(VerifyError::Status(status));
        }
        let mut body = Vec::new();
        Read::take(response, VERIFY_MAX_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|_| VerifyError::Network)?;
        parse_commit_verification(&body)
    }
}

#[cfg(test)]
#[path = "source_update_verify_tests.rs"]
mod tests;
