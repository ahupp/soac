use std::collections::{HashMap, HashSet};

use soac_core::block_py::{
    BlockPyFunction, CallArgKeyword, CallArgPositional, ChildVisitable, HasSemanticInstrId, Instr,
    InstrId, NameLocation, ParamKind, RuntimeFunctionId, RuntimeName, Visit, VisitMut,
};
use soac_lowering::passes::{
    CodegenModuleShape, InstrCodegen, InstrResolved, InstrTyped, TypedCall, TypedCallAccessPlan,
    TypedCodegenModuleShape, TypedDirectCallArgPlan, TypedDirectCallArgSource,
    TypedDirectConstructorCallGuard, TypedDirectFunctionCallGuard, TypedDirectMethodCallGuard,
};

use super::{
    DirectCallArgPlan, DirectCallArgSource, DirectCallIncompatibility, DirectEdgeStats,
    DirectFunctionSpecialization, DirectOwnerAttrKey, DirectOwnerAttrSpecialization,
    owner_attr_function_id_for_type_ref, reloc_type_ref_for_type,
    typed_attr_owner_ref_from_reloc_type_ref,
};
use crate::module_constants::{ModuleCodegenConstants, ModuleConstantId};

pub(super) struct CallSpecializationCtx<'a> {
    pub module: &'a soac_core::block_py::BlockPyModule<CodegenModuleShape>,
    pub direct_call_resolver: Option<&'a crate::module_type::SharedModuleState>,
    pub direct_call_target_functions:
        &'a HashMap<RuntimeFunctionId, BlockPyFunction<CodegenModuleShape>>,
    pub direct_owner_attr_specializations:
        Option<&'a HashMap<DirectOwnerAttrKey, Vec<DirectOwnerAttrSpecialization>>>,
    pub direct_edge_stats: &'a DirectEdgeStats,
}

pub(super) fn annotate_typed_call_accesses(
    function: &mut BlockPyFunction<TypedCodegenModuleShape>,
    ctx: &CallSpecializationCtx<'_>,
    call_target_specializations: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
) -> usize {
    struct Annotator<'a> {
        ctx: &'a CallSpecializationCtx<'a>,
        call_target_specializations: &'a HashMap<InstrId, Vec<RuntimeFunctionId>>,
        count: usize,
    }

    impl VisitMut<InstrTyped> for Annotator<'_> {
        fn visit_instr_mut(&mut self, expr: &mut InstrTyped) {
            expr.visit_children_mut(self);
            if let InstrTyped::CallTyped(op) = expr
                && let Some(instr_id) = op.try_semantic_instr_id()
                && let Some(targets) = self.call_target_specializations.get(&instr_id)
                && !targets.is_empty()
            {
                op.access =
                    guarded_typed_call_access_plan(op, self.ctx, targets).unwrap_or_else(|| {
                        if matches!(
                            op.func.as_ref(),
                            InstrTyped::GetAttrTyped(_) | InstrTyped::LegacyGetAttr(_)
                        ) {
                            TypedCallAccessPlan::ProfiledMethodTargets {
                                targets: targets.clone(),
                            }
                        } else {
                            TypedCallAccessPlan::ProfiledCallableTargets {
                                targets: targets.clone(),
                            }
                        }
                    });
                self.count += 1;
            } else if let InstrTyped::CallTyped(op) = expr
                && matches!(op.access, TypedCallAccessPlan::Generic)
                && let Some(access) = guarded_runtime_protocol_call_access_plan(
                    op,
                    self.ctx,
                    self.call_target_specializations,
                )
            {
                op.access = access;
                self.count += 1;
            }
        }
    }

    let mut annotator = Annotator {
        ctx,
        call_target_specializations,
        count: 0,
    };
    for block in &mut function.blocks {
        for instr in &mut block.body {
            annotator.visit_instr_mut(instr);
        }
        annotator.visit_term_mut(&mut block.term);
    }
    annotator.count
}

