//! Pure shell core of the wake-appear orb and its bubbles (DESIGN.md A7-1 to A7-4, A8-2, A9-1,
//! A9-2 and the Revision 10 contracts).
//!
//! It joins the session machine (`orb_session`) with the bubble stack (`bubble_hub`) and the
//! readiness fallback. There are no Tauri types, no I/O and no clock reads: every input carries
//! `now` and the window facts, so every rule runs on a fake clock. The native shell
//! (`bubble_native`) owns the one instance behind a lock and carries out the returned effects.
#![cfg_attr(not(test), allow(dead_code))]

use crate::bubble_hub::{BubbleHub, HubEntry, Snapshot};
use crate::bubble_notice::NoticeCode;
use crate::chat_presentation::{
    floating_supported, resolve_presentation, PlatformEnv, Presentation,
};
use crate::managers::chat_log::GateState;
use crate::orb_session::{reconcile, transient_share, OrbAction, Phase, SessionMachine};

/// How long a page may take to report ready after its window was created, in milliseconds.
pub const READY_TIMEOUT_MS: u64 = 2_000;

/// Which surface a conversation session uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSurface {
    /// The wake-appear orb with the fading bubbles. The classic window does not open.
    OrbBubbles,
    /// The classic conversation window, as before.
    Window,
}

/// The facts the core needs for one step. `gate` is the chat-log admission tuple read by the
/// caller, `orb_persisted_visible` the saved orb setting, `supported` the platform gate and
/// `classic_visible` / `placement_ok` the current window facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellInputs {
    /// Monotonic time in milliseconds.
    pub now: u64,
    /// The chat log's admission tuple.
    pub gate: GateState,
    /// The orb setting as persisted (`orb_should_show` without the platform gate).
    pub orb_persisted_visible: bool,
    /// Whether floating is supported on this platform.
    pub supported: bool,
    /// Whether the classic conversation window is visible.
    pub classic_visible: bool,
    /// Whether the bubble stack has a placement beside the orb.
    pub placement_ok: bool,
}

/// What the caller must do after a step.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Effects {
    /// Show or hide the orb (level-triggered: the difference from the real window).
    pub orb: Option<OrbAction>,
    /// Set when the bubble window's visibility changed (true when bubbles now show).
    pub bubbles_visible: Option<bool>,
    /// A new projection revision to send to the bubble page.
    pub snapshot: Option<Snapshot>,
    /// Open the classic conversation window once for this session.
    pub open_classic: bool,
}

/// The whole shell: session machine, bubble hub, tracked orb state and the readiness fallback.
#[derive(Debug)]
pub struct BubbleShellCore {
    machine: SessionMachine,
    hub: BubbleHub,
    orb_shown: bool,
    window_created_at: u64,
    waiting_for_ready: bool,
    fallback_done: bool,
    fallback_sid: Option<u64>,
    /// A session started on the orb surface whose windows are not yet reported by the main thread.
    /// The readiness timeout does not run while it is set.
    windows_pending: bool,
    /// The highest session id that has ended. A start for an id at or below it lingers at once.
    last_ended_sid: Option<u64>,
}

/// The platform decision for a session start (DESIGN Rev 6 R6-2, A7-4). Floating surfaces as
/// the orb with bubbles when the stored choice resolves to floating and the platform supports it;
/// everything else keeps the classic window.
pub fn session_surface(requested: Option<Presentation>, env: &PlatformEnv) -> SessionSurface {
    let resolved = resolve_presentation(requested, env).presentation;
    match resolved {
        Presentation::Floating if floating_supported(env).is_ok() => SessionSurface::OrbBubbles,
        _ => SessionSurface::Window,
    }
}

