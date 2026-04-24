use super::{
    Block, BlockArg, BlockEdge, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule,
    BlockTerm, FunctionKind, Instr, ModuleShape, TermIf, TermRaise,
};
use crate::block_py::{ParamKind, ParamSpec};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum IfBranchKind {
    Then,
    Else,
}

pub trait BlockPyFormat: ModuleShape {
    fn block_metadata_lines(block: &Block<Self::Instr, Self::BlockExtra>) -> Vec<String>
    where
        Self: Sized,
    {
        render_blockpy_block_metadata(block)
    }
}

pub trait PrettyPrint {
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result;

    fn pretty_print_with_config(&self, config: PrettyConfig) -> String {
        let mut out = String::new();
        let mut printer = PrettyPrinter::new(&mut out, config);
        self.fmt_pretty(&mut printer)
            .expect("writing pretty-printed text to a String should not fail");
        out
    }

    fn pretty_print(&self) -> String {
        self.pretty_print_with_config(PrettyConfig::default())
    }

    fn debug_pretty_print(&self) -> String {
        self.pretty_print_with_config(PrettyConfig::debug())
    }
}

impl<T> PrettyPrint for Box<T>
where
    T: PrettyPrint + ?Sized,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        self.as_ref().fmt_pretty(printer)
    }
}

impl<T> PrettyPrint for Vec<T>
where
    T: PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        printer.write_char('[')?;
        let mut first = true;
        for item in self {
            if !first {
                printer.write_str(", ")?;
            }
            first = false;
            item.fmt_pretty(printer)?;
        }
        printer.write_char(']')
    }
}

