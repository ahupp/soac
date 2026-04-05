use soac_blockpy::block_py::FunctionKind;
use soac_jit::{
    exc_dispatch_plan, jit_param_names_for_block, lookup_blockpy_function,
    register_clif_module_plans,
};
use std::any::Any;
use std::collections::HashSet;

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn parse_and_lower(source: &str) -> Result<soac_blockpy::LoweringResult, String> {
    match std::panic::catch_unwind(|| soac_blockpy::lower_python_to_blockpy_for_testing(source)) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(panic_payload_to_string(payload)),
    }
}

fn parse_and_lower_runtime_style(source: &str) -> Result<soac_blockpy::LoweringResult, String> {
    match std::panic::catch_unwind(|| soac_blockpy::lower_python_to_blockpy_for_testing(source)) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(panic_payload_to_string(payload)),
    }
}

fn validate_bb_module_for_jit(
    bb_module: &soac_blockpy::block_py::BlockPyModule<soac_blockpy::passes::CodegenBlockPyPass>,
) -> Result<(), String> {
    for function in &bb_module.callable_defs {
        match function.lowered_kind() {
            FunctionKind::Function
            | FunctionKind::Coroutine
            | FunctionKind::Generator
            | FunctionKind::AsyncGenerator => {}
        }
    }
    Ok(())
}

fn run_cranelift_jit_preflight(result: &soac_blockpy::LoweringResult) -> Result<(), String> {
    soac_jit::run_cranelift_smoke(&result.codegen_module)
}

#[test]
fn function_plan_reports_slot_inventory_for_locals_capture_and_except_state() {
    let source = r#"
def outer(scale):
    factor = scale
    def inner(x):
        total = x
        try:
            total += factor
        except Exception as exc:
            return total + len(str(exc))
        return total
    return inner
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let normalized = result.codegen_module.clone();
    let module_name = "jit_plan_slot_inventory_test";
    register_clif_module_plans(module_name, &normalized)
        .expect("plan registration should succeed");
    let inner_function = normalized
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "inner")
        .expect("missing lowered inner function");
    let registered_function = lookup_blockpy_function(module_name, inner_function.function_id)
        .expect("registered plan should exist");
    let storage_layout = inner_function
        .storage_layout()
        .as_ref()
        .expect("inner function should preserve closure layout");
    let slot_names = storage_layout.stack_slots().to_vec();
    let freevar_names = storage_layout
        .freevars
        .iter()
        .map(|slot| slot.storage_name.clone())
        .collect::<Vec<_>>();

    assert_eq!(
        freevar_names.len(),
        1,
        "expected one closure capture in storage layout freevars: {:?}",
        freevar_names
    );
    let capture_name = &freevar_names[0];
    assert!(
        capture_name.contains("factor"),
        "expected capture name to track factor: {capture_name:?}"
    );
    assert!(
        slot_names.iter().any(|name| name == "x"),
        "expected parameter x in slot inventory: {:?}",
        slot_names
    );
    assert!(
        slot_names.iter().any(|name| name == "total"),
        "expected local total in slot inventory: {:?}",
        slot_names
    );
    assert!(
        slot_names
            .iter()
            .any(|name| name.starts_with("_dp_try_exc_")),
        "expected synthetic try-exception state in slot inventory: {:?}",
        slot_names
    );

    let unique_names = slot_names.iter().collect::<HashSet<_>>();
    assert_eq!(
        unique_names.len(),
        slot_names.len(),
        "slot inventory should not duplicate names: {:?}",
        slot_names
    );
    assert_eq!(registered_function.params, inner_function.params);
}

#[test]
fn jit_validator_accepts_class_defs_without_def_fn_ops() {
    let source = r#"
class C:
    x = 1
    def m(self):
        return self.x
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let bb_module = &result.codegen_module;
    validate_bb_module_for_jit(bb_module).expect("validator should accept lowered class defs");
}

#[test]
fn jit_validator_accepts_coroutines() {
    let source = r#"
async def run():
    return 1
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let bb_module = &result.codegen_module;
    validate_bb_module_for_jit(bb_module).expect("validator should accept coroutine lowering");
}

