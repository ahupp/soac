use crate::plan_v3::{
    CallBodyPlan, ConstructorCallFallbackPlan, ConstructorCallGuardPlan, ConstructorCallOwnerType,
    ConstructorCallSpecializationPlan, ConversionKind, DeoptPointId, DirectCallArgPlan,
    DirectCallSpecializationPlan, ExactListItemAccessKind, ExactListItemFallbackPlan,
    ExactListItemGuardPlan, ExactListItemShape, ExactListItemSpecializationPlan, FailureMode,
    GuardFailure, GuardKind, IndexedFieldAccessKind, IndexedFieldFallbackPlan,
    IndexedFieldGuardKind, IndexedFieldOwnerType, IndexedFieldSpecializationPlan,
    IndexedGlobalAccessKind, IndexedGlobalFallbackPlan, IndexedGlobalGuardPlan,
    IndexedGlobalSpecializationPlan, MaterializeKind, MethodCallFallbackPlan, MethodCallGuardPlan,
    MethodCallOwnerType, MethodCallSpecializationPlan, ModuleOptimizationPlanV3, OperationNode,
    PlanNodeId, PlanNodeKind, PlanValidationError, PlanValue, PlannedConstant, PlannedOp,
    RegionExitKind, RegionExitTarget, RegionId, RegionInputSource, RegionPlan, Rep, RichCompareOp,
    ScalarLocalThreadPlan, validate_module_plan_v3,
};
use soac_core::block_py::{InstrId, SerializedFunctionId};
use std::collections::{HashMap, HashSet};
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
    pub scalar_threads: Vec<ScalarLocalThreadPlan>,
    pub regions: Vec<MechanicalRegionEmission>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MechanicalRegionFunctionParamInput<'a> {
    pub value: PlanValue,
    pub name: &'a str,
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
    pub guard: MechanicalIndexedFieldGuard,
    pub fallback: IndexedFieldFallbackPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MechanicalIndexedFieldGuard {
    pub kind: IndexedFieldGuardKind,
    pub owner_type: IndexedFieldOwnerType,
    pub attr_name: String,
    pub expected_index: u32,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MechanicalCodegenStep {
    Input {
        output: PlanValue,
    },
    ConstantI64 {
        output: PlanValue,
        value: i64,
    },
    SpecializationGuard {
        inputs: Vec<PlanValue>,
    },
    Convert {
        kind: MechanicalCodegenConversion,
        input: PlanValue,
        output: PlanValue,
    },
    PreseededConvert {
        output: PlanValue,
    },
    Operation {
        op: MechanicalCodegenOperation,
        inputs: [PlanValue; 2],
        output: PlanValue,
    },
    Materialize {
        kind: MaterializeKind,
        input: PlanValue,
        output: PlanValue,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MechanicalCodegenConversion {
    FromPythonLongCompactToI64,
    TruthinessToI32Bool01,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MechanicalCodegenOperation {
    PyNumberAdd,
    PyNumberSubtract,
    PyNumberMultiply,
    PyNumberBitAnd,
    PyNumberBitOr,
    PyNumberBitXor,
    PyObjectRichCompare { op: RichCompareOp },
    CheckedI64Add,
    CheckedI64Sub,
    CheckedI64Mul,
    I64BitAnd,
    I64BitOr,
    I64BitXor,
    I64CompareToBool01 { op: RichCompareOp },
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
    EmissionMismatch(String),
}

impl fmt::Display for MechanicalEmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPlan(err) => write!(f, "invalid optimization plan v3: {err}"),
            Self::EmissionMismatch(message) => {
                write!(f, "optimization plan v3 emission mismatch: {message}")
            }
        }
    }
}

impl std::error::Error for MechanicalEmitError {}

pub fn emit_mechanical_plan_v3(
    plan: &ModuleOptimizationPlanV3,
) -> Result<MechanicalModuleEmission, MechanicalEmitError> {
    validate_module_plan_v3(plan).map_err(MechanicalEmitError::InvalidPlan)?;
    let emission = expected_mechanical_emission_for_plan_v3(plan);
    validate_mechanical_emission_matches_plan_v3(plan, &emission)?;
    Ok(emission)
}

pub fn validate_mechanical_emission_matches_plan_v3(
    plan: &ModuleOptimizationPlanV3,
    emission: &MechanicalModuleEmission,
) -> Result<(), MechanicalEmitError> {
    validate_module_plan_v3(plan).map_err(MechanicalEmitError::InvalidPlan)?;
    if emission.module_name != plan.module.module_name {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "emitted module name is {}, expected {}",
            emission.module_name, plan.module.module_name
        )));
    }
    validate_emitted_list(
        "functions",
        "<module>",
        &expected_mechanical_emission_for_plan_v3(plan).functions,
        &emission.functions,
    )?;
    validate_current_mechanical_lowering_shape_v3(plan, emission)
        .map_err(MechanicalEmitError::EmissionMismatch)
}

pub fn mechanical_region_function_param_inputs<'a>(
    region: &'a RegionPlan,
    context: &str,
) -> Result<Vec<MechanicalRegionFunctionParamInput<'a>>, String> {
    let mut inputs = Vec::with_capacity(region.inputs.len());
    for input in &region.inputs {
        let RegionInputSource::FunctionParam {
            name: Some(name), ..
        } = &input.source
        else {
            return Err(format!(
                "prevalidated optimizer v3 {context} input {:?} has non-mechanical source {:?}",
                input.value, input.source
            ));
        };
        if input.value.rep != Rep::PyObjectBorrowed {
            return Err(format!(
                "prevalidated optimizer v3 {context} input {:?} has non-mechanical rep {:?}",
                input.value, input.value.rep
            ));
        }
        inputs.push(MechanicalRegionFunctionParamInput {
            value: input.value,
            name,
        });
    }
    Ok(inputs)
}

