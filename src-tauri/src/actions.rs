#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
use crate::apple_intelligence;
use crate::audio_feedback::{play_feedback_sound, play_feedback_sound_blocking, SoundType};
use crate::audio_toolkit::{is_microphone_access_denied, is_no_input_device_error, VadPolicy};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::command_router::{CommandAction, CommandRouter, RouteDecision};
use crate::managers::conversation::{
    build_agent_prompt, should_append_exchange, ConversationLog, Ticket,
};
use crate::managers::history::HistoryManager;
use crate::managers::media_control::{
    decide_media_command, run_media_hook, MediaHookResult, OsascriptRunner, ScriptRunner,
};
use crate::managers::memory::MemoryManager;
use crate::managers::model::ModelManager;
use crate::managers::transcription::StreamWorkKind;
use crate::managers::transcription::TranscriptionManager;
use crate::managers::voice_common::{HookEvent, VoiceHookOutcome};
use crate::managers::voice_shortcuts::{run_shortcut_hook, ShortcutOpener};
use crate::settings::{get_settings, AppSettings, OverlayStyle, APPLE_INTELLIGENCE_PROVIDER_ID};
use crate::shortcut;
use crate::tray::{set_tray_state, TrayIconState};
use crate::utils::{
    self, show_processing_overlay, show_recording_overlay, show_transcribing_overlay,
};
use crate::TranscriptionCoordinator;
use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Manager;
use tauri::{AppHandle, Emitter};
use tauri_plugin_opener::OpenerExt;

const CANCELLATION_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Clone, serde::Serialize)]
struct RecordingErrorEvent {
    error_type: String,
    detail: Option<String>,
}

/// Drop guard that finishes the transcription pipeline, including immediate
/// model unloading on early exits.
struct FinishGuard(AppHandle, Arc<TranscriptionManager>);
impl Drop for FinishGuard {
    fn drop(&mut self) {
        self.1.maybe_unload_immediately("transcription session");
        if let Some(c) = self.0.try_state::<TranscriptionCoordinator>() {
            c.notify_processing_finished();
        }
        // The pipeline just freed its large transient buffers (captured PCM,
        // WAV copy, engine scratch); hand the cached pages back to the OS so
        // they don't sit in malloc arenas until they get swapped out (#1792).
        crate::memory::trim_freed_memory();
    }
}

// Shortcut Action Trait
pub trait ShortcutAction: Send + Sync {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str);
}

// Transcribe Action
struct TranscribeAction {
    post_process: bool,
}

/// Field name for structured output JSON schema
const TRANSCRIPTION_FIELD: &str = "transcription";

/// Strip invisible Unicode characters that some LLMs may insert
fn strip_invisible_chars(s: &str) -> String {
    s.replace(['\u{200B}', '\u{200C}', '\u{200D}', '\u{FEFF}'], "")
}

/// Strip a leading `<think>...</think>` block. Some endpoints can't disable
/// reasoning, and some local servers put the reasoning text into `content`
/// instead of a separate field — without this the user would get the model's
/// chain of thought pasted along with the cleaned transcription.
fn strip_think_block(s: &str) -> &str {
    if let Some(rest) = s.trim_start().strip_prefix("<think>") {
        if let Some(end) = rest.find("</think>") {
            return rest[end + "</think>".len()..].trim_start();
        }
    }
    s
}

/// Build a system prompt from the user's prompt template.
/// Removes `${output}` placeholder since the transcription is sent as the user message.
fn build_system_prompt(prompt_template: &str) -> String {
    prompt_template.replace("${output}", "").trim().to_string()
}

/// Returns `true` when a transcription has no meaningful content to
/// post-process (empty or whitespace-only). Used to skip the post-processing
/// LLM call when nothing was actually transcribed, which would otherwise make
/// the model reply with an error message such as "you need to provide the
/// transcription".
fn is_blank_transcription(transcription: &str) -> bool {
    transcription.trim().is_empty()
}

/// Gate for the voice-command hook (`voice_commands_enabled` setting,
/// default off). Pure with respect to I/O -- `CommandRouter::route` itself
/// does the real matching; this just applies the enable/disable gate so
/// the behavior is independently testable without a running app.
#[cfg(test)]
fn decide_voice_command(
    router: &CommandRouter,
    voice_commands_enabled: bool,
    transcript: &str,
) -> Option<CommandAction> {
    if !voice_commands_enabled {
        return None;
    }
    match router.route(transcript) {
        RouteDecision::Matched { action } => Some(action),
        RouteDecision::NoMatch => None,
    }
}

/// Whether a media-hook result means the utterance was consumed: a matched
/// media command (success or failure) is never pasted and never escalated to
/// the router or agent bridge. `None` means it was not a media command.
fn media_hook_consumes_utterance(result: Option<&MediaHookResult>) -> bool {
    result.is_some_and(|result| result.outcome.skip_paste)
}

/// Which pre-router voice hook consumed an utterance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookSource {
    /// A user-defined voice shortcut.
    Shortcut,
    /// A whole-utterance media control.
    Media,
}

/// A pre-router voice hook that consumed the utterance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UtteranceHookResult {
    /// Which hook matched.
    pub source: HookSource,
    /// What the pipeline must do with the transcript (matched hooks never
    /// paste and never escalate to the agent, even when they failed).
    pub outcome: VoiceHookOutcome,
    /// The event to emit (`voice-command-executed` / `voice-command-error`).
    pub event: HookEvent,
}

/// Runs the pre-router voice hooks in their FIXED order: custom voice
/// shortcut first, then media control. The first hook that matches consumes
/// the utterance (success OR failure) and later hooks never run. `None` means
/// nothing matched, so the caller continues to the built-in router and,
/// only on a genuine router `NoMatch`, the agent bridge. Both hooks are gated
/// on `voice_commands_enabled`; media additionally on
/// `voice_media_controls_enabled`. Blocking (media can take seconds): call
/// from a blocking thread.
pub(crate) fn run_utterance_hooks(
    _settings: &AppSettings,
    _transcript: &str,
    _router: &CommandRouter,
    _home: Option<&Path>,
    _opener: &dyn ShortcutOpener,
    _media_runner: &dyn ScriptRunner,
) -> Option<UtteranceHookResult> {
    todo!("integration: implement")
}

fn should_escalate_to_agent_bridge(
    route: &RouteDecision,
    voice_commands_enabled: bool,
    agent_bridge_enabled: bool,
) -> bool {
    voice_commands_enabled && agent_bridge_enabled && matches!(route, RouteDecision::NoMatch)
}

/// Monotonic milliseconds since the first call. Conversation-log expiry uses
/// this so wall-clock jumps cannot expire or resurrect context.
fn monotonic_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let start = START.get_or_init(Instant::now);
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Builds the prompt for an agent request. When context is disabled the
/// transcript is returned unchanged and `recall` is not called. When enabled,
/// a [`Ticket`] is allocated for the exchange; `recall` runs only when
/// `memory_enabled` is set.
fn prepare_agent_prompt(
    context_enabled: bool,
    memory_enabled: bool,
    transcript: &str,
    recall: &dyn Fn(&str) -> Vec<String>,
    log: &ConversationLog,
    now_ms: u64,
) -> (String, Option<Ticket>) {
    if !context_enabled {
        return (transcript.to_string(), None);
    }
    let ticket = log.begin();
    let facts = if memory_enabled {
        recall(transcript)
    } else {
        Vec::new()
    };
    let turns = log.recent(now_ms);
    (build_agent_prompt(transcript, &facts, &turns), Some(ticket))
}

