use crate::block_py::{BuildCollectionKind, InstrWithAwaitAndYield, StoreLifetime, UnresolvedName};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::template::py_expr;

fn source_name(name: &UnresolvedName) -> &str {
    let UnresolvedName::SourceName(name) = name else {
        panic!("an expression scratch must have a source-name payload, not a runtime name");
    };
    name.as_str()
}

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
    let InstrWithAwaitAndYield::TakeOperand(take) = &lowered else {
        panic!("the selected branch's result must move to its consumer");
    };
    let target = source_name(&take.name);
    let stores = std::iter::once(&fragment.entry)
        .chain(&fragment.deps)
        .flat_map(|block| &block.body)
        .filter_map(|stmt| match stmt {
            InstrWithAwaitAndYield::Store(store) if source_name(&store.name) == target => {
                Some(store)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        stores.len(),
        2,
        "each branch publishes the same result owner"
    );
    let StoreLifetime::Operand { unwind_order } = stores[0].lifetime else {
        panic!("the result is an expression operand, not a frame local");
    };
    assert!(
        matches!(stores[1].lifetime, StoreLifetime::Operand { unwind_order: other } if other == unwind_order)
    );
    let mut branch_values = stores
        .iter()
        .map(|store| match store.value.as_ref() {
            InstrWithAwaitAndYield::Load(load) => source_name(&load.name),
            _ => panic!("the original branch expression must remain a source read"),
        })
        .collect::<Vec<_>>();
    branch_values.sort_unstable();
    assert_eq!(branch_values, ["a", "b"]);
}

#[test]
fn if_expr_call_argument_result_moves_into_singleton_keyword_buffer() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let lowered = lower_expr_into_with_setup(
        crate::passes::ast_to_instr::from_ast_expr(py_expr!(
            "callee()(*source(), tail=value() if predicate() else None)"
        )),
        &mut out,
        None,
    )
    .expect("actual conditional and expanded-call producers should compose");
    let fragment = out.finish();
    let InstrWithAwaitAndYield::PreparedCall(call) = &lowered else {
        panic!("child CFG selects the complete source-shaped call phases");
    };
    let Some(InstrWithAwaitAndYield::TakeOperand(keywords)) = call.keywords.as_deref() else {
        panic!("the prepared call must consume its keyword buffer");
    };
    let stores = std::iter::once(&fragment.entry)
        .chain(&fragment.deps)
        .flat_map(|block| &block.body)
        .filter_map(|stmt| match stmt {
            InstrWithAwaitAndYield::Store(store) => Some(store),
            _ => None,
        })
        .collect::<Vec<_>>();
    let keyword_store = stores
        .iter()
        .copied()
        .find(|store| source_name(&store.name) == source_name(&keywords.name))
        .expect("keyword buffer acquisition");
    let InstrWithAwaitAndYield::BuildCollection(dictionary) = keyword_store.value.as_ref() else {
        panic!("the contiguous named group must build its native dictionary");
    };
    assert_eq!(dictionary.kind, BuildCollectionKind::Dict);
    let [_, InstrWithAwaitAndYield::TakeOperand(value)] = dictionary.values.as_slice() else {
        panic!("one named argument is an owned key/value pair");
    };
    let value_store = stores
        .iter()
        .copied()
        .find(|store| source_name(&store.name) == source_name(&value.name))
        .expect("call-argument acquisition");
    let InstrWithAwaitAndYield::TakeOperand(result) = value_store.value.as_ref() else {
        panic!("acquiring a call argument must not leave a second conditional-result owner");
    };
    let result_stores = stores
        .iter()
        .copied()
        .filter(|store| source_name(&store.name) == source_name(&result.name))
        .collect::<Vec<_>>();
    assert_eq!(result_stores.len(), 2);
    let StoreLifetime::Operand {
        unwind_order: result_order,
    } = result_stores[0].lifetime
    else {
        panic!("the conditional join must own a movable operand");
    };
    assert!(
        matches!(result_stores[1].lifetime, StoreLifetime::Operand { unwind_order } if unwind_order == result_order)
    );
    let StoreLifetime::Operand {
        unwind_order: argument_order,
    } = value_store.lifetime
    else {
        panic!("the call argument must retain its explicit operand role");
    };
    assert!(result_order < argument_order);
}
