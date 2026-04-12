use soac_blockpy::block_py::{
    BlockArg, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, CodegenBlock, FunctionId,
    LocalLocation,
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
    SyntheticUnbound(LocalLocation),
    StackSlot(LocalLocation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockParamFacts {
    pub value: Option<PyObjFacts>,
    pub binding: ParamBindingFacts,
    pub provenance: ParamProvenance,
    pub ownership: LocalRefKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedLocalBinding {
    pub name: String,
    pub location: LocalLocation,
    pub storage: PlannedLocalStorage,
    pub param_facts: BlockParamFacts,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BlockLocalPlan {
    pub label: BlockLabel,
    pub entry_locals: Vec<PlannedLocalBinding>,
}

fn is_try_exception_alias_name(name: &str) -> bool {
    name.starts_with("_dp_try_exc_")
}

fn can_release_via_stack_slot_fallback(name: &str) -> bool {
    name.starts_with("_dp_try_exc_")
        || name.starts_with("_dp_try_abrupt_kind_")
        || name.starts_with("_dp_try_abrupt_payload_")
}

impl BlockLocalPlan {
    pub fn binding_for_name(&self, name: &str) -> Option<&PlannedLocalBinding> {
        self.entry_locals
            .iter()
            .find(|binding| binding.name == name)
    }

    pub fn binding_for_block_arg_name(&self, name: &str) -> Option<&PlannedLocalBinding> {
        self.binding_for_name(name).or_else(|| {
            if !is_try_exception_alias_name(name) {
                return None;
            }
            self.entry_locals
                .iter()
                .find(|binding| is_try_exception_alias_name(binding.name.as_str()))
        })
    }
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
        let is_entry_block = block.label == entry_label;
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
                let is_function_param_on_entry = is_entry_block
                    && function
                        .params
                        .iter()
                        .any(|param| param.name == name.as_str());
                let is_must_bound_on_entry = must_bound_locations.contains(&location);
                let py_facts = entry_facts
                    .and_then(|env| env.local_pyobj_fact(location))
                    .filter(|_| is_must_bound_on_entry);
                let is_live_in = live_in_locations.contains(&location);
                let storage = if explicit_param_names.contains(name.as_str()) {
                    PlannedLocalStorage::BlockParam
                } else if is_function_param_on_entry {
                    PlannedLocalStorage::BlockParam
                } else if is_live_in || is_must_bound_on_entry {
                    PlannedLocalStorage::BlockParam
                } else {
                    PlannedLocalStorage::StackSlot
                };
                let binding_facts =
                    if explicit_param_names.contains(name.as_str()) || is_function_param_on_entry {
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
                } else if is_function_param_on_entry {
                    ParamProvenance::ForwardedLocal(location)
                } else if is_entry_block && storage == PlannedLocalStorage::BlockParam {
                    ParamProvenance::SyntheticUnbound(location)
                } else if storage == PlannedLocalStorage::BlockParam {
                    ParamProvenance::ForwardedLocal(location)
                } else {
                    ParamProvenance::StackSlot(location)
                };
                PlannedLocalBinding {
                    name: name.clone(),
                    location,
                    storage,
                    param_facts: BlockParamFacts {
                        value: py_facts,
                        binding: binding_facts,
                        provenance,
                        ownership: local_ref_kind_for_block_entry(
                            function,
                            is_entry_block,
                            name,
                            explicit_param_names.contains(name.as_str())
                                || is_function_param_on_entry,
                            is_must_bound_on_entry,
                            py_facts,
                        ),
                    },
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
    pub terminal_local_releases: usize,
    pub normal_edge_local_releases: usize,
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
    let storage_layout_local_names = function
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
                    check_local_has_storage_layout_entry(
                        function,
                        &storage_layout_local_names,
                        &local.name,
                        &mut errors,
                    );
                    check.local_rebinds += 1;
                }
                RefcountActionKind::DeleteLocal { local, .. } => {
                    check_local_has_storage_layout_entry(
                        function,
                        &storage_layout_local_names,
                        &local.name,
                        &mut errors,
                    );
                    check.local_deletes += 1;
                }
                RefcountActionKind::ReleaseLocal { local, reason, .. } => {
                    check_local_has_storage_layout_entry(
                        function,
                        &storage_layout_local_names,
                        &local.name,
                        &mut errors,
                    );
                    match reason {
                        RefcountReleaseReason::Return | RefcountReleaseReason::Raise => {
                            check.terminal_local_releases += 1;
                        }
                        RefcountReleaseReason::Jump { .. }
                        | RefcountReleaseReason::IfThen { .. }
                        | RefcountReleaseReason::IfElse { .. }
                        | RefcountReleaseReason::BranchCase { .. }
                        | RefcountReleaseReason::BranchDefault { .. } => {
                            check.normal_edge_local_releases += 1;
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

fn check_local_has_storage_layout_entry(
    function: &BlockPyFunction<CodegenModuleShape>,
    storage_layout_local_names: &HashSet<String>,
    local_name: &str,
    errors: &mut Vec<String>,
) {
    if storage_layout_local_names.contains(local_name) {
        return;
    }
    errors.push(format!(
        "refcount plan action for function {} ({}) references local {local_name:?}, \
         but the current JIT cleanup model only tracks storage-layout locals",
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
    pub release_local_names: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct EdgeTransportPlan {
    pub slot_writes: Vec<(String, BlockArg)>,
    pub target_args: Vec<(String, BlockArg)>,
    pub forwarded_local_names: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeBlockParamPlan {
    pub arg_name: String,
    pub binding: PlannedLocalBinding,
    pub entry_aliases: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedStackSlotEntrySeed {
    pub binding: PlannedLocalBinding,
    pub entry_ref_kind: LocalRefKind,
}

#[derive(Clone, Debug)]
pub struct PlannedJitFunctionLocals {
    pub local_plan: FunctionLocalPlan,
    pub refcount_plan: FunctionRefcountPlan,
    pub runtime_block_params: Vec<Vec<RuntimeBlockParamPlan>>,
    pub implicit_target_transports: Vec<EdgeTransportPlan>,
    pub jump_edge_transports: Vec<Option<EdgeTransportPlan>>,
    pub stack_slot_entry_seeds: Vec<Vec<PlannedStackSlotEntrySeed>>,
    pub exc_dispatches: Vec<Option<BlockExcDispatchPlan>>,
}

#[derive(Clone, Debug, Default)]
pub struct PlannedJitModuleLocals {
    pub functions: HashMap<FunctionId, PlannedJitFunctionLocals>,
}

impl PlannedJitModuleLocals {
    pub fn function(&self, function_id: FunctionId) -> Option<&PlannedJitFunctionLocals> {
        self.functions.get(&function_id)
    }
}

impl PlannedJitFunctionLocals {
    pub fn validate_for_function(
        &self,
        function: &BlockPyFunction<CodegenModuleShape>,
    ) -> Result<(), String> {
        let block_count = function.blocks.len();
        if self.runtime_block_params.len() != block_count
            || self.implicit_target_transports.len() != block_count
            || self.jump_edge_transports.len() != block_count
            || self.stack_slot_entry_seeds.len() != block_count
            || self.exc_dispatches.len() != block_count
        {
            return Err(format!(
                "planned JIT local state for function {} ({}) has inconsistent block counts",
                function.function_id, function.names.qualname
            ));
        }

        for (index, block) in function.blocks.iter().enumerate() {
            let block_plan = self.local_plan.block(block.label);
            if block_plan.is_none()
                && (!self.runtime_block_params[index].is_empty()
                    || !self.stack_slot_entry_seeds[index].is_empty())
            {
                return Err(format!(
                    "planned JIT local state for function {} ({}) is missing block {}",
                    function.function_id, function.names.qualname, block.label
                ));
            }
            if let Some(block_plan) = block_plan {
                for param in &self.runtime_block_params[index] {
                    if block_plan
                        .binding_for_block_arg_name(param.arg_name.as_str())
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
                if dispatch.target_index != exc_edge.target.index() {
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
            }
        }

        Ok(())
    }
}

pub fn plan_jit_module_locals(
    module: &BlockPyModule<CodegenModuleShape>,
    facts: &FactStore,
) -> Result<PlannedJitModuleLocals, String> {
    let mut functions = HashMap::with_capacity(module.callable_defs.len());
    for function in &module.callable_defs {
        let function_plan = plan_jit_function_locals(module, function, facts)?;
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

pub fn local_ref_kind_for_stack_mirror(ref_kind: LocalRefKind) -> LocalRefKind {
    match ref_kind {
        LocalRefKind::Immortal => LocalRefKind::Immortal,
        LocalRefKind::Unbound => LocalRefKind::Unbound,
        LocalRefKind::Owned | LocalRefKind::Borrowed | LocalRefKind::Unknown => {
            LocalRefKind::Borrowed
        }
    }
}

pub fn planned_jit_params_for_function(
    function: &BlockPyFunction<CodegenModuleShape>,
    local_plan: &FunctionLocalPlan,
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
                seen_names.insert(arg_name.clone());
                let Some(binding) = block_plan
                    .and_then(|plan| plan.binding_for_block_arg_name(arg_name.as_str()).cloned())
                else {
                    return Err(format!(
                        "missing runtime block-param binding for function {} ({}) block {} arg {:?}",
                        function.function_id, function.names.qualname, block.label, arg_name
                    ));
                };
                let entry_aliases = if arg_name == binding.name {
                    Vec::new()
                } else {
                    vec![arg_name.clone()]
                };
                params.push(RuntimeBlockParamPlan {
                    arg_name,
                    entry_aliases,
                    binding,
                });
            }
            if let Some(block_plan) = block_plan {
                for binding in &block_plan.entry_locals {
                    if binding.storage != PlannedLocalStorage::BlockParam {
                        continue;
                    }
                    if !seen_names.insert(binding.name.clone()) {
                        continue;
                    }
                    params.push(RuntimeBlockParamPlan {
                        arg_name: binding.name.clone(),
                        binding: binding.clone(),
                        entry_aliases: Vec::new(),
                    });
                }
            }
            Ok(params)
        })
        .collect()
}

pub fn planned_stack_slot_entry_seeds_for_function(
    function: &BlockPyFunction<CodegenModuleShape>,
    local_plan: &FunctionLocalPlan,
) -> Vec<Vec<PlannedStackSlotEntrySeed>> {
    let live_ins = compute_function_local_live_ins(function);
    let must_bound_ins = compute_function_local_must_bound_ins(function);
    function
        .blocks
        .iter()
        .map(|block| {
            let live_in_locations = live_ins.get(&block.label).cloned().unwrap_or_default();
            let must_bound_locations = must_bound_ins
                .get(&block.label)
                .cloned()
                .unwrap_or_default();
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
                            if !live_in_locations.contains(&binding.location)
                                && !must_bound_locations.contains(&binding.location)
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
            (
                name.clone(),
                explicit_args_by_name
                    .get(name.as_str())
                    .map(|arg| (*arg).clone())
                    .unwrap_or_else(|| BlockArg::Name(name.clone())),
            )
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
    for (_, arg) in target_args.iter() {
        record_forwarded_name(arg);
    }
    EdgeTransportPlan {
        slot_writes,
        target_args,
        forwarded_local_names,
    }
}

pub fn planned_implicit_target_transports_for_function(
    function: &BlockPyFunction<CodegenModuleShape>,
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

pub fn planned_jump_edge_transports_for_function(
    function: &BlockPyFunction<CodegenModuleShape>,
    runtime_block_params: &[Vec<RuntimeBlockParamPlan>],
) -> Vec<Option<EdgeTransportPlan>> {
    let no_slot_writes = HashSet::new();
    function
        .blocks
        .iter()
        .map(|block| match &block.term {
            BlockTerm::Jump(target) => {
                let target_index = target.target.index();
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

pub fn exc_dispatch_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
    block: &CodegenBlock,
    runtime_target_params: &[RuntimeBlockParamPlan],
    refcount_plan: &FunctionRefcountPlan,
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
    let full_target_param_names = target_block.param_name_vec();
    let transport = plan_edge_transport(
        &full_target_param_names,
        &exc_edge.args,
        runtime_target_params,
        &stack_slot_name_set,
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
                || can_release_via_stack_slot_fallback(local.name.as_str())
                || forwarded_local_names.iter().any(|name| name == &local.name)
            {
                continue;
            }
            forwarded_local_names.push(local.name.clone());
            release_local_names.push(local.name.clone());
        }
    }
    Some(BlockExcDispatchPlan {
        target_index,
        slot_writes: transport.slot_writes,
        target_args: transport.target_args,
        forwarded_local_names,
        release_local_names,
    })
}

pub fn plan_jit_function_locals(
    module: &BlockPyModule<CodegenModuleShape>,
    function: &BlockPyFunction<CodegenModuleShape>,
    facts: &FactStore,
) -> Result<PlannedJitFunctionLocals, String> {
    let local_plan = plan_function_locals(function, facts);
    let refcount_plan = plan_function_refcount_ownership(module, function, facts)?;
    let _refcount_plan_check = check_refcount_plan_against_current_jit(function, &refcount_plan)?;
    let runtime_block_params = planned_jit_params_for_function(function, &local_plan)?;
    let implicit_target_transports =
        planned_implicit_target_transports_for_function(function, &runtime_block_params);
    let jump_edge_transports =
        planned_jump_edge_transports_for_function(function, &runtime_block_params);
    let stack_slot_entry_seeds = planned_stack_slot_entry_seeds_for_function(function, &local_plan);
    let exc_dispatches = function
        .blocks
        .iter()
        .map(|block| {
            let runtime_target_params = block
                .exc_edge
                .as_ref()
                .map(|edge| runtime_block_params[edge.target.index()].as_slice())
                .unwrap_or(&[]);
            exc_dispatch_plan(function, block, runtime_target_params, &refcount_plan)
        })
        .collect::<Vec<_>>();

    let plan = PlannedJitFunctionLocals {
        local_plan,
        refcount_plan,
        runtime_block_params,
        implicit_target_transports,
        jump_edge_transports,
        stack_slot_entry_seeds,
        exc_dispatches,
    };
    plan.validate_for_function(function)?;
    Ok(plan)
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
        assert_eq!(x.param_facts.ownership, LocalRefKind::Owned);
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
        let runtime_params =
            planned_jit_params_for_function(function, &plan).expect("runtime params should bind");

        let then_params = &runtime_params[if_term.then_label.index()];
        assert!(then_params.iter().any(|param| param.arg_name == "x"));
        assert!(then_params.iter().any(|param| param.arg_name == "y"));

        let else_params = &runtime_params[if_term.else_label.index()];
        assert!(else_params.iter().any(|param| param.arg_name == "y"));
        assert!(else_params.iter().any(|param| param.arg_name == "x"));
    }

    #[test]
    fn planned_jit_params_keep_binding_metadata_for_forwarded_locals() {
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
        let runtime_params =
            planned_jit_params_for_function(function, &plan).expect("runtime params should bind");
        let then_params = &runtime_params[if_term.then_label.index()];
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
    fn planned_jit_params_for_function_validate_handler_exception_carriers() {
        let (lowered, function_index) = lowered_function(
            r#"
def f():
    try:
        raise ValueError("boom")
    except ValueError:
        return 1
"#,
            "f",
        );
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_function_locals(function, &facts);

        let runtime_params =
            planned_jit_params_for_function(function, &plan).expect("runtime params should bind");
        let handler_params = runtime_params
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                function.blocks[*index]
                    .param_names()
                    .any(|name| name.starts_with("_dp_try_exc_"))
            })
            .flat_map(|(_, params)| params.iter())
            .collect::<Vec<_>>();

        assert!(
            !handler_params.is_empty(),
            "expected at least one handler runtime param set"
        );
        assert!(
            handler_params
                .iter()
                .all(|param| param.binding.name.starts_with("_dp_try_exc_")
                    || !param.arg_name.starts_with("_dp_try_exc_")),
            "validated handler runtime params should preserve bound exception-carrier names: {handler_params:#?}"
        );
    }

    #[test]
    fn must_bound_cleanup_locals_travel_as_block_params() {
        let (lowered, function_index) = lowered_function(
            r#"
def f(flag):
    x = 1
    if flag:
        return 1
    return 0
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
        let runtime_params =
            planned_jit_params_for_function(function, &plan).expect("runtime params should bind");
        let seeds = planned_stack_slot_entry_seeds_for_function(function, &plan);

        let else_params = &runtime_params[if_term.else_label.index()];
        let x = else_params
            .iter()
            .find(|param| param.binding.name == "x")
            .expect("expected runtime cleanup param for x");
        assert_eq!(x.binding.storage, PlannedLocalStorage::BlockParam);
        assert_eq!(
            x.binding.param_facts.binding,
            ParamBindingFacts::DefinitelyBound
        );
        assert_eq!(
            x.binding.param_facts.provenance,
            ParamProvenance::ForwardedLocal(x.binding.location)
        );
        assert!(
            seeds[if_term.else_label.index()]
                .iter()
                .all(|seed| seed.binding.name != "x"),
            "cleanup-only locals should not require stack-slot entry seeds"
        );
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
        let runtime_params =
            planned_jit_params_for_function(function, &plan).expect("runtime params should bind");

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
        let runtime_params =
            planned_jit_params_for_function(function, &plan).expect("runtime params should bind");
        let seeds = planned_stack_slot_entry_seeds_for_function(function, &plan);
        let entry_label = function.entry_block().label;
        let entry_plan = plan.block(entry_label).expect("missing entry local plan");
        let entry_x = binding_for_name(entry_plan, "x");

        assert_eq!(entry_x.storage, PlannedLocalStorage::BlockParam);
        assert_eq!(entry_x.param_facts.binding, ParamBindingFacts::MaybeUnbound);
        assert_eq!(entry_x.param_facts.ownership, LocalRefKind::Unbound);
        assert_eq!(
            entry_x.param_facts.provenance,
            ParamProvenance::SyntheticUnbound(entry_x.location)
        );
        assert!(
            runtime_params[entry_label.index()]
                .iter()
                .any(|param| param.arg_name == "x"),
            "entry maybe-unbound local should be initialized as a runtime block param"
        );
        assert!(
            seeds[entry_label.index()]
                .iter()
                .all(|seed| seed.binding.name != "x"),
            "entry maybe-unbound local should not require a stack-slot seed"
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
    fn planned_jit_function_locals_collects_codegen_local_state() {
        let (lowered, function_index) = lowered_function(
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
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_jit_function_locals(&lowered, function, &facts)
            .expect("JIT local state should plan before codegen");

        assert_eq!(plan.local_plan.blocks.len(), function.blocks.len());
        assert_eq!(plan.runtime_block_params.len(), function.blocks.len());
        assert_eq!(plan.implicit_target_transports.len(), function.blocks.len());
        assert_eq!(plan.jump_edge_transports.len(), function.blocks.len());
        assert_eq!(plan.stack_slot_entry_seeds.len(), function.blocks.len());
        assert_eq!(plan.exc_dispatches.len(), function.blocks.len());
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
    }

    #[test]
    fn planned_jit_module_locals_collects_all_functions() {
        let lowered = soac_blockpy::lower_python_to_blockpy_for_testing(
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
        .codegen_module;
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_jit_module_locals(&lowered, &facts)
            .expect("JIT module local state should plan before codegen");

        assert_eq!(plan.functions.len(), lowered.callable_defs.len());
        for function in &lowered.callable_defs {
            let function_plan = plan
                .function(function.function_id)
                .unwrap_or_else(|| panic!("missing plan for {}", function.names.qualname));
            function_plan
                .validate_for_function(function)
                .expect("module-level function plan should validate");
        }
    }

    #[test]
    fn exc_dispatch_plan_for_handler_preserves_forwarded_live_in_local() {
        let (lowered, function_index) = lowered_function(
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
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let local_plan = plan_function_locals(function, &facts);
        let runtime_params = planned_jit_params_for_function(function, &local_plan)
            .expect("runtime params should bind");
        let source_block = function
            .blocks
            .iter()
            .find(|block| block.exc_edge.is_some())
            .expect("expected exception edge source block");
        let exc_edge = source_block.exc_edge.as_ref().expect("checked above");
        let runtime_target_params = &runtime_params[exc_edge.target.index()];
        let dispatch_plan = exc_dispatch_plan(
            function,
            source_block,
            &runtime_target_params,
            &FunctionRefcountPlan::default(),
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
    fn exc_dispatch_plan_carries_ordinary_exception_cleanup_locals_as_target_args() {
        let (lowered, function_index) = lowered_function(
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
        let function = &lowered.callable_defs[function_index];
        let facts = infer_module_value_facts(&lowered);
        let local_plan = plan_function_locals(function, &facts);
        let refcount_plan = plan_function_refcount_ownership(&lowered, function, &facts)
            .expect("refcount plan should validate");
        let runtime_params = planned_jit_params_for_function(function, &local_plan)
            .expect("runtime params should bind");

        let dispatches = function
            .blocks
            .iter()
            .filter_map(|block| {
                let runtime_target_params = block
                    .exc_edge
                    .as_ref()
                    .map(|edge| runtime_params[edge.target.index()].as_slice())
                    .unwrap_or(&[]);
                exc_dispatch_plan(function, block, runtime_target_params, &refcount_plan)
            })
            .collect::<Vec<_>>();

        assert!(
            dispatches
                .iter()
                .any(|dispatch| dispatch.release_local_names.is_empty()
                    && dispatch
                        .forwarded_local_names
                        .iter()
                        .any(|name| name == "x")
                    && dispatch.target_args.iter().any(|(name, _)| name == "x")),
            "exception dispatch should carry ordinary cleanup locals as target args: {dispatches:#?}"
        );
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
        assert_eq!(transport.target_args[0].0, "x");
        match &transport.target_args[0].1 {
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
        }];
        let transport = plan_edge_transport(
            &["x".to_string()],
            &[],
            &runtime_target_params,
            &HashSet::new(),
        );

        assert!(transport.slot_writes.is_empty());
        assert_eq!(transport.target_args.len(), 1);
        assert_eq!(transport.target_args[0].0, "x");
        match &transport.target_args[0].1 {
            BlockArg::Name(name) => assert_eq!(name, "x"),
            other => panic!("expected implicit forwarded name, got {other:?}"),
        }
        assert_eq!(transport.forwarded_local_names, vec!["x".to_string()]);
    }

    #[test]
    fn refcount_plan_check_maps_terminal_releases_to_local_env_cleanup() {
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
        assert_eq!(check.terminal_local_releases, 1);
        assert_eq!(check.normal_edge_local_releases, 0);
        assert_eq!(check.exception_edge_stack_slot_releases, 0);
        assert_eq!(check.normal_edge_release_gaps, 0);
        assert_eq!(check.exception_edge_release_gaps, 0);
        assert!(!check.has_edge_release_gaps());
    }

    #[test]
    fn refcount_plan_check_maps_normal_edge_releases_to_local_env_cleanup() {
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
            check.normal_edge_local_releases > 0,
            "expected the plan to expose normal-edge LocalEnv releases: {check:#?}"
        );
        assert_eq!(
            check.normal_edge_release_gaps, 0,
            "normal edges are now consumed by planned LocalEnv releases"
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
            "exception edges are still consumed by planned stack-slot releases"
        );
        assert_eq!(check.normal_edge_release_gaps, 0);
        assert!(!check.has_edge_release_gaps());
    }
}
