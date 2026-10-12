//! Opt-in LIVE timing test for the real streaming transport against an Anthropic-Messages
//! gateway. Ignored by default; never runs in normal `cargo test`.
//!
//! Requires `VOX_LIVE_OK=1` plus non-empty `VOX_LIVE_ENDPOINT`, `VOX_LIVE_MODEL`,
//! `VOX_LIVE_TOKEN`. Without them the test prints which variable is missing (names only)
//! and passes. Output never contains the reply text, prompts, endpoint, token or headers.
//! Exactly 7 sequential calls, no retries.

use std::time::{Duration, Instant};

use super::try_complete_streaming;
use crate::managers::converse_plan::{build_plan_prompt, parse_plan, PlanInput, PlanKind};
use crate::managers::converse_stream::PlanStreamer;
use crate::settings::{get_default_settings, AppSettings};

const ACK_SYSTEM: &str =
    "Reply with only a spoken acknowledgement of 3 to 6 words for the user's request. No quotes.";
const STYLE: &str = "warm and brief";
const DEADLINE: Duration = Duration::from_secs(20);

fn live_settings(endpoint: &str, model: &str, token: &str) -> AppSettings {
    let mut s = get_default_settings();
    s.tts_enabled = true;
    s.exp_rt_enabled = true;
    s.exp_rt_direct_endpoint = true;
    s.exp_rt_stream_reply = true;
    s.exp_rt_early_speech = true;
    s.exp_rt_endpoint_url = endpoint.into();
    s.exp_rt_model = model.into();
    s.exp_rt_token = token.into();
    s
}

fn kind_name(kind: PlanKind) -> &'static str {
    match kind {
        PlanKind::Answer => "Answer",
        PlanKind::Paste => "Paste",
        PlanKind::Command => "Command",
        PlanKind::Clarify => "Clarify",
        PlanKind::End => "End",
    }
}

/// Prints one fixed-format line. Returns whether the call succeeded with non-empty text.
fn report(
    label: &str,
    first_delta: Option<u128>,
    first_sentence: Option<u128>,
    total: u128,
    kind: &str,
    chars: usize,
    err: Option<&str>,
) {
    let fmt_opt = |v: Option<u128>| v.map_or_else(|| "-".to_string(), |n| n.to_string());
    match err {
        Some(e) => println!("live {label} error={e} total_ms={total}"),
        None => println!(
            "live {label} first_delta_ms={} first_sentence_ms={} total_ms={total} kind={kind} chars={chars}",
            fmt_opt(first_delta),
            fmt_opt(first_sentence)
        ),
    }
}

/// One planner call: streams deltas into a `PlanStreamer`, then parses the final text.
async fn planner_call(
    label: &str,
    settings: &AppSettings,
    utterance: &str,
    history: &[(String, String)],
    draft: Option<&str>,
) -> bool {
    let prompt = build_plan_prompt(&PlanInput {
        utterance,
        history,
        last_draft: draft,
        style: STYLE,
    });
    let start = Instant::now();
    let mut streamer = PlanStreamer::new();
    let mut first_delta: Option<u128> = None;
    let mut first_sentence: Option<u128> = None;
    let mut on_text = |delta: &str| {
        if first_delta.is_none() {
            first_delta = Some(start.elapsed().as_millis());
        }
        let early = streamer.push(delta);
        if first_sentence.is_none() && !early.is_empty() {
            first_sentence = Some(start.elapsed().as_millis());
        }
    };
    let result = try_complete_streaming(
        settings,
        &prompt.system,
        &prompt.combined,
        DEADLINE,
        &mut on_text,
    )
    .await;
    let total = start.elapsed().as_millis();
    match result {
        Some(Ok(raw)) if !raw.trim().is_empty() => {
            let (kind, chars) = match parse_plan(&raw, &prompt.combined) {
                Ok(plan) => (kind_name(plan.kind), plan.speech.chars().count()),
                Err(_) => ("parse_error", 0),
            };
            report(label, first_delta, first_sentence, total, kind, chars, None);
            true
        }
        Some(Ok(_)) => {
            report(
                label,
                first_delta,
                first_sentence,
                total,
                "-",
                0,
                Some("empty"),
            );
            false
        }
        Some(Err(e)) => {
            report(
                label,
                first_delta,
                first_sentence,
                total,
                "-",
                0,
                Some(e.outcome()),
            );
            false
        }
        None => {
            report(label, None, None, total, "-", 0, Some("no_route"));
            false
        }
    }
}

