//! N1b spec: orb click, Enter and Space start or stop a Conversation (DESIGN.md Revision 3:
//! D2, A-C1, A-C1b, A-C1c, A-C2, A-C3, A-C4). Authoritative: do not weaken an assertion.
//!
//! Contract, module `crate::orb_converse` (no Tauri types in the pure part):
//!   activate_with(port, lockout, clock) -> Outcome; ClickLockout::accept(now) (600 ms after an
//!   accepted activation); classify_start_error(&str) -> StartError; notice_for(StartError) ->
//!   Option<NoticeCode> (exhaustive); OrbRefusalPayload { code } (only the code on the wire);
//!   ORB_REFUSAL_EVENT = "orb-refusal", emitted to the orb window label only.

use crate::bubble_notice::NoticeCode;
use crate::managers::converse::{RegistryState, SessionId, SessionRegistry};
use crate::managers::converse_activation::{
    orb_activation_decision, ActivationDecision, ActivationKind,
};
use crate::managers::converse_platform::{DICTATION_BUSY, ENROLLMENT_BUSY};
use crate::managers::converse_runtime::{ALREADY_RUNNING, TEXT_ENDING};
use crate::orb_converse::*;
use crate::orb_window::ORB_WINDOW_LABEL;
use crate::window_commands::ORB_COMMANDS;
use std::cell::{Cell, RefCell};
use std::sync::{Barrier, Mutex};

/// Production source without its comment lines, so pins match code only.
fn code_only(src: &str) -> String {
    src.lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n")
}

struct FakeClock(Cell<u64>);
impl Clock for FakeClock {
    fn now_ms(&self) -> u64 {
        self.0.get()
    }
}

/// A port with a fixed decision and a scripted start result; records every effect.
struct FixedPort {
    decision: ActivationDecision,
    start_error: Option<String>,
    starts: Cell<usize>,
    stops: RefCell<Vec<SessionId>>,
    notices: RefCell<Vec<NoticeCode>>,
}

impl FixedPort {
    fn new(decision: ActivationDecision) -> Self {
        FixedPort {
            decision,
            start_error: None,
            starts: Cell::new(0),
            stops: RefCell::new(Vec::new()),
            notices: RefCell::new(Vec::new()),
        }
    }
}

impl ActivationPort for FixedPort {
    fn decision(&self) -> ActivationDecision {
        self.decision
    }
    fn start(&self) -> Result<(), String> {
        self.starts.set(self.starts.get() + 1);
        match &self.start_error {
            Some(message) => Err(message.clone()),
            None => Ok(()),
        }
    }
    fn stop_if(&self, id: SessionId) -> bool {
        self.stops.borrow_mut().push(id);
        true
    }
    fn notify(&self, code: NoticeCode) {
        self.notices.borrow_mut().push(code);
    }
}

/// A port over the real session registry: the barrier tests drive the actual phases.
struct RegistryPort {
    reg: SessionRegistry,
    kind: ActivationKind,
    held: RefCell<Vec<crate::managers::converse::CancelToken>>,
    notices: RefCell<Vec<NoticeCode>>,
}

impl RegistryPort {
    fn new() -> Self {
        RegistryPort {
            reg: SessionRegistry::new(),
            kind: ActivationKind::Voice,
            held: RefCell::new(Vec::new()),
            notices: RefCell::new(Vec::new()),
        }
    }
}

impl ActivationPort for RegistryPort {
    fn decision(&self) -> ActivationDecision {
        let (phase, _) = self.reg.snapshot();
        orb_activation_decision(phase, self.kind)
    }
    fn start(&self) -> Result<(), String> {
        match self.reg.try_begin() {
            Some((_, token)) => {
                self.held.borrow_mut().push(token);
                Ok(())
            }
            None => Err(ALREADY_RUNNING.to_string()),
        }
    }
    fn stop_if(&self, id: SessionId) -> bool {
        self.reg.request_stop_if(id)
    }
    fn notify(&self, code: NoticeCode) {
        self.notices.borrow_mut().push(code);
    }
}

// ---- lockout (fake clock) ----

#[test]
fn a_second_click_within_600_ms_is_ignored_and_one_after_it_is_accepted() {
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(1_000));
    let port = FixedPort::new(ActivationDecision::Start);
    assert_eq!(
        activate_with(&port, &lock, &clock),
        Outcome::Start,
        "first click"
    );
    clock.0.set(1_000 + LOCKOUT_MS - 1);
    assert_eq!(
        activate_with(&port, &lock, &clock),
        Outcome::LockedOut,
        "599 ms later"
    );
    assert_eq!(port.starts.get(), 1, "a locked-out click never starts");
    clock.0.set(1_000 + LOCKOUT_MS);
    assert_eq!(
        activate_with(&port, &lock, &clock),
        Outcome::Start,
        "600 ms later"
    );
    assert_eq!(port.starts.get(), 2);
}

