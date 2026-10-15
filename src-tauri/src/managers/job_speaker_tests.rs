//! JobSpeaker tests (M3 S3c-2, DESIGN D5, R2-6, R2-8). Red by design: the bodies in
//! `job_speaker.rs` are `todo!()`. Do not edit these tests to make code pass.

use super::{cancel, idle_dto, snapshot_to_dto, status_dto, JobEventSink, JobSpeaker};
use crate::commands::agent_jobs::AgentJobStatusDto;
use crate::managers::agent_jobs::{JobManager, JobObserver, JobSnapshot, JobSpec, JobState};
#[cfg(unix)]
use crate::managers::converse_confirm::{FileId, JobLaunch, LaunchConfig};
use crate::managers::job_announcer::{Announcer, LowPriorityMouth};
#[cfg(unix)]
use std::os::fd::OwnedFd;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

/// Records every text the mouth was offered, and always reports it spoke.
#[derive(Default)]
struct FakeMouth {
    spoken: Mutex<Vec<String>>,
}

impl FakeMouth {
    fn texts(&self) -> Vec<String> {
        self.spoken.lock().map(|g| g.clone()).unwrap_or_default()
    }
}

impl LowPriorityMouth for FakeMouth {
    fn try_speak_if_idle(&self, text: &str) -> bool {
        if let Ok(mut g) = self.spoken.lock() {
            g.push(text.to_string());
        }
        true
    }
}

/// Records every emitted DTO and every tray refresh.
#[derive(Default)]
struct FakeSink {
    events: Mutex<Vec<AgentJobStatusDto>>,
    tray_refreshes: Mutex<usize>,
}

impl FakeSink {
    fn events(&self) -> Vec<AgentJobStatusDto> {
        self.events.lock().map(|g| g.clone()).unwrap_or_default()
    }
    fn tray_refreshes(&self) -> usize {
        self.tray_refreshes.lock().map(|g| *g).unwrap_or(0)
    }
}

impl JobEventSink for FakeSink {
    fn emit(&self, dto: &AgentJobStatusDto) {
        if let Ok(mut g) = self.events.lock() {
            g.push(dto.clone());
        }
    }
    fn refresh_tray(&self) {
        if let Ok(mut g) = self.tray_refreshes.lock() {
            *g += 1;
        }
    }
}

/// Marker strings planted in the job output tail. They must never be spoken, emitted or
/// logged. Built with concat! so no marker appears whole in the source.
fn tail_markers() -> [&'static str; 2] {
    [
        concat!("TAIL_SECRET_", "MARKER_ALPHA"),
        concat!("TAIL_SECRET_", "MARKER_BETA"),
    ]
}

fn snapshot(project: &str, state: JobState, elapsed_secs: u64) -> JobSnapshot {
    JobSnapshot {
        id: 7,
        project: project.to_string(),
        state,
        elapsed_secs,
        tail: vec![
            format!("line one {}", tail_markers()[0]),
            format!("line two {}", tail_markers()[1]),
        ],
    }
}

/// Builds a speaker over fakes. Returns the speaker plus the fakes for inspection.
fn speaker(
    project: &str,
    tts_on: bool,
) -> (JobSpeaker, Arc<FakeMouth>, Arc<FakeSink>, Arc<Announcer>) {
    let mouth = Arc::new(FakeMouth::default());
    let announcer = Arc::new(Announcer::with_timing(
        mouth.clone(),
        Duration::from_millis(10),
        Duration::from_secs(60),
    ));
    let sink = Arc::new(FakeSink::default());
    let flag = Arc::new(move || tts_on);
    let speaker = JobSpeaker::new(project.to_string(), announcer.clone(), flag, sink.clone());
    (speaker, mouth, sink, announcer)
}

/// Finishes a job with `state` and returns what was spoken.
fn finish_and_speak(project: &str, state: JobState, elapsed: u64) -> Vec<String> {
    let (sp, mouth, _sink, _ann) = speaker(project, true);
    sp.on_finish(&snapshot(project, state, elapsed));
    mouth.texts()
}

