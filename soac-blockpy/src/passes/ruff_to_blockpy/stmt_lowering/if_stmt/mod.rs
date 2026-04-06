use super::*;
use crate::block_py::TermIf;
use crate::passes::ast_to_ast::ast_rewrite::Rewrite;
use crate::passes::ast_to_ast::body::Suite;
use crate::passes::InstrRuff;
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

pub(crate) fn try_lower_if_instr_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    if_stmt: &crate::block_py::StmtIf<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    Some(lower_simplified_if_instr_fragment(
        context,
        name_gen,
        if_stmt,
        loop_ctx,
    ))
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
    let bridge = StructuredLoweringBridge::new();
    let Some(test_setup) = bridge.try_lower_inline_value(
        name_gen,
        |structured| {
            crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                InstrRuff::from_ast_expr(*test.clone()),
                structured,
                loop_ctx,
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
            &bridge,
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
        &bridge,
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

    let (fragment_entry_label, mut blocks) = entry.finish_blocks();
    let mut body_setup = body_setup;
    body_setup.relabel_entry(then_label);
    blocks.push(body_setup.entry);
    blocks.extend(body_setup.deps);
    let mut orelse_setup = orelse_setup;
    orelse_setup.relabel_entry(else_label);
    blocks.push(orelse_setup.entry);
    blocks.extend(orelse_setup.deps);

    let entry_index = blocks
        .iter()
        .position(|block| block.label == fragment_entry_label)
        .expect("if fragment entry label should be present in assembled blocks");
    let entry = blocks.remove(entry_index);
    let mut fragment = InlineFragment::new(entry, blocks);
    fragment.relabel_entry(entry_label);
    Ok(fragment)
}

fn lower_simplified_if_instr_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    if_stmt: &crate::block_py::StmtIf<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InlineFragment<E>, String>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let bridge = StructuredLoweringBridge::new();
    let Some(test_setup) = bridge.try_lower_inline_value(name_gen, |structured| {
        crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
            (*if_stmt.test).clone(),
            structured,
            loop_ctx,
        )
    }) else {
        return Err("if-test setup still requires structured lowering".to_string());
    };
    let (mut entry, test) = test_setup?;
    let entry_label = name_gen.next_block_name();

    let Some(body_setup) = lower_nested_instr_body_to_inline_fragment(
        context,
        name_gen,
        &if_stmt.body,
        loop_ctx,
        &bridge,
    ) else {
        return Err("if-body still requires structured lowering".to_string());
    };
    let body_setup = body_setup?;

    let Some(orelse_setup) = lower_instr_orelse_to_inline_fragment(
        context,
        name_gen,
        &if_stmt.orelse,
        loop_ctx,
        &bridge,
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

    let (fragment_entry_label, mut blocks) = entry.finish_blocks();
    let mut body_setup = body_setup;
    body_setup.relabel_entry(then_label);
    blocks.push(body_setup.entry);
    blocks.extend(body_setup.deps);
    let mut orelse_setup = orelse_setup;
    orelse_setup.relabel_entry(else_label);
    blocks.push(orelse_setup.entry);
    blocks.extend(orelse_setup.deps);

    let entry_index = blocks
        .iter()
        .position(|block| block.label == fragment_entry_label)
        .expect("if fragment entry label should be present in assembled blocks");
    let entry = blocks.remove(entry_index);
    let mut fragment = InlineFragment::new(entry, blocks);
    fragment.relabel_entry(entry_label);
    Ok(fragment)
}

#[allow(dead_code)]
fn lower_nested_body_to_inline_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    body: &Suite,
    loop_ctx: Option<&LoopContext>,
    bridge: &StructuredLoweringBridge,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if !suite_is_inline_fragment_compatible(context, body) {
        return None;
    }

    bridge
        .try_lower_inline_value::<E, ()>(name_gen, |out| {
            for stmt in body {
                lower_nested_stmt_into_with_expr(
                    context,
                    name_gen,
                    stmt,
                    out,
                    loop_ctx,
                )?;
            }
            Ok(())
        })
        .map(|result| {
            result.map(|(entry, ())| {
                entry.finish_fallthrough()
            })
        })
}

