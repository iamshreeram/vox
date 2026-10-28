//! The Brain: one OpenAI-compatible connection that features can inherit (P3, DESIGN Rev 3-5).
//!
//! Each feature (composer reply style, real-time stream, post-processing) either inherits this
//! connection or keeps its own legacy settings. `resolve` decides which, `effective_provider`
//! returns what a reader should call, and `migrate_with_backup` runs once at settings load.
//! Nothing here logs tokens, userinfo, query strings or conversation content.

use crate::managers::composer_config::{TtsComposerMode, SUPPORTED_PROVIDER_IDS};
use crate::settings::{AppSettings, BrainSource, PostProcessProvider};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::fmt;
use std::io::{ErrorKind, Write};
use std::path::Path;
use std::time::{Duration, Instant};
use url::{Host, Url};

/// File name of the one-time pre-migration backup, written next to the settings store.
pub const BACKUP_FILE_NAME: &str = "settings_store.pre-brain-v1.json";

/// The features that can inherit the Brain.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum BrainFeature {
    /// The speech composer (reply style).
    Composer,
    /// The real-time conversation stream.
    RealtimeStream,
    /// LLM post-processing of dictation.
    PostProcess,
}

/// Why a feature that inherits the Brain cannot run yet. The first missing piece wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrainMissing {
    /// No Brain preset is chosen.
    Provider,
    /// The endpoint address is empty or not allowed.
    BaseUrl,
    /// The model name is empty.
    Model,
    /// A remote endpoint has no access token.
    Token,
    /// The user has not confirmed that transcript text may go to this destination.
    Consent,
}

/// Why a Brain endpoint address is not allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrainUrlError {
    /// The address is empty or blank.
    Empty,
    /// The address does not parse as a URL.
    Malformed,
    /// The scheme is neither https nor http.
    Scheme,
    /// The address contains a user name or password.
    Userinfo,
    /// Plain http is allowed only for a loopback host.
    PlainHttp,
}

/// A resolved Brain connection. The token is never printed by `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct BrainConnection {
    /// Brain preset id.
    pub provider_id: String,
    /// Endpoint base URL as stored (trimmed).
    pub base_url: String,
    /// Model name (trimmed).
    pub model: String,
    /// Access token. Empty for a loopback endpoint that needs none.
    pub token: String,
}

impl fmt::Debug for BrainConnection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BrainConnection")
            .field("provider_id", &self.provider_id)
            .field("base_url", &redact_url(&self.base_url))
            .field("model", &self.model)
            .field("token", &"[REDACTED]")
            .finish()
    }
}

/// What a feature should use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolved {
    /// The feature's own legacy settings.
    Own,
    /// The Brain connection.
    Brain(BrainConnection),
    /// The feature inherits the Brain, but a piece is missing.
    NotConfigured(BrainMissing),
}

/// The provider, key and model a reader should call.
pub struct EffectiveLlm {
    /// The provider to call.
    pub provider: PostProcessProvider,
    /// The API key or access token to send.
    pub api_key: String,
    /// The model name to request.
    pub model: String,
}

/// Outcome of the one-time Brain migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// A fresh install: the marker is set and every feature inherits the Brain.
    Fresh,
    /// An existing store: the marker is set and every feature keeps its own settings.
    Migrated,
    /// The marker was already set, so nothing changed.
    AlreadyDone,
    /// The backup could not be written. Settings are untouched.
    BackupFailed,
}

/// Outcome of a Brain connection test. Carries no server text and no secrets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Type)]
pub enum BrainTestKind {
    /// The endpoint answered and the reply was usable.
    Ok,
    /// The endpoint rejected the token (401 or 403).
    Unauthorized,
    /// The endpoint path was not found (404).
    NotFound,
    /// The endpoint could not be reached.
    Unreachable,
    /// The endpoint redirected. Redirects are never followed.
    Redirect,
    /// The reply was not in the expected shape.
    BadResponse,
    /// The endpoint did not answer in time.
    Timeout,
    /// The endpoint address is not allowed.
    InvalidUrl,
}

