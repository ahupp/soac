use crate::block_py::{InstrWithAwaitAndYield, NameLike};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::passes::InstrRuff;
use crate::py_expr;

#[test]
fn named_expr_lowering_emits_blockpy_assign_directly() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let _lowered = lower_expr_into_with_setup(
        InstrRuff::from_ast_expr(py_expr!("(x := y)")),
        &mut out,
        None,
    )
    .expect("expr lowering should succeed");

    let fragment = out.finish();
    let [InstrWithAwaitAndYield::Store(assign)] = &fragment.entry.body[..] else {
        panic!("expected one direct store expr stmt, got {fragment:?}");
    };
    assert_eq!(assign.name.id_str(), "x");
}
