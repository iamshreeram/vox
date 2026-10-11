//! CaptureWorker: the ONLY starter/stopper/canceller of the session's
//! `converse` recording. One thread owns the recorder port; the audio
//! callback only enqueues levels (never blocks, never stops or transcribes).
//! No tauri types: hardware is behind ports. Contract: DESIGN.md 3.4 and the
//! AMENDMENTS section (Revision 4).

use crate::managers::converse::{ListenFailure, ListenOutcome};
use crate::managers::converse_audio::EndpointConfig;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartError {
    Busy,
    NoMicrophone,
    Permission,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscribeFailure;

/// Hardware port. Real impl: AudioRecordingManager with binding "converse".
pub trait RecorderPort: Send + Sync + 'static {
    /// Begin recording. Only ever succeeds when the recorder was idle.
    fn start(&self) -> Result<(), StartError>;
    /// Stop OUR recording and return the samples (None if there is none).
    fn stop(&self) -> Option<Vec<f32>>;
    /// Discard OUR recording; must never touch a foreign recording.
    fn cancel(&self);
}

pub trait Transcriber: Send + Sync + 'static {
    fn transcribe(&self, samples: Vec<f32>) -> Result<String, TranscribeFailure>;
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureTimings {
    /// Pause between `start()` attempts while the recorder is busy.
    pub busy_retry: Duration,
    /// Give up (Failed(Busy)) after this long of continuous Busy.
    pub busy_deadline: Duration,
    /// No levels within this long of a successful start => Failed(NoMicrophone).
    pub readiness_deadline: Duration,
    /// Absolute deadline = initial_grace + max_utterance + this slack => Failed(Other).
    pub abs_slack: Duration,
    /// Worker wake-up period for watchdogs (recv timeout).
    pub tick: Duration,
}

impl Default for CaptureTimings {
    fn default() -> Self {
        CaptureTimings {
            busy_retry: Duration::from_millis(300),
            busy_deadline: Duration::from_secs(5),
            readiness_deadline: Duration::from_millis(3000),
            abs_slack: Duration::from_millis(3000),
            tick: Duration::from_millis(50),
        }
    }
}

/// Cloneable handle to the worker thread. Implementers may change the private
/// fields freely; the public methods are the contract.
#[derive(Clone)]
pub struct CaptureHandle {
    #[allow(dead_code)]
    inner: Arc<()>,
}

pub struct CaptureWorker;

impl CaptureWorker {
    /// Spawn the worker thread.
    pub fn spawn(
        _port: Arc<dyn RecorderPort>,
        _transcriber: Arc<dyn Transcriber>,
        _timings: CaptureTimings,
    ) -> CaptureHandle {
        todo!("S3c")
    }
}

impl CaptureHandle {
    /// One listen turn. Assigns the next listen id, asks the worker to record
    /// until `cfg` says the utterance is over, and resolves exactly once.
    /// Dropping the returned future (e.g. the session loop's cancel race)
    /// cancels that listen id (a drop guard), so a recording can never leak.
    /// A worker that is already gone => `Aborted`.
    pub async fn listen(&self, _cfg: EndpointConfig) -> ListenOutcome {
        todo!("S3c")
    }

    /// Called from the audio callback thread: NON-BLOCKING (`try_send` on a
    /// bounded channel); a no-op unless a listen is armed; never stops,
    /// transcribes or waits.
    pub fn push_levels(&self, _now_ms: u64, _levels: &[f32]) {
        todo!("S3c")
    }

    /// Cancel every listen (current and future ids) of this worker.
    pub fn cancel_all(&self) {
        todo!("S3c")
    }

    /// True while the worker owns the recorder (between a successful
    /// `start()` and the end of that recording).
    pub fn capture_active(&self) -> bool {
        todo!("S3c")
    }

