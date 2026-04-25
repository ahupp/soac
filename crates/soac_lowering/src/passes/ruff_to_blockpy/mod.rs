use crate::block_py::cfg::{
    fold_jumps_to_trivial_return_blockpy, prune_unreachable_blockpy_blocks,
};
use crate::block_py::ParamSpec;
use crate::block_py::{
    Block, BlockBuilder, BlockEdge, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm,
    CallableScopeInfo, FunctionExecutionMode, FunctionKind, FunctionName, FunctionNameGen, Instr,
};
use crate::namegen::fresh_name;
use crate::passes::ast_to_ast::context::Context;
use crate::passes::ast_to_ast::rewrite_stmt::annotation::FUNCTION_ANNOTATE_PREFIX;
use crate::passes::{CoreModuleShapeWithAwaitAndYield, InstrRuff};
use crate::ruff_ast::ruff_ast_to_string;
use crate::template::is_simple;
use crate::template::{py_expr, py_stmt};
use ruff_python_ast::{self as ast, Expr, Stmt};
use std::collections::HashMap;
mod bb_shape;
mod compat;
pub(crate) mod expr_lowering;
mod module_plan;
mod param_specs;
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
    compat_block_from_blockpy_with_exc_target_and_expr,
    emit_if_branch_block_with_expr_setup_and_expr, emit_inline_fragment_with_exc_target_and_expr,
    emit_sequence_jump_block, emit_sequence_raise_block_with_expr_setup_and_expr,
    emit_sequence_return_block_with_expr_setup_and_expr,
    emit_simple_while_blocks_with_expr_setup_and_expr,
};
pub(crate) use expr_lowering::RuffToBlockPyExpr;
pub(crate) use stmt_lowering::{
    build_for_target_assign_body, lower_star_try_stmt_sequence, lower_try_stmt_sequence,
    lower_with_stmt_sequence,
};
pub(crate) use stmt_sequences::{lower_expanded_stmt_sequence, lower_stmt_sequence_with_state};
pub(crate) use try_regions::{
    block_references_label, build_try_plan, finalize_try_regions, lower_try_regions,
    prepare_except_body, prepare_finally_body, TryPlan,
};

pub(crate) type LoweredBlockPyBlock<E = Expr> = Block<E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct InlineBlockRef(BlockLabel);

impl InlineBlockRef {
    fn from_label(label: BlockLabel) -> Self {
        Self(label)
    }

