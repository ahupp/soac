use super::assign_stmt::{bind_temp, lower_target_object_with_setup};
use super::*;
use crate::block_py::{Del, Meta, StructuredInstr, WithMeta};

fn lower_delete_target_into<E>(
    context: &Context,
    target: Expr,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    match target {
        Expr::Subscript(ast::ExprSubscript {
            value,
            slice,
            range,
            node_index,
            ..
        }) => {
            let object_value =
                lower_target_object_with_setup(*value, out, loop_ctx, next_label_id)?;
            let object_temp = bind_temp(out, context.fresh("delete_obj"), object_value);
            let index_value =
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    *slice,
                    out,
                    loop_ctx,
                    next_label_id,
                )?;
            let index_temp = bind_temp(out, context.fresh("delete_index"), index_value);
            out.push_stmt(StructuredInstr::Expr(E::del_item(
                node_index,
                range,
                object_temp,
                index_temp,
            )));
            Ok(())
        }
        Expr::Attribute(ast::ExprAttribute {
            value,
            attr,
            range,
            node_index,
            ..
        }) => {
            let object_value =
                lower_target_object_with_setup(*value, out, loop_ctx, next_label_id)?;
            let object_temp = bind_temp(out, context.fresh("delete_obj"), object_value);
            let attr_expr: E = E::from_lowered_expr(Expr::from(py_expr!(
                "{attr:literal}",
                attr = attr.as_str()
            )));
            out.push_stmt(StructuredInstr::Expr(E::helper_call(
                node_index,
                range,
                "delattr",
                vec![object_temp, attr_expr],
            )));
            Ok(())
        }
        Expr::Name(target) => {
            let meta = Meta::new(target.node_index.clone(), target.range);
            out.push_stmt(StructuredInstr::Expr(
                Del::new(target, false).with_meta(meta).into(),
            ));
            Ok(())
        }
        other => Err(assign_delete_error(
            "unsupported delete target reached BlockPy conversion",
            &Stmt::Delete(ast::StmtDelete {
                targets: vec![other].into(),
                range: Default::default(),
                node_index: Default::default(),
            }),
        )),
    }
}

impl StmtLowerer for ast::StmtDelete {
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
        for target in self.targets.iter().cloned() {
            lower_delete_target_into(context, target, out, loop_ctx, next_label_id)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test;
