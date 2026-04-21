# crates/soac_jit/src/jit/typed_value.rs

## File Responsibilities

Provides the small typed-value model used by demand-aware codegen. It represents Cranelift values with SOAC-level
representations, value facts, result demands, and Python ownership so codegen can avoid unnecessary boxing, truthiness
conversion, or refcount work.

## Datatypes

- `IntWidth`: scalar integer width tracked by SOAC facts: `I32` or `I64`.
- `IntRange`: inclusive integer range used for simple scalar fact propagation.
- `IntFacts`: known width, optional exact value, and optional range for an integer value.
- `SoacRepr`: tag-only representation kind for a `SoacValue`.
- `SoacValue`: typed Cranelift value plus facts: `PyObject`, `I32`, or `I64`.
- `ResultDemand`: requested output form for an expression: effect only, PyObject, normalized truth i32, i64 value, or i64
  index.
- `ValueOwnership`: ownership state for a PyObject result: owned, borrowed, or immortal.
- `EmitResult`: emitted result plus ownership/facts, or no result for effect-only demand.

## Functions

- `IntRange::exact`, `is_within`, `checked_add`, `checked_sub`, `checked_mul`: construct and combine range facts with
  overflow-aware arithmetic.
- `IntFacts::{i32_unknown,i32_known,i32_bool01,i64_unknown,i64_known,i64_range}`: constructors for common integer fact
  states.
- `IntFacts::is_i32_bool01`: validates normalized truth values.
- `SoacValue::{pyobject,i32,i64}`: constructors that enforce representation/fact consistency.
- `SoacValue::{repr,raw_value,as_pyobject,as_i32,as_i64}`: non-panicking accessors.
- `SoacValue::{expect_pyobject,expect_i32,expect_i64,expect_i32_bool01}`: checked accessors for code paths that require a
  specific representation.
- `ResultDemand::{needs_value,borrowed_ok}`: demand queries for whether codegen must produce a value and whether a borrowed
  PyObject can satisfy the demand.
- `ValueOwnership::{is_owned,can_satisfy_pyobject_demand}`: ownership queries used before returning or forwarding PyObjects.
- `EmitResult::{no_value,pyobject,owned_pyobject,borrowed_pyobject,immortal_pyobject,i32,i64}`: constructors for emitted
  results.
- `EmitResult::{has_value,as_pyobject,as_i32,as_i64}` and related checked accessors: inspect emitted results in callers.

## Context Read

- `crates/soac_jit/src/jit/mod.rs`: main consumer for typed expression and demand-aware lowering.
- `crates/soac_jit/src/jit/direct_abi.rs`: uses `ValueOwnership` in direct-call result ABI.
- `soac_blockpy::passes::PyObjFacts`: Python object facts stored alongside PyObject typed values.
