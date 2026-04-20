use crate::optimization_plan_v3::{
    ConversionKind, DeoptPointId, FailureMode, GuardFailure, GuardKind, MaterializeKind,
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
    pub regions: Vec<MechanicalRegionEmission>,
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
        target: crate::optimization_plan_v3::FallbackTarget,
    },
    Deopt {
        target: DeoptPointId,
    },
    Ownership {
        action: crate::optimization_plan_v3::OwnershipAction,
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
    use crate::optimization_plan_v3::{
        Cost, FunctionOptimizationPlanV3, FunctionOwnershipPlan, FunctionPlanIdentity,
        MaterializeNode, ModulePlanIdentity, RegionExitPlan, RegionInput, RegionPlan, RegionSource,
        Rep,
    };
    use soac_core::block_py::{LocalFunctionId, SerializedModuleId};

    fn test_plan(include_materialization: bool) -> ModuleOptimizationPlanV3 {
        let lhs = PlanValue::new(0, Rep::I64);
        let rhs = PlanValue::new(1, Rep::I64);
        let sum = PlanValue::new(2, Rep::I64);
        let result = PlanValue::new(3, Rep::PyObjectOwned);
        let mut nodes = vec![
            crate::optimization_plan_v3::PlanNode {
                id: PlanNodeId(0),
                source: None,
                kind: PlanNodeKind::Constant {
                    output: lhs,
                    constant: PlannedConstant::I64(1),
                },
            },
            crate::optimization_plan_v3::PlanNode {
                id: PlanNodeId(1),
                source: None,
                kind: PlanNodeKind::Constant {
                    output: rhs,
                    constant: PlannedConstant::I64(2),
                },
            },
            crate::optimization_plan_v3::PlanNode {
                id: PlanNodeId(2),
                source: None,
                kind: PlanNodeKind::Operation(OperationNode {
                    op: PlannedOp::CheckedI64Add,
                    inputs: vec![lhs, rhs],
                    output: Some(sum),
                    failure_replay:
                        crate::optimization_plan_v3::FailureReplayPolicy::local_fallback(
                            "overflow would use a local fallback in a real plan",
                        ),
                    failure: FailureMode::CannotFail,
                    cost: Cost::default(),
                }),
            },
        ];
        let return_value = if include_materialization {
            nodes.push(crate::optimization_plan_v3::PlanNode {
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

        ModuleOptimizationPlanV3 {
            module: ModulePlanIdentity {
                module_name: "pkg.mod".to_string(),
                source_hash: 0x77,
                cache_identity: "test-cache".to_string(),
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
                regions: vec![RegionPlan {
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
                }],
                scalar_threads: Vec::new(),
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
    fn refuses_invalid_plan_before_emitting() {
        let err = emit_mechanical_plan_v3(&test_plan(false)).unwrap_err();
        match err {
            MechanicalEmitError::InvalidPlan(validation) => {
                assert!(validation.contains("return exits require a returnable PyObject"));
            }
        }
    }
}
