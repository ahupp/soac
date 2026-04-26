use super::*;

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
