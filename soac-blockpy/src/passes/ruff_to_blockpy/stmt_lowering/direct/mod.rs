use super::*;
use crate::passes::ast_to_ast::ast_rewrite::Rewrite;
use crate::py_stmt;

// Direct stmt lowerers map one Ruff stmt to either a BlockPy stmt, a terminator,
// or no output at all. They do not need their own AST rewrite helpers.

pub(crate) fn rewrite_raise_stmt(mut raise: ast::StmtRaise) -> Rewrite {
    match (raise.exc.take(), raise.cause.take()) {
        (Some(exc), Some(cause)) => Rewrite::Walk(vec![py_stmt!(
            "raise __soac__.raise_from({exc:expr}, {cause:expr})",
            exc = exc,
            cause = cause,
        )]),
        (exc, None) => {
            raise.exc = exc;
            Rewrite::Unmodified(raise.into())
        }
        (None, Some(_)) => {
            panic!("raise with a cause but without an exception should be impossible")
        }
    }
}

impl StmtLowerer for ast::StmtGlobal {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(Stmt::Global(self))
    }

    fn to_blockpy<E>(
        &self,
        _context: &Context,
        _out: &mut BlockPyStmtBuilder<E>,
        _loop_ctx: Option<&LoopContext>,
        _next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        Ok(())
    }
}

impl StmtLowerer for ast::StmtNonlocal {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(Stmt::Nonlocal(self))
    }

    fn to_blockpy<E>(
        &self,
        _context: &Context,
        _out: &mut BlockPyStmtBuilder<E>,
        _loop_ctx: Option<&LoopContext>,
        _next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        Ok(())
    }
}

impl StmtLowerer for ast::StmtPass {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(Stmt::Pass(self))
    }

    fn to_blockpy<E>(
        &self,
        _context: &Context,
        _out: &mut BlockPyStmtBuilder<E>,
        _loop_ctx: Option<&LoopContext>,
        _next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        Ok(())
    }
}

impl StmtLowerer for ast::StmtExpr {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(Stmt::Expr(self))
    }

    fn to_blockpy<E>(
        &self,
        _context: &Context,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        let value = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
            (*self.value).clone(),
            out,
            loop_ctx,
            next_label_id,
        )?;
        out.push_stmt(StructuredInstr::Expr(value));
        Ok(())
    }
}

impl StmtLowerer for ast::StmtBreak {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(Stmt::Break(self))
    }

    fn to_blockpy<E>(
        &self,
        _context: &Context,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        _next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        if let Some(loop_ctx) = loop_ctx {
            out.set_term(BlockTerm::Jump(BlockEdge::new(
                loop_ctx.break_label.clone(),
            )));
            Ok(())
        } else {
            panic!("Break should be lowered before Ruff AST -> BlockPy conversion");
        }
    }
}

impl StmtLowerer for ast::StmtContinue {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(Stmt::Continue(self))
    }

    fn to_blockpy<E>(
        &self,
        _context: &Context,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        _next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        if let Some(loop_ctx) = loop_ctx {
            out.set_term(BlockTerm::Jump(BlockEdge::new(
                loop_ctx.continue_label.clone(),
            )));
            Ok(())
        } else {
            panic!("Continue should be lowered before Ruff AST -> BlockPy conversion");
        }
    }
}

impl StmtLowerer for ast::StmtReturn {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        single_stmt(Stmt::Return(self))
    }

    fn to_blockpy<E>(
        &self,
        _context: &Context,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        let value = match self.value.as_ref() {
            Some(value) => {
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    (**value).clone(),
                    out,
                    loop_ctx,
                    next_label_id,
                )?
            }
            None => crate::py_expr!("None").into(),
        };
        out.set_term(BlockTerm::Return(value));
        Ok(())
    }
}

impl StmtLowerer for ast::StmtRaise {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        stmts_from_rewrite(rewrite_raise_stmt(self))
    }

    fn to_blockpy<E>(
        &self,
        _context: &Context,
        out: &mut BlockPyStmtBuilder<E>,
        loop_ctx: Option<&LoopContext>,
        next_label_id: &mut usize,
    ) -> Result<(), String>
    where
        E: RuffToBlockPyExpr,
    {
        if self.cause.is_some() {
            panic!("raise-from should be lowered before Ruff AST -> BlockPy conversion");
        }
        let exc = match self.exc.as_ref() {
            Some(exc) => Some(
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    (**exc).clone(),
                    out,
                    loop_ctx,
                    next_label_id,
                )?,
            ),
            None => None,
        };
        out.set_term(BlockTerm::Raise(TermRaise { exc }));
        Ok(())
    }
}

#[cfg(test)]
mod test;
