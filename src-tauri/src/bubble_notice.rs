//! Notice codes of the bubble window (DESIGN.md A-C3). The orb refuses some clicks and says why
//! through a bubble. Only a fixed code crosses the wire: the page turns it into localised text,
//! so no free text and no user content is ever sent or logged.

use serde::{Deserialize, Serialize};
use specta::Type;

/// Why the orb refused a click. Serialised snake_case. A code this build does not know reads as
/// `Unknown`, so an older page or a newer producer never breaks the stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum NoticeCode {
    /// Dictation is running, so the orb cannot start a conversation.
    DictationBusy,
    /// Spoken replies are turned off, so a conversation cannot speak its replies.
    TtsOff,
    /// No reply backend is configured for conversations and typed chat.
    ComposerOff,
    /// Ambient listening is on.
    Ambient,
    /// Voice conversation is not available (the feature is off or not set up).
    ExperimentalOff,
    /// The text chat is ending.
    TextEnding,
    /// Any code this build does not know.
    #[serde(other)]
    Unknown,
}

impl NoticeCode {
    /// The snake_case code, as sent to the page.
    pub fn as_str(self) -> &'static str {
        match self {
            NoticeCode::DictationBusy => "dictation_busy",
            NoticeCode::TtsOff => "tts_off",
            NoticeCode::ComposerOff => "composer_off",
            NoticeCode::Ambient => "ambient",
            NoticeCode::ExperimentalOff => "experimental_off",
            NoticeCode::TextEnding => "text_ending",
            NoticeCode::Unknown => "unknown",
        }
    }
}

/// The `kind` and `notice_code` of one bubble on the wire: `("transcript", None)` for a line,
/// `("notice", Some(code))` for a notice.
pub fn wire_parts(notice: Option<NoticeCode>) -> (&'static str, Option<&'static str>) {
    match notice {
        None => ("transcript", None),
        Some(code) => ("notice", Some(code.as_str())),
    }
}
