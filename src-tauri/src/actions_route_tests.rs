//! RED tests for DESIGN.md Revision 3, section M1 (dictation hand-off no longer
//! double-pastes). They exercise `apply_route_decision` with counting fakes.
//! Registered from actions.rs. Do not edit to make code pass.

use super::{apply_route_decision, Handoff, RouteEffect};
use crate::managers::command_router::{CommandAction, RouteDecision};
use std::cell::Cell;

fn matched() -> RouteDecision {
    RouteDecision::Matched {
        action: CommandAction::OpenUrl {
            url: "https://example.com".to_string(),
        },
    }
}

#[test]
fn route_voice_off_calls_nothing() {
    let decides = Cell::new(0usize);
    let executes = Cell::new(0usize);
    let escalates = Cell::new(0usize);
    let effect = apply_route_decision(
        false,
        true,
        &mut || {
            decides.set(decides.get() + 1);
            matched()
        },
        &mut |_: &CommandAction| {
            executes.set(executes.get() + 1);
            Ok("done".to_string())
        },
        &mut || {
            escalates.set(escalates.get() + 1);
            Handoff::Queued
        },
    );
    assert_eq!(effect, RouteEffect::default());
    assert_eq!(decides.get(), 0, "decide must not run with voice off");
    assert_eq!(executes.get(), 0, "execute must not run with voice off");
    assert_eq!(escalates.get(), 0, "escalate must not run with voice off");
}

#[test]
fn route_matched_ok_skips_paste() {
    let executes = Cell::new(0usize);
    let escalates = Cell::new(0usize);
    let effect = apply_route_decision(
        true,
        true,
        &mut matched,
        &mut |_: &CommandAction| {
            executes.set(executes.get() + 1);
            Ok("opened".to_string())
        },
        &mut || {
            escalates.set(escalates.get() + 1);
            Handoff::Queued
        },
    );
    assert_eq!(
        effect,
        RouteEffect {
            skip_paste: true,
            executed: Some("opened".to_string()),
            exec_error: None,
            handoff: None,
        }
    );
    assert_eq!(executes.get(), 1);
    assert_eq!(escalates.get(), 0, "a matched command never escalates");
}

#[test]
fn route_matched_error_still_pastes() {
    let effect = apply_route_decision(
        true,
        true,
        &mut matched,
        &mut |_: &CommandAction| Err("boom".to_string()),
        &mut || Handoff::Queued,
    );
    assert_eq!(
        effect,
        RouteEffect {
            skip_paste: false,
            executed: None,
            exec_error: Some("boom".to_string()),
            handoff: None,
        }
    );
}

#[test]
fn route_nomatch_queued_skips_paste() {
    let executes = Cell::new(0usize);
    let effect = apply_route_decision(
        true,
        true,
        &mut || RouteDecision::NoMatch,
        &mut |_: &CommandAction| {
            executes.set(executes.get() + 1);
            Ok(String::new())
        },
        &mut || Handoff::Queued,
    );
    assert_eq!(
        effect,
        RouteEffect {
            skip_paste: true,
            executed: None,
            exec_error: None,
            handoff: Some(Handoff::Queued),
        }
    );
    assert_eq!(executes.get(), 0, "NoMatch never executes an action");
}

#[test]
fn route_nomatch_rejected_still_pastes() {
    let effect = apply_route_decision(
        true,
        true,
        &mut || RouteDecision::NoMatch,
        &mut |_: &CommandAction| Ok(String::new()),
        &mut || Handoff::Rejected,
    );
    assert_eq!(
        effect,
        RouteEffect {
            skip_paste: false,
            executed: None,
            exec_error: None,
            handoff: Some(Handoff::Rejected),
        }
    );
}

#[test]
fn route_nomatch_acp_disabled_does_not_escalate() {
    let escalates = Cell::new(0usize);
    let executes = Cell::new(0usize);
    let effect = apply_route_decision(
        true,
        false,
        &mut || RouteDecision::NoMatch,
        &mut |_: &CommandAction| {
            executes.set(executes.get() + 1);
            Ok(String::new())
        },
        &mut || {
            escalates.set(escalates.get() + 1);
            Handoff::Queued
        },
    );
    assert_eq!(effect, RouteEffect::default());
    assert_eq!(escalates.get(), 0, "ACP disabled => no escalation");
    assert_eq!(executes.get(), 0);
}

#[test]
fn route_escalate_called_exactly_once() {
    let escalates = Cell::new(0usize);
    let _ = apply_route_decision(
        true,
        true,
        &mut || RouteDecision::NoMatch,
        &mut |_: &CommandAction| Ok(String::new()),
        &mut || {
            escalates.set(escalates.get() + 1);
            Handoff::Queued
        },
    );
    assert_eq!(escalates.get(), 1, "NoMatch + ACP enabled escalates once");
}
