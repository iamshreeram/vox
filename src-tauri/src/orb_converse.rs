//! Orb activation: a click, Enter or Space starts or stops a Conversation (DESIGN.md Revision 3:
//! D2, A-C1, A-C1b, A-C1c, A-C2, A-C3). The orb asks the controller what the activation means
//! (`activation_decision`, read under one registry lock) and carries out that one decision. Every
//! refusal becomes a fixed `NoticeCode`: the orb window gets the typed `orb-refusal` event and the
//! bubble window shows the notice. No free text and no user content leave this module.

use crate::bubble_native;
use crate::bubble_notice::NoticeCode;
use crate::managers::converse::SessionId;
use crate::managers::converse_activation::ActivationDecision;
use crate::managers::converse_platform::{
    AMBIENT_REFUSAL, COMPOSER_OFF_REFUSAL, DICTATION_BUSY, ENROLLMENT_BUSY, TTS_OFF_REFUSAL,
};
use crate::managers::converse_runtime::{ConverseController, ALREADY_RUNNING, TEXT_ENDING};
use crate::orb_window::ORB_WINDOW_LABEL;
use serde::Serialize;
use specta::Type;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;
use tauri::{AppHandle, Emitter, Manager};

/// The typed refusal event, sent to the orb window only.
pub const ORB_REFUSAL_EVENT: &str = "orb-refusal";

/// After an accepted activation, further orb activations are ignored for this long.
pub const LOCKOUT_MS: u64 = 600;

/// Payload of `orb-refusal`: the fixed code and nothing else.
#[derive(Debug, Clone, Copy, Serialize, Type, PartialEq, Eq)]
pub struct OrbRefusalPayload {
    pub code: NoticeCode,
}

/// The time source of the lockout, in milliseconds on a monotonic clock.
pub trait Clock {
    fn now_ms(&self) -> u64;
}

/// The process monotonic clock: milliseconds since the first reading.
pub struct MonotonicClock;

impl Clock for MonotonicClock {
    fn now_ms(&self) -> u64 {
        static ORIGIN: OnceLock<Instant> = OnceLock::new();
        let origin = ORIGIN.get_or_init(Instant::now);
        u64::try_from(origin.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

/// The orb-local lockout. Only an accepted activation arms it; a click inside the window is
/// refused without touching the controller.
#[derive(Debug, Default)]
pub struct ClickLockout {
    last_accepted: Option<u64>,
}

impl ClickLockout {
    /// True when a click at `now` is accepted (and arms the lockout). An earlier reading than the
    /// last accepted one never opens the gate.
    pub fn accept(&mut self, now: u64) -> bool {
        if let Some(last) = self.last_accepted {
            if now.saturating_sub(last) < LOCKOUT_MS {
                return false;
            }
        }
        self.last_accepted = Some(now);
        true
    }
}

/// What one activation did. The name is the only thing logged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    LockedOut,
    Start,
    Stop,
    StopText,
    Ignored,
    Refused(NoticeCode),
}

impl Outcome {
    /// The content-free name of the outcome, for the log.
    pub fn name(self) -> &'static str {
        match self {
            Outcome::LockedOut => "lockout",
            Outcome::Start => "start",
            Outcome::Stop => "stop",
            Outcome::StopText => "stop_text",
            Outcome::Ignored => "ignore",
            Outcome::Refused(code) => match code {
                NoticeCode::DictationBusy => "refused:dictation_busy",
                NoticeCode::TtsOff => "refused:tts_off",
                NoticeCode::ComposerOff => "refused:composer_off",
                NoticeCode::Ambient => "refused:ambient",
                NoticeCode::ExperimentalOff => "refused:experimental_off",
                NoticeCode::TextEnding => "refused:text_ending",
                NoticeCode::Unknown => "refused:unknown",
            },
        }
    }
}

/// The refusals a start can produce. Messages are classified once, here; the notice mapping
/// below is exhaustive over this enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    DictationBusy,
    EnrollmentBusy,
    TtsOff,
    ComposerOff,
    Ambient,
    /// No controller message produces it yet; kept so the notice mapping stays exhaustive.
    #[cfg_attr(not(test), allow(dead_code))]
    ExperimentalOff,
    AlreadyRunning,
    TextEnding,
    /// A message this build does not know. It is never shown raw.
    Unmapped,
}

