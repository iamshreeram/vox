//! Engine router: chooses, per burst, between the natural voice and the system voice while
//! keeping the manager's contracts (queue-capable while Natural is selected, truthful stop,
//! router-wide handles). Spec = `natural_router_tests.rs`. Signatures and doc contracts are
//! binding. DESIGN.md D3 (Rev 4/5) is the intent.
#![allow(dead_code)]

use crate::managers::emotion::Emotion;
use crate::managers::natural_chunk::is_mostly_latin;
use crate::managers::natural_engine::NaturalHealth;
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

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
    system: Box<dyn SpeechBackend>,
    system_queue: Box<dyn SpeechBackend>,
    natural: Option<Box<dyn SpeechBackend>>,
    health: Arc<NaturalHealth>,
    config: RouterConfig,
    shared: Arc<Shared>,
}

/// Slot indices into `Shared::tracked`.
const NATURAL: usize = 0;
const SYSTEM: usize = 1;
const SYSTEM_QUEUE: usize = 2;

type Slots = [Option<Box<dyn SpeechHandle>>; 3];

/// Router-wide state shared by the router and every handle it returned.
struct Shared {
    /// Last handle returned by each route (natural, system, system_queue).
    tracked: Mutex<Slots>,
    dropped: AtomicUsize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Slots> {
        self.tracked.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Kills every tracked handle (no early return); clears confirmed slots and keeps the
    /// unconfirmed ones so `is_running` keeps polling them. True only if all confirmed.
    fn kill_all(&self) -> bool {
        let mut slots = self.lock();
        let mut all_ok = true;
        for slot in slots.iter_mut() {
            if let Some(handle) = slot.as_mut() {
                if handle.kill() {
                    *slot = None;
                } else {
                    all_ok = false;
                }
            }
        }
        all_ok
    }

    fn any_running(&self) -> bool {
        let mut slots = self.lock();
        let mut running = false;
        for handle in slots.iter_mut().flatten() {
            if handle.is_running() {
                running = true;
            }
        }
        running
    }
}

/// Handle over the router-wide state (see `EngineBackend`).
struct RoutedHandle {
    shared: Arc<Shared>,
}

impl SpeechHandle for RoutedHandle {
    fn kill(&mut self) -> bool {
        self.shared.kill_all()
    }
    fn is_running(&mut self) -> bool {
        self.shared.any_running()
    }
    fn queue_capable(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Via {
    Spawn,
    Styled,
    Enqueue,
}

impl EngineBackend {
    pub fn new(
        system: Box<dyn SpeechBackend>,
        system_queue: Box<dyn SpeechBackend>,
        natural: Option<Box<dyn SpeechBackend>>,
        health: Arc<NaturalHealth>,
        config: RouterConfig,
    ) -> Self {
        Self {
            system,
            system_queue,
            natural,
            health,
            config,
            shared: Arc::new(Shared {
                tracked: Mutex::new([None, None, None]),
                dropped: AtomicUsize::new(0),
            }),
        }
    }

    pub fn stop_all(&self) -> bool {
        self.shared.kill_all()
    }

    /// Later sentences of a natural burst that were dropped because they are not Latin script.
    pub fn dropped_non_latin(&self) -> usize {
        self.shared.dropped.load(Ordering::SeqCst)
    }

    /// The natural route when it exists and Natural is selected (with the chosen voice).
    fn natural_selected(&self) -> Option<(&dyn SpeechBackend, String)> {
        let natural = self.natural.as_deref()?;
        let choice = (self.config.choice)();
        choice.natural.then_some((natural, choice.voice))
    }

    fn fallback_slot(&self) -> usize {
        if self.system.supports_queue() {
            SYSTEM
        } else {
            SYSTEM_QUEUE
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn call_slot(
        &self,
        slot: usize,
        natural: &dyn SpeechBackend,
        natural_voice: &str,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
        via: Via,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        match (slot, via) {
            (NATURAL, Via::Enqueue) => {
                natural.enqueue_styled(text, Some(natural_voice), rate_wpm, emotion)
            }
            (NATURAL, _) => natural.spawn_styled(text, Some(natural_voice), rate_wpm, emotion),
            (SYSTEM, Via::Spawn) => self.system.spawn(text, voice, rate_wpm),
            (SYSTEM, Via::Styled) => self.system.spawn_styled(text, voice, rate_wpm, emotion),
            (SYSTEM, Via::Enqueue) => self.system.enqueue_styled(text, voice, rate_wpm, emotion),
            (_, Via::Enqueue) => self
                .system_queue
                .enqueue_styled(text, voice, rate_wpm, emotion),
            (_, _) => self
                .system_queue
                .spawn_styled(text, voice, rate_wpm, emotion),
        }
    }

    /// Natural-selected path (see the type-level contract).
    #[allow(clippy::too_many_arguments)]
    fn start(
        &self,
        natural: &dyn SpeechBackend,
        natural_voice: &str,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
        via: Via,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        let mut slots = self.shared.lock();
        let mut pinned = None;
        if via == Via::Enqueue {
            // Poll every slot; the derived pin prefers natural, then index order.
            for (index, slot) in slots.iter_mut().enumerate() {
                if let Some(handle) = slot.as_mut() {
                    if handle.is_running() && pinned.is_none() {
                        pinned = Some(index);
                    }
                }
            }
        }
        let routed = || -> Box<dyn SpeechHandle> {
            Box::new(RoutedHandle {
                shared: self.shared.clone(),
            })
        };
        let chosen = match pinned {
            Some(NATURAL) if !is_mostly_latin(text) => {
                self.shared.dropped.fetch_add(1, Ordering::SeqCst);
                return Ok(routed());
            }
            Some(index) => index,
            None => {
                self.health.maybe_auto_retry(Instant::now());
                if self.health.is_ready() && (self.config.model_ready)() && is_mostly_latin(text) {
                    NATURAL
                } else {
                    self.fallback_slot()
                }
            }
        };
        let call = |slot: usize| {
            self.call_slot(
                slot,
                natural,
                natural_voice,
                text,
                voice,
                rate_wpm,
                emotion,
                via,
            )
        };
        let (slot, result) = match call(chosen) {
            Err(err) if chosen == NATURAL && err.kind() != io::ErrorKind::WouldBlock => {
                let fallback = self.fallback_slot();
                (fallback, call(fallback))
            }
            other => (chosen, other),
        };
        let handle = result?;
        slots[slot] = Some(handle);
        Ok(routed())
    }

    fn route(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
        via: Via,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        match self.natural_selected() {
            Some((natural, natural_voice)) => {
                self.start(natural, &natural_voice, text, voice, rate_wpm, emotion, via)
            }
            None => match via {
                Via::Spawn => self.system.spawn(text, voice, rate_wpm),
                Via::Styled => self.system.spawn_styled(text, voice, rate_wpm, emotion),
                Via::Enqueue => self.system.enqueue_styled(text, voice, rate_wpm, emotion),
            },
        }
    }
}

impl SpeechBackend for EngineBackend {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.route(text, voice, rate_wpm, Emotion::Neutral, Via::Spawn)
    }
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.route(text, voice, rate_wpm, emotion, Via::Styled)
    }
    fn supports_queue(&self) -> bool {
        match self.natural_selected() {
            Some(_) => true,
            None => self.system.supports_queue(),
        }
    }
    fn enqueue_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.route(text, voice, rate_wpm, emotion, Via::Enqueue)
    }
}
