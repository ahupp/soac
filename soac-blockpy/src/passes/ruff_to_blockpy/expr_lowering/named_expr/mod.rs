use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{HasMeta, Meta, Store, WithMeta};
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::LoopContext;
use crate::passes::InstrRuff;
use ruff_python_ast::{self as ast};

fn into_store_name(name: ast::name::Name) -> ast::name::Name {
    name
}

fn into_load_name(name: ast::ExprName) -> InstrRuff {
    InstrRuff::from_ast_expr(ast::Expr::Name(ast::ExprName {
        id: name.id,
        ctx: ast::ExprContext::Load,
        range: name.range,
        node_index: name.node_index,
    }))
}

pub(super) fn lower_named_expr_into<L, E>(
    lowerer: &L,
    named_expr: crate::block_py::ExprNamed<InstrRuff>,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let crate::block_py::ExprNamed { target, value, .. } = named_expr;
    let InstrRuff::ExprName(target_name) = *target else {
        return Err("named expression lowering expected a name target".to_string());
    };
    let value = E::from_lowered_expr(lowerer.lower_expr_instr_into(*value, out, loop_ctx)?);
    let target_meta = target_name.meta();
    let load_target = ast::ExprName {
        id: target_name.id.clone(),
        ctx: target_name.ctx,
        range: target_meta.range,
        node_index: target_meta.node_index.clone(),
    };
    let target_name = into_store_name(target_name.id);
    let meta = Meta::new(target_meta.node_index, target_meta.range);
    out.push_stmt(Store::new(target_name, value).with_meta(meta).into());
    Ok(into_load_name(load_target))
}

#[cfg(test)]
mod test;
