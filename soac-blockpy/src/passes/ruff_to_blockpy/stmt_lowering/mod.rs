use super::*;
use crate::block_py::{BlockTerm, Expr, HasMeta, TermRaise};
use crate::passes::ast_to_ast::ast_rewrite::Rewrite;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::InstrRuff;

pub(super) type BlockPyStmtBuilder<E> = crate::passes::ruff_to_blockpy::InlineBlockBuilder<E>;

pub(crate) struct StructuredLoweringBridge;

impl StructuredLoweringBridge {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn try_lower_inline_value<E, T>(
        &self,
        name_gen: &FunctionNameGen,
        lower: impl FnOnce(&mut BlockPyStmtBuilder<E>) -> Result<T, String>,
    ) -> Option<Result<(BlockPyStmtBuilder<E>, T), String>>
    where
        E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
    {
        try_lower_inline_value_from_structured(name_gen, lower)
    }
}

fn try_lower_inline_value_from_structured<E, T>(
    name_gen: &FunctionNameGen,
    lower: impl FnOnce(&mut BlockPyStmtBuilder<E>) -> Result<T, String>,
) -> Option<Result<(BlockPyStmtBuilder<E>, T), String>>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let mut structured = BlockPyStmtBuilder::<E>::new(name_gen);
    let value = match lower(&mut structured) {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    Some(Ok((structured, value)))
}

pub(super) fn stmts_from_rewrite(rewrite: Rewrite) -> Vec<Stmt> {
    match rewrite {
        Rewrite::Unmodified(stmt) => vec![stmt],
        Rewrite::Walk(stmts) => stmts,
    }
}

pub(super) fn instrs_from_rewrite(rewrite: Rewrite) -> Vec<InstrRuff> {
    stmts_from_rewrite(rewrite)
        .into_iter()
        .map(InstrRuff::from_ast_stmt)
        .collect()
}

pub(super) fn single_stmt(stmt: impl Into<Stmt>) -> Vec<Stmt> {
    vec![stmt.into()]
}

pub(crate) fn lower_nested_stmt_into_with_expr<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    stmt: &Stmt,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if should_simplify_nested_stmt_head(stmt) {
        for simplified in simplify_stmt_head_ast_for_blockpy(context, stmt.clone()) {
            lower_instr_into_with_expr(
                context,
                &InstrRuff::from_ast_stmt(simplified),
                name_gen,
                out,
                loop_ctx,
            )?;
        }
        Ok(())
    } else {
        lower_instr_into_with_expr(
            context,
            &InstrRuff::from_ast_stmt(stmt.clone()),
            name_gen,
            out,
            loop_ctx,
        )
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
pub(crate) use assign_stmt::lower_assign_instr_into;
pub(crate) use augassign_stmt::lower_augassign_instr_into;
pub(crate) use delete_stmt::lower_delete_instr_into;
pub(crate) use if_stmt::try_lower_if_instr_fragment;
pub(crate) use try_stmt::{lower_star_try_stmt_sequence, lower_try_stmt_sequence};
pub(crate) use with_stmt::lower_with_stmt_sequence;

fn simplify_stmt_ast_once_for_blockpy(context: &Context, stmt: Stmt) -> Vec<Stmt> {
    match stmt {
        Stmt::Global(stmt) => single_stmt(stmt),
        Stmt::Nonlocal(stmt) => single_stmt(stmt),
        Stmt::Pass(stmt) => single_stmt(stmt),
        Stmt::Expr(stmt) => single_stmt(stmt),
        Stmt::Assign(stmt) => single_stmt(stmt),
        Stmt::Delete(stmt) => single_stmt(stmt),
        Stmt::FunctionDef(stmt) => single_stmt(Stmt::FunctionDef(stmt)),
        Stmt::ClassDef(stmt) => single_stmt(Stmt::ClassDef(stmt)),
        Stmt::TypeAlias(stmt) => stmts_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::type_alias_stmt::rewrite_type_alias_stmt(
                context, stmt,
            ),
        ),
        Stmt::AugAssign(stmt) => single_stmt(stmt),
        Stmt::AnnAssign(stmt) => single_stmt(Stmt::AnnAssign(stmt)),
        Stmt::If(stmt) => stmts_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::if_stmt::expand_if_chain(stmt),
        ),
        Stmt::While(stmt) => single_stmt(Stmt::While(stmt)),
        Stmt::For(stmt) => single_stmt(Stmt::For(stmt)),
        Stmt::With(stmt) => {
            crate::passes::ruff_to_blockpy::stmt_lowering::with_stmt::desugar_structured_with_stmt_for_blockpy(stmt)
        }
        Stmt::Match(stmt) => stmts_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::match_stmt::rewrite_match_stmt(
                context, stmt,
            ),
        ),
        Stmt::Assert(stmt) => stmts_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::assert_stmt::rewrite_assert_stmt(stmt),
        ),
        Stmt::Import(stmt) => stmts_from_rewrite(crate::passes::ast_to_ast::rewrite_import::rewrite(stmt)),
        Stmt::ImportFrom(stmt) => stmts_from_rewrite(
            crate::passes::ast_to_ast::rewrite_import::rewrite_from(context, stmt),
        ),
        Stmt::Break(stmt) => single_stmt(stmt),
        Stmt::Continue(stmt) => single_stmt(stmt),
        Stmt::Return(stmt) => single_stmt(stmt),
        Stmt::Raise(stmt) => stmts_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::direct::rewrite_raise_stmt(stmt),
        ),
        Stmt::Try(stmt) => stmts_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::try_stmt::rewrite_try_stmt(stmt),
        ),
        Stmt::IpyEscapeCommand(stmt) => single_stmt(Stmt::IpyEscapeCommand(stmt)),
    }
}

