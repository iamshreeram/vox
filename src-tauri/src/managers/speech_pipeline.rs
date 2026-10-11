//! Orchestration of one spoken reply: admission protocol, composition with
//! cancellation, fallback, and the preview action. Everything here is driven
//! through injectable pieces (a `Composer`, a settings loader, a status sink) so
//! it is testable over a real `TtsManager` with fake backends. See DESIGN.md
//! sections 0.1 and 3.1.

#![allow(dead_code)]

use crate::managers::composer_config::{TtsComposerFallback, TtsComposerMode};
use crate::managers::composer_rules::clamp_timeout;
use crate::managers::emotion::Emotion;
use crate::managers::speech_compose::{fallback_notice, ComposeError, ComposeRequest, Composed};
use crate::managers::tts::{phrase_for_event, wait_superseded, TtsError, TtsManager};
use crate::settings::AppSettings;
use futures_util::future::{select, Either};
use serde::{Deserialize, Serialize};
use specta::Type;
use std::future::Future;
use std::pin::Pin;

const SUPERSEDED_MSG: &str = "Replaced by newer speech";

/// The fixed sample used by the Preview action (never the user's data).
pub const PREVIEW_SAMPLE: &str =
    "Build finished. 412 tests passed and 3 failed. Coverage is 81 percent.";

/// Internal app-event payload `agent-bridge-exchange`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Exchange {
    pub reply: String,
    pub utterance: Option<String>,
}

/// Decode `{"reply": string, "utterance": string|null}`; anything malformed
/// (not JSON, missing/non-string reply) => None. Never panics. Extra fields
/// are ignored; a non-string utterance is treated as None.
pub fn parse_exchange(payload: &str) -> Option<Exchange> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let reply = value.get("reply")?.as_str()?.to_string();
    let utterance = value
        .get("utterance")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Some(Exchange { reply, utterance })
}

/// Longest user utterance carried in the exchange event (characters).
pub const MAX_EXCHANGE_UTTERANCE_CHARS: usize = 500;

/// Build the internal `agent-bridge-exchange` payload. The utterance is
/// truncated on a char boundary; an absent or blank utterance becomes `null`.
pub fn build_exchange_payload(reply: &str, utterance: Option<&str>) -> serde_json::Value {
    let utterance = utterance.map(str::trim).filter(|u| !u.is_empty()).map(|u| {
        u.chars()
            .take(MAX_EXCHANGE_UTTERANCE_CHARS)
            .collect::<String>()
    });
    serde_json::json!({ "reply": reply, "utterance": utterance })
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Type)]
pub struct ComposerPreview {
    pub speech: String,
    pub emotion: Emotion,
}

pub type ComposeFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Composed, ComposeError>> + Send + 'a>>;

pub trait Composer: Send + Sync {
    fn compose<'a>(&'a self, settings: &'a AppSettings, req: ComposeRequest) -> ComposeFuture<'a>;
}

/// The production composer: `speech_backends::compose`.
pub struct RealComposer;

impl Composer for RealComposer {
    fn compose<'a>(&'a self, settings: &'a AppSettings, req: ComposeRequest) -> ComposeFuture<'a> {
        Box::pin(async move { crate::managers::speech_backends::compose(settings, req).await })
    }
}

/// Race one composition against supersession of `epoch`. `None` means the
/// epoch was superseded first; the composer future is dropped in that case.
async fn compose_until_superseded(
    tts: &TtsManager,
    settings: &AppSettings,
    composer: &dyn Composer,
    req: ComposeRequest,
    epoch: u64,
) -> Option<Result<Composed, ComposeError>> {
    let composing = composer.compose(settings, req);
    let superseded = Box::pin(wait_superseded(tts.epoch_receiver(), epoch));
    match select(composing, superseded).await {
        Either::Left((result, _)) => Some(result),
        Either::Right(((), _)) => None,
    }
}

