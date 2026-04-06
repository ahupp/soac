use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{BlockLabel, BlockTerm, Meta, Store, TermIf, WithMeta};
use crate::passes::InstrRuff;
use crate::passes::ruff_to_blockpy::expr_lowering::fresh_setup_name;
use crate::passes::ruff_to_blockpy::{FunctionNameGen, InlineFragment, LoopContext, LoweredExpr};
use crate::py_expr;
use ruff_python_ast::{self as ast, CmpOp};

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

fn relabel_blocks_entry<E>(
    blocks: &mut [crate::block_py::Block<E, E>],
    old_label: BlockLabel,
    new_label: BlockLabel,
) where
    E: crate::block_py::Instr,
{
    if old_label == new_label {
        return;
    }
    for block in blocks {
        if block.label == old_label {
            block.label = new_label;
        }
        block.term.replace_target(old_label, new_label);
        if let Some(edge) = &mut block.exc_edge {
            if edge.target == old_label {
                edge.target = new_label;
            }
        }
    }
}

#[allow(dead_code)]
pub(crate) fn try_lower_branching_expr_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<LoweredExpr<E, InstrRuff>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    match expr {
        InstrRuff::ExprBoolOp(bool_op) => Some(lower_boolop_direct(lowerer, name_gen, bool_op, loop_ctx)),
        InstrRuff::ExprCompare(compare) if compare.ops.len() > 1 => {
            Some(lower_compare_chain_direct(lowerer, name_gen, compare, loop_ctx))
        }
        _ => None,
    }
}

fn lower_boolop_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Result<LoweredExpr<E, InstrRuff>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let crate::block_py::ExprBoolOp { op, values, .. } = bool_op;
    let target = fresh_setup_name("target");
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let mut values = values.into_iter();
    let first = values.next().expect("bool op expects at least one value");
    let (mut entry, first) = bridge
        .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
            let first = lowerer.lower_expr_instr_into(first.clone(), structured, loop_ctx)?;
            structured.push_stmt(assign_name(&target, first.clone()));
            Ok(first)
        })
        .transpose()?
        .ok_or_else(|| "boolop setup still requires structured lowering".to_string())?;
    let _ = first;

    let mut dep_builders: Vec<(BlockLabel, crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>)> = Vec::new();
    let mut current_dep_index: Option<usize> = None;
    for value in values {
        let next_label = name_gen.next_block_name();
        let test = match op {
            ast::BoolOp::And => load_name(&target),
            ast::BoolOp::Or => {
                InstrRuff::from_ast_expr(py_expr!("not {target:id}", target = target.as_str()))
            }
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
            .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
                let value = lowerer.lower_expr_instr_into(value.clone(), structured, loop_ctx)?;
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
        None => entry.ensure_fallthrough_term(),
        Some(index) => dep_builders[index].1.ensure_fallthrough_term(),
    }

    let (setup_entry_label, mut setup_blocks) = entry.finish_fallthrough_blocks();
    for (label, builder) in dep_builders {
        let (entry_label, mut blocks) = builder.finish_fallthrough_blocks();
        relabel_blocks_entry(&mut blocks, entry_label, label);
        setup_blocks.extend(blocks);
    }
    let setup_entry_index = setup_blocks
        .iter()
        .position(|block| block.label == setup_entry_label)
        .expect("boolop setup entry label should be present in assembled blocks");
    let setup_entry = setup_blocks.remove(setup_entry_index);
    Ok(LoweredExpr {
        setup: InlineFragment::new(setup_entry, setup_blocks),
        value: load_name(&target),
    })
}

fn lower_compare_chain_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    compare: crate::block_py::ExprCompare<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Result<LoweredExpr<E, InstrRuff>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    let crate::block_py::ExprCompare {
        left,
        ops,
        comparators,
        ..
    } = compare;
    let compare_name = fresh_setup_name("compare");
    let target_name = fresh_setup_name("target");
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let mut steps = ops.into_iter().zip(comparators.into_iter()).peekable();
    let Some((first_op, first_comparator_expr)) = steps.next() else {
        unreachable!("compare chain should contain at least one step");
    };
    let first_has_more = steps.peek().is_some();

    let (mut entry, (_initial_left, first_comparator)) = bridge
        .try_lower_inline_value::<E, (InstrRuff, InstrRuff)>(name_gen, |structured| {
                let current_left =
                    lowerer.lower_expr_instr_into((*left).clone(), structured, loop_ctx)?;
                structured.push_stmt(assign_name(&compare_name, current_left.clone()));
                let mut first_comparator =
                    lowerer.lower_expr_instr_into(first_comparator_expr.clone(), structured, loop_ctx)?;
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
            },
        )
        .transpose()?
        .ok_or_else(|| "compare setup still requires structured lowering".to_string())?;
    let mut current_left = first_comparator.clone();

    let mut dep_builders: Vec<(BlockLabel, crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>)> = Vec::new();
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
            .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
                let mut comparator_expr =
                    lowerer.lower_expr_instr_into(comparator.clone(), structured, loop_ctx)?;
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
        None => entry.ensure_fallthrough_term(),
        Some(index) => dep_builders[index].1.ensure_fallthrough_term(),
    }

    let (setup_entry_label, mut setup_blocks) = entry.finish_fallthrough_blocks();
    for (label, builder) in dep_builders {
        let (entry_label, mut blocks) = builder.finish_fallthrough_blocks();
        relabel_blocks_entry(&mut blocks, entry_label, label);
        setup_blocks.extend(blocks);
    }
    let setup_entry_index = setup_blocks
        .iter()
        .position(|block| block.label == setup_entry_label)
        .expect("compare setup entry label should be present in assembled blocks");
    let setup_entry = setup_blocks.remove(setup_entry_index);
    Ok(LoweredExpr {
        setup: InlineFragment::new(setup_entry, setup_blocks),
        value: load_name(&target_name),
    })
}

pub(super) fn lower_boolop_into<L, E>(
    lowerer: &L,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    out: &mut crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if let Some(lowered) = try_lower_branching_expr_direct(lowerer, out.name_gen(), InstrRuff::ExprBoolOp(bool_op), loop_ctx) {
        let lowered = lowered?;
        out.append_fragment(lowered.setup);
        return Ok(lowered.value);
    }
    Err("boolop lowering requires inline fragment lowering".to_string())
}

pub(super) fn lower_compare_into<L, E>(
    lowerer: &L,
    compare: crate::block_py::ExprCompare<InstrRuff>,
    out: &mut crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr + crate::block_py::ImplicitNoneExpr,
{
    if compare.ops.len() == 1 {
        let crate::block_py::ExprCompare {
            left,
            ops,
            comparators,
            ..
        } = compare;
        let left = lowerer.lower_expr_instr_into(*left, out, loop_ctx)?;
        let right = lowerer.lower_expr_instr_into(
            comparators.into_iter().next().expect("single comparator"),
            out,
            loop_ctx,
        )?;
        return Ok(compare_expr(ops[0], left, right));
    }
    if let Some(lowered) = try_lower_branching_expr_direct(lowerer, out.name_gen(), InstrRuff::ExprCompare(compare), loop_ctx) {
        let lowered = lowered?;
        out.append_fragment(lowered.setup);
        return Ok(lowered.value);
    }
    Err("compare chain lowering requires inline fragment lowering".to_string())
}

fn compare_expr(op: CmpOp, left: InstrRuff, right: InstrRuff) -> InstrRuff {
    InstrRuff::ExprCompare(
        crate::block_py::ExprCompare::new(left, vec![op], vec![right]).with_meta(Meta::default()),
    )
}

#[cfg(test)]
mod test;
