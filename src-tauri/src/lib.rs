mod actions;
mod activation_policy;
mod appearance;
#[cfg(test)]
mod appearance_tests;
#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod apple_intelligence;
mod audio_feedback;
pub mod audio_toolkit;
mod autostart;
#[cfg(test)]
mod bubble_admission_e2e_tests;
#[cfg(test)]
mod bubble_anchor_tests;
#[cfg(test)]
mod bubble_g4b_fix_tests;
#[cfg(test)]
mod bubble_geometry_tests;
mod bubble_hub;
#[cfg(test)]
mod bubble_hub_hold_tests;
#[cfg(test)]
mod bubble_hub_tests;
mod bubble_native;
mod bubble_notice;
#[cfg(test)]
mod bubble_notice_tests;
mod bubble_shell_core;
#[cfg(test)]
mod bubble_shell_core_tests;
#[cfg(test)]
mod bubble_wiring_contracts_tests;
mod catalog;
#[cfg(target_os = "macos")]
mod chat_chrome;
mod chat_opacity;
mod chat_presentation;
#[cfg(test)]
mod chat_presentation_tests;
#[cfg(test)]
mod chat_stop_gate_tests;
mod chat_surface;
#[cfg(test)]
mod chat_surface_tests;
mod chat_window;
mod chinese_script;
pub mod cli;
mod clipboard;
mod commands;
mod helpers;
mod input;
mod job_runtime;
mod llm_client;
#[cfg(test)]
mod orb_accent_tests;
mod orb_converse;
#[cfg(test)]
mod orb_converse_tests;
#[cfg(test)]
mod orb_dismiss_tests;
#[cfg(test)]
mod orb_engine_tests;
mod orb_gaze;
mod orb_geometry;
#[cfg(test)]
mod orb_geometry_followup_tests;
#[cfg(test)]
mod orb_geometry_tests;
mod orb_gesture;
mod orb_native;
#[cfg(test)]
mod orb_native_contracts_tests;
mod orb_runtime;
#[cfg(test)]
mod orb_runtime_tests;
mod orb_session;
#[cfg(test)]
mod orb_session_tests;
#[cfg(test)]
mod orb_settings_tests;
#[cfg(test)]
mod orb_size_tests;
#[cfg(test)]
mod orb_support_tests;
mod orb_window;
#[cfg(test)]
mod settings_appearance_tests;
mod window_commands;
#[cfg(test)]
mod window_commands_bubbles_tests;
#[cfg(test)]
mod window_policy_tests;
use crate::managers::agent_jobs::ShutdownConfig;
mod managers;
mod memory;
mod overlay;
mod paste_tx;
pub mod portable;
mod secure_input;
mod settings;
mod shortcut;
mod signal_handle;
mod transcription_coordinator;
mod tray;
mod tray_i18n;
mod utils;

pub use cli::CliArgs;
#[cfg(debug_assertions)]
use specta_typescript::{BigIntExportBehavior, Typescript};
use tauri_specta::{collect_commands, collect_events, Builder};
pub use utils::env_flag_enabled;

use env_filter::Builder as EnvFilterBuilder;
use managers::ambient::{AmbientCoordinator, EchoSuppressor, EngagementJudge, RollingTranscript};
use managers::ambient_listener::AmbientListener;
use managers::audio::AudioRecordingManager;
use managers::command_router::CommandRouter;
use managers::history::HistoryManager;
use managers::memory::MemoryManager;
use managers::model::ModelManager;
use managers::source_update::SourceUpdateRuntime;
use managers::transcription::TranscriptionManager;
use managers::wakeword_listener::WakeWordListener;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
pub use transcription_coordinator::TranscriptionCoordinator;

use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Listener, Manager};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_log::{Builder as LogBuilder, RotationStrategy, Target, TargetKind};

use crate::settings::get_settings;

// Global atomic to store the file log level filter
// We use u8 to store the log::LevelFilter as a number
pub static FILE_LOG_LEVEL: AtomicU8 = AtomicU8::new(log::LevelFilter::Debug as u8);

/// When `true`, log records are also forwarded to the webview via the
/// `log://log` event for the debug panel's live log viewer. Gated on debug
/// mode — the live log viewer is its only consumer and only exists in debug
/// mode — so normal runs never broadcast log records (which can include file
/// paths or transcribed text) onto the frontend event bus. Synced at startup
/// and whenever debug mode is toggled (see `shortcut::change_debug_mode_setting`).
pub static WEBVIEW_LOG_STREAMING: AtomicBool = AtomicBool::new(false);

fn level_filter_from_u8(value: u8) -> log::LevelFilter {
    match value {
        0 => log::LevelFilter::Off,
        1 => log::LevelFilter::Error,
        2 => log::LevelFilter::Warn,
        3 => log::LevelFilter::Info,
        4 => log::LevelFilter::Debug,
        5 => log::LevelFilter::Trace,
        _ => log::LevelFilter::Trace,
    }
}

/// Turns off every log target in `natural_voices::SUPPRESSED_LOG_TARGETS` (third-party crates
/// that would log spoken text) for all sinks: stdout/stderr, file and webview.
fn with_suppressed_log_targets(mut builder: LogBuilder) -> LogBuilder {
    for target in managers::natural_voices::SUPPRESSED_LOG_TARGETS {
        builder = builder.level_for(*target, log::LevelFilter::Off);
    }
    builder
}

fn build_console_filter() -> env_filter::Filter {
    let mut builder = EnvFilterBuilder::new();

    match std::env::var("RUST_LOG") {
        Ok(spec) if !spec.trim().is_empty() => {
            if let Err(err) = builder.try_parse(&spec) {
                log::warn!(
                    "Ignoring invalid RUST_LOG value '{}': {}. Falling back to info-level console logging",
                    spec,
                    err
                );
                builder.filter_level(log::LevelFilter::Info);
            }
        }
        _ => {
            builder.filter_level(log::LevelFilter::Info);
        }
    }

    builder.build()
}

fn show_main_window(app: &AppHandle) {
    if let Some(main_window) = app.get_webview_window("main") {
        if let Err(e) = main_window.unminimize() {
            log::error!("Failed to unminimize webview window: {}", e);
        }
        if let Err(e) = main_window.show() {
            log::error!("Failed to show webview window: {}", e);
        }
        if let Err(e) = main_window.set_focus() {
            log::error!("Failed to focus webview window: {}", e);
        }
        #[cfg(target_os = "macos")]
        {
            if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Regular) {
                log::error!("Failed to set activation policy to Regular: {}", e);
            }
        }
        return;
    }

    let webview_labels = app.webview_windows().keys().cloned().collect::<Vec<_>>();
    log::error!(
        "Main window not found. Webview labels: {:?}",
        webview_labels
    );
}

/// Choose the macOS activation policy the process *launches* with.
///
/// Must run between `build()` and `run()`: that is the only point where
/// `App::set_activation_policy` sets tao's initial policy, which
/// `applicationDidFinishLaunching` then applies directly. Calling the
/// `AppHandle` variant from `setup` (which Tauri runs on `RunEvent::Ready`,
/// i.e. after launch) is instead a runtime Regular → Accessory demotion of an
/// already-activated foreground app — the transition Apple documents as
/// unreliable, and what left a Dock icon behind for start-hidden and
/// login-item launches on macOS 26+ (#1787). Launching as Accessory avoids the
/// transition entirely; showing the window later promotes to Regular, which is
/// the supported direction.
///
/// Mirrors the show-window decision in `setup`: the app launches without a
/// Dock icon only when it will start hidden (setting or `--start-hidden`) AND a
/// tray icon is available (setting and not `--no-tray`). With no tray the Dock
/// icon stays as the only way back into the app (#903). Headless one-shot
/// runs are left alone.
#[cfg(target_os = "macos")]
fn apply_startup_activation_policy(app: &mut tauri::App, headless_mode: bool) {
    if headless_mode {
        return;
    }

    let cli_args = app.state::<CliArgs>().inner().clone();
    let settings = settings::get_settings(app.handle());

    let should_hide = settings.start_hidden || cli_args.start_hidden;
    let tray_available = settings.show_tray_icon && !cli_args.no_tray;

    if should_hide && tray_available {
        log::info!("Starting hidden with tray available: launching as Accessory (no Dock icon)");
        app.set_activation_policy(tauri::ActivationPolicy::Accessory);
    }
}

#[allow(unused_variables)]
fn should_force_show_permissions_window(app: &AppHandle) -> bool {
    #[cfg(target_os = "windows")]
    {
        let model_manager = app.state::<Arc<ModelManager>>();
        let has_downloaded_models = model_manager
            .get_available_models()
            .iter()
            .any(|model| model.is_downloaded);

        if !has_downloaded_models {
            return false;
        }

        let status = commands::audio::get_windows_microphone_permission_status();
        if status.supported && status.overall_access == commands::audio::PermissionAccess::Denied {
            log::info!(
                "Windows microphone permissions are denied; forcing main window visible for onboarding"
            );
            return true;
        }
    }

    false
}

/// Real `.app` scanning only exists on macOS; other platforms get a
/// discovery that always reports no apps, so voice-command app-open
/// matching deterministically returns `NoMatch` there (folder matching
/// still works everywhere via `CommandRouter`'s own home-dir lookup).
fn new_app_discovery() -> Box<dyn managers::command_router::AppDiscovery> {
    #[cfg(target_os = "macos")]
    {
        Box::new(managers::command_router::MacAppDiscovery)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Box::new(managers::command_router::NullAppDiscovery)
    }
}

/// The app events that may be spoken aloud. Command-outcome events are
/// included so their fixed phrases are spoken; their payloads never are.
const TTS_EVENTS: [&str; 4] = [
    "agent-bridge-exchange",
    "agent-bridge-error",
    "voice-command-executed",
    "voice-command-error",
];

/// Decides whether an app event should be spoken and, if so, what. Pure so
/// the listener closure stays trivial. `None` when spoken replies are
/// disabled or the event has no speakable phrase.
fn tts_phrase_to_speak(enabled: bool, event_name: &str, payload: &str) -> Option<String> {
    if !enabled {
        return None;
    }
    managers::tts::phrase_for_event(event_name, payload)
}

