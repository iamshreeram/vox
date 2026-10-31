//! Spec for the Tamil helpers in `natural_chunk` (DESIGN R2-B, R2-C). Authoritative: do not
//! weaken an assertion; if one looks wrong, report it instead of editing it.
use crate::managers::natural_chunk::*;

fn starts_with_mark(piece: &str) -> bool {
    piece.chars().next().is_some_and(|c| {
        matches!(
            c,
            '\u{0B82}' | '\u{0B83}' | '\u{0BBE}'..='\u{0BCD}' | '\u{0BD7}'
        )
    })
}

fn unspaced_tamil(len: usize) -> String {
    // Clusters that must never be split: consonant+pulli+consonant, a vowel sign after a
    // consonant, and a consonant with a vowel sign that is followed by a dependent sign.
    let units = ["க்ஷ", "ாி", "கொடு"];
    let mut text = String::new();
    let mut i = 0;
    while text.chars().count() < len {
        text.push_str(units[i % units.len()]);
        i += 1;
    }
    text.chars().take(len).collect()
}

#[test]
fn tamil_char_covers_the_whole_block_only() {
    for c in [
        'அ', 'ி', '\u{0BCD}', '\u{0B80}', '\u{0BFF}', '\u{0BE6}', '\u{0BF3}',
    ] {
        assert!(is_tamil_char(c), "{c:?} is Tamil");
    }
    for c in [
        'a', 'Z', '\u{0B7F}', '\u{0C00}', '\u{0D85}', 'ह', '\u{00E9}', ' ', '3',
    ] {
        assert!(!is_tamil_char(c), "{c:?} is not Tamil");
    }
}

#[test]
fn tamil_letters_exclude_marks_digits_and_symbols() {
    for c in ['அ', 'க', 'ஹ', '\u{0B85}', '\u{0BB9}', '\u{0BD0}'] {
        assert!(is_tamil_letter(c), "{c:?} is a letter");
    }
    for c in [
        '\u{0B82}', '\u{0B83}', '\u{0BBE}', 'ி', '\u{0BCD}', '\u{0BD7}', '\u{0BE6}', '\u{0BF3}',
        'a', ' ',
    ] {
        assert!(!is_tamil_letter(c), "{c:?} is not a letter");
    }
}

#[test]
fn mostly_tamil_is_true_for_tamil_sentences() {
    assert!(is_mostly_tamil(
        "இன்று நான் கடைக்குச் சென்று ஒரு புத்தகம் வாங்கினேன் really good"
    ));
    assert!(is_mostly_tamil(
        "இன்று நான் கடைக்குச் சென்று ஒரு புத்தகம் வாங்கினேன் OK"
    ));
}

#[test]
fn mostly_tamil_is_false_for_other_scripts_and_english() {
    assert!(!is_mostly_tamil("Hello world, this is English."));
    assert!(!is_mostly_tamil("नमस्ते दुनिया यह हिंदी है"));
    assert!(!is_mostly_tamil("ආයුබෝවන් ලෝකය මෙය සිංහල"));
    assert!(!is_mostly_tamil("café naïve résumé"));
}

#[test]
fn mostly_tamil_needs_sixty_percent_of_alphabetic_chars() {
    assert!(!is_mostly_tamil(
        "வணக்கம் one two three four five six seven eight nine ten"
    ));
    assert!(!is_mostly_tamil(
        "வணக்கம் hello world this is a long english sentence"
    ));
}

#[test]
fn mostly_tamil_is_false_without_alphabetic_chars() {
    assert!(!is_mostly_tamil(""));
    assert!(!is_mostly_tamil("   "));
    assert!(!is_mostly_tamil("123 !!! ..."));
    assert!(!is_mostly_tamil("௧௨௩ ௳"));
}

#[test]
fn mostly_tamil_counts_marks_as_tamil() {
    // Only marks and letters: the pulli and vowel signs are Tamil.
    assert!(is_mostly_tamil("கொடு"));
    assert!(is_mostly_tamil("வணக்கம்"));
}

#[test]
fn empty_and_whitespace_only_yield_no_pieces() {
    assert!(split_tamil_for_synthesis("", 600).is_empty());
    assert!(split_tamil_for_synthesis(" \n\t  ", 600).is_empty());
}

