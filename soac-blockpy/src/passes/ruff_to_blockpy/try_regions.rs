use super::compat::set_region_exc_param;
use super::*;
use crate::block_py::{
    AbruptKind, Block, BlockArg, BlockBuilder, BlockEdge, BlockLabel, BlockParamRole, BlockTerm,
    Instr, Meta, Store, TermBranchTable, TermRaise, WithMeta,
};
use crate::passes::ast_to_ast::body::Suite;
use crate::passes::ast_to_ast::context::Context;

fn expr_name(id: &str) -> ast::name::Name {
    ast::name::Name::new(id)
}

#[derive(Debug, Clone)]
pub(crate) struct TryPlan {
    pub except_exc_name: String,
    pub finally_abrupt_kind_name: Option<String>,
    pub finally_abrupt_payload_name: Option<String>,
    pub finally_dispatch_label: Option<BlockLabel>,
    pub finally_return_label: Option<BlockLabel>,
    pub finally_raise_label: Option<BlockLabel>,
}

pub(crate) fn build_try_plan(
    name_gen: &FunctionNameGen,
    has_finally: bool,
    _needs_finally_return_flow: bool,
) -> TryPlan {
    let except_exc_name = name_gen.next_tmp_name("try_exc").to_string();
    let finally_abrupt_kind_name = if has_finally {
        Some(name_gen.next_tmp_name("try_abrupt_kind").to_string())
    } else {
        None
    };
    let finally_abrupt_payload_name = if has_finally {
        Some(name_gen.next_tmp_name("try_abrupt_payload").to_string())
    } else {
        None
    };
    let finally_dispatch_label = if has_finally {
        Some(name_gen.next_block_name())
    } else {
        None
    };
    let finally_return_label = if has_finally {
        Some(name_gen.next_block_name())
    } else {
        None
    };
    let finally_raise_label = if has_finally {
        Some(name_gen.next_block_name())
    } else {
        None
    };
    TryPlan {
        except_exc_name,
        finally_abrupt_kind_name,
        finally_abrupt_payload_name,
        finally_dispatch_label,
        finally_return_label,
        finally_raise_label,
    }
}

impl TryPlan {
    pub(crate) fn finally_cont_label(&self, rest_entry: &BlockLabel) -> BlockLabel {
        self.finally_dispatch_label
            .as_ref()
            .cloned()
            .unwrap_or_else(|| rest_entry.clone())
    }
}

pub(crate) fn prepare_finally_body(finalbody: &Suite) -> Vec<Stmt> {
    finalbody.to_vec()
}

pub(crate) fn prepare_except_body(handlers: &[ast::ExceptHandler]) -> Vec<Stmt> {
    handlers
        .first()
        .map(|handler| {
            let ast::ExceptHandler::ExceptHandler(handler) = handler;
            handler.body.to_vec()
        })
        .unwrap_or_else(|| vec![py_stmt!("raise")])
}

pub(crate) struct LoweredTryRegions {
    pub body_label: BlockLabel,
    pub body_region_range: std::ops::Range<usize>,
    pub else_region_range: std::ops::Range<usize>,
    pub except_region_range: Option<std::ops::Range<usize>>,
    pub finally_region_range: Option<std::ops::Range<usize>>,
    pub finally_label: Option<BlockLabel>,
}

