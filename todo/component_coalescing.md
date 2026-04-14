# Component Coalescing Opportunities

This note summarizes overlap found while reviewing the generated per-file summaries in `review/`.
The goal is to reduce duplicated concepts across adjacent pipeline layers so the codebase is easier
to understand and safer to change.

## Priority Targets

### Runtime ABI Registry

The same runtime-helper concept is currently described in several places:

- runtime helper facts in `soac-blockpy/src/passes/value_facts.rs`
- direct-call ABI descriptors in `soac-jit/src/jit/direct_abi.rs`
- JIT import specs and symbol registration in `soac-jit/src/jit/mod.rs`
- helper implementations and registration in `soac-jit/src/jit/specialized_helpers.rs`
- inlinable runtime implementations in `soac-runtime/src/lib.rs`

Coalesce this into one shared runtime ABI registry that records symbol name, signature, ownership,
result facts, error convention, implementation origin, inlinability, and optional direct-call
descriptor. Codegen, value facts, import declaration, symbol registration, and docs should consume
that table instead of independently encoding the same helper properties.

Do not merge `soac-runtime` and `specialized_helpers.rs` directly. `soac-runtime` is the inlinable
raw ABI layer; `specialized_helpers.rs` is the CPython/JIT bridge and panic boundary. The shared
piece should be descriptors and generated registration, not one physical helper implementation file.

### Typed Value, Demand, Ownership, And Local Environment Plans

Value facts, ownership effects, local-entry plans, and JIT result-demand handling are parts of one
conceptual model but are split across:

- `soac-blockpy/src/passes/value_facts.rs`
- `soac-blockpy/src/passes/ownership_effects.rs`
- `soac-blockpy/src/passes/local_env_plan.rs`
- `soac-jit/src/jit/typed_value.rs`
- `soac-jit/src/jit/planning.rs`
- demand-aware emission in `soac-jit/src/jit/mod.rs`

The intended direction is a BlockPy-owned typed lowering plan that records representation, demand,
ownership, cleanup obligations, and local environment requirements. JIT codegen should consume that
plan rather than rediscovering or adapting the same information imperatively.

### Runtime Name Registry

Runtime names and singletons are represented in name binding, value facts, Python runtime constants,
module constants, bootstrap constant construction, and JIT runtime-name loads.

Create a shared runtime-name registry with:

- name
- source namespace, such as `__soac__.runtime` or `builtins`
- singleton and value-fact metadata
- whether it may be lowered into a module constant slot
- whether treating it as a constant is a CPython-visible behavior change

This should remove repeated hard-coded lists for `TRUE`, `FALSE`, `NONE`, `EMPTY_TUPLE`,
and builtin aliases.

### Counter Schema

Counter metadata and interpretation are split between IR counter definitions, instrumentation,
runtime storage, binary dump parsing, inspector output, and benchmark scripts.

Coalesce this into a shared counter schema that records:

- counter kind and scope
- storage kind, such as scalar or top-values
- observed-value interpretation
- dump/rendering labels
- specialization consumer

This should make adding new counters less error-prone and reduce ad hoc counter decoding in tools.

### Scope And Name-Binding Plan

Scope concepts are currently staged, which is good, but the representations overlap:

- shallow AST symbol collection
- semantic AST scope state
- BlockPy callable scope info
- storage layouts
- name-binding resolution

Do not collapse the stages into one pass. Instead, introduce an explicit scope/name plan that carries
raw symbol facts, Python binding classification, closure layout, and final storage layout as separate
fields of one model. This keeps staged lowering while reducing translation-only datatypes.

## Lower-Risk Cleanup

### Build Support For Vendored CPython (done)

The build scripts for `soac-jit`, `soac-inspector`, and `soac-pyo3` duplicate vendored `libpython`
discovery and Cargo link directive emission. `build_support/soac_build_identity.rs` already provides
shared build identity support; extend build support with vendored-CPython library discovery and link
emission helpers.

Done: vendored CPython shared-library discovery and Cargo link directive emission now live in shared
build support and are used by all three build scripts.

### Region And CFG Builder

`ruff_to_blockpy` has several related mechanisms for inline fragments, fallthrough labels, region
targets, exception parameters, exception-edge arguments, try/finally regions, and later CFG cleanup.

A shared region/CFG builder could own fallthrough handling, exception edge wiring, and cleanup.
This is worthwhile, but it touches correctness-heavy lowering behavior and should come after the
metadata/registry cleanups above.

## Suggested Order

1. Add the runtime ABI registry and migrate one or two helpers as proof of shape.
2. Add the runtime-name registry and migrate singleton/runtime-name hard-coded lists.
3. Extract shared build-support helpers for vendored CPython linking.
4. Add the counter schema before adding more profiling counters.
5. Move typed value/demand/ownership decisions toward a BlockPy-owned typed lowering plan.
6. Consider region/CFG builder consolidation after typed lowering has a cleaner consumer boundary.
