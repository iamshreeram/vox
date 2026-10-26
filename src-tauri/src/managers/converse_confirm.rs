//! Spoken confirmation gate and the typed action preparation that feeds it
//! (M3).

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::managers::chat_log::ChatRole;
use crate::managers::command_router::CommandAction;
use crate::managers::converse::{
    Ears, Hands, ListenOutcome, Mouth, QuietOutcome, Reporter, SayOutcome,
};
use crate::managers::converse_agent::plan_confirmation_prompt;
#[cfg(test)]
pub use crate::managers::converse_plan::JobRequest;
use crate::managers::converse_plan::{
    canonical_agent, JobMode, JobSelection, PlanKind, TurnPlan, ASK_NOT_ENABLED_SPEECH,
    UNKNOWN_AGENT_SPEECH,
};
use crate::managers::emotion::Emotion;
use crate::managers::job_announcer::Announcer;
use crate::managers::redact::redact;
use crate::settings::{clamp_job_timeout_minutes, AppSettings};

/// How long to wait for a speaker that still reports speech, before giving up.
const STILL_SPEAKING_RETRY: Duration = Duration::from_secs(1);

/// Whole-utterance phrases that approve (matched on normalized word keys).
const YES_PHRASES: &[&str] = &[
    "yes",
    "yeah",
    "yep",
    "yup",
    "sure",
    "ok",
    "okay",
    "do it",
    "go ahead",
    "please do",
    "yes please",
    "yes do it",
    "confirm",
    "confirmed",
    "approved",
];

/// Whole-utterance phrases that refuse (matched on normalized word keys).
const NO_PHRASES: &[&str] = &[
    "no",
    "nope",
    "nah",
    "stop",
    "cancel",
    "dont",
    "do not",
    "dont do it",
    "never mind",
    "nevermind",
    "wait",
    "abort",
    "negative",
    "no thanks",
    "no thank you",
];

/// Word key, identical to `converse_local::word_key`: lowercase, with ASCII
/// punctuation and U+2019 removed.
fn word_key(raw: &str) -> String {
    raw.chars()
        .filter(|c| !c.is_ascii_punctuation() && *c != '\u{2019}')
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Handle type for a pinned project directory (unix: an owned descriptor).
#[cfg(unix)]
pub type DirFd = std::os::fd::OwnedFd;

/// Handle type for a pinned project directory (non-unix placeholder).
#[cfg(not(unix))]
pub struct DirFd;

/// Non-unix twin of the descriptor conversion: there is no pinned handle off unix, so the
/// opened directory is dropped and the placeholder is returned.
#[cfg(not(unix))]
impl From<std::fs::File> for DirFd {
    fn from(file: std::fs::File) -> Self {
        drop(file);
        DirFd
    }
}

/// Classification of a spoken yes/no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// The user approved.
    Yes,
    /// The user refused.
    No,
    /// The answer was not clear.
    Unclear,
}

/// Classifies a transcribed answer into a [`Verdict`].
///
/// The whole utterance must equal a phrase after word-key normalization; there is
/// no prefix or substring matching, so mixed answers are `Unclear`.
pub fn classify_confirmation(text: &str) -> Verdict {
    let key = text
        .split_whitespace()
        .map(word_key)
        .filter(|k| !k.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if key.is_empty() {
        Verdict::Unclear
    } else if YES_PHRASES.contains(&key.as_str()) {
        Verdict::Yes
    } else if NO_PHRASES.contains(&key.as_str()) {
        Verdict::No
    } else {
        Verdict::Unclear
    }
}

/// Why a confirmation did not approve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclineReason {
    /// The user said no.
    No,
    /// The answer was not clear.
    Unclear,
    /// Nothing was heard in time.
    Silence,
    /// The session was cancelled.
    Cancelled,
    /// A speech or capture error occurred.
    Error,
}

/// Outcome of a confirmation prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The user approved the action.
    Approved,
    /// The action was declined, with the reason.
    Declined(DeclineReason),
}

/// Timing knobs of the confirmation gate.
pub struct ConfirmConfig {
    /// How long to wait for the speaker to go quiet.
    pub quiet_wait: Duration,
    /// How long to listen for the answer.
    pub listen_timeout: Duration,
    /// Guard after speech ends, before the microphone is armed.
    pub post_speech_guard: Duration,
}

