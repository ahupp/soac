use super::{
    callable_scope_info, try_lower_function_to_core_blockpy_bundle, BlockPyModuleRewriter,
    FunctionScopeFrame,
};
use crate::block_py::{
    compute_make_function_capture_bindings_from_scope, BindingKind, BindingPurpose, BindingTarget,
    BlockPyModule, ClassBodyFallback, EffectiveBinding, ModuleNameGen,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::semantic::SemanticAstState;
use crate::passes::ruff_to_blockpy::rewrite_ast_to_core_blockpy_module_with_module;
use crate::passes::CoreBlockPyPassWithAwaitAndYield;
use ruff_python_ast::Stmt;
use ruff_python_parser::parse_module;

fn tracked_core_blockpy_with_await_and_yield(
    source: &str,
) -> BlockPyModule<CoreBlockPyPassWithAwaitAndYield> {
    lower_python_to_blockpy_for_testing(source)
        .unwrap()
        .pass_tracker
        .pass_core_blockpy_with_await_and_yield()
        .expect("core_blockpy_with_await_and_yield pass should be tracked")
        .clone()
}

fn lower_test_module_plan(
    context: &Context,
    mut module: Vec<Stmt>,
) -> BlockPyModule<CoreBlockPyPassWithAwaitAndYield> {
    crate::passes::ast_to_ast::simplify::flatten(&mut module);
    let mut semantic_state = SemanticAstState::from_ruff(&mut module);
    if !module.iter().any(
            |stmt| matches!(stmt, Stmt::FunctionDef(func) if func.name.id.as_str() == "_dp_module_init"),
        ) {
            crate::driver::wrap_module_init(&mut semantic_state, &mut module);
        }
    rewrite_ast_to_core_blockpy_module_with_module(
        context,
        module,
        &semantic_state,
        ModuleNameGen::new(0),
    )
}

#[test]
fn callable_semantic_info_uses_logical_storage_for_cell_captures() {
    let source = concat!(
        "def outer():\n",
        "    x = 1\n",
        "    def inner():\n",
        "        return x\n",
        "    return inner\n",
    );
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let inner = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "inner")
        .expect("missing inner callable");

    assert_eq!(
        inner.scope.binding_kind("x"),
        Some(BindingKind::Cell(crate::block_py::CellBindingKind::Capture))
    );
    assert_eq!(inner.scope.cell_storage_name("x"), "x");
    assert_eq!(inner.scope.cell_capture_source_name("x"), "_dp_cell_x");
    assert_eq!(
        inner.scope.captured_cell_bindings(),
        vec![crate::block_py::CellCaptureBinding {
            logical_name: "x".to_string(),
            source_name: "_dp_cell_x".to_string(),
        }]
    );
    assert_eq!(
        inner
            .scope
            .logical_name_for_cell_capture_source("_dp_cell_x"),
        Some("x".to_string())
    );
}

#[test]
fn callable_semantic_info_maps_classcell_capture_source_back_to_dunder_class() {
    let source = concat!(
        "class C:\n",
        "    def f(self):\n",
        "        def g():\n",
        "            return __class__\n",
        "        return g\n",
    );
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let f = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "f")
        .expect("missing method callable");

    assert_eq!(
        f.scope.binding_kind("__class__"),
        Some(BindingKind::Cell(crate::block_py::CellBindingKind::Capture))
    );
    assert_eq!(f.scope.cell_storage_name("__class__"), "__class__");
    assert_eq!(f.scope.cell_capture_source_name("__class__"), "__class__");
    assert_eq!(
        f.scope.captured_cell_bindings(),
        vec![crate::block_py::CellCaptureBinding {
            logical_name: "__class__".to_string(),
            source_name: "__class__".to_string(),
        }]
    );
    assert_eq!(
        f.scope.logical_name_for_cell_capture_source("__class__"),
        Some("__class__".to_string())
    );
}

