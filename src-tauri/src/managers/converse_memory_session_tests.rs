//! Session-level tests for the conversation memory save (DESIGN Rev 4: A2, A3, A8, A9, A10, A11,
//! and the C2 untrusted signal). Sessions run through the agent-loop rig with a recording
//! reporter that forwards transcript events and the untrusted signal to a `TranscriptCollector`,
//! exactly as `ForwardReporter` does in production.

use super::converse_agent_loop_tests::{acfg, answer, heard, rig_with, Rig};
use super::converse_agent_taint_tests::{tainted_rig, EffectKind};
use super::*;
use crate::managers::conversation_memory::{
    load_for_session, memory_eligible_turns, remember_start, ConversationMemory,
};
use crate::managers::conversation_save::{CollectedSession, TranscriptCollector};
use crate::managers::converse_plan::PlanInput;
use crate::managers::converse_platform::{
    mark_vault_taint, memory_for_plan, memory_session_on, memory_start_row, memory_user_block,
    session_collects, BrainCore,
};
use crate::managers::tool_loop::TaintState;
use crate::settings::get_default_settings;
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::Arc;

const NOW: i64 = 1_800_000_000_000;
const VAULT: &str = "REFERENCE NOTES: the release checklist lives in the vault. VAULT-EVIDENCE-7";
const LAST_UTTERANCE: &str = "hello there";

/// Forwards like `ForwardReporter` (transcript events and the untrusted signal to the sink),
/// and counts the untrusted signals it received.
struct SinkReporter {
    sink: Arc<TranscriptCollector>,
    untrusted_calls: AtomicUsize,
}

impl SinkReporter {
    fn new(sink: Arc<TranscriptCollector>) -> Self {
        SinkReporter {
            sink,
            untrusted_calls: AtomicUsize::new(0),
        }
    }
    fn untrusted_calls(&self) -> usize {
        self.untrusted_calls.load(AtomicOrdering::SeqCst)
    }
}

impl Reporter for SinkReporter {
    fn state(&self, _state: SessionState) {}

    fn transcript(&self, ev: TranscriptEvent) {
        TranscriptSink::record(self.sink.as_ref(), ev);
    }

    fn untrusted(&self) {
        self.untrusted_calls.fetch_add(1, AtomicOrdering::SeqCst);
        TranscriptSink::untrusted(self.sink.as_ref());
    }
}

/// Brain wrapper that applies one taint setter on the turn whose utterance is `LAST_UTTERANCE`
/// (the final turn of the scripted sessions), then delegates to the scripted brain.
struct MarkingBrain<'b> {
    inner: &'b dyn Brain,
    taint: TaintState,
    mark: fn(&TaintState),
    on_final_turn_only: bool,
}

impl Brain for MarkingBrain<'_> {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        let hit = !self.on_final_turn_only || input.utterance == LAST_UTTERANCE;
        if hit {
            (self.mark)(&self.taint);
        }
        self.inner.plan(input)
    }
}

/// Runs one session on the parts of `rig`, with `brain` and `reporter`.
async fn drive(rig: &Rig, brain: &dyn Brain, reporter: &dyn Reporter) -> SessionEnd {
    let deps = Deps {
        policy: None,
        announcer: None,
        ears: &rig.ears,
        mouth: &rig.mouth,
        hands: &rig.hands,
        brain,
        reporter,
    };
    let cfg = acfg();
    let token = rig.cancel.subscribe();
    tokio::time::timeout(Duration::from_secs(10), run_session(&deps, &cfg, token))
        .await
        .expect("run_session hung")
}

fn memory_db() -> ConversationMemory {
    ConversationMemory::open_in_memory().expect("open memory")
}

fn answered(user: &str, speech: &str) -> TranscriptEvent {
    TranscriptEvent {
        user: user.to_string(),
        assistant: Some(speech.to_string()),
        outcome: TurnOutcome::Answered,
        delivery: Delivery::Submitted,
        detail: None,
    }
}

/// A plain session: two answered turns, then the scripted listen ends.
fn plain_rig() -> Rig {
    rig_with(
        vec![heard("what is rust"), heard(LAST_UTTERANCE)],
        vec![answer("Rust is a language."), answer("Hi there.")],
    )
}

/// The memory save as `on_complete` runs it, with memory on: fallback row, untrusted OR taint.
fn save_now(
    memory: &ConversationMemory,
    epoch: u64,
    session: &CollectedSession,
    end: SessionEnd,
    taint: &TaintState,
) -> Option<i64> {
    memory_start_row(memory, epoch, session, end, NOW, true, taint.is_tainted())
}

/// Shared check for every ingress family: the session is eligible on its own, so the only thing
/// that stops the row is the untrusted ingress, and no row is written.
fn assert_ingress_blocks_the_save(session: &CollectedSession, taint: &TaintState) {
    assert!(
        !memory_eligible_turns(&session.turns).is_empty(),
        "the session must hold an eligible Answered turn, or the test proves nothing"
    );
    let memory = memory_db();
    let epoch = memory.epoch();
    assert_eq!(
        save_now(&memory, epoch, session, SessionEnd::UserEnded, taint),
        None
    );
    assert_eq!(
        memory.count().expect("count"),
        0,
        "no row for an untrusted session"
    );
    // Control: the same turns, with no ingress, are remembered.
    let control = remember_start(
        &memory,
        epoch,
        session.started_at_ms,
        NOW,
        &session.turns,
        SessionEnd::UserEnded,
        false,
    );
    assert!(
        control.is_some(),
        "control: the same turns are normally remembered"
    );
}