impl Default for ConfirmConfig {
    fn default() -> Self {
        ConfirmConfig {
            quiet_wait: Duration::from_secs(2),
            listen_timeout: Duration::from_secs(8),
            post_speech_guard: Duration::from_millis(350),
        }
    }
}

/// Asks a yes/no question aloud and waits for the answer.
///
/// Sequence: hold the announcer guard, speak the question, wait for quiet (one
/// retry), wait the post-speech guard, then arm exactly one listen bounded by
/// the listen timeout. Dropping the listen future cancels the capture.
#[cfg_attr(not(test), allow(dead_code))]
pub async fn confirm_action(
    ears: &dyn Ears,
    mouth: &dyn Mouth,
    announcer: &Announcer,
    question: &str,
    cfg: &ConfirmConfig,
) -> Decision {
    confirm_action_recorded(ears, mouth, announcer, question, cfg, None).await
}

/// `confirm_action` that also reports an accepted answer to the chat window (DESIGN-CHAT A2).
pub async fn confirm_action_recorded(
    ears: &dyn Ears,
    mouth: &dyn Mouth,
    announcer: &Announcer,
    question: &str,
    cfg: &ConfirmConfig,
    chat: Option<&dyn Reporter>,
) -> Decision {
    match ask_and_listen(ears, mouth, announcer, question, cfg, chat).await {
        Ok(outcome) => decision_for(outcome),
        Err(reason) => Decision::Declined(reason),
    }
}

/// Chat recording of a confirmation answer (DESIGN-CHAT A2, A11): only an accepted, non-empty
/// utterance, reported before any classification. Silence, noise and rejected voices are not.
fn record_answer(chat: Option<&dyn Reporter>, outcome: &ListenOutcome) {
    if let (Some(reporter), ListenOutcome::Utterance(text)) = (chat, outcome) {
        if !text.trim().is_empty() {
            reporter.chat(ChatRole::User, text);
        }
    }
}

/// Speaks `question` and listens once for the answer, with the timing of `confirm_action`.
/// `Err` when the question could not be spoken or no answer window opened.
async fn ask_and_listen(
    ears: &dyn Ears,
    mouth: &dyn Mouth,
    announcer: &Announcer,
    question: &str,
    cfg: &ConfirmConfig,
    chat: Option<&dyn Reporter>,
) -> Result<ListenOutcome, DeclineReason> {
    let _guard = announcer.confirmation_guard();
    match mouth.say(question, Emotion::Neutral).await {
        SayOutcome::Done => {}
        SayOutcome::Interrupted => return Err(DeclineReason::Cancelled),
        SayOutcome::Failed => return Err(DeclineReason::Error),
    }
    if !wait_until_quiet(mouth, cfg.quiet_wait).await {
        return Err(DeclineReason::Error);
    }
    tokio::time::sleep(cfg.post_speech_guard).await;
    match tokio::time::timeout(cfg.listen_timeout, ears.listen()).await {
        Ok(outcome) => {
            record_answer(chat, &outcome);
            Ok(outcome)
        }
        Err(_) => Err(DeclineReason::Silence),
    }
}

