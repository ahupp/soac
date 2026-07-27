use soac_core::block_py::{
    BlockLabel, BlockPyFunction, BlockPyModule, BlockTerm, CallArgPositional, ChildVisitable,
    ConstantExpr, FunctionKind, HasSemanticInstrId, InstrId, NameLike, NameLocation,
    ParamDefaultSource, ParamKind, ResolvedName, RuntimeFunctionId, RuntimeName,
    SerializedFunctionDebugName, SerializedFunctionId, SerializedIdentityTables,
    SerializedModuleId, SerializedModuleIdentity, Visit, VisitMut,
};
use soac_driver::blockpy_cache::hash_module_source;
use soac_ir_typed::emit_v3::{MechanicalOpaqueFusedIterationEmission, emit_mechanical_plan_v3};
use soac_ir_typed::plan_v3::{
    Cost, DirectCallArgPlan, DirectCallArgSource, FunctionPlanIdentity, ModulePlanIdentity,
    OpaqueFusedAlgorithmPlan, OpaqueFusedCompatibilityMode, OpaqueFusedCompletionPlan,
    OpaqueFusedConsumerKind, OpaqueFusedEntryGuardPlan, OpaqueFusedExceptionPlan,
    OpaqueFusedFallbackPlan, OpaqueFusedGuardExpectation, OpaqueFusedGuardInput,
    OpaqueFusedIterationPlan, OpaqueFusedPositionalDefaultPlan, OpaqueFusedProducerStagePlan,
    OpaqueFusedSinkPlan, OpaqueFusedSite, OpaqueFusedSiteOwner, OpaqueFusedStageId,
    OpaqueFusedYieldEdgePlan, OpaqueFusedYieldTarget,
};
use soac_ir_typed::{
    InstrTyped, TypedBlockPyModuleShape, TypedCall, TypedDirectCallArgPlan,
    TypedDirectCallArgSource, TypedGeneratorInstancePlan, TypedOpaqueFusedEntryGuard,
    TypedOpaqueFusedGuardExpectation, TypedOpaqueFusedGuardOperand, TypedOpaqueFusedIterationPlan,
    TypedOpaqueFusedResult,
};
use soac_opt::alternatives_v3::AlternativeCatalog;
use soac_opt::passes::{
    TrustedOwnerState, TypedVirtualBodyInstr, analyze_trusted_owner_states,
    trusted_object_origin_for_name, visit_trusted_owner_term_instrs,
};
use soac_opt::planner_v3::{
    FunctionPlanRequest, ModulePlanRequest, OpaqueFusedIterationPlanRequest,
    plan_module_optimization_v3,
};
use std::collections::{HashMap, HashSet};

const TRACKED_COUNTED_NQUEENS_SOURCE: &str = include_str!("fixtures/opaque_fused_nqueens_v1.py");
const TRACKED_PYPERFORMANCE_NQUEENS_SOURCE: &str =
    include_str!("fixtures/opaque_fused_pyperformance_nqueens_v1.py");
#[cfg(test)]
const TRACKED_NQUEENS_SOURCE: &str = TRACKED_COUNTED_NQUEENS_SOURCE;
#[cfg(test)]
const CURRENT_NQUEENS_BENCHMARK_SOURCE: &str =
    include_str!("../../../../bench/nqueens_slice_full_nqueens_list_consumer.py");
const TRACKED_NQUEENS_MAXIMUM_WIDTH: u32 = 8;

pub(super) fn tracked_nqueens_source_matches(source: &str) -> bool {
    source.as_bytes() == TRACKED_COUNTED_NQUEENS_SOURCE.as_bytes()
        || source.as_bytes() == TRACKED_PYPERFORMANCE_NQUEENS_SOURCE.as_bytes()
}

/// A semantic call site is only unique together with the function that owns it.
///
/// The serialized v3 plan uses stage ownership instead of runtime function ids.
/// Discovery keeps the runtime owner until the complete producer tree has been
/// validated and can be assigned stable stage ids.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(super) struct OpaqueFusedDiscoverySite {
    pub function_id: RuntimeFunctionId,
    pub source: InstrId,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(super) enum OpaqueFusedDiscoveryConsumerKind {
    ForLoop,
    BuildList,
    BuildTuple,
    BuildSet,
}

impl OpaqueFusedDiscoveryConsumerKind {
    fn from_runtime_name(runtime_name: RuntimeName) -> Option<Self> {
        match runtime_name {
            RuntimeName::Next => Some(Self::ForLoop),
            RuntimeName::List => Some(Self::BuildList),
            RuntimeName::Tuple => Some(Self::BuildTuple),
            RuntimeName::Set => Some(Self::BuildSet),
            _ => None,
        }
    }

