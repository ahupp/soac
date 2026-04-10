# Fact-Driven Typed Specializations

## Goal

Replace the abandoned Python-symbolic specialization experiment with a simpler
Rust-native model:

```text
ValueFacts + typed codegen value -> small specialization planner -> direct
Cranelift emission or existing fallback
```

The optimization surface should be explicit, representation-aware, and easy to
decline. We should not build a restricted Python dialect, symbolic executor, or
helper IR until the direct Rust path proves insufficient.

This plan supersedes the previous Python-based symbolic-specialization plan. That
plan and its helper-source/validator changes were intentionally removed before
landing.

## Core Idea

Codegen should carry a small typed value wrapper instead of treating every
intermediate as a raw `PyObject*` plus side-channel facts.

Sketch:

```rust
enum SoacValue {
    PyObject {
        value: cranelift_codegen::ir::Value,
        facts: PyObjFacts,
    },
    I32 {
        value: cranelift_codegen::ir::Value,
        facts: IntFacts,
    },
    I64 {
        value: cranelift_codegen::ir::Value,
        facts: IntFacts,
    },
}
```

The enum variant is the representation. The facts attached to the variant are
representation-specific:

- `PyObject`: exact type, singleton, known-not-none, ownership/refcount shape, and
  any known runtime object identity.
- `I32`: machine integer facts such as known value or range. A normalized truth
  result is represented as `I32` with facts saying the value is exactly `0` or
  `1`.
- `I64`: unboxed integer facts such as known value, range, nonzero, or overflow
  expectations.

Do not add `SoacValue::Truth`. Truth is an `I32` range invariant, not a separate
runtime representation.

## Integer Facts

Start with a deliberately small integer fact model:

```rust
enum IntWidth {
    I32,
    I64,
}

struct IntFacts {
    width: IntWidth,
    known_value: Option<i128>,
    range: Option<IntRange>,
}

struct IntRange {
    min: i128,
    max: i128,
}
```

Useful constructors:

```rust
impl IntFacts {
    fn i32_unknown() -> Self;
    fn i32_known(value: i32) -> Self;
    fn i32_bool01() -> Self; // range 0..=1

    fn i64_unknown() -> Self;
    fn i64_known(value: i64) -> Self;
}
```

`i32_bool01()` means the continuing path has already normalized any CPython
truthiness sentinel behavior. It does not mean Python `bool` object.

## Truthiness Contract

Truthiness should always return a normalized machine integer:

```rust
fn emit_truthiness(value: SoacValue, ctx: &mut CodegenCtx) -> SoacValue;
```

The returned value must be:

```rust
SoacValue::I32 {
    value,
    facts: IntFacts::i32_bool01(),
}
```

If a path calls CPython `PyObject_IsTrue`, a C `nb_bool` slot, or another helper
that can signal an exception with `-1`, that sentinel is an implementation detail
inside `emit_truthiness`:

```text
raw = call PyObject_IsTrue(obj)
if raw < 0:
    jump exception_path
truth = raw != 0
return I32(truth, bool01)
```

The `-1` state must not escape as a value fact or typed value. Every downstream
consumer of `emit_truthiness` should see only normalized `0` or `1`.

Initial cases:

- `None` singleton -> `I32(0, bool01)`
- `False` singleton -> `I32(0, bool01)`
- `True` singleton -> `I32(1, bool01)`
- exact `bool` object -> compare against `Py_True`, return `I32(bool01)`
- unboxed `I64` -> compare against zero, return `I32(bool01)`
- known/custom `nb_bool` implementation -> emit the direct implementation and
  normalize before returning
- unknown `PyObject` -> call the existing generic truthiness helper or
  `PyObject_IsTrue`, handle exception locally, return `I32(bool01)`

## Python Materialization Conversions

Converting a typed machine value into a Python object should be explicit and
named consistently as `emit_to_python_*`.

Examples:

```rust
fn emit_to_python_bool(value: SoacValue, ctx: &mut CodegenCtx) -> SoacValue;
fn emit_to_python_long(value: SoacValue, ctx: &mut CodegenCtx) -> SoacValue;
```

`emit_to_python_bool` should require an `I32` value whose facts imply `0..=1`.
It returns `PyObject(Py_True/Py_False, PyObjFacts::exact_bool())`.

`emit_to_python_long` should materialize an unboxed integer as a Python `int`
object. This name should be used instead of `emit_box_long`, so the conversion
direction is obvious and consistent with `emit_to_python_bool`.

Going the other direction should also be explicit. Candidate names:

```rust
fn emit_from_python_long(value: SoacValue, ctx: &mut CodegenCtx) -> Option<SoacValue>;
fn emit_try_from_python_long(value: SoacValue, ctx: &mut CodegenCtx) -> Option<SoacValue>;
```

The first version can return `None` when facts are not strong enough to unbox
without changing behavior.

## Consumer Pattern

Branches should consume machine truth directly:

```text
condition = emit_truthiness(input)
branch on condition.I32
```