/// Records a finished exchange (original transcript and reply) only when the
/// ticket is present, no clear happened since dispatch, and context is still
/// enabled. Returns whether it was recorded.
fn finish_agent_exchange(
    ticket: Option<Ticket>,
    log: &ConversationLog,
    context_enabled_now: bool,
    transcript: &str,
    reply: &str,
    now_ms: u64,
) -> bool {
    match ticket {
        Some(ticket) if should_append_exchange(ticket.epoch, log.epoch(), context_enabled_now) => {
            log.record(ticket, transcript, reply, now_ms)
        }
        _ => false,
    }
}

/// Gate for the memory-capture hook (`memory_enabled` setting, default
/// off). Same reasoning as `decide_voice_command` above.
fn decide_memory_fact(memory_enabled: bool, transcript: &str) -> Option<String> {
    if !memory_enabled {
        return None;
    }
    MemoryManager::extract_fact(transcript)
}

/// Adapts the Tauri opener to the `ShortcutOpener` used by voice shortcuts.
struct AppShortcutOpener<'a>(&'a AppHandle);

impl ShortcutOpener for AppShortcutOpener<'_> {
    fn open_url(&self, url: &str) -> Result<(), String> {
        self.0
            .opener()
            .open_url(url.to_string(), None::<String>)
            .map_err(|e| format!("Failed to open the URL: {e}"))
    }

    fn open_path(&self, path: &Path) -> Result<(), String> {
        self.0
            .opener()
            .open_path(path.to_string_lossy().to_string(), None::<String>)
            .map_err(|e| format!("Failed to open the folder: {e}"))
    }

    fn open_app(&self, app_path: &Path) -> Result<(), String> {
        self.0
            .opener()
            .open_path(app_path.to_string_lossy().to_string(), None::<String>)
            .map_err(|e| format!("Failed to open the app: {e}"))
    }
}

/// Performs a matched voice command via the OS opener (never arbitrary
/// shell execution -- mirrors the phase doc's non-goal). Returns a short
/// human-readable description for the confirmation toast.
fn execute_command_action(app: &AppHandle, action: &CommandAction) -> Result<String, String> {
    match action {
        CommandAction::OpenApp {
            name,
            resolved_path,
        } => {
            let path = resolved_path.to_string_lossy().to_string();
            app.opener()
                .open_path(path, None::<String>)
                .map(|_| format!("Opened {name}"))
                .map_err(|e| format!("Failed to open {name}: {e}"))
        }
        CommandAction::OpenPath { path } => {
            let display = path.to_string_lossy().to_string();
            app.opener()
                .open_path(display.clone(), None::<String>)
                .map(|_| format!("Opened {display}"))
                .map_err(|e| format!("Failed to open {display}: {e}"))
        }
        CommandAction::OpenUrl { url } => app
            .opener()
            .open_url(url.clone(), None::<String>)
            .map(|_| format!("Opened {url}"))
            .map_err(|e| format!("Failed to open {url}: {e}")),
    }
}

async fn complete_unless_cancelled<F, C>(operation: F, is_cancelled: C) -> Option<F::Output>
where
    F: Future,
    C: Fn() -> bool,
{
    tokio::pin!(operation);

    loop {
        if is_cancelled() {
            return None;
        }

        if let Ok(result) =
            tokio::time::timeout(CANCELLATION_POLL_INTERVAL, operation.as_mut()).await
        {
            return Some(result);
        }
    }
}

fn should_use_streaming_overlay(style: OverlayStyle, is_streaming: bool) -> bool {
    style == OverlayStyle::Live && is_streaming
}

async fn post_process_transcription(settings: &AppSettings, transcription: &str) -> Option<String> {
    if is_blank_transcription(transcription) {
        debug!("Post-processing skipped because the transcription is empty");
        return None;
    }

    let provider = match settings.active_post_process_provider().cloned() {
        Some(provider) => provider,
        None => {
            debug!("Post-processing enabled but no provider is selected");
            return None;
        }
    };

    let model = settings
        .post_process_models
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    if model.trim().is_empty() {
        debug!(
            "Post-processing skipped because provider '{}' has no model configured",
            provider.id
        );
        return None;
    }

    let selected_prompt_id = match &settings.post_process_selected_prompt_id {
        Some(id) => id.clone(),
        None => {
            debug!("Post-processing skipped because no prompt is selected");
            return None;
        }
    };

    let prompt = match settings
        .post_process_prompts
        .iter()
        .find(|prompt| prompt.id == selected_prompt_id)
    {
        Some(prompt) => prompt.prompt.clone(),
        None => {
            debug!(
                "Post-processing skipped because prompt '{}' was not found",
                selected_prompt_id
            );
            return None;
        }
    };

    if prompt.trim().is_empty() {
        debug!("Post-processing skipped because the selected prompt is empty");
        return None;
    }

    debug!(
        "Starting LLM post-processing with provider '{}' (model: {})",
        provider.id, model
    );

    let api_key = settings
        .post_process_api_keys
        .get(&provider.id)
        .cloned()
        .unwrap_or_default();

    // Ask these providers to skip reasoning/thinking — post-processing rarely
    // benefits from it and it adds seconds of latency. llm_client picks the
    // field the endpoint understands and retries without it if rejected.
    let disable_reasoning = matches!(provider.id.as_str(), "custom" | "openrouter");

    if provider.supports_structured_output {
        debug!("Using structured outputs for provider '{}'", provider.id);

        let system_prompt = build_system_prompt(&prompt);
        let user_content = transcription.to_string();

        // Handle Apple Intelligence separately since it uses native Swift APIs
        if provider.id == APPLE_INTELLIGENCE_PROVIDER_ID {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            {
                if !apple_intelligence::check_apple_intelligence_availability() {
                    debug!(
                        "Apple Intelligence selected but not currently available on this device"
                    );
                    return None;
                }

                let token_limit = model.trim().parse::<i32>().unwrap_or(0);
                return match apple_intelligence::process_text_with_system_prompt(
                    &system_prompt,
                    &user_content,
                    token_limit,
                ) {
                    Ok(result) => {
                        if result.trim().is_empty() {
                            debug!("Apple Intelligence returned an empty response");
                            None
                        } else {
                            let result = strip_invisible_chars(&result);
                            debug!(
                                "Apple Intelligence post-processing succeeded. Output length: {} chars",
                                result.len()
                            );
                            Some(result)
                        }
                    }
                    Err(err) => {
                        error!("Apple Intelligence post-processing failed: {}", err);
                        None
                    }
                };
            }

            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            {
                debug!("Apple Intelligence provider selected on unsupported platform");
                return None;
            }
        }

        // Define JSON schema for transcription output
        let json_schema = serde_json::json!({
            "type": "object",
            "properties": {
                (TRANSCRIPTION_FIELD): {
                    "type": "string",
                    "description": "The cleaned and processed transcription text"
                }
            },
            "required": [TRANSCRIPTION_FIELD],
            "additionalProperties": false
        });

        match crate::llm_client::send_chat_completion_with_schema(
            &provider,
            api_key.clone(),
            &model,
            user_content,
            Some(system_prompt),
            Some(json_schema),
            disable_reasoning,
        )
        .await
        {
            Ok(Some(content)) => {
                // Parse the JSON response to extract the transcription field
                let content = strip_think_block(&content);
                match serde_json::from_str::<serde_json::Value>(content) {
                    Ok(json) => {
                        if let Some(transcription_value) =
                            json.get(TRANSCRIPTION_FIELD).and_then(|t| t.as_str())
                        {
                            let result = strip_invisible_chars(transcription_value);
                            debug!(
                                "Structured output post-processing succeeded for provider '{}'. Output length: {} chars",
                                provider.id,
                                result.len()
                            );
                            return Some(result);
                        } else {
                            error!("Structured output response missing 'transcription' field");
                            return Some(strip_invisible_chars(content));
                        }
                    }
                    Err(e) => {
                        error!(
                            "Failed to parse structured output JSON: {}. Returning raw content.",
                            e
                        );
                        return Some(strip_invisible_chars(content));
                    }
                }
            }
            Ok(None) => {
                error!("LLM API response has no content");
                return None;
            }
            Err(e) => {
                warn!(
                    "Structured output failed for provider '{}': {}. Falling back to legacy mode.",
                    provider.id, e
                );
                // Fall through to legacy mode below
            }
        }
    }

    // Legacy mode: Replace ${output} variable in the prompt with the actual text
    let processed_prompt = prompt.replace("${output}", transcription);
    debug!("Processed prompt length: {} chars", processed_prompt.len());

    match crate::llm_client::send_chat_completion(
        &provider,
        api_key,
        &model,
        processed_prompt,
        disable_reasoning,
    )
    .await
    {
        Ok(Some(content)) => {
            let content = strip_invisible_chars(strip_think_block(&content));
            debug!(
                "LLM post-processing succeeded for provider '{}'. Output length: {} chars",
                provider.id,
                content.len()
            );
            Some(content)
        }
        Ok(None) => {
            error!("LLM API response has no content");
            None
        }
        Err(e) => {
            error!(
                "LLM post-processing failed for provider '{}': {}. Falling back to original transcription.",
                provider.id,
                e
            );
            None
        }
    }
}

