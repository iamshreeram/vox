//! Authoritative tests for `converse_stream`. Do not edit to make code pass.

use super::*;
use crate::managers::converse_plan::{PlanKind, TurnPlan};

const SAMPLE: &str = r#"{"kind":"answer","emotion":"calm","speech":"The weather today is sunny. Expect a high of 24 degrees. Have a great day!"}"#;

fn run(chunks: &[&str]) -> (Vec<EarlySentence>, PlanStreamer) {
    let mut s = PlanStreamer::new();
    let mut out = vec![];
    for c in chunks {
        out.extend(s.push(c));
    }
    (out, s)
}

fn texts(v: &[EarlySentence]) -> Vec<&str> {
    v.iter().map(|s| s.text.as_str()).collect()
}

fn split_every(src: &str, n: usize) -> Vec<String> {
    let chars: Vec<char> = src.chars().collect();
    chars.chunks(n).map(|c| c.iter().collect()).collect()
}

fn run_split(src: &str, n: usize) -> Vec<EarlySentence> {
    let owned = split_every(src, n);
    let refs: Vec<&str> = owned.iter().map(String::as_str).collect();
    run(&refs).0
}

// ---- canon ----------------------------------------------------------------------------

#[test]
fn canon_collapses_whitespace_strips_control_and_brackets() {
    assert_eq!(canon("  Hello \n\t world  "), "Hello world");
    let c = canon("\u{1b}[31mHi\u{1b}[0m");
    assert!(!c.contains('\u{1b}') && !c.contains('['), "{c:?}");
    assert!(c.contains("Hi"));
    assert!(!canon("a [[slnc 500]] b").contains('['));
    assert_eq!(canon("   \n "), "");
}

// ---- happy path -----------------------------------------------------------------------

#[test]
fn releases_completed_sentences_but_leaves_the_trailing_one_for_the_final_plan() {
    let (out, s) = run(&[SAMPLE]);
    assert_eq!(
        texts(&out),
        vec![
            "The weather today is sunny.",
            "Expect a high of 24 degrees."
        ]
    );
    assert!(!s.is_disabled());
    assert_eq!(s.emotion(), Some(Emotion::Calm));
    assert!(out.iter().all(|e| e.emotion == Emotion::Calm));
}

#[test]
fn any_chunking_gives_the_same_result() {
    let expect = texts(&run(&[SAMPLE]).0)
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    for n in [1usize, 2, 3, 5, 7, 11, 64] {
        let got = run_split(SAMPLE, n);
        assert_eq!(
            texts(&got),
            expect.iter().map(String::as_str).collect::<Vec<_>>(),
            "chunk {n}"
        );
    }
}