impl BubbleShellCore {
    /// A shell with nothing shown. `recording` is the chat-log recording setting.
    pub fn new(recording: bool) -> BubbleShellCore {
        BubbleShellCore {
            machine: SessionMachine::new(),
            hub: BubbleHub::new(recording),
            orb_shown: false,
            window_created_at: 0,
            waiting_for_ready: false,
            fallback_done: false,
            fallback_sid: None,
            windows_pending: false,
            last_ended_sid: None,
        }
    }

    /// The bubble window was created: a new instance, timed from `now`. Returns the instance.
    pub fn window_created(&mut self, now: u64) -> u64 {
        self.window_created_at = now;
        self.hub.new_instance()
    }

    /// A conversation session started on `surface`. `windows_ok` says whether the orb and the
    /// bubble window exist. With the orb surface and no windows, the classic window opens once
    /// for this session and the hub stays disarmed. Equal to `session_begin` followed by
    /// `windows_ready`, which is how the native shell runs it.
    pub fn session_started(
        &mut self,
        sid: u64,
        surface: SessionSurface,
        windows_ok: bool,
        inp: &ShellInputs,
    ) -> Effects {
        match surface {
            SessionSurface::OrbBubbles if !windows_ok => {
                let mut eff = Effects::default();
                self.fallback(sid, &mut eff);
                self.finish(inp, eff)
            }
            _ => {
                let eff = self.begin(sid, surface, inp);
                self.windows_pending = false;
                self.finish(inp, eff)
            }
        }
    }

    /// The synchronous part of a session start (MAJOR 3 of G4b.fix): the session machine, the hub
    /// arming and the orb effects. The converse thread runs it under the lock; the windows are
    /// built on the main thread afterwards and reported through `windows_ready`.
    pub fn session_begin(
        &mut self,
        sid: u64,
        surface: SessionSurface,
        inp: &ShellInputs,
    ) -> Effects {
        let eff = self.begin(sid, surface, inp);
        self.finish(inp, eff)
    }

    /// The main thread built (or failed to build) the windows of session `sid`. A failure makes
    /// the session classic: the orb goes, the hub disarms and the classic window opens once.
    /// A report for a session that is no longer the current one changes nothing.
    pub fn windows_ready(&mut self, sid: u64, ok: bool, inp: &ShellInputs) -> Effects {
        let mut eff = Effects::default();
        if sid == self.machine.session_id() {
            self.windows_pending = false;
            if !ok {
                self.machine.reset();
                self.fallback(sid, &mut eff);
                eff.orb = self.orb_action(inp);
            }
        }
        self.finish(inp, eff)
    }

    /// The state change of a session start, without publishing.
    fn begin(&mut self, sid: u64, surface: SessionSurface, inp: &ShellInputs) -> Effects {
        match surface {
            SessionSurface::Window => {
                self.machine.reset();
                self.waiting_for_ready = false;
                self.windows_pending = false;
                self.hub.set_armed(false);
                Effects {
                    orb: self.orb_action(inp),
                    ..Effects::default()
                }
            }
            SessionSurface::OrbBubbles => {
                self.fallback_done = false;
                self.waiting_for_ready = false;
                self.windows_pending = true;
                self.hub.on_reset();
                self.hub.set_recording(inp.gate.recording);
                self.hub.set_armed(true);
                self.machine
                    .started_as(sid, inp.now, !inp.orb_persisted_visible);
                if self.last_ended_sid.is_some_and(|ended| ended >= sid) {
                    self.machine.ended(sid, inp.now);
                }
                Effects {
                    orb: self.orb_action(inp),
                    ..Effects::default()
                }
            }
        }
    }

    /// Records a failed or timed-out window set for session `sid`: the hub disarms, the readiness
    /// wait ends, and the classic window opens once for the session.
    fn fallback(&mut self, sid: u64, eff: &mut Effects) {
        self.hub.set_armed(false);
        self.waiting_for_ready = false;
        self.windows_pending = false;
        self.fallback_done = true;
        if self.fallback_sid != Some(sid) {
            self.fallback_sid = Some(sid);
            eff.open_classic = true;
        }
    }

