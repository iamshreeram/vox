//! Pure conversation-overlay logic: maps session state labels to overlay
//! actions, gates readiness per listen, orders show/hide through an injected
//! main-thread executor, and decides where Cancel is routed.
//! Contract: DESIGN.md Revision 5 (sections 0R5, 0R4, 0R3, 0).
//!
//! INERT STUB: type definitions are final; every function body is `todo!()`
//! until the implementation step lands. The tests below are the spec.

#![allow(dead_code, unused_variables)]

use std::sync::{Arc, Mutex};

/// What the overlay should do for one conversation state label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayAction {
    /// Show the overlay with the given look.
    Show(ShowKind),
    /// Hide the overlay.
    Hide,
    /// Leave the overlay untouched.
    Keep,
}

/// Overlay look used while a conversation session is in a given state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowKind {
    /// Recording look; the pill is up from the first listen until idle.
    Listening,
    /// Transcribing look while Vox thinks.
    Thinking,
    /// Animated-dots look while Vox speaks (no mic bars).
    Speaking,
    /// Processing look while Vox acts.
    Acting,
}

/// Maps a `converse-state` label to an overlay action.
///
/// Matching is exact and case-sensitive: `listening`, `thinking`, `speaking`,
/// `acting` show their look, `idle` hides, and any other text keeps the
/// current overlay state.
pub fn overlay_action(label: &str) -> OverlayAction {
    todo!("overlay_action: not yet implemented")
}

/// Which handler receives the overlay Cancel button press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelRoute {
    /// Stop the active conversation session.
    Conversation,
    /// Run the dictation cancel.
    Dictation,
}

/// Routes the overlay Cancel button: a conversation session that is active
/// takes the press, otherwise dictation cancel runs.
pub fn cancel_route(conversation_session_active: bool) -> CancelRoute {
    todo!("cancel_route: not yet implemented")
}

/// Identifies one armed readiness request (one listen).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadyToken(u64);

/// Per-listen readiness gate shared between the recorder port and the
/// ready-delivery path. Only the most recently armed token can fire, and only
/// once; `invalidate` revokes it.
#[derive(Debug)]
pub struct ReadyGate {
    state: Mutex<GateState>,
}

#[derive(Debug, Default)]
struct GateState {
    next: u64,
    current: Option<u64>,
}

impl ReadyGate {
    /// Creates a gate with no armed token.
    pub fn new() -> Self {
        todo!("ReadyGate::new: not yet implemented")
    }

    /// Arms a fresh token, replacing any previously armed token.
    pub fn arm(&self) -> ReadyToken {
        todo!("ReadyGate::arm: not yet implemented")
    }

    /// Returns `true` only if `token` is the current, not-yet-fired,
    /// not-invalidated token, and consumes it.
    pub fn fire(&self, token: ReadyToken) -> bool {
        todo!("ReadyGate::fire: not yet implemented")
    }

    /// Revokes the current token, if any.
    pub fn invalidate(&self) {
        todo!("ReadyGate::invalidate: not yet implemented")
    }
}

/// Sink for overlay operations. The real implementation forwards to the
/// overlay window; tests record the calls.
pub trait OverlaySink: Send + Sync {
    /// Shows the overlay with the given look.
    fn show(&self, kind: ShowKind);
    /// Emits the recording-ready signal for the current listen.
    fn ready(&self);
    /// Hides the overlay.
    fn hide(&self);
}

/// Ordered executor for overlay work. The real implementation hops each job
/// to the main thread in FIFO order; tests hold and drain the queue.
pub trait OverlayExecutor: Send + Sync {
    /// Queues `job` to run after every previously submitted job.
    fn submit(&self, job: Box<dyn FnOnce() + Send>);
}

/// Turns session state labels and readiness deliveries into ordered overlay
/// operations.
pub struct OverlayDispatcher {
    exec: Arc<dyn OverlayExecutor>,
    sink: Arc<dyn OverlaySink>,
    gate: Arc<ReadyGate>,
}

impl OverlayDispatcher {
    /// Builds a dispatcher over the given executor, sink and readiness gate.
    pub fn new(
        exec: Arc<dyn OverlayExecutor>,
        sink: Arc<dyn OverlaySink>,
        gate: Arc<ReadyGate>,
    ) -> Self {
        todo!("OverlayDispatcher::new: not yet implemented")
    }

    /// Applies one state label. `Show` queues a show job. `Hide` invalidates
    /// the gate immediately on the calling thread, then queues a hide job.
    /// `Keep` does nothing.
    pub fn apply_state(&self, label: &str) {
        todo!("OverlayDispatcher::apply_state: not yet implemented")
    }

