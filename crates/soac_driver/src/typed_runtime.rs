use soac_config::SoacEnvConfig;
use soac_core::block_py::BlockPyModule;
use soac_instrument::{InstrumentationConfig, instrument_typed_module};
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::{FactStore, TypedBlockPyModuleShape, lower_blockpy_module_to_typed};
use soac_opt::passes::{
    annotate_typed_module_value_facts, infer_module_value_facts, lower_typed_if_tests_to_truthy,
};

pub struct PreparedTypedRuntimeModule {
    pub module: BlockPyModule<TypedBlockPyModuleShape>,
    pub value_facts: FactStore,
}

pub fn prepare_typed_v3_runtime_module(
    module: &BlockPyModule<BlockPyModuleShape>,
    env_config: &SoacEnvConfig,
) -> Result<PreparedTypedRuntimeModule, String> {
    prepare_typed_v3_runtime_module_with_rewrites(module, env_config, |_, _| Ok(()))
}

pub fn prepare_typed_v3_runtime_module_with_rewrites(
    module: &BlockPyModule<BlockPyModuleShape>,
    env_config: &SoacEnvConfig,
    apply_rewrites: impl FnOnce(
        &mut BlockPyModule<TypedBlockPyModuleShape>,
        &FactStore,
    ) -> Result<(), String>,
) -> Result<PreparedTypedRuntimeModule, String> {
    let value_facts = infer_module_value_facts(module);
    let mut typed_module = lower_blockpy_module_to_typed(module.clone());
    typed_module = instrument_typed_module(
        typed_module,
        &InstrumentationConfig::from_env_config(env_config),
    )?;
    annotate_typed_module_value_facts(&mut typed_module, &value_facts);
    typed_module = lower_typed_if_tests_to_truthy(typed_module);
    apply_rewrites(&mut typed_module, &value_facts)?;
    Ok(PreparedTypedRuntimeModule {
        module: typed_module,
        value_facts,
    })
}
