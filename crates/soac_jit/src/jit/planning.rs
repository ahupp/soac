use soac_core::block_py::{
    BlockArg, BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, InstrLocationMap,
    LocalLocation, RuntimeFunctionId, current_instr_locations,
};
use soac_ir_blockpy::CodegenModuleShape;
use soac_ir_typed::{
    FactStore, TypedBlock, TypedCodegenModuleShape, lower_codegen_module_to_typed,
};
pub use soac_opt::passes::{
    BlockParamFacts, FunctionLocalPlan, LocalRefKind, ParamBindingFacts, ParamProvenance,
    PlannedLocalBinding, PlannedLocalStorage, render_planned_local_binding,
};
use soac_opt::passes::{
    FunctionLocalEnvResumePlan, FunctionRefcountPlan, LocalEnvModulePlan, LocalEnvResumeEntry,
    LocalEnvResumeModulePlan, LocalEnvResumePoint, LocalEnvResumeStatePrecision,
    RefcountActionKind, RefcountPlan, RefcountReleaseReason, annotate_typed_module_value_facts,
    compute_typed_function_local_live_ins, compute_typed_function_local_must_bound_ins,
    lower_typed_if_tests_to_truthy, plan_typed_local_env_module,
    plan_typed_local_env_resume_module, plan_typed_ownership_effects,
    validate_typed_local_env_module_plan, validate_typed_local_env_resume_module_plan,
    validate_typed_ownership_effects,
};
use std::collections::{HashMap, HashSet};
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct PreparedJitTypedModulePlan {
    pub module: BlockPyModule<TypedCodegenModuleShape>,
    pub value_facts: FactStore,
    pub local_env_plan: LocalEnvModulePlan,
    pub local_env_resume_plan: LocalEnvResumeModulePlan,
    pub locals: PlannedJitModuleLocals,
    pub deopt_resume: PlannedJitDeoptResumeModule,
}

fn can_release_via_stack_slot_fallback(name: &str) -> bool {
    name.starts_with("_dp_try_exc_")
        || name.starts_with("_dp_try_abrupt_kind_")
        || name.starts_with("_dp_try_abrupt_payload_")
}

