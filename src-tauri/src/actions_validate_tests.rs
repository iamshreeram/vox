//! RED tests for M3 S3c-1 (DESIGN Rev 5, R2-2 and R2-12): `validate_model_command` and
//! `run_validated_command` must match the legacy `model_command_core` path exactly.
//! Phase A: they compile against the todo!() stubs and fail at runtime.
//! Included from actions.rs. Do not edit to make code pass.
#![allow(dead_code)]

use super::*;
use crate::managers::command_router::{CommandAction, RouteDecision};
use crate::managers::media_control::MediaHookResult;
use crate::managers::voice_common::{HookEvent, VoiceHookOutcome};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;

/// Router that returns a fixed decision and counts calls.
struct FixedRouter {
    decision: RouteDecision,
    calls: Cell<u32>,
}

impl PhraseRouter for FixedRouter {
    fn route_phrase(&self, _text: &str) -> RouteDecision {
        self.calls.set(self.calls.get() + 1);
        self.decision.clone()
    }
}

/// Records every executed action; fails when `fail` is set.
#[derive(Default)]
struct RecExec {
    calls: RefCell<Vec<CommandAction>>,
    fail: bool,
}

impl ActionExecutor for RecExec {
    fn execute_action(&self, action: &CommandAction) -> Result<String, String> {
        self.calls.borrow_mut().push(action.clone());
        if self.fail {
            Err("opener failed".into())
        } else {
            Ok("ok".into())
        }
    }
}

/// Media hook: canned result, counts consultations.
struct MediaFake {
    result: Option<MediaHookResult>,
    calls: Cell<u32>,
}

impl MediaHook for MediaFake {
    fn consult(&self, _phrase: &str) -> Option<MediaHookResult> {
        self.calls.set(self.calls.get() + 1);
        self.result.clone()
    }
}

fn no_media() -> MediaFake {
    MediaFake {
        result: None,
        calls: Cell::new(0),
    }
}

fn media_handled() -> MediaFake {
    MediaFake {
        result: Some(MediaHookResult {
            outcome: VoiceHookOutcome::HANDLED,
            event: HookEvent::Executed("Paused music".into()),
        }),
        calls: Cell::new(0),
    }
}

fn matched(action: CommandAction) -> RouteDecision {
    RouteDecision::Matched { action }
}

fn open_app(name: &str) -> CommandAction {
    CommandAction::OpenApp {
        name: name.into(),
        resolved_path: PathBuf::from(format!("/Applications/{name}.app")),
    }
}

/// Legacy outcome for one phrase, using the same inputs as the validate path.
fn legacy(
    phrase: &str,
    voice: bool,
    media: &dyn MediaHook,
    decision: RouteDecision,
    exec: &RecExec,
) -> PhraseOutcome {
    let router = FixedRouter {
        decision,
        calls: Cell::new(0),
    };
    model_command_core(phrase, voice, media, &router, exec)
}

#[test]
fn validate_accepts_open_app_exactly_when_legacy_would_execute() {
    let router = FixedRouter {
        decision: matched(open_app("Notes")),
        calls: Cell::new(0),
    };
    let got = validate_model_command("open notes", true, &no_media(), &router);
    assert_eq!(got, Some(open_app("Notes")));
}

#[test]
fn validate_accepts_https_and_http_urls_case_insensitively() {
    for url in ["https://example.com/a", "HTTP://example.com", "Https://x.y"] {
        let router = FixedRouter {
            decision: matched(CommandAction::OpenUrl { url: url.into() }),
            calls: Cell::new(0),
        };
        let got = validate_model_command("open it", true, &no_media(), &router);
        assert_eq!(
            got,
            Some(CommandAction::OpenUrl { url: url.into() }),
            "{url}"
        );
    }
}

#[test]
fn validate_refuses_each_rejected_action_like_the_legacy_path() {
    let rejected = [
        CommandAction::OpenPath {
            path: PathBuf::from("/etc/hosts"),
        },
        CommandAction::OpenUrl {
            url: "file:///etc/passwd".into(),
        },
        CommandAction::OpenUrl {
            url: " https://x.com".into(),
        },
        CommandAction::OpenUrl {
            url: "https://".into(),
        },
        CommandAction::OpenUrl {
            url: "javascript:alert(1)".into(),
        },
    ];
    for action in rejected {
        let router = FixedRouter {
            decision: matched(action.clone()),
            calls: Cell::new(0),
        };
        let got = validate_model_command("do it", true, &no_media(), &router);
        assert_eq!(got, None, "must refuse {action:?}");
        let exec = RecExec::default();
        assert_eq!(
            legacy("do it", true, &no_media(), matched(action), &exec),
            PhraseOutcome::Refused
        );
        assert!(exec.calls.borrow().is_empty());
    }
}

