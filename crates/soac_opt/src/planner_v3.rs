use crate::alternatives_v3::{
    AlternativeCatalog, AlternativeId, FailureTargets, LoweringAlternative,
};
use crate::region_v3::{
    ExtractedExit, ExtractedRegion, ExtractedValue, ExtractedValueId, ExtractedValueKind,
};
use soac_core::block_py::{
    BinOpKind, InstrId, NameLike, NameLocation, SerializedFunctionId, SerializedIdentityTables,
};
use soac_ir_typed::plan_v3::{
    CallBodyKind, CallBodyPlan, ConversionKind, ConversionOwnership, ConversionPrecondition,
    ConvertNode, Cost, DirectCallArgPlan, DirectCallCallee, DirectCallSpecializationPlan,
    ExactListItemAccessKind, ExactListItemFallbackKind, ExactListItemFallbackPlan,
    ExactListItemGuardKind, ExactListItemGuardPlan, ExactListItemShape,
    ExactListItemSpecializationPlan, FailureMode, FallbackReason, FallbackTarget,
    FunctionOptimizationPlanV3, FunctionOwnershipPlan, FunctionPlanIdentity,
    IndexedFieldAccessKind, IndexedFieldFallbackKind, IndexedFieldFallbackPlan,
    IndexedFieldGuardKind, IndexedFieldGuardPlan, IndexedFieldOwnerType,
    IndexedFieldSpecializationPlan, IndexedGlobalAccessKind, IndexedGlobalFallbackKind,
    IndexedGlobalFallbackPlan, IndexedGlobalGuardKind, IndexedGlobalGuardPlan,
    IndexedGlobalSpecializationPlan, MaterializeKind, MaterializeNode, ModuleOptimizationPlanV3,
    ModulePlanIdentity, OperationNode, PlanDiagnostic, PlanNode, PlanNodeId, PlanNodeKind,
    PlanValue, PlannedConstant, RegionExitKind, RegionExitPlan, RegionExitTarget, RegionId,
    RegionInput, RegionInputSource, RegionPlan, RegionSource, Rep,
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
    pub exact_list_items: Vec<ExactListItemPlanRequest>,
    pub indexed_fields: Vec<IndexedFieldPlanRequest>,
    pub indexed_globals: Vec<IndexedGlobalPlanRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectCallPlanRequest {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub callee: DirectCallCallee,
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
    pub inline_target: Option<SerializedFunctionId>,
    pub reason: String,
}

impl CallBodyPlanRequest {
    pub fn with_inline_candidate(inline_candidate: bool) -> Self {
        let mut alternatives = vec![CallBodyAlternativeRequest {
            kind: CallBodyKind::DirectCall,
            cost: direct_call_body_cost_v3(),
            inline_target: None,
            reason: "guarded direct call is the baseline validated call-body alternative"
                .to_string(),
        }];
        if inline_candidate {
            alternatives.push(CallBodyAlternativeRequest {
                kind: CallBodyKind::Inline,
                cost: inline_call_body_cost_v3(),
                inline_target: None,
                reason: "lowered call shape is eligible for the early inline body alternative"
                    .to_string(),
            });
        }
        Self { alternatives }
    }

    pub fn with_inline_target_candidate(inline_target: Option<SerializedFunctionId>) -> Self {
        let mut alternatives = vec![CallBodyAlternativeRequest {
            kind: CallBodyKind::DirectCall,
            cost: direct_call_body_cost_v3(),
            inline_target: None,
            reason: "guarded direct call is the baseline validated call-body alternative"
                .to_string(),
        }];
        if let Some(inline_target) = inline_target {
            alternatives.push(CallBodyAlternativeRequest {
                kind: CallBodyKind::Inline,
                cost: inline_call_body_cost_v3(),
                inline_target: Some(inline_target),
                reason:
                    "runtime-iter body target is eligible for the early inline body alternative"
                        .to_string(),
            });
        }
        Self { alternatives }
    }
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
    exact_str_values: HashSet<ExtractedValueId>,
    i64_constants: HashMap<ExtractedValueId, i64>,
}

impl PlannerFacts {
    pub fn mark_exact_compact_int(&mut self, value: ExtractedValueId) {
        self.exact_compact_int_values.insert(value);
    }

    pub fn mark_exact_str(&mut self, value: ExtractedValueId) {
        self.exact_str_values.insert(value);
    }

    pub fn set_i64_constant(&mut self, value: ExtractedValueId, constant: i64) {
        self.i64_constants.insert(value, constant);
    }

    pub fn is_exact_compact_int(&self, value: ExtractedValueId) -> bool {
        self.exact_compact_int_values.contains(&value)
    }

    pub fn is_exact_str(&self, value: ExtractedValueId) -> bool {
        self.exact_str_values.contains(&value)
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
    let exact_list_items = plan_exact_list_item_specializations_v3(&request.exact_list_items);
    let indexed_fields = plan_indexed_field_specializations_v3(&request.indexed_fields);
    let indexed_globals = plan_indexed_global_specializations_v3(&request.indexed_globals);
    let indexed_global_loads_by_source = indexed_globals
        .iter()
        .filter(|plan| plan.access == IndexedGlobalAccessKind::Load)
        .map(|plan| (plan.source, plan.clone()))
        .collect::<HashMap<_, _>>();
    let mut function = FunctionOptimizationPlanV3 {
        function: request.function,
        regions: Vec::new(),
        direct_calls,
        exact_list_items,
        indexed_fields,
        indexed_globals,
        deopt_points: Vec::new(),
        ownership: FunctionOwnershipPlan::default(),
        diagnostics: Vec::new(),
    };

    for region_request in &region_requests {
        let region = &region_request.region;
        match plan_compact_int_branch(
            catalog,
            region,
            &region_request.facts,
            &indexed_global_loads_by_source,
        ) {
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

    function
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
                callee: request.callee.clone(),
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
            inline_target: alternative.inline_target,
            reason: alternative.reason.clone(),
        })
        .unwrap_or_else(|| CallBodyPlan {
            kind: CallBodyKind::DirectCall,
            cost: direct_call_body_cost_v3(),
            inline_target: None,
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
                guard: IndexedFieldGuardPlan {
                    kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
                },
                fallback: IndexedFieldFallbackPlan {
                    kind: IndexedFieldFallbackKind::OriginalAttrAccess,
                },
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
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Result<Option<Vec<RegionPlan>>, String> {
    if let Some(planned) =
        plan_compact_int_add_gt_zero_branch(catalog, region, facts, indexed_global_loads_by_source)?
    {
        return Ok(Some(planned));
    }
    if let Some(planned) = plan_compact_int_compare_constant_branch(
        catalog,
        region,
        facts,
        indexed_global_loads_by_source,
    )? {
        return Ok(Some(planned));
    }
    if let Some(planned) =
        plan_compact_int_compare_branch(catalog, region, facts, indexed_global_loads_by_source)?
    {
        return Ok(Some(planned));
    }
    if let Some(planned) =
        plan_compact_int_compare_return(catalog, region, facts, indexed_global_loads_by_source)?
    {
        return Ok(Some(planned));
    }
    if let Some(planned) =
        plan_exact_str_compare_branch(catalog, region, facts, indexed_global_loads_by_source)?
    {
        return Ok(Some(planned));
    }
    if let Some(planned) =
        plan_exact_str_compare_return(catalog, region, facts, indexed_global_loads_by_source)?
    {
        return Ok(Some(planned));
    }
    plan_compact_int_binary_return(catalog, region, facts, indexed_global_loads_by_source)
}

fn plan_compact_int_add_gt_zero_branch(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) =
        match_compact_int_add_gt_zero_branch(region, facts, indexed_global_loads_by_source)
    else {
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
    let fallback_a_obj = fallback_pyobject_input_value(0, &shape.left);
    let fallback_b_obj = fallback_pyobject_input_value(1, &shape.right);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape.source.unwrap_or_else(|| InstrId::new(0)),
        },
        inputs: vec![
            region_pyobject_input(a_obj, 0, shape.left.clone()),
            region_pyobject_input(b_obj, 1, shape.right.clone()),
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
            region_pyobject_input(fallback_a_obj, 0, shape.left),
            region_pyobject_input(fallback_b_obj, 1, shape.right),
        ],
        nodes: vec![
            operation_node(
                20,
                generic_add,
                vec![fallback_a_obj, fallback_b_obj],
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
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) =
        match_compact_int_compare_constant_branch(region, facts, indexed_global_loads_by_source)
    else {
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
    let fallback_value_obj = fallback_pyobject_input_value(0, &shape.value);
    let guard_index = if shape.constant_on_left { 1 } else { 0 };
    let hot_operands = if shape.constant_on_left {
        vec![constant_i64, value_i64]
    } else {
        vec![value_i64, constant_i64]
    };

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape.source.unwrap_or_else(|| InstrId::new(0)),
        },
        inputs: vec![region_pyobject_input(value_obj, 0, shape.value.clone())],
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
        vec![fallback_constant_obj, fallback_value_obj]
    } else {
        vec![fallback_value_obj, fallback_constant_obj]
    };
    let fallback_region = RegionPlan {
        id: fallback_region_id,
        source: RegionSource::Synthetic {
            reason: "generic fallback for compact-int comparison with constant branch".to_string(),
        },
        inputs: vec![region_pyobject_input(fallback_value_obj, 0, shape.value)],
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
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) =
        match_compact_int_binary_return(region, facts, indexed_global_loads_by_source)
    else {
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
    let fallback_a_obj = fallback_pyobject_input_value(0, &shape.left);
    let fallback_b_obj = fallback_pyobject_input_value(1, &shape.right);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape.source.unwrap_or_else(|| InstrId::new(0)),
        },
        inputs: vec![
            region_pyobject_input(a_obj, 0, shape.left.clone()),
            region_pyobject_input(b_obj, 1, shape.right.clone()),
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
            region_pyobject_input(fallback_a_obj, 0, shape.left),
            region_pyobject_input(fallback_b_obj, 1, shape.right),
        ],
        nodes: vec![operation_node(
            20,
            generic_operation,
            vec![fallback_a_obj, fallback_b_obj],
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
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) =
        match_compact_int_compare_branch(region, facts, indexed_global_loads_by_source)
    else {
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
    let fallback_a_obj = fallback_pyobject_input_value(0, &shape.left);
    let fallback_b_obj = fallback_pyobject_input_value(1, &shape.right);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape.source.unwrap_or_else(|| InstrId::new(0)),
        },
        inputs: vec![
            region_pyobject_input(a_obj, 0, shape.left.clone()),
            region_pyobject_input(b_obj, 1, shape.right.clone()),
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
            region_pyobject_input(fallback_a_obj, 0, shape.left),
            region_pyobject_input(fallback_b_obj, 1, shape.right),
        ],
        nodes: vec![
            operation_node(
                20,
                generic_compare,
                vec![fallback_a_obj, fallback_b_obj],
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

fn plan_exact_str_compare_branch(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) = match_exact_str_compare_branch(region, facts, indexed_global_loads_by_source)
    else {
        return Ok(None);
    };
    let compare = required_alternative(catalog, shape.compare.exact_str_id)?;
    let generic_compare = required_alternative(catalog, shape.compare.generic_id)?;
    let truthiness = required_alternative(catalog, "truthiness.pyobject")?;

    let fallback_region_id = RegionId(region.id.0 + 1);
    let failure_targets =
        FailureTargets::local_fallback(FallbackTarget::Region(fallback_region_id));

    let a_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
    let b_obj = PlanValue::new(1, Rep::PyObjectBorrowed);
    let condition = PlanValue::new(2, Rep::I32Bool01);
    let fallback_a_obj = fallback_pyobject_input_value(0, &shape.left);
    let fallback_b_obj = fallback_pyobject_input_value(1, &shape.right);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape.source.unwrap_or_else(|| InstrId::new(0)),
        },
        inputs: vec![
            region_pyobject_input(a_obj, 0, shape.left.clone()),
            region_pyobject_input(b_obj, 1, shape.right.clone()),
        ],
        nodes: vec![
            guard_node(compare, 0, 0, a_obj, &failure_targets),
            guard_node(compare, 1, 1, b_obj, &failure_targets),
            operation_node(
                2,
                compare,
                vec![a_obj, b_obj],
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
            reason: "generic fallback for exact-str comparison branch".to_string(),
        },
        inputs: vec![
            region_pyobject_input(fallback_a_obj, 0, shape.left),
            region_pyobject_input(fallback_b_obj, 1, shape.right),
        ],
        nodes: vec![
            operation_node(
                20,
                generic_compare,
                vec![fallback_a_obj, fallback_b_obj],
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
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) =
        match_compact_int_compare_return(region, facts, indexed_global_loads_by_source)
    else {
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
    let fallback_a_obj = fallback_pyobject_input_value(0, &shape.left);
    let fallback_b_obj = fallback_pyobject_input_value(1, &shape.right);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape.source.unwrap_or_else(|| InstrId::new(0)),
        },
        inputs: vec![
            region_pyobject_input(a_obj, 0, shape.left.clone()),
            region_pyobject_input(b_obj, 1, shape.right.clone()),
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
            region_pyobject_input(fallback_a_obj, 0, shape.left),
            region_pyobject_input(fallback_b_obj, 1, shape.right),
        ],
        nodes: vec![operation_node(
            20,
            generic_compare,
            vec![fallback_a_obj, fallback_b_obj],
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

fn plan_exact_str_compare_return(
    catalog: &AlternativeCatalog,
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Result<Option<Vec<RegionPlan>>, String> {
    let Some(shape) = match_exact_str_compare_return(region, facts, indexed_global_loads_by_source)
    else {
        return Ok(None);
    };
    let compare = required_alternative(catalog, shape.compare.exact_str_id)?;
    let generic_compare = required_alternative(catalog, shape.compare.generic_id)?;

    let fallback_region_id = RegionId(region.id.0 + 1);
    let failure_targets =
        FailureTargets::local_fallback(FallbackTarget::Region(fallback_region_id));

    let a_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
    let b_obj = PlanValue::new(1, Rep::PyObjectBorrowed);
    let condition = PlanValue::new(2, Rep::I32Bool01);
    let result_obj = PlanValue::new(3, Rep::PyObjectImmortal);
    let fallback_a_obj = fallback_pyobject_input_value(0, &shape.left);
    let fallback_b_obj = fallback_pyobject_input_value(1, &shape.right);

    let hot_region = RegionPlan {
        id: region.id,
        source: RegionSource::Instr {
            instr_id: shape.source.unwrap_or_else(|| InstrId::new(0)),
        },
        inputs: vec![
            region_pyobject_input(a_obj, 0, shape.left.clone()),
            region_pyobject_input(b_obj, 1, shape.right.clone()),
        ],
        nodes: vec![
            guard_node(compare, 0, 0, a_obj, &failure_targets),
            guard_node(compare, 1, 1, b_obj, &failure_targets),
            operation_node(
                2,
                compare,
                vec![a_obj, b_obj],
                Some(condition),
                &failure_targets,
            )?,
            node(
                3,
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
            reason: "generic fallback for exact-str comparison return".to_string(),
        },
        inputs: vec![
            region_pyobject_input(fallback_a_obj, 0, shape.left),
            region_pyobject_input(fallback_b_obj, 1, shape.right),
        ],
        nodes: vec![operation_node(
            20,
            generic_compare,
            vec![fallback_a_obj, fallback_b_obj],
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
    left: PyObjectRegionInputSource,
    right: PyObjectRegionInputSource,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntCompareBranchShape {
    source: Option<InstrId>,
    left: PyObjectRegionInputSource,
    right: PyObjectRegionInputSource,
    compare: CompareAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExactStrCompareBranchShape {
    source: Option<InstrId>,
    left: PyObjectRegionInputSource,
    right: PyObjectRegionInputSource,
    compare: CompareAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntCompareConstantBranchShape {
    source: Option<InstrId>,
    value: PyObjectRegionInputSource,
    constant: i64,
    constant_on_left: bool,
    compare: CompareAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntReturnShape {
    source: Option<InstrId>,
    left: PyObjectRegionInputSource,
    right: PyObjectRegionInputSource,
    operation: BinaryReturnAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CompactIntCompareReturnShape {
    source: Option<InstrId>,
    left: PyObjectRegionInputSource,
    right: PyObjectRegionInputSource,
    compare: CompareAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExactStrCompareReturnShape {
    source: Option<InstrId>,
    left: PyObjectRegionInputSource,
    right: PyObjectRegionInputSource,
    compare: CompareAlternativeSpec,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum PyObjectRegionInputSource {
    LocalName(String),
    ModuleConstant(u32),
    IndexedGlobal {
        source: InstrId,
        module_name: String,
        name: String,
        expected_index: u32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CompareAlternativeSpec {
    generic_id: &'static str,
    exact_id: &'static str,
    exact_str_id: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BinaryReturnAlternativeSpec {
    generic_id: &'static str,
    exact_id: &'static str,
}

fn match_compact_int_add_gt_zero_branch(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
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
        left: region.pyobject_input_source(left, indexed_global_loads_by_source)?,
        right: region.pyobject_input_source(right, indexed_global_loads_by_source)?,
    })
}

fn match_compact_int_binary_return(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
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
        left: region.pyobject_input_source(left, indexed_global_loads_by_source)?,
        right: region.pyobject_input_source(right, indexed_global_loads_by_source)?,
        operation,
    })
}

fn match_compact_int_compare_constant_branch(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
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
                value: region.pyobject_input_source(left, indexed_global_loads_by_source)?,
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
                value: region.pyobject_input_source(right, indexed_global_loads_by_source)?,
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
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
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
        left: region.pyobject_input_source(left, indexed_global_loads_by_source)?,
        right: region.pyobject_input_source(right, indexed_global_loads_by_source)?,
        compare,
    })
}

fn match_exact_str_compare_branch(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Option<ExactStrCompareBranchShape> {
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
    if !facts.is_exact_str(left) || !facts.is_exact_str(right) {
        return None;
    }
    Some(ExactStrCompareBranchShape {
        source,
        left: region.pyobject_input_source(left, indexed_global_loads_by_source)?,
        right: region.pyobject_input_source(right, indexed_global_loads_by_source)?,
        compare,
    })
}

fn match_compact_int_compare_return(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
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
        left: region.pyobject_input_source(left, indexed_global_loads_by_source)?,
        right: region.pyobject_input_source(right, indexed_global_loads_by_source)?,
        compare,
    })
}

fn match_exact_str_compare_return(
    region: &ExtractedRegion,
    facts: &PlannerFacts,
    indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
) -> Option<ExactStrCompareReturnShape> {
    let ExtractedExit::Return { value, source } = region.exit else {
        return None;
    };
    let compare = region.value(value)?;
    let ExtractedValueKind::Binary { op, left, right } = compare.kind else {
        return None;
    };
    let compare = compare_alternative_spec(op)?;
    if !facts.is_exact_str(left) || !facts.is_exact_str(right) {
        return None;
    }
    Some(ExactStrCompareReturnShape {
        source,
        left: region.pyobject_input_source(left, indexed_global_loads_by_source)?,
        right: region.pyobject_input_source(right, indexed_global_loads_by_source)?,
        compare,
    })
}

fn compare_alternative_spec(op: BinOpKind) -> Option<CompareAlternativeSpec> {
    Some(match op {
        BinOpKind::Eq => CompareAlternativeSpec {
            generic_id: "binary.eq.py_richcompare",
            exact_id: "binary.eq.exact_compact_int.i32bool",
            exact_str_id: "binary.eq.exact_str.i32bool",
        },
        BinOpKind::Ne => CompareAlternativeSpec {
            generic_id: "binary.ne.py_richcompare",
            exact_id: "binary.ne.exact_compact_int.i32bool",
            exact_str_id: "binary.ne.exact_str.i32bool",
        },
        BinOpKind::Lt => CompareAlternativeSpec {
            generic_id: "binary.lt.py_richcompare",
            exact_id: "binary.lt.exact_compact_int.i32bool",
            exact_str_id: "binary.lt.exact_str.i32bool",
        },
        BinOpKind::Le => CompareAlternativeSpec {
            generic_id: "binary.le.py_richcompare",
            exact_id: "binary.le.exact_compact_int.i32bool",
            exact_str_id: "binary.le.exact_str.i32bool",
        },
        BinOpKind::Gt => CompareAlternativeSpec {
            generic_id: "binary.gt.py_richcompare",
            exact_id: "binary.gt.exact_compact_int.i32bool",
            exact_str_id: "binary.gt.exact_str.i32bool",
        },
        BinOpKind::Ge => CompareAlternativeSpec {
            generic_id: "binary.ge.py_richcompare",
            exact_id: "binary.ge.exact_compact_int.i32bool",
            exact_str_id: "binary.ge.exact_str.i32bool",
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
    fn pyobject_input_source(
        &self,
        value: ExtractedValueId,
        indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
    ) -> Option<PyObjectRegionInputSource>;
    fn exit_source(&self) -> Option<InstrId>;
}

impl ExtractedRegionExt for ExtractedRegion {
    fn value(&self, value: ExtractedValueId) -> Option<&ExtractedValue> {
        self.values.iter().find(|entry| entry.id == value)
    }

    fn pyobject_input_source(
        &self,
        value: ExtractedValueId,
        indexed_global_loads_by_source: &HashMap<InstrId, IndexedGlobalSpecializationPlan>,
    ) -> Option<PyObjectRegionInputSource> {
        let value = self.value(value)?;
        let ExtractedValueKind::LoadName { name } = &value.kind else {
            return None;
        };
        match name.location {
            NameLocation::Local(_) | NameLocation::Cell(_) => Some(
                PyObjectRegionInputSource::LocalName(name.id_str().to_string()),
            ),
            NameLocation::Constant(index) => Some(PyObjectRegionInputSource::ModuleConstant(index)),
            NameLocation::Global(slot) => {
                let source = value.source?;
                let plan = indexed_global_loads_by_source.get(&source)?;
                if plan.name != name.id_str() || plan.expected_index != slot.slot() {
                    return None;
                }
                Some(PyObjectRegionInputSource::IndexedGlobal {
                    source,
                    module_name: plan.module_name.clone(),
                    name: plan.name.clone(),
                    expected_index: plan.expected_index,
                })
            }
            NameLocation::GlobalName | NameLocation::RuntimeName(_) => None,
        }
    }

    fn exit_source(&self) -> Option<InstrId> {
        match &self.exit {
            ExtractedExit::Branch { source, .. } | ExtractedExit::Return { source, .. } => *source,
        }
    }
}

fn region_pyobject_input(
    value: PlanValue,
    index: u32,
    source: PyObjectRegionInputSource,
) -> RegionInput {
    let source = match source {
        PyObjectRegionInputSource::LocalName(name) => RegionInputSource::FunctionParam {
            index,
            name: Some(name),
        },
        PyObjectRegionInputSource::ModuleConstant(index) => {
            RegionInputSource::ModuleConstant { index }
        }
        PyObjectRegionInputSource::IndexedGlobal {
            source,
            module_name,
            name,
            expected_index,
        } => RegionInputSource::IndexedGlobal {
            source,
            module_name,
            name,
            expected_index,
        },
    };
    RegionInput { value, source }
}

fn fallback_pyobject_input_value(id: u32, source: &PyObjectRegionInputSource) -> PlanValue {
    let rep = match source {
        PyObjectRegionInputSource::IndexedGlobal { .. } => Rep::PyObjectOwned,
        PyObjectRegionInputSource::LocalName(_) | PyObjectRegionInputSource::ModuleConstant(_) => {
            Rep::PyObjectBorrowed
        }
    };
    PlanValue::new(id, rep)
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
    use crate::region_v3::extract_block_region_v3;
    use soac_core::block_py::{
        BinOp, Block, BlockLabel, BlockParam, BlockPyName, BlockTerm, Load, LocalFunctionId,
        LocalLocation, Meta, NameLocation, ResolvedName, SerializedFunctionId,
        SerializedIdentityTables, SerializedModuleId, SerializedModuleIdentity, TermIf, WithMeta,
    };
    use soac_ir_blockpy::InstrBlockPy;
    use soac_ir_typed::plan_v3::RichCompareOp;
    use soac_ir_typed::plan_v3::validate_module_plan_v3;

    fn label(index: usize) -> BlockLabel {
        BlockLabel::from_index(index)
    }

    fn instr_id_in_label(_block: BlockLabel, index: u32) -> InstrId {
        InstrId::new(index)
    }

    fn with_instr_id(instr: InstrBlockPy, index: u32) -> InstrBlockPy {
        with_instr_id_in_label(instr, label(0), index)
    }

    fn with_instr_id_in_label(instr: InstrBlockPy, block: BlockLabel, index: u32) -> InstrBlockPy {
        instr.with_meta(Meta {
            instr_id: Some(instr_id_in_label(block, index)),
            ..Meta::synthetic()
        })
    }

    fn local(name: &str, slot: u32) -> InstrBlockPy {
        InstrBlockPy::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Local(LocalLocation(slot)),
        }))
    }

    fn constant(slot: u32) -> InstrBlockPy {
        InstrBlockPy::Load(Load::new(ResolvedName {
            id: BlockPyName::new("__dp_constant"),
            location: NameLocation::Constant(slot),
        }))
    }

    fn global(name: &str, slot: u32) -> InstrBlockPy {
        InstrBlockPy::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Global(soac_core::block_py::GlobalSlot(slot)),
        }))
    }

    fn binary(op: BinOpKind, left: InstrBlockPy, right: InstrBlockPy, id: u32) -> InstrBlockPy {
        with_instr_id(InstrBlockPy::BinOp(BinOp::new(op, left, right)), id)
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

    fn compact_int_compare_branch_with_global_region(kind: BinOpKind) -> ExtractedRegion {
        let test = binary(
            kind,
            with_instr_id(local("a", 0), 0),
            with_instr_id(global("IntGlob", 7), 1),
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

    fn exact_str_compare_return_region(kind: BinOpKind) -> ExtractedRegion {
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

    fn exact_str_compare_return_with_constant_region(kind: BinOpKind) -> ExtractedRegion {
        let value = binary(
            kind,
            with_instr_id(local("a", 0), 0),
            with_instr_id(constant(7), 1),
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

    fn exact_str_compare_branch_with_global_region(kind: BinOpKind) -> ExtractedRegion {
        let test = binary(
            kind,
            with_instr_id(local("a", 0), 0),
            with_instr_id(global("Char1Glob", 7), 1),
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

    fn compact_int_binary_return_with_global_region(kind: BinOpKind) -> ExtractedRegion {
        let value = binary(
            kind,
            with_instr_id(local("a", 0), 0),
            with_instr_id(global("IntGlob", 7), 1),
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

    fn facts_for_exact_str_region() -> PlannerFacts {
        let mut facts = PlannerFacts::default();
        facts.mark_exact_str(ExtractedValueId(0));
        facts.mark_exact_str(ExtractedValueId(1));
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
                exact_list_items: Vec::new(),
                indexed_fields: Vec::new(),
                indexed_globals: Vec::new(),
            }],
        }
    }

    fn add_test_indexed_global(request: &mut ModulePlanRequest, name: &str) {
        request.functions[0].indexed_globals = vec![IndexedGlobalPlanRequest {
            source: InstrId::new(1),
            access: IndexedGlobalAccessKind::Load,
            module_name: "pkg.mod".to_string(),
            name: name.to_string(),
            expected_index: 7,
            reason: "test indexed global".to_string(),
        }];
    }

    #[test]
    fn plans_same_module_direct_call_selection_from_profiled_targets() {
        let mut request = module_request_regions(Vec::new());
        let source = InstrId::new(9);
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        request.functions[0].direct_calls = vec![
            DirectCallPlanRequest {
                source,
                target,
                callee: DirectCallCallee::Function,
                arg_plan: DirectCallArgPlan {
                    sources: vec![soac_ir_typed::plan_v3::DirectCallArgSource::Provided(0)],
                },
                body: CallBodyPlanRequest::with_inline_candidate(true),
                reason: "profiled call_hot_targets selected this same-module function".to_string(),
            },
            DirectCallPlanRequest {
                source,
                target,
                callee: DirectCallCallee::Function,
                arg_plan: DirectCallArgPlan {
                    sources: vec![soac_ir_typed::plan_v3::DirectCallArgSource::Provided(0)],
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
                sources: vec![soac_ir_typed::plan_v3::DirectCallArgSource::Provided(0)]
            }
        );
        assert_eq!(direct_calls[0].body.kind, CallBodyKind::Inline);
        assert!(direct_calls[0].reason.contains("call_hot_targets"));
    }

    #[test]
    fn direct_call_body_cost_model_declines_inline_without_candidate() {
        let mut request = module_request_regions(Vec::new());
        let source = InstrId::new(9);
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        request.functions[0].direct_calls = vec![DirectCallPlanRequest {
            source,
            target,
            callee: DirectCallCallee::Function,
            arg_plan: DirectCallArgPlan {
                sources: vec![soac_ir_typed::plan_v3::DirectCallArgSource::Provided(0)],
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
        let get_source = InstrId::new(9);
        let set_source = InstrId::new(11);
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
        let load_source = InstrId::new(9);
        let store_source = InstrId::new(3);
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
                op: soac_ir_typed::plan_v3::PlannedOp::CheckedI64Add,
                ..
            })
        ));
        assert!(matches!(
            function.regions[0].nodes[6].kind,
            PlanNodeKind::Operation(OperationNode {
                op: soac_ir_typed::plan_v3::PlannedOp::I64CompareToBool01 {
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
                op: soac_ir_typed::plan_v3::PlannedOp::I64CompareToBool01 {
                    op: RichCompareOp::Lt
                },
                ..
            })
        ));
        assert_eq!(function.regions[1].nodes.len(), 2);
        assert!(matches!(
            function.regions[1].nodes[0].kind,
            PlanNodeKind::Operation(OperationNode {
                op: soac_ir_typed::plan_v3::PlannedOp::PyObjectRichCompare {
                    op: RichCompareOp::Lt
                },
                ..
            })
        ));
    }

    #[test]
    fn plans_compact_int_compare_branch_with_indexed_global_operand() {
        let catalog = AlternativeCatalog::default_v3();
        let mut request = module_request(
            compact_int_compare_branch_with_global_region(BinOpKind::Lt),
            facts_for_compact_region(),
        );
        add_test_indexed_global(&mut request, "IntGlob");
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(
            function.regions[0].inputs[1].source,
            RegionInputSource::IndexedGlobal {
                source: InstrId::new(1),
                module_name: "pkg.mod".to_string(),
                name: "IntGlob".to_string(),
                expected_index: 7,
            }
        );
        assert_eq!(
            function.regions[0].inputs[1].value.rep,
            Rep::PyObjectBorrowed
        );
        assert_eq!(function.regions[1].inputs[1].value.rep, Rep::PyObjectOwned);
        assert!(matches!(
            function.regions[0].nodes[4].kind,
            PlanNodeKind::Operation(OperationNode {
                op: soac_ir_typed::plan_v3::PlannedOp::I64CompareToBool01 {
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
                op: soac_ir_typed::plan_v3::PlannedOp::I64CompareToBool01 {
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
                op: soac_ir_typed::plan_v3::PlannedOp::PyObjectRichCompare {
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
                op: soac_ir_typed::plan_v3::PlannedOp::CheckedI64Add,
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
                op: soac_ir_typed::plan_v3::PlannedOp::PyNumberAdd,
                ..
            })
        ));
    }

    #[test]
    fn plans_compact_int_add_return_with_indexed_global_operand() {
        let catalog = AlternativeCatalog::default_v3();
        let mut request = module_request(
            compact_int_binary_return_with_global_region(BinOpKind::Add),
            facts_for_compact_region(),
        );
        add_test_indexed_global(&mut request, "IntGlob");
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(
            function.regions[0].inputs[1].source,
            RegionInputSource::IndexedGlobal {
                source: InstrId::new(1),
                module_name: "pkg.mod".to_string(),
                name: "IntGlob".to_string(),
                expected_index: 7,
            }
        );
        assert_eq!(
            function.regions[0].inputs[1].value.rep,
            Rep::PyObjectBorrowed
        );
        assert_eq!(function.regions[1].inputs[1].value.rep, Rep::PyObjectOwned);
        assert!(matches!(
            function.regions[0].nodes[4].kind,
            PlanNodeKind::Operation(OperationNode {
                op: soac_ir_typed::plan_v3::PlannedOp::CheckedI64Add,
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
                soac_ir_typed::plan_v3::PlannedOp::CheckedI64Sub,
                soac_ir_typed::plan_v3::PlannedOp::PyNumberSubtract,
            ),
            (
                BinOpKind::Mul,
                soac_ir_typed::plan_v3::PlannedOp::CheckedI64Mul,
                soac_ir_typed::plan_v3::PlannedOp::PyNumberMultiply,
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
                soac_ir_typed::plan_v3::PlannedOp::I64BitAnd,
                soac_ir_typed::plan_v3::PlannedOp::PyNumberBitAnd,
            ),
            (
                BinOpKind::Or,
                soac_ir_typed::plan_v3::PlannedOp::I64BitOr,
                soac_ir_typed::plan_v3::PlannedOp::PyNumberBitOr,
            ),
            (
                BinOpKind::Xor,
                soac_ir_typed::plan_v3::PlannedOp::I64BitXor,
                soac_ir_typed::plan_v3::PlannedOp::PyNumberBitXor,
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
                op: soac_ir_typed::plan_v3::PlannedOp::I64CompareToBool01 {
                    op: RichCompareOp::Gt
                },
                ..
            })
        ));
    }

    #[test]
    fn plans_exact_str_compare_return_as_python_bool() {
        let catalog = AlternativeCatalog::default_v3();
        let request = module_request(
            exact_str_compare_return_region(BinOpKind::Ge),
            facts_for_exact_str_region(),
        );
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert!(matches!(
            function.regions[0].nodes[2].kind,
            PlanNodeKind::Operation(OperationNode {
                op: soac_ir_typed::plan_v3::PlannedOp::PyObjectRichCompareBool {
                    op: RichCompareOp::Ge
                },
                ..
            })
        ));
        assert!(matches!(
            function.regions[0].nodes[3].kind,
            PlanNodeKind::Materialize(MaterializeNode {
                kind: MaterializeKind::PythonBool,
                ..
            })
        ));
        assert!(matches!(
            function.regions[1].nodes[0].kind,
            PlanNodeKind::Operation(OperationNode {
                op: soac_ir_typed::plan_v3::PlannedOp::PyObjectRichCompare {
                    op: RichCompareOp::Ge
                },
                ..
            })
        ));
    }

    #[test]
    fn plans_exact_str_compare_return_with_module_constant_operand() {
        let catalog = AlternativeCatalog::default_v3();
        let request = module_request(
            exact_str_compare_return_with_constant_region(BinOpKind::Le),
            facts_for_exact_str_region(),
        );
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(
            function.regions[0].inputs[0].source,
            RegionInputSource::FunctionParam {
                index: 0,
                name: Some("a".to_string())
            }
        );
        assert_eq!(
            function.regions[0].inputs[1].source,
            RegionInputSource::ModuleConstant { index: 7 }
        );
        assert!(matches!(
            function.regions[0].nodes[2].kind,
            PlanNodeKind::Operation(OperationNode {
                op: soac_ir_typed::plan_v3::PlannedOp::PyObjectRichCompareBool {
                    op: RichCompareOp::Le
                },
                ..
            })
        ));
    }

    #[test]
    fn plans_exact_str_compare_branch_with_indexed_global_operand() {
        let catalog = AlternativeCatalog::default_v3();
        let mut request = module_request(
            exact_str_compare_branch_with_global_region(BinOpKind::Eq),
            facts_for_exact_str_region(),
        );
        request.functions[0].indexed_globals = vec![IndexedGlobalPlanRequest {
            source: InstrId::new(1),
            access: IndexedGlobalAccessKind::Load,
            module_name: "pkg.mod".to_string(),
            name: "Char1Glob".to_string(),
            expected_index: 7,
            reason: "test indexed global".to_string(),
        }];
        let plan = plan_module_optimization_v3(&catalog, request);

        validate_module_plan_v3(&plan).unwrap();
        let function = &plan.functions[0];
        assert!(function.diagnostics.is_empty());
        assert_eq!(function.regions.len(), 2);
        assert_eq!(
            function.regions[0].inputs[1].source,
            RegionInputSource::IndexedGlobal {
                source: InstrId::new(1),
                module_name: "pkg.mod".to_string(),
                name: "Char1Glob".to_string(),
                expected_index: 7,
            }
        );
        assert_eq!(
            function.regions[0].inputs[1].value.rep,
            Rep::PyObjectBorrowed
        );
        assert_eq!(function.regions[1].inputs[1].value.rep, Rep::PyObjectOwned);
        assert!(matches!(
            function.regions[0].nodes[2].kind,
            PlanNodeKind::Operation(OperationNode {
                op: soac_ir_typed::plan_v3::PlannedOp::PyObjectRichCompareBool {
                    op: RichCompareOp::Eq
                },
                ..
            })
        ));
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
