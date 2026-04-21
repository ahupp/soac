# crates/soac_jit/src/jit/direct_abi.rs

## File Responsibilities

Defines the staged ABI descriptor model for direct calls. The current concrete users are runtime builtin primitives (`ord`,
`chr`, and `len`), but the model is deliberately shaped to also describe direct calls to SOAC-compiled Python functions. The
file records hidden arguments, Python/object/scalar parameter ABI, result ABI, error ABI, cost metadata, and lookup from
known builtin names to runtime primitive descriptors.

## Datatypes

- `SOAC_RUNTIME_BUILTIN_ORD_I64_SYMBOL`, `SOAC_RUNTIME_BUILTIN_CHR_I64_SYMBOL`,
  `SOAC_RUNTIME_BUILTIN_LEN_I64_SYMBOL`: exported runtime helper symbol names for builtin primitive implementations.
- `DirectTargetId`: identifies either a regular Python `FunctionId` target or a runtime primitive target.
- `RuntimePrimitiveId`: enumerates currently modeled runtime primitives: `BuiltinOrdI64`, `BuiltinChrI64`, and
  `BuiltinLenI64`.
- `DirectEntry`: describes how a target is entered, either through the process JIT Python-function path or a named runtime
  symbol.
- `HiddenArgAbi`: hidden non-Python arguments required by a direct target; currently function environment and thread state.
- `ArgOwnership`: ownership contract for `PyObject` arguments.
- `ParamAbi`: physical ABI for one explicit argument: `PyObject`, `I64`, or `I32`, including optional PyLong coercion for
  scalar arguments.
- `PyLongI64Coercion`: PyLong-to-i64 conversion policy; currently only `Saturating`.
- `ResultAbi`: physical result ABI, including `PyObject` ownership/facts or scalar results.
- `ErrorAbi`: error-reporting contract; either cannot raise or reports through the current Python exception.
- `DirectCallAbi`: full hidden-arg, parameter, result, and error contract for a direct call.
- `DirectCallCost`: approximate runtime and code-size cost used by future selection logic.
- `DirectCallableDesc`: full descriptor tying target id, entry point, ABI, and cost together.
- `TSTATE_HIDDEN_ARGS`, `ORD_PARAMS`, `LEN_PARAMS`, `CHR_PARAMS`: shared static ABI slices used by primitive descriptors.
- `BUILTIN_ORD_I64_DESC`, `BUILTIN_CHR_I64_DESC`, `BUILTIN_LEN_I64_DESC`: concrete descriptors for supported builtins.

## Functions

- `DirectCallCost::new`: const constructor for cost metadata.
- `runtime_primitive_desc`: maps a `RuntimePrimitiveId` to its static descriptor.
- `runtime_primitive_for_builtin_name`: maps builtin names to supported primitive ids and returns `None` for unsupported
  names.

Test functions validate that each descriptor has the expected symbol, hidden args, parameter ABI, result ABI, and error ABI,
and that builtin-name lookup recognizes the supported builtin names.

## Context Read

- `crates/soac_jit/src/jit/mod.rs`: consumes descriptors when selecting and emitting runtime primitive calls.
- `crates/soac_jit/src/jit/typed_value.rs`: supplies `ValueOwnership`.
- `soac_blockpy::passes::PyExactType`: supplies exact-result type facts.