/// Decodes an event payload. Agent replies are emitted as Rust strings, so the
/// payload is a JSON string literal; anything that is not valid JSON string
/// falls back to the raw payload.
fn decode_tts_payload(raw: &str) -> String {
    serde_json::from_str::<String>(raw).unwrap_or_else(|_| raw.to_string())
}

/// A fixed, content-free name for a speech error, safe to log.
fn tts_error_kind(error: &managers::tts::TtsError) -> &'static str {
    use managers::tts::TtsError;
    match error {
        TtsError::Empty => "empty",
        TtsError::Recording => "recording",
        TtsError::CancelFailed => "cancel-failed",
        TtsError::Backend(_) => "backend",
        TtsError::Unsupported => "unsupported",
        TtsError::Superseded => "superseded",
    }
}

/// Registers backend listeners that speak the four speakable app events.
/// Speech is decoupled from the features that emit these events, so any
/// feature that emits one is spoken without an inline call.
fn register_tts_listeners(app: &AppHandle, tts: Arc<managers::tts::TtsManager>) {
    for event_name in TTS_EVENTS {
        if event_name == "agent-bridge-exchange" {
            let app_for_exchange = app.clone();
            let tts_for_exchange = Arc::clone(&tts);
            app.listen(event_name, move |event| {
                let Some(exchange) = managers::speech_pipeline::parse_exchange(event.payload())
                else {
                    return;
                };
                let app_for_run = app_for_exchange.clone();
                let tts_for_run = Arc::clone(&tts_for_exchange);
                tauri::async_runtime::spawn(async move {
                    let load_app = app_for_run.clone();
                    let load = move || get_settings(&load_app);
                    let status_app = app_for_run.clone();
                    let status = move |outcome: &'static str| {
                        let _ = status_app.emit(
                            "speech-composer-status",
                            serde_json::json!({ "outcome": outcome }),
                        );
                    };
                    managers::speech_pipeline::run_exchange(
                        &tts_for_run,
                        &load,
                        &managers::speech_pipeline::RealComposer,
                        &status,
                        exchange,
                    )
                    .await;
                });
            });
            continue;
        }
        let app_for_event = app.clone();
        let tts_for_event = Arc::clone(&tts);
        app.listen(event_name, move |event| {
            let settings = get_settings(&app_for_event);
            let payload = decode_tts_payload(event.payload());
            let Some(phrase) = tts_phrase_to_speak(settings.tts_enabled, event_name, &payload)
            else {
                return;
            };
            let voice = settings.tts_voice.clone();
            let rate = settings.tts_rate_wpm;
            let tts_for_speech = Arc::clone(&tts_for_event);
            // speak() may block briefly while a previous utterance is killed,
            // so keep the event thread free.
            std::thread::spawn(move || {
                if let Err(error) = tts_for_speech.speak(&phrase, voice.as_deref(), Some(rate)) {
                    // Log the error KIND only: never the spoken text or payload.
                    match error {
                        managers::tts::TtsError::Empty | managers::tts::TtsError::Recording => {}
                        other => log::warn!("Spoken reply skipped: {}", tts_error_kind(&other)),
                    }
                }
            });
        });
    }
}

/// Starts the persistent ACP agent on a background thread when it is enabled and has a
/// command. Failures are logged without agent detail and never block startup.
fn spawn_acp_prewarm(app_handle: &AppHandle) {
    let settings = get_settings(app_handle);
    if !settings.acp_agent_enabled {
        return;
    }
    let has_command = settings
        .acp_agent_command
        .as_deref()
        .is_some_and(|command| !command.trim().is_empty());
    if !has_command {
        return;
    }
    let Some(manager) = app_handle
        .try_state::<Arc<managers::acp_agent::AcpAgentManager>>()
        .map(|state| state.inner().clone())
    else {
        return;
    };
    let app = app_handle.clone();
    std::thread::spawn(move || {
        let outcome = actions::acp_agent_config(&app, &settings)
            .map_err(|_| ())
            .and_then(|cfg| manager.warm_up(&cfg).map_err(|_| ()));
        if outcome.is_err() {
            log::warn!("ACP agent prewarm failed");
        }
    });
}

