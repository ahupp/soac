use crate::block_py::HasSemanticInstrId;
use crate::block_py::{
    core_call_expr_with_meta, literal_expr, BlockPyFunction, BlockPyModule, BlockTerm,
    CallArgPositional, ChildVisitable, CounterScope, CounterSite, FunctionExecutionMode,
    IncrementCounter, InstrCodegen, InstrResolved, Load, Meta, NameLocation, ResolvedName,
    RuntimeName, StringLiteral, Tuple, Visit, WithMeta,
};
use crate::passes::{CodegenModuleShape, CounterBuilder};
use soac_config::{SoacEnvConfig, SpecializationMode};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceConfig {
    pub qualname_filter: Option<String>,
    pub include_params: bool,
}

pub fn parse_trace_env(config: &SoacEnvConfig) -> Option<TraceConfig> {
    parse_trace_config(config.soac_exec_trace()?)
}

pub fn call_target_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
    specialization_mode_instruments_top_values(config)
}

pub fn locality_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
    specialization_mode_instruments_top_values(config)
}

pub fn refcount_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
    config.specialization_mode() == Some(SpecializationMode::Verify)
}

pub fn deopt_entry_counter_instrumentation_enabled(config: &SoacEnvConfig) -> bool {
    if config
        .runtime_optimization_pipeline()
        .uses_identity_typed_runtime()
    {
        return false;
    }
    matches!(
        config.specialization_mode(),
        Some(SpecializationMode::Verify | SpecializationMode::Apply)
    )
}

pub fn specialization_runtime_logging_enabled(config: &SoacEnvConfig) -> bool {
    config.specialization_runtime_logging_enabled()
}

fn specialization_mode_instruments_top_values(config: &SoacEnvConfig) -> bool {
    config
        .specialization_mode()
        .is_some_and(SpecializationMode::records_counters)
}

fn functions_with_counter_instrumentation(
    functions: &[BlockPyFunction<CodegenModuleShape>],
) -> impl Iterator<Item = &BlockPyFunction<CodegenModuleShape>> {
    functions
        .iter()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
}

fn functions_with_counter_instrumentation_mut(
    functions: &mut [BlockPyFunction<CodegenModuleShape>],
) -> impl Iterator<Item = &mut BlockPyFunction<CodegenModuleShape>> {
    functions
        .iter_mut()
        .filter(|function| function.execution_mode() == FunctionExecutionMode::Jit)
}

pub(crate) fn parse_trace_config(raw: &str) -> Option<TraceConfig> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "0" {
        return None;
    }
    let (selector, include_params) = if let Some(stripped) = trimmed.strip_suffix(":params") {
        (stripped.trim(), true)
    } else {
        (trimmed, false)
    };
    let qualname_filter = match selector {
        "" | "1" | "*" | "all" => None,
        value => Some(value.to_string()),
    };
    Some(TraceConfig {
        qualname_filter,
        include_params,
    })
}

