//! M3 S3c-1: the pure session-parts glue. Tests `job_parts` without an AppHandle.

use super::*;
use crate::managers::agent_jobs::{JobManager, JobObserver, JobSnapshot};
use crate::managers::converse_confirm::PolicySource;
use crate::managers::job_announcer::{Announcer, LowPriorityMouth};
use crate::settings::get_default_settings;
use std::sync::Arc;

/// Test-only mouth: never speaks.
struct SilentMouth;
impl LowPriorityMouth for SilentMouth {
    fn try_speak_if_idle(&self, _text: &str) -> bool {
        false
    }
}

/// Test-only observer: ignores every snapshot.
struct NoopObserver;
impl JobObserver for NoopObserver {
    fn on_progress(&self, _snap: &JobSnapshot) {}
    fn on_finish(&self, _snap: &JobSnapshot) {}
}

fn getter(settings: AppSettings) -> Arc<dyn Fn() -> AppSettings + Send + Sync> {
    Arc::new(move || settings.clone())
}

fn runtime() -> JobRuntime {
    JobRuntime {
        manager: JobManager::new(),
        announcer: Arc::new(Announcer::new(Arc::new(SilentMouth))),
        observer_factory: Arc::new(|_project: &str| Arc::new(NoopObserver) as Arc<dyn JobObserver>),
    }
}

#[test]
fn no_runtime_gives_no_policy_and_no_announcer() {
    let (policy, announcer) = job_parts(None, getter(get_default_settings()));
    assert!(policy.is_none(), "legacy sessions get no policy");
    assert!(announcer.is_none(), "legacy sessions get no announcer");
}

#[test]
fn some_runtime_gives_both_policy_and_announcer() {
    let rt = runtime();
    let (policy, announcer) = job_parts(Some(&rt), getter(get_default_settings()));
    assert!(policy.is_some(), "a runtime gives a policy");
    let announcer = announcer.expect("a runtime gives its announcer");
    assert!(
        Arc::ptr_eq(&announcer, &rt.announcer),
        "the runtime's own announcer"
    );
}

#[test]
fn policy_reads_the_settings_getter_flags() {
    let rt = runtime();
    let mut off = get_default_settings();
    off.agent_jobs_enabled = false;
    let (policy, _) = job_parts(Some(&rt), getter(off));
    let policy = policy.expect("policy");
    assert!(
        !policy.current().jobs_allowed,
        "jobs off in settings => not allowed"
    );
    assert!(
        policy.launch_config().is_none(),
        "no launch config when jobs are off"
    );
}
