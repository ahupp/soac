use crate::optimization_alternatives_v3::{
    ALTERNATIVE_CATALOG_V3_VERSION, AlternativeCatalog, AlternativeId, FailureTargets,
    LoweringAlternative,
};
use crate::optimization_plan_v3::{
    ConversionKind, ConversionOwnership, ConversionPrecondition, ConvertNode, FailureMode,
    FallbackReason, FallbackTarget, FunctionOptimizationPlanV3, FunctionOwnershipPlan,
    FunctionPlanIdentity, MaterializeKind, MaterializeNode, ModuleOptimizationPlanV3,
    ModulePlanIdentity, OperationNode, PlanDiagnostic, PlanNode, PlanNodeId, PlanNodeKind,
    PlanValue, PlannedConstant, RegionExitKind, RegionExitPlan, RegionExitTarget, RegionId,
    RegionInput, RegionInputSource, RegionPlan, RegionSource, Rep, RichCompareOp,
};
use crate::optimization_region_v3::{
    ExtractedExit, ExtractedRegion, ExtractedValue, ExtractedValueId, ExtractedValueKind,
};
use soac_core::block_py::{BinOpKind, InstrId, NameLike, ResolvedName};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModulePlanRequest {
    pub module: ModulePlanIdentity,
    pub functions: Vec<FunctionPlanRequest>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionPlanRequest {
    pub function: FunctionPlanIdentity,
    pub regions: Vec<ExtractedRegion>,
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
    let mut function = FunctionOptimizationPlanV3 {
        function: request.function,
        regions: Vec::new(),
        deopt_points: Vec::new(),
        ownership: FunctionOwnershipPlan::default(),
        diagnostics: Vec::new(),
    };

    for region in &request.regions {
        match plan_compact_int_add_gt_zero_branch(catalog, region, &request.facts) {
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
        left_name: region.load_name(left)?.id_str().to_string(),
        right_name: region.load_name(right)?.id_str().to_string(),
    })
}

trait ExtractedRegionExt {
    fn value(&self, value: ExtractedValueId) -> Option<&ExtractedValue>;
    fn load_name(&self, value: ExtractedValueId) -> Option<&ResolvedName>;
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
    use crate::optimization_plan_v3::validate_module_plan_v3;
    use crate::optimization_region_v3::extract_block_region_v3;
    use soac_core::block_py::{
        BinOp, Block, BlockLabel, BlockParam, BlockPyName, BlockTerm, Load, LocalFunctionId,
        LocalLocation, Meta, NameLocation, SerializedFunctionId, SerializedModuleId, TermIf,
        WithMeta,
    };
    use soac_lowering::passes::InstrCodegen;

    fn label(index: usize) -> BlockLabel {
        BlockLabel::from_index(index)
    }

    fn instr_id(index: u32) -> InstrId {
        InstrId::new(label(0), index)
    }

    fn with_instr_id(instr: InstrCodegen, index: u32) -> InstrCodegen {
        instr.with_meta(Meta {
            instr_id: Some(instr_id(index)),
            ..Meta::synthetic()
        })
    }

    fn local(name: &str, slot: u32) -> InstrCodegen {
        InstrCodegen::Load(Load::new(ResolvedName {
            id: BlockPyName::new(name),
            location: NameLocation::Local(LocalLocation(slot)),
        }))
    }

    fn binary(op: BinOpKind, left: InstrCodegen, right: InstrCodegen, id: u32) -> InstrCodegen {
        with_instr_id(InstrCodegen::BinOp(BinOp::new(op, left, right)), id)
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

    fn facts_for_compact_region() -> PlannerFacts {
        let mut facts = PlannerFacts::default();
        facts.mark_exact_compact_int(ExtractedValueId(0));
        facts.mark_exact_compact_int(ExtractedValueId(1));
        facts.set_i64_constant(ExtractedValueId(3), 0);
        facts
    }

    fn module_request(region: ExtractedRegion, facts: PlannerFacts) -> ModulePlanRequest {
        ModulePlanRequest {
            module: ModulePlanIdentity {
                module_name: "pkg.mod".to_string(),
                source_hash: 0x55,
                cache_identity: "test-cache".to_string(),
            },
            functions: vec![FunctionPlanRequest {
                function: FunctionPlanIdentity {
                    function: SerializedFunctionId::new(
                        SerializedModuleId::new(0),
                        LocalFunctionId::new(1),
                    ),
                    debug_name: Some("f".to_string()),
                },
                regions: vec![region],
                facts,
            }],
        }
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
                op: crate::optimization_plan_v3::PlannedOp::CheckedI64Add,
                ..
            })
        ));
        assert!(matches!(
            function.regions[0].nodes[6].kind,
            PlanNodeKind::Operation(OperationNode {
                op: crate::optimization_plan_v3::PlannedOp::I64CompareToBool01 {
                    op: RichCompareOp::Gt
                },
                ..
            })
        ));
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