    /// Queues a ready delivery. When the job runs it calls `gate.fire(token)`
    /// and calls `sink.ready()` only if that returned `true`.
    pub fn deliver_ready(&self, token: ReadyToken) {
        todo!("OverlayDispatcher::deliver_ready: not yet implemented")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::converse::SessionState;
    use crate::managers::converse_runtime::state_label;
    use std::sync::Barrier;
    use std::thread;

    /// Every `SessionState` variant. `exhaustive_guard` fails to compile when a
    /// variant is added to the enum without a matching arm, so this list
    /// cannot silently drift from the enum.
    const ALL_STATES: [SessionState; 5] = [
        SessionState::Listening,
        SessionState::Thinking,
        SessionState::Speaking,
        SessionState::Acting,
        SessionState::Idle,
    ];

    fn exhaustive_guard(state: SessionState) {
        match state {
            SessionState::Listening
            | SessionState::Thinking
            | SessionState::Speaking
            | SessionState::Acting
            | SessionState::Idle => {}
        }
    }

    fn join_or_resume<T>(handle: thread::JoinHandle<T>) -> T {
        match handle.join() {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Op {
        Show(ShowKind),
        Ready,
        Hide,
    }

    #[derive(Default)]
    struct RecordingSink {
        ops: Mutex<Vec<Op>>,
    }

    impl RecordingSink {
        fn ops(&self) -> Vec<Op> {
            self.ops.lock().unwrap().clone()
        }
    }

    impl OverlaySink for RecordingSink {
        fn show(&self, kind: ShowKind) {
            self.ops.lock().unwrap().push(Op::Show(kind));
        }
        fn ready(&self) {
            self.ops.lock().unwrap().push(Op::Ready);
        }
        fn hide(&self) {
            self.ops.lock().unwrap().push(Op::Hide);
        }
    }

    /// Queues jobs without running them until `drain` is called.
    #[derive(Default)]
    struct HeldExecutor {
        jobs: Mutex<Vec<Box<dyn FnOnce() + Send>>>,
    }

    impl HeldExecutor {
        fn pending(&self) -> usize {
            self.jobs.lock().unwrap().len()
        }

        /// Runs queued jobs in FIFO order, including jobs queued while draining.
        fn drain(&self) {
            loop {
                let job = {
                    let mut queue = self.jobs.lock().unwrap();
                    if queue.is_empty() {
                        break;
                    }
                    queue.remove(0)
                };
                job();
            }
        }
    }

    impl OverlayExecutor for HeldExecutor {
        fn submit(&self, job: Box<dyn FnOnce() + Send>) {
            self.jobs.lock().unwrap().push(job);
        }
    }

    type Rig = (
        Arc<HeldExecutor>,
        Arc<RecordingSink>,
        Arc<ReadyGate>,
        OverlayDispatcher,
    );

    fn rig() -> Rig {
        let exec = Arc::new(HeldExecutor::default());
        let sink = Arc::new(RecordingSink::default());
        let gate = Arc::new(ReadyGate::new());
        let dispatcher = OverlayDispatcher::new(exec.clone(), sink.clone(), gate.clone());
        (exec, sink, gate, dispatcher)
    }

    // O1: the five state labels map to their actions.
    #[test]
    fn o1_overlay_action_table_for_the_five_labels() {
        assert_eq!(
            overlay_action("listening"),
            OverlayAction::Show(ShowKind::Listening)
        );
        assert_eq!(
            overlay_action("thinking"),
            OverlayAction::Show(ShowKind::Thinking)
        );
        assert_eq!(
            overlay_action("speaking"),
            OverlayAction::Show(ShowKind::Speaking)
        );
        assert_eq!(
            overlay_action("acting"),
            OverlayAction::Show(ShowKind::Acting)
        );
        assert_eq!(overlay_action("idle"), OverlayAction::Hide);
    }

    // O2: anything else keeps the current overlay state.
    #[test]
    fn o2_unknown_empty_uppercase_and_whitespace_labels_keep() {
        for label in [
            "",
            "bogus",
            "Listening",
            "THINKING",
            "Idle",
            " listening",
            "listening ",
            "idle\n",
            "speaking_",
        ] {
            assert_eq!(
                overlay_action(label),
                OverlayAction::Keep,
                "label {label:?}"
            );
        }
    }

    // O3: every label produced by state_label for every SessionState variant
    // maps to a non-Keep action.
    #[test]
    fn o3_every_session_state_label_maps_to_non_keep() {
        for state in ALL_STATES {
            exhaustive_guard(state);
            let label = state_label(state);
            assert_ne!(
                overlay_action(label),
                OverlayAction::Keep,
                "state {state:?} label {label:?}"
            );
        }
    }

    // O5: ReadyGate semantics.
    #[test]
    fn o5_arm_then_fire_is_true_once() {
        let gate = ReadyGate::new();
        let token = gate.arm();
        assert!(gate.fire(token));
        assert!(!gate.fire(token));
    }

    #[test]
    fn o5_fire_after_invalidate_is_false() {
        let gate = ReadyGate::new();
        let token = gate.arm();
        gate.invalidate();
        assert!(!gate.fire(token));
    }

    #[test]
    fn o5_old_token_is_false_after_a_new_arm() {
        let gate = ReadyGate::new();
        let old = gate.arm();
        let new = gate.arm();
        assert!(!gate.fire(old));
        assert!(gate.fire(new));
    }

    #[test]
    fn o5_fire_before_any_arm_is_false() {
        let gate = ReadyGate::new();
        assert!(!gate.fire(ReadyToken(0)));
        assert!(!gate.fire(ReadyToken(1)));
    }

    #[test]
    fn o5_concurrent_fire_of_the_same_token_wins_exactly_once() {
        let gate = Arc::new(ReadyGate::new());
        let token = gate.arm();
        let barrier = Arc::new(Barrier::new(8));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let gate = gate.clone();
                let barrier = barrier.clone();
                thread::spawn(move || {
                    barrier.wait();
                    gate.fire(token)
                })
            })
            .collect();
        let wins = handles
            .into_iter()
            .map(join_or_resume)
            .filter(|won| *won)
            .count();
        assert_eq!(wins, 1);
    }

    // O5b: a ready delivery queued before a hide and a replacement listen must
    // not be emitted for the stale token.
    #[test]
    fn o5b_stale_ready_is_not_emitted_after_hide_and_replacement_listen() {
        let (exec, sink, gate, dispatcher) = rig();
        let t1 = gate.arm();
        dispatcher.deliver_ready(t1);
        dispatcher.apply_state("idle");
        let t2 = gate.arm();
        dispatcher.apply_state("listening");
        exec.drain();
        assert_eq!(sink.ops(), vec![Op::Hide, Op::Show(ShowKind::Listening)]);

        dispatcher.deliver_ready(t2);
        exec.drain();
        assert_eq!(
            sink.ops(),
            vec![Op::Hide, Op::Show(ShowKind::Listening), Op::Ready,]
        );
    }

    #[test]
    fn o5b_hide_invalidates_the_gate_immediately_on_the_caller_thread() {
        let (exec, sink, gate, dispatcher) = rig();
        let token = gate.arm();
        dispatcher.apply_state("idle");
        assert!(!gate.fire(token), "gate must be invalidated before drain");
        exec.drain();
        assert_eq!(sink.ops(), vec![Op::Hide]);
    }

    // O6: cancel routing.
    #[test]
    fn o6_cancel_routes_to_conversation_only_when_session_active() {
        assert_eq!(cancel_route(true), CancelRoute::Conversation);
        assert_eq!(cancel_route(false), CancelRoute::Dictation);
    }

    // O8: ordered dispatch through the held queue.
    #[test]
    fn o8_show_listening_then_hide_ends_hidden() {
        let (exec, sink, _gate, dispatcher) = rig();
        dispatcher.apply_state("listening");
        dispatcher.apply_state("idle");
        exec.drain();
        assert_eq!(sink.ops(), vec![Op::Show(ShowKind::Listening), Op::Hide]);
    }

    #[test]
    fn o8_show_listening_then_show_thinking_ends_on_thinking() {
        let (exec, sink, _gate, dispatcher) = rig();
        dispatcher.apply_state("listening");
        dispatcher.apply_state("thinking");
        exec.drain();
        assert_eq!(
            sink.ops(),
            vec![Op::Show(ShowKind::Listening), Op::Show(ShowKind::Thinking)]
        );
    }

    #[test]
    fn o8_hide_then_show_ends_visible() {
        let (exec, sink, _gate, dispatcher) = rig();
        dispatcher.apply_state("idle");
        dispatcher.apply_state("listening");
        exec.drain();
        assert_eq!(sink.ops(), vec![Op::Hide, Op::Show(ShowKind::Listening)]);
    }

    #[test]
    fn o8_hide_queued_before_a_later_show_is_never_reordered_after_it() {
        let (exec, sink, _gate, dispatcher) = rig();
        dispatcher.apply_state("thinking");
        dispatcher.apply_state("idle");
        dispatcher.apply_state("listening");
        exec.drain();
        assert_eq!(
            sink.ops(),
            vec![
                Op::Show(ShowKind::Thinking),
                Op::Hide,
                Op::Show(ShowKind::Listening),
            ]
        );
    }

    #[test]
    fn o8_speaking_is_a_show_not_a_hide() {
        let (exec, sink, _gate, dispatcher) = rig();
        dispatcher.apply_state("speaking");
        exec.drain();
        assert_eq!(sink.ops(), vec![Op::Show(ShowKind::Speaking)]);
    }

    #[test]
    fn o8_unknown_label_enqueues_nothing() {
        let (exec, sink, _gate, dispatcher) = rig();
        dispatcher.apply_state("bogus");
        assert_eq!(exec.pending(), 0);
        exec.drain();
        assert!(sink.ops().is_empty());
    }
}
