use super::super::{simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;

#[test]
fn stmt_with_simplify_ast_desugars_before_blockpy_lowering() {
    let stmt = py_stmt!("with cm:\n    body()");
    let Stmt::With(with_stmt) = stmt else {
        panic!("expected with stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::With(with_stmt));

    assert!(!matches!(simplified.as_slice(), [Stmt::With(_)]));
}

#[test]
fn stmt_with_simplify_ast_uses_native_identity_test() {
    let stmt = py_stmt!("async def f(cm):\n    async with cm:\n        body()");
    let Stmt::FunctionDef(func) = stmt else {
        panic!("expected function def");
    };
    let Stmt::With(with_stmt) = func.body[0].clone() else {
        panic!("expected with stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::With(with_stmt));
    let rendered = simplified
        .iter()
        .map(crate::ruff_ast_to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("__dp_is_not("), "{rendered}");
    assert!(rendered.contains(" is not None"), "{rendered}");
}

#[test]
#[should_panic(expected = "StmtTry should have already been reduced before BlockPy lowering")]
fn stmt_with_to_blockpy_simplifies_before_hitting_sequence_only_try_lowering() {
    let stmt = py_stmt!("with cm:\n    body()");
    let Stmt::With(with_stmt) = stmt else {
        panic!("expected with stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    let _ = with_stmt.to_blockpy(&context, &mut out, None, &mut next_label_id);
}
