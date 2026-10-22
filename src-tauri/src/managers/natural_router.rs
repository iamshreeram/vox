//! Engine router: chooses, per burst, between the natural voice, the Phonon voice and the system
//! voice while keeping the manager's contracts (queue-capable while a peer engine is selected,
//! truthful stop, router-wide handles). Spec = `natural_router_tests.rs` and
//! `natural_router_phonon_tests.rs`. Signatures and doc contracts are binding. DESIGN.md D3 and
//! P4 (Rev 4) are the intent.
#![allow(dead_code)]

use crate::managers::emotion::Emotion;
use crate::managers::natural_chunk::{is_mostly_latin, is_probably_english};
use crate::managers::natural_engine::NaturalHealth;
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// What the user selected, read on EVERY call (a settings change applies at once).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouterChoice {
    /// `tts_engine == Natural`.
    pub natural: bool,
    /// `tts_natural_voice` (already validated); goes ONLY to the natural route. The Phonon route
    /// ignores voices (one bundled English voice) and always receives `None`.
    pub voice: String,
    /// `tts_engine == Phonon`.
    pub phonon: bool,
}

pub struct RouterConfig {
    pub choice: Arc<dyn Fn() -> RouterChoice + Send + Sync>,
    /// True when the Kokoro model files are installed (`ModelCoordinator::state() == Ready`).
    pub model_ready: Arc<dyn Fn() -> bool + Send + Sync>,
    /// True when the Phonon model files are installed and its voice is usable.
    pub phonon_ready: Arc<dyn Fn() -> bool + Send + Sync>,
}

/// Routes (all `SpeechBackend`s): `system` = today's system backend; `system_queue` = a
/// queue-capable system route used as the fallback while a peer engine is selected and `system`
/// cannot queue (`SerializedQueue`); `natural` = `QueueBackend<NaturalSynth>` (None off macOS);
/// `phonon` = the Phonon route (a peer of `natural`, never an engine switch inside it; None when
/// no Phonon backend is built).
///
/// Contract (DESIGN.md D3, P4):
/// * No peer selected (`natural == None` or `choice().natural == false`, and no Phonon route or
///   `choice().phonon == false`), and no Phonon burst running: 100% passthrough to `system`
///   (spawn vs spawn_styled vs enqueue_styled, voice/rate untouched, `supports_queue` = system's,
///   returned handles are system's own) — today's behaviour.
/// * Natural or Phonon selected: `supports_queue()` is true. The FALLBACK route is `system` when
///   `system.supports_queue()` at call time, else `system_queue`.
/// * A running Phonon burst is pinned BEFORE the choice is read for routing: a later sentence
///   (`enqueue_styled`) stays on Phonon whatever the choice or readiness says, even when it is
///   not English (documented trade-off; never two routes at once).
/// * A burst START (`spawn`, `spawn_styled`, or `enqueue_styled` while no route is running)
///   chooses PHONON iff `choice().phonon`, a Phonon backend exists, Phonon health is Ready (after
///   `maybe_auto_retry`), `phonon_ready()` and `is_probably_english(text)`. Otherwise, if Natural
///   is selected, it chooses NATURAL iff health is Ready AND `model_ready()` AND
///   `is_mostly_latin(text)`, else the fallback. Otherwise (Phonon selected but not eligible)
///   a spawn goes to `system` unchanged and an enqueue goes to the fallback.
/// * Natural gets `Some(choice.voice)` (never the system voice) with the caller's rate/emotion;
///   Phonon gets `None` for the voice; the fallback gets the caller's original voice/rate/emotion.
///   If a peer route refuses with any error other than `WouldBlock`, the fallback is tried once
///   in the same call; `WouldBlock` (a previous stop could not be confirmed) is propagated
///   unchanged.
/// * The pin is DERIVED: `enqueue_styled` goes to the route whose last handle `is_running()`;
///   if none is running it chooses afresh. Inside a running NATURAL burst a text that is not
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
    phonon: Option<Box<dyn SpeechBackend>>,
    phonon_health: Arc<NaturalHealth>,
    config: RouterConfig,
    shared: Arc<Shared>,
}

/// Slot indices into `Shared::tracked`.
const NATURAL: usize = 0;
const PHONON: usize = 1;
const SYSTEM: usize = 2;
const SYSTEM_QUEUE: usize = 3;

type Slots = [Option<Box<dyn SpeechHandle>>; 4];

/// Router-wide state shared by the router and every handle it returned.
struct Shared {
    /// Last handle returned by each route (natural, phonon, system, system_queue).
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

    /// Whether the last handle of one route is running (polls only that route).
    fn running_in(&self, slot: usize) -> bool {
        let mut slots = self.lock();
        slots[slot]
            .as_mut()
            .is_some_and(|handle| handle.is_running())
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
        // No Phonon route: the Phonon health is never consulted, so its cooldown is irrelevant.
        Self::with_phonon(
            system,
            system_queue,
            natural,
            health,
            None,
            NaturalHealth::new(Duration::ZERO),
            config,
        )
    }

