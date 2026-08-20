use crate::block_py::{CallArgPositional, InstrWithAwaitAndYield, NameLike, UnresolvedName};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::template::py_expr;
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
    let InstrWithAwaitAndYield::TakeOperand(callee) = call.func.as_ref() else {
        panic!("callee evaluated before argument control flow must be consumed once");
    };
    assert!(fragment.entry.body.iter().any(|instr| {
        matches!(instr, InstrWithAwaitAndYield::Store(store)
            if matches!((&store.name, &callee.name),
                (UnresolvedName::SourceName(left), UnresolvedName::SourceName(right))
                    if left.as_str() == right.as_str())
                && matches!(store.lifetime, crate::block_py::StoreLifetime::Operand { .. })
                && matches!(store.value.as_ref(), InstrWithAwaitAndYield::Load(load) if load.name.id_str() == "f"))
    }), "the exact callable must be acquired before the argument branch");
    assert!(matches!(
        call.args.first(),
        Some(CallArgPositional::Positional(InstrWithAwaitAndYield::Load(load)))
            if load.name.id_str().starts_with("_dp_target_")
    ));
}

#[test]
fn helper_spelling_and_literal_id_do_not_authorize_function_construction() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let lowered = lower_expr_into_with_setup(
        crate::passes::ast_to_instr::from_ast_expr(py_expr!(
            "__soac__.make_function(7, \"function\", (), (), None)"
        )),
        &mut out,
        None,
    )
    .expect("expr lowering should succeed");

    assert!(
        out.finish().entry.body.is_empty(),
        "ordinary helper call should not need setup"
    );
    assert!(matches!(lowered, InstrWithAwaitAndYield::Call(_)));
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

#[test]
fn ruff_setup_evaluates_callee_and_earlier_argument_before_later_branch() {
    use crate::block_py::BlockTerm;
    use std::collections::HashSet;

    fn direct_call_name(expr: &InstrWithAwaitAndYield) -> Option<&str> {
        let InstrWithAwaitAndYield::Call(call) = expr else {
            return None;
        };
        let InstrWithAwaitAndYield::Load(load) = call.func.as_ref() else {
            return None;
        };
        Some(load.name.id_str())
    }

    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_expr_into_with_setup(
        crate::passes::ast_to_instr::from_ast_expr(py_expr!(
            "make_callee()(first(), branch_left() if predicate() else branch_right())"
        )),
        &mut out,
        None,
    )
    .expect("source expression lowers");
    let fragment = out.finish();
    let mut block = &fragment.entry;
    let mut visited = HashSet::new();
    let mut evaluations = Vec::new();
    loop {
        assert!(
            visited.insert(block.label),
            "prefix must reach its source branch"
        );
        for statement in &block.body {
            if let InstrWithAwaitAndYield::Store(store) = statement {
                if let Some(name) = direct_call_name(&store.value) {
                    evaluations.push(name.to_owned());
                }
            }
        }
        match &block.term {
            BlockTerm::Jump(edge) => {
                block = fragment
                    .deps
                    .iter()
                    .find(|candidate| candidate.label == edge.target)
                    .expect("prefix jump targets a represented source block");
            }
            BlockTerm::IfTerm(branch) => {
                evaluations.push(
                    direct_call_name(&branch.test)
                        .expect("the source predicate remains the branch test")
                        .to_owned(),
                );
                break;
            }
            term => panic!("unexpected expression prefix terminator: {term:?}"),
        }
    }
    assert_eq!(evaluations, vec!["make_callee", "first", "predicate"]);
}