/// Result of a Brain connection test.
#[derive(Debug, Clone, PartialEq, Serialize, Type)]
pub struct BrainTestResult {
    /// True only when `kind` is `Ok`.
    pub ok: bool,
    /// What happened.
    pub kind: BrainTestKind,
    /// Round-trip time in milliseconds.
    pub latency_ms: u64,
    /// Number of models the endpoint listed. `None` for a chat test.
    pub model_count: Option<usize>,
}

/// Brain presets. Anything else (for example `anthropic`) is not a Brain provider.
fn is_preset(provider_id: &str) -> bool {
    SUPPORTED_PROVIDER_IDS.contains(&provider_id)
}

/// True when the URL's host is this computer (localhost, 127.0.0.1 or ::1).
fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// Checks a Brain endpoint address: https to any host, http to loopback only, no userinfo,
/// no other scheme.
pub fn validate_base_url(raw: &str) -> Result<(), BrainUrlError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(BrainUrlError::Empty);
    }
    let url = Url::parse(trimmed).map_err(|_| BrainUrlError::Malformed)?;
    if !url.username().is_empty() || url.password().is_some() {
        return Err(BrainUrlError::Userinfo);
    }
    match url.scheme() {
        "https" => Ok(()),
        "http" if is_loopback_url(&url) => Ok(()),
        "http" => Err(BrainUrlError::PlainHttp),
        _ => Err(BrainUrlError::Scheme),
    }
}

/// The user-facing text for an endpoint address error. Never includes the address itself.
pub fn url_error_message(error: BrainUrlError) -> &'static str {
    match error {
        BrainUrlError::Empty => "Enter the endpoint address on the Brain page.",
        BrainUrlError::Malformed => "The endpoint address is not a valid URL.",
        BrainUrlError::Scheme => {
            "The endpoint must start with https://, or http:// for this computer."
        }
        BrainUrlError::Userinfo => "Remove the user name and password from the endpoint address.",
        BrainUrlError::PlainHttp => {
            "Plain http is only allowed for this computer (localhost). Use https for other hosts."
        }
    }
}

/// The normalised destination `provider_id|scheme://host[:port]path`. The path loses its trailing
/// slash. Userinfo, query and fragment are dropped, so no secret can appear in it.
pub fn destination(provider_id: &str, base_url: &str) -> String {
    match Url::parse(base_url.trim()) {
        Ok(url) => {
            let port = url.port().map(|p| format!(":{p}")).unwrap_or_default();
            let host = url.host_str().unwrap_or_default();
            let path = url.path().trim_end_matches('/');
            format!("{provider_id}|{}://{host}{port}{path}", url.scheme())
        }
        Err(_) => format!("{provider_id}|invalid"),
    }
}

/// The URL without userinfo, query or fragment, for display. Unparseable input is replaced.
pub fn redact_url(raw: &str) -> String {
    match Url::parse(raw.trim()) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => "[invalid URL]".to_string(),
    }
}

/// The user-facing text for a missing piece. Every text points at the Brain page.
pub fn missing_message(missing: &BrainMissing) -> &'static str {
    match missing {
        BrainMissing::Provider => "Choose a provider on the Brain page first.",
        BrainMissing::BaseUrl => "Open the Brain page and set the endpoint.",
        BrainMissing::Model => "Choose a model on the Brain page first.",
        BrainMissing::Token => "Add an access token on the Brain page for this remote endpoint.",
        BrainMissing::Consent => {
            "Confirm on the Brain page that Vox may send what you say to the Brain endpoint."
        }
    }
}

