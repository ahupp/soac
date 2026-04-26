use super::codegen_env::{FuncBuildImports, JitCodegenEnv, declare_import_fn, declare_local_fn};
use super::imports::{DP_JIT_RAISE_MISSING_REQUIRED_ARGUMENT_IMPORT, ModuleFuncImports};
use super::runtime_context::{FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET, FunctionRuntimeDataLayout};
use super::symbols::{default_direct_function_symbol, direct_function_symbol};
use super::{DeclaredJitFunction, block_arg_values, emit_function_data_slot_borrowed};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::FuncId;
use soac_core::block_py::{BlockPyFunction, ModuleShape, ParamKind, RuntimeFunctionId};
use std::cell::Cell;
use std::collections::HashMap;
use tracing::info;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DirectCallArgPlan {
    pub(super) sources: Vec<DirectCallArgSource>,
}

impl DirectCallArgPlan {
    pub(super) fn len(&self) -> usize {
        self.sources.len()
    }

    pub(super) fn requires_default_resolving_entry(&self) -> bool {
        self.sources
            .iter()
            .any(|source| matches!(source, DirectCallArgSource::DefaultSentinel))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DirectCallArgSource {
    Provided(usize),
    DefaultSentinel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DirectCallIncompatibility {
    StarredArguments,
    Keywords,
    UnsupportedParameterKind { kind: ParamKind },
    MissingRequiredArgument,
    TooManyPositionalArguments { provided: usize, accepted: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DirectCallEntryKind {
    Core,
    DefaultResolving,
}

#[derive(Default)]
pub(super) struct DirectEdgeStats {
    clif_direct_edges: Cell<usize>,
    function_env_indirect_edges: Cell<usize>,
    guarded_generic_fallback_blocks: Cell<usize>,
    profiled_missing_target_candidates: Cell<usize>,
    profiled_arity_mismatch_candidates: Cell<usize>,
    profiled_unsupported_shape_candidates: Cell<usize>,
}

impl DirectEdgeStats {
    fn increment(cell: &Cell<usize>) {
        cell.set(cell.get() + 1);
    }

    pub(super) fn record_resolved_direct_edge(&self) {
        Self::increment(&self.clif_direct_edges);
    }

    pub(super) fn record_function_env_indirect_edge(&self) {
        Self::increment(&self.function_env_indirect_edges);
    }

    pub(super) fn record_guarded_generic_fallback_block(&self) {
        Self::increment(&self.guarded_generic_fallback_blocks);
    }

    fn record_profiled_arity_mismatch_candidate(&self) {
        Self::increment(&self.profiled_arity_mismatch_candidates);
    }

    pub(super) fn record_profiled_missing_target_candidate(&self) {
        Self::increment(&self.profiled_missing_target_candidates);
    }

    fn record_profiled_unsupported_shape_candidate(&self) {
        Self::increment(&self.profiled_unsupported_shape_candidates);
    }

    fn total(&self) -> usize {
        self.clif_direct_edges.get()
            + self.function_env_indirect_edges.get()
            + self.guarded_generic_fallback_blocks.get()
            + self.profiled_missing_target_candidates.get()
            + self.profiled_arity_mismatch_candidates.get()
            + self.profiled_unsupported_shape_candidates.get()
    }

    pub(super) fn emit_trace(
        &self,
        module_name: &str,
        function: &BlockPyFunction<impl ModuleShape>,
    ) {
        if self.total() == 0 {
            return;
        }
        let clif_direct_edges = self.clif_direct_edges.get();
        let function_env_indirect_edges = self.function_env_indirect_edges.get();
        let guarded_generic_fallback_blocks = self.guarded_generic_fallback_blocks.get();
        let profiled_missing_target_candidates = self.profiled_missing_target_candidates.get();
        let profiled_arity_mismatch_candidates = self.profiled_arity_mismatch_candidates.get();
        let profiled_unsupported_shape_candidates =
            self.profiled_unsupported_shape_candidates.get();
        let generic_fallback_edges = function_env_indirect_edges
            + guarded_generic_fallback_blocks
            + profiled_missing_target_candidates
            + profiled_arity_mismatch_candidates
            + profiled_unsupported_shape_candidates;
        info!(
            target: "soac_jit_direct_edges",
            module = module_name,
            function_id = %function.function_id,
            qualname = %function.names.qualname,
            clif_direct_edges,
            function_env_indirect_edges,
            generic_fallback_edges,
            guarded_generic_fallback_blocks,
            profiled_missing_target_candidates,
            profiled_arity_mismatch_candidates,
            profiled_unsupported_shape_candidates,
            "soac_jit_direct_edges"
        );
    }
}

pub(super) fn plan_direct_call_args_for_target<P: ModuleShape>(
    target_function: &BlockPyFunction<P>,
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

pub(super) fn function_has_default_resolving_direct_entry(
    function: &BlockPyFunction<impl ModuleShape>,
) -> bool {
    // The adapter is also needed for parameters without source defaults:
    // __defaults__ / __kwdefaults__ can be assigned after function creation.
    function.params.iter().any(|param| {
        matches!(
            param.kind,
            ParamKind::PosOnly | ParamKind::Any | ParamKind::KwOnly
        )
    })
}

fn param_runtime_default_slot(
    layout: &FunctionRuntimeDataLayout,
    param: &soac_core::block_py::Param,
    param_index: usize,
) -> Option<usize> {
    match param.kind {
        ParamKind::PosOnly | ParamKind::Any => {
            layout.positional_default_slot_for_param_index(param_index)
        }
        ParamKind::KwOnly => layout.kwonly_default_slot(&param.name),
        ParamKind::VarArg | ParamKind::KwArg => None,
    }
}

pub(super) fn validate_direct_call_compatibility(
    target_function: &BlockPyFunction<impl ModuleShape>,
    _direct_call_functions: &HashMap<RuntimeFunctionId, DeclaredJitFunction>,
    explicit_positional_arg_count: usize,
    implicit_positional_arg_count: usize,
    has_starred_arguments: bool,
    has_keywords: bool,
) -> Result<DirectCallArgPlan, DirectCallIncompatibility> {
    plan_direct_call_args_for_target(
        target_function,
        explicit_positional_arg_count,
        implicit_positional_arg_count,
        has_starred_arguments,
        has_keywords,
    )
}

pub(super) fn record_profiled_direct_call_incompatibility(
    stats: &DirectEdgeStats,
    incompatibility: DirectCallIncompatibility,
) {
    match incompatibility {
        DirectCallIncompatibility::MissingRequiredArgument
        | DirectCallIncompatibility::TooManyPositionalArguments { .. } => {
            stats.record_profiled_arity_mismatch_candidate();
        }
        DirectCallIncompatibility::StarredArguments
        | DirectCallIncompatibility::Keywords
        | DirectCallIncompatibility::UnsupportedParameterKind { .. } => {
            stats.record_profiled_unsupported_shape_candidate();
        }
    }
}

pub(super) fn make_direct_function_signature(
    codegen_env: &impl JitCodegenEnv,
    function: &BlockPyFunction<impl ModuleShape>,
) -> ir::Signature {
    let ptr_ty = codegen_env.codegen_target_config().pointer_type();
    let mut sig = codegen_env.codegen_make_signature();
    sig.params.push(ir::AbiParam::new(ptr_ty));
    sig.params.push(ir::AbiParam::new(ptr_ty));
    for _ in function.params.iter() {
        sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    sig.returns.push(ir::AbiParam::new(ptr_ty));
    sig
}

pub(super) fn declare_direct_function(
    codegen_env: &mut impl JitCodegenEnv,
    function: &BlockPyFunction<impl ModuleShape>,
    symbol_scope: Option<&str>,
) -> Result<(ir::Signature, DeclaredJitFunction), String> {
    let sig = make_direct_function_signature(codegen_env, function);
    let symbol = direct_function_symbol(function, symbol_scope);
    let func_id = declare_local_fn(codegen_env, &symbol, &sig)?;
    let (default_func_id, default_symbol) = if function_has_default_resolving_direct_entry(function)
    {
        let default_symbol = default_direct_function_symbol(function, symbol_scope);
        (
            Some(declare_local_fn(codegen_env, &default_symbol, &sig)?),
            Some(default_symbol),
        )
    } else {
        (None, None)
    };
    Ok((
        sig,
        DeclaredJitFunction {
            func_id,
            default_func_id,
            symbol,
            default_symbol,
        },
    ))
}

pub(super) fn declare_imported_direct_function(
    codegen_env: &mut impl JitCodegenEnv,
    function: &BlockPyFunction<impl ModuleShape>,
    symbol_scope: &str,
) -> Result<DeclaredJitFunction, String> {
    let sig = make_direct_function_signature(codegen_env, function);
    let symbol = direct_function_symbol(function, Some(symbol_scope));
    let func_id = declare_import_fn(codegen_env, &symbol, &sig)?;
    let (default_func_id, default_symbol) = if function_has_default_resolving_direct_entry(function)
    {
        let default_symbol = default_direct_function_symbol(function, Some(symbol_scope));
        (
            Some(declare_import_fn(codegen_env, &default_symbol, &sig)?),
            Some(default_symbol),
        )
    } else {
        (None, None)
    };
    Ok(DeclaredJitFunction {
        func_id,
        default_func_id,
        symbol,
        default_symbol,
    })
}

pub(super) fn build_default_resolving_direct_adapter(
    codegen_env: &mut impl JitCodegenEnv,
    function: &BlockPyFunction<impl ModuleShape>,
    core_func_id: FuncId,
    adapter_func_id: FuncId,
) -> Result<cranelift_codegen::Context, String> {
    let ptr_ty = codegen_env.codegen_target_config().pointer_type();
    let runtime_layout = FunctionRuntimeDataLayout::from_parts(function, 0);
    let mut module_imports = ModuleFuncImports::new();
    let mut ctx = codegen_env.codegen_make_context();
    ctx.func.signature = make_direct_function_signature(codegen_env, function);
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry_block = fb.create_block();
        fb.append_block_params_for_function_params(entry_block);
        fb.switch_to_block(entry_block);
        fb.seal_block(entry_block);

        let entry_params = fb.block_params(entry_block).to_vec();
        let function_env_value = entry_params[0];
        let thread_state_value = entry_params[1];
        let direct_entry_args = &entry_params[2..];
        let function_data_value = fb.ins().iadd_imm(
            function_env_value,
            i64::from(FUNCTION_ENV_RUNTIME_OBJECTS_OFFSET),
        );
        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let raise_missing_ref = FuncBuildImports::new(&mut module_imports).get_or_panic(
            codegen_env,
            &mut fb.func,
            &DP_JIT_RAISE_MISSING_REQUIRED_ARGUMENT_IMPORT,
        );
        let missing_block = fb.create_block();
        let call_core_block = fb.create_block();
        for _ in function.params.iter() {
            fb.append_block_param(call_core_block, ptr_ty);
        }

        let mut selected_args = Vec::with_capacity(function.params.len());
        for (param_index, (param, arg_value)) in function
            .params
            .iter()
            .zip(direct_entry_args.iter().copied())
            .enumerate()
        {
            let Some(default_slot) =
                param_runtime_default_slot(&runtime_layout, param, param_index)
            else {
                let is_missing = fb
                    .ins()
                    .icmp(ir::condcodes::IntCC::Equal, arg_value, null_ptr);
                let present_block = fb.create_block();
                fb.ins()
                    .brif(is_missing, missing_block, &[], present_block, &[]);
                fb.switch_to_block(present_block);
                selected_args.push(arg_value);
                continue;
            };

            let is_missing = fb
                .ins()
                .icmp(ir::condcodes::IntCC::Equal, arg_value, null_ptr);
            let use_default_block = fb.create_block();
            let use_arg_block = fb.create_block();
            let after_block = fb.create_block();
            fb.append_block_param(after_block, ptr_ty);
            fb.ins()
                .brif(is_missing, use_default_block, &[], use_arg_block, &[]);

            fb.switch_to_block(use_default_block);
            let default_value = emit_function_data_slot_borrowed(
                &mut fb,
                function_data_value,
                default_slot,
                ptr_ty,
            );
            let default_is_missing =
                fb.ins()
                    .icmp(ir::condcodes::IntCC::Equal, default_value, null_ptr);
            let default_ok_block = fb.create_block();
            fb.ins().brif(
                default_is_missing,
                missing_block,
                &[],
                default_ok_block,
                &[],
            );
            fb.switch_to_block(default_ok_block);
            fb.ins()
                .jump(after_block, &[ir::BlockArg::Value(default_value)]);

            fb.switch_to_block(use_arg_block);
            fb.ins()
                .jump(after_block, &[ir::BlockArg::Value(arg_value)]);

            fb.switch_to_block(after_block);
            selected_args.push(fb.block_params(after_block)[0]);
        }
        fb.ins()
            .jump(call_core_block, &block_arg_values(&selected_args));
        fb.seal_block(call_core_block);

        fb.switch_to_block(call_core_block);
        let mut call_args = Vec::with_capacity(function.params.len() + 2);
        call_args.push(function_env_value);
        call_args.push(thread_state_value);
        call_args.extend(fb.block_params(call_core_block).iter().copied());
        let core_func_ref = codegen_env.codegen_declare_func_in_func(core_func_id, &mut fb.func)?;
        let call_inst = fb.ins().call(core_func_ref, &call_args);
        let result = fb.inst_results(call_inst)[0];
        fb.ins().return_(&[result]);

        fb.seal_block(missing_block);
        fb.switch_to_block(missing_block);
        fb.ins().call(raise_missing_ref, &[]);
        fb.ins().return_(&[null_ptr]);

        fb.seal_all_blocks();
        fb.finalize();
    }
    let _ = adapter_func_id;
    Ok(ctx)
}
