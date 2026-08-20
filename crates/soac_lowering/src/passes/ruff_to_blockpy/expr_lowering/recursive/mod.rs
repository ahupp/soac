use super::{BlockPySetupExprLowerer, RuffToBlockPyExpr};
use crate::block_py::Mappable;
use crate::passes::ruff_to_blockpy::expr_lowering::boolop_compare::{
    lower_boolop_into, lower_compare_into,
};
use crate::passes::ruff_to_blockpy::expr_lowering::if_expr::lower_if_expr_into;
use crate::passes::ruff_to_blockpy::expr_lowering::named_expr::lower_named_expr_into;
use crate::passes::ruff_to_blockpy::stmt_lowering::BlockPyStmtBuilder;
use crate::passes::ruff_to_blockpy::LoopContext;
use crate::passes::InstrRuff;

pub(super) fn lower_expr_ast_recursive<L, E>(
    lowerer: &L,
    expr: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    match expr {
        InstrRuff::Call(call)
            if lowerer.recorded_call_runtime_start(&call).is_none()
                && (call
                    .args
                    .iter()
                    .any(|arg| matches!(arg, crate::block_py::CallArgPositional::Starred(_)))
                    || call
                        .keywords
                        .iter()
                        .any(|arg| matches!(arg, crate::block_py::CallArgKeyword::Starred(_)))) =>
        {
            super::call_arguments::lower_call_with_setup(lowerer, call, out, loop_ctx)
        }
        InstrRuff::ExprBoolOp(bool_op) => lower_boolop_into(lowerer, bool_op, out, loop_ctx),
        InstrRuff::ExprCompare(compare) => lower_compare_into(lowerer, compare, out, loop_ctx),
        InstrRuff::ExprIf(if_expr) => lower_if_expr_into(lowerer, if_expr, out, loop_ctx),
        InstrRuff::ExprNamed(named_expr) => {
            lower_named_expr_into(lowerer, named_expr, out, loop_ctx)
        }
        expr @ (InstrRuff::StmtFunctionDef(_)
        | InstrRuff::StmtClassDef(_)
        | InstrRuff::StmtReturn(_)
        | InstrRuff::StmtDelete(_)
        | InstrRuff::StmtTypeAlias(_)
        | InstrRuff::StmtAssign(_)
        | InstrRuff::StmtAugAssign(_)
        | InstrRuff::StmtAnnAssign(_)
        | InstrRuff::StmtFor(_)
        | InstrRuff::StmtWhile(_)
        | InstrRuff::StmtIf(_)
        | InstrRuff::StmtWith(_)
        | InstrRuff::StmtMatch(_)
        | InstrRuff::StmtRaise(_)
        | InstrRuff::StmtTry(_)
        | InstrRuff::StmtAssert(_)
        | InstrRuff::StmtImport(_)
        | InstrRuff::StmtImportFrom(_)
        | InstrRuff::StmtGlobal(_)
        | InstrRuff::StmtNonlocal(_)
        | InstrRuff::StmtExpr(_)
        | InstrRuff::StmtPass(_)
        | InstrRuff::StmtBreak(_)
        | InstrRuff::StmtContinue(_)
        | InstrRuff::StmtIpyEscapeCommand(_)) => Err(format!(
            "statement-shaped InstrRuff reached expression lowering: {expr:?}"
        )),
        other => lower_ordered_children(lowerer, other, out, loop_ctx),
    }
}

