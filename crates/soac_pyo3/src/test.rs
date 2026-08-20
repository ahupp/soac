use pyo3::prelude::*;
use pyo3::types::PyModule;
use soac_config::SoacEnvConfig;
use soac_core::block_py::FunctionKind;
use soac_jit::plan_jit_typed_module;
use std::any::Any;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

fn panic_payload_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

fn parse_and_lower(source: &str) -> Result<soac_lowering::LoweringResult, String> {
    match std::panic::catch_unwind(|| soac_lowering::lower_python_to_blockpy_for_testing(source)) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(panic_payload_to_string(payload)),
    }
}

fn parse_and_lower_runtime_style(source: &str) -> Result<soac_lowering::LoweringResult, String> {
    match std::panic::catch_unwind(|| soac_lowering::lower_python_to_blockpy_for_testing(source)) {
        Ok(Ok(result)) => Ok(result),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(panic_payload_to_string(payload)),
    }
}

fn validate_bb_module_for_jit(
    bb_module: &soac_core::block_py::BlockPyModule<soac_ir_blockpy::BlockPyModuleShape>,
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

fn run_cranelift_jit_preflight(result: &soac_lowering::LoweringResult) -> Result<(), String> {
    soac_jit::run_cranelift_smoke(&result.blockpy_module)
}

fn python_runtime_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn initialize_test_python() {
    soac_cpython::initialize_test_python("soac_pyo3-test").expect("test Python should initialize");
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
    let normalized = result.blockpy_module.clone();
    let inner_function = normalized
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "inner")
        .expect("missing lowered inner function");
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
    let bb_module = &result.blockpy_module;
    validate_bb_module_for_jit(bb_module).expect("validator should accept lowered class defs");
}

#[test]
fn jit_validator_accepts_coroutines() {
    let source = r#"
async def run():
    return 1
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let bb_module = &result.blockpy_module;
    validate_bb_module_for_jit(bb_module).expect("validator should accept coroutine lowering");
}

#[test]
fn jit_validator_accepts_async_generators() {
    let source = r#"
async def run():
    yield 1
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let bb_module = &result.blockpy_module;
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
    let bb_module = &result.blockpy_module;
    validate_bb_module_for_jit(bb_module).expect("validator should accept lowered try blocks");
}

#[test]
fn jit_preflight_runs_cranelift_for_supported_module() {
    let source = r#"
def f(x):
    return x
    "#;
    let result = parse_and_lower(source).expect("lowering should succeed");
    let bb_module = &result.blockpy_module;
    validate_bb_module_for_jit(bb_module).expect("validator should allow module");
    run_cranelift_jit_preflight(&result).expect("cranelift preflight should run");
}

