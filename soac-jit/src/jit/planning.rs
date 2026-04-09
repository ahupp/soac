use soac_blockpy::block_py::{BlockArg, BlockPyFunction, CodegenBlock};
use soac_blockpy::passes::CodegenModuleShape;
use std::collections::HashSet;

#[derive(Clone, Debug)]
pub struct BlockExcDispatchPlan {
    pub target_index: usize,
    pub slot_writes: Vec<(String, BlockArg)>,
}

pub fn jit_param_names_for_block(block: &CodegenBlock) -> Vec<String> {
    block.bb_param_names().map(ToString::to_string).collect()
}

pub fn exc_dispatch_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &CodegenBlock,
) -> Option<BlockExcDispatchPlan> {
    let exc_edge = block.exc_edge.as_ref()?;
    let target_index = exc_edge.target.index();
    let target_block = &function.blocks[target_index];
    let stack_slot_name_set = function
        .storage_layout()
        .as_ref()
        .map(|layout| {
            layout
                .stack_slots()
                .iter()
                .cloned()
                .into_iter()
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let runtime_param_name_set = jit_param_names_for_block(target_block)
        .into_iter()
        .collect::<HashSet<_>>();
    let full_target_param_names = target_block.param_name_vec();
    let mut slot_writes = Vec::new();
    for (target_param_name, source) in full_target_param_names.iter().zip(exc_edge.args.iter()) {
        if runtime_param_name_set.contains(target_param_name)
            || !stack_slot_name_set.contains(target_param_name)
        {
            continue;
        }
        slot_writes.push((target_param_name.clone(), source.clone()));
    }
    Some(BlockExcDispatchPlan {
        target_index,
        slot_writes,
    })
}