/// Strict classifier for forced confirmations (DESIGN 2.5, C6): true only when the utterance,
/// lower-cased, punctuation-stripped and whitespace-normalised, is exactly `yes`. Everything
/// else (`okay`, `sure`, `yes please`, mixed speech, silence) is not an approval.
pub fn classify_strict_yes(utterance: &str) -> bool {
    let key = utterance
        .split_whitespace()
        .map(word_key)
        .filter(|k| !k.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    key == "yes"
}

/// Asks a forced yes/no question (DESIGN 2.5, C6). The same say, quiet wait, guard and single
/// listen as `confirm_action`, but only an exact `yes` approves (`classify_strict_yes`). The
/// caller speaks the decline line; nothing runs unless this returns `Approved`.
pub async fn confirm_forced(
    ears: &dyn Ears,
    mouth: &dyn Mouth,
    announcer: &Announcer,
    question: &str,
    cfg: &ConfirmConfig,
    chat: Option<&dyn Reporter>,
) -> Decision {
    match ask_and_listen(ears, mouth, announcer, question, cfg, chat).await {
        Ok(ListenOutcome::Utterance(text)) if classify_strict_yes(&text) => Decision::Approved,
        Ok(ListenOutcome::Utterance(text)) if text.trim().is_empty() => {
            Decision::Declined(DeclineReason::Silence)
        }
        Ok(ListenOutcome::Utterance(_)) => Decision::Declined(DeclineReason::Unclear),
        Ok(other) => decision_for(other),
        Err(reason) => Decision::Declined(reason),
    }
}

/// The first `count` whitespace-separated words of `text`, joined by single spaces.
fn first_words(text: &str, count: usize) -> String {
    text.split_whitespace()
        .take(count)
        .collect::<Vec<_>>()
        .join(" ")
}

/// The exact question spoken before a planner-derived effect runs while the session is tainted
/// by an agent result (DESIGN 2.5, C6). `None` when the plan has no payload to describe: such a
/// plan performs no effect, so there is nothing to confirm.
pub fn forced_prompt(plan: &TurnPlan) -> Option<String> {
    match plan.kind {
        PlanKind::Command => {
            let command = plan.command.as_deref()?;
            Some(format!(
                "I am about to do this: {command}. Say yes to go ahead."
            ))
        }
        PlanKind::Paste => {
            let text = plan.paste_text.as_deref()?;
            let words = text.split_whitespace().count();
            let start = first_words(text, 12);
            Some(format!(
                "I am about to paste {words} words starting with: {start}. Say yes to go ahead."
            ))
        }
        PlanKind::Job => {
            let job = plan.job.as_ref()?;
            Some(format!(
                "I am about to start a coding job in {}: {}. Say yes to go ahead.",
                job.project, job.task
            ))
        }
        PlanKind::Agent => {
            let request = plan.agent.as_ref()?;
            Some(format!(
                "I am about to ask the agent: {}. Say yes to go ahead.",
                request.task
            ))
        }
        PlanKind::SetRole => {
            let directive = plan.role.as_ref()?;
            let start = first_words(&directive.role, 12);
            Some(format!(
                "I am about to take this role: {start}. Say yes to go ahead."
            ))
        }
        PlanKind::Plan => {
            let agent_plan = plan.plan.as_ref()?;
            Some(plan_confirmation_prompt(&agent_plan.steps))
        }
        _ => None,
    }
}

/// Waits for the speaker to go quiet, retrying once after a stop. `false` if it
/// is still speaking after the retry.
async fn wait_until_quiet(mouth: &dyn Mouth, quiet_wait: Duration) -> bool {
    if mouth.wait_quiet(quiet_wait).await == QuietOutcome::Quiet {
        return true;
    }
    mouth.stop();
    mouth.wait_quiet(STILL_SPEAKING_RETRY).await == QuietOutcome::Quiet
}

/// Maps the outcome of one listen to a confirmation decision.
fn decision_for(outcome: ListenOutcome) -> Decision {
    match outcome {
        ListenOutcome::Utterance(text) if text.trim().is_empty() => {
            Decision::Declined(DeclineReason::Silence)
        }
        ListenOutcome::Utterance(text) => match classify_confirmation(&text) {
            Verdict::Yes => Decision::Approved,
            Verdict::No => Decision::Declined(DeclineReason::No),
            Verdict::Unclear => Decision::Declined(DeclineReason::Unclear),
        },
        ListenOutcome::NoSpeech | ListenOutcome::TooLong => {
            Decision::Declined(DeclineReason::Silence)
        }
        // A voice the speaker filter rejected can never approve an action: it declines.
        ListenOutcome::Ignored => Decision::Declined(DeclineReason::Silence),
        ListenOutcome::Aborted => Decision::Declined(DeclineReason::Cancelled),
        ListenOutcome::Failed(_) => Decision::Declined(DeclineReason::Error),
    }
}

/// Which action paths the current settings allow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionPolicy {
    /// Whether the confirmation gate is active.
    pub gate_active: bool,
    /// Whether background jobs may be started.
    pub jobs_allowed: bool,
}