// ---- C2: the untrusted signal reaches the reporter ------------------------------------------

#[tokio::test]
async fn t_untrusted_signal_is_reported_after_a_tainted_turn_and_at_the_session_end() {
    let rig = tainted_rig("agent result text", EffectKind::Command, vec![], vec![]);
    let reporter = SinkReporter::new(Arc::new(TranscriptCollector::new()));
    drive(&rig, &rig.brain, &reporter).await;
    assert!(
        reporter.untrusted_calls() >= 1,
        "a turn that tainted the session must report it"
    );
}

#[tokio::test]
async fn t_untrusted_signal_is_never_reported_when_the_session_stays_clean() {
    let rig = plain_rig();
    let reporter = SinkReporter::new(Arc::new(TranscriptCollector::new()));
    let brain = &rig.brain;
    drive(&rig, brain, &reporter).await;
    assert_eq!(
        reporter.untrusted_calls(),
        0,
        "an untainted session reports nothing"
    );
}

// ---- A9: the outcome label of every plan kind ------------------------------------------------

/// Exhaustive on purpose: a new `PlanKind` fails to compile here until its label is chosen.
fn pinned_label(kind: &PlanKind) -> TurnOutcome {
    match kind {
        PlanKind::Answer => TurnOutcome::Answered,
        PlanKind::Clarify => TurnOutcome::Clarified,
        PlanKind::End => TurnOutcome::Ended,
        // Locally handled: the role or lesson state changes here, not an answer from the model.
        PlanKind::SetRole | PlanKind::ClearRole | PlanKind::Resume => TurnOutcome::Acted,
        // Returned by the model; the utterance went to the model route.
        PlanKind::Paste | PlanKind::Command | PlanKind::Job | PlanKind::Agent | PlanKind::Plan => {
            TurnOutcome::Answered
        }
    }
}

#[test]
fn t_a9_every_plan_kind_has_a_pinned_outcome_label() {
    let every = [
        PlanKind::Answer,
        PlanKind::Clarify,
        PlanKind::End,
        PlanKind::SetRole,
        PlanKind::ClearRole,
        PlanKind::Resume,
        PlanKind::Paste,
        PlanKind::Command,
        PlanKind::Job,
        PlanKind::Agent,
        PlanKind::Plan,
    ];
    for kind in &every {
        assert_eq!(
            speech_outcome(kind),
            pinned_label(kind),
            "outcome label of {kind:?}"
        );
    }
}

#[test]
fn t_a9_only_model_answers_may_be_answered_or_clarified() {
    // Locally handled kinds never carry the model-answer labels.
    for kind in [PlanKind::SetRole, PlanKind::ClearRole, PlanKind::Resume] {
        let label = speech_outcome(&kind);
        assert!(
            label != TurnOutcome::Answered && label != TurnOutcome::Clarified,
            "{kind:?} is handled locally and must not be Answered or Clarified"
        );
    }
}

// ---- A2: the collector exists iff records or memory need it ----------------------------------

#[test]
fn t_a2_collector_exists_iff_records_or_memory_is_on() {
    for records_on in [false, true] {
        for enabled in [false, true] {
            for store_present in [false, true] {
                for route_allowed in [false, true] {
                    let memory_on = memory_session_on(enabled, store_present, route_allowed);
                    let expected = records_on || (enabled && store_present && route_allowed);
                    assert_eq!(
                        session_collects(records_on, memory_on),
                        expected,
                        "records={records_on} enabled={enabled} store={store_present} route={route_allowed}"
                    );
                }
            }
        }
    }
}

// ---- A3: on_complete writes the fallback row synchronously ------------------------------------

fn collected(turns: &[TranscriptEvent]) -> CollectedSession {
    let collector = TranscriptCollector::with_clock(Box::new(|| NOW - 60_000));
    for ev in turns {
        TranscriptSink::record(&collector, ev.clone());
    }
    collector.take()
}

#[test]
fn t_a3_memory_only_session_writes_exactly_one_fallback_row_before_returning() {
    let memory = memory_db();
    let session = collected(&[answered("what is rust", "Rust is a language.")]);
    let id = memory_start_row(
        &memory,
        0,
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        false,
    );
    assert!(id.is_some());
    assert_eq!(
        memory.count().expect("count"),
        1,
        "one row, already written on return"
    );
}

#[test]
fn t_a3_setting_switched_off_before_the_session_ended_writes_nothing() {
    let memory = memory_db();
    let session = collected(&[answered("what is rust", "Rust is a language.")]);
    let id = memory_start_row(
        &memory,
        0,
        &session,
        SessionEnd::UserEnded,
        NOW,
        false,
        false,
    );
    assert_eq!(id, None);
    assert_eq!(memory.count().expect("count"), 0);
}

#[test]
fn t_a3_tainted_session_writes_nothing() {
    let memory = memory_db();
    let session = collected(&[answered("what is rust", "Rust is a language.")]);
    let id = memory_start_row(&memory, 0, &session, SessionEnd::UserEnded, NOW, true, true);
    assert_eq!(id, None);
    assert_eq!(memory.count().expect("count"), 0);
}

