use super::super::{lower_instr_for_test, simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;

#[test]
fn stmt_type_alias_simplify_ast_desugars_before_blockpy_lowering() {
    let stmt = py_stmt!("type X = int");
    let Stmt::TypeAlias(type_alias) = stmt else {
        panic!("expected type alias stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::TypeAlias(type_alias));

    assert!(!matches!(simplified.as_slice(), [Stmt::TypeAlias(_)]));
}

#[test]
fn stmt_type_alias_to_blockpy_uses_trait_owned_simplification_path() {
    let stmt = py_stmt!("type X = int");
    let type_alias = crate::passes::InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &type_alias, &name_gen, &mut out, None)
        .expect("type alias lowering should succeed");

    let fragment = out.finish();
    assert!(!fragment.entry.body.is_empty());
}

#[test]
fn stmt_type_alias_rewrite_type_alias_stmt_handles_type_params() {
    let stmt = py_stmt!("type Alias[T] = list[T]");
    let Stmt::TypeAlias(type_alias) = stmt else {
        panic!("expected type alias stmt");
    };

    let context = Context::new("");
    let rewritten = rewrite_type_alias_stmt(&context, type_alias);
    let simplified = stmts_from_rewrite(rewritten);

    assert!(!matches!(simplified.as_slice(), [Stmt::TypeAlias(_)]));
}
