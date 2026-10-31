//! Manager-level tests for the background-update wiring: the pinned target
//! precondition, `latest_full` on every check path, and the production-source
//! scan that keeps the removed Terminal launcher from coming back.
//! Wired from `source_update.rs` with `#[path]`.

use super::*;

fn full_sha(n: u32) -> String {
    format!("{:040x}", 0x1234_0000_u128 + n as u128)
}

fn info(state: SourceUpdateState, latest_full: &str) -> SourceUpdateInfo {
    SourceUpdateInfo {
        state,
        current_commit: "0123456".to_string(),
        latest_commit: short_sha(latest_full),
        latest_full: latest_full.to_string(),
        behind_by: 1,
        changes: Vec::new(),
        compare_url: String::new(),
        note: None,
        can_install: false,
        install_blocker: None,
    }
}

#[test]
fn pinned_target_accepts_only_an_available_lowercase_full_sha() {
    let full = full_sha(7);
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::Available, &full))),
        Some(full.clone())
    );
    assert_eq!(pinned_target(None), None);
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::UpToDate, &full))),
        None
    );
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::Untracked, &full))),
        None
    );
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::Available, ""))),
        None,
        "an unknown full SHA must refuse to start"
    );
    assert_eq!(
        pinned_target(Some(&info(SourceUpdateState::Available, "0123456"))),
        None,
        "a short SHA must refuse to start"
    );
    assert_eq!(
        pinned_target(Some(&info(
            SourceUpdateState::Available,
            "ABCDEF0123456789ABCDEF0123456789ABCDEF01"
        ))),
        None,
        "uppercase hex must refuse to start"
    );
}

#[test]
fn up_to_date_and_available_checks_carry_the_full_latest_sha() {
    let build = full_sha(1);
    let newest = full_sha(2);
    let window = vec![
        RemoteCommit {
            sha: newest.clone(),
            subject: "new".to_string(),
        },
        RemoteCommit {
            sha: build.clone(),
            subject: "old".to_string(),
        },
    ];
    match evaluate_window(&build, &window).unwrap() {
        WindowEvaluation::Resolved(info) => {
            assert_eq!(info.state, SourceUpdateState::Available);
            assert_eq!(info.latest_full, newest);
        }
        other => panic!("expected Resolved, got {other:?}"),
    }
    let up_to_date = vec![RemoteCommit {
        sha: build.clone(),
        subject: "same".to_string(),
    }];
    match evaluate_window(&build, &up_to_date).unwrap() {
        WindowEvaluation::Resolved(info) => assert_eq!(info.latest_full, build),
        other => panic!("expected Resolved, got {other:?}"),
    }
}

#[test]
fn compare_fallback_path_carries_the_full_latest_sha() {
    let build = full_sha(1);
    let latest = full_sha(9);
    let ahead = resolve_compare(
        &build,
        &latest,
        vec!["x".to_string()],
        Ok(CompareStatus::Ahead { ahead_by: 3 }),
    )
    .unwrap();
    assert_eq!(ahead.state, SourceUpdateState::Available);
    assert_eq!(ahead.latest_full, latest);

    let same = resolve_compare(&build, &latest, Vec::new(), Ok(CompareStatus::Identical)).unwrap();
    assert_eq!(same.latest_full, latest);

    let diverged =
        resolve_compare(&build, &latest, Vec::new(), Ok(CompareStatus::Diverged)).unwrap();
    assert_eq!(diverged.state, SourceUpdateState::Untracked);
    assert_eq!(diverged.latest_full, latest);
}

#[test]
fn latest_full_is_empty_when_the_latest_sha_is_not_a_full_sha() {
    let build = full_sha(1);
    let odd = resolve_compare(
        &build,
        "abc1234",
        Vec::new(),
        Ok(CompareStatus::Ahead { ahead_by: 1 }),
    )
    .unwrap();
    assert_eq!(odd.latest_full, "");
}

/// The part of a source file before its first `#[cfg(test)]` (the test modules).
fn production_part(text: &str) -> &str {
    match text.find("#[cfg(test)]") {
        Some(index) => &text[..index],
        None => text,
    }
}

/// Production code of the update feature must not contain any trace of the
/// removed Terminal launcher: no Terminal app, no osascript, no `open -a`, no
/// `.command` script file. Comments count too.
#[test]
fn production_update_code_has_no_terminal_launcher_left() {
    let sources = [
        (
            "managers/source_update.rs",
            include_str!("source_update.rs"),
        ),
        (
            "managers/source_update_runner.rs",
            include_str!("source_update_runner.rs"),
        ),
        (
            "managers/source_update_runner_poll.rs",
            include_str!("source_update_runner_poll.rs"),
        ),
        (
            "managers/source_update_runner_poller.rs",
            include_str!("source_update_runner_poller.rs"),
        ),
        (
            "managers/source_update_runner_run.rs",
            include_str!("source_update_runner_run.rs"),
        ),
        (
            "commands/source_update.rs",
            include_str!("../commands/source_update.rs"),
        ),
    ];
    for (name, text) in sources {
        let production = production_part(text);
        for needle in ["Terminal", "osascript", "open -a", ".command"] {
            assert!(
                !production.contains(needle),
                "{name} production code still mentions {needle:?}"
            );
        }
    }
}