    fn to_plan(self) -> Option<OpaqueFusedConsumerKind> {
        match self {
            Self::ForLoop => Some(OpaqueFusedConsumerKind::ForLoop),
            Self::BuildTuple => Some(OpaqueFusedConsumerKind::BuildTuple),
            Self::BuildSet => Some(OpaqueFusedConsumerKind::BuildSet),
            Self::BuildList => None,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OpaqueFusedDiscoveryConsumer {
    pub site: OpaqueFusedDiscoverySite,
    pub kind: OpaqueFusedDiscoveryConsumerKind,
    pub runtime_name: RuntimeName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OpaqueFusedDiscoveryStage {
    pub id: OpaqueFusedStageId,
    pub parent: Option<OpaqueFusedStageId>,
    pub origin: OpaqueFusedDiscoverySite,
    pub function_id: RuntimeFunctionId,
    pub arg_plan: TypedDirectCallArgPlan,
    pub positional_defaults: Vec<OpaqueFusedPositionalDefault>,
    pub yield_sources: Vec<InstrId>,
    pub completion_blocks: Vec<BlockLabel>,
    pub consumer: OpaqueFusedDiscoveryConsumer,
    pub fresh_closure_callable: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct OpaqueFusedPositionalDefault {
    pub parameter_index: u32,
    pub default_index: u32,
    pub expected_defaults_len: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OpaqueFusedCountGraph {
    pub root_function_id: RuntimeFunctionId,
    pub sink: OpaqueFusedRootSink,
    pub result_source: InstrId,
    pub consume_source: InstrId,
    pub entry_stage: OpaqueFusedStageId,
    pub width_param_index: u32,
    pub stages: Vec<OpaqueFusedDiscoveryStage>,
    pub builtin_sites: Vec<(OpaqueFusedDiscoverySite, RuntimeName)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OpaqueFusedRootSink {
    Count,
    Discard,
}

impl OpaqueFusedCountGraph {
    pub(super) fn builtin_consumer_count(&self) -> usize {
        self.stages
            .iter()
            .filter(|stage| {
                matches!(
                    stage.consumer.kind,
                    OpaqueFusedDiscoveryConsumerKind::BuildList
                        | OpaqueFusedDiscoveryConsumerKind::BuildTuple
                        | OpaqueFusedDiscoveryConsumerKind::BuildSet
                )
            })
            .count()
    }

    pub(super) fn for_loop_consumer_count(&self) -> usize {
        self.stages
            .iter()
            .filter(|stage| stage.consumer.kind == OpaqueFusedDiscoveryConsumerKind::ForLoop)
            .count()
    }

    /// Convert a validated producer graph into the typed v3 plan schema.
    ///
    /// The caller must supply the semantic algorithm decision. Discovery proves
    /// producer ownership, consumption, completion, and stage uniqueness; it
    /// deliberately does not guess that an arbitrary graph implements the
    /// affine/distinct N-Queens predicate merely because it contains the same
    /// number of `set` or `tuple` consumers.
    pub(super) fn to_plan(
        &self,
        algorithm: OpaqueFusedAlgorithmPlan,
        maximum_width: u32,
        cost: Cost,
        mut serialize_function: impl FnMut(
            RuntimeFunctionId,
        ) -> Option<soac_core::block_py::SerializedFunctionId>,
    ) -> Result<OpaqueFusedIterationPlan, OpaqueFusedRejectionReason> {
        if !(1..=63).contains(&maximum_width) {
            return Err(OpaqueFusedRejectionReason::InvalidMaximumWidth(
                maximum_width,
            ));
        }
        let width_input = OpaqueFusedGuardInput::FunctionParam {
            index: self.width_param_index,
        };
        match &algorithm {
            OpaqueFusedAlgorithmPlan::AffineDistinctPermutationCount {
                width_input: algorithm_input,
                maximum_width: algorithm_maximum,
            } if algorithm_input == &width_input && *algorithm_maximum == maximum_width => {}
            _ => return Err(OpaqueFusedRejectionReason::AlgorithmInputMismatch),
        }

        let plan_site =
            |site: OpaqueFusedDiscoverySite, owner: OpaqueFusedSiteOwner| -> OpaqueFusedSite {
                OpaqueFusedSite {
                    owner,
                    source: site.source,
                }
            };
        let mut stages = Vec::with_capacity(self.stages.len());
        let mut entry_guards = Vec::new();
        for stage in &self.stages {
            let owner = stage
                .parent
                .map(OpaqueFusedSiteOwner::Stage)
                .unwrap_or(OpaqueFusedSiteOwner::Root);
            let origin = plan_site(stage.origin, owner);
            let serialized_function = serialize_function(stage.function_id).ok_or(
                OpaqueFusedRejectionReason::MissingSerializedFunction(stage.function_id),
            )?;
            if !stage.fresh_closure_callable && stage.positional_defaults.is_empty() {
                entry_guards.push(OpaqueFusedEntryGuardPlan {
                    input: OpaqueFusedGuardInput::Site(origin),
                    expectation: OpaqueFusedGuardExpectation::FunctionIdentity {
                        target: serialized_function,
                    },
                    reason: "validate producer function identity and code before fused activation"
                        .to_string(),
                });
            }
            for positional_default in &stage.positional_defaults {
                entry_guards.push(OpaqueFusedEntryGuardPlan {
                    input: OpaqueFusedGuardInput::Site(origin),
                    expectation: OpaqueFusedGuardExpectation::FunctionPositionalDefaultIdentity {
                        target: serialized_function,
                        parameter_index: positional_default.parameter_index,
                        default_index: positional_default.default_index,
                        expected_defaults_len: positional_default.expected_defaults_len,
                        expected: RuntimeName::None,
                    },
                    reason:
                        "validate the omitted positional default is None before fused activation"
                            .to_string(),
                });
            }
            let target = if stage.id == self.entry_stage {
                OpaqueFusedYieldTarget::Sink
            } else {
                let consumer_owner = stage.parent.map(OpaqueFusedSiteOwner::Stage).ok_or(
                    OpaqueFusedRejectionReason::NestedStageWithoutParent(stage.id),
                )?;
                let kind = stage.consumer.kind.to_plan().ok_or(
                    OpaqueFusedRejectionReason::UnsupportedNestedListConsumer(stage.consumer.site),
                )?;
                OpaqueFusedYieldTarget::Consumer {
                    site: plan_site(stage.consumer.site, consumer_owner),
                    kind,
                }
            };
            stages.push(OpaqueFusedProducerStagePlan {
                id: stage.id,
                function: serialized_function,
                origin,
                arg_plan: typed_arg_plan_to_v3(&stage.arg_plan)?,
                positional_defaults: stage
                    .positional_defaults
                    .iter()
                    .map(|default| OpaqueFusedPositionalDefaultPlan {
                        parameter_index: default.parameter_index,
                        default_index: default.default_index,
                        expected_defaults_len: default.expected_defaults_len,
                    })
                    .collect(),
                yield_edges: stage
                    .yield_sources
                    .iter()
                    .copied()
                    .map(|source| OpaqueFusedYieldEdgePlan {
                        source,
                        target: target.clone(),
                    })
                    .collect(),
                completion: OpaqueFusedCompletionPlan::FinishConsumer,
                exception: OpaqueFusedExceptionPlan::Propagate,
            });
        }
        for (site, runtime_name) in &self.builtin_sites {
            let owner = if site.function_id == self.root_function_id {
                OpaqueFusedSiteOwner::Root
            } else {
                let stage = self
                    .stages
                    .iter()
                    .find(|stage| stage.function_id == site.function_id)
                    .ok_or(OpaqueFusedRejectionReason::MissingBuiltinOwner(*site))?;
                OpaqueFusedSiteOwner::Stage(stage.id)
            };
            entry_guards.push(OpaqueFusedEntryGuardPlan {
                input: OpaqueFusedGuardInput::Site(plan_site(*site, owner)),
                expectation: OpaqueFusedGuardExpectation::RuntimeBuiltinIdentity {
                    runtime_name: *runtime_name,
                },
                reason: "validate the exact immediate-consumer builtin before fused activation"
                    .to_string(),
            });
        }
        entry_guards.push(OpaqueFusedEntryGuardPlan {
            input: width_input.clone(),
            expectation: OpaqueFusedGuardExpectation::ExactCompactIntRange {
                min: 0,
                max: i64::from(maximum_width),
            },
            reason: "bound affine bitset width before entering the non-deoptimizing fused region"
                .to_string(),
        });

        Ok(OpaqueFusedIterationPlan {
            source: self.result_source,
            algorithm,
            compatibility: OpaqueFusedCompatibilityMode::OpaqueNoGeneratorMaterialization,
            entry_stage: self.entry_stage,
            stages,
            sink: match self.sink {
                OpaqueFusedRootSink::Count => OpaqueFusedSinkPlan::Count {
                    consume_source: self.consume_source,
                    result_source: self.result_source,
                },
                OpaqueFusedRootSink::Discard => OpaqueFusedSinkPlan::Discard {
                    consume_source: self.consume_source,
                    result_source: self.result_source,
                },
            },
            entry_guards,
            fallback: OpaqueFusedFallbackPlan {
                original_source: self.result_source,
            },
            cost,
            reason: "complete nonescaping producer graph selected for opaque fused count iteration"
                .to_string(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct OpaqueFusedRejection {
    pub root_function_id: RuntimeFunctionId,
    pub source: Option<InstrId>,
    pub reason: OpaqueFusedRejectionReason,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) enum OpaqueFusedRejectionReason {
    MissingSemanticInstrId(&'static str),
    MissingGeneratorFunction(RuntimeFunctionId),
    NonGeneratorFunction(RuntimeFunctionId),
    MissingYield(RuntimeFunctionId),
    MissingCompletion(RuntimeFunctionId),
    YieldFrom(RuntimeFunctionId),
    SourceCleanup(RuntimeFunctionId),
    UnsupportedKeywordArguments(OpaqueFusedDiscoverySite),
    UnsupportedStarredArguments(OpaqueFusedDiscoverySite),
    KeywordOnlyDefault {
        function_id: RuntimeFunctionId,
        param: String,
    },
    EscapedProducer(OpaqueFusedDiscoverySite),
    MissingConsumer(OpaqueFusedDiscoverySite),
    MultipleConsumers {
        producer: OpaqueFusedDiscoverySite,
        consumers: Vec<OpaqueFusedDiscoverySite>,
    },
    UnsupportedNestedListConsumer(OpaqueFusedDiscoverySite),
    ProducerCycle(RuntimeFunctionId),
    WidthInputShape,
    MissingSerializedFunction(RuntimeFunctionId),
    InvalidMaximumWidth(u32),
    AlgorithmInputMismatch,
    NestedStageWithoutParent(OpaqueFusedStageId),
    MissingBuiltinOwner(OpaqueFusedDiscoverySite),
    ArgIndexOverflow(usize),
    ArgPlanArityMismatch {
        function_id: RuntimeFunctionId,
        params: usize,
        sources: usize,
    },
    MissingPositionalDefault {
        function_id: RuntimeFunctionId,
        parameter_index: usize,
    },
}

#[derive(Clone, Debug, Default)]
pub(super) struct OpaqueFusedDiscoveryReport {
    pub graphs: Vec<OpaqueFusedCountGraph>,
    pub rejections: Vec<OpaqueFusedRejection>,
}

#[derive(Clone, Debug)]
pub(super) struct AdmittedOpaqueFusedCount {
    pub root_function_id: RuntimeFunctionId,
    pub graph: OpaqueFusedCountGraph,
    pub emission: MechanicalOpaqueFusedIterationEmission,
    pub typed_plan: TypedOpaqueFusedIterationPlan,
}

fn function_for_id(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    function_id: RuntimeFunctionId,
) -> Result<&BlockPyFunction<TypedBlockPyModuleShape>, String> {
    module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .ok_or_else(|| format!("opaque fused graph references missing function {function_id:?}"))
}

fn validate_tracked_nqueens_graph_shape(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    graph: &OpaqueFusedCountGraph,
) -> Result<(), String> {
    let root_function = function_for_id(module, graph.root_function_id)?;
    let expected_root = match graph.sink {
        OpaqueFusedRootSink::Count => "full_nqueens_list_consumer",
        OpaqueFusedRootSink::Discard => "bench_n_queens",
    };
    if root_function.names.qualname != expected_root {
        return Err(format!(
            "tracked N-Queens root is {:?}, expected {expected_root} for {:?} sink",
            root_function.names.qualname, graph.sink
        ));
    }
    let root_params = root_function.params.iter().collect::<Vec<_>>();
    if graph.width_param_index != 0
        || root_params.len() != 1
        || root_params[0].name != "queen_count"
        || root_params[0].kind != ParamKind::Any
        || root_params[0].has_default
    {
        return Err(
            "tracked N-Queens root does not have the exact queen_count width parameter".to_string(),
        );
    }
    if graph.stages.len() != 6
        || graph.builtin_consumer_count() != 5
        || graph.for_loop_consumer_count() != 1
    {
        return Err(format!(
            "tracked N-Queens producer graph has {} stages, {} aggregate consumers, and {} for consumers; expected 6/5/1",
            graph.stages.len(),
            graph.builtin_consumer_count(),
            graph.for_loop_consumer_count()
        ));
    }

    let entry = graph
        .stages
        .iter()
        .find(|stage| stage.id == graph.entry_stage)
        .ok_or_else(|| "tracked N-Queens graph is missing its entry stage".to_string())?;
    let entry_function = function_for_id(module, entry.function_id)?;
    if entry.parent.is_some()
        || entry.origin.function_id != graph.root_function_id
        || entry_function.names.qualname != "n_queens"
        || entry.consumer.kind != OpaqueFusedDiscoveryConsumerKind::BuildList
        || entry.arg_plan.sources != vec![TypedDirectCallArgSource::Provided(0)]
        || !entry.positional_defaults.is_empty()
    {
        return Err(
            "tracked N-Queens entry is not the exact root -> n_queens aggregate chain".to_string(),
        );
    }

    let permutations = graph
        .stages
        .iter()
        .filter(|stage| {
            function_for_id(module, stage.function_id)
                .is_ok_and(|function| function.names.qualname == "permutations")
        })
        .collect::<Vec<_>>();
    let [permutations] = permutations.as_slice() else {
        return Err(format!(
            "tracked N-Queens graph has {} permutations stages, expected exactly one",
            permutations.len()
        ));
    };
    let permutations_function = function_for_id(module, permutations.function_id)?;
    let permutations_params = permutations_function.params.iter().collect::<Vec<_>>();
    if permutations.parent != Some(entry.id)
        || permutations.origin.function_id != entry.function_id
        || permutations.consumer.kind != OpaqueFusedDiscoveryConsumerKind::ForLoop
        || permutations.arg_plan.sources
            != vec![
                TypedDirectCallArgSource::Provided(0),
                TypedDirectCallArgSource::DefaultSentinel,
            ]
        || permutations.positional_defaults
            != vec![OpaqueFusedPositionalDefault {
                parameter_index: 1,
                default_index: 0,
                expected_defaults_len: 1,
            }]
        || permutations_params.len() != 2
        || permutations_params[0].name != "iterable"
        || permutations_params[0].kind != ParamKind::Any
        || permutations_params[0].has_default
        || permutations_params[1].name != "r"
        || permutations_params[1].kind != ParamKind::Any
        || !permutations_params[1].has_default
    {
        return Err("tracked N-Queens permutations stage does not preserve the exact omitted r=None call shape"
            .to_string());
    }

    let genexprs = graph
        .stages
        .iter()
        .filter(|stage| stage.id != entry.id && stage.id != permutations.id)
        .collect::<Vec<_>>();
    if genexprs.len() != 4
        || genexprs.iter().any(|stage| {
            !stage.fresh_closure_callable
                || !stage.positional_defaults.is_empty()
                || !matches!(
                    stage.consumer.kind,
                    OpaqueFusedDiscoveryConsumerKind::BuildTuple
                        | OpaqueFusedDiscoveryConsumerKind::BuildSet
                )
                || function_for_id(module, stage.function_id)
                    .map_or(true, |function| function.names.display_name != "<genexpr>")
        })
    {
        return Err(format!(
            "tracked N-Queens graph does not contain exactly four immediate aggregate genexpr stages: {genexprs:#?}"
        ));
    }
    let entry_genexprs = genexprs
        .iter()
        .filter(|stage| stage.parent == Some(entry.id))
        .count();
    let permutations_genexprs = genexprs
        .iter()
        .filter(|stage| stage.parent == Some(permutations.id))
        .count();
    let tuple_consumers = graph
        .stages
        .iter()
        .filter(|stage| stage.consumer.kind == OpaqueFusedDiscoveryConsumerKind::BuildTuple)
        .count();
    let set_consumers = graph
        .stages
        .iter()
        .filter(|stage| stage.consumer.kind == OpaqueFusedDiscoveryConsumerKind::BuildSet)
        .count();
    let list_consumers = graph
        .stages
        .iter()
        .filter(|stage| stage.consumer.kind == OpaqueFusedDiscoveryConsumerKind::BuildList)
        .count();
    if entry_genexprs != 2
        || permutations_genexprs != 2
        || tuple_consumers != 2
        || set_consumers != 2
        || list_consumers != 1
    {
        return Err(format!(
            "tracked N-Queens aggregate topology is not exact: entry_genexprs={entry_genexprs}, permutations_genexprs={permutations_genexprs}, tuple={tuple_consumers}, set={set_consumers}, list={list_consumers}"
        ));
    }
    if graph.stages.iter().any(|stage| {
        stage.yield_sources.is_empty()
            || stage.completion_blocks.is_empty()
            || (stage.id != permutations.id && !stage.positional_defaults.is_empty())
    }) {
        return Err(
            "tracked N-Queens graph has an incomplete stage or an unexpected default".to_string(),
        );
    }
    let expected_len_count = match graph.sink {
        OpaqueFusedRootSink::Count => 4,
        OpaqueFusedRootSink::Discard => 3,
    };
    for (runtime_name, expected_count) in [
        (RuntimeName::Range, 4),
        (RuntimeName::Len, expected_len_count),
        (RuntimeName::List, 3),
        (RuntimeName::Tuple, 3),
        (RuntimeName::Reversed, 1),
        (RuntimeName::Set, 2),
    ] {
        let actual_count = graph
            .builtin_sites
            .iter()
            .filter(|(_, candidate)| *candidate == runtime_name)
            .count();
        if actual_count != expected_count {
            return Err(format!(
                "tracked N-Queens dependency {runtime_name:?} has {actual_count} guarded call sites, expected {expected_count}"
            ));
        }
    }
    Ok(())
}

fn owner_function_id(
    graph: &OpaqueFusedCountGraph,
    owner: OpaqueFusedSiteOwner,
) -> Result<RuntimeFunctionId, String> {
    match owner {
        OpaqueFusedSiteOwner::Root => Ok(graph.root_function_id),
        OpaqueFusedSiteOwner::Stage(stage_id) => graph
            .stages
            .iter()
            .find(|stage| stage.id == stage_id)
            .map(|stage| stage.function_id)
            .ok_or_else(|| format!("opaque fused guard references missing stage {stage_id:?}")),
    }
}

fn stage_target_function_id(
    graph: &OpaqueFusedCountGraph,
    site: OpaqueFusedSite,
) -> Result<RuntimeFunctionId, String> {
    graph
        .stages
        .iter()
        .find(|stage| {
            let owner = stage
                .parent
                .map(OpaqueFusedSiteOwner::Stage)
                .unwrap_or(OpaqueFusedSiteOwner::Root);
            owner == site.owner && stage.origin.source == site.source
        })
        .map(|stage| stage.function_id)
        .ok_or_else(|| format!("opaque fused function guard has no producer at {site:?}"))
}

fn typed_expr_at_source(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    source: InstrId,
) -> Result<InstrTyped, String> {
    struct Finder {
        source: InstrId,
        found: Vec<InstrTyped>,
    }

    impl Visit<InstrTyped> for Finder {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if expr.try_semantic_instr_id() == Some(self.source) {
                self.found.push(expr.clone());
            }
            expr.visit_children(self);
        }
    }

    let mut finder = Finder {
        source,
        found: Vec::new(),
    };
    finder.visit_fn(function);
    match finder.found.as_slice() {
        [expr] => Ok(expr.clone()),
        found => Err(format!(
            "opaque fused guard source {source} resolved to {} typed expressions in {:?}",
            found.len(),
            function.function_id
        )),
    }
}

fn typed_guard_operand_for_callable(
    callable: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Result<TypedOpaqueFusedGuardOperand, String> {
    let InstrTyped::Load(load) = callable else {
        return Err(format!(
            "opaque fused guarded callable is not a resolved load: {callable:?}"
        ));
    };
    if let Some(local) = load.name.location.as_local() {
        let mut name = load.name.clone();
        name.location = NameLocation::Local(local);
        return Ok(TypedOpaqueFusedGuardOperand::Local(name));
    }
    if let Some(global) = load.name.location.as_global() {
        return Ok(TypedOpaqueFusedGuardOperand::IndexedGlobal {
            name: load.name.id_str().to_string(),
            expected_index: global.slot(),
        });
    }
    if let Some(runtime_name) = load.name.runtime_name_id() {
        return Ok(TypedOpaqueFusedGuardOperand::RuntimeName(runtime_name));
    }
    if let Some(constant) = load.name.location.as_constant()
        && let Some(ConstantExpr::RuntimeName(runtime_name)) =
            module_constants.get(constant as usize)
    {
        return Ok(TypedOpaqueFusedGuardOperand::RuntimeName(*runtime_name));
    }
    Err(format!(
        "opaque fused guarded callable has unsupported location {:?}",
        load.name.location
    ))
}

fn resolve_width_local(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    parameter_index: u32,
) -> Result<ResolvedName, String> {
    let parameter = function
        .params
        .iter()
        .nth(parameter_index as usize)
        .ok_or_else(|| format!("opaque fused width parameter {parameter_index} is missing"))?;
    struct Finder<'a> {
        parameter: &'a str,
        names: Vec<ResolvedName>,
    }
    impl Visit<InstrTyped> for Finder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::Load(load) = expr
                && load.name.id_str() == self.parameter
                && load.name.location.as_local().is_some()
                && !self.names.contains(&load.name)
            {
                self.names.push(load.name.clone());
            }
            expr.visit_children(self);
        }
    }
    let mut finder = Finder {
        parameter: parameter.name.as_str(),
        names: Vec::new(),
    };
    finder.visit_fn(function);
    match finder.names.as_slice() {
        [name] => Ok(name.clone()),
        names => Err(format!(
            "opaque fused width parameter {:?} resolved to {} ordinary locals",
            parameter.name,
            names.len()
        )),
    }
}

fn resolve_typed_plan(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    graph: &OpaqueFusedCountGraph,
    plan: &MechanicalOpaqueFusedIterationEmission,
) -> Result<TypedOpaqueFusedIterationPlan, String> {
    let root_function = function_for_id(module, graph.root_function_id)?;
    let mut minimum_width = None;
    let mut maximum_width = None;
    let mut entry_guards = Vec::new();
    for guard in &plan.entry_guards {
        match (&guard.input, &guard.expectation) {
            (
                OpaqueFusedGuardInput::FunctionParam { index },
                OpaqueFusedGuardExpectation::ExactCompactIntRange { min, max },
            ) if *index == graph.width_param_index => {
                if minimum_width.replace(*min).is_some() || maximum_width.replace(*max).is_some() {
                    return Err("opaque fused plan has duplicate width range guards".to_string());
                }
            }
            (OpaqueFusedGuardInput::Site(site), expectation) => {
                let owner_function =
                    function_for_id(module, owner_function_id(graph, site.owner)?)?;
                let call = typed_expr_at_source(owner_function, site.source)?;
                let call = typed_call_parts(&call).ok_or_else(|| {
                    format!("opaque fused guard source {site:?} is not a typed call")
                })?;
                let (operand, expectation) = match expectation {
                    OpaqueFusedGuardExpectation::FunctionIdentity { .. } => {
                        let function_id = stage_target_function_id(graph, *site)?;
                        (
                            typed_guard_operand_for_callable(call.func, &module.module_constants)?,
                            TypedOpaqueFusedGuardExpectation::FunctionIdentity { function_id },
                        )
                    }
                    OpaqueFusedGuardExpectation::FunctionPositionalDefaultIdentity {
                        default_index,
                        expected_defaults_len,
                        expected,
                        ..
                    } => {
                        let function_id = stage_target_function_id(graph, *site)?;
                        (
                            typed_guard_operand_for_callable(call.func, &module.module_constants)?,
                            TypedOpaqueFusedGuardExpectation::FunctionPositionalDefaultIdentity {
                                function_id,
                                default_index: *default_index,
                                expected_defaults_len: *expected_defaults_len,
                                expected: *expected,
                            },
                        )
                    }
                    OpaqueFusedGuardExpectation::RuntimeBuiltinIdentity { runtime_name } => {
                        if typed_call_runtime_name(call.func, call.access, &module.module_constants)
                            != Some(*runtime_name)
                        {
                            return Err(format!(
                                "opaque fused builtin guard at {site:?} no longer resolves to {runtime_name:?}"
                            ));
                        }
                        continue;
                    }
                    OpaqueFusedGuardExpectation::ExactCompactIntRange { .. } => {
                        return Err(
                            "opaque fused exact-int range guard must target the width parameter"
                                .to_string(),
                        );
                    }
                };
                entry_guards.push(TypedOpaqueFusedEntryGuard {
                    operand,
                    expectation,
                });
            }
            _ => {
                return Err("opaque fused plan has an unsupported entry guard input".to_string());
            }
        }
    }
    Ok(TypedOpaqueFusedIterationPlan {
        source: plan.source,
        result: match &plan.sink {
            OpaqueFusedSinkPlan::Count { .. } => TypedOpaqueFusedResult::Count,
            OpaqueFusedSinkPlan::Discard { .. } => TypedOpaqueFusedResult::Discard,
        },
        width_input: resolve_width_local(root_function, graph.width_param_index)?,
        minimum_width: minimum_width
            .ok_or_else(|| "opaque fused plan is missing its minimum width".to_string())?,
        maximum_width: maximum_width
            .ok_or_else(|| "opaque fused plan is missing its maximum width".to_string())?,
        entry_guards,
    })
}

fn plan_and_emit_tracked_nqueens(
    graph: &OpaqueFusedCountGraph,
) -> Result<MechanicalOpaqueFusedIterationEmission, String> {
    let (module_name, source) = match graph.sink {
        OpaqueFusedRootSink::Count => (
            "soac.opaque_fused_nqueens.count",
            TRACKED_COUNTED_NQUEENS_SOURCE,
        ),
        OpaqueFusedRootSink::Discard => (
            "soac.opaque_fused_nqueens.pyperformance",
            TRACKED_PYPERFORMANCE_NQUEENS_SOURCE,
        ),
    };
    let serialized_module = SerializedModuleId::new(0);
    let serialize_function = |function_id: RuntimeFunctionId| {
        Some(SerializedFunctionId::new(
            serialized_module,
            function_id.local_function_id(),
        ))
    };
    let width_input = OpaqueFusedGuardInput::FunctionParam {
        index: graph.width_param_index,
    };
    let candidate = graph
        .to_plan(
            OpaqueFusedAlgorithmPlan::AffineDistinctPermutationCount {
                width_input,
                maximum_width: TRACKED_NQUEENS_MAXIMUM_WIDTH,
            },
            TRACKED_NQUEENS_MAXIMUM_WIDTH,
            Cost::default(),
            serialize_function,
        )
        .map_err(|reason| format!("tracked N-Queens plan conversion failed: {reason:?}"))?;

    let source_hash = hash_module_source(source);
    let cache_identity = "opaque-fused-nqueens-v1".to_string();
    let root_function = SerializedFunctionId::new(
        serialized_module,
        graph.root_function_id.local_function_id(),
    );
    let root_debug_name = match graph.sink {
        OpaqueFusedRootSink::Count => "full_nqueens_list_consumer",
        OpaqueFusedRootSink::Discard => "bench_n_queens",
    };
    let request = ModulePlanRequest {
        module: ModulePlanIdentity {
            module_name: module_name.to_string(),
            source_hash,
            cache_identity: cache_identity.clone(),
        },
        identity_tables: SerializedIdentityTables {
            modules: vec![SerializedModuleIdentity {
                module_name: module_name.to_string(),
                source_hash,
                cache_identity: Some(cache_identity),
            }],
            debug_names: vec![SerializedFunctionDebugName {
                function: root_function,
                qualname: root_debug_name.to_string(),
            }],
        },
        functions: vec![FunctionPlanRequest {
            function: FunctionPlanIdentity {
                function: root_function,
                debug_name: Some(root_debug_name.to_string()),
            },
            regions: Vec::new(),
            direct_calls: Vec::new(),
            exact_list_items: Vec::new(),
            opaque_fused_iterations: vec![OpaqueFusedIterationPlanRequest { plan: candidate }],
            indexed_fields: Vec::new(),
            indexed_globals: Vec::new(),
        }],
    };
    let planned = plan_module_optimization_v3(&AlternativeCatalog::default_v3(), request);
    let mechanical = emit_mechanical_plan_v3(&planned)
        .map_err(|error| format!("tracked N-Queens mechanical plan emission failed: {error}"))?;
    let [function] = mechanical.functions.as_slice() else {
        return Err(format!(
            "tracked N-Queens mechanical plan emitted {} functions, expected exactly one",
            mechanical.functions.len()
        ));
    };
    if function.function != root_function {
        return Err(format!(
            "tracked N-Queens mechanical plan targeted {:?}, expected {root_function:?}",
            function.function
        ));
    }
    let [emission] = function.opaque_fused_iterations.as_slice() else {
        return Err(format!(
            "tracked N-Queens mechanical plan emitted {} opaque iterations, expected exactly one",
            function.opaque_fused_iterations.len()
        ));
    };
    Ok(emission.clone())
}

pub(super) fn admit_tracked_nqueens_count(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    exact_source_match: bool,
) -> Result<Option<AdmittedOpaqueFusedCount>, String> {
    if !exact_source_match {
        return Ok(None);
    }
    let report = discover_opaque_fused_count_graphs_with_cleanup_policy(module, true);
    if !report.rejections.is_empty() {
        return Err(format!(
            "tracked N-Queens opaque graph discovery rejected candidates: {:?}",
            report.rejections
        ));
    }
    let [graph] = report.graphs.as_slice() else {
        return Err(format!(
            "tracked N-Queens source produced {} Count graphs, expected exactly one",
            report.graphs.len()
        ));
    };
    validate_tracked_nqueens_graph_shape(module, graph)?;
    let emission = plan_and_emit_tracked_nqueens(graph)?;
    let typed_plan = resolve_typed_plan(module, graph, &emission)?;
    Ok(Some(AdmittedOpaqueFusedCount {
        root_function_id: graph.root_function_id,
        graph: graph.clone(),
        emission,
        typed_plan,
    }))
}

pub(super) fn attach_admitted_opaque_fused_count(
    module: &mut BlockPyModule<TypedBlockPyModuleShape>,
    admission: &AdmittedOpaqueFusedCount,
) -> Result<(), String> {
    let function = module
        .callable_defs
        .iter_mut()
        .find(|function| function.function_id == admission.root_function_id)
        .ok_or_else(|| {
            format!(
                "opaque fused root function {:?} disappeared before sidecar attachment",
                admission.root_function_id
            )
        })?;
    struct Attacher<'a> {
        source: InstrId,
        plan: &'a TypedOpaqueFusedIterationPlan,
        attached: usize,
        error: Option<String>,
    }
    impl VisitMut<InstrTyped> for Attacher<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            if expr.try_semantic_instr_id() == Some(self.source) {
                let Some(extra) = expr.typed_extra_mut() else {
                    self.error = Some(format!(
                        "opaque fused root source {} has no typed metadata",
                        self.source
                    ));
                    return;
                };
                extra.set_opaque_fused_iteration_plan(self.plan.clone());
                self.attached += 1;
            }
            expr.visit_children_mut(self);
        }
    }
    let mut attacher = Attacher {
        source: admission.emission.source,
        plan: &admission.typed_plan,
        attached: 0,
        error: None,
    };
    attacher.visit_fn_mut(function);
    if let Some(error) = attacher.error {
        return Err(error);
    }
    if attacher.attached != 1 {
        return Err(format!(
            "opaque fused root source {} attached to {} expressions, expected exactly one",
            admission.emission.source, attacher.attached
        ));
    }
    Ok(())
}

pub(super) fn validate_attached_opaque_fused_count_is_atomic(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    admission: &AdmittedOpaqueFusedCount,
) -> Result<(), String> {
    let function = function_for_id(module, admission.root_function_id)?;
    let entry_origin = admission
        .graph
        .stages
        .iter()
        .find(|stage| stage.id == admission.graph.entry_stage)
        .ok_or_else(|| "opaque fused atomic validation is missing the entry stage".to_string())?
        .origin
        .source;
    let protected_sources = HashSet::from([
        admission.emission.source,
        admission.graph.consume_source,
        entry_origin,
    ]);

    struct ProtectedSourceFinder<'a> {
        protected: &'a HashSet<InstrId>,
        found: Vec<InstrId>,
    }
    impl Visit<InstrTyped> for ProtectedSourceFinder<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let Some(source) = expr.try_semantic_instr_id()
                && self.protected.contains(&source)
            {
                self.found.push(source);
            }
            expr.visit_children(self);
        }
    }

    let validate_host = |value: &InstrTyped, is_return: bool| -> Result<(), String> {
        if value.try_semantic_instr_id() != Some(admission.emission.source) {
            return Err("opaque fused atomic host source changed".to_string());
        }
        if value.opaque_fused_iteration_plan() != Some(&admission.typed_plan) {
            return Err(
                "opaque fused atomic host does not carry the admitted typed sidecar".to_string(),
            );
        }
        let materialize_expr = match (&admission.emission.sink, is_return) {
            (OpaqueFusedSinkPlan::Count { .. }, true) => {
                let outer = typed_call_parts(value).ok_or_else(|| {
                    "opaque fused atomic Count host is not a direct call".to_string()
                })?;
                if typed_call_runtime_name(outer.func, outer.access, &module.module_constants)
                    != Some(RuntimeName::Len)
                {
                    return Err("opaque fused atomic Count host is not len(...)".to_string());
                }
                let [CallArgPositional::Positional(materialize_expr)] = outer.args else {
                    return Err(
                        "opaque fused atomic len call does not have one positional argument"
                            .to_string(),
                    );
                };
                materialize_expr
            }
            (OpaqueFusedSinkPlan::Discard { .. }, false) => value,
            (OpaqueFusedSinkPlan::Count { .. }, false) => {
                return Err("opaque fused Count host is not the direct Return value".to_string());
            }
            (OpaqueFusedSinkPlan::Discard { .. }, true) => {
                return Err("opaque fused Discard host is not a direct body expression".to_string());
            }
        };
        if materialize_expr.try_semantic_instr_id() != Some(admission.graph.consume_source) {
            return Err(
                "opaque fused atomic host does not consume the original list call".to_string(),
            );
        }
        let materialize = typed_call_parts(materialize_expr)
            .ok_or_else(|| "opaque fused atomic materializer is not list(...)".to_string())?;
        if typed_call_runtime_name(
            materialize.func,
            materialize.access,
            &module.module_constants,
        ) != Some(RuntimeName::List)
        {
            return Err("opaque fused atomic materializer is not the list builtin".to_string());
        }
        let [CallArgPositional::Positional(producer)] = materialize.args else {
            return Err(
                "opaque fused atomic list call does not have one positional argument".to_string(),
            );
        };
        if producer.try_semantic_instr_id() != Some(entry_origin) {
            return Err(
                "opaque fused atomic list argument is not the original n_queens call".to_string(),
            );
        }
        let producer = typed_call_parts(producer)
            .ok_or_else(|| "opaque fused atomic producer is not a direct call".to_string())?;
        if !matches!(
            producer.func,
            InstrTyped::Load(load) if load.name.id_str() == "n_queens"
        ) {
            return Err("opaque fused atomic producer callable is not n_queens".to_string());
        }
        let [CallArgPositional::Positional(InstrTyped::Load(width))] = producer.args else {
            return Err(
                "opaque fused atomic n_queens call does not directly load its width".to_string(),
            );
        };
        if width.name != admission.typed_plan.width_input {
            return Err(
                "opaque fused atomic n_queens width does not match the admitted local".to_string(),
            );
        }
        if materialize
            .args
            .first()
            .and_then(|arg| arg.expr().try_semantic_instr_id())
            != Some(entry_origin)
        {
            return Err("opaque fused atomic nested source identities changed".to_string());
        }
        Ok(())
    };

    let mut host_count = 0usize;
    for block in &function.blocks {
        for instr in &block.body {
            if instr.try_semantic_instr_id() == Some(admission.emission.source) {
                host_count += 1;
                validate_host(instr, false)?;
                continue;
            }
            let mut finder = ProtectedSourceFinder {
                protected: &protected_sources,
                found: Vec::new(),
            };
            finder.visit_instr(instr);
            if !finder.found.is_empty() {
                return Err(format!(
                    "opaque fused root is not atomic: protected nested calls {:?} execute in prefix body block {:?}",
                    finder.found, block.label
                ));
            }
        }
        let BlockTerm::Return(value) = &block.term else {
            continue;
        };
        if value.try_semantic_instr_id() == Some(admission.emission.source) {
            host_count += 1;
            validate_host(value, true)?;
            continue;
        }
        let mut finder = ProtectedSourceFinder {
            protected: &protected_sources,
            found: Vec::new(),
        };
        finder.visit_instr(value);
        if !finder.found.is_empty() {
            return Err(format!(
                "opaque fused root is not atomic: protected nested calls {:?} execute in non-host Return block {:?}",
                finder.found, block.label
            ));
        }
    }
    if host_count != 1 {
        return Err(format!(
            "opaque fused atomic root has {host_count} direct hosts, expected one"
        ));
    }
    Ok(())
}

#[derive(Clone)]
struct RootCountCandidate {
    result: TypedCall<InstrTyped>,
    sink: OpaqueFusedRootSink,
    materialize: TypedCall<InstrTyped>,
    producer: TypedCall<InstrTyped>,
    producer_plan: TypedGeneratorInstancePlan,
}

#[derive(Clone, Debug)]
struct DiscoveredProducer {
    site: OpaqueFusedDiscoverySite,
    plan: TypedGeneratorInstancePlan,
    fresh_closure_callable: bool,
    has_keywords: bool,
    has_starred_arguments: bool,
}

#[derive(Clone, Debug, Default)]
struct FunctionProducerAnalysis {
    producers: HashMap<InstrId, DiscoveredProducer>,
    consumers: HashMap<InstrId, Vec<OpaqueFusedDiscoveryConsumer>>,
    unknown_uses: HashMap<InstrId, Vec<OpaqueFusedDiscoverySite>>,
    builtin_sites: Vec<(OpaqueFusedDiscoverySite, RuntimeName)>,
}

struct FunctionUseCollector<'a> {
    function_id: RuntimeFunctionId,
    module_constants: &'a [ConstantExpr],
    state: &'a TrustedOwnerState,
    analysis: &'a mut FunctionProducerAnalysis,
}

impl FunctionUseCollector<'_> {
    fn origin_for_expr(&self, expr: &InstrTyped) -> Option<InstrId> {
        if expr.generator_instance_plan().is_some() {
            return expr.try_semantic_instr_id();
        }
        let InstrTyped::Load(load) = expr else {
            return None;
        };
        trusted_object_origin_for_name(&load.name, self.state)
    }

    fn visit_call_arg_expr_without_consuming(&mut self, expr: &InstrTyped) {
        self.visit_instr(expr);
    }

    fn record_builtin_site(&mut self, source: InstrId, runtime_name: RuntimeName) {
        let site = OpaqueFusedDiscoverySite {
            function_id: self.function_id,
            source,
        };
        if !self.analysis.builtin_sites.contains(&(site, runtime_name)) {
            self.analysis.builtin_sites.push((site, runtime_name));
        }
    }

    fn record_skipped_body_dependency(&mut self, expr: &InstrTyped) {
        let Some(parts) = typed_call_parts(expr) else {
            return;
        };
        let Some(runtime_name) =
            typed_call_runtime_name(parts.func, parts.access, self.module_constants)
        else {
            return;
        };
        if !matches!(
            runtime_name,
            RuntimeName::Range
                | RuntimeName::Len
                | RuntimeName::List
                | RuntimeName::Tuple
                | RuntimeName::Reversed
                | RuntimeName::Set
        ) {
            return;
        }
        if let Some(source) = expr.try_semantic_instr_id() {
            self.record_builtin_site(source, runtime_name);
        }
    }

    fn maybe_visit_consumer(&mut self, expr: &InstrTyped) -> bool {
        let Some(parts) = typed_call_parts(expr) else {
            return false;
        };
        let Some(runtime_name) =
            typed_call_runtime_name(parts.func, parts.access, self.module_constants)
        else {
            return false;
        };
        if !matches!(
            runtime_name,
            RuntimeName::Iter
                | RuntimeName::Next
                | RuntimeName::List
                | RuntimeName::Tuple
                | RuntimeName::Set
        ) || parts.has_keywords
        {
            return false;
        }
        let [CallArgPositional::Positional(receiver)] = parts.args else {
            return false;
        };
        let Some(origin) = self.origin_for_expr(receiver) else {
            return false;
        };
        let Some(source) = expr.try_semantic_instr_id() else {
            self.analysis
                .unknown_uses
                .entry(origin)
                .or_default()
                .push(OpaqueFusedDiscoverySite {
                    function_id: self.function_id,
                    source: InstrId::new(u32::MAX),
                });
            return true;
        };
        self.record_builtin_site(source, runtime_name);
        if runtime_name != RuntimeName::Iter {
            let kind = OpaqueFusedDiscoveryConsumerKind::from_runtime_name(runtime_name)
                .expect("selected generator consumer runtime name should have a kind");
            self.analysis
                .consumers
                .entry(origin)
                .or_default()
                .push(OpaqueFusedDiscoveryConsumer {
                    site: OpaqueFusedDiscoverySite {
                        function_id: self.function_id,
                        source,
                    },
                    kind,
                    runtime_name,
                });
        }
        self.visit_instr(parts.func);
        if receiver.generator_instance_plan().is_some() {
            self.visit_call_arg_expr_without_consuming(receiver);
        }
        true
    }
}

impl Visit<InstrTyped> for FunctionUseCollector<'_> {
    fn visit_instr(&mut self, expr: &InstrTyped) {
        if let Some(plan) = expr.generator_instance_plan() {
            let Some(source) = expr.try_semantic_instr_id() else {
                return;
            };
            let Some(parts) = typed_call_parts(expr) else {
                return;
            };
            self.analysis
                .producers
                .entry(source)
                .or_insert_with(|| DiscoveredProducer {
                    site: OpaqueFusedDiscoverySite {
                        function_id: self.function_id,
                        source,
                    },
                    plan: plan.clone(),
                    fresh_closure_callable: matches!(
                        parts.func,
                        InstrTyped::MakeFunctionWithClosure(_)
                    ),
                    has_keywords: parts.has_keywords,
                    has_starred_arguments: parts
                        .args
                        .iter()
                        .any(|arg| matches!(arg, CallArgPositional::Starred(_))),
                });
            self.visit_instr(parts.func);
            for arg in parts.args {
                self.visit_instr(arg.expr());
            }
            return;
        }
        self.record_skipped_body_dependency(expr);
        if self.maybe_visit_consumer(expr) {
            return;
        }
        if let InstrTyped::Load(load) = expr
            && let Some(origin) = trusted_object_origin_for_name(&load.name, self.state)
            && let Some(source) = expr.try_semantic_instr_id()
        {
            self.analysis
                .unknown_uses
                .entry(origin)
                .or_default()
                .push(OpaqueFusedDiscoverySite {
                    function_id: self.function_id,
                    source,
                });
            return;
        }
        expr.visit_children(self);
    }
}

struct TypedCallParts<'a> {
    func: &'a InstrTyped,
    args: &'a [CallArgPositional<InstrTyped>],
    has_keywords: bool,
    access: Option<&'a soac_ir_typed::TypedCallAccessPlan>,
}

