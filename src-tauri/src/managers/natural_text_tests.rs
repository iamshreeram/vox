//! Spec for `natural_text::normalize_for_speech` (authoritative; do not weaken).

use crate::managers::natural_text::normalize_for_speech as n;

fn check(cases: &[(&str, &str)]) {
    for (input, want) in cases {
        assert_eq!(n(input), *want, "input {input:?}");
    }
}

#[test]
fn plain_text_and_punctuation_pass_through_unchanged() {
    check(&[
        ("", ""),
        ("Hello there.", "Hello there."),
        (
            "Hello, world! How are you? Fine; thanks: ok (yes) 'x' \"y\" - z…",
            "Hello, world! How are you? Fine; thanks: ok (yes) 'x' \"y\" - z…",
        ),
        ("café déjà vu", "café déjà vu"),
    ]);
}

#[test]
fn whitespace_is_collapsed_and_trimmed() {
    check(&[
        ("   ", ""),
        ("  a   b \n c\t", "a b c"),
        ("line one\n\nline two", "line one line two"),
    ]);
}

#[test]
fn integers_are_spelled_out_in_english() {
    check(&[
        ("I have 3 apples.", "I have three apples."),
        ("0", "zero"),
        ("10", "ten"),
        ("13", "thirteen"),
        ("21", "twenty-one"),
        ("100", "one hundred"),
        ("101", "one hundred one"),
        ("999", "nine hundred ninety-nine"),
        ("1000", "one thousand"),
        ("1,234", "one thousand two hundred thirty-four"),
        ("2024", "two thousand twenty-four"),
        ("1000000", "one million"),
        (
            "1234567",
            "one million two hundred thirty-four thousand five hundred sixty-seven",
        ),
        ("(42)", "(forty-two)"),
        ("It costs 5.", "It costs five."),
    ]);
}

#[test]
fn identifiers_are_read_digit_by_digit() {
    check(&[
        ("007", "zero zero seven"),
        ("0123", "zero one two three"),
        (
            "1234567890123",
            "one two three four five six seven eight nine zero one two three",
        ),
    ]);
}

#[test]
fn decimals_negatives_and_percent() {
    check(&[
        ("3.14", "three point one four"),
        ("0.5", "zero point five"),
        ("10.50", "ten point five zero"),
        ("-5", "minus five"),
        ("it is -5 degrees", "it is minus five degrees"),
        ("5-3", "five-three"),
        ("50%", "fifty percent"),
        ("12.5%", "twelve point five percent"),
        ("1.5x", "one point five x"),
    ]);
}

#[test]
fn currency_is_spoken_after_the_amount() {
    check(&[
        ("$5", "five dollars"),
        ("$1,000", "one thousand dollars"),
        ("$12.50", "twelve point five zero dollars"),
        ("€3", "three euros"),
        ("£7", "seven pounds"),
    ]);
}

#[test]
fn ordinals() {
    check(&[
        ("1st", "first"),
        ("2nd", "second"),
        ("3rd", "third"),
        ("4th", "fourth"),
        ("11th", "eleventh"),
        ("12th", "twelfth"),
        ("21st", "twenty-first"),
        ("22nd", "twenty-second"),
        ("30th", "thirtieth"),
        ("40th", "fortieth"),
        ("100th", "one hundredth"),
        ("on May 1st.", "on May first."),
    ]);
}

#[test]
fn clock_times_and_attached_units() {
    check(&[
        ("3:30", "three thirty"),
        ("10:00", "ten o'clock"),
        ("9:05", "nine oh five"),
        ("3:30pm", "three thirty pm"),
        ("3pm", "three pm"),
        ("5km", "five km"),
    ]);
}

#[test]
fn version_numbers_use_dot() {
    check(&[("version 1.2.3", "version one dot two dot three")]);
}

#[test]
fn symbols_become_words_or_spaces() {
    check(&[
        ("R&D", "R and D"),
        ("me@home", "me at home"),
        ("1+1=2", "one plus one equals two"),
        ("#4821", "number four thousand eight hundred twenty-one"),
        ("and/or", "and or"),
        ("a/b", "a b"),
        ("**bold** _it_ `code`", "bold it code"),
        ("***", ""),
        ("~^|\\<>[]{}", ""),
    ]);
}

#[test]
fn output_never_contains_ascii_digits_or_loose_symbols() {
    let inputs = [
        "Call 555-0123 now. Your code is #4821, and 50% off ends at 3:30pm on May 1st for $12.50 & more.",
        "v2.0.1-beta+build.5 (x86_64) [ok] {yes} <b>bold</b> a|b c\\d ~e ^f",
        "9999999999999999999999999 and 0.000001 and 1e10 and 1,2,3",
        "١٢٣ ٤٥٦ 你好 Zażółć ",
    ];
    for input in inputs {
        let out = n(input);
        for c in out.chars() {
            assert!(
                !c.is_ascii_digit(),
                "digit {c:?} survived in {out:?} (from {input:?})"
            );
            assert!(
                !"*_`~^|\\<>[]{}&@+=#$%/€£".contains(c),
                "symbol {c:?} survived in {out:?} (from {input:?})"
            );
        }
    }
}

#[test]
fn normalisation_is_idempotent() {
    let inputs = [
        "Call 555-0123 now. Your code is #4821, and 50% off ends at 3:30pm on May 1st for $12.50 & more.",
        "I have 3 apples.",
        "version 1.2.3",
        "**bold** _it_ `code` 007",
        "  a   b \n c ",
        "",
    ];
    for input in inputs {
        let once = n(input);
        assert_eq!(n(&once), once, "not idempotent for {input:?}");
    }
}

#[test]
fn a_huge_digit_string_is_bounded_and_fast() {
    let input = "9".repeat(5_000);
    let started = std::time::Instant::now();
    let out = n(&input);
    assert!(started.elapsed() < std::time::Duration::from_secs(1));
    assert!(out.split(' ').all(|w| w == "nine"));
    assert_eq!(out.split(' ').count(), 5_000);
}
