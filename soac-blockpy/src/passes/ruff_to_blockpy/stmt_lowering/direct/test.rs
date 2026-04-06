use super::super::{simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;

#[test]
fn stmt_raise_simplify_ast_desugars_raise_from_before_blockpy_lowering() {
    let stmt = py_stmt!("raise exc from cause");
    let Stmt::Raise(raise_stmt) = stmt else {
        panic!("expected raise stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::Raise(raise_stmt));

    assert!(!matches!(
        simplified.as_slice(),
        [Stmt::Raise(ast::StmtRaise { cause: Some(_), .. })]
    ));
}

#[test]
fn stmt_raise_to_blockpy_handles_bare_raise_directly() {
    let stmt = py_stmt!("raise");
    let Stmt::Raise(raise_stmt) = stmt else {
        panic!("expected raise stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    raise_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("raise lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(fragment.term, Some(BlockTerm::Raise(_))));
}

#[test]
fn stmt_expr_to_blockpy_emits_setup_for_named_exprs() {
    let stmt = py_stmt!("(x := y)");
    let Stmt::Expr(expr_stmt) = stmt else {
        panic!("expected expr stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    expr_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("expr lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(
        fragment.body.as_slice(),
        [
            StructuredInstr::Expr(InstrWithAwaitAndYield::Store(_)),
            StructuredInstr::Expr(_)
        ]
    ));
}

#[test]
fn stmt_expr_fragment_uses_plain_instr_body_with_fallthrough() {
    let stmt = py_stmt!("(x := y)");
    let context = Context::new("");
    let mut next_label_id = 0usize;

    let Some(fragment) = try_lower_direct_stmt_fragment::<InstrWithAwaitAndYield>(
        &context,
        &stmt,
        None,
        &mut next_label_id,
    ) else {
        panic!("expected direct expr stmt fragment");
    };
    let fragment = fragment.expect("expr fragment lowering should succeed");

    assert!(fragment.deps.is_empty());
    assert!(matches!(
        fragment.entry.body.as_slice(),
        [InstrWithAwaitAndYield::Store(_), _]
    ));
    assert!(matches!(
        fragment.entry.term,
        Some(BlockTerm::Jump(BlockEdge { target, args }))
            if target == BlockLabel::fallthrough() && args.is_empty()
    ));
}

#[test]
fn stmt_expr_fragment_returns_none_for_branchy_expr_setup() {
    let stmt = py_stmt!("x if cond else y");
    let context = Context::new("");
    let mut next_label_id = 17usize;

    let fragment = try_lower_direct_stmt_fragment::<InstrWithAwaitAndYield>(
        &context,
        &stmt,
        None,
        &mut next_label_id,
    );

    assert!(fragment.is_none());
    assert_eq!(next_label_id, 17);
}

#[test]
fn stmt_return_to_blockpy_emits_setup_for_if_exprs() {
    let stmt = py_stmt!("return x if cond else y");
    let Stmt::Return(return_stmt) = stmt else {
        panic!("expected return stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    return_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("return lowering should succeed");

    let fragment = out.finish();
    assert!(!fragment.body.is_empty());
    assert!(matches!(fragment.term, Some(BlockTerm::Return(_))));
}
