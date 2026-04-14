# soac-blockpy/src/block_py/counters.rs

## File Responsibilities

Defines BlockPy counter metadata and the counter-increment instruction used by instrumentation
passes and later codegen.

## Datatypes

- `CounterId`: stable numeric identifier for a counter definition.
- `CounterScope`: lifetime/aggregation scope for a counter: current site, function, or global.
- `CounterSite`: source mapping for a counter, either a block entry or a runtime/semantic
  instruction site.
- `CounterDef`: declared counter with id, scope, kind string, and site metadata.
- `IncrementCounter`: IR operation that increments a declared counter id.

## Functions

- Macro-generated `IncrementCounter::new`: constructs an increment operation.
- Macro-generated `IncrementCounter::meta` / `with_meta`: expose and update source metadata.
- Macro-generated visitor/map helpers for `IncrementCounter`: no-op child traversal because the
  instruction has no child expressions.

## Context Read

- `soac-blockpy/src/block_py/operation_macro.rs`
- `soac-blockpy/src/block_py/meta.rs`
- `soac-blockpy/src/block_py/name_gen.rs`