fn typed_call_parts(expr: &InstrTyped) -> Option<TypedCallParts<'_>> {
    match expr {
        InstrTyped::CallTyped(call) => Some(TypedCallParts {
            func: call.func.as_ref(),
            args: &call.args,
            has_keywords: !call.keywords.is_empty(),
            access: Some(&call.access),
        }),
        InstrTyped::GuardedCallableCallTyped(call) => Some(TypedCallParts {
            func: call.func.as_ref(),
            args: &call.args,
            has_keywords: !call.keywords.is_empty(),
            access: None,
        }),
        InstrTyped::DirectCallableCallTyped(call) => Some(TypedCallParts {
            func: call.func.as_ref(),
            args: &call.args,
            has_keywords: false,
            access: None,
        }),
        _ => None,
    }
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

fn typed_call_runtime_name(
    func: &InstrTyped,
    access: Option<&soac_ir_typed::TypedCallAccessPlan>,
    module_constants: &[ConstantExpr],
) -> Option<RuntimeName> {
    if let Some(soac_ir_typed::TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
        runtime_name,
        ..
    }) = access
    {
        return Some(*runtime_name);
    }
    RuntimeName::ALL
        .iter()
        .copied()
        .find(|runtime_name| typed_expr_is_runtime_name_load(func, *runtime_name, module_constants))
}

