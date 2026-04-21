use crate::plan_v3::{
    ConversionKind, Cost, DeoptPointId, DeoptReason, FailureMode, FailureReplayKind,
    FailureReplayPolicy, FallbackReason, FallbackTarget, GuardFailure, GuardKind, GuardNode,
    GuardSpec, MaterializeKind, PlannedOp, PythonExceptionSpec, Rep, RichCompareOp,
    conversion_signature,
};
use soac_core::block_py::BinOpKind;
use std::collections::HashSet;
use std::fmt;

pub const ALTERNATIVE_CATALOG_V3_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlternativeCatalog {
    pub version: u32,
    pub alternatives: Vec<LoweringAlternative>,
}

impl AlternativeCatalog {
    pub fn default_v3() -> Self {
        Self {
            version: ALTERNATIVE_CATALOG_V3_VERSION,
            alternatives: vec![
                generic_binary_add(),
                exact_compact_int_add(),
                generic_binary_subtract(),
                exact_compact_int_subtract(),
                generic_binary_multiply(),
                exact_compact_int_multiply(),
                generic_bitwise(
                    "binary.and.py_generic",
                    BinOpKind::And,
                    PlannedOp::PyNumberBitAnd,
                    "PyNumber_And",
                ),
                exact_compact_int_bitwise(
                    "binary.and.exact_compact_int.i64",
                    BinOpKind::And,
                    PlannedOp::I64BitAnd,
                ),
                generic_bitwise(
                    "binary.or.py_generic",
                    BinOpKind::Or,
                    PlannedOp::PyNumberBitOr,
                    "PyNumber_Or",
                ),
                exact_compact_int_bitwise(
                    "binary.or.exact_compact_int.i64",
                    BinOpKind::Or,
                    PlannedOp::I64BitOr,
                ),
                generic_bitwise(
                    "binary.xor.py_generic",
                    BinOpKind::Xor,
                    PlannedOp::PyNumberBitXor,
                    "PyNumber_Xor",
                ),
                exact_compact_int_bitwise(
                    "binary.xor.exact_compact_int.i64",
                    BinOpKind::Xor,
                    PlannedOp::I64BitXor,
                ),
                generic_rich_compare("binary.eq.py_richcompare", BinOpKind::Eq, RichCompareOp::Eq),
                exact_compact_int_compare(
                    "binary.eq.exact_compact_int.i32bool",
                    BinOpKind::Eq,
                    RichCompareOp::Eq,
                ),
                generic_rich_compare("binary.ne.py_richcompare", BinOpKind::Ne, RichCompareOp::Ne),
                exact_compact_int_compare(
                    "binary.ne.exact_compact_int.i32bool",
                    BinOpKind::Ne,
                    RichCompareOp::Ne,
                ),
                generic_rich_compare("binary.lt.py_richcompare", BinOpKind::Lt, RichCompareOp::Lt),
                exact_compact_int_compare(
                    "binary.lt.exact_compact_int.i32bool",
                    BinOpKind::Lt,
                    RichCompareOp::Lt,
                ),
                generic_rich_compare("binary.le.py_richcompare", BinOpKind::Le, RichCompareOp::Le),
                exact_compact_int_compare(
                    "binary.le.exact_compact_int.i32bool",
                    BinOpKind::Le,
                    RichCompareOp::Le,
                ),
                generic_rich_compare("binary.gt.py_richcompare", BinOpKind::Gt, RichCompareOp::Gt),
                exact_compact_int_compare(
                    "binary.gt.exact_compact_int.i32bool",
                    BinOpKind::Gt,
                    RichCompareOp::Gt,
                ),
                generic_rich_compare("binary.ge.py_richcompare", BinOpKind::Ge, RichCompareOp::Ge),
                exact_compact_int_compare(
                    "binary.ge.exact_compact_int.i32bool",
                    BinOpKind::Ge,
                    RichCompareOp::Ge,
                ),
                python_truthiness(),
                normalized_truthiness(),
                materialize_python_long(),
                materialize_python_bool(),
            ],
        }
    }

    pub fn by_id(&self, id: AlternativeId) -> Option<&LoweringAlternative> {
        self.alternatives
            .iter()
            .find(|alternative| alternative.id == id)
    }

