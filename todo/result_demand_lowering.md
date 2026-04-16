# Result-Demand Lowering

## Problem

Some BlockPy instructions exist mostly for side effects but still force codegen to
materialize a Python result for uniform expression handling.

Examples:

- `Store` naturally needs to evaluate and store its RHS. When the store result is
  unused, returning the stored value or manufacturing `None` is wasted work.
- `Del` has no natural result. Today it still produces `None` for consistency,
  which adds unnecessary refcount work for statement-position deletes.
- Generic calls always produce an owned result. In expression-statement position
  that result is immediately discarded, so codegen should make the discard
  explicit rather than pretending an enclosing consumer needs a value.

The key distinction is that result demand controls only the value produced for a
consumer. Required side effects, child evaluation, and error/refcount cleanup
still happen.

## Proposed Model

Add an explicit result-demand model, propagated from consumers to producers.

Start narrow:

```rust
enum ResultDemand {
    EffectOnly,
    PyObject { borrowed_ok: bool },
    I32Bool01,
}
```

Later this can grow into typed demands:

```rust
enum ResultDemand {
    EffectOnly,
    PyObject { borrowed_ok: bool },
    I32Bool01,
    I64Index,
    // Later numeric demands: I64ExactLong, unboxed float, etc.
}
```

For Python-object demands, borrowed-vs-owned is part of the demand, not just an
implementation detail of the producer. A consumer that will only inspect or pass
through a value may use `PyObject { borrowed_ok: true }`; a consumer that stores,
returns, or otherwise transfers ownership must request `PyObject { borrowed_ok:
false }`. Producers should satisfy the requested ownership shape directly when
possible, and only insert `INCREF`/`DECREF` at the conversion boundary when a
borrowed result cannot satisfy an owned demand or an owned result is being
discarded.

Codegen should return an explicit result wrapper instead of always returning an
`ir::Value`:

```rust
enum EmitResult {
    NoValue,
    PyObject {
        value: ir::Value,
        ownership: ValueOwnership,
        facts: PyObjFacts,
    },
    I32 { value: ir::Value, facts: I32Facts },
    I64 { value: ir::Value, facts: I64Facts },
}
```

Boundaries should be explicit:

```rust
fn require_pyobject(result: EmitResult, site: &str) -> ir::Value;
fn discard_result(result: EmitResult, fb: &mut FunctionBuilder<'_>, ctx: &JitEmitCtx<'_>);
```

## Demand Propagation

Consumers define what they need from children:

- Expression statement: child demand is `EffectOnly`.
- Return expression: child demand is `PyObject { borrowed_ok: false }`.
- Store RHS: RHS demand is `PyObject { borrowed_ok: false }`.
- Store result: if unused, store demand is `EffectOnly`; if used, store demand is
  whatever the parent needs.
- Delete result: `EffectOnly` emits only delete side effects; `PyObject` demand
  emits the delete and then materializes owned `None`.
- Call args and callable: keep the current borrowed-ok logic, but derive it from
  consumer demand and facts rather than ad hoc local checks over time.
- Borrowed/owned Python-object requirements should be planned with the consumer
  demand, so codegen can avoid manufacturing owned temporaries when a borrowed
  value is sufficient and can materialize ownership exactly at Python-visible
  transfer boundaries.
- Branch tests use `I32Bool01` demand rather than forcing a Python object and
  then re-lowering truthiness.
- Return expressions use `PyObject { borrowed_ok: false }` demand because the
  Python return boundary needs a value that can be returned as an owned object.

## Store Semantics

If BlockPy keeps the convention that `Store` evaluates to the stored value, then
codegen should only preserve/duplicate that value when demanded:

```text
EffectOnly:
  emit RHS as owned PyObject
  store RHS into the target
  return NoValue

PyObject:
  emit RHS as owned PyObject
  store RHS into the target while preserving a returned reference
  return the stored value in the requested ownership shape
```

For local stores, this likely means making `LocalEnv::store_location` report how
it consumed the incoming value, or adding a dedicated store-and-return helper
that handles the required `INCREF` before/after installing the binding.

## Refcount Interaction

Do not insert physical `INCREF`/`DECREF` BlockPy instructions as the first step.
BlockPy/planning should record demand, ownership, and cleanup requirements.
JIT codegen should emit concrete refcount calls where it has final control-flow
and error-cleanup paths.

`EffectOnly` does not mean "skip the operation." For a call expression statement,
codegen still emits the call, handles null/error paths, and decrefs the owned
result before returning `NoValue`.

## Implementation Order

Status: step 1 has `ResultDemand`, `ValueOwnership`, and `EmitResult` wrappers;
`ResultDemand` is now the BlockPy-owned `TypedResultDemand`, while
`ValueOwnership` and `EmitResult` remain backend-shaped JIT values. Step 2 has
started: statement-position JIT emission now requests `EffectOnly` through a
typed result wrapper. LocalEnv-backed `Store` and `Del` producers now honor
`EffectOnly` directly and return `NoValue`; generic `Call` and `CallDirect`
statement producers now execute the call and discard owned results at the call
boundary. Non-local cell/global producers still use the legacy object-producing
path before the wrapper discards the result. Step 6 has moved from a
codegen-local `HashMap<InstrId, ResultDemand>` sidecar to node-local
`TypedInstrExtra::demand` annotations on `InstrTyped`. Return values, local
store RHS expressions, raise exception expressions, generic/direct call inputs,
and operator/intrinsic inputs are annotated before JIT codegen and consumed
directly from the typed instruction. Step 7 has started by annotating branch
tests with `I32Bool01` demand and branch-table indices with `I64Index` demand;
typed term emission consumes those annotations.

1. Add `ResultDemand::{EffectOnly, PyObject { borrowed_ok }}` and an `EmitResult`
   wrapper near the existing `SoacValue`/LocalEnv codegen types.
2. Thread demand through `emit_codegen_expr_with_local_env` first, keeping a
   compatibility helper for old `ir::Value` callers.
3. Convert expression-statement emission to request `EffectOnly`.
4. Convert `Store` and `Del` LocalEnv emission to skip owned `None` materialization
   when demand is `EffectOnly`.
5. Convert generic calls so `EffectOnly` calls still execute and then discard the
   owned result internally. Done for the JIT statement-result boundary; follow-up
   work can push demand deeper into individual call specializations.
6. Add a BlockPy demand-planning pass after typed call/access lowering and
   before JIT codegen. Done for current result demands: the pass writes
   `TypedInstrExtra::demand` on the typed IR node instead of building a
   codegen-local sidecar map. Return values are planned as owned `PyObject`
   demand. Local store RHS values are planned as owned `PyObject` demand for the
   typed LocalEnv store path. Raise exception values are planned as owned
   `PyObject` demand for the typed raise path. Generic and direct call inputs are
   planned as borrowed-ok `PyObject` demand and the generic/direct call emitters
   consume those demands through a shared call-input borrow helper.
   Operator/intrinsic inputs are planned as borrowed-ok `PyObject` demand and
   intrinsic argument emission consumes that same helper path.
7. Extend demand to `I32Bool01` and `I64Index` once the value-space/refcount
   representation can carry non-Python values cleanly. Started with branch-test
   `I32Bool01` demands and branch-table `I64Index` demands.

Production codegen should read demand from `InstrTyped` extras. Missing demand
means only that a producer has not been annotated yet; statement roots default to
`EffectOnly` while strict typed consumers such as return/store/raise should keep
checking for the demand shape they require. Tests should prefer assertions on
the typed instruction extras rather than reconstructing sidecar maps.
