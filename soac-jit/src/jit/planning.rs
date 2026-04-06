use soac_blockpy::block_py::{BlockArg, BlockPyFunction, BlockPyModule, CodegenBlock, FunctionId};
use soac_blockpy::passes::CodegenBlockPyPass;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct BlockExcDispatchPlan {
    pub target_index: usize,
    pub slot_writes: Vec<(String, BlockArg)>,
}

type ModuleRegistry = HashMap<String, BlockPyModule<CodegenBlockPyPass>>;

static BB_MODULE_REGISTRY: OnceLock<Mutex<ModuleRegistry>> = OnceLock::new();

pub fn jit_param_names_for_block(block: &CodegenBlock) -> Vec<String> {
    block.bb_param_names().map(ToString::to_string).collect()
}

pub fn exc_dispatch_plan(
    function: &BlockPyFunction<CodegenBlockPyPass>,
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

fn bb_module_registry() -> &'static Mutex<ModuleRegistry> {
    BB_MODULE_REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn register_clif_module_plans(
    module_name: &str,
    module: &BlockPyModule<CodegenBlockPyPass>,
) -> Result<(), String> {
    let mut module_registry = bb_module_registry()
        .lock()
        .map_err(|_| "failed to lock bb module registry".to_string())?;
    module_registry.insert(module_name.to_string(), module.clone());
    Ok(())
}

pub fn lookup_blockpy_module(module_name: &str) -> Option<BlockPyModule<CodegenBlockPyPass>> {
    let registry = bb_module_registry().lock().ok()?;
    registry.get(module_name).cloned()
}

pub fn lookup_blockpy_function(
    module_name: &str,
    function_id: FunctionId,
) -> Option<BlockPyFunction<CodegenBlockPyPass>> {
    let module = lookup_blockpy_module(module_name)?;
    module
        .callable_defs
        .into_iter()
        .find(|function| function.function_id == function_id)
}
