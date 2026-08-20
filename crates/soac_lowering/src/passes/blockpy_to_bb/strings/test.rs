use super::hoist_module_constants;
use crate::{
    block_py::{ChildVisitable, ConstantExpr, InstrBlockPy, Literal, NameLike},
    lower_python_to_blockpy_for_testing,
    pass_tracker::LoweringPassTrackerInternalExt,
    passes::blockpy_to_bb::exception_pass::lower_try_jump_exception_flow,
};

fn tracked_name_binding_module(
    source: &str,
) -> crate::block_py::BlockPyModule<crate::passes::ResolvedStorageModuleShape> {
    lower_python_to_blockpy_for_testing(source)
        .expect("transform should succeed")
        .pass_tracker
        .pass_name_binding()
        .expect("bb module should be available")
        .clone()
}

fn module_constants_contain_string(exprs: &[ConstantExpr]) -> bool {
    exprs.iter().any(|expr| {
        matches!(
            expr,
            ConstantExpr::Literal(literal)
                if matches!(literal.as_literal(), Literal::StringLiteral(_))
        )
    })
}

fn lowered_string_values(source: &str) -> Vec<String> {
    let module = lower_python_to_blockpy_for_testing(source)
        .expect("transform should succeed")
        .blockpy_module;
    let mut values = Vec::new();
    for constant in &module.module_constants {
        if let ConstantExpr::Literal(literal) = constant {
            if let Literal::StringLiteral(value) = literal.as_literal() {
                values.push(value.value.clone());
            }
        }
    }
    values
}

fn collect_helper_like_names_in_expr(out: &mut Vec<String>, expr: &InstrBlockPy) {
    struct HelperNameVisitor<'a> {
        out: &'a mut Vec<String>,
    }

    impl crate::block_py::Visit<InstrBlockPy> for HelperNameVisitor<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy) {
            collect_helper_like_names_in_expr(self.out, expr);
        }
    }

    match expr {
        InstrBlockPy::GetAttr(_) => out.push("__dp_getattr".to_string()),
        InstrBlockPy::SetAttr(_) => out.push("__dp_setattr".to_string()),
        InstrBlockPy::GetItem(_) => out.push("__dp_getitem".to_string()),
        InstrBlockPy::SetItem(_) => out.push("__dp_setitem".to_string()),
        InstrBlockPy::Call(operation) => {
            if let InstrBlockPy::Load(op) = &*operation.func {
                out.push(op.name.id_str().to_string());
            }
        }
        _ => {}
    }
    expr.visit_children(&mut HelperNameVisitor { out });
}

#[test]
fn keeps_string_literals_in_module_constants_and_out_of_executable_codegen() {
    let source = r#"
def f():
    x = __dp_store_global(globals(), "classify", __dp_ret("ok"))
    return x
"#;
    let bb_module = tracked_name_binding_module(source);
    let prepared = lower_try_jump_exception_flow(&bb_module);
    let normalized = hoist_module_constants(&prepared);

    assert!(
        module_constants_contain_string(&normalized.module_constants),
        "expected normalized module constants to retain string literals"
    );
}

#[test]
fn preserves_structured_intrinsics_for_attr_and_item_helpers() {
    let source = r#"
def f(obj, mapping, key, value):
    a = obj.x
    obj.x = value
    b = mapping[key]
    mapping[key] = value
    return a, b
"#;
    let bb_module = tracked_name_binding_module(source);
    let prepared = lower_try_jump_exception_flow(&bb_module);
    let normalized = hoist_module_constants(&prepared);

    let mut helper_names = Vec::new();
    for function in normalized.callable_defs {
        for block in &function.blocks {
            for stmt in &block.body {
                collect_helper_like_names_in_expr(&mut helper_names, stmt);
            }
        }
    }

    assert!(
        helper_names.iter().any(|name| name == "__dp_getattr"),
        "{helper_names:?}"
    );
    assert!(
        helper_names.iter().any(|name| name == "__dp_setattr"),
        "{helper_names:?}"
    );
    assert!(
        helper_names.iter().any(|name| name == "__dp_getitem"),
        "{helper_names:?}"
    );
    assert!(
        helper_names.iter().any(|name| name == "__dp_setitem"),
        "{helper_names:?}"
    );
    assert!(
        !helper_names.iter().any(|name| name == "PyObject_GetAttr"),
        "{helper_names:?}"
    );
    assert!(
        !helper_names.iter().any(|name| name == "PyObject_SetAttr"),
        "{helper_names:?}"
    );
    assert!(
        !helper_names.iter().any(|name| name == "PyObject_GetItem"),
        "{helper_names:?}"
    );
    assert!(
        !helper_names.iter().any(|name| name == "PyObject_SetItem"),
        "{helper_names:?}"
    );
}

#[test]
fn rejects_unsupported_surrogate_escapes_before_lowering() {
    for source in [
        "def f():\n    return \"\\udca7\" \"b\"\n",
        "def f(x):\n    return f\"\\udca7{x}\"\n",
        "def f(x):\n    return f\"{x:\\udca7}\"\n",
        "def f(x):\n    return t\"\\udca7{x}\"\n",
        "def f(x):\n    return t\"{x:\\udca7}\"\n",
    ] {
        let error = match lower_python_to_blockpy_for_testing(source) {
            Err(crate::LoweringError::Other(error)) => error,
            _ => panic!("unsupported surrogate literal must fail before AST rewriting"),
        };
        let unsupported = error
            .downcast_ref::<soac_source::UnsupportedSurrogateEscape>()
            .expect("structured source-literal diagnostic");
        assert_eq!(unsupported.code_point(), 0xDCA7);
        assert_eq!(&source[unsupported.range()], r"\udca7");
    }
}

#[test]
fn preserves_raw_backslashes_and_genuine_replacement_characters() {
    for (source, expected) in [
        ("def f():\n    return r\"\\udca7\" \"b\"\n", "\\udca7b"),
        ("def f(x):\n    return rf\"\\udca7{x}\"\n", "\\udca7"),
        ("def f():\n    return \"\\\\udca7\"\n", "\\udca7"),
        ("def f():\n    return \"�\"\n", "�"),
        ("def f(x):\n    return f\"\\ufffd{x}\"\n", "�"),
    ] {
        let values = lowered_string_values(source);
        assert!(
            values.iter().any(|value| value == expected),
            "expected {expected:?} in {values:?}"
        );
    }
}
