//! Saves a finished hands-free conversation (DESIGN Rev 3, 3.5 (1)-(4)).
//!
//! Everything here is either pure (collector, routing, settings) or takes its
//! side effects through injected traits, so the save flow is testable offline.
//! Logs carry only fixed kinds, never transcript, summary or path text.

use crate::managers::composer_config::{
    composer_provider_supported, settings_from_app, TtsComposerMode,
};
use crate::managers::conversation_record::{
    clean_summary, fallback_summary, note_rel_path, render_markdown, sanitize_turn, session_id8,
    summary_prompt, SessionRecord, TurnRecord, MAX_NOTE_BYTES, MAX_TURNS,
};
use crate::managers::converse::{SessionEnd, TranscriptEvent, TranscriptSink};
use crate::managers::realtime_config::RealtimeConfig;
use crate::managers::vault::{Vault, VaultError};
use crate::managers::vault_context::build_vault_block;
use crate::settings::AppSettings;
use futures_util::future::BoxFuture;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Transcript events collected for ONE session (DESIGN Rev 3, 3.5 (1)).
///
/// Bounded by `conversation_record::MAX_TURNS`: further events are dropped and counted. Every
/// event passes `sanitize_turn`, and the outcome and delivery labels are mapped through the
/// `From` conversions in `converse`.
pub struct TranscriptCollector {
    state: Mutex<CollectorState>,
    started_at_ms: i64,
}

#[derive(Default)]
struct CollectorState {
    turns: Vec<TurnRecord>,
    dropped: usize,
}

/// The events one session collected, handed over by [`TranscriptCollector::take`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollectedSession {
    /// Sanitized turns, in order, at most `MAX_TURNS`.
    pub turns: Vec<TurnRecord>,
    /// Events dropped because the session already held `MAX_TURNS` turns.
    pub dropped: usize,
    /// Creation time of the collector, in ms since the Unix epoch (from the injected clock).
    pub started_at_ms: i64,
}

impl TranscriptCollector {
    /// Creates a collector that stamps `started_at_ms` from the system clock.
    pub fn new() -> Self {
        Self::with_clock(Box::new(system_now_ms))
    }

    /// Creates a collector that stamps `started_at_ms` from `clock` (tests inject a fixed clock).
    pub fn with_clock(clock: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        TranscriptCollector {
            state: Mutex::new(CollectorState::default()),
            started_at_ms: clock(),
        }
    }

    /// Returns everything collected so far and leaves the collector empty.
    pub fn take(&self) -> CollectedSession {
        let (turns, dropped) = match self.state.lock() {
            Ok(mut state) => {
                let turns = std::mem::take(&mut state.turns);
                let dropped = std::mem::take(&mut state.dropped);
                (turns, dropped)
            }
            Err(_) => (Vec::new(), 0),
        };
        CollectedSession {
            turns,
            dropped,
            started_at_ms: self.started_at_ms,
        }
    }
}

impl Default for TranscriptCollector {
    fn default() -> Self {
        Self::new()
    }
}

impl TranscriptSink for TranscriptCollector {
    /// Sanitizes the event and appends it, or counts it as dropped once the bound is reached.
    fn record(&self, ev: TranscriptEvent) {
        let turn = sanitize_turn(
            &ev.user,
            ev.assistant.as_deref(),
            ev.outcome.into(),
            ev.delivery.into(),
            ev.detail.as_deref(),
        );
        if let Ok(mut state) = self.state.lock() {
            if state.turns.len() < MAX_TURNS {
                state.turns.push(turn);
            } else {
                state.dropped = state.dropped.saturating_add(1);
            }
        }
    }
}

/// Current time in ms since the Unix epoch. Falls back to 0 if the clock is before the epoch.
fn system_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_millis()).ok())
        .unwrap_or(0)
}

/// The save-related settings, copied out of `AppSettings` (DESIGN Rev 3, section 2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSettings {
    /// Save a row to Vox's conversation database.
    pub records_enabled: bool,
    /// Also write a Markdown note into the vault (needs `records_enabled` as well).
    pub to_vault: bool,
    /// Absolute path of the notes vault folder. Empty or blank means not configured.
    pub vault_path: String,
}

impl SaveSettings {
    /// Copies the save-related fields out of the user's settings.
    pub fn from_settings(s: &AppSettings) -> SaveSettings {
        SaveSettings {
            records_enabled: s.conversation_records_enabled,
            to_vault: s.conversation_records_to_vault,
            vault_path: s.vault_path.clone(),
        }
    }
}

/// Summarizes a session with the conversation's own model route. Fakes implement this in tests.
pub trait Summarizer: Send + Sync {
    /// Returns the raw model text, or an error string that is never shown to the user.
    fn summarize<'a>(
        &'a self,
        system: String,
        user: String,
    ) -> BoxFuture<'a, Result<String, String>>;
}

