//! Automatic spoken-language routing (experimental, opt-in).
//!
//! Pure decision logic with no engines inside. `TranscriptionManager::transcribe` feeds it the
//! active model's text and token count; it decides whether a language check is worth its cost,
//! asks a language identifier (an installed multilingual Whisper) through `LidIdentify`, and
//! returns which text to use. See `.ram-status/auto-lang-route-20261101b/DESIGN.md` (Revision 3).
//!
/// Sample rate of every audio buffer handled here.
pub const SAMPLE_RATE_HZ: f32 = 16_000.0;
/// Active-model text is flagged when tokens per word is strictly above this.
pub const TPW_THRESHOLD: f32 = 3.5;
/// A flag needs at least this many whitespace-separated words.
pub const MIN_WORDS: usize = 2;
/// Audio shorter than this never takes the auto route.
pub const MIN_AUDIO_SECS: f32 = 0.5;
/// Whisper text may replace the active text only for audio up to this long.
pub const REPLACE_MAX_SECS: f32 = 29.0;
/// The language identifier only sees this much audio (Whisper window).
pub const LID_MAX_SECS: f32 = 30.0;
/// Replacement text needs at least this many letters.
pub const MIN_LID_LETTERS: usize = 3;
/// Share of letters that must be in the expected script for a replacement.
pub const SCRIPT_MIN_FRACTION: f32 = 0.5;

/// What the language identifier returned: ISO code (if any) and its own transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LidResult {
    /// Detected ISO 639-1 code, or `None` when the model reported nothing.
    pub lang: Option<String>,
    /// The identifier model's transcript of the same audio.
    pub text: String,
}

/// Outcome of one identifier call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LidOutcome {
    /// The identifier ran.
    Done(LidResult),
    /// Another call holds the identifier; nothing changed.
    Busy,
    /// Not available now (no model, load failure, backoff).
    Unavailable(String),
}

/// The identifier as `auto_refine` sees it. Implemented by the real slot and by test fakes.
pub trait LidIdentify {
    /// Identifies the language of 16 kHz mono samples (at most `LID_MAX_SECS` are passed).
    fn identify(&self, samples: &[f32]) -> LidOutcome;
}

/// The Tamil pack as `auto_refine` sees it.
pub trait TamilPackRoute {
    /// Transcribes the WHOLE audio. `None` means busy, unavailable or failed.
    fn transcribe(&self, samples: &[f32]) -> Option<String>;
}

/// Where the final text came from. Drives finalisation (DESIGN C4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The active model's text, with all its original evidence and prompt state.
    KeepActive,
    /// Tamil pack text; finalised once through the Tamil path only.
    TamilPack,
    /// The identifier model's transcript for this ISO language code.
    LidText {
        /// ISO code the identifier reported.
        lang: String,
    },
}

/// Result of `auto_refine`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoRefined {
    /// Text to finalise and return.
    pub text: String,
    /// Which engine produced it.
    pub source: Source,
}

/// Everything `auto_refine` needs about one finished utterance.
#[derive(Debug, Clone, Copy)]
pub struct AutoInput<'a> {
    /// The active model's transcript.
    pub active_text: &'a str,
    /// Token rows of the active run whose trimmed text is non-empty.
    pub token_count: usize,
    /// 16 kHz mono samples of the whole utterance.
    pub audio: &'a [f32],
    /// Tamil pack installed, enabled and ready.
    pub tamil_pack_ready: bool,
}

/// What `choose_route` decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoRoute {
    /// Keep the active text.
    KeepActive,
    /// Run the Tamil pack on the whole audio.
    UseTamilPack,
    /// Use the identifier's transcript.
    UseLidText,
}

/// A downloaded model that could serve as the language identifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LidCandidate {
    /// Model id.
    pub id: String,
    /// File size in bytes.
    pub bytes: u64,
    /// True for a multilingual Whisper (not an English-only `.en` model).
    pub multilingual_whisper: bool,
    /// True when it is the model currently selected for transcription.
    pub is_active: bool,
}

