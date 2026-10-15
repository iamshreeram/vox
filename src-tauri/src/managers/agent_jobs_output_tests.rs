//! RED contract tests for `managers::agent_jobs_output` (M3 S2a, DESIGN.md Revision 5:
//! R2-7, R3-5 and the D4 output paragraph).
//!
//! Every test drives the public API and asserts on the ring contents, so until
//! the implementation lands each test panics on a `todo!()` in the stubs (or on
//! the missing behavior). No fixture is a real credential: secret-shaped values
//! are built with `concat!` so that no token-shaped literal appears in one piece.

use super::*;
use std::sync::{Arc, Mutex};

const AWS_FAKE: &str = concat!("AKIA", "IOSFODNN7EXAMPLE");
const GITHUB_FAKE: &str = concat!("ghp_", "a1b2c3d4e5f6g7h8i9j0K1L2M3N4O5P6Q7R8");
const PEM_BEGIN: &str = concat!("-----BEGIN ", "PRIVATE KEY-----");
const PEM_END: &str = concat!("-----END ", "PRIVATE KEY-----");
const BLOCK_MARKER: &str = "[private key block omitted]";
const LONG_MARKER: &str = "[long line omitted]";
const READ_ERROR_MARKER: &str = "[output read error]";

/// Feeds every chunk into a fresh pipeline, calls `finish`, and returns the ring
/// contents oldest first.
fn collect(chunks: &[&[u8]]) -> Vec<String> {
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    for chunk in chunks {
        pipe.feed(chunk);
    }
    pipe.finish();
    let guard = ring.lock().expect("ring lock");
    guard.last(RING_LINES)
}

/// Same as `collect` but with an injected total byte cap.
fn collect_capped(data: &[u8], cap: u64) -> Vec<String> {
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::with_total_cap(Arc::clone(&ring), cap);
    pipe.feed(data);
    pipe.finish();
    let guard = ring.lock().expect("ring lock");
    guard.last(RING_LINES)
}

/// Output of one-shot feeding; every other split must reproduce it exactly.
fn whole(data: &[u8]) -> Vec<String> {
    collect(&[data])
}

/// Asserts that splitting `data` at every byte offset gives the one-shot output.
fn assert_every_split_matches(data: &[u8]) {
    let expected = whole(data);
    for k in 0..=data.len() {
        let got = collect(&[&data[..k], &data[k..]]);
        assert_eq!(got, expected, "two-chunk split at byte {k} differs");
    }
}

/// Asserts that feeding one byte at a time gives the one-shot output.
fn assert_byte_by_byte_matches(data: &[u8]) {
    let expected = whole(data);
    let chunks: Vec<&[u8]> = data.chunks(1).collect();
    assert_eq!(collect(&chunks), expected, "one-byte chunks differ");
}