/// Writes one session row to the conversation store.
pub trait RecordStoreSink: Send + Sync {
    /// Inserts the session. `vault_note` is `Some` when a vault note was written.
    fn insert(&self, rec: &SessionRecord, vault_note: Option<&str>) -> Result<i64, String>;
}

/// Writes one note under the vault folder.
pub trait NoteSink: Send + Sync {
    /// Writes `contents` at `rel` under `vault_path` (see `Vault::write_note`).
    fn write_note(
        &self,
        vault_path: &str,
        rel: &str,
        contents: &str,
    ) -> Result<PathBuf, VaultError>;
}

/// Everything `save_session` depends on. All dependencies are injected.
pub struct SaveDeps<'a> {
    pub summarizer: &'a dyn Summarizer,
    pub store: &'a dyn RecordStoreSink,
    pub notes: &'a dyn NoteSink,
    /// Current time in ms since the Unix epoch.
    pub now_ms: &'a (dyn Fn() -> i64 + Send + Sync),
    /// Local UTC offset in seconds, used for the note path.
    pub utc_offset_secs: i32,
}

/// Why nothing was saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// `records_enabled` is off.
    Disabled,
    /// The session had no turns.
    NoTurns,
    /// The session ended with `NotConfigured` or `AudioUnavailable`.
    NotRecordable,
}

/// The outcome of one save. Carries no transcript, summary or path text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveReport {
    pub db_saved: bool,
    pub vault_saved: bool,
    /// True when the stored summary came from the model (not the fallback).
    pub summary_from_model: bool,
    pub skipped: Option<SkipReason>,
}

/// Summary deadline for a saved session (DESIGN Rev 3, 3.5 (4)).
const SUMMARY_DEADLINE: Duration = Duration::from_secs(20);

/// Saves a finished session. The summary waits at most 20 s; see [`save_session_with_deadline`].
///
/// Skipped (nothing called) when records are disabled, the session has no turns, or the end is
/// `NotConfigured` / `AudioUnavailable`. Otherwise: summary (model, or fallback on error,
/// timeout, empty output or `clean_summary == None`), then one DB insert, then, when
/// `to_vault` and the path is set, one note write. Each sink failure is independent. A note
/// `Exists` error triggers ONE retry with a 12-hex id; a second `Exists` gives up silently.
pub async fn save_session(
    settings: &SaveSettings,
    deps: &SaveDeps<'_>,
    session: CollectedSession,
    end: SessionEnd,
) -> SaveReport {
    save_session_with_deadline(settings, deps, session, end, SUMMARY_DEADLINE).await
}

/// Same as [`save_session`] with an injectable summary deadline (tests use a short one).
pub async fn save_session_with_deadline(
    settings: &SaveSettings,
    deps: &SaveDeps<'_>,
    session: CollectedSession,
    end: SessionEnd,
    deadline: Duration,
) -> SaveReport {
    let skip = |reason: SkipReason| SaveReport {
        db_saved: false,
        vault_saved: false,
        summary_from_model: false,
        skipped: Some(reason),
    };
    if !settings.records_enabled {
        return skip(SkipReason::Disabled);
    }
    if session.turns.is_empty() {
        return skip(SkipReason::NoTurns);
    }
    if matches!(
        end,
        SessionEnd::NotConfigured | SessionEnd::AudioUnavailable
    ) {
        return skip(SkipReason::NotRecordable);
    }

    let ended_at_ms = (deps.now_ms)();
    let (summary, summary_from_model) = summarize_or_fallback(deps, &session.turns, deadline).await;
    let record = SessionRecord {
        started_at_ms: session.started_at_ms,
        ended_at_ms,
        end: end.into(),
        turns: session.turns,
        summary: Some(summary),
    };

    let sid8 = session_id8(record.started_at_ms, &record.turns);
    let vault_rel = if settings.to_vault && !settings.vault_path.trim().is_empty() {
        write_vault_note(deps, &settings.vault_path, &record, &sid8)
    } else {
        None
    };

    let db_saved = deps.store.insert(&record, vault_rel.as_deref()).is_ok();
    SaveReport {
        db_saved,
        vault_saved: vault_rel.is_some(),
        summary_from_model,
        skipped: None,
    }
}

