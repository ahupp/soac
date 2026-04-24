use super::{define_module_counter_defs, instrument_module_with_tracker};
use crate::InstrumentationConfig;
use soac_config::{RuntimeOptimizationPipeline, SoacEnvConfig, SpecializationMode};
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, ChildVisitable, CounterSite, FunctionExecutionMode, Visit,
};
use soac_core::pass_tracker::NoopPassTracker;
use soac_lowering::lower_python_to_blockpy_for_testing;
use soac_opt::typed::{InstrTyped, TypedCodegenModuleShape, lower_codegen_module_to_typed};
use std::collections::HashSet;

fn typed_module_for_test(source: &str) -> BlockPyModule<TypedCodegenModuleShape> {
    let lowered = lower_python_to_blockpy_for_testing(source).expect("transform should succeed");
    lower_codegen_module_to_typed(lowered.codegen_module)
}

fn function_contains_increment_counter(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> bool {
    struct IncrementCounterProbe {
        found: bool,
    }

    impl Visit<InstrTyped> for IncrementCounterProbe {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            self.found |= matches!(expr, InstrTyped::LegacyIncrementCounter(_));
            expr.visit_children(self);
        }
    }

    let mut probe = IncrementCounterProbe { found: false };
    probe.visit_fn(function);
    probe.found
}

#[test]
fn typed_block_entry_counters_insert_legacy_increment_counters_for_jit_functions() {
    let source = r#"
VALUE: int = 1

def f(value: int) -> int:
    if value:
        return value + VALUE
    return 0

class C:
    field: int = 1
"#;
    let typed = typed_module_for_test(source);
    let interpreted_ids = typed
        .callable_defs
        .iter()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Interpreted)
        .map(|function| function.function_id)
        .collect::<HashSet<_>>();
    assert!(!interpreted_ids.is_empty());
    let config = InstrumentationConfig::from_env_config(
        &SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Profile))
            .with_profiled_cold_blocks_enabled(true)
            .with_runtime_optimization_pipeline(RuntimeOptimizationPipeline::TypedV3),
    );

    let instrumented = instrument_module_with_tracker(typed, &config, &mut NoopPassTracker::new())
        .expect("typed counter instrumentation should succeed");

    assert!(
        instrumented
            .callable_defs
            .iter()
            .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
            .any(function_contains_increment_counter),
        "JIT functions should receive typed block-entry counter increments"
    );
    assert!(
        instrumented
            .callable_defs
            .iter()
            .filter(|function| interpreted_ids.contains(&function.function_id))
            .all(|function| !function_contains_increment_counter(function)),
        "interpreted functions should not receive typed counter increments"
    );
    assert!(
        instrumented.counter_defs.iter().all(|counter| {
            let CounterSite::BlockEntry { function_id, .. } = counter.site else {
                return true;
            };
            !interpreted_ids.contains(&function_id)
        }),
        "typed counter definitions should not target interpreted functions"
    );
}

#[test]
fn typed_counter_definition_scan_records_profile_counter_shapes() {
    let source = r#"
VALUE = 1

def f(xs, i):
    if i:
        xs[0] = VALUE + i
    return VALUE

def g(i):
    return f([], i)
"#;
    let mut typed = typed_module_for_test(source);
    let config = InstrumentationConfig::from_env_config(
        &SoacEnvConfig::default()
            .with_specialization_mode(Some(SpecializationMode::Profile))
            .with_profiled_cold_blocks_enabled(true)
            .with_runtime_optimization_pipeline(RuntimeOptimizationPipeline::TypedV3),
    );

    define_module_counter_defs(&mut typed, &config)
        .expect("typed counter definition scan should succeed");

    for expected_kind in [
        "block_entry",
        "branch_outcomes",
        "global_indexed",
        "operator_hot_shapes",
        "setitem_hot_shapes",
        "setitem_specialized",
        "call_hot_targets",
        "call_direct",
    ] {
        assert!(
            typed
                .counter_defs
                .iter()
                .any(|counter| counter.kind == expected_kind),
            "missing typed counter definition for {expected_kind}"
        );
    }
}
