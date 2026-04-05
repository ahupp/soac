use crate::block_py::{BlockPyStmtBuilder, InstrWithAwaitAndYield, StructuredInstr};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::py_expr;
use ruff_python_parser::parse_expression;

#[test]
fn nested_boolop_in_call_argument_emits_setup_via_expr_lowering() {
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    let lowered: InstrWithAwaitAndYield =
        lower_expr_into_with_setup(py_expr!("f(a and b)"), &mut out, None, &mut next_label_id)
            .expect("expr lowering should succeed");

    let fragment = out.finish();
    assert!(
        fragment
            .body
            .iter()
            .any(|stmt| matches!(stmt, StructuredInstr::If(_))),
        "{fragment:?}"
    );
    let rendered = format!("{lowered:?}");
    assert!(rendered.starts_with("f(_dp_target_"), "{rendered}");
}

#[test]
fn direct_core_expr_lowering_materializes_make_function_operation() {
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
    let mut next_label_id = 0usize;

    let lowered = lower_expr_into_with_setup(
        py_expr!(
            "__soac__.make_function(7, \"function\", __soac__.tuple_values(), __soac__.tuple_values(), None)"
        ),
        &mut out,
        None,
        &mut next_label_id,
    )
    .expect("expr lowering should succeed");

    assert!(
        out.finish().body.is_empty(),
        "make_function should not need setup"
    );
    let rendered = format!("{lowered:?}");
    assert!(rendered.contains("MakeFunction("), "{rendered}");
    assert!(!rendered.contains("__dp_make_function("), "{rendered}");
}

#[test]
fn direct_core_expr_lowering_materializes_live_operation_helpers() {
    for (source, expected) in [
        (
            "__soac__.store_global(_dp_class_ns, \"caught\", value)",
            "StoreName(",
        ),
        ("__soac__.cell_ref(\"__class__\")", "CellRefForName("),
    ] {
        let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new();
        let mut next_label_id = 0usize;

        let lowered = lower_expr_into_with_setup(
            *parse_expression(source).unwrap().into_syntax().body,
            &mut out,
            None,
            &mut next_label_id,
        )
        .expect("expr lowering should succeed");

        assert!(
            out.finish().body.is_empty(),
            "{source} should not need setup"
        );
        let rendered = format!("{lowered:?}");
        assert!(rendered.contains(expected), "{rendered}");
        assert!(!rendered.contains("__soac__."), "{rendered}");
    }
}