pub(super) fn collect_runtime_protocol_method_targets(
    function: &BlockPyFunction<CodegenModuleShape>,
    module_constants: &ModuleCodegenConstants,
    direct_owner_attr_specializations: Option<
        &HashMap<DirectOwnerAttrKey, Vec<DirectOwnerAttrSpecialization>>,
    >,
    call_target_specializations: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
) -> HashSet<RuntimeFunctionId> {
    struct Collector<'a> {
        module_constants: &'a ModuleCodegenConstants,
        direct_owner_attr_specializations:
            Option<&'a HashMap<DirectOwnerAttrKey, Vec<DirectOwnerAttrSpecialization>>>,
        call_target_specializations: &'a HashMap<InstrId, Vec<RuntimeFunctionId>>,
        out: &'a mut HashSet<RuntimeFunctionId>,
    }

    impl Visit<InstrCodegen> for Collector<'_> {
        fn visit_instr(&mut self, expr: &InstrCodegen) {
            if let InstrCodegen::Call(call) = expr {
                self.collect_call(call);
            }
            expr.visit_children(self);
        }
    }

    impl Collector<'_> {
        fn collect_call(&mut self, call: &soac_core::block_py::Call<InstrCodegen>) {
            if codegen_static_runtime_name(call.func.as_ref(), self.module_constants)
                != Some(RuntimeName::Iter)
            {
                return;
            }
            if !call.keywords.is_empty() || call.args.len() != 1 {
                return;
            }
            let CallArgPositional::Positional(receiver) = &call.args[0] else {
                return;
            };
            let Some(constructor_call) = codegen_receiver_constructor_call(receiver) else {
                return;
            };
            let Some(instr_id) = constructor_call.try_semantic_instr_id() else {
                return;
            };
            let Some(targets) = self.call_target_specializations.get(&instr_id) else {
                return;
            };
            for function_id in targets {
                for owner in direct_constructor_owner_attr_specializations_from_source(
                    self.direct_owner_attr_specializations,
                    *function_id,
                ) {
                    let Ok(Some(iter_function_id)) =
                        owner_attr_function_id_for_type_ref(&owner.owner_type_ref, "__iter__")
                    else {
                        continue;
                    };
                    self.out.insert(iter_function_id);
                }
            }
        }
    }

    let mut out = HashSet::new();
    let mut collector = Collector {
        module_constants,
        direct_owner_attr_specializations,
        call_target_specializations,
        out: &mut out,
    };
    collector.visit_fn(function);
    out
}

fn guarded_typed_call_access_plan(
    op: &TypedCall<InstrTyped>,
    ctx: &CallSpecializationCtx<'_>,
    targets: &[RuntimeFunctionId],
) -> Option<TypedCallAccessPlan> {
    if matches!(
        op.func.as_ref(),
        InstrTyped::GetAttrTyped(_) | InstrTyped::LegacyGetAttr(_)
    ) {
        let method_name = typed_method_call_name(ctx, op.func.as_ref())?;
        let shape = simple_call_shape(op.args.as_slice(), op.keywords.as_slice());
        let method_guards =
            direct_method_specializations_for_shape(ctx, targets, method_name.as_str(), &shape)
                .into_iter()
                .map(|guard| TypedDirectMethodCallGuard {
                    function_id: guard.function_id,
                    owner_type_ref: typed_attr_owner_ref_from_reloc_type_ref(&guard.owner_type_ref),
                    type_version: guard.type_version,
                    arg_plan: typed_direct_call_arg_plan(&guard.arg_plan),
                })
                .collect::<Vec<_>>();
        if method_guards.is_empty() {
            return None;
        }
        return Some(TypedCallAccessPlan::GuardedMethod {
            method_name,
            method_guards,
        });
    }

    let shape = simple_call_shape(op.args.as_slice(), op.keywords.as_slice());
    let constructor_guards = direct_constructor_specializations_for_shape(ctx, targets, &shape)
        .into_iter()
        .map(|guard| TypedDirectConstructorCallGuard {
            function_id: guard.function_id,
            owner_type_ref: typed_attr_owner_ref_from_reloc_type_ref(&guard.owner_type_ref),
            type_version: guard.type_version,
            arg_plan: typed_direct_call_arg_plan(&guard.arg_plan),
        })
        .collect::<Vec<_>>();
    let function_guards = direct_function_specializations_for_shape(
        ctx,
        targets,
        &shape,
        constructor_guards.is_empty(),
    )
    .into_iter()
    .map(|guard| TypedDirectFunctionCallGuard {
        function_id: guard.function_id,
        arg_plan: typed_direct_call_arg_plan(&guard.arg_plan),
    })
    .collect::<Vec<_>>();
    if constructor_guards.is_empty() && function_guards.is_empty() {
        return None;
    }
    Some(TypedCallAccessPlan::GuardedCallable {
        function_guards,
        constructor_guards,
    })
}