fn assert_no_fragment(lines: &[String], fragment: &str) {
    for line in lines {
        assert!(
            !line.contains(fragment),
            "fragment {fragment:?} leaked into ring line {line:?}; ring = {lines:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// TailRing: caps, eviction and ordering
// ---------------------------------------------------------------------------

#[test]
fn ring_keeps_at_most_fifty_lines() {
    let mut ring = TailRing::new();
    for i in 0..(RING_LINES + 30) {
        ring.push(format!("line{i}"));
    }
    assert_eq!(ring.len(), RING_LINES, "ring must cap at RING_LINES lines");
}

#[test]
fn ring_evicts_oldest_lines_first_when_line_cap_hit() {
    let mut ring = TailRing::new();
    for i in 0..(RING_LINES + 2) {
        ring.push(format!("L{i:03}"));
    }
    let lines = ring.last(RING_LINES);
    assert_eq!(lines.first().map(String::as_str), Some("L002"));
    assert_eq!(lines.last().map(String::as_str), Some("L051"));
}

#[test]
fn ring_keeps_at_most_thirty_two_kib_of_bytes() {
    let mut ring = TailRing::new();
    let line = "b".repeat(2000);
    for _ in 0..40 {
        ring.push(line.clone());
    }
    assert!(
        ring.bytes() <= RING_BYTES,
        "ring holds {} bytes, cap is {RING_BYTES}",
        ring.bytes()
    );
    assert!(ring.len() >= 1, "newest line must survive the byte cap");
}

#[test]
fn ring_bytes_counts_pushed_content() {
    let mut ring = TailRing::new();
    ring.push("abc".to_string());
    ring.push("de".to_string());
    assert_eq!(
        ring.bytes(),
        5,
        "bytes() must equal the sum of stored line lengths"
    );
    assert_eq!(ring.len(), 2);
}

#[test]
fn ring_last_returns_newest_n_oldest_first() {
    let mut ring = TailRing::new();
    for s in ["a", "b", "c", "d", "e"] {
        ring.push(s.to_string());
    }
    assert_eq!(ring.last(3), vec!["c", "d", "e"]);
}

#[test]
fn ring_last_zero_returns_empty() {
    let mut ring = TailRing::new();
    ring.push("only".to_string());
    assert!(ring.last(0).is_empty());
}

#[test]
fn ring_last_larger_than_len_returns_everything() {
    let mut ring = TailRing::new();
    ring.push("x".to_string());
    ring.push("y".to_string());
    assert_eq!(ring.last(100), vec!["x", "y"]);
}

#[test]
fn ring_empty_reports_zero_len_and_bytes() {
    let ring = TailRing::new();
    assert_eq!(ring.len(), 0);
    assert_eq!(ring.bytes(), 0);
    assert!(ring.last(5).is_empty());
}

// ---------------------------------------------------------------------------
// Framing: LF, CRLF, empty lines, EOF, chunk independence
// ---------------------------------------------------------------------------

#[test]
fn lf_splits_into_lines() {
    assert_eq!(collect(&[b"one\ntwo\n"]), vec!["one", "two"]);
}

#[test]
fn crlf_terminator_is_stripped_entirely() {
    assert_eq!(collect(&[b"one\r\ntwo\r\n"]), vec!["one", "two"]);
}

#[test]
fn lone_cr_mid_line_is_a_control_char_not_a_terminator() {
    assert_eq!(collect(&[b"a\rb\n"]), vec!["a?b"]);
}

#[test]
fn empty_lines_are_pushed_as_empty_strings() {
    assert_eq!(collect(&[b"\n\nx\n"]), vec!["", "", "x"]);
}

#[test]
fn eof_partial_line_is_flushed_by_finish() {
    assert_eq!(collect(&[b"done\npartial"]), vec!["done", "partial"]);
}

#[test]
fn terminated_last_line_adds_nothing_extra_at_eof() {
    assert_eq!(collect(&[b"x\n"]), vec!["x"]);
}

#[test]
fn empty_input_yields_no_lines() {
    assert!(collect(&[]).is_empty());
    assert!(collect(&[b""]).is_empty());
}

#[test]
fn one_byte_chunks_equal_one_big_chunk() {
    let data = b"alpha\r\nbeta\n\ngamma-no-newline";
    assert_byte_by_byte_matches(data);
}

#[test]
fn every_two_chunk_split_equals_one_shot() {
    assert_every_split_matches(b"alpha\r\nbeta\n\ngamma\tdelta\nlast");
}

#[test]
fn seven_byte_chunks_equal_one_big_chunk() {
    let data = "line one\nline two has more text\n".repeat(20);
    let expected = whole(data.as_bytes());
    let chunks: Vec<&[u8]> = data.as_bytes().chunks(7).collect();
    assert_eq!(collect(&chunks), expected);
}

#[test]
fn tab_is_preserved_in_output() {
    assert_eq!(collect(&[b"a\tb\n"]), vec!["a\tb"]);
}

#[test]
fn control_characters_are_replaced_with_question_mark() {
    assert_eq!(
        collect(&[b"\x1b[31mred\x07bell\x00nul\n"]),
        vec!["?[31mred?bell?nul"]
    );
}

#[test]
fn invalid_utf8_is_converted_lossily() {
    assert_eq!(collect(&[b"ok \xff end\n"]), vec!["ok \u{FFFD} end"]);
}

#[test]
fn invalid_utf8_split_across_chunks_matches_one_shot() {
    let data: &[u8] = b"caf\xc3\xa9 \xe2\x82 bad\n";
    assert_every_split_matches(data);
}

// ---------------------------------------------------------------------------
// Line cap: 2000 bytes kept, 2001 discarded entirely
// ---------------------------------------------------------------------------

#[test]
fn line_of_exactly_line_cap_bytes_is_kept_not_marked() {
    let line = "k".repeat(LINE_CAP);
    let data = format!("{line}\n");
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines.len(), 1, "a 2000-byte line must be kept: {lines:?}");
    assert_ne!(lines[0], LONG_MARKER);
    assert_eq!(
        lines[0],
        "k".repeat(DISPLAY_CAP),
        "display cap is 200 bytes"
    );
}

#[test]
fn line_of_line_cap_plus_one_bytes_is_discarded_with_marker() {
    let line = "k".repeat(LINE_CAP + 1);
    let data = format!("{line}\n");
    assert_eq!(collect(&[data.as_bytes()]), vec![LONG_MARKER]);
}

#[test]
fn overlong_line_leaks_no_prefix() {
    let data = format!("SECRETPREFIX{}\nafter\n", "z".repeat(LINE_CAP));
    let lines = collect(&[data.as_bytes()]);
    assert_no_fragment(&lines, "SECRETPREFIX");
    assert_eq!(lines, vec![LONG_MARKER, "after"]);
}

#[test]
fn overlong_line_marker_is_pushed_once_not_per_chunk() {
    let mut data = vec![b'q'; 5000];
    data.extend_from_slice(b"\nok\n");
    assert_eq!(
        collect(&[&data[..3000], &data[3000..]]),
        vec![LONG_MARKER, "ok"]
    );
}

#[test]
fn overlong_line_followed_by_normal_line_is_kept() {
    let data = format!("{}\nnext line\n", "w".repeat(LINE_CAP + 500));
    assert_eq!(collect(&[data.as_bytes()]), vec![LONG_MARKER, "next line"]);
}

#[test]
fn secret_straddling_the_cap_boundary_is_never_shown() {
    // 1990 filler bytes then a 20-byte AWS-shaped key: the line ends at 2010 bytes.
    let data = format!("{}{AWS_FAKE} tail\nafter\n", "f".repeat(1990));
    let lines = collect(&[data.as_bytes()]);
    assert_no_fragment(&lines, "AKIA");
    assert_no_fragment(&lines, "IOSFODNN7");
    assert_eq!(lines, vec![LONG_MARKER, "after"]);
}

#[test]
fn secret_straddling_exact_cap_edge_is_never_shown() {
    let data = format!("{}{AWS_FAKE}\nafter\n", "f".repeat(1985));
    let lines = collect(&[data.as_bytes()]);
    assert_no_fragment(&lines, "AKIA");
    assert_no_fragment(&lines, "IOSFODNN7");
    assert_eq!(lines, vec![LONG_MARKER, "after"]);
}

#[test]
fn cap_boundary_split_at_every_offset_matches_one_shot() {
    let data = format!("{}{AWS_FAKE}\nafter\n", "f".repeat(1990));
    let bytes = data.as_bytes();
    let expected = whole(bytes);
    for k in 1980..=bytes.len() {
        let got = collect(&[&bytes[..k], &bytes[k..]]);
        assert_eq!(got, expected, "split at {k} across the cap differs");
    }
}

#[test]
fn ten_mb_newline_free_input_keeps_buffer_bounded() {
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    let chunk = vec![b'n'; CHUNK_BYTES];
    for _ in 0..(10 * 1024 * 1024 / CHUNK_BYTES) {
        pipe.feed(&chunk);
        assert!(
            pipe.buffered_bytes() <= LINE_CAP,
            "framing buffer grew to {} bytes",
            pipe.buffered_bytes()
        );
    }
    pipe.finish();
    let guard = ring.lock().expect("ring lock");
    assert!(
        guard.len() <= 2,
        "ring must stay tiny, got {} lines",
        guard.len()
    );
    assert!(guard.bytes() < 1024, "ring holds {} bytes", guard.bytes());
}

// ---------------------------------------------------------------------------
// Sanitising, redaction and display truncation
// ---------------------------------------------------------------------------

#[test]
fn plain_text_under_display_cap_passes_unchanged() {
    assert_eq!(collect(&[b"hello world\n"]), vec!["hello world"]);
}

#[test]
fn display_truncation_limits_lines_to_display_cap_bytes() {
    let data = format!("{}\n", "p".repeat(500));
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines[0].len(), DISPLAY_CAP);
}

#[test]
fn display_truncation_never_splits_a_multibyte_char() {
    // 199 ASCII bytes then one 2-byte char: the 200-byte cut would split it.
    // Filler is 'z' (not a hex digit) so the redactor's long-hex rule does not fire.
    let data = format!("{}\u{e9}tail\n", "z".repeat(199));
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines[0], "z".repeat(199));
    assert!(lines[0].len() <= DISPLAY_CAP);
}

