//! Pure session machine for the wake-appear orb (DESIGN.md A7-1, A8-1, A9-2).
//!
//! No I/O and no clock reads: every input carries a monotonic `now` in milliseconds. The machine
//! never writes settings. Callers ask `desired_orb_visible` for the total desired visibility and
//! compare it with the real window through `reconcile`, which drops anything queued under an
//! older generation.
#![cfg_attr(not(test), allow(dead_code))]

/// How long the orb stays after a session ends, counted from the End (or the last late entry).
pub const LINGER_MS: u64 = 8_000;
/// Hard cap on how far late entries can extend a lingering session past its End.
pub const MAX_POST_END_EXTENSION_MS: u64 = 30_000;
/// A session without any entry for this long is treated as ended (typed sessions may never end).
pub const MAX_ACTIVE_MS: u64 = 900_000;

/// Where the session is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// No session.
    Idle,
    /// A session is running.
    Active,
    /// The session ended; the orb lingers until the deadline.
    Lingering,
}

/// A change the executor must make to the real orb window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrbAction {
    /// Show the orb.
    Show,
    /// Hide the orb.
    Hide,
}

/// The session state. One instance is owned by the driver behind a lock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMachine {
    phase: Phase,
    sid: u64,
    last_activity: u64,
    ended_at: u64,
    deadline: u64,
    generation: u64,
    transient_shown: bool,
    pinned: bool,
    user_hidden: bool,
}

impl Default for SessionMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionMachine {
    /// An idle machine at generation 0.
    pub fn new() -> SessionMachine {
        SessionMachine {
            phase: Phase::Idle,
            sid: 0,
            last_activity: 0,
            ended_at: 0,
            deadline: 0,
            generation: 0,
            transient_shown: false,
            pinned: false,
            user_hidden: false,
        }
    }

    /// Every transition that changes the phase or a flag bumps the generation.
    fn bump(&mut self) {
        self.generation = self.generation.saturating_add(1);
    }

    /// A session starts: Active, transient visibility wanted, any pending expiry cancelled.
    /// A new session also lifts a tray hide from the previous one.
    pub fn started(&mut self, sid: u64, now: u64) {
        self.started_as(sid, now, true);
    }

    /// Like `started`, but the session owns a transient show only when `transient` is true. A
    /// session that starts with a persisted-visible orb passes false (A9-2), so the session
    /// never hides an orb the user keeps on.
    pub fn started_as(&mut self, sid: u64, now: u64, transient: bool) {
        self.phase = Phase::Active;
        self.sid = sid;
        self.last_activity = now;
        self.ended_at = now;
        self.deadline = now;
        self.transient_shown = transient;
        self.pinned = false;
        self.user_hidden = false;
        self.bump();
    }

    /// A transcript entry for session `token`. Refused when idle or for another session, and
    /// then nothing changes. While Lingering the deadline moves to `now + LINGER_MS`, capped at
    /// `ended_at + MAX_POST_END_EXTENSION_MS`.
    pub fn entry(&mut self, token: u64, now: u64) -> bool {
        match self.phase {
            Phase::Idle => false,
            _ if token != self.sid => false,
            Phase::Active => {
                self.last_activity = now;
                true
            }
            Phase::Lingering => {
                let cap = self.ended_at.saturating_add(MAX_POST_END_EXTENSION_MS);
                self.deadline = now.saturating_add(LINGER_MS).min(cap);
                true
            }
        }
    }

    /// The session `sid` ended. Only the current Active session can end; a stale or duplicate
    /// End changes nothing.
    pub fn ended(&mut self, sid: u64, now: u64) {
        if self.phase != Phase::Active || sid != self.sid {
            return;
        }
        self.phase = Phase::Lingering;
        self.ended_at = now;
        self.deadline = now.saturating_add(LINGER_MS);
        self.bump();
    }

    /// The orb became persisted-visible or the user turned it on: the transient show no longer
    /// owns it, so the session never hides it.
    pub fn pin(&mut self) {
        self.transient_shown = false;
        self.pinned = true;
        // An explicit show beats the session's own hide (tray Hide, then tray Show).
        self.user_hidden = false;
        self.bump();
    }

    /// The user hid the orb from the tray or menu: Idle, and no visibility for this session.
    /// The next `started` lifts it.
    pub fn user_hid(&mut self) {
        self.phase = Phase::Idle;
        self.transient_shown = false;
        self.pinned = false;
        self.user_hidden = true;
        self.bump();
    }

    /// The orb window was destroyed, the presentation changed, or the app is exiting.
    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.transient_shown = false;
        self.pinned = false;
        self.bump();
    }

    /// Advances time. A lingering session whose deadline has arrived goes Idle (even when the
    /// orb is pinned). An active session with no entry for `MAX_ACTIVE_MS` counts as ended.
    /// Idle does nothing.
    pub fn tick(&mut self, now: u64) {
        match self.phase {
            Phase::Idle => {}
            Phase::Active => {
                if now.saturating_sub(self.last_activity) >= MAX_ACTIVE_MS {
                    self.phase = Phase::Lingering;
                    self.ended_at = now;
                    self.deadline = now.saturating_add(LINGER_MS);
                    self.bump();
                }
            }
            Phase::Lingering => {
                if now >= self.deadline {
                    self.phase = Phase::Idle;
                    self.transient_shown = false;
                    self.pinned = false;
                    self.bump();
                }
            }
        }
    }

    /// Current phase.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// Current generation. Effects queued under an older value are stale.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether the current session is still showing the orb transiently.
    pub fn transient_shown(&self) -> bool {
        self.transient_shown
    }

    /// Whether the orb is pinned for good by the user or the settings.
    pub fn pinned(&self) -> bool {
        self.pinned
    }

    /// The current (or last) session id.
    pub fn session_id(&self) -> u64 {
        self.sid
    }

    /// Total desired orb visibility (A9-2). `persisted_visible` is `orb_should_show` read fresh
    /// at execution time and `supported` is the platform gate. A tray hide wins until the next
    /// session. Otherwise the orb is wanted when it is persisted-visible or while a transient
    /// show is active (not pinned, session not Idle).
    pub fn desired_orb_visible(&self, persisted_visible: bool, supported: bool) -> bool {
        if !supported || self.user_hidden {
            return false;
        }
        let transient = self.phase != Phase::Idle && self.transient_shown && !self.pinned;
        persisted_visible || transient
    }
}

/// Decides what the executor does for one reconcile step (A8-1 level-triggered effects). An
/// effect queued under `queued_generation` is dropped when the machine has moved on. Otherwise
/// the action is the difference between `desired` and `actual`, or nothing when they agree.
pub fn reconcile(
    desired: bool,
    actual: bool,
    queued_generation: u64,
    current_generation: u64,
) -> Option<OrbAction> {
    if queued_generation != current_generation {
        return None;
    }
    match (desired, actual) {
        (true, false) => Some(OrbAction::Show),
        (false, true) => Some(OrbAction::Hide),
        _ => None,
    }
}