fn guarded_runtime_protocol_call_access_plan(
    op: &TypedCall<InstrTyped>,
    ctx: &CallSpecializationCtx<'_>,
    call_target_specializations: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
) -> Option<TypedCallAccessPlan> {
    if typed_static_runtime_name(ctx, op.func.as_ref())? != RuntimeName::Iter {
        return None;
    }
    if !op.keywords.is_empty() || op.args.len() != 1 {
        return None;
    }
    let CallArgPositional::Positional(receiver) = &op.args[0] else {
        return None;
    };
    let method_guards =
        runtime_iter_method_guards_for_receiver_expr(ctx, receiver, call_target_specializations);
    (!method_guards.is_empty()).then(|| TypedCallAccessPlan::GuardedRuntimeProtocolMethod {
        runtime_name: RuntimeName::Iter,
        method_name: "__iter__".to_string(),
        method_guards,
    })
}

fn runtime_iter_method_guards_for_receiver_expr(
    ctx: &CallSpecializationCtx<'_>,
    receiver: &InstrTyped,
    call_target_specializations: &HashMap<InstrId, Vec<RuntimeFunctionId>>,
) -> Vec<TypedDirectMethodCallGuard> {
    let Some(constructor_call) = receiver_constructor_call(receiver) else {
        return Vec::new();
    };
    let Some(instr_id) = constructor_call.try_semantic_instr_id() else {
        return Vec::new();
    };
    let Some(targets) = call_target_specializations.get(&instr_id) else {
        return Vec::new();
    };
    let shape = simple_call_shape(
        constructor_call.args.as_slice(),
        constructor_call.keywords.as_slice(),
    );
    direct_constructor_specializations_for_shape(ctx, targets, &shape)
        .into_iter()
        .filter_map(|constructor| {
            let function_id = match owner_attr_function_id_for_type_ref(
                &constructor.owner_type_ref,
                "__iter__",
            ) {
                Ok(Some(function_id)) => function_id,
                Ok(None) | Err(_) => return None,
            };
            let Some(target_function) = direct_call_target_function(ctx, function_id) else {
                ctx.direct_edge_stats
                    .record_profiled_missing_target_candidate();
                return None;
            };
            let arg_plan =
                match validate_direct_call_compatibility(target_function, 0, 1, false, false) {
                    Ok(arg_plan) => arg_plan,
                    Err(incompatibility) => {
                        record_profiled_direct_call_incompatibility(ctx, incompatibility);
                        return None;
                    }
                };
            Some(TypedDirectMethodCallGuard {
                function_id,
                owner_type_ref: typed_attr_owner_ref_from_reloc_type_ref(
                    &constructor.owner_type_ref,
                ),
                type_version: constructor.type_version,
                arg_plan: typed_direct_call_arg_plan(&arg_plan),
            })
        })
        .collect()
}

fn receiver_constructor_call(receiver: &InstrTyped) -> Option<&TypedCall<InstrTyped>> {
    match receiver {
        InstrTyped::CallTyped(call) => Some(call),
        _ => None,
    }
}

fn typed_static_runtime_name(
    ctx: &CallSpecializationCtx<'_>,
    expr: &InstrTyped,
) -> Option<RuntimeName> {
    match expr {
        InstrTyped::Load(load) if load.name.location.is_runtime_name() => {
            load.name.location.runtime_name_id()
        }
        InstrTyped::Load(load) => load
            .name
            .location
            .as_constant()
            .and_then(|index| ctx.module.module_constants.get(index as usize))
            .and_then(resolved_static_runtime_name),
        _ => None,
    }
}

fn resolved_static_runtime_name(expr: &InstrResolved) -> Option<RuntimeName> {
    match expr {
        InstrResolved::Load(load) if matches!(load.name.location, NameLocation::RuntimeName(_)) => {
            load.name.location.runtime_name_id()
        }
        _ => None,
    }
}

fn codegen_static_runtime_name(
    expr: &InstrCodegen,
    module_constants: &ModuleCodegenConstants,
) -> Option<RuntimeName> {
    match expr {
        InstrCodegen::Load(load) if load.name.location.is_runtime_name() => {
            load.name.location.runtime_name_id()
        }
        InstrCodegen::Load(load) => load
            .name
            .location
            .as_constant()
            .and_then(|index| {
                module_constants.constant_runtime_name_value(ModuleConstantId(index as usize))
            })
            .and_then(RuntimeName::from_name),
        _ => None,
    }
}

fn codegen_receiver_constructor_call(
    receiver: &InstrCodegen,
) -> Option<&soac_core::block_py::Call<InstrCodegen>> {
    match receiver {
        InstrCodegen::Call(call) => Some(call),
        _ => None,
    }
}

#[derive(Clone)]
struct DirectMethodSpecialization {
    function_id: RuntimeFunctionId,
    owner_type_ref: super::RelocTypeRef,
    type_version: u32,
    arg_plan: DirectCallArgPlan,
}

