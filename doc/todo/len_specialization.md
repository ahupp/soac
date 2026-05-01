---
title: "len Direct Specialization Follow-Up"
---

# `len` Direct Specialization Follow-Up

## Goal

Turn the current checked `len_i64` runtime primitive into a fallback behind more
specific `Length(x) -> I64` lowerings. The natural result should stay scalar:
only box through `I64 -> PyLong` when the consumer demands a Python object.

## Current State

- Static runtime-name `len(x)` is recognized as `BuiltinLenI64`.
- The descriptor accepts one borrowed `PyObject*`, returns `I64`, and uses
  `PyThreadState.current_exception` for error reporting.
- Codegen emits runtime primitive calls through `DirectCallableDesc`, so `len`
  does not need a custom builtin-specific codegen arm.
- `emit_runtime_primitive_result_for_demand` sends `ResultAbi::I64` through
  `emit_i64_result_for_demand`, so `len(x)` can remain an `i64` when the
  consumer demands `I64` or `I64Index`, and boxes only for `PyObject` demand.

## Specialization Rule

Represent static builtin `len(x)` as a typed scalar length operation:

```text
Call(RuntimeName("len"), [x]) -> Length(x) -> I64
```

Then choose the most specific safe lowering:

1. If facts prove `x` is an exact built-in layout type with a stable length
   field, emit a direct length load.
2. If facts prove the type's `sq_length` / `mp_length` slot is the built-in
   implementation and invalidation covers slot changes, emit a direct slot call
   or a small SOAC runtime wrapper around that slot.
3. Otherwise call `soac_runtime_builtin_len_i64`, preserving CPython behavior.

## Initial Direct Lowerings

Start with exact built-in types where CPython-visible behavior is clear:

- exact `str`: load Unicode length
- exact `list`: load `PyVarObject.ob_size`
- exact `tuple`: load `PyVarObject.ob_size`
- exact `bytes`: load `PyVarObject.ob_size`
- exact `dict`: load the dict used-count once the raw layout mirror is already
  trusted in the JIT layer

Do not apply direct layout loads to subclasses, user objects, proxies, or C
extension types unless facts explicitly prove the exact built-in type.

## Validation

- Behavior:
  - `len("abc") == 3`
  - `len([1, 2, 3]) == 3`
  - `len(custom_obj)` still goes through the generic protocol
  - `len(5)` raises `TypeError`
- Demand:
  - `chr(len("x" * 65)) == "A"` exercises `len` as an `I64` producer consumed
    directly by another scalar builtin.
  - `return len(x)` boxes through `I64 -> PyLong` only at the function return.
- Structure:
  - Exact built-in fast paths should not call `PyObject_Size`.
  - Unknown/custom object paths should still call `soac_runtime_builtin_len_i64`.