#[test]
fn t_a3_untrusted_collector_writes_nothing() {
    let memory = memory_db();
    let collector = TranscriptCollector::with_clock(Box::new(|| NOW));
    TranscriptSink::record(&collector, answered("what is rust", "Rust is a language."));
    TranscriptSink::untrusted(&collector);
    let session = collector.take();
    assert!(session.untrusted);
    let id = memory_start_row(
        &memory,
        0,
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        false,
    );
    assert_eq!(id, None);
    assert_eq!(memory.count().expect("count"), 0);
}

// ---- C2 collector: the flag is independent of the turn bound ---------------------------------

#[test]
fn t_collector_untrusted_flag_survives_the_turn_bound_and_resets_on_take() {
    let collector = TranscriptCollector::with_clock(Box::new(|| NOW));
    for _ in 0..(crate::managers::conversation_record::MAX_TURNS + 5) {
        TranscriptSink::record(&collector, answered("hi", "hello"));
    }
    TranscriptSink::untrusted(&collector);
    let first = collector.take();
    assert!(first.untrusted, "the flag is set past the turn bound");
    assert_eq!(
        first.turns.len(),
        crate::managers::conversation_record::MAX_TURNS
    );
    assert!(
        !collector.take().untrusted,
        "take() leaves the collector empty"
    );
}

#[test]
fn t_collector_untrusted_set_after_the_last_event_is_kept() {
    let collector = TranscriptCollector::with_clock(Box::new(|| NOW));
    TranscriptSink::record(&collector, answered("hi", "hello"));
    TranscriptSink::untrusted(&collector);
    assert!(collector.take().untrusted);
}

// ---- A8: session 1's summary reaches session 2's prompt --------------------------------------

fn http_settings(memory_on: bool) -> crate::settings::AppSettings {
    let mut s = get_default_settings();
    s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Model;
    s.tts_composer_provider_id = "openai".to_string();
    s.tts_composer_model = "synthetic-model".to_string();
    s.conversation_memory_enabled = memory_on;
    s
}

#[tokio::test]
async fn t_a8_second_session_prompt_carries_the_first_session_summary() {
    let memory = memory_db();
    // Session 1: two answered turns, then it ends.
    let epoch = memory.epoch();
    let rig = plain_rig();
    let collector = Arc::new(TranscriptCollector::new());
    let reporter = SinkReporter::new(collector.clone());
    let end = drive(&rig, &rig.brain, &reporter).await;
    let session = collector.take();
    let id = memory_start_row(&memory, epoch, &session, end, NOW, true, false);
    assert!(id.is_some(), "session 1 writes its fallback row");
    let stored = memory.recent(5).expect("recent");
    assert_eq!(stored.len(), 1);

    // Session 2 starts on the same store: its first plan call sees the summary wrapped.
    let ctx = load_for_session(&memory, NOW + 1_000);
    let core = BrainCore {
        vault_block: Arc::new(std::sync::Mutex::new(None)),
        vault_route: true,
        taint: TaintState::new(),
        rag: None,
        retrieval_deadline: Duration::from_secs(2),
        memory: ctx,
        notes_used: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        after_retrieve: None,
    };
    let block = memory_for_plan(&core, &http_settings(true)).expect("block on the first call");
    assert!(
        block.contains(&stored[0].summary),
        "the stored summary is in the block"
    );
    let wrapped = memory_user_block(block);
    assert!(wrapped.contains("<untrusted_memory>"));
    assert!(wrapped.contains(&stored[0].summary));

    // Setting off: nothing is injected.
    assert_eq!(memory_for_plan(&core, &http_settings(false)), None);
}

// ---- A10: each untrusted-ingress family writes no row -----------------------------------------

/// (i) agent result: `Carry.tainted` via the agent loop, reported through the loop call.
#[tokio::test]
async fn t_a10_i_agent_result_writes_no_row() {
    let rig = tainted_rig("agent result text", EffectKind::Command, vec![], vec![]);
    let collector = Arc::new(TranscriptCollector::new());
    let reporter = SinkReporter::new(collector.clone());
    drive(&rig, &rig.brain, &reporter).await;
    let session = collector.take();
    assert!(
        session.untrusted,
        "the agent result must reach the collector"
    );
    assert_ingress_blocks_the_save(&session, &TaintState::new());
}

/// (ii) tool output: the shared `TaintState` set by a tool step.
#[tokio::test]
async fn t_a10_ii_tool_output_writes_no_row() {
    let rig = plain_rig();
    let taint = TaintState::new();
    let brain = MarkingBrain {
        inner: &rig.brain,
        taint: taint.clone(),
        mark: |t| t.mark(),
        on_final_turn_only: false,
    };
    let collector = Arc::new(TranscriptCollector::new());
    drive(&rig, &brain, &SinkReporter::new(collector.clone())).await;
    assert!(taint.is_tainted(), "the tool output taints the session");
    assert_ingress_blocks_the_save(&collector.take(), &taint);
}

/// (iii) RAG or vault evidence: `mark_vault_taint` on an evidence block.
/// Synthetic: it calls `mark_vault_taint` directly. The real planning paths (plain route,
/// tool route, RAG evidence) are proven by the `t_v1_*` tests below, which call `plan_turn_core`.
#[tokio::test]
async fn t_a10_iii_vault_evidence_writes_no_row() {
    let rig = plain_rig();
    let taint = TaintState::new();
    let brain = MarkingBrain {
        inner: &rig.brain,
        taint: taint.clone(),
        mark: |t| mark_vault_taint(t, VAULT),
        on_final_turn_only: false,
    };
    let collector = Arc::new(TranscriptCollector::new());
    drive(&rig, &brain, &SinkReporter::new(collector.clone())).await;
    assert!(taint.is_tainted(), "non-empty evidence taints the session");
    assert_ingress_blocks_the_save(&collector.take(), &taint);
}

