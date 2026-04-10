//! Semantic Python ownership planning.
//!
//! This pass intentionally does not insert physical INCREF/DECREF calls into
//! BlockPy. It records ownership effects: local rebinds, deletes, transfers,
//! and cleanup obligations. Backends lower those effects to concrete refcount
//! operations once representation choices such as SSA block params, stack-slot
//! mirrors, borrowed helper results, and immortal constants are known.

use crate::block_py::{
    Block, BlockArg, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, CallArgPositional,
    CellLocation, ChildVisitable, FunctionId, HasSemanticInstrId, InstrCodegen, InstrKey,
    LocalLocation, NameLike, Visit,
};
use crate::passes::{CodegenModuleShape, FactStore, InstrResolved, PyObjFacts, ValueFacts};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefcountLocal {
    pub location: LocalLocation,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum RefcountSite {
    Instr(InstrKey),
    Term {
        function_id: FunctionId,
        block_label: BlockLabel,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RefcountAction {
    pub site: RefcountSite,
    pub kind: RefcountActionKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockRefcountPlan {
    pub label: BlockLabel,
    pub actions: Vec<RefcountAction>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionRefcountPlan {
    pub blocks: HashMap<BlockLabel, BlockRefcountPlan>,
}

impl FunctionRefcountPlan {
    pub fn block(&self, label: BlockLabel) -> Option<&BlockRefcountPlan> {
        self.blocks.get(&label)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RefcountPlan {
    pub functions: HashMap<FunctionId, FunctionRefcountPlan>,
}

impl RefcountPlan {
    pub fn function(&self, function_id: FunctionId) -> Option<&FunctionRefcountPlan> {
        self.functions.get(&function_id)
    }
}

pub fn plan_ownership_effects(
    module: &BlockPyModule<CodegenModuleShape>,
    facts: &FactStore,
) -> RefcountPlan {
    let deleted_sentinel_constants = deleted_sentinel_constant_slots(module);
    let module_constant_runtime_names = module_constant_runtime_name_slots(module);
    let functions = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                plan_function_refcounts(
                    function,
                    facts,
                    &deleted_sentinel_constants,
                    &module_constant_runtime_names,
                ),
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
    let deleted_sentinel_constants = deleted_sentinel_constant_slots(module);
    let module_constant_runtime_names = module_constant_runtime_name_slots(module);
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
        validate_function_refcount_plan(
            function,
            facts,
            plan,
            &deleted_sentinel_constants,
            &module_constant_runtime_names,
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
    function: &BlockPyFunction<CodegenModuleShape>,
    facts: &FactStore,
    plan: &RefcountPlan,
    deleted_sentinel_constants: &HashSet<u32>,
    module_constant_runtime_names: &HashMap<u32, String>,
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
    let local_liveness =
        compute_local_liveness(function, &location_by_name, module_constant_runtime_names);
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
            &target_params,
            &local_liveness,
            deleted_sentinel_constants,
            block.label == entry_label,
            errors,
        );
    }
}

fn plan_function_refcounts(
    function: &BlockPyFunction<CodegenModuleShape>,
    facts: &FactStore,
    deleted_sentinel_constants: &HashSet<u32>,
    module_constant_runtime_names: &HashMap<u32, String>,
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
    let local_liveness =
        compute_local_liveness(function, &location_by_name, module_constant_runtime_names);

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
                    &target_params,
                    &local_liveness,
                    deleted_sentinel_constants,
                    block.label == entry_label,
                ),
            )
        })
        .collect();
    FunctionRefcountPlan { blocks }
}

fn plan_block_refcounts(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &Block<InstrCodegen>,
    facts: &FactStore,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location_by_name: &HashMap<String, LocalLocation>,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    deleted_sentinel_constants: &HashSet<u32>,
    is_entry_block: bool,
) -> BlockRefcountPlan {
    let mut env = initial_block_env(
        function,
        block,
        facts,
        locals,
        location_by_name,
        is_entry_block,
    );
    let mut actions = Vec::new();

    for instr in &block.body {
        match instr {
            InstrCodegen::Store(op) => {
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
                if expr_is_deleted_sentinel(&op.value, deleted_sentinel_constants) {
                    actions.push(RefcountAction {
                        site: RefcountSite::Instr(instr.semantic_instr_key(function.function_id)),
                        kind: RefcountActionKind::DeleteLocal { local, old_state },
                    });
                    env.insert(location, LocalRefState::Unbound);
                    continue;
                }
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
            preserved_locations(edge.target, local_liveness, target_params, location_by_name),
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
            preserved_locations(edge.target, local_liveness, target_params, location_by_name),
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
                    preserved_locations(*target, local_liveness, target_params, location_by_name),
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
    target_params: &HashMap<BlockLabel, Vec<String>>,
    local_liveness: &LocalLiveness,
    deleted_sentinel_constants: &HashSet<u32>,
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

    let mut env = initial_block_env(
        function,
        block,
        facts,
        locals,
        location_by_name,
        is_entry_block,
    );

    for instr in &block.body {
        match instr {
            InstrCodegen::Store(op) => {
                let site = RefcountSite::Instr(instr.semantic_instr_key(function.function_id));
                let actions = actions_by_site.remove(&site).unwrap_or_default();
                let Some(location) = op.name.local_location() else {
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
                if expr_is_deleted_sentinel(&op.value, deleted_sentinel_constants) {
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
                    continue;
                }
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
            preserved_locations(edge.target, local_liveness, target_params, location_by_name),
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
            preserved_locations(edge.target, local_liveness, target_params, location_by_name),
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
                    preserved_locations(*target, local_liveness, target_params, location_by_name),
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

fn initial_block_env(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &Block<InstrCodegen>,
    facts: &FactStore,
    locals: &HashMap<LocalLocation, RefcountLocal>,
    location_by_name: &HashMap<String, LocalLocation>,
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

    if let Some(entry_facts) = facts.block_entry_fact(function.function_id, block.label) {
        for (location, py_facts) in entry_facts.local_pyobj_facts() {
            env.insert(location, state_for_py_facts(py_facts));
        }
    }

    env
}

fn state_for_expr(
    function_id: FunctionId,
    expr: &InstrCodegen,
    facts: &FactStore,
) -> LocalRefState {
    match facts.fact_for(expr.semantic_instr_key(function_id)) {
        Some(ValueFacts::PyObj(py_facts)) => state_for_py_facts(py_facts),
        Some(_) | None => LocalRefState::Owned,
    }
}

fn expr_is_deleted_sentinel(
    expr: &InstrCodegen,
    deleted_sentinel_constants: &HashSet<u32>,
) -> bool {
    match expr {
        InstrCodegen::Load(op) if op.name.is_runtime_symbol("DELETED") => true,
        InstrCodegen::Load(op) => op
            .name
            .location
            .as_constant()
            .is_some_and(|index| deleted_sentinel_constants.contains(&index)),
        _ => false,
    }
}

fn deleted_sentinel_constant_slots(module: &BlockPyModule<CodegenModuleShape>) -> HashSet<u32> {
    module
        .module_constants
        .iter()
        .enumerate()
        .filter_map(|(index, constant)| match constant {
            InstrResolved::Load(op) if op.name.is_runtime_symbol("DELETED") => {
                Some(u32::try_from(index).expect("module constant index should fit in u32"))
            }
            _ => None,
        })
        .collect()
}

fn module_constant_runtime_name_slots(
    module: &BlockPyModule<CodegenModuleShape>,
) -> HashMap<u32, String> {
    module
        .module_constants
        .iter()
        .enumerate()
        .filter_map(|(index, constant)| match constant {
            InstrResolved::Load(op) if op.name.is_runtime_name() => Some((
                u32::try_from(index).expect("module constant index should fit in u32"),
                op.name.id_str().to_string(),
            )),
            _ => None,
        })
        .collect()
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
    live_in_by_block: HashMap<BlockLabel, HashSet<LocalLocation>>,
}

impl LocalLiveness {
    fn live_in(&self, label: BlockLabel) -> Option<&HashSet<LocalLocation>> {
        self.live_in_by_block.get(&label)
    }
}

#[derive(Clone, Debug, Default)]
struct BlockLocalEffects {
    uses: HashSet<LocalLocation>,
    defs: HashSet<LocalLocation>,
}

fn compute_local_liveness(
    function: &BlockPyFunction<CodegenModuleShape>,
    location_by_name: &HashMap<String, LocalLocation>,
    module_constant_runtime_names: &HashMap<u32, String>,
) -> LocalLiveness {
    let owned_cell_locations = owned_cell_locations(function, location_by_name);
    let effects_by_block = function
        .blocks
        .iter()
        .map(|block| {
            (
                block.label,
                block_local_effects(
                    block,
                    location_by_name,
                    &owned_cell_locations,
                    module_constant_runtime_names,
                ),
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
        .map(|block| (block.label, HashSet::new()))
        .collect::<HashMap<_, _>>();

    let mut changed = true;
    while changed {
        changed = false;
        for block in function.blocks.iter().rev() {
            let effects = effects_by_block
                .get(&block.label)
                .expect("liveness effects should exist for every block");
            let mut live_out = HashSet::new();
            for successor in successors_by_block
                .get(&block.label)
                .expect("successors should exist for every block")
            {
                if let Some(successor_live_in) = live_in_by_block.get(successor) {
                    live_out.extend(successor_live_in.iter().copied());
                }
            }

            let mut new_live_in = effects.uses.clone();
            new_live_in.extend(
                live_out
                    .into_iter()
                    .filter(|location| !effects.defs.contains(location)),
            );
            let entry = live_in_by_block
                .get_mut(&block.label)
                .expect("live-in should exist for every block");
            if *entry != new_live_in {
                *entry = new_live_in;
                changed = true;
            }
        }
    }

    LocalLiveness { live_in_by_block }
}

fn block_local_effects(
    block: &Block<InstrCodegen>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    module_constant_runtime_names: &HashMap<u32, String>,
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
            module_constant_runtime_names,
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
        module_constant_runtime_names,
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

fn should_include_in_locals_snapshot(name: &str) -> bool {
    !name.starts_with("_dp_") && name != "__soac__"
}

fn codegen_expr_helper_name<'a>(
    expr: &'a InstrCodegen,
    module_constant_runtime_names: &'a HashMap<u32, String>,
) -> Option<&'a str> {
    match expr {
        InstrCodegen::Load(op)
            if op.name.location.is_global()
                || op.name.location.is_global_name()
                || op.name.location.is_runtime_name() =>
        {
            Some(op.name.id_str())
        }
        InstrCodegen::Load(op) => op.name.location.as_constant().and_then(|index| {
            module_constant_runtime_names
                .get(&index)
                .map(String::as_str)
        }),
        _ => None,
    }
}

fn is_current_scope_locals_snapshot_call(
    expr: &InstrCodegen,
    module_constant_runtime_names: &HashMap<u32, String>,
) -> bool {
    let InstrCodegen::Call(call) = expr else {
        return false;
    };
    if !call.keywords.is_empty()
        || call
            .args
            .iter()
            .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
    {
        return false;
    }
    match codegen_expr_helper_name(call.func.as_ref(), module_constant_runtime_names) {
        Some("locals") => call.args.is_empty(),
        Some("eval" | "exec") => call.args.len() == 1,
        _ => false,
    }
}

fn mark_current_scope_locals_snapshot_uses(
    defs: &HashSet<LocalLocation>,
    location_by_name: &HashMap<String, LocalLocation>,
    uses: &mut HashSet<LocalLocation>,
) {
    for (name, location) in location_by_name {
        if should_include_in_locals_snapshot(name) {
            mark_local_use(*location, defs, uses);
        }
    }
}

fn collect_local_reads(
    expr: &InstrCodegen,
    defs: &HashSet<LocalLocation>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    module_constant_runtime_names: &HashMap<u32, String>,
    uses: &mut HashSet<LocalLocation>,
) {
    struct LocalReadCollector<'a> {
        defs: &'a HashSet<LocalLocation>,
        location_by_name: &'a HashMap<String, LocalLocation>,
        owned_cell_locations: &'a HashMap<u32, LocalLocation>,
        module_constant_runtime_names: &'a HashMap<u32, String>,
        uses: &'a mut HashSet<LocalLocation>,
    }

    impl Visit<InstrCodegen> for LocalReadCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen)
        where
            InstrCodegen: ChildVisitable<InstrCodegen>,
        {
            if is_current_scope_locals_snapshot_call(expr, self.module_constant_runtime_names) {
                mark_current_scope_locals_snapshot_uses(
                    self.defs,
                    self.location_by_name,
                    self.uses,
                );
            }
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
        module_constant_runtime_names,
        uses,
    }
    .visit_instr(expr);
}

fn collect_term_local_reads(
    term: &BlockTerm<InstrCodegen>,
    defs: &HashSet<LocalLocation>,
    location_by_name: &HashMap<String, LocalLocation>,
    owned_cell_locations: &HashMap<u32, LocalLocation>,
    module_constant_runtime_names: &HashMap<u32, String>,
    uses: &mut HashSet<LocalLocation>,
) {
    struct TermReadCollector<'a> {
        defs: &'a HashSet<LocalLocation>,
        location_by_name: &'a HashMap<String, LocalLocation>,
        owned_cell_locations: &'a HashMap<u32, LocalLocation>,
        module_constant_runtime_names: &'a HashMap<u32, String>,
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
                self.module_constant_runtime_names,
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
        module_constant_runtime_names,
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

fn forwarded_locations(
    target: BlockLabel,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashSet<LocalLocation> {
    target_params
        .get(&target)
        .into_iter()
        .flat_map(|params| params.iter())
        .filter_map(|name| location_by_name.get(name).copied())
        .collect()
}

fn preserved_locations(
    target: BlockLabel,
    local_liveness: &LocalLiveness,
    target_params: &HashMap<BlockLabel, Vec<String>>,
    location_by_name: &HashMap<String, LocalLocation>,
) -> HashSet<LocalLocation> {
    let mut preserved = forwarded_locations(target, target_params, location_by_name);
    if let Some(live_in) = local_liveness.live_in(target) {
        preserved.extend(live_in.iter().copied());
    }
    preserved
}

fn release_unforwarded_locals(
    function_id: FunctionId,
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
    function_id: FunctionId,
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
    function_id: FunctionId,
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
        plan_ownership_effects, validate_ownership_effects, LocalRefState, RefcountActionKind,
        RefcountReleaseReason,
    };
    use crate::block_py::{BlockTerm, NameLike};
    use crate::lower_python_to_blockpy_for_testing;
    use crate::passes::{infer_module_value_facts, InstrCodegen};

    fn refcount_actions_for_function(
        source: &str,
    ) -> (
        crate::block_py::BlockPyFunction<crate::passes::CodegenModuleShape>,
        Vec<RefcountActionKind>,
    ) {
        let lowered = lower_python_to_blockpy_for_testing(source)
            .expect("transform should succeed")
            .codegen_module;
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f")
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
    fn refcount_plan_keeps_forwarded_branch_locals_live() {
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
        let block_actions = actions
            .iter()
            .filter(|action| match action {
                RefcountActionKind::ReleaseLocal { local, reason, .. } if local.name == "x" => {
                    matches!(
                        reason,
                        RefcountReleaseReason::IfThen { .. } | RefcountReleaseReason::IfElse { .. }
                    )
                }
                _ => false,
            })
            .count();

        assert_eq!(
            block_actions, 0,
            "branch block {} should forward x to both return targets instead of releasing it",
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
    fn refcount_plan_keeps_loop_target_live_for_eval_locals_snapshot() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f():
    for item in [[]]:
        marker = "value"
        eval("item")
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_ownership_effects(&lowered, &facts);
        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("missing lowered function f");
        let item_store_block = function
            .blocks
            .iter()
            .find(|block| {
                block.body.iter().any(
                    |instr| matches!(instr, InstrCodegen::Store(op) if op.name.id_str() == "item"),
                )
            })
            .expect("expected a block that stores the for-loop target");
        let block_plan = plan
            .function(function.function_id)
            .and_then(|function_plan| function_plan.block(item_store_block.label))
            .expect("missing block refcount plan");

        assert!(
            !block_plan.actions.iter().any(|action| matches!(
                &action.kind,
                RefcountActionKind::ReleaseLocal {
                    local,
                    reason: RefcountReleaseReason::Jump { .. },
                    ..
                } if local.name == "item"
            )),
            "for-loop target must stay live across the jump into eval's locals snapshot: {block_plan:#?}"
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
