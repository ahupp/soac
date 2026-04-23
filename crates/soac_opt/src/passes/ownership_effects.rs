//! Semantic Python ownership planning.
//!
//! This pass intentionally does not insert physical INCREF/DECREF calls into
//! BlockPy. It records ownership effects: local rebinds, deletes, transfers,
//! and cleanup obligations. Backends lower those effects to concrete refcount
//! operations once representation choices such as SSA block params, stack-slot
//! mirrors, borrowed helper results, and immortal constants are known.

use crate::passes::{
    CodegenModuleShape, FactStore, InstrCodegen, InstrTyped, PyObjFacts, TypedCodegenModuleShape,
    ValueFacts,
};
use soac_core::block_py::{
    Block, BlockArg, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, CellLocation,
    ChildVisitable, HasSemanticInstrId, InstrKey, LocalLocation, RuntimeFunctionId, Visit,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum LocalRefState {
    Unbound,
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
    module: &BlockPyModule<CodegenModuleShape>,
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
    module: &BlockPyModule<CodegenModuleShape>,
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
    module: &BlockPyModule<TypedCodegenModuleShape>,
    facts: &FactStore,
) -> RefcountPlan {
    let functions = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                plan_typed_function_refcounts(function, facts),
            )
        })
        .collect();
    RefcountPlan { functions }
}

pub fn validate_typed_ownership_effects(
    module: &BlockPyModule<TypedCodegenModuleShape>,
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
        validate_typed_function_refcount_plan(function, facts, plan, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

fn validate_function_refcount_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
    let local_liveness = compute_typed_local_liveness(function, &location_by_name);
    let local_must_bound = compute_typed_local_must_bound(function, &location_by_name);
    let owned_cell_locations = typed_owned_cell_locations(function, &location_by_name);
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
            errors,
        );
    }
}

fn plan_function_refcounts(
    function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
    let local_liveness = compute_typed_local_liveness(function, &location_by_name);
    let local_must_bound = compute_typed_local_must_bound(function, &location_by_name);
    let owned_cell_locations = typed_owned_cell_locations(function, &location_by_name);

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
                ),
            )
        })
        .collect();
    FunctionRefcountPlan { blocks }
}

