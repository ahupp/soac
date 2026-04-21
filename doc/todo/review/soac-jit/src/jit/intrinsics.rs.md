# crates/soac_jit/src/jit/intrinsics.rs

## File Responsibilities

Emits Cranelift for BlockPy intrinsic operations that map to CPython API calls, SOAC runtime helpers, or known
specializations. This includes arithmetic/operator calls, global loads/stores, field get/set fast paths, cell operations,
truthiness, and specialization counters. The file is parameterized through `OperationEmitState` so the same intrinsic
emission logic can be used from the larger JIT codegen state.

## Datatypes

- `OperationEmitState<'fb, E>`: trait implemented by codegen state objects; exposes the `JitEmitCtx`, Cranelift builder,
  import declaration, argument emission/release, result finishing, truth/bool conversion, type-pointer emission, and value
  facts.
- `define_owned_import_spec`, `define_bool_import_spec`: macros for declaring CPython/SOAC helper import specs.
- `PYNUMBER_*`, `PYOBJECT_*`, `PYSEQUENCE_CONTAINS_IMPORT`, `DP_JIT_*`, `SOAC_RUNTIME_*`: static import descriptors for
  operator, attribute/item, global, runtime-name, cell, and exact-long helper calls.
- `PYOBJECT_OB_TYPE_OFFSET`: offset used to load a PyObject's type pointer for operator-shape specialization.

## Functions

- Generic call helpers: `emit_positional_owned_call`, `emit_positional_owned_call_from_values`,
  `emit_positional_bool_call_from_values`, `emit_pow_like_from_values`, `emit_richcompare_from_values`, and
  `emit_identity_compare_from_values` emit common CPython call shapes and release argument temporaries.
- Attribute helpers: `emit_counted_getattr_fallback`, `emit_specialized_getattr`, `emit_setattr_fallback`, and
  `emit_specialized_setattr` emit generic attribute access plus guarded indexed-field fast paths and hit/fallback counters.
- Cell/item helpers: `emit_make_cell`, `emit_getitem`, `emit_setitem`, and `emit_del_deref_raw_cell` emit cell creation,
  item access, item mutation, and raw-cell deletion.
- Operator-shape helpers: `emit_exact_type_tag_for_value`, `emit_unary_operator_shape_from_values`, and
  `emit_binary_operator_shape_from_values` derive packed specialization shape tags from runtime operand types.
- Exact-long helpers: `emit_exact_long_binary_op` and `emit_exact_long_unary_op` emit calls to direct PyLong slot helpers or
  generic exact-long helper dispatch.
- Generic operator helpers: `emit_binop_with_arg_values`, `emit_binop`, `emit_unary_op_with_arg_values`,
  `emit_unary_op_with_arg_and_values`, and `emit_unary_op` map BlockPy operator kinds to CPython/SOAC helper calls.
- Operator specialization helpers: `emit_specialized_binop` and `emit_specialized_unary_op` record operand-shape samples,
  guard hot exact-int shapes, and branch to specialized or generic code.
- Counter/global helpers: `increment_counter_with_state`, `emit_indexed_global_load_with_state`, `emit_load`, `emit_store`,
  and `emit_del` emit scalar counters and global runtime helper access, including indexed global fast paths.
- `emit_operation`: top-level dispatcher for intrinsic `InstrCodegen` variants that this file owns; returns `None` for
  operations handled by the main codegen path.

## Context Read

- `crates/soac_jit/src/jit/mod.rs`: supplies `JitEmitCtx`, `ImportSpec`, counter helpers, type guards, module constants, and the
  implementation of `OperationEmitState`.
- `crates/soac_jit/src/jit/specialized_helpers.rs`: implements many imported `dp_jit_*` helper symbols.
- `crate::operator_specialization`: defines operator shape packing and exact-int operator kinds.
- `crate::jit::blockpy_intrinsics` and `soac_blockpy::block_py`: define intrinsic operation nodes and semantic ids.
