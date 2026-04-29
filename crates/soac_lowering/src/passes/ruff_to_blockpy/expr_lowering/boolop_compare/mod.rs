use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockParam, BlockParamRole, BlockTerm, Meta, Store,
    TermIf, TermRaise, WithMeta,
};
use crate::passes::ruff_to_blockpy::expr_lowering::fresh_setup_name;
use crate::passes::ruff_to_blockpy::{
    FunctionNameGen, InlineBlockRef, InlineFragment, LoopContext, LoweredExpr,
};
use crate::passes::InstrRuff;
use crate::template::py_expr;
use ruff_python_ast::{self as ast, CmpOp};

fn store_name(name: &str) -> ast::name::Name {
    name.into()
}

fn load_name(name: &str) -> InstrRuff {
    crate::passes::ast_to_instr::from_ast_expr(py_expr!("{name:id}", name = name))
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
pub(crate) fn try_lower_branching_expr_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<LoweredExpr<E, InstrRuff>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    match expr {
        InstrRuff::ExprBoolOp(bool_op) => {
            Some(lower_boolop_direct(lowerer, name_gen, bool_op, loop_ctx))
        }
        InstrRuff::ExprCompare(compare) if compare.ops.len() > 1 => Some(
            lower_compare_chain_direct(lowerer, name_gen, compare, loop_ctx),
        ),
        _ => None,
    }
}

pub(crate) fn try_lower_branching_expr_branch_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
    then_label: BlockLabel,
    else_label: BlockLabel,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    match expr {
        InstrRuff::ExprBoolOp(bool_op) => Some(lower_boolop_branch_direct(
            lowerer, name_gen, bool_op, then_label, else_label, loop_ctx,
        )),
        InstrRuff::ExprCompare(compare) if compare.ops.len() > 1 => {
            Some(lower_compare_chain_branch_direct(
                lowerer, name_gen, compare, then_label, else_label, loop_ctx,
            ))
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
    E: RuffToBlockPyExpr,
{
    if let Some(lowered) =
        lower_boolop_value_param_direct(lowerer, name_gen, bool_op.clone(), loop_ctx)
    {
        return lowered;
    }

    let crate::block_py::ExprBoolOp { op, values, .. } = bool_op;
    let target = fresh_setup_name("target");
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let mut values = values.into_iter();
    let first = values.next().expect("bool op expects at least one value");
    let (entry, first) = bridge
        .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
            let first = lowerer.lower_expr_instr_into(first.clone(), structured, loop_ctx)?;
            structured.push_stmt(assign_name(&target, first.clone()));
            Ok(first)
        })
        .transpose()?
        .ok_or_else(|| "boolop setup still requires structured lowering".to_string())?;
    let _ = first;

    let mut fragments = Vec::new();
    let mut current_builder = entry;
    for value in values {
        let test = match op {
            ast::BoolOp::And => load_name(&target),
            ast::BoolOp::Or => crate::passes::ast_to_instr::from_ast_expr(py_expr!(
                "not {target:id}",
                target = target.as_str()
            )),
        };
        let (next_entry, value) = bridge
            .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
                let value = lowerer.lower_expr_instr_into(value.clone(), structured, loop_ctx)?;
                structured.push_stmt(assign_name(&target, value.clone()));
                Ok(value)
            })
            .transpose()?
            .ok_or_else(|| "boolop step still requires structured lowering".to_string())?;
        let _ = value;
        current_builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(test),
            then_label: next_entry.entry_ref().label(),
            else_label: BlockLabel::fallthrough(),
        }));
        fragments.push(current_builder.finish_blocks());
        current_builder = next_entry;
    }

    current_builder.ensure_fallthrough_term();
    fragments.push(current_builder.finish_blocks());

    let mut fragments = fragments.into_iter();
    let (setup_entry_ref, mut setup_blocks) = fragments
        .next()
        .expect("boolop setup should produce at least one fragment");
    for (_, mut blocks) in fragments {
        setup_blocks.append(&mut blocks);
    }
    let setup_entry_index = setup_blocks
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .expect("boolop setup entry label should be present in assembled blocks");
    let setup_entry = setup_blocks.remove(setup_entry_index);
    Ok(LoweredExpr {
        setup: InlineFragment::new(setup_entry, setup_blocks),
        value: load_name(&target),
    })
}

