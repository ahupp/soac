use soac_core::block_py::{BlockPyFunction, ChildVisitable, RuntimeFunctionId, Visit};
use soac_ir_blockpy::{
    CodegenModuleShape, InstrCodegen, constructor_init_function_id_for_entry_function,
};
use soac_ir_typed::{InstrTyped, TypedCodegenModuleShape, TypedDirectCallableCallGuard};
use std::collections::HashSet;

use super::typed_pipeline::JitModulePlan;

pub(super) fn collect_call_direct_targets(
    function: &BlockPyFunction<CodegenModuleShape>,
) -> HashSet<RuntimeFunctionId> {
    let mut out = HashSet::new();
    if let Some(init_function_id) = constructor_init_function_id_for_entry_function(function) {
        out.insert(init_function_id);
    }
    out
}

pub(super) fn collect_typed_call_direct_targets(
    function: &BlockPyFunction<TypedCodegenModuleShape>,
) -> HashSet<RuntimeFunctionId> {
    struct CallDirectTargetCollector<'a> {
        out: &'a mut HashSet<RuntimeFunctionId>,
    }

    impl Visit<InstrTyped> for CallDirectTargetCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallDirect(call) = expr {
                self.out.insert(call.function_id);
            }
            if let InstrTyped::GuardedCallableCallTyped(call) = expr {
                self.out
                    .extend(call.function_guards.iter().map(|guard| guard.function_id));
            }
            if let InstrTyped::GuardedMethodCallTyped(call) = expr {
                self.out
                    .extend(call.method_guards.iter().map(|guard| guard.function_id));
            }
            if let InstrTyped::DirectCallableCallTyped(call) = expr {
                match &call.guard {
                    TypedDirectCallableCallGuard::Function(guard) => {
                        self.out.insert(guard.function_id);
                    }
                }
            }
            if let InstrTyped::DirectMethodCallTyped(call) = expr {
                self.out.insert(call.guard.function_id);
            }
            expr.visit_children(self);
        }
    }

    let mut out = HashSet::new();
    if let Some(init_function_id) = constructor_init_function_id_for_entry_function(function) {
        out.insert(init_function_id);
    }
    let mut collector = CallDirectTargetCollector { out: &mut out };
    collector.visit_fn(function);
    out
}

pub(super) fn collect_planned_typed_call_direct_targets(
    module_plan: &JitModulePlan,
    function_id: RuntimeFunctionId,
) -> Result<HashSet<RuntimeFunctionId>, String> {
    let planned_function = module_plan
        .module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .ok_or_else(|| {
            format!("planned JIT module is missing function {function_id} for direct-call targets")
        })?;
    Ok(collect_typed_call_direct_targets(planned_function))
}

pub(super) fn collect_make_function_targets(
    function: &BlockPyFunction<CodegenModuleShape>,
) -> HashSet<RuntimeFunctionId> {
    struct MakeFunctionTargetCollector<'a> {
        out: &'a mut HashSet<RuntimeFunctionId>,
    }

    impl Visit<InstrCodegen> for MakeFunctionTargetCollector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            if let InstrCodegen::MakeFunctionWithClosure(op) = expr {
                self.out.insert(op.function_id());
            }
            expr.visit_children(self);
        }
    }

    let mut out = HashSet::new();
    let mut collector = MakeFunctionTargetCollector { out: &mut out };
    collector.visit_fn(function);
    out
}

pub(crate) fn is_synthetic_class_helper_function(
    function: &BlockPyFunction<CodegenModuleShape>,
) -> bool {
    function.names.bind_name.starts_with("_dp_class_ns_")
        || function.names.bind_name.starts_with("_dp_define_class_")
}
