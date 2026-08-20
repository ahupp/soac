use super::{
    SemanticAstState, SemanticBindingKind, SemanticBindingUse, SemanticScope, SemanticScopeKind,
};
use crate::lower_python_to_blockpy_for_testing;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::rewrite_class_def::class_body::rewrite_class_body_scopes;
use ruff_python_ast::{self as ast, Stmt};
use ruff_python_parser::parse_module;
use ruff_text_size::Ranged;

fn semantic_from_original(body: &mut ast::Suite) -> SemanticAstState {
    let names = super::SourceNameCatalog::from_original(body);
    SemanticAstState::from_ruff(body, &names)
}

fn parse_module_body(source: &str) -> ast::Suite {
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
    crate::passes::ast_to_ast::rewrite_class_def::record_class_static_attributes(
        &context,
        &mut module,
    );
    let mut semantic_state = semantic_from_original(&mut module);
    rewrite_class_body_scopes(&context, &mut semantic_state, &mut module);
}

#[test]
fn semantic_state_module_bindings_include_assignments() {
    let mut body = parse_module_body("x = 1\ny = 2\n");
    let semantic_state = semantic_from_original(&mut body);
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
    let mut semantic_state = semantic_from_original(&mut body);
    let module_init: ast::StmtFunctionDef = crate::template::py_stmt_typed!(
        r#"
def _dp_module_init():
    pass
"#
    );
    let module_init_scope = semantic_state.synthesize_module_init_scope(&module_init);

    assert_eq!(module_init_scope.kind(), SemanticScopeKind::Module);
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
    let semantic_state = semantic_from_original(&mut body);
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
fn semantic_state_pattern_captures_obey_local_global_and_nonlocal_scope() {
    let mut body = parse_module_body(
        r#"
module_capture = 0
def probe(value):
    match value:
        case capture if guard(capture):
            return capture

def outer():
    captured = 0
    def update(value):
        global module_capture
        nonlocal captured
        match value:
            case [local, captured, module_capture]:
                return local
    return update
"#,
    );
    let state = semantic_from_original(&mut body);
    let probe = function_scope(&state, find_function(&body, "probe"));
    assert_eq!(
        probe.resolved_load_binding("capture"),
        SemanticBindingKind::Local
    );
    let outer = find_function(&body, "outer");
    let update = function_scope(&state, find_function(&outer.body, "update"));
    for (name, expected) in [
        ("local", SemanticBindingKind::Local),
        ("captured", SemanticBindingKind::Nonlocal),
        ("module_capture", SemanticBindingKind::Global),
    ] {
        assert_eq!(update.resolved_load_binding(name), expected, "{name}");
    }
}

#[test]
fn semantic_state_pattern_capture_can_be_closed_over() {
    let mut body = parse_module_body(
        r#"
def factory(value):
    match value:
        case captured:
            pass
    def get():
        return captured
    return get
"#,
    );
    let state = semantic_from_original(&mut body);
    let factory = find_function(&body, "factory");
    let outer = function_scope(&state, factory);
    let inner = function_scope(&state, find_function(&factory.body, "get"));
    assert_eq!(
        outer.resolved_load_binding("captured"),
        SemanticBindingKind::Nonlocal
    );
    assert_eq!(
        inner.resolved_load_binding("captured"),
        SemanticBindingKind::Nonlocal
    );
}

#[test]
fn semantic_state_preserves_lambda_scope_bindings() {
    let mut body = parse_module_body(concat!("def outer(x):\n", "    return lambda y: x + y\n",));
    let semantic_state = semantic_from_original(&mut body);
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
fn semantic_state_preserves_lambda_default_scopes_as_siblings() {
    let mut body = parse_module_body(concat!(
        "def outer(value):\n",
        "    return lambda callback=(lambda: value), /, *, extra=(lambda: value): callback()\n",
    ));
    let state = semantic_from_original(&mut body);
    let outer = find_function(&body, "outer");
    let outer_scope = function_scope(&state, outer);
    let Stmt::Return(statement) = &outer.body[0] else {
        panic!("expected return statement");
    };
    let Some(ast::Expr::Lambda(lambda)) = statement.value.as_deref() else {
        panic!("expected lambda return value");
    };
    let lambda_scope = state.lambda_scope(lambda).unwrap();
    let parameters = lambda.parameters.as_ref().unwrap();
    for parameter in [&parameters.posonlyargs[0], &parameters.kwonlyargs[0]] {
        let Some(ast::Expr::Lambda(default)) = parameter.default.as_deref() else {
            panic!("expected callable default");
        };
        let scope = state
            .lambda_scope(default)
            .expect("preserved default scope");
        assert_eq!(scope.data().parent, Some(outer_scope.scope_id));
        assert_ne!(scope.scope_id, lambda_scope.scope_id);
        assert_eq!(scope.qualname(), "outer.<locals>.<lambda>");
        assert_eq!(
            scope.resolved_load_binding("value"),
            SemanticBindingKind::Nonlocal
        );
    }
    assert_eq!(
        lambda_scope.binding_in_current_scope("callback"),
        Some(SemanticBindingKind::Local)
    );
    assert!(outer_scope.data().local_cell_bindings.contains("value"));
}

#[test]
fn semantic_state_named_expr_in_while_test_binds_local() {
    let mut body = parse_module_body(concat!(
        "def f(values):\n",
        "    while not (value := values[0]):\n",
        "        break\n",
        "    return value\n",
    ));
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
fn semantic_state_dunder_class_capture_keeps_nearest_function_owner() {
    for (parameters, binding) in [
        ("self, __class__", ""),
        ("self, value", "        __class__ = value\n"),
    ] {
        for declaration in ["", "            nonlocal __class__\n"] {
            let mut body = parse_module_body(&format!(
                "class C:\n    def owner({parameters}):\n{binding}        def read():\n{declaration}            return __class__\n        return read\n"
            ));
            let semantic_state = semantic_from_original(&mut body);
            let class_def = find_class(&body, "C");
            let owner = find_function(&class_def.body, "owner");
            let reader = find_function(&owner.body, "read");
            let reader_scope = function_scope(&semantic_state, reader);
            assert_eq!(
                reader_scope.binding_in_current_scope("__class__"),
                Some(SemanticBindingKind::Nonlocal)
            );
            assert_eq!(
                reader_scope.cell_storage_name("__class__"),
                None,
                "the nearest source-owned cell must not become the class's implicit cell"
            );
            assert!(function_scope(&semantic_state, owner)
                .local_cell_bindings()
                .contains("__class__"));
        }
    }
}

#[test]
fn semantic_state_dunder_class_capture_stops_at_nearer_class_owner() {
    let mut body = parse_module_body(concat!(
        "def outer(__class__):\n",
        "    class C:\n",
        "        def read(self):\n",
        "            return __class__\n",
        "    return C\n",
    ));
    let semantic_state = semantic_from_original(&mut body);
    let outer = find_function(&body, "outer");
    let class_def = find_class(&outer.body, "C");
    let reader = find_function(&class_def.body, "read");
    let reader_scope = function_scope(&semantic_state, reader);
    assert_eq!(
        reader_scope.binding_in_current_scope("__class__"),
        Some(SemanticBindingKind::Nonlocal)
    );
    assert_eq!(
        reader_scope.cell_storage_name("__class__").as_deref(),
        Some("_dp_classcell"),
        "the intervening class, not the farther function parameter, owns the method cell"
    );
}

#[test]
fn semantic_state_dunder_class_capture_stops_at_explicit_function_global() {
    let mut body = parse_module_body(concat!(
        "class C:\n",
        "    def owner(self):\n",
        "        global __class__\n",
        "        def read():\n",
        "            return __class__\n",
        "        return read\n",
    ));
    let semantic_state = semantic_from_original(&mut body);
    let class_def = find_class(&body, "C");
    let owner = find_function(&class_def.body, "owner");
    let reader = find_function(&owner.body, "read");
    let reader_scope = function_scope(&semantic_state, reader);
    assert_eq!(
        reader_scope.resolved_load_binding("__class__"),
        SemanticBindingKind::Global,
        "an explicit intervening global declaration removes the enclosing cell binding"
    );
    assert_eq!(reader_scope.cell_storage_name("__class__"), None);
}

#[test]
fn semantic_state_does_not_create_classcell_for_module_level_explicit_super() {
    let mut body = parse_module_body(concat!("def f(cls):\n", "    return super(Generic, cls)\n",));
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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
    let semantic_state = semantic_from_original(&mut body);
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

#[test]
fn source_prefixed_bindings_generated_same_spelling_does_not_gain_source_provenance() {
    let mut body = parse_module_body(
        "def owner(_dp_source):\n    def source_read():\n        return _dp_source\n    def generated_read():\n        return None\n    return source_read, generated_read\n",
    );
    let source_names = super::SourceNameCatalog::from_original(&mut body);
    let Stmt::FunctionDef(owner) = &mut body[0] else {
        unreachable!()
    };
    let source_range = match &owner.body[0] {
        Stmt::FunctionDef(source) => match &source.body[0] {
            Stmt::Return(return_) => return_.value.as_ref().unwrap().range(),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    };
    let generated = owner
        .body
        .iter_mut()
        .find_map(|statement| match statement {
            Stmt::FunctionDef(function) if function.name.as_str() == "generated_read" => {
                Some(function)
            }
            _ => None,
        })
        .unwrap();
    generated.body = vec![crate::template::py_stmt!("return _dp_source")].into();
    let Stmt::Return(return_) = &mut generated.body[0] else {
        unreachable!()
    };
    let ast::Expr::Name(name) = return_.value.as_deref_mut().unwrap() else {
        unreachable!()
    };
    name.range = source_range;
    // Both spelling and range now match a real source use, but this newly
    // generated operation does not carry the original node identity.

    let state = SemanticAstState::from_ruff(&mut body, &source_names);
    let owner = find_function(&body, "owner");
    let source_read = function_scope(&state, find_function(&owner.body, "source_read"));
    let generated_read = function_scope(&state, find_function(&owner.body, "generated_read"));
    assert_eq!(
        source_read.resolved_load_binding("_dp_source"),
        SemanticBindingKind::Nonlocal
    );
    assert!(source_read.source_names().contains("_dp_source"));
    assert_eq!(
        generated_read.resolved_load_binding("_dp_source"),
        SemanticBindingKind::Local
    );
    assert!(!generated_read.source_names().contains("_dp_source"));
}