    /// Like `new`, plus the Phonon peer route (`phonon`, with its own health).
    #[allow(clippy::too_many_arguments)]
    pub fn with_phonon(
        system: Box<dyn SpeechBackend>,
        system_queue: Box<dyn SpeechBackend>,
        natural: Option<Box<dyn SpeechBackend>>,
        health: Arc<NaturalHealth>,
        phonon: Option<Box<dyn SpeechBackend>>,
        phonon_health: Arc<NaturalHealth>,
        config: RouterConfig,
    ) -> Self {
        Self {
            system,
            system_queue,
            natural,
            health,
            phonon,
            phonon_health,
            config,
            shared: Arc::new(Shared {
                tracked: Mutex::new([None, None, None, None]),
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

    /// The natural route exists and Natural is selected.
    fn natural_on(&self, choice: &RouterChoice) -> bool {
        choice.natural && self.natural.is_some()
    }

    /// The Phonon route exists and Phonon is selected.
    fn phonon_on(&self, choice: &RouterChoice) -> bool {
        choice.phonon && self.phonon.is_some()
    }

    /// The peer backend behind a peer slot (`NATURAL` or `PHONON`).
    fn peer(&self, slot: usize) -> Option<&dyn SpeechBackend> {
        match slot {
            NATURAL => self.natural.as_deref(),
            PHONON => self.phonon.as_deref(),
            _ => None,
        }
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
        natural_voice: &str,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
        via: Via,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        match slot {
            SYSTEM => match via {
                Via::Spawn => self.system.spawn(text, voice, rate_wpm),
                Via::Styled => self.system.spawn_styled(text, voice, rate_wpm, emotion),
                Via::Enqueue => self.system.enqueue_styled(text, voice, rate_wpm, emotion),
            },
            SYSTEM_QUEUE => match via {
                Via::Enqueue => self
                    .system_queue
                    .enqueue_styled(text, voice, rate_wpm, emotion),
                _ => self
                    .system_queue
                    .spawn_styled(text, voice, rate_wpm, emotion),
            },
            _ => {
                // Peer routes: Natural takes the chosen voice, Phonon ignores voices.
                let backend = self
                    .peer(slot)
                    .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
                let peer_voice = (slot == NATURAL).then_some(natural_voice);
                match via {
                    Via::Enqueue => backend.enqueue_styled(text, peer_voice, rate_wpm, emotion),
                    _ => backend.spawn_styled(text, peer_voice, rate_wpm, emotion),
                }
            }
        }
    }

    /// The route for the first sentence of a burst that has no running route yet.
    fn first_sentence_slot(&self, choice: &RouterChoice, text: &str, via: Via) -> usize {
        if self.phonon_on(choice) {
            self.phonon_health.maybe_auto_retry(Instant::now());
            if self.phonon_health.is_ready()
                && (self.config.phonon_ready)()
                && is_probably_english(text)
            {
                return PHONON;
            }
        }
        if self.natural_on(choice) {
            self.health.maybe_auto_retry(Instant::now());
            if self.health.is_ready() && (self.config.model_ready)() && is_mostly_latin(text) {
                return NATURAL;
            }
            return self.fallback_slot();
        }
        // Phonon selected but not eligible: the system voice, unchanged for spawns.
        match via {
            Via::Enqueue => self.fallback_slot(),
            _ => SYSTEM,
        }
    }

    /// Peer-selected path (see the type-level contract).
    #[allow(clippy::too_many_arguments)]
    fn start(
        &self,
        choice: &RouterChoice,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
        via: Via,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        let mut slots = self.shared.lock();
        let mut pinned = None;
        if via == Via::Enqueue {
            // Poll every slot; the derived pin prefers natural, then phonon, then index order.
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
            // A running Phonon burst keeps every sentence, even a non-English one (documented
            // trade-off: the first sentence's decision stands for the whole burst).
            Some(index) => index,
            None => self.first_sentence_slot(choice, text, via),
        };
        let call =
            |slot: usize| self.call_slot(slot, &choice.voice, text, voice, rate_wpm, emotion, via);
        let (slot, result) = match call(chosen) {
            Err(err)
                if (chosen == NATURAL || chosen == PHONON)
                    && err.kind() != io::ErrorKind::WouldBlock =>
            {
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
        let choice = (self.config.choice)();
        // The Phonon pin is checked BEFORE the choice: a running Phonon burst is never moved.
        let phonon_burst = via == Via::Enqueue && self.shared.running_in(PHONON);
        if phonon_burst || self.natural_on(&choice) || self.phonon_on(&choice) {
            return self.start(&choice, text, voice, rate_wpm, emotion, via);
        }
        match via {
            Via::Spawn => self.system.spawn(text, voice, rate_wpm),
            Via::Styled => self.system.spawn_styled(text, voice, rate_wpm, emotion),
            Via::Enqueue => self.system.enqueue_styled(text, voice, rate_wpm, emotion),
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
        let choice = (self.config.choice)();
        if self.natural_on(&choice) || self.phonon_on(&choice) {
            true
        } else {
            self.system.supports_queue()
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
