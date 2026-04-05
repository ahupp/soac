use super::super::BlockPyStmtBuilder;
use super::*;
use crate::block_py::{InstrWithAwaitAndYield, StructuredInstr};
use crate::passes::ast_to_ast::context::Context;

#[test]
fn stmt_augassign_simplify_ast_keeps_stmt_for_direct_lowering() {
    let stmt = py_stmt!("x += y");
    let Stmt::AugAssign(aug_stmt) = stmt else {
        panic!("expected augassign stmt");
    };

    let context = Context::new("");
    let simplified = simplify_stmt_ast_once_for_blockpy(&context, Stmt::AugAssign(aug_stmt));

    assert!(matches!(simplified.as_slice(), [Stmt::AugAssign(_)]));
}

#[test]
fn stmt_augassign_to_blockpy_emits_direct_core_operations() {
    let stmt = py_stmt!("obj[idx] += y");
    let Stmt::AugAssign(aug_stmt) = stmt else {
        panic!("expected augassign stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    aug_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("augassign lowering should succeed");

    let fragment = out.finish();
    let Some(StructuredInstr::Expr(expr)) = fragment.body.last() else {
        panic!("expected final expr stmt, got {fragment:?}");
    };
    let rendered = format!("{expr:?}");

    assert!(rendered.contains("SetItem("), "{rendered}");
    assert!(rendered.contains("InplaceAdd"), "{rendered}");
    assert!(!rendered.contains("__dp_iadd"), "{rendered}");
    assert!(!rendered.contains("__dp_setitem"), "{rendered}");
}

#[test]
fn stmt_pow_augassign_to_blockpy_uses_inplace_pow() {
    let stmt = py_stmt!("x **= y");
    let Stmt::AugAssign(aug_stmt) = stmt else {
        panic!("expected augassign stmt");
    };
    let context = Context::new("");
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    aug_stmt
        .to_blockpy(&context, &mut out, None, &mut next_label_id)
        .expect("pow augassign lowering should succeed");

    let fragment = out.finish();
    let Some(StructuredInstr::Expr(InstrWithAwaitAndYield::Store(assign))) =
        fragment.body.last()
    else {
        panic!("expected final store expr stmt, got {fragment:?}");
    };
    let rendered = format!("{:?}", assign.value);

    assert!(rendered.contains("InplacePow"), "{rendered}");
}
