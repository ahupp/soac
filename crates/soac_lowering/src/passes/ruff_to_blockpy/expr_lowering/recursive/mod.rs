use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::Mappable;
use crate::passes::ruff_to_blockpy::expr_lowering::boolop_compare::{
    lower_boolop_into, lower_compare_into,
};
use crate::passes::ruff_to_blockpy::expr_lowering::if_expr::lower_if_expr_into;
use crate::passes::ruff_to_blockpy::expr_lowering::named_expr::lower_named_expr_into;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::LoopContext;
use crate::passes::InstrRuff;

fn lower_raw_ast_expr<L, E>(
    lowerer: &L,
    expr: ruff_python_ast::Expr,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<ruff_python_ast::Expr, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let instr = crate::passes::ast_to_instr::from_ast_expr(expr);
    let lowered = lower_expr_ast_recursive(lowerer, instr, out, loop_ctx)?;
    Ok(crate::passes::ast_to_instr::into_ast_expr(lowered))
}

pub(super) fn lower_expr_ast_recursive<L, E>(
    lowerer: &L,
    expr: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    match expr {
        InstrRuff::ExprBoolOp(bool_op) => lower_boolop_into(lowerer, bool_op, out, loop_ctx),
        InstrRuff::ExprCompare(compare) => lower_compare_into(lowerer, compare, out, loop_ctx),
        InstrRuff::ExprIf(if_expr) => lower_if_expr_into(lowerer, if_expr, out, loop_ctx),
        InstrRuff::ExprNamed(named_expr) => {
            lower_named_expr_into(lowerer, named_expr, out, loop_ctx)
        }
        InstrRuff::ExprDict(mut dict) => {
            let mut lowered_items = Vec::with_capacity(dict.items.len());
            for item in dict.items {
                let key = item
                    .key
                    .map(|key| lower_raw_ast_expr(lowerer, key, out, loop_ctx))
                    .transpose()?;
                let value = lower_raw_ast_expr(lowerer, item.value, out, loop_ctx)?;
                lowered_items.push(ruff_python_ast::DictItem { key, value });
            }
            dict.items = lowered_items;
            Ok(InstrRuff::ExprDict(dict))
        }
        expr @ (InstrRuff::StmtFunctionDef(_)
        | InstrRuff::StmtClassDef(_)
        | InstrRuff::StmtReturn(_)
        | InstrRuff::StmtDelete(_)
        | InstrRuff::StmtTypeAlias(_)
        | InstrRuff::StmtAssign(_)
        | InstrRuff::StmtAugAssign(_)
        | InstrRuff::StmtAnnAssign(_)
        | InstrRuff::StmtFor(_)
        | InstrRuff::StmtWhile(_)
        | InstrRuff::StmtIf(_)
        | InstrRuff::StmtWith(_)
        | InstrRuff::StmtMatch(_)
        | InstrRuff::StmtRaise(_)
        | InstrRuff::StmtTry(_)
        | InstrRuff::StmtAssert(_)
        | InstrRuff::StmtImport(_)
        | InstrRuff::StmtImportFrom(_)
        | InstrRuff::StmtGlobal(_)
        | InstrRuff::StmtNonlocal(_)
        | InstrRuff::StmtExpr(_)
        | InstrRuff::StmtPass(_)
        | InstrRuff::StmtBreak(_)
        | InstrRuff::StmtContinue(_)
        | InstrRuff::StmtIpyEscapeCommand(_)) => Err(format!(
            "statement-shaped InstrRuff reached expression lowering: {expr:?}"
        )),
        other => other.try_map_same_children(&mut |child| {
            lower_expr_ast_recursive(lowerer, child, out, loop_ctx)
        }),
    }
}

#[cfg(test)]
mod test;
