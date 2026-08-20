use super::*;
use crate::block_py::{
    Block, BlockEdge, BlockLabel, BlockTerm, Instr, InstrWithConstantNone, RaiseDisposition,
    TermIf, TermRaise,
};
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::expr_lowering::{
    try_lower_boolop_raise_direct, try_lower_boolop_return_direct,
    try_lower_branching_expr_branch_direct, try_lower_branching_expr_direct,
    try_lower_if_expr_branch_direct, try_lower_if_expr_direct, try_lower_if_expr_raise_direct,
    try_lower_if_expr_return_direct, AstSetupExprLowerer, ScopedSetupExprLowerer,
};
use crate::passes::ruff_to_blockpy::stmt_sequences::lower_stmts_to_blockpy_stmts_with_context;
use crate::passes::InstrRuff;

fn try_lower_direct_expr<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
) -> Option<Result<LoweredExpr<E, InstrRuff>, String>>
where
    E: RuffToBlockPyExpr + InstrWithConstantNone,
{
    let lowerer = ScopedSetupExprLowerer::new(context);
    match expr {
        InstrRuff::ExprIf(if_expr) => {
            try_lower_if_expr_direct::<_, E>(&lowerer, name_gen, if_expr, None)
        }
        other => try_lower_branching_expr_direct::<_, E>(&lowerer, name_gen, other, None),
    }
}

fn try_lower_direct_return_expr<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + InstrWithConstantNone,
{
    let lowerer = ScopedSetupExprLowerer::new(context);
    match expr {
        InstrRuff::ExprIf(if_expr) => {
            try_lower_if_expr_return_direct::<_, E>(&lowerer, name_gen, if_expr, None)
        }
        InstrRuff::ExprBoolOp(bool_op) => {
            try_lower_boolop_return_direct::<_, E>(&lowerer, name_gen, bool_op, None)
        }
        _ => None,
    }
}

fn try_lower_direct_raise_expr<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
    disposition: RaiseDisposition,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + InstrWithConstantNone,
{
    let lowerer = ScopedSetupExprLowerer::new(context);
    match expr {
        InstrRuff::ExprIf(if_expr) => {
            try_lower_if_expr_raise_direct::<_, E>(&lowerer, name_gen, if_expr, disposition, None)
        }
        InstrRuff::ExprBoolOp(bool_op) => {
            try_lower_boolop_raise_direct::<_, E>(&lowerer, name_gen, bool_op, disposition, None)
        }
        _ => None,
    }
}

fn try_lower_direct_branch_expr<E>(
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
    then_label: BlockLabel,
    else_label: BlockLabel,
) -> Option<Result<InlineFragment<E>, String>>
where
    E: RuffToBlockPyExpr + InstrWithConstantNone,
{
    match expr {
        InstrRuff::UnaryOp(unary) if unary.kind == crate::block_py::UnaryOpKind::Not => {
            try_lower_direct_branch_expr(name_gen, *unary.operand, else_label, then_label)
        }
        InstrRuff::ExprIf(if_expr) => try_lower_if_expr_branch_direct::<_, E>(
            &AstSetupExprLowerer,
            name_gen,
            if_expr,
            then_label,
            else_label,
            None,
        ),
        other => try_lower_branching_expr_branch_direct::<_, E>(
            &AstSetupExprLowerer,
            name_gen,
            other,
            then_label,
            else_label,
            None,
        ),
    }
}

