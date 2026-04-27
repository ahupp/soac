# Runtime Function Inventory

This document tracks the callable runtime helper surface used by SOAC. Keep it
in sync when helpers are added, removed, renamed, or moved between the raw
runtime crate, the JIT specialized-helper layer, and the Python `soac.runtime`
module.

Scope:

- `soac_jit_runtime`: exported `#[unsafe(no_mangle)]` functions in
  `crates/soac_jit_runtime/src/lib.rs`.
- `specialized_helpers.rs`: exported helpers and symbols registered by
  `register_specialized_jit_symbols`.
- `soac.runtime`: top-level Python callables, runtime classes, methods, and
  intentionally re-exported helper callables in `soac_py/src/soac/runtime.py`.
  Some of these names, such as import helpers, are native `_soac_ext`
  callables re-exported by `soac.runtime`.
- Synthetic inter-pass markers: helper-shaped names emitted by one compiler pass
  and recognized by a later pass. These names are not executable runtime APIs and
  must not survive to codegen or Python execution.

This list does not include plain runtime constants such as `TRUE`, `FALSE`,
`NONE`, `EMPTY_TUPLE`, or type/data symbols such as `PyFunction_Type`.

## soac_jit_runtime

Exported C ABI helpers:

```text
soac_runtime_decref_dealloc_preserving_error
soac_runtime_decref
soac_runtime_incref
soac_runtime_decref_applied
soac_runtime_incref_applied
soac_runtime_set_raised_exception
soac_runtime_tuple_new
soac_runtime_tuple_set_item_stolen
soac_runtime_example_known_value_source
soac_runtime_example_offset_known_value
soac_runtime_builtin_ord_i64
soac_runtime_builtin_len_i64
soac_runtime_builtin_chr_i64
soac_runtime_pylong_as_i64
soac_runtime_pylong_as_i64_saturating
soac_runtime_probe_global_indexed
soac_runtime_load_global
soac_runtime_store_global_indexed
soac_runtime_store_global
soac_runtime_probe_field_indexed
soac_runtime_store_field_indexed
```

## specialized_helpers.rs

Direct exported helpers:

```text
soac_runtime_set_runtime_error_static
soac_runtime_load_global_slow
dp_jit_enter_recursive_call
dp_jit_push_handled_exception
dp_jit_pop_handled_exception
dp_jit_record_top_value_sample
dp_jit_load_runtime_obj
dp_jit_pyobject_getattr
dp_jit_pyobject_setattr
dp_jit_pyobject_getitem
dp_jit_pyobject_setitem
dp_jit_pyobject_delitem
dp_jit_constructor_generic_alloc_supported
dp_jit_pytype_generic_alloc
dp_jit_finish_constructor_init
dp_jit_load_global_obj
dp_jit_store_global
dp_jit_del_global
dp_jit_del_global_quietly
dp_jit_del_quietly
dp_jit_pyobject_to_i64
dp_jit_make_cell
dp_jit_raise_unbound_local_error
dp_jit_raise_missing_required_argument
dp_jit_load_cell
dp_jit_store_cell
dp_jit_del_deref
dp_jit_del_deref_quietly
dp_jit_deopt_resume
dp_jit_dict_new
dp_jit_dict_set_item
dp_jit_is_true
dp_jit_raise_i64_overflow
```

Perf-frame toggle helper pairs:

```text
dp_jit_raise_from_exc
dp_jit_raise_from_exc_with_frame
dp_jit_guard_method_type_version
dp_jit_guard_method_type_version_with_frame
dp_jit_py_call_positional_three
dp_jit_py_call_positional_three_with_frame
dp_jit_py_call_object
dp_jit_py_call_object_with_frame
dp_jit_py_vectorcall
dp_jit_py_vectorcall_with_frame
dp_jit_py_call_with_kw
dp_jit_py_call_with_kw_with_frame
dp_jit_get_arg_item
dp_jit_get_arg_item_with_frame
```

Registered CPython-wrapper call targets:

