use super::{ModelDownloadError, ModelDownloader};
use reqwest::blocking::Client;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const OPEN_WAKE_WORD_RELEASE: &str =
    "https://github.com/dscripka/openWakeWord/releases/download/v0.5.1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
// `.timeout()` alone bounds the whole request, but observed in practice
// (behind a corporate HTTPS-proxy CONNECT tunnel) it does not reliably fire
// if the proxy accepts the TCP connection but never completes the CONNECT
// handshake to the real destination -- the request hangs well past
// REQUEST_TIMEOUT with no error. `.connect_timeout()` bounds that specific
// phase independently and does fire, so this is a deliberate belt-and-braces
// pairing, not redundant with REQUEST_TIMEOUT above.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Resolves this repo's public model name to its upstream openWakeWord release filename.
pub fn upstream_filename(
    model_name: &str,
    file_name: &str,
) -> Result<&'static str, ModelDownloadError> {
    match (model_name, file_name) {
        ("hey_jarvis", "melspectrogram.onnx") => Ok("melspectrogram.onnx"),
        ("hey_jarvis", "embedding_model.onnx") => Ok("embedding_model.onnx"),
        ("hey_jarvis", "classifier.onnx") => Ok("hey_jarvis_v0.1.onnx"),
        (name, _) if name != "hey_jarvis" => Err(ModelDownloadError::Network(format!(
            "Unsupported wake-word model '{name}'"
        ))),
        (_, file) => Err(ModelDownloadError::Network(format!(
            "Unsupported file '{file}' for wake-word model 'hey_jarvis'"
        ))),
    }
}

/// Synchronous GitHub-release downloader for the dedicated wake-word thread.
pub struct HttpModelDownloader {
    client: Client,
}

impl Default for HttpModelDownloader {
    fn default() -> Self {
        Self {
            client: Client::builder()
                .timeout(REQUEST_TIMEOUT)
                .connect_timeout(CONNECT_TIMEOUT)
                .build()
                .expect("reqwest blocking client configuration is valid"),
        }
    }
}

impl HttpModelDownloader {
    /// Single-attempt download -- unchanged in behavior from before this
    /// file's proxy-fallback addition. Every pre-existing test below calls
    /// this directly and must keep passing identically.
    fn download_url(&self, url: &str, destination: &Path) -> Result<(), ModelDownloadError> {
        let mut response = self
            .client
            .get(url)
            .send()
            .map_err(|error| ModelDownloadError::Network(format!("{error:?}")))?;
        if !response.status().is_success() {
            return Err(ModelDownloadError::Network(format!(
                "HTTP {} from {url}",
                response.status()
            )));
        }
        let mut file = std::fs::File::create(destination)
            .map_err(|error| ModelDownloadError::Io(error.to_string()))?;
        let downloaded = std::io::copy(&mut response, &mut file)
            .map_err(|error| ModelDownloadError::Network(format!("{error:?}")))?;
        if downloaded == 0 {
            return Err(ModelDownloadError::Network(format!(
                "HTTP response from {url} contained an empty model file"
            )));
        }
        file.flush()
            .map_err(|error| ModelDownloadError::Io(error.to_string()))?;
        Ok(())
    }