#[test]
fn validate_refuses_no_match_and_voice_commands_off_like_legacy() {
    let router = FixedRouter {
        decision: RouteDecision::NoMatch,
        calls: Cell::new(0),
    };
    assert_eq!(
        validate_model_command("hello", true, &no_media(), &router),
        None
    );
    let exec = RecExec::default();
    assert_eq!(
        legacy("hello", true, &no_media(), RouteDecision::NoMatch, &exec),
        PhraseOutcome::NoMatch
    );

    let router = FixedRouter {
        decision: matched(open_app("Notes")),
        calls: Cell::new(0),
    };
    assert_eq!(
        validate_model_command("open notes", false, &no_media(), &router),
        None
    );
    let exec = RecExec::default();
    assert_eq!(
        legacy(
            "open notes",
            false,
            &no_media(),
            matched(open_app("Notes")),
            &exec
        ),
        PhraseOutcome::NoMatch
    );
    assert!(exec.calls.borrow().is_empty());
}

#[test]
fn validate_never_executes_and_never_consults_the_router_when_voice_is_off() {
    let media = media_handled();
    let router = FixedRouter {
        decision: matched(open_app("Notes")),
        calls: Cell::new(0),
    };
    let got = validate_model_command("open notes", false, &media, &router);
    assert_eq!(got, None);
    assert_eq!(
        media.calls.get(),
        0,
        "media hook must not run with voice off"
    );
    assert_eq!(router.calls.get(), 0, "router must not run with voice off");
}

#[test]
fn validate_defers_to_the_media_hook_so_the_model_path_stays_media_only() {
    // Legacy: a media hook that handles the phrase wins; the router is not consulted.
    let media = media_handled();
    let router = FixedRouter {
        decision: matched(open_app("Notes")),
        calls: Cell::new(0),
    };
    let got = validate_model_command("pause", true, &media, &router);
    assert_eq!(got, None);
    assert_eq!(router.calls.get(), 0);
    let exec = RecExec::default();
    assert_eq!(
        legacy(
            "pause",
            true,
            &media_handled(),
            matched(open_app("Notes")),
            &exec
        ),
        PhraseOutcome::Done(crate::managers::tts::DONE_PHRASE.to_string())
    );
    assert!(exec.calls.borrow().is_empty());
}

#[test]
fn run_validated_command_executes_once_and_reports_success_like_legacy() {
    let exec = RecExec::default();
    let ok = run_validated_command(&open_app("Notes"), &exec);
    assert!(ok);
    assert_eq!(exec.calls.borrow().as_slice(), &[open_app("Notes")]);

    let failing = RecExec {
        calls: RefCell::new(vec![]),
        fail: true,
    };
    assert!(!run_validated_command(&open_app("Notes"), &failing));
    assert_eq!(failing.calls.borrow().len(), 1);
}

#[test]
fn validate_then_run_matches_legacy_outcome_over_a_corpus() {
    let corpus = [
        (true, matched(open_app("Notes"))),
        (
            true,
            matched(CommandAction::OpenUrl {
                url: "https://a.b".into(),
            }),
        ),
        (
            true,
            matched(CommandAction::OpenUrl {
                url: "ftp://a.b".into(),
            }),
        ),
        (
            true,
            matched(CommandAction::OpenPath {
                path: PathBuf::from("/tmp"),
            }),
        ),
        (true, RouteDecision::NoMatch),
        (false, matched(open_app("Notes"))),
    ];
    for (voice, decision) in corpus {
        let legacy_exec = RecExec::default();
        let legacy_outcome = legacy("phrase", voice, &no_media(), decision.clone(), &legacy_exec);

        let router = FixedRouter {
            decision,
            calls: Cell::new(0),
        };
        let validated = validate_model_command("phrase", voice, &no_media(), &router);
        let run_exec = RecExec::default();
        let new_success = match &validated {
            Some(action) => run_validated_command(action, &run_exec),
            None => false,
        };
        let legacy_success = matches!(legacy_outcome, PhraseOutcome::Done(_));
        assert_eq!(
            validated.is_some(),
            legacy_outcome != PhraseOutcome::NoMatch && legacy_outcome != PhraseOutcome::Refused,
            "accept parity for voice={voice}"
        );
        assert_eq!(
            new_success, legacy_success,
            "success parity for voice={voice}"
        );
        assert_eq!(
            run_exec.calls.borrow().len(),
            legacy_exec.calls.borrow().len(),
            "execution count parity for voice={voice}"
        );
    }
}