#[derive(Clone)]
struct DirectConstructorSpecialization {
    function_id: RuntimeFunctionId,
    owner_type_ref: super::RelocTypeRef,
    type_version: u32,
    arg_plan: DirectCallArgPlan,
}

fn typed_method_call_name(ctx: &CallSpecializationCtx<'_>, func: &InstrTyped) -> Option<String> {
    match func {
        InstrTyped::GetAttrTyped(getattr) => {
            super::typed_constant_string_value(ctx.module, getattr.attr.as_ref())
        }
        InstrTyped::LegacyGetAttr(getattr) => {
            super::typed_constant_string_value(ctx.module, getattr.attr.as_ref())
        }
        _ => None,
    }
    .map(str::to_string)
}

fn direct_method_specializations_for_shape(
    ctx: &CallSpecializationCtx<'_>,
    targets: &[RuntimeFunctionId],
    method_name: &str,
    shape: &SimpleCallShape,
) -> Vec<DirectMethodSpecialization> {
    if ctx.direct_call_resolver.is_none() && ctx.direct_owner_attr_specializations.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for function_id in targets.iter().copied() {
        let owners = direct_method_owner_attr_specializations(ctx, function_id, method_name);
        if owners.is_empty() {
            continue;
        }
        let Some(target_function) = direct_call_target_function(ctx, function_id) else {
            ctx.direct_edge_stats
                .record_profiled_missing_target_candidate();
            continue;
        };
        let arg_plan = match validate_direct_call_compatibility(
            target_function,
            shape.positional_arg_count,
            1,
            shape.has_starred_arguments,
            shape.has_keywords,
        ) {
            Ok(arg_plan) => arg_plan,
            Err(incompatibility) => {
                record_profiled_direct_call_incompatibility(ctx, incompatibility);
                continue;
            }
        };
        for owner in owners {
            out.push(DirectMethodSpecialization {
                function_id,
                owner_type_ref: owner.owner_type_ref,
                type_version: owner.type_version,
                arg_plan: arg_plan.clone(),
            });
        }
    }
    out
}

fn direct_constructor_specializations_for_shape(
    ctx: &CallSpecializationCtx<'_>,
    targets: &[RuntimeFunctionId],
    shape: &SimpleCallShape,
) -> Vec<DirectConstructorSpecialization> {
    if ctx.direct_call_resolver.is_none() && ctx.direct_owner_attr_specializations.is_none() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for function_id in targets.iter().copied() {
        let owners = direct_constructor_owner_attr_specializations(ctx, function_id);
        if owners.is_empty() {
            continue;
        }
        let Some(target_function) = direct_call_target_function(ctx, function_id) else {
            ctx.direct_edge_stats
                .record_profiled_missing_target_candidate();
            continue;
        };
        let arg_plan = match validate_direct_call_compatibility(
            target_function,
            shape.positional_arg_count,
            1,
            shape.has_starred_arguments,
            shape.has_keywords,
        ) {
            Ok(arg_plan) => arg_plan,
            Err(incompatibility) => {
                record_profiled_direct_call_incompatibility(ctx, incompatibility);
                continue;
            }
        };
        for owner in owners {
            out.push(DirectConstructorSpecialization {
                function_id,
                owner_type_ref: owner.owner_type_ref,
                type_version: owner.type_version,
                arg_plan: arg_plan.clone(),
            });
        }
    }
    out
}

fn direct_function_specializations_for_shape(
    ctx: &CallSpecializationCtx<'_>,
    targets: &[RuntimeFunctionId],
    shape: &SimpleCallShape,
    record_incompatibilities: bool,
) -> Vec<DirectFunctionSpecialization> {
    targets
        .iter()
        .copied()
        .filter_map(|function_id| {
            let Some(target_function) = direct_call_target_function(ctx, function_id) else {
                ctx.direct_edge_stats
                    .record_profiled_missing_target_candidate();
                return None;
            };
            if target_function.names.fn_name == "__init__" {
                return None;
            }
            let arg_plan = match validate_direct_call_compatibility(
                target_function,
                shape.positional_arg_count,
                0,
                shape.has_starred_arguments,
                shape.has_keywords,
            ) {
                Ok(arg_plan) => arg_plan,
                Err(incompatibility) => {
                    if record_incompatibilities {
                        record_profiled_direct_call_incompatibility(ctx, incompatibility);
                    }
                    return None;
                }
            };
            Some(DirectFunctionSpecialization {
                function_id,
                arg_plan,
            })
        })
        .collect()
}