#[test]
fn sentence_ends_are_detected_before_whitespace() {
    assert_eq!(
        split_tamil_for_synthesis("வணக்கம். நன்றி!", 600),
        vec!["வணக்கம்.", "நன்றி!"]
    );
    assert_eq!(
        split_tamil_for_synthesis("சரி? ஆம்… போகலாம்", 600),
        vec!["சரி?", "ஆம்…", "போகலாம்"]
    );
}

#[test]
fn decimals_and_unspaced_dots_are_not_sentence_ends() {
    assert_eq!(
        split_tamil_for_synthesis("விலை 3.5 ரூபாய்", 600),
        vec!["விலை 3.5 ரூபாய்"]
    );
    assert_eq!(split_tamil_for_synthesis("ம்.ஆம்", 600), vec!["ம்.ஆம்"]);
}

#[test]
fn there_is_no_forty_eight_char_first_chunk_cap() {
    let sentence = vec!["சொல்"; 20].join(" ");
    assert!(sentence.chars().count() > 48);
    assert_eq!(split_tamil_for_synthesis(&sentence, 600), vec![sentence]);
}

#[test]
fn spaced_pieces_join_back_to_the_collapsed_input() {
    let pieces = split_tamil_for_synthesis("வணக்கம்   உலகம்\n\tஇது  ஒரு சோதனை", 12);
    assert!(pieces.iter().all(|p| p.chars().count() <= 12), "{pieces:?}");
    assert_eq!(pieces.join(" "), "வணக்கம் உலகம் இது ஒரு சோதனை");
}

#[test]
fn fifty_thousand_chars_split_quickly_and_losslessly() {
    let words = ["வணக்கம்", "உலகம்", "இது", "ஒரு", "சோதனை."];
    let input = (0..10_000)
        .map(|i| words[i % words.len()])
        .collect::<Vec<_>>()
        .join(" ");
    assert!(input.chars().count() > 50_000);
    let pieces = split_tamil_for_synthesis(&input, 600);
    assert!(pieces.iter().all(|p| p.chars().count() <= 600));
    assert_eq!(pieces.join(" "), input);
}

#[test]
fn unspaced_seven_hundred_chars_split_on_grapheme_boundaries() {
    let input = unspaced_tamil(700);
    let pieces = split_tamil_for_synthesis(&input, 50);
    assert_eq!(pieces.concat(), input);
    assert!(pieces.iter().all(|p| !starts_with_mark(p)), "{pieces:?}");
    assert!(pieces.iter().all(|p| p.chars().count() <= 50));
}

#[test]
fn no_piece_starts_with_a_mark_for_mixed_input() {
    let input = "கொடு ாி கொடுகொடு க்ஷாி ா";
    for max in [2, 3, 5, 9] {
        let pieces = split_tamil_for_synthesis(input, max);
        assert!(!pieces.is_empty());
        assert!(
            pieces.iter().all(|p| !starts_with_mark(p)),
            "max {max}: {pieces:?}"
        );
    }
}

#[test]
fn small_max_chars_always_make_progress() {
    let input = "வணக்கம் உலகம் க்ஷாி கொடுகொடு";
    for max in [0, 1, 2, 3] {
        let pieces = split_tamil_for_synthesis(input, max);
        assert!(!pieces.is_empty(), "max {max}");
        assert!(pieces.iter().all(|p| !p.is_empty()), "max {max}");
    }
    // 0 and 1 are raised to the minimum of 2.
    assert_eq!(
        split_tamil_for_synthesis(input, 0),
        split_tamil_for_synthesis(input, 2)
    );
    assert_eq!(
        split_tamil_for_synthesis(input, 1),
        split_tamil_for_synthesis(input, 2)
    );
}

#[test]
fn grapheme_cuts_never_separate_a_vowel_sign_or_a_pulli() {
    assert_eq!(
        split_tamil_for_synthesis("கொடுகொடு", 2),
        vec!["கொ", "டு", "கொ", "டு"]
    );
    assert_eq!(
        split_tamil_for_synthesis("கொடுகொடு", 3),
        vec!["கொ", "டு", "கொ", "டு"]
    );
    // A consonant+pulli+consonant cluster is kept whole even above max_chars.
    assert_eq!(split_tamil_for_synthesis("க்ஷ", 2), vec!["க்ஷ"]);
}

#[test]
fn leading_orphan_marks_are_dropped() {
    assert_eq!(split_tamil_for_synthesis("ா வணக்கம்", 600), vec!["வணக்கம்"]);
}
