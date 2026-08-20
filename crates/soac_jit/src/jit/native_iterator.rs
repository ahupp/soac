//! Mechanical expansion of a validated inline-only native iterator template.

use super::inspection::RefcountFamily;
use super::native_iterator_runtime as runtime;
use super::{
    FuncBuildImports, JitCodegenEnv, JitEmitCtx, LocalEnv, ResultDemand, SoacValue,
    captured_argument_error_context, emit_checked_owned_pyobject_result,
    emit_positional_vectorcall_result_with_arg_values, emit_release_owned_inputs,
    emit_typed_pyobject_input_with_local_env, step_null_block_args,
};
use cranelift_codegen::ir::{self, InstBuilder, condcodes::IntCC};
use cranelift_frontend::FunctionBuilder;
use soac_ir_typed::{InstrTyped, NativeIteratorMaterializer, NativeIteratorStage, PyObjFacts};
use soac_opt::passes::{
    native_iterator_pipeline_operands, validate_typed_native_iterator_pipeline_plans,
};

fn release_stage(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    iterator: ir::Value,
    callback: ir::Value,
) {
    // Both stage-owned inputs retire at this safe exit. Their implicit
    // finalizer order need not reproduce map/filter's internal field order.
    emit_release_owned_inputs(fb, ctx, &[iterator, callback]);
}