/// The first child boundary with an implicit parent operation that is not
/// yet represented in this setup IR. Value capture alone cannot move iterable
/// expansion or dictionary merging across a later child's control flow.
/// TODO: represent these prefix operations before admitting their crossings.
fn unrepresented_prefix_boundary(parent: &InstrRuff) -> Option<(usize, &'static str)> {
    fn first_starred(values: &[InstrRuff]) -> Option<usize> {
        values
            .iter()
            .position(|value| matches!(value, InstrRuff::ExprStarred(_)))
            .map(|index| index.max(1))
    }
    match parent {
        InstrRuff::Call(call) => {
            let positional = call
                .args
                .iter()
                .position(|arg| matches!(arg, crate::block_py::CallArgPositional::Starred(_)))
                .map(|index| index + 2);
            let keyword = call
                .keywords
                .iter()
                .position(|arg| matches!(arg, crate::block_py::CallArgKeyword::Starred(_)))
                .map(|index| 1 + call.args.len() + index + 1);
            positional
                .or(keyword)
                .map(|index| (index, "call unpacking"))
        }
        InstrRuff::ExprTuple(tuple) => {
            first_starred(&tuple.elts).map(|index| (index, "tuple unpacking"))
        }
        InstrRuff::ExprList(list) => {
            first_starred(&list.elts).map(|index| (index, "list unpacking"))
        }
        InstrRuff::ExprSet(set) => first_starred(&set.elts).map(|index| (index, "set unpacking")),
        InstrRuff::ExprDict(dict) => {
            let mut child_index = 0;
            for item in &dict.items {
                if item.key.is_none() {
                    return Some((child_index.max(1), "dictionary unpacking"));
                }
                child_index += 2;
            }
            None
        }
        _ => None,
    }
}

struct PendingOperand {
    value: InstrRuff,
    evaluated: bool,
    /// Reserve in source evaluation order, before lowering later children.
    /// Reserving only when a later setup is discovered would give these older
    /// owners a newer unwind order than that setup's own temporaries.
    unwind_order: u64,
}

fn lower_ordered_children<L, E>(
    lowerer: &L,
    parent: InstrRuff,
    out: &mut BlockPyStmtBuilder<E>,
    loop_ctx: Option<&LoopContext>,
) -> Result<InstrRuff, String>
where
    L: BlockPySetupExprLowerer + ?Sized,
    E: RuffToBlockPyExpr,
{
    use crate::block_py::{HasMeta, Meta, Store, StoreLifetime, TakeOperand, WithMeta};

    let implicit_prefix = unrepresented_prefix_boundary(&parent);
    let runtime_children_start = match &parent {
        InstrRuff::Call(call) => lowerer.recorded_call_runtime_start(call).unwrap_or(0),
        _ => 0,
    };
    let mut children = Vec::new();
    let skeleton = parent.map_same_children(&mut |child| {
        children.push(child);
        crate::block_py::ExprNoneLiteral::new().into()
    });
    let mut pending: Vec<PendingOperand> = Vec::with_capacity(children.len());
    for (child_index, child) in children.into_iter().enumerate() {
        if child_index < runtime_children_start {
            pending.push(PendingOperand {
                value: child,
                evaluated: true,
                unwind_order: 0,
            });
            continue;
        }
        let mut setup = BlockPyStmtBuilder::<E>::new(out.name_gen());
        let value = lowerer.lower_expr_instr_into(child, &mut setup, loop_ctx)?;
        if !setup.is_empty() {
            if let Some((boundary, kind)) = implicit_prefix {
                if child_index >= boundary {
                    return Err(format!(
                        "expression setup crosses an unrepresented {kind} prefix; expansion and merge preparation are required"
                    ));
                }
            }
            for operand in &mut pending {
                if operand.evaluated {
                    continue;
                }
                let binding = lowerer.fresh_operand_binding();
                let value = std::mem::replace(
                    &mut operand.value,
                    crate::block_py::ExprNoneLiteral::new().into(),
                );
                let meta = value.meta();
                out.push_stmt(
                    Store::new(binding.clone(), E::from_lowered_expr(value))
                        .with_lifetime(StoreLifetime::Operand {
                            unwind_order: operand.unwind_order,
                        })
                        .with_meta(meta)
                        .into(),
                );
                operand.value = TakeOperand::new(binding)
                    .with_meta(Meta::synthetic())
                    .into();
                operand.evaluated = true;
            }
            out.append_fragment(setup.finish_fallthrough());
        }
        pending.push(PendingOperand {
            value,
            evaluated: false,
            unwind_order: out.name_gen().next_temporary_sequence(),
        });
    }
    let mut values = pending.into_iter().map(|operand| operand.value);
    let result = skeleton.map_same_children(&mut |_| {
        values
            .next()
            .expect("ordered child reconstruction has the same arity")
    });
    debug_assert!(values.next().is_none());
    Ok(result)
}

#[cfg(test)]
mod test;