pub fn mechanical_codegen_step(
    region: RegionId,
    step: &MechanicalStep,
    has_local_fallback: bool,
    preseeded_scalar: Option<PlanValue>,
    preseeded_convert_inputs: &HashSet<PlanValue>,
) -> Result<MechanicalCodegenStep, String> {
    match &step.op {
        MechanicalStepOp::Input { output } => Ok(MechanicalCodegenStep::Input { output: *output }),
        MechanicalStepOp::Constant { output, constant } => match constant {
            PlannedConstant::I64(value) if output.rep == Rep::I64 => {
                Ok(MechanicalCodegenStep::ConstantI64 {
                    output: *output,
                    value: *value,
                })
            }
            PlannedConstant::I64(_) => Err(format!(
                "prevalidated optimizer v3 region {region:?} constant node {:?} produces {:?}; current mechanical lowering only emits i64 constants",
                step.node, output.rep
            )),
            other => Err(format!(
                "prevalidated optimizer v3 region {region:?} constant node {:?} has non-i64 constant {other:?}",
                step.node
            )),
        },
        MechanicalStepOp::Guard {
            kind,
            inputs,
            failure,
        } => {
            if *kind != GuardKind::SpecializationCheck {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} guard node {:?} has non-mechanical kind {kind:?}",
                    step.node
                ));
            }
            if inputs.is_empty() {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} guard node {:?} has no inputs",
                    step.node
                ));
            }
            let preseeded_guard = !preseeded_convert_inputs.is_empty()
                && inputs
                    .iter()
                    .all(|input| preseeded_convert_inputs.contains(input));
            if !has_local_fallback && !preseeded_guard {
                return Err(format!(
                    "optimizer v3 region {region:?} guard node {:?} appears outside a local-fallback hot region",
                    step.node
                ));
            }
            if !matches!(
                failure,
                GuardFailure::FallbackToPlan {
                    target: crate::plan_v3::FallbackTarget::Region(_),
                    ..
                }
            ) {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} guard node {:?} has non-mechanical failure {failure:?}",
                    step.node
                ));
            }
            Ok(MechanicalCodegenStep::SpecializationGuard {
                inputs: inputs.clone(),
            })
        }
        MechanicalStepOp::Convert {
            kind,
            input,
            output,
            failure,
        } => {
            if preseeded_scalar == Some(*output) {
                return Ok(MechanicalCodegenStep::PreseededConvert { output: *output });
            }
            let kind = mechanical_codegen_conversion(
                region,
                step.node,
                *kind,
                *input,
                *output,
                failure,
                has_local_fallback,
            )?;
            Ok(MechanicalCodegenStep::Convert {
                kind,
                input: *input,
                output: *output,
            })
        }
        MechanicalStepOp::Operation {
            op,
            inputs,
            output,
            failure,
        } => {
            let (op, inputs, output) = mechanical_codegen_operation(
                region,
                step.node,
                op,
                inputs,
                *output,
                failure,
                has_local_fallback,
            )?;
            Ok(MechanicalCodegenStep::Operation { op, inputs, output })
        }
        MechanicalStepOp::Materialize {
            kind,
            input,
            output,
        } => {
            let expected = match kind {
                MaterializeKind::PythonLong => (Rep::I64, Rep::PyObjectOwned),
                MaterializeKind::PythonBool => (Rep::I32Bool01, Rep::PyObjectImmortal),
            };
            if input.rep != expected.0 || output.rep != expected.1 {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} materialize node {:?} {kind:?} expects {:?}->{:?}, got {:?}->{:?}",
                    step.node, expected.0, expected.1, input.rep, output.rep
                ));
            }
            Ok(MechanicalCodegenStep::Materialize {
                kind: *kind,
                input: *input,
                output: *output,
            })
        }
        MechanicalStepOp::Fallback { .. }
        | MechanicalStepOp::Deopt { .. }
        | MechanicalStepOp::Ownership { .. } => Err(format!(
            "prevalidated optimizer v3 region {region:?} node {:?} contains a non-emittable codegen step {:?}",
            step.node, step.op
        )),
    }
}

pub fn mechanical_convert_inputs_for_output(
    region: &MechanicalRegionEmission,
    output: PlanValue,
) -> HashSet<PlanValue> {
    region
        .steps
        .iter()
        .filter_map(|step| match &step.op {
            MechanicalStepOp::Convert {
                input,
                output: step_output,
                ..
            } if *step_output == output => Some(*input),
            _ => None,
        })
        .collect()
}

fn mechanical_codegen_conversion(
    region: RegionId,
    node: PlanNodeId,
    kind: ConversionKind,
    input: PlanValue,
    output: PlanValue,
    failure: &FailureMode,
    has_local_fallback: bool,
) -> Result<MechanicalCodegenConversion, String> {
    match kind {
        ConversionKind::FromPythonLongCompactToI64 => {
            if input.rep != Rep::PyObjectBorrowed || output.rep != Rep::I64 {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} conversion node {node:?} expects PyObjectBorrowed->I64, got {:?}->{:?}",
                    input.rep, output.rep
                ));
            }
            if !has_local_fallback
                || !matches!(
                    failure,
                    FailureMode::FallbackToPlan {
                        target: crate::plan_v3::FallbackTarget::Region(_),
                        ..
                    }
                )
            {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} conversion node {node:?} needs a region local fallback for compact-long guard misses"
                ));
            }
            Ok(MechanicalCodegenConversion::FromPythonLongCompactToI64)
        }
        ConversionKind::TruthinessToI32Bool01 => {
            if input.rep != Rep::PyObjectOwned || output.rep != Rep::I32Bool01 {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} conversion node {node:?} expects PyObjectOwned->I32Bool01, got {:?}->{:?}",
                    input.rep, output.rep
                ));
            }
            if !matches!(failure, FailureMode::Raise(_)) {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} conversion node {node:?} expects Python truthiness failure to raise locally, got {failure:?}"
                ));
            }
            Ok(MechanicalCodegenConversion::TruthinessToI32Bool01)
        }
        other => Err(format!(
            "prevalidated optimizer v3 region {region:?} conversion node {node:?} contains conversion outside the validated emitter capability set {other:?}"
        )),
    }
}

