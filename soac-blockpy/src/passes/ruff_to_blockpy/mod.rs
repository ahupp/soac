use crate::block_py::cfg::{
    fold_jumps_to_trivial_none_return_blockpy, prune_unreachable_blockpy_blocks,
};
use crate::block_py::param_specs::ParamSpec;
use crate::block_py::{
    assert_blockpy_block_normalized, Block, BlockBuilder, BlockEdge, BlockLabel,
    BlockPyFallthroughTerm, BlockPyFunction, BlockPyModule, BlockTerm, CallableScopeInfo,
    FunctionKind, FunctionName, FunctionNameGen, Instr, StructuredInstr,
};
use crate::namegen::fresh_name;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::CoreBlockPyPassWithAwaitAndYield;
use crate::ruff_ast_to_string;
use crate::template::is_simple;
use crate::{py_expr, py_stmt};
use ruff_python_ast::{self as ast, Expr, Stmt};
use std::collections::HashMap;
mod bb_shape;
mod compat;
pub(crate) mod expr_lowering;
mod module_plan;
mod stmt_lowering;
mod stmt_sequences;
mod try_regions;

#[cfg(test)]
pub(crate) use bb_shape::lower_structured_located_blocks_to_bb_blocks;
pub(crate) use bb_shape::{
    lower_structured_blocks_to_bb_blocks, lowered_exception_edges, populate_exception_edge_args,
    rewrite_current_exception_in_core_blocks,
    rewrite_current_exception_in_core_blocks_with_await_and_yield,
};
pub(crate) use module_plan::rewrite_ast_to_core_blockpy_module_plan_with_module;

pub(crate) use compat::{
    compat_block_from_blockpy_with_exc_target_and_expr, emit_for_loop_blocks,
    emit_inline_fragment_with_exc_target_and_expr,
    emit_if_branch_block_with_expr_setup_and_expr, emit_sequence_jump_block,
    emit_sequence_raise_block_with_expr_setup_and_expr,
    emit_sequence_return_block_with_expr_setup_and_expr,
    emit_simple_while_blocks_with_expr_setup_and_expr,
};
pub(crate) use expr_lowering::RuffToBlockPyExpr;
pub(crate) use stmt_lowering::{
    build_for_target_assign_body, lower_star_try_stmt_sequence, lower_try_stmt_sequence,
    lower_with_stmt_sequence,
};
pub(crate) use stmt_sequences::{
    lower_expanded_stmt_sequence, lower_stmt_sequence_with_state, lower_stmts_to_blockpy_stmts,
};
pub(crate) use try_regions::{
    block_references_label, build_try_plan, finalize_try_regions, lower_try_regions,
    prepare_except_body, prepare_finally_body, TryPlan,
};

pub(crate) type LoweredBlockPyBlock<E = Expr> = Block<StructuredInstr<E>, E>;
pub(crate) type InlineBlockBuilder<I> = BlockBuilder<I, BlockTerm<I>>;

#[derive(Debug, Clone)]
pub(crate) struct InlineFragment<I: Instr> {
    pub entry: InlineBlockBuilder<I>,
    pub deps: Vec<Block<I, I>>,
}