#[test]
fn jit_validator_accepts_async_generators() {
    let source = r#"
async def run():
    yield 1
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let bb_module = &result.codegen_module;
    validate_bb_module_for_jit(bb_module)
        .expect("validator should accept async generator lowering");
}

#[test]
fn jit_validator_accepts_lowered_try_blocks() {
    let source = r#"
def f():
    try:
        return 1
    except Exception:
        return 2
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let bb_module = &result.codegen_module;
    validate_bb_module_for_jit(bb_module).expect("validator should accept lowered try blocks");
}

#[test]
fn jit_preflight_runs_cranelift_for_supported_module() {
    let source = r#"
def f(x):
    return x
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let bb_module = &result.codegen_module;
    validate_bb_module_for_jit(bb_module).expect("validator should allow module");
    run_cranelift_jit_preflight(&result).expect("cranelift preflight should run");
}

#[test]
fn generator_throw_handler_plan_keeps_try_exception_state_and_closure_exc_binding() {
    let source = r#"
def exercise():
    outer_capture = 2
    def gen():
        total = 1
        try:
            total += outer_capture
            yield total
        except ValueError as exc:
            total += len(str(exc))
        yield total
    return gen
    "#;
    let result = parse_and_lower_runtime_style(source).expect("lowering should succeed");
    let normalized = result.codegen_module.clone();
    let module_name = "jit_plan_generator_throw_handler_param_test";
    register_clif_module_plans(module_name, &normalized)
        .expect("plan registration should succeed");
    let gen_function = normalized
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "gen_resume")
        .expect("missing lowered generator resume function");
    let registered_function = lookup_blockpy_function(module_name, gen_function.function_id)
        .expect("registered plan should exist");
    let plan_runtime_param_names = registered_function
        .blocks
        .iter()
        .map(jit_param_names_for_block)
        .collect::<Vec<_>>();
    let plan_exc_dispatches = registered_function
        .blocks
        .iter()
        .map(|block| exc_dispatch_plan(&registered_function, block))
        .collect::<Vec<_>>();

    let handler_entry_targets = plan_runtime_param_names
        .iter()
        .enumerate()
        .filter(|(index, _)| {
            registered_function.blocks[*index]
                .param_names()
                .any(|name| name.starts_with("_dp_try_exc_"))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    assert!(
        !handler_entry_targets.is_empty(),
        "expected at least one except handler block with an explicit try-exception carrier: {:?}",
        plan_runtime_param_names
    );
    assert!(
        plan_exc_dispatches
            .iter()
            .filter_map(|dispatch| dispatch.as_ref())
            .any(|dispatch| {
                handler_entry_targets.contains(&dispatch.target_index)
                    && (plan_runtime_param_names[dispatch.target_index]
                        .iter()
                        .any(|name| name.starts_with("_dp_try_exc_"))
                        || dispatch.slot_writes.iter().any(|(_, source)| {
                            matches!(source, soac_blockpy::block_py::BlockArg::CurrentException)
                        }))
            }),
        "expected a dispatch into an except handler target to pass the active exception: {:?}",
        plan_exc_dispatches
            .iter()
            .enumerate()
            .filter_map(|(index, dispatch)| {
                dispatch.as_ref().map(|dispatch| {
                    (
                        registered_function.blocks[index].label.to_string(),
                        registered_function.blocks[dispatch.target_index]
                            .label
                            .to_string(),
                        &dispatch.slot_writes,
                    )
                })
            })
            .collect::<Vec<_>>()
    );

    let storage_layout = gen_function
        .storage_layout()
        .as_ref()
        .expect("hidden resume should preserve closure layout");
    assert!(
        storage_layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == "exc"),
        "expected hidden resume closure layout to preserve the user-visible exception binding as a freevar cell: {:?}",
        storage_layout
    );
    assert!(
        storage_layout
            .freevars
            .iter()
            .any(|slot| slot.logical_name == "exc" && slot.storage_name.contains("exc")),
        "expected hidden resume closure slot for exc to keep a stable cell storage name: {:?}",
        storage_layout
    );
}