fn mechanical_codegen_operation(
    region: RegionId,
    node: PlanNodeId,
    operation: &MechanicalOperation,
    inputs: &[PlanValue],
    output: Option<PlanValue>,
    failure: &FailureMode,
    has_local_fallback: bool,
) -> Result<(MechanicalCodegenOperation, [PlanValue; 2], PlanValue), String> {
    let (inputs, output) = binary_operation_parts(region, node, operation, inputs, output)?;
    match operation {
        MechanicalOperation::PyNumberAdd
        | MechanicalOperation::PyNumberSubtract
        | MechanicalOperation::PyNumberMultiply
        | MechanicalOperation::PyNumberBitAnd
        | MechanicalOperation::PyNumberBitOr
        | MechanicalOperation::PyNumberBitXor
        | MechanicalOperation::PyObjectRichCompare { .. } => {
            if !inputs.iter().all(|input| input.rep.is_python_object())
                || output.rep != Rep::PyObjectOwned
            {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} operation node {node:?} {operation:?} expects PyObject inputs and PyObjectOwned output, got {:?}, {:?}",
                    inputs.map(|input| input.rep),
                    output.rep
                ));
            }
            if !matches!(failure, FailureMode::Raise(_)) {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} operation node {node:?} {operation:?} expects local Python raise failure, got {failure:?}"
                ));
            }
            let op = match operation {
                MechanicalOperation::PyNumberAdd => MechanicalCodegenOperation::PyNumberAdd,
                MechanicalOperation::PyNumberSubtract => {
                    MechanicalCodegenOperation::PyNumberSubtract
                }
                MechanicalOperation::PyNumberMultiply => {
                    MechanicalCodegenOperation::PyNumberMultiply
                }
                MechanicalOperation::PyNumberBitAnd => MechanicalCodegenOperation::PyNumberBitAnd,
                MechanicalOperation::PyNumberBitOr => MechanicalCodegenOperation::PyNumberBitOr,
                MechanicalOperation::PyNumberBitXor => MechanicalCodegenOperation::PyNumberBitXor,
                MechanicalOperation::PyObjectRichCompare { op } => {
                    MechanicalCodegenOperation::PyObjectRichCompare { op: *op }
                }
                _ => unreachable!("matched Python object operation"),
            };
            Ok((op, inputs, output))
        }
        MechanicalOperation::CheckedI64Add
        | MechanicalOperation::CheckedI64Sub
        | MechanicalOperation::CheckedI64Mul => {
            require_binary_signature(
                region,
                node,
                operation,
                inputs,
                output,
                [Rep::I64, Rep::I64],
                Rep::I64,
            )?;
            if !has_local_fallback
                || !matches!(
                    failure,
                    FailureMode::FallbackToPlan {
                        target: crate::plan_v3::FallbackTarget::Region(_),
                        ..
                    }
                )
            {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} operation node {node:?} {operation:?} needs a region local fallback for overflow"
                ));
            }
            let op = match operation {
                MechanicalOperation::CheckedI64Add => MechanicalCodegenOperation::CheckedI64Add,
                MechanicalOperation::CheckedI64Sub => MechanicalCodegenOperation::CheckedI64Sub,
                MechanicalOperation::CheckedI64Mul => MechanicalCodegenOperation::CheckedI64Mul,
                _ => unreachable!("matched checked i64 operation"),
            };
            Ok((op, inputs, output))
        }
        MechanicalOperation::I64BitAnd
        | MechanicalOperation::I64BitOr
        | MechanicalOperation::I64BitXor => {
            require_binary_signature(
                region,
                node,
                operation,
                inputs,
                output,
                [Rep::I64, Rep::I64],
                Rep::I64,
            )?;
            if failure != &FailureMode::CannotFail {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} operation node {node:?} {operation:?} must be CannotFail, got {failure:?}"
                ));
            }
            let op = match operation {
                MechanicalOperation::I64BitAnd => MechanicalCodegenOperation::I64BitAnd,
                MechanicalOperation::I64BitOr => MechanicalCodegenOperation::I64BitOr,
                MechanicalOperation::I64BitXor => MechanicalCodegenOperation::I64BitXor,
                _ => unreachable!("matched i64 bitwise operation"),
            };
            Ok((op, inputs, output))
        }
        MechanicalOperation::I64CompareToBool01 { op: compare_op } => {
            require_binary_signature(
                region,
                node,
                operation,
                inputs,
                output,
                [Rep::I64, Rep::I64],
                Rep::I32Bool01,
            )?;
            if failure != &FailureMode::CannotFail {
                return Err(format!(
                    "prevalidated optimizer v3 region {region:?} operation node {node:?} I64CompareToBool01 must be CannotFail, got {failure:?}"
                ));
            }
            Ok((
                MechanicalCodegenOperation::I64CompareToBool01 { op: *compare_op },
                inputs,
                output,
            ))
        }
        other => Err(format!(
            "prevalidated optimizer v3 region {region:?} node {node:?} contains operation outside the validated emitter capability set {other:?}"
        )),
    }
}

fn binary_operation_parts(
    region: RegionId,
    node: PlanNodeId,
    op: &MechanicalOperation,
    inputs: &[PlanValue],
    output: Option<PlanValue>,
) -> Result<([PlanValue; 2], PlanValue), String> {
    let [lhs, rhs] = inputs else {
        return Err(format!(
            "prevalidated optimizer v3 region {region:?} operation node {node:?} {op:?} expects two inputs, got {}",
            inputs.len()
        ));
    };
    let output = output.ok_or_else(|| {
        format!("prevalidated optimizer v3 region {region:?} node {node:?} {op:?} has no output")
    })?;
    Ok(([*lhs, *rhs], output))
}

fn require_binary_signature(
    region: RegionId,
    node: PlanNodeId,
    op: &MechanicalOperation,
    inputs: [PlanValue; 2],
    output: PlanValue,
    expected_inputs: [Rep; 2],
    expected_output: Rep,
) -> Result<(), String> {
    for (index, (input, expected)) in inputs.iter().zip(expected_inputs.iter()).enumerate() {
        if input.rep != *expected {
            return Err(format!(
                "prevalidated optimizer v3 region {region:?} operation node {node:?} {op:?} input {index} expects {expected:?}, got {:?}",
                input.rep
            ));
        }
    }
    if output.rep != expected_output {
        return Err(format!(
            "prevalidated optimizer v3 region {region:?} operation node {node:?} {op:?} output expects {expected_output:?}, got {:?}",
            output.rep
        ));
    }
    Ok(())
}

fn expected_mechanical_emission_for_plan_v3(
    plan: &ModuleOptimizationPlanV3,
) -> MechanicalModuleEmission {
    MechanicalModuleEmission {
        module_name: plan.module.module_name.clone(),
        functions: plan.functions.iter().map(emit_function).collect(),
    }
}

fn emit_function(
    function: &crate::plan_v3::FunctionOptimizationPlanV3,
) -> MechanicalFunctionEmission {
    MechanicalFunctionEmission {
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
        scalar_threads: function.scalar_threads.clone(),
        regions: function.regions.iter().map(emit_region).collect(),
    }
}

fn emit_region(region: &crate::plan_v3::RegionPlan) -> MechanicalRegionEmission {
    MechanicalRegionEmission {
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
    }
}

fn validate_emitted_list<T: PartialEq>(
    family: &str,
    owner: &str,
    expected: &[T],
    emitted: &[T],
) -> Result<(), MechanicalEmitError> {
    if expected.len() != emitted.len() {
        return Err(MechanicalEmitError::EmissionMismatch(format!(
            "{owner} contains {} expected {family} but {} emitted {family}",
            expected.len(),
            emitted.len()
        )));
    }
    for (index, (expected, emitted)) in expected.iter().zip(emitted.iter()).enumerate() {
        if expected != emitted {
            return Err(MechanicalEmitError::EmissionMismatch(format!(
                "{owner} emitted {family} #{index} does not match the selected plan"
            )));
        }
    }
    Ok(())
}

