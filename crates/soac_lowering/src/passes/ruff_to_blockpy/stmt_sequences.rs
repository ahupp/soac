use super::stmt_lowering::{lower_instr_into_with_expr, plan_instr_head_for_blockpy};
use super::*;
use crate::block_py::{
    Await, BlockTerm, Call, CallArgPositional, ExprAttribute, TermRaise, WithMeta,
};
use crate::passes::ast_to_ast::context::Context;
use crate::passes::InstrRuff;

pub(crate) fn lower_stmts_to_blockpy_stmts_with_context<E>(
    context: &Context,
    stmts: &[InstrRuff],
    name_gen: &FunctionNameGen,
) -> Result<crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>, String>
where
    E: RuffToBlockPyExpr,
{
    let mut out =
        crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder::<E>::new(name_gen);
    for stmt in stmts {
        lower_instr_into_with_expr(context, stmt, name_gen, &mut out, None)?;
    }
    Ok(out)
}

pub(crate) fn plan_instr_sequence_head(
    context: &Context,
    stmt: &InstrRuff,
) -> StmtSequenceHeadPlan {
    plan_instr_head_for_blockpy(context, stmt)
}

pub(crate) enum InstrSequenceDriveResult {
    Exhausted {
        linear: Vec<InstrRuff>,
    },
    Break {
        linear: Vec<InstrRuff>,
        index: usize,
        plan: StmtSequenceHeadPlan,
    },
}

pub(crate) fn drive_instr_sequence_until_control(
    context: &Context,
    stmts: &[InstrRuff],
    mut linear: Vec<InstrRuff>,
) -> InstrSequenceDriveResult {
    let mut index = 0;
    while index < stmts.len() {
        match plan_instr_sequence_head(context, &stmts[index]) {
            StmtSequenceHeadPlan::Linear(stmt) => {
                linear.push(stmt);
                index += 1;
            }
            StmtSequenceHeadPlan::Expanded(stmts) => {
                return InstrSequenceDriveResult::Break {
                    linear,
                    index,
                    plan: StmtSequenceHeadPlan::Expanded(stmts),
                };
            }
            StmtSequenceHeadPlan::FunctionDef(func_def) => {
                panic!(
                    "raw nested FunctionDef {} reached Ruff-to-BlockPy after exec-source fallback removal",
                    func_def.name.id
                );
            }
            plan => {
                return InstrSequenceDriveResult::Break {
                    linear,
                    index,
                    plan,
                };
            }
        }
    }
    InstrSequenceDriveResult::Exhausted { linear }
}

fn compat_blockpy_raise_from_instr(
    raise_stmt: crate::block_py::StmtRaise<InstrRuff>,
) -> TermRaise<InstrRuff> {
    assert!(
        raise_stmt.cause.is_none(),
        "raise-from should be lowered before Ruff AST -> BlockPy conversion"
    );
    TermRaise {
        exc: raise_stmt.exc.map(|expr| *expr),
    }
}

fn contains_return_instr_in_body(stmts: &[InstrRuff]) -> bool {
    stmts.iter().any(contains_return_instr)
}

fn contains_return_stmt_in_handlers(handlers: &[ast::ExceptHandler]) -> bool {
    handlers.iter().any(|handler| {
        let ast::ExceptHandler::ExceptHandler(handler) = handler;
        contains_return_stmt_in_body(&handler.body)
    })
}

fn contains_return_stmt_in_body(stmts: &[Stmt]) -> bool {
    stmts
        .iter()
        .cloned()
        .map(crate::passes::ast_to_instr::from_ast_stmt)
        .any(|stmt| contains_return_instr(&stmt))
}

