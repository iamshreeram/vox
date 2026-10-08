use super::{ModelDownloadError, ModelDownloader};
use reqwest::blocking::Client;
use std::io::Write;
use std::path::Path;
use std::time::Duration;

const OPEN_WAKE_WORD_RELEASE: &str =
    "https://github.com/dscripka/openWakeWord/releases/download/v0.5.1";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

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
                .build()
                .expect("reqwest blocking client configuration is valid"),
        }
    }
}

impl HttpModelDownloader {
    fn download_url(&self, url: &str, destination: &Path) -> Result<(), ModelDownloadError> {
        let mut response = self
            .client
            .get(url)
            .send()
            .map_err(|error| ModelDownloadError::Network(error.to_string()))?;
        if !response.status().is_success() {
            return Err(ModelDownloadError::Network(format!(
                "HTTP {} from {url}",
                response.status()
            )));
        }
        let mut file = std::fs::File::create(destination)
            .map_err(|error| ModelDownloadError::Io(error.to_string()))?;
        let downloaded = std::io::copy(&mut response, &mut file)
            .map_err(|error| ModelDownloadError::Network(error.to_string()))?;
        if downloaded == 0 {
            return Err(ModelDownloadError::Network(format!(
                "HTTP response from {url} contained an empty model file"
            )));
        }
        file.flush()
            .map_err(|error| ModelDownloadError::Io(error.to_string()))?;
        Ok(())
    }

    fn url_for(model_name: &str, file_name: &str) -> Result<String, ModelDownloadError> {
        let upstream = upstream_filename(model_name, file_name)?;
        Ok(format!("{OPEN_WAKE_WORD_RELEASE}/{upstream}"))
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
        self.download_url(&url, destination)
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
