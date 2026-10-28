//! First-run setup (P4, DESIGN Rev 2 'First run (C4)', Rev 3/4 consent).
//!
//! A fresh install walks two steps in order: spoken replies (`Voice`), then the Brain
//! (`Brain`). Each step ends as `Done` or `Skipped`. A store that existed before this feature
//! never has setup pending, so an upgrade never sees the wizard. Nothing here touches the
//! post-processing or real-time settings.

use crate::managers::brain::{self, BrainFeature, Resolved};
use crate::settings::AppSettings;
use serde::{Deserialize, Serialize};
use specta::Type;

/// Shown when the Brain step is finished while the composer does not inherit the Brain.
const BRAIN_NOT_INHERITED: &str = "Use the Brain for spoken replies on the Brain page first.";

/// The first-run steps, in the order the user sees them.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Type)]
#[serde(rename_all = "snake_case")]
pub enum FirstRunStep {
    /// Spoken replies (text-to-speech).
    Voice,
    /// The Brain connection used for spoken replies.
    Brain,
}

/// How a first-run step was resolved. `Unset` means the user has not decided yet.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, Type)]
#[serde(rename_all = "snake_case")]
pub enum FirstRunOutcome {
    /// Not decided yet.
    #[default]
    Unset,
    /// The user skipped the step.
    Skipped,
    /// The user completed the step.
    Done,
}

/// The step the user should see next, or `None` when setup is not pending or is complete.
pub fn next_step(settings: &AppSettings) -> Option<FirstRunStep> {
    if !settings.first_run_setup_pending {
        return None;
    }
    if settings.first_run_tts == FirstRunOutcome::Unset {
        Some(FirstRunStep::Voice)
    } else if settings.first_run_brain == FirstRunOutcome::Unset {
        Some(FirstRunStep::Brain)
    } else {
        None
    }
}

/// Records the outcome of `step`. A refused step changes nothing. A step that is already
/// resolved is never changed again. Once no step remains, setup is no longer pending. When
/// setup is not pending this does nothing and returns `Ok`.
pub fn finish_step(
    settings: &mut AppSettings,
    step: FirstRunStep,
    outcome: FirstRunOutcome,
) -> Result<(), String> {
    if !settings.first_run_setup_pending {
        return Ok(());
    }
    let current = match step {
        FirstRunStep::Voice => settings.first_run_tts,
        FirstRunStep::Brain => settings.first_run_brain,
    };
    if outcome == FirstRunOutcome::Unset || current != FirstRunOutcome::Unset {
        return Ok(());
    }
    match (step, outcome) {
        (FirstRunStep::Voice, FirstRunOutcome::Done) => {
            settings.tts_enabled = true;
            settings.first_run_tts = FirstRunOutcome::Done;
        }
        (FirstRunStep::Voice, _) => settings.first_run_tts = outcome,
        (FirstRunStep::Brain, FirstRunOutcome::Done) => {
            require_brain_inherited(settings)?;
            settings.first_run_brain = FirstRunOutcome::Done;
        }
        (FirstRunStep::Brain, _) => settings.first_run_brain = outcome,
    }
    if next_step(settings).is_none() {
        settings.first_run_setup_pending = false;
    }
    Ok(())
}

/// Checks that spoken replies inherit a complete Brain connection with transcript consent for
/// the current destination. Uses the same rules as the Brain page.
fn require_brain_inherited(settings: &AppSettings) -> Result<(), String> {
    match brain::resolve(settings, BrainFeature::Composer) {
        Resolved::Brain(_) => Ok(()),
        Resolved::NotConfigured(missing) => Err(brain::missing_message(&missing).to_string()),
        Resolved::Own => Err(BRAIN_NOT_INHERITED.to_string()),
    }
}