/// (iv) ingress only in the FINAL turn, and that turn then ends the session.
#[tokio::test]
async fn t_a10_iv_ingress_in_the_final_turn_writes_no_row() {
    let rig = plain_rig();
    let taint = TaintState::new();
    let brain = MarkingBrain {
        inner: &rig.brain,
        taint: taint.clone(),
        mark: |t| t.mark(),
        on_final_turn_only: true,
    };
    let collector = Arc::new(TranscriptCollector::new());
    drive(&rig, &brain, &SinkReporter::new(collector.clone())).await;
    assert!(taint.is_tainted(), "the final turn taints the session");
    assert_ingress_blocks_the_save(&collector.take(), &taint);
}

/// (v) ingress after the collector's turn bound: the flag is not limited by `MAX_TURNS`.
#[test]
fn t_a10_v_ingress_after_the_turn_bound_writes_no_row() {
    let memory = memory_db();
    let collector = TranscriptCollector::with_clock(Box::new(|| NOW));
    for _ in 0..(crate::managers::conversation_record::MAX_TURNS + 1) {
        TranscriptSink::record(&collector, answered("hi", "hello"));
    }
    TranscriptSink::untrusted(&collector);
    let session = collector.take();
    assert!(session.untrusted);
    let id = memory_start_row(
        &memory,
        0,
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        false,
    );
    assert_eq!(id, None);
    assert_eq!(memory.count().expect("count"), 0);
}

// ---- A11: a clear between start and end writes nothing ----------------------------------------

#[test]
fn t_a11_clear_between_session_start_and_end_writes_nothing() {
    let memory = memory_db();
    let session = collected(&[answered("what is rust", "Rust is a language.")]);
    let epoch_at_start = memory.epoch();
    memory.clear().expect("clear");
    let id = memory_start_row(
        &memory,
        epoch_at_start,
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        false,
    );
    assert_eq!(id, None, "the epoch moved, so the write is refused");
    assert_eq!(memory.count().expect("count"), 0);
}

// ---- corrective round 1 (G1-G4) ---------------------------------------------------------------

use super::converse_agent_loop_tests::{agent_turn, RunCtl, Step};
use crate::managers::converse::ListenOutcome;
use crate::managers::converse_agent::{AgentEvent, AgentPoll};
use crate::managers::converse_capture::CaptureHandle;
use crate::managers::converse_plan::build_plan_prompt;
use crate::managers::converse_platform::vault_text_allowed;
use crate::managers::converse_rag_tests::{
    core_for, evidence_for, input_with, plan_json, ready_rig, serve_sequence, text_reply,
};
use crate::managers::converse_runtime::{
    forward_reporter_for_tests, session_end_after_panic, Platform, SessionParts,
};
use crate::settings::AppSettings;
use futures_util::future::BoxFuture;
use futures_util::FutureExt;
use std::panic::AssertUnwindSafe;

/// Two eligible Answered turns collected through the real collector, then taken.
fn two_answered_turns_collected() -> CollectedSession {
    let collector = TranscriptCollector::with_clock(Box::new(|| NOW - 60_000));
    TranscriptSink::record(&collector, answered("what is rust", "Rust is a language."));
    TranscriptSink::record(&collector, answered("and go", "Go is a language too."));
    collector.take()
}

/// The user text of the FIRST plan prompt of a new session: the real `load_for_session`,
/// the real `memory_for_plan` and the real `build_plan_prompt`.
fn first_plan_user_text(
    memory: &ConversationMemory,
    settings: &AppSettings,
    vault_route: bool,
) -> String {
    let mut core = core_for(None, vault_route);
    core.memory = load_for_session(memory, NOW + 1_000);
    let block = memory_for_plan(&core, settings);
    let input = PlanInput {
        memory_context: block,
        ..input_with("let's continue where we left off")
    };
    build_plan_prompt(&input).user
}

// G1: the RAG setter (converse_platform.rs `core.taint.mark()` in prepare_rag_evidence) reached
// through the real function, and the save it blocks.

#[tokio::test]
async fn t_g1_rag_evidence_taints_through_prepare_rag_evidence_and_blocks_the_save() {
    let (base, _seen) = serve_sequence(vec![text_reply(&plan_json("answer", "On track."))]);
    let rig = ready_rig(&base, false);
    let taint = TaintState::new();
    let block = evidence_for(&rig, "status of Atlas", &taint).await;
    assert!(block.is_some(), "the Atlas note is rendered as evidence");
    assert!(
        taint.is_tainted(),
        "Evidence marks the shared taint in prepare_rag_evidence"
    );

    let session = two_answered_turns_collected();
    let control = ConversationMemory::open_in_memory().expect("open");
    assert!(
        memory_start_row(
            &control,
            0,
            &session,
            SessionEnd::UserEnded,
            NOW,
            true,
            false
        )
        .is_some(),
        "control: the same turns are remembered when untainted"
    );
    let memory = memory_db();
    let id = memory_start_row(
        &memory,
        memory.epoch(),
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        taint.is_tainted(),
    );
    assert_eq!(id, None, "tainted by RAG evidence: no row");
    assert_eq!(memory.count().expect("count"), 0);
}