fn contains_return_instr(instr: &InstrRuff) -> bool {
    match instr {
        InstrRuff::StmtReturn(_) => true,
        InstrRuff::StmtIf(stmt) => {
            contains_return_instr_in_body(&stmt.body) || contains_return_instr_in_body(&stmt.orelse)
        }
        InstrRuff::StmtWhile(stmt) => {
            contains_return_instr_in_body(&stmt.body) || contains_return_instr_in_body(&stmt.orelse)
        }
        InstrRuff::StmtFor(stmt) => {
            contains_return_instr_in_body(&stmt.body) || contains_return_instr_in_body(&stmt.orelse)
        }
        InstrRuff::StmtTry(stmt) => {
            contains_return_instr_in_body(&stmt.body)
                || contains_return_stmt_in_handlers(&stmt.handlers)
                || contains_return_instr_in_body(&stmt.orelse)
                || contains_return_instr_in_body(&stmt.finalbody)
        }
        InstrRuff::StmtWith(stmt) => contains_return_instr_in_body(&stmt.body),
        InstrRuff::StmtFunctionDef(_) | InstrRuff::StmtClassDef(_) => false,
        _ => false,
    }
}

pub(crate) fn lower_common_stmt_sequence_head<FSeq, E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    plan: StmtSequenceHeadPlan,
    remaining_stmts: &[InstrRuff],
    targets: RegionTargets,
    linear: Vec<InstrRuff>,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    next_label: &mut dyn FnMut() -> BlockLabel,
    lower_sequence: &mut FSeq,
) -> Option<BlockLabel>
where
    FSeq: FnMut(&[InstrRuff], RegionTargets, &mut Vec<LoweredBlockPyBlock<E>>) -> BlockLabel,
    E: RuffToBlockPyExpr,
{
    match plan {
        StmtSequenceHeadPlan::Raise(raise_stmt) => Some(
            emit_sequence_raise_block_with_expr_setup_and_expr(
                context,
                blocks,
                name_gen,
                linear,
                compat_blockpy_raise_from_instr(raise_stmt),
                targets.active_exc.as_ref(),
            )
            .unwrap_or_else(|err| {
                panic!("failed to lower sequence raise head through expr seam: {err}")
            }),
        ),
        StmtSequenceHeadPlan::Return(value) => Some(
            emit_sequence_return_block_with_expr_setup_and_expr(
                context,
                blocks,
                name_gen,
                linear,
                Some(value),
                targets.active_exc.as_ref(),
            )
            .unwrap_or_else(|err| {
                panic!("failed to lower sequence return head through expr seam: {err}")
            }),
        ),
        StmtSequenceHeadPlan::If(if_stmt) => Some(lower_if_stmt_sequence_from_stmt(
            context,
            name_gen,
            if_stmt,
            remaining_stmts,
            targets,
            linear,
            blocks,
            &mut |stmts, targets, blocks| lower_sequence(stmts, targets, blocks),
        )),
        StmtSequenceHeadPlan::While(while_stmt) => {
            let test_label = next_label();
            let linear_label = if linear.is_empty() {
                None
            } else {
                Some(next_label())
            };
            Some(lower_while_stmt_sequence_from_stmt(
                context,
                name_gen,
                while_stmt,
                remaining_stmts,
                targets,
                linear,
                blocks,
                test_label,
                linear_label,
                lower_sequence,
            ))
        }
        StmtSequenceHeadPlan::Break => match targets.loop_labels {
            Some(loop_labels) => Some(emit_sequence_jump_block(
                context,
                blocks,
                name_gen,
                linear,
                loop_labels.break_label,
                targets.active_exc.as_ref(),
            )),
            None => Some(targets.normal_cont),
        },
        StmtSequenceHeadPlan::Continue => match targets.loop_labels {
            Some(loop_labels) => Some(emit_sequence_jump_block(
                context,
                blocks,
                name_gen,
                linear,
                loop_labels.continue_label,
                targets.active_exc.as_ref(),
            )),
            None => Some(targets.normal_cont),
        },
        _ => None,
    }
}

fn synthetic_name_expr_with_context(name: &str, ctx: ast::ExprContext) -> InstrRuff {
    crate::passes::ast_to_instr::from_ast_expr(ast::Expr::Name(ast::ExprName {
        id: ast::name::Name::new(name),
        ctx,
        range: Default::default(),
        node_index: Default::default(),
    }))
}

fn synthetic_name_expr(name: &str) -> InstrRuff {
    synthetic_name_expr_with_context(name, ast::ExprContext::Load)
}

fn runtime_helper_expr(name: &'static str) -> InstrRuff {
    InstrRuff::ExprAttribute(
        ExprAttribute::new(
            synthetic_name_expr("__soac__"),
            ast::Identifier::new(name, Default::default()),
            ast::ExprContext::Load,
        )
        .with_meta(crate::block_py::Meta::synthetic()),
    )
}

