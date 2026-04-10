use super::{
    SemanticAstState, SemanticBindingKind, SemanticBindingUse, SemanticScope, SemanticScopeKind,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::rewrite_class_def::class_body::rewrite_class_body_scopes;
use ruff_python_ast::{self as ast, Stmt};
use ruff_python_parser::parse_module;

fn parse_module_body(source: &str) -> Vec<Stmt> {
    parse_module(source).unwrap().into_syntax().body
}

fn find_function<'a>(body: &'a [Stmt], name: &str) -> &'a ast::StmtFunctionDef {
    for stmt in body {
        if let Stmt::FunctionDef(func_def) = stmt {
            if func_def.name.id.as_str() == name {
                return func_def;
            }
        }
    }
    panic!("function {name} not found");
}

fn find_class<'a>(body: &'a [Stmt], name: &str) -> &'a ast::StmtClassDef {
    for stmt in body {
        if let Stmt::ClassDef(class_def) = stmt {
            if class_def.name.id.as_str() == name {
                return class_def;
            }
        }
    }
    panic!("class {name} not found");
}

fn find_class_recursive<'a>(body: &'a [Stmt], name: &str) -> Option<&'a ast::StmtClassDef> {
    for stmt in body {
        match stmt {
            Stmt::ClassDef(class_def) if class_def.name.id.as_str() == name => {
                return Some(class_def);
            }
            Stmt::If(if_stmt) => {
                if let Some(found) = find_class_recursive(&if_stmt.body, name) {
                    return Some(found);
                }
                for clause in &if_stmt.elif_else_clauses {
                    if let Some(found) = find_class_recursive(&clause.body, name) {
                        return Some(found);
                    }
                }
            }
            Stmt::For(for_stmt) => {
                if let Some(found) = find_class_recursive(&for_stmt.body, name) {
                    return Some(found);
                }
                if let Some(found) = find_class_recursive(&for_stmt.orelse, name) {
                    return Some(found);
                }
            }
            Stmt::While(while_stmt) => {
                if let Some(found) = find_class_recursive(&while_stmt.body, name) {
                    return Some(found);
                }
                if let Some(found) = find_class_recursive(&while_stmt.orelse, name) {
                    return Some(found);
                }
            }
            _ => {}
        }
    }
    None
}

fn function_scope<'a>(
    state: &'a SemanticAstState,
    func_def: &ast::StmtFunctionDef,
) -> SemanticScope {
    state
        .function_scope(func_def)
        .expect("missing function scope")
}

#[test]
fn semantic_state_keeps_class_helper_scope_overrides_transformable() {
    let source = concat!(
        "def outer():\n",
        "    shared = 1\n",
        "    class Box:\n",
        "        probe = shared\n",
        "        def get(self):\n",
        "            return shared\n",
        "    return Box\n",
    );
    let context = Context::new(source);
    let mut module = parse_module(source).unwrap().into_syntax().body;
    let mut semantic_state = SemanticAstState::from_ruff(&mut module);
    rewrite_class_body_scopes(&context, &mut semantic_state, &mut module);
}

