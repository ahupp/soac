# Python Operator Resolution

## Current State

SOAC no longer relies on Python-level `soac.runtime.add`, `eq`, `truth`, and
similar helpers for ordinary source operators. Source operators lower to
structured `BinOp` / `UnaryOp` instructions, and JIT/deopt paths dispatch
through CPython C APIs or specialized exact-type helper paths.

The old `soac.sim` Python implementation of operator resolution was removed
from the active runtime surface. The remaining `soac.sim` module contains only
small support helpers still used by `soac.runtime` or lowering:

- `_MISSING`
- `_mro_getattr`
- `aiter`

## Removed Python-Level Operator Helpers

These names used to implement Python operator resolution in Python and are no
longer active runtime helpers:

- shared dispatch helpers: `_call_special_method`, `_oper`, `_ioper`,
  `_rich_compare`, `_rich_compare_error`
- binary operators: `add`, `sub`, `mul`, `matmul`, `truediv`, `floordiv`,
  `mod`, `pow`, `lshift`, `rshift`, `or_`, `xor`, `and_`
- in-place operators: `iadd`, `isub`, `imul`, `imatmul`, `itruediv`, `imod`,
  `ipow`, `ilshift`, `irshift`, `ior`, `ixor`, `iand`, `ifloordiv`
- unary/truth operators: `pos`, `neg`, `invert`, `not_`, `truth`
- rich comparisons: `eq`, `ne`, `lt`, `le`, `gt`, `ge`
- frame-sensitive helper experiment: `globals`

## Follow-Up Direction

If SOAC needs an explicit, readable model for Python operator resolution again,
do not reintroduce these as Python runtime helpers on the hot path. Prefer an
IR-level operator-resolution component that:

- takes typed/value facts and demand as input
- models CPython forward/reverse/in-place lookup order once
- emits structured fallback-free terms when facts prove a case
- falls back to CPython C API dispatch when facts are insufficient
- can share rule descriptions with future unboxed numeric lowering

For debugging or documentation, the old Python implementation is useful as a
compact sketch of CPython-style lookup order, but it should not be treated as
the active execution mechanism.
