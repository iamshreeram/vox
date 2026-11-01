//! Tests for the adapters that connect `auto_refine` to the real slots: `SlotLid` (language
//! identifier, capped at `LID_MAX_SECS`) and `SlotTamil` (Tamil pack, whole audio). Fakes only;
//! no model file is needed. Busy cases use a channel gate with timeouts so a failure cannot hang.

use super::*;
use crate::managers::auto_lang::{LidIdentify, LidOutcome, LidResult, TamilPackRoute};
use crate::managers::indic_stt_slot::{Clock, IndicSession, IndicSlot, UnloadPolicy};
use crate::managers::lid_slot::{LidLoader, LidSession, LidSlot};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

/// Sample lengths the fake sessions received, in call order.
type Seen = Arc<Mutex<Vec<usize>>>;

/// Lets a test pause a session inside its first call: the session reports on `entered`, then
/// waits for one message on `release` (or gives up after ten seconds).
struct Gate {
    entered: Mutex<mpsc::Sender<()>>,
    release: Mutex<mpsc::Receiver<()>>,
}

/// Builds a gate and the test-side ends: (gate, entered receiver, release sender).
fn gate() -> (Arc<Gate>, mpsc::Receiver<()>, mpsc::Sender<()>) {
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let gate = Gate {
        entered: Mutex::new(entered_tx),
        release: Mutex::new(release_rx),
    };
    (Arc::new(gate), entered_rx, release_tx)
}

/// Waits inside the session until the test releases it (bounded).
fn pass_gate(gate: &Gate) {
    if let Ok(sender) = gate.entered.lock() {
        let _ = sender.send(());
    }
    if let Ok(receiver) = gate.release.lock() {
        let _ = receiver.recv_timeout(Duration::from_secs(10));
    }
}

/// A clock that does not move; the adapter tests do not depend on time.
struct FixedClock(AtomicU64);

impl Clock for FixedClock {
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(FixedClock(AtomicU64::new(0)))
}

fn seconds_of_audio(seconds: usize) -> Vec<f32> {
    vec![0.25; seconds * 16_000]
}

// ---- language identifier ----

struct FakeLidSession {
    seen: Seen,
    reply: Result<LidResult, String>,
    gate: Option<Arc<Gate>>,
}

impl LidSession for FakeLidSession {
    fn identify(&mut self, samples: &[f32]) -> Result<LidResult, String> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(samples.len());
        }
        if let Some(gate) = self.gate.take() {
            pass_gate(&gate);
        }
        self.reply.clone()
    }
}

fn lid_slot(
    reply: Result<LidResult, String>,
    seen: &Seen,
    gate: Option<Arc<Gate>>,
) -> Arc<LidSlot> {
    let seen = seen.clone();
    let loader: LidLoader = Box::new(move || {
        Ok(Box::new(FakeLidSession {
            seen: seen.clone(),
            reply: reply.clone(),
            gate: gate.clone(),
        }) as Box<dyn LidSession>)
    });
    Arc::new(LidSlot::new(loader, clock()))
}

fn failing_lid_slot(message: &str) -> Arc<LidSlot> {
    let message = message.to_string();
    let loader: LidLoader = Box::new(move || Err(message.clone()));
    Arc::new(LidSlot::new(loader, clock()))
}

fn hindi_result() -> LidResult {
    LidResult {
        lang: Some("hi".to_string()),
        text: "नमस्ते दुनिया".to_string(),
    }
}

fn lid(slot: &Arc<LidSlot>) -> SlotLid {
    SlotLid {
        slot: slot.clone(),
        policy: UnloadPolicy::Never,
    }
}

#[test]
fn lid_forty_seconds_of_audio_reaches_the_session_capped_at_thirty() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let slot = lid_slot(Ok(hindi_result()), &seen, None);

    let _ = lid(&slot).identify(&seconds_of_audio(40));

    assert_eq!(*seen.lock().unwrap(), vec![480_000]);
}

#[test]
fn lid_ten_seconds_of_audio_reaches_the_session_unchanged() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let slot = lid_slot(Ok(hindi_result()), &seen, None);

    let _ = lid(&slot).identify(&seconds_of_audio(10));

    assert_eq!(*seen.lock().unwrap(), vec![160_000]);
}

#[test]
fn lid_returns_the_identifier_result_unchanged() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let slot = lid_slot(Ok(hindi_result()), &seen, None);

    assert_eq!(
        lid(&slot).identify(&seconds_of_audio(2)),
        LidOutcome::Done(hindi_result())
    );
}

#[test]
fn lid_session_error_maps_to_unavailable_with_its_message() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let slot = lid_slot(Err("boom".to_string()), &seen, None);

    assert_eq!(
        lid(&slot).identify(&seconds_of_audio(2)),
        LidOutcome::Unavailable("boom".to_string())
    );
}

