//! Chat surface dispatcher and its pure state machine (DESIGN.md Rev 5, 'Surface state machine
//! and placement', step G2). Every conversation entry point (hotkey, tray item, settings button,
//! session start) goes through `toggle` or `open_on_session_start` here. In G2 both always open
//! the classic window; the floating panel is not available yet (`PANEL_AVAILABLE`).
//!
//! The machine holds no Tauri types. Its effects are instructions the caller carries out.
#![cfg_attr(not(test), allow(dead_code))]

use crate::activation_policy::{open_may_focus, OpenKind};
use crate::bubble_native;
use crate::bubble_shell_core::{session_surface, SessionSurface};
use crate::chat_presentation::{
    current_os, resolve_presentation, LinuxBackend, PlatformEnv, Presentation,
};
use crate::chat_window::{self, toggle_decision, ToggleAction};
use crate::settings;
use tauri::AppHandle;

/// Whether a surface is shown, and for which generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Not shown.
    Hidden,
    /// Show requested for this generation; waiting for the page to acknowledge.
    Showing(u64),
    /// Shown and acknowledged for this generation.
    Shown(u64),
    /// Hiding after the exit animation, for this generation.
    Hiding(u64),
}

/// Something that happened to the surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// The app was asked to open the surface, and why.
    Open(OpenKind),
    /// The surface should close.
    Close,
    /// The hotkey or tray toggle was used; `focused` says whether the surface has focus.
    Toggle { focused: bool },
    /// The page reports it is shown, for this generation.
    ShownAck(u64),
    /// The exit-animation timer for this generation fired.
    HideTimer(u64),
    /// The window was destroyed.
    Destroyed,
}

/// What the caller must do after an event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Show the surface, taking focus only if `focus` is set.
    Show {
        /// Whether the surface may take keyboard focus.
        focus: bool,
    },
    /// Start the exit-animation timer for this generation; it fires `HideTimer(gen)`.
    StartHide {
        /// Generation the timer belongs to.
        gen: u64,
        /// Milliseconds until the timer fires.
        after_ms: u64,
    },
    /// Hide the surface immediately.
    HideNow,
    /// Cancel any pending exit-animation timer.
    CancelTimers,
}

/// Pure state machine for one chat surface. Every visibility transition bumps `gen`, so a timer
/// or acknowledgement from an earlier transition can be recognised and ignored.
#[derive(Debug, Clone)]
pub struct SurfaceMachine {
    /// Current visibility.
    visibility: Visibility,
    /// Current generation. Only grows.
    gen: u64,
    /// Exit-animation length in milliseconds. Zero hides at once.
    exit_ms: u64,
}

impl SurfaceMachine {
    /// A hidden surface with the given exit-animation length in milliseconds.
    pub fn new(exit_ms: u64) -> Self {
        Self {
            visibility: Visibility::Hidden,
            gen: 0,
            exit_ms,
        }
    }

    /// The current visibility.
    pub fn visibility(&self) -> Visibility {
        self.visibility
    }

    /// The current generation.
    pub fn gen(&self) -> u64 {
        self.gen
    }

