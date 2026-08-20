---
title: "Typed Local Values"
---

# Typed Local Values

## Goal

Replace scalar-local special cases with a narrow typed-codegen interface that
resembles an ordinary typed language. Every Cranelift `ir::Value` that crosses a
SOAC helper boundary should carry a SOAC representation, and every consumer
should request the representation it needs explicitly.

The intended split is:

```text
InstrTyped/facts describe what an expression can produce.
Typed codegen emits a SoacValue for each produced ir::Value.
LocalEnv stores typed local bindings, not raw PyObject-only values.
Boundary operations request explicit conversions.
```

This should remove the need for hyper-specific scalar-thread shape matching
while preserving CPython-visible refcount timing.

## Current Shape

`Store()` is a Python IR binding/rooting operation. It is used for source-level
locals and synthetic Python temporaries such as `_dp_tmp_*`, but not for
Cranelift-only SSA values. Cranelift SSA currently appears in codegen as raw
`ir::Value` values, sometimes wrapped in `SoacValue` or `EmitResult`.

The JIT already has a partial typed-value layer:

```rust
enum SoacValue {
    PyObject {
        value: ir::Value,
        ownership: ValueOwnership,
        facts: PyObjFacts,
    },
    I32 {
        value: ir::Value,
        facts: IntFacts,
    },
    I64 {
        value: ir::Value,
        facts: IntFacts,
    },
}
```

`LocalEnvEntry`, however, still stores one raw `ir::Value` plus `LocalRefKind`.
That means local state is effectively modeled as a Python object/root, and
exact-int scalar values are handled by specialized paths instead of ordinary
local representation.

## Target Model

Use one representation-carrying value type across typed codegen:

```rust
enum SoacValue {
    PyObject {
        value: ir::Value,
        ownership: ValueOwnership,
        facts: PyObjFacts,
    },
    I32 {
        value: ir::Value,
        facts: IntFacts,
    },
    I64 {
        value: ir::Value,
        facts: IntFacts,
    },
}
```

Make local bindings representation-aware:

```rust
enum LocalBindingValue {
    Unbound,
    PyObject {
        value: ir::Value,
        ref_kind: LocalRefKind,
        facts: PyObjFacts,
    },
    ExactI64 {
        value: ir::Value,
        facts: IntFacts,
    },
}
```

The important invariant is that all conversions are explicit:

```rust
fn require_pyobject(value: SoacValue, ctx: MaterializeCtx) -> PyObjectValue;
fn require_i64(value: SoacValue, ctx: UnboxCtx) -> I64Value;
fn materialize_i64_as_pylong(value: I64Value) -> PyObjectValue;
```

Consumers state their demand. Integer arithmetic and comparisons can demand
`I64`; return/call/attr/global/cell boundaries demand `PyObject`.

## Semantic Constraints

- Scalar locals do not own Python references and need no DECREF cleanup.
- Overwriting a PyObject local with a scalar still releases the old Python
  binding at the CPython-compatible point.
- Overwriting a scalar local with another scalar emits no refcount work.
- If a scalar local reaches a Python-visible boundary, materialize it once at
  that boundary.
- If a guard miss, raise, deopt, or interpreter resume needs the local in Python
  frame state, materialize before transferring control.
- Cells, globals, attrs, containers, and unknown calls should stay PyObject
  boundaries until a later, more specific representation is designed.
- Identity-sensitive uses must not observe a missing PyLong allocation. The
  safe first target is exact-int values produced by optimized arithmetic or
  comparisons and not used for identity before materialization.

## Implementation Steps

1. Done: collapse `EmitResult` so it carries `SoacValue` instead of duplicating
   the same `PyObject`/`I32`/`I64` variants.