impl<I: Instr> InlineFragment<I> {
    pub(crate) fn new(entry: InlineBlockBuilder<I>, deps: Vec<Block<I, I>>) -> Self {
        Self { entry, deps }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct LoweredExpr<I: Instr> {
    pub setup: InlineFragment<I>,
    pub value: I,
}

pub(crate) fn rewrite_ast_to_core_blockpy_module_with_module(
    context: &Context,
    module: Vec<Stmt>,
    semantic_state: &crate::passes::ast_to_ast::semantic::SemanticAstState,
    module_name_gen: crate::block_py::ModuleNameGen,
) -> BlockPyModule<CoreBlockPyPassWithAwaitAndYield> {
    rewrite_ast_to_core_blockpy_module_plan_with_module(
        context,
        module,
        semantic_state,
        module_name_gen,
    )
}

#[derive(Clone)]
pub(crate) enum StmtSequenceHeadPlan {
    Linear(Stmt),
    Expanded(Vec<Stmt>),
    FunctionDef(ast::StmtFunctionDef),
    Raise(ast::StmtRaise),
    Return(Option<Expr>),
    If(ast::StmtIf),
    While(ast::StmtWhile),
    For(ast::StmtFor),
    Try(ast::StmtTry),
    With(ast::StmtWith),
    Break,
    Continue,
    Unsupported,
}

pub(crate) enum StmtSequenceDriveResult {
    Exhausted {
        linear: Vec<Stmt>,
    },
    Break {
        linear: Vec<Stmt>,
        index: usize,
        plan: StmtSequenceHeadPlan,
    },
}

pub(crate) fn attach_exception_edges_to_blocks<S, E: Instr>(
    blocks: Vec<Block<S, E>>,
    exception_edges: &HashMap<BlockLabel, Option<BlockLabel>>,
) -> Vec<Block<S, E>> {
    blocks
        .into_iter()
        .map(|block| Block {
            label: block.label.clone(),
            body: block.body,
            term: block.term,
            params: block.params,
            exc_edge: exception_edges
                .get(&block.label)
                .cloned()
                .flatten()
                .map(BlockEdge::new),
        })
        .collect()
}

fn move_entry_block_to_front<S, T: Instr>(blocks: &mut Vec<Block<S, T>>, entry_label: BlockLabel) {
    if let Some(entry_index) = blocks.iter().position(|block| block.label == entry_label) {
        if entry_index != 0 {
            let entry_block = blocks.remove(entry_index);
            blocks.insert(0, entry_block);
        }
    }
}

pub(crate) fn build_core_blockpy_callable_def_from_runtime_input(
    context: &Context,
    name_gen: FunctionNameGen,
    names: FunctionName,
    params: ParamSpec,
    runtime_input_body: &[Stmt],
    doc: Option<String>,
    end_label: BlockLabel,
    blockpy_kind: FunctionKind,
    scope: &CallableScopeInfo,
) -> BlockPyFunction<CoreBlockPyPassWithAwaitAndYield> {
    let function_id = name_gen.function_id();
    let mut blocks = Vec::new();
    let entry_label =
        lower_stmt_sequence_with_state::<crate::block_py::InstrWithAwaitAndYield>(
            context,
            runtime_input_body,
            RegionTargets::new(end_label.clone(), None),
            &mut blocks,
            &name_gen,
        );
    move_entry_block_to_front(&mut blocks, entry_label.clone());
    for block in &blocks {
        assert_blockpy_block_normalized(block);
    }
    let needs_end_block = entry_label == end_label
        || blocks
            .iter()
            .any(|block| block_references_label(block, &end_label));
    if needs_end_block {
        blocks.push(Block {
            label: end_label,
            body: Vec::new(),
            term: BlockTerm::implicit_function_return(),
            params: Vec::new(),
            exc_edge: None,
        });
    }
    fold_jumps_to_trivial_none_return_blockpy(&mut blocks);
    let extra_roots = blocks
        .iter()
        .filter_map(|block| block.exc_edge.as_ref().map(|edge| edge.target.clone()))
        .collect::<Vec<_>>();
    prune_unreachable_blockpy_blocks(entry_label, &extra_roots, &mut blocks);
    let mut blocks = lower_structured_blocks_to_bb_blocks(&name_gen, &blocks);
    if matches!(blockpy_kind, FunctionKind::Function) {
        rewrite_current_exception_in_core_blocks_with_await_and_yield(&mut blocks[..]);
    }
    BlockPyFunction {
        function_id,
        name_gen,
        names,
        kind: blockpy_kind,
        params,
        blocks,
        doc,
        storage_layout: None,
        scope: scope.clone(),
    }
}

#[derive(Clone)]
pub(crate) struct LoopContext {
    continue_label: BlockLabel,
    break_label: BlockLabel,
}

#[derive(Clone)]
pub(crate) struct LoopLabels {
    pub break_label: BlockLabel,
    pub continue_label: BlockLabel,
}

#[derive(Clone)]
pub(crate) struct RegionTargets {
    pub normal_cont: BlockLabel,
    pub loop_labels: Option<LoopLabels>,
    pub active_exc: Option<BlockLabel>,
}

impl RegionTargets {
    pub(crate) fn new(normal_cont: impl Into<BlockLabel>, active_exc: Option<BlockLabel>) -> Self {
        Self {
            normal_cont: normal_cont.into(),
            loop_labels: None,
            active_exc,
        }
    }

    pub(crate) fn nested(&self, normal_cont: impl Into<BlockLabel>) -> Self {
        Self {
            normal_cont: normal_cont.into(),
            loop_labels: self.loop_labels.clone(),
            active_exc: self.active_exc.clone(),
        }
    }

    pub(crate) fn nested_with_loop(
        &self,
        normal_cont: impl Into<BlockLabel>,
        loop_labels: Option<LoopLabels>,
    ) -> Self {
        Self {
            normal_cont: normal_cont.into(),
            loop_labels,
            active_exc: self.active_exc.clone(),
        }
    }
}

fn assign_delete_error(message: &str, stmt: &Stmt) -> String {
    format!("{message}\nstmt:\n{}", ruff_ast_to_string(stmt).trim_end())
}

#[cfg(test)]
mod test;
