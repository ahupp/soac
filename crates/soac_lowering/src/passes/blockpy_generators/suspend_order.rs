use crate::block_py::{
    instr_any, map_function_blocks, Block, BlockPyFunction, CallArgKeyword, CallArgPositional,
    FunctionNameGen, HasMeta, InstrWithConstantNone, InstrWithYield, LiteralValue, MapInstr,
    MapTerm, Mappable, Meta, Store, StoreLifetime, StringLiteral, TakeOperand, Tuple,
    UnresolvedName, WithMeta,
};
use crate::passes::ruff_to_blockpy::expr_lowering::call_arguments::{
    lower_source_call_phases, SourceCallPhaseBuilder,
};
use crate::passes::CoreModuleShapeWithYield;
use std::collections::HashSet;

fn expr_contains_suspend(expr: &InstrWithYield) -> bool {
    instr_any(expr, |expr| {
        matches!(
            expr,
            InstrWithYield::Yield(_) | InstrWithYield::YieldFrom(_)
        )
    })
}

struct SuspendOrder {
    names: FunctionNameGen,
    reserved: HashSet<String>,
}

impl SuspendOrder {
    fn capture(&mut self, value: InstrWithYield, out: &mut Vec<InstrWithYield>) -> UnresolvedName {
        let name = loop {
            let candidate = self.names.next_tmp_name("suspend_operand");
            if self.reserved.insert(candidate.to_string()) {
                break UnresolvedName::from(candidate);
            }
        };
        let unwind_order = self.names.next_temporary_sequence();
        let meta = value.meta();
        out.push(
            Store::new(name.clone(), value)
                .with_lifetime(StoreLifetime::Operand { unwind_order })
                .with_meta(meta)
                .into(),
        );
        name
    }

    fn take(name: UnresolvedName) -> InstrWithYield {
        TakeOperand::new(name).with_meta(Meta::synthetic()).into()
    }

    /// A top-level Yield/Store(Yield) is already in the generator producer's
    /// explicit form. Only a value used by a surrounding expression must move
    /// its resumed value through an Operand. This also makes the pass idempotent.
    fn expression(
        &mut self,
        expr: InstrWithYield,
        out: &mut Vec<InstrWithYield>,
        hoist_root: bool,
    ) -> InstrWithYield {
        if !expr_contains_suspend(&expr) {
            return expr;
        }
        let expr = match expr {
            InstrWithYield::Store(mut store) => {
                store.value = Box::new(self.expression(*store.value, out, false));
                InstrWithYield::Store(store)
            }
            InstrWithYield::Call(call)
                if call
                    .args
                    .iter()
                    .any(|arg| matches!(arg, CallArgPositional::Starred(_)))
                    || call
                        .keywords
                        .iter()
                        .any(|arg| matches!(arg, CallArgKeyword::Starred(_))) =>
            {
                let (_, prepared) = lower_source_call_phases(
                    call,
                    &mut SuspendCallPhaseBuilder { order: self, out },
                )
                .expect("core suspension call phases have already-lowered source operands");
                prepared.into()
            }
            other => self.ordered_children(other, out),
        };
        if hoist_root
            && matches!(
                expr,
                InstrWithYield::Yield(_) | InstrWithYield::YieldFrom(_)
            )
        {
            Self::take(self.capture(expr, out))
        } else {
            expr
        }
    }

    fn ordered_children(
        &mut self,
        parent: InstrWithYield,
        out: &mut Vec<InstrWithYield>,
    ) -> InstrWithYield {
        // Operation payloads (function ids, names, kinds) are not children.
        // Retain the existing runtime-child order and copy all parent metadata.
        let mut children = Vec::new();
        let skeleton = parent.map_same_children(&mut |child| {
            children.push(child);
            InstrWithYield::constant_none()
        });
        let mut pending: Vec<(InstrWithYield, bool)> = Vec::with_capacity(children.len());
        for child in children {
            if expr_contains_suspend(&child) {
                // Flush before lowering the later child, so both evaluation
                // and acquisition ranks precede any of its own temporary owners.
                for (value, captured) in &mut pending {
                    if !*captured {
                        let old = std::mem::replace(value, InstrWithYield::constant_none());
                        *value = Self::take(self.capture(old, out));
                        *captured = true;
                    }
                }
            }
            pending.push((self.expression(child, out, true), false));
        }
        let mut pending = pending.into_iter();
        let result = skeleton
            .map_same_children(&mut |_| pending.next().expect("same ordered child shape").0);
        assert!(pending.next().is_none(), "same ordered child shape");
        result
    }
}

struct SuspendCallPhaseBuilder<'a> {
    order: &'a mut SuspendOrder,
    out: &'a mut Vec<InstrWithYield>,
}

impl SourceCallPhaseBuilder<InstrWithYield> for SuspendCallPhaseBuilder<'_> {
    fn lower_input(&mut self, input: InstrWithYield) -> Result<InstrWithYield, String> {
        Ok(self.order.expression(input, self.out, true))
    }

    fn capture(&mut self, value: InstrWithYield) -> UnresolvedName {
        self.order.capture(value, self.out)
    }

    fn emit(&mut self, statement: InstrWithYield) {
        self.out.push(statement);
    }

    fn tuple(&mut self, values: Vec<InstrWithYield>, meta: &Meta) -> InstrWithYield {
        Tuple::new(values).with_meta(meta.clone()).into()
    }

    fn keyword_literal(&mut self, name: &str, meta: &Meta) -> InstrWithYield {
        LiteralValue::new(StringLiteral {
            value: name.to_owned(),
        })
        .with_meta(meta.clone())
        .into()
    }
}

struct SuspendTerm<'a> {
    order: &'a mut SuspendOrder,
    out: &'a mut Vec<InstrWithYield>,
}

impl MapInstr<InstrWithYield, InstrWithYield> for SuspendTerm<'_> {
    fn map_instr(&mut self, expr: InstrWithYield) -> InstrWithYield {
        self.order.expression(expr, self.out, true)
    }

    fn map_name(&mut self, name: UnresolvedName) -> UnresolvedName {
        name
    }
}

pub(crate) fn make_suspend_order_explicit_in_core_callable_def(
    callable_def: BlockPyFunction<CoreModuleShapeWithYield>,
) -> BlockPyFunction<CoreModuleShapeWithYield> {
    let mut order = SuspendOrder {
        names: callable_def.name_gen.share(),
        reserved: super::reserved_callable_names(&callable_def),
    };
    map_function_blocks(callable_def, |block| {
        let Block {
            label,
            body: input_body,
            term,
            params,
            exc_edge,
            extra,
        } = block;
        let mut body = Vec::new();
        for statement in input_body {
            let statement = order.expression(statement, &mut body, false);
            body.push(statement);
        }
        let term = SuspendTerm {
            order: &mut order,
            out: &mut body,
        }
        .map_term(term);
        Block {
            label,
            body,
            term,
            params,
            exc_edge,
            extra,
        }
    })
}

#[cfg(test)]
mod test;
