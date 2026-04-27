#![deny(unreachable_pub)]

pub(crate) mod block_py {
    pub(crate) use soac_core::block_py::*;
}

pub mod emit_v3;
pub mod plan_v3;
pub mod value_facts;

mod instr_id;
mod typed;

pub use instr_id::assign_missing_typed_function_instr_ids;
pub use typed::{
    InstrTyped, TypedAttrAccessPlan, TypedAttrOwnerRef, TypedBlock, TypedBlockExtra,
    TypedBlockLayoutHint, TypedCall, TypedCallAccessPlan, TypedCallEmissionPlan,
    TypedCallEmissionPlans, TypedCodegenModuleShape, TypedDirectCallArgPlan,
    TypedDirectCallArgSource, TypedDirectCallGuardTest, TypedDirectCallGuardTestKind,
    TypedDirectCallableCall, TypedDirectCallableCallGuard, TypedDirectFunctionCallGuard,
    TypedDirectMethodCall, TypedDirectMethodCallGuard, TypedExactIntBranchPlan,
    TypedExactIntPlanSource, TypedExactIntReturnPlan, TypedExactIntScalarThreadPlan,
    TypedExactListItemAccessPlan, TypedExactListItemCounterSource, TypedExactListItemPlanSource,
    TypedGetAttr, TypedGuardedCallableCall, TypedGuardedMethodCall, TypedIndexedFieldGuard,
    TypedIndexedFieldPlanSource, TypedIndexedGlobalAccessPlan, TypedIndexedGlobalPlanSource,
    TypedInstrExtra, TypedPlannedResult, TypedPyObjectOwnershipPlan, TypedResultDemand,
    TypedSetAttr, TypedTruthy, lower_codegen_function_to_typed, lower_codegen_module_to_typed,
};
pub use value_facts::{
    BoolFacts, BoolSingletonFact, CallableFact, EnvFacts, FactStore, I32Facts, I64Facts, NoneFact,
    ProvenanceFact, PyExactType, PyObjFacts, RefcountFact, RuntimeHelperId, RuntimeHelperSignature,
    RuntimeSingleton, ThrowSpec, TruthinessFact, TypeFact, ValueFacts,
};
