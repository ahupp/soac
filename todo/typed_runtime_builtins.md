# Typed Runtime Builtins

## Goal

Allow builtin calls such as `ord(x)` and `chr(x)` to participate in the same
typed lowering, coercion, and direct-call machinery as specialized Python
functions.

The first target is not a separate one-off `ord` fast path. The target is a
general direct-call ABI description that can say:

- this callable accepts borrowed Python object arguments
- this callable returns an `i64`, `i32`, owned `PyObject`, or no value
- this callable reports errors by setting `PyThreadState.current_exception`
- this callable is implemented by a `soac-runtime` symbol that can be imported
  into Cranelift and inlined when small enough

`soac-runtime` should hold the actual checked builtin implementations. The JIT
should hold selection policy, typed demands, coercions, and fallback wiring.

## Design

Introduce a direct callable descriptor:

```rust
struct DirectCallableDesc {
    target: DirectTargetId,
    entry: DirectEntry,
    abi: DirectCallAbi,
    cost: DirectCallCost,
}

enum DirectTargetId {
    PythonFunction(FunctionId),
    RuntimePrimitive(RuntimePrimitiveId),
}

enum RuntimePrimitiveId {
    BuiltinOrdI64,
    BuiltinChrI64,
}

struct DirectCallAbi {
    hidden_args: &'static [HiddenArgAbi],
    params: &'static [ParamAbi],
    result: ResultAbi,
    error: ErrorAbi,
}
```

For checked runtime builtins, errors are internal to the callee. A builtin that
may fail sets `PyThreadState.current_exception` before returning. The caller
checks `current_exception` after the call when the descriptor says
`ErrorAbi::CurrentException`. This avoids sentinel return values for scalar
results, where `-1` may be a valid value.

Do not require range facts in the first version. `ord` can return plain
`ResultAbi::I64`; later passes can add facts such as `0..=0x10ffff` once an
optimization needs them.

## Runtime Primitive Shape

`ord` should be implemented in `soac-runtime` as a checked primitive:

```rust
unsafe extern "C" fn soac_runtime_builtin_ord_i64(
    tstate: *mut RawPyThreadState,
    obj: ObjPtr,
) -> i64 {
    if Py_TYPE(obj) != PyUnicode_Type {
        set_type_error(tstate, ...);
        return 0;
    }
    if raw_unicode_len(obj) != 1 {
        set_type_error(tstate, ...);
        return 0;
    }
    raw_unicode_codepoint_at_0(obj)
}
```

`chr` should accept an `i64` and return an owned `PyObject`:

```rust
unsafe extern "C" fn soac_runtime_builtin_chr_i64(
    tstate: *mut RawPyThreadState,
    value: i64,
) -> ObjPtr {
    if value < 0 || value > 0x10ffff {
        set_value_error(tstate, ...);
        return null_mut();
    }
    raw_or_capi_create_unicode_char(tstate, value)
}
```

The runtime implementation may perform type and length checks internally. Those
checks are not external guards in the first version. Later, an unchecked
preconditioned variant can be added if profiling shows the internal checks are
worth hoisting or eliminating.

## Coercions

The typed lowering planner should treat builtin return materialization as an
explicit coercion edge:

```text
Call(ord, [x]) demanded as I64
  -> ord_i64(x)

Call(ord, [x]) demanded as PyObjectOwned
  -> ord_i64(x)
  -> emit_to_python_long

Call(chr, [Call(ord, [x])]) demanded as PyObjectOwned
  -> ord_i64(x)
  -> chr_i64(result)
```

This requires generic coercion edges:

- `I64 -> PyObjectOwned` via `emit_to_python_long`
- `I32Bool01 -> PyObjectOwned` via `emit_to_python_bool`
- `PyObjectBorrowed -> PyObjectOwned` via `incref`
- `PyObjectOwned -> PyObjectBorrowedOk` with no value conversion, while keeping
  the original owner cleanup obligation

## Implementation Order

Status: steps 1 through 5 have started. The direct ABI descriptor scaffold
exists in `soac-jit`, checked `soac_runtime_builtin_ord_i64` /
`soac_runtime_builtin_chr_i64` entry points exist in `soac-runtime`, and static
runtime-name `ord` calls can emit an `i64`. Static `chr(ord(x))` can consume
that `i64` without materializing an intermediate `PyLong`. Static
`chr(<i64 module constant>)` can also use the scalar `chr_i64` path.