// G2: the real chain from collected turns to the first plan prompt, with negatives.

#[test]
fn t_g2_real_chain_puts_the_first_session_summary_inside_the_untrusted_memory_wrapper() {
    let memory = memory_db();
    let session = two_answered_turns_collected();
    let id = memory_start_row(
        &memory,
        memory.epoch(),
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        false,
    );
    assert!(id.is_some(), "session 1 writes its fallback row");
    let summary = memory.recent(5).expect("recent")[0].summary.clone();

    let user = first_plan_user_text(&memory, &http_settings(true), true);
    let open = user.find("<untrusted_memory>").expect("the wrapper opens");
    let close = user
        .find("</untrusted_memory>")
        .expect("the wrapper closes");
    let at = user
        .find(&summary)
        .expect("the first session's summary is in the prompt");
    assert!(
        open < at && at < close,
        "the summary sits inside the wrapper"
    );
}

#[test]
fn t_g2_setting_off_keeps_the_summary_out_of_the_first_prompt() {
    let memory = memory_db();
    let session = two_answered_turns_collected();
    memory_start_row(
        &memory,
        memory.epoch(),
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        false,
    );
    let summary = memory.recent(5).expect("recent")[0].summary.clone();
    let user = first_plan_user_text(&memory, &http_settings(false), true);
    assert!(!user.contains(&summary), "setting off: no summary");
    assert!(
        !user.contains("<untrusted_memory>"),
        "setting off: no wrapper"
    );
}

#[test]
fn t_g2_route_not_allowed_keeps_the_summary_out_of_the_first_prompt() {
    let memory = memory_db();
    let session = two_answered_turns_collected();
    memory_start_row(
        &memory,
        memory.epoch(),
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        false,
    );
    let summary = memory.recent(5).expect("recent")[0].summary.clone();

    // Core not on a vault route.
    let user = first_plan_user_text(&memory, &http_settings(true), false);
    assert!(!user.contains(&summary), "vault_route false: no summary");

    // Settings whose route is not an HTTP model route.
    let mut local = get_default_settings();
    local.conversation_memory_enabled = true;
    assert!(
        !vault_text_allowed(&local),
        "the default route is not allowed"
    );
    let user = first_plan_user_text(&memory, &local, true);
    assert!(!user.contains(&summary), "route not allowed: no summary");
}

#[test]
fn t_g2_epoch_bumped_after_load_keeps_the_summary_out_of_the_prompt() {
    let memory = memory_db();
    let session = two_answered_turns_collected();
    memory_start_row(
        &memory,
        memory.epoch(),
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        false,
    );
    let summary = memory.recent(5).expect("recent")[0].summary.clone();

    let mut core = core_for(None, true);
    core.memory = load_for_session(&memory, NOW + 1_000);
    memory.bump_epoch();
    assert_eq!(
        memory.count().expect("count"),
        1,
        "the row survives the bump"
    );
    let block = memory_for_plan(&core, &http_settings(true));
    assert_eq!(
        block, None,
        "a bumped epoch removes the block from the next call"
    );
    let input = PlanInput {
        memory_context: block,
        ..input_with("let's continue")
    };
    assert!(!build_plan_prompt(&input).user.contains(&summary));
}

// G3: the real forwarding reporter (converse_runtime.rs) wrapping a real collector.

/// A platform for sessions driven directly: nothing here is ever called by these tests.
struct FakePlatform;

impl Platform for FakePlatform {
    fn preflight(&self) -> Result<(), String> {
        Ok(())
    }
    fn capture(&self) -> CaptureHandle {
        unreachable!("these tests drive the session directly")
    }
    fn parts(&self, _capture: &CaptureHandle) -> SessionParts {
        unreachable!("these tests drive the session directly")
    }
    fn on_state(&self, _state: &'static str, _end: Option<&'static str>) {}
}

fn forwarder(collector: &Arc<TranscriptCollector>) -> impl Reporter {
    forward_reporter_for_tests(
        Arc::new(FakePlatform),
        Some(collector.clone() as Arc<dyn TranscriptSink>),
    )
}

#[test]
fn t_g3_forward_reporter_untrusted_reaches_the_collector() {
    let collector = Arc::new(TranscriptCollector::new());
    let reporter = forwarder(&collector);
    reporter.untrusted();
    assert!(
        collector.take().untrusted,
        "the forwarder sets the collector flag"
    );
}

#[tokio::test]
async fn t_g3_agent_result_reaches_the_collector_through_the_real_forwarder() {
    let rig = tainted_rig("agent result text", EffectKind::Command, vec![], vec![]);
    let collector = Arc::new(TranscriptCollector::new());
    let reporter = forwarder(&collector);
    drive(&rig, &rig.brain, &reporter).await;
    let session = collector.take();
    assert!(session.untrusted, "a tainted session reports untrusted");
    assert!(!memory_eligible_turns(&session.turns).is_empty());
}

#[tokio::test]
async fn t_g3_untainted_whole_session_reports_false_through_the_real_forwarder() {
    let rig = plain_rig();
    let collector = Arc::new(TranscriptCollector::new());
    let reporter = forwarder(&collector);
    drive(&rig, &rig.brain, &reporter).await;
    assert!(
        !collector.take().untrusted,
        "a clean session reports nothing"
    );
}