fn initialize_core_logic(app_handle: &AppHandle) {
    // Note: Enigo (keyboard/mouse simulation) is NOT initialized here.
    // The frontend is responsible for calling the `initialize_enigo` command
    // after onboarding completes. This avoids triggering permission dialogs
    // on macOS before the user is ready.

    // Initialize the managers. The audio recorder receives the streaming router
    // explicitly, so always-on microphone startup can wire live-preview frames
    // even before Tauri state is populated.
    let model_manager =
        Arc::new(ModelManager::new(app_handle).expect("Failed to initialize model manager"));
    let transcription_manager = Arc::new(
        TranscriptionManager::new(app_handle, model_manager.clone())
            .expect("Failed to initialize transcription manager"),
    );
    let recording_manager = Arc::new(
        AudioRecordingManager::new(app_handle, transcription_manager.stream_router())
            .expect("Failed to initialize recording manager"),
    );
    let history_manager =
        Arc::new(HistoryManager::new(app_handle).expect("Failed to initialize history manager"));
    let memory_manager =
        Arc::new(MemoryManager::new(app_handle).expect("Failed to initialize memory manager"));
    // Conversation records are optional: a store that cannot open is logged (kind only) and
    // skipped, so startup and the conversation itself are unaffected.
    let conversation_store = match managers::conversation_store::ConversationStore::new(app_handle)
    {
        Ok(store) => Some(Arc::new(store)),
        Err(_) => {
            log::warn!("Conversation records unavailable: store did not open");
            None
        }
    };
    // Conversation memory is optional too: a store that cannot open is logged with a fixed
    // message and skipped; the commands tolerate its absence.
    let conversation_memory = crate::portable::app_data_dir(app_handle)
        .map_err(|_| ())
        .and_then(|dir| {
            managers::conversation_memory::ConversationMemory::open(
                &dir.join("conversation_memory.db"),
            )
            .map_err(|_| ())
        });
    let conversation_memory = match conversation_memory {
        Ok(store) => Some(Arc::new(store)),
        Err(()) => {
            log::warn!("Conversation memory unavailable: store did not open");
            None
        }
    };
    let conversation_log = Arc::new(managers::conversation::ConversationLog::new());
    let command_router = Arc::new(CommandRouter::new(new_app_discovery()));
    let wakeword_listener = Arc::new(WakeWordListener::new(app_handle.clone()));
    let ambient_settings = settings::get_settings(app_handle);
    let ambient_transcript = Arc::new(RollingTranscript::new(
        settings::effective_ambient_window_ms(&ambient_settings),
    ));
    let ambient_coordinator = Arc::new(AmbientCoordinator::with_shared_transcript(
        Arc::clone(&ambient_transcript),
        EngagementJudge::new(settings::effective_ambient_wake_names(&ambient_settings)),
        EchoSuppressor::new(),
    ));
    let ambient_listener = Arc::new(AmbientListener::new(app_handle.clone()));

    // Initialize the transcribe-cpp native backend (logging + backend module
    // registration) once, before any whisper model is loaded.
    managers::transcription::init_transcribe_backend();

    // Apply accelerator preferences before any model loads
    managers::transcription::apply_accelerator_settings(app_handle);

    // Add managers to Tauri's managed state
    app_handle.manage(recording_manager.clone());
    app_handle.manage(model_manager.clone());
    app_handle.manage(transcription_manager.clone());
    app_handle.manage(history_manager.clone());
    app_handle.manage(memory_manager.clone());
    if let Some(store) = conversation_store {
        app_handle.manage(store);
    }
    if let Some(memory) = conversation_memory {
        app_handle.manage(memory);
    }
    app_handle.manage(conversation_log);
    // Persistent ACP agent client used by dictation escalation (DESIGN rev 3.1 section 3).
    // Permission requests are decided by the current settings and approved in the UI.
    let acp_settings_app = app_handle.clone();
    let acp_timeout_app = app_handle.clone();
    let acp_prompter = managers::acp_permission_ui::TauriPermissionPrompter::new(
        app_handle.clone(),
        Arc::new(move || {
            settings::clamp_permission_timeout_secs(
                get_settings(&acp_timeout_app).acp_permission_timeout_secs,
            )
        }),
    );
    app_handle.manage(Arc::new(
        managers::acp_agent::AcpAgentManager::new_with_gate(
            managers::agent_settings_gate::process_gate(),
        )
        .with_permissions(
            Arc::new(acp_prompter),
            Arc::new(move || get_settings(&acp_settings_app).acp_permission_settings()),
        ),
    ));
    app_handle.manage(command_router.clone());
    app_handle.manage(wakeword_listener.clone());
    app_handle.manage(ambient_transcript);
    app_handle.manage(ambient_coordinator);
    app_handle.manage(ambient_listener.clone());
    app_handle.manage(SourceUpdateRuntime::new());
    // Natural voice: model files coordinator and health, managed on every platform so the
    // settings commands answer everywhere. Nothing is created on disk and the model is NOT
    // loaded here: the first natural spoken reply loads it lazily.
    let natural_coordinator = Arc::new(managers::natural_model::ModelCoordinator::new(
        portable::app_data_dir(app_handle)
            .expect("Failed to resolve app data dir")
            .join("tts_models"),
        managers::natural_model::kokoro_assets(),
        vec![managers::natural_model::KOKORO_URL_PREFIX.to_string()],
    ));
    let natural_health = managers::natural_engine::NaturalHealth::new(
        managers::natural_engine::DEFAULT_AUTO_RETRY_COOLDOWN,
    );
    {
        let event_handle = app_handle.clone();
        natural_health.set_event_sink(Box::new(move |reason| {
            let _ = event_handle.emit(
                "natural-voice-error",
                commands::natural_voice::failure_name(reason).to_string(),
            );
        }));
    }
    app_handle.manage(natural_coordinator.clone());
    app_handle.manage(natural_health.clone());
    // Optional model downloads (design D2): both kinds in one state, rooted at
    // <app data>/aux_models. Nothing is downloaded here; only the user's Download starts one.
    if let Ok(app_data) = portable::app_data_dir(app_handle) {
        let aux_models = Arc::new(commands::aux_model::AuxModels::new(
            app_data.join("aux_models"),
        ));
        app_handle.manage(Arc::clone(&aux_models));
        // Voice features (S12): one runtime over the shared ownership gate and recording manager.
        // Models already downloaded are installed here; nothing is downloaded.
        let settings_handle = app_handle.clone();
        let voice_runtime = Arc::new(managers::voice_runtime::VoiceRuntime::production(
            &app_data,
            managers::ownership::global().clone(),
            recording_manager.clone(),
            Arc::new(move || get_settings(&settings_handle)),
        ));
        commands::aux_model::startup_sync(&voice_runtime, &aux_models);
        managers::voice_runtime::spawn_expiry_ticker(&voice_runtime);
        app_handle.manage(voice_runtime);
    } else {
        log::warn!("optional models: app data dir unavailable");
    }
    // Spoken replies: one shared manager, managed before any event listener
    // that might speak. Event listeners are registered in `register_tts_listeners`.
    #[cfg(target_os = "macos")]
    let tts_manager = {
        use managers::natural_router::{EngineBackend, RouterChoice, RouterConfig};
        use managers::tts_queue::{QueueBackend, SharedCore, SpeakerCore};
        use std::sync::Mutex;
        use std::time::Duration;

        let settings_handle = app_handle.clone();
        let pick_fast: Arc<dyn Fn() -> bool + Send + Sync> = Arc::new(move || {
            managers::realtime_config::RealtimeConfig::from_settings(&get_settings(
                &settings_handle,
            ))
            .fast_voice
        });
        let system = managers::tts_av::default_backend_with(pick_fast);
        let system_queue: Box<dyn managers::tts::SpeechBackend> = Box::new(
            managers::tts_serial::SerializedQueue::new(Box::new(managers::tts::SayBackend)),
        );

        let loader_coordinator = natural_coordinator.clone();
        let loader = managers::natural_kokoro::KokoroLoader::new(Box::new(move || {
            loader_coordinator
                .load_verified_bytes()
                .map_err(|e| e.to_string())
        }));
        let device_handle = app_handle.clone();
        let sink = Arc::new(managers::natural_sink::ManagedSink::new(
            Arc::new(
                managers::natural_sink::CpalStreamFactory::with_device_source(Arc::new(
                    move || get_settings(&device_handle).selected_output_device,
                )),
            ),
            Duration::from_secs(3),
        ));
        let synth = managers::natural_engine::NaturalSynth::new(
            Box::new(loader),
            sink,
            natural_health.clone(),
            managers::natural_engine::NaturalConfig::default(),
        );
        let core: SharedCore<_> = Arc::new(Mutex::new(SpeakerCore::new(synth)));
        let natural: Box<dyn managers::tts::SpeechBackend> = Box::new(QueueBackend::new(core));

        // Phonon voice: a peer of Kokoro with its own coordinator, health and sink. Its model
        // root and its scratch folder are siblings under the app data dir (scratch is never
        // inside the model root). Its stream factory reads the same output device as Kokoro's.
        let phonon_app_data =
            portable::app_data_dir(app_handle).expect("Failed to resolve app data dir");
        let phonon_device_handle = app_handle.clone();
        let phonon_factory = Arc::new(
            managers::natural_sink::CpalStreamFactory::with_device_source(Arc::new(move || {
                get_settings(&phonon_device_handle).selected_output_device
            })),
        );
        let managers::natural_phonon::PhononStack {
            backend: phonon,
            health: phonon_health,
            coordinator: phonon_coordinator,
        } = managers::natural_phonon::build_phonon_stack(
            phonon_app_data.join("phonon_models"),
            phonon_app_data.join("phonon_scratch"),
            phonon_factory,
        );
        {
            let phonon_event_handle = app_handle.clone();
            phonon_health.set_event_sink(commands::phonon_voice::phonon_failure_sink(Arc::new(
                move |name: &str, payload: serde_json::Value| {
                    let _ = phonon_event_handle.emit(name, payload);
                },
            )));
        }
        app_handle.manage(commands::phonon_voice::PhononCoordinator(
            phonon_coordinator.clone(),
        ));
        app_handle.manage(commands::phonon_voice::PhononHealth(phonon_health.clone()));

        let choice_handle = app_handle.clone();
        let ready_coordinator = natural_coordinator.clone();
        let phonon_ready_coordinator = phonon_coordinator.clone();
        let router = EngineBackend::with_phonon(
            system,
            system_queue,
            Some(natural),
            natural_health.clone(),
            Some(phonon),
            phonon_health,
            RouterConfig {
                choice: Arc::new(move || {
                    let s = get_settings(&choice_handle);
                    RouterChoice {
                        natural: s.tts_engine == settings::TtsEngine::Natural,
                        voice: managers::natural_voices::normalize_natural_voice(
                            &s.tts_natural_voice,
                        ),
                        phonon: s.tts_engine == settings::TtsEngine::Phonon,
                    }
                }),
                model_ready: Arc::new(move || {
                    ready_coordinator.state() == managers::natural_model::ModelState::Ready
                }),
                phonon_ready: Arc::new(move || {
                    phonon_ready_coordinator.state() == managers::natural_model::ModelState::Ready
                }),
            },
        );
        Arc::new(managers::tts::TtsManager::new(Box::new(router)))
    };
    #[cfg(not(target_os = "macos"))]
    let tts_manager = Arc::new(managers::tts::TtsManager::new(
        managers::tts::default_backend(),
    ));
    app_handle.manage(tts_manager.clone());
    // Background coding jobs: one manager, one announcer, one exit coordinator (M3 S3d).
    let (job_wiring, job_runtime) = job_runtime::build(app_handle, tts_manager.clone());
    app_handle.manage(job_wiring);
    // Automatic-update admission restore (DESIGN R7-3, R8-2, R9-3). Placed here: the ownership
    // gate and the agent-job manager are managed by now, and nothing below can start work yet.
    // Listeners, wake word, ambient, conversation, the tray and the scheduler all come later.
    managers::source_update_driver::restore_admission(app_handle);

    // Bubble shell state (G4b): one core, created before anything can report a session or a line.
    bubble_native::init(app_handle);
    // Chat window transcript (DESIGN-CHAT 2.1): one in-memory log, fed by the conversation
    // platform and emitted through the app. Recording follows the saved setting.
    let chat_log = managers::chat_log::ChatLog::start(
        Arc::new(commands::chat::TauriChatEmitter {
            app: app_handle.clone(),
        }),
        get_settings(app_handle).converse_chat_log,
    );
    app_handle.manage(chat_log.clone());
    let chat_tap = Arc::new(managers::chat_tap::ChatTap::new(chat_log));
    // Conversation sessions: one controller, built with the app-backed platform.
    app_handle.manage(Arc::new(
        managers::converse_runtime::ConverseController::new(Arc::new(
            managers::converse_platform::RealPlatform::new(app_handle.clone())
                .with_job_runtime(job_runtime)
                .with_chat_tap(chat_tap),
        )),
    ));
    register_tts_listeners(app_handle, tts_manager);
    // Vault RAG runtime (DESIGN 3.6): None on any init failure; startup only queues the settings.
    app_handle.manage(managers::vault_rag_runtime::start_for_app(app_handle));
    let startup_settings = get_settings(app_handle);
    if startup_settings.wake_word_enabled {
        wakeword_listener.start();
    } else if crate::settings::ambient_effective(&startup_settings) {
        ambient_listener.start();
    }
    // Background "new version available" checks. Only this normal startup path
    // reaches here; the headless CLI path returns earlier in `setup`.
    managers::source_update::spawn_scheduler(app_handle.clone());
    // A background update started by an earlier Vox may still be running (or may have
    // finished while Vox restarted): adopt it, never boot it out.
    managers::source_update::adopt_existing_run(app_handle);
    app_handle.manage(tray::TrayState::new());

    // Note: Shortcuts are NOT initialized here.
    // The frontend is responsible for calling the `initialize_shortcuts` command
    // after permissions are confirmed (on macOS) or after onboarding completes.
    // This matches the pattern used for Enigo initialization.

    // Set up signal handlers for toggling transcription. On Linux, SIGUSR1 is
    // deliberately not handled — it belongs to WebKitGTK's garbage collector
    // (#1660) — see signal_handle.rs.
    #[cfg(unix)]
    signal_handle::setup_signal_handler(app_handle.clone());

    // The macOS activation policy for a start-hidden launch is applied before
    // the event loop runs (see `apply_startup_activation_policy`), not here:
    // by the time `setup` runs the app has already launched as a Regular
    // (Dock) app, and demoting it at runtime is unreliable (#1787).

    // Get the current theme to set the appropriate initial icon
    let initial_theme = tray::get_current_theme(app_handle);

    // Choose the appropriate initial icon based on theme
    let initial_icon_path = tray::get_icon_path(initial_theme, tray::TrayIconState::Idle, false);

    // Resolving and loading the initial tray icon must NEVER panic here:
    // this runs inside `setup()`, which macOS's AppKit invokes synchronously
    // from `applicationDidFinishLaunching` via an Objective-C callback. A
    // Rust panic crossing that boundary cannot unwind (it hits
    // `panic_cannot_unwind` and the whole process aborts via SIGABRT instead
    // of returning a normal tauri::Result error) -- confirmed to happen for
    // real: an intermittent `UnknownPath` resolve failure here (observed
    // after a sleep/wake cycle) previously crashed the app on every single
    // launch attempt with no recoverable error path at all. Falling back to
    // no custom tray icon on failure is vastly preferable to that.
    let initial_icon = match tray::load_tray_icon(
        app_handle
            .path()
            .resolve(initial_icon_path, tauri::path::BaseDirectory::Resource),
    ) {
        Ok(image) => Some(image),
        Err(err) => {
            log::error!(
                "Failed to resolve/load initial tray icon '{initial_icon_path}': {err} -- continuing startup without a custom tray icon instead of crashing"
            );
            None
        }
    };

    let mut tray_builder = TrayIconBuilder::new()
        .tooltip(tray::tray_tooltip())
        // Not a template image: the Vox tray glyph is a deliberately colorful
        // gold orb, not a monochrome system-style silhouette. A template
        // image here would make AppKit discard all RGB and render
        // alpha-only black/white, which is exactly the "not colorful" bug
        // this fixes.
        .icon_as_template(false);
    if let Some(icon) = initial_icon {
        tray_builder = tray_builder.icon(icon);
    }

    // Windows notification-area convention: left click opens the app, right click
    // shows the menu. Elsewhere (macOS menu bar, Linux) the menu stays on left click.
    #[cfg(target_os = "windows")]
    {
        tray_builder = tray_builder
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
                let opens_window = matches!(
                    event,
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    }
                );
                if opens_window {
                    show_main_window(tray.app_handle());
                }
            });
    }
    #[cfg(not(target_os = "windows"))]
    {
        tray_builder = tray_builder.show_menu_on_left_click(true);
    }

    let tray = tray_builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "settings" => {
                show_main_window(app);
            }
            "secure_input_warning" => {
                // Full explanation lives in the settings-window banner
                show_main_window(app);
            }
            "update_available" | "update_failed" => {
                // Only opens the window; installing is an explicit action there.
                show_main_window(app);
            }
            "check_updates" => {
                let settings = settings::get_settings(app);
                if settings::update_checks_effectively_enabled(&settings) {
                    show_main_window(app);
                    let _ = app.emit("check-for-updates", ());
                }
            }
            "copy_last_transcript" => {
                tray::copy_last_transcript(app);
            }
            "converse" => {
                commands::converse::toggle_conversation(app);
            }
            "chat" => {
                let _ = chat_surface::toggle(app);
            }
            "orb" => orb_native::toggle_visibility(app),
            "listening_mode:hotkey" => {
                let _ = commands::wakeword::wakeword_set_enabled(app.clone(), false);
                let _ = commands::ambient::ambient_set_enabled(app.clone(), false);
            }
            "listening_mode:wake_word" => {
                let _ = commands::wakeword::wakeword_set_enabled(app.clone(), true);
            }
            "listening_mode:ambient" => {
                let _ = commands::ambient::ambient_set_enabled(app.clone(), true);
            }
            "unload_model" => {
                let transcription_manager = app.state::<Arc<TranscriptionManager>>();
                if !transcription_manager.is_model_loaded() {
                    log::warn!("No model is currently loaded.");
                    return;
                }
                match transcription_manager.unload_model() {
                    Ok(()) => log::info!("Model unloaded via tray."),
                    Err(e) => log::error!("Failed to unload model via tray: {}", e),
                }
            }
            "cancel" => {
                // Conversation-aware cancel (same routing as the overlay button).
                commands::cancel_routed(app);
            }
            "cancel_job" => {
                tray::invoke_cancel_job_handler();
            }
            "quit" => {
                let job_wiring = app.state::<job_runtime::JobWiring>();
                let quit_handle = app.clone();
                job_wiring.exit.request_exit(
                    &job_wiring.manager,
                    move |_result| quit_handle.exit(0),
                    &crate::managers::agent_jobs::ExitConfig::default(),
                );
            }
            id if id.starts_with("model_select:") => {
                let model_id = id.strip_prefix("model_select:").unwrap().to_string();
                let current_model = settings::get_settings(app).selected_model;
                if model_id == current_model {
                    return;
                }
                let app_clone = app.clone();
                std::thread::spawn(move || {
                    match commands::models::switch_active_model(&app_clone, &model_id) {
                        Ok(()) => {
                            log::info!("Model switched to {} via tray.", model_id);
                        }
                        Err(e) => {
                            log::error!("Failed to switch model via tray: {}", e);
                        }
                    }
                    tray::update_tray_menu(&app_clone);
                });
            }
            _ => {}
        })
        .build(app_handle)
        .unwrap();
    app_handle.manage(tray);

    // Initialize tray menu with idle state
    tray::update_tray_menu(app_handle);

    // Apply show_tray_icon setting
    let settings = settings::get_settings(app_handle);
    if !settings.show_tray_icon {
        tray::set_tray_visibility(app_handle, false);
    }

    // Refresh tray menu when model state changes
    let app_handle_for_listener = app_handle.clone();
    app_handle.listen("model-state-changed", move |_| {
        tray::update_tray_menu(&app_handle_for_listener);
    });

    // Apply the autostart preference (SMAppService login item on macOS 13+,
    // tauri-plugin-autostart elsewhere)
    autostart::apply_autostart(app_handle, settings.autostart_enabled);

    // Create the recording overlay window (hidden by default)
    utils::create_recording_overlay(app_handle);

    // Show the floating orb when the setting allows it. Nothing is built while it is hidden.
    orb_native::apply_visibility(app_handle);
}