fn with_exc_meta<E: Instr>(
    mut block: crate::block_py::Block<E>,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E> {
    // A nested expression may already own a compiler cleanup/dispatch edge.
    // The enclosing region supplies only the fallback for unclaimed blocks.
    if block.exc_edge.is_none() {
        block.exc_edge = exc_target.cloned().map(crate::block_py::BlockEdge::new);
    }
    block
}

fn compat_block_from_inline_with_exc_target_and_expr<E>(
    mut block: Block<E>,
    fallthrough_target: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E>
where
    E: RuffToBlockPyExpr,
{
    block.replace_fallthrough_target(fallthrough_target);
    with_exc_meta(block, exc_target)
}

pub(crate) fn emit_inline_fragment_with_exc_target_and_expr<E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    fragment: InlineFragment<E>,
    fallthrough_target: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> InlineBlockRef
where
    E: RuffToBlockPyExpr,
{
    let entry_ref = fragment.entry_ref();
    blocks.push(compat_block_from_inline_with_exc_target_and_expr(
        fragment.entry,
        fallthrough_target.clone(),
        exc_target,
    ));
    blocks.extend(fragment.deps.into_iter().map(|block| {
        compat_block_from_inline_with_exc_target_and_expr(
            block,
            fallthrough_target.clone(),
            exc_target,
        )
    }));
    entry_ref
}

pub(crate) fn compat_block_from_blockpy_with_exc_target_and_expr<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    label: BlockLabel,
    body: Vec<InstrRuff>,
    term: BlockTerm<E>,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E>
where
    E: RuffToBlockPyExpr,
{
    let body = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &body, name_gen)
        .unwrap_or_else(|err| {
            panic!("failed to convert compatibility block body to BlockPy: {err}")
        });
    compat_block_from_lowered_builder_with_exc_target_and_expr(label, body, term, exc_target)
}

pub(crate) fn compat_block_from_lowered_builder_with_exc_target_and_expr<E>(
    label: BlockLabel,
    builder: InlineBlockBuilder<E>,
    term: BlockTerm<E>,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E>
where
    E: RuffToBlockPyExpr,
{
    let block = builder.finish_linear_block(label, term).unwrap_or_else(|| {
        panic!("compatibility block body should lower to a single linear block")
    });
    with_exc_meta(block, exc_target)
}

pub(crate) fn emit_lowered_builder_fragment_with_exc_target_and_expr<E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    builder: InlineBlockBuilder<E>,
    term: BlockTerm<E>,
    fallthrough_target: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> InlineBlockRef
where
    E: RuffToBlockPyExpr,
{
    let (entry_ref, finished_blocks) = builder.finish_blocks_with_term(term);
    for block in finished_blocks {
        blocks.push(compat_block_from_inline_with_exc_target_and_expr(
            block,
            fallthrough_target.clone(),
            exc_target,
        ));
    }
    entry_ref
}

pub(crate) fn emit_lowered_builder_fragment_with_preferred_linear_entry_and_expr<E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    builder: InlineBlockBuilder<E>,
    preferred_label: BlockLabel,
    term: BlockTerm<E>,
    fallthrough_target: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> InlineBlockRef
where
    E: RuffToBlockPyExpr,
{
    if builder.can_finish_linear_block() {
        let block = builder
            .finish_linear_block(preferred_label.clone(), term)
            .expect("linear-compatible builder should finish as a linear block");
        blocks.push(compat_block_from_inline_with_exc_target_and_expr(
            block,
            fallthrough_target,
            exc_target,
        ));
        InlineBlockRef::from_label(preferred_label)
    } else {
        emit_lowered_builder_fragment_with_exc_target_and_expr(
            blocks,
            builder,
            term,
            fallthrough_target,
            exc_target,
        )
    }
}

pub(crate) fn set_region_exc_param<E: Instr>(
    blocks: &mut [LoweredBlockPyBlock<E>],
    region: &std::ops::Range<usize>,
    exc_param: &str,
) {
    for block in &mut blocks[region.clone()] {
        match block.exception_param() {
            None => block.set_exception_param(exc_param.to_string()),
            Some(current) if current == exc_param => {}
            Some(_) => block.ensure_param(
                exc_param,
                crate::block_py::BlockParamRole::EnclosingException,
            ),
        }
    }
}

pub(crate) fn emit_sequence_jump_block<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    linear: Vec<InstrRuff>,
    target_label: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> BlockLabel
where
    E: RuffToBlockPyExpr,
{
    if linear.is_empty() {
        return target_label;
    }
    let lowered_linear = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)
        .unwrap_or_else(|err| {
            panic!("failed to lower sequence jump prefix through production path: {err}")
        });
    emit_lowered_builder_fragment_with_exc_target_and_expr(
        blocks,
        lowered_linear,
        BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
        target_label,
        exc_target,
    )
    .label()
}

pub(crate) fn emit_sequence_return_block_with_expr_setup_and_expr<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    linear: Vec<InstrRuff>,
    value: Option<InstrRuff>,
    exc_target: Option<&BlockLabel>,
) -> Result<BlockLabel, String>
where
    E: RuffToBlockPyExpr,
{
    let mut out = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)?;
    if let Some(expr) = value.clone() {
        let lowered_terminal = try_lower_direct_return_expr::<E>(context, name_gen, expr.clone());
        if let Some(Ok(fragment)) = lowered_terminal {
            let setup_entry = emit_inline_fragment_with_exc_target_and_expr(
                blocks,
                fragment,
                BlockLabel::fallthrough(),
                exc_target,
            );
            let fragment_entry = if out.is_empty() {
                None
            } else {
                Some(emit_lowered_builder_fragment_with_exc_target_and_expr(
                    blocks,
                    out,
                    BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                    setup_entry.label(),
                    exc_target,
                ))
            };
            return Ok(fragment_entry.unwrap_or(setup_entry).label());
        }
        let lowered_direct = try_lower_direct_expr::<E>(context, name_gen, expr);
        if let Some(Ok(lowered)) = lowered_direct {
            let dispatch_label = name_gen.next_block_name();
            let setup_entry = emit_inline_fragment_with_exc_target_and_expr(
                blocks,
                lowered.setup,
                dispatch_label.clone(),
                exc_target,
            );
            blocks.push(with_exc_meta(
                Block::new(
                    dispatch_label,
                    Vec::new(),
                    BlockTerm::Return(E::from_lowered_expr(lowered.value)),
                    Vec::new(),
                    None,
                ),
                exc_target,
            ));
            let fragment_entry = if out.is_empty() {
                None
            } else {
                Some(emit_lowered_builder_fragment_with_exc_target_and_expr(
                    blocks,
                    out,
                    BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                    setup_entry.label(),
                    exc_target,
                ))
            };
            return Ok(fragment_entry.unwrap_or(setup_entry).label());
        }
    }
    let value = value
        .map(|expr| {
            crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
                context, expr, &mut out, None,
            )
        })
        .transpose()?;
    let return_term = BlockTerm::Return(value.unwrap_or_else(E::constant_none));
    Ok(emit_lowered_builder_fragment_with_exc_target_and_expr(
        blocks,
        out,
        return_term,
        BlockLabel::fallthrough(),
        exc_target,
    )
    .label())
}

