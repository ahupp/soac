---
title: "Fact-Driven Typed Specializations"
---

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

## Producer Facts And Representation Choices

Representation choice should start from what the producer can prove, not only
from what a later consumer demands.

Examples:

- `ord(x)` semantically produces a Python `int`, but the checked builtin
  implementation naturally produces an `I64` with exact-int/range facts.
- a comparison semantically produces the Python bool result, but the natural
  representation is `I32` with a `0..=1` invariant.
- an exact-int module constant may be available as an `I64`, while still being
  materializable as a `PyLong` object at Python-visible boundaries.

The planner should treat each SSA value as a semantic value with one or more
available physical representations:

```rust
struct ValueState {
    facts: ValueFacts,
    available_reps: SmallVec<[Rep; 2]>,
}

enum Rep {
    PyObjectBorrowed,
    PyObjectOwned,
    I64,
    I32Bool01,
}
```

Operations then choose among legal lowering alternatives. Coercions are explicit
edges in the same graph:

```text
I64 -> PyObjectOwned       via emit_to_python_long
I32Bool01 -> PyObjectOwned via emit_to_python_bool
PyObjectBorrowed -> I64    only with proven exact-int facts or a checked guard
```

This avoids the current failure mode where scalar work only happens when the
consumer already asks for an `I64`. A producer such as `ord` should make its
cheap native representation available immediately, and downstream operations
should use it when their semantic preconditions are satisfied.

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

## Lowering Alternatives And Guards

Each optimizable operation should expose a small set of lowering alternatives.
The planner chooses the cheapest legal alternative, rather than hard-coding a
single demand-driven path.

Sketch:

```rust
struct LoweringAlternative {
    name: &'static str,
    input_reps: SmallVec<[RepRequirement; 2]>,
    output_rep: Rep,
    required_facts: FactsPredicate,
    output_facts: FactsTransform,
    guards: SmallVec<[Guard; 2]>,
    failure: FailureMode,
    cost: Cost,
}

enum GuardKind {
    SemanticCheck,
    SpecializationCheck,
}

enum FailureMode {
    Raise(RuntimeErrorKind),
    FallbackToGeneric,
}
```

There are two distinct guard families:

- **Semantic checks** implement the operation's required Python behavior. A
  failed `ord` unicode/length check raises the same exception as `ord`; it is not
  a speculative miss.
- **Specialization checks** protect an optional fast path. A failed exact-int
  operator guard falls back to the generic Python operator path so reflected
  methods, `NotImplemented`, and custom overloads still behave correctly.

Facts decide whether a guard is needed:

- proven facts make the specialized alternative unconditional;
- dominating runtime checks narrow facts in the dominated region;
- profile/counter facts require a specialization guard and fallback;
- missing facts should make the candidate decline unless the guard/fallback
  behavior is explicit.

Costs should include more than local helper latency:

```text
total =
  expected_hot_count * fast_path_cost
  + expected_miss_count * fallback_cost
  + materialization_cost
  + guard_cost
  + code_size_weight * estimated_bytes
  + compile_cost_weight * estimated_compile_cost
```

For unary `-x`, useful candidates are:

- `I64 -> I64` checked machine negate when the value is already unboxed.
- `PyLong -> I64` via exact-int/fits-i64 guard, then checked machine negate.
- exact `PyLong` slot helper returning a Python object.
- generic `PyNumber_Negative`.

If SOAC intentionally treats optimized integer overflow as `OverflowError`
instead of CPython arbitrary-precision growth, that should be part of the
candidate's explicit semantics. The candidate should not silently pretend it is
the ordinary CPython `int` operation.

For `ord(x)`, a checked primitive can be represented as a semantic-guarded
candidate:

```rust
#[soac_builtin(name = "ord")]
#[requires(arg0.type = PyUnicode, failure = RaiseOrdTypeError)]
#[returns(rep = I64, facts = ExactIntRange(0, 0x10ffff))]
fn builtin_ord_unicode(obj: PyObject) -> i64;
```