fn synthetic_assign(target: InstrRuff, value: InstrRuff) -> InstrRuff {
    crate::block_py::StmtAssign::new(vec![target], value)
        .with_meta(crate::block_py::Meta::synthetic())
        .into()
}

fn synthetic_delete(target: InstrRuff) -> InstrRuff {
    crate::block_py::StmtDelete::new(vec![target])
        .with_meta(crate::block_py::Meta::synthetic())
        .into()
}

fn stop_iteration_handler_with_body(
    exception_name: &str,
    body: Vec<InstrRuff>,
) -> ast::ExceptHandler {
    let mut try_stmt = match exception_name {
        "StopIteration" => {
            let ast::Stmt::Try(try_stmt) = py_stmt!(
                r#"
try:
    pass
except __soac__.StopIteration:
    pass
"#
            ) else {
                unreachable!("synthetic StopIteration handler should parse as try statement");
            };
            try_stmt
        }
        "StopAsyncIteration" => {
            let ast::Stmt::Try(try_stmt) = py_stmt!(
                r#"
try:
    pass
except __soac__.StopAsyncIteration:
    pass
"#
            ) else {
                unreachable!("synthetic StopAsyncIteration handler should parse as try statement");
            };
            try_stmt
        }
        other => panic!("unsupported synthetic for-loop stop exception {other}"),
    };
    assert_eq!(
        try_stmt.handlers.len(),
        1,
        "synthetic {exception_name} handler should contain one handler"
    );
    let handler = try_stmt.handlers.remove(0);
    let ast::ExceptHandler::ExceptHandler(mut handler) = handler;
    handler.body = body
        .into_iter()
        .map(crate::passes::ast_to_instr::into_ast_stmt)
        .collect();
    ast::ExceptHandler::ExceptHandler(handler)
}

