#![deny(unreachable_pub)]

pub(crate) mod block_py {
    pub(crate) use soac_core::block_py::*;
}

pub mod emit_v3;
pub mod plan_v3;
pub mod value_facts;

mod instr_id;
mod native_iterator;
mod typed;

pub use instr_id::assign_missing_typed_function_instr_ids;
pub use native_iterator::{
    NativeIteratorBuiltin, NativeIteratorCallee, NativeIteratorCalleeGuard,
    NativeIteratorMaterializer, NativeIteratorMustEliminate, NativeIteratorStage,
    TypedNativeIteratorPipelinePlan,
};
pub use typed::{
    InstrTyped, TypedAttrAccessPlan, TypedAttrOwnerRef, TypedBlock, TypedBlockExtra,
    TypedBlockLayoutHint, TypedBlockPyModuleShape, TypedBuiltinImplementationPlan, TypedCall,
    TypedCallAccessPlan, TypedCallEmissionPlan, TypedCallEmissionPlans, TypedConstructorInitPlan,
    TypedConstructorInitPlanSource, TypedDirectCallArgPlan, TypedDirectCallArgSource,
    TypedDirectCallGuardTest, TypedDirectCallGuardTestKind, TypedDirectCallableCall,
    TypedDirectCallableCallGuard, TypedDirectFunctionCallGuard, TypedDirectMethodCall,
    TypedDirectMethodCallGuard, TypedExactFloatExpressionPlan, TypedExactIntBranchPlan,
    TypedExactIntPlanSource, TypedExactIntReturnPlan, TypedExactListItemAccessPlan,
    TypedExactListItemCounterSource, TypedExactListItemPlanSource, TypedGeneratorInstancePlan,
    TypedGeneratorResumePlan, TypedGetAttr, TypedGuardedCallableCall, TypedGuardedMethodCall,
    TypedIndexedFieldCounterSource, TypedIndexedFieldGuard, TypedIndexedFieldPlanSource,
    TypedIndexedGlobalAccessPlan, TypedIndexedGlobalPlanSource, TypedInstrExtra,
    TypedLateBoundOwnerFieldPlan, TypedOpaqueFusedEntryGuard, TypedOpaqueFusedGuardExpectation,
    TypedOpaqueFusedGuardOperand, TypedOpaqueFusedIterationPlan, TypedOpaqueFusedResult,
    TypedPlannedResult, TypedPyObjectOwnershipPlan, TypedResultDemand, TypedSealedFieldAccessPlan,
    TypedSealedMethodAccessPlan, TypedSetAttr, TypedSourceBodyTarget, TypedSourceCallPlan,
    TypedTruthy, lower_blockpy_function_to_typed, lower_blockpy_module_to_typed,
};
pub use value_facts::{
    BoolFacts, BoolSingletonFact, CallableFact, EnvFacts, FactStore, I32Facts, I64Facts, NoneFact,
    ProvenanceFact, PyExactType, PyObjFacts, RefcountFact, RuntimeHelperId, RuntimeHelperSignature,
    RuntimeSingleton, ThrowSpec, TruthinessFact, TypeFact, ValueFacts,
};
