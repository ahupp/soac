#[cfg(test)]
use super::StructuredInstr;
use super::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule,
    BlockPyPass, BlockTerm, FunctionKind, Instr, TermIf, TermRaise,
};
use crate::block_py::param_specs::{ParamKind, ParamSpec};
use crate::passes::{
    CodegenBlockPyPass, CoreBlockPyPass, CoreBlockPyPassWithAwaitAndYield,
    CoreBlockPyPassWithYield, ResolvedStorageBlockPyPass,
};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::marker::PhantomData;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum IfBranchKind {
    Then,
    Else,
}

pub(crate) trait BlockPyPrettyPrinter: BlockPyPass {
    fn block_metadata_lines<S>(block: &Block<S, Self::Expr>) -> Vec<String>
    where
        Self: Sized;
}

macro_rules! impl_default_blockpy_pretty_printer {
    ($($pass:ty),* $(,)?) => {
        $(
            impl BlockPyPrettyPrinter for $pass {
                fn block_metadata_lines<S>(block: &Block<S, Self::Expr>) -> Vec<String> {
                    render_blockpy_block_metadata(block)
                }
            }
        )*
    };
}

impl_default_blockpy_pretty_printer!(
    CoreBlockPyPassWithAwaitAndYield,
    CoreBlockPyPassWithYield,
    CoreBlockPyPass,
);

impl BlockPyPrettyPrinter for ResolvedStorageBlockPyPass {
    fn block_metadata_lines<S>(block: &Block<S, Self::Expr>) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(exc_edge) = &block.exc_edge {
            lines.push(format!("exc_target: {}", exc_edge.target));
        }
        if let Some(exc_name) = block.exception_param() {
            lines.push(format!("exc_name: {exc_name}"));
        }
        lines
    }
}

impl BlockPyPrettyPrinter for CodegenBlockPyPass {
    fn block_metadata_lines<S>(block: &Block<S, Self::Expr>) -> Vec<String> {
        render_resolved_storage_block_metadata::<Self, S>(block)
    }
}