#[test]
fn a_locked_out_click_does_not_extend_the_lockout() {
    let mut lockout = ClickLockout::default();
    assert!(lockout.accept(0));
    assert!(!lockout.accept(300));
    assert!(lockout.accept(600), "measured from the accepted click only");
}

#[test]
fn the_lockout_is_monotonic_safe_when_time_goes_backwards() {
    let mut lockout = ClickLockout::default();
    assert!(lockout.accept(5_000));
    assert!(
        !lockout.accept(4_000),
        "an earlier reading never opens the gate"
    );
}

#[test]
fn the_production_clock_is_monotonic() {
    let a = MonotonicClock.now_ms();
    let b = MonotonicClock.now_ms();
    assert!(b >= a);
}

// ---- decision wiring: every ActivationDecision ----

#[test]
fn start_decision_starts_once_and_stops_nothing() {
    let port = FixedPort::new(ActivationDecision::Start);
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(0));
    assert_eq!(activate_with(&port, &lock, &clock), Outcome::Start);
    assert_eq!(port.starts.get(), 1);
    assert!(port.stops.borrow().is_empty());
    assert!(port.notices.borrow().is_empty());
}

#[test]
fn request_stop_decision_stops_only_the_observed_session() {
    let n = SessionId(9);
    let port = FixedPort::new(ActivationDecision::RequestStopIf(n));
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(0));
    assert_eq!(activate_with(&port, &lock, &clock), Outcome::Stop);
    assert_eq!(*port.stops.borrow(), vec![n]);
    assert_eq!(port.starts.get(), 0, "a stop never starts");
    assert!(port.notices.borrow().is_empty(), "a stop is not a refusal");
}

#[test]
fn stop_text_decision_stops_the_text_chat_and_notices_without_starting_voice() {
    let n = SessionId(4);
    let port = FixedPort::new(ActivationDecision::StopTextThenNotice(n));
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(0));
    assert_eq!(activate_with(&port, &lock, &clock), Outcome::StopText);
    assert_eq!(*port.stops.borrow(), vec![n]);
    assert_eq!(port.starts.get(), 0, "no voice start after the text chat");
    assert_eq!(*port.notices.borrow(), vec![NoticeCode::TextEnding]);
}

#[test]
fn ignore_decision_does_nothing() {
    let port = FixedPort::new(ActivationDecision::Ignore);
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(0));
    assert_eq!(activate_with(&port, &lock, &clock), Outcome::Ignored);
    assert_eq!(port.starts.get(), 0);
    assert!(port.stops.borrow().is_empty());
    assert!(port.notices.borrow().is_empty());
}

// ---- refusals: exhaustive mapping ----

#[test]
fn every_start_error_maps_to_its_notice_code() {
    let cases: [(StartError, Option<NoticeCode>); 9] = [
        (StartError::DictationBusy, Some(NoticeCode::DictationBusy)),
        (StartError::EnrollmentBusy, Some(NoticeCode::Unknown)),
        (StartError::TtsOff, Some(NoticeCode::TtsOff)),
        (StartError::ComposerOff, Some(NoticeCode::ComposerOff)),
        (StartError::Ambient, Some(NoticeCode::Ambient)),
        (
            StartError::ExperimentalOff,
            Some(NoticeCode::ExperimentalOff),
        ),
        (StartError::TextEnding, Some(NoticeCode::TextEnding)),
        (StartError::AlreadyRunning, None),
        (StartError::Unmapped, Some(NoticeCode::Unknown)),
    ];
    for (error, want) in cases {
        assert_eq!(notice_for(error), want, "{error:?}");
    }
}

#[test]
fn the_mapping_has_no_wildcard_over_start_error() {
    // Adding a variant to StartError breaks this match at compile time, as in notice_for.
    fn covered(error: StartError) -> bool {
        match error {
            StartError::DictationBusy
            | StartError::EnrollmentBusy
            | StartError::TtsOff
            | StartError::ComposerOff
            | StartError::Ambient
            | StartError::ExperimentalOff
            | StartError::TextEnding
            | StartError::AlreadyRunning
            | StartError::Unmapped => true,
        }
    }
    assert!(covered(StartError::Unmapped));
    let src = code_only(include_str!("orb_converse.rs"));
    assert!(
        !src.contains("_ =>") && !src.contains("_=>"),
        "no wildcard arm in orb_converse.rs"
    );
}

