//! Spec for the Tamil text policy in `natural_text` (DESIGN R2-B). Authoritative: do not
//! weaken an assertion; if one looks wrong, report it instead of editing it.
use crate::managers::natural_text::{has_speakable_tamil, prepare_tamil_for_speech as prep};

#[test]
fn latin_letters_and_emoji_are_removed() {
    assert_eq!(prep("Hello வணக்கம் 2024 :)"), "வணக்கம் 2024 :");
    assert_eq!(prep("a  b"), "");
    assert_eq!(prep("Only Latin text here"), "");
}

#[test]
fn pulli_and_vowel_signs_are_preserved() {
    assert_eq!(prep("வணக்கம்"), "வணக்கம்");
    assert_eq!(prep("தமிழ்நாடு"), "தமிழ்நாடு");
    assert_eq!(prep("கொடு"), "கொடு");
    assert_eq!(prep("வணக்கம்"), "வணக்கம்");
}

#[test]
fn digits_are_kept_as_digits_never_spelled() {
    assert_eq!(prep("5"), "5");
    assert_eq!(prep("3.5"), "3.5");
    assert_eq!(prep("நாள் 1"), "நாள் 1");
    assert_eq!(prep("௨௦௨௪"), "௨௦௨௪");
    assert_eq!(prep("100% சரி"), "100 சரி");
}

#[test]
fn allowed_punctuation_is_kept() {
    assert_eq!(
        prep("சரி, ஆம்; இல்லை: உண்மை! நிச்சயமா? போ…"),
        "சரி, ஆம்; இல்லை: உண்மை! நிச்சயமா? போ…"
    );
    assert_eq!(prep("அது' ok"), "அது'");
}

#[test]
fn markdown_and_symbols_are_removed() {
    assert_eq!(prep("**இது** `code` [link](url) <b>"), "இது");
    assert_eq!(prep("வணக்கம்\n\tஉலகம்!"), "வணக்கம் உலகம்!");
}

#[test]
fn whitespace_runs_including_nbsp_collapse_to_one_space() {
    assert_eq!(prep("வணக்கம்\u{00A0}\u{00A0}உலகம்"), "வணக்கம் உலகம்");
    assert_eq!(prep("  வணக்கம்   \n\n  "), "வணக்கம்");
}

#[test]
fn empty_and_whitespace_only_input_gives_empty_output() {
    assert_eq!(prep(""), "");
    assert_eq!(prep("   \n\t\u{00A0} "), "");
}

#[test]
fn has_speakable_tamil_needs_a_letter_or_a_digit() {
    assert!(has_speakable_tamil("வணக்கம்"));
    assert!(has_speakable_tamil("Hello வ"));
    assert!(has_speakable_tamil("123"));
    assert!(has_speakable_tamil("௧"));
    assert!(!has_speakable_tamil("Hello"));
    assert!(!has_speakable_tamil("!!!"));
    assert!(!has_speakable_tamil(" *** ___"));
    assert!(!has_speakable_tamil("ா ி"));
    assert!(!has_speakable_tamil(""));
}
