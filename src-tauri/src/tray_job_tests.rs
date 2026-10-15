//! Tray "Cancel coding job" enabled-state tests (M3 S3c-2, DESIGN D6). Red by design: the
//! pure function `cancel_job_item_enabled` is `todo!()`. Do not edit these tests to make code pass.
//!
//! The menu item and its click handler live in `tray.rs`; the enable rule is a pure function
//! so it can be unit-tested without a Tauri app handle.

use super::cancel_job_item_enabled;

#[test]
fn cancel_item_is_enabled_only_while_a_job_is_running() {
    assert!(cancel_job_item_enabled(true));
}

#[test]
fn cancel_item_is_disabled_when_no_job_is_running() {
    assert!(!cancel_job_item_enabled(false));
}

#[test]
fn cancel_item_enabled_state_flips_with_the_running_flag() {
    // The item follows the flag in both directions across a job's lifecycle.
    let lifecycle = [false, true, true, false];
    let got: Vec<bool> = lifecycle
        .iter()
        .map(|r| cancel_job_item_enabled(*r))
        .collect();
    assert_eq!(got, vec![false, true, true, false]);
}
