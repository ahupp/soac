# SSA Locals With Refcount Ownership

## Goal

Use SSA for semantic Python locals, and make reference ownership explicit in the
SSA environment. Stack slots should become backend/ABI implementation details,
not the source of truth for Python local state.

This should make value-based optimization decisions easier while still allowing
correct CPython-style refcount timing, especially when rebinding a local requires
the old value to be decref'd.

## Core Model

Represent each local binding as the current SSA value plus ownership state:

```rust
enum RefKind {
    Owned,
    Borrowed,
    Immortal,
    Unbound,
}

struct LocalBinding {
    value: Option<Value>,
    ref_kind: RefKind,
}
```

Expression lowering should specify whether an operation produces an owned,
borrowed, or immortal value. Every owned value must have exactly one
lifetime-ending action on every path: decref, return transfer, argument transfer,
store-steal, or exceptional cleanup.

## Assignment

For `x = rhs`, the current binding in the SSA environment is the old value:

```text
new = lower(rhs)
old = env[x]
env[x] = new
DECREF(old)
```

The RHS is fully evaluated before rebinding, and the old binding is decref'd
after the new binding is installed. This preserves the important CPython timing
property where destructors observe the post-store local state.

Self-assignment still works if `load_local(x)` produces an owned reference: the
RHS incref balances the old binding decref.

## Control Flow

Block parameters replace stack-slot locals at control-flow joins.

```text
entry:
  x0 = ...
  if cond goto then(x0) else else(x0)

then(x_in):
  y = f()
  DECREF(x_in)
  goto join(y)

else(x_in):
  goto join(x_in)

join(x1):
  ...
```

Only one edge executes dynamically, so passing an owned SSA value to multiple
successors does not require an incref. Ownership transfers along the edge that is
actually taken. At the merge, the block parameter becomes the current owned
binding.

Loop headers should use block params for loop-carried locals:

```text
loop_header(i, acc):
  ...
  new_acc = ...
  DECREF(acc)
  goto loop_header(next_i, new_acc)
```

Loop exits, `break`, and `continue` are edges with explicit live-out
environments. Values not forwarded to the target environment are decref'd on the
edge.

## Failure And Exception Paths

Potentially failing operations need explicit cleanup continuations. Instead of
loading every possible local from a stack slot during cleanup, each failure edge
should know the current SSA environment and emit cleanup for live owned values.

```text
v = call_may_fail(...)
if v == NULL:
    cleanup(env.live_owned_values())
    return NULL
```

Add a verifier for this representation. It should check that every owned value is
released or transferred exactly once on every normal and exceptional path.

## Stack Slot Role

Do not eliminate stack slots entirely. Keep them for:

- C ABI scratch buffers, such as vectorcall trampoline bound-arg arrays.
- Address-taken values.
- Future frame/debug/deopt materialization.
- Cells and closures, where the Python cell object is the mutable state.
- Backend spills owned by Cranelift.

Normal Python locals should not be stack-slot-backed by default.

## Why This Helps

The current stack-slot model forces broad initialization and ownership churn:
slots are initialized with a deleted sentinel, locals are frequently reloaded,
and block transitions clone owned values through incref/decref sequences.

SSA locals allow:

- No initial sentinel fill for every stack slot.
- Unbound locals represented as `Unbound`, not as a materialized object.
- Loads to be borrowed or owned based on use.
- Rebinding to decref the actual previous SSA binding.
- Cranelift to keep values in registers and handle backend spills itself.
- Refcount elision for immortal constants, sentinels, and moved block values.

## Migration Plan

1. Add an ownership-aware SSA environment for local bindings in codegen.
2. Convert straight-line local load/store first.
3. Add block-param environments for branches and loops.
4. Replace deleted-sentinel stack-slot initialization with `Unbound` local state.
5. Generate cleanup from the SSA environment at returns and failure edges.
6. Add a verifier for owned-value release/transfer on all paths.
7. Add refcount optimizations for immortal constants and move-only forwarding.

The design rule is: stack slots are an implementation detail; Python local state
is the SSA environment plus ownership.
