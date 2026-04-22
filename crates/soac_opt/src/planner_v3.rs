use crate::alternatives_v3::{
    AlternativeCatalog, AlternativeId, FailureTargets, LoweringAlternative,
};
use crate::plan_v3::{
    CallBodyKind, CallBodyPlan, ConstructorCallFallbackKind, ConstructorCallFallbackPlan,
    ConstructorCallGuardKind, ConstructorCallGuardPlan, ConstructorCallOwnerType,
    ConstructorCallSpecializationPlan, ConversionKind, ConversionOwnership, ConversionPrecondition,
    ConvertNode, Cost, DirectCallArgPlan, DirectCallSpecializationPlan, ExactListItemAccessKind,
    ExactListItemFallbackKind, ExactListItemFallbackPlan, ExactListItemGuardKind,
    ExactListItemGuardPlan, ExactListItemShape, ExactListItemSpecializationPlan, FailureMode,
    FallbackReason, FallbackTarget, FunctionOptimizationPlanV3, FunctionOwnershipPlan,
    FunctionPlanIdentity, IndexedFieldAccessKind, IndexedFieldOwnerType,
    IndexedFieldSpecializationPlan, IndexedGlobalAccessKind, IndexedGlobalFallbackKind,
    IndexedGlobalFallbackPlan, IndexedGlobalGuardKind, IndexedGlobalGuardPlan,
    IndexedGlobalSpecializationPlan, MaterializeKind, MaterializeNode, MethodCallFallbackKind,
    MethodCallFallbackPlan, MethodCallGuardKind, MethodCallGuardPlan, MethodCallOwnerType,
    MethodCallSpecializationPlan, ModuleOptimizationPlanV3, ModulePlanIdentity, OperationNode,
    PlanDiagnostic, PlanNode, PlanNodeId, PlanNodeKind, PlanValue, PlannedConstant, RegionExitKind,
    RegionExitPlan, RegionExitTarget, RegionId, RegionInput, RegionInputSource, RegionPlan,
    RegionSource, RegionValueRef, Rep, ScalarLocalThreadPlan, ScalarThreadFallback,
    ScalarThreadLocal, ScalarThreadLocalCleanup, ScalarThreadLocalLocation, ScalarThreadLocalState,
    ScalarThreadMaterialization,
};
use crate::region_v3::{
    ExtractedExit, ExtractedRegion, ExtractedValue, ExtractedValueId, ExtractedValueKind,
};
use soac_core::block_py::{
    BinOpKind, InstrId, NameLike, NameLocation, ResolvedName, SerializedFunctionId,
    SerializedIdentityTables,
};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModulePlanRequest {
    pub module: ModulePlanIdentity,
    pub identity_tables: SerializedIdentityTables,
    pub functions: Vec<FunctionPlanRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionPlanRequest {
    pub function: FunctionPlanIdentity,
    pub regions: Vec<ExtractedRegionPlanRequest>,
    pub direct_calls: Vec<DirectCallPlanRequest>,
    pub constructor_calls: Vec<ConstructorCallPlanRequest>,
    pub method_calls: Vec<MethodCallPlanRequest>,
    pub exact_list_items: Vec<ExactListItemPlanRequest>,
    pub indexed_fields: Vec<IndexedFieldPlanRequest>,
    pub indexed_globals: Vec<IndexedGlobalPlanRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectCallPlanRequest {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub arg_plan: DirectCallArgPlan,
    pub body: CallBodyPlanRequest,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallBodyPlanRequest {
    pub alternatives: Vec<CallBodyAlternativeRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallBodyAlternativeRequest {
    pub kind: CallBodyKind,
    pub cost: Cost,
    pub reason: String,
}

impl CallBodyPlanRequest {
    pub fn with_inline_candidate(inline_candidate: bool) -> Self {
        let mut alternatives = vec![CallBodyAlternativeRequest {
            kind: CallBodyKind::DirectCall,
            cost: direct_call_body_cost_v3(),
            reason: "guarded direct call is the baseline validated call-body alternative"
                .to_string(),
        }];
        if inline_candidate {
            alternatives.push(CallBodyAlternativeRequest {
                kind: CallBodyKind::Inline,
                cost: inline_call_body_cost_v3(),
                reason: "lowered call shape is eligible for the early inline body alternative"
                    .to_string(),
            });
        }
        Self { alternatives }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstructorCallPlanRequest {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub owner_type: ConstructorCallOwnerType,
    pub arg_plan: DirectCallArgPlan,
    pub body: CallBodyPlanRequest,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodCallPlanRequest {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub method_name: String,
    pub owner_type: MethodCallOwnerType,
    pub arg_plan: DirectCallArgPlan,
    pub body: CallBodyPlanRequest,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactListItemPlanRequest {
    pub source: InstrId,
    pub access: ExactListItemAccessKind,
    pub shape: ExactListItemShape,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedFieldPlanRequest {
    pub source: InstrId,
    pub access: IndexedFieldAccessKind,
    pub owner_type: IndexedFieldOwnerType,
    pub attr_name: String,
    pub expected_index: u32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IndexedGlobalPlanRequest {
    pub source: InstrId,
    pub access: IndexedGlobalAccessKind,
    pub module_name: String,
    pub name: String,
    pub expected_index: u32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedRegionPlanRequest {
    pub region: ExtractedRegion,
    pub facts: PlannerFacts,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlannerFacts {
    exact_compact_int_values: HashSet<ExtractedValueId>,
    i64_constants: HashMap<ExtractedValueId, i64>,
}

impl PlannerFacts {
    pub fn mark_exact_compact_int(&mut self, value: ExtractedValueId) {
        self.exact_compact_int_values.insert(value);
    }

    pub fn set_i64_constant(&mut self, value: ExtractedValueId, constant: i64) {
        self.i64_constants.insert(value, constant);
    }

    pub fn is_exact_compact_int(&self, value: ExtractedValueId) -> bool {
        self.exact_compact_int_values.contains(&value)
    }

    pub fn i64_constant(&self, value: ExtractedValueId) -> Option<i64> {
        self.i64_constants.get(&value).copied()
    }
}

pub fn plan_module_optimization_v3(
    catalog: &AlternativeCatalog,
    request: ModulePlanRequest,
) -> ModuleOptimizationPlanV3 {
    ModuleOptimizationPlanV3 {
        module: request.module,
        identity_tables: request.identity_tables,
        helper_catalog_version: catalog.version,
        cost_model_version: 1,
        functions: request
            .functions
            .into_iter()
            .map(|function| plan_function_optimization_v3(catalog, function))
            .collect(),
    }
}

pub fn plan_function_optimization_v3(
    catalog: &AlternativeCatalog,
    request: FunctionPlanRequest,
) -> FunctionOptimizationPlanV3 {
    let region_requests = request.regions;
    let direct_calls = plan_direct_call_specializations_v3(&request.direct_calls);
    let constructor_calls = plan_constructor_call_specializations_v3(&request.constructor_calls);
    let method_calls = plan_method_call_specializations_v3(&request.method_calls);
    let exact_list_items = plan_exact_list_item_specializations_v3(&request.exact_list_items);
    let indexed_fields = plan_indexed_field_specializations_v3(&request.indexed_fields);
    let indexed_globals = plan_indexed_global_specializations_v3(&request.indexed_globals);
    let mut function = FunctionOptimizationPlanV3 {
        function: request.function,
        regions: Vec::new(),
        scalar_threads: Vec::new(),
        direct_calls,
        constructor_calls,
        method_calls,
        exact_list_items,
        indexed_fields,
        indexed_globals,
        deopt_points: Vec::new(),
        ownership: FunctionOwnershipPlan::default(),
        diagnostics: Vec::new(),
    };

    for region_request in &region_requests {
        let region = &region_request.region;
        match plan_compact_int_branch(catalog, region, &region_request.facts) {
            Ok(Some(planned_regions)) => function.regions.extend(planned_regions),
            Ok(None) => function.diagnostics.push(PlanDiagnostic {
                source: region.exit_source(),
                message: "v3 planner declined region: unsupported shape".to_string(),
            }),
            Err(error) => function.diagnostics.push(PlanDiagnostic {
                source: region.exit_source(),
                message: format!("v3 planner declined region: {error}"),
            }),
        }
    }

    function.scalar_threads = plan_scalar_local_threads_v3(&region_requests, &function.regions);
    function
}

fn plan_constructor_call_specializations_v3(
    constructor_call_requests: &[ConstructorCallPlanRequest],
) -> Vec<ConstructorCallSpecializationPlan> {
    let mut entries = constructor_call_requests.iter().collect::<Vec<_>>();
    entries.sort_by(|lhs, rhs| {
        (
            lhs.source,
            lhs.target,
            lhs.owner_type.module_name.as_str(),
            lhs.owner_type.qualname.as_str(),
        )
            .cmp(&(
                rhs.source,
                rhs.target,
                rhs.owner_type.module_name.as_str(),
                rhs.owner_type.qualname.as_str(),
            ))
    });
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for request in entries {
        if seen.insert((request.source, request.target, request.owner_type.clone())) {
            plans.push(ConstructorCallSpecializationPlan {
                source: request.source,
                target: request.target,
                owner_type: request.owner_type.clone(),
                arg_plan: request.arg_plan.clone(),
                guard: ConstructorCallGuardPlan {
                    kind: ConstructorCallGuardKind::ExactCallableTypeVersion,
                },
                fallback: ConstructorCallFallbackPlan {
                    kind: ConstructorCallFallbackKind::OriginalConstructorCall,
                },
                body: choose_call_body_plan_v3(&request.body),
                reason: request.reason.clone(),
            });
        }
    }
    plans
}

fn plan_method_call_specializations_v3(
    method_call_requests: &[MethodCallPlanRequest],
) -> Vec<MethodCallSpecializationPlan> {
    let mut entries = method_call_requests.iter().collect::<Vec<_>>();
    entries.sort_by(|lhs, rhs| {
        (
            lhs.source,
            lhs.target,
            lhs.method_name.as_str(),
            lhs.owner_type.module_name.as_str(),
            lhs.owner_type.qualname.as_str(),
        )
            .cmp(&(
                rhs.source,
                rhs.target,
                rhs.method_name.as_str(),
                rhs.owner_type.module_name.as_str(),
                rhs.owner_type.qualname.as_str(),
            ))
    });
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for request in entries {
        if seen.insert((
            request.source,
            request.target,
            request.method_name.clone(),
            request.owner_type.clone(),
        )) {
            plans.push(MethodCallSpecializationPlan {
                source: request.source,
                target: request.target,
                method_name: request.method_name.clone(),
                owner_type: request.owner_type.clone(),
                arg_plan: request.arg_plan.clone(),
                guard: MethodCallGuardPlan {
                    kind: MethodCallGuardKind::ExactReceiverTypeVersion,
                },
                fallback: MethodCallFallbackPlan {
                    kind: MethodCallFallbackKind::OriginalMethodCall,
                },
                body: choose_call_body_plan_v3(&request.body),
                reason: request.reason.clone(),
            });
        }
    }
    plans
}

fn plan_direct_call_specializations_v3(
    direct_call_requests: &[DirectCallPlanRequest],
) -> Vec<DirectCallSpecializationPlan> {
    let mut entries = direct_call_requests.iter().collect::<Vec<_>>();
    entries.sort_by_key(|request| (request.source, request.target));
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for request in entries {
        if seen.insert((request.source, request.target)) {
            plans.push(DirectCallSpecializationPlan {
                source: request.source,
                target: request.target,
                arg_plan: request.arg_plan.clone(),
                body: choose_call_body_plan_v3(&request.body),
                reason: request.reason.clone(),
            });
        }
    }
    plans
}

fn choose_call_body_plan_v3(request: &CallBodyPlanRequest) -> CallBodyPlan {
    request
        .alternatives
        .iter()
        .min_by_key(|alternative| call_body_cost_key(&alternative.cost))
        .map(|alternative| CallBodyPlan {
            kind: alternative.kind,
            cost: alternative.cost,
            reason: alternative.reason.clone(),
        })
        .unwrap_or_else(|| CallBodyPlan {
            kind: CallBodyKind::DirectCall,
            cost: direct_call_body_cost_v3(),
            reason: "guarded direct call is the default call-body alternative".to_string(),
        })
}

fn call_body_cost_key(cost: &Cost) -> (u32, u32, u32, u32) {
    (
        cost.hot_path
            .saturating_add(cost.materialization)
            .saturating_add(cost.ownership),
        cost.miss_path.saturating_add(cost.deopt),
        cost.code_size,
        cost.compile,
    )
}

fn direct_call_body_cost_v3() -> Cost {
    Cost {
        hot_path: 8,
        miss_path: 2,
        deopt: 0,
        materialization: 0,
        ownership: 1,
        code_size: 2,
        compile: 1,
    }
}

fn inline_call_body_cost_v3() -> Cost {
    Cost {
        hot_path: 2,
        miss_path: 2,
        deopt: 0,
        materialization: 0,
        ownership: 0,
        code_size: 6,
        compile: 4,
    }
}

fn plan_exact_list_item_specializations_v3(
    item_requests: &[ExactListItemPlanRequest],
) -> Vec<ExactListItemSpecializationPlan> {
    let mut entries = item_requests.iter().collect::<Vec<_>>();
    entries.sort_by_key(|request| (request.source, request.access, request.shape));
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for request in entries {
        if seen.insert((request.source, request.access, request.shape)) {
            plans.push(ExactListItemSpecializationPlan {
                source: request.source,
                access: request.access,
                shape: request.shape,
                guard: ExactListItemGuardPlan {
                    kind: ExactListItemGuardKind::ExactListExactCompactIntInBounds,
                },
                fallback: ExactListItemFallbackPlan {
                    kind: ExactListItemFallbackKind::OriginalItemAccess,
                },
                reason: request.reason.clone(),
            });
        }
    }
    plans
}

fn plan_indexed_field_specializations_v3(
    indexed_field_requests: &[IndexedFieldPlanRequest],
) -> Vec<IndexedFieldSpecializationPlan> {
    let mut entries = indexed_field_requests.iter().collect::<Vec<_>>();
    entries.sort_by(|lhs, rhs| {
        (
            lhs.source,
            lhs.access,
            lhs.owner_type.module_name.as_str(),
            lhs.owner_type.qualname.as_str(),
            lhs.attr_name.as_str(),
            lhs.expected_index,
        )
            .cmp(&(
                rhs.source,
                rhs.access,
                rhs.owner_type.module_name.as_str(),
                rhs.owner_type.qualname.as_str(),
                rhs.attr_name.as_str(),
                rhs.expected_index,
            ))
    });
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for request in entries {
        if seen.insert((
            request.source,
            request.access,
            request.owner_type.clone(),
            request.attr_name.clone(),
            request.expected_index,
        )) {
            plans.push(IndexedFieldSpecializationPlan {
                source: request.source,
                access: request.access,
                owner_type: request.owner_type.clone(),
                attr_name: request.attr_name.clone(),
                expected_index: request.expected_index,
                reason: request.reason.clone(),
            });
        }
    }
    plans
}

fn plan_indexed_global_specializations_v3(
    indexed_global_requests: &[IndexedGlobalPlanRequest],
) -> Vec<IndexedGlobalSpecializationPlan> {
    let mut entries = indexed_global_requests.iter().collect::<Vec<_>>();
    entries.sort_by(|lhs, rhs| {
        (
            lhs.source,
            lhs.access,
            lhs.module_name.as_str(),
            lhs.name.as_str(),
            lhs.expected_index,
        )
            .cmp(&(
                rhs.source,
                rhs.access,
                rhs.module_name.as_str(),
                rhs.name.as_str(),
                rhs.expected_index,
            ))
    });
    let mut seen = HashSet::new();
    let mut plans = Vec::new();
    for request in entries {
        if seen.insert((
            request.source,
            request.access,
            request.module_name.clone(),
            request.name.clone(),
            request.expected_index,
        )) {
            plans.push(IndexedGlobalSpecializationPlan {
                source: request.source,
                access: request.access,
                module_name: request.module_name.clone(),
                name: request.name.clone(),
                expected_index: request.expected_index,
                guard: IndexedGlobalGuardPlan {
                    kind: IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
                },
                fallback: IndexedGlobalFallbackPlan {
                    kind: IndexedGlobalFallbackKind::OriginalGlobalAccess,
                },
                reason: request.reason.clone(),
            });
        }
    }
    plans
}

fn plan_compact_int_branch(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Result<Option<Vec<RegionPlan>>, String> {
    if let Some(planned) = plan_compact_int_add_gt_zero_branch(catalog, region, facts)? {
        return Ok(Some(planned));
    }
    if let Some(planned) = plan_compact_int_compare_constant_branch(catalog, region, facts)? {
        return Ok(Some(planned));
    }
    if let Some(planned) = plan_compact_int_compare_branch(catalog, region, facts)? {
        return Ok(Some(planned));
    }
    if let Some(planned) = plan_compact_int_compare_return(catalog, region, facts)? {
        return Ok(Some(planned));
    }
    plan_compact_int_binary_return(catalog, region, facts)
}

fn plan_scalar_local_threads_v3(
    region_requests: &[ExtractedRegionPlanRequest],
    planned_regions: &[RegionPlan],
) -> Vec<ScalarLocalThreadPlan> {
    let mut threads = Vec::new();
    for producer_request in region_requests {
        let producer_region = &producer_request.region;
        if producer_region.block_body_len != 1 {
            continue;
        }
        let Some(store) = &producer_region.store else {
            continue;
        };
        let Some(continuation) = store.continuation else {
            continue;
        };
        let Some(local) = scalar_thread_local_from_name(&store.target) else {
            continue;
        };
        if !planned_regions
            .iter()
            .any(|region| region.id == producer_region.id)
        {
            continue;
        }
        let Some(producer_shape) =
            match_compact_int_binary_return(producer_region, &producer_request.facts)
        else {
            continue;
        };
        let Some(producer_value) = planned_i64_output(planned_regions, producer_region.id) else {
            continue;
        };
        let producer_fallback_region = RegionId(producer_region.id.0 + 1);
        if !planned_regions
            .iter()
            .any(|region| region.id == producer_fallback_region)
        {
            continue;
        }

        for consumer_request in region_requests {
            let consumer_region = &consumer_request.region;
            if consumer_region.block != continuation {
                continue;
            }
            if consumer_region.block_body_len != 0 {
                continue;
            }
            if !planned_regions
                .iter()
                .any(|region| region.id == consumer_region.id)
            {
                continue;
            }
            let Some(consumer_shape) = match_compact_int_compare_constant_local_branch(
                consumer_region,
                &consumer_request.facts,
            ) else {
                continue;
            };
            if !same_scalar_thread_local(&store.target, &consumer_shape.value_name) {
                continue;
            }
            let Some(consumer_value) = planned_unbox_output(planned_regions, consumer_region.id)
            else {
                continue;
            };
            threads.push(ScalarLocalThreadPlan {
                local: local.clone(),
                producer: RegionValueRef {
                    region: producer_region.id,
                    value: producer_value,
                },
                consumer: RegionValueRef {
                    region: consumer_region.id,
                    value: consumer_value,
                },
                fallback: ScalarThreadFallback::LocalFallbackRegion {
                    region: producer_fallback_region,
                    reason: format!(
                        "guard miss or overflow in {:?} must execute the original store before branching",
                        producer_shape.source
                    ),
                },
                local_state: ScalarThreadLocalState::ScalarOnlyHotPath {
                    cleanup: ScalarThreadLocalCleanup::NoPyObjectSlotOwnership,
                    reason: "the hot path keeps the store target as a scalar and never writes a PyObject local slot"
                        .to_string(),
                },
                materialization: ScalarThreadMaterialization::DeferredUntilPythonObjectUse {
                    reason: "the stored scalar may replace the later local unbox until Python object semantics are required"
                        .to_string(),
                },
                estimated_savings: Cost {
                    hot_path: 2,
                    materialization: 1,
                    ownership: 1,
                    ..Cost::default()
                },
                reason: format!(
                    "thread exact compact-int store into following comparison at {:?}",
                    consumer_shape.source
                ),
            });
        }
    }
    threads
}

fn planned_i64_output(planned_regions: &[RegionPlan], region_id: RegionId) -> Option<PlanValue> {
    planned_regions
        .iter()
        .find(|region| region.id == region_id)?
        .nodes
        .iter()
        .find_map(|node| match &node.kind {
            PlanNodeKind::Operation(operation) => {
                operation.output.filter(|value| value.rep == Rep::I64)
            }
            _ => None,
        })
}

fn planned_unbox_output(planned_regions: &[RegionPlan], region_id: RegionId) -> Option<PlanValue> {
    planned_regions
        .iter()
        .find(|region| region.id == region_id)?
        .nodes
        .iter()
        .find_map(|node| match &node.kind {
            PlanNodeKind::Convert(convert)
                if convert.kind == ConversionKind::FromPythonLongCompactToI64 =>
            {
                Some(convert.output)
            }
            _ => None,
        })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntCompareConstantLocalBranchShape {
    source: Option<InstrId>,
    value_name: ResolvedName,
}

fn match_compact_int_compare_constant_local_branch(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Option<CompactIntCompareConstantLocalBranchShape> {
    let ExtractedExit::Branch {
        condition, source, ..
    } = region.exit
    else {
        return None;
    };
    let truth = region.value(condition)?;
    let ExtractedValueKind::Truthiness {
        value: compare_value,
    } = truth.kind
    else {
        return None;
    };
    let compare = region.value(compare_value)?;
    let ExtractedValueKind::Binary { op, left, right } = compare.kind else {
        return None;
    };
    compare_alternative_spec(op)?;
    if facts.is_exact_compact_int(left) && facts.i64_constant(right).is_some() {
        return Some(CompactIntCompareConstantLocalBranchShape {
            source,
            value_name: region.load_name(left)?.clone(),
        });
    }
    if facts.is_exact_compact_int(right) && facts.i64_constant(left).is_some() {
        return Some(CompactIntCompareConstantLocalBranchShape {
            source,
            value_name: region.load_name(right)?.clone(),
        });
    }
    None
}

fn scalar_thread_local_from_name(name: &ResolvedName) -> Option<ScalarThreadLocal> {
    let NameLocation::Local(location) = name.location else {
        return None;
    };
    Some(ScalarThreadLocal {
        name: name.id_str().to_string(),
        location: ScalarThreadLocalLocation::Local {
            slot: location.slot(),
        },
    })
}

fn same_scalar_thread_local(left: &ResolvedName, right: &ResolvedName) -> bool {
    left.id == right.id
        && left.location == right.location
        && scalar_thread_local_from_name(left).is_some()
}

fn plan_compact_int_add_gt_zero_branch(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) = match_compact_int_add_gt_zero_branch(region, facts) else {
        return Ok(None);
    };
    let add = required_alternative(catalog, "binary.add.exact_compact_int.i64")?;
    let compare = required_alternative(catalog, "binary.gt.exact_compact_int.i32bool")?;
    let generic_add = required_alternative(catalog, "binary.add.py_generic")?;
    let generic_compare = required_alternative(catalog, "binary.gt.py_richcompare")?;
    let truthiness = required_alternative(catalog, "truthiness.pyobject")?;

    let fallback_region_id = RegionId(region.id.0 + 1);
    let failure_targets =
        FailureTargets::local_fallback(FallbackTarget::Region(fallback_region_id));

    let a_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
    let b_obj = PlanValue::new(1, Rep::PyObjectBorrowed);
    let a_i64 = PlanValue::new(2, Rep::I64);
    let b_i64 = PlanValue::new(3, Rep::I64);
    let sum_i64 = PlanValue::new(4, Rep::I64);
    let zero_i64 = PlanValue::new(5, Rep::I64);
    let condition = PlanValue::new(6, Rep::I32Bool01);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape
                .source
                .unwrap_or_else(|| InstrId::new(region.block, 0)),
        },
        inputs: vec![
            region_input(a_obj, 0, shape.left_name.clone()),
            region_input(b_obj, 1, shape.right_name.clone()),
        ],
        nodes: vec![
            guard_node(add, 0, 0, a_obj, &failure_targets),
            guard_node(add, 1, 1, b_obj, &failure_targets),
            unbox_node(2, a_obj, a_i64, 0, fallback_region_id),
            unbox_node(3, b_obj, b_i64, 1, fallback_region_id),
            operation_node(4, add, vec![a_i64, b_i64], Some(sum_i64), &failure_targets)?,
            node(
                5,
                PlanNodeKind::Constant {
                    output: zero_i64,
                    constant: PlannedConstant::I64(0),
                },
            ),
            operation_node(
                6,
                compare,
                vec![sum_i64, zero_i64],
                Some(condition),
                &failure_targets,
            )?,
        ],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Branch {
                condition,
                then_target: RegionExitTarget::OriginalCfg,
                else_target: RegionExitTarget::OriginalCfg,
            },
        }],
    };

    let fallback_sum = PlanValue::new(20, Rep::PyObjectOwned);
    let fallback_zero_i64 = PlanValue::new(21, Rep::I64);
    let fallback_zero_obj = PlanValue::new(22, Rep::PyObjectOwned);
    let fallback_compare = PlanValue::new(23, Rep::PyObjectOwned);
    let fallback_condition = PlanValue::new(24, Rep::I32Bool01);
    let fallback_region = RegionPlan {
        id: fallback_region_id,
        source: RegionSource::Synthetic {
            reason: "generic fallback for compact-int add/compare branch".to_string(),
        },
        inputs: vec![
            region_input(a_obj, 0, shape.left_name),
            region_input(b_obj, 1, shape.right_name),
        ],
        nodes: vec![
            operation_node(
                20,
                generic_add,
                vec![a_obj, b_obj],
                Some(fallback_sum),
                &failure_targets,
            )?,
            node(
                21,
                PlanNodeKind::Constant {
                    output: fallback_zero_i64,
                    constant: PlannedConstant::I64(0),
                },
            ),
            node(
                22,
                PlanNodeKind::Materialize(MaterializeNode {
                    input: fallback_zero_i64,
                    output: fallback_zero_obj,
                    kind: MaterializeKind::PythonLong,
                }),
            ),
            operation_node(
                23,
                generic_compare,
                vec![fallback_sum, fallback_zero_obj],
                Some(fallback_compare),
                &failure_targets,
            )?,
            node(
                24,
                PlanNodeKind::Convert(ConvertNode {
                    input: fallback_compare,
                    output: fallback_condition,
                    kind: ConversionKind::TruthinessToI32Bool01,
                    precondition: ConversionPrecondition::Infallible,
                    failure: truthiness
                        .instantiate_failure(&failure_targets)
                        .map_err(|err| err.0)?,
                    ownership: ConversionOwnership::ConsumeOwned,
                }),
            ),
        ],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Branch {
                condition: fallback_condition,
                then_target: RegionExitTarget::OriginalCfg,
                else_target: RegionExitTarget::OriginalCfg,
            },
        }],
    };

    Ok(Some(vec![hot_region, fallback_region]))
}

fn plan_compact_int_compare_constant_branch(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) = match_compact_int_compare_constant_branch(region, facts) else {
        return Ok(None);
    };
    let compare = required_alternative(catalog, shape.compare.exact_id)?;
    let generic_compare = required_alternative(catalog, shape.compare.generic_id)?;
    let truthiness = required_alternative(catalog, "truthiness.pyobject")?;

    let fallback_region_id = RegionId(region.id.0 + 1);
    let failure_targets =
        FailureTargets::local_fallback(FallbackTarget::Region(fallback_region_id));

    let value_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
    let value_i64 = PlanValue::new(1, Rep::I64);
    let constant_i64 = PlanValue::new(2, Rep::I64);
    let condition = PlanValue::new(3, Rep::I32Bool01);
    let guard_index = if shape.constant_on_left { 1 } else { 0 };
    let hot_operands = if shape.constant_on_left {
        vec![constant_i64, value_i64]
    } else {
        vec![value_i64, constant_i64]
    };

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape
                .source
                .unwrap_or_else(|| InstrId::new(region.block, 0)),
        },
        inputs: vec![region_input(value_obj, 0, shape.value_name.clone())],
        nodes: vec![
            guard_node(compare, guard_index, 0, value_obj, &failure_targets),
            unbox_node(1, value_obj, value_i64, 0, fallback_region_id),
            node(
                2,
                PlanNodeKind::Constant {
                    output: constant_i64,
                    constant: PlannedConstant::I64(shape.constant),
                },
            ),
            operation_node(3, compare, hot_operands, Some(condition), &failure_targets)?,
        ],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Branch {
                condition,
                then_target: RegionExitTarget::OriginalCfg,
                else_target: RegionExitTarget::OriginalCfg,
            },
        }],
    };

    let fallback_constant_i64 = PlanValue::new(20, Rep::I64);
    let fallback_constant_obj = PlanValue::new(21, Rep::PyObjectOwned);
    let fallback_compare = PlanValue::new(22, Rep::PyObjectOwned);
    let fallback_condition = PlanValue::new(23, Rep::I32Bool01);
    let fallback_operands = if shape.constant_on_left {
        vec![fallback_constant_obj, value_obj]
    } else {
        vec![value_obj, fallback_constant_obj]
    };
    let fallback_region = RegionPlan {
        id: fallback_region_id,
        source: RegionSource::Synthetic {
            reason: "generic fallback for compact-int comparison with constant branch".to_string(),
        },
        inputs: vec![region_input(value_obj, 0, shape.value_name)],
        nodes: vec![
            node(
                20,
                PlanNodeKind::Constant {
                    output: fallback_constant_i64,
                    constant: PlannedConstant::I64(shape.constant),
                },
            ),
            node(
                21,
                PlanNodeKind::Materialize(MaterializeNode {
                    input: fallback_constant_i64,
                    output: fallback_constant_obj,
                    kind: MaterializeKind::PythonLong,
                }),
            ),
            operation_node(
                22,
                generic_compare,
                fallback_operands,
                Some(fallback_compare),
                &failure_targets,
            )?,
            node(
                23,
                PlanNodeKind::Convert(ConvertNode {
                    input: fallback_compare,
                    output: fallback_condition,
                    kind: ConversionKind::TruthinessToI32Bool01,
                    precondition: ConversionPrecondition::Infallible,
                    failure: truthiness
                        .instantiate_failure(&failure_targets)
                        .map_err(|err| err.0)?,
                    ownership: ConversionOwnership::ConsumeOwned,
                }),
            ),
        ],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Branch {
                condition: fallback_condition,
                then_target: RegionExitTarget::OriginalCfg,
                else_target: RegionExitTarget::OriginalCfg,
            },
        }],
    };

    Ok(Some(vec![hot_region, fallback_region]))
}

fn plan_compact_int_binary_return(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) = match_compact_int_binary_return(region, facts) else {
        return Ok(None);
    };
    let operation = required_alternative(catalog, shape.operation.exact_id)?;
    let generic_operation = required_alternative(catalog, shape.operation.generic_id)?;

    let fallback_region_id = RegionId(region.id.0 + 1);
    let failure_targets =
        FailureTargets::local_fallback(FallbackTarget::Region(fallback_region_id));

    let a_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
    let b_obj = PlanValue::new(1, Rep::PyObjectBorrowed);
    let a_i64 = PlanValue::new(2, Rep::I64);
    let b_i64 = PlanValue::new(3, Rep::I64);
    let result_i64 = PlanValue::new(4, Rep::I64);
    let result_obj = PlanValue::new(5, Rep::PyObjectOwned);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape
                .source
                .unwrap_or_else(|| InstrId::new(region.block, 0)),
        },
        inputs: vec![
            region_input(a_obj, 0, shape.left_name.clone()),
            region_input(b_obj, 1, shape.right_name.clone()),
        ],
        nodes: vec![
            guard_node(operation, 0, 0, a_obj, &failure_targets),
            guard_node(operation, 1, 1, b_obj, &failure_targets),
            unbox_node(2, a_obj, a_i64, 0, fallback_region_id),
            unbox_node(3, b_obj, b_i64, 1, fallback_region_id),
            operation_node(
                4,
                operation,
                vec![a_i64, b_i64],
                Some(result_i64),
                &failure_targets,
            )?,
            node(
                5,
                PlanNodeKind::Materialize(MaterializeNode {
                    input: result_i64,
                    output: result_obj,
                    kind: MaterializeKind::PythonLong,
                }),
            ),
        ],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Return { value: result_obj },
        }],
    };

    let fallback_sum = PlanValue::new(20, Rep::PyObjectOwned);
    let fallback_region = RegionPlan {
        id: fallback_region_id,
        source: RegionSource::Synthetic {
            reason: "generic fallback for compact-int binary return".to_string(),
        },
        inputs: vec![
            region_input(a_obj, 0, shape.left_name),
            region_input(b_obj, 1, shape.right_name),
        ],
        nodes: vec![operation_node(
            20,
            generic_operation,
            vec![a_obj, b_obj],
            Some(fallback_sum),
            &failure_targets,
        )?],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Return {
                value: fallback_sum,
            },
        }],
    };

    Ok(Some(vec![hot_region, fallback_region]))
}

