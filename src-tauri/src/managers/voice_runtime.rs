//! Production wiring of the optional voice features (design S12).
//!
//! One `VoiceRuntime` is built in `lib.rs`: the single inference service (`OrtLoader`), the
//! owner-only voiceprint store, and the enrollment controller over the shared `OwnershipGate`.
//! Nothing here downloads anything. Models reach the service only through `apply_kind`, which the
//! aux-model commands call after a download, a delete, and at startup for ready models.
//!
//! Logs in this file carry numbers and fixed messages only: never audio, embeddings or text.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::managers::audio::AudioRecordingManager;
use crate::managers::aux_model::{aux_spec, AuxKind, AuxState};
use crate::managers::converse_capture::CaptureOptions;
use crate::managers::ort_sessions::OrtLoader;
use crate::managers::ownership::OwnershipGate;
use crate::managers::smart_turn::TurnDetector;
use crate::managers::speaker_enroll::{EnrollConfig, EnrollmentController};
use crate::managers::speaker_enroll_adapters::{
    ManagerEnrollRecorder, ServiceEnrollEmbedder, SystemEnrollClock,
};
use crate::managers::speaker_id::{SpeakerGate, VoiceprintStore};
use crate::managers::voice_adapters::{ServiceSpeakerGate, ServiceTurnDetector};
use crate::managers::voice_models::{ModelKind, VoiceModelService};
use crate::settings::{AppSettings, SpeakerSensitivity};

/// Directory under the app data dir that holds the voiceprint.
const VOICEPRINT_DIR: &str = "voiceprint";
/// How often the idle-expiry ticker asks the enrollment controller to expire a session.
const EXPIRY_TICK: Duration = Duration::from_secs(5);
/// "No remembered enrollment session". Controller ids start at 1.
const NO_SESSION: u64 = 0;

/// Reads the live settings on every call.
pub type SettingsGetter = Arc<dyn Fn() -> AppSettings + Send + Sync>;

/// Reads the live speaker sensitivity on every call.
pub type SensitivityReader = Arc<dyn Fn() -> SpeakerSensitivity + Send + Sync>;

/// The single inference service, the voiceprint store and the enrollment controller.
pub struct VoiceRuntime {
    pub service: Arc<VoiceModelService>,
    pub store: Arc<VoiceprintStore>,
    pub enroll: Arc<EnrollmentController>,
    settings: SettingsGetter,
    /// Id of the enrollment session begun through the command layer, or `NO_SESSION`.
    /// Lets a Speaker-model delete cancel the session without a session id from the UI.
    session: AtomicU64,
}

impl VoiceRuntime {
    pub fn new(
        service: Arc<VoiceModelService>,
        store: Arc<VoiceprintStore>,
        enroll: Arc<EnrollmentController>,
        settings: SettingsGetter,
    ) -> Self {
        Self {
            service,
            store,
            enroll,
            settings,
            session: AtomicU64::new(NO_SESSION),
        }
    }

    /// The app's runtime: `OrtLoader` service, store under `<app_data>/voiceprint/`, and an
    /// enrollment controller over the shared gate and recording manager.
    pub fn production(
        app_data: &Path,
        gate: Arc<OwnershipGate>,
        recordings: Arc<AudioRecordingManager>,
        settings: SettingsGetter,
    ) -> Self {
        let dir = app_data.join(VOICEPRINT_DIR);
        if ensure_private_dir(&dir).is_err() {
            log::warn!("optional voice features: voiceprint directory unavailable");
        }
        let service = Arc::new(VoiceModelService::new(Arc::new(OrtLoader)));
        let store = Arc::new(VoiceprintStore::new(dir));
        let enroll = Arc::new(EnrollmentController::new(
            gate,
            Arc::new(ManagerEnrollRecorder::new(recordings)),
            Arc::new(ServiceEnrollEmbedder::new(Arc::clone(&service))),
            Arc::clone(&store),
            Arc::new(SystemEnrollClock::new()),
            EnrollConfig::default(),
        ));
        Self::new(service, store, enroll, settings)
    }

    /// Remembers the enrollment session the command layer just began.
    pub fn remember_session(&self, id: u64) {
        self.session.store(id, Ordering::SeqCst);
    }