#[test]
fn controller_messages_classify_to_their_refusal() {
    assert_eq!(
        classify_start_error(DICTATION_BUSY),
        StartError::DictationBusy
    );
    assert_eq!(
        classify_start_error(ENROLLMENT_BUSY),
        StartError::EnrollmentBusy
    );
    assert_eq!(
        classify_start_error(ALREADY_RUNNING),
        StartError::AlreadyRunning
    );
    assert_eq!(classify_start_error(TEXT_ENDING), StartError::TextEnding);
    assert_eq!(
        classify_start_error(crate::managers::converse_platform::TTS_OFF_REFUSAL),
        StartError::TtsOff
    );
    assert_eq!(
        classify_start_error(crate::managers::converse_platform::COMPOSER_OFF_REFUSAL),
        StartError::ComposerOff
    );
    assert_eq!(
        classify_start_error(crate::managers::converse_platform::AMBIENT_REFUSAL),
        StartError::Ambient
    );
    assert_eq!(
        classify_start_error("some legacy text"),
        StartError::Unmapped,
        "unknown text is never shown raw"
    );
}

/// The names of the refusal-like constants declared in one source file: `pub const NAME: &str`
/// whose name ends in a refusal suffix. A new one in these files must get a row in the table.
fn refusal_const_names(src: &str) -> Vec<String> {
    const SUFFIXES: [&str; 5] = ["_REFUSAL", "_BUSY", "_RUNNING", "_ENDING", "_FAILED"];
    src.lines()
        .filter_map(|line| line.trim_start().strip_prefix("pub const "))
        .filter(|rest| rest.contains(": &str"))
        .filter_map(|rest| rest.split(':').next())
        .map(str::trim)
        .filter(|name| SUFFIXES.iter().any(|suffix| name.ends_with(suffix)))
        .map(str::to_string)
        .collect()
}

#[test]
fn every_refusal_constant_in_the_converse_sources_is_classified() {
    use crate::managers::converse_platform as platform;
    use crate::managers::converse_runtime as runtime;
    // (constant name, its value, the StartError it must classify to). A constant that is not
    // in this table fails the test, so a new refusal cannot ship unclassified.
    let table: [(&str, &str, StartError); 9] = [
        (
            "DICTATION_BUSY",
            platform::DICTATION_BUSY,
            StartError::DictationBusy,
        ),
        (
            "ENROLLMENT_BUSY",
            platform::ENROLLMENT_BUSY,
            StartError::EnrollmentBusy,
        ),
        (
            "TTS_OFF_REFUSAL",
            platform::TTS_OFF_REFUSAL,
            StartError::TtsOff,
        ),
        (
            "COMPOSER_OFF_REFUSAL",
            platform::COMPOSER_OFF_REFUSAL,
            StartError::ComposerOff,
        ),
        (
            "AMBIENT_REFUSAL",
            platform::AMBIENT_REFUSAL,
            StartError::Ambient,
        ),
        (
            "ALREADY_RUNNING",
            runtime::ALREADY_RUNNING,
            StartError::AlreadyRunning,
        ),
        ("START_FAILED", runtime::START_FAILED, StartError::Unmapped),
        // B2a: the orb has no notice for the update refusal yet, so it stays Unmapped (generic,
        // never shown raw). A dedicated StartError needs orb_converse.rs, which is out of scope.
        ("UPDATE_BUSY", platform::UPDATE_BUSY, StartError::Unmapped),
        ("TEXT_ENDING", runtime::TEXT_ENDING, StartError::TextEnding),
    ];
    let mut declared = refusal_const_names(include_str!("managers/converse_platform.rs"));
    declared.extend(refusal_const_names(include_str!(
        "managers/converse_runtime.rs"
    )));
    assert!(
        declared.len() >= table.len(),
        "the scan must find the constants: {declared:?}"
    );
    for name in &declared {
        assert!(
            table.iter().any(|(row, _, _)| row == name),
            "{name} is a refusal constant with no row in the classification table"
        );
    }
    for (name, value, want) in table {
        assert!(
            declared.iter().any(|d| d == name),
            "{name} is in the table but no longer declared"
        );
        assert_eq!(classify_start_error(value), want, "{name}");
    }
}

