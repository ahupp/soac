---
title: "Crate Decomposition For Compile Times"
---

# Crate Decomposition For Compile Times

## Problem

The current crate graph has too much rebuild blast radius.

`soac_pyo3` depends on `soac_jit`, and `soac_jit` depends on all of `soac-blockpy`.
`soac-blockpy` contains both stable BlockPy data structures and high-churn lowering passes. This
means a pass-only edit can invalidate the JIT crate and then the extension crate, even though the
JIT usually only needs the final IR shape.

`soac_jit` also combines several concerns:

- Cranelift codegen and object/shared-library emission
- CPython/PyO3 runtime glue
- module constants and materialization
- counters and profile dump handling
- direct-call/function metadata
- runtime helper registration

The goal is not to create many tiny crates. The goal is to split stable shared models from
high-churn implementation code so common edits rebuild less of the workspace.

## Current Pressure Points

- `soac-blockpy` mixes IR, lowering, passes, env config, logging setup, cache code, fixtures, and
  renderer/test support.
- `soac_jit` depends on the whole `soac-blockpy` crate instead of only the IR it consumes.
- `crates/soac_jit/src/jit/mod.rs` is a large monolithic blockpy module, which makes crate boundaries hard
  to see.
- `crates/soac_jit/src/jit/test.rs` is large enough that `soac_jit` test builds have a high fixed cost.
- `soac_inspector` pulls together inspection logic and the web stack, so non-web inspection work can
  pay for web dependencies.
- Shared config, logging, build identity, counter schema, and profile formats are currently spread
  across frontend, JIT, inspector, and runtime entrypoints.

## Done

### `soac_core`

Own the first stable BlockPy model slice from the `soac-ir` proposal.

Completed: added the `soac_core` workspace crate and moved context-free BlockPy model,
instruction-operation, visitor, and mapping infrastructure there. `soac-blockpy` remains the
lowering facade and re-exports these core types while keeping concrete pass instruction enums, CFG
rewrites, validation, pretty-printing, and pass-specific lowering code local to `soac-blockpy`.

Contents:

- `BlockPyModule`, `BlockPyFunction`, blocks, terms, edges, and block params
- module/function/runtime ids and name generators
- source metadata and semantic instruction ids
- core instruction trait plus generic operation payloads and operation macros
- generic instruction visitor and mapping traits/helpers
- parameter specs
- scope/storage-layout data
- counter site definitions

### `soac_config`

Own typed environment parsing and process logging setup.

Completed: extracted into the `soac_config` workspace crate. Direct runtime and entrypoint
consumers now import config and logging from `soac_config`; no `soac-blockpy` compatibility
re-export remains. BlockPy cache metadata/path helpers now live with the cache format in
`soac_driver::blockpy_cache`.

Contents:

- SOAC env-var parsing
- tracing initialization
- shared runtime/test/benchmark config structs
- config validation and defaulting

### `soac_cpython`

Own shared embedded CPython initialization.

Completed: extracted the duplicated vendored-CPython setup used by Rust tests and inspector tools
into the `soac_cpython` workspace crate. Rust embedded-Python paths now initialize through
CPython pre-init path configuration instead of mutating process-global `PYTHONHOME`/`PYTHONPATH`.
Test initialization also centralizes `_soac_ext` staging and `sys.path` repair.

This is intentionally narrower than the proposed `soac_cpython-runtime` crate below: it owns
interpreter startup and path setup, not SOAC module state, callback ownership, or runtime helper
registration.

Contents:

- vendored CPython path discovery
- CPython pre-init interpreter path configuration
- Rust test `_soac_ext` staging
- shared `sys.path` insertion helpers

### `soac_lowering`

Own the remaining BlockPy lowering implementation behind the temporary `soac-blockpy` facade.

Completed: added the `soac_lowering` workspace crate and moved the remaining BlockPy lowering
implementation there. `soac-blockpy` now has a thin library facade that re-exports
`soac_lowering`, while its existing developer binaries remain in the package and call the facade.

Contents:

- remaining BlockPy facade payloads and pretty/validation helpers
- pure parse-to-`BlockPyModule<BlockPyModuleShape>` entrypoints
- pass tracker
- transformation passes
- template and transformer support
- lowering fixtures and tests

### `soac_driver`

