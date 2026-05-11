use super::operation_specializations::OptV3ResolvedIndexedFieldAccess;
use super::planning::{
    PlannedJitDeoptResumeModule, PlannedJitModuleLocals, PreparedJitTypedModulePlan,
    plan_jit_typed_module,
};
use super::{SpecializationProfile, annotate_typed_profiled_cold_blocks};
use crate::module_constants::ModuleCodegenConstants;
use crate::module_type::SharedModuleState;
use soac_config::SoacEnvConfig;
use soac_config::SpecializationMode;
use soac_core::block_py::{
    BlockArg, BlockLabel, BlockParamRole, BlockPyFunction, BlockPyModule, BlockTerm,
    CallableScopeKind, ChildVisitable, ConstantExpr, FunctionKind, HasSemanticInstrId, InstrId,
    InstrLocationMap, Literal, LocalLocation, NameLike, RuntimeFunctionId, RuntimeName, Visit,
    VisitMut, current_instr_locations,
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
    TypedBlockPyModuleShape, TypedCallEmissionPlan, TypedCallEmissionPlans,
    TypedConstructorInitPlan, TypedConstructorInitPlanSource, TypedDirectCallArgPlan,
    TypedDirectFunctionCallGuard, TypedDirectMethodCallGuard, TypedExactIntBranchPlan,
    TypedExactIntPlanSource, TypedExactIntReturnPlan, TypedExactListItemAccessPlan,
    TypedExactListItemCounterSource, TypedExactListItemPlanSource, TypedGeneratorInstancePlan,
    TypedIndexedFieldCounterSource, TypedIndexedFieldPlanSource, TypedIndexedGlobalAccessPlan,
    TypedIndexedGlobalPlanSource, assign_missing_typed_function_instr_ids,
};
use soac_opt::access_emission_v3::{
    ExactListItemAccessPlan as OptV3ExactListItemAccessPlan,
    IndexedGlobalAccessPlan as OptV3IndexedGlobalAccessPlan,
};
use soac_opt::artifacts_v3::ExactIntBranchV3Artifacts;
use soac_opt::call_emission_v3::{ResolvedV3DirectCallPlan, typed_call_emission_plans_from_v3};
#[cfg(test)]
use soac_opt::passes::TypedVirtualObjectId;
use soac_opt::passes::{
    TypedConstructorFieldBindings, TypedExternalInlineCallee, TypedInlineInstrIdMapping,
    TypedInlineLocalMapping, TypedVirtualBodyInstr, TypedVirtualFieldRef,
    TypedVirtualFieldStateAnalysis, TypedVirtualState, TypedVirtualizationPlan,
    inline_typed_constructor_init_bodies_with_external_callees,
    inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls,
    lower_typed_fully_virtual_objects_to_locals_with_plan,
    lower_typed_function_call_emission_plans, lower_typed_virtual_objects_to_locals_with_plan,
    plan_module_inlining, plan_typed_fully_virtual_objects, plan_typed_virtual_objects,
    refresh_typed_function_value_facts, rewrite_typed_stop_iteration_raises_to_handler_jumps,
    simplify_typed_virtual_tuple_ops, split_typed_alias_hot_continuations_with_budget,
    split_typed_constructor_hot_continuations_with_budget,
    split_typed_inline_cleanup_hot_continuations_for_labels_with_budget, summarize_module_escapes,
    typed_constructor_field_bindings_from_inline_stats_with_external_callees,
    typed_constructor_init_plans_from_inline_stats_with_external_callees,
    validate_typed_function_value_facts,
};
use soac_opt::region_emission_v3::{
    ExactIntBranchSelection as OptV3ExactIntBranchSelection,
    ExactIntReturnSelection as OptV3ExactIntReturnSelection,
    exact_int_branch_selection_for_source as opt_v3_exact_int_branch_selection_for_source,
    exact_int_return_selection_for_source as opt_v3_exact_int_return_selection_for_source,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

const MAX_TYPED_INLINE_PASSES: usize = 16;
const MAX_TRANSITIVE_PROFILE_INLINE_BLOCKS: usize = 8;
const MAX_TRANSITIVE_PROFILE_INLINE_BODY_INSTRS: usize = 32;
const MAX_TYPED_CONSTRUCTOR_CLONED_BLOCKS_PER_FUNCTION: usize = 256;
const MAX_TYPED_ALIAS_CLONED_BLOCKS_PER_FUNCTION: usize = 256;
const MAX_TYPED_INLINE_CLEANUP_CLONED_BLOCKS_PER_FUNCTION: usize = 256;

type TypedInlineTargets = HashMap<InstrId, Vec<(RuntimeFunctionId, TypedDirectCallArgPlan)>>;
type StaticTypedDirectCalls =
    HashMap<RuntimeFunctionId, HashMap<InstrId, Vec<ResolvedV3DirectCallPlan>>>;
type SuppressedTypedInlineTargets = HashMap<RuntimeFunctionId, HashSet<InstrId>>;
type RemappedTypedCallEmissions = HashMap<RuntimeFunctionId, TypedCallEmissionPlans>;

#[derive(Clone)]
struct StaticDirectCallTarget {
    function: BlockPyFunction<BlockPyModuleShape>,
    constructor_owner_type_ref: Option<TypedAttrOwnerRef>,
}

type StaticRuntimeDirectCallTargets = HashMap<RuntimeName, StaticDirectCallTarget>;
type StaticModuleGlobalDirectCallTargets = HashMap<u32, HashMap<String, StaticDirectCallTarget>>;
type StaticModuleGlobalGeneratorTargets =
    HashMap<u32, HashMap<String, BlockPyFunction<BlockPyModuleShape>>>;
type StaticStrictMethodTargets =
    HashMap<(String, String, String), BlockPyFunction<BlockPyModuleShape>>;

#[derive(Clone, Default)]
struct StaticDirectCallTargets {
    runtime_names: StaticRuntimeDirectCallTargets,
    module_globals: StaticModuleGlobalDirectCallTargets,
    module_global_generators: StaticModuleGlobalGeneratorTargets,
    strict_methods: StaticStrictMethodTargets,
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

fn trusted_fully_virtual_constructor_owner(owner: &TypedAttrOwnerRef) -> bool {
    matches!(
        owner,
        TypedAttrOwnerRef::TypeKey {
            module_name,
            qualname,
        } if module_name == "soac.runtime"
            && matches!(
                qualname.as_str(),
                "range" | "IterRange" | "ClosureGenerator" | "ClosureAsyncGenerator"
            )
    )
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
) -> Result<StaticDirectCallTargets, String> {
    let mut module_globals = HashMap::new();
    let mut module_global_generators = HashMap::new();
    let mut strict_methods = HashMap::new();
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
        strict_module_global_generator_targets_for_module(&shared_state.lowered_module),
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
                strict_module_global_generator_targets_for_module(&state.lowered_module)
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
    Ok(StaticDirectCallTargets {
        runtime_names,
        module_globals,
        module_global_generators,
        strict_methods,
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

fn static_generator_instance_plans_for_module(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    targets: &StaticDirectCallTargets,
) -> HashMap<RuntimeFunctionId, HashMap<InstrId, TypedGeneratorInstancePlan>> {
    module
        .callable_defs
        .iter()
        .filter_map(|function| {
            let plans = static_generator_instance_plans_for_function(function, targets);
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
                call.extra.set_generator_instance_plan(plan.clone());
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
            "typed generator-instance plans were not attached to call nodes: {missing}"
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
    local_names: HashMap<String, String>,
}

fn typed_inline_exact_int_remap_contexts(
    instr_mappings: &[TypedInlineInstrIdMapping],
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
            RegionInputSource::ModuleConstant { .. }
            | RegionInputSource::CapturedValue { .. }
            | RegionInputSource::Synthetic { .. } => {}
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
    local_mappings: &[TypedInlineLocalMapping],
    profile: &SpecializationProfile<'_>,
    remapped_branches: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntBranchPlan>>,
    remapped_returns: &mut HashMap<RuntimeFunctionId, HashMap<InstrId, TypedExactIntReturnPlan>>,
) -> Result<usize, String> {
    let contexts = typed_inline_exact_int_remap_contexts(instr_mappings, local_mappings)?;
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

fn typed_inline_target_is_small_enough_to_propagate(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    target: RuntimeFunctionId,
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
    function.blocks.len() <= MAX_TRANSITIVE_PROFILE_INLINE_BLOCKS
        && counter.count <= MAX_TRANSITIVE_PROFILE_INLINE_BODY_INSTRS
}

fn transitive_profile_inline_targets_for_function(
    function_id: RuntimeFunctionId,
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
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
                    )
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrustedOwnerState {
    locals: HashMap<LocalLocation, TypedAttrOwnerRef>,
    runtime_names: HashMap<LocalLocation, RuntimeName>,
    object_origins: HashMap<LocalLocation, InstrId>,
    local_functions: HashMap<LocalLocation, RuntimeFunctionId>,
    function_fields: HashMap<(InstrId, String), RuntimeFunctionId>,
    escaped_origins: HashSet<InstrId>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TrustedOwnerStateAnalysis {
    body_before_instr: HashMap<TypedVirtualBodyInstr, TrustedOwnerState>,
    block_before_term: HashMap<BlockLabel, TrustedOwnerState>,
}

fn trusted_owner_state_for_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> Option<TypedAttrOwnerRef> {
    if let Some(plan) = value.generator_instance_plan()
        && let Some(owner_type_ref) = trusted_generator_instance_owner(plan)
    {
        return Some(owner_type_ref);
    }
    if let Some(instr_id) = value.try_semantic_instr_id()
        && let Some(owner_type_ref) = trusted_constructor_calls.get(&instr_id)
    {
        return Some(owner_type_ref.clone());
    }
    if let Some(owner_type_ref) = trusted_materialized_constructor_owner(
        value,
        state,
        module_constants,
        trusted_constructor_init_owners,
    ) {
        return Some(owner_type_ref);
    }
    let InstrTyped::Load(load) = value else {
        return None;
    };
    state.locals.get(&load.name.local_location()?).cloned()
}

fn trusted_object_origin_for_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
) -> Option<InstrId> {
    if value.generator_instance_plan().is_some() {
        return value.try_semantic_instr_id();
    }
    if let Some(instr_id) = value.try_semantic_instr_id()
        && trusted_constructor_calls.contains_key(&instr_id)
    {
        return Some(instr_id);
    }
    let InstrTyped::Load(load) = value else {
        return None;
    };
    state
        .object_origins
        .get(&load.name.local_location()?)
        .copied()
}

fn trusted_function_id_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
) -> Option<RuntimeFunctionId> {
    match expr {
        InstrTyped::MakeFunctionWithClosure(op) => Some(op.function_id),
        InstrTyped::Load(load) => state
            .local_functions
            .get(&load.name.local_location()?)
            .copied(),
        _ => None,
    }
}

fn trusted_generator_instance_owner(
    plan: &TypedGeneratorInstancePlan,
) -> Option<TypedAttrOwnerRef> {
    let qualname = match plan.kind {
        FunctionKind::Generator => "ClosureGenerator",
        FunctionKind::Coroutine => "Coroutine",
        FunctionKind::AsyncGenerator => "ClosureAsyncGenerator",
        FunctionKind::Function => return None,
    };
    Some(TypedAttrOwnerRef::TypeKey {
        module_name: "soac.runtime".to_string(),
        qualname: qualname.to_string(),
    })
}

fn trusted_object_origins_in_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
) -> HashSet<InstrId> {
    struct Collector<'a> {
        state: &'a TrustedOwnerState,
        origins: HashSet<InstrId>,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::Load(load) = expr
                && let Some(location) = load.name.local_location()
                && let Some(origin) = self.state.object_origins.get(&location)
            {
                self.origins.insert(*origin);
            }
            expr.visit_children(self);
        }
    }

    let mut collector = Collector {
        state,
        origins: HashSet::new(),
    };
    collector.visit_instr(expr);
    collector.origins
}

fn mark_trusted_owner_escapes_for_instr(instr: &InstrTyped, state: &mut TrustedOwnerState) {
    let escaped = match instr {
        InstrTyped::Store(store)
            if store.name.local_location().is_some()
                && matches!(store.value.as_ref(), InstrTyped::Load(_)) =>
        {
            HashSet::new()
        }
        InstrTyped::Store(store) => trusted_object_origins_in_expr(store.value.as_ref(), state),
        InstrTyped::SetAttrTyped(op) => {
            trusted_object_origins_in_expr(op.replacement.as_ref(), state)
        }
        _ => trusted_object_origins_in_expr(instr, state),
    };
    state.escaped_origins.extend(escaped);
}

fn trusted_runtime_name_for_expr(
    expr: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<RuntimeName> {
    if let InstrTyped::Load(load) = expr {
        if let Some(runtime_name) = load.name.runtime_name_id() {
            return Some(runtime_name);
        }
        if let Some(location) = load.name.local_location()
            && let Some(runtime_name) = state.runtime_names.get(&location)
        {
            return Some(*runtime_name);
        }
        if let Some(index) = load.name.location.as_constant()
            && let Some(ConstantExpr::RuntimeName(runtime_name)) =
                module_constants.get(index as usize)
        {
            return Some(*runtime_name);
        }
    }
    typed_expr_runtime_name_provenance(expr, module_constants)
}

fn trusted_materialized_constructor_owner(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> Option<TypedAttrOwnerRef> {
    let (func, args, init_plan) = match value {
        InstrTyped::CallTyped(call) => (
            call.func.as_ref(),
            call.args.as_slice(),
            call.extra.constructor_init_plan(),
        ),
        InstrTyped::DirectCallableCallTyped(call) => (
            call.func.as_ref(),
            call.args.as_slice(),
            call.extra.constructor_init_plan(),
        ),
        _ => return None,
    };
    if let Some(init_plan) = init_plan
        && let Some(owner_type_ref) =
            trusted_constructor_init_owners.get(&init_plan.init_function_id)
    {
        return Some(owner_type_ref.clone());
    }
    if init_plan.is_none()
        && !typed_expr_is_runtime_name_load(func, RuntimeName::ConstructorCall, module_constants)
    {
        return None;
    }
    let soac_core::block_py::CallArgPositional::Positional(class_expr) = args.first()? else {
        return None;
    };
    let runtime_name = trusted_runtime_name_for_expr(class_expr, state, module_constants)?;
    let owner_type_ref = TypedAttrOwnerRef::TypeKey {
        module_name: "soac.runtime".to_string(),
        qualname: runtime_name.name().to_string(),
    };
    trusted_fully_virtual_constructor_owner(&owner_type_ref).then_some(owner_type_ref)
}

fn trusted_runtime_name_for_store_value(
    value: &InstrTyped,
    state: &TrustedOwnerState,
    module_constants: &[ConstantExpr],
) -> Option<RuntimeName> {
    trusted_runtime_name_for_expr(value, state, module_constants)
}

fn transfer_trusted_owner_instr(
    instr: &InstrTyped,
    state: &mut TrustedOwnerState,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) {
    mark_trusted_owner_escapes_for_instr(instr, state);
    match instr {
        InstrTyped::Store(store) => {
            let Some(location) = store.name.local_location() else {
                return;
            };
            if let Some(owner_type_ref) = trusted_owner_state_for_store_value(
                store.value.as_ref(),
                state,
                module_constants,
                trusted_constructor_calls,
                trusted_constructor_init_owners,
            ) {
                state.locals.insert(location, owner_type_ref);
            } else {
                state.locals.remove(&location);
            }
            if let Some(runtime_name) =
                trusted_runtime_name_for_store_value(store.value.as_ref(), state, module_constants)
            {
                state.runtime_names.insert(location, runtime_name);
            } else {
                state.runtime_names.remove(&location);
            }
            if let Some(origin) = trusted_object_origin_for_store_value(
                store.value.as_ref(),
                state,
                trusted_constructor_calls,
            ) {
                state.object_origins.insert(location, origin);
                if store.value.try_semantic_instr_id() == Some(origin) {
                    state.escaped_origins.remove(&origin);
                }
            } else {
                state.object_origins.remove(&location);
            }
            if let Some(function_id) = trusted_function_id_for_expr(store.value.as_ref(), state) {
                state.local_functions.insert(location, function_id);
            } else {
                state.local_functions.remove(&location);
            }
            if let Some(origin) = store.value.try_semantic_instr_id()
                && let Some(plan) = store.value.generator_instance_plan()
            {
                state
                    .function_fields
                    .insert((origin, "_resume_function".to_string()), plan.function_id);
            }
        }
        InstrTyped::Del(del) => {
            if let Some(location) = del.name.local_location() {
                state.locals.remove(&location);
                state.runtime_names.remove(&location);
                state.object_origins.remove(&location);
                state.local_functions.remove(&location);
            }
        }
        InstrTyped::SetAttrTyped(op) => {
            let Some(field_name) = typed_constant_string(op.attr.as_ref(), module_constants) else {
                return;
            };
            let InstrTyped::Load(receiver) = op.value.as_ref() else {
                return;
            };
            let Some(receiver_location) = receiver.name.local_location() else {
                return;
            };
            let Some(origin) = state.object_origins.get(&receiver_location).copied() else {
                return;
            };
            let key = (origin, field_name.to_string());
            if let Some(function_id) = trusted_function_id_for_expr(op.replacement.as_ref(), state)
            {
                state.function_fields.insert(key, function_id);
            } else {
                state.function_fields.remove(&key);
            }
        }
        _ => {}
    }
}

fn merge_trusted_owner_states(states: &[TrustedOwnerState]) -> TrustedOwnerState {
    let Some(first) = states.first() else {
        return TrustedOwnerState::default();
    };
    let locals = first
        .locals
        .iter()
        .filter(|(location, owner)| {
            states
                .iter()
                .all(|state| state.locals.get(location) == Some(*owner))
        })
        .map(|(location, owner)| (*location, owner.clone()))
        .collect();
    let runtime_names = first
        .runtime_names
        .iter()
        .filter(|(location, runtime_name)| {
            states
                .iter()
                .all(|state| state.runtime_names.get(location) == Some(*runtime_name))
        })
        .map(|(location, runtime_name)| (*location, *runtime_name))
        .collect();
    let object_origins = first
        .object_origins
        .iter()
        .filter(|(location, origin)| {
            states
                .iter()
                .all(|state| state.object_origins.get(location) == Some(*origin))
        })
        .map(|(location, origin)| (*location, *origin))
        .collect();
    let local_functions = first
        .local_functions
        .iter()
        .filter(|(location, function_id)| {
            states
                .iter()
                .all(|state| state.local_functions.get(location) == Some(*function_id))
        })
        .map(|(location, function_id)| (*location, *function_id))
        .collect();
    let function_fields = first
        .function_fields
        .iter()
        .filter(|(field, function_id)| {
            states
                .iter()
                .all(|state| state.function_fields.get(field) == Some(*function_id))
        })
        .map(|(field, function_id)| (field.clone(), *function_id))
        .collect();
    let escaped_origins = states
        .iter()
        .flat_map(|state| state.escaped_origins.iter().copied())
        .collect();
    TrustedOwnerState {
        locals,
        runtime_names,
        object_origins,
        local_functions,
        function_fields,
        escaped_origins,
    }
}

#[derive(Clone, Debug)]
struct TrustedOwnerPredecessorEdge {
    from: BlockLabel,
    explicit_args: Option<Vec<BlockArg>>,
}

fn trusted_owner_block_predecessor_edges(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
) -> HashMap<BlockLabel, Vec<TrustedOwnerPredecessorEdge>> {
    let mut predecessors = HashMap::<BlockLabel, Vec<TrustedOwnerPredecessorEdge>>::new();
    for block in &function.blocks {
        match &block.term {
            BlockTerm::Jump(edge) => {
                predecessors
                    .entry(edge.target)
                    .or_default()
                    .push(TrustedOwnerPredecessorEdge {
                        from: block.label,
                        explicit_args: Some(edge.args.clone()),
                    });
            }
            BlockTerm::IfTerm(if_term) => {
                for target in [if_term.then_label, if_term.else_label] {
                    predecessors
                        .entry(target)
                        .or_default()
                        .push(TrustedOwnerPredecessorEdge {
                            from: block.label,
                            explicit_args: None,
                        });
                }
            }
            BlockTerm::BranchTable(branch) => {
                for target in branch
                    .targets
                    .iter()
                    .copied()
                    .chain(std::iter::once(branch.default_label))
                {
                    predecessors
                        .entry(target)
                        .or_default()
                        .push(TrustedOwnerPredecessorEdge {
                            from: block.label,
                            explicit_args: None,
                        });
                }
            }
            BlockTerm::Raise(_) | BlockTerm::Return(_) => {}
        }
    }
    predecessors
}

fn remap_trusted_owner_state_for_edge(
    target: &soac_ir_typed::TypedBlock,
    explicit_args: Option<&[BlockArg]>,
    local_locations_by_name: &HashMap<String, LocalLocation>,
    state: &TrustedOwnerState,
) -> TrustedOwnerState {
    let Some(args) = explicit_args else {
        return state.clone();
    };
    let mut remapped = state.clone();
    for (param, arg) in target.params.iter().zip(args) {
        if param.role != BlockParamRole::Value {
            continue;
        }
        let BlockArg::Name(source_name) = arg else {
            continue;
        };
        let (Some(source), Some(target)) = (
            local_locations_by_name.get(source_name).copied(),
            local_locations_by_name.get(&param.name).copied(),
        ) else {
            continue;
        };
        match remapped.locals.get(&source).cloned() {
            Some(owner_type_ref) => {
                remapped.locals.insert(target, owner_type_ref);
            }
            None => {
                remapped.locals.remove(&target);
            }
        }
        match remapped.runtime_names.get(&source).copied() {
            Some(runtime_name) => {
                remapped.runtime_names.insert(target, runtime_name);
            }
            None => {
                remapped.runtime_names.remove(&target);
            }
        }
        match remapped.object_origins.get(&source).copied() {
            Some(origin) => {
                remapped.object_origins.insert(target, origin);
            }
            None => {
                remapped.object_origins.remove(&target);
            }
        }
        match remapped.local_functions.get(&source).copied() {
            Some(function_id) => {
                remapped.local_functions.insert(target, function_id);
            }
            None => {
                remapped.local_functions.remove(&target);
            }
        }
    }
    remapped
}

fn analyze_trusted_owner_states(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
) -> TrustedOwnerStateAnalysis {
    let labels = function
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| (block.label, index))
        .collect::<HashMap<_, _>>();
    let predecessors = trusted_owner_block_predecessor_edges(function);
    let local_locations_by_name = typed_local_locations_by_name(function);
    let entry_label = function.blocks.first().map(|block| block.label);
    let mut in_states = vec![None::<TrustedOwnerState>; function.blocks.len()];
    let mut out_states = vec![None::<TrustedOwnerState>; function.blocks.len()];

    loop {
        let mut changed = false;
        for (block_index, block) in function.blocks.iter().enumerate() {
            let in_state = if Some(block.label) == entry_label {
                Some(TrustedOwnerState::default())
            } else {
                let incoming = predecessors
                    .get(&block.label)
                    .into_iter()
                    .flatten()
                    .filter_map(|edge| {
                        let source_index = *labels.get(&edge.from)?;
                        let source_state = out_states[source_index].as_ref()?;
                        Some(remap_trusted_owner_state_for_edge(
                            block,
                            edge.explicit_args.as_deref(),
                            &local_locations_by_name,
                            source_state,
                        ))
                    })
                    .collect::<Vec<_>>();
                (!incoming.is_empty()).then(|| merge_trusted_owner_states(&incoming))
            };
            if in_states[block_index] != in_state {
                in_states[block_index] = in_state.clone();
                changed = true;
            }
            let out_state = in_state.map(|mut state| {
                for instr in &block.body {
                    transfer_trusted_owner_instr(
                        instr,
                        &mut state,
                        module_constants,
                        trusted_constructor_calls,
                        trusted_constructor_init_owners,
                    );
                }
                state
            });
            if out_states[block_index] != out_state {
                out_states[block_index] = out_state;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut analysis = TrustedOwnerStateAnalysis::default();
    for (block_index, block) in function.blocks.iter().enumerate() {
        let Some(mut state) = in_states[block_index].clone() else {
            continue;
        };
        for (instr_index, instr) in block.body.iter().enumerate() {
            analysis.body_before_instr.insert(
                TypedVirtualBodyInstr {
                    block: block.label,
                    instr_index,
                },
                state.clone(),
            );
            transfer_trusted_owner_instr(
                instr,
                &mut state,
                module_constants,
                trusted_constructor_calls,
                trusted_constructor_init_owners,
            );
        }
        analysis.block_before_term.insert(block.label, state);
    }
    analysis
}

fn visit_trusted_owner_term_instrs(
    term: &BlockTerm<InstrTyped>,
    visitor: &mut impl Visit<InstrTyped>,
) {
    match term {
        BlockTerm::IfTerm(if_term) => visitor.visit_instr(&if_term.test),
        BlockTerm::BranchTable(branch) => visitor.visit_instr(&branch.index),
        BlockTerm::Raise(raise) => {
            if let Some(exc) = &raise.exc {
                visitor.visit_instr(exc);
            }
        }
        BlockTerm::Return(value) => visitor.visit_instr(value),
        BlockTerm::Jump(_) => {}
    }
}

fn trusted_runtime_protocol_calls_from_owner_states(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
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
                && let Some(location) = load.name.local_location()
                && let Some(owner_type_ref) = self.state.locals.get(&location)
            {
                self.calls.insert(instr_id, owner_type_ref.clone());
            }
            expr.visit_children(self);
        }
    }

    let mut calls = HashMap::new();
    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
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

fn trusted_static_runtime_protocol_inlines_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    static_targets: &StaticDirectCallTargets,
) -> (HashMap<InstrId, TypedAttrOwnerRef>, TypedInlineTargets) {
    struct Collector<'a> {
        module_constants: &'a [ConstantExpr],
        state: &'a TrustedOwnerState,
        static_targets: &'a StaticDirectCallTargets,
        owners: HashMap<InstrId, TypedAttrOwnerRef>,
        inline_targets: TypedInlineTargets,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && matches!(call.access, soac_ir_typed::TypedCallAccessPlan::Generic)
                && let Some(instr_id) = call.try_semantic_instr_id()
            {
                let runtime_name =
                    [RuntimeName::Iter, RuntimeName::Next]
                        .into_iter()
                        .find_map(|runtime_name| {
                            typed_expr_is_runtime_name_load(
                                call.func.as_ref(),
                                runtime_name,
                                self.module_constants,
                            )
                            .then_some(runtime_name)
                        });
                let owner_type_ref = call
                    .args
                    .first()
                    .and_then(|arg| match arg {
                        soac_core::block_py::CallArgPositional::Positional(InstrTyped::Load(
                            load,
                        )) => load.name.local_location(),
                        _ => None,
                    })
                    .and_then(|location| self.state.locals.get(&location));
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
                    expr.visit_children(self);
                    return;
                };
                let Some(owner_type_ref) = owner_type_ref else {
                    expr.visit_children(self);
                    return;
                };
                let Some(target) = target else {
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
                self.owners.insert(instr_id, owner_type_ref.clone());
                self.inline_targets
                    .entry(instr_id)
                    .or_default()
                    .push((target.function_id, arg_plan));
            }
            expr.visit_children(self);
        }
    }

    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    let mut owners = HashMap::new();
    let mut inline_targets = HashMap::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                continue;
            };
            let mut collector = Collector {
                module_constants,
                state,
                static_targets,
                owners: HashMap::new(),
                inline_targets: HashMap::new(),
            };
            collector.visit_instr(instr);
            owners.extend(collector.owners);
            inline_targets.extend(collector.inline_targets);
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            module_constants,
            state,
            static_targets,
            owners: HashMap::new(),
            inline_targets: HashMap::new(),
        };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
        owners.extend(collector.owners);
        inline_targets.extend(collector.inline_targets);
    }
    (owners, inline_targets)
}

fn trusted_static_method_inlines_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    static_targets: &StaticDirectCallTargets,
) -> (HashMap<InstrId, TypedAttrOwnerRef>, TypedInlineTargets) {
    struct Collector<'a> {
        module_constants: &'a [ConstantExpr],
        state: &'a TrustedOwnerState,
        static_targets: &'a StaticDirectCallTargets,
        owners: HashMap<InstrId, TypedAttrOwnerRef>,
        inline_targets: TypedInlineTargets,
    }

    impl Visit<InstrTyped> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr
                && matches!(call.access, soac_ir_typed::TypedCallAccessPlan::Generic)
                && let Some(instr_id) = call.try_semantic_instr_id()
                && let InstrTyped::GetAttrTyped(get_attr) = call.func.as_ref()
                && let Some(method_name) =
                    typed_constant_string(get_attr.attr.as_ref(), self.module_constants)
                && let InstrTyped::Load(load) = get_attr.value.as_ref()
                && let Some(location) = load.name.local_location()
                && let Some(owner_type_ref) = self.state.locals.get(&location)
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
                self.owners.insert(instr_id, owner_type_ref.clone());
                self.inline_targets
                    .entry(instr_id)
                    .or_default()
                    .push((target.function_id, arg_plan));
            }
            expr.visit_children(self);
        }
    }

    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    let mut owners = HashMap::new();
    let mut inline_targets = HashMap::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                continue;
            };
            let mut collector = Collector {
                module_constants,
                state,
                static_targets,
                owners: HashMap::new(),
                inline_targets: HashMap::new(),
            };
            collector.visit_instr(instr);
            owners.extend(collector.owners);
            inline_targets.extend(collector.inline_targets);
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            module_constants,
            state,
            static_targets,
            owners: HashMap::new(),
            inline_targets: HashMap::new(),
        };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
        owners.extend(collector.owners);
        inline_targets.extend(collector.inline_targets);
    }
    (owners, inline_targets)
}

