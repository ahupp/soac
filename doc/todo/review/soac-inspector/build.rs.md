# crates/soac_inspector/build.rs

## File Responsibilities

Build script for `soac_inspector`. It discovers the vendored CPython shared-library build directory, emits linker search/library directives, and embeds the SOAC build identity.

## Datatypes

- No Rust datatypes are defined.

## Functions

- `main`: locates `vendor/cpython/build/lib*`, selects the Python shared library name, emits Cargo linker directives, and sets `SOAC_BUILD_IDENTITY`.
- `find_python_shared_lib_name`: scans a directory for a `libpython*.so` file and returns the linker library name without `lib`/`.so`.

## Context Read

- `crates/build_support/src/lib.rs`
- `crates/soac_pyo3/build.rs`