/// Preview error text for a composition failure (exact strings, see
/// `run_preview`).
fn preview_error_message(err: ComposeError, settings: &AppSettings) -> String {
    let model_missing = settings.tts_composer_mode == TtsComposerMode::Model
        && settings.tts_composer_model.trim().is_empty();
    match err {
        ComposeError::Timeout => format!(
            "Timed out after {}s",
            clamp_timeout(settings.tts_composer_timeout_secs)
        ),
        ComposeError::Unparseable | ComposeError::Empty => {
            "The model returned nothing usable".to_string()
        }
        ComposeError::NeedsAcknowledgement => "Command needs acknowledgement".to_string(),
        ComposeError::NotConfigured | ComposeError::BadConfig => {
            if model_missing {
                "Pick a model first".to_string()
            } else {
                "Not configured".to_string()
            }
        }
        ComposeError::Backend => "The model or command failed".to_string(),
        ComposeError::OutputTooLarge => "The output was too large".to_string(),
    }
}

/// Speak one agent exchange. ADMISSION (DESIGN 0.1), in this exact order:
/// (1) `tts.begin_utterance()`; Err => return silently (no status).
/// (2) `load_settings()` (AFTER the reservation).
/// (3) `tts.is_current(epoch)`; false => return silently (a writer invalidated).
/// (4) `settings.tts_enabled == false` => return silently.
///
/// Mode Off => speak `phrase_for_event("agent-bridge-reply", reply)` through
/// `speak_styled_if_current(epoch, .., Neutral)`; no composer call, NO status.
/// Mode Model/Command => build `ComposeRequest{reply, utterance (only when
/// `tts_composer_send_user_utterance`), style}`, run `composer.compose` raced
/// against `wait_superseded(tts.epoch_receiver(), epoch)` (the losing composer
/// future is DROPPED). Voice = `tts_voice`, rate = `Some(tts_rate_wpm)`.
/// Exactly ONE status string is emitted per exchange in these modes:
/// composed + spoken => "ok"; superseded while composing => "superseded";
/// composed but `speak_styled_if_current` returned Superseded/Recording =>
/// "superseded", any other speak error => "backend_error"; composition error =>
/// `err.outcome()` after applying the configured fallback (ShortNotice speaks
/// `fallback_notice()`, Verbatim speaks the reply, Silent nothing; all with the
/// ORIGINAL epoch via `speak_styled_if_current`, Neutral emotion); if the
/// fallback speech is refused with Superseded/Recording the status becomes
/// "superseded", if it fails otherwise "backend_error".
pub async fn run_exchange(
    tts: &TtsManager,
    load_settings: &(dyn Fn() -> AppSettings + Sync),
    composer: &dyn Composer,
    status: &(dyn Fn(&'static str) + Sync),
    exchange: Exchange,
) {
    let epoch = match tts.begin_utterance() {
        Ok(epoch) => epoch,
        Err(_) => return,
    };
    let settings = load_settings();
    if !tts.is_current(epoch) {
        return;
    }
    if !settings.tts_enabled {
        return;
    }
    let voice = settings.tts_voice.as_deref();
    let rate = Some(settings.tts_rate_wpm);

    if settings.tts_composer_mode == TtsComposerMode::Off {
        if let Some(text) = phrase_for_event("agent-bridge-reply", &exchange.reply) {
            let _ = tts.speak_styled_if_current(epoch, &text, voice, rate, Emotion::Neutral);
        }
        return;
    }

    let req = ComposeRequest {
        reply: exchange.reply.clone(),
        user_utterance: if settings.tts_composer_send_user_utterance {
            exchange.utterance.clone()
        } else {
            None
        },
        style: settings.tts_composer_style.clone(),
    };

    let composed = match compose_until_superseded(tts, &settings, composer, req, epoch).await {
        Some(result) => result,
        None => {
            log::debug!("agent exchange outcome: superseded");
            status("superseded");
            return;
        }
    };

    let outcome = match composed {
        Ok(composed) => {
            match tts.speak_styled_if_current(
                epoch,
                &composed.speech,
                voice,
                rate,
                composed.emotion,
            ) {
                Ok(()) => "ok",
                Err(TtsError::Superseded | TtsError::Recording) => "superseded",
                Err(_) => "backend_error",
            }
        }
        Err(err) => {
            let outcome = err.outcome();
            let fallback_text = match settings.tts_composer_fallback {
                TtsComposerFallback::ShortNotice => Some(fallback_notice()),
                TtsComposerFallback::Verbatim => Some(exchange.reply.as_str()),
                TtsComposerFallback::Silent => None,
            };
            match fallback_text {
                None => outcome,
                Some(text) => {
                    match tts.speak_styled_if_current(epoch, text, voice, rate, Emotion::Neutral) {
                        Ok(()) => outcome,
                        Err(TtsError::Superseded | TtsError::Recording) => "superseded",
                        Err(_) => "backend_error",
                    }
                }
            }
        }
    };
    log::debug!("agent exchange outcome: {outcome}");
    status(outcome);
}

/// Preview: compose the fixed sample with the CURRENT configuration (even if
/// `tts_enabled` is false) and, only when `tts_enabled`, speak it. Follows the
/// same admission protocol and supersede race as `run_exchange` and sends NO
/// user utterance. Errors (exact strings): recording => "Recording in
/// progress"; superseded (at any point) => "Replaced by newer speech"; Off mode
/// => "Choose a mode first"; NotConfigured/BadConfig => "Pick a model first"
/// when mode is Model and the model is empty, else "Not configured";
/// NeedsAcknowledgement => "Command needs acknowledgement"; Timeout =>
/// `format!("Timed out after {}s", clamped timeout secs)`; Empty|Unparseable =>
/// "The model returned nothing usable"; Backend => "The model or command
/// failed"; OutputTooLarge => "The output was too large"; a speak error other
/// than Superseded/Recording => `crate::shortcut::tts_error_message(&e)`.
pub async fn run_preview(
    tts: &TtsManager,
    load_settings: &(dyn Fn() -> AppSettings + Sync),
    composer: &dyn Composer,
) -> Result<ComposerPreview, String> {
    let epoch = tts
        .begin_utterance()
        .map_err(|_| "Recording in progress".to_string())?;
    let settings = load_settings();
    if !tts.is_current(epoch) {
        return Err(SUPERSEDED_MSG.to_string());
    }
    if settings.tts_composer_mode == TtsComposerMode::Off {
        return Err("Choose a mode first".to_string());
    }

    let req = ComposeRequest {
        reply: PREVIEW_SAMPLE.to_string(),
        user_utterance: None,
        style: settings.tts_composer_style.clone(),
    };
    let composed = match compose_until_superseded(tts, &settings, composer, req, epoch).await {
        None => return Err(SUPERSEDED_MSG.to_string()),
        Some(Err(err)) => return Err(preview_error_message(err, &settings)),
        Some(Ok(composed)) => composed,
    };

    if settings.tts_enabled {
        let voice = settings.tts_voice.as_deref();
        let rate = Some(settings.tts_rate_wpm);
        match tts.speak_styled_if_current(epoch, &composed.speech, voice, rate, composed.emotion) {
            Ok(()) => {}
            Err(TtsError::Superseded) => return Err(SUPERSEDED_MSG.to_string()),
            Err(TtsError::Recording) => return Err("Recording in progress".to_string()),
            Err(e) => return Err(crate::shortcut::tts_error_message(&e)),
        }
    }

    Ok(ComposerPreview {
        speech: composed.speech,
        emotion: composed.emotion,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::composer_config::{TtsComposerFallback, TtsComposerMode};
    use crate::managers::speech_compose::fallback_notice;
    use crate::managers::tts::{SpeechBackend, SpeechHandle};
    use crate::settings::get_default_settings;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    fn block_on<F: Future>(f: F) -> F::Output {
        tauri::async_runtime::block_on(f)
    }

    // ---------- recording speech backend ----------

    #[derive(Debug, Clone, PartialEq)]
    struct Spoken {
        text: String,
        voice: Option<String>,
        rate: Option<u32>,
        emotion: Emotion,
    }

    struct Log {
        spawns: Mutex<Vec<Spoken>>,
        fail: std::sync::atomic::AtomicBool,
    }
    struct RecBackend(Arc<Log>);
    struct RecHandle;
    impl SpeechBackend for RecBackend {
        fn spawn(
            &self,
            text: &str,
            voice: Option<&str>,
            rate: Option<u32>,
        ) -> io::Result<Box<dyn SpeechHandle>> {
            self.spawn_styled(text, voice, rate, Emotion::Neutral)
        }
        fn spawn_styled(
            &self,
            text: &str,
            voice: Option<&str>,
            rate: Option<u32>,
            emotion: Emotion,
        ) -> io::Result<Box<dyn SpeechHandle>> {
            if self.0.fail.load(Ordering::SeqCst) {
                return Err(io::Error::new(io::ErrorKind::Other, "boom"));
            }
            self.0.spawns.lock().unwrap().push(Spoken {
                text: text.into(),
                voice: voice.map(str::to_string),
                rate,
                emotion,
            });
            Ok(Box::new(RecHandle))
        }
    }
    impl SpeechHandle for RecHandle {
        fn kill(&mut self) -> bool {
            true
        }
        fn is_running(&mut self) -> bool {
            true
        }
    }

    fn tts() -> (Arc<TtsManager>, Arc<Log>) {
        let log = Arc::new(Log {
            spawns: Mutex::new(vec![]),
            fail: std::sync::atomic::AtomicBool::new(false),
        });
        (
            Arc::new(TtsManager::new(Box::new(RecBackend(log.clone())))),
            log,
        )
    }

    fn spoken(log: &Log) -> Vec<Spoken> {
        log.spawns.lock().unwrap().clone()
    }

    // ---------- fake composer ----------

    type Script = dyn Fn(&ComposeRequest) -> (u64, Result<Composed, ComposeError>) + Send + Sync;

    struct DropGuard(Arc<AtomicUsize>);
    impl Drop for DropGuard {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    struct Fake {
        script: Box<Script>,
        calls: Mutex<Vec<ComposeRequest>>,
        completed: Arc<AtomicUsize>,
        dropped_midway: Arc<AtomicUsize>,
    }
    impl Fake {
        fn new(
            script: impl Fn(&ComposeRequest) -> (u64, Result<Composed, ComposeError>)
                + Send
                + Sync
                + 'static,
        ) -> Self {
            Fake {
                script: Box::new(script),
                calls: Mutex::new(vec![]),
                completed: Arc::new(AtomicUsize::new(0)),
                dropped_midway: Arc::new(AtomicUsize::new(0)),
            }
        }
        fn ok(speech: &str, emotion: Emotion, delay_ms: u64) -> Self {
            let c = Composed {
                speech: speech.to_string(),
                emotion,
            };
            Fake::new(move |_| (delay_ms, Ok(c.clone())))
        }
        fn err(e: ComposeError, delay_ms: u64) -> Self {
            Fake::new(move |_| (delay_ms, Err(e)))
        }
        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }
    impl Composer for Fake {
        fn compose<'a>(
            &'a self,
            _settings: &'a AppSettings,
            req: ComposeRequest,
        ) -> ComposeFuture<'a> {
            self.calls.lock().unwrap().push(req.clone());
            let (delay, result) = (self.script)(&req);
            let completed = self.completed.clone();
            let guard = DropGuard(self.dropped_midway.clone());
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(delay)).await;
                completed.fetch_add(1, Ordering::SeqCst);
                std::mem::forget(guard); // only a drop BEFORE completion counts
                result
            })
        }
    }

    // ---------- settings / status helpers ----------

    fn settings(mode: TtsComposerMode, enabled: bool) -> AppSettings {
        let mut s = get_default_settings();
        s.tts_enabled = enabled;
        s.tts_voice = Some("Samantha".to_string());
        s.tts_rate_wpm = 200;
        s.tts_composer_mode = mode;
        s.tts_composer_style = "calm".to_string();
        s
    }

    struct Statuses(Mutex<Vec<&'static str>>);
    impl Statuses {
        fn new() -> Self {
            Statuses(Mutex::new(vec![]))
        }
        fn sink(&self) -> impl Fn(&'static str) + Sync + '_ {
            move |s| self.0.lock().unwrap().push(s)
        }
        fn all(&self) -> Vec<&'static str> {
            self.0.lock().unwrap().clone()
        }
    }

    fn exchange(reply: &str) -> Exchange {
        Exchange {
            reply: reply.to_string(),
            utterance: Some("what happened".to_string()),
        }
    }

    fn run(tts: &TtsManager, s: AppSettings, c: &Fake, st: &Statuses, ex: Exchange) {
        let load = move || s.clone();
        block_on(run_exchange(tts, &load, c, &st.sink(), ex));
    }

    // =====================================================================
    // parse_exchange
    // =====================================================================

    #[test]
    fn exchange_payload_decoding() {
        let ok = parse_exchange(r#"{"reply":"All done","utterance":"status"}"#).unwrap();
        assert_eq!(
            (ok.reply.as_str(), ok.utterance.as_deref()),
            ("All done", Some("status"))
        );
        let uni =
            parse_exchange("{\"reply\":\"caf\\u00e9 \u{1f600}\",\"utterance\":null}").unwrap();
        assert_eq!(uni.reply, "caf\u{e9} \u{1f600}");
        assert_eq!(uni.utterance, None);
        assert_eq!(parse_exchange(r#"{"reply":"x"}"#).unwrap().utterance, None);
        assert_eq!(
            parse_exchange(r#"{"reply":"x","utterance":5,"extra":1}"#)
                .unwrap()
                .utterance,
            None
        );
        for bad in [
            "",
            "not json",
            "{}",
            r#"{"reply":5}"#,
            r#"{"reply":null}"#,
            "[1]",
            "\"str\"",
            r#"{"utterance":"x"}"#,
        ] {
            assert_eq!(parse_exchange(bad), None, "{bad:?}");
        }
    }

    // =====================================================================
    // run_exchange: happy path and gating
    // =====================================================================

    #[test]
    fn composed_speech_is_spoken_once_with_voice_rate_and_emotion() {
        let (t, log) = tts();
        let fake = Fake::ok("Short and sweet.", Emotion::Cheerful, 0);
        let st = Statuses::new();
        run(
            &t,
            settings(TtsComposerMode::Command, true),
            &fake,
            &st,
            exchange("a very long reply"),
        );
        let s = spoken(&log);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "Short and sweet.");
        assert_eq!(s[0].emotion, Emotion::Cheerful);
        assert_eq!(s[0].voice.as_deref(), Some("Samantha"));
        assert_eq!(s[0].rate, Some(200));
        assert_eq!(st.all(), vec!["ok"]);
        let calls = fake.calls.lock().unwrap().clone();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].reply, "a very long reply");
        assert_eq!(calls[0].style, "calm");
        assert_eq!(calls[0].user_utterance.as_deref(), Some("what happened"));
    }

    #[test]
    fn the_user_utterance_is_only_sent_when_the_setting_allows_it() {
        let (t, _log) = tts();
        let fake = Fake::ok("x", Emotion::Neutral, 0);
        let st = Statuses::new();
        let mut s = settings(TtsComposerMode::Command, true);
        s.tts_composer_send_user_utterance = false;
        run(&t, s, &fake, &st, exchange("r"));
        assert_eq!(fake.calls.lock().unwrap()[0].user_utterance, None);
    }

    #[test]
    fn disabled_tts_does_nothing_at_all() {
        let (t, log) = tts();
        let fake = Fake::ok("x", Emotion::Neutral, 0);
        let st = Statuses::new();
        run(
            &t,
            settings(TtsComposerMode::Command, false),
            &fake,
            &st,
            exchange("r"),
        );
        assert!(spoken(&log).is_empty());
        assert_eq!(fake.call_count(), 0);
        assert!(st.all().is_empty());
    }

    #[test]
    fn off_mode_speaks_the_reply_verbatim_with_no_composer_and_no_status() {
        let (t, log) = tts();
        let fake = Fake::ok("SHOULD NOT BE USED", Emotion::Excited, 0);
        let st = Statuses::new();
        run(
            &t,
            settings(TtsComposerMode::Off, true),
            &fake,
            &st,
            exchange("The raw reply text."),
        );
        let s = spoken(&log);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "The raw reply text.");
        assert_eq!(s[0].emotion, Emotion::Neutral);
        assert_eq!(fake.call_count(), 0);
        assert!(st.all().is_empty());
    }

    #[test]
    fn a_recording_in_progress_means_nothing_is_composed_or_spoken() {
        let (t, log) = tts();
        t.note_recording_started();
        let fake = Fake::ok("x", Emotion::Neutral, 0);
        let st = Statuses::new();
        run(
            &t,
            settings(TtsComposerMode::Command, true),
            &fake,
            &st,
            exchange("r"),
        );
        assert!(spoken(&log).is_empty());
        assert_eq!(fake.call_count(), 0);
    }

    // =====================================================================
    // admission ordering
    // =====================================================================

    #[test]
    fn settings_are_read_after_the_epoch_is_reserved() {
        let (t, _log) = tts();
        let rx = t.epoch_receiver();
        let before = *rx.borrow();
        let seen_inside = Mutex::new(None);
        let s = settings(TtsComposerMode::Command, true);
        let load = || {
            *seen_inside.lock().unwrap() = Some(*rx.borrow());
            s.clone()
        };
        let fake = Fake::ok("x", Emotion::Neutral, 0);
        let st = Statuses::new();
        block_on(run_exchange(&t, &load, &fake, &st.sink(), exchange("r")));
        let seen = seen_inside.lock().unwrap().unwrap();
        assert_eq!(
            seen,
            before + 1,
            "begin_utterance must run before load_settings"
        );
    }

    #[test]
    fn a_writer_that_invalidates_between_reservation_and_snapshot_cancels_the_exchange() {
        let (t, log) = tts();
        let s = settings(TtsComposerMode::Command, true);
        let t2 = t.clone();
        let load = move || {
            t2.invalidate(); // a settings writer persisted and invalidated just now
            s.clone()
        };
        let fake = Fake::ok("stale", Emotion::Neutral, 0);
        let st = Statuses::new();
        block_on(run_exchange(&t, &load, &fake, &st.sink(), exchange("r")));
        assert!(spoken(&log).is_empty());
        assert_eq!(
            fake.call_count(),
            0,
            "no IO may start with a stale snapshot"
        );
    }

    // =====================================================================
    // supersession while composing
    // =====================================================================

    #[test]
    fn invalidate_mid_compose_drops_the_composer_and_never_speaks() {
        let (t, log) = tts();
        let fake = Fake::ok("late", Emotion::Neutral, 5000);
        let st = Statuses::new();
        let s = settings(TtsComposerMode::Command, true);
        let load = move || s.clone();
        let t2 = t.clone();
        let start = Instant::now();
        block_on(async {
            let bump = async {
                tokio::time::sleep(Duration::from_millis(150)).await;
                t2.invalidate();
            };
            futures_util::future::join(
                run_exchange(&t, &load, &fake, &st.sink(), exchange("r")),
                bump,
            )
            .await;
        });
        assert!(
            start.elapsed() < Duration::from_millis(2500),
            "must not wait for the composer"
        );
        assert!(spoken(&log).is_empty());
        assert_eq!(st.all(), vec!["superseded"]);
        assert_eq!(
            fake.dropped_midway.load(Ordering::SeqCst),
            1,
            "the in-flight composition is dropped"
        );
        assert_eq!(fake.completed.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn recording_started_mid_compose_prevents_late_speech() {
        let (t, log) = tts();
        let fake = Fake::ok("late", Emotion::Neutral, 400);
        let st = Statuses::new();
        let s = settings(TtsComposerMode::Command, true);
        let load = move || s.clone();
        let t2 = t.clone();
        block_on(async {
            let bump = async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                t2.note_recording_started();
            };
            futures_util::future::join(
                run_exchange(&t, &load, &fake, &st.sink(), exchange("r")),
                bump,
            )
            .await;
        });
        assert!(spoken(&log).is_empty());
        assert_eq!(st.all(), vec!["superseded"]);
    }

    #[test]
    fn a_newer_exchange_supersedes_an_older_one_and_only_the_newer_speaks() {
        let (t, log) = tts();
        let fake = Fake::new(|req| {
            if req.reply == "old" {
                (
                    1500,
                    Ok(Composed {
                        speech: "old speech".into(),
                        emotion: Emotion::Neutral,
                    }),
                )
            } else {
                (
                    50,
                    Ok(Composed {
                        speech: "new speech".into(),
                        emotion: Emotion::Calm,
                    }),
                )
            }
        });
        let st = Statuses::new();
        let s = settings(TtsComposerMode::Command, true);
        let load = move || s.clone();
        block_on(async {
            let newer = async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                run_exchange(&t, &load, &fake, &st.sink(), exchange("new")).await;
            };
            futures_util::future::join(
                run_exchange(&t, &load, &fake, &st.sink(), exchange("old")),
                newer,
            )
            .await;
        });
        let s = spoken(&log);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "new speech");
        let mut statuses = st.all();
        statuses.sort();
        assert_eq!(statuses, vec!["ok", "superseded"]);
        assert_eq!(fake.dropped_midway.load(Ordering::SeqCst), 1);
    }

    // =====================================================================
    // fallback
    // =====================================================================

    fn failing(fallback: TtsComposerFallback) -> AppSettings {
        let mut s = settings(TtsComposerMode::Command, true);
        s.tts_composer_fallback = fallback;
        s
    }

    #[test]
    fn short_notice_fallback_speaks_the_fixed_sentence_exactly_once() {
        let (t, log) = tts();
        let fake = Fake::err(ComposeError::Timeout, 0);
        let st = Statuses::new();
        run(
            &t,
            failing(TtsComposerFallback::ShortNotice),
            &fake,
            &st,
            exchange("SECRET reply"),
        );
        let s = spoken(&log);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, fallback_notice());
        assert_eq!(st.all(), vec!["timeout"]);
    }

    #[test]
    fn verbatim_fallback_speaks_the_reply_and_silent_speaks_nothing() {
        let (t, log) = tts();
        let st = Statuses::new();
        run(
            &t,
            failing(TtsComposerFallback::Verbatim),
            &Fake::err(ComposeError::Unparseable, 0),
            &st,
            exchange("The plain reply."),
        );
        assert_eq!(spoken(&log).len(), 1);
        assert_eq!(spoken(&log)[0].text, "The plain reply.");
        assert_eq!(st.all(), vec!["unparseable"]);
        let (t, log) = tts();
        let st = Statuses::new();
        run(
            &t,
            failing(TtsComposerFallback::Silent),
            &Fake::err(ComposeError::Backend, 0),
            &st,
            exchange("x"),
        );
        assert!(spoken(&log).is_empty());
        assert_eq!(st.all(), vec!["backend_error"]);
    }

    #[test]
    fn composition_failures_report_their_static_outcome_strings() {
        for (err, outcome) in [
            (ComposeError::NotConfigured, "not_configured"),
            (ComposeError::NeedsAcknowledgement, "needs_acknowledgement"),
            (ComposeError::OutputTooLarge, "output_too_large"),
            (ComposeError::Empty, "empty"),
        ] {
            let (t, _log) = tts();
            let st = Statuses::new();
            run(
                &t,
                failing(TtsComposerFallback::Silent),
                &Fake::err(err, 0),
                &st,
                exchange("x"),
            );
            assert_eq!(st.all(), vec![outcome]);
        }
    }

    #[test]
    fn a_fallback_refused_because_it_was_superseded_does_not_speak() {
        let (t, log) = tts();
        let fake = Fake::err(ComposeError::Timeout, 300);
        let st = Statuses::new();
        let s = failing(TtsComposerFallback::ShortNotice);
        let load = move || s.clone();
        let t2 = t.clone();
        block_on(async {
            let bump = async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                t2.invalidate();
            };
            futures_util::future::join(
                run_exchange(&t, &load, &fake, &st.sink(), exchange("r")),
                bump,
            )
            .await;
        });
        assert!(spoken(&log).is_empty());
        assert_eq!(st.all(), vec!["superseded"]);
    }

    #[test]
    fn a_speech_backend_failure_is_reported_as_backend_error() {
        let (t, log) = tts();
        log.fail.store(true, Ordering::SeqCst);
        let st = Statuses::new();
        run(
            &t,
            settings(TtsComposerMode::Command, true),
            &Fake::ok("x", Emotion::Neutral, 0),
            &st,
            exchange("r"),
        );
        assert_eq!(st.all(), vec!["backend_error"]);
    }

    // =====================================================================
    // preview
    // =====================================================================

    fn preview(t: &TtsManager, s: AppSettings, c: &Fake) -> Result<ComposerPreview, String> {
        let load = move || s.clone();
        block_on(run_preview(t, &load, c))
    }

    #[test]
    fn preview_composes_the_fixed_sample_without_the_user_utterance_and_speaks_when_enabled() {
        let (t, log) = tts();
        let fake = Fake::ok("Three tests failed.", Emotion::Concerned, 0);
        let p = preview(&t, settings(TtsComposerMode::Command, true), &fake).unwrap();
        assert_eq!(
            p,
            ComposerPreview {
                speech: "Three tests failed.".into(),
                emotion: Emotion::Concerned
            }
        );
        let calls = fake.calls.lock().unwrap().clone();
        assert_eq!(calls[0].reply, PREVIEW_SAMPLE);
        assert_eq!(calls[0].user_utterance, None);
        let s = spoken(&log);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].emotion, Emotion::Concerned);
    }

    #[test]
    fn preview_composes_but_stays_silent_when_tts_is_disabled() {
        let (t, log) = tts();
        let fake = Fake::ok("Quiet.", Emotion::Calm, 0);
        let p = preview(&t, settings(TtsComposerMode::Command, false), &fake).unwrap();
        assert_eq!(p.speech, "Quiet.");
        assert!(spoken(&log).is_empty());
    }

    #[test]
    fn preview_error_messages_are_exact_and_short() {
        let cases: Vec<(ComposeError, TtsComposerMode, &str)> = vec![
            (
                ComposeError::Timeout,
                TtsComposerMode::Command,
                "Timed out after 45s",
            ),
            (
                ComposeError::Unparseable,
                TtsComposerMode::Command,
                "The model returned nothing usable",
            ),
            (
                ComposeError::Empty,
                TtsComposerMode::Command,
                "The model returned nothing usable",
            ),
            (
                ComposeError::NeedsAcknowledgement,
                TtsComposerMode::Command,
                "Command needs acknowledgement",
            ),
            (
                ComposeError::NotConfigured,
                TtsComposerMode::Model,
                "Pick a model first",
            ),
            (
                ComposeError::NotConfigured,
                TtsComposerMode::Command,
                "Not configured",
            ),
            (
                ComposeError::BadConfig,
                TtsComposerMode::Command,
                "Not configured",
            ),
            (
                ComposeError::Backend,
                TtsComposerMode::Model,
                "The model or command failed",
            ),
            (
                ComposeError::OutputTooLarge,
                TtsComposerMode::Command,
                "The output was too large",
            ),
        ];
        for (err, mode, msg) in cases {
            let (t, _log) = tts();
            let r = preview(&t, settings(mode, true), &Fake::err(err, 0));
            assert_eq!(r, Err(msg.to_string()), "{err:?}");
        }
        let (t, _log) = tts();
        assert_eq!(
            preview(
                &t,
                settings(TtsComposerMode::Off, true),
                &Fake::ok("x", Emotion::Neutral, 0)
            ),
            Err("Choose a mode first".to_string())
        );
    }

    #[test]
    fn preview_refuses_during_a_recording_without_composing() {
        let (t, log) = tts();
        t.note_recording_started();
        let fake = Fake::ok("x", Emotion::Neutral, 0);
        assert_eq!(
            preview(&t, settings(TtsComposerMode::Command, true), &fake),
            Err("Recording in progress".to_string())
        );
        assert_eq!(fake.call_count(), 0);
        assert!(spoken(&log).is_empty());
    }

    #[test]
    fn a_newer_preview_supersedes_the_older_one() {
        let (t, log) = tts();
        let fake = Fake::new(|_| {
            (
                300,
                Ok(Composed {
                    speech: "sample".into(),
                    emotion: Emotion::Neutral,
                }),
            )
        });
        let s = settings(TtsComposerMode::Command, true);
        let load = move || s.clone();
        let (first, second) = block_on(async {
            let newer = async {
                tokio::time::sleep(Duration::from_millis(100)).await;
                run_preview(&t, &load, &fake).await
            };
            futures_util::future::join(run_preview(&t, &load, &fake), newer).await
        });
        assert_eq!(first, Err("Replaced by newer speech".to_string()));
        assert!(second.is_ok());
        assert_eq!(spoken(&log).len(), 1);
    }
}

