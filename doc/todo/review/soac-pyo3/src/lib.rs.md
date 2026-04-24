# crates/soac_pyo3/src/lib.rs

## File Responsibilities

Defines the `_soac_ext` Python extension module. It exposes counter-dump inspection, indexed module type construction, and the JIT runtime bridge functions implemented in `jit_runtime.rs`.

## Datatypes

- No local structs/enums/classes are defined.

## Functions

- `lowering_error_to_pyerr`: maps SOAC lowering errors to Python syntax/runtime exceptions.
- `inspect_counter_dump_json`: Python-exposed helper that serializes counter-dump records, module/type key layouts, type tables, and rows to JSON.
- `_soac_ext`: PyO3 module initializer; initializes logging and registers all extension functions/types.

## Context Read

- `crates/soac_pyo3/src/jit_runtime.rs`
- `soac_py/src/soac/import_hook.py`
- `crates/soac_jit/src/counter_dump.rs`