/// One acknowledgement call: first delta and total time, reply char count.
async fn ack_call(label: &str, settings: &AppSettings, utterance: &str) -> bool {
    let combined = format!("{ACK_SYSTEM}\n\n{utterance}");
    let start = Instant::now();
    let mut first_delta: Option<u128> = None;
    let mut on_text = |_: &str| {
        if first_delta.is_none() {
            first_delta = Some(start.elapsed().as_millis());
        }
    };
    let result =
        try_complete_streaming(settings, ACK_SYSTEM, &combined, DEADLINE, &mut on_text).await;
    let total = start.elapsed().as_millis();
    match result {
        Some(Ok(text)) if !text.trim().is_empty() => {
            report(
                label,
                first_delta,
                None,
                total,
                "ack",
                text.chars().count(),
                None,
            );
            true
        }
        Some(Ok(_)) => {
            report(label, first_delta, None, total, "-", 0, Some("empty"));
            false
        }
        Some(Err(e)) => {
            report(label, first_delta, None, total, "-", 0, Some(e.outcome()));
            false
        }
        None => {
            report(label, None, None, total, "-", 0, Some("no_route"));
            false
        }
    }
}

#[tokio::test]
#[ignore = "live network; needs VOX_LIVE_OK=1 and VOX_LIVE_* env"]
async fn live_gateway_planner_timing() {
    if std::env::var("VOX_LIVE_OK").as_deref() != Ok("1") {
        eprintln!("live skipped: VOX_LIVE_OK is not set to 1");
        return;
    }
    let mut missing = Vec::new();
    let mut values = Vec::new();
    for name in ["VOX_LIVE_ENDPOINT", "VOX_LIVE_MODEL", "VOX_LIVE_TOKEN"] {
        match std::env::var(name) {
            Ok(v) if !v.trim().is_empty() => values.push(v),
            _ => missing.push(name),
        }
    }
    if !missing.is_empty() {
        for name in &missing {
            eprintln!("live skipped: {name} is missing or empty");
        }
        return;
    }
    let settings = live_settings(&values[0], &values[1], &values[2]);

    let mut ok = true;
    // (0) warm-up acknowledgement call.
    ok &= ack_call(
        "0_warmup_ack",
        &settings,
        "open safari and search for the rust async book",
    )
    .await;
    // (1)-(3) planner calls.
    ok &= planner_call(
        "1_plan_mutex",
        &settings,
        "what is the difference between a mutex and a semaphore",
        &[],
        None,
    )
    .await;
    ok &= planner_call(
        "2_plan_safari",
        &settings,
        "open safari and search for the rust async book",
        &[],
        None,
    )
    .await;
    let history = vec![(
        "write a short note to my team about the delay".to_string(),
        "Done.".to_string(),
    )];
    ok &= planner_call(
        "3_plan_revise",
        &settings,
        "make it shorter and more casual",
        &history,
        Some("Hi team, the release is delayed to Friday because of a failing test."),
    )
    .await;
    // (4)-(5) acknowledgement calls.
    ok &= ack_call(
        "4_ack_mutex",
        &settings,
        "what is the difference between a mutex and a semaphore",
    )
    .await;
    ok &= ack_call("5_ack_revise", &settings, "make it shorter and more casual").await;
    // (6) planner call.
    ok &= planner_call(
        "6_plan_end",
        &settings,
        "thanks, that is all for now",
        &[],
        None,
    )
    .await;

    assert!(
        ok,
        "one or more live calls failed or returned empty text (see labelled lines above)"
    );
}