pub(super) fn emit(
    fb: &mut FunctionBuilder<'_>,
    expr: &InstrTyped,
    local_env: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    func_imports: &mut FuncBuildImports<'_>,
) -> Result<Option<SoacValue>, String> {
    let Some(plan) = expr.native_iterator_pipeline_plan() else {
        return Ok(None);
    };
    let function = ctx
        .module
        .callable_defs
        .iter()
        .find(|function| function.function_id == ctx.function_id)
        .ok_or_else(|| "native iterator emitter has no current typed function".to_owned())?;
    validate_typed_native_iterator_pipeline_plans(
        function,
        &ctx.module.module_constants,
        std::slice::from_ref(plan),
    )?;
    let operands = native_iterator_pipeline_operands(expr, &ctx.module.module_constants, plan)?;
    let stage_kind = match plan.stage {
        NativeIteratorStage::Map => runtime::MAP,
        NativeIteratorStage::Filter => runtime::FILTER,
    };
    let sink_kind = match plan.materializer {
        NativeIteratorMaterializer::List => runtime::LIST,
        NativeIteratorMaterializer::Tuple => runtime::TUPLE,
    };

    // Import every fixed primitive before modifying the CFG. No Python helper
    // lookup or callable-admission decision happens in this emitter.
    let guard_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::GUARD)?;
    let get_iter_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::GET_ITER)?;
    let next_slot_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::NEXT_SLOT)?;
    let map_call_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::MAP_CALL)?;
    let filter_truth_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::FILTER_TRUTH)?;
    let exhausted_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::EXHAUSTED)?;
    let init_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::MATERIALIZER_INIT)?;
    let append_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::MATERIALIZER_APPEND)?;
    let finish_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::MATERIALIZER_FINISH)?;
    let abort_ref = func_imports.get(codegen_env, &mut fb.func, &runtime::MATERIALIZER_ABORT)?;

    // Evaluate operands in source order. Scalar boxing and later-operand
    // failures use the existing acquired-prefix cleanup. The stage acquires
    // its callback owner after successful iterator creation, before releasing
    // the call's supporting operands.
    let mut values = Vec::with_capacity(4);
    let mut borrowed = Vec::with_capacity(4);
    for input in [
        operands.materializer_call.func.as_ref(),
        operands.stage_call.func.as_ref(),
        operands.callback,
        operands.iterable,
    ] {
        let input_ctx = captured_argument_error_context(fb, ctx, None, &values, &borrowed);
        let (value, is_borrowed) = emit_typed_pyobject_input_with_local_env(
            fb,
            input,
            local_env,
            &input_ctx,
            codegen_env,
            func_imports,
            "native iterator pipeline operand",
        )?;
        values.push(value);
        borrowed.push(is_borrowed);
    }
    let (materializer, stage, callback, iterable) = (values[0], values[1], values[2], values[3]);
    let ptr_ty = ctx.consts.ptr_ty;
    let stage_tag = fb.ins().iconst(ir::types::I32, i64::from(stage_kind));
    let sink_tag = fb.ins().iconst(ir::types::I32, i64::from(sink_kind));
    let guard = fb
        .ins()
        .call(guard_ref, &[materializer, stage, stage_tag, sink_tag]);
    let matches = fb.inst_results(guard)[0];
    let hot = fb.create_block();
    let fallback = fb.create_block();
    fb.set_cold_block(fallback);
    let result_join = fb.create_block();
    fb.append_block_param(result_join, ptr_ty);
    fb.ins().brif(matches, hot, &[], fallback, &[]);

    // The guarded fallback owns the already evaluated source values. It neither
    // repeats argument evaluation nor invokes an unadmitted implementation body.
    fb.switch_to_block(fallback);
    let fallback_ctx =
        captured_argument_error_context(fb, ctx, (!borrowed[0]).then_some(materializer), &[], &[]);
    let wrapper = emit_positional_vectorcall_result_with_arg_values(
        fb,
        stage,
        borrowed[1],
        vec![callback, iterable],
        vec![borrowed[2], borrowed[3]],
        &fallback_ctx,
        ResultDemand::PYOBJECT_OWNED,
    )
    .expect_pyobject("native iterator original stage fallback")
    .0;
    let result = emit_positional_vectorcall_result_with_arg_values(
        fb,
        materializer,
        borrowed[0],
        vec![wrapper],
        vec![false],
        ctx,
        ResultDemand::PYOBJECT_OWNED,
    )
    .expect_pyobject("native iterator original materializer fallback")
    .0;
    fb.ins().jump(result_join, &[result.into()]);

    fb.switch_to_block(hot);
    let iter_call = fb.ins().call(get_iter_ref, &[iterable]);
    let iterator = fb.inst_results(iter_call)[0];
    let acquisition_failed = fb.create_block();
    fb.set_cold_block(acquisition_failed);
    let acquired = fb.create_block();
    let is_null = fb.ins().icmp_imm(IntCC::Equal, iterator, 0);
    fb.ins()
        .brif(is_null, acquisition_failed, &[], acquired, &[]);
    fb.switch_to_block(acquisition_failed);
    let construction_owners = values
        .iter()
        .zip(&borrowed)
        .rev()
        .filter_map(|(value, borrowed)| (!borrowed).then_some(*value))
        .collect::<Vec<_>>();
    emit_release_owned_inputs(fb, ctx, &construction_owners);
    // __iter__ raising StopIteration is a constructor error, not exhaustion.
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(acquired);
    // map/filter constructor owns its own callback edge BEFORE CALL retires
    // arguments. The iterable operand's destructor can observe both that edge
    // and an owned callback operand, so transfer/elision here is not equivalent.
    ctx.emit_incref_for_family(fb, callback, None, RefcountFamily::OwnedTemporary);
    let construction_only = [
        (iterable, borrowed[3]),
        (callback, borrowed[2]),
        (stage, borrowed[1]),
    ]
    .into_iter()
    .filter_map(|(value, borrowed)| (!borrowed).then_some(value))
    .collect::<Vec<_>>();
    emit_release_owned_inputs(fb, ctx, &construction_only);
    let state_slot = fb.create_sized_stack_slot(ir::StackSlotData::new(
        ir::StackSlotKind::ExplicitSlot,
        std::mem::size_of::<runtime::RawNativeIteratorMaterializer>() as u32,
        std::mem::align_of::<runtime::RawNativeIteratorMaterializer>().trailing_zeros() as u8,
    ));
    let state = fb.ins().stack_addr(ptr_ty, state_slot, 0);
    let initialized = fb.ins().call(init_ref, &[state, sink_tag]);
    let init_status = fb.inst_results(initialized)[0];
    let failed = fb.create_block();
    fb.set_cold_block(failed);
    let request = fb.create_block();
    let init_failed = fb.ins().icmp_imm(IntCC::SignedLessThan, init_status, 0);
    fb.ins().brif(init_failed, failed, &[], request, &[]);

    let next = fb.create_block();
    fb.append_block_param(next, ptr_ty);
    let item_ready = fb.create_block();
    let iteration_ended = fb.create_block();
    let append = fb.create_block();
    fb.append_block_param(append, ptr_ty);
    let finish = fb.create_block();
    fb.switch_to_block(request);
    let slot_call = fb.ins().call(next_slot_ref, &[iterator]);
    let slot = fb.inst_results(slot_call)[0];
    fb.ins().jump(next, &[slot.into()]);

    fb.switch_to_block(next);
    let slot = fb.block_params(next)[0];
    let mut next_signature = codegen_env.codegen_make_signature();
    next_signature.params.push(ir::AbiParam::new(ptr_ty));
    next_signature.returns.push(ir::AbiParam::new(ptr_ty));
    let next_signature = fb.import_signature(next_signature);
    let item_call = fb.ins().call_indirect(next_signature, slot, &[iterator]);
    let item = fb.inst_results(item_call)[0];
    let no_item = fb.ins().icmp_imm(IntCC::Equal, item, 0);
    fb.ins()
        .brif(no_item, iteration_ended, &[], item_ready, &[]);

    fb.switch_to_block(item_ready);
    match plan.stage {
        NativeIteratorStage::Map => {
            // map_next uses one ordinary vectorcall argument without the
            // ARGUMENTS_OFFSET flag; filter_next separately uses CallOneArg.
            let args_slot = fb.create_sized_stack_slot(ir::StackSlotData::new(
                ir::StackSlotKind::ExplicitSlot,
                std::mem::size_of::<*mut pyo3::ffi::PyObject>() as u32,
                std::mem::align_of::<*mut pyo3::ffi::PyObject>().trailing_zeros() as u8,
            ));
            fb.ins().stack_store(item, args_slot, 0);
            let args = fb.ins().stack_addr(ptr_ty, args_slot, 0);
            let nargs = fb.ins().iconst(ptr_ty, 1);
            let no_keywords = fb.ins().iconst(ptr_ty, 0);
            let mapped_call = fb
                .ins()
                .call(map_call_ref, &[callback, args, nargs, no_keywords]);
            let mapped = fb.inst_results(mapped_call)[0];
            // map_next retires the input before returning either a result or
            // an exception (which may itself be StopIteration).
            emit_release_owned_inputs(fb, ctx, &[item]);
            let failed = fb.ins().icmp_imm(IntCC::Equal, mapped, 0);
            fb.ins()
                .brif(failed, iteration_ended, &[], append, &[mapped.into()]);
        }
        NativeIteratorStage::Filter => {
            let truth_call = fb.ins().call(filter_truth_ref, &[callback, item]);
            let truth = fb.inst_results(truth_call)[0];
            let rejected = fb.create_block();
            let accepted = fb.ins().icmp_imm(IntCC::SignedGreaterThan, truth, 0);
            fb.ins()
                .brif(accepted, append, &[item.into()], rejected, &[]);
            fb.switch_to_block(rejected);
            emit_release_owned_inputs(fb, ctx, &[item]);
            let failed = fb.ins().icmp_imm(IntCC::SignedLessThan, truth, 0);
            // filter_next reuses its captured native next slot while rejecting
            // items, and reloads it only for the next externally requested item.
            fb.ins()
                .brif(failed, iteration_ended, &[], next, &[slot.into()]);
        }
    }

    fb.switch_to_block(append);
    let output = fb.block_params(append)[0];
    let appended = fb.ins().call(append_ref, &[state, output]);
    let append_status = fb.inst_results(appended)[0];
    let append_failed = fb.ins().icmp_imm(IntCC::SignedLessThan, append_status, 0);
    // append consumes its input on both edges; failed cleanup owns only state.
    fb.ins().brif(append_failed, failed, &[], request, &[]);

    fb.switch_to_block(iteration_ended);
    let exhausted_call = fb.ins().call(exhausted_ref, &[]);
    let exhausted = fb.inst_results(exhausted_call)[0];
    fb.ins().brif(exhausted, finish, &[], failed, &[]);

    fb.switch_to_block(failed);
    fb.ins().call(abort_ref, &[state]);
    release_stage(fb, ctx, iterator, callback);
    if !borrowed[0] {
        emit_release_owned_inputs(fb, ctx, &[materializer]);
    }
    fb.ins()
        .jump(ctx.consts.step_null_block, &step_null_block_args(ctx));

    fb.switch_to_block(finish);
    let finished = fb.ins().call(finish_ref, &[state]);
    let result = fb.inst_results(finished)[0];
    // Tuple allocation failure already consumed the partial materializer; do
    // not run abort again. Keep the wrapper fields alive until after completion.
    release_stage(fb, ctx, iterator, callback);
    if !borrowed[0] {
        emit_release_owned_inputs(fb, ctx, &[materializer]);
    }
    let result = emit_checked_owned_pyobject_result(fb, result, ctx);
    fb.ins().jump(result_join, &[result.into()]);

    fb.switch_to_block(result_join);
    let result = fb.block_params(result_join)[0];
    tracing::info!(
        target: "soac_native_iterator_pipeline",
        function_id = ?function.function_id,
        function_qualname = %function.names.qualname,
        source = ?plan.source,
        stage = ?plan.stage,
        materializer = ?plan.materializer,
        canonical_guard_count = 2,
        native_input_count = 1,
        eliminated_wrapper_count = 1,
        remaining_template_calls = 0,
        eliminated_source_activations = 0,
        "typed_native_iterator_pipeline_committed",
    );
    Ok(Some(SoacValue::pyobject(
        result,
        PyObjFacts::unknown().with_non_null_ref(),
    )))
}