#[cfg(test)]
mod payload_tests {
    use super::*;

    #[test]
    fn payload_round_trips_through_parse_exchange() {
        let v = build_exchange_payload("All done", Some("what is the status"));
        let ex = parse_exchange(&v.to_string()).unwrap();
        assert_eq!(ex.reply, "All done");
        assert_eq!(ex.utterance.as_deref(), Some("what is the status"));
    }

    #[test]
    fn blank_or_missing_utterance_is_null() {
        for u in [None, Some(""), Some("  \n ")] {
            let v = build_exchange_payload("r", u);
            assert!(v["utterance"].is_null());
            assert_eq!(parse_exchange(&v.to_string()).unwrap().utterance, None);
        }
    }

    #[test]
    fn long_utterances_are_truncated_on_a_char_boundary() {
        let long = "\u{1f600}".repeat(900);
        let v = build_exchange_payload("r", Some(&long));
        assert_eq!(
            v["utterance"].as_str().unwrap().chars().count(),
            MAX_EXCHANGE_UTTERANCE_CHARS
        );
    }

    #[test]
    fn unicode_and_quotes_survive_the_json_encoding() {
        let v = build_exchange_payload("caf\u{e9} \"quoted\" \n line", None);
        assert_eq!(
            parse_exchange(&v.to_string()).unwrap().reply,
            "caf\u{e9} \"quoted\" \n line"
        );
    }
}
