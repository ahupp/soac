use super::assign_stmt::{bind_temp, lower_target_object_with_setup};
use super::*;
use crate::block_py::{HasMeta, Meta, Store, StructuredInstr, WithMeta};

impl StmtLowerer for ast::StmtAugAssign {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(self)
    }

    fn to_blockpy<E>(
        &self,
        context: &Context,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        let stmt = Stmt::AugAssign(self.clone());

        match &*self.target {
            Expr::Name(target) => {
                let target_meta = self.target.meta();
                let mut load_name = target.clone();
                load_name.ctx = ast::ExprContext::Load;
                let current_value = bind_temp(
                    out,
                    context.fresh("augassign_value"),
                    E::from_lowered_expr(Expr::Name(load_name)),
                );
                let rhs =
                    crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                        (*self.value).clone(),
                        out,
                        loop_ctx,
                        next_label_id,
                    )?;

                out.push_stmt(StructuredInstr::Expr(
                    Store::new(
                        target.clone(),
                        E::lower_augassign_value(
                            target_meta.node_index.clone(),
                            target_meta.range,
                            self.op,
                            current_value,
                            rhs,
                        ),
                    )
                    .with_meta(Meta::new(target_meta.node_index, target_meta.range))
                    .into(),
                ));
                Ok(())
            }
            Expr::Attribute(target) => {
                let object_value = lower_target_object_with_setup(
                    (*target.value).clone(),
                    out,
                    loop_ctx,
                    next_label_id,
                )?;
                let object_temp = bind_temp(out, context.fresh("augassign_obj"), object_value);
                let current_value = bind_temp(
                    out,
                    context.fresh("augassign_value"),
                    E::get_attr(
                        target.node_index.clone(),
                        target.range,
                        object_temp.clone(),
                        target.attr.to_string(),
                    ),
                );
                let rhs =
                    crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                        (*self.value).clone(),
                        out,
                        loop_ctx,
                        next_label_id,
                    )?;

                out.push_stmt(StructuredInstr::Expr(E::set_attr(
                    target.node_index.clone(),
                    target.range,
                    object_temp,
                    target.attr.to_string(),
                    E::lower_augassign_value(
                        target.node_index.clone(),
                        target.range,
                        self.op,
                        current_value,
                        rhs,
                    ),
                )));
                Ok(())
            }
            Expr::Subscript(target) => {
                let object_value = lower_target_object_with_setup(
                    (*target.value).clone(),
                    out,
                    loop_ctx,
                    next_label_id,
                )?;
                let object_temp = bind_temp(out, context.fresh("augassign_obj"), object_value);
                let index_value =
                    crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                        (*target.slice).clone(),
                        out,
                        loop_ctx,
                        next_label_id,
                    )?;
                let index_temp = bind_temp(out, context.fresh("augassign_index"), index_value);
                let current_value = bind_temp(
                    out,
                    context.fresh("augassign_value"),
                    E::get_item(
                        target.node_index.clone(),
                        target.range,
                        object_temp.clone(),
                        index_temp.clone(),
                    ),
                );
                let rhs =
                    crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                        (*self.value).clone(),
                        out,
                        loop_ctx,
                        next_label_id,
                    )?;

                out.push_stmt(StructuredInstr::Expr(E::set_item(
                    target.node_index.clone(),
                    target.range,
                    object_temp,
                    index_temp,
                    E::lower_augassign_value(
                        target.node_index.clone(),
                        target.range,
                        self.op,
                        current_value,
                        rhs,
                    ),
                )));
                Ok(())
            }
            _ => Err(assign_delete_error(
                "unsupported augmented assignment target reached BlockPy conversion",
                &stmt,
            )),
        }
    }
}

#[cfg(test)]
mod test;
