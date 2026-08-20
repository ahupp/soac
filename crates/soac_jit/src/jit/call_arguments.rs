//! Mechanical emission of source-selected call preparation and Operand moves.

use super::call_arguments_runtime as runtime;
use super::{
    CallEmission, EmitResult, FuncBuildImports, InstrBlockPy, JitCodegenEnv, JitEmitCtx, LocalEnv,
    PyObjFacts, ResultDemand, SoacValue, ValueOwnership, captured_argument_error_context,
    emit_checked_owned_pyobject_result, emit_checked_owned_pyobject_result_for_demand,
    emit_codegen_expr_with_local_env, emit_object_call_with_tuple_args_result,
    emit_record_authenticated_call_target_sample, emit_release_owned_inputs,
    emit_typed_frame_namespace_with_local_env, emit_typed_pyobject_value_with_local_env,
    emit_vectorcall_argument_buffer, owned_slots, step_null_block_args,
};
use cranelift_codegen::ir::{self, InstBuilder, condcodes::IntCC};
use cranelift_frontend::FunctionBuilder;
use soac_core::block_py::{
    Call, CallArgPositional, CallArgumentOp, CallArgumentOpKind, HasSemanticInstrId, PreparedCall,
};
use soac_ir_typed::{InstrTyped, TypedCall};

fn check_status(fb: &mut FunctionBuilder<'_>, status: ir::Value, ctx: &JitEmitCtx<'_>) {
    let failed = fb.ins().icmp_imm(IntCC::SignedLessThan, status, 0);
    let ready = fb.create_block();
    fb.ins().brif(
        failed,
        ctx.consts.step_null_block,
        &step_null_block_args(ctx),
        ready,
        &[],
    );
    fb.switch_to_block(ready);
}

pub(super) fn emit_phase(
    fb: &mut FunctionBuilder<'_>,
    op: &CallArgumentOp<InstrTyped>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    let layout = ctx
        .storage_layout
        .as_ref()
        .ok_or("call-argument phase has no storage layout")?;
    let (callable_location, buffer_location) = op.validate_resolved(layout)?;
    let spec = match op.kind {
        CallArgumentOpKind::ExtendPositional | CallArgumentOpKind::MergeKeywords => {
            &runtime::UPDATE
        }
        CallArgumentOpKind::FinishPositionalList => &runtime::FINISH_LIST,
        CallArgumentOpKind::NormalizeSingletonStar => &runtime::NORMALIZE_SINGLETON,
    };
    let helper = imports.get(codegen_env, &mut fb.func, spec)?;
    let callable = owned_slots::borrow_operand(fb, callable_location, locals, ctx)?;
    let buffer = owned_slots::borrow_operand(fb, buffer_location, locals, ctx)?;
    match op.kind {
        CallArgumentOpKind::ExtendPositional | CallArgumentOpKind::MergeKeywords => {
            let input = op.value.as_deref().expect("validated call update");
            let (value, _, _) = emit_typed_pyobject_value_with_local_env(
                fb,
                input,
                locals,
                ctx,
                false,
                codegen_env,
                imports,
                "owned call-argument update",
            )?;
            let kind = fb
                .ins()
                .iconst(ir::types::I32, i64::from(runtime::update_kind(op.kind)));
            let call = fb.ins().call(helper, &[kind, callable, buffer, value]);
            let status = fb.inst_results(call)[0];
            // The update was consumed on either native edge. Only the named
            // callable/buffer owners remain for the surrounding cleanup.
            check_status(fb, status, ctx);
        }
        CallArgumentOpKind::FinishPositionalList => {
            let owned = owned_slots::publish_operand_owned(fb, buffer_location, None, locals, ctx)?;
            let call = fb.ins().call(helper, &[owned]);
            let result = fb.inst_results(call)[0];
            let result = emit_checked_owned_pyobject_result(fb, result, ctx);
            let old =
                owned_slots::publish_operand_owned(fb, buffer_location, Some(result), locals, ctx)?;
            emit_release_owned_inputs(fb, ctx, &[old]);
        }
        CallArgumentOpKind::NormalizeSingletonStar => {
            let call = fb.ins().call(helper, &[callable, buffer]);
            let result = fb.inst_results(call)[0];
            let result = emit_checked_owned_pyobject_result(fb, result, ctx);
            // ERROR_NO_POP above leaves the raw primary untouched. Success
            // publishes the tuple before the old raw value can run a finalizer.
            let old =
                owned_slots::publish_operand_owned(fb, buffer_location, Some(result), locals, ctx)?;
            emit_release_owned_inputs(fb, ctx, &[old]);
        }
    }
    Ok(())
}