fn forwardable_name_arg<L>(lowerer: &L, value: &InstrRuff) -> Option<String>
where
    L: BlockPySetupExprLowerer + ?Sized,
{
    let InstrRuff::ExprName(name) = value else {
        return None;
    };
    let name = name.id.as_str();
    lowerer
        .can_forward_name_value(name)
        .then(|| name.to_string())
}

fn inline_fragment_from_fragments<E>(
    fragments: Vec<(InlineBlockRef, Vec<Block<E>>)>,
    context: &str,
    external_targets: &[BlockLabel],
) -> InlineFragment<E>
where
    E: RuffToBlockPyExpr,
{
    let mut fragments = fragments.into_iter();
    let (setup_entry_ref, mut setup_blocks) = fragments
        .next()
        .unwrap_or_else(|| panic!("{context} should produce at least one fragment"));
    for (_, mut blocks) in fragments {
        setup_blocks.append(&mut blocks);
    }
    let setup_entry_index = setup_blocks
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .unwrap_or_else(|| panic!("{context} entry label should be present in assembled blocks"));
    let setup_entry = setup_blocks.remove(setup_entry_index);
    InlineFragment::new_with_external_targets(setup_entry, setup_blocks, external_targets)
}

fn lower_boolop_value_param_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<LoweredExpr<E, InstrRuff>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprBoolOp { op, values, .. } = bool_op;
    if values.is_empty() {
        return Some(Err("bool op expects at least one value".to_string()));
    }

    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let join_name = fresh_setup_name("value");
    let lowered_values = match lower_boolop_values(lowerer, &bridge, name_gen, values, loop_ctx) {
        Ok(values) => values,
        Err(err) => return Some(Err(err)),
    };
    let join_label = name_gen.next_block_name();
    let value_count = lowered_values.len();
    let entries = lowered_values
        .iter()
        .map(|(builder, _)| builder.entry_ref().label())
        .collect::<Vec<_>>();

    let mut fragments = Vec::new();
    for (index, (mut builder, lowered_value)) in lowered_values.into_iter().enumerate() {
        if index + 1 == value_count {
            let arg_name = if let Some(arg_name) = forwardable_name_arg(lowerer, &lowered_value) {
                arg_name
            } else {
                builder.push_stmt(assign_name(&join_name, lowered_value));
                join_name.clone()
            };
            builder.set_term(BlockTerm::Jump(BlockEdge::with_args(
                join_label,
                vec![BlockArg::Name(arg_name)],
            )));
            fragments.push(builder.finish_blocks());
            break;
        }

        let Some(arg_name) = forwardable_name_arg(lowerer, &lowered_value) else {
            return None;
        };
        let selected_edge = || {
            BlockTerm::Jump(BlockEdge::with_args(
                join_label,
                vec![BlockArg::Name(arg_name.clone())],
            ))
        };
        let selected_label = name_gen.next_block_name();
        let next_label = entries[index + 1];
        let (truthy_label, falsey_label) = match op {
            ast::BoolOp::And => (next_label, selected_label),
            ast::BoolOp::Or => (selected_label, next_label),
        };
        builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(lowered_value),
            then_label: truthy_label,
            else_label: falsey_label,
        }));
        fragments.push(builder.finish_blocks());
        fragments.push((
            crate::passes::ruff_to_blockpy::InlineBlockRef::from_label(selected_label),
            vec![Block::new(
                selected_label,
                Vec::new(),
                selected_edge(),
                Vec::new(),
                None,
            )],
        ));
    }

    fragments.push((
        crate::passes::ruff_to_blockpy::InlineBlockRef::from_label(join_label),
        vec![Block::new(
            join_label,
            Vec::new(),
            BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
            vec![BlockParam {
                name: join_name.clone(),
                role: BlockParamRole::Value,
            }],
            None,
        )],
    ));

    Some(Ok(LoweredExpr {
        setup: inline_fragment_from_fragments(fragments, "boolop value-param setup", &[]),
        value: load_name(&join_name),
    }))
}

