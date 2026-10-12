//! Authoritative tests for `converse_ack` (do not weaken). Design B, critic findings C4.

use super::*;
use std::time::Duration;

fn kind(u: &str, draft: bool) -> AckKind {
    classify(u, draft)
}

#[test]
fn questions_are_questions_even_with_a_draft_and_revise_words() {
    // critic C4: these must NOT become Revise / Action
    for u in [
        "what is the difference between a mutex and a semaphore",
        "Why is the sky blue",
        "is there a shorter route?",
        "what does formal mean?",
        "Explain how DNS works",
        "how do I rewrite this in rust",
        "which one is simpler",
        "Compare these two options",
        "open the pod bay doors?",
        "can you open safari?",
        "do not open safari",
    ] {
        assert_eq!(kind(u, true), AckKind::Question, "{u} (draft)");
        assert_eq!(kind(u, false), AckKind::Question, "{u} (no draft)");
    }
}

#[test]
fn actions_skip_leading_fillers() {
    for u in [
        "open safari",
        "please open safari",
        "hey vox, open safari and search for rust",
        "okay so open the settings",
        "can you open safari",
        "could you please search for pizza",
        "go ahead and play some music",
        "Lets start a timer",
        "I want you to find my notes",
        "now close this window",
        "Launch the terminal",
    ] {
        assert_eq!(kind(u, false), AckKind::Action, "{u}");
        assert_eq!(kind(u, true), AckKind::Action, "{u} (draft present)");
    }
}

#[test]
fn drafting_requests_are_draft() {
    for u in [
        "write a short note to my team",
        "could you please write a reply",
        "go ahead and write a note",
        "lets draft an email",
        "compose a message to bob",
    ] {
        assert_eq!(kind(u, false), AckKind::Draft, "{u}");
    }
}

#[test]
fn revise_needs_a_draft_and_an_imperative_form() {
    for u in [
        "make it shorter and more casual",
        "make that more formal",
        "rewrite that more formally",
        "rephrase it",
        "shorten the second sentence",
        "fix the typo",
        "change it to friday",
        "please polish this",
        "can you simplify it",
    ] {
        assert_eq!(kind(u, true), AckKind::Revise, "{u} with a draft");
    }
    // without a draft there is nothing to revise: never promise it
    assert_eq!(kind("make it shorter and more casual", false), AckKind::Other);
    assert_eq!(kind("rewrite that more formally", false), AckKind::Other);
    assert_eq!(kind("fix the typo", false), AckKind::Other);
    // "make" without a pronoun target, "formal"/"shorter" only mentioned
    assert_eq!(kind("make a sandwich", true), AckKind::Other);
    assert_eq!(kind("that sounds too formal", true), AckKind::Other);
    assert_eq!(kind("a shorter version would be nice", true), AckKind::Other);
}

#[test]
fn consequential_or_ambiguous_verbs_never_get_a_confident_ack() {
    for u in [
        "delete all my files",
        "send it to bob",
        "call mom",
        "remind me at five",
        "add milk to the list",
        "set a timer for ten minutes",
        "turn off the lights",
        "take a note",
        "text anna that I am late",
        "email the team",
        "reply to that",
        "tweet this",
    ] {
        assert_eq!(kind(u, false), AckKind::Other, "{u}");
        assert_eq!(kind(u, true), AckKind::Other, "{u} (draft)");
    }
}

#[test]
fn negations_and_non_requests_are_other() {
    assert_eq!(kind("don't open safari", false), AckKind::Other); // "dont"
    assert_eq!(kind("hello there", false), AckKind::Other);
    assert_eq!(kind("thanks", false), AckKind::Other);
}

