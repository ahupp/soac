# crates/soac_inspector/src/bin/inspect_counters.rs

## File Responsibilities

CLI for inspecting SOAC counter dump files. It can print module/type key layouts, raw counter rows, and specialization summaries.

## Datatypes

- `Args`: parsed CLI options for dump path and whether to print specializations.

## Functions

- `parse_args`: accepts a counter dump path plus `--specializations`.
- `print_usage`: writes CLI usage text.
- `format_counter_row`: renders one counter row with scope, kind, function, instruction, value, and observed-value fields.
- `format_key_layout_row`: renders one module/type key-layout row.
- `format_type_key_layout_row`: renders one watched type layout row.
- `main`: opens the dump, iterates records, prints module metadata/key layouts/type table/counter rows, and optionally prints specialization summaries.

## Context Read

- `crates/soac_jit/src/counter_dump.rs`
- `.codex/skills/soac-clif-snippet/scripts/profile_snippet_clif.py`

