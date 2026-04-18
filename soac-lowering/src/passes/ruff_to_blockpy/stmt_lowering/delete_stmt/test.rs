use super::super::{lower_instr_for_test, BlockPyStmtBuilder};
use super::*;
use crate::block_py::InstrWithAwaitAndYield;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::test_name_gen;

#[test]
fn stmt_delete_to_blockpy_emits_direct_core_delitem() {
    let stmt = py_stmt!("del obj[idx]");
    let delete_stmt = crate::passes::ast_to_instr::from_ast_stmt(stmt);
    let context = Context::new("");
    let name_gen = test_name_gen();
    let mut out = BlockPyStmtBuilder::<InstrWithAwaitAndYield>::new(&name_gen);
    lower_instr_for_test(&context, &delete_stmt, &name_gen, &mut out, None)
        .expect("delete lowering should succeed");

    let fragment = out.finish();
    assert!(matches!(
        fragment.entry.body.last(),
        Some(InstrWithAwaitAndYield::DelItem(_))
    ));
}