pub(super) fn emit_prepared(
    fb: &mut FunctionBuilder<'_>,
    op: &PreparedCall<InstrTyped>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    imports: &mut FuncBuildImports<'_>,
) -> Result<SoacValue, String> {
    let layout = ctx
        .storage_layout
        .as_ref()
        .ok_or("prepared call has no storage layout")?;
    op.validate_resolved(layout)?;
    let check = imports.get(codegen_env, &mut fb.func, &runtime::CHECK_PREPARED)?;
    let mut contextual = ctx.clone();
    contextual.frame_namespace_value = emit_typed_frame_namespace_with_local_env(
        fb,
        op.frame_namespace.as_ref(),
        locals,
        ctx,
        codegen_env,
        imports,
    )?;
    let ctx = &contextual;
    let mut values = Vec::with_capacity(3);
    let borrowed = [false; 3];
    for input in std::iter::once(op.func.as_ref())
        .chain(std::iter::once(op.arguments.as_ref()))
        .chain(op.keywords.as_deref())
    {
        let input_ctx =
            captured_argument_error_context(fb, ctx, None, &values, &borrowed[..values.len()]);
        let (value, _, _) = emit_typed_pyobject_value_with_local_env(
            fb,
            input,
            locals,
            &input_ctx,
            false,
            codegen_env,
            imports,
            "prepared call Operand move",
        )?;
        values.push(value);
    }
    let kwargs = values.get(2).copied();
    let raw_kwargs = kwargs.unwrap_or_else(|| fb.ins().iconst(ctx.consts.ptr_ty, 0));
    let call = fb.ins().call(check, &[values[1], raw_kwargs]);
    let status = fb.inst_results(call)[0];
    let rejected_ctx =
        captured_argument_error_context(fb, ctx, None, &values, &borrowed[..values.len()]);
    check_status(fb, status, &rejected_ctx);
    // Invoke the existing frame-aware native call directly. No tuple rebuilding,
    // star expansion, keyword merging, or callable-admission shortcut remains.
    emit_object_call_with_tuple_args_result(
        fb,
        values[0],
        false,
        values[1],
        kwargs,
        ctx,
        ResultDemand::PYOBJECT_OWNED,
        CallEmission::Ordinary,
    )
    .value()
    .ok_or_else(|| "prepared call produced no value".into())
}

/// Unlike the ordinary vectorcall helpers, this helper consumes the complete
/// input region on success AND failure. Never emit another argument close.
fn emit_owned_operand_result(
    fb: &mut FunctionBuilder<'_>,
    values: &[ir::Value],
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    imports: &mut FuncBuildImports<'_>,
) -> Result<EmitResult, String> {
    let helper = imports.get(codegen_env, &mut fb.func, &runtime::OWNED_OPERANDS)?;
    let inputs = emit_vectorcall_argument_buffer(fb, values, ctx.consts.ptr_ty);
    let count = fb.ins().iconst(ir::types::I64, values.len() as i64);
    let call = fb
        .ins()
        .call(helper, &[ctx.consts.function_env_value, inputs, count]);
    let result = fb.inst_results(call)[0];
    Ok(emit_checked_owned_pyobject_result_for_demand(
        fb,
        result,
        PyObjFacts::unknown(),
        ctx,
        demand,
    ))
}

pub(super) fn emit_owned_operand_call(
    fb: &mut FunctionBuilder<'_>,
    call: &Call<InstrBlockPy>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    imports: &mut FuncBuildImports<'_>,
) -> Option<EmitResult> {
    if !ctx.storage_layout.as_ref().is_some_and(|layout| {
        runtime::blockpy_owned_operand_call(call, layout)
            .expect("source Operand calls have validated physical ownership")
    }) {
        return None;
    }
    let mut values = Vec::with_capacity(call.args.len() + 1);
    let borrowed = vec![false; call.args.len() + 1];
    for input in std::iter::once(call.func.as_ref()).chain(call.args.iter().map(|arg| {
        let CallArgPositional::Positional(input) = arg else {
            unreachable!("selected owned call is positional")
        };
        input
    })) {
        let input_ctx =
            captured_argument_error_context(fb, ctx, None, &values, &borrowed[..values.len()]);
        let value = emit_codegen_expr_with_local_env(
            fb,
            input,
            locals,
            &input_ctx,
            false,
            codegen_env,
            imports,
        );
        values.push(value);
        if values.len() == 1
            && let Some(counter) = call
                .try_semantic_instr_id()
                .and_then(|site| ctx.call_target_counter_ids.get(&site))
        {
            emit_record_authenticated_call_target_sample(
                fb,
                *counter,
                value,
                ctx,
                codegen_env,
                imports,
            );
        }
    }
    Some(
        emit_owned_operand_result(fb, &values, ctx, demand, codegen_env, imports)
            .expect("owned outgoing helper has its registered native signature"),
    )
}

pub(super) fn emit_typed_owned_operand_call(
    fb: &mut FunctionBuilder<'_>,
    call: &TypedCall<InstrTyped>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    demand: ResultDemand,
    codegen_env: &mut impl JitCodegenEnv,
    imports: &mut FuncBuildImports<'_>,
) -> Result<Option<EmitResult>, String> {
    let Some(layout) = ctx.storage_layout.as_ref() else {
        return Ok(None);
    };
    if !runtime::typed_owned_operand_call(call, layout)? {
        return Ok(None);
    }
    let mut values = Vec::with_capacity(call.args.len() + 1);
    let borrowed = vec![false; call.args.len() + 1];
    for input in std::iter::once(call.func.as_ref()).chain(call.args.iter().map(|arg| {
        let CallArgPositional::Positional(input) = arg else {
            unreachable!("selected owned call is positional")
        };
        input
    })) {
        let input_ctx =
            captured_argument_error_context(fb, ctx, None, &values, &borrowed[..values.len()]);
        let (value, ownership, _) = emit_typed_pyobject_value_with_local_env(
            fb,
            input,
            locals,
            &input_ctx,
            false,
            codegen_env,
            imports,
            "owned outgoing expression operand",
        )?;
        if !matches!(ownership, ValueOwnership::Owned | ValueOwnership::Immortal) {
            return Err("selected outgoing Operand did not emit an owned value".into());
        }
        values.push(value);
        if values.len() == 1
            && let Some(counter) = call
                .try_semantic_instr_id()
                .and_then(|site| ctx.call_target_counter_ids.get(&site))
        {
            emit_record_authenticated_call_target_sample(
                fb,
                *counter,
                value,
                ctx,
                codegen_env,
                imports,
            );
        }
    }
    emit_owned_operand_result(fb, &values, ctx, demand, codegen_env, imports).map(Some)
}
