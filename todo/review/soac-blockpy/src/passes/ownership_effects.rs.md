# soac-blockpy/src/passes/ownership_effects.rs

## File Responsibilities
Computes and validates refcount ownership plans for codegen-stage BlockPy. It models local liveness, must-bound state, value ownership facts, forwarding across edges, and emits planned incref/decref/release actions at stores, deletes, returns, jumps, and exceptional exits.

## Datatypes
- `LocalRefState`: abstract refcount state for a local value.
- `RefcountLocal`: identifies a local by function/location for refcount actions.
- `RefcountSite`: site where an action occurs, including instruction, terminator, or edge context.
- `RefcountReleaseReason`: why a value is released.
- `RefcountActionKind`: incref/decref/release action category.
- `RefcountAction`: one planned ownership operation.
- `BlockRefcountPlan`, `FunctionRefcountPlan`, and `RefcountPlan`: nested ownership plans.
- `LocalLiveness`, `LocalMustBound`, and `BlockLocalEffects`: internal dataflow records for local use/def/delete/forwarding behavior.

## Functions
- Plan accessors `FunctionRefcountPlan::block` and `RefcountPlan::function` query nested plans.
- `plan_ownership_effects`: computes the module-wide refcount plan.
- `validate_ownership_effects`: checks a refcount plan against the module.
- `validate_function_refcount_plan` and `validate_block_refcount_plan`: validate per-function/per-block action placement.
- `plan_function_refcounts` and `plan_block_refcounts`: main planning algorithms for one function/block.
- `compute_function_local_live_ins` / `compute_local_liveness`: backward liveness for locals and cells.
- `compute_function_local_must_bound_ins`, `compute_local_must_bound`, and `transfer_must_bound_through_block`: forward must-bound analysis.
- State helpers `initial_block_env`, `state_for_expr`, `expr_is_deleted_sentinel`, `deleted_sentinel_constant_slots`, and `state_for_py_facts` classify current local states.
- Local-use helpers `block_local_effects`, `owned_cell_locations`, `store_binding_location`, `mark_local_use`, `mark_cell_use`, `collect_local_reads`, and `collect_term_local_reads` extract use/def effects.
- Edge/release helpers `block_successors`, `forwarded_locations`, `preserved_locations`, `release_unforwarded_locals`, `release_all_live_locals`, `sorted_live_releases`, and `push_release_action` create action lists.
- Test/validation helpers build expected action sets and assert exact action placement.

## Context Read
- `local_env_plan.rs` for local storage/resume concepts.
- `value_facts.rs` for immortal/singleton/deleted facts.
- `soac-blockpy/src/block_py/cfg.rs` and scope metadata for block/control-flow details.