/// Steps for a session whose agent result is delivered DURING the last listen: the result is
/// collected at the next turn boundary, so the tainting turn is the final turn and the session
/// ends straight after it. Launch, then a thanks, then the final utterance.
fn final_turn_result_steps(run: Arc<RunCtl>) -> Vec<Step> {
    vec![
        heard("run the build"),
        heard("thanks"),
        Step {
            outcome: ListenOutcome::Utterance(LAST_UTTERANCE.to_string()),
            hook: Some(Box::new(move || {
                run.push(AgentPoll::Event(AgentEvent::Done(
                    "build passed".to_string(),
                )));
            })),
        },
    ]
}

fn final_turn_rig(run: Arc<RunCtl>) -> Rig {
    let r = rig_with(
        final_turn_result_steps(run.clone()),
        vec![
            agent_turn("check the build", "On it."),
            answer("You're welcome."),
            answer("Hi there."),
        ],
    );
    r.hands.starts.lock().unwrap().push_back(Ok(run));
    r
}

#[tokio::test]
async fn t_g3_tainting_turn_as_the_final_turn_reaches_the_collector() {
    let run = RunCtl::new();
    let rig = final_turn_rig(run);
    let collector = Arc::new(TranscriptCollector::new());
    let reporter = forwarder(&collector);
    drive(&rig, &rig.brain, &reporter).await;
    let session = collector.take();
    assert!(
        session.untrusted,
        "the final-turn taint is reported after the loop exits"
    );
    assert!(!memory_eligible_turns(&session.turns).is_empty());
}

// G4: a panicking session is flagged through `session_end_after_panic`, and writes no row.

/// Brain wrapper that panics on the turn whose utterance is `when`, else delegates.
struct PanicOn<'b> {
    inner: &'b dyn Brain,
    when: &'static str,
}

impl Brain for PanicOn<'_> {
    fn plan<'a>(&'a self, input: PlanInput<'a>) -> BoxFuture<'a, Result<TurnPlan, PlanError>> {
        if input.utterance == self.when {
            panic!("scripted brain panic");
        }
        self.inner.plan(input)
    }
}

/// Counts the untrusted signals it receives.
struct CountingReporter(AtomicUsize);

impl Reporter for CountingReporter {
    fn state(&self, _state: SessionState) {}
    fn untrusted(&self) {
        self.0.fetch_add(1, AtomicOrdering::SeqCst);
    }
}

#[test]
fn t_g4_session_end_after_panic_flags_untrusted_and_is_cancelled() {
    let reporter = CountingReporter(AtomicUsize::new(0));
    assert_eq!(session_end_after_panic(&reporter), SessionEnd::Cancelled);
    assert_eq!(
        reporter.0.load(AtomicOrdering::SeqCst),
        1,
        "flagged exactly once"
    );
}

#[tokio::test]
async fn t_g4_panicking_session_writes_no_row() {
    // The agent result is delivered during the last listen and the brain panics on that same
    // turn, so the turn boundary that would have reported it is never reached: only the panic
    // path can flag the session.
    let run = RunCtl::new();
    let rig = final_turn_rig(run);
    let brain = PanicOn {
        inner: &rig.brain,
        when: LAST_UTTERANCE,
    };
    let collector = Arc::new(TranscriptCollector::new());
    let reporter = forwarder(&collector);
    let panicked = FutureExt::catch_unwind(AssertUnwindSafe(drive(&rig, &brain, &reporter))).await;
    assert!(panicked.is_err(), "the scripted brain panicked");

    let end = session_end_after_panic(&reporter);
    let session = collector.take();
    assert_eq!(end, SessionEnd::Cancelled);
    assert!(session.untrusted, "the panic path flags the session");
    assert!(
        !memory_eligible_turns(&session.turns).is_empty(),
        "the session had eligible turns"
    );
    let memory = memory_db();
    let id = memory_start_row(&memory, memory.epoch(), &session, end, NOW, true, false);
    assert_eq!(id, None, "a panicked session writes no row");
    assert_eq!(memory.count().expect("count"), 0);
}

// ---- V1: notes reach a plan on either route, so the session is not remembered -----------------

use crate::managers::converse_platform::core_contaminated;
use crate::managers::converse_platform::{same_summary_route, summary_allowed};
use crate::managers::converse_rag_tests::{plan_once, rag_settings, request_text};

/// A vault note that must never reach a remembered summary of this session.
const NOTES: &str = "PRIVATE-NOTE-42: the vault says the launch is on Friday.";

/// A plain brain core whose M1 slot holds `notes` (`None` is an empty slot).
fn core_with_notes(notes: Option<&str>) -> BrainCore {
    let core = core_for(None, true);
    if let Ok(mut slot) = core.vault_block.lock() {
        *slot = notes.map(str::to_string);
    }
    core
}

/// The end-of-session decision as `on_complete` makes it: the taint OR the notes flag.
fn remember_decision(memory: &ConversationMemory, core: &BrainCore) -> Option<i64> {
    let session = two_answered_turns_collected();
    let tainted = core.taint.is_tainted() || core.notes_used.load(AtomicOrdering::SeqCst);
    memory_start_row(
        memory,
        memory.epoch(),
        &session,
        SessionEnd::UserEnded,
        NOW,
        true,
        tainted,
    )
}

/// The request bodies the loopback server received, as text.
fn sent_texts(seen: &Arc<std::sync::Mutex<Vec<Vec<u8>>>>) -> Vec<String> {
    seen.lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .map(|raw| request_text(raw))
        .collect()
}

