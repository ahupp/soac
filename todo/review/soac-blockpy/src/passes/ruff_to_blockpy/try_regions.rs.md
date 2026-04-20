# soac-blockpy/src/passes/ruff_to_blockpy/try_regions.rs

## File Responsibilities

Builds and finalizes structured try/except/else/finally regions in BlockPy. It creates cleanup and
dispatch blocks for finally, wires exception and normal continuations, rewrites returns through
finally when needed, and exposes label-reference checks used by CFG cleanup.

## Datatypes

- `TryPlan`: generated names and labels needed to lower a try statement, including exception temp,
  finally abrupt-kind/payload slots, dispatch label, return label, and raise label.
- `LoweredTryRegions`: labels and block-index ranges produced for try body, else, except, and
  finally regions.

## Functions

- `expr_name`: builds a Ruff name object.
- `build_try_plan`: allocates exception names and optional finally dispatch/return/raise labels.
- `TryPlan::finally_cont_label`: chooses the finally dispatch label or rest continuation.
- `prepare_finally_body`: returns the finalbody suite for lowering.
- `prepare_except_body`: returns the first except handler body or a default re-raise body.
- `lower_try_regions`: lowers finally, else, except, and body regions; creates finally normal and
  exception entry shims; and emits finally abrupt dispatch blocks when required.
- `finalize_try_regions`: rewrites returns through finally, sets exception params on handler/finally
  ranges, and emits the try entry block.
- `rewrite_region_returns_to_finally_blockpy`: replaces returns in a region with payload stores and
  jumps to finally carrying an abrupt-kind argument.
- `emit_finally_abrupt_dispatch_blocks`: emits return, raise, and branch-table dispatch blocks for
  finally completion.
- `emit_try_jump_entry`: lowers linear prefix statements and emits the jump into the try body.
- `block_references_label`: checks whether a block's terminator or exception edge references a
  label.

## Context Read

- `soac-blockpy/src/passes/ruff_to_blockpy/stmt_sequences.rs`
- `soac-blockpy/src/passes/ruff_to_blockpy/compat.rs`
- `soac-blockpy/src/block_py/mod.rs`

