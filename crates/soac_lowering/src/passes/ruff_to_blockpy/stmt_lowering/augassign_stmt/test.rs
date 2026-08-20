use super::super::{lower_instr_for_test, BlockPyStmtBuilder};
use super::*;
use crate::block_py::{BinOpKind, InstrWithAwaitAndYield, NameLike, StoreLifetime};
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
    let aug_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &aug_stmt, &name_gen, &mut out, None)
        .expect("augassign lowering should succeed");

    let fragment = out.finish();
    let [InstrWithAwaitAndYield::Store(owner), InstrWithAwaitAndYield::Store(key), InstrWithAwaitAndYield::Store(old), InstrWithAwaitAndYield::Store(result), InstrWithAwaitAndYield::SetItem(setitem), InstrWithAwaitAndYield::Del(drop_key), InstrWithAwaitAndYield::Del(drop_owner), InstrWithAwaitAndYield::Del(drop_result)] =
        fragment.entry.body.as_slice()
    else {
        panic!("expected explicit operator and target lifetimes, got {fragment:?}");
    };
    assert!(matches!(
        result.value.as_ref(),
        InstrWithAwaitAndYield::BinOp(op) if op.kind == BinOpKind::InplaceAdd
            && matches!(op.left.as_ref(), InstrWithAwaitAndYield::TakeOperand(take)
                if take.name.id_str() == old.name.id_str())
    ));
    assert!(
        matches!(setitem.replacement.as_ref(), InstrWithAwaitAndYield::Load(load)
        if load.name.id_str() == result.name.id_str())
    );
    assert_eq!(drop_key.name.id_str(), key.name.id_str());
    assert_eq!(drop_owner.name.id_str(), owner.name.id_str());
    assert_eq!(drop_result.name.id_str(), result.name.id_str());
    let ranks = [result, owner, key, old].map(|store| {
        let StoreLifetime::Operand { unwind_order } = store.lifetime else {
            panic!("augmented assignment needs operand lifetimes")
        };
        unwind_order
    });
    assert!(
        ranks.windows(2).all(|pair| pair[0] < pair[1]),
        "setter error order follows stack position, not Store execution order"
    );
}

#[test]
fn stmt_attr_augassign_releases_old_value_before_setter_then_owner_before_result() {
    let stmt = crate::passes::ast_to_instr::from_ast_stmt(py_stmt!("obj.attr += y"));
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &stmt, &name_gen, &mut out, None)
        .expect("attribute augassign lowering should succeed");

    let fragment = out.finish();
    let [InstrWithAwaitAndYield::Store(owner), InstrWithAwaitAndYield::Store(old), InstrWithAwaitAndYield::Store(result), InstrWithAwaitAndYield::SetAttr(setattr), InstrWithAwaitAndYield::Del(drop_owner), InstrWithAwaitAndYield::Del(drop_result)] =
        fragment.entry.body.as_slice()
    else {
        panic!("expected explicit attribute operand lifetimes, got {fragment:?}");
    };
    assert!(
        matches!(setattr.replacement.as_ref(), InstrWithAwaitAndYield::Load(load)
        if load.name.id_str() == result.name.id_str())
    );
    assert!(
        matches!(result.value.as_ref(), InstrWithAwaitAndYield::BinOp(op)
        if matches!(op.left.as_ref(), InstrWithAwaitAndYield::TakeOperand(take)
            if take.name.id_str() == old.name.id_str()))
    );
    assert_eq!(drop_owner.name.id_str(), owner.name.id_str());
    assert_eq!(drop_result.name.id_str(), result.name.id_str());
    let ranks = [result, owner, old].map(|store| {
        let StoreLifetime::Operand { unwind_order } = store.lifetime else {
            panic!("augmented assignment needs operand lifetimes")
        };
        unwind_order
    });
    assert!(ranks.windows(2).all(|pair| pair[0] < pair[1]));
}

#[test]
fn stmt_pow_augassign_to_blockpy_uses_inplace_pow() {
    let stmt = py_stmt!("x **= y");
    let aug_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &aug_stmt, &name_gen, &mut out, None)
        .expect("pow augassign lowering should succeed");

    let fragment = out.finish();
    let [InstrWithAwaitAndYield::Store(old), InstrWithAwaitAndYield::Store(result), InstrWithAwaitAndYield::Store(assign), InstrWithAwaitAndYield::Del(drop_result)] =
        fragment.entry.body.as_slice()
    else {
        panic!("expected explicit name operand lifetimes, got {fragment:?}");
    };
    assert!(matches!(
        result.value.as_ref(),
        InstrWithAwaitAndYield::BinOp(op) if op.kind == BinOpKind::InplacePow
            && matches!(op.left.as_ref(), InstrWithAwaitAndYield::TakeOperand(take)
                if take.name.id_str() == old.name.id_str())
    ));
    assert_eq!(assign.name.id_str(), "x");
    assert_eq!(assign.lifetime, StoreLifetime::Frame);
    assert!(
        matches!(assign.value.as_ref(), InstrWithAwaitAndYield::Load(load)
        if load.name.id_str() == result.name.id_str())
    );
    assert_eq!(drop_result.name.id_str(), result.name.id_str());
}