pub(crate) struct ProcessedTranscription {
    pub final_text: String,
    pub post_processed_text: Option<String>,
    pub post_process_prompt: Option<String>,
}

pub(crate) async fn process_transcription_output(
    app: &AppHandle,
    transcription: &str,
    post_process: bool,
) -> ProcessedTranscription {
    let settings = get_settings(app);
    let mut final_text = transcription.to_string();
    let mut post_processed_text: Option<String> = None;
    let mut post_process_prompt: Option<String> = None;

    if post_process {
        if let Some(processed_text) = post_process_transcription(&settings, &final_text).await {
            post_processed_text = Some(processed_text.clone());
            final_text = processed_text;

            if let Some(prompt_id) = &settings.post_process_selected_prompt_id {
                if let Some(prompt) = settings
                    .post_process_prompts
                    .iter()
                    .find(|prompt| &prompt.id == prompt_id)
                {
                    post_process_prompt = Some(prompt.prompt.clone());
                }
            }
        }
    }

    ProcessedTranscription {
        final_text,
        post_processed_text,
        post_process_prompt,
    }
}

impl ShortcutAction for TranscribeAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        let start_time = Instant::now();
        debug!("TranscribeAction::start called for binding: {}", binding_id);

        // Load model in the background
        let tm = app.state::<Arc<TranscriptionManager>>();
        let rm = app.state::<Arc<AudioRecordingManager>>();

        // Load ASR model and VAD model in parallel
        let kickoff_started = Instant::now();
        tm.initiate_model_load();
        let rm_clone = Arc::clone(&rm);
        std::thread::spawn(move || {
            if let Err(e) = rm_clone.preload_vad() {
                debug!("VAD pre-load failed: {}", e);
            }
        });
        let kickoff_elapsed = kickoff_started.elapsed();

        // Don't open the mic if nothing can transcribe the recording; the load
        // kicked off above fails and reports why.
        if !tm.is_model_loaded() {
            let selected_model = get_settings(app).selected_model;
            if let Err(e) = app
                .state::<Arc<ModelManager>>()
                .get_model_path(&selected_model)
            {
                warn!("Not starting recording: no model can transcribe it ({})", e);
                return;
            }
        }

        let binding_id = binding_id.to_string();
        let tray_started = Instant::now();
        set_tray_state(app, TrayIconState::Recording);
        let tray_elapsed = tray_started.elapsed();

        // Get the microphone mode to determine audio feedback timing
        let plan_started = Instant::now();
        let settings = get_settings(app);
        crate::managers::wakeword_listener::on_recording_started(
            shortcut_str,
            settings.wake_word_silence_timeout_ms,
        );
        let is_always_on = settings.always_on_microphone;

        let selected_model_info = app
            .state::<Arc<ModelManager>>()
            .get_model_info(&settings.selected_model);

        // Use the app-facing model capability as the single pre-recording source
        // for live streaming decisions. Unknown support is represented as false
        // until the model registry is updated by discovery or runtime load.
        let model_supports_streaming = selected_model_info
            .as_ref()
            .map(|m| m.supports_streaming)
            .unwrap_or(false);
        let vad_policy = if !settings.vad_enabled {
            VadPolicy::Disabled
        } else if model_supports_streaming {
            VadPolicy::Streaming
        } else {
            VadPolicy::Offline
        };
        if model_supports_streaming {
            tm.start_stream();
        }
        let plan_elapsed = plan_started.elapsed();

        // Sizing the overlay follows the same advertised capability. A model that
        // doesn't stream (or whose capability is not known yet) gets the compact
        // pill instead of an oversized transparent live window.
        let overlay_started = Instant::now();
        match settings.overlay_style {
            OverlayStyle::Live if model_supports_streaming => utils::show_streaming_overlay(app),
            OverlayStyle::Live | OverlayStyle::Minimal => show_recording_overlay(app),
            OverlayStyle::None => {} // show_overlay_state no-ops on None anyway
        }
        // Everything above runs before capture can begin, so each span here is
        // added keypress->capture latency.
        debug!(
            "start-path pre-recording steps: model_kickoff={:?} tray={:?} settings+stream_plan={:?} overlay={:?}",
            kickoff_elapsed,
            tray_elapsed,
            plan_elapsed,
            overlay_started.elapsed()
        );
        debug!("Microphone mode - always_on: {}", is_always_on);

        let mut recording_error: Option<String> = None;
        let recording_start_time = Instant::now();
        match rm.try_start_recording(&binding_id, vad_policy) {
            Ok(readiness) => {
                debug!(
                    "Recording request accepted in {:?}; waiting for first microphone samples",
                    recording_start_time.elapsed()
                );
                let generation = readiness.generation();
                let app_clone = app.clone();
                let rm_clone = Arc::clone(&rm);
                std::thread::spawn(move || {
                    if !readiness.wait() {
                        debug!("Microphone readiness wait ended without receiving samples");
                        return;
                    }

                    // Development-only preview hook for evaluating the brief
                    // arming animation on hardware that normally starts too fast
                    // to make it visible.
                    #[cfg(debug_assertions)]
                    if let Ok(delay_ms) = std::env::var("HANDY_DEBUG_MIC_READY_DELAY_MS")
                        .unwrap_or_default()
                        .parse::<u64>()
                    {
                        let delay_ms = delay_ms.min(10_000);
                        if delay_ms > 0 {
                            debug!("Delaying microphone-ready cue by {delay_ms}ms for UI preview");
                            std::thread::sleep(Duration::from_millis(delay_ms));
                        }
                    }

                    if !rm_clone.is_recording_readiness_current(generation) {
                        debug!("Microphone became ready for an inactive recording");
                        return;
                    }

                    debug!("Microphone is receiving samples; recording is ready");
                    utils::emit_recording_ready(&app_clone);

                    // The start chime is a readiness cue, so it must follow the
                    // first real input callback rather than Stream::play() or a
                    // fixed delay. The helper returns immediately when feedback
                    // is disabled; mute still follows the same readiness point.
                    if rm_clone.is_recording_readiness_current(generation) {
                        play_feedback_sound_blocking(&app_clone, SoundType::Start);
                    }
                    if rm_clone.is_recording_readiness_current(generation) {
                        rm_clone.apply_mute();
                    }
                });
            }
            Err(e) => {
                debug!("Failed to start recording: {}", e);
                recording_error = Some(e);
            }
        }

        if recording_error.is_none() {
            // Dynamically register the cancel shortcut in a separate task to avoid deadlock
            shortcut::register_cancel_shortcut(app);
        } else {
            // Starting failed (for example due to blocked microphone permissions).
            // Revert UI state so we don't stay stuck in the recording overlay.
            tm.cancel_stream();
            utils::hide_recording_overlay(app);
            set_tray_state(app, TrayIconState::Idle);
            if let Some(err) = recording_error {
                let error_type = if is_microphone_access_denied(&err) {
                    "microphone_permission_denied"
                } else if is_no_input_device_error(&err) {
                    "no_input_device"
                } else {
                    "unknown"
                };
                let _ = app.emit(
                    "recording-error",
                    RecordingErrorEvent {
                        error_type: error_type.to_string(),
                        detail: Some(err),
                    },
                );
            }
        }

        debug!(
            "TranscribeAction::start completed in {:?}",
            start_time.elapsed()
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, _shortcut_str: &str) {
        // Prevent a slow microphone from emitting a ready event or start chime
        // after the user has already requested stop.
        app.state::<Arc<AudioRecordingManager>>()
            .invalidate_recording_readiness();

        // Unregister the cancel shortcut when transcription stops
        shortcut::unregister_cancel_shortcut(app);

        let stop_time = Instant::now();
        debug!("TranscribeAction::stop called for binding: {}", binding_id);

        let ah = app.clone();
        let rm = Arc::clone(&app.state::<Arc<AudioRecordingManager>>());
        let tm = Arc::clone(&app.state::<Arc<TranscriptionManager>>());
        let hm = Arc::clone(&app.state::<Arc<HistoryManager>>());

        set_tray_state(app, TrayIconState::Transcribing);
        // Stop should give immediate visual feedback. Live streaming can keep
        // the larger panel, but it still switches from listening to a working
        // spinner while the stream finalizes. Non-streaming paths use the
        // compact transcribing pill (None no-ops in show_*).
        let style = get_settings(app).overlay_style;
        // Capture this before finalizing the stream so every later working state
        // targets the same overlay that was shown for this transcription.
        let use_streaming_overlay = should_use_streaming_overlay(style, tm.is_streaming());
        if use_streaming_overlay {
            tm.emit_stream_working(StreamWorkKind::Transcribing);
        } else {
            show_transcribing_overlay(app);
        }

        // Unmute before playing audio feedback so the stop sound is audible
        rm.remove_mute();

        // Play audio feedback for recording stop
        play_feedback_sound(app, SoundType::Stop);

        let binding_id = binding_id.to_string(); // Clone binding_id for the async task
        let post_process = self.post_process;
        let cancel_generation = rm.cancel_generation();

        tauri::async_runtime::spawn(async move {
            let _guard = FinishGuard(ah.clone(), Arc::clone(&tm));
            debug!(
                "Starting async transcription task for binding: {}",
                binding_id
            );

            let stop_recording_time = Instant::now();
            if let Some(samples) = rm.stop_recording(&binding_id, cancel_generation) {
                debug!(
                    "Recording stopped and samples retrieved in {:?}, sample count: {}",
                    stop_recording_time.elapsed(),
                    samples.len()
                );

                if rm.was_cancelled_since(cancel_generation) {
                    debug!("Transcription operation cancelled after recording stop");
                    tm.cancel_stream();
                    utils::hide_recording_overlay(&ah);
                    set_tray_state(&ah, TrayIconState::Idle);
                    return;
                }

                if samples.is_empty() {
                    debug!("Recording produced no audio samples; skipping persistence");
                    // Tear down any streaming worker so its channel doesn't leak
                    // and block the next start_stream.
                    tm.cancel_stream();
                    utils::hide_recording_overlay(&ah);
                    set_tray_state(&ah, TrayIconState::Idle);
                } else {
                    // Save WAV concurrently with transcription
                    let sample_count = samples.len();
                    let file_name = format!("handy-{}.wav", chrono::Utc::now().timestamp());
                    let wav_path = hm.recordings_dir().join(&file_name);
                    let wav_path_for_verify = wav_path.clone();
                    let samples_for_wav = samples.clone();
                    let wav_handle = tauri::async_runtime::spawn_blocking(move || {
                        crate::audio_toolkit::save_wav_file(&wav_path, &samples_for_wav)
                    });

                    // Transcribe concurrently with WAV save. If a live stream was
                    // running, finalize it and use its text (all audio was already
                    // fed to the stream); otherwise batch-transcribe the samples.
                    let transcription_time = Instant::now();
                    let transcription_result = match tm.finalize_stream() {
                        // A finalized stream with usable text wins. An empty result
                        // (no active stream, produced nothing, or a finalize error
                        // after the engine was returned) falls back to a full batch
                        // transcription of the same audio. A finalize timeout is
                        // surfaced instead — the worker may still hold the engine,
                        // so a batch fallback would contend with it.
                        Ok(Some(text)) if !text.trim().is_empty() => Ok(text),
                        Ok(_) => tm.transcribe(samples),
                        Err(err) => Err(err),
                    };

                    // Await WAV save and verify
                    let wav_saved = match wav_handle.await {
                        Ok(Ok(())) => {
                            match crate::audio_toolkit::verify_wav_file(
                                &wav_path_for_verify,
                                sample_count,
                            ) {
                                Ok(()) => true,
                                Err(e) => {
                                    error!("WAV verification failed: {}", e);
                                    false
                                }
                            }
                        }
                        Ok(Err(e)) => {
                            error!("Failed to save WAV file: {}", e);
                            false
                        }
                        Err(e) => {
                            error!("WAV save task panicked: {}", e);
                            false
                        }
                    };

                    if rm.was_cancelled_since(cancel_generation) {
                        debug!("Transcription operation cancelled before output handling");
                        utils::hide_recording_overlay(&ah);
                        set_tray_state(&ah, TrayIconState::Idle);
                        return;
                    }

                    match transcription_result {
                        Ok(transcription) => {
                            debug!(
                                "Transcription completed in {:?}: '{}'",
                                transcription_time.elapsed(),
                                utils::redact_text(&transcription)
                            );

                            if post_process {
                                if use_streaming_overlay {
                                    tm.emit_stream_working(StreamWorkKind::Polishing);
                                } else {
                                    show_processing_overlay(&ah);
                                }
                            }
                            let Some(processed) = complete_unless_cancelled(
                                process_transcription_output(&ah, &transcription, post_process),
                                || rm.was_cancelled_since(cancel_generation),
                            )
                            .await
                            else {
                                debug!("Transcription operation cancelled during output handling");
                                utils::hide_recording_overlay(&ah);
                                set_tray_state(&ah, TrayIconState::Idle);
                                return;
                            };

                            if rm.was_cancelled_since(cancel_generation) {
                                debug!("Transcription operation cancelled before paste");
                                utils::hide_recording_overlay(&ah);
                                set_tray_state(&ah, TrayIconState::Idle);
                                return;
                            }

                            // Voice-command / memory hooks run on the raw
                            // transcript (never the post-processed text, so
                            // an LLM rewrite can't mangle a trigger phrase),
                            // before `transcription` is moved into
                            // `save_entry` below. Both are opt-in and off
                            // by default.
                            let hook_settings = get_settings(&ah);
                            let mut skip_paste_for_command = false;

                            // Voice hooks run in a fixed order and each one that
                            // matches CONSUMES the utterance (even if it fails):
                            // custom shortcut -> media control -> built-in router
                            // -> agent bridge (only on a genuine router NoMatch).
                            let mut hook_consumed = false;

                            let shortcut_hook = {
                                let router = ah.state::<Arc<CommandRouter>>();
                                let home = std::env::var_os("HOME").map(PathBuf::from);
                                let opener = AppShortcutOpener(&ah);
                                run_shortcut_hook(
                                    hook_settings.voice_commands_enabled,
                                    &hook_settings.voice_shortcuts,
                                    &transcription,
                                    &router,
                                    home.as_deref(),
                                    &opener,
                                )
                            };
                            if let Some(hook) = shortcut_hook {
                                match hook.event {
                                    HookEvent::Executed(description) => {
                                        info!("Voice shortcut executed: {description}");
                                        let _ = ah.emit("voice-command-executed", description);
                                    }
                                    HookEvent::Error(message) => {
                                        error!("Voice shortcut failed: {message}");
                                        let _ = ah.emit("voice-command-error", message);
                                    }
                                }
                                skip_paste_for_command = hook.outcome.skip_paste;
                                hook_consumed = true;
                            }

                            // Media controls. The cheap pure gate avoids spawning a
                            // thread for ordinary dictation; the osascript call
                            // itself (up to 5s) runs on a blocking thread so the
                            // async runtime stays free.
                            if !hook_consumed {
                                let media_result = if decide_media_command(
                                    hook_settings.voice_media_controls_enabled,
                                    hook_settings.voice_commands_enabled,
                                    &transcription,
                                )
                                .is_some()
                                {
                                    let media_text = transcription.clone();
                                    match tauri::async_runtime::spawn_blocking(move || {
                                        run_media_hook(true, true, &media_text, &OsascriptRunner)
                                    })
                                    .await
                                    {
                                        Ok(result) => result,
                                        Err(join_err) => {
                                            // The utterance matched a media command, so it
                                            // is still consumed even if the run crashed.
                                            error!("Media control task failed: {join_err}");
                                            Some(MediaHookResult {
                                                outcome: VoiceHookOutcome::HANDLED,
                                                event: HookEvent::Error(
                                                    "Media control failed unexpectedly.".to_string(),
                                                ),
                                            })
                                        }
                                    }
                                } else {
                                    None
                                };
                                if let Some(result) = media_result.as_ref() {
                                    match &result.event {
                                        HookEvent::Executed(description) => {
                                            info!("Media control executed: {description}");
                                            let _ = ah
                                                .emit("voice-command-executed", description.clone());
                                        }
                                        HookEvent::Error(message) => {
                                            error!("Failed to run media control: {message}");
                                            let _ = ah.emit("voice-command-error", message.clone());
                                        }
                                    }
                                }
                                if media_hook_consumes_utterance(media_result.as_ref()) {
                                    skip_paste_for_command = true;
                                    hook_consumed = true;
                                }
                            }

                            if !hook_consumed && hook_settings.voice_commands_enabled {
                                let router = ah.state::<Arc<CommandRouter>>();
                                match router.route(&transcription) {
                                    RouteDecision::Matched { action } => {
                                        match execute_command_action(&ah, &action) {
                                            Ok(description) => {
                                                info!("Voice command executed: {description}");
                                                let _ =
                                                    ah.emit("voice-command-executed", description);
                                                skip_paste_for_command = true;
                                            }
                                            Err(err) => {
                                                error!("Failed to execute voice command: {err}");
                                            }
                                        }
                                    }
                                    RouteDecision::NoMatch
                                        if should_escalate_to_agent_bridge(
                                            &RouteDecision::NoMatch,
                                            hook_settings.voice_commands_enabled,
                                            hook_settings.agent_bridge_enabled,
                                        ) =>
                                    {
                                        let app = ah.clone();
                                        let settings = hook_settings.clone();
                                        let log =
                                            ah.state::<Arc<ConversationLog>>().inner().clone();
                                        let memory =
                                            ah.state::<Arc<MemoryManager>>().inner().clone();
                                        let now_ms = monotonic_ms();
                                        let recall = move |query: &str| -> Vec<String> {
                                            memory
                                                .recall(query, 5)
                                                .map(|facts| {
                                                    facts
                                                        .into_iter()
                                                        .map(|fact| fact.text)
                                                        .collect()
                                                })
                                                .unwrap_or_default()
                                        };
                                        let (prompt, ticket) = prepare_agent_prompt(
                                            hook_settings.agent_context_enabled,
                                            hook_settings.memory_enabled,
                                            &transcription,
                                            &recall,
                                            &log,
                                            now_ms,
                                        );
                                        let prompt_transcript = transcription.clone();
                                        tauri::async_runtime::spawn(async move {
                                            let worker = crate::managers::agent_bridge::CliAgentWorker::from_settings(
                                                settings.agent_bridge_binary_path.clone(),
                                                settings.agent_bridge_prompt_flag.clone(),
                                                settings.agent_bridge_timeout_secs,
                                            );
                                            let result = match worker {
                                                Ok(worker) => crate::commands::agent_bridge::invoke_with_worker(
                                                    &settings,
                                                    &prompt,
                                                    &worker,
                                                ),
                                                Err(crate::managers::agent_bridge::AgentBridgeError::NotConfigured) => Err(
                                                    "No agent binary is configured. Set a binary path in Settings.".to_string(),
                                                ),
                                                Err(crate::managers::agent_bridge::AgentBridgeError::BinaryNotFound) => Err(
                                                    "The configured agent binary was not found. Check its path in Settings.".to_string(),
                                                ),
                                                Err(error) => Err(format!("Could not configure the agent: {error:?}")),
                                            };
                                            match result {
                                                Ok(reply) => {
                                                    let _ = app
                                                        .emit("agent-bridge-reply", reply.clone());
                                                    finish_agent_exchange(
                                                        ticket,
                                                        &log,
                                                        get_settings(&app).agent_context_enabled,
                                                        &prompt_transcript,
                                                        &reply,
                                                        monotonic_ms(),
                                                    );
                                                }
                                                Err(message) => {
                                                    let _ = app.emit("agent-bridge-error", message);
                                                }
                                            }
                                        });
                                    }
                                    RouteDecision::NoMatch => {}
                                }
                            }

                            if hook_settings.memory_enabled {
                                if let Some(fact) = decide_memory_fact(true, &transcription) {
                                    let memory_manager = ah.state::<Arc<MemoryManager>>();
                                    match memory_manager.remember(&fact, &transcription) {
                                        Ok(id) => {
                                            info!("Remembered fact (id {id}): {fact}");
                                            let _ = ah.emit("memory-remembered", fact.clone());
                                        }
                                        Err(err) => error!("Failed to remember fact: {err}"),
                                    }
                                }
                            }

                            // Save to history if WAV was saved
                            if wav_saved {
                                if let Err(err) = hm.save_entry(
                                    file_name,
                                    transcription,
                                    post_process,
                                    processed.post_processed_text.clone(),
                                    processed.post_process_prompt.clone(),
                                ) {
                                    error!("Failed to save history entry: {}", err);
                                }
                            }

                            if skip_paste_for_command || processed.final_text.is_empty() {
                                utils::hide_recording_overlay(&ah);
                                set_tray_state(&ah, TrayIconState::Idle);
                            } else {
                                let ah_clone = ah.clone();
                                let paste_time = Instant::now();
                                let final_text = processed.final_text;
                                let rm_for_paste = Arc::clone(&rm);
                                ah.run_on_main_thread(move || {
                                    if rm_for_paste.was_cancelled_since(cancel_generation) {
                                        debug!("Transcription operation cancelled before paste");
                                        utils::hide_recording_overlay(&ah_clone);
                                        set_tray_state(&ah_clone, TrayIconState::Idle);
                                        return;
                                    }

                                    match utils::paste(final_text, ah_clone.clone()) {
                                        Ok(()) => debug!(
                                            "Text pasted successfully in {:?}",
                                            paste_time.elapsed()
                                        ),
                                        Err(e) => {
                                            error!("Failed to paste transcription: {}", e);
                                            let _ = ah_clone.emit("paste-error", ());
                                        }
                                    }
                                    utils::hide_recording_overlay(&ah_clone);
                                    set_tray_state(&ah_clone, TrayIconState::Idle);
                                })
                                .unwrap_or_else(|e| {
                                    error!("Failed to run paste on main thread: {:?}", e);
                                    utils::hide_recording_overlay(&ah);
                                    set_tray_state(&ah, TrayIconState::Idle);
                                });
                            }
                        }
                        Err(err) => {
                            if rm.was_cancelled_since(cancel_generation) {
                                debug!(
                                    "Transcription operation cancelled after transcription error"
                                );
                                utils::hide_recording_overlay(&ah);
                                set_tray_state(&ah, TrayIconState::Idle);
                                return;
                            }

                            error!("Transcription failed: {}", err);
                            // Surface the failure to the UI (toast). The full
                            // message is also in handy.log via the line above.
                            let _ = ah.emit("transcription-error", err.to_string());
                            // Save entry with empty text so user can retry
                            if wav_saved {
                                if let Err(save_err) = hm.save_entry(
                                    file_name,
                                    String::new(),
                                    post_process,
                                    None,
                                    None,
                                ) {
                                    error!("Failed to save failed history entry: {}", save_err);
                                }
                            }
                            utils::hide_recording_overlay(&ah);
                            set_tray_state(&ah, TrayIconState::Idle);
                        }
                    }
                }
            } else {
                debug!("No samples retrieved from recording stop");
                // Tear down any streaming worker so its channel doesn't leak.
                tm.cancel_stream();
                utils::hide_recording_overlay(&ah);
                set_tray_state(&ah, TrayIconState::Idle);
            }
        });

        debug!(
            "TranscribeAction::stop completed in {:?}",
            stop_time.elapsed()
        );
    }
}

