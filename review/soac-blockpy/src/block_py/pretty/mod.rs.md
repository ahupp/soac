# soac-blockpy/src/block_py/pretty/mod.rs

## File Responsibilities

Pretty-printer for BlockPy modules. It renders functions, blocks, metadata, terminators, closure
layouts, and structured CFG nesting used in snapshots and debugging.

## Datatypes

- `IfBranchKind`: branch side key for inlining then/else target blocks in rendered output.
- `BlockPyPrettyPrinter`: per-module-shape hook for rendering block metadata.
- `BlockPyPrettyPrint`: trait for values that can render user/debug text.
- `InlineExprRenderer<E>` and `DebugInlineExprRenderer`: expression rendering abstraction; current
  implementation uses `Debug`.
- `BlockPyFormatter<R>`: mutable formatter state with output buffer and indentation.
- `BlockRenderLayout`: computed dominator/child layout used to render blocks in a readable nested
  order and inline one-predecessor if targets.

## Functions

- `impl_default_blockpy_pretty_printer!`: implements default metadata rendering for several module
  shapes.
- `BlockPyPrettyPrinter` impls: customize metadata for resolved-storage and codegen module shapes.
- `render_resolved_storage_block_metadata`, `render_blockpy_block_metadata`: render params,
  exception targets, and exception param metadata.
- `BlockPyPrettyPrint::pretty_print`, `debug_pretty_print`: standard render entry points.
- `blockpy_module_to_string`: formats a whole module using debug expression rendering.
- `DebugInlineExprRenderer::render`: renders an expression with `Debug`.
- `BlockPyFormatter::finish`, `write_module`, `write_function`, `write_function_block`,
  `write_block_contents`, `write_linear_stmt_list`, `write_linear_stmt`, `write_term`,
  `write_raise`, `with_indent`, `line`: structured rendering primitives for modules, blocks,
  statements, and terminators.
- `render_closure_slots`, `closure_init_name`, `function_kind_name`, `format_parameters`,
  `join_labels`, `render_edge`, `render_block_arg`, `render_block_header`,
  `render_block_param_role`, `bb_expr_text`: formatting helpers for common payloads.
- `BlockRenderLayout::new`: computes reachable roots, dominator tree children, and inlineable if
  targets for readable output.
- `sort_block_indices_by_label`, `compute_inline_if_term_targets`,
  `can_inline_if_term_target`: choose block rendering order and inlineable branch targets.
- `collect_top_level_successors_from_block`, `collect_top_level_successors_from_linear_stmts`,
  `collect_top_level_successors_from_term`, `push_top_level_successor`: collect CFG successors for
  rendering layout.
- `collect_predecessors`, `collect_discovery_order`, `compute_dominators`,
  `compute_immediate_dominators`: graph algorithms for rendering hierarchy.
- `collect_referenced_labels_from_blocks`, `collect_referenced_labels_from_term`: find labels that
  should remain visible in rendered output.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/param_specs/mod.rs`
- `soac-blockpy/src/passes/mod.rs`
