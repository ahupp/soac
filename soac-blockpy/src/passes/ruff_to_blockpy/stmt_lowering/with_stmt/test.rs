use super::super::{lower_instr_for_test, simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;

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
#[should_panic(expected = "With should be lowered before Ruff AST -> BlockPy stmt-list conversion")]
fn stmt_with_to_blockpy_rejects_sequence_only_stmt_lowering() {
    let stmt = py_stmt!("with cm:\n    body()");
    let with_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let _ = lower_instr_for_test(&context, &with_stmt, &name_gen, &mut out, None);
}
