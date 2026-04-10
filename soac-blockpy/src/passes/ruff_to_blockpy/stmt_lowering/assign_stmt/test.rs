use super::super::{lower_instr_for_test, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;
use ruff_python_ast::{Expr, Stmt};

fn is_soac_attr_call(expr: &Expr, expected_attr: &str) -> bool {
    let Expr::Call(call) = expr else {
        return false;
    };
    matches!(
        call.func.as_ref(),
        Expr::Attribute(attr)
            if matches!(attr.value.as_ref(), Expr::Name(name) if name.id.as_str() == "__soac__")
                && attr.attr.id.as_str() == expected_attr
    )
}

#[test]
fn stmt_assign_to_blockpy_emits_direct_core_setitem() {
    let stmt = py_stmt!("obj[idx] = value");
    let assign_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &assign_stmt, &name_gen, &mut out, None)
        .expect("assign lowering should succeed");

    let fragment = out.finish();
    assert!(fragment
        .entry
        .body
        .iter()
        .any(|instr| matches!(instr, InstrWithAwaitAndYield::SetItem(_))));
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

    assert!(out.iter().any(|stmt| matches!(
        stmt,
        Stmt::Assign(assign)
            if matches!(assign.targets.as_slice(), [Expr::Name(name)] if name.id.as_str() == "a")
                && matches!(assign.value.as_ref(), Expr::Subscript(_))
    )));
    assert!(out.iter().any(|stmt| matches!(
        stmt,
        Stmt::Assign(assign)
            if matches!(assign.targets.as_slice(), [Expr::Name(name)] if name.id.as_str() == "b")
                && is_soac_attr_call(assign.value.as_ref(), "list")
    )));
}

#[test]
fn rewrite_assignment_target_uses_native_store_targets() {
    let cases = ["obj[idx]", "obj.attr"];

    for target_src in cases {
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

        let [Stmt::Assign(assign)] = out.as_slice() else {
            panic!("expected one assignment for {target_src}, got {out:?}");
        };
        assert!(
            matches!(
                (target_src, assign.targets.as_slice(), assign.value.as_ref()),
                ("obj[idx]", [Expr::Subscript(_)], Expr::Name(name)) if name.id.as_str() == "value"
            ) || matches!(
                (target_src, assign.targets.as_slice(), assign.value.as_ref()),
                ("obj.attr", [Expr::Attribute(_)], Expr::Name(name)) if name.id.as_str() == "value"
            ),
            "unexpected assignment rewrite for {target_src}: {assign:?}",
        );
    }
}
