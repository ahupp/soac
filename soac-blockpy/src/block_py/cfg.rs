use super::{
    instr_any, Block, BlockBuilder, BlockLabel, BlockParam, BlockPyFunction, BlockPyNameLike,
    BlockPyPass, BlockTerm, ChildVisitable, Del, FunctionNameGen, HasMeta, ImplicitNoneExpr,
    Instr, Load, MapInstr, MapTerm, Mappable, Store, StructuredInstr, TermIf, UnresolvedName,
    WithMeta,
};
use crate::namegen::fresh_name;
use crate::py_expr;
use ruff_python_ast as ast;
use std::collections::{HashMap, HashSet};

fn blockpy_successors<E: Instr>(block: &Block<StructuredInstr<E>, E>) -> Vec<BlockLabel> {
    match &block.term {
        BlockTerm::Jump(target) => vec![target.target.clone()],
        BlockTerm::IfTerm(if_term) => {
            vec![if_term.then_label.clone(), if_term.else_label.clone()]
        }
        BlockTerm::BranchTable(branch) => {
            let mut out = branch.targets.clone();
            out.push(branch.default_label.clone());
            out
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => Vec::new(),
    }
}

fn params_for_linearized_names(
    param_names: &[String],
    declared_params: &[BlockParam],
) -> Vec<BlockParam> {
    declared_params
        .iter()
        .filter(|param| param_names.iter().any(|name| name == &param.name))
        .cloned()
        .collect()
}

fn linearize_blockpy_if_sequence<E>(
    name_gen: &FunctionNameGen,
    label: BlockLabel,
    body: Vec<StructuredInstr<E>>,
    final_term: BlockTerm<E>,
    exc_edge: Option<super::BlockEdge>,
    block_params: Vec<String>,
    declared_params: Vec<BlockParam>,
    exc_target: Option<BlockLabel>,
    out_blocks: &mut Vec<Block<StructuredInstr<E>, E>>,
    out_block_params: &mut HashMap<BlockLabel, Vec<String>>,
    out_exception_edges: &mut HashMap<BlockLabel, Option<BlockLabel>>,
) where
    E: Clone + Instr,
{
    let Some(if_index) = body
        .iter()
        .position(|stmt| matches!(stmt, StructuredInstr::If(_)))
    else {
        out_block_params.insert(label.clone(), block_params.clone());
        out_exception_edges.insert(label.clone(), exc_target);
        out_blocks.push(Block {
            label,
            body,
            term: final_term,
            params: params_for_linearized_names(&block_params, &declared_params),
            exc_edge,
        });
        return;
    };

    let mut body = body;
    let rest = body.split_off(if_index + 1);
    let if_stmt = match body.pop() {
        Some(StructuredInstr::If(if_stmt)) => if_stmt,
        _ => unreachable!("expected structured BlockPy if at split point"),
    };
    let then_label = name_gen.next_block_name();
    let else_label = name_gen.next_block_name();
    let join_label = if rest.is_empty() {
        None
    } else {
        Some(name_gen.next_block_name())
    };

    out_block_params.insert(label.clone(), block_params.clone());
    out_exception_edges.insert(label.clone(), exc_target.clone());
    out_blocks.push(Block {
        label: label.clone(),
        body,
        term: BlockTerm::IfTerm(TermIf {
            test: if_stmt.test.clone(),
            then_label: then_label.clone(),
            else_label: else_label.clone(),
        }),
        params: params_for_linearized_names(
            out_block_params.get(&label).unwrap(),
            &declared_params,
        ),
        exc_edge: exc_edge.clone(),
    });

    let branch_fallthrough = join_label
        .clone()
        .map(|next_label| BlockTerm::Jump(super::BlockEdge::new(next_label)))
        .unwrap_or_else(|| final_term.clone());
    linearize_blockpy_fragment(
        name_gen,
        then_label,
        if_stmt.body,
        branch_fallthrough.clone(),
        exc_edge.clone(),
        block_params.clone(),
        declared_params.clone(),
        exc_target.clone(),
        out_blocks,
        out_block_params,
        out_exception_edges,
    );
    linearize_blockpy_fragment(
        name_gen,
        else_label,
        if_stmt.orelse,
        branch_fallthrough,
        exc_edge.clone(),
        block_params.clone(),
        declared_params.clone(),
        exc_target.clone(),
        out_blocks,
        out_block_params,
        out_exception_edges,
    );

    if let Some(join_label) = join_label {
        linearize_blockpy_if_sequence(
            name_gen,
            join_label,
            rest,
            final_term,
            exc_edge,
            block_params,
            declared_params,
            exc_target,
            out_blocks,
            out_block_params,
            out_exception_edges,
        );
    }
}

fn linearize_blockpy_fragment<E>(
    name_gen: &FunctionNameGen,
    label: BlockLabel,
    fragment: BlockBuilder<StructuredInstr<E>, BlockTerm<E>>,
    fallthrough_term: BlockTerm<E>,
    exc_edge: Option<super::BlockEdge>,
    block_params: Vec<String>,
    declared_params: Vec<BlockParam>,
    exc_target: Option<BlockLabel>,
    out_blocks: &mut Vec<Block<StructuredInstr<E>, E>>,
    out_block_params: &mut HashMap<BlockLabel, Vec<String>>,
    out_exception_edges: &mut HashMap<BlockLabel, Option<BlockLabel>>,
) where
    E: Clone + Instr,
{
    linearize_blockpy_if_sequence(
        name_gen,
        label,
        fragment.body,
        fragment.term.unwrap_or(fallthrough_term),
        exc_edge,
        block_params,
        declared_params,
        exc_target,
        out_blocks,
        out_block_params,
        out_exception_edges,
    );
}

pub(crate) fn linearize_structured_ifs<E>(
    name_gen: &FunctionNameGen,
    blocks: &[Block<StructuredInstr<E>, E>],
    block_params: &HashMap<BlockLabel, Vec<String>>,
    exception_edges: &HashMap<BlockLabel, Option<BlockLabel>>,
) -> (
    Vec<Block<StructuredInstr<E>, E>>,
    HashMap<BlockLabel, Vec<String>>,
    HashMap<BlockLabel, Option<BlockLabel>>,
)
where
    E: Clone + Instr,
{
    let mut out_blocks = Vec::new();
    let mut out_block_params = HashMap::new();
    let mut out_exception_edges = HashMap::new();
    for block in blocks {
        let mut params = block_params.get(&block.label).cloned().unwrap_or_default();
        for name in block.bb_param_names() {
            if !params.iter().any(|existing| existing == name) {
                params.push(name.to_string());
            }
        }
        let exc_target = exception_edges.get(&block.label).cloned().unwrap_or(None);
        linearize_blockpy_if_sequence(
            name_gen,
            block.label.clone(),
            block.body.clone(),
            block.term.clone(),
            block.exc_edge.clone(),
            params,
            block.params.clone(),
            exc_target,
            &mut out_blocks,
            &mut out_block_params,
            &mut out_exception_edges,
        );
    }
    (out_blocks, out_block_params, out_exception_edges)
}

pub(crate) fn fold_jumps_to_trivial_none_return_blockpy<E>(
    blocks: &mut [Block<StructuredInstr<E>, E>],
) where
    E: Clone + ImplicitNoneExpr + Instr,
{
    let trivial_ret_none_terms: HashMap<BlockLabel, BlockTerm<E>> = blocks
        .iter()
        .filter(|block| {
            block.body.is_empty()
                && match &block.term {
                    BlockTerm::Return(expr) => E::is_implicit_none_expr(expr),
                    _ => false,
                }
        })
        .map(|block| (block.label.clone(), block.term.clone()))
        .collect();

    for block in blocks.iter_mut() {
        let jump_target = match &block.term {
            BlockTerm::Jump(target) => Some(target.target.clone()),
            _ => None,
        };
        if let Some(target) = jump_target {
            if let Some(term) = trivial_ret_none_terms.get(&target) {
                block.term = term.clone();
            }
        }
    }
}

pub(crate) fn prune_unreachable_blockpy_blocks<E: Instr>(
    entry_label: BlockLabel,
    extra_roots: &[BlockLabel],
    blocks: &mut Vec<Block<StructuredInstr<E>, E>>,
) {
    let index_by_label: HashMap<BlockLabel, usize> = blocks
        .iter()
        .enumerate()
        .map(|(idx, block)| (block.label.clone(), idx))
        .collect();

    let mut worklist = vec![entry_label];
    worklist.extend(extra_roots.iter().cloned());
    let mut reachable = HashSet::new();
    while let Some(label) = worklist.pop() {
        if !reachable.insert(label.clone()) {
            continue;
        }
        let Some(idx) = index_by_label.get(&label) else {
            continue;
        };
        for succ in blockpy_successors(&blocks[*idx]) {
            worklist.push(succ);
        }
    }

    blocks.retain(|block| reachable.contains(&block.label));
}

fn fresh_eval_name() -> ast::ExprName {
    let name = fresh_name("eval");
    let ast::Expr::Name(expr) = py_expr!("{name:id}", name = name.as_str()) else {
        unreachable!();
    };
    expr
}

fn typed_store_expr<E, N>(target: N, value: E) -> E
where
    E: Instr + From<Store<E>>,
    N: BlockPyNameLike + HasMeta + Into<<E as Instr>::Name>,
{
    let meta = target.meta();
    Store::<E>::new(target, value).with_meta(meta).into()
}

fn typed_del_expr<E, N>(target: N) -> E
where
    E: Instr<Name = UnresolvedName> + From<Del<E>>,
    N: HasMeta + Into<<E as Instr>::Name>,
{
    let meta = target.meta();
    let target = target.into();
    Del::<E>::new(target, false).with_meta(meta).into()
}

fn append_stmt_cleanup<E>(out: &mut Vec<E>, cleanup: Vec<ast::ExprName>)
where
    E: Instr<Name = UnresolvedName> + From<Del<E>>,
{
    for temp in cleanup.into_iter().rev() {
        out.push(typed_del_expr(temp));
    }
}

fn expr_contains_matching_subexpression<E, F>(expr: &E, should_hoist: &mut F) -> bool
where
    E: Instr + ChildVisitable<E>,
    F: FnMut(&E) -> bool,
{
    instr_any(expr, |value| should_hoist(value))
}

fn hoist_subexpression_if_matching<E, F>(
    expr: E,
    out: &mut Vec<E>,
    cleanup: &mut Vec<ast::ExprName>,
    should_hoist: &mut F,
) -> E
where
    E: Instr<Name = UnresolvedName>
        + ChildVisitable<E>
        + Mappable<E, Mapped<E> = E>
        + From<Load<E>>
        + From<Store<E>>
        + From<Del<E>>,
    F: FnMut(&E) -> bool,
{
    let expr = expr.map_same_children(&mut |value| {
        hoist_subexpression_if_matching(value, out, cleanup, should_hoist)
    });
    if should_hoist(&expr) {
        let target = fresh_eval_name();
        out.push(typed_store_expr(target.clone(), expr));
        cleanup.push(target.clone());
        let meta = target.meta();
        Load::new(target).with_meta(meta).into()
    } else {
        expr
    }
}

fn rewrite_matching_children_in_expr<E, F>(
    expr: E,
    out: &mut Vec<E>,
    cleanup: &mut Vec<ast::ExprName>,
    should_hoist: &mut F,
) -> E
where
    E: Instr<Name = UnresolvedName>
        + ChildVisitable<E>
        + Mappable<E, Mapped<E> = E>
        + From<Load<E>>
        + From<Store<E>>
        + From<Del<E>>,
    F: FnMut(&E) -> bool,
{
    expr.map_same_children(&mut |value| {
        hoist_subexpression_if_matching(value, out, cleanup, should_hoist)
    })
}

struct HoistMatchingSubexpressionsInTerm<'a, 'b, E, F> {
    out: &'a mut Vec<E>,
    cleanup: &'b mut Vec<ast::ExprName>,
    should_hoist: &'b mut F,
}

impl<E, F> MapInstr<E, E> for HoistMatchingSubexpressionsInTerm<'_, '_, E, F>
where
    E: Instr<Name = UnresolvedName>
        + ChildVisitable<E>
        + Mappable<E, Mapped<E> = E>
        + From<Load<E>>
        + From<Store<E>>
        + From<Del<E>>,
    F: FnMut(&E) -> bool,
{
    fn map_instr(&mut self, expr: E) -> E {
        hoist_subexpression_if_matching(expr, self.out, self.cleanup, self.should_hoist)
    }

    fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
        name
    }
}