fn render_resolved_storage_block_metadata<P, S>(block: &Block<S, P::Expr>) -> Vec<String>
where
    P: BlockPyPass,
    P::Expr: Instr<Name = super::ResolvedName>,
{
    let mut lines = Vec::new();
    if !block.params.is_empty() {
        lines.push(format!(
            "params: [{}]",
            block
                .param_names()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if let Some(exc_edge) = &block.exc_edge {
        lines.push(format!("exc_target: {}", exc_edge.target));
    }
    if let Some(exc_name) = block.exception_param() {
        lines.push(format!("exc_name: {exc_name}"));
    }
    lines
}

pub(crate) trait BlockPyPrettyPrint {
    fn pretty_print(&self) -> String;

    fn debug_pretty_print(&self) -> String {
        self.pretty_print()
    }
}

impl<P, S> BlockPyPrettyPrint for BlockPyModule<P, S>
where
    P: BlockPyPrettyPrinter<Expr = S>,
    S: fmt::Debug + Instr,
    P::Expr: fmt::Debug,
{
    fn pretty_print(&self) -> String {
        blockpy_module_to_string(self)
    }

    fn debug_pretty_print(&self) -> String {
        blockpy_module_to_string(self)
    }
}

pub(crate) fn blockpy_module_to_string<P, S>(module: &BlockPyModule<P, S>) -> String
where
    P: BlockPyPrettyPrinter<Expr = S>,
    S: fmt::Debug + Instr,
    P::Expr: fmt::Debug,
{
    let mut formatter = BlockPyFormatter::<DebugInlineExprRenderer>::default();
    formatter.write_module(module);
    formatter.finish()
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn blockpy_module_to_debug_string<P, S>(module: &BlockPyModule<P, S>) -> String
where
    P: BlockPyPrettyPrinter<Expr = S>,
    S: fmt::Debug + Instr,
    P::Expr: fmt::Debug,
{
    blockpy_module_to_string(module)
}

trait InlineExprRenderer<E> {
    fn render(expr: &E) -> String;
}

struct DebugInlineExprRenderer;

impl<E> InlineExprRenderer<E> for DebugInlineExprRenderer
where
    E: fmt::Debug,
{
    fn render(expr: &E) -> String {
        format!("{expr:?}")
    }
}

struct BlockPyFormatter<R> {
    out: String,
    indent: usize,
    _renderer: PhantomData<R>,
}

impl<R> Default for BlockPyFormatter<R> {
    fn default() -> Self {
        Self {
            out: String::new(),
            indent: 0,
            _renderer: PhantomData,
        }
    }
}

impl<R> BlockPyFormatter<R> {
    fn finish(mut self) -> String {
        if self.out.is_empty() {
            self.line("; empty BlockPy module");
        }
        self.out
    }

    fn write_module<P, S>(&mut self, module: &BlockPyModule<P, S>)
    where
        P: BlockPyPrettyPrinter<Expr = S>,
        S: fmt::Debug + Instr,
        R: InlineExprRenderer<P::Expr>,
    {
        for function in &module.callable_defs {
            if !self.out.is_empty() {
                self.out.push('\n');
            }
            self.write_function(function);
        }
    }

    fn write_function<P, S>(&mut self, function: &BlockPyFunction<P, S>)
    where
        P: BlockPyPrettyPrinter<Expr = S>,
        S: fmt::Debug + Instr,
        R: InlineExprRenderer<P::Expr>,
    {
        let params = format_parameters(&function.params);
        let referenced_labels = collect_referenced_labels_from_blocks::<P>(&function.blocks);
        let render_layout = BlockRenderLayout::new(function);
        self.line(format!(
            "{} {}({params}):",
            function_kind_name(function.kind),
            function.names.qualname
        ));
        self.with_indent(|this| {
            this.line(format!("function_id: {}", function.function_id));
            if function.names.display_name != function.names.bind_name {
                this.line(format!("display_name: {}", function.names.display_name));
            }
            if let Some(layout) = &function.storage_layout {
                if !layout.freevars.is_empty() {
                    this.line(format!(
                        "freevars: [{}]",
                        render_closure_slots(&layout.freevars)
                    ));
                }
                if !layout.cellvars.is_empty() {
                    this.line(format!(
                        "cellvars: [{}]",
                        render_closure_slots(&layout.cellvars)
                    ));
                }
                if !layout.runtime_cells.is_empty() {
                    this.line(format!(
                        "runtime_cells: [{}]",
                        render_closure_slots(&layout.runtime_cells)
                    ));
                }
            }
            if function.blocks.is_empty() {
                this.line("pass");
            } else {
                for root_block in &render_layout.root_blocks {
                    this.write_function_block(
                        function,
                        &render_layout,
                        *root_block,
                        &referenced_labels,
                    );
                }
            }
        });
    }

    fn write_function_block<P, S>(
        &mut self,
        function: &BlockPyFunction<P, S>,
        render_layout: &BlockRenderLayout,
        block_index: usize,
        referenced_labels: &HashSet<BlockLabel>,
    ) where
        P: BlockPyPrettyPrinter<Expr = S>,
        S: fmt::Debug + Instr,
        R: InlineExprRenderer<P::Expr>,
    {
        let block = &function.blocks[block_index];
        self.line(render_block_header(block));
        self.with_indent(|this| {
            for line in P::block_metadata_lines(block) {
                this.line(line);
            }
            this.write_block_contents(
                function,
                render_layout,
                Some(block_index),
                block,
                referenced_labels,
            );
            for child_block in &render_layout.child_blocks[block_index] {
                if render_layout.inlined_blocks.contains(child_block) {
                    continue;
                }
                this.write_function_block(function, render_layout, *child_block, referenced_labels);
            }
        });
    }

    fn write_block_contents<P, S>(
        &mut self,
        function: &BlockPyFunction<P, S>,
        render_layout: &BlockRenderLayout,
        current_block_index: Option<usize>,
        block: &Block<S, P::Expr>,
        referenced_labels: &HashSet<BlockLabel>,
    ) where
        P: BlockPyPrettyPrinter<Expr = S>,
        S: fmt::Debug + Instr,
        R: InlineExprRenderer<P::Expr>,
    {
        if block.body.is_empty() {
            self.write_term(
                function,
                render_layout,
                current_block_index,
                &block.term,
                referenced_labels,
            );
            return;
        }
        self.write_linear_stmt_list(&block.body, referenced_labels);
        self.write_term(
            function,
            render_layout,
            current_block_index,
            &block.term,
            referenced_labels,
        );
    }

    fn write_linear_stmt_list<S>(&mut self, stmts: &[S], referenced_labels: &HashSet<BlockLabel>)
    where
        S: std::fmt::Debug,
    {
        for stmt in stmts {
            self.write_linear_stmt(stmt, referenced_labels);
        }
    }

    fn write_linear_stmt<S>(&mut self, stmt: &S, _referenced_labels: &HashSet<BlockLabel>)
    where
        S: std::fmt::Debug,
    {
        self.line(format!("{stmt:?}"));
    }

    fn write_term<P>(
        &mut self,
        function: &BlockPyFunction<P>,
        render_layout: &BlockRenderLayout,
        current_block_index: Option<usize>,
        term: &BlockTerm<P::Expr>,
        referenced_labels: &HashSet<BlockLabel>,
    ) where
        P: BlockPyPrettyPrinter,
        R: InlineExprRenderer<P::Expr>,
    {
        match term {
            BlockTerm::Jump(edge) => self.line(format!("jump {}", render_edge(edge))),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label,
                else_label,
            }) => {
                self.line(format!("if_term {}:", R::render(test)));
                self.with_indent(|this| {
                    this.line("then:");
                    this.with_indent(|this| {
                        if let Some(target_index) = current_block_index.and_then(|block_index| {
                            render_layout
                                .inline_if_term_targets
                                .get(&(block_index, IfBranchKind::Then))
                                .copied()
                        }) {
                            this.write_function_block(
                                function,
                                render_layout,
                                target_index,
                                referenced_labels,
                            );
                        } else {
                            this.line(format!("jump {}", then_label));
                        }
                    });
                    this.line("else:");
                    this.with_indent(|this| {
                        if let Some(target_index) = current_block_index.and_then(|block_index| {
                            render_layout
                                .inline_if_term_targets
                                .get(&(block_index, IfBranchKind::Else))
                                .copied()
                        }) {
                            this.write_function_block(
                                function,
                                render_layout,
                                target_index,
                                referenced_labels,
                            );
                        } else {
                            this.line(format!("jump {}", else_label));
                        }
                    });
                });
            }
            BlockTerm::BranchTable(branch) => self.line(format!(
                "branch_table {} -> [{}] default {}",
                R::render(&branch.index),
                join_labels(&branch.targets),
                branch.default_label,
            )),
            BlockTerm::Raise(raise_stmt) => self.write_raise(raise_stmt),
            BlockTerm::Return(value) => self.line(format!("return {}", R::render(value))),
        }
    }

    fn write_raise<E>(&mut self, raise_stmt: &TermRaise<E>)
    where
        E: Instr,
        R: InlineExprRenderer<E>,
    {
        match &raise_stmt.exc {
            Some(exc) => self.line(format!("raise {}", R::render(exc))),
            None => self.line("raise"),
        }
    }
    fn with_indent(&mut self, f: impl FnOnce(&mut Self)) {
        self.indent += 1;
        f(self);
        self.indent -= 1;
    }

    fn line(&mut self, line: impl AsRef<str>) {
        for _ in 0..self.indent {
            self.out.push_str("    ");
        }
        self.out.push_str(line.as_ref());
        self.out.push('\n');
    }
}

fn render_closure_slots(slots: &[crate::block_py::ClosureSlot]) -> String {
    slots
        .iter()
        .map(|slot| {
            format!(
                "{}->{}@{}",
                slot.logical_name,
                slot.storage_name,
                closure_init_name(&slot.init),
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn closure_init_name(init: &crate::block_py::ClosureInit) -> &'static str {
    match init {
        crate::block_py::ClosureInit::InheritedCapture => "inherited",
        crate::block_py::ClosureInit::Parameter => "param",
        crate::block_py::ClosureInit::DeletedSentinel => "deleted",
        crate::block_py::ClosureInit::RuntimePcUnstarted => "pc_unstarted",
        crate::block_py::ClosureInit::RuntimeAbruptKindFallthrough => "abrupt_kind_fallthrough",
        crate::block_py::ClosureInit::RuntimeNone => "none",
        crate::block_py::ClosureInit::Deferred => "deferred",
    }
}

fn function_kind_name(kind: FunctionKind) -> &'static str {
    match kind {
        FunctionKind::Function => "function",
        FunctionKind::Coroutine => "coroutine",
        FunctionKind::Generator => "generator",
        FunctionKind::AsyncGenerator => "async_generator",
    }
}

#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn bb_expr_text<N: fmt::Debug>(expr: &N) -> String {
    format!("{expr:?}")
}

#[cfg(test)]
pub(crate) fn core_bb_stmt_text<N: crate::block_py::BlockPyNameLike>(
    stmt: &crate::block_py::InstrLow<N>,
) -> String {
    bb_expr_text(stmt)
}

#[cfg(test)]
pub(crate) fn bb_stmt_text<E>(stmt: &E) -> String
where
    E: fmt::Debug + Instr,
{
    format!("{stmt:?}")
}

fn format_parameters(parameters: &ParamSpec) -> String {
    let mut parts = Vec::new();
    let mut saw_kw_separator = false;

    for (index, param) in parameters.params.iter().enumerate() {
        if index > 0
            && parameters.params[index - 1].kind == ParamKind::PosOnly
            && param.kind != ParamKind::PosOnly
        {
            parts.push("/".to_string());
        }
        if !saw_kw_separator
            && param.kind == ParamKind::KwOnly
            && !parameters.params[..index]
                .iter()
                .any(|existing| existing.kind == ParamKind::VarArg)
        {
            parts.push("*".to_string());
            saw_kw_separator = true;
        }

        let rendered_name = match param.kind {
            ParamKind::VarArg => format!("*{}", param.name),
            ParamKind::KwArg => format!("**{}", param.name),
            _ => param.name.clone(),
        };
        parts.push(rendered_name);
    }
    parts.join(", ")
}

fn join_labels(labels: &[BlockLabel]) -> String {
    labels
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_edge(edge: &BlockEdge) -> String {
    if edge.args.is_empty() {
        return edge.target.to_string();
    }
    format!(
        "{}({})",
        edge.target,
        edge.args
            .iter()
            .map(render_block_arg)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn render_block_arg(arg: &BlockArg) -> String {
    format!("{arg:?}")
}

fn render_blockpy_block_metadata<S, T: Instr>(block: &Block<S, T>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(exc_param) = block.exception_param() {
        lines.push(format!("exc_param: {exc_param}"));
    }
    lines
}

fn render_block_header<S, T: Instr>(block: &Block<S, T>) -> String {
    let params = block
        .params
        .iter()
        .map(|param| format!("{}: {}", param.name, render_block_param_role(param.role)))
        .collect::<Vec<_>>();
    if params.is_empty() {
        format!("block {}:", block.label)
    } else {
        format!("block {}({}):", block.label, params.join(", "))
    }
}

fn render_block_param_role(role: BlockParamRole) -> String {
    format!("{role:?}")
}

#[derive(Debug)]
struct BlockRenderLayout {
    root_blocks: Vec<usize>,
    child_blocks: Vec<Vec<usize>>,
    inline_if_term_targets: HashMap<(usize, IfBranchKind), usize>,
    inlined_blocks: HashSet<usize>,
}

impl BlockRenderLayout {
    fn new<P>(function: &BlockPyFunction<P>) -> Self
    where
        P: BlockPyPrettyPrinter,
    {
        let block_count = function.blocks.len();
        if block_count == 0 {
            return Self {
                root_blocks: Vec::new(),
                child_blocks: Vec::new(),
                inline_if_term_targets: HashMap::new(),
                inlined_blocks: HashSet::new(),
            };
        }

        let label_to_index = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block.label, index))
            .collect::<HashMap<_, _>>();

        let successors = function
            .blocks
            .iter()
            .map(|block| collect_top_level_successors_from_block::<P>(block, &label_to_index))
            .collect::<Vec<_>>();
        let predecessors = collect_predecessors(&successors);
        let entry_index = 0;
        let discovery_order = collect_discovery_order(entry_index, &successors);
        let reachable = discovery_order.iter().copied().collect::<HashSet<_>>();
        let dominators =
            compute_dominators(entry_index, &discovery_order, &predecessors, &reachable);
        let immediate_dominators =
            compute_immediate_dominators(entry_index, &discovery_order, &dominators, &reachable);

        let mut child_blocks = vec![Vec::new(); block_count];
        for (block_index, immediate_dominator) in immediate_dominators.iter().enumerate() {
            if let Some(parent_index) = immediate_dominator {
                child_blocks[*parent_index].push(block_index);
            }
        }
        for children in &mut child_blocks {
            sort_block_indices_by_label(children, function);
        }

        let (inline_if_term_targets, inlined_blocks) = compute_inline_if_term_targets(
            function,
            &label_to_index,
            &predecessors,
            &immediate_dominators,
        );

        let mut root_blocks = vec![entry_index];
        let reachable_roots = discovery_order
            .iter()
            .copied()
            .filter(|index| *index != entry_index && immediate_dominators[*index].is_none())
            .collect::<Vec<_>>();
        root_blocks.extend(reachable_roots);
        root_blocks.extend((0..block_count).filter(|index| !reachable.contains(index)));
        sort_block_indices_by_label(&mut root_blocks[1..], function);

        Self {
            root_blocks,
            child_blocks,
            inline_if_term_targets,
            inlined_blocks,
        }
    }
}

fn sort_block_indices_by_label<P>(indices: &mut [usize], function: &BlockPyFunction<P>)
where
    P: BlockPyPrettyPrinter,
{
    indices.sort_by_key(|index| function.blocks[*index].label);
}

fn compute_inline_if_term_targets<P>(
    function: &BlockPyFunction<P>,
    label_to_index: &HashMap<BlockLabel, usize>,
    predecessors: &[Vec<usize>],
    immediate_dominators: &[Option<usize>],
) -> (HashMap<(usize, IfBranchKind), usize>, HashSet<usize>)
where
    P: BlockPyPrettyPrinter,
{
    let mut targets = HashMap::new();
    let mut inlined_blocks = HashSet::new();

    for (block_index, block) in function.blocks.iter().enumerate() {
        let BlockTerm::IfTerm(TermIf {
            then_label,
            else_label,
            ..
        }) = &block.term
        else {
            continue;
        };

        let then_target = label_to_index.get(then_label).copied();
        let else_target = label_to_index.get(else_label).copied();

        if let Some(target_index) = then_target {
            if can_inline_if_term_target(
                block_index,
                target_index,
                else_target,
                predecessors,
                immediate_dominators,
            ) {
                targets.insert((block_index, IfBranchKind::Then), target_index);
                inlined_blocks.insert(target_index);
            }
        }

        if let Some(target_index) = else_target {
            if can_inline_if_term_target(
                block_index,
                target_index,
                then_target,
                predecessors,
                immediate_dominators,
            ) {
                targets.insert((block_index, IfBranchKind::Else), target_index);
                inlined_blocks.insert(target_index);
            }
        }
    }

    (targets, inlined_blocks)
}
fn can_inline_if_term_target(
    parent_index: usize,
    target_index: usize,
    sibling_target: Option<usize>,
    predecessors: &[Vec<usize>],
    immediate_dominators: &[Option<usize>],
) -> bool {
    if sibling_target == Some(target_index) {
        return false;
    }
    immediate_dominators[target_index] == Some(parent_index)
        && predecessors[target_index].len() == 1
        && predecessors[target_index][0] == parent_index
}

fn collect_top_level_successors_from_block<P>(
    block: &Block<P::Expr, P::Expr>,
    label_to_index: &HashMap<BlockLabel, usize>,
) -> Vec<usize>
where
    P: BlockPyPass,
{
    let mut successors = Vec::new();
    let mut seen = HashSet::new();
    collect_top_level_successors_from_linear_stmts(
        &block.body,
        label_to_index,
        &mut seen,
        &mut successors,
    );
    collect_top_level_successors_from_term(&block.term, label_to_index, &mut seen, &mut successors);
    successors
}

fn collect_top_level_successors_from_linear_stmts<S>(
    stmts: &[S],
    label_to_index: &HashMap<BlockLabel, usize>,
    seen: &mut HashSet<usize>,
    out: &mut Vec<usize>,
) where
    S: Clone,
{
    let _ = stmts;
    let _ = label_to_index;
    let _ = seen;
    let _ = out;
}

fn collect_top_level_successors_from_term(
    term: &BlockTerm<impl Clone + Instr>,
    label_to_index: &HashMap<BlockLabel, usize>,
    seen: &mut HashSet<usize>,
    out: &mut Vec<usize>,
) {
    match term {
        BlockTerm::Jump(label) => {
            push_top_level_successor(&label.target, label_to_index, seen, out);
        }
        BlockTerm::IfTerm(TermIf {
            then_label,
            else_label,
            ..
        }) => {
            push_top_level_successor(then_label, label_to_index, seen, out);
            push_top_level_successor(else_label, label_to_index, seen, out);
        }
        BlockTerm::BranchTable(branch) => {
            for label in &branch.targets {
                push_top_level_successor(label, label_to_index, seen, out);
            }
            push_top_level_successor(&branch.default_label, label_to_index, seen, out);
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
    }
}

fn push_top_level_successor(
    label: &BlockLabel,
    label_to_index: &HashMap<BlockLabel, usize>,
    seen: &mut HashSet<usize>,
    out: &mut Vec<usize>,
) {
    let Some(successor_index) = label_to_index.get(label) else {
        return;
    };
    if seen.insert(*successor_index) {
        out.push(*successor_index);
    }
}

fn collect_predecessors(successors: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let mut predecessors = vec![Vec::new(); successors.len()];
    for (source_index, targets) in successors.iter().enumerate() {
        for target_index in targets {
            predecessors[*target_index].push(source_index);
        }
    }
    predecessors
}

fn collect_discovery_order(entry_index: usize, successors: &[Vec<usize>]) -> Vec<usize> {
    fn visit(
        block_index: usize,
        successors: &[Vec<usize>],
        visited: &mut HashSet<usize>,
        order: &mut Vec<usize>,
    ) {
        if !visited.insert(block_index) {
            return;
        }
        order.push(block_index);
        for successor_index in &successors[block_index] {
            visit(*successor_index, successors, visited, order);
        }
    }

    let mut visited = HashSet::new();
    let mut order = Vec::new();
    visit(entry_index, successors, &mut visited, &mut order);
    order
}

fn compute_dominators(
    entry_index: usize,
    discovery_order: &[usize],
    predecessors: &[Vec<usize>],
    reachable: &HashSet<usize>,
) -> Vec<HashSet<usize>> {
    let mut dominators = vec![HashSet::new(); predecessors.len()];
    let all_reachable = reachable.iter().copied().collect::<HashSet<_>>();
    for block_index in discovery_order {
        if *block_index == entry_index {
            dominators[*block_index].insert(*block_index);
        } else {
            dominators[*block_index] = all_reachable.clone();
        }
    }

    loop {
        let mut changed = false;
        for block_index in discovery_order
            .iter()
            .copied()
            .filter(|block_index| *block_index != entry_index)
        {
            let mut reachable_predecessors = predecessors[block_index]
                .iter()
                .copied()
                .filter(|predecessor| reachable.contains(predecessor));
            let Some(first_predecessor) = reachable_predecessors.next() else {
                let mut singleton = HashSet::new();
                singleton.insert(block_index);
                if dominators[block_index] != singleton {
                    dominators[block_index] = singleton;
                    changed = true;
                }
                continue;
            };

            let mut new_dominators = dominators[first_predecessor].clone();
            for predecessor in reachable_predecessors {
                new_dominators = new_dominators
                    .intersection(&dominators[predecessor])
                    .copied()
                    .collect();
            }
            new_dominators.insert(block_index);

            if dominators[block_index] != new_dominators {
                dominators[block_index] = new_dominators;
                changed = true;
            }
        }

        if !changed {
            return dominators;
        }
    }
}

fn compute_immediate_dominators(
    entry_index: usize,
    discovery_order: &[usize],
    dominators: &[HashSet<usize>],
    reachable: &HashSet<usize>,
) -> Vec<Option<usize>> {
    let mut immediate_dominators = vec![None; dominators.len()];
    for block_index in discovery_order
        .iter()
        .copied()
        .filter(|block_index| *block_index != entry_index)
    {
        let strict_dominators = dominators[block_index]
            .iter()
            .copied()
            .filter(|dominator| *dominator != block_index && reachable.contains(dominator))
            .collect::<Vec<_>>();
        let immediate_dominator = strict_dominators.iter().copied().find(|candidate| {
            strict_dominators
                .iter()
                .all(|other| *other == *candidate || dominators[*candidate].contains(other))
        });
        immediate_dominators[block_index] = immediate_dominator;
    }
    immediate_dominators
}

fn collect_referenced_labels_from_blocks<P>(
    blocks: &[Block<P::Expr, P::Expr>],
) -> HashSet<BlockLabel>
where
    P: BlockPyPass,
{
    let mut referenced = HashSet::new();
    for block in blocks {
        if let Some(exc_edge) = &block.exc_edge {
            referenced.insert(exc_edge.target);
        }
        collect_referenced_labels_from_term(&block.term, &mut referenced);
    }
    referenced
}

#[cfg(test)]
fn collect_referenced_labels_from_structured_blocks<E>(
    blocks: &[Block<StructuredInstr<E>, E>],
) -> HashSet<BlockLabel>
where
    E: Clone + std::fmt::Debug + Instr,
{
    let mut referenced = HashSet::new();
    for block in blocks {
        if let Some(exc_edge) = &block.exc_edge {
            referenced.insert(exc_edge.target);
        }
        collect_referenced_labels_from_structured_stmts(&block.body, &mut referenced);
        collect_referenced_labels_from_term(&block.term, &mut referenced);
    }
    referenced
}

#[cfg(test)]
fn collect_referenced_labels_from_structured_stmts<E>(
    stmts: &[StructuredInstr<E>],
    out: &mut HashSet<BlockLabel>,
) where
    E: Clone + std::fmt::Debug + Instr,
{
    for stmt in stmts {
        if let StructuredInstr::If(if_stmt) = stmt {
            collect_referenced_labels_from_structured_stmts(&if_stmt.body.body, out);
            if let Some(term) = &if_stmt.body.term {
                collect_referenced_labels_from_term(term, out);
            }
            collect_referenced_labels_from_structured_stmts(&if_stmt.orelse.body, out);
            if let Some(term) = &if_stmt.orelse.term {
                collect_referenced_labels_from_term(term, out);
            }
        }
    }
}

fn collect_referenced_labels_from_term(
    term: &BlockTerm<impl Clone + Instr>,
    out: &mut HashSet<BlockLabel>,
) {
    match term {
        BlockTerm::Jump(edge) => {
            out.insert(edge.target);
        }
        BlockTerm::IfTerm(if_term) => {
            out.insert(if_term.then_label);
            out.insert(if_term.else_label);
        }
        BlockTerm::BranchTable(branch) => {
            for label in &branch.targets {
                out.insert(*label);
            }
            out.insert(branch.default_label);
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
    }
}

#[cfg(test)]
mod test;
