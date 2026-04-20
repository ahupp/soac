# soac-blockpy/src/lib.rs

## File Responsibilities

Public crate facade for SOAC BlockPy lowering. It exposes the main lowering APIs, logging
initialization, the crate error type, AST-to-string helpers, and public/internal modules used by
other crates and tools.

## Datatypes

- `LoweringError`: public error wrapper distinguishing parse errors from other lowering failures.
- `Result<T>`: crate result alias using `LoweringError`.
- `LoweringResult<P>`: lowered codegen module, pass tracker, and total lowering time.
- `ToRuffAst`: trait for values that can be rendered by the Ruff code generator.

## Functions

- `LoweringError` display/source/from impls: integrate parse and anyhow errors with standard
  error handling.
- `open_soac_log_file`: creates parent dirs and opens an append-only tracing JSONL file.
- `init_logging`: parses env config and initializes tracing.
- `init_logging_with_config`: installs either JSON tracing to a configured path or formatted stderr
  tracing.
- `lower_python_to_blockpy_with_tracker`: lowers with default options and a caller-supplied tracker.
- `lower_python_to_blockpy_with_tracker_and_options`: parses env config, initializes logging,
  resets temp-name state, runs the driver, and returns timing/module/tracker data.
- `lower_python_to_blockpy_for_testing`: testing entry point with module id 0 and recording tracker.
- `lower_python_to_blockpy`: production-style entry point with a no-op tracker.
- `lower_python_to_blockpy_recorded`: production-style lowering with recorded pass outputs.
- `lower_python_to_blockpy_recorded_with_options`: recorded lowering with explicit options.
- `ToRuffAst` impls for expressions/statements/slices: normalize values into statement lists for
  rendering.
- `ruff_ast_to_string`: pretty-prints Ruff AST statements with standard indentation and line endings.

## Context Read

- `soac-blockpy/src/driver.rs`
- `soac-blockpy/src/env_config.rs`
- `soac-blockpy/src/pass_tracker.rs`
