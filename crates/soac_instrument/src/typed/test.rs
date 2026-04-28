use super::{define_typed_module_counter_defs, instrument_typed_module_with_tracker};
use crate::InstrumentationConfig;
use soac_config::{ExecTraceConfig, SoacEnvConfig, SpecializationMode};
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, ChildVisitable, CounterSite, FunctionExecutionMode, NameLike,
    NameLocation, Visit,
};
use soac_core::pass_tracker::NoopPassTracker;
use soac_ir_typed::{
    InstrTyped, TypedBlockPyModuleShape, TypedCall, lower_blockpy_module_to_typed,
};
use soac_lowering::lower_python_to_blockpy_for_testing;
use std::collections::HashSet;

fn typed_module_for_test(source: &str) -> BlockPyModule<TypedBlockPyModuleShape> {
    let lowered = lower_python_to_blockpy_for_testing(source).expect("transform should succeed");
    lower_blockpy_module_to_typed(lowered.blockpy_module)
}

fn function_contains_increment_counter(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> bool {
    struct IncrementCounterProbe {
        found: bool,
    }

    impl Visit<InstrTyped> for IncrementCounterProbe {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            self.found |= matches!(expr, InstrTyped::IncrementCounter(_));
            expr.visit_children(self);
        }
    }

    let mut probe = IncrementCounterProbe { found: false };
    probe.visit_fn(function);
    probe.found
}

fn trace_enter_calls(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> Vec<&TypedCall<InstrTyped>> {
    function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|stmt| {
            let InstrTyped::CallTyped(call) = stmt else {
                return None;
            };
            match call.func.as_ref() {
                InstrTyped::Load(load) if load.name.is_runtime_symbol("bb_trace_enter") => {
                    Some(call)
                }
                _ => None,
            }
        })
        .collect()
}

fn expr_tree_contains_local_load(expr: &InstrTyped) -> bool {
    struct LocalLoadProbe {
        found: bool,
    }

    impl Visit<InstrTyped> for LocalLoadProbe {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            self.found |= matches!(
                expr,
                InstrTyped::Load(load) if matches!(load.name.location, NameLocation::Local(_))
            );
            expr.visit_children(self);
        }
    }

    let mut probe = LocalLoadProbe { found: false };
    probe.visit_instr(expr);
    probe.found
}

#[test]
fn typed_exec_trace_instruments_matching_function_blocks() {
    let source = "def f(x):\n    try:\n        return x + 1\n    except Exception:\n        return 0\n\ndef g(y):\n    return y + 2\n";
    let typed = typed_module_for_test(source);
    let mut config = InstrumentationConfig::from_env_config(&SoacEnvConfig::default());
    config.trace = Some(ExecTraceConfig {
        qualname_filter: Some("f".to_string()),
        include_params: true,
    });

    let instrumented =
        instrument_typed_module_with_tracker(typed, &config, &mut NoopPassTracker::new())
            .expect("typed trace instrumentation should succeed");

    let f = instrumented
        .callable_defs
        .iter()
        .find(|function| function.names.qualname == "f")
        .expect("missing f");
    let g = instrumented
        .callable_defs
        .iter()
        .find(|function| function.names.qualname == "g")
        .expect("missing g");
    let f_trace_calls = trace_enter_calls(f);
    assert!(!f_trace_calls.is_empty(), "missing trace op in f");
    assert!(
        f_trace_calls.iter().any(|call| call
            .args
            .iter()
            .any(|arg| expr_tree_contains_local_load(arg.expr()))),
        "missing local-load param payload in trace calls"
    );
    assert!(trace_enter_calls(g).is_empty());
}

#[test]
fn typed_block_entry_counters_insert_typed_increment_counters_for_jit_functions() {
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
            .with_profiled_cold_blocks_enabled(true),
    );

    let instrumented =
        instrument_typed_module_with_tracker(typed, &config, &mut NoopPassTracker::new())
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
            .with_profiled_cold_blocks_enabled(true),
    );

    define_typed_module_counter_defs(&mut typed, &config)
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
        "call_direct_targets",
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