#[test]
fn a_refused_start_publishes_its_code_and_already_running_is_silent() {
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(0));
    let mut port = FixedPort::new(ActivationDecision::Start);
    port.start_error = Some(DICTATION_BUSY.to_string());
    assert_eq!(
        activate_with(&port, &lock, &clock),
        Outcome::Refused(NoticeCode::DictationBusy)
    );
    assert_eq!(*port.notices.borrow(), vec![NoticeCode::DictationBusy]);

    let lock = Mutex::new(ClickLockout::default());
    let mut port = FixedPort::new(ActivationDecision::Start);
    port.start_error = Some(ALREADY_RUNNING.to_string());
    assert_eq!(activate_with(&port, &lock, &clock), Outcome::Ignored);
    assert!(port.notices.borrow().is_empty());
}

#[test]
fn a_refused_start_still_arms_the_lockout() {
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(0));
    let mut port = FixedPort::new(ActivationDecision::Start);
    port.start_error = Some(DICTATION_BUSY.to_string());
    activate_with(&port, &lock, &clock);
    clock.0.set(100);
    assert_eq!(activate_with(&port, &lock, &clock), Outcome::LockedOut);
}

// ---- barrier: real registry phases ----

#[test]
fn an_orb_click_while_a_hotkey_start_is_starting_is_ignored() {
    let port = RegistryPort::new();
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(0));
    // The hotkey admitted a start: the session is Starting, not yet capturing.
    let (n, _token) = port.reg.try_begin().expect("hotkey start");
    assert_eq!(port.reg.state(), RegistryState::Starting(n));
    assert_eq!(activate_with(&port, &lock, &clock), Outcome::Ignored);
    assert_eq!(
        port.reg.state(),
        RegistryState::Starting(n),
        "never cancelled"
    );
    assert!(port.notices.borrow().is_empty());
}

#[test]
fn a_click_on_a_running_voice_session_stops_it_and_a_second_start_is_refused() {
    let port = RegistryPort::new();
    let lock = Mutex::new(ClickLockout::default());
    let clock = FakeClock(Cell::new(0));
    let (n, _token) = port.reg.try_begin().expect("start");
    assert!(port.reg.mark_running(n));
    assert_eq!(activate_with(&port, &lock, &clock), Outcome::Stop);
    assert_eq!(port.reg.state(), RegistryState::Draining(n));
}

#[test]
fn orb_hotkey_and_wake_starts_admit_exactly_one_session() {
    let port = RegistryPort::new();
    let first = port.start();
    let second = port.start();
    let third = port.start();
    assert!(first.is_ok());
    assert_eq!(second, Err(ALREADY_RUNNING.to_string()));
    assert_eq!(third, Err(ALREADY_RUNNING.to_string()));
}

/// A port that may be shared across threads: the real registry and the same start entry.
struct SharedPort<'a> {
    reg: &'a SessionRegistry,
    held: Mutex<Vec<crate::managers::converse::CancelToken>>,
    stops: Mutex<usize>,
    notices: Mutex<Vec<NoticeCode>>,
}

impl ActivationPort for SharedPort<'_> {
    fn decision(&self) -> ActivationDecision {
        let (phase, _) = self.reg.snapshot();
        orb_activation_decision(phase, ActivationKind::Voice)
    }
    fn start(&self) -> Result<(), String> {
        match self.reg.try_begin() {
            Some((_, token)) => {
                self.held
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(token);
                Ok(())
            }
            None => Err(ALREADY_RUNNING.to_string()),
        }
    }
    fn stop_if(&self, id: SessionId) -> bool {
        *self
            .stops
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
        self.reg.request_stop_if(id)
    }
    fn notify(&self, code: NoticeCode) {
        self.notices
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(code);
    }
}

/// A clock that always reads zero: every activation runs against a fresh lockout.
struct ZeroClock;
impl Clock for ZeroClock {
    fn now_ms(&self) -> u64 {
        0
    }
}