/// Decides what `feature` should use. Checks run in a fixed order: provider, base URL, model,
/// token, then transcript consent for the exact destination.
pub fn resolve(settings: &AppSettings, feature: BrainFeature) -> Resolved {
    if !settings.brain_schema_v1 {
        return Resolved::Own;
    }
    let source = match feature {
        BrainFeature::Composer => settings.brain_source_composer,
        BrainFeature::RealtimeStream => settings.brain_source_realtime,
        BrainFeature::PostProcess => settings.brain_source_postprocess,
    };
    if source == BrainSource::Own {
        return Resolved::Own;
    }
    let provider_id = settings.brain_provider_id.as_str();
    if !is_preset(provider_id) {
        return Resolved::NotConfigured(BrainMissing::Provider);
    }
    let base_url = settings.brain_base_url.trim();
    if validate_base_url(base_url).is_err() {
        return Resolved::NotConfigured(BrainMissing::BaseUrl);
    }
    let model = settings.brain_model.trim();
    if model.is_empty() {
        return Resolved::NotConfigured(BrainMissing::Model);
    }
    let token = settings.brain_token.as_str();
    let loopback = Url::parse(base_url).is_ok_and(|url| is_loopback_url(&url));
    if token.is_empty() && !loopback {
        return Resolved::NotConfigured(BrainMissing::Token);
    }
    if settings.brain_transcript_consent_dest != destination(provider_id, base_url) {
        return Resolved::NotConfigured(BrainMissing::Consent);
    }
    Resolved::Brain(BrainConnection {
        provider_id: provider_id.to_string(),
        base_url: base_url.to_string(),
        model: model.to_string(),
        token: token.to_string(),
    })
}

/// The provider, key and model a reader should call for `feature`.
///
/// Own returns exactly the legacy values the readers used before the Brain existed. Inherit maps
/// the Brain connection onto a provider and uses the Brain token as the key.
pub fn effective_provider(
    settings: &AppSettings,
    feature: BrainFeature,
) -> Result<EffectiveLlm, BrainMissing> {
    match resolve(settings, feature) {
        Resolved::Own => own_effective(settings, feature),
        Resolved::Brain(connection) => Ok(brain_effective(connection)),
        Resolved::NotConfigured(missing) => Err(missing),
    }
}

/// The legacy provider, key and model for an Own feature.
fn own_effective(
    settings: &AppSettings,
    feature: BrainFeature,
) -> Result<EffectiveLlm, BrainMissing> {
    let (provider, model) = match feature {
        BrainFeature::Composer | BrainFeature::RealtimeStream => {
            let provider = settings
                .post_process_providers
                .iter()
                .find(|p| p.id == settings.tts_composer_provider_id)
                .ok_or(BrainMissing::Provider)?;
            (provider, settings.tts_composer_model.clone())
        }
        BrainFeature::PostProcess => {
            let provider = settings
                .active_post_process_provider()
                .ok_or(BrainMissing::Provider)?;
            let model = settings
                .post_process_models
                .get(&provider.id)
                .cloned()
                .unwrap_or_default();
            (provider, model)
        }
    };
    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();
    Ok(EffectiveLlm {
        provider: provider.clone(),
        api_key,
        model,
    })
}

/// The Brain connection mapped onto the provider shape the readers already use.
fn brain_effective(connection: BrainConnection) -> EffectiveLlm {
    let BrainConnection {
        provider_id,
        base_url,
        model,
        token,
    } = connection;
    EffectiveLlm {
        provider: PostProcessProvider {
            id: provider_id.clone(),
            label: provider_id,
            base_url: base_url.trim_end_matches('/').to_string(),
            allow_base_url_edit: false,
            models_endpoint: None,
            supports_structured_output: false,
        },
        api_key: token,
        model,
    }
}

/// The one-time migration, without a backup. Marker set => AlreadyDone. A fresh store gets
/// Inherit for every feature; an existing store keeps Own. Nothing is seeded from legacy keys.
pub fn migrate(settings: &mut AppSettings, store_existed: bool) -> MigrationOutcome {
    if settings.brain_schema_v1 {
        return MigrationOutcome::AlreadyDone;
    }
    settings.brain_schema_v1 = true;
    let source = if store_existed {
        BrainSource::Own
    } else {
        BrainSource::Inherit
    };
    settings.brain_source_composer = source;
    settings.brain_source_realtime = source;
    settings.brain_source_postprocess = source;
    if store_existed {
        MigrationOutcome::Migrated
    } else {
        MigrationOutcome::Fresh
    }
}

