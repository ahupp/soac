# soac-blockpy/src/passes/ruff_to_blockpy/bb_shape.rs

## File Responsibilities

Converts structured BlockPy blocks into plain basic-block-shaped blocks and repairs exception-edge
metadata. It also rewrites synthetic `__soac__.current_exception()` calls inside exception handler
regions into direct loads from the block's exception parameter.

## Datatypes

- `CurrentExceptionExpr`: trait over BlockPy instruction forms that can recognize a zero-argument
  `current_exception` helper call and can be rebuilt as a load.

## Functions

- `lower_structured_blocks_to_bb_blocks`: clones structured blocks into BB blocks, preserving body,
  terminator, params, and exception edge, then populates exception-edge arguments.
- `CurrentExceptionExpr::is_current_exception_call` for `InstrLow`, `InstrResolved`, and
  `InstrWithAwaitAndYield`: recognizes `current_exception()` calls in each instruction variant.
- `rewrite_current_exception_in_core_blocks`: walks blocks with exception params and rewrites
  `current_exception()` uses in bodies and terms to load that param.
- `rewrite_current_exception_in_core_blocks_with_await_and_yield`: typed wrapper for
  `InstrWithAwaitAndYield` blocks.
- `rewrite_current_exception_in_term`: walks terminators, rewrites nested `current_exception()`, and
  fills bare `raise` terms with the current exception name.
- `rewrite_current_exception_in_expr`: recursively rewrites expression children, then replaces a
  matching helper call with a load of the exception param.
- `populate_exception_edge_args`: computes arguments for exception edges by matching target params
  against source params and exception/current-exception params.
- `lowered_exception_edges`: returns a label-to-exception-target summary for already lowered blocks.
- `current_exception_name_expr`: builds a synthetic load expression for an exception param.
- `compat_node_index`, `compat_range`: provide default metadata for synthetic compatibility nodes.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/instr.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/mod.rs`

