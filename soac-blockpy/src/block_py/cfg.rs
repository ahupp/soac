use super::{
    instr_any, map_function_blocks, Block, BlockLabel, BlockPyFunction, BlockTerm, ChildVisitable,
    Del, Instr, Load, MapInstr, MapTerm, Mappable, Meta, ModuleShape, Store, UnresolvedName,
    WithMeta,
};
use crate::namegen::fresh_name;
use ruff_python_ast as ast;
use std::collections::{HashMap, HashSet};

fn blockpy_successors<E: Instr>(block: &Block<E>) -> Vec<BlockLabel> {
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

pub(crate) fn fold_jumps_to_trivial_return_blockpy<E>(blocks: &mut [Block<E>])
where
    E: Clone + Instr,
{
    let trivial_ret_none_terms: HashMap<BlockLabel, BlockTerm<E>> = blocks
        .iter()
        .filter(|block| {
            block.body.is_empty()
                && block.params.is_empty()
                && block.exc_edge.is_none()
                && matches!(&block.term, BlockTerm::Return(_))
        })
        .map(|block| (block.label.clone(), block.term.clone()))
        .collect();

    for block in blocks.iter_mut() {
        let jump_target = match &block.term {
            BlockTerm::Jump(target) if target.args.is_empty() && block.exc_edge.is_none() => {
                Some(target.target.clone())
            }
            _ => None,
        };
        if let Some(target) = jump_target {
            if let Some(term) = trivial_ret_none_terms.get(&target) {
                block.term = term.clone();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_py::{BlockEdge, InstrWithAwaitAndYield, InstrWithConstantNone};

    #[test]
    fn fold_jumps_preserves_exception_scoped_return_blocks() {
        let entry = BlockLabel::from_index(0);
        let body = BlockLabel::from_index(1);
        let handler = BlockLabel::from_index(2);
        let mut blocks = vec![
            Block::new(
                entry,
                Vec::new(),
                BlockTerm::Jump(BlockEdge::new(body)),
                Vec::new(),
                None,
            ),
            Block::new(
                body,
                Vec::new(),
                BlockTerm::Return(InstrWithAwaitAndYield::constant_none()),
                Vec::new(),
                Some(BlockEdge::new(handler)),
            ),
            Block::new(
                handler,
                Vec::new(),
                BlockTerm::Return(InstrWithAwaitAndYield::constant_none()),
                Vec::new(),
                None,
            ),
        ];

        fold_jumps_to_trivial_return_blockpy(&mut blocks);

        let BlockTerm::Jump(edge) = &blocks[0].term else {
            panic!("entry jump should not fold across exception context: {blocks:#?}");
        };
        assert_eq!(edge.target, body);
    }
}

pub(crate) fn prune_unreachable_blockpy_blocks<E: Instr>(
    entry_label: BlockLabel,
    extra_roots: &[BlockLabel],
    blocks: &mut Vec<Block<E>>,
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

fn fresh_eval_name() -> ast::name::Name {
    fresh_name("eval").into()
}

fn typed_store_expr<E>(target: ast::name::Name, meta: Meta, value: E) -> E
where
    E: Instr + From<Store<E>>,
    <E as Instr>::Name: From<ast::name::Name>,
{
    Store::<E>::new(target, value).with_meta(meta).into()
}

fn typed_del_expr<E>(target: ast::name::Name, meta: Meta) -> E
where
    E: Instr<Name = UnresolvedName> + From<Del<E>>,
{
    Del::<E>::new(target, false).with_meta(meta).into()
}

fn append_stmt_cleanup<E>(out: &mut Vec<E>, cleanup: Vec<ast::name::Name>)
where
    E: Instr<Name = UnresolvedName> + From<Del<E>>,
{
    for temp in cleanup.into_iter().rev() {
        out.push(typed_del_expr(temp, Meta::synthetic()));
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
    cleanup: &mut Vec<ast::name::Name>,
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
        out.push(typed_store_expr(target.clone(), Meta::synthetic(), expr));
        cleanup.push(target.clone());
        Load::new(target).with_meta(Meta::synthetic()).into()
    } else {
        expr
    }
}

fn rewrite_matching_children_in_expr<E, F>(
    expr: E,
    out: &mut Vec<E>,
    cleanup: &mut Vec<ast::name::Name>,
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
    cleanup: &'b mut Vec<ast::name::Name>,
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
    P: ModuleShape<Instr = E>,
    E: Instr<Name = UnresolvedName>
        + ChildVisitable<E>
        + Mappable<E, Mapped<E> = E>
        + From<Load<E>>
        + From<Store<E>>
        + From<Del<E>>,
    F: FnMut(&E) -> bool,
{
    map_function_blocks(callable_def, |block| {
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
            let expr = if expr_contains_matching_subexpression(&expr, &mut should_hoist) {
                rewrite_matching_children_in_expr(expr, &mut setup, &mut cleanup, &mut should_hoist)
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

pub(crate) fn relabel_blockpy_blocks_dense<I: Instr>(blocks: &mut [Block<I>]) {
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
                edge.target = *relabel
                    .get(&edge.target)
                    .expect("dense relabel should cover every jump target");
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