Own high-level coordination across the frontend, cache artifacts, prepared codegen plans, and
profile/counter instrumentation.

Completed: added the `soac_driver` workspace crate and moved pre-optimization module-cache
handling, prepared-codegen cache serialization, env/logging/timing orchestration, and profile/trace
counter insertion out of `soac_lowering`. The lowering crate now exposes the pure source-to-codegen
module pipeline; production/runtime entrypoints that need cache or counter policy call through
`soac_driver`.

Contents:

- cache artifact path helpers and pre-optimization module cache format
- high-level lowering result orchestration
- prepared codegen plan calculation and cache store/load
- profile, verify, apply, and trace counter insertion policy
- runtime/import-hook lowering entrypoints

### `soac_core::profile`

Own counter dump/profile serialization formats that are shared by runtime, inspector, extension,
and offline tools.

Completed: moved the binary counter dump format, reader/writer, mmap views, type-key layout
collection, block-entry count reader, and specialization summary rendering out of `soac_jit`.
These APIs now live in `soac_core::profile`, so consumers can share profile data structures without
depending on the JIT backend or a separate profile crate. `soac_jit` still owns live runtime counter
collection and optimization-plan construction.

Contents:

- counter dump row/record/type-key schemas
- profile/verify dump readers and writers
- module hash references carried by counter dump records
- summary helpers used by inspector and offline tools
- serialization compatibility checks

## Proposed Crates

### `soac-ir`

Own the remaining stable data model that downstream crates consume. The current concrete crate name
for the first slice is `soac_core`; this proposal still covers the follow-up model moves that have
not happened yet.

Likely contents:

- remaining pass-specific instruction payloads
- `FunctionId` and other stable ids
- storage/location payloads
- typed instruction payloads
- value facts and ownership facts
- module/function shape metadata
- small shared runtime-name and counter-key payload types

This crate should avoid PyO3, Cranelift, Ruff parser/codegen, `tracing-subscriber`, and process
runtime state. A first extraction may still depend on small Ruff types such as source ranges or
names; if that keeps too much Ruff surface in the core model, follow up by introducing SOAC-owned
source range and name wrappers.

`soac_jit`, `soac_inspector`, profile readers, and offline tools should depend on this crate instead
of depending on all of `soac-blockpy`.

### `soac_lowering`

Own parsing, transformation, name binding, and pass execution.

Likely contents:

- current `soac-blockpy/src/passes`
- pure parse-to-BlockPy lowering pipeline
- AST transformer
- template support
- parse-to-BlockPy entrypoints
- lowering fixtures that are not needed by production consumers

This crate depends on `soac-ir` and Ruff. It is expected to be high churn. Keeping it separate means
pass-only edits do not force recompilation of the JIT backend or extension crate.

The existing `soac-blockpy` crate can temporarily become a compatibility facade that re-exports
`soac-ir` and `soac_lowering` APIs while callsites are migrated.

### `soac-codegen-model`

Own codegen-facing descriptions that are not Cranelift-specific.

Likely contents:

- helper ABI descriptors
- direct-call ABI descriptors
- module constant descriptors
- runtime object ids
- function environment layouts
- precompiled code validation metadata

This crate should describe what codegen needs without deciding how Cranelift emits it.

### `soac-cranelift`

Own the Cranelift backend.

Likely contents:

- CLIF emission
- helper signature declaration
- Cranelift function/module setup
- object and shared-library emission
- unwind/DWARF/jitdump support
- backend-specific optimization knobs

Do not start with this extraction. First split `crates/soac_jit/src/jit/mod.rs` into internal modules so
the backend boundary is visible inside one crate. Extracting a crate from the current monolith would
mostly preserve the monolith behind a new package name.

### `soac_cpython-runtime`

Own CPython/PyO3-facing runtime integration.

Likely contents:

- `SharedModuleState`
- function metadata attachment
- module type / loader integration
- import helpers
- Python object materialization
- CPython callback boundaries and panic handling
- runtime helper registration that must interact with live CPython state

This should be the main crate that knows about PyO3 and CPython object lifetime boundaries. It can
depend on `soac-cranelift` for compiled code handles and on `soac-codegen-model` for ABI metadata.

### `soac_inspector-core`

Own non-web inspection and rendering logic.

The existing inspector binary can depend on `soac_inspector-core` plus axum/tokio/tower-http. This
keeps web dependencies out of non-web rendering and offline analysis tools.

