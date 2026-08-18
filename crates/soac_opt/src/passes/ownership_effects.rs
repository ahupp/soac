//! Semantic Python ownership planning.
//!
//! This pass intentionally does not insert physical INCREF/DECREF calls into
//! BlockPy. It records ownership effects: local rebinds, deletes, transfers,
//! and cleanup obligations. Backends lower those effects to concrete refcount
//! operations once representation choices such as SSA block params, stack-slot
//! mirrors, borrowed helper results, and immortal constants are known.

use crate::passes::{BlockPyModuleShape, InstrBlockPy};
use crate::typed::typed_expr_planned_pyobject_ownership;
use soac_core::block_py::{
    Block, BlockArg, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, CellLocation,
    ChildVisitable, HasSemanticInstrId, InstrKey, LocalLocation, RuntimeFunctionId, Visit,
};
use soac_ir_typed::plan_v3::{IndexedFieldReceiverSource, RegionInputSource, RegionPlan};
use soac_ir_typed::{
    FactStore, InstrTyped, PyObjFacts, TypedBlock, TypedBlockPyModuleShape,
    TypedPyObjectOwnershipPlan, ValueFacts,
};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum LocalRefState {
    Unbound,
    Borrowed,
    Owned,
    Immortal,
}

impl LocalRefState {
    pub const fn needs_decref(self) -> bool {
        matches!(self, Self::Owned)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RefcountLocal {
    pub location: LocalLocation,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RefcountSite {
    Instr(InstrKey),
    Term {
        function_id: RuntimeFunctionId,
        block_label: BlockLabel,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RefcountReleaseReason {
    Return,
    Raise,
    Jump { target: BlockLabel },
    IfThen { target: BlockLabel },
    IfElse { target: BlockLabel },
    BranchCase { target: BlockLabel },
    BranchDefault { target: BlockLabel },
    ExceptionEdge { target: BlockLabel },
}

pub fn refcount_release_reason_label(reason: &RefcountReleaseReason) -> String {
    match reason {
        RefcountReleaseReason::Return => "return".to_string(),
        RefcountReleaseReason::Raise => "raise".to_string(),
        RefcountReleaseReason::Jump { target } => format!("jump->{target}"),
        RefcountReleaseReason::IfThen { target } => format!("if_then->{target}"),
        RefcountReleaseReason::IfElse { target } => format!("if_else->{target}"),
        RefcountReleaseReason::BranchCase { target } => format!("branch_case->{target}"),
        RefcountReleaseReason::BranchDefault { target } => format!("branch_default->{target}"),
        RefcountReleaseReason::ExceptionEdge { target } => format!("exception_edge->{target}"),
    }
}

pub fn refcount_release_location_branch_name(
    source_label: BlockLabel,
    local: &RefcountLocal,
    reason: &RefcountReleaseReason,
) -> String {
    format!(
        "source={source_label};reason={};slot={};name={}",
        refcount_release_reason_label(reason),
        local.location.slot(),
        local.name
    )
}

pub const REFCOUNT_STACK_SLOT_REPLACE_CLONED_PREVIOUS: &str = "stack_replace_cloned_previous";
pub const REFCOUNT_STACK_SLOT_REPLACE_TRANSFERRED_PREVIOUS: &str =
    "stack_replace_transferred_previous";
pub const REFCOUNT_STACK_SLOT_REPLACE_MOVED_PREVIOUS: &str = "stack_replace_moved_previous";
pub const REFCOUNT_STACK_SLOT_CLEAR_PREVIOUS: &str = "stack_clear_previous";
pub const REFCOUNT_STACK_SLOT_EXIT_SWEEP: &str = "stack_exit_sweep";

pub const REFCOUNT_STACK_SLOT_DECREF_PURPOSES: &[&str] = &[
    REFCOUNT_STACK_SLOT_REPLACE_CLONED_PREVIOUS,
    REFCOUNT_STACK_SLOT_REPLACE_TRANSFERRED_PREVIOUS,
    REFCOUNT_STACK_SLOT_REPLACE_MOVED_PREVIOUS,
    REFCOUNT_STACK_SLOT_CLEAR_PREVIOUS,
    REFCOUNT_STACK_SLOT_EXIT_SWEEP,
];

pub fn refcount_stack_slot_location_branch_name(
    purpose: &str,
    slot_index: usize,
    name: &str,
) -> String {
    format!("purpose={purpose};slot={slot_index};name={name}")
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RefcountActionKind {
    RebindLocal {
        local: RefcountLocal,
        old_state: LocalRefState,
        new_state: LocalRefState,
    },
    DeleteLocal {
        local: RefcountLocal,
        old_state: LocalRefState,
    },
    ReleaseLocal {
        local: RefcountLocal,
        state: LocalRefState,
        reason: RefcountReleaseReason,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RefcountAction {
    pub site: RefcountSite,
    pub kind: RefcountActionKind,
}

#[derive(Clone, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct BlockRefcountPlan {
    pub label: BlockLabel,
    pub actions: Vec<RefcountAction>,
}

#[derive(
    Clone, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct FunctionRefcountPlan {
    pub blocks: HashMap<BlockLabel, BlockRefcountPlan>,
}

impl FunctionRefcountPlan {
    pub fn block(&self, label: BlockLabel) -> Option<&BlockRefcountPlan> {
        self.blocks.get(&label)
    }

    pub fn remap_function_ids(
        &mut self,
        remap: impl Fn(RuntimeFunctionId) -> RuntimeFunctionId + Copy,
    ) {
        for block in self.blocks.values_mut() {
            for action in &mut block.actions {
                action.site.remap_function_ids(remap);
            }
        }
    }
}

#[derive(
    Clone, Debug, Default, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct RefcountPlan {
    pub functions: HashMap<RuntimeFunctionId, FunctionRefcountPlan>,
}

pub type TypedPreciseImmortalLocalEntryStates =
    HashMap<BlockLabel, HashMap<LocalLocation, LocalRefState>>;
pub type TypedModulePreciseImmortalLocalEntryStates =
    HashMap<RuntimeFunctionId, TypedPreciseImmortalLocalEntryStates>;

impl RefcountPlan {
    pub fn function(&self, function_id: RuntimeFunctionId) -> Option<&FunctionRefcountPlan> {
        self.functions.get(&function_id)
    }

    pub fn remap_function_ids(
        &mut self,
        remap: impl Fn(RuntimeFunctionId) -> RuntimeFunctionId + Copy,
    ) {
        self.functions = std::mem::take(&mut self.functions)
            .into_iter()
            .map(|(function_id, mut plan)| {
                plan.remap_function_ids(remap);
                (remap(function_id), plan)
            })
            .collect();
    }
}

impl RefcountSite {
    pub fn remap_function_ids(
        &mut self,
        remap: impl Fn(RuntimeFunctionId) -> RuntimeFunctionId + Copy,
    ) {
        match self {
            Self::Instr(key) => {
                key.function_id = remap(key.function_id);
            }
            Self::Term { function_id, .. } => {
                *function_id = remap(*function_id);
            }
        }
    }
}

pub fn plan_ownership_effects(
    module: &BlockPyModule<BlockPyModuleShape>,
    facts: &FactStore,
) -> RefcountPlan {
    let functions = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                plan_function_refcounts(function, facts),
            )
        })
        .collect();
    RefcountPlan { functions }
}

pub fn validate_ownership_effects(
    module: &BlockPyModule<BlockPyModuleShape>,
    facts: &FactStore,
    plan: &RefcountPlan,
) -> Result<(), String> {
    let mut errors = Vec::new();
    let function_ids = module
        .callable_defs
        .iter()
        .map(|function| function.function_id)
        .collect::<HashSet<_>>();

    for function_id in plan.functions.keys() {
        if !function_ids.contains(function_id) {
            errors.push(format!(
                "refcount plan contains unknown function {function_id}"
            ));
        }
    }

    for function in &module.callable_defs {
        validate_function_refcount_plan(function, facts, plan, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

pub fn plan_typed_ownership_effects(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    facts: &FactStore,
) -> RefcountPlan {
    let precise_immortal_entry_states =
        compute_typed_module_precise_immortal_local_entry_states(module, facts);
    plan_typed_ownership_effects_with_precise_immortal_states(
        module,
        facts,
        &precise_immortal_entry_states,
    )
}

pub fn plan_typed_ownership_effects_with_precise_immortal_states(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    facts: &FactStore,
    precise_immortal_entry_states: &TypedModulePreciseImmortalLocalEntryStates,
) -> RefcountPlan {
    let functions = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                plan_typed_function_refcounts(
                    function,
                    facts,
                    precise_immortal_entry_states.get(&function.function_id),
                ),
            )
        })
        .collect();
    RefcountPlan { functions }
}

pub fn validate_typed_ownership_effects(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    facts: &FactStore,
    plan: &RefcountPlan,
) -> Result<(), String> {
    let precise_immortal_entry_states =
        compute_typed_module_precise_immortal_local_entry_states(module, facts);
    validate_typed_ownership_effects_with_precise_immortal_states(
        module,
        facts,
        plan,
        &precise_immortal_entry_states,
    )
}

pub fn validate_typed_ownership_effects_with_precise_immortal_states(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    facts: &FactStore,
    plan: &RefcountPlan,
    precise_immortal_entry_states: &TypedModulePreciseImmortalLocalEntryStates,
) -> Result<(), String> {
    let mut errors = Vec::new();
    let function_ids = module
        .callable_defs
        .iter()
        .map(|function| function.function_id)
        .collect::<HashSet<_>>();

    for function_id in plan.functions.keys() {
        if !function_ids.contains(function_id) {
            errors.push(format!(
                "refcount plan contains unknown function {function_id}"
            ));
        }
    }

    for function in &module.callable_defs {
        validate_typed_function_refcount_plan(
            function,
            facts,
            plan,
            precise_immortal_entry_states.get(&function.function_id),
            &mut errors,
        );
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn validate_function_refcount_plan(
    function: &BlockPyFunction<BlockPyModuleShape>,
    facts: &FactStore,
    plan: &RefcountPlan,
    errors: &mut Vec<String>,
) {
    let Some(function_plan) = plan.function(function.function_id) else {
        errors.push(format!(
            "refcount plan missing function {} ({})",
            function.function_id, function.names.qualname
        ));
        return;
    };
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        if !function_plan.blocks.is_empty() {
            errors.push(format!(
                "refcount plan for function {} ({}) has blocks but the function has no storage layout",
                function.function_id, function.names.qualname
            ));
        }
        return;
    };

    let locals = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            let location =
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32"));
            (
                location,
                RefcountLocal {
                    location,
                    name: name.clone(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let location_by_name = locals
        .iter()
        .map(|(location, local)| (local.name.clone(), *location))
        .collect::<HashMap<_, _>>();
    let target_params = function
        .blocks
        .iter()
        .map(|block| (block.label, block.param_name_vec()))
        .collect::<HashMap<_, _>>();
    let local_liveness = compute_local_liveness(function, &location_by_name);
    let local_must_bound = compute_local_must_bound(function, &location_by_name);
    let owned_cell_locations = owned_cell_locations(function, &location_by_name);
    let block_labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<HashSet<_>>();
    let entry_label = function.entry_block().label;

    for label in function_plan.blocks.keys() {
        if !block_labels.contains(label) {
            errors.push(format!(
                "refcount plan for function {} ({}) contains unknown block {label}",
                function.function_id, function.names.qualname
            ));
        }
    }

    for block in &function.blocks {
        let Some(block_plan) = function_plan.block(block.label) else {
            errors.push(format!(
                "refcount plan missing block {} in function {} ({})",
                block.label, function.function_id, function.names.qualname
            ));
            continue;
        };
        validate_block_refcount_plan(
            function,
            block,
            block_plan,
            facts,
            &locals,
            &location_by_name,
            &owned_cell_locations,
            &target_params,
            &local_liveness,
            &local_must_bound,
            block.label == entry_label,
            errors,
        );
    }
}

fn validate_typed_function_refcount_plan(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    facts: &FactStore,
    plan: &RefcountPlan,
    precise_immortal_entry_states: Option<&TypedPreciseImmortalLocalEntryStates>,
    errors: &mut Vec<String>,
) {
    let Some(function_plan) = plan.function(function.function_id) else {
        errors.push(format!(
            "refcount plan missing function {} ({})",
            function.function_id, function.names.qualname
        ));
        return;
    };
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        if !function_plan.blocks.is_empty() {
            errors.push(format!(
                "refcount plan for function {} ({}) has blocks but the function has no storage layout",
                function.function_id, function.names.qualname
            ));
        }
        return;
    };

    let locals = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            let location =
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32"));
            (
                location,
                RefcountLocal {
                    location,
                    name: name.clone(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let location_by_name = locals
        .iter()
        .map(|(location, local)| (local.name.clone(), *location))
        .collect::<HashMap<_, _>>();
    let target_params = function
        .blocks
        .iter()
        .map(|block| (block.label, block.param_name_vec()))
        .collect::<HashMap<_, _>>();
    let local_liveness = compute_typed_local_liveness(function, &location_by_name);
    let local_must_bound = compute_typed_local_must_bound(function, &location_by_name);
    let owned_cell_locations = typed_owned_cell_locations(function, &location_by_name);
    let computed_precise_entry_states;
    let precise_entry_states = if let Some(states) = precise_immortal_entry_states {
        states
    } else {
        computed_precise_entry_states = compute_typed_precise_immortal_entry_states_from_parts(
            function,
            facts,
            &location_by_name,
            &owned_cell_locations,
            &target_params,
            &local_liveness,
        );
        &computed_precise_entry_states
    };
    let entry_label = function.entry_block().label;
    let block_labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<HashSet<_>>();

    for label in function_plan.blocks.keys() {
        if !block_labels.contains(label) {
            errors.push(format!(
                "refcount plan for function {} ({}) contains unknown block {label}",
                function.function_id, function.names.qualname
            ));
        }
    }

    for block in &function.blocks {
        let Some(block_plan) = function_plan.block(block.label) else {
            errors.push(format!(
                "refcount plan missing block {} in function {} ({})",
                block.label, function.function_id, function.names.qualname
            ));
            continue;
        };
        validate_typed_block_refcount_plan(
            function,
            block,
            block_plan,
            facts,
            &locals,
            &location_by_name,
            &owned_cell_locations,
            &target_params,
            &local_liveness,
            &local_must_bound,
            block.label == entry_label,
            precise_entry_states.get(&block.label),
            errors,
        );
    }
}

fn plan_function_refcounts(
    function: &BlockPyFunction<BlockPyModuleShape>,
    facts: &FactStore,
) -> FunctionRefcountPlan {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return FunctionRefcountPlan::default();
    };
    let locals = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            let location =
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32"));
            (
                location,
                RefcountLocal {
                    location,
                    name: name.clone(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let location_by_name = locals
        .iter()
        .map(|(location, local)| (local.name.clone(), *location))
        .collect::<HashMap<_, _>>();
    let entry_label = function.entry_block().label;
    let target_params = function
        .blocks
        .iter()
        .map(|block| (block.label, block.param_name_vec()))
        .collect::<HashMap<_, _>>();
    let local_liveness = compute_local_liveness(function, &location_by_name);
    let local_must_bound = compute_local_must_bound(function, &location_by_name);
    let owned_cell_locations = owned_cell_locations(function, &location_by_name);

    let blocks = function
        .blocks
        .iter()
        .map(|block| {
            (
                block.label,
                plan_block_refcounts(
                    function,
                    block,
                    facts,
                    &locals,
                    &location_by_name,
                    &owned_cell_locations,
                    &target_params,
                    &local_liveness,
                    &local_must_bound,
                    block.label == entry_label,
                ),
            )
        })
        .collect();
    FunctionRefcountPlan { blocks }
}

fn plan_typed_function_refcounts(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    facts: &FactStore,
    precise_immortal_entry_states: Option<&TypedPreciseImmortalLocalEntryStates>,
) -> FunctionRefcountPlan {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return FunctionRefcountPlan::default();
    };
    let locals = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            let location =
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32"));
            (
                location,
                RefcountLocal {
                    location,
                    name: name.clone(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let location_by_name = locals
        .iter()
        .map(|(location, local)| (local.name.clone(), *location))
        .collect::<HashMap<_, _>>();
    let entry_label = function.entry_block().label;
    let target_params = function
        .blocks
        .iter()
        .map(|block| (block.label, block.param_name_vec()))
        .collect::<HashMap<_, _>>();
    let local_liveness = compute_typed_local_liveness(function, &location_by_name);
    let local_must_bound = compute_typed_local_must_bound(function, &location_by_name);
    let owned_cell_locations = typed_owned_cell_locations(function, &location_by_name);
    let computed_precise_entry_states;
    let precise_entry_states = if let Some(states) = precise_immortal_entry_states {
        states
    } else {
        computed_precise_entry_states = compute_typed_precise_immortal_entry_states_from_parts(
            function,
            facts,
            &location_by_name,
            &owned_cell_locations,
            &target_params,
            &local_liveness,
        );
        &computed_precise_entry_states
    };

    let blocks = function
        .blocks
        .iter()
        .map(|block| {
            (
                block.label,
                plan_typed_block_refcounts(
                    function,
                    block,
                    facts,
                    &locals,
                    &location_by_name,
                    &owned_cell_locations,
                    &target_params,
                    &local_liveness,
                    &local_must_bound,
                    block.label == entry_label,
                    precise_entry_states.get(&block.label),
                ),
            )
        })
        .collect();
    FunctionRefcountPlan { blocks }
}

pub fn compute_typed_module_precise_immortal_local_entry_states(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    facts: &FactStore,
) -> TypedModulePreciseImmortalLocalEntryStates {
    module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                compute_typed_function_precise_immortal_local_entry_states(function, facts),
            )
        })
        .collect()
}

pub(crate) fn compute_typed_function_precise_immortal_local_entry_states(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    facts: &FactStore,
) -> TypedPreciseImmortalLocalEntryStates {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    let location_by_name = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            (
                name.clone(),
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32")),
            )
        })
        .collect::<HashMap<_, _>>();
    let target_params = function
        .blocks
        .iter()
        .map(|block| (block.label, block.param_name_vec()))
        .collect::<HashMap<_, _>>();
    let local_liveness = compute_typed_local_liveness(function, &location_by_name);
    let owned_cell_locations = typed_owned_cell_locations(function, &location_by_name);
    compute_typed_precise_immortal_entry_states_from_parts(
        function,
        facts,
        &location_by_name,
        &owned_cell_locations,
        &target_params,
        &local_liveness,
    )
}

fn compute_typed_precise_immortal_entry_states_from_parts(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    facts: &FactStore,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
) -> HashMap<BlockLabel, HashMap<LocalLocation, LocalRefState>> {
    let block_indices_by_label = function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect::<HashMap<_, _>>();
    let mut entry_states = vec![HashMap::new(); function.blocks.len()];
    let mut out_states = vec![None::<HashMap<LocalLocation, LocalRefState>>; function.blocks.len()];
    let mut predecessor_edges =
        vec![Vec::<(usize, BlockLabel, Option<Vec<BlockArg>>)>::new(); function.blocks.len()];
    let mut successor_indices = vec![Vec::<usize>::new(); function.blocks.len()];
    for (source_index, block) in function.blocks.iter().enumerate() {
        for (target, explicit_args) in typed_successor_edges(block) {
            let Some(target_index) = block_indices_by_label.get(&target).copied() else {
                continue;
            };
            predecessor_edges[target_index].push((
                source_index,
                target,
                explicit_args.map(<[BlockArg]>::to_vec),
            ));
            successor_indices[source_index].push(target_index);
        }
    }
    let mut pending = (0..function.blocks.len()).collect::<VecDeque<_>>();
    let mut queued = vec![true; function.blocks.len()];
    while let Some(block_index) = pending.pop_front() {
        queued[block_index] = false;
        let incoming_states = predecessor_edges[block_index]
            .iter()
            .filter_map(|(source_index, target, explicit_args)| {
                out_states[*source_index].as_ref().map(|out_state| {
                    typed_successor_precise_immortal_state(
                        *target,
                        explicit_args.as_deref(),
                        out_state,
                        target_params,
                        local_liveness,
                        location_by_name,
                    )
                })
            })
            .collect::<Vec<_>>();
        let new_entry = merge_precise_immortal_incoming_states(&incoming_states);
        if entry_states[block_index] == new_entry && out_states[block_index].is_some() {
            continue;
        }
        entry_states[block_index] = new_entry;
        let new_out = transfer_typed_precise_immortal_state_through_block(
            function.function_id,
            &function.blocks[block_index],
            facts,
            &entry_states[block_index],
            owned_cell_locations,
        );
        if out_states[block_index].as_ref() == Some(&new_out) {
            continue;
        }
        out_states[block_index] = Some(new_out);
        for successor_index in &successor_indices[block_index] {
            if !queued[*successor_index] {
                pending.push_back(*successor_index);
                queued[*successor_index] = true;
            }
        }
    }

    function
        .blocks
        .iter()
        .zip(entry_states)
        .map(|(block, state)| (block.label, state))
        .collect()
}

fn transfer_typed_precise_immortal_state_through_block(
    function_id: RuntimeFunctionId,
    block: &TypedBlock,
    facts: &FactStore,
    entry_state: &HashMap<LocalLocation, LocalRefState>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> HashMap<LocalLocation, LocalRefState> {
    let mut state = entry_state.clone();
    for instr in &block.body {
        match instr {
            InstrTyped::Store(op) => {
                let Some(location) = typed_store_binding_location(op, owned_cell_locations) else {
                    continue;
                };
                if typed_expr_is_precisely_immortal(function_id, &op.value, facts, &state) {
                    state.insert(location, LocalRefState::Immortal);
                } else {
                    state.remove(&location);
                }
            }
            InstrTyped::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    state.remove(&location);
                }
            }
            _ => {}
        }
    }
    state
}

fn typed_expr_is_precisely_immortal(
    function_id: RuntimeFunctionId,
    expr: &InstrTyped,
    facts: &FactStore,
    state: &HashMap<LocalLocation, LocalRefState>,
) -> bool {
    if matches!(
        typed_expr_planned_pyobject_ownership(expr),
        Some(TypedPyObjectOwnershipPlan::Immortal)
    ) {
        return true;
    }
    if let Some(location) = typed_local_load_location(expr) {
        return state.get(&location) == Some(&LocalRefState::Immortal);
    }
    if let Some(ValueFacts::PyObj(py_facts)) =
        expr.typed_extra().and_then(|extra| extra.result_facts())
    {
        return py_facts.is_immortal();
    }
    expr.try_semantic_instr_id()
        .and_then(|instr_id| facts.fact_for(InstrKey::new(function_id, instr_id)))
        .and_then(ValueFacts::as_pyobj)
        .is_some_and(PyObjFacts::is_immortal)
}

fn typed_successor_precise_immortal_state(
    target: BlockLabel,
    explicit_args: Option<&[BlockArg]>,
    exit_state: &HashMap<LocalLocation, LocalRefState>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashMap<LocalLocation, LocalRefState> {
    let mut successor_state = HashMap::new();
    let mut target_param_locations = HashSet::new();
    if let Some(params) = target_params.get(&target) {
        let explicit_start = explicit_args
            .map(|args| params.len().saturating_sub(args.len()))
            .unwrap_or(params.len());
        for (index, target_name) in params.iter().enumerate() {
            let Some(target_location) = location_by_name.get(target_name).copied() else {
                continue;
            };
            target_param_locations.insert(target_location);
            let arg = explicit_args.and_then(|args| {
                index
                    .checked_sub(explicit_start)
                    .and_then(|offset| args.get(offset))
            });
            let is_immortal =
                match arg {
                    Some(BlockArg::Name(source_name)) => location_by_name
                        .get(source_name)
                        .is_some_and(|source_location| {
                            exit_state.get(source_location) == Some(&LocalRefState::Immortal)
                        }),
                    Some(BlockArg::None) => true,
                    Some(BlockArg::CurrentException | BlockArg::AbruptKind(_)) => false,
                    None => exit_state.get(&target_location) == Some(&LocalRefState::Immortal),
                };
            if is_immortal {
                successor_state.insert(target_location, LocalRefState::Immortal);
            }
        }
    }

    if let Some(live_in) = local_liveness.live_in(target) {
        for location in live_in.locations() {
            if target_param_locations.contains(&location) {
                continue;
            }
            if exit_state.get(&location) == Some(&LocalRefState::Immortal) {
                successor_state.insert(location, LocalRefState::Immortal);
            }
        }
    }
    successor_state
}

fn merge_precise_immortal_incoming_states(
    incoming_states: &[HashMap<LocalLocation, LocalRefState>],
) -> HashMap<LocalLocation, LocalRefState> {
    let Some((first, rest)) = incoming_states.split_first() else {
        return HashMap::new();
    };
    let mut merged = first.clone();
    for incoming in rest {
        merged.retain(|location, state| incoming.get(location) == Some(state));
    }
    merged
}

pub fn compute_function_local_live_ins(
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> HashMap<BlockLabel, HashSet<LocalLocation>> {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    let location_by_name = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            (
                name.clone(),
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32")),
            )
        })
        .collect::<HashMap<_, _>>();
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    compute_local_liveness(function, &location_by_name).into_hash_map(&labels)
}

pub fn compute_function_local_must_bound_ins(
    function: &BlockPyFunction<BlockPyModuleShape>,
) -> HashMap<BlockLabel, HashSet<LocalLocation>> {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    let location_by_name = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            (
                name.clone(),
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32")),
            )
        })
        .collect::<HashMap<_, _>>();
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    compute_local_must_bound(function, &location_by_name).into_hash_map(&labels)
}

pub fn compute_typed_function_local_live_ins(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, HashSet<LocalLocation>> {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    let location_by_name = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            (
                name.clone(),
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32")),
            )
        })
        .collect::<HashMap<_, _>>();
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    compute_typed_local_liveness(function, &location_by_name).into_hash_map(&labels)
}

pub fn compute_typed_function_local_must_bound_ins(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, HashSet<LocalLocation>> {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    let location_by_name = storage_layout
        .stack_slots()
        .iter()
        .enumerate()
        .map(|(slot, name)| {
            (
                name.clone(),
                LocalLocation(u32::try_from(slot).expect("local slot index should fit in u32")),
            )
        })
        .collect::<HashMap<_, _>>();
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    compute_typed_local_must_bound(function, &location_by_name).into_hash_map(&labels)
}

fn plan_block_refcounts(
    function: &BlockPyFunction<BlockPyModuleShape>,
    block: &Block<InstrBlockPy>,
    facts: &FactStore,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    local_must_bound: &LocalMustBound,
    is_entry_block: bool,
) -> BlockRefcountPlan {
    let empty_must_bound = LocalBitSet::empty(locals.len());
    let must_bound_on_entry = local_must_bound
        .live_in(block.label)
        .unwrap_or(&empty_must_bound);
    let mut env = initial_block_env(
        function,
        block,
        facts,
        locals,
        location_by_name,
        must_bound_on_entry,
        is_entry_block,
    );
    let mut actions = Vec::new();

    for instr in &block.body {
        match instr {
            InstrBlockPy::Store(op) => {
                let Some(location) = store_binding_location(op, owned_cell_locations) else {
                    continue;
                };
                let Some(local) = locals.get(&location).cloned() else {
                    continue;
                };
                let old_state = env
                    .get(&location)
                    .copied()
                    .unwrap_or(LocalRefState::Unbound);
                let new_state = state_for_expr(function.function_id, &op.value, facts);
                actions.push(RefcountAction {
                    site: RefcountSite::Instr(instr.semantic_instr_key(function.function_id)),
                    kind: RefcountActionKind::RebindLocal {
                        local,
                        old_state,
                        new_state,
                    },
                });
                env.insert(location, new_state);
            }
            InstrBlockPy::Del(op) => {
                let Some(location) = op.name.local_location() else {
                    continue;
                };
                let Some(local) = locals.get(&location).cloned() else {
                    continue;
                };
                let old_state = env
                    .get(&location)
                    .copied()
                    .unwrap_or(LocalRefState::Unbound);
                actions.push(RefcountAction {
                    site: RefcountSite::Instr(instr.semantic_instr_key(function.function_id)),
                    kind: RefcountActionKind::DeleteLocal { local, old_state },
                });
                env.insert(location, LocalRefState::Unbound);
            }
            _ => {}
        }
    }

    if let Some(edge) = &block.exc_edge {
        release_unforwarded_locals(
            function.function_id,
            block.label,
            &env,
            locals,
            preserved_locations(
                edge.target,
                Some(&edge.args),
                local_liveness,
                target_params,
                location_by_name,
            ),
            RefcountReleaseReason::ExceptionEdge {
                target: edge.target,
            },
            &mut actions,
        );
    }

    match &block.term {
        BlockTerm::Jump(edge) => release_unforwarded_locals(
            function.function_id,
            block.label,
            &env,
            locals,
            preserved_locations(
                edge.target,
                Some(&edge.args),
                local_liveness,
                target_params,
                location_by_name,
            ),
            RefcountReleaseReason::Jump {
                target: edge.target,
            },
            &mut actions,
        ),
        BlockTerm::IfTerm(if_term) => {
            release_unforwarded_locals(
                function.function_id,
                block.label,
                &env,
                locals,
                preserved_locations(
                    if_term.then_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::IfThen {
                    target: if_term.then_label,
                },
                &mut actions,
            );
            release_unforwarded_locals(
                function.function_id,
                block.label,
                &env,
                locals,
                preserved_locations(
                    if_term.else_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::IfElse {
                    target: if_term.else_label,
                },
                &mut actions,
            );
        }
        BlockTerm::BranchTable(branch) => {
            for target in &branch.targets {
                release_unforwarded_locals(
                    function.function_id,
                    block.label,
                    &env,
                    locals,
                    preserved_locations(
                        *target,
                        None,
                        local_liveness,
                        target_params,
                        location_by_name,
                    ),
                    RefcountReleaseReason::BranchCase { target: *target },
                    &mut actions,
                );
            }
            release_unforwarded_locals(
                function.function_id,
                block.label,
                &env,
                locals,
                preserved_locations(
                    branch.default_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::BranchDefault {
                    target: branch.default_label,
                },
                &mut actions,
            );
        }
        BlockTerm::Raise(_) => release_all_live_locals(
            function.function_id,
            block.label,
            &env,
            locals,
            RefcountReleaseReason::Raise,
            &mut actions,
        ),
        BlockTerm::Return(_) => release_all_live_locals(
            function.function_id,
            block.label,
            &env,
            locals,
            RefcountReleaseReason::Return,
            &mut actions,
        ),
    }

    BlockRefcountPlan {
        label: block.label,
        actions,
    }
}

#[allow(clippy::too_many_arguments)]
fn plan_typed_block_refcounts(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    facts: &FactStore,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    local_must_bound: &LocalMustBound,
    is_entry_block: bool,
    precise_entry_states: Option<&HashMap<LocalLocation, LocalRefState>>,
) -> BlockRefcountPlan {
    let empty_must_bound = LocalBitSet::empty(locals.len());
    let must_bound_on_entry = local_must_bound
        .live_in(block.label)
        .unwrap_or(&empty_must_bound);
    let mut env = initial_typed_block_env(
        function,
        block,
        facts,
        locals,
        location_by_name,
        must_bound_on_entry,
        is_entry_block,
    );
    apply_precise_immortal_overrides(&mut env, precise_entry_states);
    let mut borrow_owners = initial_typed_borrow_owners(function, location_by_name, is_entry_block);
    let mut actions = Vec::new();

    for (instr_index, instr) in block.body.iter().enumerate() {
        match instr {
            InstrTyped::Store(op) => {
                let Some(location) = typed_store_binding_location(op, owned_cell_locations) else {
                    continue;
                };
                let Some(local) = locals.get(&location).cloned() else {
                    continue;
                };
                let old_state = env
                    .get(&location)
                    .copied()
                    .unwrap_or(LocalRefState::Unbound);
                let borrow_owner = typed_local_store_borrow_owner(
                    function,
                    block,
                    instr_index,
                    location,
                    &op.value,
                    &env,
                    &borrow_owners,
                    locals,
                    owned_cell_locations,
                    target_params,
                    local_liveness,
                    location_by_name,
                );
                let new_state = if borrow_owner.is_some() {
                    LocalRefState::Borrowed
                } else {
                    state_for_typed_expr(function.function_id, &op.value, facts)
                };
                actions.push(RefcountAction {
                    site: RefcountSite::Instr(instr.semantic_instr_key(function.function_id)),
                    kind: RefcountActionKind::RebindLocal {
                        local,
                        old_state,
                        new_state,
                    },
                });
                env.insert(location, new_state);
                if let Some(owner) = borrow_owner {
                    borrow_owners.insert(location, owner);
                } else {
                    borrow_owners.remove(&location);
                }
            }
            InstrTyped::Del(op) => {
                let Some(location) = op.name.local_location() else {
                    continue;
                };
                let Some(local) = locals.get(&location).cloned() else {
                    continue;
                };
                let old_state = env
                    .get(&location)
                    .copied()
                    .unwrap_or(LocalRefState::Unbound);
                actions.push(RefcountAction {
                    site: RefcountSite::Instr(instr.semantic_instr_key(function.function_id)),
                    kind: RefcountActionKind::DeleteLocal { local, old_state },
                });
                env.insert(location, LocalRefState::Unbound);
                borrow_owners.remove(&location);
            }
            _ => {}
        }
    }

    if let Some(edge) = &block.exc_edge {
        release_unforwarded_locals(
            function.function_id,
            block.label,
            &env,
            locals,
            preserved_locations(
                edge.target,
                Some(&edge.args),
                local_liveness,
                target_params,
                location_by_name,
            ),
            RefcountReleaseReason::ExceptionEdge {
                target: edge.target,
            },
            &mut actions,
        );
    }

    match &block.term {
        BlockTerm::Jump(edge) => release_unforwarded_locals(
            function.function_id,
            block.label,
            &env,
            locals,
            preserved_locations(
                edge.target,
                Some(&edge.args),
                local_liveness,
                target_params,
                location_by_name,
            ),
            RefcountReleaseReason::Jump {
                target: edge.target,
            },
            &mut actions,
        ),
        BlockTerm::IfTerm(if_term) => {
            release_unforwarded_locals(
                function.function_id,
                block.label,
                &env,
                locals,
                preserved_locations(
                    if_term.then_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::IfThen {
                    target: if_term.then_label,
                },
                &mut actions,
            );
            release_unforwarded_locals(
                function.function_id,
                block.label,
                &env,
                locals,
                preserved_locations(
                    if_term.else_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::IfElse {
                    target: if_term.else_label,
                },
                &mut actions,
            );
        }
        BlockTerm::BranchTable(branch) => {
            for target in &branch.targets {
                release_unforwarded_locals(
                    function.function_id,
                    block.label,
                    &env,
                    locals,
                    preserved_locations(
                        *target,
                        None,
                        local_liveness,
                        target_params,
                        location_by_name,
                    ),
                    RefcountReleaseReason::BranchCase { target: *target },
                    &mut actions,
                );
            }
            release_unforwarded_locals(
                function.function_id,
                block.label,
                &env,
                locals,
                preserved_locations(
                    branch.default_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::BranchDefault {
                    target: branch.default_label,
                },
                &mut actions,
            );
        }
        BlockTerm::Raise(_) => release_all_live_locals(
            function.function_id,
            block.label,
            &env,
            locals,
            RefcountReleaseReason::Raise,
            &mut actions,
        ),
        BlockTerm::Return(_) => release_all_live_locals(
            function.function_id,
            block.label,
            &env,
            locals,
            RefcountReleaseReason::Return,
            &mut actions,
        ),
    }

    BlockRefcountPlan {
        label: block.label,
        actions,
    }
}

#[allow(clippy::too_many_arguments)]
fn typed_local_store_borrow_owner(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    instr_index: usize,
    target_location: LocalLocation,
    value: &InstrTyped,
    env: &HashMap<LocalLocation, LocalRefState>,
    borrow_owners: &HashMap<LocalLocation, LocalLocation>,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    location_by_name: &HashMap<String, LocalLocation>,
) -> Option<LocalLocation> {
    if locals
        .get(&target_location)
        .is_none_or(|local| local.name.starts_with("_dp_"))
    {
        return None;
    }
    let source_location = typed_local_load_location(value)?;
    if source_location == target_location {
        return None;
    }
    let source_state = env
        .get(&source_location)
        .copied()
        .unwrap_or(LocalRefState::Unbound);
    let owner_location = match source_state {
        LocalRefState::Owned => source_location,
        LocalRefState::Borrowed => *borrow_owners.get(&source_location)?,
        LocalRefState::Immortal | LocalRefState::Unbound => return None,
    };
    if !owner_can_back_borrowed_local(owner_location, env, borrow_owners, locals) {
        return None;
    }
    if typed_later_instrs_can_invalidate_borrow(
        &block.body[instr_index + 1..],
        target_location,
        owner_location,
        owned_cell_locations,
    ) {
        return None;
    }
    for preserved in
        typed_successor_preserved_locations(block, target_params, local_liveness, location_by_name)
    {
        if !preserved.contains(&target_location) {
            continue;
        }
        if preserved.contains(&owner_location)
            && !owner_can_be_shared_across_successor(function, owner_location, owned_cell_locations)
        {
            return None;
        }
        if !owner_can_survive_unforwarded_edge(owner_location, env, borrow_owners, locals) {
            return None;
        }
    }
    Some(owner_location)
}

fn initial_typed_borrow_owners(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
    is_entry_block: bool,
) -> HashMap<LocalLocation, LocalLocation> {
    if !is_entry_block {
        return HashMap::new();
    }
    function
        .params
        .iter()
        .filter_map(|param| location_by_name.get(&param.name).copied())
        .map(|location| (location, location))
        .collect()
}

fn is_external_borrow_owner(
    owner_location: LocalLocation,
    borrow_owners: &HashMap<LocalLocation, LocalLocation>,
) -> bool {
    borrow_owners.get(&owner_location) == Some(&owner_location)
}

fn typed_local_load_location(value: &InstrTyped) -> Option<LocalLocation> {
    let InstrTyped::Load(load) = value else {
        return None;
    };
    load.name.local_location()
}

fn owner_can_back_borrowed_local(
    owner_location: LocalLocation,
    env: &HashMap<LocalLocation, LocalRefState>,
    borrow_owners: &HashMap<LocalLocation, LocalLocation>,
    locals: &HashMap<LocalLocation, RefcountLocal>,
) -> bool {
    match env
        .get(&owner_location)
        .copied()
        .unwrap_or(LocalRefState::Unbound)
    {
        LocalRefState::Owned => locals
            .get(&owner_location)
            .is_some_and(|local| !local.name.starts_with("_dp_")),
        LocalRefState::Immortal => true,
        LocalRefState::Borrowed => is_external_borrow_owner(owner_location, borrow_owners),
        LocalRefState::Unbound => false,
    }
}

fn owner_can_survive_unforwarded_edge(
    owner_location: LocalLocation,
    env: &HashMap<LocalLocation, LocalRefState>,
    borrow_owners: &HashMap<LocalLocation, LocalLocation>,
    locals: &HashMap<LocalLocation, RefcountLocal>,
) -> bool {
    match env
        .get(&owner_location)
        .copied()
        .unwrap_or(LocalRefState::Unbound)
    {
        LocalRefState::Owned => locals
            .get(&owner_location)
            .is_some_and(|local| !local.name.starts_with("_dp_")),
        LocalRefState::Immortal => true,
        LocalRefState::Borrowed => is_external_borrow_owner(owner_location, borrow_owners),
        LocalRefState::Unbound => false,
    }
}

fn typed_later_instrs_can_invalidate_borrow(
    later_instrs: &[InstrTyped],
    target_location: LocalLocation,
    owner_location: LocalLocation,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> bool {
    for instr in later_instrs {
        if typed_instr_rebinds_or_deletes_location(instr, target_location, owned_cell_locations) {
            return false;
        }
        if typed_instr_rebinds_or_deletes_location(instr, owner_location, owned_cell_locations) {
            return true;
        }
    }
    false
}

fn owner_can_be_shared_across_successor(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    owner_location: LocalLocation,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> bool {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return false;
    };
    let Some(owner_name) = storage_layout
        .stack_slots()
        .get(usize::try_from(owner_location.slot()).expect("local slot should fit usize"))
    else {
        return false;
    };
    if !function
        .params
        .iter()
        .any(|param| param.name == owner_name.as_str())
    {
        return false;
    }
    function.blocks.iter().all(|block| {
        block.body.iter().all(|instr| {
            !typed_instr_rebinds_or_deletes_location(instr, owner_location, owned_cell_locations)
        })
    })
}

fn typed_instr_rebinds_or_deletes_location(
    instr: &InstrTyped,
    location: LocalLocation,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> bool {
    match instr {
        InstrTyped::Store(op) => typed_store_binding_location(op, owned_cell_locations)
            .is_some_and(|stored_location| stored_location == location),
        InstrTyped::Del(op) => op
            .name
            .local_location()
            .is_some_and(|deleted_location| deleted_location == location),
        _ => false,
    }
}

fn typed_successor_preserved_locations(
    block: &TypedBlock,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    location_by_name: &HashMap<String, LocalLocation>,
) -> Vec<HashSet<LocalLocation>> {
    typed_successor_edges(block)
        .into_iter()
        .map(|(target, explicit_args)| {
            preserved_locations(
                target,
                explicit_args,
                local_liveness,
                target_params,
                location_by_name,
            )
        })
        .collect()
}

fn typed_successor_edges(block: &TypedBlock) -> Vec<(BlockLabel, Option<&[BlockArg]>)> {
    let mut successors = Vec::new();
    if let Some(edge) = &block.exc_edge {
        successors.push((edge.target, Some(edge.args.as_slice())));
    }
    match &block.term {
        BlockTerm::Jump(edge) => successors.push((edge.target, Some(edge.args.as_slice()))),
        BlockTerm::IfTerm(if_term) => {
            successors.push((if_term.then_label, None));
            successors.push((if_term.else_label, None));
        }
        BlockTerm::BranchTable(branch) => {
            successors.extend(branch.targets.iter().copied().map(|target| (target, None)));
            successors.push((branch.default_label, None));
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
    }
    successors
}

#[allow(clippy::too_many_arguments)]
fn validate_block_refcount_plan(
    function: &BlockPyFunction<BlockPyModuleShape>,
    block: &Block<InstrBlockPy>,
    block_plan: &BlockRefcountPlan,
    facts: &FactStore,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    local_must_bound: &LocalMustBound,
    is_entry_block: bool,
    errors: &mut Vec<String>,
) {
    if block_plan.label != block.label {
        errors.push(format!(
            "refcount plan block label mismatch for function {} ({}): key {} contains {}",
            function.function_id, function.names.qualname, block.label, block_plan.label
        ));
    }

    let mut actions_by_site: HashMap<RefcountSite, Vec<RefcountActionKind>> = HashMap::new();
    for action in &block_plan.actions {
        actions_by_site
            .entry(action.site.clone())
            .or_default()
            .push(action.kind.clone());
    }

    let empty_must_bound = LocalBitSet::empty(locals.len());
    let must_bound_on_entry = local_must_bound
        .live_in(block.label)
        .unwrap_or(&empty_must_bound);
    let mut env = initial_block_env(
        function,
        block,
        facts,
        locals,
        location_by_name,
        must_bound_on_entry,
        is_entry_block,
    );

    for instr in &block.body {
        match instr {
            InstrBlockPy::Store(op) => {
                let site = RefcountSite::Instr(instr.semantic_instr_key(function.function_id));
                let actions = actions_by_site.remove(&site).unwrap_or_default();
                let Some(location) = store_binding_location(op, owned_cell_locations) else {
                    validate_no_refcount_actions(
                        function,
                        block.label,
                        &site,
                        actions,
                        "non-local store",
                        errors,
                    );
                    continue;
                };
                let Some(local) = locals.get(&location).cloned() else {
                    validate_no_refcount_actions(
                        function,
                        block.label,
                        &site,
                        actions,
                        "store to non-layout local",
                        errors,
                    );
                    continue;
                };
                let old_state = env
                    .get(&location)
                    .copied()
                    .unwrap_or(LocalRefState::Unbound);
                let new_state = state_for_expr(function.function_id, &op.value, facts);
                let expected = RefcountActionKind::RebindLocal {
                    local,
                    old_state,
                    new_state,
                };
                validate_exact_refcount_action(
                    function,
                    block.label,
                    &site,
                    actions,
                    expected,
                    errors,
                );
                env.insert(location, new_state);
            }
            InstrBlockPy::Del(op) => {
                let site = RefcountSite::Instr(instr.semantic_instr_key(function.function_id));
                let actions = actions_by_site.remove(&site).unwrap_or_default();
                let Some(location) = op.name.local_location() else {
                    validate_no_refcount_actions(
                        function,
                        block.label,
                        &site,
                        actions,
                        "non-local delete",
                        errors,
                    );
                    continue;
                };
                let Some(local) = locals.get(&location).cloned() else {
                    validate_no_refcount_actions(
                        function,
                        block.label,
                        &site,
                        actions,
                        "delete of non-layout local",
                        errors,
                    );
                    continue;
                };
                let old_state = env
                    .get(&location)
                    .copied()
                    .unwrap_or(LocalRefState::Unbound);
                let expected = RefcountActionKind::DeleteLocal { local, old_state };
                validate_exact_refcount_action(
                    function,
                    block.label,
                    &site,
                    actions,
                    expected,
                    errors,
                );
                env.insert(location, LocalRefState::Unbound);
            }
            _ => {}
        }
    }

    let term_site = RefcountSite::Term {
        function_id: function.function_id,
        block_label: block.label,
    };
    let mut term_actions = actions_by_site.remove(&term_site).unwrap_or_default();

    if let Some(edge) = &block.exc_edge {
        validate_release_actions(
            function,
            block.label,
            &mut term_actions,
            &env,
            locals,
            preserved_locations(
                edge.target,
                Some(&edge.args),
                local_liveness,
                target_params,
                location_by_name,
            ),
            RefcountReleaseReason::ExceptionEdge {
                target: edge.target,
            },
            errors,
        );
    }

    match &block.term {
        BlockTerm::Jump(edge) => validate_release_actions(
            function,
            block.label,
            &mut term_actions,
            &env,
            locals,
            preserved_locations(
                edge.target,
                Some(&edge.args),
                local_liveness,
                target_params,
                location_by_name,
            ),
            RefcountReleaseReason::Jump {
                target: edge.target,
            },
            errors,
        ),
        BlockTerm::IfTerm(if_term) => {
            validate_release_actions(
                function,
                block.label,
                &mut term_actions,
                &env,
                locals,
                preserved_locations(
                    if_term.then_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::IfThen {
                    target: if_term.then_label,
                },
                errors,
            );
            validate_release_actions(
                function,
                block.label,
                &mut term_actions,
                &env,
                locals,
                preserved_locations(
                    if_term.else_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::IfElse {
                    target: if_term.else_label,
                },
                errors,
            );
        }
        BlockTerm::BranchTable(branch) => {
            for target in &branch.targets {
                validate_release_actions(
                    function,
                    block.label,
                    &mut term_actions,
                    &env,
                    locals,
                    preserved_locations(
                        *target,
                        None,
                        local_liveness,
                        target_params,
                        location_by_name,
                    ),
                    RefcountReleaseReason::BranchCase { target: *target },
                    errors,
                );
            }
            validate_release_actions(
                function,
                block.label,
                &mut term_actions,
                &env,
                locals,
                preserved_locations(
                    branch.default_label,
                    None,
                    local_liveness,
                    target_params,
                    location_by_name,
                ),
                RefcountReleaseReason::BranchDefault {
                    target: branch.default_label,
                },
                errors,
            );
        }
        BlockTerm::Raise(_) => validate_release_actions(
            function,
            block.label,
            &mut term_actions,
            &env,
            locals,
            HashSet::new(),
            RefcountReleaseReason::Raise,
            errors,
        ),
        BlockTerm::Return(_) => validate_release_actions(
            function,
            block.label,
            &mut term_actions,
            &env,
            locals,
            HashSet::new(),
            RefcountReleaseReason::Return,
            errors,
        ),
    }

    for action in term_actions {
        errors.push(format!(
            "unexpected refcount terminator action in function {} ({}) block {}: {action:?}",
            function.function_id, function.names.qualname, block.label
        ));
    }
    for (site, actions) in actions_by_site {
        for action in actions {
            errors.push(format!(
                "unexpected refcount action in function {} ({}) block {} at {site:?}: {action:?}",
                function.function_id, function.names.qualname, block.label
            ));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_typed_block_refcount_plan(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    block_plan: &BlockRefcountPlan,
    facts: &FactStore,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    local_must_bound: &LocalMustBound,
    is_entry_block: bool,
    precise_entry_states: Option<&HashMap<LocalLocation, LocalRefState>>,
    errors: &mut Vec<String>,
) {
    let expected = plan_typed_block_refcounts(
        function,
        block,
        facts,
        locals,
        location_by_name,
        owned_cell_locations,
        target_params,
        local_liveness,
        local_must_bound,
        is_entry_block,
        precise_entry_states,
    );
    if &expected != block_plan {
        errors.push(format!(
            "typed refcount plan mismatch for function {} ({}) block {}\nexpected: {expected:#?}\nactual: {block_plan:#?}",
            function.function_id, function.names.qualname, block.label
        ));
    }
}

fn initial_block_env(
    function: &BlockPyFunction<BlockPyModuleShape>,
    block: &Block<InstrBlockPy>,
    facts: &FactStore,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location_by_name: &HashMap<String, LocalLocation>,
    must_bound_on_entry: &LocalBitSet,
    is_entry_block: bool,
) -> HashMap<LocalLocation, LocalRefState> {
    let mut env = locals
        .keys()
        .copied()
        .map(|location| (location, LocalRefState::Unbound))
        .collect::<HashMap<_, _>>();

    if is_entry_block {
        for param in function.body_params().iter() {
            if let Some(location) = location_by_name.get(&param.name) {
                env.insert(*location, LocalRefState::Borrowed);
            }
        }
    }

    for name in block.param_names() {
        if let Some(location) = location_by_name.get(name) {
            let is_entry_param = is_entry_block
                && function
                    .body_params()
                    .iter()
                    .any(|param| param.name.as_str() == name);
            env.insert(
                *location,
                if is_entry_param {
                    LocalRefState::Borrowed
                } else {
                    LocalRefState::Owned
                },
            );
        }
    }

    let entry_param_locations = if is_entry_block {
        function
            .body_params()
            .iter()
            .filter_map(|param| location_by_name.get(&param.name).copied())
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    for location in must_bound_on_entry.locations() {
        if entry_param_locations.contains(&location) {
            continue;
        }
        env.insert(location, LocalRefState::Owned);
    }

    if let Some(entry_facts) = facts.block_entry_fact(function.function_id, block.label) {
        for (location, py_facts) in entry_facts.local_pyobj_facts() {
            if must_bound_on_entry.contains(location) {
                let state = state_for_py_facts(py_facts);
                env.insert(
                    location,
                    if entry_param_locations.contains(&location) && state == LocalRefState::Owned {
                        LocalRefState::Borrowed
                    } else {
                        state
                    },
                );
            }
        }
    }

    env
}

fn state_for_expr(
    function_id: RuntimeFunctionId,
    expr: &InstrBlockPy,
    facts: &FactStore,
) -> LocalRefState {
    match facts.fact_for(expr.semantic_instr_key(function_id)) {
        Some(ValueFacts::PyObj(py_facts)) => state_for_py_facts(py_facts),
        Some(_) | None => LocalRefState::Owned,
    }
}

fn initial_typed_block_env(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    facts: &FactStore,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location_by_name: &HashMap<String, LocalLocation>,
    must_bound_on_entry: &LocalBitSet,
    is_entry_block: bool,
) -> HashMap<LocalLocation, LocalRefState> {
    let mut env = locals
        .keys()
        .copied()
        .map(|location| (location, LocalRefState::Unbound))
        .collect::<HashMap<_, _>>();

    if is_entry_block {
        for param in function.body_params().iter() {
            if let Some(location) = location_by_name.get(&param.name) {
                env.insert(*location, LocalRefState::Borrowed);
            }
        }
    }

    for name in block.param_names() {
        if let Some(location) = location_by_name.get(name) {
            let is_entry_param = is_entry_block
                && function
                    .body_params()
                    .iter()
                    .any(|param| param.name.as_str() == name);
            env.insert(
                *location,
                if is_entry_param {
                    LocalRefState::Borrowed
                } else {
                    LocalRefState::Owned
                },
            );
        }
    }

    let entry_param_locations = if is_entry_block {
        function
            .body_params()
            .iter()
            .filter_map(|param| location_by_name.get(&param.name).copied())
            .collect::<HashSet<_>>()
    } else {
        HashSet::new()
    };
    for location in must_bound_on_entry.locations() {
        if entry_param_locations.contains(&location) {
            continue;
        }
        env.insert(location, LocalRefState::Owned);
    }

    if let Some(entry_facts) = facts.block_entry_fact(function.function_id, block.label) {
        for (location, py_facts) in entry_facts.local_pyobj_facts() {
            if must_bound_on_entry.contains(location) {
                let state = state_for_py_facts(py_facts);
                env.insert(
                    location,
                    if entry_param_locations.contains(&location) && state == LocalRefState::Owned {
                        LocalRefState::Borrowed
                    } else {
                        state
                    },
                );
            }
        }
    }

    env
}

fn apply_precise_immortal_overrides(
    env: &mut HashMap<LocalLocation, LocalRefState>,
    precise_entry_states: Option<&HashMap<LocalLocation, LocalRefState>>,
) {
    let Some(precise_entry_states) = precise_entry_states else {
        return;
    };
    for (location, state) in precise_entry_states {
        debug_assert_eq!(*state, LocalRefState::Immortal);
        if *state == LocalRefState::Immortal {
            env.insert(*location, LocalRefState::Immortal);
        }
    }
}

fn state_for_typed_expr(
    function_id: RuntimeFunctionId,
    expr: &InstrTyped,
    facts: &FactStore,
) -> LocalRefState {
    if matches!(
        typed_expr_planned_pyobject_ownership(expr),
        Some(TypedPyObjectOwnershipPlan::Immortal)
    ) {
        return LocalRefState::Immortal;
    }
    if let Some(ValueFacts::PyObj(py_facts)) =
        expr.typed_extra().and_then(|extra| extra.result_facts())
    {
        return state_for_py_facts(py_facts);
    }
    match expr
        .try_semantic_instr_id()
        .and_then(|instr_id| facts.fact_for(InstrKey::new(function_id, instr_id)))
    {
        Some(ValueFacts::PyObj(py_facts)) => state_for_py_facts(py_facts),
        Some(_) | None => LocalRefState::Owned,
    }
}

fn state_for_py_facts(facts: PyObjFacts) -> LocalRefState {
    if facts.is_immortal() {
        LocalRefState::Immortal
    } else {
        LocalRefState::Owned
    }
}

#[derive(Clone, Debug, Default)]
struct LocalLiveness {
    live_in_by_block: HashMap<BlockLabel, LocalBitSet>,
}

impl LocalLiveness {
    fn live_in(&self, label: BlockLabel) -> Option<&LocalBitSet> {
        self.live_in_by_block.get(&label)
    }

    fn into_hash_map(self, _labels: &[BlockLabel]) -> HashMap<BlockLabel, HashSet<LocalLocation>> {
        self.live_in_by_block
            .into_iter()
            .map(|(label, live_in)| (label, live_in.to_hash_set()))
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
struct LocalMustBound {
    must_bound_in_by_block: HashMap<BlockLabel, LocalBitSet>,
}

impl LocalMustBound {
    fn live_in(&self, label: BlockLabel) -> Option<&LocalBitSet> {
        self.must_bound_in_by_block.get(&label)
    }

    fn into_hash_map(self, _labels: &[BlockLabel]) -> HashMap<BlockLabel, HashSet<LocalLocation>> {
        self.must_bound_in_by_block
            .into_iter()
            .map(|(label, must_bound)| (label, must_bound.to_hash_set()))
            .collect()
    }
}

#[derive(Clone, Debug, Default)]
struct BlockLocalEffects {
    uses: HashSet<LocalLocation>,
    defs: HashSet<LocalLocation>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LocalBitSet {
    words: Vec<u64>,
    local_count: usize,
}

impl LocalBitSet {
    fn empty(local_count: usize) -> Self {
        Self {
            words: vec![0; local_count.div_ceil(u64::BITS as usize)],
            local_count,
        }
    }

    fn full(local_count: usize) -> Self {
        let mut bitset = Self {
            words: vec![u64::MAX; local_count.div_ceil(u64::BITS as usize)],
            local_count,
        };
        bitset.clear_unused_bits();
        bitset
    }

    fn from_locations(
        local_count: usize,
        locations: impl IntoIterator<Item = LocalLocation>,
    ) -> Self {
        let mut bitset = Self::empty(local_count);
        for location in locations {
            bitset.insert(location);
        }
        bitset
    }

    fn contains(&self, location: LocalLocation) -> bool {
        self.index(location)
            .is_some_and(|(word_index, mask)| self.words[word_index] & mask != 0)
    }

    fn insert(&mut self, location: LocalLocation) {
        if let Some((word_index, mask)) = self.index(location) {
            self.words[word_index] |= mask;
        }
    }

    fn remove(&mut self, location: LocalLocation) {
        if let Some((word_index, mask)) = self.index(location) {
            self.words[word_index] &= !mask;
        }
    }

    fn union_with(&mut self, other: &Self) {
        debug_assert_eq!(self.local_count, other.local_count);
        for (left, right) in self.words.iter_mut().zip(&other.words) {
            *left |= *right;
        }
    }

    fn intersect_with(&mut self, other: &Self) {
        debug_assert_eq!(self.local_count, other.local_count);
        for (left, right) in self.words.iter_mut().zip(&other.words) {
            *left &= *right;
        }
    }

    fn difference_with(&mut self, other: &Self) {
        debug_assert_eq!(self.local_count, other.local_count);
        for (left, right) in self.words.iter_mut().zip(&other.words) {
            *left &= !*right;
        }
    }

    fn to_hash_set(&self) -> HashSet<LocalLocation> {
        self.locations().collect()
    }

    fn locations(&self) -> impl Iterator<Item = LocalLocation> + '_ {
        (0..self.local_count).filter_map(|slot| {
            let location = LocalLocation(u32::try_from(slot).ok()?);
            self.contains(location).then_some(location)
        })
    }

    fn index(&self, location: LocalLocation) -> Option<(usize, u64)> {
        let index = usize::try_from(location.0).ok()?;
        if index >= self.local_count {
            return None;
        }
        let word_index = index / u64::BITS as usize;
        let bit_index = index % u64::BITS as usize;
        Some((word_index, 1_u64 << bit_index))
    }

    fn clear_unused_bits(&mut self) {
        let used_bits = self.local_count % u64::BITS as usize;
        if used_bits == 0 {
            return;
        }
        if let Some(last) = self.words.last_mut() {
            *last &= (1_u64 << used_bits) - 1;
        }
    }
}

fn compute_local_liveness(
    function: &BlockPyFunction<BlockPyModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> LocalLiveness {
    let owned_cell_locations = owned_cell_locations(function, location_by_name);
    let local_count = location_by_name.len();
    let effects_by_block = function
        .blocks
        .iter()
        .map(|block| {
            (
                block.label,
                block_local_effects(block, location_by_name, &owned_cell_locations),
            )
        })
        .collect::<HashMap<_, _>>();
    let successors_by_block = function
        .blocks
        .iter()
        .map(|block| (block.label, block_successors(block)))
        .collect::<HashMap<_, _>>();
    let mut live_in_by_block = function
        .blocks
        .iter()
        .map(|block| (block.label, LocalBitSet::empty(local_count)))
        .collect::<HashMap<_, _>>();

    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks.iter().rev() {
            let effects = effects_by_block
                .get(&block.label)
                .expect("liveness effects should exist for every block");
            let mut live_out = LocalBitSet::empty(local_count);
            for successor in successors_by_block
                .get(&block.label)
                .expect("liveness successors should exist for every block")
            {
                if let Some(successor_live_in) = live_in_by_block.get(successor) {
                    live_out.union_with(successor_live_in);
                }
            }

            let mut new_live_in =
                LocalBitSet::from_locations(local_count, effects.uses.iter().copied());
            let defs = LocalBitSet::from_locations(local_count, effects.defs.iter().copied());
            live_out.difference_with(&defs);
            new_live_in.union_with(&live_out);
            let entry = live_in_by_block
                .get_mut(&block.label)
                .expect("liveness entry should exist for every block");
            if *entry != new_live_in {
                *entry = new_live_in;
                changed = true;
            }
        }
    }

    LocalLiveness { live_in_by_block }
}

fn compute_local_must_bound(
    function: &BlockPyFunction<BlockPyModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> LocalMustBound {
    let owned_cell_locations = owned_cell_locations(function, location_by_name);
    let local_count = location_by_name.len();
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    let successors_by_block = function
        .blocks
        .iter()
        .map(|block| (block.label, block_successors(block)))
        .collect::<HashMap<_, _>>();
    let mut predecessors_by_block = labels
        .iter()
        .copied()
        .map(|label| (label, Vec::new()))
        .collect::<HashMap<_, _>>();
    for (source, successors) in &successors_by_block {
        for successor in successors {
            predecessors_by_block
                .get_mut(successor)
                .expect("must-bound predecessor target should exist")
                .push(*source);
        }
    }

    let entry_label = function.entry_block().label;
    let entry_bound = LocalBitSet::from_locations(
        local_count,
        function
            .params
            .iter()
            .filter_map(|param| location_by_name.get(&param.name).copied()),
    );

    let mut must_bound_in_by_block = labels
        .iter()
        .copied()
        .map(|label| {
            let state = if label == entry_label {
                entry_bound.clone()
            } else {
                LocalBitSet::full(local_count)
            };
            (label, state)
        })
        .collect::<HashMap<_, _>>();
    let mut must_bound_out_by_block = labels
        .iter()
        .copied()
        .map(|label| {
            let initial_in = must_bound_in_by_block
                .get(&label)
                .expect("must-bound entry should exist for every block");
            let out = transfer_must_bound_through_block(
                function,
                function
                    .blocks
                    .iter()
                    .find(|block| block.label == label)
                    .expect("must-bound block should exist for label"),
                initial_in,
                &owned_cell_locations,
            );
            (label, out)
        })
        .collect::<HashMap<_, _>>();

    let mut changed = true;
    while changed {
        changed = false;
        for block in &function.blocks {
            let new_in = if block.label == entry_label {
                entry_bound.clone()
            } else {
                let predecessors = predecessors_by_block
                    .get(&block.label)
                    .expect("must-bound predecessors should exist for every block");
                if predecessors.is_empty() {
                    LocalBitSet::empty(local_count)
                } else {
                    let mut intersection = must_bound_out_by_block
                        .get(&predecessors[0])
                        .expect("must-bound predecessor out should exist")
                        .clone();
                    for predecessor in &predecessors[1..] {
                        let predecessor_out = must_bound_out_by_block
                            .get(predecessor)
                            .expect("must-bound predecessor out should exist");
                        intersection.intersect_with(predecessor_out);
                    }
                    intersection
                }
            };
            let new_out =
                transfer_must_bound_through_block(function, block, &new_in, &owned_cell_locations);
            let in_entry = must_bound_in_by_block
                .get_mut(&block.label)
                .expect("must-bound in entry should exist for every block");
            if *in_entry != new_in {
                *in_entry = new_in;
                changed = true;
            }
            let out_entry = must_bound_out_by_block
                .get_mut(&block.label)
                .expect("must-bound out entry should exist for every block");
            if *out_entry != new_out {
                *out_entry = new_out;
                changed = true;
            }
        }
    }

    LocalMustBound {
        must_bound_in_by_block,
    }
}

fn compute_typed_local_liveness(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> LocalLiveness {
    let owned_cell_locations = typed_owned_cell_locations(function, location_by_name);
    let local_count = location_by_name.len();
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    let block_indices_by_label = labels
        .iter()
        .copied()
        .enumerate()
        .map(|(index, label)| (label, index))
        .collect::<HashMap<_, _>>();
    let effects_by_index = function
        .blocks
        .iter()
        .map(|block| typed_block_local_effects(block, location_by_name, &owned_cell_locations))
        .collect::<Vec<_>>();
    let mut successors_by_index = vec![Vec::<usize>::new(); function.blocks.len()];
    let mut predecessors_by_index = vec![Vec::<usize>::new(); function.blocks.len()];
    for (source_index, block) in function.blocks.iter().enumerate() {
        for successor in typed_block_successors(block) {
            let successor_index = *block_indices_by_label
                .get(&successor)
                .expect("typed liveness successor target should exist");
            successors_by_index[source_index].push(successor_index);
            predecessors_by_index[successor_index].push(source_index);
        }
    }
    let mut live_in_by_index = vec![LocalBitSet::empty(local_count); function.blocks.len()];
    let mut pending = (0..function.blocks.len()).rev().collect::<VecDeque<_>>();
    let mut queued = vec![true; function.blocks.len()];
    while let Some(block_index) = pending.pop_front() {
        queued[block_index] = false;
        let effects = &effects_by_index[block_index];
        let mut live_out = LocalBitSet::empty(local_count);
        for successor_index in &successors_by_index[block_index] {
            live_out.union_with(&live_in_by_index[*successor_index]);
        }
        let mut new_live_in =
            LocalBitSet::from_locations(local_count, effects.uses.iter().copied());
        let defs = LocalBitSet::from_locations(local_count, effects.defs.iter().copied());
        live_out.difference_with(&defs);
        new_live_in.union_with(&live_out);
        if live_in_by_index[block_index] == new_live_in {
            continue;
        }
        live_in_by_index[block_index] = new_live_in;
        for predecessor_index in &predecessors_by_index[block_index] {
            if !queued[*predecessor_index] {
                pending.push_back(*predecessor_index);
                queued[*predecessor_index] = true;
            }
        }
    }

    let live_in_by_block = labels
        .into_iter()
        .zip(live_in_by_index)
        .collect::<HashMap<_, _>>();
    LocalLiveness { live_in_by_block }
}

fn compute_typed_local_must_bound(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> LocalMustBound {
    let owned_cell_locations = typed_owned_cell_locations(function, location_by_name);
    let local_count = location_by_name.len();
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    let block_indices_by_label = labels
        .iter()
        .copied()
        .enumerate()
        .map(|(index, label)| (label, index))
        .collect::<HashMap<_, _>>();
    let mut successors_by_index = vec![Vec::<usize>::new(); function.blocks.len()];
    let mut predecessors_by_index = vec![Vec::<usize>::new(); function.blocks.len()];
    for (source_index, block) in function.blocks.iter().enumerate() {
        for successor in typed_block_successors(block) {
            let successor_index = *block_indices_by_label
                .get(&successor)
                .expect("must-bound successor target should exist");
            successors_by_index[source_index].push(successor_index);
            predecessors_by_index[successor_index].push(source_index);
        }
    }
    let entry_label = function.entry_block().label;
    let entry_index = *block_indices_by_label
        .get(&entry_label)
        .expect("must-bound entry block should exist");
    let entry_bound = LocalBitSet::from_locations(
        local_count,
        function
            .params
            .iter()
            .filter_map(|param| location_by_name.get(&param.name).copied()),
    );

    let mut must_bound_in_by_index = labels
        .iter()
        .enumerate()
        .map(|(index, _label)| {
            if index == entry_index {
                entry_bound.clone()
            } else {
                LocalBitSet::full(local_count)
            }
        })
        .collect::<Vec<_>>();
    let mut must_bound_out_by_index = function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            transfer_typed_must_bound_through_block(
                function,
                block,
                &must_bound_in_by_index[index],
                &owned_cell_locations,
            )
        })
        .collect::<Vec<_>>();
    let mut pending = (0..function.blocks.len()).collect::<VecDeque<_>>();
    let mut queued = vec![true; function.blocks.len()];
    while let Some(block_index) = pending.pop_front() {
        queued[block_index] = false;
        let new_in = if block_index == entry_index {
            entry_bound.clone()
        } else {
            let predecessors = &predecessors_by_index[block_index];
            if predecessors.is_empty() {
                LocalBitSet::empty(local_count)
            } else {
                let mut intersection = must_bound_out_by_index[predecessors[0]].clone();
                for predecessor_index in &predecessors[1..] {
                    intersection.intersect_with(&must_bound_out_by_index[*predecessor_index]);
                }
                intersection
            }
        };
        if must_bound_in_by_index[block_index] == new_in {
            continue;
        }
        must_bound_in_by_index[block_index] = new_in;
        let new_out = transfer_typed_must_bound_through_block(
            function,
            &function.blocks[block_index],
            &must_bound_in_by_index[block_index],
            &owned_cell_locations,
        );
        if must_bound_out_by_index[block_index] == new_out {
            continue;
        }
        must_bound_out_by_index[block_index] = new_out;
        for successor_index in &successors_by_index[block_index] {
            if !queued[*successor_index] {
                pending.push_back(*successor_index);
                queued[*successor_index] = true;
            }
        }
    }

    let must_bound_in_by_block = labels
        .into_iter()
        .zip(must_bound_in_by_index)
        .collect::<HashMap<_, _>>();
    LocalMustBound {
        must_bound_in_by_block,
    }
}

fn transfer_must_bound_through_block(
    function: &BlockPyFunction<BlockPyModuleShape>,
    block: &Block<InstrBlockPy>,
    must_bound_in: &LocalBitSet,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> LocalBitSet {
    let mut must_bound = must_bound_in.clone();
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return must_bound;
    };
    for name in block.param_names() {
        if let Some(index) = storage_layout
            .stack_slots()
            .iter()
            .position(|candidate| candidate == name)
        {
            must_bound.insert(LocalLocation(
                u32::try_from(index).expect("local slot index should fit in u32"),
            ));
        }
    }
    for instr in &block.body {
        match instr {
            InstrBlockPy::Store(op) => {
                if let Some(location) = store_binding_location(op, owned_cell_locations) {
                    must_bound.insert(location);
                }
            }
            InstrBlockPy::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    must_bound.remove(location);
                }
            }
            _ => {}
        }
    }
    must_bound
}

fn transfer_typed_must_bound_through_block(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    block: &TypedBlock,
    must_bound_in: &LocalBitSet,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> LocalBitSet {
    let mut must_bound = must_bound_in.clone();
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return must_bound;
    };
    for name in block.param_names() {
        if let Some(index) = storage_layout
            .stack_slots()
            .iter()
            .position(|candidate| candidate == name)
        {
            must_bound.insert(LocalLocation(
                u32::try_from(index).expect("local slot index should fit in u32"),
            ));
        }
    }
    for instr in &block.body {
        match instr {
            InstrTyped::Store(op) => {
                if let Some(location) = typed_store_binding_location(op, owned_cell_locations) {
                    must_bound.insert(location);
                }
            }
            InstrTyped::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    must_bound.remove(location);
                }
            }
            _ => {}
        }
    }
    must_bound
}

fn block_local_effects(
    block: &Block<InstrBlockPy>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> BlockLocalEffects {
    let mut effects = BlockLocalEffects::default();
    for name in block.param_names() {
        if let Some(location) = location_by_name.get(name) {
            effects.defs.insert(*location);
        }
    }

    for instr in &block.body {
        collect_local_reads(
            instr,
            &effects.defs,
            location_by_name,
            owned_cell_locations,
            &mut effects.uses,
        );
        match instr {
            InstrBlockPy::Store(op) => {
                if let Some(location) = op.name.local_location() {
                    effects.defs.insert(location);
                }
            }
            InstrBlockPy::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    effects.defs.insert(location);
                }
            }
            _ => {}
        }
    }

    collect_term_local_reads(
        &block.term,
        &effects.defs,
        location_by_name,
        owned_cell_locations,
        &mut effects.uses,
    );
    effects
}

fn typed_block_local_effects(
    block: &TypedBlock,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> BlockLocalEffects {
    let mut effects = BlockLocalEffects::default();
    for name in block.param_names() {
        if let Some(location) = location_by_name.get(name) {
            effects.defs.insert(*location);
        }
    }

    for instr in &block.body {
        collect_typed_local_reads(
            instr,
            &effects.defs,
            location_by_name,
            owned_cell_locations,
            &mut effects.uses,
        );
        match instr {
            InstrTyped::Store(op) => {
                if let Some(location) = op.name.local_location() {
                    effects.defs.insert(location);
                }
            }
            InstrTyped::Del(op) => {
                if let Some(location) = op.name.local_location() {
                    effects.defs.insert(location);
                }
            }
            _ => {}
        }
    }

    collect_typed_term_local_reads(
        &block.term,
        &effects.defs,
        location_by_name,
        owned_cell_locations,
        &mut effects.uses,
    );
    effects
}

fn owned_cell_locations(
    function: &BlockPyFunction<BlockPyModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashMap<u32, LocalLocation> {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    storage_layout
        .cellvars
        .iter()
        .enumerate()
        .filter_map(|(slot, cell)| {
            let location = location_by_name.get(cell.storage_name.as_str()).copied()?;
            let slot = u32::try_from(slot).expect("owned cell slot should fit in u32");
            Some((slot, location))
        })
        .collect()
}

fn typed_owned_cell_locations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashMap<u32, LocalLocation> {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    storage_layout
        .cellvars
        .iter()
        .enumerate()
        .filter_map(|(slot, cell)| {
            let location = location_by_name.get(cell.storage_name.as_str()).copied()?;
            let slot = u32::try_from(slot).expect("owned cell slot should fit in u32");
            Some((slot, location))
        })
        .collect()
}

fn store_binding_location(
    op: &soac_core::block_py::Store<InstrBlockPy>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> Option<LocalLocation> {
    op.name.local_location().or_else(|| {
        let CellLocation::Owned(slot) = op.name.cell_location()? else {
            return None;
        };
        matches!(op.value.as_ref(), InstrBlockPy::MakeCell(_))
            .then(|| owned_cell_locations.get(&slot).copied())
            .flatten()
    })
}

fn typed_store_binding_location(
    op: &soac_core::block_py::Store<InstrTyped>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> Option<LocalLocation> {
    op.name.local_location().or_else(|| {
        let CellLocation::Owned(slot) = op.name.cell_location()? else {
            return None;
        };
        matches!(op.value.as_ref(), InstrTyped::MakeCell(_))
            .then(|| owned_cell_locations.get(&slot).copied())
            .flatten()
    })
}

fn mark_local_use(
    location: LocalLocation,
    defs: &HashSet<LocalLocation>,
    uses: &mut HashSet<LocalLocation>,
) {
    if !defs.contains(&location) {
        uses.insert(location);
    }
}

fn mark_cell_use(
    cell_location: CellLocation,
    defs: &HashSet<LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    uses: &mut HashSet<LocalLocation>,
) {
    if let CellLocation::Owned(slot) = cell_location {
        if let Some(location) = owned_cell_locations.get(&slot) {
            mark_local_use(*location, defs, uses);
        }
    }
}

fn collect_local_reads(
    expr: &InstrBlockPy,
    defs: &HashSet<LocalLocation>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    uses: &mut HashSet<LocalLocation>,
) {
    struct LocalReadCollector<'a> {
        defs: &'a HashSet<LocalLocation>,
        location_by_name: &'a HashMap<String, LocalLocation>,
        owned_cell_locations: &'a HashMap<u32, LocalLocation>,
        uses: &'a mut HashSet<LocalLocation>,
    }

    impl Visit<InstrBlockPy> for LocalReadCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            match expr {
                InstrBlockPy::Load(op) => {
                    if let Some(location) = op.name.local_location() {
                        mark_local_use(location, self.defs, self.uses);
                    }
                    if let Some(cell_location) = op.name.cell_location() {
                        mark_cell_use(
                            cell_location,
                            self.defs,
                            self.owned_cell_locations,
                            self.uses,
                        );
                    }
                }
                InstrBlockPy::Store(op) => {
                    if let Some(cell_location) = op.name.cell_location() {
                        mark_cell_use(
                            cell_location,
                            self.defs,
                            self.owned_cell_locations,
                            self.uses,
                        );
                    }
                }
                InstrBlockPy::Del(op) => {
                    if let Some(location) = op.name.local_location() {
                        mark_local_use(location, self.defs, self.uses);
                    }
                    if let Some(cell_location) = op.name.cell_location() {
                        mark_cell_use(
                            cell_location,
                            self.defs,
                            self.owned_cell_locations,
                            self.uses,
                        );
                    }
                }
                InstrBlockPy::CellRef(op) => {
                    mark_cell_use(op.location, self.defs, self.owned_cell_locations, self.uses)
                }
                _ => {}
            }
            expr.visit_children(self);
        }

        fn visit_block_arg(&mut self, arg: &BlockArg) {
            if let BlockArg::Name(name) = arg {
                if let Some(location) = self.location_by_name.get(name) {
                    mark_local_use(*location, self.defs, self.uses);
                }
            }
        }
    }

    LocalReadCollector {
        defs,
        location_by_name,
        owned_cell_locations,
        uses,
    }
    .visit_instr(expr);
}

fn collect_term_local_reads(
    term: &BlockTerm<InstrBlockPy>,
    defs: &HashSet<LocalLocation>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    uses: &mut HashSet<LocalLocation>,
) {
    struct TermReadCollector<'a> {
        defs: &'a HashSet<LocalLocation>,
        location_by_name: &'a HashMap<String, LocalLocation>,
        owned_cell_locations: &'a HashMap<u32, LocalLocation>,
        uses: &'a mut HashSet<LocalLocation>,
    }

    impl Visit<InstrBlockPy> for TermReadCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy)
        where
            InstrBlockPy: ChildVisitable<InstrBlockPy>,
        {
            collect_local_reads(
                expr,
                self.defs,
                self.location_by_name,
                self.owned_cell_locations,
                self.uses,
            );
        }

        fn visit_block_arg(&mut self, arg: &BlockArg) {
            if let BlockArg::Name(name) = arg {
                if let Some(location) = self.location_by_name.get(name) {
                    mark_local_use(*location, self.defs, self.uses);
                }
            }
        }
    }

    TermReadCollector {
        defs,
        location_by_name,
        owned_cell_locations,
        uses,
    }
    .visit_term(term);
}

fn block_successors(block: &Block<InstrBlockPy>) -> Vec<BlockLabel> {
    let mut successors = Vec::new();
    if let Some(edge) = &block.exc_edge {
        successors.push(edge.target);
    }
    match &block.term {
        BlockTerm::Jump(edge) => successors.push(edge.target),
        BlockTerm::IfTerm(if_term) => {
            successors.push(if_term.then_label);
            successors.push(if_term.else_label);
        }
        BlockTerm::BranchTable(branch) => {
            successors.extend(branch.targets.iter().copied());
            successors.push(branch.default_label);
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
    }
    successors
}

fn collect_typed_local_reads(
    expr: &InstrTyped,
    defs: &HashSet<LocalLocation>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    uses: &mut HashSet<LocalLocation>,
) {
    struct LocalReadCollector<'a> {
        defs: &'a HashSet<LocalLocation>,
        location_by_name: &'a HashMap<String, LocalLocation>,
        owned_cell_locations: &'a HashMap<u32, LocalLocation>,
        uses: &'a mut HashSet<LocalLocation>,
    }

    impl LocalReadCollector<'_> {
        fn mark_exact_int_region_input_reads(&mut self, regions: [&RegionPlan; 2]) {
            for region in regions {
                for input in &region.inputs {
                    let name = match &input.source {
                        RegionInputSource::FunctionParam {
                            name: Some(name), ..
                        }
                        | RegionInputSource::IndexedField {
                            receiver: IndexedFieldReceiverSource::LocalName { name },
                            ..
                        } => name,
                        _ => continue,
                    };
                    if let Some(location) = self.location_by_name.get(name) {
                        mark_local_use(*location, self.defs, self.uses);
                    }
                }
            }
        }
    }

    impl Visit<InstrTyped> for LocalReadCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped)
        where
            InstrTyped: ChildVisitable<InstrTyped>,
        {
            if let Some(extra) = expr.typed_extra() {
                if let Some(plan) = extra.exact_int_branch_plan() {
                    self.mark_exact_int_region_input_reads([&plan.hot_plan, &plan.fallback_plan]);
                }
                if let Some(plan) = extra.exact_int_return_plan() {
                    self.mark_exact_int_region_input_reads([&plan.hot_plan, &plan.fallback_plan]);
                }
            }

            match expr {
                InstrTyped::Load(op) => {
                    if let Some(location) = op.name.local_location() {
                        mark_local_use(location, self.defs, self.uses);
                    }
                    if let Some(cell_location) = op.name.cell_location() {
                        mark_cell_use(
                            cell_location,
                            self.defs,
                            self.owned_cell_locations,
                            self.uses,
                        );
                    }
                }
                InstrTyped::Store(op) => {
                    if let Some(cell_location) = op.name.cell_location() {
                        mark_cell_use(
                            cell_location,
                            self.defs,
                            self.owned_cell_locations,
                            self.uses,
                        );
                    }
                }
                InstrTyped::Del(op) => {
                    if let Some(location) = op.name.local_location() {
                        mark_local_use(location, self.defs, self.uses);
                    }
                    if let Some(cell_location) = op.name.cell_location() {
                        mark_cell_use(
                            cell_location,
                            self.defs,
                            self.owned_cell_locations,
                            self.uses,
                        );
                    }
                }
                InstrTyped::CellRef(op) => {
                    mark_cell_use(op.location, self.defs, self.owned_cell_locations, self.uses)
                }
                _ => {}
            }
            expr.visit_children(self);
        }

        fn visit_block_arg(&mut self, arg: &BlockArg) {
            if let BlockArg::Name(name) = arg {
                if let Some(location) = self.location_by_name.get(name) {
                    mark_local_use(*location, self.defs, self.uses);
                }
            }
        }
    }

    LocalReadCollector {
        defs,
        location_by_name,
        owned_cell_locations,
        uses,
    }
    .visit_instr(expr);
}

fn collect_typed_term_local_reads(
    term: &BlockTerm<InstrTyped>,
    defs: &HashSet<LocalLocation>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    uses: &mut HashSet<LocalLocation>,
) {
    struct TermReadCollector<'a> {
        defs: &'a HashSet<LocalLocation>,
        location_by_name: &'a HashMap<String, LocalLocation>,
        owned_cell_locations: &'a HashMap<u32, LocalLocation>,
        uses: &'a mut HashSet<LocalLocation>,
    }

    impl Visit<InstrTyped> for TermReadCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped)
        where
            InstrTyped: ChildVisitable<InstrTyped>,
        {
            collect_typed_local_reads(
                expr,
                self.defs,
                self.location_by_name,
                self.owned_cell_locations,
                self.uses,
            );
        }

        fn visit_block_arg(&mut self, arg: &BlockArg) {
            if let BlockArg::Name(name) = arg {
                if let Some(location) = self.location_by_name.get(name) {
                    mark_local_use(*location, self.defs, self.uses);
                }
            }
        }
    }

    TermReadCollector {
        defs,
        location_by_name,
        owned_cell_locations,
        uses,
    }
    .visit_term(term);
}

fn typed_block_successors(block: &TypedBlock) -> Vec<BlockLabel> {
    let mut successors = Vec::new();
    if let Some(edge) = &block.exc_edge {
        successors.push(edge.target);
    }
    match &block.term {
        BlockTerm::Jump(edge) => successors.push(edge.target),
        BlockTerm::IfTerm(if_term) => {
            successors.push(if_term.then_label);
            successors.push(if_term.else_label);
        }
        BlockTerm::BranchTable(branch) => {
            successors.extend(branch.targets.iter().copied());
            successors.push(branch.default_label);
        }
        BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
    }
    successors
}

fn forwarded_locations(
    target: BlockLabel,
    explicit_args: Option<&[BlockArg]>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashSet<LocalLocation> {
    let Some(params) = target_params.get(&target) else {
        return HashSet::new();
    };
    let explicit_start = explicit_args
        .map(|args| params.len().saturating_sub(args.len()))
        .unwrap_or(params.len());
    params
        .iter()
        .enumerate()
        .filter_map(|(index, target_name)| {
            let arg = explicit_args.and_then(|args| {
                index
                    .checked_sub(explicit_start)
                    .and_then(|offset| args.get(offset))
            });
            let source_name = match arg {
                Some(BlockArg::Name(name)) => name.as_str(),
                Some(_) => return None,
                None => target_name.as_str(),
            };
            location_by_name.get(source_name).copied()
        })
        .collect()
}

fn preserved_locations(
    target: BlockLabel,
    explicit_args: Option<&[BlockArg]>,
    local_liveness: &LocalLiveness,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashSet<LocalLocation> {
    let mut preserved = forwarded_locations(target, explicit_args, target_params, location_by_name);
    if let Some(live_in) = local_liveness.live_in(target) {
        preserved.extend(live_in.locations());
    }
    preserved
}

fn release_unforwarded_locals(
    function_id: RuntimeFunctionId,
    block_label: BlockLabel,
    env: &HashMap<LocalLocation, LocalRefState>,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    forwarded: HashSet<LocalLocation>,
    reason: RefcountReleaseReason,
    actions: &mut Vec<RefcountAction>,
) {
    for (location, state) in sorted_live_releases(env) {
        if forwarded.contains(&location) {
            continue;
        }
        push_release_action(
            function_id,
            block_label,
            locals,
            location,
            state,
            reason.clone(),
            actions,
        );
    }
}

fn release_all_live_locals(
    function_id: RuntimeFunctionId,
    block_label: BlockLabel,
    env: &HashMap<LocalLocation, LocalRefState>,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    reason: RefcountReleaseReason,
    actions: &mut Vec<RefcountAction>,
) {
    for (location, state) in sorted_live_releases(env) {
        push_release_action(
            function_id,
            block_label,
            locals,
            location,
            state,
            reason.clone(),
            actions,
        );
    }
}

fn sorted_live_releases(
    env: &HashMap<LocalLocation, LocalRefState>,
) -> Vec<(LocalLocation, LocalRefState)> {
    let mut releases = env
        .iter()
        .filter_map(|(location, state)| state.needs_decref().then_some((*location, *state)))
        .collect::<Vec<_>>();
    releases.sort_by_key(|(location, _)| location.slot());
    releases
}

fn push_release_action(
    function_id: RuntimeFunctionId,
    block_label: BlockLabel,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location: LocalLocation,
    state: LocalRefState,
    reason: RefcountReleaseReason,
    actions: &mut Vec<RefcountAction>,
) {
    let Some(local) = locals.get(&location).cloned() else {
        return;
    };
    actions.push(RefcountAction {
        site: RefcountSite::Term {
            function_id,
            block_label,
        },
        kind: RefcountActionKind::ReleaseLocal {
            local,
            state,
            reason,
        },
    });
}

fn validate_no_refcount_actions(
    function: &BlockPyFunction<BlockPyModuleShape>,
    block_label: BlockLabel,
    site: &RefcountSite,
    actions: Vec<RefcountActionKind>,
    context: &str,
    errors: &mut Vec<String>,
) {
    for action in actions {
        errors.push(format!(
            "unexpected refcount action for {context} in function {} ({}) block {block_label} at {site:?}: {action:?}",
            function.function_id, function.names.qualname
        ));
    }
}

fn validate_exact_refcount_action(
    function: &BlockPyFunction<BlockPyModuleShape>,
    block_label: BlockLabel,
    site: &RefcountSite,
    actions: Vec<RefcountActionKind>,
    expected: RefcountActionKind,
    errors: &mut Vec<String>,
) {
    if actions.len() == 1 && actions[0] == expected {
        return;
    }
    if actions.is_empty() {
        errors.push(format!(
            "missing refcount action in function {} ({}) block {block_label} at {site:?}: expected {expected:?}",
            function.function_id, function.names.qualname
        ));
        return;
    }
    errors.push(format!(
        "wrong refcount action in function {} ({}) block {block_label} at {site:?}: expected {expected:?}, got {actions:?}",
        function.function_id, function.names.qualname
    ));
}

#[allow(clippy::too_many_arguments)]
fn validate_release_actions(
    function: &BlockPyFunction<BlockPyModuleShape>,
    block_label: BlockLabel,
    term_actions: &mut Vec<RefcountActionKind>,
    env: &HashMap<LocalLocation, LocalRefState>,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    forwarded: HashSet<LocalLocation>,
    reason: RefcountReleaseReason,
    errors: &mut Vec<String>,
) {
    for expected in expected_release_actions(env, locals, forwarded, reason) {
        let Some(index) = term_actions.iter().position(|action| *action == expected) else {
            errors.push(format!(
                "missing refcount release in function {} ({}) block {block_label}: expected {expected:?}",
                function.function_id, function.names.qualname
            ));
            continue;
        };
        term_actions.remove(index);
    }
}

fn expected_release_actions(
    env: &HashMap<LocalLocation, LocalRefState>,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    forwarded: HashSet<LocalLocation>,
    reason: RefcountReleaseReason,
) -> Vec<RefcountActionKind> {
    sorted_live_releases(env)
        .into_iter()
        .filter(|(location, _)| !forwarded.contains(location))
        .filter_map(|(location, state)| {
            locals
                .get(&location)
                .cloned()
                .map(|local| RefcountActionKind::ReleaseLocal {
                    local,
                    state,
                    reason: reason.clone(),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{
        LocalRefState, RefcountActionKind, RefcountReleaseReason, compute_function_local_live_ins,
        compute_function_local_must_bound_ins, compute_typed_function_local_live_ins,
        compute_typed_function_precise_immortal_local_entry_states, forwarded_locations,
        plan_ownership_effects, plan_typed_ownership_effects, validate_ownership_effects,
        validate_typed_ownership_effects,
    };
    use crate::passes::{LocalRefKind, infer_module_value_facts, plan_typed_local_env_module};
    use soac_core::block_py::{
        BlockArg, BlockLabel, BlockPyFunction, BlockTerm, HasSemanticInstrId, LocalLocation,
    };
    use soac_ir_typed::emit_v3::MechanicalRegionEmission;
    use soac_ir_typed::plan_v3::{
        IndexedFieldOwnerType, IndexedFieldReceiverSource, PlanValue, RegionId, RegionInput,
        RegionInputSource, RegionPlan, RegionSource, Rep,
    };
    use soac_ir_typed::{
        InstrTyped, TypedExactIntBranchPlan, TypedExactIntPlanSource, TypedExactIntReturnPlan,
        TypedPlannedResult, lower_blockpy_module_to_typed,
    };
    use soac_lowering::lower_python_to_blockpy_for_testing;
    use std::collections::{HashMap, HashSet};

    fn refcount_actions_for_function(
        source: &str,
    ) -> (
        BlockPyFunction<crate::passes::BlockPyModuleShape>,
        Vec<RefcountActionKind>,
    ) {
        refcount_actions_for_named_function(source, "f")
    }

    fn refcount_actions_for_named_function(
        source: &str,
        qualname: &str,
    ) -> (
        BlockPyFunction<crate::passes::BlockPyModuleShape>,
        Vec<RefcountActionKind>,
    ) {
        let lowered = lower_python_to_blockpy_for_testing(source)
            .expect("transform should succeed")
            .blockpy_module;
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing lowered function {qualname}"))
            .clone();
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_ownership_effects(&lowered, &facts);
        let function_plan = plan
            .function(function.function_id)
            .expect("missing function refcount plan");
        let actions = function
            .blocks
            .iter()
            .flat_map(|block| {
                function_plan
                    .block(block.label)
                    .expect("missing block refcount plan")
                    .actions
                    .iter()
                    .map(|action| action.kind.clone())
            })
            .collect();
        (function, actions)
    }

    #[test]
    fn refcount_plan_records_store_rebind_after_rhs_classification() {
        let (_function, actions) = refcount_actions_for_function(
            r#"
def f():
    x = []
    x = None
    return x
"#,
        );
        let rebinds = actions
            .iter()
            .filter_map(|action| match action {
                RefcountActionKind::RebindLocal {
                    local,
                    old_state,
                    new_state,
                } if local.name == "x" => Some((*old_state, *new_state)),
                _ => None,
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rebinds,
            vec![
                (LocalRefState::Unbound, LocalRefState::Owned),
                (LocalRefState::Owned, LocalRefState::Immortal),
            ]
        );
    }

    #[test]
    fn refcount_plan_skips_immortal_return_cleanup() {
        let (_function, actions) = refcount_actions_for_function(
            r#"
def f():
    x = None
    return 1
"#,
        );

        assert!(
            !actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::ReleaseLocal { local, reason, .. }
                    if local.name == "x" && *reason == RefcountReleaseReason::Return
            )),
            "immortal local bindings should not require return cleanup"
        );
    }

    #[test]
    fn refcount_plan_releases_owned_locals_on_return() {
        let (_function, actions) = refcount_actions_for_function(
            r#"
def f():
    x = []
    return None
"#,
        );

        assert!(
            actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::ReleaseLocal {
                    local,
                    state: LocalRefState::Owned,
                    reason: RefcountReleaseReason::Return,
                } if local.name == "x"
            )),
            "owned local bindings should be released by return cleanup"
        );
    }

    #[test]
    fn refcount_plan_releases_deleted_locals_on_delete_not_return() {
        let (_function, actions) = refcount_actions_for_function(
            r#"
def f():
    x = []
    del x
    return None
"#,
        );

        assert!(
            actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::DeleteLocal {
                    local,
                    old_state: LocalRefState::Owned,
                } if local.name == "x"
            )),
            "delete should release the old owned local binding: {actions:#?}"
        );
        assert!(
            !actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::ReleaseLocal { local, reason, .. }
                    if local.name == "x" && *reason == RefcountReleaseReason::Return
            )),
            "deleted locals should not also be released by return cleanup"
        );
    }

    #[test]
    fn refcount_plan_releases_branch_local_only_on_non_forwarding_edge() {
        let (function, actions) = refcount_actions_for_function(
            r#"
def f(flag):
    x = []
    if flag:
        return x
    return None
"#,
        );
        let branch_block = function
            .blocks
            .iter()
            .find(|block| matches!(block.term, BlockTerm::IfTerm(_)))
            .expect("expected conditional block");
        let then_releases = actions
            .iter()
            .filter(|action| match action {
                RefcountActionKind::ReleaseLocal { local, reason, .. } if local.name == "x" => {
                    matches!(reason, RefcountReleaseReason::IfThen { .. })
                }
                _ => false,
            })
            .count();
        let else_releases = actions
            .iter()
            .filter(|action| match action {
                RefcountActionKind::ReleaseLocal { local, reason, .. } if local.name == "x" => {
                    matches!(reason, RefcountReleaseReason::IfElse { .. })
                }
                _ => false,
            })
            .count();

        assert_eq!(
            then_releases, 0,
            "branch block {} should forward x to the then-return target",
            branch_block.label
        );
        assert_eq!(
            else_releases, 1,
            "branch block {} should release x only on the non-forwarding else edge",
            branch_block.label
        );
    }

    #[test]
    fn refcount_plan_keeps_loop_iterator_live_across_check_jump() {
        let (_function, actions) = refcount_actions_for_function(
            r#"
def f(cls):
    for item in cls.__mro__:
        item.__name__
"#,
        );

        assert!(
            !actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::ReleaseLocal {
                    local,
                    reason: RefcountReleaseReason::Jump { .. },
                    ..
                } if local.name.starts_with("_dp_iter_")
            )),
            "loop iterator slot should stay live across jumps back to the loop check: {actions:#?}"
        );
    }

    #[test]
    fn refcount_plan_keeps_owned_cell_live_across_cell_backed_condition() {
        let (_function, actions) = refcount_actions_for_function(
            r#"
def f(reason):
    def decorator(test_item):
        return reason

    if isinstance(reason, int):
        return decorator(reason)
    return decorator
"#,
        );

        assert!(
            !actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::ReleaseLocal {
                    local,
                    reason: RefcountReleaseReason::Jump { .. },
                    ..
                } if local.name == "_dp_cell_reason"
            )),
            "owned cell backing slot must stay live for later cell-backed reads: {actions:#?}"
        );
    }

    #[test]
    fn refcount_plan_records_owned_cell_makecell_rebind() {
        let (_function, actions) = refcount_actions_for_named_function(
            r#"
def outer():
    x = 1
    def inner():
        return x
    return inner
"#,
            "outer",
        );

        assert!(
            actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::RebindLocal {
                    local,
                    old_state: LocalRefState::Unbound,
                    new_state: LocalRefState::Owned,
                } if local.name == "x" || local.name == "_dp_cell_x"
            )),
            "owned cell initialization should register a local rebind for the backing slot: {actions:#?}"
        );
    }

    #[test]
    fn forwarded_locations_uses_explicit_edge_source_names() {
        let target = BlockLabel::from_index(1);
        let target_params =
            HashMap::from([(target, vec!["target_x".to_string(), "target_y".to_string()])]);
        let location_by_name = HashMap::from([
            ("target_x".to_string(), LocalLocation(10)),
            ("target_y".to_string(), LocalLocation(11)),
            ("source_x".to_string(), LocalLocation(3)),
        ]);

        let forwarded = forwarded_locations(
            target,
            Some(&[BlockArg::Name("source_x".to_string())]),
            &target_params,
            &location_by_name,
        );

        assert_eq!(
            forwarded,
            HashSet::from([LocalLocation(10), LocalLocation(3)]),
            "explicit edge args should preserve the source local location rather than the tail target param name",
        );
    }

    #[test]
    fn typed_exact_int_region_inputs_keep_hidden_locals_live_at_their_block() {
        for (return_plan, fallback_region, indexed_receiver) in [
            (false, false, false),
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let lowered = lower_python_to_blockpy_for_testing(
                r#"
def f(h, flag):
    if flag:
        value = 1
    else:
        value = 2
    if value:
        return 1
    return 0
"#,
            )
            .expect("hidden region-local fixture should lower")
            .blockpy_module;
            let mut typed = lower_blockpy_module_to_typed(lowered);
            let function = typed
                .callable_defs
                .iter_mut()
                .find(|function| function.names.qualname == "f")
                .expect("hidden region-local fixture should contain f");
            assert!(
                function.body_params().iter().any(|param| param.name == "h"),
                "h must be a real function parameter, not a fabricated local",
            );
            let h_location = function
                .storage_layout()
                .as_ref()
                .expect("typed function should have storage")
                .stack_slots()
                .iter()
                .position(|name| name == "h")
                .map(|slot| LocalLocation(u32::try_from(slot).expect("slot should fit in u32")))
                .expect("h should have a declared local slot");
            let entry_label = function.entry_block().label;
            let target_label = function
                .blocks
                .iter()
                .find(|block| {
                    block.label != entry_label
                        && matches!(block.term, BlockTerm::IfTerm(_))
                        && !block.param_names().any(|name| name == "h")
                })
                .map(|block| block.label)
                .expect("fixture should contain a non-entry conditional without an h block param");
            assert!(
                !compute_typed_function_local_live_ins(function)
                    .get(&target_label)
                    .is_some_and(|live| live.contains(&h_location)),
                "h should be dead until the exact-int sidecar introduces its hidden read",
            );

            let branch = function
                .blocks
                .iter_mut()
                .find_map(|block| match &mut block.term {
                    BlockTerm::IfTerm(if_term) if block.label == target_label => {
                        Some(&mut if_term.test)
                    }
                    _ => None,
                })
                .expect("target conditional should survive fixture setup");
            let source = branch
                .try_semantic_instr_id()
                .expect("target conditional should have a semantic instruction ID");
            let input = RegionInput {
                value: PlanValue::new(1, Rep::PyObjectBorrowed),
                source: if indexed_receiver {
                    RegionInputSource::IndexedField {
                        source,
                        receiver: IndexedFieldReceiverSource::LocalName {
                            name: "h".to_string(),
                        },
                        owner_type: IndexedFieldOwnerType {
                            module_name: "fixture".to_string(),
                            qualname: "Record".to_string(),
                        },
                        attr_name: "value".to_string(),
                        expected_index: 0,
                    }
                } else {
                    RegionInputSource::FunctionParam {
                        index: 0,
                        name: Some("h".to_string()),
                    }
                },
            };
            let mut hot_plan = RegionPlan {
                id: RegionId(0),
                source: RegionSource::Instr { instr_id: source },
                inputs: Vec::new(),
                nodes: Vec::new(),
                exits: Vec::new(),
            };
            let mut fallback_plan = hot_plan.clone();
            fallback_plan.id = RegionId(1);
            if fallback_region {
                fallback_plan.inputs.push(input);
            } else {
                hot_plan.inputs.push(input);
            }
            let hot_region = MechanicalRegionEmission {
                region: hot_plan.id,
                steps: Vec::new(),
                exits: Vec::new(),
            };
            let fallback_emission = MechanicalRegionEmission {
                region: fallback_plan.id,
                steps: Vec::new(),
                exits: Vec::new(),
            };
            let extra = branch
                .typed_extra_mut()
                .expect("target conditional should support exact-int plans");
            if return_plan {
                extra.set_exact_int_return_plan(TypedExactIntReturnPlan {
                    source: TypedExactIntPlanSource::OptimizationPlanV3,
                    instr_id: source,
                    hot_plan,
                    hot_region,
                    fallback_plan,
                    fallback_region: fallback_emission,
                });
            } else {
                extra.set_exact_int_branch_plan(TypedExactIntBranchPlan {
                    source: TypedExactIntPlanSource::OptimizationPlanV3,
                    instr_id: source,
                    hot_plan,
                    hot_region,
                    fallback_plan,
                    fallback_region: fallback_emission,
                });
            }

            assert!(
                compute_typed_function_local_live_ins(function)
                    .get(&target_label)
                    .is_some_and(|live| live.contains(&h_location)),
                "exact-int {kind} {region} {input} must keep declared parameter h live in its actual block",
                kind = if return_plan { "return" } else { "branch" },
                region = if fallback_region { "fallback" } else { "hot" },
                input = if indexed_receiver {
                    "indexed-field receiver"
                } else {
                    "named local input"
                },
            );
        }
    }

    #[test]
    fn typed_precise_immortal_store_propagates_to_successor_entry_cleanup() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(flag):
    y = flag == flag
    if flag:
        pass
    return y
"#,
        )
        .expect("transform should succeed")
        .blockpy_module;
        let facts = infer_module_value_facts(&lowered);
        let mut typed = lower_blockpy_module_to_typed(lowered);
        let function = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "f")
            .expect("missing typed function f");
        let mut patched = false;
        for block in &mut function.blocks {
            for instr in &mut block.body {
                let InstrTyped::Store(op) = instr else {
                    continue;
                };
                if op.name.id.as_str() != "y" {
                    continue;
                }
                let Some(extra) = op.value.typed_extra_mut() else {
                    panic!("typed store value should carry typed extra");
                };
                extra.set_planned_result(TypedPlannedResult::PYOBJECT_IMMORTAL);
                patched = true;
            }
        }
        assert!(patched, "test should patch the y store value");

        let function_id = function.function_id;
        let entry_states =
            compute_typed_function_precise_immortal_local_entry_states(function, &facts);
        let y_location = function
            .storage_layout()
            .as_ref()
            .expect("typed function should have storage")
            .stack_slots()
            .iter()
            .position(|name| name == "y")
            .map(|slot| LocalLocation(u32::try_from(slot).expect("slot should fit in u32")))
            .expect("y should have a local slot");
        assert!(
            entry_states
                .values()
                .any(|states| states.get(&y_location) == Some(&LocalRefState::Immortal)),
            "successor entry state should preserve the planned immortal store: {entry_states:#?}"
        );

        let local_env_plan = plan_typed_local_env_module(&typed, &facts);
        let local_env_function = local_env_plan
            .function(function_id)
            .expect("function should have LocalEnv plan");
        assert!(
            local_env_function.blocks.values().any(|block| {
                block.entry_locals.iter().any(|binding| {
                    binding.location == y_location
                        && binding.param_facts.ownership == LocalRefKind::Immortal
                })
            }),
            "LocalEnv should consume the same precise immortal entry state: {local_env_function:#?}"
        );

        let plan = plan_typed_ownership_effects(&typed, &facts);
        validate_typed_ownership_effects(&typed, &facts, &plan)
            .expect("typed ownership plan should validate");
        let function_plan = plan
            .function(function_id)
            .expect("function should have refcount plan");
        let actions = function_plan
            .blocks
            .values()
            .flat_map(|block| block.actions.iter().map(|action| &action.kind))
            .collect::<Vec<_>>();
        assert!(
            actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::RebindLocal {
                    local,
                    new_state: LocalRefState::Immortal,
                    ..
                } if local.name == "y"
            )),
            "the patched store should be planned as an immortal rebind: {actions:#?}"
        );
        assert!(
            !actions.iter().any(|action| matches!(
                action,
                RefcountActionKind::ReleaseLocal {
                    local,
                    reason: RefcountReleaseReason::Return,
                    ..
                } if local.name == "y"
            )),
            "planned immortal y should not get return cleanup after a successor block: {actions:#?}"
        );
    }

    #[test]
    fn local_dataflow_accepts_sparse_block_labels() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(flag):
    x = []
    if flag:
        x = [1]
    return x