```text
PyObject_RichCompare
PySequence_Contains
PyLong_FromLongLong
PyObject_Not
PyObject_IsTrue
PyNumber_Add
PyNumber_Subtract
PyNumber_Multiply
PyNumber_MatrixMultiply
PyNumber_TrueDivide
PyNumber_FloorDivide
PyNumber_Remainder
PyNumber_Power
PyNumber_Lshift
PyNumber_Rshift
PyNumber_Or
PyNumber_Xor
PyNumber_And
PyNumber_InPlaceAdd
PyNumber_InPlaceSubtract
PyNumber_InPlaceMultiply
PyNumber_InPlaceMatrixMultiply
PyNumber_InPlaceTrueDivide
PyNumber_InPlaceFloorDivide
PyNumber_InPlaceRemainder
PyNumber_InPlacePower
PyNumber_InPlaceLshift
PyNumber_InPlaceRshift
PyNumber_InPlaceOr
PyNumber_InPlaceXor
PyNumber_InPlaceAnd
PyNumber_Positive
PyNumber_Negative
PyNumber_Invert
```

Registered call targets implemented outside `specialized_helpers.rs`:

```text
dp_jit_vectorcall_bind_direct_args
dp_jit_vectorcall_compile_function_env
dp_jit_direct_compile_function_env
```

## Synthetic Inter-Pass Markers

These helper-shaped names are compiler-internal markers. They may appear in
intermediate AST or BlockPy during lowering, but a later pass must replace them
with structured IR or dataflow before runtime execution.

```text
current_exception
```

- `current_exception()` is emitted by exception/with statement lowering to mean
  "the active exception object for this exception edge". Name binding rewrites it
  to the block's explicit exception parameter before codegen. It is not
  `soac.runtime.current_exception` and should not be exposed as a Python helper.

## soac.bootstrap

`soac.bootstrap` is evaluated through normal Python, not the SOAC import
transform. It provides constants and function-instantiation helpers needed while
`soac.runtime` itself is still initializing.

Top-level functions defined by `soac.bootstrap`:

```text
code_with_freevars
_entry_template
```

## soac.runtime

Top-level functions defined or re-exported by `soac.runtime`:

```text
_unsupported_frame_builtin
_index
IterRange
range
tuple_from_iter
constructor_call
__deepcopy__
templatelib_Template
templatelib_Interpolation
bb_trace_enter
_yieldfrom_cell_value
_current_yieldfrom
_is_cancelled_error
_reraise_control_flow
_clear_cell
_mark_closed
_normalize_throw_exc
_current_throw_context
float_from_literal
complex_from_parts
class_lookup_cell
class_lookup_global
_validate_exception_type
exception_matches
exceptiongroup_split
unpack
call_super
call_super_noargs
_match_class_validate_arity
match_class_attr_exists
match_class_attr_value
code_template_gen
code_template_async_gen
annotation_forwardref_value
create_class
exc_info
exc_info_from_exception
_get_awaitable_iter
await_iter
raise_from
_call_exception_class
import_
import_attr
import_star
_lookup_special_method
_has_special_method
_missing_context_protocol_message
contextmanager_enter
contextmanager_get_exit
contextmanager_exit
_ensure_awaitable
asynccontextmanager_aenter
asynccontextmanager_get_aexit
asynccontextmanager_exit
```

Runtime classes and methods:

```text
AsyncGenComplete

ClosureGenerator:
  __init__
  __iter__
  __next__
  send
  throw
  close
  gi_yieldfrom

Coroutine:
  __init__
  __await__
  __iter__
  __next__
  send
  throw
  close
  cr_frame
  cr_running
  cr_code
  cr_await

ClosureAsyncGenerator:
  __init__
  __aiter__
  __anext__
  __getattr__
  gi_yieldfrom
  asend
  athrow
  aclose

AsyncGenSend:
  __init__
  __iter__
  __await__
  __next__
  _step
  send
  throw
  close

_AwaitIterWrapper:
  __init__
  __await__
```

Runtime aliases and re-exports:

```text
next
iter
anext
isinstance
getattr
setattr
delattr
tuple
list
dict
set
slice
type
int
classmethod
ascii
repr
str
format
pow
aiter
code_with_freevars
_entry_template
AssertionError
AttributeError
ImportError
TypeError
ValueError

globals = _unsupported_frame_builtin
locals = _unsupported_frame_builtin
eval = _unsupported_frame_builtin
exec = _unsupported_frame_builtin
```

Runtime helpers imported from `soac.sim`:

```text
_MISSING
_mro_getattr
```
