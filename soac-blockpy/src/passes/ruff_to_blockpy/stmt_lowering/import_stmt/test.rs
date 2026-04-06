use super::super::{lower_instr_for_test, simplify_stmt_ast_once_for_blockpy, BlockPyStmtBuilder};
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::py_stmt;
use ruff_python_ast::Stmt;

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
    let import_stmt = crate::passes::InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &import_stmt, &name_gen, &mut out, None)
        .expect("import lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(
        fragment.entry.body.as_slice(),
        [InstrWithAwaitAndYield::Store(_)]
    ));
}