#[test]
fn finish_succeeded_speaks_exact_success_message() {
    assert_eq!(
        finish_and_speak("proj", JobState::Succeeded, 12),
        vec!["The coding job in proj finished successfully.".to_string()]
    );
}

#[test]
fn finish_failed_with_exit_zero_speaks_code_zero() {
    assert_eq!(
        finish_and_speak("proj", JobState::Failed { code: Some(0) }, 3),
        vec!["The coding job in proj failed with exit code 0.".to_string()]
    );
}

#[test]
fn finish_failed_with_exit_137_speaks_code_137() {
    assert_eq!(
        finish_and_speak("proj", JobState::Failed { code: Some(137) }, 3),
        vec!["The coding job in proj failed with exit code 137.".to_string()]
    );
}

#[test]
fn finish_failed_without_code_speaks_plain_failed() {
    assert_eq!(
        finish_and_speak("proj", JobState::Failed { code: None }, 3),
        vec!["The coding job in proj failed.".to_string()]
    );
}

#[test]
fn finish_cancelled_speaks_was_cancelled() {
    assert_eq!(
        finish_and_speak("proj", JobState::Cancelled, 5),
        vec!["The coding job in proj was cancelled.".to_string()]
    );
}

#[test]
fn finish_timed_out_without_configured_minutes_omits_the_clause() {
    // JobSnapshot carries no configured timeout, so the minutes clause is omitted.
    assert_eq!(
        finish_and_speak("proj", JobState::TimedOut, 600),
        vec!["The coding job in proj timed out.".to_string()]
    );
}

#[test]
fn finish_spawn_failed_speaks_couldnt_start() {
    assert_eq!(
        finish_and_speak("proj", JobState::SpawnFailed, 0),
        vec!["The coding job in proj couldn't start.".to_string()]
    );
}

#[test]
fn finish_speaks_nothing_when_tts_is_disabled() {
    let (sp, mouth, _sink, _ann) = speaker("proj", false);
    sp.on_finish(&snapshot("proj", JobState::Succeeded, 1));
    assert!(mouth.texts().is_empty(), "no speech while tts is off");
}

#[test]
fn finish_still_emits_event_and_refreshes_tray_when_tts_is_disabled() {
    let (sp, _mouth, sink, _ann) = speaker("proj", false);
    sp.on_finish(&snapshot("proj", JobState::Failed { code: Some(2) }, 4));
    assert_eq!(sink.events().len(), 1, "one status event even with tts off");
    assert_eq!(sink.tray_refreshes(), 1, "tray refreshed even with tts off");
}

#[test]
fn finish_emits_final_state_dto_and_refreshes_tray_once() {
    let (sp, _mouth, sink, _ann) = speaker("proj", true);
    sp.on_finish(&snapshot("proj", JobState::Cancelled, 9));
    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].state, "cancelled");
    assert_eq!(events[0].project.as_deref(), Some("proj"));
    assert_eq!(events[0].elapsed_secs, 9);
    assert_eq!(sink.tray_refreshes(), 1);
}

#[test]
fn progress_speaks_still_working_message_as_progress_kind() {
    let (sp, mouth, _sink, _ann) = speaker("proj", true);
    sp.on_progress(&snapshot("proj", JobState::Running, 90));
    assert_eq!(
        mouth.texts(),
        vec!["Still working on the job in proj.".to_string()]
    );
}

#[test]
fn progress_emits_running_event_even_when_tts_is_disabled() {
    let (sp, mouth, sink, _ann) = speaker("proj", false);
    sp.on_progress(&snapshot("proj", JobState::Running, 90));
    assert!(
        mouth.texts().is_empty(),
        "no progress speech while tts is off"
    );
    let events = sink.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].state, "running");
}

#[test]
fn progress_does_not_refresh_the_tray() {
    // Only finishes refresh the tray (D5/R2-8); progress just emits the event.
    let (sp, _mouth, sink, _ann) = speaker("proj", true);
    sp.on_progress(&snapshot("proj", JobState::Running, 90));
    assert_eq!(sink.tray_refreshes(), 0);
}

