use crate::block_py::{
    Block, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, FunctionId, HasSemanticInstrId,
    InstrCodegen, InstrKey, LocalLocation, NameLike,
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

pub fn lower_refcount_ownership(
    module: &BlockPyModule<CodegenModuleShape>,
    facts: &FactStore,
) -> RefcountPlan {
    let deleted_sentinel_constants = deleted_sentinel_constant_slots(module);
    let functions = module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                plan_function_refcounts(function, facts, &deleted_sentinel_constants),
            )
        })
        .collect();
    RefcountPlan { functions }
}

pub fn validate_refcount_plan(
    module: &BlockPyModule<CodegenModuleShape>,
    facts: &FactStore,
    plan: &RefcountPlan,
) -> Result<(), String> {
    let mut errors = Vec::new();
    let deleted_sentinel_constants = deleted_sentinel_constant_slots(module);
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
            forwarded_locations(edge.target, target_params, location_by_name),
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
            forwarded_locations(edge.target, target_params, location_by_name),
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
                forwarded_locations(if_term.then_label, target_params, location_by_name),
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
                forwarded_locations(if_term.else_label, target_params, location_by_name),
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
                    forwarded_locations(*target, target_params, location_by_name),
                    RefcountReleaseReason::BranchCase { target: *target },
                    &mut actions,
                );
            }
            release_unforwarded_locals(
                function.function_id,
                block.label,
                &env,
                locals,
                forwarded_locations(branch.default_label, target_params, location_by_name),
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
            forwarded_locations(edge.target, target_params, location_by_name),
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
            forwarded_locations(edge.target, target_params, location_by_name),
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
                forwarded_locations(if_term.then_label, target_params, location_by_name),
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
                forwarded_locations(if_term.else_label, target_params, location_by_name),
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
                    forwarded_locations(*target, target_params, location_by_name),
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
                forwarded_locations(branch.default_label, target_params, location_by_name),
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

fn state_for_py_facts(facts: PyObjFacts) -> LocalRefState {
    if facts.is_immortal() {
        LocalRefState::Immortal
    } else {
        LocalRefState::Owned
    }
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
        lower_refcount_ownership, validate_refcount_plan, LocalRefState, RefcountActionKind,
        RefcountReleaseReason,
    };
    use crate::block_py::BlockTerm;
    use crate::lower_python_to_blockpy_for_testing;
    use crate::passes::infer_module_value_facts;

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
        let plan = lower_refcount_ownership(&lowered, &facts);
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
        let plan = lower_refcount_ownership(&lowered, &facts);

        validate_refcount_plan(&lowered, &facts, &plan).expect("lowered plan should validate");
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
        let mut plan = lower_refcount_ownership(&lowered, &facts);
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

        let err = validate_refcount_plan(&lowered, &facts, &plan)
            .expect_err("missing release should fail validation");
        assert!(
            err.contains("missing refcount release"),
            "unexpected validation error: {err}"
        );
    }
}
