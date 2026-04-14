# soac-blockpy/src/passes/name_binding.rs

## File Responsibilities
Resolves unresolved source names into concrete storage locations for globals, locals, cells, class locals, runtime names, builtins, deleted sentinels, and direct function references. It also applies specialization/profile information for builtin/global/class access, records storage layouts, and validates name-binding invariants.

## Datatypes
- The file defines several internal visitor/planner structs for binding functions, blocks, and expressions; these track active scope, storage layout, known runtime names, direct functions, and specialization input.
- Internal binding result/state datatypes represent whether a name is local, global, cell, free, class-local, runtime, deleted, or unresolved at a given point.
- Validation helpers track expected bindings and report consistency errors.

## Functions
- Public module-level entry points lower unresolved modules through name binding into `ResolvedStorageModuleShape`, including default and specialization-aware variants.
- Scope/layout builders compute function storage layouts, closure cells, freevars, class namespace handling, and parameter/local binding locations.
- Name-resolution helpers classify loads/stores/deletes for local, global, nonlocal, cell, class, runtime, and builtin names.
- Unsound/specialization helpers identify undeclared known builtins, direct function targets, global index opportunities, and field/global specialization candidates.
- Visitor implementations walk instructions and terminators, rewrite `Load`/`Store`/`Del` names, update `MakeFunction` closure/default references, and attach callable/storage metadata.
- Validation helpers check that resolved names have valid locations and that function storage layouts match their scope metadata.

## Context Read
- `soac-blockpy/src/block_py/scope.rs` for storage layout and callable scope models.
- `soac-blockpy/src/passes/ast_to_ast/semantic/mod.rs` for semantic scope information feeding binding.
- `global_index.rs`, `local_env_plan.rs`, and `value_facts.rs` for downstream consumers of resolved names.
