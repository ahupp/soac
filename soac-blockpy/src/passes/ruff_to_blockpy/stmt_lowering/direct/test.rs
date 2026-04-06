use super::super::{lower_instr_for_test, simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;

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
    let raise_stmt = InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &raise_stmt, &name_gen, &mut out, None)
        .expect("raise lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(fragment.entry.term, BlockTerm::Raise(_)));
}

#[test]
fn stmt_expr_to_blockpy_emits_setup_for_named_exprs() {
    let stmt = py_stmt!("(x := y)");
    let expr_stmt = InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &expr_stmt, &name_gen, &mut out, None)
        .expect("expr lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(
        fragment.entry.body.as_slice(),
        [InstrWithAwaitAndYield::Store(_), _]
    ));
}

#[test]
fn stmt_return_to_blockpy_sets_terminator_for_plain_value() {
    let stmt = py_stmt!("return value");
    let context = Context::new("");
    let return_stmt = InstrRuff::from_ast_stmt(stmt);
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &return_stmt, &name_gen, &mut out, None)
        .expect("return lowering should succeed");

    let fragment = out.finish();

    assert!(matches!(fragment.entry.term, BlockTerm::Return(_)));
}

#[test]
fn stmt_raise_to_blockpy_sets_terminator_for_plain_exc() {
    let stmt = py_stmt!("raise exc");
    let context = Context::new("");
    let raise_stmt = InstrRuff::from_ast_stmt(stmt);
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &raise_stmt, &name_gen, &mut out, None)
        .expect("raise lowering should succeed");

    let fragment = out.finish();

    assert!(matches!(
        fragment.entry.term,
        BlockTerm::Raise(TermRaise { exc: Some(_) })
    ));
}

#[test]
fn stmt_break_to_blockpy_uses_loop_jump() {
    let stmt = py_stmt!("break");
    let context = Context::new("");
    let break_stmt = InstrRuff::from_ast_stmt(stmt);
    let loop_ctx = LoopContext {
        continue_label: BlockLabel::from_index(7),
        break_label: BlockLabel::from_index(9),
    };
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &break_stmt, &name_gen, &mut out, Some(&loop_ctx))
        .expect("break lowering should succeed");

    let (entry_ref, blocks) = out.finish_blocks();
    let entry = blocks
        .iter()
        .find(|block| block.label == entry_ref.label())
        .expect("break entry block should be present");

    assert!(matches!(
        entry.term,
        BlockTerm::Jump(BlockEdge { target, ref args })
            if target == BlockLabel::from_index(9) && args.is_empty()
    ));
}

#[test]
fn stmt_return_to_blockpy_emits_setup_for_if_exprs() {
    let stmt = py_stmt!("return x if cond else y");
    let return_stmt = InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &return_stmt, &name_gen, &mut out, None)
        .expect("return lowering should succeed");

    let fragment = out.finish();
    assert!(fragment.entry.body.is_empty(), "{fragment:?}");
    assert!(matches!(fragment.entry.term, BlockTerm::Jump(_)), "{fragment:?}");
    assert!(fragment.deps.iter().any(|block| matches!(block.term, BlockTerm::Return(_))), "{fragment:?}");
}