fn typed_block_indices_by_label(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> HashMap<BlockLabel, usize> {
    function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect()
}

fn typed_block_index_for_label(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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

#[derive(Clone, Debug)]
pub struct BlockExcDispatchPlan {
    pub target_index: usize,
    pub slot_writes: Vec<(String, BlockArg)>,
    pub target_args: Vec<(String, BlockArg)>,
    pub forwarded_local_names: Vec<String>,
    pub release_local_names: Vec<String>,
    pub drop_forwarded_local_names: Vec<String>,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlannedLocalEnvEntrySource {
    BlockParam { param_index: usize },
    StackSlotLoad,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedLocalEnvEntryMaterialization {
    pub binding: PlannedLocalBinding,
    pub entry_aliases: Vec<String>,
    pub source: PlannedLocalEnvEntrySource,
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
    pub local_locations: Vec<LocalLocation>,
}

impl PlannedJitModuleLocals {
    pub fn function(&self, function_id: RuntimeFunctionId) -> Option<&PlannedJitFunctionLocals> {
        self.functions.get(&function_id)
    }

    pub fn validate_for_typed_module(
        &self,
        module: &BlockPyModule<TypedCodegenModuleShape>,
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
        function: &BlockPyFunction<TypedCodegenModuleShape>,
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

            let mut seen_locations = HashSet::new();
            for location in &deopt_point.local_locations {
                if !seen_locations.insert(*location) {
                    errors.push(format!(
                        "JIT deopt point {:?} for function {} ({}) duplicates local location {}",
                        deopt_point.point,
                        function.function_id,
                        function.names.qualname,
                        location.0
                    ));
                }
                if entry.binding_for_location(*location).is_none() {
                    errors.push(format!(
                        "JIT deopt point {:?} for function {} ({}) references unavailable local \
                         location {}",
                        deopt_point.point,
                        function.function_id,
                        function.names.qualname,
                        location.0
                    ));
                }
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
        module: &BlockPyModule<TypedCodegenModuleShape>,
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
    pub fn required_stack_slot_names_for_function(
        &self,
        function: &BlockPyFunction<impl soac_core::block_py::ModuleShape>,
    ) -> Vec<String> {
        let mut required = HashSet::new();

        for params in &self.runtime_block_params {
            for param in params {
                if param.binding.storage == PlannedLocalStorage::StackSlot {
                    required.insert(param.binding.name.clone());
                }
            }
        }

        for seeds in &self.stack_slot_entry_seeds {
            for seed in seeds {
                required.insert(seed.binding.name.clone());
            }
        }

        for dispatch in self.exc_dispatches.iter().flatten() {
            for (target_name, _) in &dispatch.slot_writes {
                required.insert(target_name.clone());
            }
            for source_name in &dispatch.forwarded_local_names {
                required.insert(source_name.clone());
            }
        }

        for block_plan in self.refcount_plan.blocks.values() {
            for action in &block_plan.actions {
                if let RefcountActionKind::ReleaseLocal { local, reason, .. } = &action.kind {
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

    pub fn validate_for_typed_function(
        &self,
        function: &BlockPyFunction<TypedCodegenModuleShape>,
    ) -> Result<(), String> {
        let block_count = function.blocks.len();
        let block_indices_by_label = typed_block_indices_by_label(function);
        if self.runtime_block_params.len() != block_count
            || self.implicit_target_transports.len() != block_count
            || self.jump_edge_transports.len() != block_count
            || self.stack_slot_entry_seeds.len() != block_count
            || self.entry_materializations.len() != block_count
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
            validate_entry_materializations_for_block(
                function,
                block.label,
                index,
                &self.runtime_block_params[index],
                &self.stack_slot_entry_seeds[index],
                &self.entry_materializations[index],
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
                validate_exception_dispatch_ownership_sinks(function, block.label, dispatch)?;
            }
        }

        Ok(())
    }
}

fn validate_exception_dispatch_ownership_sinks<P: soac_core::block_py::ModuleShape>(
    function: &BlockPyFunction<P>,
    block_label: BlockLabel,
    dispatch: &BlockExcDispatchPlan,
) -> Result<(), String> {
    let mut errors = Vec::new();
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
    let target_source_names = named_block_arg_sources(&dispatch.target_args);
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
        if target_source_names.contains(name) {
            sinks.push("target");
        }
        if release_names.contains(name) {
            sinks.push("release");
        }
        if drop_names.contains(name) {
            sinks.push("drop");
        }
        if sinks.len() != 1 {
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

fn validate_entry_materializations_for_block<P: soac_core::block_py::ModuleShape>(
    function: &BlockPyFunction<P>,
    block_label: BlockLabel,
    block_index: usize,
    runtime_params: &[RuntimeBlockParamPlan],
    stack_slot_entry_seeds: &[PlannedStackSlotEntrySeed],
    entry_materializations: &[PlannedLocalEnvEntryMaterialization],
) -> Result<(), String> {
    let expected_count = runtime_params.len() + stack_slot_entry_seeds.len();
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
            PlannedLocalStorage::BlockParam => param.binding.param_facts.ownership,
            PlannedLocalStorage::StackSlot => {
                local_ref_kind_for_stack_mirror(param.binding.param_facts.ownership)
            }
        };
        if entry.source != (PlannedLocalEnvEntrySource::BlockParam { param_index })
            || entry.binding != param.binding
            || entry.entry_aliases != param.entry_aliases
            || entry.entry_ref_kind != expected_entry_ref_kind
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
        {
            return Err(format!(
                "stack-slot entry materialization mismatch for function {} ({}) block {} \
                 seed index {}",
                function.function_id, function.names.qualname, block_label, seed_index
            ));
        }
    }
    Ok(())
}

pub fn plan_jit_typed_module_locals_from_passes(
    module: &BlockPyModule<TypedCodegenModuleShape>,
    facts: &FactStore,
    local_env_plan: &LocalEnvModulePlan,
    refcount_plan: &RefcountPlan,
) -> Result<PlannedJitModuleLocals, String> {
    validate_typed_local_env_module_plan(module, facts, local_env_plan)?;
    validate_typed_ownership_effects(module, facts, refcount_plan)?;
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
        let function_refcount_plan = refcount_plan
            .function(function.function_id)
            .cloned()
            .unwrap_or_default();
        let function_plan = plan_jit_typed_function_locals_from_plans(
            function,
            local_plan,
            function_refcount_plan,
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

pub fn plan_jit_typed_deopt_resume_module_from_passes(
    module: &BlockPyModule<TypedCodegenModuleShape>,
    facts: &FactStore,
    local_env_plan: &LocalEnvModulePlan,
    resume_plan: &LocalEnvResumeModulePlan,
) -> Result<PlannedJitDeoptResumeModule, String> {
    validate_typed_local_env_module_plan(module, facts, local_env_plan)?;
    validate_typed_local_env_resume_module_plan(module, local_env_plan, facts, resume_plan)?;
    let mut functions = HashMap::with_capacity(module.callable_defs.len());
    for function in &module.callable_defs {
        let resume_plan = resume_plan
            .function(function.function_id)
            .cloned()
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

pub fn plan_jit_typed_module(
    module: BlockPyModule<TypedCodegenModuleShape>,
    value_facts: FactStore,
) -> Result<PreparedJitTypedModulePlan, String> {
    let local_env_plan = plan_typed_local_env_module(&module, &value_facts);
    let local_env_resume_plan =
        plan_typed_local_env_resume_module(&module, &local_env_plan, &value_facts);
    let refcount_plan = plan_typed_ownership_effects(&module, &value_facts);
    let locals = plan_jit_typed_module_locals_from_passes(
        &module,
        &value_facts,
        &local_env_plan,
        &refcount_plan,
    )?;
    let deopt_resume = plan_jit_typed_deopt_resume_module_from_passes(
        &module,
        &value_facts,
        &local_env_plan,
        &local_env_resume_plan,
    )?;
    Ok(PreparedJitTypedModulePlan {
        module,
        value_facts,
        local_env_plan,
        local_env_resume_plan,
        locals,
        deopt_resume,
    })
}

pub fn plan_jit_module_from_codegen(
    module: &BlockPyModule<CodegenModuleShape>,
    value_facts: FactStore,
) -> Result<PreparedJitTypedModulePlan, String> {
    let mut typed_module = lower_codegen_module_to_typed(module.clone());
    annotate_typed_module_value_facts(&mut typed_module, &value_facts);
    let typed_module = lower_typed_if_tests_to_truthy(typed_module);
    plan_jit_typed_module(typed_module, value_facts)
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
            local_locations: entry
                .locals
                .iter()
                .map(|binding| binding.location)
                .collect(),
        })
        .collect()
}

pub fn render_jit_deopt_resume_module(
    module: &BlockPyModule<TypedCodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
            for location in &deopt_point.local_locations {
                let binding = entry.binding_for_location(*location).ok_or_else(|| {
                    format!(
                        "deopt point {:?} for function {} ({}) references missing local location {}",
                        deopt_point.point,
                        function.function_id,
                        function.names.qualname,
                        location.0
                    )
                })?;
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
    module: &BlockPyModule<TypedCodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
                param.entry_aliases
            )
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
                render_named_block_args(&dispatch.target_args)
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
        "{} source={:?} entry_ref_kind={:?} aliases={:?}",
        render_planned_local_binding(&entry.binding),
        entry.source,
        entry.entry_ref_kind,
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
        render_named_block_args(&transport.target_args)
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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

pub fn planned_stack_slot_entry_seeds_for_typed_function(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    local_plan: &FunctionLocalPlan,
) -> Vec<Vec<PlannedStackSlotEntrySeed>> {
    let live_ins = compute_typed_function_local_live_ins(function);
    let must_bound_ins = compute_typed_function_local_must_bound_ins(function);
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

pub fn planned_local_env_entry_materializations_for_function(
    runtime_block_params: &[Vec<RuntimeBlockParamPlan>],
    stack_slot_entry_seeds: &[Vec<PlannedStackSlotEntrySeed>],
) -> Result<Vec<Vec<PlannedLocalEnvEntryMaterialization>>, String> {
    if runtime_block_params.len() != stack_slot_entry_seeds.len() {
        return Err(format!(
            "entry materialization inputs have inconsistent block counts: runtime={}, stack_seeds={}",
            runtime_block_params.len(),
            stack_slot_entry_seeds.len()
        ));
    }
    Ok(runtime_block_params
        .iter()
        .zip(stack_slot_entry_seeds.iter())
        .map(|(params, seeds)| {
            let mut entries = Vec::with_capacity(params.len() + seeds.len());
            entries.extend(params.iter().enumerate().map(|(param_index, param)| {
                let entry_ref_kind = match param.binding.storage {
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

pub fn planned_implicit_target_transports_for_typed_function(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
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
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    block: &TypedBlock,
    runtime_target_params: &[RuntimeBlockParamPlan],
    refcount_plan: &FunctionRefcountPlan,
) -> Option<BlockExcDispatchPlan> {
    let exc_edge = block.exc_edge.as_ref()?;
    let block_indices_by_label = typed_block_indices_by_label(function);
    let target_index =
        typed_block_index_for_label(function, &block_indices_by_label, exc_edge.target);
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
    let drop_forwarded_local_names = planned_drop_forwarded_local_names(
        &forwarded_local_names,
        &transport.target_args,
        &release_local_names,
    );
    Some(BlockExcDispatchPlan {
        target_index,
        slot_writes: transport.slot_writes,
        target_args: transport.target_args,
        forwarded_local_names,
        release_local_names,
        drop_forwarded_local_names,
    })
}

fn planned_drop_forwarded_local_names(
    forwarded_local_names: &[String],
    target_args: &[(String, BlockArg)],
    release_local_names: &[String],
) -> Vec<String> {
    let target_arg_source_names = named_block_arg_sources(target_args);
    let release_name_set = release_local_names
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    forwarded_local_names
        .iter()
        .filter(|name| {
            !target_arg_source_names.contains(name.as_str())
                && !release_name_set.contains(name.as_str())
        })
        .cloned()
        .collect()
}

pub fn plan_jit_typed_function_locals_from_plans(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
    local_plan: FunctionLocalPlan,
    refcount_plan: FunctionRefcountPlan,
) -> Result<PlannedJitFunctionLocals, String> {
    let block_indices_by_label = typed_block_indices_by_label(function);
    let runtime_block_params = planned_jit_params_for_typed_function(function, &local_plan)?;
    let implicit_target_transports =
        planned_implicit_target_transports_for_typed_function(function, &runtime_block_params);
    let jump_edge_transports =
        planned_jump_edge_transports_for_typed_function(function, &runtime_block_params);
    let stack_slot_entry_seeds =
        planned_stack_slot_entry_seeds_for_typed_function(function, &local_plan);
    let entry_materializations = planned_local_env_entry_materializations_for_function(
        &runtime_block_params,
        &stack_slot_entry_seeds,
    )?;
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
            typed_exc_dispatch_plan(function, block, runtime_target_params, &refcount_plan)
        })
        .collect::<Vec<_>>();

    let plan = PlannedJitFunctionLocals {
        local_plan,
        refcount_plan,
        runtime_block_params,
        implicit_target_transports,
        jump_edge_transports,
        stack_slot_entry_seeds,
        entry_materializations,
        exc_dispatches,
    };
    plan.validate_for_typed_function(function)?;
    Ok(plan)
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_core::block_py::BlockTerm;
    use soac_lowering::lower_python_to_blockpy_for_testing;
    use soac_opt::passes::BlockLocalPlan;
    use soac_opt::passes::{
        LocalEnvResumeBindingState, LocalEnvResumeValueSource, RefcountActionKind,
        RefcountReleaseReason, infer_module_value_facts,
    };

    fn lowered_function(
        source: &str,
        qualname: &str,
    ) -> (
        soac_core::block_py::BlockPyModule<CodegenModuleShape>,
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

    fn plan_typed_module_from_codegen_module(
        module: &BlockPyModule<CodegenModuleShape>,
    ) -> PreparedJitTypedModulePlan {
        let facts = infer_module_value_facts(module);
        plan_jit_module_from_codegen(module, facts)
            .expect("typed JIT module planning should succeed")
    }

    fn prepared_typed_function(
        source: &str,
        qualname: &str,
    ) -> (PreparedJitTypedModulePlan, usize) {
        let (lowered, codegen_function_index) = lowered_function(source, qualname);
        let function_id = lowered.callable_defs[codegen_function_index].function_id;
        let prepared = plan_typed_module_from_codegen_module(&lowered);
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

    fn sparsely_relabel_function_blocks(function: &mut BlockPyFunction<CodegenModuleShape>) {
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
                BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
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
    fn local_plan_treats_function_params_as_owned_without_entry_fact() {
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
        let runtime_params = planned_jit_params_for_typed_function(function, plan)
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
        let runtime_params = planned_jit_params_for_typed_function(function, plan)
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

        let runtime_params = planned_jit_params_for_typed_function(function, plan)
            .expect("runtime params should bind");
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
        let runtime_params = planned_jit_params_for_typed_function(function, plan)
            .expect("runtime params should bind");
        let seeds = planned_stack_slot_entry_seeds_for_typed_function(function, plan);
        let block_indices_by_label = typed_block_indices_by_label(function);
        let else_index =
            typed_block_index_for_label(function, &block_indices_by_label, if_term.else_label);

        let else_params = &runtime_params[else_index];
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
            seeds[else_index]
                .iter()
                .all(|seed| seed.binding.name != "x"),
            "cleanup-only locals should not require stack-slot entry seeds"
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
        let runtime_params = planned_jit_params_for_typed_function(function, plan)
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
        let runtime_params = planned_jit_params_for_typed_function(function, plan)
            .expect("runtime params should bind");
        let seeds = planned_stack_slot_entry_seeds_for_typed_function(function, plan);
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
    fn planned_jit_function_locals_collects_codegen_local_state() {
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
        assert_eq!(
            materialization_count,
            runtime_param_count + stack_seed_count
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
            required_stack_slot_names
                .iter()
                .any(|name| name.starts_with("_dp_try_exc_")),
            "expected exception state stack slots to be represented in the pre-codegen plan: {required_stack_slot_names:?}"
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
            },
            RuntimeBlockParamPlan {
                arg_name: "z".to_string(),
                binding: stack_runtime_binding.clone(),
                entry_aliases: Vec::new(),
            },
        ]];
        let stack_slot_entry_seeds = vec![vec![PlannedStackSlotEntrySeed {
            binding: stack_binding.clone(),
            entry_ref_kind: LocalRefKind::Borrowed,
        }]];

        let entries = planned_local_env_entry_materializations_for_function(
            &runtime_params,
            &stack_slot_entry_seeds,
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
        .codegen_module;
        let prepared = plan_typed_module_from_codegen_module(&lowered);
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
        .codegen_module;
        let prepared = plan_typed_module_from_codegen_module(&lowered);

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
        let prepared = plan_typed_module_from_codegen_module(&lowered);
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
        .codegen_module;
        let prepared = plan_typed_module_from_codegen_module(&lowered);
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
        assert!(
            planned_deopt
                .local_locations
                .iter()
                .any(|location| *location == x_binding.location)
        );
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
        let runtime_params = planned_jit_params_for_typed_function(function, local_plan)
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
        let runtime_params = planned_jit_params_for_typed_function(function, local_plan)
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
                typed_exc_dispatch_plan(function, block, runtime_target_params, refcount_plan)
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
            target_args: vec![(
                "target_param".to_string(),
                BlockArg::Name("to_target".to_string()),
            )],
            forwarded_local_names: vec![
                "slot_only".to_string(),
                "to_target".to_string(),
                "released".to_string(),
                "dropped".to_string(),
            ],
            release_local_names: vec!["released".to_string()],
            drop_forwarded_local_names: vec!["slot_only".to_string(), "dropped".to_string()],
        };

        validate_exception_dispatch_ownership_sinks(function, block_label, &dispatch)
            .expect("dispatch with one ownership sink per forwarded local should validate");

        let mut double_sink = dispatch.clone();
        double_sink
            .drop_forwarded_local_names
            .push("to_target".to_string());
        let err = validate_exception_dispatch_ownership_sinks(function, block_label, &double_sink)
            .expect_err("targeted forwarded local should not also be dropped");
        assert!(
            err.contains("\"to_target\"") && err.contains("expected exactly one"),
            "expected a targeted+drop ownership-sink error, got: {err}"
        );

        let mut missing_sink = dispatch.clone();
        missing_sink
            .drop_forwarded_local_names
            .retain(|name| name != "slot_only");
        let err = validate_exception_dispatch_ownership_sinks(function, block_label, &missing_sink)
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
        let target_args = vec![(
            "target_param".to_string(),
            BlockArg::Name("to_target".to_string()),
        )];
        let release_names = vec!["released".to_string()];

        assert_eq!(
            planned_drop_forwarded_local_names(&forwarded, &target_args, &release_names),
            vec!["slot_only".to_string(), "unused".to_string()]
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
}