    /// Cancel any active recording, answer any pending listen with `Aborted`,
    /// stop the thread and JOIN it. Blocking. Idempotent.
    pub fn shutdown(&self) {
        todo!("S3c")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{mpsc, Mutex};
    use std::time::Instant;

    // ------------------------- fakes -------------------------

    #[derive(Default)]
    struct FakePort {
        active: AtomicBool,
        starts: AtomicUsize,
        stops: AtomicUsize,
        cancels: AtomicUsize,
        start_script: Mutex<VecDeque<Result<(), StartError>>>,
        /// When the script is empty: this result.
        start_default: Mutex<Option<Result<(), StartError>>>,
        stop_script: Mutex<VecDeque<Option<Vec<f32>>>>,
        /// If set, `start()` blocks until a message arrives.
        start_gate: Mutex<Option<mpsc::Receiver<()>>>,
    }
    impl FakePort {
        fn new() -> Arc<Self> {
            Arc::new(FakePort::default())
        }
        fn script_start(&self, items: Vec<Result<(), StartError>>) {
            self.start_script.lock().unwrap().extend(items);
        }
        fn set_default(&self, r: Result<(), StartError>) {
            *self.start_default.lock().unwrap() = Some(r);
        }
        fn is_active(&self) -> bool {
            self.active.load(Ordering::SeqCst)
        }
    }
    impl RecorderPort for FakePort {
        fn start(&self) -> Result<(), StartError> {
            self.starts.fetch_add(1, Ordering::SeqCst);
            if let Some(gate) = self.start_gate.lock().unwrap().take() {
                let _ = gate.recv();
            }
            let r = self
                .start_script
                .lock()
                .unwrap()
                .pop_front()
                .or(*self.start_default.lock().unwrap())
                .unwrap_or(Ok(()));
            if r.is_ok() {
                self.active.store(true, Ordering::SeqCst);
            }
            r
        }
        fn stop(&self) -> Option<Vec<f32>> {
            self.stops.fetch_add(1, Ordering::SeqCst);
            let was = self.active.swap(false, Ordering::SeqCst);
            let scripted = self.stop_script.lock().unwrap().pop_front();
            match scripted {
                Some(v) => v,
                None if was => Some(vec![0.1; 160]),
                None => None,
            }
        }
        fn cancel(&self) {
            self.cancels.fetch_add(1, Ordering::SeqCst);
            self.active.store(false, Ordering::SeqCst);
        }
    }

    struct FakeTranscriber {
        script: Mutex<VecDeque<Result<String, TranscribeFailure>>>,
        calls: AtomicUsize,
        gate: Mutex<Option<mpsc::Receiver<()>>>,
    }
    impl FakeTranscriber {
        fn new() -> Arc<Self> {
            Arc::new(FakeTranscriber {
                script: Mutex::new(VecDeque::new()),
                calls: AtomicUsize::new(0),
                gate: Mutex::new(None),
            })
        }
    }
    impl Transcriber for FakeTranscriber {
        fn transcribe(&self, _samples: Vec<f32>) -> Result<String, TranscribeFailure> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if let Some(g) = self.gate.lock().unwrap().take() {
                let _ = g.recv();
            }
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok("hello world".to_string()))
        }
    }

    fn fast() -> CaptureTimings {
        CaptureTimings {
            busy_retry: Duration::from_millis(10),
            busy_deadline: Duration::from_millis(150),
            readiness_deadline: Duration::from_millis(150),
            abs_slack: Duration::from_millis(150),
            tick: Duration::from_millis(5),
        }
    }

    fn cfg() -> EndpointConfig {
        EndpointConfig {
            silence_ms: 300,
            initial_grace_ms: 600,
            max_utterance_ms: 5000,
            min_speech_ms: 100,
            floor: 0.02,
        }
    }

    const LOUD: &[f32] = &[0.0, 0.6, 0.2];
    const QUIET: &[f32] = &[0.0, 0.0, 0.0];

    fn spawn(port: &Arc<FakePort>, tr: &Arc<FakeTranscriber>) -> CaptureHandle {
        CaptureWorker::spawn(port.clone(), tr.clone(), fast())
    }

    async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let started = Instant::now();
        while !cond() {
            assert!(
                started.elapsed() < Duration::from_secs(3),
                "timed out waiting for {what}"
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    /// Speech of ~400 ms followed by enough silence to end the utterance.
    fn speak_then_silence(h: &CaptureHandle) {
        let mut t = 0u64;
        while t < 400 {
            h.push_levels(t, LOUD);
            t += 50;
        }
        while t < 1200 {
            h.push_levels(t, QUIET);
            t += 50;
        }
    }

    async fn listen_with_speech(h: &CaptureHandle, port: &Arc<FakePort>) -> ListenOutcome {
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording start", || port.is_active()).await;
        speak_then_silence(h);
        tokio::time::timeout(Duration::from_secs(3), task)
            .await
            .expect("listen hung")
            .unwrap()
    }

    // ------------------------- happy path and outcomes -------------------------

    #[tokio::test]
    async fn happy_path_returns_the_transcript_and_releases_the_recorder() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        assert!(!h.capture_active());
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        assert!(h.capture_active());
        speak_then_silence(&h);
        let out = task.await.unwrap();
        assert_eq!(out, ListenOutcome::Utterance("hello world".into()));
        assert!(!port.is_active());
        assert_eq!(port.stops.load(Ordering::SeqCst), 1);
        wait_for("capture flag cleared", || !h.capture_active()).await;
        h.shutdown();
    }

    #[tokio::test]
    async fn transcript_is_trimmed_and_blank_means_no_speech() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        tr.script
            .lock()
            .unwrap()
            .extend([Ok("  spaced out \n".to_string()), Ok("   ".to_string())]);
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("spaced out".into())
        );
        assert_eq!(listen_with_speech(&h, &port).await, ListenOutcome::NoSpeech);
        h.shutdown();
    }

    #[tokio::test]
    async fn transcriber_failure_is_model_unavailable() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        tr.script.lock().unwrap().push_back(Err(TranscribeFailure));
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Failed(ListenFailure::ModelUnavailable)
        );
        assert!(!port.is_active());
        h.shutdown();
    }

    #[tokio::test]
    async fn missing_samples_after_speech_is_aborted() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        port.stop_script.lock().unwrap().push_back(None);
        let h = spawn(&port, &tr);
        assert_eq!(listen_with_speech(&h, &port).await, ListenOutcome::Aborted);
        assert_eq!(tr.calls.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    #[tokio::test]
    async fn silence_only_is_no_speech_without_transcribing() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        let mut t = 0u64;
        while t <= 700 {
            h.push_levels(t, QUIET);
            t += 50;
        }
        assert_eq!(task.await.unwrap(), ListenOutcome::NoSpeech);
        assert_eq!(
            tr.calls.load(Ordering::SeqCst),
            0,
            "silence must never be transcribed"
        );
        assert!(!port.is_active());
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
        h.shutdown();
    }

    #[tokio::test]
    async fn endless_speech_is_too_long_and_never_transcribed() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let mut c = cfg();
        c.max_utterance_ms = 500;
        let task = tokio::spawn(async move { h2.listen(c).await });
        wait_for("recording", || port.is_active()).await;
        let mut t = 0u64;
        while t <= 600 {
            h.push_levels(t, LOUD);
            t += 50;
        }
        assert_eq!(task.await.unwrap(), ListenOutcome::TooLong);
        assert_eq!(tr.calls.load(Ordering::SeqCst), 0);
        assert!(!port.is_active());
        h.shutdown();
    }

    #[tokio::test]
    async fn sequential_listens_each_get_exactly_one_reply() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        tr.script
            .lock()
            .unwrap()
            .extend([Ok("one".to_string()), Ok("two".to_string())]);
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("one".into())
        );
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("two".into())
        );
        assert_eq!(port.starts.load(Ordering::SeqCst), 2);
        h.shutdown();
    }

    #[tokio::test]
    async fn late_levels_after_the_reply_are_ignored() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("hello world".into())
        );
        for t in 0..20u64 {
            h.push_levels(2000 + t * 50, LOUD);
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(port.stops.load(Ordering::SeqCst), 1);
        assert_eq!(port.cancels.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    // ------------------------- start errors -------------------------

    #[tokio::test]
    async fn permanent_start_errors_are_not_retried() {
        for (e, f) in [
            (StartError::NoMicrophone, ListenFailure::NoMicrophone),
            (StartError::Permission, ListenFailure::Permission),
            (StartError::Other, ListenFailure::Other),
        ] {
            let port = FakePort::new();
            port.set_default(Err(e));
            let tr = FakeTranscriber::new();
            let h = spawn(&port, &tr);
            assert_eq!(h.listen(cfg()).await, ListenOutcome::Failed(f));
            assert_eq!(
                port.starts.load(Ordering::SeqCst),
                1,
                "{e:?} must not be retried"
            );
            assert!(!h.capture_active());
            h.shutdown();
        }
    }

    #[tokio::test]
    async fn busy_is_retried_until_the_recorder_frees_up() {
        let port = FakePort::new();
        port.script_start(vec![Err(StartError::Busy), Err(StartError::Busy)]);
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("hello world".into())
        );
        assert_eq!(port.starts.load(Ordering::SeqCst), 3);
        h.shutdown();
    }

    #[tokio::test]
    async fn busy_forever_fails_busy_and_never_cancels_a_foreign_recording() {
        let port = FakePort::new();
        port.set_default(Err(StartError::Busy));
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let started = Instant::now();
        assert_eq!(
            h.listen(cfg()).await,
            ListenOutcome::Failed(ListenFailure::Busy)
        );
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert_eq!(port.cancels.load(Ordering::SeqCst), 0);
        h.shutdown();
        assert_eq!(
            port.cancels.load(Ordering::SeqCst),
            0,
            "shutdown must not cancel a recording we never owned"
        );
    }

    #[tokio::test]
    async fn cancel_honoured_at_a_busy_retry_boundary() {
        let port = FakePort::new();
        port.set_default(Err(StartError::Busy));
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        tokio::time::sleep(Duration::from_millis(30)).await;
        h.cancel_all();
        let started = Instant::now();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap(),
            ListenOutcome::Aborted
        );
        assert!(
            started.elapsed() < Duration::from_millis(120),
            "must not wait out the busy deadline"
        );
        assert_eq!(port.cancels.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    // ------------------------- cancellation / shutdown -------------------------

    #[tokio::test]
    async fn cancel_before_listen_never_starts_the_recorder() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        h.cancel_all();
        assert_eq!(h.listen(cfg()).await, ListenOutcome::Aborted);
        assert_eq!(port.starts.load(Ordering::SeqCst), 0);
        h.shutdown();
    }

    #[tokio::test]
    async fn dropping_the_listen_future_cancels_the_recording() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        task.abort(); // drops the future => drop guard cancels
        let _ = task.await;
        wait_for("recorder released", || !port.is_active()).await;
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
        assert_eq!(tr.calls.load(Ordering::SeqCst), 0);
        // the worker is still usable afterwards
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("hello world".into())
        );
        h.shutdown();
    }

    #[tokio::test]
    async fn cancel_while_start_is_blocked_releases_the_late_recording() {
        let port = FakePort::new();
        let (gate_tx, gate_rx) = mpsc::channel();
        *port.start_gate.lock().unwrap() = Some(gate_rx);
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("start entered", || port.starts.load(Ordering::SeqCst) == 1).await;
        h.cancel_all();
        tokio::time::sleep(Duration::from_millis(20)).await;
        gate_tx.send(()).unwrap(); // start() now returns Ok => recorder becomes active
        let out = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out, ListenOutcome::Aborted);
        wait_for("late recording cancelled", || !port.is_active()).await;
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
        assert!(!h.capture_active());
        h.shutdown();
    }

    #[tokio::test]
    async fn cancel_during_transcription_discards_the_result_and_the_worker_survives() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let (gate_tx, gate_rx) = mpsc::channel();
        *tr.gate.lock().unwrap() = Some(gate_rx);
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        speak_then_silence(&h);
        wait_for("transcribe entered", || {
            tr.calls.load(Ordering::SeqCst) == 1
        })
        .await;
        task.abort();
        let _ = task.await;
        gate_tx.send(()).unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!port.is_active());
        assert_eq!(
            listen_with_speech(&h, &port).await,
            ListenOutcome::Utterance("hello world".into())
        );
        h.shutdown();
    }

    #[tokio::test]
    async fn shutdown_while_recording_cancels_and_aborts_the_pending_listen() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let task = tokio::spawn(async move { h2.listen(cfg()).await });
        wait_for("recording", || port.is_active()).await;
        let h3 = h.clone();
        tokio::task::spawn_blocking(move || h3.shutdown())
            .await
            .unwrap();
        assert!(
            !port.is_active(),
            "no converse recording may outlive shutdown"
        );
        assert!(!h.capture_active());
        assert_eq!(task.await.unwrap(), ListenOutcome::Aborted);
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn shutdown_is_idempotent_and_listen_after_shutdown_aborts() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        tokio::task::spawn_blocking(move || {
            h2.shutdown();
            h2.shutdown();
        })
        .await
        .unwrap();
        assert_eq!(h.listen(cfg()).await, ListenOutcome::Aborted);
        assert_eq!(port.starts.load(Ordering::SeqCst), 0);
        // harmless after shutdown
        h.push_levels(0, LOUD);
        h.cancel_all();
    }

    // ------------------------- watchdogs -------------------------

    #[tokio::test]
    async fn no_levels_after_a_successful_start_fails_no_microphone() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let started = Instant::now();
        assert_eq!(
            h.listen(cfg()).await,
            ListenOutcome::Failed(ListenFailure::NoMicrophone)
        );
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!port.is_active());
        assert_eq!(port.cancels.load(Ordering::SeqCst), 1);
        h.shutdown();
    }

    #[tokio::test]
    async fn absolute_deadline_ends_a_listen_whose_clock_never_advances() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        let h = spawn(&port, &tr);
        let h2 = h.clone();
        let mut c = cfg();
        c.initial_grace_ms = 100;
        c.max_utterance_ms = 100;
        // abs deadline = 100 + 100 + 150ms slack = 350ms wall clock
        let task = tokio::spawn(async move { h2.listen(c).await });
        wait_for("recording", || port.is_active()).await;
        let feeder = {
            let h = h.clone();
            tokio::spawn(async move {
                // levels keep arriving (readiness satisfied) but now_ms is frozen
                for _ in 0..200 {
                    h.push_levels(0, LOUD);
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
        };
        let out = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
        feeder.abort();
        assert_eq!(out, ListenOutcome::Failed(ListenFailure::Other));
        assert!(!port.is_active());
        h.shutdown();
    }

    // ------------------------- audio-thread safety -------------------------

    #[test]
    fn push_levels_never_blocks_even_when_nobody_is_listening_or_the_queue_is_full() {
        let port = FakePort::new();
        let tr = FakeTranscriber::new();
        // Worker blocked inside start() so nothing drains the channel.
        let (gate_tx, gate_rx) = mpsc::channel();
        *port.start_gate.lock().unwrap() = Some(gate_rx);
        let h = CaptureWorker::spawn(port.clone(), tr.clone(), fast());
        let started = Instant::now();
        for t in 0..10_000u64 {
            h.push_levels(t, LOUD); // not armed: no-op
        }
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let h2 = h.clone();
        let jh = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                tokio::time::timeout(Duration::from_millis(300), h2.listen(cfg()))
                    .await
                    .is_err()
            })
        });
        std::thread::sleep(Duration::from_millis(50));
        for t in 0..10_000u64 {
            h.push_levels(t, LOUD); // armed, worker blocked, queue fills: must still not block
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "push_levels blocked the caller"
        );
        gate_tx.send(()).unwrap();
        let _ = jh.join();
        drop(rt);
        h.shutdown();
    }
}