pub(super) fn simplify_stmt_head_ast_for_blockpy(context: &Context, stmt: Stmt) -> Vec<Stmt> {
    let stmts = simplify_stmt_ast_once_for_blockpy(context, stmt);
    finish_stmt_head_ast_for_blockpy(context, stmts)
}

pub(super) fn simplify_instr_head_for_blockpy(
    context: &Context,
    stmt: InstrRuff,
) -> Vec<InstrRuff> {
    match stmt {
        InstrRuff::StmtIf(mut if_stmt) => {
            if_stmt.test = Box::new(
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_head_ast_for_blockpy(
                    *if_stmt.test,
                ),
            );
            vec![InstrRuff::StmtIf(if_stmt)]
        }
        InstrRuff::StmtRaise(stmt) => instrs_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::direct::rewrite_raise_stmt(
                ast::StmtRaise {
                    range: stmt.meta().range,
                    node_index: stmt.meta().node_index,
                    exc: stmt.exc.map(|expr| Box::new(expr.into_ast_expr())),
                    cause: stmt.cause.map(|expr| Box::new(expr.into_ast_expr())),
                },
            ),
        ),
        InstrRuff::StmtTry(stmt) => {
            crate::passes::ruff_to_blockpy::stmt_lowering::try_stmt::rewrite_try_instr(stmt)
        }
        InstrRuff::StmtAssert(stmt) => instrs_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::assert_stmt::rewrite_assert_stmt(
                ast::StmtAssert {
                    range: stmt.meta().range,
                    node_index: stmt.meta().node_index,
                    test: Box::new((*stmt.test).into_ast_expr()),
                    msg: stmt.msg.map(|expr| Box::new(expr.into_ast_expr())),
                },
            ),
        ),
        InstrRuff::StmtMatch(stmt) => instrs_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::match_stmt::rewrite_match_stmt(
                context,
                ast::StmtMatch {
                    range: stmt.meta().range,
                    node_index: stmt.meta().node_index,
                    subject: Box::new((*stmt.subject).into_ast_expr()),
                    cases: stmt.cases,
                },
            ),
        ),
        InstrRuff::StmtImport(stmt) => instrs_from_rewrite(
            crate::passes::ast_to_ast::rewrite_import::rewrite(ast::StmtImport {
                range: stmt.meta().range,
                node_index: stmt.meta().node_index,
                names: stmt.names,
            }),
        ),
        InstrRuff::StmtImportFrom(stmt) => instrs_from_rewrite(
            crate::passes::ast_to_ast::rewrite_import::rewrite_from(
                context,
                ast::StmtImportFrom {
                    range: stmt.meta().range,
                    node_index: stmt.meta().node_index,
                    module: stmt.module,
                    names: stmt.names,
                    level: stmt.level,
                },
            ),
        ),
        InstrRuff::StmtTypeAlias(stmt) => instrs_from_rewrite(
            crate::passes::ruff_to_blockpy::stmt_lowering::type_alias_stmt::rewrite_type_alias_stmt(
                context,
                ast::StmtTypeAlias {
                    range: stmt.meta().range,
                    node_index: stmt.meta().node_index,
                    name: Box::new(stmt.name.into_ast_expr()),
                    type_params: stmt.type_params,
                    value: Box::new((*stmt.value).into_ast_expr()),
                },
            ),
        ),
        other => vec![other],
    }
}