    /// GitHub Releases always 302-redirects asset downloads to a separate
    /// CDN host (`release-assets.githubusercontent.com` at time of
    /// writing). Confirmed on a real affected machine: that CDN host can be
    /// blocked outright by a network that otherwise allowlists `github.com`
    /// itself -- the TCP handshake completes, then the TLS/HTTP exchange
    /// simply times out, consistent with SNI-based filtering rather than a
    /// DNS or routing failure. On that same machine, the OS's own
    /// configured HTTP(S) proxy reaches the exact same CDN host
    /// successfully even though a direct connection cannot. Try direct
    /// first (correct and sufficient on most networks, and actively
    /// *required* on others -- see install-macos.sh's own history of a
    /// misconfigured proxy breaking otherwise-working direct access), then
    /// fall back to an explicit proxy client built from whatever standard
    /// proxy env var is already present in this process's own environment,
    /// if any, before giving up. Takes a URL directly (rather than
    /// model/file names) so it's independently testable against a local
    /// stub server without any real network dependency.
    fn download_with_proxy_fallback(
        &self,
        url: &str,
        destination: &Path,
    ) -> Result<(), ModelDownloadError> {
        let direct_error = match self.download_url(url, destination) {
            Ok(()) => return Ok(()),
            Err(error) => error,
        };

        let Some(proxy_url) = Self::configured_proxy_url() else {
            return Err(direct_error);
        };
        let Ok(proxy) = reqwest::Proxy::all(&proxy_url) else {
            return Err(direct_error);
        };
        let Ok(proxy_client) = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .proxy(proxy)
            .build()
        else {
            return Err(direct_error);
        };

        log::warn!(
            "Direct download of {url} failed ({direct_error:?}); retrying via the configured proxy"
        );
        let fallback = HttpModelDownloader {
            client: proxy_client,
        };
        fallback.download_url(url, destination).map_err(|proxy_error| {
            ModelDownloadError::Network(format!(
                "direct attempt failed ({direct_error:?}); proxy-fallback attempt also failed ({proxy_error:?})"
            ))
        })
    }

    fn url_for(model_name: &str, file_name: &str) -> Result<String, ModelDownloadError> {
        let upstream = upstream_filename(model_name, file_name)?;
        Ok(format!("{OPEN_WAKE_WORD_RELEASE}/{upstream}"))
    }

    /// Reads a standard proxy env var from this process's own environment,
    /// if any is set. Deliberately generic -- this must never hardcode any
    /// specific proxy hostname, so it works for any user on any network
    /// that needs a proxy, not one specific employer's infrastructure.
    fn configured_proxy_url() -> Option<String> {
        std::env::var("HTTPS_PROXY")
            .or_else(|_| std::env::var("https_proxy"))
            .or_else(|_| std::env::var("HTTP_PROXY"))
            .or_else(|_| std::env::var("http_proxy"))
            .ok()
            .filter(|value| !value.is_empty())
    }
}

impl ModelDownloader for HttpModelDownloader {
    fn download_file(
        &self,
        model_name: &str,
        file_name: &str,
        destination: &Path,
    ) -> Result<(), ModelDownloadError> {
        let url = Self::url_for(model_name, file_name)?;
        self.download_with_proxy_fallback(&url, destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn server(response: &'static [u8]) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 1024];
            let _ = stream.read(&mut request);
            stream.write_all(response).unwrap();
        });
        format!("http://{address}/asset")
    }

    #[test]
    fn successful_download_writes_expected_bytes() {
        let url = server(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello");
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("asset");
        HttpModelDownloader::default()
            .download_url(&url, &destination)
            .unwrap();
        assert_eq!(std::fs::read(destination).unwrap(), b"hello");
    }

    #[test]
    fn non_success_status_is_network_error_and_does_not_create_file() {
        let url = server(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 4\r\nConnection: close\r\n\r\nnope");
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("asset");
        let error = HttpModelDownloader::default()
            .download_url(&url, &destination)
            .unwrap_err();
        assert!(
            matches!(error, ModelDownloadError::Network(ref message) if message.contains("503"))
        );
        assert!(!destination.exists());
    }

    #[test]
    fn empty_success_body_is_not_silently_accepted() {
        let url = server(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("asset");
        let error = HttpModelDownloader::default()
            .download_url(&url, &destination)
            .unwrap_err();
        assert!(
            matches!(error, ModelDownloadError::Network(ref message) if message.contains("empty"))
        );
    }

    #[test]
    fn connection_failure_is_network_error() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        drop(listener);
        let error = HttpModelDownloader::default()
            .download_url(&format!("http://{address}/"), Path::new("unused"))
            .unwrap_err();
        assert!(matches!(error, ModelDownloadError::Network(_)));
    }

    // `configured_proxy_url()`/`download_with_proxy_fallback()` both read
    // process-global env vars, so every scenario depending on their state
    // lives in this ONE serialized test rather than separate #[test]
    // functions -- Rust runs tests in parallel threads within the same
    // process by default, and separate tests mutating the same global env
    // var race each other (confirmed: an earlier version of this file had
    // exactly this flake, failing nondeterministically depending on
    // scheduling AND on whatever real proxy vars happened to already be set
    // in the ambient shell running `cargo test`). The real env var state is
    // captured up front and always restored before any assertion runs, so a
    // failing assertion still can't leave the ambient environment mutated
    // for whatever other test happens to run next.
    #[test]
    fn download_with_proxy_fallback_covers_both_branches() {
        let proxy_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let proxy_address = proxy_listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in proxy_listener.incoming() {
                let mut stream = stream.unwrap();
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nhello",
                );
            }
        });
        // Closed local ports: guaranteed to fail the "direct" attempt fast,
        // with zero dependency on any real external network.
        let closed_listener_a = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_address_a = closed_listener_a.local_addr().unwrap();
        drop(closed_listener_a);
        let closed_listener_b = TcpListener::bind("127.0.0.1:0").unwrap();
        let closed_address_b = closed_listener_b.local_addr().unwrap();
        drop(closed_listener_b);

