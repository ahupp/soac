use super::*;
use crate::block_py::{StructuredIf, TermIf};
use crate::passes::ast_to_ast::ast_rewrite::Rewrite;
use crate::passes::ast_to_ast::body::Suite;
use ruff_text_size::TextRange;

pub(crate) fn expand_if_chain(mut if_stmt: ast::StmtIf) -> Rewrite {
    if !if_stmt
        .elif_else_clauses
        .iter()
        .any(|clause| clause.test.is_some())
    {
        return Rewrite::Unmodified(if_stmt.into());
    }
    let mut else_body: Option<Suite> = None;

    for clause in if_stmt.elif_else_clauses.into_iter().rev() {
        match clause.test {
            Some(test) => {
                let mut nested_if = ast::StmtIf {
                    node_index: ast::AtomicNodeIndex::default(),
                    range: TextRange::default(),
                    test: Box::new(test),
                    body: clause.body,
                    elif_else_clauses: Vec::new(),
                };

                if let Some(body) = else_body.take() {
                    nested_if.elif_else_clauses.push(ast::ElifElseClause {
                        test: None,
                        body,
                        range: TextRange::default(),
                        node_index: ast::AtomicNodeIndex::default(),
                    });
                }

                else_body = Some(vec![Stmt::If(nested_if)]);
            }
            None => {
                let mut body = clause.body;
                else_body = Some(std::mem::take(&mut body));
            }
        }
    }

    if let Some(body) = else_body {
        if_stmt.elif_else_clauses = vec![ast::ElifElseClause {
            range: TextRange::default(),
            node_index: ast::AtomicNodeIndex::default(),
            test: None,
            body,
        }];
    } else {
        if_stmt.elif_else_clauses = Vec::new();
    }

    Rewrite::Walk(vec![if_stmt.into()])
}

#[allow(dead_code)]
pub(crate) fn try_lower_if_stmt_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    if_stmt: &ast::StmtIf,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    match simplify_stmt_head_ast_for_blockpy(context, Stmt::If(if_stmt.clone())).as_slice() {
        [Stmt::If(simplified_if)] => Some(lower_simplified_if_stmt_fragment(
            context,
            name_gen,
            simplified_if,
            loop_ctx,
        )),
        _ => None,
    }
}

#[allow(dead_code)]
fn lower_simplified_if_stmt_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    if_stmt: &ast::StmtIf,
    loop_ctx: Option<&LoopContext>,
) -> Result<InlineFragment<E>, String>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let ast::StmtIf {
        test,
        body,
        elif_else_clauses,
        ..
    } = if_stmt;
    let mut bridge = StructuredLoweringBridge::new();
    let Some(test_setup) = bridge.try_lower_inline_value(
        |structured, scratch_next_label_id| {
            crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                *test.clone(),
                structured,
                loop_ctx,
                scratch_next_label_id,
            )
        },
    ) else {
        return Err("if-test setup still requires structured lowering".to_string());
    };
    let (mut entry, test) = test_setup?;
    let entry_label = name_gen.next_block_name();

    let Some(body_setup) =
        lower_nested_body_to_inline_fragment(
            context,
            name_gen,
            body,
            loop_ctx,
            &mut bridge,
        )
    else {
        return Err("if-body still requires structured lowering".to_string());
    };
    let body_setup = body_setup?;

    let Some(orelse_setup) = lower_orelse_to_inline_fragment(
        context,
        name_gen,
        elif_else_clauses,
        &Stmt::If(if_stmt.clone()),
        loop_ctx,
        &mut bridge,
    ) else {
        return Err("if-orelse still requires structured lowering".to_string());
    };
    let orelse_setup = orelse_setup?;

    let then_label = name_gen.next_block_name();
    let else_label = name_gen.next_block_name();
    entry.set_term(BlockTerm::IfTerm(TermIf {
        test,
        then_label,
        else_label,
    }));

    let mut deps = Vec::new();
    let mut body_entry = body_setup.entry;
    body_entry.label = then_label;
    deps.push(body_entry);
    deps.extend(body_setup.deps);
    let mut orelse_entry = orelse_setup.entry;
    orelse_entry.label = else_label;
    deps.push(orelse_entry);
    deps.extend(orelse_setup.deps);

    Ok(InlineFragment::from_closed_builder(entry_label, entry, deps))
}