fn expand_for_stmt(
    name_gen: &FunctionNameGen,
    for_stmt: crate::block_py::StmtFor<InstrRuff>,
    iter_name: &str,
    tmp_name: &str,
    assign_body: Vec<InstrRuff>,
) -> Vec<InstrRuff> {
    let iter_helper = if for_stmt.is_async { "aiter" } else { "iter" };
    let iterable_name = name_gen.next_tmp_name("iterable").to_string();
    let iterable_assign = synthetic_assign(synthetic_name_expr(&iterable_name), *for_stmt.iter);
    let iter_value: InstrRuff = Call::new(
        runtime_helper_expr(iter_helper),
        vec![CallArgPositional::Positional(synthetic_name_expr(
            &iterable_name,
        ))],
        Vec::new(),
    )
    .with_meta(crate::block_py::Meta::synthetic())
    .into();
    let iter_assign = synthetic_assign(synthetic_name_expr(&iter_name), iter_value);
    let iter_setup = crate::block_py::StmtTry::new(
        vec![iter_assign],
        Vec::new(),
        Vec::new(),
        vec![synthetic_delete(synthetic_name_expr_with_context(
            &iterable_name,
            ast::ExprContext::Del,
        ))],
        false,
    )
    .with_meta(crate::block_py::Meta::synthetic())
    .into();

    let completed_name =
        (!for_stmt.orelse.is_empty()).then(|| name_gen.next_tmp_name("completed").to_string());
    let mut stop_body = Vec::new();
    if let Some(completed_name) = &completed_name {
        stop_body.push(synthetic_assign(
            synthetic_name_expr(completed_name),
            crate::passes::ast_to_instr::from_ast_expr(py_expr!("True")),
        ));
    }
    stop_body.push(
        crate::block_py::StmtBreak::new()
            .with_meta(crate::block_py::Meta::synthetic())
            .into(),
    );

    let mut while_body = if for_stmt.is_async {
        let next_call: InstrRuff = Call::new(
            runtime_helper_expr("anext"),
            vec![CallArgPositional::Positional(synthetic_name_expr(
                iter_name,
            ))],
            Vec::new(),
        )
        .with_meta(crate::block_py::Meta::synthetic())
        .into();
        let next_value = Await::new(next_call)
            .with_meta(crate::block_py::Meta::synthetic())
            .into();
        let next_assign = synthetic_assign(synthetic_name_expr(tmp_name), next_value);
        let fetch_next = crate::block_py::StmtTry::new(
            vec![next_assign],
            vec![stop_iteration_handler_with_body(
                "StopAsyncIteration",
                stop_body,
            )],
            Vec::new(),
            Vec::new(),
            false,
        )
        .with_meta(crate::block_py::Meta::synthetic())
        .into();
        vec![fetch_next]
    } else {
        let next_call: InstrRuff = Call::new(
            runtime_helper_expr("next"),
            vec![CallArgPositional::Positional(synthetic_name_expr(
                iter_name,
            ))],
            Vec::new(),
        )
        .with_meta(crate::block_py::Meta::synthetic())
        .into();
        let next_assign = synthetic_assign(synthetic_name_expr(tmp_name), next_call);
        let fetch_next = crate::block_py::StmtTry::new(
            vec![next_assign],
            vec![stop_iteration_handler_with_body("StopIteration", stop_body)],
            Vec::new(),
            Vec::new(),
            false,
        )
        .with_meta(crate::block_py::Meta::synthetic())
        .into();
        vec![fetch_next]
    };
    while_body.extend(assign_body);
    while_body.extend(for_stmt.body);

    let while_stmt: InstrRuff = crate::block_py::StmtWhile::new(
        crate::passes::ast_to_instr::from_ast_expr(py_expr!("True")),
        while_body,
        Vec::new(),
    )
    .with_meta(crate::block_py::Meta::synthetic())
    .into();

    let mut expanded = vec![iterable_assign, iter_setup];
    if let Some(completed_name) = &completed_name {
        expanded.push(synthetic_assign(
            synthetic_name_expr(completed_name),
            crate::passes::ast_to_instr::from_ast_expr(py_expr!("False")),
        ));
    }
    expanded.push(while_stmt);
    if let Some(completed_name) = completed_name {
        expanded.push(
            crate::block_py::StmtIf::new(
                crate::passes::ast_to_instr::from_ast_expr(py_expr!(
                    "{completed:id}",
                    completed = completed_name.as_str()
                )),
                for_stmt.orelse,
                Vec::new(),
            )
            .with_meta(crate::block_py::Meta::synthetic())
            .into(),
        );
    }
    expanded
}

pub(crate) fn lower_stmt_sequence_with_state<E>(
    context: &Context,
    stmts: &[InstrRuff],
    targets: RegionTargets,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
) -> BlockLabel
where
    E: RuffToBlockPyExpr,
{
    lower_instr_stmt_sequence_with_state(context, stmts, targets, blocks, name_gen)
}

