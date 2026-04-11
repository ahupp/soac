use soac_blockpy::block_py::{
    BlockArg, BlockLabel, BlockPyFunction, BlockPyModule, CodegenBlock, LocalLocation,
};
use soac_blockpy::passes::{
    CodegenModuleShape, FactStore, FunctionRefcountPlan, PyObjFacts, RefcountActionKind,
    RefcountReleaseReason, compute_function_local_live_ins, compute_function_local_must_bound_ins,
    plan_ownership_effects, validate_ownership_effects,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalRefKind {
    Unknown,
    Owned,
    Borrowed,
    Immortal,
    Unbound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedLocalStorage {
    BlockParam,
    StackSlot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParamBindingFacts {
    DefinitelyBound,
    CheckedLocalValue,
    MaybeUnbound,
}

impl ParamBindingFacts {
    pub const fn requires_checked_local_load(self) -> bool {
        !matches!(self, Self::DefinitelyBound)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParamProvenance {
    ExplicitBlockParam(LocalLocation),
    ForwardedLocal(LocalLocation),
    StackSlot(LocalLocation),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedLocalBinding {
    pub name: String,
    pub location: LocalLocation,
    pub ref_kind: LocalRefKind,
    pub facts: Option<PyObjFacts>,
    pub storage: PlannedLocalStorage,
    pub binding_facts: ParamBindingFacts,
    pub provenance: ParamProvenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockLocalPlan {
    pub label: BlockLabel,
    pub entry_locals: Vec<PlannedLocalBinding>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionLocalPlan {
    pub blocks: HashMap<BlockLabel, BlockLocalPlan>,
}

impl FunctionLocalPlan {
    pub fn block(&self, label: BlockLabel) -> Option<&BlockLocalPlan> {
        self.blocks.get(&label)
    }
}

pub fn plan_function_locals(
    function: &BlockPyFunction<CodegenModuleShape>,
    facts: &FactStore,
) -> FunctionLocalPlan {
    let Some(storage_layout) = function.storage_layout().as_ref() else {
        return FunctionLocalPlan::default();
    };
    let live_ins = compute_function_local_live_ins(function);
    let must_bound_ins = compute_function_local_must_bound_ins(function);
    let entry_label = function.entry_block().label;
    let mut blocks = HashMap::with_capacity(function.blocks.len());
    for block in &function.blocks {
        let entry_facts = facts.block_entry_fact(function.function_id, block.label);
        let live_in_locations = live_ins.get(&block.label).cloned().unwrap_or_default();
        let must_bound_locations = must_bound_ins
            .get(&block.label)
            .cloned()
            .unwrap_or_default();
        let explicit_param_names = block.param_names().collect::<HashSet<_>>();
        let entry_locals = storage_layout
            .stack_slots()
            .iter()
            .enumerate()
            .map(|(slot, name)| {
                let location = LocalLocation(
                    u32::try_from(slot).expect("storage layout slot index should fit in u32"),
                );
                let is_must_bound_on_entry = must_bound_locations.contains(&location);
                let py_facts = entry_facts
                    .and_then(|env| env.local_pyobj_fact(location))
                    .filter(|_| is_must_bound_on_entry);
                let storage = if explicit_param_names.contains(name.as_str()) {
                    PlannedLocalStorage::BlockParam
                } else if live_in_locations.contains(&location) {
                    PlannedLocalStorage::BlockParam
                } else {
                    PlannedLocalStorage::StackSlot
                };
                let binding_facts = if explicit_param_names.contains(name.as_str()) {
                    ParamBindingFacts::DefinitelyBound
                } else if is_must_bound_on_entry {
                    match storage {
                        PlannedLocalStorage::BlockParam => ParamBindingFacts::DefinitelyBound,
                        PlannedLocalStorage::StackSlot => ParamBindingFacts::CheckedLocalValue,
                    }
                } else {
                    ParamBindingFacts::MaybeUnbound
                };
                let provenance = if explicit_param_names.contains(name.as_str()) {
                    ParamProvenance::ExplicitBlockParam(location)
                } else if storage == PlannedLocalStorage::BlockParam {
                    ParamProvenance::ForwardedLocal(location)
                } else {
                    ParamProvenance::StackSlot(location)
                };
                PlannedLocalBinding {
                    name: name.clone(),
                    location,
                    ref_kind: local_ref_kind_for_block_entry(
                        function,
                        block.label == entry_label,
                        name,
                        explicit_param_names.contains(name.as_str()),
                        is_must_bound_on_entry,
                        py_facts,
                    ),
                    facts: py_facts,
                    storage,
                    binding_facts,
                    provenance,
                }
            })
            .collect();
        blocks.insert(
            block.label,
            BlockLocalPlan {
                label: block.label,
                entry_locals,
            },
        );
    }
    FunctionLocalPlan { blocks }
}

pub fn plan_function_refcount_ownership(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    facts: &FactStore,
) -> Result<FunctionRefcountPlan, String> {
    let plan = plan_ownership_effects(module, facts);
    validate_ownership_effects(module, facts, &plan)?;
    Ok(plan
        .function(function.function_id)
        .cloned()
        .unwrap_or_default())
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CurrentJitRefcountPlanCheck {
    pub local_rebinds: usize,
    pub local_deletes: usize,
    pub terminal_stack_slot_releases: usize,
    pub normal_edge_stack_slot_releases: usize,
    pub exception_edge_stack_slot_releases: usize,
    pub normal_edge_release_gaps: usize,
    pub exception_edge_release_gaps: usize,
}

impl CurrentJitRefcountPlanCheck {
    pub fn has_edge_release_gaps(&self) -> bool {
        self.normal_edge_release_gaps > 0 || self.exception_edge_release_gaps > 0
    }
}

pub fn check_refcount_plan_against_current_jit(
    function: &BlockPyFunction<CodegenModuleShape>,
    plan: &FunctionRefcountPlan,
) -> Result<CurrentJitRefcountPlanCheck, String> {
    let stack_slot_names = function
        .storage_layout()
        .as_ref()
        .map(|layout| layout.stack_slots().iter().cloned().collect::<HashSet<_>>())
        .unwrap_or_default();
    let block_labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<HashSet<_>>();
    let mut check = CurrentJitRefcountPlanCheck::default();
    let mut errors = Vec::new();

    for (block_label, block_plan) in &plan.blocks {
        if !block_labels.contains(block_label) {
            errors.push(format!(
                "refcount plan for function {} ({}) contains unknown block {block_label}",
                function.function_id, function.names.qualname
            ));
        }
        for action in &block_plan.actions {
            match &action.kind {
                RefcountActionKind::RebindLocal { local, .. } => {
                    check_local_has_stack_slot(
                        function,
                        &stack_slot_names,
                        &local.name,
                        &mut errors,
                    );
                    check.local_rebinds += 1;
                }
                RefcountActionKind::DeleteLocal { local, .. } => {
                    check_local_has_stack_slot(
                        function,
                        &stack_slot_names,
                        &local.name,
                        &mut errors,
                    );
                    check.local_deletes += 1;
                }
                RefcountActionKind::ReleaseLocal { local, reason, .. } => {
                    check_local_has_stack_slot(
                        function,
                        &stack_slot_names,
                        &local.name,
                        &mut errors,
                    );
                    match reason {
                        RefcountReleaseReason::Return | RefcountReleaseReason::Raise => {
                            check.terminal_stack_slot_releases += 1;
                        }
                        RefcountReleaseReason::Jump { .. }
                        | RefcountReleaseReason::IfThen { .. }
                        | RefcountReleaseReason::IfElse { .. }
                        | RefcountReleaseReason::BranchCase { .. }
                        | RefcountReleaseReason::BranchDefault { .. } => {
                            check.normal_edge_stack_slot_releases += 1;
                        }
                        RefcountReleaseReason::ExceptionEdge { .. } => {
                            check.exception_edge_stack_slot_releases += 1;
                        }
                    }
                }
            }
        }
    }

    if errors.is_empty() {
        Ok(check)
    } else {
        Err(errors.join("\n"))
    }
}

fn check_local_has_stack_slot(
    function: &BlockPyFunction<CodegenModuleShape>,
    stack_slot_names: &HashSet<String>,
    local_name: &str,
    errors: &mut Vec<String>,
) {
    if stack_slot_names.contains(local_name) {
        return;
    }
    errors.push(format!(
        "refcount plan action for function {} ({}) references local {local_name:?}, \
         but the current JIT cleanup model only tracks storage-layout stack slots",
        function.function_id, function.names.qualname
    ));
}

fn local_ref_kind_for_block_entry(
    function: &BlockPyFunction<CodegenModuleShape>,
    is_entry_block: bool,
    name: &str,
    is_explicit_block_param: bool,
    is_must_bound_on_entry: bool,
    facts: Option<PyObjFacts>,
) -> LocalRefKind {
    match facts {
        Some(facts) if facts.is_immortal() => return LocalRefKind::Immortal,
        Some(_) => return LocalRefKind::Owned,
        None => {}
    }
    if is_entry_block && function.params.iter().any(|param| param.name == name) {
        return LocalRefKind::Owned;
    }
    if is_explicit_block_param {
        return LocalRefKind::Owned;
    }
    if is_must_bound_on_entry {
        return LocalRefKind::Owned;
    }
    LocalRefKind::Unbound
}

#[derive(Clone, Debug)]
pub struct BlockExcDispatchPlan {
    pub target_index: usize,
    pub slot_writes: Vec<(String, BlockArg)>,
    pub target_args: Vec<(String, BlockArg)>,
    pub forwarded_local_names: Vec<String>,
}

pub fn jit_param_names_for_block(block: &CodegenBlock) -> Vec<String> {
    block.bb_param_names().map(ToString::to_string).collect()
}

pub fn planned_jit_param_names_for_block(
    block: &CodegenBlock,
    block_plan: Option<&BlockLocalPlan>,
) -> Vec<String> {
    let mut names = jit_param_names_for_block(block);
    if let Some(block_plan) = block_plan {
        for binding in &block_plan.entry_locals {
            if binding.storage != PlannedLocalStorage::BlockParam {
                continue;
            }
            if names.iter().any(|name| name == &binding.name) {
                continue;
            }
            names.push(binding.name.clone());
        }
    }
    names
}

pub fn exc_dispatch_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &CodegenBlock,
    runtime_target_params: &[String],
) -> Option<BlockExcDispatchPlan> {
    let exc_edge = block.exc_edge.as_ref()?;
    let target_index = exc_edge.target.index();
    let target_block = &function.blocks[target_index];
    let stack_slot_name_set = function
        .storage_layout()
        .as_ref()
        .map(|layout| {
            layout
                .stack_slots()
                .iter()
                .cloned()
                .into_iter()
                .collect::<HashSet<_>>()
        })
        .unwrap_or_default();
    let runtime_param_name_set = runtime_target_params
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let full_target_param_names = target_block.param_name_vec();
    let mut slot_writes = Vec::new();
    for (target_param_name, source) in full_target_param_names.iter().zip(exc_edge.args.iter()) {
        if runtime_param_name_set.contains(target_param_name.as_str())
            || !stack_slot_name_set.contains(target_param_name)
        {
            continue;
        }
        slot_writes.push((target_param_name.clone(), source.clone()));
    }
    let explicit_args_by_name = full_target_param_names
        .iter()
        .zip(exc_edge.args.iter())
        .map(|(name, arg)| (name.as_str(), arg))
        .collect::<HashMap<_, _>>();
    let target_args: Vec<(String, BlockArg)> = runtime_target_params
        .iter()
        .map(|name| {
            (
                name.clone(),
                explicit_args_by_name
                    .get(name.as_str())
                    .map(|arg| (*arg).clone())
                    .unwrap_or_else(|| BlockArg::Name(name.clone())),
            )
        })
        .collect();
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
    for (_, arg) in target_args.iter() {
        record_forwarded_name(arg);
    }
    Some(BlockExcDispatchPlan {
        target_index,
        slot_writes,
        target_args,
        forwarded_local_names,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_blockpy::block_py::BlockTerm;
    use soac_blockpy::lower_python_to_blockpy_for_testing;
    use soac_blockpy::passes::{
        RefcountActionKind, RefcountReleaseReason, infer_module_value_facts,
    };

    fn lowered_function(
        source: &str,
        qualname: &str,
    ) -> (
        soac_blockpy::block_py::BlockPyModule<CodegenModuleShape>,
        usize,
    ) {
        let lowered = lower_python_to_blockpy_for_testing(source)
            .expect("transform should succeed")
            .codegen_module;
        let function_index = lowered
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing lowered function {qualname}"));
        (lowered, function_index)
    }

    fn binding_for_name<'a>(block_plan: &'a BlockLocalPlan, name: &str) -> &'a PlannedLocalBinding {
        block_plan
            .entry_locals
            .iter()
            .find(|binding| binding.name == name)
            .unwrap_or_else(|| panic!("missing planned local binding {name}"))
    }

    #[test]
    fn local_plan_marks_immortal_entry_locals_from_value_facts() {
        let (lowered, function_index) = lowered_function(
            r#"
def f(flag):
    x = None
    if flag:
        return x
    return x
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let if_term = function
            .blocks
            .iter()
            .filter_map(|block| match &block.term {
                BlockTerm::IfTerm(if_term) => Some(if_term),
                _ => None,
            })
            .last()
            .expect("expected lowered conditional branch");
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_locals(function, &facts);

        for label in [if_term.then_label, if_term.else_label] {
            let block_plan = plan.block(label).expect("missing block local plan");
            let x = binding_for_name(block_plan, "x");
            assert_eq!(x.ref_kind, LocalRefKind::Immortal);
            assert_eq!(x.storage, PlannedLocalStorage::BlockParam);
            assert_eq!(x.binding_facts, ParamBindingFacts::DefinitelyBound);
            assert_eq!(x.provenance, ParamProvenance::ForwardedLocal(x.location));
            assert!(
                x.facts.expect("x should have entry facts").is_none(),
                "x should keep the underlying None singleton fact"
            );
        }
    }

    #[test]
    fn local_plan_treats_function_params_as_owned_without_entry_fact() {
        let (lowered, function_index) = lowered_function(
            r#"
def f(x):
    return x
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_locals(function, &facts);
        let entry_block = function.blocks.first().expect("expected an entry block");

        let block_plan = plan
            .block(entry_block.label)
            .expect("missing entry block local plan");
        let x = binding_for_name(block_plan, "x");
        assert_eq!(x.ref_kind, LocalRefKind::Owned);
        assert_eq!(x.facts, None);
        assert_eq!(x.storage, PlannedLocalStorage::BlockParam);
        assert_eq!(x.binding_facts, ParamBindingFacts::DefinitelyBound);
        assert_eq!(x.provenance, ParamProvenance::ForwardedLocal(x.location));
    }

    #[test]
    fn planned_jit_params_include_only_live_in_locals() {
        let (lowered, function_index) = lowered_function(
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
        let function = &lowered.callable_defs[function_index];
        let if_term = function
            .blocks
            .iter()
            .filter_map(|block| match &block.term {
                BlockTerm::IfTerm(if_term) => Some(if_term),
                _ => None,
            })
            .last()
            .expect("expected lowered conditional branch");
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_locals(function, &facts);

        let then_block = &function.blocks[if_term.then_label.index()];
        let then_params =
            planned_jit_param_names_for_block(then_block, plan.block(if_term.then_label));
        assert!(then_params.iter().any(|name| name == "x"));
        assert!(!then_params.iter().any(|name| name == "y"));

        let else_block = &function.blocks[if_term.else_label.index()];
        let else_params =
            planned_jit_param_names_for_block(else_block, plan.block(if_term.else_label));
        assert!(else_params.iter().any(|name| name == "y"));
        assert!(!else_params.iter().any(|name| name == "x"));
    }

    #[test]
    fn local_plan_carries_maybe_unbound_live_ins_as_block_params() {
        let (lowered, function_index) = lowered_function(
            r#"
def f(flag):
    if flag:
        x = 1
    return x
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_locals(function, &facts);
        let entry_label = function.entry_block().label;

        let non_entry_x_bindings = function
            .blocks
            .iter()
            .filter(|block| block.label != entry_label)
            .filter_map(|block| {
                let runtime_param_names =
                    planned_jit_param_names_for_block(block, plan.block(block.label));
                if !runtime_param_names.iter().any(|name| name == "x") {
                    return None;
                }
                plan.block(block.label).and_then(|block_plan| {
                    block_plan
                        .entry_locals
                        .iter()
                        .find(|binding| binding.name == "x")
                })
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
                .all(|binding| binding.binding_facts == ParamBindingFacts::MaybeUnbound),
            "maybe-unbound live-ins should preserve checked local-load semantics: {non_entry_x_bindings:?}"
        );
        assert!(
            non_entry_x_bindings
                .iter()
                .all(|binding| binding.provenance
                    == ParamProvenance::ForwardedLocal(binding.location)),
            "maybe-unbound live-ins should preserve forwarded-local provenance: {non_entry_x_bindings:?}"
        );
    }

    #[test]
    fn refcount_plan_is_available_to_jit_planning() {
        let (lowered, function_index) = lowered_function(
            r#"
def f():
    x = []
    return None
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_refcount_ownership(&lowered, function, &facts)
            .expect("JIT planning should accept the verified BlockPy refcount plan");

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
    fn refcount_plan_check_maps_terminal_releases_to_stack_slot_cleanup() {
        let (lowered, function_index) = lowered_function(
            r#"
def f():
    x = []
    return None
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_refcount_ownership(&lowered, function, &facts)
            .expect("refcount plan should validate");
        let check = check_refcount_plan_against_current_jit(function, &plan)
            .expect("current JIT should account for storage-layout locals");

        assert_eq!(check.local_rebinds, 1);
        assert_eq!(check.terminal_stack_slot_releases, 1);
        assert_eq!(check.normal_edge_stack_slot_releases, 0);
        assert_eq!(check.exception_edge_stack_slot_releases, 0);
        assert_eq!(check.normal_edge_release_gaps, 0);
        assert_eq!(check.exception_edge_release_gaps, 0);
        assert!(!check.has_edge_release_gaps());
    }

    #[test]
    fn refcount_plan_check_maps_normal_edge_releases_to_stack_slot_cleanup() {
        let (lowered, function_index) = lowered_function(
            r#"
def f(flag):
    x = []
    if flag:
        return None
    return None
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_refcount_ownership(&lowered, function, &facts)
            .expect("refcount plan should validate");
        let check = check_refcount_plan_against_current_jit(function, &plan)
            .expect("current JIT should account for storage-layout locals");

        assert!(
            check.normal_edge_stack_slot_releases > 0,
            "expected the plan to expose normal-edge stack-slot releases: {check:#?}"
        );
        assert_eq!(
            check.normal_edge_release_gaps, 0,
            "normal edges are now consumed by planned stack-slot releases"
        );
        assert_eq!(check.exception_edge_release_gaps, 0);
        assert!(!check.has_edge_release_gaps());
    }

    #[test]
    fn refcount_plan_check_maps_exception_edge_releases_to_stack_slot_cleanup() {
        let (lowered, function_index) = lowered_function(
            r#"
def f():
    try:
        x = []
        raise ValueError("boom")
    except ValueError:
        return None
    return x
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_refcount_ownership(&lowered, function, &facts)
            .expect("refcount plan should validate");
        let check = check_refcount_plan_against_current_jit(function, &plan)
            .expect("current JIT should account for storage-layout locals");

        assert!(
            check.exception_edge_stack_slot_releases > 0,
            "expected the plan to expose exception-edge stack-slot releases: {check:#?}"
        );
        assert_eq!(
            check.exception_edge_release_gaps, 0,
            "exception edges are now consumed by planned stack-slot releases"
        );
        assert_eq!(check.normal_edge_release_gaps, 0);
        assert!(!check.has_edge_release_gaps());
    }
}