#[test]
fn display_truncation_happens_after_redaction() {
    // A long password value: if truncation ran first, a fragment of the value
    // would survive. Redaction must replace the whole value first.
    let data = format!("password={}\n", "a".repeat(250));
    let lines = collect(&[data.as_bytes()]);
    assert!(
        lines[0].contains("<redacted:"),
        "line not redacted: {:?}",
        lines[0]
    );
    assert_no_fragment(&lines, &"a".repeat(8));
}

#[test]
fn password_assignment_is_redacted() {
    let data = b"login password=hunter2 ok\n";
    let lines = collect(&[data]);
    assert_no_fragment(&lines, "hunter2");
    assert!(lines[0].contains("<redacted:"), "{lines:?}");
}

#[test]
fn aws_key_is_redacted() {
    let data = format!("key {AWS_FAKE} end\n");
    let lines = collect(&[data.as_bytes()]);
    assert_no_fragment(&lines, "IOSFODNN7EXAMPLE");
}

#[test]
fn github_token_is_redacted() {
    let data = format!("token {GITHUB_FAKE}\n");
    let lines = collect(&[data.as_bytes()]);
    assert_no_fragment(&lines, "a1b2c3d4e5f6g7h8");
}

#[test]
fn redaction_is_applied_on_every_split_of_a_secret_line() {
    let data = format!("x {AWS_FAKE} y\n");
    assert_every_split_matches(data.as_bytes());
    let lines = collect(&[data.as_bytes()]);
    assert_no_fragment(&lines, "IOSFODNN7EXAMPLE");
}

