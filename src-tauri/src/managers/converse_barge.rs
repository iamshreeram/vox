//! Stage B1 (DESIGN.md Rev 8, section 12.2.2 / 12.2.4 / 12.7.C): barge-in policy
//! and the seam types shared with the supervisor (B1b) and the monitor (B3).
//!
//! `BargePolicy` is pure: it takes one frame at a time with an explicit clock
//! (`now_ms` = time at the END of the frame) and returns the events to act on.
//! Nothing here touches audio devices, TTS or the network.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use tokio::sync::watch;

use crate::managers::converse::BoxFuture;

/// Voiced time that must elapse after onset before the reply is cancelled.
// Used by B1b/B3 (not yet wired into the conversation loop).
#[allow(dead_code)]
pub const SUSTAIN_MS: u64 = 600;
/// Longest silent gap that is bridged while ducked; a longer one is a backchannel.
// Used by B1b/B3 (not yet wired into the conversation loop).
#[allow(dead_code)]
pub const GAP_MS: u64 = 250;
/// Quiet period after a restore during which voiced audio is ignored.
// Used by B1b/B3 (not yet wired into the conversation loop).
#[allow(dead_code)]
pub const COOLDOWN_MS: u64 = 300;
/// Lead-in kept before the onset so the first word is not clipped.
// Used by B1b/B3 (not yet wired into the conversation loop).
#[allow(dead_code)]
pub const PREROLL_MS: u64 = 300;

/// Where the policy is in the barge-in cycle.
// Used by B1b/B3 (not yet wired into the conversation loop).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyState {
    Idle,
    Armed,
    Ducked,
    Capturing,
    Cooldown,
    Finished,
}

/// Side effects the caller must perform, in the order they are returned.
// Used by B1b/B3 (not yet wired into the conversation loop).
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum PolicyEvent {
    /// Speech started while the assistant speaks: lower the reply volume.
    Duck,
    /// Backchannel or playback ended before a cancel: restore the volume.
    Restore,
    /// Sustained speech: stop the reply now.
    CancelReply,
    /// The utterance is complete; the samples run from the lead-in to the endpoint.
    Done(Vec<f32>),
}

// Used by B1b/B3 (not yet wired into the conversation loop).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct PolicyConfig {
    pub endpoint_silence_ms: u64,
    pub max_utterance_ms: u64,
    pub frame_ms: u64,
}

// Used by B1b/B3 (not yet wired into the conversation loop).
#[allow(dead_code)]
pub struct BargePolicy {
    cfg: PolicyConfig,
    state: PolicyState,
    ring: VecDeque<Vec<f32>>,
    ring_cap: usize,
    buffer: Vec<Vec<f32>>,
    t0: u64,
    last_voiced: u64,
    cooldown_until: u64,
}

#[allow(dead_code)]
impl BargePolicy {
    pub fn new(cfg: PolicyConfig) -> Self {
        let ring_cap = (PREROLL_MS / cfg.frame_ms.max(1)) as usize;
        Self {
            cfg,
            state: PolicyState::Idle,
            ring: VecDeque::with_capacity(ring_cap),
            ring_cap,
            buffer: Vec::new(),
            t0: 0,
            last_voiced: 0,
            cooldown_until: 0,
        }
    }

    pub fn state(&self) -> PolicyState {
        self.state
    }

    /// Feed one frame. `now_ms` is the time at the end of the frame.
    pub fn push_frame(
        &mut self,
        now_ms: u64,
        voiced: bool,
        speaking: bool,
        samples: &[f32],
    ) -> Vec<PolicyEvent> {
        let mut out = Vec::new();
        if self.state == PolicyState::Finished {
            return out;
        }
        if self.state == PolicyState::Idle {
            if !speaking {
                return out;
            }
            self.state = PolicyState::Armed;
        }
        if self.state == PolicyState::Cooldown {
            if !speaking {
                self.go_idle();
                return out;
            }
            if now_ms < self.cooldown_until {
                self.push_ring(samples);
                return out;
            }
            self.state = PolicyState::Armed;
        }
        match self.state {
            PolicyState::Armed => {
                if !speaking {
                    self.go_idle();
                } else if voiced {
                    self.onset(now_ms, samples);
                    out.push(PolicyEvent::Duck);
                } else {
                    self.push_ring(samples);
                }
            }
            PolicyState::Ducked => self.ducked_frame(now_ms, voiced, speaking, samples, &mut out),
            PolicyState::Capturing => {
                self.buffer.push(samples.to_vec());
                if voiced {
                    self.last_voiced = now_ms;
                }
                self.finish_if_due(now_ms, &mut out);
            }
            PolicyState::Idle | PolicyState::Cooldown | PolicyState::Finished => {}
        }
        out
    }

