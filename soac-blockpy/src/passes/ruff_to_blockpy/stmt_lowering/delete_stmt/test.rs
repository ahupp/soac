use super::super::BlockPyStmtBuilder;
use super::*;
use crate::block_py::{InstrWithAwaitAndYield, StructuredInstr};
use crate::passes::ast_to_ast::context::Context;

#[test]
fn stmt_delete_to_blockpy_emits_direct_core_delitem() {
    let stmt = py_stmt!("del obj[idx]");
    let Stmt::Delete(delete_stmt) = stmt else {
        panic!("expected delete stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    delete_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("delete lowering should succeed");

    let fragment = out.finish();
    let Some(StructuredInstr::Expr(expr)) = fragment.body.last() else {
        panic!("expected final expr stmt, got {fragment:?}");
    };
    let rendered = format!("{expr:?}");

    assert!(rendered.contains("DelItem("), "{rendered}");
    assert!(!rendered.contains("__dp_delitem"), "{rendered}");
}