#[test]
fn read_error_pushes_the_marker_line() {
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    pipe.feed(b"before\n");
    pipe.read_error();
    pipe.finish();
    let guard = ring.lock().expect("ring lock");
    assert_eq!(guard.last(RING_LINES), vec!["before", READ_ERROR_MARKER]);
}

#[test]
fn buffered_bytes_grows_with_pending_partial_line() {
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    pipe.feed(b"abcdef");
    assert_eq!(pipe.buffered_bytes(), 6);
    pipe.feed(b"\n");
    assert_eq!(
        pipe.buffered_bytes(),
        0,
        "a completed line leaves nothing buffered"
    );
}

// ---------------------------------------------------------------------------
// TOTAL_CAP: bytes after the cap are counted and dropped
// ---------------------------------------------------------------------------

#[test]
fn bytes_after_total_cap_are_ignored() {
    // 50 bytes of "aaaa\n" with a 30-byte cap: only the first six lines fit.
    let data = "aaaa\n".repeat(10);
    let lines = collect_capped(data.as_bytes(), 30);
    assert_eq!(lines.len(), 6, "{lines:?}");
    assert!(lines.iter().all(|l| l == "aaaa"));
}

#[test]
fn total_cap_drops_large_volume_without_growth_or_panic() {
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::with_total_cap(Arc::clone(&ring), 1000);
    let chunk = b"volume line of text\n".repeat(400);
    for _ in 0..20 {
        pipe.feed(&chunk);
        assert!(pipe.buffered_bytes() <= LINE_CAP);
    }
    pipe.finish();
    let guard = ring.lock().expect("ring lock");
    assert!(guard.len() <= RING_LINES);
}

#[test]
fn total_cap_zero_ignores_every_byte() {
    assert!(collect_capped(b"never shown\n", 0).is_empty());
}

// ---------------------------------------------------------------------------
// R3-5 private-key block suppression
// ---------------------------------------------------------------------------

#[test]
fn private_key_block_emits_one_marker_and_hides_body() {
    let data = format!("{PEM_BEGIN}\nSECRETBODYLINE\n{PEM_END}\nvisible\n");
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines, vec![BLOCK_MARKER, "visible"]);
    assert_no_fragment(&lines, "SECRETBODY");
}