2. Add typed conversion helpers that satisfy `ResultDemand` from `SoacValue`.
   These helpers should be the only place that boxes an exact `I64` into a
   `PyLong` or promotes a borrowed PyObject to an owned boundary value.

   Progress: `emit_soac_value_result_for_demand` now centralizes discarding,
   PyObject materialization/promotion, I32 truthiness, and I64 demand handling
   for several typed-codegen paths. Remaining work is to route the rest of the
   ad hoc materialization sites through it before treating it as the only
   conversion boundary.

3. Refactor helper entry points so typed codegen returns `SoacValue` or
   `Option<SoacValue>` internally, and effect-only statement emission discards
   typed results through one cleanup helper.

4. Replace `LocalEnvEntry.value` plus `LocalRefKind` with
   `LocalBindingValue`. Keep stack-slot mirror state separate from the current
   value representation.

   Progress: `LocalEnvEntry` now stores a `LocalBindingValue` with existing
   PyObject and unbound variants. This is still behavior-preserving; scalar
   bindings are not introduced yet, but LocalEnv no longer exposes raw value,
   ref-kind, and PyObject fact fields as separate storage.

5. Change local `Store(local, rhs)` to store the natural representation of the
   RHS. Exact-int RHS values should bind as `ExactI64`; PyObject RHS values keep
   existing ownership behavior.

   Progress: Local stores whose RHS can satisfy an `I64` demand now bind as
   `ExactI64`. Scalar local loads satisfy `I64` consumers directly and
   materialize to an owned PyObject at Python boundaries or block-arg
   forwarding. CFG joins still materialize through existing PyObject block
   params, so loop-carried scalar locals are not preserved yet.

6. Change local `Load(local)` to return the current local representation. A
   scalar local load should produce `SoacValue::I64` for consumers that can use
   it, and materialize only when a PyObject demand reaches the load.

7. Handle CFG joins conservatively. If all incoming local bindings agree on
   `ExactI64`, keep the target local scalar. If representations differ,
   materialize scalar predecessors and join as `PyObject`.

   Progress: runtime block params and edge transports now carry an explicit
   `PyObject`/`ExactI64` representation. A conservative typed-local
   representation analysis preserves scalar locals across normal CFG edges when
   all incoming sources are scalar-compatible, while exception edges and mixed
   joins stay PyObject-backed.

8. Handle guard/deopt/failure boundaries by materializing only locals required
   by resumed Python/interpreter state. Scalars not needed by the boundary do
   not become cleanup roots.

   Progress: guard-miss deopt live-value buffers now materialize scalar
   `ExactI64` LocalEnv entries into owned `PyLong` values only for locals that
   appear in the planned resume entry. Scalars outside the resume entry remain
   SSA-only.

9. Done: delete the narrow scalar-thread store/branch special case once generic
   typed locals cover the same behavior. The generic path now carries scalar
   locals through runtime block params and pre-seeds matching mechanical i64
   conversion outputs from `LocalEnv` scalar bindings.

10. Add structured tests for scalar store/load, scalar loop-carried locals,
    materialization at return/call/attr/global/cell boundaries, mixed joins, and
    identity-sensitive cases.

## First Acceptance Cases

```python
def count(n):
    i = 0
    while i < n:
        i = i + 1
    return i
```

Expected: `i` stays `ExactI64` through the loop, boxes once at return, and does
not update a cleanup-root PyObject slot every iteration.

```python
def branch_after_add(x):
    y = x + 1
    if y < 100:
        return y
    return 0
```

Expected: `y` stays `ExactI64` for the comparison and materializes only on the
returning edge that returns `y`.

```python
def call_boundary(x):
    y = x + 1
    return str(y)
```

Expected: `y` stays `ExactI64` until the call argument boundary, then boxes once
before the call.

## Measurement

Use focused structured tests first. Then compare pystone with refcount counters,
generated code size, and apply throughput. The expected pystone signal is lower
`runtime_incref`, `runtime_decref`, cleanup-root stack replacement, and Proc0
code size from removing per-iteration PyLong materialization and local-slot
overwrite cleanup.
