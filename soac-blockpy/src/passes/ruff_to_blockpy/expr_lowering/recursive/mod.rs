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

pub(super) fn lower_expr_ast_recursive<L, E>(
    lowerer: &L,
    expr: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    match expr {
        InstrRuff::ExprBoolOp(bool_op) => lower_boolop_into(lowerer, bool_op, out, loop_ctx),
        InstrRuff::ExprCompare(compare) => lower_compare_into(lowerer, compare, out, loop_ctx),
        InstrRuff::ExprIf(if_expr) => lower_if_expr_into(lowerer, if_expr, out, loop_ctx),
        InstrRuff::ExprNamed(named_expr) => {
            lower_named_expr_into(lowerer, named_expr, out, loop_ctx)
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