/// Derives the [`ActionPolicy`] from the settings.
///
/// Jobs need the bridge enabled, the jobs flag, a valid binary and a unix
/// platform. The gate is active when confirmation is on and tools or jobs are on.
pub fn effective_policy(settings: &AppSettings) -> ActionPolicy {
    let jobs_allowed = settings.agent_jobs_enabled
        && settings.agent_bridge_enabled
        && cfg!(unix)
        && settings
            .agent_bridge_binary_path
            .as_deref()
            .is_some_and(|p| binary_valid(Path::new(p)));
    ActionPolicy {
        gate_active: settings.agent_confirm_actions
            && (settings.agent_tools_enabled || settings.agent_jobs_enabled),
        jobs_allowed,
    }
}

/// Identity of a file on disk, used to detect replaced binaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileId {
    /// Device number.
    pub dev: u64,
    /// Inode number.
    pub ino: u64,
    /// Length in bytes.
    pub len: u64,
    /// Modification time in whole seconds.
    pub mtime_secs: i64,
}

/// Launch settings for the configured coding CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchConfig {
    /// Path to the coding CLI binary.
    pub binary: PathBuf,
    /// Identity of the binary at approval time.
    pub binary_id: FileId,
    /// Flag that precedes the task prompt.
    pub prompt_flag: String,
    /// Maximum run time of a job.
    pub timeout: Duration,
    /// M4: flag that passes the selected agent name. `Some` only for a selected agent.
    pub agent_flag: Option<String>,
}

/// M4: agent-selection parts of an approval, for a job with a selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFp {
    /// The selected agent (if any) is still in the allowlist.
    pub agent_ok: bool,
    /// Ask mode (if selected) is still allowed.
    pub ask_ok: bool,
    /// The agent flag in effect, when an agent is selected.
    pub agent_flag: Option<String>,
}

/// Snapshot of everything an approval depends on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalFingerprint {
    /// Gate state at approval time.
    pub gate_active: bool,
    /// Job permission at approval time.
    pub jobs_allowed: bool,
    /// Launch settings at approval time, if any.
    pub launch: Option<LaunchConfig>,
    /// M4: agent-selection state at approval time (`None` when not a selected job).
    pub job: Option<JobFp>,
}

/// Live source of the action policy and launch settings.
pub trait PolicySource: Send + Sync {
    /// The current action policy.
    fn current(&self) -> ActionPolicy;
    /// The current launch settings, if jobs are configured.
    fn launch_config(&self) -> Option<LaunchConfig>;
    /// M4: whether ask mode is currently allowed. Defaults to refused.
    fn ask_allowed(&self) -> bool {
        false
    }
    /// M4: the current agent allowlist. Defaults to empty (no agent selection).
    fn allowed_agents(&self) -> Vec<String> {
        Vec::new()
    }
    /// M4: the current agent flag. Defaults to `--agent`.
    fn agent_flag(&self) -> String {
        "--agent".to_string()
    }
    /// M4: launch settings for a selection. Sets `agent_flag` only when an agent is selected.
    fn launch_config_for(&self, sel: Option<&JobSelection>) -> Option<LaunchConfig> {
        let mut config = self.launch_config()?;
        config.agent_flag = sel
            .and_then(|s| s.agent.as_ref())
            .map(|_| self.agent_flag());
        Some(config)
    }
}

/// A model-proposed command that passed validation.
pub struct ValidatedCommand {
    /// Short label spoken in confirmations.
    pub label: String,
    /// The validated action (the value returned by `restrict_model_action`).
    pub(crate) action: CommandAction,
}

/// A fully prepared background job, ready to start.
pub struct JobLaunch {
    /// Project the job runs in.
    pub project: String,
    /// Pinned project directory handle.
    pub dir: DirFd,
    /// Task text for the coding CLI.
    pub task: String,
    /// Launch settings captured at preparation time.
    pub config: LaunchConfig,
    /// M4: the agent selection captured at parse time (default = no selection).
    pub selection: JobSelection,
}

/// An action that is validated and ready to execute after approval.
pub enum PreparedAction {
    /// Paste the given text.
    Paste {
        /// Text to paste.
        text: String,
    },
    /// Run a validated command.
    Command {
        /// The validated command.
        cmd: ValidatedCommand,
    },
    /// Start a background job.
    Job {
        /// The prepared launch.
        launch: JobLaunch,
    },
}

