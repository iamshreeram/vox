//! G1 spec: presentation resolution (DESIGN.md Rev 4: 'Presentation setting').
//! Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (module `crate::chat_presentation`):
//!   enum Presentation { Floating, Window }  (Debug, Clone, Copy, PartialEq, Eq, serde snake_case, specta Type)
//!   enum Os { MacOs, Windows, Linux, Other }  enum LinuxBackend { X11, Wayland, Unknown }   (Debug, Clone, Copy, PartialEq, Eq)
//!   struct PlatformEnv { os: Os, linux_backend: LinuxBackend, composited: bool }             (Debug, Clone, Copy, PartialEq, Eq)
//!   enum FloatingBlock { Wayland, UnknownBackend, NoCompositor, UnsupportedOs }              (Debug, Clone, Copy, PartialEq, Eq)
//!   struct Effective { presentation: Presentation, blocked: Option<FloatingBlock> }          (Debug, Clone, Copy, PartialEq, Eq)
//!   fn default_presentation(Os) -> Presentation
//!   fn floating_supported(&PlatformEnv) -> Result<(), FloatingBlock>
//!   fn resolve_presentation(Option<Presentation>, &PlatformEnv) -> Effective
//!   fn linux_backend_from_display_type(Option<&str>) -> LinuxBackend
//!   const ORB_VISIBLE_DEFAULT: bool = false     // flipped to true in G4 (a deliberate, tested change)
//!   fn orb_should_show(orb_visible: Option<bool>, effective: Presentation) -> bool
use crate::chat_presentation::*;

fn env(os: Os, backend: LinuxBackend, composited: bool) -> PlatformEnv {
    PlatformEnv {
        os,
        linux_backend: backend,
        composited,
    }
}

fn mac() -> PlatformEnv {
    env(Os::MacOs, LinuxBackend::Unknown, false)
}
fn windows() -> PlatformEnv {
    env(Os::Windows, LinuxBackend::Unknown, false)
}
fn x11(composited: bool) -> PlatformEnv {
    env(Os::Linux, LinuxBackend::X11, composited)
}

fn ok(presentation: Presentation) -> Effective {
    Effective {
        presentation,
        blocked: None,
    }
}
fn blocked(why: FloatingBlock) -> Effective {
    Effective {
        presentation: Presentation::Window,
        blocked: Some(why),
    }
}

#[test]
fn platform_defaults_float_only_where_verified() {
    assert_eq!(default_presentation(Os::MacOs), Presentation::Floating);
    assert_eq!(default_presentation(Os::Windows), Presentation::Window);
    assert_eq!(default_presentation(Os::Linux), Presentation::Window);
    assert_eq!(default_presentation(Os::Other), Presentation::Window);
}

#[test]
fn without_an_explicit_setting_the_platform_default_applies() {
    assert_eq!(
        resolve_presentation(None, &mac()),
        ok(Presentation::Floating)
    );
    assert_eq!(
        resolve_presentation(None, &windows()),
        ok(Presentation::Window)
    );
    assert_eq!(
        resolve_presentation(None, &x11(true)),
        ok(Presentation::Window)
    );
    // A default of Window is never reported as blocked, even on an unsupported backend.
    assert_eq!(
        resolve_presentation(None, &env(Os::Linux, LinuxBackend::Wayland, true)),
        ok(Presentation::Window)
    );
}

#[test]
fn an_explicit_window_is_honoured_everywhere() {
    for e in [
        mac(),
        windows(),
        x11(true),
        env(Os::Linux, LinuxBackend::Wayland, true),
    ] {
        assert_eq!(
            resolve_presentation(Some(Presentation::Window), &e),
            ok(Presentation::Window)
        );
    }
}

#[test]
fn an_explicit_floating_works_on_macos_windows_and_composited_x11() {
    let floating = ok(Presentation::Floating);
    assert_eq!(
        resolve_presentation(Some(Presentation::Floating), &mac()),
        floating
    );
    assert_eq!(
        resolve_presentation(Some(Presentation::Floating), &windows()),
        floating
    );
    assert_eq!(
        resolve_presentation(Some(Presentation::Floating), &x11(true)),
        floating
    );
}

#[test]
fn unsupported_configurations_fall_back_to_window_with_a_reason() {
    let want = Some(Presentation::Floating);
    assert_eq!(
        resolve_presentation(want, &x11(false)),
        blocked(FloatingBlock::NoCompositor)
    );
    assert_eq!(
        resolve_presentation(want, &env(Os::Linux, LinuxBackend::Wayland, true)),
        blocked(FloatingBlock::Wayland)
    );
    assert_eq!(
        resolve_presentation(want, &env(Os::Linux, LinuxBackend::Unknown, true)),
        blocked(FloatingBlock::UnknownBackend)
    );
    assert_eq!(
        resolve_presentation(want, &env(Os::Other, LinuxBackend::Unknown, true)),
        blocked(FloatingBlock::UnsupportedOs)
    );
}

#[test]
fn wayland_takes_precedence_over_a_missing_compositor() {
    assert_eq!(
        floating_supported(&env(Os::Linux, LinuxBackend::Wayland, false)),
        Err(FloatingBlock::Wayland)
    );
    assert_eq!(
        floating_supported(&env(Os::Linux, LinuxBackend::Unknown, false)),
        Err(FloatingBlock::UnknownBackend)
    );
}

#[test]
fn compositing_is_irrelevant_on_macos_and_windows() {
    assert_eq!(
        floating_supported(&env(Os::MacOs, LinuxBackend::Wayland, false)),
        Ok(())
    );
    assert_eq!(
        floating_supported(&env(Os::Windows, LinuxBackend::Unknown, false)),
        Ok(())
    );
}

#[test]
fn the_linux_backend_comes_from_the_display_type_name_only() {
    assert_eq!(
        linux_backend_from_display_type(Some("GdkX11Display")),
        LinuxBackend::X11
    );
    assert_eq!(
        linux_backend_from_display_type(Some("GdkWaylandDisplay")),
        LinuxBackend::Wayland
    );
    assert_eq!(
        linux_backend_from_display_type(Some("GdkBroadwayDisplay")),
        LinuxBackend::Unknown
    );
    assert_eq!(
        linux_backend_from_display_type(Some("")),
        LinuxBackend::Unknown
    );
    assert_eq!(linux_backend_from_display_type(None), LinuxBackend::Unknown);
}

#[test]
fn the_orb_shows_only_when_floating_and_enabled() {
    assert!(
        !ORB_VISIBLE_DEFAULT,
        "G1-G3 ship with the orb hidden until the user enables it"
    );
    assert!(!orb_should_show(None, Presentation::Floating));
    assert!(orb_should_show(Some(true), Presentation::Floating));
    assert!(!orb_should_show(Some(false), Presentation::Floating));
    assert!(!orb_should_show(Some(true), Presentation::Window));
    assert!(!orb_should_show(None, Presentation::Window));
}

#[test]
fn presentation_serialises_as_snake_case() {
    assert_eq!(
        serde_json::to_string(&Presentation::Floating).unwrap(),
        "\"floating\""
    );
    assert_eq!(
        serde_json::to_string(&Presentation::Window).unwrap(),
        "\"window\""
    );
    let back: Presentation = serde_json::from_str("\"window\"").unwrap();
    assert_eq!(back, Presentation::Window);
}
