//! Mechanical BUILD_LIST/BUILD_SET/BUILD_MAP emission.

use super::collection_runtime;
use super::{
    FuncBuildImports, JitCodegenEnv, JitEmitCtx, LocalEnv, SoacValue,
    captured_argument_error_context, emit_checked_owned_pyobject_result,
    emit_typed_pyobject_value_with_local_env,
};
use cranelift_codegen::ir::{self, InstBuilder};
use cranelift_frontend::FunctionBuilder;
use soac_core::block_py::BuildCollection;
use soac_ir_typed::{InstrTyped, PyObjFacts};

pub(super) fn emit(
    fb: &mut FunctionBuilder<'_>,
    op: &BuildCollection<InstrTyped>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    imports: &mut FuncBuildImports<'_>,
) -> Result<SoacValue, String> {
    op.validate_shape()?;
    let pointer_size = ctx.consts.ptr_ty.bytes();
    let count = u32::try_from(op.values.len()).map_err(|_| "too many collection inputs")?;
    let size = count
        .max(1)
        .checked_mul(pointer_size)
        .ok_or("collection input array is too large")?;
    let size = i32::try_from(size).map_err(|_| "collection input offsets exceed native limits")?;
    let build = imports.get(codegen_env, &mut fb.func, &collection_runtime::BUILD)?;
    let slot = fb.create_sized_stack_slot(ir::StackSlotData::new(
        ir::StackSlotKind::ExplicitSlot,
        size as u32,
        pointer_size.trailing_zeros() as u8,
    ));
    let mut values = Vec::with_capacity(op.values.len());
    let borrowed = vec![false; op.values.len()];
    for (index, input) in op.values.iter().enumerate() {
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
            "owned native collection input",
        )?;
        fb.ins()
            .stack_store(value, slot, (index as u32 * pointer_size) as i32);
        values.push(value);
    }
    let array = fb.ins().stack_addr(ctx.consts.ptr_ty, slot, 0);
    let count = fb.ins().iconst(ctx.consts.ptr_ty, i64::from(count));
    let kind = fb.ins().iconst(
        ir::types::I32,
        i64::from(collection_runtime::build_kind_tag(op.kind)),
    );
    let call = fb.ins().call(build, &[kind, array, count]);
    let result = fb.inst_results(call)[0];
    // The helper owns every input from this point, including its failure edge.
    let result = emit_checked_owned_pyobject_result(fb, result, ctx);
    Ok(SoacValue::owned_pyobject(
        result,
        PyObjFacts::unknown().with_non_null_ref(),
    ))
}