#[test]
fn unselected_module_delegates_without_registering_method_owners() {
    let _guard = python_runtime_test_lock().lock().unwrap();
    initialize_test_python();
    Python::attach(|py| {
        let ext = PyModule::new(py, "_soac_ext").expect("extension module should allocate");
        let sys = py.import("sys").expect("sys should import");
        let modules = sys.getattr("modules").expect("sys.modules should exist");
        modules
            .set_item("_soac_ext", &ext)
            .expect("sys.modules should accept _soac_ext");
        crate::_soac_ext(py, &ext).expect("extension init should succeed");
        let importlib = py
            .import("importlib.machinery")
            .expect("importlib.machinery should import");
        let module_spec = importlib
            .getattr("ModuleSpec")
            .expect("ModuleSpec should exist");
        let source = "class C:\n    def __init__(self):\n        self.value = 1\n    def f(self):\n        return 1\n";
        let source_path = std::env::temp_dir().join(format!(
            "soac_create_module_test_{}_{}.py",
            std::process::id(),
            "owner_lookup"
        ));
        std::fs::write(&source_path, source).expect("test source file should be writable");
        let path = source_path
            .to_str()
            .expect("test source path should be utf-8");
        let original_loader = importlib
            .getattr("SourceFileLoader")
            .unwrap()
            .call1(("transformed_owner_lookup_test", path))
            .expect("ordinary source loader should instantiate");
        let spec = module_spec
            .call1(("transformed_owner_lookup_test", &original_loader))
            .expect("ModuleSpec should instantiate");
        let creation = ext
            .getattr("create_module")
            .expect("create_module should be exported")
            .call1((path, &spec))
            .expect("unselected creation should delegate");
        // A source file and an enabled extension are not strict authority.
        // PEP451 leaves ordinary creation and execution with the real loader.
        assert!(creation.is_none());
        let module = py
            .import("importlib.util")
            .unwrap()
            .call_method1("module_from_spec", (&spec,))
            .expect("ordinary module creation should succeed");
        assert!(
            module.get_type().is(&py.get_type::<PyModule>()),
            "unselected modules use the ordinary native module type"
        );
        assert!(
            !ext.getattr("exec_module")
                .unwrap()
                .call1((&module,))
                .unwrap()
                .extract::<bool>()
                .unwrap()
        );
        assert!(!module.hasattr("C").unwrap());
        original_loader
            .call_method1("exec_module", (&module,))
            .expect("the original loader should execute the unchanged source");
        assert!(
            ext.getattr("strict_module_diagnostics")
                .unwrap()
                .call1((&module,))
                .unwrap()
                .is_none()
        );
        let cls = module.getattr("C").unwrap();
        for name in ["__init__", "f"] {
            let function = cls.getattr(name).unwrap();
            assert!(
                unsafe { soac_jit::registered_clif_function_id(function.as_ptr()) }
                    .unwrap()
                    .is_none()
            );
            assert!(
                ext.getattr("strict_function_diagnostics")
                    .unwrap()
                    .call1((&function,))
                    .unwrap()
                    .is_none()
            );
        }
        let instance = cls.call0().unwrap();
        assert_eq!(
            instance.getattr("value").unwrap().extract::<i32>().unwrap(),
            1
        );
        assert_eq!(
            instance
                .call_method0("f")
                .unwrap()
                .extract::<i32>()
                .unwrap(),
            1
        );
        instance.setattr("value", "ordinary mutation").unwrap();
        assert_eq!(
            instance
                .getattr("value")
                .unwrap()
                .extract::<String>()
                .unwrap(),
            "ordinary mutation"
        );
        std::fs::remove_file(&source_path).expect("remove this test's source fixture");
    });
}

#[test]
fn generator_throw_handler_plan_keeps_try_exception_state_and_local_exc_binding() {
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
    let normalized = result.blockpy_module.clone();
    let gen_function = normalized
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "gen")
        .expect("missing lowered generator function");
    let registered_function = gen_function;
    let prepared = soac_driver::typed_runtime::prepare_typed_v3_runtime_module(
        &normalized,
        &SoacEnvConfig::default(),
    )
    .expect("typed runtime preparation should validate");
    let jit_module_local_plan = plan_jit_typed_module(prepared.module, prepared.value_facts)
        .expect("JIT local plan should validate")
        .locals;
    let jit_local_plan = jit_module_local_plan
        .function(registered_function.function_id)
        .expect("generator resume function should have a JIT local plan");

    let handler_entry_targets = jit_local_plan
        .runtime_block_params
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
        jit_local_plan.runtime_block_params
    );
    assert!(
        jit_local_plan
            .exc_dispatches
            .iter()
            .filter_map(|dispatch| dispatch.as_ref())
            .any(|dispatch| {
                handler_entry_targets.contains(&dispatch.target_index)
                    && (jit_local_plan.runtime_block_params[dispatch.target_index]
                        .iter()
                        .any(|param| param.arg_name.starts_with("_dp_try_exc_"))
                        || dispatch.slot_writes.iter().any(|(_, source)| {
                            matches!(source, soac_core::block_py::BlockArg::CurrentException)
                        }))
            }),
        "expected a dispatch into an except handler target to pass the active exception: {:?}",
        jit_local_plan
            .exc_dispatches
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
            .preserved_slots
            .iter()
            .any(|slot| slot.logical_name == "exc"),
        "expected hidden resume layout to preserve the user-visible exception binding as activation state: {:?}",
        storage_layout
    );
    assert!(
        storage_layout
            .freevars
            .iter()
            .all(|slot| slot.logical_name != "exc"),
        "expected hidden resume layout to keep user-visible exception bindings out of closure state: {:?}",
        storage_layout
    );
}
