//! Failure-path tests for the conversation memory clear (V3). A forced database error must
//! not look like success: the epoch still moves, the rows stay, and the command reports the
//! fixed text `clear failed`.

use crate::commands::conversation_memory::{clear_memory, CLEAR_FAILED};
use crate::managers::conversation_memory::ConversationMemory;

/// A store with two rows, then every later write forced to fail.
fn failing_store() -> ConversationMemory {
    let mem = ConversationMemory::open_in_memory().expect("in-memory memory");
    let epoch = mem.epoch();
    mem.add(epoch, 0, 1_000, 2, "first kept summary", false)
        .expect("row one");
    mem.add(epoch, 0, 2_000, 2, "second kept summary", false)
        .expect("row two");
    mem.force_read_only_for_tests();
    mem
}

#[test]
fn clear_error_returns_err_keeps_rows_and_still_bumps_the_epoch() {
    let mem = failing_store();
    let before = mem.epoch();

    assert!(
        mem.clear().is_err(),
        "a forced DB failure must surface as Err"
    );
    assert_eq!(mem.count().expect("count still readable"), 2, "rows kept");
    assert_eq!(
        mem.epoch(),
        before + 1,
        "the epoch moved despite the failure"
    );
}

#[test]
fn a_write_from_before_the_failed_clear_is_refused() {
    let mem = failing_store();
    let before = mem.epoch();
    let _ = mem.clear();

    assert!(
        mem.upgrade(before, 1, "late model text").is_err(),
        "a pre-clear write is stale even though the clear failed"
    );
}

#[test]
fn clear_memory_maps_a_store_error_to_the_fixed_text() {
    let mem = failing_store();

    assert_eq!(clear_memory(Some(&mem)), Err(CLEAR_FAILED.to_string()));
    assert_eq!(CLEAR_FAILED, "clear failed");
    assert_eq!(mem.count().expect("count still readable"), 2);
}