fn finish_stmt_head_ast_for_blockpy(_context: &Context, stmts: Vec<Stmt>) -> Vec<Stmt> {
    match stmts.as_slice() {
        [Stmt::If(if_stmt)] => {
            let mut if_stmt = if_stmt.clone();
            if_stmt.test = Box::new(
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_head_ast_for_blockpy(
                    crate::passes::InstrRuff::from_ast_expr(*if_stmt.test),
                )
                .into_ast_expr(),
            );
            vec![Stmt::If(if_stmt)]
        }
        [_] | [] => stmts,
        _ => stmts,
    }
}

fn plan_simplified_instr_head_for_blockpy(
    _context: &Context,
    simplified: Vec<InstrRuff>,
) -> StmtSequenceHeadPlan {
    if simplified.len() != 1 {
        return StmtSequenceHeadPlan::Expanded(simplified);
    }
    let simplified = simplified
        .into_iter()
        .next()
        .expect("single simplified instr should exist");
    match simplified {
        InstrRuff::StmtExpr(_)
        | InstrRuff::StmtPass(_)
        | InstrRuff::StmtAssign(_)
        | InstrRuff::StmtDelete(_)
        | InstrRuff::StmtGlobal(_)
        | InstrRuff::StmtNonlocal(_)
        | InstrRuff::StmtAugAssign(_)
        | InstrRuff::StmtTypeAlias(_)
        | InstrRuff::StmtImportFrom(_) => StmtSequenceHeadPlan::Linear(simplified),
        InstrRuff::StmtFunctionDef(func_def) => StmtSequenceHeadPlan::FunctionDef(func_def),
        InstrRuff::StmtRaise(raise_stmt) => StmtSequenceHeadPlan::Raise(raise_stmt),
        InstrRuff::StmtReturn(ret) => StmtSequenceHeadPlan::Return(*ret.value),
        InstrRuff::StmtIf(if_stmt) => StmtSequenceHeadPlan::If(if_stmt),
        InstrRuff::StmtWhile(while_stmt) => StmtSequenceHeadPlan::While(while_stmt),
        InstrRuff::StmtFor(for_stmt) => StmtSequenceHeadPlan::For(for_stmt),
        InstrRuff::StmtTry(try_stmt) => StmtSequenceHeadPlan::Try(try_stmt),
        InstrRuff::StmtWith(with_stmt) => StmtSequenceHeadPlan::With(with_stmt),
        InstrRuff::StmtBreak(_) => StmtSequenceHeadPlan::Break,
        InstrRuff::StmtContinue(_) => StmtSequenceHeadPlan::Continue,
        _ => StmtSequenceHeadPlan::Unsupported,
    }
}

pub(crate) fn plan_instr_head_for_blockpy(
    context: &Context,
    stmt: &InstrRuff,
) -> StmtSequenceHeadPlan {
    plan_simplified_instr_head_for_blockpy(context, simplify_instr_head_for_blockpy(context, stmt.clone()))
}