#[test]
fn semantic_state_module_bindings_include_assignments() {
    let mut body = parse_module_body("x = 1\ny = 2\n");
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let scope = semantic_state.module_scope();
    assert_eq!(
        scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
    assert_eq!(
        scope.binding_in_scope("y", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
}

#[test]
fn synthesized_module_init_scope_reuses_module_children_and_translates_bindings() {
    let mut body = parse_module_body(concat!(
        "x = 1\n",
        "def f():\n",
        "    return x\n",
        "class C:\n",
        "    y = x\n",
    ));
    let mut semantic_state = SemanticAstState::from_ruff(&mut body);
    let module_init: ast::StmtFunctionDef = crate::py_stmt_typed!(
        r#"
def _dp_module_init():
    pass
"#
    );
    let module_init_scope = semantic_state.synthesize_module_init_scope(&module_init);

    assert_eq!(
        module_init_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Global
    );
    assert_eq!(
        module_init_scope.binding_in_scope("f", SemanticBindingUse::Load),
        SemanticBindingKind::Global
    );
    assert!(module_init_scope
        .child_scope_for_function(find_function(&body, "f"))
        .is_some());
    assert!(module_init_scope
        .child_scope_for_class(find_class(&body, "C"))
        .is_some());
}

#[test]
fn semantic_state_function_scope_tracks_parameters_and_globals() {
    let mut body = parse_module_body(concat!(
        "x = 0\n",
        "def f(a, b, *args, c=1, **kwargs):\n",
        "    global x\n",
        "    x = a\n",
        "    y = b\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let func_scope = function_scope(&semantic_state, find_function(&body, "f"));

    for name in ["a", "b", "args", "c", "kwargs", "y"] {
        assert_eq!(
            func_scope.binding_in_scope(name, SemanticBindingUse::Load),
            SemanticBindingKind::Local,
            "{name}"
        );
    }
    assert_eq!(
        func_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Global
    );
}

#[test]
fn semantic_state_preserves_lambda_scope_bindings() {
    let mut body = parse_module_body(concat!("def outer(x):\n", "    return lambda y: x + y\n",));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let outer = find_function(&body, "outer");
    let outer_scope = function_scope(&semantic_state, outer);
    let Stmt::Return(return_stmt) = &outer.body[0] else {
        panic!("expected return statement");
    };
    let Some(ast::Expr::Lambda(lambda)) = return_stmt.value.as_deref() else {
        panic!("expected lambda return value");
    };
    let lambda_scope = semantic_state
        .lambda_scope(lambda)
        .expect("missing preserved lambda scope");

    assert_eq!(lambda_scope.qualname(), "outer.<locals>.<lambda>");
    assert_eq!(
        lambda_scope.binding_in_scope("y", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
    assert_eq!(
        lambda_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert!(outer_scope.has_local_def("x"));
}

#[test]
fn semantic_state_named_expr_in_while_test_binds_local() {
    let mut body = parse_module_body(concat!(
        "def f(values):\n",
        "    while not (value := values[0]):\n",
        "        break\n",
        "    return value\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let func_scope = function_scope(&semantic_state, find_function(&body, "f"));

    assert_eq!(
        func_scope.binding_in_scope("value", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
    assert!(func_scope.local_binding_names().contains("value"));
}

#[test]
fn semantic_state_nested_global_function_def_qualifies_globally() {
    let mut body = parse_module_body(concat!(
        "def build_qualnames():\n",
        "    def global_function():\n",
        "        def inner_function():\n",
        "            global inner_global_function\n",
        "            def inner_global_function():\n",
        "                pass\n",
        "            return inner_global_function\n",
        "        return inner_function()\n",
        "    return global_function()\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let build_qualnames = find_function(&body, "build_qualnames");
    let global_function = find_function(&build_qualnames.body, "global_function");
    let inner_function = find_function(&global_function.body, "inner_function");
    let inner_scope = function_scope(&semantic_state, inner_function);
    let inner_global_function = find_function(&inner_function.body, "inner_global_function");
    let inner_global_scope = function_scope(&semantic_state, inner_global_function);

    assert_eq!(
        inner_scope.binding_in_scope("inner_global_function", SemanticBindingUse::Load),
        SemanticBindingKind::Global
    );
    assert_eq!(inner_global_scope.qualname(), "inner_global_function");
}

#[test]
fn semantic_state_nonlocal_in_child_scopes_is_detected() {
    let mut body = parse_module_body(concat!(
        "def outer():\n",
        "    x = 1\n",
        "    def inner():\n",
        "        nonlocal x\n",
        "        return x\n",
        "    return inner\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let outer_scope = function_scope(&semantic_state, find_function(&body, "outer"));
    let inner_def = find_function(&find_function(&body, "outer").body, "inner");
    let inner_scope = function_scope(&semantic_state, inner_def);

    assert_eq!(
        inner_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(
        outer_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(
        outer_scope.binding_in_scope("y", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
}

#[test]
fn semantic_state_implicit_nonlocal_reads_mark_root_binding() {
    let mut body = parse_module_body(concat!(
        "def outer():\n",
        "    x = 1\n",
        "    def inner():\n",
        "        return x\n",
        "    return inner\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let outer_scope = function_scope(&semantic_state, find_function(&body, "outer"));
    let inner_def = find_function(&find_function(&body, "outer").body, "inner");
    let inner_scope = function_scope(&semantic_state, inner_def);

    assert_eq!(
        inner_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(
        outer_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
}

#[test]
fn semantic_state_marks_method_dunder_class_as_nonlocal_cell_capture() {
    let mut body = parse_module_body(concat!(
        "class C:\n",
        "    def f(self):\n",
        "        return __class__\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let class_def = find_class(&body, "C");
    let method_def = find_function(&class_def.body, "f");
    let method_scope = function_scope(&semantic_state, method_def);

    assert_eq!(
        method_scope.binding_in_scope("__class__", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(
        method_scope.cell_storage_name("__class__").as_deref(),
        Some("_dp_classcell")
    );
}

#[test]
fn semantic_state_does_not_create_classcell_for_module_level_explicit_super() {
    let mut body = parse_module_body(concat!("def f(cls):\n", "    return super(Generic, cls)\n",));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let function_scope = function_scope(&semantic_state, find_function(&body, "f"));

    assert_eq!(function_scope.binding_in_current_scope("__class__"), None);
    assert_eq!(function_scope.cell_storage_name("__class__"), None);
}

#[test]
fn semantic_state_propagates_method_dunder_class_binding_to_nested_functions() {
    let mut body = parse_module_body(concat!(
        "class C:\n",
        "    def f(self):\n",
        "        def g():\n",
        "            return __class__\n",
        "        return g\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let class_def = find_class(&body, "C");
    let method_def = find_function(&class_def.body, "f");
    let method_scope = function_scope(&semantic_state, method_def);
    let nested_def = find_function(&method_def.body, "g");
    let nested_scope = function_scope(&semantic_state, nested_def);

    assert_eq!(
        method_scope.binding_in_scope("__class__", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(
        nested_scope.binding_in_scope("__class__", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(
        nested_scope.cell_storage_name("__class__").as_deref(),
        Some("_dp_classcell")
    );
}

#[test]
fn semantic_state_does_not_propagate_dunder_class_out_of_nested_class_scope() {
    let mut body = parse_module_body(concat!(
        "def exercise():\n",
        "    class X:\n",
        "        global __class__\n",
        "        __class__ = 42\n",
        "        def f(self):\n",
        "            return __class__\n",
        "    return X\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let exercise_def = find_function(&body, "exercise");
    let exercise_scope = function_scope(&semantic_state, exercise_def);

    assert_ne!(
        exercise_scope.binding_in_scope("__class__", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(exercise_scope.cell_storage_name("__class__"), None);
}

#[test]
fn semantic_state_recursive_local_function_is_tracked_as_cell_binding() {
    let mut body = parse_module_body(concat!(
        "def outer():\n",
        "    def recurse():\n",
        "        return recurse()\n",
        "    return recurse\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let outer_scope = function_scope(&semantic_state, find_function(&body, "outer"));

    assert!(outer_scope.local_cell_bindings().contains("recurse"));
}

#[test]
fn semantic_state_class_scope_has_local_bindings() {
    let mut body = parse_module_body(concat!(
        "class C:\n",
        "    y = 1\n",
        "    def m(self):\n",
        "        z = y\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let class_scope = semantic_state
        .module_scope()
        .child_scope_for_class(find_class(&body, "C"))
        .expect("missing class scope");

    assert_eq!(class_scope.kind(), SemanticScopeKind::Class);
    assert_eq!(
        class_scope.binding_in_scope("y", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
}

#[test]
fn semantic_state_class_type_params_are_local_bindings() {
    let mut body = parse_module_body(concat!(
        "class Box[T, **P]:\n",
        "    value = T\n",
        "    params = P\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let class_scope = semantic_state
        .module_scope()
        .child_scope_for_class(find_class(&body, "Box"))
        .expect("missing class scope");

    assert_eq!(
        class_scope.binding_in_scope("T", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
    assert_eq!(
        class_scope.binding_in_scope("P", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
    assert!(class_scope.type_param_names().contains("T"));
    assert!(class_scope.type_param_names().contains("P"));
}

#[test]
fn semantic_state_function_type_params_are_local_bindings() {
    let mut body = parse_module_body(concat!(
        "def f[T, **P](x: T, *args: P.args, **kwargs: P.kwargs) -> T:\n",
        "    return x\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let func_scope = function_scope(&semantic_state, find_function(&body, "f"));

    assert_eq!(
        func_scope.binding_in_scope("T", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
    assert_eq!(
        func_scope.binding_in_scope("P", SemanticBindingUse::Load),
        SemanticBindingKind::Local
    );
    assert!(func_scope.type_param_names().contains("T"));
    assert!(func_scope.type_param_names().contains("P"));
}

#[test]
fn semantic_state_class_scope_marks_enclosing_function_loads_nonlocal() {
    let mut body = parse_module_body(concat!(
        "def outer():\n",
        "    x = 1\n",
        "    class C:\n",
        "        y = x\n",
        "    return C\n",
    ));
    let semantic_state = SemanticAstState::from_ruff(&mut body);
    let outer_scope = function_scope(&semantic_state, find_function(&body, "outer"));
    let class_scope = outer_scope
        .child_scope_for_class(
            find_class_recursive(&find_function(&body, "outer").body, "C").expect("missing class"),
        )
        .expect("missing class scope");

    assert_eq!(
        class_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(
        outer_scope.binding_in_scope("x", SemanticBindingUse::Load),
        SemanticBindingKind::Nonlocal
    );
}

#[test]
fn semantic_state_keeps_nested_class_binding_shape_transformable() {
    let source = concat!(
        "class Container:\n",
        "    class Member:\n",
        "        pass\n",
        "\n",
        "def get_member():\n",
        "    return getattr(Container, \"Member\", None)\n",
    );
    let _ = lower_python_to_blockpy_for_testing(source).expect("transform should succeed");
}

#[test]
fn semantic_state_keeps_genexpr_iter_once_shape_transformable() {
    let source = concat!(
        "class Iterator:\n",
        "    def __next__(self):\n",
        "        raise StopIteration\n",
        "\n",
        "class Iterable:\n",
        "    def __iter__(self):\n",
        "        return Iterator()\n",
        "\n",
        "def run():\n",
        "    return list(x for x in Iterable())\n",
    );
    let _ = lower_python_to_blockpy_for_testing(source).expect("transform should succeed");
}