/// Runs the summary under `deadline`. Any error, timeout, empty output or unusable output falls
/// back to the deterministic summary. Returns `(summary, from_model)`.
async fn summarize_or_fallback(
    deps: &SaveDeps<'_>,
    turns: &[TurnRecord],
    deadline: Duration,
) -> (String, bool) {
    let (system, user) = summary_prompt(turns);
    let call = deps.summarizer.summarize(system, user);
    let raw = match tokio::time::timeout(deadline, call).await {
        Ok(Ok(raw)) => raw,
        Ok(Err(_)) => {
            log::debug!("conversation summary unavailable: backend");
            return (fallback_summary(turns), false);
        }
        Err(_) => {
            log::debug!("conversation summary unavailable: timeout");
            return (fallback_summary(turns), false);
        }
    };
    match clean_summary(&raw) {
        Some(text) => (text, true),
        None => {
            log::debug!("conversation summary unavailable: empty");
            (fallback_summary(turns), false)
        }
    }
}

/// Writes the note for `record`. Returns the vault-relative path on success. One retry with a
/// 12-hex id follows an `Exists` error; any other failure, or a second `Exists`, gives up.
fn write_vault_note(
    deps: &SaveDeps<'_>,
    vault_path: &str,
    record: &SessionRecord,
    sid8: &str,
) -> Option<String> {
    let markdown = render_markdown(record, sid8);
    if markdown.len() > MAX_NOTE_BYTES {
        log::debug!("conversation note skipped: too large");
        return None;
    }
    let rel = note_rel_path(record, sid8, deps.utc_offset_secs);
    match deps.notes.write_note(vault_path, &rel, &markdown) {
        Ok(_) => return Some(rel),
        Err(VaultError::Exists) => {}
        Err(err) => {
            log::debug!("conversation note not written: {}", vault_error_kind(&err));
            return None;
        }
    }
    let retry_sid = session_id8_long(record);
    let retry_markdown = render_markdown(record, &retry_sid);
    let retry_rel = note_rel_path(record, &retry_sid, deps.utc_offset_secs);
    match deps
        .notes
        .write_note(vault_path, &retry_rel, &retry_markdown)
    {
        Ok(_) => Some(retry_rel),
        Err(err) => {
            log::debug!("conversation note not written: {}", vault_error_kind(&err));
            None
        }
    }
}

/// The 12-hex id used by the single `Exists` retry: the first 12 hex digits of the same digest
/// as [`session_id8`], extended.
fn session_id8_long(record: &SessionRecord) -> String {
    use sha2::{Digest, Sha256};
    let first_user = record.turns.first().map_or("", |t| t.user.as_str());
    let mut hasher = Sha256::new();
    hasher.update(record.started_at_ms.to_string().as_bytes());
    hasher.update(b"\n");
    hasher.update(first_user.as_bytes());
    hasher
        .finalize()
        .iter()
        .take(6)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// A fixed, text-free label for a vault error (logs never carry paths or contents).
fn vault_error_kind(err: &VaultError) -> &'static str {
    match err {
        VaultError::Invalid => "invalid",
        VaultError::Missing => "missing",
        VaultError::TooLarge => "too_large",
        VaultError::Unsafe => "unsafe",
        VaultError::Exists => "exists",
        VaultError::Io(_) => "io",
        VaultError::Forbidden => "forbidden",
        VaultError::Unstable => "unstable",
    }
}

/// Reads the context pack from the vault and builds the wrapped block (DESIGN Rev 3, 3.5 (5)).
///
/// `Vault::open` and `read_context_pack` run in `spawn_blocking` under `timeout`. Any error,
/// timeout or empty path yields `None`. Success yields `build_vault_block(pack)`.
pub async fn load_vault_block(vault_path: &str, timeout: Duration) -> Option<String> {
    let trimmed = vault_path.trim().to_string();
    if trimmed.is_empty() {
        return None;
    }
    let read = tauri::async_runtime::spawn_blocking(move || {
        let vault = Vault::open(&trimmed).ok()?;
        let pack = vault.read_context_pack().ok()?;
        build_vault_block(&pack)
    });
    match tokio::time::timeout(timeout, read).await {
        Ok(Ok(block)) => block,
        _ => None,
    }
}

/// True when the conversation runs answer-only (DESIGN Rev 3, 3.6): vault context enabled and a
/// non-blank vault path. Conservative: stays true even if the later read fails.
pub fn answer_only_for(settings: &AppSettings) -> bool {
    let has_path = !settings.vault_path.trim().is_empty();
    // A RAG session is answer-only unconditionally (DESIGN Rev 5, 3.2), even with guard_actions off.
    (settings.vault_context_enabled && has_path) || (settings.vault_rag_enabled && has_path)
}

/// True when vault context is actually in use: `answer_only_for` and the realtime master
/// (`exp_rt_enabled`), combined the way `realtime_config` combines it.
pub fn vault_context_active(settings: &AppSettings) -> bool {
    answer_only_for(settings) && RealtimeConfig::from_settings(settings).enabled
}