fn lower_instr_stmt_sequence_with_state<E>(
    context: &Context,
    stmts: &[InstrRuff],
    targets: RegionTargets,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
) -> BlockLabel
where
    E: RuffToBlockPyExpr,
{
    if stmts.is_empty() {
        return targets.normal_cont;
    }

    let mut linear = Vec::new();
    let mut index = 0;
    while index < stmts.len() {
        let plan;
        (linear, index, plan) =
            match drive_instr_sequence_until_control(context, &stmts[index..], linear) {
                InstrSequenceDriveResult::Exhausted { linear } => {
                    return emit_sequence_jump_block(
                        context,
                        blocks,
                        name_gen,
                        linear,
                        targets.normal_cont.clone(),
                        targets.active_exc.as_ref(),
                    );
                }
                InstrSequenceDriveResult::Break {
                    linear,
                    index: break_index,
                    plan,
                } => (linear, index + break_index, plan),
            };

        match plan {
            plan @ (StmtSequenceHeadPlan::Raise(_)
            | StmtSequenceHeadPlan::Return(_)
            | StmtSequenceHeadPlan::If(_)
            | StmtSequenceHeadPlan::While(_)
            | StmtSequenceHeadPlan::Break
            | StmtSequenceHeadPlan::Continue) => {
                let label = lower_common_stmt_sequence_head(
                    context,
                    name_gen,
                    plan,
                    &stmts[index + 1..],
                    targets.clone(),
                    linear.clone(),
                    blocks,
                    &mut || name_gen.next_block_name(),
                    &mut |stmts, nested_targets, blocks| {
                        lower_instr_stmt_sequence_with_state(
                            context,
                            stmts,
                            nested_targets,
                            blocks,
                            name_gen,
                        )
                    },
                );
                if let Some(label) = label {
                    return label;
                }
                unreachable!("common head helper must lower supported head");
            }
            StmtSequenceHeadPlan::With(with_stmt) => {
                let needs_finally_return_flow = contains_return_instr_in_body(&with_stmt.body);
                let entry = lower_with_stmt_sequence(
                    context,
                    with_stmt,
                    &stmts[index + 1..],
                    targets.clone(),
                    linear.clone(),
                    blocks,
                    name_gen,
                    needs_finally_return_flow,
                    &mut |stmts, nested_targets, blocks| {
                        lower_instr_stmt_sequence_with_state(
                            context,
                            stmts,
                            nested_targets,
                            blocks,
                            name_gen,
                        )
                    },
                );
                return entry;
            }
            StmtSequenceHeadPlan::For(for_stmt) => {
                let iter_name = name_gen.next_tmp_name("iter");
                let tmp_name = name_gen.next_tmp_name("tmp");
                let assign_body = build_for_target_assign_body(
                    *for_stmt.target.clone(),
                    crate::passes::ast_to_instr::from_ast_expr(py_expr!(
                        "{name:id}",
                        name = tmp_name.as_str()
                    )),
                    tmp_name.as_str(),
                );
                let mut expanded = linear;
                expanded.extend(expand_for_stmt(
                    name_gen,
                    for_stmt,
                    iter_name.as_str(),
                    tmp_name.as_str(),
                    assign_body,
                ));
                expanded.extend_from_slice(&stmts[index + 1..]);
                return lower_instr_stmt_sequence_with_state(
                    context, &expanded, targets, blocks, name_gen,
                );
            }
            StmtSequenceHeadPlan::Try(try_stmt) => {
                let label = if try_stmt.is_star {
                    let jump_label = (!linear.is_empty()).then(|| name_gen.next_block_name());
                    lower_star_try_stmt_sequence(
                        context,
                        name_gen,
                        try_stmt,
                        &stmts[index + 1..],
                        targets.clone(),
                        linear.clone(),
                        blocks,
                        jump_label,
                        &mut |stmts, nested_targets, blocks| {
                            lower_instr_stmt_sequence_with_state(
                                context,
                                stmts,
                                nested_targets,
                                blocks,
                                name_gen,
                            )
                        },
                    )
                } else {
                    let has_finally = !&try_stmt.finalbody.is_empty();
                    let needs_finally_return_flow = has_finally
                        && (contains_return_instr_in_body(&try_stmt.body)
                            || contains_return_stmt_in_handlers(&try_stmt.handlers)
                            || contains_return_instr_in_body(&try_stmt.orelse));
                    let try_plan = build_try_plan(name_gen, has_finally, needs_finally_return_flow);
                    let label = name_gen.next_block_name();
                    lower_try_stmt_sequence(
                        try_stmt,
                        &stmts[index + 1..],
                        targets.clone(),
                        linear.clone(),
                        blocks,
                        name_gen,
                        label.clone(),
                        try_plan,
                        &mut |stmts, nested_targets, blocks| {
                            lower_instr_stmt_sequence_with_state(
                                context,
                                stmts,
                                nested_targets,
                                blocks,
                                name_gen,
                            )
                        },
                    )
                };
                return label;
            }
            StmtSequenceHeadPlan::Linear(_) | StmtSequenceHeadPlan::FunctionDef(_) => {
                unreachable!("sequence driver should consume linear/functiondef heads")
            }
            StmtSequenceHeadPlan::Expanded(expanded_stmts) => {
                let jump_label = (!linear.is_empty()).then(|| name_gen.next_block_name());
                return lower_expanded_stmt_sequence(
                    context,
                    name_gen,
                    expanded_stmts,
                    &stmts[index + 1..],
                    targets,
                    linear.clone(),
                    blocks,
                    jump_label,
                    &mut |stmts, nested_targets, blocks| {
                        lower_instr_stmt_sequence_with_state(
                            context,
                            stmts,
                            nested_targets,
                            blocks,
                            name_gen,
                        )
                    },
                );
            }
            StmtSequenceHeadPlan::Unsupported => return targets.normal_cont,
        }
    }

    emit_sequence_jump_block(
        context,
        blocks,
        name_gen,
        linear,
        targets.normal_cont,
        targets.active_exc.as_ref(),
    )
}

