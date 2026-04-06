use super::super::{lower_instr_for_test, simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::{BlockTerm, InstrWithAwaitAndYield};
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;

#[test]
fn stmt_if_simplify_ast_expands_elif_chain_before_blockpy_lowering() {
    let stmt = py_stmt!("if x:\n    a()\nelif y:\n    b()\nelse:\n    c()");
    let Stmt::If(if_stmt) = stmt else {
        panic!("expected if stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::If(if_stmt));
    let [Stmt::If(simplified_if)] = simplified.as_slice() else {
        panic!("if simplification should remain an if stmt");
    };

    assert_eq!(simplified_if.elif_else_clauses.len(), 1);
    let clause = &simplified_if.elif_else_clauses[0];
    assert!(clause.test.is_none());
    assert!(matches!(&clause.body[0], Stmt::If(_)));
}

#[test]
fn stmt_if_to_blockpy_uses_trait_owned_simplification_path_for_elif() {
    let stmt = py_stmt!("if x:\n    a()\nelif y:\n    b()\nelse:\n    c()");
    let if_stmt = crate::passes::InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &if_stmt, &name_gen, &mut out, None)
        .expect("if lowering should succeed");

    let fragment = out.finish();
    assert!(!fragment.deps.is_empty());
}

#[test]
fn stmt_if_fragment_empty_orelse_uses_explicit_fallthrough() {
    let stmt = py_stmt!("if False:\n    x = None");
    let Stmt::If(if_stmt) = stmt else {
        panic!("expected if stmt");
    };

    let module_name_gen = crate::block_py::ModuleNameGen::new(0);
    let name_gen = module_name_gen.next_function_name_gen();
    let context = Context::new("");
    let fragment = try_lower_if_stmt_fragment::<InstrWithAwaitAndYield>(
        &context,
        &name_gen,
        &if_stmt,
        None,
    )
    .expect("if stmt should use direct fragment path")
    .expect("if stmt fragment lowering should succeed");

    let else_block = fragment
        .deps
        .iter()
        .find(|block| {
            block.body.is_empty()
                && matches!(
                    &block.term,
                    BlockTerm::Jump(edge) if edge.target.is_fallthrough()
                )
        })
        .expect("empty else branch should become an explicit fallthrough block");

    assert!(else_block.body.is_empty(), "{else_block:#?}");
}
