# Specialization

This document describes the runtime specializations that SOAC currently
implements for JIT codegen.

For each specialization, this covers:

- what profiling input is recorded
- what specialized code is emitted on the second pass
- current limitations, soundness boundaries, and likely extensions


## Profiling Input

There are currently two hot specialization input streams and one cold
key-layout metadata stream in the counter dump. The hot streams are
consumed by the current replay path:

- `call_hot_targets`
  - Instrumented for `Call` expressions with no keywords and only
    positional arguments in
    `instrument_bb_module_with_call_target_counters`, at
    `soac-blockpy/src/passes/trace/mod.rs:186`.
  - Records observed packed `FunctionId` values via the runtime
    heavy-hitter counter path.
  - Consumed from the binary counter dump in
    `collect_call_target_specializations_for_function`, at
    `soac-jit/src/counter_dump.rs:759`.

- `operator_hot_shapes`
  - Instrumented for candidate `BinOp` and `UnaryOp` expressions in
    `instrument_bb_module_with_call_target_counters`, at
    `soac-blockpy/src/passes/trace/mod.rs:190`.
  - Records packed operand shape tags, currently exact-type tags only.
  - Consumed from the binary counter dump in
    `collect_operator_specializations_for_function`, at
    `soac-jit/src/counter_dump.rs:789`.

The cold metadata stream records dictionary-key layouts for later replay
passes, but does not currently change emitted code:

- `module_keys`
  - Recorded when `DIET_PYTHON_KEY_LAYOUT_COUNTERS=1`, or implicitly
    when `DIET_PYTHON_CALL_TARGET_COUNTERS=1`.
  - Contains the lowered module global-name table as
    `(module_name, key, index)` entries.
  - Consumed from the binary counter dump with
    `collect_module_key_layouts`, at `soac-jit/src/counter_dump.rs`.

- `type_keys`
  - Recorded from the vendored CPython split-key insertion watcher for
    transformed classes created while key-layout profiling is enabled.
  - Contains `(module.qualname, key, split_keys_index)` entries.
  - Consumed from the binary counter dump with
    `collect_type_key_layouts`, at `soac-jit/src/counter_dump.rs`.

Planned use: a later attribute-load/store replay pass can combine
`type_keys` with profile counts, guard that an object's instance dict
still uses the expected shared-key layout, and load the value slot by the
recorded key index. That pass must still provide a normal CPython
attribute-lookup fallback.

Normal multi-pass runs use one counters directory and one mode:

- `DIET_PYTHON_COUNTERS_DIR=/path/to/dir`
- `DIET_PYTHON_SPECIALIZATION_MODE=profile`
  - run unspecialized
  - instrument specialization-input counters
  - write `/path/to/dir/profile.bin`
- `DIET_PYTHON_SPECIALIZATION_MODE=verify`
  - read `/path/to/dir/profile.bin`
  - apply specializations
  - instrument specialization-input counters
  - write `/path/to/dir/verify.bin`
- `DIET_PYTHON_SPECIALIZATION_MODE=apply`
  - read `/path/to/dir/profile.bin`
  - apply specializations
  - emit no specialization-input counters

The JIT also supports the low-level file override
`DIET_PYTHON_COUNTERS_FILE`. It loads both hot kinds from that file, or
from the mode-derived `profile.bin`, unless an explicit specialization
override env var is present:

- `load_call_target_specializations`, at
  `soac-jit/src/jit/mod.rs:1966`
- `load_operator_specializations`, at
  `soac-jit/src/jit/mod.rs:2057`


## Direct Function Calls

### Counted Input

- Source input is `call_hot_targets`.
- The observed value is the callee `FunctionId` recovered by
  `emit_callee_function_id_checked`, at
  `soac-jit/src/jit/mod.rs:2084`.
- This only applies to `Call` sites with:
  - no keywords
  - only positional arguments

### Codegen

- The generic direct-call specialization path lives in the `Call`
  lowering branch in `emit_codegen_expr`, at
  `soac-jit/src/jit/mod.rs:3720`.
- On the hot path it:
  - computes the callee `FunctionId`
  - compares it against profiled targets
  - emits a direct `call_indirect` to the already-compiled specialized
    runner for that function via
    `emit_direct_call_resolved_with_arg_values`, at
    `soac-jit/src/jit/mod.rs:2378`
