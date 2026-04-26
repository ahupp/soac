use crate::instrument::{
    SpecializationCounterCandidate, define_block_entry_counter, define_branch_outcome_counter,
    define_refcount_counters, define_specialization_counter_candidate,
    is_operator_specialization_binop_kind, is_profile_call_candidate,
};
use crate::{CounterBuilder, InstrumentationConfig};
use soac_config::ExecTraceConfig;
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, BlockTerm, CallArgPositional, ChildVisitable, ConstantExpr,
    CounterScope, FunctionExecutionMode, HasSemanticInstrId, IncrementCounter, LiteralValue, Load,
    Meta, NameLocation, ResolvedName, RuntimeFunctionId, RuntimeName, StringLiteral, Tuple, Visit,
    WithMeta,
};
use soac_core::pass_tracker::{NoopPassTracker, PassTracker};
use soac_ir_typed::{InstrTyped, TypedCall, TypedCodegenModuleShape};
use std::collections::HashMap;

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

pub fn instrument_typed_module(
    module: BlockPyModule<TypedCodegenModuleShape>,
    config: &InstrumentationConfig,
) -> Result<BlockPyModule<TypedCodegenModuleShape>, String> {
    instrument_typed_module_with_tracker(module, config, &mut NoopPassTracker::new())
}

pub fn instrument_typed_module_with_tracker(
    module: BlockPyModule<TypedCodegenModuleShape>,
    config: &InstrumentationConfig,
    pass_tracker: &mut impl PassTracker,
) -> Result<BlockPyModule<TypedCodegenModuleShape>, String> {
    let traced = if let Some(trace_config) = config.trace.as_ref() {
        pass_tracker.record_timing("typed_trace", || {
            let mut traced = module;
            instrument_typed_module_for_trace(&mut traced, trace_config);
            traced
        })
    } else {
        module
    };

    if config.counters.locality && config.counters.profiled_cold_blocks {
        Ok(
            pass_tracker.record_timing("typed_block_entry_counters", || {
                let mut counted = traced;
                instrument_typed_module_with_block_entry_counters(&mut counted);
                counted
            }),
        )
    } else {
        Ok(traced)
    }
}

pub fn define_typed_module_counter_defs(
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
            InstrTyped::Store(op) => matches!(op.name.location, NameLocation::Global(_)),
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
                InstrTyped::GetAttrTyped(_) => {
                    let instr_id = expr.semantic_instr_id();
                    define_specialization_counter_candidate(
                        self.counters,
                        self.function_id,
                        SpecializationCounterCandidate::FieldAccess { instr_id },
                    );
                }
                InstrTyped::SetAttrTyped(_) => {
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
            if matches!(expr, InstrTyped::GetItem(_)) {
                let instr_id = expr.semantic_instr_id();
                define_specialization_counter_candidate(
                    self.counters,
                    self.function_id,
                    SpecializationCounterCandidate::GetItem { instr_id },
                );
            }
            if matches!(expr, InstrTyped::SetItem(_)) {
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
                InstrTyped::IncrementCounter(
                    IncrementCounter::new(counter_id).with_meta(Meta::synthetic()),
                ),
            );
        }
    }
}

pub(crate) fn instrument_typed_module_for_trace(
    module: &mut BlockPyModule<TypedCodegenModuleShape>,
    config: &ExecTraceConfig,
) {
    let global_names = module.global_names.clone();
    let module_constants = &mut module.module_constants;
    for function in &mut module.callable_defs {
        if let Some(filter) = config.qualname_filter.as_ref() {
            if function.names.qualname != *filter {
                continue;
            }
        }
        let qualname = function.names.qualname.clone();
        let locator = PreparedTraceNameLocator::new(function, global_names.as_slice());
        for block in &mut function.blocks {
            let block_params = block.param_name_vec();
            let trace_expr = if config.include_params && !block_params.is_empty() {
                helper_call_expr(
                    "bb_trace_enter",
                    vec![
                        string_literal_expr(module_constants, qualname.as_str()),
                        string_literal_expr(module_constants, block.label.to_string().as_str()),
                        param_pairs_expr(module_constants, &locator, block_params.as_slice()),
                    ],
                )
            } else {
                helper_call_expr(
                    "bb_trace_enter",
                    vec![
                        string_literal_expr(module_constants, qualname.as_str()),
                        string_literal_expr(module_constants, block.label.to_string().as_str()),
                    ],
                )
            };
            block.body.insert(0, trace_expr);
        }
    }
}

struct PreparedTraceNameLocator {
    local_slots: HashMap<String, u32>,
    existing_locations: HashMap<String, NameLocation>,
    captured_cell_slots: HashMap<String, u32>,
    owned_cell_slots: HashMap<String, u32>,
    global_slots: HashMap<String, u32>,
}

