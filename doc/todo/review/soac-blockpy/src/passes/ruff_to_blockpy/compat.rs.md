# soac-blockpy/src/passes/ruff_to_blockpy/compat.rs

## File Responsibilities

Provides compatibility helpers for converting lowered inline fragments and linear statement
prefixes into BlockPy blocks with correct fallthrough and exception-edge wiring. It is the bridge
between expression lowering, statement-sequence lowering, and block construction for returns,
raises, branches, loops, and for-loop setup/check bodies.

## Datatypes

- None.

## Functions

- `try_lower_direct_expr`: tries to lower branching expressions, including if-expressions,
  boolops, and chained comparisons, as inline fragments instead of linear setup statements.
- `with_exc_meta`: attaches an optional exception edge to a block.
- `compat_block_from_inline_with_exc_target_and_expr`: rewrites fallthrough targets on an inline
  block and attaches exception metadata.
- `emit_inline_fragment_with_exc_target_and_expr`: emits an `InlineFragment` into a block list.
- `compat_block_from_blockpy_with_exc_target_and_expr`: lowers a Ruff instruction body through the
  production statement path, then builds one compatibility block.
- `compat_block_from_lowered_builder_with_exc_target_and_expr`: finishes a linear builder into one
  block and attaches an exception edge.
- `emit_lowered_builder_fragment_with_exc_target_and_expr`: emits all blocks produced by an inline
  builder with rewritten fallthrough and exception metadata.
- `emit_lowered_builder_fragment_with_preferred_linear_entry_and_expr`: prefers a caller-supplied
  label when a builder is still a single linear block; otherwise emits a fragment normally.
- `emit_lowered_builder_fragment_with_required_entry_and_expr`: guarantees an entry label by adding
  a trampoline block if the preferred-entry path could not use it.
- `set_region_exc_param`: sets the exception parameter on a block range and rewrites edge args if
  the parameter name changed.
- `rename_exception_edge_args`: rewrites jump and exception-edge args from an old exception param
  name to a new one.
- `emit_sequence_jump_block`: lowers a linear prefix and emits a jump to a target label.
- `emit_sequence_return_block_with_expr_setup_and_expr`: emits a linear prefix plus return, using
  direct expression fragments when the return value needs branch-shaped setup.
- `emit_sequence_raise_block_with_expr_setup_and_expr`: emits a linear prefix plus raise, using
  direct expression fragments when the raised expression needs branch-shaped setup.
- `emit_if_branch_block_with_expr_setup_and_expr`: emits a block or fragment for an if-test and
  branch terminator.
- `emit_simple_while_blocks_with_expr_setup_and_expr`: emits while test blocks and optional linear
  loop-prefix blocks.
- `emit_for_loop_blocks`: emits for/async-for setup, next/anext sentinel check, target assignment,
  and branch wiring.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/expr_lowering/mod.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_sequences.rs`
- `soac-blockpy/src/block_py/mod.rs`

