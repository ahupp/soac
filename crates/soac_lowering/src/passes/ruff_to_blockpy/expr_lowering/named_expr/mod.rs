use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{HasMeta, Meta, Store, StoreLifetime, TakeOperand, WithMeta};
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::LoopContext;
use crate::passes::InstrRuff;

pub(super) fn lower_named_expr_into<L, E>(
    lowerer: &L,
    named_expr: crate::block_py::ExprNamed<InstrRuff>,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprNamed { target, value, .. } = named_expr;
    let InstrRuff::ExprName(target_name) = *target else {
        return Err("named expression lowering expected a name target".to_string());
    };
    let value = lowerer.lower_expr_instr_into(*value, out, loop_ctx)?;
    let value_meta = value.meta();
    let binding = lowerer.fresh_operand_binding();
    let unwind_order = out.name_gen().next_temporary_sequence();
    // A named expression returns its original value, not a new lookup of the
    // target after assignment. Preserve the native COPY/STORE ownership: a
    // custom class namespace may change the stored value, and a class-cell
    // assignment has only a source STORE receipt, never a source LOAD receipt.
    out.push_stmt(
        Store::new(binding.clone(), E::from_lowered_expr(value))
            .with_lifetime(StoreLifetime::Operand { unwind_order })
            .with_meta(value_meta)
            .into(),
    );
    let target_meta = target_name.meta();
    out.push_stmt(
        Store::new(
            target_name.id,
            E::from_lowered_expr(super::load_name(&binding)),
        )
        .with_meta(target_meta)
        .into(),
    );
    Ok(TakeOperand::new(binding)
        .with_meta(Meta::synthetic())
        .into())
}

#[cfg(test)]
mod test;
