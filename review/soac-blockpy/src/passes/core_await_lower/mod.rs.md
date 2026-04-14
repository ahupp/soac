# soac-blockpy/src/passes/core_await_lower/mod.rs

## File Responsibilities
Lowers core expressions that may contain `await` into yield-capable core expressions by translating awaits to runtime await-iterator/yield-from protocol operations.

## Datatypes
- `CoreAwaitLoweringMap`: mapper from `InstrWithAwaitAndYield` to `InstrWithYield`.

## Functions
- `CoreAwaitLoweringMap::map_instr`: rewrites `Await` nodes and maps all other instructions recursively.
- `CoreAwaitLoweringMap::map_name`: preserves unresolved names.
- Public lowering function in this module applies the mapper to a whole core module.

## Context Read
- `soac-blockpy/src/passes/mod.rs` for await/yield instruction variants.
- `soac_py/src/soac/runtime.py` for `await_iter`/yield-from runtime helpers.
