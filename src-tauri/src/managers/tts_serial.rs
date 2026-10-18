//! Queue-capable adapter over a non-queue `SpeechBackend` (the system `say` backend): lets the
//! natural-voice router fall back to the system voice WITHOUT losing the "append to a live
//! queue" behaviour that `TtsManager` relies on while the natural engine is selected.
//!
//! Spec = `tts_serial_tests.rs`. Signatures and doc contracts are binding. DESIGN.md D3
//! describes the intent.
// Not referenced from production code until the natural-voice router is wired in.
#![allow(dead_code)]

use crate::managers::emotion::Emotion;
use crate::managers::tts::{SpeechBackend, SpeechHandle};
use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Items allowed to WAIT (not counting the one being spoken); further appends are refused.
pub const SERIAL_MAX_PENDING: usize = 16;

/// Poll interval of the worker while a child is speaking (contract: <= 10 ms).
const POLL_INTERVAL: Duration = Duration::from_millis(5);
/// Upper bound for joining the worker on drop (after that it is detached).
const JOIN_BOUND: Duration = Duration::from_millis(200);

/// One thread runs one inner utterance at a time from a FIFO.
/// * `supports_queue()` is true; `spawn`, `spawn_styled` and `enqueue_styled` ALL append to the
///   FIFO (the manager has already killed previous speech before a `spawn`) and return a handle
///   that is `queue_capable()` and shared (every handle controls the whole queue).
/// * A full FIFO (`SERIAL_MAX_PENDING` waiting) refuses with `ErrorKind::WouldBlock`.
/// * The worker pops an item, starts it with `inner.spawn_styled(text, voice, rate, emotion)`
///   (an `Err` skips that item and continues), polls the inner handle's `is_running()` every
///   <= 10 ms, and starts the next item only after the child has exited.
/// * `is_running()` is true while the FIFO is non-empty OR an item is being started/spoken.
/// * `kill()` clears the FIFO, kills the current child and returns true if there was no child
///   or the inner `kill()` confirmed; false if the inner kill failed (the child is then still
///   running and `is_running()` stays true). An item that was popped but is still being started
///   when `kill()` runs must be killed as soon as it has started (generation check) and never
///   keep speaking.
/// * Dropping the adapter stops the worker and kills a running child.
pub struct SerializedQueue {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    /// Shared with the worker (which does all the calls); kept here as the owner's reference.
    inner: Arc<dyn SpeechBackend>,
}

struct Item {
    text: String,
    voice: Option<String>,
    rate: Option<u32>,
    emotion: Emotion,
}

struct St {
    fifo: VecDeque<Item>,
    generation: u64,
    /// An item is being started or spoken.
    busy: bool,
    current: Option<Box<dyn SpeechHandle>>,
    shutdown: bool,
}

