use crate::instrument::{
    SpecializationCounterCandidate, define_block_entry_counter, define_branch_outcome_counter,
    define_refcount_counters, define_specialization_counter_candidate,
    is_operator_specialization_binop_kind, is_profile_call_candidate,
};
use crate::{CounterBuilder, ExplicitCounterPlacement, InstrumentationConfig};
use soac_config::{ExecTraceConfig, SoacEnvConfig};
use soac_core::block_py::{
    BlockPyFunction, BlockPyModule, BlockTerm, Call, CallArgPositional, ChildVisitable,
    CounterScope, CounterSite, FunctionExecutionMode, HasSemanticInstrId, LiteralValue, Load, Meta,
    ModuleShape, NameLocation, ResolvedName, RuntimeFunctionId, RuntimeName, StringLiteral, Tuple,
    Visit, WithMeta,
};
use soac_core::pass_tracker::PassTracker;
use soac_lowering::block_py::counters::IncrementCounter;
use soac_lowering::passes::{CodegenModuleShape, InstrCodegen, InstrResolved};
use std::collections::HashMap;

pub fn call_target_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
    InstrumentationConfig::from_env_config(config)
        .counters
        .call_targets
}

pub fn locality_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
    InstrumentationConfig::from_env_config(config)
        .counters
        .locality
}

pub fn refcount_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
    InstrumentationConfig::from_env_config(config)
        .counters
        .refcounts
        .scope()
        .is_some()
}

pub fn deopt_entry_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
    InstrumentationConfig::from_env_config(config).deopt_entry_counters_enabled()
}

pub fn specialization_runtime_logging_enabled(config: &SoacEnvConfig) -> bool {
    InstrumentationConfig::from_env_config(config).specialization_runtime_logging_enabled()
}

fn functions_with_counter_instrumentation<P: ModuleShape>(
    functions: &[BlockPyFunction<P>],
) -> impl Iterator<Item = &BlockPyFunction<P>> {
    functions
        .iter()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
}

pub fn instrument_module_with_tracker(
    module: BlockPyModule<CodegenModuleShape>,
    config: &InstrumentationConfig,
    pass_tracker: &mut impl PassTracker,
) -> Result<BlockPyModule<CodegenModuleShape>, String> {
    let traced = if let Some(trace_config) = config.trace.as_ref() {
        pass_tracker.run_pass("bb_trace", || {
            let mut traced = module;
            instrument_bb_module_for_trace(&mut traced, trace_config);
            traced
        })
    } else {
        module
    };

    if config.explicit_counter_placement == ExplicitCounterPlacement::Typed {
        return Ok(traced);
    }

    let call_target_counted = if config.counters.call_targets {
        pass_tracker.run_pass("bb_call_target_counters", || {
            let mut counted = traced;
            instrument_bb_module_with_call_target_counters(&mut counted);
            counted
        })
    } else {
        traced
    };

    let locality_counted = if config.counters.locality {
        pass_tracker.run_pass("bb_locality_counters", || {
            let mut counted = call_target_counted;
            if config.counters.profiled_cold_blocks {
                match config.explicit_counter_placement {
                    ExplicitCounterPlacement::Codegen => {
                        instrument_bb_module_with_block_entry_counters(&mut counted);
                    }
                    ExplicitCounterPlacement::Typed => {
                        define_bb_module_block_entry_counters(&mut counted);
                    }
                }
            }
            instrument_bb_module_with_locality_counters(&mut counted);
            counted
        })
    } else {
        call_target_counted
    };

    if let Some(scope) = config.counters.refcounts.scope() {
        pass_tracker.record_timing("bb_refcount_counters", || {
            let mut counted = locality_counted;
            instrument_bb_module_with_refcount_counters(&mut counted, scope)?;
            Ok(counted)
        })
    } else {
        Ok(locality_counted)
    }
}

fn functions_with_counter_instrumentation_mut<P: ModuleShape>(
    functions: &mut [BlockPyFunction<P>],
) -> impl Iterator<Item = &mut BlockPyFunction<P>> {
    functions
        .iter_mut()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
}

pub fn instrument_bb_module_for_trace(
    module: &mut BlockPyModule<CodegenModuleShape>,
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

pub(crate) fn define_bb_module_block_entry_counters<P: ModuleShape>(module: &mut BlockPyModule<P>) {
    let BlockPyModule {
        callable_defs,
        counter_defs,
        ..
    } = module;
    let mut counters = CounterBuilder::new(counter_defs);
    for function in functions_with_counter_instrumentation(callable_defs) {
        for block in &function.blocks {
            define_block_entry_counter(&mut counters, function.function_id, block.label);
        }
    }
}

pub fn instrument_bb_module_with_block_entry_counters(
    module: &mut BlockPyModule<CodegenModuleShape>,
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
                InstrCodegen::from(IncrementCounter::new(counter_id).with_meta(Meta::synthetic())),
            );
        }
    }
}

pub fn instrument_bb_module_with_refcount_counters(
    module: &mut BlockPyModule<CodegenModuleShape>,
    scope: CounterScope,
) -> Result<(), String> {
    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    let function_ids = functions_with_counter_instrumentation(&module.callable_defs)
        .map(|function| function.function_id)
        .collect::<Vec<_>>();
    define_refcount_counters(&mut counters, scope, function_ids)
}

