//! Pure activation decision for the conversation toggle (orb click, hotkey, tray).
//! DESIGN A-C1b (Revision 3): over the registry phase and the kind of the observed session,
//! decide whether an activation starts a session, asks a specific session to stop, or does
//! nothing. A session that is still Starting is never cancelled by an activation.
//! The orb call site comes in a later step; this module has no Tauri types.

use crate::managers::converse::{RegistryState, SessionId};

/// The kind of the session an activation observes (Voice when no session runs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationKind {
    Voice,
    Text,
}

/// What an activation does. Stop decisions carry the observed id, so a stop can only land on
/// the session that was observed (compare-and-stop, `SessionRegistry::request_stop_if`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationDecision {
    Start,
    RequestStopIf(SessionId),
    StopTextThenNotice(SessionId),
    Ignore,
}

/// Decides one activation from the observed phase and session kind.
/// Idle -> Start; Starting -> Ignore; Running voice -> stop if the same id; Running text ->
/// stop the text chat and notice; Draining -> Ignore.
pub fn orb_activation_decision(phase: RegistryState, kind: ActivationKind) -> ActivationDecision {
    match (phase, kind) {
        (RegistryState::Idle, _) => ActivationDecision::Start,
        (RegistryState::Starting(_), _) | (RegistryState::Draining(_), _) => {
            ActivationDecision::Ignore
        }
        (RegistryState::Running(id), ActivationKind::Voice) => {
            ActivationDecision::RequestStopIf(id)
        }
        (RegistryState::Running(id), ActivationKind::Text) => {
            ActivationDecision::StopTextThenNotice(id)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::converse::SessionRegistry;

    #[test]
    fn phase_table_over_all_phases_and_kinds() {
        let n = SessionId(7);
        let cases = [
            (
                RegistryState::Idle,
                ActivationKind::Voice,
                ActivationDecision::Start,
            ),
            (
                RegistryState::Idle,
                ActivationKind::Text,
                ActivationDecision::Start,
            ),
            (
                RegistryState::Starting(n),
                ActivationKind::Voice,
                ActivationDecision::Ignore,
            ),
            (
                RegistryState::Starting(n),
                ActivationKind::Text,
                ActivationDecision::Ignore,
            ),
            (
                RegistryState::Running(n),
                ActivationKind::Voice,
                ActivationDecision::RequestStopIf(n),
            ),
            (
                RegistryState::Running(n),
                ActivationKind::Text,
                ActivationDecision::StopTextThenNotice(n),
            ),
            (
                RegistryState::Draining(n),
                ActivationKind::Voice,
                ActivationDecision::Ignore,
            ),
            (
                RegistryState::Draining(n),
                ActivationKind::Text,
                ActivationDecision::Ignore,
            ),
        ];
        for (phase, kind, want) in cases {
            assert_eq!(
                orb_activation_decision(phase, kind),
                want,
                "phase {phase:?} kind {kind:?}"
            );
        }
    }

    #[test]
    fn starting_phase_ignores_stop_requests_and_keeps_the_session() {
        let reg = SessionRegistry::new();
        let (n, token) = reg.try_begin().expect("begin");
        assert_eq!(reg.state(), RegistryState::Starting(n));
        assert!(reg.is_active());
        assert!(
            !reg.request_stop_if(n),
            "compare-and-stop is ignored while Starting"
        );
        assert!(
            !*token.session_ref(),
            "a starting session is never cancelled by an activation"
        );
        assert_eq!(reg.state(), RegistryState::Starting(n));
        assert!(
            reg.mark_running(n),
            "capture ready flips Starting to Running"
        );
        assert!(
            !reg.mark_running(n),
            "mark_running only applies to Starting"
        );
        assert_eq!(reg.state(), RegistryState::Running(n));
    }

    #[test]
    fn replaced_session_is_never_stopped_by_an_observed_old_id() {
        let reg = SessionRegistry::new();
        let (n, _t) = reg.try_begin().expect("begin N");
        assert!(reg.mark_running(n));
        let (phase, observed) = reg.snapshot();
        assert_eq!(phase, RegistryState::Running(n));
        assert_eq!(observed, n);
        assert_eq!(
            orb_activation_decision(phase, ActivationKind::Voice),
            ActivationDecision::RequestStopIf(observed)
        );
        // The observer is paused here; meanwhile the session is replaced by N+1.
        assert!(reg.request_stop_if(n));
        assert!(reg.finish(n));
        let (m, token) = reg.try_begin().expect("begin N+1");
        assert!(m > n, "ids are monotonically increasing");
        assert!(reg.mark_running(m));
        assert!(!reg.request_stop_if(n), "stale id must not stop N+1");
        assert_eq!(reg.state(), RegistryState::Running(m));
        assert!(!*token.session_ref(), "N+1 must not be cancelled");
        assert!(reg.request_stop_if(m), "the current id still stops");
    }

    #[test]
    fn start_failure_returns_to_idle_and_a_later_start_works() {
        let reg = SessionRegistry::new();
        let (n, _t) = reg.try_begin().expect("begin");
        assert!(reg.finish(n), "a failed start frees the registry");
        assert_eq!(reg.state(), RegistryState::Idle);
        assert_eq!(
            orb_activation_decision(reg.state(), ActivationKind::Voice),
            ActivationDecision::Start
        );
        let (m, _t2) = reg.try_begin().expect("later start");
        assert!(m > n);
    }

    #[test]
    fn snapshot_reports_the_last_issued_id_when_idle() {
        let reg = SessionRegistry::new();
        assert_eq!(reg.snapshot(), (RegistryState::Idle, SessionId(0)));
        let (n, _t) = reg.try_begin().expect("begin");
        assert!(reg.finish(n));
        assert_eq!(reg.snapshot(), (RegistryState::Idle, n));
    }

    #[test]
    fn legacy_request_stop_resolves_the_current_id_under_one_lock() {
        let reg = SessionRegistry::new();
        let (n, token) = reg.try_begin().expect("begin");
        assert!(reg.mark_running(n));
        assert!(reg.request_stop(), "legacy stop resolves the running id");
        assert!(*token.session_ref());
        assert_eq!(reg.state(), RegistryState::Draining(n));
        assert!(!reg.request_stop_if(SessionId(n.0 + 1)));
    }
}