#[tauri::command]
#[specta::specta]
fn trigger_update_check(app: AppHandle) -> Result<(), String> {
    let settings = settings::get_settings(&app);
    if !settings::update_checks_effectively_enabled(&settings) {
        return Ok(());
    }
    app.emit("check-for-updates", ())
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
#[specta::specta]
fn show_main_window_command(app: AppHandle) -> Result<(), String> {
    show_main_window(&app);
    Ok(())
}

/// Convert an unexpected panic on the headless worker into a normal CLI
/// failure. Without this guard the Tauri event loop remains alive after the
/// worker exits, leaving `--transcribe-file` hung indefinitely.
fn run_headless_guarded<F>(operation: F) -> i32
where
    F: FnOnce() -> i32,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation)) {
        Ok(code) => code,
        Err(payload) => {
            let message = if let Some(message) = payload.downcast_ref::<&str>() {
                (*message).to_string()
            } else if let Some(message) = payload.downcast_ref::<String>() {
                message.clone()
            } else {
                "unknown panic".to_string()
            };
            eprintln!("error: headless transcription panicked: {message}");
            1
        }
    }
}

#[cfg(test)]
mod headless_guard_tests {
    use super::run_headless_guarded;

    #[test]
    fn preserves_normal_exit_codes() {
        assert_eq!(run_headless_guarded(|| 2), 2);
    }

    #[test]
    fn converts_worker_panics_to_runtime_failures() {
        assert_eq!(run_headless_guarded(|| panic!("simulated failure")), 1);
    }
}

/// Headless one-shot transcription for the `--transcribe-file` / `--list-devices`
/// path. Drives the same `TranscriptionManager::transcribe` the app uses; no
/// mic, no VAD, no download. Returns a process exit code (0 ok, 1 runtime
/// failure, 2 bad input/usage).
fn run_headless_transcription(app: &AppHandle, args: &CliArgs) -> i32 {
    use std::time::Instant;

    // --list-devices: print registered compute devices (with indices) and exit.
    // Useful on multi-GPU machines to discover the index for --device-index.
    if args.list_devices {
        let devices = crate::managers::transcription::describe_compute_devices();
        if devices.is_empty() {
            println!("No transcribe-cpp compute devices registered.");
        } else {
            println!("transcribe-cpp compute devices:");
            for d in &devices {
                println!("  {}", d);
            }
        }
        if args.transcribe_file.is_none() {
            return 0;
        }
    }

    // --list-models: print the model registry (catalog + on-disk + custom) with
    // their ids — the same ids `--model` accepts — then exit. `--json` emits the
    // full ModelInfo array for scripting.
    if args.list_models {
        let model_manager = app.state::<Arc<ModelManager>>();
        let models = model_manager.get_available_models();
        if args.json {
            match serde_json::to_string_pretty(&models) {
                Ok(s) => println!("{}", s),
                Err(e) => {
                    eprintln!("error: failed to serialize models: {}", e);
                    return 1;
                }
            }
        } else if models.is_empty() {
            println!("No models available.");
        } else {
            println!("Available models (✓ = installed):");
            let width = models.iter().map(|m| m.id.len()).max().unwrap_or(0);
            for m in &models {
                let mark = if m.is_downloaded { "✓" } else { " " };
                let rec = if m.is_recommended {
                    "  [recommended]"
                } else {
                    ""
                };
                println!(
                    "  {}  {:<width$}  {}{}",
                    mark,
                    m.id,
                    m.name,
                    rec,
                    width = width
                );
            }
        }
        if args.transcribe_file.is_none() {
            return 0;
        }
    }

    let Some(wav) = args.transcribe_file.clone() else {
        return 0;
    };

    // read_wav_samples reads 16-bit int samples and does no validation; the app
    // only ever saves 16 kHz mono 16-bit PCM, so reject anything else rather than
    // transcribe garbage / mis-time / mis-decode.
    match hound::WavReader::open(&wav) {
        Ok(reader) => {
            let spec = reader.spec();
            if spec.sample_rate != 16_000
                || spec.channels != 1
                || spec.bits_per_sample != 16
                || spec.sample_format != hound::SampleFormat::Int
            {
                eprintln!(
                    "error: expected 16 kHz mono 16-bit PCM WAV, got {} Hz / {} ch / {}-bit {:?}",
                    spec.sample_rate, spec.channels, spec.bits_per_sample, spec.sample_format
                );
                return 2;
            }
        }
        Err(e) => {
            eprintln!("error: cannot open {}: {}", wav.display(), e);
            return 2;
        }
    }

    let samples = match crate::audio_toolkit::read_wav_samples(&wav) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("error: failed to read {}: {}", wav.display(), e);
            return 2;
        }
    };
    let audio_secs = samples.len() as f64 / 16_000.0;

    let tm = app.state::<Arc<TranscriptionManager>>();

    let model_id = args
        .model
        .clone()
        .unwrap_or_else(|| get_settings(app).selected_model);
    if model_id.is_empty() {
        eprintln!("error: no model selected (pass --model or pick one in the app)");
        return 2;
    }

    // --device-index hard-selects a compute device by its --list-devices registry
    // index (transcribe-cpp / whisper-family models only; not persisted). Omit it
    // to use the persisted accelerator setting.
    let device_index = args.device_index;
    let requested_device = match device_index {
        Some(idx) => format!("index {}", idx),
        None => "settings".to_string(),
    };

    // Cold load (timed).
    let load_start = Instant::now();
    if let Err(e) = tm.load_model_with_device(&model_id, device_index) {
        eprintln!("error: load_model('{}') failed: {}", model_id, e);
        return 1;
    }
    let load_ms = load_start.elapsed().as_millis() as u64;
    let bound_backend = tm.current_backend();

    let runs = args.repeat.unwrap_or(1).max(1);
    let mut times_ms: Vec<u64> = Vec::new();
    let mut text = String::new();
    for i in 0..runs {
        // If the model's unload-timeout is "Immediately", transcribe() unloads
        // the engine after each run; reload (untimed) so repeats keep working
        // and the inference timing below stays clean.
        if !tm.is_model_loaded() {
            if let Err(e) = tm.load_model_with_device(&model_id, device_index) {
                eprintln!("error: reload before run {} failed: {}", i + 1, e);
                return 1;
            }
        }
        let t = Instant::now();
        match tm.transcribe(samples.clone()) {
            Ok(out) => text = out,
            Err(e) => {
                eprintln!("error: transcribe failed: {}", e);
                return 1;
            }
        }
        times_ms.push(t.elapsed().as_millis() as u64);
    }
    let best_ms = times_ms.iter().copied().min().unwrap_or(0);
    let rtf = if best_ms > 0 {
        audio_secs / (best_ms as f64 / 1000.0)
    } else {
        0.0
    };

    if args.json {
        println!(
            "{}",
            serde_json::json!({
                "model": model_id,
                "requested_device": requested_device,
                "bound_backend": bound_backend,
                "audio_secs": audio_secs,
                "load_ms": load_ms,
                "transcribe_ms": times_ms,
                "best_ms": best_ms,
                "rtf": rtf,
                "text": text,
            })
        );
    } else {
        println!(
            "model={} device={} backend={} audio={:.2}s load={}ms best={}ms rtf={:.2}x",
            model_id,
            requested_device,
            bound_backend.as_deref().unwrap_or("?"),
            audio_secs,
            load_ms,
            best_ms,
            rtf,
        );
        println!("text: {}", text);
    }
    0
}

