use super::assign_stmt::{bind_temp, lower_target_object_with_setup};
use super::*;
use crate::block_py::{Del, HasMeta, WithMeta};
use crate::passes::InstrRuff;

fn lower_delete_target_into<E>(
    context: &Context,
    target: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    match target {
        InstrRuff::ExprSubscript(target) => {
            let meta = target.meta();
            let object_value = lower_target_object_with_setup(*target.value, out, loop_ctx)?;
            let object_temp = bind_temp(out, context.fresh("delete_obj"), object_value);
            let index_value =
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    *target.slice,
                    out,
                    loop_ctx,
                )?;
            let index_temp = bind_temp(out, context.fresh("delete_index"), index_value);
            out.push_stmt(E::del_item(
                meta.node_index,
                meta.range,
                object_temp,
                index_temp,
            ));
            Ok(())
        }
        InstrRuff::ExprAttribute(target) => {
            let target_meta = target.meta();
            let object_value = lower_target_object_with_setup(*target.value, out, loop_ctx)?;
            let object_temp = bind_temp(out, context.fresh("delete_obj"), object_value);
            let attr_expr: E = E::from_lowered_expr(InstrRuff::from_ast_expr(Expr::from(py_expr!(
                "{attr:literal}",
                attr = target.attr.as_str()
            ))));
            out.push_stmt(E::helper_call(
                target_meta.node_index,
                target_meta.range,
                "delattr",
                vec![object_temp, attr_expr],
            ));
            Ok(())
        }
        InstrRuff::ExprName(target) => {
            let meta = target.meta();
            out.push_stmt(Del::new(target.id.clone(), false).with_meta(meta).into());
            Ok(())
        }
        other => Err(assign_delete_error(
            "unsupported delete target reached BlockPy conversion",
            &InstrRuff::StmtDelete(crate::block_py::StmtDelete::new(vec![other])).into_ast_stmt(),
        )),
    }
}

pub(crate) fn lower_delete_instr_into<E>(
    context: &Context,
    stmt: &crate::block_py::StmtDelete<InstrRuff>,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    for target in stmt.targets.iter().cloned() {
        lower_delete_target_into(context, target, out, loop_ctx)?;
    }
    Ok(())
}

#[cfg(test)]
mod test;