fn trusted_field_callable_inlines_for_function(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
    trusted_constructor_calls: &HashMap<InstrId, TypedAttrOwnerRef>,
    trusted_constructor_init_owners: &HashMap<RuntimeFunctionId, TypedAttrOwnerRef>,
    callee_module: &BlockPyModule<TypedBlockPyModuleShape>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
) -> Result<(TypedCallEmissionPlans, TypedInlineTargets), String> {
    struct Collector<'a> {
        module_constants: &'a [ConstantExpr],
        state: &'a TrustedOwnerState,
        callee_module: &'a BlockPyModule<TypedBlockPyModuleShape>,
        external_callees: &'a HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
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
                && let InstrTyped::GetAttrTyped(get_attr) = call.func.as_ref()
                && let Some(field_name) =
                    typed_constant_string(get_attr.attr.as_ref(), self.module_constants)
                && let InstrTyped::Load(receiver) = get_attr.value.as_ref()
                && let Some(receiver_location) = receiver.name.local_location()
                && let Some(origin) = self.state.object_origins.get(&receiver_location)
                && !self.state.escaped_origins.contains(origin)
                && let Some(target_function_id) = self
                    .state
                    .function_fields
                    .get(&(*origin, field_name.to_string()))
                    .copied()
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

    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        trusted_constructor_calls,
        trusted_constructor_init_owners,
    );
    let mut emissions = TypedCallEmissionPlans::default();
    let mut inline_targets = HashMap::new();
    for block in &function.blocks {
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                continue;
            };
            let mut collector = Collector {
                module_constants,
                state,
                callee_module,
                external_callees,
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
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        let mut collector = Collector {
            module_constants,
            state,
            callee_module,
            external_callees,
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
) -> Result<usize, String> {
    let caller_function_id = function.function_id;
    let constructor_split_stats = split_typed_constructor_hot_continuations_with_budget(
        function,
        module_constants,
        MAX_TYPED_CONSTRUCTOR_CLONED_BLOCKS_PER_FUNCTION,
    );
    remap_cloned_hot_state_cleanup_labels(
        hot_state_cleanup_labels,
        &constructor_split_stats.label_mappings,
    );
    let alias_split_stats = split_typed_alias_hot_continuations_with_budget(
        function,
        MAX_TYPED_ALIAS_CLONED_BLOCKS_PER_FUNCTION,
    );
    remap_cloned_hot_state_cleanup_labels(
        hot_state_cleanup_labels,
        &alias_split_stats.label_mappings,
    );
    let mut cloned_instr_id_mappings = constructor_split_stats.instr_id_mappings;
    cloned_instr_id_mappings.extend(alias_split_stats.instr_id_mappings);
    if cloned_instr_id_mappings.is_empty() {
        return Ok(0);
    }
    remap_cloned_profile_rewrites(
        caller_function_id,
        &cloned_instr_id_mappings,
        profile,
        static_direct_calls,
        remapped_call_emissions,
        remapped_inline_targets,
        suppressed_inline_targets,
        remapped_indexed_fields,
        remapped_indexed_field_counter_sources,
        remapped_exact_list_items,
        remapped_exact_int_branches,
        remapped_exact_int_returns,
        constructor_init_plans,
        constructor_field_bindings,
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
    Ok(cloned_instr_id_mappings.len())
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
) -> Result<usize, String> {
    let caller_function_id = function.function_id;
    let split_stats = split_typed_inline_cleanup_hot_continuations_for_labels_with_budget(
        function,
        hot_state_cleanup_labels,
        MAX_TYPED_INLINE_CLEANUP_CLONED_BLOCKS_PER_FUNCTION,
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
        suppressed_inline_targets,
        remapped_indexed_fields,
        remapped_indexed_field_counter_sources,
        remapped_exact_list_items,
        remapped_exact_int_branches,
        remapped_exact_int_returns,
        constructor_init_plans,
        constructor_field_bindings,
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
        validate_typed_function_value_facts(function)?;
    }
    Ok(Arc::new(JitModulePlan {
        module: Arc::new(prepared.module),
        value_facts: prepared.value_facts,
        locals: prepared.locals,
        deopt_resume: prepared.deopt_resume,
    }))
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
    )
}

pub(super) fn optimize_blockpy_for_shared_state(
    shared_state: &SharedModuleState,
    compile_session: Option<&crate::session::CompileSession>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
) -> Result<Arc<JitModulePlan>, String> {
    // Keep profile mode on the original call graph so nested runtime protocol
    // sites still collect evidence before apply/verify rewrites inline them.
    let static_targets = if env_config.specialization_mode() == Some(SpecializationMode::Profile) {
        StaticDirectCallTargets::default()
    } else {
        static_direct_call_targets(shared_state, compile_session)?
    };
    let external_callees = external_typed_inline_callees(
        shared_state,
        compile_session,
        profile,
        env_config,
        &static_targets,
    )?;
    optimize_blockpy_with_external_inline_callees(
        &shared_state.lowered_module,
        profile,
        env_config,
        external_callees,
        static_targets,
    )
}

fn optimize_blockpy_with_external_inline_callees(
    module: &BlockPyModule<BlockPyModuleShape>,
    profile: Option<&SpecializationProfile<'_>>,
    env_config: &SoacEnvConfig,
    external_callees: HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    static_targets: StaticDirectCallTargets,
) -> Result<Arc<JitModulePlan>, String> {
    let inline_plan = profile.map(|_| plan_module_inlining(&summarize_module_escapes(module)));
    let prepared = soac_driver::typed_runtime::prepare_typed_v3_runtime_module_with_rewrites(
        module,
        env_config,
        |typed_module, _value_facts| {
            let static_generator_instances =
                static_generator_instance_plans_for_module(typed_module, &static_targets);
            for function in &mut typed_module.callable_defs {
                annotate_typed_generator_instance_plans(
                    function,
                    static_generator_instances.get(&function.function_id),
                )?;
            }
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
            Ok(())
        },
    )?;
    build_jit_module_plan_from_prepared_typed_module(plan_jit_typed_module(
        prepared.module,
        prepared.value_facts,
    )?)
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
            .strict_methods
            .values()
            .map(|target| target.function_id)
            .filter(|function_id| function_id.runtime_module_id().as_u32() != current_module_id),
    );
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
                    apply_call_emission_plans_to_typed_function(
                        function,
                        profile,
                        static_direct_calls.get(&function.function_id),
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

fn apply_typed_v3_module_rewrites(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
    profile: &SpecializationProfile<'_>,
    inline_plan: Option<&soac_opt::passes::InlinePlanModule>,
    external_callees: &HashMap<RuntimeFunctionId, TypedExternalInlineCallee>,
    static_targets: &StaticDirectCallTargets,
    static_direct_calls: &StaticTypedDirectCalls,
) -> Result<(), String> {
    let mut static_constructor_calls = static_constructor_calls_for_module(module, static_targets);
    static_constructor_calls.extend(static_constructor_calls_for_external_callees(
        external_callees,
        static_targets,
    ));
    let trusted_constructor_init_owners = trusted_constructor_init_owner_refs(static_targets);
    let mut trusted_static_constructor_calls =
        trusted_static_constructor_calls(&static_constructor_calls);
    for function in &mut module.callable_defs {
        apply_call_emission_plans_to_typed_function(
            function,
            profile,
            static_direct_calls.get(&function.function_id),
        )?;
    }

    let callee_module = module.clone();
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
    let mut suppressed_inline_targets = SuppressedTypedInlineTargets::new();
    let mut constructor_init_plans =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedConstructorInitPlan>>::new();
    let mut constructor_field_bindings =
        HashMap::<RuntimeFunctionId, HashMap<InstrId, TypedConstructorFieldBindings>>::new();
    for function in &mut module.callable_defs {
        let mut hot_state_cleanup_labels = HashSet::<BlockLabel>::new();
        seed_profile_exact_int_selections_for_function(
            function,
            profile,
            &mut remapped_exact_int_branches,
            &mut remapped_exact_int_returns,
        )?;
        for pass in 0..MAX_TYPED_INLINE_PASSES {
            let mut trusted_runtime_protocol_calls = trusted_runtime_protocol_calls_for_function(
                function,
                &module.module_constants,
                trusted_static_constructor_calls.get(&function.function_id),
                &trusted_constructor_init_owners,
            );
            let (static_protocol_calls, static_protocol_inline_targets) =
                trusted_static_runtime_protocol_inlines_for_function(
                    function,
                    &module.module_constants,
                    trusted_static_constructor_calls
                        .get(&function.function_id)
                        .unwrap_or(&HashMap::new()),
                    &trusted_constructor_init_owners,
                    static_targets,
                );
            trusted_runtime_protocol_calls.extend(static_protocol_calls);
            let (trusted_static_method_calls, static_method_inline_targets) =
                trusted_static_method_inlines_for_function(
                    function,
                    &module.module_constants,
                    trusted_static_constructor_calls
                        .get(&function.function_id)
                        .unwrap_or(&HashMap::new()),
                    &trusted_constructor_init_owners,
                    static_targets,
                );
            trusted_runtime_protocol_calls.extend(trusted_static_method_calls);
            let (field_callable_emissions, static_field_callable_inline_targets) =
                trusted_field_callable_inlines_for_function(
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
            }
            let mut runtime_protocol_call_instr_ids = runtime_protocol_call_instr_ids(function);
            runtime_protocol_call_instr_ids.extend(trusted_runtime_protocol_calls.keys().copied());
            let mut inline_targets = typed_inline_targets_for_function(
                function.function_id,
                profile,
                static_direct_calls,
                &remapped_inline_targets,
                &suppressed_inline_targets,
            );
            inline_targets.extend(static_protocol_inline_targets);
            inline_targets.extend(static_method_inline_targets);
            inline_targets.extend(static_field_callable_inline_targets);
            let inline_targets = staged_inline_targets_for_trusted_runtime_protocols(
                inline_targets,
                &runtime_protocol_call_instr_ids,
                &trusted_runtime_protocol_calls,
                trusted_static_constructor_calls.get(&function.function_id),
                constructor_field_bindings.get(&function.function_id),
                &collect_typed_semantic_instr_ids(function),
            );
            if inline_targets.is_empty() {
                if split_typed_post_inline_cleanup_hot_continuations(
                    function,
                    profile,
                    static_direct_calls,
                    &mut trusted_static_constructor_calls,
                    &mut hot_state_cleanup_labels,
                    &mut remapped_call_emissions,
                    &mut remapped_inline_targets,
                    &mut suppressed_inline_targets,
                    &mut remapped_indexed_fields,
                    &mut remapped_indexed_field_counter_sources,
                    &mut remapped_exact_list_items,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                    &mut constructor_init_plans,
                    &mut constructor_field_bindings,
                )? != 0
                {
                    continue;
                }
                break;
            }
            let caller_function_id = function.function_id;
            let stats =
                inline_typed_function_direct_call_stores_with_external_callees_and_trusted_calls(
                    function,
                    &callee_module,
                    &mut module.module_constants,
                    external_callees,
                    &inline_targets,
                    &trusted_runtime_protocol_calls,
                );
            let rewrote_inline = stats.rewritten_stores != 0
                || stats.rewritten_effect_only_calls != 0
                || stats.rewritten_returns != 0;
            if !rewrote_inline {
                if split_typed_post_inline_cleanup_hot_continuations(
                    function,
                    profile,
                    static_direct_calls,
                    &mut trusted_static_constructor_calls,
                    &mut hot_state_cleanup_labels,
                    &mut remapped_call_emissions,
                    &mut remapped_inline_targets,
                    &mut suppressed_inline_targets,
                    &mut remapped_indexed_fields,
                    &mut remapped_indexed_field_counter_sources,
                    &mut remapped_exact_list_items,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                    &mut constructor_init_plans,
                    &mut constructor_field_bindings,
                )? != 0
                {
                    continue;
                }
                break;
            }
            hot_state_cleanup_labels.extend(stats.hot_state_cleanup_labels.iter().copied());
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
            let bound_constructor_sources = constructor_field_bindings
                .get(&caller_function_id)
                .map(|bindings| bindings.keys().copied().collect::<HashSet<_>>())
                .unwrap_or_default();
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
            if !stats.instr_id_mappings.is_empty() {
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
                    &stats.local_mappings,
                    profile,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                )?;
            }
            if !init_body_stats.inline_stats.instr_id_mappings.is_empty() {
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
                    &init_body_stats.inline_stats.local_mappings,
                    profile,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                )?;
            }
            if simplify_typed_virtual_tuple_ops(function, &mut module.module_constants) != 0 {
                retain_live_typed_profile_sidecars(
                    function,
                    &mut remapped_call_emissions,
                    &mut remapped_inline_targets,
                    &mut remapped_indexed_fields,
                    &mut remapped_indexed_field_counter_sources,
                    &mut remapped_exact_list_items,
                    &mut remapped_exact_int_branches,
                    &mut remapped_exact_int_returns,
                    &mut constructor_init_plans,
                );
                assign_missing_typed_function_instr_ids(function);
                refresh_typed_function_value_facts(function);
            }
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
            if let Some(remapped_call_emissions) =
                remapped_call_emissions.get(&function.function_id)
            {
                lower_typed_function_call_emission_plans(function, remapped_call_emissions)?;
                refresh_typed_function_value_facts(function);
            }
            split_typed_post_inline_hot_continuations(
                function,
                &module.module_constants,
                profile,
                static_direct_calls,
                &mut trusted_static_constructor_calls,
                &mut hot_state_cleanup_labels,
                &mut remapped_call_emissions,
                &mut remapped_inline_targets,
                &mut suppressed_inline_targets,
                &mut remapped_indexed_fields,
                &mut remapped_indexed_field_counter_sources,
                &mut remapped_exact_list_items,
                &mut remapped_exact_int_branches,
                &mut remapped_exact_int_returns,
                &mut constructor_init_plans,
                &mut constructor_field_bindings,
            )?;
            split_typed_post_inline_cleanup_hot_continuations(
                function,
                profile,
                static_direct_calls,
                &mut trusted_static_constructor_calls,
                &mut hot_state_cleanup_labels,
                &mut remapped_call_emissions,
                &mut remapped_inline_targets,
                &mut suppressed_inline_targets,
                &mut remapped_indexed_fields,
                &mut remapped_indexed_field_counter_sources,
                &mut remapped_exact_list_items,
                &mut remapped_exact_int_branches,
                &mut remapped_exact_int_returns,
                &mut constructor_init_plans,
                &mut constructor_field_bindings,
            )?;
            if pass + 1 == MAX_TYPED_INLINE_PASSES {
                return Err(format!(
                    "typed-v3 direct-call inlining exceeded {MAX_TYPED_INLINE_PASSES} passes in function {}",
                    function.function_id
                ));
            }
        }
        split_typed_post_inline_hot_continuations(
            function,
            &module.module_constants,
            profile,
            static_direct_calls,
            &mut trusted_static_constructor_calls,
            &mut hot_state_cleanup_labels,
            &mut remapped_call_emissions,
            &mut remapped_inline_targets,
            &mut suppressed_inline_targets,
            &mut remapped_indexed_fields,
            &mut remapped_indexed_field_counter_sources,
            &mut remapped_exact_list_items,
            &mut remapped_exact_int_branches,
            &mut remapped_exact_int_returns,
            &mut constructor_init_plans,
            &mut constructor_field_bindings,
        )?;
        split_typed_post_inline_cleanup_hot_continuations(
            function,
            profile,
            static_direct_calls,
            &mut trusted_static_constructor_calls,
            &mut hot_state_cleanup_labels,
            &mut remapped_call_emissions,
            &mut remapped_inline_targets,
            &mut suppressed_inline_targets,
            &mut remapped_indexed_fields,
            &mut remapped_indexed_field_counter_sources,
            &mut remapped_exact_list_items,
            &mut remapped_exact_int_branches,
            &mut remapped_exact_int_returns,
            &mut constructor_init_plans,
            &mut constructor_field_bindings,
        )?;
        if let Some(remapped_call_emissions) = remapped_call_emissions.get(&function.function_id) {
            lower_typed_function_call_emission_plans(function, remapped_call_emissions)?;
            refresh_typed_function_value_facts(function);
        }
        if rewrite_typed_stop_iteration_raises_to_handler_jumps(function, &module.module_constants)
            != 0
        {
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
        }
        if simplify_typed_virtual_tuple_ops(function, &mut module.module_constants) != 0 {
            retain_live_typed_profile_sidecars(
                function,
                &mut remapped_call_emissions,
                &mut remapped_inline_targets,
                &mut remapped_indexed_fields,
                &mut remapped_indexed_field_counter_sources,
                &mut remapped_exact_list_items,
                &mut remapped_exact_int_branches,
                &mut remapped_exact_int_returns,
                &mut constructor_init_plans,
            );
            assign_missing_typed_function_instr_ids(function);
            refresh_typed_function_value_facts(function);
        }
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
        if let Some(bindings) = constructor_field_bindings.get(&function.function_id) {
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
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jit::specialization_profile::DirectCallEmissionScope;
    use soac_core::block_py::{
        BlockParam, InstrWithConstantNone, LocalFunctionId, NameLocation, ResolvedName,
        RuntimeFunctionId, RuntimeModuleId,
    };
    use soac_ir_typed::lower_blockpy_module_to_typed;
    use soac_opt::passes::{TypedInlineInstanceSource, TypedInlineRewriteStats};

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
        let (owners, inline_targets) = trusted_static_runtime_protocol_inlines_for_function(
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
            module_globals: HashMap::new(),
            module_global_generators: HashMap::from([(module_id, generator_targets)]),
            strict_methods: HashMap::new(),
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
        let (owners, inline_targets) = trusted_static_runtime_protocol_inlines_for_function(
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
    fn trusted_owner_edge_remap_preserves_object_and_function_facts() {
        let source_location = LocalLocation(7);
        let target_location = LocalLocation(8);
        let origin = InstrId::new(9);
        let function_id =
            RuntimeFunctionId::new(RuntimeModuleId::new(10), LocalFunctionId::new(11));
        let state = TrustedOwnerState {
            object_origins: HashMap::from([(source_location, origin)]),
            local_functions: HashMap::from([(source_location, function_id)]),
            ..TrustedOwnerState::default()
        };
        let target = soac_ir_typed::TypedBlock {
            label: BlockLabel::from_index(12),
            params: vec![BlockParam {
                name: "target".to_string(),
                role: BlockParamRole::Value,
            }],
            body: Vec::new(),
            term: BlockTerm::Return(InstrTyped::constant_none()),
            exc_edge: None,
            extra: Default::default(),
        };
        let remapped = remap_trusted_owner_state_for_edge(
            &target,
            Some(&[BlockArg::Name("source".to_string())]),
            &HashMap::from([
                ("source".to_string(), source_location),
                ("target".to_string(), target_location),
            ]),
            &state,
        );

        assert_eq!(remapped.object_origins.get(&target_location), Some(&origin));
        assert_eq!(
            remapped.local_functions.get(&target_location),
            Some(&function_id)
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
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod"),
            )]),
            module_global_generators: HashMap::from([(
                module_id,
                strict_module_global_generator_targets_for_module(&lowered),
            )]),
            strict_methods: HashMap::new(),
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
                module_globals: HashMap::new(),
                module_global_generators: HashMap::new(),
                strict_methods: HashMap::new(),
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
                module_globals: HashMap::new(),
                module_global_generators: HashMap::new(),
                strict_methods: HashMap::new(),
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
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "soac.runtime"),
            )]),
            module_global_generators: HashMap::new(),
            strict_methods: HashMap::new(),
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
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "soac.runtime"),
            )]),
            module_global_generators: HashMap::new(),
            strict_methods: HashMap::new(),
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
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod"),
            )]),
            module_global_generators: HashMap::new(),
            strict_methods: HashMap::new(),
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
            module_globals: HashMap::from([(
                module_id,
                strict_module_global_direct_call_targets_for_module(&lowered, "pkg.mod"),
            )]),
            module_global_generators: HashMap::new(),
            strict_methods: HashMap::new(),
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
