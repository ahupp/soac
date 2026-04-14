//! Block-entry local environment planning.
//!
//! This pass records the local values that must be available when each
//! Codegen BlockPy block starts. It is intentionally representation-facing but
//! backend-neutral: backends may materialize these entries as SSA block params,
//! stack-slot loads, or another resume-state representation.

use crate::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, FunctionId, HasSemanticInstrId, InstrCodegen,
    InstrKey, InstrResolved, LocalLocation, NameLike,
};
use crate::passes::ownership_effects::{
    compute_function_local_live_ins, compute_function_local_must_bound_ins,
};
use crate::passes::{CodegenModuleShape, FactStore, PyObjFacts};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalEnvModulePlan {
    pub functions: HashMap<FunctionId, FunctionLocalPlan>,
}

impl LocalEnvModulePlan {
    pub fn function(&self, function_id: FunctionId) -> Option<&FunctionLocalPlan> {
        self.functions.get(&function_id)
    }

    pub fn validate_for_module(
        &self,
        module: &BlockPyModule<CodegenModuleShape>,
        facts: &FactStore,
    ) -> Result<(), String> {
        validate_local_env_module_plan(module, facts, self)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalEnvResumeStatePrecision {
    /// The resume record contains the LocalEnv entries that are valid at the
    /// containing block's entry. This is sufficient for block-entry resume
    /// validation and rendering, but not yet a runnable mid-block deopt state.
    BlockEntry,
    /// The resume record has applied direct top-level local Store/Del effects
    /// within the block up to the instruction boundary.
    InstructionBoundary,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalEnvResumePoint {
    BlockEntry {
        function_id: FunctionId,
        block: BlockLabel,
    },
    BeforeInstr {
        key: InstrKey,
    },
    BeforeTerm {
        function_id: FunctionId,
        block: BlockLabel,
    },
}

impl LocalEnvResumePoint {
    pub const fn function_id(self) -> FunctionId {
        match self {
            Self::BlockEntry { function_id, .. } | Self::BeforeTerm { function_id, .. } => {
                function_id
            }
            Self::BeforeInstr { key } => key.function_id,
        }
    }

    pub const fn block_label(self) -> BlockLabel {
        match self {
            Self::BlockEntry { block, .. } | Self::BeforeTerm { block, .. } => block,
            Self::BeforeInstr { key } => key.instr_id.block_label(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalEnvResumeEntry {
    pub point: LocalEnvResumePoint,
    pub precision: LocalEnvResumeStatePrecision,
    pub locals: Vec<LocalEnvResumeBinding>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalEnvResumeBindingState {
    Bound,
    MaybeUnbound,
    Unbound,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LocalEnvResumeValueSource {
    BlockParam(LocalLocation),
    StackSlot(LocalLocation),
    StoredValue(InstrKey),
    Unbound,
    Unknown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalEnvResumeBinding {
    pub name: String,
    pub location: LocalLocation,
    pub binding: LocalEnvResumeBindingState,
    pub source: LocalEnvResumeValueSource,
    pub ownership: LocalRefKind,
    pub value: Option<PyObjFacts>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FunctionLocalEnvResumePlan {
    pub entries: Vec<LocalEnvResumeEntry>,
}

impl FunctionLocalEnvResumePlan {
    pub fn entries_for_block(
        &self,
        block: BlockLabel,
    ) -> impl Iterator<Item = &LocalEnvResumeEntry> {
        self.entries
            .iter()
            .filter(move |entry| entry.point.block_label() == block)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalEnvResumeModulePlan {
    pub functions: HashMap<FunctionId, FunctionLocalEnvResumePlan>,
}

impl LocalEnvResumeModulePlan {
    pub fn function(&self, function_id: FunctionId) -> Option<&FunctionLocalEnvResumePlan> {
        self.functions.get(&function_id)
    }

    pub fn validate_for_module(
        &self,
        module: &BlockPyModule<CodegenModuleShape>,
        local_env_plan: &LocalEnvModulePlan,
        facts: &FactStore,
    ) -> Result<(), String> {
        validate_local_env_resume_module_plan(module, local_env_plan, facts, self)
    }
}

pub fn plan_local_env_module(
    module: &BlockPyModule<CodegenModuleShape>,
    facts: &FactStore,
) -> LocalEnvModulePlan {
    let functions = module
        .callable_defs
        .iter()
        .map(|function| (function.function_id, plan_function_locals(function, facts)))
        .collect();
    LocalEnvModulePlan { functions }
}

pub fn plan_local_env_resume_module(
    module: &BlockPyModule<CodegenModuleShape>,
    local_env_plan: &LocalEnvModulePlan,
    facts: &FactStore,
) -> LocalEnvResumeModulePlan {
    let deleted_sentinel_constants = deleted_sentinel_constant_slots(module);
    let functions = module
        .callable_defs
        .iter()
        .filter_map(|function| {
            let function_plan = local_env_plan.function(function.function_id)?;
            Some((
                function.function_id,
                plan_function_local_env_resume_with_deleted_constants(
                    function,
                    function_plan,
                    facts,
                    &deleted_sentinel_constants,
                ),
            ))
        })
        .collect();
    LocalEnvResumeModulePlan { functions }
}

pub fn plan_function_local_env_resume(
    function: &BlockPyFunction<CodegenModuleShape>,
    local_env_plan: &FunctionLocalPlan,
    facts: &FactStore,
) -> FunctionLocalEnvResumePlan {
    plan_function_local_env_resume_with_deleted_constants(
        function,
        local_env_plan,
        facts,
        &HashSet::new(),
    )
}

fn plan_function_local_env_resume_with_deleted_constants(
    function: &BlockPyFunction<CodegenModuleShape>,
    local_env_plan: &FunctionLocalPlan,
    facts: &FactStore,
    deleted_sentinel_constants: &HashSet<u32>,
) -> FunctionLocalEnvResumePlan {
    let mut entries = Vec::new();
    for block in &function.blocks {
        let Some(block_plan) = local_env_plan.block(block.label) else {
            continue;
        };
        let mut locals = block_plan
            .entry_locals
            .iter()
            .map(resume_binding_from_planned_local)
            .collect::<Vec<_>>();
        entries.push(LocalEnvResumeEntry {
            point: LocalEnvResumePoint::BlockEntry {
                function_id: function.function_id,
                block: block.label,
            },
            precision: LocalEnvResumeStatePrecision::BlockEntry,
            locals: locals.clone(),
        });
        for instr in &block.body {
            let Some(instr_id) = instr.try_semantic_instr_id() else {
                transfer_resume_local_state(
                    function.function_id,
                    instr,
                    facts,
                    deleted_sentinel_constants,
                    &mut locals,
                );
                continue;
            };
            entries.push(LocalEnvResumeEntry {
                point: LocalEnvResumePoint::BeforeInstr {
                    key: InstrKey::new(function.function_id, instr_id),
                },
                precision: LocalEnvResumeStatePrecision::InstructionBoundary,
                locals: locals.clone(),
            });
            transfer_resume_local_state(
                function.function_id,
                instr,
                facts,
                deleted_sentinel_constants,
                &mut locals,
            );
        }
        entries.push(LocalEnvResumeEntry {
            point: LocalEnvResumePoint::BeforeTerm {
                function_id: function.function_id,
                block: block.label,
            },
            precision: LocalEnvResumeStatePrecision::InstructionBoundary,
            locals,
        });
    }
    FunctionLocalEnvResumePlan { entries }
}

pub fn validate_local_env_module_plan(
    module: &BlockPyModule<CodegenModuleShape>,
    facts: &FactStore,
    plan: &LocalEnvModulePlan,
) -> Result<(), String> {
    let expected_function_ids = module
        .callable_defs
        .iter()
        .map(|function| function.function_id)
        .collect::<HashSet<_>>();
    let mut errors = Vec::new();

    for function in &module.callable_defs {
        let Some(function_plan) = plan.function(function.function_id) else {
            errors.push(format!(
                "missing LocalEnv plan for function {} ({})",
                function.function_id, function.names.qualname
            ));
            continue;
        };
        validate_function_local_plan(function, facts, function_plan, &mut errors);
    }

    for function_id in plan.functions.keys() {
        if !expected_function_ids.contains(function_id) {
            errors.push(format!(
                "LocalEnv plan contains unknown function id {function_id}"
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

pub fn validate_local_env_resume_module_plan(
    module: &BlockPyModule<CodegenModuleShape>,
    local_env_plan: &LocalEnvModulePlan,
    facts: &FactStore,
    resume_plan: &LocalEnvResumeModulePlan,
) -> Result<(), String> {
    let expected = plan_local_env_resume_module(module, local_env_plan, facts);
    if &expected != resume_plan {
        return Err(format!(
            "LocalEnv resume plan mismatch\nexpected: {expected:#?}\nactual: {resume_plan:#?}"
        ));
    }

    let expected_function_ids = module
        .callable_defs
        .iter()
        .map(|function| function.function_id)
        .collect::<HashSet<_>>();
    let mut errors = Vec::new();
    for function in &module.callable_defs {
        if resume_plan.function(function.function_id).is_none() {
            errors.push(format!(
                "missing LocalEnv resume plan for function {} ({})",
                function.function_id, function.names.qualname
            ));
        }
    }
    for function_id in resume_plan.functions.keys() {
        if !expected_function_ids.contains(function_id) {
            errors.push(format!(
                "LocalEnv resume plan contains unknown function id {function_id}"
            ));
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
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

pub fn render_local_env_module_plan(
    module: &BlockPyModule<CodegenModuleShape>,
    facts: &FactStore,
    plan: &LocalEnvModulePlan,
) -> Result<String, String> {
    validate_local_env_module_plan(module, facts, plan)?;
    let mut out = String::new();
    for function in &module.callable_defs {
        let function_plan = plan.function(function.function_id).ok_or_else(|| {
            format!(
                "missing LocalEnv plan for function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_local_env_function_plan(function, function_plan)?);
    }
    Ok(out)
}

pub fn render_local_env_resume_module_plan(
    module: &BlockPyModule<CodegenModuleShape>,
    local_env_plan: &LocalEnvModulePlan,
    facts: &FactStore,
    resume_plan: &LocalEnvResumeModulePlan,
) -> Result<String, String> {
    validate_local_env_resume_module_plan(module, local_env_plan, facts, resume_plan)?;
    let mut out = String::new();
    for function in &module.callable_defs {
        let function_plan = resume_plan.function(function.function_id).ok_or_else(|| {
            format!(
                "missing LocalEnv resume plan for function {} ({})",
                function.function_id, function.names.qualname
            )
        })?;
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&render_local_env_resume_function_plan(
            function,
            function_plan,
        )?);
    }
    Ok(out)
}

pub fn render_local_env_resume_function_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
    plan: &FunctionLocalEnvResumePlan,
) -> Result<String, String> {
    let mut out = String::new();
    writeln!(
        out,
        "function {} {}:",
        function.function_id, function.names.qualname
    )
    .expect("writing to String should not fail");
    for block in &function.blocks {
        writeln!(out, "  block {}:", block.label).expect("writing to String should not fail");
        for entry in plan.entries_for_block(block.label) {
            writeln!(
                out,
                "    {} precision={:?}:",
                render_local_env_resume_point(entry.point),
                entry.precision,
            )
            .expect("writing to String should not fail");
            writeln!(out, "      locals:").expect("writing to String should not fail");
            for binding in &entry.locals {
                writeln!(out, "        {}", render_local_env_resume_binding(binding))
                    .expect("writing to String should not fail");
            }
        }
    }
    Ok(out)
}

pub fn render_local_env_function_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
    plan: &FunctionLocalPlan,
) -> Result<String, String> {
    let mut out = String::new();
    writeln!(
        out,
        "function {} {}:",
        function.function_id, function.names.qualname
    )
    .expect("writing to String should not fail");
    for block in &function.blocks {
        let Some(block_plan) = plan.block(block.label) else {
            return Err(format!(
                "missing LocalEnv block plan for function {} ({}) block {}",
                function.function_id, function.names.qualname, block.label
            ));
        };
        writeln!(out, "  block {}:", block.label).expect("writing to String should not fail");
        writeln!(out, "    entry_locals:").expect("writing to String should not fail");
        for binding in &block_plan.entry_locals {
            writeln!(out, "      {}", render_planned_local_binding(binding))
                .expect("writing to String should not fail");
        }
    }
    Ok(out)
}

pub fn render_planned_local_binding(binding: &PlannedLocalBinding) -> String {
    format!(
        "{}@{} storage={:?} binding={:?} ownership={:?} provenance={:?} value={:?}",
        binding.name,
        binding.location.0,
        binding.storage,
        binding.param_facts.binding,
        binding.param_facts.ownership,
        binding.param_facts.provenance,
        binding.param_facts.value
    )
}

fn render_local_env_resume_point(point: LocalEnvResumePoint) -> String {
    match point {
        LocalEnvResumePoint::BlockEntry { .. } => "block_entry".to_string(),
        LocalEnvResumePoint::BeforeInstr { key } => format!("before_instr {key}"),
        LocalEnvResumePoint::BeforeTerm { .. } => "before_term".to_string(),
    }
}

fn render_local_env_resume_binding(binding: &LocalEnvResumeBinding) -> String {
    format!(
        "{}@{} binding={:?} source={:?} ownership={:?} value={:?}",
        binding.name,
        binding.location.0,
        binding.binding,
        binding.source,
        binding.ownership,
        binding.value
    )
}

fn resume_binding_from_planned_local(binding: &PlannedLocalBinding) -> LocalEnvResumeBinding {
    let binding_state = resume_binding_state_for_planned_local(binding);
    LocalEnvResumeBinding {
        name: binding.name.clone(),
        location: binding.location,
        binding: binding_state,
        source: resume_value_source_for_planned_local(binding, binding_state),
        ownership: binding.param_facts.ownership,
        value: binding.param_facts.value,
    }
}

fn resume_binding_state_for_planned_local(
    binding: &PlannedLocalBinding,
) -> LocalEnvResumeBindingState {
    match binding.param_facts.binding {
        ParamBindingFacts::DefinitelyBound | ParamBindingFacts::CheckedLocalValue => {
            LocalEnvResumeBindingState::Bound
        }
        ParamBindingFacts::MaybeUnbound
            if binding.param_facts.ownership == LocalRefKind::Unbound =>
        {
            LocalEnvResumeBindingState::Unbound
        }
        ParamBindingFacts::MaybeUnbound => LocalEnvResumeBindingState::MaybeUnbound,
    }
}

fn resume_value_source_for_planned_local(
    binding: &PlannedLocalBinding,
    binding_state: LocalEnvResumeBindingState,
) -> LocalEnvResumeValueSource {
    if binding_state == LocalEnvResumeBindingState::Unbound {
        return LocalEnvResumeValueSource::Unbound;
    }
    match binding.param_facts.provenance {
        ParamProvenance::ExplicitBlockParam(location)
        | ParamProvenance::ForwardedLocal(location) => {
            LocalEnvResumeValueSource::BlockParam(location)
        }
        ParamProvenance::SyntheticUnbound(_) => LocalEnvResumeValueSource::Unbound,
        ParamProvenance::StackSlot(location) => LocalEnvResumeValueSource::StackSlot(location),
    }
}

fn transfer_resume_local_state(
    function_id: FunctionId,
    instr: &InstrCodegen,
    facts: &FactStore,
    deleted_sentinel_constants: &HashSet<u32>,
    locals: &mut [LocalEnvResumeBinding],
) {
    match instr {
        InstrCodegen::Store(op) => {
            let Some(location) = op.name.local_location() else {
                return;
            };
            if expr_is_deleted_sentinel(&op.value, deleted_sentinel_constants) {
                if let Some(binding) = locals
                    .iter_mut()
                    .find(|binding| binding.location == location)
                {
                    binding.binding = LocalEnvResumeBindingState::Unbound;
                    binding.source = LocalEnvResumeValueSource::Unbound;
                    binding.ownership = LocalRefKind::Unbound;
                    binding.value = None;
                }
                return;
            }
            let value_key = op
                .value
                .try_semantic_instr_id()
                .map(|instr_id| InstrKey::new(function_id, instr_id));
            let py_facts = value_key
                .and_then(|key| facts.fact_for(key))
                .and_then(|facts| facts.as_pyobj());
            if let Some(binding) = locals
                .iter_mut()
                .find(|binding| binding.location == location)
            {
                binding.binding = LocalEnvResumeBindingState::Bound;
                binding.source = value_key
                    .map(LocalEnvResumeValueSource::StoredValue)
                    .unwrap_or(LocalEnvResumeValueSource::Unknown);
                binding.ownership = local_ref_kind_for_resume_value(py_facts);
                binding.value = py_facts;
            }
        }
        InstrCodegen::Del(op) => {
            let Some(location) = op.name.local_location() else {
                return;
            };
            if let Some(binding) = locals
                .iter_mut()
                .find(|binding| binding.location == location)
            {
                binding.binding = LocalEnvResumeBindingState::Unbound;
                binding.source = LocalEnvResumeValueSource::Unbound;
                binding.ownership = LocalRefKind::Unbound;
                binding.value = None;
            }
        }
        _ => {}
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

fn local_ref_kind_for_resume_value(facts: Option<PyObjFacts>) -> LocalRefKind {
    match facts {
        Some(facts) if facts.is_immortal() => LocalRefKind::Immortal,
        Some(_) | None => LocalRefKind::Owned,
    }
}

fn validate_function_local_plan(
    function: &BlockPyFunction<CodegenModuleShape>,
    facts: &FactStore,
    plan: &FunctionLocalPlan,
    errors: &mut Vec<String>,
) {
    let expected = plan_function_locals(function, facts);
    if &expected != plan {
        errors.push(format!(
            "LocalEnv plan mismatch for function {} ({})\nexpected: {expected:#?}\nactual: {plan:#?}",
            function.function_id, function.names.qualname
        ));
    }
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

fn is_try_exception_alias_name(name: &str) -> bool {
    name.starts_with("_dp_try_exc_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower_python_to_blockpy_for_testing;
    use crate::passes::infer_module_value_facts;

    #[test]
    fn local_env_module_plan_covers_codegen_functions_and_blocks() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f(flag):
    x = None
    if flag:
        return x
    return x
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let facts = infer_module_value_facts(&lowered);
        let plan = plan_local_env_module(&lowered, &facts);

        validate_local_env_module_plan(&lowered, &facts, &plan)
            .expect("fresh LocalEnv plan should validate");

        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("lowered function should exist");
        let function_plan = plan
            .function(function.function_id)
            .expect("function should have a LocalEnv plan");
        assert_eq!(function_plan.blocks.len(), function.blocks.len());
        let has_immortal_x = function_plan.blocks.values().any(|block_plan| {
            block_plan.entry_locals.iter().any(|binding| {
                binding.name == "x"
                    && binding.storage == PlannedLocalStorage::BlockParam
                    && binding.param_facts.ownership == LocalRefKind::Immortal
            })
        });
        assert!(has_immortal_x, "LocalEnv plan should carry value facts");
    }

    #[test]
    fn local_env_resume_plan_records_validated_block_boundaries() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def choose(flag, x):
    if flag:
        y = x
    else:
        y = 1
    return y
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let facts = infer_module_value_facts(&lowered);
        let local_plan = plan_local_env_module(&lowered, &facts);
        let resume_plan = plan_local_env_resume_module(&lowered, &local_plan, &facts);

        validate_local_env_resume_module_plan(&lowered, &local_plan, &facts, &resume_plan)
            .expect("fresh LocalEnv resume plan should validate");

        for function in &lowered.callable_defs {
            let function_plan = resume_plan
                .function(function.function_id)
                .expect("each function should have a LocalEnv resume plan");
            let block_entry_count = function_plan
                .entries
                .iter()
                .filter(|entry| matches!(entry.point, LocalEnvResumePoint::BlockEntry { .. }))
                .count();
            let before_term_count = function_plan
                .entries
                .iter()
                .filter(|entry| matches!(entry.point, LocalEnvResumePoint::BeforeTerm { .. }))
                .count();
            assert_eq!(block_entry_count, function.blocks.len());
            assert_eq!(before_term_count, function.blocks.len());
            assert!(
                function_plan
                    .entries
                    .iter()
                    .any(|entry| matches!(entry.point, LocalEnvResumePoint::BeforeInstr { .. })),
                "non-empty functions should expose instruction-keyed resume points"
            );
            assert!(function_plan
                .entries
                .iter()
                .all(|entry| entry.point.function_id() == function.function_id));
        }
    }

    #[test]
    fn local_env_resume_plan_applies_direct_local_store_and_del_effects() {
        let lowered = lower_python_to_blockpy_for_testing(
            r#"
def f():
    x = None
    del x
    return 1
"#,
        )
        .expect("transform should succeed")
        .codegen_module;
        let facts = infer_module_value_facts(&lowered);
        let local_plan = plan_local_env_module(&lowered, &facts);
        let resume_plan = plan_local_env_resume_module(&lowered, &local_plan, &facts);
        validate_local_env_resume_module_plan(&lowered, &local_plan, &facts, &resume_plan)
            .expect("fresh LocalEnv resume plan should validate");

        let function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "f")
            .expect("lowered function should exist");
        let function_plan = resume_plan
            .function(function.function_id)
            .expect("function should have a LocalEnv resume plan");
        assert!(function_plan.entries.iter().any(|entry| {
            matches!(entry.point, LocalEnvResumePoint::BeforeInstr { .. })
                && entry.locals.iter().any(|binding| {
                    binding.name == "x"
                        && binding.binding == LocalEnvResumeBindingState::Bound
                        && matches!(binding.source, LocalEnvResumeValueSource::StoredValue(_))
                })
        }));
        assert!(function_plan.entries.iter().any(|entry| {
            matches!(entry.point, LocalEnvResumePoint::BeforeTerm { .. })
                && entry.locals.iter().any(|binding| {
                    binding.name == "x"
                        && binding.binding == LocalEnvResumeBindingState::Unbound
                        && binding.source == LocalEnvResumeValueSource::Unbound
                        && binding.ownership == LocalRefKind::Unbound
                })
        }));
    }
}
