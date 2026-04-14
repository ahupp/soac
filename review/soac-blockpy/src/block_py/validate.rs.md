# soac-blockpy/src/block_py/validate.rs

## File Responsibilities

Validates structural invariants of BlockPy modules after lowering/instrumentation: dense labels,
known edge targets, legal edge argument forwarding, exception-edge argument shape, and consistency
between scope-derived closure layout and stored function layout.

## Datatypes

- None.

## Functions

- `validate_module`: validates every callable definition in a module.
- `validate_function`: validates a single function's storage layout, dense block labels, exception
  edges, and normal terminator edges.
- `validate_non_exception_edge`: validates an ordinary edge target and its parameter forwarding.
- `validate_edge_param_forwarding`: checks implicit/explicit block parameter forwarding rules.
- `validate_explicit_edge_arg`: ensures explicit edge args are valid for the target parameter role.
- `validate_storage_layout_scoping`: recomputes expected closure layout from scope facts and checks
  stored freevar/cellvar layout compatibility.
- `lookup_known_block`: resolves a target label to a block and reports unknown/non-dense labels.

## Context Read

- `soac-blockpy/src/block_py/mod.rs`
- `soac-blockpy/src/block_py/scope.rs`
