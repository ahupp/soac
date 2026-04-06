use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{
    BlockPyStmtBuilder, BlockTerm, Meta, Store, StructuredIf, StructuredInstr, TermIf, WithMeta,
};
use crate::passes::ruff_to_blockpy::expr_lowering::fresh_setup_name;
use crate::passes::ruff_to_blockpy::{InlineFragment, LoweredExpr, LoopContext};
use crate::py_expr;
use ruff_python_ast::{self as ast, Expr};

fn store_name(name: &str) -> ast::ExprName {
    ast::ExprName {
        id: name.into(),
        ctx: ast::ExprContext::Store,
        range: Default::default(),
        node_index: ast::AtomicNodeIndex::default(),
    }
}

fn load_name(name: &str) -> Expr {
    py_expr!("{name:id}", name = name)
}

fn assign_name<E>(target: &str, value: Expr) -> StructuredInstr<E>
where
    E: RuffToBlockPyExpr,
{
    let target = store_name(target);
    let meta = Meta::new(target.node_index.clone(), target.range);
    StructuredInstr::Expr(
        Store::new(target, Box::new(E::from_lowered_expr(value)))
            .with_meta(meta)
            .into(),
    )
}

#[allow(dead_code)]
pub(crate) fn try_lower_if_expr_direct<L, E>(
     lowerer: &L,
     name_gen: &crate::block_py::FunctionNameGen,
     if_expr: ast::ExprIf,
     loop_ctx: Option<&LoopContext>,
 ) -> Option<Result<LoweredExpr<E>, String>>
 where
     L: BlockPySetupExprLowerer + ?Sized,
     E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
 {
     let ast::ExprIf {
         test, body, orelse, ..
     } = if_expr;
     let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
     let Some(test_setup) = bridge.try_lower_inline_value::<E, Expr>(
         |structured, scratch_next_label_id| {
             lowerer.lower_expr_ast_into(*test.clone(), structured, loop_ctx, scratch_next_label_id)
         },
     )
     else {
         return None;
     };
     let (mut entry, test) = match test_setup {
         Ok(value) => value,
         Err(err) => return Some(Err(err)),
     };
 
     let target = fresh_setup_name("tmp");
     let Some(body_setup) = bridge.try_lower_inline_value::<E, Expr>(
         |structured, scratch_next_label_id| {
             let body_value = lowerer.lower_expr_ast_into(
                 *body.clone(),
                 structured,
                 loop_ctx,
                 scratch_next_label_id,
             )?;
             structured.push_stmt(assign_name(&target, body_value.clone()));
             Ok(body_value)
         },
     )
     else {
         return None;
     };
    let (body_entry, _) = match body_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    let body_fragment =
        InlineFragment::from_fallthrough_builder(name_gen.next_block_name(), body_entry, Vec::new());
 
     let Some(orelse_setup) = bridge.try_lower_inline_value::<E, Expr>(
         |structured, scratch_next_label_id| {
             let orelse_value = lowerer.lower_expr_ast_into(
                 *orelse.clone(),
                 structured,
                 loop_ctx,
                 scratch_next_label_id,
             )?;
             structured.push_stmt(assign_name(&target, orelse_value.clone()));
             Ok(orelse_value)
         },
     )
     else {
         return None;
     };
    let (orelse_entry, _) = match orelse_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    let orelse_fragment = InlineFragment::from_fallthrough_builder(
        name_gen.next_block_name(),
        orelse_entry,
        Vec::new(),
    );
 
     let then_label = name_gen.next_block_name();
     let else_label = name_gen.next_block_name();
     entry.set_term(BlockTerm::IfTerm(TermIf {
         test: test.into(),
         then_label,
         else_label,
     }));
 
     let mut deps = Vec::new();
     let mut body_entry_block = body_fragment.entry;
     body_entry_block.label = then_label;
     deps.push(body_entry_block);
     deps.extend(body_fragment.deps);
     let mut orelse_entry_block = orelse_fragment.entry;
     orelse_entry_block.label = else_label;
     deps.push(orelse_entry_block);
     deps.extend(orelse_fragment.deps);
 
     Some(Ok(LoweredExpr {
         setup: InlineFragment::from_closed_builder(name_gen.next_block_name(), entry, deps),
         value: E::from_lowered_expr(load_name(&target)),
     }))
}
pub(super) fn lower_if_expr_into<L, E>(
    lowerer: &L,
    if_expr: ast::ExprIf,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<Expr, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let ast::ExprIf {
        test, body, orelse, ..
    } = if_expr;
    let target = fresh_setup_name("tmp");
    let test = lowerer.lower_expr_ast_into(*test, out, loop_ctx, next_label_id)?;

    let mut body_out = BlockPyStmtBuilder::<E>::new();
    let body_value = lowerer.lower_expr_ast_into(*body, &mut body_out, loop_ctx, next_label_id)?;
    body_out.push_stmt(assign_name(&target, body_value));

    let mut orelse_out = BlockPyStmtBuilder::<E>::new();
    let orelse_value =
        lowerer.lower_expr_ast_into(*orelse, &mut orelse_out, loop_ctx, next_label_id)?;
    orelse_out.push_stmt(assign_name(&target, orelse_value));

    out.push_stmt(StructuredInstr::If(StructuredIf {
        test: test.into(),
        body: body_out.finish(),
        orelse: orelse_out.finish(),
    }));
    Ok(load_name(&target))
}

#[cfg(test)]
mod test;