/// Tokens per whitespace-separated word, or `None` when the text has no words.
pub fn tokens_per_word(text: &str, tokens: usize) -> Option<f32> {
    let words = text.split_whitespace().count();
    if words == 0 {
        return None;
    }
    Some(tokens as f32 / words as f32)
}

/// True when the active text looks like non-English speech rendered as gibberish:
/// at least `MIN_WORDS` words, tokens per word strictly above `TPW_THRESHOLD`, and audio of at
/// least `MIN_AUDIO_SECS`. Empty or whitespace-only text is never flagged.
pub fn needs_language_check(text: &str, tokens: usize, audio_secs: f32) -> bool {
    let words = text.split_whitespace().count();
    if words < MIN_WORDS || audio_secs < MIN_AUDIO_SECS {
        return false;
    }
    match tokens_per_word(text, tokens) {
        Some(ratio) => ratio > TPW_THRESHOLD,
        None => false,
    }
}

/// Share of letters (alphabetic chars) in the Tamil block U+0B80..=U+0BFF. Digits, punctuation
/// and whitespace are ignored. 0.0 when there are no letters.
pub fn tamil_letter_fraction(text: &str) -> f32 {
    let letters = letter_count(text);
    if letters == 0 {
        return 0.0;
    }
    let tamil = text.chars().filter(|&c| is_tamil_letter(c)).count();
    tamil as f32 / letters as f32
}

/// Share of letters that are not Latin-script letters (accented Latin counts as Latin).
/// Digits, punctuation and whitespace are ignored. 0.0 when there are no letters.
pub fn non_latin_letter_fraction(text: &str) -> f32 {
    let letters = letter_count(text);
    if letters == 0 {
        return 0.0;
    }
    let non_latin = text
        .chars()
        .filter(|&c| is_letter(c) && !is_latin_letter(c))
        .count();
    non_latin as f32 / letters as f32
}

/// Number of alphabetic characters in `text`.
pub fn letter_count(text: &str) -> usize {
    text.chars().filter(|&c| is_letter(c)).count()
}

/// Letter rule: alphabetic, or in the Tamil block excluding Tamil digits.
fn is_letter(c: char) -> bool {
    c.is_alphabetic() || is_tamil_letter(c)
}

/// Tamil block U+0B80..=U+0BFF, excluding Tamil digits U+0BE6..=U+0BEF.
fn is_tamil_letter(c: char) -> bool {
    ('\u{0B80}'..='\u{0BFF}').contains(&c) && !('\u{0BE6}'..='\u{0BEF}').contains(&c)
}

/// Latin script: ASCII letters, Latin-1 and Latin Extended-A/B (minus the multiplication and
/// division signs), and Latin Extended Additional.
fn is_latin_letter(c: char) -> bool {
    c.is_ascii_alphabetic()
        || (('\u{00C0}'..='\u{024F}').contains(&c) && c != '\u{00D7}' && c != '\u{00F7}')
        || ('\u{1E00}'..='\u{1EFF}').contains(&c)
}

/// Picks the route from an identifier result (DESIGN Behaviour E).
pub fn choose_route(lid: &LidResult, tamil_ready: bool, audio_secs: f32) -> AutoRoute {
    let lang = lid
        .lang
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if lang.is_empty() || lang == "en" {
        return AutoRoute::KeepActive;
    }
    if letter_count(&lid.text) < MIN_LID_LETTERS {
        return AutoRoute::KeepActive;
    }
    if lang == "ta" {
        if tamil_letter_fraction(&lid.text) < SCRIPT_MIN_FRACTION {
            return AutoRoute::KeepActive;
        }
        if tamil_ready {
            return AutoRoute::UseTamilPack;
        }
        return if audio_secs <= REPLACE_MAX_SECS {
            AutoRoute::UseLidText
        } else {
            AutoRoute::KeepActive
        };
    }
    let non_latin = non_latin_letter_fraction(&lid.text);
    if audio_secs <= REPLACE_MAX_SECS && non_latin >= SCRIPT_MIN_FRACTION {
        AutoRoute::UseLidText
    } else {
        AutoRoute::KeepActive
    }
}

