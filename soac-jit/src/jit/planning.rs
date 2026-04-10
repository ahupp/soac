use soac_blockpy::block_py::{
    BlockArg, BlockLabel, BlockPyFunction, BlockPyModule, CodegenBlock, LocalLocation,
};
use soac_blockpy::passes::{
    CodegenModuleShape, FactStore, FunctionRefcountPlan, PyObjFacts, RefcountActionKind,
    RefcountReleaseReason, lower_refcount_ownership, validate_refcount_plan,
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedLocalBinding {
    pub name: String,
    pub location: LocalLocation,
    pub ref_kind: LocalRefKind,
    pub facts: Option<PyObjFacts>,
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
    let mut blocks = HashMap::with_capacity(function.blocks.len());
    for block in &function.blocks {
        let entry_facts = facts.block_entry_fact(function.function_id, block.label);
        let entry_locals = storage_layout
            .stack_slots()
            .iter()
            .enumerate()
            .map(|(slot, name)| {
                let location = LocalLocation(
                    u32::try_from(slot).expect("storage layout slot index should fit in u32"),
                );
                let py_facts = entry_facts.and_then(|env| env.local_pyobj_fact(location));
                PlannedLocalBinding {
                    name: name.clone(),
                    location,
                    ref_kind: local_ref_kind_for_facts(py_facts),
                    facts: py_facts,
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
    let plan = lower_refcount_ownership(module, facts);
    validate_refcount_plan(module, facts, &plan)?;
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
}

impl CurrentJitRefcountPlanCheck {
    pub fn has_edge_release_gaps(&self) -> bool {
        self.normal_edge_stack_slot_releases > 0 || self.exception_edge_stack_slot_releases > 0
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

fn local_ref_kind_for_facts(facts: Option<PyObjFacts>) -> LocalRefKind {
    match facts {
        Some(facts) if facts.is_immortal() => LocalRefKind::Immortal,
        _ => LocalRefKind::Unknown,
    }
}

#[derive(Clone, Debug)]
pub struct BlockExcDispatchPlan {
    pub target_index: usize,
    pub slot_writes: Vec<(String, BlockArg)>,
}

pub fn jit_param_names_for_block(block: &CodegenBlock) -> Vec<String> {
    block.bb_param_names().map(ToString::to_string).collect()
}

pub fn exc_dispatch_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &CodegenBlock,
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
    let runtime_param_name_set = jit_param_names_for_block(target_block)
        .into_iter()
        .collect::<HashSet<_>>();
    let full_target_param_names = target_block.param_name_vec();
    let mut slot_writes = Vec::new();
    for (target_param_name, source) in full_target_param_names.iter().zip(exc_edge.args.iter()) {
        if runtime_param_name_set.contains(target_param_name)
            || !stack_slot_name_set.contains(target_param_name)
        {
            continue;
        }
        slot_writes.push((target_param_name.clone(), source.clone()));
    }
    Some(BlockExcDispatchPlan {
        target_index,
        slot_writes,
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
            assert!(
                x.facts.expect("x should have entry facts").is_none(),
                "x should keep the underlying None singleton fact"
            );
        }
    }

    #[test]
    fn local_plan_uses_unknown_when_entry_fact_is_absent() {
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
        assert_eq!(x.ref_kind, LocalRefKind::Unknown);
        assert_eq!(x.facts, None);
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
        assert!(!check.has_edge_release_gaps());
    }

    #[test]
    fn refcount_plan_check_reports_normal_edge_release_gaps() {
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
            "expected the plan to expose stack-slot releases that current normal-edge code does not emit: {check:#?}"
        );
        assert!(check.has_edge_release_gaps());
    }
}
