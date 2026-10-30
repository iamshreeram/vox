//! Natural forced confirmation (DESIGN D1-D4): the spoken question, a local verdict, one model
//! judgement for answers the local table cannot decide, and at most one re-ask.
//!
//! Nothing here runs an effect. The caller runs the pending plan only on `Decision::Approved`.
//! Safety property: an approval needs a clear, unconditional agreement in the user's own answer,
//! either from the local table (whole utterance) or from the judge after the gate passed.

use crate::managers::converse::{Brain, Ears, ListenOutcome, Mouth, Reporter};
use crate::managers::converse_confirm::{
    ask_and_listen, decision_for, ConfirmConfig, Decision, DeclineReason, Verdict,
    FORCED_PROMPT_TAIL,
};
use crate::managers::converse_confirm_judge::{
    forced_local_verdict, judge_gate, AnswerJudgement, Gate, JUDGE_DEADLINE,
};
use crate::managers::job_announcer::Announcer;

/// The re-ask for `question`, or `None` when the question is not a forced question. The re-ask
/// restates the same pending action (DESIGN A3); it equals `converse_confirm::reask_prompt` of the
/// plan the question was built from.
pub fn reask_for_question(question: &str) -> Option<String> {
    let body = question
        .strip_suffix(FORCED_PROMPT_TAIL)?
        .trim_end()
        .trim_end_matches('.');
    if body.trim().is_empty() {
        return None;
    }
    Some(format!(
        "I have not changed anything. Again: {body}. {FORCED_PROMPT_TAIL}"
    ))
}

/// What one heard answer decided: a final decision, or still unclear (the re-ask may follow).
enum Heard {
    Decided(Decision),
    Unclear,
}

/// Judges one listen outcome. Local verdicts first; the model only for locally unclear, gated
/// utterances; a model failure or timeout is unclear (fail closed).
async fn judge_heard(brain: &dyn Brain, outcome: ListenOutcome, spoken: &[&str]) -> Heard {
    match outcome {
        ListenOutcome::Utterance(text) if text.trim().is_empty() => {
            Heard::Decided(Decision::Declined(DeclineReason::Silence))
        }
        ListenOutcome::Utterance(text) => match forced_local_verdict(&text, spoken) {
            Verdict::Yes => Heard::Decided(Decision::Approved),
            Verdict::No => Heard::Decided(Decision::Declined(DeclineReason::No)),
            Verdict::Unclear => judge_unclear(brain, &text, spoken).await,
        },
        other => Heard::Decided(decision_for(other)),
    }
}

/// The model step for a locally unclear answer. A blocked answer is never sent to the model.
async fn judge_unclear(brain: &dyn Brain, text: &str, spoken: &[&str]) -> Heard {
    if judge_gate(text, spoken) == Gate::Block {
        return Heard::Unclear;
    }
    let judged = match tokio::time::timeout(JUDGE_DEADLINE, brain.judge_answer(text)).await {
        Ok(judgement) => judgement,
        Err(_) => AnswerJudgement::Unavailable,
    };
    match judged {
        AnswerJudgement::Approve => Heard::Decided(Decision::Approved),
        AnswerJudgement::Decline => Heard::Decided(Decision::Declined(DeclineReason::No)),
        AnswerJudgement::Unclear | AnswerJudgement::Unavailable => Heard::Unclear,
    }
}

/// The content-free outcome word written to the log.
fn outcome_word(decision: &Decision) -> &'static str {
    match decision {
        Decision::Approved => "approved",
        Decision::Declined(DeclineReason::No) => "declined(no)",
        Decision::Declined(DeclineReason::Unclear) => "declined(unclear)",
        Decision::Declined(_) => "declined",
    }
}

/// Asks a forced question and decides it (DESIGN D1-D4). The question is spoken and one answer
/// is heard. An unclear answer is re-asked once, when `reask` is given; the same pipeline then
/// judges the second answer without a second re-ask. Silence, errors, cancellation, barge-in and
/// rejected voices decline at once, with no re-ask and no model call.
#[allow(clippy::too_many_arguments)]
pub async fn confirm_forced_natural(
    ears: &dyn Ears,
    mouth: &dyn Mouth,
    announcer: &Announcer,
    brain: &dyn Brain,
    question: &str,
    reask: Option<&str>,
    cfg: &ConfirmConfig,
    chat: Option<&dyn Reporter>,
) -> Decision {
    let decision = decide(ears, mouth, announcer, brain, question, reask, cfg, chat).await;
    log::info!("forced confirm: {}", outcome_word(&decision));
    decision
}

#[allow(clippy::too_many_arguments)]
async fn decide(
    ears: &dyn Ears,
    mouth: &dyn Mouth,
    announcer: &Announcer,
    brain: &dyn Brain,
    question: &str,
    reask: Option<&str>,
    cfg: &ConfirmConfig,
    chat: Option<&dyn Reporter>,
) -> Decision {
    let mut spoken: Vec<&str> = vec![question];
    if let Some(text) = reask {
        spoken.push(text);
    }
    let first = match ask_and_listen(ears, mouth, announcer, question, cfg, chat).await {
        Ok(outcome) => outcome,
        Err(reason) => return Decision::Declined(reason),
    };
    if let Heard::Decided(decision) = judge_heard(brain, first, &spoken).await {
        return decision;
    }
    let Some(again) = reask else {
        return Decision::Declined(DeclineReason::Unclear);
    };
    log::info!("forced confirm: reask");
    let second = match ask_and_listen(ears, mouth, announcer, again, cfg, chat).await {
        Ok(outcome) => outcome,
        Err(reason) => return Decision::Declined(reason),
    };
    // Nothing heard after the re-ask is still an unclear answer to the question.
    if matches!(second, ListenOutcome::NoSpeech | ListenOutcome::TooLong) {
        return Decision::Declined(DeclineReason::Unclear);
    }
    match judge_heard(brain, second, &spoken).await {
        Heard::Decided(decision) => decision,
        Heard::Unclear => Decision::Declined(DeclineReason::Unclear),
    }
}
