# soac-jit/src/jit/specialized_helpers.rs

## File Responsibilities

Provides Rust/CPython helper functions registered as symbols for generated JIT code. These helpers cover CPython calls,
global/name/cell operations, object/item/attribute access, deopt entry, exact-long specialization, counters, constructor
helpers, exception helpers, and perf-attribution wrapper variants. The file is intentionally ABI-shaped and mostly raw FFI.

## Datatypes

- CPython extern declarations: imported CPython globals/functions used by helper implementations.
- `ObjPtr`: raw `*mut c_void` object pointer ABI used by generated code.
- `SoacPyLongValue`, `SoacPyLongObject`, `PY_LONG_SIGN_MASK`, `PY_LONG_NON_SIZE_BITS`: partial CPython long layout mirror
  used for compact exact-list index fast paths.
- Test-only panic export macros/stubs: define exported helper symbols that panic when mistakenly executed in unit tests.
- `preserve_helper_frame`: macro that prevents tail-collapse for perf-visible helper frame variants.
- `define_perf_toggle_export`: macro that emits both fast and `_with_frame` exported helper symbols.
- CPython wrapper macros (`define_unary_obj_wrapper`, `define_unary_i32_wrapper`, `define_binary_obj_wrapper`,
  `define_binary_i32_wrapper`, `define_ternary_obj_wrapper`, `define_i64_obj_wrapper`): create dlsym-backed wrappers used
  as JIT symbols for CPython APIs.

## Functions

- Basic validation/error helpers: `is_cell_object`, `object_type_name`, `soac_runtime_set_runtime_error_static`,
  `raise_expected_cell`, `exact_long_type_mismatch_error`, `exact_long_missing_slot_error`,
  `exact_long_i64_overflow_error`, and `dp_jit_raise_i64_overflow`.
- Call helpers: `py_call_positional_three_hook`, `dp_jit_py_call_positional_three`,
  `py_call_object_hook`, `dp_jit_py_call_object`, `py_vectorcall_hook`, `dp_jit_py_vectorcall`,
  `py_call_with_kw_hook`, `dp_jit_py_call_with_kw`, `get_arg_item_hook`, and `dp_jit_get_arg_item` bridge generated call
  shapes to CPython call APIs.
- Iterator/recursion/constructor helpers: `next_or_sentinel_hook`, `dp_jit_next_or_sentinel`,
  `enter_recursive_call_hook`, `dp_jit_enter_recursive_call`, `pytype_generic_alloc_hook`,
  `dp_jit_pytype_generic_alloc`, `finish_constructor_init_hook`, and `dp_jit_finish_constructor_init`.
- Global and runtime-name helpers: `load_global_obj_impl`, `ensure_global_load_error`, `guarded_indexed_global_slot`,
  `globals_builtins_owned`, `load_global_slow`, `load_global_obj_hook`, `dp_jit_load_global_obj`,
  `soac_runtime_load_global_slow`, `store_global_hook`, `dp_jit_store_global`, `del_quietly_hook`,
  `del_global_hook`, `dp_jit_del_global`, `dp_jit_del_global_quietly`, `dp_jit_del_quietly`, and
  `dp_jit_load_runtime_obj`.
- Attribute/item helpers: `pyobject_getattr_hook`, `dp_jit_pyobject_getattr`, `pyobject_setattr_hook`,
  `dp_jit_pyobject_setattr`, `exact_compact_long_value`, `exact_list_index`, `new_none`, `pyobject_getitem_hook`,
  `dp_jit_pyobject_getitem`, `pyobject_setitem_hook`, `dp_jit_pyobject_setitem`, `pyobject_delitem_hook`, and
  `dp_jit_pyobject_delitem`.
- Cell helpers: `make_cell_hook`, `dp_jit_make_cell`, `load_cell_hook`, `dp_jit_load_cell`, `store_cell_hook`,
  `dp_jit_store_cell`, `del_deref_hook`, `dp_jit_del_deref`, `del_deref_quietly_hook`, and
  `dp_jit_del_deref_quietly`.
- Exception helpers: `raise_deleted_name_error_hook`, `dp_jit_raise_deleted_name_error`,
  `raise_missing_required_argument_hook`, `dp_jit_raise_missing_required_argument`, `raise_from_exc_hook`,
  `dp_jit_raise_from_exc`, `attach_implicit_exception_context`, `push_handled_exception_hook`,
  `dp_jit_push_handled_exception`, `pop_handled_exception_hook`, and `dp_jit_pop_handled_exception`.
- Counter/deopt/dict/truth helpers: `record_top_value_sample_hook`, `dp_jit_record_top_value_sample`,
  `dp_jit_deopt_resume`, `run_deopt_resume`, `set_deopt_unsupported_continuation_error`, `dict_new_hook`,
  `dp_jit_dict_new`, `dict_set_item_hook`, `dp_jit_dict_set_item`, `is_true_hook`, and `dp_jit_is_true`.
- Numeric helpers: `pyobject_to_i64_hook`, `dp_jit_pyobject_to_i64`, `pyobject_richcompare_wrapper`,
  `load_python_capi_symbol`, generated CPython wrapper functions, `exact_long_binary_op_hook`,
  `dp_jit_exact_long_binary_op`, `exact_long_unary_op_hook`, `dp_jit_exact_long_unary_op`,
  `exact_long_number_slot_symbol`, `exact_long_richcompare_slot_symbol`, `exact_long_number_slot_call`,
  `dp_jit_exact_long_add_slot`, `dp_jit_exact_long_sub_slot`, `dp_jit_exact_long_mul_slot`,
  `dp_jit_exact_long_true_div_slot`, and `dp_jit_exact_long_richcompare_slot`.
- Symbol registration: `chosen_helper_symbol`, `register_exact_long_slot_symbols`, and
  `register_specialized_jit_symbols` publish helper and CPython wrapper addresses to the Cranelift `JITBuilder`.

## Context Read

- `soac-jit/src/jit/mod.rs`: declares import specs for these helpers and calls `register_specialized_jit_symbols`.
- `soac-jit/src/jit/deopt_interpreter.rs`: invoked by `dp_jit_deopt_resume`.
- `crate::module_constants`: runtime-name load and missing-name error helpers.
- `crate::operator_specialization`: exact-int operator enums used by numeric specialization helpers.
- `crate::config::jit_perf_helper_frames_enabled`: selects fast versus perf-frame helper symbols.