fn direct_method_owner_attr_specializations(
    ctx: &CallSpecializationCtx<'_>,
    function_id: RuntimeFunctionId,
    method_name: &str,
) -> Vec<DirectOwnerAttrSpecialization> {
    let key = DirectOwnerAttrKey::new(function_id, method_name);
    if let Some(predeclared) = ctx.direct_owner_attr_specializations {
        return predeclared.get(&key).cloned().unwrap_or_default();
    }
    let Ok(owner_types) =
        (unsafe { crate::lookup_exact_owner_types_for_method(function_id, method_name) })
    else {
        return Vec::new();
    };
    owner_types
        .into_iter()
        .filter_map(|owner| {
            let Ok(Some(owner_type_ref)) = reloc_type_ref_for_type(owner.owner_type) else {
                return None;
            };
            Some(DirectOwnerAttrSpecialization {
                owner_type_ref,
                type_version: owner.type_version,
            })
        })
        .collect()
}

fn direct_constructor_owner_attr_specializations(
    ctx: &CallSpecializationCtx<'_>,
    function_id: RuntimeFunctionId,
) -> Vec<DirectOwnerAttrSpecialization> {
    direct_constructor_owner_attr_specializations_from_source(
        ctx.direct_owner_attr_specializations,
        function_id,
    )
}

pub(super) fn direct_constructor_owner_attr_specializations_from_source(
    direct_owner_attr_specializations: Option<
        &HashMap<DirectOwnerAttrKey, Vec<DirectOwnerAttrSpecialization>>,
    >,
    function_id: RuntimeFunctionId,
) -> Vec<DirectOwnerAttrSpecialization> {
    let key = DirectOwnerAttrKey::new(function_id, "__init__");
    if let Some(predeclared) = direct_owner_attr_specializations {
        return predeclared.get(&key).cloned().unwrap_or_default();
    }
    let Ok(owner_types) = (unsafe { crate::lookup_exact_owner_types_for_constructor(function_id) })
    else {
        return Vec::new();
    };
    owner_types
        .into_iter()
        .filter_map(|owner| {
            let Ok(Some(owner_type_ref)) = reloc_type_ref_for_type(owner.owner_type) else {
                return None;
            };
            Some(DirectOwnerAttrSpecialization {
                owner_type_ref,
                type_version: owner.type_version,
            })
        })
        .collect()
}

fn direct_call_target_function<'a>(
    ctx: &'a CallSpecializationCtx<'_>,
    function_id: RuntimeFunctionId,
) -> Option<&'a BlockPyFunction<CodegenModuleShape>> {
    ctx.module
        .callable_defs
        .iter()
        .find(|function| function.function_id == function_id)
        .or_else(|| ctx.direct_call_target_functions.get(&function_id))
}

fn validate_direct_call_compatibility(
    target_function: &BlockPyFunction<CodegenModuleShape>,
    explicit_positional_arg_count: usize,
    implicit_positional_arg_count: usize,
    has_starred_arguments: bool,
    has_keywords: bool,
) -> Result<DirectCallArgPlan, DirectCallIncompatibility> {
    if has_starred_arguments {
        return Err(DirectCallIncompatibility::StarredArguments);
    }
    if has_keywords {
        return Err(DirectCallIncompatibility::Keywords);
    }

    for param in target_function.params.iter() {
        if matches!(param.kind, ParamKind::VarArg | ParamKind::KwArg) {
            return Err(DirectCallIncompatibility::UnsupportedParameterKind { kind: param.kind });
        }
    }

    let provided_positional_arg_count =
        implicit_positional_arg_count + explicit_positional_arg_count;
    let accepted_positional_arg_count = target_function
        .params
        .iter()
        .filter(|param| matches!(param.kind, ParamKind::PosOnly | ParamKind::Any))
        .count();
    if provided_positional_arg_count > accepted_positional_arg_count {
        return Err(DirectCallIncompatibility::TooManyPositionalArguments {
            provided: provided_positional_arg_count,
            accepted: accepted_positional_arg_count,
        });
    }

    let mut sources = Vec::with_capacity(target_function.params.len());
    let mut next_provided_arg = 0usize;
    for param in target_function.params.iter() {
        match param.kind {
            ParamKind::PosOnly | ParamKind::Any => {
                if next_provided_arg < provided_positional_arg_count {
                    sources.push(DirectCallArgSource::Provided(next_provided_arg));
                    next_provided_arg += 1;
                } else if param.has_default {
                    sources.push(DirectCallArgSource::DefaultSentinel);
                } else {
                    return Err(DirectCallIncompatibility::MissingRequiredArgument);
                }
            }
            ParamKind::KwOnly => {
                if param.has_default {
                    sources.push(DirectCallArgSource::DefaultSentinel);
                } else {
                    return Err(DirectCallIncompatibility::MissingRequiredArgument);
                }
            }
            ParamKind::VarArg | ParamKind::KwArg => unreachable!(
                "unsupported variadic params should be rejected before planning direct-call args"
            ),
        }
    }
    debug_assert_eq!(next_provided_arg, provided_positional_arg_count);
    Ok(DirectCallArgPlan { sources })
}

