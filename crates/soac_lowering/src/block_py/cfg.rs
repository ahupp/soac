use super::{
    Block, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, FunctionKind,
    HandledExceptionContext, HasBlockContext, Instr, ModuleShape,
};
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
        BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => Vec::new(),
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

/// Validate ephemeral suspension edges before resolved ownership consumes them.
/// These edges describe a future activation, never an executable same-frame jump.
pub(crate) fn validate_suspension_resumes<P: ModuleShape>(
    function: &BlockPyFunction<P>,
) -> Result<(), String> {
    let blocks = function
        .blocks
        .iter()
        .map(|block| (block.label, block))
        .collect::<HashMap<_, _>>();
    for block in &function.blocks {
        let context = block.extra.block_context();
        let Some(target_label) = context.suspension_resume else {
            continue;
        };
        if function.kind == FunctionKind::Function
            || !matches!(block.term, BlockTerm::Return(_))
            || context.handled_exception == HandledExceptionContext::Terminal
        {
            return Err(format!(
                "{}: suspension edge on non-yielding block {}",
                function.names.qualname, block.label,
            ));
        }
        let Some(target) = blocks.get(&target_label) else {
            return Err(format!(
                "{}: suspension edge from {} has missing resume target {}",
                function.names.qualname, block.label, target_label,
            ));
        };
        if !target.params.is_empty()
            || !matches!(target.term, BlockTerm::Jump(_))
            || target.extra.block_context().handled_exception != HandledExceptionContext::Preserve
        {
            return Err(format!(
                "{}: suspension edge from {} does not target a parameter-free resume wrapper at {}",
                function.names.qualname, block.label, target_label,
            ));
        }
    }
    Ok(())
}

pub(crate) fn relabel_blockpy_blocks_dense<I: Instr, E: HasBlockContext>(
    blocks: &mut [Block<I, E>],
) {
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
        let mut context = block.extra.block_context();
        if let Some(target) = context.suspension_resume {
            context.suspension_resume = Some(
                *relabel
                    .get(&target)
                    .expect("dense relabel should cover every suspension resume target"),
            );
            block.extra.set_block_context(context);
        }
        if let Some(exc_edge) = &mut block.exc_edge {
            exc_edge.target = relabel
                .get(&exc_edge.target)
                .expect("dense relabel should cover every exception target")
                .clone();
        }
    }
}

pub(crate) fn relabel_dense_bb_module<P: ModuleShape>(module: &mut BlockPyModule<P>) {
    for callable in &mut module.callable_defs {
        relabel_blockpy_blocks_dense(&mut callable.blocks);
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
            BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {}
        }
    }
}
