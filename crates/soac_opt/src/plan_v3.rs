use soac_core::block_py::{
    InstrId, SerializedFunctionId, SerializedIdentityTables, SerializedModuleId,
};
use std::collections::{HashMap, HashSet};
use std::fmt;

pub const EXACT_LIST_EXACT_INT_ITEM_SHAPE_TAG: u64 = 1;

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ModuleOptimizationPlanV3 {
    pub module: ModulePlanIdentity,
    pub identity_tables: SerializedIdentityTables,
    pub helper_catalog_version: u32,
    pub cost_model_version: u32,
    pub functions: Vec<FunctionOptimizationPlanV3>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ModulePlanIdentity {
    pub module_name: String,
    pub source_hash: u64,
    pub cache_identity: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FunctionOptimizationPlanV3 {
    pub function: FunctionPlanIdentity,
    pub regions: Vec<RegionPlan>,
    pub scalar_threads: Vec<ScalarLocalThreadPlan>,
    pub direct_calls: Vec<DirectCallSpecializationPlan>,
    pub constructor_calls: Vec<ConstructorCallSpecializationPlan>,
    pub method_calls: Vec<MethodCallSpecializationPlan>,
    pub exact_list_items: Vec<ExactListItemSpecializationPlan>,
    pub indexed_fields: Vec<IndexedFieldSpecializationPlan>,
    pub indexed_globals: Vec<IndexedGlobalSpecializationPlan>,
    pub deopt_points: Vec<PlannedDeoptPoint>,
    pub ownership: FunctionOwnershipPlan,
    pub diagnostics: Vec<PlanDiagnostic>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FunctionPlanIdentity {
    pub function: SerializedFunctionId,
    pub debug_name: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct DirectCallSpecializationPlan {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub arg_plan: DirectCallArgPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct DirectCallArgPlan {
    pub sources: Vec<DirectCallArgSource>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum DirectCallArgSource {
    Provided(u32),
    DefaultSentinel,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConstructorCallSpecializationPlan {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub owner_type: ConstructorCallOwnerType,
    pub arg_plan: DirectCallArgPlan,
    pub guard: ConstructorCallGuardPlan,
    pub fallback: ConstructorCallFallbackPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConstructorCallOwnerType {
    pub module_name: String,
    pub qualname: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConstructorCallGuardPlan {
    pub kind: ConstructorCallGuardKind,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum ConstructorCallGuardKind {
    ExactCallableTypeVersion,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConstructorCallFallbackPlan {
    pub kind: ConstructorCallFallbackKind,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum ConstructorCallFallbackKind {
    OriginalConstructorCall,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MethodCallSpecializationPlan {
    pub source: InstrId,
    pub target: SerializedFunctionId,
    pub method_name: String,
    pub owner_type: MethodCallOwnerType,
    pub arg_plan: DirectCallArgPlan,
    pub guard: MethodCallGuardPlan,
    pub fallback: MethodCallFallbackPlan,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MethodCallOwnerType {
    pub module_name: String,
    pub qualname: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MethodCallGuardPlan {
    pub kind: MethodCallGuardKind,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum MethodCallGuardKind {
    ExactReceiverTypeVersion,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MethodCallFallbackPlan {
    pub kind: MethodCallFallbackKind,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum MethodCallFallbackKind {
    OriginalMethodCall,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExactListItemSpecializationPlan {
    pub source: InstrId,
    pub access: ExactListItemAccessKind,
    pub shape: ExactListItemShape,
    pub guard: ExactListItemGuardPlan,
    pub fallback: ExactListItemFallbackPlan,
    pub reason: String,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum ExactListItemAccessKind {
    Get,
    Set,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum ExactListItemShape {
    ExactListExactInt,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExactListItemGuardPlan {
    pub kind: ExactListItemGuardKind,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum ExactListItemGuardKind {
    ExactListExactCompactIntInBounds,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ExactListItemFallbackPlan {
    pub kind: ExactListItemFallbackKind,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum ExactListItemFallbackKind {
    OriginalItemAccess,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct IndexedFieldSpecializationPlan {
    pub source: InstrId,
    pub access: IndexedFieldAccessKind,
    pub owner_type: IndexedFieldOwnerType,
    pub attr_name: String,
    pub expected_index: u32,
    pub reason: String,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum IndexedFieldAccessKind {
    Load,
    Store,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct IndexedFieldOwnerType {
    pub module_name: String,
    pub qualname: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct IndexedGlobalSpecializationPlan {
    pub source: InstrId,
    pub access: IndexedGlobalAccessKind,
    pub module_name: String,
    pub name: String,
    pub expected_index: u32,
    pub guard: IndexedGlobalGuardPlan,
    pub fallback: IndexedGlobalFallbackPlan,
    pub reason: String,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub enum IndexedGlobalAccessKind {
    Load,
    Store,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct IndexedGlobalGuardPlan {
    pub kind: IndexedGlobalGuardKind,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum IndexedGlobalGuardKind {
    ModuleDictKeyAtIndex,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct IndexedGlobalFallbackPlan {
    pub kind: IndexedGlobalFallbackKind,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum IndexedGlobalFallbackKind {
    OriginalGlobalAccess,
}

#[derive(
    Clone, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct FunctionOwnershipPlan {
    pub actions: Vec<OwnershipAction>,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ScalarLocalThreadPlan {
    pub local: ScalarThreadLocal,
    pub producer: RegionValueRef,
    pub consumer: RegionValueRef,
    pub fallback: ScalarThreadFallback,
    pub local_state: ScalarThreadLocalState,
    pub materialization: ScalarThreadMaterialization,
    pub estimated_savings: Cost,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ScalarThreadLocal {
    pub name: String,
    pub location: ScalarThreadLocalLocation,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ScalarThreadLocalLocation {
    Local { slot: u32 },
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct RegionValueRef {
    pub region: RegionId,
    pub value: PlanValue,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ScalarThreadFallback {
    LocalFallbackRegion { region: RegionId, reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ScalarThreadLocalState {
    ScalarOnlyHotPath {
        cleanup: ScalarThreadLocalCleanup,
        reason: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ScalarThreadLocalCleanup {
    NoPyObjectSlotOwnership,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ScalarThreadMaterialization {
    DeferredUntilPythonObjectUse { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RegionPlan {
    pub id: RegionId,
    pub source: RegionSource,
    pub inputs: Vec<RegionInput>,
    pub nodes: Vec<PlanNode>,
    pub exits: Vec<RegionExitPlan>,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct RegionId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RegionSource {
    FunctionEntry,
    Instr { instr_id: InstrId },
    Synthetic { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RegionInput {
    pub value: PlanValue,
    pub source: RegionInputSource,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RegionInputSource {
    FunctionParam {
        index: u32,
        name: Option<String>,
    },
    CapturedValue {
        from_region: RegionId,
        value: PlanValueId,
    },
    Synthetic {
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct RegionExitPlan {
    pub source: Option<InstrId>,
    pub kind: RegionExitKind,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RegionExitKind {
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

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RegionExitTarget {
    Region(RegionId),
    OriginalCfg,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PlanNode {
    pub id: PlanNodeId,
    pub source: Option<InstrId>,
    pub kind: PlanNodeKind,
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct PlanNodeId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PlanNodeKind {
    Input {
        output: PlanValue,
    },
    Constant {
        output: PlanValue,
        constant: PlannedConstant,
    },
    Convert(ConvertNode),
    Guard(GuardNode),
    Operation(OperationNode),
    Materialize(MaterializeNode),
    Fallback {
        target: FallbackTarget,
    },
    Deopt {
        target: DeoptPointId,
    },
    Ownership {
        action: OwnershipAction,
    },
}

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct PlanValueId(pub u32);

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct PlanValue {
    pub id: PlanValueId,
    pub rep: Rep,
}

impl PlanValue {
    pub const fn new(id: u32, rep: Rep) -> Self {
        Self {
            id: PlanValueId(id),
            rep,
        }
    }
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum Rep {
    PyObjectOwned,
    PyObjectBorrowed,
    PyObjectImmortal,
    I32Bool01,
    I64,
}

impl Rep {
    pub const fn is_python_object(self) -> bool {
        matches!(
            self,
            Self::PyObjectOwned | Self::PyObjectBorrowed | Self::PyObjectImmortal
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PlannedConstant {
    I64(i64),
    Bool(bool),
    RuntimeName(String),
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ConvertNode {
    pub input: PlanValue,
    pub output: PlanValue,
    pub kind: ConversionKind,
    pub precondition: ConversionPrecondition,
    pub failure: FailureMode,
    pub ownership: ConversionOwnership,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub enum ConversionKind {
    FromPythonLongCompactToI64,
    ToPythonLongOwned,
    ToPythonBoolImmortal,
    TruthinessToI32Bool01,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ConversionPrecondition {
    Infallible,
    DominatingFacts { reason: String },
    SpecializationGuard { guard: PlanNodeId, reason: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum ConversionOwnership {
    Preserve,
    MaterializeOwned,
    BorrowInput,
    ConsumeOwned,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct GuardNode {
    pub inputs: Vec<PlanValue>,
    pub guard: GuardSpec,
    pub failure: GuardFailure,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct GuardSpec {
    pub kind: GuardKind,
    pub replay: FailureReplayPolicy,
    pub description: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum GuardKind {
    SpecializationCheck,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum GuardFailure {
    FallbackToPlan {
        target: FallbackTarget,
        reason: FallbackReason,
    },
    DeoptTo {
        target: DeoptPointId,
        reason: DeoptReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct OperationNode {
    pub op: PlannedOp,
    pub inputs: Vec<PlanValue>,
    pub output: Option<PlanValue>,
    pub failure_replay: FailureReplayPolicy,
    pub failure: FailureMode,
    pub cost: Cost,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum PlannedOp {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum RichCompareOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct MaterializeNode {
    pub input: PlanValue,
    pub output: PlanValue,
    pub kind: MaterializeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum MaterializeKind {
    PythonLong,
    PythonBool,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum FailureMode {
    CannotFail,
    Raise(PythonExceptionSpec),
    FallbackToPlan {
        target: FallbackTarget,
        reason: FallbackReason,
    },
    DeoptTo {
        target: DeoptPointId,
        reason: DeoptReason,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum FallbackTarget {
    Region(RegionId),
    Node(PlanNodeId),
    OriginalInstruction { instr_id: InstrId },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PythonExceptionSpec {
    pub kind: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FallbackReason(pub String);

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct DeoptReason(pub String);

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct FailureReplayPolicy {
    pub replay: FailureReplayKind,
    pub reason: ReplayReason,
}

impl FailureReplayPolicy {
    pub fn safe(reason: impl Into<String>) -> Self {
        Self {
            replay: FailureReplayKind::SafeToReplay,
            reason: ReplayReason(reason.into()),
        }
    }

    pub fn local_fallback(reason: impl Into<String>) -> Self {
        Self {
            replay: FailureReplayKind::MustUseLocalFallback,
            reason: ReplayReason(reason.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum FailureReplayKind {
    SafeToReplay,
    MustUseLocalFallback,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct ReplayReason(pub String);

#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    rkyv::Archive,
    rkyv::Serialize,
    rkyv::Deserialize,
)]
pub struct DeoptPointId(pub u32);

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PlannedDeoptPoint {
    pub id: DeoptPointId,
    pub source: DeoptPointSource,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum DeoptPointSource {
    BeforeInstr { instr_id: InstrId },
    BeforeRegion { region: RegionId },
    Synthetic { reason: String },
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct OwnershipAction {
    pub value: PlanValue,
    pub kind: OwnershipActionKind,
    pub reason: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub enum OwnershipActionKind {
    Incref,
    Decref,
    TransferOwned,
    BorrowLocal,
    MaterializeOwned,
}

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize,
)]
pub struct Cost {
    pub hot_path: u32,
    pub miss_path: u32,
    pub deopt: u32,
    pub materialization: u32,
    pub ownership: u32,
    pub code_size: u32,
    pub compile: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)]
pub struct PlanDiagnostic {
    pub source: Option<InstrId>,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanValidationError {
    pub errors: Vec<String>,
}

impl PlanValidationError {
    pub fn contains(&self, needle: &str) -> bool {
        self.errors.iter().any(|error| error.contains(needle))
    }
}

impl fmt::Display for PlanValidationError {
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

impl std::error::Error for PlanValidationError {}

pub fn validate_module_plan_v3(plan: &ModuleOptimizationPlanV3) -> Result<(), PlanValidationError> {
    let mut errors = Vec::new();
    if plan.module.module_name.is_empty() {
        errors.push("module identity has empty module name".to_string());
    }
    if plan
        .identity_tables
        .module(SerializedModuleId::new(0))
        .is_err()
    {
        errors.push("module identity table is missing current module id 0".to_string());
    }
    for function in &plan.functions {
        validate_function_plan(function, &plan.identity_tables, &mut errors);
    }
    finish_validation(errors)
}

fn validate_function_plan(
    function: &FunctionOptimizationPlanV3,
    identity_tables: &SerializedIdentityTables,
    errors: &mut Vec<String>,
) {
    if identity_tables
        .module(function.function.function.module_id())
        .is_err()
    {
        errors.push(format!(
            "function {} references missing module id {}",
            function.function.function,
            function.function.function.module_id()
        ));
    }
    let mut region_ids = HashSet::new();
    for region in &function.regions {
        if !region_ids.insert(region.id) {
            errors.push(format!(
                "function {} has duplicate region {:?}",
                function.function.function, region.id
            ));
        }
    }
    let region_positions = function
        .regions
        .iter()
        .enumerate()
        .map(|(index, region)| (region.id, index))
        .collect::<HashMap<_, _>>();
    let deopt_points = function
        .deopt_points
        .iter()
        .map(|point| point.id)
        .collect::<HashSet<_>>();

    for region in &function.regions {
        validate_region_plan(region, &region_ids, &deopt_points, errors);
    }
    let region_values = collect_region_value_reps(&function.regions);
    for region in &function.regions {
        for input in &region.inputs {
            validate_region_input_source(region.id, input, &region_ids, &region_values, errors);
        }
    }
    let mut threaded_consumers = HashSet::<RegionValueRef>::new();
    for thread in &function.scalar_threads {
        validate_scalar_thread_plan(
            function,
            thread,
            &region_ids,
            &region_positions,
            &region_values,
            &mut threaded_consumers,
            errors,
        );
    }
    validate_direct_call_plans(function, identity_tables, errors);
    validate_constructor_call_plans(function, identity_tables, errors);
    validate_method_call_plans(function, identity_tables, errors);
    validate_exact_list_item_plans(function, errors);
    validate_indexed_field_plans(function, errors);
    validate_indexed_global_plans(function, errors);
    for action in &function.ownership.actions {
        if action.reason.is_empty() {
            errors.push(format!(
                "function {} has ownership action for {:?} without reason",
                function.function.function, action.value
            ));
        }
    }
}

fn validate_direct_call_plans(
    function: &FunctionOptimizationPlanV3,
    identity_tables: &SerializedIdentityTables,
    errors: &mut Vec<String>,
) {
    let mut seen = HashSet::new();
    for direct_call in &function.direct_calls {
        if !seen.insert((direct_call.source, direct_call.target)) {
            errors.push(format!(
                "function {} has duplicate direct-call target {} at {}",
                function.function.function, direct_call.target, direct_call.source
            ));
        }
        if direct_call.reason.is_empty() {
            errors.push(format!(
                "function {} direct-call target {} at {} has empty reason",
                function.function.function, direct_call.target, direct_call.source
            ));
        }
        if identity_tables
            .module(direct_call.target.module_id())
            .is_err()
        {
            errors.push(format!(
                "function {} direct-call target {} references missing module id {}",
                function.function.function,
                direct_call.target,
                direct_call.target.module_id()
            ));
        }
        validate_direct_call_arg_plan(
            function,
            "direct-call",
            direct_call.target,
            direct_call.source,
            &direct_call.arg_plan,
            errors,
        );
    }
}

fn validate_constructor_call_plans(
    function: &FunctionOptimizationPlanV3,
    identity_tables: &SerializedIdentityTables,
    errors: &mut Vec<String>,
) {
    let mut seen = HashSet::new();
    for constructor_call in &function.constructor_calls {
        if !seen.insert((
            constructor_call.source,
            constructor_call.target,
            constructor_call.owner_type.clone(),
        )) {
            errors.push(format!(
                "function {} has duplicate constructor-call target {} {}.{} at {}",
                function.function.function,
                constructor_call.target,
                constructor_call.owner_type.module_name,
                constructor_call.owner_type.qualname,
                constructor_call.source
            ));
        }
        if constructor_call.reason.is_empty() {
            errors.push(format!(
                "function {} constructor-call target {} at {} has empty reason",
                function.function.function, constructor_call.target, constructor_call.source
            ));
        }
        if identity_tables
            .module(constructor_call.target.module_id())
            .is_err()
        {
            errors.push(format!(
                "function {} constructor-call target {} references missing module id {}",
                function.function.function,
                constructor_call.target,
                constructor_call.target.module_id()
            ));
        }
        if constructor_call.owner_type.module_name.is_empty() {
            errors.push(format!(
                "function {} constructor-call target {} at {} has empty owner module",
                function.function.function, constructor_call.target, constructor_call.source
            ));
        }
        if constructor_call.owner_type.qualname.is_empty() {
            errors.push(format!(
                "function {} constructor-call target {} at {} has empty owner qualname",
                function.function.function, constructor_call.target, constructor_call.source
            ));
        }
        if constructor_call.guard.kind != ConstructorCallGuardKind::ExactCallableTypeVersion {
            errors.push(format!(
                "function {} constructor-call target {} at {} has unsupported guard {:?}",
                function.function.function,
                constructor_call.target,
                constructor_call.source,
                constructor_call.guard.kind
            ));
        }
        if constructor_call.fallback.kind != ConstructorCallFallbackKind::OriginalConstructorCall {
            errors.push(format!(
                "function {} constructor-call target {} at {} has unsupported fallback {:?}",
                function.function.function,
                constructor_call.target,
                constructor_call.source,
                constructor_call.fallback.kind
            ));
        }
        validate_direct_call_arg_plan(
            function,
            "constructor-call",
            constructor_call.target,
            constructor_call.source,
            &constructor_call.arg_plan,
            errors,
        );
    }
}

fn validate_method_call_plans(
    function: &FunctionOptimizationPlanV3,
    identity_tables: &SerializedIdentityTables,
    errors: &mut Vec<String>,
) {
    let mut seen = HashSet::new();
    for method_call in &function.method_calls {
        if !seen.insert((
            method_call.source,
            method_call.target,
            method_call.method_name.clone(),
            method_call.owner_type.clone(),
        )) {
            errors.push(format!(
                "function {} has duplicate method-call target {} {}.{} method={} at {}",
                function.function.function,
                method_call.target,
                method_call.owner_type.module_name,
                method_call.owner_type.qualname,
                method_call.method_name,
                method_call.source
            ));
        }
        if method_call.reason.is_empty() {
            errors.push(format!(
                "function {} method-call target {} at {} has empty reason",
                function.function.function, method_call.target, method_call.source
            ));
        }
        if identity_tables
            .module(method_call.target.module_id())
            .is_err()
        {
            errors.push(format!(
                "function {} method-call target {} references missing module id {}",
                function.function.function,
                method_call.target,
                method_call.target.module_id()
            ));
        }
        if method_call.method_name.is_empty() {
            errors.push(format!(
                "function {} method-call target {} at {} has empty method name",
                function.function.function, method_call.target, method_call.source
            ));
        }
        if method_call.owner_type.module_name.is_empty() {
            errors.push(format!(
                "function {} method-call target {} at {} has empty owner module",
                function.function.function, method_call.target, method_call.source
            ));
        }
        if method_call.owner_type.qualname.is_empty() {
            errors.push(format!(
                "function {} method-call target {} at {} has empty owner qualname",
                function.function.function, method_call.target, method_call.source
            ));
        }
        if method_call.guard.kind != MethodCallGuardKind::ExactReceiverTypeVersion {
            errors.push(format!(
                "function {} method-call target {} at {} has unsupported guard {:?}",
                function.function.function,
                method_call.target,
                method_call.source,
                method_call.guard.kind
            ));
        }
        if method_call.fallback.kind != MethodCallFallbackKind::OriginalMethodCall {
            errors.push(format!(
                "function {} method-call target {} at {} has unsupported fallback {:?}",
                function.function.function,
                method_call.target,
                method_call.source,
                method_call.fallback.kind
            ));
        }
        validate_direct_call_arg_plan(
            function,
            "method-call",
            method_call.target,
            method_call.source,
            &method_call.arg_plan,
            errors,
        );
    }
}

fn validate_exact_list_item_plans(function: &FunctionOptimizationPlanV3, errors: &mut Vec<String>) {
    let mut seen = HashSet::new();
    for item in &function.exact_list_items {
        if !seen.insert((item.source, item.access, item.shape)) {
            errors.push(format!(
                "function {} has duplicate exact-list item {:?} {:?} at {}",
                function.function.function, item.access, item.shape, item.source
            ));
        }
        if item.guard.kind != ExactListItemGuardKind::ExactListExactCompactIntInBounds {
            errors.push(format!(
                "function {} exact-list item at {} has unsupported guard {:?}",
                function.function.function, item.source, item.guard.kind
            ));
        }
        if item.fallback.kind != ExactListItemFallbackKind::OriginalItemAccess {
            errors.push(format!(
                "function {} exact-list item at {} has unsupported fallback {:?}",
                function.function.function, item.source, item.fallback.kind
            ));
        }
        if item.reason.is_empty() {
            errors.push(format!(
                "function {} exact-list item at {} has empty reason",
                function.function.function, item.source
            ));
        }
    }
}

fn validate_indexed_field_plans(function: &FunctionOptimizationPlanV3, errors: &mut Vec<String>) {
    let mut seen = HashSet::new();
    for indexed_field in &function.indexed_fields {
        if !seen.insert((
            indexed_field.source,
            indexed_field.access,
            indexed_field.owner_type.clone(),
            indexed_field.attr_name.clone(),
            indexed_field.expected_index,
        )) {
            errors.push(format!(
                "function {} has duplicate indexed-field {:?} {}.{} attr={} index={} at {}",
                function.function.function,
                indexed_field.access,
                indexed_field.owner_type.module_name,
                indexed_field.owner_type.qualname,
                indexed_field.attr_name,
                indexed_field.expected_index,
                indexed_field.source
            ));
        }
        if indexed_field.owner_type.module_name.is_empty() {
            errors.push(format!(
                "function {} indexed-field at {} has empty owner module",
                function.function.function, indexed_field.source
            ));
        }
        if indexed_field.owner_type.qualname.is_empty() {
            errors.push(format!(
                "function {} indexed-field at {} has empty owner qualname",
                function.function.function, indexed_field.source
            ));
        }
        if indexed_field.attr_name.is_empty() {
            errors.push(format!(
                "function {} indexed-field at {} has empty attr name",
                function.function.function, indexed_field.source
            ));
        }
        if indexed_field.reason.is_empty() {
            errors.push(format!(
                "function {} indexed-field at {} has empty reason",
                function.function.function, indexed_field.source
            ));
        }
    }
}

fn validate_indexed_global_plans(function: &FunctionOptimizationPlanV3, errors: &mut Vec<String>) {
    let mut seen = HashSet::new();
    for indexed_global in &function.indexed_globals {
        if !seen.insert((
            indexed_global.source,
            indexed_global.access,
            indexed_global.module_name.clone(),
            indexed_global.name.clone(),
            indexed_global.expected_index,
        )) {
            errors.push(format!(
                "function {} has duplicate indexed-global {:?} {}.{} index={} at {}",
                function.function.function,
                indexed_global.access,
                indexed_global.module_name,
                indexed_global.name,
                indexed_global.expected_index,
                indexed_global.source
            ));
        }
        if indexed_global.module_name.is_empty() {
            errors.push(format!(
                "function {} indexed-global at {} has empty module name",
                function.function.function, indexed_global.source
            ));
        }
        if indexed_global.name.is_empty() {
            errors.push(format!(
                "function {} indexed-global at {} has empty name",
                function.function.function, indexed_global.source
            ));
        }
        if indexed_global.guard.kind != IndexedGlobalGuardKind::ModuleDictKeyAtIndex {
            errors.push(format!(
                "function {} indexed-global at {} has unsupported guard {:?}",
                function.function.function, indexed_global.source, indexed_global.guard.kind
            ));
        }
        if indexed_global.fallback.kind != IndexedGlobalFallbackKind::OriginalGlobalAccess {
            errors.push(format!(
                "function {} indexed-global at {} has unsupported fallback {:?}",
                function.function.function, indexed_global.source, indexed_global.fallback.kind
            ));
        }
        if indexed_global.reason.is_empty() {
            errors.push(format!(
                "function {} indexed-global at {} has empty reason",
                function.function.function, indexed_global.source
            ));
        }
    }
}

fn validate_direct_call_arg_plan(
    function: &FunctionOptimizationPlanV3,
    kind: &str,
    target: SerializedFunctionId,
    source: InstrId,
    arg_plan: &DirectCallArgPlan,
    errors: &mut Vec<String>,
) {
    let mut next_provided = 0u32;
    let mut saw_default = false;
    for arg_source in &arg_plan.sources {
        match arg_source {
            DirectCallArgSource::Provided(index) => {
                if saw_default {
                    errors.push(format!(
                        "function {} {kind} target {} at {} has provided argument after a default sentinel",
                        function.function.function, target, source
                    ));
                }
                if *index != next_provided {
                    errors.push(format!(
                        "function {} {kind} target {} at {} has non-contiguous provided argument index {}, expected {}",
                        function.function.function, target, source, index, next_provided
                    ));
                }
                next_provided = next_provided.saturating_add(1);
            }
            DirectCallArgSource::DefaultSentinel => {
                saw_default = true;
            }
        }
    }
}

fn collect_region_value_reps(regions: &[RegionPlan]) -> HashMap<(RegionId, PlanValueId), Rep> {
    let mut values = HashMap::new();
    for region in regions {
        for input in &region.inputs {
            values.insert((region.id, input.value.id), input.value.rep);
        }
        for node in &region.nodes {
            match &node.kind {
                PlanNodeKind::Input { output } | PlanNodeKind::Constant { output, .. } => {
                    values.insert((region.id, output.id), output.rep);
                }
                PlanNodeKind::Convert(convert) => {
                    values.insert((region.id, convert.output.id), convert.output.rep);
                }
                PlanNodeKind::Operation(operation) => {
                    if let Some(output) = operation.output {
                        values.insert((region.id, output.id), output.rep);
                    }
                }
                PlanNodeKind::Materialize(materialize) => {
                    values.insert((region.id, materialize.output.id), materialize.output.rep);
                }
                PlanNodeKind::Guard(_)
                | PlanNodeKind::Fallback { .. }
                | PlanNodeKind::Deopt { .. }
                | PlanNodeKind::Ownership { .. } => {}
            }
        }
    }
    values
}

fn validate_region_input_source(
    region: RegionId,
    input: &RegionInput,
    region_ids: &HashSet<RegionId>,
    region_values: &HashMap<(RegionId, PlanValueId), Rep>,
    errors: &mut Vec<String>,
) {
    match &input.source {
        RegionInputSource::FunctionParam { name, .. } => {
            if matches!(name, Some(name) if name.is_empty()) {
                errors.push(format!(
                    "region {region:?} function-param input {:?} has empty name",
                    input.value
                ));
            }
        }
        RegionInputSource::CapturedValue { from_region, value } => {
            if !region_ids.contains(from_region) {
                errors.push(format!(
                    "region {region:?} captures value {:?} from unknown region {:?}",
                    value, from_region
                ));
                return;
            }
            match region_values.get(&(*from_region, *value)) {
                Some(rep) if *rep == input.value.rep => {}
                Some(rep) => errors.push(format!(
                    "region {region:?} captures value {:?} as {:?}, but producer rep is {:?}",
                    value, input.value.rep, rep
                )),
                None => errors.push(format!(
                    "region {region:?} captures undefined value {:?} from region {:?}",
                    value, from_region
                )),
            }
        }
        RegionInputSource::Synthetic { reason } if reason.is_empty() => {
            errors.push(format!(
                "region {region:?} synthetic input {:?} has empty reason",
                input.value
            ));
        }
        RegionInputSource::Synthetic { .. } => {}
    }
}

fn validate_scalar_thread_plan(
    function: &FunctionOptimizationPlanV3,
    thread: &ScalarLocalThreadPlan,
    region_ids: &HashSet<RegionId>,
    region_positions: &HashMap<RegionId, usize>,
    region_values: &HashMap<(RegionId, PlanValueId), Rep>,
    threaded_consumers: &mut HashSet<RegionValueRef>,
    errors: &mut Vec<String>,
) {
    if thread.local.name.is_empty() {
        errors.push(format!(
            "function {} has scalar thread with empty local name",
            function.function.function
        ));
    }
    if thread.reason.is_empty() {
        errors.push(format!(
            "function {} has scalar thread for local {} without reason",
            function.function.function, thread.local.name
        ));
    }
    if thread.producer.region == thread.consumer.region {
        errors.push(format!(
            "function {} scalar thread for local {} must cross regions",
            function.function.function, thread.local.name
        ));
    }
    match (
        region_positions.get(&thread.producer.region),
        region_positions.get(&thread.consumer.region),
    ) {
        (Some(producer_index), Some(consumer_index)) if producer_index < consumer_index => {}
        (Some(_), Some(_)) => errors.push(format!(
            "function {} scalar thread for local {} requires producer region {:?} before consumer region {:?}",
            function.function.function,
            thread.local.name,
            thread.producer.region,
            thread.consumer.region
        )),
        _ => {}
    }
    if !threaded_consumers.insert(thread.consumer) {
        errors.push(format!(
            "function {} has duplicate scalar thread consumer {:?}",
            function.function.function, thread.consumer
        ));
    }

    let producer_rep = check_region_value_ref(
        function,
        "scalar thread producer",
        thread.producer,
        region_values,
        errors,
    );
    let consumer_rep = check_region_value_ref(
        function,
        "scalar thread consumer",
        thread.consumer,
        region_values,
        errors,
    );
    if let (Some(producer_rep), Some(consumer_rep)) = (producer_rep, consumer_rep) {
        if producer_rep != consumer_rep {
            errors.push(format!(
                "function {} scalar thread for local {} changes rep {:?}->{:?}",
                function.function.function, thread.local.name, producer_rep, consumer_rep
            ));
        }
        if producer_rep.is_python_object() {
            errors.push(format!(
                "function {} scalar thread for local {} must thread a scalar rep, got {:?}",
                function.function.function, thread.local.name, producer_rep
            ));
        }
    }

    match &thread.fallback {
        ScalarThreadFallback::LocalFallbackRegion { region, reason } => {
            if !region_ids.contains(region) {
                errors.push(format!(
                    "function {} scalar thread for local {} references unknown fallback region {:?}",
                    function.function.function, thread.local.name, region
                ));
            }
            if *region == thread.producer.region || *region == thread.consumer.region {
                errors.push(format!(
                    "function {} scalar thread for local {} fallback region {:?} must be distinct from producer {:?} and consumer {:?}",
                    function.function.function,
                    thread.local.name,
                    region,
                    thread.producer.region,
                    thread.consumer.region
                ));
            }
            if reason.is_empty() {
                errors.push(format!(
                    "function {} scalar thread for local {} has fallback without reason",
                    function.function.function, thread.local.name
                ));
            }
            validate_scalar_thread_producer_fallbacks(function, thread, *region, errors);
        }
    }
    match &thread.local_state {
        ScalarThreadLocalState::ScalarOnlyHotPath { reason, .. } if reason.is_empty() => {
            errors.push(format!(
                "function {} scalar thread for local {} has scalar-only local state without reason",
                function.function.function, thread.local.name
            ));
        }
        ScalarThreadLocalState::ScalarOnlyHotPath {
            cleanup: ScalarThreadLocalCleanup::NoPyObjectSlotOwnership,
            ..
        } => {
            if !matches!(
                thread.materialization,
                ScalarThreadMaterialization::DeferredUntilPythonObjectUse { .. }
            ) {
                errors.push(format!(
                    "function {} scalar thread for local {} cannot use no-PyObject cleanup without deferred materialization",
                    function.function.function, thread.local.name
                ));
            }
        }
    }
    match &thread.materialization {
        ScalarThreadMaterialization::DeferredUntilPythonObjectUse { reason }
            if reason.is_empty() =>
        {
            errors.push(format!(
                "function {} scalar thread for local {} has deferred materialization without reason",
                function.function.function, thread.local.name
            ));
        }
        ScalarThreadMaterialization::DeferredUntilPythonObjectUse { .. } => {}
    }
}

fn validate_scalar_thread_producer_fallbacks(
    function: &FunctionOptimizationPlanV3,
    thread: &ScalarLocalThreadPlan,
    fallback_region: RegionId,
    errors: &mut Vec<String>,
) {
    let Some(producer_region) = function
        .regions
        .iter()
        .find(|region| region.id == thread.producer.region)
    else {
        return;
    };
    for node in &producer_region.nodes {
        let target = match &node.kind {
            PlanNodeKind::Guard(guard) => match &guard.failure {
                GuardFailure::FallbackToPlan {
                    target: FallbackTarget::Region(region),
                    ..
                } => Some(*region),
                GuardFailure::FallbackToPlan { .. } | GuardFailure::DeoptTo { .. } => None,
            },
            PlanNodeKind::Convert(convert) => match &convert.failure {
                FailureMode::FallbackToPlan {
                    target: FallbackTarget::Region(region),
                    ..
                } => Some(*region),
                FailureMode::CannotFail | FailureMode::Raise(_) | FailureMode::DeoptTo { .. } => {
                    None
                }
                FailureMode::FallbackToPlan { .. } => None,
            },
            PlanNodeKind::Operation(operation) => match &operation.failure {
                FailureMode::FallbackToPlan {
                    target: FallbackTarget::Region(region),
                    ..
                } => Some(*region),
                FailureMode::CannotFail | FailureMode::Raise(_) | FailureMode::DeoptTo { .. } => {
                    None
                }
                FailureMode::FallbackToPlan { .. } => None,
            },
            PlanNodeKind::Input { .. }
            | PlanNodeKind::Constant { .. }
            | PlanNodeKind::Materialize(_)
            | PlanNodeKind::Fallback { .. }
            | PlanNodeKind::Deopt { .. }
            | PlanNodeKind::Ownership { .. } => None,
        };
        if let Some(target) = target
            && target != fallback_region
        {
            errors.push(format!(
                "function {} scalar thread for local {} producer node {:?} falls back to {:?}, expected scalar-thread fallback {:?}",
                function.function.function, thread.local.name, node.id, target, fallback_region
            ));
        }
    }
}

fn check_region_value_ref(
    function: &FunctionOptimizationPlanV3,
    kind: &str,
    value_ref: RegionValueRef,
    region_values: &HashMap<(RegionId, PlanValueId), Rep>,
    errors: &mut Vec<String>,
) -> Option<Rep> {
    match region_values
        .get(&(value_ref.region, value_ref.value.id))
        .copied()
    {
        Some(rep) if rep == value_ref.value.rep => Some(rep),
        Some(rep) => {
            errors.push(format!(
                "function {} {kind} {:?} declares rep {:?}, but region defines {:?}",
                function.function.function, value_ref, value_ref.value.rep, rep
            ));
            Some(rep)
        }
        None => {
            errors.push(format!(
                "function {} {kind} references undefined value {:?}",
                function.function.function, value_ref
            ));
            None
        }
    }
}

fn validate_region_plan(
    region: &RegionPlan,
    region_ids: &HashSet<RegionId>,
    deopt_points: &HashSet<DeoptPointId>,
    errors: &mut Vec<String>,
) {
    let mut available_values = HashMap::<PlanValueId, Rep>::new();
    let mut output_values = HashSet::<PlanValueId>::new();
    for input in &region.inputs {
        define_value(
            region.id,
            input.value,
            "region input",
            &mut available_values,
            &mut output_values,
            errors,
        );
    }

    let node_ids = region
        .nodes
        .iter()
        .map(|node| node.id)
        .collect::<HashSet<_>>();
    let mut seen_node_ids = HashSet::<PlanNodeId>::new();
    let mut seen_guard_ids = HashSet::<PlanNodeId>::new();

    for node in &region.nodes {
        if !seen_node_ids.insert(node.id) {
            errors.push(format!(
                "region {:?} has duplicate node {:?}",
                region.id, node.id
            ));
        }
        match &node.kind {
            PlanNodeKind::Input { output } => {
                define_value(
                    region.id,
                    *output,
                    "input node",
                    &mut available_values,
                    &mut output_values,
                    errors,
                );
            }
            PlanNodeKind::Constant { output, .. } => {
                define_value(
                    region.id,
                    *output,
                    "constant node",
                    &mut available_values,
                    &mut output_values,
                    errors,
                );
            }
            PlanNodeKind::Convert(convert) => {
                check_input(region.id, convert.input, &available_values, errors);
                validate_conversion(region.id, convert, &seen_guard_ids, errors);
                validate_failure_mode(
                    region.id,
                    &convert.failure,
                    &FailureReplayPolicy::safe("conversion rule owns failure semantics"),
                    &node_ids,
                    region_ids,
                    deopt_points,
                    errors,
                );
                define_value(
                    region.id,
                    convert.output,
                    "conversion output",
                    &mut available_values,
                    &mut output_values,
                    errors,
                );
            }
            PlanNodeKind::Guard(guard) => {
                seen_guard_ids.insert(node.id);
                for input in &guard.inputs {
                    check_input(region.id, *input, &available_values, errors);
                }
                validate_guard_failure(
                    region.id,
                    &guard.failure,
                    &guard.guard.replay,
                    &node_ids,
                    region_ids,
                    deopt_points,
                    errors,
                );
            }
            PlanNodeKind::Operation(operation) => {
                for input in &operation.inputs {
                    check_input(region.id, *input, &available_values, errors);
                }
                validate_operation(region.id, operation, errors);
                validate_operation_failure_semantics(region.id, operation, errors);
                validate_failure_mode(
                    region.id,
                    &operation.failure,
                    &operation.failure_replay,
                    &node_ids,
                    region_ids,
                    deopt_points,
                    errors,
                );
                if let Some(output) = operation.output {
                    define_value(
                        region.id,
                        output,
                        "operation output",
                        &mut available_values,
                        &mut output_values,
                        errors,
                    );
                }
            }
            PlanNodeKind::Materialize(materialize) => {
                check_input(region.id, materialize.input, &available_values, errors);
                validate_materialize(region.id, materialize, errors);
                define_value(
                    region.id,
                    materialize.output,
                    "materialization output",
                    &mut available_values,
                    &mut output_values,
                    errors,
                );
            }
            PlanNodeKind::Fallback { target } => {
                validate_fallback_target(region.id, target, &node_ids, region_ids, errors);
            }
            PlanNodeKind::Deopt { target } => {
                if !deopt_points.contains(target) {
                    errors.push(format!(
                        "region {:?} references unknown deopt point {:?}",
                        region.id, target
                    ));
                }
            }
            PlanNodeKind::Ownership { action } => {
                check_input(region.id, action.value, &available_values, errors);
                if action.reason.is_empty() {
                    errors.push(format!(
                        "region {:?} has ownership action for {:?} without reason",
                        region.id, action.value
                    ));
                }
            }
        }
    }

    for exit in &region.exits {
        match &exit.kind {
            RegionExitKind::Branch {
                condition,
                then_target,
                else_target,
            } => {
                check_input(region.id, *condition, &available_values, errors);
                if condition.rep != Rep::I32Bool01 {
                    errors.push(format!(
                        "region {:?} branch exits require I32Bool01, got {:?}",
                        region.id, condition.rep
                    ));
                }
                validate_exit_target(region.id, then_target, region_ids, errors);
                validate_exit_target(region.id, else_target, region_ids, errors);
            }
            RegionExitKind::Return { value } => {
                check_input(region.id, *value, &available_values, errors);
                if !matches!(value.rep, Rep::PyObjectOwned | Rep::PyObjectImmortal) {
                    errors.push(format!(
                        "region {:?} return exits require a returnable PyObject, got {:?}",
                        region.id, value.rep
                    ));
                }
            }
            RegionExitKind::Jump { target } => {
                validate_exit_target(region.id, target, region_ids, errors);
            }
        }
    }
}

fn define_value(
    region: RegionId,
    value: PlanValue,
    source: &str,
    available_values: &mut HashMap<PlanValueId, Rep>,
    output_values: &mut HashSet<PlanValueId>,
    errors: &mut Vec<String>,
) {
    if !output_values.insert(value.id) {
        errors.push(format!(
            "region {region:?} defines duplicate value {:?} from {source}",
            value.id
        ));
        return;
    }
    available_values.insert(value.id, value.rep);
}

fn check_input(
    region: RegionId,
    value: PlanValue,
    available_values: &HashMap<PlanValueId, Rep>,
    errors: &mut Vec<String>,
) {
    match available_values.get(&value.id) {
        Some(rep) if *rep == value.rep => {}
        Some(rep) => errors.push(format!(
            "region {region:?} input {:?} expected rep {:?}, but available rep is {:?}",
            value.id, value.rep, rep
        )),
        None => errors.push(format!(
            "region {region:?} uses undefined value {:?}",
            value.id
        )),
    }
}

fn validate_conversion(
    region: RegionId,
    convert: &ConvertNode,
    seen_guard_ids: &HashSet<PlanNodeId>,
    errors: &mut Vec<String>,
) {
    let rule = conversion_signature(convert.kind);
    if convert.input.rep != rule.input {
        errors.push(format!(
            "region {region:?} conversion {:?} expects input {:?}, got {:?}",
            convert.kind, rule.input, convert.input.rep
        ));
    }
    if convert.output.rep != rule.output {
        errors.push(format!(
            "region {region:?} conversion {:?} produces {:?}, got {:?}",
            convert.kind, rule.output, convert.output.rep
        ));
    }
    if convert.ownership != rule.ownership {
        errors.push(format!(
            "region {region:?} conversion {:?} requires ownership {:?}, got {:?}",
            convert.kind, rule.ownership, convert.ownership
        ));
    }
    match (&rule.precondition, &convert.precondition) {
        (ConversionPreconditionRule::Infallible, ConversionPrecondition::Infallible) => {}
        (
            ConversionPreconditionRule::FactsOrGuard,
            ConversionPrecondition::DominatingFacts { reason },
        ) if !reason.is_empty() => {}
        (
            ConversionPreconditionRule::FactsOrGuard,
            ConversionPrecondition::SpecializationGuard { guard, reason },
        ) if !reason.is_empty() && seen_guard_ids.contains(guard) => {}
        (
            ConversionPreconditionRule::FactsOrGuard,
            ConversionPrecondition::SpecializationGuard { guard, reason },
        ) if reason.is_empty() => errors.push(format!(
            "region {region:?} conversion {:?} references guard {:?} without reason",
            convert.kind, guard
        )),
        (
            ConversionPreconditionRule::FactsOrGuard,
            ConversionPrecondition::SpecializationGuard { guard, .. },
        ) => errors.push(format!(
            "region {region:?} conversion {:?} references non-dominating guard {:?}",
            convert.kind, guard
        )),
        _ => errors.push(format!(
            "region {region:?} conversion {:?} has invalid precondition {:?}",
            convert.kind, convert.precondition
        )),
    }
    validate_conversion_failure(region, convert, rule, errors);
}

fn validate_operation_failure_semantics(
    region: RegionId,
    operation: &OperationNode,
    errors: &mut Vec<String>,
) {
    match &operation.op {
        PlannedOp::CheckedI64Add | PlannedOp::CheckedI64Sub | PlannedOp::CheckedI64Mul => {
            if !matches!(operation.failure, FailureMode::FallbackToPlan { .. }) {
                errors.push(format!(
                    "region {region:?} operation {:?} must use an explicit local fallback for overflow, got {:?}",
                    operation.op, operation.failure
                ));
            }
        }
        PlannedOp::I64BitAnd
        | PlannedOp::I64BitOr
        | PlannedOp::I64BitXor
        | PlannedOp::I64CompareToBool01 { .. } => {
            if operation.failure != FailureMode::CannotFail {
                errors.push(format!(
                    "region {region:?} operation {:?} should be CannotFail after selected scalar inputs, got {:?}",
                    operation.op, operation.failure
                ));
            }
        }
        PlannedOp::PyNumberAdd
        | PlannedOp::PyNumberSubtract
        | PlannedOp::PyNumberMultiply
        | PlannedOp::PyNumberBitAnd
        | PlannedOp::PyNumberBitOr
        | PlannedOp::PyNumberBitXor
        | PlannedOp::PyObjectRichCompare { .. }
        | PlannedOp::PyObjectIsTrue => {
            if !matches!(operation.failure, FailureMode::Raise(_)) {
                errors.push(format!(
                    "region {region:?} operation {:?} must raise locally through Python semantics, got {:?}",
                    operation.op, operation.failure
                ));
            }
        }
        PlannedOp::DirectHelper { .. } => {}
    }
}

fn validate_conversion_failure(
    region: RegionId,
    convert: &ConvertNode,
    rule: ConversionSignature,
    errors: &mut Vec<String>,
) {
    match rule.failure {
        ConversionFailureRule::CannotFail if convert.failure == FailureMode::CannotFail => {}
        ConversionFailureRule::MayRaise if matches!(convert.failure, FailureMode::Raise(_)) => {}
        ConversionFailureRule::SpecializationMiss
            if matches!(
                convert.failure,
                FailureMode::FallbackToPlan { .. } | FailureMode::DeoptTo { .. }
            ) => {}
        _ => errors.push(format!(
            "region {region:?} conversion {:?} has invalid failure mode {:?}",
            convert.kind, convert.failure
        )),
    }
}

fn validate_materialize(region: RegionId, materialize: &MaterializeNode, errors: &mut Vec<String>) {
    let expected = match materialize.kind {
        MaterializeKind::PythonLong => (Rep::I64, Rep::PyObjectOwned),
        MaterializeKind::PythonBool => (Rep::I32Bool01, Rep::PyObjectImmortal),
    };
    if materialize.input.rep != expected.0 || materialize.output.rep != expected.1 {
        errors.push(format!(
            "region {region:?} materialization {:?} expects {:?}->{:?}, got {:?}->{:?}",
            materialize.kind, expected.0, expected.1, materialize.input.rep, materialize.output.rep
        ));
    }
}

fn validate_operation(region: RegionId, operation: &OperationNode, errors: &mut Vec<String>) {
    match &operation.op {
        PlannedOp::PyNumberAdd
        | PlannedOp::PyNumberSubtract
        | PlannedOp::PyNumberMultiply
        | PlannedOp::PyNumberBitAnd
        | PlannedOp::PyNumberBitOr
        | PlannedOp::PyNumberBitXor => {
            validate_python_operation_inputs(region, operation, 2, "PyNumberBinary", errors);
            validate_operation_output(
                region,
                operation,
                Some(Rep::PyObjectOwned),
                "PyNumberBinary",
                errors,
            );
        }
        PlannedOp::PyObjectRichCompare { .. } => {
            validate_python_operation_inputs(region, operation, 2, "PyObjectRichCompare", errors);
            validate_operation_output(
                region,
                operation,
                Some(Rep::PyObjectOwned),
                "PyObjectRichCompare",
                errors,
            );
        }
        PlannedOp::PyObjectIsTrue => {
            validate_python_operation_inputs(region, operation, 1, "PyObjectIsTrue", errors);
            validate_operation_output(
                region,
                operation,
                Some(Rep::I32Bool01),
                "PyObjectIsTrue",
                errors,
            );
        }
        PlannedOp::CheckedI64Add
        | PlannedOp::CheckedI64Sub
        | PlannedOp::CheckedI64Mul
        | PlannedOp::I64BitAnd
        | PlannedOp::I64BitOr
        | PlannedOp::I64BitXor => {
            validate_exact_inputs(
                region,
                operation,
                &[Rep::I64, Rep::I64],
                "I64Binary",
                errors,
            );
            validate_operation_output(region, operation, Some(Rep::I64), "I64Binary", errors);
        }
        PlannedOp::I64CompareToBool01 { .. } => {
            validate_exact_inputs(
                region,
                operation,
                &[Rep::I64, Rep::I64],
                "I64CompareToBool01",
                errors,
            );
            validate_operation_output(
                region,
                operation,
                Some(Rep::I32Bool01),
                "I64CompareToBool01",
                errors,
            );
        }
        PlannedOp::DirectHelper { .. } => {}
    }
}

fn validate_python_operation_inputs(
    region: RegionId,
    operation: &OperationNode,
    arity: usize,
    name: &str,
    errors: &mut Vec<String>,
) {
    if operation.inputs.len() != arity {
        errors.push(format!(
            "region {region:?} operation {name} expects {arity} inputs, got {}",
            operation.inputs.len()
        ));
        return;
    }
    for (index, input) in operation.inputs.iter().enumerate() {
        if !input.rep.is_python_object() {
            errors.push(format!(
                "region {region:?} operation {name} input {index} expects a Python object rep, got {:?}",
                input.rep
            ));
        }
    }
}

fn validate_exact_inputs(
    region: RegionId,
    operation: &OperationNode,
    expected: &[Rep],
    name: &str,
    errors: &mut Vec<String>,
) {
    if operation.inputs.len() != expected.len() {
        errors.push(format!(
            "region {region:?} operation {name} expects {} inputs, got {}",
            expected.len(),
            operation.inputs.len()
        ));
        return;
    }
    for (index, (input, expected_rep)) in operation.inputs.iter().zip(expected.iter()).enumerate() {
        if input.rep != *expected_rep {
            errors.push(format!(
                "region {region:?} operation {name} input {index} expects {:?}, got {:?}",
                expected_rep, input.rep
            ));
        }
    }
}

fn validate_operation_output(
    region: RegionId,
    operation: &OperationNode,
    expected: Option<Rep>,
    name: &str,
    errors: &mut Vec<String>,
) {
    match (operation.output, expected) {
        (Some(output), Some(expected_rep)) if output.rep == expected_rep => {}
        (Some(output), Some(expected_rep)) => errors.push(format!(
            "region {region:?} operation {name} output expects {:?}, got {:?}",
            expected_rep, output.rep
        )),
        (None, Some(expected_rep)) => errors.push(format!(
            "region {region:?} operation {name} expects output {:?}, got no output",
            expected_rep
        )),
        (Some(output), None) => errors.push(format!(
            "region {region:?} operation {name} expects no output, got {:?}",
            output.rep
        )),
        (None, None) => {}
    }
}

fn validate_guard_failure(
    region: RegionId,
    failure: &GuardFailure,
    replay: &FailureReplayPolicy,
    node_ids: &HashSet<PlanNodeId>,
    region_ids: &HashSet<RegionId>,
    deopt_points: &HashSet<DeoptPointId>,
    errors: &mut Vec<String>,
) {
    match failure {
        GuardFailure::FallbackToPlan { target, reason } => {
            if reason.0.is_empty() {
                errors.push(format!(
                    "region {region:?} guard fallback target {target:?} has empty reason"
                ));
            }
            validate_fallback_target(region, target, node_ids, region_ids, errors);
        }
        GuardFailure::DeoptTo { target, reason } => {
            if reason.0.is_empty() {
                errors.push(format!(
                    "region {region:?} guard deopt target {target:?} has empty reason"
                ));
            }
            validate_deopt_replay(region, replay, errors);
            if !deopt_points.contains(target) {
                errors.push(format!(
                    "region {region:?} references unknown deopt point {target:?}"
                ));
            }
        }
    }
}

fn validate_failure_mode(
    region: RegionId,
    failure: &FailureMode,
    replay: &FailureReplayPolicy,
    node_ids: &HashSet<PlanNodeId>,
    region_ids: &HashSet<RegionId>,
    deopt_points: &HashSet<DeoptPointId>,
    errors: &mut Vec<String>,
) {
    match failure {
        FailureMode::CannotFail | FailureMode::Raise(_) => {}
        FailureMode::FallbackToPlan { target, reason } => {
            if replay.replay == FailureReplayKind::MustUseLocalFallback
                && matches!(target, FallbackTarget::OriginalInstruction { .. })
            {
                errors.push(format!(
                    "region {region:?} local fallback must target an explicit plan node or region"
                ));
            }
            if reason.0.is_empty() {
                errors.push(format!(
                    "region {region:?} fallback target {target:?} has empty reason"
                ));
            }
            validate_fallback_target(region, target, node_ids, region_ids, errors);
        }
        FailureMode::DeoptTo { target, reason } => {
            if reason.0.is_empty() {
                errors.push(format!(
                    "region {region:?} deopt target {target:?} has empty reason"
                ));
            }
            validate_deopt_replay(region, replay, errors);
            if !deopt_points.contains(target) {
                errors.push(format!(
                    "region {region:?} references unknown deopt point {target:?}"
                ));
            }
        }
    }
}

fn validate_deopt_replay(region: RegionId, replay: &FailureReplayPolicy, errors: &mut Vec<String>) {
    if replay.replay != FailureReplayKind::SafeToReplay {
        errors.push(format!(
            "region {region:?} deopt failure requires replay-safe policy, got {:?}",
            replay.replay
        ));
    }
    if replay.reason.0.is_empty() {
        errors.push(format!(
            "region {region:?} deopt failure requires a non-empty replay reason"
        ));
    }
}

fn validate_fallback_target(
    region: RegionId,
    target: &FallbackTarget,
    node_ids: &HashSet<PlanNodeId>,
    region_ids: &HashSet<RegionId>,
    errors: &mut Vec<String>,
) {
    match target {
        FallbackTarget::Region(target_region) if !region_ids.contains(target_region) => {
            errors.push(format!(
                "region {region:?} references unknown fallback region {target_region:?}"
            ));
        }
        FallbackTarget::Node(target_node) if !node_ids.contains(target_node) => {
            errors.push(format!(
                "region {region:?} references unknown fallback node {target_node:?}"
            ));
        }
        FallbackTarget::Region(_)
        | FallbackTarget::Node(_)
        | FallbackTarget::OriginalInstruction { .. } => {}
    }
}

fn validate_exit_target(
    region: RegionId,
    target: &RegionExitTarget,
    region_ids: &HashSet<RegionId>,
    errors: &mut Vec<String>,
) {
    if let RegionExitTarget::Region(target_region) = target
        && !region_ids.contains(target_region)
    {
        errors.push(format!(
            "region {region:?} references unknown exit region {target_region:?}"
        ));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConversionSignature {
    pub input: Rep,
    pub output: Rep,
    pub precondition: ConversionPreconditionRule,
    pub failure: ConversionFailureRule,
    pub ownership: ConversionOwnership,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionPreconditionRule {
    Infallible,
    FactsOrGuard,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversionFailureRule {
    CannotFail,
    MayRaise,
    SpecializationMiss,
}

pub fn conversion_signature(kind: ConversionKind) -> ConversionSignature {
    match kind {
        ConversionKind::FromPythonLongCompactToI64 => ConversionSignature {
            input: Rep::PyObjectBorrowed,
            output: Rep::I64,
            precondition: ConversionPreconditionRule::FactsOrGuard,
            failure: ConversionFailureRule::SpecializationMiss,
            ownership: ConversionOwnership::BorrowInput,
        },
        ConversionKind::ToPythonLongOwned => ConversionSignature {
            input: Rep::I64,
            output: Rep::PyObjectOwned,
            precondition: ConversionPreconditionRule::Infallible,
            failure: ConversionFailureRule::MayRaise,
            ownership: ConversionOwnership::MaterializeOwned,
        },
        ConversionKind::ToPythonBoolImmortal => ConversionSignature {
            input: Rep::I32Bool01,
            output: Rep::PyObjectImmortal,
            precondition: ConversionPreconditionRule::Infallible,
            failure: ConversionFailureRule::CannotFail,
            ownership: ConversionOwnership::Preserve,
        },
        ConversionKind::TruthinessToI32Bool01 => ConversionSignature {
            input: Rep::PyObjectOwned,
            output: Rep::I32Bool01,
            precondition: ConversionPreconditionRule::Infallible,
            failure: ConversionFailureRule::MayRaise,
            ownership: ConversionOwnership::ConsumeOwned,
        },
    }
}

fn finish_validation(errors: Vec<String>) -> Result<(), PlanValidationError> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(PlanValidationError { errors })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_core::block_py::{
        BlockLabel, LocalFunctionId, SerializedModuleId, SerializedModuleIdentity,
    };

    fn function_id() -> SerializedFunctionId {
        SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(1))
    }

    fn module_with_regions(regions: Vec<RegionPlan>) -> ModuleOptimizationPlanV3 {
        module_with_regions_and_scalar_threads(regions, Vec::new())
    }

    fn module_with_direct_calls(
        direct_calls: Vec<DirectCallSpecializationPlan>,
    ) -> ModuleOptimizationPlanV3 {
        let mut module = module_with_regions(Vec::new());
        module.functions[0].direct_calls = direct_calls;
        module
    }

    fn module_with_constructor_calls(
        constructor_calls: Vec<ConstructorCallSpecializationPlan>,
    ) -> ModuleOptimizationPlanV3 {
        let mut module = module_with_regions(Vec::new());
        module.functions[0].constructor_calls = constructor_calls;
        module
    }

    fn module_with_method_calls(
        method_calls: Vec<MethodCallSpecializationPlan>,
    ) -> ModuleOptimizationPlanV3 {
        let mut module = module_with_regions(Vec::new());
        module.functions[0].method_calls = method_calls;
        module
    }

    fn module_with_indexed_fields(
        indexed_fields: Vec<IndexedFieldSpecializationPlan>,
    ) -> ModuleOptimizationPlanV3 {
        let mut module = module_with_regions(Vec::new());
        module.functions[0].indexed_fields = indexed_fields;
        module
    }

    fn module_with_exact_list_items(
        exact_list_items: Vec<ExactListItemSpecializationPlan>,
    ) -> ModuleOptimizationPlanV3 {
        let mut module = module_with_regions(Vec::new());
        module.functions[0].exact_list_items = exact_list_items;
        module
    }

    fn module_with_indexed_globals(
        indexed_globals: Vec<IndexedGlobalSpecializationPlan>,
    ) -> ModuleOptimizationPlanV3 {
        let mut module = module_with_regions(Vec::new());
        module.functions[0].indexed_globals = indexed_globals;
        module
    }

    fn module_with_regions_and_scalar_threads(
        regions: Vec<RegionPlan>,
        scalar_threads: Vec<ScalarLocalThreadPlan>,
    ) -> ModuleOptimizationPlanV3 {
        ModuleOptimizationPlanV3 {
            module: ModulePlanIdentity {
                module_name: "pkg.mod".to_string(),
                source_hash: 0x1234,
                cache_identity: "test-cache".to_string(),
            },
            identity_tables: SerializedIdentityTables {
                modules: vec![SerializedModuleIdentity {
                    module_name: "pkg.mod".to_string(),
                    source_hash: 0x1234,
                    cache_identity: Some("test-cache".to_string()),
                }],
                debug_names: Vec::new(),
            },
            helper_catalog_version: 1,
            cost_model_version: 1,
            functions: vec![FunctionOptimizationPlanV3 {
                function: FunctionPlanIdentity {
                    function: function_id(),
                    debug_name: Some("f".to_string()),
                },
                regions,
                scalar_threads,
                direct_calls: Vec::new(),
                constructor_calls: Vec::new(),
                method_calls: Vec::new(),
                exact_list_items: Vec::new(),
                indexed_fields: Vec::new(),
                indexed_globals: Vec::new(),
                deopt_points: vec![PlannedDeoptPoint {
                    id: DeoptPointId(0),
                    source: DeoptPointSource::Synthetic {
                        reason: "test".to_string(),
                    },
                    reason: "test".to_string(),
                }],
                ownership: FunctionOwnershipPlan::default(),
                diagnostics: Vec::new(),
            }],
        }
    }

    fn instr_id(index: u32) -> InstrId {
        InstrId::new(BlockLabel::from_index(0), index)
    }

    #[test]
    fn validates_direct_call_selections() {
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        let plan = module_with_direct_calls(vec![DirectCallSpecializationPlan {
            source: instr_id(7),
            target,
            arg_plan: DirectCallArgPlan {
                sources: vec![DirectCallArgSource::Provided(0)],
            },
            reason: "profiled call target".to_string(),
        }]);

        validate_module_plan_v3(&plan).unwrap();
    }

    #[test]
    fn validates_cross_module_direct_call_selections_with_identity() {
        let target = SerializedFunctionId::new(SerializedModuleId::new(1), LocalFunctionId::new(2));
        let mut plan = module_with_direct_calls(vec![DirectCallSpecializationPlan {
            source: instr_id(7),
            target,
            arg_plan: DirectCallArgPlan {
                sources: vec![DirectCallArgSource::Provided(0)],
            },
            reason: "profiled call target".to_string(),
        }]);
        plan.identity_tables.modules.push(SerializedModuleIdentity {
            module_name: "pkg.callee".to_string(),
            source_hash: 0x5678,
            cache_identity: None,
        });

        validate_module_plan_v3(&plan).unwrap();
    }

    #[test]
    fn rejects_direct_call_selections_with_missing_target_module_identity() {
        let target = SerializedFunctionId::new(SerializedModuleId::new(1), LocalFunctionId::new(2));
        let plan = module_with_direct_calls(vec![DirectCallSpecializationPlan {
            source: instr_id(7),
            target,
            arg_plan: DirectCallArgPlan {
                sources: vec![DirectCallArgSource::Provided(0)],
            },
            reason: "profiled call target".to_string(),
        }]);
        let err = validate_module_plan_v3(&plan).unwrap_err();
        assert!(err.to_string().contains("missing module id 1"));
    }

    #[test]
    fn validates_constructor_call_selections() {
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        let plan = module_with_constructor_calls(vec![ConstructorCallSpecializationPlan {
            source: instr_id(7),
            target,
            owner_type: ConstructorCallOwnerType {
                module_name: "pkg.mod".to_string(),
                qualname: "Box".to_string(),
            },
            arg_plan: DirectCallArgPlan {
                sources: vec![
                    DirectCallArgSource::Provided(0),
                    DirectCallArgSource::Provided(1),
                ],
            },
            guard: ConstructorCallGuardPlan {
                kind: ConstructorCallGuardKind::ExactCallableTypeVersion,
            },
            fallback: ConstructorCallFallbackPlan {
                kind: ConstructorCallFallbackKind::OriginalConstructorCall,
            },
            reason: "profiled constructor target".to_string(),
        }]);

        validate_module_plan_v3(&plan).unwrap();
    }

    #[test]
    fn rejects_constructor_call_selections_with_missing_target_module_identity() {
        let target = SerializedFunctionId::new(SerializedModuleId::new(1), LocalFunctionId::new(2));
        let plan = module_with_constructor_calls(vec![ConstructorCallSpecializationPlan {
            source: instr_id(7),
            target,
            owner_type: ConstructorCallOwnerType {
                module_name: "pkg.mod".to_string(),
                qualname: "Box".to_string(),
            },
            arg_plan: DirectCallArgPlan {
                sources: vec![DirectCallArgSource::Provided(0)],
            },
            guard: ConstructorCallGuardPlan {
                kind: ConstructorCallGuardKind::ExactCallableTypeVersion,
            },
            fallback: ConstructorCallFallbackPlan {
                kind: ConstructorCallFallbackKind::OriginalConstructorCall,
            },
            reason: "profiled constructor target".to_string(),
        }]);

        let err = validate_module_plan_v3(&plan).unwrap_err();
        assert!(
            err.to_string()
                .contains("constructor-call target 1:2 references missing module id 1"),
            "{err}"
        );
    }

    #[test]
    fn validates_method_call_selections() {
        let target = SerializedFunctionId::new(SerializedModuleId::new(0), LocalFunctionId::new(2));
        let plan = module_with_method_calls(vec![MethodCallSpecializationPlan {
            source: instr_id(7),
            target,
            method_name: "get".to_string(),
            owner_type: MethodCallOwnerType {
                module_name: "pkg.mod".to_string(),
                qualname: "Box".to_string(),
            },
            arg_plan: DirectCallArgPlan {
                sources: vec![DirectCallArgSource::Provided(0)],
            },
            guard: MethodCallGuardPlan {
                kind: MethodCallGuardKind::ExactReceiverTypeVersion,
            },
            fallback: MethodCallFallbackPlan {
                kind: MethodCallFallbackKind::OriginalMethodCall,
            },
            reason: "profiled owner-method target".to_string(),
        }]);

        validate_module_plan_v3(&plan).unwrap();
    }

    #[test]
    fn rejects_method_call_selections_with_missing_target_module_identity() {
        let target = SerializedFunctionId::new(SerializedModuleId::new(1), LocalFunctionId::new(2));
        let plan = module_with_method_calls(vec![MethodCallSpecializationPlan {
            source: instr_id(7),
            target,
            method_name: "get".to_string(),
            owner_type: MethodCallOwnerType {
                module_name: "pkg.mod".to_string(),
                qualname: "Box".to_string(),
            },
            arg_plan: DirectCallArgPlan {
                sources: vec![DirectCallArgSource::Provided(0)],
            },
            guard: MethodCallGuardPlan {
                kind: MethodCallGuardKind::ExactReceiverTypeVersion,
            },
            fallback: MethodCallFallbackPlan {
                kind: MethodCallFallbackKind::OriginalMethodCall,
            },
            reason: "profiled owner-method target".to_string(),
        }]);

        let err = validate_module_plan_v3(&plan).unwrap_err();
        assert!(
            err.to_string()
                .contains("method-call target 1:2 references missing module id 1"),
            "{err}"
        );
    }

    #[test]
    fn validates_exact_list_item_selections() {
        let plan = module_with_exact_list_items(vec![ExactListItemSpecializationPlan {
            source: instr_id(7),
            access: ExactListItemAccessKind::Get,
            shape: ExactListItemShape::ExactListExactInt,
            guard: ExactListItemGuardPlan {
                kind: ExactListItemGuardKind::ExactListExactCompactIntInBounds,
            },
            fallback: ExactListItemFallbackPlan {
                kind: ExactListItemFallbackKind::OriginalItemAccess,
            },
            reason: "profiled getitem_hot_shapes selected exact-list/exact-int".to_string(),
        }]);

        validate_module_plan_v3(&plan).unwrap();
    }

    #[test]
    fn rejects_duplicate_exact_list_item_selections() {
        let exact_list_item = ExactListItemSpecializationPlan {
            source: instr_id(7),
            access: ExactListItemAccessKind::Get,
            shape: ExactListItemShape::ExactListExactInt,
            guard: ExactListItemGuardPlan {
                kind: ExactListItemGuardKind::ExactListExactCompactIntInBounds,
            },
            fallback: ExactListItemFallbackPlan {
                kind: ExactListItemFallbackKind::OriginalItemAccess,
            },
            reason: "profiled getitem_hot_shapes selected exact-list/exact-int".to_string(),
        };
        let plan = module_with_exact_list_items(vec![exact_list_item.clone(), exact_list_item]);

        let err = validate_module_plan_v3(&plan).unwrap_err();
        assert!(err.to_string().contains("duplicate exact-list item"));
    }

    #[test]
    fn validates_indexed_field_selections() {
        let plan = module_with_indexed_fields(vec![IndexedFieldSpecializationPlan {
            source: instr_id(7),
            access: IndexedFieldAccessKind::Load,
            owner_type: IndexedFieldOwnerType {
                module_name: "pkg.model".to_string(),
                qualname: "Record".to_string(),
            },
            attr_name: "value".to_string(),
            expected_index: 2,
            reason: "profiled type_keys selected this indexed-field layout".to_string(),
        }]);

        validate_module_plan_v3(&plan).unwrap();
    }

    #[test]
    fn rejects_duplicate_indexed_field_selections() {
        let indexed_field = IndexedFieldSpecializationPlan {
            source: instr_id(7),
            access: IndexedFieldAccessKind::Load,
            owner_type: IndexedFieldOwnerType {
                module_name: "pkg.model".to_string(),
                qualname: "Record".to_string(),
            },
            attr_name: "value".to_string(),
            expected_index: 2,
            reason: "profiled type_keys selected this indexed-field layout".to_string(),
        };
        let plan = module_with_indexed_fields(vec![indexed_field.clone(), indexed_field]);

        let err = validate_module_plan_v3(&plan).unwrap_err();
        assert!(err.to_string().contains("duplicate indexed-field"));
    }

    #[test]
    fn validates_indexed_global_selections() {
        let plan = module_with_indexed_globals(vec![IndexedGlobalSpecializationPlan {
            source: instr_id(7),
            access: IndexedGlobalAccessKind::Load,
            module_name: "pkg.mod".to_string(),
            name: "value".to_string(),
            expected_index: 2,
            guard: IndexedGlobalGuardPlan {
                kind: IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
            },
            fallback: IndexedGlobalFallbackPlan {
                kind: IndexedGlobalFallbackKind::OriginalGlobalAccess,
            },
            reason: "profiled module_keys selected this indexed-global slot".to_string(),
        }]);

        validate_module_plan_v3(&plan).unwrap();
    }

    #[test]
    fn rejects_duplicate_indexed_global_selections() {
        let indexed_global = IndexedGlobalSpecializationPlan {
            source: instr_id(7),
            access: IndexedGlobalAccessKind::Load,
            module_name: "pkg.mod".to_string(),
            name: "value".to_string(),
            expected_index: 2,
            guard: IndexedGlobalGuardPlan {
                kind: IndexedGlobalGuardKind::ModuleDictKeyAtIndex,
            },
            fallback: IndexedGlobalFallbackPlan {
                kind: IndexedGlobalFallbackKind::OriginalGlobalAccess,
            },
            reason: "profiled module_keys selected this indexed-global slot".to_string(),
        };
        let plan = module_with_indexed_globals(vec![indexed_global.clone(), indexed_global]);

        let err = validate_module_plan_v3(&plan).unwrap_err();
        assert!(err.to_string().contains("duplicate indexed-global"));
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

    fn fallback_to(region: u32) -> GuardFailure {
        GuardFailure::FallbackToPlan {
            target: FallbackTarget::Region(RegionId(region)),
            reason: FallbackReason("guard miss uses generic fallback".to_string()),
        }
    }

    fn exact_long_guard(input: PlanValue, id: u32, fallback_region: u32) -> PlanNode {
        node(
            id,
            PlanNodeKind::Guard(GuardNode {
                inputs: vec![input],
                guard: GuardSpec {
                    kind: GuardKind::SpecializationCheck,
                    replay: FailureReplayPolicy::local_fallback(
                        "fallback region reuses original Python values",
                    ),
                    description: "exact compact PyLong".to_string(),
                },
                failure: fallback_to(fallback_region),
            }),
        )
    }

    fn unbox(
        id: u32,
        input: PlanValue,
        output: PlanValue,
        guard: u32,
        fallback_region: u32,
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
                    target: FallbackTarget::Region(RegionId(fallback_region)),
                    reason: FallbackReason("conversion miss uses generic fallback".to_string()),
                },
                ownership: ConversionOwnership::BorrowInput,
            }),
        )
    }

    fn valid_compact_int_plan() -> ModuleOptimizationPlanV3 {
        let a_obj = PlanValue::new(0, Rep::PyObjectBorrowed);
        let b_obj = PlanValue::new(1, Rep::PyObjectBorrowed);
        let a_i64 = PlanValue::new(2, Rep::I64);
        let b_i64 = PlanValue::new(3, Rep::I64);
        let c_i64 = PlanValue::new(4, Rep::I64);
        let zero_i64 = PlanValue::new(5, Rep::I64);
        let cmp_i32 = PlanValue::new(6, Rep::I32Bool01);

        let hot_region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![input(a_obj, 0, "a"), input(b_obj, 1, "b")],
            nodes: vec![
                exact_long_guard(a_obj, 0, 1),
                exact_long_guard(b_obj, 1, 1),
                unbox(2, a_obj, a_i64, 0, 1),
                unbox(3, b_obj, b_i64, 1, 1),
                node(
                    4,
                    PlanNodeKind::Operation(OperationNode {
                        op: PlannedOp::CheckedI64Add,
                        inputs: vec![a_i64, b_i64],
                        output: Some(c_i64),
                        failure_replay: FailureReplayPolicy::local_fallback(
                            "overflow uses local generic fallback",
                        ),
                        failure: FailureMode::FallbackToPlan {
                            target: FallbackTarget::Region(RegionId(1)),
                            reason: FallbackReason("overflow uses generic add".to_string()),
                        },
                        cost: Cost::default(),
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
                    PlanNodeKind::Operation(OperationNode {
                        op: PlannedOp::I64CompareToBool01 {
                            op: RichCompareOp::Gt,
                        },
                        inputs: vec![c_i64, zero_i64],
                        output: Some(cmp_i32),
                        failure_replay: FailureReplayPolicy::safe("integer compare cannot fail"),
                        failure: FailureMode::CannotFail,
                        cost: Cost::default(),
                    }),
                ),
            ],
            exits: vec![RegionExitPlan {
                source: Some(instr_id(3)),
                kind: RegionExitKind::Branch {
                    condition: cmp_i32,
                    then_target: RegionExitTarget::OriginalCfg,
                    else_target: RegionExitTarget::OriginalCfg,
                },
            }],
        };

        let fallback_cmp = PlanValue::new(20, Rep::PyObjectOwned);
        let fallback_truth = PlanValue::new(21, Rep::I32Bool01);
        let fallback_region = RegionPlan {
            id: RegionId(1),
            source: RegionSource::Synthetic {
                reason: "generic fallback".to_string(),
            },
            inputs: vec![input(a_obj, 0, "a"), input(b_obj, 1, "b")],
            nodes: vec![
                node(
                    10,
                    PlanNodeKind::Operation(OperationNode {
                        op: PlannedOp::PyObjectRichCompare {
                            op: RichCompareOp::Gt,
                        },
                        inputs: vec![a_obj, b_obj],
                        output: Some(fallback_cmp),
                        failure_replay: FailureReplayPolicy::local_fallback(
                            "richcompare failure is raised by helper",
                        ),
                        failure: FailureMode::Raise(PythonExceptionSpec {
                            kind: "Exception".to_string(),
                            reason: "PyObject_RichCompare may raise".to_string(),
                        }),
                        cost: Cost::default(),
                    }),
                ),
                node(
                    11,
                    PlanNodeKind::Convert(ConvertNode {
                        input: fallback_cmp,
                        output: fallback_truth,
                        kind: ConversionKind::TruthinessToI32Bool01,
                        precondition: ConversionPrecondition::Infallible,
                        failure: FailureMode::Raise(PythonExceptionSpec {
                            kind: "Exception".to_string(),
                            reason: "PyObject_IsTrue may raise".to_string(),
                        }),
                        ownership: ConversionOwnership::ConsumeOwned,
                    }),
                ),
            ],
            exits: vec![RegionExitPlan {
                source: Some(instr_id(3)),
                kind: RegionExitKind::Branch {
                    condition: fallback_truth,
                    then_target: RegionExitTarget::OriginalCfg,
                    else_target: RegionExitTarget::OriginalCfg,
                },
            }],
        };

        module_with_regions(vec![hot_region, fallback_region])
    }

    #[test]
    fn valid_compact_int_branch_plan_validates() {
        let plan = valid_compact_int_plan();
        validate_module_plan_v3(&plan).unwrap();
    }

    #[test]
    fn scalar_thread_between_regions_validates() {
        let scalar = PlanValue::new(0, Rep::I64);
        let producer_region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: Vec::new(),
            nodes: vec![node(
                0,
                PlanNodeKind::Constant {
                    output: scalar,
                    constant: PlannedConstant::I64(3),
                },
            )],
            exits: Vec::new(),
        };
        let fallback_region = RegionPlan {
            id: RegionId(1),
            source: RegionSource::Synthetic {
                reason: "generic fallback".to_string(),
            },
            inputs: Vec::new(),
            nodes: Vec::new(),
            exits: Vec::new(),
        };
        let consumer_region = RegionPlan {
            id: RegionId(2),
            source: RegionSource::Synthetic {
                reason: "consumer".to_string(),
            },
            inputs: vec![RegionInput {
                value: scalar,
                source: RegionInputSource::CapturedValue {
                    from_region: RegionId(0),
                    value: scalar.id,
                },
            }],
            nodes: Vec::new(),
            exits: Vec::new(),
        };
        let thread = ScalarLocalThreadPlan {
            local: ScalarThreadLocal {
                name: "c".to_string(),
                location: ScalarThreadLocalLocation::Local { slot: 2 },
            },
            producer: RegionValueRef {
                region: RegionId(0),
                value: scalar,
            },
            consumer: RegionValueRef {
                region: RegionId(2),
                value: scalar,
            },
            fallback: ScalarThreadFallback::LocalFallbackRegion {
                region: RegionId(1),
                reason: "fallback preserves local store semantics".to_string(),
            },
            local_state: ScalarThreadLocalState::ScalarOnlyHotPath {
                cleanup: ScalarThreadLocalCleanup::NoPyObjectSlotOwnership,
                reason: "hot path keeps c as a scalar and never stores a PyObject".to_string(),
            },
            materialization: ScalarThreadMaterialization::DeferredUntilPythonObjectUse {
                reason: "only scalar consumers use the value".to_string(),
            },
            estimated_savings: Cost {
                hot_path: 1,
                ..Cost::default()
            },
            reason: "thread local c as i64".to_string(),
        };

        validate_module_plan_v3(&module_with_regions_and_scalar_threads(
            vec![producer_region, fallback_region, consumer_region],
            vec![thread],
        ))
        .unwrap();
    }

    #[test]
    fn scalar_thread_unknown_fallback_fails() {
        let scalar = PlanValue::new(0, Rep::I64);
        let producer_region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: Vec::new(),
            nodes: vec![node(
                0,
                PlanNodeKind::Constant {
                    output: scalar,
                    constant: PlannedConstant::I64(3),
                },
            )],
            exits: Vec::new(),
        };
        let consumer_region = RegionPlan {
            id: RegionId(2),
            source: RegionSource::Synthetic {
                reason: "consumer".to_string(),
            },
            inputs: vec![RegionInput {
                value: scalar,
                source: RegionInputSource::CapturedValue {
                    from_region: RegionId(0),
                    value: scalar.id,
                },
            }],
            nodes: Vec::new(),
            exits: Vec::new(),
        };
        let thread = ScalarLocalThreadPlan {
            local: ScalarThreadLocal {
                name: "c".to_string(),
                location: ScalarThreadLocalLocation::Local { slot: 2 },
            },
            producer: RegionValueRef {
                region: RegionId(0),
                value: scalar,
            },
            consumer: RegionValueRef {
                region: RegionId(2),
                value: scalar,
            },
            fallback: ScalarThreadFallback::LocalFallbackRegion {
                region: RegionId(9),
                reason: "fallback preserves local store semantics".to_string(),
            },
            local_state: ScalarThreadLocalState::ScalarOnlyHotPath {
                cleanup: ScalarThreadLocalCleanup::NoPyObjectSlotOwnership,
                reason: "hot path keeps c as a scalar and never stores a PyObject".to_string(),
            },
            materialization: ScalarThreadMaterialization::DeferredUntilPythonObjectUse {
                reason: "only scalar consumers use the value".to_string(),
            },
            estimated_savings: Cost::default(),
            reason: "thread local c as i64".to_string(),
        };

        let err = validate_module_plan_v3(&module_with_regions_and_scalar_threads(
            vec![producer_region, consumer_region],
            vec![thread],
        ))
        .unwrap_err();
        assert!(err.contains("unknown fallback region"), "{err}");
    }

    #[test]
    fn scalar_thread_producer_after_consumer_fails() {
        let scalar = PlanValue::new(0, Rep::I64);
        let consumer_region = RegionPlan {
            id: RegionId(2),
            source: RegionSource::Synthetic {
                reason: "consumer".to_string(),
            },
            inputs: vec![RegionInput {
                value: scalar,
                source: RegionInputSource::CapturedValue {
                    from_region: RegionId(0),
                    value: scalar.id,
                },
            }],
            nodes: Vec::new(),
            exits: Vec::new(),
        };
        let fallback_region = RegionPlan {
            id: RegionId(1),
            source: RegionSource::Synthetic {
                reason: "generic fallback".to_string(),
            },
            inputs: Vec::new(),
            nodes: Vec::new(),
            exits: Vec::new(),
        };
        let producer_region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: Vec::new(),
            nodes: vec![node(
                0,
                PlanNodeKind::Constant {
                    output: scalar,
                    constant: PlannedConstant::I64(3),
                },
            )],
            exits: Vec::new(),
        };
        let thread = ScalarLocalThreadPlan {
            local: ScalarThreadLocal {
                name: "c".to_string(),
                location: ScalarThreadLocalLocation::Local { slot: 2 },
            },
            producer: RegionValueRef {
                region: RegionId(0),
                value: scalar,
            },
            consumer: RegionValueRef {
                region: RegionId(2),
                value: scalar,
            },
            fallback: ScalarThreadFallback::LocalFallbackRegion {
                region: RegionId(1),
                reason: "fallback preserves local store semantics".to_string(),
            },
            local_state: ScalarThreadLocalState::ScalarOnlyHotPath {
                cleanup: ScalarThreadLocalCleanup::NoPyObjectSlotOwnership,
                reason: "hot path keeps c as a scalar and never stores a PyObject".to_string(),
            },
            materialization: ScalarThreadMaterialization::DeferredUntilPythonObjectUse {
                reason: "only scalar consumers use the value".to_string(),
            },
            estimated_savings: Cost::default(),
            reason: "thread local c as i64".to_string(),
        };

        let err = validate_module_plan_v3(&module_with_regions_and_scalar_threads(
            vec![consumer_region, fallback_region, producer_region],
            vec![thread],
        ))
        .unwrap_err();
        assert!(
            err.contains("requires producer region"),
            "expected scalar-thread ordering error, got {err}"
        );
    }

    #[test]
    fn branch_on_pyobject_fails() {
        let condition = PlanValue::new(0, Rep::PyObjectOwned);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![input(condition, 0, "value")],
            nodes: Vec::new(),
            exits: vec![RegionExitPlan {
                source: None,
                kind: RegionExitKind::Branch {
                    condition,
                    then_target: RegionExitTarget::OriginalCfg,
                    else_target: RegionExitTarget::OriginalCfg,
                },
            }],
        };
        let err = validate_module_plan_v3(&module_with_regions(vec![region])).unwrap_err();
        assert!(err.contains("branch exits require I32Bool01"), "{err}");
    }

    #[test]
    fn conversion_undefined_input_fails() {
        let output = PlanValue::new(1, Rep::I64);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: Vec::new(),
            nodes: vec![unbox(
                0,
                PlanValue::new(0, Rep::PyObjectBorrowed),
                output,
                99,
                0,
            )],
            exits: Vec::new(),
        };
        let err = validate_module_plan_v3(&module_with_regions(vec![region])).unwrap_err();
        assert!(err.contains("uses undefined value"), "{err}");
    }

    #[test]
    fn conversion_mismatched_reps_fail() {
        let input_value = PlanValue::new(0, Rep::I32Bool01);
        let output = PlanValue::new(1, Rep::I64);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![input(input_value, 0, "value")],
            nodes: vec![node(
                0,
                PlanNodeKind::Convert(ConvertNode {
                    input: input_value,
                    output,
                    kind: ConversionKind::FromPythonLongCompactToI64,
                    precondition: ConversionPrecondition::DominatingFacts {
                        reason: "test".to_string(),
                    },
                    failure: FailureMode::FallbackToPlan {
                        target: FallbackTarget::Region(RegionId(0)),
                        reason: FallbackReason("test".to_string()),
                    },
                    ownership: ConversionOwnership::BorrowInput,
                }),
            )],
            exits: Vec::new(),
        };
        let err = validate_module_plan_v3(&module_with_regions(vec![region])).unwrap_err();
        assert!(err.contains("expects input PyObjectBorrowed"), "{err}");
    }

    #[test]
    fn conversion_precondition_must_reference_dominating_guard() {
        let input_value = PlanValue::new(0, Rep::PyObjectBorrowed);
        let output = PlanValue::new(1, Rep::I64);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![input(input_value, 0, "value")],
            nodes: vec![
                node(
                    0,
                    PlanNodeKind::Constant {
                        output: PlanValue::new(2, Rep::I64),
                        constant: PlannedConstant::I64(1),
                    },
                ),
                node(
                    1,
                    PlanNodeKind::Convert(ConvertNode {
                        input: input_value,
                        output,
                        kind: ConversionKind::FromPythonLongCompactToI64,
                        precondition: ConversionPrecondition::SpecializationGuard {
                            guard: PlanNodeId(0),
                            reason: "not actually a guard".to_string(),
                        },
                        failure: FailureMode::FallbackToPlan {
                            target: FallbackTarget::Region(RegionId(0)),
                            reason: FallbackReason("test".to_string()),
                        },
                        ownership: ConversionOwnership::BorrowInput,
                    }),
                ),
            ],
            exits: Vec::new(),
        };
        let err = validate_module_plan_v3(&module_with_regions(vec![region])).unwrap_err();
        assert!(err.contains("non-dominating guard"), "{err}");
    }

    #[test]
    fn checked_i64_operation_requires_explicit_fallback() {
        let left = PlanValue::new(0, Rep::I64);
        let right = PlanValue::new(1, Rep::I64);
        let output = PlanValue::new(2, Rep::I64);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![input(left, 0, "left"), input(right, 1, "right")],
            nodes: vec![node(
                0,
                PlanNodeKind::Operation(OperationNode {
                    op: PlannedOp::CheckedI64Add,
                    inputs: vec![left, right],
                    output: Some(output),
                    failure_replay: FailureReplayPolicy::safe("invalid test replay policy"),
                    failure: FailureMode::CannotFail,
                    cost: Cost::default(),
                }),
            )],
            exits: Vec::new(),
        };
        let err = validate_module_plan_v3(&module_with_regions(vec![region])).unwrap_err();
        assert!(err.contains("explicit local fallback"), "{err}");
    }

    #[test]
    fn duplicate_output_value_fails() {
        let output = PlanValue::new(0, Rep::I64);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: Vec::new(),
            nodes: vec![
                node(
                    0,
                    PlanNodeKind::Constant {
                        output,
                        constant: PlannedConstant::I64(1),
                    },
                ),
                node(
                    1,
                    PlanNodeKind::Constant {
                        output,
                        constant: PlannedConstant::I64(2),
                    },
                ),
            ],
            exits: Vec::new(),
        };
        let err = validate_module_plan_v3(&module_with_regions(vec![region])).unwrap_err();
        assert!(err.contains("duplicate value"), "{err}");
    }

    #[test]
    fn deopt_without_replay_safe_reason_fails() {
        let input_value = PlanValue::new(0, Rep::PyObjectBorrowed);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: vec![input(input_value, 0, "value")],
            nodes: vec![node(
                0,
                PlanNodeKind::Guard(GuardNode {
                    inputs: vec![input_value],
                    guard: GuardSpec {
                        kind: GuardKind::SpecializationCheck,
                        replay: FailureReplayPolicy::local_fallback(
                            "local fallback required after side effect",
                        ),
                        description: "test guard".to_string(),
                    },
                    failure: GuardFailure::DeoptTo {
                        target: DeoptPointId(0),
                        reason: DeoptReason("test deopt".to_string()),
                    },
                }),
            )],
            exits: Vec::new(),
        };
        let err = validate_module_plan_v3(&module_with_regions(vec![region])).unwrap_err();
        assert!(
            err.contains("deopt failure requires replay-safe policy"),
            "{err}"
        );
    }

    #[test]
    fn return_i64_without_materialization_fails() {
        let value = PlanValue::new(0, Rep::I64);
        let region = RegionPlan {
            id: RegionId(0),
            source: RegionSource::FunctionEntry,
            inputs: Vec::new(),
            nodes: vec![node(
                0,
                PlanNodeKind::Constant {
                    output: value,
                    constant: PlannedConstant::I64(1),
                },
            )],
            exits: vec![RegionExitPlan {
                source: None,
                kind: RegionExitKind::Return { value },
            }],
        };
        let err = validate_module_plan_v3(&module_with_regions(vec![region])).unwrap_err();
        assert!(
            err.contains("return exits require a returnable PyObject"),
            "{err}"
        );
    }
}