        let previous = ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
            .map(|key| (key, std::env::var(key).ok()));

        // Scenario 1: no proxy configured at all -> the direct error must
        // pass straight through unchanged, no fallback attempted.
        for (key, _) in &previous {
            std::env::remove_var(key);
        }
        let dir_a = tempfile::tempdir().unwrap();
        let destination_a = dir_a.path().join("asset");
        let no_proxy_result = HttpModelDownloader::default().download_with_proxy_fallback(
            &format!("http://{closed_address_a}/direct-should-fail"),
            &destination_a,
        );

        // Scenario 2: a proxy IS configured -> fallback must retry through
        // it and succeed, even though direct access fails.
        std::env::set_var("HTTPS_PROXY", format!("http://{proxy_address}"));
        let dir_b = tempfile::tempdir().unwrap();
        let destination_b = dir_b.path().join("asset");
        let with_proxy_result = HttpModelDownloader::default().download_with_proxy_fallback(
            &format!("http://{closed_address_b}/direct-should-fail"),
            &destination_b,
        );
        let with_proxy_bytes = std::fs::read(&destination_b);

        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }

        assert!(matches!(
            no_proxy_result,
            Err(ModelDownloadError::Network(_))
        ));
        assert!(
            with_proxy_result.is_ok(),
            "expected proxy fallback to succeed, got {with_proxy_result:?}"
        );
        assert_eq!(with_proxy_bytes.unwrap(), b"hello");
    }

    // Manual diagnostic only -- hits the real network, never run in CI/`cargo test`.
    // `cargo test --lib downloader::tests::real_download_against_live_github_release -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_download_against_live_github_release() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("melspectrogram.onnx");
        let result = HttpModelDownloader::default().download_file(
            "hey_jarvis",
            "melspectrogram.onnx",
            &destination,
        );
        eprintln!("result: {result:?}");
        result.unwrap();
        let size = std::fs::metadata(&destination).unwrap().len();
        eprintln!("downloaded {size} bytes");
        assert!(size > 0);
    }

    #[test]
    fn model_and_file_url_lookup_is_explicit() {
        assert_eq!(
            HttpModelDownloader::url_for("hey_jarvis", "classifier.onnx").unwrap(),
            format!("{OPEN_WAKE_WORD_RELEASE}/hey_jarvis_v0.1.onnx")
        );
        assert_eq!(
            HttpModelDownloader::url_for("hey_jarvis", "melspectrogram.onnx").unwrap(),
            format!("{OPEN_WAKE_WORD_RELEASE}/melspectrogram.onnx")
        );
        assert!(matches!(
            HttpModelDownloader::url_for("unknown", "classifier.onnx"),
            Err(ModelDownloadError::Network(message)) if message.contains("Unsupported wake-word model 'unknown'")
        ));
    }
}