#[test]
fn recursive_local_function_bindings_are_cell_owned_in_parent_scope() {
    let source = concat!(
        "def outer():\n",
        "    def recurse():\n",
        "        return recurse()\n",
        "    return recurse\n",
    );
    let context = Context::new(source);
    let mut module = parse_module(source).unwrap().into_syntax().body;
    let semantic_state = SemanticAstState::from_ruff(&mut module);
    let Stmt::FunctionDef(outer) = &mut module[0] else {
        panic!("expected outer function");
    };
    let outer_scope = semantic_state
        .function_scope(outer)
        .expect("missing outer scope");
    let mut rewriter = BlockPyModuleRewriter {
        context: &context,
        semantic_state: semantic_state.clone(),
        module_name_gen: ModuleNameGen::new(0),
        function_scope_stack: vec![FunctionScopeFrame {
            scope: Some(outer_scope.clone()),
            callable_scope: callable_scope_info(
                &semantic_state,
                None,
                Some(&outer_scope),
                Some(outer),
                &outer.body,
            ),
            hoisted_to_parent: Vec::new(),
        }],
        callable_defs: Vec::new(),
        lower_function_to_blockpy: try_lower_function_to_core_blockpy_bundle,
    };
    let nested_stmt = &mut outer
        .body
        .iter_mut()
        .find(|stmt| matches!(stmt, Stmt::FunctionDef(_)))
        .expect("missing nested function");
    let Stmt::FunctionDef(nested_func) = nested_stmt else {
        panic!("expected nested function def");
    };
    let nested_state = rewriter.walk_function_def_with_scope(nested_func);
    assert_eq!(
        rewriter
            .function_scope_stack
            .last()
            .expect("missing outer function frame")
            .callable_scope
            .binding_kind("recurse"),
        Some(crate::block_py::BindingKind::Cell(
            crate::block_py::CellBindingKind::Owner
        ))
    );
    let replacement = rewriter.rewrite_visited_function_def(nested_func, nested_state);
    let rendered = replacement
        .iter()
        .map(crate::ruff_ast_to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered.contains("__dp_store_cell(_dp_cell_recurse, recurse)"),
        "{rendered}"
    );
}

#[test]
fn callable_semantic_info_tracks_bind_and_qualname_for_class_helper_override() {
    let source = "class Box:\n    value = 1\n";
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let class_helper = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "_dp_class_ns_Box")
        .expect("missing class helper");
    assert_eq!(class_helper.scope.names.bind_name, "_dp_class_ns_Box");
    assert_eq!(class_helper.scope.names.display_name, "_dp_class_ns_Box");
    assert_eq!(class_helper.scope.names.qualname, "_dp_class_ns_Box");
}

#[test]
fn callable_semantic_info_marks_class_helper_as_owning_classcell() {
    let source = "class Box:\n    pass\n";
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let class_helper = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "_dp_class_ns_Box")
        .expect("missing class helper");

    assert_eq!(
        class_helper.scope.binding_kind("__class__"),
        Some(BindingKind::Cell(crate::block_py::CellBindingKind::Owner))
    );
    assert!(class_helper.scope.has_local_def("__class__"));
    assert_eq!(
        class_helper.scope.cell_storage_name("__class__"),
        "_dp_classcell"
    );
}

#[test]
fn callable_semantic_info_does_not_leak_dunder_class_capture_to_outer_function() {
    let source = concat!(
        "def exercise():\n",
        "    class X:\n",
        "        global __class__\n",
        "        __class__ = 42\n",
        "        def f(self):\n",
        "            return __class__\n",
        "    return X\n",
    );
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let exercise = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "exercise")
        .expect("missing outer function");

    assert_eq!(exercise.scope.binding_kind("__class__"), None);
    assert_eq!(exercise.scope.captured_cell_bindings(), Vec::new());
}

#[test]
fn class_helper_semantic_info_stays_lexical_when_nested_methods_capture_outer_closure() {
    let source = concat!(
        "def run():\n",
        "    log = []\n",
        "    class C:\n",
        "        def f(self):\n",
        "            log.append('x')\n",
        "            return log\n",
        "    return C\n",
    );
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let class_helper = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "_dp_class_ns_C")
        .expect("missing class helper");

    assert_eq!(class_helper.scope.captured_cell_bindings(), Vec::new());
    assert_eq!(
        compute_make_function_capture_bindings_from_scope(class_helper),
        Vec::new()
    );
}

#[test]
fn callable_semantic_info_distinguishes_class_type_params_from_class_body_locals() {
    let source = "class Box[T]:\n    value = T\n";
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let class_helper = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "_dp_class_ns_Box")
        .expect("missing class helper");

    assert!(class_helper.scope.type_param_names.contains("T"));
    assert_eq!(
        class_helper
            .scope
            .effective_binding("T", BindingPurpose::Store),
        Some(EffectiveBinding::Local),
    );
    assert_eq!(
        class_helper
            .scope
            .binding_target_for_name("T", BindingPurpose::Store),
        BindingTarget::Local,
    );
    assert_eq!(
        class_helper
            .scope
            .effective_binding("T", BindingPurpose::Load),
        Some(EffectiveBinding::ClassBody(ClassBodyFallback::Global)),
    );
    assert_eq!(
        class_helper
            .scope
            .binding_target_for_name("value", BindingPurpose::Store),
        BindingTarget::ClassNamespace,
    );
    assert_eq!(
        class_helper
            .scope
            .effective_binding("value", BindingPurpose::Store),
        Some(EffectiveBinding::ClassBody(ClassBodyFallback::Global))
    );
}

