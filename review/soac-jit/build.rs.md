# soac-jit/build.rs

## File Responsibilities

Build script for `soac-jit`. It configures PyO3 cfg flags, verifies the vendored CPython runtime layout is compatible with raw
runtime CLIF assumptions, links against the vendored shared `libpython`, compiles `soac-runtime/src/lib.rs` with
rustc-codegen-cranelift, extracts optimized `soac_runtime_*` CLIF files, and writes them into a generated
`SOAC_RUNTIME_CLIF` constant.

## Datatypes

- `RUNTIME_CRATE_NAME`: build-time crate name used when compiling `soac-runtime` to CLIF.
- `BuildOutput`: captured stdout/stderr from the rustc-codegen-cranelift invocation so missing CLIF output can report useful
  diagnostics.

## Functions

- `main`: orchestrates PyO3 config, CPython-layout validation, vendored libpython linking, runtime CLIF compilation, CLIF
  discovery, and generated Rust output.
- `emit_vendored_python_link`: emits Cargo link search, rpath, and dylib directives for `vendor/cpython`.
- `find_python_shared_lib_name`: scans a directory for a `libpython*.so` and returns the link name without `lib`/`.so`.
- `ensure_supported_python_runtime_layout`: rejects non-CPython and CPython builds with free-threading, `Py_REF_DEBUG`, or
  `Py_TRACE_REFS`, because the inlined runtime helpers assume normal CPython object layout.
- `emit_rerun_if_changed`: emits Cargo dependency tracking for `soac-runtime` and all files beneath it.
- `build_runtime_clif`: invokes nightly rustc with the Cranelift backend to emit optimized CLIF for `soac-runtime`.
- `clif_output_dir`: computes rustc-codegen-cranelift's emitted `.clif` directory.
- `find_runtime_clif_files`: reads sorted `.opt.clif` files whose names start with `soac_runtime_`.
- `write_runtime_clif_constant`: writes `soac_runtime_clif.rs` containing the embedded `(symbol, clif)` table.
- `raw_string_literal`: builds a raw Rust string literal with enough `#` delimiters for arbitrary CLIF text.
- `walk_files` / `walk_files_inner`: recursively enumerate files under a directory for Cargo rerun tracking.

## Context Read

- `soac-runtime/src/lib.rs`
- `soac-jit/src/lib.rs`
- `soac-jit/src/jit/mod.rs`

