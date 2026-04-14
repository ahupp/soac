# build_support/soac_build_identity.rs

## File Responsibilities

Computes a stable SOAC build-identity hash from source files that affect runtime, lowering, code generation, and extension ABI behavior. Build scripts embed this identity so caches and generated artifacts can reject stale data.

## Datatypes

- `StableHasher`: small deterministic FNV-1a-style hasher used to avoid platform-dependent `DefaultHasher` output.

## Functions

- `compute_soac_build_identity`: walks the repository, selects identity-relevant files, hashes path names plus bytes, and returns a fixed-width hex identity string.
- `collect_identity_paths`: recursively visits directories while skipping ignored build/cache/vendor paths and accumulates files that should affect the identity.
- `path_is_build_identity_input`: filters paths to Rust, Python, Cargo metadata, and other source/config inputs that should invalidate SOAC build artifacts.
- `StableHasher::new`: initializes the deterministic hash state.
- `StableHasher::update`: folds bytes into the hash.
- `StableHasher::finish`: returns the final hash value.

## Context Read

- `soac-pyo3/build.rs`
- `soac-inspector/build.rs`
- `soac-pyo3/src/jit_runtime.rs`
- `soac-inspector/src/bin/precompile_blockpy.rs`

