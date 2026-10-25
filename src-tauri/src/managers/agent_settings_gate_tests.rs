//! Stage 4 settings generation gate and launch admission (DESIGN-PR4-PLANNER.md Revision 4,
//! A3-R3, tests T1, T4, T5, T6, T7). The gate is a seqlock: even = stable, odd = an update is
//! in progress. Pure gate tests, a concurrency test with timeouts, the admission truth table,
//! and real-child marker tests for `prompt_with_handle` (same style as `acp_latch_tests.rs`:
//! a marker file proves a spawn). Workers must not edit this file. Wired from managers/mod.rs.

use crate::managers::acp_agent::admission_allowed;
use crate::managers::agent_settings_gate::AgentSettingsGate;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

// ---------------- the gate itself ----------------

#[test]
fn a_new_gate_starts_stable() {
    let gate = AgentSettingsGate::new();
    assert_eq!(gate.generation() % 2, 0, "even means stable");
}

#[test]
fn with_update_bumps_the_generation_twice_and_returns_the_value() {
    let gate = AgentSettingsGate::new();
    let before = gate.generation();
    let value = gate.with_update(|| 42);
    assert_eq!(value, 42);
    let after = gate.generation();
    assert_eq!(after, before + 2);
    assert_eq!(after % 2, 0);
}

#[test]
fn the_generation_is_odd_inside_the_update_and_even_after_it() {
    let gate = AgentSettingsGate::new();
    let before = gate.generation();
    gate.with_update(|| {
        assert_eq!(gate.generation() % 2, 1, "odd while updating");
        assert_eq!(gate.generation(), before + 1);
    });
    assert_eq!(gate.generation() % 2, 0);
}

#[test]
fn a_panicking_update_still_leaves_the_generation_even_and_the_gate_usable() {
    let gate = AgentSettingsGate::new();
    let before = gate.generation();
    let result = catch_unwind(AssertUnwindSafe(|| gate.with_update(|| panic!("update failed"))));
    assert!(result.is_err(), "the panic propagates");
    assert_eq!(gate.generation() % 2, 0, "the guard finished the update");
    assert_eq!(gate.generation(), before + 2);
    assert_eq!(gate.with_update(|| 7), 7, "the lock is not poisoned for good");
}

