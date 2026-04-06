use crate::passes::InstrRuff;
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::block_py::InstrWithAwaitAndYield;
use crate::py_expr;

#[test]
fn boolop_lowering_emits_blockpy_setup_directly() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let lowered = lower_expr_into_with_setup(
        InstrRuff::from_ast_expr(py_expr!("a and b")),
        &mut out,
        None,
    )
    .expect("expr lowering should succeed");

    let fragment = out.finish();
    let rendered = format!("{lowered:?}");
    assert!(rendered.contains("_dp_target_"), "{rendered}");
    assert!(
        fragment
            .deps
            .iter()
            .flat_map(|block| block.body.iter())
            .any(|stmt| matches!(stmt, InstrWithAwaitAndYield::Store(_))),
        "{fragment:?}"
    );
    assert!(!fragment.deps.is_empty(), "{fragment:?}");
}

#[test]
fn compare_lowering_keeps_native_compare_expr() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let lowered = lower_expr_into_with_setup(
        InstrRuff::from_ast_expr(py_expr!("a < b")),
        &mut out,
        None,
    )
    .expect("expr lowering should succeed");

    assert!(
        out.finish().entry.body.is_empty(),
        "single comparison should not need setup statements"
    );
    let rendered = format!("{lowered:?}");
    assert!(rendered.contains("Lt"), "{rendered}");
}
