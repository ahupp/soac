use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::{
    BlockTerm, Meta, RaiseDisposition, Store, StoreLifetime, TakeOperand, TermIf, TermRaise,
    WithMeta,
};
use crate::passes::ruff_to_blockpy::{InlineFragment, LoopContext, LoweredExpr};
use crate::passes::InstrRuff;
use ruff_python_ast::{self as ast};

fn store_name(name: &str) -> ast::name::Name {
    name.into()
}

fn assign_operand<E>(target: &str, value: InstrRuff, unwind_order: u64) -> E
where
    E: RuffToBlockPyExpr,
{
    let target = store_name(target);
    let meta = Meta::synthetic();
    Store::new(target, E::from_lowered_expr(value))
        .with_lifetime(StoreLifetime::Operand { unwind_order })
        .with_meta(meta)
        .into()
}

#[allow(dead_code)]
pub(crate) fn try_lower_if_expr_direct<L, E>(
    lowerer: &L,
    name_gen: &crate::block_py::FunctionNameGen,
    if_expr: crate::block_py::ExprIf<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<LoweredExpr<E, InstrRuff>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprIf {
        test, body, orelse, ..
    } = if_expr;
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let Some(test_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
        lowerer.lower_expr_instr_into(*test.clone(), structured, loop_ctx)
    }) else {
        return None;
    };
    let (mut entry, test) = match test_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };

    let target = lowerer.fresh_operand_binding();
    // Both mutually exclusive branches publish the same expression owner.
    // Reserve its rank before lowering either branch's private temporaries.
    let unwind_order = name_gen.next_temporary_sequence();
    let Some(body_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
        let body_value = lowerer.lower_expr_instr_into(*body.clone(), structured, loop_ctx)?;
        structured.push_stmt(assign_operand(&target, body_value.clone(), unwind_order));
        Ok(body_value)
    }) else {
        return None;
    };
    let (body_entry, _) = match body_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };

    let Some(orelse_setup) =
        bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
            let orelse_value =
                lowerer.lower_expr_instr_into(*orelse.clone(), structured, loop_ctx)?;
            structured.push_stmt(assign_operand(&target, orelse_value.clone(), unwind_order));
            Ok(orelse_value)
        })
    else {
        return None;
    };
    let (orelse_entry, _) = match orelse_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };

    let then_label = body_entry.entry_ref().label();
    let else_label = orelse_entry.entry_ref().label();
    entry.set_term(BlockTerm::IfTerm(TermIf {
        test: E::from_lowered_expr(test),
        then_label,
        else_label,
    }));

    let (setup_entry_ref, mut deps) = entry.finish_blocks();
    let (_, mut body_blocks) = body_entry.finish_fallthrough_blocks();
    let (_, mut orelse_blocks) = orelse_entry.finish_fallthrough_blocks();
    deps.append(&mut body_blocks);
    deps.append(&mut orelse_blocks);
    let setup_entry_index = deps
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .expect("if-expression setup entry label should be present in assembled blocks");
    let setup_entry = deps.remove(setup_entry_index);

    Some(Ok(LoweredExpr {
        setup: InlineFragment::new(setup_entry, deps),
        value: TakeOperand::new(target).with_meta(Meta::synthetic()).into(),
    }))
}

pub(crate) fn try_lower_if_expr_return_direct<L, E>(
    lowerer: &L,
    name_gen: &crate::block_py::FunctionNameGen,
    if_expr: crate::block_py::ExprIf<InstrRuff>,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprIf {
        test, body, orelse, ..
    } = if_expr;
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let Some(test_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
        lowerer.lower_expr_instr_into(*test.clone(), structured, loop_ctx)
    }) else {
        return None;
    };
    let (mut entry, test) = match test_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };

    let Some(body_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
        lowerer.lower_expr_instr_into(*body.clone(), structured, loop_ctx)
    }) else {
        return None;
    };
    let (mut body_entry, body_value) = match body_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    body_entry.set_term(BlockTerm::Return(E::from_lowered_expr(body_value)));

    let Some(orelse_setup) = bridge
        .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
            lowerer.lower_expr_instr_into(*orelse.clone(), structured, loop_ctx)
        })
    else {
        return None;
    };
    let (mut orelse_entry, orelse_value) = match orelse_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    orelse_entry.set_term(BlockTerm::Return(E::from_lowered_expr(orelse_value)));

    let then_label = body_entry.entry_ref().label();
    let else_label = orelse_entry.entry_ref().label();
    entry.set_term(BlockTerm::IfTerm(TermIf {
        test: E::from_lowered_expr(test),
        then_label,
        else_label,
    }));

    let (setup_entry_ref, mut deps) = entry.finish_blocks();
    let (_, mut body_blocks) = body_entry.finish_blocks();
    let (_, mut orelse_blocks) = orelse_entry.finish_blocks();
    deps.append(&mut body_blocks);
    deps.append(&mut orelse_blocks);
    let setup_entry_index = deps
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .expect("if-expression return setup entry label should be present in assembled blocks");
    let setup_entry = deps.remove(setup_entry_index);

    Some(Ok(InlineFragment::new(setup_entry, deps)))
}

