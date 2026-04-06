use super::{
    instrument_bb_module_for_trace, instrument_bb_module_with_global_load_counters,
    parse_trace_config, TraceConfig,
};
use crate::block_py::{CounterScope, CounterSite};
use crate::lower_python_to_blockpy_for_testing;
use crate::passes::{lower_try_jump_exception_flow, normalize_bb_module_strings};

fn tracked_name_binding_module(
    source: &str,
) -> anyhow::Result<Option<crate::block_py::BlockPyModule<crate::passes::ResolvedStorageModuleShape>>>
{
    Ok(lower_python_to_blockpy_for_testing(source)?
        .pass_tracker
        .pass_name_binding()
        .cloned())
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
    instrument_bb_module_for_trace(
        &mut normalized,
        &TraceConfig {
            qualname_filter: Some("f".to_string()),
            include_params: true,
        },
    );
    let f = normalized
        .callable_defs
        .iter()
        .find(|function| function.names.qualname == "f")
        .expect("missing f");
    let g = normalized
        .callable_defs
        .iter()
        .find(|function| function.names.qualname == "g")
        .expect("missing g");
    let f_trace_stmts = f
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .map(crate::block_py::pretty::bb_stmt_text)
        .filter(|stmt| stmt.contains("bb_trace_enter"))
        .collect::<Vec<_>>();
    assert!(!f_trace_stmts.is_empty(), "missing trace op in f");
    assert!(f_trace_stmts
        .iter()
        .any(|stmt| stmt.contains("bb_trace_enter")));
    assert!(f_trace_stmts
        .iter()
        .any(|stmt| stmt.contains("LocalLocation(")));
    let g_has_trace = g
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .map(crate::block_py::pretty::bb_stmt_text)
        .any(|stmt| stmt.contains("bb_trace_enter"));
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
    instrument_bb_module_with_global_load_counters(&mut normalized);
    instrument_bb_module_with_global_load_counters(&mut normalized);
    let counters = normalized
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
