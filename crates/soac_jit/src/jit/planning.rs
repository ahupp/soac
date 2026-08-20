use super::deopt::{
    runtime_jit_deopt_continuation_for_point, typed_nested_guard_misses_can_resume_before_instr,
};
use super::{can_release_via_stack_slot_fallback, local_name_has_block_parameter_role};
#[cfg(test)]
use soac_config::SoacEnvConfig;
use soac_core::block_py::{
    BinOpKind, BlockArg, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm,
    ChildVisitable, ConstantExpr, HasSemanticInstrId, InstrId, InstrKey, InstrLocationMap, Literal,
    LocalLocation, NumberLiteralValue, RuntimeFunctionId, StorageLayout, Visit,
    current_instr_locations, is_internal_symbol, visit_operand_takes, visit_term_operand_takes,
};
use soac_ir_blockpy::BlockPyModuleShape;
use soac_ir_typed::emit_v3::{MechanicalExitKind, MechanicalStepOp};
use soac_ir_typed::plan_v3::{MaterializeKind, PlanValue, Rep};
use soac_ir_typed::{
    FactStore, InstrTyped, PyObjFacts, TypedBlock, TypedBlockPyModuleShape,
    TypedExactIntReturnPlan, ValueFacts,
};
use soac_opt::passes::{
    BlockLocalPlan, FunctionLocalEnvResumePlan, FunctionRefcountPlan, LocalEnvModulePlan,
    LocalEnvResumeEntry, LocalEnvResumeModulePlan, LocalEnvResumePoint,
    LocalEnvResumeStatePrecision, LocalEnvResumeValueSource, LocalRefState, RefcountActionKind,
    RefcountLocal, RefcountPlan, RefcountReleaseReason, RefcountSite,
    compute_typed_function_local_live_ins, compute_typed_function_local_must_bound_ins,
    compute_typed_module_precise_immortal_local_entry_states,
    plan_typed_local_env_module_with_precise_immortal_states, plan_typed_local_env_resume_module,
    plan_typed_ownership_effects_with_precise_immortal_states,
    validate_typed_local_env_module_plan_with_precise_immortal_states,
    validate_typed_local_env_resume_module_plan,
    validate_typed_ownership_effects_with_precise_immortal_states,
};
pub use soac_opt::passes::{
    BlockParamFacts, FunctionLocalPlan, LocalRefKind, ParamBindingFacts, ParamProvenance,
    PlannedLocalBinding, PlannedLocalStorage, render_planned_local_binding,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt::Write;
use std::time::{Duration, Instant};

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

#[derive(Clone, Debug)]
pub struct PreparedJitTypedModulePlan {
    pub module: BlockPyModule<TypedBlockPyModuleShape>,
    pub value_facts: FactStore,
    pub local_env_plan: LocalEnvModulePlan,
    pub locals: PlannedJitModuleLocals,
    pub deopt_resume: PlannedJitDeoptResumeModule,
}

fn can_use_cleanup_root(name: &str) -> bool {
    !name.starts_with("_dp_")
}

/// Consuming expression operands and semantic class cells require boxed
/// Python ownership, independently of whether a scalar fact is known.
fn boxed_owner_local_locations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> Result<HashSet<LocalLocation>, String> {
    struct Validate<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        result: Result<(), String>,
        operands: HashSet<LocalLocation>,
    }
    impl Visit<InstrTyped> for Validate<'_> {
        fn visit_instr(&mut self, instr: &InstrTyped) {
            if self.result.is_err() {
                return;
            }
            let operand = match instr {
                InstrTyped::TakeOperand(op) => Some(
                    self.function
                        .storage_layout()
                        .as_ref()
                        .ok_or_else(|| "operand take has no physical layout".to_owned())
                        .and_then(|layout| op.validate_resolved(layout)),
                ),
                InstrTyped::ComprehensionInsert(op) => Some(
                    self.function
                        .storage_layout()
                        .as_ref()
                        .ok_or_else(|| "comprehension insertion has no physical layout".to_owned())
                        .and_then(|layout| op.validate_resolved(layout)),
                ),
                InstrTyped::IteratorStep(op) => Some(
                    self.function
                        .storage_layout()
                        .as_ref()
                        .ok_or_else(|| "iterator step has no physical layout".to_owned())
                        .and_then(|layout| op.validate_resolved(layout)),
                ),
                _ => None,
            };
            if let Some(operand) = operand {
                match operand {
                    Ok(location) => {
                        if let Some(location) = location.local_location() {
                            self.operands.insert(location);
                        }
                    }
                    Err(error) => {
                        self.result = Err(error);
                    }
                }
            }
            match instr {
                InstrTyped::BuildCollection(op) => self.result = op.validate_shape(),
                InstrTyped::CallArgumentOp(op) => {
                    self.result = self
                        .function
                        .storage_layout()
                        .as_ref()
                        .ok_or_else(|| "call-argument phase has no physical layout".to_owned())
                        .and_then(|layout| op.validate_resolved(layout))
                        .map(|(callable, buffer)| {
                            self.operands.extend(
                                [callable, buffer]
                                    .into_iter()
                                    .filter_map(|slot| slot.local_location()),
                            );
                        });
                }
                InstrTyped::PreparedCall(op) => {
                    self.result = self
                        .function
                        .storage_layout()
                        .as_ref()
                        .ok_or_else(|| "prepared call has no physical layout".to_owned())
                        .and_then(|layout| op.validate_resolved(layout));
                }
                _ => {}
            }
            instr.visit_children(self);
        }
    }
    let mut validator = Validate {
        function,
        result: Ok(()),
        operands: HashSet::new(),
    };
    validator.visit_fn(function);
    validator.result?;
    let mut owners = validator.operands;
    let Some(scope) = &function.scope.class_bindings else {
        if function
            .storage_layout()
            .as_ref()
            .is_some_and(|layout| layout.class_bindings.is_some())
        {
            return Err("class slot projection has no source recipe".into());
        }
        return Ok(owners);
    };
    let layout = function
        .storage_layout()
        .as_ref()
        .ok_or("class activation has no layout")?;
    let projection = layout
        .class_bindings
        .as_ref()
        .ok_or("class activation has no slot projection")?;
    projection.validate(scope, layout, &function.scope)?;
    for slot in &projection.slots {
        owners.insert(
            slot.storage
                .raw_local(layout)
                .ok_or("class slot has no raw owner")?,
        );
    }
    owners.insert(projection.namespace);
    Ok(owners)
}

fn typed_block_indices_by_label(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, usize> {
    function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect()
}

fn typed_block_index_for_label(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    label: BlockLabel,
) -> usize {
    *block_indices_by_label.get(&label).unwrap_or_else(|| {
        panic!(
            "function {} ({}) references unknown block label {}",
            function.function_id, function.names.qualname, label
        )
    })
}

fn planned_loop_backedges_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
) -> Result<HashSet<(BlockLabel, BlockLabel)>, String> {
    const UNVISITED: u8 = 0;
    const ACTIVE: u8 = 1;
    const FINISHED: u8 = 2;

    if function.blocks.is_empty() {
        return Ok(HashSet::new());
    }

    let successors = function
        .blocks
        .iter()
        .map(|block| match &block.term {
            BlockTerm::Jump(edge) => vec![edge.target],
            BlockTerm::IfTerm(if_term) => vec![if_term.then_label, if_term.else_label],
            BlockTerm::BranchTable(branch) => branch
                .targets
                .iter()
                .copied()
                .chain(std::iter::once(branch.default_label))
                .collect(),
            BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {
                Vec::new()
            }
        })
        .collect::<Vec<_>>();
    let mut remaining_roots = (1..function.blocks.len()).collect::<Vec<_>>();
    remaining_roots.sort_unstable_by_key(|index| function.blocks[*index].label);
    let root_indices = std::iter::once(0)
        .chain(remaining_roots)
        .collect::<Vec<_>>();
    let mut visit_states = vec![UNVISITED; function.blocks.len()];
    let mut backedges = HashSet::new();

    for root_index in root_indices.iter().copied() {
        if visit_states[root_index] != UNVISITED {
            continue;
        }

        visit_states[root_index] = ACTIVE;
        let mut pending = vec![(root_index, 0usize)];
        while let Some((block_index, next_successor_index)) = pending.last_mut() {
            let source_index = *block_index;
            if *next_successor_index == successors[source_index].len() {
                visit_states[source_index] = FINISHED;
                pending.pop();
                continue;
            }

            let target_label = successors[source_index][*next_successor_index];
            *next_successor_index += 1;
            let target_index =
                typed_block_index_for_label(function, block_indices_by_label, target_label);
            match visit_states[target_index] {
                UNVISITED => {
                    visit_states[target_index] = ACTIVE;
                    pending.push((target_index, 0));
                }
                ACTIVE => {
                    backedges.insert((function.blocks[source_index].label, target_label));
                }
                FINISHED => {}
                _ => unreachable!("invalid CFG traversal state"),
            }
        }
    }

    if function.blocks.iter().any(|block| block.exc_edge.is_some()) {
        // Exception dispatches cannot host polls, but they can close cycles.
        // Remove one pollable edge from each remaining full-CFG cycle until
        // the unpolled graph is acyclic, including overlapping handler paths.
        while let Some(edge) = next_unpolled_cfg_cycle_normal_edge(
            function,
            block_indices_by_label,
            &successors,
            &root_indices,
            &backedges,
        )? {
            backedges.insert(edge);
        }
    }

    Ok(backedges)
}

fn next_unpolled_cfg_cycle_normal_edge(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    normal_successors: &[Vec<BlockLabel>],
    root_indices: &[usize],
    selected_normal_edges: &HashSet<(BlockLabel, BlockLabel)>,
) -> Result<Option<(BlockLabel, BlockLabel)>, String> {
    const UNVISITED: u8 = 0;
    const ACTIVE: u8 = 1;
    const FINISHED: u8 = 2;

    let mut visit_states = vec![UNVISITED; function.blocks.len()];
    let mut incoming_edges = vec![None; function.blocks.len()];

    for root_index in root_indices.iter().copied() {
        if visit_states[root_index] != UNVISITED {
            continue;
        }

        visit_states[root_index] = ACTIVE;
        let mut pending = vec![(root_index, 0usize)];
        while let Some((block_index, next_successor_index)) = pending.last_mut() {
            let source_index = *block_index;
            let normal_count = normal_successors[source_index].len();
            let exceptional_target = function.blocks[source_index]
                .exc_edge
                .as_ref()
                .map(|edge| edge.target);
            let successor_count = normal_count + usize::from(exceptional_target.is_some());
            if *next_successor_index == successor_count {
                visit_states[source_index] = FINISHED;
                pending.pop();
                continue;
            }

            let successor_index = *next_successor_index;
            *next_successor_index += 1;
            let (target_label, is_exceptional) = if successor_index < normal_count {
                (normal_successors[source_index][successor_index], false)
            } else {
                (
                    exceptional_target.expect("exceptional successor should exist"),
                    true,
                )
            };
            let source_label = function.blocks[source_index].label;
            if !is_exceptional && selected_normal_edges.contains(&(source_label, target_label)) {
                continue;
            }

            let target_index =
                typed_block_index_for_label(function, block_indices_by_label, target_label);
            match visit_states[target_index] {
                UNVISITED => {
                    visit_states[target_index] = ACTIVE;
                    incoming_edges[target_index] =
                        Some((source_label, target_label, is_exceptional));
                    pending.push((target_index, 0));
                }
                ACTIVE if !is_exceptional => return Ok(Some((source_label, target_label))),
                ACTIVE => {
                    for (active_index, _) in pending.iter().rev() {
                        if *active_index == target_index {
                            break;
                        }
                        if let Some((parent_label, child_label, false)) =
                            incoming_edges[*active_index]
                        {
                            return Ok(Some((parent_label, child_label)));
                        }
                    }
                    return Err(format!(
                        "function {} ({}) has an exception-only CFG cycle with no safe pending-event poll edge",
                        function.function_id, function.names.qualname
                    ));
                }
                FINISHED => {}
                _ => unreachable!("invalid CFG traversal state"),
            }
        }
    }

    Ok(None)
}

