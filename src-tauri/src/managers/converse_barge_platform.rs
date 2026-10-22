//! Stage B3b (DESIGN Rev 8, 12.2.6, 12.3, 12.7): production adapters of the barge-in monitor
//! core, bound to the app. The decisions (enablement, config, fail-soft start, verdict mapping,
//! slot ownership) are pure and tested in converse_barge_platform_tests.rs. The device, ONNX and
//! TTS paths in this file are exercised only live (no audio hardware in the test suite).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tauri::path::BaseDirectory;
use tauri::{AppHandle, Manager};

use crate::audio_toolkit::{AudioRecorder, SileroVad, VadPolicy, VoiceActivityDetector};
use crate::managers::audio::{SILERO_VAD_RESOURCE, SILERO_VAD_THRESHOLD};
use crate::managers::composer_config::{settings_from_app, TtsComposerMode};
use crate::managers::converse::BoxFuture;
use crate::managers::converse_audio::EndpointConfig;
use crate::managers::converse_barge::BargeIn;
use crate::managers::converse_barge_monitor::{
    BargeConfig, BargeMonitor, ClipGate, ClipTranscriber, MonitorEffects, MonitorInput,
    MonitorStream, SampleCallback, VadFactory,
};
use crate::managers::converse_capture::Transcriber;
use crate::managers::natural_sink;
use crate::managers::realtime_config::RealtimeConfig;
use crate::managers::speaker_id::{SpeakerGate, Verdict};
use crate::managers::tts::TtsManager;
use crate::managers::wakeword_listener::resolve_device;
use crate::settings::{get_settings, AppSettings, ConverseBargeIn};

/// Duck heartbeat cadence while Ducked (the sink's watchdog is 3 s; DESIGN 12.2.7).
pub const HEARTBEAT_MS: u64 = 1000;
/// Audio frames queued between the stream callback and the monitor worker (DESIGN 12.2.6).
pub const CHANNEL_FRAMES: usize = 64;

/// Barge-in is on only for the explicit headphones setting, with spoken replies and a reply
/// backend. The same backend rule as `preflight_check`, so the monitor never runs in a session
/// the preflight would refuse. Read once per session (frozen at start by the caller).
pub fn barge_enabled(settings: &AppSettings) -> bool {
    settings.converse_barge_in == ConverseBargeIn::Headphones
        && settings.tts_enabled
        && has_reply_backend(settings)
}

fn has_reply_backend(settings: &AppSettings) -> bool {
    let direct = RealtimeConfig::from_settings(settings).direct_endpoint;
    direct || settings_from_app(settings).mode != TtsComposerMode::Off
}

/// Upper bound on the silence that ends an interrupting utterance (DESIGN 12.2.6): a user who
/// configured a long pause for normal turns must not wait that long before a barge-in is heard.
pub const MAX_BARGE_SILENCE_MS: u64 = 1500;

/// The monitor endpoints like a normal turn (same length limit, same silence up to the clamp).
pub fn barge_config(endpoint: &EndpointConfig) -> BargeConfig {
    BargeConfig {
        endpoint_silence_ms: endpoint.silence_ms.min(MAX_BARGE_SILENCE_MS),
        max_utterance_ms: endpoint.max_utterance_ms,
        heartbeat_ms: HEARTBEAT_MS,
        channel_frames: CHANNEL_FRAMES,
        stall_ms: 1000,
    }
}

/// Starts the monitor only when enabled. Any failure (including a panic while building) is
/// logged with its reason and yields `None`: the session then runs exactly as half-duplex.
pub fn start_monitor_fail_soft(
    enabled: bool,
    build: impl FnOnce() -> Result<Arc<BargeMonitor>, String>,
) -> Option<Arc<BargeMonitor>> {
    if !enabled {
        return None;
    }
    let outcome = catch_unwind(AssertUnwindSafe(build))
        .unwrap_or_else(|_| Err("starting the monitor panicked".to_string()));
    match outcome {
        Ok(monitor) => Some(monitor),
        Err(reason) => {
            log::warn!("barge-in unavailable this session, continuing half-duplex: {reason}");
            None
        }
    }
}

/// The session's monitor (or none). Owned by `RealEars`; the finalizer shuts it down.
#[derive(Default)]
pub struct BargeSlot {
    monitor: Option<Arc<BargeMonitor>>,
}

impl BargeSlot {
    pub fn new(monitor: Option<Arc<BargeMonitor>>) -> Self {
        Self { monitor }
    }

    /// The barge-in view for the conversation loop; `None` when there is no monitor.
    pub fn view(&self) -> Option<&dyn BargeIn> {
        self.monitor
            .as_deref()
            .map(|monitor| monitor as &dyn BargeIn)
    }

    /// Stops the worker and closes the input stream. Idempotent.
    pub fn shutdown(&self) {
        if let Some(monitor) = self.monitor.as_ref() {
            monitor.shutdown();
        }
    }
}

/// A speaker-gate verdict as the monitor needs it: only a Reject discards the clip.
pub fn accepts_verdict(verdict: Verdict) -> bool {
    verdict != Verdict::Reject
}