fn plan_compact_int_compare_branch(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) = match_compact_int_compare_branch(region, facts) else {
        return Ok(None);
    };
    let compare = required_alternative(catalog, shape.compare.exact_id)?;
    let generic_compare = required_alternative(catalog, shape.compare.generic_id)?;
    let truthiness = required_alternative(catalog, "truthiness.pyobject")?;

    let fallback_region_id = RegionId(region.id.0 + 1);
    let failure_targets =
        FailureTargets::local_fallback(FallbackTarget::Region(fallback_region_id));

    let a_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
    let b_obj = PlanValue::new(1, Rep::PyObjectBorrowed);
    let a_i64 = PlanValue::new(2, Rep::I64);
    let b_i64 = PlanValue::new(3, Rep::I64);
    let condition = PlanValue::new(4, Rep::I32Bool01);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape
                .source
                .unwrap_or_else(|| InstrId::new(region.block, 0)),
        },
        inputs: vec![
            region_input(a_obj, 0, shape.left_name.clone()),
            region_input(b_obj, 1, shape.right_name.clone()),
        ],
        nodes: vec![
            guard_node(compare, 0, 0, a_obj, &failure_targets),
            guard_node(compare, 1, 1, b_obj, &failure_targets),
            unbox_node(2, a_obj, a_i64, 0, fallback_region_id),
            unbox_node(3, b_obj, b_i64, 1, fallback_region_id),
            operation_node(
                4,
                compare,
                vec![a_i64, b_i64],
                Some(condition),
                &failure_targets,
            )?,
        ],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Branch {
                condition,
                then_target: RegionExitTarget::OriginalCfg,
                else_target: RegionExitTarget::OriginalCfg,
            },
        }],
    };

    let fallback_compare = PlanValue::new(20, Rep::PyObjectOwned);
    let fallback_condition = PlanValue::new(21, Rep::I32Bool01);
    let fallback_region = RegionPlan {
        id: fallback_region_id,
        source: RegionSource::Synthetic {
            reason: "generic fallback for compact-int comparison branch".to_string(),
        },
        inputs: vec![
            region_input(a_obj, 0, shape.left_name),
            region_input(b_obj, 1, shape.right_name),
        ],
        nodes: vec![
            operation_node(
                20,
                generic_compare,
                vec![a_obj, b_obj],
                Some(fallback_compare),
                &failure_targets,
            )?,
            node(
                21,
                PlanNodeKind::Convert(ConvertNode {
                    input: fallback_compare,
                    output: fallback_condition,
                    kind: ConversionKind::TruthinessToI32Bool01,
                    precondition: ConversionPrecondition::Infallible,
                    failure: truthiness
                        .instantiate_failure(&failure_targets)
                        .map_err(|err| err.0)?,
                    ownership: ConversionOwnership::ConsumeOwned,
                }),
            ),
        ],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Branch {
                condition: fallback_condition,
                then_target: RegionExitTarget::OriginalCfg,
                else_target: RegionExitTarget::OriginalCfg,
            },
        }],
    };

    Ok(Some(vec![hot_region, fallback_region]))
}

