//! Pure helpers for the audio/paste side of conversation sessions. No tauri
//! types, no hardware: everything hardware-shaped is behind a trait so it is
//! unit-testable. Contract: DESIGN.md section 3.4 (Revision 4).

use crate::managers::command_router::{CommandAction, RouteDecision};
use std::time::Duration;

// ---------------------------------------------------------------------
// Endpointer
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EndpointConfig {
    /// Silence after speech that ends the utterance.
    pub silence_ms: u64,
    /// No speech this long after the FIRST fed level => NoSpeech.
    pub initial_grace_ms: u64,
    /// Utterance longer than this (from the first fed level) => MaxLength.
    pub max_utterance_ms: u64,
    /// Above-floor levels must persist this long to count as speech.
    pub min_speech_ms: u64,
    /// A level at or below this is silence.
    pub floor: f32,
}

impl EndpointConfig {
    pub fn with_silence(silence_ms: u64) -> Self {
        EndpointConfig {
            silence_ms,
            initial_grace_ms: 4000,
            max_utterance_ms: 30_000,
            min_speech_ms: 150,
            floor: 0.02,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointEvent {
    SpeechEnded,
    NoSpeech,
    MaxLength,
}

/// Decides when an utterance is over from a stream of level buckets.
///
/// Production signal: the VAD speech decision, fed via `feed_activity`
/// (a feed is "loud" when `speech_active` is true). `feed` stays for
/// level-based callers and tests: it is `feed_activity` of "any level > floor".
///
/// Rules (evaluated on every feed, in this order, until one event fires;
/// after an event every later feed returns `None`):
/// * the first feed fixes t0 (readiness-based clock);
/// * a loud feed is "loud", otherwise "silent";
///   speech has started once loud feeds have been continuous for at least
///   `min_speech_ms` (a silent feed before that resets the run);
/// * `SpeechEnded`: speech started and `silence_ms` elapsed since the last
///   loud feed;
/// * `NoSpeech`: speech not started and `initial_grace_ms` elapsed since t0;
/// * `MaxLength`: `max_utterance_ms` elapsed since t0.
pub struct Endpointer {
    cfg: EndpointConfig,
    /// Timestamp of the first feed; the clock origin for grace and max length.
    t0: Option<u64>,
    /// Timestamp of the first loud feed of the current loud run (reset by a
    /// silent feed while speech has not started).
    run_start: Option<u64>,
    /// Timestamp of the most recent loud feed.
    last_loud: u64,
    speech_started: bool,
    /// Set once an event has fired; every later feed returns `None`.
    finished: bool,
}

impl Endpointer {
    pub fn new(cfg: EndpointConfig) -> Self {
        Endpointer {
            cfg,
            t0: None,
            run_start: None,
            last_loud: 0,
            speech_started: false,
            finished: false,
        }
    }

    /// Level-based entry point (tests and level-only callers): a feed is loud
    /// when any level exceeds the configured floor.
    pub fn feed(&mut self, now_ms: u64, levels: &[f32]) -> Option<EndpointEvent> {
        self.feed_activity(now_ms, levels.iter().any(|&level| level > self.cfg.floor))
    }

    /// Production entry point: `speech_active` is the VAD speech decision for
    /// this feed. Same state machine as `feed`.
    pub fn feed_activity(&mut self, now_ms: u64, speech_active: bool) -> Option<EndpointEvent> {
        if self.finished {
            return None;
        }
        let t0 = *self.t0.get_or_insert(now_ms);
        let loud = speech_active;

        if loud {
            let start = *self.run_start.get_or_insert(now_ms);
            self.last_loud = now_ms;
            if !self.speech_started && now_ms.saturating_sub(start) >= self.cfg.min_speech_ms {
                self.speech_started = true;
            }
        } else if !self.speech_started {
            self.run_start = None;
        }

        let event = if self.speech_started
            && now_ms.saturating_sub(self.last_loud) >= self.cfg.silence_ms
        {
            Some(EndpointEvent::SpeechEnded)
        } else if !self.speech_started && now_ms.saturating_sub(t0) >= self.cfg.initial_grace_ms {
            Some(EndpointEvent::NoSpeech)
        } else if now_ms.saturating_sub(t0) >= self.cfg.max_utterance_ms {
            Some(EndpointEvent::MaxLength)
        } else {
            None
        };

        if event.is_some() {
            self.finished = true;
        }
        event
    }

    /// Only the tests read this; production decides end-of-speech via `poll`.
    #[cfg(test)]
    pub fn had_speech(&self) -> bool {
        self.speech_started
    }
}

// ---------------------------------------------------------------------
// Speech completion
// ---------------------------------------------------------------------

pub trait SpeechProbe: Send + Sync {
    fn is_current(&self, epoch: u64) -> bool;
    fn is_speaking(&self) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitOutcome {
    Done,
    Interrupted,
    TimedOut,
}

/// Poll until speech for `epoch` is over. Each poll: not current =>
/// `Interrupted` (checked FIRST), not speaking => `Done`, deadline passed =>
/// `TimedOut`; otherwise sleep `poll`.
pub async fn wait_for_speech_end(
    probe: &dyn SpeechProbe,
    epoch: u64,
    deadline: Duration,
    poll: Duration,
) -> WaitOutcome {
    let started = std::time::Instant::now();
    loop {
        if !probe.is_current(epoch) {
            return WaitOutcome::Interrupted;
        }
        if !probe.is_speaking() {
            return WaitOutcome::Done;
        }
        if started.elapsed() >= deadline {
            return WaitOutcome::TimedOut;
        }
        tokio::time::sleep(poll).await;
    }
}

// ---------------------------------------------------------------------
// Paste safety
// ---------------------------------------------------------------------

/// Terminal-like frontmost app (case-insensitive exact bundle id match).
pub fn is_terminal_like(bundle_id: &str) -> bool {
    const TERMINALS: [&str; 9] = [
        "com.apple.Terminal",
        "com.googlecode.iterm2",
        "dev.warp.Warp-Stable",
        "com.mitchellh.ghostty",
        "net.kovidgoyal.kitty",
        "org.alacritty",
        "io.alacritty",
        "com.github.wez.wezterm",
        "co.zeit.hyper",
    ];
    TERMINALS
        .iter()
        .any(|known| bundle_id.eq_ignore_ascii_case(known))
}

/// Multi-line text (any `\n` or `\r`) is refused when the frontmost app is
/// terminal-like OR unknown (`None`). Single-line text is always allowed.
pub fn paste_allowed(text: &str, frontmost: Option<&str>) -> bool {
    if !text.contains(['\n', '\r']) {
        return true;
    }
    match frontmost {
        None => false,
        Some(bundle_id) => !is_terminal_like(bundle_id),
    }
}

pub trait FrontmostProbe: Send + Sync {
    fn frontmost_bundle(&self) -> Option<String>;
}

/// Run a generated-text paste under a focus guard.
/// 1. probe once; `!paste_allowed` => `Err` and `do_paste` is NOT called;
/// 2. call `do_paste(pre_chord)`: `pre_chord` re-probes and returns true only
///    when the frontmost app is IDENTICAL to step 1 and paste is still
///    allowed; `do_paste` MUST call it immediately before the paste key chord
///    and abort (restoring the clipboard) when it returns false;
/// 3. if `do_paste` returned `Ok` without ever calling `pre_chord`, return
///    `Err` (contract violation detector).
pub fn guarded_paste(
    probe: &dyn FrontmostProbe,
    text: &str,
    do_paste: &mut dyn FnMut(&mut dyn FnMut() -> bool) -> Result<(), String>,
) -> Result<(), String> {
    const REFUSED: &str = "paste refused: frontmost app is not safe for this text";
    const GUARD_SKIPPED: &str = "paste aborted: focus guard was not consulted";

    let first = probe.frontmost_bundle();
    if !paste_allowed(text, first.as_deref()) {
        return Err(REFUSED.to_string());
    }

    let guard_consulted = std::cell::Cell::new(false);
    let mut pre_chord = || {
        guard_consulted.set(true);
        let now = probe.frontmost_bundle();
        now == first && paste_allowed(text, now.as_deref())
    };

    match do_paste(&mut pre_chord) {
        Err(err) => Err(err),
        Ok(()) if guard_consulted.get() => Ok(()),
        Ok(()) => Err(GUARD_SKIPPED.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteOptions {
    pub auto_submit: bool,
    pub force_clipboard: bool,
    pub append_trailing_newline: bool,
}

/// Options for model-generated or dictated session text: never submit,
/// always clipboard (never Direct typing), never a trailing newline.
pub fn generated_paste_options() -> PasteOptions {
    PasteOptions {
        auto_submit: false,
        force_clipboard: true,
        append_trailing_newline: false,
    }
}

// ---------------------------------------------------------------------
// Model-proposed action allowlist
// ---------------------------------------------------------------------

/// Only `OpenApp` and `OpenUrl` with an http/https scheme (case-insensitive,
/// no leading whitespace, non-empty remainder) pass. Everything else,
/// including `OpenPath` and `NoMatch`, is `None`.
pub fn restrict_model_action(decision: &RouteDecision) -> Option<CommandAction> {
    match decision {
        RouteDecision::Matched { action } => match action {
            CommandAction::OpenApp { .. } => Some(action.clone()),
            CommandAction::OpenUrl { url } if web_remainder(url).is_some() => Some(action.clone()),
            CommandAction::OpenUrl { .. } | CommandAction::OpenPath { .. } => None,
        },
        RouteDecision::NoMatch => None,
    }
}

/// The part after an `http://` or `https://` prefix (matched
/// case-insensitively, no trimming), if that remainder is non-empty.
fn web_remainder(url: &str) -> Option<&str> {
    for scheme in ["https://", "http://"] {
        if let Some(head) = url.get(..scheme.len()) {
            if head.eq_ignore_ascii_case(scheme) {
                return url.get(scheme.len()..).filter(|rest| !rest.is_empty());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;

    // ------------------------- endpointer -------------------------

    const LOUD: &[f32] = &[0.0, 0.5, 0.1];
    const QUIET: &[f32] = &[0.0, 0.01, 0.02]; // 0.02 == floor == silence

    fn ep() -> Endpointer {
        Endpointer::new(EndpointConfig {
            silence_ms: 1000,
            ..EndpointConfig::with_silence(1000)
        })
    }

    /// Feed `levels` every 50 ms over [from, to). Returns the first event.
    fn feed_range(
        e: &mut Endpointer,
        from: u64,
        to: u64,
        levels: &[f32],
    ) -> Option<(u64, EndpointEvent)> {
        let mut t = from;
        while t < to {
            if let Some(ev) = e.feed(t, levels) {
                return Some((t, ev));
            }
            t += 50;
        }
        None
    }

    /// Feed a fixed VAD speech-activity flag every `step` ms over [from, to).
    fn feed_activity_range(
        e: &mut Endpointer,
        from: u64,
        to: u64,
        step: u64,
        active: bool,
    ) -> Option<(u64, EndpointEvent)> {
        let mut t = from;
        while t < to {
            if let Some(ev) = e.feed_activity(t, active) {
                return Some((t, ev));
            }
            t += step;
        }
        None
    }

    // ---- VAD speech activity (design rev 2): the endpointing signal is the
    // VAD's speech/no-speech decision, never the display-scale level values.

    #[test]
    fn room_noise_with_no_vad_speech_ends_as_no_speech_not_max_length() {
        let mut e = ep();
        assert_eq!(
            feed_activity_range(&mut e, 0, 6000, 20, false),
            Some((4000, EndpointEvent::NoSpeech))
        );
        assert!(!e.had_speech());
    }

    #[test]
    fn vad_speech_then_vad_silence_ends_after_silence_ms_regardless_of_levels() {
        let mut e = ep();
        assert_eq!(feed_activity_range(&mut e, 0, 1000, 50, true), None);
        assert!(e.had_speech());
        // last active feed at 950; silence_ms = 1000 => fires at 1950
        assert_eq!(
            feed_activity_range(&mut e, 1000, 5000, 50, false),
            Some((1950, EndpointEvent::SpeechEnded))
        );
    }

    #[test]
    fn continuous_vad_speech_reaches_max_length_only_at_30_seconds() {
        let mut e = ep();
        assert_eq!(
            feed_activity_range(&mut e, 0, 40_000, 50, true),
            Some((30_000, EndpointEvent::MaxLength))
        );
    }

    #[test]
    fn a_vad_blip_shorter_than_min_speech_does_not_start_speech() {
        let mut e = ep();
        assert_eq!(feed_activity_range(&mut e, 0, 100, 50, true), None);
        assert_eq!(
            feed_activity_range(&mut e, 100, 6000, 50, false),
            Some((4000, EndpointEvent::NoSpeech))
        );
        assert!(!e.had_speech());
    }

    #[test]
    fn feed_with_levels_is_exactly_feed_activity_of_any_level_above_the_floor() {
        // (levels, repeat-count) segments, 50 ms apart; covers room tone,
        // quiet, speech, exactly-at-floor and empty slices.
        let segments: &[(&[f32], u32)] = &[
            (QUIET, 20),
            (LOUD, 10),
            (&[], 5),
            (QUIET, 30),
            (&[0.77, 0.7], 120),
            (LOUD, 700),
        ];
        let mut old = ep();
        let mut new = ep();
        let floor = EndpointConfig::with_silence(1000).floor;
        let mut t = 0u64;
        for (levels, n) in segments {
            for _ in 0..*n {
                let a = old.feed(t, levels);
                let b = new.feed_activity(t, levels.iter().any(|&l| l > floor));
                assert_eq!(a, b, "diverged at t={t} levels={levels:?}");
                t += 50;
            }
        }
    }

    #[test]
    fn speech_then_silence_ends_exactly_after_silence_ms() {
        let mut e = ep();
        assert_eq!(feed_range(&mut e, 0, 200, QUIET), None);
        assert_eq!(feed_range(&mut e, 200, 1000, LOUD), None);
        assert!(e.had_speech());
        // last loud feed is at 950; silence_ms=1000 => fires at 1950
        assert_eq!(
            feed_range(&mut e, 1000, 2500, QUIET),
            Some((1950, EndpointEvent::SpeechEnded))
        );
    }

    #[test]
    fn a_pause_shorter_than_silence_ms_does_not_end_the_utterance() {
        let mut e = ep();
        assert_eq!(feed_range(&mut e, 0, 500, LOUD), None);
        assert_eq!(feed_range(&mut e, 500, 1200, QUIET), None); // 700ms pause
        assert_eq!(feed_range(&mut e, 1200, 1700, LOUD), None);
        assert_eq!(
            feed_range(&mut e, 1700, 4000, QUIET),
            Some((2650, EndpointEvent::SpeechEnded))
        );
    }

    #[test]
    fn no_speech_fires_after_the_initial_grace() {
        let mut e = ep();
        assert_eq!(feed_range(&mut e, 100, 4100, QUIET), None);
        assert_eq!(e.feed(4100, QUIET), Some(EndpointEvent::NoSpeech));
        assert!(!e.had_speech());
    }

    #[test]
    fn grace_is_counted_from_the_first_feed_not_from_zero() {
        let mut e = ep();
        assert_eq!(e.feed(10_000, QUIET), None);
        assert_eq!(feed_range(&mut e, 10_050, 14_000, QUIET), None);
        assert_eq!(e.feed(14_000, QUIET), Some(EndpointEvent::NoSpeech));
    }

    #[test]
    fn a_short_blip_below_min_speech_is_not_speech() {
        let mut e = ep();
        assert_eq!(feed_range(&mut e, 0, 500, QUIET), None);
        assert_eq!(e.feed(500, LOUD), None);
        assert_eq!(e.feed(550, LOUD), None); // 50ms run only
        assert_eq!(e.feed(600, QUIET), None); // resets the run
        assert!(!e.had_speech());
        assert_eq!(
            feed_range(&mut e, 650, 4500, QUIET),
            Some((4000, EndpointEvent::NoSpeech))
        );
    }

    #[test]
    fn continuous_loud_for_min_speech_counts_as_speech() {
        let mut e = ep();
        assert_eq!(e.feed(0, LOUD), None);
        assert!(!e.had_speech());
        assert_eq!(e.feed(100, LOUD), None);
        assert!(!e.had_speech());
        assert_eq!(e.feed(150, LOUD), None);
        assert!(e.had_speech());
    }

    #[test]
    fn max_length_fires_for_endless_speech() {
        let mut e = ep();
        assert_eq!(feed_range(&mut e, 0, 30_000, LOUD), None);
        assert_eq!(e.feed(30_000, LOUD), Some(EndpointEvent::MaxLength));
    }

    #[test]
    fn events_fire_at_most_once() {
        let mut e = ep();
        assert_eq!(e.feed(0, QUIET), None);
        assert_eq!(e.feed(4000, QUIET), Some(EndpointEvent::NoSpeech));
        assert_eq!(e.feed(4050, QUIET), None);
        assert_eq!(e.feed(60_000, LOUD), None);
    }

    #[test]
    fn empty_level_slices_count_as_silence() {
        let mut e = ep();
        assert_eq!(feed_range(&mut e, 0, 1000, LOUD), None);
        assert_eq!(
            feed_range(&mut e, 1000, 3000, &[]),
            Some((1950, EndpointEvent::SpeechEnded))
        );
        let mut e2 = ep();
        assert_eq!(e2.feed(0, &[]), None);
        assert_eq!(e2.feed(4000, &[]), Some(EndpointEvent::NoSpeech));
    }

    #[test]
    fn level_exactly_at_the_floor_is_silence() {
        let mut e = ep();
        assert_eq!(feed_range(&mut e, 0, 3999, &[0.02]), None);
        assert_eq!(e.feed(4000, &[0.02]), Some(EndpointEvent::NoSpeech));
    }

    // ------------------------- wait_for_speech_end -------------------------

    struct FakeProbe {
        /// Each poll pops (is_current, is_speaking); when empty repeats `rest`.
        script: Mutex<VecDeque<(bool, bool)>>,
        rest: (bool, bool),
        polls: Mutex<u32>,
    }
    impl FakeProbe {
        fn new(script: Vec<(bool, bool)>, rest: (bool, bool)) -> Self {
            FakeProbe {
                script: Mutex::new(script.into()),
                rest,
                polls: Mutex::new(0),
            }
        }
        fn next(&self) -> (bool, bool) {
            *self.polls.lock().unwrap() += 1;
            self.script.lock().unwrap().pop_front().unwrap_or(self.rest)
        }
    }
    // The probe is polled once per loop iteration through BOTH methods, so
    // cache the pair per iteration: is_current is always called first.
    struct PairProbe(FakeProbe, Mutex<(bool, bool)>);
    impl SpeechProbe for PairProbe {
        fn is_current(&self, _epoch: u64) -> bool {
            let pair = self.0.next();
            *self.1.lock().unwrap() = pair;
            pair.0
        }
        fn is_speaking(&self) -> bool {
            self.1.lock().unwrap().1
        }
    }
    fn probe(script: Vec<(bool, bool)>, rest: (bool, bool)) -> PairProbe {
        PairProbe(FakeProbe::new(script, rest), Mutex::new((true, true)))
    }

    #[tokio::test]
    async fn wait_returns_done_when_speech_stops() {
        let p = probe(
            vec![(true, true), (true, true), (true, false)],
            (true, false),
        );
        let out =
            wait_for_speech_end(&p, 1, Duration::from_secs(2), Duration::from_millis(5)).await;
        assert_eq!(out, WaitOutcome::Done);
        assert_eq!(*p.0.polls.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn wait_returns_interrupted_when_the_epoch_is_superseded_even_if_speaking() {
        let p = probe(vec![(true, true), (false, true)], (false, true));
        let out =
            wait_for_speech_end(&p, 1, Duration::from_secs(2), Duration::from_millis(5)).await;
        assert_eq!(out, WaitOutcome::Interrupted);
    }

    #[tokio::test]
    async fn not_current_beats_not_speaking() {
        let p = probe(vec![(false, false)], (false, false));
        let out =
            wait_for_speech_end(&p, 1, Duration::from_secs(2), Duration::from_millis(5)).await;
        assert_eq!(out, WaitOutcome::Interrupted);
    }

    #[tokio::test]
    async fn wait_times_out_when_speech_never_ends() {
        let p = probe(vec![], (true, true));
        let started = std::time::Instant::now();
        let out =
            wait_for_speech_end(&p, 1, Duration::from_millis(60), Duration::from_millis(10)).await;
        assert_eq!(out, WaitOutcome::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    // ------------------------- terminal / paste_allowed -------------------------

    #[test]
    fn terminal_like_matches_known_bundle_ids_case_insensitively() {
        for id in [
            "com.apple.Terminal",
            "COM.APPLE.TERMINAL",
            "com.googlecode.iterm2",
            "dev.warp.Warp-Stable",
            "com.mitchellh.ghostty",
            "net.kovidgoyal.kitty",
            "org.alacritty",
            "io.alacritty",
            "com.github.wez.wezterm",
            "co.zeit.hyper",
        ] {
            assert!(is_terminal_like(id), "{id}");
        }
        for id in [
            "com.apple.Safari",
            "com.apple.Notes",
            "",
            "com.apple.Terminal.extra",
            "terminal",
        ] {
            assert!(!is_terminal_like(id), "{id}");
        }
    }

    #[test]
    fn paste_allowed_matrix() {
        // single line: always fine, even unknown/terminal
        assert!(paste_allowed("hello", None));
        assert!(paste_allowed("hello", Some("com.apple.Terminal")));
        assert!(paste_allowed("hello", Some("com.apple.Safari")));
        assert!(paste_allowed("", None));
        // multi-line: fine in a normal app only
        assert!(paste_allowed("a\nb", Some("com.apple.Notes")));
        assert!(!paste_allowed("a\nb", Some("com.apple.Terminal")));
        assert!(!paste_allowed("a\nb", Some("com.googlecode.iterm2")));
        assert!(
            !paste_allowed("a\nb", None),
            "unknown frontmost is unsafe for multiline"
        );
        assert!(!paste_allowed("a\rb", Some("com.apple.Terminal")));
        assert!(!paste_allowed("a\n", None), "any newline counts");
    }

    // ------------------------- guarded_paste -------------------------

    struct SeqProbe(Mutex<VecDeque<Option<String>>>, Mutex<u32>);
    impl SeqProbe {
        fn new(seq: Vec<Option<&str>>) -> Self {
            SeqProbe(
                Mutex::new(seq.into_iter().map(|o| o.map(|s| s.to_string())).collect()),
                Mutex::new(0),
            )
        }
    }
    impl FrontmostProbe for SeqProbe {
        fn frontmost_bundle(&self) -> Option<String> {
            *self.1.lock().unwrap() += 1;
            let mut q = self.0.lock().unwrap();
            if q.len() > 1 {
                q.pop_front().unwrap()
            } else {
                q.front().cloned().unwrap_or(None)
            }
        }
    }

    #[test]
    fn guarded_paste_ok_for_single_line_and_calls_pre_chord() {
        let probe = SeqProbe::new(vec![Some("com.apple.Notes"), Some("com.apple.Notes")]);
        let mut called = 0;
        let r = guarded_paste(&probe, "hello", &mut |pre| {
            called += 1;
            assert!(pre());
            Ok(())
        });
        assert_eq!(r, Ok(()));
        assert_eq!(called, 1);
    }

    #[test]
    fn guarded_paste_refuses_multiline_into_a_terminal_before_touching_the_clipboard() {
        let probe = SeqProbe::new(vec![Some("com.apple.Terminal")]);
        let mut called = false;
        let r = guarded_paste(&probe, "a\nb", &mut |_pre| {
            called = true;
            Ok(())
        });
        assert!(r.is_err());
        assert!(!called, "do_paste must not run when refused up front");
        assert_eq!(*probe.1.lock().unwrap(), 1);
    }

    #[test]
    fn guarded_paste_refuses_multiline_when_frontmost_is_unknown() {
        let probe = SeqProbe::new(vec![None]);
        let mut called = false;
        let r = guarded_paste(&probe, "a\nb", &mut |_pre| {
            called = true;
            Ok(())
        });
        assert!(r.is_err() && !called);
    }

    #[test]
    fn guarded_paste_allows_multiline_into_a_normal_app_with_stable_focus() {
        let probe = SeqProbe::new(vec![Some("com.apple.Notes"), Some("com.apple.Notes")]);
        let r = guarded_paste(&probe, "a\nb", &mut |pre| {
            if pre() {
                Ok(())
            } else {
                Err("focus".into())
            }
        });
        assert_eq!(r, Ok(()));
    }

    #[test]
    fn guarded_paste_aborts_when_focus_moves_to_a_terminal_before_the_chord() {
        let probe = SeqProbe::new(vec![Some("com.apple.Notes"), Some("com.apple.Terminal")]);
        let mut chord_sent = false;
        let r = guarded_paste(&probe, "a\nb", &mut |pre| {
            if !pre() {
                return Err("focus changed".into());
            }
            chord_sent = true;
            Ok(())
        });
        assert!(r.is_err());
        assert!(!chord_sent);
    }

    #[test]
    fn guarded_paste_aborts_on_any_focus_identity_change() {
        let probe = SeqProbe::new(vec![Some("com.apple.Notes"), Some("com.apple.Safari")]);
        let r = guarded_paste(&probe, "single line", &mut |pre| {
            if pre() {
                Ok(())
            } else {
                Err("changed".into())
            }
        });
        assert!(r.is_err());
    }

    #[test]
    fn guarded_paste_detects_a_do_paste_that_never_consulted_the_guard() {
        let probe = SeqProbe::new(vec![Some("com.apple.Notes")]);
        let r = guarded_paste(&probe, "hi", &mut |_pre| Ok(()));
        assert!(r.is_err());
    }

    #[test]
    fn guarded_paste_propagates_the_paste_error() {
        let probe = SeqProbe::new(vec![Some("com.apple.Notes")]);
        let r = guarded_paste(&probe, "hi", &mut |pre| {
            let _ = pre();
            Err("clipboard unavailable".into())
        });
        assert_eq!(r, Err("clipboard unavailable".to_string()));
    }

    #[test]
    fn generated_paste_options_never_submit_or_type() {
        let o = generated_paste_options();
        assert!(!o.auto_submit);
        assert!(o.force_clipboard);
        assert!(!o.append_trailing_newline);
    }

    // ------------------------- restrict_model_action -------------------------

    fn matched(a: CommandAction) -> RouteDecision {
        RouteDecision::Matched { action: a }
    }
    fn url(u: &str) -> RouteDecision {
        matched(CommandAction::OpenUrl { url: u.to_string() })
    }

    #[test]
    fn restrict_allows_open_app_and_web_urls() {
        let app = matched(CommandAction::OpenApp {
            name: "Safari".into(),
            resolved_path: PathBuf::from("/Applications/Safari.app"),
        });
        assert!(restrict_model_action(&app).is_some());
        for u in [
            "https://example.com",
            "http://example.org/x?y=1",
            "HTTPS://EXAMPLE.COM",
            "Http://a.b",
        ] {
            assert!(restrict_model_action(&url(u)).is_some(), "{u}");
        }
    }

    #[test]
    fn restrict_refuses_everything_else() {
        for u in [
            "ftp://example.com",
            "file:///etc/hosts",
            "javascript:alert(1)",
            "data:text/html,hi",
            "vbscript:x",
            " https://x.com",
            "https://",
            "http://",
            "https:/x",
            "example.com",
            "",
            "ssh://host",
            "x-apple.systempreferences:",
            "tel:123",
        ] {
            assert!(restrict_model_action(&url(u)).is_none(), "{u:?}");
        }
        let path = matched(CommandAction::OpenPath {
            path: PathBuf::from("/Users/me/run.command"),
        });
        assert!(restrict_model_action(&path).is_none());
        assert!(restrict_model_action(&RouteDecision::NoMatch).is_none());
    }

    #[test]
    fn restrict_returns_the_same_action_unchanged() {
        let a = CommandAction::OpenUrl {
            url: "https://example.com/a".into(),
        };
        assert_eq!(restrict_model_action(&matched(a.clone())), Some(a));
    }
}