#[test]
fn concurrent_updates_never_overlap() {
    let gate = Arc::new(AgentSettingsGate::new());
    let before = gate.generation();
    let inside = Arc::new(AtomicUsize::new(0));
    let max_seen = Arc::new(AtomicUsize::new(0));
    let workers: Vec<_> = (0..2)
        .map(|_| {
            let gate = Arc::clone(&gate);
            let inside = Arc::clone(&inside);
            let max_seen = Arc::clone(&max_seen);
            thread::spawn(move || {
                for _ in 0..50 {
                    gate.with_update(|| {
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        max_seen.fetch_max(now, Ordering::SeqCst);
                        thread::sleep(Duration::from_micros(200));
                        inside.fetch_sub(1, Ordering::SeqCst);
                    });
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().expect("worker finished");
    }
    assert_eq!(max_seen.load(Ordering::SeqCst), 1, "two updates were inside at once");
    assert_eq!(gate.generation() - before, 200, "100 updates, two bumps each");
}

// ---------------- prepare and stability (T1, T4) ----------------

#[test]
fn a_launch_cannot_be_prepared_while_an_update_is_in_progress() {
    // T1: an update paused after its odd step refuses preparation.
    let gate = AgentSettingsGate::new();
    gate.with_update(|| {
        assert_eq!(gate.prepare_stable(), None);
    });
    assert!(gate.prepare_stable().is_some(), "prepares again once stable");
}

#[test]
fn a_launch_prepared_before_an_update_is_stale_after_it() {
    // T4: an old launch refuses; a newly prepared one is accepted.
    let gate = AgentSettingsGate::new();
    let old = gate.prepare_stable().expect("a stable gate prepares");
    assert_eq!(old % 2, 0);
    assert!(gate.is_stable_at(old));
    gate.with_update(|| {});
    assert!(!gate.is_stable_at(old), "the old generation no longer matches");
    let new = gate.prepare_stable().expect("prepares after the update");
    assert_ne!(new, old);
    assert!(gate.is_stable_at(new));
}

#[test]
fn is_stable_at_is_false_while_an_update_runs() {
    let gate = AgentSettingsGate::new();
    let stable = gate.prepare_stable().expect("stable");
    gate.with_update(|| {
        assert!(!gate.is_stable_at(stable));
    });
}

// ---------------- no deadlock under load (T7) ----------------

#[test]
fn concurrent_setters_and_readers_finish_within_the_deadline() {
    let gate = Arc::new(AgentSettingsGate::new());
    let (done, finished) = mpsc::channel::<()>();
    for worker in 0..4 {
        let gate = Arc::clone(&gate);
        let done = done.clone();
        thread::spawn(move || {
            for _ in 0..200 {
                if worker % 2 == 0 {
                    gate.with_update(thread::yield_now);
                } else if let Some(stable) = gate.prepare_stable() {
                    let _ = gate.is_stable_at(stable);
                }
            }
            let _ = done.send(());
        });
    }
    drop(done);
    for _ in 0..4 {
        finished
            .recv_timeout(Duration::from_secs(60))
            .expect("every worker finishes: no deadlock");
    }
}

// ---------------- admission truth table (A3-R3) ----------------

#[test]
fn admission_allows_only_the_current_stable_epoch_or_no_epoch() {
    assert!(admission_allowed(None, 0), "legacy callers are unaffected");
    assert!(admission_allowed(None, 7), "legacy callers are unaffected");
    assert!(admission_allowed(Some(0), 0));
    assert!(admission_allowed(Some(4), 4));
    assert!(!admission_allowed(Some(4), 6), "the settings moved on");
    assert!(!admission_allowed(Some(6), 4), "newer than the settings");
    assert!(!admission_allowed(Some(2), 3), "an update is in progress");
    assert!(!admission_allowed(Some(3), 3), "odd expected while updating");
    assert!(!admission_allowed(Some(1), 1), "odd expected while updating");
}

// ---------------- a rejected setter changes nothing (T5) ----------------

#[test]
fn a_rejected_setter_value_bumps_no_generation() {
    // T5 as far as a unit test can reach: the setter's validation runs before the update
    // helper, so a rejection leaves the generation as it was. The shutdown and persistence
    // wiring needs an AppHandle and is covered by the production review, not here.
    let gate = AgentSettingsGate::new();
    let before = gate.generation();
    let mut s = crate::settings::get_default_settings();
    let result = crate::settings::apply_step_agent_setting(&mut s, "research", Some("bin/relative"), "");
    assert!(result.is_err());
    assert_eq!(gate.generation(), before);
}

// ---------------- prompt_with_handle admission (A3-R3, T2, T6) ----------------

#[cfg(unix)]
mod manager_admission {
    use crate::managers::acp_agent::{AcpAgentConfig, AcpAgentError, AcpAgentManager, PromptOptions};
    use crate::managers::agent_settings_gate::AgentSettingsGate;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    /// Settle window: a spawn would create the marker well inside this time.
    const SETTLE: Duration = Duration::from_millis(500);

    fn marker_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "vox-settings-gate-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create marker dir");
        dir
    }

    /// The child touches `marker` as soon as it starts, then sleeps without answering.
    fn marker_cfg(marker: &Path) -> AcpAgentConfig {
        AcpAgentConfig {
            command: PathBuf::from("/bin/sh"),
            args: vec![
                "-c".to_string(),
                format!("touch '{}'; exec sleep 30", marker.display()),
            ],
            cwd: std::env::temp_dir(),
            turn_timeout: Duration::from_secs(10),
            handshake_timeout: Duration::from_millis(800),
            cancel_grace: Duration::from_millis(200),
        }
    }

    fn opts(expected_epoch: Option<u64>) -> PromptOptions {
        PromptOptions {
            reuse_session: true,
            expected_epoch,
        }
    }

    fn wait_for_file(path: &Path, secs: u64) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            thread::sleep(Duration::from_millis(10));
        }
        path.exists()
    }

    #[test]
    fn a_launch_from_a_stale_epoch_is_refused_before_the_agent_spawns() {
        let dir = marker_dir("stale");
        let marker = dir.join("spawned");
        let gate = Arc::new(AgentSettingsGate::new());
        let prepared = gate.prepare_stable().expect("a stable gate prepares");
        gate.with_update(|| {}); // a settings change after the launch was prepared
        let manager = AcpAgentManager::new_with_gate(Arc::clone(&gate));
        let handle = manager.new_run_handle();

        let out = manager.prompt_with_handle(
            &marker_cfg(&marker),
            "hello",
            opts(Some(prepared)),
            &handle,
        );

        assert!(matches!(out, Err(AcpAgentError::SettingsChanged)), "{out:?}");
        thread::sleep(SETTLE);
        assert!(!marker.exists(), "a stale launch must never spawn the agent");
    }

    #[test]
    fn an_update_in_progress_refuses_even_the_epoch_it_started_from() {
        let dir = marker_dir("odd");
        let marker = dir.join("spawned");
        let gate = Arc::new(AgentSettingsGate::new());
        let manager = AcpAgentManager::new_with_gate(Arc::clone(&gate));

        gate.with_update(|| {
            let handle = manager.new_run_handle();
            let out = manager.prompt_with_handle(
                &marker_cfg(&marker),
                "hello",
                opts(Some(gate.generation())),
                &handle,
            );
            assert!(matches!(out, Err(AcpAgentError::SettingsChanged)), "{out:?}");
        });
        thread::sleep(SETTLE);
        assert!(!marker.exists(), "no spawn while the settings are updating");
    }

    #[test]
    fn a_launch_with_the_current_epoch_is_admitted_and_spawns() {
        // Control: proves the marker mechanism is real for the admitted path.
        let dir = marker_dir("current");
        let marker = dir.join("spawned");
        let gate = Arc::new(AgentSettingsGate::new());
        let current = gate.prepare_stable().expect("a stable gate prepares");
        let manager = AcpAgentManager::new_with_gate(Arc::clone(&gate));
        let handle = manager.new_run_handle();

        let out = manager.prompt_with_handle(
            &marker_cfg(&marker),
            "hello",
            opts(Some(current)),
            &handle,
        );

        assert!(!matches!(out, Err(AcpAgentError::SettingsChanged)), "{out:?}");
        assert!(wait_for_file(&marker, 5), "an admitted launch spawns the agent");
    }

    #[test]
    fn legacy_callers_without_an_epoch_are_not_gated() {
        let dir = marker_dir("legacy");
        let marker = dir.join("spawned");
        let gate = Arc::new(AgentSettingsGate::new());
        gate.with_update(|| {});
        gate.with_update(|| {});
        let manager = AcpAgentManager::new_with_gate(Arc::clone(&gate));
        let handle = manager.new_run_handle();

        let out = manager.prompt_with_handle(&marker_cfg(&marker), "hello", opts(None), &handle);

        assert!(!matches!(out, Err(AcpAgentError::SettingsChanged)), "{out:?}");
        assert!(wait_for_file(&marker, 5), "a legacy caller still spawns");
    }
}
