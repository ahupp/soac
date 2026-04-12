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
}
```

Later this can grow into typed demands:

```rust
enum ResultDemand {
    EffectOnly,
    PyObject { borrowed_ok: bool },
    TruthValue,
    I64Index,
}
```

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
- Branch tests: eventually use `TruthValue` demand rather than forcing a Python
  object and then re-lowering truthiness.

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

Status: step 1 has the initial codegen-local `ResultDemand`, `ValueOwnership`,
and `EmitResult` wrappers in `soac-jit/src/jit/typed_value.rs`. Step 2 has
started: statement-position JIT emission now requests `EffectOnly` through a
typed result wrapper. LocalEnv-backed `Store` and `Del` producers now honor
`EffectOnly` directly and return `NoValue`; generic `Call` and `CallDirect`
statement producers now execute the call and discard owned results at the call
boundary. Non-local cell/global producers still use the legacy object-producing
path before the wrapper discards the result.

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
6. Add a later BlockPy demand-planning pass keyed by semantic `InstrId`, after
   name binding / simplification / instrumentation and before JIT planning.
7. Extend demand to `TruthValue` and `I64Index` once the value-space/refcount
   representation can carry non-Python values cleanly.

Production paths should be strict about missing semantic instruction ids. Tests
should use builders that assign ids instead of silently defaulting demands.
