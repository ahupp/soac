# soac_py/src/soac/sim.py

## File Responsibilities

Python semantic simulation helpers for operations that lowered code calls when it needs CPython-like operator behavior in Python space. It implements binary, in-place, unary, rich-comparison, globals, and async-iteration helpers.

## Datatypes

- `_MISSING`: private sentinel for absent special-method lookup.
- No classes are defined.

## Functions

- `_mro_getattr`: searches a class MRO dictionary for an attribute without invoking descriptor binding.
- `_call_special_method`: binds and calls a special method with the first object and remaining args.
- `_oper`: implements binary operator dispatch, including subclass reverse-method priority, `NotImplemented`, and error construction.
- `_ioper`: implements in-place operator dispatch with fallback to regular binary dispatch.
- Binary operators: `add`, `sub`, `mul`, `matmul`, `truediv`, `floordiv`, `mod`, `pow`, `lshift`, `rshift`, `or_`, `xor`, and `and_`.
- In-place operators: `iadd`, `isub`, `imul`, `imatmul`, `itruediv`, `imod`, `ipow`, `ilshift`, `irshift`, `ior`, `ixor`, `iand`, and `ifloordiv`.
- Unary/truth operators: `pos`, `neg`, `invert`, `not_`, and `truth`.
- `_rich_compare`: implements rich-comparison dispatch with reverse-method handling and identity fallbacks for equality.
- `_rich_compare_error`: raises comparison-specific type errors for unsupported ordering comparisons.
- Rich comparison helpers: `eq`, `ne`, `lt`, `le`, `gt`, and `ge`.
- `globals`: returns the caller's globals through frame inspection.
- `aiter`: returns an async iterator via `__aiter__` and validates the result.

## Context Read

- `soac_py/src/soac/runtime.py`
- CPython operator dispatch behavior mirrored by the helpers.

