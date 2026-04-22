use crate::plan_v3::{
    CallBodyPlan, ConstructorCallFallbackPlan, ConstructorCallGuardPlan, ConstructorCallOwnerType,
    ConstructorCallSpecializationPlan, ConversionKind, DeoptPointId, DirectCallArgPlan,
    DirectCallSpecializationPlan, ExactListItemAccessKind, ExactListItemFallbackPlan,
    ExactListItemGuardPlan, ExactListItemShape, ExactListItemSpecializationPlan, FailureMode,
    GuardFailure, GuardKind, IndexedFieldAccessKind, IndexedFieldOwnerType,
    IndexedFieldSpecializationPlan, IndexedGlobalAccessKind, IndexedGlobalFallbackPlan,
    IndexedGlobalGuardPlan, IndexedGlobalSpecializationPlan, MaterializeKind,
    MethodCallFallbackPlan, MethodCallGuardPlan, MethodCallOwnerType, MethodCallSpecializationPlan,
    ModuleOptimizationPlanV3, OperationNode, PlanNodeId, PlanNodeKind, PlanValidationError,
    PlanValue, PlannedConstant, PlannedOp, RegionExitKind, RegionExitTarget, RegionId,
    RichCompareOp, validate_module_plan_v3,
};
use soac_core::block_py::{InstrId, SerializedFunctionId};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalModuleEmission {
    pub module_name: String,
    pub functions: Vec<MechanicalFunctionEmission>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalFunctionEmission {
    pub function: SerializedFunctionId,
    pub debug_name: Option<String>,
    pub direct_calls: Vec<MechanicalDirectCallEmission>,
    pub constructor_calls: Vec<MechanicalConstructorCallEmission>,
    pub method_calls: Vec<MechanicalMethodCallEmission>,
    pub exact_list_items: Vec<MechanicalExactListItemEmission>,
    pub indexed_fields: Vec<MechanicalIndexedFieldEmission>,
    pub indexed_globals: Vec<MechanicalIndexedGlobalEmission>,
    pub regions: Vec<MechanicalRegionEmission>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalDirectCallEmission {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub arg_plan: DirectCallArgPlan,
    pub body: CallBodyPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalConstructorCallEmission {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub owner_type: ConstructorCallOwnerType,
    pub arg_plan: DirectCallArgPlan,
    pub guard: ConstructorCallGuardPlan,
    pub fallback: ConstructorCallFallbackPlan,
    pub body: CallBodyPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalMethodCallEmission {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub method_name: String,
    pub owner_type: MethodCallOwnerType,
    pub arg_plan: DirectCallArgPlan,
    pub guard: MethodCallGuardPlan,
    pub fallback: MethodCallFallbackPlan,
    pub body: CallBodyPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalExactListItemEmission {
    pub source: InstrId,
    pub access: ExactListItemAccessKind,
    pub shape: ExactListItemShape,
    pub guard: ExactListItemGuardPlan,
    pub fallback: ExactListItemFallbackPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalIndexedFieldEmission {
    pub source: InstrId,
    pub access: IndexedFieldAccessKind,
    pub owner_type: IndexedFieldOwnerType,
    pub attr_name: String,
    pub expected_index: u32,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalIndexedGlobalEmission {
    pub source: InstrId,
    pub access: IndexedGlobalAccessKind,
    pub module_name: String,
    pub name: String,
    pub expected_index: u32,
    pub guard: IndexedGlobalGuardPlan,
    pub fallback: IndexedGlobalFallbackPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalRegionEmission {
    pub region: RegionId,
    pub steps: Vec<MechanicalStep>,
    pub exits: Vec<MechanicalExit>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalStep {
    pub node: PlanNodeId,
    pub source: Option<InstrId>,
    pub op: MechanicalStepOp,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum MechanicalStepOp {
    Input {
        output: PlanValue,
    },
    Constant {
        output: PlanValue,
        constant: PlannedConstant,
    },
    Convert {
        kind: ConversionKind,
        input: PlanValue,
        output: PlanValue,
        failure: FailureMode,
    },
    Guard {
        kind: GuardKind,
        inputs: Vec<PlanValue>,
        failure: GuardFailure,
    },
    Operation {
        op: MechanicalOperation,
        inputs: Vec<PlanValue>,
        output: Option<PlanValue>,
        failure: FailureMode,
    },
    Materialize {
        kind: MaterializeKind,
        input: PlanValue,
        output: PlanValue,
    },
    Fallback {
        target: crate::plan_v3::FallbackTarget,
    },
    Deopt {
        target: DeoptPointId,
    },
    Ownership {
        action: crate::plan_v3::OwnershipAction,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum MechanicalOperation {
    PyNumberAdd,
    PyNumberSubtract,
    PyNumberMultiply,
    PyNumberBitAnd,
    PyNumberBitOr,
    PyNumberBitXor,
    PyObjectRichCompare { op: RichCompareOp },
    PyObjectIsTrue,
    CheckedI64Add,
    CheckedI64Sub,
    CheckedI64Mul,
    I64BitAnd,
    I64BitOr,
    I64BitXor,
    I64CompareToBool01 { op: RichCompareOp },
    DirectHelper { name: String },
}

impl From<&PlannedOp> for MechanicalOperation {
    fn from(value: &PlannedOp) -> Self {
        match value {
            PlannedOp::PyNumberAdd => Self::PyNumberAdd,
            PlannedOp::PyNumberSubtract => Self::PyNumberSubtract,
            PlannedOp::PyNumberMultiply => Self::PyNumberMultiply,
            PlannedOp::PyNumberBitAnd => Self::PyNumberBitAnd,
            PlannedOp::PyNumberBitOr => Self::PyNumberBitOr,
            PlannedOp::PyNumberBitXor => Self::PyNumberBitXor,
            PlannedOp::PyObjectRichCompare { op } => Self::PyObjectRichCompare { op: *op },
            PlannedOp::PyObjectIsTrue => Self::PyObjectIsTrue,
            PlannedOp::CheckedI64Add => Self::CheckedI64Add,
            PlannedOp::CheckedI64Sub => Self::CheckedI64Sub,
            PlannedOp::CheckedI64Mul => Self::CheckedI64Mul,
            PlannedOp::I64BitAnd => Self::I64BitAnd,
            PlannedOp::I64BitOr => Self::I64BitOr,
            PlannedOp::I64BitXor => Self::I64BitXor,
            PlannedOp::I64CompareToBool01 { op } => Self::I64CompareToBool01 { op: *op },
            PlannedOp::DirectHelper { name } => Self::DirectHelper { name: name.clone() },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalExit {
    pub source: Option<InstrId>,
    pub kind: MechanicalExitKind,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum MechanicalExitKind {
    Branch {
        condition: PlanValue,
        then_target: RegionExitTarget,
        else_target: RegionExitTarget,
    },
    Return {
        value: PlanValue,
    },
    Jump {
        target: RegionExitTarget,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MechanicalEmitError {
    InvalidPlan(PlanValidationError),
}

impl fmt::Display for MechanicalEmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPlan(err) => write!(f, "invalid optimization plan v3: {err}"),
        }
    }
}

impl std::error::Error for MechanicalEmitError {}

pub fn emit_mechanical_plan_v3(
    plan: &ModuleOptimizationPlanV3,
) -> Result<MechanicalModuleEmission, MechanicalEmitError> {
    validate_module_plan_v3(plan).map_err(MechanicalEmitError::InvalidPlan)?;
    Ok(MechanicalModuleEmission {
        module_name: plan.module.module_name.clone(),
        functions: plan
            .functions
            .iter()
            .map(|function| MechanicalFunctionEmission {
                function: function.function.function,
                debug_name: function.function.debug_name.clone(),
                direct_calls: function.direct_calls.iter().map(emit_direct_call).collect(),
                constructor_calls: function
                    .constructor_calls
                    .iter()
                    .map(emit_constructor_call)
                    .collect(),
                method_calls: function.method_calls.iter().map(emit_method_call).collect(),
                exact_list_items: function
                    .exact_list_items
                    .iter()
                    .map(emit_exact_list_item)
                    .collect(),
                indexed_fields: function
                    .indexed_fields
                    .iter()
                    .map(emit_indexed_field)
                    .collect(),
                indexed_globals: function
                    .indexed_globals
                    .iter()
                    .map(emit_indexed_global)
                    .collect(),
                regions: function
                    .regions
                    .iter()
                    .map(|region| MechanicalRegionEmission {
                        region: region.id,
                        steps: region
                            .nodes
                            .iter()
                            .map(|node| MechanicalStep {
                                node: node.id,
                                source: node.source,
                                op: emit_node_op(&node.kind),
                            })
                            .collect(),
                        exits: region
                            .exits
                            .iter()
                            .map(|exit| MechanicalExit {
                                source: exit.source,
                                kind: emit_exit_kind(&exit.kind),
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect(),
    })
}

fn emit_direct_call(direct_call: &DirectCallSpecializationPlan) -> MechanicalDirectCallEmission {
    MechanicalDirectCallEmission {
        source: direct_call.source,
        target: direct_call.target,
        arg_plan: direct_call.arg_plan.clone(),
        body: direct_call.body.clone(),
        reason: direct_call.reason.clone(),
    }
}

fn emit_constructor_call(
    constructor_call: &ConstructorCallSpecializationPlan,
) -> MechanicalConstructorCallEmission {
    MechanicalConstructorCallEmission {
        source: constructor_call.source,
        target: constructor_call.target,
        owner_type: constructor_call.owner_type.clone(),
        arg_plan: constructor_call.arg_plan.clone(),
        guard: constructor_call.guard.clone(),
        fallback: constructor_call.fallback.clone(),
        body: constructor_call.body.clone(),
        reason: constructor_call.reason.clone(),
    }
}

fn emit_method_call(method_call: &MethodCallSpecializationPlan) -> MechanicalMethodCallEmission {
    MechanicalMethodCallEmission {
        source: method_call.source,
        target: method_call.target,
        method_name: method_call.method_name.clone(),
        owner_type: method_call.owner_type.clone(),
        arg_plan: method_call.arg_plan.clone(),
        guard: method_call.guard.clone(),
        fallback: method_call.fallback.clone(),
        body: method_call.body.clone(),
        reason: method_call.reason.clone(),
    }
}

fn emit_exact_list_item(item: &ExactListItemSpecializationPlan) -> MechanicalExactListItemEmission {
    MechanicalExactListItemEmission {
        source: item.source,
        access: item.access,
        shape: item.shape,
        guard: item.guard.clone(),
        fallback: item.fallback.clone(),
        reason: item.reason.clone(),
    }
}

fn emit_indexed_field(
    indexed_field: &IndexedFieldSpecializationPlan,
) -> MechanicalIndexedFieldEmission {
    MechanicalIndexedFieldEmission {
        source: indexed_field.source,
        access: indexed_field.access,
        owner_type: indexed_field.owner_type.clone(),
        attr_name: indexed_field.attr_name.clone(),
        expected_index: indexed_field.expected_index,
        reason: indexed_field.reason.clone(),
    }
}

fn emit_indexed_global(
    indexed_global: &IndexedGlobalSpecializationPlan,
) -> MechanicalIndexedGlobalEmission {
    MechanicalIndexedGlobalEmission {
        source: indexed_global.source,
        access: indexed_global.access,
        module_name: indexed_global.module_name.clone(),
        name: indexed_global.name.clone(),
        expected_index: indexed_global.expected_index,
        guard: indexed_global.guard.clone(),
        fallback: indexed_global.fallback.clone(),
        reason: indexed_global.reason.clone(),
    }
}

fn emit_node_op(kind: &PlanNodeKind) -> MechanicalStepOp {
    match kind {
        PlanNodeKind::Input { output } => MechanicalStepOp::Input { output: *output },
        PlanNodeKind::Constant { output, constant } => MechanicalStepOp::Constant {
            output: *output,
            constant: constant.clone(),
        },
        PlanNodeKind::Convert(convert) => MechanicalStepOp::Convert {
            kind: convert.kind,
            input: convert.input,
            output: convert.output,
            failure: convert.failure.clone(),
        },
        PlanNodeKind::Guard(guard) => MechanicalStepOp::Guard {
            kind: guard.guard.kind,
            inputs: guard.inputs.clone(),
            failure: guard.failure.clone(),
        },
        PlanNodeKind::Operation(operation) => emit_operation(operation),
        PlanNodeKind::Materialize(materialize) => MechanicalStepOp::Materialize {
            kind: materialize.kind,
            input: materialize.input,
            output: materialize.output,
        },
        PlanNodeKind::Fallback { target } => MechanicalStepOp::Fallback {
            target: target.clone(),
        },
        PlanNodeKind::Deopt { target } => MechanicalStepOp::Deopt { target: *target },
        PlanNodeKind::Ownership { action } => MechanicalStepOp::Ownership {
            action: action.clone(),
        },
    }
}

fn emit_operation(operation: &OperationNode) -> MechanicalStepOp {
    MechanicalStepOp::Operation {
        op: MechanicalOperation::from(&operation.op),
        inputs: operation.inputs.clone(),
        output: operation.output,
        failure: operation.failure.clone(),
    }
}

fn emit_exit_kind(kind: &RegionExitKind) -> MechanicalExitKind {
    match kind {
        RegionExitKind::Branch {
            condition,
            then_target,
            else_target,
        } => MechanicalExitKind::Branch {
            condition: *condition,
            then_target: then_target.clone(),
            else_target: else_target.clone(),
        },
        RegionExitKind::Return { value } => MechanicalExitKind::Return { value: *value },
        RegionExitKind::Jump { target } => MechanicalExitKind::Jump {
            target: target.clone(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_v3::{
        CallBodyKind, CallBodyPlan, ConstructorCallFallbackKind, ConstructorCallFallbackPlan,
        ConstructorCallGuardKind, ConstructorCallGuardPlan, ConstructorCallOwnerType,
        ConstructorCallSpecializationPlan, Cost, DirectCallArgPlan, DirectCallArgSource,
        DirectCallSpecializationPlan, ExactListItemAccessKind, ExactListItemFallbackKind,
        ExactListItemFallbackPlan, ExactListItemGuardKind, ExactListItemGuardPlan,
        ExactListItemShape, ExactListItemSpecializationPlan, FallbackReason, FallbackTarget,
        FunctionOptimizationPlanV3, FunctionOwnershipPlan, FunctionPlanIdentity,
        IndexedFieldAccessKind, IndexedFieldOwnerType, IndexedFieldSpecializationPlan,
        MaterializeNode, MethodCallFallbackKind, MethodCallFallbackPlan, MethodCallGuardKind,
        MethodCallGuardPlan, MethodCallOwnerType, MethodCallSpecializationPlan, ModulePlanIdentity,
        RegionExitPlan, RegionInput, RegionPlan, RegionSource, Rep,
    };
    use soac_core::block_py::{
        BlockLabel, LocalFunctionId, SerializedIdentityTables, SerializedModuleId,
        SerializedModuleIdentity,
    };

    fn inline_call_body() -> CallBodyPlan {
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
            reason: "test inline body".to_string(),
        }
    }

    fn test_plan(include_materialization: bool) -> ModuleOptimizationPlanV3 {
        let lhs = PlanValue::new(0, Rep::I64);
        let rhs = PlanValue::new(1, Rep::I64);
        let sum = PlanValue::new(2, Rep::I64);
        let result = PlanValue::new(3, Rep::PyObjectOwned);
        let mut nodes = vec![
            crate::plan_v3::PlanNode {
                id: PlanNodeId(0),
                source: None,
                kind: PlanNodeKind::Constant {
                    output: lhs,
                    constant: PlannedConstant::I64(1),
                },
            },
            crate::plan_v3::PlanNode {
                id: PlanNodeId(1),
                source: None,
                kind: PlanNodeKind::Constant {
                    output: rhs,
                    constant: PlannedConstant::I64(2),
                },
            },
            crate::plan_v3::PlanNode {
                id: PlanNodeId(2),
                source: None,
                kind: PlanNodeKind::Operation(OperationNode {
                    op: PlannedOp::CheckedI64Add,
                    inputs: vec![lhs, rhs],
                    output: Some(sum),
                    failure_replay: crate::plan_v3::FailureReplayPolicy::local_fallback(
                        "overflow would use a local fallback in a real plan",
                    ),
                    failure: FailureMode::FallbackToPlan {
                        target: FallbackTarget::Region(RegionId(1)),
                        reason: FallbackReason("overflow uses synthetic fallback".to_string()),
                    },
                    cost: Cost::default(),
                }),
            },
        ];
        let return_value = if include_materialization {
            nodes.push(crate::plan_v3::PlanNode {
                id: PlanNodeId(3),
                source: None,
                kind: PlanNodeKind::Materialize(MaterializeNode {
                    input: sum,
                    output: result,
                    kind: MaterializeKind::PythonLong,
                }),
            });
            result
        } else {
            sum
        };
        let fallback_i64 = PlanValue::new(10, Rep::I64);
        let fallback_result = PlanValue::new(11, Rep::PyObjectOwned);

        ModuleOptimizationPlanV3 {
            module: ModulePlanIdentity {
                module_name: "pkg.mod".to_string(),
                source_hash: 0x77,
                cache_identity: "test-cache".to_string(),
            },
            identity_tables: SerializedIdentityTables {
                modules: vec![SerializedModuleIdentity {
                    module_name: "pkg.mod".to_string(),
                    source_hash: 0x77,
                    cache_identity: Some("test-cache".to_string()),
                }],
                debug_names: Vec::new(),
            },
            helper_catalog_version: 1,
            cost_model_version: 1,
            functions: vec![FunctionOptimizationPlanV3 {
                function: FunctionPlanIdentity {
                    function: SerializedFunctionId::new(
                        SerializedModuleId::new(0),
                        LocalFunctionId::new(1),
                    ),
                    debug_name: Some("f".to_string()),
                },
                regions: vec![
                    RegionPlan {
                        id: RegionId(0),
                        source: RegionSource::FunctionEntry,
                        inputs: Vec::<RegionInput>::new(),
                        nodes,
                        exits: vec![RegionExitPlan {
                            source: None,
                            kind: RegionExitKind::Return {
                                value: return_value,
                            },
                        }],
                    },
                    RegionPlan {
                        id: RegionId(1),
                        source: RegionSource::Synthetic {
                            reason: "test overflow fallback".to_string(),
                        },
                        inputs: Vec::<RegionInput>::new(),
                        nodes: vec![
                            crate::plan_v3::PlanNode {
                                id: PlanNodeId(10),
                                source: None,
                                kind: PlanNodeKind::Constant {
                                    output: fallback_i64,
                                    constant: PlannedConstant::I64(0),
                                },
                            },
                            crate::plan_v3::PlanNode {
                                id: PlanNodeId(11),
                                source: None,
                                kind: PlanNodeKind::Materialize(MaterializeNode {
                                    input: fallback_i64,
                                    output: fallback_result,
                                    kind: MaterializeKind::PythonLong,
                                }),
                            },
                        ],
                        exits: vec![RegionExitPlan {
                            source: None,
                            kind: RegionExitKind::Return {
                                value: fallback_result,
                            },
                        }],
                    },
                ],
                scalar_threads: Vec::new(),
                direct_calls: Vec::new(),
                constructor_calls: Vec::new(),
                method_calls: Vec::new(),
                exact_list_items: Vec::new(),
                indexed_fields: Vec::new(),
                indexed_globals: Vec::new(),
                deopt_points: Vec::new(),
                ownership: FunctionOwnershipPlan::default(),
                diagnostics: Vec::new(),
            }],
        }
    }

    #[test]
    fn emits_valid_plan_mechanically() {
        let emission = emit_mechanical_plan_v3(&test_plan(true)).unwrap();

        assert_eq!(emission.module_name, "pkg.mod");
        assert!(emission.functions[0].direct_calls.is_empty());
        let region = &emission.functions[0].regions[0];
        assert_eq!(region.steps.len(), 4);
        assert!(matches!(
            region.steps[2].op,
            MechanicalStepOp::Operation {
                op: MechanicalOperation::CheckedI64Add,
                ..
            }
        ));
        assert!(matches!(
            region.steps[3].op,
            MechanicalStepOp::Materialize {
                kind: MaterializeKind::PythonLong,
                ..
            }
        ));
        assert!(matches!(
            region.exits[0].kind,
            MechanicalExitKind::Return {
                value: PlanValue {
                    rep: Rep::PyObjectOwned,
                    ..
                }
            }
        ));
    }

    #[test]
    fn emits_direct_call_decisions_mechanically() {
        let mut plan = test_plan(true);
        let source = InstrId::new(BlockLabel::from_index(0), 7);
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        let body = inline_call_body();
        plan.functions[0]
            .direct_calls
            .push(DirectCallSpecializationPlan {
                source,
                target,
                arg_plan: DirectCallArgPlan {
                    sources: vec![DirectCallArgSource::Provided(0)],
                },
                body: body.clone(),
                reason: "profiled call_hot_targets selected this same-module function".to_string(),
            });

        let emission = emit_mechanical_plan_v3(&plan).unwrap();

        assert_eq!(
            emission.functions[0].direct_calls,
            vec![MechanicalDirectCallEmission {
                source,
                target,
                arg_plan: DirectCallArgPlan {
                    sources: vec![DirectCallArgSource::Provided(0)],
                },
                body,
                reason: "profiled call_hot_targets selected this same-module function".to_string(),
            }]
        );
    }

    #[test]
    fn emits_constructor_call_decisions_mechanically() {
        let mut plan = test_plan(true);
        let source = InstrId::new(BlockLabel::from_index(0), 7);
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        let owner_type = ConstructorCallOwnerType {
            module_name: "pkg.mod".to_string(),
            qualname: "Box".to_string(),
        };
        let guard = ConstructorCallGuardPlan {
            kind: ConstructorCallGuardKind::ExactCallableTypeVersion,
        };
        let fallback = ConstructorCallFallbackPlan {
            kind: ConstructorCallFallbackKind::OriginalConstructorCall,
        };
        let inline_target =
            SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(3));
        let body = CallBodyPlan {
            inline_target: Some(inline_target),
            ..inline_call_body()
        };
        plan.functions[0]
            .constructor_calls
            .push(ConstructorCallSpecializationPlan {
                source,
                target,
                owner_type: owner_type.clone(),
                arg_plan: DirectCallArgPlan {
                    sources: vec![
                        DirectCallArgSource::Provided(0),
                        DirectCallArgSource::Provided(1),
                    ],
                },
                guard: guard.clone(),
                fallback: fallback.clone(),
                body: body.clone(),
                reason: "profiled constructor target".to_string(),
            });

        let emission = emit_mechanical_plan_v3(&plan).unwrap();

        assert_eq!(
            emission.functions[0].constructor_calls,
            vec![MechanicalConstructorCallEmission {
                source,
                target,
                owner_type,
                arg_plan: DirectCallArgPlan {
                    sources: vec![
                        DirectCallArgSource::Provided(0),
                        DirectCallArgSource::Provided(1),
                    ],
                },
                guard,
                fallback,
                body,
                reason: "profiled constructor target".to_string(),
            }]
        );
    }

    #[test]
    fn emits_method_call_decisions_mechanically() {
        let mut plan = test_plan(true);
        let source = InstrId::new(BlockLabel::from_index(0), 7);
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        let owner_type = MethodCallOwnerType {
            module_name: "pkg.mod".to_string(),
            qualname: "Box".to_string(),
        };
        let guard = MethodCallGuardPlan {
            kind: MethodCallGuardKind::ExactReceiverTypeVersion,
        };
        let fallback = MethodCallFallbackPlan {
            kind: MethodCallFallbackKind::OriginalMethodCall,
        };
        let body = inline_call_body();
        plan.functions[0]
            .method_calls
            .push(MethodCallSpecializationPlan {
                source,
                target,
                method_name: "get".to_string(),
                owner_type: owner_type.clone(),
                arg_plan: DirectCallArgPlan {
                    sources: vec![DirectCallArgSource::Provided(0)],
                },
                guard: guard.clone(),
                fallback: fallback.clone(),
                body: body.clone(),
                reason: "profiled call_hot_targets selected this owner method".to_string(),
            });

        let emission = emit_mechanical_plan_v3(&plan).unwrap();

        assert_eq!(
            emission.functions[0].method_calls,
            vec![MechanicalMethodCallEmission {
                source,
                target,
                method_name: "get".to_string(),
                owner_type,
                arg_plan: DirectCallArgPlan {
                    sources: vec![DirectCallArgSource::Provided(0)],
                },
                guard,
                fallback,
                body,
                reason: "profiled call_hot_targets selected this owner method".to_string(),
            }]
        );
    }

    #[test]
    fn emits_exact_list_item_decisions_mechanically() {
        let mut plan = test_plan(true);
        let get_source = InstrId::new(BlockLabel::from_index(0), 7);
        let set_source = InstrId::new(BlockLabel::from_index(0), 9);
        let guard = ExactListItemGuardPlan {
            kind: ExactListItemGuardKind::ExactListExactCompactIntInBounds,
        };
        let fallback = ExactListItemFallbackPlan {
            kind: ExactListItemFallbackKind::OriginalItemAccess,
        };
        plan.functions[0]
            .exact_list_items
            .push(ExactListItemSpecializationPlan {
                source: get_source,
                access: ExactListItemAccessKind::Get,
                shape: ExactListItemShape::ExactListExactInt,
                guard: guard.clone(),
                fallback: fallback.clone(),
                reason: "profiled getitem_hot_shapes selected exact-list/exact-int".to_string(),
            });
        plan.functions[0]
            .exact_list_items
            .push(ExactListItemSpecializationPlan {
                source: set_source,
                access: ExactListItemAccessKind::Set,
                shape: ExactListItemShape::ExactListExactInt,
                guard: guard.clone(),
                fallback: fallback.clone(),
                reason: "profiled setitem_hot_shapes selected exact-list/exact-int".to_string(),
            });

        let emission = emit_mechanical_plan_v3(&plan).unwrap();

        assert_eq!(
            emission.functions[0].exact_list_items,
            vec![
                MechanicalExactListItemEmission {
                    source: get_source,
                    access: ExactListItemAccessKind::Get,
                    shape: ExactListItemShape::ExactListExactInt,
                    guard: guard.clone(),
                    fallback: fallback.clone(),
                    reason: "profiled getitem_hot_shapes selected exact-list/exact-int".to_string(),
                },
                MechanicalExactListItemEmission {
                    source: set_source,
                    access: ExactListItemAccessKind::Set,
                    shape: ExactListItemShape::ExactListExactInt,
                    guard,
                    fallback,
                    reason: "profiled setitem_hot_shapes selected exact-list/exact-int".to_string(),
                },
            ]
        );
    }

    #[test]
    fn emits_indexed_field_decisions_mechanically() {
        let mut plan = test_plan(true);
        let load_source = InstrId::new(BlockLabel::from_index(0), 7);
        let store_source = InstrId::new(BlockLabel::from_index(0), 9);
        let owner_type = IndexedFieldOwnerType {
            module_name: "pkg.model".to_string(),
            qualname: "Record".to_string(),
        };
        plan.functions[0]
            .indexed_fields
            .push(IndexedFieldSpecializationPlan {
                source: load_source,
                access: IndexedFieldAccessKind::Load,
                owner_type: owner_type.clone(),
                attr_name: "value".to_string(),
                expected_index: 2,
                reason: "profiled type_keys selected this indexed-field layout".to_string(),
            });
        plan.functions[0]
            .indexed_fields
            .push(IndexedFieldSpecializationPlan {
                source: store_source,
                access: IndexedFieldAccessKind::Store,
                owner_type: owner_type.clone(),
                attr_name: "value".to_string(),
                expected_index: 2,
                reason: "profiled type_keys selected this indexed-field layout".to_string(),
            });

        let emission = emit_mechanical_plan_v3(&plan).unwrap();

        assert_eq!(
            emission.functions[0].indexed_fields,
            vec![
                MechanicalIndexedFieldEmission {
                    source: load_source,
                    access: IndexedFieldAccessKind::Load,
                    owner_type: owner_type.clone(),
                    attr_name: "value".to_string(),
                    expected_index: 2,
                    reason: "profiled type_keys selected this indexed-field layout".to_string(),
                },
                MechanicalIndexedFieldEmission {
                    source: store_source,
                    access: IndexedFieldAccessKind::Store,
                    owner_type,
                    attr_name: "value".to_string(),
                    expected_index: 2,
                    reason: "profiled type_keys selected this indexed-field layout".to_string(),
                },
            ]
        );
    }

    #[test]
    fn emits_indexed_global_decisions_mechanically() {
        let mut plan = test_plan(true);
        let load_source = InstrId::new(BlockLabel::from_index(0), 7);
        let store_source = InstrId::new(BlockLabel::from_index(0), 9);
        plan.functions[0]
            .indexed_globals
            .push(IndexedGlobalSpecializationPlan {
                source: load_source,
                access: IndexedGlobalAccessKind::Load,
                module_name: "pkg.mod".to_string(),
                name: "value".to_string(),
                expected_index: 2,
                guard: IndexedGlobalGuardPlan {
                    kind: crate::plan_v3::IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
                },
                fallback: IndexedGlobalFallbackPlan {
                    kind: crate::plan_v3::IndexedGlobalFallbackKind::OriginalGlobalAccess,
                },
                reason: "profiled module_keys selected this indexed-global slot".to_string(),
            });
        plan.functions[0]
            .indexed_globals
            .push(IndexedGlobalSpecializationPlan {
                source: store_source,
                access: IndexedGlobalAccessKind::Store,
                module_name: "pkg.mod".to_string(),
                name: "value".to_string(),
                expected_index: 2,
                guard: IndexedGlobalGuardPlan {
                    kind: crate::plan_v3::IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
                },
                fallback: IndexedGlobalFallbackPlan {
                    kind: crate::plan_v3::IndexedGlobalFallbackKind::OriginalGlobalAccess,
                },
                reason: "profiled module_keys selected this indexed-global slot".to_string(),
            });

        let emission = emit_mechanical_plan_v3(&plan).unwrap();

        assert_eq!(
            emission.functions[0].indexed_globals,
            vec![
                MechanicalIndexedGlobalEmission {
                    source: load_source,
                    access: IndexedGlobalAccessKind::Load,
                    module_name: "pkg.mod".to_string(),
                    name: "value".to_string(),
                    expected_index: 2,
                    guard: IndexedGlobalGuardPlan {
                        kind: crate::plan_v3::IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
                    },
                    fallback: IndexedGlobalFallbackPlan {
                        kind: crate::plan_v3::IndexedGlobalFallbackKind::OriginalGlobalAccess,
                    },
                    reason: "profiled module_keys selected this indexed-global slot".to_string(),
                },
                MechanicalIndexedGlobalEmission {
                    source: store_source,
                    access: IndexedGlobalAccessKind::Store,
                    module_name: "pkg.mod".to_string(),
                    name: "value".to_string(),
                    expected_index: 2,
                    guard: IndexedGlobalGuardPlan {
                        kind: crate::plan_v3::IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
                    },
                    fallback: IndexedGlobalFallbackPlan {
                        kind: crate::plan_v3::IndexedGlobalFallbackKind::OriginalGlobalAccess,
                    },
                    reason: "profiled module_keys selected this indexed-global slot".to_string(),
                },
            ]
        );
    }

    #[test]
    fn refuses_invalid_plan_before_emitting() {
        let err = emit_mechanical_plan_v3(&test_plan(false)).unwrap_err();
        match err {
            MechanicalEmitError::InvalidPlan(validation) => {
                assert!(validation.contains("return exits require a returnable PyObject"));
            }
        }
    }
}
