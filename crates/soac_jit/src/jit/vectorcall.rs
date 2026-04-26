use super::backend::{define_prepared_function, register_jit_signal_diagnostics};
use super::codegen_env::{FuncBuildImports, JitCodegenEnv, declare_local_fn};
use super::imports::{
    DP_JIT_DECREF_IMPORT, DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
    DP_JIT_VECTORCALL_BIND_DIRECT_ARGS_IMPORT, DP_JIT_VECTORCALL_COMPILE_FUNCTION_ENV_IMPORT,
    ModuleFuncImports, PY_THREAD_STATE_GET_UNCHECKED_IMPORT,
    SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
};
use super::runtime_context::{
    FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET, PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
    load_function_env_obj, load_py_function_soac_metadata_obj,
};
use super::{
    RuntimeFunctionId, SoacEnvConfig, VectorcallEntryFn,
    emit_take_current_raised_exception_or_trap, jitdump,
};
use cranelift_codegen::ir;
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_jit::JITModule;

pub(super) fn define_shared_vectorcall_trampoline(
    jit_module: &mut JITModule,
    env_config: &SoacEnvConfig,
    param_count: usize,
    symbol_name: &str,
) -> Result<VectorcallEntryFn, String> {
    let ptr_ty = jit_module.codegen_target_config().pointer_type();
    let i64_ty = ir::types::I64;
    let mut module_imports = ModuleFuncImports::new();

    let mut main_sig = jit_module.codegen_make_signature();
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.params.push(ir::AbiParam::new(ptr_ty));
    main_sig.returns.push(ir::AbiParam::new(ptr_ty));

    let main_id = declare_local_fn(jit_module, symbol_name, &main_sig)?;

    let mut direct_sig = jit_module.codegen_make_signature();
    direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    for _ in 0..param_count {
        direct_sig.params.push(ir::AbiParam::new(ptr_ty));
    }
    direct_sig.returns.push(ir::AbiParam::new(ptr_ty));

    let mut ctx = jit_module.codegen_make_context();
    ctx.func.signature = main_sig;
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut fb = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);
        let entry = fb.create_block();
        fb.append_block_params_for_function_params(entry);
        fb.switch_to_block(entry);
        fb.seal_block(entry);

        let callable_val = fb.block_params(entry)[0];
        let args_val = fb.block_params(entry)[1];
        let nargsf_val = fb.block_params(entry)[2];
        let kwnames_val = fb.block_params(entry)[3];

        let mut func_imports = FuncBuildImports::new(&mut module_imports);
        let bind_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_VECTORCALL_BIND_DIRECT_ARGS_IMPORT,
        );
        let compile_env_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_VECTORCALL_COMPILE_FUNCTION_ENV_IMPORT,
        );
        let enter_recursive_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &DP_JIT_ENTER_RECURSIVE_CALL_IMPORT,
        );
        let decref_ref = func_imports.get_or_panic(jit_module, &mut fb.func, &DP_JIT_DECREF_IMPORT);
        let thread_state_get_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &PY_THREAD_STATE_GET_UNCHECKED_IMPORT,
        );
        let set_raised_exception_ref = func_imports.get_or_panic(
            jit_module,
            &mut fb.func,
            &SOAC_RUNTIME_SET_RAISED_EXCEPTION_IMPORT,
        );

        let null_ptr = fb.ins().iconst(ptr_ty, 0);
        let function_extra_val = load_py_function_soac_metadata_obj(&mut fb, ptr_ty, callable_val);
        let function_extra_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, function_extra_val, 0);
        let function_extra_ok = fb.create_block();
        let early_fail_block = fb.create_block();
        fb.ins().brif(
            function_extra_missing,
            early_fail_block,
            &[],
            function_extra_ok,
            &[],
        );
        fb.seal_block(early_fail_block);
        fb.seal_block(function_extra_ok);

        fb.switch_to_block(early_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(function_extra_ok);
        let function_env_val = fb.ins().load(
            ptr_ty,
            ir::MemFlags::trusted(),
            function_extra_val,
            PY_FUNCTION_JIT_EXTRA_FUNCTION_ENV_OFFSET,
        );
        let function_env_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, function_env_val, 0);
        let function_env_ok = fb.create_block();
        let context_fail_block = fb.create_block();
        fb.ins().brif(
            function_env_missing,
            context_fail_block,
            &[],
            function_env_ok,
            &[],
        );
        fb.seal_block(context_fail_block);
        fb.seal_block(function_env_ok);

        fb.switch_to_block(context_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(function_env_ok);
        let initial_callee_ptr = load_function_env_obj(
            &mut fb,
            ptr_ty,
            function_env_val,
            FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET,
        );
        let initial_callee_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, initial_callee_ptr, 0);
        let compile_env_block = fb.create_block();
        let function_env_ready = fb.create_block();
        fb.append_block_param(function_env_ready, ptr_ty);
        fb.append_block_param(function_env_ready, ptr_ty);
        fb.ins().brif(
            initial_callee_missing,
            compile_env_block,
            &[],
            function_env_ready,
            &[
                ir::BlockArg::Value(function_env_val),
                ir::BlockArg::Value(initial_callee_ptr),
            ],
        );
        fb.seal_block(compile_env_block);

        fb.switch_to_block(compile_env_block);
        let compile_inst = fb
            .ins()
            .call(compile_env_ref, &[callable_val, function_extra_val]);
        let compiled_function_env_val = fb.inst_results(compile_inst)[0];
        let compiled_function_env_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, compiled_function_env_val, 0);
        let compile_fail_block = fb.create_block();
        let compiled_function_env_ok = fb.create_block();
        fb.ins().brif(
            compiled_function_env_missing,
            compile_fail_block,
            &[],
            compiled_function_env_ok,
            &[],
        );
        fb.seal_block(compile_fail_block);
        fb.seal_block(compiled_function_env_ok);

        fb.switch_to_block(compile_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(compiled_function_env_ok);
        let compiled_callee_ptr = load_function_env_obj(
            &mut fb,
            ptr_ty,
            compiled_function_env_val,
            FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET,
        );
        let compiled_callee_missing =
            fb.ins()
                .icmp_imm(ir::condcodes::IntCC::Equal, compiled_callee_ptr, 0);
        let compiled_callee_fail_block = fb.create_block();
        fb.ins().brif(
            compiled_callee_missing,
            compiled_callee_fail_block,
            &[],
            function_env_ready,
            &[
                ir::BlockArg::Value(compiled_function_env_val),
                ir::BlockArg::Value(compiled_callee_ptr),
            ],
        );
        fb.seal_block(compiled_callee_fail_block);
        fb.seal_block(function_env_ready);

        fb.switch_to_block(compiled_callee_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(function_env_ready);
        let function_env_val = fb.block_params(function_env_ready)[0];
        let callee_ptr = fb.block_params(function_env_ready)[1];
        let thread_state_inst = fb.ins().call(thread_state_get_ref, &[]);
        let thread_state_val = fb.inst_results(thread_state_inst)[0];
        let enter_inst = fb.ins().call(enter_recursive_ref, &[thread_state_val]);
        let enter_status = fb.inst_results(enter_inst)[0];
        let enter_failed = fb
            .ins()
            .icmp_imm(ir::condcodes::IntCC::NotEqual, enter_status, 0);
        let recursion_fail_block = fb.create_block();
        let bind_block = fb.create_block();
        fb.ins()
            .brif(enter_failed, recursion_fail_block, &[], bind_block, &[]);
        fb.seal_block(recursion_fail_block);
        fb.seal_block(bind_block);

        fb.switch_to_block(recursion_fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(bind_block);
        let bound_args_slot = if param_count == 0 {
            None
        } else {
            Some(fb.create_sized_stack_slot(ir::StackSlotData::new(
                ir::StackSlotKind::ExplicitSlot,
                (param_count * std::mem::size_of::<u64>()) as u32,
                0,
            )))
        };
        let bound_args_ptr = if let Some(slot) = bound_args_slot {
            fb.ins().stack_addr(ptr_ty, slot, 0)
        } else {
            null_ptr
        };
        let out_len = fb.ins().iconst(i64_ty, param_count as i64);
        let bind_inst = fb.ins().call(
            bind_ref,
            &[
                callable_val,
                args_val,
                nargsf_val,
                kwnames_val,
                function_extra_val,
                bound_args_ptr,
                out_len,
            ],
        );
        let bind_ok = fb.inst_results(bind_inst)[0];
        let bind_failed = fb.ins().icmp_imm(ir::condcodes::IntCC::Equal, bind_ok, 0);
        let fail_block = fb.create_block();
        let ok_block = fb.create_block();
        fb.ins().brif(bind_failed, fail_block, &[], ok_block, &[]);
        fb.seal_block(fail_block);
        fb.seal_block(ok_block);

        fb.switch_to_block(fail_block);
        fb.ins().return_(&[null_ptr]);

        fb.switch_to_block(ok_block);
        let direct_sig_ref = fb.import_signature(direct_sig);
        let mut call_args = Vec::with_capacity(param_count + 2);
        call_args.push(function_env_val);
        call_args.push(thread_state_val);
        let mut owned_args = Vec::with_capacity(param_count);
        if let Some(slot) = bound_args_slot {
            for index in 0..param_count {
                let value =
                    fb.ins()
                        .stack_load(ptr_ty, slot, (index * std::mem::size_of::<u64>()) as i32);
                owned_args.push(value);
                call_args.push(value);
            }
        }
        let call_inst = fb
            .ins()
            .call_indirect(direct_sig_ref, callee_ptr, &call_args);
        let result = fb.inst_results(call_inst)[0];
        let result_is_null = fb.ins().icmp(ir::condcodes::IntCC::Equal, result, null_ptr);
        let direct_null_block = fb.create_block();
        let direct_ok_block = fb.create_block();
        fb.ins()
            .brif(result_is_null, direct_null_block, &[], direct_ok_block, &[]);
        fb.seal_block(direct_null_block);
        fb.seal_block(direct_ok_block);

        fb.switch_to_block(direct_null_block);
        let error_value =
            emit_take_current_raised_exception_or_trap(&mut fb, ptr_ty, thread_state_val);
        for value in owned_args.iter().copied() {
            fb.ins().call(decref_ref, &[thread_state_val, value]);
        }
        fb.ins()
            .call(set_raised_exception_ref, &[thread_state_val, error_value]);
        fb.ins().return_(&[result]);

        fb.switch_to_block(direct_ok_block);
        for value in owned_args {
            fb.ins().call(decref_ref, &[thread_state_val, value]);
        }
        fb.ins().return_(&[result]);
        fb.seal_all_blocks();
        fb.finalize();
    }

    let main_artifact = define_prepared_function(
        jit_module,
        env_config,
        main_id,
        &mut ctx,
        &format!("direct-vectorcall-trampoline:{param_count}"),
        "failed to define direct vectorcall trampoline",
    )?;
    jit_module.codegen_clear_context(&mut ctx);
    jit_module
        .finalize_definitions()
        .map_err(|err| format!("failed to finalize direct vectorcall trampoline: {err}"))?;

    let code_ptr = jit_module.get_finalized_function(main_id);
    jitdump::record_code_load(
        symbol_name,
        code_ptr.cast::<u8>(),
        main_artifact.code_size,
        jit_module.codegen_isa(),
        main_artifact.systemv_unwind_info.as_ref(),
    )?;
    register_jit_signal_diagnostics(
        symbol_name,
        code_ptr.cast::<u8>(),
        &main_artifact,
        RuntimeFunctionId::global(),
        symbol_name,
        "direct_vectorcall_trampoline",
    );
    let entry: VectorcallEntryFn = unsafe { std::mem::transmute(code_ptr) };
    Ok(entry)
}