#[test]
fn output_tail_markers_are_never_spoken_on_progress_or_finish() {
    let (sp, mouth, _sink, _ann) = speaker("proj", true);
    sp.on_progress(&snapshot("proj", JobState::Running, 90));
    sp.on_finish(&snapshot("proj", JobState::Failed { code: Some(1) }, 5));
    for said in mouth.texts() {
        for marker in tail_markers() {
            assert!(!said.contains(marker), "tail leaked into speech: {said}");
        }
    }
}

#[test]
fn output_tail_markers_are_never_present_in_emitted_events() {
    let (sp, _mouth, sink, _ann) = speaker("proj", true);
    sp.on_progress(&snapshot("proj", JobState::Running, 90));
    sp.on_finish(&snapshot("proj", JobState::Succeeded, 5));
    for dto in sink.events() {
        let rendered = format!("{dto:?}");
        for marker in tail_markers() {
            assert!(
                !rendered.contains(marker),
                "tail leaked into event: {rendered}"
            );
        }
    }
}

#[test]
fn project_name_is_sanitized_before_it_is_spoken() {
    // Brackets are replaced with spaces and whitespace collapsed by sanitize_speech_text.
    let (sp, mouth, _sink, _ann) = speaker("[[volm 0]] proj", true);
    sp.on_finish(&snapshot("[[volm 0]] proj", JobState::Succeeded, 1));
    let said = mouth.texts();
    assert_eq!(said.len(), 1);
    assert!(
        !said[0].contains('['),
        "brackets must be stripped: {}",
        said[0]
    );
    assert!(
        !said[0].contains(']'),
        "brackets must be stripped: {}",
        said[0]
    );
    assert!(said[0].contains("finished successfully."));
}

#[test]
fn empty_project_name_still_yields_a_message() {
    // An empty name sanitizes to nothing; the message must still be well-formed, not a
    // dangling "in ." fragment.
    let (sp, mouth, _sink, _ann) = speaker("", true);
    sp.on_finish(&snapshot("", JobState::Succeeded, 1));
    let said = mouth.texts();
    assert_eq!(said.len(), 1, "one announcement for an empty project");
    assert!(
        !said[0].contains(" in ."),
        "no dangling fragment: {}",
        said[0]
    );
}

#[test]
fn dto_maps_every_job_state_to_its_name() {
    let cases = [
        (JobState::Running, "running"),
        (JobState::Succeeded, "succeeded"),
        (JobState::Failed { code: Some(3) }, "failed"),
        (JobState::Failed { code: None }, "failed"),
        (JobState::Cancelled, "cancelled"),
        (JobState::TimedOut, "timed_out"),
        (JobState::SpawnFailed, "spawn_failed"),
    ];
    for (state, want) in cases {
        let dto = snapshot_to_dto(&snapshot("proj", state, 4));
        assert_eq!(dto.state, want, "state name for {state:?}");
    }
}

#[test]
fn dto_carries_project_and_elapsed_seconds_and_no_output() {
    let dto = snapshot_to_dto(&snapshot("alpha", JobState::Running, 42));
    assert_eq!(dto.project.as_deref(), Some("alpha"));
    assert_eq!(dto.elapsed_secs, 42);
    let rendered = format!("{dto:?}");
    for marker in tail_markers() {
        assert!(
            !rendered.contains(marker),
            "dto must not carry output: {rendered}"
        );
    }
}

#[test]
fn idle_dto_is_idle_with_no_project_and_zero_elapsed() {
    let dto = idle_dto();
    assert_eq!(dto.state, "idle");
    assert_eq!(dto.project, None);
    assert_eq!(dto.elapsed_secs, 0);
}

#[test]
fn status_dto_is_idle_when_no_job_has_run() {
    let manager = JobManager::new();
    assert_eq!(status_dto(&manager), idle_dto());
}

#[test]
fn cancel_is_false_when_no_job_is_running() {
    let manager = JobManager::new();
    assert!(!cancel(&manager), "nothing to cancel");
}

/// Kills the recorded fake-CLI process group when dropped, so a failing test never leaves a
/// `sleep` running. A stale id at worst signals nothing.
#[cfg(unix)]
struct GroupGuard {
    pgid: Option<i32>,
}

