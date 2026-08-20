//! Mechanical IteratorStep emission from its validated physical Operand owner.

use super::iteration_runtime;
use super::{
    FuncBuildImports, JitCodegenEnv, JitEmitCtx, LocalEnv, SoacValue,
    emit_checked_owned_pyobject_result, owned_slots,
};
use cranelift_codegen::ir::InstBuilder;
use cranelift_frontend::FunctionBuilder;
use soac_core::block_py::{Instr, IteratorStep, ResolvedName};
use soac_ir_typed::PyObjFacts;

pub(super) fn emit<I: Instr<Name = ResolvedName>>(
    fb: &mut FunctionBuilder<'_>,
    op: &IteratorStep<I>,
    locals: &mut LocalEnv,
    ctx: &JitEmitCtx<'_>,
    codegen_env: &mut impl JitCodegenEnv,
    imports: &mut FuncBuildImports<'_>,
) -> Result<SoacValue, String> {
    let layout = ctx
        .storage_layout
        .as_ref()
        .ok_or("iterator step has no resolved storage layout")?;
    let location = op.validate_resolved(layout)?;
    let step = imports.get(codegen_env, &mut fb.func, &iteration_runtime::STEP)?;
    // The explicit name-use effect keeps this primary live on both outcomes.
    // Do not synthesize an owned Load or a Python call argument around it.
    let iterator = owned_slots::borrow_operand(fb, location, locals, ctx)?;
    let call = fb.ins().call(step, &[iterator]);
    let item = fb.inst_results(call)[0];
    let item = emit_checked_owned_pyobject_result(fb, item, ctx);
    Ok(SoacValue::owned_pyobject(
        item,
        PyObjFacts::unknown().with_non_null_ref(),
    ))
}