    pub fn alternatives_for(
        &self,
        op: SemanticOpKind,
    ) -> impl Iterator<Item = &LoweringAlternative> {
        self.alternatives
            .iter()
            .filter(move |alternative| alternative.op == op)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoweringAlternative {
    pub id: AlternativeId,
    pub op: SemanticOpKind,
    pub input_reps: Vec<RepRequirement>,
    pub output_rep: Option<Rep>,
    pub required_facts: Vec<FactPredicate>,
    pub output_facts: Vec<ValueFact>,
    pub emission: AlternativeEmission,
    pub guards: Vec<GuardRequirement>,
    pub failure_replay: FailureReplayPolicy,
    pub failure: AlternativeFailure,
    pub cost: Cost,
    pub rationale: &'static str,
}

impl LoweringAlternative {
    pub fn planned_operation(&self) -> Option<PlannedOp> {
        match &self.emission {
            AlternativeEmission::Operation(op) => Some(op.clone()),
            AlternativeEmission::Conversion(_)
            | AlternativeEmission::Materialize(_)
            | AlternativeEmission::Identity => None,
        }
    }

    pub fn instantiate_failure(
        &self,
        targets: &FailureTargets,
    ) -> Result<FailureMode, AlternativeInstantiationError> {
        self.failure.instantiate(targets)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AlternativeId(pub &'static str);

impl AlternativeId {
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SemanticOpKind {
    Binary { op: BinOpKind },
    Truthiness,
    Materialize { kind: MaterializeKind },
}

impl SemanticOpKind {
    pub const fn arity(self) -> usize {
        match self {
            Self::Binary { .. } => 2,
            Self::Truthiness | Self::Materialize { .. } => 1,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepRequirement {
    pub input: u32,
    pub rep: Rep,
    pub source: RepRequirementSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RepRequirementSource {
    AlreadyAvailable,
    Conversion { kind: ConversionKind },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactPredicate {
    Input { input: u32, fact: ValueFact },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ValueFact {
    ExactCompactPyLong,
    I64,
    Bool01,
    PythonObject,
    PythonBoolObject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlternativeEmission {
    Operation(PlannedOp),
    Conversion(ConversionKind),
    Materialize(MaterializeKind),
    Identity,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GuardRequirement {
    pub input: u32,
    pub kind: GuardRequirementKind,
    pub description: &'static str,
    pub replay: FailureReplayPolicy,
    pub miss: GuardMiss,
}

impl GuardRequirement {
    pub fn instantiate(
        &self,
        targets: &FailureTargets,
    ) -> Result<GuardNode, AlternativeInstantiationError> {
        Ok(GuardNode {
            inputs: Vec::new(),
            guard: GuardSpec {
                kind: GuardKind::SpecializationCheck,
                replay: self.replay.clone(),
                description: self.description.to_string(),
            },
            failure: self.miss.instantiate(targets)?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GuardRequirementKind {
    ExactCompactPyLong,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GuardMiss {
    LocalFallback { reason: &'static str },
    Deopt { reason: &'static str },
}

impl GuardMiss {
    fn instantiate(
        &self,
        targets: &FailureTargets,
    ) -> Result<GuardFailure, AlternativeInstantiationError> {
        match self {
            Self::LocalFallback { reason } => Ok(GuardFailure::FallbackToPlan {
                target: targets.fallback.clone(),
                reason: FallbackReason((*reason).to_string()),
            }),
            Self::Deopt { reason } => {
                let target = targets.deopt.ok_or_else(|| {
                    AlternativeInstantiationError("deopt guard requires a deopt target".to_string())
                })?;
                Ok(GuardFailure::DeoptTo {
                    target,
                    reason: DeoptReason((*reason).to_string()),
                })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AlternativeFailure {
    CannotFail,
    Raise {
        kind: &'static str,
        reason: &'static str,
    },
    LocalFallback {
        reason: &'static str,
    },
    Deopt {
        reason: &'static str,
    },
}

impl AlternativeFailure {
    fn instantiate(
        &self,
        targets: &FailureTargets,
    ) -> Result<FailureMode, AlternativeInstantiationError> {
        match self {
            Self::CannotFail => Ok(FailureMode::CannotFail),
            Self::Raise { kind, reason } => Ok(FailureMode::Raise(PythonExceptionSpec {
                kind: (*kind).to_string(),
                reason: (*reason).to_string(),
            })),
            Self::LocalFallback { reason } => Ok(FailureMode::FallbackToPlan {
                target: targets.fallback.clone(),
                reason: FallbackReason((*reason).to_string()),
            }),
            Self::Deopt { reason } => {
                let target = targets.deopt.ok_or_else(|| {
                    AlternativeInstantiationError(
                        "deopt alternative requires a deopt target".to_string(),
                    )
                })?;
                Ok(FailureMode::DeoptTo {
                    target,
                    reason: DeoptReason((*reason).to_string()),
                })
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailureTargets {
    pub fallback: FallbackTarget,
    pub deopt: Option<DeoptPointId>,
}

impl FailureTargets {
    pub const fn local_fallback(fallback: FallbackTarget) -> Self {
        Self {
            fallback,
            deopt: None,
        }
    }

    pub const fn with_deopt(fallback: FallbackTarget, deopt: DeoptPointId) -> Self {
        Self {
            fallback,
            deopt: Some(deopt),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlternativeInstantiationError(pub String);

impl fmt::Display for AlternativeInstantiationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for AlternativeInstantiationError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AlternativeCatalogValidationError {
    pub errors: Vec<String>,
}

impl AlternativeCatalogValidationError {
    pub fn contains(&self, needle: &str) -> bool {
        self.errors.iter().any(|error| error.contains(needle))
    }
}

impl fmt::Display for AlternativeCatalogValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, error) in self.errors.iter().enumerate() {
            if index != 0 {
                writeln!(f)?;
            }
            write!(f, "{error}")?;
        }
        Ok(())
    }
}

impl std::error::Error for AlternativeCatalogValidationError {}

pub fn validate_alternative_catalog_v3(
    catalog: &AlternativeCatalog,
) -> Result<(), AlternativeCatalogValidationError> {
    let mut errors = Vec::new();
    if catalog.version == 0 {
        errors.push("alternative catalog has zero version".to_string());
    }
    let mut ids = HashSet::new();
    for alternative in &catalog.alternatives {
        validate_alternative(alternative, &mut ids, &mut errors);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(AlternativeCatalogValidationError { errors })
    }
}

fn validate_alternative(
    alternative: &LoweringAlternative,
    ids: &mut HashSet<AlternativeId>,
    errors: &mut Vec<String>,
) {
    let id = alternative.id.as_str();
    if id.is_empty() {
        errors.push("alternative has empty id".to_string());
    }
    if !ids.insert(alternative.id) {
        errors.push(format!("duplicate alternative id {id}"));
    }
    let arity = alternative.op.arity();
    let mut inputs = HashSet::new();
    for requirement in &alternative.input_reps {
        if requirement.input as usize >= arity {
            errors.push(format!(
                "alternative {id} has input requirement for input {}, but {:?} has arity {arity}",
                requirement.input, alternative.op
            ));
        }
        if !inputs.insert(requirement.input) {
            errors.push(format!(
                "alternative {id} has duplicate rep requirement for input {}",
                requirement.input
            ));
        }
        if let RepRequirementSource::Conversion { kind } = requirement.source {
            let signature = conversion_signature(kind);
            if signature.output != requirement.rep {
                errors.push(format!(
                    "alternative {id} conversion {:?} produces {:?}, not required {:?}",
                    kind, signature.output, requirement.rep
                ));
            }
        }
    }
    for input in 0..arity as u32 {
        if !inputs.contains(&input) {
            errors.push(format!(
                "alternative {id} has no rep requirement for input {input}"
            ));
        }
    }
    for predicate in &alternative.required_facts {
        let FactPredicate::Input { input, .. } = predicate;
        if *input as usize >= arity {
            errors.push(format!(
                "alternative {id} has fact predicate for input {input}, but {:?} has arity {arity}",
                alternative.op
            ));
        }
    }
    for guard in &alternative.guards {
        if guard.input as usize >= arity {
            errors.push(format!(
                "alternative {id} has guard for input {}, but {:?} has arity {arity}",
                guard.input, alternative.op
            ));
        }
        if guard.description.is_empty() {
            errors.push(format!("alternative {id} has guard without description"));
        }
        validate_replay_policy(
            id,
            &guard.replay,
            matches!(guard.miss, GuardMiss::Deopt { .. }),
            errors,
        );
        match &guard.miss {
            GuardMiss::LocalFallback { reason } | GuardMiss::Deopt { reason } => {
                if reason.is_empty() {
                    errors.push(format!("alternative {id} has guard miss without reason"));
                }
            }
        }
    }
    validate_alternative_emission(alternative, errors);
    validate_replay_policy(
        id,
        &alternative.failure_replay,
        matches!(alternative.failure, AlternativeFailure::Deopt { .. }),
        errors,
    );
    match &alternative.failure {
        AlternativeFailure::Raise { kind, reason } => {
            if kind.is_empty() || reason.is_empty() {
                errors.push(format!(
                    "alternative {id} has raising failure without kind and reason"
                ));
            }
        }
        AlternativeFailure::LocalFallback { reason } | AlternativeFailure::Deopt { reason } => {
            if reason.is_empty() {
                errors.push(format!("alternative {id} has failure without reason"));
            }
        }
        AlternativeFailure::CannotFail => {}
    }
    if alternative.rationale.is_empty() {
        errors.push(format!("alternative {id} has empty rationale"));
    }
}

fn validate_alternative_emission(alternative: &LoweringAlternative, errors: &mut Vec<String>) {
    let id = alternative.id.as_str();
    match &alternative.emission {
        AlternativeEmission::Operation(op) => {
            validate_operation_emission(id, op, alternative, errors)
        }
        AlternativeEmission::Conversion(kind) => {
            let signature = conversion_signature(*kind);
            if alternative.input_reps.len() == 1 && alternative.input_reps[0].rep != signature.input
            {
                errors.push(format!(
                    "alternative {id} conversion {:?} expects input {:?}, got {:?}",
                    kind, signature.input, alternative.input_reps[0].rep
                ));
            }
            if alternative.output_rep != Some(signature.output) {
                errors.push(format!(
                    "alternative {id} conversion {:?} produces {:?}, not {:?}",
                    kind, signature.output, alternative.output_rep
                ));
            }
        }
        AlternativeEmission::Materialize(kind) => {
            let expected = match kind {
                MaterializeKind::PythonLong => (Rep::I64, Rep::PyObjectOwned),
                MaterializeKind::PythonBool => (Rep::I32Bool01, Rep::PyObjectImmortal),
            };
            if alternative.input_reps.len() == 1 && alternative.input_reps[0].rep != expected.0 {
                errors.push(format!(
                    "alternative {id} materialization {:?} expects input {:?}, got {:?}",
                    kind, expected.0, alternative.input_reps[0].rep
                ));
            }
            if alternative.output_rep != Some(expected.1) {
                errors.push(format!(
                    "alternative {id} materialization {:?} produces {:?}, not {:?}",
                    kind, expected.1, alternative.output_rep
                ));
            }
        }
        AlternativeEmission::Identity => {
            if alternative.input_reps.len() == 1
                && alternative.output_rep != Some(alternative.input_reps[0].rep)
            {
                errors.push(format!(
                    "alternative {id} identity output {:?} does not match input {:?}",
                    alternative.output_rep, alternative.input_reps[0].rep
                ));
            }
            if !matches!(alternative.failure, AlternativeFailure::CannotFail) {
                errors.push(format!("alternative {id} identity must not fail"));
            }
        }
    }
}

fn validate_operation_emission(
    id: &str,
    op: &PlannedOp,
    alternative: &LoweringAlternative,
    errors: &mut Vec<String>,
) {
    match op {
        PlannedOp::PyNumberAdd
        | PlannedOp::PyNumberSubtract
        | PlannedOp::PyNumberMultiply
        | PlannedOp::PyNumberBitAnd
        | PlannedOp::PyNumberBitOr
        | PlannedOp::PyNumberBitXor => {
            validate_catalog_operation_signature(
                id,
                alternative,
                &[RepClass::PythonObject, RepClass::PythonObject],
                Some(Rep::PyObjectOwned),
                errors,
            );
        }
        PlannedOp::PyObjectRichCompare { .. } => {
            validate_catalog_operation_signature(
                id,
                alternative,
                &[RepClass::PythonObject, RepClass::PythonObject],
                Some(Rep::PyObjectOwned),
                errors,
            );
        }
        PlannedOp::PyObjectIsTrue => {
            validate_catalog_operation_signature(
                id,
                alternative,
                &[RepClass::PythonObject],
                Some(Rep::I32Bool01),
                errors,
            );
        }
        PlannedOp::CheckedI64Add
        | PlannedOp::CheckedI64Sub
        | PlannedOp::CheckedI64Mul
        | PlannedOp::I64BitAnd
        | PlannedOp::I64BitOr
        | PlannedOp::I64BitXor => {
            validate_catalog_operation_signature(
                id,
                alternative,
                &[RepClass::Exact(Rep::I64), RepClass::Exact(Rep::I64)],
                Some(Rep::I64),
                errors,
            );
        }
        PlannedOp::I64CompareToBool01 { .. } => {
            validate_catalog_operation_signature(
                id,
                alternative,
                &[RepClass::Exact(Rep::I64), RepClass::Exact(Rep::I64)],
                Some(Rep::I32Bool01),
                errors,
            );
        }
        PlannedOp::DirectHelper { .. } => {}
    }
}

#[derive(Clone, Copy)]
enum RepClass {
    Exact(Rep),
    PythonObject,
}

fn validate_catalog_operation_signature(
    id: &str,
    alternative: &LoweringAlternative,
    inputs: &[RepClass],
    output: Option<Rep>,
    errors: &mut Vec<String>,
) {
    if alternative.input_reps.len() != inputs.len() {
        errors.push(format!(
            "alternative {id} emission expects {} inputs, got {}",
            inputs.len(),
            alternative.input_reps.len()
        ));
        return;
    }
    for (index, (requirement, expected)) in alternative.input_reps.iter().zip(inputs).enumerate() {
        let matches = match expected {
            RepClass::Exact(rep) => requirement.rep == *rep,
            RepClass::PythonObject => requirement.rep.is_python_object(),
        };
        if !matches {
            errors.push(format!(
                "alternative {id} emission input {index} expects {}, got {:?}",
                expected.label(),
                requirement.rep
            ));
        }
    }
    if alternative.output_rep != output {
        errors.push(format!(
            "alternative {id} emission output expects {:?}, got {:?}",
            output, alternative.output_rep
        ));
    }
}

impl RepClass {
    fn label(self) -> &'static str {
        match self {
            Self::Exact(Rep::PyObjectOwned) => "PyObjectOwned",
            Self::Exact(Rep::PyObjectBorrowed) => "PyObjectBorrowed",
            Self::Exact(Rep::PyObjectImmortal) => "PyObjectImmortal",
            Self::Exact(Rep::I32Bool01) => "I32Bool01",
            Self::Exact(Rep::I64) => "I64",
            Self::PythonObject => "a Python object representation",
        }
    }
}

fn validate_replay_policy(
    id: &str,
    replay: &FailureReplayPolicy,
    deopt: bool,
    errors: &mut Vec<String>,
) {
    if replay.reason.0.is_empty() {
        errors.push(format!("alternative {id} has replay policy without reason"));
    }
    if deopt && replay.replay != FailureReplayKind::SafeToReplay {
        errors.push(format!(
            "alternative {id} uses deopt without replay-safe policy"
        ));
    }
}

fn generic_binary_add() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("binary.add.py_generic"),
        op: SemanticOpKind::Binary { op: BinOpKind::Add },
        input_reps: vec![pyobject_input(0), pyobject_input(1)],
        output_rep: Some(Rep::PyObjectOwned),
        required_facts: vec![
            FactPredicate::Input {
                input: 0,
                fact: ValueFact::PythonObject,
            },
            FactPredicate::Input {
                input: 1,
                fact: ValueFact::PythonObject,
            },
        ],
        output_facts: vec![ValueFact::PythonObject],
        emission: AlternativeEmission::Operation(PlannedOp::PyNumberAdd),
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "PyNumber_Add raises or returns locally without replay",
        ),
        failure: AlternativeFailure::Raise {
            kind: "Exception",
            reason: "PyNumber_Add may raise while implementing Python addition",
        },
        cost: Cost {
            hot_path: 80,
            miss_path: 0,
            deopt: 0,
            materialization: 0,
            ownership: 4,
            code_size: 2,
            compile: 1,
        },
        rationale: "generic Python addition preserves Python dispatch and exception behavior",
    }
}

fn exact_compact_int_add() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("binary.add.exact_compact_int.i64"),
        op: SemanticOpKind::Binary { op: BinOpKind::Add },
        input_reps: vec![
            i64_from_compact_long_input(0),
            i64_from_compact_long_input(1),
        ],
        output_rep: Some(Rep::I64),
        required_facts: exact_compact_int_inputs(),
        output_facts: vec![ValueFact::I64, ValueFact::ExactCompactPyLong],
        emission: AlternativeEmission::Operation(PlannedOp::CheckedI64Add),
        guards: exact_compact_int_guards(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "overflow must use local fallback because the add has consumed unboxed operands",
        ),
        failure: AlternativeFailure::LocalFallback {
            reason: "checked i64 add overflow falls back to generic Python addition",
        },
        cost: Cost {
            hot_path: 2,
            miss_path: 85,
            deopt: 0,
            materialization: 0,
            ownership: 0,
            code_size: 6,
            compile: 2,
        },
        rationale: "exact compact PyLong operands can use checked i64 addition before materialization",
    }
}

fn generic_binary_subtract() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("binary.sub.py_generic"),
        op: SemanticOpKind::Binary { op: BinOpKind::Sub },
        input_reps: vec![pyobject_input(0), pyobject_input(1)],
        output_rep: Some(Rep::PyObjectOwned),
        required_facts: python_object_inputs(),
        output_facts: vec![ValueFact::PythonObject],
        emission: AlternativeEmission::Operation(PlannedOp::PyNumberSubtract),
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "PyNumber_Subtract raises or returns locally without replay",
        ),
        failure: AlternativeFailure::Raise {
            kind: "Exception",
            reason: "PyNumber_Subtract may raise while implementing Python subtraction",
        },
        cost: Cost {
            hot_path: 80,
            miss_path: 0,
            deopt: 0,
            materialization: 0,
            ownership: 4,
            code_size: 2,
            compile: 1,
        },
        rationale: "generic Python subtraction preserves Python dispatch and exception behavior",
    }
}

fn exact_compact_int_subtract() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("binary.sub.exact_compact_int.i64"),
        op: SemanticOpKind::Binary { op: BinOpKind::Sub },
        input_reps: vec![
            i64_from_compact_long_input(0),
            i64_from_compact_long_input(1),
        ],
        output_rep: Some(Rep::I64),
        required_facts: exact_compact_int_inputs(),
        output_facts: vec![ValueFact::I64, ValueFact::ExactCompactPyLong],
        emission: AlternativeEmission::Operation(PlannedOp::CheckedI64Sub),
        guards: exact_compact_int_guards(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "overflow must use local fallback because the subtraction has consumed unboxed operands",
        ),
        failure: AlternativeFailure::LocalFallback {
            reason: "checked i64 subtraction overflow falls back to generic Python subtraction",
        },
        cost: Cost {
            hot_path: 2,
            miss_path: 85,
            deopt: 0,
            materialization: 0,
            ownership: 0,
            code_size: 6,
            compile: 2,
        },
        rationale: "exact compact PyLong operands can use checked i64 subtraction before materialization",
    }
}

fn generic_binary_multiply() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("binary.mul.py_generic"),
        op: SemanticOpKind::Binary { op: BinOpKind::Mul },
        input_reps: vec![pyobject_input(0), pyobject_input(1)],
        output_rep: Some(Rep::PyObjectOwned),
        required_facts: python_object_inputs(),
        output_facts: vec![ValueFact::PythonObject],
        emission: AlternativeEmission::Operation(PlannedOp::PyNumberMultiply),
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "PyNumber_Multiply raises or returns locally without replay",
        ),
        failure: AlternativeFailure::Raise {
            kind: "Exception",
            reason: "PyNumber_Multiply may raise while implementing Python multiplication",
        },
        cost: Cost {
            hot_path: 80,
            miss_path: 0,
            deopt: 0,
            materialization: 0,
            ownership: 4,
            code_size: 2,
            compile: 1,
        },
        rationale: "generic Python multiplication preserves Python dispatch and exception behavior",
    }
}

fn exact_compact_int_multiply() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("binary.mul.exact_compact_int.i64"),
        op: SemanticOpKind::Binary { op: BinOpKind::Mul },
        input_reps: vec![
            i64_from_compact_long_input(0),
            i64_from_compact_long_input(1),
        ],
        output_rep: Some(Rep::I64),
        required_facts: exact_compact_int_inputs(),
        output_facts: vec![ValueFact::I64, ValueFact::ExactCompactPyLong],
        emission: AlternativeEmission::Operation(PlannedOp::CheckedI64Mul),
        guards: exact_compact_int_guards(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "overflow must use local fallback because the multiplication has consumed unboxed operands",
        ),
        failure: AlternativeFailure::LocalFallback {
            reason: "checked i64 multiplication overflow falls back to generic Python multiplication",
        },
        cost: Cost {
            hot_path: 3,
            miss_path: 85,
            deopt: 0,
            materialization: 0,
            ownership: 0,
            code_size: 6,
            compile: 2,
        },
        rationale: "exact compact PyLong operands can use checked i64 multiplication before materialization",
    }
}

fn generic_bitwise(
    id: &'static str,
    binop: BinOpKind,
    emission: PlannedOp,
    py_number_name: &'static str,
) -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new(id),
        op: SemanticOpKind::Binary { op: binop },
        input_reps: vec![pyobject_input(0), pyobject_input(1)],
        output_rep: Some(Rep::PyObjectOwned),
        required_facts: python_object_inputs(),
        output_facts: vec![ValueFact::PythonObject],
        emission: AlternativeEmission::Operation(emission),
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::local_fallback(format!(
            "{py_number_name} raises or returns locally without replay"
        )),
        failure: AlternativeFailure::Raise {
            kind: "Exception",
            reason: "generic Python bitwise operation may raise while preserving Python dispatch",
        },
        cost: Cost {
            hot_path: 80,
            miss_path: 0,
            deopt: 0,
            materialization: 0,
            ownership: 4,
            code_size: 2,
            compile: 1,
        },
        rationale: match binop {
            BinOpKind::And => {
                "generic Python bitwise and preserves Python dispatch and exception behavior"
            }
            BinOpKind::Or => {
                "generic Python bitwise or preserves Python dispatch and exception behavior"
            }
            BinOpKind::Xor => {
                "generic Python bitwise xor preserves Python dispatch and exception behavior"
            }
            _ => unreachable!("generic bitwise alternative only covers and/or/xor"),
        },
    }
}

fn exact_compact_int_bitwise(
    id: &'static str,
    binop: BinOpKind,
    emission: PlannedOp,
) -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new(id),
        op: SemanticOpKind::Binary { op: binop },
        input_reps: vec![
            i64_from_compact_long_input(0),
            i64_from_compact_long_input(1),
        ],
        output_rep: Some(Rep::I64),
        required_facts: exact_compact_int_inputs(),
        output_facts: vec![ValueFact::I64, ValueFact::ExactCompactPyLong],
        emission: AlternativeEmission::Operation(emission),
        guards: exact_compact_int_guards(),
        failure_replay: FailureReplayPolicy::safe(
            "i64 bitwise operations cannot fail after exact compact-int guards",
        ),
        failure: AlternativeFailure::CannotFail,
        cost: Cost {
            hot_path: 1,
            miss_path: 70,
            deopt: 0,
            materialization: 0,
            ownership: 0,
            code_size: 2,
            compile: 1,
        },
        rationale: match binop {
            BinOpKind::And => {
                "exact compact PyLong operands can use machine i64 bitwise and before materialization"
            }
            BinOpKind::Or => {
                "exact compact PyLong operands can use machine i64 bitwise or before materialization"
            }
            BinOpKind::Xor => {
                "exact compact PyLong operands can use machine i64 bitwise xor before materialization"
            }
            _ => unreachable!("exact bitwise alternative only covers and/or/xor"),
        },
    }
}

fn generic_rich_compare(
    id: &'static str,
    binop: BinOpKind,
    op: RichCompareOp,
) -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new(id),
        op: SemanticOpKind::Binary { op: binop },
        input_reps: vec![pyobject_input(0), pyobject_input(1)],
        output_rep: Some(Rep::PyObjectOwned),
        required_facts: vec![
            FactPredicate::Input {
                input: 0,
                fact: ValueFact::PythonObject,
            },
            FactPredicate::Input {
                input: 1,
                fact: ValueFact::PythonObject,
            },
        ],
        output_facts: vec![ValueFact::PythonObject],
        emission: AlternativeEmission::Operation(PlannedOp::PyObjectRichCompare { op }),
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "PyObject_RichCompare raises or returns locally without replay",
        ),
        failure: AlternativeFailure::Raise {
            kind: "Exception",
            reason: "PyObject_RichCompare may raise while implementing Python comparison",
        },
        cost: Cost {
            hot_path: 75,
            miss_path: 0,
            deopt: 0,
            materialization: 0,
            ownership: 4,
            code_size: 2,
            compile: 1,
        },
        rationale: "generic rich comparison preserves Python dispatch and comparison exceptions",
    }
}

fn exact_compact_int_compare(
    id: &'static str,
    binop: BinOpKind,
    op: RichCompareOp,
) -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new(id),
        op: SemanticOpKind::Binary { op: binop },
        input_reps: vec![
            i64_from_compact_long_input(0),
            i64_from_compact_long_input(1),
        ],
        output_rep: Some(Rep::I32Bool01),
        required_facts: exact_compact_int_inputs(),
        output_facts: vec![ValueFact::Bool01],
        emission: AlternativeEmission::Operation(PlannedOp::I64CompareToBool01 { op }),
        guards: exact_compact_int_guards(),
        failure_replay: FailureReplayPolicy::safe("i64 comparison cannot fail after input guards"),
        failure: AlternativeFailure::CannotFail,
        cost: Cost {
            hot_path: 1,
            miss_path: 75,
            deopt: 0,
            materialization: 0,
            ownership: 0,
            code_size: 3,
            compile: 1,
        },
        rationale: "exact compact PyLong comparison can produce a normalized branch boolean directly",
    }
}

fn python_truthiness() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("truthiness.pyobject"),
        op: SemanticOpKind::Truthiness,
        input_reps: vec![RepRequirement {
            input: 0,
            rep: Rep::PyObjectOwned,
            source: RepRequirementSource::AlreadyAvailable,
        }],
        output_rep: Some(Rep::I32Bool01),
        required_facts: vec![FactPredicate::Input {
            input: 0,
            fact: ValueFact::PythonObject,
        }],
        output_facts: vec![ValueFact::Bool01],
        emission: AlternativeEmission::Conversion(ConversionKind::TruthinessToI32Bool01),
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "PyObject_IsTrue raises locally and cannot be replayed after consuming owned input",
        ),
        failure: AlternativeFailure::Raise {
            kind: "Exception",
            reason: "PyObject_IsTrue may raise while implementing truthiness",
        },
        cost: Cost {
            hot_path: 28,
            miss_path: 0,
            deopt: 0,
            materialization: 0,
            ownership: 1,
            code_size: 2,
            compile: 1,
        },
        rationale: "truthiness of a Python object is a normal conversion node that handles errors locally",
    }
}

fn normalized_truthiness() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("truthiness.i32bool.identity"),
        op: SemanticOpKind::Truthiness,
        input_reps: vec![RepRequirement {
            input: 0,
            rep: Rep::I32Bool01,
            source: RepRequirementSource::AlreadyAvailable,
        }],
        output_rep: Some(Rep::I32Bool01),
        required_facts: vec![FactPredicate::Input {
            input: 0,
            fact: ValueFact::Bool01,
        }],
        output_facts: vec![ValueFact::Bool01],
        emission: AlternativeEmission::Identity,
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::safe("normalized boolean identity cannot fail"),
        failure: AlternativeFailure::CannotFail,
        cost: Cost {
            hot_path: 0,
            miss_path: 0,
            deopt: 0,
            materialization: 0,
            ownership: 0,
            code_size: 0,
            compile: 0,
        },
        rationale: "already-normalized branch booleans do not need a codegen operation",
    }
}