pub(crate) fn try_lower_boolop_assign_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    target: &str,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprBoolOp { op, values, .. } = bool_op;
    if values.is_empty() {
        return Some(Err("bool op expects at least one value".to_string()));
    }

    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let lowered_values = match lower_boolop_values(lowerer, &bridge, name_gen, values, loop_ctx) {
        Ok(values) => values,
        Err(err) => return Some(Err(err)),
    };
    let value_count = lowered_values.len();
    let entries = lowered_values
        .iter()
        .map(|(builder, _)| builder.entry_ref().label())
        .collect::<Vec<_>>();

    let mut fragments = Vec::new();
    for (index, (mut builder, lowered_value)) in lowered_values.into_iter().enumerate() {
        if index + 1 == value_count {
            builder.push_stmt(assign_name(target, lowered_value));
            builder.ensure_fallthrough_term();
            fragments.push(builder.finish_blocks());
            break;
        }

        let Some(selected_name) = forwardable_name_arg(lowerer, &lowered_value) else {
            return None;
        };
        let selected_label = name_gen.next_block_name();
        let next_label = entries[index + 1];
        let (truthy_label, falsey_label) = match op {
            ast::BoolOp::And => (next_label, selected_label),
            ast::BoolOp::Or => (selected_label, next_label),
        };
        builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(lowered_value),
            then_label: truthy_label,
            else_label: falsey_label,
        }));
        fragments.push(builder.finish_blocks());
        fragments.push((
            InlineBlockRef::from_label(selected_label),
            vec![Block::new(
                selected_label,
                vec![assign_name(target, load_name(&selected_name))],
                BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                Vec::new(),
                None,
            )],
        ));
    }

    Some(Ok(inline_fragment_from_fragments(
        fragments,
        "boolop assignment setup",
        &[],
    )))
}

#[derive(Clone, Copy)]
enum BoolopTerminalKind {
    Return,
    Raise,
}

fn terminal_term<E>(kind: BoolopTerminalKind, value: InstrRuff) -> BlockTerm<E>
where
    E: RuffToBlockPyExpr,
{
    match kind {
        BoolopTerminalKind::Return => BlockTerm::Return(E::from_lowered_expr(value)),
        BoolopTerminalKind::Raise => BlockTerm::Raise(TermRaise {
            exc: Some(E::from_lowered_expr(value)),
        }),
    }
}

pub(crate) fn try_lower_boolop_return_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    try_lower_boolop_terminal_direct(
        lowerer,
        name_gen,
        bool_op,
        loop_ctx,
        BoolopTerminalKind::Return,
    )
}

pub(crate) fn try_lower_boolop_raise_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    try_lower_boolop_terminal_direct(
        lowerer,
        name_gen,
        bool_op,
        loop_ctx,
        BoolopTerminalKind::Raise,
    )
}

fn try_lower_boolop_terminal_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
    kind: BoolopTerminalKind,
) -> Option<Result<InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprBoolOp { op, values, .. } = bool_op;
    if values.is_empty() {
        return Some(Err("bool op expects at least one value".to_string()));
    }

    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let lowered_values = match lower_boolop_values(lowerer, &bridge, name_gen, values, loop_ctx) {
        Ok(values) => values,
        Err(err) => return Some(Err(err)),
    };
    let value_count = lowered_values.len();
    let entries = lowered_values
        .iter()
        .map(|(builder, _)| builder.entry_ref().label())
        .collect::<Vec<_>>();

    let mut fragments = Vec::new();
    for (index, (mut builder, lowered_value)) in lowered_values.into_iter().enumerate() {
        if index + 1 == value_count {
            builder.set_term(terminal_term(kind, lowered_value));
            fragments.push(builder.finish_blocks());
            break;
        }

        let Some(selected_name) = forwardable_name_arg(lowerer, &lowered_value) else {
            return None;
        };
        let selected_label = name_gen.next_block_name();
        let next_label = entries[index + 1];
        let (truthy_label, falsey_label) = match op {
            ast::BoolOp::And => (next_label, selected_label),
            ast::BoolOp::Or => (selected_label, next_label),
        };
        builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(lowered_value),
            then_label: truthy_label,
            else_label: falsey_label,
        }));
        fragments.push(builder.finish_blocks());
        fragments.push((
            InlineBlockRef::from_label(selected_label),
            vec![Block::new(
                selected_label,
                Vec::new(),
                terminal_term(kind, load_name(&selected_name)),
                Vec::new(),
                None,
            )],
        ));
    }

    Some(Ok(inline_fragment_from_fragments(
        fragments,
        "boolop terminal setup",
        &[],
    )))
}