pub(crate) fn lower_try_regions<F, E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    try_plan: &TryPlan,
    rest_entry: &BlockLabel,
    finally_body: Option<Vec<Stmt>>,
    else_body: Vec<Stmt>,
    try_body: Vec<Stmt>,
    except_body: Option<Vec<Stmt>>,
    loop_labels: Option<LoopLabels>,
    active_exc_target: Option<BlockLabel>,
    lower_sequence: &mut F,
) -> LoweredTryRegions
where
    F: FnMut(&[Stmt], RegionTargets, &mut Vec<LoweredBlockPyBlock<E>>) -> BlockLabel,
    E: RuffToBlockPyExpr,
{
    let finally_label = if let Some(finally_body) = finally_body {
        let finally_region_start = blocks.len();
        let finally_label = lower_sequence(
            &finally_body,
            RegionTargets {
                normal_cont: try_plan.finally_cont_label(rest_entry),
                loop_labels: loop_labels.clone(),
                active_exc: active_exc_target.clone(),
            },
            blocks,
        );
        let finally_region_end = blocks.len();
        if let Some(finally_entry) = blocks.iter_mut().find(|block| block.label == finally_label) {
            if let Some(kind_name) = try_plan.finally_abrupt_kind_name.as_ref() {
                finally_entry.ensure_param(kind_name.clone(), BlockParamRole::AbruptKind);
            }
            if let Some(payload_name) = try_plan.finally_abrupt_payload_name.as_ref() {
                finally_entry.ensure_param(payload_name.clone(), BlockParamRole::AbruptPayload);
            }
        }
        let finally_normal_entry = try_plan.finally_abrupt_kind_name.as_ref().map(|_| {
            let normal_label = name_gen.next_block_name();
            let mut args = Vec::new();
            args.push(BlockArg::AbruptKind(AbruptKind::Fallthrough));
            args.push(BlockArg::None);
            blocks.push(Block::from_builder(
                normal_label.clone(),
                BlockBuilder::with_term(
                    Vec::new(),
                    Some(BlockTerm::Jump(BlockEdge::with_args(finally_label, args))),
                ),
                Vec::new(),
                active_exc_target.clone().map(BlockEdge::new),
                None,
            ));
            normal_label
        });
        let finally_exception_entry = try_plan.finally_abrupt_kind_name.as_ref().map(|_| {
            let exception_label = name_gen.next_block_name();
            let args = vec![
                BlockArg::AbruptKind(AbruptKind::Exception),
                BlockArg::Name(try_plan.except_exc_name.clone()),
            ];
            let mut block = Block::from_builder(
                exception_label.clone(),
                BlockBuilder::with_term(
                    Vec::new(),
                    Some(BlockTerm::Jump(BlockEdge::with_args(finally_label, args))),
                ),
                Vec::new(),
                active_exc_target.clone().map(BlockEdge::new),
                None,
            );
            block.set_exception_param(try_plan.except_exc_name.clone());
            blocks.push(block);
            exception_label
        });
        if let (
            Some(finally_return_label),
            Some(finally_dispatch_label),
            Some(finally_raise_label),
            Some(payload_name),
            Some(kind_name),
        ) = (
            try_plan.finally_return_label.clone(),
            try_plan.finally_dispatch_label.clone(),
            try_plan.finally_raise_label.clone(),
            try_plan.finally_abrupt_payload_name.as_ref(),
            try_plan.finally_abrupt_kind_name.as_ref(),
        ) {
            emit_finally_abrupt_dispatch_blocks(
                name_gen,
                blocks,
                finally_return_label,
                finally_raise_label,
                finally_dispatch_label,
                payload_name,
                kind_name,
                rest_entry.clone(),
                active_exc_target.clone(),
            );
        }
        Some((
            finally_label,
            finally_region_start..finally_region_end,
            finally_normal_entry,
            finally_exception_entry,
        ))
    } else {
        None
    };

    let cleanup_target = finally_label
        .as_ref()
        .map(|(label, _, normal_entry, _)| normal_entry.clone().unwrap_or_else(|| label.clone()))
        .unwrap_or_else(|| rest_entry.clone());
    let cleanup_exc_target = finally_label
        .as_ref()
        .map(|(label, _, _, finally_exception_entry)| {
            finally_exception_entry
                .clone()
                .unwrap_or_else(|| label.clone())
        })
        .or(active_exc_target.clone());

    let else_region_start = blocks.len();
    let else_entry = if else_body.is_empty() {
        cleanup_target.clone()
    } else {
        lower_sequence(
            &else_body,
            RegionTargets {
                normal_cont: cleanup_target.clone(),
                loop_labels: loop_labels.clone(),
                active_exc: cleanup_exc_target.clone(),
            },
            blocks,
        )
    };
    let else_region_end = blocks.len();

    let except_region_range;
    let except_label = if let Some(except_body) = except_body {
        let except_region_start = blocks.len();
        let except_label = lower_sequence(
            &except_body,
            RegionTargets {
                normal_cont: cleanup_target,
                loop_labels: loop_labels.clone(),
                active_exc: cleanup_exc_target,
            },
            blocks,
        );
        let except_region_end = blocks.len();
        except_region_range = Some(except_region_start..except_region_end);
        except_label
    } else if let Some((finally_label, _, _, finally_exception_entry)) = finally_label.clone() {
        except_region_range = None;
        finally_exception_entry.unwrap_or(finally_label)
    } else {
        panic!("expected except body or finally body when lowering try");
    };

    let body_region_start = blocks.len();
    let body_label = lower_sequence(
        &try_body,
        RegionTargets {
            normal_cont: else_entry,
            loop_labels,
            active_exc: Some(except_label.clone()),
        },
        blocks,
    );
    let body_region_end = blocks.len();

    LoweredTryRegions {
        body_label,
        body_region_range: body_region_start..body_region_end,
        else_region_range: else_region_start..else_region_end,
        except_region_range,
        finally_region_range: finally_label.as_ref().map(|(_, range, _, _)| range.clone()),
        finally_label: finally_label.map(|(label, _, _, _)| label),
    }
}