`bool(x)` should be:

```text
truth = emit_truthiness(x)
result = emit_to_python_bool(truth)
```

Integer arithmetic that remains unboxed should produce `I64` or `I32` values and
only call `emit_to_python_long` at Python-observable materialization boundaries:

- returning from a Python function that expects `PyObject*`
- storing into a Python container or object field
- passing to a generic Python C-API helper
- crossing a deopt/fallback boundary that expects Python objects

This keeps specialization benefits from being immediately erased by boxing.

## Slot And Operator Specialization

Slot lookup should resolve to an implementation shape, not only to a C function
pointer.

Sketch:

```rust
enum SlotImpl {
    CpythonSlot { ptr: cranelift_codegen::ir::Value },
    RuntimeHelper(RuntimeHelperId),
    InlineSoac(InlineImplId),
    UnboxedLong(UnboxedLongImpl),
    GenericFallback(RuntimeHelperId),
}
```

For truthiness, a custom bool slot or unboxed implementation should produce
`I32(bool01)` directly. It should not materialize `PyBool` unless the consumer
requires a Python object.

For binary operators, use one shared planner that owns CPython forward/reverse
slot ordering:

```rust
struct BinaryOpSpec {
    name: &'static str,
    forward_slot: SlotId,
    reverse_slot: SlotId,
    generic_fallback: RuntimeHelperId,
}

fn emit_binary_op(
    spec: BinaryOpSpec,
    lhs: SoacValue,
    rhs: SoacValue,
    ctx: &mut CodegenCtx,
) -> Option<SoacValue>;
```

The planner should:

- inspect `PyObjFacts` / `IntFacts`
- choose exact or unboxed implementations when facts are strong enough
- preserve CPython forward/reverse/subtype ordering for Python-object slot paths
- handle `NotImplemented` only inside paths where it can actually occur
- decline to the existing generic helper when facts are missing

For example, exact/unboxed integer addition can return `SoacValue::I64` when the
consumer can stay unboxed, or can call `emit_to_python_long` when a Python object
is required.

## Diagnostics

Each planner should be able to report why it emitted or declined a specialization.
Keep diagnostics structured rather than relying on rendered CLIF text.

Useful fields:

- operation kind: `truthiness`, `binary_add`, etc.
- input representations and facts
- selected implementation
- guards emitted
- fallback reason, if declined
- materializations inserted, such as `emit_to_python_long`

These diagnostics should feed benchmark artifacts and targeted tests.

## Safety Rules

- CPython-visible behavior is the default correctness bar.
- If a specialization cannot prove its guards and fallback behavior, it should
  decline.
- Sentinel/error states from CPython helpers must be consumed at the call site
  that creates them. Do not expose sentinel states as normal `SoacValue`s.
- Python materialization conversions must be explicit. Do not hide boxing inside
  operator helpers unless the operation semantically requires a Python object.
- Keep representation changes local until there are tests that show a value can
  safely remain unboxed across multiple operations.

## Implementation Steps

1. Add a small codegen-local typed value module.

   Define `SoacValue`, `IntFacts`, constructors such as `IntFacts::i32_bool01`,
   and helpers for extracting the raw Cranelift value with checked
   representation expectations. Do not change generated code yet.

2. Route truthiness through typed values without changing behavior.

   Make the existing truthiness emission return `SoacValue::I32(...,
   IntFacts::i32_bool01())`. The generic CPython/helper path should handle `-1`
   immediately and only return normalized `0` or `1`.

3. Add `emit_to_python_bool`.

   Use it only at object-producing sites such as `bool(x)`. Branch lowering
   should consume the `I32(bool01)` value directly.

4. Add singleton and exact-bool truthiness facts.

   Use existing `PyObjFacts` for `None`, `False`, `True`, and exact `bool`.
   Emit direct constants or pointer comparisons instead of the generic helper.

5. Add unboxed integer truthiness.

   Once an `I64` representation exists, emit `value != 0` and return
   `I32(bool01)`.

6. Add `emit_to_python_long`.

   Materialize unboxed integers at explicit Python object boundaries. Avoid the
   name `emit_box_long`; use the `emit_to_python_*` naming convention.

7. Introduce shared binary operator planning.

   Start with `add`, but make the CPython forward/reverse ordering live in one
   operator-family planner so later `sub`, `mul`, comparisons, and unboxed cases
   reuse the same structure.

8. Benchmark and log kept performance changes.

   Measure before/after on the standard benchmark workflow. If a specialization
   is kept, add a succinct entry to `docs/CODEX_OPT_LOG.md`.

## Suggested First Step

Add the codegen-local typed value module only:

- define `SoacValue`
- define `IntFacts` with `i32_unknown`, `i32_known`, and `i32_bool01`
- add small accessor helpers that assert/check the expected representation
- add unit tests for fact constructors and representation checks

Do not wire it into truthiness yet. This keeps the first code change mechanical
and reviewable, and it gives later truthiness work a concrete target type without
mixing value-model design with behavior changes.
