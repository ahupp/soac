//! Deferred exception-edge cleanup. Capture borrowed SSA inputs in the producer
//! block; create forwarding owners only on the exceptional edge that consumes them.

use super::*;

pub(super) struct PendingFailure {
    block: ir::Block,
    cleanup_null_block: ir::Block,
    target: ir::Block,
    arguments: Vec<ir::Value>,
    locals: LocalEnv,
}

pub(super) fn caught_failure_context<'mc>(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'mc>,
    local_env: &LocalEnv,
    cleanup_null_block: ir::Block,
) -> JitEmitCtx<'mc> {
    let failure = fb.create_block();
    fb.set_cold_block(failure);
    let mut arguments = Vec::new();
    let mut locals = local_env.clone();
    for entry in &mut locals.entries {
        let value = entry.value();
        arguments.push(value);
        let parameter = fb.append_block_param(failure, fb.func.dfg.value_type(value));
        entry.binding = entry.binding.with_value(parameter);
    }
    let mut target_arguments = Vec::with_capacity(ctx.consts.step_null_args.len());
    for &value in &ctx.consts.step_null_args {
        arguments.push(value);
        target_arguments.push(fb.append_block_param(failure, fb.func.dfg.value_type(value)));
    }
    ctx.pending_exception_failures
        .borrow_mut()
        .push(PendingFailure {
            block: failure,
            cleanup_null_block,
            target: ctx.consts.step_null_block,
            arguments: target_arguments,
            locals,
        });
    let mut delegated = ctx.with_step_null_target(failure, arguments);
    delegated.failure_local_cleanup_delegated = true;
    delegated
}

pub(super) fn emit_pending_failures(
    fb: &mut FunctionBuilder<'_>,
    ctx: &JitEmitCtx<'_>,
    pending_local_failure_cleanups: &mut Vec<PendingLocalFailureCleanup>,
    local_failure_cleanup_blocks: &mut HashMap<LocalFailureCleanupKey, ir::Block>,
    block_roles: &mut ClifBlockRoles,
) -> Result<(), String> {
    for failure in std::mem::take(&mut *ctx.pending_exception_failures.borrow_mut()) {
        fb.switch_to_block(failure.block);
        let base = ctx.with_step_null_target(failure.target, failure.arguments);
        let cleanup = local_failure_cleanup_emit_ctx(
            fb,
            &base,
            &failure.locals,
            failure.cleanup_null_block,
            pending_local_failure_cleanups,
            local_failure_cleanup_blocks,
            block_roles,
        )?;
        let cleanup = cleanup.as_ref().unwrap_or(&base);
        fb.ins().jump(
            cleanup.consts.step_null_block,
            &block_arg_values(&cleanup.consts.step_null_args),
        );
    }
    Ok(())
}
