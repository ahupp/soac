use crate::block_py::cfg::hoist_matching_subexpressions_in_callable_def;
use crate::block_py::{instr_any, BlockPyFunction, CoreBlockPyExprWithYield};
use crate::passes::CoreBlockPyPassWithYield;

fn expr_contains_suspend(expr: &CoreBlockPyExprWithYield) -> bool {
    instr_any(expr, |expr| {
        matches!(
            expr,
            CoreBlockPyExprWithYield::Yield(_) | CoreBlockPyExprWithYield::YieldFrom(_)
        )
    })
}

pub(crate) fn make_suspend_order_explicit_in_core_callable_def(
    callable_def: BlockPyFunction<CoreBlockPyPassWithYield>,
) -> BlockPyFunction<CoreBlockPyPassWithYield> {
    hoist_matching_subexpressions_in_callable_def(callable_def, expr_contains_suspend)
}

#[cfg(test)]
mod test;
