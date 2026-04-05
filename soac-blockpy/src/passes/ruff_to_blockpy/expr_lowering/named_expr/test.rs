use crate::block_py::{
    BlockPyNameLike, BlockPyStmtBuilder, InstrWithAwaitAndYield, StructuredInstr,
};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::py_expr;

#[test]
fn named_expr_lowering_emits_blockpy_assign_directly() {
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    let _lowered =
        lower_expr_into_with_setup(py_expr!("(x := y)"), &mut out, None, &mut next_label_id)
            .expect("expr lowering should succeed");

    let fragment = out.finish();
    let [StructuredInstr::Expr(InstrWithAwaitAndYield::Store(assign))] =
        &fragment.body[..]
    else {
        panic!("expected one direct store expr stmt, got {fragment:?}");
    };
    assert_eq!(assign.name.id_str(), "x");
}