/// (a) plain route, notes in the slot: the notes reach the model, the flag is set, the taint
/// is still untouched (the plain-route guard is unchanged), and no row is written.
#[tokio::test]
async fn t_v1_a_plain_route_with_notes_is_not_remembered() {
    let (base, seen) = serve_sequence(vec![text_reply(&plan_json("answer", "Friday."))]);
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = rag_settings(tmp.path(), false, &base);
    let core = core_with_notes(Some(NOTES));
    let memory = memory_db();

    plan_once(
        &core,
        &settings,
        "when is launch",
        Arc::new(AtomicUsize::new(0)),
    )
    .await
    .expect("plain route plan");

    assert!(
        sent_texts(&seen).iter().any(|text| text.contains(NOTES)),
        "the notes reached the model on the plain route"
    );
    assert!(core.notes_used.load(AtomicOrdering::SeqCst), "flag set");
    assert!(!core.taint.is_tainted(), "TaintState stays untainted");
    assert_eq!(remember_decision(&memory, &core), None, "no row");
    assert_eq!(memory.count().expect("count"), 0);
}

/// (b) same call with an empty slot: the flag stays false and the row is written.
#[tokio::test]
async fn t_v1_b_plain_route_without_notes_is_remembered() {
    let (base, _seen) = serve_sequence(vec![text_reply(&plan_json("answer", "Friday."))]);
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = rag_settings(tmp.path(), false, &base);
    let core = core_with_notes(None);
    let memory = memory_db();

    plan_once(
        &core,
        &settings,
        "when is launch",
        Arc::new(AtomicUsize::new(0)),
    )
    .await
    .expect("plain route plan");

    assert!(!core.notes_used.load(AtomicOrdering::SeqCst), "flag false");
    assert!(remember_decision(&memory, &core).is_some(), "row written");
    assert_eq!(memory.count().expect("count"), 1);
}

/// (c) tool route, notes in the slot: the flag is set, the taint is set as before, no row.
#[tokio::test]
async fn t_v1_c_tool_route_with_notes_is_tainted_and_not_remembered() {
    let (base, _seen) = serve_sequence(vec![text_reply(&plan_json("answer", "Friday."))]);
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = rag_settings(tmp.path(), true, &base);
    let core = core_with_notes(Some(NOTES));
    let memory = memory_db();

    plan_once(
        &core,
        &settings,
        "when is launch",
        Arc::new(AtomicUsize::new(0)),
    )
    .await
    .expect("tool route plan");

    assert!(core.notes_used.load(AtomicOrdering::SeqCst), "flag set");
    assert!(core.taint.is_tainted(), "the tool route taints as before");
    assert_eq!(remember_decision(&memory, &core), None, "no row");
}

/// (d) RAG evidence on the plain route: the evidence block sets the flag and no row is written.
#[tokio::test]
async fn t_v1_d_rag_evidence_is_not_remembered() {
    let (base, _seen) = serve_sequence(vec![text_reply(&plan_json("answer", "On track."))]);
    let rig = ready_rig(&base, false);
    let core = core_for(Some(rig.runtime.session(&rig.settings)), true);
    let memory = memory_db();

    plan_once(
        &core,
        &rig.settings,
        "status of Atlas",
        Arc::new(AtomicUsize::new(0)),
    )
    .await
    .expect("plan with evidence");

    assert!(core.notes_used.load(AtomicOrdering::SeqCst), "flag set");
    assert_eq!(remember_decision(&memory, &core), None, "no row");
    assert_eq!(memory.count().expect("count"), 0);
}

/// (e) the flag is sticky: a later turn with no notes leaves it set and the row still unwritten.
#[tokio::test]
async fn t_v1_e_flag_stays_set_after_a_later_turn_without_notes() {
    let (base, _seen) = serve_sequence(vec![
        text_reply(&plan_json("answer", "Friday.")),
        text_reply(&plan_json("answer", "Still Friday.")),
    ]);
    let tmp = tempfile::tempdir().expect("tempdir");
    let settings = rag_settings(tmp.path(), false, &base);
    let core = core_with_notes(Some(NOTES));
    let memory = memory_db();
    let calls = Arc::new(AtomicUsize::new(0));

    plan_once(&core, &settings, "when is launch", calls.clone())
        .await
        .expect("first turn");
    if let Ok(mut slot) = core.vault_block.lock() {
        *slot = None;
    }
    plan_once(&core, &settings, "and the date", calls)
        .await
        .expect("second turn without notes");

    assert!(core.notes_used.load(AtomicOrdering::SeqCst), "still set");
    assert_eq!(remember_decision(&memory, &core), None, "no row");
}

// ---- V4: the summary route is re-checked against the snapshot ---------------------------------

/// Settings with a direct summary route to a fixed endpoint, model and token.
fn summary_settings() -> crate::settings::AppSettings {
    let mut s = get_default_settings();
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_endpoint_url = "https://summary.example.invalid/v1".into();
    s.exp_rt_model = "model-a".into();
    s.exp_rt_token = "token-a".into();
    s
}

/// Whether mutating one field of a copy changes the summary route.
fn route_changes_when(mutate: impl FnOnce(&mut crate::settings::AppSettings)) -> bool {
    let base = summary_settings();
    let mut changed = base.clone();
    mutate(&mut changed);
    !same_summary_route(&base, &changed)
}

