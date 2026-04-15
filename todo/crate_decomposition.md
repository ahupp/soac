# Crate Decomposition For Compile Times

## Problem

The current crate graph has too much rebuild blast radius.

`soac-pyo3` depends on `soac-jit`, and `soac-jit` depends on all of `soac-blockpy`.
`soac-blockpy` contains both stable BlockPy data structures and high-churn lowering passes. This
means a pass-only edit can invalidate the JIT crate and then the extension crate, even though the
JIT usually only needs the final IR shape.

`soac-jit` also combines several concerns:

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
- `soac-jit` depends on the whole `soac-blockpy` crate instead of only the IR it consumes.
- `soac-jit/src/jit/mod.rs` is a large monolithic codegen module, which makes crate boundaries hard
  to see.
- `soac-jit/src/jit/test.rs` is large enough that `soac-jit` test builds have a high fixed cost.
- `soac-inspector` pulls together inspection logic and the web stack, so non-web inspection work can
  pay for web dependencies.
- Shared config, logging, build identity, counter schema, and profile formats are currently spread
  across frontend, JIT, inspector, and runtime entrypoints.

## Proposed Crates

### `soac-ir`

Own the stable data model that downstream crates consume.

Likely contents:

- BlockPy module/function/instruction/term types
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

`soac-jit`, `soac-inspector`, profile readers, and offline tools should depend on this crate instead
of depending on all of `soac-blockpy`.

### `soac-lowering`

Own parsing, transformation, name binding, and pass execution.

Likely contents:

- current `soac-blockpy/src/passes`
- current lowering driver
- AST transformer
- template support
- parse-to-BlockPy entrypoints
- lowering fixtures that are not needed by production consumers

This crate depends on `soac-ir` and Ruff. It is expected to be high churn. Keeping it separate means
pass-only edits do not force recompilation of the JIT backend or extension crate.

The existing `soac-blockpy` crate can temporarily become a compatibility facade that re-exports
`soac-ir` and `soac-lowering` APIs while callsites are migrated.

### `soac-config`

Own typed environment parsing and process logging setup.

Likely contents:

- SOAC env-var parsing
- tracing initialization
- shared runtime/test/benchmark config structs
- config validation and defaulting

This should keep `tracing-subscriber` and env parsing out of foundational crates like `soac-ir`.
Entry points should construct config once and pass typed config into consumers rather than reading
env vars at consumption sites.

### `soac-profile`

Own counter and profile formats.

Likely contents:

- counter schemas and ids
- profile/verify dump readers and writers
- module hash references used by counters
- summary helpers used by benchmarks and inspector
- serialization compatibility checks

This crate should depend on `soac-ir` where it needs shared ids, but should not depend on PyO3 or
Cranelift. Inspector and offline precompile tooling should be able to inspect counters without
rebuilding the JIT backend.

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

Do not start with this extraction. First split `soac-jit/src/jit/mod.rs` into internal modules so
the backend boundary is visible inside one crate. Extracting a crate from the current monolith would
mostly preserve the monolith behind a new package name.

### `soac-cpython-runtime`

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

### `soac-inspector-core`

Own non-web inspection and rendering logic.

The existing inspector binary can depend on `soac-inspector-core` plus axum/tokio/tower-http. This
keeps web dependencies out of non-web rendering and offline analysis tools.

## Suggested Migration Order

1. Measure baseline compile behavior before moving code:
   - clean workspace build
   - edit-only lowering pass rebuild
   - edit-only JIT backend rebuild
   - `soac-jit` test build
   - extension build path
2. Remove accidental crate outputs if safe. In particular, verify whether `soac-blockpy` still needs
   to build a `cdylib`; if not, make it an `rlib` only.
3. Add `soac-ir` and move stable BlockPy payload types first.
4. Keep `soac-blockpy` as a facade re-exporting `soac-ir` so the first extraction does not require a
   whole-workspace import rewrite.
5. Change `soac-jit`, `soac-pyo3`, `soac-inspector`, and offline tools to import shared IR from
   `soac-ir` where possible.
6. Add `soac-lowering` and move passes, driver, transformer, and template code there.
7. Move env parsing and logging setup into `soac-config`.
8. Move counter/profile schemas and serialization into `soac-profile`.
9. Split `soac-jit/src/jit/mod.rs` into internal modules along backend responsibility boundaries.
10. Extract `soac-codegen-model` after the shared descriptors are visible and no longer buried in
    backend emission code.
11. Extract `soac-cranelift` from the internal backend modules.
12. Extract `soac-cpython-runtime` only after the PyO3/CPython ownership boundary is clear.
13. Split inspector web code from `soac-inspector-core`.
14. Re-measure the same compile scenarios from step 1 and keep the results with this todo or in a
    dedicated compile-time note.

## Expected Compile-Time Wins

- Editing a lowering pass should rebuild `soac-lowering`, but not `soac-jit` or the extension.
- Editing counter/profile decoding should not rebuild Cranelift backend code.
- Editing inspector web UI/server code should not rebuild inspection core or JIT internals.
- Editing Cranelift emission should not rebuild parsing/lowering or PyO3 extension glue unless the
  shared codegen model changes.
- Rust test builds can become more focused if large JIT-only test helpers move out of production
  modules or into a dedicated dev/test crate.

## Risks And Constraints

- Too many crates can make clean builds worse and create premature public APIs. Split only around
  stable ownership boundaries.
- `soac-ir` must not depend on `soac-lowering`, `soac-jit`, PyO3, or Cranelift.
- Keep high-churn lowering internals out of `soac-ir`; otherwise every optimization pass edit will
  still invalidate downstream crates.
- Avoid putting `tracing-subscriber` or env parsing in foundational model crates.
- Be careful with PyO3 feature unification. PyO3 should stay in CPython boundary crates rather than
  leaking into IR or lowering crates.
- Do not split every pass into its own crate. Adjacent passes share context and tend to change
  together; a pass-per-crate layout would add API friction without reducing the rebuilds that matter.
- Cross-crate inlining can matter for very hot helpers. Keep very-hot ABI-shaped runtime code in
  `soac-runtime` or the backend/runtime boundary, and use explicit benchmarks before moving hot code
  behind a less optimizable crate boundary.

## First Concrete Step

Start with the `soac-ir` extraction.

Definition of done for the first step:

- New workspace crate `soac-ir`.
- Stable BlockPy data structures move from `soac-blockpy` into `soac-ir`.
- `soac-blockpy` re-exports the moved types so existing users keep compiling during migration.
- `soac-jit` imports at least the core BlockPy instruction/module/function types directly from
  `soac-ir`.
- A lowering-pass-only edit no longer forces `soac-jit` to rebuild, verified with a simple
  before/after compile timing check.