fn lower_nested_instr_body_to_inline_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    body: &[InstrRuff],
    loop_ctx: Option<&LoopContext>,
    bridge: &StructuredLoweringBridge,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if !instr_suite_is_inline_fragment_compatible(context, body) {
        return None;
    }

    bridge
        .try_lower_inline_value::<E, ()>(name_gen, |out| {
            for stmt in body {
                lower_instr_into_with_expr(context, stmt, name_gen, out, loop_ctx)?;
            }
            Ok(())
        })
        .map(|result| result.map(|(entry, ())| entry.finish_fallthrough()))
}

#[allow(dead_code)]
fn lower_orelse_to_inline_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    clauses: &[ast::ElifElseClause],
    stmt: &Stmt,
    loop_ctx: Option<&LoopContext>,
    bridge: &StructuredLoweringBridge,
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
        [clause] if clause.test.is_none() => {
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

fn lower_instr_orelse_to_inline_fragment<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    orelse: &[InstrRuff],
    loop_ctx: Option<&LoopContext>,
    bridge: &StructuredLoweringBridge,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if orelse.is_empty() {
        return Some(Ok(InlineFragment::new(
            Block::new(
                name_gen.next_block_name(),
                Vec::new(),
                BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                Vec::new(),
                None,
            ),
            Vec::new(),
        )));
    }

    lower_nested_instr_body_to_inline_fragment(context, name_gen, orelse, loop_ctx, bridge)
}

fn suite_is_inline_fragment_compatible(context: &Context, body: &[Stmt]) -> bool {
    body.iter()
        .all(|stmt| stmt_is_inline_fragment_compatible(context, stmt))
}

fn stmt_is_inline_fragment_compatible(context: &Context, stmt: &Stmt) -> bool {
    let simplified = if should_simplify_nested_stmt_head(stmt) {
        simplify_stmt_head_ast_for_blockpy(context, stmt.clone())
    } else {
        vec![stmt.clone()]
    };

    simplified.into_iter().all(|stmt| match stmt {
        Stmt::Expr(_)
        | Stmt::Pass(_)
        | Stmt::Assign(_)
        | Stmt::Delete(_)
        | Stmt::Global(_)
        | Stmt::Nonlocal(_)
        | Stmt::AugAssign(_)
        | Stmt::TypeAlias(_)
        | Stmt::Import(_)
        | Stmt::ImportFrom(_)
        | Stmt::Return(_)
        | Stmt::Raise(_) => true,
        Stmt::Break(_) | Stmt::Continue(_) => false,
        Stmt::If(if_stmt) => {
            suite_is_inline_fragment_compatible(context, &if_stmt.body)
                && if_stmt.elif_else_clauses.iter().all(|clause| {
                    clause.test.is_none()
                        && suite_is_inline_fragment_compatible(context, &clause.body)
                })
        }
        _ => false,
    })
}

fn instr_suite_is_inline_fragment_compatible(context: &Context, body: &[InstrRuff]) -> bool {
    body.iter()
        .all(|stmt| instr_stmt_is_inline_fragment_compatible(context, stmt))
}

fn instr_stmt_is_inline_fragment_compatible(context: &Context, stmt: &InstrRuff) -> bool {
    let simplified = simplify_instr_head_for_blockpy(context, stmt.clone());

    simplified.into_iter().all(|stmt| match stmt {
        InstrRuff::StmtExpr(_)
        | InstrRuff::StmtPass(_)
        | InstrRuff::StmtAssign(_)
        | InstrRuff::StmtDelete(_)
        | InstrRuff::StmtGlobal(_)
        | InstrRuff::StmtNonlocal(_)
        | InstrRuff::StmtAugAssign(_)
        | InstrRuff::StmtTypeAlias(_)
        | InstrRuff::StmtImport(_)
        | InstrRuff::StmtImportFrom(_)
        | InstrRuff::StmtReturn(_)
        | InstrRuff::StmtRaise(_) => true,
        InstrRuff::StmtBreak(_) | InstrRuff::StmtContinue(_) => false,
        InstrRuff::StmtIf(if_stmt) => {
            instr_suite_is_inline_fragment_compatible(context, &if_stmt.body)
                && instr_suite_is_inline_fragment_compatible(context, &if_stmt.orelse)
        }
        _ => false,
    })
}

#[cfg(test)]
mod test;