impl PreparedAction {
    /// M4: the job selection of a prepared job; `None` for paste and command actions.
    pub fn selection(&self) -> Option<&JobSelection> {
        match self {
            PreparedAction::Job { launch } => Some(&launch.selection),
            PreparedAction::Paste { .. } | PreparedAction::Command { .. } => None,
        }
    }
}

/// Why an action could not be prepared.
pub enum PrepareError {
    /// Refused, with fixed spoken text.
    Refused(String),
    /// The action kind is not supported.
    Unsupported,
}

/// Validates a plan and prepares the action it asks for.
///
/// The prepared payload is the one that is both presented and executed. Job
/// refusals are checked in this order: allowed, name, task, secret, launch
/// settings, then pinning the project directory.
pub async fn prepare_action(
    plan: &TurnPlan,
    policy: &dyn PolicySource,
    hands: &dyn Hands,
) -> Result<PreparedAction, PrepareError> {
    match plan.kind {
        PlanKind::Paste => prepare_paste(plan),
        PlanKind::Command => prepare_command(plan, hands),
        PlanKind::Job => prepare_job(plan, policy, hands),
        // Stage 1 roles: never prepared as actions (they are speech-only for now).
        PlanKind::Answer
        | PlanKind::Clarify
        | PlanKind::End
        | PlanKind::SetRole
        | PlanKind::ClearRole
        // Stage 2 (C3 wires the real behaviour): agent plans are never prepared as actions yet.
        | PlanKind::Agent
        // Stage 4 (S4 wires the executor): plans are never prepared as actions yet.
        | PlanKind::Plan
        | PlanKind::Resume => Err(PrepareError::Unsupported),
    }
}

fn refuse(text: &str) -> PrepareError {
    PrepareError::Refused(text.to_string())
}

fn prepare_paste(plan: &TurnPlan) -> Result<PreparedAction, PrepareError> {
    match plan.paste_text.as_deref() {
        Some(text) if !text.is_empty() => Ok(PreparedAction::Paste {
            text: text.to_string(),
        }),
        _ => Err(refuse("I don't have anything to type.")),
    }
}

fn prepare_command(plan: &TurnPlan, hands: &dyn Hands) -> Result<PreparedAction, PrepareError> {
    let validated = plan
        .command
        .as_deref()
        .and_then(|text| hands.validate_command(text))
        .ok_or_else(|| refuse("I can't do that one."))?;
    Ok(PreparedAction::Command {
        cmd: ValidatedCommand {
            label: sanitize_label(&validated.label),
            action: validated.action,
        },
    })
}

/// Keeps only `[A-Za-z0-9 ._-]`, at most 40 characters; `that` if nothing is left.
fn sanitize_label(raw: &str) -> String {
    let kept: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '.' | '_' | '-'))
        .take(LABEL_MAX_CHARS)
        .collect();
    if kept.trim().is_empty() {
        "that".to_string()
    } else {
        kept
    }
}

fn prepare_job(
    plan: &TurnPlan,
    policy: &dyn PolicySource,
    hands: &dyn Hands,
) -> Result<PreparedAction, PrepareError> {
    if !policy.current().jobs_allowed {
        return Err(refuse("Coding jobs aren't set up yet."));
    }
    let request = plan.job.as_ref();
    let project = request.map(|r| r.project.as_str()).unwrap_or("");
    if !is_valid_project_name(project) {
        return Err(refuse("I couldn't find that project."));
    }
    let task = normalize_task(request.map(|r| r.task.as_str()).unwrap_or(""))
        .ok_or_else(|| refuse(TASK_TOO_LONG))?;
    if redact(&task) != task {
        return Err(refuse(
            "The task contains something that looks like a secret, so I didn't start it.",
        ));
    }
    let selection = job_selection(request, policy)?;
    let config = policy
        .launch_config_for(Some(&selection))
        .ok_or_else(|| refuse("Coding jobs aren't set up yet."))?;
    let dir = hands.pin_project(project).map_err(|err| match err {
        PinError::Unknown | PinError::Ambiguous => refuse("I couldn't find that project."),
        PinError::Unavailable => refuse("Coding jobs aren't set up yet."),
    })?;
    Ok(PreparedAction::Job {
        launch: JobLaunch {
            project: project.to_string(),
            dir,
            task,
            config,
            selection,
        },
    })
}

