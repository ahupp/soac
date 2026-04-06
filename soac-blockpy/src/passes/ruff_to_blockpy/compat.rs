use super::*;
use crate::block_py::{
    Await, Block, BlockEdge, BlockLabel, BlockTerm, Call, CallArgPositional, ExprAttribute,
    ImplicitNoneExpr, Instr, TermIf, TermRaise, WithMeta,
};
use crate::passes::ast_to_ast::context::Context;
use crate::passes::InstrRuff;
use crate::passes::ruff_to_blockpy::expr_lowering::{
    try_lower_branching_expr_direct, try_lower_if_expr_direct, AstSetupExprLowerer,
};
use crate::passes::ruff_to_blockpy::stmt_sequences::lower_stmts_to_blockpy_stmts_with_context;

fn try_lower_direct_expr<E>(
    name_gen: &FunctionNameGen,
    expr: InstrRuff,
) -> Option<Result<LoweredExpr<E, InstrRuff>, String>>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    match expr {
        InstrRuff::ExprIf(if_expr) => {
            try_lower_if_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, if_expr, None)
        }
        other => try_lower_branching_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, other, None),
    }
}

fn with_exc_meta<E: Instr>(
    mut block: crate::block_py::Block<E, E>,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E> {
    block.exc_edge = exc_target.cloned().map(crate::block_py::BlockEdge::new);
    block
}

fn compat_block_from_inline_with_exc_target_and_expr<E>(
    mut block: Block<E, E>,
    fallthrough_target: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
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
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
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
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let body = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &body, name_gen)
        .unwrap_or_else(|err| panic!("failed to convert compatibility block body to BlockPy: {err}"));
    compat_block_from_lowered_builder_with_exc_target_and_expr(label, body, term, exc_target)
}

pub(crate) fn compat_block_from_lowered_builder_with_exc_target_and_expr<E>(
    label: BlockLabel,
    builder: InlineBlockBuilder<E>,
    term: BlockTerm<E>,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let block = builder
        .finish_linear_block(label, term)
        .unwrap_or_else(|| panic!("compatibility block body should lower to a single linear block"));
    with_exc_meta(block, exc_target)
}

fn emit_lowered_builder_fragment_with_exc_target_and_expr<E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    builder: InlineBlockBuilder<E>,
    term: BlockTerm<E>,
    fallthrough_target: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> InlineBlockRef
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
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

pub(crate) fn set_region_exc_param<E: Instr>(
    blocks: &mut [LoweredBlockPyBlock<E>],
    region: &std::ops::Range<usize>,
    exc_param: &str,
) {
    for block in &mut blocks[region.clone()] {
        let old_exc_param = block.exception_param().map(ToString::to_string);
        block.set_exception_param(exc_param.to_string());
        if let Some(old_exc_param) = old_exc_param {
            if old_exc_param != exc_param {
                rename_exception_edge_args(block, old_exc_param.as_str(), exc_param);
            }
        }
    }
}