    pub(crate) fn label(self) -> BlockLabel {
        self.0
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InlineBlockBuilder<I: Instr> {
    name_gen: FunctionNameGen,
    entry: InlineBlockRef,
    current_ref: InlineBlockRef,
    current_block: BlockBuilder<I>,
    deps: Vec<Block<I>>,
}

impl<I: Instr> InlineBlockBuilder<I> {
    pub(crate) fn new(name_gen: &FunctionNameGen) -> Self {
        let entry = InlineBlockRef::from_label(name_gen.next_block_name());
        Self {
            name_gen: name_gen.share(),
            entry,
            current_ref: entry,
            current_block: BlockBuilder::new(),
            deps: Vec::new(),
        }
    }

    pub(crate) fn push_stmt(&mut self, stmt: I) {
        self.current_block.push_stmt(stmt);
    }

    pub(crate) fn name_gen(&self) -> &FunctionNameGen {
        &self.name_gen
    }

    pub(crate) fn entry_ref(&self) -> InlineBlockRef {
        self.entry
    }

    pub(crate) fn set_term(&mut self, term: BlockTerm<I>) {
        self.current_block.set_term(term);
    }

    pub(crate) fn ensure_fallthrough_term(&mut self) {
        self.current_block.ensure_fallthrough_term();
    }

    pub(crate) fn append_fragment(&mut self, mut fragment: InlineFragment<I>) {
        assert!(
            self.current_block.term.is_none(),
            "cannot append inline fragment after builder terminator"
        );
        let continuation = InlineBlockRef::from_label(self.name_gen.next_block_name());
        self.current_block.set_term(BlockTerm::Jump(BlockEdge::new(
            fragment.entry_ref().label(),
        )));
        self.flush_current_block();
        fragment
            .entry
            .replace_fallthrough_target(continuation.label());
        for dep in &mut fragment.deps {
            dep.replace_fallthrough_target(continuation.label());
        }
        self.deps.push(fragment.entry);
        self.deps.extend(fragment.deps);
        self.current_ref = continuation;
        self.current_block = BlockBuilder::new();
    }

    pub(crate) fn finish_blocks_with_term(
        mut self,
        term: BlockTerm<I>,
    ) -> (InlineBlockRef, Vec<Block<I>>) {
        if self.current_block.term.is_none() {
            self.current_block.set_term(term);
        }
        self.finish_blocks()
    }

    pub(crate) fn finish_fallthrough(mut self) -> InlineFragment<I> {
        self.current_block.ensure_fallthrough_term();
        self.finish_fragment()
    }

    pub(crate) fn finish_fallthrough_blocks(mut self) -> (InlineBlockRef, Vec<Block<I>>) {
        self.current_block.ensure_fallthrough_term();
        self.finish_blocks()
    }

    pub(crate) fn finish_linear_block(
        mut self,
        label: BlockLabel,
        term: BlockTerm<I>,
    ) -> Option<Block<I>> {
        if !self.deps.is_empty() {
            return None;
        }
        if self.current_block.term.is_none() {
            self.current_block.set_term(term);
        }
        let current = self.current_block.finish();
        Some(Block::new(
            label,
            current.body,
            current.term.expect("explicit term"),
            Vec::new(),
            None,
        ))
    }

    pub(crate) fn can_finish_linear_block(&self) -> bool {
        self.deps.is_empty()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.deps.is_empty()
            && self.current_block.body.is_empty()
            && self.current_block.term.is_none()
    }

    fn flush_current_block(&mut self) {
        let current = std::mem::replace(&mut self.current_block, BlockBuilder::new()).finish();
        let term = current
            .term
            .expect("inline block builder current block must have an explicit terminator");
        self.deps.push(Block::new(
            self.current_ref.label(),
            current.body,
            term,
            Vec::new(),
            None,
        ));
    }

    fn finish_fragment(self) -> InlineFragment<I> {
        let (entry_ref, mut blocks) = self.finish_blocks();
        if blocks.len() == 1 {
            return InlineFragment::new(blocks.pop().expect("single block fragment"), Vec::new());
        }
        let entry = if let Some(entry_index) = blocks
            .iter()
            .position(|block| block.label == entry_ref.label())
        {
            blocks.remove(entry_index)
        } else {
            let first_target = blocks
                .first()
                .map(|block| block.label)
                .expect("multi-block fragment should contain at least one block");
            Block::new(
                entry_ref.label(),
                Vec::new(),
                BlockTerm::Jump(BlockEdge::new(first_target)),
                Vec::new(),
                None,
            )
        };
        InlineFragment::new(entry, blocks)
    }

    pub(crate) fn finish_blocks(self) -> (InlineBlockRef, Vec<Block<I>>) {
        let current = self.current_block.finish();
        let term = current
            .term
            .expect("inline block builder must end with an explicit terminator");
        let entry = Block::new(
            self.current_ref.label(),
            current.body,
            term,
            Vec::new(),
            None,
        );
        let mut blocks = self.deps;
        blocks.push(entry);
        if !blocks.iter().any(|block| block.label == self.entry.label()) {
            let target = blocks
                .first()
                .map(|block| block.label)
                .expect("inline block builder with a current block should produce blocks");
            blocks.insert(
                0,
                Block::new(
                    self.entry.label(),
                    Vec::new(),
                    BlockTerm::Jump(BlockEdge::new(target)),
                    Vec::new(),
                    None,
                ),
            );
        }
        (self.entry, blocks)
    }
}

#[cfg(test)]
impl<I: Instr> InlineBlockBuilder<I> {
    pub(crate) fn finish(self) -> InlineFragment<I> {
        self.finish_fallthrough()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InlineFragment<I: Instr> {
    pub entry: Block<I>,
    pub deps: Vec<Block<I>>,
}

impl<I: Instr> InlineFragment<I> {
    pub(crate) fn new(entry: Block<I>, deps: Vec<Block<I>>) -> Self {
        let fragment = Self { entry, deps };
        fragment.assert_well_formed();
        fragment
    }

    fn assert_well_formed(&self) {
        use std::collections::HashSet;

        assert!(
            !self.entry.label.is_fallthrough(),
            "inline fragment entry must have a real label"
        );

        let mut labels = HashSet::from([self.entry.label]);
        for block in &self.deps {
            assert!(
                !block.label.is_fallthrough(),
                "inline fragment dependency block must have a real label"
            );
            assert!(
                labels.insert(block.label),
                "duplicate block label in inline fragment: {}",
                block.label
            );
        }

        let block_summaries = std::iter::once(&self.entry)
            .chain(self.deps.iter())
            .map(|block| match &block.term {
                BlockTerm::Jump(edge) => format!("{}: jump {}", block.label, edge.target),
                BlockTerm::IfTerm(if_term) => {
                    format!(
                        "{}: if {} / {}",
                        block.label, if_term.then_label, if_term.else_label
                    )
                }
                BlockTerm::BranchTable(branch) => format!(
                    "{}: branch {:?} default {}",
                    block.label, branch.targets, branch.default_label
                ),
                BlockTerm::Raise(_) => format!("{}: raise", block.label),
                BlockTerm::Return(_) => format!("{}: return", block.label),
            })
            .collect::<Vec<_>>();

        fn assert_target_present(
            labels: &HashSet<BlockLabel>,
            block_summaries: &[String],
            source: BlockLabel,
            target: BlockLabel,
            kind: &str,
        ) {
            let mut sorted_labels = labels.iter().copied().collect::<Vec<_>>();
            sorted_labels.sort();
            assert!(
                target.is_fallthrough() || labels.contains(&target),
                "inline fragment block {} has {} target {} outside fragment; labels={:?}; blocks={:?}",
                source,
                kind,
                target,
                sorted_labels,
                block_summaries
            );
        }

        for block in std::iter::once(&self.entry).chain(self.deps.iter()) {
            match &block.term {
                BlockTerm::Jump(edge) => {
                    assert_target_present(
                        &labels,
                        &block_summaries,
                        block.label,
                        edge.target,
                        "jump",
                    );
                }
                BlockTerm::IfTerm(if_term) => {
                    assert_target_present(
                        &labels,
                        &block_summaries,
                        block.label,
                        if_term.then_label,
                        "then",
                    );
                    assert_target_present(
                        &labels,
                        &block_summaries,
                        block.label,
                        if_term.else_label,
                        "else",
                    );
                }
                BlockTerm::BranchTable(branch) => {
                    for target in &branch.targets {
                        assert_target_present(
                            &labels,
                            &block_summaries,
                            block.label,
                            *target,
                            "branch",
                        );
                    }
                    assert_target_present(
                        &labels,
                        &block_summaries,
                        block.label,
                        branch.default_label,
                        "branch default",
                    );
                }
                BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
            }
            if let Some(edge) = &block.exc_edge {
                assert_target_present(
                    &labels,
                    &block_summaries,
                    block.label,
                    edge.target,
                    "exception",
                );
            }
        }
    }

    pub(crate) fn entry_ref(&self) -> InlineBlockRef {
        InlineBlockRef::from_label(self.entry.label)
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct LoweredExpr<S: Instr, V = S> {
    pub setup: InlineFragment<S>,
    pub value: V,
}

pub(crate) fn rewrite_ast_to_core_blockpy_module_with_module(
    context: &Context,
    module: Vec<Stmt>,
    semantic_state: &crate::passes::ast_to_ast::semantic::SemanticAstState,
    module_name_gen: crate::block_py::ModuleNameGen,
) -> BlockPyModule<CoreModuleShapeWithAwaitAndYield> {
    rewrite_ast_to_core_blockpy_module_plan_with_module(
        context,
        module,
        semantic_state,
        module_name_gen,
    )
}

#[cfg(test)]
pub(crate) fn test_name_gen() -> FunctionNameGen {
    crate::block_py::ModuleNameGen::new(0).next_function_name_gen()
}

#[derive(Clone)]
pub(crate) enum StmtSequenceHeadPlan {
    Linear(InstrRuff),
    Expanded(Vec<InstrRuff>),
    FunctionDef(crate::block_py::StmtFunctionDef<InstrRuff>),
    Raise(crate::block_py::StmtRaise<InstrRuff>),
    Return(InstrRuff),
    If(crate::block_py::StmtIf<InstrRuff>),
    While(crate::block_py::StmtWhile<InstrRuff>),
    For(crate::block_py::StmtFor<InstrRuff>),
    Try(crate::block_py::StmtTry<InstrRuff>),
    With(crate::block_py::StmtWith<InstrRuff>),
    Break,
    Continue,
    Unsupported,
}

pub(crate) fn attach_exception_edges_to_blocks<E: Instr>(
    blocks: Vec<Block<E>>,
    exception_edges: &HashMap<BlockLabel, Option<BlockLabel>>,
) -> Vec<Block<E>> {
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
            extra: block.extra,
        })
        .collect()
}

fn move_entry_block_to_front<I: Instr>(blocks: &mut Vec<Block<I>>, entry_label: BlockLabel) {
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
    runtime_input_body: &[InstrRuff],
    doc: Option<String>,
    end_label: BlockLabel,
    blockpy_kind: FunctionKind,
    scope: &CallableScopeInfo,
) -> BlockPyFunction<CoreModuleShapeWithAwaitAndYield> {
    let function_id = name_gen.function_id();
    let execution_mode = default_execution_mode_for_function(&names);
    let mut blocks = Vec::new();
    let entry_label = lower_stmt_sequence_with_state::<crate::block_py::InstrWithAwaitAndYield>(
        context,
        runtime_input_body,
        RegionTargets::new(end_label.clone(), None),
        &mut blocks,
        &name_gen,
    );
    move_entry_block_to_front(&mut blocks, entry_label.clone());
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
            extra: Default::default(),
        });
    }
    fold_jumps_to_trivial_return_blockpy(&mut blocks);
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
        execution_mode,
        params,
        blocks,
        doc,
        storage_layout: None,
        scope: scope.clone(),
    }
}

fn default_execution_mode_for_function(names: &FunctionName) -> FunctionExecutionMode {
    if names.bind_name == "_dp_module_init"
        || names.bind_name.starts_with("_dp_class_ns_")
        || names.bind_name.starts_with("_dp_define_class_")
        || is_annotation_helper_or_descendant(names)
    {
        FunctionExecutionMode::Interpreted
    } else {
        FunctionExecutionMode::Jit
    }
}

fn is_annotation_helper_or_descendant(names: &FunctionName) -> bool {
    names.bind_name == "__annotate__"
        || names.bind_name == "__annotate_func__"
        || names.bind_name.starts_with(FUNCTION_ANNOTATE_PREFIX)
        || names.qualname == "__annotate__"
        || names.qualname.starts_with("__annotate__.<locals>.")
        || names.qualname.starts_with(FUNCTION_ANNOTATE_PREFIX)
        || names.qualname.contains(".__annotate_func__")
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