pub(crate) fn finalize_try_regions<E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    label: BlockLabel,
    linear: Vec<Stmt>,
    try_plan: TryPlan,
    lowered_try: LoweredTryRegions,
    active_exc_target: Option<BlockLabel>,
) -> BlockLabel
where
    E: RuffToBlockPyExpr,
{
    if let Some(finally_target) = lowered_try.finally_label.as_ref() {
        let payload_name = try_plan
            .finally_abrupt_payload_name
            .as_deref()
            .expect("finally region must have abrupt payload slot");
        rewrite_region_returns_to_finally_blockpy(
            &mut blocks[lowered_try.body_region_range.clone()],
            finally_target,
            payload_name,
        );
        rewrite_region_returns_to_finally_blockpy(
            &mut blocks[lowered_try.else_region_range.clone()],
            finally_target,
            payload_name,
        );
        if let Some(except_region_range) = lowered_try.except_region_range.as_ref() {
            rewrite_region_returns_to_finally_blockpy(
                &mut blocks[except_region_range.clone()],
                finally_target,
                payload_name,
            );
        }
    }

    if let Some(except_region_range) = lowered_try.except_region_range.as_ref() {
        set_region_exc_param(
            blocks,
            except_region_range,
            try_plan.except_exc_name.as_str(),
        );
    }
    if let Some(finally_region_range) = lowered_try.finally_region_range.as_ref() {
        set_region_exc_param(
            blocks,
            finally_region_range,
            try_plan.except_exc_name.as_str(),
        );
    }
    emit_try_jump_entry(
        blocks,
        name_gen,
        label,
        linear,
        lowered_try.body_label,
        active_exc_target,
    )
}

fn rewrite_region_returns_to_finally_blockpy<E>(
    blocks: &mut [Block<E>],
    finally_target: &BlockLabel,
    payload_name: &str,
) where
    E: RuffToBlockPyExpr,
{
    for block in blocks.iter_mut() {
        let ret_value =
            match std::mem::replace(&mut block.term, BlockTerm::Return(E::constant_none())) {
                BlockTerm::Return(value) => value,
                other => {
                    block.term = other;
                    continue;
                }
            };
        let target = expr_name(payload_name);
        let meta = Meta::synthetic();
        block
            .body
            .push(Store::new(target, ret_value).with_meta(meta).into());
        let payload_arg = BlockArg::Name(payload_name.to_string());
        block.term = BlockTerm::Jump(BlockEdge::with_args(
            finally_target.clone(),
            vec![BlockArg::AbruptKind(AbruptKind::Return), payload_arg],
        ));
    }
}

pub(crate) fn emit_finally_abrupt_dispatch_blocks<E>(
    name_gen: &FunctionNameGen,
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    finally_return_label: BlockLabel,
    finally_raise_label: BlockLabel,
    finally_dispatch_label: BlockLabel,
    payload_name: &str,
    kind_name: &str,
    rest_entry: BlockLabel,
    active_exc_target: Option<BlockLabel>,
) where
    E: RuffToBlockPyExpr,
{
    blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
        &Context::new(""),
        name_gen,
        finally_return_label.clone(),
        Vec::new(),
        BlockTerm::Return(E::from_lowered_expr(
            crate::passes::ast_to_instr::from_ast_expr(py_expr!("{name:id}", name = payload_name)),
        )),
        active_exc_target.as_ref(),
    ));
    blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
        &Context::new(""),
        name_gen,
        finally_raise_label.clone(),
        Vec::new(),
        BlockTerm::Raise(TermRaise {
            exc: Some(E::from_lowered_expr(
                crate::passes::ast_to_instr::from_ast_expr(py_expr!(
                    "{name:id}",
                    name = payload_name
                )),
            )),
        }),
        active_exc_target.as_ref(),
    ));
    blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
        &Context::new(""),
        name_gen,
        finally_dispatch_label.clone(),
        Vec::new(),
        BlockTerm::BranchTable(TermBranchTable {
            index: E::from_lowered_expr(crate::passes::ast_to_instr::from_ast_expr(py_expr!(
                "{name:id}",
                name = kind_name
            ))),
            targets: vec![
                rest_entry.clone(),
                finally_return_label,
                finally_raise_label,
            ],
            default_label: rest_entry,
        }),
        active_exc_target.as_ref(),
    ));
}

pub(crate) fn emit_try_jump_entry<E>(
    blocks: &mut Vec<LoweredBlockPyBlock<E>>,
    name_gen: &FunctionNameGen,
    label: BlockLabel,
    linear: Vec<Stmt>,
    body_label: BlockLabel,
    active_exc_target: Option<BlockLabel>,
) -> BlockLabel
where
    E: RuffToBlockPyExpr,
{
    blocks.push(compat_block_from_blockpy_with_exc_target_and_expr(
        &Context::new(""),
        name_gen,
        label.clone(),
        linear
            .into_iter()
            .map(crate::passes::ast_to_instr::from_ast_stmt)
            .collect(),
        BlockTerm::Jump(BlockEdge::new(body_label)),
        active_exc_target.as_ref(),
    ));
    label
}

pub(crate) fn block_references_label<E: Instr>(
    block: &crate::block_py::Block<E>,
    label: &BlockLabel,
) -> bool {
    fn term_references_label<E: Instr>(term: &BlockTerm<E>, label: &BlockLabel) -> bool {
        match term {
            BlockTerm::Jump(target) => &target.target == label,
            BlockTerm::IfTerm(if_term) => {
                &if_term.then_label == label || &if_term.else_label == label
            }
            BlockTerm::BranchTable(branch) => {
                &branch.default_label == label
                    || branch.targets.iter().any(|target| target == label)
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => false,
        }
    }

    block
        .exc_edge
        .as_ref()
        .is_some_and(|edge| &edge.target == label)
        || term_references_label(&block.term, label)
}