#[test]
fn empty_filler_only_and_odd_input_never_panic() {
    assert_eq!(kind("", false), AckKind::Other);
    assert_eq!(kind("   \n\t ", true), AckKind::Other);
    assert_eq!(kind("please", false), AckKind::Other);
    assert_eq!(kind("hey vox okay so", true), AckKind::Other);
    assert_eq!(kind("ÉCOUTE ÇA", false), AckKind::Other);
    assert_eq!(kind("\u{0}\u{7}!!!", false), AckKind::Other);
    let long = format!("open {}", "a".repeat(5000));
    assert_eq!(kind(&long, false), AckKind::Action);
    let long_q = format!("{}?", "x".repeat(5000));
    assert_eq!(kind(&long_q, false), AckKind::Question);
    let _ = choose(&long, true, Some("On it."));
}

#[test]
fn delays_per_kind() {
    assert_eq!(after_for(AckKind::Revise), Duration::from_millis(600));
    assert_eq!(after_for(AckKind::Draft), Duration::from_millis(600));
    assert_eq!(after_for(AckKind::Action), Duration::from_millis(600));
    assert_eq!(after_for(AckKind::Question), Duration::from_millis(1500));
    assert_eq!(after_for(AckKind::Other), Duration::from_millis(1000));
    assert_eq!(choose("open safari", false, None).after, Duration::from_millis(600));
    assert_eq!(choose("why is the sky blue", false, None).after, Duration::from_millis(1500));
    assert_eq!(choose("hello there", false, None).after, Duration::from_millis(1000));
}

#[test]
fn text_comes_from_the_pool_of_the_classified_kind() {
    let cases = [
        ("make it shorter", true, AckKind::Revise),
        ("write a note", false, AckKind::Draft),
        ("open safari", false, AckKind::Action),
        ("why is the sky blue", false, AckKind::Question),
        ("hello there", false, AckKind::Other),
    ];
    for (u, d, k) in cases {
        let a = choose(u, d, None);
        assert_eq!(a.kind, k, "{u}");
        assert!(pool(k).contains(&a.text), "{u}: {}", a.text);
    }
}

#[test]
fn choice_is_deterministic_and_follows_the_length_rule() {
    // "open a" = 6 chars -> idx 0; "open ab" = 7 chars -> idx 1 (pool of 2)
    assert_eq!(choose("open a", false, None).text, "On it.");
    assert_eq!(choose("open ab", false, None).text, "Okay, working on that.");
    assert_eq!(choose("open ab", false, None), choose("open ab", false, None));
    // equal to `last` => next entry (wraps)
    assert_eq!(choose("open a", false, Some("On it.")).text, "Okay, working on that.");
    assert_eq!(choose("open ab", false, Some("Okay, working on that.")).text, "On it.");
    // question pool of 3: "why x" = 5 chars -> idx 2
    assert_eq!(choose("why x", false, None).text, "Let me check.");
    assert_eq!(choose("why x", false, Some("Let me check.")).text, "Let me think.");
}

#[test]
fn never_the_same_phrase_twice_in_a_row() {
    let samples = [
        ("make it shorter", true),
        ("write a note", false),
        ("open safari", false),
        ("why is the sky blue", false),
        ("hello there", false),
    ];
    for (u, d) in samples {
        let k = classify(u, d);
        for last in pool(k) {
            let a = choose(u, d, Some(last));
            assert_ne!(a.text, *last, "{u}");
            assert!(pool(k).contains(&a.text));
        }
        // an unknown `last` changes nothing
        assert_eq!(choose(u, d, Some("not a phrase")), choose(u, d, None));
    }
}

#[test]
fn every_pool_entry_is_reachable() {
    let families: [(&str, bool, AckKind); 5] = [
        ("open ", false, AckKind::Action),
        ("write ", false, AckKind::Draft),
        ("make it ", true, AckKind::Revise),
        ("what ", false, AckKind::Question),
        ("hello ", false, AckKind::Other),
    ];
    for (prefix, draft, k) in families {
        let mut seen = std::collections::BTreeSet::new();
        for n in 0..12 {
            let u = format!("{prefix}{}", "x".repeat(n));
            assert_eq!(classify(&u, draft), k, "{u}");
            seen.insert(choose(&u, draft, None).text);
        }
        assert_eq!(seen.len(), pool(k).len(), "{k:?}: {seen:?}");
    }
}