fn hoist_matching_subexpressions_in_term<E, F>(
    term: BlockTerm<E>,
    out: &mut Vec<E>,
    should_hoist: &mut F,
) -> BlockTerm<E>
where
    E: Instr<Name = UnresolvedName>
        + ChildVisitable<E>
        + Mappable<E, Mapped<E> = E>
        + From<Load<E>>
        + From<Store<E>>
        + From<Del<E>>,
    F: FnMut(&E) -> bool,
{
    let mut cleanup = Vec::new();
    let mut map = HoistMatchingSubexpressionsInTerm {
        out,
        cleanup: &mut cleanup,
        should_hoist,
    };
    map.map_term(term)
}

pub(crate) fn hoist_matching_subexpressions_in_callable_def<P, E, F>(
    callable_def: BlockPyFunction<P>,
    mut should_hoist: F,
) -> BlockPyFunction<P>
where
    P: BlockPyPass<Expr = E>,
    E: Instr<Name = UnresolvedName>
        + ChildVisitable<E>
        + Mappable<E, Mapped<E> = E>
        + From<Load<E>>
        + From<Store<E>>
        + From<Del<E>>,
    F: FnMut(&E) -> bool,
{
    callable_def.map_blocks(|block| {
        let Block {
            label,
            body: input_body,
            term: input_term,
            params,
            exc_edge,
        } = block;
        let mut body = Vec::new();
        for expr in input_body {
            let mut setup = Vec::new();
            let mut cleanup = Vec::new();
            let expr =
                if expr_contains_matching_subexpression(&expr, &mut should_hoist) {
                    rewrite_matching_children_in_expr(
                        expr,
                        &mut setup,
                        &mut cleanup,
                        &mut should_hoist,
                    )
                } else {
                    expr
                };
            body.extend(setup);
            body.push(expr);
            append_stmt_cleanup(&mut body, cleanup);
        }
        let term = hoist_matching_subexpressions_in_term(input_term, &mut body, &mut should_hoist);
        Block {
            label,
            body,
            term,
            params,
            exc_edge,
        }
    })
}