#[test]
fn lid_load_error_maps_to_unavailable_with_its_message() {
    let slot = failing_lid_slot("no multilingual Whisper model installed");

    assert_eq!(
        lid(&slot).identify(&seconds_of_audio(2)),
        LidOutcome::Unavailable("no multilingual Whisper model installed".to_string())
    );
}

#[test]
fn lid_busy_slot_maps_to_busy_while_another_call_runs() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let (gate, entered, release) = gate();
    let slot = lid_slot(Ok(hindi_result()), &seen, Some(gate));

    let worker_slot = slot.clone();
    let worker = thread::spawn(move || lid(&worker_slot).identify(&seconds_of_audio(2)));
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("the first call reached the session");

    let during = lid(&slot).identify(&seconds_of_audio(2));
    release.send(()).expect("release the first call");
    let first = worker.join().expect("first call finished");

    assert_eq!(during, LidOutcome::Busy);
    assert_eq!(first, LidOutcome::Done(hindi_result()));
}

// ---- Tamil pack ----

struct FakeIndicSession {
    seen: Seen,
    reply: Result<String, String>,
    gate: Option<Arc<Gate>>,
}

impl IndicSession for FakeIndicSession {
    fn transcribe(&mut self, samples: &[f32]) -> Result<String, String> {
        if let Ok(mut seen) = self.seen.lock() {
            seen.push(samples.len());
        }
        if let Some(gate) = self.gate.take() {
            pass_gate(&gate);
        }
        self.reply.clone()
    }
}

fn indic_slot(reply: Result<String, String>, seen: &Seen, gate: Option<Arc<Gate>>) -> IndicSlot {
    let seen = seen.clone();
    IndicSlot::new(
        Box::new(move || {
            Ok(Box::new(FakeIndicSession {
                seen: seen.clone(),
                reply: reply.clone(),
                gate: gate.clone(),
            }) as Box<dyn IndicSession>)
        }),
        clock(),
    )
}

fn failing_indic_slot(message: &str) -> IndicSlot {
    let message = message.to_string();
    IndicSlot::new(Box::new(move || Err(message.clone())), clock())
}

fn tamil(slot: &IndicSlot) -> SlotTamil<'_> {
    SlotTamil {
        slot,
        policy: UnloadPolicy::Never,
    }
}

#[test]
fn tamil_text_is_returned_as_is() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let slot = indic_slot(Ok("வணக்கம் தமிழ்நாடு".to_string()), &seen, None);

    assert_eq!(
        tamil(&slot).transcribe(&seconds_of_audio(2)),
        Some("வணக்கம் தமிழ்நாடு".to_string())
    );
}

#[test]
fn tamil_blank_text_maps_to_none() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    for blank in ["", "   ", "\n\t"] {
        let slot = indic_slot(Ok(blank.to_string()), &seen, None);
        assert_eq!(
            tamil(&slot).transcribe(&seconds_of_audio(2)),
            None,
            "{blank:?}"
        );
    }
}

#[test]
fn tamil_run_error_maps_to_none() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let slot = indic_slot(Err("boom".to_string()), &seen, None);

    assert_eq!(tamil(&slot).transcribe(&seconds_of_audio(2)), None);
}

#[test]
fn tamil_load_error_maps_to_none() {
    let slot = failing_indic_slot("pack missing");

    assert_eq!(tamil(&slot).transcribe(&seconds_of_audio(2)), None);
}

#[test]
fn tamil_busy_slot_maps_to_none() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let (gate, entered, release) = gate();
    let slot = Arc::new(indic_slot(Ok("தமிழ்".to_string()), &seen, Some(gate)));

    let worker_slot = slot.clone();
    let worker = thread::spawn(move || {
        SlotTamil {
            slot: &worker_slot,
            policy: UnloadPolicy::Never,
        }
        .transcribe(&seconds_of_audio(2))
    });
    entered
        .recv_timeout(Duration::from_secs(10))
        .expect("the first call reached the session");

    let during = tamil(&slot).transcribe(&seconds_of_audio(2));
    release.send(()).expect("release the first call");
    let first = worker.join().expect("first call finished");

    assert_eq!(during, None);
    assert_eq!(first, Some("தமிழ்".to_string()));
}

#[test]
fn tamil_gets_the_whole_audio_not_a_capped_slice() {
    let seen: Seen = Arc::new(Mutex::new(Vec::new()));
    let slot = indic_slot(Ok("தமிழ்".to_string()), &seen, None);

    let _ = tamil(&slot).transcribe(&seconds_of_audio(40));

    assert_eq!(*seen.lock().unwrap(), vec![640_000]);
}