/// M4 (D3): re-validates the planner's mode and agent against the CURRENT settings.
/// Ask needs ask to be allowed; a named agent must still be on the allowlist (matched
/// ASCII case-insensitively, the canonical spelling is stored). Refusals never echo the name.
fn job_selection(
    request: Option<&crate::managers::converse_plan::JobRequest>,
    policy: &dyn PolicySource,
) -> Result<JobSelection, PrepareError> {
    let mode = request.map_or(JobMode::Task, |r| r.mode);
    if mode == JobMode::Ask && !policy.ask_allowed() {
        return Err(refuse(ASK_NOT_ENABLED_SPEECH));
    }
    let agent = match request.and_then(|r| r.agent.as_deref()) {
        None => None,
        Some(name) => Some(
            canonical_agent(name, &policy.allowed_agents())
                .ok_or_else(|| refuse(UNKNOWN_AGENT_SPEECH))?,
        ),
    };
    Ok(JobSelection { agent, mode })
}

const LABEL_MAX_CHARS: usize = 40;
const TASK_MAX_CHARS: usize = 300;
const TASK_TOO_LONG: &str = "That task is too long to confirm by voice. Please shorten it.";

/// Project names: 1 to 64 characters from `[A-Za-z0-9._-]`.
fn is_valid_project_name(name: &str) -> bool {
    let len = name.chars().count();
    (1..=64).contains(&len)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

/// Newlines and tabs become spaces, the text is trimmed, and it must be 1 to 300
/// characters. `None` when the length is out of range.
fn normalize_task(raw: &str) -> Option<String> {
    let spaced: String = raw
        .chars()
        .map(|c| {
            if matches!(c, '\n' | '\r' | '\t') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let task = spaced.trim().to_string();
    let len = task.chars().count();
    (1..=TASK_MAX_CHARS).contains(&len).then_some(task)
}

/// Captures the approval fingerprint from a policy source (no job selection).
/// Production paths use `fingerprint_for`; this stays for the M3 contract and its tests.
#[allow(dead_code)] // M3 entry point kept for the fingerprint contract tests
pub fn fingerprint_of(policy: &dyn PolicySource) -> ApprovalFingerprint {
    fingerprint_for(policy, None)
}

/// M4 (D3): captures the approval fingerprint, including the agent selection of a job.
///
/// `sel = None` leaves the job part empty. A default job (no agent, task mode) gets the
/// constant part, so unrelated allowlist, flag or ask edits never change it. A selected
/// agent records whether it is still allowed and the flag in effect; Ask records whether
/// ask is still allowed.
pub fn fingerprint_for(
    policy: &dyn PolicySource,
    sel: Option<&JobSelection>,
) -> ApprovalFingerprint {
    let current = policy.current();
    ApprovalFingerprint {
        gate_active: current.gate_active,
        jobs_allowed: current.jobs_allowed,
        launch: policy.launch_config(),
        job: sel.map(|sel| job_fingerprint(policy, sel)),
    }
}

/// M4 (D3): the agent-selection part of an approval for one job selection.
fn job_fingerprint(policy: &dyn PolicySource, sel: &JobSelection) -> JobFp {
    let agent_ok = match sel.agent.as_deref() {
        None => true,
        Some(name) => canonical_agent(name, &policy.allowed_agents()).is_some(),
    };
    let ask_ok = sel.mode == JobMode::Task || policy.ask_allowed();
    JobFp {
        agent_ok,
        ask_ok,
        agent_flag: sel.agent.as_ref().map(|_| policy.agent_flag()),
    }
}

/// Why a project could not be pinned.
#[derive(Debug)]
pub enum PinError {
    /// The name matched no configured project.
    Unknown,
    /// The name matched more than one project.
    Ambiguous,
    /// Pinning is not available on this platform or configuration.
    Unavailable,
}

/// Outcome of asking to start a background job.
pub enum JobStartOutcome {
    /// Started; carries the job id.
    Started(u64),
    /// Another job is already active.
    Busy,
    /// Jobs are not configured.
    NotConfigured,
    /// The project name is unknown.
    // Part of the Hands contract (the gate speaks it); the real hands refuse an
    // unknown project earlier, while preparing the action.
    #[allow(dead_code)]
    UnknownProject,
    /// Starting jobs is not supported here.
    Unsupported,
    /// Refused, with fixed spoken text.
    Refused(String),
}

/// Fixed question spoken before a prepared action runs.
///
/// Only validated label, project and task text is interpolated.
pub fn confirmation_question(action: &PreparedAction) -> String {
    match action {
        PreparedAction::Paste { .. } => {
            "I'll type that into the active app. Say yes or no.".to_string()
        }
        PreparedAction::Command { cmd } => {
            format!("I'll open {}. Say yes or no.", cmd.label)
        }
        PreparedAction::Job { launch } => job_question(launch),
    }
}

/// M4 (D3): the job question. Without an agent and in task mode this is exactly the M3
/// wording. Names and text are interpolated in full, as validated.
fn job_question(launch: &JobLaunch) -> String {
    let agent = launch.selection.agent.as_deref();
    match (launch.selection.mode, agent) {
        (JobMode::Task, None) => format!(
            "I'll start a coding job in {}. The task is: {}. Say yes or no.",
            launch.project, launch.task
        ),
        (JobMode::Task, Some(name)) => format!(
            "I'll start a coding job in {} with the {name} agent. The task is: {}. Say yes or no.",
            launch.project, launch.task
        ),
        (JobMode::Ask, agent) => format!(
            "I'll ask the {} about {}: {}. It can change files. Say yes or no.",
            agent.map_or_else(
                || "coding agent".to_string(),
                |name| format!("{name} agent")
            ),
            launch.project,
            launch.task
        ),
    }
}

/// Fixed spoken reply after a decline.
pub fn decline_message(reason: DeclineReason) -> &'static str {
    match reason {
        DeclineReason::No | DeclineReason::Silence | DeclineReason::Cancelled => {
            "Okay, I won't do that."
        }
        DeclineReason::Unclear => "I didn't catch a clear yes, so I didn't do it.",
        DeclineReason::Error => "Something went wrong, so I didn't do it.",
    }
}

/// True when `path` is an absolute, existing, executable regular file.
///
/// Always false on non-unix platforms.
pub fn binary_valid(path: &Path) -> bool {
    if !cfg!(unix) || !path.is_absolute() {
        return false;
    }
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && is_executable(&meta),
        Err(_) => false,
    }
}

#[cfg(unix)]
fn is_executable(meta: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    meta.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_meta: &std::fs::Metadata) -> bool {
    false
}

/// Launch settings from the current settings, or `None` if the binary is not valid.
///
/// The timeout uses the clamped job minutes; the file identity pins the binary
/// so a replaced file is detected before dispatch.
pub fn launch_config_from_settings(settings: &AppSettings) -> Option<LaunchConfig> {
    let binary = PathBuf::from(settings.agent_bridge_binary_path.as_deref()?);
    if !binary_valid(&binary) {
        return None;
    }
    let meta = std::fs::metadata(&binary).ok()?;
    let binary_id = file_id(&meta)?;
    let minutes = clamp_job_timeout_minutes(settings.agent_jobs_timeout_minutes);
    Some(LaunchConfig {
        binary,
        binary_id,
        prompt_flag: crate::settings::effective_prompt_flag(settings),
        timeout: Duration::from_secs(u64::from(minutes) * 60),
        agent_flag: None,
    })
}

#[cfg(unix)]
fn file_id(meta: &std::fs::Metadata) -> Option<FileId> {
    use std::os::unix::fs::MetadataExt;
    Some(FileId {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime_secs: meta.mtime(),
    })
}

#[cfg(not(unix))]
fn file_id(_meta: &std::fs::Metadata) -> Option<FileId> {
    None
}

#[cfg(test)]
#[path = "converse_confirm_tests.rs"]
mod converse_confirm_tests;

#[cfg(test)]
#[path = "converse_confirm_adapter_tests.rs"]
mod converse_confirm_adapter_tests;

#[cfg(test)]
#[path = "converse_confirm_m4_tests.rs"]
mod converse_confirm_m4_tests;
