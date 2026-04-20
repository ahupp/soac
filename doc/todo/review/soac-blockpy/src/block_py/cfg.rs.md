# soac-blockpy/src/block_py/cfg.rs

## File Responsibilities

Small control-flow utilities for BlockPy functions: successor discovery, unreachable-block pruning,
jump folding, subexpression hoisting into temporaries, and dense block relabeling.

## Datatypes

- `HoistMatchingSubexpressionsInTerm`: adapter that implements `MapInstr` while accumulating setup
  statements and cleanup deletions for hoisted terminator expressions.
- `RelabelBlockTargets`: local trait for retargeting labels inside `BlockTerm`.

## Functions

- `blockpy_successors`: returns normal terminator successors for a block.
- `fold_jumps_to_trivial_return_blockpy`: replaces simple jumps to empty return blocks with the
  return terminator itself when exception context does not make that unsafe.
- `prune_unreachable_blockpy_blocks`: keeps blocks reachable from the entry label or extra roots.
- `fresh_eval_name`: creates a temporary evaluation name.
- `typed_store_expr`, `typed_del_expr`: build synthetic `Store`/`Del` instructions for hoisting.
- `append_stmt_cleanup`: emits cleanup deletes for hoisted temporaries in reverse order.
- `expr_contains_matching_subexpression`: recursively tests whether an expression tree contains a
  hoistable subexpression.
- `hoist_subexpression_if_matching`: recursively rewrites children and stores matching expressions
  into temporaries.
- `rewrite_matching_children_in_expr`: rewrites only child expressions while preserving the root.
- `HoistMatchingSubexpressionsInTerm::map_instr`: hoists expressions while mapping terminator
  children.
- `HoistMatchingSubexpressionsInTerm::map_name`: preserves unresolved names.
- `hoist_matching_subexpressions_in_term`: applies hoisting to expressions inside a terminator.
- `hoist_matching_subexpressions_in_callable_def`: applies subexpression hoisting to every block in
  a callable, placing setup/cleanup around affected statements.
- `relabel_blockpy_blocks_dense`: rewrites block labels to dense `bb0..bbN` labels and retargets
  normal/exception edges.
- `RelabelBlockTargets for BlockTerm::relabel_targets`: updates jump, if, and branch-table labels.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/map.rs`
- `soac-blockpy/src/block_py/visit.rs`
- `soac-blockpy/src/namegen.rs`
