use super::super::{simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;

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
    let Stmt::Assert(assert_stmt) = stmt else {
        panic!("expected assert stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    assert_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("assert lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(fragment.body.as_slice(), [StructuredInstr::If(_)]));
}
