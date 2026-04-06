use super::*;
use crate::block_py::{
    Block, BlockBuilder, BlockLabel, BlockPyStmtBuilder, BlockTerm, Expr, ImplicitNoneExpr, Instr,
    StructuredInstr, TermIf, TermRaise,
};
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ruff_to_blockpy::expr_lowering::{
    try_lower_branching_expr_direct, try_lower_if_expr_direct, AstSetupExprLowerer,
};
use crate::passes::ruff_to_blockpy::stmt_lowering::{
    lower_nested_stmt_into_with_expr, StructuredLoweringBridge,
};

fn with_exc_meta<E: Instr>(
    block: crate::block_py::Block<StructuredInstr<E>, E>,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E> {
    crate::block_py::Block {
        label: block.label,
        body: block.body,
        term: block.term,
        params: block.params,
        exc_edge: exc_target.cloned().map(crate::block_py::BlockEdge::new),
    }
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
    with_exc_meta(
        Block {
            label: block.label,
            body: block.body.into_iter().map(StructuredInstr::Expr).collect(),
            term: block.term,
            params: block.params,
            exc_edge: None,
        },
        exc_target,
    )
}

pub(crate) fn emit_inline_fragment_with_exc_target_and_expr<E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    fragment: InlineFragment<E>,
    fallthrough_target: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> BlockLabel
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let entry_label = fragment.entry.label;
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
    entry_label
}

pub(crate) fn compat_block_from_blockpy_with_exc_target_and_expr<E>(
    label: BlockLabel,
    body: Vec<Stmt>,
    term: BlockTerm<E>,
    exc_target: Option<&BlockLabel>,
) -> LoweredBlockPyBlock<E>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let body = lower_stmts_to_blockpy_stmts::<E>(&body).unwrap_or_else(|err| {
        panic!("failed to convert compatibility block body to BlockPy: {err}")
    });
    assert!(
        body.term.is_none(),
        "compatibility block body should not contain its own terminator"
    );
    with_exc_meta(
        Block::from_builder(
            label,
            BlockBuilder::with_term(body.body, Some(term)),
            Vec::new(),
            None,
            None,
        ),
        exc_target,
    )
}

fn compat_block_builder_with_expr_setup_and_expr<E>(
    context: &Context,
    body: Vec<Stmt>,
) -> Result<BlockPyStmtBuilder<E>, String>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let mut out = BlockPyStmtBuilder::<E>::new();
    let mut next_label_id = 0usize;
    for stmt in &body {
        lower_nested_stmt_into_with_expr(context, stmt, &mut out, None, &mut next_label_id)?;
    }
    Ok(out)
}

pub(crate) fn compat_if_jump_block_with_expr_setup_and_exc_target_and_expr<E>(
    context: &Context,
    label: BlockLabel,
    body: Vec<Stmt>,
    test: Expr,
    then_label: BlockLabel,
    else_label: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> Result<LoweredBlockPyBlock<E>, String>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let mut out = compat_block_builder_with_expr_setup_and_expr::<E>(context, body)?;
    let mut next_label_id = 0usize;
    let test = crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
        test,
        &mut out,
        None,
        &mut next_label_id,
    )?;
    let fragment = out.finish();
    assert!(
        fragment.term.is_none(),
        "compatibility block body should not contain its own terminator"
    );
    Ok(with_exc_meta(
        Block::from_builder(
            label,
            BlockBuilder::with_term(
                fragment.body,
                Some(BlockTerm::IfTerm(TermIf {
                    test,
                    then_label,
                    else_label,
                })),
            ),
            Vec::new(),
            None,
            None,
        ),
        exc_target,
    ))
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
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    label: BlockLabel,
    linear: Vec<Stmt>,
    target_label: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> BlockLabel
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
        label.clone(),
        linear,
        BlockTerm::Jump(BlockEdge::new(target_label)),
        exc_target,
    ));
    label
}