pub fn instrument_bb_module_with_global_load_counters(
    module: &mut BlockPyModule<CodegenModuleShape>,
) {
    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    for kind in ["global_load_hit", "global_load_miss"] {
        counters.define_if_missing(
            CounterScope::Global,
            kind,
            CounterSite::Runtime {
                function_id: None,
                instr_id: None,
            },
        );
    }
}

pub fn instrument_bb_module_with_call_target_counters(
    module: &mut BlockPyModule<CodegenModuleShape>,
) {
    fn is_operator_specialization_candidate(expr: &InstrCodegen) -> bool {
        match expr {
            InstrCodegen::BinOp(op) => is_operator_specialization_binop_kind(op.kind),
            _ => false,
        }
    }

    fn is_global_index_candidate(expr: &InstrCodegen) -> bool {
        match expr {
            InstrCodegen::Load(op) => matches!(op.name.location, NameLocation::Global(_)),
            InstrCodegen::Store(op) => matches!(op.name.location, NameLocation::Global(_)),
            _ => false,
        }
    }

    fn is_getitem_specialization_candidate(expr: &InstrCodegen) -> bool {
        matches!(expr, InstrCodegen::GetItem(_))
    }

    fn is_setitem_specialization_candidate(expr: &InstrCodegen) -> bool {
        matches!(expr, InstrCodegen::SetItem(_))
    }

    struct SpecializationCandidateCounterCollector<'a, 'b> {
        function_id: RuntimeFunctionId,
        counters: &'a mut CounterBuilder<'b>,
    }

    impl Visit<InstrCodegen> for SpecializationCandidateCounterCollector<'_, '_> {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            if is_global_index_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                define_specialization_counter_candidate(
                    self.counters,
                    self.function_id,
                    SpecializationCounterCandidate::GlobalIndexed { instr_id },
                );
            }
            match expr {
                InstrCodegen::GetAttr(_) => {
                    let instr_id = expr.semantic_instr_id();
                    define_specialization_counter_candidate(
                        self.counters,
                        self.function_id,
                        SpecializationCounterCandidate::FieldAccess { instr_id },
                    );
                }
                InstrCodegen::SetAttr(_) => {
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
            if is_getitem_specialization_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                define_specialization_counter_candidate(
                    self.counters,
                    self.function_id,
                    SpecializationCounterCandidate::GetItem { instr_id },
                );
            }
            if is_setitem_specialization_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                define_specialization_counter_candidate(
                    self.counters,
                    self.function_id,
                    SpecializationCounterCandidate::SetItem { instr_id },
                );
            }
            if let InstrCodegen::Call(call) = expr {
                if is_profile_call_candidate(&call.args, &call.keywords) {
                    let instr_id = expr.semantic_instr_id();
                    define_specialization_counter_candidate(
                        self.counters,
                        self.function_id,
                        SpecializationCounterCandidate::Call { instr_id },
                    );
                }
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

pub fn instrument_bb_module_with_locality_counters(module: &mut BlockPyModule<CodegenModuleShape>) {
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

struct PreparedTraceNameLocator {
    local_slots: HashMap<String, u32>,
    existing_locations: HashMap<String, NameLocation>,
    captured_cell_slots: HashMap<String, u32>,
    owned_cell_slots: HashMap<String, u32>,
    global_slots: HashMap<String, u32>,
}

impl PreparedTraceNameLocator {
    fn new(function: &BlockPyFunction<CodegenModuleShape>, global_names: &[String]) -> Self {
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
                if let InstrCodegen::Store(store) = stmt {
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

fn helper_call_expr(helper_name: &str, args: Vec<InstrCodegen>) -> InstrCodegen {
    let meta = Meta::synthetic();
    let runtime_name = RuntimeName::from_name(helper_name)
        .unwrap_or_else(|| panic!("unknown SOAC trace helper runtime name {helper_name:?}"));
    let func: InstrCodegen = Load::new(ResolvedName {
        id: runtime_name.name().into(),
        location: NameLocation::RuntimeName(runtime_name),
    })
    .with_meta(meta.clone())
    .into();
    Call::new(
        func,
        args.into_iter()
            .map(CallArgPositional::Positional)
            .collect::<Vec<_>>(),
        Vec::new(),
    )
    .with_meta(meta)
    .into()
}

fn string_literal_expr(module_constants: &mut Vec<InstrResolved>, value: &str) -> InstrCodegen {
    let meta = Meta::synthetic();
    let index = u32::try_from(module_constants.len())
        .expect("trace module constant count should fit in u32");
    module_constants.push(
        LiteralValue::new(StringLiteral {
            value: value.to_string(),
        })
        .with_meta(meta.clone())
        .into(),
    );
    Load::new(ResolvedName {
        id: format!("__dp_constant_{index}").into(),
        location: NameLocation::Constant(index),
    })
    .with_meta(meta)
    .into()
}

fn tuple_expr(values: Vec<InstrCodegen>) -> InstrCodegen {
    Tuple::new(values).with_meta(Meta::synthetic()).into()
}

fn param_pairs_expr(
    module_constants: &mut Vec<InstrResolved>,
    locator: &PreparedTraceNameLocator,
    params: &[String],
) -> InstrCodegen {
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
