//! G3c spec: orb support for the settings UI (DESIGN.md 'Settings (G1)' and 'Frontend' ->
//! 'Settings UI'). Authoritative: do not weaken an assertion; if a test is wrong, report it.
//!
//! Contract (implementer builds to this; pure parts live in `chat_presentation.rs`):
//!   crate::chat_presentation::block_key(FloatingBlock) -> &'static str
//!       Wayland -> "wayland", UnknownBackend -> "unknown_backend",
//!       NoCompositor -> "no_compositor", UnsupportedOs -> "unsupported_os".
//!   crate::chat_presentation::OrbSupport { supported: bool, block: Option<String> }
//!       (Debug, Clone, PartialEq, Eq, Serialize, specta Type)
//!   crate::chat_presentation::orb_support_for(&PlatformEnv) -> OrbSupport
//!   A main-window command `get_orb_support` (async, returns OrbSupport, real platform env) is
//!   registered in lib.rs `collect_commands!` and is NOT in the orb or chat command lists.
use crate::chat_presentation::*;
use crate::window_commands::{window_command_allowed, CHAT_COMMANDS, ORB_COMMANDS};
use serde::Serialize;
use std::fmt::Debug;
use std::path::{Path, PathBuf};

const ALL_OS: [Os; 4] = [Os::MacOs, Os::Windows, Os::Linux, Os::Other];
const ALL_BACKENDS: [LinuxBackend; 3] = [
    LinuxBackend::X11,
    LinuxBackend::Wayland,
    LinuxBackend::Unknown,
];

fn env(os: Os, backend: LinuxBackend, composited: bool) -> PlatformEnv {
    PlatformEnv {
        os,
        linux_backend: backend,
        composited,
    }
}

fn supported() -> OrbSupport {
    OrbSupport {
        supported: true,
        block: None,
    }
}

fn blocked(key: &str) -> OrbSupport {
    OrbSupport {
        supported: false,
        block: Some(key.to_string()),
    }
}

fn assert_traits<T: Debug + Clone + PartialEq + Eq + Serialize>() {}
fn assert_specta<T: specta::Type>() {}

#[test]
fn orb_support_has_the_documented_derives() {
    assert_traits::<OrbSupport>();
    assert_specta::<OrbSupport>();
}

#[test]
fn block_keys_are_the_exact_wire_strings() {
    assert_eq!(block_key(FloatingBlock::Wayland), "wayland");
    assert_eq!(block_key(FloatingBlock::UnknownBackend), "unknown_backend");
    assert_eq!(block_key(FloatingBlock::NoCompositor), "no_compositor");
    assert_eq!(block_key(FloatingBlock::UnsupportedOs), "unsupported_os");
}

#[test]
fn macos_and_windows_are_supported_without_a_block() {
    for e in [
        env(Os::MacOs, LinuxBackend::Unknown, false),
        env(Os::MacOs, LinuxBackend::Unknown, true),
        env(Os::Windows, LinuxBackend::Unknown, false),
        env(Os::Windows, LinuxBackend::Unknown, true),
    ] {
        assert_eq!(orb_support_for(&e), supported(), "{e:?}");
    }
}

#[test]
fn linux_x11_is_supported_only_with_a_compositor() {
    assert_eq!(
        orb_support_for(&env(Os::Linux, LinuxBackend::X11, true)),
        supported()
    );
    assert_eq!(
        orb_support_for(&env(Os::Linux, LinuxBackend::X11, false)),
        blocked("no_compositor")
    );
}

#[test]
fn native_wayland_is_blocked_even_when_it_reports_a_compositor() {
    for composited in [false, true] {
        assert_eq!(
            orb_support_for(&env(Os::Linux, LinuxBackend::Wayland, composited)),
            blocked("wayland"),
            "composited={composited}"
        );
    }
}

#[test]
fn an_unidentified_linux_backend_is_blocked_as_unknown_backend() {
    for composited in [false, true] {
        assert_eq!(
            orb_support_for(&env(Os::Linux, LinuxBackend::Unknown, composited)),
            blocked("unknown_backend"),
            "composited={composited}"
        );
    }
}

#[test]
fn other_systems_are_blocked_as_unsupported_os() {
    for backend in ALL_BACKENDS {
        for composited in [false, true] {
            assert_eq!(
                orb_support_for(&env(Os::Other, backend, composited)),
                blocked("unsupported_os"),
                "{backend:?} composited={composited}"
            );
        }
    }
}

#[test]
fn support_agrees_with_floating_supported_on_every_platform() {
    for os in ALL_OS {
        for backend in ALL_BACKENDS {
            for composited in [false, true] {
                let e = env(os, backend, composited);
                let got = orb_support_for(&e);
                match floating_supported(&e) {
                    Ok(()) => assert_eq!(got, supported(), "{e:?}"),
                    Err(why) => assert_eq!(got, blocked(block_key(why)), "{e:?}"),
                }
                assert_eq!(got.supported, got.block.is_none(), "{e:?}");
            }
        }
    }
}

#[test]
fn support_serialises_to_the_frontend_shape() {
    assert_eq!(
        serde_json::to_value(supported()).expect("serialise"),
        serde_json::json!({ "supported": true, "block": null })
    );
    assert_eq!(
        serde_json::to_value(blocked("wayland")).expect("serialise"),
        serde_json::json!({ "supported": false, "block": "wayland" })
    );
}

#[test]
fn get_orb_support_is_a_main_window_command_only() {
    assert!(window_command_allowed("main", "get_orb_support"));
    assert!(!window_command_allowed("orb", "get_orb_support"));
    assert!(!window_command_allowed("chat", "get_orb_support"));
    assert!(!ORB_COMMANDS.contains(&"get_orb_support"));
    assert!(!CHAT_COMMANDS.contains(&"get_orb_support"));
}

#[test]
fn window_command_matrix_does_not_list_get_orb_support() {
    let matrix = include_str!("window_commands.rs");
    assert!(
        !matrix.contains("get_orb_support"),
        "window_commands.rs must not list get_orb_support"
    );
}

#[test]
fn get_orb_support_is_registered_in_the_main_builder() {
    let lib = include_str!("lib.rs");
    let at = lib
        .find("collect_commands![")
        .expect("lib.rs has a collect_commands! block");
    assert!(
        lib[at..].contains("get_orb_support"),
        "get_orb_support must be listed in lib.rs collect_commands!"
    );
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn get_orb_support_is_an_async_command_returning_orb_support() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_sources(&src_dir, &mut files);
    let definition =
        regex::Regex::new(r"pub async fn get_orb_support\s*\([^)]*\)\s*->\s*OrbSupport\b")
            .expect("regex");
    let found = files.iter().any(|path| {
        let is_test = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with("_tests.rs"));
        !is_test
            && std::fs::read_to_string(path)
                .map(|text| definition.is_match(&text))
                .unwrap_or(false)
    });
    assert!(
        found,
        "expected `pub async fn get_orb_support(..) -> OrbSupport` in a non-test source file"
    );
}