/// The tauri-specta command/event/type registry. Shared by `run()` (which exports the
/// TypeScript bindings in debug builds) and the headless `export_bindings` test.
pub fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            shortcut::change_binding,
            shortcut::reset_binding,
            shortcut::change_shortcut_activation_setting,
            shortcut::change_hold_threshold_ms_setting,
            shortcut::change_audio_feedback_setting,
            shortcut::change_audio_feedback_volume_setting,
            shortcut::change_sound_theme_setting,
            shortcut::change_theme_setting,
            shortcut::update_custom_theme_colors,
            shortcut::change_start_hidden_setting,
            shortcut::change_autostart_setting,
            shortcut::change_translate_to_english_setting,
            shortcut::change_selected_language_setting,
            shortcut::change_overlay_position_setting,
            shortcut::change_overlay_style_setting,
            shortcut::change_debug_mode_setting,
            shortcut::change_word_correction_threshold_setting,
            shortcut::change_extra_recording_buffer_setting,
            shortcut::change_paste_delay_ms_setting,
            shortcut::change_paste_delay_after_ms_setting,
            shortcut::change_reliable_paste_setting,
            shortcut::change_paste_method_setting,
            shortcut::get_available_typing_tools,
            shortcut::change_typing_tool_setting,
            shortcut::change_external_script_path_setting,
            shortcut::change_clipboard_handling_setting,
            shortcut::change_auto_submit_setting,
            shortcut::change_auto_submit_key_setting,
            shortcut::change_post_process_enabled_setting,
            shortcut::change_experimental_enabled_setting,
            shortcut::change_voice_commands_setting,
            shortcut::change_memory_enabled_setting,
            commands::conversation_records::change_conversation_records_setting,
            commands::conversation_records::change_conversation_records_to_vault_setting,
            commands::conversation_records::change_vault_context_setting,
            commands::conversation_records::change_vault_path_setting,
            commands::vault_rag::change_vault_rag_setting,
            commands::vault_rag::change_vault_rag_folders_setting,
            commands::vault_rag::vault_index_status,
            commands::vault_rag::vault_reindex,
            commands::vault_rag::vault_search_preview,
            commands::vault_rag::vault_rag_probe,
            commands::conversation_records::list_conversation_records,
            commands::agent_tools::change_agent_tools_setting,
            commands::agent_tools::change_agent_tools_guard_actions_setting,
            commands::agent_tools::change_agent_project_roots_setting,
            commands::agent_jobs::change_agent_confirm_actions,
            commands::agent_jobs::change_agent_jobs_enabled,
            commands::agent_jobs::change_agent_jobs_timeout_minutes,
            commands::agent_jobs::change_agent_jobs_agents,
            commands::agent_jobs::change_agent_jobs_agent_flag,
            commands::agent_jobs::change_agent_jobs_ask_enabled,
            commands::agent_jobs::agent_job_status,
            commands::agent_jobs::agent_job_cancel,
            shortcut::change_agent_bridge_enabled_setting,
            shortcut::change_agent_bridge_binary_path_setting,
            shortcut::change_voice_media_controls_setting,
            shortcut::change_tts_enabled_setting,
            shortcut::change_tts_voice_setting,
            shortcut::change_tts_rate_setting,
            shortcut::change_tts_engine_setting,
            shortcut::change_tts_natural_voice_setting,
            commands::natural_voice::natural_voice_status,
            commands::natural_voice::natural_voice_download,
            commands::natural_voice::natural_voice_cancel_download,
            commands::natural_voice::natural_voice_delete,
            commands::natural_voice::natural_voice_retry,
            commands::natural_voice::list_natural_voices,
            commands::phonon_voice::phonon_voice_status,
            commands::phonon_voice::phonon_voice_install,
            commands::phonon_voice::phonon_voice_import_folder,
            commands::phonon_voice::phonon_voice_cancel,
            commands::phonon_voice::phonon_voice_delete,
            commands::tts_engines::list_tts_engines,
            shortcut::tts_speak_test,
            shortcut::tts_stop,
            shortcut::change_post_process_base_url_setting,
            shortcut::change_post_process_api_key_setting,
            shortcut::change_post_process_model_setting,
            shortcut::set_post_process_provider,
            shortcut::fetch_post_process_models,
            shortcut::add_post_process_prompt,
            shortcut::update_post_process_prompt,
            shortcut::delete_post_process_prompt,
            shortcut::set_post_process_selected_prompt,
            shortcut::update_custom_words,
            shortcut::suspend_all_bindings,
            shortcut::resume_all_bindings,
            shortcut::change_mute_while_recording_setting,
            shortcut::change_append_trailing_space_setting,
            shortcut::change_lazy_stream_close_setting,
            shortcut::change_vad_enabled_setting,
            shortcut::change_vad_backend_setting,
            shortcut::change_filler_word_removal_enabled_setting,
            shortcut::change_chinese_script_setting,
            shortcut::change_wake_word_action_setting,
            shortcut::change_converse_barge_in_setting,
            shortcut::change_app_language_setting,
            shortcut::change_update_checks_setting,
            shortcut::change_show_whats_new_on_update_setting,
            shortcut::change_whats_new_last_seen_version_setting,
            shortcut::change_keyboard_implementation_setting,
            shortcut::get_keyboard_implementation,
            shortcut::change_show_tray_icon_setting,
            shortcut::change_transcribe_accelerator_setting,
            shortcut::change_ort_accelerator_setting,
            shortcut::change_transcribe_gpu_device,
            shortcut::get_available_accelerators,
            shortcut::handy_keys::start_handy_keys_recording,
            shortcut::handy_keys::stop_handy_keys_recording,
            secure_input::get_secure_input_status,
            secure_input::run_keyboard_diagnostic,
            trigger_update_check,
            show_main_window_command,
            commands::cancel_operation,
            commands::is_portable,
            commands::is_update_checks_locked,
            commands::source_update::get_update_mode,
            commands::source_update::check_source_update,
            commands::source_update::get_cached_source_update,
            commands::source_update::get_source_update_install_blocker,
            commands::source_update::start_source_update,
            commands::source_update::get_source_update_progress,
            commands::source_update::change_auto_update_setting,
            commands::source_update::get_auto_update_status,
            commands::get_app_dir_path,
            commands::get_app_settings,
            commands::get_default_settings,
            commands::get_log_dir_path,
            commands::set_log_level,
            commands::open_recordings_folder,
            commands::open_log_dir,
            commands::open_app_data_dir,
            commands::check_apple_intelligence_available,
            commands::initialize_enigo,
            commands::initialize_shortcuts,
            commands::models::get_available_models,
            commands::models::get_model_info,
            commands::models::download_model,
            commands::models::delete_model,
            commands::models::cancel_download,
            commands::models::set_active_model,
            commands::models::get_current_model,
            commands::models::get_transcription_model_status,
            commands::models::is_model_loading,
            commands::models::rescan_local_models,
            commands::audio::update_microphone_mode,
            commands::audio::get_microphone_mode,
            commands::audio::get_windows_microphone_permission_status,
            commands::audio::open_microphone_privacy_settings,
            commands::audio::get_available_microphones,
            commands::audio::set_selected_microphone,
            commands::audio::get_selected_microphone,
            commands::audio::get_available_output_devices,
            commands::audio::set_selected_output_device,
            commands::audio::get_selected_output_device,
            commands::audio::play_test_sound,
            commands::audio::check_custom_sounds,
            commands::audio::set_clamshell_microphone,
            commands::audio::get_clamshell_microphone,
            commands::audio::is_recording,
            commands::audio::get_microphone_channels,
            commands::audio::set_selected_channel,
            commands::transcription::set_model_unload_timeout,
            commands::transcription::get_model_load_status,
            commands::transcription::unload_model_manually,
            commands::history::get_history_entries,
            commands::history::search_history_entries,
            commands::history::toggle_history_entry_saved,
            commands::history::get_audio_file_path,
            commands::history::delete_history_entry,
            commands::history::retry_history_entry_transcription,
            commands::history::update_history_limit,
            commands::history::update_recording_retention_period,
            commands::tts_composer::change_tts_composer_settings,
            commands::tts_composer::list_tts_composer_providers,
            commands::tts_composer::preview_speech_composer,
            commands::tts_voices::list_tts_voices,
            commands::memory::memory_remember,
            commands::memory::memory_recall,
            commands::memory::memory_list_all,
            commands::memory::memory_forget,
            commands::memory::memory_set_enabled,
            commands::conversation_memory::clear_conversation_memory,
            commands::conversation_memory::conversation_memory_count,
            commands::conversation_memory::change_conversation_memory_setting,
            shortcut::change_agent_context_setting,
            shortcut::agent_clear_conversation,
            commands::ambient::ambient_set_enabled,
            commands::ambient::ambient_clear,
            commands::wakeword::wakeword_set_enabled,
            shortcut::change_wake_word_confidence_threshold_setting,
            shortcut::change_wake_word_cooldown_ms_setting,
            shortcut::change_wake_word_silence_timeout_ms_setting,
            shortcut::change_ambient_wake_name_setting,
            shortcut::change_ambient_window_ms_setting,
            shortcut::change_custom_filler_words_setting,
            shortcut::change_agent_bridge_prompt_flag_setting,
            shortcut::change_acp_agent_enabled_setting,
            shortcut::change_acp_dictation_enabled_setting,
            shortcut::change_acp_agent_command_setting,
            shortcut::change_acp_agent_args_setting,
            shortcut::change_acp_agent_turn_timeout_setting,
            shortcut::change_acp_step_agent_setting,
            shortcut::acp_agent_test_connection,
            commands::acp_permission::acp_permission_respond,
            commands::acp_permission::change_acp_permission_mode_setting,
            commands::acp_permission::change_acp_permission_timeout_secs_setting,
            commands::acp_permission::change_conversation_agent_handoff_setting,
            commands::acp_permission::change_conversation_planner_setting,
            commands::brain::change_brain_connection_setting,
            commands::brain::set_brain_token,
            commands::brain::brain_acknowledge_test,
            commands::brain::brain_use_for_feature,
            commands::brain::brain_test_connection,
            commands::brain::brain_fetch_models,
            commands::first_run::first_run_finish_step,
            commands::voice_shortcuts::set_voice_shortcuts,
            commands::converse::change_converse_silence_ms_setting,
            commands::converse::change_converse_max_idle_turns_setting,
            commands::converse::change_converse_context_turns_setting,
            commands::converse::converse_start,
            commands::converse::converse_stop,
            commands::converse::converse_status,
            // optional voice features
            commands::aux_model::aux_model_status,
            commands::aux_model::aux_model_download,
            commands::aux_model::aux_model_cancel,
            commands::aux_model::aux_model_delete,
            commands::persona::list_personas,
            commands::persona::save_persona,
            commands::persona::delete_persona,
            commands::persona::apply_persona,
            commands::speaker::speaker_status,
            commands::speaker::speaker_enroll_begin,
            commands::speaker::speaker_enroll_step,
            commands::speaker::speaker_enroll_cancel,
            commands::speaker::speaker_enroll_finish,
            commands::speaker::speaker_delete_voiceprint,
            commands::converse_extras::change_converse_smart_turn_setting,
            commands::converse_extras::change_converse_latency_panel_setting,
            commands::chat::get_chat_snapshot,
            commands::chat::clear_chat_log,
            commands::chat::send_chat_message,
            commands::chat::change_converse_chat_log_setting,
            commands::chat::change_converse_chat_open_on_start_setting,
            commands::chat::toggle_chat_window_command,
            commands::chat::chat_hide_window,
            commands::chat::chat_stop_conversation,
            commands::orb::orb_pointer_down,
            commands::orb::orb_activate,
            commands::orb::orb_menu,
            commands::orb::orb_dismiss,
            commands::orb::orb_get_state,
            commands::orb::orb_set_reduced_motion,
            commands::bubbles::bubbles_ready,
            commands::orb::get_orb_support,
            commands::orb::change_orb_visible_setting,
            commands::orb::change_orb_size_setting,
            commands::orb::change_orb_size_px_setting,
            commands::chat_opacity::change_chat_opacity_setting,
            commands::appearance::change_accent_setting,
            commands::appearance::change_chat_theme_setting,
            commands::appearance::change_visual_style_setting,
            commands::appearance::change_orb_accent_setting,
            commands::orb::change_chat_presentation_setting,
            commands::orb::reset_orb_position,
            commands::converse_extras::change_speaker_filter_enabled_setting,
            commands::converse_extras::change_speaker_filter_sensitivity_setting,
            commands::realtime::change_exp_rt_bool_setting,
            commands::realtime::change_exp_rt_text_setting,
            helpers::clamshell::is_laptop,
        ])
        .events(collect_events![
            managers::history::HistoryUpdatePayload,
            managers::transcription::StreamTextEvent,
            managers::transcription::StreamPhaseEvent,
        ])
        .typ::<settings::AppSettings>()
        .typ::<bubble_notice::NoticeCode>()
        .typ::<orb_converse::OrbRefusalPayload>()
        .typ::<appearance::AppearanceSnapshot>()
        .typ::<managers::wakeword::WakeWordDetection>()
        .typ::<commands::ambient::AmbientAddressedPayload>()
        .typ::<managers::converse_platform::ConverseStateEvent>()
        .typ::<commands::natural_voice::NaturalVoiceDownloadProgress>()
        .typ::<commands::natural_voice::NaturalVoiceDownloadFinished>()
        .typ::<commands::aux_model::AuxProgressEvent>()
        .typ::<commands::aux_model::AuxFinishedEvent>()
        .typ::<managers::converse_latency::LatencyEvent>()
        .typ::<commands::speaker::SpeakerStatus>()
        .typ::<commands::speaker::SpeakerCmdError>()
        .typ::<managers::speaker_enroll::StepResult>()
        .typ::<managers::speaker_enroll::EnrollError>()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(cli_args: CliArgs) {
    // Avoid ggml-metal residency-set teardown assertions when a native engine
    // outlives the Tauri shutdown sequence (#1902). This must happen before
    // transcribe-cpp initializes its Metal device. Advanced users can restore
    // upstream residency behavior with HANDY_METAL_RESIDENCY=1.
    #[cfg(target_os = "macos")]
    if std::env::var("HANDY_METAL_RESIDENCY").as_deref() == Ok("1") {
        // ggml treats GGML_METAL_NO_RESIDENCY as presence-based, so remove an
        // inherited value as well when explicitly opting back in.
        std::env::remove_var("GGML_METAL_NO_RESIDENCY");
    } else {
        std::env::set_var("GGML_METAL_NO_RESIDENCY", "1");
    }

    // Pin glibc's dynamic mmap threshold before the first large allocation,
    // so per-dictation transient buffers are returned to the OS on free
    // instead of accumulating in malloc arenas (#1792). No-op off Linux/glibc.
    memory::init_allocator();

    // Detect portable mode before anything else
    portable::init();

    // Parse console logging directives from RUST_LOG, falling back to info-level logging
    // when the variable is unset
    let console_filter = build_console_filter();

    let specta_builder = specta_builder();

    #[cfg(debug_assertions)] // <- Only export on non-release builds
    // Dev convenience only (regenerates bindings.ts while running `cargo
    // tauri dev`/`cargo run` from the project root, where this relative
    // path resolves). Must never crash a real running instance: the same
    // debug-assertions build also ships as the installed .app (`tauri
    // build --debug`), where CWD is unrelated to the source tree and this
    // path is never writable -- panicking here previously killed the app
    // instantly on every non-dev launch (Finder/Dock/Spotlight/launchd),
    // completely independent of and easily mistaken for the Gatekeeper
    // issues launched from those same surfaces.
    if let Err(err) = specta_builder.export(
        Typescript::default().bigint(BigIntExportBehavior::Number),
        "../src/bindings.ts",
    ) {
        log::warn!(
            "Skipping TypeScript bindings export (expected outside `cargo tauri dev`): {err}"
        );
    }

    let specta_invoke_handler = specta_builder.invoke_handler();
    // Single choke point for the per-window command allow-list (DESIGN-CHAT A8). The chat
    // webview may call only its two commands; plugin commands never reach this handler.
    let invoke_handler = move |invoke: tauri::ipc::Invoke<tauri::Wry>| {
        if !window_commands::window_command_allowed(
            invoke.message.webview().label(),
            invoke.message.command(),
        ) {
            invoke
                .resolver
                .reject("command not allowed from this window");
            return true;
        }
        specta_invoke_handler(invoke)
    };

    // The headless path must run as its own instance (see the single-instance
    // note below), not forward to an already-running app.
    let headless_mode =
        cli_args.transcribe_file.is_some() || cli_args.list_devices || cli_args.list_models;

    #[allow(unused_mut)]
    let mut builder = tauri::Builder::default()
        .device_event_filter(tauri::DeviceEventFilter::Always)
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            with_suppressed_log_targets(LogBuilder::new())
                .level(log::LevelFilter::Trace) // Set to most verbose level globally
                .max_file_size(500_000)
                .rotation_strategy(RotationStrategy::KeepOne)
                .clear_targets()
                .targets([
                    // Console output respects RUST_LOG environment variable. In
                    // headless mode (--transcribe-file/--list-devices/--list-models)
                    // stdout carries only the result (JSON or plain), so send console
                    // logs to stderr instead to keep stdout clean for CI parsing.
                    Target::new(if headless_mode {
                        TargetKind::Stderr
                    } else {
                        TargetKind::Stdout
                    })
                    .filter({
                        let console_filter = console_filter.clone();
                        move |metadata| console_filter.enabled(metadata)
                    }),
                    // File logs respect the user's settings (stored in FILE_LOG_LEVEL atomic)
                    Target::new(if let Some(data_dir) = portable::data_dir() {
                        TargetKind::Folder {
                            path: data_dir.join("logs"),
                            file_name: Some("vox".into()),
                        }
                    } else {
                        TargetKind::LogDir {
                            file_name: Some("vox".into()),
                        }
                    })
                    .filter(|metadata| {
                        let file_level = FILE_LOG_LEVEL.load(Ordering::Relaxed);
                        metadata.level() <= level_filter_from_u8(file_level)
                    }),
                    // Stream logs to the webview (via the `log://log` event) so the
                    // debug panel's live log viewer can show them in real time. Only
                    // active while debug mode is on (its sole consumer), and shares the
                    // file log level so the "Log Level" setting controls verbosity.
                    Target::new(TargetKind::Webview).filter(|metadata| {
                        WEBVIEW_LOG_STREAMING.load(Ordering::Relaxed)
                            && metadata.level()
                                <= level_filter_from_u8(FILE_LOG_LEVEL.load(Ordering::Relaxed))
                    }),
                ])
                .build(),
        );

    #[cfg(target_os = "macos")]
    {
        builder = builder.plugin(tauri_nspanel::init());
    }

    // Single-instance forwards CLI args to an already-running Vox and exits.
    // That would make the headless path
    // (--transcribe-file/--list-devices/--list-models) a silent no-op whenever the
    // app is already open, so skip it in headless mode and run a standalone
    // instance instead.
    if !headless_mode {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if args.iter().any(|a| a == "--toggle-transcription") {
                signal_handle::send_transcription_input(app, "transcribe", "CLI");
            } else if args.iter().any(|a| a == "--toggle-post-process") {
                signal_handle::send_transcription_input(app, "transcribe_with_post_process", "CLI");
            } else if args.iter().any(|a| a == "--cancel") {
                commands::cancel_routed(app);
            } else {
                // A second process was launched without remote-control flags
                // (e.g. the binary run from a shell). On macOS, relaunching the
                // bundle from Spotlight/Finder/Dock does not start a process —
                // it arrives as RunEvent::Reopen below — but treat this the
                // same way: raise the window and recreate a possibly vanished
                // tray icon (#1948).
                #[cfg(target_os = "macos")]
                tray::recreate_tray_icon(app);
                show_main_window(app);
            }
        }));
    }

    #[allow(unused_mut)]
    let mut app = builder
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_macos_permissions::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            Some(vec![]),
        ))
        .manage(cli_args.clone())
        .setup(move |app| {
            #[cfg(target_os = "windows")]
            log::info!(
                "Vulkan layer policy: VK_LOADER_LAYERS_DISABLE={:?}, HANDY_KEEP_VULKAN_IMPLICIT_LAYERS={}",
                std::env::var_os("VK_LOADER_LAYERS_DISABLE"),
                utils::env_flag_enabled("HANDY_KEEP_VULKAN_IMPLICIT_LAYERS"),
            );

            specta_builder.mount_events(app);

            // Headless one-shot path (`--transcribe-file` / `--list-devices` /
            // `--list-models`): initialize only what transcription needs — the
            // store/paths plugins, the model + transcription managers, and the
            // transcribe-cpp backend + accelerator settings — then run on a worker
            // thread and exit. Deliberately skips the window, tray, overlay, audio
            // recorder (so it never opens the mic, even with always_on_microphone),
            // signal handlers, and autostart that initialize_core_logic sets up.
            if headless_mode {
                let app_handle = app.handle().clone();
                let model_manager = Arc::new(
                    ModelManager::new(&app_handle).expect("Failed to initialize model manager"),
                );
                let transcription_manager = Arc::new(
                    TranscriptionManager::new(&app_handle, model_manager.clone())
                        .expect("Failed to initialize transcription manager"),
                );
                app_handle.manage(model_manager);
                app_handle.manage(transcription_manager);
                managers::transcription::init_transcribe_backend();
                managers::transcription::report_compute_devices();
                managers::transcription::apply_accelerator_settings(&app_handle);

                let handle = app_handle.clone();
                let args = cli_args.clone();
                std::thread::spawn(move || {
                    let code = run_headless_guarded(|| run_headless_transcription(&handle, &args));
                    // Drop the loaded engine before teardown: ggml-metal's global
                    // device free asserts (SIGABRT) if a model's Metal resources
                    // are still alive at C++ static-destructor time.
                    if let Some(tm) = handle.try_state::<Arc<TranscriptionManager>>() {
                        let _ = tm.unload_model();
                    }
                    // process::exit (not app.exit, which exits 0 regardless) so the
                    // exit code propagates to the shell for CI gating. Flush first
                    // since process::exit runs no destructors / buffer flushes.
                    use std::io::Write;
                    let _ = std::io::stdout().flush();
                    let _ = std::io::stderr().flush();
                    std::process::exit(code);
                });
                return Ok(());
            }

            // Create main window programmatically so we can set data_directory
            // for portable mode (redirects WebView2 cache to portable Data dir)
            let mut win_builder =
                tauri::WebviewWindowBuilder::new(app, "main", tauri::WebviewUrl::App("/".into()))
                    .title("Vox")
                    .inner_size(680.0, 570.0)
                    .min_inner_size(680.0, 570.0)
                    .resizable(true)
                    .maximizable(true)
                    .visible(false);

            if let Some(data_dir) = portable::data_dir() {
                win_builder = win_builder.data_directory(data_dir.join("webview"));
            }

            // Only used on Windows, to disable WebView2 browser accelerators.
            #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
            let main_window = win_builder.build()?;

            // Disable WebView2 browser accelerators (F5, F6, Ctrl+F, F12, ...).
            // A settings window has no use for them, and pressing F6 while
            // recording a shortcut was reported to turn the whole window white
            // WebView2 can lose focus when activation policy is cycled.
            // DevTools stays enabled; only the F12 accelerator is lost.
            #[cfg(target_os = "windows")]
            {
                let _ = main_window.with_webview(|webview| unsafe {
                    use webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3;
                    use windows::core::Interface;

                    let result = webview
                        .controller()
                        .CoreWebView2()
                        .and_then(|core| core.Settings())
                        .and_then(|settings| settings.cast::<ICoreWebView2Settings3>())
                        .and_then(|settings| settings.SetAreBrowserAcceleratorKeysEnabled(false));

                    if let Err(error) = result {
                        log::warn!("Failed to disable WebView2 browser accelerators: {error}");
                    }
                });
            }

            let mut settings = get_settings(app.handle());

            // Apply the persisted appearance theme to the native title bar before
            // the window is shown, so it matches the in-app palette without a flash
            // of the wrong theme. See `apply_window_theme` for what this does per
            // platform.
            #[cfg(any(target_os = "windows", target_os = "macos"))]
            shortcut::apply_window_theme(
                app.handle(),
                settings.theme,
                &settings.custom_theme_colors.background,
            );

            // CLI --debug flag overrides debug_mode and log level (runtime-only, not persisted)
            if cli_args.debug {
                settings.debug_mode = true;
                settings.log_level = settings::LogLevel::Trace;
            }

            let tauri_log_level: tauri_plugin_log::LogLevel = settings.log_level.into();
            let file_log_level: log::Level = tauri_log_level.into();
            // Store the file log level in the atomic for the filter to use
            FILE_LOG_LEVEL.store(file_log_level.to_level_filter() as u8, Ordering::Relaxed);
            // Only forward logs to the webview while debug mode is on (the live log
            // viewer is the sole consumer and only exists in debug mode). This also
            // honors the runtime `--debug` override applied to `settings` above.
            WEBVIEW_LOG_STREAMING.store(settings.debug_mode, Ordering::Relaxed);
            let app_handle = app.handle().clone();
            app.manage(TranscriptionCoordinator::new(app_handle.clone()));

            initialize_core_logic(&app_handle);

            // Warm the persistent ACP agent in the background so the first request is fast.
            spawn_acp_prewarm(&app_handle);

            // Secure Input monitor (macOS): detects stuck secure input that
            // silently blocks keyed shortcuts, warns the user, and activates
            // the Carbon fallback. See secure_input.rs and issue #1578.
            secure_input::init(&app_handle);

            // Populate the overlay-enabled cache from initial settings so the
            // audio path (overlay::emit_levels, called ~24 Hz during recording)
            // can do a single atomic load instead of reading the Tauri store.
            // Kept in sync by shortcut::change_overlay_style_setting.
            overlay::update_overlay_enabled_cache(
                settings.overlay_style != settings::OverlayStyle::None,
            );

            // Pre-warm GPU/accelerator enumeration on a background thread. The first
            // device listing opens the GPU, which on macOS loads ggml's Metal library
            // and compiles it when the system shader cache does not have it yet, so it
            // stays off the startup path. get_available_accelerators then enumerates
            // ORT execution providers and transcribe-cpp compute devices; without this
            // the cost is paid synchronously when the user first opens Advanced
            // settings, freezing the UI. Result is cached in a OnceLock.
            std::thread::spawn(|| {
                crate::managers::transcription::report_compute_devices();
                let _ = crate::managers::transcription::get_available_accelerators();
            });

            // Hide tray icon if --no-tray was passed
            if cli_args.no_tray {
                tray::set_tray_visibility(&app_handle, false);
            }

            // Show main window only if not starting hidden.
            // CLI --start-hidden flag overrides the setting.
            // But if permission onboarding is required, always show the window.
            let should_hide = settings.start_hidden || cli_args.start_hidden;
            let should_force_show = should_force_show_permissions_window(&app_handle);

            // If start_hidden but tray is disabled, we must show the window
            // anyway. Without a tray icon, the dock is the only way back in.
            // Keep in sync with `apply_startup_activation_policy` (macOS).
            let tray_available = settings.show_tray_icon && !cli_args.no_tray;
            if should_force_show || !should_hide || !tray_available {
                show_main_window(&app_handle);
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            tauri::WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _res = window.hide();

                #[cfg(target_os = "macos")]
                {
                    let settings = get_settings(window.app_handle());
                    let tray_visible =
                        settings.show_tray_icon && !window.app_handle().state::<CliArgs>().no_tray;
                    if tray_visible {
                        // Tray is available: hide the dock icon, app lives in the tray
                        let res = window
                            .app_handle()
                            .set_activation_policy(tauri::ActivationPolicy::Accessory);
                        if let Err(e) = res {
                            log::error!("Failed to set activation policy: {}", e);
                        }
                    }
                    // No tray: keep the dock icon visible so the user can reopen
                }
            }
            tauri::WindowEvent::ThemeChanged(theme) => {
                log::info!("Theme changed to: {:?}", theme);
                // Re-apply the current tray state with the new theme's icon set
                utils::refresh_tray_icon(window.app_handle());
            }
            _ => {}
        })
        .invoke_handler(invoke_handler)
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    // Must sit between build() and run(): see the doc comment.
    #[cfg(target_os = "macos")]
    apply_startup_activation_policy(&mut app, headless_mode);

    app.run(|app, event| match &event {
        #[cfg(target_os = "macos")]
        tauri::RunEvent::Reopen { .. } => {
            // Fired when the already-running bundle is launched again from
            // Spotlight/Finder or the Dock icon is clicked. If the settings
            // window is hidden, the user is likely looking for a tray icon
            // that vanished (#1948): recreate it. When the window is
            // already visible this is just a focus request and the tray is
            // left alone.
            let window_visible = app
                .get_webview_window("main")
                .and_then(|w| w.is_visible().ok())
                .unwrap_or(false);
            if !window_visible {
                tray::recreate_tray_icon(app);
            }
            show_main_window(app);
        }
        // Teardown transcribe.cpp before exit
        tauri::RunEvent::ExitRequested { api, .. } => {
            if let Some(acp) = app.try_state::<Arc<managers::acp_agent::AcpAgentManager>>() {
                acp.close();
            }
            let job_wiring = app.state::<job_runtime::JobWiring>();
            let exit_handle = app.clone();
            let decision = job_wiring.exit.on_exit_requested(
                &job_wiring.manager,
                || api.prevent_exit(),
                move |_result| exit_handle.exit(0),
                &crate::managers::agent_jobs::ExitConfig::default(),
            );
            log::debug!("native exit request decision: {:?}", decision);
        }
        tauri::RunEvent::Exit => {
            bubble_native::mark_exiting(app);
            bubble_native::destroy_bubbles(app);
            if let Some(acp) = app.try_state::<Arc<managers::acp_agent::AcpAgentManager>>() {
                acp.close();
            }
            // Non-blocking: the coordinator and IO worker are detached (DESIGN 3.6 shutdown).
            if let Some(state) =
                app.try_state::<Option<Arc<managers::vault_rag_runtime::VaultRagRuntime>>>()
            {
                if let Some(runtime) = state.inner() {
                    runtime.shutdown();
                }
            }
            if let Some(wiring) = app.try_state::<job_runtime::JobWiring>() {
                // Idempotent belt: the exit coordinator has normally stopped the job already.
                let budgets = job_runtime::exit_fallback_budgets();
                wiring.manager.shutdown(&ShutdownConfig {
                    term_grace: budgets.term_grace,
                    drain: budgets.drain,
                    deadline: budgets.deadline,
                });
            }
            if let Some(tm) = app.try_state::<Arc<TranscriptionManager>>() {
                let _ = tm.unload_model();
            }
            // Chat transcript: stop the worker. Memory only, nothing to flush.
            if let Some(chat) = app.try_state::<Arc<managers::chat_log::ChatLog>>() {
                chat.shutdown();
            }
        }
        _ => {}
    });
}

