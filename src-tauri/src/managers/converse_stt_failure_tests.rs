//! #119 tests: a failed transcription is logged with the transcriber's own error text
//! (outcome only: never audio, never a transcript), and the failure is still reported to the
//! capture layer as the unit `TranscribeFailure`. Included from converse.rs.

use crate::managers::converse_capture::TranscribeFailure;
use crate::managers::converse_platform::{report_transcribe_result, transcribe_failure_line};
use std::cell::RefCell;

const PREFIX: &str = "transcription failed: ";

#[test]
fn failure_line_carries_the_original_error_text() {
    assert_eq!(
        transcribe_failure_line("Model is not loaded for transcription."),
        "transcription failed: Model is not loaded for transcription."
    );
}

#[test]
fn failure_line_is_one_line_without_control_characters() {
    let line = transcribe_failure_line("a\n\tb\u{7}c\r\nd");
    assert_eq!(line, "transcription failed: a b c d");
    assert!(!line.chars().any(char::is_control), "{line:?}");
}

#[test]
fn failure_line_with_no_text_says_so_without_a_dangling_colon() {
    for empty in ["", "   ", "\n\t \r\n"] {
        assert_eq!(
            transcribe_failure_line(empty),
            "transcription failed (no error text)",
            "{empty:?}"
        );
    }
}

#[test]
fn failure_line_keeps_exactly_200_chars_and_marks_longer_text() {
    let exact = "x".repeat(200);
    let line = transcribe_failure_line(&exact);
    assert_eq!(line, format!("{PREFIX}{exact}"), "200 chars: no ellipsis");

    let long = "y".repeat(500);
    let line = transcribe_failure_line(&long);
    assert_eq!(line, format!("{PREFIX}{}…", "y".repeat(200)));
}

#[test]
fn failure_line_truncates_on_a_char_boundary() {
    let long = "é".repeat(300); // 2 bytes per char: a byte cut at 200 would split a char
    let line = transcribe_failure_line(&long);
    assert_eq!(line, format!("{PREFIX}{}…", "é".repeat(200)));
}

#[test]
fn a_failing_transcribe_writes_its_error_text_to_the_log_sink() {
    let sink: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let result: Result<String, TranscribeFailure> = report_transcribe_result(
        Err::<String, _>("Model is not loaded for transcription."),
        |line| sink.borrow_mut().push(line.to_string()),
    );
    assert_eq!(result, Err(TranscribeFailure));
    assert_eq!(
        *sink.borrow(),
        vec!["transcription failed: Model is not loaded for transcription.".to_string()]
    );
}

#[test]
fn a_successful_transcribe_logs_nothing_and_passes_the_text_through() {
    let sink: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let result = report_transcribe_result(Ok::<_, &str>("hello there".to_string()), |line| {
        sink.borrow_mut().push(line.to_string())
    });
    assert_eq!(result, Ok("hello there".to_string()));
    assert!(sink.borrow().is_empty(), "the transcript is never logged");
}

#[test]
fn the_failure_log_line_is_sanitized_end_to_end() {
    let sink: RefCell<Vec<String>> = RefCell::new(Vec::new());
    let _ = report_transcribe_result(Err::<String, _>("bad\nthing\u{7}"), |line| {
        sink.borrow_mut().push(line.to_string())
    });
    assert_eq!(
        *sink.borrow(),
        vec!["transcription failed: bad thing".to_string()]
    );
}