pub(crate) fn emit_sequence_raise_block_with_expr_setup_and_expr<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    linear: Vec<InstrRuff>,
    exc: TermRaise<InstrRuff>,
    exc_target: Option<&BlockLabel>,
) -> Result<BlockLabel, String>
where
    E: RuffToBlockPyExpr,
{
    let mut out = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)?;
    if let Some(expr) = exc.exc.clone() {
        let lowered_terminal =
            try_lower_direct_raise_expr::<E>(context, name_gen, expr.clone(), exc.disposition);
        if let Some(Ok(fragment)) = lowered_terminal {
            let setup_entry = emit_inline_fragment_with_exc_target_and_expr(
                blocks,
                fragment,
                BlockLabel::fallthrough(),
                exc_target,
            );
            let fragment_entry = if out.is_empty() {
                None
            } else {
                Some(emit_lowered_builder_fragment_with_exc_target_and_expr(
                    blocks,
                    out,
                    BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                    setup_entry.label(),
                    exc_target,
                ))
            };
            return Ok(fragment_entry.unwrap_or(setup_entry).label());
        }
        let lowered_direct = try_lower_direct_expr::<E>(context, name_gen, expr);
        if let Some(Ok(lowered)) = lowered_direct {
            let dispatch_label = name_gen.next_block_name();
            let setup_entry = emit_inline_fragment_with_exc_target_and_expr(
                blocks,
                lowered.setup,
                dispatch_label.clone(),
                exc_target,
            );
            blocks.push(with_exc_meta(
                Block::new(
                    dispatch_label,
                    Vec::new(),
                    BlockTerm::Raise(TermRaise {
                        exc: Some(E::from_lowered_expr(lowered.value)),
                        disposition: exc.disposition,
                    }),
                    Vec::new(),
                    None,
                ),
                exc_target,
            ));
            let fragment_entry = if out.is_empty() {
                None
            } else {
                Some(emit_lowered_builder_fragment_with_exc_target_and_expr(
                    blocks,
                    out,
                    BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                    setup_entry.label(),
                    exc_target,
                ))
            };
            return Ok(fragment_entry.unwrap_or(setup_entry).label());
        }
    }
    let exc = TermRaise {
        disposition: exc.disposition,
        exc: exc
            .exc
            .map(|expr| {
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
                    context, expr, &mut out, None,
                )
            })
            .transpose()?,
    };
    let raise_term = BlockTerm::Raise(exc);
    Ok(emit_lowered_builder_fragment_with_exc_target_and_expr(
        blocks,
        out,
        raise_term,
        BlockLabel::fallthrough(),
        exc_target,
    )
    .label())
}