fn validate_current_mechanical_lowering_shape_v3(
    plan: &ModuleOptimizationPlanV3,
    emission: &MechanicalModuleEmission,
) -> Result<(), String> {
    let planned_functions = plan
        .functions
        .iter()
        .map(|function| (function.function.function, function))
        .collect::<HashMap<_, _>>();
    for emitted_function in &emission.functions {
        let planned_function = planned_functions
            .get(&emitted_function.function)
            .ok_or_else(|| {
                format!(
                    "emitted function {} has no matching selected plan",
                    emitted_function.function
                )
            })?;
        validate_function_mechanical_lowering_shape_v3(planned_function, emitted_function)?;
    }
    Ok(())
}

fn validate_function_mechanical_lowering_shape_v3(
    planned_function: &crate::plan_v3::FunctionOptimizationPlanV3,
    emitted_function: &MechanicalFunctionEmission,
) -> Result<(), String> {
    let planned_regions = planned_function
        .regions
        .iter()
        .map(|region| (region.id, region))
        .collect::<HashMap<_, _>>();
    let emitted_regions = emitted_function
        .regions
        .iter()
        .map(|region| (region.region, region))
        .collect::<HashMap<_, _>>();
    for emitted_region in &emitted_function.regions {
        let planned_region = planned_regions.get(&emitted_region.region).ok_or_else(|| {
            format!(
                "emitted function {} has region {:?} with no matching selected plan",
                emitted_function.function, emitted_region.region
            )
        })?;
        validate_region_inputs_supported_by_current_lowering_v3(
            emitted_function.function,
            planned_region,
        )?;
        validate_region_mechanical_lowering_shape_v3(
            emitted_function.function,
            emitted_region,
            &emitted_regions,
        )?;
        validate_region_steps_supported_by_current_lowering_v3(
            emitted_function.function,
            emitted_region,
        )?;
    }
    for thread in &planned_function.scalar_threads {
        validate_scalar_thread_mechanical_lowering_shape_v3(
            emitted_function.function,
            thread,
            &emitted_regions,
        )?;
    }
    Ok(())
}

fn validate_region_inputs_supported_by_current_lowering_v3(
    function: SerializedFunctionId,
    region: &RegionPlan,
) -> Result<(), String> {
    for input in &region.inputs {
        match &input.source {
            RegionInputSource::FunctionParam { name: Some(_), .. }
                if input.value.rep == Rep::PyObjectBorrowed => {}
            RegionInputSource::FunctionParam { name: Some(_), .. } => {
                return Err(format!(
                    "function {function} region {:?} input {:?} has rep {:?}; current mechanical lowering loads named inputs as borrowed PyObjects",
                    region.id, input.value.id, input.value.rep
                ));
            }
            source => {
                return Err(format!(
                    "function {function} region {:?} input {:?} has unsupported source {source:?}; current mechanical lowering only supports named function-param inputs",
                    region.id, input.value.id
                ));
            }
        }
    }
    Ok(())
}

fn validate_region_mechanical_lowering_shape_v3(
    function: SerializedFunctionId,
    region: &MechanicalRegionEmission,
    emitted_regions: &HashMap<RegionId, &MechanicalRegionEmission>,
) -> Result<(), String> {
    let Some(exit) = region.exits.first() else {
        return Ok(());
    };
    match &exit.kind {
        MechanicalExitKind::Branch { .. } => {
            require_single_original_cfg_branch_exit_v3(function, region)?;
            validate_region_local_fallback_shape_v3(function, region, emitted_regions)?;
        }
        MechanicalExitKind::Return { .. } => {
            require_single_return_exit_v3(function, region)?;
            validate_region_local_fallback_shape_v3(function, region, emitted_regions)?;
        }
        MechanicalExitKind::Jump { .. } => {}
    }
    Ok(())
}

fn validate_region_local_fallback_shape_v3(
    function: SerializedFunctionId,
    region: &MechanicalRegionEmission,
    emitted_regions: &HashMap<RegionId, &MechanicalRegionEmission>,
) -> Result<(), String> {
    let targets = local_fallback_region_targets_v3(region)?;
    if targets.is_empty() {
        return Ok(());
    }
    if targets.len() != 1 {
        return Err(format!(
            "function {function} region {:?} has {} local fallback targets; current mechanical lowering expects at most one",
            region.region,
            targets.len()
        ));
    }
    let fallback_id = *targets.iter().next().expect("checked one fallback target");
    let fallback_region = emitted_regions.get(&fallback_id).ok_or_else(|| {
        format!(
            "function {function} region {:?} references missing local fallback region {:?}",
            region.region, fallback_id
        )
    })?;
    require_matching_fallback_exit_v3(function, region, fallback_region)
}

fn validate_region_steps_supported_by_current_lowering_v3(
    function: SerializedFunctionId,
    region: &MechanicalRegionEmission,
) -> Result<(), String> {
    let local_fallback_targets = local_fallback_region_targets_v3(region)?;
    for step in &region.steps {
        match &step.op {
            MechanicalStepOp::Input { output } => {
                return Err(format!(
                    "function {function} region {:?} input node {:?} produces {:?}; current mechanical lowering expects region inputs to be preloaded, not emitted as nodes",
                    region.region, step.node, output.id
                ));
            }
            MechanicalStepOp::Constant { output, constant } => {
                if !matches!(constant, PlannedConstant::I64(_)) {
                    return Err(format!(
                        "function {function} region {:?} constant node {:?} uses unsupported constant {constant:?}; current mechanical lowering only emits i64 constants",
                        region.region, step.node
                    ));
                }
                if output.rep != Rep::I64 {
                    return Err(format!(
                        "function {function} region {:?} constant node {:?} produces {:?}; current mechanical lowering only emits i64 constants",
                        region.region, step.node, output.rep
                    ));
                }
            }
            MechanicalStepOp::Guard {
                kind,
                inputs,
                failure,
            } => {
                if *kind != GuardKind::SpecializationCheck {
                    return Err(format!(
                        "function {function} region {:?} guard node {:?} has unsupported kind {kind:?}",
                        region.region, step.node
                    ));
                }
                if inputs.is_empty() {
                    return Err(format!(
                        "function {function} region {:?} guard node {:?} has no inputs",
                        region.region, step.node
                    ));
                }
                if !matches!(
                    failure,
                    GuardFailure::FallbackToPlan {
                        target: crate::plan_v3::FallbackTarget::Region(_),
                        ..
                    }
                ) {
                    return Err(format!(
                        "function {function} region {:?} guard node {:?} has unsupported failure {failure:?}; current mechanical lowering requires a region fallback",
                        region.region, step.node
                    ));
                }
            }
            MechanicalStepOp::Convert {
                kind,
                input,
                output,
                failure,
            } => validate_conversion_supported_by_current_lowering_v3(
                function,
                region.region,
                step.node,
                *kind,
                *input,
                *output,
                failure,
                !local_fallback_targets.is_empty(),
            )?,
            MechanicalStepOp::Operation {
                op,
                inputs,
                output,
                failure,
            } => validate_operation_supported_by_current_lowering_v3(
                function,
                region.region,
                step.node,
                op,
                inputs,
                *output,
                failure,
                !local_fallback_targets.is_empty(),
            )?,
            MechanicalStepOp::Materialize {
                kind,
                input,
                output,
            } => validate_materialize_supported_by_current_lowering_v3(
                function,
                region.region,
                step.node,
                *kind,
                *input,
                *output,
            )?,
            MechanicalStepOp::Fallback { .. }
            | MechanicalStepOp::Deopt { .. }
            | MechanicalStepOp::Ownership { .. } => {
                return Err(format!(
                    "function {function} region {:?} node {:?} has unsupported codegen step {:?}",
                    region.region, step.node, step.op
                ));
            }
        }
    }
    Ok(())
}

