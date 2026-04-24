use crate::codegen::define_block_entry_counter;
use crate::{CounterBuilder, ExplicitCounterPlacement, InstrumentationConfig};
use soac_core::block_py::{BlockPyFunction, BlockPyModule, FunctionExecutionMode, Meta, WithMeta};
use soac_core::pass_tracker::{NoopPassTracker, PassTracker};
use soac_lowering::block_py::counters::IncrementCounter;
use soac_opt::typed::{InstrTyped, TypedCodegenModuleShape};

fn functions_with_counter_instrumentation_mut(
    functions: &mut [BlockPyFunction<TypedCodegenModuleShape>],
) -> impl Iterator<Item = &mut BlockPyFunction<TypedCodegenModuleShape>> {
    functions
        .iter_mut()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
}

pub fn instrument_module(
    module: BlockPyModule<TypedCodegenModuleShape>,
    config: &InstrumentationConfig,
) -> Result<BlockPyModule<TypedCodegenModuleShape>, String> {
    instrument_module_with_tracker(module, config, &mut NoopPassTracker::new())
}

pub fn instrument_module_with_tracker(
    module: BlockPyModule<TypedCodegenModuleShape>,
    config: &InstrumentationConfig,
    pass_tracker: &mut impl PassTracker,
) -> Result<BlockPyModule<TypedCodegenModuleShape>, String> {
    if config.explicit_counter_placement != ExplicitCounterPlacement::Typed {
        return Ok(module);
    }
    if config.counters.locality && config.counters.profiled_cold_blocks {
        Ok(
            pass_tracker.record_timing("typed_block_entry_counters", || {
                let mut counted = module;
                instrument_typed_module_with_block_entry_counters(&mut counted);
                counted
            }),
        )
    } else {
        Ok(module)
    }
}

pub fn instrument_typed_module_with_block_entry_counters(
    module: &mut BlockPyModule<TypedCodegenModuleShape>,
) {
    let BlockPyModule {
        callable_defs,
        counter_defs,
        ..
    } = module;
    let mut counters = CounterBuilder::new(counter_defs);
    for function in functions_with_counter_instrumentation_mut(callable_defs) {
        for block in &mut function.blocks {
            let counter_id =
                define_block_entry_counter(&mut counters, function.function_id, block.label).id();
            block.body.insert(
                0,
                InstrTyped::LegacyIncrementCounter(
                    IncrementCounter::new(counter_id).with_meta(Meta::synthetic()),
                ),
            );
        }
    }
}

#[cfg(test)]
mod test;