pub fn compute_function_local_live_ins(
    function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &Block<InstrCodegen>,
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
            InstrCodegen::Store(op) => {
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
            InstrCodegen::Del(op) => {
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    block: &Block<InstrTyped>,
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
    let mut env = initial_typed_block_env(
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
            InstrTyped::LegacyStore(op) => {
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
                let new_state = state_for_typed_expr(function.function_id, &op.value, facts);
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
            InstrTyped::LegacyDel(op) => {
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
fn validate_block_refcount_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &Block<InstrCodegen>,
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
            InstrCodegen::Store(op) => {
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
            InstrCodegen::Del(op) => {
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    block: &Block<InstrTyped>,
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
    );
    if &expected != block_plan {
        errors.push(format!(
            "typed refcount plan mismatch for function {} ({}) block {}\nexpected: {expected:#?}\nactual: {block_plan:#?}",
            function.function_id, function.names.qualname, block.label
        ));
    }
}

fn initial_block_env(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &Block<InstrCodegen>,
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
        for param in function.params.iter() {
            if let Some(location) = location_by_name.get(&param.name) {
                env.insert(*location, LocalRefState::Owned);
            }
        }
    }

    for name in block.param_names() {
        if let Some(location) = location_by_name.get(name) {
            env.insert(*location, LocalRefState::Owned);
        }
    }

    for location in must_bound_on_entry.locations() {
        env.insert(location, LocalRefState::Owned);
    }

    if let Some(entry_facts) = facts.block_entry_fact(function.function_id, block.label) {
        for (location, py_facts) in entry_facts.local_pyobj_facts() {
            if must_bound_on_entry.contains(location) {
                env.insert(location, state_for_py_facts(py_facts));
            }
        }
    }

    env
}

fn state_for_expr(
    function_id: RuntimeFunctionId,
    expr: &InstrCodegen,
    facts: &FactStore,
) -> LocalRefState {
    match facts.fact_for(expr.semantic_instr_key(function_id)) {
        Some(ValueFacts::PyObj(py_facts)) => state_for_py_facts(py_facts),
        Some(_) | None => LocalRefState::Owned,
    }
}

fn initial_typed_block_env(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    block: &Block<InstrTyped>,
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
        for param in function.params.iter() {
            if let Some(location) = location_by_name.get(&param.name) {
                env.insert(*location, LocalRefState::Owned);
            }
        }
    }

    for name in block.param_names() {
        if let Some(location) = location_by_name.get(name) {
            env.insert(*location, LocalRefState::Owned);
        }
    }

    for location in must_bound_on_entry.locations() {
        env.insert(location, LocalRefState::Owned);
    }

    if let Some(entry_facts) = facts.block_entry_fact(function.function_id, block.label) {
        for (location, py_facts) in entry_facts.local_pyobj_facts() {
            if must_bound_on_entry.contains(location) {
                env.insert(location, state_for_py_facts(py_facts));
            }
        }
    }

    env
}

fn state_for_typed_expr(
    function_id: RuntimeFunctionId,
    expr: &InstrTyped,
    facts: &FactStore,
) -> LocalRefState {
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
    function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> LocalLiveness {
    let owned_cell_locations = typed_owned_cell_locations(function, location_by_name);
    let local_count = location_by_name.len();
    let effects_by_block = function
        .blocks
        .iter()
        .map(|block| {
            (
                block.label,
                typed_block_local_effects(block, location_by_name, &owned_cell_locations),
            )
        })
        .collect::<HashMap<_, _>>();
    let successors_by_block = function
        .blocks
        .iter()
        .map(|block| (block.label, typed_block_successors(block)))
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

fn compute_typed_local_must_bound(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> LocalMustBound {
    let owned_cell_locations = typed_owned_cell_locations(function, location_by_name);
    let local_count = location_by_name.len();
    let labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<Vec<_>>();
    let successors_by_block = function
        .blocks
        .iter()
        .map(|block| (block.label, typed_block_successors(block)))
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
            let out = transfer_typed_must_bound_through_block(
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
            let new_out = transfer_typed_must_bound_through_block(
                function,
                block,
                &new_in,
                &owned_cell_locations,
            );
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

fn transfer_must_bound_through_block(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &Block<InstrCodegen>,
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
            InstrCodegen::Store(op) => {
                if let Some(location) = store_binding_location(op, owned_cell_locations) {
                    must_bound.insert(location);
                }
            }
            InstrCodegen::Del(op) => {
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    block: &Block<InstrTyped>,
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
            InstrTyped::LegacyStore(op) => {
                if let Some(location) = typed_store_binding_location(op, owned_cell_locations) {
                    must_bound.insert(location);
                }
            }
            InstrTyped::LegacyDel(op) => {
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
    block: &Block<InstrCodegen>,
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
            InstrCodegen::Store(op) => {
                if let Some(location) = op.name.local_location() {
                    effects.defs.insert(location);
                }
            }
            InstrCodegen::Del(op) => {
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
    block: &Block<InstrTyped>,
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
            InstrTyped::LegacyStore(op) => {
                if let Some(location) = op.name.local_location() {
                    effects.defs.insert(location);
                }
            }
            InstrTyped::LegacyDel(op) => {
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
    function: &BlockPyFunction<CodegenModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashMap<u32, LocalLocation> {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    storage_layout
        .cellvars
        .iter()
        .chain(storage_layout.runtime_cells.iter())
        .enumerate()
        .filter_map(|(slot, cell)| {
            let location = location_by_name.get(cell.storage_name.as_str()).copied()?;
            let slot = u32::try_from(slot).expect("owned cell slot should fit in u32");
            Some((slot, location))
        })
        .collect()
}

fn typed_owned_cell_locations(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashMap<u32, LocalLocation> {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return HashMap::new();
    };
    storage_layout
        .cellvars
        .iter()
        .chain(storage_layout.runtime_cells.iter())
        .enumerate()
        .filter_map(|(slot, cell)| {
            let location = location_by_name.get(cell.storage_name.as_str()).copied()?;
            let slot = u32::try_from(slot).expect("owned cell slot should fit in u32");
            Some((slot, location))
        })
        .collect()
}

fn store_binding_location(
    op: &soac_core::block_py::Store<InstrCodegen>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
) -> Option<LocalLocation> {
    op.name.local_location().or_else(|| {
        let CellLocation::Owned(slot) = op.name.cell_location()? else {
            return None;
        };
        matches!(op.value.as_ref(), InstrCodegen::MakeCell(_))
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
        matches!(op.value.as_ref(), InstrTyped::LegacyMakeCell(_))
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
    expr: &InstrCodegen,
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

    impl Visit<InstrCodegen> for LocalReadCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            match expr {
                InstrCodegen::Load(op) => {
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
                InstrCodegen::Store(op) => {
                    if let Some(cell_location) = op.name.cell_location() {
                        mark_cell_use(
                            cell_location,
                            self.defs,
                            self.owned_cell_locations,
                            self.uses,
                        );
                    }
                }
                InstrCodegen::Del(op) => {
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
                InstrCodegen::CellRef(op) => {
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
    term: &BlockTerm<InstrCodegen>,
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

    impl Visit<InstrCodegen> for TermReadCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
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

fn block_successors(block: &Block<InstrCodegen>) -> Vec<BlockLabel> {
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

    impl Visit<InstrTyped> for LocalReadCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped)
        where
            InstrTyped: ChildVisitable<InstrTyped>,
        {
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
                InstrTyped::LegacyStore(op) => {
                    if let Some(cell_location) = op.name.cell_location() {
                        mark_cell_use(
                            cell_location,
                            self.defs,
                            self.owned_cell_locations,
                            self.uses,
                        );
                    }
                }
                InstrTyped::LegacyDel(op) => {
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
                InstrTyped::LegacyCellRef(op) => {
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

fn typed_block_successors(block: &Block<InstrTyped>) -> Vec<BlockLabel> {
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
    function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<CodegenModuleShape>,
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
    function: &BlockPyFunction<CodegenModuleShape>,
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
        compute_function_local_must_bound_ins, forwarded_locations, plan_ownership_effects,
        validate_ownership_effects,
    };
    use crate::passes::infer_module_value_facts;
    use soac_core::block_py::{BlockArg, BlockLabel, BlockPyFunction, BlockTerm, LocalLocation};
    use soac_lowering::lower_python_to_blockpy_for_testing;
    use std::collections::{HashMap, HashSet};

    fn refcount_actions_for_function(
        source: &str,
    ) -> (
        BlockPyFunction<crate::passes::CodegenModuleShape>,
        Vec<RefcountActionKind>,
    ) {
        refcount_actions_for_named_function(source, "f")
    }

    fn refcount_actions_for_named_function(
        source: &str,
        qualname: &str,
    ) -> (
        BlockPyFunction<crate::passes::CodegenModuleShape>,
        Vec<RefcountActionKind>,
    ) {
        let lowered = lower_python_to_blockpy_for_testing(source)
            .expect("transform should succeed")
            .codegen_module;
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
        .codegen_module;
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
        .codegen_module;
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
        .codegen_module;
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