## Suggested Migration Order

1. Measure baseline compile behavior before moving code:
   - clean workspace build
   - edit-only lowering pass rebuild
   - edit-only JIT backend rebuild
   - `soac_jit` test build
   - extension build path
2. Done: removed the accidental `cdylib` output from `soac-blockpy`; it now builds as the default
   Rust library only. The Python extension `cdylib` remains owned by `soac_pyo3`.
3. Done, partial: add `soac_core` and move the first stable BlockPy model slice.
4. Done, partial: keep `soac-blockpy` as a facade re-exporting `soac_core` so the first extraction
   does not require a whole-workspace import rewrite.
5. Done, partial: changed `soac_jit`, `soac_pyo3`, `soac_inspector`, and offline tools to import
   shared stable IR from `soac_core` where possible. They still depend on `soac_lowering` for
   concrete codegen/resolved instruction enums, pass module shapes, BlockPy-specific literal
   payloads, and lowering/cache APIs.
6. Done, partial: added `soac_lowering` and moved the remaining BlockPy lowering implementation
   there. `soac-blockpy` is now a compatibility facade at the library layer; internal downstream
   crates import lowering/codegen-cache APIs from `soac_lowering` directly.
7. Done: move env parsing and logging setup into `soac_config`.
8. Done, partial: moved counter dump schemas, serialization, readers, and summary helpers into
   `soac_core::profile`. Live runtime counter collection remains in `soac_jit`, and optimization-plan
   evidence construction remains there until that boundary is split from backend emission.
9. Split `crates/soac_jit/src/jit/mod.rs` into internal modules along backend responsibility boundaries.
10. Extract `soac-codegen-model` after the shared descriptors are visible and no longer buried in
    backend emission code.
11. Extract `soac-cranelift` from the internal backend modules.
12. Done: extract shared embedded-CPython initialization into `soac_cpython`.
13. Extract `soac_cpython-runtime` only after the PyO3/CPython ownership boundary is clear.
14. Split inspector web code from `soac_inspector-core`.
15. Re-measure the same compile scenarios from step 1 and keep the results with this todo or in a
    dedicated compile-time note.

## Expected Compile-Time Wins

- Editing a lowering pass should rebuild `soac_lowering`, but not `soac_jit` or the extension.
- Editing counter/profile decoding should not rebuild Cranelift backend code.
- Editing inspector web UI/server code should not rebuild inspection core or JIT internals.
- Editing Cranelift emission should not rebuild parsing/lowering or PyO3 extension glue unless the
  shared codegen model changes.
- Rust test builds can become more focused if large JIT-only test helpers move out of production
  modules or into a dedicated dev/test crate.

## Risks And Constraints

- Too many crates can make clean builds worse and create premature public APIs. Split only around
  stable ownership boundaries.
- `soac-ir` must not depend on `soac_lowering`, `soac_jit`, PyO3, or Cranelift.
- Keep high-churn lowering internals out of `soac-ir`; otherwise every optimization pass edit will
  still invalidate downstream crates.
- Avoid putting `tracing-subscriber` or env parsing in foundational model crates.
- Be careful with PyO3 feature unification. PyO3 should stay in CPython boundary crates rather than
  leaking into IR or lowering crates.
- Do not split every pass into its own crate. Adjacent passes share context and tend to change
  together; a pass-per-crate layout would add API friction without reducing the rebuilds that matter.
- Cross-crate inlining can matter for very hot helpers. Keep very-hot ABI-shaped runtime code in
  `soac_jit_runtime` or the backend/runtime boundary, and use explicit benchmarks before moving hot code
  behind a less optimizable crate boundary.

## First Concrete Step

Start with the remaining `soac_core`/`soac-ir` extraction.

Definition of done for the first step:

- New workspace crate `soac_core`.
- Stable BlockPy module/function/block, metadata, parameter, scope/storage, name/id, and counter
  site data move from `soac-blockpy` into `soac_core`.
- `soac-blockpy` re-exports the moved types so existing users keep compiling during migration.
- `soac_jit` imports at least the core BlockPy instruction/module/function types directly from
  `soac_core`.
- A lowering-pass-only edit no longer forces `soac_jit` to rebuild, verified with a simple
  before/after compile timing check.
