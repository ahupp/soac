use super::assign_stmt::{bind_temp, lower_target_object_with_setup};
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
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    match stmt.target.as_ref() {
        InstrRuff::ExprName(target) => {
            let target_meta = stmt.target.meta();
            let mut load_name = target.clone();
            load_name.ctx = ast::ExprContext::Load;
            let current_value = bind_temp(
                out,
                context.fresh("augassign_value"),
                E::from_lowered_expr(InstrRuff::ExprName(load_name)),
            );
            let rhs = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                (*stmt.value).clone(),
                out,
                loop_ctx,
            )?;

            out.push_stmt(
                Store::new(
                    target.id.clone(),
                    E::lower_augassign_value(
                        target_meta.node_index.clone(),
                        target_meta.range,
                        stmt.op,
                        current_value,
                        rhs,
                    ),
                )
                .with_meta(Meta::new(target_meta.node_index, target_meta.range))
                .into(),
            );
            Ok(())
        }
        InstrRuff::ExprAttribute(target) => {
            let target_meta = target.meta();
            let object_value =
                lower_target_object_with_setup((*target.value).clone(), out, loop_ctx)?;
            let object_temp = bind_temp(out, context.fresh("augassign_obj"), object_value);
            let current_value = bind_temp(
                out,
                context.fresh("augassign_value"),
                E::get_attr(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    object_temp.clone(),
                    target.attr.to_string(),
                ),
            );
            let rhs = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                (*stmt.value).clone(),
                out,
                loop_ctx,
            )?;

            out.push_stmt(E::set_attr(
                target_meta.node_index.clone(),
                target_meta.range,
                object_temp,
                target.attr.to_string(),
                E::lower_augassign_value(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    stmt.op,
                    current_value,
                    rhs,
                ),
            ));
            Ok(())
        }
        InstrRuff::ExprSubscript(target) => {
            let target_meta = target.meta();
            let object_value =
                lower_target_object_with_setup((*target.value).clone(), out, loop_ctx)?;
            let object_temp = bind_temp(out, context.fresh("augassign_obj"), object_value);
            let index_value =
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    (*target.slice).clone(),
                    out,
                    loop_ctx,
                )?;
            let index_temp = bind_temp(out, context.fresh("augassign_index"), index_value);
            let current_value = bind_temp(
                out,
                context.fresh("augassign_value"),
                E::get_item(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    object_temp.clone(),
                    index_temp.clone(),
                ),
            );
            let rhs = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                (*stmt.value).clone(),
                out,
                loop_ctx,
            )?;

            out.push_stmt(E::set_item(
                target_meta.node_index.clone(),
                target_meta.range,
                object_temp,
                index_temp,
                E::lower_augassign_value(
                    target_meta.node_index.clone(),
                    target_meta.range,
                    stmt.op,
                    current_value,
                    rhs,
                ),
            ));
            Ok(())
        }
        _ => Err(assign_delete_error(
            "unsupported augmented assignment target reached BlockPy conversion",
            &InstrRuff::StmtAugAssign(stmt.clone()).into_ast_stmt(),
        )),
    }
}

#[cfg(test)]
mod test;