/// Speaker gate for a barge clip, the same gate object the capture path uses.
pub struct SpeakerClipGate {
    gate: Arc<dyn SpeakerGate>,
}

impl SpeakerClipGate {
    pub fn new(gate: Arc<dyn SpeakerGate>) -> Self {
        Self { gate }
    }
}

impl ClipGate for SpeakerClipGate {
    fn accepts(&self, samples: &[f32]) -> bool {
        // A fresh flag that is never set: the clip is judged to completion, like a turn.
        let cancel = AtomicBool::new(false);
        accepts_verdict(self.gate.check(samples, &cancel).verdict)
    }
}

/// Transcribes a barge clip with the session's transcription handle.
pub struct TranscriberClip {
    transcriber: Arc<dyn Transcriber>,
}

impl TranscriberClip {
    pub fn new(transcriber: Arc<dyn Transcriber>) -> Self {
        Self { transcriber }
    }
}

impl ClipTranscriber for TranscriberClip {
    fn transcribe(&self, samples: Vec<f32>) -> BoxFuture<'static, Result<String, String>> {
        // Called on the monitor's clip thread, never on the async executor: the blocking call
        // happens here and the ready future only carries its result.
        let result = self
            .transcriber
            .transcribe(samples)
            .map_err(|_| "transcription failed".to_string());
        Box::pin(std::future::ready(result))
    }
}

/// Second input stream for the monitor: a recorder with the VAD disabled, exactly like the
/// wake-word listener's stream (the recorder's own VAD would drop frames the monitor needs).
pub struct RecorderInput {
    app: AppHandle,
}

impl RecorderInput {
    pub fn new(app: AppHandle) -> Self {
        Self { app }
    }
}

impl MonitorInput for RecorderInput {
    fn open(&self, on_samples: SampleCallback) -> Result<Box<dyn MonitorStream>, String> {
        let settings = get_settings(&self.app);
        let device = resolve_device(&settings)?;
        // The recorder has no error callback: a failed stream is rebuilt inside the recorder
        // and never reported here, so `report_input_failure` is not wired (see the report).
        let mut recorder = AudioRecorder::new()
            .map_err(|error| error.to_string())?
            .with_audio_callback(move |frame: &[f32]| on_samples(frame));
        recorder.open(device).map_err(|error| error.to_string())?;
        if let Err(error) = recorder.start(VadPolicy::Disabled) {
            let _ = recorder.close();
            return Err(error.to_string());
        }
        Ok(Box::new(RecorderStream { recorder }))
    }
}

struct RecorderStream {
    recorder: AudioRecorder,
}

impl MonitorStream for RecorderStream {
    fn close(mut self: Box<Self>) {
        if let Err(error) = self.recorder.close() {
            log::warn!("barge-in input close failed: {error}");
        }
    }
}

/// Builds the same Silero VAD the recorder uses (same model resource and threshold).
pub fn silero_vad_factory(app: AppHandle) -> VadFactory {
    Box::new(move || {
        let path = app
            .path()
            .resolve(SILERO_VAD_RESOURCE, BaseDirectory::Resource)
            .map_err(|error| format!("VAD model path: {error}"))?;
        let vad = SileroVad::new(path, SILERO_VAD_THRESHOLD)
            .map_err(|error| format!("SileroVad: {error}"))?;
        let detector: Box<dyn VoiceActivityDetector> = Box::new(vad);
        Ok(detector)
    })
}

/// Playback side of the monitor. The system voice runs in an external process, so `duck` is a
/// harmless gain change on in-process audio only; `stop_speech` still cancels any engine.
pub struct TtsEffects {
    tts: Arc<TtsManager>,
}

impl TtsEffects {
    pub fn new(tts: Arc<TtsManager>) -> Self {
        Self { tts }
    }
}

impl MonitorEffects for TtsEffects {
    /// `TtsManager::is_speaking` covers in-process Natural/Phonon playback: their handles are
    /// router handles whose `is_running` reports the engine's busy or audible state.
    fn speaking(&self) -> bool {
        self.tts.is_speaking()
    }

    fn duck(&self) {
        natural_sink::duck();
    }

    fn restore(&self) {
        natural_sink::restore();
    }

    fn stop_speech(&self) {
        let _ = self.tts.stop();
    }
}

/// Builds the production monitor. Every failure is returned as a reason (the caller is
/// fail-soft).
pub fn build_monitor(
    app: &AppHandle,
    transcriber: Arc<dyn Transcriber>,
    gate: Option<Arc<dyn SpeakerGate>>,
    cfg: BargeConfig,
) -> Result<Arc<BargeMonitor>, String> {
    let tts = app
        .try_state::<Arc<TtsManager>>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| "text-to-speech is not available".to_string())?;
    let input: Arc<dyn MonitorInput> = Arc::new(RecorderInput::new(app.clone()));
    let clip_gate: Option<Arc<dyn ClipGate>> =
        gate.map(|gate| Arc::new(SpeakerClipGate::new(gate)) as Arc<dyn ClipGate>);
    BargeMonitor::start(
        input,
        silero_vad_factory(app.clone()),
        Arc::new(TtsEffects::new(tts)),
        Arc::new(TranscriberClip::new(transcriber)),
        clip_gate,
        cfg,
    )
}