fn validate_conversion_supported_by_current_lowering_v3(
    function: SerializedFunctionId,
    region: RegionId,
    node: PlanNodeId,
    kind: ConversionKind,
    input: PlanValue,
    output: PlanValue,
    failure: &FailureMode,
    has_local_fallback: bool,
) -> Result<(), String> {
    match kind {
        ConversionKind::FromPythonLongCompactToI64 => {
            if !has_local_fallback
                || !matches!(
                    failure,
                    FailureMode::FallbackToPlan {
                        target: crate::plan_v3::FallbackTarget::Region(_),
                        ..
                    }
                )
            {
                return Err(format!(
                    "function {function} region {region:?} conversion node {node:?} needs a region local fallback for compact-long guard misses"
                ));
            }
            if input.rep != Rep::PyObjectBorrowed || output.rep != Rep::I64 {
                return Err(format!(
                    "function {function} region {region:?} conversion node {node:?} expects PyObjectBorrowed->I64, got {:?}->{:?}",
                    input.rep, output.rep
                ));
            }
            Ok(())
        }
        ConversionKind::TruthinessToI32Bool01 => {
            if input.rep != Rep::PyObjectOwned || output.rep != Rep::I32Bool01 {
                return Err(format!(
                    "function {function} region {region:?} conversion node {node:?} expects PyObjectOwned->I32Bool01, got {:?}->{:?}",
                    input.rep, output.rep
                ));
            }
            if !matches!(failure, FailureMode::Raise(_)) {
                return Err(format!(
                    "function {function} region {region:?} conversion node {node:?} expects Python truthiness failure to raise locally, got {failure:?}"
                ));
            }
            Ok(())
        }
        ConversionKind::ToPythonLongOwned | ConversionKind::ToPythonBoolImmortal => Err(format!(
            "function {function} region {region:?} conversion node {node:?} has unsupported conversion {kind:?}; current mechanical lowering uses materialization nodes for Python object creation"
        )),
    }
}

fn validate_operation_supported_by_current_lowering_v3(
    function: SerializedFunctionId,
    region: RegionId,
    node: PlanNodeId,
    op: &MechanicalOperation,
    inputs: &[PlanValue],
    output: Option<PlanValue>,
    failure: &FailureMode,
    has_local_fallback: bool,
) -> Result<(), String> {
    match op {
        MechanicalOperation::PyNumberAdd
        | MechanicalOperation::PyNumberSubtract
        | MechanicalOperation::PyNumberMultiply
        | MechanicalOperation::PyNumberBitAnd
        | MechanicalOperation::PyNumberBitOr
        | MechanicalOperation::PyNumberBitXor
        | MechanicalOperation::PyObjectRichCompare { .. } => {
            validate_supported_python_operation_signature_v3(
                function, region, node, op, inputs, output, 2,
            )?;
            if !matches!(failure, FailureMode::Raise(_)) {
                return Err(format!(
                    "function {function} region {region:?} operation node {node:?} {op:?} expects local Python raise failure, got {failure:?}"
                ));
            }
            Ok(())
        }
        MechanicalOperation::CheckedI64Add
        | MechanicalOperation::CheckedI64Sub
        | MechanicalOperation::CheckedI64Mul => {
            validate_supported_operation_signature_v3(
                function,
                region,
                node,
                op,
                inputs,
                output,
                &[Rep::I64, Rep::I64],
                Rep::I64,
            )?;
            if !has_local_fallback
                || !matches!(
                    failure,
                    FailureMode::FallbackToPlan {
                        target: crate::plan_v3::FallbackTarget::Region(_),
                        ..
                    }
                )
            {
                return Err(format!(
                    "function {function} region {region:?} operation node {node:?} {op:?} needs a region local fallback for overflow"
                ));
            }
            Ok(())
        }
        MechanicalOperation::I64BitAnd
        | MechanicalOperation::I64BitOr
        | MechanicalOperation::I64BitXor => {
            validate_supported_operation_signature_v3(
                function,
                region,
                node,
                op,
                inputs,
                output,
                &[Rep::I64, Rep::I64],
                Rep::I64,
            )?;
            if failure != &FailureMode::CannotFail {
                return Err(format!(
                    "function {function} region {region:?} operation node {node:?} {op:?} must be CannotFail, got {failure:?}"
                ));
            }
            Ok(())
        }
        MechanicalOperation::I64CompareToBool01 { .. } => {
            validate_supported_operation_signature_v3(
                function,
                region,
                node,
                op,
                inputs,
                output,
                &[Rep::I64, Rep::I64],
                Rep::I32Bool01,
            )?;
            if failure != &FailureMode::CannotFail {
                return Err(format!(
                    "function {function} region {region:?} operation node {node:?} {op:?} must be CannotFail, got {failure:?}"
                ));
            }
            Ok(())
        }
        MechanicalOperation::PyObjectIsTrue | MechanicalOperation::DirectHelper { .. } => {
            Err(format!(
                "function {function} region {region:?} operation node {node:?} has unsupported operation {op:?}"
            ))
        }
    }
}

