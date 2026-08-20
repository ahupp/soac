use crate::block_py::{HasMeta, InstrWithAwaitAndYield, NameLike, StoreLifetime};
use crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::test_name_gen;
use crate::passes::InstrRuff;
use crate::template::py_expr;

#[test]
fn named_expr_keeps_its_value_instead_of_reloading_the_source_target() {
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    let expression = crate::passes::ast_to_instr::from_ast_expr(py_expr!("(x := y)"));
    let InstrRuff::ExprNamed(named) = &expression else {
        panic!("named expression fixture");
    };
    let target_meta = named.target.meta();
    let lowered = lower_expr_into_with_setup(expression, &mut out, None)
        .expect("expr lowering should succeed");

    let fragment = out.finish();
    let [InstrWithAwaitAndYield::Store(value), InstrWithAwaitAndYield::Store(target)] =
        &fragment.entry.body[..]
    else {
        panic!("expected a value owner followed by the original source store");
    };
    assert_ne!(value.name.id_str(), "x");
    assert!(matches!(value.lifetime, StoreLifetime::Operand { .. }));
    let InstrWithAwaitAndYield::Load(original) = value.value.as_ref() else {
        panic!("evaluate the original value exactly once");
    };
    assert_eq!(original.name.id_str(), "y");
    assert_eq!(target.name.id_str(), "x");
    assert_eq!(target.meta().range, target_meta.range);
    let InstrWithAwaitAndYield::Load(copy) = target.value.as_ref() else {
        panic!("assign from the saved expression value");
    };
    assert_eq!(copy.name.id_str(), value.name.id_str());
    assert_eq!(copy.name.runtime_name_id(), value.name.runtime_name_id());
    let InstrWithAwaitAndYield::TakeOperand(result) = lowered else {
        panic!("consume the expression owner without reloading the source target");
    };
    assert_eq!(result.name.id_str(), value.name.id_str());
    assert_eq!(result.name.runtime_name_id(), value.name.runtime_name_id());
}