fn plan_compact_int_compare_return(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) = match_compact_int_compare_return(region, facts) else {
        return Ok(None);
    };
    let compare = required_alternative(catalog, shape.compare.exact_id)?;
    let generic_compare = required_alternative(catalog, shape.compare.generic_id)?;

    let fallback_region_id = RegionId(region.id.0 + 1);
    let failure_targets =
        FailureTargets::local_fallback(FallbackTarget::Region(fallback_region_id));

    let a_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
    let b_obj = PlanValue::new(1, Rep::PyObjectBorrowed);
    let a_i64 = PlanValue::new(2, Rep::I64);
    let b_i64 = PlanValue::new(3, Rep::I64);
    let condition = PlanValue::new(4, Rep::I32Bool01);
    let result_obj = PlanValue::new(5, Rep::PyObjectImmortal);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape
                .source
                .unwrap_or_else(|| InstrId::new(region.block, 0)),
        },
        inputs: vec![
            region_input(a_obj, 0, shape.left_name.clone()),
            region_input(b_obj, 1, shape.right_name.clone()),
        ],
        nodes: vec![
            guard_node(compare, 0, 0, a_obj, &failure_targets),
            guard_node(compare, 1, 1, b_obj, &failure_targets),
            unbox_node(2, a_obj, a_i64, 0, fallback_region_id),
            unbox_node(3, b_obj, b_i64, 1, fallback_region_id),
            operation_node(
                4,
                compare,
                vec![a_i64, b_i64],
                Some(condition),
                &failure_targets,
            )?,
            node(
                5,
                PlanNodeKind::Materialize(MaterializeNode {
                    input: condition,
                    output: result_obj,
                    kind: MaterializeKind::PythonBool,
                }),
            ),
        ],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Return { value: result_obj },
        }],
    };

    let fallback_compare = PlanValue::new(20, Rep::PyObjectOwned);
    let fallback_region = RegionPlan {
        id: fallback_region_id,
        source: RegionSource::Synthetic {
            reason: "generic fallback for compact-int comparison return".to_string(),
        },
        inputs: vec![
            region_input(a_obj, 0, shape.left_name),
            region_input(b_obj, 1, shape.right_name),
        ],
        nodes: vec![operation_node(
            20,
            generic_compare,
            vec![a_obj, b_obj],
            Some(fallback_compare),
            &failure_targets,
        )?],
        exits: vec![RegionExitPlan {
            source: shape.source,
            kind: RegionExitKind::Return {
                value: fallback_compare,
            },
        }],
    };

    Ok(Some(vec![hot_region, fallback_region]))
}

