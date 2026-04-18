use crate::pass_tracker::{PassTracker, RecordingPassTracker};
use crate::passes::ast_to_ast::body::Suite;
use crate::py_stmt;

#[test]
#[should_panic(expected = "PassTracker already contains a pass named one")]
fn pass_tracker_rejects_duplicate_names() {
    let mut tracker = RecordingPassTracker::new();
    let _suite: Suite = tracker.run_pass("one", || vec![py_stmt!("x = 1")]);
    let _suite: Suite = tracker.run_pass("one", || vec![py_stmt!("x = 2")]);
}

#[test]
fn pass_tracker_records_timing_without_storing_pass_value() {
    let mut tracker = RecordingPassTracker::new();
    let value: i32 = tracker.record_timing("timed-only", || 7);

    assert_eq!(value, 7);
    assert_eq!(
        tracker
            .pass_timings()
            .map(|timing| timing.name)
            .collect::<Vec<_>>(),
        vec!["timed-only".to_string()]
    );
    assert_eq!(tracker.render_pass_text("timed-only"), None);
    assert_eq!(tracker.render_pass_debug_text("timed-only"), None);
}

#[test]
fn pass_tracker_renders_tracked_pass_text_for_renderable_passes() {
    let mut tracker = RecordingPassTracker::new();
    let _suite: Suite = tracker.run_pass("one", || vec![py_stmt!("x = 1")]);

    assert_eq!(tracker.render_pass_text("one").as_deref(), Some("x = 1\n"));
    assert_eq!(
        tracker.render_pass_debug_text("one").as_deref(),
        Some("x = 1\n")
    );
    assert_eq!(
        tracker
            .pass_timings()
            .map(|timing| timing.name)
            .collect::<Vec<_>>(),
        vec!["one".to_string()]
    );
}