pub fn instrument_bb_module_for_trace(
    module: &mut BlockPyModule<CodegenModuleShape>,
    config: &TraceConfig,
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

pub fn instrument_bb_module_with_block_entry_counters(
    module: &mut BlockPyModule<CodegenModuleShape>,
) {
    let mut counters = CounterBuilder::new(&mut module.counter_defs);
    for function in functions_with_counter_instrumentation_mut(&mut module.callable_defs) {
        for block in &mut function.blocks {
            let counter_id = counters
                .define(
                    CounterScope::This,
                    "block_entry",
                    CounterSite::BlockEntry {
                        function_id: function.function_id,
                        block_label: block.label,
                    },
                )
                .id();
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
    match scope {
        CounterScope::This => {
            return Err(
                "refcount counters do not yet support CounterScope::This; use Function or Global"
                    .to_string(),
            );
        }
        CounterScope::Function => {
            let function_ids = functions_with_counter_instrumentation(&module.callable_defs)
                .map(|function| function.function_id)
                .collect::<Vec<_>>();
            for function_id in function_ids {
                for kind in ["runtime_incref", "runtime_decref"] {
                    counters.define_if_missing(
                        scope,
                        kind,
                        CounterSite::Runtime {
                            function_id: Some(function_id),
                            instr_id: None,
                        },
                    );
                }
            }
        }
        CounterScope::Global => {
            for kind in ["runtime_incref", "runtime_decref"] {
                counters.define_if_missing(
                    scope,
                    kind,
                    CounterSite::Runtime {
                        function_id: None,
                        instr_id: None,
                    },
                );
            }
        }
    }
    Ok(())
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
            InstrCodegen::BinOp(op) => matches!(
                op.kind,
                crate::block_py::BinOpKind::Add
                    | crate::block_py::BinOpKind::Sub
                    | crate::block_py::BinOpKind::Mul
                    | crate::block_py::BinOpKind::And
                    | crate::block_py::BinOpKind::Or
                    | crate::block_py::BinOpKind::Xor
                    | crate::block_py::BinOpKind::Eq
                    | crate::block_py::BinOpKind::Ne
                    | crate::block_py::BinOpKind::Lt
                    | crate::block_py::BinOpKind::Le
                    | crate::block_py::BinOpKind::Gt
                    | crate::block_py::BinOpKind::Ge
            ),
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

    fn define_instr_shape_counters(
        counters: &mut CounterBuilder<'_>,
        function_id: crate::block_py::RuntimeFunctionId,
        instr_id: crate::block_py::InstrId,
        shape_kind: &'static str,
        branch_kind: &'static str,
    ) {
        counters.define_if_missing(
            CounterScope::This,
            shape_kind,
            CounterSite::Runtime {
                function_id: Some(function_id),
                instr_id: Some(instr_id),
            },
        );
        counters.define_branch_counter_if_missing(
            CounterScope::This,
            branch_kind,
            CounterSite::Runtime {
                function_id: Some(function_id),
                instr_id: Some(instr_id),
            },
            ["hit", "fallback"],
        );
    }

    fn define_indexed_counter(
        counters: &mut CounterBuilder<'_>,
        function_id: crate::block_py::RuntimeFunctionId,
        instr_id: crate::block_py::InstrId,
        kind: &'static str,
    ) {
        counters.define_branch_counter_if_missing(
            CounterScope::This,
            kind,
            CounterSite::Runtime {
                function_id: Some(function_id),
                instr_id: Some(instr_id),
            },
            ["hit", "fallback"],
        );
    }

    fn define_field_access_counter(
        counters: &mut CounterBuilder<'_>,
        function_id: crate::block_py::RuntimeFunctionId,
        instr_id: crate::block_py::InstrId,
    ) {
        counters.define_branch_counter_if_missing(
            CounterScope::This,
            "field_access",
            CounterSite::Runtime {
                function_id: Some(function_id),
                instr_id: Some(instr_id),
            },
            [
                "indexed_hit",
                "indexed_fallback",
                "generic_getattr",
                "generic_setattr",
            ],
        );
    }

    struct SpecializationCandidateCounterCollector<'a, 'b> {
        function_id: crate::block_py::RuntimeFunctionId,
        counters: &'a mut CounterBuilder<'b>,
    }

    impl Visit<InstrCodegen> for SpecializationCandidateCounterCollector<'_, '_> {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            if is_global_index_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                define_indexed_counter(self.counters, self.function_id, instr_id, "global_indexed");
            }
            match expr {
                InstrCodegen::GetAttr(_) => {
                    let instr_id = expr.semantic_instr_id();
                    define_field_access_counter(self.counters, self.function_id, instr_id);
                }
                InstrCodegen::SetAttr(_) => {
                    let instr_id = expr.semantic_instr_id();
                    define_field_access_counter(self.counters, self.function_id, instr_id);
                }
                _ => {}
            }
            if is_operator_specialization_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                self.counters.define_if_missing(
                    CounterScope::This,
                    "operator_hot_shapes",
                    CounterSite::Runtime {
                        function_id: Some(self.function_id),
                        instr_id: Some(instr_id),
                    },
                );
            }
            if is_getitem_specialization_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                define_instr_shape_counters(
                    self.counters,
                    self.function_id,
                    instr_id,
                    "getitem_hot_shapes",
                    "getitem_specialized",
                );
            }
            if is_setitem_specialization_candidate(expr) {
                let instr_id = expr.semantic_instr_id();
                define_instr_shape_counters(
                    self.counters,
                    self.function_id,
                    instr_id,
                    "setitem_hot_shapes",
                    "setitem_specialized",
                );
            }
            if let InstrCodegen::Call(call) = expr {
                let is_candidate = call.keywords.is_empty()
                    && call
                        .args
                        .iter()
                        .all(|arg| matches!(arg, CallArgPositional::Positional(_)));
                if is_candidate {
                    let instr_id = expr.semantic_instr_id();
                    self.counters.define_if_missing(
                        CounterScope::This,
                        "call_hot_targets",
                        CounterSite::Runtime {
                            function_id: Some(self.function_id),
                            instr_id: Some(instr_id),
                        },
                    );
                    self.counters.define_branch_counter_if_missing(
                        CounterScope::This,
                        "call_direct",
                        CounterSite::Runtime {
                            function_id: Some(self.function_id),
                            instr_id: Some(instr_id),
                        },
                        ["hit", "fallback"],
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
            counters.define_if_missing(
                CounterScope::This,
                "branch_outcomes",
                CounterSite::Runtime {
                    function_id: Some(function.function_id),
                    instr_id: Some(instr_id),
                },
            );
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
    let func = Load::new(ResolvedName {
        id: runtime_name.name().into(),
        location: NameLocation::RuntimeName(runtime_name),
    })
    .with_meta(meta.clone())
    .into();
    core_call_expr_with_meta(
        func,
        meta.node_index,
        meta.range,
        args.into_iter()
            .map(CallArgPositional::Positional)
            .collect(),
        Vec::new(),
    )
}

fn string_literal_expr(module_constants: &mut Vec<InstrResolved>, value: &str) -> InstrCodegen {
    let meta = Meta::synthetic();
    let index = u32::try_from(module_constants.len())
        .expect("trace module constant count should fit in u32");
    module_constants.push(literal_expr(
        StringLiteral {
            value: value.to_string(),
        },
        meta.clone(),
    ));
    crate::block_py::Load::new(crate::block_py::ResolvedName {
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