#[cfg(test)]
mod acp_agent_settings_tests;
#[cfg(test)]
mod acp_dictation_switch_tests;
#[cfg(test)]
mod agent_bridge_settings_tests;
#[cfg(test)]
mod ambient_settings_tests;
#[cfg(test)]
mod settings_acp_permission_tests;
#[cfg(test)]
mod settings_validation_tests;
#[cfg(test)]
mod settings_write_audit_tests;

#[cfg(test)]
mod tts_listener_tests {
    use super::{decode_tts_payload, tts_phrase_to_speak, TTS_EVENTS};

    #[test]
    fn the_listener_subscribes_to_the_exchange_event_not_the_bare_reply() {
        assert_eq!(
            TTS_EVENTS,
            [
                "agent-bridge-exchange",
                "agent-bridge-error",
                "voice-command-executed",
                "voice-command-error",
            ]
        );
    }

    #[test]
    fn disabled_setting_never_speaks() {
        assert_eq!(
            tts_phrase_to_speak(false, "agent-bridge-reply", "hello"),
            None
        );
        assert_eq!(
            tts_phrase_to_speak(false, "voice-command-executed", "x"),
            None
        );
    }

    #[test]
    fn enabled_agent_reply_speaks_its_text() {
        assert_eq!(
            tts_phrase_to_speak(true, "agent-bridge-reply", "It is sunny."),
            Some("It is sunny.".to_string())
        );
    }

