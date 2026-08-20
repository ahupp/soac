//! Mechanical insertion into a validated live Operand collection owner.

use super::collection_runtime;
use super::{
    FuncBuildImports, JitCodegenEnv, JitEmitCtx, LocalEnv, captured_argument_error_context,
    emit_typed_pyobject_value_with_local_env, owned_slots, step_null_block_args,
};
use cranelift_codegen::ir::{self, InstBuilder, condcodes::IntCC};
use cranelift_frontend::FunctionBuilder;
use soac_core::block_py::ComprehensionInsert;
use soac_ir_typed::InstrTyped;

pub(super) fn emit(
    fb: &mut FunctionBuilder<'_>,
    op: &ComprehensionInsert<InstrTyped>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    imports: &mut FuncBuildImports<'_>,
) -> Result<(), String> {
    let layout = ctx
        .storage_layout
        .as_ref()
        .ok_or("comprehension insertion has no resolved storage layout")?;
    let location = op.validate_resolved(layout)?;
    let insert = imports.get(codegen_env, &mut fb.func, &collection_runtime::INSERT)?;
    // This checked read is a borrow of the actual Operand owner, not an owned
    // Load or a bound method receiver. It survives all input callbacks solely
    // through its existing slot and the operation's explicit name-use effect.
    let container = owned_slots::borrow_operand(fb, location, locals, ctx)?;
    let mut values = Vec::with_capacity(2);
    let borrowed = [false; 2];
    for input in op.key.as_deref().into_iter().chain([op.value.as_ref()]) {
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
            "owned comprehension insertion operand",
        )?;
        values.push(value);
    }
    let kind = fb.ins().iconst(
        ir::types::I32,
        i64::from(collection_runtime::kind_tag(op.kind)),
    );
    let key = if op.key.is_some() {
        values[0]
    } else {
        fb.ins().iconst(ctx.consts.ptr_ty, 0)
    };
    let value = *values.last().expect("validated insertion value");
    let inserted = fb.ins().call(insert, &[kind, container, key, value]);
    let status = fb.inst_results(inserted)[0];
    // All input references have been consumed on either edge. The outer
    // exception continuation still owns the collection/iterator Operand slots.
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
    Ok(())
}