#[test]
fn private_key_block_begin_line_is_dropped_not_pushed() {
    let data = format!("{PEM_BEGIN}\nbody\n{PEM_END}\n");
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines, vec![BLOCK_MARKER]);
    assert_no_fragment(&lines, "BEGIN");
}

#[test]
fn private_key_block_over_one_thousand_lines_then_end_text_is_visible() {
    let mut data = format!("{PEM_BEGIN}\n");
    for i in 0..1500 {
        data.push_str(&format!("BODYROW{i:05}\n"));
    }
    data.push_str(&format!("{PEM_END}\nafter-block-visible\n"));
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(
        lines.last().map(String::as_str),
        Some("after-block-visible")
    );
    assert_eq!(lines.iter().filter(|l| *l == BLOCK_MARKER).count(), 1);
    assert_no_fragment(&lines, "BODYROW");
}

#[test]
fn private_key_block_without_end_hides_everything_to_eof() {
    let mut data = format!("{PEM_BEGIN}\n");
    for i in 0..300 {
        data.push_str(&format!("HIDDENROW{i}\n"));
    }
    data.push_str("tail-after-begin\n");
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    pipe.feed(data.as_bytes());
    assert!(pipe.suppressing(), "still inside an unterminated block");
    pipe.finish();
    assert!(!pipe.suppressing(), "EOF must end suppression");
    let lines = ring.lock().expect("ring lock").last(RING_LINES);
    assert_eq!(lines, vec![BLOCK_MARKER]);
    assert_no_fragment(&lines, "HIDDENROW");
    assert_no_fragment(&lines, "tail-after-begin");
}

#[test]
fn suppressing_keeps_framing_buffer_empty_for_large_body() {
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    pipe.feed(format!("{PEM_BEGIN}\n").as_bytes());
    let body = format!("{}\n", "x".repeat(1000));
    for _ in 0..1000 {
        pipe.feed(body.as_bytes());
    }
    assert!(pipe.suppressing());
    assert_eq!(
        pipe.buffered_bytes(),
        0,
        "nothing is stored while suppressing"
    );
    assert_eq!(
        ring.lock().expect("ring lock").len(),
        1,
        "only the block marker"
    );
}

#[test]
fn overlong_begin_line_with_marker_at_start_still_suppresses_body() {
    let begin = format!("{PEM_BEGIN}{}", "x".repeat(2970));
    let data = format!("{begin}\nBODYAFTERLONG\n{PEM_END}\nvisible\n");
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines, vec![BLOCK_MARKER, "visible"]);
    assert_no_fragment(&lines, "BODYAFTERLONG");
    assert_no_fragment(&lines, "xxxxxxxx");
}

#[test]
fn overlong_begin_line_with_marker_beyond_byte_2000_still_suppresses() {
    let begin = format!("{}{PEM_BEGIN}", "x".repeat(1990));
    let data = format!("{begin}\nBODYPAST2000\n{PEM_END}\nvisible\n");
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines.last().map(String::as_str), Some("visible"));
    assert_eq!(lines.iter().filter(|l| *l == BLOCK_MARKER).count(), 1);
    assert_no_fragment(&lines, "BODYPAST2000");
    assert_no_fragment(&lines, "xxxxxxxx");
}

#[test]
fn private_key_markers_split_at_every_offset_match_one_shot() {
    let data = format!("before\n{PEM_BEGIN}\nSECRETSPLIT\n{PEM_END}\nafter\n");
    assert_every_split_matches(data.as_bytes());
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines, vec!["before", BLOCK_MARKER, "after"]);
}

#[test]
fn private_key_markers_one_byte_at_a_time_match_one_shot() {
    let data = format!("{PEM_BEGIN}\nSECRETBYTES\n{PEM_END}\nvisible\n");
    assert_byte_by_byte_matches(data.as_bytes());
}

#[test]
fn private_key_markers_split_across_the_eight_kib_chunk_boundary() {
    let mut data = "y".repeat(CHUNK_BYTES - 11);
    data.push('\n');
    data.push_str(&format!("{PEM_BEGIN}\nSECRETCHUNK\n{PEM_END}\nvisible\n"));
    let bytes = data.as_bytes();
    let lines = collect(&[&bytes[..CHUNK_BYTES], &bytes[CHUNK_BYTES..]]);
    assert_no_fragment(&lines, "SECRETCHUNK");
    assert_eq!(lines.last().map(String::as_str), Some("visible"));
    assert_eq!(lines.iter().filter(|l| *l == BLOCK_MARKER).count(), 1);
}

