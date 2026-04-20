# Code Size Analysis

## Problem

Generated CLIF and VCode are larger than they need to be in several hot JIT
lowering paths. That costs us in three places at once:

- more compile-time work in Cranelift
- more machine-code bytes and I-cache pressure at runtime
- more duplicated cold cleanup/error code mixed into otherwise hot regions

The 2026-04-11 `sosmzxqw` benchmark is a useful data point here: it improved
specialized pystone throughput by about `+69.5%` without changing the
specialization set. That means path shape and helper structure matter
independently of which specializations fire.

## Why Cranelift Does Not Fully Fix This

Cranelift at `speed` or `speed_and_size` will do some local CFG cleanup:

- fold obviously empty blocks
- simplify branches
- remove some dead instructions

But it will not do the main structural work we need:

- it will not invent shared cleanup blocks across multiple callsites
- it will not infer that a produced Python object is immediately effect-only
- it will not turn repeated ownership/error scaffolding into a canonical shared
  form
- it will not outline very cold exception-preserving dealloc paths on its own

At `SOAC_CRANELIFT_OPT_LEVEL=none`, even the local CFG cleanup mostly goes away,
so correctness/test runs see the raw lowering shape even more directly.

So the right model is: let Cranelift clean up what it can, but generate much
smaller CLIF in the first place.

## Current Code-Size Pathologies

### 1. Checked-call diamonds around nearly every helper call

Many helpers follow this pattern:

```text
call helper
check for null
branch to fail block
success block continues
```

That is sometimes necessary, but today it is emitted eagerly and repeatedly,
even when multiple adjacent callsites all want the same cleanup and the same
failure successor.

### 2. Repeated error-preserving cleanup sequences

We repeatedly emit shapes like:

```text
take raised error
decref N temporaries
restore raised error
jump step_null_block
```

The details differ only in which values get decref'd, but the control-flow
shape is the same. Right now we duplicate that CFG instead of sharing it.

### 3. Keyword and unpacked-call lowering explodes into micro-steps

The generic keyword/unpack paths are especially expensive:

- build list/dict containers
- fetch `append`/`extend`/`update`
- call them
- decref results
- set items
- branch through cleanup paths on each failure edge

This produces large straight-line regions with many tiny cold diamonds.

### 4. Rare decref/dealloc exceptional logic is inline

The slow path that preserves a pending exception around `_Py_Dealloc` is cold,
but if we inline it into common refcount paths, every caller pays in code size.

### 5. We still materialize results whose only consumer is "discard"

If a call or helper is in effect-only position, producing an owned Python
object and then immediately decrefing it is both extra runtime work and extra
CFG.

### 6. Ownership differences clone otherwise-identical CFG

We often have separate small branches or cleanup vectors only because one input
is borrowed and another is owned. Some of that is semantically necessary, but a
lot of it is representational churn caused by how we currently build cleanup
plans.

## CFG Changes To Make

### 1. Separate nullable-call emission from checked-cleanup emission

We should stop making every helper call "self-checking" by default.

Instead, split the API into two layers:

```rust
emit_nullable_pyobject_call(...)
emit_checked_nullable_pyobject_call(..., cleanup_plan)
```

The first layer only emits the call and returns the nullable result. The second
layer handles the null check and branches to a shared cleanup block.

That lets callers compose multiple adjacent operations without nesting small
diamonds inside other small diamonds.

## 2. Intern shared failure blocks by cleanup shape

Inside a function body, memoize cold fail blocks by:

- whether they must preserve/restore the current exception
- which owned values must be dropped
- which final successor they jump to

Use block params for the dynamic values.

For example, multiple `kwargs` `setitem` failures in one region should be able
to branch to one shared slow block instead of each emitting its own
`set_fail`/restore/decref sequence.

Conceptually:

```text
FailShape {
  preserve_error: true,
  drop: [kwargs_obj, call_args_tuple, maybe_callable],
  successor: step_null_block,
}
```

The exact key type can be internal to codegen; the point is to share by shape,
not by callsite.

### 3. Keep one cold failure edge per straight-line region when the cleanup is the same

When several operations all fail to the same cleanup policy, their cold edge
should converge immediately instead of each callsite spelling out its own slow
path.

We will still need continuation blocks because CLIF blocks end in terminators,
but we can stop duplicating the cold side.

### 4. Push effect-only lowering earlier

This connects directly to `doc/todo/result_demand_lowering.md`.

When the result is not semantically consumed:

- emit the operation
- handle null/error
- discard internally
- return `NoValue`

Do not materialize an owned Python result just so the caller can immediately
decref it. This should remove a large amount of refcount and cleanup scaffolding
from statement-position generic calls and helper-driven container mutation.

### 5. Lower keyword/unpack mutation through fewer high-level building blocks

The current generic lowering expands keyword/unpack behavior into many tiny
steps. We should either:

- use shared JIT helper builders with common failure blocks, or
- move some of the colder generic container-mutation paths into runtime helper
  calls

Promising targets:

- `append` / `extend` / `update` effect-only calls
- `kwargs_setitem_checked`
- `tuple_from_iter_checked`

The principle is not "move everything to runtime helpers." The principle is:
keep the hottest fast paths inline, but do not open-code large generic cold-ish
scaffolding inside every specialized function.

### 6. Outline the exception-preserving dealloc path

The "pending exception + decref goes to zero + `_Py_Dealloc` must run without
losing the exception" path should live in one cold runtime helper, not as an
inline block expansion in common refcount helpers.

This is a code-size win even before it is a performance win.

### 7. Canonicalize cleanup ordering

If cleanup vectors are built in slightly different orders at equivalent sites,
we lose sharing opportunities. Cleanup plans should have a stable order and
stable representation so identical logical slow paths hash to the same shape.

## Concrete Near-Term Targets

The main places worth restructuring first are:

- `emit_object_call_with_tuple_args`
- the `emit_checked_owned_pyobject_call_with_cleanup` family
- `emit_kwargs_setitem_or_cleanup`
- `emit_keyword_call_with_local_env`
- `emit_unpack_call_with_local_env`
- the decref/dealloc slow path in runtime/specialized helpers

These are large enough and central enough that shrinking them should help both
compile time and generated code size.

## What I Would Not Count On

- `speed_and_size` alone is not the main fix
- empty-block coalescing alone is not the main fix
- waiting for Cranelift to discover shared cleanup structure is not realistic

Those are secondary levers after we stop generating structurally duplicated
error/refcount CFG.

## Measurement

We should add code-size summaries to the benchmark artifacts, per function:

- CLIF block count
- CLIF instruction count
- VCode instruction count
- final machine-code byte size

Then diff the top regressions/improvements between revisions.

Success signal:

- fewer blocks/instructions/bytes in the hot pystone functions
- unchanged specialization sets and verify counters
- lower compile time and lower perf share in helper/error/refcount buckets

## Order Of Attack

1. Land result-demand/effect-only lowering for generic call discard paths.
2. Outline the cold exception-preserving dealloc path.
3. Add a shared fail-block emission API keyed by cleanup shape.
4. Rewrite keyword/unpack lowering to use shared fail blocks and fewer
   open-coded helper-call diamonds.
5. Only then measure whether `speed_and_size` buys anything additional.
