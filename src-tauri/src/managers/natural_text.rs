//! Speech text normalisation for the natural voice.
//!
//! The Kokoro crate's grapheme-to-phoneme step is built for Chinese-plus-English text: it turns
//! the FIRST number of a chunk into Chinese numerals, passes later digits and most symbols
//! through unchanged, and then logs every out-of-vocabulary character. Replies contain numbers,
//! so the engine normalises text BEFORE it reaches the crate: numbers, percentages, currency,
//! ordinals, clock times and symbols become plain English words, markup noise is dropped.
//!
//! Single forward pass over the chars (no regex, no recursion): every char is consumed once.

/// Digit strings longer than this are read digit by digit (they are identifiers, not amounts).
const MAX_CARDINAL_DIGITS: usize = 12;

const ONES: [&str; 20] = [
    "zero",
    "one",
    "two",
    "three",
    "four",
    "five",
    "six",
    "seven",
    "eight",
    "nine",
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];

const TENS: [&str; 10] = [
    "", "", "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];

/// Returns `text` rewritten so it contains only letters (any script), whitespace and the
/// punctuation `. , ! ? ; : ' " - ( ) …`; see `natural_text_tests.rs` for the exact rules.
pub fn normalize_for_speech(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_ascii_digit() {
            i = emit_number_at(&chars, i, &mut out, true);
            continue;
        }
        match c {
            '-' if digit_at(&chars, i + 1)
                && (i == 0 || chars[i - 1].is_whitespace() || chars[i - 1] == '(') =>
            {
                out.push_str(" minus ");
            }
            '$' | '€' | '£' => {
                let unit = match c {
                    '$' => "dollars",
                    '€' => "euros",
                    _ => "pounds",
                };
                if digit_at(&chars, i + 1) {
                    i = emit_number_at(&chars, i + 1, &mut out, false);
                    out.push(' ');
                    out.push_str(unit);
                    space_before_letter(&chars, i, &mut out);
                    continue;
                }
                push_word(&mut out, unit);
            }
            '%' => push_word(&mut out, "percent"),
            '&' => push_word(&mut out, "and"),
            '@' => push_word(&mut out, "at"),
            '+' => push_word(&mut out, "plus"),
            '=' => push_word(&mut out, "equals"),
            '#' => push_word(
                &mut out,
                if digit_at(&chars, i + 1) {
                    "number"
                } else {
                    "hash"
                },
            ),
            '/' | '*' | '_' | '`' | '~' | '^' | '|' | '\\' | '<' | '>' | '[' | ']' | '{' | '}' => {
                out.push(' ');
            }
            _ => out.push(c),
        }
        i += 1;
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn digit_at(chars: &[char], i: usize) -> bool {
    chars.get(i).is_some_and(|c| c.is_ascii_digit())
}

/// End (exclusive) of the ASCII digit run starting at `i`.
fn digit_run_end(chars: &[char], i: usize) -> usize {
    let mut j = i;
    while digit_at(chars, j) {
        j += 1;
    }
    j
}

fn push_word(out: &mut String, word: &str) {
    out.push(' ');
    out.push_str(word);
    out.push(' ');
}

fn space_before_letter(chars: &[char], i: usize, out: &mut String) {
    if chars.get(i).is_some_and(|c| c.is_alphabetic()) {
        out.push(' ');
    }
}

/// Emits the words for the number starting at `chars[start]` (an ASCII digit) and returns the
/// index just past what was consumed. `allow_ordinal_and_clock` is false for currency amounts.
fn emit_number_at(
    chars: &[char],
    start: usize,
    out: &mut String,
    allow_ordinal_and_clock: bool,
) -> usize {
    if out.chars().next_back().is_some_and(|c| c.is_alphabetic()) {
        out.push(' ');
    }
    let first_end = digit_run_end(chars, start);
    let first: String = chars[start..first_end].iter().collect();

    // Clock time H:MM / HH:MM.
    if allow_ordinal_and_clock && first.len() <= 2 && chars.get(first_end) == Some(&':') {
        let m = first_end + 1;
        if digit_run_end(chars, m) == m + 2 {
            let hours: u64 = first.parse().unwrap_or(99);
            let minutes = (chars[m] as u64 - '0' as u64) * 10 + (chars[m + 1] as u64 - '0' as u64);
            if hours <= 24 && minutes <= 59 {
                out.push_str(&cardinal(hours));
                out.push(' ');
                if minutes == 0 {
                    out.push_str("o'clock");
                } else if minutes < 10 {
                    out.push_str("oh ");
                    out.push_str(ONES[minutes as usize]);
                } else {
                    out.push_str(&cardinal(minutes));
                }
                space_before_letter(chars, m + 2, out);
                return m + 2;
            }
        }
    }

    // Integer part, with thousands commas only when groups of exactly three digits follow.
    let mut int_part = first;
    let mut j = first_end;
    if int_part.len() <= 3 && !int_part.starts_with('0') {
        while chars.get(j) == Some(&',') && digit_run_end(chars, j + 1) == j + 4 {
            int_part.extend(&chars[j + 1..j + 4]);
            j += 4;
        }
    }

    // Following `.digits` runs: one = decimal, more = dotted chain (version numbers).
    let mut runs: Vec<String> = Vec::new();
    while chars.get(j) == Some(&'.') && digit_at(chars, j + 1) {
        let end = digit_run_end(chars, j + 1);
        runs.push(chars[j + 1..end].iter().collect());
        j = end;
    }

    match runs.len() {
        0 => {
            let lower = |k: usize| chars.get(k).map(|c| c.to_ascii_lowercase());
            let suffix = (lower(j), lower(j + 1));
            let is_ordinal_suffix = matches!(
                suffix,
                (Some('s'), Some('t'))
                    | (Some('n'), Some('d'))
                    | (Some('r'), Some('d'))
                    | (Some('t'), Some('h'))
            ) && !chars.get(j + 2).is_some_and(|c| c.is_alphanumeric());
            if allow_ordinal_and_clock && is_ordinal_suffix && reads_as_cardinal(&int_part) {
                out.push_str(&ordinal(&cardinal(int_part.parse().unwrap_or(0))));
                j += 2;
            } else {
                out.push_str(&integer_words(&int_part));
            }
        }
        1 => {
            out.push_str(&integer_words(&int_part));
            out.push_str(" point ");
            out.push_str(&digit_by_digit(&runs[0]));
        }
        _ => {
            out.push_str(&integer_words(&int_part));
            for run in &runs {
                out.push_str(" dot ");
                out.push_str(&integer_words(run));
            }
        }
    }
    space_before_letter(chars, j, out);
    j
}

fn reads_as_cardinal(digits: &str) -> bool {
    digits.len() <= MAX_CARDINAL_DIGITS && !(digits.len() > 1 && digits.starts_with('0'))
}

fn integer_words(digits: &str) -> String {
    if reads_as_cardinal(digits) {
        cardinal(digits.parse().unwrap_or(0))
    } else {
        digit_by_digit(digits)
    }
}

fn digit_by_digit(digits: &str) -> String {
    let words: Vec<&str> = digits
        .bytes()
        .map(|b| ONES[usize::from(b - b'0')])
        .collect();
    words.join(" ")
}

fn under_thousand(n: u64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let hundreds = n / 100;
    let rest = n % 100;
    if hundreds > 0 {
        parts.push(format!("{} hundred", ONES[hundreds as usize]));
    }
    if rest > 0 {
        if rest < 20 {
            parts.push(ONES[rest as usize].to_string());
        } else if rest.is_multiple_of(10) {
            parts.push(TENS[(rest / 10) as usize].to_string());
        } else {
            parts.push(format!(
                "{}-{}",
                TENS[(rest / 10) as usize],
                ONES[(rest % 10) as usize]
            ));
        }
    }
    parts.join(" ")
}

/// English cardinal words for `n` (< 10^12).
fn cardinal(n: u64) -> String {
    if n == 0 {
        return "zero".to_string();
    }
    let scales = [
        (1_000_000_000, "billion"),
        (1_000_000, "million"),
        (1_000, "thousand"),
    ];
    let mut parts: Vec<String> = Vec::new();
    let mut rest = n;
    for (size, name) in scales {
        let group = rest / size;
        if group > 0 {
            parts.push(format!("{} {}", under_thousand(group), name));
        }
        rest %= size;
    }
    if rest > 0 {
        parts.push(under_thousand(rest));
    }
    parts.join(" ")
}

/// Turns cardinal words into ordinal words by rewriting the last word.
fn ordinal(words: &str) -> String {
    let split = words.rfind([' ', '-']).map(|p| p + 1).unwrap_or(0);
    let (head, last) = words.split_at(split);
    let last = match last {
        "one" => "first".to_string(),
        "two" => "second".to_string(),
        "three" => "third".to_string(),
        "five" => "fifth".to_string(),
        "eight" => "eighth".to_string(),
        "nine" => "ninth".to_string(),
        "twelve" => "twelfth".to_string(),
        w if w.ends_with('y') => format!("{}ieth", &w[..w.len() - 1]),
        w => format!("{w}th"),
    };
    format!("{head}{last}")
}