#[cfg(unix)]
impl Drop for GroupGuard {
    fn drop(&mut self) {
        if let Some(pgid) = self.pgid {
            // SAFETY: SIGKILL to a process group id recorded by this test (the leader pid of a
            // job spawned with process_group(0)); ESRCH is harmless.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(unix)]
fn file_id(path: &Path) -> FileId {
    let meta = std::fs::metadata(path).expect("stat fake binary");
    FileId {
        dev: meta.dev(),
        ino: meta.ino(),
        len: meta.len(),
        mtime_secs: meta.mtime(),
    }
}

/// Writes a fake coding CLI that records its pid in `pidfile` and then sleeps 60 s.
#[cfg(unix)]
fn sleeping_cli(dir: &Path, pidfile: &Path) -> PathBuf {
    let path = dir.join("sleep-cli");
    let body = format!("#!/bin/sh\necho $$ > '{}'\nsleep 60\n", pidfile.display());
    std::fs::write(&path, body).expect("write fake cli");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    path
}

/// Polls `cond` until it is true or `limit` elapses.
#[cfg(unix)]
fn wait_until(limit: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < limit {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    cond()
}

/// Starts a real fake-CLI job in a temp project folder, observed by a `JobSpeaker`.
/// Returns the manager, the sink, the mouth and the group guard.
#[cfg(unix)]
fn start_live_job(tmp: &Path) -> (Arc<JobManager>, Arc<FakeSink>, Arc<FakeMouth>, GroupGuard) {
    let pidfile = tmp.join("job.pid");
    let bin = sleeping_cli(tmp, &pidfile);
    let project_dir = tmp.join("proj");
    std::fs::create_dir_all(&project_dir).expect("create project dir");
    let dir_handle: OwnedFd = std::fs::File::open(&project_dir)
        .expect("open project dir")
        .into();
    let launch = JobLaunch {
        project: "proj".to_string(),
        dir: dir_handle,
        task: "do a thing".to_string(),
        config: LaunchConfig {
            binary: bin.clone(),
            binary_id: file_id(&bin),
            prompt_flag: "-p".to_string(),
            agent_flag: None,
            timeout: Duration::from_secs(60),
        },
        selection: Default::default(),
    };
    let spec = JobSpec {
        launch,
        progress_interval: Duration::from_secs(90),
    };
    let (sp, mouth, sink, _ann) = speaker("proj", true);
    let manager = JobManager::new();
    let observer: Arc<dyn JobObserver> = Arc::new(sp);
    manager
        .start(spec, observer)
        .expect("start must succeed for a fake CLI job");
    let mut guard = GroupGuard { pgid: None };
    let got_pid = wait_until(Duration::from_secs(15), || pidfile.exists());
    if got_pid {
        if let Some(pid) = std::fs::read_to_string(&pidfile)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
        {
            guard.pgid = Some(pid);
        }
    }
    (manager, sink, mouth, guard)
}

#[cfg(unix)]
#[test]
fn status_dto_reports_running_while_a_live_job_runs() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (manager, _sink, _mouth, _guard) = start_live_job(tmp.path());
    let dto = status_dto(&manager);
    assert_eq!(
        dto.state, "running",
        "live job must report running: {dto:?}"
    );
    assert_eq!(dto.project.as_deref(), Some("proj"));
    assert!(cancel(&manager), "cancel must stop the running job");
}

#[cfg(unix)]
#[test]
fn cancel_returns_true_for_a_live_job_then_false_once_it_has_stopped() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (manager, sink, mouth, _guard) = start_live_job(tmp.path());
    assert!(cancel(&manager), "first cancel must return true");
    let stopped = wait_until(Duration::from_secs(15), || !manager.has_active_job());
    assert!(stopped, "cancelled job must stop within the budget");
    assert!(!cancel(&manager), "second cancel must return false");
    assert_eq!(status_dto(&manager).state, "cancelled");
    assert_eq!(
        mouth.texts(),
        vec!["The coding job in proj was cancelled.".to_string()],
        "cancel must speak the cancelled message exactly once"
    );
    assert!(
        sink.events().iter().any(|e| e.state == "cancelled"),
        "a cancelled status event must be emitted"
    );
}