pub(crate) fn emit_sequence_return_block_with_expr_setup_and_expr<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    label: BlockLabel,
    linear: Vec<Stmt>,
    value: Option<Expr>,
    exc_target: Option<&BlockLabel>,
) -> Result<BlockLabel, String>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let mut out = compat_block_builder_with_expr_setup_and_expr::<E>(context, linear)?;
    if let Some(expr) = value.clone() {
        let lowered_direct = match expr {
            Expr::If(if_expr) => {
                try_lower_if_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, if_expr, None)
            }
            other => {
                try_lower_branching_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, other, None)
            }
        };
        if let Some(Ok(lowered)) = lowered_direct {
            let linear_fragment = out.finish();
            assert!(
                linear_fragment.term.is_none(),
                "compatibility block body should not contain its own terminator"
            );
            let fragment_entry_label = if linear_fragment.body.is_empty() {
                label.clone()
            } else {
                let next_label = name_gen.next_block_name();
                blocks.push(with_exc_meta(
                    Block::from_builder(
                        label.clone(),
                        BlockBuilder::with_term(
                            linear_fragment.body,
                            Some(BlockTerm::Jump(BlockEdge::new(next_label.clone()))),
                        ),
                        Vec::new(),
                        None,
                        None,
                    ),
                    exc_target,
                ));
                next_label
            };
            let dispatch_label = name_gen.next_block_name();
            let mut setup = lowered.setup;
            setup.entry.label = fragment_entry_label;
            emit_inline_fragment_with_exc_target_and_expr(
                blocks,
                setup,
                dispatch_label.clone(),
                exc_target,
            );
            blocks.push(with_exc_meta(
                Block::from_builder(
                    dispatch_label,
                    BlockBuilder::with_term(
                        Vec::new(),
                        Some(BlockTerm::Return(lowered.value)),
                    ),
                    Vec::new(),
                    None,
                    None,
                ),
                exc_target,
            ));
            return Ok(label);
        }
    }
    let mut next_label_id = 0usize;
    let value = value
        .map(|expr| {
            crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                expr,
                &mut out,
                None,
                &mut next_label_id,
            )
        })
        .transpose()?;
    let fragment = out.finish();
    assert!(
        fragment.term.is_none(),
        "compatibility block body should not contain its own terminator"
    );
    blocks.push(with_exc_meta(
        Block::from_builder(
            label.clone(),
            BlockBuilder::with_term(
                fragment.body,
                Some(BlockTerm::Return(
                    value.unwrap_or_else(E::implicit_none_expr),
                )),
            ),
            Vec::new(),
            None,
            None,
        ),
        exc_target,
    ));
    Ok(label)
}

pub(crate) fn emit_sequence_raise_block_with_expr_setup_and_expr<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    label: BlockLabel,
    linear: Vec<Stmt>,
    exc: TermRaise<Expr>,
    exc_target: Option<&BlockLabel>,
) -> Result<BlockLabel, String>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let mut out = compat_block_builder_with_expr_setup_and_expr::<E>(context, linear)?;
    if let Some(expr) = exc.exc.clone() {
        let lowered_direct = match expr {
            Expr::If(if_expr) => {
                try_lower_if_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, if_expr, None)
            }
            other => {
                try_lower_branching_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, other, None)
            }
        };
        if let Some(Ok(lowered)) = lowered_direct {
            let linear_fragment = out.finish();
            assert!(
                linear_fragment.term.is_none(),
                "compatibility block body should not contain its own terminator"
            );
            let fragment_entry_label = if linear_fragment.body.is_empty() {
                label.clone()
            } else {
                let next_label = name_gen.next_block_name();
                blocks.push(with_exc_meta(
                    Block::from_builder(
                        label.clone(),
                        BlockBuilder::with_term(
                            linear_fragment.body,
                            Some(BlockTerm::Jump(BlockEdge::new(next_label.clone()))),
                        ),
                        Vec::new(),
                        None,
                        None,
                    ),
                    exc_target,
                ));
                next_label
            };
            let dispatch_label = name_gen.next_block_name();
            let mut setup = lowered.setup;
            setup.entry.label = fragment_entry_label;
            emit_inline_fragment_with_exc_target_and_expr(
                blocks,
                setup,
                dispatch_label.clone(),
                exc_target,
            );
            blocks.push(with_exc_meta(
                Block::from_builder(
                    dispatch_label,
                    BlockBuilder::with_term(
                        Vec::new(),
                        Some(BlockTerm::Raise(TermRaise {
                            exc: Some(lowered.value),
                        })),
                    ),
                    Vec::new(),
                    None,
                    None,
                ),
                exc_target,
            ));
            return Ok(label);
        }
    }
    let mut next_label_id = 0usize;
    let exc = TermRaise {
        exc: exc
            .exc
            .map(|expr| {
                crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                    expr,
                    &mut out,
                    None,
                    &mut next_label_id,
                )
            })
            .transpose()?,
    };
    let fragment = out.finish();
    assert!(
        fragment.term.is_none(),
        "compatibility block body should not contain its own terminator"
    );
    blocks.push(with_exc_meta(
        Block::from_builder(
            label.clone(),
            BlockBuilder::with_term(fragment.body, Some(BlockTerm::Raise(exc))),
            Vec::new(),
            None,
            None,
        ),
        exc_target,
    ));
    Ok(label)
}

