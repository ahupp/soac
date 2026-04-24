use crate::instrument::{
    SpecializationCounterCandidate, define_block_entry_counter, define_branch_outcome_counter,
    define_refcount_counters, define_specialization_counter_candidate,
    is_operator_specialization_binop_kind, is_profile_call_candidate,
};
use crate::{CounterBuilder, ExplicitCounterPlacement, InstrumentationConfig};
use soac_core::block_py::IncrementCounter;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, BlockTerm, ChildVisitable, CounterScope, FunctionExecutionMode,
    HasSemanticInstrId, Meta, NameLocation, RuntimeFunctionId, Visit, WithMeta,
};
use soac_core::pass_tracker::{NoopPassTracker, PassTracker};
use soac_opt::typed::{InstrTyped, TypedCodegenModuleShape};

fn functions_with_counter_instrumentation_mut(
    functions: &mut [BlockPyFunction<TypedCodegenModuleShape>],
) -> impl Iterator<Item = &mut BlockPyFunction<TypedCodegenModuleShape>> {
    functions
        .iter_mut()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
}

fn functions_with_counter_instrumentation(
    functions: &[BlockPyFunction<TypedCodegenModuleShape>],
) -> impl Iterator<Item = &BlockPyFunction<TypedCodegenModuleShape>> {
    functions
        .iter()
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

pub fn define_module_counter_defs(
    module: &mut BlockPyModule<TypedCodegenModuleShape>,
    config: &InstrumentationConfig,
) -> Result<(), String> {
    if config.counters.call_targets {
        define_module_call_target_counters(module);
    }
    if config.counters.locality {
        if config.counters.profiled_cold_blocks {
            define_module_block_entry_counters(module);
        }
        define_module_locality_counters(module);
    }
    if let Some(scope) = config.counters.refcounts.scope() {
        define_module_refcount_counters(module, scope)?;
    }
    Ok(())
}

fn define_module_block_entry_counters(module: &mut BlockPyModule<TypedCodegenModuleShape>) {
    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    for function in functions_with_counter_instrumentation(&module.callable_defs) {
        for block in &function.blocks {
            define_block_entry_counter(&mut counters, function.function_id, block.label);
        }
    }
}

fn define_module_refcount_counters(
    module: &mut BlockPyModule<TypedCodegenModuleShape>,
    scope: CounterScope,
) -> Result<(), String> {
    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    let function_ids = functions_with_counter_instrumentation(&module.callable_defs)
        .map(|function| function.function_id)
        .collect::<Vec<_>>();
    define_refcount_counters(&mut counters, scope, function_ids)
}

fn define_module_locality_counters(module: &mut BlockPyModule<TypedCodegenModuleShape>) {
    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    for function in functions_with_counter_instrumentation(&module.callable_defs) {
        for block in &function.blocks {
            let BlockTerm::IfTerm(if_term) = &block.term else {
                continue;
            };
            let instr_id = if_term.test.semantic_instr_id();
            define_branch_outcome_counter(&mut counters, function.function_id, instr_id);
        }
    }
}

fn define_module_call_target_counters(module: &mut BlockPyModule<TypedCodegenModuleShape>) {
    fn is_operator_specialization_candidate(expr: &InstrTyped) -> bool {
        match expr {
            InstrTyped::BinOp(op) => is_operator_specialization_binop_kind(op.kind),
            _ => false,
        }
    }

    fn is_global_index_candidate(expr: &InstrTyped) -> bool {
        match expr {
            InstrTyped::Load(op) => matches!(op.name.location, NameLocation::Global(_)),
            InstrTyped::LegacyStore(op) => matches!(op.name.location, NameLocation::Global(_)),
            _ => false,
        }
    }

    struct SpecializationCandidateCounterCollector<'a, 'b> {
        function_id: RuntimeFunctionId,
        counters: &'a mut CounterBuilder<'b>,
    }

    impl Visit<InstrTyped> for SpecializationCandidateCounterCollector<'_, '_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if is_global_index_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                define_specialization_counter_candidate(
                    self.counters,
                    self.function_id,
                    SpecializationCounterCandidate::GlobalIndexed { instr_id },
                );
            }
            match expr {
                InstrTyped::GetAttrTyped(_) | InstrTyped::LegacyGetAttr(_) => {
                    let instr_id = expr.semantic_instr_id();
                    define_specialization_counter_candidate(
                        self.counters,
                        self.function_id,
                        SpecializationCounterCandidate::FieldAccess { instr_id },
                    );
                }
                InstrTyped::SetAttrTyped(_) | InstrTyped::LegacySetAttr(_) => {
                    let instr_id = expr.semantic_instr_id();
                    define_specialization_counter_candidate(
                        self.counters,
                        self.function_id,
                        SpecializationCounterCandidate::FieldAccess { instr_id },
                    );
                }
                _ => {}
            }
            if is_operator_specialization_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                define_specialization_counter_candidate(
                    self.counters,
                    self.function_id,
                    SpecializationCounterCandidate::OperatorHotShapes { instr_id },
                );
            }
            if matches!(expr, InstrTyped::LegacyGetItem(_)) {
                let instr_id = expr.semantic_instr_id();
                define_specialization_counter_candidate(
                    self.counters,
                    self.function_id,
                    SpecializationCounterCandidate::GetItem { instr_id },
                );
            }
            if matches!(expr, InstrTyped::LegacySetItem(_)) {
                let instr_id = expr.semantic_instr_id();
                define_specialization_counter_candidate(
                    self.counters,
                    self.function_id,
                    SpecializationCounterCandidate::SetItem { instr_id },
                );
            }
            match expr {
                InstrTyped::CallTyped(call)
                    if is_profile_call_candidate(&call.args, &call.keywords) =>
                {
                    let instr_id = expr.semantic_instr_id();
                    define_specialization_counter_candidate(
                        self.counters,
                        self.function_id,
                        SpecializationCounterCandidate::Call { instr_id },
                    );
                }
                InstrTyped::LegacyCall(call)
                    if is_profile_call_candidate(&call.args, &call.keywords) =>
                {
                    let instr_id = expr.semantic_instr_id();
                    define_specialization_counter_candidate(
                        self.counters,
                        self.function_id,
                        SpecializationCounterCandidate::Call { instr_id },
                    );
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    for function in functions_with_counter_instrumentation(&module.callable_defs) {
        let mut collector = SpecializationCandidateCounterCollector {
            function_id: function.function_id,
            counters: &mut counters,
        };
        collector.visit_fn(function);
    }
}

pub(crate) fn instrument_typed_module_with_block_entry_counters(
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
