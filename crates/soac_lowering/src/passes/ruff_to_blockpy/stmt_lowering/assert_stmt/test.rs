use super::super::{lower_instr_for_test, simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;

#[test]
fn stmt_assert_simplify_ast_desugars_before_blockpy_lowering() {
    let stmt = py_stmt!("assert cond, msg");
    let Stmt::Assert(assert_stmt) = stmt else {
        panic!("expected assert stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::Assert(assert_stmt));

    assert!(!matches!(simplified.as_slice(), [Stmt::Assert(_)]));
}

#[test]
fn stmt_assert_to_blockpy_uses_trait_owned_simplification_path() {
    let stmt = py_stmt!("assert cond, msg");
    let assert_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &assert_stmt, &name_gen, &mut out, None)
        .expect("assert lowering should succeed");

    let fragment = out.finish();
    assert!(!fragment.deps.is_empty());
}