    #[test]
    fn command_outcomes_speak_fixed_phrases_not_payloads() {
        assert_eq!(
            tts_phrase_to_speak(true, "voice-command-executed", "open /secret"),
            Some(crate::managers::tts::DONE_PHRASE.to_string())
        );
        assert_eq!(
            tts_phrase_to_speak(true, "voice-command-error", "stack trace"),
            Some(crate::managers::tts::ERROR_PHRASE.to_string())
        );
    }

    #[test]
    fn unknown_event_is_never_spoken() {
        assert_eq!(tts_phrase_to_speak(true, "model-state-changed", "x"), None);
    }

    #[test]
    fn json_string_payload_is_decoded() {
        assert_eq!(decode_tts_payload("\"hi \\\"there\\\"\""), "hi \"there\"");
    }

    #[test]
    fn non_json_payload_falls_back_to_raw_text() {
        assert_eq!(decode_tts_payload("plain text"), "plain text");
    }
}

#[cfg(test)]
mod tts_error_kind_tests {
    use super::tts_error_kind;
    use crate::managers::tts::TtsError;

    #[test]
    fn error_kinds_are_fixed_content_free_names() {
        assert_eq!(tts_error_kind(&TtsError::Superseded), "superseded");
        assert_eq!(tts_error_kind(&TtsError::Recording), "recording");
        assert_eq!(
            tts_error_kind(&TtsError::Backend("secret".into())),
            "backend"
        );
    }
}

