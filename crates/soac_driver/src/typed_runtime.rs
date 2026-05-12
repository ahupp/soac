use soac_config::SoacEnvConfig;
use soac_core::block_py::BlockPyModule;
use soac_instrument::{InstrumentationConfig, instrument_typed_module};
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::{FactStore, TypedBlockPyModuleShape, lower_blockpy_module_to_typed};
use soac_opt::passes::{
    annotate_typed_module_value_facts, infer_module_value_facts, lower_typed_if_tests_to_truthy,
    sync_typed_module_value_facts,
};
use std::time::{Duration, Instant};

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

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
    let total_start = Instant::now();
    let infer_start = Instant::now();
    let mut value_facts = infer_module_value_facts(module);
    let infer_elapsed = infer_start.elapsed();
    let lower_start = Instant::now();
    let mut typed_module = lower_blockpy_module_to_typed(module.clone());
    let lower_elapsed = lower_start.elapsed();
    let instrument_start = Instant::now();
    typed_module = instrument_typed_module(
        typed_module,
        &InstrumentationConfig::from_env_config(env_config),
    )?;
    let instrument_elapsed = instrument_start.elapsed();
    let annotate_start = Instant::now();
    annotate_typed_module_value_facts(&mut typed_module, &value_facts);
    let annotate_elapsed = annotate_start.elapsed();
    let truthy_start = Instant::now();
    typed_module = lower_typed_if_tests_to_truthy(typed_module);
    let truthy_elapsed = truthy_start.elapsed();
    let rewrite_start = Instant::now();
    apply_rewrites(&mut typed_module, &value_facts)?;
    let rewrite_elapsed = rewrite_start.elapsed();
    let sync_start = Instant::now();
    sync_typed_module_value_facts(&typed_module, &mut value_facts);
    let sync_elapsed = sync_start.elapsed();
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.typed_runtime_prepare",
        runtime_module_id = module.module_name_gen.runtime_module_id().as_u32(),
        function_count = u64::try_from(module.callable_defs.len()).unwrap_or(u64::MAX),
        typed_prepare_infer_facts_us = duration_micros(infer_elapsed),
        typed_prepare_lower_blockpy_us = duration_micros(lower_elapsed),
        typed_prepare_instrument_us = duration_micros(instrument_elapsed),
        typed_prepare_annotate_facts_us = duration_micros(annotate_elapsed),
        typed_prepare_lower_truthy_us = duration_micros(truthy_elapsed),
        typed_prepare_apply_rewrites_us = duration_micros(rewrite_elapsed),
        typed_prepare_sync_facts_us = duration_micros(sync_elapsed),
        typed_prepare_total_us = duration_micros(total_start.elapsed()),
        "typed_runtime_prepare",
    );
    Ok(PreparedTypedRuntimeModule {
        module: typed_module,
        value_facts,
    })
}
