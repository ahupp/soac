use crate::block_py::{BlockPyStmtBuilder, InstrWithAwaitAndYield, StructuredInstr};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::py_expr;

#[test]
fn boolop_lowering_emits_blockpy_setup_directly() {
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    let lowered =
        lower_expr_into_with_setup(py_expr!("a and b"), &mut out, None, &mut next_label_id)
            .expect("expr lowering should succeed");

    let fragment = out.finish();
    let rendered = format!("{lowered:?}");
    assert!(rendered.contains("_dp_target_"), "{rendered}");
    assert!(
        fragment.body.iter().any(|stmt| matches!(
            stmt,
            StructuredInstr::Expr(InstrWithAwaitAndYield::Store(_))
        )),
        "{fragment:?}"
    );
    assert!(
        fragment
            .body
            .iter()
            .any(|stmt| matches!(stmt, StructuredInstr::If(_))),
        "{fragment:?}"
    );
}

#[test]
fn compare_lowering_keeps_native_compare_expr() {
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    let lowered = lower_expr_into_with_setup(py_expr!("a < b"), &mut out, None, &mut next_label_id)
        .expect("expr lowering should succeed");

    assert!(
        out.finish().body.is_empty(),
        "single comparison should not need setup statements"
    );
    let rendered = format!("{lowered:?}");
    assert!(rendered.contains("Lt"), "{rendered}");
}
