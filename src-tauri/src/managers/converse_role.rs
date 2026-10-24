//! Stage 1 spoken roles (DESIGN-PR2-STAGE1 Revision 3, sections 2.2, 2.4b and 2.5): pure helpers
//! for the conversation loop. The loop owns the state (`Carry` in `converse.rs`); this module
//! only splits speech into chunks and models a paused lesson with its expiry.

#[cfg(test)]
use std::cell::Cell;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// A paused lesson is dropped when nobody has touched it for this long.
pub const LESSON_TTL: Duration = Duration::from_secs(10 * 60);

/// The lesson clock. Production reads `Instant::now`; tests move it forward with `advance_clock`
/// so the ten-minute rule is exercised without sleeping.
pub fn now() -> Instant {
    Instant::now() + clock_offset()
}

#[cfg(test)]
thread_local! {
    static CLOCK_OFFSET: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

#[cfg(test)]
fn clock_offset() -> Duration {
    CLOCK_OFFSET.with(Cell::get)
}

#[cfg(not(test))]
fn clock_offset() -> Duration {
    Duration::ZERO
}

/// Moves the lesson clock forward (tests only). The offset is per thread, so tests that use
/// it must run on tokio's default current-thread test runtime.
#[cfg(test)]
pub fn advance_clock(by: Duration) {
    CLOCK_OFFSET.with(|offset| offset.set(offset.get() + by));
}

/// Sets the lesson clock back to the real clock (tests only).
#[cfg(test)]
pub fn reset_clock() {
    CLOCK_OFFSET.with(|offset| offset.set(Duration::ZERO));
}
/// At most this many sentences in one spoken chunk.
const CHUNK_MAX_SENTENCES: usize = 3;
/// At most this many characters in one spoken chunk.
const CHUNK_MAX_CHARS: usize = 300;

/// Spoken when the user ends the role with a local clear phrase.
pub const CLEARED_SPOKEN: &str = "Okay, back to normal.";
/// Spoken before the remainder of a lesson resumes automatically.
pub const CONTINUE_SPOKEN: &str = "Let's continue.";
/// Spoken when the lesson request that follows a role acknowledgement fails.
pub const LESSON_START_FAILED_SPOKEN: &str = "I couldn't start the lesson.";

/// The unspoken part of a lesson, in speaking order. A chunk leaves the queue only after its say
/// returned `Done`, so an interrupted chunk is replayed from its start. Never persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lesson {
    pub chunks: VecDeque<String>,
    /// Continue by itself after a question was answered (set by the role's `resume` flag).
    pub resume_auto: bool,
    /// When the lesson was last touched: created, a chunk committed, or a continuation or Resume
    /// started. The ten-minute lifetime runs from here.
    pub touched: Instant,
}

impl Lesson {
    pub fn new(chunks: Vec<String>, resume_auto: bool, touched: Instant) -> Lesson {
        Lesson {
            chunks: chunks.into(),
            resume_auto,
            touched,
        }
    }

    /// Marks the lesson as used at `now`, restarting its lifetime.
    pub fn touch(&mut self, now: Instant) {
        self.touched = now;
    }

    /// True when the lesson is at least `LESSON_TTL` old since its last touch, at `now`.
    pub fn expired(&self, now: Instant) -> bool {
        Self::expired_after(now.saturating_duration_since(self.touched))
    }

    /// True when a lesson of this age is expired. Pure, so the lifetime is testable without a clock.
    pub fn expired_after(age: Duration) -> bool {
        age >= LESSON_TTL
    }
}

/// Splits speech into spoken chunks of at most three sentences and 300 characters. Sentences end
/// at `.`, `!` or `?` followed by whitespace or the end of the text. A sentence longer than 300
/// characters is cut at the last space inside its first 300 characters, or hard-cut at 300.
pub fn split_chunks(text: &str) -> Vec<String> {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chunks = Vec::new();
    let mut current = String::new();
    let mut sentences = 0usize;
    for sentence in split_sentences(&flat) {
        let len = sentence.chars().count();
        if len > CHUNK_MAX_CHARS {
            push_chunk(&mut chunks, &mut current, &mut sentences);
            chunks.extend(cut_long(&sentence));
            continue;
        }
        let joined = if current.is_empty() {
            len
        } else {
            current.chars().count() + 1 + len
        };
        if sentences == CHUNK_MAX_SENTENCES || (sentences > 0 && joined > CHUNK_MAX_CHARS) {
            push_chunk(&mut chunks, &mut current, &mut sentences);
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(&sentence);
        sentences += 1;
    }
    push_chunk(&mut chunks, &mut current, &mut sentences);
    chunks
}

/// Moves the chunk being built (if any) into `chunks` and starts a new one.
fn push_chunk(chunks: &mut Vec<String>, current: &mut String, sentences: &mut usize) {
    if !current.is_empty() {
        chunks.push(std::mem::take(current));
    }
    *sentences = 0;
}

/// Sentences of already-normalised text (single spaces). Empty pieces are dropped.
fn split_sentences(flat: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    for (i, c) in flat.char_indices() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        let end = i + c.len_utf8();
        let ends_here = end == flat.len() || flat[end..].starts_with(' ');
        if ends_here {
            push_nonempty(&mut out, &flat[start..end]);
            start = end;
        }
    }
    push_nonempty(&mut out, &flat[start..]);
    out
}

