# Block Param Facts

## Goal

Teach JIT block params enough provenance and binding state that they can carry
Python locals across CFG edges without relying on mirrored stack slots as a
second storage location.

This is adjacent to `ValueFacts`, but it should not be modeled as only an
extension of expression value facts. The key missing information is not "what
object is this?" but "what semantics still apply to this incoming block param?"

## Why `ValueFacts` Alone Is Not Enough

`ValueFacts` answers questions like:

- exact type
- truthiness
- `None` / bool singleton identity
- immortality
- numeric range

The block-param problem needs different facts:

- is this incoming local definitely bound on every predecessor edge?
- was null / deleted / unbound already checked before forwarding?
- is this param owned, borrowed, immortal, or still semantically unbound?
- is this a forwarded local binding, a stack-backed load, an exception payload,
  or a constant?

Those are transport facts for CFG edges, not value facts for expressions.

## Proposed Shape

```rust
pub struct BlockParamFacts {
    pub value: Option<PyObjFacts>,
    pub binding: ParamBindingFacts,
    pub provenance: ParamProvenance,
    pub ownership: LocalRefKind,
}

pub enum ParamBindingFacts {
    DefinitelyBound,
    MaybeUnbound,
    CheckedLocalValue,
}

pub enum ParamProvenance {
    ForwardedLocal(LocalLocation),
    StackSlot(LocalLocation),
    ExceptionValue,
    Constant,
    Unknown,
}
```

The exact names can change. The important part is the separation:

- `PyObjFacts` describes the value itself.
- `BlockParamFacts` describes how safe it is to consume the incoming param as a
  Python local.

## Intended Semantics

The JIT consumer should be able to decide:

- `DefinitelyBound + CheckedLocalValue`
  A raw fast-path use is allowed.
- `MaybeUnbound`
  The consumer must preserve local-load semantics and raise
  `UnboundLocalError` / deleted-name failure instead of treating the value as a
  plain `PyObject*`.
- borrowed versus owned still controls incref/decref policy independently.

This keeps the check at the consumption boundary instead of forcing the edge
forwarder to erase maybe-unbound state into a bare pointer-valued block arg.

## Where This Lives

This should be produced by JIT planning, alongside `FunctionLocalPlan`, not by
the general expression `value_facts` pass.

Suggested boundary:

```text
value_facts
-> ownership_effects
-> local liveness / must-bound analysis
-> block-param planning
   - choose runtime block params
   - attach BlockParamFacts
-> JIT codegen consumes BlockParamFacts
```

`compute_function_local_must_bound_ins(...)` is already the beginning of this
story. `BlockParamFacts` would generalize that from a yes/no local property into
per-edge / per-param consumption semantics.

## What This Enables

Once the consumer can trust `BlockParamFacts`, we can remove stack-slot-local
mirroring for normal Python locals:

1. carry all live-in locals as explicit runtime block params
2. consume them using `BlockParamFacts`
3. remove stack-slot fallbacks for normal local loads/stores
4. keep stack slots only for backend/runtime bookkeeping, not as a second local
   store

This is the missing piece for deleting `LocalEnvStorage::StackMirror` and
removing duplicated local state.

## Migration Plan

1. Extend `BlockLocalPlan` or a sibling structure with per-param facts.
2. Thread those facts into `planned_jit_param_names_for_block(...)` consumers.
3. Make `LocalEnv::load_location` / `load_name` consult the new param facts
   instead of inferring semantics from `StackMirror` versus `LocalOnly`.
4. Convert edge-preparation helpers to preserve provenance rather than forcing
   checked loads there.
5. Remove local stack-slot mirroring once normal local loads/stores no longer
   need it.

## Non-Goals

- Do not fold transport semantics into general `ValueFacts`.
- Do not keep both block-param facts and stack-slot mirroring long-term as equal
  sources of truth for Python locals.