1. Add compiler-visible direct ABI descriptor scaffolding in `soac-jit`.

   This should define `DirectTargetId`, `RuntimePrimitiveId`,
   `DirectCallableDesc`, `DirectCallAbi`, `ParamAbi`, `ResultAbi`, `ErrorAbi`,
   argument ownership, runtime symbol names, and static descriptors for
   `BuiltinOrdI64` and `BuiltinChrI64`. It should not change generated code yet.

2. Add checked `ord_i64` and `chr_i64` implementations to `soac-runtime`.

   Keep the implementation raw and ABI-shaped. Prefer direct Unicode layout
   reads where practical, but keep all validation inside the checked primitive.
   Wire the symbols into the runtime CLIF library and runtime support inliner.
   If the full checked function is too large to inline, split slow error
   formatting into non-inline helpers.

3. Add primitive call emission in `soac-jit`.

   Given a `DirectCallableDesc`, emit child arguments according to `ParamAbi`,
   call the runtime symbol, check `PyThreadState.current_exception` when
   `ErrorAbi::CurrentException`, release owned temporary inputs according to the
   argument ownership contract, and return an `EmitResult`.

   Current first slice: `ord(x)` emits the checked runtime primitive and returns
   an `EmitResult::I64`; `chr(ord(x))` and `chr(<i64 module constant>)` emit
   the checked `chr_i64` primitive. Primitive applicability now consults the
   descriptor parameter ABI, but the actual call emission still matches each
   primitive manually. The next cleanup is to make call emission table-driven
   from `DirectCallableDesc`.

4. Add coercion emission between typed results and demands.

   Start with `I64 -> PyObjectOwned` and `I32Bool01 -> PyObjectOwned`, using the
   existing `emit_to_python_long` and `emit_to_python_bool` helpers. Use this to
   satisfy object-demanded `ord(x)` without changing the natural `ord_i64`
   result.

   Current first slice: `I64 -> PyObjectOwned` is wired for object-demanded
   `ord(x)`. `I32Bool01 -> PyObjectOwned` remains a general coercion follow-up.

5. Wire static builtin call recognition.

   When name binding/runtime-name lowering proves a call target is the builtin
   `ord` or `chr`, consider the runtime primitive descriptor before generic
   vectorcall. Keep the existing vectorcall path as fallback for unsupported
   call shapes, keywords, or missing descriptors.

   Current first slice: codegen recognizes only proven runtime-name constants.
   It intentionally excludes global-name loads so shadowable globals do not get
   treated as builtin primitives at codegen time.

6. Generalize profiled direct-call target identity.

   Replace the assumption that call-target counters contain only `FunctionId`
   values with a packed `DirectTargetId` space. Extend callee identity lookup so
   builtin functions can be reported as runtime primitive targets, not just
   Python functions with SOAC metadata.

7. Incorporate regular specialized Python calls with the same mechanism.

   Add a `DirectCallableDesc` for ordinary SOAC Python functions. Initially it
   should describe the existing direct ABI:

   ```text
   (fn_env, tstate, PyObject* args...) -> owned PyObject
   ```

   Then allow additional typed direct ABIs, such as:

   ```text
   (fn_env, tstate, PyObject* args...) -> i64
   ```

   when a caller demands `I64` and the callee body can satisfy that demand. At
   that point, process-JIT direct entries must be keyed by `(FunctionId,
   DirectAbiKey)`, not just `FunctionId`, and `FunctionEnv` needs either
   ABI-indexed entry pointers or a side table instead of a single
   `direct_code_ptr`.

8. Add cost-based candidate selection.

   Once runtime primitives and regular functions both expose descriptors, make
   call lowering choose among:

   - typed primitive call plus coercion
   - typed Python direct call plus coercion
   - existing object-returning direct call
   - generic vectorcall fallback

   The cost model should account for helper calls, current-exception checks,
   boxing, generic fallback blocks, and expected guard hit rate.

## Validation

- Unit-test descriptor lookup for `ord` and `chr`.
- Unit-test argument ownership and result ABI declarations.
- Add behavior tests for:
  - `ord("A") == 65`
  - `chr(65) == "A"`
  - `ord("AB")` raises the CPython-compatible exception type
  - `ord(1)` raises the CPython-compatible exception type
  - `chr(-1)` and `chr(0x110000)` raise correctly
- Add structural JIT tests once call lowering is wired:
  - `ord(x)` demanded as `I64` does not call generic vectorcall
  - `ord(x)` demanded as `PyObjectOwned` emits `I64 -> PyLong`
  - `chr(ord(x))` does not materialize the intermediate `PyLong`
- Run `just test-all` for each non-doc implementation slice.