fn validate_supported_python_operation_signature_v3(
    function: SerializedFunctionId,
    region: RegionId,
    node: PlanNodeId,
    op: &MechanicalOperation,
    inputs: &[PlanValue],
    output: Option<PlanValue>,
    expected_input_count: usize,
) -> Result<(), String> {
    if inputs.len() != expected_input_count {
        return Err(format!(
            "function {function} region {region:?} operation node {node:?} {op:?} expects {expected_input_count} inputs, got {}",
            inputs.len()
        ));
    }
    for (index, input) in inputs.iter().enumerate() {
        if !input.rep.is_python_object() {
            return Err(format!(
                "function {function} region {region:?} operation node {node:?} {op:?} input {index} expects a Python object rep, got {:?}",
                input.rep
            ));
        }
    }
    match output {
        Some(output) if output.rep == Rep::PyObjectOwned => Ok(()),
        Some(output) => Err(format!(
            "function {function} region {region:?} operation node {node:?} {op:?} output expects PyObjectOwned, got {:?}",
            output.rep
        )),
        None => Err(format!(
            "function {function} region {region:?} operation node {node:?} {op:?} expects output PyObjectOwned, got no output"
        )),
    }
}

fn validate_supported_operation_signature_v3(
    function: SerializedFunctionId,
    region: RegionId,
    node: PlanNodeId,
    op: &MechanicalOperation,
    inputs: &[PlanValue],
    output: Option<PlanValue>,
    expected_inputs: &[Rep],
    expected_output: Rep,
) -> Result<(), String> {
    if inputs.len() != expected_inputs.len() {
        return Err(format!(
            "function {function} region {region:?} operation node {node:?} {op:?} expects {} inputs, got {}",
            expected_inputs.len(),
            inputs.len()
        ));
    }
    for (index, (input, expected)) in inputs.iter().zip(expected_inputs.iter()).enumerate() {
        if input.rep != *expected {
            return Err(format!(
                "function {function} region {region:?} operation node {node:?} {op:?} input {index} expects {expected:?}, got {:?}",
                input.rep
            ));
        }
    }
    match output {
        Some(output) if output.rep == expected_output => Ok(()),
        Some(output) => Err(format!(
            "function {function} region {region:?} operation node {node:?} {op:?} output expects {expected_output:?}, got {:?}",
            output.rep
        )),
        None => Err(format!(
            "function {function} region {region:?} operation node {node:?} {op:?} expects output {expected_output:?}, got no output"
        )),
    }
}

fn validate_materialize_supported_by_current_lowering_v3(
    function: SerializedFunctionId,
    region: RegionId,
    node: PlanNodeId,
    kind: MaterializeKind,
    input: PlanValue,
    output: PlanValue,
) -> Result<(), String> {
    let expected = match kind {
        MaterializeKind::PythonLong => (Rep::I64, Rep::PyObjectOwned),
        MaterializeKind::PythonBool => (Rep::I32Bool01, Rep::PyObjectImmortal),
    };
    if input.rep != expected.0 || output.rep != expected.1 {
        return Err(format!(
            "function {function} region {region:?} materialize node {node:?} {kind:?} expects {:?}->{:?}, got {:?}->{:?}",
            expected.0, expected.1, input.rep, output.rep
        ));
    }
    Ok(())
}

fn validate_scalar_thread_mechanical_lowering_shape_v3(
    function: SerializedFunctionId,
    thread: &ScalarLocalThreadPlan,
    emitted_regions: &HashMap<RegionId, &MechanicalRegionEmission>,
) -> Result<(), String> {
    let producer = emitted_regions.get(&thread.producer.region).ok_or_else(|| {
        format!(
            "function {function} scalar thread for local {} references missing producer region {:?}",
            thread.local.name, thread.producer.region
        )
    })?;
    let consumer = emitted_regions.get(&thread.consumer.region).ok_or_else(|| {
        format!(
            "function {function} scalar thread for local {} references missing consumer region {:?}",
            thread.local.name, thread.consumer.region
        )
    })?;
    if !has_single_return_exit_v3(producer) {
        return Err(format!(
            "function {function} scalar thread for local {} producer region {:?} is not a single-return region",
            thread.local.name, thread.producer.region
        ));
    }
    if !has_single_original_cfg_branch_exit_v3(consumer) {
        return Err(format!(
            "function {function} scalar thread for local {} consumer region {:?} is not a single OriginalCfg branch region",
            thread.local.name, thread.consumer.region
        ));
    }
    let producer_fallbacks = local_fallback_region_targets_v3(producer)?;
    let expected_fallback = match &thread.fallback {
        crate::plan_v3::ScalarThreadFallback::LocalFallbackRegion { region, .. } => *region,
    };
    if producer_fallbacks.len() != 1 || !producer_fallbacks.contains(&expected_fallback) {
        return Err(format!(
            "function {function} scalar thread for local {} has fallback {:?}, but producer region {:?} uses {:?}",
            thread.local.name, expected_fallback, thread.producer.region, producer_fallbacks
        ));
    }
    Ok(())
}

fn require_matching_fallback_exit_v3(
    function: SerializedFunctionId,
    hot_region: &MechanicalRegionEmission,
    fallback_region: &MechanicalRegionEmission,
) -> Result<(), String> {
    let hot_exit = hot_region
        .exits
        .first()
        .expect("caller checked hot region has an exit");
    match &hot_exit.kind {
        MechanicalExitKind::Branch { .. } => {
            require_single_original_cfg_branch_exit_v3(function, fallback_region)?;
            let fallback_exit = fallback_region
                .exits
                .first()
                .expect("branch fallback has one exit");
            if fallback_exit.source != hot_exit.source {
                return Err(format!(
                    "function {function} branch region {:?} fallback {:?} uses exit source {:?}, expected {:?}",
                    hot_region.region,
                    fallback_region.region,
                    fallback_exit.source,
                    hot_exit.source
                ));
            }
            Ok(())
        }
        MechanicalExitKind::Return { .. } => {
            require_single_return_exit_v3(function, fallback_region)?;
            let fallback_exit = fallback_region
                .exits
                .first()
                .expect("return fallback has one exit");
            if fallback_exit.source != hot_exit.source {
                return Err(format!(
                    "function {function} return region {:?} fallback {:?} uses exit source {:?}, expected {:?}",
                    hot_region.region,
                    fallback_region.region,
                    fallback_exit.source,
                    hot_exit.source
                ));
            }
            Ok(())
        }
        MechanicalExitKind::Jump { .. } => Ok(()),
    }
}