pub(crate) fn emit_if_branch_block_with_expr_setup_and_expr<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    label: BlockLabel,
    body: Vec<Stmt>,
    test: Expr,
    then_label: BlockLabel,
    else_label: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> Result<BlockLabel, String>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let lowered_direct = match test.clone() {
        Expr::If(if_expr) => {
            try_lower_if_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, if_expr, None)
        }
        other => try_lower_branching_expr_direct::<_, E>(
            &AstSetupExprLowerer,
            name_gen,
            other,
            None,
        ),
    };
    if let Some(Ok(lowered)) = lowered_direct {
        let setup_label = if body.is_empty() {
            label.clone()
        } else {
            name_gen.next_block_name()
        };
        let dispatch_label = name_gen.next_block_name();
        let mut setup = lowered.setup;
        setup.entry.label = setup_label.clone();
        emit_inline_fragment_with_exc_target_and_expr(
            blocks,
            setup,
            dispatch_label.clone(),
            exc_target,
        );
        blocks.push(with_exc_meta(
            Block::from_builder(
                dispatch_label,
                BlockBuilder::with_term(
                    Vec::new(),
                    Some(BlockTerm::IfTerm(TermIf {
                        test: lowered.value,
                        then_label,
                        else_label,
                    })),
                ),
                Vec::new(),
                None,
                None,
            ),
            exc_target,
        ));
        if !body.is_empty() {
            blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
                label.clone(),
                body,
                BlockTerm::Jump(BlockEdge::new(setup_label)),
                exc_target,
            ));
        }
        return Ok(label);
    }

    let mut bridge = StructuredLoweringBridge::new();
    if let Some(fragment) = bridge.try_lower_inline_value::<E, E>(
        |out, scratch_next_label_id| {
            for stmt in &body {
                lower_nested_stmt_into_with_expr(
                    context,
                    stmt,
                    out,
                    None,
                    scratch_next_label_id,
                )?;
            }
            crate::passes::ruff_to_blockpy::expr_lowering::lower_expr_into_with_setup(
                test.clone(),
                out,
                None,
                scratch_next_label_id,
            )
        },
    ) {
        let (mut entry, test) = fragment?;
        entry.set_term(BlockTerm::IfTerm(TermIf {
            test,
            then_label,
            else_label,
        }));
        return Ok(emit_inline_fragment_with_exc_target_and_expr(
            blocks,
            InlineFragment::from_closed_builder(label, entry, Vec::new()),
            BlockLabel::fallthrough(),
            exc_target,
        ));
    }

    blocks.push(
        compat_if_jump_block_with_expr_setup_and_exc_target_and_expr(
            context,
            label.clone(),
            body,
            test,
            then_label,
            else_label,
            exc_target,
        )?,
    );
    Ok(label)
}