"#,
        )
        .expect("transform should succeed")
        .blockpy_module;
        let mut function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f")
            .clone();
        let relabel = function
            .blocks
            .iter()
            .enumerate()
            .map(|(index, block)| (block.label, BlockLabel::from_index(index * 3)))
            .collect::<HashMap<_, _>>();
        for block in &mut function.blocks {
            block.label = *relabel
                .get(&block.label)
                .expect("test relabel should cover block");
            for (from, to) in &relabel {
                block.term.replace_target(*from, *to);
            }
            if let Some(exc_edge) = &mut block.exc_edge {
                exc_edge.target = *relabel
                    .get(&exc_edge.target)
                    .expect("test relabel should cover exception target");
            }
        }

        let labels = function
            .blocks
            .iter()
            .map(|block| block.label)
            .collect::<HashSet<_>>();

        let live_ins = compute_function_local_live_ins(&function);
        let must_bound_ins = compute_function_local_must_bound_ins(&function);

        assert_eq!(live_ins.keys().copied().collect::<HashSet<_>>(), labels);
        assert_eq!(
            must_bound_ins.keys().copied().collect::<HashSet<_>>(),
            labels
        );
    }

    #[test]
    fn refcount_plan_validator_accepts_lowered_plan() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(flag):
    x = []
    if flag:
        return x
    del x
    return None
"#,
        )
        .expect("transform should succeed")
        .blockpy_module;
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_ownership_effects(&lowered, &facts);

        validate_ownership_effects(&lowered, &facts, &plan).expect("lowered plan should validate");
    }

    #[test]
    fn refcount_plan_validator_rejects_missing_return_release() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f():
    x = []
    return None
"#,
        )
        .expect("transform should succeed")
        .blockpy_module;
        let facts = infer_module_value_facts(&lowered);
        let mut plan = plan_ownership_effects(&lowered, &facts);
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f");
        for block_plan in plan
            .functions
            .get_mut(&function.function_id)
            .expect("missing function refcount plan")
            .blocks
            .values_mut()
        {
            block_plan.actions.retain(|action| {
                !matches!(
                    &action.kind,
                    RefcountActionKind::ReleaseLocal {
                        local,
                        reason: RefcountReleaseReason::Return,
                        ..
                    } if local.name == "x"
                )
            });
        }

        let err = validate_ownership_effects(&lowered, &facts, &plan)
            .expect_err("missing release should fail validation");
        assert!(
            err.contains("missing refcount release"),
            "unexpected validation error: {err}"
        );
    }
}