fn require_single_original_cfg_branch_exit_v3(
    function: SerializedFunctionId,
    region: &MechanicalRegionEmission,
) -> Result<(), String> {
    if !has_single_original_cfg_branch_exit_v3(region) {
        return Err(format!(
            "function {function} region {:?} must have exactly one OriginalCfg branch exit for current mechanical lowering, got {:?}",
            region.region, region.exits
        ));
    }
    Ok(())
}

fn has_single_original_cfg_branch_exit_v3(region: &MechanicalRegionEmission) -> bool {
    let [exit] = region.exits.as_slice() else {
        return false;
    };
    matches!(
        &exit.kind,
        MechanicalExitKind::Branch {
            then_target,
            else_target,
            ..
        } if matches!(then_target, &RegionExitTarget::OriginalCfg)
            && matches!(else_target, &RegionExitTarget::OriginalCfg)
    )
}

fn require_single_return_exit_v3(
    function: SerializedFunctionId,
    region: &MechanicalRegionEmission,
) -> Result<(), String> {
    if !has_single_return_exit_v3(region) {
        return Err(format!(
            "function {function} region {:?} must have exactly one return exit for current mechanical lowering, got {:?}",
            region.region, region.exits
        ));
    }
    Ok(())
}

fn has_single_return_exit_v3(region: &MechanicalRegionEmission) -> bool {
    let [exit] = region.exits.as_slice() else {
        return false;
    };
    matches!(&exit.kind, MechanicalExitKind::Return { .. })
}

