use crate::block_py::PrettyPrint;
use std::any::Any;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct PassTiming {
    pub name: String,
    pub elapsed: Duration,
}

struct TrackedPass {
    name: String,
    value: Box<dyn Any>,
    render_text: Option<fn(&dyn Any) -> String>,
    render_debug_text: Option<fn(&dyn Any) -> String>,
}

#[derive(Default)]
pub struct NoopPassTracker;

pub struct RecordingPassTracker {
    passes: Vec<TrackedPass>,
    timings: Vec<PassTiming>,
}

pub trait PassTracker {
    fn run_pass<T, F>(&mut self, name: &str, build: F) -> T
    where
        T: Clone + Any + PrettyPrint,
        F: FnOnce() -> T;

    fn record_timing<T, F>(&mut self, name: &str, build: F) -> T
    where
        F: FnOnce() -> T;
}

fn render_tracked_pass_value<T>(value: &dyn Any) -> String
where
    T: Any + PrettyPrint,
{
    value
        .downcast_ref::<T>()
        .expect("tracked pass renderer type should match stored value")
        .pretty_print()
}

fn render_tracked_pass_debug_value<T>(value: &dyn Any) -> String
where
    T: Any + PrettyPrint,
{
    value
        .downcast_ref::<T>()
        .expect("tracked pass renderer type should match stored value")
        .debug_pretty_print()
}

impl NoopPassTracker {
    pub fn new() -> Self {
        Self
    }
}

impl PassTracker for NoopPassTracker {
    fn run_pass<T, F>(&mut self, _name: &str, build: F) -> T
    where
        T: Clone + Any + PrettyPrint,
        F: FnOnce() -> T,
    {
        build()
    }

    fn record_timing<T, F>(&mut self, _name: &str, build: F) -> T
    where
        F: FnOnce() -> T,
    {
        build()
    }
}

impl RecordingPassTracker {
    pub fn new() -> Self {
        Self {
            passes: Vec::new(),
            timings: Vec::new(),
        }
    }

    fn record_pass_timing(&mut self, name: &str, elapsed: Duration) {
        assert!(
            !self.timings.iter().any(|timing| timing.name == name),
            "PassTracker already contains a pass named {name}",
        );
        self.timings.push(PassTiming {
            name: name.to_string(),
            elapsed,
        });
    }

    pub fn get<T: Any>(&self, name: &str) -> Option<&T> {
        self.passes
            .iter()
            .find(|pass| pass.name == name)
            .and_then(|pass| pass.value.downcast_ref::<T>())
    }

    pub fn pass_names(&self) -> impl Iterator<Item = &str> {
        self.passes.iter().map(|pass| pass.name.as_str())
    }

    pub fn render_pass_text(&self, name: &str) -> Option<String> {
        let pass = self.passes.iter().find(|pass| pass.name == name)?;
        pass.render_text.map(|render| render(pass.value.as_ref()))
    }

    pub fn render_pass_debug_text(&self, name: &str) -> Option<String> {
        let pass = self.passes.iter().find(|pass| pass.name == name)?;
        pass.render_debug_text
            .map(|render| render(pass.value.as_ref()))
    }

    pub fn pass_timings(&self) -> impl Iterator<Item = PassTiming> + '_ {
        self.timings.iter().cloned()
    }
}

impl PassTracker for RecordingPassTracker {
    fn run_pass<T, F>(&mut self, name: &str, build: F) -> T
    where
        T: Clone + Any + PrettyPrint,
        F: FnOnce() -> T,
    {
        let value = self.record_timing(name, build);
        self.passes.push(TrackedPass {
            name: name.to_string(),
            value: Box::new(value.clone()),
            render_text: Some(render_tracked_pass_value::<T>),
            render_debug_text: Some(render_tracked_pass_debug_value::<T>),
        });
        value
    }

    fn record_timing<T, F>(&mut self, name: &str, build: F) -> T
    where
        F: FnOnce() -> T,
    {
        let start = Instant::now();
        let value = build();
        let elapsed = start.elapsed();
        self.record_pass_timing(name, elapsed);
        value
    }
}

#[cfg(test)]
mod tests {
    use super::{PassTracker, RecordingPassTracker};
    use crate::block_py::{PrettyPrint, PrettyPrinter};
    use std::fmt;
    use std::fmt::Write;

    #[derive(Clone)]
    struct PrettyValue(&'static str);

    impl PrettyPrint for PrettyValue {
        fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
            printer.write_str(self.0)
        }
    }

    #[test]
    #[should_panic(expected = "PassTracker already contains a pass named one")]
    fn recording_pass_tracker_rejects_duplicate_names() {
        let mut tracker = RecordingPassTracker::new();
        let _: PrettyValue = tracker.run_pass("one", || PrettyValue("first"));
        let _: PrettyValue = tracker.run_pass("one", || PrettyValue("second"));
    }

    #[test]
    fn recording_pass_tracker_records_timing_without_storing_pass_value() {
        let mut tracker = RecordingPassTracker::new();
        let value: i32 = tracker.record_timing("timed-only", || 7);

        assert_eq!(value, 7);
        assert_eq!(tracker.pass_names().collect::<Vec<_>>(), Vec::<&str>::new());

        let timings = tracker.pass_timings().collect::<Vec<_>>();
        assert_eq!(timings.len(), 1);
        assert_eq!(timings[0].name, "timed-only");
    }

    #[test]
    fn recording_pass_tracker_stores_and_renders_pretty_passes() {
        let mut tracker = RecordingPassTracker::new();
        let value: PrettyValue = tracker.run_pass("one", || PrettyValue("rendered"));

        assert_eq!(value.0, "rendered");
        assert_eq!(tracker.pass_names().collect::<Vec<_>>(), vec!["one"]);
        assert_eq!(tracker.get::<PrettyValue>("one").unwrap().0, "rendered");
        assert_eq!(tracker.render_pass_text("one").as_deref(), Some("rendered"));
        assert_eq!(
            tracker.render_pass_debug_text("one").as_deref(),
            Some("rendered")
        );
    }
}
