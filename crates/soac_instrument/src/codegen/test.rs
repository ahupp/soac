use super::{
    instrument_bb_module_for_trace, instrument_bb_module_with_block_entry_counters,
    instrument_bb_module_with_call_target_counters, instrument_bb_module_with_global_load_counters,
    instrument_bb_module_with_locality_counters, instrument_bb_module_with_refcount_counters,
};
use soac_config::ExecTraceConfig;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, Call, ChildVisitable, CounterScope, CounterSite,
    FunctionExecutionMode, NameLike, NameLocation, RuntimeFunctionId, Visit,
};
use soac_lowering::lower_python_to_blockpy_for_testing;
use soac_lowering::passes::{CodegenModuleShape, InstrCodegen};
use std::collections::HashSet;

fn trace_enter_calls(function: &BlockPyFunction<CodegenModuleShape>) -> Vec<&Call<InstrCodegen>> {
    function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|stmt| {
            let InstrCodegen::Call(call) = stmt else {
                return None;
            };
            match call.func.as_ref() {
                InstrCodegen::Load(load) if load.name.is_runtime_symbol("bb_trace_enter") => {
                    Some(call)
                }
                _ => None,
            }
        })
        .collect()
}

fn codegen_module_for_trace_test(source: &str) -> BlockPyModule<CodegenModuleShape> {
    lower_python_to_blockpy_for_testing(source)
        .expect("transform should succeed")
        .codegen_module
}

struct LocalLoadProbe {
    found: bool,
}

impl Visit<InstrCodegen> for LocalLoadProbe {
    fn visit_instr(&mut self, expr: &InstrCodegen) {
        self.found |= matches!(
            expr,
            InstrCodegen::Load(load) if matches!(load.name.location, NameLocation::Local(_))
        );
        expr.visit_children(self);
    }
}

fn expr_tree_contains_local_load(expr: &InstrCodegen) -> bool {
    let mut probe = LocalLoadProbe { found: false };
    probe.visit_instr(expr);
    probe.found
}

fn function_contains_increment_counter(function: &BlockPyFunction<CodegenModuleShape>) -> bool {
    struct IncrementCounterProbe {
        found: bool,
    }

    impl Visit<InstrCodegen> for IncrementCounterProbe {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            self.found |= matches!(expr, InstrCodegen::IncrementCounter(_));
            expr.visit_children(self);
        }
    }

    let mut probe = IncrementCounterProbe { found: false };
    probe.visit_fn(function);
    probe.found
}

fn counter_site_function_id(site: &CounterSite) -> Option<RuntimeFunctionId> {
    match site {
        CounterSite::BlockEntry { function_id, .. }
        | CounterSite::DeoptEntry { function_id, .. } => Some(*function_id),
        CounterSite::Runtime { function_id, .. } => *function_id,
    }
}

#[test]
fn instruments_matching_function_blocks() {
    let source = "def f(x):\n    try:\n        return x + 1\n    except Exception:\n        return 0\n\ndef g(y):\n    return y + 2\n";
    let mut codegen = codegen_module_for_trace_test(source);
    instrument_bb_module_for_trace(
        &mut codegen,
        &ExecTraceConfig {
            qualname_filter: Some("f".to_string()),
            include_params: true,
        },
    );
    let f = codegen
        .callable_defs
        .iter()
        .find(|function| function.names.qualname == "f")
        .expect("missing f");
    let g = codegen
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
    let g_has_trace = !trace_enter_calls(g).is_empty();
    assert!(!g_has_trace);
}

#[test]
fn adds_named_global_load_counters_once() {
    let source = "VALUE = 1\n\ndef f():\n    return VALUE\n";
    let mut codegen = codegen_module_for_trace_test(source);
    instrument_bb_module_with_global_load_counters(&mut codegen);
    instrument_bb_module_with_global_load_counters(&mut codegen);
    let counters = codegen
        .counter_defs
        .iter()
        .filter(|counter| counter.scope == CounterScope::Global)
        .collect::<Vec<_>>();
    assert_eq!(counters.len(), 2);
    assert!(counters.iter().any(|counter| {
        counter.kind == "global_load_hit"
            && counter.site
                == CounterSite::Runtime {
                    function_id: None,
                    instr_id: None,
                }
    }));
    assert!(counters.iter().any(|counter| {
        counter.kind == "global_load_miss"
            && counter.site
                == CounterSite::Runtime {
                    function_id: None,
                    instr_id: None,
                }
    }));
}

#[test]
fn adds_branch_outcome_counters_for_conditional_terms() {
    let source = "def f(x):\n    if x:\n        return 1\n    return 0\n";
    let mut codegen = codegen_module_for_trace_test(source);
    instrument_bb_module_with_locality_counters(&mut codegen);

    let counters = codegen
        .counter_defs
        .iter()
        .filter(|counter| counter.kind == "branch_outcomes")
        .collect::<Vec<_>>();
    assert_eq!(counters.len(), 1);
    assert_eq!(counters[0].scope, CounterScope::This);
    assert!(
        matches!(
            counters[0].site,
            CounterSite::Runtime {
                function_id: Some(_),
                instr_id: Some(_),
            }
        ),
        "branch outcome counter should point at the conditional test instruction"
    );
}

#[test]
fn skips_counter_instrumentation_for_interpreted_functions() {
    let source = r#"
VALUE: int = 1

def f(value: int) -> int:
    if value:
        return value + VALUE
    return 0

class C:
    field: int = 1
"#;
    let mut codegen = codegen_module_for_trace_test(source);
    let interpreted_ids = codegen
        .callable_defs
        .iter()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Interpreted)
        .map(|function| function.function_id)
        .collect::<HashSet<_>>();
    assert!(!interpreted_ids.is_empty());
    assert!(
        codegen.callable_defs.iter().any(|function| {
            interpreted_ids.contains(&function.function_id)
                && function
                    .names
                    .qualname
                    .starts_with("_dp_annotate_func_f.<locals>.")
                && function.names.qualname.ends_with(".<lambda>")
        }),
        "function annotation helper lambda should be interpreted: {:?}",
        codegen
            .callable_defs
            .iter()
            .map(|function| (function.names.qualname.as_str(), function.execution_mode()))
            .collect::<Vec<_>>()
    );

    instrument_bb_module_with_block_entry_counters(&mut codegen);
    instrument_bb_module_with_call_target_counters(&mut codegen);
    instrument_bb_module_with_locality_counters(&mut codegen);
    instrument_bb_module_with_refcount_counters(&mut codegen, CounterScope::Function)
        .expect("function refcount counters should be defined");

    for function in &codegen.callable_defs {
        if interpreted_ids.contains(&function.function_id) {
            assert!(
                !function_contains_increment_counter(function),
                "interpreted function {} should not contain counter increments",
                function.names.qualname
            );
        }
    }
    assert!(
        codegen
            .callable_defs
            .iter()
            .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
            .any(function_contains_increment_counter),
        "JIT functions should still receive block-entry counters"
    );
    assert!(
        codegen
            .counter_defs
            .iter()
            .filter_map(|counter| counter_site_function_id(&counter.site))
            .all(|function_id| !interpreted_ids.contains(&function_id)),
        "counter definitions should not target interpreted functions: {:?}",
        codegen.counter_defs
    );
}