#[test]
fn a_sentence_is_released_only_when_the_following_whitespace_has_arrived() {
    let mut s = PlanStreamer::new();
    assert!(s
        .push(r#"{"kind":"answer","speech":"The weather today is sunny."#)
        .is_empty());
    let out = s.push(" Next");
    assert_eq!(texts(&out), vec!["The weather today is sunny."]);
}

#[test]
fn clarify_and_uppercase_kind_work_and_leading_whitespace_is_allowed() {
    let src = "\n  {\"kind\":\"CLARIFY\",\"speech\":\"Which contact do you mean exactly? The one from work. ok\"}";
    let out = run(&[src]).0;
    assert_eq!(texts(&out), vec!["Which contact do you mean exactly?"]);
}

#[test]
fn emotion_after_speech_means_neutral_for_early_sentences() {
    let src = r#"{"kind":"answer","speech":"The weather today is sunny. Expect more later today. x","emotion":"cheerful"}"#;
    let (out, s) = run(&[src]);
    assert_eq!(texts(&out).len(), 2);
    assert!(out.iter().all(|e| e.emotion == Emotion::Neutral));
    assert_eq!(
        s.emotion(),
        Some(Emotion::Cheerful),
        "still reported once seen"
    );
}

#[test]
fn escapes_are_decoded_even_when_split_across_chunks() {
    let src = "{\"kind\":\"answer\",\"speech\":\"She said \\\"hello there\\\" to me.\\nCaf\\u00e9 time is at noon! \\ud83d\\ude00 tail\"}";
    for n in [1usize, 2, 3, 4, 6] {
        let got = run_split(src, n);
        assert_eq!(
            texts(&got),
            vec!["She said \"hello there\" to me.", "Café time is at noon!"],
            "chunk {n}"
        );
    }
}

#[test]
fn short_sentences_merge_into_the_next_one() {
    let src = r#"{"kind":"answer","speech":"Yes. It is. That is correct, my friend. Tail"}"#;
    assert_eq!(
        texts(&run(&[src]).0),
        vec!["Yes. It is. That is correct, my friend."]
    );
}

#[test]
fn a_decimal_point_is_not_a_boundary() {
    let src = r#"{"kind":"answer","speech":"The value is 3.5 meters tall today. Another long sentence here. Tail"}"#;
    assert_eq!(
        texts(&run(&[src]).0),
        vec![
            "The value is 3.5 meters tall today.",
            "Another long sentence here."
        ]
    );
}

#[test]
fn unknown_keys_are_skipped_and_never_spoken() {
    let src = r#"{"kind":"answer","note":"Do not speak this. Really not at all.","n":12.5,"flag":true,"z":null,"speech":"Spoken sentence number one. Spoken sentence two. x"}"#;
    assert_eq!(
        texts(&run(&[src]).0),
        vec!["Spoken sentence number one.", "Spoken sentence two."]
    );
}

#[test]
fn text_after_the_closing_brace_is_ignored() {
    let src = r#"{"kind":"answer","speech":"First long sentence here. Second long sentence. x"} trailing. Words. here."#;
    assert_eq!(texts(&run(&[src]).0).len(), 2);
}

#[test]
fn the_early_total_is_capped() {
    let sentence = "This is a reasonably long sentence of text. ";
    let speech = sentence.repeat(30);
    let src = format!("{{\"kind\":\"answer\",\"speech\":\"{speech}tail\"}}");
    let out = run(&[&src]).0;
    let total: usize =
        out.iter().map(|e| e.text.chars().count()).sum::<usize>() + out.len().saturating_sub(1);
    assert!(!out.is_empty());
    assert!(total <= MAX_EARLY_CHARS, "total {total}");
}

// ---- rejection ------------------------------------------------------------------------

#[test]
fn non_speech_kinds_never_release_anything() {
    for kind in ["command", "paste", "end", "bogus"] {
        let src = format!(
            "{{\"kind\":\"{kind}\",\"speech\":\"This is some long speech text. And another sentence. x\"}}"
        );
        let (out, s) = run(&[&src]);
        assert!(out.is_empty(), "{kind}");
        assert!(s.is_disabled(), "{kind}");
    }
}

#[test]
fn kind_must_be_the_first_member() {
    let src =
        r#"{"speech":"This is some long speech text. And another sentence. x","kind":"answer"}"#;
    let (out, s) = run(&[src]);
    assert!(out.is_empty());
    assert!(s.is_disabled());
}

#[test]
fn prose_or_code_fences_before_the_object_disable_it() {
    for src in [
        "Sure! {\"kind\":\"answer\",\"speech\":\"A long enough sentence here. Another one here. x\"}",
        "```json\n{\"kind\":\"answer\",\"speech\":\"A long enough sentence here. Another one here. x\"}\n```",
        "[1,2,3]",
        "",
    ] {
        let (out, s) = run(&[src]);
        assert!(out.is_empty(), "{src}");
        if !src.is_empty() {
            assert!(s.is_disabled(), "{src}");
        }
    }
}

#[test]
fn duplicate_speech_or_kind_disables_but_keeps_what_was_already_released() {
    let src = r#"{"kind":"answer","speech":"First long sentence here. Second long sentence. Third","speech":"Other. Words here that are long enough. y"}"#;
    let (out, s) = run(&[src]);
    // both sentences were complete (followed by whitespace) before the duplicate key arrived,
    // so they were legitimately released; the final `reconcile` prefix check is what guards
    // against a contradicting final plan.
    assert_eq!(
        texts(&out),
        vec!["First long sentence here.", "Second long sentence."]
    );
    assert!(s.is_disabled());
    let dup_kind = r#"{"kind":"answer","kind":"command","speech":"Long sentence number one. Long sentence number two. x"}"#;
    let (out, s) = run(&[dup_kind]);
    assert!(out.is_empty());
    assert!(s.is_disabled());
}

#[test]
fn an_invalid_escape_or_a_nested_value_disables_the_streamer() {
    let bad_escape =
        r#"{"kind":"answer","speech":"Long sentence number one \q. Long sentence two here. x"}"#;
    let (out, s) = run(&[bad_escape]);
    assert!(out.is_empty());
    assert!(s.is_disabled());
    let nested = r#"{"kind":"answer","extra":{"a":1},"speech":"Long sentence number one. Long sentence number two. x"}"#;
    let (out, s) = run(&[nested]);
    assert!(out.is_empty());
    assert!(s.is_disabled());
}

#[test]
fn a_disabled_streamer_is_inert_and_never_panics() {
    let mut s = PlanStreamer::new();
    s.push("not json at all");
    assert!(s.is_disabled());
    for _ in 0..50 {
        assert!(s
            .push("{\"kind\":\"answer\",\"speech\":\"Long sentence one. Long sentence two. x\"}")
            .is_empty());
    }
}

#[test]
fn a_truncated_stream_releases_no_partial_sentence() {
    let src = r#"{"kind":"answer","speech":"First long sentence here. Second long sent"#;
    let out = run(&[src]).0;
    assert_eq!(texts(&out), vec!["First long sentence here."]);
}

// ---- reconcile ------------------------------------------------------------------------

fn plan(kind: PlanKind, speech: &str) -> TurnPlan {
    TurnPlan {
        plan: None,
        job: None,
        role: None,
        kind,
        speech: speech.to_string(),
        emotion: Emotion::Neutral,
        paste_text: None,
        command: None,
        agent: None,
    }
}

fn spoken(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

#[test]
fn nothing_spoken_early_answers_speak_everything() {
    assert_eq!(
        reconcile(&plan(PlanKind::Answer, " Hello  there. "), &[]),
        Reconcile::Remainder {
            text: "Hello there.".into()
        }
    );
    for k in [PlanKind::Clarify, PlanKind::End] {
        assert!(matches!(
            reconcile(&plan(k, "Bye."), &[]),
            Reconcile::Remainder { .. }
        ));
    }
}

#[test]
fn nothing_spoken_early_actions_use_the_legacy_path() {
    assert_eq!(
        reconcile(&plan(PlanKind::Paste, "Done."), &[]),
        Reconcile::Legacy
    );
    assert_eq!(
        reconcile(&plan(PlanKind::Command, "Opening."), &[]),
        Reconcile::Legacy
    );
}

#[test]
fn the_remainder_is_the_final_speech_minus_the_spoken_prefix() {
    let early = spoken(&[
        "The weather today is sunny.",
        "Expect a high of 24 degrees.",
    ]);
    let p = plan(
        PlanKind::Answer,
        "The weather today is sunny. Expect a high of 24 degrees. Have a great day!",
    );
    assert_eq!(
        reconcile(&p, &early),
        Reconcile::Remainder {
            text: "Have a great day!".into()
        }
    );
    let p2 = plan(
        PlanKind::Clarify,
        "The weather today is sunny. Expect a high of 24 degrees.",
    );
    assert_eq!(
        reconcile(&p2, &early),
        Reconcile::Remainder {
            text: String::new()
        }
    );
}

#[test]
fn whitespace_differences_do_not_matter_but_word_boundaries_do() {
    let early = spoken(&["Hello there my friend."]);
    let ok = plan(PlanKind::Answer, "Hello   there\nmy friend.   And more.");
    assert_eq!(
        reconcile(&ok, &early),
        Reconcile::Remainder {
            text: "And more.".into()
        }
    );
    let glued = plan(PlanKind::Answer, "Hello there my friend.And more.");
    assert_eq!(reconcile(&glued, &early), Reconcile::Abort);
}

#[test]
fn contradicting_or_non_speech_finals_abort_after_early_speech() {
    let early = spoken(&["The weather today is sunny."]);
    assert_eq!(
        reconcile(
            &plan(PlanKind::Answer, "Actually it is raining heavily."),
            &early
        ),
        Reconcile::Abort
    );
    for k in [PlanKind::Command, PlanKind::Paste, PlanKind::End] {
        assert_eq!(
            reconcile(&plan(k, "The weather today is sunny."), &early),
            Reconcile::Abort,
            "{k:?}"
        );
    }
    assert_eq!(
        reconcile(&plan(PlanKind::Answer, ""), &early),
        Reconcile::Abort
    );
}

// ---- never panics on arbitrary input (deterministic fuzz) -------------------------------

#[test]
fn arbitrary_input_split_anywhere_never_panics_and_respects_the_cap() {
    // xorshift64*: deterministic, no dependencies
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let atoms: &[&str] = &[
        "{",
        "}",
        "\"",
        "\\",
        "\\u",
        "\\ud83d",
        "\\ude00",
        "\\u00e9",
        "\\uZZZZ",
        "\\n",
        "\\q",
        ":",
        ",",
        "kind",
        "answer",
        "clarify",
        "speech",
        "emotion",
        "calm",
        " ",
        "\n",
        ".",
        "!",
        "?",
        "…",
        "3.5",
        "Hello there my friend",
        "é",
        "",
        "\u{0}",
        "\u{7}",
        "[[slnc 5]]",
        "[",
        "]",
        "null",
        "true",
        "12.5",
        "{\"a\":1}",
    ];
    for round in 0..1500 {
        let mut doc = String::new();
        if next() % 2 == 0 {
            doc.push_str(r#"{"kind":"answer","speech":""#);
        }
        for _ in 0..(next() % 60) {
            doc.push_str(atoms[(next() % atoms.len() as u64) as usize]);
        }
        let chars: Vec<char> = doc.chars().collect();
        let mut s = PlanStreamer::new();
        let mut total = 0usize;
        let mut i = 0usize;
        while i < chars.len() {
            let n = 1 + (next() % 9) as usize;
            let end = (i + n).min(chars.len());
            let piece: String = chars[i..end].iter().collect();
            for e in s.push(&piece) {
                assert!(!e.text.is_empty(), "round {round}");
                total += e.text.chars().count() + 1;
            }
            i = end;
        }
        assert!(total <= MAX_EARLY_CHARS + 1, "round {round}: total {total}");
    }
}
