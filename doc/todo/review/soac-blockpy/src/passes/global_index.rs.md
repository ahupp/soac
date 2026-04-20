# soac-blockpy/src/passes/global_index.rs

## File Responsibilities
Assigns stable integer slots to module globals and rewrites resolved global loads/stores/deletes to indexed global locations.

## Datatypes
- `ModuleGlobalSlots`: name-to-slot allocator preserving global name order.
- `GlobalIndexer`: mutable visitor that rewrites global `ResolvedName` locations using `ModuleGlobalSlots`.

## Functions
- `ModuleGlobalSlots::with_preferred_names`: initializes slots with existing and preferred global names.
- `ModuleGlobalSlots::slot_for`: returns or allocates a global slot.
- `ModuleGlobalSlots::into_names`: returns slot-ordered global names.
- `GlobalIndexer::index_name`: rewrites one global name to a slot location.
- `GlobalIndexer::visit_instr_mut`: visits load/store/delete operations and indexes global names.
- `lower_global_index_in_resolved_module`: applies indexing to a module with preferred names.
- `lower_global_index_in_resolved_module_default`: default entry point with no extra preferred names.

## Context Read
- `soac-blockpy/src/block_py/name_gen.rs` and name/location definitions in `block_py`.
- `name_binding.rs` for where global locations originate.