    /// Applies one event and returns the effects the caller must carry out.
    pub fn apply(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Open(kind) => self.open(kind),
            Event::Close => self.close(),
            Event::Toggle { focused } => self.toggle(focused),
            Event::ShownAck(gen) => self.shown_ack(gen),
            Event::HideTimer(gen) => self.hide_timer(gen),
            Event::Destroyed => self.destroyed(),
        }
    }

    /// Advances the generation. Saturates rather than wrapping, so it never goes backwards.
    fn bump(&mut self) -> u64 {
        self.gen = self.gen.saturating_add(1);
        self.gen
    }

    /// Starts showing for a new generation, after any `prefix` effects.
    fn begin_show(&mut self, focus: bool, mut effects: Vec<Effect>) -> Vec<Effect> {
        let gen = self.bump();
        self.visibility = Visibility::Showing(gen);
        effects.push(Effect::Show { focus });
        effects
    }

    /// Starts hiding: immediately when there is no exit animation, otherwise through a timer.
    fn begin_hide(&mut self) -> Vec<Effect> {
        let gen = self.bump();
        if self.exit_ms == 0 {
            self.visibility = Visibility::Hidden;
            vec![Effect::HideNow]
        } else {
            self.visibility = Visibility::Hiding(gen);
            vec![Effect::StartHide {
                gen,
                after_ms: self.exit_ms,
            }]
        }
    }

    /// Open: a new show when hidden or hiding; while visible, only a user open re-focuses.
    fn open(&mut self, kind: OpenKind) -> Vec<Effect> {
        let focus = open_may_focus(kind);
        match self.visibility {
            Visibility::Hidden => self.begin_show(focus, Vec::new()),
            Visibility::Hiding(_) => self.begin_show(focus, vec![Effect::CancelTimers]),
            Visibility::Showing(_) | Visibility::Shown(_) => {
                if focus {
                    vec![Effect::Show { focus: true }]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Close: hides a showing or shown surface; nothing otherwise.
    fn close(&mut self) -> Vec<Effect> {
        match self.visibility {
            Visibility::Showing(_) | Visibility::Shown(_) => self.begin_hide(),
            Visibility::Hidden | Visibility::Hiding(_) => Vec::new(),
        }
    }

    /// Toggle: the same decision as the classic window. A hiding surface always reopens.
    fn toggle(&mut self, focused: bool) -> Vec<Effect> {
        match self.visibility {
            Visibility::Showing(_) | Visibility::Shown(_) => match toggle_decision(true, focused) {
                ToggleAction::Hide => self.begin_hide(),
                ToggleAction::Show => vec![Effect::Show { focus: true }],
            },
            Visibility::Hidden => self.begin_show(true, Vec::new()),
            Visibility::Hiding(_) => self.begin_show(true, vec![Effect::CancelTimers]),
        }
    }

    /// Acknowledgement: only the current generation's show is accepted. Does not bump `gen`.
    fn shown_ack(&mut self, gen: u64) -> Vec<Effect> {
        if self.visibility == Visibility::Showing(gen) {
            self.visibility = Visibility::Shown(gen);
        }
        Vec::new()
    }

    /// Timer: hides only when the timer belongs to the current hiding generation.
    fn hide_timer(&mut self, gen: u64) -> Vec<Effect> {
        if self.visibility == Visibility::Hiding(gen) {
            self.bump();
            self.visibility = Visibility::Hidden;
            vec![Effect::HideNow]
        } else {
            Vec::new()
        }
    }

    /// Destruction: cancels timers, resets to hidden, and invalidates every outstanding timer.
    fn destroyed(&mut self) -> Vec<Effect> {
        self.bump();
        self.visibility = Visibility::Hidden;
        vec![Effect::CancelTimers]
    }
}

/// Which surface a conversation entry point opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// The classic decorated conversation window.
    Classic,
    /// The floating orb with its anchored chat panel.
    Panel,
}

/// Whether the floating panel exists in this build. G4 flips it together with the default and
/// its tests, as a deliberate change.
pub const PANEL_AVAILABLE: bool = false;

/// The surface for a stored presentation choice on a platform. `Panel` only when the panel
/// exists and the resolved presentation is floating; otherwise `Classic`.
pub fn target_for(setting: Option<Presentation>, env: &PlatformEnv) -> Target {
    if PANEL_AVAILABLE && resolve_presentation(setting, env).presentation == Presentation::Floating
    {
        Target::Panel
    } else {
        Target::Classic
    }
}

/// The platform facts this build can report. The Linux backend is unknown until the runtime
/// adapter exists (a later step), so Linux resolves to the classic window.
fn current_env() -> PlatformEnv {
    PlatformEnv {
        os: current_os(),
        linux_backend: LinuxBackend::Unknown,
        composited: false,
    }
}

/// The surface the current settings and platform call for.
fn current_target(app: &AppHandle) -> Target {
    target_for(
        settings::get_settings(app).chat_presentation,
        &current_env(),
    )
}

/// Single entry for the hotkey, the tray item and the settings button: shows or hides the
/// conversation surface.
pub fn toggle(app: &AppHandle) -> tauri::Result<()> {
    match current_target(app) {
        Target::Classic => chat_window::toggle_chat_window(app),
        // Unreachable while PANEL_AVAILABLE is false. Falls back to the classic window, never panics.
        Target::Panel => chat_window::toggle_chat_window(app),
    }
}

/// Single entry for the conversation start (auto-open, never focuses). The surface is decided
/// on the main thread, so the session-start hook itself never reads the settings store. `sid` is
/// the chat log's session token taken by the hook (A8-1). The orb surface starts the wake-appear
/// orb with its bubbles; the classic surface opens the conversation window as before.
pub fn open_on_session_start(app: &AppHandle, sid: u64) {
    let job_app = app.clone();
    let _ = app.run_on_main_thread(move || {
        let requested = settings::get_settings(&job_app).chat_presentation;
        match session_surface(requested, &current_env()) {
            SessionSurface::OrbBubbles => bubble_native::session_started(&job_app, sid),
            SessionSurface::Window => chat_window::open_chat_window_on_session_start(&job_app),
        }
    });
}
