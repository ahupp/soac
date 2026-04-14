# soac-blockpy/src/passes/blockpy_to_bb/exception_pass/mod.rs

## File Responsibilities
Splits and rewrites lowered BlockPy basic blocks so expressions that may update exception state are followed by explicit exception-check control flow. This makes Python exception propagation visible in the CFG before codegen.

## Datatypes
- None.

## Functions
- `lower_try_jump_exception_flow`: module entry point that applies exception-flow lowering to every function.
- `lower_function_try_jump_exception_flow`: applies block splitting to one function.
- `split_exception_blocks_for_expr_checks`: scans instructions, finds operations that update exception state, and inserts exception-check blocks/terms.
- `op_updates_exception_state`: classifies operations that can change the current Python exception state.

## Context Read
- `soac-blockpy/src/block_py/cfg.rs` for block and terminator structures.
- `soac-blockpy/src/passes/blockpy_to_bb/mod.rs` for pass sequencing.