#[test]
fn t_v4_unrelated_settings_keep_the_route() {
    let base = summary_settings();
    let mut other = base.clone();
    other.tts_enabled = !base.tts_enabled;
    other.vault_path = "/somewhere/else".into();
    other.conversation_memory_enabled = true;
    assert!(same_summary_route(&base, &other));
    assert!(
        summary_allowed(&other, &base),
        "allowed when only unrelated settings differ"
    );
}

#[test]
fn t_v4_endpoint_change_is_not_the_same_route() {
    assert!(route_changes_when(
        |s| s.exp_rt_endpoint_url = "https://other.example.invalid/v1".into()
    ));
}

#[test]
fn t_v4_model_change_is_not_the_same_route() {
    assert!(route_changes_when(|s| s.exp_rt_model = "model-b".into()));
}

#[test]
fn t_v4_token_change_is_not_the_same_route() {
    assert!(route_changes_when(|s| s.exp_rt_token = "token-b".into()));
}

#[test]
fn t_v4_route_kind_change_is_not_the_same_route() {
    assert!(route_changes_when(|s| s.exp_rt_direct_endpoint = false));
}

#[test]
fn t_v4_stream_flag_change_is_not_the_same_route() {
    assert!(route_changes_when(
        |s| s.exp_rt_stream_reply = !s.exp_rt_stream_reply
    ));
}

#[test]
fn t_v4_composer_mode_change_is_not_the_same_route() {
    assert!(route_changes_when(|s| {
        s.tts_composer_mode = crate::managers::composer_config::TtsComposerMode::Model
    }));
}

#[test]
fn t_v4_composer_model_change_is_not_the_same_route() {
    assert!(route_changes_when(
        |s| s.tts_composer_model = "composer-b".into()
    ));
}

#[test]
fn t_v4_brain_source_change_is_not_the_same_route() {
    assert!(route_changes_when(|s| {
        use crate::settings::BrainSource;
        s.brain_source_realtime = match s.brain_source_realtime {
            BrainSource::Own => BrainSource::Inherit,
            BrainSource::Inherit => BrainSource::Own,
        };
    }));
}

#[test]
fn t_v4_brain_model_change_is_not_the_same_route() {
    assert!(route_changes_when(|s| s.brain_model = "brain-b".into()));
}

#[test]
fn t_v4_api_key_change_is_not_the_same_route() {
    assert!(route_changes_when(|s| {
        s.post_process_api_keys
            .insert("provider-x".into(), "key-b".into());
    }));
}

#[test]
fn t_v4_provider_base_url_change_is_not_the_same_route() {
    let base = get_default_settings();
    assert!(
        !base.post_process_providers.is_empty(),
        "the default table has providers"
    );
    assert!(route_changes_when(|s| {
        if let Some(p) = s.post_process_providers.first_mut() {
            p.base_url.push_str("/changed");
        }
    }));
}

#[test]
fn t_v4_allowed_needs_memory_on_the_http_route_and_the_same_route() {
    let base = summary_settings();
    let mut on = base.clone();
    on.conversation_memory_enabled = true;
    assert!(
        summary_allowed(&on, &on.clone()),
        "on, HTTP route, same route"
    );

    let mut off = on.clone();
    off.conversation_memory_enabled = false;
    assert!(!summary_allowed(&off, &on), "setting off stops the summary");

    let mut moved = on.clone();
    moved.exp_rt_endpoint_url = "https://moved.example.invalid/v1".into();
    assert!(
        !summary_allowed(&moved, &on),
        "a route switch stops the summary"
    );
}

// ---- T1: core_contaminated reads the taint and notes flags (PR-A, role-state eligibility) ------

/// `core_contaminated` is the seam `RealBrain::state_contaminated` delegates to.
#[test]
fn t1_core_contaminated_reads_the_taint_and_the_notes_flags() {
    let core = core_for(None, false);
    assert!(
        !core_contaminated(&core),
        "a clean core is not contaminated"
    );
    core.taint.mark();
    assert!(core_contaminated(&core), "a tainted core is contaminated");
    let core = core_for(None, false);
    core.notes_used.store(true, AtomicOrdering::SeqCst);
    assert!(
        core_contaminated(&core),
        "a core with notes_used is contaminated"
    );
}

/// A plan that places a notes block sets `notes_used` (through `plan_turn_core`), so the core is
/// contaminated after the call, while the taint stays clear. An empty slot is the control.
#[tokio::test]
async fn t1_a_plan_with_notes_makes_the_core_contaminated() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (base, _seen) = serve_sequence(vec![text_reply(&plan_json("answer", "Friday."))]);
    let settings = rag_settings(tmp.path(), false, &base);
    let clean = core_with_notes(None);
    plan_once(
        &clean,
        &settings,
        "when is launch",
        Arc::new(AtomicUsize::new(0)),
    )
    .await
    .expect("plan with an empty slot");
    assert!(
        !core_contaminated(&clean),
        "control: an empty notes slot leaves the core clean"
    );

    let (base, _seen) = serve_sequence(vec![text_reply(&plan_json("answer", "Friday."))]);
    let settings = rag_settings(tmp.path(), false, &base);
    let core = core_with_notes(Some(NOTES));
    plan_once(
        &core,
        &settings,
        "when is launch",
        Arc::new(AtomicUsize::new(0)),
    )
    .await
    .expect("plan with notes");
    assert!(!core.taint.is_tainted(), "the taint is not set by notes");
    assert!(
        core_contaminated(&core),
        "a plan with a notes block makes the core contaminated"
    );
}
