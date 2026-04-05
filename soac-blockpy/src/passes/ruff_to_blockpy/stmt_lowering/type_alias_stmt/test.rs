use super::super::{simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;

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
    let Stmt::TypeAlias(type_alias) = stmt else {
        panic!("expected type alias stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    type_alias
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("type alias lowering should succeed");

    let fragment = out.finish();
    assert!(!fragment.body.is_empty());
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
