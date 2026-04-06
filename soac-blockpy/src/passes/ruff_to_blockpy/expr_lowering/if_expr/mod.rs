use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{
    BlockTerm, Meta, Store, TermIf, WithMeta,
};
use crate::passes::InstrRuff;
use crate::passes::ruff_to_blockpy::expr_lowering::fresh_setup_name;
use crate::passes::ruff_to_blockpy::{InlineFragment, LoweredExpr, LoopContext};
use crate::py_expr;
use ruff_python_ast::{self as ast};

fn store_name(name: &str) -> ast::name::Name {
    name.into()
}

fn load_name(name: &str) -> InstrRuff {
    InstrRuff::from_ast_expr(py_expr!("{name:id}", name = name))
}

fn assign_name<E>(target: &str, value: InstrRuff) -> E
where
    E: RuffToBlockPyExpr,
{
    let target = store_name(target);
    let meta = Meta::synthetic();
    Store::new(target, E::from_lowered_expr(value))
        .with_meta(meta)
        .into()
}

#[allow(dead_code)]
pub(crate) fn try_lower_if_expr_direct<L, E>(
     lowerer: &L,
     name_gen: &crate::block_py::FunctionNameGen,
     if_expr: crate::block_py::ExprIf<InstrRuff>,
     loop_ctx: Option<&LoopContext>,
 ) -> Option<Result<LoweredExpr<E, InstrRuff>, String>>
 where
     L: BlockPySetupExprLowerer + ?Sized,
     E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
 {
     let crate::block_py::ExprIf { test, body, orelse, .. } = if_expr;
     let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
     let Some(test_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(
         name_gen,
         |structured| lowerer.lower_expr_instr_into(*test.clone(), structured, loop_ctx),
     )
     else {
         return None;
     };
     let (mut entry, test) = match test_setup {
         Ok(value) => value,
         Err(err) => return Some(Err(err)),
     };
 
     let target = fresh_setup_name("tmp");
     let Some(body_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(
         name_gen,
         |structured| {
             let body_value = lowerer.lower_expr_instr_into(*body.clone(), structured, loop_ctx)?;
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
    let mut body_fragment = body_entry.finish_fallthrough();
 
     let Some(orelse_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(
         name_gen,
         |structured| {
             let orelse_value =
                 lowerer.lower_expr_instr_into(*orelse.clone(), structured, loop_ctx)?;
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
    let mut orelse_fragment = orelse_entry.finish_fallthrough();
 
     let then_label = name_gen.next_block_name();
     let else_label = name_gen.next_block_name();
     entry.set_term(BlockTerm::IfTerm(TermIf {
         test: E::from_lowered_expr(test),
         then_label,
         else_label,
     }));
 
    body_fragment.relabel_entry(then_label);
    orelse_fragment.relabel_entry(else_label);

    let mut deps = Vec::new();
    deps.push(body_fragment.entry);
    deps.extend(body_fragment.deps);
    deps.push(orelse_fragment.entry);
    deps.extend(orelse_fragment.deps);

    let mut setup = entry.finish_fallthrough();
    setup.relabel_entry(name_gen.next_block_name());
    setup.deps.extend(deps);

    Some(Ok(LoweredExpr {
        setup: InlineFragment::new(setup.entry, setup.deps),
        value: load_name(&target),
    }))
}
pub(super) fn lower_if_expr_into<L, E>(
    lowerer: &L,
    if_expr: crate::block_py::ExprIf<InstrRuff>,
    out: &mut crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if let Some(lowered) = try_lower_if_expr_direct(lowerer, out.name_gen(), if_expr, loop_ctx) {
        let lowered = lowered?;
        out.append_fragment(lowered.setup);
        return Ok(lowered.value);
    }
    Err("if expression lowering requires inline fragment lowering".to_string())
}

#[cfg(test)]
mod test;
