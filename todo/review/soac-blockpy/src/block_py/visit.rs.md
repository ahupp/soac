# soac-blockpy/src/block_py/visit.rs

## File Responsibilities

Defines immutable and mutable visitor traits for BlockPy modules, functions, blocks, terms, edges,
and instruction trees, plus default walking functions. This is the read/mutate traversal counterpart
to the ownership-consuming mapping traits.

## Datatypes

- `ChildVisitable<E>`: trait implemented by operation payloads/instruction enums that can expose
  child expressions.
- `Visit<I>`: immutable visitor with overridable hooks for instructions, terms, edges, labels,
  block args, blocks, functions, and modules.
- `VisitMut<I>`: mutable visitor counterpart.

## Functions

- `instr_any`: recursively tests an instruction tree with a predicate.
- `Visit` default methods: delegate to `walk_*` functions while allowing targeted hook overrides.
- `VisitMut` default methods: mutable equivalents.
- `walk_module`, `walk_module_mut`: visit callable definitions.
- `walk_fn`, `walk_fn_mut`: visit function blocks.
- `walk_block`, `walk_block_mut`: visit block params, body statements, exception edge, and term.
- `walk_stmt`, `walk_stmt_mut`: visit a statement instruction.
- `walk_edge`, `walk_edge_mut`: visit an edge target and args.
- `walk_term`, `walk_term_mut`: dispatch to jump/if/branch-table/raise/return hooks.
- `walk_if_term`, `walk_if_term_mut`: visit condition and branch labels.
- `walk_branch_table_term`, `walk_branch_table_term_mut`: visit branch index and labels.
- `walk_raise_term`, `walk_raise_term_mut`: visit optional raised exception.
- `walk_expr`, `walk_expr_mut`: delegate to `ChildVisitable` child traversal.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/operation_macro.rs`