fn local_fallback_region_targets_v3(
    region: &MechanicalRegionEmission,
) -> Result<HashSet<RegionId>, String> {
    let mut targets = HashSet::new();
    let mut raise_before_fallback = None;
    for step in &region.steps {
        match &step.op {
            MechanicalStepOp::Guard { failure, .. } => match failure {
                GuardFailure::FallbackToPlan {
                    target: crate::plan_v3::FallbackTarget::Region(region),
                    ..
                } => {
                    targets.insert(*region);
                }
                GuardFailure::FallbackToPlan { target, .. } => {
                    return Err(format!(
                        "region {:?} guard node {:?} uses unsupported fallback target {target:?}; current mechanical lowering requires region fallback targets",
                        region.region, step.node
                    ));
                }
                GuardFailure::DeoptTo { .. } => {
                    return Err(format!(
                        "region {:?} guard node {:?} uses deopt; current mechanical lowering requires a visible local fallback",
                        region.region, step.node
                    ));
                }
            },
            MechanicalStepOp::Convert { failure, .. }
            | MechanicalStepOp::Operation { failure, .. } => match failure {
                FailureMode::CannotFail => {}
                FailureMode::FallbackToPlan {
                    target: crate::plan_v3::FallbackTarget::Region(region),
                    ..
                } => {
                    targets.insert(*region);
                }
                FailureMode::FallbackToPlan { target, .. } => {
                    return Err(format!(
                        "current mechanical lowering only supports region fallback targets, got {target:?}"
                    ));
                }
                FailureMode::Raise(exception) => {
                    raise_before_fallback = Some(format!("{exception:?}"));
                }
                FailureMode::DeoptTo { .. } => {
                    return Err(
                        "current mechanical lowering requires a local fallback, not deopt"
                            .to_string(),
                    );
                }
            },
            _ => {}
        }
    }
    if !targets.is_empty()
        && let Some(exception) = raise_before_fallback
    {
        return Err(format!(
            "current mechanical lowering cannot raise before the local fallback, got {exception}"
        ));
    }
    Ok(targets)
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
        guard: MechanicalIndexedFieldGuard {
            kind: indexed_field.guard.kind,
            owner_type: indexed_field.owner_type.clone(),
            attr_name: indexed_field.attr_name.clone(),
            expected_index: indexed_field.expected_index,
        },
        fallback: indexed_field.fallback.clone(),
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
        ConstructorCallSpecializationPlan, ConversionOwnership, ConversionPrecondition,
        ConvertNode, Cost, DirectCallArgPlan, DirectCallArgSource, DirectCallSpecializationPlan,
        ExactListItemAccessKind, ExactListItemFallbackKind, ExactListItemFallbackPlan,
        ExactListItemGuardKind, ExactListItemGuardPlan, ExactListItemShape,
        ExactListItemSpecializationPlan, FallbackReason, FallbackTarget,
        FunctionOptimizationPlanV3, FunctionOwnershipPlan, FunctionPlanIdentity,
        IndexedFieldAccessKind, IndexedFieldFallbackKind, IndexedFieldFallbackPlan,
        IndexedFieldGuardKind, IndexedFieldGuardPlan, IndexedFieldOwnerType,
        IndexedFieldSpecializationPlan, MaterializeNode, MethodCallFallbackKind,
        MethodCallFallbackPlan, MethodCallGuardKind, MethodCallGuardPlan, MethodCallOwnerType,
        MethodCallSpecializationPlan, ModulePlanIdentity, PythonExceptionSpec, RegionExitPlan,
        RegionInput, RegionInputSource, RegionPlan, RegionSource, Rep,
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
    fn prepares_supported_codegen_steps() {
        let emission = emit_mechanical_plan_v3(&test_plan(true)).unwrap();
        let region = &emission.functions[0].regions[0];

        let step =
            mechanical_codegen_step(region.region, &region.steps[2], true, None, &HashSet::new())
                .expect("checked i64 add should prepare for current codegen");

        assert!(matches!(
            step,
            MechanicalCodegenStep::Operation {
                op: MechanicalCodegenOperation::CheckedI64Add,
                inputs: [
                    PlanValue { rep: Rep::I64, .. },
                    PlanValue { rep: Rep::I64, .. }
                ],
                output: PlanValue { rep: Rep::I64, .. }
            }
        ));

        let err = mechanical_codegen_step(
            region.region,
            &region.steps[2],
            false,
            None,
            &HashSet::new(),
        )
        .expect_err("checked i64 add without local fallback should be rejected before JIT");
        assert!(err.contains("local fallback"), "unexpected error: {err}");
    }

    #[test]
    fn prepares_region_function_param_inputs() {
        let value = PlanValue::new(0, Rep::PyObjectBorrowed);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![RegionInput {
                value,
                source: RegionInputSource::FunctionParam {
                    index: 0,
                    name: Some("arg".to_string()),
                },
            }],
            nodes: Vec::new(),
            exits: Vec::new(),
        };

        let inputs = mechanical_region_function_param_inputs(&region, "test region")
            .expect("named function-param input should prepare");

        assert_eq!(
            inputs,
            vec![MechanicalRegionFunctionParamInput { value, name: "arg" }]
        );
    }

    #[test]
    fn validates_emission_matches_plan() {
        let plan = test_plan(true);
        let mut emission = emit_mechanical_plan_v3(&plan).unwrap();
        emission.module_name = "pkg.other".to_string();

        let err = validate_mechanical_emission_matches_plan_v3(&plan, &emission).unwrap_err();

        match err {
            MechanicalEmitError::EmissionMismatch(message) => {
                assert!(message.contains("emitted module name"));
            }
            MechanicalEmitError::InvalidPlan(err) => panic!("unexpected plan error: {err}"),
        }
    }

    #[test]
    fn validates_current_mechanical_lowering_shape() {
        let mut plan = test_plan(true);
        let PlanNodeKind::Operation(operation) = &mut plan.functions[0].regions[0].nodes[2].kind
        else {
            panic!("test plan node should be an operation");
        };
        operation.failure = FailureMode::FallbackToPlan {
            target: FallbackTarget::Node(PlanNodeId(3)),
            reason: FallbackReason("overflow uses node fallback".to_string()),
        };

        let err = emit_mechanical_plan_v3(&plan).unwrap_err();

        match err {
            MechanicalEmitError::EmissionMismatch(message) => {
                assert!(
                    message.contains("only supports region fallback targets"),
                    "{message}"
                );
            }
            MechanicalEmitError::InvalidPlan(err) => panic!("unexpected plan error: {err}"),
        }
    }

    fn mechanical_emission_mismatch(plan: &ModuleOptimizationPlanV3) -> String {
        match emit_mechanical_plan_v3(plan).unwrap_err() {
            MechanicalEmitError::EmissionMismatch(message) => message,
            MechanicalEmitError::InvalidPlan(err) => panic!("unexpected plan error: {err}"),
        }
    }

    #[test]
    fn validates_current_lowering_rejects_unsupported_region_inputs() {
        let mut plan = test_plan(true);
        let input = PlanValue::new(99, Rep::I64);
        plan.functions[0].regions[0].inputs.push(RegionInput {
            value: input,
            source: RegionInputSource::Synthetic {
                reason: "test synthetic input".to_string(),
            },
        });

        let message = mechanical_emission_mismatch(&plan);

        assert!(
            message.contains("only supports named function-param inputs"),
            "{message}"
        );
    }

    #[test]
    fn validates_current_lowering_rejects_unsupported_constants() {
        let mut plan = test_plan(true);
        let PlanNodeKind::Constant { constant, .. } =
            &mut plan.functions[0].regions[0].nodes[0].kind
        else {
            panic!("test plan node should be a constant");
        };
        *constant = PlannedConstant::Bool(true);

        let message = mechanical_emission_mismatch(&plan);

        assert!(message.contains("unsupported constant"), "{message}");
    }

    #[test]
    fn validates_current_lowering_rejects_unsupported_conversions() {
        let mut plan = test_plan(true);
        let input = PlanValue::new(10, Rep::I64);
        let output = PlanValue::new(98, Rep::PyObjectOwned);
        plan.functions[0].regions[1].nodes.insert(
            1,
            crate::plan_v3::PlanNode {
                id: PlanNodeId(98),
                source: None,
                kind: PlanNodeKind::Convert(ConvertNode {
                    input,
                    output,
                    kind: ConversionKind::ToPythonLongOwned,
                    precondition: ConversionPrecondition::Infallible,
                    failure: FailureMode::Raise(PythonExceptionSpec {
                        kind: "MemoryError".to_string(),
                        reason: "test allocation failure".to_string(),
                    }),
                    ownership: ConversionOwnership::MaterializeOwned,
                }),
            },
        );

        let message = mechanical_emission_mismatch(&plan);

        assert!(
            message.contains("unsupported conversion ToPythonLongOwned"),
            "{message}"
        );
    }

    #[test]
    fn validates_current_lowering_rejects_unsupported_operations() {
        let mut plan = test_plan(true);
        let input = PlanValue::new(11, Rep::PyObjectOwned);
        let output = PlanValue::new(98, Rep::I32Bool01);
        plan.functions[0].regions[1]
            .nodes
            .push(crate::plan_v3::PlanNode {
                id: PlanNodeId(98),
                source: None,
                kind: PlanNodeKind::Operation(OperationNode {
                    op: PlannedOp::PyObjectIsTrue,
                    inputs: vec![input],
                    output: Some(output),
                    failure_replay: crate::plan_v3::FailureReplayPolicy::local_fallback(
                        "test truthiness failure replay",
                    ),
                    failure: FailureMode::Raise(PythonExceptionSpec {
                        kind: "Exception".to_string(),
                        reason: "test truthiness can raise".to_string(),
                    }),
                    cost: Cost::default(),
                }),
            });

        let message = mechanical_emission_mismatch(&plan);

        assert!(
            message.contains("unsupported operation PyObjectIsTrue"),
            "{message}"
        );
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
        let guard = IndexedFieldGuardPlan {
            kind: IndexedFieldGuardKind::OwnerTypeVersionAndFieldIndex,
        };
        let fallback = IndexedFieldFallbackPlan {
            kind: IndexedFieldFallbackKind::OriginalAttrAccess,
        };
        plan.functions[0]
            .indexed_fields
            .push(IndexedFieldSpecializationPlan {
                source: load_source,
                access: IndexedFieldAccessKind::Load,
                owner_type: owner_type.clone(),
                attr_name: "value".to_string(),
                expected_index: 2,
                guard: guard.clone(),
                fallback: fallback.clone(),
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
                guard: guard.clone(),
                fallback: fallback.clone(),
                reason: "profiled type_keys selected this indexed-field layout".to_string(),
            });

        let emission = emit_mechanical_plan_v3(&plan).unwrap();

        assert_eq!(
            emission.functions[0].indexed_fields,
            vec![
                MechanicalIndexedFieldEmission {
                    source: load_source,
                    access: IndexedFieldAccessKind::Load,
                    guard: MechanicalIndexedFieldGuard {
                        kind: guard.kind,
                        owner_type: owner_type.clone(),
                        attr_name: "value".to_string(),
                        expected_index: 2,
                    },
                    fallback: fallback.clone(),
                    reason: "profiled type_keys selected this indexed-field layout".to_string(),
                },
                MechanicalIndexedFieldEmission {
                    source: store_source,
                    access: IndexedFieldAccessKind::Store,
                    guard: MechanicalIndexedFieldGuard {
                        kind: guard.kind,
                        owner_type,
                        attr_name: "value".to_string(),
                        expected_index: 2,
                    },
                    fallback,
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
            MechanicalEmitError::EmissionMismatch(message) => {
                panic!("unexpected emission mismatch: {message}");
            }
        }
    }
}