fn materialize_python_long() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("materialize.i64.python_long"),
        op: SemanticOpKind::Materialize {
            kind: MaterializeKind::PythonLong,
        },
        input_reps: vec![RepRequirement {
            input: 0,
            rep: Rep::I64,
            source: RepRequirementSource::AlreadyAvailable,
        }],
        output_rep: Some(Rep::PyObjectOwned),
        required_facts: vec![FactPredicate::Input {
            input: 0,
            fact: ValueFact::I64,
        }],
        output_facts: vec![ValueFact::PythonObject, ValueFact::ExactCompactPyLong],
        emission: AlternativeEmission::Materialize(MaterializeKind::PythonLong),
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::local_fallback(
            "PyLong materialization raises locally if allocation fails",
        ),
        failure: AlternativeFailure::Raise {
            kind: "MemoryError",
            reason: "materializing an i64 as PyLong may allocate",
        },
        cost: Cost {
            hot_path: 18,
            miss_path: 0,
            deopt: 0,
            materialization: 18,
            ownership: 1,
            code_size: 2,
            compile: 1,
        },
        rationale: "materialization is explicit when a Python object result is required",
    }
}

fn materialize_python_bool() -> LoweringAlternative {
    LoweringAlternative {
        id: AlternativeId::new("materialize.i32bool.python_bool"),
        op: SemanticOpKind::Materialize {
            kind: MaterializeKind::PythonBool,
        },
        input_reps: vec![RepRequirement {
            input: 0,
            rep: Rep::I32Bool01,
            source: RepRequirementSource::AlreadyAvailable,
        }],
        output_rep: Some(Rep::PyObjectImmortal),
        required_facts: vec![FactPredicate::Input {
            input: 0,
            fact: ValueFact::Bool01,
        }],
        output_facts: vec![ValueFact::PythonObject, ValueFact::PythonBoolObject],
        emission: AlternativeEmission::Materialize(MaterializeKind::PythonBool),
        guards: Vec::new(),
        failure_replay: FailureReplayPolicy::safe(
            "bool materialization selects an immortal object",
        ),
        failure: AlternativeFailure::CannotFail,
        cost: Cost {
            hot_path: 1,
            miss_path: 0,
            deopt: 0,
            materialization: 1,
            ownership: 0,
            code_size: 1,
            compile: 1,
        },
        rationale: "normalized booleans materialize as Py_True or Py_False without allocation",
    }
}

