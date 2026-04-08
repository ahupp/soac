use crate::block_py::{InstrWithAwaitAndYield, NameLike};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::py_expr;

#[test]
fn if_expr_lowering_emits_blockpy_setup_directly() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let lowered = lower_expr_into_with_setup(
        crate::passes::ast_to_instr::from_ast_expr(py_expr!("a if cond else b")),
        &mut out,
        None,
    )
    .expect("expr lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(
        &lowered,
        InstrWithAwaitAndYield::Load(load) if load.name.id_str().starts_with("_dp_tmp_")
    ));
    assert!(fragment.entry.body.is_empty(), "{fragment:?}");
    assert_eq!(fragment.deps.len(), 4, "{fragment:?}");
    assert!(fragment.deps.iter().any(|block| {
        block
            .body
            .iter()
            .any(|stmt| matches!(stmt, InstrWithAwaitAndYield::Store(_)))
    }));
}
