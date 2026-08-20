use super::assign_stmt::{
    bind_temp, bind_temp_with_unwind_order, delete_temp, lower_target_object_with_setup, take_temp,
};
use super::*;
use crate::block_py::{HasMeta, Meta, Store, WithMeta};
use crate::passes::InstrRuff;

pub(crate) fn lower_augassign_instr_into<E>(
    context: &Context,
    stmt: &crate::block_py::StmtAugAssign<InstrRuff>,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    // The target-store stack is [result, receiver, key], even though the
    // result is computed last. Reserve its unwind position before evaluating
    // the target so setter errors release key/receiver before the result.
    let result_order = out.name_gen().next_temporary_sequence();
    let result_name = context.fresh("augassign_result");
    match stmt.target.as_ref() {
        InstrRuff::ExprName(target) => {
            let target_meta = stmt.target.meta();
            let mut load_name = target.clone();
            load_name.ctx = ast::ExprContext::Load;
            let current_name = context.fresh("augassign_value");
            bind_temp(
                out,
                current_name.clone(),
                E::from_lowered_expr(InstrRuff::ExprName(load_name)),
            );
            let rhs = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
                context,
                (*stmt.value).clone(),
                out,
                loop_ctx,
            )?;

            let result = bind_temp_with_unwind_order(
                out,
                result_name.clone(),
                E::lower_augassign_value(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    stmt.op,
                    take_temp(&current_name),
                    rhs,
                ),
                result_order,
            );
            out.push_stmt(
                Store::new(target.id.clone(), result)
                    .with_meta(Meta::new(target_meta.node_index, target_meta.range))
                    .into(),
            );
            delete_temp(out, result_name);
            Ok(())
        }
        InstrRuff::ExprAttribute(target) => {
            let target_meta = target.meta();
            let object_value =
                lower_target_object_with_setup(context, (*target.value).clone(), out, loop_ctx)?;
            let object_name = context.fresh("augassign_obj");
            let object_temp = bind_temp(out, object_name.clone(), object_value);
            let current_name = context.fresh("augassign_value");
            bind_temp(
                out,
                current_name.clone(),
                E::get_attr(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    object_temp.clone(),
                    target.attr.to_string(),
                ),
            );
            let rhs = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
                context,
                (*stmt.value).clone(),
                out,
                loop_ctx,
            )?;

            let result = bind_temp_with_unwind_order(
                out,
                result_name.clone(),
                E::lower_augassign_value(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    stmt.op,
                    take_temp(&current_name),
                    rhs,
                ),
                result_order,
            );
            // The operator consumes the captured old value. Its finalizer
            // runs after the operator, before invoking the setter, even when
            // the right operand suspended and moved it into preserved state.
            out.push_stmt(E::set_attr(
                target_meta.node_index.clone(),
                target_meta.range,
                object_temp,
                target.attr.to_string(),
                result,
            ));
            delete_temp(out, object_name);
            delete_temp(out, result_name);
            Ok(())
        }
        InstrRuff::ExprSubscript(target) => {
            let target_meta = target.meta();
            let object_value =
                lower_target_object_with_setup(context, (*target.value).clone(), out, loop_ctx)?;
            let object_name = context.fresh("augassign_obj");
            let object_temp = bind_temp(out, object_name.clone(), object_value);
            let index_value =
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
                    context,
                    (*target.slice).clone(),
                    out,
                    loop_ctx,
                )?;
            let index_name = context.fresh("augassign_index");
            let index_temp = bind_temp(out, index_name.clone(), index_value);
            let current_name = context.fresh("augassign_value");
            bind_temp(
                out,
                current_name.clone(),
                E::get_item(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    object_temp.clone(),
                    index_temp.clone(),
                ),
            );
            let rhs = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
                context,
                (*stmt.value).clone(),
                out,
                loop_ctx,
            )?;

            let result = bind_temp_with_unwind_order(
                out,
                result_name.clone(),
                E::lower_augassign_value(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    stmt.op,
                    take_temp(&current_name),
                    rhs,
                ),
                result_order,
            );
            out.push_stmt(E::set_item(
                target_meta.node_index.clone(),
                target_meta.range,
                object_temp,
                index_temp,
                result,
            ));
            delete_temp(out, index_name);
            delete_temp(out, object_name);
            delete_temp(out, result_name);
            Ok(())
        }
        _ => Err(assign_delete_error(
            "unsupported augmented assignment target reached BlockPy conversion",
            &crate::passes::ast_to_instr::into_ast_stmt(InstrRuff::StmtAugAssign(stmt.clone())),
        )),
    }
}

#[cfg(test)]
mod test;
