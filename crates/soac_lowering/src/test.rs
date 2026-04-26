use crate::passes::ast_to_ast::body::Suite;
use crate::template::py_stmt;
use soac_core::block_py::{ChildVisitable, FunctionExecutionMode, PrettyPrint, Visit};
use soac_core::pass_tracker::{PassTracker, RecordingPassTracker};
use soac_ir_blockpy::{
    constructor_entry_function_id_for_init, InstrCodegen, CONSTRUCTOR_ENTRY_FUNCTION_NAME,
};

#[derive(Clone)]
struct TestPrettySuite(Suite);

impl PrettyPrint for TestPrettySuite {
    fn fmt_pretty(&self, printer: &mut soac_core::block_py::PrettyPrinter<'_>) -> std::fmt::Result {
        std::fmt::Write::write_str(printer, &crate::ruff_ast::ruff_ast_to_string(&self.0))
    }
}

#[test]
#[should_panic(expected = "PassTracker already contains a pass named one")]
fn pass_tracker_rejects_duplicate_names() {
    let mut tracker = RecordingPassTracker::new();
    let _suite: TestPrettySuite =
        tracker.run_pass("one", || TestPrettySuite(vec![py_stmt!("x = 1")]));
    let _suite: TestPrettySuite =
        tracker.run_pass("one", || TestPrettySuite(vec![py_stmt!("x = 2")]));
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
    let _suite: TestPrettySuite =
        tracker.run_pass("one", || TestPrettySuite(vec![py_stmt!("x = 1")]));

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

#[test]
fn pure_lowering_does_not_insert_counters() {
    let lowered = crate::lower_python_to_blockpy_for_testing(
        "def f(x):\n    if x:\n        return 1\n    return 0\n",
    )
    .expect("lowering should succeed")
    .codegen_module;

    assert!(lowered.counter_defs.is_empty());

    let mut probe = IncrementCounterProbe::default();
    for function in &lowered.callable_defs {
        for block in &function.blocks {
            for instr in &block.body {
                probe.visit_instr(instr);
            }
            probe.visit_term(&block.term);
        }
    }
    assert_eq!(probe.increment_counters, 0);
}

#[test]
fn class_lowering_adds_constructor_entry_function() {
    let lowered = crate::lower_python_to_blockpy_for_testing(
        "class C:\n    def __init__(self, value, *, scale=1):\n        self.value = value * scale\n",
    )
    .expect("lowering should succeed")
    .codegen_module;

    let init_function = lowered
        .callable_defs
        .iter()
        .find(|function| function.names.qualname == "C.__init__")
        .expect("lowered __init__ should exist");
    let constructor_entry_function_id =
        constructor_entry_function_id_for_init(&lowered, init_function.function_id)
            .expect("constructor entry placeholder should be associated with __init__");
    let constructor_entry = lowered
        .callable_defs
        .iter()
        .find(|function| function.function_id == constructor_entry_function_id)
        .expect("constructor entry placeholder should exist");

    assert_ne!(constructor_entry.function_id, init_function.function_id);
    assert_eq!(
        constructor_entry.names.fn_name,
        CONSTRUCTOR_ENTRY_FUNCTION_NAME
    );
    assert_eq!(
        constructor_entry.execution_mode,
        FunctionExecutionMode::Interpreted
    );
    assert_eq!(
        constructor_entry.params.names(),
        vec!["value".to_string(), "scale".to_string()]
    );
}

#[derive(Default)]
struct IncrementCounterProbe {
    increment_counters: usize,
}

impl Visit<InstrCodegen> for IncrementCounterProbe {
    fn visit_instr(&mut self, expr: &InstrCodegen) {
        if matches!(expr, InstrCodegen::IncrementCounter(_)) {
            self.increment_counters += 1;
        }
        expr.visit_children(self);
    }
}
