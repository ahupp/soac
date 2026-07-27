use super::operation_specializations::OptV3ResolvedIndexedFieldAccess;
use super::planning::{
    PlannedJitDeoptResumeModule, PlannedJitModuleLocals, PreparedJitTypedModulePlan,
    plan_jit_typed_module_with_runtime_replay_module,
};
use super::{SpecializationProfile, annotate_typed_profiled_cold_blocks};
use crate::module_constants::ModuleCodegenConstants;
use crate::module_type::SharedModuleState;
use crate::session::SharedTypedModulePlanCacheKey;
use soac_config::SoacEnvConfig;
use soac_config::SpecializationMode;
use soac_core::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, CallArgKeyword, CallArgPositional,
    CallableScopeKind, CellLocation, ChildVisitable, ConstantExpr, CounterSite, FunctionKind,
    HasMeta, HasSemanticInstrId, InstrId, InstrLocationMap, Literal, LocalLocation, NameLike,
    PreservedLocation, ResolvedName, RuntimeFunctionId, RuntimeName, Visit, VisitMut, WithMeta,
    current_instr_locations,
};
use soac_ir_blockpy::{
    BlockPyModuleShape, InstrBlockPy, constructor_entry_function_id_for_init,
    constructor_init_function_id_for_entry_function,
};
use soac_ir_typed::emit_v3::MechanicalRegionEmission;
use soac_ir_typed::plan_v3::{
    CallBodyKind, CallBodyPlan, Cost, DirectCallCallee, ExactListItemAccessKind,
    IndexedFieldAccessKind as PlanV3IndexedFieldAccessKind, IndexedFieldReceiverSource,
    IndexedGlobalAccessKind as PlanV3IndexedGlobalAccessKind, RegionInputSource, RegionPlan,
    RegionSource,
};
use soac_ir_typed::{
    FactStore, InstrTyped, ProvenanceFact, TypedAttrAccessPlan, TypedAttrOwnerRef,
    TypedBlockPyModuleShape, TypedBuiltinImplementationPlan, TypedCall, TypedCallEmissionPlan,
    TypedCallEmissionPlans, TypedConstructorInitPlan, TypedConstructorInitPlanSource,
    TypedDirectCallArgPlan, TypedDirectFunctionCallGuard, TypedDirectMethodCallGuard,
    TypedExactIntBranchPlan, TypedExactIntPlanSource, TypedExactIntReturnPlan,
    TypedExactListItemAccessPlan, TypedExactListItemCounterSource, TypedExactListItemPlanSource,
    TypedGeneratorInstancePlan, TypedGeneratorResumePlan, TypedIndexedFieldCounterSource,
    TypedIndexedFieldPlanSource, TypedIndexedGlobalAccessPlan, TypedIndexedGlobalPlanSource,
    assign_missing_typed_function_instr_ids,
};
use soac_opt::access_emission_v3::{
    ExactListItemAccessPlan as OptV3ExactListItemAccessPlan,
    IndexedGlobalAccessPlan as OptV3IndexedGlobalAccessPlan,
};
use soac_opt::artifacts_v3::ExactIntBranchV3Artifacts;
use soac_opt::call_emission_v3::{ResolvedV3DirectCallPlan, typed_call_emission_plans_from_v3};
#[cfg(test)]
use soac_opt::passes::TypedVirtualObjectId;
#[cfg(test)]
use soac_opt::passes::trusted_generator_origin_for_name;
use soac_opt::passes::{
    TrustedGeneratorResumePlanLookup, TrustedGeneratorResumePlanMissReason, TrustedOwnerState,
    TrustedOwnerStateAnalysis, TypedConstructorFieldBindings, TypedExternalInlineCallee,
    TypedGeneratorStateConstructor, TypedGeneratorStateLoweringPlan,
    TypedHotContinuationSplitStats, TypedInlineConstantMapping, TypedInlineInstrIdMapping,
    TypedInlineLocalMapping, TypedVirtualBodyInstr, TypedVirtualFieldRef,
    TypedVirtualFieldStateAnalysis, TypedVirtualState, TypedVirtualizationPlan,
    analyze_trusted_function_states, analyze_trusted_owner_states,
    cleanup_lowered_typed_generator_alias_setup_with_existing_constructor,
    ensure_typed_generator_resume_boundary_writebacks,
    inline_typed_constructor_init_bodies_with_external_callees,
    inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls,
    linearize_typed_function_expressions, lower_typed_fully_virtual_objects_to_locals_with_plan,
    lower_typed_function_call_emission_plans,
    lower_typed_generator_resume_preserved_state_to_locals_and_collect_preserved_locals,
    lower_typed_generator_state_to_locals_with_plan_and_collect_preserved_locals,
    lower_typed_virtual_objects_to_locals_with_plan, plan_module_inlining,
    plan_typed_fully_virtual_objects, plan_typed_virtual_objects, prune_unreachable_typed_blocks,
    refresh_typed_function_value_facts,
    remap_typed_generator_preserved_instrs_with_existing_locals,
    rewrite_lowered_typed_generator_state_helper_calls_with_existing_constructor,
    rewrite_typed_stop_iteration_raises_to_handler_jumps, simplify_typed_virtual_tuple_ops,
    split_typed_alias_hot_continuations_with_budget,
    split_typed_constructor_hot_continuations_with_budget,
    split_typed_generator_alias_hot_continuations_with_budget,
    split_typed_inline_cleanup_hot_continuations_for_labels_with_budget, summarize_module_escapes,
    trusted_fully_virtual_constructor_owner, trusted_function_field_target_for_origin,
    trusted_function_id_for_expr, trusted_generator_instance_owner,
    trusted_generator_origin_has_escaped, trusted_generator_resume_function_fact_for_name,
    trusted_generator_resume_plan_lookup_for_expr, trusted_object_origin_for_name,
    trusted_owner_state_for_name, trusted_runtime_name_for_expr,
    typed_constructor_field_bindings_from_inline_stats_with_external_callees,
    typed_constructor_init_plans_from_inline_stats_with_external_callees,
    typed_generator_alias_ignored_instr_ids_by_origin,
    typed_generator_constructor_capture_bindings_by_origin,
    typed_generator_state_origin_can_lower_aliases_in_blocks, validate_typed_function_value_facts,
    visit_trusted_owner_term_instrs,
};
use soac_opt::region_emission_v3::{
    ExactIntBranchSelection as OptV3ExactIntBranchSelection,
    ExactIntReturnSelection as OptV3ExactIntReturnSelection,
    exact_int_branch_selection_for_source as opt_v3_exact_int_branch_selection_for_source,
    exact_int_return_selection_for_source as opt_v3_exact_int_return_selection_for_source,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

const MAX_TYPED_INLINE_PASSES: usize = 64;
const MAX_TYPED_INLINE_MAINTENANCE_PASSES: usize = 32;
const MAX_TYPED_INLINE_FUNCTION_BLOCKS: usize = 384;
const MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS: usize = 4096;
const MAX_LATE_TYPED_GENERATOR_RESUME_PASSES: usize = 8;
const MAX_LATE_TYPED_RUNTIME_PROTOCOL_PASSES: usize = 8;
const MAX_LATE_TYPED_BUILTIN_IMPLEMENTATION_PASSES: usize = 8;
const MAX_TRANSITIVE_PROFILE_INLINE_BLOCKS: usize = 8;
const MAX_TRANSITIVE_PROFILE_INLINE_BODY_INSTRS: usize = 32;
const MAX_GENERATOR_PROTOCOL_BRIDGE_INLINE_BLOCKS: usize = 24;
const MAX_GENERATOR_PROTOCOL_BRIDGE_INLINE_BODY_INSTRS: usize = 96;
const MAX_GENERATOR_RESUME_INLINE_BLOCKS: usize = 64;
const MAX_GENERATOR_RESUME_INLINE_BODY_INSTRS: usize = 512;
const MAX_TYPED_CONSTRUCTOR_CLONED_BLOCKS_PER_FUNCTION: usize = 256;
const MAX_TYPED_ALIAS_CLONED_BLOCKS_PER_FUNCTION: usize = 256;
const MAX_TYPED_GENERATOR_ALIAS_CLONED_BLOCKS_PER_FUNCTION: usize = 256;
const MAX_TYPED_INLINE_CLEANUP_CLONED_BLOCKS_PER_FUNCTION: usize = 256;

fn duration_micros(duration: Duration) -> u64 {
    u64::try_from(duration.as_micros()).unwrap_or(u64::MAX)
}

fn typed_inline_function_body_instr_count(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> usize {
    struct BodyInstrCounter {
        count: usize,
    }

    impl Visit<InstrTyped> for BodyInstrCounter {
        fn visit_instr(&mut self, instr: &InstrTyped) {
            if self.count >= MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS {
                return;
            }
            self.count += 1;
            instr.visit_children(self);
        }
    }

    let mut counter = BodyInstrCounter { count: 0 };
    counter.visit_fn(function);
    counter.count
}

fn typed_inline_function_within_cfg_budget(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> bool {
    function.blocks.len() < MAX_TYPED_INLINE_FUNCTION_BLOCKS
        && typed_inline_function_body_instr_count(function) < MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS
}

fn typed_inline_remaining_cfg_blocks(function: &BlockPyFunction<TypedBlockPyModuleShape>) -> usize {
    MAX_TYPED_INLINE_FUNCTION_BLOCKS
        .saturating_sub(function.blocks.len())
        .saturating_sub(1)
}

type TypedInlineTargets = HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>;
type StaticTypedDirectCalls =
    HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>;
type SuppressedTypedInlineTargets = HashMap<RuntimeFunctionId, HashSet<InstrId>>;
type RemappedTypedCallEmissions = HashMap<RuntimeFunctionId, TypedCallEmissionPlans>;
type LoweredGeneratorPreservedLocals = HashMap<InstrId, HashMap<PreservedLocation, ResolvedName>>;
type RemappedTypedGeneratorInstancePlans =
    HashMap<RuntimeFunctionId, HashMap<InstrId, TypedGeneratorInstancePlan>>;

#[derive(Clone)]
struct StaticDirectCallTarget {
    function: BlockPyFunction<BlockPyModuleShape>,
    constructor_owner_type_ref: Option<TypedAttrOwnerRef>,
}

type StaticRuntimeDirectCallTargets = HashMap<RuntimeName, StaticDirectCallTarget>;
type StaticRuntimeBuiltinImplementationTargets =
    HashMap<RuntimeName, BlockPyFunction<BlockPyModuleShape>>;
type StaticModuleGlobalDirectCallTargets = HashMap<u32, HashMap<String, StaticDirectCallTarget>>;
type StaticModuleGlobalGeneratorTargets =
    HashMap<u32, HashMap<String, BlockPyFunction<BlockPyModuleShape>>>;
type StaticStrictMethodTargets =
    HashMap<(String, String, String), BlockPyFunction<BlockPyModuleShape>>;

#[derive(Clone, Default)]
struct StaticDirectCallTargets {
    runtime_names: StaticRuntimeDirectCallTargets,
    runtime_builtin_implementations: StaticRuntimeBuiltinImplementationTargets,
    module_globals: StaticModuleGlobalDirectCallTargets,
    module_global_generators: StaticModuleGlobalGeneratorTargets,
    strict_methods: StaticStrictMethodTargets,
    suppressed_source_generators: HashSet<RuntimeFunctionId>,
}

fn typed_expr_is_runtime_name_load(
    expr: &InstrTyped,
    runtime_name: RuntimeName,
    module_constants: &[ConstantExpr],
) -> bool {
    let InstrTyped::Load(load) = expr else {
        return false;
    };
    if load.name.runtime_name_id() == Some(runtime_name) {
        return true;
    }
    let Some(index) = load.name.location.as_constant() else {
        return false;
    };
    matches!(
        module_constants.get(index as usize),
        Some(ConstantExpr::RuntimeName(name)) if *name == runtime_name
    )
}

fn typed_expr_runtime_name_provenance(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<RuntimeName> {
    if let InstrTyped::Load(load) = expr
        && let Some(runtime_name) = load.name.runtime_name_id()
    {
        return Some(runtime_name);
    }
    let facts = expr.result_facts()?.as_pyobj()?;
    let ProvenanceFact::ModuleConstant(index) = facts.provenance else {
        return None;
    };
    match module_constants.get(index as usize) {
        Some(ConstantExpr::RuntimeName(runtime_name)) => Some(*runtime_name),
        _ => None,
    }
}

fn typed_constant_string<'a>(
    expr: &InstrTyped,
    module_constants: &'a [ConstantExpr],
) -> Option<&'a str> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    let constant_index = load.name.location.as_constant()? as usize;
    match module_constants.get(constant_index)? {
        ConstantExpr::Literal(value) => match value.as_literal() {
            Literal::StringLiteral(value) => Some(value.value.as_str()),
            Literal::BytesLiteral(_) | Literal::NumberLiteral(_) => None,
        },
        ConstantExpr::RuntimeName(_) => None,
    }
}

#[derive(Clone)]
enum StaticDirectCallSource {
    RuntimeName(RuntimeName),
    StrictModuleGlobal(String),
}

fn static_constructor_call_owner_refs(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    targets: &StaticDirectCallTargets,
) -> HashMap<InstrId, TypedAttrOwnerRef> {
    struct Collector<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &'a [ConstantExpr],
        targets: &'a StaticDirectCallTargets,
        calls: HashMap<InstrId, TypedAttrOwnerRef>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            let static_constructor = match expr {
                InstrTyped::CallTyped(call) => {
                    call.try_semantic_instr_id()
                        .zip(static_direct_call_target_for_expr(
                            self.function,
                            call.func.as_ref(),
                            self.module_constants,
                            self.targets,
                        ))
                }
                InstrTyped::DirectCallableCallTyped(call) => {
                    call.try_semantic_instr_id()
                        .zip(static_direct_call_target_for_expr(
                            self.function,
                            call.func.as_ref(),
                            self.module_constants,
                            self.targets,
                        ))
                }
                _ => None,
            };
            if let Some((instr_id, (target, _))) = static_constructor
                && let Some(owner_type_ref) = &target.constructor_owner_type_ref
            {
                self.calls.insert(instr_id, owner_type_ref.clone());
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        function,
        module_constants,
        targets,
        calls: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.calls
}

fn static_runtime_constructor_init_qualname(runtime_name: RuntimeName) -> Option<&'static str> {
    match runtime_name {
        RuntimeName::Range => Some("range.__init__"),
        RuntimeName::IterRange => Some("IterRange.__init__"),
        RuntimeName::ClosureGenerator => Some("ClosureGenerator.__init__"),
        RuntimeName::ClosureAsyncGenerator => Some("ClosureAsyncGenerator.__init__"),
        _ => None,
    }
}

#[derive(Default)]
struct StrictModuleGlobalWriteSummary {
    module_store_count: usize,
    total_store_count: usize,
    delete_count: usize,
}

fn summarize_strict_module_global_writes(
    module: &BlockPyModule<BlockPyModuleShape>,
) -> HashMap<String, StrictModuleGlobalWriteSummary> {
    struct Collector<'a> {
        is_module_scope: bool,
        writes: &'a mut HashMap<String, StrictModuleGlobalWriteSummary>,
    }

    impl Visit<InstrBlockPy> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrBlockPy) {
            match expr {
                InstrBlockPy::Store(store) if store.name.location.is_global() => {
                    let summary = self
                        .writes
                        .entry(store.name.id_str().to_string())
                        .or_default();
                    summary.total_store_count += 1;
                    summary.module_store_count += usize::from(self.is_module_scope);
                }
                InstrBlockPy::Del(del) if del.name.location.is_global() => {
                    self.writes
                        .entry(del.name.id_str().to_string())
                        .or_default()
                        .delete_count += 1;
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut writes = HashMap::new();
    for function in &module.callable_defs {
        let mut collector = Collector {
            is_module_scope: function.scope.scope_kind == CallableScopeKind::Module,
            writes: &mut writes,
        };
        collector.visit_fn(function);
    }
    writes
}

fn is_strict_module_final_global(
    writes: &HashMap<String, StrictModuleGlobalWriteSummary>,
    name: &str,
) -> bool {
    writes.get(name).is_some_and(|summary| {
        summary.module_store_count == 1
            && summary.total_store_count == 1
            && summary.delete_count == 0
    })
}

fn strict_module_global_direct_call_targets_for_module(
    module: &BlockPyModule<BlockPyModuleShape>,
    module_name: &str,
) -> HashMap<String, StaticDirectCallTarget> {
    let writes = summarize_strict_module_global_writes(module);
    let mut targets = HashMap::new();

    for function in &module.callable_defs {
        if function.names.qualname != function.names.bind_name
            || function.names.bind_name.starts_with("_dp_")
            || function.lowered_kind() != &FunctionKind::Function
            || !is_strict_module_final_global(&writes, function.names.bind_name.as_str())
        {
            continue;
        }
        targets.insert(
            function.names.bind_name.clone(),
            StaticDirectCallTarget {
                function: function.clone(),
                constructor_owner_type_ref: None,
            },
        );
    }

    if module_name != "soac.runtime" {
        return targets;
    }

    for global_name in writes.keys() {
        if !is_strict_module_final_global(&writes, global_name.as_str()) {
            continue;
        }
        let init_qualname = format!("{global_name}.__init__");
        let Some(init_function) = module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == init_qualname)
        else {
            continue;
        };
        let Some(entry_function_id) =
            constructor_entry_function_id_for_init(module, init_function.function_id)
        else {
            continue;
        };
        let Some(entry_function) = module
            .callable_defs
            .iter()
            .find(|function| function.function_id == entry_function_id)
        else {
            continue;
        };
        targets.insert(
            global_name.clone(),
            StaticDirectCallTarget {
                function: entry_function.clone(),
                constructor_owner_type_ref: Some(TypedAttrOwnerRef::TypeKey {
                    module_name: module_name.to_string(),
                    qualname: global_name.clone(),
                }),
            },
        );
    }

    targets
}

fn strict_module_global_generator_targets_for_module(
    module: &BlockPyModule<BlockPyModuleShape>,
) -> HashMap<String, BlockPyFunction<BlockPyModuleShape>> {
    let writes = summarize_strict_module_global_writes(module);
    module
        .callable_defs
        .iter()
        .filter(|function| {
            function.names.qualname == function.names.bind_name
                && !function.names.bind_name.starts_with("_dp_")
                && matches!(
                    function.lowered_kind(),
                    FunctionKind::Generator
                        | FunctionKind::Coroutine
                        | FunctionKind::AsyncGenerator
                )
                && is_strict_module_final_global(&writes, function.names.bind_name.as_str())
        })
        .map(|function| (function.names.bind_name.clone(), function.clone()))
        .collect()
}

fn keeps_original_source_generator_vectorcall(
    function_kind: &FunctionKind,
    display_name: &str,
    has_original_runtime_code: bool,
    records_specialization_counters: bool,
) -> bool {
    has_original_runtime_code
        && *function_kind == FunctionKind::Generator
        && display_name != "<genexpr>"
        && !records_specialization_counters
}

fn suppressed_source_generators_for_shared_state(
    shared_state: &SharedModuleState,
    records_specialization_counters: bool,
) -> HashSet<RuntimeFunctionId> {
    shared_state
        .lowered_module
        .callable_defs
        .iter()
        .filter(|function| {
            keeps_original_source_generator_vectorcall(
                function.lowered_kind(),
                function.names.display_name.as_str(),
                shared_state
                    .lookup_original_code(function.function_id)
                    .is_some(),
                records_specialization_counters,
            )
        })
        .map(|function| function.function_id)
        .collect()
}

fn strict_module_global_generator_targets_for_shared_state(
    shared_state: &SharedModuleState,
    _suppressed_source_generators: &HashSet<RuntimeFunctionId>,
) -> HashMap<String, BlockPyFunction<BlockPyModuleShape>> {
    strict_module_global_generator_targets_for_module(&shared_state.lowered_module)
}

fn strict_module_method_targets_for_module(
    module: &BlockPyModule<BlockPyModuleShape>,
    module_name: &str,
) -> StaticStrictMethodTargets {
    let writes = summarize_strict_module_global_writes(module);
    let mut targets = HashMap::new();
    for function in &module.callable_defs {
        let Some((owner_qualname, method_name)) = function.names.qualname.split_once('.') else {
            continue;
        };
        if owner_qualname.contains('.')
            || function.lowered_kind() != &FunctionKind::Function
            || !is_strict_module_final_global(&writes, owner_qualname)
        {
            continue;
        }
        targets.insert(
            (
                module_name.to_string(),
                owner_qualname.to_string(),
                method_name.to_string(),
            ),
            function.clone(),
        );
    }
    targets
}

fn static_direct_call_target_for_expr<'a>(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
    targets: &'a StaticDirectCallTargets,
) -> Option<(&'a StaticDirectCallTarget, StaticDirectCallSource)> {
    for runtime_name in RuntimeName::ALL.iter().copied() {
        if typed_expr_is_runtime_name_load(expr, runtime_name, module_constants)
            && let Some(target) = targets.runtime_names.get(&runtime_name)
        {
            return Some((target, StaticDirectCallSource::RuntimeName(runtime_name)));
        }
    }

    let InstrTyped::Load(load) = expr else {
        return None;
    };
    if function.scope.scope_kind == CallableScopeKind::Module || !load.name.location.is_global() {
        return None;
    }
    let module_id = function.function_id.runtime_module_id().as_u32();
    let name = load.name.id_str();
    targets
        .module_globals
        .get(&module_id)?
        .get(name)
        .map(|target| {
            (
                target,
                StaticDirectCallSource::StrictModuleGlobal(name.to_string()),
            )
        })
}

fn static_direct_call_targets(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    env_config: &SoacEnvConfig,
) -> Result<StaticDirectCallTargets, String> {
    let mut module_globals = HashMap::new();
    let mut module_global_generators = HashMap::new();
    let mut strict_methods = HashMap::new();
    let records_specialization_counters = env_config
        .specialization_mode()
        .is_some_and(SpecializationMode::records_counters);
    let mut suppressed_source_generators = suppressed_source_generators_for_shared_state(
        shared_state,
        records_specialization_counters,
    );
    let current_module_id = shared_state
        .lowered_module
        .module_name_gen
        .runtime_module_id()
        .as_u32();
    module_globals.insert(
        current_module_id,
        strict_module_global_direct_call_targets_for_module(
            &shared_state.lowered_module,
            shared_state.module_name.as_str(),
        ),
    );
    module_global_generators.insert(
        current_module_id,
        strict_module_global_generator_targets_for_shared_state(
            shared_state,
            &suppressed_source_generators,
        ),
    );
    strict_methods.extend(strict_module_method_targets_for_module(
        &shared_state.lowered_module,
        shared_state.module_name.as_str(),
    ));

    let shared_module_states = compile_session
        .map(crate::session::CompileSession::shared_module_states_snapshot)
        .transpose()?
        .unwrap_or_default();
    for state in &shared_module_states {
        suppressed_source_generators.extend(suppressed_source_generators_for_shared_state(
            state,
            records_specialization_counters,
        ));
        module_globals
            .entry(
                state
                    .lowered_module
                    .module_name_gen
                    .runtime_module_id()
                    .as_u32(),
            )
            .or_insert_with(|| {
                strict_module_global_direct_call_targets_for_module(
                    &state.lowered_module,
                    state.module_name.as_str(),
                )
            });
        module_global_generators
            .entry(
                state
                    .lowered_module
                    .module_name_gen
                    .runtime_module_id()
                    .as_u32(),
            )
            .or_insert_with(|| {
                strict_module_global_generator_targets_for_shared_state(
                    state,
                    &suppressed_source_generators,
                )
            });
        strict_methods.extend(strict_module_method_targets_for_module(
            &state.lowered_module,
            state.module_name.as_str(),
        ));
    }

    let runtime_module_id = if shared_state.module_name == "soac.runtime" {
        Some(
            shared_state
                .lowered_module
                .module_name_gen
                .runtime_module_id()
                .as_u32(),
        )
    } else {
        shared_module_states
            .iter()
            .find(|state| state.module_name == "soac.runtime")
            .map(|state| {
                state
                    .lowered_module
                    .module_name_gen
                    .runtime_module_id()
                    .as_u32()
            })
    };

    let runtime_globals = runtime_module_id.and_then(|module_id| module_globals.get(&module_id));
    let mut runtime_names = HashMap::new();
    for runtime_name in RuntimeName::ALL.iter().copied() {
        if static_runtime_constructor_init_qualname(runtime_name).is_none() {
            continue;
        }
        if let Some(target) = runtime_globals.and_then(|globals| globals.get(runtime_name.name())) {
            runtime_names.insert(runtime_name, target.clone());
        }
    }
    let mut runtime_builtin_implementations = HashMap::new();
    for (source, helper_name) in [
        (RuntimeName::List, "list_from_iter"),
        (RuntimeName::Set, "set_from_iter"),
        (RuntimeName::Tuple, "tuple_from_iter"),
    ] {
        if let Some(target) = runtime_globals.and_then(|globals| globals.get(helper_name)) {
            runtime_builtin_implementations.insert(source, target.function.clone());
        }
    }
    Ok(StaticDirectCallTargets {
        runtime_names,
        runtime_builtin_implementations,
        module_globals,
        module_global_generators,
        strict_methods,
        suppressed_source_generators,
    })
}

fn static_direct_call_body() -> CallBodyPlan {
    CallBodyPlan {
        kind: CallBodyKind::DirectCall,
        cost: Cost {
            hot_path: 8,
            miss_path: 2,
            deopt: 0,
            materialization: 0,
            ownership: 1,
            code_size: 2,
            compile: 1,
        },
        inline_target: None,
        reason: "statically known call uses typed direct-call lowering".to_string(),
    }
}

fn static_inline_call_body() -> CallBodyPlan {
    CallBodyPlan {
        kind: CallBodyKind::Inline,
        cost: Cost {
            hot_path: 2,
            miss_path: 2,
            deopt: 0,
            materialization: 0,
            ownership: 0,
            code_size: 6,
            compile: 4,
        },
        inline_target: None,
        reason: "statically known runtime-name call uses typed inline lowering".to_string(),
    }
}

fn static_call_body_for_target(target: &StaticDirectCallTarget) -> CallBodyPlan {
    if target
        .constructor_owner_type_ref
        .as_ref()
        .is_some_and(trusted_fully_virtual_constructor_owner)
    {
        static_inline_call_body()
    } else {
        static_direct_call_body()
    }
}

fn typed_direct_call_arg_plan_from_direct_plan(
    plan: super::direct_function::DirectCallArgPlan,
) -> TypedDirectCallArgPlan {
    TypedDirectCallArgPlan {
        sources: plan
            .sources
            .into_iter()
            .map(|source| match source {
                super::direct_function::DirectCallArgSource::Provided(index) => {
                    soac_ir_typed::TypedDirectCallArgSource::Provided(index)
                }
                super::direct_function::DirectCallArgSource::PackedRest { start } => {
                    soac_ir_typed::TypedDirectCallArgSource::PackedRest { start }
                }
                super::direct_function::DirectCallArgSource::DefaultSentinel => {
                    soac_ir_typed::TypedDirectCallArgSource::DefaultSentinel
                }
            })
            .collect(),
    }
}

fn static_direct_call_plans_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    targets: &StaticDirectCallTargets,
) -> HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>> {
    struct Collector<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &'a [ConstantExpr],
        targets: &'a StaticDirectCallTargets,
        direct_calls: HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && let Some(instr_id) = call.try_semantic_instr_id()
            {
                if let Some((target, source)) = static_direct_call_target_for_expr(
                    self.function,
                    call.func.as_ref(),
                    self.module_constants,
                    self.targets,
                ) {
                    let has_starred_arguments = call.args.iter().any(|arg| {
                        matches!(arg, soac_core::block_py::CallArgPositional::Starred(_))
                    });
                    let explicit_positional_arg_count = call
                        .args
                        .iter()
                        .filter(|arg| {
                            matches!(arg, soac_core::block_py::CallArgPositional::Positional(_))
                        })
                        .count();
                    let implicit_positional_arg_count = usize::from(
                        soac_ir_blockpy::is_constructor_entry_function(&target.function),
                    );
                    if let Ok(arg_plan) = super::direct_function::plan_direct_call_args_for_target(
                        &target.function,
                        explicit_positional_arg_count,
                        implicit_positional_arg_count,
                        has_starred_arguments,
                        !call.keywords.is_empty(),
                    ) {
                        let arg_plan = typed_direct_call_arg_plan_from_direct_plan(arg_plan);
                        if !(soac_ir_blockpy::is_constructor_entry_function(&target.function)
                            && arg_plan.sources.iter().any(|source| {
                                matches!(
                                    source,
                                    soac_ir_typed::TypedDirectCallArgSource::DefaultSentinel
                                )
                            }))
                        {
                            self.direct_calls.entry(instr_id).or_default().push(
                                ResolvedV3DirectCallPlan {
                                    source: instr_id,
                                    target: target.function.function_id,
                                    callee: DirectCallCallee::Function,
                                    arg_plan,
                                    body: static_call_body_for_target(target),
                                    reason: match source {
                                        StaticDirectCallSource::RuntimeName(runtime_name) => format!(
                                            "runtime name {} resolves to a static target",
                                            runtime_name.name()
                                        ),
                                        StaticDirectCallSource::StrictModuleGlobal(name) => format!(
                                            "strict module global {name} resolves to a static target"
                                        ),
                                    },
                                },
                            );
                        }
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        function,
        module_constants,
        targets,
        direct_calls: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.direct_calls
}

fn static_direct_calls_for_module(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    targets: &StaticDirectCallTargets,
) -> StaticTypedDirectCalls {
    module
        .callable_defs
        .iter()
        .filter_map(|function| {
            let direct_calls =
                static_direct_call_plans_for_function(function, &module.module_constants, targets);
            (!direct_calls.is_empty()).then_some((function.function_id, direct_calls))
        })
        .collect()
}

fn static_direct_calls_for_external_callees(
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    targets: &StaticDirectCallTargets,
) -> StaticTypedDirectCalls {
    external_callees
        .values()
        .filter_map(|callee| {
            let direct_calls = static_direct_call_plans_for_function(
                &callee.function,
                &callee.module_constants,
                targets,
            );
            (!direct_calls.is_empty()).then_some((callee.function.function_id, direct_calls))
        })
        .collect()
}

fn static_generator_instance_plan_for_expr(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    expr: &InstrTyped,
    targets: &StaticDirectCallTargets,
    explicit_positional_arg_count: usize,
    has_starred_arguments: bool,
    has_keywords: bool,
) -> Option<TypedGeneratorInstancePlan> {
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    if function.scope.scope_kind == CallableScopeKind::Module || !load.name.location.is_global() {
        return None;
    }
    let module_id = function.function_id.runtime_module_id().as_u32();
    let target = targets
        .module_global_generators
        .get(&module_id)?
        .get(load.name.id_str())?;
    if targets
        .suppressed_source_generators
        .contains(&target.function_id)
    {
        return None;
    }
    let arg_plan = super::direct_function::plan_public_call_args_for_target(
        target,
        explicit_positional_arg_count,
        0,
        has_starred_arguments,
        has_keywords,
    )
    .ok()?;
    Some(TypedGeneratorInstancePlan {
        function_id: target.function_id,
        kind: *target.lowered_kind(),
        arg_plan: typed_direct_call_arg_plan_from_direct_plan(arg_plan),
    })
}

fn static_generator_instance_plans_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    targets: &StaticDirectCallTargets,
) -> HashMap<InstrId, TypedGeneratorInstancePlan> {
    struct Collector<'a> {
        function: &'a BlockPyFunction<TypedBlockPyModuleShape>,
        targets: &'a StaticDirectCallTargets,
        plans: HashMap<InstrId, TypedGeneratorInstancePlan>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && let Some(instr_id) = call.try_semantic_instr_id()
            {
                let has_starred_arguments = call
                    .args
                    .iter()
                    .any(|arg| matches!(arg, soac_core::block_py::CallArgPositional::Starred(_)));
                let explicit_positional_arg_count = call
                    .args
                    .iter()
                    .filter(|arg| {
                        matches!(arg, soac_core::block_py::CallArgPositional::Positional(_))
                    })
                    .count();
                if let Some(plan) = static_generator_instance_plan_for_expr(
                    self.function,
                    call.func.as_ref(),
                    self.targets,
                    explicit_positional_arg_count,
                    has_starred_arguments,
                    !call.keywords.is_empty(),
                ) {
                    self.plans.insert(instr_id, plan);
                }
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        function,
        targets,
        plans: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.plans
}

fn local_generator_targets_for_module(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
) -> HashMap<RuntimeFunctionId, &BlockPyFunction<TypedBlockPyModuleShape>> {
    module
        .callable_defs
        .iter()
        .filter(|function| {
            matches!(
                function.lowered_kind(),
                FunctionKind::Generator | FunctionKind::Coroutine | FunctionKind::AsyncGenerator
            )
        })
        .map(|function| (function.function_id, function))
        .collect()
}

fn static_local_generator_instance_plan_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    local_generators: &HashMap<RuntimeFunctionId, &BlockPyFunction<TypedBlockPyModuleShape>>,
    explicit_positional_arg_count: usize,
    has_starred_arguments: bool,
    has_keywords: bool,
) -> Option<TypedGeneratorInstancePlan> {
    let function_id = trusted_function_id_for_expr(expr, state)?;
    let target = *local_generators.get(&function_id)?;
    let arg_plan = super::direct_function::plan_public_call_args_for_target(
        target,
        explicit_positional_arg_count,
        0,
        has_starred_arguments,
        has_keywords,
    )
    .ok()?;
    Some(TypedGeneratorInstancePlan {
        function_id: target.function_id,
        kind: *target.lowered_kind(),
        arg_plan: typed_direct_call_arg_plan_from_direct_plan(arg_plan),
    })
}

fn static_local_generator_instance_plans_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    local_generators: &HashMap<RuntimeFunctionId, &BlockPyFunction<TypedBlockPyModuleShape>>,
) -> HashMap<InstrId, TypedGeneratorInstancePlan> {
    struct Collector<'a> {
        state: &'a TrustedOwnerState,
        local_generators:
            &'a HashMap<RuntimeFunctionId, &'a BlockPyFunction<TypedBlockPyModuleShape>>,
        plans: HashMap<InstrId, TypedGeneratorInstancePlan>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && let Some(instr_id) = call.try_semantic_instr_id()
            {
                let has_starred_arguments = call
                    .args
                    .iter()
                    .any(|arg| matches!(arg, soac_core::block_py::CallArgPositional::Starred(_)));
                let explicit_positional_arg_count = call
                    .args
                    .iter()
                    .filter(|arg| {
                        matches!(arg, soac_core::block_py::CallArgPositional::Positional(_))
                    })
                    .count();
                if let Some(plan) = static_local_generator_instance_plan_for_expr(
                    call.func.as_ref(),
                    self.state,
                    self.local_generators,
                    explicit_positional_arg_count,
                    has_starred_arguments,
                    !call.keywords.is_empty(),
                ) {
                    self.plans.insert(instr_id, plan);
                }
            }
            expr.visit_children(self);
        }
    }

    let states = analyze_trusted_function_states(function);
    let mut plans = HashMap::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                continue;
            };
            let mut collector = Collector {
                state,
                local_generators,
                plans: HashMap::new(),
            };
            collector.visit_instr(instr);
            plans.extend(collector.plans);
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            state,
            local_generators,
            plans: HashMap::new(),
        };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
        plans.extend(collector.plans);
    }
    plans
}

fn static_generator_instance_plans_for_module(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    targets: &StaticDirectCallTargets,
) -> HashMap<RuntimeFunctionId, HashMap<InstrId, TypedGeneratorInstancePlan>> {
    let local_generators = local_generator_targets_for_module(module)
        .into_iter()
        .filter(|(function_id, _)| !targets.suppressed_source_generators.contains(function_id))
        .collect();
    module
        .callable_defs
        .iter()
        .filter_map(|function| {
            let mut plans = static_generator_instance_plans_for_function(function, targets);
            plans.extend(static_local_generator_instance_plans_for_function(
                function,
                &local_generators,
            ));
            (!plans.is_empty()).then_some((function.function_id, plans))
        })
        .collect()
}

fn annotate_typed_generator_instance_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    plans: Option<&HashMap<InstrId, TypedGeneratorInstancePlan>>,
) -> Result<usize, String> {
    let Some(plans) = plans else {
        return Ok(0);
    };
    if plans.is_empty() {
        return Ok(0);
    }
    let live_instr_ids = collect_typed_semantic_instr_ids(function);
    let expected = plans
        .keys()
        .filter(|instr_id| live_instr_ids.contains(instr_id))
        .count();

    struct Annotator<'a> {
        plans: &'a HashMap<InstrId, TypedGeneratorInstancePlan>,
        used: HashSet<InstrId>,
        count: usize,
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && let Some(instr_id) = call.try_semantic_instr_id()
                && let Some(plan) = self.plans.get(&instr_id)
            {
                if call.extra.generator_instance_plan() != Some(plan) {
                    call.extra.set_generator_instance_plan(plan.clone());
                    self.count += 1;
                }
                self.used.insert(instr_id);
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        plans,
        used: HashSet::new(),
        count: 0,
    };
    annotator.visit_fn_mut(function);
    if annotator.used.len() != expected {
        let missing = plans
            .keys()
            .filter(|instr_id| live_instr_ids.contains(instr_id))
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "typed generator-instance plans were not attached to call nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

fn typed_generator_instance_plans_by_origin(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<InstrId, TypedGeneratorInstancePlan> {
    struct Collector {
        plans: HashMap<InstrId, TypedGeneratorInstancePlan>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(plan) = expr.generator_instance_plan()
                && let Some(instr_id) = expr.try_semantic_instr_id()
            {
                self.plans.insert(instr_id, plan.clone());
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        plans: HashMap::new(),
    };
    collector.visit_fn(function);
    collector.plans
}

fn materialized_generator_state_constructors_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<InstrId, TypedGeneratorStateConstructor> {
    function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| {
            let InstrTyped::Store(store) = instr else {
                return None;
            };
            let call = typed_generator_state_constructor_call(store.value.as_ref())?;
            let generator_origin = store.value.try_semantic_instr_id()?;
            tracing::info!(
                target: "soac_generator_state_lowering",
                caller = ?function.function_id,
                generator_origin = ?generator_origin,
                target_name = store.name.id_str(),
                "typed_generator_state_constructor_snapshot_from_store",
            );
            Some((
                generator_origin,
                TypedGeneratorStateConstructor {
                    target: store.name.clone(),
                    call,
                    closure_cell_bindings: None,
                },
            ))
        })
        .collect()
}

fn trace_materialized_generator_state_constructor_anchors(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    pass: usize,
    stage: &'static str,
    constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
) {
    struct ConstructorTargetUseFinder<'a> {
        target: &'a str,
        found: bool,
    }

    impl Visit<InstrTyped> for ConstructorTargetUseFinder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            if let InstrTyped::Load(load) = expr
                && load.name.id_str() == self.target
            {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    for (generator_origin, constructor) in constructors {
        let constructor_location = constructor.target.local_location();
        let matching_store = function.blocks.iter().find_map(|block| {
            block.body.iter().find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                (store.name == constructor.target)
                    .then(|| (block.label, format!("{:?}", store.value.as_ref())))
            })
        });
        let same_slot_stores = function
            .blocks
            .iter()
            .flat_map(|block| {
                block.body.iter().filter_map(move |instr| {
                    let InstrTyped::Store(store) = instr else {
                        return None;
                    };
                    (store.name.local_location() == constructor_location).then(|| {
                        (
                            block.label,
                            store.name.id_str().to_string(),
                            format!("{:?}", store.value.as_ref()),
                        )
                    })
                })
            })
            .collect::<Vec<_>>();
        let remaining_load_uses = function
            .blocks
            .iter()
            .flat_map(|block| {
                block.body.iter().filter_map(move |instr| {
                    let mut finder = ConstructorTargetUseFinder {
                        target: constructor.target.id_str(),
                        found: false,
                    };
                    finder.visit_instr(instr);
                    finder.found.then(|| (block.label, format!("{instr:?}")))
                })
            })
            .collect::<Vec<_>>();
        tracing::info!(
            target: "soac_generator_state_lowering",
            caller = ?function.function_id,
            pass,
            stage,
            generator_origin = ?generator_origin,
            target_name = constructor.target.id_str(),
            entry_block = ?function.blocks.first().map(|block| block.label),
            store_block = ?matching_store.as_ref().map(|(block, _)| *block),
            has_store = matching_store.is_some(),
            store_value = ?matching_store.as_ref().map(|(_, value)| value),
            constructor_location = ?constructor_location,
            same_slot_stores = ?same_slot_stores,
            remaining_load_uses = ?remaining_load_uses,
            "typed_generator_state_constructor_anchor_state",
        );
    }
}

fn typed_generator_state_constructor_call(expr: &InstrTyped) -> Option<TypedCall<InstrTyped>> {
    match expr {
        InstrTyped::CallTyped(call) if call.extra.generator_instance_plan().is_some() => {
            Some(call.clone())
        }
        InstrTyped::GuardedCallableCallTyped(call)
            if call.extra.generator_instance_plan().is_some() =>
        {
            Some(call.clone().into_typed_call())
        }
        InstrTyped::DirectCallableCallTyped(call)
            if call.extra.generator_instance_plan().is_some() =>
        {
            let mut normalized = TypedCall::generic(
                call.func.clone(),
                call.args.clone(),
                Vec::<CallArgKeyword<InstrTyped>>::new(),
            )
            .with_meta(call.meta());
            normalized.extra = call.extra.clone();
            Some(normalized)
        }
        _ => None,
    }
}

fn trusted_generator_instance_plan_for_expr<'a>(
    expr: &'a InstrTyped,
    state: &TrustedOwnerState,
    plans_by_origin: &'a HashMap<InstrId, TypedGeneratorInstancePlan>,
) -> Option<&'a TypedGeneratorInstancePlan> {
    if let Some(plan) = expr.generator_instance_plan() {
        return (plan.kind == FunctionKind::Generator).then_some(plan);
    }
    let InstrTyped::Load(load) = expr else {
        return None;
    };
    let origin = trusted_object_origin_for_name(&load.name, state)?;
    if trusted_generator_origin_has_escaped(origin, state) {
        return None;
    }
    let plan = plans_by_origin.get(&origin)?;
    (plan.kind == FunctionKind::Generator).then_some(plan)
}

#[cfg(test)]
fn trusted_generator_builtin_implementation_plans_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    static_targets: &StaticDirectCallTargets,
) -> HashMap<InstrId, TypedBuiltinImplementationPlan> {
    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    trusted_generator_builtin_implementation_plans_from_analysis(
        function,
        module,
        external_callees,
        module_constants,
        &states,
        static_targets,
    )
}

fn trusted_generator_builtin_implementation_plans_from_analysis(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    module_constants: &[ConstantExpr],
    states: &TrustedOwnerStateAnalysis,
    static_targets: &StaticDirectCallTargets,
) -> HashMap<InstrId, TypedBuiltinImplementationPlan> {
    let plans_by_origin = typed_generator_instance_plans_by_origin(function);

    struct Collector<'a> {
        module: &'a BlockPyModule<TypedBlockPyModuleShape>,
        external_callees: &'a HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
        module_constants: &'a [ConstantExpr],
        state: &'a TrustedOwnerState,
        source: RuntimeName,
        target: &'a BlockPyFunction<BlockPyModuleShape>,
        plans_by_origin: &'a HashMap<InstrId, TypedGeneratorInstancePlan>,
        plans: HashMap<InstrId, TypedBuiltinImplementationPlan>,
    }

    impl Collector<'_> {
        fn maybe_collect_call(
            &mut self,
            instr_id: Option<InstrId>,
            func: &InstrTyped,
            args: &[CallArgPositional<InstrTyped>],
            has_keywords: bool,
        ) {
            if has_keywords
                || trusted_runtime_name_for_expr(func, self.state, self.module_constants)
                    != Some(self.source)
            {
                return;
            }
            let Some(instr_id) = instr_id else {
                tracing::debug!(
                    target: "soac_builtin_consumer_planning",
                    source = ?self.source,
                    "typed_builtin_generator_consumer_skipped_missing_instr_id",
                );
                return;
            };
            let [CallArgPositional::Positional(arg)] = args else {
                tracing::debug!(
                    target: "soac_builtin_consumer_planning",
                    source = ?self.source,
                    instr_id = ?instr_id,
                    positional_args = args.len(),
                    "typed_builtin_generator_consumer_skipped_arg_shape",
                );
                return;
            };
            let Some(generator_plan) =
                trusted_generator_instance_plan_for_expr(arg, self.state, self.plans_by_origin)
            else {
                let arg_has_generator_plan = arg.generator_instance_plan().is_some();
                let origin = match arg {
                    InstrTyped::Load(load) => {
                        trusted_object_origin_for_name(&load.name, self.state)
                    }
                    _ => None,
                };
                let origin_escaped = origin
                    .is_some_and(|origin| trusted_generator_origin_has_escaped(origin, self.state));
                let origin_has_generator_plan =
                    origin.is_some_and(|origin| self.plans_by_origin.contains_key(&origin));
                let local_location = match arg {
                    InstrTyped::Load(load) => load.name.local_location(),
                    _ => None,
                };
                let arg_shape = match arg {
                    InstrTyped::CallTyped(call) => match call.func.as_ref() {
                        InstrTyped::MakeFunctionWithClosure(_) => "call_make_function",
                        InstrTyped::Load(_) => "call_load",
                        _ => "call_other",
                    },
                    InstrTyped::GuardedCallableCallTyped(call) => match call.func.as_ref() {
                        InstrTyped::MakeFunctionWithClosure(_) => "guarded_call_make_function",
                        InstrTyped::Load(_) => "guarded_call_load",
                        _ => "guarded_call_other",
                    },
                    InstrTyped::DirectCallableCallTyped(call) => match call.func.as_ref() {
                        InstrTyped::MakeFunctionWithClosure(_) => "direct_call_make_function",
                        InstrTyped::Load(_) => "direct_call_load",
                        _ => "direct_call_other",
                    },
                    InstrTyped::Load(_) => "load",
                    _ => "other",
                };
                let arg_call_known_function_id = match arg {
                    InstrTyped::CallTyped(call) => {
                        trusted_function_id_for_expr(call.func.as_ref(), self.state)
                    }
                    InstrTyped::GuardedCallableCallTyped(call) => {
                        trusted_function_id_for_expr(call.func.as_ref(), self.state)
                    }
                    InstrTyped::DirectCallableCallTyped(call) => {
                        trusted_function_id_for_expr(call.func.as_ref(), self.state)
                    }
                    _ => None,
                };
                let (arg_call_func_name, arg_call_func_location) = match arg {
                    InstrTyped::CallTyped(call) => match call.func.as_ref() {
                        InstrTyped::Load(load) => {
                            (Some(load.name.id_str()), Some(&load.name.location))
                        }
                        _ => (None, None),
                    },
                    InstrTyped::GuardedCallableCallTyped(call) => match call.func.as_ref() {
                        InstrTyped::Load(load) => {
                            (Some(load.name.id_str()), Some(&load.name.location))
                        }
                        _ => (None, None),
                    },
                    InstrTyped::DirectCallableCallTyped(call) => match call.func.as_ref() {
                        InstrTyped::Load(load) => {
                            (Some(load.name.id_str()), Some(&load.name.location))
                        }
                        _ => (None, None),
                    },
                    _ => (None, None),
                };
                tracing::debug!(
                    target: "soac_builtin_consumer_planning",
                    source = ?self.source,
                    instr_id = ?instr_id,
                    arg_shape,
                    arg_call_known_function_id = ?arg_call_known_function_id,
                    arg_call_func_name = ?arg_call_func_name,
                    arg_call_func_location = ?arg_call_func_location,
                    arg_has_generator_plan,
                    local_location = ?local_location,
                    origin = ?origin,
                    origin_escaped,
                    origin_has_generator_plan,
                    "typed_builtin_generator_consumer_skipped_missing_generator_plan",
                );
                return;
            };
            if !typed_generator_resume_inline_target_is_small_enough(
                self.module,
                self.external_callees,
                generator_plan.function_id,
            ) {
                tracing::debug!(
                    target: "soac_builtin_consumer_planning",
                    source = ?self.source,
                    instr_id = ?instr_id,
                    generator_function_id = ?generator_plan.function_id,
                    "typed_builtin_generator_consumer_skipped_resume_inline_budget",
                );
                return;
            }
            let Ok(arg_plan) = super::direct_function::plan_direct_call_args_for_target(
                self.target,
                1,
                0,
                false,
                false,
            ) else {
                tracing::debug!(
                    target: "soac_builtin_consumer_planning",
                    source = ?self.source,
                    instr_id = ?instr_id,
                    target_function_id = ?self.target.function_id,
                    "typed_builtin_generator_consumer_skipped_arg_plan",
                );
                return;
            };
            self.plans.insert(
                instr_id,
                TypedBuiltinImplementationPlan {
                    source: self.source,
                    function_id: self.target.function_id,
                    arg_plan: typed_direct_call_arg_plan_from_direct_plan(arg_plan),
                },
            );
        }
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            match expr {
                InstrTyped::CallTyped(call) => self.maybe_collect_call(
                    call.try_semantic_instr_id(),
                    call.func.as_ref(),
                    &call.args,
                    !call.keywords.is_empty(),
                ),
                InstrTyped::GuardedCallableCallTyped(call) => self.maybe_collect_call(
                    call.try_semantic_instr_id(),
                    call.func.as_ref(),
                    &call.args,
                    !call.keywords.is_empty(),
                ),
                InstrTyped::DirectCallableCallTyped(call) => self.maybe_collect_call(
                    call.try_semantic_instr_id(),
                    call.func.as_ref(),
                    &call.args,
                    false,
                ),
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut plans = HashMap::new();
    for source in [RuntimeName::List, RuntimeName::Set, RuntimeName::Tuple] {
        let Some(target) = static_targets.runtime_builtin_implementations.get(&source) else {
            continue;
        };
        for block in &function.blocks {
            for (instr_index, instr) in block.body.iter().enumerate() {
                let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                    block: block.label,
                    instr_index,
                }) else {
                    continue;
                };
                let mut collector = Collector {
                    module,
                    external_callees,
                    module_constants,
                    state,
                    source,
                    target,
                    plans_by_origin: &plans_by_origin,
                    plans: HashMap::new(),
                };
                collector.visit_instr(instr);
                plans.extend(collector.plans);
            }
            let Some(state) = states.block_before_term.get(&block.label) else {
                continue;
            };
            let mut collector = Collector {
                module,
                external_callees,
                module_constants,
                state,
                source,
                target,
                plans_by_origin: &plans_by_origin,
                plans: HashMap::new(),
            };
            visit_trusted_owner_term_instrs(&block.term, &mut collector);
            plans.extend(collector.plans);
        }
    }
    if !plans.is_empty() {
        tracing::debug!(
            target: "soac_builtin_consumer_planning",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            total = plans.len(),
            list = plans
                .values()
                .filter(|plan| plan.source == RuntimeName::List)
                .count(),
            set = plans
                .values()
                .filter(|plan| plan.source == RuntimeName::Set)
                .count(),
            tuple = plans
                .values()
                .filter(|plan| plan.source == RuntimeName::Tuple)
                .count(),
            "typed_builtin_generator_consumer_plans",
        );
    }
    plans
}

fn annotate_typed_builtin_implementation_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    plans: &HashMap<InstrId, TypedBuiltinImplementationPlan>,
) -> Result<usize, String> {
    if plans.is_empty() {
        return Ok(0);
    }
    let live_instr_ids = collect_typed_semantic_instr_ids(function);
    let expected = plans
        .keys()
        .filter(|instr_id| live_instr_ids.contains(instr_id))
        .count();

    struct Annotator<'a> {
        plans: &'a HashMap<InstrId, TypedBuiltinImplementationPlan>,
        used: HashSet<InstrId>,
        count: usize,
    }

    impl Annotator<'_> {
        fn maybe_annotate(
            &mut self,
            instr_id: Option<InstrId>,
            extra: &mut soac_ir_typed::TypedInstrExtra,
        ) {
            let Some(instr_id) = instr_id else {
                return;
            };
            let Some(plan) = self.plans.get(&instr_id) else {
                return;
            };
            extra.set_builtin_implementation_plan(plan.clone());
            self.used.insert(instr_id);
            self.count += 1;
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            match expr {
                InstrTyped::CallTyped(call) => {
                    self.maybe_annotate(call.try_semantic_instr_id(), &mut call.extra);
                }
                InstrTyped::GuardedCallableCallTyped(call) => {
                    self.maybe_annotate(call.try_semantic_instr_id(), &mut call.extra);
                }
                InstrTyped::DirectCallableCallTyped(call) => {
                    self.maybe_annotate(call.try_semantic_instr_id(), &mut call.extra);
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        plans,
        used: HashSet::new(),
        count: 0,
    };
    annotator.visit_fn_mut(function);
    if annotator.used.len() != expected {
        let missing = plans
            .keys()
            .filter(|instr_id| live_instr_ids.contains(instr_id))
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "typed builtin-implementation plans were not attached to call nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

fn retain_selected_typed_builtin_implementation_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    plans: &HashMap<InstrId, TypedBuiltinImplementationPlan>,
    selected_targets: &TypedInlineTargets,
) -> Result<usize, String> {
    let selected_plans = plans
        .iter()
        .filter(|(source, _)| selected_targets.contains_key(source))
        .map(|(source, plan)| (*source, plan.clone()))
        .collect::<HashMap<_, _>>();

    struct UnselectedPlanClearer<'a> {
        selected_plans: &'a HashMap<InstrId, TypedBuiltinImplementationPlan>,
    }

    impl UnselectedPlanClearer<'_> {
        fn maybe_clear(
            &self,
            instr_id: Option<InstrId>,
            extra: &mut soac_ir_typed::TypedInstrExtra,
        ) {
            if instr_id.is_none_or(|source| !self.selected_plans.contains_key(&source)) {
                extra.clear_builtin_implementation_plan();
            }
        }
    }

    impl VisitMut<InstrTyped> for UnselectedPlanClearer<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            match expr {
                InstrTyped::CallTyped(call) => {
                    self.maybe_clear(call.try_semantic_instr_id(), &mut call.extra);
                }
                InstrTyped::GuardedCallableCallTyped(call) => {
                    self.maybe_clear(call.try_semantic_instr_id(), &mut call.extra);
                }
                InstrTyped::DirectCallableCallTyped(call) => {
                    self.maybe_clear(call.try_semantic_instr_id(), &mut call.extra);
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    UnselectedPlanClearer {
        selected_plans: &selected_plans,
    }
    .visit_fn_mut(function);
    annotate_typed_builtin_implementation_plans(function, &selected_plans)
}

#[derive(Default)]
struct BuiltinImplementationPlanPlacementCounts {
    store_values: usize,
    effect_only_body: usize,
    returns: usize,
    nested_body: usize,
    nested_terms: usize,
}

fn trace_builtin_implementation_plan_placements(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    plans: &HashMap<InstrId, TypedBuiltinImplementationPlan>,
) {
    if plans.is_empty() {
        return;
    }

    fn collect_nested_plan_ids(
        expr: &InstrTyped,
        plans: &HashMap<InstrId, TypedBuiltinImplementationPlan>,
        count: &mut usize,
    ) {
        struct Finder<'a, 'b> {
            plans: &'a HashMap<InstrId, TypedBuiltinImplementationPlan>,
            count: &'b mut usize,
        }

        impl Visit<InstrTyped> for Finder<'_, '_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if expr
                    .try_semantic_instr_id()
                    .is_some_and(|instr_id| self.plans.contains_key(&instr_id))
                {
                    *self.count += 1;
                }
                expr.visit_children(self);
            }
        }

        let mut finder = Finder { plans, count };
        expr.visit_children(&mut finder);
    }

    let mut placements = BuiltinImplementationPlanPlacementCounts::default();
    for block in &function.blocks {
        for instr in &block.body {
            match instr {
                InstrTyped::Store(store)
                    if store
                        .value
                        .try_semantic_instr_id()
                        .is_some_and(|instr_id| plans.contains_key(&instr_id)) =>
                {
                    placements.store_values += 1;
                    collect_nested_plan_ids(
                        store.value.as_ref(),
                        plans,
                        &mut placements.nested_body,
                    );
                }
                instr
                    if instr
                        .try_semantic_instr_id()
                        .is_some_and(|instr_id| plans.contains_key(&instr_id)) =>
                {
                    placements.effect_only_body += 1;
                    collect_nested_plan_ids(instr, plans, &mut placements.nested_body);
                }
                instr => {
                    collect_nested_plan_ids(instr, plans, &mut placements.nested_body);
                }
            }
        }
        match &block.term {
            BlockTerm::Return(expr)
                if expr
                    .try_semantic_instr_id()
                    .is_some_and(|instr_id| plans.contains_key(&instr_id)) =>
            {
                placements.returns += 1;
                collect_nested_plan_ids(expr, plans, &mut placements.nested_terms);
            }
            term => {
                struct TermFinder<'a, 'b> {
                    plans: &'a HashMap<InstrId, TypedBuiltinImplementationPlan>,
                    count: &'b mut usize,
                }

                impl Visit<InstrTyped> for TermFinder<'_, '_> {
                    fn visit_instr(&mut self, expr: &InstrTyped) {
                        if expr
                            .try_semantic_instr_id()
                            .is_some_and(|instr_id| self.plans.contains_key(&instr_id))
                        {
                            *self.count += 1;
                        }
                        expr.visit_children(self);
                    }
                }

                let mut finder = TermFinder {
                    plans,
                    count: &mut placements.nested_terms,
                };
                visit_trusted_owner_term_instrs(term, &mut finder);
            }
        }
    }

    tracing::debug!(
        target: "soac_builtin_consumer_planning",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        store_values = placements.store_values,
        effect_only_body = placements.effect_only_body,
        returns = placements.returns,
        nested_body = placements.nested_body,
        nested_terms = placements.nested_terms,
        "typed_builtin_generator_consumer_plan_placements",
    );
}

fn annotate_typed_generator_resume_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
) -> Result<usize, String> {
    if plans.is_empty() {
        return Ok(0);
    }
    let live_instr_ids = collect_typed_semantic_instr_ids(function);
    let expected = plans
        .keys()
        .filter(|instr_id| live_instr_ids.contains(instr_id))
        .count();

    struct Annotator<'a> {
        plans: &'a HashMap<InstrId, TypedGeneratorResumePlan>,
        used: HashSet<InstrId>,
        count: usize,
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && let Some(instr_id) = call.try_semantic_instr_id()
                && let Some(plan) = self.plans.get(&instr_id)
            {
                call.extra.set_generator_resume_plan(plan.clone());
                self.used.insert(instr_id);
                self.count += 1;
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        plans,
        used: HashSet::new(),
        count: 0,
    };
    annotator.visit_fn_mut(function);
    if annotator.used.len() != expected {
        let missing = plans
            .keys()
            .filter(|instr_id| live_instr_ids.contains(instr_id))
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "typed generator-resume plans were not attached to call nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

type StaticConstructorCalls = HashMap<RuntimeFunctionId, HashMap<InstrId, TypedAttrOwnerRef>>;

fn static_constructor_calls_for_module(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    targets: &StaticDirectCallTargets,
) -> StaticConstructorCalls {
    module
        .callable_defs
        .iter()
        .filter_map(|function| {
            let calls =
                static_constructor_call_owner_refs(function, &module.module_constants, targets);
            (!calls.is_empty()).then_some((function.function_id, calls))
        })
        .collect()
}

fn static_constructor_calls_for_external_callees(
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    targets: &StaticDirectCallTargets,
) -> StaticConstructorCalls {
    external_callees
        .values()
        .filter_map(|callee| {
            let calls = static_constructor_call_owner_refs(
                &callee.function,
                &callee.module_constants,
                targets,
            );
            (!calls.is_empty()).then_some((callee.function.function_id, calls))
        })
        .collect()
}

fn trusted_static_constructor_calls(
    static_constructor_calls: &StaticConstructorCalls,
) -> StaticConstructorCalls {
    static_constructor_calls
        .iter()
        .filter_map(|(function_id, calls)| {
            let trusted_calls = calls
                .iter()
                .filter(|(_, owner)| trusted_fully_virtual_constructor_owner(owner))
                .map(|(instr_id, owner)| (*instr_id, owner.clone()))
                .collect::<HashMap<_, _>>();
            (!trusted_calls.is_empty()).then_some((*function_id, trusted_calls))
        })
        .collect()
}

fn trusted_constructor_init_owner_refs(
    static_targets: &StaticDirectCallTargets,
) -> HashMap<RuntimeFunctionId, TypedAttrOwnerRef> {
    static_targets
        .runtime_names
        .values()
        .chain(
            static_targets
                .module_globals
                .values()
                .flat_map(|targets| targets.values()),
        )
        .filter_map(|target| {
            let owner_type_ref = target.constructor_owner_type_ref.as_ref()?;
            trusted_fully_virtual_constructor_owner(owner_type_ref).then_some(())?;
            let init_function_id =
                constructor_init_function_id_for_entry_function(&target.function)?;
            Some((init_function_id, owner_type_ref.clone()))
        })
        .collect()
}

fn method_guards_for_v3_direct_call(
    plan: &ResolvedV3DirectCallPlan,
    method_name: &str,
) -> Result<Vec<TypedDirectMethodCallGuard>, String> {
    let owners = unsafe { crate::lookup_exact_owner_types_for_method(plan.target, method_name) }
        .map_err(|_| {
            format!(
                "failed to resolve owner types for method {} target {}",
                method_name, plan.target
            )
        })?;
    let mut guards = Vec::with_capacity(owners.len());
    for owner in owners {
        let Some(owner_type_ref) = super::symbols::reloc_type_ref_for_type(owner.owner_type)?
        else {
            continue;
        };
        if !super::symbols::ensure_reloc_callable_symbol_registered(
            &super::symbols::RelocCallableRef::OwnerAttr {
                owner_type_ref: owner_type_ref.clone(),
                attr_name: method_name.to_string(),
            },
        )? {
            continue;
        }
        guards.push(TypedDirectMethodCallGuard {
            function_id: plan.target,
            owner_type_ref: super::symbols::typed_attr_owner_ref_from_reloc_type_ref(
                &owner_type_ref,
            ),
            type_version: owner.type_version,
            arg_plan: plan.arg_plan.clone(),
        });
    }
    Ok(guards)
}

fn insert_method_guards(
    emissions: &mut TypedCallEmissionPlans,
    source: InstrId,
    method_name: String,
    guards: Vec<TypedDirectMethodCallGuard>,
) -> Result<(), String> {
    if guards.is_empty() {
        return Ok(());
    }
    let plan = emissions
        .by_source
        .entry(source)
        .or_insert_with(|| TypedCallEmissionPlan::Method {
            method_name: method_name.clone(),
            method_guards: Vec::new(),
        });
    let TypedCallEmissionPlan::Method {
        method_name: existing_name,
        method_guards,
    } = plan
    else {
        return Err(format!(
            "method-call emission source {source:?} already has non-method plan"
        ));
    };
    if existing_name != &method_name {
        return Err(format!(
            "method-call emission source {source:?} has conflicting method names {existing_name:?} and {method_name:?}"
        ));
    }
    method_guards.extend(guards);
    Ok(())
}

fn insert_static_direct_callable_plan(
    emissions: &mut TypedCallEmissionPlans,
    source: InstrId,
    plans: &[ResolvedV3DirectCallPlan],
) -> Result<(), String> {
    let function_plans = plans
        .iter()
        .filter(|plan| matches!(plan.callee, DirectCallCallee::Function))
        .collect::<Vec<_>>();
    let [plan] = function_plans.as_slice() else {
        return Err(format!(
            "static runtime direct-call emission source {source:?} requires exactly one function plan"
        ));
    };
    emissions.by_source.insert(
        source,
        TypedCallEmissionPlan::DirectCallable {
            function_guard: TypedDirectFunctionCallGuard {
                function_id: plan.target,
                arg_plan: plan.arg_plan.clone(),
            },
        },
    );
    Ok(())
}

fn insert_runtime_protocol_method_guards(
    emissions: &mut TypedCallEmissionPlans,
    source: InstrId,
    runtime_name: RuntimeName,
    method_name: String,
    guards: Vec<TypedDirectMethodCallGuard>,
) -> Result<(), String> {
    if guards.is_empty() {
        return Ok(());
    }
    let plan = emissions.by_source.entry(source).or_insert_with(|| {
        TypedCallEmissionPlan::RuntimeProtocolMethod {
            runtime_name,
            method_name: method_name.clone(),
            method_guards: Vec::new(),
        }
    });
    let TypedCallEmissionPlan::RuntimeProtocolMethod {
        runtime_name: existing_runtime_name,
        method_name: existing_name,
        method_guards,
    } = plan
    else {
        return Err(format!(
            "runtime-protocol method emission source {source:?} already has non-protocol plan"
        ));
    };
    if *existing_runtime_name != runtime_name || existing_name != &method_name {
        return Err(format!(
            "runtime-protocol method emission source {source:?} has conflicting methods {existing_runtime_name:?}.{existing_name:?} and {runtime_name:?}.{method_name:?}"
        ));
    }
    method_guards.extend(guards);
    Ok(())
}

fn typed_call_emission_plans_for_function(
    profile: &SpecializationProfile<'_>,
    function_id: RuntimeFunctionId,
    static_direct_calls: Option<&HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
) -> Result<TypedCallEmissionPlans, String> {
    let opt_v3_direct_calls_by_instr = profile.typed_call_emission_direct_calls(function_id);
    let mut ordinary_direct_calls_by_instr =
        HashMap::<InstrId, Vec<ResolvedV3DirectCallPlan>>::new();
    let mut method_guards_by_instr =
        HashMap::<InstrId, HashMap<String, Vec<TypedDirectMethodCallGuard>>>::new();
    let mut runtime_protocol_method_guards_by_instr =
        HashMap::<InstrId, HashMap<(RuntimeName, String), Vec<TypedDirectMethodCallGuard>>>::new();
    for (source, plans) in opt_v3_direct_calls_by_instr {
        for plan in plans {
            match &plan.callee {
                DirectCallCallee::Function => {
                    ordinary_direct_calls_by_instr
                        .entry(source)
                        .or_default()
                        .push(plan);
                }
                DirectCallCallee::Method { method_name } => {
                    let method_guards = method_guards_for_v3_direct_call(&plan, method_name)?;
                    method_guards_by_instr
                        .entry(source)
                        .or_default()
                        .entry(method_name.clone())
                        .or_default()
                        .extend(method_guards);
                }
                DirectCallCallee::RuntimeProtocolMethod {
                    runtime_name,
                    method_name,
                } => {
                    if plan.body.kind != CallBodyKind::Inline {
                        continue;
                    }
                    let method_guards = method_guards_for_v3_direct_call(&plan, method_name)?;
                    runtime_protocol_method_guards_by_instr
                        .entry(source)
                        .or_default()
                        .entry((*runtime_name, method_name.clone()))
                        .or_default()
                        .extend(method_guards);
                }
            }
        }
    }
    let mut emissions = typed_call_emission_plans_from_v3(&ordinary_direct_calls_by_instr)?;
    if let Some(static_direct_calls) = static_direct_calls {
        for (source, plans) in static_direct_calls {
            insert_static_direct_callable_plan(&mut emissions, *source, plans)?;
        }
    }
    for (source, guards_by_method) in method_guards_by_instr {
        for (method_name, guards) in guards_by_method {
            insert_method_guards(&mut emissions, source, method_name, guards)?;
        }
    }
    for (source, guards_by_method) in runtime_protocol_method_guards_by_instr {
        for ((runtime_name, method_name), guards) in guards_by_method {
            insert_runtime_protocol_method_guards(
                &mut emissions,
                source,
                runtime_name,
                method_name,
                guards,
            )?;
        }
    }
    Ok(emissions)
}

fn merge_typed_call_emission_plans(
    target: &mut TypedCallEmissionPlans,
    incoming: &TypedCallEmissionPlans,
) -> Result<(), String> {
    for (source, plan) in &incoming.by_source {
        match target.by_source.entry(*source) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(plan.clone());
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                if entry.get() != plan {
                    return Err(format!(
                        "typed call emission source {source:?} maps to conflicting plans"
                    ));
                }
            }
        }
    }
    Ok(())
}

fn typed_call_emission_plans_for_function_with_remapped(
    profile: &SpecializationProfile<'_>,
    function_id: RuntimeFunctionId,
    static_direct_calls: &StaticTypedDirectCalls,
    remapped_call_emissions: &RemappedTypedCallEmissions,
) -> Result<TypedCallEmissionPlans, String> {
    let mut emissions = typed_call_emission_plans_for_function(
        profile,
        function_id,
        static_direct_calls.get(&function_id),
    )?;
    if let Some(remapped) = remapped_call_emissions.get(&function_id) {
        merge_typed_call_emission_plans(&mut emissions, remapped)?;
    }
    Ok(emissions)
}

pub(super) fn apply_profile_call_emission_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    apply_call_emission_plans_to_typed_function(function, profile, None)
}

fn apply_call_emission_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    static_direct_calls: Option<&HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
) -> Result<(), String> {
    let call_emissions =
        typed_call_emission_plans_for_function(profile, function.function_id, static_direct_calls)?;
    lower_typed_function_call_emission_plans(function, &call_emissions)?;
    Ok(())
}

fn typed_function_depends_on_suppressed_source_generator(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    static_targets: &StaticDirectCallTargets,
) -> bool {
    struct Finder<'a> {
        module_id: u32,
        static_targets: &'a StaticDirectCallTargets,
        found: bool,
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            if let InstrTyped::CallTyped(call) = expr
                && let InstrTyped::Load(load) = call.func.as_ref()
                && load.name.location.is_global()
                && let Some(target) = self
                    .static_targets
                    .module_global_generators
                    .get(&self.module_id)
                    .and_then(|targets| targets.get(load.name.id_str()))
                && self
                    .static_targets
                    .suppressed_source_generators
                    .contains(&target.function_id)
            {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    if static_targets.suppressed_source_generators.is_empty() {
        return false;
    }
    let mut finder = Finder {
        module_id: function.function_id.runtime_module_id().as_u32(),
        static_targets,
        found: false,
    };
    finder.visit_fn(function);
    finder.found
}

fn apply_call_emission_plans_to_typed_function_with_static_targets(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    static_direct_calls: Option<&HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>,
    static_targets: &StaticDirectCallTargets,
) -> Result<(), String> {
    let mut call_emissions =
        typed_call_emission_plans_for_function(profile, function.function_id, static_direct_calls)?;
    let depends_on_suppressed_source_generator =
        typed_function_depends_on_suppressed_source_generator(function, static_targets);
    if !static_targets.suppressed_source_generators.is_empty() {
        call_emissions.by_source.retain(|_, plan| match plan {
            TypedCallEmissionPlan::Callable { function_guards } => {
                function_guards.retain(|guard| {
                    !static_targets
                        .suppressed_source_generators
                        .contains(&guard.function_id)
                });
                !function_guards.is_empty()
            }
            TypedCallEmissionPlan::DirectCallable { function_guard } => !static_targets
                .suppressed_source_generators
                .contains(&function_guard.function_id),
            TypedCallEmissionPlan::Method { method_guards, .. } => {
                method_guards.retain(|guard| {
                    !static_targets
                        .suppressed_source_generators
                        .contains(&guard.function_id)
                });
                !method_guards.is_empty()
            }
            TypedCallEmissionPlan::RuntimeProtocolMethod {
                runtime_name,
                method_guards,
                ..
            } => {
                if depends_on_suppressed_source_generator
                    && matches!(runtime_name, RuntimeName::Iter | RuntimeName::Next)
                {
                    return false;
                }
                method_guards.retain(|guard| {
                    !static_targets
                        .suppressed_source_generators
                        .contains(&guard.function_id)
                });
                !method_guards.is_empty()
            }
        });
    }
    lower_typed_function_call_emission_plans(function, &call_emissions)?;
    Ok(())
}

pub(super) fn annotate_typed_attr_accesses(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    opt_v3_indexed_fields_by_instr: &HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    indexed_field_counter_sources: &HashMap<InstrId, TypedIndexedFieldCounterSource>,
    specialize_stores: bool,
) -> Result<usize, String> {
    struct Annotator<'a> {
        opt_v3_indexed_fields_by_instr: &'a HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
        indexed_field_counter_sources: &'a HashMap<InstrId, TypedIndexedFieldCounterSource>,
        specialize_stores: bool,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn opt_v3_guards_for_attr(&mut self, instr_id: InstrId) -> Option<TypedAttrAccessPlan> {
            let accesses = self.opt_v3_indexed_fields_by_instr.get(&instr_id)?;
            let mut guards = Vec::with_capacity(accesses.len());
            for access in accesses {
                guards.push(access.specialization.to_typed_guard());
            }
            Some(TypedAttrAccessPlan::IndexedField {
                source: TypedIndexedFieldPlanSource::OptimizationPlanV3,
                counter_source: self.indexed_field_counter_sources.get(&instr_id).copied(),
                guards,
            })
        }

        fn annotate_attr(
            &mut self,
            instr_id: InstrId,
            expected_access: PlanV3IndexedFieldAccessKind,
        ) -> Option<TypedAttrAccessPlan> {
            if self.opt_v3_indexed_fields_by_instr.contains_key(&instr_id) {
                for access in self.opt_v3_indexed_fields_by_instr.get(&instr_id)? {
                    if access.access != expected_access {
                        self.error = Some(format!(
                            "optimizer v3 indexed-field for {instr_id} was prevalidated as {:?}, but typed node requires {:?}",
                            access.access, expected_access
                        ));
                        return None;
                    }
                }
                return self.opt_v3_guards_for_attr(instr_id);
            }
            None
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::GetAttrTyped(op) => {
                    if let Some(access) = self
                        .annotate_attr(op.semantic_instr_id(), PlanV3IndexedFieldAccessKind::Load)
                    {
                        op.access = access;
                        self.count += 1;
                    }
                }
                InstrTyped::SetAttrTyped(op) if self.specialize_stores => {
                    if let Some(access) = self
                        .annotate_attr(op.semantic_instr_id(), PlanV3IndexedFieldAccessKind::Store)
                    {
                        op.access = access;
                        self.count += 1;
                    }
                }
                InstrTyped::SetAttrTyped(op)
                    if self
                        .opt_v3_indexed_fields_by_instr
                        .contains_key(&op.semantic_instr_id()) =>
                {
                    self.error = Some(format!(
                        "optimizer v3 indexed-field store emission for {} cannot be consumed because indexed stores are disabled",
                        op.semantic_instr_id()
                    ));
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        opt_v3_indexed_fields_by_instr,
        indexed_field_counter_sources,
        specialize_stores,
        count: 0,
        error: None,
    };
    for block in &mut function.blocks {
        for instr in &mut block.body {
            annotator.visit_instr_mut(instr);
        }
        annotator.visit_term_mut(&mut block.term);
        if let Some(error) = annotator.error.take() {
            return Err(error);
        }
    }
    Ok(annotator.count)
}

fn annotate_typed_indexed_field_accesses_from_profile(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    remapped_indexed_fields: Option<&HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>>,
    remapped_indexed_field_counter_sources: Option<
        &HashMap<InstrId, TypedIndexedFieldCounterSource>,
    >,
) -> Result<(), String> {
    let (_, _, mut opt_v3_indexed_fields_by_instr) =
        profile.field_index_specialization_maps(function.function_id)?;
    if let Some(remapped_indexed_fields) = remapped_indexed_fields {
        for (instr_id, accesses) in remapped_indexed_fields {
            let entry = opt_v3_indexed_fields_by_instr.entry(*instr_id).or_default();
            for access in accesses {
                if !entry.contains(access) {
                    entry.push(access.clone());
                }
            }
        }
    }
    if opt_v3_indexed_fields_by_instr.is_empty() {
        return Ok(());
    }
    let specialize_field_stores = profile.typed_specializations_embedded()
        || (profile.behavior_change_indexed_stores
            && function.scope.scope_kind != CallableScopeKind::Module);
    annotate_typed_attr_accesses(
        function,
        &opt_v3_indexed_fields_by_instr,
        remapped_indexed_field_counter_sources.unwrap_or(&HashMap::new()),
        specialize_field_stores,
    )?;
    Ok(())
}

fn typed_indexed_global_access_plan_from_opt_v3(
    plan: &OptV3IndexedGlobalAccessPlan,
) -> TypedIndexedGlobalAccessPlan {
    TypedIndexedGlobalAccessPlan {
        source: TypedIndexedGlobalPlanSource::OptimizationPlanV3,
        instr_id: plan.source,
        access: plan.access,
        module_name: plan.module_name.clone(),
        name: plan.name.clone(),
        expected_index: plan.expected_index,
        guard: plan.guard,
        fallback: plan.fallback,
    }
}

pub(super) fn annotate_typed_indexed_global_accesses(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    indexed_globals_by_instr: &HashMap<InstrId, OptV3IndexedGlobalAccessPlan>,
) -> Result<usize, String> {
    struct Annotator<'a> {
        indexed_globals_by_instr: &'a HashMap<InstrId, OptV3IndexedGlobalAccessPlan>,
        used: HashSet<InstrId>,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn plan_for_instr(
            &mut self,
            instr_id: InstrId,
            expected_access: PlanV3IndexedGlobalAccessKind,
            location_is_global: bool,
        ) -> Option<TypedIndexedGlobalAccessPlan> {
            let plan = self.indexed_globals_by_instr.get(&instr_id)?;
            if plan.access != expected_access {
                self.error = Some(format!(
                    "optimizer v3 indexed-global plan for {instr_id} expected {:?}, but typed node requires {:?}",
                    plan.access, expected_access
                ));
                return None;
            }
            if !location_is_global {
                self.error = Some(format!(
                    "optimizer v3 indexed-global plan for {instr_id} reached a non-global typed node"
                ));
                return None;
            }
            self.used.insert(instr_id);
            Some(typed_indexed_global_access_plan_from_opt_v3(plan))
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::Load(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) = self.plan_for_instr(
                            instr_id,
                            PlanV3IndexedGlobalAccessKind::Load,
                            op.name.location.is_global(),
                        )
                    {
                        op.extra_mut().set_indexed_global_access_plan(plan);
                        self.count += 1;
                    }
                }
                InstrTyped::Store(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) = self.plan_for_instr(
                            instr_id,
                            PlanV3IndexedGlobalAccessKind::Store,
                            op.name.location.is_global(),
                        )
                    {
                        op.extra_mut().set_indexed_global_access_plan(plan);
                        self.count += 1;
                    }
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        indexed_globals_by_instr,
        used: HashSet::new(),
        count: 0,
        error: None,
    };
    annotator.visit_fn_mut(function);
    if let Some(error) = annotator.error {
        return Err(error);
    }
    if annotator.used.len() != indexed_globals_by_instr.len() {
        let missing = indexed_globals_by_instr
            .keys()
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "optimizer v3 indexed-global plans were not attached to typed nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

fn annotate_typed_indexed_global_accesses_from_profile(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let Some(indexed_globals_by_instr) = profile
        .opt_v3_emitted_indexed_globals
        .get(&function.function_id)
    else {
        return Ok(());
    };
    let live_instr_ids = collect_typed_semantic_instr_ids(function);
    let indexed_globals_by_instr = indexed_globals_by_instr
        .iter()
        .filter(|(instr_id, _)| live_instr_ids.contains(instr_id))
        .map(|(instr_id, plan)| (*instr_id, plan.clone()))
        .collect::<HashMap<_, _>>();
    if indexed_globals_by_instr.is_empty() {
        return Ok(());
    }
    annotate_typed_indexed_global_accesses(function, &indexed_globals_by_instr)?;
    Ok(())
}

#[derive(Clone)]
struct ProfileExactListItemAccessPlan {
    plan: OptV3ExactListItemAccessPlan,
    counter_source: Option<TypedExactListItemCounterSource>,
}

fn typed_exact_list_item_access_plan_from_opt_v3(
    plan: &OptV3ExactListItemAccessPlan,
    counter_source: Option<TypedExactListItemCounterSource>,
) -> TypedExactListItemAccessPlan {
    TypedExactListItemAccessPlan {
        source: TypedExactListItemPlanSource::OptimizationPlanV3,
        instr_id: plan.source,
        counter_source,
        access: plan.access,
        shape: plan.shape,
        guard: plan.guard,
        fallback: plan.fallback,
    }
}

fn annotate_typed_exact_list_item_accesses(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    exact_list_items_by_instr: &HashMap<InstrId, ProfileExactListItemAccessPlan>,
) -> Result<usize, String> {
    struct Annotator<'a> {
        exact_list_items_by_instr: &'a HashMap<InstrId, ProfileExactListItemAccessPlan>,
        used: HashSet<InstrId>,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn plan_for_instr(
            &mut self,
            instr_id: InstrId,
            expected_access: ExactListItemAccessKind,
        ) -> Option<TypedExactListItemAccessPlan> {
            let plan = self.exact_list_items_by_instr.get(&instr_id)?;
            if plan.plan.access != expected_access {
                self.error = Some(format!(
                    "optimizer v3 exact-list item plan for {instr_id} expected {:?}, but typed node requires {:?}",
                    plan.plan.access, expected_access
                ));
                return None;
            }
            self.used.insert(instr_id);
            Some(typed_exact_list_item_access_plan_from_opt_v3(
                &plan.plan,
                plan.counter_source,
            ))
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            match expr {
                InstrTyped::GetItem(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) =
                            self.plan_for_instr(instr_id, ExactListItemAccessKind::Get)
                    {
                        op.extra_mut().set_exact_list_item_access_plan(plan);
                        self.count += 1;
                    }
                }
                InstrTyped::SetItem(op) => {
                    if let Some(instr_id) = op.try_semantic_instr_id()
                        && let Some(plan) =
                            self.plan_for_instr(instr_id, ExactListItemAccessKind::Set)
                    {
                        op.extra_mut().set_exact_list_item_access_plan(plan);
                        self.count += 1;
                    }
                }
                _ => {}
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        exact_list_items_by_instr,
        used: HashSet::new(),
        count: 0,
        error: None,
    };
    annotator.visit_fn_mut(function);
    if let Some(error) = annotator.error {
        return Err(error);
    }
    if annotator.used.len() != exact_list_items_by_instr.len() {
        let missing = exact_list_items_by_instr
            .keys()
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "optimizer v3 exact-list item plans were not attached to typed nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

fn annotate_typed_exact_list_item_accesses_from_profile(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    remapped_exact_list_items: Option<&HashMap<InstrId, ProfileExactListItemAccessPlan>>,
) -> Result<(), String> {
    let mut exact_list_items_by_instr = profile
        .opt_v3_emitted_exact_list_items
        .get(&function.function_id)
        .map(|plans| {
            plans
                .iter()
                .map(|(instr_id, plan)| {
                    (
                        *instr_id,
                        ProfileExactListItemAccessPlan {
                            plan: plan.clone(),
                            counter_source: None,
                        },
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    if let Some(remapped_exact_list_items) = remapped_exact_list_items {
        for (instr_id, plan) in remapped_exact_list_items {
            if exact_list_items_by_instr
                .insert(*instr_id, plan.clone())
                .is_some()
            {
                return Err(format!(
                    "remapped optimizer v3 exact-list item plan for {} collides with an existing caller plan",
                    instr_id
                ));
            }
        }
    }
    let live_instr_ids = collect_typed_semantic_instr_ids(function);
    exact_list_items_by_instr.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    if exact_list_items_by_instr.is_empty() {
        return Ok(());
    }
    annotate_typed_exact_list_item_accesses(function, &exact_list_items_by_instr)?;
    Ok(())
}

fn typed_exact_int_branch_plan_from_opt_v3(
    instr_id: InstrId,
    selection: OptV3ExactIntBranchSelection<'_>,
) -> TypedExactIntBranchPlan {
    TypedExactIntBranchPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: selection.hot_plan.clone(),
        hot_region: selection.hot_region.clone(),
        fallback_plan: selection.fallback_plan.clone(),
        fallback_region: selection.fallback_region.clone(),
    }
}

fn typed_exact_int_return_plan_from_opt_v3(
    instr_id: InstrId,
    selection: OptV3ExactIntReturnSelection<'_>,
) -> TypedExactIntReturnPlan {
    TypedExactIntReturnPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: selection.hot_plan.clone(),
        hot_region: selection.hot_region.clone(),
        fallback_plan: selection.fallback_plan.clone(),
        fallback_region: selection.fallback_region.clone(),
    }
}

fn seed_profile_exact_int_selections_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    remapped_branches: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_returns: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>,
) -> Result<usize, String> {
    let Some(artifacts) = profile
        .opt_v3_exact_int_branch_artifacts
        .get(&function.function_id)
    else {
        return Ok(0);
    };

    let mut count = 0;
    for instr_id in collect_typed_semantic_instr_ids(function) {
        if let Some(selection) = opt_v3_exact_int_branch_selection_for_source(artifacts, instr_id)?
        {
            let plan = typed_exact_int_branch_plan_from_opt_v3(instr_id, selection);
            let entry = remapped_branches
                .entry(function.function_id)
                .or_default()
                .entry(instr_id);
            match entry {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(plan);
                    count += 1;
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    if entry.get() != &plan {
                        return Err(format!(
                            "profile exact-int branch plan for instruction {} collides with an existing remapped plan",
                            instr_id
                        ));
                    }
                }
            }
        }
        if let Some(selection) = opt_v3_exact_int_return_selection_for_source(artifacts, instr_id)?
        {
            let plan = typed_exact_int_return_plan_from_opt_v3(instr_id, selection);
            let entry = remapped_returns
                .entry(function.function_id)
                .or_default()
                .entry(instr_id);
            match entry {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(plan);
                    count += 1;
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    if entry.get() != &plan {
                        return Err(format!(
                            "profile exact-int return plan for instruction {} collides with an existing remapped plan",
                            instr_id
                        ));
                    }
                }
            }
        }
    }
    Ok(count)
}

pub(super) fn annotate_typed_exact_int_selections(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    artifacts: &ExactIntBranchV3Artifacts,
) -> Result<usize, String> {
    struct Annotator<'a> {
        artifacts: &'a ExactIntBranchV3Artifacts,
        count: usize,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn attach_branch_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let selection =
                match opt_v3_exact_int_branch_selection_for_source(self.artifacts, instr_id) {
                    Ok(selection) => selection,
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                };
            let Some(selection) = selection else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "optimizer v3 exact-int branch plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            let plan = typed_exact_int_branch_plan_from_opt_v3(instr_id, selection);
            self.count += usize::from(extra.set_exact_int_branch_plan(plan));
        }

        fn attach_return_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let selection =
                match opt_v3_exact_int_return_selection_for_source(self.artifacts, instr_id) {
                    Ok(selection) => selection,
                    Err(error) => {
                        self.error = Some(error);
                        return;
                    }
                };
            let Some(selection) = selection else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "optimizer v3 exact-int return plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            let plan = typed_exact_int_return_plan_from_opt_v3(instr_id, selection);
            self.count += usize::from(extra.set_exact_int_return_plan(plan));
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            self.attach_return_plan(expr);
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        artifacts,
        count: 0,
        error: None,
    };
    for block in &mut function.blocks {
        if let BlockTerm::IfTerm(if_term) = &mut block.term {
            annotator.attach_branch_plan(&mut if_term.test);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        for instr in &mut block.body {
            annotator.visit_instr_mut(instr);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        annotator.visit_term_mut(&mut block.term);
        if let Some(error) = annotator.error.take() {
            return Err(error);
        }
    }
    Ok(annotator.count)
}

fn annotate_typed_exact_int_selections_from_profile(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    let Some(artifacts) = profile
        .opt_v3_exact_int_branch_artifacts
        .get(&function.function_id)
    else {
        return Ok(());
    };
    annotate_typed_exact_int_selections(function, artifacts)?;
    Ok(())
}

fn annotate_typed_remapped_exact_int_selections(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    branch_plans: Option<&HashMap<InstrId, TypedExactIntBranchPlan>>,
    return_plans: Option<&HashMap<InstrId, TypedExactIntReturnPlan>>,
) -> Result<(), String> {
    let empty_branch_plans = HashMap::new();
    let empty_return_plans = HashMap::new();
    let branch_plans = branch_plans.unwrap_or(&empty_branch_plans);
    let return_plans = return_plans.unwrap_or(&empty_return_plans);
    if branch_plans.is_empty() && return_plans.is_empty() {
        return Ok(());
    }

    struct Annotator<'a> {
        branch_plans: &'a HashMap<InstrId, TypedExactIntBranchPlan>,
        return_plans: &'a HashMap<InstrId, TypedExactIntReturnPlan>,
        used_branches: HashSet<InstrId>,
        used_returns: HashSet<InstrId>,
        error: Option<String>,
    }

    impl Annotator<'_> {
        fn attach_branch_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let Some(plan) = self.branch_plans.get(&instr_id).cloned() else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "inlined optimizer v3 exact-int branch plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            if let Some(existing) = extra.exact_int_branch_plan()
                && existing != &plan
            {
                self.error = Some(format!(
                    "inlined optimizer v3 exact-int branch plan for {instr_id} collides with an existing branch plan"
                ));
                return;
            }
            extra.set_exact_int_branch_plan(plan);
            self.used_branches.insert(instr_id);
        }

        fn attach_return_plan(&mut self, expr: &mut InstrTyped) {
            let Some(instr_id) = expr.try_semantic_instr_id() else {
                return;
            };
            let Some(plan) = self.return_plans.get(&instr_id).cloned() else {
                return;
            };
            let Some(extra) = expr.typed_extra_mut() else {
                self.error = Some(format!(
                    "inlined optimizer v3 exact-int return plan for {instr_id} reached a typed node without metadata"
                ));
                return;
            };
            if let Some(existing) = extra.exact_int_return_plan()
                && existing != &plan
            {
                self.error = Some(format!(
                    "inlined optimizer v3 exact-int return plan for {instr_id} collides with an existing return plan"
                ));
                return;
            }
            extra.set_exact_int_return_plan(plan);
            self.used_returns.insert(instr_id);
        }
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if self.error.is_some() {
                return;
            }
            self.attach_return_plan(expr);
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        branch_plans,
        return_plans,
        used_branches: HashSet::new(),
        used_returns: HashSet::new(),
        error: None,
    };
    for block in &mut function.blocks {
        if let BlockTerm::IfTerm(if_term) = &mut block.term {
            annotator.attach_branch_plan(&mut if_term.test);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        for instr in &mut block.body {
            annotator.visit_instr_mut(instr);
            if let Some(error) = annotator.error.take() {
                return Err(error);
            }
        }
        annotator.visit_term_mut(&mut block.term);
        if let Some(error) = annotator.error.take() {
            return Err(error);
        }
    }
    if annotator.used_branches.len() != branch_plans.len() {
        let missing = branch_plans
            .keys()
            .filter(|instr_id| !annotator.used_branches.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "inlined optimizer v3 exact-int branch plans were not attached to typed nodes: {missing}"
        ));
    }
    if annotator.used_returns.len() != return_plans.len() {
        let missing = return_plans
            .keys()
            .filter(|instr_id| !annotator.used_returns.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "inlined optimizer v3 exact-int return plans were not attached to typed nodes: {missing}"
        ));
    }
    Ok(())
}

fn apply_profile_access_and_scalar_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    remapped_indexed_fields: Option<&HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>>,
    remapped_indexed_field_counter_sources: Option<
        &HashMap<InstrId, TypedIndexedFieldCounterSource>,
    >,
    remapped_exact_list_items: Option<&HashMap<InstrId, ProfileExactListItemAccessPlan>>,
    remapped_exact_int_branches: Option<&HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_exact_int_returns: Option<&HashMap<InstrId, TypedExactIntReturnPlan>>,
    constructor_init_plans: Option<&HashMap<InstrId, TypedConstructorInitPlan>>,
) -> Result<(), String> {
    annotate_typed_indexed_field_accesses_from_profile(
        function,
        profile,
        remapped_indexed_fields,
        remapped_indexed_field_counter_sources,
    )?;
    annotate_typed_indexed_global_accesses_from_profile(function, profile)?;
    annotate_typed_exact_list_item_accesses_from_profile(
        function,
        profile,
        remapped_exact_list_items,
    )?;
    annotate_typed_exact_int_selections_from_profile(function, profile)?;
    annotate_typed_remapped_exact_int_selections(
        function,
        remapped_exact_int_branches,
        remapped_exact_int_returns,
    )?;
    annotate_typed_constructor_init_plans(function, constructor_init_plans)?;
    Ok(())
}

fn annotate_typed_constructor_init_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    constructor_init_plans: Option<&HashMap<InstrId, TypedConstructorInitPlan>>,
) -> Result<usize, String> {
    let Some(constructor_init_plans) = constructor_init_plans else {
        return Ok(0);
    };
    if constructor_init_plans.is_empty() {
        return Ok(0);
    }
    let live_instr_ids = collect_typed_semantic_instr_ids(function);
    let expected = constructor_init_plans
        .keys()
        .filter(|instr_id| live_instr_ids.contains(instr_id))
        .count();

    struct Annotator<'a> {
        constructor_init_plans: &'a HashMap<InstrId, TypedConstructorInitPlan>,
        used: HashSet<InstrId>,
        count: usize,
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if matches!(
                expr,
                InstrTyped::CallTyped(_) | InstrTyped::DirectCallableCallTyped(_)
            ) && let Some(instr_id) = expr.try_semantic_instr_id()
                && let Some(plan) = self.constructor_init_plans.get(&instr_id)
            {
                expr.typed_extra_mut()
                    .expect("typed constructor call should have typed metadata")
                    .set_constructor_init_plan(*plan);
                self.used.insert(instr_id);
                self.count += 1;
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator {
        constructor_init_plans,
        used: HashSet::new(),
        count: 0,
    };
    annotator.visit_fn_mut(function);
    if annotator.used.len() != expected {
        let missing = constructor_init_plans
            .keys()
            .filter(|instr_id| live_instr_ids.contains(instr_id))
            .filter(|instr_id| !annotator.used.contains(instr_id))
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "inlined constructor-init plans were not attached to typed call nodes: {missing}"
        ));
    }
    Ok(annotator.count)
}

pub(super) fn apply_profile_typed_block_metadata_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) -> Result<(), String> {
    annotate_typed_profiled_cold_blocks(function, profile)?;
    Ok(())
}

pub(super) fn apply_profile_typed_guard_miss_policy_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
) {
    let enabled =
        profile.guard_miss_deopt && function.scope.scope_kind != CallableScopeKind::Module;
    if !enabled {
        return;
    }

    struct Annotator;

    impl VisitMut<InstrTyped> for Annotator {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if expr.try_semantic_instr_id().is_some()
                && let Some(extra) = expr.typed_extra_mut()
            {
                extra.set_guard_miss_deopt_enabled(true);
            }
            expr.visit_children_mut(self);
        }
    }

    let mut annotator = Annotator;
    annotator.visit_fn_mut(function);
}

#[derive(Clone, Default)]
struct TypedInlineExactIntRemapContext {
    instr_ids: HashMap<InstrId, InstrId>,
    constant_indices: HashMap<u32, u32>,
    local_names: HashMap<String, String>,
}

fn typed_inline_exact_int_remap_contexts(
    instr_mappings: &[TypedInlineInstrIdMapping],
    constant_mappings: &[TypedInlineConstantMapping],
    local_mappings: &[TypedInlineLocalMapping],
) -> Result<HashMap<(RuntimeFunctionId, u32), TypedInlineExactIntRemapContext>, String> {
    let mut contexts = HashMap::<(RuntimeFunctionId, u32), TypedInlineExactIntRemapContext>::new();
    for mapping in instr_mappings {
        let context = contexts
            .entry((mapping.callee, mapping.inline_instance))
            .or_default();
        context
            .instr_ids
            .entry(mapping.callee_instr_id)
            .or_insert(mapping.caller_instr_id);
    }
    for mapping in constant_mappings {
        let context = contexts
            .entry((mapping.callee, mapping.inline_instance))
            .or_default();
        if let Some(existing) = context
            .constant_indices
            .insert(mapping.callee_index, mapping.caller_index)
            && existing != mapping.caller_index
        {
            return Err(format!(
                "typed inline instance {} for callee {} maps module constant {} to both {} and {}",
                mapping.inline_instance,
                mapping.callee,
                mapping.callee_index,
                existing,
                mapping.caller_index
            ));
        }
    }
    for mapping in local_mappings {
        let context = contexts
            .entry((mapping.callee, mapping.inline_instance))
            .or_default();
        if let Some(existing) = context
            .local_names
            .insert(mapping.callee_name.clone(), mapping.caller_name.clone())
            && existing != mapping.caller_name
        {
            return Err(format!(
                "typed inline instance {} for callee {} maps local {:?} to both {:?} and {:?}",
                mapping.inline_instance,
                mapping.callee,
                mapping.callee_name,
                existing,
                mapping.caller_name
            ));
        }
    }
    Ok(contexts)
}

fn remapped_typed_inline_instr_id(
    source: InstrId,
    context: &TypedInlineExactIntRemapContext,
    label: &str,
) -> Result<InstrId, String> {
    context.instr_ids.get(&source).copied().ok_or_else(|| {
        format!(
            "inlined optimizer v3 exact-int {label} references unmapped callee instruction {source}"
        )
    })
}

fn remap_optional_typed_inline_instr_id(
    source: &mut Option<InstrId>,
    context: &TypedInlineExactIntRemapContext,
    label: &str,
) -> Result<(), String> {
    let Some(original) = *source else {
        return Ok(());
    };
    *source = Some(remapped_typed_inline_instr_id(original, context, label)?);
    Ok(())
}

fn remap_exact_int_region_plan(
    region: &RegionPlan,
    context: &TypedInlineExactIntRemapContext,
) -> Result<RegionPlan, String> {
    let mut remapped = region.clone();
    if let RegionSource::Instr { instr_id } = &mut remapped.source {
        *instr_id = remapped_typed_inline_instr_id(*instr_id, context, "region source")?;
    }
    for input in &mut remapped.inputs {
        match &mut input.source {
            RegionInputSource::FunctionParam {
                name: Some(name), ..
            } => {
                let Some(mapped_name) = context.local_names.get(name.as_str()) else {
                    return Err(format!(
                        "inlined optimizer v3 exact-int region input references unmapped callee local {name:?}"
                    ));
                };
                *name = mapped_name.clone();
            }
            RegionInputSource::FunctionParam { name: None, .. } => {
                return Err(
                    "inlined optimizer v3 exact-int region input has unnamed local source"
                        .to_string(),
                );
            }
            RegionInputSource::IndexedGlobal { source, .. } => {
                *source = remapped_typed_inline_instr_id(*source, context, "indexed-global input")?;
            }
            RegionInputSource::IndexedField {
                source, receiver, ..
            } => {
                *source = remapped_typed_inline_instr_id(*source, context, "indexed-field input")?;
                match receiver {
                    IndexedFieldReceiverSource::LocalName { name } => {
                        let Some(mapped_name) = context.local_names.get(name.as_str()) else {
                            return Err(format!(
                                "inlined optimizer v3 exact-int indexed-field input references unmapped callee receiver {name:?}"
                            ));
                        };
                        *name = mapped_name.clone();
                    }
                }
            }
            RegionInputSource::ModuleConstant { index } => {
                if let Some(mapped_index) = context.constant_indices.get(index).copied() {
                    *index = mapped_index;
                }
            }
            RegionInputSource::CapturedValue { .. } | RegionInputSource::Synthetic { .. } => {}
        }
    }
    for exit in &mut remapped.exits {
        remap_optional_typed_inline_instr_id(&mut exit.source, context, "region exit")?;
    }
    Ok(remapped)
}

fn remap_exact_int_mechanical_region(
    region: &MechanicalRegionEmission,
    context: &TypedInlineExactIntRemapContext,
) -> Result<MechanicalRegionEmission, String> {
    let mut remapped = region.clone();
    for step in &mut remapped.steps {
        remap_optional_typed_inline_instr_id(&mut step.source, context, "mechanical step")?;
    }
    for exit in &mut remapped.exits {
        remap_optional_typed_inline_instr_id(&mut exit.source, context, "mechanical exit")?;
    }
    Ok(remapped)
}

fn remap_typed_exact_int_branch_plan(
    instr_id: InstrId,
    selection: OptV3ExactIntBranchSelection<'_>,
    context: &TypedInlineExactIntRemapContext,
) -> Result<TypedExactIntBranchPlan, String> {
    Ok(TypedExactIntBranchPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: remap_exact_int_region_plan(selection.hot_plan, context)?,
        hot_region: remap_exact_int_mechanical_region(selection.hot_region, context)?,
        fallback_plan: remap_exact_int_region_plan(selection.fallback_plan, context)?,
        fallback_region: remap_exact_int_mechanical_region(selection.fallback_region, context)?,
    })
}

fn remap_typed_exact_int_return_plan(
    instr_id: InstrId,
    selection: OptV3ExactIntReturnSelection<'_>,
    context: &TypedInlineExactIntRemapContext,
) -> Result<TypedExactIntReturnPlan, String> {
    Ok(TypedExactIntReturnPlan {
        source: TypedExactIntPlanSource::OptimizationPlanV3,
        instr_id,
        hot_plan: remap_exact_int_region_plan(selection.hot_plan, context)?,
        hot_region: remap_exact_int_mechanical_region(selection.hot_region, context)?,
        fallback_plan: remap_exact_int_region_plan(selection.fallback_plan, context)?,
        fallback_region: remap_exact_int_mechanical_region(selection.fallback_region, context)?,
    })
}

fn remap_inlined_exact_int_selections(
    caller_function_id: RuntimeFunctionId,
    instr_mappings: &[TypedInlineInstrIdMapping],
    constant_mappings: &[TypedInlineConstantMapping],
    local_mappings: &[TypedInlineLocalMapping],
    profile: &SpecializationProfile<'_>,
    remapped_branches: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_returns: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>,
) -> Result<usize, String> {
    let contexts =
        typed_inline_exact_int_remap_contexts(instr_mappings, constant_mappings, local_mappings)?;
    let mut count = 0;
    for mapping in instr_mappings {
        let Some(artifacts) = profile
            .opt_v3_exact_int_branch_artifacts
            .get(&mapping.callee)
        else {
            continue;
        };
        let context = contexts
            .get(&(mapping.callee, mapping.inline_instance))
            .ok_or_else(|| {
                format!(
                    "typed inline instance {} for callee {} has instruction mappings but no remap context",
                    mapping.inline_instance, mapping.callee
                )
            })?;
        let mut context = context.clone();
        context
            .instr_ids
            .insert(mapping.callee_instr_id, mapping.caller_instr_id);
        if let Some(selection) =
            opt_v3_exact_int_branch_selection_for_source(artifacts, mapping.callee_instr_id)?
        {
            let plan =
                remap_typed_exact_int_branch_plan(mapping.caller_instr_id, selection, &context)?;
            if remapped_branches
                .entry(caller_function_id)
                .or_default()
                .insert(mapping.caller_instr_id, plan)
                .is_some()
            {
                return Err(format!(
                    "inlined exact-int branch plan for callee {} instruction {} collides at caller instruction {}",
                    mapping.callee, mapping.callee_instr_id, mapping.caller_instr_id
                ));
            }
            count += 1;
        }
        if let Some(selection) =
            opt_v3_exact_int_return_selection_for_source(artifacts, mapping.callee_instr_id)?
        {
            let plan =
                remap_typed_exact_int_return_plan(mapping.caller_instr_id, selection, &context)?;
            if remapped_returns
                .entry(caller_function_id)
                .or_default()
                .insert(mapping.caller_instr_id, plan)
                .is_some()
            {
                return Err(format!(
                    "inlined exact-int return plan for callee {} instruction {} collides at caller instruction {}",
                    mapping.callee, mapping.callee_instr_id, mapping.caller_instr_id
                ));
            }
            count += 1;
        }
    }
    Ok(count)
}

fn remap_inlined_indexed_field_accesses(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    remapped_indexed_fields: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    >,
    remapped_indexed_field_counter_sources: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedIndexedFieldCounterSource>,
    >,
) -> Result<usize, String> {
    let mut resolved_by_callee =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>>::new();
    let mut count = 0;
    for mapping in mappings {
        let Some(callee_fields) = profile.opt_v3_emitted_indexed_fields.get(&mapping.callee) else {
            continue;
        };
        if !callee_fields.contains_key(&mapping.callee_instr_id) {
            continue;
        }
        if let std::collections::hash_map::Entry::Vacant(entry) =
            resolved_by_callee.entry(mapping.callee)
        {
            let (_, _, resolved) = profile.field_index_specialization_maps(mapping.callee)?;
            entry.insert(resolved);
        }
        let Some(accesses) = resolved_by_callee
            .get(&mapping.callee)
            .and_then(|fields| fields.get(&mapping.callee_instr_id))
        else {
            continue;
        };
        let entry = remapped_indexed_fields
            .entry(caller_function_id)
            .or_default()
            .entry(mapping.caller_instr_id)
            .or_default();
        for access in accesses {
            if !entry.contains(access) {
                entry.push(access.clone());
                count += 1;
            }
        }
        let source = TypedIndexedFieldCounterSource {
            function_id: mapping.callee,
            instr_id: mapping.callee_instr_id,
        };
        let counter_source = remapped_indexed_field_counter_sources
            .entry(caller_function_id)
            .or_default()
            .entry(mapping.caller_instr_id)
            .or_insert(source);
        if *counter_source != source {
            return Err(format!(
                "inlined indexed-field counter source for caller instruction {} maps to both {}:{} and {}:{}",
                mapping.caller_instr_id,
                counter_source.function_id,
                counter_source.instr_id,
                source.function_id,
                source.instr_id
            ));
        }
    }
    Ok(count)
}

fn remap_inlined_exact_list_item_accesses(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    remapped_exact_list_items: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, ProfileExactListItemAccessPlan>,
    >,
) -> Result<usize, String> {
    let mut count = 0;
    for mapping in mappings {
        let Some(callee_items) = profile.opt_v3_emitted_exact_list_items.get(&mapping.callee)
        else {
            continue;
        };
        let Some(plan) = callee_items.get(&mapping.callee_instr_id) else {
            continue;
        };
        let mut remapped = plan.clone();
        remapped.source = mapping.caller_instr_id;
        let remapped = ProfileExactListItemAccessPlan {
            plan: remapped,
            counter_source: Some(TypedExactListItemCounterSource {
                function_id: mapping.callee,
                instr_id: mapping.callee_instr_id,
            }),
        };
        if remapped_exact_list_items
            .entry(caller_function_id)
            .or_default()
            .insert(mapping.caller_instr_id, remapped)
            .is_some()
        {
            return Err(format!(
                "inlined exact-list item plan for callee {} instruction {} collides at caller instruction {}",
                mapping.callee, mapping.callee_instr_id, mapping.caller_instr_id
            ));
        }
        count += 1;
    }
    Ok(count)
}

fn remap_inlined_generator_instance_plans(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    remapped_generator_instance_plans: &mut RemappedTypedGeneratorInstancePlans,
) -> Result<usize, String> {
    let mut plans_by_callee =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedGeneratorInstancePlan>>::new();
    let mut count = 0;
    for mapping in mappings {
        let callee_plans = plans_by_callee.entry(mapping.callee).or_insert_with(|| {
            callee_module
                .callable_defs
                .iter()
                .find(|function| function.function_id == mapping.callee)
                .or_else(|| {
                    external_callees
                        .get(&mapping.callee)
                        .map(|callee| &callee.function)
                })
                .map(typed_generator_instance_plans_by_origin)
                .unwrap_or_default()
        });
        let Some(plan) = callee_plans.get(&mapping.callee_instr_id) else {
            continue;
        };
        let entry = remapped_generator_instance_plans
            .entry(caller_function_id)
            .or_default()
            .entry(mapping.caller_instr_id);
        match entry {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(plan.clone());
                count += 1;
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                if entry.get() != plan {
                    return Err(format!(
                        "inlined generator-instance plan for callee {} instruction {} collides at caller instruction {}",
                        mapping.callee, mapping.callee_instr_id, mapping.caller_instr_id
                    ));
                }
            }
        }
    }
    Ok(count)
}

fn profile_exact_list_item_accesses_for_function(
    function_id: RuntimeFunctionId,
    profile: &SpecializationProfile<'_>,
    remapped_exact_list_items: &HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, ProfileExactListItemAccessPlan>,
    >,
) -> HashMap<InstrId, ProfileExactListItemAccessPlan> {
    let mut exact_list_items = profile
        .opt_v3_emitted_exact_list_items
        .get(&function_id)
        .map(|plans| {
            plans
                .iter()
                .map(|(instr_id, plan)| {
                    (
                        *instr_id,
                        ProfileExactListItemAccessPlan {
                            plan: plan.clone(),
                            counter_source: None,
                        },
                    )
                })
                .collect::<HashMap<_, _>>()
        })
        .unwrap_or_default();
    if let Some(remapped) = remapped_exact_list_items.get(&function_id) {
        for (instr_id, plan) in remapped {
            exact_list_items
                .entry(*instr_id)
                .or_insert_with(|| plan.clone());
        }
    }
    exact_list_items
}

fn remap_cloned_exact_list_item_accesses(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    remapped_exact_list_items: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, ProfileExactListItemAccessPlan>,
    >,
) -> Result<usize, String> {
    let source_items = profile_exact_list_item_accesses_for_function(
        caller_function_id,
        profile,
        remapped_exact_list_items,
    );
    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let Some(plan) = source_items.get(&mapping.callee_instr_id) else {
            continue;
        };
        let mut remapped = plan.clone();
        remapped.plan.source = mapping.caller_instr_id;
        remapped
            .counter_source
            .get_or_insert_with(|| TypedExactListItemCounterSource {
                function_id: mapping.callee,
                instr_id: mapping.callee_instr_id,
            });
        if remapped_exact_list_items
            .entry(caller_function_id)
            .or_default()
            .contains_key(&mapping.caller_instr_id)
        {
            continue;
        }
        remapped_exact_list_items
            .entry(caller_function_id)
            .or_default()
            .insert(mapping.caller_instr_id, remapped);
        count += 1;
    }
    Ok(count)
}

fn merge_typed_inline_targets(targets: &mut TypedInlineTargets, incoming: &TypedInlineTargets) {
    for (source, plans) in incoming {
        let entry = targets.entry(*source).or_default();
        for plan in plans {
            if !entry.contains(plan) {
                entry.push(plan.clone());
            }
        }
    }
}

fn typed_inline_targets_for_function(
    function_id: RuntimeFunctionId,
    profile: &SpecializationProfile<'_>,
    static_direct_calls: &StaticTypedDirectCalls,
    remapped_inline_targets: &HashMap<RuntimeFunctionId, TypedInlineTargets>,
    suppressed_inline_targets: &SuppressedTypedInlineTargets,
) -> TypedInlineTargets {
    let mut targets = profile.typed_inline_direct_calls(function_id);
    let static_targets = static_inline_targets_for_function(
        function_id,
        static_direct_calls,
        remapped_inline_targets,
        suppressed_inline_targets,
    );
    merge_typed_inline_targets(&mut targets, &static_targets);
    targets
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TypedInlineTargetPriority {
    GeneratorResume,
    RuntimeProtocol,
    BuiltinImplementation,
    Other,
}

fn select_typed_inline_targets_within_cfg_budget(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    generator_resume_plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
    targets: TypedInlineTargets,
) -> TypedInlineTargets {
    select_typed_inline_targets_within_cfg_budget_and_priorities(
        function,
        module,
        external_callees,
        generator_resume_plans,
        &HashSet::new(),
        &HashSet::new(),
        targets,
        None,
    )
}

fn typed_builtin_generator_followup_cfg_budget(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    source: InstrId,
) -> (usize, usize) {
    struct GeneratorFollowupFinder {
        source: InstrId,
        resume_function_id: Option<RuntimeFunctionId>,
    }

    impl GeneratorFollowupFinder {
        fn maybe_record(
            &mut self,
            instr_id: Option<InstrId>,
            args: &[CallArgPositional<InstrTyped>],
        ) {
            if instr_id != Some(self.source) {
                return;
            }
            let [CallArgPositional::Positional(generator)] = args else {
                return;
            };
            self.resume_function_id = generator
                .generator_instance_plan()
                .map(|plan| plan.function_id);
        }
    }

    impl Visit<InstrTyped> for GeneratorFollowupFinder {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            match expr {
                InstrTyped::CallTyped(call) => {
                    self.maybe_record(call.try_semantic_instr_id(), &call.args);
                }
                InstrTyped::GuardedCallableCallTyped(call) => {
                    self.maybe_record(call.try_semantic_instr_id(), &call.args);
                }
                InstrTyped::DirectCallableCallTyped(call) => {
                    self.maybe_record(call.try_semantic_instr_id(), &call.args);
                }
                _ => {}
            }
            expr.visit_children(self);
        }
    }

    let mut finder = GeneratorFollowupFinder {
        source,
        resume_function_id: None,
    };
    finder.visit_fn(function);
    let Some(resume_function_id) = finder.resume_function_id else {
        return (0, 0);
    };
    let Some(resume_function) = module
        .callable_defs
        .iter()
        .find(|callee| callee.function_id == resume_function_id)
        .or_else(|| {
            external_callees
                .get(&resume_function_id)
                .map(|callee| &callee.function)
        })
    else {
        return (0, 0);
    };

    let mut reserved_blocks = resume_function.blocks.len().saturating_add(2);
    let mut reserved_body_instrs =
        typed_inline_function_body_instr_count(resume_function).saturating_add(8);
    let protocol_bridge = module
        .callable_defs
        .iter()
        .find(|callee| callee.names.qualname == "ClosureGenerator.send")
        .or_else(|| {
            external_callees
                .values()
                .map(|callee| &callee.function)
                .find(|callee| callee.names.qualname == "ClosureGenerator.send")
        });
    if let Some(protocol_bridge) = protocol_bridge {
        reserved_blocks = reserved_blocks
            .saturating_add(protocol_bridge.blocks.len())
            .saturating_add(2);
        reserved_body_instrs = reserved_body_instrs
            .saturating_add(typed_inline_function_body_instr_count(protocol_bridge))
            .saturating_add(8);
    }
    (reserved_blocks, reserved_body_instrs)
}

#[allow(clippy::too_many_arguments)]
fn select_typed_inline_targets_within_cfg_budget_and_priorities(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    generator_resume_plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
    trusted_runtime_protocol_sources: &HashSet<InstrId>,
    builtin_implementation_sources: &HashSet<InstrId>,
    targets: TypedInlineTargets,
    max_builtin_sources: Option<usize>,
) -> TypedInlineTargets {
    let mut candidates = targets.into_iter().collect::<Vec<_>>();
    candidates.sort_by_key(|(source, _)| {
        let priority = if generator_resume_plans.contains_key(source) {
            TypedInlineTargetPriority::GeneratorResume
        } else if trusted_runtime_protocol_sources.contains(source) {
            TypedInlineTargetPriority::RuntimeProtocol
        } else if builtin_implementation_sources.contains(source) {
            TypedInlineTargetPriority::BuiltinImplementation
        } else {
            TypedInlineTargetPriority::Other
        };
        (priority, source.index())
    });

    let mut selected = TypedInlineTargets::new();
    let mut selected_builtin_sources = 0usize;
    let mut projected_blocks = function.blocks.len();
    let mut projected_body_instrs = typed_inline_function_body_instr_count(function);
    let single_source_fixpoint =
        max_builtin_sources.is_some() && !builtin_implementation_sources.is_empty();

    for (source, plans) in candidates {
        let is_builtin_implementation = builtin_implementation_sources.contains(&source);
        if is_builtin_implementation
            && max_builtin_sources.is_some_and(|limit| selected_builtin_sources >= limit)
        {
            continue;
        }
        let mut additional_blocks = 2usize;
        let mut additional_body_instrs = 8usize;
        let mut targets_are_available = !plans.is_empty();

        for (target, arg_plan) in &plans {
            let Some(callee) = module
                .callable_defs
                .iter()
                .find(|callee| callee.function_id == *target)
                .or_else(|| external_callees.get(target).map(|callee| &callee.function))
            else {
                targets_are_available = false;
                break;
            };

            additional_blocks = additional_blocks.saturating_add(callee.blocks.len() + 1);
            additional_body_instrs = additional_body_instrs
                .saturating_add(typed_inline_function_body_instr_count(callee))
                .saturating_add(arg_plan.sources.len())
                .saturating_add(8);
        }

        let next_blocks = projected_blocks.saturating_add(additional_blocks);
        let next_body_instrs = projected_body_instrs.saturating_add(additional_body_instrs);
        let (reserved_followup_blocks, reserved_followup_body_instrs) = if is_builtin_implementation
        {
            typed_builtin_generator_followup_cfg_budget(function, module, external_callees, source)
        } else {
            (0, 0)
        };
        if !targets_are_available
            || next_blocks.saturating_add(reserved_followup_blocks)
                >= MAX_TYPED_INLINE_FUNCTION_BLOCKS
            || next_body_instrs.saturating_add(reserved_followup_body_instrs)
                >= MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS
        {
            tracing::debug!(
                target: "soac_inline_budget",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                source = ?source,
                projected_blocks = next_blocks,
                projected_body_instrs = next_body_instrs,
                reserved_followup_blocks,
                reserved_followup_body_instrs,
                max_blocks = MAX_TYPED_INLINE_FUNCTION_BLOCKS,
                max_body_instrs = MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS,
                "typed_inline_target_rejected_by_function_cfg_budget",
            );
            continue;
        }

        projected_blocks = next_blocks;
        projected_body_instrs = next_body_instrs;
        selected_builtin_sources += usize::from(is_builtin_implementation);
        selected.insert(source, plans);
        if single_source_fixpoint {
            break;
        }
    }

    selected
}

fn typed_inline_target_is_small_enough_with_limits(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    target: RuntimeFunctionId,
    max_blocks: usize,
    max_body_instrs: usize,
) -> bool {
    let function = module
        .callable_defs
        .iter()
        .find(|function| function.function_id == target)
        .or_else(|| external_callees.get(&target).map(|callee| &callee.function));
    let Some(function) = function else {
        return false;
    };
    struct TypedInstrCounter {
        count: usize,
    }

    impl Visit<InstrTyped> for TypedInstrCounter {
        fn visit_instr(&mut self, expr: &InstrTyped)
        where
            InstrTyped: ChildVisitable<InstrTyped>,
        {
            self.count += 1;
            expr.visit_children(self);
        }
    }

    let mut counter = TypedInstrCounter { count: 0 };
    counter.visit_fn(function);
    let block_count = function.blocks.len();
    let body_instr_count = counter.count;
    let within_budget = block_count <= max_blocks && body_instr_count <= max_body_instrs;
    if !within_budget {
        tracing::debug!(
            target: "soac_inline_budget",
            target_function = ?target,
            function_qualname = %function.names.qualname,
            block_count,
            body_instr_count,
            max_blocks,
            max_body_instrs,
            "typed_inline_target_rejected_by_budget",
        );
    }
    within_budget
}

fn typed_inline_target_is_small_enough_to_propagate(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    target: RuntimeFunctionId,
) -> bool {
    typed_inline_target_is_small_enough_with_limits(
        module,
        external_callees,
        target,
        MAX_TRANSITIVE_PROFILE_INLINE_BLOCKS,
        MAX_TRANSITIVE_PROFILE_INLINE_BODY_INSTRS,
    )
}

fn typed_generator_resume_inline_target_is_small_enough(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    target: RuntimeFunctionId,
) -> bool {
    typed_inline_target_is_small_enough_with_limits(
        module,
        external_callees,
        target,
        MAX_GENERATOR_RESUME_INLINE_BLOCKS,
        MAX_GENERATOR_RESUME_INLINE_BODY_INSTRS,
    )
}

fn typed_generator_protocol_bridge_inline_target_is_small_enough(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    target: RuntimeFunctionId,
) -> bool {
    typed_inline_target_is_small_enough_with_limits(
        module,
        external_callees,
        target,
        MAX_GENERATOR_PROTOCOL_BRIDGE_INLINE_BLOCKS,
        MAX_GENERATOR_PROTOCOL_BRIDGE_INLINE_BODY_INSTRS,
    )
}

fn trusted_generator_protocol_bridge_targets(
    static_targets: &StaticDirectCallTargets,
) -> HashSet<RuntimeFunctionId> {
    static_targets
        .strict_methods
        .iter()
        .filter_map(|((module_name, qualname, method_name), function)| {
            (module_name == "soac.runtime"
                && qualname == "ClosureGenerator"
                && method_name == "send")
                .then_some(function.function_id)
        })
        .collect()
}

fn transitive_profile_inline_targets_for_function(
    function_id: RuntimeFunctionId,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    trusted_generator_bridge_targets: &HashSet<RuntimeFunctionId>,
    profile: &SpecializationProfile<'_>,
) -> TypedInlineTargets {
    profile
        .typed_inline_direct_calls(function_id)
        .into_iter()
        .filter_map(|(source, plans)| {
            let plans = plans
                .into_iter()
                .filter(|(target, _)| {
                    typed_inline_target_is_small_enough_to_propagate(
                        module,
                        external_callees,
                        *target,
                    ) || (trusted_generator_bridge_targets.contains(target)
                        && typed_generator_protocol_bridge_inline_target_is_small_enough(
                            module,
                            external_callees,
                            *target,
                        ))
                })
                .collect::<Vec<_>>();
            (!plans.is_empty()).then_some((source, plans))
        })
        .collect()
}

fn static_inline_targets_for_function(
    function_id: RuntimeFunctionId,
    static_direct_calls: &StaticTypedDirectCalls,
    remapped_inline_targets: &HashMap<RuntimeFunctionId, TypedInlineTargets>,
    suppressed_inline_targets: &SuppressedTypedInlineTargets,
) -> TypedInlineTargets {
    let mut targets = TypedInlineTargets::new();
    if let Some(static_calls) = static_direct_calls.get(&function_id) {
        let static_targets = static_calls
            .iter()
            .filter_map(|(source, plans)| {
                let plans = plans
                    .iter()
                    .filter(|plan| plan.body.kind == CallBodyKind::Inline)
                    .map(|plan| (plan.target, plan.arg_plan.clone()))
                    .collect::<Vec<_>>();
                (!plans.is_empty()).then_some((*source, plans))
            })
            .collect();
        merge_typed_inline_targets(&mut targets, &static_targets);
    }
    if let Some(remapped) = remapped_inline_targets.get(&function_id) {
        merge_typed_inline_targets(&mut targets, remapped);
    }
    if let Some(suppressed) = suppressed_inline_targets.get(&function_id) {
        targets.retain(|instr_id, _| !suppressed.contains(instr_id));
    }
    targets
}

fn remap_inlined_direct_call_targets(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    profile: &SpecializationProfile<'_>,
    trusted_generator_bridge_targets: &HashSet<RuntimeFunctionId>,
    static_direct_calls: &StaticTypedDirectCalls,
    remapped_inline_targets: &mut HashMap<RuntimeFunctionId, TypedInlineTargets>,
    suppressed_inline_targets: &SuppressedTypedInlineTargets,
) -> usize {
    let mut targets_by_callee = HashMap::<RuntimeFunctionId, TypedInlineTargets>::new();
    let mut count = 0;
    for mapping in mappings {
        let targets = targets_by_callee.entry(mapping.callee).or_insert_with(|| {
            let mut targets = transitive_profile_inline_targets_for_function(
                mapping.callee,
                module,
                external_callees,
                trusted_generator_bridge_targets,
                profile,
            );
            // Profile-selected inline decisions are caller-local. Only keep
            // small nested profile targets here; larger bodies need a fresh
            // caller-local decision instead of being cloned transitively into
            // every caller that inlines this body.
            merge_typed_inline_targets(
                &mut targets,
                &static_inline_targets_for_function(
                    mapping.callee,
                    static_direct_calls,
                    remapped_inline_targets,
                    suppressed_inline_targets,
                ),
            );
            targets
        });
        let Some(plans) = targets.get(&mapping.callee_instr_id) else {
            continue;
        };
        let entry = remapped_inline_targets
            .entry(caller_function_id)
            .or_default()
            .entry(mapping.caller_instr_id)
            .or_default();
        for plan in plans {
            if !entry.contains(plan) {
                entry.push(plan.clone());
                count += 1;
            }
        }
    }
    count
}

fn remap_inlined_call_emission_plans(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    static_direct_calls: &StaticTypedDirectCalls,
    remapped_call_emissions: &mut RemappedTypedCallEmissions,
) -> Result<usize, String> {
    let mut emissions_by_callee = HashMap::<RuntimeFunctionId, TypedCallEmissionPlans>::new();
    let mut count = 0;
    for mapping in mappings {
        if !emissions_by_callee.contains_key(&mapping.callee) {
            let emissions = typed_call_emission_plans_for_function_with_remapped(
                profile,
                mapping.callee,
                static_direct_calls,
                remapped_call_emissions,
            )?;
            emissions_by_callee.insert(mapping.callee, emissions);
        }
        let Some(plan) = emissions_by_callee
            .get(&mapping.callee)
            .and_then(|emissions| emissions.by_source.get(&mapping.callee_instr_id))
            .cloned()
        else {
            continue;
        };
        let entry = remapped_call_emissions
            .entry(caller_function_id)
            .or_default()
            .by_source
            .entry(mapping.caller_instr_id);
        match entry {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(plan);
                count += 1;
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                if entry.get() != &plan {
                    return Err(format!(
                        "inlined typed call emission source {} maps to conflicting plans",
                        mapping.caller_instr_id
                    ));
                }
            }
        }
    }
    Ok(count)
}

fn remap_cloned_direct_call_targets(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    static_direct_calls: &StaticTypedDirectCalls,
    remapped_inline_targets: &mut HashMap<RuntimeFunctionId, TypedInlineTargets>,
    suppressed_inline_targets: &SuppressedTypedInlineTargets,
) -> usize {
    let source_targets = typed_inline_targets_for_function(
        caller_function_id,
        profile,
        static_direct_calls,
        remapped_inline_targets,
        suppressed_inline_targets,
    );
    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let Some(plans) = source_targets.get(&mapping.callee_instr_id) else {
            continue;
        };
        let entry = remapped_inline_targets
            .entry(caller_function_id)
            .or_default()
            .entry(mapping.caller_instr_id)
            .or_default();
        for plan in plans {
            if !entry.contains(plan) {
                entry.push(plan.clone());
                count += 1;
            }
        }
    }
    count
}

fn remap_cloned_call_emission_plans(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    static_direct_calls: &StaticTypedDirectCalls,
    remapped_call_emissions: &mut RemappedTypedCallEmissions,
) -> Result<usize, String> {
    let source_emissions = typed_call_emission_plans_for_function_with_remapped(
        profile,
        caller_function_id,
        static_direct_calls,
        remapped_call_emissions,
    )?;
    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let Some(plan) = source_emissions
            .by_source
            .get(&mapping.callee_instr_id)
            .cloned()
        else {
            continue;
        };
        let entry = remapped_call_emissions
            .entry(caller_function_id)
            .or_default()
            .by_source
            .entry(mapping.caller_instr_id);
        match entry {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(plan);
                count += 1;
            }
            std::collections::hash_map::Entry::Occupied(entry) => {
                if entry.get() != &plan {
                    return Err(format!(
                        "cloned typed call emission source {} maps to conflicting plans",
                        mapping.caller_instr_id
                    ));
                }
            }
        }
    }
    Ok(count)
}

fn indexed_field_accesses_for_function(
    function_id: RuntimeFunctionId,
    profile: &SpecializationProfile<'_>,
    remapped_indexed_fields: &HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    >,
) -> Result<HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>, String> {
    let (_, _, mut fields) = profile.field_index_specialization_maps(function_id)?;
    if let Some(remapped) = remapped_indexed_fields.get(&function_id) {
        for (instr_id, accesses) in remapped {
            let entry = fields.entry(*instr_id).or_default();
            for access in accesses {
                if !entry.contains(access) {
                    entry.push(access.clone());
                }
            }
        }
    }
    Ok(fields)
}

fn remap_cloned_indexed_field_accesses(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    remapped_indexed_fields: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    >,
    remapped_indexed_field_counter_sources: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedIndexedFieldCounterSource>,
    >,
) -> Result<usize, String> {
    let source_fields =
        indexed_field_accesses_for_function(caller_function_id, profile, remapped_indexed_fields)?;
    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let Some(accesses) = source_fields.get(&mapping.callee_instr_id) else {
            continue;
        };
        let source = remapped_indexed_field_counter_sources
            .get(&caller_function_id)
            .and_then(|sources| sources.get(&mapping.callee_instr_id))
            .copied()
            .unwrap_or(TypedIndexedFieldCounterSource {
                function_id: mapping.callee,
                instr_id: mapping.callee_instr_id,
            });
        let entry = remapped_indexed_fields
            .entry(caller_function_id)
            .or_default()
            .entry(mapping.caller_instr_id)
            .or_default();
        for access in accesses {
            if !entry.contains(access) {
                entry.push(access.clone());
                count += 1;
            }
        }
        let counter_source = remapped_indexed_field_counter_sources
            .entry(caller_function_id)
            .or_default()
            .entry(mapping.caller_instr_id)
            .or_insert(source);
        if *counter_source != source {
            return Err(format!(
                "cloned indexed-field counter source for instruction {} maps to both {}:{} and {}:{}",
                mapping.caller_instr_id,
                counter_source.function_id,
                counter_source.instr_id,
                source.function_id,
                source.instr_id
            ));
        }
    }
    Ok(count)
}

fn remap_cloned_constructor_init_plans(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    constructor_init_plans: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorInitPlan>,
    >,
) -> Result<usize, String> {
    let source_plans = constructor_init_plans
        .get(&caller_function_id)
        .cloned()
        .unwrap_or_default();
    if source_plans.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let Some(plan) = source_plans.get(&mapping.callee_instr_id).copied() else {
            continue;
        };
        let entry = constructor_init_plans
            .entry(caller_function_id)
            .or_default();
        if let Some(existing) = entry.get(&mapping.caller_instr_id) {
            if *existing != plan {
                return Err(format!(
                    "cloned constructor-init plan for instruction {} maps to both {} and {}",
                    mapping.caller_instr_id, existing.init_function_id, plan.init_function_id
                ));
            }
            continue;
        }
        entry.insert(mapping.caller_instr_id, plan);
        count += 1;
    }
    Ok(count)
}

fn remap_cloned_generator_instance_plans(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    remapped_generator_instance_plans: &mut RemappedTypedGeneratorInstancePlans,
) -> Result<usize, String> {
    let source_plans = remapped_generator_instance_plans
        .get(&caller_function_id)
        .cloned()
        .unwrap_or_default();
    if source_plans.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let Some(plan) = source_plans.get(&mapping.callee_instr_id).cloned() else {
            continue;
        };
        let entry = remapped_generator_instance_plans
            .entry(caller_function_id)
            .or_default();
        if let Some(existing) = entry.get(&mapping.caller_instr_id) {
            if *existing != plan {
                return Err(format!(
                    "cloned generator-instance plan for instruction {} maps to conflicting targets",
                    mapping.caller_instr_id
                ));
            }
            continue;
        }
        entry.insert(mapping.caller_instr_id, plan);
        count += 1;
    }
    Ok(count)
}

fn remap_cloned_generator_state_lowering_instr_ids(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    instr_ids_by_origin: &mut HashMap<InstrId, (RuntimeFunctionId, HashSet<InstrId>)>,
) -> Result<usize, String> {
    let mut total = 0;
    loop {
        let source = instr_ids_by_origin.clone();
        if source.is_empty() {
            return Ok(total);
        }

        let mut count = 0;
        for (generator_origin, (function_id, body_instr_ids)) in source {
            let origin_mappings = mappings
                .iter()
                .filter(|mapping| {
                    mapping.callee == caller_function_id
                        && mapping.callee_instr_id == generator_origin
                })
                .collect::<Vec<_>>();

            if origin_mappings.is_empty() {
                let remapped_body_instr_ids = mappings
                    .iter()
                    .filter(|mapping| {
                        mapping.callee == caller_function_id
                            && body_instr_ids.contains(&mapping.callee_instr_id)
                    })
                    .map(|mapping| mapping.caller_instr_id)
                    .collect::<HashSet<_>>();
                if remapped_body_instr_ids.is_empty() {
                    continue;
                }
                let entry = instr_ids_by_origin
                    .entry(generator_origin)
                    .or_insert_with(|| (function_id, HashSet::new()));
                if entry.0 != function_id {
                    return Err(format!(
                        "cloned generator-state body ids for origin {generator_origin} map to conflicting generator functions"
                    ));
                }
                let old_len = entry.1.len();
                entry.1.extend(remapped_body_instr_ids.iter().copied());
                count += entry.1.len() - old_len;
                continue;
            }

            for origin_mapping in origin_mappings {
                let cloned_origin = origin_mapping.caller_instr_id;
                let remapped_body_instr_ids = mappings
                    .iter()
                    .filter(|mapping| {
                        mapping.callee == caller_function_id
                            && mapping.inline_instance == origin_mapping.inline_instance
                            && body_instr_ids.contains(&mapping.callee_instr_id)
                    })
                    .map(|mapping| mapping.caller_instr_id)
                    .collect::<HashSet<_>>();
                let entry = instr_ids_by_origin
                    .entry(cloned_origin)
                    .or_insert_with(|| (function_id, HashSet::new()));
                if entry.0 != function_id {
                    return Err(format!(
                        "cloned generator-state origin {cloned_origin} maps to conflicting generator functions"
                    ));
                }
                let old_len = entry.1.len();
                entry.1.extend(remapped_body_instr_ids);
                count += entry.1.len() - old_len;
            }
        }

        if count == 0 {
            return Ok(total);
        }
        total += count;
    }
}

fn remap_cloned_constructor_field_bindings(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    constructor_field_bindings: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorFieldBindings>,
    >,
) -> Result<usize, String> {
    let source_bindings = constructor_field_bindings
        .get(&caller_function_id)
        .cloned()
        .unwrap_or_default();
    if source_bindings.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let Some(bindings) = source_bindings.get(&mapping.callee_instr_id).cloned() else {
            continue;
        };
        let entry = constructor_field_bindings
            .entry(caller_function_id)
            .or_default();
        if let Some(existing) = entry.get(&mapping.caller_instr_id) {
            if *existing != bindings {
                return Err(format!(
                    "cloned constructor field bindings for instruction {} map to conflicting field sets",
                    mapping.caller_instr_id
                ));
            }
            continue;
        }
        entry.insert(mapping.caller_instr_id, bindings);
        count += 1;
    }
    Ok(count)
}

fn remap_cloned_generator_constructor_capture_bindings(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
) -> Result<usize, String> {
    let source_bindings = constructor_capture_bindings_by_origin.clone();
    if source_bindings.is_empty() {
        return Ok(0);
    }

    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let Some(bindings) = source_bindings.get(&mapping.callee_instr_id).cloned() else {
            continue;
        };
        if let Some(existing) = constructor_capture_bindings_by_origin.get(&mapping.caller_instr_id)
        {
            if *existing != bindings {
                return Err(format!(
                    "cloned generator constructor capture bindings for instruction {} map to conflicting capture sets",
                    mapping.caller_instr_id
                ));
            }
            continue;
        }
        constructor_capture_bindings_by_origin.insert(mapping.caller_instr_id, bindings);
        count += 1;
    }
    Ok(count)
}

fn remap_cloned_exact_int_instr_id(
    instr_id: InstrId,
    context: &HashMap<InstrId, InstrId>,
) -> InstrId {
    context.get(&instr_id).copied().unwrap_or(instr_id)
}

fn remap_cloned_exact_int_region_plan(
    region: &RegionPlan,
    context: &HashMap<InstrId, InstrId>,
) -> RegionPlan {
    let mut remapped = region.clone();
    if let RegionSource::Instr { instr_id } = &mut remapped.source {
        *instr_id = remap_cloned_exact_int_instr_id(*instr_id, context);
    }
    for input in &mut remapped.inputs {
        match &mut input.source {
            RegionInputSource::IndexedGlobal { source, .. }
            | RegionInputSource::IndexedField { source, .. } => {
                *source = remap_cloned_exact_int_instr_id(*source, context);
            }
            RegionInputSource::FunctionParam { .. }
            | RegionInputSource::ModuleConstant { .. }
            | RegionInputSource::CapturedValue { .. }
            | RegionInputSource::Synthetic { .. } => {}
        }
    }
    for exit in &mut remapped.exits {
        if let Some(source) = &mut exit.source {
            *source = remap_cloned_exact_int_instr_id(*source, context);
        }
    }
    remapped
}

fn remap_cloned_exact_int_mechanical_region(
    region: &MechanicalRegionEmission,
    context: &HashMap<InstrId, InstrId>,
) -> MechanicalRegionEmission {
    let mut remapped = region.clone();
    for step in &mut remapped.steps {
        if let Some(source) = &mut step.source {
            *source = remap_cloned_exact_int_instr_id(*source, context);
        }
    }
    for exit in &mut remapped.exits {
        if let Some(source) = &mut exit.source {
            *source = remap_cloned_exact_int_instr_id(*source, context);
        }
    }
    remapped
}

fn remap_cloned_exact_int_branch_plan(
    instr_id: InstrId,
    plan: &TypedExactIntBranchPlan,
    context: &HashMap<InstrId, InstrId>,
) -> TypedExactIntBranchPlan {
    TypedExactIntBranchPlan {
        source: plan.source,
        instr_id,
        hot_plan: remap_cloned_exact_int_region_plan(&plan.hot_plan, context),
        hot_region: remap_cloned_exact_int_mechanical_region(&plan.hot_region, context),
        fallback_plan: remap_cloned_exact_int_region_plan(&plan.fallback_plan, context),
        fallback_region: remap_cloned_exact_int_mechanical_region(&plan.fallback_region, context),
    }
}

fn remap_cloned_exact_int_return_plan(
    instr_id: InstrId,
    plan: &TypedExactIntReturnPlan,
    context: &HashMap<InstrId, InstrId>,
) -> TypedExactIntReturnPlan {
    TypedExactIntReturnPlan {
        source: plan.source,
        instr_id,
        hot_plan: remap_cloned_exact_int_region_plan(&plan.hot_plan, context),
        hot_region: remap_cloned_exact_int_mechanical_region(&plan.hot_region, context),
        fallback_plan: remap_cloned_exact_int_region_plan(&plan.fallback_plan, context),
        fallback_region: remap_cloned_exact_int_mechanical_region(&plan.fallback_region, context),
    }
}

fn remap_cloned_exact_int_selections(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    remapped_branches: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_returns: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>,
) -> Result<usize, String> {
    let mut contexts = HashMap::<u32, HashMap<InstrId, InstrId>>::new();
    for mapping in mappings {
        if mapping.callee == caller_function_id {
            contexts
                .entry(mapping.inline_instance)
                .or_default()
                .insert(mapping.callee_instr_id, mapping.caller_instr_id);
        }
    }
    if contexts.is_empty() {
        return Ok(0);
    }

    let source_branches = remapped_branches
        .get(&caller_function_id)
        .cloned()
        .unwrap_or_default();
    let source_returns = remapped_returns
        .get(&caller_function_id)
        .cloned()
        .unwrap_or_default();
    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        let context = contexts
            .get(&mapping.inline_instance)
            .expect("same-function clone should have a remap context");
        if let Some(plan) = source_branches.get(&mapping.callee_instr_id) {
            let remapped =
                remap_cloned_exact_int_branch_plan(mapping.caller_instr_id, plan, context);
            let entry = remapped_branches
                .entry(caller_function_id)
                .or_default()
                .entry(mapping.caller_instr_id);
            match entry {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(remapped);
                    count += 1;
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    if entry.get() != &remapped {
                        return Err(format!(
                            "cloned exact-int branch plan for instruction {} maps to conflicting plans",
                            mapping.caller_instr_id
                        ));
                    }
                }
            }
        }
        if let Some(plan) = source_returns.get(&mapping.callee_instr_id) {
            let remapped =
                remap_cloned_exact_int_return_plan(mapping.caller_instr_id, plan, context);
            let entry = remapped_returns
                .entry(caller_function_id)
                .or_default()
                .entry(mapping.caller_instr_id);
            match entry {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(remapped);
                    count += 1;
                }
                std::collections::hash_map::Entry::Occupied(entry) => {
                    if entry.get() != &remapped {
                        return Err(format!(
                            "cloned exact-int return plan for instruction {} maps to conflicting plans",
                            mapping.caller_instr_id
                        ));
                    }
                }
            }
        }
    }
    Ok(count)
}

fn typed_local_locations_by_name(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<String, LocalLocation> {
    function
        .storage_layout
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
                                .expect("stack slot index should fit in LocalLocation"),
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn typed_virtual_state_for_exact_int_instr<'a>(
    field_states: &'a TypedVirtualFieldStateAnalysis,
    instr_locations: &InstrLocationMap,
    instr_id: InstrId,
) -> Option<&'a TypedVirtualState> {
    let location = instr_locations.get(&instr_id).copied()?;
    match location.body_index() {
        Some(instr_index) => field_states.body_before_instr.get(&TypedVirtualBodyInstr {
            block: location.block_label(),
            instr_index,
        }),
        None => field_states.block_before_term.get(&location.block_label()),
    }
}

fn remap_virtualized_exact_int_region_inputs_to_scalar_locals(
    region: &mut RegionPlan,
    state: &TypedVirtualState,
    local_locations_by_name: &HashMap<String, LocalLocation>,
) -> usize {
    let mut count = 0;
    for input in &mut region.inputs {
        if input.value.rep != soac_ir_typed::plan_v3::Rep::PyObjectBorrowed {
            continue;
        }
        let RegionInputSource::IndexedField {
            receiver:
                IndexedFieldReceiverSource::LocalName {
                    name: receiver_name,
                },
            attr_name,
            ..
        } = &input.source
        else {
            continue;
        };
        let Some(receiver_location) = local_locations_by_name.get(receiver_name).copied() else {
            continue;
        };
        let Some(object) = state.aliases.get(&receiver_location).copied() else {
            continue;
        };
        let Some(scalar) = state
            .fields
            .get(&TypedVirtualFieldRef {
                object,
                field_name: attr_name.clone(),
            })
            .cloned()
        else {
            continue;
        };
        input.source = RegionInputSource::FunctionParam {
            index: input.value.id.0,
            name: Some(scalar.id_str().to_string()),
        };
        count += 1;
    }
    count
}

fn remap_virtualized_exact_int_plan_inputs_to_scalar_locals(
    plan: &mut TypedExactIntBranchPlan,
    state: &TypedVirtualState,
    local_locations_by_name: &HashMap<String, LocalLocation>,
) -> usize {
    remap_virtualized_exact_int_region_inputs_to_scalar_locals(
        &mut plan.hot_plan,
        state,
        local_locations_by_name,
    ) + remap_virtualized_exact_int_region_inputs_to_scalar_locals(
        &mut plan.fallback_plan,
        state,
        local_locations_by_name,
    )
}

fn remap_virtualized_exact_int_return_inputs_to_scalar_locals(
    plan: &mut TypedExactIntReturnPlan,
    state: &TypedVirtualState,
    local_locations_by_name: &HashMap<String, LocalLocation>,
) -> usize {
    remap_virtualized_exact_int_region_inputs_to_scalar_locals(
        &mut plan.hot_plan,
        state,
        local_locations_by_name,
    ) + remap_virtualized_exact_int_region_inputs_to_scalar_locals(
        &mut plan.fallback_plan,
        state,
        local_locations_by_name,
    )
}

fn remap_virtualized_exact_int_inputs_to_scalar_locals(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    virtualization_plan: &TypedVirtualizationPlan,
) -> usize {
    let Some(field_states) = virtualization_plan.field_states.as_ref() else {
        return 0;
    };
    let instr_locations = current_instr_locations(function);
    let local_locations_by_name = typed_local_locations_by_name(function);

    struct Remapper<'a> {
        field_states: &'a TypedVirtualFieldStateAnalysis,
        instr_locations: &'a InstrLocationMap,
        local_locations_by_name: &'a HashMap<String, LocalLocation>,
        count: usize,
    }

    impl VisitMut<InstrTyped> for Remapper<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if let Some(extra) = expr.typed_extra_mut() {
                if let Some(plan) = extra.exact_int_branch_plan_mut()
                    && let Some(state) = typed_virtual_state_for_exact_int_instr(
                        self.field_states,
                        self.instr_locations,
                        plan.instr_id,
                    )
                {
                    self.count += remap_virtualized_exact_int_plan_inputs_to_scalar_locals(
                        plan,
                        state,
                        self.local_locations_by_name,
                    );
                }
                if let Some(plan) = extra.exact_int_return_plan_mut()
                    && let Some(state) = typed_virtual_state_for_exact_int_instr(
                        self.field_states,
                        self.instr_locations,
                        plan.instr_id,
                    )
                {
                    self.count += remap_virtualized_exact_int_return_inputs_to_scalar_locals(
                        plan,
                        state,
                        self.local_locations_by_name,
                    );
                }
            }
            expr.visit_children_mut(self);
        }
    }

    let mut remapper = Remapper {
        field_states,
        instr_locations: &instr_locations,
        local_locations_by_name: &local_locations_by_name,
        count: 0,
    };
    remapper.visit_fn_mut(function);
    remapper.count
}

fn retain_live_typed_profile_sidecars(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    remapped_call_emissions: &mut RemappedTypedCallEmissions,
    remapped_inline_targets: &mut HashMap<RuntimeFunctionId, TypedInlineTargets>,
    remapped_generator_instance_plans: &mut RemappedTypedGeneratorInstancePlans,
    remapped_indexed_fields: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    >,
    remapped_indexed_field_counter_sources: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedIndexedFieldCounterSource>,
    >,
    remapped_exact_list_items: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, ProfileExactListItemAccessPlan>,
    >,
    remapped_branches: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_returns: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>,
    constructor_init_plans: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorInitPlan>,
    >,
) {
    let live_instr_ids = collect_typed_semantic_instr_ids(function);
    if let Some(emissions) = remapped_call_emissions.get_mut(&function.function_id) {
        emissions
            .by_source
            .retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
    if let Some(targets) = remapped_inline_targets.get_mut(&function.function_id) {
        targets.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
    if let Some(plans) = remapped_generator_instance_plans.get_mut(&function.function_id) {
        plans.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
    if let Some(fields) = remapped_indexed_fields.get_mut(&function.function_id) {
        fields.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
    if let Some(counter_sources) =
        remapped_indexed_field_counter_sources.get_mut(&function.function_id)
    {
        counter_sources.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
    if let Some(items) = remapped_exact_list_items.get_mut(&function.function_id) {
        items.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
    if let Some(branches) = remapped_branches.get_mut(&function.function_id) {
        branches.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
    if let Some(returns) = remapped_returns.get_mut(&function.function_id) {
        returns.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
    if let Some(plans) = constructor_init_plans.get_mut(&function.function_id) {
        plans.retain(|instr_id, _| live_instr_ids.contains(instr_id));
    }
}

fn collect_typed_semantic_instr_ids(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<InstrId> {
    struct Collector {
        instr_ids: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(instr_id) = expr.try_semantic_instr_id() {
                self.instr_ids.insert(instr_id);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        instr_ids: HashSet::new(),
    };
    collector.visit_fn(function);
    collector.instr_ids
}

fn remap_cloned_profile_rewrites(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    profile: &SpecializationProfile<'_>,
    static_direct_calls: &StaticTypedDirectCalls,
    remapped_call_emissions: &mut RemappedTypedCallEmissions,
    remapped_inline_targets: &mut HashMap<RuntimeFunctionId, TypedInlineTargets>,
    remapped_generator_instance_plans: &mut RemappedTypedGeneratorInstancePlans,
    suppressed_inline_targets: &SuppressedTypedInlineTargets,
    remapped_indexed_fields: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    >,
    remapped_indexed_field_counter_sources: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedIndexedFieldCounterSource>,
    >,
    remapped_exact_list_items: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, ProfileExactListItemAccessPlan>,
    >,
    remapped_branches: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_returns: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>,
    constructor_init_plans: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorInitPlan>,
    >,
    constructor_field_bindings: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorFieldBindings>,
    >,
) -> Result<usize, String> {
    let mut total = 0;
    loop {
        let mut count = 0;
        count += remap_cloned_call_emission_plans(
            caller_function_id,
            mappings,
            profile,
            static_direct_calls,
            remapped_call_emissions,
        )?;
        count += remap_cloned_direct_call_targets(
            caller_function_id,
            mappings,
            profile,
            static_direct_calls,
            remapped_inline_targets,
            suppressed_inline_targets,
        );
        count += remap_cloned_generator_instance_plans(
            caller_function_id,
            mappings,
            remapped_generator_instance_plans,
        )?;
        count += remap_cloned_indexed_field_accesses(
            caller_function_id,
            mappings,
            profile,
            remapped_indexed_fields,
            remapped_indexed_field_counter_sources,
        )?;
        count += remap_cloned_exact_list_item_accesses(
            caller_function_id,
            mappings,
            profile,
            remapped_exact_list_items,
        )?;
        count += remap_cloned_exact_int_selections(
            caller_function_id,
            mappings,
            remapped_branches,
            remapped_returns,
        )?;
        count += remap_cloned_constructor_init_plans(
            caller_function_id,
            mappings,
            constructor_init_plans,
        )?;
        count += remap_cloned_constructor_field_bindings(
            caller_function_id,
            mappings,
            constructor_field_bindings,
        )?;
        if count == 0 {
            return Ok(total);
        }
        total += count;
    }
}

fn retire_cloned_inline_targets(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    remapped_inline_targets: &mut HashMap<RuntimeFunctionId, TypedInlineTargets>,
    suppressed_inline_targets: &mut SuppressedTypedInlineTargets,
) -> usize {
    let mut count = 0;
    for mapping in mappings {
        if mapping.callee != caller_function_id {
            continue;
        }
        if suppressed_inline_targets
            .entry(caller_function_id)
            .or_default()
            .insert(mapping.callee_instr_id)
        {
            count += 1;
        }
        if let Some(targets) = remapped_inline_targets.get_mut(&caller_function_id) {
            targets.remove(&mapping.callee_instr_id);
        }
    }
    count
}

fn remap_cloned_hot_state_cleanup_labels(
    labels: &mut HashSet<BlockLabel>,
    mappings: &[(BlockLabel, BlockLabel)],
) {
    let remapped = mappings
        .iter()
        .filter_map(|(source, target)| labels.contains(source).then_some(*target))
        .collect::<Vec<_>>();
    labels.extend(remapped);
}

fn remap_cloned_generator_pending_alias_use_instr_ids(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    pending_alias_use_instr_ids_by_origin: &mut HashMap<InstrId, HashSet<InstrId>>,
) -> usize {
    let mut total = 0;
    loop {
        let source = pending_alias_use_instr_ids_by_origin.clone();
        if source.is_empty() {
            return total;
        }

        let mut count = 0;
        for (generator_origin, pending_alias_use_instr_ids) in source {
            let origin_mappings = mappings
                .iter()
                .filter(|mapping| {
                    mapping.callee == caller_function_id
                        && mapping.callee_instr_id == generator_origin
                })
                .collect::<Vec<_>>();

            if origin_mappings.is_empty() {
                let remapped_pending_alias_use_instr_ids = mappings
                    .iter()
                    .filter(|mapping| {
                        mapping.callee == caller_function_id
                            && pending_alias_use_instr_ids.contains(&mapping.callee_instr_id)
                    })
                    .map(|mapping| mapping.caller_instr_id)
                    .collect::<HashSet<_>>();
                if remapped_pending_alias_use_instr_ids.is_empty() {
                    continue;
                }
                let entry = pending_alias_use_instr_ids_by_origin
                    .entry(generator_origin)
                    .or_default();
                let old_len = entry.len();
                entry.extend(remapped_pending_alias_use_instr_ids.iter().copied());
                count += entry.len() - old_len;
                continue;
            }

            for origin_mapping in origin_mappings {
                let cloned_origin = origin_mapping.caller_instr_id;
                let remapped_pending_alias_use_instr_ids = mappings
                    .iter()
                    .filter(|mapping| {
                        mapping.callee == caller_function_id
                            && mapping.inline_instance == origin_mapping.inline_instance
                            && pending_alias_use_instr_ids.contains(&mapping.callee_instr_id)
                    })
                    .map(|mapping| mapping.caller_instr_id)
                    .collect::<HashSet<_>>();
                let entry = pending_alias_use_instr_ids_by_origin
                    .entry(cloned_origin)
                    .or_default();
                let old_len = entry.len();
                entry.extend(remapped_pending_alias_use_instr_ids.iter().copied());
                count += entry.len() - old_len;
            }
        }

        if count == 0 {
            return total;
        }
        total += count;
    }
}

fn remap_cloned_static_constructor_calls(
    calls: &mut HashMap<InstrId, TypedAttrOwnerRef>,
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
) {
    let remapped = mappings
        .iter()
        .filter_map(|mapping| {
            (mapping.callee == caller_function_id)
                .then(|| calls.get(&mapping.callee_instr_id).cloned())
                .flatten()
                .map(|owner_type_ref| (mapping.caller_instr_id, owner_type_ref))
        })
        .collect::<Vec<_>>();
    calls.extend(remapped);
}

fn remapped_static_constructor_calls_from_inline_stats(
    stats: &soac_opt::passes::TypedInlineRewriteStats,
    static_constructor_calls: &StaticConstructorCalls,
) -> HashMap<InstrId, TypedAttrOwnerRef> {
    stats
        .instr_id_mappings
        .iter()
        .filter_map(|mapping| {
            static_constructor_calls
                .get(&mapping.callee)?
                .get(&mapping.callee_instr_id)
                .cloned()
                .map(|owner_type_ref| (mapping.caller_instr_id, owner_type_ref))
        })
        .collect()
}

fn trusted_materialized_constructor_calls_from_inline_stats(
    stats: &soac_opt::passes::TypedInlineRewriteStats,
    trusted_call_sources: &HashMap<InstrId, TypedAttrOwnerRef>,
    constructor_field_bindings: &HashMap<InstrId, TypedConstructorFieldBindings>,
) -> HashMap<InstrId, TypedAttrOwnerRef> {
    let trusted_inline_instances = stats
        .inline_instance_sources
        .iter()
        .filter_map(|mapping| {
            trusted_call_sources
                .get(&mapping.source_instr_id)
                .cloned()
                .map(|owner_type_ref| (mapping.inline_instance, owner_type_ref))
        })
        .collect::<HashMap<_, _>>();
    if trusted_inline_instances.is_empty() {
        return HashMap::new();
    }
    stats
        .instr_id_mappings
        .iter()
        .filter_map(|mapping| {
            constructor_field_bindings
                .contains_key(&mapping.caller_instr_id)
                .then(|| {
                    trusted_inline_instances
                        .get(&mapping.inline_instance)
                        .cloned()
                        .map(|owner_type_ref| (mapping.caller_instr_id, owner_type_ref))
                })
                .flatten()
        })
        .collect()
}

#[cfg(test)]
fn trusted_runtime_protocol_calls_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: Option<&HashMap<InstrId, TypedAttrOwnerRef>>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> HashMap<InstrId, TypedAttrOwnerRef> {
    trusted_runtime_protocol_calls_from_owner_states(
        function,
        module_constants,
        trusted_constructor_calls.unwrap_or(&HashMap::new()),
        trusted_constructor_init_owners,
    )
}

#[cfg(test)]
fn trusted_runtime_protocol_calls_from_field_states(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    field_states: &TypedVirtualFieldStateAnalysis,
    trusted_owners_by_object: &HashMap<TypedVirtualObjectId, TypedAttrOwnerRef>,
) -> HashMap<InstrId, TypedAttrOwnerRef> {
    struct Collector<'a> {
        state: &'a TypedVirtualState,
        trusted_owners_by_object: &'a HashMap<TypedVirtualObjectId, TypedAttrOwnerRef>,
        calls: HashMap<InstrId, TypedAttrOwnerRef>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && matches!(
                    call.access,
                    soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod { .. }
                )
                && let Some(instr_id) = call.try_semantic_instr_id()
                && let Some(soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(
                    load,
                ))) = call.args.first()
                && let Some(location) = load.name.local_location()
                && let Some(object) = self.state.aliases.get(&location)
                && let Some(owner_type_ref) = self.trusted_owners_by_object.get(object)
            {
                self.calls.insert(instr_id, owner_type_ref.clone());
            }
            expr.visit_children(self);
        }
    }

    let mut calls = HashMap::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = field_states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                continue;
            };
            let mut collector = Collector {
                state,
                trusted_owners_by_object,
                calls: HashMap::new(),
            };
            collector.visit_instr(instr);
            calls.extend(collector.calls);
        }
    }
    calls
}

fn typed_expr_mentions_resume_generator(expr: &InstrTyped) -> bool {
    struct Finder {
        found: bool,
    }

    impl Visit<InstrTyped> for Finder {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            if let InstrTyped::Load(load) = expr
                && load.name.id_str() == RuntimeName::ResumeGenerator.name()
            {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder { found: false };
    finder.visit_instr(expr);
    finder.found
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrustedGeneratorResumeDecisionPhase {
    InitialPlanning,
    PostNormalizationRefresh,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum TrustedGeneratorResumeDecisionOutcome {
    Selected,
    Escaped {
        generator_origin: InstrId,
    },
    MissingOwnerState,
    PlanMissing {
        reason: TrustedGeneratorResumePlanMissReason,
    },
    AliasFiltered {
        generator_origin: InstrId,
    },
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct TrustedGeneratorResumeCandidateId {
    function_id: RuntimeFunctionId,
    instr_id: InstrId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TrustedGeneratorResumeDecision {
    candidate: Option<TrustedGeneratorResumeCandidateId>,
    instr_id: Option<InstrId>,
    block: BlockLabel,
    instr_index: Option<usize>,
    phase: TrustedGeneratorResumeDecisionPhase,
    reachable: bool,
    outcome: TrustedGeneratorResumeDecisionOutcome,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrustedGeneratorResumeDecisionReport {
    decisions: Vec<TrustedGeneratorResumeDecision>,
    discovered_candidates: HashSet<TrustedGeneratorResumeCandidateId>,
    duplicate_candidate_discoveries: usize,
    missing_plan_counts: HashMap<TrustedGeneratorResumePlanMissReason, usize>,
}

impl TrustedGeneratorResumeDecisionReport {
    fn push(&mut self, decision: TrustedGeneratorResumeDecision) {
        if let Some(candidate) = decision.candidate
            && !self.discovered_candidates.insert(candidate)
        {
            self.duplicate_candidate_discoveries += 1;
        }
        if let TrustedGeneratorResumeDecisionOutcome::PlanMissing { reason } = &decision.outcome {
            *self.missing_plan_counts.entry(*reason).or_default() += 1;
        }
        self.decisions.push(decision);
    }

    #[cfg(test)]
    fn has_outcome<F>(&self, predicate: F) -> bool
    where
        F: Fn(&TrustedGeneratorResumeDecisionOutcome) -> bool,
    {
        self.decisions
            .iter()
            .any(|decision| predicate(&decision.outcome))
    }

    fn discovered_candidate_count(&self) -> usize {
        self.discovered_candidates.len()
    }

    #[cfg(test)]
    fn missing_plan_count(&self, reason: TrustedGeneratorResumePlanMissReason) -> usize {
        self.missing_plan_counts.get(&reason).copied().unwrap_or(0)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrustedGeneratorResumeCandidateWorklist {
    pending: VecDeque<TrustedGeneratorResumeCandidateId>,
    queued_candidates: HashSet<TrustedGeneratorResumeCandidateId>,
    duplicate_candidate_enqueues: usize,
    processed_candidate_count: usize,
}

impl TrustedGeneratorResumeCandidateWorklist {
    fn from_report(report: &TrustedGeneratorResumeDecisionReport) -> Self {
        let mut worklist = Self::default();
        for decision in &report.decisions {
            if let Some(candidate) = decision.candidate {
                worklist.enqueue(candidate);
            }
        }
        worklist
    }

    fn enqueue(&mut self, candidate: TrustedGeneratorResumeCandidateId) {
        if !self.queued_candidates.insert(candidate) {
            self.duplicate_candidate_enqueues += 1;
            return;
        }
        self.pending.push_back(candidate);
    }

    fn retain_discovered_plans(
        &mut self,
        discovered_plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
    ) -> HashMap<InstrId, TypedGeneratorResumePlan> {
        let mut plans = HashMap::new();
        while let Some(candidate) = self.pending.pop_front() {
            self.processed_candidate_count += 1;
            if let Some(plan) = discovered_plans.get(&candidate.instr_id).cloned() {
                plans.insert(candidate.instr_id, plan);
            }
        }
        plans
    }

    fn queued_candidate_count(&self) -> usize {
        self.queued_candidates.len()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct LateTypedRefreshSchedule {
    rewritten_stop_iteration: usize,
    pending_families: HashSet<LateTypedRefreshFamily>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum LateTypedRefreshFamily {
    TrustedGeneratorResume,
}

impl LateTypedRefreshSchedule {
    fn from_rewritten_stop_iteration(rewritten_stop_iteration: usize) -> Self {
        let mut pending_families = HashSet::new();
        if rewritten_stop_iteration != 0 {
            pending_families.insert(LateTypedRefreshFamily::TrustedGeneratorResume);
        }
        Self {
            rewritten_stop_iteration,
            pending_families,
        }
    }

    fn requests(&self, family: LateTypedRefreshFamily) -> bool {
        self.pending_families.contains(&family)
    }

    fn request(&mut self, family: LateTypedRefreshFamily) -> bool {
        self.pending_families.insert(family)
    }

    fn consume(&mut self, family: LateTypedRefreshFamily) -> bool {
        self.pending_families.remove(&family)
    }

    fn record_rewritten_stop_iteration(&mut self, rewritten_stop_iteration: usize) {
        if rewritten_stop_iteration == 0 {
            return;
        }
        self.rewritten_stop_iteration += rewritten_stop_iteration;
        self.pending_families
            .insert(LateTypedRefreshFamily::TrustedGeneratorResume);
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct LateTypedFixpointIteration {
    builtin_implementation_rewrites: usize,
    runtime_protocol_rewrites: usize,
    resume_rewrites: usize,
    stop_iteration_rewrites: usize,
}

impl LateTypedFixpointIteration {
    fn may_expose_stop_iteration_edges(&self) -> bool {
        self.builtin_implementation_rewrites != 0
            || self.runtime_protocol_rewrites != 0
            || self.resume_rewrites != 0
    }

    fn made_progress(&self) -> bool {
        self.may_expose_stop_iteration_edges() || self.stop_iteration_rewrites != 0
    }
}

#[cfg(test)]
fn trusted_generator_resume_plans_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> HashMap<InstrId, TypedGeneratorResumePlan> {
    trusted_generator_resume_plans_and_report_for_function(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    )
    .0
}

#[cfg(test)]
fn trusted_generator_resume_plans_and_report_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> (
    HashMap<InstrId, TypedGeneratorResumePlan>,
    TrustedGeneratorResumeDecisionReport,
) {
    trusted_generator_resume_plans_and_report_for_function_with_phase(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
        TrustedGeneratorResumeDecisionPhase::InitialPlanning,
    )
}

#[cfg(test)]
fn trusted_generator_resume_plans_and_report_for_function_with_phase(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    phase: TrustedGeneratorResumeDecisionPhase,
) -> (
    HashMap<InstrId, TypedGeneratorResumePlan>,
    TrustedGeneratorResumeDecisionReport,
) {
    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    trusted_generator_resume_plans_and_report_from_analysis(
        function,
        module_constants,
        &states,
        phase,
        &HashMap::new(),
    )
}

fn trusted_generator_resume_plans_from_analysis(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    states: &TrustedOwnerStateAnalysis,
    retained_pending_alias_use_source_instr_ids_by_origin: &HashMap<InstrId, HashSet<InstrId>>,
) -> HashMap<InstrId, TypedGeneratorResumePlan> {
    trusted_generator_resume_plans_and_report_from_analysis(
        function,
        module_constants,
        states,
        TrustedGeneratorResumeDecisionPhase::InitialPlanning,
        retained_pending_alias_use_source_instr_ids_by_origin,
    )
    .0
}

fn trusted_generator_alias_cleanup_active_blocks(
    states: &TrustedOwnerStateAnalysis,
) -> HashSet<BlockLabel> {
    states.block_before_term.keys().copied().collect()
}

fn trusted_generator_resume_plans_and_report_from_analysis(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    states: &TrustedOwnerStateAnalysis,
    phase: TrustedGeneratorResumeDecisionPhase,
    retained_pending_alias_use_source_instr_ids_by_origin: &HashMap<InstrId, HashSet<InstrId>>,
) -> (
    HashMap<InstrId, TypedGeneratorResumePlan>,
    TrustedGeneratorResumeDecisionReport,
) {
    fn merged_component_resume_lookup(
        expr: &InstrTyped,
        merged_state: &TrustedOwnerState,
        component_states: &[TrustedOwnerState],
        module_constants: &[ConstantExpr],
    ) -> TrustedGeneratorResumePlanLookup {
        let merged_lookup =
            trusted_generator_resume_plan_lookup_for_expr(expr, merged_state, module_constants);
        if component_states.len() <= 1
            || !matches!(
                merged_lookup,
                TrustedGeneratorResumePlanLookup::Missing {
                    reason: TrustedGeneratorResumePlanMissReason::MissingResumeFunction
                        | TrustedGeneratorResumePlanMissReason::MissingOwnerOrigin
                        | TrustedGeneratorResumePlanMissReason::OriginMismatch,
                    ..
                }
            )
        {
            return merged_lookup;
        }

        let mut merged_instr_id = None;
        let mut merged_function_id = None;
        let mut merged_generator_origin = None;
        let mut merged_candidate_origins = HashSet::new();
        let mut rejected_because_escaped = None;

        for component_state in component_states {
            match trusted_generator_resume_plan_lookup_for_expr(
                expr,
                component_state,
                module_constants,
            ) {
                TrustedGeneratorResumePlanLookup::Present { instr_id, plan } => {
                    if merged_instr_id.is_some_and(|candidate| candidate != instr_id) {
                        return merged_lookup;
                    }
                    if merged_function_id.is_some_and(|candidate| candidate != plan.function_id) {
                        return merged_lookup;
                    }
                    merged_instr_id = Some(instr_id);
                    merged_function_id = Some(plan.function_id);
                    match (merged_generator_origin, plan.generator_origin) {
                        (None, origin) => merged_generator_origin = Some(origin),
                        (Some(existing), origin) if existing == origin => {}
                        (Some(_), _) => merged_generator_origin = Some(None),
                    }
                    merged_candidate_origins.extend(plan.candidate_origins);
                }
                lookup @ TrustedGeneratorResumePlanLookup::RejectedBecauseEscaped { .. } => {
                    rejected_because_escaped.get_or_insert(lookup);
                }
                _ => return merged_lookup,
            }
        }

        if let Some(lookup) = rejected_because_escaped {
            return lookup;
        }

        let Some(instr_id) = merged_instr_id else {
            return merged_lookup;
        };
        let Some(function_id) = merged_function_id else {
            return merged_lookup;
        };
        let mut candidate_origins = merged_candidate_origins.into_iter().collect::<Vec<_>>();
        candidate_origins.sort_by_key(|origin| origin.index());
        TrustedGeneratorResumePlanLookup::Present {
            instr_id,
            plan: TypedGeneratorResumePlan {
                function_id,
                generator_origin: merged_generator_origin.flatten(),
                candidate_origins,
            },
        }
    }

    let materialized_owner_blocks = trusted_generator_alias_cleanup_active_blocks(states);
    struct Collector<'a> {
        function_id: RuntimeFunctionId,
        module_constants: &'a [ConstantExpr],
        state: &'a TrustedOwnerState,
        component_states: &'a [TrustedOwnerState],
        block: BlockLabel,
        instr_index: Option<usize>,
        phase: TrustedGeneratorResumeDecisionPhase,
        plans: HashMap<InstrId, TypedGeneratorResumePlan>,
        decisions: Vec<TrustedGeneratorResumeDecision>,
        selected_plan_count: usize,
        escaped_plan_count: usize,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            match merged_component_resume_lookup(
                expr,
                self.state,
                self.component_states,
                self.module_constants,
            ) {
                TrustedGeneratorResumePlanLookup::Present { instr_id, plan } => {
                    self.selected_plan_count += 1;
                    self.plans.insert(instr_id, plan);
                    self.decisions.push(TrustedGeneratorResumeDecision {
                        candidate: Some(TrustedGeneratorResumeCandidateId {
                            function_id: self.function_id,
                            instr_id,
                        }),
                        instr_id: Some(instr_id),
                        block: self.block,
                        instr_index: self.instr_index,
                        phase: self.phase,
                        reachable: true,
                        outcome: TrustedGeneratorResumeDecisionOutcome::Selected,
                    });
                }
                TrustedGeneratorResumePlanLookup::RejectedBecauseEscaped {
                    instr_id,
                    generator_origin,
                    ..
                } => {
                    self.selected_plan_count += 1;
                    self.escaped_plan_count += 1;
                    self.decisions.push(TrustedGeneratorResumeDecision {
                        candidate: Some(TrustedGeneratorResumeCandidateId {
                            function_id: self.function_id,
                            instr_id,
                        }),
                        instr_id: Some(instr_id),
                        block: self.block,
                        instr_index: self.instr_index,
                        phase: self.phase,
                        reachable: true,
                        outcome: TrustedGeneratorResumeDecisionOutcome::Escaped {
                            generator_origin,
                        },
                    });
                }
                TrustedGeneratorResumePlanLookup::Missing { instr_id, reason } => {
                    if typed_expr_mentions_resume_generator(expr) {
                        self.decisions.push(TrustedGeneratorResumeDecision {
                            candidate: instr_id.map(|instr_id| TrustedGeneratorResumeCandidateId {
                                function_id: self.function_id,
                                instr_id,
                            }),
                            instr_id,
                            block: self.block,
                            instr_index: self.instr_index,
                            phase: self.phase,
                            reachable: true,
                            outcome: TrustedGeneratorResumeDecisionOutcome::PlanMissing { reason },
                        });
                    }
                }
            }
            expr.visit_children(self);
        }
    }

    let mut plans = HashMap::new();
    let mut selected_plan_count = 0usize;
    let mut escaped_plan_count = 0usize;
    let mut missing_owner_state_count = 0usize;
    let mut report = TrustedGeneratorResumeDecisionReport::default();
    for block in states.reachable_blocks.iter_blocks(function) {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                if typed_expr_mentions_resume_generator(instr) {
                    missing_owner_state_count += 1;
                    tracing::debug!(
                        target: "soac_generator_resume_planning",
                        function_id = ?function.function_id,
                        block = ?block.label,
                        instr_index,
                        instr_id = ?instr.try_semantic_instr_id(),
                        "typed_generator_resume_plan_skipped_missing_owner_state",
                    );
                    report.push(TrustedGeneratorResumeDecision {
                        candidate: instr.try_semantic_instr_id().map(|instr_id| {
                            TrustedGeneratorResumeCandidateId {
                                function_id: function.function_id,
                                instr_id,
                            }
                        }),
                        instr_id: instr.try_semantic_instr_id(),
                        block: block.label,
                        instr_index: Some(instr_index),
                        phase,
                        reachable: true,
                        outcome: TrustedGeneratorResumeDecisionOutcome::MissingOwnerState,
                    });
                }
                continue;
            };
            let mut collector = Collector {
                function_id: function.function_id,
                module_constants,
                state,
                component_states: states
                    .body_before_instr_components
                    .get(&TypedVirtualBodyInstr {
                        block: block.label,
                        instr_index,
                    })
                    .map(Vec::as_slice)
                    .unwrap_or_else(|| std::slice::from_ref(state)),
                block: block.label,
                instr_index: Some(instr_index),
                phase,
                plans: HashMap::new(),
                decisions: Vec::new(),
                selected_plan_count: 0,
                escaped_plan_count: 0,
            };
            collector.visit_instr(instr);
            selected_plan_count += collector.selected_plan_count;
            escaped_plan_count += collector.escaped_plan_count;
            for decision in collector.decisions {
                report.push(decision);
            }
            plans.extend(collector.plans);
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            function_id: function.function_id,
            module_constants,
            state,
            component_states: states
                .block_before_term_components
                .get(&block.label)
                .map(Vec::as_slice)
                .unwrap_or_else(|| std::slice::from_ref(state)),
            block: block.label,
            instr_index: None,
            phase,
            plans: HashMap::new(),
            decisions: Vec::new(),
            selected_plan_count: 0,
            escaped_plan_count: 0,
        };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
        selected_plan_count += collector.selected_plan_count;
        escaped_plan_count += collector.escaped_plan_count;
        for decision in collector.decisions {
            report.push(decision);
        }
        plans.extend(collector.plans);
    }
    let candidate_plan_count = plans.len();
    let inlined_resume_instr_ids_by_origin = plans.iter().fold(
        HashMap::<InstrId, HashSet<InstrId>>::new(),
        |mut sources_by_origin, (source, plan)| {
            if let Some(generator_origin) = typed_generator_resume_plan_state_origin(plan) {
                sources_by_origin
                    .entry(generator_origin)
                    .or_default()
                    .insert(*source);
            }
            sources_by_origin
        },
    );
    let mut alias_filter_source_instr_ids_by_origin =
        retained_pending_alias_use_source_instr_ids_by_origin.clone();
    retain_typed_generator_pending_alias_use_instr_ids_by_origin(
        &mut alias_filter_source_instr_ids_by_origin,
        inlined_resume_instr_ids_by_origin.clone(),
    );
    let ignored_resume_instr_ids_by_origin = typed_generator_alias_ignored_instr_ids_by_origin(
        function,
        module_constants,
        &alias_filter_source_instr_ids_by_origin,
    );
    for (&generator_origin, planned_resume_instr_ids) in &inlined_resume_instr_ids_by_origin {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            function_id = ?function.function_id,
            generator_origin = ?generator_origin,
            planned_resume_instr_ids = ?planned_resume_instr_ids,
            ignored_resume_instr_ids = ?ignored_resume_instr_ids_by_origin.get(&generator_origin),
            "typed_generator_resume_alias_filter_inputs",
        );
    }
    plans.retain(|source, plan| {
        let Some(generator_origin) = typed_generator_resume_plan_state_origin(plan) else {
            return true;
        };
        let retained = typed_generator_state_origin_can_lower_aliases_in_blocks(
            function,
            module_constants,
            generator_origin,
            ignored_resume_instr_ids_by_origin
                .get(&generator_origin)
                .expect("planned generator origin should retain its resume source ids"),
            Some(&materialized_owner_blocks),
        );
        if !retained {
            report.push(TrustedGeneratorResumeDecision {
                candidate: Some(TrustedGeneratorResumeCandidateId {
                    function_id: function.function_id,
                    instr_id: *source,
                }),
                instr_id: Some(*source),
                block: BlockLabel::fallthrough(),
                instr_index: None,
                phase,
                reachable: true,
                outcome: TrustedGeneratorResumeDecisionOutcome::AliasFiltered { generator_origin },
            });
        }
        retained
    });
    if candidate_plan_count != 0 {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            function_id = ?function.function_id,
            candidate_plan_count,
            retained_plan_count = plans.len(),
            filtered_plan_count = candidate_plan_count.saturating_sub(plans.len()),
            candidate_origin_count = inlined_resume_instr_ids_by_origin.len(),
            "typed_generator_resume_plan_alias_filter_summary",
        );
    }
    if selected_plan_count != 0 || escaped_plan_count != 0 || missing_owner_state_count != 0 {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            function_id = ?function.function_id,
            selected_plan_count,
            escaped_plan_count,
            missing_owner_state_count,
            candidate_plan_count,
            retained_plan_count = plans.len(),
            alias_filtered_plan_count = candidate_plan_count.saturating_sub(plans.len()),
            "typed_generator_resume_plan_collection_summary",
        );
    }
    (plans, report)
}

fn generator_resume_inline_targets(
    plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
) -> TypedInlineTargets {
    plans
        .iter()
        .filter_map(|(source, plan)| {
            if typed_generator_resume_plan_state_origin(plan).is_none()
                || !typed_generator_resume_inline_target_is_small_enough(
                    module,
                    external_callees,
                    plan.function_id,
                )
            {
                return None;
            }

            Some((
                *source,
                vec![(
                    plan.function_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            soac_ir_typed::TypedDirectCallArgSource::Provided(1),
                            soac_ir_typed::TypedDirectCallArgSource::Provided(2),
                            soac_ir_typed::TypedDirectCallArgSource::Provided(3),
                            soac_ir_typed::TypedDirectCallArgSource::Provided(4),
                        ],
                    },
                )],
            ))
        })
        .collect()
}

#[derive(Default)]
struct TrustedOwnerStateCache {
    states: Option<TrustedOwnerStateAnalysis>,
    builds: usize,
    reuses: usize,
    invalidations: usize,
}

impl TrustedOwnerStateCache {
    fn invalidate(&mut self) {
        if self.states.take().is_some() {
            self.invalidations += 1;
        }
    }

    fn states<'a>(
        &'a mut self,
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &[ConstantExpr],
        trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
        trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    ) -> &'a TrustedOwnerStateAnalysis {
        if self.states.is_some() {
            self.reuses += 1;
        } else {
            self.builds += 1;
            self.states = Some(analyze_trusted_owner_states(
                function,
                module_constants,
                trusted_constructor_calls,
                trusted_constructor_init_owners,
            ));
        }
        self.states
            .as_ref()
            .expect("trusted-owner state cache should be populated")
    }
}

#[allow(clippy::too_many_arguments)]
fn inline_late_typed_generator_resume_plans_after_stop_iteration_normalization(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &mut Vec<ConstantExpr>,
    late_refresh_schedule: &mut LateTypedRefreshSchedule,
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    generator_state_constructors_by_origin: &HashMap<InstrId, TypedGeneratorStateConstructor>,
    constructor_capture_bindings_by_function: &HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, HashMap<u32, CellLocation>>,
    >,
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
    generator_state_instr_ids_by_origin: &mut HashMap<
        InstrId,
        (RuntimeFunctionId, HashSet<InstrId>),
    >,
    generator_state_pending_alias_use_instr_ids_by_origin: &mut HashMap<InstrId, HashSet<InstrId>>,
    lowered_generator_preserved_locals: &mut LoweredGeneratorPreservedLocals,
    static_targets: &StaticDirectCallTargets,
    local_generators: &HashMap<RuntimeFunctionId, &BlockPyFunction<TypedBlockPyModuleShape>>,
    remapped_generator_instance_plans: Option<&HashMap<InstrId, TypedGeneratorInstancePlan>>,
    late_trusted_owner_states: &mut TrustedOwnerStateCache,
) -> Result<usize, String> {
    if !late_refresh_schedule.requests(LateTypedRefreshFamily::TrustedGeneratorResume) {
        return Ok(0);
    }
    let pruned_unreachable_blocks = prune_unreachable_typed_blocks(function);
    if pruned_unreachable_blocks != 0 {
        late_trusted_owner_states.invalidate();
        tracing::info!(
            target: "soac_generator_resume_planning",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            pruned_unreachable_blocks,
            "typed_generator_resume_pruned_unreachable_blocks_before_late_planning",
        );
    }
    let mut late_generator_state_constructors_by_origin =
        generator_state_constructors_by_origin.clone();
    for (origin, constructor) in &mut late_generator_state_constructors_by_origin {
        if constructor.closure_cell_bindings.is_none() {
            constructor.closure_cell_bindings =
                constructor_capture_bindings_by_origin.get(origin).cloned();
        }
    }
    let trusted_owner_states = late_trusted_owner_states.states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    let (generator_resume_plans, _decision_report) =
        refresh_typed_generator_resume_candidates_after_late_normalization_from_analysis(
            function,
            module_constants,
            late_refresh_schedule,
            trusted_owner_states,
            generator_state_pending_alias_use_instr_ids_by_origin,
        );
    late_refresh_schedule.consume(LateTypedRefreshFamily::TrustedGeneratorResume);
    annotate_typed_generator_resume_plans(function, &generator_resume_plans)?;
    retain_typed_generator_pending_alias_evidence_by_origin(
        function,
        module_constants,
        generator_state_pending_alias_use_instr_ids_by_origin,
        &generator_resume_plans,
    );
    let inline_targets =
        generator_resume_inline_targets(&generator_resume_plans, callee_module, external_callees);
    let inline_targets = select_typed_inline_targets_within_cfg_budget(
        function,
        callee_module,
        external_callees,
        &generator_resume_plans,
        inline_targets,
    );
    if !generator_resume_plans.is_empty() {
        tracing::info!(
            target: "soac_generator_state_lowering",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            generator_resume_plan_count = generator_resume_plans.len(),
            inline_target_count = inline_targets.len(),
            "typed_late_generator_resume_targets_collected",
        );
    }
    if inline_targets.is_empty() {
        return Ok(0);
    }

    let pre_inline_block_labels = function
        .blocks
        .iter()
        .map(|block| block.label)
        .collect::<HashSet<_>>();
    let stats = inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls(
        function,
        callee_module,
        module_constants,
        external_callees,
        &inline_targets,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        &late_generator_state_constructors_by_origin,
    );
    let rewritten =
        stats.rewritten_stores + stats.rewritten_effect_only_calls + stats.rewritten_returns;
    tracing::info!(
        target: "soac_generator_state_lowering",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        generator_resume_plan_count = generator_resume_plans.len(),
        rewritten_stores = stats.rewritten_stores,
        rewritten_effect_only_calls = stats.rewritten_effect_only_calls,
        rewritten_returns = stats.rewritten_returns,
        skipped_candidates = stats.skipped_candidates,
        skipped_exception_edges = stats.skipped_exception_edges,
        inline_source_count = stats.inline_instance_sources.len(),
        instr_id_mapping_count = stats.instr_id_mappings.len(),
        "typed_late_generator_resume_inline_stats",
    );
    if rewritten == 0 {
        return Ok(0);
    }

    import_typed_generator_constructor_capture_bindings_from_mappings(
        function.function_id,
        &stats.instr_id_mappings,
        constructor_capture_bindings_by_function,
        constructor_capture_bindings_by_origin,
    );
    collect_generator_state_lowering_instr_ids(
        &generator_resume_plans,
        &stats,
        generator_state_instr_ids_by_origin,
    );
    propagate_generator_state_lowering_synthetic_instr_ids(
        &stats,
        generator_state_instr_ids_by_origin,
    );
    propagate_generator_pending_alias_use_synthetic_instr_ids(
        &stats,
        generator_state_pending_alias_use_instr_ids_by_origin,
    );
    let mut generator_resume_alias_cleanup_active_blocks =
        trusted_generator_alias_cleanup_active_blocks(trusted_owner_states);
    generator_resume_alias_cleanup_active_blocks.extend(
        function
            .blocks
            .iter()
            .filter(|block| !pre_inline_block_labels.contains(&block.label))
            .map(|block| block.label),
    );
    retain_typed_generator_pending_alias_use_instr_ids_by_origin(
        generator_state_pending_alias_use_instr_ids_by_origin,
        typed_generator_pending_alias_use_source_instr_ids_by_origin(&generator_resume_plans),
    );
    let pending_alias_use_instr_ids_by_origin = typed_generator_alias_ignored_instr_ids_by_origin(
        function,
        module_constants,
        generator_state_pending_alias_use_instr_ids_by_origin,
    );
    retain_typed_generator_pending_alias_use_instr_ids_by_origin(
        generator_state_pending_alias_use_instr_ids_by_origin,
        pending_alias_use_instr_ids_by_origin.clone(),
    );
    lower_or_remap_typed_generator_state_for_function(
        function,
        module_constants,
        callee_module,
        typed_generator_state_lowering_plans(
            generator_state_instr_ids_by_origin.clone(),
            &late_generator_state_constructors_by_origin,
            &pending_alias_use_instr_ids_by_origin,
            Some(&generator_resume_alias_cleanup_active_blocks),
        ),
        0,
        None,
        lowered_generator_preserved_locals,
    );
    remap_inlined_generator_constructor_capture_bindings_for_lowered_state(
        function,
        &generator_resume_plans,
        &stats,
        lowered_generator_preserved_locals,
        constructor_capture_bindings_by_origin,
    );
    assign_missing_typed_function_instr_ids(function);
    refresh_typed_function_value_facts(function);
    let mut refreshed_generator_instance_plans =
        static_generator_instance_plans_for_function(function, static_targets);
    refreshed_generator_instance_plans.extend(static_local_generator_instance_plans_for_function(
        function,
        local_generators,
    ));
    if let Some(remapped) = remapped_generator_instance_plans {
        refreshed_generator_instance_plans.extend(remapped.clone());
    }
    annotate_typed_generator_instance_plans(function, Some(&refreshed_generator_instance_plans))?;
    Ok(rewritten)
}

#[cfg(test)]
fn refresh_typed_generator_resume_candidates_after_late_normalization(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    late_refresh_schedule: &LateTypedRefreshSchedule,
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> (
    HashMap<InstrId, TypedGeneratorResumePlan>,
    TrustedGeneratorResumeDecisionReport,
) {
    let trusted_owner_states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    refresh_typed_generator_resume_candidates_after_late_normalization_from_analysis(
        function,
        module_constants,
        late_refresh_schedule,
        &trusted_owner_states,
        &HashMap::new(),
    )
}

fn refresh_typed_generator_resume_candidates_after_late_normalization_from_analysis(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    late_refresh_schedule: &LateTypedRefreshSchedule,
    trusted_owner_states: &TrustedOwnerStateAnalysis,
    retained_pending_alias_use_source_instr_ids_by_origin: &HashMap<InstrId, HashSet<InstrId>>,
) -> (
    HashMap<InstrId, TypedGeneratorResumePlan>,
    TrustedGeneratorResumeDecisionReport,
) {
    if !late_refresh_schedule.requests(LateTypedRefreshFamily::TrustedGeneratorResume) {
        tracing::debug!(
            target: "soac_generator_resume_planning",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            rewritten_stop_iteration = late_refresh_schedule.rewritten_stop_iteration,
            "typed_generator_resume_refresh_skipped_after_late_normalization",
        );
        return (
            HashMap::new(),
            TrustedGeneratorResumeDecisionReport::default(),
        );
    }
    let (discovered_plans, report) = trusted_generator_resume_plans_and_report_from_analysis(
        function,
        module_constants,
        trusted_owner_states,
        TrustedGeneratorResumeDecisionPhase::PostNormalizationRefresh,
        retained_pending_alias_use_source_instr_ids_by_origin,
    );
    let mut worklist = TrustedGeneratorResumeCandidateWorklist::from_report(&report);
    let plans = worklist.retain_discovered_plans(&discovered_plans);
    tracing::debug!(
        target: "soac_generator_resume_planning",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        rewritten_stop_iteration = late_refresh_schedule.rewritten_stop_iteration,
        refresh_trusted_generator_resume = late_refresh_schedule
            .requests(LateTypedRefreshFamily::TrustedGeneratorResume),
        reported_candidate_count = report.discovered_candidate_count(),
        queued_candidate_count = worklist.queued_candidate_count(),
        duplicate_candidate_discoveries = report.duplicate_candidate_discoveries,
        duplicate_candidate_enqueues = worklist.duplicate_candidate_enqueues,
        processed_candidate_count = worklist.processed_candidate_count,
        retained_plan_count = plans.len(),
        "typed_generator_resume_candidates_refreshed_after_late_normalization",
    );
    (plans, report)
}

#[allow(clippy::too_many_arguments)]
fn inline_late_typed_runtime_protocol_and_static_method_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &mut Vec<ConstantExpr>,
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    static_targets: &StaticDirectCallTargets,
    late_trusted_owner_states: &mut TrustedOwnerStateCache,
) -> Result<soac_opt::passes::TypedInlineRewriteStats, String> {
    let linearization = linearize_typed_function_expressions(function).map_err(|reason| {
        format!(
            "late runtime-protocol typed expression linearization failed for {}: {reason:?}",
            function.names.qualname
        )
    })?;
    if linearization.lifted_nested_exprs != 0 {
        late_trusted_owner_states.invalidate();
        assign_missing_typed_function_instr_ids(function);
        refresh_typed_function_value_facts(function);
        tracing::debug!(
            target: "soac_typed_linearization",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            rewritten_body_roots = linearization.rewritten_body_roots,
            rewritten_terms = linearization.rewritten_terms,
            lifted_nested_exprs = linearization.lifted_nested_exprs,
            "late_typed_expression_linearization_before_runtime_protocols",
        );
    }
    let states = late_trusted_owner_states.states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    let (
        mut trusted_runtime_protocol_calls,
        mut inline_targets,
        mut trusted_runtime_protocol_receiver_origins,
        mut trusted_runtime_protocol_resume_functions,
    ) = trusted_static_runtime_protocol_inlines_from_analysis(
        function,
        module_constants,
        &states,
        static_targets,
    );
    let (
        trusted_static_method_calls,
        static_method_inline_targets,
        static_method_receiver_origins,
        static_method_receiver_resume_functions,
    ) = trusted_static_method_inlines_from_analysis(
        function,
        module_constants,
        &states,
        static_targets,
    );
    trusted_runtime_protocol_calls.extend(trusted_static_method_calls);
    inline_targets.extend(static_method_inline_targets);
    for (instr_id, mut origins) in static_method_receiver_origins {
        let receiver_origins = trusted_runtime_protocol_receiver_origins
            .entry(instr_id)
            .or_default();
        receiver_origins.append(&mut origins);
        receiver_origins.sort_by_key(|origin| origin.index());
        receiver_origins.dedup();
    }
    trusted_runtime_protocol_resume_functions.extend(static_method_receiver_resume_functions);
    let trusted_runtime_protocol_sources = trusted_runtime_protocol_calls
        .keys()
        .copied()
        .collect::<HashSet<_>>();
    let inline_targets = select_typed_inline_targets_within_cfg_budget_and_priorities(
        function,
        callee_module,
        external_callees,
        &HashMap::new(),
        &trusted_runtime_protocol_sources,
        &HashSet::new(),
        inline_targets,
        None,
    );
    if inline_targets.is_empty() {
        tracing::debug!(
            target: "soac_late_typed_refresh",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            refresh_family = "runtime_protocols",
            inline_target_count = 0usize,
            "late_typed_refresh_family_idle",
        );
        return Ok(soac_opt::passes::TypedInlineRewriteStats::default());
    }

    let stats = inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls(
        function,
        callee_module,
        module_constants,
        external_callees,
        &inline_targets,
        &trusted_runtime_protocol_calls,
        &trusted_runtime_protocol_receiver_origins,
        &trusted_runtime_protocol_resume_functions,
        &HashMap::new(),
    );
    let rewritten =
        stats.rewritten_stores + stats.rewritten_effect_only_calls + stats.rewritten_returns;
    tracing::debug!(
        target: "soac_late_typed_refresh",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        refresh_family = "runtime_protocols",
        inline_target_count = inline_targets.len(),
        rewritten,
        skipped_candidates = stats.skipped_candidates,
        skipped_exception_edges = stats.skipped_exception_edges,
        "late_typed_refresh_family_applied",
    );
    if rewritten != 0 {
        assign_missing_typed_function_instr_ids(function);
        refresh_typed_function_value_facts(function);
    }
    Ok(stats)
}

fn refresh_typed_generator_inline_sidecars_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constructors_by_origin: &mut HashMap<InstrId, TypedGeneratorStateConstructor>,
    bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
) {
    refresh_materialized_generator_state_constructors_for_function(
        function,
        constructors_by_origin,
    );
    bindings_by_origin.extend(typed_generator_constructor_capture_bindings_by_origin(
        function,
    ));
    for (origin, constructor) in constructors_by_origin {
        if constructor.closure_cell_bindings.is_none() {
            constructor.closure_cell_bindings = bindings_by_origin.get(origin).cloned();
        }
    }
}

fn import_typed_generator_constructor_capture_bindings_from_mappings(
    caller_function_id: RuntimeFunctionId,
    mappings: &[TypedInlineInstrIdMapping],
    constructor_capture_bindings_by_function: &HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, HashMap<u32, CellLocation>>,
    >,
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
) {
    for mapping in mappings {
        let bindings = (mapping.callee == caller_function_id)
            .then(|| {
                constructor_capture_bindings_by_origin
                    .get(&mapping.callee_instr_id)
                    .cloned()
            })
            .flatten()
            .or_else(|| {
                constructor_capture_bindings_by_function
                    .get(&mapping.callee)
                    .and_then(|bindings_by_origin| bindings_by_origin.get(&mapping.callee_instr_id))
                    .cloned()
            });
        if let Some(bindings) = bindings {
            constructor_capture_bindings_by_origin
                .entry(mapping.caller_instr_id)
                .or_insert(bindings);
        }
    }
}

fn absorb_typed_generator_inline_materializations(
    materialized_args: &[soac_opt::passes::TypedInlineMaterializedGeneratorArg],
    constructors_by_origin: &mut HashMap<InstrId, TypedGeneratorStateConstructor>,
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
    trusted_constructor_calls: &mut HashMap<InstrId, TypedAttrOwnerRef>,
) {
    for materialized in materialized_args {
        let constructor = constructors_by_origin
            .entry(materialized.generator_origin)
            .or_insert_with(|| TypedGeneratorStateConstructor {
                target: materialized.target.clone(),
                call: materialized.call.clone(),
                closure_cell_bindings: None,
            });
        if constructor.closure_cell_bindings.is_none() {
            constructor.closure_cell_bindings =
                materialized.closure_cell_bindings.clone().or_else(|| {
                    constructor_capture_bindings_by_origin
                        .get(&materialized.generator_origin)
                        .cloned()
                });
        }
        if let Some(bindings) = constructor.closure_cell_bindings.clone() {
            constructor_capture_bindings_by_origin
                .entry(materialized.generator_origin)
                .or_insert(bindings);
        }
        if let Some(owner_type_ref) = materialized
            .call
            .extra
            .generator_instance_plan()
            .and_then(trusted_generator_instance_owner)
        {
            trusted_constructor_calls.insert(materialized.generator_origin, owner_type_ref);
        }
    }
}

fn refresh_typed_generator_inline_sidecars_after_rewrite(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    inline_stats: &soac_opt::passes::TypedInlineRewriteStats,
    constructor_capture_bindings_by_function: &HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, HashMap<u32, CellLocation>>,
    >,
    constructors_by_origin: &mut HashMap<InstrId, TypedGeneratorStateConstructor>,
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
    trusted_constructor_calls: &mut HashMap<InstrId, TypedAttrOwnerRef>,
) {
    import_typed_generator_constructor_capture_bindings_from_mappings(
        function.function_id,
        &inline_stats.instr_id_mappings,
        constructor_capture_bindings_by_function,
        constructor_capture_bindings_by_origin,
    );
    absorb_typed_generator_inline_materializations(
        &inline_stats.materialized_generator_args,
        constructors_by_origin,
        constructor_capture_bindings_by_origin,
        trusted_constructor_calls,
    );
    refresh_typed_generator_inline_sidecars_for_function(
        function,
        constructors_by_origin,
        constructor_capture_bindings_by_origin,
    );
}

#[allow(clippy::too_many_arguments)]
fn inline_late_typed_builtin_implementation_plans(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &mut Vec<ConstantExpr>,
    trusted_constructor_calls: &mut HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    static_targets: &StaticDirectCallTargets,
    generator_state_constructors_by_origin: &mut HashMap<InstrId, TypedGeneratorStateConstructor>,
    constructor_capture_bindings_by_function: &HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, HashMap<u32, CellLocation>>,
    >,
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
    late_trusted_owner_states: &mut TrustedOwnerStateCache,
) -> Result<usize, String> {
    let linearization = linearize_typed_function_expressions(function).map_err(|reason| {
        format!(
            "late typed expression linearization failed for {}: {reason:?}",
            function.names.qualname
        )
    })?;
    if linearization.lifted_nested_exprs != 0 {
        late_trusted_owner_states.invalidate();
        assign_missing_typed_function_instr_ids(function);
        refresh_typed_function_value_facts(function);
        tracing::debug!(
            target: "soac_typed_linearization",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            rewritten_body_roots = linearization.rewritten_body_roots,
            rewritten_terms = linearization.rewritten_terms,
            lifted_nested_exprs = linearization.lifted_nested_exprs,
            "late_typed_expression_linearization_before_builtin_consumers",
        );
    }
    refresh_typed_generator_inline_sidecars_for_function(
        function,
        generator_state_constructors_by_origin,
        constructor_capture_bindings_by_origin,
    );
    let states = late_trusted_owner_states.states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    let discovered_builtin_implementation_plans =
        trusted_generator_builtin_implementation_plans_from_analysis(
            function,
            callee_module,
            external_callees,
            module_constants,
            &states,
            static_targets,
        );
    let mut builtin_candidate_ids = discovered_builtin_implementation_plans
        .keys()
        .copied()
        .collect::<Vec<_>>();
    builtin_candidate_ids.sort_by_key(|source| source.index());
    let mut builtin_worklist = VecDeque::from(builtin_candidate_ids);
    let mut processed_builtin_candidates = HashSet::new();
    let mut builtin_implementation_plans = HashMap::new();
    while let Some(instr_id) = builtin_worklist.pop_front() {
        if !processed_builtin_candidates.insert(instr_id) {
            continue;
        }
        if let Some(plan) = discovered_builtin_implementation_plans
            .get(&instr_id)
            .cloned()
        {
            builtin_implementation_plans.insert(instr_id, plan);
        }
    }
    tracing::debug!(
        target: "soac_builtin_consumer_planning",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        refreshed_candidate_count = processed_builtin_candidates.len(),
        retained_plan_count = builtin_implementation_plans.len(),
        "typed_builtin_generator_consumer_candidates_revisited_during_late_fixpoint",
    );
    tracing::debug!(
        target: "soac_late_typed_refresh",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        refresh_family = "builtin_implementation",
        plan_count = builtin_implementation_plans.len(),
        "late_typed_refresh_family_planned",
    );
    trace_builtin_implementation_plan_placements(function, &builtin_implementation_plans);
    let builtin_implementation_sources = builtin_implementation_plans
        .keys()
        .copied()
        .collect::<HashSet<_>>();
    let inline_targets = builtin_implementation_inline_targets(&builtin_implementation_plans);
    let inline_targets = select_typed_inline_targets_within_cfg_budget_and_priorities(
        function,
        callee_module,
        external_callees,
        &HashMap::new(),
        &HashSet::new(),
        &builtin_implementation_sources,
        inline_targets,
        Some(1),
    );
    retain_selected_typed_builtin_implementation_plans(
        function,
        &builtin_implementation_plans,
        &inline_targets,
    )?;
    if inline_targets.is_empty() {
        return Ok(0);
    }

    let stats = inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls(
        function,
        callee_module,
        module_constants,
        external_callees,
        &inline_targets,
        &HashMap::new(),
        &HashMap::new(),
        &HashMap::new(),
        generator_state_constructors_by_origin,
    );
    let rewritten =
        stats.rewritten_stores + stats.rewritten_effect_only_calls + stats.rewritten_returns;
    tracing::debug!(
        target: "soac_late_typed_refresh",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        refresh_family = "builtin_implementation",
        plan_count = builtin_implementation_plans.len(),
        rewritten,
        skipped_candidates = stats.skipped_candidates,
        skipped_exception_edges = stats.skipped_exception_edges,
        "late_typed_refresh_family_applied",
    );
    if rewritten == 0 {
        return Ok(0);
    }

    refresh_typed_generator_inline_sidecars_after_rewrite(
        function,
        &stats,
        constructor_capture_bindings_by_function,
        generator_state_constructors_by_origin,
        constructor_capture_bindings_by_origin,
        trusted_constructor_calls,
    );
    assign_missing_typed_function_instr_ids(function);
    refresh_typed_function_value_facts(function);
    Ok(rewritten)
}

fn builtin_implementation_inline_targets(
    plans: &HashMap<InstrId, TypedBuiltinImplementationPlan>,
) -> TypedInlineTargets {
    plans
        .iter()
        .map(|(source, plan)| (*source, vec![(plan.function_id, plan.arg_plan.clone())]))
        .collect()
}

fn typed_generator_resume_plan_state_origin(plan: &TypedGeneratorResumePlan) -> Option<InstrId> {
    plan.generator_origin
        .or_else(|| match plan.candidate_origins.as_slice() {
            [origin] => Some(*origin),
            _ => None,
        })
}

fn typed_generator_resume_plan_state_origins(
    plan: &TypedGeneratorResumePlan,
) -> impl Iterator<Item = InstrId> + '_ {
    typed_generator_resume_plan_state_origin(plan).into_iter()
}

fn collect_generator_state_lowering_instr_ids(
    plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
    inline_stats: &soac_opt::passes::TypedInlineRewriteStats,
    instr_ids_by_origin: &mut HashMap<InstrId, (RuntimeFunctionId, HashSet<InstrId>)>,
) {
    let instances_by_origin = inline_stats
        .inline_instance_sources
        .iter()
        .filter_map(|source| {
            let plan = plans.get(&source.source_instr_id)?;
            Some((source.inline_instance, plan))
        })
        .collect::<HashMap<_, _>>();
    let mut collected_instr_ids = 0usize;
    for mapping in &inline_stats.instr_id_mappings {
        let Some(plan) = instances_by_origin.get(&mapping.inline_instance) else {
            continue;
        };
        for generator_origin in typed_generator_resume_plan_state_origins(plan) {
            let entry = instr_ids_by_origin
                .entry(generator_origin)
                .or_insert_with(|| (plan.function_id, HashSet::new()));
            if entry.0 != plan.function_id {
                continue;
            }
            collected_instr_ids += usize::from(entry.1.insert(mapping.caller_instr_id));
        }
    }
    for synthetic in &inline_stats.synthetic_instr_ids {
        let Some(plan) = instances_by_origin.get(&synthetic.inline_instance) else {
            continue;
        };
        for generator_origin in typed_generator_resume_plan_state_origins(plan) {
            let entry = instr_ids_by_origin
                .entry(generator_origin)
                .or_insert_with(|| (plan.function_id, HashSet::new()));
            if entry.0 != plan.function_id {
                continue;
            }
            collected_instr_ids += usize::from(entry.1.insert(synthetic.instr_id));
        }
    }
    if !plans.is_empty() {
        tracing::info!(
            target: "soac_generator_state_lowering",
            resume_plan_count = plans.len(),
            inline_source_count = inline_stats.inline_instance_sources.len(),
            instr_id_mapping_count = inline_stats.instr_id_mappings.len(),
            synthetic_instr_id_count = inline_stats.synthetic_instr_ids.len(),
            matched_inline_instance_count = instances_by_origin.len(),
            collected_instr_ids,
            "typed_generator_state_lowering_instr_ids_collected",
        );
    }
}

fn propagate_generator_state_lowering_synthetic_instr_ids(
    inline_stats: &soac_opt::passes::TypedInlineRewriteStats,
    instr_ids_by_origin: &mut HashMap<InstrId, (RuntimeFunctionId, HashSet<InstrId>)>,
) -> usize {
    let source_instr_ids_by_instance = inline_stats
        .inline_instance_sources
        .iter()
        .map(|source| (source.inline_instance, source.source_instr_id))
        .collect::<HashMap<_, _>>();
    let mut propagated = 0;
    for synthetic in &inline_stats.synthetic_instr_ids {
        let Some(source_instr_id) = source_instr_ids_by_instance.get(&synthetic.inline_instance)
        else {
            continue;
        };
        for (_, (_, body_instr_ids)) in instr_ids_by_origin.iter_mut() {
            if body_instr_ids.contains(source_instr_id) {
                propagated += usize::from(body_instr_ids.insert(synthetic.instr_id));
            }
        }
    }
    propagated
}

fn propagate_generator_pending_alias_use_synthetic_instr_ids(
    inline_stats: &soac_opt::passes::TypedInlineRewriteStats,
    pending_alias_use_instr_ids_by_origin: &mut HashMap<InstrId, HashSet<InstrId>>,
) -> usize {
    let source_instr_ids_by_instance = inline_stats
        .inline_instance_sources
        .iter()
        .map(|source| (source.inline_instance, source.source_instr_id))
        .collect::<HashMap<_, _>>();
    let mut propagated = 0;
    for synthetic in &inline_stats.synthetic_instr_ids {
        let Some(source_instr_id) = source_instr_ids_by_instance.get(&synthetic.inline_instance)
        else {
            continue;
        };
        for pending_alias_use_instr_ids in pending_alias_use_instr_ids_by_origin.values_mut() {
            if pending_alias_use_instr_ids.contains(source_instr_id) {
                propagated += usize::from(pending_alias_use_instr_ids.insert(synthetic.instr_id));
            }
        }
    }
    propagated
}

fn remap_generator_capture_bindings_for_lowered_preserved_cells(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    bindings: &mut HashMap<u32, CellLocation>,
    preserved_locals: &HashMap<PreservedLocation, ResolvedName>,
) -> usize {
    let Some(layout) = function.storage_layout.as_ref() else {
        return 0;
    };
    let mut changed = 0;
    for location in bindings.values_mut() {
        let CellLocation::Preserved(slot) = *location else {
            continue;
        };
        let Some(local) = preserved_locals.get(&PreservedLocation(slot)) else {
            continue;
        };
        let Some(alias_slot) = layout
            .cellvars
            .iter()
            .position(|cellvar| cellvar.storage_name == local.id_str())
        else {
            continue;
        };
        tracing::info!(
            target: "soac_generator_state_lowering",
            function_id = ?function.function_id,
            preserved_slot = slot,
            preserved_local = local.id_str(),
            alias_slot,
            "typed_generator_constructor_capture_binding_remapped",
        );
        *location = CellLocation::Owned(
            u32::try_from(alias_slot).expect("preserved-cell alias slot should fit in u32"),
        );
        changed += 1;
    }
    changed
}

fn remap_inlined_generator_constructor_capture_bindings_for_lowered_state(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    generator_resume_plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
    inline_stats: &soac_opt::passes::TypedInlineRewriteStats,
    lowered_preserved_locals: &LoweredGeneratorPreservedLocals,
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
) -> usize {
    let instances_by_origin = inline_stats
        .inline_instance_sources
        .iter()
        .filter_map(|source| {
            let plan = generator_resume_plans.get(&source.source_instr_id)?;
            Some((
                source.inline_instance,
                typed_generator_resume_plan_state_origins(plan).collect::<Vec<_>>(),
            ))
        })
        .collect::<HashMap<_, _>>();
    let mut changed = 0;
    for mapping in &inline_stats.instr_id_mappings {
        let Some(generator_origins) = instances_by_origin.get(&mapping.inline_instance) else {
            continue;
        };
        for generator_origin in generator_origins {
            let Some(preserved_locals) = lowered_preserved_locals.get(generator_origin) else {
                continue;
            };
            let Some(bindings) =
                constructor_capture_bindings_by_origin.get_mut(&mapping.caller_instr_id)
            else {
                continue;
            };
            changed += remap_generator_capture_bindings_for_lowered_preserved_cells(
                function,
                bindings,
                preserved_locals,
            );
        }
    }
    changed
}

fn typed_generator_pending_alias_use_source_instr_ids_by_origin(
    plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
) -> HashMap<InstrId, HashSet<InstrId>> {
    plans.iter().fold(
        HashMap::<InstrId, HashSet<InstrId>>::new(),
        |mut sources_by_origin, (source, plan)| {
            for generator_origin in typed_generator_resume_plan_state_origins(plan) {
                sources_by_origin
                    .entry(generator_origin)
                    .or_default()
                    .insert(*source);
            }
            sources_by_origin
        },
    )
}

fn retain_typed_generator_pending_alias_use_instr_ids_by_origin(
    retained: &mut HashMap<InstrId, HashSet<InstrId>>,
    additional: HashMap<InstrId, HashSet<InstrId>>,
) {
    for (generator_origin, instr_ids) in additional {
        retained
            .entry(generator_origin)
            .or_default()
            .extend(instr_ids);
    }
}

fn retain_typed_generator_pending_alias_evidence_by_origin(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    retained: &mut HashMap<InstrId, HashSet<InstrId>>,
    plans: &HashMap<InstrId, TypedGeneratorResumePlan>,
) {
    retain_typed_generator_pending_alias_use_instr_ids_by_origin(
        retained,
        typed_generator_pending_alias_use_source_instr_ids_by_origin(plans),
    );
    let grouped_alias_use_instr_ids_by_origin =
        typed_generator_alias_ignored_instr_ids_by_origin(function, module_constants, retained);
    retain_typed_generator_pending_alias_use_instr_ids_by_origin(
        retained,
        grouped_alias_use_instr_ids_by_origin,
    );
}

fn typed_generator_state_lowering_plans(
    instr_ids_by_origin: HashMap<InstrId, (RuntimeFunctionId, HashSet<InstrId>)>,
    materialized_constructors: &HashMap<InstrId, TypedGeneratorStateConstructor>,
    pending_alias_use_instr_ids_by_origin: &HashMap<InstrId, HashSet<InstrId>>,
    alias_cleanup_active_blocks: Option<&HashSet<BlockLabel>>,
) -> Vec<TypedGeneratorStateLoweringPlan> {
    instr_ids_by_origin
        .into_iter()
        .map(
            |(generator_origin, (function_id, body_instr_ids))| TypedGeneratorStateLoweringPlan {
                generator_origin,
                function_id,
                body_instr_ids,
                pending_alias_use_instr_ids: pending_alias_use_instr_ids_by_origin
                    .get(&generator_origin)
                    .cloned()
                    .unwrap_or_default(),
                alias_cleanup_active_blocks: alias_cleanup_active_blocks.cloned(),
                materialized_constructor: materialized_constructors.get(&generator_origin).cloned(),
            },
        )
        .collect()
}

#[derive(Debug, Clone, Eq, PartialEq, Hash)]
struct TypedGeneratorStateLoweringAttemptKey {
    epoch: usize,
    generator_origin: InstrId,
    function_id: RuntimeFunctionId,
    body_instr_ids: Vec<InstrId>,
    pending_alias_use_instr_ids: Vec<InstrId>,
    alias_cleanup_active_blocks: Option<Vec<BlockLabel>>,
    has_materialized_constructor: bool,
    already_lowered: bool,
}

fn typed_generator_state_lowering_attempt_key(
    plan: &TypedGeneratorStateLoweringPlan,
    epoch: usize,
    already_lowered: bool,
) -> TypedGeneratorStateLoweringAttemptKey {
    let mut body_instr_ids = plan.body_instr_ids.iter().copied().collect::<Vec<_>>();
    body_instr_ids.sort_unstable();
    let mut pending_alias_use_instr_ids = plan
        .pending_alias_use_instr_ids
        .iter()
        .copied()
        .collect::<Vec<_>>();
    pending_alias_use_instr_ids.sort_unstable();
    let alias_cleanup_active_blocks = plan.alias_cleanup_active_blocks.as_ref().map(|blocks| {
        let mut blocks = blocks.iter().copied().collect::<Vec<_>>();
        blocks.sort_unstable();
        blocks
    });
    TypedGeneratorStateLoweringAttemptKey {
        epoch,
        generator_origin: plan.generator_origin,
        function_id: plan.function_id,
        body_instr_ids,
        pending_alias_use_instr_ids,
        alias_cleanup_active_blocks,
        has_materialized_constructor: plan.materialized_constructor.is_some(),
        already_lowered,
    }
}

fn lower_or_remap_typed_generator_state_for_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &mut Vec<ConstantExpr>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    plans: Vec<TypedGeneratorStateLoweringPlan>,
    lowering_attempt_epoch: usize,
    noop_attempts: Option<&mut HashSet<TypedGeneratorStateLoweringAttemptKey>>,
    lowered_preserved_locals: &mut LoweredGeneratorPreservedLocals,
) -> bool {
    let mut remapped_existing_instrs = 0;
    let mut remapped_existing_helper_calls = 0;
    let mut removed_reused_alias_setup = 0;
    let mut skipped_noop_plans = 0;
    let mut noop_attempts = noop_attempts;
    let mut fresh_plans = Vec::new();
    for plan in plans {
        let already_lowered = lowered_preserved_locals.contains_key(&plan.generator_origin);
        let attempt_key = typed_generator_state_lowering_attempt_key(
            &plan,
            lowering_attempt_epoch,
            already_lowered,
        );
        if noop_attempts
            .as_ref()
            .is_some_and(|attempts| attempts.contains(&attempt_key))
        {
            skipped_noop_plans += 1;
            continue;
        }
        if let Some(preserved_locals) = lowered_preserved_locals.get(&plan.generator_origin) {
            let next_remapped_existing_instrs =
                remap_typed_generator_preserved_instrs_with_existing_locals(
                    function,
                    &plan.body_instr_ids,
                    preserved_locals,
                );
            let mut next_remapped_existing_helper_calls = 0;
            let mut next_removed_reused_alias_setup = 0;
            if let Some(constructor) = plan.materialized_constructor.as_ref() {
                let preserved_locals_by_name = callee_module
                    .callable_defs
                    .iter()
                    .find(|callee| callee.function_id == plan.function_id)
                    .and_then(BlockPyFunction::public_storage_layout)
                    .map(|layout| {
                        layout
                            .preserved_slots
                            .iter()
                            .enumerate()
                            .filter_map(|(slot_index, slot)| {
                                preserved_locals
                                    .get(&PreservedLocation(
                                        u32::try_from(slot_index)
                                            .expect("preserved slot index should fit in u32"),
                                    ))
                                    .cloned()
                                    .map(|local| (slot.logical_name.clone(), local))
                            })
                            .collect::<HashMap<_, _>>()
                    })
                    .unwrap_or_default();
                next_remapped_existing_helper_calls +=
                    rewrite_lowered_typed_generator_state_helper_calls_with_existing_constructor(
                        function,
                        module_constants,
                        constructor,
                        &preserved_locals_by_name,
                        &plan.pending_alias_use_instr_ids,
                    );
                if plan.pending_alias_use_instr_ids.is_empty() {
                    next_removed_reused_alias_setup +=
                        cleanup_lowered_typed_generator_alias_setup_with_existing_constructor(
                            function,
                            module_constants,
                            constructor,
                            &HashSet::new(),
                        );
                }
            }
            if next_remapped_existing_instrs == 0
                && next_remapped_existing_helper_calls == 0
                && next_removed_reused_alias_setup == 0
            {
                if let Some(attempts) = noop_attempts.as_mut() {
                    attempts.insert(attempt_key);
                }
            }
            remapped_existing_instrs += next_remapped_existing_instrs;
            remapped_existing_helper_calls += next_remapped_existing_helper_calls;
            removed_reused_alias_setup += next_removed_reused_alias_setup;
        } else {
            fresh_plans.push((attempt_key, plan));
        }
    }

    let fresh_plan_count = fresh_plans.len();
    let fresh_lowering_plans = fresh_plans
        .iter()
        .map(|(_, plan)| plan.clone())
        .collect::<Vec<_>>();
    let outcome = lower_typed_generator_state_to_locals_with_plan_and_collect_preserved_locals(
        function,
        module_constants,
        callee_module,
        fresh_lowering_plans.as_slice(),
    );
    let lowered_origins = outcome
        .preserved_locals_by_origin
        .keys()
        .copied()
        .collect::<HashSet<_>>();
    if let Some(attempts) = noop_attempts.as_mut() {
        for (attempt_key, plan) in &fresh_plans {
            if !lowered_origins.contains(&plan.generator_origin) {
                attempts.insert(attempt_key.clone());
            }
        }
    }
    tracing::info!(
        target: "soac_generator_state_lowering",
        caller = ?function.function_id,
        fresh_plans = fresh_plan_count,
        reused_origins = lowered_preserved_locals.len(),
        skipped_noop_plans,
        remapped_existing_instrs,
        remapped_existing_helper_calls,
        removed_reused_alias_setup,
        newly_lowered_generators = outcome.stats.lowered_generators,
        newly_remapped_instrs = outcome.stats.remapped_instrs,
        "typed_generator_state_lower_or_remap_summary",
    );
    lowered_preserved_locals.extend(outcome.preserved_locals_by_origin);
    remapped_existing_instrs != 0
        || remapped_existing_helper_calls != 0
        || removed_reused_alias_setup != 0
        || outcome.stats.changed()
}

#[derive(Default)]
struct TypedPreservedNameCounts {
    preserved_name_count: usize,
    preserved_cell_name_count: usize,
    preserved_cell_ref_count: usize,
}

impl Visit<InstrTyped> for TypedPreservedNameCounts {
    fn visit_instr(&mut self, expr: &InstrTyped) {
        match expr {
            InstrTyped::Load(load) if load.name.preserved_location().is_some() => {
                self.preserved_name_count += 1;
            }
            InstrTyped::Store(store) if store.name.preserved_location().is_some() => {
                self.preserved_name_count += 1;
            }
            InstrTyped::Del(del) if del.name.preserved_location().is_some() => {
                self.preserved_name_count += 1;
            }
            InstrTyped::Load(load)
                if matches!(load.name.cell_location(), Some(CellLocation::Preserved(_))) =>
            {
                self.preserved_cell_name_count += 1;
            }
            InstrTyped::Store(store)
                if matches!(store.name.cell_location(), Some(CellLocation::Preserved(_))) =>
            {
                self.preserved_cell_name_count += 1;
            }
            InstrTyped::Del(del)
                if matches!(del.name.cell_location(), Some(CellLocation::Preserved(_))) =>
            {
                self.preserved_cell_name_count += 1;
            }
            InstrTyped::CellRef(op) if op.location.is_preserved() => {
                self.preserved_cell_ref_count += 1;
            }
            _ => {}
        }
        expr.visit_children(self);
    }
}

fn typed_preserved_name_counts(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> TypedPreservedNameCounts {
    let mut counter = TypedPreservedNameCounts::default();
    counter.visit_fn(function);
    counter
}

fn validate_typed_preserved_storage_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> Result<(), String> {
    if function
        .storage_layout
        .as_ref()
        .is_some_and(|layout| !layout.preserved_slots.is_empty())
    {
        return Ok(());
    }

    let counter = typed_preserved_name_counts(function);
    if counter.preserved_name_count == 0
        && counter.preserved_cell_name_count == 0
        && counter.preserved_cell_ref_count == 0
    {
        return Ok(());
    }

    Err(format!(
        "function {} ({}) has no preserved state but retained foreign generator preserved storage after typed rewrites: {} preserved names, {} preserved cell names, {} preserved cell refs",
        function.function_id,
        function.names.qualname,
        counter.preserved_name_count,
        counter.preserved_cell_name_count,
        counter.preserved_cell_ref_count,
    ))
}

fn trace_typed_preserved_name_count(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    pass: usize,
    stage: &'static str,
) {
    let counter = typed_preserved_name_counts(function);
    tracing::info!(
        target: "soac_generator_state_lowering",
        caller = ?function.function_id,
        pass,
        stage,
        preserved_name_count = counter.preserved_name_count,
        preserved_cell_name_count = counter.preserved_cell_name_count,
        preserved_cell_ref_count = counter.preserved_cell_ref_count,
        "typed_generator_state_preserved_name_count",
    );
}

#[cfg(test)]
fn trusted_runtime_protocol_calls_from_owner_states(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> HashMap<InstrId, TypedAttrOwnerRef> {
    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    trusted_runtime_protocol_calls_from_analysis(function, &states)
}

fn trusted_runtime_protocol_calls_from_analysis(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    states: &TrustedOwnerStateAnalysis,
) -> HashMap<InstrId, TypedAttrOwnerRef> {
    struct Collector<'a> {
        state: &'a TrustedOwnerState,
        calls: HashMap<InstrId, TypedAttrOwnerRef>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && matches!(
                    call.access,
                    soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod { .. }
                )
                && let Some(instr_id) = call.try_semantic_instr_id()
                && let Some(soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(
                    load,
                ))) = call.args.first()
                && let Some(owner_type_ref) = trusted_owner_state_for_name(&load.name, self.state)
            {
                self.calls.insert(instr_id, owner_type_ref.clone());
            }
            expr.visit_children(self);
        }
    }

    let mut calls = HashMap::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                continue;
            };
            let mut collector = Collector {
                state,
                calls: HashMap::new(),
            };
            collector.visit_instr(instr);
            calls.extend(collector.calls);
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            state,
            calls: HashMap::new(),
        };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
        calls.extend(collector.calls);
    }
    calls
}

#[cfg(test)]
fn trusted_static_runtime_protocol_inlines_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    static_targets: &StaticDirectCallTargets,
) -> (
    HashMap<InstrId, TypedAttrOwnerRef>,
    TypedInlineTargets,
    HashMap<InstrId, Vec<InstrId>>,
    HashMap<InstrId, RuntimeFunctionId>,
) {
    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    trusted_static_runtime_protocol_inlines_from_analysis(
        function,
        module_constants,
        &states,
        static_targets,
    )
}

fn trusted_static_runtime_protocol_inlines_from_analysis(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    states: &TrustedOwnerStateAnalysis,
    static_targets: &StaticDirectCallTargets,
) -> (
    HashMap<InstrId, TypedAttrOwnerRef>,
    TypedInlineTargets,
    HashMap<InstrId, Vec<InstrId>>,
    HashMap<InstrId, RuntimeFunctionId>,
) {
    struct Collector<'a> {
        function_id: RuntimeFunctionId,
        module_constants: &'a [ConstantExpr],
        state: &'a TrustedOwnerState,
        static_targets: &'a StaticDirectCallTargets,
        owners: HashMap<InstrId, TypedAttrOwnerRef>,
        inline_targets: TypedInlineTargets,
        closure_generator_next_sources: HashMap<InstrId, HashSet<InstrId>>,
        receiver_origins: HashMap<InstrId, Vec<InstrId>>,
        receiver_resume_functions: HashMap<InstrId, RuntimeFunctionId>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr {
                let mentions_next_name = matches!(
                    call.func.as_ref(),
                    InstrTyped::Load(load) if load.name.id_str() == RuntimeName::Next.name()
                );
                let Some(instr_id) = call.try_semantic_instr_id() else {
                    if mentions_next_name {
                        tracing::debug!(
                            target: "soac_generator_protocol_planning",
                            function_id = ?self.function_id,
                            func = ?call.func,
                            access = ?call.access,
                            "typed_generator_protocol_inline_skipped_missing_instr_id",
                        );
                    }
                    expr.visit_children(self);
                    return;
                };
                let runtime_name = match &call.access {
                    soac_ir_typed::TypedCallAccessPlan::Generic => {
                        [RuntimeName::Iter, RuntimeName::Next].into_iter().find_map(
                            |runtime_name| {
                                typed_expr_is_runtime_name_load(
                                    call.func.as_ref(),
                                    runtime_name,
                                    self.module_constants,
                                )
                                .then_some(runtime_name)
                            },
                        )
                    }
                    soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                        runtime_name,
                        ..
                    } if matches!(runtime_name, RuntimeName::Iter | RuntimeName::Next) => {
                        Some(*runtime_name)
                    }
                    _ => None,
                };
                let owner_type_ref = call.args.first().and_then(|arg| match arg {
                    soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(load)) => {
                        trusted_owner_state_for_name(&load.name, self.state)
                    }
                    _ => None,
                });
                let target =
                    runtime_name
                        .zip(owner_type_ref)
                        .and_then(|(runtime_name, owner_type_ref)| {
                            let TypedAttrOwnerRef::TypeKey {
                                module_name,
                                qualname,
                            } = owner_type_ref
                            else {
                                return None;
                            };
                            self.static_targets.strict_methods.get(&(
                                module_name.clone(),
                                qualname.clone(),
                                match runtime_name {
                                    RuntimeName::Iter => "__iter__".to_string(),
                                    RuntimeName::Next => "__next__".to_string(),
                                    _ => unreachable!(
                                        "only iter/next protocol names are synthesized here"
                                    ),
                                },
                            ))
                        });
                let Some(_) = runtime_name else {
                    if mentions_next_name {
                        tracing::debug!(
                            target: "soac_generator_protocol_planning",
                            function_id = ?self.function_id,
                            instr_id = ?instr_id,
                            func = ?call.func,
                            access = ?call.access,
                            "typed_generator_protocol_inline_skipped_untrusted_next_name",
                        );
                    }
                    expr.visit_children(self);
                    return;
                };
                let Some(owner_type_ref) = owner_type_ref else {
                    if runtime_name == Some(RuntimeName::Next) {
                        let receiver_name = call.args.first().and_then(|arg| match arg {
                            soac_core::block_py::CallArgPositional::Positional(
                                InstrTyped::Load(load),
                            ) => Some(&load.name),
                            _ => None,
                        });
                        let receiver_origin = receiver_name
                            .and_then(|name| trusted_object_origin_for_name(name, self.state));
                        tracing::debug!(
                            target: "soac_generator_protocol_planning",
                            function_id = ?self.function_id,
                            instr_id = ?instr_id,
                            receiver_name = receiver_name.map(|name| name.id_str()),
                            receiver_origin = ?receiver_origin,
                            args = ?call.args,
                            "typed_generator_protocol_inline_skipped_missing_owner_type",
                        );
                    }
                    expr.visit_children(self);
                    return;
                };
                let Some(target) = target else {
                    if runtime_name == Some(RuntimeName::Next) {
                        tracing::debug!(
                            target: "soac_generator_protocol_planning",
                            function_id = ?self.function_id,
                            instr_id = ?instr_id,
                            owner_type_ref = ?owner_type_ref,
                            "typed_generator_protocol_inline_skipped_missing_strict_method_target",
                        );
                    }
                    expr.visit_children(self);
                    return;
                };
                let has_starred_arguments = call
                    .args
                    .iter()
                    .any(|arg| matches!(arg, soac_core::block_py::CallArgPositional::Starred(_)));
                let explicit_positional_arg_count = call
                    .args
                    .iter()
                    .filter(|arg| {
                        matches!(arg, soac_core::block_py::CallArgPositional::Positional(_))
                    })
                    .count();
                if !call.keywords.is_empty() {
                    return;
                }
                let Ok(arg_plan) = super::direct_function::plan_direct_call_args_for_target(
                    target,
                    explicit_positional_arg_count,
                    0,
                    has_starred_arguments,
                    false,
                ) else {
                    return;
                };
                let arg_plan = typed_direct_call_arg_plan_from_direct_plan(arg_plan);
                if runtime_name == Some(RuntimeName::Next)
                    && matches!(
                        owner_type_ref,
                        TypedAttrOwnerRef::TypeKey {
                            module_name,
                            qualname,
                        } if module_name == "soac.runtime" && qualname == "ClosureGenerator"
                    )
                    && let Some(generator_origin) = call.args.first().and_then(|arg| match arg {
                        soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(
                            load,
                        )) => trusted_object_origin_for_name(&load.name, self.state),
                        _ => None,
                    })
                {
                    self.closure_generator_next_sources
                        .entry(generator_origin)
                        .or_default()
                        .insert(instr_id);
                    self.receiver_origins
                        .entry(instr_id)
                        .or_default()
                        .push(generator_origin);
                    let function_id = trusted_function_field_target_for_origin(
                        generator_origin,
                        "_resume_function",
                        self.state,
                    )
                    .or_else(|| {
                        call.args.first().and_then(|arg| match arg {
                            soac_core::block_py::CallArgPositional::Positional(
                                InstrTyped::Load(load),
                            ) => trusted_generator_resume_function_fact_for_name(
                                &load.name, self.state,
                            )
                            .map(|fact| fact.function_id),
                            _ => None,
                        })
                    });
                    if let Some(function_id) = function_id {
                        self.receiver_resume_functions.insert(instr_id, function_id);
                    }
                }
                self.owners.insert(instr_id, owner_type_ref.clone());
                self.inline_targets
                    .entry(instr_id)
                    .or_default()
                    .push((target.function_id, arg_plan));
            }
            expr.visit_children(self);
        }
    }

    let mut owners = HashMap::new();
    let mut inline_targets = HashMap::new();
    let mut closure_generator_next_sources = HashMap::<InstrId, HashSet<InstrId>>::new();
    let mut receiver_origins = HashMap::<InstrId, Vec<InstrId>>::new();
    let mut receiver_resume_functions = HashMap::<InstrId, RuntimeFunctionId>::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                continue;
            };
            let mut collector = Collector {
                function_id: function.function_id,
                module_constants,
                state,
                static_targets,
                owners: HashMap::new(),
                inline_targets: HashMap::new(),
                closure_generator_next_sources: HashMap::new(),
                receiver_origins: HashMap::new(),
                receiver_resume_functions: HashMap::new(),
            };
            collector.visit_instr(instr);
            owners.extend(collector.owners);
            inline_targets.extend(collector.inline_targets);
            for (origin, sources) in collector.closure_generator_next_sources {
                closure_generator_next_sources
                    .entry(origin)
                    .or_default()
                    .extend(sources);
            }
            for (instr_id, mut origins) in collector.receiver_origins {
                receiver_origins
                    .entry(instr_id)
                    .or_default()
                    .append(&mut origins);
            }
            receiver_resume_functions.extend(collector.receiver_resume_functions);
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            function_id: function.function_id,
            module_constants,
            state,
            static_targets,
            owners: HashMap::new(),
            inline_targets: HashMap::new(),
            closure_generator_next_sources: HashMap::new(),
            receiver_origins: HashMap::new(),
            receiver_resume_functions: HashMap::new(),
        };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
        owners.extend(collector.owners);
        inline_targets.extend(collector.inline_targets);
        for (origin, sources) in collector.closure_generator_next_sources {
            closure_generator_next_sources
                .entry(origin)
                .or_default()
                .extend(sources);
        }
        for (instr_id, mut origins) in collector.receiver_origins {
            receiver_origins
                .entry(instr_id)
                .or_default()
                .append(&mut origins);
        }
        receiver_resume_functions.extend(collector.receiver_resume_functions);
    }

    let materialized_owner_blocks = trusted_generator_alias_cleanup_active_blocks(states);
    let ignored_protocol_instr_ids_by_origin = typed_generator_alias_ignored_instr_ids_by_origin(
        function,
        module_constants,
        &closure_generator_next_sources,
    );
    let blocked_sources = closure_generator_next_sources
        .into_iter()
        .filter_map(|(generator_origin, sources)| {
            let can_lower_aliases = typed_generator_state_origin_can_lower_aliases_in_blocks(
                function,
                module_constants,
                generator_origin,
                ignored_protocol_instr_ids_by_origin
                    .get(&generator_origin)
                    .unwrap_or(&sources),
                Some(&materialized_owner_blocks),
            );
            tracing::debug!(
                target: "soac_generator_protocol_planning",
                function_id = ?function.function_id,
                generator_origin = ?generator_origin,
                source_count = sources.len(),
                sources = ?sources,
                can_lower_aliases,
                "typed_generator_protocol_alias_filter",
            );
            (!can_lower_aliases).then_some(sources)
        })
        .flatten()
        .collect::<HashSet<_>>();
    if !blocked_sources.is_empty() {
        owners.retain(|source, _| !blocked_sources.contains(source));
        inline_targets.retain(|source, _| !blocked_sources.contains(source));
        receiver_origins.retain(|source, _| !blocked_sources.contains(source));
        receiver_resume_functions.retain(|source, _| !blocked_sources.contains(source));
    }

    for origins in receiver_origins.values_mut() {
        origins.sort_by_key(|origin| origin.index());
        origins.dedup();
    }

    (
        owners,
        inline_targets,
        receiver_origins,
        receiver_resume_functions,
    )
}

type TypedLinearizedGetAttrDefs<'a> =
    HashMap<TypedBindingLocation, (&'a InstrTyped, &'a InstrTyped)>;

fn typed_call_func_get_attr_parts<'a>(
    func: &'a InstrTyped,
    linearized_get_attrs: &'a TypedLinearizedGetAttrDefs<'a>,
) -> Option<(&'a InstrTyped, &'a InstrTyped)> {
    match func {
        InstrTyped::GetAttrTyped(get_attr) => {
            Some((get_attr.value.as_ref(), get_attr.attr.as_ref()))
        }
        InstrTyped::Load(load) => typed_binding_location(&load.name)
            .and_then(|location| linearized_get_attrs.get(&location).copied()),
        _ => None,
    }
}

fn update_typed_linearized_get_attr_defs<'a>(
    defs: &mut TypedLinearizedGetAttrDefs<'a>,
    instr: &'a InstrTyped,
) {
    match instr {
        InstrTyped::Store(store) => {
            let Some(location) = typed_binding_location(&store.name) else {
                return;
            };
            if let InstrTyped::GetAttrTyped(get_attr) = store.value.as_ref() {
                defs.insert(location, (get_attr.value.as_ref(), get_attr.attr.as_ref()));
            } else {
                defs.remove(&location);
            }
        }
        InstrTyped::Del(del) => {
            if let Some(location) = typed_binding_location(&del.name) {
                defs.remove(&location);
            }
        }
        _ => {}
    }
}

fn trusted_static_method_inlines_from_analysis(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    states: &TrustedOwnerStateAnalysis,
    static_targets: &StaticDirectCallTargets,
) -> (
    HashMap<InstrId, TypedAttrOwnerRef>,
    TypedInlineTargets,
    HashMap<InstrId, Vec<InstrId>>,
    HashMap<InstrId, RuntimeFunctionId>,
) {
    struct Collector<'a> {
        module_constants: &'a [ConstantExpr],
        state: &'a TrustedOwnerState,
        static_targets: &'a StaticDirectCallTargets,
        linearized_get_attrs: &'a TypedLinearizedGetAttrDefs<'a>,
        owners: HashMap<InstrId, TypedAttrOwnerRef>,
        inline_targets: TypedInlineTargets,
        receiver_origins: HashMap<InstrId, Vec<InstrId>>,
        receiver_resume_functions: HashMap<InstrId, RuntimeFunctionId>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && matches!(call.access, soac_ir_typed::TypedCallAccessPlan::Generic)
                && let Some(instr_id) = call.try_semantic_instr_id()
                && let Some((get_attr_value, get_attr_attr)) =
                    typed_call_func_get_attr_parts(call.func.as_ref(), self.linearized_get_attrs)
                && let Some(method_name) =
                    typed_constant_string(get_attr_attr, self.module_constants)
                && let InstrTyped::Load(load) = get_attr_value
                && let Some(owner_type_ref) = trusted_owner_state_for_name(&load.name, self.state)
                && let TypedAttrOwnerRef::TypeKey {
                    module_name,
                    qualname,
                } = owner_type_ref
                && let Some(target) = self.static_targets.strict_methods.get(&(
                    module_name.clone(),
                    qualname.clone(),
                    method_name.to_string(),
                ))
            {
                let has_starred_arguments = call
                    .args
                    .iter()
                    .any(|arg| matches!(arg, soac_core::block_py::CallArgPositional::Starred(_)));
                let explicit_positional_arg_count = call
                    .args
                    .iter()
                    .filter(|arg| {
                        matches!(arg, soac_core::block_py::CallArgPositional::Positional(_))
                    })
                    .count();
                if !call.keywords.is_empty() {
                    return;
                }
                let Ok(arg_plan) = super::direct_function::plan_direct_call_args_for_target(
                    target,
                    explicit_positional_arg_count + 1,
                    0,
                    has_starred_arguments,
                    false,
                ) else {
                    return;
                };
                let arg_plan = typed_direct_call_arg_plan_from_direct_plan(arg_plan);
                if let Some(receiver_origin) =
                    trusted_object_origin_for_name(&load.name, self.state)
                {
                    self.receiver_origins
                        .entry(instr_id)
                        .or_default()
                        .push(receiver_origin);
                    if let Some(function_id) = trusted_function_field_target_for_origin(
                        receiver_origin,
                        "_resume_function",
                        self.state,
                    )
                    .or_else(|| {
                        trusted_generator_resume_function_fact_for_name(&load.name, self.state)
                            .map(|fact| fact.function_id)
                    }) {
                        self.receiver_resume_functions.insert(instr_id, function_id);
                    }
                }
                self.owners.insert(instr_id, owner_type_ref.clone());
                self.inline_targets
                    .entry(instr_id)
                    .or_default()
                    .push((target.function_id, arg_plan));
            }
            expr.visit_children(self);
        }
    }

    let mut owners = HashMap::new();
    let mut inline_targets = HashMap::new();
    let mut receiver_origins = HashMap::<InstrId, Vec<InstrId>>::new();
    let mut receiver_resume_functions = HashMap::<InstrId, RuntimeFunctionId>::new();
    for block in &function.blocks {
        let mut linearized_get_attrs = TypedLinearizedGetAttrDefs::new();
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                update_typed_linearized_get_attr_defs(&mut linearized_get_attrs, instr);
                continue;
            };
            let mut collector = Collector {
                module_constants,
                state,
                static_targets,
                linearized_get_attrs: &linearized_get_attrs,
                owners: HashMap::new(),
                inline_targets: HashMap::new(),
                receiver_origins: HashMap::new(),
                receiver_resume_functions: HashMap::new(),
            };
            collector.visit_instr(instr);
            owners.extend(collector.owners);
            inline_targets.extend(collector.inline_targets);
            for (instr_id, mut origins) in collector.receiver_origins {
                receiver_origins
                    .entry(instr_id)
                    .or_default()
                    .append(&mut origins);
            }
            receiver_resume_functions.extend(collector.receiver_resume_functions);
            update_typed_linearized_get_attr_defs(&mut linearized_get_attrs, instr);
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            module_constants,
            state,
            static_targets,
            linearized_get_attrs: &linearized_get_attrs,
            owners: HashMap::new(),
            inline_targets: HashMap::new(),
            receiver_origins: HashMap::new(),
            receiver_resume_functions: HashMap::new(),
        };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
        owners.extend(collector.owners);
        inline_targets.extend(collector.inline_targets);
        for (instr_id, mut origins) in collector.receiver_origins {
            receiver_origins
                .entry(instr_id)
                .or_default()
                .append(&mut origins);
        }
        receiver_resume_functions.extend(collector.receiver_resume_functions);
    }
    for origins in receiver_origins.values_mut() {
        origins.sort_by_key(|origin| origin.index());
        origins.dedup();
    }
    (
        owners,
        inline_targets,
        receiver_origins,
        receiver_resume_functions,
    )
}

fn trusted_field_callable_inlines_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
) -> Result<(TypedCallEmissionPlans, TypedInlineTargets), String> {
    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    trusted_field_callable_inlines_from_analysis(
        function,
        module_constants,
        &states,
        callee_module,
        external_callees,
    )
}

fn trusted_field_callable_inlines_from_analysis(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    states: &TrustedOwnerStateAnalysis,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
) -> Result<(TypedCallEmissionPlans, TypedInlineTargets), String> {
    struct Collector<'a> {
        module_constants: &'a [ConstantExpr],
        state: &'a TrustedOwnerState,
        callee_module: &'a BlockPyModule<TypedBlockPyModuleShape>,
        external_callees: &'a HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
        linearized_get_attrs: &'a TypedLinearizedGetAttrDefs<'a>,
        emissions: TypedCallEmissionPlans,
        inline_targets: TypedInlineTargets,
        error: Option<String>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.error.is_some() {
                return;
            }
            if let InstrTyped::CallTyped(call) = expr
                && matches!(call.access, soac_ir_typed::TypedCallAccessPlan::Generic)
                && call.keywords.is_empty()
                && let Some(instr_id) = call.try_semantic_instr_id()
                && let Some((get_attr_value, get_attr_attr)) =
                    typed_call_func_get_attr_parts(call.func.as_ref(), self.linearized_get_attrs)
                && let Some(field_name) =
                    typed_constant_string(get_attr_attr, self.module_constants)
                && let InstrTyped::Load(receiver) = get_attr_value
                && let Some(origin) = trusted_object_origin_for_name(&receiver.name, self.state)
                && !trusted_generator_origin_has_escaped(origin, self.state)
                && let Some(target_function_id) =
                    trusted_function_field_target_for_origin(origin, field_name, self.state)
                && let Some(target) = self
                    .callee_module
                    .callable_defs
                    .iter()
                    .find(|candidate| candidate.function_id == target_function_id)
                    .or_else(|| {
                        self.external_callees
                            .get(&target_function_id)
                            .map(|callee| &callee.function)
                    })
            {
                let has_starred_arguments = call
                    .args
                    .iter()
                    .any(|arg| matches!(arg, soac_core::block_py::CallArgPositional::Starred(_)));
                let explicit_positional_arg_count = call
                    .args
                    .iter()
                    .filter(|arg| {
                        matches!(arg, soac_core::block_py::CallArgPositional::Positional(_))
                    })
                    .count();
                let Ok(arg_plan) = super::direct_function::plan_direct_call_args_for_target(
                    target,
                    explicit_positional_arg_count,
                    0,
                    has_starred_arguments,
                    false,
                ) else {
                    expr.visit_children(self);
                    return;
                };
                let arg_plan = typed_direct_call_arg_plan_from_direct_plan(arg_plan);
                let plans = [ResolvedV3DirectCallPlan {
                    source: instr_id,
                    target: target_function_id,
                    callee: DirectCallCallee::Function,
                    arg_plan: arg_plan.clone(),
                    body: static_inline_call_body(),
                    reason: "trusted field callable".to_string(),
                }];
                if let Err(err) =
                    insert_static_direct_callable_plan(&mut self.emissions, instr_id, &plans)
                {
                    self.error = Some(err);
                    return;
                }
                self.inline_targets
                    .entry(instr_id)
                    .or_default()
                    .push((target_function_id, arg_plan));
            }
            expr.visit_children(self);
        }
    }

    let mut emissions = TypedCallEmissionPlans::default();
    let mut inline_targets = HashMap::new();
    for block in &function.blocks {
        let mut linearized_get_attrs = TypedLinearizedGetAttrDefs::new();
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                update_typed_linearized_get_attr_defs(&mut linearized_get_attrs, instr);
                continue;
            };
            let mut collector = Collector {
                module_constants,
                state,
                callee_module,
                external_callees,
                linearized_get_attrs: &linearized_get_attrs,
                emissions: TypedCallEmissionPlans::default(),
                inline_targets: HashMap::new(),
                error: None,
            };
            collector.visit_instr(instr);
            if let Some(err) = collector.error {
                return Err(err);
            }
            merge_typed_call_emission_plans(&mut emissions, &collector.emissions)?;
            inline_targets.extend(collector.inline_targets);
            update_typed_linearized_get_attr_defs(&mut linearized_get_attrs, instr);
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            module_constants,
            state,
            callee_module,
            external_callees,
            linearized_get_attrs: &linearized_get_attrs,
            emissions: TypedCallEmissionPlans::default(),
            inline_targets: HashMap::new(),
            error: None,
        };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
        if let Some(err) = collector.error {
            return Err(err);
        }
        merge_typed_call_emission_plans(&mut emissions, &collector.emissions)?;
        inline_targets.extend(collector.inline_targets);
    }
    Ok((emissions, inline_targets))
}

fn typed_function_has_runtime_generator_resume_call(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> bool {
    struct Finder<'a> {
        module_constants: &'a [ConstantExpr],
        found: bool,
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.found {
                return;
            }
            let func = match expr {
                InstrTyped::CallTyped(call) => Some(call.func.as_ref()),
                InstrTyped::GuardedCallableCallTyped(call) => Some(call.func.as_ref()),
                InstrTyped::DirectCallableCallTyped(call) => Some(call.func.as_ref()),
                _ => None,
            };
            if func.is_some_and(|func| {
                typed_expr_is_runtime_name_load(
                    func,
                    RuntimeName::ResumeGenerator,
                    self.module_constants,
                )
            }) {
                self.found = true;
                return;
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        module_constants,
        found: false,
    };
    finder.visit_fn(function);
    finder.found
}

fn runtime_protocol_call_instr_ids(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashSet<InstrId> {
    struct Collector {
        calls: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && matches!(
                    call.access,
                    soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod { .. }
                )
                && let Some(instr_id) = call.try_semantic_instr_id()
            {
                self.calls.insert(instr_id);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        calls: HashSet::new(),
    };
    collector.visit_fn(function);
    collector.calls
}

fn staged_inline_targets_for_trusted_runtime_protocols(
    mut inline_targets: TypedInlineTargets,
    runtime_protocol_calls: &HashSet<InstrId>,
    trusted_runtime_protocol_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_calls: Option<&HashMap<InstrId, TypedAttrOwnerRef>>,
    constructor_field_bindings: Option<&HashMap<InstrId, TypedConstructorFieldBindings>>,
    live_instr_ids: &HashSet<InstrId>,
) -> TypedInlineTargets {
    if runtime_protocol_calls.is_empty() {
        return inline_targets;
    }
    let pending_trusted_constructor_owners = trusted_constructor_calls
        .into_iter()
        .flat_map(|calls| calls.iter())
        .filter_map(|(source, owner)| {
            (live_instr_ids.contains(source)
                && !constructor_field_bindings
                    .is_some_and(|bindings| bindings.contains_key(source))
                && inline_targets.contains_key(source))
            .then_some(owner.clone())
        })
        .collect::<Vec<_>>();
    if !pending_trusted_constructor_owners.is_empty() {
        inline_targets.retain(|instr_id, _| {
            !runtime_protocol_calls.contains(instr_id)
                || trusted_runtime_protocol_calls
                    .get(instr_id)
                    .is_some_and(|owner| !pending_trusted_constructor_owners.contains(owner))
        });
        return inline_targets;
    }
    if !trusted_runtime_protocol_calls.is_empty() {
        inline_targets.retain(|instr_id, _| {
            !runtime_protocol_calls.contains(instr_id)
                || trusted_runtime_protocol_calls.contains_key(instr_id)
        });
    }
    inline_targets
}

fn refresh_materialized_generator_state_constructors_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    constructors_by_origin: &mut HashMap<InstrId, TypedGeneratorStateConstructor>,
) {
    for (generator_origin, constructor) in
        materialized_generator_state_constructors_for_function(function)
    {
        constructors_by_origin
            .entry(generator_origin)
            .and_modify(|existing| {
                existing.target = constructor.target.clone();
                existing.call = constructor.call.clone();
                if existing.closure_cell_bindings.is_none() {
                    existing.closure_cell_bindings = constructor.closure_cell_bindings.clone();
                }
            })
            .or_insert(constructor);
    }
}

fn split_post_inline_generator_alias_hot_continuations(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    hot_state_cleanup_labels: &mut HashSet<BlockLabel>,
    suppressed_generator_alias_hot_split_instr_ids: &mut HashSet<InstrId>,
) -> TypedHotContinuationSplitStats {
    let cloned_block_budget = MAX_TYPED_GENERATOR_ALIAS_CLONED_BLOCKS_PER_FUNCTION
        .min(typed_inline_remaining_cfg_blocks(function));
    let split_stats = split_typed_generator_alias_hot_continuations_with_budget(
        function,
        module_constants,
        suppressed_generator_alias_hot_split_instr_ids,
        cloned_block_budget,
    );
    suppressed_generator_alias_hot_split_instr_ids
        .extend(split_stats.alias_store_instr_ids.iter().copied());
    remap_cloned_hot_state_cleanup_labels(hot_state_cleanup_labels, &split_stats.label_mappings);
    split_stats
}

fn remap_cloned_suppressed_hot_split_instr_ids(
    suppressed_instr_ids: &mut HashSet<InstrId>,
    cloned_instr_id_mappings: &[TypedInlineInstrIdMapping],
) {
    if suppressed_instr_ids.is_empty() || cloned_instr_id_mappings.is_empty() {
        return;
    }
    for mapping in cloned_instr_id_mappings {
        if suppressed_instr_ids.contains(&mapping.callee_instr_id) {
            suppressed_instr_ids.insert(mapping.caller_instr_id);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn split_typed_post_inline_hot_continuations(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    profile: &SpecializationProfile<'_>,
    static_direct_calls: &StaticTypedDirectCalls,
    trusted_static_constructor_calls: &mut StaticConstructorCalls,
    hot_state_cleanup_labels: &mut HashSet<BlockLabel>,
    remapped_call_emissions: &mut RemappedTypedCallEmissions,
    remapped_inline_targets: &mut HashMap<RuntimeFunctionId, TypedInlineTargets>,
    remapped_generator_instance_plans: &mut RemappedTypedGeneratorInstancePlans,
    suppressed_inline_targets: &mut SuppressedTypedInlineTargets,
    remapped_indexed_fields: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    >,
    remapped_indexed_field_counter_sources: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedIndexedFieldCounterSource>,
    >,
    remapped_exact_list_items: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, ProfileExactListItemAccessPlan>,
    >,
    remapped_exact_int_branches: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedExactIntBranchPlan>,
    >,
    remapped_exact_int_returns: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedExactIntReturnPlan>,
    >,
    constructor_init_plans: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorInitPlan>,
    >,
    constructor_field_bindings: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorFieldBindings>,
    >,
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
    suppressed_alias_hot_split_instr_ids: &mut HashSet<InstrId>,
    suppressed_generator_alias_hot_split_instr_ids: &mut HashSet<InstrId>,
    generator_state_instr_ids_by_origin: &mut HashMap<
        InstrId,
        (RuntimeFunctionId, HashSet<InstrId>),
    >,
    generator_state_pending_alias_use_instr_ids_by_origin: &mut HashMap<InstrId, HashSet<InstrId>>,
) -> Result<TypedPostInlineHotContinuationSplitStats, String> {
    let caller_function_id = function.function_id;
    let constructor_cloned_block_budget = MAX_TYPED_CONSTRUCTOR_CLONED_BLOCKS_PER_FUNCTION
        .min(typed_inline_remaining_cfg_blocks(function));
    let constructor_split_stats = split_typed_constructor_hot_continuations_with_budget(
        function,
        module_constants,
        constructor_cloned_block_budget,
    );
    remap_cloned_hot_state_cleanup_labels(
        hot_state_cleanup_labels,
        &constructor_split_stats.label_mappings,
    );
    let alias_cloned_block_budget =
        MAX_TYPED_ALIAS_CLONED_BLOCKS_PER_FUNCTION.min(typed_inline_remaining_cfg_blocks(function));
    let alias_split_stats = split_typed_alias_hot_continuations_with_budget(
        function,
        module_constants,
        suppressed_alias_hot_split_instr_ids,
        alias_cloned_block_budget,
    );
    suppressed_alias_hot_split_instr_ids
        .extend(alias_split_stats.alias_store_instr_ids.iter().copied());
    remap_cloned_hot_state_cleanup_labels(
        hot_state_cleanup_labels,
        &alias_split_stats.label_mappings,
    );
    let generator_alias_split_stats = split_post_inline_generator_alias_hot_continuations(
        function,
        module_constants,
        hot_state_cleanup_labels,
        suppressed_generator_alias_hot_split_instr_ids,
    );
    for mappings in [
        constructor_split_stats.instr_id_mappings.as_slice(),
        alias_split_stats.instr_id_mappings.as_slice(),
        generator_alias_split_stats.instr_id_mappings.as_slice(),
    ] {
        remap_cloned_generator_state_lowering_instr_ids(
            caller_function_id,
            mappings,
            generator_state_instr_ids_by_origin,
        )?;
        remap_cloned_generator_pending_alias_use_instr_ids(
            caller_function_id,
            mappings,
            generator_state_pending_alias_use_instr_ids_by_origin,
        );
    }
    let mut cloned_instr_id_mappings = constructor_split_stats.instr_id_mappings;
    cloned_instr_id_mappings.extend(alias_split_stats.instr_id_mappings);
    cloned_instr_id_mappings.extend(generator_alias_split_stats.instr_id_mappings);
    remap_cloned_suppressed_hot_split_instr_ids(
        suppressed_alias_hot_split_instr_ids,
        &cloned_instr_id_mappings,
    );
    remap_cloned_suppressed_hot_split_instr_ids(
        suppressed_generator_alias_hot_split_instr_ids,
        &cloned_instr_id_mappings,
    );
    let split_stats = TypedPostInlineHotContinuationSplitStats {
        remapped_instr_ids: cloned_instr_id_mappings.len(),
        constructor_clones: constructor_split_stats.clones.len(),
        constructor_blocks: constructor_split_stats.cloned_blocks,
        alias_clones: alias_split_stats.clones.len(),
        alias_blocks: alias_split_stats.cloned_blocks,
        alias_store_instr_ids: alias_split_stats.alias_store_instr_ids.len(),
        generator_alias_clones: generator_alias_split_stats.clones.len(),
        generator_alias_blocks: generator_alias_split_stats.cloned_blocks,
        generator_alias_store_instr_ids: generator_alias_split_stats.alias_store_instr_ids.len(),
        alias_clone_roots: alias_split_stats
            .clones
            .iter()
            .map(|clone| {
                (
                    clone.hot_block,
                    clone.original_entry,
                    clone.cloned_entry,
                    clone.cloned_blocks,
                    clone.cyclic_hot_region,
                )
            })
            .collect(),
        generator_alias_clone_roots: generator_alias_split_stats
            .clones
            .iter()
            .map(|clone| {
                (
                    clone.hot_block,
                    clone.original_entry,
                    clone.cloned_entry,
                    clone.cloned_blocks,
                    clone.cyclic_hot_region,
                )
            })
            .collect(),
    };
    if cloned_instr_id_mappings.is_empty() {
        return Ok(split_stats);
    }
    remap_cloned_profile_rewrites(
        caller_function_id,
        &cloned_instr_id_mappings,
        profile,
        static_direct_calls,
        remapped_call_emissions,
        remapped_inline_targets,
        remapped_generator_instance_plans,
        suppressed_inline_targets,
        remapped_indexed_fields,
        remapped_indexed_field_counter_sources,
        remapped_exact_list_items,
        remapped_exact_int_branches,
        remapped_exact_int_returns,
        constructor_init_plans,
        constructor_field_bindings,
    )?;
    remap_cloned_generator_constructor_capture_bindings(
        caller_function_id,
        &cloned_instr_id_mappings,
        constructor_capture_bindings_by_origin,
    )?;
    if let Some(calls) = trusted_static_constructor_calls.get_mut(&caller_function_id) {
        remap_cloned_static_constructor_calls(calls, caller_function_id, &cloned_instr_id_mappings);
    }
    retire_cloned_inline_targets(
        caller_function_id,
        &cloned_instr_id_mappings,
        remapped_inline_targets,
        suppressed_inline_targets,
    );
    assign_missing_typed_function_instr_ids(function);
    refresh_typed_function_value_facts(function);
    Ok(split_stats)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TypedPostInlineHotContinuationSplitStats {
    remapped_instr_ids: usize,
    constructor_clones: usize,
    constructor_blocks: usize,
    alias_clones: usize,
    alias_blocks: usize,
    alias_store_instr_ids: usize,
    generator_alias_clones: usize,
    generator_alias_blocks: usize,
    generator_alias_store_instr_ids: usize,
    alias_clone_roots: Vec<(BlockLabel, BlockLabel, BlockLabel, usize, bool)>,
    generator_alias_clone_roots: Vec<(BlockLabel, BlockLabel, BlockLabel, usize, bool)>,
}

#[allow(clippy::too_many_arguments)]
fn split_typed_post_inline_cleanup_hot_continuations(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    static_direct_calls: &StaticTypedDirectCalls,
    trusted_static_constructor_calls: &mut StaticConstructorCalls,
    hot_state_cleanup_labels: &mut HashSet<BlockLabel>,
    remapped_call_emissions: &mut RemappedTypedCallEmissions,
    remapped_inline_targets: &mut HashMap<RuntimeFunctionId, TypedInlineTargets>,
    remapped_generator_instance_plans: &mut RemappedTypedGeneratorInstancePlans,
    suppressed_inline_targets: &mut SuppressedTypedInlineTargets,
    remapped_indexed_fields: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>,
    >,
    remapped_indexed_field_counter_sources: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedIndexedFieldCounterSource>,
    >,
    remapped_exact_list_items: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, ProfileExactListItemAccessPlan>,
    >,
    remapped_exact_int_branches: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedExactIntBranchPlan>,
    >,
    remapped_exact_int_returns: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedExactIntReturnPlan>,
    >,
    constructor_init_plans: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorInitPlan>,
    >,
    constructor_field_bindings: &mut HashMap<
        RuntimeFunctionId,
        HashMap<InstrId, TypedConstructorFieldBindings>,
    >,
    constructor_capture_bindings_by_origin: &mut HashMap<InstrId, HashMap<u32, CellLocation>>,
    generator_state_instr_ids_by_origin: &mut HashMap<
        InstrId,
        (RuntimeFunctionId, HashSet<InstrId>),
    >,
    generator_state_pending_alias_use_instr_ids_by_origin: &mut HashMap<InstrId, HashSet<InstrId>>,
) -> Result<usize, String> {
    let caller_function_id = function.function_id;
    let cloned_block_budget = MAX_TYPED_INLINE_CLEANUP_CLONED_BLOCKS_PER_FUNCTION
        .min(typed_inline_remaining_cfg_blocks(function));
    let split_stats = split_typed_inline_cleanup_hot_continuations_for_labels_with_budget(
        function,
        hot_state_cleanup_labels,
        cloned_block_budget,
    );
    for clone in &split_stats.clones {
        hot_state_cleanup_labels.remove(&clone.hot_block);
    }
    if split_stats.instr_id_mappings.is_empty() {
        return Ok(0);
    }
    remap_cloned_profile_rewrites(
        caller_function_id,
        &split_stats.instr_id_mappings,
        profile,
        static_direct_calls,
        remapped_call_emissions,
        remapped_inline_targets,
        remapped_generator_instance_plans,
        suppressed_inline_targets,
        remapped_indexed_fields,
        remapped_indexed_field_counter_sources,
        remapped_exact_list_items,
        remapped_exact_int_branches,
        remapped_exact_int_returns,
        constructor_init_plans,
        constructor_field_bindings,
    )?;
    remap_cloned_generator_state_lowering_instr_ids(
        caller_function_id,
        &split_stats.instr_id_mappings,
        generator_state_instr_ids_by_origin,
    )?;
    remap_cloned_generator_pending_alias_use_instr_ids(
        caller_function_id,
        &split_stats.instr_id_mappings,
        generator_state_pending_alias_use_instr_ids_by_origin,
    );
    remap_cloned_generator_constructor_capture_bindings(
        caller_function_id,
        &split_stats.instr_id_mappings,
        constructor_capture_bindings_by_origin,
    )?;
    if let Some(calls) = trusted_static_constructor_calls.get_mut(&caller_function_id) {
        remap_cloned_static_constructor_calls(
            calls,
            caller_function_id,
            &split_stats.instr_id_mappings,
        );
    }
    retire_cloned_inline_targets(
        caller_function_id,
        &split_stats.instr_id_mappings,
        remapped_inline_targets,
        suppressed_inline_targets,
    );
    assign_missing_typed_function_instr_ids(function);
    refresh_typed_function_value_facts(function);
    Ok(split_stats.instr_id_mappings.len())
}

#[cfg(test)]
pub(super) fn apply_profile_typed_plans_to_typed_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
) -> Result<(), String> {
    let Some(profile) = profile else {
        return Ok(());
    };
    apply_profile_call_emission_plans_to_typed_function(function, profile)?;
    apply_profile_access_and_scalar_plans_to_typed_function(
        function, profile, None, None, None, None, None, None,
    )?;
    apply_profile_typed_block_metadata_to_typed_function(function, profile)?;
    apply_profile_typed_guard_miss_policy_to_typed_function(function, profile);
    Ok(())
}

pub(crate) struct JitModulePlan {
    pub(super) module: Arc<BlockPyModule<TypedBlockPyModuleShape>>,
    pub(super) value_facts: FactStore,
    pub(super) locals: PlannedJitModuleLocals,
    pub(super) deopt_resume: PlannedJitDeoptResumeModule,
}

pub(super) fn collect_codegen_constants_for_module_name(
    module_name: &str,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
) -> ModuleCodegenConstants {
    if module_name == "soac.runtime" {
        ModuleCodegenConstants::collect_from_typed_runtime_module(module)
    } else {
        ModuleCodegenConstants::collect_from_typed_module(module)
    }
}

fn build_jit_module_plan_from_prepared_typed_module(
    prepared: PreparedJitTypedModulePlan,
) -> Result<Arc<JitModulePlan>, String> {
    for function in &prepared.module.callable_defs {
        trace_typed_preserved_name_count(function, usize::MAX, "before_jit_module_plan");
        trace_typed_inline_arg_load_uses(function, "before_jit_module_plan");
        validate_typed_function_value_facts(function)?;
    }
    Ok(Arc::new(JitModulePlan {
        module: Arc::new(prepared.module),
        value_facts: prepared.value_facts,
        locals: prepared.locals,
        deopt_resume: prepared.deopt_resume,
    }))
}

fn trace_typed_inline_arg_load_uses(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    stage: &'static str,
) {
    if !tracing::enabled!(target: "soac_inline_arg_load_uses", tracing::Level::INFO) {
        return;
    }

    fn traced_inline_temp(name: &ResolvedName) -> bool {
        name.id_str().starts_with("_dp_typed_inline_arg_")
    }

    struct Finder {
        names: HashSet<(String, Option<LocalLocation>)>,
    }

    impl Visit<InstrTyped> for Finder {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::Load(load) = expr
                && traced_inline_temp(&load.name)
            {
                self.names
                    .insert((load.name.id_str().to_string(), load.name.local_location()));
            }
            expr.visit_children(self);
        }
    }

    for block in &function.blocks {
        for instr in &block.body {
            if let InstrTyped::Store(store) = instr
                && traced_inline_temp(&store.name)
            {
                tracing::info!(
                    target: "soac_inline_arg_load_uses",
                    function_id = ?function.function_id,
                    stage,
                    block = ?block.label,
                    name = store.name.id_str(),
                    location = ?store.name.local_location(),
                    value_instr_id = ?store.value.try_semantic_instr_id(),
                    "typed_inline_arg_store_def",
                );
            }
            let mut finder = Finder {
                names: HashSet::new(),
            };
            finder.visit_instr(instr);
            if !finder.names.is_empty() {
                tracing::info!(
                    target: "soac_inline_arg_load_uses",
                    function_id = ?function.function_id,
                    stage,
                    block = ?block.label,
                    names = ?finder.names,
                    top_level_instr_id = ?instr.try_semantic_instr_id(),
                    "typed_inline_arg_load_use",
                );
            }
        }
        let mut finder = Finder {
            names: HashSet::new(),
        };
        finder.visit_term(&block.term);
        if !finder.names.is_empty() {
            tracing::info!(
                target: "soac_inline_arg_load_uses",
                function_id = ?function.function_id,
                stage,
                block = ?block.label,
                names = ?finder.names,
                top_level_kind = "block_term",
                "typed_inline_arg_load_use",
            );
        }
    }
}

pub(super) fn optimize_blockpy(
    module: &BlockPyModule<BlockPyModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
) -> Result<Arc<JitModulePlan>, String> {
    optimize_blockpy_with_external_inline_callees(
        module,
        profile,
        env_config,
        HashMap::new(),
        StaticDirectCallTargets::default(),
        false,
    )
}

pub(super) fn optimize_blockpy_for_shared_state(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
) -> Result<Arc<JitModulePlan>, String> {
    if let Some(cache_key) =
        shared_typed_module_plan_cache_key(shared_state, compile_session, profile, env_config)
    {
        return compile_session
            .expect("shared typed module plan cache key requires a compile session")
            .cached_shared_typed_module_plan(cache_key, || {
                build_shared_state_jit_module_plan(
                    shared_state,
                    compile_session,
                    profile,
                    env_config,
                )
            });
    }
    build_shared_state_jit_module_plan(shared_state, compile_session, profile, env_config)
}

fn shared_typed_module_plan_cache_key(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
) -> Option<SharedTypedModulePlanCacheKey> {
    let compile_session = compile_session?;
    let profile = profile?;
    let counter_dump_path = profile
        .counter_dump_path
        .as_ref()
        .map(|path| path.to_path_buf());
    let specialization_mode = env_config.specialization_mode()?;
    SharedTypedModulePlanCacheKey::new(
        shared_state.storage_instance_key(),
        compile_session.shared_module_registry_epoch(),
        counter_dump_path,
        specialization_mode,
        profile.behavior_change_indexed_stores,
        profile.profiled_cold_blocks,
        profile.guard_miss_deopt,
    )
}

fn build_shared_state_jit_module_plan(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
) -> Result<Arc<JitModulePlan>, String> {
    let total_start = Instant::now();
    // Keep profile mode on the original call graph so nested runtime protocol
    // sites still collect evidence before apply/verify rewrites inline them.
    let static_targets_start = Instant::now();
    let static_targets = if env_config.specialization_mode() == Some(SpecializationMode::Profile) {
        StaticDirectCallTargets::default()
    } else {
        static_direct_call_targets(shared_state, compile_session, env_config)?
    };
    let static_targets_elapsed = static_targets_start.elapsed();
    let external_callees_start = Instant::now();
    let external_callees = external_typed_inline_callees(
        shared_state,
        compile_session,
        profile,
        env_config,
        &static_targets,
    )?;
    let external_callee_count = external_callees.len();
    let external_callees_elapsed = external_callees_start.elapsed();
    let optimize_start = Instant::now();
    let plan = optimize_blockpy_with_external_inline_callees(
        &shared_state.lowered_module,
        profile,
        env_config,
        external_callees,
        static_targets,
        shared_state.opaque_fused_nqueens_source_matches(),
    )?;
    let optimize_elapsed = optimize_start.elapsed();
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.shared_typed_module_plan",
        module_name = shared_state.module_name.as_str(),
        specialization_mode = ?env_config.specialization_mode(),
        external_callee_count = u64::try_from(external_callee_count).unwrap_or(u64::MAX),
        shared_typed_plan_static_targets_us = duration_micros(static_targets_elapsed),
        shared_typed_plan_external_callees_us = duration_micros(external_callees_elapsed),
        shared_typed_plan_optimize_us = duration_micros(optimize_elapsed),
        shared_typed_plan_total_us = duration_micros(total_start.elapsed()),
        "shared_typed_module_plan",
    );
    Ok(plan)
}

fn optimize_blockpy_with_external_inline_callees(
    module: &BlockPyModule<BlockPyModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
    external_callees: HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    static_targets: StaticDirectCallTargets,
    opaque_fused_nqueens_source_matches: bool,
) -> Result<Arc<JitModulePlan>, String> {
    let total_start = Instant::now();
    let inline_plan_start = Instant::now();
    let inline_plan = profile.map(|_| plan_module_inlining(&summarize_module_escapes(module)));
    let inline_plan_elapsed = inline_plan_start.elapsed();
    let prepare_start = Instant::now();
    let prepared = soac_driver::typed_runtime::prepare_typed_v3_runtime_module_with_rewrites(
        module,
        env_config,
        |typed_module, _value_facts| {
            for function in &mut typed_module.callable_defs {
                assign_missing_typed_function_instr_ids(function);
            }
            let static_generator_instances =
                static_generator_instance_plans_for_module(typed_module, &static_targets);
            for function in &mut typed_module.callable_defs {
                annotate_typed_generator_instance_plans(
                    function,
                    static_generator_instances.get(&function.function_id),
                )?;
            }
            // Verify installs counter-recording vectorcalls on named source
            // generators, intentionally invalidating their CPython function
            // versions. The opaque entry guard therefore cannot activate in
            // that mode; admission is an Apply-only terminal specialization.
            let opaque_fused_admission =
                if env_config.specialization_mode() == Some(SpecializationMode::Apply) {
                    prepare_opaque_fused_count_admission(
                        typed_module,
                        &static_targets,
                        opaque_fused_nqueens_source_matches,
                    )?
                } else {
                    None
                };
            let opaque_fused_root_snapshot = opaque_fused_admission
                .as_ref()
                .map(|admission| {
                    typed_module
                        .callable_defs
                        .iter()
                        .find(|function| function.function_id == admission.root_function_id)
                        .cloned()
                        .ok_or_else(|| {
                            format!(
                                "opaque fused root {:?} disappeared before the rewrite snapshot",
                                admission.root_function_id
                            )
                        })
                })
                .transpose()?;
            if let Some(profile) = profile {
                let mut static_direct_calls =
                    static_direct_calls_for_module(typed_module, &static_targets);
                static_direct_calls.extend(static_direct_calls_for_external_callees(
                    &external_callees,
                    &static_targets,
                ));
                apply_typed_v3_module_rewrites(
                    typed_module,
                    profile,
                    inline_plan.as_ref(),
                    &external_callees,
                    &static_targets,
                    &static_direct_calls,
                )?;
            }
            if let (Some(admission), Some(root_snapshot)) =
                (&opaque_fused_admission, opaque_fused_root_snapshot)
            {
                let root = typed_module
                    .callable_defs
                    .iter_mut()
                    .find(|function| function.function_id == admission.root_function_id)
                    .ok_or_else(|| {
                        format!(
                            "opaque fused root {:?} disappeared during ordinary rewrites",
                            admission.root_function_id
                        )
                    })?;
                *root = root_snapshot;
                super::opaque_fused_iteration::attach_admitted_opaque_fused_count(
                    typed_module,
                    admission,
                )?;
                super::opaque_fused_iteration::validate_attached_opaque_fused_count_is_atomic(
                    typed_module,
                    admission,
                )?;
            }
            Ok(())
        },
    )?;
    let prepare_elapsed = prepare_start.elapsed();
    let typed_plan_start = Instant::now();
    let typed_plan = plan_jit_typed_module_with_runtime_replay_module(
        prepared.module,
        prepared.value_facts,
        Some(module),
    )?;
    let typed_plan_elapsed = typed_plan_start.elapsed();
    let finalize_start = Instant::now();
    let plan = build_jit_module_plan_from_prepared_typed_module(typed_plan)?;
    let finalize_elapsed = finalize_start.elapsed();
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.typed_module_plan",
        runtime_module_id = module.module_name_gen.runtime_module_id().as_u32(),
        function_count = u64::try_from(module.callable_defs.len()).unwrap_or(u64::MAX),
        profile_enabled = profile.is_some(),
        typed_plan_inline_plan_us = duration_micros(inline_plan_elapsed),
        typed_plan_prepare_us = duration_micros(prepare_elapsed),
        typed_plan_plan_jit_us = duration_micros(typed_plan_elapsed),
        typed_plan_finalize_us = duration_micros(finalize_elapsed),
        typed_plan_total_us = duration_micros(total_start.elapsed()),
        "typed_module_plan",
    );
    Ok(plan)
}

fn external_typed_inline_callees(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
    static_targets: &StaticDirectCallTargets,
) -> Result<HashMap<RuntimeFunctionId, TypedExternalInlineCallee>, String> {
    let (Some(compile_session), Some(profile)) = (compile_session, profile) else {
        return Ok(HashMap::new());
    };
    let current_module_id = shared_state.lowered_module.module_name_gen.module_id();
    let mut targets = profile
        .opt_v3_emitted_direct_calls
        .values()
        .flat_map(|calls| calls.values())
        .flat_map(|plans| plans.iter())
        .map(|plan| plan.target)
        .filter(|function_id| function_id.runtime_module_id().as_u32() != current_module_id)
        .collect::<HashSet<_>>();
    targets.extend(
        static_targets
            .runtime_names
            .values()
            .map(|target| target.function.function_id)
            .filter(|function_id| function_id.runtime_module_id().as_u32() != current_module_id),
    );
    targets.extend(
        static_targets
            .runtime_builtin_implementations
            .values()
            .map(|target| target.function_id)
            .filter(|function_id| function_id.runtime_module_id().as_u32() != current_module_id),
    );
    targets.extend(
        static_targets
            .strict_methods
            .values()
            .map(|target| target.function_id)
            .filter(|function_id| function_id.runtime_module_id().as_u32() != current_module_id),
    );
    targets.retain(|function_id| {
        !static_targets
            .suppressed_source_generators
            .contains(function_id)
    });
    if targets.is_empty() {
        return Ok(HashMap::new());
    }

    let mut by_module = HashMap::<u32, Arc<SharedModuleState>>::new();
    for function_id in &targets {
        let Some(target_state) =
            compile_session.shared_module_state_for_function_id(*function_id)?
        else {
            continue;
        };
        by_module
            .entry(function_id.runtime_module_id().as_u32())
            .or_insert(target_state);
    }

    let mut external_callees = HashMap::new();
    for target_state in by_module.into_values() {
        let inline_plan =
            plan_module_inlining(&summarize_module_escapes(&target_state.lowered_module));
        let prepared = soac_driver::typed_runtime::prepare_typed_v3_runtime_module_with_rewrites(
            &target_state.lowered_module,
            env_config,
            |typed_module, _value_facts| {
                let static_direct_calls =
                    static_direct_calls_for_module(typed_module, static_targets);
                let static_generator_instances =
                    static_generator_instance_plans_for_module(typed_module, static_targets);
                for function in &mut typed_module.callable_defs {
                    annotate_typed_generator_instance_plans(
                        function,
                        static_generator_instances.get(&function.function_id),
                    )?;
                    apply_call_emission_plans_to_typed_function_with_static_targets(
                        function,
                        profile,
                        static_direct_calls.get(&function.function_id),
                        static_targets,
                    )?;
                }
                Ok(())
            },
        )?;
        let module_constants = prepared.module.module_constants.clone();
        let constructor_init_targets = prepared
            .module
            .callable_defs
            .iter()
            .filter_map(|function| {
                targets
                    .contains(&function.function_id)
                    .then(|| constructor_init_function_id_for_entry_function(function))
                    .flatten()
            })
            .collect::<HashSet<_>>();
        for function in prepared.module.callable_defs {
            if targets.remove(&function.function_id)
                || constructor_init_targets.contains(&function.function_id)
            {
                external_callees.insert(
                    function.function_id,
                    TypedExternalInlineCallee {
                        function,
                        module_constants: module_constants.clone(),
                        inline_plan: Some(inline_plan.clone()),
                    },
                );
            }
        }
    }
    Ok(external_callees)
}

#[derive(Default)]
struct TrustedTypedInlineWork {
    trusted_runtime_protocol_calls: HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_runtime_protocol_receiver_origins: HashMap<InstrId, Vec<InstrId>>,
    trusted_runtime_protocol_resume_functions: HashMap<InstrId, RuntimeFunctionId>,
    static_protocol_inline_targets: TypedInlineTargets,
    static_method_inline_targets: TypedInlineTargets,
    static_field_callable_inline_targets: TypedInlineTargets,
    generator_resume_plans: HashMap<InstrId, TypedGeneratorResumePlan>,
    generator_resume_alias_cleanup_active_blocks: HashSet<BlockLabel>,
    builtin_implementation_plans: HashMap<InstrId, TypedBuiltinImplementationPlan>,
    timings: TrustedTypedInlineWorkTimings,
}

#[derive(Default)]
struct TrustedTypedInlineWorkTimings {
    trusted_plan: Duration,
    owner_state: Duration,
    runtime_protocol: Duration,
    static_protocol: Duration,
    static_method: Duration,
    field_callable: Duration,
    builtin_plan: Duration,
}

fn prepare_trusted_typed_inline_work_for_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    trusted_owner_state_cache: &mut TrustedOwnerStateCache,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    static_targets: &StaticDirectCallTargets,
    local_generators: &HashMap<RuntimeFunctionId, &BlockPyFunction<TypedBlockPyModuleShape>>,
    remapped_generator_instance_plans: Option<&HashMap<InstrId, TypedGeneratorInstancePlan>>,
    retained_pending_alias_use_source_instr_ids_by_origin: &HashMap<InstrId, HashSet<InstrId>>,
) -> Result<TrustedTypedInlineWork, String> {
    let trusted_plan_start = Instant::now();
    let linearization = linearize_typed_function_expressions(function).map_err(|reason| {
        format!(
            "typed expression linearization failed for {}: {reason:?}",
            function.names.qualname
        )
    })?;
    if linearization.lifted_nested_exprs != 0 {
        trusted_owner_state_cache.invalidate();
        assign_missing_typed_function_instr_ids(function);
        refresh_typed_function_value_facts(function);
        let mut refreshed_generator_instance_plans =
            static_generator_instance_plans_for_function(function, static_targets);
        refreshed_generator_instance_plans.extend(
            static_local_generator_instance_plans_for_function(function, local_generators),
        );
        if let Some(remapped) = remapped_generator_instance_plans {
            refreshed_generator_instance_plans.extend(remapped.clone());
        }
        annotate_typed_generator_instance_plans(
            function,
            Some(&refreshed_generator_instance_plans),
        )?;
        tracing::debug!(
            target: "soac_typed_linearization",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            rewritten_body_roots = linearization.rewritten_body_roots,
            rewritten_terms = linearization.rewritten_terms,
            lifted_nested_exprs = linearization.lifted_nested_exprs,
            "typed_expression_linearization",
        );
    }
    let owner_state_start = Instant::now();
    let trusted_owner_states = trusted_owner_state_cache.states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    let owner_state = owner_state_start.elapsed();

    let runtime_protocol_start = Instant::now();
    let mut trusted_runtime_protocol_calls =
        trusted_runtime_protocol_calls_from_analysis(function, &trusted_owner_states);
    let runtime_protocol = runtime_protocol_start.elapsed();

    let static_protocol_start = Instant::now();
    let (
        static_protocol_calls,
        static_protocol_inline_targets,
        mut trusted_runtime_protocol_receiver_origins,
        mut trusted_runtime_protocol_resume_functions,
    ) = trusted_static_runtime_protocol_inlines_from_analysis(
        function,
        module_constants,
        &trusted_owner_states,
        static_targets,
    );
    trusted_runtime_protocol_calls.extend(static_protocol_calls);
    let static_protocol = static_protocol_start.elapsed();

    let static_method_start = Instant::now();
    let (
        trusted_static_method_calls,
        static_method_inline_targets,
        static_method_receiver_origins,
        static_method_receiver_resume_functions,
    ) = trusted_static_method_inlines_from_analysis(
        function,
        module_constants,
        &trusted_owner_states,
        static_targets,
    );
    trusted_runtime_protocol_calls.extend(trusted_static_method_calls);
    for (instr_id, mut origins) in static_method_receiver_origins {
        let receiver_origins = trusted_runtime_protocol_receiver_origins
            .entry(instr_id)
            .or_default();
        receiver_origins.append(&mut origins);
        receiver_origins.sort_by_key(|origin| origin.index());
        receiver_origins.dedup();
    }
    trusted_runtime_protocol_resume_functions.extend(static_method_receiver_resume_functions);
    let static_method = static_method_start.elapsed();

    let field_callable_start = Instant::now();
    let (field_callable_emissions, static_field_callable_inline_targets) =
        trusted_field_callable_inlines_from_analysis(
            function,
            module_constants,
            &trusted_owner_states,
            callee_module,
            external_callees,
        )?;
    let field_callable_mutated = !field_callable_emissions.is_empty();
    if field_callable_mutated {
        lower_typed_function_call_emission_plans(function, &field_callable_emissions)?;
        refresh_typed_function_value_facts(function);
    }
    let field_callable = field_callable_start.elapsed();
    let trusted_plan = trusted_plan_start.elapsed();

    let builtin_plan_start = Instant::now();
    let generator_resume_plans = trusted_generator_resume_plans_from_analysis(
        function,
        module_constants,
        &trusted_owner_states,
        retained_pending_alias_use_source_instr_ids_by_origin,
    )
    .into_iter()
    .filter(|(_, plan)| typed_generator_resume_plan_state_origin(plan).is_some())
    .collect::<HashMap<_, _>>();
    annotate_typed_generator_resume_plans(function, &generator_resume_plans)?;
    let builtin_implementation_plans = trusted_generator_builtin_implementation_plans_from_analysis(
        function,
        callee_module,
        external_callees,
        module_constants,
        &trusted_owner_states,
        static_targets,
    );
    trace_builtin_implementation_plan_placements(function, &builtin_implementation_plans);
    let builtin_plan = builtin_plan_start.elapsed();
    let generator_resume_alias_cleanup_active_blocks =
        trusted_generator_alias_cleanup_active_blocks(&trusted_owner_states);
    if field_callable_mutated {
        trusted_owner_state_cache.invalidate();
    }

    Ok(TrustedTypedInlineWork {
        trusted_runtime_protocol_calls,
        trusted_runtime_protocol_receiver_origins,
        trusted_runtime_protocol_resume_functions,
        static_protocol_inline_targets,
        static_method_inline_targets,
        static_field_callable_inline_targets,
        generator_resume_plans,
        generator_resume_alias_cleanup_active_blocks,
        builtin_implementation_plans,
        timings: TrustedTypedInlineWorkTimings {
            trusted_plan,
            owner_state,
            runtime_protocol,
            static_protocol,
            static_method,
            field_callable,
            builtin_plan,
        },
    })
}

/// Prepare exact-source opaque fusion without mutating the production module.
/// Source-backed generators remain suppressed on the ordinary inline path;
/// only this discovery clone exposes the complete graph for transactional
/// semantic admission and typed-sidecar resolution.
fn prepare_opaque_fused_count_admission(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    static_targets: &StaticDirectCallTargets,
    exact_source_match: bool,
) -> Result<Option<super::opaque_fused_iteration::AdmittedOpaqueFusedCount>, String> {
    let mut discovery_module = module.clone();
    for function in &mut discovery_module.callable_defs {
        assign_missing_typed_function_instr_ids(function);
    }
    let mut discovery_targets = static_targets.clone();
    discovery_targets.suppressed_source_generators.clear();
    let plans = static_generator_instance_plans_for_module(&discovery_module, &discovery_targets);
    for function in &mut discovery_module.callable_defs {
        annotate_typed_generator_instance_plans(function, plans.get(&function.function_id))?;
    }
    super::opaque_fused_iteration::admit_tracked_nqueens_count(
        &discovery_module,
        exact_source_match,
    )
}

fn apply_typed_v3_module_rewrites(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    inline_plan: Option<&soac_opt::passes::InlinePlanModule>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    static_targets: &StaticDirectCallTargets,
    static_direct_calls: &StaticTypedDirectCalls,
) -> Result<(), String> {
    let total_start = Instant::now();
    let setup_start = Instant::now();
    let field_counter_function_ids = module
        .counter_defs
        .iter()
        .filter(|counter| counter.kind == "field_access")
        .filter_map(|counter| match &counter.site {
            CounterSite::Runtime {
                function_id: Some(function_id),
                ..
            } => Some(*function_id),
            _ => None,
        })
        .collect::<HashSet<_>>();
    let mut static_constructor_calls = static_constructor_calls_for_module(module, static_targets);
    static_constructor_calls.extend(static_constructor_calls_for_external_callees(
        external_callees,
        static_targets,
    ));
    let trusted_constructor_init_owners = trusted_constructor_init_owner_refs(static_targets);
    let mut trusted_static_constructor_calls =
        trusted_static_constructor_calls(&static_constructor_calls);
    for function in &mut module.callable_defs {
        apply_call_emission_plans_to_typed_function_with_static_targets(
            function,
            profile,
            static_direct_calls.get(&function.function_id),
            static_targets,
        )?;
    }
    for function in &mut module.callable_defs {
        assign_missing_typed_function_instr_ids(function);
    }

    let callee_module = module.clone();
    let mut constructor_capture_bindings_by_function = callee_module
        .callable_defs
        .iter()
        .map(|function| {
            (
                function.function_id,
                typed_generator_constructor_capture_bindings_by_origin(function),
            )
        })
        .collect::<HashMap<_, _>>();
    constructor_capture_bindings_by_function.extend(external_callees.iter().map(
        |(&function_id, callee)| {
            (
                function_id,
                typed_generator_constructor_capture_bindings_by_origin(&callee.function),
            )
        },
    ));
    let local_generators = local_generator_targets_for_module(&callee_module)
        .into_iter()
        .filter(|(function_id, _)| {
            !static_targets
                .suppressed_source_generators
                .contains(function_id)
        })
        .collect();
    let synthetic_genexpr_function_ids = synthetic_genexpr_function_ids_for_module(&callee_module);
    let suppressed_source_generator_protocol_inline_targets = static_targets
        .strict_methods
        .iter()
        .filter_map(|((module_name, owner_qualname, method_name), function)| {
            (module_name == "soac.runtime"
                && owner_qualname == "ClosureGenerator"
                && matches!(method_name.as_str(), "__iter__" | "__next__"))
            .then_some(function.function_id)
        })
        .collect::<HashSet<_>>();
    let trusted_generator_bridge_targets =
        trusted_generator_protocol_bridge_targets(static_targets);
    let mut remapped_call_emissions = RemappedTypedCallEmissions::new();
    let mut remapped_inline_targets = HashMap::<RuntimeFunctionId, TypedInlineTargets>::new();
    let mut remapped_indexed_fields =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, Vec<OptV3ResolvedIndexedFieldAccess>>>::new();
    let mut remapped_indexed_field_counter_sources =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedIndexedFieldCounterSource>>::new();
    let mut remapped_exact_list_items =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, ProfileExactListItemAccessPlan>>::new();
    let mut remapped_exact_int_branches =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>::new();
    let mut remapped_exact_int_returns =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>::new();
    let mut remapped_generator_instance_plans = RemappedTypedGeneratorInstancePlans::new();
    let mut suppressed_inline_targets = SuppressedTypedInlineTargets::new();
    let mut constructor_init_plans =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedConstructorInitPlan>>::new();
    let mut constructor_field_bindings =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedConstructorFieldBindings>>::new();
    let setup_elapsed = setup_start.elapsed();
    let function_rewrites_start = Instant::now();
    for function in &mut module.callable_defs {
        let function_total_start = Instant::now();
        let pre_inline_start = Instant::now();
        let mut constructor_capture_bindings_by_origin = constructor_capture_bindings_by_function
            .get(&function.function_id)
            .cloned()
            .unwrap_or_default();
        let resume_state_outcome =
            lower_typed_generator_resume_preserved_state_to_locals_and_collect_preserved_locals(
                function,
            );
        let resume_state_stats = &resume_state_outcome.stats;
        let resume_preserved_locals = resume_state_outcome.preserved_locals;
        if resume_state_stats.changed() {
            tracing::info!(
                target: "soac_generator_resume_state_lowering",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                lowered_slots = resume_state_stats.lowered_slots,
                entry_transfers = resume_state_stats.entry_transfers,
                boundary_writebacks = resume_state_stats.boundary_writebacks,
                remapped_instrs = resume_state_stats.remapped_instrs,
                "typed_generator_resume_state_lowered_to_locals",
            );
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
        }
        let mut hot_state_cleanup_labels = HashSet::<BlockLabel>::new();
        let mut generator_state_instr_ids_by_origin =
            HashMap::<InstrId, (RuntimeFunctionId, HashSet<InstrId>)>::new();
        let mut generator_state_pending_alias_use_instr_ids_by_origin =
            HashMap::<InstrId, HashSet<InstrId>>::new();
        let mut generator_state_constructors_by_origin =
            HashMap::<InstrId, TypedGeneratorStateConstructor>::new();
        let mut lowered_generator_preserved_locals = LoweredGeneratorPreservedLocals::new();
        let mut generator_state_lowering_attempt_epoch = 0usize;
        let mut noop_generator_state_lowering_attempts =
            HashSet::<TypedGeneratorStateLoweringAttemptKey>::new();
        let mut inline_trusted_owner_states = TrustedOwnerStateCache::default();
        seed_profile_exact_int_selections_for_function(
            function,
            profile,
            &mut remapped_exact_int_branches,
            &mut remapped_exact_int_returns,
        )?;
        let pre_inline_elapsed = pre_inline_start.elapsed();
        let inline_loop_start = Instant::now();
        let mut inline_pass_count = 0usize;
        let mut inline_target_build_elapsed = Duration::ZERO;
        let mut inline_idle_split_elapsed = Duration::ZERO;
        let mut inline_rewrite_elapsed = Duration::ZERO;
        let mut inline_state_lowering_elapsed = Duration::ZERO;
        let mut inline_constructor_init_elapsed = Duration::ZERO;
        let mut inline_sidecar_remap_elapsed = Duration::ZERO;
        let mut inline_refresh_elapsed = Duration::ZERO;
        let mut inline_constructor_scan_elapsed = Duration::ZERO;
        let mut inline_generator_plan_elapsed = Duration::ZERO;
        let mut inline_generator_static_elapsed = Duration::ZERO;
        let mut inline_generator_static_local_elapsed = Duration::ZERO;
        let mut inline_generator_remap_elapsed = Duration::ZERO;
        let mut inline_generator_annotate_elapsed = Duration::ZERO;
        let mut inline_trusted_plan_elapsed = Duration::ZERO;
        let mut inline_trusted_owner_state_elapsed = Duration::ZERO;
        let mut inline_trusted_runtime_protocol_elapsed = Duration::ZERO;
        let mut inline_trusted_static_protocol_elapsed = Duration::ZERO;
        let mut inline_trusted_static_method_elapsed = Duration::ZERO;
        let mut inline_trusted_field_callable_elapsed = Duration::ZERO;
        let mut inline_builtin_plan_elapsed = Duration::ZERO;
        let mut inline_target_collect_elapsed = Duration::ZERO;
        let mut inline_target_stage_elapsed = Duration::ZERO;
        let mut inline_tuple_simplify_elapsed = Duration::ZERO;
        let mut inline_value_fact_refresh_elapsed = Duration::ZERO;
        let mut inline_call_emission_lower_elapsed = Duration::ZERO;
        let mut inline_post_split_elapsed = Duration::ZERO;
        let mut inline_progress = Vec::new();
        let mut suppressed_alias_hot_split_instr_ids = HashSet::new();
        let mut suppressed_generator_alias_hot_split_instr_ids = HashSet::new();
        let mut retried_final_generator_plan_refresh = false;
        let mut retried_idle_hot_continuation_split = false;
        let mut inline_rewrite_pass_count = 0usize;
        let mut inline_maintenance_pass_count = 0usize;
        loop {
            if !typed_inline_function_within_cfg_budget(function) {
                tracing::info!(
                    target: "soac_inline_budget",
                    function_id = ?function.function_id,
                    function_qualname = %function.names.qualname,
                    block_count = function.blocks.len(),
                    max_blocks = MAX_TYPED_INLINE_FUNCTION_BLOCKS,
                    max_body_instrs = MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS,
                    "typed_inline_fixpoint_reached_function_cfg_budget",
                );
                break;
            }
            let pass = inline_pass_count;
            inline_pass_count += 1;
            let inline_target_build_start = Instant::now();
            let inline_constructor_scan_start = Instant::now();
            refresh_typed_generator_inline_sidecars_for_function(
                function,
                &mut generator_state_constructors_by_origin,
                &mut constructor_capture_bindings_by_origin,
            );
            trace_materialized_generator_state_constructor_anchors(
                function,
                pass,
                "pass_start",
                &generator_state_constructors_by_origin,
            );
            inline_constructor_scan_elapsed += inline_constructor_scan_start.elapsed();
            let inline_generator_plan_start = Instant::now();
            let inline_generator_static_start = Instant::now();
            let mut refreshed_generator_instance_plans =
                static_generator_instance_plans_for_function(function, static_targets);
            inline_generator_static_elapsed += inline_generator_static_start.elapsed();
            let inline_generator_static_local_start = Instant::now();
            refreshed_generator_instance_plans.extend(
                static_local_generator_instance_plans_for_function(function, &local_generators),
            );
            inline_generator_static_local_elapsed += inline_generator_static_local_start.elapsed();
            let inline_generator_remap_start = Instant::now();
            if let Some(remapped) = remapped_generator_instance_plans.get(&function.function_id) {
                refreshed_generator_instance_plans.extend(remapped.clone());
            }
            inline_generator_remap_elapsed += inline_generator_remap_start.elapsed();
            let inline_generator_annotate_start = Instant::now();
            annotate_typed_generator_instance_plans(
                function,
                Some(&refreshed_generator_instance_plans),
            )?;
            inline_generator_annotate_elapsed += inline_generator_annotate_start.elapsed();
            inline_generator_plan_elapsed += inline_generator_plan_start.elapsed();
            let empty_trusted_constructor_calls = HashMap::new();
            let trusted_constructor_calls_for_function = trusted_static_constructor_calls
                .get(&function.function_id)
                .unwrap_or(&empty_trusted_constructor_calls);
            let trusted_inline_work = prepare_trusted_typed_inline_work_for_function(
                function,
                &module.module_constants,
                trusted_constructor_calls_for_function,
                &trusted_constructor_init_owners,
                &mut inline_trusted_owner_states,
                &callee_module,
                external_callees,
                static_targets,
                &local_generators,
                remapped_generator_instance_plans.get(&function.function_id),
                &generator_state_pending_alias_use_instr_ids_by_origin,
            )?;
            inline_trusted_plan_elapsed += trusted_inline_work.timings.trusted_plan;
            inline_trusted_owner_state_elapsed += trusted_inline_work.timings.owner_state;
            inline_trusted_runtime_protocol_elapsed += trusted_inline_work.timings.runtime_protocol;
            inline_trusted_static_protocol_elapsed += trusted_inline_work.timings.static_protocol;
            inline_trusted_static_method_elapsed += trusted_inline_work.timings.static_method;
            inline_trusted_field_callable_elapsed += trusted_inline_work.timings.field_callable;
            inline_builtin_plan_elapsed += trusted_inline_work.timings.builtin_plan;
            let TrustedTypedInlineWork {
                timings: _,
                trusted_runtime_protocol_calls,
                trusted_runtime_protocol_receiver_origins,
                trusted_runtime_protocol_resume_functions,
                static_protocol_inline_targets,
                static_method_inline_targets,
                static_field_callable_inline_targets,
                generator_resume_plans,
                mut generator_resume_alias_cleanup_active_blocks,
                builtin_implementation_plans,
            } = trusted_inline_work;
            retain_typed_generator_pending_alias_evidence_by_origin(
                function,
                &module.module_constants,
                &mut generator_state_pending_alias_use_instr_ids_by_origin,
                &generator_resume_plans,
            );
            let inline_target_collect_start = Instant::now();
            let mut runtime_protocol_call_instr_ids = runtime_protocol_call_instr_ids(function);
            runtime_protocol_call_instr_ids.extend(trusted_runtime_protocol_calls.keys().copied());
            let mut inline_targets = typed_inline_targets_for_function(
                function.function_id,
                profile,
                static_direct_calls,
                &remapped_inline_targets,
                &suppressed_inline_targets,
            );
            let depends_on_suppressed_source_generator =
                typed_function_depends_on_suppressed_source_generator(function, static_targets);
            inline_targets.retain(|_, plans| {
                plans.retain(|(target, _)| {
                    !static_targets.suppressed_source_generators.contains(target)
                        && !(depends_on_suppressed_source_generator
                            && suppressed_source_generator_protocol_inline_targets.contains(target))
                });
                !plans.is_empty()
            });
            let profile_inline_target_count = inline_targets.len();
            let static_protocol_inline_target_count = static_protocol_inline_targets.len();
            let static_method_inline_target_count = static_method_inline_targets.len();
            let static_field_callable_inline_target_count =
                static_field_callable_inline_targets.len();
            inline_targets.extend(static_protocol_inline_targets);
            inline_targets.extend(static_method_inline_targets);
            inline_targets.extend(static_field_callable_inline_targets);
            let generator_resume_inline_targets = generator_resume_inline_targets(
                &generator_resume_plans,
                &callee_module,
                external_callees,
            );
            let generator_resume_inline_target_count = generator_resume_inline_targets.len();
            if !generator_resume_plans.is_empty() {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    caller = ?function.function_id,
                    pass,
                    generator_resume_plan_count = generator_resume_plans.len(),
                    generator_resume_inline_target_count =
                        generator_resume_inline_targets.len(),
                    "typed_generator_resume_inline_targets_collected",
                );
            }
            inline_targets.extend(generator_resume_inline_targets);
            let builtin_implementation_inline_targets =
                builtin_implementation_inline_targets(&builtin_implementation_plans);
            let builtin_implementation_inline_target_count =
                builtin_implementation_inline_targets.len();
            inline_targets.extend(builtin_implementation_inline_targets);
            inline_target_collect_elapsed += inline_target_collect_start.elapsed();
            let inline_target_stage_start = Instant::now();
            let inline_target_count_before_stage = inline_targets.len();
            let inline_targets = staged_inline_targets_for_trusted_runtime_protocols(
                inline_targets,
                &runtime_protocol_call_instr_ids,
                &trusted_runtime_protocol_calls,
                trusted_static_constructor_calls.get(&function.function_id),
                constructor_field_bindings.get(&function.function_id),
                &collect_typed_semantic_instr_ids(function),
            );
            let trusted_runtime_protocol_sources = trusted_runtime_protocol_calls
                .keys()
                .copied()
                .collect::<HashSet<_>>();
            let builtin_implementation_sources = builtin_implementation_plans
                .keys()
                .copied()
                .collect::<HashSet<_>>();
            let inline_targets = select_typed_inline_targets_within_cfg_budget_and_priorities(
                function,
                &callee_module,
                external_callees,
                &generator_resume_plans,
                &trusted_runtime_protocol_sources,
                &builtin_implementation_sources,
                inline_targets,
                Some(1),
            );
            retain_selected_typed_builtin_implementation_plans(
                function,
                &builtin_implementation_plans,
                &inline_targets,
            )?;
            if !generator_resume_plans.is_empty() {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    caller = ?function.function_id,
                    pass,
                    generator_resume_plan_count = generator_resume_plans.len(),
                    inline_target_count_before_stage,
                    inline_target_count_after_stage = inline_targets.len(),
                    "typed_generator_resume_inline_targets_after_staging",
                );
            }
            tracing::debug!(
                target: "soac_typed_inline_fixpoint",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                pass,
                block_count = function.blocks.len(),
                profile_inline_target_count,
                static_protocol_inline_target_count,
                static_method_inline_target_count,
                static_field_callable_inline_target_count,
                generator_resume_inline_target_count,
                builtin_implementation_inline_target_count,
                inline_target_count_before_stage,
                inline_target_count_after_stage = inline_targets.len(),
                "typed_inline_fixpoint_targets_staged",
            );
            inline_target_stage_elapsed += inline_target_stage_start.elapsed();
            inline_target_build_elapsed += inline_target_build_start.elapsed();
            if inline_targets.is_empty() {
                if !retried_idle_hot_continuation_split {
                    let idle_split_start = Instant::now();
                    let hot_continuation_split_stats = split_typed_post_inline_hot_continuations(
                        function,
                        &module.module_constants,
                        profile,
                        static_direct_calls,
                        &mut trusted_static_constructor_calls,
                        &mut hot_state_cleanup_labels,
                        &mut remapped_call_emissions,
                        &mut remapped_inline_targets,
                        &mut remapped_generator_instance_plans,
                        &mut suppressed_inline_targets,
                        &mut remapped_indexed_fields,
                        &mut remapped_indexed_field_counter_sources,
                        &mut remapped_exact_list_items,
                        &mut remapped_exact_int_branches,
                        &mut remapped_exact_int_returns,
                        &mut constructor_init_plans,
                        &mut constructor_field_bindings,
                        &mut constructor_capture_bindings_by_origin,
                        &mut suppressed_alias_hot_split_instr_ids,
                        &mut suppressed_generator_alias_hot_split_instr_ids,
                        &mut generator_state_instr_ids_by_origin,
                        &mut generator_state_pending_alias_use_instr_ids_by_origin,
                    )?;
                    let cleanup_hot_continuation_splits =
                        split_typed_post_inline_cleanup_hot_continuations(
                            function,
                            profile,
                            static_direct_calls,
                            &mut trusted_static_constructor_calls,
                            &mut hot_state_cleanup_labels,
                            &mut remapped_call_emissions,
                            &mut remapped_inline_targets,
                            &mut remapped_generator_instance_plans,
                            &mut suppressed_inline_targets,
                            &mut remapped_indexed_fields,
                            &mut remapped_indexed_field_counter_sources,
                            &mut remapped_exact_list_items,
                            &mut remapped_exact_int_branches,
                            &mut remapped_exact_int_returns,
                            &mut constructor_init_plans,
                            &mut constructor_field_bindings,
                            &mut constructor_capture_bindings_by_origin,
                            &mut generator_state_instr_ids_by_origin,
                            &mut generator_state_pending_alias_use_instr_ids_by_origin,
                        )?;
                    inline_idle_split_elapsed += idle_split_start.elapsed();
                    if hot_continuation_split_stats.remapped_instr_ids != 0
                        || cleanup_hot_continuation_splits != 0
                    {
                        generator_state_lowering_attempt_epoch += 1;
                        inline_trusted_owner_states.invalidate();
                        retried_idle_hot_continuation_split = true;
                        inline_progress.push(format!(
                            "pass {pass}: hot_instrs={}, ctor_clones={}, ctor_blocks={}, alias_clones={}, alias_blocks={}, alias_store_instr_ids={}, generator_alias_clones={}, generator_alias_blocks={}, generator_alias_store_instr_ids={}, alias_roots={:?}, generator_alias_roots={:?}, cleanup_splits={cleanup_hot_continuation_splits}",
                            hot_continuation_split_stats.remapped_instr_ids,
                            hot_continuation_split_stats.constructor_clones,
                            hot_continuation_split_stats.constructor_blocks,
                            hot_continuation_split_stats.alias_clones,
                            hot_continuation_split_stats.alias_blocks,
                            hot_continuation_split_stats.alias_store_instr_ids,
                            hot_continuation_split_stats.generator_alias_clones,
                            hot_continuation_split_stats.generator_alias_blocks,
                            hot_continuation_split_stats.generator_alias_store_instr_ids,
                            hot_continuation_split_stats.alias_clone_roots,
                            hot_continuation_split_stats.generator_alias_clone_roots,
                        ));
                        inline_maintenance_pass_count += 1;
                        if inline_maintenance_pass_count > MAX_TYPED_INLINE_MAINTENANCE_PASSES {
                            return Err(format!(
                                "typed-v3 direct-call inlining exhausted {MAX_TYPED_INLINE_MAINTENANCE_PASSES} maintenance passes in function {} without reaching a fixpoint: {inline_progress:?}",
                                function.function_id,
                            ));
                        }
                        continue;
                    }
                }
                retain_typed_generator_pending_alias_use_instr_ids_by_origin(
                    &mut generator_state_pending_alias_use_instr_ids_by_origin,
                    typed_generator_pending_alias_use_source_instr_ids_by_origin(
                        &generator_resume_plans,
                    ),
                );
                let pending_alias_use_instr_ids_by_origin =
                    typed_generator_alias_ignored_instr_ids_by_origin(
                        function,
                        &module.module_constants,
                        &generator_state_pending_alias_use_instr_ids_by_origin,
                    );
                retain_typed_generator_pending_alias_use_instr_ids_by_origin(
                    &mut generator_state_pending_alias_use_instr_ids_by_origin,
                    pending_alias_use_instr_ids_by_origin.clone(),
                );
                let generator_state_changed = lower_or_remap_typed_generator_state_for_function(
                    function,
                    &mut module.module_constants,
                    &callee_module,
                    typed_generator_state_lowering_plans(
                        generator_state_instr_ids_by_origin.clone(),
                        &generator_state_constructors_by_origin,
                        &pending_alias_use_instr_ids_by_origin,
                        Some(&generator_resume_alias_cleanup_active_blocks),
                    ),
                    generator_state_lowering_attempt_epoch,
                    Some(&mut noop_generator_state_lowering_attempts),
                    &mut lowered_generator_preserved_locals,
                );
                if generator_state_changed {
                    generator_state_lowering_attempt_epoch += 1;
                    inline_trusted_owner_states.invalidate();
                    retried_final_generator_plan_refresh = false;
                    retried_idle_hot_continuation_split = false;
                    inline_progress.push(format!("pass {pass}: generator_state_changed"));
                    inline_maintenance_pass_count += 1;
                    if inline_maintenance_pass_count > MAX_TYPED_INLINE_MAINTENANCE_PASSES {
                        return Err(format!(
                            "typed-v3 direct-call inlining exhausted {MAX_TYPED_INLINE_MAINTENANCE_PASSES} maintenance passes in function {} without reaching a fixpoint: {inline_progress:?}",
                            function.function_id,
                        ));
                    }
                    assign_missing_typed_function_instr_ids(function);
                    refresh_typed_function_value_facts(function);
                    let mut refreshed_generator_instance_plans =
                        static_generator_instance_plans_for_function(function, static_targets);
                    refreshed_generator_instance_plans.extend(
                        static_local_generator_instance_plans_for_function(
                            function,
                            &local_generators,
                        ),
                    );
                    if let Some(remapped) =
                        remapped_generator_instance_plans.get(&function.function_id)
                    {
                        refreshed_generator_instance_plans.extend(remapped.clone());
                    }
                    annotate_typed_generator_instance_plans(
                        function,
                        Some(&refreshed_generator_instance_plans),
                    )?;
                    let mut post_lowering_alias_split_instr_ids = HashSet::new();
                    let mut post_lowering_generator_alias_split_instr_ids = HashSet::new();
                    let post_lowering_hot_split_stats = split_typed_post_inline_hot_continuations(
                        function,
                        &module.module_constants,
                        profile,
                        static_direct_calls,
                        &mut trusted_static_constructor_calls,
                        &mut hot_state_cleanup_labels,
                        &mut remapped_call_emissions,
                        &mut remapped_inline_targets,
                        &mut remapped_generator_instance_plans,
                        &mut suppressed_inline_targets,
                        &mut remapped_indexed_fields,
                        &mut remapped_indexed_field_counter_sources,
                        &mut remapped_exact_list_items,
                        &mut remapped_exact_int_branches,
                        &mut remapped_exact_int_returns,
                        &mut constructor_init_plans,
                        &mut constructor_field_bindings,
                        &mut constructor_capture_bindings_by_origin,
                        &mut post_lowering_alias_split_instr_ids,
                        &mut post_lowering_generator_alias_split_instr_ids,
                        &mut generator_state_instr_ids_by_origin,
                        &mut generator_state_pending_alias_use_instr_ids_by_origin,
                    )?;
                    suppressed_alias_hot_split_instr_ids
                        .extend(post_lowering_alias_split_instr_ids);
                    suppressed_generator_alias_hot_split_instr_ids
                        .extend(post_lowering_generator_alias_split_instr_ids);
                    if post_lowering_hot_split_stats.remapped_instr_ids != 0 {
                        inline_progress.push(format!(
                            "pass {pass}: post_lowering_hot_instrs={}",
                            post_lowering_hot_split_stats.remapped_instr_ids
                        ));
                    }
                    continue;
                }
                let mut refreshed_generator_instance_plans =
                    static_generator_instance_plans_for_function(function, static_targets);
                refreshed_generator_instance_plans.extend(
                    static_local_generator_instance_plans_for_function(function, &local_generators),
                );
                if let Some(remapped) = remapped_generator_instance_plans.get(&function.function_id)
                {
                    refreshed_generator_instance_plans.extend(remapped.clone());
                }
                let attached_generator_instance_plans = annotate_typed_generator_instance_plans(
                    function,
                    Some(&refreshed_generator_instance_plans),
                )?;
                if attached_generator_instance_plans != 0 {
                    inline_progress.push(format!(
                        "pass {pass}: final_attached_generator_instance_plans={attached_generator_instance_plans}"
                    ));
                    if !retried_final_generator_plan_refresh {
                        retried_final_generator_plan_refresh = true;
                        inline_maintenance_pass_count += 1;
                        if inline_maintenance_pass_count > MAX_TYPED_INLINE_MAINTENANCE_PASSES {
                            return Err(format!(
                                "typed-v3 direct-call inlining exhausted {MAX_TYPED_INLINE_MAINTENANCE_PASSES} maintenance passes in function {} without reaching a fixpoint: {inline_progress:?}",
                                function.function_id,
                            ));
                        }
                        continue;
                    }
                }
                break;
            }
            inline_progress.push(format!(
                "pass {pass}: inline_targets={}",
                inline_targets.len()
            ));
            retried_idle_hot_continuation_split = false;
            inline_rewrite_pass_count += 1;
            if inline_rewrite_pass_count > MAX_TYPED_INLINE_PASSES {
                return Err(format!(
                    "typed-v3 direct-call inlining exceeded {MAX_TYPED_INLINE_PASSES} rewrite passes in function {}",
                    function.function_id
                ));
            }
            retried_final_generator_plan_refresh = false;
            let caller_function_id = function.function_id;
            let inline_rewrite_start = Instant::now();
            let pre_inline_block_labels = function
                .blocks
                .iter()
                .map(|block| block.label)
                .collect::<HashSet<_>>();
            let stats =
                inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls(
                    function,
                    &callee_module,
                    &mut module.module_constants,
                    external_callees,
                    &inline_targets,
                    &trusted_runtime_protocol_calls,
                    &trusted_runtime_protocol_receiver_origins,
                    &trusted_runtime_protocol_resume_functions,
                    &mut generator_state_constructors_by_origin,
                );
            generator_resume_alias_cleanup_active_blocks.extend(
                function
                    .blocks
                    .iter()
                    .filter(|block| !pre_inline_block_labels.contains(&block.label))
                    .map(|block| block.label),
            );
            inline_rewrite_elapsed += inline_rewrite_start.elapsed();
            if !generator_resume_plans.is_empty() {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    caller = ?function.function_id,
                    generator_resume_plan_count = generator_resume_plans.len(),
                    rewritten_stores = stats.rewritten_stores,
                    rewritten_effect_only_calls = stats.rewritten_effect_only_calls,
                    rewritten_returns = stats.rewritten_returns,
                    skipped_candidates = stats.skipped_candidates,
                    skipped_exception_edges = stats.skipped_exception_edges,
                    inline_source_count = stats.inline_instance_sources.len(),
                    instr_id_mapping_count = stats.instr_id_mappings.len(),
                    "typed_generator_resume_inline_stats",
                );
            }
            if !builtin_implementation_plans.is_empty() {
                let rewritten_builtin_implementation_calls = stats
                    .inline_instance_sources
                    .iter()
                    .filter(|source| {
                        builtin_implementation_plans.contains_key(&source.source_instr_id)
                    })
                    .count();
                tracing::debug!(
                    target: "soac_builtin_consumer_planning",
                    function_id = ?function.function_id,
                    function_qualname = %function.names.qualname,
                    builtin_plan_count = builtin_implementation_plans.len(),
                    rewritten_builtin_implementation_calls,
                    rewritten_stores = stats.rewritten_stores,
                    rewritten_effect_only_calls = stats.rewritten_effect_only_calls,
                    rewritten_returns = stats.rewritten_returns,
                    skipped_candidates = stats.skipped_candidates,
                    skipped_exception_edges = stats.skipped_exception_edges,
                    "typed_builtin_generator_consumer_inline_rewrites",
                );
            }
            trace_materialized_generator_state_constructor_anchors(
                function,
                pass,
                "after_inline",
                &generator_state_constructors_by_origin,
            );
            trace_typed_inline_arg_load_uses(function, "after_inline_before_state_lowering");
            let rewrote_inline = stats.rewritten_stores != 0
                || stats.rewritten_effect_only_calls != 0
                || stats.rewritten_returns != 0;
            tracing::debug!(
                target: "soac_typed_inline_fixpoint",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                pass,
                rewritten_stores = stats.rewritten_stores,
                rewritten_effect_only_calls = stats.rewritten_effect_only_calls,
                rewritten_returns = stats.rewritten_returns,
                skipped_candidates = stats.skipped_candidates,
                skipped_exception_edges = stats.skipped_exception_edges,
                inline_instance_source_count = stats.inline_instance_sources.len(),
                instr_id_mapping_count = stats.instr_id_mappings.len(),
                block_count_after_rewrite = function.blocks.len(),
                rewrote_inline,
                "typed_inline_fixpoint_rewrite_stats",
            );
            if !rewrote_inline {
                let idle_split_start = Instant::now();
                let _ = split_typed_post_inline_hot_continuations(
                    function,
                    &module.module_constants,
                    profile,
                    static_direct_calls,
                    &mut trusted_static_constructor_calls,
                    &mut hot_state_cleanup_labels,
                    &mut remapped_call_emissions,
                    &mut remapped_inline_targets,
                    &mut remapped_generator_instance_plans,
                    &mut suppressed_inline_targets,
                    &mut remapped_indexed_fields,
                    &mut remapped_indexed_field_counter_sources,
                    &mut remapped_exact_list_items,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                    &mut constructor_init_plans,
                    &mut constructor_field_bindings,
                    &mut constructor_capture_bindings_by_origin,
                    &mut suppressed_alias_hot_split_instr_ids,
                    &mut suppressed_generator_alias_hot_split_instr_ids,
                    &mut generator_state_instr_ids_by_origin,
                    &mut generator_state_pending_alias_use_instr_ids_by_origin,
                )?;
                split_typed_post_inline_cleanup_hot_continuations(
                    function,
                    profile,
                    static_direct_calls,
                    &mut trusted_static_constructor_calls,
                    &mut hot_state_cleanup_labels,
                    &mut remapped_call_emissions,
                    &mut remapped_inline_targets,
                    &mut remapped_generator_instance_plans,
                    &mut suppressed_inline_targets,
                    &mut remapped_indexed_fields,
                    &mut remapped_indexed_field_counter_sources,
                    &mut remapped_exact_list_items,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                    &mut constructor_init_plans,
                    &mut constructor_field_bindings,
                    &mut constructor_capture_bindings_by_origin,
                    &mut generator_state_instr_ids_by_origin,
                    &mut generator_state_pending_alias_use_instr_ids_by_origin,
                )?;
                inline_idle_split_elapsed += idle_split_start.elapsed();
                retain_typed_generator_pending_alias_use_instr_ids_by_origin(
                    &mut generator_state_pending_alias_use_instr_ids_by_origin,
                    typed_generator_pending_alias_use_source_instr_ids_by_origin(
                        &generator_resume_plans,
                    ),
                );
                let pending_alias_use_instr_ids_by_origin =
                    typed_generator_alias_ignored_instr_ids_by_origin(
                        function,
                        &module.module_constants,
                        &generator_state_pending_alias_use_instr_ids_by_origin,
                    );
                retain_typed_generator_pending_alias_use_instr_ids_by_origin(
                    &mut generator_state_pending_alias_use_instr_ids_by_origin,
                    pending_alias_use_instr_ids_by_origin.clone(),
                );
                let generator_state_changed = lower_or_remap_typed_generator_state_for_function(
                    function,
                    &mut module.module_constants,
                    &callee_module,
                    typed_generator_state_lowering_plans(
                        generator_state_instr_ids_by_origin.clone(),
                        &generator_state_constructors_by_origin,
                        &pending_alias_use_instr_ids_by_origin,
                        Some(&generator_resume_alias_cleanup_active_blocks),
                    ),
                    generator_state_lowering_attempt_epoch,
                    Some(&mut noop_generator_state_lowering_attempts),
                    &mut lowered_generator_preserved_locals,
                );
                if generator_state_changed {
                    generator_state_lowering_attempt_epoch += 1;
                    inline_trusted_owner_states.invalidate();
                    retried_idle_hot_continuation_split = false;
                    assign_missing_typed_function_instr_ids(function);
                    refresh_typed_function_value_facts(function);
                    let mut refreshed_generator_instance_plans =
                        static_generator_instance_plans_for_function(function, static_targets);
                    refreshed_generator_instance_plans.extend(
                        static_local_generator_instance_plans_for_function(
                            function,
                            &local_generators,
                        ),
                    );
                    if let Some(remapped) =
                        remapped_generator_instance_plans.get(&function.function_id)
                    {
                        refreshed_generator_instance_plans.extend(remapped.clone());
                    }
                    annotate_typed_generator_instance_plans(
                        function,
                        Some(&refreshed_generator_instance_plans),
                    )?;
                    inline_maintenance_pass_count += 1;
                    if inline_maintenance_pass_count > MAX_TYPED_INLINE_MAINTENANCE_PASSES {
                        return Err(format!(
                            "typed-v3 direct-call inlining exhausted {MAX_TYPED_INLINE_MAINTENANCE_PASSES} maintenance passes in function {} without reaching a fixpoint: {inline_progress:?}",
                            function.function_id,
                        ));
                    }
                    continue;
                }
                break;
            }
            generator_state_lowering_attempt_epoch += 1;
            inline_trusted_owner_states.invalidate();
            let inline_state_lowering_start = Instant::now();
            collect_generator_state_lowering_instr_ids(
                &generator_resume_plans,
                &stats,
                &mut generator_state_instr_ids_by_origin,
            );
            propagate_generator_state_lowering_synthetic_instr_ids(
                &stats,
                &mut generator_state_instr_ids_by_origin,
            );
            propagate_generator_pending_alias_use_synthetic_instr_ids(
                &stats,
                &mut generator_state_pending_alias_use_instr_ids_by_origin,
            );
            refresh_typed_generator_inline_sidecars_after_rewrite(
                function,
                &stats,
                &constructor_capture_bindings_by_function,
                &mut generator_state_constructors_by_origin,
                &mut constructor_capture_bindings_by_origin,
                trusted_static_constructor_calls
                    .entry(caller_function_id)
                    .or_default(),
            );
            trace_materialized_generator_state_constructor_anchors(
                function,
                pass,
                "before_state_lowering",
                &generator_state_constructors_by_origin,
            );
            retain_typed_generator_pending_alias_use_instr_ids_by_origin(
                &mut generator_state_pending_alias_use_instr_ids_by_origin,
                typed_generator_pending_alias_use_source_instr_ids_by_origin(
                    &generator_resume_plans,
                ),
            );
            let pending_alias_use_instr_ids_by_origin =
                typed_generator_alias_ignored_instr_ids_by_origin(
                    function,
                    &module.module_constants,
                    &generator_state_pending_alias_use_instr_ids_by_origin,
                );
            retain_typed_generator_pending_alias_use_instr_ids_by_origin(
                &mut generator_state_pending_alias_use_instr_ids_by_origin,
                pending_alias_use_instr_ids_by_origin.clone(),
            );
            let generator_state_changed = lower_or_remap_typed_generator_state_for_function(
                function,
                &mut module.module_constants,
                &callee_module,
                typed_generator_state_lowering_plans(
                    generator_state_instr_ids_by_origin.clone(),
                    &mut generator_state_constructors_by_origin,
                    &pending_alias_use_instr_ids_by_origin,
                    Some(&generator_resume_alias_cleanup_active_blocks),
                ),
                generator_state_lowering_attempt_epoch,
                Some(&mut noop_generator_state_lowering_attempts),
                &mut lowered_generator_preserved_locals,
            );
            let remapped_constructor_capture_bindings =
                remap_inlined_generator_constructor_capture_bindings_for_lowered_state(
                    function,
                    &generator_resume_plans,
                    &stats,
                    &lowered_generator_preserved_locals,
                    &mut constructor_capture_bindings_by_origin,
                );
            if remapped_constructor_capture_bindings != 0 {
                tracing::info!(
                    target: "soac_generator_state_lowering",
                    function_id = ?function.function_id,
                    remapped_constructor_capture_bindings,
                    "typed_generator_constructor_capture_bindings_remapped_after_state_lowering",
                );
            }
            trace_typed_preserved_name_count(function, pass, "after_state_lowering");
            if generator_state_changed {
                assign_missing_typed_function_instr_ids(function);
                refresh_typed_function_value_facts(function);
                let mut refreshed_generator_instance_plans =
                    static_generator_instance_plans_for_function(function, static_targets);
                refreshed_generator_instance_plans.extend(
                    static_local_generator_instance_plans_for_function(function, &local_generators),
                );
                if let Some(remapped) = remapped_generator_instance_plans.get(&function.function_id)
                {
                    refreshed_generator_instance_plans.extend(remapped.clone());
                }
                annotate_typed_generator_instance_plans(
                    function,
                    Some(&refreshed_generator_instance_plans),
                )?;
            }
            inline_state_lowering_elapsed += inline_state_lowering_start.elapsed();
            trace_typed_inline_arg_load_uses(function, "after_inline_state_lowering");
            hot_state_cleanup_labels.extend(stats.hot_state_cleanup_labels.iter().copied());
            let inline_constructor_init_start = Instant::now();
            let init_plans = typed_constructor_init_plans_from_inline_stats_with_external_callees(
                &callee_module,
                &module.module_constants,
                external_callees,
                &stats,
            );
            trusted_static_constructor_calls
                .entry(caller_function_id)
                .or_default()
                .extend(remapped_static_constructor_calls_from_inline_stats(
                    &stats,
                    &static_constructor_calls,
                ));
            if !init_plans.is_empty() {
                constructor_init_plans
                    .entry(caller_function_id)
                    .or_default()
                    .extend(init_plans);
            }
            if let Some(inline_plan) = inline_plan {
                let bindings =
                    typed_constructor_field_bindings_from_inline_stats_with_external_callees(
                        &callee_module,
                        inline_plan,
                        &module.module_constants,
                        external_callees,
                        &stats,
                    );
                if !bindings.is_empty() {
                    let trusted_materialized_calls = trusted_static_constructor_calls
                        .get(&caller_function_id)
                        .map(|trusted_calls| {
                            trusted_materialized_constructor_calls_from_inline_stats(
                                &stats,
                                trusted_calls,
                                &bindings,
                            )
                        })
                        .unwrap_or_default();
                    if let Some(trusted_calls) =
                        trusted_static_constructor_calls.get_mut(&caller_function_id)
                    {
                        trusted_calls.extend(trusted_materialized_calls);
                    }
                    constructor_field_bindings
                        .entry(caller_function_id)
                        .or_default()
                        .extend(bindings);
                }
            }
            let function_constructor_init_plans = constructor_init_plans
                .get(&caller_function_id)
                .cloned()
                .unwrap_or_default();
            if !function_constructor_init_plans.is_empty() {
                annotate_typed_constructor_init_plans(
                    function,
                    Some(&function_constructor_init_plans),
                )?;
            }
            let mut bound_constructor_sources = constructor_field_bindings
                .get(&caller_function_id)
                .map(|bindings| bindings.keys().copied().collect::<HashSet<_>>())
                .unwrap_or_default();
            let mut constructor_sources = function_constructor_init_plans
                .iter()
                .map(|(source, plan)| (*source, *plan))
                .collect::<Vec<_>>();
            constructor_sources.sort_by_key(|(source, _)| source.index());
            let mut projected_constructor_blocks = function.blocks.len();
            let mut projected_constructor_body_instrs =
                typed_inline_function_body_instr_count(function);
            for (source, plan) in constructor_sources {
                if bound_constructor_sources.contains(&source) {
                    continue;
                }
                let Some(callee) = callee_module
                    .callable_defs
                    .iter()
                    .find(|callee| callee.function_id == plan.init_function_id)
                    .or_else(|| {
                        external_callees
                            .get(&plan.init_function_id)
                            .map(|callee| &callee.function)
                    })
                else {
                    bound_constructor_sources.insert(source);
                    continue;
                };
                let next_blocks = projected_constructor_blocks
                    .saturating_add(callee.blocks.len())
                    .saturating_add(2);
                let next_body_instrs = projected_constructor_body_instrs
                    .saturating_add(typed_inline_function_body_instr_count(callee))
                    .saturating_add(8);
                if next_blocks >= MAX_TYPED_INLINE_FUNCTION_BLOCKS
                    || next_body_instrs >= MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS
                {
                    bound_constructor_sources.insert(source);
                    continue;
                }
                projected_constructor_blocks = next_blocks;
                projected_constructor_body_instrs = next_body_instrs;
            }
            let init_body_stats = inline_typed_constructor_init_bodies_with_external_callees(
                function,
                &callee_module,
                &mut module.module_constants,
                external_callees,
                &bound_constructor_sources,
            );
            if !init_body_stats.inlined_constructor_init_calls.is_empty() {
                if let Some(plans) = constructor_init_plans.get_mut(&caller_function_id) {
                    for instr_id in &init_body_stats.inlined_constructor_init_calls {
                        if let Some(plan) = plans.get_mut(instr_id) {
                            plan.source =
                                TypedConstructorInitPlanSource::InlinedConstructorEntryWithInlinedInitBody;
                        }
                    }
                }
            }
            if !init_body_stats.constructor_field_bindings.is_empty() {
                let trusted_materialized_calls = trusted_static_constructor_calls
                    .get(&caller_function_id)
                    .map(|trusted_calls| {
                        trusted_materialized_constructor_calls_from_inline_stats(
                            &stats,
                            trusted_calls,
                            &init_body_stats.constructor_field_bindings,
                        )
                    })
                    .unwrap_or_default();
                if let Some(trusted_calls) =
                    trusted_static_constructor_calls.get_mut(&caller_function_id)
                {
                    trusted_calls.extend(trusted_materialized_calls);
                }
                constructor_field_bindings
                    .entry(caller_function_id)
                    .or_default()
                    .extend(init_body_stats.constructor_field_bindings);
            }
            inline_constructor_init_elapsed += inline_constructor_init_start.elapsed();
            let inline_sidecar_remap_start = Instant::now();
            if !stats.instr_id_mappings.is_empty() {
                remap_inlined_generator_instance_plans(
                    caller_function_id,
                    &stats.instr_id_mappings,
                    &callee_module,
                    external_callees,
                    &mut remapped_generator_instance_plans,
                )?;
                remap_inlined_call_emission_plans(
                    caller_function_id,
                    &stats.instr_id_mappings,
                    profile,
                    static_direct_calls,
                    &mut remapped_call_emissions,
                )?;
                remap_inlined_direct_call_targets(
                    caller_function_id,
                    &stats.instr_id_mappings,
                    &callee_module,
                    external_callees,
                    profile,
                    &trusted_generator_bridge_targets,
                    static_direct_calls,
                    &mut remapped_inline_targets,
                    &suppressed_inline_targets,
                );
                remap_inlined_indexed_field_accesses(
                    caller_function_id,
                    &stats.instr_id_mappings,
                    profile,
                    &mut remapped_indexed_fields,
                    &mut remapped_indexed_field_counter_sources,
                )?;
                remap_inlined_exact_list_item_accesses(
                    caller_function_id,
                    &stats.instr_id_mappings,
                    profile,
                    &mut remapped_exact_list_items,
                )?;
                remap_inlined_exact_int_selections(
                    caller_function_id,
                    &stats.instr_id_mappings,
                    &stats.constant_mappings,
                    &stats.local_mappings,
                    profile,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                )?;
            }
            if !init_body_stats.inline_stats.instr_id_mappings.is_empty() {
                remap_inlined_generator_instance_plans(
                    caller_function_id,
                    &init_body_stats.inline_stats.instr_id_mappings,
                    &callee_module,
                    external_callees,
                    &mut remapped_generator_instance_plans,
                )?;
                remap_inlined_call_emission_plans(
                    caller_function_id,
                    &init_body_stats.inline_stats.instr_id_mappings,
                    profile,
                    static_direct_calls,
                    &mut remapped_call_emissions,
                )?;
                remap_inlined_direct_call_targets(
                    caller_function_id,
                    &init_body_stats.inline_stats.instr_id_mappings,
                    &callee_module,
                    external_callees,
                    profile,
                    &trusted_generator_bridge_targets,
                    static_direct_calls,
                    &mut remapped_inline_targets,
                    &suppressed_inline_targets,
                );
                remap_inlined_indexed_field_accesses(
                    caller_function_id,
                    &init_body_stats.inline_stats.instr_id_mappings,
                    profile,
                    &mut remapped_indexed_fields,
                    &mut remapped_indexed_field_counter_sources,
                )?;
                remap_inlined_exact_list_item_accesses(
                    caller_function_id,
                    &init_body_stats.inline_stats.instr_id_mappings,
                    profile,
                    &mut remapped_exact_list_items,
                )?;
                remap_inlined_exact_int_selections(
                    caller_function_id,
                    &init_body_stats.inline_stats.instr_id_mappings,
                    &init_body_stats.inline_stats.constant_mappings,
                    &init_body_stats.inline_stats.local_mappings,
                    profile,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                )?;
            }
            inline_sidecar_remap_elapsed += inline_sidecar_remap_start.elapsed();
            let inline_refresh_start = Instant::now();
            let inline_tuple_simplify_start = Instant::now();
            let simplified_inline_virtual_tuples =
                simplify_typed_virtual_tuple_ops(function, &mut module.module_constants);
            inline_tuple_simplify_elapsed += inline_tuple_simplify_start.elapsed();
            if simplified_inline_virtual_tuples != 0 {
                let inline_value_fact_refresh_start = Instant::now();
                retain_live_typed_profile_sidecars(
                    function,
                    &mut remapped_call_emissions,
                    &mut remapped_inline_targets,
                    &mut remapped_generator_instance_plans,
                    &mut remapped_indexed_fields,
                    &mut remapped_indexed_field_counter_sources,
                    &mut remapped_exact_list_items,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                    &mut constructor_init_plans,
                );
                assign_missing_typed_function_instr_ids(function);
                refresh_typed_function_value_facts(function);
                inline_value_fact_refresh_elapsed += inline_value_fact_refresh_start.elapsed();
            }
            trace_materialized_generator_state_constructor_anchors(
                function,
                pass,
                "after_virtual_tuple_simplify",
                &generator_state_constructors_by_origin,
            );
            let inline_value_fact_refresh_start = Instant::now();
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
            inline_value_fact_refresh_elapsed += inline_value_fact_refresh_start.elapsed();
            if let Some(remapped_call_emissions) =
                remapped_call_emissions.get(&function.function_id)
            {
                let inline_call_emission_lower_start = Instant::now();
                lower_typed_function_call_emission_plans(function, remapped_call_emissions)?;
                refresh_typed_function_value_facts(function);
                inline_call_emission_lower_elapsed += inline_call_emission_lower_start.elapsed();
            }
            inline_refresh_elapsed += inline_refresh_start.elapsed();
            trace_materialized_generator_state_constructor_anchors(
                function,
                pass,
                "after_call_emission_lowering",
                &generator_state_constructors_by_origin,
            );
            let inline_post_split_start = Instant::now();
            let _ = split_typed_post_inline_hot_continuations(
                function,
                &module.module_constants,
                profile,
                static_direct_calls,
                &mut trusted_static_constructor_calls,
                &mut hot_state_cleanup_labels,
                &mut remapped_call_emissions,
                &mut remapped_inline_targets,
                &mut remapped_generator_instance_plans,
                &mut suppressed_inline_targets,
                &mut remapped_indexed_fields,
                &mut remapped_indexed_field_counter_sources,
                &mut remapped_exact_list_items,
                &mut remapped_exact_int_branches,
                &mut remapped_exact_int_returns,
                &mut constructor_init_plans,
                &mut constructor_field_bindings,
                &mut constructor_capture_bindings_by_origin,
                &mut suppressed_alias_hot_split_instr_ids,
                &mut suppressed_generator_alias_hot_split_instr_ids,
                &mut generator_state_instr_ids_by_origin,
                &mut generator_state_pending_alias_use_instr_ids_by_origin,
            )?;
            trace_materialized_generator_state_constructor_anchors(
                function,
                pass,
                "after_hot_continuation_split",
                &generator_state_constructors_by_origin,
            );
            split_typed_post_inline_cleanup_hot_continuations(
                function,
                profile,
                static_direct_calls,
                &mut trusted_static_constructor_calls,
                &mut hot_state_cleanup_labels,
                &mut remapped_call_emissions,
                &mut remapped_inline_targets,
                &mut remapped_generator_instance_plans,
                &mut suppressed_inline_targets,
                &mut remapped_indexed_fields,
                &mut remapped_indexed_field_counter_sources,
                &mut remapped_exact_list_items,
                &mut remapped_exact_int_branches,
                &mut remapped_exact_int_returns,
                &mut constructor_init_plans,
                &mut constructor_field_bindings,
                &mut constructor_capture_bindings_by_origin,
                &mut generator_state_instr_ids_by_origin,
                &mut generator_state_pending_alias_use_instr_ids_by_origin,
            )?;
            inline_post_split_elapsed += inline_post_split_start.elapsed();
            trace_materialized_generator_state_constructor_anchors(
                function,
                pass,
                "after_cleanup_hot_continuation_split",
                &generator_state_constructors_by_origin,
            );
        }
        let inline_loop_elapsed = inline_loop_start.elapsed();
        let post_inline_cfg_start = Instant::now();
        let final_state_lowering_start = Instant::now();
        let pending_alias_use_instr_ids_by_origin =
            typed_generator_alias_ignored_instr_ids_by_origin(
                function,
                &module.module_constants,
                &generator_state_pending_alias_use_instr_ids_by_origin,
            );
        retain_typed_generator_pending_alias_use_instr_ids_by_origin(
            &mut generator_state_pending_alias_use_instr_ids_by_origin,
            pending_alias_use_instr_ids_by_origin.clone(),
        );
        let generator_state_changed = lower_or_remap_typed_generator_state_for_function(
            function,
            &mut module.module_constants,
            &callee_module,
            typed_generator_state_lowering_plans(
                generator_state_instr_ids_by_origin.clone(),
                &generator_state_constructors_by_origin,
                &pending_alias_use_instr_ids_by_origin,
                None,
            ),
            generator_state_lowering_attempt_epoch,
            Some(&mut noop_generator_state_lowering_attempts),
            &mut lowered_generator_preserved_locals,
        );
        trace_typed_preserved_name_count(
            function,
            MAX_TYPED_INLINE_PASSES,
            "after_final_state_lowering",
        );
        if generator_state_changed {
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
            let mut refreshed_generator_instance_plans =
                static_generator_instance_plans_for_function(function, static_targets);
            refreshed_generator_instance_plans.extend(
                static_local_generator_instance_plans_for_function(function, &local_generators),
            );
            if let Some(remapped) = remapped_generator_instance_plans.get(&function.function_id) {
                refreshed_generator_instance_plans.extend(remapped.clone());
            }
            annotate_typed_generator_instance_plans(
                function,
                Some(&refreshed_generator_instance_plans),
            )?;
        }
        let final_state_lowering_elapsed = final_state_lowering_start.elapsed();
        trace_typed_inline_arg_load_uses(function, "after_final_state_lowering");
        let final_hot_split_elapsed = Duration::ZERO;
        let final_cleanup_split_elapsed = Duration::ZERO;
        let final_call_emission_start = Instant::now();
        if let Some(remapped_call_emissions) = remapped_call_emissions.get(&function.function_id) {
            lower_typed_function_call_emission_plans(function, remapped_call_emissions)?;
            refresh_typed_function_value_facts(function);
        }
        let final_call_emission_elapsed = final_call_emission_start.elapsed();
        trace_typed_inline_arg_load_uses(function, "after_final_call_emission_lowering");
        // Late post-inline order is deliberate:
        // 1. normalize StopIteration control flow exposed by earlier inlining;
        // 2. issue refresh tickets for families whose eligibility depends on that normalization;
        // 3. revisit builtin/runtime-protocol late passes and consume scheduled resume refreshes
        //    to a fixpoint;
        // 4. normalize any additional StopIteration edges produced by that iteration and
        //    reissue the resume refresh ticket when needed.
        let final_stop_iteration_start = Instant::now();
        let rewritten_stop_iteration = rewrite_typed_stop_iteration_raises_to_handler_jumps(
            function,
            &module.module_constants,
        );
        if rewritten_stop_iteration != 0 {
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
        }
        let mut late_refresh_schedule =
            LateTypedRefreshSchedule::from_rewritten_stop_iteration(rewritten_stop_iteration);
        if typed_function_has_runtime_generator_resume_call(function, &module.module_constants) {
            late_refresh_schedule.request(LateTypedRefreshFamily::TrustedGeneratorResume);
        }
        if function
            .storage_layout
            .as_ref()
            .is_none_or(|layout| layout.preserved_slots.is_empty())
        {
            let preserved = typed_preserved_name_counts(function);
            if preserved.preserved_name_count != 0
                || preserved.preserved_cell_name_count != 0
                || preserved.preserved_cell_ref_count != 0
            {
                late_refresh_schedule
                    .pending_families
                    .insert(LateTypedRefreshFamily::TrustedGeneratorResume);
            }
        }
        let trusted_constructor_calls_for_function = trusted_static_constructor_calls
            .entry(function.function_id)
            .or_default();
        let mut late_resume_rewrites = 0;
        let mut late_resume_passes = 0;
        let mut late_runtime_protocol_rewrites = 0;
        let mut late_runtime_protocol_passes = 0;
        let mut late_builtin_implementation_rewrites = 0;
        let mut late_builtin_implementation_passes = 0;
        let mut late_stop_iteration_rewrites = 0;
        let mut late_trusted_owner_states = TrustedOwnerStateCache::default();
        loop {
            if !typed_inline_function_within_cfg_budget(function) {
                tracing::info!(
                    target: "soac_inline_budget",
                    function_id = ?function.function_id,
                    function_qualname = %function.names.qualname,
                    block_count = function.blocks.len(),
                    max_blocks = MAX_TYPED_INLINE_FUNCTION_BLOCKS,
                    max_body_instrs = MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS,
                    "typed_late_inline_fixpoint_reached_function_cfg_budget",
                );
                if !late_refresh_schedule.requests(LateTypedRefreshFamily::TrustedGeneratorResume) {
                    break;
                }
            }
            if late_resume_passes == MAX_LATE_TYPED_GENERATOR_RESUME_PASSES {
                return Err(format!(
                    "typed-v3 late generator-resume inlining exceeded {MAX_LATE_TYPED_GENERATOR_RESUME_PASSES} passes in function {}",
                    function.names.qualname
                ));
            }
            let runtime_protocol_stats = if typed_inline_function_within_cfg_budget(function) {
                if late_runtime_protocol_passes == MAX_LATE_TYPED_RUNTIME_PROTOCOL_PASSES {
                    return Err(format!(
                        "typed-v3 late runtime-protocol inlining exceeded {MAX_LATE_TYPED_RUNTIME_PROTOCOL_PASSES} passes in function {}",
                        function.names.qualname
                    ));
                }
                inline_late_typed_runtime_protocol_and_static_method_plans(
                    function,
                    &mut module.module_constants,
                    trusted_constructor_calls_for_function,
                    &trusted_constructor_init_owners,
                    &callee_module,
                    external_callees,
                    static_targets,
                    &mut late_trusted_owner_states,
                )?
            } else {
                soac_opt::passes::TypedInlineRewriteStats::default()
            };
            let rewritten_runtime_protocol = runtime_protocol_stats.rewritten_stores
                + runtime_protocol_stats.rewritten_effect_only_calls
                + runtime_protocol_stats.rewritten_returns;
            if rewritten_runtime_protocol != 0 {
                late_trusted_owner_states.invalidate();
                refresh_typed_generator_inline_sidecars_after_rewrite(
                    function,
                    &runtime_protocol_stats,
                    &constructor_capture_bindings_by_function,
                    &mut generator_state_constructors_by_origin,
                    &mut constructor_capture_bindings_by_origin,
                    trusted_constructor_calls_for_function,
                );
                late_refresh_schedule.request(LateTypedRefreshFamily::TrustedGeneratorResume);
            }
            late_runtime_protocol_rewrites += rewritten_runtime_protocol;
            late_runtime_protocol_passes += usize::from(rewritten_runtime_protocol != 0);

            let rewritten_builtin_implementation = if rewritten_runtime_protocol == 0
                && typed_inline_function_within_cfg_budget(function)
            {
                if late_builtin_implementation_passes
                    == MAX_LATE_TYPED_BUILTIN_IMPLEMENTATION_PASSES
                {
                    return Err(format!(
                        "typed-v3 late builtin-implementation inlining exceeded {MAX_LATE_TYPED_BUILTIN_IMPLEMENTATION_PASSES} passes in function {}",
                        function.names.qualname
                    ));
                }
                inline_late_typed_builtin_implementation_plans(
                    function,
                    &mut module.module_constants,
                    trusted_constructor_calls_for_function,
                    &trusted_constructor_init_owners,
                    &callee_module,
                    external_callees,
                    static_targets,
                    &mut generator_state_constructors_by_origin,
                    &constructor_capture_bindings_by_function,
                    &mut constructor_capture_bindings_by_origin,
                    &mut late_trusted_owner_states,
                )?
            } else {
                0
            };
            late_builtin_implementation_rewrites += rewritten_builtin_implementation;
            late_builtin_implementation_passes +=
                usize::from(rewritten_builtin_implementation != 0);
            if rewritten_builtin_implementation != 0 {
                late_trusted_owner_states.invalidate();
                refresh_typed_generator_inline_sidecars_for_function(
                    function,
                    &mut generator_state_constructors_by_origin,
                    &mut constructor_capture_bindings_by_origin,
                );
                late_refresh_schedule.request(LateTypedRefreshFamily::TrustedGeneratorResume);
            }

            let rewritten_resume =
                inline_late_typed_generator_resume_plans_after_stop_iteration_normalization(
                    function,
                    &mut module.module_constants,
                    &mut late_refresh_schedule,
                    trusted_constructor_calls_for_function,
                    &trusted_constructor_init_owners,
                    &callee_module,
                    external_callees,
                    &generator_state_constructors_by_origin,
                    &constructor_capture_bindings_by_function,
                    &mut constructor_capture_bindings_by_origin,
                    &mut generator_state_instr_ids_by_origin,
                    &mut generator_state_pending_alias_use_instr_ids_by_origin,
                    &mut lowered_generator_preserved_locals,
                    static_targets,
                    &local_generators,
                    remapped_generator_instance_plans.get(&function.function_id),
                    &mut late_trusted_owner_states,
                )?;
            late_resume_rewrites += rewritten_resume;
            late_resume_passes += usize::from(rewritten_resume != 0);
            if rewritten_resume != 0 {
                late_trusted_owner_states.invalidate();
            }

            let mut late_iteration = LateTypedFixpointIteration {
                builtin_implementation_rewrites: rewritten_builtin_implementation,
                runtime_protocol_rewrites: rewritten_runtime_protocol,
                resume_rewrites: rewritten_resume,
                stop_iteration_rewrites: 0,
            };
            late_iteration.stop_iteration_rewrites =
                if late_iteration.may_expose_stop_iteration_edges() {
                    rewrite_typed_stop_iteration_raises_to_handler_jumps(
                        function,
                        &module.module_constants,
                    )
                } else {
                    0
                };
            if late_iteration.stop_iteration_rewrites != 0 {
                late_trusted_owner_states.invalidate();
                late_stop_iteration_rewrites += late_iteration.stop_iteration_rewrites;
                late_refresh_schedule
                    .record_rewritten_stop_iteration(late_iteration.stop_iteration_rewrites);
                assign_missing_typed_function_instr_ids(function);
                refresh_typed_function_value_facts(function);
            }
            if !late_iteration.made_progress() {
                break;
            }
        }
        if late_resume_rewrites != 0 {
            tracing::info!(
                target: "soac_generator_state_lowering",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                rewritten_stop_iteration = late_refresh_schedule.rewritten_stop_iteration,
                pending_refresh_trusted_generator_resume = late_refresh_schedule
                    .requests(LateTypedRefreshFamily::TrustedGeneratorResume),
                late_resume_rewrites,
                late_resume_passes,
                "typed_generator_resume_inlined_during_late_post_inline_fixpoint",
            );
        }
        if late_trusted_owner_states.builds != 0 || late_trusted_owner_states.reuses != 0 {
            tracing::debug!(
                target: "soac_late_typed_refresh",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                trusted_owner_state_cache_builds = late_trusted_owner_states.builds,
                trusted_owner_state_cache_reuses = late_trusted_owner_states.reuses,
                trusted_owner_state_cache_invalidations = late_trusted_owner_states.invalidations,
                "late_typed_trusted_owner_state_cache_summary",
            );
        }
        if late_runtime_protocol_rewrites != 0 {
            tracing::info!(
                target: "soac_generator_protocol_planning",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                rewritten_stop_iteration,
                late_runtime_protocol_rewrites,
                late_runtime_protocol_passes,
                "typed_runtime_protocol_inlined_during_late_post_inline_fixpoint",
            );
        }
        if late_builtin_implementation_rewrites != 0 {
            tracing::info!(
                target: "soac_builtin_consumer_planning",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                rewritten_stop_iteration,
                late_builtin_implementation_rewrites,
                late_builtin_implementation_passes,
                "typed_builtin_generator_consumer_inlined_during_late_post_inline_fixpoint",
            );
        }
        if late_stop_iteration_rewrites != 0 {
            tracing::info!(
                target: "soac_generator_protocol_planning",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                rewritten_stop_iteration,
                late_stop_iteration_rewrites,
                "typed_stop_iteration_normalized_during_late_generator_fixpoint",
            );
        }
        let final_stop_iteration_elapsed = final_stop_iteration_start.elapsed();
        let final_tuple_simplify_start = Instant::now();
        let simplified_final_virtual_tuples =
            simplify_typed_virtual_tuple_ops(function, &mut module.module_constants);
        let final_tuple_rewrite_elapsed = final_tuple_simplify_start.elapsed();
        let mut final_tuple_refresh_elapsed = Duration::ZERO;
        retain_live_typed_profile_sidecars(
            function,
            &mut remapped_call_emissions,
            &mut remapped_inline_targets,
            &mut remapped_generator_instance_plans,
            &mut remapped_indexed_fields,
            &mut remapped_indexed_field_counter_sources,
            &mut remapped_exact_list_items,
            &mut remapped_exact_int_branches,
            &mut remapped_exact_int_returns,
            &mut constructor_init_plans,
        );
        if simplified_final_virtual_tuples != 0 {
            let final_tuple_refresh_start = Instant::now();
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
            final_tuple_refresh_elapsed = final_tuple_refresh_start.elapsed();
        }
        let final_tuple_simplify_elapsed =
            final_tuple_rewrite_elapsed + final_tuple_refresh_elapsed;
        let post_inline_cfg_elapsed = post_inline_cfg_start.elapsed();
        let profile_virtualize_start = Instant::now();
        apply_profile_access_and_scalar_plans_to_typed_function(
            function,
            profile,
            remapped_indexed_fields.get(&function.function_id),
            remapped_indexed_field_counter_sources.get(&function.function_id),
            remapped_exact_list_items.get(&function.function_id),
            remapped_exact_int_branches.get(&function.function_id),
            remapped_exact_int_returns.get(&function.function_id),
            constructor_init_plans.get(&function.function_id),
        )?;
        // Verify must exercise the selected indexed accesses whose counters it emits.
        // Profile has no replayed field plans, and apply emits no field counters.
        let preserve_profiled_indexed_field_counters = field_counter_function_ids
            .contains(&function.function_id)
            && !indexed_field_accesses_for_function(
                function.function_id,
                profile,
                &remapped_indexed_fields,
            )?
            .is_empty();
        if let Some(bindings) = constructor_field_bindings
            .get(&function.function_id)
            .filter(|_| !preserve_profiled_indexed_field_counters)
        {
            let trusted_sources = trusted_static_constructor_calls
                .get(&function.function_id)
                .map(|calls| calls.keys().copied().collect::<HashSet<_>>())
                .unwrap_or_default();
            if !trusted_sources.is_empty() {
                let mut fully_virtual_plan = plan_typed_fully_virtual_objects(
                    function,
                    &module.module_constants,
                    bindings,
                    &trusted_sources,
                );
                let (field_callable_emissions, _) = trusted_field_callable_inlines_for_function(
                    function,
                    &module.module_constants,
                    trusted_static_constructor_calls
                        .get(&function.function_id)
                        .unwrap_or(&HashMap::new()),
                    &trusted_constructor_init_owners,
                    &callee_module,
                    external_callees,
                )?;
                if !field_callable_emissions.is_empty() {
                    lower_typed_function_call_emission_plans(function, &field_callable_emissions)?;
                    refresh_typed_function_value_facts(function);
                    fully_virtual_plan = plan_typed_fully_virtual_objects(
                        function,
                        &module.module_constants,
                        bindings,
                        &trusted_sources,
                    );
                }
                fully_virtual_plan.set_virtual_field_edge_block_budget(
                    typed_inline_remaining_cfg_blocks(function),
                );
                let stats = lower_typed_fully_virtual_objects_to_locals_with_plan(
                    function,
                    &module.module_constants,
                    &mut fully_virtual_plan,
                );
                if stats.changed() {
                    remap_virtualized_exact_int_inputs_to_scalar_locals(
                        function,
                        &fully_virtual_plan,
                    );
                    assign_missing_typed_function_instr_ids(function);
                    refresh_typed_function_value_facts(function);
                }
            }
            let remaining_bindings = bindings
                .iter()
                .filter(|(source, _)| !trusted_sources.contains(source))
                .map(|(source, bindings)| (*source, bindings.clone()))
                .collect::<HashMap<_, _>>();
            if !remaining_bindings.is_empty() {
                let mut virtualization_plan = plan_typed_virtual_objects(
                    function,
                    &module.module_constants,
                    &remaining_bindings,
                );
                virtualization_plan.set_virtual_field_edge_block_budget(
                    typed_inline_remaining_cfg_blocks(function),
                );
                let stats = lower_typed_virtual_objects_to_locals_with_plan(
                    function,
                    &module.module_constants,
                    &mut virtualization_plan,
                );
                if stats.changed() {
                    remap_virtualized_exact_int_inputs_to_scalar_locals(
                        function,
                        &virtualization_plan,
                    );
                    assign_missing_typed_function_instr_ids(function);
                    refresh_typed_function_value_facts(function);
                }
            }
        }
        apply_profile_typed_block_metadata_to_typed_function(function, profile)?;
        apply_profile_typed_guard_miss_policy_to_typed_function(function, profile);
        let profile_virtualize_elapsed = profile_virtualize_start.elapsed();
        let tail_cleanup_start = Instant::now();
        let residual_builtin_consumers = residual_builtin_generator_consumer_counts_for_function(
            function,
            &module.module_constants,
        );
        if residual_builtin_consumers.total() != 0 {
            tracing::debug!(
                target: "soac_builtin_consumer_planning",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                list_total = residual_builtin_consumers.list_total,
                list_with_generator = residual_builtin_consumers.list_with_generator,
                set_total = residual_builtin_consumers.set_total,
                set_with_generator = residual_builtin_consumers.set_with_generator,
                tuple_total = residual_builtin_consumers.tuple_total,
                tuple_with_generator = residual_builtin_consumers.tuple_with_generator,
                "typed_builtin_generator_consumer_residuals",
            );
        }
        let final_pending_alias_use_instr_ids_by_origin =
            typed_generator_alias_ignored_instr_ids_by_origin(
                function,
                &module.module_constants,
                &generator_state_pending_alias_use_instr_ids_by_origin,
            );
        retain_typed_generator_pending_alias_use_instr_ids_by_origin(
            &mut generator_state_pending_alias_use_instr_ids_by_origin,
            final_pending_alias_use_instr_ids_by_origin.clone(),
        );
        if lower_or_remap_typed_generator_state_for_function(
            function,
            &mut module.module_constants,
            &callee_module,
            typed_generator_state_lowering_plans(
                generator_state_instr_ids_by_origin.clone(),
                &generator_state_constructors_by_origin,
                &final_pending_alias_use_instr_ids_by_origin,
                None,
            ),
            generator_state_lowering_attempt_epoch,
            None,
            &mut lowered_generator_preserved_locals,
        ) {
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
        }
        let repaired_resume_writebacks =
            ensure_typed_generator_resume_boundary_writebacks(function, &resume_preserved_locals);
        if repaired_resume_writebacks != 0 {
            tracing::info!(
                target: "soac_generator_resume_state_lowering",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                repaired_writebacks = repaired_resume_writebacks,
                "typed_generator_resume_boundary_writebacks_repaired_after_rewrites",
            );
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
        }
        let mut removed_genexpr_materializations = 0usize;
        loop {
            let pruned_unreachable_blocks = prune_unreachable_typed_blocks(function);
            let removed_generator_alias_setups =
                cleanup_lowered_typed_generator_alias_setups_for_function(
                    function,
                    &module.module_constants,
                    &generator_state_constructors_by_origin,
                    &lowered_generator_preserved_locals,
                );
            let removed_this_pass = remove_unused_typed_genexpr_function_materializations(
                function,
                &module.module_constants,
                &synthetic_genexpr_function_ids,
            );
            removed_genexpr_materializations =
                removed_genexpr_materializations.saturating_add(removed_this_pass);
            if pruned_unreachable_blocks == 0
                && removed_generator_alias_setups == 0
                && removed_this_pass == 0
            {
                break;
            }
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
        }
        if removed_genexpr_materializations != 0 {
            tracing::info!(
                target: "soac_generator_state_lowering",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                removed_genexpr_materializations,
                "typed_unused_genexpr_function_materializations_removed",
            );
        }
        assign_missing_typed_function_instr_ids(function);
        refresh_typed_function_value_facts(function);
        trace_typed_preserved_name_count(
            function,
            MAX_TYPED_INLINE_PASSES,
            "after_all_typed_rewrites",
        );
        validate_typed_preserved_storage_for_function(function)?;
        let tail_cleanup_elapsed = tail_cleanup_start.elapsed();
        tracing::info!(
            target: "soac_jit_codegen",
            event = "soac.typed_v3_function_rewrite",
            function_id = ?function.function_id,
            function_qualname = %function.names.qualname,
            inline_pass_count = u64::try_from(inline_pass_count).unwrap_or(u64::MAX),
            final_block_count = u64::try_from(function.blocks.len()).unwrap_or(u64::MAX),
            typed_rewrite_pre_inline_us = duration_micros(pre_inline_elapsed),
            typed_rewrite_inline_loop_us = duration_micros(inline_loop_elapsed),
            typed_rewrite_inline_target_build_us = duration_micros(inline_target_build_elapsed),
            typed_rewrite_inline_constructor_scan_us = duration_micros(inline_constructor_scan_elapsed),
            typed_rewrite_inline_generator_plan_us = duration_micros(inline_generator_plan_elapsed),
            typed_rewrite_inline_generator_static_us = duration_micros(inline_generator_static_elapsed),
            typed_rewrite_inline_generator_static_local_us = duration_micros(inline_generator_static_local_elapsed),
            typed_rewrite_inline_generator_remap_us = duration_micros(inline_generator_remap_elapsed),
            typed_rewrite_inline_generator_annotate_us = duration_micros(inline_generator_annotate_elapsed),
            typed_rewrite_inline_trusted_plan_us = duration_micros(inline_trusted_plan_elapsed),
            typed_rewrite_inline_trusted_owner_state_us = duration_micros(inline_trusted_owner_state_elapsed),
            typed_rewrite_inline_trusted_runtime_protocol_us = duration_micros(inline_trusted_runtime_protocol_elapsed),
            typed_rewrite_inline_trusted_static_protocol_us = duration_micros(inline_trusted_static_protocol_elapsed),
            typed_rewrite_inline_trusted_static_method_us = duration_micros(inline_trusted_static_method_elapsed),
            typed_rewrite_inline_trusted_field_callable_us = duration_micros(inline_trusted_field_callable_elapsed),
            typed_rewrite_inline_builtin_plan_us = duration_micros(inline_builtin_plan_elapsed),
            typed_rewrite_inline_target_collect_us = duration_micros(inline_target_collect_elapsed),
            typed_rewrite_inline_target_stage_us = duration_micros(inline_target_stage_elapsed),
            typed_rewrite_inline_idle_split_us = duration_micros(inline_idle_split_elapsed),
            typed_rewrite_inline_rewrite_us = duration_micros(inline_rewrite_elapsed),
            typed_rewrite_inline_state_lowering_us = duration_micros(inline_state_lowering_elapsed),
            typed_rewrite_inline_constructor_init_us = duration_micros(inline_constructor_init_elapsed),
            typed_rewrite_inline_sidecar_remap_us = duration_micros(inline_sidecar_remap_elapsed),
            typed_rewrite_inline_refresh_us = duration_micros(inline_refresh_elapsed),
            typed_rewrite_inline_tuple_simplify_us = duration_micros(inline_tuple_simplify_elapsed),
            typed_rewrite_inline_value_fact_refresh_us = duration_micros(inline_value_fact_refresh_elapsed),
            typed_rewrite_inline_call_emission_lower_us = duration_micros(inline_call_emission_lower_elapsed),
            typed_rewrite_inline_post_split_us = duration_micros(inline_post_split_elapsed),
            typed_rewrite_post_inline_cfg_us = duration_micros(post_inline_cfg_elapsed),
            typed_rewrite_final_state_lowering_us = duration_micros(final_state_lowering_elapsed),
            typed_rewrite_final_hot_split_us = duration_micros(final_hot_split_elapsed),
            typed_rewrite_final_cleanup_split_us = duration_micros(final_cleanup_split_elapsed),
            typed_rewrite_final_call_emission_us = duration_micros(final_call_emission_elapsed),
            typed_rewrite_final_stop_iteration_us = duration_micros(final_stop_iteration_elapsed),
            typed_rewrite_final_tuple_simplify_us = duration_micros(final_tuple_simplify_elapsed),
            typed_rewrite_final_tuple_rewrite_us = duration_micros(final_tuple_rewrite_elapsed),
            typed_rewrite_final_tuple_refresh_us = duration_micros(final_tuple_refresh_elapsed),
            typed_rewrite_profile_virtualize_us = duration_micros(profile_virtualize_elapsed),
            typed_rewrite_tail_cleanup_us = duration_micros(tail_cleanup_elapsed),
            trusted_owner_state_cache_builds = inline_trusted_owner_states.builds,
            trusted_owner_state_cache_reuses = inline_trusted_owner_states.reuses,
            trusted_owner_state_cache_invalidations = inline_trusted_owner_states.invalidations,
            typed_rewrite_total_us = duration_micros(function_total_start.elapsed()),
            "typed_v3_function_rewrite",
        );
    }
    let function_rewrites_elapsed = function_rewrites_start.elapsed();
    tracing::info!(
        target: "soac_jit_codegen",
        event = "soac.typed_v3_module_rewrites",
        runtime_module_id = module.module_name_gen.runtime_module_id().as_u32(),
        function_count = u64::try_from(module.callable_defs.len()).unwrap_or(u64::MAX),
        typed_rewrite_setup_us = duration_micros(setup_elapsed),
        typed_rewrite_functions_us = duration_micros(function_rewrites_elapsed),
        typed_rewrite_total_us = duration_micros(total_start.elapsed()),
        "typed_v3_module_rewrites",
    );
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum TypedBindingLocation {
    Local(LocalLocation),
    Preserved(PreservedLocation),
}

fn typed_binding_location(name: &ResolvedName) -> Option<TypedBindingLocation> {
    name.local_location()
        .map(TypedBindingLocation::Local)
        .or_else(|| {
            name.preserved_location()
                .map(TypedBindingLocation::Preserved)
        })
}

fn synthetic_genexpr_function_ids_for_module(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
) -> HashSet<RuntimeFunctionId> {
    module
        .callable_defs
        .iter()
        .filter(|function| {
            function.kind == FunctionKind::Generator && function.names.display_name == "<genexpr>"
        })
        .map(|function| function.function_id)
        .collect()
}

fn typed_genexpr_function_materialization_location(
    instr: &InstrTyped,
    synthetic_genexpr_function_ids: &HashSet<RuntimeFunctionId>,
) -> Option<TypedBindingLocation> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    let InstrTyped::MakeFunctionWithClosure(make_function) = store.value.as_ref() else {
        return None;
    };
    (make_function.kind == FunctionKind::Generator
        && synthetic_genexpr_function_ids.contains(&make_function.function_id()))
    .then(|| typed_binding_location(&store.name))
    .flatten()
}

fn typed_generator_instance_materialization_location(
    instr: &InstrTyped,
) -> Option<TypedBindingLocation> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    store
        .value
        .generator_instance_plan()
        .is_some()
        .then(|| typed_binding_location(&store.name))
        .flatten()
}

fn typed_none_placeholder_store_location(
    instr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<TypedBindingLocation> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    typed_expr_is_runtime_name_load(store.value.as_ref(), RuntimeName::None, module_constants)
        .then(|| typed_binding_location(&store.name))
        .flatten()
}

fn typed_generator_instance_materialization_binding_locations(
    instr: &InstrTyped,
) -> Option<(TypedBindingLocation, TypedBindingLocation)> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    let target = typed_binding_location(&store.name)?;
    let func = match store.value.as_ref() {
        InstrTyped::CallTyped(call) if call.extra.generator_instance_plan().is_some() => {
            call.func.as_ref()
        }
        InstrTyped::GuardedCallableCallTyped(call)
            if call.extra.generator_instance_plan().is_some() =>
        {
            call.func.as_ref()
        }
        InstrTyped::DirectCallableCallTyped(call)
            if call.extra.generator_instance_plan().is_some() =>
        {
            call.func.as_ref()
        }
        _ => return None,
    };
    let InstrTyped::Load(load) = func else {
        return None;
    };
    Some((target, typed_binding_location(&load.name)?))
}

fn typed_copy_store_binding_locations(
    instr: &InstrTyped,
) -> Option<(TypedBindingLocation, TypedBindingLocation)> {
    let InstrTyped::Store(store) = instr else {
        return None;
    };
    let target = typed_binding_location(&store.name)?;
    let InstrTyped::Load(load) = store.value.as_ref() else {
        return None;
    };
    Some((target, typed_binding_location(&load.name)?))
}

fn collect_typed_copy_alias_closure(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    roots: &HashSet<TypedBindingLocation>,
) -> HashSet<TypedBindingLocation> {
    let mut aliases = roots.clone();
    loop {
        let mut changed = false;
        for instr in function.blocks.iter().flat_map(|block| block.body.iter()) {
            if let Some((target, source)) = typed_copy_store_binding_locations(instr) {
                changed |= aliases.contains(&source) && aliases.insert(target);
            }
            if let Some((target, source)) =
                typed_generator_instance_materialization_binding_locations(instr)
            {
                changed |= aliases.contains(&source) && aliases.insert(target);
            }
        }
        if !changed {
            return aliases;
        }
    }
}

fn collect_typed_non_copy_loaded_binding_locations(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    copy_aliases: &HashSet<TypedBindingLocation>,
) -> HashSet<TypedBindingLocation> {
    struct Collector<'a> {
        copy_aliases: &'a HashSet<TypedBindingLocation>,
        current_ignored_load: Option<TypedBindingLocation>,
        locations: HashSet<TypedBindingLocation>,
    }

    impl Collector<'_> {
        fn visit_top_level_instr(&mut self, expr: &InstrTyped) {
            self.current_ignored_load = typed_copy_store_binding_locations(expr)
                .and_then(|(_, source)| self.copy_aliases.contains(&source).then_some(source))
                .or_else(|| removable_generator_instance_func_alias(expr, self.copy_aliases));
            self.visit_instr(expr);
            self.current_ignored_load = None;
        }
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::Load(load) = expr
                && let Some(location) = typed_binding_location(&load.name)
                && self.copy_aliases.contains(&location)
                && self.current_ignored_load != Some(location)
            {
                self.locations.insert(location);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        copy_aliases,
        current_ignored_load: None,
        locations: HashSet::new(),
    };
    for block in &function.blocks {
        for instr in &block.body {
            collector.visit_top_level_instr(instr);
        }
        collector.visit_term(&block.term);
    }
    collector.locations
}

fn collect_typed_non_copy_loaded_binding_contexts(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    copy_aliases: &HashSet<TypedBindingLocation>,
) -> Vec<String> {
    struct Collector<'a> {
        copy_aliases: &'a HashSet<TypedBindingLocation>,
        current_ignored_load: Option<TypedBindingLocation>,
        current_top_level: Option<String>,
        contexts: Vec<String>,
    }

    impl Collector<'_> {
        fn visit_top_level_instr(&mut self, expr: &InstrTyped) {
            self.current_ignored_load = typed_copy_store_binding_locations(expr)
                .and_then(|(_, source)| self.copy_aliases.contains(&source).then_some(source))
                .or_else(|| removable_generator_instance_func_alias(expr, self.copy_aliases));
            self.current_top_level = Some(format!("{expr:?}"));
            self.visit_instr(expr);
            self.current_ignored_load = None;
            self.current_top_level = None;
        }
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if self.contexts.len() >= 4 {
                return;
            }
            if let InstrTyped::Load(load) = expr
                && let Some(location) = typed_binding_location(&load.name)
                && self.copy_aliases.contains(&location)
                && self.current_ignored_load != Some(location)
            {
                self.contexts.push(format!(
                    "{location:?} in {}",
                    self.current_top_level.as_deref().unwrap_or("<term>")
                ));
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        copy_aliases,
        current_ignored_load: None,
        current_top_level: None,
        contexts: Vec::new(),
    };
    for block in &function.blocks {
        for instr in &block.body {
            collector.visit_top_level_instr(instr);
        }
        collector.current_top_level = Some(format!("{:?}", block.term));
        collector.visit_term(&block.term);
        collector.current_top_level = None;
    }
    collector.contexts
}

fn removable_generator_instance_func_alias(
    instr: &InstrTyped,
    aliases: &HashSet<TypedBindingLocation>,
) -> Option<TypedBindingLocation> {
    let (target, source) = typed_generator_instance_materialization_binding_locations(instr)?;
    (aliases.contains(&target) && aliases.contains(&source)).then_some(source)
}

fn typed_copy_alias_closure_has_only_removable_stores(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    synthetic_genexpr_function_ids: &HashSet<RuntimeFunctionId>,
    aliases: &HashSet<TypedBindingLocation>,
) -> bool {
    function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .all(|instr| {
            let InstrTyped::Store(store) = instr else {
                return true;
            };
            let Some(location) = typed_binding_location(&store.name) else {
                return true;
            };
            if !aliases.contains(&location) {
                return true;
            }
            typed_genexpr_function_materialization_location(instr, synthetic_genexpr_function_ids)
                == Some(location)
                || typed_generator_instance_materialization_location(instr) == Some(location)
                || typed_none_placeholder_store_location(instr, module_constants) == Some(location)
                || typed_copy_store_binding_locations(instr)
                    .is_some_and(|(_, source)| aliases.contains(&source))
        })
}

fn remove_unused_typed_genexpr_function_materializations(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    synthetic_genexpr_function_ids: &HashSet<RuntimeFunctionId>,
) -> usize {
    let candidate_locations = function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(|instr| {
            typed_genexpr_function_materialization_location(instr, synthetic_genexpr_function_ids)
        })
        .collect::<HashSet<_>>();
    let candidate_generator_instance_locations = function
        .blocks
        .iter()
        .flat_map(|block| block.body.iter())
        .filter_map(typed_generator_instance_materialization_location)
        .collect::<HashSet<_>>();
    if candidate_locations.is_empty() && candidate_generator_instance_locations.is_empty() {
        return 0;
    }
    tracing::debug!(
        target: "soac_generator_state_lowering",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        candidate_function_locations = ?candidate_locations,
        candidate_generator_instance_locations = ?candidate_generator_instance_locations,
        "typed_unused_genexpr_materialization_candidates",
    );

    let candidate_roots = candidate_locations
        .union(&candidate_generator_instance_locations)
        .copied()
        .collect::<HashSet<_>>();
    let mut removable_locations = HashSet::new();
    for root in candidate_roots {
        let root_aliases = collect_typed_copy_alias_closure(function, &HashSet::from([root]));
        let externally_loaded_locations =
            collect_typed_non_copy_loaded_binding_locations(function, &root_aliases);
        let has_only_removable_stores = typed_copy_alias_closure_has_only_removable_stores(
            function,
            module_constants,
            synthetic_genexpr_function_ids,
            &root_aliases,
        );
        let externally_unused = root_aliases.is_disjoint(&externally_loaded_locations);
        if externally_unused && has_only_removable_stores {
            removable_locations.extend(root_aliases);
        } else {
            let external_load_contexts =
                collect_typed_non_copy_loaded_binding_contexts(function, &root_aliases);
            tracing::debug!(
                target: "soac_generator_state_lowering",
                function_id = ?function.function_id,
                function_qualname = %function.names.qualname,
                root = ?root,
                root_aliases = ?root_aliases,
                externally_loaded_locations = ?externally_loaded_locations,
                externally_unused,
                has_only_removable_stores,
                external_load_contexts = ?external_load_contexts,
                "typed_unused_genexpr_materialization_retained",
            );
        }
    }
    if removable_locations.is_empty() {
        return 0;
    }

    let mut removed = 0;
    for block in &mut function.blocks {
        block.body.retain(|instr| {
            let remove = match instr {
                InstrTyped::Store(store) => {
                    typed_binding_location(&store.name).is_some_and(|location| {
                        removable_locations.contains(&location)
                            && (typed_genexpr_function_materialization_location(
                                instr,
                                synthetic_genexpr_function_ids,
                            ) == Some(location)
                                || typed_generator_instance_materialization_location(instr)
                                    == Some(location)
                                || typed_none_placeholder_store_location(instr, module_constants)
                                    == Some(location)
                                || typed_copy_store_binding_locations(instr).is_some_and(
                                    |(_, source)| removable_locations.contains(&source),
                                ))
                    })
                }
                InstrTyped::Del(del) => typed_binding_location(&del.name)
                    .is_some_and(|location| removable_locations.contains(&location)),
                _ => false,
            };
            removed += usize::from(remove);
            !remove
        });
    }
    removed
}

fn cleanup_lowered_typed_generator_alias_setups_for_function(
    function: &mut BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    constructors_by_origin: &HashMap<InstrId, TypedGeneratorStateConstructor>,
    lowered_preserved_locals: &LoweredGeneratorPreservedLocals,
) -> usize {
    let mut removed = 0;
    for origin in lowered_preserved_locals.keys() {
        let Some(constructor) = constructors_by_origin.get(origin) else {
            continue;
        };
        removed += cleanup_lowered_typed_generator_alias_setup_with_existing_constructor(
            function,
            module_constants,
            constructor,
            &HashSet::new(),
        );
    }
    if removed != 0 {
        refresh_typed_function_value_facts(function);
    }
    removed
}

#[derive(Default)]
struct ResidualBuiltinGeneratorConsumerCounts {
    list_total: usize,
    list_with_generator: usize,
    set_total: usize,
    set_with_generator: usize,
    tuple_total: usize,
    tuple_with_generator: usize,
}

impl ResidualBuiltinGeneratorConsumerCounts {
    fn total(&self) -> usize {
        self.list_total + self.set_total + self.tuple_total
    }
}

fn residual_builtin_generator_consumer_counts_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> ResidualBuiltinGeneratorConsumerCounts {
    struct Finder<'a> {
        module_constants: &'a [ConstantExpr],
        counts: ResidualBuiltinGeneratorConsumerCounts,
    }

    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            let residual = match expr {
                InstrTyped::CallTyped(call) if call.keywords.is_empty() => {
                    Some((call.func.as_ref(), call.args.as_slice()))
                }
                InstrTyped::GuardedCallableCallTyped(call) if call.keywords.is_empty() => {
                    Some((call.func.as_ref(), call.args.as_slice()))
                }
                InstrTyped::DirectCallableCallTyped(call) => {
                    Some((call.func.as_ref(), call.args.as_slice()))
                }
                _ => None,
            };
            if let Some((func, [CallArgPositional::Positional(arg)])) = residual {
                let has_generator_plan = arg.generator_instance_plan().is_some();
                match typed_expr_runtime_name_provenance(func, self.module_constants) {
                    Some(RuntimeName::List) => {
                        self.counts.list_total += 1;
                        self.counts.list_with_generator += usize::from(has_generator_plan);
                    }
                    Some(RuntimeName::Set) => {
                        self.counts.set_total += 1;
                        self.counts.set_with_generator += usize::from(has_generator_plan);
                    }
                    Some(RuntimeName::Tuple) => {
                        self.counts.tuple_total += 1;
                        self.counts.tuple_with_generator += usize::from(has_generator_plan);
                    }
                    _ => {}
                }
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        module_constants,
        counts: ResidualBuiltinGeneratorConsumerCounts::default(),
    };
    finder.visit_fn(function);
    finder.counts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jit::specialization_profile::DirectCallEmissionScope;
    use soac_core::block_py::{
        InstrWithConstantNone, Load, LocalFunctionId, NameLocation, ResolvedName,
        RuntimeFunctionId, RuntimeModuleId,
    };
    use soac_ir_typed::lower_blockpy_module_to_typed;
    use soac_opt::passes::{
        TypedInlineInstanceSource, TypedInlineRewriteStats,
        lower_typed_function_call_access_plan_instrs, merge_trusted_owner_states,
        remap_trusted_owner_state_for_edge, trusted_owner_block_predecessor_edges,
    };

    #[test]
    fn pinned_pyperformance_nqueens_discard_reaches_full_typed_pipeline() {
        let source = include_str!("fixtures/opaque_fused_pyperformance_nqueens_v1.py");
        let exact_source_match =
            crate::jit::opaque_fused_iteration::tracked_nqueens_source_matches(source);
        assert!(exact_source_match);
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("pinned pyperformance N-Queens should lower")
            .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        assert_eq!(
            generator_targets
                .keys()
                .map(String::as_str)
                .collect::<HashSet<_>>(),
            HashSet::from(["permutations", "n_queens"])
        );
        let suppressed_source_generators = generator_targets
            .values()
            .map(|function| function.function_id)
            .collect();
        let static_targets = StaticDirectCallTargets {
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            suppressed_source_generators,
            ..StaticDirectCallTargets::default()
        };
        let plan = optimize_blockpy_with_external_inline_callees(
            &lowered,
            None,
            &SoacEnvConfig::default().with_specialization_mode(Some(SpecializationMode::Apply)),
            HashMap::new(),
            static_targets,
            exact_source_match,
        )
        .expect("pinned pyperformance N-Queens should admit opaque fusion");
        let root = plan
            .module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "bench_n_queens")
            .expect("typed plan should retain bench_n_queens");
        let fused = root
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(InstrTyped::opaque_fused_iteration_plan)
            .collect::<Vec<_>>();
        let [fused] = fused.as_slice() else {
            panic!("expected exactly one body-hosted fused plan, got {fused:#?}");
        };
        assert_eq!(fused.result, soac_ir_typed::TypedOpaqueFusedResult::Discard);
        assert_eq!((fused.minimum_width, fused.maximum_width), (0, 8));
    }

    fn assert_no_dead_synthetic_generator_materializations(
        function: &BlockPyFunction<TypedBlockPyModuleShape>,
        module_constants: &[ConstantExpr],
        synthetic_genexpr_function_ids: &HashSet<RuntimeFunctionId>,
        context: &str,
    ) {
        let materialization_roots = function
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                typed_genexpr_function_materialization_location(
                    instr,
                    synthetic_genexpr_function_ids,
                )
                .or_else(|| typed_generator_instance_materialization_location(instr))
            })
            .collect::<HashSet<_>>();

        for root in materialization_roots {
            let aliases = collect_typed_copy_alias_closure(function, &HashSet::from([root]));
            let externally_loaded =
                collect_typed_non_copy_loaded_binding_locations(function, &aliases);
            let has_only_removable_stores = typed_copy_alias_closure_has_only_removable_stores(
                function,
                module_constants,
                synthetic_genexpr_function_ids,
                &aliases,
            );
            assert!(
                !aliases.is_disjoint(&externally_loaded) || !has_only_removable_stores,
                "{context}: unused generator function or instance materialization should have been removed; root={root:?}; aliases={aliases:?}; externally_loaded={externally_loaded:?}; has_only_removable_stores={has_only_removable_stores}; external_load_contexts={:?}",
                collect_typed_non_copy_loaded_binding_contexts(function, &aliases),
            );
        }
    }

    #[test]
    fn typed_inline_cfg_budget_prioritizes_generator_resume_then_trusted_protocol() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller():\n    return None\n\ndef target():\n    return None\n",
        )
        .expect("source should lower")
        .blockpy_module;
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut function = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let target = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "target")
            .map(|function| function.function_id)
            .expect("typed inline target should exist");
        let template = function
            .blocks
            .first()
            .cloned()
            .expect("typed caller should have an entry block");
        while function.blocks.len() < MAX_TYPED_INLINE_FUNCTION_BLOCKS - 5 {
            let mut block = template.clone();
            block.label = function.name_gen.next_block_name();
            function.blocks.push(block);
        }

        let ordinary_source = InstrId::new(1);
        let builtin_source = InstrId::new(2);
        let protocol_source = InstrId::new(3);
        let resume_source = InstrId::new(4);
        let arg_plan = TypedDirectCallArgPlan {
            sources: Vec::new(),
        };
        let targets = HashMap::from([
            (ordinary_source, vec![(target, arg_plan.clone())]),
            (builtin_source, vec![(target, arg_plan.clone())]),
            (protocol_source, vec![(target, arg_plan.clone())]),
            (resume_source, vec![(target, arg_plan)]),
        ]);
        let resume_plans = HashMap::from([(
            resume_source,
            TypedGeneratorResumePlan {
                function_id: target,
                generator_origin: Some(resume_source),
                candidate_origins: vec![resume_source],
            },
        )]);
        let protocol_sources = HashSet::from([protocol_source]);
        let builtin_sources = HashSet::from([builtin_source]);

        let selected = select_typed_inline_targets_within_cfg_budget_and_priorities(
            &function,
            &typed,
            &HashMap::new(),
            &resume_plans,
            &protocol_sources,
            &builtin_sources,
            targets.clone(),
            Some(1),
        );
        assert_eq!(
            selected.keys().copied().collect::<HashSet<_>>(),
            HashSet::from([resume_source]),
            "the final CFG space must go to a proven generator resume, not a lower-id helper",
        );

        let selected = select_typed_inline_targets_within_cfg_budget_and_priorities(
            &function,
            &typed,
            &HashMap::new(),
            &HashMap::new(),
            &protocol_sources,
            &builtin_sources,
            targets,
            Some(1),
        );
        assert_eq!(
            selected.keys().copied().collect::<HashSet<_>>(),
            HashSet::from([protocol_source]),
            "a proven runtime protocol must precede generator consumers and unrelated calls",
        );
    }

    #[test]
    fn typed_inline_cfg_budget_admits_one_deterministic_builtin_consumer() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller():\n    return None\n\ndef target():\n    return None\n",
        )
        .expect("source should lower")
        .blockpy_module;
        let typed = lower_blockpy_module_to_typed(lowered);
        let function = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let target = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "target")
            .map(|function| function.function_id)
            .expect("typed inline target should exist");
        let first_source = InstrId::new(3);
        let second_source = InstrId::new(9);
        let third_source = InstrId::new(17);
        let arg_plan = TypedDirectCallArgPlan {
            sources: Vec::new(),
        };
        let builtin_sources = HashSet::from([third_source, first_source, second_source]);
        let targets = HashMap::from([
            (third_source, vec![(target, arg_plan.clone())]),
            (first_source, vec![(target, arg_plan.clone())]),
            (second_source, vec![(target, arg_plan)]),
        ]);

        let selected = select_typed_inline_targets_within_cfg_budget_and_priorities(
            function,
            &typed,
            &HashMap::new(),
            &HashMap::new(),
            &HashSet::new(),
            &builtin_sources,
            targets,
            Some(1),
        );
        assert_eq!(
            selected.keys().copied().collect::<HashSet<_>>(),
            HashSet::from([first_source]),
            "each refresh must admit only the lowest-id consumer and retain resume CFG headroom",
        );
    }

    #[test]
    fn only_admitted_builtin_sidecars_can_trigger_one_inline_rewrite() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def implementation(value):
    return value

def caller(left, right):
    first = tuple(left)
    second = tuple(right)
    return first, second
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut function = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        linearize_typed_function_expressions(&mut function)
            .expect("typed tuple consumer expressions should linearize");
        assign_missing_typed_function_instr_ids(&mut function);
        let target = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "implementation")
            .map(|function| function.function_id)
            .expect("typed builtin implementation should exist");

        #[derive(Default)]
        struct TupleCallSources {
            sources: Vec<InstrId>,
        }

        impl TupleCallSources {
            fn record(&mut self, instr_id: Option<InstrId>) {
                self.sources
                    .push(instr_id.expect("linearized tuple consumer should have a semantic ID"));
            }
        }

        impl Visit<InstrTyped> for TupleCallSources {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                match expr {
                    InstrTyped::CallTyped(call) => {
                        self.record(call.try_semantic_instr_id());
                    }
                    InstrTyped::GuardedCallableCallTyped(call) => {
                        self.record(call.try_semantic_instr_id());
                    }
                    InstrTyped::DirectCallableCallTyped(call) => {
                        self.record(call.try_semantic_instr_id());
                    }
                    _ => {}
                }
                expr.visit_children(self);
            }
        }

        let mut tuple_calls = TupleCallSources::default();
        tuple_calls.visit_fn(&function);
        let mut sources = tuple_calls.sources;
        sources.sort_by_key(|source| source.index());
        sources.dedup();
        let [selected_source, deferred_source] = sources.as_slice() else {
            panic!(
                "caller should contain exactly two distinct tuple consumer sites; found {}",
                sources.len(),
            );
        };
        let selected_source = *selected_source;
        let deferred_source = *deferred_source;
        let arg_plan = TypedDirectCallArgPlan {
            sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
        };
        let plans = HashMap::from([
            (
                selected_source,
                TypedBuiltinImplementationPlan {
                    source: RuntimeName::Tuple,
                    function_id: target,
                    arg_plan: arg_plan.clone(),
                },
            ),
            (
                deferred_source,
                TypedBuiltinImplementationPlan {
                    source: RuntimeName::Tuple,
                    function_id: target,
                    arg_plan: arg_plan.clone(),
                },
            ),
        ]);
        assert_eq!(
            annotate_typed_builtin_implementation_plans(&mut function, &plans)
                .expect("the fixture should initially annotate both builtin sites"),
            2,
        );
        let selected_targets = HashMap::from([(selected_source, vec![(target, arg_plan)])]);
        assert_eq!(
            retain_selected_typed_builtin_implementation_plans(
                &mut function,
                &plans,
                &selected_targets,
            )
            .expect("only the admitted builtin site should remain annotated"),
            1,
        );

        let mut module_constants = typed.module_constants.clone();
        let stats =
            inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls(
                &mut function,
                &typed,
                &mut module_constants,
                &HashMap::new(),
                &selected_targets,
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
                &HashMap::new(),
            );
        assert_eq!(stats.rewritten_stores, 1);
        assert_eq!(stats.inline_instance_sources.len(), 1);
        assert_eq!(
            stats.inline_instance_sources[0].source_instr_id, selected_source,
            "deferred builtin sidecars must not bypass deterministic source admission",
        );
    }

    #[test]
    fn typed_inline_cfg_budget_rejects_functions_at_block_limit() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def caller():\n    return None\n")
                .expect("source should lower")
                .blockpy_module;
        let mut typed = lower_blockpy_module_to_typed(lowered);
        let function = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let template = function
            .blocks
            .first()
            .cloned()
            .expect("typed caller should have an entry block");

        while function.blocks.len() < MAX_TYPED_INLINE_FUNCTION_BLOCKS - 1 {
            let mut block = template.clone();
            block.label = function.name_gen.next_block_name();
            function.blocks.push(block);
        }
        assert!(
            typed_inline_function_within_cfg_budget(function),
            "functions below the aggregate block limit should remain inlineable",
        );

        let mut final_block = template;
        final_block.label = function.name_gen.next_block_name();
        function.blocks.push(final_block);
        assert!(
            !typed_inline_function_within_cfg_budget(function),
            "functions at the aggregate block limit must stop inlining",
        );
    }

    #[test]
    fn typed_inline_cfg_budget_rejects_functions_at_body_instruction_limit() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def caller():\n    return None\n")
                .expect("source should lower")
                .blockpy_module;
        let mut typed = lower_blockpy_module_to_typed(lowered);
        let function = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        function.blocks.truncate(1);
        let entry = function
            .blocks
            .first_mut()
            .expect("typed caller should have an entry block");
        entry.body = vec![InstrTyped::constant_none(); MAX_TYPED_INLINE_FUNCTION_BODY_INSTRS - 2];
        entry.term = BlockTerm::Return(InstrTyped::constant_none());
        assert!(
            typed_inline_function_within_cfg_budget(function),
            "functions below the aggregate instruction limit should remain inlineable",
        );

        function.blocks[0].body.push(InstrTyped::constant_none());
        assert!(
            !typed_inline_function_within_cfg_budget(function),
            "functions at the aggregate instruction limit must stop inlining",
        );
    }

    #[test]
    fn cloned_hot_split_suppression_follows_nested_clone_instr_ids() {
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(1), LocalFunctionId::new(2));
        let suppressed_source = InstrId::new(3);
        let first_clone = InstrId::new(4);
        let nested_clone = InstrId::new(5);
        let unrelated_clone = InstrId::new(6);
        let mappings = vec![
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 0,
                callee_instr_id: suppressed_source,
                caller_instr_id: first_clone,
            },
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 1,
                callee_instr_id: first_clone,
                caller_instr_id: nested_clone,
            },
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 2,
                callee_instr_id: InstrId::new(7),
                caller_instr_id: unrelated_clone,
            },
        ];
        let mut suppressed = HashSet::from([suppressed_source]);

        remap_cloned_suppressed_hot_split_instr_ids(&mut suppressed, &mappings);

        assert_eq!(
            suppressed,
            HashSet::from([suppressed_source, first_clone, nested_clone])
        );
        assert!(!suppressed.contains(&unrelated_clone));
    }

    #[test]
    fn live_genexpr_instances_retain_their_function_materializations() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def caller(values):
    return (value for value in values)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let mut typed = lower_blockpy_module_to_typed(lowered);
        let static_targets = StaticDirectCallTargets::default();
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let synthetic_genexpr_function_ids = synthetic_genexpr_function_ids_for_module(&typed);
        let caller = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let retained_materializations_before_cleanup = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                typed_genexpr_function_materialization_location(
                    instr,
                    &synthetic_genexpr_function_ids,
                )
            })
            .collect::<Vec<_>>();
        assert!(
            !retained_materializations_before_cleanup.is_empty(),
            "returning a live genexpr should still materialize its generator function before cleanup"
        );

        let removed = remove_unused_typed_genexpr_function_materializations(
            caller,
            &typed.module_constants,
            &synthetic_genexpr_function_ids,
        );

        assert_eq!(
            removed, 0,
            "cleanup must retain the genexpr function while the derived generator instance escapes"
        );
        let retained_materializations_after_cleanup = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                typed_genexpr_function_materialization_location(
                    instr,
                    &synthetic_genexpr_function_ids,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            retained_materializations_after_cleanup, retained_materializations_before_cleanup,
            "cleanup should leave the escaping genexpr function materialization intact"
        );
    }

    #[test]
    fn static_constructor_calls_follow_ordinary_inlining() {
        let iter_range_owner = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "IterRange".to_string(),
        };
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let static_constructor_calls = HashMap::from([(
            function_id,
            HashMap::from([(InstrId::new(3), iter_range_owner.clone())]),
        )]);
        let stats = TypedInlineRewriteStats {
            inline_instance_sources: vec![
                TypedInlineInstanceSource {
                    inline_instance: 0,
                    source_instr_id: InstrId::new(1),
                },
                TypedInlineInstanceSource {
                    inline_instance: 1,
                    source_instr_id: InstrId::new(2),
                },
            ],
            instr_id_mappings: vec![
                TypedInlineInstrIdMapping {
                    callee: function_id,
                    inline_instance: 0,
                    callee_instr_id: InstrId::new(3),
                    caller_instr_id: InstrId::new(8),
                },
                TypedInlineInstrIdMapping {
                    callee: function_id,
                    inline_instance: 1,
                    callee_instr_id: InstrId::new(3),
                    caller_instr_id: InstrId::new(9),
                },
            ],
            ..TypedInlineRewriteStats::default()
        };

        assert_eq!(
            remapped_static_constructor_calls_from_inline_stats(&stats, &static_constructor_calls),
            HashMap::from([
                (InstrId::new(8), iter_range_owner.clone()),
                (InstrId::new(9), iter_range_owner),
            ]),
        );
    }

    #[test]
    fn static_constructor_calls_follow_protocol_inlines() {
        let iter_range_owner = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "IterRange".to_string(),
        };
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let static_constructor_calls = HashMap::from([(
            function_id,
            HashMap::from([(InstrId::new(3), iter_range_owner.clone())]),
        )]);
        let stats = TypedInlineRewriteStats {
            inline_instance_sources: vec![TypedInlineInstanceSource {
                inline_instance: 0,
                source_instr_id: InstrId::new(1),
            }],
            instr_id_mappings: vec![TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 0,
                callee_instr_id: InstrId::new(3),
                caller_instr_id: InstrId::new(8),
            }],
            ..TypedInlineRewriteStats::default()
        };

        assert_eq!(
            remapped_static_constructor_calls_from_inline_stats(&stats, &static_constructor_calls),
            HashMap::from([(InstrId::new(8), iter_range_owner)]),
        );
    }

    #[test]
    fn trusted_materialized_constructor_calls_follow_inlined_constructor_entries() {
        let range_owner = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "range".to_string(),
        };
        let trusted_call_sources = HashMap::from([(InstrId::new(1), range_owner.clone())]);
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let stats = TypedInlineRewriteStats {
            inline_instance_sources: vec![TypedInlineInstanceSource {
                inline_instance: 0,
                source_instr_id: InstrId::new(1),
            }],
            instr_id_mappings: vec![
                TypedInlineInstrIdMapping {
                    callee: function_id,
                    inline_instance: 0,
                    callee_instr_id: InstrId::new(3),
                    caller_instr_id: InstrId::new(8),
                },
                TypedInlineInstrIdMapping {
                    callee: function_id,
                    inline_instance: 1,
                    callee_instr_id: InstrId::new(4),
                    caller_instr_id: InstrId::new(9),
                },
            ],
            ..TypedInlineRewriteStats::default()
        };
        let constructor_field_bindings = HashMap::from([(
            InstrId::new(8),
            TypedConstructorFieldBindings { fields: Vec::new() },
        )]);

        assert_eq!(
            trusted_materialized_constructor_calls_from_inline_stats(
                &stats,
                &trusted_call_sources,
                &constructor_field_bindings,
            ),
            HashMap::from([(InstrId::new(8), range_owner)]),
        );
    }

    #[test]
    fn trusted_runtime_protocol_calls_follow_known_constructor_owner_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def make():\n    return None\n\n\
def caller():\n    it = make()\n    value = next(it)\n    return value\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "IterRange".to_string(),
        };
        let mut constructor_instr_id = None;
        let mut protocol_instr_id = None;
        for block in &mut caller.blocks {
            for instr in &mut block.body {
                let InstrTyped::Store(store) = instr else {
                    continue;
                };
                let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                    continue;
                };
                let instr_id = call
                    .try_semantic_instr_id()
                    .expect("lowered call should have semantic id");
                if constructor_instr_id.is_none() {
                    constructor_instr_id = Some(instr_id);
                    continue;
                }
                call.access = soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                    runtime_name: RuntimeName::Next,
                    method_name: "__next__".to_string(),
                    method_guards: vec![TypedDirectMethodCallGuard {
                        function_id: RuntimeFunctionId::new(
                            RuntimeModuleId::new(2),
                            LocalFunctionId::new(7),
                        ),
                        owner_type_ref: owner_type_ref.clone(),
                        type_version: 1,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
                        },
                    }],
                };
                protocol_instr_id = Some(instr_id);
            }
        }
        let constructor_instr_id =
            constructor_instr_id.expect("caller should contain a constructor-like call");
        let protocol_instr_id = protocol_instr_id.expect("caller should contain a protocol call");

        assert_eq!(
            trusted_runtime_protocol_calls_for_function(
                caller,
                &typed.module_constants,
                Some(&HashMap::from([(
                    constructor_instr_id,
                    owner_type_ref.clone()
                )])),
                &HashMap::new(),
            ),
            HashMap::from([(protocol_instr_id, owner_type_ref)]),
        );
    }

    #[test]
    fn trusted_static_protocol_inlines_follow_identity_iter_aliases() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def caller(limit):
    gen = values(limit)
    iterator = iter(gen)
    return next(iterator)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class ClosureGenerator:\n    def __next__(self):\n        return 1\n",
        )
        .expect("runtime source should lower");
        let next_function = runtime_lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("runtime next function should exist");
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let static_targets = StaticDirectCallTargets {
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([(
                (
                    "soac.runtime".to_string(),
                    "ClosureGenerator".to_string(),
                    "__next__".to_string(),
                ),
                next_function.clone(),
            )]),
            ..StaticDirectCallTargets::default()
        };
        let plans = static_generator_instance_plans_for_function(&caller, &static_targets);
        annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
            .expect("instance-plan annotation should succeed");
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .any(|instr| matches!(
                    instr,
                    InstrTyped::Store(store)
                        if matches!(
                            store.value.as_ref(),
                            InstrTyped::CallTyped(call)
                                if typed_expr_is_runtime_name_load(
                                    call.func.as_ref(),
                                    RuntimeName::Iter,
                                    &typed.module_constants,
                                )
                        )
                )),
            "caller should retain the runtime iter(gen) store",
        );
        let next_instr_id = caller
            .blocks
            .iter()
            .find_map(|block| match &block.term {
                BlockTerm::Return(InstrTyped::CallTyped(call))
                    if typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        &typed.module_constants,
                    ) =>
                {
                    call.try_semantic_instr_id()
                }
                _ => None,
            })
            .expect("caller should return next(iterator)");
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "ClosureGenerator".to_string(),
        };
        let analysis = analyze_trusted_owner_states(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        let (next_receiver, next_state) = caller
            .blocks
            .iter()
            .find_map(|block| {
                let BlockTerm::Return(InstrTyped::CallTyped(call)) = &block.term else {
                    return None;
                };
                if !typed_expr_is_runtime_name_load(
                    call.func.as_ref(),
                    RuntimeName::Next,
                    &typed.module_constants,
                ) {
                    return None;
                }
                let soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(receiver)) =
                    call.args.first()?
                else {
                    return None;
                };
                Some((
                    receiver.name.clone(),
                    analysis
                        .block_before_term
                        .get(&block.label)
                        .expect("next() term should have owner state"),
                ))
            })
            .expect("caller should return next(iterator)");
        assert_eq!(
            trusted_owner_state_for_name(&next_receiver, next_state),
            Some(&owner_type_ref),
            "identity iter aliases should preserve trusted owner facts: {next_state:#?}",
        );
        assert!(
            trusted_object_origin_for_name(&next_receiver, next_state).is_some(),
            "identity iter aliases should preserve trusted object origins: {next_state:#?}",
        );
        let (owners, inline_targets, _, _) = trusted_static_runtime_protocol_inlines_for_function(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &static_targets,
        );
        assert_eq!(owners, HashMap::from([(next_instr_id, owner_type_ref)]));
        assert_eq!(
            inline_targets,
            HashMap::from([(
                next_instr_id,
                vec![(
                    next_function.function_id,
                    TypedDirectCallArgPlan {
                        sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );
    }

    #[test]
    fn trusted_static_protocol_inlines_follow_generic_runtime_calls_in_terms() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def make():\n    return None\n\n\
def caller():\n    it = make()\n    return next(it)\n",
        )
        .expect("source should lower");
        let next_function = lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "IterRange.__next__")
            .cloned()
            .expect("IterRange.__next__ should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let module_constants = typed.module_constants.clone();
        let caller = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let mut protocol_instr_id = None;
        let constructor_init_function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(7), LocalFunctionId::new(11));
        for block in &mut caller.blocks {
            for instr in &mut block.body {
                if let InstrTyped::Store(store) = instr
                    && let InstrTyped::CallTyped(call) = store.value.as_mut()
                {
                    call.extra.set_constructor_init_plan(TypedConstructorInitPlan {
                        source:
                            TypedConstructorInitPlanSource::InlinedConstructorEntryWithInlinedInitBody,
                        init_function_id: constructor_init_function_id,
                    });
                }
            }
            if let BlockTerm::Return(InstrTyped::CallTyped(call)) = &block.term
                && typed_expr_is_runtime_name_load(
                    call.func.as_ref(),
                    RuntimeName::Next,
                    &typed.module_constants,
                )
            {
                protocol_instr_id = call.try_semantic_instr_id();
            }
        }
        let protocol_instr_id = protocol_instr_id.expect("caller should contain next()");
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "pkg.runtime".to_string(),
            qualname: "IterRange".to_string(),
        };
        let (owners, inline_targets, _, _) = trusted_static_runtime_protocol_inlines_for_function(
            caller,
            &module_constants,
            &HashMap::new(),
            &HashMap::from([(constructor_init_function_id, owner_type_ref.clone())]),
            &StaticDirectCallTargets {
                strict_methods: HashMap::from([(
                    (
                        "pkg.runtime".to_string(),
                        "IterRange".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function.clone(),
                )]),
                ..StaticDirectCallTargets::default()
            },
        );
        assert_eq!(owners, HashMap::from([(protocol_instr_id, owner_type_ref)]));
        assert_eq!(
            inline_targets,
            HashMap::from([(
                protocol_instr_id,
                vec![(
                    next_function.function_id,
                    TypedDirectCallArgPlan {
                        sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );
    }

    #[test]
    fn trusted_field_callable_inlines_follow_function_values_stored_on_trusted_objects() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def make():
    return None

def caller():
    def resume(owner, value, exc):
        return value
    gen = make()
    gen._resume_fn = resume
    return gen._resume_fn(gen, 1, None)
"#,
        )
        .expect("source should lower");
        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let resume = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller.<locals>.resume")
            .expect("nested resume should exist");
        let mut constructor_instr_id = None;
        let mut field_call_instr_id = None;
        for block in &caller.blocks {
            for instr in &block.body {
                let InstrTyped::Store(store) = instr else {
                    continue;
                };
                if let InstrTyped::CallTyped(call) = store.value.as_ref() {
                    if constructor_instr_id.is_none() {
                        constructor_instr_id = call.try_semantic_instr_id();
                    }
                }
            }
            if let BlockTerm::Return(InstrTyped::CallTyped(call)) = &block.term {
                field_call_instr_id = call.try_semantic_instr_id();
            }
        }
        let constructor_instr_id =
            constructor_instr_id.expect("caller should contain constructor-like call");
        let field_call_instr_id = field_call_instr_id.expect("caller should return a field call");
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "ClosureGenerator".to_string(),
        };

        let (emissions, inline_targets) = trusted_field_callable_inlines_for_function(
            caller,
            &typed.module_constants,
            &HashMap::from([(constructor_instr_id, owner_type_ref)]),
            &HashMap::new(),
            &typed,
            &HashMap::new(),
        )
        .expect("trusted field callable planning should succeed");

        assert_eq!(
            emissions.by_source.get(&field_call_instr_id),
            Some(&TypedCallEmissionPlan::DirectCallable {
                function_guard: TypedDirectFunctionCallGuard {
                    function_id: resume.function_id,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![
                            soac_ir_typed::TypedDirectCallArgSource::Provided(0),
                            soac_ir_typed::TypedDirectCallArgSource::Provided(1),
                            soac_ir_typed::TypedDirectCallArgSource::Provided(2),
                        ],
                    },
                },
            }),
        );
        assert_eq!(
            inline_targets,
            HashMap::from([(
                field_call_instr_id,
                vec![(
                    resume.function_id,
                    TypedDirectCallArgPlan {
                        sources: vec![
                            soac_ir_typed::TypedDirectCallArgSource::Provided(0),
                            soac_ir_typed::TypedDirectCallArgSource::Provided(1),
                            soac_ir_typed::TypedDirectCallArgSource::Provided(2),
                        ],
                    },
                )],
            )]),
        );

        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def make():
    return None

def sink(value):
    return None

def caller():
    def resume(owner, value, exc):
        return value
    gen = make()
    gen._resume_fn = resume
    sink(gen)
    return gen._resume_fn(gen, 1, None)
"#,
        )
        .expect("source should lower");
        let escaped = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = escaped
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let mut constructor_instr_id = None;
        for block in &caller.blocks {
            for instr in &block.body {
                let InstrTyped::Store(store) = instr else {
                    continue;
                };
                if let InstrTyped::CallTyped(call) = store.value.as_ref()
                    && constructor_instr_id.is_none()
                {
                    constructor_instr_id = call.try_semantic_instr_id();
                }
            }
        }
        let constructor_instr_id =
            constructor_instr_id.expect("caller should contain constructor-like call");
        let (emissions, inline_targets) = trusted_field_callable_inlines_for_function(
            caller,
            &escaped.module_constants,
            &HashMap::from([(
                constructor_instr_id,
                TypedAttrOwnerRef::TypeKey {
                    module_name: "soac.runtime".to_string(),
                    qualname: "ClosureGenerator".to_string(),
                },
            )]),
            &HashMap::new(),
            &escaped,
            &HashMap::new(),
        )
        .expect("escaped field callable planning should still succeed");
        assert!(emissions.is_empty());
        assert!(inline_targets.is_empty());
    }

    #[test]
    fn generator_instance_plans_seed_trusted_owner_and_resume_field_state() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def caller(limit):
    gen = values(limit)
    return next(gen)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let target_function_id = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "values")
            .expect("generator target should exist")
            .function_id;
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let targets = StaticDirectCallTargets {
            runtime_names: HashMap::new(),
            runtime_builtin_implementations: HashMap::new(),
            module_globals: HashMap::new(),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::new(),
            suppressed_source_generators: HashSet::new(),
        };
        let plans = static_generator_instance_plans_for_function(&caller, &targets);
        annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
            .expect("instance-plan annotation should succeed");

        let analysis = analyze_trusted_owner_states(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        let (next_instr_id, state) = caller
            .blocks
            .iter()
            .find_map(|block| {
                let BlockTerm::Return(InstrTyped::CallTyped(call)) = &block.term else {
                    return None;
                };
                typed_expr_is_runtime_name_load(
                    call.func.as_ref(),
                    RuntimeName::Next,
                    &typed.module_constants,
                )
                .then_some((
                    call.try_semantic_instr_id()
                        .expect("next() call should have an instruction id"),
                    analysis
                        .block_before_term
                        .get(&block.label)
                        .expect("next() term should have owner state"),
                ))
            })
            .expect("caller should return next(gen)");
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "ClosureGenerator".to_string(),
        };
        let gen_location = state
            .locals
            .iter()
            .find_map(|(location, owner)| (owner == &owner_type_ref).then_some(*location))
            .expect("generator local should be trusted");
        let origin = state
            .object_origins
            .get(&gen_location)
            .copied()
            .expect("trusted generator should retain its origin");
        assert_eq!(
            state
                .function_fields
                .get(&(origin, "_resume_function".to_string())),
            Some(&target_function_id),
        );

        let runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class ClosureGenerator:\n    def __next__(self):\n        return 1\n",
        )
        .expect("runtime source should lower");
        let next_function = runtime_lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("runtime next function should exist");
        let (owners, inline_targets, _, _) = trusted_static_runtime_protocol_inlines_for_function(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &StaticDirectCallTargets {
                strict_methods: HashMap::from([(
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function.clone(),
                )]),
                ..StaticDirectCallTargets::default()
            },
        );
        assert_eq!(owners, HashMap::from([(next_instr_id, owner_type_ref)]));
        assert_eq!(
            inline_targets,
            HashMap::from([(
                next_instr_id,
                vec![(
                    next_function.function_id,
                    TypedDirectCallArgPlan {
                        sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );
    }

    #[test]
    fn repeated_next_protocol_calls_inline_together_for_one_generator_origin() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit
    yield limit + 1

def caller(limit):
    gen = values(limit)
    first = next(gen)
    second = next(gen)
    return first + second
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class ClosureGenerator:\n    def __next__(self):\n        return 1\n",
        )
        .expect("runtime source should lower");
        let next_function = runtime_lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("runtime next function should exist");
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let static_targets = StaticDirectCallTargets {
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([(
                (
                    "soac.runtime".to_string(),
                    "ClosureGenerator".to_string(),
                    "__next__".to_string(),
                ),
                next_function.clone(),
            )]),
            ..StaticDirectCallTargets::default()
        };
        let plans = static_generator_instance_plans_for_function(&caller, &static_targets);
        annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
            .expect("instance-plan annotation should succeed");

        struct NextCallIds<'a> {
            module_constants: &'a [ConstantExpr],
            ids: Vec<InstrId>,
        }

        impl Visit<InstrTyped> for NextCallIds<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr
                    && typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        self.module_constants,
                    )
                    && let Some(instr_id) = call.try_semantic_instr_id()
                {
                    self.ids.push(instr_id);
                }
                expr.visit_children(self);
            }
        }

        let mut next_call_ids = NextCallIds {
            module_constants: &typed.module_constants,
            ids: Vec::new(),
        };
        next_call_ids.visit_fn(&caller);
        next_call_ids.ids.sort();
        assert_eq!(next_call_ids.ids.len(), 2);

        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "ClosureGenerator".to_string(),
        };
        let (owners, inline_targets, _, _) = trusted_static_runtime_protocol_inlines_for_function(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &static_targets,
        );
        assert_eq!(
            owners,
            next_call_ids
                .ids
                .iter()
                .copied()
                .map(|instr_id| (instr_id, owner_type_ref.clone()))
                .collect::<HashMap<_, _>>(),
        );
        assert_eq!(
            inline_targets,
            next_call_ids
                .ids
                .iter()
                .copied()
                .map(|instr_id| {
                    (
                        instr_id,
                        vec![(
                            next_function.function_id,
                            TypedDirectCallArgPlan {
                                sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
                            },
                        )],
                    )
                })
                .collect::<HashMap<_, _>>(),
        );
    }

    #[test]
    fn residual_generator_alias_uses_keep_next_protocol_calls_generic() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def caller(limit):
    gen = values(limit)
    value = next(gen)
    gen.throw(Exception("boom"))
    return value
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class ClosureGenerator:\n    def __next__(self):\n        return 1\n",
        )
        .expect("runtime source should lower");
        let next_function = runtime_lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("runtime next function should exist");
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let static_targets = StaticDirectCallTargets {
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([(
                (
                    "soac.runtime".to_string(),
                    "ClosureGenerator".to_string(),
                    "__next__".to_string(),
                ),
                next_function,
            )]),
            ..StaticDirectCallTargets::default()
        };
        let plans = static_generator_instance_plans_for_function(&caller, &static_targets);
        annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
            .expect("instance-plan annotation should succeed");

        let (_, inline_targets, _, _) = trusted_static_runtime_protocol_inlines_for_function(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &static_targets,
        );
        assert!(
            inline_targets.is_empty(),
            "next(gen) should stay generic when a later generator alias use keeps the wrapper observable"
        );
    }

    #[test]
    fn list_builtin_implementation_plans_follow_proven_generator_instances() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    return value

def values(limit):
    total = limit
    def inner():
        return total
    yield inner()

def caller(limit):
    return list(values(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, helper.clone())]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            ..StaticDirectCallTargets::default()
        };
        let generator_plans = static_generator_instance_plans_for_function(&caller, &targets);
        annotate_typed_generator_instance_plans(&mut caller, Some(&generator_plans))
            .expect("generator instance annotation should succeed");

        let plans = trusted_generator_builtin_implementation_plans_for_function(
            &caller,
            &typed,
            &HashMap::new(),
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &targets,
        );
        let (source, plan) = plans
            .iter()
            .next()
            .expect("list(generator) should get a builtin implementation plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plan.source, RuntimeName::List);
        assert_eq!(plan.function_id, helper.function_id);
        assert_eq!(
            plan.arg_plan,
            TypedDirectCallArgPlan {
                sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
            }
        );
        assert_eq!(
            builtin_implementation_inline_targets(&plans),
            HashMap::from([(
                *source,
                vec![(
                    helper.function_id,
                    TypedDirectCallArgPlan {
                        sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
                    },
                )],
            )]),
        );

        annotate_typed_builtin_implementation_plans(&mut caller, &plans)
            .expect("builtin implementation annotation should succeed");
        assert!(
            caller.blocks.iter().any(|block| {
                matches!(
                    &block.term,
                    BlockTerm::Return(expr)
                        if expr.try_semantic_instr_id() == Some(*source)
                            && expr
                                .builtin_implementation_plan()
                                .is_some_and(|plan| plan.function_id == helper.function_id)
                )
            }),
            "list call should retain the selected helper on InstrTyped"
        );

        let escaped_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    return value

def values(limit):
    yield limit

def sink(value):
    return None

def caller(limit):
    gen = values(limit)
    sink(gen)
    return list(gen)
"#,
        )
        .expect("escaped source should lower")
        .blockpy_module;
        let escaped_module_id = escaped_lowered.module_name_gen.runtime_module_id().as_u32();
        let escaped_helper = escaped_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("escaped helper should exist");
        let escaped_generator_targets =
            strict_module_global_generator_targets_for_module(&escaped_lowered);
        let escaped_typed = lower_blockpy_module_to_typed(escaped_lowered);
        let mut escaped_caller = escaped_typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("escaped typed caller should exist");
        let escaped_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, escaped_helper)]),
            module_global_generators: HashMap::from([(
                escaped_module_id,
                escaped_generator_targets,
            )]),
            ..StaticDirectCallTargets::default()
        };
        let escaped_generator_plans =
            static_generator_instance_plans_for_function(&escaped_caller, &escaped_targets);
        annotate_typed_generator_instance_plans(
            &mut escaped_caller,
            Some(&escaped_generator_plans),
        )
        .expect("escaped generator instance annotation should succeed");
        assert!(
            trusted_generator_builtin_implementation_plans_for_function(
                &escaped_caller,
                &escaped_typed,
                &HashMap::new(),
                &escaped_typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
                &escaped_targets,
            )
            .is_empty(),
            "escaped generator instances must keep the ordinary builtin list path"
        );
    }

    #[test]
    fn list_builtin_implementation_plans_skip_large_generator_resume_bodies() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    return value

def values(limit):
    if limit == 0:
        yield 0
    if limit == 1:
        yield 1
    if limit == 2:
        yield 2
    if limit == 3:
        yield 3
    if limit == 4:
        yield 4
    if limit == 5:
        yield 5
    if limit == 6:
        yield 6
    if limit == 7:
        yield 7
    if limit == 8:
        yield 8
    if limit == 9:
        yield 9
    if limit == 10:
        yield 10
    if limit == 11:
        yield 11
    if limit == 12:
        yield 12
    if limit == 13:
        yield 13
    if limit == 14:
        yield 14
    if limit == 15:
        yield 15
    if limit == 16:
        yield 16
    if limit == 17:
        yield 17
    if limit == 18:
        yield 18
    if limit == 19:
        yield 19
    if limit == 20:
        yield 20
    if limit == 21:
        yield 21
    if limit == 22:
        yield 22
    if limit == 23:
        yield 23
    if limit == 24:
        yield 24
    if limit == 25:
        yield 25

def caller(limit):
    return list(values(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, helper)]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            ..StaticDirectCallTargets::default()
        };
        let generator_plans = static_generator_instance_plans_for_function(&caller, &targets);
        annotate_typed_generator_instance_plans(&mut caller, Some(&generator_plans))
            .expect("generator instance annotation should succeed");

        assert!(
            trusted_generator_builtin_implementation_plans_for_function(
                &caller,
                &typed,
                &HashMap::new(),
                &typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
                &targets,
            )
            .is_empty(),
            "large generator resume bodies should not trigger builtin-consumer inlining",
        );
    }

    #[test]
    fn set_builtin_implementation_plans_follow_proven_generator_instances() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def set_from_iter(value):
    return value

def values(limit):
    yield limit

def caller(limit):
    return set(values(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "set_from_iter")
            .cloned()
            .expect("helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Set, helper.clone())]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            ..StaticDirectCallTargets::default()
        };
        let generator_plans = static_generator_instance_plans_for_function(&caller, &targets);
        annotate_typed_generator_instance_plans(&mut caller, Some(&generator_plans))
            .expect("generator instance annotation should succeed");

        let plans = trusted_generator_builtin_implementation_plans_for_function(
            &caller,
            &typed,
            &HashMap::new(),
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &targets,
        );
        let (source, plan) = plans
            .iter()
            .next()
            .expect("set(generator) should get a builtin implementation plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plan.source, RuntimeName::Set);
        assert_eq!(plan.function_id, helper.function_id);
        assert_eq!(
            plan.arg_plan,
            TypedDirectCallArgPlan {
                sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
            }
        );

        let mut guarded_caller = caller.clone();
        let guarded_call = guarded_caller
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.term {
                BlockTerm::Return(InstrTyped::CallTyped(call)) => Some(call),
                _ => None,
            })
            .expect("caller should return the set call");
        guarded_call.access = soac_ir_typed::TypedCallAccessPlan::GuardedCallable {
            function_guards: Vec::new(),
        };
        lower_typed_function_call_access_plan_instrs(&mut guarded_caller);
        assert_eq!(
            trusted_generator_builtin_implementation_plans_for_function(
                &guarded_caller,
                &typed,
                &HashMap::new(),
                &typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
                &targets,
            )
            .len(),
            1,
            "set(generator) planning should survive an earlier guarded call-emission selection",
        );
        let guarded_plans = trusted_generator_builtin_implementation_plans_for_function(
            &guarded_caller,
            &typed,
            &HashMap::new(),
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &targets,
        );
        annotate_typed_builtin_implementation_plans(&mut guarded_caller, &guarded_plans)
            .expect("guarded builtin implementation annotation should succeed");
        assert!(
            guarded_caller.blocks.iter().any(|block| {
                matches!(
                    &block.term,
                    BlockTerm::Return(InstrTyped::GuardedCallableCallTyped(call))
                        if call.extra.builtin_implementation_plan().is_some_and(|plan| {
                            plan.function_id == helper.function_id
                        })
                )
            }),
            "guarded set(generator) should retain the builtin implementation plan after call-emission lowering",
        );

        annotate_typed_builtin_implementation_plans(&mut caller, &plans)
            .expect("builtin implementation annotation should succeed");
        assert!(
            caller.blocks.iter().any(|block| {
                matches!(
                    &block.term,
                    BlockTerm::Return(expr)
                        if expr.try_semantic_instr_id() == Some(*source)
                            && expr
                                .builtin_implementation_plan()
                                .is_some_and(|plan| plan.function_id == helper.function_id)
                )
            }),
            "set call should retain the selected helper on InstrTyped"
        );
    }

    #[test]
    fn tuple_builtin_implementation_plans_follow_proven_generator_instances() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def tuple_from_iter(value):
    return value

def values(limit):
    yield limit

def caller(limit):
    return tuple(values(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Tuple, helper.clone())]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            ..StaticDirectCallTargets::default()
        };
        let generator_plans = static_generator_instance_plans_for_function(&caller, &targets);
        annotate_typed_generator_instance_plans(&mut caller, Some(&generator_plans))
            .expect("generator instance annotation should succeed");

        let plans = trusted_generator_builtin_implementation_plans_for_function(
            &caller,
            &typed,
            &HashMap::new(),
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &targets,
        );
        let (source, plan) = plans
            .iter()
            .next()
            .expect("tuple(generator) should get a builtin implementation plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plan.source, RuntimeName::Tuple);
        assert_eq!(plan.function_id, helper.function_id);
        assert_eq!(
            plan.arg_plan,
            TypedDirectCallArgPlan {
                sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
            }
        );

        annotate_typed_builtin_implementation_plans(&mut caller, &plans)
            .expect("builtin implementation annotation should succeed");
        assert!(
            caller.blocks.iter().any(|block| {
                matches!(
                    &block.term,
                    BlockTerm::Return(expr)
                        if expr.try_semantic_instr_id() == Some(*source)
                            && expr
                                .builtin_implementation_plan()
                                .is_some_and(|plan| plan.function_id == helper.function_id)
                )
            }),
            "tuple call should retain the selected helper on InstrTyped"
        );
    }

    #[test]
    fn tuple_builtin_implementation_plans_follow_preserved_local_generator_functions() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def tuple_from_iter(value):
    return value

def outer(rows):
    for row in rows:
        yield tuple(item for item in row)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("helper should exist");
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut outer = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "outer")
            .cloned()
            .expect("outer should lower");
        assert!(
            outer.blocks.iter().any(|block| {
                block.body.iter().any(|instr| {
                    let InstrTyped::Store(store) = instr else {
                        return false;
                    };
                    store.name.preserved_location().is_some()
                        && matches!(
                            store.value.as_ref(),
                            InstrTyped::MakeFunctionWithClosure(op)
                                if op.kind == FunctionKind::Generator
                        )
                })
            }),
            "generator lowering should preserve the nested tuple genexpr function object"
        );

        let local_generators = local_generator_targets_for_module(&typed);
        let generator_plans =
            static_local_generator_instance_plans_for_function(&outer, &local_generators);
        annotate_typed_generator_instance_plans(&mut outer, Some(&generator_plans))
            .expect("preserved generator instance annotation should succeed");

        let targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Tuple, helper.clone())]),
            ..StaticDirectCallTargets::default()
        };
        let plans = trusted_generator_builtin_implementation_plans_for_function(
            &outer,
            &typed,
            &HashMap::new(),
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &targets,
        );
        assert!(
            plans.values().any(|plan| {
                plan.source == RuntimeName::Tuple && plan.function_id == helper.function_id
            }),
            "tuple(genexpr) should retain the generator plan after the genexpr function moves through preserved storage"
        );
    }

    #[test]
    fn tuple_builtin_rewrites_consume_preserved_local_generator_functions_in_non_inlined_generators()
     {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def tuple_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return tuple(result)

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def outer(rows):
    for row in rows:
        yield tuple(item for item in row)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("test helper should exist");
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Tuple, helper)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should consume tuple(genexpr) in the generator body");

        let outer = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "outer")
            .expect("typed outer should exist");
        let retained_generator_materializations = outer
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::MakeFunctionWithClosure(op) = store.value.as_ref() else {
                    return None;
                };
                (op.kind == FunctionKind::Generator)
                    .then(|| (store.name.id_str().to_string(), op.function_id()))
            })
            .collect::<Vec<_>>();
        assert!(
            retained_generator_materializations.is_empty(),
            "non-inlined generator resume rewriting should consume nested tuple genexpr materializations: {retained_generator_materializations:?}",
        );
    }

    #[test]
    fn tuple_builtin_rewrites_consume_permutations_style_nested_generators() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def tuple_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return tuple(result)

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def permutations(iterable, r=None):
    pool = tuple(iterable)
    n = len(pool)
    if r is None:
        r = n
    indices = list(range(n))
    cycles = list(range(n - r + 1, n + 1))[::-1]
    yield tuple(pool[i] for i in indices[:r])
    while n:
        for i in reversed(range(r)):
            cycles[i] -= 1
            if cycles[i] == 0:
                indices[i:] = indices[i + 1:] + indices[i:i + 1]
                cycles[i] = n - i
            else:
                j = cycles[i]
                indices[i], indices[-j] = indices[-j], indices[i]
                yield tuple(pool[i] for i in indices[:r])
                break
        else:
            return
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("test helper should exist");
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Tuple, helper)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should consume tuple(genexpr) inside permutations");

        let synthetic_genexpr_function_ids = synthetic_genexpr_function_ids_for_module(&typed);
        let permutations = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "permutations")
            .expect("typed permutations should exist");
        assert_no_dead_synthetic_generator_materializations(
            permutations,
            &typed.module_constants,
            &synthetic_genexpr_function_ids,
            "permutations-style tuple(genexpr) rewrites should not retain dead generator function materializations",
        );
    }

    #[test]
    fn tuple_builtin_rewrites_consume_permutations_generators_under_sum_consumer() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def tuple_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return tuple(result)

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def permutations(iterable, r=None):
    pool = tuple(iterable)
    n = len(pool)
    if r is None:
        r = n
    indices = list(range(n))
    cycles = list(range(n - r + 1, n + 1))[::-1]
    yield tuple(pool[i] for i in indices[:r])
    while n:
        for i in reversed(range(r)):
            cycles[i] -= 1
            if cycles[i] == 0:
                indices[i:] = indices[i + 1:] + indices[i:i + 1]
                cycles[i] = n - i
            else:
                j = cycles[i]
                indices[i], indices[-j] = indices[-j], indices[i]
                yield tuple(pool[i] for i in indices[:r])
                break
        else:
            return

def caller(limit):
    return sum(1 for _ in permutations(range(limit)))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let inline_plan = plan_module_inlining(&summarize_module_escapes(&lowered));
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("test helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Tuple, helper)]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            Some(&inline_plan),
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should consume tuple(genexpr) under the sum consumer");

        let synthetic_genexpr_function_ids = synthetic_genexpr_function_ids_for_module(&typed);
        let permutations = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "permutations")
            .expect("typed permutations should exist");
        assert_no_dead_synthetic_generator_materializations(
            permutations,
            &typed.module_constants,
            &synthetic_genexpr_function_ids,
            "sum(permutations(...)) should not strand dead tuple genexpr function materializations inside permutations",
        );
    }

    #[test]
    fn tuple_builtin_rewrites_consume_cross_module_runtime_builtins_alias_helpers() {
        let mut runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
import builtins as _builtins

iter = _builtins.iter
next = _builtins.next
tuple = _builtins.tuple

def tuple_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return tuple(result)

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)
"#,
        )
        .expect("runtime source should lower")
        .blockpy_module;
        soac_driver::blockpy_cache::remap_blockpy_module_function_ids(
            &mut runtime_lowered,
            soac_core::block_py::ModuleNameGen::new(1),
        );
        let runtime_inline_plan = plan_module_inlining(&summarize_module_escapes(&runtime_lowered));
        let user_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def outer(values):
    yield tuple(value for value in values)

def caller(values):
    return tuple(outer(values))
"#,
        )
        .expect("user source should lower")
        .blockpy_module;
        let runtime_module_id = runtime_lowered.module_name_gen.runtime_module_id().as_u32();
        let user_module_id = user_lowered.module_name_gen.runtime_module_id().as_u32();
        let runtime_helper = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("runtime helper should exist");
        let iter_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("runtime iter method should exist");
        let next_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("runtime next method should exist");
        let send_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("runtime send method should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&user_lowered);
        let mut runtime_typed = lower_blockpy_module_to_typed(runtime_lowered.clone());
        let mut typed = lower_blockpy_module_to_typed(user_lowered);
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Tuple, runtime_helper)]),
            module_globals: HashMap::from([(
                runtime_module_id,
                strict_module_global_direct_call_targets_for_module(
                    &runtime_lowered,
                    "soac.runtime",
                ),
            )]),
            module_global_generators: HashMap::from([(user_module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        let runtime_static_direct_calls =
            static_direct_calls_for_module(&runtime_typed, &static_targets);
        for function in &mut runtime_typed.callable_defs {
            apply_call_emission_plans_to_typed_function(
                function,
                &profile,
                runtime_static_direct_calls.get(&function.function_id),
            )
            .expect("runtime call emission plans should lower");
        }
        let runtime_functions = runtime_typed
            .callable_defs
            .iter()
            .map(|function| (function.names.qualname.clone(), function.clone()))
            .collect::<HashMap<_, _>>();
        let runtime_module_constants = runtime_typed.module_constants.clone();
        let external_callees = HashMap::from([
            (
                runtime_functions
                    .get("tuple_from_iter")
                    .expect("typed runtime helper should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("tuple_from_iter")
                        .cloned()
                        .expect("typed runtime helper should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.__iter__")
                    .expect("typed runtime iter method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.__iter__")
                        .cloned()
                        .expect("typed runtime iter method should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.__next__")
                    .expect("typed runtime next method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.__next__")
                        .cloned()
                        .expect("typed runtime next method should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.send")
                    .expect("typed runtime send method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.send")
                        .cloned()
                        .expect("typed runtime send method should exist"),
                    module_constants: runtime_module_constants,
                    inline_plan: Some(runtime_inline_plan),
                },
            ),
        ]);
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &external_callees,
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the cross-module tuple consumer path");

        let outer = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "outer")
            .expect("typed outer should exist");
        let retained_generator_materializations = outer
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::MakeFunctionWithClosure(op) = store.value.as_ref() else {
                    return None;
                };
                (op.kind == FunctionKind::Generator)
                    .then(|| (store.name.id_str().to_string(), op.function_id()))
            })
            .collect::<Vec<_>>();
        assert!(
            retained_generator_materializations.is_empty(),
            "cross-module runtime helpers that alias builtins should still consume nested tuple genexpr materializations: {retained_generator_materializations:?}",
        );
    }

    #[test]
    fn set_builtin_implementation_plans_follow_preserved_local_generator_functions() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def set_from_iter(value):
    return value

def outer(rows):
    for row in rows:
        yield set(item for item in row)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "set_from_iter")
            .cloned()
            .expect("helper should exist");
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut outer = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "outer")
            .cloned()
            .expect("outer should lower");
        assert!(
            outer.blocks.iter().any(|block| {
                block.body.iter().any(|instr| {
                    let InstrTyped::Store(store) = instr else {
                        return false;
                    };
                    store.name.preserved_location().is_some()
                        && matches!(
                            store.value.as_ref(),
                            InstrTyped::MakeFunctionWithClosure(op)
                                if op.kind == FunctionKind::Generator
                        )
                })
            }),
            "generator lowering should preserve the nested genexpr function object"
        );

        let local_generators = local_generator_targets_for_module(&typed);
        let generator_plans =
            static_local_generator_instance_plans_for_function(&outer, &local_generators);
        annotate_typed_generator_instance_plans(&mut outer, Some(&generator_plans))
            .expect("preserved generator instance annotation should succeed");

        let targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Set, helper.clone())]),
            ..StaticDirectCallTargets::default()
        };
        let plans = trusted_generator_builtin_implementation_plans_for_function(
            &outer,
            &typed,
            &HashMap::new(),
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &targets,
        );
        assert!(
            plans.values().any(|plan| {
                plan.source == RuntimeName::Set && plan.function_id == helper.function_id
            }),
            "set(genexpr) should retain the generator plan after the genexpr function moves through preserved storage"
        );
    }

    #[test]
    fn trusted_generator_resume_candidate_worklist_deduplicates_reported_candidates() {
        let candidate = TrustedGeneratorResumeCandidateId {
            function_id: RuntimeFunctionId::new(RuntimeModuleId::new(1), LocalFunctionId::new(2)),
            instr_id: InstrId::new(3),
        };
        let mut report = TrustedGeneratorResumeDecisionReport::default();
        for outcome in [
            TrustedGeneratorResumeDecisionOutcome::Selected,
            TrustedGeneratorResumeDecisionOutcome::MissingOwnerState,
        ] {
            report.push(TrustedGeneratorResumeDecision {
                candidate: Some(candidate),
                instr_id: Some(candidate.instr_id),
                block: BlockLabel::fallthrough(),
                instr_index: None,
                phase: TrustedGeneratorResumeDecisionPhase::PostNormalizationRefresh,
                reachable: true,
                outcome,
            });
        }

        let mut worklist = TrustedGeneratorResumeCandidateWorklist::from_report(&report);
        let retained_plans = worklist.retain_discovered_plans(&HashMap::new());
        assert!(retained_plans.is_empty());
        assert_eq!(worklist.queued_candidate_count(), 1);
        assert_eq!(worklist.duplicate_candidate_enqueues, 1);
        assert_eq!(worklist.processed_candidate_count, 1);
    }

    #[test]
    fn late_trusted_owner_state_cache_reuses_until_invalidated() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def caller(value):
    return value
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let typed = lower_blockpy_module_to_typed(lowered);
        let function = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let mut cache = TrustedOwnerStateCache::default();

        let first = cache.states(
            function,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        ) as *const TrustedOwnerStateAnalysis;
        let second = cache.states(
            function,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        ) as *const TrustedOwnerStateAnalysis;

        assert_eq!(
            first, second,
            "unchanged functions should reuse the cached analysis"
        );
        assert_eq!(cache.builds, 1);
        assert_eq!(cache.reuses, 1);
        assert_eq!(cache.invalidations, 0);

        cache.invalidate();
        let _ = cache.states(
            function,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );

        assert_eq!(cache.builds, 2);
        assert_eq!(cache.reuses, 1);
        assert_eq!(cache.invalidations, 1);
    }

    #[test]
    fn trusted_generator_resume_plans_follow_unescaped_generator_instances() {
        fn typed_caller_with_resume_call(
            source: &str,
        ) -> (
            BlockPyModule<TypedBlockPyModuleShape>,
            BlockPyFunction<TypedBlockPyModuleShape>,
            RuntimeFunctionId,
        ) {
            let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
                .expect("source should lower")
                .blockpy_module;
            let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
            let target_function_id = lowered
                .callable_defs
                .iter()
                .find(|function| function.names.qualname == "values")
                .expect("generator target should exist")
                .function_id;
            let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
            let typed = lower_blockpy_module_to_typed(lowered);
            let mut caller = typed
                .callable_defs
                .iter()
                .find(|function| function.names.qualname == "caller")
                .cloned()
                .expect("typed caller should exist");
            let plans = static_generator_instance_plans_for_function(
                &caller,
                &StaticDirectCallTargets {
                    module_global_generators: HashMap::from([(module_id, generator_targets)]),
                    ..StaticDirectCallTargets::default()
                },
            );
            annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
                .expect("instance-plan annotation should succeed");
            let call = caller
                .blocks
                .iter_mut()
                .find_map(|block| match &mut block.term {
                    BlockTerm::Return(InstrTyped::CallTyped(call)) => Some(call),
                    _ => None,
                })
                .expect("caller should return the helper call");
            call.func = Box::new(Load::new(ResolvedName::runtime_name("resume_generator")).into());
            (typed, caller, target_function_id)
        }

        let (typed, mut caller, target_function_id) = typed_caller_with_resume_call(
            r#"
def values(limit):
    yield limit

def helper(fn, owner, state, value, exc):
    return value

def caller(limit):
    gen = values(limit)
    return helper(gen._resume_function, gen, gen._preserved_values, None, None)
"#,
        );
        let plans = trusted_generator_resume_plans_for_function(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        let (_, selected_report) = trusted_generator_resume_plans_and_report_for_function(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        let (source, plan) = plans
            .iter()
            .next()
            .expect("unescaped generator resume should be planned");
        assert_eq!(plans.len(), 1);
        assert_eq!(plan.function_id, target_function_id);
        assert!(
            selected_report.has_outcome(|outcome| matches!(
                outcome,
                TrustedGeneratorResumeDecisionOutcome::Selected
            )),
            "trusted resume planning should report selected candidates structurally",
        );
        assert_eq!(
            selected_report.discovered_candidates.len(),
            1,
            "trusted resume planning should track one stable candidate identity for the positive case",
        );
        assert_eq!(
            selected_report.duplicate_candidate_discoveries, 0,
            "trusted resume planning should not rediscover the same positive candidate in one planning pass",
        );
        let late_refresh_schedule = LateTypedRefreshSchedule::from_rewritten_stop_iteration(1);
        assert!(
            late_refresh_schedule.requests(LateTypedRefreshFamily::TrustedGeneratorResume),
            "rewriting StopIteration should explicitly request trusted-generator-resume refresh",
        );
        let mut consumed_late_refresh_schedule = late_refresh_schedule.clone();
        assert!(
            consumed_late_refresh_schedule.consume(LateTypedRefreshFamily::TrustedGeneratorResume),
            "the late refresh ticket should be consumable once",
        );
        assert!(
            !consumed_late_refresh_schedule
                .requests(LateTypedRefreshFamily::TrustedGeneratorResume),
            "consuming the late refresh ticket should prevent repeated scans until new normalization happens",
        );
        let idle_late_refresh_schedule = LateTypedRefreshSchedule::from_rewritten_stop_iteration(0);
        assert!(
            !idle_late_refresh_schedule.requests(LateTypedRefreshFamily::TrustedGeneratorResume),
            "a no-op StopIteration normalization must not advertise a post-normalization resume refresh",
        );
        let mut reopened_late_refresh_schedule = idle_late_refresh_schedule.clone();
        reopened_late_refresh_schedule.record_rewritten_stop_iteration(1);
        assert!(
            reopened_late_refresh_schedule.requests(LateTypedRefreshFamily::TrustedGeneratorResume),
            "a later StopIteration rewrite inside the late fixpoint must reopen trusted-generator-resume refresh",
        );
        let (idle_refreshed_plans, idle_refreshed_report) =
            refresh_typed_generator_resume_candidates_after_late_normalization(
                &caller,
                &typed.module_constants,
                &idle_late_refresh_schedule,
                &HashMap::new(),
                &HashMap::new(),
            );
        assert!(
            idle_refreshed_plans.is_empty() && idle_refreshed_report.decisions.is_empty(),
            "a no-op StopIteration normalization must not rescan trusted-generator-resume candidates",
        );
        let (_, refreshed_report) =
            refresh_typed_generator_resume_candidates_after_late_normalization(
                &caller,
                &typed.module_constants,
                &late_refresh_schedule,
                &HashMap::new(),
                &HashMap::new(),
            );
        assert!(
            refreshed_report.decisions.iter().any(|decision| {
                decision.phase == TrustedGeneratorResumeDecisionPhase::PostNormalizationRefresh
                    && matches!(
                        decision.outcome,
                        TrustedGeneratorResumeDecisionOutcome::Selected
                    )
            }),
            "late refresh reporting should distinguish post-normalization resume candidates from initial planning",
        );
        annotate_typed_generator_resume_plans(&mut caller, &plans)
            .expect("resume-plan annotation should succeed");
        let annotated_plan = caller
            .blocks
            .iter()
            .find_map(|block| match &block.term {
                BlockTerm::Return(expr) if expr.try_semantic_instr_id() == Some(*source) => {
                    expr.generator_resume_plan()
                }
                _ => None,
            })
            .expect("resume call should carry typed metadata");
        assert_eq!(annotated_plan, *plan);

        let (direct_typed, mut direct_caller, direct_target_function_id) =
            typed_caller_with_resume_call(
                r#"
def values(limit):
    yield limit

def helper(fn, owner, state, value, exc):
    return value

def caller(limit):
    gen = values(limit)
    return helper(gen._resume_function, gen, gen._preserved_values, None, None)
"#,
            );
        let direct_expr = direct_caller
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.term {
                BlockTerm::Return(expr @ InstrTyped::CallTyped(_)) => Some(expr),
                _ => None,
            })
            .expect("caller should return the helper call");
        let InstrTyped::CallTyped(call) =
            std::mem::replace(direct_expr, InstrTyped::constant_none())
        else {
            unreachable!("resume test should replace a typed call");
        };
        *direct_expr = InstrTyped::DirectCallableCallTyped(
            soac_ir_typed::TypedDirectCallableCall::from_typed_call(
                call,
                soac_ir_typed::TypedDirectCallableCallGuard::Function(
                    TypedDirectFunctionCallGuard {
                        function_id: direct_target_function_id,
                        arg_plan: TypedDirectCallArgPlan {
                            sources: Vec::new(),
                        },
                    },
                ),
            ),
        );
        let direct_plans = trusted_generator_resume_plans_for_function(
            &direct_caller,
            &direct_typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            direct_plans.len(),
            1,
            "direct callable resume helpers should stay eligible for generator-state lowering"
        );

        let (guarded_typed, mut guarded_caller, _) = typed_caller_with_resume_call(
            r#"
def values(limit):
    yield limit

def helper(fn, owner, state, value, exc):
    return value

def caller(limit):
    gen = values(limit)
    return helper(gen._resume_function, gen, gen._preserved_values, None, None)
"#,
        );
        let guarded_expr = guarded_caller
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.term {
                BlockTerm::Return(expr @ InstrTyped::CallTyped(_)) => Some(expr),
                _ => None,
            })
            .expect("caller should return the helper call");
        let InstrTyped::CallTyped(call) =
            std::mem::replace(guarded_expr, InstrTyped::constant_none())
        else {
            unreachable!("resume test should replace a typed call");
        };
        *guarded_expr = InstrTyped::GuardedCallableCallTyped(
            soac_ir_typed::TypedGuardedCallableCall::from_typed_call(call, Vec::new()),
        );
        let guarded_plans = trusted_generator_resume_plans_for_function(
            &guarded_caller,
            &guarded_typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            guarded_plans.len(),
            1,
            "guarded callable resume helpers should stay eligible for generator-state lowering"
        );

        let (temp_typed, mut temp_caller, temp_target_function_id) = typed_caller_with_resume_call(
            r#"
def values(limit):
    yield limit

def helper(fn, owner, state, value, exc):
    return value

def caller(limit):
    gen = values(limit)
    resume = gen._resume_function
    return helper(resume, gen, gen._preserved_values, None, None)
"#,
        );
        let temp_call = temp_caller
            .blocks
            .iter_mut()
            .find_map(|block| match &mut block.term {
                BlockTerm::Return(InstrTyped::CallTyped(call)) => Some(call),
                _ => None,
            })
            .expect("caller should return the helper call");
        temp_call.func = Box::new(Load::new(ResolvedName::runtime_name("resume_generator")).into());
        let temp_plans = trusted_generator_resume_plans_for_function(
            &temp_caller,
            &temp_typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(temp_plans.len(), 1);
        assert_eq!(
            temp_plans.values().next().map(|plan| plan.function_id),
            Some(temp_target_function_id),
            "resume-function temps copied from trusted generator owners should remain inlineable",
        );

        let (state_reader_typed, mut state_reader_caller, _) = typed_caller_with_resume_call(
            r#"
def values(limit):
    yield limit

def _is_generator_closed(owner):
    return False

def helper(fn, owner, state, value, exc):
    return value

def caller(limit):
    gen = values(limit)
    _is_generator_closed(gen)
    return helper(gen._resume_function, gen, gen._preserved_values, None, None)
"#,
        );
        struct RuntimeGeneratorStateReaderMarker;
        impl VisitMut<InstrTyped> for RuntimeGeneratorStateReaderMarker {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr
                    && matches!(
                        call.func.as_ref(),
                        InstrTyped::Load(load) if load.name.id_str() == "_is_generator_closed"
                    )
                {
                    call.func = Box::new(
                        Load::new(ResolvedName::runtime_name("_is_generator_closed")).into(),
                    );
                    return;
                }
                expr.visit_children_mut(self);
            }
        }
        RuntimeGeneratorStateReaderMarker.visit_fn_mut(&mut state_reader_caller);
        let state_reader_states = analyze_trusted_owner_states(
            &state_reader_caller,
            &state_reader_typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        let state_reader_return_state = state_reader_caller
            .blocks
            .iter()
            .find_map(|block| match &block.term {
                BlockTerm::Return(_) => state_reader_states.block_before_term.get(&block.label),
                _ => None,
            })
            .expect("caller return should have trusted-owner state");
        assert!(
            state_reader_return_state.escaped_origins.is_empty(),
            "internal generator state reads and plain aliases must not mark trusted owners escaped"
        );
        let (escaped_typed, escaped_caller, _) = typed_caller_with_resume_call(
            r#"
def values(limit):
    yield limit

def helper(fn, owner, state, value, exc):
    return value

def sink(value):
    return None

def caller(limit):
    gen = values(limit)
    sink(gen)
    return helper(gen._resume_function, gen, gen._preserved_values, None, None)
"#,
        );
        let (escaped_plans, escaped_report) =
            trusted_generator_resume_plans_and_report_for_function(
                &escaped_caller,
                &escaped_typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
            );
        assert!(escaped_plans.is_empty());
        assert!(
            escaped_report.has_outcome(|outcome| matches!(
                outcome,
                TrustedGeneratorResumeDecisionOutcome::Escaped { .. }
            )),
            "trusted resume planning should report escape-driven rejection structurally",
        );

        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def helper(fn, owner, state, value, exc):
    return value

def caller(limit):
    gen = values(limit)
    value = helper(gen._resume_function, gen, gen._preserved_values, None, None)
    gen.throw(Exception("boom"))
    return value
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut residual_caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let plans = static_generator_instance_plans_for_function(
            &residual_caller,
            &StaticDirectCallTargets {
                module_global_generators: HashMap::from([(module_id, generator_targets)]),
                ..StaticDirectCallTargets::default()
            },
        );
        annotate_typed_generator_instance_plans(&mut residual_caller, Some(&plans))
            .expect("instance-plan annotation should succeed");
        let call = residual_caller
            .blocks
            .iter_mut()
            .flat_map(|block| block.body.iter_mut())
            .find_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                    return None;
                };
                (call.args.len() == 5).then_some(call)
            })
            .expect("caller should store the helper resume call");
        call.func = Box::new(Load::new(ResolvedName::runtime_name("resume_generator")).into());
        assert!(
            trusted_generator_resume_plans_for_function(
                &residual_caller,
                &typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
            )
            .is_empty(),
            "later generator alias uses must keep resume execution on the non-inlined state path"
        );
    }

    #[test]
    fn generator_state_lowering_plans_follow_inlined_resume_body_instrs() {
        let generator_origin = InstrId::new(3);
        let resume_source = InstrId::new(4);
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(1), LocalFunctionId::new(2));
        let mut instr_ids_by_origin = HashMap::new();
        collect_generator_state_lowering_instr_ids(
            &HashMap::from([(
                resume_source,
                TypedGeneratorResumePlan {
                    function_id,
                    generator_origin: Some(generator_origin),
                    candidate_origins: vec![generator_origin],
                },
            )]),
            &soac_opt::passes::TypedInlineRewriteStats {
                inline_instance_sources: vec![soac_opt::passes::TypedInlineInstanceSource {
                    inline_instance: 7,
                    source_instr_id: resume_source,
                }],
                instr_id_mappings: vec![
                    soac_opt::passes::TypedInlineInstrIdMapping {
                        callee: function_id,
                        inline_instance: 7,
                        callee_instr_id: InstrId::new(8),
                        caller_instr_id: InstrId::new(9),
                    },
                    soac_opt::passes::TypedInlineInstrIdMapping {
                        callee: function_id,
                        inline_instance: 6,
                        callee_instr_id: InstrId::new(10),
                        caller_instr_id: InstrId::new(11),
                    },
                ],
                ..soac_opt::passes::TypedInlineRewriteStats::default()
            },
            &mut instr_ids_by_origin,
        );

        let plans = typed_generator_state_lowering_plans(
            instr_ids_by_origin,
            &HashMap::new(),
            &HashMap::new(),
            None,
        );
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].generator_origin, generator_origin);
        assert_eq!(plans[0].function_id, function_id);
        assert_eq!(plans[0].body_instr_ids, HashSet::from([InstrId::new(9)]));
        assert!(plans[0].materialized_constructor.is_none());
    }

    #[test]
    fn generator_state_lowering_rejects_ambiguous_inlined_resume_origins() {
        let first_origin = InstrId::new(3);
        let second_origin = InstrId::new(4);
        let resume_source = InstrId::new(5);
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(1), LocalFunctionId::new(2));
        let plan = TypedGeneratorResumePlan {
            function_id,
            generator_origin: None,
            candidate_origins: vec![first_origin, second_origin],
        };
        let mut instr_ids_by_origin = HashMap::new();

        assert_eq!(typed_generator_resume_plan_state_origin(&plan), None);
        assert_eq!(typed_generator_resume_plan_state_origins(&plan).count(), 0);

        collect_generator_state_lowering_instr_ids(
            &HashMap::from([(resume_source, plan)]),
            &soac_opt::passes::TypedInlineRewriteStats {
                inline_instance_sources: vec![soac_opt::passes::TypedInlineInstanceSource {
                    inline_instance: 7,
                    source_instr_id: resume_source,
                }],
                instr_id_mappings: vec![soac_opt::passes::TypedInlineInstrIdMapping {
                    callee: function_id,
                    inline_instance: 7,
                    callee_instr_id: InstrId::new(8),
                    caller_instr_id: InstrId::new(9),
                }],
                ..soac_opt::passes::TypedInlineRewriteStats::default()
            },
            &mut instr_ids_by_origin,
        );

        assert!(
            instr_ids_by_origin.is_empty(),
            "one inlined resume instruction must never be assigned to multiple generator owners",
        );
    }

    #[test]
    fn generator_state_lowering_accepts_one_proven_candidate_origin() {
        let generator_origin = InstrId::new(3);
        let resume_source = InstrId::new(4);
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(1), LocalFunctionId::new(2));
        let plan = TypedGeneratorResumePlan {
            function_id,
            generator_origin: None,
            candidate_origins: vec![generator_origin],
        };
        let mut instr_ids_by_origin = HashMap::new();

        assert_eq!(
            typed_generator_resume_plan_state_origin(&plan),
            Some(generator_origin),
        );

        collect_generator_state_lowering_instr_ids(
            &HashMap::from([(resume_source, plan)]),
            &soac_opt::passes::TypedInlineRewriteStats {
                inline_instance_sources: vec![soac_opt::passes::TypedInlineInstanceSource {
                    inline_instance: 7,
                    source_instr_id: resume_source,
                }],
                instr_id_mappings: vec![soac_opt::passes::TypedInlineInstrIdMapping {
                    callee: function_id,
                    inline_instance: 7,
                    callee_instr_id: InstrId::new(8),
                    caller_instr_id: InstrId::new(9),
                }],
                ..soac_opt::passes::TypedInlineRewriteStats::default()
            },
            &mut instr_ids_by_origin,
        );

        assert_eq!(
            instr_ids_by_origin,
            HashMap::from([(
                generator_origin,
                (function_id, HashSet::from([InstrId::new(9)])),
            )]),
        );
    }

    #[test]
    fn ordinary_function_rejects_foreign_generator_preserved_storage() {
        let lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def caller():\n    return 1\n")
                .expect("source should lower")
                .blockpy_module;
        let mut typed = lower_blockpy_module_to_typed(lowered);
        let caller = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");

        assert!(validate_typed_preserved_storage_for_function(caller).is_ok());

        caller.blocks[0]
            .body
            .push(InstrTyped::Load(Load::new(ResolvedName {
                id: "foreign_generator_state".into(),
                location: NameLocation::Preserved(PreservedLocation(0)),
            })));

        let error = validate_typed_preserved_storage_for_function(caller)
            .expect_err("ordinary callers must reject foreign preserved generator state");
        assert!(error.contains("foreign generator preserved storage"));
    }

    #[test]
    fn normal_next_calls_inline_generator_resume_state_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

class ClosureGenerator:
    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def caller(limit):
    gen = values(limit)
    return next(gen)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_names: HashMap::new(),
            runtime_builtin_implementations: HashMap::new(),
            module_globals: HashMap::new(),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            suppressed_source_generators: HashSet::new(),
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the normal next() path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| !matches!(
                    instr,
                    InstrTyped::Store(store)
                        if matches!(
                            store.value.as_ref(),
                            InstrTyped::CallTyped(call)
                                if call.extra.generator_instance_plan().is_some()
                        )
                )),
            "normal next() inlining should remove the generator construction"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| {
                    struct PreservedFinder(bool);
                    impl Visit<InstrTyped> for PreservedFinder {
                        fn visit_instr(&mut self, expr: &InstrTyped) {
                            if let InstrTyped::Load(load) = expr
                                && load.name.preserved_location().is_some()
                            {
                                self.0 = true;
                                return;
                            }
                            expr.visit_children(self);
                        }
                    }
                    let mut finder = PreservedFinder(false);
                    finder.visit_instr(instr);
                    !finder.0
                }),
            "normal next() inlining should remap preserved state to caller locals"
        );
    }

    #[test]
    fn local_generator_next_calls_inline_resume_state_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class ClosureGenerator:
    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def caller(limit):
    def values(limit):
        yield limit
    gen = values(limit)
    return next(gen)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the local next() path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| !matches!(
                    instr,
                    InstrTyped::Store(store)
                        if matches!(
                            store.value.as_ref(),
                            InstrTyped::CallTyped(call)
                                if call.extra.generator_instance_plan().is_some()
                        )
                )),
            "local generator next() inlining should remove the generator construction"
        );
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| {
                    struct PreservedFinder(bool);
                    impl Visit<InstrTyped> for PreservedFinder {
                        fn visit_instr(&mut self, expr: &InstrTyped) {
                            if let InstrTyped::Load(load) = expr
                                && load.name.preserved_location().is_some()
                            {
                                self.0 = true;
                                return;
                            }
                            expr.visit_children(self);
                        }
                    }
                    let mut finder = PreservedFinder(false);
                    finder.visit_instr(instr);
                    !finder.0
                }),
            "local generator next() inlining should remap preserved state to caller locals"
        );
    }

    #[test]
    fn named_generator_for_loops_inline_runtime_protocol_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def values(limit):
    yield limit

def caller(limit):
    total = 0
    for value in values(limit):
        total = value
    return total
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the named generator for-loop path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        struct RuntimeCallCounter<'a> {
            module_constants: &'a [ConstantExpr],
            iter_calls: usize,
            next_calls: usize,
            resume_calls: usize,
        }
        impl Visit<InstrTyped> for RuntimeCallCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr {
                    self.iter_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Iter,
                        self.module_constants,
                    ));
                    self.next_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        self.module_constants,
                    ));
                    self.resume_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ResumeGenerator,
                        self.module_constants,
                    ));
                }
                expr.visit_children(self);
            }
        }
        let mut runtime_calls = RuntimeCallCounter {
            module_constants: &typed.module_constants,
            iter_calls: 0,
            next_calls: 0,
            resume_calls: 0,
        };
        runtime_calls.visit_fn(caller);
        assert_eq!(
            (
                runtime_calls.iter_calls,
                runtime_calls.next_calls,
                runtime_calls.resume_calls,
            ),
            (0, 0, 0),
            "named generator for-loop inlining should consume the runtime protocol path",
        );
    }

    #[test]
    fn list_generator_calls_inline_resume_state_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return result

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def caller(limit):
    def values(limit):
        yield limit
    return list(values(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("test helper should exist");
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, helper)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the list(generator) path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        struct RuntimeCallCounter<'a> {
            module_constants: &'a [ConstantExpr],
            iter_calls: usize,
            next_calls: usize,
            resume_calls: usize,
        }
        impl Visit<InstrTyped> for RuntimeCallCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr {
                    self.iter_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Iter,
                        self.module_constants,
                    ));
                    self.next_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        self.module_constants,
                    ));
                    self.resume_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ResumeGenerator,
                        self.module_constants,
                    ));
                }
                expr.visit_children(self);
            }
        }
        let mut runtime_calls = RuntimeCallCounter {
            module_constants: &typed.module_constants,
            iter_calls: 0,
            next_calls: 0,
            resume_calls: 0,
        };
        runtime_calls.visit_fn(caller);
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| !matches!(
                    instr,
                    InstrTyped::Store(store)
                        if matches!(
                            store.value.as_ref(),
                            InstrTyped::CallTyped(call)
                                if call.extra.generator_instance_plan().is_some()
                        )
                )),
            "list(generator) inlining should remove the generator construction; remaining iter={} next={} resume={}",
            runtime_calls.iter_calls,
            runtime_calls.next_calls,
            runtime_calls.resume_calls,
        );
        assert_eq!(
            (
                runtime_calls.iter_calls,
                runtime_calls.next_calls,
                runtime_calls.resume_calls,
            ),
            (0, 0, 0),
            "list(generator) inlining should consume the runtime generator protocol path",
        );
    }

    #[test]
    fn diagonal_set_genexpr_consumers_drop_materializations_after_state_lowering() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def set_from_iter(value):
    result = set()
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.add(item)
        except StopIteration:
            return result

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def caller(queen_count):
    cols = range(queen_count)
    vec = tuple(range(queen_count))
    total = 0
    total += len(set(vec[i] + i for i in cols))
    total += len(set(vec[i] - i for i in cols))
    return total
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "set_from_iter")
            .cloned()
            .expect("test helper should exist");
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Set, helper)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the diagonal set(genexpr) path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let retained_generator_instances = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let generator_plan = match store.value.as_ref() {
                    InstrTyped::CallTyped(call) => call.extra.generator_instance_plan(),
                    InstrTyped::GuardedCallableCallTyped(call) => {
                        call.extra.generator_instance_plan()
                    }
                    InstrTyped::DirectCallableCallTyped(call) => {
                        call.extra.generator_instance_plan()
                    }
                    _ => None,
                }?;
                Some((store.name.id_str().to_string(), generator_plan.function_id))
            })
            .collect::<Vec<_>>();
        assert!(
            retained_generator_instances.is_empty(),
            "diagonal set(genexpr) inlining should remove generator instance constructions: {retained_generator_instances:?}",
        );
        let retained_generator_materializations = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::MakeFunctionWithClosure(op) = store.value.as_ref() else {
                    return None;
                };
                (op.kind == FunctionKind::Generator)
                    .then(|| (store.name.id_str().to_string(), op.function_id()))
            })
            .collect::<Vec<_>>();
        assert!(
            retained_generator_materializations.is_empty(),
            "diagonal set(genexpr) inlining should not retain dead generator function materializations: {retained_generator_materializations:?}",
        );
    }

    #[test]
    fn tuple_generator_calls_inline_resume_state_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def tuple_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return tuple(result)

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def caller(limit):
    def values(limit):
        yield limit
    return tuple(values(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("test helper should exist");
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::Tuple, helper)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the tuple(generator) path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| !matches!(
                    instr,
                    InstrTyped::Store(store)
                        if matches!(
                            store.value.as_ref(),
                            InstrTyped::CallTyped(call)
                                if call.extra.generator_instance_plan().is_some()
                        )
                )),
            "tuple(generator) inlining should remove the generator construction",
        );
    }

    #[test]
    fn list_named_generator_calls_inline_resume_state_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return result

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def values(limit):
    total = limit
    def inner():
        return total
    yield inner()

def caller(limit):
    return list(values(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("test helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, helper)]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the named list(generator) path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        assert!(
            caller
                .blocks
                .iter()
                .flat_map(|block| block.body.iter())
                .all(|instr| !matches!(
                    instr,
                    InstrTyped::Store(store)
                        if matches!(
                            store.value.as_ref(),
                            InstrTyped::CallTyped(call)
                                if call.extra.generator_instance_plan().is_some()
                        )
                )),
            "named list(generator) inlining should remove the generator construction",
        );
    }

    #[test]
    fn list_generator_preserved_generator_state_keeps_protocol_owner_facts() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return result

NO_DEFAULT = object()

def _is_generator_closed(owner):
    return bool(load_preserved_state(owner._preserved_values, owner._closed_slot))

def _reraise_control_flow(exc):
    raise exc

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        if _is_generator_closed(self):
            raise StopIteration
        try:
            return resume_generator(self._resume_function, self, self._preserved_values, value, NO_DEFAULT)
        except BaseException as exc:
            _reraise_control_flow(exc)

def inner(limit):
    yield limit
    yield limit + 1

def outer(limit):
    gen = inner(limit)
    yield next(gen)
    yield next(gen)

def caller(limit):
    return list(outer(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("test helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered.clone());
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, helper)]),
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "soac.runtime"),
            )]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline preserved generator state consumers");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        struct RuntimeCallCounter<'a> {
            module_constants: &'a [ConstantExpr],
            iter_calls: usize,
            next_calls: usize,
            resume_calls: usize,
        }
        impl Visit<InstrTyped> for RuntimeCallCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr {
                    self.iter_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Iter,
                        self.module_constants,
                    ));
                    self.next_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        self.module_constants,
                    ));
                    self.resume_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ResumeGenerator,
                        self.module_constants,
                    ));
                }
                expr.visit_children(self);
            }
        }
        let mut runtime_calls = RuntimeCallCounter {
            module_constants: &typed.module_constants,
            iter_calls: 0,
            next_calls: 0,
            resume_calls: 0,
        };
        runtime_calls.visit_fn(caller);
        assert_eq!(
            (
                runtime_calls.iter_calls,
                runtime_calls.next_calls,
                runtime_calls.resume_calls,
            ),
            (0, 0, 0),
            "preserved generator locals should keep enough owner/origin state to inline the nested runtime protocol path",
        );
    }

    #[test]
    fn list_generator_preserved_generator_state_keeps_protocol_owner_facts_through_send_wrapper() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return result

NO_DEFAULT = object()

def _is_generator_closed(owner):
    return bool(load_preserved_state(owner._preserved_values, owner._closed_slot))

def _reraise_control_flow(exc):
    raise exc

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        if _is_generator_closed(self):
            raise StopIteration
        try:
            return resume_generator(self._resume_function, self, self._preserved_values, value, NO_DEFAULT)
        except BaseException as exc:
            _reraise_control_flow(exc)

def inner(limit):
    yield limit
    yield limit + 1

def outer(limit):
    gen = inner(limit)
    yield next(gen)
    yield next(gen)

def caller(limit):
    return list(outer(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("test helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered.clone());
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, helper)]),
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "soac.runtime"),
            )]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the send-wrapper generator path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        struct RuntimeCallCounter<'a> {
            module_constants: &'a [ConstantExpr],
            iter_calls: usize,
            next_calls: usize,
            resume_calls: usize,
        }
        impl Visit<InstrTyped> for RuntimeCallCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr {
                    self.iter_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Iter,
                        self.module_constants,
                    ));
                    self.next_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        self.module_constants,
                    ));
                    self.resume_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ResumeGenerator,
                        self.module_constants,
                    ));
                }
                expr.visit_children(self);
            }
        }
        let mut runtime_calls = RuntimeCallCounter {
            module_constants: &typed.module_constants,
            iter_calls: 0,
            next_calls: 0,
            resume_calls: 0,
        };
        runtime_calls.visit_fn(caller);
        assert_eq!(
            (
                runtime_calls.iter_calls,
                runtime_calls.next_calls,
                runtime_calls.resume_calls,
            ),
            (0, 0, 0),
            "send-wrapper generator paths should preserve enough owner/origin state to inline the nested runtime protocol path",
        );
    }

    #[test]
    fn list_generator_preserved_generator_state_keeps_protocol_owner_facts_across_runtime_boundary()
    {
        let mut runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return result

NO_DEFAULT = object()

def _is_generator_closed(owner):
    return bool(load_preserved_state(owner._preserved_values, owner._closed_slot))

def _reraise_control_flow(exc):
    raise exc

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        if _is_generator_closed(self):
            raise StopIteration
        try:
            return resume_generator(self._resume_function, self, self._preserved_values, value, NO_DEFAULT)
        except BaseException as exc:
            _reraise_control_flow(exc)
"#,
        )
        .expect("runtime source should lower")
        .blockpy_module;
        soac_driver::blockpy_cache::remap_blockpy_module_function_ids(
            &mut runtime_lowered,
            soac_core::block_py::ModuleNameGen::new(1),
        );
        let user_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def inner(limit):
    yield limit
    yield limit + 1

def outer(limit):
    for value in inner(limit):
        yield value

def caller(limit):
    return list(outer(limit))
"#,
        )
        .expect("user source should lower")
        .blockpy_module;
        let runtime_module_id = runtime_lowered.module_name_gen.runtime_module_id().as_u32();
        let user_module_id = user_lowered.module_name_gen.runtime_module_id().as_u32();
        let runtime_inline_plan = plan_module_inlining(&summarize_module_escapes(&runtime_lowered));
        let runtime_helper = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("runtime helper should exist");
        let iter_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("runtime iter method should exist");
        let next_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("runtime next method should exist");
        let send_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("runtime send method should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&user_lowered);
        let mut runtime_typed = lower_blockpy_module_to_typed(runtime_lowered.clone());
        let mut typed = lower_blockpy_module_to_typed(user_lowered);
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, runtime_helper)]),
            module_globals: HashMap::from([(
                runtime_module_id,
                strict_module_global_direct_call_targets_for_module(
                    &runtime_lowered,
                    "soac.runtime",
                ),
            )]),
            module_global_generators: HashMap::from([(user_module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        let runtime_static_direct_calls =
            static_direct_calls_for_module(&runtime_typed, &static_targets);
        for function in &mut runtime_typed.callable_defs {
            apply_call_emission_plans_to_typed_function(
                function,
                &profile,
                runtime_static_direct_calls.get(&function.function_id),
            )
            .expect("runtime call emission plans should lower");
        }
        let runtime_functions = runtime_typed
            .callable_defs
            .iter()
            .map(|function| (function.names.qualname.clone(), function.clone()))
            .collect::<HashMap<_, _>>();
        let runtime_module_constants = runtime_typed.module_constants.clone();
        let external_callees = HashMap::from([
            (
                runtime_functions
                    .get("list_from_iter")
                    .expect("typed runtime helper should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("list_from_iter")
                        .cloned()
                        .expect("typed runtime helper should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.__iter__")
                    .expect("typed runtime iter method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.__iter__")
                        .cloned()
                        .expect("typed runtime iter method should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.__next__")
                    .expect("typed runtime next method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.__next__")
                        .cloned()
                        .expect("typed runtime next method should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.send")
                    .expect("typed runtime send method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.send")
                        .cloned()
                        .expect("typed runtime send method should exist"),
                    module_constants: runtime_module_constants,
                    inline_plan: Some(runtime_inline_plan),
                },
            ),
        ]);
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let mut static_direct_calls = static_direct_calls_for_module(&typed, &static_targets);
        static_direct_calls.extend(static_direct_calls_for_external_callees(
            &external_callees,
            &static_targets,
        ));
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &external_callees,
            &static_targets,
            &static_direct_calls,
        )
        .expect("typed rewrite loop should inline the cross-module send-wrapper path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        struct RuntimeCallCounter<'a> {
            module_constants: &'a [ConstantExpr],
            iter_calls: usize,
            next_calls: usize,
            resume_calls: usize,
        }
        impl Visit<InstrTyped> for RuntimeCallCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr {
                    self.iter_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Iter,
                        self.module_constants,
                    ));
                    self.next_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        self.module_constants,
                    ));
                    self.resume_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ResumeGenerator,
                        self.module_constants,
                    ));
                }
                expr.visit_children(self);
            }
        }
        let mut runtime_calls = RuntimeCallCounter {
            module_constants: &typed.module_constants,
            iter_calls: 0,
            next_calls: 0,
            resume_calls: 0,
        };
        runtime_calls.visit_fn(caller);
        let (_, residual_resume_report) = trusted_generator_resume_plans_and_report_for_function(
            caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert_eq!(
            runtime_calls.iter_calls, 0,
            "cross-module send-wrapper generator paths should inline the trusted iterator protocol: {residual_resume_report:#?}",
        );
        assert_eq!(
            runtime_calls.resume_calls, 0,
            "cross-module send-wrapper generator paths should consume trusted generator resumes: {residual_resume_report:#?}",
        );
        let (_, residual_next_inline_targets, _, _) =
            trusted_static_runtime_protocol_inlines_for_function(
                caller,
                &typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
                &static_targets,
            );
        assert!(
            residual_next_inline_targets.is_empty(),
            "cross-module send-wrapper generator paths should not leave any still-eligible nested generator next protocol inlines behind: {residual_next_inline_targets:#?}",
        );
        assert_eq!(
            residual_resume_report
                .missing_plan_count(TrustedGeneratorResumePlanMissReason::MissingResumeFunction,),
            0,
            "cross-module list/send-wrapper rewrites should not strand resume calls after dropping _resume_function facts: {residual_resume_report:#?}",
        );
    }

    #[test]
    fn cross_module_send_wrapper_rewrites_keep_resume_targets_plannable() {
        let mut runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
NO_DEFAULT = object()

def _is_generator_closed(owner):
    return bool(load_preserved_state(owner._preserved_values, owner._closed_slot))

def _reraise_control_flow(exc):
    raise exc

class ClosureGenerator:
    def send(self, value):
        if _is_generator_closed(self):
            raise StopIteration
        try:
            return resume_generator(self._resume_function, self, self._preserved_values, value, NO_DEFAULT)
        except BaseException as exc:
            _reraise_control_flow(exc)
"#,
        )
        .expect("runtime source should lower")
        .blockpy_module;
        soac_driver::blockpy_cache::remap_blockpy_module_function_ids(
            &mut runtime_lowered,
            soac_core::block_py::ModuleNameGen::new(1),
        );
        let runtime_inline_plan = plan_module_inlining(&summarize_module_escapes(&runtime_lowered));
        let user_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def caller(limit):
    gen = values(limit)
    return gen.send(None)
"#,
        )
        .expect("user source should lower")
        .blockpy_module;
        let runtime_module_id = runtime_lowered.module_name_gen.runtime_module_id().as_u32();
        let user_module_id = user_lowered.module_name_gen.runtime_module_id().as_u32();
        let send_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("runtime send method should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&user_lowered);
        let mut runtime_typed = lower_blockpy_module_to_typed(runtime_lowered.clone());
        let mut typed = lower_blockpy_module_to_typed(user_lowered);
        let static_targets = StaticDirectCallTargets {
            module_globals: HashMap::from([(
                runtime_module_id,
                strict_module_global_direct_call_targets_for_module(
                    &runtime_lowered,
                    "soac.runtime",
                ),
            )]),
            module_global_generators: HashMap::from([(user_module_id, generator_targets)]),
            strict_methods: HashMap::from([(
                (
                    "soac.runtime".to_string(),
                    "ClosureGenerator".to_string(),
                    "send".to_string(),
                ),
                send_function,
            )]),
            ..StaticDirectCallTargets::default()
        };
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        let runtime_static_direct_calls =
            static_direct_calls_for_module(&runtime_typed, &static_targets);
        for function in &mut runtime_typed.callable_defs {
            apply_call_emission_plans_to_typed_function(
                function,
                &profile,
                runtime_static_direct_calls.get(&function.function_id),
            )
            .expect("runtime call emission plans should lower");
        }
        let runtime_functions = runtime_typed
            .callable_defs
            .iter()
            .map(|function| (function.names.qualname.clone(), function.clone()))
            .collect::<HashMap<_, _>>();
        let runtime_module_constants = runtime_typed.module_constants.clone();
        let external_callees = HashMap::from([(
            runtime_functions
                .get("ClosureGenerator.send")
                .expect("typed runtime send method should exist")
                .function_id,
            TypedExternalInlineCallee {
                function: runtime_functions
                    .get("ClosureGenerator.send")
                    .cloned()
                    .expect("typed runtime send method should exist"),
                module_constants: runtime_module_constants,
                inline_plan: Some(runtime_inline_plan),
            },
        )]);
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let callee_module = typed.clone();
        let caller_index = typed
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let mut late_trusted_owner_states = TrustedOwnerStateCache::default();
        let runtime_protocol_stats = inline_late_typed_runtime_protocol_and_static_method_plans(
            &mut typed.callable_defs[caller_index],
            &mut typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &callee_module,
            &external_callees,
            &static_targets,
            &mut late_trusted_owner_states,
        )
        .expect("late static-method inlining should inline the cross-module send wrapper");
        assert!(
            runtime_protocol_stats.rewritten_returns != 0,
            "the cross-module send wrapper should inline before checking the nested resume helper",
        );
        let linearization =
            linearize_typed_function_expressions(&mut typed.callable_defs[caller_index])
                .expect("post-send expression linearization should succeed");
        if linearization.lifted_nested_exprs != 0 {
            assign_missing_typed_function_instr_ids(&mut typed.callable_defs[caller_index]);
            refresh_typed_function_value_facts(&mut typed.callable_defs[caller_index]);
        }

        let caller = &typed.callable_defs[caller_index];
        let (residual_resume_plans, resume_report) =
            trusted_generator_resume_plans_and_report_for_function(
                caller,
                &typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
            );
        assert!(
            !residual_resume_plans.is_empty(),
            "cross-module send-wrapper inlining should retain a trusted resume plan before the later resume-inline pass consumes it: {resume_report:#?}",
        );
        assert_eq!(
            resume_report
                .missing_plan_count(TrustedGeneratorResumePlanMissReason::MissingResumeFunction,),
            0,
            "cross-module send-wrapper inlining should keep the _resume_function fact intact: {resume_report:#?}",
        );
    }

    #[test]
    fn cross_module_diagonal_set_shell_keeps_resume_targets_plannable() {
        let mut runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return result

def tuple_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return tuple(result)

def set_from_iter(value):
    result = set()
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.add(item)
        except StopIteration:
            return result

NO_DEFAULT = object()

def _is_generator_closed(owner):
    return bool(load_preserved_state(owner._preserved_values, owner._closed_slot))

def _reraise_control_flow(exc):
    raise exc

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        if _is_generator_closed(self):
            raise StopIteration
        try:
            return resume_generator(self._resume_function, self, self._preserved_values, value, NO_DEFAULT)
        except BaseException as exc:
            _reraise_control_flow(exc)
"#,
        )
        .expect("runtime source should lower")
        .blockpy_module;
        soac_driver::blockpy_cache::remap_blockpy_module_function_ids(
            &mut runtime_lowered,
            soac_core::block_py::ModuleNameGen::new(1),
        );
        let runtime_inline_plan = plan_module_inlining(&summarize_module_escapes(&runtime_lowered));
        let user_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def n_queens(queen_count):
    cols = range(queen_count)
    vec = tuple(cols)
    if queen_count == len(set(vec[i] + i for i in cols)):
        yield vec

def caller(limit):
    return list(n_queens(limit))
"#,
        )
        .expect("user source should lower")
        .blockpy_module;
        let runtime_module_id = runtime_lowered.module_name_gen.runtime_module_id().as_u32();
        let user_module_id = user_lowered.module_name_gen.runtime_module_id().as_u32();
        let runtime_list_helper = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("runtime list helper should exist");
        let runtime_tuple_helper = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("runtime tuple helper should exist");
        let runtime_set_helper = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "set_from_iter")
            .cloned()
            .expect("runtime set helper should exist");
        let iter_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("runtime iter method should exist");
        let next_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("runtime next method should exist");
        let send_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("runtime send method should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&user_lowered);
        let mut runtime_typed = lower_blockpy_module_to_typed(runtime_lowered.clone());
        let mut typed = lower_blockpy_module_to_typed(user_lowered);
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([
                (RuntimeName::List, runtime_list_helper),
                (RuntimeName::Tuple, runtime_tuple_helper),
                (RuntimeName::Set, runtime_set_helper),
            ]),
            module_globals: HashMap::from([(
                runtime_module_id,
                strict_module_global_direct_call_targets_for_module(
                    &runtime_lowered,
                    "soac.runtime",
                ),
            )]),
            module_global_generators: HashMap::from([(user_module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        let runtime_static_direct_calls =
            static_direct_calls_for_module(&runtime_typed, &static_targets);
        for function in &mut runtime_typed.callable_defs {
            apply_call_emission_plans_to_typed_function(
                function,
                &profile,
                runtime_static_direct_calls.get(&function.function_id),
            )
            .expect("runtime call emission plans should lower");
        }
        let runtime_functions = runtime_typed
            .callable_defs
            .iter()
            .map(|function| (function.names.qualname.clone(), function.clone()))
            .collect::<HashMap<_, _>>();
        let runtime_module_constants = runtime_typed.module_constants.clone();
        let external_callees = HashMap::from([
            (
                runtime_functions
                    .get("list_from_iter")
                    .expect("typed runtime list helper should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("list_from_iter")
                        .cloned()
                        .expect("typed runtime list helper should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("tuple_from_iter")
                    .expect("typed runtime tuple helper should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("tuple_from_iter")
                        .cloned()
                        .expect("typed runtime tuple helper should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("set_from_iter")
                    .expect("typed runtime set helper should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("set_from_iter")
                        .cloned()
                        .expect("typed runtime set helper should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.__iter__")
                    .expect("typed runtime iter method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.__iter__")
                        .cloned()
                        .expect("typed runtime iter method should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.__next__")
                    .expect("typed runtime next method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.__next__")
                        .cloned()
                        .expect("typed runtime next method should exist"),
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                runtime_functions
                    .get("ClosureGenerator.send")
                    .expect("typed runtime send method should exist")
                    .function_id,
                TypedExternalInlineCallee {
                    function: runtime_functions
                        .get("ClosureGenerator.send")
                        .cloned()
                        .expect("typed runtime send method should exist"),
                    module_constants: runtime_module_constants,
                    inline_plan: Some(runtime_inline_plan),
                },
            ),
        ]);
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &external_callees,
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the cross-module diagonal-set shell path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let (_, residual_resume_report) = trusted_generator_resume_plans_and_report_for_function(
            caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        struct ResidualResumeCounter<'a> {
            module_constants: &'a [ConstantExpr],
            resume_calls: usize,
        }
        impl Visit<InstrTyped> for ResidualResumeCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr {
                    self.resume_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ResumeGenerator,
                        self.module_constants,
                    ));
                }
                expr.visit_children(self);
            }
        }
        let mut residual_resume_calls = ResidualResumeCounter {
            module_constants: &typed.module_constants,
            resume_calls: 0,
        };
        residual_resume_calls.visit_fn(caller);
        let residual_owner_fact_snapshots = residual_resume_report
            .decisions
            .iter()
            .filter_map(|decision| {
                if !matches!(
                    decision.outcome,
                    TrustedGeneratorResumeDecisionOutcome::PlanMissing {
                        reason: TrustedGeneratorResumePlanMissReason::MissingResumeFunction,
                    }
                ) {
                    return None;
                }
                let instr_index = decision.instr_index?;
                let state = analyze_trusted_owner_states(
                    caller,
                    &typed.module_constants,
                    &HashMap::new(),
                    &HashMap::new(),
                )
                .body_before_instr
                .get(&TypedVirtualBodyInstr {
                    block: decision.block,
                    instr_index,
                })?
                .clone();
                let block = caller
                    .blocks
                    .iter()
                    .find(|block| block.label == decision.block)?;
                let instr = block.body.get(instr_index)?;
                let mut snapshot = None;
                struct ResumeOwnerSnapshot<'a> {
                    state: &'a TrustedOwnerState,
                    snapshot: &'a mut Option<(Option<InstrId>, bool, bool)>,
                }
                impl Visit<InstrTyped> for ResumeOwnerSnapshot<'_> {
                    fn visit_instr(&mut self, expr: &InstrTyped) {
                        if self.snapshot.is_none()
                            && let InstrTyped::CallTyped(call) = expr
                            && typed_expr_mentions_resume_generator(expr)
                            && let [
                                CallArgPositional::Positional(_),
                                CallArgPositional::Positional(InstrTyped::Load(owner)),
                                ..,
                            ] = call.args.as_slice()
                        {
                            let origin = trusted_generator_origin_for_name(&owner.name, self.state);
                            *self.snapshot = Some((
                                origin,
                                trusted_generator_resume_function_fact_for_name(
                                    &owner.name,
                                    self.state,
                                )
                                .is_some(),
                                origin.is_some_and(|origin| {
                                    trusted_generator_origin_has_escaped(origin, self.state)
                                }),
                            ));
                            return;
                        }
                        expr.visit_children(self);
                    }
                }
                ResumeOwnerSnapshot {
                    state: &state,
                    snapshot: &mut snapshot,
                }
                .visit_instr(instr);
                let predecessors = trusted_owner_block_predecessor_edges(caller)
                    .remove(&decision.block)
                    .unwrap_or_default();
                let predecessor_snapshots = predecessors
                    .iter()
                    .map(|edge| {
                        let source = caller
                            .blocks
                            .iter()
                            .find(|block| block.label == edge.from)
                            .expect("snapshot predecessor block should exist");
                        let source_state = analyze_trusted_owner_states(
                            caller,
                            &typed.module_constants,
                            &HashMap::new(),
                            &HashMap::new(),
                        )
                        .block_before_term
                        .get(&edge.from)
                        .cloned();
                        let second_hop_predecessors = trusted_owner_block_predecessor_edges(caller)
                            .remove(&edge.from)
                            .unwrap_or_default()
                            .into_iter()
                            .map(|second_edge| {
                                let second_source = caller
                                    .blocks
                                    .iter()
                                    .find(|block| block.label == second_edge.from)
                                    .expect("second-hop predecessor block should exist");
                                let second_state = analyze_trusted_owner_states(
                                    caller,
                                    &typed.module_constants,
                                    &HashMap::new(),
                                    &HashMap::new(),
                                )
                                .block_before_term
                                .get(&second_edge.from)
                                .cloned();
                                (
                                    second_edge,
                                    second_source.body.clone(),
                                    second_source.term.clone(),
                                    second_state,
                                )
                            })
                            .collect::<Vec<_>>();
                        (
                            edge.clone(),
                            source.body.clone(),
                            source.term.clone(),
                            source_state,
                            second_hop_predecessors,
                        )
                    })
                    .collect::<Vec<_>>();
                Some((
                    decision.block,
                    instr_index,
                    snapshot,
                    predecessor_snapshots,
                    state,
                ))
            })
            .collect::<Vec<_>>();
        assert_eq!(
            residual_resume_calls.resume_calls, 0,
            "cross-module diagonal-set shell rewrites should consume nested resume_generator calls: {residual_resume_report:#?}; owner snapshots: {residual_owner_fact_snapshots:#?}",
        );
        assert_eq!(
            residual_resume_report
                .missing_plan_count(TrustedGeneratorResumePlanMissReason::MissingResumeFunction,),
            0,
            "cross-module diagonal-set shell rewrites should keep _resume_function facts intact: {residual_resume_report:#?}",
        );
        struct PreservedFinder(bool);
        impl Visit<InstrTyped> for PreservedFinder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::Load(load) = expr
                    && load.name.preserved_location().is_some()
                {
                    self.0 = true;
                    return;
                }
                expr.visit_children(self);
            }
        }
        let mut preserved_finder = PreservedFinder(false);
        preserved_finder.visit_fn(caller);
        assert!(
            !preserved_finder.0,
            "cross-module diagonal-set shell rewrites should remap preserved state to caller locals",
        );
        let retained_generator_instances = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let generator_plan = match store.value.as_ref() {
                    InstrTyped::CallTyped(call) => call.extra.generator_instance_plan(),
                    InstrTyped::GuardedCallableCallTyped(call) => {
                        call.extra.generator_instance_plan()
                    }
                    InstrTyped::DirectCallableCallTyped(call) => {
                        call.extra.generator_instance_plan()
                    }
                    _ => None,
                }?;
                Some((store.name.id_str().to_string(), generator_plan.function_id))
            })
            .collect::<Vec<_>>();
        assert!(
            retained_generator_instances.is_empty(),
            "cross-module diagonal-set rewrites should remove generator instance constructions: {retained_generator_instances:?}",
        );
        let retained_genexpr_functions = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let InstrTyped::MakeFunctionWithClosure(make_function) = store.value.as_ref()
                else {
                    return None;
                };
                (make_function.kind == FunctionKind::Generator)
                    .then(|| (store.name.id_str().to_string(), make_function.function_id()))
            })
            .collect::<Vec<_>>();
        assert!(
            retained_genexpr_functions.is_empty(),
            "cross-module diagonal-set rewrites should remove synthetic genexpr function materializations: {retained_genexpr_functions:?}",
        );
    }

    #[test]
    fn list_nqueens_style_named_generator_calls_inline_across_runtime_module_boundary() {
        let mut runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return result

def set_from_iter(value):
    result = set()
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.add(item)
        except StopIteration:
            return result

def tuple_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return tuple(result)

NO_DEFAULT = object()

def _is_generator_closed(owner):
    return bool(load_preserved_state(owner._preserved_values, owner._closed_slot))

def _reraise_control_flow(exc):
    raise exc

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        if _is_generator_closed(self):
            raise StopIteration
        try:
            return resume_generator(self._resume_function, self, self._preserved_values, value, NO_DEFAULT)
        except BaseException as exc:
            _reraise_control_flow(exc)
"#,
        )
        .expect("runtime source should lower")
        .blockpy_module;
        soac_driver::blockpy_cache::remap_blockpy_module_function_ids(
            &mut runtime_lowered,
            soac_core::block_py::ModuleNameGen::new(1),
        );
        let runtime_inline_plan = plan_module_inlining(&summarize_module_escapes(&runtime_lowered));
        let user_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def permutations(iterable, r=None):
    pool = tuple(iterable)
    n = len(pool)
    if r is None:
        r = n
    indices = list(range(n))
    cycles = list(range(n - r + 1, n + 1))[::-1]
    yield tuple(pool[i] for i in indices[:r])
    while n:
        for i in reversed(range(r)):
            cycles[i] -= 1
            if cycles[i] == 0:
                indices[i:] = indices[i + 1:] + indices[i:i + 1]
                cycles[i] = n - i
            else:
                j = cycles[i]
                indices[i], indices[-j] = indices[-j], indices[i]
                yield tuple(pool[i] for i in indices[:r])
                break
        else:
            return

def n_queens(queen_count):
    cols = range(queen_count)
    for vec in permutations(cols):
        if (queen_count == len(set(vec[i] + i for i in cols))
                == len(set(vec[i] - i for i in cols))):
            yield vec

def caller(limit):
    list(n_queens(limit))
"#,
        )
        .expect("user source should lower")
        .blockpy_module;
        let runtime_module_id = runtime_lowered.module_name_gen.runtime_module_id().as_u32();
        let user_module_id = user_lowered.module_name_gen.runtime_module_id().as_u32();
        let runtime_helper = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("runtime helper should exist");
        let runtime_set_helper = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "set_from_iter")
            .cloned()
            .expect("runtime set helper should exist");
        let runtime_tuple_helper = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "tuple_from_iter")
            .cloned()
            .expect("runtime tuple helper should exist");
        let iter_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("runtime iter method should exist");
        let next_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("runtime next method should exist");
        let send_function = runtime_lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("runtime send method should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&user_lowered);
        let mut runtime_typed = lower_blockpy_module_to_typed(runtime_lowered.clone());
        let mut typed = lower_blockpy_module_to_typed(user_lowered);
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([
                (RuntimeName::List, runtime_helper),
                (RuntimeName::Set, runtime_set_helper),
                (RuntimeName::Tuple, runtime_tuple_helper),
            ]),
            module_globals: HashMap::from([(
                runtime_module_id,
                strict_module_global_direct_call_targets_for_module(
                    &runtime_lowered,
                    "soac.runtime",
                ),
            )]),
            module_global_generators: HashMap::from([(user_module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        let runtime_static_direct_calls =
            static_direct_calls_for_module(&runtime_typed, &static_targets);
        for function in &mut runtime_typed.callable_defs {
            apply_call_emission_plans_to_typed_function(
                function,
                &profile,
                runtime_static_direct_calls.get(&function.function_id),
            )
            .expect("runtime call emission plans should lower");
        }
        let runtime_functions = runtime_typed
            .callable_defs
            .iter()
            .map(|function| (function.names.qualname.clone(), function.clone()))
            .collect::<HashMap<_, _>>();
        let runtime_module_constants = runtime_typed.module_constants.clone();
        let helper = runtime_functions
            .get("list_from_iter")
            .cloned()
            .expect("typed runtime helper should exist");
        let set_helper = runtime_functions
            .get("set_from_iter")
            .cloned()
            .expect("typed runtime set helper should exist");
        let tuple_helper = runtime_functions
            .get("tuple_from_iter")
            .cloned()
            .expect("typed runtime tuple helper should exist");
        let typed_iter_function = runtime_functions
            .get("ClosureGenerator.__iter__")
            .cloned()
            .expect("typed runtime iter method should exist");
        let typed_next_function = runtime_functions
            .get("ClosureGenerator.__next__")
            .cloned()
            .expect("typed runtime next method should exist");
        let typed_send_function = runtime_functions
            .get("ClosureGenerator.send")
            .cloned()
            .expect("typed runtime send method should exist");
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        let initial_nqueens_generator_plan_count = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "n_queens")
            .and_then(|function| static_generator_instances.get(&function.function_id))
            .map_or(0, HashMap::len);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let external_callees = HashMap::from([
            (
                helper.function_id,
                TypedExternalInlineCallee {
                    function: helper,
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                set_helper.function_id,
                TypedExternalInlineCallee {
                    function: set_helper,
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                tuple_helper.function_id,
                TypedExternalInlineCallee {
                    function: tuple_helper,
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                typed_iter_function.function_id,
                TypedExternalInlineCallee {
                    function: typed_iter_function,
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                typed_next_function.function_id,
                TypedExternalInlineCallee {
                    function: typed_next_function,
                    module_constants: runtime_module_constants.clone(),
                    inline_plan: Some(runtime_inline_plan.clone()),
                },
            ),
            (
                typed_send_function.function_id,
                TypedExternalInlineCallee {
                    function: typed_send_function,
                    module_constants: runtime_module_constants,
                    inline_plan: Some(runtime_inline_plan),
                },
            ),
        ]);
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &external_callees,
            &static_targets,
            &HashMap::new(),
        )
        .expect(
            "typed rewrite loop should inline the cross-module nqueens-style list(generator) path",
        );

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        struct PreservedFinder(Vec<InstrId>);
        impl Visit<InstrTyped> for PreservedFinder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::Load(load) = expr
                    && load.name.preserved_location().is_some()
                {
                    if let Some(instr_id) = expr.try_semantic_instr_id() {
                        self.0.push(instr_id);
                    }
                    return;
                }
                expr.visit_children(self);
            }
        }
        let mut finder = PreservedFinder(Vec::new());
        finder.visit_fn(caller);
        let (_, residual_resume_report) = trusted_generator_resume_plans_and_report_for_function(
            caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            finder.0.is_empty(),
            "cross-module nqueens-style named list(generator) inlining should remap preserved state to caller locals; residual preserved loads={:?}: {residual_resume_report:#?}",
            finder.0,
        );
        struct RuntimeProtocolCallCounter<'a> {
            module_constants: &'a [ConstantExpr],
            iter_calls: usize,
            next_calls: usize,
            resume_calls: usize,
        }
        impl Visit<InstrTyped> for RuntimeProtocolCallCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr {
                    self.iter_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Iter,
                        self.module_constants,
                    ));
                    self.next_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        self.module_constants,
                    ));
                    self.resume_calls += usize::from(typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::ResumeGenerator,
                        self.module_constants,
                    ));
                }
                expr.visit_children(self);
            }
        }
        let mut protocol_calls = RuntimeProtocolCallCounter {
            module_constants: &typed.module_constants,
            iter_calls: 0,
            next_calls: 0,
            resume_calls: 0,
        };
        protocol_calls.visit_fn(caller);
        assert_eq!(
            protocol_calls.resume_calls, 0,
            "cross-module nqueens-style rewrites should consume the inlinable resume-generator path: {residual_resume_report:#?}",
        );
        assert_eq!(
            initial_nqueens_generator_plan_count, 3,
            "nqueens should seed generator-instance plans for the nested named-generator sites"
        );
        let (_, residual_next_inline_targets, _, _) =
            trusted_static_runtime_protocol_inlines_for_function(
                caller,
                &typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
                &static_targets,
            );
        assert!(
            residual_next_inline_targets.is_empty(),
            "cross-module nqueens-style rewrites should not leave any still-eligible nested generator protocol inlines behind",
        );
        let residual_builtin_plans = trusted_generator_builtin_implementation_plans_for_function(
            caller,
            &typed,
            &external_callees,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &static_targets,
        );
        assert!(
            residual_builtin_plans.is_empty(),
            "cross-module nqueens-style rewrites should not leave any still-eligible nested builtin generator consumers behind: {residual_builtin_plans:#?}"
        );
        let n_queens = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "n_queens")
            .expect("typed n_queens should exist");
        struct MissingIdRuntimeNextCounter<'a> {
            module_constants: &'a [ConstantExpr],
            count: usize,
        }
        impl Visit<InstrTyped> for MissingIdRuntimeNextCounter<'_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::CallTyped(call) = expr
                    && typed_expr_is_runtime_name_load(
                        call.func.as_ref(),
                        RuntimeName::Next,
                        self.module_constants,
                    )
                    && call.try_semantic_instr_id().is_none()
                {
                    self.count += 1;
                }
                expr.visit_children(self);
            }
        }
        let mut missing_id_runtime_next_calls = MissingIdRuntimeNextCounter {
            module_constants: &typed.module_constants,
            count: 0,
        };
        missing_id_runtime_next_calls.visit_fn(n_queens);
        assert_eq!(
            missing_id_runtime_next_calls.count, 0,
            "nqueens-style set(genexpr) rewrites should linearize fresh next(...) calls before late runtime-protocol planning",
        );
        let nested_builtin_plans = trusted_generator_builtin_implementation_plans_for_function(
            n_queens,
            &typed,
            &external_callees,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
            &static_targets,
        );
        assert!(
            nested_builtin_plans.is_empty(),
            "nqueens-style nested set(genexpr) consumers should be consumed once typed expression linearization makes them statement-shaped: {nested_builtin_plans:#?}",
        );
        let synthetic_genexpr_function_ids = synthetic_genexpr_function_ids_for_module(&typed);
        assert_no_dead_synthetic_generator_materializations(
            n_queens,
            &typed.module_constants,
            &synthetic_genexpr_function_ids,
            "nqueens-style set(genexpr) consumers should not retain dead generator function materializations",
        );
        let permutations = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "permutations")
            .expect("typed permutations should exist");
        let residual_tuple_builtin_plans =
            trusted_generator_builtin_implementation_plans_for_function(
                permutations,
                &typed,
                &external_callees,
                &typed.module_constants,
                &HashMap::new(),
                &HashMap::new(),
                &static_targets,
            );
        assert!(
            residual_tuple_builtin_plans.is_empty(),
            "nqueens-style tuple(genexpr) rewrites should not leave still-eligible builtin consumers behind: {residual_tuple_builtin_plans:#?}",
        );
        assert_no_dead_synthetic_generator_materializations(
            permutations,
            &typed.module_constants,
            &synthetic_genexpr_function_ids,
            "nqueens-style tuple(genexpr) consumers should not retain dead generator function or instance materializations",
        );
    }

    #[test]
    fn list_nqueens_style_named_generator_calls_inline_resume_state_to_caller_locals() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def list_from_iter(value):
    result = []
    iterator = iter(value)
    while True:
        try:
            item = next(iterator)
            result.append(item)
        except StopIteration:
            return result

class ClosureGenerator:
    def __iter__(self):
        return self

    def __next__(self):
        return self.send(None)

    def send(self, value):
        return resume_generator(self._resume_function, self, self._preserved_values, value, None)

def permutations(iterable, r=None):
    pool = tuple(iterable)
    n = len(pool)
    if r is None:
        r = n
    indices = list(range(n))
    cycles = list(range(n - r + 1, n + 1))[::-1]
    yield tuple(pool[i] for i in indices[:r])
    while n:
        for i in reversed(range(r)):
            cycles[i] -= 1
            if cycles[i] == 0:
                indices[i:] = indices[i + 1:] + indices[i:i + 1]
                cycles[i] = n - i
            else:
                j = cycles[i]
                indices[i], indices[-j] = indices[-j], indices[i]
                yield tuple(pool[i] for i in indices[:r])
                break
        else:
            return

def n_queens(queen_count):
    cols = range(queen_count)
    for vec in permutations(cols):
        if (queen_count == len(set(vec[i] + i for i in cols))
                == len(set(vec[i] - i for i in cols))):
            yield vec

def caller(limit):
    return list(n_queens(limit))
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "list_from_iter")
            .cloned()
            .expect("test helper should exist");
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let iter_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__iter__")
            .cloned()
            .expect("test runtime iter method should exist");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__next__")
            .cloned()
            .expect("test runtime next method should exist");
        let send_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.send")
            .cloned()
            .expect("test runtime send method should exist");
        let mut typed = lower_blockpy_module_to_typed(lowered);
        for function in &mut typed.callable_defs {
            if function.names.qualname != "ClosureGenerator.send" {
                continue;
            }
            struct RuntimeResumeMarker;
            impl VisitMut<InstrTyped> for RuntimeResumeMarker {
                fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                    if let InstrTyped::CallTyped(call) = expr
                        && matches!(
                            call.func.as_ref(),
                            InstrTyped::Load(load) if load.name.id_str() == "resume_generator"
                        )
                    {
                        call.func = Box::new(
                            Load::new(ResolvedName::runtime_name("resume_generator")).into(),
                        );
                        return;
                    }
                    expr.visit_children_mut(self);
                }
            }
            RuntimeResumeMarker.visit_fn_mut(function);
        }
        let static_targets = StaticDirectCallTargets {
            runtime_builtin_implementations: HashMap::from([(RuntimeName::List, helper)]),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::from([
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__iter__".to_string(),
                    ),
                    iter_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "__next__".to_string(),
                    ),
                    next_function,
                ),
                (
                    (
                        "soac.runtime".to_string(),
                        "ClosureGenerator".to_string(),
                        "send".to_string(),
                    ),
                    send_function,
                ),
            ]),
            ..StaticDirectCallTargets::default()
        };
        let static_generator_instances =
            static_generator_instance_plans_for_module(&typed, &static_targets);
        for function in &mut typed.callable_defs {
            annotate_typed_generator_instance_plans(
                function,
                static_generator_instances.get(&function.function_id),
            )
            .expect("generator instance plans should attach");
        }
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        apply_typed_v3_module_rewrites(
            &mut typed,
            &profile,
            None,
            &HashMap::new(),
            &static_targets,
            &HashMap::new(),
        )
        .expect("typed rewrite loop should inline the nqueens-style list(generator) path");

        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let n_queens_function_id = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "n_queens")
            .map(|function| function.function_id)
            .expect("typed n_queens should exist");
        let retained_named_list_generator_instances = caller
            .blocks
            .iter()
            .flat_map(|block| block.body.iter())
            .filter_map(|instr| {
                let InstrTyped::Store(store) = instr else {
                    return None;
                };
                let generator_plan = match store.value.as_ref() {
                    InstrTyped::CallTyped(call) => call.extra.generator_instance_plan(),
                    InstrTyped::GuardedCallableCallTyped(call) => {
                        call.extra.generator_instance_plan()
                    }
                    InstrTyped::DirectCallableCallTyped(call) => {
                        call.extra.generator_instance_plan()
                    }
                    _ => None,
                }?;
                if generator_plan.function_id != n_queens_function_id {
                    return None;
                }
                Some((store.name.id_str().to_string(), generator_plan.function_id))
            })
            .collect::<Vec<_>>();
        assert!(
            retained_named_list_generator_instances.is_empty(),
            "nqueens-style named list(generator) inlining should remove the outer n_queens generator construction: {retained_named_list_generator_instances:?}",
        );
        struct PreservedFinder(bool);
        impl Visit<InstrTyped> for PreservedFinder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                if let InstrTyped::Load(load) = expr
                    && load.name.preserved_location().is_some()
                {
                    self.0 = true;
                    return;
                }
                expr.visit_children(self);
            }
        }
        let mut finder = PreservedFinder(false);
        finder.visit_fn(caller);
        assert!(
            !finder.0,
            "nqueens-style named list(generator) inlining should remap preserved state to caller locals"
        );
        struct ForeignClosureCellFinder(bool);
        impl Visit<InstrTyped> for ForeignClosureCellFinder {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                let foreign_cell = match expr {
                    InstrTyped::CellRef(cell_ref) => matches!(
                        cell_ref.location,
                        CellLocation::Closure(_) | CellLocation::CapturedSource(_)
                    ),
                    InstrTyped::Load(load) => matches!(
                        load.name.location,
                        soac_core::block_py::NameLocation::Cell(
                            CellLocation::Closure(_) | CellLocation::CapturedSource(_)
                        )
                    ),
                    InstrTyped::Store(store) => matches!(
                        store.name.location,
                        soac_core::block_py::NameLocation::Cell(
                            CellLocation::Closure(_) | CellLocation::CapturedSource(_)
                        )
                    ),
                    InstrTyped::Del(del) => matches!(
                        del.name.location,
                        soac_core::block_py::NameLocation::Cell(
                            CellLocation::Closure(_) | CellLocation::CapturedSource(_)
                        )
                    ),
                    _ => false,
                };
                if foreign_cell {
                    self.0 = true;
                    return;
                }
                expr.visit_children(self);
            }
        }
        let mut finder = ForeignClosureCellFinder(false);
        finder.visit_fn(caller);
        assert!(
            !finder.0,
            "resume inlining must not leave callee closure slots addressed through the caller runtime environment"
        );
    }

    #[test]
    fn trusted_owner_analysis_ignores_unreachable_resume_blocks_without_pruning_them() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def helper(fn, owner, state, value, exc):
    return value

def caller(limit):
    gen = values(limit)
    value = helper(gen._resume_function, gen, gen._preserved_values, None, None)
    return value
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let plans = static_generator_instance_plans_for_function(
            &caller,
            &StaticDirectCallTargets {
                module_global_generators: HashMap::from([(module_id, generator_targets)]),
                ..StaticDirectCallTargets::default()
            },
        );
        annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
            .expect("instance-plan annotation should succeed");

        let resume_block_label = caller
            .blocks
            .iter_mut()
            .find_map(|block| {
                block.body.iter_mut().find_map(|instr| {
                    let InstrTyped::Store(store) = instr else {
                        return None;
                    };
                    let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                        return None;
                    };
                    call.func =
                        Box::new(Load::new(ResolvedName::runtime_name("resume_generator")).into());
                    Some(block.label)
                })
            })
            .expect("caller should store the resume helper result");
        let reachable_resume_block = caller
            .blocks
            .iter()
            .find(|block| block.label == resume_block_label)
            .cloned()
            .expect("resume block should remain present after marking its call");
        let next_label_index = caller
            .blocks
            .iter()
            .map(|block| block.label.as_u32())
            .filter(|index| *index != BlockLabel::FALLTHROUGH_INDEX)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        let unreachable_label = BlockLabel::from_index(
            usize::try_from(next_label_index).expect("block label should fit usize"),
        );
        let mut unreachable_resume_block = reachable_resume_block.clone();
        unreachable_resume_block.label = unreachable_label;
        caller.blocks.push(unreachable_resume_block);

        let analysis = analyze_trusted_owner_states(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        assert!(
            analysis
                .reachable_blocks
                .contains(reachable_resume_block.label),
            "reachable resume block should stay visible to trusted-owner analysis",
        );
        assert!(
            !analysis.reachable_blocks.contains(unreachable_label),
            "unreachable resume block should be excluded from the trusted-owner analysis view",
        );
        assert!(
            analysis
                .body_before_instr
                .keys()
                .all(|site| site.block != unreachable_label),
            "analysis should not materialize trusted-owner state for physically present unreachable blocks",
        );

        let block_count_before_prune = caller.blocks.len();
        assert_eq!(prune_unreachable_typed_blocks(&mut caller), 1);
        assert_eq!(caller.blocks.len(), block_count_before_prune - 1);
        assert!(
            caller
                .blocks
                .iter()
                .all(|block| block.label != unreachable_label),
            "physical CFG pruning should stay separate from the non-mutating analysis view",
        );
    }

    #[test]
    fn trusted_owner_edge_remap_preserves_object_and_function_facts() {
        let source_location = LocalLocation(7);
        let target_location = LocalLocation(8);
        let origin = InstrId::new(9);
        let function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(10), LocalFunctionId::new(11));
        let state = TrustedOwnerState {
            object_origins: HashMap::from([(source_location, origin)]),
            local_functions: HashMap::from([(source_location, function_id)]),
            resume_functions: HashMap::from([(
                source_location,
                soac_opt::passes::TrustedResumeFunctionFact {
                    function_id,
                    origins: soac_opt::passes::TrustedResumeFunctionOrigins::Single(origin),
                },
            )]),
            ..TrustedOwnerState::default()
        };
        let remapped = remap_trusted_owner_state_for_edge(
            Some(&[(source_location, target_location)]),
            None,
            &state,
        );

        assert_eq!(remapped.object_origins.get(&target_location), Some(&origin));
        assert_eq!(
            remapped.local_functions.get(&target_location),
            Some(&function_id)
        );
        assert_eq!(
            remapped
                .resume_functions
                .get(&target_location)
                .map(|fact| fact.function_id),
            Some(function_id),
            "edge remapping should keep resume-function facts in lockstep with ordinary function facts",
        );
    }

    #[test]
    fn trusted_owner_state_merge_keeps_non_conflicting_origin_field_facts() {
        let origin = InstrId::new(12);
        let field = (origin, "_resume_function".to_string());
        let function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(13), LocalFunctionId::new(14));
        let other_function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(13), LocalFunctionId::new(15));
        let precise_state = TrustedOwnerState {
            function_fields: HashMap::from([(field.clone(), function_id)]),
            ..TrustedOwnerState::default()
        };

        let merged =
            merge_trusted_owner_states(&[precise_state.clone(), TrustedOwnerState::default()]);
        assert_eq!(merged.function_fields.get(&field), Some(&function_id));

        let conflicting = merge_trusted_owner_states(&[
            precise_state,
            TrustedOwnerState {
                function_fields: HashMap::from([(field.clone(), other_function_id)]),
                ..TrustedOwnerState::default()
            },
        ]);
        assert!(
            !conflicting.function_fields.contains_key(&field),
            "conflicting origin-scoped field bindings must stop being trusted"
        );
    }

    #[test]
    fn trusted_owner_state_merge_keeps_non_conflicting_resume_function_facts() {
        let location = LocalLocation(16);
        let origin = InstrId::new(17);
        let second_origin = InstrId::new(18);
        let function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(19), LocalFunctionId::new(20));
        let other_function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(19), LocalFunctionId::new(21));
        let precise_state = TrustedOwnerState {
            resume_functions: HashMap::from([(
                location,
                soac_opt::passes::TrustedResumeFunctionFact {
                    function_id,
                    origins: soac_opt::passes::TrustedResumeFunctionOrigins::Single(origin),
                },
            )]),
            ..TrustedOwnerState::default()
        };
        let widened_same_target = TrustedOwnerState {
            resume_functions: HashMap::from([(
                location,
                soac_opt::passes::TrustedResumeFunctionFact {
                    function_id,
                    origins: soac_opt::passes::TrustedResumeFunctionOrigins::Single(second_origin),
                },
            )]),
            ..TrustedOwnerState::default()
        };

        let merged = merge_trusted_owner_states(&[precise_state.clone(), widened_same_target]);
        assert_eq!(
            merged
                .resume_functions
                .get(&location)
                .map(|fact| (fact.function_id, fact.origins.clone())),
            Some((
                function_id,
                soac_opt::passes::TrustedResumeFunctionOrigins::Multiple(HashSet::from([
                    origin,
                    second_origin,
                ])),
            )),
            "matching resume-function targets should survive a join while widening candidate origins",
        );

        let conflicting = merge_trusted_owner_states(&[
            precise_state,
            TrustedOwnerState {
                resume_functions: HashMap::from([(
                    location,
                    soac_opt::passes::TrustedResumeFunctionFact {
                        function_id: other_function_id,
                        origins: soac_opt::passes::TrustedResumeFunctionOrigins::Single(origin),
                    },
                )]),
                ..TrustedOwnerState::default()
            },
        ]);
        assert!(
            !conflicting.resume_functions.contains_key(&location),
            "conflicting resume-function targets must stop being trusted",
        );
    }

    #[test]
    fn trusted_owner_state_merge_keeps_resume_function_facts_across_missing_join_arms() {
        let location = LocalLocation(22);
        let origin = InstrId::new(23);
        let function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(24), LocalFunctionId::new(25));
        let precise_state = TrustedOwnerState {
            resume_functions: HashMap::from([(
                location,
                soac_opt::passes::TrustedResumeFunctionFact {
                    function_id,
                    origins: soac_opt::passes::TrustedResumeFunctionOrigins::Single(origin),
                },
            )]),
            ..TrustedOwnerState::default()
        };

        let merged =
            merge_trusted_owner_states(&[precise_state.clone(), TrustedOwnerState::default()]);
        assert_eq!(
            merged
                .resume_functions
                .get(&location)
                .map(|fact| (fact.function_id, fact.origins.clone())),
            Some((
                function_id,
                soac_opt::passes::TrustedResumeFunctionOrigins::Single(origin),
            )),
            "a branch that contributes no conflicting resume-function fact should not erase the trusted target",
        );
    }

    #[test]
    fn trusted_owner_predecessors_include_exception_edges() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(value):\n    try:\n        return value.attr\n    except Exception:\n        return None\n",
        )
        .expect("source should lower");
        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let (source_block, target_block, exc_args) = caller
            .blocks
            .iter()
            .find_map(|block| {
                block
                    .exc_edge
                    .as_ref()
                    .map(|edge| (block.label, edge.target, edge.args.clone()))
            })
            .expect("lowered try/except should have an exception edge");
        let predecessors = trusted_owner_block_predecessor_edges(caller);
        assert!(
            predecessors
                .get(&target_block)
                .is_some_and(|edges| edges.iter().any(|edge| {
                    edge.from == source_block
                        && edge
                            .explicit_args
                            .as_ref()
                            .is_some_and(|args| args.len() == exc_args.len())
                })),
            "trusted owner dataflow should see exception-edge predecessors",
        );
    }

    #[test]
    fn trusted_owner_abrupt_kind_dispatch_preserves_generator_facts_for_fallthrough_case() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def caller(limit):
    try:
        gen = values(limit)
    finally:
        limit = limit
    return next(gen)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let plans = static_generator_instance_plans_for_function(
            &caller,
            &StaticDirectCallTargets {
                module_global_generators: HashMap::from([(module_id, generator_targets)]),
                ..StaticDirectCallTargets::default()
            },
        );
        annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
            .expect("instance-plan annotation should succeed");

        let analysis = analyze_trusted_owner_states(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "ClosureGenerator".to_string(),
        };
        let (next_receiver, next_state) = caller
            .blocks
            .iter()
            .find_map(|block| {
                let BlockTerm::Return(InstrTyped::CallTyped(call)) = &block.term else {
                    return None;
                };
                if !typed_expr_is_runtime_name_load(
                    call.func.as_ref(),
                    RuntimeName::Next,
                    &typed.module_constants,
                ) {
                    return None;
                }
                let soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(receiver)) =
                    call.args.first()?
                else {
                    return None;
                };
                Some((
                    receiver.name.clone(),
                    analysis
                        .block_before_term
                        .get(&block.label)
                        .expect("next() term should have trusted-owner state"),
                ))
            })
            .expect("caller should return next(gen)");

        assert_eq!(
            trusted_owner_state_for_name(&next_receiver, next_state),
            Some(&owner_type_ref),
            "fallthrough abrupt-kind dispatch should preserve trusted generator owner facts: {next_state:#?}",
        );
        assert!(
            trusted_object_origin_for_name(&next_receiver, next_state).is_some(),
            "fallthrough abrupt-kind dispatch should preserve the exact generator origin: {next_state:#?}",
        );
    }

    #[test]
    fn trusted_owner_abrupt_kind_dispatch_materializes_generator_facts_for_body_calls() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def caller(limit):
    try:
        gen = values(limit)
    finally:
        limit = limit
    value = next(gen)
    return value
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let plans = static_generator_instance_plans_for_function(
            &caller,
            &StaticDirectCallTargets {
                module_global_generators: HashMap::from([(module_id, generator_targets)]),
                ..StaticDirectCallTargets::default()
            },
        );
        annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
            .expect("instance-plan annotation should succeed");

        let analysis = analyze_trusted_owner_states(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "ClosureGenerator".to_string(),
        };
        let (next_receiver, next_state) = caller
            .blocks
            .iter()
            .find_map(|block| {
                block
                    .body
                    .iter()
                    .enumerate()
                    .find_map(|(instr_index, instr)| {
                        let InstrTyped::Store(store) = instr else {
                            return None;
                        };
                        let InstrTyped::CallTyped(call) = store.value.as_ref() else {
                            return None;
                        };
                        if !typed_expr_is_runtime_name_load(
                            call.func.as_ref(),
                            RuntimeName::Next,
                            &typed.module_constants,
                        ) {
                            return None;
                        }
                        let soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(
                            receiver,
                        )) = call.args.first()?
                        else {
                            return None;
                        };
                        Some((
                            receiver.name.clone(),
                            analysis
                                .body_before_instr
                                .get(&TypedVirtualBodyInstr {
                                    block: block.label,
                                    instr_index,
                                })
                                .expect("next() body call should have trusted-owner state"),
                        ))
                    })
            })
            .expect("caller should store next(gen)");

        assert_eq!(
            trusted_owner_state_for_name(&next_receiver, next_state),
            Some(&owner_type_ref),
            "fallthrough abrupt-kind dispatch should materialize trusted generator owner facts for later body calls: {next_state:#?}",
        );
        assert!(
            trusted_object_origin_for_name(&next_receiver, next_state).is_some(),
            "fallthrough abrupt-kind dispatch should materialize the exact generator origin for later body calls: {next_state:#?}",
        );
    }

    #[test]
    fn trusted_owner_abrupt_kind_dispatch_rejoins_generator_facts_after_case_key_release() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def caller(limit):
    try:
        gen = values(limit)
    finally:
        limit = limit
    if limit:
        alias = gen
    else:
        alias = gen
    value = next(alias)
    return value
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let generator_targets = strict_module_global_generator_targets_for_module(&lowered);
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let plans = static_generator_instance_plans_for_function(
            &caller,
            &StaticDirectCallTargets {
                module_global_generators: HashMap::from([(module_id, generator_targets)]),
                ..StaticDirectCallTargets::default()
            },
        );
        annotate_typed_generator_instance_plans(&mut caller, Some(&plans))
            .expect("instance-plan annotation should succeed");

        let analysis = analyze_trusted_owner_states(
            &caller,
            &typed.module_constants,
            &HashMap::new(),
            &HashMap::new(),
        );
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "ClosureGenerator".to_string(),
        };
        let (next_receiver, next_state) = caller
            .blocks
            .iter()
            .find_map(|block| {
                block
                    .body
                    .iter()
                    .enumerate()
                    .find_map(|(instr_index, instr)| {
                        let InstrTyped::Store(store) = instr else {
                            return None;
                        };
                        let InstrTyped::CallTyped(call) = store.value.as_ref() else {
                            return None;
                        };
                        if !typed_expr_is_runtime_name_load(
                            call.func.as_ref(),
                            RuntimeName::Next,
                            &typed.module_constants,
                        ) {
                            return None;
                        }
                        let soac_core::block_py::CallArgPositional::Positional(
                            InstrTyped::Load(receiver),
                        ) = call.args.first()?
                        else {
                            return None;
                        };
                        Some((
                            receiver.name.clone(),
                            analysis
                                .body_before_instr
                                .get(&TypedVirtualBodyInstr {
                                    block: block.label,
                                    instr_index,
                                })
                                .expect(
                                    "next() after post-finally join should have trusted-owner state",
                                ),
                        ))
                    })
            })
            .expect("caller should store next(alias)");

        assert_eq!(
            trusted_owner_state_for_name(&next_receiver, next_state),
            Some(&owner_type_ref),
            "released abrupt-kind dispatch facts should rejoin the ordinary owner lattice: {next_state:#?}",
        );
        assert!(
            trusted_object_origin_for_name(&next_receiver, next_state).is_some(),
            "released abrupt-kind dispatch facts should keep the exact generator origin: {next_state:#?}",
        );
    }

    #[test]
    fn trusted_runtime_protocol_calls_use_callsite_virtual_state() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(it):\n    value = next(it)\n    return value\n",
        )
        .expect("source should lower");
        let mut typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let caller = typed
            .callable_defs
            .iter_mut()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let owner_type_ref = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "IterRange".to_string(),
        };
        let (block, instr_index, instr_id, receiver_location) = caller
            .blocks
            .iter_mut()
            .find_map(|block| {
                block
                    .body
                    .iter_mut()
                    .enumerate()
                    .find_map(|(instr_index, instr)| {
                        let InstrTyped::Store(store) = instr else {
                            return None;
                        };
                        let InstrTyped::CallTyped(call) = store.value.as_mut() else {
                            return None;
                        };
                        let soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(
                            receiver,
                        )) = call.args.first()?
                        else {
                            return None;
                        };
                        let receiver_location = receiver.name.local_location()?;
                        let instr_id = call.try_semantic_instr_id()?;
                        call.access =
                            soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
                                runtime_name: RuntimeName::Next,
                                method_name: "__next__".to_string(),
                                method_guards: vec![TypedDirectMethodCallGuard {
                                    function_id: RuntimeFunctionId::new(
                                        RuntimeModuleId::new(2),
                                        LocalFunctionId::new(7),
                                    ),
                                    owner_type_ref: owner_type_ref.clone(),
                                    type_version: 1,
                                    arg_plan: TypedDirectCallArgPlan {
                                        sources: vec![
                                            soac_ir_typed::TypedDirectCallArgSource::Provided(0),
                                        ],
                                    },
                                }],
                            };
                        Some((block.label, instr_index, instr_id, receiver_location))
                    })
            })
            .expect("caller should contain a next() store");
        let object = TypedVirtualObjectId(11);
        let field_states = TypedVirtualFieldStateAnalysis {
            body_before_instr: HashMap::from([(
                TypedVirtualBodyInstr { block, instr_index },
                TypedVirtualState {
                    aliases: HashMap::from([(receiver_location, object)]),
                    ..TypedVirtualState::default()
                },
            )]),
            ..TypedVirtualFieldStateAnalysis::default()
        };

        assert_eq!(
            trusted_runtime_protocol_calls_from_field_states(
                caller,
                &field_states,
                &HashMap::from([(object, owner_type_ref.clone())]),
            ),
            HashMap::from([(instr_id, owner_type_ref)]),
        );
    }

    #[test]
    fn strict_module_final_globals_build_static_targets() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def helper(value):
    return value

class Box:
    def __init__(self, value):
        self.value = value
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let targets = strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod");
        let helper = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "helper")
            .expect("helper function should exist");
        assert_eq!(
            targets
                .get("helper")
                .expect("helper global should be static")
                .function
                .function_id,
            helper.function_id,
        );
        assert!(
            !targets.contains_key("Box"),
            "user-module constructors are not direct-call ready during module initialization"
        );
    }

    #[test]
    fn strict_module_rebound_globals_do_not_build_static_targets() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

Box = None
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let targets = strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod");

        assert!(
            !targets.contains_key("Box"),
            "rebound globals should stay dynamic"
        );
    }

    #[test]
    fn strict_module_generator_globals_do_not_build_static_function_targets() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    for value in range(limit):
        yield value
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let targets = strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod");

        assert!(
            !targets.contains_key("values"),
            "named generators keep CPython vectorcall and do not have direct-call metadata"
        );
    }

    #[test]
    fn strict_module_generator_calls_build_instance_plans_without_direct_calling() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def values(limit):
    yield limit

def caller(limit):
    return values(limit)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let target_function_id = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "values")
            .expect("generator target should exist")
            .function_id;
        let targets = StaticDirectCallTargets {
            runtime_names: HashMap::new(),
            runtime_builtin_implementations: HashMap::new(),
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod"),
            )]),
            module_global_generators: HashMap::from([(
                module_id,
                strict_module_global_generator_targets_for_module(&lowered),
            )]),
            strict_methods: HashMap::new(),
            suppressed_source_generators: HashSet::new(),
        };
        let typed = lower_blockpy_module_to_typed(lowered);
        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");

        assert!(
            static_direct_call_plans_for_function(caller, &typed.module_constants, &targets)
                .is_empty(),
            "generator public calls must keep factory/vectorcall semantics"
        );
        let plans = static_generator_instance_plans_for_function(caller, &targets);
        let (source, plan) = plans
            .iter()
            .next()
            .expect("generator call should receive an instance plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plan.function_id, target_function_id);
        assert_eq!(plan.kind, FunctionKind::Generator);
        assert_eq!(
            plan.arg_plan,
            TypedDirectCallArgPlan {
                sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
            }
        );

        let mut annotated = caller.clone();
        annotate_typed_generator_instance_plans(&mut annotated, Some(&plans))
            .expect("instance-plan annotation should succeed");
        let annotated_plan = annotated
            .blocks
            .iter()
            .find_map(|block| match &block.term {
                BlockTerm::Return(expr) if expr.try_semantic_instr_id() == Some(*source) => {
                    expr.generator_instance_plan()
                }
                _ => None,
            })
            .expect("generator call should carry typed metadata");
        assert_eq!(annotated_plan, plan);
    }

    #[test]
    fn local_generator_calls_build_instance_plans_from_known_function_values() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def caller(limit):
    def values(limit):
        yield limit
    return values(limit)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let target_function_id = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller.<locals>.values")
            .expect("nested generator target should exist")
            .function_id;
        let typed = lower_blockpy_module_to_typed(lowered);
        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let plans_by_function =
            static_generator_instance_plans_for_module(&typed, &StaticDirectCallTargets::default());
        let plans = plans_by_function
            .get(&caller.function_id)
            .expect("caller should receive a local generator-instance plan");
        let plan = plans
            .values()
            .next()
            .expect("nested generator call should receive an instance plan");

        assert_eq!(plans.len(), 1);
        assert_eq!(plan.function_id, target_function_id);
        assert_eq!(plan.kind, FunctionKind::Generator);
        assert_eq!(
            plan.arg_plan,
            TypedDirectCallArgPlan {
                sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
            }
        );
    }

    #[test]
    fn strict_module_protocol_methods_build_static_targets() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class IterRange:
    def __iter__(self):
        return self

    def __next__(self):
        return 1
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let targets = strict_module_method_targets_for_module(&lowered, "pkg.runtime");
        let next_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "IterRange.__next__")
            .expect("IterRange.__next__ should exist");
        assert_eq!(
            targets
                .get(&(
                    "pkg.runtime".to_string(),
                    "IterRange".to_string(),
                    "__next__".to_string(),
                ))
                .expect("strict protocol method should be static")
                .function_id,
            next_function.function_id,
        );
    }

    #[test]
    fn only_runtime_range_constructors_are_trusted_for_full_virtualization() {
        assert!(trusted_fully_virtual_constructor_owner(
            &TypedAttrOwnerRef::TypeKey {
                module_name: "soac.runtime".to_string(),
                qualname: "range".to_string(),
            }
        ));
        assert!(trusted_fully_virtual_constructor_owner(
            &TypedAttrOwnerRef::TypeKey {
                module_name: "soac.runtime".to_string(),
                qualname: "IterRange".to_string(),
            }
        ));
        assert!(!trusted_fully_virtual_constructor_owner(
            &TypedAttrOwnerRef::TypeKey {
                module_name: "pkg.mod".to_string(),
                qualname: "Watch".to_string(),
            }
        ));
    }

    #[test]
    fn static_runtime_range_calls_build_inline_constructor_plans() {
        let caller_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "def caller(i):\n    value = range(i)\n    return value\n",
        )
        .expect("caller source should lower");
        let caller_typed = lower_blockpy_module_to_typed(caller_lowered.blockpy_module);
        let caller = caller_typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");

        let runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class range:\n    def __init__(self, *args):\n        self.args = args\n",
        )
        .expect("runtime source should lower");
        let init_function = runtime_lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "range.__init__")
            .expect("runtime module should contain range.__init__");
        let entry_function_id = soac_ir_blockpy::constructor_entry_function_id_for_init(
            &runtime_lowered.blockpy_module,
            init_function.function_id,
        )
        .expect("range init should have a constructor entry");
        let entry_function = runtime_lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.function_id == entry_function_id)
            .cloned()
            .expect("runtime module should contain the constructor entry");
        let plans = static_direct_call_plans_for_function(
            caller,
            &caller_typed.module_constants,
            &StaticDirectCallTargets {
                runtime_names: HashMap::from([(
                    RuntimeName::Range,
                    StaticDirectCallTarget {
                        function: entry_function,
                        constructor_owner_type_ref: Some(TypedAttrOwnerRef::TypeKey {
                            module_name: "soac.runtime".to_string(),
                            qualname: "range".to_string(),
                        }),
                    },
                )]),
                runtime_builtin_implementations: HashMap::new(),
                module_globals: HashMap::new(),
                module_global_generators: HashMap::new(),
                strict_methods: HashMap::new(),
                suppressed_source_generators: HashSet::new(),
            },
        );
        let plans = plans
            .values()
            .next()
            .expect("runtime range call should receive a static direct-call plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].target, entry_function_id);
        assert_eq!(plans[0].body.kind, CallBodyKind::Inline);
    }

    #[test]
    fn generated_generator_functions_do_not_emit_runtime_constructor_calls() {
        let caller_lowered =
            soac_lowering::lower_python_to_blockpy_for_testing("def gen():\n    yield 1\n")
                .expect("caller source should lower");
        let caller_typed = lower_blockpy_module_to_typed(caller_lowered.blockpy_module);
        let factory = caller_typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "gen")
            .expect("typed generator factory should exist");

        let runtime_lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            "class ClosureGenerator:\n    def __init__(self, resume, name, qualname, code, preserved_values, yieldfrom_slot, throw_context_slot):\n        self.resume = resume\n",
        )
        .expect("runtime source should lower");
        let init_function = runtime_lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "ClosureGenerator.__init__")
            .expect("runtime module should contain ClosureGenerator.__init__");
        let entry_function_id = soac_ir_blockpy::constructor_entry_function_id_for_init(
            &runtime_lowered.blockpy_module,
            init_function.function_id,
        )
        .expect("ClosureGenerator init should have a constructor entry");
        let entry_function = runtime_lowered
            .blockpy_module
            .callable_defs
            .iter()
            .find(|function| function.function_id == entry_function_id)
            .cloned()
            .expect("runtime module should contain the constructor entry");
        let plans = static_direct_call_plans_for_function(
            factory,
            &caller_typed.module_constants,
            &StaticDirectCallTargets {
                runtime_names: HashMap::from([(
                    RuntimeName::ClosureGenerator,
                    StaticDirectCallTarget {
                        function: entry_function,
                        constructor_owner_type_ref: Some(TypedAttrOwnerRef::TypeKey {
                            module_name: "soac.runtime".to_string(),
                            qualname: "ClosureGenerator".to_string(),
                        }),
                    },
                )]),
                runtime_builtin_implementations: HashMap::new(),
                module_globals: HashMap::new(),
                module_global_generators: HashMap::new(),
                strict_methods: HashMap::new(),
                suppressed_source_generators: HashSet::new(),
            },
        );
        assert!(
            plans.is_empty(),
            "single-callable generators should be instantiated by vectorcall, not by a generated ClosureGenerator constructor call: {plans:?}"
        );
    }

    #[test]
    fn strict_runtime_global_calls_build_direct_constructor_plans_without_trust() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def caller(value):
    return Box(value)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let init_function = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "Box.__init__")
            .expect("Box.__init__ should exist");
        let constructor_entry_id =
            constructor_entry_function_id_for_init(&lowered, init_function.function_id)
                .expect("Box should have a constructor entry");
        let targets = StaticDirectCallTargets {
            runtime_names: HashMap::new(),
            runtime_builtin_implementations: HashMap::new(),
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "soac.runtime"),
            )]),
            module_global_generators: HashMap::new(),
            strict_methods: HashMap::new(),
            suppressed_source_generators: HashSet::new(),
        };
        let typed = lower_blockpy_module_to_typed(lowered);
        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let plans =
            static_direct_call_plans_for_function(caller, &typed.module_constants, &targets);
        let plans = plans
            .values()
            .next()
            .expect("Box call should receive a static direct-call plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].target, constructor_entry_id);
        assert_eq!(plans[0].body.kind, CallBodyKind::DirectCall);
        assert!(
            plans[0].reason.contains("strict module global Box"),
            "strict module calls should record why they were static"
        );
    }

    #[test]
    fn static_constructor_owner_refs_survive_direct_callable_lowering() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
class Box:
    def __init__(self, value):
        self.value = value

def caller(value):
    return Box(value)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let targets = StaticDirectCallTargets {
            runtime_names: HashMap::new(),
            runtime_builtin_implementations: HashMap::new(),
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "soac.runtime"),
            )]),
            module_global_generators: HashMap::new(),
            strict_methods: HashMap::new(),
            suppressed_source_generators: HashSet::new(),
        };
        let typed = lower_blockpy_module_to_typed(lowered);
        let mut caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .cloned()
            .expect("typed caller should exist");
        let plans =
            static_direct_call_plans_for_function(&caller, &typed.module_constants, &targets);
        let (source, plans) = plans
            .iter()
            .next()
            .expect("Box call should receive a static direct-call plan");
        let mut emissions = TypedCallEmissionPlans::default();
        insert_static_direct_callable_plan(&mut emissions, *source, plans)
            .expect("static constructor should lower to direct callable");
        lower_typed_function_call_emission_plans(&mut caller, &emissions)
            .expect("direct callable lowering should succeed");

        assert_eq!(
            static_constructor_call_owner_refs(&caller, &typed.module_constants, &targets),
            HashMap::from([(
                *source,
                TypedAttrOwnerRef::TypeKey {
                    module_name: "soac.runtime".to_string(),
                    qualname: "Box".to_string(),
                },
            )]),
        );
    }

    #[test]
    fn strict_module_function_globals_build_direct_call_plans_without_inlining() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def helper(value):
    return value

def caller(value):
    return helper(value)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let helper_function_id = lowered
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "helper")
            .expect("helper should exist")
            .function_id;
        let targets = StaticDirectCallTargets {
            runtime_names: HashMap::new(),
            runtime_builtin_implementations: HashMap::new(),
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod"),
            )]),
            module_global_generators: HashMap::new(),
            strict_methods: HashMap::new(),
            suppressed_source_generators: HashSet::new(),
        };
        let typed = lower_blockpy_module_to_typed(lowered);
        let caller = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "caller")
            .expect("typed caller should exist");
        let plans =
            static_direct_call_plans_for_function(caller, &typed.module_constants, &targets);
        let plans = plans
            .values()
            .next()
            .expect("helper call should receive a static direct-call plan");
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].target, helper_function_id);
        assert_eq!(plans[0].body.kind, CallBodyKind::DirectCall);
    }

    #[test]
    fn strict_module_globals_stay_dynamic_during_module_initialization() {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(
            r#"
def helper(value):
    return value

helper(1)
"#,
        )
        .expect("source should lower")
        .blockpy_module;
        let module_id = lowered.module_name_gen.runtime_module_id().as_u32();
        let targets = StaticDirectCallTargets {
            runtime_names: HashMap::new(),
            runtime_builtin_implementations: HashMap::new(),
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod"),
            )]),
            module_global_generators: HashMap::new(),
            strict_methods: HashMap::new(),
            suppressed_source_generators: HashSet::new(),
        };
        let typed = lower_blockpy_module_to_typed(lowered);
        let module_init = typed
            .callable_defs
            .iter()
            .find(|function| function.scope.scope_kind == CallableScopeKind::Module)
            .expect("typed module init should exist");

        assert!(
            static_direct_call_plans_for_function(module_init, &typed.module_constants, &targets)
                .is_empty(),
            "module initialization should not assume final globals are already settled"
        );
    }

    #[test]
    fn static_runtime_calls_emit_direct_callable_plans() {
        let source = InstrId::new(7);
        let target = RuntimeFunctionId::new(RuntimeModuleId::new(2), LocalFunctionId::new(9));
        let direct_call = ResolvedV3DirectCallPlan {
            source,
            target,
            callee: DirectCallCallee::Function,
            arg_plan: TypedDirectCallArgPlan {
                sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
            },
            body: static_inline_call_body(),
            reason: "static test target".to_string(),
        };
        let mut emissions = TypedCallEmissionPlans::default();

        insert_static_direct_callable_plan(&mut emissions, source, &[direct_call])
            .expect("static runtime call should become a direct callable plan");

        assert_eq!(
            emissions.by_source.get(&source),
            Some(&TypedCallEmissionPlan::DirectCallable {
                function_guard: TypedDirectFunctionCallGuard {
                    function_id: target,
                    arg_plan: TypedDirectCallArgPlan {
                        sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
                    },
                },
            }),
        );
    }

    #[test]
    fn remapped_call_emissions_follow_inlined_static_calls() {
        let caller_id = RuntimeFunctionId::new(RuntimeModuleId::new(1), LocalFunctionId::new(2));
        let callee_id = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let target = RuntimeFunctionId::new(RuntimeModuleId::new(5), LocalFunctionId::new(6));
        let callee_source = InstrId::new(7);
        let caller_source = InstrId::new(8);
        let arg_plan = TypedDirectCallArgPlan {
            sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
        };
        let static_direct_calls = HashMap::from([(
            callee_id,
            HashMap::from([(
                callee_source,
                vec![ResolvedV3DirectCallPlan {
                    source: callee_source,
                    target,
                    callee: DirectCallCallee::Function,
                    arg_plan: arg_plan.clone(),
                    body: static_inline_call_body(),
                    reason: "static nested call".to_string(),
                }],
            )]),
        )]);
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::new(),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };
        let mappings = [TypedInlineInstrIdMapping {
            callee: callee_id,
            inline_instance: 0,
            callee_instr_id: callee_source,
            caller_instr_id: caller_source,
        }];
        let mut remapped_call_emissions = RemappedTypedCallEmissions::new();

        assert_eq!(
            remap_inlined_call_emission_plans(
                caller_id,
                &mappings,
                &profile,
                &static_direct_calls,
                &mut remapped_call_emissions,
            )
            .expect("static nested call emission should remap"),
            1,
        );
        assert_eq!(
            remapped_call_emissions
                .get(&caller_id)
                .and_then(|plans| plans.by_source.get(&caller_source)),
            Some(&TypedCallEmissionPlan::DirectCallable {
                function_guard: TypedDirectFunctionCallGuard {
                    function_id: target,
                    arg_plan,
                },
            }),
        );
    }

    #[test]
    fn trusted_runtime_protocol_inlining_waits_for_constructor_then_prefers_trusted_calls() {
        let constructor_source = InstrId::new(1);
        let trusted_protocol_source = InstrId::new(2);
        let untrusted_protocol_source = InstrId::new(3);
        let unrelated_trusted_protocol_source = InstrId::new(4);
        let target = RuntimeFunctionId::new(RuntimeModuleId::new(4), LocalFunctionId::new(5));
        let arg_plan = TypedDirectCallArgPlan {
            sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
        };
        let inline_targets = HashMap::from([
            (constructor_source, vec![(target, arg_plan.clone())]),
            (trusted_protocol_source, vec![(target, arg_plan.clone())]),
            (untrusted_protocol_source, vec![(target, arg_plan.clone())]),
            (
                unrelated_trusted_protocol_source,
                vec![(target, arg_plan.clone())],
            ),
        ]);
        let runtime_protocol_calls = HashSet::from([
            trusted_protocol_source,
            untrusted_protocol_source,
            unrelated_trusted_protocol_source,
        ]);
        let trusted_owner = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "IterRange".to_string(),
        };
        let unrelated_trusted_owner = TypedAttrOwnerRef::TypeKey {
            module_name: "soac.runtime".to_string(),
            qualname: "ClosureGenerator".to_string(),
        };
        let trusted_constructor_calls =
            HashMap::from([(constructor_source, trusted_owner.clone())]);
        let live_instr_ids = HashSet::from([
            constructor_source,
            trusted_protocol_source,
            untrusted_protocol_source,
            unrelated_trusted_protocol_source,
        ]);

        assert_eq!(
            staged_inline_targets_for_trusted_runtime_protocols(
                inline_targets.clone(),
                &runtime_protocol_calls,
                &HashMap::from([
                    (trusted_protocol_source, trusted_owner.clone()),
                    (
                        unrelated_trusted_protocol_source,
                        unrelated_trusted_owner.clone(),
                    ),
                ]),
                Some(&trusted_constructor_calls),
                None,
                &live_instr_ids,
            ),
            HashMap::from([
                (constructor_source, vec![(target, arg_plan.clone())]),
                (
                    unrelated_trusted_protocol_source,
                    vec![(target, arg_plan.clone())],
                ),
            ]),
            "pending trusted constructors should only delay protocol calls for the same owner"
        );

        let constructor_field_bindings = HashMap::from([(
            constructor_source,
            TypedConstructorFieldBindings { fields: Vec::new() },
        )]);
        assert_eq!(
            staged_inline_targets_for_trusted_runtime_protocols(
                inline_targets,
                &runtime_protocol_calls,
                &HashMap::from([
                    (trusted_protocol_source, trusted_owner),
                    (unrelated_trusted_protocol_source, unrelated_trusted_owner,),
                ]),
                Some(&trusted_constructor_calls),
                Some(&constructor_field_bindings),
                &live_instr_ids,
            ),
            HashMap::from([
                (constructor_source, vec![(target, arg_plan.clone())]),
                (trusted_protocol_source, vec![(target, arg_plan.clone())]),
                (unrelated_trusted_protocol_source, vec![(target, arg_plan)]),
            ]),
            "once trusted protocol calls are available, weaker protocol calls should wait"
        );
    }

    #[test]
    fn inlined_bodies_only_propagate_small_profile_or_static_nested_inline_targets() {
        let mut large_body = String::new();
        for index in 0..40 {
            large_body.push_str(&format!("    value_{index} = x\n"));
        }
        large_body.push_str("    return value_39\n");
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(&format!(
            "def outer(x):\n    return x\n\n\
def small(x):\n    return x\n\n\
def large(x):\n{large_body}"
        ))
        .expect("source should lower");
        let typed = lower_blockpy_module_to_typed(lowered.blockpy_module);
        let callee_id = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "outer")
            .expect("outer should exist")
            .function_id;
        let small_target_id = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "small")
            .expect("small should exist")
            .function_id;
        let large_target_id = typed
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == "large")
            .expect("large should exist")
            .function_id;
        let caller_id = RuntimeFunctionId::new(RuntimeModuleId::new(4), LocalFunctionId::new(5));
        let small_callee_instr_id = InstrId::new(11);
        let large_callee_instr_id = InstrId::new(12);
        let small_caller_instr_id = InstrId::new(13);
        let large_caller_instr_id = InstrId::new(14);
        let arg_plan = TypedDirectCallArgPlan {
            sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
        };
        let nested_inline = |source, target| ResolvedV3DirectCallPlan {
            source,
            target,
            callee: DirectCallCallee::Function,
            arg_plan: arg_plan.clone(),
            body: CallBodyPlan {
                kind: CallBodyKind::Inline,
                cost: Cost::default(),
                inline_target: None,
                reason: "nested inline".to_string(),
            },
            reason: "nested direct call".to_string(),
        };
        let mappings = [
            TypedInlineInstrIdMapping {
                callee: callee_id,
                inline_instance: 0,
                callee_instr_id: small_callee_instr_id,
                caller_instr_id: small_caller_instr_id,
            },
            TypedInlineInstrIdMapping {
                callee: callee_id,
                inline_instance: 0,
                callee_instr_id: large_callee_instr_id,
                caller_instr_id: large_caller_instr_id,
            },
        ];
        let profile = SpecializationProfile {
            module_name: None,
            counter_dump_path: None,
            direct_call_emission_scope: DirectCallEmissionScope::AllDirectCallCandidates,
            opt_v3_emitted_direct_calls: HashMap::from([(
                callee_id,
                HashMap::from([
                    (
                        small_callee_instr_id,
                        vec![nested_inline(small_callee_instr_id, small_target_id)],
                    ),
                    (
                        large_callee_instr_id,
                        vec![nested_inline(large_callee_instr_id, large_target_id)],
                    ),
                ]),
            )]),
            opt_v3_emitted_exact_list_items: HashMap::new(),
            opt_v3_emitted_indexed_fields: HashMap::new(),
            opt_v3_emitted_indexed_globals: HashMap::new(),
            opt_v3_exact_int_branch_artifacts: HashMap::new(),
            behavior_change_indexed_stores: false,
            profiled_cold_blocks: false,
            guard_miss_deopt: false,
        };

        let mut remapped_inline_targets = HashMap::new();
        assert_eq!(
            remap_inlined_direct_call_targets(
                caller_id,
                &mappings,
                &typed,
                &HashMap::new(),
                &profile,
                &HashSet::new(),
                &HashMap::new(),
                &mut remapped_inline_targets,
                &SuppressedTypedInlineTargets::new(),
            ),
            1,
            "only small profile-driven nested inline choices should propagate through another inline"
        );
        assert_eq!(
            remapped_inline_targets
                .get(&caller_id)
                .and_then(|targets| targets.get(&small_caller_instr_id)),
            Some(&vec![(small_target_id, arg_plan.clone())]),
        );
        assert!(
            remapped_inline_targets
                .get(&caller_id)
                .is_none_or(|targets| !targets.contains_key(&large_caller_instr_id)),
            "large profile-driven nested inline bodies should stay caller-local"
        );

        let mut remapped_inline_targets = HashMap::new();
        assert_eq!(
            remap_inlined_direct_call_targets(
                caller_id,
                &mappings,
                &typed,
                &HashMap::new(),
                &profile,
                &HashSet::new(),
                &HashMap::from([(
                    callee_id,
                    HashMap::from([(
                        large_callee_instr_id,
                        vec![nested_inline(large_callee_instr_id, large_target_id)],
                    )])
                )]),
                &mut remapped_inline_targets,
                &SuppressedTypedInlineTargets::new(),
            ),
            2,
            "structurally static nested inline targets should keep propagating alongside small profile targets"
        );
        assert_eq!(
            remapped_inline_targets
                .get(&caller_id)
                .and_then(|targets| targets.get(&large_caller_instr_id)),
            Some(&vec![(large_target_id, arg_plan)]),
        );

        let mut remapped_inline_targets = HashMap::new();
        assert_eq!(
            remap_inlined_direct_call_targets(
                caller_id,
                &mappings,
                &typed,
                &HashMap::new(),
                &profile,
                &HashSet::from([large_target_id]),
                &HashMap::new(),
                &mut remapped_inline_targets,
                &SuppressedTypedInlineTargets::new(),
            ),
            2,
            "trusted generator bridge targets should survive one nested profile-inline hop"
        );
        assert_eq!(
            remapped_inline_targets
                .get(&caller_id)
                .and_then(|targets| targets.get(&large_caller_instr_id)),
            Some(&vec![(
                large_target_id,
                TypedDirectCallArgPlan {
                    sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
                },
            )]),
        );
    }

    #[test]
    fn cloned_hot_continuations_retire_original_inline_targets() {
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let target_id = RuntimeFunctionId::new(RuntimeModuleId::new(5), LocalFunctionId::new(6));
        let arg_plan = TypedDirectCallArgPlan {
            sources: vec![soac_ir_typed::TypedDirectCallArgSource::Provided(0)],
        };
        let mut remapped_inline_targets = HashMap::from([(
            function_id,
            HashMap::from([
                (InstrId::new(7), vec![(target_id, arg_plan.clone())]),
                (InstrId::new(9), vec![(target_id, arg_plan)]),
            ]),
        )]);
        let mut suppressed_inline_targets = SuppressedTypedInlineTargets::new();
        let mappings = [TypedInlineInstrIdMapping {
            callee: function_id,
            inline_instance: 0,
            callee_instr_id: InstrId::new(7),
            caller_instr_id: InstrId::new(9),
        }];

        assert_eq!(
            retire_cloned_inline_targets(
                function_id,
                &mappings,
                &mut remapped_inline_targets,
                &mut suppressed_inline_targets,
            ),
            1,
        );
        assert!(
            suppressed_inline_targets
                .get(&function_id)
                .is_some_and(|targets| targets.contains(&InstrId::new(7))),
            "the original cloned-away inline site should become cold-only"
        );
        let targets = remapped_inline_targets
            .get(&function_id)
            .expect("function inline targets should remain available");
        assert!(
            !targets.contains_key(&InstrId::new(7)),
            "the old cold copy should not keep its remapped inline target"
        );
        assert!(
            targets.contains_key(&InstrId::new(9)),
            "the new hot clone should retain the moved inline target"
        );
    }

    #[test]
    fn cloned_hot_continuations_remap_generator_state_lowering_ids_by_instance() {
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let generator_function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(5), LocalFunctionId::new(6));
        let source_origin = InstrId::new(7);
        let source_body = InstrId::new(8);
        let cloned_origin = InstrId::new(9);
        let cloned_body = InstrId::new(10);
        let transitive_origin = InstrId::new(11);
        let transitive_body = InstrId::new(12);
        let mappings = [
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 0,
                callee_instr_id: source_origin,
                caller_instr_id: cloned_origin,
            },
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 0,
                callee_instr_id: source_body,
                caller_instr_id: cloned_body,
            },
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 1,
                callee_instr_id: cloned_origin,
                caller_instr_id: transitive_origin,
            },
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 1,
                callee_instr_id: cloned_body,
                caller_instr_id: transitive_body,
            },
        ];
        let mut instr_ids_by_origin = HashMap::from([(
            source_origin,
            (generator_function_id, HashSet::from([source_body])),
        )]);

        assert!(
            remap_cloned_generator_state_lowering_instr_ids(
                function_id,
                &mappings,
                &mut instr_ids_by_origin,
            )
            .expect("generator-state clone remap should succeed")
                > 0
        );
        assert_eq!(
            instr_ids_by_origin
                .get(&source_origin)
                .map(|(_, instr_ids)| instr_ids),
            Some(&HashSet::from([source_body])),
        );
        assert_eq!(
            instr_ids_by_origin
                .get(&cloned_origin)
                .map(|(_, instr_ids)| instr_ids),
            Some(&HashSet::from([cloned_body])),
        );
        assert_eq!(
            instr_ids_by_origin
                .get(&transitive_origin)
                .map(|(_, instr_ids)| instr_ids),
            Some(&HashSet::from([transitive_body])),
        );
    }

    #[test]
    fn cloned_generator_state_body_ids_do_not_cross_sibling_instances() {
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let generator_function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(5), LocalFunctionId::new(6));
        let source_origin = InstrId::new(7);
        let source_body = InstrId::new(8);
        let first_origin = InstrId::new(9);
        let first_body = InstrId::new(10);
        let second_origin = InstrId::new(11);
        let second_body = InstrId::new(12);
        let mappings = [
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 0,
                callee_instr_id: source_origin,
                caller_instr_id: first_origin,
            },
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 0,
                callee_instr_id: source_body,
                caller_instr_id: first_body,
            },
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 1,
                callee_instr_id: source_origin,
                caller_instr_id: second_origin,
            },
            TypedInlineInstrIdMapping {
                callee: function_id,
                inline_instance: 1,
                callee_instr_id: source_body,
                caller_instr_id: second_body,
            },
        ];
        let mut instr_ids_by_origin = HashMap::from([(
            source_origin,
            (generator_function_id, HashSet::from([source_body])),
        )]);

        remap_cloned_generator_state_lowering_instr_ids(
            function_id,
            &mappings,
            &mut instr_ids_by_origin,
        )
        .expect("sibling generator-state clone remap should succeed");

        assert_eq!(
            instr_ids_by_origin
                .get(&source_origin)
                .map(|(_, instr_ids)| instr_ids),
            Some(&HashSet::from([source_body])),
        );
        assert_eq!(
            instr_ids_by_origin
                .get(&first_origin)
                .map(|(_, instr_ids)| instr_ids),
            Some(&HashSet::from([first_body])),
        );
        assert_eq!(
            instr_ids_by_origin
                .get(&second_origin)
                .map(|(_, instr_ids)| instr_ids),
            Some(&HashSet::from([second_body])),
        );
    }

    #[test]
    fn cloned_hot_continuations_remap_exact_int_sidecars() {
        let function_id = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let source_instr_id = InstrId::new(7);
        let cloned_instr_id = InstrId::new(9);
        let region_plan = RegionPlan {
            id: soac_ir_typed::plan_v3::RegionId(0),
            source: RegionSource::Instr {
                instr_id: source_instr_id,
            },
            inputs: Vec::new(),
            nodes: Vec::new(),
            exits: Vec::new(),
        };
        let region_emission = MechanicalRegionEmission {
            region: soac_ir_typed::plan_v3::RegionId(0),
            steps: Vec::new(),
            exits: Vec::new(),
        };
        let mut remapped_branches = HashMap::from([(
            function_id,
            HashMap::from([(
                source_instr_id,
                TypedExactIntBranchPlan {
                    source: TypedExactIntPlanSource::OptimizationPlanV3,
                    instr_id: source_instr_id,
                    hot_plan: region_plan.clone(),
                    hot_region: region_emission.clone(),
                    fallback_plan: region_plan.clone(),
                    fallback_region: region_emission.clone(),
                },
            )]),
        )]);
        let mut remapped_returns = HashMap::from([(
            function_id,
            HashMap::from([(
                source_instr_id,
                TypedExactIntReturnPlan {
                    source: TypedExactIntPlanSource::OptimizationPlanV3,
                    instr_id: source_instr_id,
                    hot_plan: region_plan.clone(),
                    hot_region: region_emission.clone(),
                    fallback_plan: region_plan,
                    fallback_region: region_emission,
                },
            )]),
        )]);
        let mappings = [TypedInlineInstrIdMapping {
            callee: function_id,
            inline_instance: 0,
            callee_instr_id: source_instr_id,
            caller_instr_id: cloned_instr_id,
        }];

        assert_eq!(
            remap_cloned_exact_int_selections(
                function_id,
                &mappings,
                &mut remapped_branches,
                &mut remapped_returns,
            )
            .expect("same-function clone remap should succeed"),
            2,
        );
        let branch_plan = remapped_branches
            .get(&function_id)
            .and_then(|plans| plans.get(&cloned_instr_id))
            .expect("cloned branch plan should be preserved");
        assert_eq!(branch_plan.instr_id, cloned_instr_id);
        assert_eq!(
            branch_plan.hot_plan.source,
            RegionSource::Instr {
                instr_id: cloned_instr_id,
            }
        );
        let return_plan = remapped_returns
            .get(&function_id)
            .and_then(|plans| plans.get(&cloned_instr_id))
            .expect("cloned return plan should be preserved");
        assert_eq!(return_plan.instr_id, cloned_instr_id);
        assert_eq!(
            return_plan.hot_plan.source,
            RegionSource::Instr {
                instr_id: cloned_instr_id,
            }
        );
    }

    #[test]
    fn inlined_exact_int_sidecars_remap_module_constant_indices() {
        let callee = RuntimeFunctionId::new(RuntimeModuleId::new(3), LocalFunctionId::new(4));
        let contexts = typed_inline_exact_int_remap_contexts(
            &[],
            &[TypedInlineConstantMapping {
                callee,
                inline_instance: 0,
                callee_index: 84,
                caller_index: 34,
            }],
            &[],
        )
        .expect("constant-only inline remap context should build");
        let context = contexts
            .get(&(callee, 0))
            .expect("constant-only inline remap context should be recorded");
        let region = RegionPlan {
            id: soac_ir_typed::plan_v3::RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![soac_ir_typed::plan_v3::RegionInput {
                value: soac_ir_typed::plan_v3::PlanValue::new(
                    1,
                    soac_ir_typed::plan_v3::Rep::PyObjectBorrowed,
                ),
                source: RegionInputSource::ModuleConstant { index: 84 },
            }],
            nodes: Vec::new(),
            exits: Vec::new(),
        };

        let remapped = remap_exact_int_region_plan(&region, context)
            .expect("module-constant exact-int sidecar remap should succeed");

        assert_eq!(
            remapped.inputs[0].source,
            RegionInputSource::ModuleConstant { index: 34 },
        );
    }

    #[test]
    fn virtualized_exact_int_inputs_remap_to_scalar_locals() {
        let field_source = InstrId::new(7);
        let object = soac_opt::passes::TypedVirtualObjectId(11);
        let receiver = ResolvedName {
            id: "iter".to_string().into(),
            location: NameLocation::Local(LocalLocation(0)),
        };
        let scalar = ResolvedName {
            id: "scalar_stop".to_string().into(),
            location: NameLocation::Local(LocalLocation(1)),
        };
        let mut state = TypedVirtualState::default();
        state.aliases.insert(LocalLocation(0), object);
        state.fields.insert(
            TypedVirtualFieldRef {
                object,
                field_name: "stop".to_string(),
            },
            scalar.clone(),
        );
        let mut region = RegionPlan {
            id: soac_ir_typed::plan_v3::RegionId(0),
            source: RegionSource::Instr {
                instr_id: field_source,
            },
            inputs: vec![soac_ir_typed::plan_v3::RegionInput {
                value: soac_ir_typed::plan_v3::PlanValue::new(
                    3,
                    soac_ir_typed::plan_v3::Rep::PyObjectBorrowed,
                ),
                source: RegionInputSource::IndexedField {
                    source: field_source,
                    receiver: IndexedFieldReceiverSource::LocalName {
                        name: receiver.id_str().to_string(),
                    },
                    owner_type: soac_ir_typed::plan_v3::IndexedFieldOwnerType {
                        module_name: "runtime".to_string(),
                        qualname: "IterRange".to_string(),
                    },
                    attr_name: "stop".to_string(),
                    expected_index: 0,
                },
            }],
            nodes: Vec::new(),
            exits: Vec::new(),
        };

        assert_eq!(
            remap_virtualized_exact_int_region_inputs_to_scalar_locals(
                &mut region,
                &state,
                &HashMap::from([(receiver.id_str().to_string(), LocalLocation(0))]),
            ),
            1,
        );
        assert_eq!(
            region.inputs[0].source,
            RegionInputSource::FunctionParam {
                index: soac_ir_typed::plan_v3::PlanValueId(3).0,
                name: Some(scalar.id_str().to_string()),
            }
        );
    }
}