- On miss it falls back to normal Python vectorcall lowering.

### Limitations / Soundness / Extensions

- Current limitations:
  - keywords are excluded
  - starred args are excluded
  - argument count must exactly match the target function parameter
    count
- Soundness boundary:
  - this is sound as long as the `FunctionId` metadata attached to
    transformed Python functions stays correct
  - the specialization is guarded by exact callee identity, so misses
    fall back cleanly
- Natural extensions:
  - omitted-default direct calls
  - keyword-only/default argument handling for profiled call sites
  - richer support for more Python callable shapes that still map to a
    transformed function body


## Direct Method Calls

### Counted Input

- Source input is also `call_hot_targets`.
- This specialization is only considered when the call target is a
  `GetAttr`, in `direct_method_specializations_for_call_site`, at
  `soac-jit/src/jit/mod.rs:1723`.
- The profiled hot target is still the method function's `FunctionId`.
- The specialization then refines that with owner-type metadata from
  `lookup_exact_owner_types_for_method`, called from
  `direct_method_specializations_for_call_site`, at
  `soac-jit/src/jit/mod.rs:1780`.

### Codegen

- Method specialization is emitted in `emit_codegen_expr`, at
  `soac-jit/src/jit/mod.rs:3530`.
- The fast path:
  - evaluates the receiver once
  - guards exact owner type and owner type version via
    `dp_jit_guard_method_type_version`
  - uses the descriptor function object directly
  - prepends the receiver as arg0
  - emits a direct `call_indirect` to the compiled target function
- On miss it falls back to ordinary attribute lookup plus generic call
  lowering.

### Limitations / Soundness / Extensions

- Current limitations:
  - only constant-string `GetAttr` call targets
  - no keywords / only positional args
  - exact-arity match only
  - only methods backed by transformed Python functions with registered
    owner-type metadata
- Soundness boundary:
  - relies on exact owner type and type-version guards
  - if owner-type invalidation misses a real semantic mutation, this
    path could become unsound
- Natural extensions:
  - omitted defaults and kwargs
  - broader descriptor families
  - more precise subtype-friendly guards where exact owner type is too
    strict but still provably safe


## Type Constructors

### Counted Input

- Source input is again `call_hot_targets`.
- Constructor calls do not get their own counter kind; they reuse the
  observed `FunctionId` for the hot transformed `__init__` target.
- The constructor-specific refinement happens in
  `direct_constructor_specializations_for_call_site`, at
  `soac-jit/src/jit/mod.rs:1813`, which uses
  `lookup_exact_owner_types_for_constructor`, at
  `soac-jit/src/lib.rs:795`.

### Codegen

- Constructor specialization is emitted from the `Call` lowering path
  in `emit_codegen_expr`, at `soac-jit/src/jit/mod.rs:3718`.
- The actual constructor fast path is
  `emit_direct_constructor_resolved_with_arg_values`, at
  `soac-jit/src/jit/mod.rs:2422`.
- The fast path:
  - guards exact callee object identity against the profiled owner type
  - guards the owner type version
  - allocates with `dp_jit_pytype_generic_alloc`
  - directly calls the transformed `__init__`
  - finalizes through `dp_jit_finish_constructor_init`, which enforces
    `__init__` returning `None`
- Helper entrypoints are:
  - `pytype_generic_alloc_hook`, at
    `soac-jit/src/jit/specialized_helpers.rs:107`
  - `finish_constructor_init_hook`, at
    `soac-jit/src/jit/specialized_helpers.rs:118`

### Limitations / Soundness / Extensions

- Current limitations:
  - only heap types with a simple default constructor shape
  - exact callee type object match
  - exact-arity positional calls only
  - no kwargs
  - no custom metaclass `__call__`
  - no custom `tp_new`
  - no custom allocator
  - no abstract types
- Soundness boundary:
  - this path is intentionally restricted to types where the default
    `type.__call__` shape can be reproduced safely enough:
    exact type object, default `type` metaclass call semantics,
    `object.__new__`, and `PyType_GenericAlloc`
  - broadening beyond that without stronger guards would be unsound,
    because class calls are not generally equivalent to direct
    `__init__` calls