fn record_profiled_direct_call_incompatibility(
    ctx: &CallSpecializationCtx<'_>,
    incompatibility: DirectCallIncompatibility,
) {
    match incompatibility {
        DirectCallIncompatibility::MissingRequiredArgument
        | DirectCallIncompatibility::TooManyPositionalArguments { .. } => {
            ctx.direct_edge_stats
                .record_profiled_arity_mismatch_candidate();
        }
        DirectCallIncompatibility::StarredArguments
        | DirectCallIncompatibility::Keywords
        | DirectCallIncompatibility::UnsupportedParameterKind { .. } => {
            ctx.direct_edge_stats
                .record_profiled_unsupported_shape_candidate();
        }
    }
}

struct SimpleCallShape {
    positional_arg_count: usize,
    has_starred_arguments: bool,
    has_keywords: bool,
}

fn simple_call_shape<E: Instr>(
    args: &[CallArgPositional<E>],
    keywords: &[CallArgKeyword<E>],
) -> SimpleCallShape {
    SimpleCallShape {
        positional_arg_count: args
            .iter()
            .filter(|arg| matches!(arg, CallArgPositional::Positional(_)))
            .count(),
        has_starred_arguments: args
            .iter()
            .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
            || keywords
                .iter()
                .any(|keyword| matches!(keyword, CallArgKeyword::Starred(_))),
        has_keywords: !keywords.is_empty(),
    }
}

