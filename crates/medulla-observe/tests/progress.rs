//! The progress guard of a loop.

use medulla_observe::progress::ProgressGuard;
use medulla_observe::registry::tasks::ProgressCursor;

// spec: MEDULLA-OBSERVE-R005.1
#[test]
fn equal_cursors_fail_with_name_and_location() {
    let mut guard = ProgressGuard::with_limit("paging-loop", 2);
    let created_at_line = line!() - 1;
    let cursor = ProgressCursor::Counter(5);
    // A differing cursor is progress; the first cursor differs from none.
    assert!(guard.check(&cursor).is_ok());
    assert!(guard.check(&ProgressCursor::Counter(6)).is_ok());
    let stuck = ProgressCursor::Counter(9);
    assert!(guard.check(&stuck).is_ok());
    // One equal cursor is under the limit; the second in a row fails.
    assert!(guard.check(&stuck).is_ok());
    let error = guard.check(&stuck).unwrap_err();
    assert_eq!(error.name, "paging-loop");
    assert_eq!(error.cursor, stuck);
    assert_eq!(error.idle_iterations, 2);
    assert_eq!(error.code().as_str(), "NO_PROGRESS");
    let text = error.to_string();
    assert!(text.starts_with("NO_PROGRESS: "), "{text}");
    assert!(text.contains("paging-loop"), "{text}");
    assert!(
        text.contains(&format!("progress.rs:{created_at_line}")),
        "{text}"
    );
    // A moving cursor resets the count.
    assert!(guard.check(&ProgressCursor::Counter(10)).is_ok());
    assert!(guard.check(&ProgressCursor::Counter(10)).is_ok());
}

// spec: MEDULLA-OBSERVE-R005.1
#[test]
fn the_default_idle_limit_is_the_knob_default() {
    let mut guard = ProgressGuard::new("default-limit");
    let cursor = ProgressCursor::opaque("same");
    assert!(guard.check(&cursor).is_ok());
    assert!(guard.check(&cursor).is_ok());
    assert_eq!(guard.check(&cursor).unwrap_err().idle_iterations, 2);
}
