use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{
    Block, BlockBuilder, BlockLabel, BlockPyStmtBuilder, BlockTerm, Instr, Meta, Store,
    StructuredIf, StructuredInstr, TermIf, WithMeta,
};
use crate::passes::ruff_to_blockpy::expr_lowering::fresh_setup_name;
use crate::passes::ruff_to_blockpy::{FunctionNameGen, InlineFragment, LoopContext, LoweredExpr};
use crate::py_expr;
use ruff_python_ast::{self as ast, CmpOp, Expr};

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

fn empty_fragment<E>() -> BlockBuilder<StructuredInstr<E>, BlockTerm<E>>
where
    E: std::fmt::Debug + Instr,
{
    BlockBuilder::from_stmts(Vec::new())
}

fn close_inline_block<E>(label: BlockLabel, builder: BlockBuilder<E, BlockTerm<E>>) -> Block<E, E>
where
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let block = builder.finish();
    assert!(
        block.term.is_some(),
        "inline boolop/compare fragment block must have an explicit terminator"
    );
    Block::new(
        label,
        block.body,
        block.term.expect("checked explicit terminator"),
        Vec::new(),
        None,
    )
}

#[allow(dead_code)]
pub(crate) fn try_lower_branching_expr_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    expr: Expr,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<LoweredExpr<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    match expr {
        Expr::BoolOp(bool_op) => Some(lower_boolop_direct(lowerer, name_gen, bool_op, loop_ctx)),
        Expr::Compare(compare) if compare.ops.len() > 1 => {
            Some(lower_compare_chain_direct(lowerer, name_gen, compare, loop_ctx))
        }
        _ => None,
    }
}

fn lower_boolop_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    bool_op: ast::ExprBoolOp,
    loop_ctx: Option<&LoopContext>,
) -> Result<LoweredExpr<E>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let ast::ExprBoolOp { op, values, .. } = bool_op;
    let target = fresh_setup_name("target");
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let mut values = values.into_iter();
    let first = values.next().expect("bool op expects at least one value");
    let (mut entry, first) = bridge
        .try_lower_inline_value::<E, Expr>(|structured, scratch_next_label_id| {
            let first =
                lowerer.lower_expr_ast_into(first.clone(), structured, loop_ctx, scratch_next_label_id)?;
            structured.push_stmt(assign_name(&target, first.clone()));
            Ok(first)
        })
        .transpose()?
        .ok_or_else(|| "boolop setup still requires structured lowering".to_string())?;
    let _ = first;

    let mut dep_builders: Vec<(BlockLabel, BlockBuilder<E, BlockTerm<E>>)> = Vec::new();
    let mut current_dep_index: Option<usize> = None;
    for value in values {
        let next_label = name_gen.next_block_name();
        let test = match op {
            ast::BoolOp::And => load_name(&target),
            ast::BoolOp::Or => py_expr!("not {target:id}", target = target.as_str()),
        };
        match current_dep_index {
            None => entry.set_term(BlockTerm::IfTerm(TermIf {
                test: E::from_lowered_expr(test),
                then_label: next_label,
                else_label: BlockLabel::fallthrough(),
            })),
            Some(index) => dep_builders[index].1.set_term(BlockTerm::IfTerm(TermIf {
                test: E::from_lowered_expr(test),
                then_label: next_label,
                else_label: BlockLabel::fallthrough(),
            })),
        }

        let (next_entry, value) = bridge
            .try_lower_inline_value::<E, Expr>(|structured, scratch_next_label_id| {
                let value = lowerer.lower_expr_ast_into(
                    value.clone(),
                    structured,
                    loop_ctx,
                    scratch_next_label_id,
                )?;
                structured.push_stmt(assign_name(&target, value.clone()));
                Ok(value)
            })
            .transpose()?
            .ok_or_else(|| "boolop step still requires structured lowering".to_string())?;
        let _ = value;
        dep_builders.push((next_label, next_entry));
        current_dep_index = Some(dep_builders.len() - 1);
    }

    match current_dep_index {
        None => {
            entry.ensure_fallthrough_term();
        }
        Some(index) => {
            dep_builders[index].1.ensure_fallthrough_term();
        }
    }

    let deps = dep_builders
        .into_iter()
        .map(|(label, builder)| close_inline_block(label, builder))
        .collect();

    Ok(LoweredExpr {
        setup: InlineFragment::from_closed_builder(name_gen.next_block_name(), entry, deps),
        value: E::from_lowered_expr(load_name(&target)),
    })
}