pub(crate) fn emit_simple_while_blocks_with_expr_setup_and_expr<E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    test_label: BlockLabel,
    linear_label: Option<BlockLabel>,
    linear: Vec<Stmt>,
    test: Expr,
    body_entry: BlockLabel,
    cond_false_entry: BlockLabel,
    exc_target: Option<&BlockLabel>,
) -> Result<BlockLabel, String>
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let lowered_direct = match test.clone() {
        Expr::If(if_expr) => {
            try_lower_if_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, if_expr, None)
        }
        other => try_lower_branching_expr_direct::<_, E>(&AstSetupExprLowerer, name_gen, other, None),
    };
    if let Some(Ok(lowered)) = lowered_direct {
        let dispatch_label = name_gen.next_block_name();
        let mut setup = lowered.setup;
        setup.entry.label = test_label.clone();
        emit_inline_fragment_with_exc_target_and_expr(
            blocks,
            setup,
            dispatch_label.clone(),
            exc_target,
        );
        blocks.push(with_exc_meta(
            Block::from_builder(
                dispatch_label,
                BlockBuilder::with_term(
                    Vec::new(),
                    Some(BlockTerm::IfTerm(TermIf {
                        test: lowered.value,
                        then_label: body_entry,
                        else_label: cond_false_entry,
                    })),
                ),
                Vec::new(),
                None,
                None,
            ),
            exc_target,
        ));
    } else {
        blocks.push(
            compat_if_jump_block_with_expr_setup_and_exc_target_and_expr(
                context,
                test_label.clone(),
                Vec::new(),
                test,
                body_entry,
                cond_false_entry,
                exc_target,
            )?,
        );
    }
    if let Some(linear_label) = linear_label {
        blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
            linear_label.clone(),
            linear,
            BlockTerm::Jump(BlockEdge::new(test_label)),
            exc_target,
        ));
        Ok(linear_label)
    } else {
        Ok(test_label)
    }
}

pub(crate) fn emit_for_loop_blocks<E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    setup_label: BlockLabel,
    assign_label: BlockLabel,
    loop_check_label: BlockLabel,
    loop_continue_label: BlockLabel,
    linear: Vec<Stmt>,
    iter_name: &str,
    tmp_name: &str,
    iterable: Expr,
    is_async: bool,
    exhausted_entry: BlockLabel,
    body_entry: BlockLabel,
    assign_body: Vec<Stmt>,
    exc_target: Option<&BlockLabel>,
) -> BlockLabel
where
    E: RuffToBlockPyExpr + ImplicitNoneExpr,
{
    let iter_expr = py_expr!("{iter:id}", iter = iter_name);
    let tmp_expr = py_expr!("{tmp:id}", tmp = tmp_name);

    blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
        assign_label.clone(),
        assign_body,
        BlockTerm::Jump(BlockEdge::new(body_entry)),
        exc_target,
    ));

    let exhausted_test = py_expr!("{value:expr} is __soac__.ITER_COMPLETE", value = tmp_expr);
    let check_body = if is_async {
        vec![py_stmt!(
            "{tmp:id} = await __soac__.anext_or_sentinel({iter:expr})",
            tmp = tmp_name,
            iter = iter_expr.clone(),
        )]
    } else {
        vec![py_stmt!(
            "{tmp:id} = __soac__.next_or_sentinel({iter:expr})",
            tmp = tmp_name,
            iter = iter_expr.clone(),
        )]
    };
    blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
        loop_check_label.clone(),
        check_body,
        BlockTerm::IfTerm(TermIf {
            test: exhausted_test.into(),
            then_label: exhausted_entry,
            else_label: assign_label.clone(),
        }),
        exc_target,
    ));

    let mut setup_body = linear;
    if is_async {
        setup_body.push(py_stmt!(
            "{iter:id} = __soac__.aiter({iterable:expr})",
            iter = iter_name,
            iterable = iterable,
        ));
    } else {
        setup_body.push(py_stmt!(
            "{iter:id} = __soac__.iter({iterable:expr})",
            iter = iter_name,
            iterable = iterable,
        ));
    }
    blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
        setup_label.clone(),
        setup_body,
        BlockTerm::Jump(BlockEdge::new(loop_continue_label)),
        exc_target,
    ));
    setup_label
}