fn lower_boolop_values<L, E>(
    lowerer: &L,
    bridge: &crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge,
    name_gen: &FunctionNameGen,
    values: Vec<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Result<
    Vec<(
        crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>,
        InstrRuff,
    )>,
    String,
>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    values
        .into_iter()
        .map(|value| {
            bridge
                .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
                    lowerer.lower_expr_instr_into(value.clone(), structured, loop_ctx)
                })
                .transpose()?
                .ok_or_else(|| "boolop setup still requires structured lowering".to_string())
        })
        .collect()
}

fn lower_boolop_branch_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    then_label: BlockLabel,
    else_label: BlockLabel,
    loop_ctx: Option<&LoopContext>,
) -> Result<InlineFragment<E>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprBoolOp { op, values, .. } = bool_op;
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let lowered_values = values
        .into_iter()
        .map(|value| {
            bridge
                .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
                    lowerer.lower_expr_instr_into(value.clone(), structured, loop_ctx)
                })
                .transpose()?
                .ok_or_else(|| "boolop branch setup still requires structured lowering".to_string())
        })
        .collect::<Result<Vec<_>, String>>()?;
    let value_count = lowered_values.len();
    assert!(value_count > 0, "bool op expects at least one value");

    let entries = lowered_values
        .iter()
        .map(|(builder, _)| builder.entry_ref().label())
        .collect::<Vec<_>>();
    let mut fragments = Vec::new();
    for (index, (mut builder, value)) in lowered_values.into_iter().enumerate() {
        let is_last = index + 1 == value_count;
        let next_label = entries.get(index + 1).copied();
        let (truthy_label, falsey_label) = if is_last {
            (then_label.clone(), else_label.clone())
        } else {
            match op {
                ast::BoolOp::And => (
                    next_label.expect("non-final boolop value should have a successor"),
                    else_label.clone(),
                ),
                ast::BoolOp::Or => (
                    then_label.clone(),
                    next_label.expect("non-final boolop value should have a successor"),
                ),
            }
        };
        builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(value),
            then_label: truthy_label,
            else_label: falsey_label,
        }));
        fragments.push(builder.finish_blocks());
    }

    let mut fragments = fragments.into_iter();
    let (setup_entry_ref, mut setup_blocks) = fragments
        .next()
        .expect("boolop branch setup should produce at least one fragment");
    for (_, mut blocks) in fragments {
        setup_blocks.append(&mut blocks);
    }
    let setup_entry_index = setup_blocks
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .expect("boolop branch setup entry label should be present in assembled blocks");
    let setup_entry = setup_blocks.remove(setup_entry_index);
    Ok(InlineFragment::new_with_external_targets(
        setup_entry,
        setup_blocks,
        &[then_label, else_label],
    ))
}

fn lower_compare_chain_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    compare: crate::block_py::ExprCompare<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Result<LoweredExpr<E, InstrRuff>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
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

    let (entry, (_initial_left, first_comparator)) = bridge
        .try_lower_inline_value::<E, (InstrRuff, InstrRuff)>(name_gen, |structured| {
            let current_left =
                lowerer.lower_expr_instr_into((*left).clone(), structured, loop_ctx)?;
            structured.push_stmt(assign_name(&compare_name, current_left.clone()));
            let mut first_comparator = lowerer.lower_expr_instr_into(
                first_comparator_expr.clone(),
                structured,
                loop_ctx,
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

    let mut fragments = Vec::new();
    let mut current_builder = entry;

    while let Some((op, comparator)) = steps.next() {
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
        current_builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(load_name(&target_name)),
            then_label: next_entry.entry_ref().label(),
            else_label: BlockLabel::fallthrough(),
        }));
        fragments.push(current_builder.finish_blocks());
        current_builder = next_entry;
    }

    current_builder.ensure_fallthrough_term();
    fragments.push(current_builder.finish_blocks());

    let mut fragments = fragments.into_iter();
    let (setup_entry_ref, mut setup_blocks) = fragments
        .next()
        .expect("compare setup should produce at least one fragment");
    for (_, mut blocks) in fragments {
        setup_blocks.append(&mut blocks);
    }
    let setup_entry_index = setup_blocks
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .expect("compare setup entry label should be present in assembled blocks");
    let setup_entry = setup_blocks.remove(setup_entry_index);
    Ok(LoweredExpr {
        setup: InlineFragment::new(setup_entry, setup_blocks),
        value: load_name(&target_name),
    })
}