// Cancel Action
struct CancelAction;

impl ShortcutAction for CancelAction {
    fn start(&self, app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        utils::cancel_current_operation(app);
    }

    fn stop(&self, _app: &AppHandle, _binding_id: &str, _shortcut_str: &str) {
        // Nothing to do on stop for cancel
    }
}

// Test Action
struct TestAction;

impl ShortcutAction for TestAction {
    fn start(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Started - {} (App: {})", // Changed "Pressed" to "Started" for consistency
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }

    fn stop(&self, app: &AppHandle, binding_id: &str, shortcut_str: &str) {
        log::info!(
            "Shortcut ID '{}': Stopped - {} (App: {})", // Changed "Released" to "Stopped" for consistency
            binding_id,
            shortcut_str,
            app.package_info().name
        );
    }
}

// Static Action Map
pub static ACTION_MAP: Lazy<HashMap<String, Arc<dyn ShortcutAction>>> = Lazy::new(|| {
    let mut map = HashMap::new();
    map.insert(
        "transcribe".to_string(),
        Arc::new(TranscribeAction {
            post_process: false,
        }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "transcribe_with_post_process".to_string(),
        Arc::new(TranscribeAction { post_process: true }) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "cancel".to_string(),
        Arc::new(CancelAction) as Arc<dyn ShortcutAction>,
    );
    map.insert(
        "test".to_string(),
        Arc::new(TestAction) as Arc<dyn ShortcutAction>,
    );
    map
});

#[cfg(test)]
mod tests {
    use super::{
        complete_unless_cancelled, decide_memory_fact, decide_voice_command, finish_agent_exchange,
        is_blank_transcription, prepare_agent_prompt, should_escalate_to_agent_bridge,
        should_use_streaming_overlay, strip_think_block,
    };
    use crate::managers::command_router::{
        AppDiscovery, CommandAction, CommandRouter, HomeDirProvider, NullAppDiscovery,
        RouteDecision,
    };
    use crate::managers::conversation::ConversationLog;
    use crate::settings::OverlayStyle;
    use std::future;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn blank_transcription_is_detected() {
        assert!(is_blank_transcription(""));
        assert!(is_blank_transcription("   "));
        assert!(is_blank_transcription("\t\n  \r\n"));
    }

    #[test]
    fn non_blank_transcription_is_kept() {
        assert!(!is_blank_transcription("hello"));
        assert!(!is_blank_transcription("  hello  "));
    }

    #[test]
    fn completed_operation_returns_its_output() {
        let result = tauri::async_runtime::block_on(complete_unless_cancelled(
            future::ready("done"),
            || false,
        ));

        assert_eq!(result, Some("done"));
    }

    #[test]
    fn pending_operation_stops_after_cancellation() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancelled_for_thread = Arc::clone(&cancelled);
        let cancel_thread = thread::spawn(move || {
            thread::sleep(Duration::from_millis(10));
            cancelled_for_thread.store(true, Ordering::Release);
        });

        let result = tauri::async_runtime::block_on(complete_unless_cancelled(
            future::pending::<()>(),
            || cancelled.load(Ordering::Acquire),
        ));

        cancel_thread.join().unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn leading_think_block_is_stripped() {
        assert_eq!(
            strip_think_block("<think>pondering...</think>Cleaned text."),
            "Cleaned text."
        );
        assert_eq!(
            strip_think_block("  \n<think>multi\nline</think>\n  Cleaned text."),
            "Cleaned text."
        );
    }

    #[test]
    fn content_without_think_block_is_unchanged() {
        assert_eq!(strip_think_block("Cleaned text."), "Cleaned text.");
        assert_eq!(
            strip_think_block("Mentions <think> mid-sentence."),
            "Mentions <think> mid-sentence."
        );
        // Unclosed block: leave untouched rather than guess
        assert_eq!(
            strip_think_block("<think>never closed"),
            "<think>never closed"
        );
    }

    #[test]
    fn live_overlay_uses_streaming_states_only_for_streaming_models() {
        assert!(should_use_streaming_overlay(OverlayStyle::Live, true));
        assert!(!should_use_streaming_overlay(OverlayStyle::Live, false));
        assert!(!should_use_streaming_overlay(OverlayStyle::Minimal, true));
        assert!(!should_use_streaming_overlay(OverlayStyle::None, true));
    }

    struct FakeAppDiscovery(Vec<PathBuf>);
    impl AppDiscovery for FakeAppDiscovery {
        fn scan_installed_apps(&self) -> Vec<PathBuf> {
            self.0.clone()
        }
    }

    struct FakeHome(PathBuf);
    impl HomeDirProvider for FakeHome {
        fn home_dir(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    #[test]
    fn voice_command_disabled_never_routes_even_on_a_matching_phrase() {
        let router = CommandRouter::with_home(
            Box::new(FakeAppDiscovery(vec![PathBuf::from(
                "/Applications/iTerm.app",
            )])),
            Box::new(FakeHome(PathBuf::from("/tmp"))),
        );
        assert_eq!(decide_voice_command(&router, false, "open iterm"), None);
    }

    #[test]
    fn voice_command_enabled_but_no_match_returns_none() {
        let router = CommandRouter::new(Box::new(NullAppDiscovery));
        assert_eq!(
            decide_voice_command(&router, true, "what's the weather"),
            None
        );
    }

    #[test]
    fn voice_command_enabled_and_matching_returns_the_action() {
        let router = CommandRouter::with_home(
            Box::new(FakeAppDiscovery(vec![PathBuf::from(
                "/Applications/iTerm.app",
            )])),
            Box::new(FakeHome(PathBuf::from("/tmp"))),
        );
        let action = decide_voice_command(&router, true, "open iterm");
        assert_eq!(
            action,
            Some(CommandAction::OpenApp {
                name: "iTerm".to_string(),
                resolved_path: PathBuf::from("/Applications/iTerm.app"),
            })
        );
    }

    #[test]
    fn agent_bridge_escalates_when_enabled_and_route_is_no_match() {
        assert!(should_escalate_to_agent_bridge(
            &RouteDecision::NoMatch,
            true,
            true
        ));
    }

    #[test]
    fn agent_bridge_does_not_escalate_when_disabled_or_command_matched() {
        assert!(!should_escalate_to_agent_bridge(
            &RouteDecision::NoMatch,
            true,
            false
        ));
        assert!(!should_escalate_to_agent_bridge(
            &RouteDecision::NoMatch,
            false,
            true
        ));
        let matched = RouteDecision::Matched {
            action: CommandAction::OpenUrl {
                url: "https://example.com".to_string(),
            },
        };
        assert!(!should_escalate_to_agent_bridge(&matched, true, true));
    }

    #[test]
    fn memory_disabled_never_extracts_even_on_a_trigger_phrase() {
        assert_eq!(
            decide_memory_fact(false, "remember that I prefer tea"),
            None
        );
    }

    #[test]
    fn memory_enabled_but_no_trigger_phrase_returns_none() {
        assert_eq!(decide_memory_fact(true, "what's the weather"), None);
    }

    #[test]
    fn memory_enabled_and_triggered_extracts_the_fact() {
        assert_eq!(
            decide_memory_fact(true, "remember that I prefer tea"),
            Some("I prefer tea".to_string())
        );
    }

    #[test]
    fn media_hook_consumes_utterance_when_executed() {
        use crate::managers::voice_common::{HookEvent, VoiceHookOutcome};
        let result = crate::managers::media_control::MediaHookResult {
            outcome: VoiceHookOutcome::HANDLED,
            event: HookEvent::Executed("Paused music".to_string()),
        };
        assert!(super::media_hook_consumes_utterance(Some(&result)));
    }

    #[test]
    fn media_hook_consumes_utterance_when_execution_fails() {
        use crate::managers::voice_common::{HookEvent, VoiceHookOutcome};
        let result = crate::managers::media_control::MediaHookResult {
            outcome: VoiceHookOutcome::HANDLED,
            event: HookEvent::Error("Automation permission denied".to_string()),
        };
        assert!(super::media_hook_consumes_utterance(Some(&result)));
    }

    #[test]
    fn media_hook_does_not_consume_when_not_a_media_command() {
        assert!(!super::media_hook_consumes_utterance(None));
    }

    fn turn(user: &str, assistant: &str) -> (String, String) {
        (user.to_string(), assistant.to_string())
    }

    #[test]
    fn agent_context_disabled_returns_transcript_and_never_recalls() {
        let log = ConversationLog::new();
        let calls = std::cell::Cell::new(0usize);
        let recall = |_: &str| {
            calls.set(calls.get() + 1);
            vec!["fact".to_string()]
        };
        let (prompt, ticket) = prepare_agent_prompt(false, true, "hello", &recall, &log, 0);
        assert_eq!(prompt, "hello");
        assert!(ticket.is_none());
        assert_eq!(calls.get(), 0, "recall must not run when context is off");
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn agent_context_enabled_without_memory_sends_turns_but_no_facts() {
        let log = ConversationLog::new();
        let first = log.begin();
        assert!(log.record(first, "q1", "a1", 0));
        let calls = std::cell::Cell::new(0usize);
        let recall = |_: &str| {
            calls.set(calls.get() + 1);
            vec!["fact".to_string()]
        };
        let (prompt, ticket) = prepare_agent_prompt(true, false, "now", &recall, &log, 1);
        assert!(ticket.is_some());
        assert_eq!(calls.get(), 0, "recall must not run when memory is off");
        assert!(!prompt.contains("[Remembered facts]"));
        assert_eq!(
            prompt,
            "[Recent conversation]\nUser: q1\nAssistant: a1\n\n[Current request]\nnow"
        );
    }

    #[test]
    fn agent_context_enabled_with_memory_renders_recalled_facts() {
        let log = ConversationLog::new();
        let recall = |query: &str| {
            assert_eq!(query, "what do I drink");
            vec!["I prefer tea".to_string()]
        };
        let (prompt, ticket) =
            prepare_agent_prompt(true, true, "what do I drink", &recall, &log, 0);
        assert!(ticket.is_some());
        assert_eq!(
            prompt,
            "[Remembered facts]\n- I prefer tea\n\n[Current request]\nwhat do I drink"
        );
    }

    #[test]
    fn agent_context_empty_recall_adds_no_facts_section() {
        let log = ConversationLog::new();
        let recall = |_: &str| Vec::<String>::new();
        let (prompt, _) = prepare_agent_prompt(true, true, "go", &recall, &log, 0);
        assert_eq!(prompt, "go");
    }

    #[test]
    fn agent_context_finish_keeps_chronological_order_by_dispatch() {
        let log = ConversationLog::new();
        let recall = |_: &str| Vec::<String>::new();
        let (_, first) = prepare_agent_prompt(true, false, "q1", &recall, &log, 0);
        let (_, second) = prepare_agent_prompt(true, false, "q2", &recall, &log, 0);
        assert!(finish_agent_exchange(second, &log, true, "q2", "a2", 10));
        assert!(finish_agent_exchange(first, &log, true, "q1", "a1", 20));
        assert_eq!(log.recent(30), vec![turn("q1", "a1"), turn("q2", "a2")]);
    }

    #[test]
    fn agent_context_finish_after_clear_records_nothing() {
        let log = ConversationLog::new();
        let recall = |_: &str| Vec::<String>::new();
        let (_, ticket) = prepare_agent_prompt(true, false, "q", &recall, &log, 0);
        log.clear();
        assert!(!finish_agent_exchange(ticket, &log, true, "q", "a", 5));
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn agent_context_finish_when_disabled_now_records_nothing() {
        let log = ConversationLog::new();
        let recall = |_: &str| Vec::<String>::new();
        let (_, ticket) = prepare_agent_prompt(true, false, "q", &recall, &log, 0);
        assert!(!finish_agent_exchange(ticket, &log, false, "q", "a", 5));
        assert_eq!(log.len(), 0);
    }

    #[test]
    fn agent_context_finish_without_ticket_records_nothing() {
        let log = ConversationLog::new();
        assert!(!finish_agent_exchange(None, &log, true, "q", "a", 5));
        assert_eq!(log.len(), 0);
    }

    mod utterance_hook_order {
        use super::super::{run_utterance_hooks, HookSource, UtteranceHookResult};
        use crate::managers::command_router::{AppDiscovery, CommandRouter, HomeDirProvider};
        use crate::managers::media_control::{MediaError, ScriptRunner};
        use crate::managers::voice_common::HookEvent;
        use crate::managers::voice_shortcuts::{ShortcutOpener, VoiceShortcut, VoiceShortcutAction};
        use crate::settings::{get_default_settings, AppSettings};
        use std::path::{Path, PathBuf};
        use std::sync::Mutex;

        struct NoApps;
        impl AppDiscovery for NoApps {
            fn scan_installed_apps(&self) -> Vec<PathBuf> {
                Vec::new()
            }
        }
        struct NoHome;
        impl HomeDirProvider for NoHome {
            fn home_dir(&self) -> Option<PathBuf> {
                None
            }
        }
        fn router() -> CommandRouter {
            CommandRouter::with_home(Box::new(NoApps), Box::new(NoHome))
        }

        #[derive(Default)]
        struct Opener {
            calls: Mutex<Vec<String>>,
            fail: bool,
        }
        impl Opener {
            fn record(&self, entry: String) -> Result<(), String> {
                self.calls.lock().unwrap().push(entry);
                if self.fail {
                    Err("opener failed".into())
                } else {
                    Ok(())
                }
            }
            fn count(&self) -> usize {
                self.calls.lock().unwrap().len()
            }
        }
        impl ShortcutOpener for Opener {
            fn open_url(&self, url: &str) -> Result<(), String> {
                self.record(format!("url:{url}"))
            }
            fn open_path(&self, path: &Path) -> Result<(), String> {
                self.record(format!("path:{}", path.display()))
            }
            fn open_app(&self, app_path: &Path) -> Result<(), String> {
                self.record(format!("app:{}", app_path.display()))
            }
        }

        struct Runner {
            scripts: Mutex<Vec<String>>,
            result: Result<String, MediaError>,
        }
        impl Runner {
            fn ok() -> Self {
                Self {
                    scripts: Mutex::new(Vec::new()),
                    result: Ok("ok".into()),
                }
            }
            fn failing() -> Self {
                Self {
                    scripts: Mutex::new(Vec::new()),
                    result: Err(MediaError::PermissionDenied),
                }
            }
            fn count(&self) -> usize {
                self.scripts.lock().unwrap().len()
            }
        }
        impl ScriptRunner for Runner {
            fn run(&self, script: &str) -> Result<String, MediaError> {
                self.scripts.lock().unwrap().push(script.to_string());
                self.result.clone()
            }
        }

        fn url_shortcut(phrase: &str, url: &str) -> VoiceShortcut {
            VoiceShortcut {
                phrase: phrase.to_string(),
                action: VoiceShortcutAction::OpenUrl {
                    url: url.to_string(),
                },
            }
        }

        fn settings(voice: bool, media: bool, shortcuts: Vec<VoiceShortcut>) -> AppSettings {
            let mut s = get_default_settings();
            s.voice_commands_enabled = voice;
            s.voice_media_controls_enabled = media;
            s.voice_shortcuts = shortcuts;
            s
        }

        fn run(
            settings: &AppSettings,
            text: &str,
            opener: &Opener,
            runner: &Runner,
        ) -> Option<UtteranceHookResult> {
            run_utterance_hooks(settings, text, &router(), None, opener, runner)
        }

        #[test]
        fn custom_shortcut_wins_over_media_for_the_same_phrase() {
            let s = settings(true, true, vec![url_shortcut("pause", "https://example.com/p")]);
            let (opener, runner) = (Opener::default(), Runner::ok());
            let result = run(&s, "pause", &opener, &runner).expect("consumed");
            assert_eq!(result.source, HookSource::Shortcut);
            assert_eq!(opener.count(), 1);
            assert_eq!(runner.count(), 0, "media must not run once a shortcut matched");
        }

        #[test]
        fn media_runs_when_no_shortcut_matches() {
            let s = settings(true, true, vec![url_shortcut("open docs", "https://example.com/d")]);
            let (opener, runner) = (Opener::default(), Runner::ok());
            let result = run(&s, "pause", &opener, &runner).expect("consumed");
            assert_eq!(result.source, HookSource::Media);
            assert!(result.outcome.skip_paste);
            assert!(!result.outcome.escalate_to_agent);
            assert_eq!(opener.count(), 0);
            assert!(runner.count() >= 1);
        }

        #[test]
        fn ordinary_dictation_matches_nothing_and_touches_nothing() {
            let s = settings(true, true, vec![url_shortcut("open docs", "https://example.com/d")]);
            let (opener, runner) = (Opener::default(), Runner::ok());
            assert_eq!(run(&s, "please write me a short email to the team", &opener, &runner), None);
            assert_eq!(run(&s, "", &opener, &runner), None);
            assert_eq!(opener.count(), 0);
            assert_eq!(runner.count(), 0);
        }

        #[test]
        fn voice_commands_off_disables_both_hooks() {
            let s = settings(false, true, vec![url_shortcut("pause", "https://example.com/p")]);
            let (opener, runner) = (Opener::default(), Runner::ok());
            assert_eq!(run(&s, "pause", &opener, &runner), None);
            assert_eq!(run(&s, "volume 40", &opener, &runner), None);
            assert_eq!(opener.count(), 0);
            assert_eq!(runner.count(), 0);
        }

        #[test]
        fn media_off_leaves_media_phrases_alone_but_shortcuts_still_work() {
            let s = settings(true, false, vec![url_shortcut("open docs", "https://example.com/d")]);
            let (opener, runner) = (Opener::default(), Runner::ok());
            assert_eq!(run(&s, "pause", &opener, &runner), None, "falls through to dictation/router");
            let result = run(&s, "open docs", &opener, &runner).expect("shortcut still works");
            assert_eq!(result.source, HookSource::Shortcut);
            assert_eq!(runner.count(), 0);
        }

        #[test]
        fn failed_shortcut_still_consumes_and_never_falls_through_to_media() {
            let s = settings(true, true, vec![url_shortcut("pause", "https://example.com/p")]);
            let (opener, runner) = (Opener { fail: true, ..Opener::default() }, Runner::ok());
            let result = run(&s, "pause", &opener, &runner).expect("consumed even though it failed");
            assert_eq!(result.source, HookSource::Shortcut);
            assert!(matches!(result.event, HookEvent::Error(_)));
            assert!(result.outcome.skip_paste);
            assert!(!result.outcome.escalate_to_agent);
            assert_eq!(runner.count(), 0);
        }

        #[test]
        fn failed_media_command_still_consumes_the_utterance() {
            let s = settings(true, true, Vec::new());
            let (opener, runner) = (Opener::default(), Runner::failing());
            let result = run(&s, "next track", &opener, &runner).expect("consumed even though it failed");
            assert_eq!(result.source, HookSource::Media);
            assert!(matches!(result.event, HookEvent::Error(_)));
            assert!(result.outcome.skip_paste);
            assert!(!result.outcome.escalate_to_agent);
        }

        #[test]
        fn error_events_never_contain_the_transcript() {
            let s = settings(true, true, Vec::new());
            let (opener, runner) = (Opener::default(), Runner::failing());
            let result = run(&s, "volume 40", &opener, &runner).expect("consumed");
            match result.event {
                HookEvent::Error(message) => assert!(!message.contains("volume 40"), "{message}"),
                other => panic!("expected an error event, got {other:?}"),
            }
        }
    }
}
