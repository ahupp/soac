use super::super::{simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;

#[test]
fn stmt_import_simplify_ast_desugars_before_blockpy_lowering() {
    let stmt = py_stmt!("import pkg.sub");
    let Stmt::Import(import_stmt) = stmt else {
        panic!("expected import stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::Import(import_stmt));

    assert!(!matches!(simplified.as_slice(), [Stmt::Import(_)]));
}

#[test]
fn stmt_import_to_blockpy_uses_trait_owned_simplification_path() {
    let stmt = py_stmt!("import pkg.sub");
    let Stmt::Import(import_stmt) = stmt else {
        panic!("expected import stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    import_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("import lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(
        fragment.body.as_slice(),
        [StructuredInstr::Expr(
            InstrWithAwaitAndYield::Store(_)
        )]
    ));
}