- Natural extensions:
  - omitted-default constructor args
  - kwargs
  - explicit support for selected builtin constructors
  - broader safe subsets of heap types, if we can prove the constructor
    protocol remains equivalent to the fast path


## Exact-Int Binary Operators

### Counted Input

- Source input is `operator_hot_shapes`.
- Candidate operators are `BinOp` nodes excluding:
  - `Contains`
  - `Is`
  - `MatMul`
  - `InplaceMatMul`
- Candidate detection is in
  `instrument_bb_module_with_call_target_counters`, at
  `soac-blockpy/src/passes/trace/mod.rs:190`.
- Shapes are packed exact-type tags defined in
  `soac-jit/src/operator_specialization.rs:4`.
- Today the only exact type tag is `ExactTypeTag::Int`.

### Codegen

- Binary specialization is emitted in
  `emit_specialized_binop`, at
  `soac-jit/src/jit/intrinsics.rs:714`.
- The fast path:
  - records the current observed operand shape
  - compares it against the profiled exact-int shape
  - on hit, calls the exact-int helper
  - on miss, falls back to the normal Python operator lowering
- The current specialized operator space covers:
  - arithmetic
  - bitwise ops
  - in-place arithmetic / bitwise ops
  - comparisons represented as binary op kinds

### Limitations / Soundness / Extensions

- Current limitations:
  - only exact `int`/`int`
  - no mixed-type shapes
  - excluded binops still always use generic lowering
- Soundness boundary:
  - specialization is guarded by exact observed type-shape match
  - unsupported or mismatched shapes always fall back
- Natural extensions:
  - `float`, `str`, `bytes`, `bool`, and mixed-type shapes
  - richer shape encodings
  - specialization for more operations that are currently excluded


## Exact-Int Unary Operators

### Counted Input

- Source input is `operator_hot_shapes`.
- Candidate operators are all `UnaryOp` nodes, via
  `instrument_bb_module_with_call_target_counters`, at
  `soac-blockpy/src/passes/trace/mod.rs:190`.
- Shapes are packed exact-type tags from
  `soac-jit/src/operator_specialization.rs:4`.

### Codegen

- Unary specialization is emitted in
  `emit_specialized_unary_op`, at
  `soac-jit/src/jit/intrinsics.rs:801`.
- The fast path:
  - records the observed unary operand shape
  - checks for exact `int`
  - on hit, calls the exact-int unary helper
  - on miss, falls back to generic Python unary lowering

### Limitations / Soundness / Extensions

- Current limitations:
  - only exact `int`
- Soundness boundary:
  - exact-type guard plus generic fallback
- Natural extensions:
  - additional exact-type tags
  - mixed numeric shape handling where unary semantics make sense


## Exact-Int Comparisons

### Counted Input

- There is no separate comparison counter kind.
- Comparisons reuse the binary operator machinery, because comparison
  operations are represented in the `BinOpKind` space for this stage.
- The same `operator_hot_shapes` counter input drives them.

### Codegen

- Comparison specialization also goes through
  `emit_specialized_binop`, at
  `soac-jit/src/jit/intrinsics.rs:714`.
- If the profiled shape is exact `int`/`int`, comparisons such as
  `Eq`, `Ne`, `Lt`, `Le`, `Gt`, and `Ge` use the direct exact-int
  helper path instead of generic `PyObject_RichCompare` lowering.

### Limitations / Soundness / Extensions

- Current limitations:
  - only exact `int`/`int`
  - no string/bytes/tuple/list comparison specialization
- Soundness boundary:
  - exact-shape guarded, otherwise generic fallback
- Natural extensions:
  - exact `str` comparisons
  - exact `float` comparisons
  - container comparisons where CPython semantics are stable enough to
    encode directly


## What Is Not Yet Specialized

Some notable hot paths still use only generic lowering:

- keyword calls
- starred-argument calls
- omitted-default profiled direct calls
- most constructor calls outside the current simple default-constructor
  subset
- most non-`int` operator shapes

Those are the main expansion areas if we want the specialization system
to cover more of the remaining Python/runtime overhead.