/// Headless bindings checks (no GUI). `export_bindings_to_env` writes a reference export to
/// the path in `BINDINGS_OUT` (never the committed file). `bindings_in_sync` compares the
/// committed `src/bindings.ts` with a fresh export on SEMANTIC parity only: the set of invoked
/// command names and the set of `AppSettings` field names.
#[cfg(test)]
mod bindings_export_tests {
    use super::specta_builder;
    use specta_typescript::{BigIntExportBehavior, Typescript};
    use std::collections::BTreeSet;

    fn export_to(path: &str) -> Result<(), Box<dyn std::error::Error>> {
        specta_builder()
            .export(
                Typescript::default().bigint(BigIntExportBehavior::Number),
                path,
            )
            .map_err(Into::into)
    }

    /// Names of every `TAURI_INVOKE("name"` call in a bindings file.
    fn invoke_names(src: &str) -> BTreeSet<String> {
        src.split("TAURI_INVOKE(\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .map(str::to_string)
            .collect()
    }

    /// Field names of `export type AppSettings = { ... }`: for each line before the next
    /// top-level `export type`, the identifier before `?:` or `:`. Comment lines are skipped.
    fn app_settings_fields(src: &str) -> BTreeSet<String> {
        let mut fields = BTreeSet::new();
        let Some(start) = src.find("export type AppSettings = {") else {
            return fields;
        };
        for line in src[start..].lines().skip(1) {
            let trimmed = line.trim();
            if line.starts_with("export type") || line.starts_with('}') {
                break;
            }
            if trimmed.is_empty()
                || trimmed.starts_with("//")
                || trimmed.starts_with('*')
                || trimmed.starts_with("/*")
            {
                continue;
            }
            // Several `name?: type;` entries may share one line.
            for entry in trimmed.split(';') {
                if let Some((name, _)) = entry.split_once(':') {
                    let name = name.trim().trim_end_matches('?');
                    if !name.is_empty()
                        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    {
                        fields.insert(name.to_string());
                    }
                }
            }
        }
        fields
    }

    #[test]
    fn app_settings_parser_reads_fields_and_skips_comments() {
        let src = "export type AppSettings = { \n\
            /**\n * doc: with colon\n */\n\
            alpha?: boolean; beta?: string | null; \n\
            // note: not a field\n\
            gamma: number;\n\
            }\n\
            export type Other = { delta?: boolean };\n";
        let expected: BTreeSet<String> = ["alpha", "beta", "gamma"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(app_settings_fields(src), expected);
    }

    #[test]
    fn invoke_parser_reads_names() {
        let src = "TAURI_INVOKE(\"a_cmd\", { x }); TAURI_INVOKE(\"b_cmd\")";
        let expected: BTreeSet<String> = ["a_cmd", "b_cmd"].iter().map(|s| s.to_string()).collect();
        assert_eq!(invoke_names(src), expected);
    }

    #[test]
    #[ignore = "writes a reference export to BINDINGS_OUT; never the committed file"]
    fn export_bindings_to_env() {
        let out =
            std::env::var("BINDINGS_OUT").expect("set BINDINGS_OUT to the reference output path");
        export_to(&out).expect("bindings export failed");
    }

    #[test]
    fn bindings_in_sync() {
        let tmp = std::env::temp_dir().join(format!("vox-bindings-{}.ts", std::process::id()));
        export_to(tmp.to_str().unwrap()).expect("bindings export failed");
        let fresh = std::fs::read_to_string(&tmp).expect("read exported bindings");
        let _ = std::fs::remove_file(&tmp);
        let committed =
            std::fs::read_to_string("../src/bindings.ts").expect("read committed bindings");

        let mut problems = Vec::new();
        let (ci, fi) = (invoke_names(&committed), invoke_names(&fresh));
        for n in ci.difference(&fi) {
            problems.push(format!("command only in committed bindings.ts: {n}"));
        }
        for n in fi.difference(&ci) {
            problems.push(format!("command only in fresh export: {n}"));
        }
        let (cf, ff) = (app_settings_fields(&committed), app_settings_fields(&fresh));
        for n in cf.difference(&ff) {
            problems.push(format!(
                "AppSettings field only in committed bindings.ts: {n}"
            ));
        }
        for n in ff.difference(&cf) {
            problems.push(format!("AppSettings field only in fresh export: {n}"));
        }
        assert!(
            problems.is_empty(),
            "src/bindings.ts out of sync (names only):\n{}",
            problems.join("\n")
        );
    }
}
