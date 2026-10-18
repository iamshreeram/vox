//! Engine router: chooses, per burst, between the natural voice and the system voice while
//! keeping the manager's contracts (queue-capable while Natural is selected, truthful stop,
//! router-wide handles). Spec = `natural_router_tests.rs`. STUB: bodies are `todo!()` until step
//! S5b. Signatures and doc contracts are binding. DESIGN.md D3 (Rev 4/5) is the intent.
#![allow(dead_code)]

use crate::managers::emotion::Emotion;
use crate::managers::natural_engine::NaturalHealth;
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use std::io;
use std::sync::Arc;

/// What the user selected, read on EVERY call (a settings change applies at once).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterChoice {
    /// `tts_engine == Natural`.
    pub natural: bool,
    /// `tts_natural_voice` (already validated); goes ONLY to the natural route.
    pub voice: String,
}

pub struct RouterConfig {
    pub choice: Arc<dyn Fn() -> RouterChoice + Send + Sync>,
    /// True when the model files are installed (`ModelCoordinator::state() == Ready`).
    pub model_ready: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Routes (all `SpeechBackend`s): `system` = today's system backend; `system_queue` = a
/// queue-capable system route used as the fallback while Natural is selected and `system` cannot
/// queue (`SerializedQueue`); `natural` = `QueueBackend<NaturalSynth>` (None off macOS).
///
/// Contract (DESIGN.md D3):
/// * `natural == None`, or `choice().natural == false`: 100% passthrough to `system` (spawn vs
///   spawn_styled vs enqueue_styled, voice/rate untouched, `supports_queue` = system's, returned
///   handles are system's own) — today's behaviour.
/// * Natural selected: `supports_queue()` is true. The FALLBACK route is `system` when
///   `system.supports_queue()` at call time, else `system_queue`.
/// * A burst START (`spawn`, `spawn_styled`, or `enqueue_styled` while no route is running) calls
///   `health.maybe_auto_retry(now)` then chooses the natural route iff health is Ready AND
///   `model_ready()` AND `is_mostly_latin(text)`, else the fallback. Natural gets
///   `Some(choice.voice)` (never the system voice) with the caller's rate/emotion; the fallback
///   gets the caller's original voice/rate/emotion. If the natural route refuses with any error
///   other than `WouldBlock`, the fallback is tried once in the same call; `WouldBlock` (a
///   previous stop could not be confirmed) is propagated unchanged.
/// * The pin is DERIVED: `enqueue_styled` goes to the route whose last handle `is_running()`;
///   if none is running it chooses afresh. Inside a running natural burst a text that is not
///   Latin script is dropped and counted (`dropped_non_latin`) and `Ok` is returned.
/// * Every returned handle is a `RoutedHandle` over the SAME router-wide state: `is_running()` is
///   true while ANY route's last handle is running; `queue_capable()` is true; `kill()` kills
///   EVERY route's last handle (never stopping early after one failure), clears the tracked
///   handles and returns true only if all confirmed.
/// * `stop_all()` does the same as `kill()` without needing a handle.
pub struct EngineBackend {
    _private: (),
}

impl EngineBackend {
    pub fn new(
        system: Box<dyn SpeechBackend>,
        system_queue: Box<dyn SpeechBackend>,
        natural: Option<Box<dyn SpeechBackend>>,
        health: Arc<NaturalHealth>,
        config: RouterConfig,
    ) -> Self {
        let _ = (system, system_queue, natural, health, config);
        todo!("S5b")
    }

    pub fn stop_all(&self) -> bool {
        todo!("S5b")
    }

    /// Later sentences of a natural burst that were dropped because they are not Latin script.
    pub fn dropped_non_latin(&self) -> usize {
        todo!("S5b")
    }
}

impl SpeechBackend for EngineBackend {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        let _ = (text, voice, rate_wpm);
        todo!("S5b")
    }
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        let _ = (text, voice, rate_wpm, emotion);
        todo!("S5b")
    }
    fn supports_queue(&self) -> bool {
        todo!("S5b")
    }
    fn enqueue_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        let _ = (text, voice, rate_wpm, emotion);
        todo!("S5b")
    }
}
