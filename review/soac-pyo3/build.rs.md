# soac-pyo3/build.rs

## File Responsibilities

Build script for the `_soac_ext` Python extension. It links against the vendored CPython shared library and embeds the SOAC build identity.

## Datatypes

- No Rust datatypes are defined.

## Functions

- `main`: locates the vendored CPython build library directory, emits Cargo linker directives, and exports `SOAC_BUILD_IDENTITY`.
- `find_python_shared_lib_name`: finds the `libpython*.so` file and converts it into a linker library name.

## Context Read

- `build_support/soac_build_identity.rs`
- `soac-inspector/build.rs`