/// The known controller messages and their refusal. Anything else is `Unmapped`.
/// `ExperimentalOff` has no controller message today (the realtime preflight only validates
/// an enabled feature), so no row maps to it.
const KNOWN_REFUSALS: [(&str, StartError); 7] = [
    (DICTATION_BUSY, StartError::DictationBusy),
    (ENROLLMENT_BUSY, StartError::EnrollmentBusy),
    (TTS_OFF_REFUSAL, StartError::TtsOff),
    (COMPOSER_OFF_REFUSAL, StartError::ComposerOff),
    (AMBIENT_REFUSAL, StartError::Ambient),
    (ALREADY_RUNNING, StartError::AlreadyRunning),
    (TEXT_ENDING, StartError::TextEnding),
];

/// Classifies a controller start message.
pub fn classify_start_error(message: &str) -> StartError {
    KNOWN_REFUSALS
        .iter()
        .find(|(known, _)| *known == message)
        .map_or(StartError::Unmapped, |(_, error)| *error)
}

/// The notice for a refusal. `None` means the refusal is not shown (the session already runs).
pub fn notice_for(error: StartError) -> Option<NoticeCode> {
    match error {
        StartError::DictationBusy => Some(NoticeCode::DictationBusy),
        StartError::EnrollmentBusy => Some(NoticeCode::Unknown),
        StartError::TtsOff => Some(NoticeCode::TtsOff),
        StartError::ComposerOff => Some(NoticeCode::ComposerOff),
        StartError::Ambient => Some(NoticeCode::Ambient),
        StartError::ExperimentalOff => Some(NoticeCode::ExperimentalOff),
        StartError::AlreadyRunning => None,
        StartError::TextEnding => Some(NoticeCode::TextEnding),
        StartError::Unmapped => Some(NoticeCode::Unknown),
    }
}

/// What the activation logic needs from the controller and the window layer.
pub trait ActivationPort {
    /// The decision for this activation, read under one registry lock.
    fn decision(&self) -> ActivationDecision;
    /// The same start entry the hotkey uses.
    fn start(&self) -> Result<(), String>;
    /// Compare-and-stop of the session `id`.
    fn stop_if(&self, id: SessionId) -> bool;
    /// Publishes a refusal (orb event and bubble notice).
    fn notify(&self, code: NoticeCode);
}

/// Carries out one orb activation. The lockout is checked first; an accepted click arms it.
pub fn activate_with(
    port: &impl ActivationPort,
    lockout: &Mutex<ClickLockout>,
    clock: &impl Clock,
) -> Outcome {
    let accepted = {
        let mut gate = lockout
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        gate.accept(clock.now_ms())
    };
    if !accepted {
        return Outcome::LockedOut;
    }
    match port.decision() {
        ActivationDecision::Start => match port.start() {
            Ok(()) => Outcome::Start,
            Err(message) => match notice_for(classify_start_error(&message)) {
                Some(code) => {
                    port.notify(code);
                    Outcome::Refused(code)
                }
                None => Outcome::Ignored,
            },
        },
        ActivationDecision::RequestStopIf(id) => {
            if port.stop_if(id) {
                Outcome::Stop
            } else {
                Outcome::Ignored
            }
        }
        ActivationDecision::StopTextThenNotice(id) => {
            port.stop_if(id);
            port.notify(NoticeCode::TextEnding);
            Outcome::StopText
        }
        ActivationDecision::Ignore => Outcome::Ignored,
    }
}

/// The lockout shared by click, Enter and Space.
static LOCKOUT: Mutex<ClickLockout> = Mutex::new(ClickLockout {
    last_accepted: None,
});

/// The production port: the managed controller and the app handle.
struct AppPort {
    app: AppHandle,
    controller: Arc<ConverseController>,
}

impl ActivationPort for AppPort {
    fn decision(&self) -> ActivationDecision {
        self.controller.activation_decision()
    }
    fn start(&self) -> Result<(), String> {
        self.controller.start().map(|_| ())
    }
    fn stop_if(&self, id: SessionId) -> bool {
        self.controller.stop_if(id)
    }
    fn notify(&self, code: NoticeCode) {
        if self
            .app
            .emit_to(
                ORB_WINDOW_LABEL,
                ORB_REFUSAL_EVENT,
                OrbRefusalPayload { code },
            )
            .is_err()
        {
            log::debug!("orb refusal event not delivered");
        }
        bubble_native::show_notice(&self.app, code);
    }
}

/// Entry point for a click, Enter or Space on the orb.
pub fn activate(app: &AppHandle) {
    let Some(controller) = app.try_state::<Arc<ConverseController>>() else {
        log::warn!("orb activation: no conversation controller");
        return;
    };
    let port = AppPort {
        app: app.clone(),
        controller: controller.inner().clone(),
    };
    let outcome = activate_with(&port, &LOCKOUT, &MonotonicClock);
    log::info!("orb activation: {}", outcome.name());
}
