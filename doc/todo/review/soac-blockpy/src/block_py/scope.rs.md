# soac-blockpy/src/block_py/scope.rs

## File Responsibilities

Models scope/name-binding facts and derives closure storage layouts from lowered BlockPy functions.
It determines whether names bind locally, globally, through class namespace fallback, or through
closure cells, and it collects freevar/cellvar/runtime-cell layout data used by MakeFunction and
codegen.

## Datatypes

- `BindingTarget`: target storage class for a load/store: local, module global, or class namespace.
- `CellBindingKind`: whether a cell binding owns the cell or captures one from an outer scope.
- `BindingKind`: raw binding classification before class-body and internal-name adjustments.
- `StorageLayout`: function closure layout: freevars, cellvars, runtime cells, and stack slots.
- `ClosureSlot`: logical name, storage name, and initialization mode for one closure slot.
- `ClosureInit`: how a closure slot should be initialized.
- `CallableScopeKind`: function, class, or module scope.
- `ClassBodyFallback`: whether class-body lookup fallback is global or cell based.
- `EffectiveBinding`: binding after scope-kind/type-param/internal-name rules.
- `BindingPurpose`: load versus store, because class-body semantics differ.
- `CellCaptureBinding`: logical capture name plus source storage name.
- `CallableScopeInfo`: collected scope metadata for one callable.
- `ScopeExprNode`: trait exposing root-level name/use/def/delete/cell-ref observations for
  different instruction stages.
- `StorageLayoutScopeCollector`: visitor that gathers used/defined/deleted/cell-ref names for
  storage-layout derivation.

## Functions

- `derive_effective_binding_for_name`: applies CPython class-scope and internal-name rules to a raw
  binding.
- `StorageLayout` methods: lookup closure slots, list/test storage names, and manage stack slots.
- `CallableScopeInfo::honors_internal_binding`, `binding_kind`, `has_local_def`,
  `effective_binding`: inspect scope facts.
- `CallableScopeInfo::insert_binding`, `insert_binding_with_cell_names`: record raw/effective
  bindings and optional cell storage names.
- `CallableScopeInfo::resolved_load_binding_kind`, `is_cell_binding`, `cell_storage_name`,
  `cell_capture_source_name`, `cell_ref_source_name`: derive storage names and binding behavior.
- `CallableScopeInfo::logical_name_for_cell_capture_source`, `logical_name_for_cell_storage`:
  reverse-map storage names to logical names.
- `CallableScopeInfo::binding_target_for_name`: choose the load/store target storage class.
- `CallableScopeInfo::owned_cell_storage_names`, `captured_cell_logical_names`,
  `captured_cell_bindings`, `local_cell_storage_names`: compute closure ownership/capture sets.
- `ScopeExprNode` default methods: no-op observation hooks.
- `call_root_cell_ref_logical_name`: recognizes runtime `cell_ref("name")` helper calls.
- `walk_assigned_name_targets_in_instr_ruff`: walks assignment/delete targets in Ruff-shaped IR.
- `ScopeExprNode` impls for `InstrRuff`, `InstrWithAwaitAndYield`, `InstrWithYield`,
  `InstrLow<N>`, `InstrResolved`, and `InstrCodegen`: expose stage-specific name and cell-ref
  observations.
- `StorageLayoutScopeCollector::visit_instr`, `visit_block`, `visit_stmt`: collect names from
  instructions, block exception params, and deleted names.
- `is_runtime_closure_name`: identifies internal closure state cells.
- `compute_make_function_capture_bindings_from_scope`: computes capture bindings needed by
  `MakeFunction`.
- `compute_storage_layout_from_scope`: derives the function's full closure layout from scope info.
- `build_storage_layout_from_capture_names`: builds freevar/cellvar layout from explicit capture
  names and local cell slots.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/passes/ast_to_ast/scope_helpers.rs`
- `soac-blockpy/src/block_py/visit.rs`