pub(crate) fn lower_expanded_stmt_sequence<F, E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    desugared_stmts: Vec<InstrRuff>,
    remaining_stmts: &[InstrRuff],
    targets: RegionTargets,
    linear: Vec<InstrRuff>,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    jump_label: Option<BlockLabel>,
    lower_sequence: &mut F,
) -> BlockLabel
where
    F: FnMut(&[InstrRuff], RegionTargets, &mut Vec<LoweredBlockPyBlock<E>>) -> BlockLabel,
    E: RuffToBlockPyExpr,
{
    let mut expanded = desugared_stmts;
    expanded.extend_from_slice(remaining_stmts);
    let active_exc = targets.active_exc.clone();
    let expanded_entry = lower_sequence(&expanded, targets, blocks);
    if linear.is_empty() {
        return expanded_entry;
    }
    let jump_label = jump_label.expect("linear prefix requires a jump label");
    let lowered_linear = lower_stmts_to_blockpy_stmts_with_context::<E>(context, &linear, name_gen)
        .unwrap_or_else(|err| {
            panic!("failed to lower expanded-sequence jump prefix through production path: {err}")
        });
    let entry = crate::passes::ruff_to_blockpy::compat::emit_lowered_builder_fragment_with_preferred_linear_entry_and_expr(
        blocks,
        lowered_linear,
        jump_label,
        BlockTerm::Jump(BlockEdge::new(BlockLabel::fallthrough())),
        expanded_entry,
        active_exc.as_ref(),
    );
    entry.label()
}

pub(crate) fn lower_if_stmt_sequence<F, E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    linear: Vec<InstrRuff>,
    test: InstrRuff,
    then_body: &[InstrRuff],
    else_body: &[InstrRuff],
    rest_entry: BlockLabel,
    targets: &RegionTargets,
    lower_region: &mut F,
) -> BlockLabel
where
    F: FnMut(&[InstrRuff], RegionTargets, &mut Vec<LoweredBlockPyBlock<E>>) -> BlockLabel,
    E: RuffToBlockPyExpr,
{
    let then_entry = lower_region(
        then_body,
        RegionTargets {
            normal_cont: rest_entry.clone(),
            loop_labels: targets.loop_labels.clone(),
            active_exc: targets.active_exc.clone(),
        },
        blocks,
    );
    let else_entry = lower_region(
        else_body,
        RegionTargets {
            normal_cont: rest_entry,
            loop_labels: targets.loop_labels.clone(),
            active_exc: targets.active_exc.clone(),
        },
        blocks,
    );
    emit_if_branch_block_with_expr_setup_and_expr(
        context,
        blocks,
        name_gen,
        linear,
        test,
        then_entry,
        else_entry,
        targets.active_exc.as_ref(),
    )
    .unwrap_or_else(|err| panic!("failed to lower sequence if head through expr seam: {err}"))
}

pub(crate) fn lower_if_stmt_sequence_from_stmt<F, E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    if_stmt: crate::block_py::StmtIf<InstrRuff>,
    remaining_stmts: &[InstrRuff],
    targets: RegionTargets,
    linear: Vec<InstrRuff>,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    lower_region: &mut F,
) -> BlockLabel
where
    F: FnMut(&[InstrRuff], RegionTargets, &mut Vec<LoweredBlockPyBlock<E>>) -> BlockLabel,
    E: RuffToBlockPyExpr,
{
    let rest_entry = lower_region(remaining_stmts, targets.clone(), blocks);
    let loop_ctx = targets.loop_labels.as_ref().map(|loop_labels| LoopContext {
        continue_label: loop_labels.continue_label.clone(),
        break_label: loop_labels.break_label.clone(),
    });
    if let Some(Ok(fragment)) = stmt_lowering::try_lower_if_instr_fragment::<E>(
        context,
        name_gen,
        &if_stmt,
        loop_ctx.as_ref(),
    ) {
        let fragment_entry = emit_inline_fragment_with_exc_target_and_expr(
            blocks,
            fragment,
            rest_entry,
            targets.active_exc.as_ref(),
        );
        if linear.is_empty() {
            return fragment_entry.label();
        }
        return emit_sequence_jump_block(
            context,
            blocks,
            name_gen,
            linear,
            fragment_entry.label(),
            targets.active_exc.as_ref(),
        );
    }

    let then_body = &if_stmt.body.to_vec();
    let else_body = if_stmt.orelse.clone();
    lower_if_stmt_sequence(
        context,
        blocks,
        name_gen,
        linear,
        *if_stmt.test,
        &then_body,
        &else_body,
        rest_entry,
        &targets,
        lower_region,
    )
}

