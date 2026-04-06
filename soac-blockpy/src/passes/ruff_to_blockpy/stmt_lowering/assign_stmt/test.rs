use super::super::{BlockPyStmtBuilder, lower_instr_for_test};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;

#[test]
fn stmt_assign_to_blockpy_emits_direct_core_setitem() {
    let stmt = py_stmt!("obj[idx] = value");
    let assign_stmt = crate::passes::InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &assign_stmt, &name_gen, &mut out, None)
        .expect("assign lowering should succeed");

    let fragment = out.finish();
    let Some(expr) = fragment.entry.body.last() else {
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
