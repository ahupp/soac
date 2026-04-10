use super::{
    instrument_bb_module_for_trace, instrument_bb_module_with_global_load_counters,
    instrument_bb_module_with_locality_counters, parse_trace_config, TraceConfig,
};
use crate::block_py::{
    BlockPyFunction, Call, ChildVisitable, CounterScope, CounterSite, InstrCodegen, NameLike,
    NameLocation, Visit,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::passes::{
    assign_module_instr_ids, lower_try_jump_exception_flow, normalize_bb_module_strings,
    CodegenModuleShape,
};

fn tracked_name_binding_module(
    source: &str,
) -> anyhow::Result<Option<crate::block_py::BlockPyModule<crate::passes::ResolvedStorageModuleShape>>>
{
    Ok(lower_python_to_blockpy_for_testing(source)?
        .pass_tracker
        .pass_name_binding()
        .cloned())
}

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

#[test]
fn parses_all_and_params_variants() {
    assert_eq!(
        parse_trace_config("all:params"),
        Some(TraceConfig {
            qualname_filter: None,
            include_params: true,
        })
    );
    assert_eq!(
        parse_trace_config("run"),
        Some(TraceConfig {
            qualname_filter: Some("run".to_string()),
            include_params: false,
        })
    );
    assert_eq!(
        parse_trace_config("run:params"),
        Some(TraceConfig {
            qualname_filter: Some("run".to_string()),
            include_params: true,
        })
    );
    assert_eq!(parse_trace_config("0"), None);
}

#[test]
fn instruments_matching_function_blocks() {
    let source = "def f(x):\n    try:\n        return x + 1\n    except Exception:\n        return 0\n\ndef g(y):\n    return y + 2\n";
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let prepared = lower_try_jump_exception_flow(&bb_module);
    let mut normalized = normalize_bb_module_strings(&prepared);
    crate::passes::relabel_dense_bb_module(&mut normalized);
    let mut codegen = assign_module_instr_ids(normalized);
    instrument_bb_module_for_trace(
        &mut codegen,
        &TraceConfig {
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
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let prepared = lower_try_jump_exception_flow(&bb_module);
    let mut normalized = normalize_bb_module_strings(&prepared);
    crate::passes::relabel_dense_bb_module(&mut normalized);
    let mut codegen = assign_module_instr_ids(normalized);
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
    let bb_module = tracked_name_binding_module(source)
        .expect("transform should succeed")
        .expect("bb module should be available");
    let prepared = lower_try_jump_exception_flow(&bb_module);
    let mut normalized = normalize_bb_module_strings(&prepared);
    crate::passes::relabel_dense_bb_module(&mut normalized);
    let mut codegen = assign_module_instr_ids(normalized);
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
