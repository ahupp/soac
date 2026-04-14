# soac-blockpy/src/passes/blockpy_generators/suspend_order.rs

## File Responsibilities
Provides a small predicate for whether a yield-capable expression contains suspension, used by generator lowering to preserve execution order.

## Datatypes
- None.

## Functions
- `expr_contains_suspend`: recursively checks an `InstrWithYield` expression for yield or yield-from suspension points.

## Context Read
- `blockpy_generators/mod.rs` for generator lowering consumers.
- `soac-blockpy/src/passes/mod.rs` for `InstrWithYield` variants.