/// The whole refinement for one utterance: gate, identify, choose, and fall through when the
/// Tamil pack fails (DESIGN Behaviour B to F). Never calls `lid` unless
/// `needs_language_check` is true; never calls `tamil` unless the route is `UseTamilPack`.
/// At most `LID_MAX_SECS` of audio go to `lid`; `tamil` gets the whole audio.
pub fn auto_refine(
    input: &AutoInput<'_>,
    lid: &dyn LidIdentify,
    tamil: &dyn TamilPackRoute,
) -> AutoRefined {
    let keep_active = || AutoRefined {
        text: input.active_text.to_string(),
        source: Source::KeepActive,
    };
    let audio_secs = input.audio.len() as f32 / SAMPLE_RATE_HZ;
    if !needs_language_check(input.active_text, input.token_count, audio_secs) {
        return keep_active();
    }

    let lid_cap = (LID_MAX_SECS * SAMPLE_RATE_HZ) as usize;
    let lid_take = input.audio.len().min(lid_cap);
    let lid_audio = input.audio.get(..lid_take).unwrap_or(input.audio);
    let result = match lid.identify(lid_audio) {
        LidOutcome::Done(result) => result,
        LidOutcome::Busy | LidOutcome::Unavailable(_) => return keep_active(),
    };

    let mut route = choose_route(&result, input.tamil_pack_ready, audio_secs);
    if route == AutoRoute::UseTamilPack {
        if let Some(text) = tamil.transcribe(input.audio) {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return AutoRefined {
                    text: trimmed.to_string(),
                    source: Source::TamilPack,
                };
            }
        }
        // Pack failed or returned nothing: fall through as if the pack were not installed.
        route = choose_route(&result, false, audio_secs);
    }

    if route != AutoRoute::UseLidText {
        return keep_active();
    }
    AutoRefined {
        text: result.text.trim().to_string(),
        source: Source::LidText {
            lang: result
                .lang
                .as_deref()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase(),
        },
    }
}

/// Smallest multilingual Whisper by size (ties by id). `.en` models are ignored. `None` when
/// there is none. A candidate with `is_active` is skipped (the active model already detects).
pub fn pick_lid_model(cands: &[LidCandidate]) -> Option<String> {
    cands
        .iter()
        .filter(|c| c.multilingual_whisper && !c.is_active && !c.id.ends_with(".en"))
        .min_by(|a, b| a.bytes.cmp(&b.bytes).then_with(|| a.id.cmp(&b.id)))
        .map(|c| c.id.clone())
}

/// True when the auto route applies to this utterance: language is "auto", the setting is on,
/// translate-to-English is off, audio is at least `MIN_AUDIO_SECS`, and the active engine
/// produced token rows.
pub fn auto_route_active(
    selected_language: &str,
    enabled: bool,
    translate_to_english: bool,
    audio_secs: f32,
    engine_has_token_rows: bool,
) -> bool {
    selected_language.eq_ignore_ascii_case("auto")
        && enabled
        && !translate_to_english
        && audio_secs >= MIN_AUDIO_SECS
        && engine_has_token_rows
}

/// Branch C: an active multilingual model reported `ta`; upgrade to the Tamil pack when the
/// language is "auto", the setting is on, translate is off, and the pack is ready.
pub fn should_upgrade_to_tamil_pack(
    model_detected_lang: Option<&str>,
    tamil_pack_ready: bool,
    enabled: bool,
    selected_language: &str,
    translate_to_english: bool,
) -> bool {
    let detected_tamil =
        model_detected_lang.is_some_and(|lang| lang.trim().eq_ignore_ascii_case("ta"));
    detected_tamil
        && tamil_pack_ready
        && enabled
        && selected_language.eq_ignore_ascii_case("auto")
        && !translate_to_english
}