    /// The session `sid` ended (voice or typed terminal state).
    pub fn session_ended(&mut self, sid: u64, inp: &ShellInputs) -> Effects {
        self.last_ended_sid = Some(self.last_ended_sid.map_or(sid, |last| last.max(sid)));
        self.machine.ended(sid, inp.now);
        let eff = Effects {
            orb: self.orb_action(inp),
            ..Effects::default()
        };
        self.finish(inp, eff)
    }

    /// A transcript line was offered. A refused line changes nothing: not the deadline, not the
    /// projection (A8-1, A9-1). An admitted line while the page is not ready is waiting.
    pub fn chat_entry(&mut self, entry: HubEntry, inp: &ShellInputs) -> Effects {
        self.sync(inp);
        let sid = entry.stamp.session_token;
        let belongs = self.machine.phase() != Phase::Idle && self.machine.session_id() == sid;
        if belongs && self.hub.on_entry(entry, inp.gate, inp.now) {
            self.machine.entry(sid, inp.now);
            if !self.hub.is_ready() {
                self.waiting_for_ready = true;
            }
        }
        self.finish(inp, Effects::default())
    }

    /// Shows a notice (DESIGN.md A-C3). Touches only the bubble stack: no session change, no orb
    /// action, no classic window. The page must be ready; the gates are checked when projected.
    pub fn notice(&mut self, code: NoticeCode, inp: &ShellInputs) -> Effects {
        self.hub.on_notice(code, inp.now);
        self.finish(inp, Effects::default())
    }

    /// The transcript was reset for a new session.
    pub fn chat_reset(&mut self, inp: &ShellInputs) -> Effects {
        self.hub.on_reset();
        self.finish(inp, Effects::default())
    }

    /// The user cleared the transcript.
    pub fn chat_clear(&mut self, inp: &ShellInputs) -> Effects {
        self.hub.on_clear();
        self.finish(inp, Effects::default())
    }

    /// The recording setting changed. Call after the chat log's gate has changed (A9-1).
    pub fn recording_changed(&mut self, on: bool, inp: &ShellInputs) -> Effects {
        self.hub.set_recording(on);
        self.finish(inp, Effects::default())
    }

    /// The bubble page of `instance` reported ready. Returns the snapshot to send, or `None`
    /// for a stale instance, or after the fallback opened the classic window.
    pub fn page_ready(&mut self, instance: u64, inp: &ShellInputs) -> Option<Snapshot> {
        if self.fallback_done || instance != self.hub.instance() {
            return None;
        }
        self.sync(inp);
        self.hub.set_ready(instance);
        self.waiting_for_ready = false;
        self.hub.snapshot_for_ready(instance, inp.now)
    }

    /// The real orb window became shown or hidden.
    pub fn orb_visibility_changed(&mut self, shown: bool, inp: &ShellInputs) -> Effects {
        self.orb_shown = shown;
        self.finish(inp, Effects::default())
    }

    /// The classic conversation window became shown or hidden.
    pub fn classic_visibility_changed(&mut self, visible: bool, inp: &ShellInputs) -> Effects {
        let updated = ShellInputs {
            classic_visible: visible,
            ..*inp
        };
        self.finish(&updated, Effects::default())
    }

    /// The user hid the orb from the tray or menu: no visibility for this session.
    pub fn user_hid_orb(&mut self, inp: &ShellInputs) -> Effects {
        self.machine.user_hid();
        let eff = Effects {
            orb: self.orb_action(inp),
            ..Effects::default()
        };
        self.finish(inp, eff)
    }

    /// The user showed the orb from the tray or menu: the session never hides it.
    pub fn user_showed_orb(&mut self, inp: &ShellInputs) -> Effects {
        self.machine.pin();
        let eff = Effects {
            orb: self.orb_action(inp),
            ..Effects::default()
        };
        self.finish(inp, eff)
    }