pub(crate) fn emit_if_branch_block_with_expr_setup_and_expr<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    body: Vec<InstrRuff>,
    test: InstrRuff,
    then_label: BlockLabel,
    else_label: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> Result<BlockLabel, String>
where
    E: RuffToBlockPyExpr,
{
    let lowered_direct_branch = try_lower_direct_branch_expr::<E>(
        name_gen,
        test.clone(),
        then_label.clone(),
        else_label.clone(),
    );
    if let Some(lowered_direct_branch) = lowered_direct_branch {
        let fragment = lowered_direct_branch?;
        let setup_entry = emit_inline_fragment_with_exc_target_and_expr(
            blocks,
            fragment,
            BlockLabel::fallthrough(),
            exc_target,
        );
        if !body.is_empty() {
            let lowered_body =
                lower_stmts_to_blockpy_stmts_with_context::<E>(context, &body, name_gen)?;
            return Ok(emit_lowered_builder_fragment_with_exc_target_and_expr(
                blocks,
                lowered_body,
                BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                setup_entry.label(),
                exc_target,
            )
            .label());
        }
        return Ok(setup_entry.label());
    }

    let lowered_direct = try_lower_direct_expr::<E>(context, name_gen, test.clone());
    if let Some(Ok(lowered)) = lowered_direct {
        let dispatch_label = name_gen.next_block_name();
        let setup_entry = emit_inline_fragment_with_exc_target_and_expr(
            blocks,
            lowered.setup,
            dispatch_label.clone(),
            exc_target,
        );
        blocks.push(with_exc_meta(
            Block::new(
                dispatch_label,
                Vec::new(),
                BlockTerm::IfTerm(TermIf {
                    test: E::from_lowered_expr(lowered.value),
                    then_label,
                    else_label,
                }),
                Vec::new(),
                None,
            ),
            exc_target,
        ));
        if !body.is_empty() {
            let lowered_body =
                lower_stmts_to_blockpy_stmts_with_context::<E>(context, &body, name_gen)?;
            return Ok(emit_lowered_builder_fragment_with_exc_target_and_expr(
                blocks,
                lowered_body,
                BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
                setup_entry.label(),
                exc_target,
            )
            .label());
        }
        return Ok(setup_entry.label());
    }

    let mut out = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &body, name_gen)?;
    let lowered_test = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
        context, test, &mut out, None,
    )?;
    let if_term = BlockTerm::IfTerm(TermIf {
        test: lowered_test,
        then_label,
        else_label,
    });
    Ok(emit_lowered_builder_fragment_with_exc_target_and_expr(
        blocks,
        out,
        if_term,
        BlockLabel::fallthrough(),
        exc_target,
    )
    .label())
}