fn root_materialize_parts(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<(
    TypedCall<InstrTyped>,
    TypedCall<InstrTyped>,
    TypedGeneratorInstancePlan,
)> {
    let InstrTyped::CallTyped(materialize) = expr else {
        return None;
    };
    if !materialize.keywords.is_empty()
        || !typed_expr_is_runtime_name_load(
            materialize.func.as_ref(),
            RuntimeName::List,
            module_constants,
        )
    {
        return None;
    }
    let [CallArgPositional::Positional(InstrTyped::CallTyped(producer))] =
        materialize.args.as_slice()
    else {
        return None;
    };
    if !producer.keywords.is_empty() {
        return None;
    }
    let producer_plan = producer.extra.generator_instance_plan()?.clone();
    (producer_plan.kind == FunctionKind::Generator)
        .then(|| (materialize.clone(), producer.clone(), producer_plan))
}

fn root_count_parts(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<RootCountCandidate> {
    let InstrTyped::CallTyped(count) = expr else {
        return None;
    };
    if !count.keywords.is_empty()
        || !typed_expr_is_runtime_name_load(count.func.as_ref(), RuntimeName::Len, module_constants)
    {
        return None;
    }
    let [CallArgPositional::Positional(materialize_expr)] = count.args.as_slice() else {
        return None;
    };
    let (materialize, producer, producer_plan) =
        root_materialize_parts(materialize_expr, module_constants)?;
    Some(RootCountCandidate {
        result: count.clone(),
        sink: OpaqueFusedRootSink::Count,
        materialize,
        producer,
        producer_plan,
    })
}

fn root_discard_parts(
    expr: &InstrTyped,
    module_constants: &[ConstantExpr],
) -> Option<RootCountCandidate> {
    let (materialize, producer, producer_plan) = root_materialize_parts(expr, module_constants)?;
    Some(RootCountCandidate {
        result: materialize.clone(),
        sink: OpaqueFusedRootSink::Discard,
        materialize,
        producer,
        producer_plan,
    })
}

fn root_count_candidates(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> Vec<RootCountCandidate> {
    let mut candidates = Vec::new();
    for block in &function.blocks {
        for instr in &block.body {
            match instr {
                InstrTyped::Store(store) => {
                    if let Some(candidate) =
                        root_count_parts(store.value.as_ref(), module_constants)
                    {
                        candidates.push(candidate);
                    }
                }
                other => {
                    if let Some(candidate) = root_count_parts(other, module_constants)
                        .or_else(|| root_discard_parts(other, module_constants))
                    {
                        candidates.push(candidate);
                    }
                }
            }
        }
        if let BlockTerm::Return(value) = &block.term
            && let Some(candidate) = root_count_parts(value, module_constants)
        {
            candidates.push(candidate);
        }
    }
    candidates
}

fn analyze_function_producers(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> FunctionProducerAnalysis {
    let empty_constructor_calls = HashMap::new();
    let empty_constructor_owners = HashMap::new();
    let states = analyze_trusted_owner_states(
        function,
        module_constants,
        &empty_constructor_calls,
        &empty_constructor_owners,
    );
    let mut analysis = FunctionProducerAnalysis::default();
    for block in &function.blocks {
        if !states.reachable_blocks.contains(block.label) {
            continue;
        }
        for (instr_index, instr) in block.body.iter().enumerate() {
            let Some(state) = states.body_before_instr.get(&TypedVirtualBodyInstr {
                block: block.label,
                instr_index,
            }) else {
                continue;
            };
            match instr {
                InstrTyped::Store(store) => {
                    if matches!(store.value.as_ref(), InstrTyped::Load(load) if trusted_object_origin_for_name(&load.name, state).is_some())
                    {
                        continue;
                    }
                    let mut collector = FunctionUseCollector {
                        function_id: function.function_id,
                        module_constants,
                        state,
                        analysis: &mut analysis,
                    };
                    collector.visit_instr(store.value.as_ref());
                }
                InstrTyped::Del(del)
                    if trusted_object_origin_for_name(&del.name, state).is_some() => {}
                other => {
                    let mut collector = FunctionUseCollector {
                        function_id: function.function_id,
                        module_constants,
                        state,
                        analysis: &mut analysis,
                    };
                    collector.visit_instr(other);
                }
            }
        }
        let Some(state) = states.block_before_term.get(&block.label) else {
            continue;
        };
        struct TermCollector<'a, 'b> {
            inner: &'a mut FunctionUseCollector<'b>,
        }
        impl Visit<InstrTyped> for TermCollector<'_, '_> {
            fn visit_instr(&mut self, expr: &InstrTyped) {
                self.inner.visit_instr(expr);
            }
        }
        let mut inner = FunctionUseCollector {
            function_id: function.function_id,
            module_constants,
            state,
            analysis: &mut analysis,
        };
        let mut collector = TermCollector { inner: &mut inner };
        visit_trusted_owner_term_instrs(&block.term, &mut collector);
    }
    analysis.builtin_sites.sort_by_key(|(site, runtime_name)| {
        (
            site.function_id.to_packed_runtime_u64(),
            site.source.index(),
            runtime_name.id(),
        )
    });
    analysis
}

fn function_has_yield_from(
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
            if let Some(parts) = typed_call_parts(expr)
                && typed_call_runtime_name(parts.func, parts.access, self.module_constants)
                    == Some(RuntimeName::Next)
                && matches!(
                    parts.args,
                    [CallArgPositional::Positional(InstrTyped::Load(load))]
                        if load.name.id_str().contains("_dp_yieldfrom")
                )
            {
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

fn function_has_source_cleanup(function: &BlockPyFunction<TypedBlockPyModuleShape>) -> bool {
    function.storage_layout.as_ref().is_some_and(|layout| {
        layout.stack_slots().iter().any(|name| {
            name.contains("_dp_try_abrupt_kind") || name.contains("_dp_try_abrupt_payload")
        })
    })
}

fn generator_boundaries(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    module_constants: &[ConstantExpr],
) -> (Vec<InstrId>, Vec<BlockLabel>) {
    let mut yields = Vec::new();
    let mut completions = Vec::new();
    for block in &function.blocks {
        match &block.term {
            BlockTerm::Return(value) => {
                if let Some(source) = value.try_semantic_instr_id() {
                    yields.push(source);
                }
            }
            BlockTerm::Raise(raise)
                if block.exc_edge.is_none()
                    && raise.exc.as_ref().is_some_and(|exc| {
                        typed_call_parts(exc).is_some_and(|parts| {
                            typed_call_runtime_name(parts.func, parts.access, module_constants)
                                == Some(RuntimeName::StopIteration)
                        })
                    }) =>
            {
                completions.push(block.label);
            }
            _ => {}
        }
    }
    yields.sort_by_key(|source| source.index());
    yields.dedup();
    completions.sort();
    completions.dedup();
    (yields, completions)
}

fn validate_generator_arg_plan(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    producer: &DiscoveredProducer,
) -> Result<(), OpaqueFusedRejectionReason> {
    let site = producer.site;
    if producer.has_keywords {
        return Err(OpaqueFusedRejectionReason::UnsupportedKeywordArguments(
            site,
        ));
    }
    if producer.has_starred_arguments {
        return Err(OpaqueFusedRejectionReason::UnsupportedStarredArguments(
            site,
        ));
    }
    for (param, source) in function.params.iter().zip(&producer.plan.arg_plan.sources) {
        if matches!(source, TypedDirectCallArgSource::DefaultSentinel)
            && param.kind == ParamKind::KwOnly
        {
            return Err(OpaqueFusedRejectionReason::KeywordOnlyDefault {
                function_id: function.function_id,
                param: param.name.clone(),
            });
        }
    }
    Ok(())
}

struct GraphBuilder<'a> {
    module: &'a BlockPyModule<TypedBlockPyModuleShape>,
    allow_lowered_cleanup_scaffolding: bool,
    functions: HashMap<RuntimeFunctionId, &'a BlockPyFunction<TypedBlockPyModuleShape>>,
    analyses: HashMap<RuntimeFunctionId, FunctionProducerAnalysis>,
    stages: Vec<OpaqueFusedDiscoveryStage>,
    builtin_sites: Vec<(OpaqueFusedDiscoverySite, RuntimeName)>,
    active_functions: HashSet<RuntimeFunctionId>,
}

impl<'a> GraphBuilder<'a> {
    fn new(
        module: &'a BlockPyModule<TypedBlockPyModuleShape>,
        allow_lowered_cleanup_scaffolding: bool,
    ) -> Self {
        let functions = module
            .callable_defs
            .iter()
            .map(|function| (function.function_id, function))
            .collect::<HashMap<_, _>>();
        let analyses = module
            .callable_defs
            .iter()
            .map(|function| {
                (
                    function.function_id,
                    analyze_function_producers(function, &module.module_constants),
                )
            })
            .collect::<HashMap<_, _>>();
        Self {
            module,
            allow_lowered_cleanup_scaffolding,
            functions,
            analyses,
            stages: Vec::new(),
            builtin_sites: Vec::new(),
            active_functions: HashSet::new(),
        }
    }

    fn add_stage(
        &mut self,
        producer: DiscoveredProducer,
        parent: Option<OpaqueFusedStageId>,
        consumer: OpaqueFusedDiscoveryConsumer,
    ) -> Result<OpaqueFusedStageId, OpaqueFusedRejectionReason> {
        let callee = self
            .functions
            .get(&producer.plan.function_id)
            .copied()
            .ok_or(OpaqueFusedRejectionReason::MissingGeneratorFunction(
                producer.plan.function_id,
            ))?;
        if callee.kind != FunctionKind::Generator {
            return Err(OpaqueFusedRejectionReason::NonGeneratorFunction(
                producer.plan.function_id,
            ));
        }
        validate_generator_arg_plan(callee, &producer)?;
        if producer.plan.arg_plan.sources.len() != callee.params.len() {
            return Err(OpaqueFusedRejectionReason::ArgPlanArityMismatch {
                function_id: callee.function_id,
                params: callee.params.len(),
                sources: producer.plan.arg_plan.sources.len(),
            });
        }
        let positional_defaults = callee
            .params
            .iter_with_default_sources()
            .zip(&producer.plan.arg_plan.sources)
            .enumerate()
            .filter_map(|(parameter_index, ((_, default_source), arg_source))| {
                matches!(arg_source, TypedDirectCallArgSource::DefaultSentinel)
                    .then_some((parameter_index, default_source))
            })
            .map(|(parameter_index, default_source)| {
                let Some(ParamDefaultSource::Positional(default_index)) = default_source else {
                    return Err(OpaqueFusedRejectionReason::MissingPositionalDefault {
                        function_id: callee.function_id,
                        parameter_index,
                    });
                };
                Ok(OpaqueFusedPositionalDefault {
                    parameter_index: u32::try_from(parameter_index).map_err(|_| {
                        OpaqueFusedRejectionReason::ArgIndexOverflow(parameter_index)
                    })?,
                    default_index: u32::try_from(default_index)
                        .map_err(|_| OpaqueFusedRejectionReason::ArgIndexOverflow(default_index))?,
                    expected_defaults_len: u32::try_from(
                        callee
                            .params
                            .iter_with_default_sources()
                            .filter(|(_, source)| {
                                matches!(source, Some(ParamDefaultSource::Positional(_)))
                            })
                            .count(),
                    )
                    .map_err(|_| {
                        OpaqueFusedRejectionReason::ArgIndexOverflow(callee.params.len())
                    })?,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if function_has_yield_from(callee, &self.module.module_constants) {
            return Err(OpaqueFusedRejectionReason::YieldFrom(callee.function_id));
        }
        if !self.allow_lowered_cleanup_scaffolding && function_has_source_cleanup(callee) {
            return Err(OpaqueFusedRejectionReason::SourceCleanup(
                callee.function_id,
            ));
        }
        if !self.active_functions.insert(callee.function_id) {
            return Err(OpaqueFusedRejectionReason::ProducerCycle(
                callee.function_id,
            ));
        }
        let (yield_sources, completion_blocks) =
            generator_boundaries(callee, &self.module.module_constants);
        if yield_sources.is_empty() {
            self.active_functions.remove(&callee.function_id);
            return Err(OpaqueFusedRejectionReason::MissingYield(callee.function_id));
        }
        if completion_blocks.is_empty() {
            self.active_functions.remove(&callee.function_id);
            return Err(OpaqueFusedRejectionReason::MissingCompletion(
                callee.function_id,
            ));
        }

        let id = OpaqueFusedStageId(
            u32::try_from(self.stages.len()).expect("opaque fused stage count should fit in a u32"),
        );
        self.stages.push(OpaqueFusedDiscoveryStage {
            id,
            parent,
            origin: producer.site,
            function_id: producer.plan.function_id,
            arg_plan: producer.plan.arg_plan.clone(),
            positional_defaults,
            yield_sources,
            completion_blocks,
            consumer,
            // Lowering may store a freshly created genexpr function and load
            // it at the call site, hiding the immediate MakeFunction shape.
            // The synthetic <genexpr> callable is nevertheless fresh within
            // its owning invocation and has no root-visible identity to guard.
            fresh_closure_callable: producer.fresh_closure_callable
                || callee.names.display_name == "<genexpr>",
        });

        let analysis = self
            .analyses
            .get(&callee.function_id)
            .cloned()
            .unwrap_or_default();
        self.builtin_sites
            .extend(analysis.builtin_sites.iter().copied());
        let mut children = analysis.producers.values().cloned().collect::<Vec<_>>();
        children.sort_by_key(|child| child.site);
        for child in children {
            if analysis.unknown_uses.contains_key(&child.site.source) {
                self.active_functions.remove(&callee.function_id);
                return Err(OpaqueFusedRejectionReason::EscapedProducer(child.site));
            }
            let consumers = analysis
                .consumers
                .get(&child.site.source)
                .cloned()
                .unwrap_or_default();
            let child_consumer = match consumers.as_slice() {
                [] => {
                    self.active_functions.remove(&callee.function_id);
                    return Err(OpaqueFusedRejectionReason::MissingConsumer(child.site));
                }
                [consumer] => consumer.clone(),
                consumers => {
                    self.active_functions.remove(&callee.function_id);
                    return Err(OpaqueFusedRejectionReason::MultipleConsumers {
                        producer: child.site,
                        consumers: consumers.iter().map(|consumer| consumer.site).collect(),
                    });
                }
            };
            if child_consumer.kind == OpaqueFusedDiscoveryConsumerKind::BuildList {
                self.active_functions.remove(&callee.function_id);
                return Err(OpaqueFusedRejectionReason::UnsupportedNestedListConsumer(
                    child_consumer.site,
                ));
            }
            self.add_stage(child, Some(id), child_consumer)?;
        }
        self.active_functions.remove(&callee.function_id);
        Ok(id)
    }
}

#[cfg(test)]
pub(super) fn discover_opaque_fused_count_graphs(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
) -> OpaqueFusedDiscoveryReport {
    discover_opaque_fused_count_graphs_with_cleanup_policy(module, false)
}

/// Exact pinned sources contain lowering-generated abrupt-state slots for
/// ordinary generator loops despite having no source `finally` or `with`.
/// Generic discovery remains conservative; exact-source admission alone may
/// permit that scaffolding.
fn discover_opaque_fused_count_graphs_with_cleanup_policy(
    module: &BlockPyModule<TypedBlockPyModuleShape>,
    allow_lowered_cleanup_scaffolding: bool,
) -> OpaqueFusedDiscoveryReport {
    let mut report = OpaqueFusedDiscoveryReport::default();
    for function in &module.callable_defs {
        for candidate in root_count_candidates(function, &module.module_constants) {
            let result_source = candidate.result.try_semantic_instr_id();
            let consume_source = candidate.materialize.try_semantic_instr_id();
            let producer_source = candidate.producer.try_semantic_instr_id();
            let source = result_source;
            let make_rejection = |reason| OpaqueFusedRejection {
                root_function_id: function.function_id,
                source,
                reason,
            };
            let (Some(result_source), Some(consume_source), Some(producer_source)) =
                (result_source, consume_source, producer_source)
            else {
                report.rejections.push(make_rejection(
                    OpaqueFusedRejectionReason::MissingSemanticInstrId("count root"),
                ));
                continue;
            };
            let Some(width_param_index) = root_width_param_index(function, &candidate) else {
                report
                    .rejections
                    .push(make_rejection(OpaqueFusedRejectionReason::WidthInputShape));
                continue;
            };
            let root_site = OpaqueFusedDiscoverySite {
                function_id: function.function_id,
                source: producer_source,
            };
            let root_consumer = OpaqueFusedDiscoveryConsumer {
                site: OpaqueFusedDiscoverySite {
                    function_id: function.function_id,
                    source: consume_source,
                },
                kind: OpaqueFusedDiscoveryConsumerKind::BuildList,
                runtime_name: RuntimeName::List,
            };
            let root_producer = DiscoveredProducer {
                site: root_site,
                plan: candidate.producer_plan.clone(),
                fresh_closure_callable: matches!(
                    candidate.producer.func.as_ref(),
                    InstrTyped::MakeFunctionWithClosure(_)
                ),
                has_keywords: !candidate.producer.keywords.is_empty(),
                has_starred_arguments: candidate
                    .producer
                    .args
                    .iter()
                    .any(|arg| matches!(arg, CallArgPositional::Starred(_))),
            };
            let mut builder = GraphBuilder::new(module, allow_lowered_cleanup_scaffolding);
            if candidate.sink == OpaqueFusedRootSink::Count {
                builder.builtin_sites.push((
                    OpaqueFusedDiscoverySite {
                        function_id: function.function_id,
                        source: result_source,
                    },
                    RuntimeName::Len,
                ));
            }
            builder
                .builtin_sites
                .push((root_consumer.site, RuntimeName::List));
            match builder.add_stage(root_producer, None, root_consumer) {
                Ok(entry_stage) => {
                    builder.builtin_sites.sort_by_key(|(site, runtime_name)| {
                        (
                            site.function_id.to_packed_runtime_u64(),
                            site.source.index(),
                            runtime_name.id(),
                        )
                    });
                    builder.builtin_sites.dedup();
                    report.graphs.push(OpaqueFusedCountGraph {
                        root_function_id: function.function_id,
                        sink: candidate.sink,
                        result_source,
                        consume_source,
                        entry_stage,
                        width_param_index,
                        stages: builder.stages,
                        builtin_sites: builder.builtin_sites,
                    });
                }
                Err(reason) => report.rejections.push(make_rejection(reason)),
            }
        }
    }
    report
}

fn root_width_param_index(
    function: &BlockPyFunction<TypedBlockPyModuleShape>,
    candidate: &RootCountCandidate,
) -> Option<u32> {
    let [CallArgPositional::Positional(InstrTyped::Load(load))] =
        candidate.producer.args.as_slice()
    else {
        return None;
    };
    function
        .params
        .iter()
        .position(|param| param.name == load.name.id_str())
        .and_then(|index| u32::try_from(index).ok())
}

fn typed_arg_plan_to_v3(
    plan: &TypedDirectCallArgPlan,
) -> Result<DirectCallArgPlan, OpaqueFusedRejectionReason> {
    Ok(DirectCallArgPlan {
        sources: plan
            .sources
            .iter()
            .map(|source| match source {
                TypedDirectCallArgSource::Provided(index) => u32::try_from(*index)
                    .map(DirectCallArgSource::Provided)
                    .map_err(|_| OpaqueFusedRejectionReason::ArgIndexOverflow(*index)),
                TypedDirectCallArgSource::PackedRest { start } => u32::try_from(*start)
                    .map(|start| DirectCallArgSource::PackedRest { start })
                    .map_err(|_| OpaqueFusedRejectionReason::ArgIndexOverflow(*start)),
                TypedDirectCallArgSource::DefaultSentinel => {
                    Ok(DirectCallArgSource::DefaultSentinel)
                }
            })
            .collect::<Result<Vec<_>, _>>()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_core::block_py::VisitMut;
    use soac_ir_typed::{
        TypedDirectCallArgPlan, TypedDirectCallArgSource, assign_missing_typed_function_instr_ids,
        lower_blockpy_module_to_typed,
    };

    fn lower_annotated_generator_module(source: &str) -> BlockPyModule<TypedBlockPyModuleShape> {
        let lowered = soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("test source should lower");
        let mut module = lower_blockpy_module_to_typed(lowered.blockpy_module);
        for function in &mut module.callable_defs {
            assign_missing_typed_function_instr_ids(function);
        }
        let generator_targets = module
            .callable_defs
            .iter()
            .filter(|function| function.kind == FunctionKind::Generator)
            .map(|function| {
                (
                    function.names.bind_name.clone(),
                    (function.function_id, function.kind, function.params.len()),
                )
            })
            .collect::<HashMap<_, _>>();
        let generator_ids = generator_targets
            .values()
            .map(|(function_id, _, _)| *function_id)
            .collect::<HashSet<_>>();
        let generator_params = generator_targets
            .values()
            .map(|(function_id, _, params)| (*function_id, *params))
            .collect::<HashMap<_, _>>();

        struct Annotator<'a> {
            generator_targets: &'a HashMap<String, (RuntimeFunctionId, FunctionKind, usize)>,
            generator_ids: &'a HashSet<RuntimeFunctionId>,
            generator_params: &'a HashMap<RuntimeFunctionId, usize>,
        }
        impl VisitMut<InstrTyped> for Annotator<'_> {
            fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
                let InstrTyped::CallTyped(call) = expr else {
                    expr.visit_children_mut(self);
                    return;
                };
                let target = match call.func.as_ref() {
                    InstrTyped::Load(load) => {
                        self.generator_targets.get(load.name.id_str()).copied()
                    }
                    InstrTyped::MakeFunctionWithClosure(make_function)
                        if self.generator_ids.contains(&make_function.function_id()) =>
                    {
                        let function_id = make_function.function_id();
                        self.generator_params
                            .get(&function_id)
                            .copied()
                            .map(|params| (function_id, FunctionKind::Generator, params))
                    }
                    _ => None,
                };
                if let Some((function_id, kind, param_count)) = target
                    && call
                        .args
                        .iter()
                        .all(|arg| matches!(arg, CallArgPositional::Positional(_)))
                    && call.keywords.is_empty()
                    && call.args.len() <= param_count
                {
                    call.extra
                        .set_generator_instance_plan(TypedGeneratorInstancePlan {
                            function_id,
                            kind,
                            arg_plan: TypedDirectCallArgPlan {
                                sources: (0..param_count)
                                    .map(|index| {
                                        if index < call.args.len() {
                                            TypedDirectCallArgSource::Provided(index)
                                        } else {
                                            TypedDirectCallArgSource::DefaultSentinel
                                        }
                                    })
                                    .collect(),
                            },
                        });
                }
                expr.visit_children_mut(self);
            }
        }
        let mut annotator = Annotator {
            generator_targets: &generator_targets,
            generator_ids: &generator_ids,
            generator_params: &generator_params,
        };
        annotator.visit_module_mut(&mut module);
        module
    }

    fn function_by_qualname<'a>(
        module: &'a BlockPyModule<TypedBlockPyModuleShape>,
        qualname: &str,
    ) -> &'a BlockPyFunction<TypedBlockPyModuleShape> {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing test function {qualname}"))
    }

    #[test]
    fn discovers_complete_nqueens_producer_graph() {
        let module = lower_annotated_generator_module(TRACKED_NQUEENS_SOURCE);
        let report = discover_opaque_fused_count_graphs_with_cleanup_policy(&module, true);
        assert!(report.rejections.is_empty(), "{:?}", report.rejections);
        let graph = report
            .graphs
            .iter()
            .find(|graph| {
                function_by_qualname(&module, "full_nqueens_list_consumer").function_id
                    == graph.root_function_id
            })
            .expect("N-Queens count root should be discovered");
        assert_eq!(graph.stages.len(), 6);
        assert_eq!(graph.builtin_consumer_count(), 5);
        assert_eq!(graph.for_loop_consumer_count(), 1);
        assert_eq!(
            graph
                .stages
                .iter()
                .filter(|stage| stage.parent.is_none())
                .count(),
            1
        );
        assert!(graph.stages.iter().all(|stage| {
            !stage.yield_sources.is_empty() && !stage.completion_blocks.is_empty()
        }));
        let defaulted_stage = graph
            .stages
            .iter()
            .find(|stage| !stage.positional_defaults.is_empty())
            .expect("permutations should retain its omitted r default");
        assert_eq!(
            defaulted_stage.arg_plan.sources,
            vec![
                TypedDirectCallArgSource::Provided(0),
                TypedDirectCallArgSource::DefaultSentinel,
            ]
        );
        assert_eq!(
            defaulted_stage.positional_defaults,
            vec![OpaqueFusedPositionalDefault {
                parameter_index: 1,
                default_index: 0,
                expected_defaults_len: 1,
            }]
        );
        let width_input = OpaqueFusedGuardInput::FunctionParam {
            index: graph.width_param_index,
        };
        let plan = graph
            .to_plan(
                OpaqueFusedAlgorithmPlan::AffineDistinctPermutationCount {
                    width_input,
                    maximum_width: 16,
                },
                16,
                Cost::default(),
                |function_id| {
                    Some(soac_core::block_py::SerializedFunctionId::new(
                        soac_core::block_py::SerializedModuleId::new(0),
                        function_id.local_function_id(),
                    ))
                },
            )
            .expect("N-Queens graph should convert to the typed plan");
        assert!(plan.entry_guards.iter().any(|guard| {
            matches!(
                &guard.expectation,
                OpaqueFusedGuardExpectation::FunctionPositionalDefaultIdentity {
                    parameter_index: 1,
                    default_index: 0,
                    expected: RuntimeName::None,
                    ..
                }
            )
        }));
    }

    #[test]
    fn tracked_nqueens_benchmark_matches_independently_pinned_source() {
        assert_eq!(CURRENT_NQUEENS_BENCHMARK_SOURCE, TRACKED_NQUEENS_SOURCE);
        assert!(tracked_nqueens_source_matches(TRACKED_NQUEENS_SOURCE));
        assert!(tracked_nqueens_source_matches(
            TRACKED_PYPERFORMANCE_NQUEENS_SOURCE
        ));
    }

    #[test]
    fn admits_pyperformance_nqueens_body_discard_through_mechanical_plan() {
        let module = lower_annotated_generator_module(TRACKED_PYPERFORMANCE_NQUEENS_SOURCE);
        let report = discover_opaque_fused_count_graphs_with_cleanup_policy(&module, true);
        assert!(report.rejections.is_empty(), "{:?}", report.rejections);
        let [graph] = report.graphs.as_slice() else {
            panic!(
                "pyperformance N-Queens should produce one graph, got {:#?}",
                report.graphs
            );
        };
        assert_eq!(graph.sink, OpaqueFusedRootSink::Discard);
        assert_eq!(
            function_by_qualname(&module, "bench_n_queens").function_id,
            graph.root_function_id
        );

        let admission = admit_tracked_nqueens_count(&module, true)
            .expect("pyperformance N-Queens admission should validate")
            .expect("the exact pinned pyperformance source should be admitted");
        assert!(matches!(
            admission.emission.sink,
            OpaqueFusedSinkPlan::Discard { .. }
        ));
        assert_eq!(admission.typed_plan.result, TypedOpaqueFusedResult::Discard);

        let mut attached = module.clone();
        attach_admitted_opaque_fused_count(&mut attached, &admission)
            .expect("the pyperformance Discard sidecar should attach to its body host");
        validate_attached_opaque_fused_count_is_atomic(&attached, &admission)
            .expect("the body-hosted list(n_queens(width)) sink should remain atomic");
    }

    #[test]
    fn admits_only_the_exact_tracked_nqueens_graph_with_complete_guards() {
        let module = lower_annotated_generator_module(TRACKED_NQUEENS_SOURCE);
        let admission = admit_tracked_nqueens_count(&module, true)
            .expect("tracked N-Queens admission should validate")
            .expect("the exact tracked source should be admitted");

        assert_eq!(admission.graph.stages.len(), 6);
        assert_eq!(admission.graph.builtin_consumer_count(), 5);
        assert_eq!(admission.graph.for_loop_consumer_count(), 1);
        assert!(matches!(
            &admission.emission.algorithm,
            OpaqueFusedAlgorithmPlan::AffineDistinctPermutationCount {
                maximum_width: TRACKED_NQUEENS_MAXIMUM_WIDTH,
                ..
            }
        ));
        assert_eq!(
            admission.typed_plan.maximum_width,
            i64::from(TRACKED_NQUEENS_MAXIMUM_WIDTH)
        );

        let guarded_body_dependencies = admission
            .emission
            .entry_guards
            .iter()
            .filter_map(|guard| match &guard.expectation {
                OpaqueFusedGuardExpectation::RuntimeBuiltinIdentity { runtime_name }
                    if matches!(
                        runtime_name,
                        RuntimeName::Range
                            | RuntimeName::Len
                            | RuntimeName::List
                            | RuntimeName::Tuple
                            | RuntimeName::Reversed
                            | RuntimeName::Set
                    ) =>
                {
                    Some(runtime_name.clone())
                }
                _ => None,
            })
            .collect::<HashSet<_>>();
        assert_eq!(
            guarded_body_dependencies,
            HashSet::from([
                RuntimeName::Range,
                RuntimeName::Len,
                RuntimeName::List,
                RuntimeName::Tuple,
                RuntimeName::Reversed,
                RuntimeName::Set,
            ])
        );

        let guarded_module_functions = admission
            .typed_plan
            .entry_guards
            .iter()
            .filter_map(|guard| match (&guard.operand, &guard.expectation) {
                (
                    TypedOpaqueFusedGuardOperand::IndexedGlobal { name, .. },
                    TypedOpaqueFusedGuardExpectation::FunctionIdentity { .. }
                    | TypedOpaqueFusedGuardExpectation::FunctionPositionalDefaultIdentity { .. },
                ) => Some(name.as_str()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        assert_eq!(
            guarded_module_functions,
            HashSet::from(["n_queens", "permutations"])
        );
        assert!(admission.typed_plan.entry_guards.iter().any(|guard| {
            matches!(
                guard,
                TypedOpaqueFusedEntryGuard {
                    operand: TypedOpaqueFusedGuardOperand::IndexedGlobal { name, .. },
                    expectation:
                        TypedOpaqueFusedGuardExpectation::FunctionPositionalDefaultIdentity {
                            default_index: 0,
                            expected: RuntimeName::None,
                            ..
                        },
                } if name == "permutations"
            )
        }));

        let mut attached = module.clone();
        attach_admitted_opaque_fused_count(&mut attached, &admission)
            .expect("the admitted sidecar should attach to the untouched root");
        validate_attached_opaque_fused_count_is_atomic(&attached, &admission)
            .expect("the untouched root should retain direct len(list(n_queens(width))) nesting");
    }

    #[test]
    fn rejects_same_topology_nqueens_graph_without_exact_source_equality() {
        let mutated_source = TRACKED_NQUEENS_SOURCE.replacen(
            "vec[i] + i for i in cols",
            "vec[i] + i + 1 for i in cols",
            1,
        );
        let module = lower_annotated_generator_module(mutated_source.as_str());
        assert!(tracked_nqueens_source_matches(TRACKED_NQUEENS_SOURCE));
        let exact_source_match = tracked_nqueens_source_matches(mutated_source.as_str());
        assert!(!exact_source_match);
        assert!(
            admit_tracked_nqueens_count(&module, exact_source_match)
                .expect("exact-source mismatch should be a clean ineligibility")
                .is_none()
        );
    }

    #[test]
    fn plan_conversion_keeps_exact_root_source_and_stage_owners() {
        let module = lower_annotated_generator_module(
            r#"
def values(width):
    yield width

def count_values(width):
    return len(list(values(width)))
"#,
        );
        let graph = discover_opaque_fused_count_graphs(&module)
            .graphs
            .pop()
            .expect("count graph should be discovered");
        let width_input = OpaqueFusedGuardInput::FunctionParam {
            index: graph.width_param_index,
        };
        let plan = graph
            .to_plan(
                OpaqueFusedAlgorithmPlan::AffineDistinctPermutationCount {
                    width_input,
                    maximum_width: 16,
                },
                16,
                Cost::default(),
                |function_id| {
                    Some(soac_core::block_py::SerializedFunctionId::new(
                        soac_core::block_py::SerializedModuleId::new(0),
                        function_id.local_function_id(),
                    ))
                },
            )
            .expect("graph should convert to the typed plan");
        assert_eq!(plan.source, graph.result_source);
        assert_eq!(plan.fallback.original_source, graph.result_source);
        assert!(matches!(
            plan.sink,
            OpaqueFusedSinkPlan::Count {
                consume_source,
                result_source,
            } if consume_source == graph.consume_source && result_source == graph.result_source
        ));
        assert_eq!(plan.stages.len(), graph.stages.len());
        assert_eq!(
            plan.stages
                .iter()
                .filter(|stage| stage.origin.owner == OpaqueFusedSiteOwner::Root)
                .count(),
            1
        );
    }
}