fn required_alternative<'a>(
    catalog: &'a AlternativeCatalog,
    id: &'static str,
) -> Result<&'a LoweringAlternative, String> {
    catalog
        .by_id(AlternativeId::new(id))
        .ok_or_else(|| format!("missing catalog alternative {id}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntBranchShape {
    source: Option<InstrId>,
    left_name: String,
    right_name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntCompareBranchShape {
    source: Option<InstrId>,
    left_name: String,
    right_name: String,
    compare: CompareAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntCompareConstantBranchShape {
    source: Option<InstrId>,
    value_name: String,
    constant: i64,
    constant_on_left: bool,
    compare: CompareAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntReturnShape {
    source: Option<InstrId>,
    left_name: String,
    right_name: String,
    operation: BinaryReturnAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntCompareReturnShape {
    source: Option<InstrId>,
    left_name: String,
    right_name: String,
    compare: CompareAlternativeSpec,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CompareAlternativeSpec {
    generic_id: &'static str,
    exact_id: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BinaryReturnAlternativeSpec {
    generic_id: &'static str,
    exact_id: &'static str,
}

fn match_compact_int_add_gt_zero_branch(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Option<CompactIntBranchShape> {
    let ExtractedExit::Branch {
        condition, source, ..
    } = region.exit
    else {
        return None;
    };
    let truth = region.value(condition)?;
    let ExtractedValueKind::Truthiness {
        value: compare_value,
    } = truth.kind
    else {
        return None;
    };
    let compare = region.value(compare_value)?;
    let ExtractedValueKind::Binary {
        op: BinOpKind::Gt,
        left: sum_value,
        right: zero_value,
    } = compare.kind
    else {
        return None;
    };
    if facts.i64_constant(zero_value)? != 0 {
        return None;
    }
    let sum = region.value(sum_value)?;
    let ExtractedValueKind::Binary {
        op: BinOpKind::Add,
        left,
        right,
    } = sum.kind
    else {
        return None;
    };
    if !facts.is_exact_compact_int(left) || !facts.is_exact_compact_int(right) {
        return None;
    }
    Some(CompactIntBranchShape {
        source,
        left_name: region.loadable_name(left)?,
        right_name: region.loadable_name(right)?,
    })
}

fn match_compact_int_binary_return(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Option<CompactIntReturnShape> {
    let ExtractedExit::Return { value, source } = region.exit else {
        return None;
    };
    let add = region.value(value)?;
    let ExtractedValueKind::Binary { op, left, right } = add.kind else {
        return None;
    };
    let operation = binary_return_alternative_spec(op)?;
    if !facts.is_exact_compact_int(left) || !facts.is_exact_compact_int(right) {
        return None;
    }
    Some(CompactIntReturnShape {
        source,
        left_name: region.loadable_name(left)?,
        right_name: region.loadable_name(right)?,
        operation,
    })
}

fn match_compact_int_compare_constant_branch(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Option<CompactIntCompareConstantBranchShape> {
    let ExtractedExit::Branch {
        condition, source, ..
    } = region.exit
    else {
        return None;
    };
    let truth = region.value(condition)?;
    let ExtractedValueKind::Truthiness {
        value: compare_value,
    } = truth.kind
    else {
        return None;
    };
    let compare = region.value(compare_value)?;
    let ExtractedValueKind::Binary { op, left, right } = compare.kind else {
        return None;
    };
    let compare = compare_alternative_spec(op)?;
    if facts.is_exact_compact_int(left) {
        if let Some(constant) = facts.i64_constant(right) {
            return Some(CompactIntCompareConstantBranchShape {
                source,
                value_name: region.loadable_name(left)?,
                constant,
                constant_on_left: false,
                compare,
            });
        }
    }
    if facts.is_exact_compact_int(right) {
        if let Some(constant) = facts.i64_constant(left) {
            return Some(CompactIntCompareConstantBranchShape {
                source,
                value_name: region.loadable_name(right)?,
                constant,
                constant_on_left: true,
                compare,
            });
        }
    }
    None
}

fn match_compact_int_compare_branch(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Option<CompactIntCompareBranchShape> {
    let ExtractedExit::Branch {
        condition, source, ..
    } = region.exit
    else {
        return None;
    };
    let truth = region.value(condition)?;
    let ExtractedValueKind::Truthiness {
        value: compare_value,
    } = truth.kind
    else {
        return None;
    };
    let compare = region.value(compare_value)?;
    let ExtractedValueKind::Binary { op, left, right } = compare.kind else {
        return None;
    };
    let compare = compare_alternative_spec(op)?;
    if !facts.is_exact_compact_int(left) || !facts.is_exact_compact_int(right) {
        return None;
    }
    Some(CompactIntCompareBranchShape {
        source,
        left_name: region.loadable_name(left)?,
        right_name: region.loadable_name(right)?,
        compare,
    })
}

fn match_compact_int_compare_return(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
) -> Option<CompactIntCompareReturnShape> {
    let ExtractedExit::Return { value, source } = region.exit else {
        return None;
    };
    let compare = region.value(value)?;
    let ExtractedValueKind::Binary { op, left, right } = compare.kind else {
        return None;
    };
    let compare = compare_alternative_spec(op)?;
    if !facts.is_exact_compact_int(left) || !facts.is_exact_compact_int(right) {
        return None;
    }
    Some(CompactIntCompareReturnShape {
        source,
        left_name: region.loadable_name(left)?,
        right_name: region.loadable_name(right)?,
        compare,
    })
}

fn compare_alternative_spec(op: BinOpKind) -> Option<CompareAlternativeSpec> {
    Some(match op {
        BinOpKind::Eq => CompareAlternativeSpec {
            generic_id: "binary.eq.py_richcompare",
            exact_id: "binary.eq.exact_compact_int.i32bool",
        },
        BinOpKind::Ne => CompareAlternativeSpec {
            generic_id: "binary.ne.py_richcompare",
            exact_id: "binary.ne.exact_compact_int.i32bool",
        },
        BinOpKind::Lt => CompareAlternativeSpec {
            generic_id: "binary.lt.py_richcompare",
            exact_id: "binary.lt.exact_compact_int.i32bool",
        },
        BinOpKind::Le => CompareAlternativeSpec {
            generic_id: "binary.le.py_richcompare",
            exact_id: "binary.le.exact_compact_int.i32bool",
        },
        BinOpKind::Gt => CompareAlternativeSpec {
            generic_id: "binary.gt.py_richcompare",
            exact_id: "binary.gt.exact_compact_int.i32bool",
        },
        BinOpKind::Ge => CompareAlternativeSpec {
            generic_id: "binary.ge.py_richcompare",
            exact_id: "binary.ge.exact_compact_int.i32bool",
        },
        _ => return None,
    })
}

fn binary_return_alternative_spec(op: BinOpKind) -> Option<BinaryReturnAlternativeSpec> {
    Some(match op {
        BinOpKind::Add => BinaryReturnAlternativeSpec {
            generic_id: "binary.add.py_generic",
            exact_id: "binary.add.exact_compact_int.i64",
        },
        BinOpKind::Sub => BinaryReturnAlternativeSpec {
            generic_id: "binary.sub.py_generic",
            exact_id: "binary.sub.exact_compact_int.i64",
        },
        BinOpKind::Mul => BinaryReturnAlternativeSpec {
            generic_id: "binary.mul.py_generic",
            exact_id: "binary.mul.exact_compact_int.i64",
        },
        BinOpKind::And => BinaryReturnAlternativeSpec {
            generic_id: "binary.and.py_generic",
            exact_id: "binary.and.exact_compact_int.i64",
        },
        BinOpKind::Or => BinaryReturnAlternativeSpec {
            generic_id: "binary.or.py_generic",
            exact_id: "binary.or.exact_compact_int.i64",
        },
        BinOpKind::Xor => BinaryReturnAlternativeSpec {
            generic_id: "binary.xor.py_generic",
            exact_id: "binary.xor.exact_compact_int.i64",
        },
        _ => return None,
    })
}

trait ExtractedRegionExt {
    fn value(&self, value: ExtractedValueId) -> Option<&ExtractedValue>;
    fn load_name(&self, value: ExtractedValueId) -> Option<&ResolvedName>;
    fn loadable_name(&self, value: ExtractedValueId) -> Option<String>;
    fn exit_source(&self) -> Option<InstrId>;
}

impl ExtractedRegionExt for ExtractedRegion {
    fn value(&self, value: ExtractedValueId) -> Option<&ExtractedValue> {
        self.values.iter().find(|entry| entry.id == value)
    }

    fn load_name(&self, value: ExtractedValueId) -> Option<&ResolvedName> {
        match &self.value(value)?.kind {
            ExtractedValueKind::LoadName { name } => Some(name),
            ExtractedValueKind::Binary { .. } | ExtractedValueKind::Truthiness { .. } => None,
        }
    }

    fn loadable_name(&self, value: ExtractedValueId) -> Option<String> {
        let name = self.load_name(value)?;
        match name.location {
            NameLocation::Local(_) | NameLocation::Cell(_) => Some(name.id_str().to_string()),
            NameLocation::GlobalName
            | NameLocation::Global(_)
            | NameLocation::RuntimeName(_)
            | NameLocation::Constant(_) => None,
        }
    }

    fn exit_source(&self) -> Option<InstrId> {
        match &self.exit {
            ExtractedExit::Branch { source, .. } | ExtractedExit::Return { source, .. } => *source,
        }
    }
}

fn region_input(value: PlanValue, index: u32, name: String) -> RegionInput {
    RegionInput {
        value,
        source: RegionInputSource::FunctionParam {
            index,
            name: Some(name),
        },
    }
}

fn node(id: u32, kind: PlanNodeKind) -> PlanNode {
    PlanNode {
        id: PlanNodeId(id),
        source: None,
        kind,
    }
}

fn guard_node(
    alternative: &LoweringAlternative,
    guard_index: usize,
    id: u32,
    input: PlanValue,
    targets: &FailureTargets,
) -> PlanNode {
    let mut guard = alternative.guards[guard_index]
        .instantiate(targets)
        .expect("compact-int alternative should instantiate guard");
    guard.inputs = vec![input];
    node(id, PlanNodeKind::Guard(guard))
}

fn unbox_node(
    id: u32,
    input: PlanValue,
    output: PlanValue,
    guard: u32,
    fallback_region: RegionId,
) -> PlanNode {
    node(
        id,
        PlanNodeKind::Convert(ConvertNode {
            input,
            output,
            kind: ConversionKind::FromPythonLongCompactToI64,
            precondition: ConversionPrecondition::SpecializationGuard {
                guard: PlanNodeId(guard),
                reason: "exact compact PyLong guard dominates conversion".to_string(),
            },
            failure: FailureMode::FallbackToPlan {
                target: FallbackTarget::Region(fallback_region),
                reason: FallbackReason("conversion miss uses generic fallback".to_string()),
            },
            ownership: ConversionOwnership::BorrowInput,
        }),
    )
}

fn operation_node(
    id: u32,
    alternative: &LoweringAlternative,
    inputs: Vec<PlanValue>,
    output: Option<PlanValue>,
    targets: &FailureTargets,
) -> Result<PlanNode, String> {
    Ok(node(
        id,
        PlanNodeKind::Operation(OperationNode {
            op: alternative
                .planned_operation()
                .ok_or_else(|| format!("alternative {} is not an operation", alternative.id.0))?,
            inputs,
            output,
            failure_replay: alternative.failure_replay.clone(),
            failure: alternative
                .instantiate_failure(targets)
                .map_err(|err| err.0)?,
            cost: alternative.cost,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alternatives_v3::ALTERNATIVE_CATALOG_V3_VERSION;
    use crate::plan_v3::RichCompareOp;
    use crate::plan_v3::validate_module_plan_v3;
    use crate::region_v3::{extract_block_region_v3, extract_function_regions_v3};
    use soac_core::block_py::{
        BinOp, Block, BlockEdge, BlockLabel, BlockParam, BlockPyFunction, BlockPyName, BlockTerm,
        FunctionName, Load, LocalFunctionId, LocalLocation, Meta, ModuleNameGen, NameLocation,
        ParamSpec, SerializedFunctionId, SerializedIdentityTables, SerializedModuleId,
        SerializedModuleIdentity, Store, TermIf, WithMeta,
    };
    use soac_lowering::passes::{CodegenModuleShape, InstrCodegen};

    fn label(index: usize) -> BlockLabel {
        BlockLabel::from_index(index)
    }

    fn instr_id_in_label(block: BlockLabel, index: u32) -> InstrId {
        InstrId::new(block, index)
    }

    fn with_instr_id(instr: InstrCodegen, index: u32) -> InstrCodegen {
        with_instr_id_in_label(instr, label(0), index)
    }

    fn with_instr_id_in_label(instr: InstrCodegen, block: BlockLabel, index: u32) -> InstrCodegen {
        instr.with_meta(Meta {
            instr_id: Some(instr_id_in_label(block, index)),
            ..Meta::synthetic()
        })
    }

    fn local(name: &str, slot: u32) -> InstrCodegen {
        InstrCodegen::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Local(LocalLocation(slot)),
        }))
    }

    fn constant(slot: u32) -> InstrCodegen {
        InstrCodegen::Load(Load::new(ResolvedName {
            id: BlockPyName::new("__dp_constant"),
            location: NameLocation::Constant(slot),
        }))
    }

    fn binary(op: BinOpKind, left: InstrCodegen, right: InstrCodegen, id: u32) -> InstrCodegen {
        with_instr_id(InstrCodegen::BinOp(BinOp::new(op, left, right)), id)
    }

    fn binary_in_label(
        op: BinOpKind,
        left: InstrCodegen,
        right: InstrCodegen,
        block: BlockLabel,
        id: u32,
    ) -> InstrCodegen {
        with_instr_id_in_label(InstrCodegen::BinOp(BinOp::new(op, left, right)), block, id)
    }

    fn test_function(blocks: Vec<Block<InstrCodegen>>) -> BlockPyFunction<CodegenModuleShape> {
        let name_gen = ModuleNameGen::new(0).next_function_name_gen();
        BlockPyFunction {
            function_id: name_gen.function_id(),
            name_gen,
            names: FunctionName::new("f", "f", "f", "f"),
            kind: soac_core::block_py::FunctionKind::Function,
            execution_mode: Default::default(),
            params: ParamSpec::default(),
            blocks,
            doc: None,
            storage_layout: None,
            scope: Default::default(),
        }
    }

    fn compact_int_branch_region() -> ExtractedRegion {
        let add = binary(
            BinOpKind::Add,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let test = binary(BinOpKind::Gt, add, with_instr_id(local("zero", 2), 3), 4);
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label: label(1),
                else_label: label(2),
            }),
            Vec::<BlockParam>::new(),
            None,
        );
        extract_block_region_v3(&block, RegionId(0)).unwrap()
    }

    fn compact_int_compare_branch_region(kind: BinOpKind) -> ExtractedRegion {
        let test = binary(
            kind,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label: label(1),
                else_label: label(2),
            }),
            Vec::<BlockParam>::new(),
            None,
        );
        extract_block_region_v3(&block, RegionId(0)).unwrap()
    }

    fn compact_int_compare_branch_with_constant_region(kind: BinOpKind) -> ExtractedRegion {
        let test = binary(
            kind,
            with_instr_id(local("c", 0), 0),
            with_instr_id(constant(0), 1),
            2,
        );
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test,
                then_label: label(1),
                else_label: label(2),
            }),
            Vec::<BlockParam>::new(),
            None,
        );
        extract_block_region_v3(&block, RegionId(0)).unwrap()
    }

    fn compact_int_compare_return_region(kind: BinOpKind) -> ExtractedRegion {
        let value = binary(
            kind,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::Return(value),
            Vec::<BlockParam>::new(),
            None,
        );
        extract_block_region_v3(&block, RegionId(0)).unwrap()
    }

    fn compact_int_binary_return_region(kind: BinOpKind) -> ExtractedRegion {
        let value = binary(
            kind,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let block = Block::new(
            label(0),
            Vec::new(),
            BlockTerm::Return(value),
            Vec::<BlockParam>::new(),
            None,
        );
        extract_block_region_v3(&block, RegionId(0)).unwrap()
    }

    fn facts_for_compact_region() -> PlannerFacts {
        let mut facts = PlannerFacts::default();
        facts.mark_exact_compact_int(ExtractedValueId(0));
        facts.mark_exact_compact_int(ExtractedValueId(1));
        facts.set_i64_constant(ExtractedValueId(3), 0);
        facts
    }

    fn facts_for_compare_constant_region() -> PlannerFacts {
        let mut facts = PlannerFacts::default();
        facts.mark_exact_compact_int(ExtractedValueId(0));
        facts.mark_exact_compact_int(ExtractedValueId(1));
        facts.set_i64_constant(ExtractedValueId(1), 0);
        facts
    }

    fn module_request(region: ExtractedRegion, facts: PlannerFacts) -> ModulePlanRequest {
        module_request_regions(vec![ExtractedRegionPlanRequest { region, facts }])
    }

    fn module_request_regions(regions: Vec<ExtractedRegionPlanRequest>) -> ModulePlanRequest {
        ModulePlanRequest {
            module: ModulePlanIdentity {
                module_name: "pkg.mod".to_string(),
                source_hash: 0x55,
                cache_identity: "test-cache".to_string(),
            },
            identity_tables: SerializedIdentityTables {
                modules: vec![SerializedModuleIdentity {
                    module_name: "pkg.mod".to_string(),
                    source_hash: 0x55,
                    cache_identity: Some("test-cache".to_string()),
                }],
                debug_names: Vec::new(),
            },
            functions: vec![FunctionPlanRequest {
                function: FunctionPlanIdentity {
                    function: SerializedFunctionId::new(
                        SerializedModuleId::new(0),
                        LocalFunctionId::new(1),
                    ),
                    debug_name: Some("f".to_string()),
                },
                regions,
                direct_calls: Vec::new(),
                constructor_calls: Vec::new(),
                method_calls: Vec::new(),
                exact_list_items: Vec::new(),
                indexed_fields: Vec::new(),
                indexed_globals: Vec::new(),
            }],
        }
    }

    #[test]
    fn plans_same_module_direct_call_selection_from_profiled_targets() {
        let mut request = module_request_regions(Vec::new());
        let source = InstrId::new(label(0), 9);
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        request.functions[0].direct_calls = vec![
            DirectCallPlanRequest {
                source,
                target,
                arg_plan: DirectCallArgPlan {
                    sources: vec![crate::plan_v3::DirectCallArgSource::Provided(0)],
                },
                body: CallBodyPlanRequest::with_inline_candidate(true),
                reason: "profiled call_hot_targets selected this same-module function".to_string(),
            },
            DirectCallPlanRequest {
                source,
                target,
                arg_plan: DirectCallArgPlan {
                    sources: vec![crate::plan_v3::DirectCallArgSource::Provided(0)],
                },
                body: CallBodyPlanRequest::with_inline_candidate(true),
                reason: "profiled call_hot_targets selected this same-module function".to_string(),
            },
        ];

        let plan = plan_module_optimization_v3(&AlternativeCatalog::default_v3(), request);

        validate_module_plan_v3(&plan).unwrap();
        let direct_calls = &plan.functions[0].direct_calls;
        assert_eq!(direct_calls.len(), 1);
        assert_eq!(direct_calls[0].source, source);
        assert_eq!(direct_calls[0].target, target);
        assert_eq!(
            direct_calls[0].arg_plan,
            DirectCallArgPlan {
                sources: vec![crate::plan_v3::DirectCallArgSource::Provided(0)]
            }
        );
        assert_eq!(direct_calls[0].body.kind, CallBodyKind::Inline);
        assert!(direct_calls[0].reason.contains("call_hot_targets"));
    }

    #[test]
    fn direct_call_body_cost_model_declines_inline_without_candidate() {
        let mut request = module_request_regions(Vec::new());
        let source = InstrId::new(label(0), 9);
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        request.functions[0].direct_calls = vec![DirectCallPlanRequest {
            source,
            target,
            arg_plan: DirectCallArgPlan {
                sources: vec![crate::plan_v3::DirectCallArgSource::Provided(0)],
            },
            body: CallBodyPlanRequest::with_inline_candidate(false),
            reason: "profiled call_hot_targets selected this same-module function".to_string(),
        }];

        let plan = plan_module_optimization_v3(&AlternativeCatalog::default_v3(), request);

        validate_module_plan_v3(&plan).unwrap();
        assert_eq!(
            plan.functions[0].direct_calls[0].body.kind,
            CallBodyKind::DirectCall
        );
    }

    #[test]
    fn plans_exact_list_item_selection_from_profiled_shapes() {
        let mut request = module_request_regions(Vec::new());
        let get_source = InstrId::new(label(0), 9);
        let set_source = InstrId::new(label(0), 11);
        request.functions[0].exact_list_items = vec![
            ExactListItemPlanRequest {
                source: get_source,
                access: ExactListItemAccessKind::Get,
                shape: ExactListItemShape::ExactListExactInt,
                reason: "profiled getitem".to_string(),
            },
            ExactListItemPlanRequest {
                source: get_source,
                access: ExactListItemAccessKind::Get,
                shape: ExactListItemShape::ExactListExactInt,
                reason: "profiled getitem".to_string(),
            },
            ExactListItemPlanRequest {
                source: set_source,
                access: ExactListItemAccessKind::Set,
                shape: ExactListItemShape::ExactListExactInt,
                reason: "profiled setitem".to_string(),
            },
        ];

        let plan = plan_module_optimization_v3(&AlternativeCatalog::default_v3(), request);

        validate_module_plan_v3(&plan).unwrap();
        let items = &plan.functions[0].exact_list_items;
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].access, ExactListItemAccessKind::Get);
        assert_eq!(
            items[0].guard.kind,
            ExactListItemGuardKind::ExactListExactCompactIntInBounds
        );
        assert_eq!(
            items[0].fallback.kind,
            ExactListItemFallbackKind::OriginalItemAccess
        );
        assert_eq!(items[1].access, ExactListItemAccessKind::Set);
    }

    #[test]
    fn plans_indexed_field_selections_from_profiled_type_keys() {
        let mut request = module_request_regions(Vec::new());
        let load_source = InstrId::new(label(0), 9);
        let store_source = InstrId::new(label(0), 3);
        let owner_type = IndexedFieldOwnerType {
            module_name: "pkg.model".to_string(),
            qualname: "Record".to_string(),
        };
        request.functions[0].indexed_fields = vec![
            IndexedFieldPlanRequest {
                source: load_source,
                access: IndexedFieldAccessKind::Load,
                owner_type: owner_type.clone(),
                attr_name: "value".to_string(),
                expected_index: 2,
                reason: "profiled type_keys selected this indexed-field layout".to_string(),
            },
            IndexedFieldPlanRequest {
                source: load_source,
                access: IndexedFieldAccessKind::Load,
                owner_type: owner_type.clone(),
                attr_name: "value".to_string(),
                expected_index: 2,
                reason: "profiled type_keys selected this indexed-field layout".to_string(),
            },
            IndexedFieldPlanRequest {
                source: store_source,
                access: IndexedFieldAccessKind::Store,
                owner_type,
                attr_name: "value".to_string(),
                expected_index: 2,
                reason: "profiled type_keys selected this indexed-field layout".to_string(),
            },
        ];

        let plan = plan_module_optimization_v3(&AlternativeCatalog::default_v3(), request);

        validate_module_plan_v3(&plan).unwrap();
        let indexed_fields = &plan.functions[0].indexed_fields;
        assert_eq!(indexed_fields.len(), 2);
        assert_eq!(indexed_fields[0].source, store_source);
        assert_eq!(indexed_fields[0].access, IndexedFieldAccessKind::Store);
        assert_eq!(indexed_fields[1].source, load_source);
        assert_eq!(indexed_fields[1].access, IndexedFieldAccessKind::Load);
    }

    #[test]
    fn plans_compact_int_add_gt_zero_branch() {
        let catalog = AlternativeCatalog::default_v3();
        let request = module_request(compact_int_branch_region(), facts_for_compact_region());
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        assert_eq!(plan.helper_catalog_version, ALTERNATIVE_CATALOG_V3_VERSION);
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(function.regions[0].nodes.len(), 7);
        assert!(matches!(
            function.regions[0].nodes[4].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::CheckedI64Add,
                ..
            })
        ));
        assert!(matches!(
            function.regions[0].nodes[6].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::I64CompareToBool01 {
                    op: RichCompareOp::Gt
                },
                ..
            })
        ));
    }

    #[test]
    fn plans_direct_compact_int_compare_branch() {
        let catalog = AlternativeCatalog::default_v3();
        let request = module_request(
            compact_int_compare_branch_region(BinOpKind::Lt),
            facts_for_compact_region(),
        );
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(function.regions[0].nodes.len(), 5);
        assert!(matches!(
            function.regions[0].nodes[4].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::I64CompareToBool01 {
                    op: RichCompareOp::Lt
                },
                ..
            })
        ));
        assert_eq!(function.regions[1].nodes.len(), 2);
        assert!(matches!(
            function.regions[1].nodes[0].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::PyObjectRichCompare {
                    op: RichCompareOp::Lt
                },
                ..
            })
        ));
    }

    #[test]
    fn plans_compact_int_compare_return() {
        let catalog = AlternativeCatalog::default_v3();
        let request = module_request(
            compact_int_compare_return_region(BinOpKind::Ge),
            facts_for_compact_region(),
        );
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(function.regions[0].nodes.len(), 6);
        assert!(matches!(
            function.regions[0].nodes[4].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::I64CompareToBool01 {
                    op: RichCompareOp::Ge
                },
                ..
            })
        ));
        assert!(matches!(
            function.regions[0].nodes[5].kind,
            PlanNodeKind::Materialize(MaterializeNode {
                kind: MaterializeKind::PythonBool,
                ..
            })
        ));
        assert!(matches!(
            function.regions[0].exits[0].kind,
            RegionExitKind::Return { .. }
        ));
        assert!(matches!(
            function.regions[1].nodes[0].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::PyObjectRichCompare {
                    op: RichCompareOp::Ge
                },
                ..
            })
        ));
    }

    #[test]
    fn plans_compact_int_add_return() {
        let catalog = AlternativeCatalog::default_v3();
        let request = module_request(
            compact_int_binary_return_region(BinOpKind::Add),
            facts_for_compact_region(),
        );
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(function.regions[0].nodes.len(), 6);
        assert!(matches!(
            function.regions[0].nodes[4].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::CheckedI64Add,
                ..
            })
        ));
        assert!(matches!(
            function.regions[0].nodes[5].kind,
            PlanNodeKind::Materialize(MaterializeNode {
                kind: MaterializeKind::PythonLong,
                ..
            })
        ));
        assert!(matches!(
            function.regions[0].exits[0].kind,
            RegionExitKind::Return { .. }
        ));
        assert!(matches!(
            function.regions[1].nodes[0].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::PyNumberAdd,
                ..
            })
        ));
    }

    #[test]
    fn plans_compact_int_sub_and_mul_return() {
        let catalog = AlternativeCatalog::default_v3();
        for (kind, exact_op, generic_op) in [
            (
                BinOpKind::Sub,
                crate::plan_v3::PlannedOp::CheckedI64Sub,
                crate::plan_v3::PlannedOp::PyNumberSubtract,
            ),
            (
                BinOpKind::Mul,
                crate::plan_v3::PlannedOp::CheckedI64Mul,
                crate::plan_v3::PlannedOp::PyNumberMultiply,
            ),
        ] {
            let request = module_request(
                compact_int_binary_return_region(kind),
                facts_for_compact_region(),
            );
            let plan = plan_module_optimization_v3(&catalog, request);

            validate_module_plan_v3(&plan).unwrap();
            let function = &plan.functions[0];
            assert!(function.diagnostics.is_empty(), "{kind:?}");
            assert_eq!(function.regions.len(), 2, "{kind:?}");
            assert_eq!(function.regions[0].nodes.len(), 6, "{kind:?}");
            assert!(
                matches!(
                    &function.regions[0].nodes[4].kind,
                    PlanNodeKind::Operation(OperationNode { op, .. }) if op == &exact_op
                ),
                "{kind:?}"
            );
            assert!(
                matches!(
                    &function.regions[1].nodes[0].kind,
                    PlanNodeKind::Operation(OperationNode { op, .. }) if op == &generic_op
                ),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn plans_compact_int_bitwise_returns() {
        let catalog = AlternativeCatalog::default_v3();
        for (kind, exact_op, generic_op) in [
            (
                BinOpKind::And,
                crate::plan_v3::PlannedOp::I64BitAnd,
                crate::plan_v3::PlannedOp::PyNumberBitAnd,
            ),
            (
                BinOpKind::Or,
                crate::plan_v3::PlannedOp::I64BitOr,
                crate::plan_v3::PlannedOp::PyNumberBitOr,
            ),
            (
                BinOpKind::Xor,
                crate::plan_v3::PlannedOp::I64BitXor,
                crate::plan_v3::PlannedOp::PyNumberBitXor,
            ),
        ] {
            let request = module_request(
                compact_int_binary_return_region(kind),
                facts_for_compact_region(),
            );
            let plan = plan_module_optimization_v3(&catalog, request);

            validate_module_plan_v3(&plan).unwrap();
            let function = &plan.functions[0];
            assert!(function.diagnostics.is_empty(), "{kind:?}");
            assert_eq!(function.regions.len(), 2, "{kind:?}");
            assert_eq!(function.regions[0].nodes.len(), 6, "{kind:?}");
            assert!(
                matches!(
                    &function.regions[0].nodes[4].kind,
                    PlanNodeKind::Operation(OperationNode { op, .. }) if op == &exact_op
                ),
                "{kind:?}"
            );
            assert!(
                matches!(
                    &function.regions[1].nodes[0].kind,
                    PlanNodeKind::Operation(OperationNode { op, .. }) if op == &generic_op
                ),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn missing_exact_int_facts_declines_region() {
        let catalog = AlternativeCatalog::default_v3();
        let request = module_request(compact_int_branch_region(), PlannerFacts::default());
        let plan = plan_module_optimization_v3(&catalog, request);

        let function = &plan.functions[0];
        assert!(function.regions.is_empty());
        assert_eq!(function.diagnostics.len(), 1);
        assert!(
            function.diagnostics[0]
                .message
                .contains("unsupported shape"),
            "{:?}",
            function.diagnostics[0]
        );
    }

    #[test]
    fn plans_compact_int_compare_branch_with_constant_operand() {
        let catalog = AlternativeCatalog::default_v3();
        let request = module_request(
            compact_int_compare_branch_with_constant_region(BinOpKind::Gt),
            facts_for_compare_constant_region(),
        );
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(function.regions[0].inputs.len(), 1);
        assert_eq!(
            function.regions[0].inputs[0].source,
            RegionInputSource::FunctionParam {
                index: 0,
                name: Some("c".to_string())
            }
        );
        assert!(matches!(
            function.regions[0].nodes[2].kind,
            PlanNodeKind::Constant {
                constant: PlannedConstant::I64(0),
                ..
            }
        ));
        assert!(matches!(
            function.regions[0].nodes[3].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::plan_v3::PlannedOp::I64CompareToBool01 {
                    op: RichCompareOp::Gt
                },
                ..
            })
        ));
    }

    #[test]
    fn plans_scalar_thread_for_store_rhs_followed_by_compare() {
        let catalog = AlternativeCatalog::default_v3();
        let c = ResolvedName {
            id: BlockPyName::new("c"),
            location: NameLocation::Local(LocalLocation(2)),
        };
        let add = binary(
            BinOpKind::Add,
            with_instr_id(local("a", 0), 0),
            with_instr_id(local("b", 1), 1),
            2,
        );
        let entry = Block::new(
            label(0),
            vec![with_instr_id(
                InstrCodegen::Store(Store::new(c.clone(), add)),
                3,
            )],
            BlockTerm::Jump(BlockEdge::new(label(1))),
            Vec::<BlockParam>::new(),
            None,
        );
        let test_label = label(1);
        let compare = binary_in_label(
            BinOpKind::Gt,
            with_instr_id_in_label(InstrCodegen::Load(Load::new(c)), test_label, 4),
            with_instr_id_in_label(constant(0), test_label, 5),
            test_label,
            6,
        );
        let test = Block::new(
            test_label,
            Vec::new(),
            BlockTerm::IfTerm(TermIf {
                test: compare,
                then_label: label(2),
                else_label: label(3),
            }),
            Vec::<BlockParam>::new(),
            None,
        );
        let region_requests = extract_function_regions_v3(&test_function(vec![entry, test]))
            .into_iter()
            .filter_map(|attempt| {
                let region = attempt.result.ok()?;
                let mut facts = PlannerFacts::default();
                match region.id {
                    RegionId(0) => {
                        facts.mark_exact_compact_int(ExtractedValueId(0));
                        facts.mark_exact_compact_int(ExtractedValueId(1));
                    }
                    RegionId(4) => {
                        facts.mark_exact_compact_int(ExtractedValueId(0));
                        facts.set_i64_constant(ExtractedValueId(1), 0);
                    }
                    _ => {}
                }
                Some(ExtractedRegionPlanRequest { region, facts })
            })
            .collect::<Vec<_>>();

        let plan = plan_module_optimization_v3(&catalog, module_request_regions(region_requests));

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 4);
        assert_eq!(function.scalar_threads.len(), 1);
        let thread = &function.scalar_threads[0];
        assert_eq!(thread.local.name, "c");
        assert_eq!(
            thread.local.location,
            ScalarThreadLocalLocation::Local { slot: 2 }
        );
        assert_eq!(thread.producer.region, RegionId(0));
        assert_eq!(thread.producer.value.rep, Rep::I64);
        assert_eq!(thread.consumer.region, RegionId(4));
        assert_eq!(thread.consumer.value.rep, Rep::I64);
        assert!(matches!(
            thread.fallback,
            ScalarThreadFallback::LocalFallbackRegion {
                region: RegionId(1),
                ..
            }
        ));
        assert!(matches!(
            thread.local_state,
            ScalarThreadLocalState::ScalarOnlyHotPath {
                cleanup: ScalarThreadLocalCleanup::NoPyObjectSlotOwnership,
                ..
            }
        ));
    }

    #[test]
    fn scalar_thread_declines_intervening_consumer_body() {
        let catalog = AlternativeCatalog::default_v3();
        let c = ResolvedName {
            id: BlockPyName::new("c"),
            location: NameLocation::Local(LocalLocation(2)),
        };
        let producer = ExtractedRegion {
            id: RegionId(0),
            block: label(0),
            block_body_len: 1,
            store: Some(crate::region_v3::ExtractedStoreContext {
                target: c.clone(),
                continuation: Some(label(1)),
            }),
            values: vec![
                ExtractedValue {
                    id: ExtractedValueId(0),
                    source: None,
                    kind: ExtractedValueKind::LoadName {
                        name: ResolvedName {
                            id: BlockPyName::new("a"),
                            location: NameLocation::Local(LocalLocation(0)),
                        },
                    },
                },
                ExtractedValue {
                    id: ExtractedValueId(1),
                    source: None,
                    kind: ExtractedValueKind::LoadName {
                        name: ResolvedName {
                            id: BlockPyName::new("b"),
                            location: NameLocation::Local(LocalLocation(1)),
                        },
                    },
                },
                ExtractedValue {
                    id: ExtractedValueId(2),
                    source: None,
                    kind: ExtractedValueKind::Binary {
                        op: BinOpKind::Add,
                        left: ExtractedValueId(0),
                        right: ExtractedValueId(1),
                    },
                },
            ],
            exit: ExtractedExit::Return {
                source: None,
                value: ExtractedValueId(2),
            },
        };
        let consumer = ExtractedRegion {
            id: RegionId(4),
            block: label(1),
            block_body_len: 1,
            store: None,
            values: vec![
                ExtractedValue {
                    id: ExtractedValueId(0),
                    source: None,
                    kind: ExtractedValueKind::LoadName { name: c },
                },
                ExtractedValue {
                    id: ExtractedValueId(1),
                    source: None,
                    kind: ExtractedValueKind::LoadName {
                        name: ResolvedName {
                            id: BlockPyName::new("__dp_constant"),
                            location: NameLocation::Constant(0),
                        },
                    },
                },
                ExtractedValue {
                    id: ExtractedValueId(2),
                    source: None,
                    kind: ExtractedValueKind::Binary {
                        op: BinOpKind::Gt,
                        left: ExtractedValueId(0),
                        right: ExtractedValueId(1),
                    },
                },
                ExtractedValue {
                    id: ExtractedValueId(3),
                    source: None,
                    kind: ExtractedValueKind::Truthiness {
                        value: ExtractedValueId(2),
                    },
                },
            ],
            exit: ExtractedExit::Branch {
                source: None,
                condition: ExtractedValueId(3),
                then_label: label(2),
                else_label: label(3),
            },
        };
        let mut producer_facts = PlannerFacts::default();
        producer_facts.mark_exact_compact_int(ExtractedValueId(0));
        producer_facts.mark_exact_compact_int(ExtractedValueId(1));
        let mut consumer_facts = PlannerFacts::default();
        consumer_facts.mark_exact_compact_int(ExtractedValueId(0));
        consumer_facts.set_i64_constant(ExtractedValueId(1), 0);

        let plan = plan_module_optimization_v3(
            &catalog,
            module_request_regions(vec![
                ExtractedRegionPlanRequest {
                    region: producer,
                    facts: producer_facts,
                },
                ExtractedRegionPlanRequest {
                    region: consumer,
                    facts: consumer_facts,
                },
            ]),
        );

        validate_module_plan_v3(&plan).unwrap();
        assert_eq!(plan.functions[0].regions.len(), 4);
        assert!(plan.functions[0].scalar_threads.is_empty());
    }

    #[test]
    fn nonzero_constant_declines_region() {
        let catalog = AlternativeCatalog::default_v3();
        let mut facts = facts_for_compact_region();
        facts.set_i64_constant(ExtractedValueId(3), 1);
        let request = module_request(compact_int_branch_region(), facts);
        let plan = plan_module_optimization_v3(&catalog, request);

        assert!(plan.functions[0].regions.is_empty());
        assert_eq!(plan.functions[0].diagnostics.len(), 1);
    }

    #[test]
    fn missing_catalog_entry_reports_decline() {
        let mut catalog = AlternativeCatalog::default_v3();
        catalog
            .alternatives
            .retain(|alternative| alternative.id != AlternativeId::new("binary.add.py_generic"));
        let request = module_request(compact_int_branch_region(), facts_for_compact_region());
        let plan = plan_module_optimization_v3(&catalog, request);

        assert!(plan.functions[0].regions.is_empty());
        assert!(
            plan.functions[0].diagnostics[0]
                .message
                .contains("missing catalog alternative binary.add.py_generic")
        );
    }
}
