use super::super::BlockPyStmtBuilder;
use super::*;
use crate::block_py::{InstrWithAwaitAndYield, StructuredInstr};
use crate::passes::ast_to_ast::context::Context;

#[test]
fn stmt_assign_to_blockpy_emits_direct_core_setitem() {
    let stmt = py_stmt!("obj[idx] = value");
    let Stmt::Assign(assign_stmt) = stmt else {
        panic!("expected assign stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    assign_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("assign lowering should succeed");

    let fragment = out.finish();
    let Some(StructuredInstr::Expr(expr)) = fragment.body.last() else {
        panic!("expected final expr stmt, got {fragment:?}");
    };
    let rendered = format!("{expr:?}");

    assert!(rendered.contains("SetItem("), "{rendered}");
    assert!(!rendered.contains("__dp_setitem"), "{rendered}");
}

#[test]
fn rewrite_assignment_target_unpack_uses_native_subscript_ast() {
    let target = py_expr!("a, *b");
    let rhs = py_expr!("value");
    let mut out = Vec::new();
    let mut next_temp_id = 0usize;
    let mut next_temp = |prefix: &str| {
        let name = format!("_dp_{prefix}_{next_temp_id}");
        next_temp_id += 1;
        name
    };

    rewrite_assignment_target(target, rhs, &mut out, &mut next_temp);

    let rendered = out
        .iter()
        .map(crate::ruff_ast_to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("__dp_getitem("), "{rendered}");
    assert!(rendered.contains("[0]"), "{rendered}");
    assert!(rendered.contains("__soac__.list("), "{rendered}");
}

#[test]
fn rewrite_assignment_target_uses_native_store_targets() {
    let cases = [("obj[idx]", "[idx] = value"), ("obj.attr", ".attr = value")];

    for (target_src, expected) in cases {
        let target = *ruff_python_parser::parse_expression(target_src)
            .unwrap()
            .into_syntax()
            .body;
        let rhs = py_expr!("value");
        let mut out = Vec::new();
        let mut next_temp_id = 0usize;
        let mut next_temp = |prefix: &str| {
            let name = format!("_dp_{prefix}_{next_temp_id}");
            next_temp_id += 1;
            name
        };

        rewrite_assignment_target(target, rhs, &mut out, &mut next_temp);

        let rendered = out
            .iter()
            .map(crate::ruff_ast_to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("__dp_setitem("), "{rendered}");
        assert!(!rendered.contains("__dp_setattr("), "{rendered}");
        assert!(rendered.contains(expected), "{rendered}");
    }
}