#[test]
fn callable_semantic_info_keeps_class_attrs_out_of_cell_bindings() {
    let source = concat!(
        "def outer():\n",
        "    x = \"outer\"\n",
        "    class Inner:\n",
        "        x = \"class\"\n",
        "        def read():\n",
        "            return x\n",
        "    return Inner\n",
    );
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let class_helper = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "_dp_class_ns_Inner")
        .expect("missing class helper");

    assert_eq!(
        class_helper.scope.binding_kind("x"),
        Some(BindingKind::Local),
    );
    assert_eq!(
        class_helper
            .scope
            .binding_target_for_name("x", BindingPurpose::Store),
        BindingTarget::ClassNamespace,
    );
}

#[test]
fn callable_semantic_info_records_class_cell_fallback_for_outer_reads() {
    let source = concat!(
        "def outer():\n",
        "    x = 1\n",
        "    class Box:\n",
        "        value = x\n",
        "    return Box\n",
    );
    let blockpy_module = tracked_core_blockpy_with_await_and_yield(source);
    let class_helper = blockpy_module
        .callable_defs
        .iter()
        .find(|func| func.names.bind_name == "_dp_class_ns_Box")
        .expect("missing class helper");

    assert_eq!(
        class_helper
            .scope
            .effective_binding("x", BindingPurpose::Load),
        Some(EffectiveBinding::ClassBody(ClassBodyFallback::Cell))
    );
}

#[test]
fn callable_semantic_info_resolves_implicit_global_loads_in_body() {
    let source = concat!(
        "def outer(scale):\n",
        "    factor = scale\n",
        "    def inner(x):\n",
        "        try:\n",
        "            return x + factor\n",
        "        except Exception as exc:\n",
        "            return len(str(exc))\n",
        "    return inner\n",
    );
    let mut module = parse_module(source).unwrap().into_syntax().body;
    let semantic_state = SemanticAstState::from_ruff(&mut module);
    let Stmt::FunctionDef(outer) = &module[0] else {
        panic!("expected outer function");
    };
    let inner = &outer
        .body
        .iter()
        .find_map(|stmt| match stmt {
            Stmt::FunctionDef(func) if func.name.id.as_str() == "inner" => Some(func),
            _ => None,
        })
        .expect("missing inner");
    let inner_scope = semantic_state
        .function_scope(inner)
        .expect("missing inner scope");
    let outer_scope = semantic_state
        .function_scope(outer)
        .expect("missing outer scope");
    let scope = callable_scope_info(
        &semantic_state,
        Some(&outer_scope),
        Some(&inner_scope),
        Some(inner),
        &inner.body,
    );

    assert_eq!(
        scope.binding_kind("factor"),
        Some(crate::block_py::BindingKind::Cell(
            crate::block_py::CellBindingKind::Capture
        ))
    );
    assert_eq!(
        scope.binding_kind("x"),
        Some(crate::block_py::BindingKind::Local)
    );
    assert_eq!(
        scope.binding_kind("Exception"),
        Some(crate::block_py::BindingKind::Global)
    );
    assert_eq!(
        scope.binding_kind("len"),
        Some(crate::block_py::BindingKind::Global)
    );
    assert_eq!(
        scope.binding_kind("str"),
        Some(crate::block_py::BindingKind::Global)
    );
}