fn push_nonempty(out: &mut Vec<String>, raw: &str) {
    let trimmed = raw.trim();
    if !trimmed.is_empty() {
        out.push(trimmed.to_string());
    }
}

/// Cuts one over-long sentence into pieces of at most 300 characters.
fn cut_long(sentence: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = sentence.to_string();
    while rest.chars().count() > CHUNK_MAX_CHARS {
        let window: Vec<char> = rest.chars().take(CHUNK_MAX_CHARS).collect();
        let cut = window
            .iter()
            .rposition(|c| *c == ' ')
            .filter(|at| *at > 0)
            .unwrap_or(CHUNK_MAX_CHARS);
        let head: String = window[..cut].iter().collect();
        pieces.push(head);
        rest = rest
            .chars()
            .skip(cut)
            .collect::<String>()
            .trim_start()
            .to_string();
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_sentences_make_one_chunk() {
        assert_eq!(
            split_chunks("One is first. Two follows it. Three comes next. Four is here."),
            vec![
                "One is first. Two follows it. Three comes next.".to_string(),
                "Four is here.".to_string(),
            ]
        );
    }

    #[test]
    fn a_short_answer_is_one_chunk() {
        assert_eq!(
            split_chunks("Two short sentences. Done here."),
            vec!["Two short sentences. Done here.".to_string()]
        );
    }

    #[test]
    fn empty_and_blank_text_make_no_chunks() {
        assert!(split_chunks("").is_empty());
        assert!(split_chunks("   \n  ").is_empty());
    }

    #[test]
    fn a_decimal_point_does_not_split() {
        assert_eq!(
            split_chunks("Pi is 3.14 and that is fine."),
            vec!["Pi is 3.14 and that is fine.".to_string()]
        );
    }

    #[test]
    fn newlines_collapse_to_single_spaces() {
        assert_eq!(
            split_chunks("First line.\nSecond line."),
            vec!["First line. Second line.".to_string()]
        );
    }

    #[test]
    fn chunks_respect_the_300_character_limit() {
        let sentence = "word ".repeat(40); // 200 chars, one sentence after the full stop below
        let text = format!("{sentence}end. {sentence}end.");
        let chunks = split_chunks(&text);
        assert!(chunks.iter().all(|c| c.chars().count() <= CHUNK_MAX_CHARS));
        assert_eq!(chunks.len(), 2);
    }

    #[test]
    fn long_sentence_cuts_at_the_last_space_before_300() {
        let text = format!("{} {}.", "a".repeat(250), "b".repeat(100));
        assert_eq!(
            split_chunks(&text),
            vec!["a".repeat(250), format!("{}.", "b".repeat(100))]
        );
    }

    #[test]
    fn long_word_without_space_is_hard_cut_at_300() {
        assert_eq!(
            split_chunks(&"c".repeat(350)),
            vec!["c".repeat(300), "c".repeat(50)]
        );
    }

    #[test]
    fn lesson_lifetime_is_ten_minutes() {
        assert!(!Lesson::expired_after(Duration::from_secs(599)));
        assert!(Lesson::expired_after(Duration::from_secs(600)));
        assert!(Lesson::expired_after(Duration::from_secs(3_600)));
    }

    #[test]
    fn lesson_expiry_reads_the_clock_from_its_creation() {
        let created = Instant::now();
        let lesson = Lesson::new(vec!["Hello.".to_string()], false, created);
        assert!(!lesson.expired(created + Duration::from_secs(60)));
        assert!(lesson.expired(created + LESSON_TTL));
    }
}