fn rename_exception_edge_args<E: Instr>(
    block: &mut LoweredBlockPyBlock<E>,
    old_exc_param: &str,
    new_exc_param: &str,
) {
    fn rewrite_edge_args(
        args: &mut [crate::block_py::BlockArg],
        old_exc_param: &str,
        new_exc_param: &str,
    ) {
        for arg in args {
            if let crate::block_py::BlockArg::Name(name) = arg {
                if name == old_exc_param {
                    *name = new_exc_param.to_string();
                }
            }
        }
    }

    if let BlockTerm::Jump(edge) = &mut block.term {
        rewrite_edge_args(&mut edge.args, old_exc_param, new_exc_param);
    }
    if let Some(edge) = &mut block.exc_edge {
        rewrite_edge_args(&mut edge.args, old_exc_param, new_exc_param);
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
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    if linear.is_empty() {
        return target_label;
    }
    let lowered_linear = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)
        .unwrap_or_else(|err| panic!("failed to lower sequence jump prefix through production path: {err}"));
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
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let mut out = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)?;
    if let Some(expr) = value.clone() {
        let lowered_direct = try_lower_direct_expr::<E>(name_gen, expr);
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
        .map(|expr| crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
            expr,
            &mut out,
            None,
        ))
        .transpose()?;
    let return_term = BlockTerm::Return(value.unwrap_or_else(E::implicit_none_expr));
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
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let mut out = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)?;
    if let Some(expr) = exc.exc.clone() {
        let lowered_direct = try_lower_direct_expr::<E>(name_gen, expr);
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
        exc: exc
            .exc
            .map(|expr| {
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    expr,
                    &mut out,
                    None,
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
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let lowered_direct = try_lower_direct_expr::<E>(name_gen, test.clone());
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
            let lowered_body = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &body, name_gen)?;
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
    let lowered_test = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
        test,
        &mut out,
        None,
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
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let lowered_direct = try_lower_direct_expr::<E>(name_gen, test.clone());
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
        let lowered_test = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
            test,
            &mut out,
            None,
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
    if let Some(linear_label) = linear_label {
        let lowered_linear = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)?;
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

pub(crate) fn emit_for_loop_blocks<E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    setup_label: BlockLabel,
    assign_label: BlockLabel,
    loop_check_label: BlockLabel,
    loop_continue_label: BlockLabel,
    linear: Vec<InstrRuff>,
    iter_name: &str,
    tmp_name: &str,
    iterable: InstrRuff,
    is_async: bool,
    exhausted_entry: BlockLabel,
    body_entry: BlockLabel,
    assign_body: Vec<InstrRuff>,
    exc_target: Option<&BlockLabel>,
) -> BlockLabel
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let synthetic_name_expr = |name: &str| InstrRuff::from_ast_expr(py_expr!("{name:id}", name = name));
    let runtime_helper_expr = |name: &'static str| {
        InstrRuff::ExprAttribute(
            ExprAttribute::new(
                synthetic_name_expr("__soac__"),
                ast::Identifier::new(name, Default::default()),
                ast::ExprContext::Load,
            )
            .with_meta(crate::block_py::Meta::synthetic()),
        )
    };

    let lowered_assign = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &assign_body, name_gen)
        .unwrap_or_else(|err| panic!("failed to lower for-loop target assignment through production path: {err}"));
    blocks.push(compat_block_from_lowered_builder_with_exc_target_and_expr(
        assign_label.clone(),
        lowered_assign,
        BlockTerm::Jump(BlockEdge::new(body_entry)),
        exc_target,
    ));

    let exhausted_test = InstrRuff::from_ast_expr(
        py_expr!("{tmp:id} is __soac__.ITER_COMPLETE", tmp = tmp_name)
    );
    let next_helper = if is_async { "anext_or_sentinel" } else { "next_or_sentinel" };
    let next_call: InstrRuff = Call::new(
        runtime_helper_expr(next_helper),
        vec![CallArgPositional::Positional(synthetic_name_expr(iter_name))],
        Vec::new(),
    )
    .with_meta(crate::block_py::Meta::synthetic())
    .into();
    let next_value: InstrRuff = if is_async {
        Await::new(next_call)
            .with_meta(crate::block_py::Meta::synthetic())
            .into()
    } else {
        next_call
    };
    let check_body = vec![crate::block_py::StmtAssign::new(
        vec![InstrRuff::from_ast_expr(py_expr!("{tmp:id}", tmp = tmp_name))],
        next_value,
    )
    .into()];
    let lowered_check = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &check_body, name_gen)
        .unwrap_or_else(|err| panic!("failed to lower for-loop next step through production path: {err}"));
    blocks.push(compat_block_from_lowered_builder_with_exc_target_and_expr(
        loop_check_label.clone(),
        lowered_check,
        BlockTerm::IfTerm(TermIf {
            test: E::from_lowered_expr(exhausted_test),
            then_label: exhausted_entry,
            else_label: assign_label.clone(),
        }),
        exc_target,
    ));

    let iter_helper = if is_async { "aiter" } else { "iter" };
    let mut setup_body = linear;
    let iter_value: InstrRuff = Call::new(
        runtime_helper_expr(iter_helper),
        vec![CallArgPositional::Positional(iterable)],
        Vec::new(),
    )
    .with_meta(crate::block_py::Meta::synthetic())
    .into();
    setup_body.push(
        crate::block_py::StmtAssign::new(
            vec![InstrRuff::from_ast_expr(py_expr!("{iter:id}", iter = iter_name))],
            iter_value,
        )
        .into(),
    );
    let lowered_setup = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &setup_body, name_gen)
        .unwrap_or_else(|err| panic!("failed to lower for-loop setup through production path: {err}"));
    blocks.push(compat_block_from_lowered_builder_with_exc_target_and_expr(
        setup_label.clone(),
        lowered_setup,
        BlockTerm::Jump(BlockEdge::new(loop_continue_label)),
        exc_target,
    ));
    setup_label
}