fn lower_compare_chain_branch_direct<L, E>(
    lowerer: &L,
    name_gen: &FunctionNameGen,
    compare: crate::block_py::ExprCompare<InstrRuff>,
    then_label: BlockLabel,
    else_label: BlockLabel,
    loop_ctx: Option<&LoopContext>,
) -> Result<InlineFragment<E>, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprCompare {
        left,
        ops,
        comparators,
        ..
    } = compare;
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let mut steps = ops.into_iter().zip(comparators.into_iter()).peekable();
    let Some((first_op, first_comparator_expr)) = steps.next() else {
        unreachable!("compare chain should contain at least one step");
    };
    let first_has_more = steps.peek().is_some();

    let (entry, (initial_left, first_comparator)) = bridge
        .try_lower_inline_value::<E, (InstrRuff, InstrRuff)>(name_gen, |structured| {
            let initial_left =
                lowerer.lower_expr_instr_into((*left).clone(), structured, loop_ctx)?;
            let mut first_comparator = lowerer.lower_expr_instr_into(
                first_comparator_expr.clone(),
                structured,
                loop_ctx,
            )?;
            if first_has_more {
                let tmp_name = fresh_setup_name("compare");
                structured.push_stmt(assign_name(&tmp_name, first_comparator.clone()));
                first_comparator = load_name(&tmp_name);
            }
            Ok((initial_left, first_comparator))
        })
        .transpose()?
        .ok_or_else(|| "compare branch setup still requires structured lowering".to_string())?;

    let mut fragments = Vec::new();
    let mut current_builder = entry;
    let mut current_left = first_comparator.clone();
    let mut current_test = compare_expr(first_op, initial_left, first_comparator);

    while let Some((op, comparator)) = steps.next() {
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
                Ok(comparator_expr)
            })
            .transpose()?
            .ok_or_else(|| "compare branch step still requires structured lowering".to_string())?;

        current_builder.set_term(BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(current_test),
            then_label: next_entry.entry_ref().label(),
            else_label: else_label.clone(),
        }));
        fragments.push(current_builder.finish_blocks());
        current_builder = next_entry;
        current_left = comparator_expr.clone();
        current_test = compare_expr(op, current_left_for_step, comparator_expr);
    }

    current_builder.set_term(BlockTerm::IfTerm(TermIf {
        test: E::from_lowered_expr(current_test),
        then_label: then_label.clone(),
        else_label: else_label.clone(),
    }));
    fragments.push(current_builder.finish_blocks());

    let mut fragments = fragments.into_iter();
    let (setup_entry_ref, mut setup_blocks) = fragments
        .next()
        .expect("compare branch setup should produce at least one fragment");
    for (_, mut blocks) in fragments {
        setup_blocks.append(&mut blocks);
    }
    let setup_entry_index = setup_blocks
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .expect("compare branch setup entry label should be present in assembled blocks");
    let setup_entry = setup_blocks.remove(setup_entry_index);
    Ok(InlineFragment::new_with_external_targets(
        setup_entry,
        setup_blocks,
        &[then_label, else_label],
    ))
}

pub(super) fn lower_boolop_into<L, E>(
    lowerer: &L,
    bool_op: crate::block_py::ExprBoolOp<InstrRuff>,
    out: &mut crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    if let Some(lowered) = try_lower_branching_expr_direct(
        lowerer,
        out.name_gen(),
        InstrRuff::ExprBoolOp(bool_op),
        loop_ctx,
    ) {
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
    E: RuffToBlockPyExpr,
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
    if let Some(lowered) = try_lower_branching_expr_direct(
        lowerer,
        out.name_gen(),
        InstrRuff::ExprCompare(compare),
        loop_ctx,
    ) {
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