    /// A frame was lost (overflow or stalled stream). It counts as unvoiced
    /// and breaks continuity; it never completes a sustain window.
    pub fn push_missing(&mut self, now_ms: u64) -> Vec<PolicyEvent> {
        let mut out = Vec::new();
        match self.state {
            PolicyState::Armed => self.ring.clear(),
            PolicyState::Ducked => {
                self.buffer.clear();
                self.ring.clear();
                self.start_cooldown(now_ms);
                out.push(PolicyEvent::Restore);
            }
            PolicyState::Capturing => self.finish_if_due(now_ms, &mut out),
            PolicyState::Idle | PolicyState::Cooldown | PolicyState::Finished => {}
        }
        out
    }

    fn ducked_frame(
        &mut self,
        now_ms: u64,
        voiced: bool,
        speaking: bool,
        samples: &[f32],
        out: &mut Vec<PolicyEvent>,
    ) {
        if !speaking {
            self.buffer.clear();
            self.ring.clear();
            self.state = PolicyState::Idle;
            out.push(PolicyEvent::Restore);
            return;
        }
        self.buffer.push(samples.to_vec());
        if voiced {
            self.last_voiced = now_ms;
        }
        if now_ms.saturating_sub(self.last_voiced) > GAP_MS {
            self.buffer.clear();
            self.ring.clear();
            self.start_cooldown(now_ms);
            out.push(PolicyEvent::Restore);
        } else if now_ms.saturating_sub(self.t0) >= SUSTAIN_MS {
            self.state = PolicyState::Capturing;
            out.push(PolicyEvent::CancelReply);
        }
    }

    fn onset(&mut self, now_ms: u64, samples: &[f32]) {
        self.t0 = now_ms;
        self.last_voiced = now_ms;
        self.buffer = self.ring.drain(..).collect();
        self.buffer.push(samples.to_vec());
        self.state = PolicyState::Ducked;
    }

    fn push_ring(&mut self, samples: &[f32]) {
        if self.ring.len() == self.ring_cap {
            self.ring.pop_front();
        }
        self.ring.push_back(samples.to_vec());
    }

    fn go_idle(&mut self) {
        self.ring.clear();
        self.buffer.clear();
        self.state = PolicyState::Idle;
    }

    fn start_cooldown(&mut self, now_ms: u64) {
        self.cooldown_until = now_ms + COOLDOWN_MS;
        self.state = PolicyState::Cooldown;
    }

    fn finish_if_due(&mut self, now_ms: u64, out: &mut Vec<PolicyEvent>) {
        let silent_for = now_ms.saturating_sub(self.last_voiced);
        let elapsed = now_ms.saturating_sub(self.t0);
        if silent_for >= self.cfg.endpoint_silence_ms || elapsed >= self.cfg.max_utterance_ms {
            let samples: Vec<f32> = self.buffer.drain(..).flatten().collect();
            self.state = PolicyState::Finished;
            out.push(PolicyEvent::Done(samples));
        }
    }
}

/// Result of one barge-in capture, handed to the supervisor.
#[derive(Debug, Clone, PartialEq)]
pub enum BargeCapture {
    Utterance(String),
    NoSpeech,
}

/// Linearization point between reply admission and barge-in cancel: once
/// `close()` returns, no admission is running and none can start.
pub struct ReplyGate {
    closed: Mutex<bool>,
}

impl ReplyGate {
    pub fn new() -> Arc<ReplyGate> {
        Arc::new(ReplyGate {
            closed: Mutex::new(false),
        })
    }

    /// Blocks until any in-progress admission has finished.
    pub fn close(&self) {
        let mut closed = match self.closed.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *closed = true;
    }

    /// A poisoned gate counts as closed.
    // Used by B3b (admission checks in the conversation loop); tests use it today.
    #[allow(dead_code)]
    pub fn is_closed(&self) -> bool {
        match self.closed.lock() {
            Ok(guard) => *guard,
            Err(_) => true,
        }
    }

