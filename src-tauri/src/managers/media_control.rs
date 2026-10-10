//! Voice media controls ("pause", "next track", "volume 40"): a direct-action
//! fast path with no LLM. Whole-utterance phrases only -- substring matching is
//! forbidden so ordinary dictation containing these words is never swallowed.
//!
//! The one unavoidable collision (a standalone dictated "pause"/"skip"/"mute")
//! is documented in the setting description; the feature is opt-in and only
//! effective while voice commands are enabled.
//!
//! Execution is orchestrated from Rust through a [`ScriptRunner`] (fakeable)
//! using fixed `/usr/bin/osascript` scripts. Only a validated 0..=100 integer
//! is ever interpolated into a script -- never user text. The final player
//! command is guarded so it does not launch an app that is not running; the
//! residual window (a player quitting between the guard and the verb) is
//! accepted and documented.

#![allow(dead_code)]

use crate::managers::voice_common::{normalize_phrase, HookEvent, VoiceHookOutcome};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaAction {
    Play,
    Pause,
    Next,
    Previous,
    VolumeUp,
    VolumeDown,
    Mute,
    Unmute,
    SetVolume(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Player {
    Spotify,
    Music,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerState {
    Playing,
    Paused,
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaError {
    /// Neither Spotify nor Music is running.
    NoPlayer,
    /// macOS automation permission was denied (osascript error -1743).
    PermissionDenied,
    Timeout,
    Failed(String),
    Unsupported,
}

pub trait ScriptRunner: Send + Sync {
    fn run(&self, script: &str) -> Result<String, MediaError>;
}

/// Volume change per "louder"/"quieter".
pub const VOLUME_STEP: u8 = 10;
/// Per-stream output retention cap for the subprocess runner.
pub const OUTPUT_CAP_BYTES: usize = 64 * 1024;
/// Whole-run deadline for one osascript invocation.
pub const SCRIPT_DEADLINE: Duration = Duration::from_secs(5);

/// Parses a whole utterance into a media action. `None` for anything that is
/// not EXACTLY one of the supported phrases after [`normalize_phrase`].
pub fn parse_media_command(_text: &str) -> Option<MediaAction> {
    let _ = normalize_phrase;
    todo!("F3: implement")
}

/// Gate: media controls only act when BOTH the master voice-commands switch
/// and the media-controls switch are on.
pub fn decide_media_command(
    _media_enabled: bool,
    _voice_commands_enabled: bool,
    _text: &str,
) -> Option<MediaAction> {
    todo!("F3: implement")
}

/// `states` lists only RUNNING players in discovery order (Spotify, Music).
/// Pause/Next/Previous prefer the first PLAYING player, else the first
/// running one; Play picks the first running one. Volume/mute actions have no
/// player and return `None`.
pub fn choose_player(_states: &[(Player, PlayerState)], _action: MediaAction) -> Option<Player> {
    todo!("F3: implement")
}

pub fn discover_script() -> &'static str {
    todo!("F3: implement")
}
pub fn state_script(_player: Player) -> String {
    todo!("F3: implement")
}
/// Guarded player command. For `Player::Spotify` + pause this MUST be exactly:
/// `if application "Spotify" is running then\n    tell application "Spotify" to pause\nend if`
/// Verbs: play, pause, next track, previous track.
pub fn verb_script(_player: Player, _action: MediaAction) -> String {
    todo!("F3: implement")
}
pub fn volume_get_script() -> &'static str {
    todo!("F3: implement")
}
pub fn volume_set_script(_level: u8) -> String {
    todo!("F3: implement")
}
pub fn mute_script(_muted: bool) -> String {
    todo!("F3: implement")
}

/// Maps osascript stderr to an error: `-1743` / "not allowed" => PermissionDenied.
pub fn map_script_error(_stderr: &str) -> MediaError {
    todo!("F3: implement")
}

/// Runs `action` through `runner`; returns a short description on success.
pub fn execute_media(_action: MediaAction, _runner: &dyn ScriptRunner) -> Result<String, MediaError> {
    todo!("F3: implement")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exit_code: Option<i32>,
}

/// Runs `program args...` with a hard deadline. stdout and stderr are drained
/// CONCURRENTLY (so a chatty child can never block on a full pipe); each is
/// capped at `cap` bytes with the excess read and discarded. On deadline the
/// child is killed and reaped and `Err(MediaError::Timeout)` is returned
/// within the deadline plus a small tolerance EVEN IF a descendant keeps the
/// pipes open -- reader threads are detached rather than joined without bound.
pub fn run_command_with_deadline(
    _program: &Path,
    _args: &[&str],
    _deadline: Duration,
    _cap: usize,
) -> Result<CommandOutput, MediaError> {
    todo!("F3: implement")
}

/// Production runner: fixed `/usr/bin/osascript -e <script>` on macOS,
/// `Err(Unsupported)` elsewhere.
pub struct OsascriptRunner;

impl ScriptRunner for OsascriptRunner {
    fn run(&self, _script: &str) -> Result<String, MediaError> {
        todo!("F3: implement")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaHookResult {
    pub outcome: VoiceHookOutcome,
    pub event: HookEvent,
}

/// The whole hook minus Tauri: gates, parse, execute. `None` => not a media
/// command (caller continues to the router). `Some` => the utterance is
/// consumed (`VoiceHookOutcome::HANDLED`) whether the run succeeded
/// (`HookEvent::Executed`) or failed (`HookEvent::Error`).
pub fn run_media_hook(
    _media_enabled: bool,
    _voice_commands_enabled: bool,
    _text: &str,
    _runner: &dyn ScriptRunner,
) -> Option<MediaHookResult> {
    let _ = (VoiceHookOutcome::HANDLED, HookEvent::Executed(String::new()));
    todo!("F3: implement")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Instant;

    // ---------- parse_media_command ----------

    fn parses(text: &str, expected: MediaAction) {
        assert_eq!(parse_media_command(text), Some(expected), "input {text:?}");
    }
    fn rejects(text: &str) {
        assert_eq!(parse_media_command(text), None, "input {text:?}");
    }

    #[test]
    fn exact_phrases_map_to_actions() {
        parses("pause", MediaAction::Pause);
        parses("pause music", MediaAction::Pause);
        parses("play", MediaAction::Play);
        parses("play music", MediaAction::Play);
        parses("resume", MediaAction::Play);
        parses("resume music", MediaAction::Play);
        parses("next track", MediaAction::Next);
        parses("next song", MediaAction::Next);
        parses("skip", MediaAction::Next);
        parses("previous track", MediaAction::Previous);
        parses("previous song", MediaAction::Previous);
        parses("volume up", MediaAction::VolumeUp);
        parses("louder", MediaAction::VolumeUp);
        parses("volume down", MediaAction::VolumeDown);
        parses("quieter", MediaAction::VolumeDown);
        parses("mute", MediaAction::Mute);
        parses("unmute", MediaAction::Unmute);
    }

    #[test]
    fn normalization_is_applied_before_matching() {
        parses("  Pause Music.  ", MediaAction::Pause);
        parses("PLEASE PAUSE!", MediaAction::Pause);
        parses("please   skip", MediaAction::Next);
        parses("Next   Track?", MediaAction::Next);
        rejects("pleasepause");
        rejects("please, pause");
    }

    #[test]
    fn substrings_and_extra_words_never_match() {
        rejects("play the song despacito");
        rejects("pause for a moment while I think");
        rejects("could you pause");
        rejects("pause the music please now");
        rejects("skip this paragraph");
        rejects("mute the microphone");
        rejects("turn the volume up");
        rejects("next track please thanks");
        rejects("volume");
        rejects("play play");
    }

    #[test]
    fn volume_with_digits_and_words() {
        parses("volume 50", MediaAction::SetVolume(50));
        parses("set volume to 50", MediaAction::SetVolume(50));
        parses("set volume to 0", MediaAction::SetVolume(0));
        parses("volume 100", MediaAction::SetVolume(100));
        parses("volume 7", MediaAction::SetVolume(7));
        parses("set volume to fifty", MediaAction::SetVolume(50));
        parses("set volume to twenty five", MediaAction::SetVolume(25));
        parses("volume ninety nine", MediaAction::SetVolume(99));
        parses("volume one hundred", MediaAction::SetVolume(100));
        parses("volume hundred", MediaAction::SetVolume(100));
        parses("volume zero", MediaAction::SetVolume(0));
        parses("volume twenty", MediaAction::SetVolume(20));
        parses("volume thirteen", MediaAction::SetVolume(13));
    }

    #[test]
    fn malformed_or_out_of_range_volume_is_rejected() {
        rejects("volume 101");
        rejects("volume 150");
        rejects("volume 1000");
        rejects("volume -5");
        rejects("volume +5");
        rejects("volume 5.5");
        rejects("volume 50%");
        rejects("volume 5,0");
        rejects("volume 050a");
        rejects("volume five five");
        rejects("volume twenty twenty");
        rejects("volume fifty five five");
        rejects("volume one hundred one");
        rejects("volume ");
        rejects("set volume to");
        rejects("volume 0x10");
        rejects("volume ５０"); // full-width digits
    }

    #[test]
    fn empty_unicode_and_huge_inputs_do_not_panic() {
        rejects("");
        rejects("   ");
        rejects("暂停");
        rejects("");
        rejects(&"pause ".repeat(5_000));
        rejects(&"é".repeat(10_000));
    }

    #[test]
    fn decide_requires_both_switches() {
        assert_eq!(decide_media_command(true, true, "pause"), Some(MediaAction::Pause));
        assert_eq!(decide_media_command(false, true, "pause"), None);
        assert_eq!(decide_media_command(true, false, "pause"), None);
        assert_eq!(decide_media_command(false, false, "pause"), None);
        assert_eq!(decide_media_command(true, true, "open safari"), None);
    }

    // ---------- choose_player ----------

    use Player::{Music, Spotify};
    use PlayerState::{Paused, Playing, Stopped};

    #[test]
    fn choose_player_prefers_the_playing_one_for_pause_next_previous() {
        let both = [(Spotify, Paused), (Music, Playing)];
        assert_eq!(choose_player(&both, MediaAction::Pause), Some(Music));
        assert_eq!(choose_player(&both, MediaAction::Next), Some(Music));
        assert_eq!(choose_player(&both, MediaAction::Previous), Some(Music));
    }

    #[test]
    fn choose_player_falls_back_to_first_running_in_discovery_order() {
        let both = [(Spotify, Paused), (Music, Stopped)];
        assert_eq!(choose_player(&both, MediaAction::Pause), Some(Spotify));
        assert_eq!(choose_player(&[(Music, Paused)], MediaAction::Next), Some(Music));
    }

    #[test]
    fn choose_player_tie_between_two_playing_is_spotify() {
        let both = [(Spotify, Playing), (Music, Playing)];
        assert_eq!(choose_player(&both, MediaAction::Pause), Some(Spotify));
    }

    #[test]
    fn choose_player_for_play_is_first_running_regardless_of_state() {
        assert_eq!(
            choose_player(&[(Spotify, Stopped), (Music, Playing)], MediaAction::Play),
            Some(Spotify)
        );
        assert_eq!(choose_player(&[(Music, Paused)], MediaAction::Play), Some(Music));
    }

    #[test]
    fn choose_player_none_when_nothing_running_or_not_a_player_action() {
        assert_eq!(choose_player(&[], MediaAction::Pause), None);
        assert_eq!(choose_player(&[], MediaAction::Play), None);
        assert_eq!(choose_player(&[(Spotify, Playing)], MediaAction::VolumeUp), None);
        assert_eq!(choose_player(&[(Spotify, Playing)], MediaAction::Mute), None);
        assert_eq!(choose_player(&[(Spotify, Playing)], MediaAction::SetVolume(5)), None);
    }

    // ---------- script builders ----------

    #[test]
    fn guarded_verb_scripts_are_exact() {
        assert_eq!(
            verb_script(Spotify, MediaAction::Pause),
            "if application \"Spotify\" is running then\n    tell application \"Spotify\" to pause\nend if"
        );
        assert_eq!(
            verb_script(Music, MediaAction::Play),
            "if application \"Music\" is running then\n    tell application \"Music\" to play\nend if"
        );
        assert!(verb_script(Spotify, MediaAction::Next).contains("to next track"));
        assert!(verb_script(Music, MediaAction::Previous).contains("to previous track"));
    }

    #[test]
    fn volume_and_mute_scripts_interpolate_only_integers() {
        assert_eq!(volume_set_script(40), "set volume output volume 40");
        assert_eq!(volume_set_script(0), "set volume output volume 0");
        assert_eq!(volume_set_script(100), "set volume output volume 100");
        assert_eq!(mute_script(true), "set volume output muted true");
        assert_eq!(mute_script(false), "set volume output muted false");
        assert_eq!(volume_get_script(), "output volume of (get volume settings)");
        assert!(discover_script().contains("System Events"));
        assert!(discover_script().contains("\"Spotify\""));
        assert!(discover_script().contains("\"Music\""));
        assert_eq!(
            state_script(Spotify),
            "tell application \"Spotify\" to get player state as text"
        );
    }

    #[test]
    fn map_script_error_detects_permission_denial() {
        assert_eq!(
            map_script_error("execution error: Not authorized to send Apple events to Spotify. (-1743)"),
            MediaError::PermissionDenied
        );
        assert_eq!(
            map_script_error("osascript is not allowed assistive access"),
            MediaError::PermissionDenied
        );
        match map_script_error("syntax error (-2740)") {
            MediaError::Failed(message) => assert!(message.contains("syntax error")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // ---------- execute_media with a scripted fake runner ----------

    #[derive(Default)]
    struct FakeRunner {
        replies: Mutex<HashMap<String, Result<String, MediaError>>>,
        calls: Mutex<Vec<String>>,
    }
    impl FakeRunner {
        fn reply(self, script: String, result: Result<String, MediaError>) -> Self {
            self.replies.lock().unwrap().insert(script, result);
            self
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl ScriptRunner for FakeRunner {
        fn run(&self, script: &str) -> Result<String, MediaError> {
            self.calls.lock().unwrap().push(script.to_string());
            self.replies
                .lock()
                .unwrap()
                .get(script)
                .cloned()
                .unwrap_or_else(|| Err(MediaError::Failed(format!("unscripted: {script}"))))
        }
    }

    #[test]
    fn pause_targets_the_playing_player_and_runs_exactly_one_verb() {
        let runner = FakeRunner::default()
            .reply(discover_script().to_string(), Ok("Spotify, Music".into()))
            .reply(state_script(Spotify), Ok("paused".into()))
            .reply(state_script(Music), Ok("playing".into()))
            .reply(verb_script(Music, MediaAction::Pause), Ok(String::new()));
        let description = execute_media(MediaAction::Pause, &runner).expect("pause succeeds");
        assert!(description.to_lowercase().contains("music"), "{description}");
        let calls = runner.calls();
        assert_eq!(calls.last().unwrap(), &verb_script(Music, MediaAction::Pause));
        assert_eq!(
            calls.iter().filter(|c| c.contains("to pause")).count(),
            1,
            "exactly one verb script runs: {calls:?}"
        );
    }

    #[test]
    fn no_running_player_is_an_error_and_runs_no_verb() {
        let runner = FakeRunner::default().reply(discover_script().to_string(), Ok(String::new()));
        assert_eq!(execute_media(MediaAction::Play, &runner), Err(MediaError::NoPlayer));
        assert_eq!(runner.calls().len(), 1, "only discovery ran");
    }

    #[test]
    fn unknown_state_text_is_treated_as_stopped_not_a_failure() {
        let runner = FakeRunner::default()
            .reply(discover_script().to_string(), Ok("Music".into()))
            .reply(state_script(Music), Ok("something odd".into()))
            .reply(verb_script(Music, MediaAction::Next), Ok(String::new()));
        assert!(execute_media(MediaAction::Next, &runner).is_ok());
    }

    #[test]
    fn permission_denial_and_timeout_propagate_distinctly() {
        let denied = FakeRunner::default()
            .reply(discover_script().to_string(), Err(MediaError::PermissionDenied));
        assert_eq!(execute_media(MediaAction::Pause, &denied), Err(MediaError::PermissionDenied));
        let timed_out =
            FakeRunner::default().reply(discover_script().to_string(), Err(MediaError::Timeout));
        assert_eq!(execute_media(MediaAction::Pause, &timed_out), Err(MediaError::Timeout));
    }

    #[test]
    fn volume_up_reads_then_sets_clamped_absolute_value() {
        let runner = FakeRunner::default()
            .reply(volume_get_script().to_string(), Ok("55\n".into()))
            .reply(volume_set_script(65), Ok(String::new()));
        assert!(execute_media(MediaAction::VolumeUp, &runner).is_ok());
        assert_eq!(runner.calls().last().unwrap(), &volume_set_script(65));

        let near_top = FakeRunner::default()
            .reply(volume_get_script().to_string(), Ok("95".into()))
            .reply(volume_set_script(100), Ok(String::new()));
        assert!(execute_media(MediaAction::VolumeUp, &near_top).is_ok());

        let near_bottom = FakeRunner::default()
            .reply(volume_get_script().to_string(), Ok("4".into()))
            .reply(volume_set_script(0), Ok(String::new()));
        assert!(execute_media(MediaAction::VolumeDown, &near_bottom).is_ok());
    }

    #[test]
    fn unparseable_current_volume_is_a_failure_not_a_blind_set() {
        let runner = FakeRunner::default().reply(volume_get_script().to_string(), Ok("loud".into()));
        assert!(matches!(
            execute_media(MediaAction::VolumeUp, &runner),
            Err(MediaError::Failed(_))
        ));
        assert_eq!(runner.calls().len(), 1, "no set script ran");
        let out_of_range = FakeRunner::default().reply(volume_get_script().to_string(), Ok("500".into()));
        assert!(matches!(
            execute_media(MediaAction::VolumeUp, &out_of_range),
            Err(MediaError::Failed(_))
        ));
    }

    #[test]
    fn set_volume_and_mute_run_a_single_script() {
        let runner = FakeRunner::default().reply(volume_set_script(33), Ok(String::new()));
        assert!(execute_media(MediaAction::SetVolume(33), &runner).is_ok());
        assert_eq!(runner.calls(), vec![volume_set_script(33)]);

        let mute = FakeRunner::default().reply(mute_script(true), Ok(String::new()));
        assert!(execute_media(MediaAction::Mute, &mute).is_ok());
        let unmute = FakeRunner::default().reply(mute_script(false), Ok(String::new()));
        assert!(execute_media(MediaAction::Unmute, &unmute).is_ok());
    }

    #[test]
    fn set_volume_above_100_is_refused_defensively() {
        let runner = FakeRunner::default();
        assert!(matches!(
            execute_media(MediaAction::SetVolume(101), &runner),
            Err(MediaError::Failed(_))
        ));
        assert!(runner.calls().is_empty());
    }

    // ---------- hook (executor-level: matched failures never paste/escalate) ----------

    #[test]
    fn hook_is_none_when_gated_off_or_not_a_media_phrase() {
        let runner = FakeRunner::default();
        assert_eq!(run_media_hook(false, true, "pause", &runner), None);
        assert_eq!(run_media_hook(true, false, "pause", &runner), None);
        assert_eq!(run_media_hook(true, true, "open safari", &runner), None);
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn hook_success_is_handled_with_executed_event() {
        let runner = FakeRunner::default().reply(mute_script(true), Ok(String::new()));
        let result = run_media_hook(true, true, "mute", &runner).expect("matched");
        assert_eq!(result.outcome, VoiceHookOutcome::HANDLED);
        assert!(matches!(result.event, HookEvent::Executed(_)));
    }

    #[test]
    fn hook_failure_is_still_handled_never_pastes_or_escalates() {
        for error in [
            MediaError::NoPlayer,
            MediaError::PermissionDenied,
            MediaError::Timeout,
            MediaError::Failed("boom".into()),
            MediaError::Unsupported,
        ] {
            let runner = FakeRunner::default().reply(mute_script(true), Err(error.clone()));
            let result = run_media_hook(true, true, "mute", &runner).expect("matched");
            assert!(result.outcome.skip_paste, "{error:?}");
            assert!(!result.outcome.escalate_to_agent, "{error:?}");
            assert!(matches!(result.event, HookEvent::Error(_)), "{error:?}");
        }
    }

    // ---------- real subprocess runner (unix) ----------

    #[cfg(unix)]
    mod subprocess {
        use super::*;
        use std::path::Path;

        const SH: &str = "/bin/sh";

        fn run(script: &str, deadline: Duration) -> (Result<CommandOutput, MediaError>, Duration) {
            let started = Instant::now();
            let result = run_command_with_deadline(Path::new(SH), &["-c", script], deadline, OUTPUT_CAP_BYTES);
            (result, started.elapsed())
        }

        #[test]
        fn captures_stdout_stderr_and_exit_code() {
            let (result, _) = run("printf out; printf err >&2; exit 3", Duration::from_secs(5));
            let output = result.expect("completes");
            assert_eq!(output.stdout, b"out");
            assert_eq!(output.stderr, b"err");
            assert_eq!(output.exit_code, Some(3));
        }

        #[test]
        fn large_stdout_does_not_deadlock_and_is_capped() {
            let (result, elapsed) = run(
                "head -c 300000 /dev/zero | tr '\\0' a",
                Duration::from_secs(5),
            );
            let output = result.expect("completes");
            assert_eq!(output.stdout.len(), OUTPUT_CAP_BYTES);
            assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
        }

        #[test]
        fn large_stderr_does_not_deadlock_and_is_capped() {
            let (result, _) = run(
                "head -c 300000 /dev/zero | tr '\\0' b >&2",
                Duration::from_secs(5),
            );
            let output = result.expect("completes");
            assert_eq!(output.stderr.len(), OUTPUT_CAP_BYTES);
        }

        #[test]
        fn large_output_on_both_streams_at_once_does_not_deadlock() {
            let (result, elapsed) = run(
                "head -c 300000 /dev/zero | tr '\\0' a & head -c 300000 /dev/zero | tr '\\0' b >&2 & wait",
                Duration::from_secs(5),
            );
            let output = result.expect("completes");
            assert_eq!(output.stdout.len(), OUTPUT_CAP_BYTES);
            assert_eq!(output.stderr.len(), OUTPUT_CAP_BYTES);
            assert!(elapsed < Duration::from_secs(4), "took {elapsed:?}");
        }

        #[test]
        fn a_slow_child_times_out_within_tolerance() {
            let (result, elapsed) = run("sleep 30", Duration::from_millis(300));
            assert_eq!(result, Err(MediaError::Timeout));
            assert!(elapsed < Duration::from_millis(1500), "took {elapsed:?}");
        }

        #[test]
        fn a_descendant_holding_the_pipes_cannot_defeat_the_deadline() {
            // The immediate child is killed but a grandchild keeps stdout/stderr
            // open for 5s; the runner must still return near the deadline.
            let (result, elapsed) = run("sleep 5 & wait", Duration::from_millis(300));
            assert_eq!(result, Err(MediaError::Timeout));
            assert!(elapsed < Duration::from_millis(1500), "took {elapsed:?}");
        }

        #[test]
        fn missing_program_is_a_failure_not_a_panic() {
            let result = run_command_with_deadline(
                Path::new("/definitely/not/a/real/program"),
                &[],
                Duration::from_secs(1),
                OUTPUT_CAP_BYTES,
            );
            assert!(matches!(result, Err(MediaError::Failed(_))));
        }
    }
}