impl<T> PrettyPrint for Option<T>
where
    T: PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        match self {
            Some(value) => {
                printer.write_str("Some(")?;
                value.fmt_pretty(printer)?;
                printer.write_char(')')
            }
            None => printer.write_str("None"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PrettyConfig {
    pub mode: PrettyMode,
}

impl PrettyConfig {
    pub fn debug() -> Self {
        Self {
            mode: PrettyMode::Debug,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PrettyMode {
    #[default]
    Normal,
    Debug,
}

pub struct PrettyPrinter<'a> {
    out: &'a mut dyn fmt::Write,
    config: PrettyConfig,
}

impl<'a> PrettyPrinter<'a> {
    pub fn new(out: &'a mut dyn fmt::Write, config: PrettyConfig) -> Self {
        Self { out, config }
    }

    pub fn config(&self) -> PrettyConfig {
        self.config
    }

    pub fn mode(&self) -> PrettyMode {
        self.config.mode
    }
}

impl fmt::Write for PrettyPrinter<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.out.write_str(s)
    }
}

impl<P> PrettyPrint for BlockPyModule<P>
where
    P: BlockPyFormat,
    P::Instr: PrettyPrint,
{
    fn fmt_pretty(&self, printer: &mut PrettyPrinter<'_>) -> fmt::Result {
        printer.write_str(&blockpy_module_to_string_with_config(
            self,
            printer.config(),
        ))
    }
}

pub fn blockpy_module_to_string<P>(module: &BlockPyModule<P>) -> String
where
    P: BlockPyFormat,
    P::Instr: PrettyPrint,
{
    blockpy_module_to_string_with_config(module, PrettyConfig::default())
}

pub fn blockpy_module_to_string_with_config<P>(
    module: &BlockPyModule<P>,
    config: PrettyConfig,
) -> String
where
    P: BlockPyFormat,
    P::Instr: PrettyPrint,
{
    let mut formatter = BlockPyFormatter::new(config);
    formatter.write_module(module);
    formatter.finish()
}

struct BlockPyFormatter {
    out: String,
    indent: usize,
    config: PrettyConfig,
}

impl BlockPyFormatter {
    fn new(config: PrettyConfig) -> Self {
        Self {
            out: String::new(),
            indent: 0,
            config,
        }
    }

    fn finish(mut self) -> String {
        if self.out.is_empty() {
            self.line("; empty BlockPy module");
        }
        self.out
    }

    fn write_module<P>(&mut self, module: &BlockPyModule<P>)
    where
        P: BlockPyFormat,
        P::Instr: PrettyPrint,
    {
        for function in &module.callable_defs {
            if !self.out.is_empty() {
                self.out.push('\n');
            }
            self.write_function(function);
        }
    }

    fn write_function<P>(&mut self, function: &BlockPyFunction<P>)
    where
        P: BlockPyFormat,
        P::Instr: PrettyPrint,
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

    fn write_function_block<P>(
        &mut self,
        function: &BlockPyFunction<P>,
        render_layout: &BlockRenderLayout,
        block_index: usize,
        referenced_labels: &HashSet<BlockLabel>,
    ) where
        P: BlockPyFormat,
        P::Instr: PrettyPrint,
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

    fn write_block_contents<P>(
        &mut self,
        function: &BlockPyFunction<P>,
        render_layout: &BlockRenderLayout,
        current_block_index: Option<usize>,
        block: &Block<P::Instr, P::BlockExtra>,
        referenced_labels: &HashSet<BlockLabel>,
    ) where
        P: BlockPyFormat,
        P::Instr: PrettyPrint,
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
        S: PrettyPrint,
    {
        for stmt in stmts {
            self.write_linear_stmt(stmt, referenced_labels);
        }
    }

    fn write_linear_stmt<S>(&mut self, stmt: &S, _referenced_labels: &HashSet<BlockLabel>)
    where
        S: PrettyPrint,
    {
        self.line(stmt.pretty_print_with_config(self.config));
    }

    fn write_term<P>(
        &mut self,
        function: &BlockPyFunction<P>,
        render_layout: &BlockRenderLayout,
        current_block_index: Option<usize>,
        term: &BlockTerm<P::Instr>,
        referenced_labels: &HashSet<BlockLabel>,
    ) where
        P: BlockPyFormat,
        P::Instr: PrettyPrint,
    {
        match term {
            BlockTerm::Jump(edge) => self.line(format!("jump {}", render_edge(edge))),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label,
                else_label,
            }) => {
                self.line(format!(
                    "if_term {}:",
                    render_inline_expr(test, self.config)
                ));
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
                render_inline_expr(&branch.index, self.config),
                join_labels(&branch.targets),
                branch.default_label,
            )),
            BlockTerm::Raise(raise_stmt) => self.write_raise(raise_stmt),
            BlockTerm::Return(value) => {
                self.line(format!("return {}", render_inline_expr(value, self.config)))
            }
        }
    }

    fn write_raise<E>(&mut self, raise_stmt: &TermRaise<E>)
    where
        E: Instr,
        E: PrettyPrint,
    {
        match &raise_stmt.exc {
            Some(exc) => self.line(format!("raise {}", render_inline_expr(exc, self.config))),
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
        crate::block_py::ClosureInit::EmptyCell => "empty_cell",
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
pub fn bb_expr_text<N: PrettyPrint>(expr: &N) -> String {
    expr.pretty_print()
}

fn render_inline_expr<N: PrettyPrint>(expr: &N, config: PrettyConfig) -> String {
    expr.pretty_print_with_config(config)
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

fn render_blockpy_block_metadata<I: Instr, E>(block: &Block<I, E>) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(exc_param) = block.exception_param() {
        lines.push(format!("exc_param: {exc_param}"));
    }
    lines
}

fn render_block_header<I: Instr, E>(block: &Block<I, E>) -> String {
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
        P: BlockPyFormat,
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
    P: BlockPyFormat,
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
    P: BlockPyFormat,
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
    block: &Block<P::Instr, P::BlockExtra>,
    label_to_index: &HashMap<BlockLabel, usize>,
) -> Vec<usize>
where
    P: ModuleShape,
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
    blocks: &[Block<P::Instr, P::BlockExtra>],
) -> HashSet<BlockLabel>
where
    P: ModuleShape,
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