/// True when the vault RAG may be used for these settings (DESIGN Rev 5, 3.1): RAG enabled, a
/// vault path set, realtime master on and an HTTP model route. Pure, no IO.
// Consumed by RagSession (S8); allowed until then.
#[allow(dead_code)]
pub(crate) fn rag_possible(settings: &AppSettings) -> bool {
    settings.vault_rag_enabled
        && !settings.vault_path.trim().is_empty()
        && RealtimeConfig::from_settings(settings).enabled
        && crate::managers::converse_platform::vault_text_allowed(settings)
}

/// Which backend the summary call uses (DESIGN Rev 3, 3.5 (4)). Mirrors the reply route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryRoute {
    /// Direct Anthropic-style endpoint: `exp_rt_enabled && exp_rt_direct_endpoint`.
    DirectAnthropic,
    /// OpenAI-compatible HTTP: composer mode Model with a supported provider and a model, and
    /// the realtime stream route on (`exp_rt_enabled && exp_rt_stream_reply`).
    OpenAiCompatible,
    /// Legacy model route: as `OpenAiCompatible`, with the stream route off.
    LegacyModel,
    /// No model call (fallback summary only): composer mode Off or Command (external process),
    /// or Model without a supported provider or model.
    NoModel,
}

/// Picks the summary route from the same settings the reply route reads (DESIGN Rev 3, 3.5 (4)).
///
/// Precedence: the direct endpoint first (a misconfigured one never falls back to the composer),
/// then the composer mode. `agent_bridge_enabled` does not affect the conversation.
pub fn summary_route(settings: &AppSettings) -> SummaryRoute {
    let rt = RealtimeConfig::from_settings(settings);
    if rt.direct_endpoint {
        return SummaryRoute::DirectAnthropic;
    }
    let composer = settings_from_app(settings);
    if composer.mode != TtsComposerMode::Model || composer.model.trim().is_empty() {
        return SummaryRoute::NoModel;
    }
    let supported = settings
        .post_process_providers
        .iter()
        .find(|p| p.id == composer.provider_id)
        .is_some_and(composer_provider_supported);
    if !supported {
        return SummaryRoute::NoModel;
    }
    if rt.stream_reply {
        SummaryRoute::OpenAiCompatible
    } else {
        SummaryRoute::LegacyModel
    }
}

/// Request options for the summary call (DESIGN Rev 3, 3.5 (4)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SummaryOptions {
    /// Always false: the summary is plain text.
    pub json_mode: bool,
    /// Always 300.
    pub max_tokens: u32,
    /// Always 20.
    pub deadline_secs: u64,
}

/// The summary request options for `route`: json_mode false, max_tokens 300, deadline 20 s for
/// every model route.
pub fn summary_options(route: SummaryRoute) -> SummaryOptions {
    let _ = route;
    SummaryOptions {
        json_mode: false,
        max_tokens: 300,
        deadline_secs: 20,
    }
}

/// Live [`Summarizer`] for the conversation's own route (DESIGN Rev 3, 3.5 (4)). Command and
/// off routes never call a model: they fail fast and the fallback summary is stored.
pub struct RouteSummarizer {
    settings: AppSettings,
}

impl RouteSummarizer {
    /// Captures the settings snapshot at session start.
    pub fn new(settings: AppSettings) -> Self {
        RouteSummarizer { settings }
    }
}

impl Summarizer for RouteSummarizer {
    fn summarize<'a>(
        &'a self,
        system: String,
        user: String,
    ) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let route = summary_route(&self.settings);
            if route == SummaryRoute::NoModel {
                return Err("no model route".to_string());
            }
            let opts = summary_options(route);
            let deadline = Duration::from_secs(opts.deadline_secs);
            crate::managers::speech_backends::complete_summary(
                &self.settings,
                &system,
                &user,
                opts.max_tokens,
                deadline,
            )
            .await
            .map_err(|err| err.outcome().to_string())
        })
    }
}

/// Live [`RecordStoreSink`] over the managed conversation store.
pub struct StoreSink {
    store: Arc<crate::managers::conversation_store::ConversationStore>,
}

impl StoreSink {
    /// Wraps the shared store handle.
    pub fn new(store: Arc<crate::managers::conversation_store::ConversationStore>) -> Self {
        StoreSink { store }
    }
}

impl RecordStoreSink for StoreSink {
    fn insert(&self, rec: &SessionRecord, vault_note: Option<&str>) -> Result<i64, String> {
        self.store
            .insert(rec, vault_note)
            .map_err(|err| err.to_string())
    }
}

/// Live [`NoteSink`] that writes through [`Vault`] (handle-relative, no-follow IO).
pub struct VaultNoteSink;

impl NoteSink for VaultNoteSink {
    fn write_note(
        &self,
        vault_path: &str,
        rel: &str,
        contents: &str,
    ) -> Result<PathBuf, VaultError> {
        Vault::open(vault_path)?.write_note(rel, contents)
    }
}
