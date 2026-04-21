# crates/soac_inspector/src/bin/list_jit_functions.rs

## File Responsibilities

Small CLI that lowers a source file and prints JSONL records for each lowered callable that can be rendered or profiled by inspector tooling.

## Datatypes

- `VALIDATE_DELIMITER`: source-file delimiter used to keep only the executable side of fixture files.
- No structs or enums are defined.

## Functions

- `parse_args`: reads the source path argument.
- `print_usage`: writes CLI usage text.
- `split_source`: reads a file and strips text after the validation delimiter.
- `main`: prepares embedded Python, lowers the source, and prints function metadata records.

## Context Read

- `crates/soac_inspector/src/lib.rs`
- `crates/soac_inspector/src/bin/render_jit_clif.rs`

