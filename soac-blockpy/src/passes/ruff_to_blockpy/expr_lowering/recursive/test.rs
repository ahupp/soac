use crate::block_py::{CallArgPositional, InstrWithAwaitAndYield, NameLike};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::py_expr;
use ruff_python_parser::parse_expression;

#[test]
fn nested_boolop_in_call_argument_emits_setup_via_expr_lowering() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let lowered: InstrWithAwaitAndYield = lower_expr_into_with_setup(
        crate::passes::ast_to_instr::from_ast_expr(py_expr!("f(a and b)")),
        &mut out,
        None,
    )
    .expect("expr lowering should succeed");

    let fragment = out.finish();
    assert!(!fragment.deps.is_empty(), "{fragment:?}");
    let InstrWithAwaitAndYield::Call(call) = lowered else {
        panic!("expected lowered call");
    };
    assert!(
        matches!(call.func.as_ref(), InstrWithAwaitAndYield::Load(load) if load.name.id_str() == "f")
    );
    assert!(matches!(
        call.args.first(),
        Some(CallArgPositional::Positional(InstrWithAwaitAndYield::Load(load)))
            if load.name.id_str().starts_with("_dp_target_")
    ));
}

#[test]
fn direct_core_expr_lowering_materializes_make_function_operation() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let lowered = lower_expr_into_with_setup(
        crate::passes::ast_to_instr::from_ast_expr(py_expr!(
            "__soac__.make_function(7, \"function\", __soac__.tuple_values(), __soac__.tuple_values(), None)"
        )),
        &mut out,
        None,
    )
    .expect("expr lowering should succeed");

    assert!(
        out.finish().entry.body.is_empty(),
        "make_function should not need setup"
    );
    assert!(matches!(lowered, InstrWithAwaitAndYield::MakeFunction(_)));
}

#[test]
fn direct_core_expr_lowering_materializes_live_operation_helpers() {
    for source in [
        "__soac__.store_global(_dp_class_ns, \"caught\", value)",
        "__soac__.cell_ref(\"__class__\")",
    ] {
        let name_gen = test_name_gen();
        let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
        let lowered = lower_expr_into_with_setup(
            crate::passes::ast_to_instr::from_ast_expr(
                *parse_expression(source).unwrap().into_syntax().body,
            ),
            &mut out,
            None,
        )
        .expect("expr lowering should succeed");

        assert!(
            out.finish().entry.body.is_empty(),
            "{source} should not need setup"
        );
        match (source, lowered) {
            (
                "__soac__.store_global(_dp_class_ns, \"caught\", value)",
                InstrWithAwaitAndYield::Store(store),
            ) => {
                assert_eq!(store.name.id_str(), "caught");
                assert!(
                    matches!(store.value.as_ref(), InstrWithAwaitAndYield::Load(load) if load.name.id_str() == "value")
                );
            }
            ("__soac__.cell_ref(\"__class__\")", InstrWithAwaitAndYield::CellRefForName(cell)) => {
                assert_eq!(cell.logical_name, "__class__");
            }
            _ => panic!("unexpected lowered helper shape for {source}"),
        }
    }
}
