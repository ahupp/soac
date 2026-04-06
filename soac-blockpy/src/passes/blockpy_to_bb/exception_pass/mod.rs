use crate::block_py::{
    BlockEdge, BlockPyFunction, BlockPyModule, BlockTerm, InstrLow, ResolvedStorageBlock,
};
use crate::passes::ruff_to_blockpy::populate_exception_edge_args;
use crate::passes::ResolvedStorageBlockPyPass;

pub fn lower_try_jump_exception_flow(
    module: &BlockPyModule<ResolvedStorageBlockPyPass>,
) -> BlockPyModule<ResolvedStorageBlockPyPass> {
    let callable_defs = module
        .callable_defs
        .iter()
        .cloned()
        .map(lower_function_try_jump_exception_flow)
        .collect();
    BlockPyModule {
        module_name_gen: module.module_name_gen.clone(),
        global_names: module.global_names.clone(),
        callable_defs,
        module_constants: module.module_constants.clone(),
        counter_defs: module.counter_defs.clone(),
    }
}

fn lower_function_try_jump_exception_flow(
    function: BlockPyFunction<ResolvedStorageBlockPyPass>,
) -> BlockPyFunction<ResolvedStorageBlockPyPass> {
    let mut function = BlockPyFunction {
        function_id: function.function_id,
        name_gen: function.name_gen,
        names: function.names,
        kind: function.kind,
        params: function.params,
        blocks: function.blocks,
        doc: function.doc,
        storage_layout: function.storage_layout,
        scope: function.scope,
    };
    // Canonicalize exception-edge blocks so each potentially-raising expression
    // step sits in its own block. This keeps per-expression exception checks
    // explicit in CFG shape (op-block -> jump -> ... -> term-block), which
    // allows the JIT fast path to dispatch exceptions directly from each step.
    split_exception_blocks_for_expr_checks(&mut function);
    populate_exception_edge_args(&mut function.blocks);

    function
}

fn split_exception_blocks_for_expr_checks(
    function: &mut BlockPyFunction<ResolvedStorageBlockPyPass>,
) {
    let mut out = Vec::with_capacity(function.blocks.len());

    for block in std::mem::take(&mut function.blocks) {
        if block.exc_edge.is_none() || block.body.is_empty() {
            out.push(block);
            continue;
        }

        let segment_params = block.bb_params().cloned().collect::<Vec<_>>();
        let mut current_label = block.label;
        let exc_edge = block.exc_edge.clone();
        let mut ops = block.body.into_iter().peekable();

        let mut segment_ops = Vec::new();
        while let Some(op) = ops.next() {
            let ends_segment = op_updates_exception_state(&op) && ops.peek().is_some();
            segment_ops.push(op.clone());

            if ends_segment {
                let next_label = function.name_gen.next_block_name();
                out.push(ResolvedStorageBlock {
                    label: current_label,
                    body: std::mem::take(&mut segment_ops),
                    term: BlockTerm::Jump(BlockEdge::new(next_label)),
                    params: segment_params.clone(),
                    exc_edge: exc_edge.clone(),
                });
                current_label = next_label;
            }

            if ops.peek().is_none() {
                out.push(ResolvedStorageBlock {
                    label: current_label,
                    body: std::mem::take(&mut segment_ops),
                    term: block.term.clone(),
                    params: segment_params.clone(),
                    exc_edge: exc_edge.clone(),
                });
            }
        }
    }

    function.blocks = out;
}

fn op_updates_exception_state<N>(op: &InstrLow<N>) -> bool
where
    N: crate::block_py::NameLike,
{
    matches!(op, InstrLow::Store(_) | InstrLow::Del(_))
}

#[cfg(test)]
mod test;
