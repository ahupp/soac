# soac-inspector/src/bin/render_jit_clif.rs

## File Responsibilities

CLI for rendering JIT CLIF/VCode for a lowered Python function. It supports profile dumps, specialized rendering, exact-slot patching, and optional branch reordering.

## Datatypes

- `Args`: parsed source path, function id, profile dump path, and rendering feature flags.
- `VALIDATE_DELIMITER`: fixture delimiter used to strip validation text from input files.

## Functions

- `parse_args`: parses source path, packed function id, and rendering flags.
- `print_usage`: writes CLI help text.
- `split_source`: reads source and drops validation text after the delimiter.
- `main`: prepares Python support, builds `JitClifRenderOptions`, lowers source, renders CLIF/VCode, and prints it.

## Context Read

- `soac-inspector/src/lib.rs`
- `soac-jit` debug rendering APIs

