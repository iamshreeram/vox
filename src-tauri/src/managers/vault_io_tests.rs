//! Contract tests for the bounded vault-IO worker `VaultIoWorker` (DESIGN Rev 3, section 3.3).
//!
//! These tests run the real `VaultIoWorker::start`, `submit`, `wedged`, `thread_count` and
//! `exit_flag` in `vault_io.rs`. The wedge test takes more than 10 seconds by design,
//! because a job only counts as wedged after it has been running for more than 10 s.

use crate::managers::vault_io::{IoError, VaultIoWorker};
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Upper bound for how long a test waits for the wedge flag to turn on (DESIGN: 10 s threshold).
const WEDGE_WAIT: Duration = Duration::from_secs(15);

/// Boxes a closure as the job type `submit` expects.
fn boxed<T, F>(f: F) -> Box<dyn FnOnce() -> T + Send>
where
    F: FnOnce() -> T + Send + 'static,
    T: 'static,
{
    Box::new(f)
}

#[test]
fn submit_returns_the_job_value() {
    let worker = VaultIoWorker::start();

    let value = worker.submit(boxed(|| 42u32), Duration::from_secs(5));

    assert_eq!(value, Ok(42));
}

#[test]
fn job_past_deadline_is_timed_out_and_wedge_turns_on_only_after_ten_seconds() {
    let worker = VaultIoWorker::start();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    thread::scope(|s| {
        let held = s.spawn(|| {
            worker.submit(
                boxed(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    0u8
                }),
                Duration::from_millis(50),
            )
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("held job started");

        let outcome = held.join().expect("submit thread did not panic");
        assert_eq!(outcome, Err(IoError::TimedOut), "job past its deadline");
        assert!(
            !worker.wedged(),
            "a job that has run for under 10 s must not be wedged yet"
        );

        let began = Instant::now();
        while !worker.wedged() && began.elapsed() < WEDGE_WAIT {
            thread::sleep(Duration::from_millis(50));
        }
        assert!(
            worker.wedged(),
            "wedged() must become true while the job is still running past 10 s"
        );

        // While wedged, new submissions are refused at once.
        let quick = Instant::now();
        let refused = worker.submit(boxed(|| 1u8), Duration::from_secs(5));
        let elapsed = quick.elapsed();
        assert_eq!(
            refused,
            Err(IoError::Unavailable),
            "wedged worker refuses jobs"
        );
        assert!(
            elapsed < Duration::from_millis(100),
            "refusal while wedged must be immediate, took {elapsed:?}"
        );

        // Release the held job; the worker must recover and run new jobs.
        release_tx.send(()).expect("release held job");
        let recovered = Instant::now();
        while worker.wedged() && recovered.elapsed() < Duration::from_secs(5) {
            thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !worker.wedged(),
            "worker must recover after the held job ends"
        );
        assert_eq!(
            worker.submit(boxed(|| 9u8), Duration::from_secs(5)),
            Ok(9),
            "recovered worker runs new jobs"
        );
    });
}

#[test]
fn full_queue_refuses_overflow_immediately_and_accepts_at_most_four_queued() {
    let worker = VaultIoWorker::start();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();

    thread::scope(|s| {
        // Hold the single worker on one job with a long deadline.
        let holder = s.spawn(|| {
            worker.submit(
                boxed(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    0u8
                }),
                Duration::from_secs(60),
            )
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("holder job started");

        // Six submissions with a long deadline: four fit in the queue, two overflow.
        let shared = &worker;
        let (result_tx, result_rx) = mpsc::channel::<Result<u8, IoError>>();
        for _ in 0..6 {
            let result_tx = result_tx.clone();
            s.spawn(move || {
                let outcome = shared.submit(boxed(|| 1u8), Duration::from_secs(30));
                let _ = result_tx.send(outcome);
            });
        }
        drop(result_tx);

        // The two overflow results come back at once, without waiting for the held job.
        for _ in 0..2 {
            let outcome = result_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("overflow submission must be refused quickly");
            assert_eq!(outcome, Err(IoError::Unavailable), "overflow job refused");
        }
        // The other four stay queued behind the held job: no further result yet.
        assert!(
            result_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "queued jobs must wait while the worker is held"
        );

        release_tx.send(()).expect("release holder");
        assert_eq!(holder.join().expect("holder thread"), Ok(0u8));
        for _ in 0..4 {
            let outcome = result_rx
                .recv_timeout(Duration::from_secs(10))
                .expect("queued job completes after release");
            assert_eq!(outcome, Ok(1u8), "queued job runs");
        }
    });
}

#[test]
fn worker_never_spawns_a_second_thread_even_after_timeouts() {
    let worker = VaultIoWorker::start();
    assert_eq!(
        worker.thread_count(),
        1,
        "exactly one vault-io thread at start"
    );

    for _ in 0..5 {
        let outcome = worker.submit(
            boxed(|| {
                thread::sleep(Duration::from_millis(100));
                0u8
            }),
            Duration::from_millis(10),
        );
        assert_eq!(outcome, Err(IoError::TimedOut), "slow job times out");
        // A probe waits behind the slow job and succeeds once it finishes.
        assert_eq!(
            worker.submit(boxed(|| 3u8), Duration::from_secs(5)),
            Ok(3),
            "worker still serves jobs after a timeout"
        );
    }

    assert_eq!(
        worker.thread_count(),
        1,
        "timeouts must never spawn an extra worker thread"
    );
}

#[test]
fn queued_jobs_run_in_submission_order() {
    let worker = VaultIoWorker::start();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let recorded = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));

    thread::scope(|s| {
        let holder = s.spawn(|| {
            worker.submit(
                boxed(move || {
                    let _ = started_tx.send(());
                    let _ = release_rx.recv();
                    0u8
                }),
                Duration::from_secs(60),
            )
        });
        started_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("holder job started");

        // The worker is held, so each job is only enqueued and the wait returns TimedOut at once.
        // The test thread submits them one after another, so each enqueue completes before the
        // next one starts: the queue order is 1, 2, 3 without any sleep or thread scheduling.
        for id in 1u8..=3 {
            let sink = std::sync::Arc::clone(&recorded);
            let outcome = worker.submit(
                boxed(move || {
                    sink.lock().expect("order sink").push(id);
                }),
                Duration::ZERO,
            );
            assert_eq!(
                outcome,
                Err(IoError::TimedOut),
                "job {id} is queued behind the held job"
            );
        }

        release_tx.send(()).expect("release holder");
        assert_eq!(holder.join().expect("holder thread"), Ok(0u8));
    });

    // The queued jobs run after the holder returns; poll with a hard deadline, not a fixed sleep.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let ran = recorded.lock().expect("order sink").len();
        if ran == 3 || Instant::now() >= deadline {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }

    let order: Vec<u8> = recorded.lock().expect("order sink").clone();
    assert_eq!(order, vec![1, 2, 3], "jobs run in submission order");
}

#[test]
fn panicking_job_returns_an_error_and_worker_stays_usable() {
    let worker = VaultIoWorker::start();

    let outcome = worker.submit(
        boxed(|| -> u32 { panic!("synthetic panic in job") }),
        Duration::from_secs(5),
    );

    assert_eq!(
        outcome,
        Err(IoError::Unavailable),
        "a panicking job is reported as Unavailable (no IoError::Panicked variant)"
    );
    assert_eq!(
        worker.submit(boxed(|| 7u32), Duration::from_secs(5)),
        Ok(7),
        "worker must stay usable after a panicking job"
    );
}

#[test]
fn dropping_the_worker_joins_its_thread() {
    let worker = VaultIoWorker::start();
    let exited = worker.exit_flag();
    assert_eq!(worker.thread_count(), 1);
    assert!(
        !exited.load(Ordering::SeqCst),
        "thread still alive before drop"
    );

    drop(worker);

    assert!(
        exited.load(Ordering::SeqCst),
        "dropping the worker must join the vault-io thread before returning"
    );
}