pub(crate) fn lower_while_stmt_sequence<F, E>(
    context: &Context,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    test_label: BlockLabel,
    linear_label: Option<BlockLabel>,
    linear: Vec<InstrRuff>,
    test: InstrRuff,
    body: &[InstrRuff],
    else_body: &[InstrRuff],
    remaining_stmts: &[InstrRuff],
    targets: RegionTargets,
    lower_region: &mut F,
) -> BlockLabel
where
    F: FnMut(&[InstrRuff], RegionTargets, &mut Vec<LoweredBlockPyBlock<E>>) -> BlockLabel,
    E: RuffToBlockPyExpr,
{
    let rest_entry = lower_region(remaining_stmts, targets.clone(), blocks);
    let cond_false_entry = if else_body.is_empty() {
        rest_entry.clone()
    } else {
        lower_region(else_body, targets.nested(rest_entry.clone()), blocks)
    };
    let body_entry = lower_region(
        body,
        targets.nested_with_loop(
            test_label.clone(),
            Some(LoopLabels {
                break_label: rest_entry,
                continue_label: test_label.clone(),
            }),
        ),
        blocks,
    );
    emit_simple_while_blocks_with_expr_setup_and_expr(
        context,
        blocks,
        name_gen,
        test_label,
        linear_label,
        linear,
        test,
        body_entry,
        cond_false_entry,
        targets.active_exc.as_ref(),
    )
    .unwrap_or_else(|err| panic!("failed to lower sequence while head through expr seam: {err}"))
}

pub(crate) fn lower_while_stmt_sequence_from_stmt<F, E>(
    context: &Context,
    name_gen: &FunctionNameGen,
    while_stmt: crate::block_py::StmtWhile<InstrRuff>,
    remaining_stmts: &[InstrRuff],
    targets: RegionTargets,
    linear: Vec<InstrRuff>,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    test_label: BlockLabel,
    linear_label: Option<BlockLabel>,
    lower_region: &mut F,
) -> BlockLabel
where
    F: FnMut(&[InstrRuff], RegionTargets, &mut Vec<LoweredBlockPyBlock<E>>) -> BlockLabel,
    E: RuffToBlockPyExpr,
{
    let body = &while_stmt.body.to_vec();
    let else_body = &while_stmt.orelse.to_vec();
    lower_while_stmt_sequence(
        context,
        blocks,
        name_gen,
        test_label,
        linear_label,
        linear,
        *while_stmt.test,
        &body,
        &else_body,
        remaining_stmts,
        targets,
        lower_region,
    )
}

#[cfg(test)]
mod stop_iteration_handler_tests {
    use super::*;

    #[test]
    fn synthetic_iteration_handlers_use_compiler_owned_runtime_exceptions() {
        for exception_name in ["StopIteration", "StopAsyncIteration"] {
            let ast::ExceptHandler::ExceptHandler(handler) =
                stop_iteration_handler_with_body(exception_name, Vec::new());
            let Some(ast::Expr::Attribute(exception)) = handler.type_.as_deref() else {
                panic!("synthetic {exception_name} handler must use a runtime attribute");
            };
            assert_eq!(exception.attr.as_str(), exception_name);
            let ast::Expr::Name(runtime) = exception.value.as_ref() else {
                panic!("synthetic {exception_name} handler must load the runtime namespace");
            };
            assert_eq!(runtime.id.as_str(), "__soac__");
        }
    }
}