impl StmtLowerer for ast::StmtIf {
    fn simplify_ast(self, _context: &Context) -> Vec<Stmt> {
        stmts_from_rewrite(expand_if_chain(self))
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
        match simplify_stmt_head_ast_for_blockpy(context, Stmt::If(self.clone())).as_slice() {
            [Stmt::If(simplified_if)] => {
                let body = lower_nested_body_to_stmts_with_expr(
                    context,
                    &simplified_if.body,
                    loop_ctx,
                    next_label_id,
                )?;
                let orelse = lower_orelse_to_stmts_with_expr(
                    context,
                    &simplified_if.elif_else_clauses,
                    &Stmt::If(simplified_if.clone()),
                    loop_ctx,
                    next_label_id,
                )?;
                let test =
                    crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                        (*simplified_if.test).clone(),
                        out,
                        loop_ctx,
                        next_label_id,
                    )?;
                out.push_stmt(StructuredInstr::If(StructuredIf { test, body, orelse }));
                Ok(())
            }
            expanded => {
                for stmt in expanded {
                    lower_nested_stmt_into_with_expr(context, stmt, out, loop_ctx, next_label_id)?;
                }
                Ok(())
            }
        }
    }
}

fn lower_nested_body_to_stmts_with_expr<E>(
    context: &Context,
    body: &Suite,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<crate::block_py::BlockBuilder<StructuredInstr<E>, BlockTerm<E>>, String>
where
    E: RuffToBlockPyExpr,
{
    let mut out = crate::block_py::BlockBuilder::<StructuredInstr<E>, BlockTerm<E>>::new();
    for stmt in body {
        lower_nested_stmt_into_with_expr(context, stmt, &mut out, loop_ctx, next_label_id)?;
    }
    Ok(out.finish())
}

#[allow(dead_code)]
fn lower_nested_body_to_inline_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    body: &Suite,
    loop_ctx: Option<&LoopContext>,
    bridge: &mut StructuredLoweringBridge,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if !suite_is_plain_linear(context, body) {
        return None;
    }

    bridge
        .try_lower_inline_value::<E, ()>(|out, scratch_next_label_id| {
            for stmt in body {
                lower_nested_stmt_into_with_expr(
                    context,
                    stmt,
                    out,
                    loop_ctx,
                    scratch_next_label_id,
                )?;
            }
            Ok(())
        })
        .map(|result| {
            result.map(|(entry, ())| {
                InlineFragment::from_fallthrough_builder(name_gen.next_block_name(), entry, Vec::new())
            })
        })
}

fn lower_orelse_to_stmts_with_expr<E>(
    context: &Context,
    clauses: &[ast::ElifElseClause],
    stmt: &Stmt,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<crate::block_py::BlockBuilder<StructuredInstr<E>, BlockTerm<E>>, String>
where
    E: RuffToBlockPyExpr,
{
    match clauses {
        [] => Ok(crate::block_py::BlockBuilder::<
            StructuredInstr<E>,
            BlockTerm<E>,
        >::from_stmts(Vec::new())),
        [clause] if clause.test.is_none() => {
            lower_nested_body_to_stmts_with_expr(context, &clause.body, loop_ctx, next_label_id)
        }
        _ => Err(format!(
            "`elif` chain reached Ruff AST -> BlockPy conversion\nstmt:\n{}",
            ruff_ast_to_string(stmt).trim_end()
        )),
    }
}

#[allow(dead_code)]
fn lower_orelse_to_inline_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    clauses: &[ast::ElifElseClause],
    stmt: &Stmt,
    loop_ctx: Option<&LoopContext>,
    bridge: &mut StructuredLoweringBridge,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    match clauses {
        [] => Some(Ok(InlineFragment::new(
            Block::new(
                name_gen.next_block_name(),
                Vec::new(),
                BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                Vec::new(),
                None,
            ),
            Vec::new(),
        ))),
        [clause] if clause.test.is_none() && suite_is_plain_linear(context, &clause.body) => {
            lower_nested_body_to_inline_fragment(
                context,
                name_gen,
                &clause.body,
                loop_ctx,
                bridge,
            )
        }
        _ => Some(Err(format!(
            "`elif` chain reached inline Ruff fragment lowering\nstmt:\n{}",
            ruff_ast_to_string(stmt).trim_end()
        ))),
    }
}

fn suite_is_plain_linear(context: &Context, body: &[Stmt]) -> bool {
    matches!(
        crate::passes::ruff_to_blockpy::stmt_sequences::drive_stmt_sequence_until_control(
            context,
            body,
            Vec::new(),
        ),
        StmtSequenceDriveResult::Exhausted { .. }
    )
}

#[cfg(test)]
mod test;