If facts already prove `x` is a unicode object of length one, codegen can omit
the checks. If facts are missing, the checked primitive still remains legal
because its checks implement `ord`'s required behavior.

Binary operators use the same model, but specialization guards fail to generic
operator dispatch:

```text
ord(a) + ord(b)
  ord producers make I64 values available
  exact-int I64 add candidate is legal
  output can remain I64 until a Python object boundary

ord(a) + some_random_object
  RHS facts do not satisfy the I64 add candidate
  materialize the LHS I64 as PyLong
  call generic PyNumber_Add

ord(a) + x  with profiled exact-int RHS
  guard x exact int / fits I64
  run checked machine add on hit
  materialize LHS and fall back to PyNumber_Add on miss
```

## Across Function Calls And External Facts

The same representation/cost model should apply across transformed direct calls.
Python-visible entry points keep the ordinary object ABI, but direct call
lowering can expose typed internal variants:

```rust
struct FunctionVariant {
    param_reps: SmallVec<[Rep; 4]>,
    param_required_facts: SmallVec<[FactsPredicate; 4]>,
    return_rep: Rep,
    return_facts: ValueFacts,
    guards_at_entry: SmallVec<[Guard; 2]>,
    cost: Cost,
}
```

The public ABI remains:

```text
(fn_env, tstate, PyObject*...) -> owned PyObject
```

but a direct-call variant may be:

```text
(fn_env, tstate, PyObject unicode, PyObject unicode) -> I64
(fn_env, tstate, I64, I64) -> I64
```

Call lowering should choose the cheapest compatible variant. If caller facts
prove the callee's entry requirements, call it directly. If profile data makes a
variant likely but not guaranteed, emit guards and a fallback to the generic
object ABI. Compile cache keys for direct variants need to include the function
id plus the selected parameter/return representation key and assumption key.

External type information should enter as scoped facts:

- an enforced runtime check such as `type(x) is int` can narrow `x` in dominated
  code;
- `isinstance(x, int)` is not enough to skip Python operator behavior because it
  admits subclasses;
- annotations alone are not facts unless SOAC or user code inserted an actual
  enforcing check;
- profile counters create guarded assumptions, never unconditional facts.

Use CLIF transforms only as cleanup for already-chosen plans: remove redundant
guards, fold known helper calls, and simplify materialization chains. The primary
choice belongs in BlockPy/JIT planning, where the compiler still knows whether a
guard failure means "raise the builtin's required exception" or "fall back to
generic Python operator dispatch."

## Diagnostics

Each planner should be able to report why it emitted or declined a specialization.
Keep diagnostics structured rather than relying on rendered CLIF text.

Useful fields:

- operation kind: `truthiness`, `binary_add`, etc.
- input representations and facts
- selected implementation
- candidate costs
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

8. Add cost-based candidate selection.

   Start with unary operators, because their candidate space is smaller than
   binary operators and they still exercise the important choices: existing
   machine value, guarded `PyLong -> I64`, exact slot helper, or generic Python
   fallback.

9. Add typed direct-call variants.

   Extend the same planner to runtime primitives and transformed Python
   functions, keyed by parameter/return representation and assumption facts.

10. Benchmark and log kept performance changes.

   Measure before/after on the standard benchmark workflow. If a specialization
   is kept, add a succinct entry to `doc/PERF_LOG.md`.

## Suggested First Step

Add the codegen-local typed value module only:

- define `SoacValue`
- define `IntFacts` with `i32_unknown`, `i32_known`, and `i32_bool01`
- add small accessor helpers that assert/check the expected representation
- add unit tests for fact constructors and representation checks

Do not wire it into truthiness yet. This keeps the first code change mechanical
and reviewable, and it gives later truthiness work a concrete target type without
mixing value-model design with behavior changes.