struct Shared {
    st: Mutex<St>,
    cv: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, St> {
        self.st.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Clears the FIFO, invalidates any item being started and kills the current child.
    /// True when there was no child or its kill was confirmed.
    fn kill_all(&self) -> bool {
        let mut st = self.lock();
        st.generation = st.generation.wrapping_add(1);
        st.fifo.clear();
        let ok = match st.current.as_mut() {
            Some(child) => {
                let ok = child.kill();
                if ok {
                    st.current = None;
                    st.busy = false;
                }
                ok
            }
            None => true,
        };
        drop(st);
        self.cv.notify_all();
        ok
    }
}

/// Appends one item; refuses with `WouldBlock` when `SERIAL_MAX_PENDING` items already wait.
fn append(shared: &Arc<Shared>, item: Item) -> io::Result<Box<dyn SpeechHandle>> {
    let mut st = shared.lock();
    if st.fifo.len() >= SERIAL_MAX_PENDING {
        return Err(io::Error::new(io::ErrorKind::WouldBlock, "queue full"));
    }
    st.fifo.push_back(item);
    drop(st);
    shared.cv.notify_all();
    Ok(Box::new(SerialHandle {
        shared: Arc::clone(shared),
    }))
}

/// Handle onto the whole shared queue.
struct SerialHandle {
    shared: Arc<Shared>,
}

impl SpeechHandle for SerialHandle {
    fn kill(&mut self) -> bool {
        self.shared.kill_all()
    }
    fn is_running(&mut self) -> bool {
        let st = self.shared.lock();
        !st.fifo.is_empty() || st.busy
    }
    fn queue_capable(&self) -> bool {
        true
    }
}

fn worker_loop(shared: Arc<Shared>, inner: Arc<dyn SpeechBackend>) {
    loop {
        // Wait for an item (or shutdown) and claim it.
        let (item, gen) = {
            let mut st = shared.lock();
            let item = loop {
                if st.shutdown {
                    return;
                }
                if let Some(item) = st.fifo.pop_front() {
                    break item;
                }
                st = shared.cv.wait(st).unwrap_or_else(|e| e.into_inner());
            };
            st.busy = true;
            (item, st.generation)
        };

        // Start it WITHOUT holding the lock (process start may be slow).
        let started =
            inner.spawn_styled(&item.text, item.voice.as_deref(), item.rate, item.emotion);
        let child = match started {
            Ok(child) => child,
            Err(e) => {
                log::warn!("serial tts: could not start an utterance: {}", e.kind());
                shared.lock().busy = false;
                continue;
            }
        };

        {
            let mut st = shared.lock();
            if st.generation != gen || st.shutdown {
                // Killed (or shutting down) while starting: never let it keep speaking.
                let mut child = child;
                if !child.kill() {
                    log::warn!("serial tts: could not confirm stop of a stale utterance");
                }
                st.busy = false;
                continue;
            }
            st.current = Some(child);
        }

        // Poll until the child exits or `kill()` removed it.
        loop {
            {
                let mut st = shared.lock();
                if st.shutdown {
                    if let Some(mut child) = st.current.take() {
                        let _ = child.kill();
                    }
                    st.busy = false;
                    return;
                }
                match st.current.as_mut() {
                    // `kill()` confirmed the stop (and already reset `busy`).
                    None => break,
                    Some(child) => {
                        if !child.is_running() {
                            st.current = None;
                            st.busy = false;
                            break;
                        }
                    }
                }
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

impl SerializedQueue {
    pub fn new(inner: Box<dyn SpeechBackend>) -> Self {
        let inner: Arc<dyn SpeechBackend> = Arc::from(inner);
        let shared = Arc::new(Shared {
            st: Mutex::new(St {
                fifo: VecDeque::new(),
                generation: 0,
                busy: false,
                current: None,
                shutdown: false,
            }),
            cv: Condvar::new(),
        });
        let worker = {
            let shared = Arc::clone(&shared);
            let inner = Arc::clone(&inner);
            std::thread::Builder::new()
                .name("tts-serial".into())
                .spawn(move || worker_loop(shared, inner))
        };
        let worker = match worker {
            Ok(handle) => Some(handle),
            Err(e) => {
                log::error!("serial tts: could not start the worker thread: {e}");
                None
            }
        };
        Self {
            shared,
            worker,
            inner,
        }
    }

    fn push(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        if self.worker.is_none() {
            return Err(io::Error::other("serial tts worker is not running"));
        }
        append(
            &self.shared,
            Item {
                text: text.to_string(),
                voice: voice.map(str::to_string),
                rate: rate_wpm,
                emotion,
            },
        )
    }
}

impl Drop for SerializedQueue {
    fn drop(&mut self) {
        {
            let mut st = self.shared.lock();
            st.shutdown = true;
            st.generation = st.generation.wrapping_add(1);
            st.fifo.clear();
            if let Some(mut child) = st.current.take() {
                if !child.kill() {
                    log::warn!("serial tts: could not confirm stop of the running utterance");
                }
            }
        }
        self.shared.cv.notify_all();
        if let Some(worker) = self.worker.take() {
            let deadline = Instant::now() + JOIN_BOUND;
            while !worker.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
            // Otherwise detach: the worker exits on its own once its inner start returns.
        }
    }
}

impl SpeechBackend for SerializedQueue {
    fn spawn(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.push(text, voice, rate_wpm, Emotion::Neutral)
    }
    fn spawn_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.push(text, voice, rate_wpm, emotion)
    }
    fn supports_queue(&self) -> bool {
        true
    }
    fn enqueue_styled(
        &self,
        text: &str,
        voice: Option<&str>,
        rate_wpm: Option<u32>,
        emotion: Emotion,
    ) -> io::Result<Box<dyn SpeechHandle>> {
        self.push(text, voice, rate_wpm, emotion)
    }
}