/// The migration with its backup. A backup is written first for an existing unmarked store. If
/// it fails, settings stay untouched and legacy resolution continues.
pub fn migrate_with_backup(
    settings: &mut AppSettings,
    store_existed: bool,
    store_path: &Path,
    raw: &[u8],
) -> MigrationOutcome {
    if settings.brain_schema_v1 || !store_existed {
        return migrate(settings, store_existed);
    }
    if write_backup_once(store_path, raw).is_err() {
        return MigrationOutcome::BackupFailed;
    }
    migrate(settings, store_existed)
}

/// Writes the pre-migration backup next to `store_path`. Returns `Ok(true)` when it was written
/// and `Ok(false)` when a backup already exists. An existing backup is never overwritten.
pub fn write_backup_once(store_path: &Path, raw: &[u8]) -> std::io::Result<bool> {
    let path = store_path.with_file_name(BACKUP_FILE_NAME);
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    };
    if let Err(e) = write_and_sync(&mut file, raw) {
        // A partial backup must not block a retry, so remove it before reporting the error.
        let _ = std::fs::remove_file(&path);
        return Err(e);
    }
    Ok(true)
}

fn write_and_sync(file: &mut std::fs::File, raw: &[u8]) -> std::io::Result<()> {
    file.write_all(raw)?;
    file.sync_all()
}

/// Saves the connection the user entered. Only a preset, a valid endpoint and a trimmed model
/// are accepted. A rejected call changes nothing. A new destination clears the test
/// acknowledgement and the transcript consent.
pub fn apply_connection(
    settings: &mut AppSettings,
    provider_id: &str,
    base_url: &str,
    model: &str,
) -> Result<(), String> {
    let provider_id = provider_id.trim();
    if !is_preset(provider_id) {
        return Err("Choose a provider from the list on the Brain page.".to_string());
    }
    let base_url = base_url.trim();
    validate_base_url(base_url).map_err(|e| url_error_message(e).to_string())?;
    let previous = destination(&settings.brain_provider_id, &settings.brain_base_url);
    let next = destination(provider_id, base_url);
    settings.brain_provider_id = provider_id.to_string();
    settings.brain_base_url = base_url.to_string();
    settings.brain_model = model.trim().to_string();
    if previous != next {
        settings.brain_test_ack_host.clear();
        settings.brain_transcript_consent_dest.clear();
    }
    Ok(())
}

fn set_source(settings: &mut AppSettings, feature: BrainFeature, source: BrainSource) {
    match feature {
        BrainFeature::Composer => settings.brain_source_composer = source,
        BrainFeature::RealtimeStream => settings.brain_source_realtime = source,
        BrainFeature::PostProcess => settings.brain_source_postprocess = source,
    }
}

/// Makes `feature` inherit the Brain. This also records consent for the current destination.
/// For the composer, mode Off becomes Model. Command mode is never changed.
pub fn use_brain_for(settings: &mut AppSettings, feature: BrainFeature) {
    set_source(settings, feature, BrainSource::Inherit);
    settings.brain_transcript_consent_dest =
        destination(&settings.brain_provider_id, &settings.brain_base_url);
    if feature == BrainFeature::Composer && settings.tts_composer_mode == TtsComposerMode::Off {
        settings.tts_composer_mode = TtsComposerMode::Model;
    }
}

/// Makes `feature` use its own legacy settings again. Nothing else changes.
pub fn use_own_for(settings: &mut AppSettings, feature: BrainFeature) {
    set_source(settings, feature, BrainSource::Own);
}

/// True when the test acknowledgement was given for the current destination.
pub fn ack_allows_network(settings: &AppSettings) -> bool {
    let current = destination(&settings.brain_provider_id, &settings.brain_base_url);
    !settings.brain_test_ack_host.is_empty() && settings.brain_test_ack_host == current
}

