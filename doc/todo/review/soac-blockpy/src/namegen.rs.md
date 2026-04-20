# soac-blockpy/src/namegen.rs

## File Responsibilities

Thread-local simple fresh-name generator for synthesized Ruff AST names outside the structured
BlockPy module/function name generators.

## Datatypes

- `COUNTER`: thread-local atomic counter used for generated `_dp_*` names.

## Functions

- `fresh_name`: returns `_dp_{prefix}_{id}` using the thread-local counter.
- `reset_namegen_state`: resets the thread-local counter to zero at the start of lowering.

## Context Read

- `soac-blockpy/src/lib.rs`
- `soac-blockpy/src/template/mod.rs`
- `soac-blockpy/src/block_py/cfg.rs`
