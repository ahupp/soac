use super::normalize_bb_module_strings;
use crate::{
    block_py::{
        BlockPyLiteral, BlockPyNameLike, ChildVisitable, CodegenBlockPyExpr, InstrLow,
        InstrResolved,
    },
    lower_python_to_blockpy_for_testing,
    passes::lower_try_jump_exception_flow,
};

fn tracked_name_binding_module(
    source: &str,
) -> crate::block_py::BlockPyModule<crate::passes::ResolvedStorageBlockPyPass> {
    lower_python_to_blockpy_for_testing(source)
        .expect("transform should succeed")
        .pass_tracker
        .pass_name_binding()
        .expect("bb module should be available")
        .clone()
}

fn module_constants_contain_string(exprs: &[InstrResolved]) -> bool {
    exprs.iter().any(|expr| {
        matches!(
            expr,
            InstrLow::Literal(literal)
                if matches!(literal.as_literal(), BlockPyLiteral::StringLiteral(_))
        )
    })
}

fn collect_helper_like_names_in_expr(out: &mut Vec<String>, expr: &CodegenBlockPyExpr) {
    struct HelperNameVisitor<'a> {
        out: &'a mut Vec<String>,
    }

    impl crate::block_py::Visit<CodegenBlockPyExpr> for HelperNameVisitor<'_> {
        fn visit_instr(&mut self, expr: &CodegenBlockPyExpr) {
            collect_helper_like_names_in_expr(self.out, expr);
        }
    }

    match expr {
        CodegenBlockPyExpr::CalleeFunctionId(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::GetAttr(operation) => {
            out.push("__dp_getattr".to_string());
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::SetAttr(operation) => {
            out.push("__dp_setattr".to_string());
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::GetItem(operation) => {
            out.push("__dp_getitem".to_string());
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::SetItem(operation) => {
            out.push("__dp_setitem".to_string());
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::Call(operation) => {
            if let CodegenBlockPyExpr::Load(op) = &*operation.func {
                out.push(op.name.id_str().to_string());
            }
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::CallDirect(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::BinOp(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::UnaryOp(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::Load(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::Store(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::Del(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::MakeCell(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::IncrementCounter(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::CellRef(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::MakeFunction(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
        CodegenBlockPyExpr::DelItem(operation) => {
            operation.visit_children(&mut HelperNameVisitor { out });
        }
    }
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
    let normalized = normalize_bb_module_strings(&prepared);

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
    let normalized = normalize_bb_module_strings(&prepared);

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
fn preserves_surrogate_escaped_string_literals_in_module_constants() {
    let source = "def f():\n    return \"\\udca7\" \"b\"\n";
    let bb_module = tracked_name_binding_module(source);
    let prepared = lower_try_jump_exception_flow(&bb_module);
    let normalized = normalize_bb_module_strings(&prepared);

    assert!(
        module_constants_contain_string(&normalized.module_constants),
        "expected surrogate-escaped string to remain in module constants"
    );
}