/// The pieces a connection test needs: a preset, a valid endpoint and a model. The token and
/// transcript consent are not needed to test, so the user can test before confirming consent.
pub fn ready_for_test(settings: &AppSettings) -> Result<(), BrainMissing> {
    if !is_preset(settings.brain_provider_id.trim()) {
        return Err(BrainMissing::Provider);
    }
    if validate_base_url(&settings.brain_base_url).is_err() {
        return Err(BrainMissing::BaseUrl);
    }
    if settings.brain_model.trim().is_empty() {
        return Err(BrainMissing::Model);
    }
    Ok(())
}

/// Records the test acknowledgement for the current destination, once the connection is ready.
pub fn acknowledge_test(settings: &mut AppSettings) -> Result<(), String> {
    ready_for_test(settings).map_err(|m| missing_message(&m).to_string())?;
    settings.brain_test_ack_host =
        destination(&settings.brain_provider_id, &settings.brain_base_url);
    Ok(())
}

/// The connection a network command may use. The caller must have checked the acknowledgement.
/// The token is read here and only here, so it never crosses the IPC boundary.
pub fn connection_for_network(settings: &AppSettings) -> Result<BrainConnection, String> {
    ready_for_test(settings).map_err(|m| missing_message(&m).to_string())?;
    Ok(BrainConnection {
        provider_id: settings.brain_provider_id.trim().to_string(),
        base_url: settings.brain_base_url.trim().to_string(),
        model: settings.brain_model.trim().to_string(),
        token: settings.brain_token.as_str().to_string(),
    })
}

/// Makes `feature` inherit the Brain, but only when the connection is complete. Transcript
/// consent is the one piece this call itself records, so it is the only missing piece allowed.
/// The feature's current source is ignored for this check.
pub fn use_brain_checked(settings: &mut AppSettings, feature: BrainFeature) -> Result<(), String> {
    let mut probe = settings.clone();
    set_source(&mut probe, feature, BrainSource::Inherit);
    if let Resolved::NotConfigured(missing) = resolve(&probe, feature) {
        if missing != BrainMissing::Consent {
            return Err(missing_message(&missing).to_string());
        }
    }
    use_brain_for(settings, feature);
    Ok(())
}

/// The fixed synthetic prompt. A connection test sends this and nothing else.
const TEST_PROMPT: &str = "Reply with OK.";

/// Time allowed to connect to the endpoint.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Time allowed for a whole Brain request, including reading the reply.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Connection test: one 1-token chat request with a fixed prompt. Only this function sends a
/// request on the user's behalf for a connection test. The reply text is never kept or returned.
pub async fn run_test(connection: &BrainConnection) -> BrainTestResult {
    let started = Instant::now();
    let kind = match chat_probe(connection).await {
        Ok(()) => BrainTestKind::Ok,
        Err(kind) => kind,
    };
    BrainTestResult {
        ok: kind == BrainTestKind::Ok,
        kind,
        latency_ms: elapsed_ms(started),
        model_count: None,
    }
}

/// Lists the models the endpoint offers (`{base}/models`), deduplicated in the order given.
pub async fn list_models(connection: &BrainConnection) -> Result<Vec<String>, BrainTestKind> {
    validate_base_url(&connection.base_url).map_err(|_| BrainTestKind::InvalidUrl)?;
    let client = brain_client()?;
    let request = authorize(client.get(endpoint(connection, "models")), connection);
    let response = send(request).await?;
    if let Some(kind) = classify_status(response.status()) {
        return Err(kind);
    }
    let value: serde_json::Value = response.json().await.map_err(map_body_error)?;
    model_ids(&value).ok_or(BrainTestKind::BadResponse)
}