pub(crate) fn emit_simple_while_blocks_with_expr_setup_and_expr<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    test_label: BlockLabel,
    linear_label: Option<BlockLabel>,
    linear: Vec<InstrRuff>,
    test: InstrRuff,
    body_entry: BlockLabel,
    cond_false_entry: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> Result<BlockLabel, String>
where
    E: RuffToBlockPyExpr,
{
    let lowered_direct_branch = try_lower_direct_branch_expr::<E>(
        name_gen,
        test.clone(),
        body_entry.clone(),
        cond_false_entry.clone(),
    );
    if let Some(lowered_direct_branch) = lowered_direct_branch {
        let fragment = lowered_direct_branch?;
        let setup_entry = emit_inline_fragment_with_exc_target_and_expr(
            blocks,
            fragment,
            BlockLabel::fallthrough(),
            exc_target,
        );
        if setup_entry.label() != test_label {
            blocks.push(with_exc_meta(
                Block::new(
                    test_label.clone(),
                    Vec::new(),
                    BlockTerm::Jump(BlockEdge::new(setup_entry.label())),
                    Vec::new(),
                    None,
                ),
                exc_target,
            ));
        }
    } else {
        let lowered_direct = try_lower_direct_expr::<E>(context, name_gen, test.clone());
        if let Some(Ok(lowered)) = lowered_direct {
            let dispatch_label = name_gen.next_block_name();
            let setup_entry = emit_inline_fragment_with_exc_target_and_expr(
                blocks,
                lowered.setup,
                dispatch_label.clone(),
                exc_target,
            );
            if setup_entry.label() != test_label {
                blocks.push(with_exc_meta(
                    Block::new(
                        test_label.clone(),
                        Vec::new(),
                        BlockTerm::Jump(BlockEdge::new(setup_entry.label())),
                        Vec::new(),
                        None,
                    ),
                    exc_target,
                ));
            }
            blocks.push(with_exc_meta(
                Block::new(
                    dispatch_label,
                    Vec::new(),
                    BlockTerm::IfTerm(TermIf {
                        test: E::from_lowered_expr(lowered.value),
                        then_label: body_entry,
                        else_label: cond_false_entry,
                    }),
                    Vec::new(),
                    None,
                ),
                exc_target,
            ));
        } else {
            let mut out = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &[], name_gen)?;
            let lowered_test =
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_context(
                    context, test, &mut out, None,
                )?;
            let if_term = BlockTerm::IfTerm(TermIf {
                test: lowered_test,
                then_label: body_entry,
                else_label: cond_false_entry,
            });
            let emitted_test_entry = emit_lowered_builder_fragment_with_exc_target_and_expr(
                blocks,
                out,
                if_term,
                BlockLabel::fallthrough(),
                exc_target,
            );
            if emitted_test_entry.label() != test_label {
                blocks.push(with_exc_meta(
                    Block::new(
                        test_label.clone(),
                        Vec::new(),
                        BlockTerm::Jump(BlockEdge::new(emitted_test_entry.label())),
                        Vec::new(),
                        None,
                    ),
                    exc_target,
                ));
            }
        }
    }
    if let Some(linear_label) = linear_label {
        let lowered_linear =
            lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)?;
        let emitted_linear_entry = emit_lowered_builder_fragment_with_exc_target_and_expr(
            blocks,
            lowered_linear,
            BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
            test_label,
            exc_target,
        );
        if emitted_linear_entry.label() != linear_label {
            blocks.push(with_exc_meta(
                Block::new(
                    linear_label.clone(),
                    Vec::new(),
                    BlockTerm::Jump(BlockEdge::new(emitted_linear_entry.label())),
                    Vec::new(),
                    None,
                ),
                exc_target,
            ));
        }
        Ok(linear_label)
    } else {
        Ok(test_label)
    }
}