#[derive(Clone, Debug)]
pub struct BlockExcDispatchPlan {
    pub target_index: usize,
    pub slot_writes: Vec<(String, BlockArg)>,
    pub target_args: Vec<RuntimeBlockArgPlan>,
    /// Per-target demand, in the same order as `target_args`. Only Owned and
    /// Borrowed occur here; a borrowed destination never consumes a new owner.
    pub target_arg_ref_kinds: Vec<LocalRefKind>,
    pub forwarded_local_names: Vec<String>,
    /// These inputs serve only borrowed targets. Every other forwarded input
    /// carries one owner to a target, release, or drop sink.
    pub borrowed_forwarded_local_names: HashSet<String>,
    pub release_local_names: Vec<String>,
    pub drop_forwarded_local_names: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct EdgeTransportPlan {
    pub slot_writes: Vec<(String, BlockArg)>,
    pub target_args: Vec<RuntimeBlockArgPlan>,
    pub forwarded_local_names: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeBlockParamRepr {
    PyObject,
    ExactI64,
    I32Bool01,
}

#[derive(Clone, Debug)]
pub struct RuntimeBlockArgPlan {
    pub target_name: String,
    pub source: BlockArg,
    pub repr: RuntimeBlockParamRepr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeBlockParamPlan {
    pub arg_name: String,
    pub binding: PlannedLocalBinding,
    pub entry_aliases: Vec<String>,
    pub repr: RuntimeBlockParamRepr,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PlannedRuntimeLocalReprs {
    block_entry_reprs: Vec<HashMap<LocalLocation, RuntimeBlockParamRepr>>,
    block_param_reprs: Vec<Vec<RuntimeBlockParamRepr>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedStackSlotEntrySeed {
    pub binding: PlannedLocalBinding,
    pub entry_ref_kind: LocalRefKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedLocalEnvEntrySource {
    BlockParam {
        param_index: usize,
    },
    StackSlotLoad,
    /// The validated source block has no incoming value for this location.
    /// It still needs an explicit null before a raising producer so that a
    /// successor's nullable ownership obligation has a concrete operand.
    Unbound,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedLocalEnvEntryMaterialization {
    pub binding: PlannedLocalBinding,
    pub entry_aliases: Vec<String>,
    pub source: PlannedLocalEnvEntrySource,
    pub entry_ref_kind: LocalRefKind,
    pub repr: RuntimeBlockParamRepr,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CleanupRootSlotState {
    NoOwnedReference,
    MaybeOwnedReference,
}

impl CleanupRootSlotState {
    pub const fn may_hold_owned_reference(self) -> bool {
        matches!(self, Self::MaybeOwnedReference)
    }
}

/// Successful evaluation and a failure at an earlier evaluation prefix have
/// different physical owners. In particular, a nested TakeOperand may not
/// have executed when a preceding key/call fails.
struct CleanupRootBlockTransfer<T> {
    normal: T,
    exceptional: T,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlannedCleanupRootSlotStates {
    pub block_entry_states: HashMap<BlockLabel, HashMap<String, CleanupRootSlotState>>,
    pub block_exit_states: HashMap<BlockLabel, HashMap<String, CleanupRootSlotState>>,
    pub block_exception_states: HashMap<BlockLabel, HashMap<String, CleanupRootSlotState>>,
    pub instr_previous_states: HashMap<InstrKey, HashMap<String, CleanupRootSlotState>>,
    pub block_exit_facts: HashMap<BlockLabel, HashMap<String, PyObjFacts>>,
    pub block_exception_facts: HashMap<BlockLabel, HashMap<String, PyObjFacts>>,
    pub instr_previous_facts: HashMap<InstrKey, HashMap<String, PyObjFacts>>,
}

impl PlannedCleanupRootSlotStates {
    pub fn previous_state_for_instr(
        &self,
        instr_key: InstrKey,
        name: &str,
    ) -> CleanupRootSlotState {
        self.instr_previous_states
            .get(&instr_key)
            .and_then(|states| states.get(name))
            .copied()
            .unwrap_or(CleanupRootSlotState::MaybeOwnedReference)
    }

    pub fn entry_state_for_block(
        &self,
        label: BlockLabel,
    ) -> HashMap<String, CleanupRootSlotState> {
        self.block_entry_states
            .get(&label)
            .cloned()
            .unwrap_or_default()
    }

    pub fn exit_state_for_block(&self, label: BlockLabel) -> HashMap<String, CleanupRootSlotState> {
        self.block_exit_states
            .get(&label)
            .cloned()
            .unwrap_or_default()
    }

    pub fn union_exit_states(&self) -> HashMap<String, CleanupRootSlotState> {
        let mut union = HashMap::new();
        let mut initialized = false;
        // The common terminal-error sweep is reachable from an incomplete
        // operation too, not only from a block's successful terminator.
        for states in self
            .block_exit_states
            .values()
            .chain(self.block_exception_states.values())
        {
            if !initialized {
                union = states.clone();
                initialized = true;
                continue;
            }
            merge_cleanup_root_slot_state_maps(&mut union, states);
        }
        union
    }

    pub fn previous_facts_for_instr(&self, instr_key: InstrKey, name: &str) -> Option<PyObjFacts> {
        self.instr_previous_facts
            .get(&instr_key)
            .and_then(|facts| facts.get(name))
            .copied()
    }

    pub fn exit_facts_for_block(&self, label: BlockLabel) -> HashMap<String, PyObjFacts> {
        self.block_exit_facts
            .get(&label)
            .cloned()
            .unwrap_or_default()
    }

    pub fn union_exit_facts(&self) -> HashMap<String, PyObjFacts> {
        let mut union = HashMap::new();
        let mut initialized = false;
        for facts in self
            .block_exit_facts
            .values()
            .chain(self.block_exception_facts.values())
        {
            if !initialized {
                union = facts.clone();
                initialized = true;
                continue;
            }
            retain_common_cleanup_root_slot_facts(&mut union, facts);
        }
        union
    }
}

#[derive(Clone, Debug)]
pub struct PlannedJitFunctionLocals {
    pub local_plan: FunctionLocalPlan,
    pub refcount_plan: FunctionRefcountPlan,
    pub loop_backedges: HashSet<(BlockLabel, BlockLabel)>,
    pub cleanup_root_names: HashSet<String>,
    pub cleanup_root_slot_states: PlannedCleanupRootSlotStates,
    pub truthiness_only_local_locations: HashSet<LocalLocation>,
    /// Consuming expression operands, semantic class cells and the class
    /// namespace require boxed owning storage even when scalar facts are known.
    pub boxed_owner_local_locations: HashSet<LocalLocation>,
    pub runtime_block_params: Vec<Vec<RuntimeBlockParamPlan>>,
    pub implicit_target_transports: Vec<EdgeTransportPlan>,
    pub jump_edge_transports: Vec<Option<EdgeTransportPlan>>,
    pub stack_slot_entry_seeds: Vec<Vec<PlannedStackSlotEntrySeed>>,
    pub entry_materializations: Vec<Vec<PlannedLocalEnvEntryMaterialization>>,
    pub exc_dispatches: Vec<Option<BlockExcDispatchPlan>>,
}

#[derive(Clone, Debug, Default)]
pub struct PlannedJitModuleLocals {
    pub functions: HashMap<RuntimeFunctionId, PlannedJitFunctionLocals>,
}

#[derive(Clone, Debug)]
pub struct PlannedJitDeoptResumeFunction {
    pub resume_plan: FunctionLocalEnvResumePlan,
    pub deopt_points: Vec<PlannedJitDeoptPoint>,
}

#[derive(Clone, Debug, Default)]
pub struct PlannedJitDeoptResumeModule {
    pub functions: HashMap<RuntimeFunctionId, PlannedJitDeoptResumeFunction>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PlannedJitDeoptPointId {
    pub function_id: RuntimeFunctionId,
    pub ordinal: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedJitDeoptPoint {
    pub id: PlannedJitDeoptPointId,
    pub point: LocalEnvResumePoint,
    pub resume_point: LocalEnvResumePoint,
    pub precision: LocalEnvResumeStatePrecision,
}

impl PlannedJitModuleLocals {
    pub fn function(&self, function_id: RuntimeFunctionId) -> Option<&PlannedJitFunctionLocals> {
        self.functions.get(&function_id)
    }

    pub fn validate_for_typed_module(
        &self,
        module: &BlockPyModule<TypedBlockPyModuleShape>,
    ) -> Result<(), String> {
        let expected_function_ids = module
            .callable_defs
            .iter()
            .map(|function| function.function_id)
            .collect::<HashSet<_>>();
        for function in &module.callable_defs {
            let function_plan = self.function(function.function_id).ok_or_else(|| {
                format!(
                    "missing JIT local plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
            function_plan.validate_for_typed_function(function)?;
        }
        for function_id in self.functions.keys() {
            if !expected_function_ids.contains(function_id) {
                return Err(format!(
                    "JIT local plan contains unknown function id {function_id}"
                ));
            }
        }
        Ok(())
    }
}

impl PlannedJitDeoptResumeFunction {
    pub fn entry(&self, point: LocalEnvResumePoint) -> Option<&LocalEnvResumeEntry> {
        self.resume_plan.entry(point)
    }

    pub fn deopt_point(&self, point: LocalEnvResumePoint) -> Option<&PlannedJitDeoptPoint> {
        self.deopt_points
            .iter()
            .find(|deopt_point| deopt_point.point == point)
    }

    pub fn deopt_point_by_id(&self, id: PlannedJitDeoptPointId) -> Option<&PlannedJitDeoptPoint> {
        self.deopt_points
            .iter()
            .find(|deopt_point| deopt_point.id == id)
    }

    pub fn deopt_points_for_block(
        &self,
        block: BlockLabel,
        instr_locations: &InstrLocationMap,
    ) -> impl Iterator<Item = &PlannedJitDeoptPoint> {
        self.deopt_points
            .iter()
            .filter(move |point| point.point.current_block_label(instr_locations) == Some(block))
    }

    pub fn validate_for_typed_function(
        &self,
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
    ) -> Result<(), String> {
        let mut errors = Vec::new();
        let instr_locations = current_instr_locations(function);
        if self.deopt_points.len() != self.resume_plan.entries.len() {
            errors.push(format!(
                "JIT deopt resume plan for function {} ({}) has {} deopt points but {} \
                 LocalEnv resume entries",
                function.function_id,
                function.names.qualname,
                self.deopt_points.len(),
                self.resume_plan.entries.len()
            ));
        }

        for (ordinal, entry) in self.resume_plan.entries.iter().enumerate() {
            let Some(deopt_point) = self.deopt_points.get(ordinal) else {
                errors.push(format!(
                    "JIT deopt resume plan for function {} ({}) is missing deopt point {ordinal}",
                    function.function_id, function.names.qualname
                ));
                continue;
            };
            if deopt_point.id.function_id != function.function_id {
                errors.push(format!(
                    "JIT deopt point {ordinal} for function {} ({}) has wrong function id {}",
                    function.function_id, function.names.qualname, deopt_point.id.function_id
                ));
            }
            if deopt_point.id.ordinal != ordinal {
                errors.push(format!(
                    "JIT deopt point for function {} ({}) at index {ordinal} has id ordinal {}",
                    function.function_id, function.names.qualname, deopt_point.id.ordinal
                ));
            }
            if deopt_point.point != entry.point || deopt_point.resume_point != entry.point {
                errors.push(format!(
                    "JIT deopt point {:?} for function {} ({}) does not map exactly to LocalEnv \
                     resume point {:?}",
                    deopt_point.point, function.function_id, function.names.qualname, entry.point
                ));
            }
            if deopt_point
                .point
                .current_block_label(&instr_locations)
                .is_none()
            {
                errors.push(format!(
                    "JIT deopt point {:?} for function {} ({}) does not resolve to a current block",
                    deopt_point.point, function.function_id, function.names.qualname
                ));
            }
            if deopt_point.precision != entry.precision {
                errors.push(format!(
                    "JIT deopt point {:?} for function {} ({}) has precision {:?}, expected {:?}",
                    deopt_point.point,
                    function.function_id,
                    function.names.qualname,
                    deopt_point.precision,
                    entry.precision
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("\n"))
        }
    }
}

impl PlannedJitDeoptResumeModule {
    pub fn function(
        &self,
        function_id: RuntimeFunctionId,
    ) -> Option<&PlannedJitDeoptResumeFunction> {
        self.functions.get(&function_id)
    }

    pub fn entry(&self, point: LocalEnvResumePoint) -> Option<&LocalEnvResumeEntry> {
        self.function(point.function_id())
            .and_then(|function| function.entry(point))
    }

    pub fn deopt_point(&self, point: LocalEnvResumePoint) -> Option<&PlannedJitDeoptPoint> {
        self.function(point.function_id())
            .and_then(|function| function.deopt_point(point))
    }

    pub fn validate_for_typed_module(
        &self,
        module: &BlockPyModule<TypedBlockPyModuleShape>,
    ) -> Result<(), String> {
        let expected_function_ids = module
            .callable_defs
            .iter()
            .map(|function| function.function_id)
            .collect::<HashSet<_>>();
        for function in &module.callable_defs {
            let function_plan = self.function(function.function_id).ok_or_else(|| {
                format!(
                    "missing JIT deopt resume plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
            function_plan.validate_for_typed_function(function)?;
            for block in &function.blocks {
                if function_plan
                    .resume_plan
                    .block_entry(function.function_id, block.label)
                    .is_none()
                    || function_plan
                        .resume_plan
                        .before_term(function.function_id, block.label)
                        .is_none()
                {
                    return Err(format!(
                        "JIT deopt resume plan for function {} ({}) is missing block boundary \
                         entries for block {}",
                        function.function_id, function.names.qualname, block.label
                    ));
                }
            }
        }
        for function_id in self.functions.keys() {
            if !expected_function_ids.contains(function_id) {
                return Err(format!(
                    "JIT deopt resume plan contains unknown function id {function_id}"
                ));
            }
        }
        Ok(())
    }
}

impl PlannedJitFunctionLocals {
    pub fn is_cleanup_root_name(&self, name: &str) -> bool {
        self.cleanup_root_names.contains(name)
    }

    pub fn required_stack_slot_names_for_function(
        &self,
        function: &BlockPyFunction<impl soac_core::block_py::ModuleShape>,
    ) -> Vec<String> {
        required_stack_slot_names_for_function_parts(
            function,
            &self.runtime_block_params,
            &self.stack_slot_entry_seeds,
            &self.exc_dispatches,
            &self.cleanup_root_names,
            &self.refcount_plan,
        )
    }

    pub fn validate_for_typed_function(
        &self,
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
    ) -> Result<(), String> {
        let expected_boxed_owners = boxed_owner_local_locations(function)?;
        if self.boxed_owner_local_locations != expected_boxed_owners {
            return Err(format!(
                "source owner representation plan for function {} disagrees with its projection",
                function.function_id
            ));
        }
        let block_count = function.blocks.len();
        let block_indices_by_label = typed_block_indices_by_label(function);
        if self.runtime_block_params.len() != block_count
            || self.implicit_target_transports.len() != block_count
            || self.jump_edge_transports.len() != block_count
            || self.stack_slot_entry_seeds.len() != block_count
            || self.entry_materializations.len() != block_count
            || self.exc_dispatches.len() != block_count
            || self.cleanup_root_slot_states.block_entry_states.len() != block_count
            || self.cleanup_root_slot_states.block_exit_states.len() != block_count
            || self.cleanup_root_slot_states.block_exception_states.len() != block_count
            || self.cleanup_root_slot_states.block_exception_facts.len() != block_count
        {
            return Err(format!(
                "planned JIT local state for function {} ({}) has inconsistent block counts",
                function.function_id, function.names.qualname
            ));
        }
        if self.loop_backedges
            != planned_loop_backedges_for_typed_function(function, &block_indices_by_label)?
        {
            return Err(format!(
                "planned JIT loop backedges for function {} ({}) do not match its control-flow graph",
                function.function_id, function.names.qualname
            ));
        }

        for (index, block) in function.blocks.iter().enumerate() {
            let block_plan = self.local_plan.block(block.label);
            if block_plan.is_none()
                && (!self.runtime_block_params[index].is_empty()
                    || !self.stack_slot_entry_seeds[index].is_empty()
                    || !self.entry_materializations[index].is_empty())
            {
                return Err(format!(
                    "planned JIT local state for function {} ({}) is missing block {}",
                    function.function_id, function.names.qualname, block.label
                ));
            }
            if let Some(block_plan) = block_plan {
                for param in &self.runtime_block_params[index] {
                    if self
                        .boxed_owner_local_locations
                        .contains(&param.binding.location)
                        && param.repr != RuntimeBlockParamRepr::PyObject
                    {
                        return Err(format!(
                            "source owner {:?} in function {} block {} must remain PyObject",
                            param.binding.location, function.function_id, block.label
                        ));
                    }
                    if block_plan
                        .binding_for_name(param.arg_name.as_str())
                        .is_none()
                    {
                        return Err(format!(
                            "runtime block param {:?} for function {} ({}) block {} has no local binding",
                            param.arg_name,
                            function.function_id,
                            function.names.qualname,
                            block.label
                        ));
                    }
                    if param.binding.storage != PlannedLocalStorage::BlockParam {
                        return Err(format!(
                            "runtime block param {:?} for function {} ({}) block {} is not block-param backed",
                            param.arg_name,
                            function.function_id,
                            function.names.qualname,
                            block.label
                        ));
                    }
                }
            }
            if !self
                .cleanup_root_slot_states
                .block_entry_states
                .contains_key(&block.label)
                || !self
                    .cleanup_root_slot_states
                    .block_exit_states
                    .contains_key(&block.label)
            {
                return Err(format!(
                    "cleanup-root slot state for function {} ({}) is missing block {}",
                    function.function_id, function.names.qualname, block.label
                ));
            }
            for seed in &self.stack_slot_entry_seeds[index] {
                if seed.binding.storage != PlannedLocalStorage::StackSlot {
                    return Err(format!(
                        "stack-slot entry seed {:?} for function {} ({}) block {} is not stack-slot backed",
                        seed.binding.name,
                        function.function_id,
                        function.names.qualname,
                        block.label
                    ));
                }
            }
            for entry in &self.entry_materializations[index] {
                if self
                    .boxed_owner_local_locations
                    .contains(&entry.binding.location)
                    && entry.repr != RuntimeBlockParamRepr::PyObject
                {
                    return Err(format!(
                        "source owner {:?} in function {} block {} has scalar entry materialization",
                        entry.binding.location, function.function_id, block.label
                    ));
                }
            }
            validate_entry_materializations_for_block(
                function,
                block.label,
                index,
                block_plan,
                &self.runtime_block_params[index],
                &self.stack_slot_entry_seeds[index],
                &self.entry_materializations[index],
                &self.cleanup_root_names,
            )?;

            let expected_jump = matches!(block.term, BlockTerm::Jump(_));
            if self.jump_edge_transports[index].is_some() != expected_jump {
                return Err(format!(
                    "jump edge transport presence mismatch for function {} ({}) block {}",
                    function.function_id, function.names.qualname, block.label
                ));
            }
            if self.exc_dispatches[index].is_some() != block.exc_edge.is_some() {
                return Err(format!(
                    "exception dispatch presence mismatch for function {} ({}) block {}",
                    function.function_id, function.names.qualname, block.label
                ));
            }
            if let Some(dispatch) = &self.exc_dispatches[index] {
                let Some(exc_edge) = block.exc_edge.as_ref() else {
                    unreachable!("presence checked above");
                };
                let expected_target_index =
                    typed_block_index_for_label(function, &block_indices_by_label, exc_edge.target);
                if dispatch.target_index != expected_target_index {
                    return Err(format!(
                        "exception dispatch target mismatch for function {} ({}) block {}",
                        function.function_id, function.names.qualname, block.label
                    ));
                }
                if dispatch.target_args.len()
                    != self.runtime_block_params[dispatch.target_index].len()
                {
                    return Err(format!(
                        "exception dispatch target arg count mismatch for function {} ({}) block {}",
                        function.function_id, function.names.qualname, block.label
                    ));
                }
                for release_name in &dispatch.release_local_names {
                    if !dispatch
                        .forwarded_local_names
                        .iter()
                        .any(|name| name == release_name)
                    {
                        return Err(format!(
                            "exception dispatch release local {:?} for function {} ({}) block {} is not forwarded",
                            release_name,
                            function.function_id,
                            function.names.qualname,
                            block.label
                        ));
                    }
                }
                validate_exception_dispatch_ownership_sinks(
                    function,
                    block.label,
                    dispatch,
                    &self.runtime_block_params[dispatch.target_index],
                    &self.cleanup_root_names,
                )?;
            }
        }

        Ok(())
    }
}

fn required_stack_slot_names_for_function_parts(
    function: &BlockPyFunction<impl soac_core::block_py::ModuleShape>,
    runtime_block_params: &[Vec<RuntimeBlockParamPlan>],
    stack_slot_entry_seeds: &[Vec<PlannedStackSlotEntrySeed>],
    exc_dispatches: &[Option<BlockExcDispatchPlan>],
    cleanup_root_names: &HashSet<String>,
    refcount_plan: &FunctionRefcountPlan,
) -> Vec<String> {
    let mut required = HashSet::new();

    for params in runtime_block_params {
        for param in params {
            if param.binding.storage == PlannedLocalStorage::StackSlot {
                required.insert(param.binding.name.clone());
            }
        }
    }

    for seeds in stack_slot_entry_seeds {
        for seed in seeds {
            required.insert(seed.binding.name.clone());
        }
    }

    for dispatch in exc_dispatches.iter().flatten() {
        for (target_name, _) in &dispatch.slot_writes {
            required.insert(target_name.clone());
        }
        for source_name in &dispatch.forwarded_local_names {
            required.insert(source_name.clone());
        }
    }

    required.extend(cleanup_root_names.iter().cloned());
    for block_plan in refcount_plan.blocks.values() {
        for action in &block_plan.actions {
            let RefcountActionKind::ReleaseLocal { local, reason, .. } = &action.kind else {
                continue;
            };
            if !can_release_via_stack_slot_fallback(
                function.storage_layout().as_ref(),
                local.location,
            ) {
                continue;
            }
            match reason {
                RefcountReleaseReason::Return | RefcountReleaseReason::Raise => {}
                RefcountReleaseReason::Jump { .. }
                | RefcountReleaseReason::IfThen { .. }
                | RefcountReleaseReason::IfElse { .. }
                | RefcountReleaseReason::BranchCase { .. }
                | RefcountReleaseReason::BranchDefault { .. }
                | RefcountReleaseReason::ExceptionEdge { .. } => {
                    required.insert(local.name.clone());
                }
            }
        }
    }

    for block in &function.blocks {
        if let Some(exception_name) = block.exception_param() {
            required.insert(exception_name.to_string());
        }
    }

    function
        .storage_layout()
        .as_ref()
        .map(|layout| {
            layout
                .stack_slots()
                .iter()
                .filter(|name| required.contains(*name))
                .cloned()
                .collect()
        })
        .unwrap_or_default()
}

fn validate_exception_dispatch_ownership_sinks<P: soac_core::block_py::ModuleShape>(
    function: &BlockPyFunction<P>,
    block_label: BlockLabel,
    dispatch: &BlockExcDispatchPlan,
    runtime_target_params: &[RuntimeBlockParamPlan],
    cleanup_root_names: &HashSet<String>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    if dispatch.target_args.len() != dispatch.target_arg_ref_kinds.len()
        || dispatch.target_args.len() != runtime_target_params.len()
    {
        return Err(format!(
            "exception dispatch for function {} ({}) block {} has inconsistent target ownership counts",
            function.function_id, function.names.qualname, block_label,
        ));
    }
    for ((arg, &kind), param) in dispatch
        .target_args
        .iter()
        .zip(&dispatch.target_arg_ref_kinds)
        .zip(runtime_target_params)
    {
        if arg.target_name != param.arg_name
            || arg.repr != RuntimeBlockParamRepr::PyObject
            || arg.repr != param.repr
            || kind != planned_exception_target_ref_kind(param, cleanup_root_names)
        {
            errors.push(format!(
                "exception dispatch for function {} ({}) block {} target {:?} ownership {:?} disagrees with its runtime binding",
                function.function_id, function.names.qualname, block_label, arg.target_name, kind,
            ));
        }
    }
    let expected_borrowed = planned_borrowed_exception_forwarded_local_names(
        &dispatch.forwarded_local_names,
        &dispatch.target_args,
        &dispatch.target_arg_ref_kinds,
        &dispatch.slot_writes,
        &dispatch.release_local_names,
    );
    if dispatch.borrowed_forwarded_local_names != expected_borrowed {
        errors.push(format!(
            "exception dispatch for function {} ({}) block {} borrowed forwarded inputs disagree with target demands: expected {:?}, got {:?}",
            function.function_id, function.names.qualname, block_label,
            expected_borrowed, dispatch.borrowed_forwarded_local_names,
        ));
    }
    let forwarded_names = dispatch
        .forwarded_local_names
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if forwarded_names.len() != dispatch.forwarded_local_names.len() {
        errors.push(format!(
            "exception dispatch for function {} ({}) block {} has duplicate forwarded locals: {:?}",
            function.function_id,
            function.names.qualname,
            block_label,
            dispatch.forwarded_local_names
        ));
    }
    let target_source_names = runtime_block_arg_sources(&dispatch.target_args);
    let owned_target_source_names = runtime_block_arg_sources_with_ref_kind(
        &dispatch.target_args,
        &dispatch.target_arg_ref_kinds,
        LocalRefKind::Owned,
    );
    let slot_write_source_names = named_block_arg_sources(&dispatch.slot_writes);
    let release_names = dispatch
        .release_local_names
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let drop_names = dispatch
        .drop_forwarded_local_names
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    if release_names.len() != dispatch.release_local_names.len() {
        errors.push(format!(
            "exception dispatch for function {} ({}) block {} has duplicate release locals: {:?}",
            function.function_id,
            function.names.qualname,
            block_label,
            dispatch.release_local_names
        ));
    }
    if drop_names.len() != dispatch.drop_forwarded_local_names.len() {
        errors.push(format!(
            "exception dispatch for function {} ({}) block {} has duplicate drop locals: {:?}",
            function.function_id,
            function.names.qualname,
            block_label,
            dispatch.drop_forwarded_local_names
        ));
    }

    for name in target_source_names
        .iter()
        .chain(slot_write_source_names.iter())
    {
        if !forwarded_names.contains(name) {
            errors.push(format!(
                "exception dispatch for function {} ({}) block {} uses forwarded source {:?} \
                 without forwarding it",
                function.function_id, function.names.qualname, block_label, name
            ));
        }
    }
    for name in release_names.iter().chain(drop_names.iter()) {
        if !forwarded_names.contains(name) {
            errors.push(format!(
                "exception dispatch for function {} ({}) block {} has ownership sink {:?} \
                 without forwarding it",
                function.function_id, function.names.qualname, block_label, name
            ));
        }
    }

    for name in &dispatch.forwarded_local_names {
        let name = name.as_str();
        let mut sinks = Vec::new();
        if owned_target_source_names.contains(name) {
            sinks.push("target");
        }
        if release_names.contains(name) {
            sinks.push("release");
        }
        if drop_names.contains(name) {
            sinks.push("drop");
        }
        let borrowed = dispatch.borrowed_forwarded_local_names.contains(name);
        if borrowed && !sinks.is_empty() {
            errors.push(format!(
                "exception dispatch for function {} ({}) block {} borrowed forwarded local {:?} has ownership sinks {:?}",
                function.function_id, function.names.qualname, block_label, name, sinks,
            ));
        } else if !borrowed && sinks.len() != 1 {
            errors.push(format!(
                "exception dispatch for function {} ({}) block {} forwarded local {:?} has {} \
                 ownership sinks {:?}; expected exactly one of target, release, or drop",
                function.function_id,
                function.names.qualname,
                block_label,
                name,
                sinks.len(),
                sinks
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn named_block_arg_sources(args: &[(String, BlockArg)]) -> HashSet<&str> {
    args.iter()
        .filter_map(|(_, arg)| match arg {
            BlockArg::Name(name) => Some(name.as_str()),
            BlockArg::CurrentException | BlockArg::None | BlockArg::AbruptKind(_) => None,
        })
        .collect()
}

fn runtime_block_arg_sources(args: &[RuntimeBlockArgPlan]) -> HashSet<&str> {
    args.iter()
        .filter_map(|arg| match &arg.source {
            BlockArg::Name(name) => Some(name.as_str()),
            BlockArg::CurrentException | BlockArg::None | BlockArg::AbruptKind(_) => None,
        })
        .collect()
}

fn runtime_block_arg_sources_with_ref_kind<'a>(
    args: &'a [RuntimeBlockArgPlan],
    kinds: &[LocalRefKind],
    wanted: LocalRefKind,
) -> HashSet<&'a str> {
    args.iter()
        .zip(kinds)
        .filter_map(|(arg, &kind)| match &arg.source {
            BlockArg::Name(name) if kind == wanted => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

fn planned_exception_target_ref_kind(
    param: &RuntimeBlockParamPlan,
    cleanup_root_names: &HashSet<String>,
) -> LocalRefKind {
    if param.binding.storage == PlannedLocalStorage::BlockParam
        && !cleanup_root_names.contains(&param.binding.name)
        && param.binding.param_facts.ownership == LocalRefKind::Borrowed
    {
        LocalRefKind::Borrowed
    } else {
        LocalRefKind::Owned
    }
}

fn planned_borrowed_exception_forwarded_local_names(
    forwarded_local_names: &[String],
    target_args: &[RuntimeBlockArgPlan],
    target_arg_ref_kinds: &[LocalRefKind],
    slot_writes: &[(String, BlockArg)],
    release_local_names: &[String],
) -> HashSet<String> {
    let borrowed_targets = runtime_block_arg_sources_with_ref_kind(
        target_args,
        target_arg_ref_kinds,
        LocalRefKind::Borrowed,
    );
    let owned_targets = runtime_block_arg_sources_with_ref_kind(
        target_args,
        target_arg_ref_kinds,
        LocalRefKind::Owned,
    );
    let slot_sources = named_block_arg_sources(slot_writes);
    forwarded_local_names
        .iter()
        .filter(|name| {
            borrowed_targets.contains(name.as_str())
                && !owned_targets.contains(name.as_str())
                && !slot_sources.contains(name.as_str())
                && !release_local_names.contains(name)
        })
        .cloned()
        .collect()
}

fn validate_entry_materializations_for_block<P: soac_core::block_py::ModuleShape>(
    function: &BlockPyFunction<P>,
    block_label: BlockLabel,
    block_index: usize,
    block_plan: Option<&BlockLocalPlan>,
    runtime_params: &[RuntimeBlockParamPlan],
    stack_slot_entry_seeds: &[PlannedStackSlotEntrySeed],
    entry_materializations: &[PlannedLocalEnvEntryMaterialization],
    cleanup_root_names: &HashSet<String>,
) -> Result<(), String> {
    let unbound_bindings =
        unmaterialized_unbound_bindings(block_plan, runtime_params, stack_slot_entry_seeds);
    let expected_count =
        runtime_params.len() + stack_slot_entry_seeds.len() + unbound_bindings.len();
    if entry_materializations.len() != expected_count {
        return Err(format!(
            "entry materialization count mismatch for function {} ({}) block {}: expected {}, got {}",
            function.function_id,
            function.names.qualname,
            block_label,
            expected_count,
            entry_materializations.len()
        ));
    }
    for (param_index, param) in runtime_params.iter().enumerate() {
        let Some(entry) = entry_materializations.get(param_index) else {
            unreachable!("count checked above");
        };
        let expected_entry_ref_kind = match param.binding.storage {
            PlannedLocalStorage::BlockParam if cleanup_root_names.contains(&param.binding.name) => {
                local_ref_kind_for_stack_mirror(param.binding.param_facts.ownership)
            }
            PlannedLocalStorage::BlockParam => param.binding.param_facts.ownership,
            PlannedLocalStorage::StackSlot => {
                local_ref_kind_for_stack_mirror(param.binding.param_facts.ownership)
            }
        };
        if entry.source != (PlannedLocalEnvEntrySource::BlockParam { param_index })
            || entry.binding != param.binding
            || entry.entry_aliases != param.entry_aliases
            || entry.entry_ref_kind != expected_entry_ref_kind
            || entry.repr != param.repr
        {
            return Err(format!(
                "runtime-param entry materialization mismatch for function {} ({}) block {} \
                 param index {}",
                function.function_id, function.names.qualname, block_label, block_index
            ));
        }
    }
    for (seed_index, seed) in stack_slot_entry_seeds.iter().enumerate() {
        let materialization_index = runtime_params.len() + seed_index;
        let Some(entry) = entry_materializations.get(materialization_index) else {
            unreachable!("count checked above");
        };
        if entry.source != PlannedLocalEnvEntrySource::StackSlotLoad
            || entry.binding != seed.binding
            || !entry.entry_aliases.is_empty()
            || entry.entry_ref_kind != seed.entry_ref_kind
            || entry.repr != RuntimeBlockParamRepr::PyObject
        {
            return Err(format!(
                "stack-slot entry materialization mismatch for function {} ({}) block {} \
                 seed index {}",
                function.function_id, function.names.qualname, block_label, seed_index
            ));
        }
    }
    for (index, binding) in unbound_bindings.into_iter().enumerate() {
        let entry =
            &entry_materializations[runtime_params.len() + stack_slot_entry_seeds.len() + index];
        if entry.source != PlannedLocalEnvEntrySource::Unbound
            || &entry.binding != binding
            || !entry.entry_aliases.is_empty()
            || entry.entry_ref_kind != LocalRefKind::Unbound
            || entry.repr != RuntimeBlockParamRepr::PyObject
        {
            return Err(format!(
                "unbound entry materialization mismatch for function {} ({}) block {} local {}",
                function.function_id, function.names.qualname, block_label, binding.name
            ));
        }
    }
    Ok(())
}

fn build_jit_typed_module_locals_from_validated_passes(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    local_env_plan: &LocalEnvModulePlan,
    local_env_resume_plan: &LocalEnvResumeModulePlan,
    refcount_plan: &RefcountPlan,
    runtime_supported_deopt_resume_points: Option<
        &HashMap<RuntimeFunctionId, Vec<LocalEnvResumePoint>>,
    >,
) -> Result<PlannedJitModuleLocals, String> {
    let mut functions = HashMap::with_capacity(module.callable_defs.len());
    for function in &module.callable_defs {
        let local_plan = local_env_plan
            .function(function.function_id)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "missing LocalEnv plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let function_resume_plan = local_env_resume_plan
            .function(function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing LocalEnv resume plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let function_refcount_plan = refcount_plan
            .function(function.function_id)
            .cloned()
            .unwrap_or_default();
        let function_plan = plan_jit_typed_function_locals_from_plans(
            function,
            local_plan,
            function_refcount_plan,
            function_resume_plan,
            &module.module_constants,
            runtime_supported_deopt_resume_points
                .and_then(|resume_points| resume_points.get(&function.function_id))
                .map(Vec::as_slice),
        )?;
        if functions
            .insert(function.function_id, function_plan)
            .is_some()
        {
            return Err(format!(
                "duplicate JIT local plan for function id {} ({})",
                function.function_id, function.names.qualname
            ));
        }
    }
    Ok(PlannedJitModuleLocals { functions })
}

fn build_jit_typed_deopt_resume_module_from_validated_passes(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    mut resume_plan: LocalEnvResumeModulePlan,
) -> Result<PlannedJitDeoptResumeModule, String> {
    let mut functions = HashMap::with_capacity(module.callable_defs.len());
    for function in &module.callable_defs {
        let resume_plan = resume_plan
            .functions
            .remove(&function.function_id)
            .ok_or_else(|| {
                format!(
                    "missing LocalEnv resume plan for function {} ({})",
                    function.function_id, function.names.qualname
                )
            })?;
        let deopt_points =
            planned_deopt_points_from_resume_plan(function.function_id, &resume_plan);
        if functions
            .insert(
                function.function_id,
                PlannedJitDeoptResumeFunction {
                    resume_plan,
                    deopt_points,
                },
            )
            .is_some()
        {
            return Err(format!(
                "duplicate JIT deopt resume plan for function id {} ({})",
                function.function_id, function.names.qualname
            ));
        }
    }
    let plan = PlannedJitDeoptResumeModule { functions };
    plan.validate_for_typed_module(module)?;
    Ok(plan)
}

fn runtime_supported_deopt_resume_points_for_module(
    runtime_replay_module: &BlockPyModule<BlockPyModuleShape>,
    local_env_resume_plan: &LocalEnvResumeModulePlan,
) -> HashMap<RuntimeFunctionId, Vec<LocalEnvResumePoint>> {
    let runtime_functions = runtime_replay_module
        .callable_defs
        .iter()
        .map(|function| (function.function_id, function))
        .collect::<HashMap<_, _>>();
    local_env_resume_plan
        .functions
        .iter()
        .filter_map(|(function_id, resume_plan)| {
            let function = runtime_functions.get(function_id).copied()?;
            let instr_locations = current_instr_locations(function);
            let supported = resume_plan
                .entries
                .iter()
                .filter_map(|entry| {
                    runtime_jit_deopt_continuation_for_point(
                        function,
                        &instr_locations,
                        entry.point,
                    )
                    .unsupported_reason()
                    .is_none()
                    .then_some(entry.point)
                })
                .collect::<Vec<_>>();
            Some((*function_id, supported))
        })
        .collect()
}

pub fn plan_jit_typed_module_with_runtime_replay_module(
    module: BlockPyModule<TypedBlockPyModuleShape>,
    value_facts: FactStore,
    runtime_replay_module: Option<&BlockPyModule<BlockPyModuleShape>>,
) -> Result<PreparedJitTypedModulePlan, String> {
    let total_start = Instant::now();
    let precise_immortal_start = Instant::now();
    let precise_immortal_entry_states =
        compute_typed_module_precise_immortal_local_entry_states(&module, &value_facts);
    let precise_immortal_elapsed = precise_immortal_start.elapsed();
    let local_env_start = Instant::now();
    let local_env_plan = plan_typed_local_env_module_with_precise_immortal_states(
        &module,
        &value_facts,
        &precise_immortal_entry_states,
    );
    let local_env_elapsed = local_env_start.elapsed();
    let local_env_resume_start = Instant::now();
    let local_env_resume_plan =
        plan_typed_local_env_resume_module(&module, &local_env_plan, &value_facts);
    let local_env_resume_elapsed = local_env_resume_start.elapsed();
    let refcount_start = Instant::now();
    let refcount_plan = plan_typed_ownership_effects_with_precise_immortal_states(
        &module,
        &value_facts,
        &precise_immortal_entry_states,
    );
    let refcount_elapsed = refcount_start.elapsed();
    let validate_local_env_start = Instant::now();
    validate_typed_local_env_module_plan_with_precise_immortal_states(
        &module,
        &value_facts,
        &local_env_plan,
        &precise_immortal_entry_states,
    )?;
    let validate_local_env_elapsed = validate_local_env_start.elapsed();
    let validate_local_env_resume_start = Instant::now();
    validate_typed_local_env_resume_module_plan(
        &module,
        &local_env_plan,
        &value_facts,
        &local_env_resume_plan,
    )?;
    let validate_local_env_resume_elapsed = validate_local_env_resume_start.elapsed();
    let validate_refcount_start = Instant::now();
    validate_typed_ownership_effects_with_precise_immortal_states(
        &module,
        &value_facts,
        &refcount_plan,
        &precise_immortal_entry_states,
    )?;
    let validate_refcount_elapsed = validate_refcount_start.elapsed();
    let runtime_supported_deopt_resume_points = runtime_replay_module.map(|module| {
        runtime_supported_deopt_resume_points_for_module(module, &local_env_resume_plan)
    });
    let locals_start = Instant::now();
    let locals = build_jit_typed_module_locals_from_validated_passes(
        &module,
        &local_env_plan,
        &local_env_resume_plan,
        &refcount_plan,
        runtime_supported_deopt_resume_points.as_ref(),
    )?;
    let locals_elapsed = locals_start.elapsed();
    let deopt_resume_start = Instant::now();
    let deopt_resume =
        build_jit_typed_deopt_resume_module_from_validated_passes(&module, local_env_resume_plan)?;
    let deopt_resume_elapsed = deopt_resume_start.elapsed();
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.jit_typed_plan_detail",
        runtime_module_id = module.module_name_gen.runtime_module_id().as_u32(),
        function_count = u64::try_from(module.callable_defs.len()).unwrap_or(u64::MAX),
        precise_immortal_us = duration_micros(precise_immortal_elapsed),
        local_env_us = duration_micros(local_env_elapsed),
        local_env_resume_us = duration_micros(local_env_resume_elapsed),
        refcount_us = duration_micros(refcount_elapsed),
        validate_local_env_us = duration_micros(validate_local_env_elapsed),
        validate_local_env_resume_us = duration_micros(validate_local_env_resume_elapsed),
        validate_refcount_us = duration_micros(validate_refcount_elapsed),
        locals_us = duration_micros(locals_elapsed),
        deopt_resume_us = duration_micros(deopt_resume_elapsed),
        total_us = duration_micros(total_start.elapsed()),
        "jit_typed_plan_detail",
    );
    Ok(PreparedJitTypedModulePlan {
        module,
        value_facts,
        local_env_plan,
        locals,
        deopt_resume,
    })
}

pub fn plan_jit_typed_module(
    module: BlockPyModule<TypedBlockPyModuleShape>,
    value_facts: FactStore,
) -> Result<PreparedJitTypedModulePlan, String> {
    plan_jit_typed_module_with_runtime_replay_module(module, value_facts, None)
}

#[cfg(test)]
pub(super) fn plan_typed_v3_jit_module_for_test(
    module: &BlockPyModule<BlockPyModuleShape>,
    _value_facts: FactStore,
) -> Result<PreparedJitTypedModulePlan, String> {
    let prepared = soac_driver::typed_runtime::prepare_typed_v3_runtime_module(
        module,
        &SoacEnvConfig::default(),
    )?;
    plan_jit_typed_module_with_runtime_replay_module(
        prepared.module,
        prepared.value_facts,
        Some(module),
    )
}

fn planned_deopt_points_from_resume_plan(
    function_id: RuntimeFunctionId,
    resume_plan: &FunctionLocalEnvResumePlan,
) -> Vec<PlannedJitDeoptPoint> {
    resume_plan
        .entries
        .iter()
        .enumerate()
        .map(|(ordinal, entry)| PlannedJitDeoptPoint {
            id: PlannedJitDeoptPointId {
                function_id,
                ordinal,
            },
            point: entry.point,
            resume_point: entry.point,
            precision: entry.precision,
        })
        .collect()
}

pub fn render_jit_deopt_resume_module(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    plan: &PlannedJitDeoptResumeModule,
) -> Result<String, String> {
    plan.validate_for_typed_module(module)?;
    let mut out = String::new();
    for function in &module.callable_defs {
        let function_plan = plan.function(function.function_id).ok_or_else(|| {
            format!(
                "missing JIT deopt resume plan for function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_jit_deopt_resume_function(function, function_plan)?);
    }
    Ok(out)
}

pub fn render_jit_deopt_resume_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &PlannedJitDeoptResumeFunction,
) -> Result<String, String> {
    plan.validate_for_typed_function(function)?;
    let mut out = String::new();
    writeln!(
        out,
        "function {} {}:",
        function.function_id, function.names.qualname
    )
    .expect("writing to String should not fail");
    let instr_locations = current_instr_locations(function);
    for block in &function.blocks {
        writeln!(out, "  block {}:", block.label).expect("writing to String should not fail");
        for deopt_point in plan.deopt_points_for_block(block.label, &instr_locations) {
            let entry = plan.entry(deopt_point.resume_point).ok_or_else(|| {
                format!(
                    "deopt point {:?} for function {} ({}) references missing resume entry",
                    deopt_point.point, function.function_id, function.names.qualname
                )
            })?;
            writeln!(
                out,
                "    deopt #{} {} precision={:?}:",
                deopt_point.id.ordinal,
                render_jit_deopt_point(deopt_point.point),
                deopt_point.precision
            )
            .expect("writing to String should not fail");
            for binding in &entry.locals {
                writeln!(
                    out,
                    "      {}@{} binding={:?} source={:?} ownership={:?} value={:?}",
                    binding.name,
                    binding.location.0,
                    binding.binding,
                    binding.source,
                    binding.ownership,
                    binding.value
                )
                .expect("writing to String should not fail");
            }
        }
    }
    Ok(out)
}

fn render_jit_deopt_point(point: LocalEnvResumePoint) -> String {
    match point {
        LocalEnvResumePoint::BlockEntry { .. } => "block_entry".to_string(),
        LocalEnvResumePoint::BeforeInstr { key } => format!("before_instr {key}"),
        LocalEnvResumePoint::BeforeTerm { .. } => "before_term".to_string(),
    }
}

pub fn render_jit_module_locals(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    plan: &PlannedJitModuleLocals,
) -> Result<String, String> {
    plan.validate_for_typed_module(module)?;
    let mut out = String::new();
    for function in &module.callable_defs {
        let function_plan = plan.function(function.function_id).ok_or_else(|| {
            format!(
                "missing JIT local plan for function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_jit_function_locals(function, function_plan)?);
    }
    Ok(out)
}

pub fn render_jit_function_locals(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plan: &PlannedJitFunctionLocals,
) -> Result<String, String> {
    plan.validate_for_typed_function(function)?;
    let mut out = String::new();
    writeln!(
        out,
        "function {} {}:",
        function.function_id, function.names.qualname
    )
    .expect("writing to String should not fail");
    writeln!(
        out,
        "  required_stack_slots={:?}",
        plan.required_stack_slot_names_for_function(function)
    )
    .expect("writing to String should not fail");
    let mut cleanup_root_names = plan.cleanup_root_names.iter().collect::<Vec<_>>();
    cleanup_root_names.sort();
    writeln!(out, "  cleanup_roots={cleanup_root_names:?}")
        .expect("writing to String should not fail");
    for (index, block) in function.blocks.iter().enumerate() {
        writeln!(out, "  block {}:", block.label).expect("writing to String should not fail");
        if let Some(local_plan) = plan.local_plan.block(block.label) {
            writeln!(out, "    entry_locals:").expect("writing to String should not fail");
            for binding in &local_plan.entry_locals {
                writeln!(out, "      {}", render_planned_local_binding(binding))
                    .expect("writing to String should not fail");
            }
        }
        writeln!(out, "    entry_materializations:").expect("writing to String should not fail");
        for entry in &plan.entry_materializations[index] {
            writeln!(
                out,
                "      {}",
                render_local_env_entry_materialization(entry)
            )
            .expect("writing to String should not fail");
        }
        writeln!(out, "    runtime_params:").expect("writing to String should not fail");
        for param in &plan.runtime_block_params[index] {
            writeln!(
                out,
                "      {} <- {} aliases={:?}",
                param.arg_name,
                render_planned_local_binding(&param.binding),
                param.entry_aliases,
            )
            .expect("writing to String should not fail");
            writeln!(out, "        repr={:?}", param.repr)
                .expect("writing to String should not fail");
        }
        if !plan.stack_slot_entry_seeds[index].is_empty() {
            writeln!(out, "    stack_slot_entry_seeds:")
                .expect("writing to String should not fail");
            for seed in &plan.stack_slot_entry_seeds[index] {
                writeln!(
                    out,
                    "      {} entry_ref_kind={:?}",
                    render_planned_local_binding(&seed.binding),
                    seed.entry_ref_kind
                )
                .expect("writing to String should not fail");
            }
        }
        render_edge_transport(
            &mut out,
            "implicit_transport",
            &plan.implicit_target_transports[index],
        );
        if let Some(transport) = &plan.jump_edge_transports[index] {
            render_edge_transport(&mut out, "jump_transport", transport);
        }
        if let Some(dispatch) = &plan.exc_dispatches[index] {
            writeln!(out, "    exc_dispatch:").expect("writing to String should not fail");
            writeln!(out, "      target_index={}", dispatch.target_index)
                .expect("writing to String should not fail");
            writeln!(
                out,
                "      target_args=[{}]",
                render_runtime_block_args(&dispatch.target_args)
            )
            .expect("writing to String should not fail");
            writeln!(
                out,
                "      slot_writes=[{}]",
                render_named_block_args(&dispatch.slot_writes)
            )
            .expect("writing to String should not fail");
            writeln!(
                out,
                "      forwarded_locals={:?}",
                dispatch.forwarded_local_names
            )
            .expect("writing to String should not fail");
            writeln!(
                out,
                "      release_locals={:?}",
                dispatch.release_local_names
            )
            .expect("writing to String should not fail");
            writeln!(
                out,
                "      drop_forwarded_locals={:?}",
                dispatch.drop_forwarded_local_names
            )
            .expect("writing to String should not fail");
        }
    }
    Ok(out)
}

fn render_local_env_entry_materialization(entry: &PlannedLocalEnvEntryMaterialization) -> String {
    format!(
        "{} source={:?} entry_ref_kind={:?} repr={:?} aliases={:?}",
        render_planned_local_binding(&entry.binding),
        entry.source,
        entry.entry_ref_kind,
        entry.repr,
        entry.entry_aliases
    )
}

fn render_edge_transport(out: &mut String, label: &str, transport: &EdgeTransportPlan) {
    if transport.slot_writes.is_empty()
        && transport.target_args.is_empty()
        && transport.forwarded_local_names.is_empty()
    {
        return;
    }
    writeln!(out, "    {label}:").expect("writing to String should not fail");
    writeln!(
        out,
        "      target_args=[{}]",
        render_runtime_block_args(&transport.target_args)
    )
    .expect("writing to String should not fail");
    writeln!(
        out,
        "      slot_writes=[{}]",
        render_named_block_args(&transport.slot_writes)
    )
    .expect("writing to String should not fail");
    writeln!(
        out,
        "      forwarded_locals={:?}",
        transport.forwarded_local_names
    )
    .expect("writing to String should not fail");
}

fn render_named_block_args(args: &[(String, BlockArg)]) -> String {
    args.iter()
        .map(|(name, arg)| format!("{name}={arg:?}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_runtime_block_args(args: &[RuntimeBlockArgPlan]) -> String {
    args.iter()
        .map(|arg| format!("{}={:?}:{:?}", arg.target_name, arg.source, arg.repr))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn local_ref_kind_for_stack_mirror(ref_kind: LocalRefKind) -> LocalRefKind {
    match ref_kind {
        LocalRefKind::Immortal => LocalRefKind::Immortal,
        LocalRefKind::Unbound => LocalRefKind::Unbound,
        LocalRefKind::Owned | LocalRefKind::Borrowed | LocalRefKind::Unknown => {
            LocalRefKind::Borrowed
        }
    }
}

pub fn planned_jit_params_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    local_plan: &FunctionLocalPlan,
    cleanup_root_names: &HashSet<String>,
) -> Result<Vec<Vec<RuntimeBlockParamPlan>>, String> {
    let live_ins = compute_typed_function_local_live_ins(function);
    planned_jit_params_for_typed_function_with_live_ins(
        function,
        local_plan,
        cleanup_root_names,
        &live_ins,
    )
}

fn planned_jit_params_for_typed_function_with_live_ins(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    local_plan: &FunctionLocalPlan,
    cleanup_root_names: &HashSet<String>,
    live_ins: &HashMap<BlockLabel, HashSet<LocalLocation>>,
) -> Result<Vec<Vec<RuntimeBlockParamPlan>>, String> {
    function
        .blocks
        .iter()
        .map(|block| {
            let mut params = Vec::new();
            let mut seen_names = HashSet::new();
            let block_plan = local_plan.block(block.label);
            for name in block.bb_param_names() {
                let arg_name = name.to_string();
                let Some(binding) = block_plan
                    .and_then(|plan| plan.binding_for_name(arg_name.as_str()).cloned())
                else {
                    return Err(format!(
                        "missing runtime block-param binding for function {} ({}) block {} arg {:?}",
                        function.function_id, function.names.qualname, block.label, arg_name
                    ));
                };
                if cleanup_root_names.contains(&binding.name)
                    && !live_ins
                        .get(&block.label)
                        .is_some_and(|live| live.contains(&binding.location))
                {
                    continue;
                }
                seen_names.insert(arg_name.clone());
                let entry_aliases = if arg_name == binding.name {
                    Vec::new()
                } else {
                    vec![arg_name.clone()]
                };
                params.push(RuntimeBlockParamPlan {
                    arg_name,
                    entry_aliases,
                    binding,
                    repr: RuntimeBlockParamRepr::PyObject,
                });
            }
            if let Some(block_plan) = block_plan {
                for binding in &block_plan.entry_locals {
                    if binding.storage != PlannedLocalStorage::BlockParam {
                        continue;
                    }
                    if cleanup_root_names.contains(&binding.name)
                        && !live_ins
                            .get(&block.label)
                            .is_some_and(|live| live.contains(&binding.location))
                    {
                        continue;
                    }
                    if !seen_names.insert(binding.name.clone()) {
                        continue;
                    }
                    params.push(RuntimeBlockParamPlan {
                        arg_name: binding.name.clone(),
                        binding: binding.clone(),
                        entry_aliases: Vec::new(),
                        repr: RuntimeBlockParamRepr::PyObject,
                    });
                }
            }
            Ok(params)
        })
        .collect()
}

fn typed_module_constant_i64_value(module_constants: &[ConstantExpr], index: u32) -> Option<i64> {
    let constant = module_constants.get(index as usize)?;
    let ConstantExpr::Literal(value) = constant else {
        return None;
    };
    let Literal::NumberLiteral(number) = value.as_literal() else {
        return None;
    };
    let NumberLiteralValue::Int(value) = &number.value else {
        return None;
    };
    value.as_i64()
}

fn typed_expr_planned_const_i64(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<i64> {
    match expr {
        InstrTyped::Load(op) => op
            .name
            .location
            .as_constant()
            .and_then(|index| typed_module_constant_i64_value(module_constants, index)),
        _ => None,
    }
}

const fn typed_i64_binop_kind_supported(kind: BinOpKind) -> bool {
    matches!(kind, BinOpKind::Add | BinOpKind::Sub | BinOpKind::Mul)
}

fn typed_expr_planned_i64_facts(
    expr: &InstrTyped,
    local_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> Option<super::IntFacts> {
    if let Some(value) = typed_expr_planned_const_i64(expr, module_constants) {
        return Some(super::IntFacts::i64_known(value));
    }
    if expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
        .filter(|plan| exact_int_scalar_deopt_instr_ids.contains(&plan.instr_id))
        .and_then(exact_int_return_plan_i64_result)
        .is_some()
    {
        return Some(super::IntFacts::i64_unknown());
    }
    match expr {
        InstrTyped::Load(op) => op
            .name
            .local_location()
            .filter(|location| local_reprs.get(location) == Some(&RuntimeBlockParamRepr::ExactI64))
            .map(|_| super::IntFacts::i64_unknown()),
        InstrTyped::BinOp(op) if typed_i64_binop_kind_supported(op.kind) => {
            let lhs_facts = typed_expr_planned_i64_facts(
                op.left.as_ref(),
                local_reprs,
                module_constants,
                exact_int_scalar_deopt_instr_ids,
            )?;
            let rhs_facts = typed_expr_planned_i64_facts(
                op.right.as_ref(),
                local_reprs,
                module_constants,
                exact_int_scalar_deopt_instr_ids,
            )?;
            super::i64_binop_result_facts(op.kind, lhs_facts, rhs_facts)
        }
        _ => matches!(expr.result_facts(), Some(ValueFacts::I64(_)))
            .then_some(super::IntFacts::i64_unknown()),
    }
}

fn typed_expr_can_satisfy_planned_i64(
    expr: &InstrTyped,
    local_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> bool {
    typed_expr_planned_i64_facts(
        expr,
        local_reprs,
        module_constants,
        exact_int_scalar_deopt_instr_ids,
    )
    .is_some()
}

pub(super) fn exact_int_return_plan_i64_result(
    plan: &TypedExactIntReturnPlan,
) -> Option<PlanValue> {
    let [exit] = plan.hot_region.exits.as_slice() else {
        return None;
    };
    let MechanicalExitKind::Return {
        value: return_value,
    } = exit.kind
    else {
        return None;
    };
    plan.hot_region.steps.iter().find_map(|step| match step.op {
        MechanicalStepOp::Materialize {
            kind: MaterializeKind::PythonLong,
            input,
            output,
        } if output == return_value && input.rep == Rep::I64 => Some(input),
        _ => None,
    })
}

pub(super) fn exact_int_return_plan_i32_bool01_result(
    plan: &TypedExactIntReturnPlan,
) -> Option<PlanValue> {
    let [exit] = plan.hot_region.exits.as_slice() else {
        return None;
    };
    let MechanicalExitKind::Return {
        value: return_value,
    } = exit.kind
    else {
        return None;
    };
    plan.hot_region.steps.iter().find_map(|step| match step.op {
        MechanicalStepOp::Materialize {
            kind: MaterializeKind::PythonBool,
            input,
            output,
        } if output == return_value && input.rep == Rep::I32Bool01 => Some(input),
        _ => None,
    })
}

pub(super) fn exact_int_return_plan_immortal_pyobject_result(
    plan: &TypedExactIntReturnPlan,
) -> Option<PlanValue> {
    let [exit] = plan.hot_region.exits.as_slice() else {
        return None;
    };
    let MechanicalExitKind::Return { value } = exit.kind else {
        return None;
    };
    (value.rep == Rep::PyObjectImmortal).then_some(value)
}

fn typed_expr_can_satisfy_planned_i32_bool01(
    expr: &InstrTyped,
    local_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> bool {
    if expr
        .typed_extra()
        .and_then(|extra| extra.exact_int_return_plan())
        .filter(|plan| exact_int_scalar_deopt_instr_ids.contains(&plan.instr_id))
        .and_then(exact_int_return_plan_i32_bool01_result)
        .is_some()
    {
        return true;
    }
    match expr {
        InstrTyped::Load(op) => op.name.local_location().is_some_and(|location| {
            local_reprs.get(&location) == Some(&RuntimeBlockParamRepr::I32Bool01)
        }),
        InstrTyped::Truthy(_) => true,
        _ => matches!(expr.result_facts(), Some(ValueFacts::Bool(_))),
    }
}

pub(super) fn typed_expr_can_satisfy_pyobject_truthiness_repr(expr: &InstrTyped) -> bool {
    matches!(
        expr,
        InstrTyped::Load(_)
            | InstrTyped::BinOp(_)
            | InstrTyped::UnaryOp(_)
            | InstrTyped::CallTyped(_)
            | InstrTyped::GuardedCallableCallTyped(_)
            | InstrTyped::GuardedMethodCallTyped(_)
            | InstrTyped::DirectCallableCallTyped(_)
            | InstrTyped::DirectMethodCallTyped(_)
            | InstrTyped::DirectCallGuardTest(_)
            | InstrTyped::IncrementCounter(_)
            | InstrTyped::CellRef(_)
            | InstrTyped::MakeFunctionWithClosure(_)
    )
}

fn typed_store_runtime_repr_for_value(
    location: LocalLocation,
    value: &InstrTyped,
    local_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> RuntimeBlockParamRepr {
    if boxed_owner_local_locations.contains(&location) {
        return RuntimeBlockParamRepr::PyObject;
    }
    if typed_expr_can_satisfy_planned_i32_bool01(
        value,
        local_reprs,
        exact_int_scalar_deopt_instr_ids,
    ) {
        RuntimeBlockParamRepr::I32Bool01
    } else if truthiness_only_local_locations.contains(&location)
        && typed_expr_can_satisfy_pyobject_truthiness_repr(value)
    {
        RuntimeBlockParamRepr::I32Bool01
    } else if typed_expr_can_satisfy_planned_i64(
        value,
        local_reprs,
        module_constants,
        exact_int_scalar_deopt_instr_ids,
    ) {
        RuntimeBlockParamRepr::ExactI64
    } else {
        RuntimeBlockParamRepr::PyObject
    }
}

fn local_locations_by_name(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<String, LocalLocation> {
    function
        .storage_layout()
        .as_ref()
        .map(|layout| {
            layout
                .stack_slots()
                .iter()
                .enumerate()
                .map(|(slot, name)| {
                    (
                        name.clone(),
                        LocalLocation(
                            u32::try_from(slot)
                                .expect("storage layout slot index should fit in u32"),
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn internal_local_locations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<LocalLocation> {
    function
        .storage_layout()
        .as_ref()
        .map(|layout| {
            layout
                .stack_slots()
                .iter()
                .enumerate()
                .filter_map(|(slot, name)| {
                    is_internal_symbol(name).then(|| {
                        LocalLocation(
                            u32::try_from(slot)
                                .expect("storage layout slot index should fit in u32"),
                        )
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Default)]
struct TypedLocalLoadUseCollector {
    truthiness_uses: HashSet<LocalLocation>,
    pyobject_uses: HashSet<LocalLocation>,
}

impl TypedLocalLoadUseCollector {
    fn visit_pyobject_expr(&mut self, expr: &InstrTyped) {
        match expr {
            InstrTyped::Load(op) => {
                if let Some(location) = op.name.local_location() {
                    self.pyobject_uses.insert(location);
                }
            }
            InstrTyped::Truthy(op) => self.visit_truthiness_expr(op.value()),
            _ => expr.visit_children(self),
        }
    }

    fn visit_truthiness_expr(&mut self, expr: &InstrTyped) {
        if let InstrTyped::Load(op) = expr
            && let Some(location) = op.name.local_location()
        {
            self.truthiness_uses.insert(location);
            return;
        }
        self.visit_pyobject_expr(expr);
    }
}

impl Visit<InstrTyped> for TypedLocalLoadUseCollector {
    fn visit_instr(&mut self, expr: &InstrTyped)
    where
        InstrTyped: ChildVisitable<InstrTyped>,
    {
        self.visit_pyobject_expr(expr);
    }
}

fn typed_truthiness_only_internal_local_locations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<LocalLocation> {
    let internal_locations = internal_local_locations(function);
    if internal_locations.is_empty() {
        return HashSet::new();
    }

    let mut collector = TypedLocalLoadUseCollector::default();
    for block in &function.blocks {
        for instr in &block.body {
            collector.visit_pyobject_expr(instr);
        }
        match &block.term {
            BlockTerm::IfTerm(if_term) => collector.visit_truthiness_expr(&if_term.test),
            BlockTerm::BranchTable(branch) => collector.visit_pyobject_expr(&branch.index),
            BlockTerm::Raise(raise) => {
                if let Some(exc) = &raise.exc {
                    collector.visit_pyobject_expr(exc);
                }
            }
            BlockTerm::Return(value) | BlockTerm::GeneratorReturn(value) => {
                collector.visit_pyobject_expr(value)
            }
            BlockTerm::Jump(_) => {}
        }
    }

    collector
        .truthiness_uses
        .difference(&collector.pyobject_uses)
        .copied()
        .filter(|location| internal_locations.contains(location))
        .collect()
}

fn transfer_runtime_local_reprs_for_typed_block(
    block: &TypedBlock,
    entry_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> HashMap<LocalLocation, RuntimeBlockParamRepr> {
    let mut reprs = entry_reprs.clone();
    for instr in &block.body {
        transfer_runtime_local_repr_for_instr(
            instr,
            &mut reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
    }
    reprs
}

fn typed_store_runtime_local_repr(
    instr: &InstrTyped,
    local_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> Option<(LocalLocation, RuntimeBlockParamRepr)> {
    let InstrTyped::Store(op) = instr else {
        return None;
    };
    let location = op.name.local_location()?;
    let repr = typed_store_runtime_repr_for_value(
        location,
        op.value.as_ref(),
        local_reprs,
        module_constants,
        truthiness_only_local_locations,
        boxed_owner_local_locations,
        exact_int_scalar_deopt_instr_ids,
    );
    Some((location, repr))
}

fn transfer_runtime_local_repr_for_instr(
    instr: &InstrTyped,
    local_reprs: &mut HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) {
    visit_operand_takes(instr, |location| {
        let Some(location) = location.local_location() else {
            return;
        };
        local_reprs.insert(location, RuntimeBlockParamRepr::PyObject);
    });
    if let InstrTyped::CallArgumentOp(op) = instr {
        for location in op.written_names().filter_map(|name| name.local_location()) {
            local_reprs.insert(location, RuntimeBlockParamRepr::PyObject);
        }
        return;
    }
    if let Some((location, repr)) = typed_store_runtime_local_repr(
        instr,
        local_reprs,
        module_constants,
        truthiness_only_local_locations,
        boxed_owner_local_locations,
        exact_int_scalar_deopt_instr_ids,
    ) {
        local_reprs.insert(location, repr);
        return;
    }
    if let InstrTyped::Del(op) = instr
        && let Some(location) = op.name.local_location()
    {
        local_reprs.insert(location, RuntimeBlockParamRepr::PyObject);
    }
}

fn runtime_block_param_allows_scalar(
    param: &RuntimeBlockParamPlan,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
) -> bool {
    !boxed_owner_local_locations.contains(&param.binding.location)
        && param.binding.storage == PlannedLocalStorage::BlockParam
        && param.binding.param_facts.binding == ParamBindingFacts::DefinitelyBound
}

fn runtime_block_params_include_location(
    runtime_params: &[RuntimeBlockParamPlan],
    location: LocalLocation,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
) -> bool {
    runtime_params.iter().any(|param| {
        param.binding.location == location
            && runtime_block_param_allows_scalar(param, boxed_owner_local_locations)
    })
}

fn runtime_block_params_materialize_location_as_repr(
    runtime_params: &[RuntimeBlockParamPlan],
    location: LocalLocation,
    repr: RuntimeBlockParamRepr,
) -> bool {
    runtime_params
        .iter()
        .any(|param| param.binding.location == location && param.repr == repr)
}

fn runtime_block_param_reprs_by_location(
    runtime_params: &[RuntimeBlockParamPlan],
) -> HashMap<LocalLocation, RuntimeBlockParamRepr> {
    runtime_params
        .iter()
        .map(|param| (param.binding.location, param.repr))
        .collect()
}

fn block_arg_planned_repr(
    source: &BlockArg,
    source_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    local_locations_by_name: &HashMap<String, LocalLocation>,
) -> Option<RuntimeBlockParamRepr> {
    let BlockArg::Name(source_name) = source else {
        return Some(RuntimeBlockParamRepr::PyObject);
    };
    let Some(location) = local_locations_by_name.get(source_name) else {
        return Some(RuntimeBlockParamRepr::PyObject);
    };
    Some(
        source_reprs
            .get(location)
            .copied()
            .unwrap_or(RuntimeBlockParamRepr::PyObject),
    )
}

fn typed_block_stores_scalar_runtime_repr(
    block: &TypedBlock,
    location: LocalLocation,
    entry_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> bool {
    let mut local_reprs = entry_reprs.clone();
    for instr in &block.body {
        if let Some((stored_location, repr)) = typed_store_runtime_local_repr(
            instr,
            &local_reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        ) {
            local_reprs.insert(stored_location, repr);
            if stored_location == location && repr != RuntimeBlockParamRepr::PyObject {
                return true;
            }
            continue;
        }
        if let InstrTyped::Del(op) = instr
            && let Some(deleted_location) = op.name.local_location()
        {
            local_reprs.insert(deleted_location, RuntimeBlockParamRepr::PyObject);
        }
    }
    false
}

fn typed_block_stores_matching_runtime_repr(
    block: &TypedBlock,
    location: LocalLocation,
    required_repr: RuntimeBlockParamRepr,
    entry_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> bool {
    let mut local_reprs = entry_reprs.clone();
    for instr in &block.body {
        if let Some((stored_location, repr)) = typed_store_runtime_local_repr(
            instr,
            &local_reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        ) {
            local_reprs.insert(stored_location, repr);
            if stored_location == location && repr == required_repr {
                return true;
            }
            continue;
        }
        if let InstrTyped::Del(op) = instr
            && let Some(deleted_location) = op.name.local_location()
        {
            local_reprs.insert(deleted_location, RuntimeBlockParamRepr::PyObject);
        }
    }
    false
}

fn block_can_forward_scalar_source(
    source: &BlockArg,
    source_repr: RuntimeBlockParamRepr,
    source_block: &TypedBlock,
    source_runtime_params: &[RuntimeBlockParamPlan],
    source_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    local_locations_by_name: &HashMap<String, LocalLocation>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> bool {
    if source_repr == RuntimeBlockParamRepr::PyObject {
        return true;
    }
    let BlockArg::Name(source_name) = source else {
        return false;
    };
    let Some(location) = local_locations_by_name.get(source_name).copied() else {
        return false;
    };
    runtime_block_params_include_location(
        source_runtime_params,
        location,
        boxed_owner_local_locations,
    ) || typed_block_stores_scalar_runtime_repr(
        source_block,
        location,
        source_reprs,
        module_constants,
        truthiness_only_local_locations,
        boxed_owner_local_locations,
        exact_int_scalar_deopt_instr_ids,
    )
}

fn block_can_forward_scalar_source_from_final_plan(
    source: &BlockArg,
    required_repr: RuntimeBlockParamRepr,
    source_block: &TypedBlock,
    source_runtime_params: &[RuntimeBlockParamPlan],
    local_locations_by_name: &HashMap<String, LocalLocation>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> bool {
    if required_repr == RuntimeBlockParamRepr::PyObject {
        return true;
    }
    let BlockArg::Name(source_name) = source else {
        return false;
    };
    let Some(location) = local_locations_by_name.get(source_name).copied() else {
        return false;
    };
    if runtime_block_params_materialize_location_as_repr(
        source_runtime_params,
        location,
        required_repr,
    ) {
        return true;
    }
    let source_entry_reprs = runtime_block_param_reprs_by_location(source_runtime_params);
    typed_block_stores_matching_runtime_repr(
        source_block,
        location,
        required_repr,
        &source_entry_reprs,
        module_constants,
        truthiness_only_local_locations,
        boxed_owner_local_locations,
        exact_int_scalar_deopt_instr_ids,
    )
}

fn merge_runtime_local_repr(
    target_reprs: &mut HashMap<LocalLocation, RuntimeBlockParamRepr>,
    location: LocalLocation,
    incoming: RuntimeBlockParamRepr,
) -> bool {
    match target_reprs.get_mut(&location) {
        None => {
            target_reprs.insert(location, incoming);
            true
        }
        Some(existing) if *existing == incoming || *existing == RuntimeBlockParamRepr::PyObject => {
            false
        }
        Some(existing) => {
            *existing = RuntimeBlockParamRepr::PyObject;
            true
        }
    }
}

fn merge_runtime_block_param_edge_reprs(
    target_reprs: &mut HashMap<LocalLocation, RuntimeBlockParamRepr>,
    runtime_target_params: &[RuntimeBlockParamPlan],
    full_target_param_names: &[String],
    explicit_args: &[BlockArg],
    source_block: &TypedBlock,
    source_runtime_params: &[RuntimeBlockParamPlan],
    source_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    local_locations_by_name: &HashMap<String, LocalLocation>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> bool {
    let explicit_args_by_name = full_target_param_names
        .iter()
        .zip(explicit_args.iter())
        .map(|(name, arg)| (name.as_str(), arg))
        .collect::<HashMap<_, _>>();
    let mut changed = false;
    for param in runtime_target_params {
        let source = explicit_args_by_name
            .get(param.arg_name.as_str())
            .copied()
            .cloned()
            .unwrap_or_else(|| BlockArg::Name(param.arg_name.clone()));
        let Some(source_repr) =
            block_arg_planned_repr(&source, source_reprs, local_locations_by_name)
        else {
            continue;
        };
        let can_forward_scalar = block_can_forward_scalar_source(
            &source,
            source_repr,
            source_block,
            source_runtime_params,
            source_reprs,
            local_locations_by_name,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
        let incoming = if runtime_block_param_allows_scalar(param, boxed_owner_local_locations)
            && can_forward_scalar
        {
            source_repr
        } else {
            RuntimeBlockParamRepr::PyObject
        };
        changed |= merge_runtime_local_repr(target_reprs, param.binding.location, incoming);
    }
    changed
}

fn merge_runtime_block_param_pyobject_edge_reprs(
    target_reprs: &mut HashMap<LocalLocation, RuntimeBlockParamRepr>,
    runtime_target_params: &[RuntimeBlockParamPlan],
) -> bool {
    let mut changed = false;
    for param in runtime_target_params {
        changed |= merge_runtime_local_repr(
            target_reprs,
            param.binding.location,
            RuntimeBlockParamRepr::PyObject,
        );
    }
    changed
}

fn collect_unforwardable_scalar_edge_param_downgrades(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    runtime_block_params: &[Vec<RuntimeBlockParamPlan>],
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> Vec<(usize, usize)> {
    let block_indices_by_label = typed_block_indices_by_label(function);
    let local_locations_by_name = local_locations_by_name(function);
    let mut downgrades = Vec::new();

    let mut visit_edge =
        |source_index: usize, target_label: BlockLabel, explicit_args: &[BlockArg]| {
            let source_block = &function.blocks[source_index];
            let target_index =
                typed_block_index_for_label(function, &block_indices_by_label, target_label);
            let target_param_names = function.blocks[target_index].param_name_vec();
            let explicit_args_by_name = target_param_names
                .iter()
                .zip(explicit_args.iter())
                .map(|(name, arg)| (name.as_str(), arg))
                .collect::<HashMap<_, _>>();
            for (param_index, param) in runtime_block_params[target_index].iter().enumerate() {
                if param.repr == RuntimeBlockParamRepr::PyObject {
                    continue;
                }
                let source = explicit_args_by_name
                    .get(param.arg_name.as_str())
                    .copied()
                    .cloned()
                    .unwrap_or_else(|| BlockArg::Name(param.arg_name.clone()));
                if block_can_forward_scalar_source_from_final_plan(
                    &source,
                    param.repr,
                    source_block,
                    &runtime_block_params[source_index],
                    &local_locations_by_name,
                    module_constants,
                    truthiness_only_local_locations,
                    boxed_owner_local_locations,
                    exact_int_scalar_deopt_instr_ids,
                ) {
                    continue;
                }
                downgrades.push((target_index, param_index));
            }
        };

    for (source_index, block) in function.blocks.iter().enumerate() {
        if let Some(exc_edge) = block.exc_edge.as_ref() {
            visit_edge(source_index, exc_edge.target, &exc_edge.args);
        }
        match &block.term {
            BlockTerm::Jump(edge) => visit_edge(source_index, edge.target, &edge.args),
            BlockTerm::IfTerm(if_term) => {
                visit_edge(source_index, if_term.then_label, &[]);
                visit_edge(source_index, if_term.else_label, &[]);
            }
            BlockTerm::BranchTable(branch) => {
                for target in branch
                    .targets
                    .iter()
                    .copied()
                    .chain(std::iter::once(branch.default_label))
                {
                    visit_edge(source_index, target, &[]);
                }
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {}
        }
    }

    downgrades.sort_unstable();
    downgrades.dedup();
    downgrades
}

fn downgrade_unforwardable_scalar_runtime_block_params(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    runtime_block_params: &mut [Vec<RuntimeBlockParamPlan>],
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) {
    loop {
        let downgrades = collect_unforwardable_scalar_edge_param_downgrades(
            function,
            runtime_block_params,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
        if downgrades.is_empty() {
            return;
        }
        for (block_index, param_index) in downgrades {
            runtime_block_params[block_index][param_index].repr = RuntimeBlockParamRepr::PyObject;
        }
    }
}

fn force_exception_forwarded_source_reprs_to_pyobject(
    source_reprs: &mut HashMap<LocalLocation, RuntimeBlockParamRepr>,
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    runtime_target_params: &[RuntimeBlockParamPlan],
    local_locations_by_name: &HashMap<String, LocalLocation>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    stack_slot_names: &HashSet<String>,
) -> bool {
    let Some(exc_edge) = block.exc_edge.as_ref() else {
        return false;
    };
    let target_index =
        typed_block_index_for_label(function, block_indices_by_label, exc_edge.target);
    let target_block = &function.blocks[target_index];
    let transport = plan_edge_transport(
        &target_block.param_name_vec(),
        &exc_edge.args,
        runtime_target_params,
        stack_slot_names,
    );
    let mut changed = false;
    for source_name in transport.forwarded_local_names {
        let Some(location) = local_locations_by_name.get(source_name.as_str()).copied() else {
            continue;
        };
        changed |=
            merge_runtime_local_repr(source_reprs, location, RuntimeBlockParamRepr::PyObject);
    }
    changed
}

fn runtime_block_param_reprs_known(
    runtime_params: &[RuntimeBlockParamPlan],
    entry_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
) -> bool {
    runtime_params
        .iter()
        .all(|param| entry_reprs.contains_key(&param.binding.location))
}

fn exact_int_scalar_deopt_instr_ids_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    resume_plan: &FunctionLocalEnvResumePlan,
    runtime_supported_deopt_resume_points: Option<&[LocalEnvResumePoint]>,
) -> HashSet<InstrId> {
    struct Collector<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        instr_locations: InstrLocationMap,
        resume_plan: &'a FunctionLocalEnvResumePlan,
        runtime_supported_deopt_resume_points: Option<&'a [LocalEnvResumePoint]>,
        instr_ids: HashSet<InstrId>,
    }

    fn enclosing_top_level_instr<'a>(
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        instr_locations: &InstrLocationMap,
        instr_id: InstrId,
    ) -> Option<&'a InstrTyped> {
        let location = instr_locations.get(&instr_id)?;
        let body_index = location.body_index()?;
        function
            .blocks
            .iter()
            .find(|block| block.label == location.block_label())
            .and_then(|block| block.body.get(body_index))
    }

    fn scalar_deopt_resume_available(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        instr_locations: &InstrLocationMap,
        resume_plan: &FunctionLocalEnvResumePlan,
        runtime_supported_deopt_resume_points: Option<&[LocalEnvResumePoint]>,
        instr_id: InstrId,
    ) -> bool {
        let Some(stmt) = enclosing_top_level_instr(function, instr_locations, instr_id) else {
            return false;
        };
        let Some(stmt_id) = stmt.try_semantic_instr_id() else {
            return false;
        };
        let point = LocalEnvResumePoint::BeforeInstr {
            key: InstrKey::new(function.function_id, stmt_id),
        };
        if resume_plan.entry(point).is_none() {
            return false;
        }
        if runtime_supported_deopt_resume_points
            .is_some_and(|resume_points| !resume_points.contains(&point))
        {
            return false;
        }
        instr_id == stmt_id || typed_nested_guard_misses_can_resume_before_instr(stmt)
    }

    impl Collector<'_> {
        fn visit_maybe_scalar_exact_int_return(&mut self, expr: &InstrTyped) {
            let Some(plan) = expr
                .typed_extra()
                .and_then(|extra| extra.exact_int_return_plan())
            else {
                return;
            };
            if exact_int_return_plan_i64_result(plan).is_none()
                && exact_int_return_plan_i32_bool01_result(plan).is_none()
            {
                return;
            }
            let Some(location) = self.instr_locations.get(&plan.instr_id) else {
                return;
            };
            if location.body_index().is_none() {
                return;
            }
            if scalar_deopt_resume_available(
                self.function,
                &self.instr_locations,
                self.resume_plan,
                self.runtime_supported_deopt_resume_points,
                plan.instr_id,
            ) {
                self.instr_ids.insert(plan.instr_id);
            }
        }
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            self.visit_maybe_scalar_exact_int_return(expr);
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        function,
        instr_locations: current_instr_locations(function),
        resume_plan,
        runtime_supported_deopt_resume_points,
        instr_ids: HashSet::new(),
    };
    collector.visit_fn(function);
    collector.instr_ids
}

fn planned_runtime_block_param_reprs_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    runtime_block_params: &[Vec<RuntimeBlockParamPlan>],
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> PlannedRuntimeLocalReprs {
    let block_indices_by_label = typed_block_indices_by_label(function);
    let local_locations_by_name = local_locations_by_name(function);
    let stack_slot_names = function
        .storage_layout()
        .as_ref()
        .map(|layout| layout.stack_slots().iter().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();
    let mut entry_reprs =
        vec![HashMap::<LocalLocation, RuntimeBlockParamRepr>::new(); function.blocks.len()];
    if let Some(entry_reprs) = entry_reprs.first_mut() {
        for location in local_locations_by_name.values().copied() {
            entry_reprs.insert(location, RuntimeBlockParamRepr::PyObject);
        }
    }

    let mut queued = vec![false; function.blocks.len()];
    let mut worklist = VecDeque::new();
    if !function.blocks.is_empty() {
        queued[0] = true;
        worklist.push_back(0);
    }
    while let Some(source_index) = worklist.pop_front() {
        queued[source_index] = false;
        let block = &function.blocks[source_index];
        if !runtime_block_param_reprs_known(
            &runtime_block_params[source_index],
            &entry_reprs[source_index],
        ) {
            continue;
        }
        if let Some(exc_edge) = block.exc_edge.as_ref() {
            let target_index =
                typed_block_index_for_label(function, &block_indices_by_label, exc_edge.target);
            force_exception_forwarded_source_reprs_to_pyobject(
                &mut entry_reprs[source_index],
                function,
                block,
                &runtime_block_params[target_index],
                &local_locations_by_name,
                &block_indices_by_label,
                &stack_slot_names,
            );
        }
        let exit_reprs = transfer_runtime_local_reprs_for_typed_block(
            block,
            &entry_reprs[source_index],
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
        let mut queue_target = |target_index: usize, changed: bool| {
            if changed && !queued[target_index] {
                queued[target_index] = true;
                worklist.push_back(target_index);
            }
        };
        if let Some(exc_edge) = block.exc_edge.as_ref() {
            let target_index =
                typed_block_index_for_label(function, &block_indices_by_label, exc_edge.target);
            let changed = merge_runtime_block_param_pyobject_edge_reprs(
                &mut entry_reprs[target_index],
                &runtime_block_params[target_index],
            );
            queue_target(target_index, changed);
        }
        match &block.term {
            BlockTerm::Jump(edge) => {
                let target_index =
                    typed_block_index_for_label(function, &block_indices_by_label, edge.target);
                let changed = merge_runtime_block_param_edge_reprs(
                    &mut entry_reprs[target_index],
                    &runtime_block_params[target_index],
                    &function.blocks[target_index].param_name_vec(),
                    &edge.args,
                    block,
                    &runtime_block_params[source_index],
                    &exit_reprs,
                    &local_locations_by_name,
                    module_constants,
                    truthiness_only_local_locations,
                    boxed_owner_local_locations,
                    exact_int_scalar_deopt_instr_ids,
                );
                queue_target(target_index, changed);
            }
            BlockTerm::IfTerm(if_term) => {
                for target in [if_term.then_label, if_term.else_label] {
                    let target_index =
                        typed_block_index_for_label(function, &block_indices_by_label, target);
                    let changed = merge_runtime_block_param_edge_reprs(
                        &mut entry_reprs[target_index],
                        &runtime_block_params[target_index],
                        &function.blocks[target_index].param_name_vec(),
                        &[],
                        block,
                        &runtime_block_params[source_index],
                        &exit_reprs,
                        &local_locations_by_name,
                        module_constants,
                        truthiness_only_local_locations,
                        boxed_owner_local_locations,
                        exact_int_scalar_deopt_instr_ids,
                    );
                    queue_target(target_index, changed);
                }
            }
            BlockTerm::BranchTable(branch) => {
                for target in branch
                    .targets
                    .iter()
                    .copied()
                    .chain(std::iter::once(branch.default_label))
                {
                    let target_index =
                        typed_block_index_for_label(function, &block_indices_by_label, target);
                    let changed = merge_runtime_block_param_edge_reprs(
                        &mut entry_reprs[target_index],
                        &runtime_block_params[target_index],
                        &function.blocks[target_index].param_name_vec(),
                        &[],
                        block,
                        &runtime_block_params[source_index],
                        &exit_reprs,
                        &local_locations_by_name,
                        module_constants,
                        truthiness_only_local_locations,
                        boxed_owner_local_locations,
                        exact_int_scalar_deopt_instr_ids,
                    );
                    queue_target(target_index, changed);
                }
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {}
        }
    }

    let block_param_reprs = runtime_block_params
        .iter()
        .enumerate()
        .map(|(block_index, params)| {
            params
                .iter()
                .map(|param| {
                    let repr = entry_reprs[block_index]
                        .get(&param.binding.location)
                        .copied()
                        .unwrap_or(RuntimeBlockParamRepr::PyObject);
                    if runtime_block_param_allows_scalar(param, boxed_owner_local_locations) {
                        repr
                    } else {
                        RuntimeBlockParamRepr::PyObject
                    }
                })
                .collect()
        })
        .collect();
    PlannedRuntimeLocalReprs {
        block_entry_reprs: entry_reprs,
        block_param_reprs,
    }
}

fn apply_runtime_block_param_reprs(
    runtime_block_params: &mut [Vec<RuntimeBlockParamPlan>],
    reprs: Vec<Vec<RuntimeBlockParamRepr>>,
) {
    for (params, reprs) in runtime_block_params.iter_mut().zip(reprs) {
        for (param, repr) in params.iter_mut().zip(reprs) {
            param.repr = repr;
        }
    }
}

pub fn planned_stack_slot_entry_seeds_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    local_plan: &FunctionLocalPlan,
    local_env_resume_plan: &FunctionLocalEnvResumePlan,
) -> Vec<Vec<PlannedStackSlotEntrySeed>> {
    let live_ins = compute_typed_function_local_live_ins(function);
    planned_stack_slot_entry_seeds_for_typed_function_with_live_ins(
        function,
        local_plan,
        local_env_resume_plan,
        &live_ins,
    )
}

fn planned_stack_slot_entry_seeds_for_typed_function_with_live_ins(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    local_plan: &FunctionLocalPlan,
    local_env_resume_plan: &FunctionLocalEnvResumePlan,
    live_ins: &HashMap<BlockLabel, HashSet<LocalLocation>>,
) -> Vec<Vec<PlannedStackSlotEntrySeed>> {
    let must_bound_ins = compute_typed_function_local_must_bound_ins(function);
    let instr_locations = current_instr_locations(function);
    function
        .blocks
        .iter()
        .map(|block| {
            let live_in_locations = live_ins.get(&block.label);
            let must_bound_locations = must_bound_ins.get(&block.label);
            let deopt_stack_slot_locations = local_env_resume_plan
                .entries_for_block(block.label, &instr_locations)
                .flat_map(|entry| entry.locals.iter())
                .filter_map(|binding| match binding.source {
                    LocalEnvResumeValueSource::StackSlot(location) => Some(location),
                    _ => None,
                })
                .collect::<HashSet<_>>();
            local_plan
                .block(block.label)
                .map(|block_plan| {
                    block_plan
                        .entry_locals
                        .iter()
                        .cloned()
                        .filter_map(|binding| {
                            if binding.storage != PlannedLocalStorage::StackSlot {
                                return None;
                            }
                            if !live_in_locations
                                .is_some_and(|locations| locations.contains(&binding.location))
                                && !must_bound_locations
                                    .is_some_and(|locations| locations.contains(&binding.location))
                                && !deopt_stack_slot_locations.contains(&binding.location)
                            {
                                return None;
                            }
                            let entry_ref_kind =
                                local_ref_kind_for_stack_mirror(binding.param_facts.ownership);
                            Some(PlannedStackSlotEntrySeed {
                                entry_ref_kind,
                                binding,
                            })
                        })
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect()
}

pub fn planned_cleanup_root_names_for_refcount_plan(
    refcount_plan: &FunctionRefcountPlan,
    storage_layout: Option<&StorageLayout>,
) -> HashSet<String> {
    let mut names = HashSet::new();
    for block_plan in refcount_plan.blocks.values() {
        for action in &block_plan.actions {
            let RefcountActionKind::ReleaseLocal { local, reason, .. } = &action.kind else {
                continue;
            };
            match reason {
                RefcountReleaseReason::Return | RefcountReleaseReason::Raise => {}
                RefcountReleaseReason::Jump { .. }
                | RefcountReleaseReason::IfThen { .. }
                | RefcountReleaseReason::IfElse { .. }
                | RefcountReleaseReason::BranchCase { .. }
                | RefcountReleaseReason::BranchDefault { .. }
                | RefcountReleaseReason::ExceptionEdge { .. } => {
                    if local.cleanup_order.0
                        && can_use_cleanup_root(local.name.as_str())
                        && !can_release_via_stack_slot_fallback(storage_layout, local.location)
                    {
                        names.insert(local.name.clone());
                    }
                }
            }
        }
    }
    names
}

fn cleanup_root_slot_state_for_local_ref_state(state: LocalRefState) -> CleanupRootSlotState {
    if state.needs_decref() {
        CleanupRootSlotState::MaybeOwnedReference
    } else {
        CleanupRootSlotState::NoOwnedReference
    }
}

fn merge_cleanup_root_slot_state_maps(
    target: &mut HashMap<String, CleanupRootSlotState>,
    incoming: &HashMap<String, CleanupRootSlotState>,
) -> bool {
    let mut changed = false;
    for (name, incoming_state) in incoming {
        let Some(target_state) = target.get_mut(name) else {
            target.insert(name.clone(), *incoming_state);
            changed = true;
            continue;
        };
        if *target_state == CleanupRootSlotState::NoOwnedReference
            && *incoming_state == CleanupRootSlotState::MaybeOwnedReference
        {
            *target_state = CleanupRootSlotState::MaybeOwnedReference;
            changed = true;
        }
    }
    changed
}

fn cleanup_root_slot_state_vec_to_map(
    slot_names: &[String],
    states: &[CleanupRootSlotState],
) -> HashMap<String, CleanupRootSlotState> {
    slot_names
        .iter()
        .cloned()
        .zip(states.iter().copied())
        .collect()
}

fn merge_cleanup_root_slot_state_vecs(
    target: &mut [CleanupRootSlotState],
    incoming: &[CleanupRootSlotState],
) -> bool {
    let mut changed = false;
    for (target_state, incoming_state) in target.iter_mut().zip(incoming.iter().copied()) {
        if *target_state == CleanupRootSlotState::NoOwnedReference
            && incoming_state == CleanupRootSlotState::MaybeOwnedReference
        {
            *target_state = CleanupRootSlotState::MaybeOwnedReference;
            changed = true;
        }
    }
    changed
}

fn cleanup_root_slot_state_after_dispatch_write(source: &BlockArg) -> CleanupRootSlotState {
    match source {
        BlockArg::None | BlockArg::AbruptKind(_) => CleanupRootSlotState::NoOwnedReference,
        BlockArg::Name(_) | BlockArg::CurrentException => CleanupRootSlotState::MaybeOwnedReference,
    }
}

fn apply_exception_dispatch_writes_to_dense_cleanup_root_slot_state(
    mut state: Vec<CleanupRootSlotState>,
    dispatch: &BlockExcDispatchPlan,
    slot_indices_by_name: &HashMap<String, usize>,
) -> Vec<CleanupRootSlotState> {
    for (target_name, source) in &dispatch.slot_writes {
        let Some(slot_index) = slot_indices_by_name.get(target_name).copied() else {
            continue;
        };
        state[slot_index] = cleanup_root_slot_state_after_dispatch_write(source);
    }
    state
}

fn cleanup_root_slot_successors_for_block<'a>(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    source_index: usize,
    exc_dispatches: &'a [Option<BlockExcDispatchPlan>],
) -> Vec<(usize, Option<&'a BlockExcDispatchPlan>)> {
    let block = &function.blocks[source_index];
    let mut successors = Vec::new();
    if let Some(dispatch) = exc_dispatches[source_index].as_ref() {
        successors.push((dispatch.target_index, Some(dispatch)));
    }
    match &block.term {
        BlockTerm::Jump(edge) => {
            successors.push((
                typed_block_index_for_label(function, block_indices_by_label, edge.target),
                None,
            ));
        }
        BlockTerm::IfTerm(if_term) => {
            successors.push((
                typed_block_index_for_label(function, block_indices_by_label, if_term.then_label),
                None,
            ));
            successors.push((
                typed_block_index_for_label(function, block_indices_by_label, if_term.else_label),
                None,
            ));
        }
        BlockTerm::BranchTable(branch) => {
            for target in &branch.targets {
                successors.push((
                    typed_block_index_for_label(function, block_indices_by_label, *target),
                    None,
                ));
            }
            successors.push((
                typed_block_index_for_label(function, block_indices_by_label, branch.default_label),
                None,
            ));
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {}
    }
    successors
}

fn operand_slot_name(layout: Option<&StorageLayout>, location: LocalLocation) -> &str {
    &layout
        .expect("validated operand storage layout")
        .stack_slots()[location.slot() as usize]
}

fn call_argument_slot_update<'a>(
    instr: &InstrTyped,
    layout: Option<&'a StorageLayout>,
) -> Option<(&'a str, bool)> {
    let InstrTyped::CallArgumentOp(op) = instr else {
        return None;
    };
    if !op.kind.replaces_buffer() {
        return None;
    }
    let location = op.buffer.local_location()?;
    Some((
        operand_slot_name(layout, location),
        op.kind.consumes_buffer_before_helper(),
    ))
}

fn transfer_cleanup_root_slot_state_for_block(
    function_id: RuntimeFunctionId,
    block: &TypedBlock,
    layout: Option<&StorageLayout>,
    refcount_plan: &FunctionRefcountPlan,
    tracked_slot_names: &HashSet<String>,
    entry_state: &HashMap<String, CleanupRootSlotState>,
    entry_runtime_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
    previous_states: Option<&mut HashMap<InstrKey, HashMap<String, CleanupRootSlotState>>>,
) -> CleanupRootBlockTransfer<HashMap<String, CleanupRootSlotState>> {
    let mut state = entry_state.clone();
    let mut exceptional = state.clone();
    let mut runtime_reprs = entry_runtime_reprs.clone();
    let mut previous_states = previous_states;
    let mut actions_by_instr = HashMap::<InstrKey, Vec<&RefcountActionKind>>::new();
    for action in refcount_plan
        .block(block.label)
        .into_iter()
        .flat_map(|plan| &plan.actions)
    {
        let RefcountSite::Instr(instr_key) = &action.site else {
            continue;
        };
        actions_by_instr
            .entry(*instr_key)
            .or_default()
            .push(&action.kind);
    }

    for instr in &block.body {
        // Include every completed operation boundary, not merely entry/final:
        // an earlier Store may acquire an owner that a later Take consumes.
        // Nested expressions can remove Operand owners but cannot contain a
        // Store, so their possible prefixes are covered by the pre-op state.
        merge_cleanup_root_slot_state_maps(&mut exceptional, &state);
        // A nested consuming read clears its owner before the enclosing
        // operation's rebind, even when that operation has no refcount action.
        visit_operand_takes(instr, |location| {
            let Some(location) = location.local_location() else {
                return;
            };
            let name = operand_slot_name(layout, location);
            if tracked_slot_names.contains(name) {
                state.insert(name.to_owned(), CleanupRootSlotState::NoOwnedReference);
            }
        });
        if let Some((name, _)) = call_argument_slot_update(instr, layout)
            && tracked_slot_names.contains(name)
        {
            // Successful replacement owns the newly prepared tuple. Failure
            // remains nullable, represented by the separate prefix facts.
            state.insert(name.to_owned(), CleanupRootSlotState::MaybeOwnedReference);
        }
        let store_repr = typed_store_runtime_local_repr(
            instr,
            &runtime_reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
        let Some(instr_id) = instr.try_semantic_instr_id() else {
            transfer_runtime_local_repr_for_instr(
                instr,
                &mut runtime_reprs,
                module_constants,
                truthiness_only_local_locations,
                boxed_owner_local_locations,
                exact_int_scalar_deopt_instr_ids,
            );
            continue;
        };
        let instr_key = InstrKey::new(function_id, instr_id);
        let Some(actions) = actions_by_instr.get(&instr_key) else {
            transfer_runtime_local_repr_for_instr(
                instr,
                &mut runtime_reprs,
                module_constants,
                truthiness_only_local_locations,
                boxed_owner_local_locations,
                exact_int_scalar_deopt_instr_ids,
            );
            continue;
        };
        for action in actions {
            match action {
                RefcountActionKind::RebindLocal {
                    local, new_state, ..
                } if tracked_slot_names.contains(&local.name) => {
                    let previous_state = state
                        .get(&local.name)
                        .copied()
                        .unwrap_or(CleanupRootSlotState::NoOwnedReference);
                    if let Some(previous_states) = previous_states.as_mut() {
                        previous_states
                            .entry(instr_key)
                            .or_default()
                            .entry(local.name.clone())
                            .or_insert(previous_state);
                    }
                    let new_slot_state = match store_repr {
                        Some((
                            location,
                            RuntimeBlockParamRepr::ExactI64 | RuntimeBlockParamRepr::I32Bool01,
                        )) if location == local.location => CleanupRootSlotState::NoOwnedReference,
                        _ => cleanup_root_slot_state_for_local_ref_state(*new_state),
                    };
                    state.insert(local.name.clone(), new_slot_state);
                }
                RefcountActionKind::DeleteLocal { local, .. }
                    if tracked_slot_names.contains(&local.name) =>
                {
                    let previous_state = state
                        .get(&local.name)
                        .copied()
                        .unwrap_or(CleanupRootSlotState::NoOwnedReference);
                    if let Some(previous_states) = previous_states.as_mut() {
                        previous_states
                            .entry(instr_key)
                            .or_default()
                            .entry(local.name.clone())
                            .or_insert(previous_state);
                    }
                    state.insert(local.name.clone(), CleanupRootSlotState::NoOwnedReference);
                }
                _ => {}
            }
        }
        transfer_runtime_local_repr_for_instr(
            instr,
            &mut runtime_reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
    }
    merge_cleanup_root_slot_state_maps(&mut exceptional, &state);
    visit_term_operand_takes(&block.term, |location| {
        let Some(location) = location.local_location() else {
            return;
        };
        let name = operand_slot_name(layout, location);
        if tracked_slot_names.contains(name) {
            state.insert(name.to_owned(), CleanupRootSlotState::NoOwnedReference);
        }
    });
    merge_cleanup_root_slot_state_maps(&mut exceptional, &state);
    CleanupRootBlockTransfer {
        normal: state,
        exceptional,
    }
}

fn transfer_dense_cleanup_root_slot_state_for_block(
    function_id: RuntimeFunctionId,
    block: &TypedBlock,
    layout: Option<&StorageLayout>,
    refcount_plan: &FunctionRefcountPlan,
    slot_indices_by_name: &HashMap<String, usize>,
    entry_state: &[CleanupRootSlotState],
    entry_runtime_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> CleanupRootBlockTransfer<Vec<CleanupRootSlotState>> {
    let mut state = entry_state.to_vec();
    let mut exceptional = state.clone();
    let mut runtime_reprs = entry_runtime_reprs.clone();
    let mut actions_by_instr = HashMap::<InstrKey, Vec<&RefcountActionKind>>::new();
    for action in refcount_plan
        .block(block.label)
        .into_iter()
        .flat_map(|plan| &plan.actions)
    {
        let RefcountSite::Instr(instr_key) = &action.site else {
            continue;
        };
        actions_by_instr
            .entry(*instr_key)
            .or_default()
            .push(&action.kind);
    }

    for instr in &block.body {
        merge_cleanup_root_slot_state_vecs(&mut exceptional, &state);
        visit_operand_takes(instr, |location| {
            let Some(location) = location.local_location() else {
                return;
            };
            if let Some(index) = slot_indices_by_name.get(operand_slot_name(layout, location)) {
                state[*index] = CleanupRootSlotState::NoOwnedReference;
            }
        });
        if let Some((name, _)) = call_argument_slot_update(instr, layout)
            && let Some(index) = slot_indices_by_name.get(name)
        {
            state[*index] = CleanupRootSlotState::MaybeOwnedReference;
        }
        let store_repr = typed_store_runtime_local_repr(
            instr,
            &runtime_reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
        let Some(instr_id) = instr.try_semantic_instr_id() else {
            transfer_runtime_local_repr_for_instr(
                instr,
                &mut runtime_reprs,
                module_constants,
                truthiness_only_local_locations,
                boxed_owner_local_locations,
                exact_int_scalar_deopt_instr_ids,
            );
            continue;
        };
        let instr_key = InstrKey::new(function_id, instr_id);
        let Some(actions) = actions_by_instr.get(&instr_key) else {
            transfer_runtime_local_repr_for_instr(
                instr,
                &mut runtime_reprs,
                module_constants,
                truthiness_only_local_locations,
                boxed_owner_local_locations,
                exact_int_scalar_deopt_instr_ids,
            );
            continue;
        };
        for action in actions {
            match action {
                RefcountActionKind::RebindLocal {
                    local, new_state, ..
                } if slot_indices_by_name.contains_key(&local.name) => {
                    let slot_index = slot_indices_by_name[&local.name];
                    let new_slot_state = match store_repr {
                        Some((
                            location,
                            RuntimeBlockParamRepr::ExactI64 | RuntimeBlockParamRepr::I32Bool01,
                        )) if location == local.location => CleanupRootSlotState::NoOwnedReference,
                        _ => cleanup_root_slot_state_for_local_ref_state(*new_state),
                    };
                    state[slot_index] = new_slot_state;
                }
                RefcountActionKind::DeleteLocal { local, .. }
                    if slot_indices_by_name.contains_key(&local.name) =>
                {
                    let slot_index = slot_indices_by_name[&local.name];
                    state[slot_index] = CleanupRootSlotState::NoOwnedReference;
                }
                _ => {}
            }
        }
        transfer_runtime_local_repr_for_instr(
            instr,
            &mut runtime_reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
    }
    merge_cleanup_root_slot_state_vecs(&mut exceptional, &state);
    visit_term_operand_takes(&block.term, |location| {
        let Some(location) = location.local_location() else {
            return;
        };
        if let Some(index) = slot_indices_by_name.get(operand_slot_name(layout, location)) {
            state[*index] = CleanupRootSlotState::NoOwnedReference;
        }
    });
    merge_cleanup_root_slot_state_vecs(&mut exceptional, &state);
    CleanupRootBlockTransfer {
        normal: state,
        exceptional,
    }
}

fn typed_store_py_facts(instr: &InstrTyped, target_location: LocalLocation) -> Option<PyObjFacts> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    (store.name.local_location() == Some(target_location))
        .then(|| store.value.result_facts().and_then(ValueFacts::as_pyobj))
        .flatten()
}

fn cleanup_root_slot_facts_for_local_rebind(
    instr: &InstrTyped,
    local: &RefcountLocal,
    new_state: LocalRefState,
    store_repr: Option<(LocalLocation, RuntimeBlockParamRepr)>,
) -> Option<PyObjFacts> {
    if matches!(
        store_repr,
        Some((
            location,
            RuntimeBlockParamRepr::ExactI64 | RuntimeBlockParamRepr::I32Bool01,
        )) if location == local.location
    ) {
        return None;
    }
    typed_store_py_facts(instr, local.location).or_else(|| {
        (new_state == LocalRefState::Immortal).then(|| {
            PyObjFacts::unknown()
                .with_non_null_ref()
                .with_immortal_refcount()
        })
    })
}

fn transfer_cleanup_root_slot_facts_for_block(
    function_id: RuntimeFunctionId,
    block: &TypedBlock,
    layout: Option<&StorageLayout>,
    refcount_plan: &FunctionRefcountPlan,
    tracked_slot_names: &HashSet<String>,
    entry_facts: &HashMap<String, PyObjFacts>,
    entry_runtime_reprs: &HashMap<LocalLocation, RuntimeBlockParamRepr>,
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
    previous_facts: Option<&mut HashMap<InstrKey, HashMap<String, PyObjFacts>>>,
) -> CleanupRootBlockTransfer<HashMap<String, PyObjFacts>> {
    let mut facts = entry_facts.clone();
    let mut exceptional = facts.clone();
    let mut runtime_reprs = entry_runtime_reprs.clone();
    let mut previous_facts = previous_facts;
    let mut actions_by_instr = HashMap::<InstrKey, Vec<&RefcountActionKind>>::new();
    for action in refcount_plan
        .block(block.label)
        .into_iter()
        .flat_map(|plan| &plan.actions)
    {
        let RefcountSite::Instr(instr_key) = &action.site else {
            continue;
        };
        actions_by_instr
            .entry(*instr_key)
            .or_default()
            .push(&action.kind);
    }

    for instr in &block.body {
        retain_common_cleanup_root_slot_facts(&mut exceptional, &facts);
        visit_operand_takes(instr, |location| {
            let Some(location) = location.local_location() else {
                return;
            };
            let name = operand_slot_name(layout, location);
            facts.remove(name);
            // A later child may fail after the move but before this root's
            // successful Store replaces the same slot. Both endpoint values
            // can be non-null even though that failure observes NULL.
            exceptional.remove(name);
        });
        if let Some((name, consumed_before_helper)) = call_argument_slot_update(instr, layout) {
            facts.remove(name);
            if consumed_before_helper {
                // LIST_TO_TUPLE can fail while both the old and replacement
                // owner are absent. A non-null fact from either endpoint
                // cannot authorize a non-null release on that error edge.
                exceptional.remove(name);
            }
        }
        let store_repr = typed_store_runtime_local_repr(
            instr,
            &runtime_reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
        let Some(instr_id) = instr.try_semantic_instr_id() else {
            transfer_runtime_local_repr_for_instr(
                instr,
                &mut runtime_reprs,
                module_constants,
                truthiness_only_local_locations,
                boxed_owner_local_locations,
                exact_int_scalar_deopt_instr_ids,
            );
            continue;
        };
        let instr_key = InstrKey::new(function_id, instr_id);
        let Some(actions) = actions_by_instr.get(&instr_key) else {
            transfer_runtime_local_repr_for_instr(
                instr,
                &mut runtime_reprs,
                module_constants,
                truthiness_only_local_locations,
                boxed_owner_local_locations,
                exact_int_scalar_deopt_instr_ids,
            );
            continue;
        };
        for action in actions {
            match action {
                RefcountActionKind::RebindLocal {
                    local, new_state, ..
                } if tracked_slot_names.contains(&local.name) => {
                    if let Some(previous) = facts.get(&local.name).copied()
                        && let Some(previous_facts) = previous_facts.as_mut()
                    {
                        previous_facts
                            .entry(instr_key)
                            .or_default()
                            .entry(local.name.clone())
                            .or_insert(previous);
                    }
                    if let Some(new_facts) = cleanup_root_slot_facts_for_local_rebind(
                        instr, local, *new_state, store_repr,
                    ) {
                        facts.insert(local.name.clone(), new_facts);
                    } else {
                        facts.remove(&local.name);
                    }
                }
                RefcountActionKind::DeleteLocal { local, .. }
                    if tracked_slot_names.contains(&local.name) =>
                {
                    if let Some(previous) = facts.get(&local.name).copied()
                        && let Some(previous_facts) = previous_facts.as_mut()
                    {
                        previous_facts
                            .entry(instr_key)
                            .or_default()
                            .entry(local.name.clone())
                            .or_insert(previous);
                    }
                    facts.remove(&local.name);
                }
                _ => {}
            }
        }
        transfer_runtime_local_repr_for_instr(
            instr,
            &mut runtime_reprs,
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
    }
    retain_common_cleanup_root_slot_facts(&mut exceptional, &facts);
    visit_term_operand_takes(&block.term, |location| {
        let Some(location) = location.local_location() else {
            return;
        };
        facts.remove(operand_slot_name(layout, location));
    });
    retain_common_cleanup_root_slot_facts(&mut exceptional, &facts);
    CleanupRootBlockTransfer {
        normal: facts,
        exceptional,
    }
}

fn retain_common_cleanup_root_slot_facts(
    current: &mut HashMap<String, PyObjFacts>,
    incoming: &HashMap<String, PyObjFacts>,
) {
    current.retain(|name, current_facts| {
        let Some(incoming_facts) = incoming.get(name).copied() else {
            return false;
        };
        let mut merged = PyObjFacts::unknown();
        if current_facts.is_non_null_ref() && incoming_facts.is_non_null_ref() {
            merged = merged.with_non_null_ref();
        }
        if current_facts.is_immortal() && incoming_facts.is_immortal() {
            merged = merged.with_immortal_refcount();
        }
        if merged == PyObjFacts::unknown() {
            return false;
        }
        *current_facts = merged;
        true
    });
}

fn cleanup_root_entry_facts_for_block(
    local_plan: &FunctionLocalPlan,
    label: BlockLabel,
    tracked_slot_names: &HashSet<String>,
) -> HashMap<String, PyObjFacts> {
    local_plan
        .block(label)
        .into_iter()
        .flat_map(|block| block.entry_locals.iter())
        .filter(|binding| tracked_slot_names.contains(&binding.name))
        // Cleanup-root block params are rebound as stack mirrors without
        // overwriting the previous stack-slot value. Only stack-slot-backed
        // entry bindings describe the value already resident in the slot.
        .filter(|binding| binding.storage == PlannedLocalStorage::StackSlot)
        .filter_map(|binding| {
            binding.param_facts.value.map(|facts| {
                let facts = if binding.param_facts.binding.requires_checked_local_load() {
                    facts.without_non_null_ref()
                } else {
                    facts
                };
                (binding.name.clone(), facts)
            })
        })
        .collect()
}

pub fn planned_cleanup_root_slot_states_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    local_plan: &FunctionLocalPlan,
    refcount_plan: &FunctionRefcountPlan,
    tracked_slot_names: &HashSet<String>,
    exc_dispatches: &[Option<BlockExcDispatchPlan>],
    runtime_entry_reprs: &[HashMap<LocalLocation, RuntimeBlockParamRepr>],
    module_constants: &[ConstantExpr],
    truthiness_only_local_locations: &HashSet<LocalLocation>,
    boxed_owner_local_locations: &HashSet<LocalLocation>,
    exact_int_scalar_deopt_instr_ids: &HashSet<InstrId>,
) -> PlannedCleanupRootSlotStates {
    let total_start = Instant::now();
    let block_count = function.blocks.len();
    let block_indices_by_label = typed_block_indices_by_label(function);
    let slot_names = tracked_slot_names.iter().cloned().collect::<Vec<_>>();
    let slot_indices_by_name = slot_names
        .iter()
        .enumerate()
        .map(|(index, name)| (name.clone(), index))
        .collect::<HashMap<_, _>>();
    let mut entry_states =
        vec![vec![CleanupRootSlotState::NoOwnedReference; slot_names.len()]; block_count];
    let mut entry_reached = vec![false; block_count];
    if let Some(entry_state) = entry_states.first_mut() {
        entry_reached[0] = true;
        for name in &slot_names {
            if local_name_has_block_parameter_role(
                function.storage_layout().as_ref(),
                name,
                BlockParamRole::AbruptKind,
            ) {
                let slot_index = slot_indices_by_name[name];
                entry_state[slot_index] = CleanupRootSlotState::MaybeOwnedReference;
            }
        }
    }

    let successors_by_block = (0..block_count)
        .map(|source_index| {
            cleanup_root_slot_successors_for_block(
                function,
                &block_indices_by_label,
                source_index,
                exc_dispatches,
            )
        })
        .collect::<Vec<_>>();
    let mut queued = vec![false; block_count];
    let mut worklist = VecDeque::new();
    if block_count > 0 {
        queued[0] = true;
        worklist.push_back(0);
    }
    let propagate_start = Instant::now();
    while let Some(source_index) = worklist.pop_front() {
        queued[source_index] = false;
        if !entry_reached[source_index] {
            continue;
        }
        let block = &function.blocks[source_index];
        let transfer = transfer_dense_cleanup_root_slot_state_for_block(
            function.function_id,
            block,
            function.storage_layout().as_ref(),
            refcount_plan,
            &slot_indices_by_name,
            &entry_states[source_index],
            &runtime_entry_reprs[source_index],
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
        );
        for (target_index, maybe_dispatch) in &successors_by_block[source_index] {
            let incoming = if let Some(dispatch) = maybe_dispatch {
                apply_exception_dispatch_writes_to_dense_cleanup_root_slot_state(
                    transfer.exceptional.clone(),
                    dispatch,
                    &slot_indices_by_name,
                )
            } else {
                transfer.normal.clone()
            };
            let state_changed = if !entry_reached[*target_index] {
                entry_states[*target_index] = incoming;
                entry_reached[*target_index] = true;
                true
            } else {
                merge_cleanup_root_slot_state_vecs(&mut entry_states[*target_index], &incoming)
            };
            if state_changed && !queued[*target_index] {
                queued[*target_index] = true;
                worklist.push_back(*target_index);
            }
        }
    }
    let propagate_elapsed = propagate_start.elapsed();

    let materialize_start = Instant::now();
    let mut block_entry_states = HashMap::new();
    let mut block_exit_states = HashMap::new();
    let mut block_exception_states = HashMap::new();
    let mut instr_previous_states = HashMap::new();
    let mut block_exit_facts = HashMap::new();
    let mut block_exception_facts = HashMap::new();
    let mut instr_previous_facts = HashMap::new();
    for (index, block) in function.blocks.iter().enumerate() {
        let entry_state = cleanup_root_slot_state_vec_to_map(&slot_names, &entry_states[index]);
        let exit_state = transfer_cleanup_root_slot_state_for_block(
            function.function_id,
            block,
            function.storage_layout().as_ref(),
            refcount_plan,
            tracked_slot_names,
            &entry_state,
            &runtime_entry_reprs[index],
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
            Some(&mut instr_previous_states),
        );
        let entry_facts =
            cleanup_root_entry_facts_for_block(local_plan, block.label, tracked_slot_names);
        let exit_facts = transfer_cleanup_root_slot_facts_for_block(
            function.function_id,
            block,
            function.storage_layout().as_ref(),
            refcount_plan,
            tracked_slot_names,
            &entry_facts,
            &runtime_entry_reprs[index],
            module_constants,
            truthiness_only_local_locations,
            boxed_owner_local_locations,
            exact_int_scalar_deopt_instr_ids,
            Some(&mut instr_previous_facts),
        );
        block_entry_states.insert(block.label, entry_state);
        block_exit_states.insert(block.label, exit_state.normal);
        block_exception_states.insert(block.label, exit_state.exceptional);
        block_exit_facts.insert(block.label, exit_facts.normal);
        block_exception_facts.insert(block.label, exit_facts.exceptional);
    }
    let materialize_elapsed = materialize_start.elapsed();

    let plan = PlannedCleanupRootSlotStates {
        block_entry_states,
        block_exit_states,
        block_exception_states,
        instr_previous_states,
        block_exit_facts,
        block_exception_facts,
        instr_previous_facts,
    };
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.cleanup_root_slot_states_detail",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        block_count = u64::try_from(function.blocks.len()).unwrap_or(u64::MAX),
        tracked_slot_count = u64::try_from(tracked_slot_names.len()).unwrap_or(u64::MAX),
        propagate_us = duration_micros(propagate_elapsed),
        materialize_us = duration_micros(materialize_elapsed),
        total_us = duration_micros(total_start.elapsed()),
        "cleanup_root_slot_states_detail",
    );
    plan
}

fn unmaterialized_unbound_bindings<'a>(
    block_plan: Option<&'a BlockLocalPlan>,
    runtime_params: &[RuntimeBlockParamPlan],
    stack_slot_entry_seeds: &[PlannedStackSlotEntrySeed],
) -> Vec<&'a PlannedLocalBinding> {
    block_plan
        .into_iter()
        .flat_map(|plan| &plan.entry_locals)
        .filter(|binding| {
            binding.param_facts.ownership == LocalRefKind::Unbound
                && !runtime_params
                    .iter()
                    .any(|param| param.binding.location == binding.location)
                && !stack_slot_entry_seeds
                    .iter()
                    .any(|seed| seed.binding.location == binding.location)
        })
        .collect()
}

pub fn planned_local_env_entry_materializations_for_function(
    block_plans: &[&BlockLocalPlan],
    runtime_block_params: &[Vec<RuntimeBlockParamPlan>],
    stack_slot_entry_seeds: &[Vec<PlannedStackSlotEntrySeed>],
    cleanup_root_names: &HashSet<String>,
) -> Result<Vec<Vec<PlannedLocalEnvEntryMaterialization>>, String> {
    if runtime_block_params.len() != stack_slot_entry_seeds.len()
        || runtime_block_params.len() != block_plans.len()
    {
        return Err(format!(
            "entry materialization inputs have inconsistent block counts: locals={}, runtime={}, stack_seeds={}",
            block_plans.len(),
            runtime_block_params.len(),
            stack_slot_entry_seeds.len()
        ));
    }
    Ok(runtime_block_params
        .iter()
        .zip(stack_slot_entry_seeds.iter())
        .zip(block_plans)
        .map(|((params, seeds), block_plan)| {
            let mut entries = Vec::with_capacity(params.len() + seeds.len());
            entries.extend(params.iter().enumerate().map(|(param_index, param)| {
                let entry_ref_kind = match param.binding.storage {
                    PlannedLocalStorage::BlockParam
                        if cleanup_root_names.contains(&param.binding.name) =>
                    {
                        local_ref_kind_for_stack_mirror(param.binding.param_facts.ownership)
                    }
                    PlannedLocalStorage::BlockParam => param.binding.param_facts.ownership,
                    PlannedLocalStorage::StackSlot => {
                        local_ref_kind_for_stack_mirror(param.binding.param_facts.ownership)
                    }
                };
                PlannedLocalEnvEntryMaterialization {
                    binding: param.binding.clone(),
                    entry_aliases: param.entry_aliases.clone(),
                    source: PlannedLocalEnvEntrySource::BlockParam { param_index },
                    entry_ref_kind,
                    repr: param.repr,
                }
            }));
            entries.extend(
                seeds
                    .iter()
                    .map(|seed| PlannedLocalEnvEntryMaterialization {
                        binding: seed.binding.clone(),
                        entry_aliases: Vec::new(),
                        source: PlannedLocalEnvEntrySource::StackSlotLoad,
                        entry_ref_kind: seed.entry_ref_kind,
                        repr: RuntimeBlockParamRepr::PyObject,
                    }),
            );
            entries.extend(
                unmaterialized_unbound_bindings(Some(block_plan), params, seeds)
                    .into_iter()
                    .map(|binding| PlannedLocalEnvEntryMaterialization {
                        binding: binding.clone(),
                        entry_aliases: Vec::new(),
                        source: PlannedLocalEnvEntrySource::Unbound,
                        entry_ref_kind: LocalRefKind::Unbound,
                        repr: RuntimeBlockParamRepr::PyObject,
                    }),
            );
            entries
        })
        .collect())
}

pub fn plan_edge_transport(
    full_target_param_names: &[String],
    explicit_args: &[BlockArg],
    runtime_target_params: &[RuntimeBlockParamPlan],
    stack_slot_names: &HashSet<String>,
) -> EdgeTransportPlan {
    let runtime_param_name_set = runtime_target_params
        .iter()
        .map(|param| param.arg_name.as_str())
        .collect::<HashSet<_>>();
    let mut slot_writes = Vec::new();
    for (target_param_name, source) in full_target_param_names.iter().zip(explicit_args.iter()) {
        if runtime_param_name_set.contains(target_param_name.as_str())
            || !stack_slot_names.contains(target_param_name)
        {
            continue;
        }
        slot_writes.push((target_param_name.clone(), source.clone()));
    }
    let explicit_args_by_name = full_target_param_names
        .iter()
        .zip(explicit_args.iter())
        .map(|(name, arg)| (name.as_str(), arg))
        .collect::<HashMap<_, _>>();
    let target_args = runtime_target_params
        .iter()
        .map(|param| {
            let name = param.arg_name.clone();
            let source = explicit_args_by_name
                .get(name.as_str())
                .map(|arg| (*arg).clone())
                .unwrap_or_else(|| BlockArg::Name(name.clone()));
            RuntimeBlockArgPlan {
                target_name: name,
                source,
                repr: param.repr,
            }
        })
        .collect::<Vec<_>>();
    let mut forwarded_local_names = Vec::new();
    let mut record_forwarded_name = |arg: &BlockArg| {
        let BlockArg::Name(source_name) = arg else {
            return;
        };
        if forwarded_local_names.iter().any(|name| name == source_name) {
            return;
        }
        forwarded_local_names.push(source_name.clone());
    };
    for (_, arg) in slot_writes.iter() {
        record_forwarded_name(arg);
    }
    for arg in target_args.iter() {
        record_forwarded_name(&arg.source);
    }
    EdgeTransportPlan {
        slot_writes,
        target_args,
        forwarded_local_names,
    }
}

pub fn planned_implicit_target_transports_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    runtime_block_params: &[Vec<RuntimeBlockParamPlan>],
) -> Vec<EdgeTransportPlan> {
    let no_slot_writes = HashSet::new();
    function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            plan_edge_transport(
                &block.param_name_vec(),
                &[],
                &runtime_block_params[index],
                &no_slot_writes,
            )
        })
        .collect()
}

pub fn planned_jump_edge_transports_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    runtime_block_params: &[Vec<RuntimeBlockParamPlan>],
) -> Vec<Option<EdgeTransportPlan>> {
    let no_slot_writes = HashSet::new();
    let block_indices_by_label = typed_block_indices_by_label(function);
    function
        .blocks
        .iter()
        .map(|block| match &block.term {
            BlockTerm::Jump(target) => {
                let target_index =
                    typed_block_index_for_label(function, &block_indices_by_label, target.target);
                let target_block = &function.blocks[target_index];
                Some(plan_edge_transport(
                    &target_block.param_name_vec(),
                    &target.args,
                    &runtime_block_params[target_index],
                    &no_slot_writes,
                ))
            }
            _ => None,
        })
        .collect()
}

pub fn typed_exc_dispatch_plan(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    runtime_target_params: &[RuntimeBlockParamPlan],
    refcount_plan: &FunctionRefcountPlan,
    cleanup_root_names: &HashSet<String>,
) -> Option<BlockExcDispatchPlan> {
    let block_indices_by_label = typed_block_indices_by_label(function);
    let stack_slot_name_set = stack_slot_name_set_for_typed_function(function);
    typed_exc_dispatch_plan_with_shared_inputs(
        function,
        block,
        runtime_target_params,
        refcount_plan,
        cleanup_root_names,
        &block_indices_by_label,
        &stack_slot_name_set,
    )
}

fn stack_slot_name_set_for_typed_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<String> {
    function
        .storage_layout()
        .as_ref()
        .map(|layout| layout.stack_slots().iter().cloned().collect())
        .unwrap_or_default()
}

fn typed_exc_dispatch_plan_with_shared_inputs(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    runtime_target_params: &[RuntimeBlockParamPlan],
    refcount_plan: &FunctionRefcountPlan,
    cleanup_root_names: &HashSet<String>,
    block_indices_by_label: &HashMap<BlockLabel, usize>,
    stack_slot_name_set: &HashSet<String>,
) -> Option<BlockExcDispatchPlan> {
    let exc_edge = block.exc_edge.as_ref()?;
    let target_index =
        typed_block_index_for_label(function, block_indices_by_label, exc_edge.target);
    let target_block = &function.blocks[target_index];
    let full_target_param_names = target_block.param_name_vec();
    let transport = plan_edge_transport(
        &full_target_param_names,
        &exc_edge.args,
        runtime_target_params,
        stack_slot_name_set,
    );
    let release_reason = RefcountReleaseReason::ExceptionEdge {
        target: exc_edge.target,
    };
    let mut forwarded_local_names = transport.forwarded_local_names;
    let mut release_local_names = Vec::new();
    if let Some(block_plan) = refcount_plan.block(block.label) {
        for action in &block_plan.actions {
            let RefcountActionKind::ReleaseLocal {
                local,
                reason: action_reason,
                ..
            } = &action.kind
            else {
                continue;
            };
            if action_reason != &release_reason
                || can_release_via_stack_slot_fallback(
                    function.storage_layout().as_ref(),
                    local.location,
                )
                || cleanup_root_names.contains(&local.name)
                || forwarded_local_names.iter().any(|name| name == &local.name)
            {
                continue;
            }
            forwarded_local_names.push(local.name.clone());
            release_local_names.push(local.name.clone());
        }
    }
    let target_arg_ref_kinds = runtime_target_params
        .iter()
        .map(|param| planned_exception_target_ref_kind(param, cleanup_root_names))
        .collect::<Vec<_>>();
    let borrowed_forwarded_local_names = planned_borrowed_exception_forwarded_local_names(
        &forwarded_local_names,
        &transport.target_args,
        &target_arg_ref_kinds,
        &transport.slot_writes,
        &release_local_names,
    );
    let drop_forwarded_local_names = planned_drop_forwarded_local_names(
        &forwarded_local_names,
        &transport.target_args,
        &target_arg_ref_kinds,
        &release_local_names,
        &borrowed_forwarded_local_names,
    );
    Some(BlockExcDispatchPlan {
        target_index,
        slot_writes: transport.slot_writes,
        target_args: transport.target_args,
        target_arg_ref_kinds,
        forwarded_local_names,
        borrowed_forwarded_local_names,
        release_local_names,
        drop_forwarded_local_names,
    })
}

fn planned_drop_forwarded_local_names(
    forwarded_local_names: &[String],
    target_args: &[RuntimeBlockArgPlan],
    target_arg_ref_kinds: &[LocalRefKind],
    release_local_names: &[String],
    borrowed_forwarded_local_names: &HashSet<String>,
) -> Vec<String> {
    let target_arg_source_names = runtime_block_arg_sources_with_ref_kind(
        target_args,
        target_arg_ref_kinds,
        LocalRefKind::Owned,
    );
    let release_name_set = release_local_names
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    forwarded_local_names
        .iter()
        .filter(|name| {
            !target_arg_source_names.contains(name.as_str())
                && !release_name_set.contains(name.as_str())
                && !borrowed_forwarded_local_names.contains(name.as_str())
        })
        .cloned()
        .collect()
}

pub fn plan_jit_typed_function_locals_from_plans(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    local_plan: FunctionLocalPlan,
    refcount_plan: FunctionRefcountPlan,
    local_env_resume_plan: &FunctionLocalEnvResumePlan,
    module_constants: &[ConstantExpr],
    runtime_supported_deopt_resume_points: Option<&[LocalEnvResumePoint]>,
) -> Result<PlannedJitFunctionLocals, String> {
    let total_start = Instant::now();
    let setup_start = Instant::now();
    if let Some(layout) = function.storage_layout() {
        layout.validate_block_parameter_roles().map_err(|error| {
            format!(
                "invalid control storage in function {}: {error}",
                function.function_id
            )
        })?;
    }
    let block_indices_by_label = typed_block_indices_by_label(function);
    let loop_backedges =
        planned_loop_backedges_for_typed_function(function, &block_indices_by_label)?;
    let mut cleanup_root_names = planned_cleanup_root_names_for_refcount_plan(
        &refcount_plan,
        function.storage_layout().as_ref(),
    );
    let boxed_owner_local_locations = boxed_owner_local_locations(function)?;
    if let Some(layout) = function.storage_layout() {
        cleanup_root_names.extend(
            boxed_owner_local_locations
                .iter()
                .map(|location| layout.stack_slots()[location.slot() as usize].clone()),
        );
    }
    let truthiness_only_local_locations = typed_truthiness_only_internal_local_locations(function);
    let exact_int_scalar_deopt_instr_ids = exact_int_scalar_deopt_instr_ids_for_typed_function(
        function,
        local_env_resume_plan,
        runtime_supported_deopt_resume_points,
    );
    let setup_elapsed = setup_start.elapsed();
    let live_ins_start = Instant::now();
    let live_ins = compute_typed_function_local_live_ins(function);
    let live_ins_elapsed = live_ins_start.elapsed();
    let params_start = Instant::now();
    let mut runtime_block_params = planned_jit_params_for_typed_function_with_live_ins(
        function,
        &local_plan,
        &cleanup_root_names,
        &live_ins,
    )?;
    let runtime_local_reprs = planned_runtime_block_param_reprs_for_typed_function(
        function,
        &runtime_block_params,
        module_constants,
        &truthiness_only_local_locations,
        &boxed_owner_local_locations,
        &exact_int_scalar_deopt_instr_ids,
    );
    let params_elapsed = params_start.elapsed();
    let transport_start = Instant::now();
    apply_runtime_block_param_reprs(
        &mut runtime_block_params,
        runtime_local_reprs.block_param_reprs,
    );
    downgrade_unforwardable_scalar_runtime_block_params(
        function,
        &mut runtime_block_params,
        module_constants,
        &truthiness_only_local_locations,
        &boxed_owner_local_locations,
        &exact_int_scalar_deopt_instr_ids,
    );
    let implicit_target_transports =
        planned_implicit_target_transports_for_typed_function(function, &runtime_block_params);
    let jump_edge_transports =
        planned_jump_edge_transports_for_typed_function(function, &runtime_block_params);
    let transport_elapsed = transport_start.elapsed();
    let entries_start = Instant::now();
    let stack_slot_entry_seeds = planned_stack_slot_entry_seeds_for_typed_function_with_live_ins(
        function,
        &local_plan,
        local_env_resume_plan,
        &live_ins,
    );
    let entry_materializations = planned_local_env_entry_materializations_for_function(
        &function
            .blocks
            .iter()
            .map(|block| {
                local_plan.block(block.label).ok_or_else(|| {
                    format!(
                        "missing LocalEnv source plan for function {} block {}",
                        function.function_id, block.label
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        &runtime_block_params,
        &stack_slot_entry_seeds,
        &cleanup_root_names,
    )?;
    let entries_elapsed = entries_start.elapsed();
    let exc_dispatch_start = Instant::now();
    let stack_slot_name_set = stack_slot_name_set_for_typed_function(function);
    let exc_dispatches = function
        .blocks
        .iter()
        .map(|block| {
            let runtime_target_params = block
                .exc_edge
                .as_ref()
                .map(|edge| {
                    let target_index =
                        typed_block_index_for_label(function, &block_indices_by_label, edge.target);
                    runtime_block_params[target_index].as_slice()
                })
                .unwrap_or(&[]);
            typed_exc_dispatch_plan_with_shared_inputs(
                function,
                block,
                runtime_target_params,
                &refcount_plan,
                &cleanup_root_names,
                &block_indices_by_label,
                &stack_slot_name_set,
            )
        })
        .collect::<Vec<_>>();
    let exc_dispatch_elapsed = exc_dispatch_start.elapsed();
    let cleanup_start = Instant::now();
    let tracked_stack_slot_names = required_stack_slot_names_for_function_parts(
        function,
        &runtime_block_params,
        &stack_slot_entry_seeds,
        &exc_dispatches,
        &cleanup_root_names,
        &refcount_plan,
    )
    .into_iter()
    .collect::<HashSet<_>>();
    let cleanup_root_slot_states = planned_cleanup_root_slot_states_for_typed_function(
        function,
        &local_plan,
        &refcount_plan,
        &tracked_stack_slot_names,
        &exc_dispatches,
        &runtime_local_reprs.block_entry_reprs,
        module_constants,
        &truthiness_only_local_locations,
        &boxed_owner_local_locations,
        &exact_int_scalar_deopt_instr_ids,
    );
    let cleanup_elapsed = cleanup_start.elapsed();

    let plan = PlannedJitFunctionLocals {
        local_plan,
        refcount_plan,
        loop_backedges,
        cleanup_root_names,
        cleanup_root_slot_states,
        truthiness_only_local_locations,
        boxed_owner_local_locations,
        runtime_block_params,
        implicit_target_transports,
        jump_edge_transports,
        stack_slot_entry_seeds,
        entry_materializations,
        exc_dispatches,
    };
    let validate_start = Instant::now();
    plan.validate_for_typed_function(function)?;
    let validate_elapsed = validate_start.elapsed();
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.jit_typed_function_locals_detail",
        function_id = %function.function_id,
        function_qualname = function.names.qualname.as_str(),
        block_count = u64::try_from(function.blocks.len()).unwrap_or(u64::MAX),
        setup_us = duration_micros(setup_elapsed),
        live_ins_us = duration_micros(live_ins_elapsed),
        params_us = duration_micros(params_elapsed),
        transport_us = duration_micros(transport_elapsed),
        entries_us = duration_micros(entries_elapsed),
        exc_dispatch_us = duration_micros(exc_dispatch_elapsed),
        cleanup_us = duration_micros(cleanup_elapsed),
        validate_us = duration_micros(validate_elapsed),
        total_us = duration_micros(total_start.elapsed()),
        "jit_typed_function_locals_detail",
    );
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::{
        BlockExcDispatchPlan, BlockParamFacts, CleanupRootSlotState, LocalRefKind,
        ParamBindingFacts, ParamProvenance, PlannedLocalBinding, PlannedLocalEnvEntrySource,
        PlannedLocalStorage, PlannedStackSlotEntrySeed, PreparedJitTypedModulePlan,
        RuntimeBlockArgPlan, RuntimeBlockParamPlan, RuntimeBlockParamRepr,
        cleanup_root_entry_facts_for_block, local_name_has_block_parameter_role,
        plan_edge_transport, plan_typed_v3_jit_module_for_test,
        planned_cleanup_root_names_for_refcount_plan, planned_drop_forwarded_local_names,
        planned_jit_params_for_typed_function,
        planned_local_env_entry_materializations_for_function,
        planned_stack_slot_entry_seeds_for_typed_function, typed_block_index_for_label,
        typed_block_indices_by_label, typed_exc_dispatch_plan,
        validate_exception_dispatch_ownership_sinks,
    };
    use soac_core::block_py::{
        BlockArg, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm,
        LocalLocation,
    };
    use soac_ir_blockpy::BlockPyModuleShape;
    use soac_ir_typed::{InstrTyped, PyExactType, PyObjFacts};
    use soac_lowering::lower_python_to_blockpy_for_testing;
    use soac_opt::passes::{BlockLocalPlan, FunctionLocalPlan};
    use soac_opt::passes::{
        FunctionLocalEnvResumePlan, FunctionRefcountPlan, LocalEnvResumeBinding,
        LocalEnvResumeBindingState, LocalEnvResumeEntry, LocalEnvResumePoint,
        LocalEnvResumeStatePrecision, LocalEnvResumeValueSource, LocalRefState, RefcountActionKind,
        RefcountReleaseReason, infer_module_value_facts,
    };
    use std::collections::{HashMap, HashSet};

    fn lowered_function(
        source: &str,
        qualname: &str,
    ) -> (
        soac_core::block_py::BlockPyModule<BlockPyModuleShape>,
        usize,
    ) {
        let lowered = lower_python_to_blockpy_for_testing(source)
            .expect("transform should succeed")
            .blockpy_module;
        let function_index = lowered
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing lowered function {qualname}"));
        (lowered, function_index)
    }

    fn plan_typed_module_from_blockpy_module(
        module: &BlockPyModule<BlockPyModuleShape>,
    ) -> PreparedJitTypedModulePlan {
        let facts = infer_module_value_facts(module);
        plan_typed_v3_jit_module_for_test(module, facts)
            .expect("typed JIT module planning should succeed")
    }

    fn prepared_typed_function(
        source: &str,
        qualname: &str,
    ) -> (PreparedJitTypedModulePlan, usize) {
        let (lowered, blockpy_function_index) = lowered_function(source, qualname);
        let function_id = lowered.callable_defs[blockpy_function_index].function_id;
        let prepared = plan_typed_module_from_blockpy_module(&lowered);
        let typed_function_index = prepared
            .module
            .callable_defs
            .iter()
            .position(|function| function.function_id == function_id)
            .unwrap_or_else(|| panic!("missing typed function {qualname}"));
        (prepared, typed_function_index)
    }

    fn binding_for_name<'a>(block_plan: &'a BlockLocalPlan, name: &str) -> &'a PlannedLocalBinding {
        block_plan
            .entry_locals
            .iter()
            .find(|binding| binding.name == name)
            .unwrap_or_else(|| panic!("missing planned local binding {name}"))
    }

    fn sparsely_relabel_function_blocks(function: &mut BlockPyFunction<BlockPyModuleShape>) {
        let relabel = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block.label, BlockLabel::from_index(index * 3 + 2)))
            .collect::<HashMap<_, _>>();
        for block in &mut function.blocks {
            block.label = *relabel
                .get(&block.label)
                .expect("sparse relabel should cover every block");
            match &mut block.term {
                BlockTerm::Jump(edge) => {
                    edge.target = *relabel
                        .get(&edge.target)
                        .expect("sparse relabel should cover every jump target");
                }
                BlockTerm::IfTerm(if_term) => {
                    if_term.then_label = *relabel
                        .get(&if_term.then_label)
                        .expect("sparse relabel should cover every then target");
                    if_term.else_label = *relabel
                        .get(&if_term.else_label)
                        .expect("sparse relabel should cover every else target");
                }
                BlockTerm::BranchTable(branch) => {
                    for target in &mut branch.targets {
                        *target = *relabel
                            .get(target)
                            .expect("sparse relabel should cover every branch target");
                    }
                    branch.default_label = *relabel
                        .get(&branch.default_label)
                        .expect("sparse relabel should cover every branch default");
                }
                BlockTerm::Raise(_) | BlockTerm::Return(_) | BlockTerm::GeneratorReturn(_) => {}
            }
            if let Some(exc_edge) = &mut block.exc_edge {
                exc_edge.target = *relabel
                    .get(&exc_edge.target)
                    .expect("sparse relabel should cover every exception target");
            }
        }
    }

    #[test]
    fn local_plan_marks_immortal_entry_locals_from_value_facts() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    x = None
    if flag:
        return x
    return x
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let if_term = function
            .blocks
            .iter()
            .filter_map(|block| match &block.term {
                BlockTerm::IfTerm(if_term) => Some(if_term),
                _ => None,
            })
            .last()
            .expect("expected lowered conditional branch");
        let plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");

        for label in [if_term.then_label, if_term.else_label] {
            let block_plan = plan.block(label).expect("missing block local plan");
            let x = binding_for_name(block_plan, "x");
            assert_eq!(x.param_facts.ownership, LocalRefKind::Immortal);
            assert_eq!(x.storage, PlannedLocalStorage::BlockParam);
            assert_eq!(x.param_facts.binding, ParamBindingFacts::DefinitelyBound);
            assert_eq!(
                x.param_facts.provenance,
                ParamProvenance::ForwardedLocal(x.location)
            );
            assert!(
                x.param_facts
                    .value
                    .expect("x should have entry facts")
                    .is_none(),
                "x should keep the underlying None singleton fact"
            );
        }
    }

    #[test]
    fn local_plan_treats_function_params_as_borrowed_without_entry_fact() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(x):
    return x
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let entry_block = function.blocks.first().expect("expected an entry block");

        let block_plan = plan
            .block(entry_block.label)
            .expect("missing entry block local plan");
        let x = binding_for_name(block_plan, "x");
        assert_eq!(x.param_facts.ownership, LocalRefKind::Borrowed);
        assert_eq!(x.param_facts.value, None);
        assert_eq!(x.storage, PlannedLocalStorage::BlockParam);
        assert_eq!(x.param_facts.binding, ParamBindingFacts::DefinitelyBound);
        assert_eq!(
            x.param_facts.provenance,
            ParamProvenance::ForwardedLocal(x.location)
        );
    }

    #[test]
    fn planned_jit_params_include_semantic_and_cleanup_live_ins() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    x = []
    y = []
    if flag:
        return x
    return y
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let if_term = function
            .blocks
            .iter()
            .filter_map(|block| match &block.term {
                BlockTerm::IfTerm(if_term) => Some(if_term),
                _ => None,
            })
            .last()
            .expect("expected lowered conditional branch");
        let plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let runtime_params = planned_jit_params_for_typed_function(function, plan, &HashSet::new())
            .expect("runtime params should bind");
        let block_indices_by_label = typed_block_indices_by_label(function);

        let then_params = &runtime_params
            [typed_block_index_for_label(function, &block_indices_by_label, if_term.then_label)];
        assert!(then_params.iter().any(|param| param.arg_name == "x"));
        assert!(then_params.iter().any(|param| param.arg_name == "y"));

        let else_params = &runtime_params
            [typed_block_index_for_label(function, &block_indices_by_label, if_term.else_label)];
        assert!(else_params.iter().any(|param| param.arg_name == "y"));
        assert!(else_params.iter().any(|param| param.arg_name == "x"));
    }

    #[test]
    fn planned_jit_params_keep_binding_metadata_for_forwarded_locals() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    x = None
    if flag:
        return x
    return x
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let if_term = function
            .blocks
            .iter()
            .filter_map(|block| match &block.term {
                BlockTerm::IfTerm(if_term) => Some(if_term),
                _ => None,
            })
            .last()
            .expect("expected lowered conditional branch");
        let plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let runtime_params = planned_jit_params_for_typed_function(function, plan, &HashSet::new())
            .expect("runtime params should bind");
        let block_indices_by_label = typed_block_indices_by_label(function);
        let then_params = &runtime_params
            [typed_block_index_for_label(function, &block_indices_by_label, if_term.then_label)];
        let x = then_params
            .iter()
            .find(|param| param.arg_name == "x")
            .expect("expected forwarded x param");
        let binding = &x.binding;
        assert_eq!(binding.name, "x");
        assert_eq!(binding.storage, PlannedLocalStorage::BlockParam);
        assert_eq!(
            binding.param_facts.binding,
            ParamBindingFacts::DefinitelyBound
        );
    }

    #[test]
    fn planned_jit_params_for_typed_function_validate_handler_exception_carriers() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f():
    try:
        raise ValueError("boom")
    except ValueError:
        return 1
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");

        let runtime_params = planned_jit_params_for_typed_function(function, plan, &HashSet::new())
            .expect("runtime params should bind");
        let handler_params = runtime_params
            .iter()
            .enumerate()
            .flat_map(|(index, params)| {
                params.iter().filter(move |param| {
                    function.blocks[index].params.iter().any(|declaration| {
                        declaration.name == param.arg_name
                            && matches!(
                                declaration.role,
                                BlockParamRole::Exception | BlockParamRole::EnclosingException
                            )
                    })
                })
            })
            .collect::<Vec<_>>();
        let layout = function
            .storage_layout()
            .as_ref()
            .expect("resolved local storage");

        assert!(
            !handler_params.is_empty(),
            "expected at least one handler runtime param set"
        );
        assert!(
            handler_params.iter().all(|param| {
                layout
                    .block_parameter_roles_at(soac_core::block_py::NameLocation::Local(
                        param.binding.location,
                    ))
                    .any(|role| {
                        matches!(
                            role,
                            BlockParamRole::Exception | BlockParamRole::EnclosingException
                        )
                    })
            }),
            "validated handler runtime params must preserve exact exception-carrier locations: {handler_params:#?}"
        );
    }

    #[test]
    fn must_bound_cleanup_root_locals_do_not_travel_as_block_params() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    x = 1
    if flag:
        return 1
    return 0
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let if_term = function
            .blocks
            .iter()
            .filter_map(|block| match &block.term {
                BlockTerm::IfTerm(if_term) => Some(if_term),
                _ => None,
            })
            .last()
            .expect("expected lowered conditional branch");
        let plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let cleanup_root_names = HashSet::from(["x".to_string()]);
        let runtime_params =
            planned_jit_params_for_typed_function(function, plan, &cleanup_root_names)
                .expect("runtime params should bind");
        let resume_plan = &prepared
            .deopt_resume
            .function(function.function_id)
            .expect("missing typed resume plan")
            .resume_plan;
        let seeds = planned_stack_slot_entry_seeds_for_typed_function(function, plan, resume_plan);
        let block_indices_by_label = typed_block_indices_by_label(function);
        let else_index =
            typed_block_index_for_label(function, &block_indices_by_label, if_term.else_label);

        let else_params = &runtime_params[else_index];
        assert!(
            else_params.iter().all(|param| param.binding.name != "x"),
            "cleanup-root-only local should be kept in the frame root instead of a runtime param: {else_params:#?}"
        );
        assert!(
            seeds[else_index]
                .iter()
                .all(|seed| seed.binding.name != "x"),
            "cleanup-only locals should not require stack-slot entry seeds"
        );
    }

    #[test]
    fn cleanup_root_exception_prefix_keeps_untaken_operand_owned() {
        use soac_core::block_py::{
            BlockEdge, ComprehensionInsert, ComprehensionInsertKind, Del, InstrId, InstrKey, Load,
            Meta, NameLike, NameLocation, ResolvedName, StorageLayout, Store, TakeOperand,
            WithMeta,
        };
        use soac_ir_typed::TypedCall;
        use soac_opt::passes::{BlockRefcountPlan, RefcountAction, RefcountLocal, RefcountSite};

        for acquired_in_same_block in [false, true] {
            let (prepared, index) = prepared_typed_function("def run():\n    return None\n", "run");
            let mut function = prepared.module.callable_defs[index].clone();
            let mut layout = StorageLayout {
                stack_slots: vec!["container".into(), "value".into()],
                ..StorageLayout::default()
            };
            layout.mark_expression_temporary(LocalLocation(0));
            layout.mark_expression_temporary(LocalLocation(1));
            let name = |slot: u32| ResolvedName {
                id: format!("display_alias_{slot}").into(),
                location: NameLocation::Local(LocalLocation(slot)),
            };
            let meta = |index| Meta {
                instr_id: Some(InstrId::new(index)),
                ..Meta::default()
            };
            let call = |builtin: &str, args| {
                InstrTyped::CallTyped(TypedCall::generic(
                    InstrTyped::Load(Load::new(ResolvedName::runtime_name(builtin))),
                    args,
                    vec![],
                ))
            };
            let none = || InstrTyped::Load(Load::new(ResolvedName::runtime_name("NONE")));
            let insert = ComprehensionInsert::new(
                ComprehensionInsertKind::DictSetItem,
                name(0),
                Some(Box::new(call(
                    "tuple",
                    vec![soac_core::block_py::CallArgPositional::Positional(none())],
                ))),
                Box::new(InstrTyped::TakeOperand(TakeOperand::new(name(1)))),
            );
            insert.validate_resolved(&layout).unwrap();
            let mut entry = function.entry_block().clone();
            entry.label = BlockLabel::from_index(0);
            entry.exc_edge = None;
            entry.body = vec![
                InstrTyped::Store(
                    Store::new(name(0), Box::new(call("dict", vec![]))).with_meta(meta(1)),
                ),
                InstrTyped::Store(
                    Store::new(name(1), Box::new(call("list", vec![]))).with_meta(meta(2)),
                ),
            ];
            let source_index = usize::from(!acquired_in_same_block);
            let handler_index = source_index + 1;
            let handler_label = BlockLabel::from_index(handler_index);
            let mut source = entry.clone();
            source.label = BlockLabel::from_index(source_index);
            source.body = if acquired_in_same_block {
                entry.body.clone()
            } else {
                vec![]
            };
            source.body.push(InstrTyped::ComprehensionInsert(insert));
            source.term = BlockTerm::Return(none());
            source.exc_edge = Some(BlockEdge::new(handler_label));
            let mut handler = entry.clone();
            handler.label = handler_label;
            handler.body = vec![InstrTyped::Del(Del::new(name(1), true).with_meta(meta(3)))];
            handler.term = BlockTerm::Return(none());
            handler.exc_edge = None;
            entry.term = BlockTerm::Jump(BlockEdge::new(source.label));
            function.blocks = if acquired_in_same_block {
                vec![source, handler]
            } else {
                vec![entry, source, handler]
            };
            function.storage_layout = Some(layout);
            let local = |slot: u32| RefcountLocal {
                location: LocalLocation(slot),
                name: if slot == 0 { "container" } else { "value" }.into(),
                cleanup_order: (true, slot),
            };
            let mut refcounts = FunctionRefcountPlan::default();
            refcounts.blocks.insert(
                BlockLabel::from_index(0),
                BlockRefcountPlan {
                    label: BlockLabel::from_index(0),
                    actions: (0..2)
                        .map(|slot| RefcountAction {
                            site: RefcountSite::Instr(InstrKey::new(
                                function.function_id,
                                InstrId::new(slot + 1),
                            )),
                            kind: RefcountActionKind::RebindLocal {
                                local: local(slot),
                                old_state: LocalRefState::Unbound,
                                new_state: LocalRefState::Owned,
                            },
                        })
                        .collect(),
                },
            );
            refcounts.blocks.insert(
                handler_label,
                BlockRefcountPlan {
                    label: handler_label,
                    actions: vec![RefcountAction {
                        site: RefcountSite::Instr(InstrKey::new(
                            function.function_id,
                            InstrId::new(3),
                        )),
                        kind: RefcountActionKind::DeleteLocal {
                            local: local(1),
                            old_state: LocalRefState::Owned,
                        },
                    }],
                },
            );
            let mut dispatches = vec![None; function.blocks.len()];
            dispatches[source_index] = Some(BlockExcDispatchPlan {
                target_index: handler_index,
                slot_writes: vec![],
                target_args: vec![],
                target_arg_ref_kinds: vec![],
                forwarded_local_names: vec![],
                borrowed_forwarded_local_names: HashSet::new(),
                release_local_names: vec![],
                drop_forwarded_local_names: vec![],
            });
            let plan = super::planned_cleanup_root_slot_states_for_typed_function(
                &function,
                &FunctionLocalPlan::default(),
                &refcounts,
                &HashSet::from(["container".into(), "value".into()]),
                &dispatches,
                &vec![HashMap::new(); function.blocks.len()],
                &[],
                &HashSet::new(),
                &HashSet::from([LocalLocation(0), LocalLocation(1)]),
                &HashSet::new(),
            );
            let source_label = BlockLabel::from_index(source_index);
            assert_eq!(
                plan.exit_state_for_block(source_label)["value"],
                CleanupRootSlotState::NoOwnedReference
            );
            assert_eq!(
                plan.entry_state_for_block(handler_label)["value"],
                CleanupRootSlotState::MaybeOwnedReference,
                "tuple(None) can fail before the different value operand is taken; same_block={acquired_in_same_block}",
            );
            assert_eq!(
                plan.previous_state_for_instr(
                    InstrKey::new(function.function_id, InstrId::new(3)),
                    "value"
                ),
                CleanupRootSlotState::MaybeOwnedReference,
                "handler cleanup must read the actual nullable owner, not elide its release",
            );
            assert_eq!(
                plan.union_exit_states()["value"],
                CleanupRootSlotState::MaybeOwnedReference
            );
        }
    }

    #[test]
    fn cleanup_root_exception_prefix_forgets_null_interval_before_rebind() {
        use soac_core::block_py::{
            CallArgPositional, InstrId, InstrKey, Load, Meta, NameLike, NameLocation, ResolvedName,
            StorageLayout, Store, TakeOperand, WithMeta,
        };
        use soac_ir_typed::{TypedCall, ValueFacts};
        use soac_opt::passes::{BlockRefcountPlan, RefcountAction, RefcountLocal, RefcountSite};

        let (prepared, index) = prepared_typed_function("def run():\n    return None\n", "run");
        let function = &prepared.module.callable_defs[index];
        let mut layout = StorageLayout {
            stack_slots: vec!["value".into()],
            ..StorageLayout::default()
        };
        layout.mark_expression_temporary(LocalLocation(0));
        let name = ResolvedName {
            id: "display_alias".into(),
            location: NameLocation::Local(LocalLocation(0)),
        };
        let none = || InstrTyped::Load(Load::new(ResolvedName::runtime_name("NONE")));
        let raising_child = InstrTyped::CallTyped(TypedCall::generic(
            InstrTyped::Load(Load::new(ResolvedName::runtime_name("tuple"))),
            vec![CallArgPositional::Positional(none())],
            vec![],
        ));
        let mut value = InstrTyped::CallTyped(TypedCall::generic(
            InstrTyped::Load(Load::new(ResolvedName::runtime_name("getattr"))),
            vec![
                CallArgPositional::Positional(InstrTyped::TakeOperand(TakeOperand::new(
                    name.clone(),
                ))),
                CallArgPositional::Positional(raising_child),
            ],
            vec![],
        ));
        let non_null = PyObjFacts::unknown().with_non_null_ref();
        value.typed_extra_mut().unwrap().result_facts = Some(ValueFacts::PyObj(non_null));
        let instr_id = InstrId::new(1);
        let mut block = function.entry_block().clone();
        block.body = vec![InstrTyped::Store(
            Store::new(name, Box::new(value)).with_meta(Meta {
                instr_id: Some(instr_id),
                ..Meta::default()
            }),
        )];
        block.term = BlockTerm::Return(none());
        let refcounts = FunctionRefcountPlan {
            blocks: HashMap::from([(
                block.label,
                BlockRefcountPlan {
                    label: block.label,
                    actions: vec![RefcountAction {
                        site: RefcountSite::Instr(InstrKey::new(function.function_id, instr_id)),
                        kind: RefcountActionKind::RebindLocal {
                            local: RefcountLocal {
                                location: LocalLocation(0),
                                name: "value".into(),
                                cleanup_order: (true, 0),
                            },
                            old_state: LocalRefState::Owned,
                            new_state: LocalRefState::Owned,
                        },
                    }],
                },
            )]),
        };
        let facts = super::transfer_cleanup_root_slot_facts_for_block(
            function.function_id,
            &block,
            Some(&layout),
            &refcounts,
            &HashSet::from(["value".into()]),
            &HashMap::from([("value".into(), non_null)]),
            &HashMap::new(),
            &[],
            &HashSet::new(),
            &HashSet::from([LocalLocation(0)]),
            &HashSet::new(),
            None,
        );
        assert_eq!(
            facts.normal["value"], non_null,
            "successful rebind stays precise"
        );
        assert!(
            facts.exceptional.is_empty(),
            "the later tuple(None) failure observes the NULL slot before the rebind",
        );
    }

    #[test]
    fn cleanup_root_take_without_actions_clears_normal_maps_facts_and_terms() {
        use soac_core::block_py::{
            Load, NameLike, NameLocation, ResolvedName, StorageLayout, TakeOperand,
        };
        let (prepared, index) = prepared_typed_function("def run():\n    return None\n", "run");
        let function = &prepared.module.callable_defs[index];
        let mut layout = StorageLayout {
            stack_slots: vec!["value".into()],
            ..StorageLayout::default()
        };
        layout.mark_expression_temporary(LocalLocation(0));
        let take = || {
            InstrTyped::TakeOperand(TakeOperand::new(ResolvedName {
                id: "not_the_physical_name".into(),
                location: NameLocation::Local(LocalLocation(0)),
            }))
        };
        let empty = CleanupRootSlotState::NoOwnedReference;
        let owned = CleanupRootSlotState::MaybeOwnedReference;
        let tracked = HashSet::from(["value".to_owned()]);
        let indices = HashMap::from([("value".to_owned(), 0)]);
        let initial = HashMap::from([("value".to_owned(), owned)]);
        let initial_facts = HashMap::from([(
            "value".to_owned(),
            PyObjFacts::unknown().with_non_null_ref(),
        )]);
        for terminal_kind in 0..3 {
            let mut block = function.entry_block().clone();
            block.body.clear();
            block.term = match terminal_kind {
                0 => {
                    block.body.push(take());
                    BlockTerm::Return(InstrTyped::Load(Load::new(ResolvedName::runtime_name(
                        "NONE",
                    ))))
                }
                1 => BlockTerm::Return(take()),
                _ => BlockTerm::GeneratorReturn(take()),
            };
            let refcounts = FunctionRefcountPlan::default();
            let map = super::transfer_cleanup_root_slot_state_for_block(
                function.function_id,
                &block,
                Some(&layout),
                &refcounts,
                &tracked,
                &initial,
                &HashMap::new(),
                &[],
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
            );
            let dense = super::transfer_dense_cleanup_root_slot_state_for_block(
                function.function_id,
                &block,
                Some(&layout),
                &refcounts,
                &indices,
                &[owned],
                &HashMap::new(),
                &[],
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
            );
            let facts = super::transfer_cleanup_root_slot_facts_for_block(
                function.function_id,
                &block,
                Some(&layout),
                &refcounts,
                &tracked,
                &initial_facts,
                &HashMap::new(),
                &[],
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
            );
            assert_eq!(map.normal["value"], empty);
            assert_eq!(dense.normal, [empty]);
            assert!(facts.normal.is_empty());
            assert_eq!(map.exceptional["value"], owned);
            assert_eq!(dense.exceptional, [owned]);
            assert!(
                facts.exceptional.is_empty(),
                "nullable prefix cleanup cannot reuse non-null facts after a move"
            );
        }
    }

    #[test]
    fn call_argument_singleton_preserves_forwarded_immortal_keyword_owner() {
        use soac_core::block_py::{
            BuildCollectionKind, CallArgumentOp, CallArgumentOpKind, ChildVisitable, PreparedCall,
            Store, Visit,
        };

        #[derive(Default)]
        struct CallPhases {
            stores: Vec<Store<InstrTyped>>,
            calls: Vec<PreparedCall<InstrTyped>>,
            phases: Vec<CallArgumentOp<InstrTyped>>,
        }
        impl Visit<InstrTyped> for CallPhases {
            fn visit_instr(&mut self, instr: &InstrTyped) {
                match instr {
                    InstrTyped::Store(op) => self.stores.push(op.clone()),
                    InstrTyped::PreparedCall(op) => self.calls.push(op.clone()),
                    InstrTyped::CallArgumentOp(op) => self.phases.push(op.clone()),
                    _ => {}
                }
                instr.visit_children(self);
            }
        }

        // This is the source shape of the maintained expanded-argument runtime
        // regression. The conditional creates real block edges between the
        // keyword-name acquisition and the dictionary's consuming key Take.
        let (mut prepared, index) = prepared_typed_function(
            r#"
def singleton(callee, source, predicate, value):
    return callee()(*source(), tail=value() if predicate() else None)
"#,
            "singleton",
        );
        let function = &mut prepared.module.callable_defs[index];
        soac_opt::passes::linearize_typed_function_expressions(function)
            .expect("production late expression preparation should succeed");
        soac_ir_typed::assign_missing_typed_function_instr_ids(function);
        super::super::refresh_typed_function_value_facts(function);
        let prepared = super::plan_jit_typed_module(prepared.module, prepared.value_facts)
            .expect("plan the actual post-linearization call phases");
        let function = &prepared.module.callable_defs[index];
        let layout = function.storage_layout().as_ref().unwrap();
        let plan = prepared.locals.function(function.function_id).unwrap();
        let resume = &prepared
            .deopt_resume
            .function(function.function_id)
            .unwrap()
            .resume_plan;
        let mut phases = CallPhases::default();
        phases.visit_fn(function);
        let [call] = phases.calls.as_slice() else {
            panic!("the source call must select exactly one prepared invocation");
        };
        call.validate_resolved(layout).unwrap();
        let [normalize] = phases.phases.as_slice() else {
            panic!("a singleton star must have one normalization, not list expansion");
        };
        assert_eq!(normalize.kind, CallArgumentOpKind::NormalizeSingletonStar);
        let InstrTyped::TakeOperand(arguments) = call.arguments.as_ref() else {
            panic!("the invocation must consume the same normalized argument primary");
        };
        assert_eq!(normalize.buffer.location, arguments.name.location);
        let Some(InstrTyped::TakeOperand(keywords)) = call.keywords.as_deref() else {
            panic!("the invocation must consume its keyword dictionary");
        };
        let dictionary = phases
            .stores
            .iter()
            .find(|store| store.name.location == keywords.name.location)
            .and_then(|store| match store.value.as_ref() {
                InstrTyped::BuildCollection(op) => Some(op),
                _ => None,
            })
            .expect("the actual named group must use native dictionary construction");
        assert_eq!(dictionary.kind, BuildCollectionKind::Dict);
        let [InstrTyped::TakeOperand(key), InstrTyped::TakeOperand(_)] =
            dictionary.values.as_slice()
        else {
            panic!("the single named argument must consume its key and value owners");
        };
        let key_location = key
            .validate_resolved(layout)
            .unwrap()
            .local_location()
            .expect("an ordinary function stores the key in a local Operand");
        let key_name = &layout.stack_slots()[key_location.slot() as usize];
        let key_store = phases
            .stores
            .iter()
            .find(|store| store.name.local_location() == Some(key_location))
            .expect("the dictionary key must have an explicit producer");
        assert!(matches!(
            key_store.value.as_ref(),
            InstrTyped::Load(load) if load.name.location.as_constant().is_some()
        ));
        assert!(plan.boxed_owner_local_locations.contains(&key_location));
        assert!(plan.cleanup_root_names.contains(key_name));

        let entries = plan
            .entry_materializations
            .iter()
            .enumerate()
            .flat_map(|(index, entries)| entries.iter().map(move |entry| (index, entry)))
            .filter(|(_, entry)| {
                entry.binding.location == key_location
                    && entry.entry_ref_kind == LocalRefKind::Immortal
            })
            .collect::<Vec<_>>();
        assert!(
            !entries.is_empty(),
            "the captured keyword must cross a real CFG edge"
        );
        for (index, entry) in entries {
            let label = function.blocks[index].label;
            assert_eq!(entry.repr, RuntimeBlockParamRepr::PyObject);
            assert_eq!(entry.binding.storage, PlannedLocalStorage::BlockParam);
            assert!(matches!(
                entry.source,
                PlannedLocalEnvEntrySource::BlockParam { .. }
            ));
            assert_eq!(
                plan.cleanup_root_slot_states.entry_state_for_block(label)[key_name],
                CleanupRootSlotState::NoOwnedReference,
                "an immortal SSA acquisition has not published a physical stack owner"
            );
            let resumed = resume
                .block_entry(function.function_id, label)
                .and_then(|entry| {
                    entry
                        .locals
                        .iter()
                        .find(|local| local.location == key_location)
                })
                .expect("deopt must retain the same live keyword binding");
            assert_eq!(resumed.ownership, LocalRefKind::Immortal);
            assert_eq!(
                resumed.source,
                LocalEnvResumeValueSource::BlockParam(key_location)
            );
            assert_eq!(
                super::super::local_env_storage_for_block_param(entry, true),
                super::super::LocalEnvStorage::LocalOnly,
                "{key_name} in {label} must read the forwarded constant, not an unpopulated stack mirror: {entry:?}"
            );
        }
    }

    #[test]
    fn call_argument_replacement_invalidates_old_owner_facts_without_refcount_actions() {
        use soac_core::block_py::{
            CallArgumentOp, CallArgumentOpKind, Load, NameLike, NameLocation, ResolvedName,
            StorageLayout,
        };
        let (prepared, index) = prepared_typed_function("def run():\n    return None\n", "run");
        let function = &prepared.module.callable_defs[index];
        let mut layout = StorageLayout {
            stack_slots: vec!["callable".into(), "buffer".into()],
            ..StorageLayout::default()
        };
        for slot in 0..2 {
            layout.mark_expression_temporary(LocalLocation(slot));
        }
        let name = |slot| ResolvedName {
            id: "display_name_is_not_storage".into(),
            location: NameLocation::Local(LocalLocation(slot)),
        };
        let initial = HashMap::from([(
            "buffer".to_owned(),
            PyObjFacts::unknown().with_non_null_ref(),
        )]);
        let tracked = HashSet::from(["buffer".to_owned()]);
        for kind in [
            CallArgumentOpKind::ExtendPositional,
            CallArgumentOpKind::MergeKeywords,
            CallArgumentOpKind::FinishPositionalList,
            CallArgumentOpKind::NormalizeSingletonStar,
        ] {
            let none = || InstrTyped::Load(Load::new(ResolvedName::runtime_name("NONE")));
            let phase = CallArgumentOp::new(
                kind,
                name(0),
                name(1),
                kind.has_value().then(|| Box::new(none())),
            );
            phase.validate_resolved(&layout).unwrap();
            let mut block = function.entry_block().clone();
            block.body = vec![InstrTyped::CallArgumentOp(phase)];
            block.term = BlockTerm::Return(none());
            let facts = super::transfer_cleanup_root_slot_facts_for_block(
                function.function_id,
                &block,
                Some(&layout),
                &FunctionRefcountPlan::default(),
                &tracked,
                &initial,
                &HashMap::new(),
                &[],
                &HashSet::new(),
                &HashSet::new(),
                &HashSet::new(),
                None,
            );
            if kind.replaces_buffer() {
                assert!(
                    facts.normal.is_empty(),
                    "new tuple cannot reuse old raw-buffer facts"
                );
                assert!(
                    facts.exceptional.is_empty(),
                    "failed conversion cannot inherit a non-null endpoint fact"
                );
            } else {
                assert_eq!(facts.normal, initial);
                assert_eq!(
                    facts.exceptional, initial,
                    "failed mutation retains its buffer owner"
                );
            }
        }
    }

    #[test]
    fn cleanup_root_slot_state_tracks_empty_first_store_and_later_overwrite() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    x = []
    if flag:
        pass
    x = []
    return None
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");

        assert!(
            plan.cleanup_root_names.contains("x"),
            "expected edge-retired local x to be a cleanup root: {:?}",
            plan.cleanup_root_names
        );
        let previous_states = plan
            .cleanup_root_slot_states
            .instr_previous_states
            .values()
            .filter_map(|states| states.get("x").copied())
            .collect::<Vec<_>>();
        assert!(
            previous_states.contains(&CleanupRootSlotState::NoOwnedReference),
            "first cleanup-root store should know the slot starts empty: {previous_states:?}"
        );
        assert!(
            previous_states.contains(&CleanupRootSlotState::MaybeOwnedReference),
            "cleanup-root overwrite should preserve the previous-slot cleanup obligation: {previous_states:?}"
        );
        assert!(
            plan.cleanup_root_slot_states
                .block_exit_states
                .values()
                .any(|states| states.get("x") == Some(&CleanupRootSlotState::MaybeOwnedReference)),
            "at least one exit should still sweep the final x root"
        );
    }

    #[test]
    fn maybe_bound_local_keeps_owned_cleanup_across_finally_join() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(factory, flag, tick):
    try:
        if flag:
            value = factory()
    finally:
        tick()
    return value
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");
        let return_block = function
            .blocks
            .iter()
            .find(|block| {
                matches!(&block.term, BlockTerm::Return(InstrTyped::Load(load))
                    if load.name.id.as_str() == "value")
            })
            .expect("expected the conditional local return");
        let binding = binding_for_name(plan.local_plan.block(return_block.label).unwrap(), "value");
        assert_eq!(binding.param_facts.binding, ParamBindingFacts::MaybeUnbound);
        assert_eq!(binding.param_facts.ownership, LocalRefKind::Unknown);
        assert!(
            plan.refcount_plan
                .block(return_block.label)
                .unwrap()
                .actions
                .iter()
                .any(|action| matches!(&action.kind,
                    RefcountActionKind::ReleaseLocal { local, state: LocalRefState::Owned,
                        reason: RefcountReleaseReason::Return } if local.name == "value")),
            "a nullable incoming local still owns its non-null runtime reference"
        );
    }

    #[test]
    fn maybe_bound_local_delete_keeps_owned_cleanup_across_finally_join() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(factory, flag, tick):
    try:
        if flag:
            value = factory()
    finally:
        tick()
    del value
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");
        let deletes = plan
            .refcount_plan
            .blocks
            .values()
            .flat_map(|block| &block.actions)
            .filter_map(|action| match &action.kind {
                RefcountActionKind::DeleteLocal { local, old_state } if local.name == "value" => {
                    Some(*old_state)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(deletes, [LocalRefState::Owned]);
    }

    #[test]
    fn call_temporary_is_unbound_before_its_producer_and_after_exception_unwind() {
        let (mut prepared, function_index) = prepared_typed_function(
            include_str!(
                "../../../../tests/integration_modules/yield_from_throw_clears_delegate.py"
            ),
            "throw_check",
        );
        let function = &mut prepared.module.callable_defs[function_index];
        let linearization = soac_opt::passes::linearize_typed_function_expressions(function)
            .expect("the production late expression pass should succeed");
        assert!(linearization.lifted_nested_exprs > 0);
        soac_ir_typed::assign_missing_typed_function_instr_ids(function);
        super::super::refresh_typed_function_value_facts(function);
        let prepared = super::plan_jit_typed_module(prepared.module, prepared.value_facts)
            .expect("replan the actual post-linearization ownership boundary");
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared.locals.function(function.function_id).unwrap();
        let (source_index, factory_result) = function
            .blocks
            .iter()
            .enumerate()
            .find_map(|(index, block)| {
                block.body.iter().find_map(|instr| match instr {
                    InstrTyped::Store(store)
                        if block.exc_edge.is_some()
                            && matches!(store.value.as_ref(), InstrTyped::CallTyped(_)) =>
                    {
                        Some((
                            index,
                            store
                                .name
                                .local_location()
                                .expect("linearized result has a local"),
                        ))
                    }
                    _ => None,
                })
            })
            .expect("nested call should have an explicit temporary result");
        let handlers = function
            .blocks
            .iter()
            .filter_map(|block| block.exc_edge.as_ref())
            .map(|edge| {
                function
                    .blocks
                    .iter()
                    .position(|block| block.label == edge.target)
                    .unwrap()
            })
            .collect::<HashSet<_>>();
        assert!(
            !handlers.is_empty(),
            "the fixture needs real exceptional edges"
        );
        assert!(
            handlers
                .iter()
                .all(|index| plan.runtime_block_params[*index]
                    .iter()
                    .all(|param| param.binding.location != factory_result)),
            "a dead expression operand must unwind before the exceptional successor"
        );
        assert!(
            handlers
                .iter()
                .all(
                    |index| plan.entry_materializations[*index].iter().any(|entry| entry
                        .binding
                        .location
                        == factory_result
                        && entry.entry_ref_kind == LocalRefKind::Unbound
                        && entry.source == PlannedLocalEnvEntrySource::Unbound)
                ),
            "the handler cannot inherit an already-retired operand as a frame root"
        );
        assert!(
            plan.entry_materializations[source_index]
                .iter()
                .any(|entry| entry.binding.location == factory_result
                    && entry.entry_ref_kind == LocalRefKind::Unbound
                    && entry.source == PlannedLocalEnvEntrySource::Unbound),
            "an exception before the producer must see proven-unbound, not a missing value"
        );
    }

    #[test]
    fn cleanup_root_slot_facts_do_not_reuse_block_param_value_facts_across_edges() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    x = (1,)
    if flag:
        pass
    x = (2,)
    return None
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");

        let previous_facts = plan
            .cleanup_root_slot_states
            .instr_previous_facts
            .values()
            .filter_map(|facts| facts.get("x").copied())
            .collect::<Vec<_>>();
        assert!(
            previous_facts.iter().all(|facts| !facts.is_non_null_ref()),
            "block-param value facts must not be reused as old cleanup-root slot facts: {previous_facts:?}"
        );
    }

    #[test]
    fn cleanup_root_entry_facts_ignore_block_param_values() {
        let block_param_label = BlockLabel::from_index(0);
        let stack_slot_label = BlockLabel::from_index(1);
        let tracked_slot_names = HashSet::from(["x".to_string()]);
        let local_plan = FunctionLocalPlan {
            blocks: HashMap::from([
                (
                    block_param_label,
                    BlockLocalPlan {
                        label: block_param_label,
                        entry_locals: vec![PlannedLocalBinding {
                            name: "x".to_string(),
                            location: LocalLocation(0),
                            storage: PlannedLocalStorage::BlockParam,
                            param_facts: BlockParamFacts {
                                value: Some(PyObjFacts::known_not_none()),
                                binding: ParamBindingFacts::DefinitelyBound,
                                provenance: ParamProvenance::ForwardedLocal(LocalLocation(0)),
                                ownership: LocalRefKind::Owned,
                            },
                        }],
                    },
                ),
                (
                    stack_slot_label,
                    BlockLocalPlan {
                        label: stack_slot_label,
                        entry_locals: vec![PlannedLocalBinding {
                            name: "x".to_string(),
                            location: LocalLocation(0),
                            storage: PlannedLocalStorage::StackSlot,
                            param_facts: BlockParamFacts {
                                value: Some(PyObjFacts::exact_type(PyExactType::Int)),
                                binding: ParamBindingFacts::CheckedLocalValue,
                                provenance: ParamProvenance::StackSlot(LocalLocation(0)),
                                ownership: LocalRefKind::Owned,
                            },
                        }],
                    },
                ),
            ]),
        };

        let block_param_facts =
            cleanup_root_entry_facts_for_block(&local_plan, block_param_label, &tracked_slot_names);
        let stack_slot_facts =
            cleanup_root_entry_facts_for_block(&local_plan, stack_slot_label, &tracked_slot_names);

        assert!(block_param_facts.is_empty());
        assert_eq!(
            stack_slot_facts.get("x"),
            Some(&PyObjFacts::exact_type(PyExactType::Int).without_non_null_ref())
        );
        assert!(
            !stack_slot_facts["x"].is_non_null_ref(),
            "checked-local cleanup facts must keep the null guard alive"
        );
    }

    #[test]
    fn cleanup_root_slot_state_keeps_branch_optional_roots_maybe_owned() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    if flag:
        x = []
    if flag:
        pass
    x = []
    return None
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");

        let previous_states = plan
            .cleanup_root_slot_states
            .instr_previous_states
            .values()
            .filter_map(|states| states.get("x").copied())
            .collect::<Vec<_>>();
        assert!(
            previous_states.contains(&CleanupRootSlotState::MaybeOwnedReference),
            "branch-optional root should stay nullable at the later overwrite: {previous_states:?}"
        );
    }

    #[test]
    fn cleanup_root_slot_state_treats_entry_param_alias_stores_as_borrowed() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(a, b):
    c = a
    d = c
    if d != b:
        return 1
    return 2
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");

        let borrowed_rebinds = plan
            .refcount_plan
            .blocks
            .values()
            .flat_map(|block| block.actions.iter())
            .filter(|action| {
                matches!(
                    &action.kind,
                    RefcountActionKind::RebindLocal {
                        local,
                        new_state: LocalRefState::Borrowed,
                        ..
                    } if local.name == "c" || local.name == "d"
                )
            })
            .count();
        assert_eq!(borrowed_rebinds, 2);
        assert!(
            !plan
                .refcount_plan
                .blocks
                .values()
                .flat_map(|block| block.actions.iter())
                .any(|action| matches!(
                    &action.kind,
                    RefcountActionKind::RebindLocal {
                        local,
                        new_state: LocalRefState::Owned,
                        ..
                    } if local.name == "c"
                )),
            "entry-parameter aliases should stay borrowed instead of taking ownership"
        );

        for block in &function.blocks {
            if !matches!(block.term, BlockTerm::Return(_)) {
                continue;
            }
            let states = plan
                .cleanup_root_slot_states
                .block_exit_states
                .get(&block.label)
                .unwrap_or_else(|| panic!("missing exit state for {}", block.label));
            let root_state = |name| {
                states
                    .get(name)
                    .copied()
                    .unwrap_or(CleanupRootSlotState::NoOwnedReference)
            };
            assert_eq!(
                root_state("a"),
                CleanupRootSlotState::NoOwnedReference,
                "borrowed entry parameter a should not be exit-swept"
            );
            assert_eq!(
                root_state("b"),
                CleanupRootSlotState::NoOwnedReference,
                "unaliased borrowed entry parameter b should not be exit-swept"
            );
            assert_eq!(
                root_state("c"),
                CleanupRootSlotState::NoOwnedReference,
                "borrowed alias c should not be exit-swept"
            );
            assert_eq!(
                root_state("d"),
                CleanupRootSlotState::NoOwnedReference,
                "borrowed alias d should not be exit-swept"
            );
        }
    }

    #[test]
    fn cleanup_root_slot_state_tracks_boxed_arithmetic_ownership() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
Ident1 = 1

def helper(value):
    if value:
        return Ident1
    return 0

def f(seq):
    IntLoc = 1
    while IntLoc <= 1:
        if helper(seq[IntLoc]) == Ident1:
            CharLoc = "A"
            IntLoc = IntLoc + 1
    if CharLoc == "X":
        return 1
    return 0
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");

        assert!(
            plan.cleanup_root_names.contains("IntLoc"),
            "expected edge-retired boxed IntLoc to remain a cleanup root: {:?}",
            plan.cleanup_root_names
        );
        let previous_states = plan
            .cleanup_root_slot_states
            .instr_previous_states
            .values()
            .filter_map(|states| states.get("IntLoc").copied())
            .collect::<Vec<_>>();
        assert!(
            !previous_states.is_empty(),
            "expected boxed arithmetic stores to record previous cleanup-root slot state"
        );
        assert!(
            previous_states.contains(&CleanupRootSlotState::NoOwnedReference),
            "the initial cleanup-root store should see an empty slot: {previous_states:?}"
        );
        assert!(
            previous_states.contains(&CleanupRootSlotState::MaybeOwnedReference),
            "later boxed arithmetic must preserve its owned cleanup-root value: {previous_states:?}"
        );
        assert!(
            plan.cleanup_root_slot_states
                .block_exit_states
                .values()
                .any(|states| states.get("IntLoc")
                    == Some(&CleanupRootSlotState::MaybeOwnedReference)),
            "boxed cleanup root IntLoc must be available for exit sweeping: {:?}",
            plan.cleanup_root_slot_states.block_exit_states
        );
    }

    #[test]
    fn local_plan_carries_maybe_unbound_live_ins_as_block_params() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    if flag:
        x = 1
    return x
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let entry_label = function.entry_block().label;
        let runtime_params = planned_jit_params_for_typed_function(function, plan, &HashSet::new())
            .expect("runtime params should bind");

        let non_entry_x_bindings = function
            .blocks
            .iter()
            .enumerate()
            .filter(|(_, block)| block.label != entry_label)
            .filter_map(|(index, _)| {
                runtime_params[index]
                    .iter()
                    .find(|param| param.arg_name == "x")
                    .map(|param| &param.binding)
            })
            .collect::<Vec<_>>();

        assert!(
            !non_entry_x_bindings.is_empty(),
            "expected at least one non-entry maybe-unbound x binding"
        );
        assert!(
            non_entry_x_bindings
                .iter()
                .all(|binding| binding.storage == PlannedLocalStorage::BlockParam),
            "maybe-unbound live-ins should travel through runtime block params: {non_entry_x_bindings:?}"
        );
        assert!(
            non_entry_x_bindings
                .iter()
                .all(|binding| binding.param_facts.binding == ParamBindingFacts::MaybeUnbound),
            "maybe-unbound live-ins should preserve checked local-load semantics: {non_entry_x_bindings:?}"
        );
        assert!(
            non_entry_x_bindings
                .iter()
                .all(|binding| binding.param_facts.provenance
                    == ParamProvenance::ForwardedLocal(binding.location)),
            "maybe-unbound live-ins should preserve forwarded-local provenance: {non_entry_x_bindings:?}"
        );
    }

    #[test]
    fn local_plan_carries_entry_maybe_unbound_live_ins_as_synthetic_block_params() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    if flag:
        x = 1
    return x
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let runtime_params = planned_jit_params_for_typed_function(function, plan, &HashSet::new())
            .expect("runtime params should bind");
        let resume_plan = &prepared
            .deopt_resume
            .function(function.function_id)
            .expect("missing typed resume plan")
            .resume_plan;
        let seeds = planned_stack_slot_entry_seeds_for_typed_function(function, plan, resume_plan);
        let entry_label = function.entry_block().label;
        let entry_plan = plan.block(entry_label).expect("missing entry local plan");
        let entry_x = binding_for_name(entry_plan, "x");
        let block_indices_by_label = typed_block_indices_by_label(function);
        let entry_index =
            typed_block_index_for_label(function, &block_indices_by_label, entry_label);

        assert_eq!(entry_x.storage, PlannedLocalStorage::BlockParam);
        assert_eq!(entry_x.param_facts.binding, ParamBindingFacts::MaybeUnbound);
        assert_eq!(entry_x.param_facts.ownership, LocalRefKind::Unbound);
        assert_eq!(
            entry_x.param_facts.provenance,
            ParamProvenance::SyntheticUnbound(entry_x.location)
        );
        assert!(
            runtime_params[entry_index]
                .iter()
                .any(|param| param.arg_name == "x"),
            "entry maybe-unbound local should be initialized as a runtime block param"
        );
        assert!(
            seeds[entry_index]
                .iter()
                .all(|seed| seed.binding.name != "x"),
            "entry maybe-unbound local should not require a stack-slot seed"
        );
    }

    #[test]
    fn refcount_plan_is_available_to_jit_planning() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f():
    x = []
    return None
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = &prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan")
            .refcount_plan;

        assert!(plan.blocks.values().any(|block| {
            block.actions.iter().any(|action| {
                matches!(
                    &action.kind,
                    RefcountActionKind::ReleaseLocal {
                        local,
                        reason: RefcountReleaseReason::Return,
                        ..
                    } if local.name == "x"
                )
            })
        }));
    }

    #[test]
    fn planned_jit_function_locals_collects_blockpy_local_state() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(flag):
    x = []
    try:
        if flag:
            raise ValueError("boom")
        return x
    except ValueError:
        return None
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");

        assert_eq!(plan.local_plan.blocks.len(), function.blocks.len());
        assert_eq!(plan.runtime_block_params.len(), function.blocks.len());
        assert_eq!(plan.implicit_target_transports.len(), function.blocks.len());
        assert_eq!(plan.jump_edge_transports.len(), function.blocks.len());
        assert_eq!(plan.stack_slot_entry_seeds.len(), function.blocks.len());
        assert_eq!(plan.entry_materializations.len(), function.blocks.len());
        assert_eq!(plan.exc_dispatches.len(), function.blocks.len());
        assert!(
            plan.cleanup_root_names.contains("x"),
            "expected edge-released local x to be planned as a cleanup root: {:?}",
            plan.cleanup_root_names
        );
        let materialization_count = plan
            .entry_materializations
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        let runtime_param_count = plan
            .runtime_block_params
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        let stack_seed_count = plan
            .stack_slot_entry_seeds
            .iter()
            .map(Vec::len)
            .sum::<usize>();
        let unbound_count = plan
            .entry_materializations
            .iter()
            .flatten()
            .filter(|entry| matches!(entry.source, PlannedLocalEnvEntrySource::Unbound))
            .count();
        assert_eq!(
            materialization_count,
            runtime_param_count + stack_seed_count + unbound_count
        );
        assert!(
            plan.entry_materializations
                .iter()
                .flatten()
                .any(|entry| matches!(entry.source, PlannedLocalEnvEntrySource::BlockParam { .. })),
            "expected block-param entry materialization to be represented in the pre-codegen plan"
        );
        assert!(
            plan.exc_dispatches.iter().any(Option::is_some),
            "expected exception dispatches to be represented in the pre-codegen plan"
        );
        assert!(
            plan.refcount_plan
                .blocks
                .values()
                .flat_map(|block| block.actions.iter())
                .any(|action| matches!(action.kind, RefcountActionKind::ReleaseLocal { .. })),
            "expected refcount releases to be represented in the pre-codegen plan"
        );
        let required_stack_slot_names = plan.required_stack_slot_names_for_function(function);
        assert!(
            required_stack_slot_names.iter().any(|name| name == "x"),
            "expected edge-retired local x to require a cleanup-root slot: {required_stack_slot_names:?}"
        );
        assert!(
            required_stack_slot_names.iter().any(|name| [
                BlockParamRole::Exception,
                BlockParamRole::EnclosingException
            ]
            .into_iter()
            .any(|role| local_name_has_block_parameter_role(
                function.storage_layout().as_ref(),
                name,
                role
            ))),
            "expected exception state stack slots to be represented in the pre-codegen plan: {required_stack_slot_names:?}"
        );
    }

    #[test]
    fn planned_jit_function_locals_rejects_tampered_cfg_loop_backedges() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f(value):
    while value:
        value -= 1
    return value
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let mut plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan")
            .clone();

        assert!(
            !plan.loop_backedges.is_empty(),
            "a genuine source loop must have a planned CFG backedge"
        );
        plan.validate_for_typed_function(function)
            .expect("the original CFG backedge plan should validate");

        plan.loop_backedges.clear();
        assert!(
            plan.validate_for_typed_function(function).is_err(),
            "a tampered CFG backedge plan must be rejected before code generation"
        );
    }

    #[test]
    fn planned_jit_function_locals_rejects_exception_only_cycles() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f():
    return None
"#,
            "f",
        );
        let mut function = prepared.module.callable_defs[function_index].clone();
        let entry_label = function.blocks[0].label;
        function.blocks[0].exc_edge = Some(soac_core::block_py::BlockEdge::new(entry_label));
        let block_indices_by_label = typed_block_indices_by_label(&function);

        assert!(
            super::planned_loop_backedges_for_typed_function(&function, &block_indices_by_label,)
                .is_err(),
            "exception-only cycles must fail explicitly rather than silently skip pending events"
        );
    }

    #[test]
    fn planned_local_env_entry_materializations_preserve_sources() {
        let block_binding = PlannedLocalBinding {
            name: "x".to_string(),
            location: LocalLocation(0),
            storage: PlannedLocalStorage::BlockParam,
            param_facts: BlockParamFacts {
                value: None,
                binding: ParamBindingFacts::DefinitelyBound,
                provenance: ParamProvenance::ExplicitBlockParam(LocalLocation(0)),
                ownership: LocalRefKind::Owned,
            },
        };
        let stack_binding = PlannedLocalBinding {
            name: "y".to_string(),
            location: LocalLocation(1),
            storage: PlannedLocalStorage::StackSlot,
            param_facts: BlockParamFacts {
                value: None,
                binding: ParamBindingFacts::CheckedLocalValue,
                provenance: ParamProvenance::StackSlot(LocalLocation(1)),
                ownership: LocalRefKind::Owned,
            },
        };
        let stack_runtime_binding = PlannedLocalBinding {
            name: "z".to_string(),
            location: LocalLocation(2),
            storage: PlannedLocalStorage::StackSlot,
            param_facts: BlockParamFacts {
                value: None,
                binding: ParamBindingFacts::DefinitelyBound,
                provenance: ParamProvenance::ForwardedLocal(LocalLocation(2)),
                ownership: LocalRefKind::Owned,
            },
        };
        let runtime_params = vec![vec![
            RuntimeBlockParamPlan {
                arg_name: "x".to_string(),
                binding: block_binding.clone(),
                entry_aliases: vec!["x_alias".to_string()],
                repr: RuntimeBlockParamRepr::PyObject,
            },
            RuntimeBlockParamPlan {
                arg_name: "z".to_string(),
                binding: stack_runtime_binding.clone(),
                entry_aliases: Vec::new(),
                repr: RuntimeBlockParamRepr::PyObject,
            },
        ]];
        let stack_slot_entry_seeds = vec![vec![PlannedStackSlotEntrySeed {
            binding: stack_binding.clone(),
            entry_ref_kind: LocalRefKind::Borrowed,
        }]];

        let block_plan = BlockLocalPlan {
            label: BlockLabel::from_index(0),
            entry_locals: vec![
                block_binding.clone(),
                stack_binding.clone(),
                stack_runtime_binding.clone(),
            ],
        };
        let entries = planned_local_env_entry_materializations_for_function(
            &[&block_plan],
            &runtime_params,
            &stack_slot_entry_seeds,
            &HashSet::new(),
        )
        .expect("entry materialization planning should succeed");

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].len(), 3);
        assert_eq!(entries[0][0].binding, block_binding);
        assert_eq!(entries[0][0].entry_aliases, vec!["x_alias"]);
        assert_eq!(
            entries[0][0].source,
            PlannedLocalEnvEntrySource::BlockParam { param_index: 0 }
        );
        assert_eq!(entries[0][0].entry_ref_kind, LocalRefKind::Owned);
        assert_eq!(entries[0][1].binding, stack_runtime_binding);
        assert_eq!(
            entries[0][1].source,
            PlannedLocalEnvEntrySource::BlockParam { param_index: 1 }
        );
        assert_eq!(entries[0][1].entry_ref_kind, LocalRefKind::Borrowed);
        assert_eq!(entries[0][2].binding, stack_binding);
        assert_eq!(
            entries[0][2].source,
            PlannedLocalEnvEntrySource::StackSlotLoad
        );
        assert_eq!(entries[0][2].entry_ref_kind, LocalRefKind::Borrowed);
    }

    #[test]
    fn planned_local_env_entry_materializations_borrow_cleanup_root_block_params() {
        let block_binding = PlannedLocalBinding {
            name: "x".to_string(),
            location: LocalLocation(0),
            storage: PlannedLocalStorage::BlockParam,
            param_facts: BlockParamFacts {
                value: None,
                binding: ParamBindingFacts::DefinitelyBound,
                provenance: ParamProvenance::ForwardedLocal(LocalLocation(0)),
                ownership: LocalRefKind::Owned,
            },
        };
        let runtime_params = vec![vec![RuntimeBlockParamPlan {
            arg_name: "x".to_string(),
            binding: block_binding,
            entry_aliases: Vec::new(),
            repr: RuntimeBlockParamRepr::PyObject,
        }]];
        let cleanup_root_names = HashSet::from(["x".to_string()]);

        let block_plan = BlockLocalPlan {
            label: BlockLabel::from_index(0),
            entry_locals: vec![runtime_params[0][0].binding.clone()],
        };
        let entries = planned_local_env_entry_materializations_for_function(
            &[&block_plan],
            &runtime_params,
            &[Vec::new()],
            &cleanup_root_names,
        )
        .expect("entry materialization planning should succeed");

        assert_eq!(entries[0][0].entry_ref_kind, LocalRefKind::Borrowed);
    }

    #[test]
    fn planned_jit_module_locals_collects_all_functions() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def f(x):
    return x

def g(flag):
    if flag:
        y = []
    return None
"#,
        )
        .expect("lowering should succeed")
        .blockpy_module;
        let prepared = plan_typed_module_from_blockpy_module(&lowered);
        let plan = &prepared.locals;
        plan.validate_for_typed_module(&prepared.module)
            .expect("module-level function plan should validate");

        assert_eq!(plan.functions.len(), prepared.module.callable_defs.len());
        for function in &prepared.module.callable_defs {
            let function_plan = plan
                .function(function.function_id)
                .unwrap_or_else(|| panic!("missing plan for {}", function.names.qualname));
            function_plan
                .validate_for_typed_function(function)
                .expect("module-level function plan should validate");
        }
    }

    #[test]
    fn planned_jit_module_uses_precomputed_typed_pass_sidecars() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def f(flag):
    x = None
    if flag:
        return x
    return x
"#,
        )
        .expect("lowering should succeed")
        .blockpy_module;
        let prepared = plan_typed_module_from_blockpy_module(&lowered);

        prepared
            .locals
            .validate_for_typed_module(&prepared.module)
            .expect("JIT local plan from typed pass sidecars should validate");
        prepared
            .deopt_resume
            .validate_for_typed_module(&prepared.module)
            .expect("JIT deopt plan from typed pass sidecars should validate");
        assert_eq!(
            prepared.locals.functions.len(),
            prepared.module.callable_defs.len()
        );
    }

    #[test]
    fn jit_local_planning_accepts_sparse_block_labels() {
        let (mut lowered, function_index) = lowered_function(
            r#"
def f(flag):
    x = []
    if flag:
        y = x
    else:
        y = []
    return y
"#,
            "f",
        );
        sparsely_relabel_function_blocks(&mut lowered.callable_defs[function_index]);
        let function_id = lowered.callable_defs[function_index].function_id;
        let prepared = plan_typed_module_from_blockpy_module(&lowered);
        let function = prepared
            .module
            .callable_defs
            .iter()
            .find(|function| function.function_id == function_id)
            .expect("missing typed function");
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");

        plan.validate_for_typed_function(function)
            .expect("sparse-label JIT local plan should validate");
    }

    #[test]
    fn planned_jit_deopt_resume_module_wraps_validated_local_env_resume_plan() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def f():
    x = None
    del x
    return 1
"#,
        )
        .expect("lowering should succeed")
        .blockpy_module;
        let prepared = plan_typed_module_from_blockpy_module(&lowered);
        let plan = &prepared.deopt_resume;
        plan.validate_for_typed_module(&prepared.module)
            .expect("JIT deopt resume plan should validate");

        let function = prepared
            .module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("lowered function should exist");
        let function_plan = plan
            .function(function.function_id)
            .expect("function should have a JIT deopt resume plan");
        assert_eq!(
            function_plan.deopt_points.len(),
            function_plan.resume_plan.entries.len()
        );
        let entry_block = function.entry_block();
        let before_term_point = LocalEnvResumePoint::BeforeTerm {
            function_id: function.function_id,
            block: entry_block.label,
        };
        let before_term = function_plan
            .resume_plan
            .before_term(function.function_id, entry_block.label)
            .expect("resume plan should expose before-term lookup");
        let x_binding = before_term
            .binding_for_name("x")
            .expect("before-term resume state should include x");
        assert_eq!(x_binding.binding, LocalEnvResumeBindingState::Unbound);
        assert_eq!(x_binding.source, LocalEnvResumeValueSource::Unbound);

        let planned_deopt = function_plan
            .deopt_point(before_term_point)
            .expect("before-term resume point should have a planned deopt point");
        assert_eq!(planned_deopt.resume_point, before_term.point);
        assert_eq!(planned_deopt.precision, before_term.precision);
        assert_eq!(
            function_plan.deopt_point_by_id(planned_deopt.id),
            Some(planned_deopt)
        );

        let via_module = plan
            .entry(before_term_point)
            .expect("module-level point lookup should find before-term entry");
        assert_eq!(via_module, before_term);
        let via_module_deopt = plan
            .deopt_point(before_term_point)
            .expect("module-level point lookup should find planned deopt point");
        assert_eq!(via_module_deopt, planned_deopt);
    }

    #[test]
    fn deopt_only_stack_slot_resume_bindings_seed_frame_slots() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f():
    x = 1
    return x
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let local_plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let entry_label = function.entry_block().label;
        let entry_index = typed_block_index_for_label(
            function,
            &typed_block_indices_by_label(function),
            entry_label,
        );
        let binding = local_plan
            .block(entry_label)
            .and_then(|block| block.binding_for_name("x"))
            .cloned()
            .expect("entry block should plan local x");
        assert_eq!(binding.storage, PlannedLocalStorage::StackSlot);

        let resume_plan = FunctionLocalEnvResumePlan {
            entries: vec![LocalEnvResumeEntry {
                point: LocalEnvResumePoint::BeforeTerm {
                    function_id: function.function_id,
                    block: entry_label,
                },
                precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                locals: vec![LocalEnvResumeBinding {
                    name: binding.name.clone(),
                    location: binding.location,
                    binding: LocalEnvResumeBindingState::Bound,
                    source: LocalEnvResumeValueSource::StackSlot(binding.location),
                    ownership: binding.param_facts.ownership,
                    value: binding.param_facts.value,
                }],
            }],
        };

        let seeds =
            planned_stack_slot_entry_seeds_for_typed_function(function, local_plan, &resume_plan);
        assert!(
            seeds[entry_index]
                .iter()
                .any(|seed| seed.binding.name == "x"),
            "deopt-only stack-slot bindings must seed frame slots before codegen: {seeds:#?}"
        );
    }

    #[test]
    fn exc_dispatch_plan_for_handler_preserves_forwarded_live_in_local() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f():
    import builtins
    original_import = builtins.__import__
    try:
        raise ValueError("boom")
    except ValueError:
        pass
    return original_import is builtins.__import__
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let local_plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let runtime_params =
            planned_jit_params_for_typed_function(function, local_plan, &HashSet::new())
                .expect("runtime params should bind");
        let source_block = function
            .blocks
            .iter()
            .find(|block| block.exc_edge.is_some())
            .expect("expected exception edge source block");
        let exc_edge = source_block.exc_edge.as_ref().expect("checked above");
        let block_indices_by_label = typed_block_indices_by_label(function);
        let runtime_target_params = &runtime_params
            [typed_block_index_for_label(function, &block_indices_by_label, exc_edge.target)];
        let dispatch_plan = typed_exc_dispatch_plan(
            function,
            source_block,
            &runtime_target_params,
            &FunctionRefcountPlan::default(),
            &HashSet::new(),
        )
        .expect("expected exception dispatch plan");

        assert!(
            dispatch_plan
                .forwarded_local_names
                .iter()
                .any(|name| name == "original_import"),
            "exception dispatch should preserve original_import: {dispatch_plan:#?}"
        );
    }

    #[test]
    fn exc_dispatch_plan_leaves_cleanup_root_locals_in_frame_roots() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def f():
    try:
        x = []
        raise ValueError("boom")
    except ValueError:
        return None
    return None
"#,
            "f",
        );
        let function = &prepared.module.callable_defs[function_index];
        let local_plan = prepared
            .local_env_plan
            .function(function.function_id)
            .expect("missing typed local plan");
        let refcount_plan = &prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan")
            .refcount_plan;
        let cleanup_root_names = planned_cleanup_root_names_for_refcount_plan(
            refcount_plan,
            function.storage_layout().as_ref(),
        );
        let runtime_params =
            planned_jit_params_for_typed_function(function, local_plan, &cleanup_root_names)
                .expect("runtime params should bind");

        let dispatches = function
            .blocks
            .iter()
            .filter_map(|block| {
                let block_indices_by_label = typed_block_indices_by_label(function);
                let runtime_target_params = block
                    .exc_edge
                    .as_ref()
                    .map(|edge| {
                        let target_index = typed_block_index_for_label(
                            function,
                            &block_indices_by_label,
                            edge.target,
                        );
                        runtime_params[target_index].as_slice()
                    })
                    .unwrap_or(&[]);
                typed_exc_dispatch_plan(
                    function,
                    block,
                    runtime_target_params,
                    refcount_plan,
                    &cleanup_root_names,
                )
            })
            .collect::<Vec<_>>();

        assert!(
            dispatches.iter().all(|dispatch| !dispatch
                .forwarded_local_names
                .iter()
                .any(|name| name == "x")
                && !dispatch.release_local_names.iter().any(|name| name == "x")
                && !dispatch
                    .target_args
                    .iter()
                    .any(|arg| arg.target_name == "x")),
            "root-backed exception cleanup locals should stay in cleanup roots instead of \
             being forwarded through dispatch: {dispatches:#?}"
        );
    }

    #[test]
    fn exception_dispatch_ownership_validator_requires_one_sink_per_forwarded_local() {
        let (lowered, function_index) = lowered_function(
            r#"
def f():
    return None
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let block_label = function.entry_block().label;
        let dispatch = BlockExcDispatchPlan {
            target_index: 0,
            slot_writes: vec![(
                "slot_target".to_string(),
                BlockArg::Name("slot_only".to_string()),
            )],
            target_args: vec![RuntimeBlockArgPlan {
                target_name: "target_param".to_string(),
                source: BlockArg::Name("to_target".to_string()),
                repr: RuntimeBlockParamRepr::PyObject,
            }],
            target_arg_ref_kinds: vec![LocalRefKind::Owned],
            forwarded_local_names: vec![
                "slot_only".to_string(),
                "to_target".to_string(),
                "released".to_string(),
                "dropped".to_string(),
            ],
            release_local_names: vec!["released".to_string()],
            drop_forwarded_local_names: vec!["slot_only".to_string(), "dropped".to_string()],
            borrowed_forwarded_local_names: HashSet::new(),
        };
        let params = [exception_transport_test_param(
            "target_param",
            LocalRefKind::Owned,
        )];
        let cleanup_roots = HashSet::new();

        validate_exception_dispatch_ownership_sinks(
            function,
            block_label,
            &dispatch,
            &params,
            &cleanup_roots,
        )
        .expect("dispatch with one ownership sink per forwarded local should validate");

        let mut double_sink = dispatch.clone();
        double_sink
            .drop_forwarded_local_names
            .push("to_target".to_string());
        let err = validate_exception_dispatch_ownership_sinks(
            function,
            block_label,
            &double_sink,
            &params,
            &cleanup_roots,
        )
        .expect_err("targeted forwarded local should not also be dropped");
        assert!(
            err.contains("\"to_target\"") && err.contains("expected exactly one"),
            "expected a targeted+drop ownership-sink error, got: {err}"
        );

        let mut missing_sink = dispatch.clone();
        missing_sink
            .drop_forwarded_local_names
            .retain(|name| name != "slot_only");
        let err = validate_exception_dispatch_ownership_sinks(
            function,
            block_label,
            &missing_sink,
            &params,
            &cleanup_roots,
        )
        .expect_err("slot-write-only forwarded local still needs a planned drop sink");
        assert!(
            err.contains("\"slot_only\"") && err.contains("0 ownership sinks"),
            "expected a missing ownership-sink error, got: {err}"
        );
    }

    #[test]
    fn planned_drop_forwarded_local_names_excludes_targeted_and_released_locals() {
        let forwarded = vec![
            "slot_only".to_string(),
            "to_target".to_string(),
            "released".to_string(),
            "unused".to_string(),
        ];
        let target_args = vec![RuntimeBlockArgPlan {
            target_name: "target_param".to_string(),
            source: BlockArg::Name("to_target".to_string()),
            repr: RuntimeBlockParamRepr::PyObject,
        }];
        let release_names = vec!["released".to_string()];

        assert_eq!(
            planned_drop_forwarded_local_names(
                &forwarded,
                &target_args,
                &[LocalRefKind::Owned],
                &release_names,
                &HashSet::new(),
            ),
            vec!["slot_only".to_string(), "unused".to_string()]
        );
    }

    fn exception_transport_test_param(
        name: &str,
        ownership: LocalRefKind,
    ) -> RuntimeBlockParamPlan {
        RuntimeBlockParamPlan {
            arg_name: name.into(),
            binding: PlannedLocalBinding {
                name: name.into(),
                location: LocalLocation(0),
                storage: PlannedLocalStorage::BlockParam,
                param_facts: BlockParamFacts {
                    value: None,
                    binding: ParamBindingFacts::DefinitelyBound,
                    provenance: ParamProvenance::ForwardedLocal(LocalLocation(0)),
                    ownership,
                },
            },
            entry_aliases: Vec::new(),
            repr: RuntimeBlockParamRepr::PyObject,
        }
    }

    #[test]
    fn exception_transport_keeps_actual_suspended_owner_borrowed_on_caught_edge() {
        use soac_core::block_py::GeneratorResumeParamRole;

        let (prepared, function_index) = prepared_typed_function(
            r#"
def make(save, payload_factory, connect):
    async def source():
        payload = payload_factory()
        try:
            raise ValueError('retained before suspension')
        except ValueError as error:
            save(error)
        yield 'ready'
    value = source()
    connect(source, value)
    return value
"#,
            "make.<locals>.source",
        );
        let function = &prepared.module.callable_defs[function_index];
        let layout = function.storage_layout().as_ref().unwrap();
        let owner_name = layout
            .generator_resume_abi
            .as_ref()
            .unwrap()
            .parameter(GeneratorResumeParamRole::SelfValue)
            .unwrap();
        let plan = prepared.locals.function(function.function_id).unwrap();
        let witnesses =
            plan.exc_dispatches
                .iter()
                .enumerate()
                .flat_map(|(block_index, dispatch)| {
                    dispatch.iter().flat_map(move |dispatch| {
                dispatch.target_args.iter().enumerate().filter_map(move |(arg_index, argument)| {
                    matches!(&argument.source, BlockArg::Name(name) if name == owner_name)
                        .then_some((block_index, arg_index, dispatch))
                })
            })
                })
                .collect::<Vec<_>>();
        assert!(
            !witnesses.is_empty(),
            "the actual resume-owner must cross a caught edge"
        );
        for (_, index, dispatch) in &witnesses {
            let target = &plan.entry_materializations[dispatch.target_index][*index];
            assert_eq!(target.entry_ref_kind, LocalRefKind::Borrowed);
            assert_eq!(target.binding.storage, PlannedLocalStorage::BlockParam);
            assert!(!plan.cleanup_root_names.contains(&target.binding.name));
            assert_eq!(
                dispatch.target_arg_ref_kinds[*index],
                LocalRefKind::Borrowed
            );
            assert!(dispatch.borrowed_forwarded_local_names.contains(owner_name));
            assert!(
                !dispatch
                    .drop_forwarded_local_names
                    .iter()
                    .any(|name| name == owner_name)
            );
            assert!(
                !dispatch
                    .release_local_names
                    .iter()
                    .any(|name| name == owner_name)
            );
        }
        let (block_index, arg_index, _) = witnesses[0];
        let mut stale = plan.clone();
        stale.exc_dispatches[block_index]
            .as_mut()
            .unwrap()
            .target_arg_ref_kinds[arg_index] = LocalRefKind::Owned;
        let error = stale.validate_for_typed_function(function).unwrap_err();
        assert!(
            error.contains("disagrees with its runtime binding"),
            "{error}"
        );
    }

    #[test]
    fn exception_transport_mixed_targets_and_slot_writes_have_one_owned_sink() {
        let (lowered, function_index) = lowered_function("def f():\n    return None\n", "f");
        let function = &lowered.callable_defs[function_index];
        let block = function.entry_block().label;
        let cleanup_roots = HashSet::new();
        for owned_first in [false, true] {
            let mut targets = vec![
                ("borrow_a", "borrowed", LocalRefKind::Borrowed),
                ("borrow_b", "borrowed", LocalRefKind::Borrowed),
                ("mixed_borrow", "mixed", LocalRefKind::Borrowed),
                ("mixed_owned", "mixed", LocalRefKind::Owned),
                ("slot_borrow", "slot_source", LocalRefKind::Borrowed),
                ("released_borrow", "released", LocalRefKind::Borrowed),
            ];
            if owned_first {
                targets.swap(2, 3);
            }
            let params = targets
                .iter()
                .enumerate()
                .map(|(index, (name, _, kind))| {
                    let mut param = exception_transport_test_param(name, *kind);
                    param.binding.location = LocalLocation(index as u32);
                    param.binding.param_facts.provenance =
                        ParamProvenance::ForwardedLocal(param.binding.location);
                    param
                })
                .collect::<Vec<_>>();
            let target_args = targets
                .iter()
                .map(|(target, source, _)| RuntimeBlockArgPlan {
                    target_name: (*target).into(),
                    source: BlockArg::Name((*source).into()),
                    repr: RuntimeBlockParamRepr::PyObject,
                })
                .collect::<Vec<_>>();
            let target_arg_ref_kinds = params
                .iter()
                .map(|param| super::planned_exception_target_ref_kind(param, &cleanup_roots))
                .collect::<Vec<_>>();
            let forwarded_local_names = ["borrowed", "mixed", "slot_source", "released", "unused"]
                .map(String::from)
                .to_vec();
            let slot_writes = vec![("slot".into(), BlockArg::Name("slot_source".into()))];
            let release_local_names = vec!["released".into()];
            let borrowed_forwarded_local_names =
                super::planned_borrowed_exception_forwarded_local_names(
                    &forwarded_local_names,
                    &target_args,
                    &target_arg_ref_kinds,
                    &slot_writes,
                    &release_local_names,
                );
            assert_eq!(
                borrowed_forwarded_local_names,
                HashSet::from(["borrowed".into()])
            );
            let drop_forwarded_local_names = planned_drop_forwarded_local_names(
                &forwarded_local_names,
                &target_args,
                &target_arg_ref_kinds,
                &release_local_names,
                &borrowed_forwarded_local_names,
            );
            assert_eq!(
                drop_forwarded_local_names,
                vec!["slot_source".to_string(), "unused".to_string()]
            );
            let dispatch = BlockExcDispatchPlan {
                target_index: 0,
                slot_writes,
                target_args,
                target_arg_ref_kinds,
                forwarded_local_names,
                borrowed_forwarded_local_names,
                release_local_names,
                drop_forwarded_local_names,
            };
            validate_exception_dispatch_ownership_sinks(
                function,
                block,
                &dispatch,
                &params,
                &cleanup_roots,
            )
            .unwrap();

            let mut drops_borrow = dispatch.clone();
            drops_borrow
                .drop_forwarded_local_names
                .push("borrowed".into());
            let error = validate_exception_dispatch_ownership_sinks(
                function,
                block,
                &drops_borrow,
                &params,
                &cleanup_roots,
            )
            .unwrap_err();
            assert!(error.contains("borrowed forwarded local"), "{error}");

            let mut loses_owner = dispatch.clone();
            loses_owner
                .borrowed_forwarded_local_names
                .insert("mixed".into());
            let error = validate_exception_dispatch_ownership_sinks(
                function,
                block,
                &loses_owner,
                &params,
                &cleanup_roots,
            )
            .unwrap_err();
            assert!(
                error.contains("borrowed forwarded inputs disagree"),
                "{error}"
            );

            let mut loses_slot_owner = dispatch.clone();
            loses_slot_owner
                .drop_forwarded_local_names
                .retain(|name| name != "slot_source");
            let error = validate_exception_dispatch_ownership_sinks(
                function,
                block,
                &loses_slot_owner,
                &params,
                &cleanup_roots,
            )
            .unwrap_err();
            assert!(error.contains("0 ownership sinks"), "{error}");
        }
    }

    #[test]
    fn exception_transport_borrow_demand_requires_unmirrored_borrowed_parameter() {
        let mut parameter =
            exception_transport_test_param("arbitrary_name", LocalRefKind::Borrowed);
        assert_eq!(
            super::planned_exception_target_ref_kind(&parameter, &HashSet::new()),
            LocalRefKind::Borrowed
        );
        assert_eq!(
            super::planned_exception_target_ref_kind(
                &parameter,
                &HashSet::from([parameter.binding.name.clone()])
            ),
            LocalRefKind::Owned,
        );
        parameter.binding.storage = PlannedLocalStorage::StackSlot;
        assert_eq!(
            super::planned_exception_target_ref_kind(&parameter, &HashSet::new()),
            LocalRefKind::Owned
        );
        parameter.binding.storage = PlannedLocalStorage::BlockParam;
        parameter.binding.param_facts.ownership = LocalRefKind::Owned;
        parameter.binding.name = "_dp_self".into();
        parameter.arg_name = parameter.binding.name.clone();
        assert_eq!(
            super::planned_exception_target_ref_kind(&parameter, &HashSet::new()),
            LocalRefKind::Owned
        );
    }

    #[test]
    fn runtime_block_param_reprs_box_loop_carried_arithmetic_without_overflow_proof() {
        let (prepared, function_index) = prepared_typed_function(
            r#"
def count(n):
    i = 0
    while i < n:
        i = i + 1
    return i
"#,
            "count",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");

        let i_params = plan
            .runtime_block_params
            .iter()
            .enumerate()
            .flat_map(|(block_index, params)| params.iter().map(move |param| (block_index, param)))
            .filter(|(_, param)| param.binding.name == "i")
            .collect::<Vec<_>>();
        assert!(
            !i_params.is_empty(),
            "expected loop-carried i to have runtime block params: {:#?}",
            plan.runtime_block_params
        );
        assert!(
            i_params
                .iter()
                .all(|(_, param)| param.repr == RuntimeBlockParamRepr::PyObject),
            "loop-carried i without an overflow proof must stay boxed: {:#?}",
            plan.runtime_block_params
        );

        let i_target_args = plan
            .implicit_target_transports
            .iter()
            .chain(plan.jump_edge_transports.iter().flatten())
            .flat_map(|transport| transport.target_args.iter())
            .filter(|arg| arg.target_name == "i")
            .collect::<Vec<_>>();
        assert!(
            !i_target_args.is_empty(),
            "expected edge transports to carry loop-local i: {:#?}",
            plan.jump_edge_transports
        );
        assert!(
            i_target_args
                .iter()
                .all(|arg| arg.repr == RuntimeBlockParamRepr::PyObject),
            "loop-local i edge transports must agree on boxed representation: {:#?}",
            plan.jump_edge_transports
        );
    }

    #[test]
    fn exception_forwarded_source_locals_stay_pyobject_at_throwing_block() {
        use soac_core::block_py::instr_any;
        let (prepared, function_index) = prepared_typed_function(
            r#"
def break_through_finally():
    total = 0
    for value in (1, 2, 3):
        try:
            break
        finally:
            total = total + 40
    return total + value
"#,
            "break_through_finally",
        );
        let function = &prepared.module.callable_defs[function_index];
        let plan = prepared
            .locals
            .function(function.function_id)
            .expect("missing JIT local plan");
        // Synthetic pending-payload cleanup can introduce earlier exception
        // edges. Select the source operation whose boxing contract is under
        // test, not whichever dispatch happens to be first in block order.
        let source_index = function
            .blocks
            .iter()
            .position(|block| {
                block.body.iter().any(|instr| {
                    instr_any(instr, |instr| matches!(instr, InstrTyped::IteratorStep(_)))
                })
            })
            .expect("the source for-loop must contain its native iterator step");
        let dispatch = plan.exc_dispatches[source_index]
            .as_ref()
            .expect("for-loop iterator-step block should have an exception dispatch");

        assert!(
            dispatch
                .forwarded_local_names
                .iter()
                .any(|name| name == "total"),
            "expected exception dispatch to forward total: {dispatch:#?}"
        );
        let total_param = plan.runtime_block_params[source_index]
            .iter()
            .find(|param| param.binding.name == "total")
            .expect("throwing block should carry total as a runtime param");
        assert_eq!(total_param.repr, RuntimeBlockParamRepr::PyObject);
    }

    #[test]
    fn edge_transport_plan_separates_slot_writes_from_runtime_target_args() {
        let runtime_target_params = vec![RuntimeBlockParamPlan {
            arg_name: "x".to_string(),
            binding: PlannedLocalBinding {
                name: "bound_x".to_string(),
                location: LocalLocation(0),
                storage: PlannedLocalStorage::BlockParam,
                param_facts: BlockParamFacts {
                    value: None,
                    binding: ParamBindingFacts::DefinitelyBound,
                    provenance: ParamProvenance::ForwardedLocal(LocalLocation(0)),
                    ownership: LocalRefKind::Owned,
                },
            },
            entry_aliases: vec!["x".to_string()],
            repr: RuntimeBlockParamRepr::PyObject,
        }];
        let stack_slot_names = HashSet::from(["slot_only".to_string(), "bound_x".to_string()]);
        let transport = plan_edge_transport(
            &["x".to_string(), "slot_only".to_string()],
            &[
                BlockArg::Name("source_x".to_string()),
                BlockArg::Name("spill".to_string()),
            ],
            &runtime_target_params,
            &stack_slot_names,
        );

        assert_eq!(transport.slot_writes.len(), 1);
        assert_eq!(transport.slot_writes[0].0, "slot_only");
        match &transport.slot_writes[0].1 {
            BlockArg::Name(name) => assert_eq!(name, "spill"),
            other => panic!("expected slot write source to be a forwarded name, got {other:?}"),
        }
        assert_eq!(transport.target_args.len(), 1);
        assert_eq!(transport.target_args[0].target_name, "x");
        assert_eq!(
            transport.target_args[0].repr,
            RuntimeBlockParamRepr::PyObject
        );
        match &transport.target_args[0].source {
            BlockArg::Name(name) => assert_eq!(name, "source_x"),
            other => panic!("expected target arg source to be a forwarded name, got {other:?}"),
        }
        assert_eq!(
            transport.forwarded_local_names,
            vec!["spill".to_string(), "source_x".to_string()]
        );
    }

    #[test]
    fn edge_transport_plan_implicitly_forwards_runtime_target_args() {
        let runtime_target_params = vec![RuntimeBlockParamPlan {
            arg_name: "x".to_string(),
            binding: PlannedLocalBinding {
                name: "bound_x".to_string(),
                location: LocalLocation(0),
                storage: PlannedLocalStorage::BlockParam,
                param_facts: BlockParamFacts {
                    value: None,
                    binding: ParamBindingFacts::DefinitelyBound,
                    provenance: ParamProvenance::ForwardedLocal(LocalLocation(0)),
                    ownership: LocalRefKind::Owned,
                },
            },
            entry_aliases: vec!["x".to_string()],
            repr: RuntimeBlockParamRepr::PyObject,
        }];
        let transport = plan_edge_transport(
            &["x".to_string()],
            &[],
            &runtime_target_params,
            &HashSet::new(),
        );

        assert!(transport.slot_writes.is_empty());
        assert_eq!(transport.target_args.len(), 1);
        assert_eq!(transport.target_args[0].target_name, "x");
        assert_eq!(
            transport.target_args[0].repr,
            RuntimeBlockParamRepr::PyObject
        );
        match &transport.target_args[0].source {
            BlockArg::Name(name) => assert_eq!(name, "x"),
            other => panic!("expected implicit forwarded name, got {other:?}"),
        }
        assert_eq!(transport.forwarded_local_names, vec!["x".to_string()]);
    }
}
