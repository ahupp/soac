# soac-blockpy/src/passes/ruff_to_blockpy/mod.rs

## File Responsibilities

Top-level Ruff-to-BlockPy lowering module. It wires submodules together, exposes the public lowering
entrypoints used by later passes, defines inline fragment builders used by expression and statement
lowering, and builds full core BlockPy callable definitions from runtime-input instruction bodies.

## Datatypes

- `LoweredBlockPyBlock<E>`: alias for a BlockPy `Block<E>` produced by this lowering stage.
- `InlineBlockRef`: lightweight wrapper around a block label used as a stable fragment entry
  reference.
- `InlineBlockBuilder<I>`: mutable builder for a sequence of inline BlockPy blocks with a current
  block plus dependency blocks.
- `InlineFragment<I>`: completed inline fragment with one entry block and dependency blocks.
- `LoweredExpr<S, V>`: pair of setup fragment and final value for an expression that cannot be
  represented as a single expression node.
- `StmtSequenceHeadPlan`: statement-sequence planning result identifying linear statements,
  expanded statements, structured heads, terminators, and unsupported heads.
- `LoopContext`: local loop labels passed while lowering nested statements.
- `LoopLabels`: break/continue labels for the current loop.
- `RegionTargets`: normal continuation, optional loop labels, and optional active exception target
  for a lowered region.

## Functions

- `InlineBlockRef::from_label`, `InlineBlockRef::label`: construct and read inline block refs.
- `InlineBlockBuilder::new`: starts a fragment with a fresh entry label.
- `InlineBlockBuilder::push_stmt`, `name_gen`, `entry_ref`, `set_term`,
  `ensure_fallthrough_term`: mutate or inspect the current inline block.
- `InlineBlockBuilder::append_fragment`: appends another fragment after the current block and
  creates a fresh continuation block.
- `InlineBlockBuilder::{finish_blocks_with_term,finish_fallthrough,finish_fallthrough_blocks}`:
  finish builders with explicit or fallthrough terms.
- `InlineBlockBuilder::finish_linear_block`: finishes as one caller-labeled block if no dependency
  blocks were emitted.
- `InlineBlockBuilder::{can_finish_linear_block,is_empty}`: inspect builder shape.
- `InlineBlockBuilder::flush_current_block`: internal helper that moves the current block into
  dependencies.
- `InlineBlockBuilder::finish_fragment`: finishes blocks and separates the entry block from deps.
- `InlineBlockBuilder::finish_blocks`: completes the builder and ensures the original entry label
  is represented.
- `InlineFragment::new`: constructs and validates an inline fragment.
- `InlineFragment::assert_well_formed`: checks labels are unique and all internal targets are in
  the fragment or fallthrough.
- `InlineFragment::entry_ref`: returns the fragment entry reference.
- `rewrite_ast_to_core_blockpy_module_with_module`: top-level lowering wrapper for
  `CoreModuleShapeWithAwaitAndYield`.
- `test_name_gen`: test-only function-name generator factory.
- `attach_exception_edges_to_blocks`: overlays saved exception-edge targets onto lowered blocks.
- `move_entry_block_to_front`: makes the entry block first when present.
- `build_core_blockpy_callable_def_from_runtime_input`: lowers a callable body to blocks, adds an
  implicit return block when needed, prunes CFG, converts to BB shape, and returns a
  `BlockPyFunction`.
- `RegionTargets::{new,nested,nested_with_loop}`: create derived region target sets.
- `assign_delete_error`: formats unsupported assignment/delete target diagnostics with source text.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/module_plan/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_sequences.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/bb_shape.rs`
- `soac-blockpy/src/block_py/mod.rs`