pub(crate) fn try_lower_if_expr_raise_direct<L, E>(
    lowerer: &L,
    name_gen: &crate::block_py::FunctionNameGen,
    if_expr: crate::block_py::ExprIf<InstrRuff>,
    disposition: RaiseDisposition,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprIf {
        test, body, orelse, ..
    } = if_expr;
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let Some(test_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
        lowerer.lower_expr_instr_into(*test.clone(), structured, loop_ctx)
    }) else {
        return None;
    };
    let (mut entry, test) = match test_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };

    let Some(body_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
        lowerer.lower_expr_instr_into(*body.clone(), structured, loop_ctx)
    }) else {
        return None;
    };
    let (mut body_entry, body_value) = match body_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    body_entry.set_term(BlockTerm::Raise(TermRaise {
        exc: Some(E::from_lowered_expr(body_value)),
        disposition,
    }));

    let Some(orelse_setup) = bridge
        .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
            lowerer.lower_expr_instr_into(*orelse.clone(), structured, loop_ctx)
        })
    else {
        return None;
    };
    let (mut orelse_entry, orelse_value) = match orelse_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    orelse_entry.set_term(BlockTerm::Raise(TermRaise {
        exc: Some(E::from_lowered_expr(orelse_value)),
        disposition,
    }));

    let then_label = body_entry.entry_ref().label();
    let else_label = orelse_entry.entry_ref().label();
    entry.set_term(BlockTerm::IfTerm(TermIf {
        test: E::from_lowered_expr(test),
        then_label,
        else_label,
    }));

    let (setup_entry_ref, mut deps) = entry.finish_blocks();
    let (_, mut body_blocks) = body_entry.finish_blocks();
    let (_, mut orelse_blocks) = orelse_entry.finish_blocks();
    deps.append(&mut body_blocks);
    deps.append(&mut orelse_blocks);
    let setup_entry_index = deps
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .expect("if-expression raise setup entry label should be present in assembled blocks");
    let setup_entry = deps.remove(setup_entry_index);

    Some(Ok(InlineFragment::new(setup_entry, deps)))
}

pub(crate) fn try_lower_if_expr_branch_direct<L, E>(
    lowerer: &L,
    name_gen: &crate::block_py::FunctionNameGen,
    if_expr: crate::block_py::ExprIf<InstrRuff>,
    then_label: crate::block_py::BlockLabel,
    else_label: crate::block_py::BlockLabel,
    loop_ctx: Option<&LoopContext>,
) -> Option<Result<InlineFragment<E>, String>>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    let crate::block_py::ExprIf {
        test, body, orelse, ..
    } = if_expr;
    let bridge = crate::passes::ruff_to_blockpy::stmt_lowering::StructuredLoweringBridge::new();
    let Some(test_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
        lowerer.lower_expr_instr_into(*test.clone(), structured, loop_ctx)
    }) else {
        return None;
    };
    let (mut entry, test) = match test_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };

    let Some(body_setup) = bridge.try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
        lowerer.lower_expr_instr_into(*body.clone(), structured, loop_ctx)
    }) else {
        return None;
    };
    let (mut body_entry, body_value) = match body_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    body_entry.set_term(BlockTerm::IfTerm(TermIf {
        test: E::from_lowered_expr(body_value),
        then_label: then_label.clone(),
        else_label: else_label.clone(),
    }));

    let Some(orelse_setup) = bridge
        .try_lower_inline_value::<E, InstrRuff>(name_gen, |structured| {
            lowerer.lower_expr_instr_into(*orelse.clone(), structured, loop_ctx)
        })
    else {
        return None;
    };
    let (mut orelse_entry, orelse_value) = match orelse_setup {
        Ok(value) => value,
        Err(err) => return Some(Err(err)),
    };
    orelse_entry.set_term(BlockTerm::IfTerm(TermIf {
        test: E::from_lowered_expr(orelse_value),
        then_label: then_label.clone(),
        else_label: else_label.clone(),
    }));

    let body_label = body_entry.entry_ref().label();
    let orelse_label = orelse_entry.entry_ref().label();
    entry.set_term(BlockTerm::IfTerm(TermIf {
        test: E::from_lowered_expr(test),
        then_label: body_label,
        else_label: orelse_label,
    }));

    let (setup_entry_ref, mut deps) = entry.finish_blocks();
    let (_, mut body_blocks) = body_entry.finish_blocks();
    let (_, mut orelse_blocks) = orelse_entry.finish_blocks();
    deps.append(&mut body_blocks);
    deps.append(&mut orelse_blocks);
    let setup_entry_index = deps
        .iter()
        .position(|block| block.label == setup_entry_ref.label())
        .expect("if-expression branch setup entry label should be present in assembled blocks");
    let setup_entry = deps.remove(setup_entry_index);

    Some(Ok(InlineFragment::new_with_external_targets(
        setup_entry,
        deps,
        &[then_label, else_label],
    )))
}

pub(super) fn lower_if_expr_into<L, E>(
    lowerer: &L,
    if_expr: crate::block_py::ExprIf<InstrRuff>,
    out: &mut crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    if let Some(lowered) = try_lower_if_expr_direct(lowerer, out.name_gen(), if_expr, loop_ctx) {
        let lowered = lowered?;
        out.append_fragment(lowered.setup);
        return Ok(lowered.value);
    }
    Err("if expression lowering requires inline fragment lowering".to_string())
}

#[cfg(test)]
mod test;