/// The user-facing text for a connection outcome. Never includes server text or the address.
pub fn test_kind_message(kind: BrainTestKind) -> &'static str {
    match kind {
        BrainTestKind::Ok => "The Brain answered.",
        BrainTestKind::Unauthorized => {
            "The endpoint rejected the access token. Check the token on the Brain page."
        }
        BrainTestKind::NotFound => {
            "The endpoint path was not found. Check the endpoint address and the model name."
        }
        BrainTestKind::Unreachable => {
            "Could not reach the endpoint. Check the address and that the server is running."
        }
        BrainTestKind::Redirect => {
            "The endpoint redirected the request. Redirects are not followed; use the final address."
        }
        BrainTestKind::BadResponse => "The endpoint replied, but not in the expected format.",
        BrainTestKind::Timeout => "The endpoint did not answer in time.",
        BrainTestKind::InvalidUrl => "The endpoint address is not allowed. See the Brain page.",
    }
}

/// Sends the 1-token chat request and checks that the reply has choices.
async fn chat_probe(connection: &BrainConnection) -> Result<(), BrainTestKind> {
    validate_base_url(&connection.base_url).map_err(|_| BrainTestKind::InvalidUrl)?;
    let client = brain_client()?;
    let body = serde_json::json!({
        "model": connection.model,
        "messages": [{"role": "user", "content": TEST_PROMPT}],
        "max_tokens": 8,
    });
    let request = authorize(
        client.post(endpoint(connection, "chat/completions")),
        connection,
    );
    let response = send(request.json(&body)).await?;
    if let Some(kind) = classify_status(response.status()) {
        return Err(kind);
    }
    let value: serde_json::Value = response.json().await.map_err(map_body_error)?;
    if value
        .get("choices")
        .is_some_and(serde_json::Value::is_array)
    {
        Ok(())
    } else {
        Err(BrainTestKind::BadResponse)
    }
}

/// The one HTTP client for a Brain request. Redirects are never followed, so the token cannot
/// reach another origin.
fn brain_client() -> Result<reqwest::Client, BrainTestKind> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| BrainTestKind::Unreachable)
}

/// `{base}/{path}` with the trailing slash of the base removed.
fn endpoint(connection: &BrainConnection, path: &str) -> String {
    format!(
        "{}/{path}",
        connection.base_url.trim().trim_end_matches('/')
    )
}

/// Adds the bearer token only when one is stored.
fn authorize(
    request: reqwest::RequestBuilder,
    connection: &BrainConnection,
) -> reqwest::RequestBuilder {
    if connection.token.is_empty() {
        request
    } else {
        request.bearer_auth(&connection.token)
    }
}

async fn send(request: reqwest::RequestBuilder) -> Result<reqwest::Response, BrainTestKind> {
    request.send().await.map_err(|e| {
        if e.is_timeout() {
            BrainTestKind::Timeout
        } else {
            BrainTestKind::Unreachable
        }
    })
}

fn map_body_error(error: reqwest::Error) -> BrainTestKind {
    if error.is_timeout() {
        BrainTestKind::Timeout
    } else {
        BrainTestKind::BadResponse
    }
}

/// Maps a non-success status to its kind. `None` for a 2xx reply.
fn classify_status(status: reqwest::StatusCode) -> Option<BrainTestKind> {
    if status.is_redirection() {
        return Some(BrainTestKind::Redirect);
    }
    match status.as_u16() {
        200..=299 => None,
        401 | 403 => Some(BrainTestKind::Unauthorized),
        404 => Some(BrainTestKind::NotFound),
        _ => Some(BrainTestKind::BadResponse),
    }
}

/// Model ids from an OpenAI-style `data` list, or a `models` list with `id` or `name`.
/// Duplicates are dropped and the first occurrence keeps its place.
fn model_ids(value: &serde_json::Value) -> Option<Vec<String>> {
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))?
        .as_array()?;
    let mut ids: Vec<String> = Vec::new();
    for entry in entries {
        let id = entry
            .get("id")
            .or_else(|| entry.get("name"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|id| !id.is_empty());
        if let Some(id) = id {
            if !ids.iter().any(|known| known == id) {
                ids.push(id.to_string());
            }
        }
    }
    Some(ids)
}

fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
