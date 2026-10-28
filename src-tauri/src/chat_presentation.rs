//! Presentation of the chat conversation: the classic window or the floating panel (DESIGN.md
//! Rev 4, 'Presentation setting'). Pure decisions over an explicit platform description, so the
//! rules can be tested on any host. Linux backend detection at runtime arrives in a later step;
//! here the backend is only ever supplied by the caller.
#![cfg_attr(not(test), allow(dead_code))]

use serde::{Deserialize, Serialize};
use specta::Type;

/// Whether the conversation uses the floating panel or the classic window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum Presentation {
    /// Floating orb with an anchored chat panel.
    Floating,
    /// The classic decorated chat window.
    Window,
}

/// Operating system family relevant to presentation decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// macOS.
    MacOs,
    /// Windows.
    Windows,
    /// Linux (any display server).
    Linux,
    /// Anything else.
    Other,
}

/// Display server behind a Linux session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinuxBackend {
    /// X11.
    X11,
    /// Native Wayland.
    Wayland,
    /// Could not be determined.
    Unknown,
}

/// The platform facts that presentation decisions depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformEnv {
    /// Operating system family.
    pub os: Os,
    /// Linux display server (`Unknown` off Linux).
    pub linux_backend: LinuxBackend,
    /// Whether a compositing window manager is running (relevant on X11 only).
    pub composited: bool,
}

/// Why the floating panel cannot be used on this platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatingBlock {
    /// Native Wayland: always-on-top floating windows are not reliable.
    Wayland,
    /// Linux display server could not be identified.
    UnknownBackend,
    /// X11 without a compositing window manager.
    NoCompositor,
    /// Operating system not verified for floating windows.
    UnsupportedOs,
}

/// The presentation actually in effect, and the reason it differs from the request if it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Effective {
    /// Presentation to use.
    pub presentation: Presentation,
    /// Set when floating was requested but this platform falls back to the window.
    pub blocked: Option<FloatingBlock>,
}

/// Platform default when the user has not chosen: floating only where it has been verified
/// (macOS), the classic window everywhere else.
pub fn default_presentation(os: Os) -> Presentation {
    match os {
        Os::MacOs => Presentation::Floating,
        Os::Windows | Os::Linux | Os::Other => Presentation::Window,
    }
}

/// Whether the floating panel can be used on this platform.
///
/// macOS and Windows: always. Linux: X11 only, and only with a compositing window manager.
/// Wayland and unidentified backends are refused first, then missing compositing. Other systems
/// are refused.
pub fn floating_supported(env: &PlatformEnv) -> Result<(), FloatingBlock> {
    match env.os {
        Os::MacOs | Os::Windows => Ok(()),
        Os::Linux => match env.linux_backend {
            LinuxBackend::Wayland => Err(FloatingBlock::Wayland),
            LinuxBackend::Unknown => Err(FloatingBlock::UnknownBackend),
            LinuxBackend::X11 if env.composited => Ok(()),
            LinuxBackend::X11 => Err(FloatingBlock::NoCompositor),
        },
        Os::Other => Err(FloatingBlock::UnsupportedOs),
    }
}

/// The presentation to use for the stored choice on this platform.
///
/// `None` (never chosen) uses the platform default and is never reported as blocked. An explicit
/// `Window` is always honoured. An explicit `Floating` is honoured where supported and otherwise
/// falls back to the window with the reason recorded.
pub fn resolve_presentation(requested: Option<Presentation>, env: &PlatformEnv) -> Effective {
    match requested {
        None => Effective {
            presentation: default_presentation(env.os),
            blocked: None,
        },
        Some(Presentation::Window) => Effective {
            presentation: Presentation::Window,
            blocked: None,
        },
        Some(Presentation::Floating) => match floating_supported(env) {
            Ok(()) => Effective {
                presentation: Presentation::Floating,
                blocked: None,
            },
            Err(why) => Effective {
                presentation: Presentation::Window,
                blocked: Some(why),
            },
        },
    }
}

/// Reads the Linux display server from a GTK display type name (for example `GdkX11Display`).
/// Anything not recognised, including a missing name, is `Unknown`.
pub fn linux_backend_from_display_type(display_type: Option<&str>) -> LinuxBackend {
    match display_type {
        Some("GdkX11Display") => LinuxBackend::X11,
        Some("GdkWaylandDisplay") => LinuxBackend::Wayland,
        _ => LinuxBackend::Unknown,
    }
}

/// Whether the orb is shown when the setting is unset. Stays `false` until the orb is verified
/// on the release platform.
pub const ORB_VISIBLE_DEFAULT: bool = false;

/// Whether the orb is shown: when the setting is enabled (`None` means [`ORB_VISIBLE_DEFAULT`])
/// and this platform can float a window. The orb is a launcher for the classic window as well, so
/// it does not depend on the chosen presentation.
pub fn orb_should_show(orb_visible: Option<bool>, env: &PlatformEnv) -> bool {
    orb_visible.unwrap_or(ORB_VISIBLE_DEFAULT) && floating_supported(env).is_ok()
}

/// The operating system this build runs on. Unused until the orb window is wired (G2+).
#[allow(dead_code)]
pub fn current_os() -> Os {
    if cfg!(target_os = "macos") {
        Os::MacOs
    } else if cfg!(target_os = "windows") {
        Os::Windows
    } else if cfg!(target_os = "linux") {
        Os::Linux
    } else {
        Os::Other
    }
}

/// The wire name of a floating block, as the settings UI reads it (`reason.<key>` text).
pub fn block_key(block: FloatingBlock) -> &'static str {
    match block {
        FloatingBlock::Wayland => "wayland",
        FloatingBlock::UnknownBackend => "unknown_backend",
        FloatingBlock::NoCompositor => "no_compositor",
        FloatingBlock::UnsupportedOs => "unsupported_os",
    }
}

/// Whether the floating orb can be used on this system, and the block key when it cannot.
/// `block` is `None` exactly when `supported` is true.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
pub struct OrbSupport {
    /// The floating orb can be shown on this system.
    pub supported: bool,
    /// Why it cannot (see [`block_key`]); `None` when supported.
    pub block: Option<String>,
}

/// The orb support for a platform description, derived from [`floating_supported`].
pub fn orb_support_for(env: &PlatformEnv) -> OrbSupport {
    match floating_supported(env) {
        Ok(()) => OrbSupport {
            supported: true,
            block: None,
        },
        Err(why) => OrbSupport {
            supported: false,
            block: Some(block_key(why).to_string()),
        },
    }
}