    /// The bubble window was destroyed, the presentation changed, or the app is exiting. The
    /// per-session fallback state is cleared too, so the next session builds a new window instance
    /// and its page is accepted (M1 of G4b.fix).
    pub fn reset(&mut self, inp: &ShellInputs) -> Effects {
        self.machine.reset();
        self.waiting_for_ready = false;
        self.windows_pending = false;
        self.fallback_sid = None;
        let eff = Effects {
            orb: self.orb_action(inp),
            ..Effects::default()
        };
        self.finish(inp, eff)
    }

    /// Advances time: the session deadline and the readiness timeout.
    pub fn tick(&mut self, inp: &ShellInputs) -> Effects {
        self.machine.tick(inp.now);
        let mut eff = Effects::default();
        let due = inp.now >= self.window_created_at.saturating_add(READY_TIMEOUT_MS);
        if self.waiting_for_ready
            && !self.fallback_done
            && !self.windows_pending
            && !self.hub.is_ready()
            && due
        {
            let sid = self.machine.session_id();
            self.fallback(sid, &mut eff);
        }
        eff.orb = self.orb_action(inp);
        self.finish(inp, eff)
    }

    /// Re-evaluates the projection after a window fact changed (placement, orb geometry).
    pub fn refresh(&mut self, inp: &ShellInputs) -> Effects {
        let eff = Effects {
            orb: self.orb_action(inp),
            ..Effects::default()
        };
        self.finish(inp, eff)
    }

    /// Whether no session is running (the timer may stop).
    pub fn is_idle(&self) -> bool {
        self.machine.phase() == Phase::Idle
    }

    /// The current session phase.
    pub fn phase(&self) -> Phase {
        self.machine.phase()
    }

    /// The session's own share of the orb (MAJOR 1 of G4b.fix): what the executor stores in
    /// `ORB_TRANSIENT`. Never the saved setting.
    pub fn orb_share(&self) -> bool {
        transient_share(&self.machine)
    }

    /// Total desired orb visibility (A9-2).
    pub fn desired_orb(&self, inp: &ShellInputs) -> bool {
        self.machine
            .desired_orb_visible(inp.orb_persisted_visible, inp.supported)
    }

    /// Whether bubbles would show now (projection non-empty).
    pub fn bubbles_wanted(&self, now: u64) -> bool {
        !self.hub.projection(now).is_empty()
    }

    /// The current snapshot, unpublished, for a re-send that must keep the revision (a changed
    /// anchor). `None` before any window instance exists.
    pub fn current_snapshot(&self, now: u64) -> Option<Snapshot> {
        self.hub.current(now)
    }

    /// The current bubble window instance.
    pub fn instance(&self) -> u64 {
        self.hub.instance()
    }

    /// Copies the tracked facts into the hub before a projection update.
    fn sync(&mut self, inp: &ShellInputs) {
        self.hub.set_orb_visible(self.orb_shown);
        self.hub.set_classic_visible(inp.classic_visible);
        self.hub.set_placement_ok(inp.placement_ok);
    }

    /// The orb action that carries the machine's desired visibility to the real window.
    fn orb_action(&self, inp: &ShellInputs) -> Option<OrbAction> {
        let generation = self.machine.generation();
        let desired = self
            .machine
            .desired_orb_visible(inp.orb_persisted_visible, inp.supported);
        reconcile(desired, self.orb_shown, generation, generation)
    }

    /// Ends a step: disarms when idle, then publishes the projection when it changed.
    fn finish(&mut self, inp: &ShellInputs, mut eff: Effects) -> Effects {
        self.sync(inp);
        if self.machine.phase() == Phase::Idle {
            self.hub.set_armed(false);
        }
        if let Some(snapshot) = self.hub.publish(inp.now) {
            eff.bubbles_visible = Some(!snapshot.items.is_empty());
            eff.snapshot = Some(snapshot);
        }
        eff
    }
}
