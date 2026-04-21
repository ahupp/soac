# crates/soac_jit_runtime/src/lib.rs

## File Responsibilities

Very-hot raw ABI runtime helper library for generated SOAC code. It mirrors selected CPython object layouts, provides inlinable reference-counting, exception, tuple, builtin scalar, global, and field fast paths, and falls back to CPython/SOAC slow helpers when fast-path guards fail.

## Datatypes

- `PyObjectObFlagsAndRefcnt`: CPython object flag/refcount layout, with separate definitions for free-threaded vs non-free-threaded builds.
- `RawPyObject`: minimal CPython object header mirror.
- `RawPyVarObject`: variable-sized object header mirror with `ob_size`.
- `RawPyTupleObject`: tuple layout mirror for fresh-tuple item stores.
- `RawPyTypeObject`: partial type-object layout used for flags, basic size, dict offset, version tag, and unicode check.
- `RawPyHeapTypeObject`: heap-type layout mirror used to inspect cached keys for field access.
- `RawPyDictObject`, `RawPyDictKeysObject`, `RawPyDictUnicodeEntry`, `RawPyDictIndexedValues`, `RawPyDictSplitValues`: dict and split-table layout mirrors for indexed globals/fields.
- `RawPyThreadState`: partial thread-state mirror exposing `current_exception`.
- Constants: CPython flag/offset constants for managed dicts, inline values, and managed-dict placement.

## Functions and Macros

- Raw layout helpers: `dict_unicode_entries`, `indexed_key`, `indexed_value`, `set_indexed_value`, `split_value`, `set_split_value`, `split_values_insertion_order_array`, and `add_split_value_to_insertion_order` compute and mutate CPython dict/table slots.
- Refcount helpers: `can_skip_incref`, `can_skip_decref`, `decref_impl`, `incref_impl`, `soac_runtime_decref_dealloc_preserving_error`, `soac_runtime_decref`, `soac_runtime_incref`, `soac_runtime_decref_applied`, and `soac_runtime_incref_applied` implement fast refcount paths while preserving Python exceptions around deallocation when needed.
- Exception helpers/macros: `set_raised_exception_direct` and `soac_runtime_set_raised_exception` write `PyThreadState.current_exception` directly.
- Tuple helpers: `soac_runtime_tuple_new` calls `PyTuple_New`; `soac_runtime_tuple_set_item_stolen` writes a stolen reference into a proven-fresh tuple slot.
- Example helpers: `soac_runtime_example_known_value_source` and `soac_runtime_example_offset_known_value` support known-value/offset tests.
- Builtin scalar helpers: `soac_runtime_builtin_ord_i64`, `soac_runtime_builtin_len_i64`, `soac_runtime_builtin_chr_i64`, `soac_runtime_pylong_as_i64`, and `soac_runtime_pylong_as_i64_saturating` implement integer-returning builtin/coercion fast paths with Python exceptions set internally on failure.
- Dict guard helpers/macros: `dict_guarded_index`, `probe_indexed_dict_value`, `probe_split_values`, and `probe_split_dict_value` validate dict key layouts and load candidate values.
- Global helpers: `soac_runtime_probe_global_indexed`, `soac_runtime_load_global`, `soac_runtime_store_global_indexed`, and `soac_runtime_store_global` implement profiled indexed global load/store and slow fallback behavior.
- Field helpers/macros: `object_dict`, `inline_values`, `cached_keys`, `probe_field_value`, `soac_runtime_probe_field_indexed`, and `soac_runtime_store_field_indexed` implement profiled instance-field load/store paths for managed dicts and inline values.
- External CPython/SOAC symbols: imported C functions and statics provide deallocation, exception creation, unicode/long/tuple APIs, dict indexed stores, and slow global stores.

## Context Read

- `doc/RUNTIME_FUNCTIONS.md`
- `crates/soac_jit/src/jit/mod.rs`
- `crates/soac_jit/src/jit/specialized_helpers.rs`
- CPython raw object layout assumptions in `vendor/cpython` headers were treated as the ABI source, but not edited.

