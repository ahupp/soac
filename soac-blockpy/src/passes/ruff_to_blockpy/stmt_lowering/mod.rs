use super::*;
use crate::block_py::{BlockTerm, Expr, StructuredInstr, TermRaise};
use crate::passes::ast_to_ast::ast_rewrite::Rewrite;
use crate::passes::ast_to_ast::context::Context;

pub(super) type BlockPyStmtBuilder<E> =
    crate::block_py::BlockBuilder<StructuredInstr<E>, BlockTerm<E>>;

pub(crate) fn try_lower_inline_value_from_structured<E, T>(
    next_label_id: &mut usize,
    lower: impl FnOnce(&mut BlockPyStmtBuilder<E>, &mut usize) -> Result<T, String>,
) -> Option<Result<(InlineBlockBuilder<E>, T), String>>
where
    E: RuffToBlockPyExpr,
{
    let mut structured = BlockPyStmtBuilder::<E>::new();
    let mut scratch_next_label_id = *next_label_id;
    let value = match lower(&mut structured, &mut scratch_next_label_id) {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    let structured = structured.finish();

    let mut entry = crate::block_py::BlockBuilder::<E, BlockTerm<E>>::new();
    for stmt in structured.body {
        let StructuredInstr::Expr(stmt) = stmt else {
            return None;
        };
        entry.push_stmt(stmt);
    }

    match structured.term {
        None => {}
        Some(term) => entry.set_term(term),
    }

    *next_label_id = scratch_next_label_id;
    Some(Ok((entry, value)))
}

pub(super) fn try_lower_inline_from_structured<E>(
    name_gen: &FunctionNameGen,
    next_label_id: &mut usize,
    lower: impl FnOnce(&mut BlockPyStmtBuilder<E>, &mut usize) -> Result<(), String>,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr,
{
    try_lower_inline_value_from_structured(next_label_id, lower)
        .map(|result| {
            result.map(|(mut entry, ())| {
                if entry.term.is_none() {
                    entry.set_term(BlockTerm::Jump(crate::block_py::BlockEdge::new(
                        BlockLabel::fallthrough(),
                    )));
                }
                InlineFragment::from_builder(name_gen.next_block_name(), entry, Vec::new())
            })
        })
}

pub(crate) fn lower_stmt_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    stmt: &Stmt,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr,
{
    direct::try_lower_direct_stmt_fragment(context, name_gen, stmt, loop_ctx)
}

pub(super) fn stmts_from_rewrite(rewrite: Rewrite) -> Vec<Stmt> {
    match rewrite {
        Rewrite::Unmodified(stmt) => vec![stmt],
        Rewrite::Walk(stmts) => stmts,
    }
}

pub(super) fn single_stmt(stmt: impl Into<Stmt>) -> Vec<Stmt> {
    vec![stmt.into()]
}

pub(super) fn stmt_to_stmts(stmt: Stmt) -> Vec<Stmt> {
    vec![stmt]
}

pub(super) fn lower_stmt_via_simplify<T, E>(
    context: &Context,
    stmt: &T,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<(), String>
where
    T: StmtLowerer + Clone,
    E: RuffToBlockPyExpr,
{
    for simplified in stmt.clone().simplify_ast(context) {
        lower_stmt_into_with_expr(context, &simplified, out, loop_ctx, next_label_id)?;
    }
    Ok(())
}

pub(crate) fn lower_nested_stmt_into_with_expr<E>(
    context: &Context,
    stmt: &Stmt,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    if should_simplify_nested_stmt_head(stmt) {
        for simplified in simplify_stmt_head_ast_for_blockpy(context, stmt.clone()) {
            lower_stmt_into_with_expr(context, &simplified, out, loop_ctx, next_label_id)?;
        }
        Ok(())
    } else {
        lower_stmt_into_with_expr(context, stmt, out, loop_ctx, next_label_id)
    }
}

fn should_simplify_nested_stmt_head(stmt: &Stmt) -> bool {
    matches!(
        stmt,
        Stmt::If(_)
            | Stmt::Match(_)
            | Stmt::Assert(_)
            | Stmt::Expr(_)
            | Stmt::Assign(_)
            | Stmt::AugAssign(_)
            | Stmt::Return(_)
            | Stmt::Raise(_)
    )
}

pub(super) trait StmtLowerer {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt>
    where
        Self: Sized;

    fn plan_head(self, context: &Context) -> StmtSequenceHeadPlan
    where
        Self: Sized,
    {
        plan_simplified_stmt_head_for_blockpy(context, self.simplify_ast(context))
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
        panic!(
            "{} should have already been reduced before BlockPy lowering",
            std::any::type_name::<Self>()
        );
    }
}

macro_rules! impl_unreduced_stmt_lowerer {
    ($ty:path, $variant:path, $message:literal) => {
        impl StmtLowerer for $ty {
            fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
                single_stmt($variant(self))
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
                panic!($message);
            }
        }
    };
}

mod assert_stmt;
mod assign_stmt;
mod augassign_stmt;
mod delete_stmt;
mod direct;
mod if_stmt;
mod import_from_stmt;
mod import_stmt;
mod match_stmt;
mod try_stmt;
mod type_alias_stmt;
mod unreduced;
mod with_stmt;

pub(crate) use assign_stmt::build_for_target_assign_body;
pub(crate) use if_stmt::try_lower_if_stmt_fragment;
pub(crate) use try_stmt::{lower_star_try_stmt_sequence, lower_try_stmt_sequence};
pub(crate) use with_stmt::lower_with_stmt_sequence;

fn simplify_stmt_ast_once_for_blockpy(context: &Context, stmt: Stmt) -> Vec<Stmt> {
    match stmt {
        Stmt::Global(stmt) => stmt.simplify_ast(context),
        Stmt::Nonlocal(stmt) => stmt.simplify_ast(context),
        Stmt::Pass(stmt) => stmt.simplify_ast(context),
        Stmt::Expr(stmt) => stmt.simplify_ast(context),
        Stmt::Assign(stmt) => stmt.simplify_ast(context),
        Stmt::Delete(stmt) => stmt.simplify_ast(context),
        Stmt::FunctionDef(stmt) => stmt.simplify_ast(context),
        Stmt::ClassDef(stmt) => stmt.simplify_ast(context),
        Stmt::TypeAlias(stmt) => stmt.simplify_ast(context),
        Stmt::AugAssign(stmt) => stmt.simplify_ast(context),
        Stmt::AnnAssign(stmt) => stmt.simplify_ast(context),
        Stmt::If(stmt) => stmt.simplify_ast(context),
        Stmt::While(stmt) => stmt.simplify_ast(context),
        Stmt::For(stmt) => stmt.simplify_ast(context),
        Stmt::With(stmt) => stmt.simplify_ast(context),
        Stmt::Match(stmt) => stmt.simplify_ast(context),
        Stmt::Assert(stmt) => stmt.simplify_ast(context),
        Stmt::Import(stmt) => stmt.simplify_ast(context),
        Stmt::ImportFrom(stmt) => stmt.simplify_ast(context),
        Stmt::Break(stmt) => stmt.simplify_ast(context),
        Stmt::Continue(stmt) => stmt.simplify_ast(context),
        Stmt::Return(stmt) => stmt.simplify_ast(context),
        Stmt::Raise(stmt) => stmt.simplify_ast(context),
        Stmt::Try(stmt) => stmt.simplify_ast(context),
        Stmt::IpyEscapeCommand(stmt) => stmt.simplify_ast(context),
    }
}

pub(super) fn simplify_stmt_head_ast_for_blockpy(context: &Context, stmt: Stmt) -> Vec<Stmt> {
    let stmts = simplify_stmt_ast_once_for_blockpy(context, stmt);
    finish_stmt_head_ast_for_blockpy(context, stmts)
}

fn finish_stmt_head_ast_for_blockpy(context: &Context, stmts: Vec<Stmt>) -> Vec<Stmt> {
    match stmts.as_slice() {
        [Stmt::If(if_stmt)] => vec![simplify_if_test_for_blockpy(context, if_stmt.clone())],
        [_] | [] => stmts,
        _ => stmts,
    }
}

fn plan_simplified_stmt_head_for_blockpy(
    context: &Context,
    simplified: Vec<Stmt>,
) -> StmtSequenceHeadPlan {
    let simplified = finish_stmt_head_ast_for_blockpy(context, simplified);
    if simplified.len() != 1 {
        return StmtSequenceHeadPlan::Expanded(simplified);
    }
    let simplified = simplified
        .into_iter()
        .next()
        .expect("single simplified stmt should exist");
    match simplified {
        Stmt::Expr(_)
        | Stmt::Pass(_)
        | Stmt::Assign(_)
        | Stmt::Delete(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::AugAssign(_)
        | Stmt::TypeAlias(_)
        | Stmt::ImportFrom(_) => StmtSequenceHeadPlan::Linear(simplified),
        Stmt::FunctionDef(func_def) => StmtSequenceHeadPlan::FunctionDef(func_def),
        Stmt::Raise(raise_stmt) => StmtSequenceHeadPlan::Raise(raise_stmt),
        Stmt::Return(ret) => {
            StmtSequenceHeadPlan::Return(ret.value.as_ref().map(|expr| *expr.clone()))
        }
        Stmt::If(if_stmt) => StmtSequenceHeadPlan::If(if_stmt),
        Stmt::While(while_stmt) => StmtSequenceHeadPlan::While(while_stmt),
        Stmt::For(for_stmt) => StmtSequenceHeadPlan::For(for_stmt),
        Stmt::Try(try_stmt) => StmtSequenceHeadPlan::Try(try_stmt),
        Stmt::With(with_stmt) => StmtSequenceHeadPlan::With(with_stmt),
        Stmt::Break(_) => StmtSequenceHeadPlan::Break,
        Stmt::Continue(_) => StmtSequenceHeadPlan::Continue,
        _ => StmtSequenceHeadPlan::Unsupported,
    }
}

pub(crate) fn plan_stmt_head_for_blockpy(context: &Context, stmt: &Stmt) -> StmtSequenceHeadPlan {
    match stmt {
        Stmt::Global(stmt) => stmt.clone().plan_head(context),
        Stmt::Nonlocal(stmt) => stmt.clone().plan_head(context),
        Stmt::Pass(stmt) => stmt.clone().plan_head(context),
        Stmt::Expr(stmt) => stmt.clone().plan_head(context),
        Stmt::Assign(stmt) => stmt.clone().plan_head(context),
        Stmt::Delete(stmt) => stmt.clone().plan_head(context),
        Stmt::FunctionDef(stmt) => stmt.clone().plan_head(context),
        Stmt::ClassDef(stmt) => stmt.clone().plan_head(context),
        Stmt::TypeAlias(stmt) => stmt.clone().plan_head(context),
        Stmt::AugAssign(stmt) => stmt.clone().plan_head(context),
        Stmt::AnnAssign(stmt) => stmt.clone().plan_head(context),
        Stmt::If(stmt) => stmt.clone().plan_head(context),
        Stmt::While(stmt) => stmt.clone().plan_head(context),
        Stmt::For(stmt) => stmt.clone().plan_head(context),
        Stmt::With(stmt) => stmt.clone().plan_head(context),
        Stmt::Match(stmt) => stmt.clone().plan_head(context),
        Stmt::Assert(stmt) => stmt.clone().plan_head(context),
        Stmt::Import(stmt) => stmt.clone().plan_head(context),
        Stmt::ImportFrom(stmt) => stmt.clone().plan_head(context),
        Stmt::Break(stmt) => stmt.clone().plan_head(context),
        Stmt::Continue(stmt) => stmt.clone().plan_head(context),
        Stmt::Return(stmt) => stmt.clone().plan_head(context),
        Stmt::Raise(stmt) => stmt.clone().plan_head(context),
        Stmt::Try(stmt) => stmt.clone().plan_head(context),
        Stmt::IpyEscapeCommand(stmt) => stmt.clone().plan_head(context),
    }
}

fn simplify_if_test_for_blockpy(_context: &Context, mut if_stmt: ast::StmtIf) -> Stmt {
    if_stmt.test = Box::new(
        crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_head_ast_for_blockpy(
            *if_stmt.test,
        ),
    );
    Stmt::If(if_stmt)
}

#[cfg(test)]
pub(crate) fn lower_stmt_into(
    context: &Context,
    stmt: &Stmt,
    out: &mut crate::block_py::BlockBuilder<
        StructuredInstr<crate::block_py::InstrWithAwaitAndYield>,
        BlockTerm<crate::block_py::InstrWithAwaitAndYield>,
    >,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<(), String> {
    lower_stmt_into_with_expr(context, stmt, out, loop_ctx, next_label_id)
}

pub(crate) fn lower_stmt_into_with_expr<E>(
    context: &Context,
    stmt: &Stmt,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr,
{
    match stmt {
        Stmt::Global(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Nonlocal(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Pass(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Expr(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Assign(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Delete(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::FunctionDef(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::ClassDef(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::TypeAlias(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::AugAssign(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::AnnAssign(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::If(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::While(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::For(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::With(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Match(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Assert(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Import(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::ImportFrom(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Break(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Continue(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Return(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Raise(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::Try(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
        Stmt::IpyEscapeCommand(stmt) => stmt.to_blockpy(context, out, loop_ctx, next_label_id),
    }?;
    Ok(())
}
