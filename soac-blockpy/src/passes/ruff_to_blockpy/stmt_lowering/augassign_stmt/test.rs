use super::super::{lower_instr_for_test, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;

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
    let aug_stmt = crate::passes::InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &aug_stmt, &name_gen, &mut out, None)
        .expect("augassign lowering should succeed");

    let fragment = out.finish();
    let Some(expr) = fragment.entry.body.last() else {
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
    let aug_stmt = crate::passes::InstrRuff::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &aug_stmt, &name_gen, &mut out, None)
        .expect("pow augassign lowering should succeed");

    let fragment = out.finish();
    let Some(InstrWithAwaitAndYield::Store(assign)) = fragment.entry.body.last() else {
        panic!("expected final store expr stmt, got {fragment:?}");
    };
    let rendered = format!("{:?}", assign.value);

    assert!(rendered.contains("InplacePow"), "{rendered}");
}