impl PreparedTraceNameLocator {
    fn new(function: &BlockPyFunction<TypedCodegenModuleShape>, global_names: &[String]) -> Self {
        let mut local_slots = function
            .storage_layout
            .as_ref()
            .map(|layout| {
                layout
                    .stack_slots()
                    .iter()
                    .enumerate()
                    .map(|(slot, name)| (name.clone(), slot as u32))
                    .collect::<HashMap<_, _>>()
            })
            .unwrap_or_default();
        for (slot, name) in function.params.names().into_iter().enumerate() {
            local_slots.entry(name).or_insert(slot as u32);
        }
        let mut existing_locations = HashMap::new();
        for block in &function.blocks {
            for stmt in &block.body {
                if let InstrTyped::Store(store) = stmt {
                    existing_locations
                        .entry(store.name.id.to_string())
                        .or_insert(store.name.location);
                }
            }
        }
        let captured_cell_slots = function
            .storage_layout
            .as_ref()
            .map(|layout| {
                let mut slots = HashMap::new();
                for (slot, closure_slot) in layout.freevars.iter().enumerate() {
                    slots.insert(closure_slot.storage_name.clone(), slot as u32);
                    slots.insert(closure_slot.logical_name.clone(), slot as u32);
                }
                slots
            })
            .unwrap_or_default();
        let owned_cell_slots = function
            .storage_layout
            .as_ref()
            .map(|layout| {
                let mut slots = HashMap::new();
                for (slot, closure_slot) in layout
                    .cellvars
                    .iter()
                    .chain(layout.runtime_cells.iter())
                    .enumerate()
                {
                    slots.insert(closure_slot.storage_name.clone(), slot as u32);
                    slots.insert(closure_slot.logical_name.clone(), slot as u32);
                }
                slots
            })
            .unwrap_or_default();
        let global_slots = global_names
            .iter()
            .enumerate()
            .map(|(slot, name)| (name.clone(), slot as u32))
            .collect::<HashMap<_, _>>();
        Self {
            local_slots,
            existing_locations,
            captured_cell_slots,
            owned_cell_slots,
            global_slots,
        }
    }

    fn load_name(&self, id: &str) -> ResolvedName {
        let location = if let Some(slot) = self.local_slots.get(id).copied() {
            NameLocation::local(slot)
        } else if let Some(location) = self.existing_locations.get(id).copied() {
            location
        } else if let Some(slot) = self.captured_cell_slots.get(id).copied() {
            NameLocation::closure_cell(slot)
        } else if let Some(slot) = self.owned_cell_slots.get(id).copied() {
            NameLocation::owned_cell(slot)
        } else {
            let slot = self
                .global_slots
                .get(id)
                .copied()
                .unwrap_or_else(|| panic!("trace locator missing global slot for {id}"));
            NameLocation::global(slot)
        };
        ResolvedName {
            id: id.into(),
            location,
        }
    }
}

fn helper_call_expr(helper_name: &str, args: Vec<InstrTyped>) -> InstrTyped {
    let meta = Meta::synthetic();
    let runtime_name = RuntimeName::from_name(helper_name)
        .unwrap_or_else(|| panic!("unknown SOAC trace helper runtime name {helper_name:?}"));
    let func: InstrTyped = Load::new(ResolvedName {
        id: runtime_name.name().into(),
        location: NameLocation::RuntimeName(runtime_name),
    })
    .with_meta(meta.clone())
    .into();
    InstrTyped::CallTyped(
        TypedCall::generic(
            func,
            args.into_iter()
                .map(CallArgPositional::Positional)
                .collect::<Vec<_>>(),
            Vec::new(),
        )
        .with_meta(meta),
    )
}

fn string_literal_expr(module_constants: &mut Vec<ConstantExpr>, value: &str) -> InstrTyped {
    let meta = Meta::synthetic();
    let index = u32::try_from(module_constants.len())
        .expect("trace module constant count should fit in u32");
    module_constants.push(ConstantExpr::Literal(
        LiteralValue::new(StringLiteral {
            value: value.to_string(),
        })
        .with_meta(meta.clone()),
    ));
    Load::new(ResolvedName {
        id: format!("__dp_constant_{index}").into(),
        location: NameLocation::Constant(index),
    })
    .with_meta(meta)
    .into()
}

fn tuple_expr(values: Vec<InstrTyped>) -> InstrTyped {
    Tuple::new(values).with_meta(Meta::synthetic()).into()
}

fn param_pairs_expr(
    module_constants: &mut Vec<ConstantExpr>,
    locator: &PreparedTraceNameLocator,
    params: &[String],
) -> InstrTyped {
    tuple_expr(
        params
            .iter()
            .map(|param| {
                let name = locator.load_name(param);
                let meta = Meta::synthetic();
                tuple_expr(vec![
                    string_literal_expr(module_constants, param),
                    Load::new(name).with_meta(meta).into(),
                ])
            })
            .collect(),
    )
}

#[cfg(test)]
mod test;