#[cfg(test)]
pub(crate) fn lower_instr_for_test(
    context: &Context,
    stmt: &InstrRuff,
    name_gen: &FunctionNameGen,
    out: &mut BlockPyStmtBuilder<crate::block_py::InstrWithAwaitAndYield>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String> {
    lower_instr_into_with_expr(context, stmt, name_gen, out, loop_ctx)
}

pub(crate) fn lower_instr_into_with_expr<E>(
    context: &Context,
    stmt: &InstrRuff,
    name_gen: &FunctionNameGen,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<(), String>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let lower_simplified =
        |instrs: Vec<InstrRuff>, out: &mut BlockPyStmtBuilder<E>| -> Result<(), String> {
            for instr in instrs {
                lower_instr_into_with_expr(context, &instr, name_gen, out, loop_ctx)?;
            }
            Ok(())
        };
    match stmt {
        InstrRuff::StmtGlobal(_) | InstrRuff::StmtNonlocal(_) | InstrRuff::StmtPass(_) => Ok(()),
        InstrRuff::StmtExpr(stmt) => {
            let value = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                (*stmt.value).clone(),
                out,
                loop_ctx,
            )?;
            out.push_stmt(value);
            Ok(())
        }
        InstrRuff::StmtAssign(stmt) => lower_assign_instr_into(context, stmt, out, loop_ctx),
        InstrRuff::StmtAugAssign(stmt) => lower_augassign_instr_into(context, stmt, out, loop_ctx),
        InstrRuff::StmtDelete(stmt) => lower_delete_instr_into(context, stmt, out, loop_ctx),
        InstrRuff::StmtIf(stmt) => match try_lower_if_instr_fragment(context, name_gen, stmt, loop_ctx) {
            Some(result) => {
                out.append_fragment(result?);
                Ok(())
            }
            None => Err("if statement lowering requires inline fragment lowering".to_string()),
        },
        InstrRuff::StmtAssert(_)
        | InstrRuff::StmtImport(_)
        | InstrRuff::StmtImportFrom(_)
        | InstrRuff::StmtMatch(_)
        | InstrRuff::StmtTypeAlias(_) => lower_simplified(
            simplify_instr_head_for_blockpy(context, stmt.clone()),
            out,
        ),
        InstrRuff::StmtBreak(_) => {
            if let Some(loop_ctx) = loop_ctx {
                out.set_term(BlockTerm::Jump(BlockEdge::new(loop_ctx.break_label.clone())));
                Ok(())
            } else {
                panic!("Break should be lowered before Ruff AST -> BlockPy conversion");
            }
        }
        InstrRuff::StmtContinue(_) => {
            if let Some(loop_ctx) = loop_ctx {
                out.set_term(BlockTerm::Jump(BlockEdge::new(
                    loop_ctx.continue_label.clone(),
                )));
                Ok(())
            } else {
                panic!("Continue should be lowered before Ruff AST -> BlockPy conversion");
            }
        }
        InstrRuff::StmtReturn(stmt) => {
            let value = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                (*stmt.value).clone(),
                out,
                loop_ctx,
            )?;
            out.set_term(BlockTerm::Return(value));
            Ok(())
        }
        InstrRuff::StmtRaise(stmt) => {
            if stmt.cause.is_some() {
                panic!("raise-from should be lowered before Ruff AST -> BlockPy conversion");
            }
            let exc = stmt
                .exc
                .as_ref()
                .map(|exc| {
                    crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                        (**exc).clone(),
                        out,
                        loop_ctx,
                    )
                })
                .transpose()?;
            out.set_term(BlockTerm::Raise(TermRaise { exc }));
            Ok(())
        }
        InstrRuff::StmtFunctionDef(func_def) => {
            panic!(
                "FunctionDef {} should be extracted before Ruff AST -> BlockPy conversion",
                func_def.name.id
            );
        }
        InstrRuff::StmtClassDef(class_def) => {
            panic!(
                "ClassDef {} should be lowered before Ruff AST -> BlockPy conversion",
                class_def.name.id
            );
        }
        InstrRuff::StmtAnnAssign(_) => {
            panic!("AnnAssign should be lowered before Ruff AST -> BlockPy conversion");
        }
        InstrRuff::StmtWhile(_) => {
            panic!("While should be lowered before Ruff AST -> BlockPy stmt-list conversion");
        }
        InstrRuff::StmtFor(_) => {
            panic!("For should be lowered before Ruff AST -> BlockPy stmt-list conversion");
        }
        InstrRuff::StmtWith(_) => {
            panic!("With should be lowered before Ruff AST -> BlockPy stmt-list conversion");
        }
        InstrRuff::StmtTry(_) => {
            panic!("Try should be lowered before Ruff AST -> BlockPy stmt-list conversion");
        }
        InstrRuff::StmtIpyEscapeCommand(_) => {
            panic!("IpyEscapeCommand should not reach BlockPy conversion");
        }
        _ => {
            panic!("expression should not reach Ruff AST stmt lowering");
        }
    }
}
