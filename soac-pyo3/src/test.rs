use pyo3::prelude::*;
use pyo3::types::PyModule;
use soac_blockpy::block_py::FunctionKind;
use soac_blockpy::passes::infer_module_value_facts;
use soac_jit::{
    exc_dispatch_plan,
    module_type::{hash_module_source, indexed_module_info},
    plan_function_locals, plan_function_refcount_ownership, planned_jit_params_for_function,
};
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
    bb_module: &soac_blockpy::block_py::BlockPyModule<soac_blockpy::passes::CodegenModuleShape>,
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

fn python_runtime_test_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn initialize_test_python() {
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace crate should have a repo-root parent");
    let python_home = repo_root.join("vendor").join("cpython");
    unsafe {
        std::env::set_var("PYTHONHOME", &python_home);
    }
    let rel_build_dir = std::fs::read_to_string(python_home.join("pybuilddir.txt"))
        .expect("vendored CPython pybuilddir.txt should exist");
    let python_path = std::env::join_paths([
        python_home.join("Lib"),
        python_home.join(rel_build_dir.trim()),
        repo_root.join("soac_py").join("src"),
    ])
    .expect("test PYTHONPATH should join");
    unsafe {
        std::env::set_var("PYTHONPATH", python_path);
    }
    Python::initialize();
}

unsafe fn class_dict_function(
    owner_type: *mut pyo3::ffi::PyTypeObject,
    name: &'static std::ffi::CStr,
) -> *mut pyo3::ffi::PyObject {
    let dict = (*owner_type).tp_dict;
    assert!(!dict.is_null(), "owner type should have a tp_dict");
    let function = pyo3::ffi::PyDict_GetItemString(dict, name.as_ptr());
    assert!(
        !function.is_null(),
        "class dict should contain requested function"
    );
    pyo3::ffi::Py_INCREF(function);
    function
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
fn transformed_module_methods_register_owner_types_for_lookup() {
    let _guard = python_runtime_test_lock().lock().unwrap();
    initialize_test_python();
    Python::attach(|py| unsafe {
        let ext = PyModule::new(py, "_soac_ext").expect("extension module should allocate");
        crate::_soac_ext(py, &ext).expect("extension init should succeed");
        let sys = py.import("sys").expect("sys should import");
        let modules = sys.getattr("modules").expect("sys.modules should exist");
        modules
            .set_item("_soac_ext", &ext)
            .expect("sys.modules should accept _soac_ext");
        let importlib = py
            .import("importlib.machinery")
            .expect("importlib.machinery should import");
        let module_spec = importlib
            .getattr("ModuleSpec")
            .expect("ModuleSpec should exist");
        let spec = module_spec
            .call1(("transformed_owner_lookup_test", py.None()))
            .expect("ModuleSpec should instantiate");
        let source = "class C:\n    def f(self):\n        return 1\n";
        let source_path = std::env::temp_dir().join(format!(
            "soac_create_module_test_{}_{}.py",
            std::process::id(),
            "owner_lookup"
        ));
        std::fs::write(&source_path, source).expect("test source file should be writable");
        let module = ext
            .getattr("create_module")
            .expect("create_module should be exported")
            .call1((
                source_path
                    .to_str()
                    .expect("test source path should be utf-8"),
                &spec,
            ))
            .expect("transformed module creation should succeed");
        let module_info = indexed_module_info(module.as_any())
            .expect("created module should expose SOAC module info");
        assert_eq!(
            module_info.hash,
            hash_module_source(source),
            "IndexedModuleType tail should store the source hash"
        );
        assert!(
            module_info.indexed_module_keys.iter().any(|key| key == "C"),
            "IndexedModuleType tail should preserve Rust-owned module-key metadata: {module_info:?}"
        );
        module
            .getattr("__dict__")
            .expect("module should expose __dict__")
            .set_item("__package__", "")
            .expect("module globals should accept __package__");
        ext.getattr("exec_module")
            .expect("exec_module should be exported")
            .call1((&module,))
            .expect("transformed module execution should succeed");

        let cls = module
            .getattr("C")
            .expect("executed module should define C");
        let owner_type = cls.as_ptr() as *mut pyo3::ffi::PyTypeObject;
        let function = class_dict_function(owner_type, c"f");
        let function_id = soac_jit::registered_clif_function_id(function)
            .expect("registered function id lookup should succeed")
            .expect("transformed method should carry a FunctionId");
        let owners = soac_jit::lookup_exact_owner_types_for_method(function_id, "f")
            .expect("exact owner lookup should succeed");
        assert_eq!(
            owners.len(),
            1,
            "expected one owner type for transformed C.f"
        );
        assert_eq!(owners[0].owner_type, owner_type);
        assert_eq!(owners[0].function_obj, function);
        pyo3::ffi::Py_DECREF(function);
    });
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
    let gen_function = normalized
        .callable_defs
        .iter()
        .find(|function| function.names.bind_name == "gen_resume")
        .expect("missing lowered generator resume function");
    let registered_function = gen_function;
    let value_facts = infer_module_value_facts(&normalized);
    let local_plan = plan_function_locals(registered_function, &value_facts);
    let refcount_plan =
        plan_function_refcount_ownership(&normalized, registered_function, &value_facts)
            .expect("refcount plan should validate");
    let plan_runtime_params = planned_jit_params_for_function(registered_function, &local_plan)
        .expect("runtime params should bind");
    let plan_exc_dispatches = registered_function
        .blocks
        .iter()
        .map(|block| {
            let runtime_target_params = block
                .exc_edge
                .as_ref()
                .map(|edge| plan_runtime_params[edge.target.index()].as_slice())
                .unwrap_or(&[]);
            exc_dispatch_plan(
                registered_function,
                block,
                runtime_target_params,
                &refcount_plan,
            )
        })
        .collect::<Vec<_>>();

    let handler_entry_targets = plan_runtime_params
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
        plan_runtime_params
    );
    assert!(
        plan_exc_dispatches
            .iter()
            .filter_map(|dispatch| dispatch.as_ref())
            .any(|dispatch| {
                handler_entry_targets.contains(&dispatch.target_index)
                    && (plan_runtime_params[dispatch.target_index]
                        .iter()
                        .any(|param| param.arg_name.starts_with("_dp_try_exc_"))
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