pub(crate) fn relabel_blockpy_blocks_dense<S, T: Instr>(blocks: &mut [Block<S, T>])
where
    BlockTerm<T>: RelabelBlockTargets,
{
    let relabel = blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, BlockLabel::from_index(index)))
        .collect::<HashMap<_, _>>();

    for block in blocks.iter_mut() {
        block.label = relabel
            .get(&block.label)
            .expect("dense relabel should cover every block")
            .clone();
        block.term.relabel_targets(&relabel);
        if let Some(exc_edge) = &mut block.exc_edge {
            exc_edge.target = relabel
                .get(&exc_edge.target)
                .expect("dense relabel should cover every exception target")
                .clone();
        }
    }
}

pub(crate) trait RelabelBlockTargets {
    fn relabel_targets(&mut self, relabel: &HashMap<BlockLabel, BlockLabel>);
}

impl<E: Instr> RelabelBlockTargets for BlockTerm<E> {
    fn relabel_targets(&mut self, relabel: &HashMap<BlockLabel, BlockLabel>) {
        match self {
            BlockTerm::Jump(edge) => {
                edge.target = relabel
                    .get(&edge.target)
                    .expect("dense relabel should cover every jump target")
                    .clone();
            }
            BlockTerm::IfTerm(if_term) => {
                if_term.then_label = relabel
                    .get(&if_term.then_label)
                    .expect("dense relabel should cover every then target")
                    .clone();
                if_term.else_label = relabel
                    .get(&if_term.else_label)
                    .expect("dense relabel should cover every else target")
                    .clone();
            }
            BlockTerm::BranchTable(branch) => {
                for target in &mut branch.targets {
                    *target = relabel
                        .get(target)
                        .expect("dense relabel should cover every br_table target")
                        .clone();
                }
                branch.default_label = relabel
                    .get(&branch.default_label)
                    .expect("dense relabel should cover every br_table default target")
                    .clone();
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
        }
    }
}