#[test]
fn two_sequential_blocks_with_text_between_emit_two_markers() {
    let data = format!(
        "{PEM_BEGIN}\nFIRSTBODY\n{PEM_END}\nbetween\n{PEM_BEGIN}\nSECONDBODY\n{PEM_END}\nafter\n"
    );
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines, vec![BLOCK_MARKER, "between", BLOCK_MARKER, "after"]);
    assert_no_fragment(&lines, "FIRSTBODY");
    assert_no_fragment(&lines, "SECONDBODY");
}

#[test]
fn end_line_rest_after_private_key_marker_is_also_dropped() {
    let data = format!("{PEM_BEGIN}\nbody\n{PEM_END} trailing-text\nvisible\n");
    let lines = collect(&[data.as_bytes()]);
    assert_no_fragment(&lines, "trailing-text");
    assert_eq!(lines, vec![BLOCK_MARKER, "visible"]);
}

#[test]
fn end_certificate_marker_does_not_end_a_private_key_block() {
    let filler = "f".repeat(120);
    let data = format!(
        "{PEM_BEGIN}\nKEEPHIDDEN\n-----END CERTIFICATE-----\nSTILLHIDDEN\n{filler}\n{PEM_END}\nvisible\n"
    );
    let lines = collect(&[data.as_bytes()]);
    assert_no_fragment(&lines, "KEEPHIDDEN");
    assert_no_fragment(&lines, "STILLHIDDEN");
    assert_eq!(lines, vec![BLOCK_MARKER, "visible"]);
}

#[test]
fn certificate_block_is_not_suppressed() {
    let data = b"-----BEGIN CERTIFICATE-----\nvisible-cert-body\n";
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    pipe.feed(data);
    assert!(!pipe.suppressing());
    pipe.finish();
    let lines = ring.lock().expect("ring lock").last(RING_LINES);
    assert!(lines.iter().any(|l| l == "visible-cert-body"), "{lines:?}");
}

#[test]
fn begin_and_private_key_too_far_apart_does_not_suppress() {
    // More than 40 bytes between BEGIN and the PRIVATE KEY marker: no block.
    let data = format!(
        "-----BEGIN {}PRIVATE KEY-----\nvisible-far\n",
        "z".repeat(60)
    );
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    pipe.feed(data.as_bytes());
    assert!(
        !pipe.suppressing(),
        "gap of 60 bytes must not start suppression"
    );
    pipe.finish();
    let lines = ring.lock().expect("ring lock").last(RING_LINES);
    assert!(lines.iter().any(|l| l == "visible-far"), "{lines:?}");
}

#[test]
fn begin_and_private_key_close_together_do_suppress() {
    let data = format!("-----BEGIN {}\nbody\n", "PRIVATE KEY-----");
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    pipe.feed(data.as_bytes());
    assert!(
        pipe.suppressing(),
        "BEGIN followed by PRIVATE KEY-----: must suppress"
    );
}

#[test]
fn finish_ends_suppression_and_hides_unterminated_tail() {
    let data = format!("{PEM_BEGIN}\nbody\nunterminated-tail-secret");
    let ring = Arc::new(Mutex::new(TailRing::new()));
    let mut pipe = OutputPipeline::new(Arc::clone(&ring));
    pipe.feed(data.as_bytes());
    pipe.finish();
    assert!(!pipe.suppressing());
    let lines = ring.lock().expect("ring lock").last(RING_LINES);
    assert_no_fragment(&lines, "unterminated-tail-secret");
    assert_eq!(lines, vec![BLOCK_MARKER]);
}

#[test]
fn block_suppression_is_identical_across_all_chunkings() {
    let data = format!("a\n{PEM_BEGIN}\nQQQ\n{PEM_END}\nb\n{PEM_BEGIN}\nRRR\n{PEM_END}\nc\n");
    assert_byte_by_byte_matches(data.as_bytes());
    assert_every_split_matches(data.as_bytes());
    let lines = collect(&[data.as_bytes()]);
    assert_eq!(lines, vec!["a", BLOCK_MARKER, "b", BLOCK_MARKER, "c"]);
}