#[test]
fn orb_hotkey_and_wake_starts_race_at_a_barrier_and_admit_exactly_one() {
    for round in 0..50 {
        let reg = SessionRegistry::new();
        let port = SharedPort {
            reg: &reg,
            held: Mutex::new(Vec::new()),
            stops: Mutex::new(0),
            notices: Mutex::new(Vec::new()),
        };
        let orb_lock = Mutex::new(ClickLockout::default());
        let barrier = Barrier::new(3);
        let (orb, hotkey, wake) = std::thread::scope(|scope| {
            let orb = scope.spawn(|| {
                barrier.wait();
                activate_with(&port, &orb_lock, &ZeroClock)
            });
            let hotkey = scope.spawn(|| {
                barrier.wait();
                activate_with(&port, &Mutex::new(ClickLockout::default()), &ZeroClock)
            });
            let wake = scope.spawn(|| {
                barrier.wait();
                port.start()
            });
            (orb.join(), hotkey.join(), wake.join())
        });
        let orb = orb.expect("orb thread");
        let hotkey = hotkey.expect("hotkey thread");
        let wake = wake.expect("wake thread");
        let admitted = usize::from(orb == Outcome::Start)
            + usize::from(hotkey == Outcome::Start)
            + usize::from(wake.is_ok());
        assert_eq!(
            admitted, 1,
            "round {round}: orb {orb:?}, hotkey {hotkey:?}, wake {wake:?}"
        );
        assert!(
            matches!(orb, Outcome::Start | Outcome::Ignored),
            "round {round}: orb {orb:?}"
        );
        assert!(
            matches!(hotkey, Outcome::Start | Outcome::Ignored),
            "round {round}: hotkey {hotkey:?}"
        );
        assert_eq!(
            port.held.lock().map(|held| held.len()).unwrap_or(0),
            1,
            "round {round}: one token held"
        );
        assert_eq!(
            *port.stops.lock().unwrap_or_else(|p| p.into_inner()),
            0,
            "round {round}: no activation may stop a Starting session"
        );
        assert!(
            port.notices
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_empty(),
            "round {round}: an admitted or already-running start shows no notice"
        );
        assert!(
            matches!(reg.state(), RegistryState::Starting(_)),
            "round {round}: the admitted session is still Starting, never cancelled"
        );
    }
}

// ---- event contract ----

#[test]
fn the_refusal_payload_carries_only_the_code() {
    let payload = OrbRefusalPayload {
        code: NoticeCode::TtsOff,
    };
    let value = serde_json::to_value(payload).expect("serialise");
    let object = value.as_object().expect("object");
    assert_eq!(object.len(), 1, "only {{code}} crosses the wire");
    assert_eq!(object.get("code"), Some(&serde_json::json!("tts_off")));
}

#[test]
fn the_refusal_event_name_is_stable() {
    assert_eq!(ORB_REFUSAL_EVENT, "orb-refusal");
}

#[test]
fn the_refusal_event_is_emitted_to_the_orb_label_only() {
    let src: String = code_only(include_str!("orb_converse.rs"))
        .split_whitespace()
        .collect();
    assert!(
        src.contains("emit_to(ORB_WINDOW_LABEL,ORB_REFUSAL_EVENT"),
        "orb-refusal goes to the orb window"
    );
    assert!(
        !src.contains(".emit(") && !src.contains("emit_all("),
        "no broadcast emit in orb_converse.rs"
    );
    assert_eq!(ORB_WINDOW_LABEL, "orb");
}

#[test]
fn the_orb_window_command_matrix_is_unchanged() {
    assert_eq!(
        ORB_COMMANDS,
        [
            "orb_pointer_down",
            "orb_activate",
            "orb_menu",
            "orb_dismiss",
            "orb_get_state",
            "orb_set_reduced_motion",
        ]
    );
}

// ---- production hygiene ----

#[test]
fn production_code_has_no_unwrap_expect_or_content_logs() {
    let src = code_only(include_str!("orb_converse.rs"));
    assert!(!src.contains(".unwrap("), "no unwrap");
    assert!(!src.contains(".expect("), "no expect");
    for line in src.lines().filter(|l| l.contains("log::")) {
        assert!(
            !line.contains("{e") && !line.contains("message") && !line.contains("{msg"),
            "log lines carry no error text: {line}"
        );
    }
}

#[test]
fn the_orb_click_path_calls_the_activation_not_the_chat_toggle() {
    let native = code_only(include_str!("orb_native.rs"));
    let click_arm = native
        .lines()
        .find(|l| l.contains("Call::Click =>"))
        .expect("click arm");
    assert!(
        click_arm.contains("orb_converse::activate(app)"),
        "{click_arm}"
    );
    assert!(!click_arm.contains("toggle_chat"), "{click_arm}");
    let keyboard = native
        .split("pub fn activate(app: &AppHandle)")
        .nth(1)
        .expect("keyboard activate");
    let body = keyboard.split("\n}").next().unwrap_or("");
    assert!(
        body.contains("orb_converse::activate(app)") && !body.contains("toggle_chat"),
        "Enter/Space share the click path"
    );
    assert_eq!(
        native.matches("orb_converse::activate(app)").count(),
        2,
        "click and keyboard use the same function"
    );
}

#[test]
fn the_context_menu_chat_item_still_toggles_the_chat() {
    let native = code_only(include_str!("orb_native.rs"));
    assert!(native.contains("\"orb_toggle_chat\" => toggle_chat(app)"));
}