fn lower_compare_chain_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    compare: ast::ExprCompare,
    loop_ctx: Option<&LoopContext>,
) -> Result<LoweredExpr<E>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let ast::ExprCompare {
        left,
        ops,
        comparators,
        ..
    } = compare;
    let compare_name = fresh_setup_name("compare");
    let target_name = fresh_setup_name("target");
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let ops = ops.into_vec();
    let comparators = comparators.into_vec();
    let mut steps = ops.into_iter().zip(comparators.into_iter()).peekable();
    let Some((first_op, first_comparator_expr)) = steps.next() else {
        unreachable!("compare chain should contain at least one step");
    };
    let first_has_more = steps.peek().is_some();

    let (mut entry, (_initial_left, first_comparator)) = bridge
        .try_lower_inline_value::<E, (Expr, Expr)>(|structured, scratch_next_label_id| {
            let current_left = lowerer.lower_expr_ast_into(
                (*left).clone(),
                structured,
                loop_ctx,
                scratch_next_label_id,
            )?;
            structured.push_stmt(assign_name(&compare_name, current_left.clone()));
            let mut first_comparator = lowerer.lower_expr_ast_into(
                first_comparator_expr.clone(),
                structured,
                loop_ctx,
                scratch_next_label_id,
            )?;
            if first_has_more {
                let tmp_name = fresh_setup_name("compare");
                structured.push_stmt(assign_name(&tmp_name, first_comparator.clone()));
                first_comparator = load_name(&tmp_name);
            }
            structured.push_stmt(assign_name(
                &target_name,
                compare_expr(first_op, load_name(&compare_name), first_comparator.clone()),
            ));
            Ok((load_name(&compare_name), first_comparator))
        })
        .transpose()?
        .ok_or_else(|| "compare setup still requires structured lowering".to_string())?;
    let mut current_left = first_comparator.clone();

    let mut dep_builders: Vec<(BlockLabel, BlockBuilder<E, BlockTerm<E>>)> = Vec::new();
    let mut current_dep_index: Option<usize> = None;

    while let Some((op, comparator)) = steps.next() {
        let next_label = name_gen.next_block_name();
        match current_dep_index {
            None => entry.set_term(BlockTerm::IfTerm(TermIf {
                test: E::from_lowered_expr(load_name(&target_name)),
                then_label: next_label,
                else_label: BlockLabel::fallthrough(),
            })),
            Some(index) => dep_builders[index].1.set_term(BlockTerm::IfTerm(TermIf {
                test: E::from_lowered_expr(load_name(&target_name)),
                then_label: next_label,
                else_label: BlockLabel::fallthrough(),
            })),
        }

        let has_more = steps.peek().is_some();
        let current_left_for_step = current_left.clone();
        let (next_entry, comparator_expr) = bridge
            .try_lower_inline_value::<E, Expr>(|structured, scratch_next_label_id| {
                let mut comparator_expr = lowerer.lower_expr_ast_into(
                    comparator.clone(),
                    structured,
                    loop_ctx,
                    scratch_next_label_id,
                )?;
                if has_more {
                    let tmp_name = fresh_setup_name("compare");
                    structured.push_stmt(assign_name(&tmp_name, comparator_expr.clone()));
                    comparator_expr = load_name(&tmp_name);
                }
                structured.push_stmt(assign_name(
                    &target_name,
                    compare_expr(op, current_left_for_step.clone(), comparator_expr.clone()),
                ));
                Ok(comparator_expr)
            })
            .transpose()?
            .ok_or_else(|| "compare step still requires structured lowering".to_string())?;

        current_left = comparator_expr.clone();
        dep_builders.push((next_label, next_entry));
        current_dep_index = Some(dep_builders.len() - 1);
    }

    match current_dep_index {
        None => {
            entry.ensure_fallthrough_term();
        }
        Some(index) => {
            dep_builders[index].1.ensure_fallthrough_term();
        }
    }

    let deps = dep_builders
        .into_iter()
        .map(|(label, builder)| close_inline_block(label, builder))
        .collect();

    Ok(LoweredExpr {
        setup: InlineFragment::from_closed_builder(name_gen.next_block_name(), entry, deps),
        value: E::from_lowered_expr(load_name(&target_name)),
    })
}