    /// Runs `f` while holding the gate if it is open; `None` (and `f` not run)
    /// once the gate is closed or poisoned.
    pub fn with_admission<R>(&self, f: impl FnOnce() -> R) -> Option<R> {
        let guard = match self.closed.lock() {
            Ok(guard) => guard,
            Err(_) => return None,
        };
        if *guard {
            return None;
        }
        let result = f();
        drop(guard);
        Some(result)
    }
}

/// Arms one barge-in watch per reply turn. `None` means unavailable (fail-soft).
pub trait BargeIn: Send + Sync {
    fn arm(&self) -> Option<Box<dyn BargeWatch>>;
}

/// One reply turn's barge-in watch. Dropping it disarms the monitor and restores the volume.
///
/// Contract: an implementation MUST make `gate().is_closed()` and `cancel_latched()` observable
/// (SeqCst) BEFORE `reply_token()` fires, because the loop reads the latch after seeing the token
/// (converse.rs run_turns).
pub trait BargeWatch: Send {
    /// Fires (true) when the reply must stop. Closed without a value means never fires.
    fn reply_token(&self) -> watch::Receiver<bool>;
    /// True once a barge-in cancelled this reply.
    fn cancel_latched(&self) -> bool;
    /// The admission gate of this reply; closed before the token fires.
    fn gate(&self) -> Arc<ReplyGate>;
    /// Resolves once the barge-in clip is classified (`NoSpeech` if none is coming).
    fn capture(&mut self) -> BoxFuture<'_, BargeCapture>;
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn admission_runs_while_open() {
        let gate = ReplyGate::new();
        assert_eq!(gate.with_admission(|| 7), Some(7));
        assert!(!gate.is_closed());
    }

    #[test]
    fn closed_gate_refuses_admission_without_calling_f() {
        let gate = ReplyGate::new();
        gate.close();
        let mut called = false;
        assert_eq!(
            gate.with_admission(|| {
                called = true;
                1
            }),
            None
        );
        assert!(!called);
        assert!(gate.is_closed());
    }

    #[test]
    fn close_waits_for_an_in_progress_admission() {
        let gate = ReplyGate::new();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let admission_done = Arc::new(AtomicBool::new(false));

        let admitter = {
            let gate = gate.clone();
            let admission_done = admission_done.clone();
            std::thread::spawn(move || {
                gate.with_admission(|| {
                    entered_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    admission_done.store(true, Ordering::SeqCst);
                })
            })
        };
        entered_rx.recv().unwrap();

        let closed_done = Arc::new(AtomicBool::new(false));
        let saw_admission_done = Arc::new(AtomicBool::new(false));
        let closer = {
            let gate = gate.clone();
            let closed_done = closed_done.clone();
            let admission_done = admission_done.clone();
            let saw_admission_done = saw_admission_done.clone();
            std::thread::spawn(move || {
                gate.close();
                saw_admission_done.store(admission_done.load(Ordering::SeqCst), Ordering::SeqCst);
                closed_done.store(true, Ordering::SeqCst);
            })
        };

        std::thread::sleep(Duration::from_millis(100));
        assert!(
            !closed_done.load(Ordering::SeqCst),
            "close() returned while an admission was running"
        );
        release_tx.send(()).unwrap();
        assert_eq!(admitter.join().unwrap(), Some(()));
        closer.join().unwrap();
        assert!(closed_done.load(Ordering::SeqCst));
        assert!(saw_admission_done.load(Ordering::SeqCst));
    }

    #[test]
    fn poisoned_gate_behaves_as_closed() {
        let gate = ReplyGate::new();
        let poisoner = gate.clone();
        let _ = std::thread::spawn(move || {
            poisoner.with_admission(|| panic!("poison the gate"));
        })
        .join();
        let mut called = false;
        assert_eq!(
            gate.with_admission(|| {
                called = true;
            }),
            None
        );
        assert!(!called);
        assert!(gate.is_closed());
    }

    #[test]
    fn capture_results_compare_by_value() {
        assert_eq!(
            BargeCapture::Utterance("stop".into()),
            BargeCapture::Utterance("stop".into())
        );
        assert_ne!(
            BargeCapture::NoSpeech,
            BargeCapture::Utterance(String::new())
        );
    }
}