    /// Forgets the remembered session when it is `id` (after cancel or finish).
    pub fn forget_session(&self, id: u64) {
        let _ = self
            .session
            .compare_exchange(id, NO_SESSION, Ordering::SeqCst, Ordering::SeqCst);
    }

    /// Cancels the remembered enrollment session, if any. A stale id is ignored.
    pub fn cancel_enrollment(&self) {
        let id = self.session.swap(NO_SESSION, Ordering::SeqCst);
        if id != NO_SESSION {
            let _ = self.enroll.cancel(id);
        }
    }
}

/// The service model that serves an aux kind.
pub fn kind_to_model(kind: AuxKind) -> ModelKind {
    match kind {
        AuxKind::SmartTurn => ModelKind::SmartTurn,
        AuxKind::SpeakerResnet34 => ModelKind::Speaker,
    }
}

/// Makes the service match the aux kind's state. `Ready` with a verified path installs it.
/// Anything else uninstalls it, and removing the Speaker model also cancels an enrollment.
/// Installing a kind that is already installed is harmless.
pub fn apply_kind(
    runtime: &VoiceRuntime,
    kind: AuxKind,
    state: AuxState,
    ready_path: Option<PathBuf>,
) {
    let model = kind_to_model(kind);
    match (state, ready_path) {
        (AuxState::Ready, Some(path)) => runtime.service.install(model, path),
        _ => {
            runtime.service.uninstall(model);
            if model == ModelKind::Speaker {
                runtime.cancel_enrollment();
            }
        }
    }
}

/// Speaker sensitivity read from the live settings on every call, so a change applies to the
/// next utterance without rebuilding the gate.
pub fn sensitivity_reader(runtime: &VoiceRuntime) -> SensitivityReader {
    let settings = Arc::clone(&runtime.settings);
    Arc::new(move || settings().speaker_filter_sensitivity)
}

/// The optional parts of one capture worker. Each is `Some` only when its feature is on, its
/// model is installed and, for the speaker filter, a voiceprint is enrolled. With every feature
/// off both fields are `None`: no tap, ring, helper thread or inference is used.
pub fn capture_options(settings: &AppSettings, runtime: &VoiceRuntime) -> CaptureOptions {
    let turn_detector: Option<Arc<dyn TurnDetector>> =
        if settings.converse_smart_turn && runtime.service.is_installed(ModelKind::SmartTurn) {
            Some(Arc::new(ServiceTurnDetector::new(Arc::clone(
                &runtime.service,
            ))))
        } else {
            None
        };
    let speaker_gate: Option<Arc<dyn SpeakerGate>> = if settings.speaker_filter_enabled
        && runtime.service.is_installed(ModelKind::Speaker)
        && runtime.store.is_enrolled()
    {
        Some(Arc::new(ServiceSpeakerGate::new(
            Arc::clone(&runtime.service),
            Arc::clone(&runtime.store),
            sensitivity_reader(runtime),
            aux_spec(AuxKind::SpeakerResnet34).sha256,
        )))
    } else {
        None
    };
    CaptureOptions {
        turn_detector,
        speaker_gate,
    }
}

/// Starts one thread that calls `expire_if_idle` every `EXPIRY_TICK`. It holds only a weak
/// reference, so it stops when the runtime is dropped and otherwise lives until the process exits.
pub fn spawn_expiry_ticker(runtime: &Arc<VoiceRuntime>) {
    let weak = Arc::downgrade(runtime);
    let spawned = std::thread::Builder::new()
        .name("speaker-enroll-expiry".to_owned())
        .spawn(move || loop {
            std::thread::sleep(EXPIRY_TICK);
            let Some(runtime) = weak.upgrade() else {
                break;
            };
            runtime.enroll.expire_if_idle();
        });
    if spawned.is_err() {
        log::warn!("optional voice features: expiry thread unavailable");
    }
}

/// Creates `dir` (and parents) with owner-only mode 0700. Refuses a symlink.
pub fn ensure_private_dir(dir: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(dir).is_ok_and(|meta| meta.file_type().is_symlink()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "voiceprint directory is a symlink",
        ));
    }
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "voice_runtime_tests.rs"]
mod tests;