fn typed_direct_call_arg_plan(plan: &DirectCallArgPlan) -> TypedDirectCallArgPlan {
    TypedDirectCallArgPlan {
        sources: plan
            .sources
            .iter()
            .map(|source| match source {
                DirectCallArgSource::Provided(index) => TypedDirectCallArgSource::Provided(*index),
                DirectCallArgSource::DefaultSentinel => TypedDirectCallArgSource::DefaultSentinel,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soac_core::block_py::Visit;
    use soac_lowering::passes::{TypedDirectCallArgSource, lower_codegen_module_to_typed};
    use soac_profile::CounterDumpTypeKey;

    fn lowered_module(source: &str) -> soac_core::block_py::BlockPyModule<CodegenModuleShape> {
        soac_lowering::lower_python_to_blockpy_for_testing(source)
            .expect("test source should lower")
            .codegen_module
    }

    fn function_by_qualname<'a>(
        module: &'a soac_core::block_py::BlockPyModule<CodegenModuleShape>,
        qualname: &str,
    ) -> &'a BlockPyFunction<CodegenModuleShape> {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing lowered function {qualname}"))
    }

    fn typed_function_by_qualname<'a>(
        module: &'a soac_core::block_py::BlockPyModule<TypedCodegenModuleShape>,
        qualname: &str,
    ) -> &'a BlockPyFunction<TypedCodegenModuleShape> {
        module
            .callable_defs
            .iter()
            .find(|function| function.names.qualname == qualname)
            .unwrap_or_else(|| panic!("missing typed function {qualname}"))
    }

    #[derive(Default)]
    struct TypedCallAccessCollector {
        calls: Vec<(InstrId, TypedCallAccessPlan)>,
    }

    impl Visit<InstrTyped> for TypedCallAccessCollector {
        fn visit_instr(&mut self, expr: &InstrTyped) {
            if let InstrTyped::CallTyped(call) = expr {
                self.calls.push((
                    call.try_semantic_instr_id()
                        .expect("typed call should carry semantic instruction id"),
                    call.access.clone(),
                ));
            }
            expr.visit_children(self);
        }
    }

    fn typed_call_accesses(
        function: &BlockPyFunction<TypedCodegenModuleShape>,
    ) -> Vec<(InstrId, TypedCallAccessPlan)> {
        let mut collector = TypedCallAccessCollector::default();
        collector.visit_fn(function);
        collector.calls
    }

    fn annotate_caller_for_single_target(
        source: &str,
    ) -> (
        usize,
        Vec<(InstrId, TypedCallAccessPlan)>,
        RuntimeFunctionId,
    ) {
        let module = lowered_module(source);
        let callee_id = function_by_qualname(&module, "callee").function_id;
        let mut typed_module = lower_codegen_module_to_typed(module.clone());
        let caller_index = typed_module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("caller should lower");
        let call_instr_id = typed_call_accesses(&typed_module.callable_defs[caller_index])
            .into_iter()
            .next()
            .expect("caller should contain a call")
            .0;

        let direct_call_target_functions = HashMap::new();
        let direct_edge_stats = DirectEdgeStats::default();
        let ctx = CallSpecializationCtx {
            module: &module,
            direct_call_resolver: None,
            direct_call_target_functions: &direct_call_target_functions,
            direct_owner_attr_specializations: None,
            direct_edge_stats: &direct_edge_stats,
        };
        let call_target_specializations = HashMap::from([(call_instr_id, vec![callee_id])]);
        let annotated = annotate_typed_call_accesses(
            &mut typed_module.callable_defs[caller_index],
            &ctx,
            &call_target_specializations,
        );
        let caller = typed_function_by_qualname(&typed_module, "caller");
        (annotated, typed_call_accesses(caller), callee_id)
    }

    #[test]
    fn annotates_profiled_function_target_as_guarded_callable() {
        let (annotated, calls, callee_id) = annotate_caller_for_single_target(
            "def callee(a):\n    return a\n\ndef caller(fn, x):\n    return fn(x)\n",
        );

        assert_eq!(annotated, 1);
        assert_eq!(calls.len(), 1);
        let (_, access) = &calls[0];
        let TypedCallAccessPlan::GuardedCallable {
            function_guards,
            constructor_guards,
        } = access
        else {
            panic!("expected profiled ordinary call to become a guarded callable plan");
        };
        assert!(constructor_guards.is_empty());
        assert_eq!(function_guards.len(), 1);
        assert_eq!(function_guards[0].function_id, callee_id);
        assert_eq!(
            function_guards[0].arg_plan.sources,
            vec![TypedDirectCallArgSource::Provided(0)]
        );
    }

    #[test]
    fn keeps_profiled_callable_targets_when_direct_call_shape_is_incompatible() {
        let (annotated, calls, callee_id) = annotate_caller_for_single_target(
            "def callee(a):\n    return a\n\ndef caller(fn):\n    return fn(1, 2)\n",
        );

        assert_eq!(annotated, 1);
        assert_eq!(calls.len(), 1);
        let (_, access) = &calls[0];
        assert_eq!(
            access,
            &TypedCallAccessPlan::ProfiledCallableTargets {
                targets: vec![callee_id],
            }
        );
    }

    #[test]
    fn annotates_profiled_method_target_as_guarded_method_from_predeclared_owner_attr() {
        let module = lowered_module(
            "class IterRange:\n    def __next__(self):\n        return 1\n\n\
def caller(it):\n    return it.__next__()\n",
        );
        let next_id = function_by_qualname(&module, "IterRange.__next__").function_id;
        let mut typed_module = lower_codegen_module_to_typed(module.clone());
        let caller_index = typed_module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("caller should lower");
        let call_instr_id = typed_call_accesses(&typed_module.callable_defs[caller_index])
            .into_iter()
            .next()
            .expect("caller should contain a call")
            .0;

        let direct_call_target_functions = HashMap::new();
        let direct_edge_stats = DirectEdgeStats::default();
        let direct_owner_attr_specializations = HashMap::from([(
            DirectOwnerAttrKey::new(next_id, "__next__"),
            vec![DirectOwnerAttrSpecialization {
                owner_type_ref: super::super::RelocTypeRef::TypeKey(CounterDumpTypeKey {
                    module_name: "__main__".to_string(),
                    qualname: "IterRange".to_string(),
                }),
                type_version: 7,
            }],
        )]);
        let ctx = CallSpecializationCtx {
            module: &module,
            direct_call_resolver: None,
            direct_call_target_functions: &direct_call_target_functions,
            direct_owner_attr_specializations: Some(&direct_owner_attr_specializations),
            direct_edge_stats: &direct_edge_stats,
        };
        let call_target_specializations = HashMap::from([(call_instr_id, vec![next_id])]);
        let annotated = annotate_typed_call_accesses(
            &mut typed_module.callable_defs[caller_index],
            &ctx,
            &call_target_specializations,
        );
        let caller = typed_function_by_qualname(&typed_module, "caller");
        let calls = typed_call_accesses(caller);

        assert_eq!(annotated, 1);
        assert_eq!(calls.len(), 1);
        let (_, access) = &calls[0];
        let TypedCallAccessPlan::GuardedMethod {
            method_name,
            method_guards,
        } = access
        else {
            panic!("expected profiled method call to become a guarded method plan");
        };
        assert_eq!(method_name, "__next__");
        assert_eq!(method_guards.len(), 1);
        assert_eq!(method_guards[0].function_id, next_id);
        assert_eq!(method_guards[0].type_version, 7);
        assert_eq!(
            method_guards[0].arg_plan.sources,
            vec![TypedDirectCallArgSource::Provided(0)]
        );
    }

    #[test]
    fn annotates_profiled_constructor_target_as_guarded_callable_from_predeclared_owner_attr() {
        let module = lowered_module(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(cls, value):\n    return cls(value)\n",
        );
        let init_id = function_by_qualname(&module, "Box.__init__").function_id;
        let mut typed_module = lower_codegen_module_to_typed(module.clone());
        let caller_index = typed_module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("caller should lower");
        let call_instr_id = typed_call_accesses(&typed_module.callable_defs[caller_index])
            .into_iter()
            .next()
            .expect("caller should contain a call")
            .0;

        let direct_call_target_functions = HashMap::new();
        let direct_edge_stats = DirectEdgeStats::default();
        let direct_owner_attr_specializations = HashMap::from([(
            DirectOwnerAttrKey::new(init_id, "__init__"),
            vec![DirectOwnerAttrSpecialization {
                owner_type_ref: super::super::RelocTypeRef::TypeKey(CounterDumpTypeKey {
                    module_name: "__main__".to_string(),
                    qualname: "Box".to_string(),
                }),
                type_version: 11,
            }],
        )]);
        let ctx = CallSpecializationCtx {
            module: &module,
            direct_call_resolver: None,
            direct_call_target_functions: &direct_call_target_functions,
            direct_owner_attr_specializations: Some(&direct_owner_attr_specializations),
            direct_edge_stats: &direct_edge_stats,
        };
        let call_target_specializations = HashMap::from([(call_instr_id, vec![init_id])]);
        let annotated = annotate_typed_call_accesses(
            &mut typed_module.callable_defs[caller_index],
            &ctx,
            &call_target_specializations,
        );
        let caller = typed_function_by_qualname(&typed_module, "caller");
        let calls = typed_call_accesses(caller);

        assert_eq!(annotated, 1);
        assert_eq!(calls.len(), 1);
        let (_, access) = &calls[0];
        let TypedCallAccessPlan::GuardedCallable {
            function_guards,
            constructor_guards,
        } = access
        else {
            panic!("expected profiled constructor call to become a guarded callable plan");
        };
        assert!(function_guards.is_empty());
        assert_eq!(constructor_guards.len(), 1);
        assert_eq!(constructor_guards[0].function_id, init_id);
        assert_eq!(constructor_guards[0].type_version, 11);
        assert_eq!(
            constructor_guards[0].arg_plan.sources,
            vec![
                TypedDirectCallArgSource::Provided(0),
                TypedDirectCallArgSource::Provided(1),
            ]
        );
    }

    #[test]
    fn profiled_init_target_without_constructor_owner_is_not_an_arity_mismatch() {
        let module = lowered_module(
            "class Box:\n    def __init__(self, value):\n        self.value = value\n\n\
def caller(cls, value):\n    return cls(value)\n",
        );
        let init_id = function_by_qualname(&module, "Box.__init__").function_id;
        let mut typed_module = lower_codegen_module_to_typed(module.clone());
        let caller_index = typed_module
            .callable_defs
            .iter()
            .position(|function| function.names.qualname == "caller")
            .expect("caller should lower");
        let call_instr_id = typed_call_accesses(&typed_module.callable_defs[caller_index])
            .into_iter()
            .next()
            .expect("caller should contain a call")
            .0;

        let direct_call_target_functions = HashMap::new();
        let direct_edge_stats = DirectEdgeStats::default();
        let ctx = CallSpecializationCtx {
            module: &module,
            direct_call_resolver: None,
            direct_call_target_functions: &direct_call_target_functions,
            direct_owner_attr_specializations: None,
            direct_edge_stats: &direct_edge_stats,
        };
        let annotated = annotate_typed_call_accesses(
            &mut typed_module.callable_defs[caller_index],
            &ctx,
            &HashMap::from([(call_instr_id, vec![init_id])]),
        );

        assert_eq!(annotated, 1);
        assert_eq!(
            direct_edge_stats.profiled_arity_mismatch_candidates.get(),
            0
        );
        let calls = typed_call_accesses(typed_function_by_qualname(&typed_module, "caller"));
        assert_eq!(
            calls[0].1,
            TypedCallAccessPlan::ProfiledCallableTargets {
                targets: vec![init_id],
            }
        );
    }
}