fn pyobject_input(input: u32) -> RepRequirement {
    RepRequirement {
        input,
        rep: Rep::PyObjectBorrowed,
        source: RepRequirementSource::AlreadyAvailable,
    }
}

fn python_object_inputs() -> Vec<FactPredicate> {
    vec![
        FactPredicate::Input {
            input: 0,
            fact: ValueFact::PythonObject,
        },
        FactPredicate::Input {
            input: 1,
            fact: ValueFact::PythonObject,
        },
    ]
}

fn i64_from_compact_long_input(input: u32) -> RepRequirement {
    RepRequirement {
        input,
        rep: Rep::I64,
        source: RepRequirementSource::Conversion {
            kind: ConversionKind::FromPythonLongCompactToI64,
        },
    }
}

fn exact_compact_int_inputs() -> Vec<FactPredicate> {
    vec![
        FactPredicate::Input {
            input: 0,
            fact: ValueFact::ExactCompactPyLong,
        },
        FactPredicate::Input {
            input: 1,
            fact: ValueFact::ExactCompactPyLong,
        },
    ]
}

fn exact_compact_int_guards() -> Vec<GuardRequirement> {
    vec![exact_compact_int_guard(0), exact_compact_int_guard(1)]
}

fn exact_compact_int_guard(input: u32) -> GuardRequirement {
    GuardRequirement {
        input,
        kind: GuardRequirementKind::ExactCompactPyLong,
        description: "exact compact PyLong specialization guard",
        replay: FailureReplayPolicy::local_fallback(
            "fallback region reuses original Python object operands",
        ),
        miss: GuardMiss::LocalFallback {
            reason: "exact compact PyLong guard miss uses generic Python fallback",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_v3::{
        ConversionOwnership, ConversionPrecondition, ConvertNode, MaterializeNode,
        ModuleOptimizationPlanV3, ModulePlanIdentity, PlanNode, PlanNodeId, PlanNodeKind,
        PlanValue, PlannedConstant, RegionExitKind, RegionExitPlan, RegionExitTarget, RegionId,
        RegionInput, RegionInputSource, RegionPlan, RegionSource, validate_module_plan_v3,
    };
    use soac_core::block_py::{
        BlockLabel, InstrId, LocalFunctionId, SerializedFunctionId, SerializedIdentityTables,
        SerializedModuleId, SerializedModuleIdentity,
    };

    fn test_function_id() -> SerializedFunctionId {
        SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2))
    }

    fn instr_id(index: u32) -> InstrId {
        InstrId::new(BlockLabel::from_index(0), index)
    }

    fn input(value: PlanValue, index: u32, name: &str) -> RegionInput {
        RegionInput {
            value,
            source: RegionInputSource::FunctionParam {
                index,
                name: Some(name.to_string()),
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

    #[test]
    fn default_catalog_validates() {
        let catalog = AlternativeCatalog::default_v3();
        validate_alternative_catalog_v3(&catalog).unwrap();
    }

    #[test]
    fn catalog_exposes_generic_and_exact_binary_return_alternatives() {
        let catalog = AlternativeCatalog::default_v3();
        for (kind, generic_id, exact_id) in [
            (
                BinOpKind::Add,
                "binary.add.py_generic",
                "binary.add.exact_compact_int.i64",
            ),
            (
                BinOpKind::Sub,
                "binary.sub.py_generic",
                "binary.sub.exact_compact_int.i64",
            ),
            (
                BinOpKind::Mul,
                "binary.mul.py_generic",
                "binary.mul.exact_compact_int.i64",
            ),
            (
                BinOpKind::And,
                "binary.and.py_generic",
                "binary.and.exact_compact_int.i64",
            ),
            (
                BinOpKind::Or,
                "binary.or.py_generic",
                "binary.or.exact_compact_int.i64",
            ),
            (
                BinOpKind::Xor,
                "binary.xor.py_generic",
                "binary.xor.exact_compact_int.i64",
            ),
        ] {
            let alternatives = catalog
                .alternatives_for(SemanticOpKind::Binary { op: kind })
                .map(|alternative| alternative.id)
                .collect::<Vec<_>>();
            assert_eq!(
                alternatives,
                vec![AlternativeId::new(generic_id), AlternativeId::new(exact_id)],
                "{kind:?}",
            );
        }
    }

    #[test]
    fn catalog_exposes_all_rich_compare_alternatives() {
        let catalog = AlternativeCatalog::default_v3();
        for (kind, generic_id, exact_id) in [
            (
                BinOpKind::Eq,
                "binary.eq.py_richcompare",
                "binary.eq.exact_compact_int.i32bool",
            ),
            (
                BinOpKind::Ne,
                "binary.ne.py_richcompare",
                "binary.ne.exact_compact_int.i32bool",
            ),
            (
                BinOpKind::Lt,
                "binary.lt.py_richcompare",
                "binary.lt.exact_compact_int.i32bool",
            ),
            (
                BinOpKind::Le,
                "binary.le.py_richcompare",
                "binary.le.exact_compact_int.i32bool",
            ),
            (
                BinOpKind::Gt,
                "binary.gt.py_richcompare",
                "binary.gt.exact_compact_int.i32bool",
            ),
            (
                BinOpKind::Ge,
                "binary.ge.py_richcompare",
                "binary.ge.exact_compact_int.i32bool",
            ),
        ] {
            assert_eq!(
                catalog
                    .by_id(AlternativeId::new(generic_id))
                    .map(|alt| alt.op),
                Some(SemanticOpKind::Binary { op: kind }),
                "missing generic rich-compare alternative {generic_id}",
            );
            assert_eq!(
                catalog
                    .by_id(AlternativeId::new(exact_id))
                    .map(|alt| alt.op),
                Some(SemanticOpKind::Binary { op: kind }),
                "missing exact compact-int rich-compare alternative {exact_id}",
            );
        }
    }

    #[test]
    fn duplicate_alternative_id_fails_validation() {
        let mut catalog = AlternativeCatalog::default_v3();
        let duplicate = catalog.alternatives[0].clone();
        catalog.alternatives.push(duplicate);
        let err = validate_alternative_catalog_v3(&catalog).unwrap_err();
        assert!(err.contains("duplicate alternative id"), "{err}");
    }

    #[test]
    fn conversion_requirement_must_match_signature() {
        let mut catalog = AlternativeCatalog::default_v3();
        let alternative = catalog
            .by_id(AlternativeId::new("binary.add.exact_compact_int.i64"))
            .unwrap()
            .clone();
        let mut broken = alternative;
        broken.input_reps[0].source = RepRequirementSource::Conversion {
            kind: ConversionKind::ToPythonLongOwned,
        };
        catalog.alternatives = vec![broken];
        let err = validate_alternative_catalog_v3(&catalog).unwrap_err();
        assert!(
            err.contains("produces PyObjectOwned, not required I64"),
            "{err}"
        );
    }

    #[test]
    fn catalog_built_compact_int_branch_plan_validates() {
        let catalog = AlternativeCatalog::default_v3();
        let plan = compact_int_branch_plan_from_catalog(&catalog);
        validate_module_plan_v3(&plan).unwrap();
    }

    fn compact_int_branch_plan_from_catalog(
        catalog: &AlternativeCatalog,
    ) -> ModuleOptimizationPlanV3 {
        let add = catalog
            .by_id(AlternativeId::new("binary.add.exact_compact_int.i64"))
            .unwrap();
        let compare = catalog
            .by_id(AlternativeId::new("binary.gt.exact_compact_int.i32bool"))
            .unwrap();
        let generic_add = catalog
            .by_id(AlternativeId::new("binary.add.py_generic"))
            .unwrap();
        let generic_compare = catalog
            .by_id(AlternativeId::new("binary.gt.py_richcompare"))
            .unwrap();
        let truthiness = catalog
            .by_id(AlternativeId::new("truthiness.pyobject"))
            .unwrap();
        let materialize_long = catalog
            .by_id(AlternativeId::new("materialize.i64.python_long"))
            .unwrap();

        let a_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
        let b_obj = PlanValue::new(1, Rep::PyObjectBorrowed);
        let a_i64 = PlanValue::new(2, Rep::I64);
        let b_i64 = PlanValue::new(3, Rep::I64);
        let sum_i64 = PlanValue::new(4, Rep::I64);
        let zero_i64 = PlanValue::new(5, Rep::I64);
        let condition = PlanValue::new(6, Rep::I32Bool01);
        let fallback_target = FailureTargets::local_fallback(FallbackTarget::Region(RegionId(1)));

        let hot_region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![input(a_obj, 0, "a"), input(b_obj, 1, "b")],
            nodes: vec![
                exact_compact_guard_node(add, 0, 0, a_obj, &fallback_target),
                exact_compact_guard_node(add, 1, 1, b_obj, &fallback_target),
                unbox_node(2, a_obj, a_i64, 0),
                unbox_node(3, b_obj, b_i64, 1),
                node(
                    4,
                    PlanNodeKind::Operation(crate::plan_v3::OperationNode {
                        op: add.planned_operation().unwrap(),
                        inputs: vec![a_i64, b_i64],
                        output: Some(sum_i64),
                        failure_replay: add.failure_replay.clone(),
                        failure: add.instantiate_failure(&fallback_target).unwrap(),
                        cost: add.cost,
                    }),
                ),
                node(
                    5,
                    PlanNodeKind::Constant {
                        output: zero_i64,
                        constant: PlannedConstant::I64(0),
                    },
                ),
                node(
                    6,
                    PlanNodeKind::Operation(crate::plan_v3::OperationNode {
                        op: compare.planned_operation().unwrap(),
                        inputs: vec![sum_i64, zero_i64],
                        output: Some(condition),
                        failure_replay: compare.failure_replay.clone(),
                        failure: compare.instantiate_failure(&fallback_target).unwrap(),
                        cost: compare.cost,
                    }),
                ),
            ],
            exits: vec![RegionExitPlan {
                source: Some(instr_id(3)),
                kind: RegionExitKind::Branch {
                    condition,
                    then_target: RegionExitTarget::OriginalCfg,
                    else_target: RegionExitTarget::OriginalCfg,
                },
            }],
        };

        let fallback_sum = PlanValue::new(10, Rep::PyObjectOwned);
        let fallback_zero_i64 = PlanValue::new(11, Rep::I64);
        let fallback_zero_obj = PlanValue::new(12, Rep::PyObjectOwned);
        let fallback_compare = PlanValue::new(13, Rep::PyObjectOwned);
        let fallback_condition = PlanValue::new(14, Rep::I32Bool01);
        let fallback_region = RegionPlan {
            id: RegionId(1),
            source: RegionSource::Synthetic {
                reason: "generic fallback for a + b > 0".to_string(),
            },
            inputs: vec![input(a_obj, 0, "a"), input(b_obj, 1, "b")],
            nodes: vec![
                node(
                    10,
                    PlanNodeKind::Operation(crate::plan_v3::OperationNode {
                        op: generic_add.planned_operation().unwrap(),
                        inputs: vec![a_obj, b_obj],
                        output: Some(fallback_sum),
                        failure_replay: generic_add.failure_replay.clone(),
                        failure: generic_add.instantiate_failure(&fallback_target).unwrap(),
                        cost: generic_add.cost,
                    }),
                ),
                node(
                    11,
                    PlanNodeKind::Constant {
                        output: fallback_zero_i64,
                        constant: PlannedConstant::I64(0),
                    },
                ),
                node(
                    12,
                    PlanNodeKind::Materialize(MaterializeNode {
                        input: fallback_zero_i64,
                        output: fallback_zero_obj,
                        kind: MaterializeKind::PythonLong,
                    }),
                ),
                node(
                    13,
                    PlanNodeKind::Operation(crate::plan_v3::OperationNode {
                        op: generic_compare.planned_operation().unwrap(),
                        inputs: vec![fallback_sum, fallback_zero_obj],
                        output: Some(fallback_compare),
                        failure_replay: generic_compare.failure_replay.clone(),
                        failure: generic_compare
                            .instantiate_failure(&fallback_target)
                            .unwrap(),
                        cost: generic_compare.cost,
                    }),
                ),
                node(
                    14,
                    PlanNodeKind::Convert(ConvertNode {
                        input: fallback_compare,
                        output: fallback_condition,
                        kind: ConversionKind::TruthinessToI32Bool01,
                        precondition: ConversionPrecondition::Infallible,
                        failure: truthiness.instantiate_failure(&fallback_target).unwrap(),
                        ownership: ConversionOwnership::ConsumeOwned,
                    }),
                ),
            ],
            exits: vec![RegionExitPlan {
                source: Some(instr_id(3)),
                kind: RegionExitKind::Branch {
                    condition: fallback_condition,
                    then_target: RegionExitTarget::OriginalCfg,
                    else_target: RegionExitTarget::OriginalCfg,
                },
            }],
        };

        assert_eq!(
            materialize_long.emission,
            AlternativeEmission::Materialize(MaterializeKind::PythonLong)
        );

        ModuleOptimizationPlanV3 {
            module: ModulePlanIdentity {
                module_name: "pkg.mod".to_string(),
                source_hash: 0x4321,
                cache_identity: "test-cache".to_string(),
            },
            identity_tables: SerializedIdentityTables {
                modules: vec![SerializedModuleIdentity {
                    module_name: "pkg.mod".to_string(),
                    source_hash: 0x4321,
                    cache_identity: Some("test-cache".to_string()),
                }],
                debug_names: Vec::new(),
            },
            helper_catalog_version: ALTERNATIVE_CATALOG_V3_VERSION,
            cost_model_version: 1,
            functions: vec![crate::plan_v3::FunctionOptimizationPlanV3 {
                function: crate::plan_v3::FunctionPlanIdentity {
                    function: test_function_id(),
                    debug_name: Some("f".to_string()),
                },
                regions: vec![hot_region, fallback_region],
                scalar_threads: Vec::new(),
                direct_calls: Vec::new(),
                exact_list_items: Vec::new(),
                indexed_fields: Vec::new(),
                indexed_globals: Vec::new(),
                deopt_points: Vec::new(),
                ownership: crate::plan_v3::FunctionOwnershipPlan::default(),
                diagnostics: Vec::new(),
            }],
        }
    }

    fn exact_compact_guard_node(
        alternative: &LoweringAlternative,
        guard_index: usize,
        node_id: u32,
        input_value: PlanValue,
        target: &FailureTargets,
    ) -> PlanNode {
        let mut guard = alternative.guards[guard_index].instantiate(target).unwrap();
        guard.inputs = vec![input_value];
        node(node_id, PlanNodeKind::Guard(guard))
    }

    fn unbox_node(id: u32, input: PlanValue, output: PlanValue, guard: u32) -> PlanNode {
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
                    target: FallbackTarget::Region(RegionId(1)),
                    reason: FallbackReason("conversion miss uses generic fallback".to_string()),
                },
                ownership: ConversionOwnership::BorrowInput,
            }),
        )
    }
}