#[test]
fn lowering_recursive_local_function_with_finally_keeps_plain_binding_before_name_binding() {
    let source = concat!(
        "import sys\n",
        "def exercise():\n",
        "    original_limit = sys.getrecursionlimit()\n",
        "    sys.setrecursionlimit(50)\n",
        "    def recurse():\n",
        "        return recurse()\n",
        "    try:\n",
        "        try:\n",
        "            recurse()\n",
        "        except RecursionError:\n",
        "            return True\n",
        "        return False\n",
        "    finally:\n",
        "        sys.setrecursionlimit(original_limit)\n",
    );
    let context = Context::new(source);
    let module = parse_module(source).unwrap().into_syntax().body;
    let blockpy = lower_test_module_plan(&context, module);
    let exercise = blockpy
        .callable_defs
        .iter()
        .find(|callable| callable.names.bind_name == "exercise")
        .expect("missing lowered exercise callable");
    let rendered =
        crate::block_py::pretty::blockpy_module_to_string(&crate::block_py::BlockPyModule {
            module_name_gen: crate::block_py::ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![exercise.clone()],
            module_constants: Vec::new(),
            counter_defs: Vec::new(),
        });
    assert!(
        rendered.contains("StoreName(\"recurse\", MakeFunction"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("__dp_store_cell(_dp_cell_recurse, recurse)"),
        "{rendered}"
    );
}

#[test]
fn lowering_recursive_local_function_treats_recurse_cell_as_local_state() {
    let source = concat!(
        "import sys\n",
        "def exercise():\n",
        "    original_limit = sys.getrecursionlimit()\n",
        "    sys.setrecursionlimit(50)\n",
        "    def recurse():\n",
        "        return recurse()\n",
        "    try:\n",
        "        try:\n",
        "            recurse()\n",
        "        except RecursionError:\n",
        "            return True\n",
        "        return False\n",
        "    finally:\n",
        "        sys.setrecursionlimit(original_limit)\n",
    );
    let context = Context::new(source);
    let module = parse_module(source).unwrap().into_syntax().body;
    let blockpy = lower_test_module_plan(&context, module);
    let exercise = blockpy
        .callable_defs
        .iter()
        .find(|callable| callable.names.bind_name == "exercise")
        .expect("missing lowered exercise callable");
    assert!(
        exercise.scope.binding_kind("recurse")
            == Some(crate::block_py::BindingKind::Cell(
                crate::block_py::CellBindingKind::Owner
            )),
        "semantic_bindings={:?}",
        exercise.scope.bindings,
    );
}

#[test]
fn lowering_recursive_local_function_finally_return_preserves_liveins() {
    let source = concat!(
        "import sys\n",
        "def exercise():\n",
        "    original_limit = sys.getrecursionlimit()\n",
        "    sys.setrecursionlimit(50)\n",
        "    def recurse():\n",
        "        return recurse()\n",
        "    try:\n",
        "        try:\n",
        "            recurse()\n",
        "        except RecursionError:\n",
        "            return True\n",
        "        return False\n",
        "    finally:\n",
        "        sys.setrecursionlimit(original_limit)\n",
    );
    let context = Context::new(source);
    let module = parse_module(source).unwrap().into_syntax().body;
    let blockpy = lower_test_module_plan(&context, module);
    let exercise = blockpy
        .callable_defs
        .iter()
        .find(|callable| callable.names.bind_name == "exercise")
        .expect("missing lowered exercise callable");
    let rendered =
        crate::block_py::pretty::blockpy_module_to_string(&crate::block_py::BlockPyModule {
            module_name_gen: crate::block_py::ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![exercise.clone()],
            module_constants: Vec::new(),
            counter_defs: Vec::new(),
        });
    assert!(
        rendered.contains("jump ")
            && rendered.contains("(AbruptKind(Return), Name(\"_dp_try_abrupt_payload_"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("(None, AbruptKind(Return), Name(\"_dp_try_abrupt_payload_"),
        "{rendered}"
    );
}

#[test]
fn lowering_nonlocal_inner_captures_outer_cell() {
    let source = concat!(
        "def outer():\n",
        "    x = 5\n",
        "    def inner():\n",
        "        nonlocal x\n",
        "        x = 2\n",
        "        return x\n",
        "    return inner()\n",
    );
    let context = Context::new(source);
    let module = parse_module(source).unwrap().into_syntax().body;
    let blockpy = lower_test_module_plan(&context, module);
    let inner = blockpy
        .callable_defs
        .iter()
        .find(|callable| callable.names.bind_name == "inner")
        .expect("missing lowered inner callable");
    let outer = blockpy
        .callable_defs
        .iter()
        .find(|callable| callable.names.bind_name == "outer")
        .expect("missing lowered outer callable");
    assert!(
        inner.storage_layout().is_none(),
        "{:?}",
        inner.storage_layout()
    );
    let rendered =
        crate::block_py::pretty::blockpy_module_to_string(&crate::block_py::BlockPyModule {
            module_name_gen: crate::block_py::ModuleNameGen::new(0),
            global_names: Vec::new(),
            callable_defs: vec![outer.clone()],
            module_constants: Vec::new(),
            counter_defs: Vec::new(),
        });
    assert!(
        rendered
            .contains("StoreName(\"inner\", MakeFunction(0:1, Function, tuple_values(), NONE))"),
        "{rendered}"
    );
}