pub(super) fn lower_boolop_into<L, E>(
    lowerer: &L,
    bool_op: ast::ExprBoolOp,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<Expr, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let ast::ExprBoolOp { op, values, .. } = bool_op;
    let target = fresh_setup_name("target");
    let mut values = values.into_iter();
    let first = values.next().expect("bool op expects at least one value");
    let first = lowerer.lower_expr_ast_into(first, out, loop_ctx, next_label_id)?;
    out.push_stmt(assign_name(&target, first));

    for value in values {
        let mut body = BlockPyStmtBuilder::<E>::new();
        let value = lowerer.lower_expr_ast_into(value, &mut body, loop_ctx, next_label_id)?;
        body.push_stmt(assign_name(&target, value));
        let test = match op {
            ast::BoolOp::And => load_name(&target),
            ast::BoolOp::Or => py_expr!("not {target:id}", target = target.as_str()),
        };
        out.push_stmt(StructuredInstr::If(StructuredIf {
            test: test.into(),
            body: body.finish(),
            orelse: empty_fragment(),
        }));
    }

    Ok(load_name(&target))
}

pub(super) fn lower_compare_into<L, E>(
    lowerer: &L,
    compare: ast::ExprCompare,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
    next_label_id: &mut usize,
) -> Result<Expr, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let ast::ExprCompare {
        left,
        ops,
        comparators,
        ..
    } = compare;

    let ops = ops.into_vec();
    let comparators = comparators.into_vec();
    if ops.len() == 1 {
        let left = lowerer.lower_expr_ast_into(*left, out, loop_ctx, next_label_id)?;
        let right = lowerer.lower_expr_ast_into(
            comparators.into_iter().next().expect("single comparator"),
            out,
            loop_ctx,
            next_label_id,
        )?;
        return Ok(compare_expr(ops[0], left, right));
    }

    let compare_name = fresh_setup_name("compare");
    let mut current_left = lowerer.lower_expr_ast_into(*left, out, loop_ctx, next_label_id)?;
    out.push_stmt(assign_name(&compare_name, current_left));
    current_left = load_name(&compare_name);

    let target_name = fresh_setup_name("target");
    let mut steps = ops.into_iter().zip(comparators.into_iter()).peekable();
    let Some((first_op, first_comparator)) = steps.next() else {
        unreachable!("compare chain should contain at least one step");
    };
    let mut first_comparator =
        lowerer.lower_expr_ast_into(first_comparator, out, loop_ctx, next_label_id)?;
    if steps.peek().is_some() {
        let tmp_name = fresh_setup_name("compare");
        out.push_stmt(assign_name(&tmp_name, first_comparator));
        first_comparator = load_name(&tmp_name);
    }
    out.push_stmt(assign_name(
        &target_name,
        compare_expr(first_op, current_left.clone(), first_comparator.clone()),
    ));
    current_left = first_comparator;

    while let Some((op, comparator)) = steps.next() {
        let mut step_body = BlockPyStmtBuilder::<E>::new();
        let mut comparator_expr =
            lowerer.lower_expr_ast_into(comparator, &mut step_body, loop_ctx, next_label_id)?;
        if steps.peek().is_some() {
            let tmp_name = fresh_setup_name("compare");
            step_body.push_stmt(assign_name(&tmp_name, comparator_expr));
            comparator_expr = load_name(&tmp_name);
        }
        step_body.push_stmt(assign_name(
            &target_name,
            compare_expr(op, current_left.clone(), comparator_expr.clone()),
        ));
        current_left = comparator_expr;
        out.push_stmt(StructuredInstr::If(StructuredIf {
            test: load_name(&target_name).into(),
            body: step_body.finish(),
            orelse: empty_fragment(),
        }));
    }

    Ok(load_name(&target_name))
}

fn compare_expr(op: CmpOp, left: Expr, right: Expr) -> Expr {
    Expr::Compare(ast::ExprCompare {
        left: Box::new(left),
        ops: vec![op].into(),
        comparators: vec![right].into(),
        range: Default::default(),
        node_index: Default::default(),
    })
}

#[cfg(test)]
mod test;
