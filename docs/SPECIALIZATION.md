# Specialization

This document describes the runtime specializations that SOAC currently
implements for JIT codegen.

For each specialization, this covers:

- what profiling input is recorded
- what specialized code is emitted on the second pass
- current limitations, soundness boundaries, and likely extensions


## Profiling Input

The counter dump contains hot specialization input, cold key-layout
metadata, and optional verify counters for indexed storage fast paths.
The hot streams consumed by the current replay path are:

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

- `branch_outcomes`
  - Instrumented for `IfTerm` terminators, keyed by the conditional
    test expression's `InstrId`.
  - Records the boolean result consumed by specialized JIT branch
    lowering: observed value `1` is the `then` edge, and observed
    value `0` is the `else` edge.
  - Consumed from the binary counter dump in
    `collect_branch_preferences_for_function`, then replayed as a
    per-site preference for true-hot or false-hot lowering.

The cold metadata stream records dictionary-key layouts for replay:

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

Verify-mode indexed-storage counters are scalar per-site counters:

- `global_indexed_hit` / `global_indexed_fallback`
  - Instrumented for lowered global loads/stores.
  - Count whether the indexed module-dict helper returned a direct
    result or needed the normal global slow path.

- `field_indexed_hit` / `field_indexed_fallback`
  - Instrumented for `GetAttr`/`SetAttr`.
  - Count whether a type-key specialization loaded the instance
    split-dict value directly or fell back to CPython attribute access.

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
`DIET_PYTHON_COUNTERS_FILE`. It loads hot profile input from that file,
or from the mode-derived `profile.bin`, unless an explicit
specialization override env var is present:

- `load_call_target_specializations`, at
  `soac-jit/src/jit/mod.rs:1966`
- `load_operator_specializations`, at
  `soac-jit/src/jit/mod.rs:2057`
- `load_branch_preferences`, at `soac-jit/src/jit/mod.rs`


## Profiled Branch Locality

### Counted Input

- Source input is `branch_outcomes`.
- A counter definition is attached to each `IfTerm` test instruction
  while profile/verify instrumentation is enabled.
- Specialized JIT code records the post-truthiness boolean immediately
  before lowering the terminator. This avoids conflating Python objects
  that were truth-tested differently.

### Codegen

- In apply/verify mode, counter replay compares false and true sample
  counts for each conditional test `InstrId`.
- If true is at least as hot as false, codegen emits the normal
  `brif(is_true, then_arm, else_arm)` shape.
- If false is hotter, codegen inverts the Cranelift predicate and emits
  the else arm as the first branch:
  `brif(is_false, else_arm, then_arm)`.

### Limitations / Soundness / Extensions

- This is a layout / branch-shape hint only. It does not rewrite
  BlockPy control flow and does not change which Python block each
  outcome reaches.
- The current profile input is for conditional terminators. It does not
  yet record a uniform "source block -> destination block" stream for
  unconditional jumps or branch tables.
- Counts come from the top-two heavy-hitter storage. That exactly covers
  boolean branches, but a future branch-table locality profile needs a
  wider representation or per-edge scalar counters.


## Indexed Globals

### Counted Input

- Source layout input is `module_keys`.
- In profile/apply modes, the transformed module object creates an
  indexed unicode dict whose key table matches the lowered module
  global-name table.
- In verify mode, each global load/store also gets
  `global_indexed_hit` and `global_indexed_fallback` scalar counters.

### Codegen

- Direct-name global loads/stores use the expected lowered global index.
- The emitted fast path calls a local-runtime helper with the globals
  dict, constant key object, and expected index.
- The helper guards that the globals dict still has an indexed-unicode
  keys object and an indexed-values block large enough for the compiled
  slot, then reads or writes that slot.
- On guard miss, tombstone, absent value, or store failure, codegen
  increments the fallback counter when enabled and executes the existing
  global load/store slow path.
- Store fast paths are emitted only when
  `SOAC_OPT_UNSOUND=1`; the helper then performs a raw
  overwrite of an existing indexed-values slot.

### Limitations / Soundness / Extensions

- This path does not cache a module global value outside the dict; the
  dict remains the storage authority.
- Builtins readthrough stays on the slow path because a value appearing
  in the module globals dict changes the correct result.
- The fast path is specific to indexed module dictionaries. Ordinary
  `dict` globals miss the guard and use the existing slow path.


## Indexed Instance Fields

### Counted Input

- Source layout input is `type_keys`, recorded from CPython split-key
  insertion events.
- Codegen resolves a recorded owner name to the currently imported type,
  then rejects the specialization if a class binding/descriptor for that
  attribute is present.
- In verify mode, each `GetAttr`/`SetAttr` also gets
  `field_indexed_hit` and `field_indexed_fallback` scalar counters.

### Codegen

- Constant-string `GetAttr` sites with a recorded key index get a
  guard on exact owner type and owner type version.
- After that guard, the inlineable local-runtime helper checks the
  currently attached split dict, or the object's CPython inline-values
  block when no dict has been materialized, for the expected key at the
  recorded index. Loads return an owned reference to the value slot.
- Missing values, invalidated inline-values blocks, promoted/combined
  dicts, key-index mismatch, type guard miss, or type-version miss
  increment the fallback counter when enabled and execute normal CPython
  attribute lookup.
- `SetAttr` sites use generic attribute set by default.
- When `SOAC_OPT_UNSOUND=1`, constant-string `SetAttr`
  sites with a recorded key index get the same exact-owner/version guard
  and then perform a raw overwrite of an existing split-dict or
  inline-values slot.

### Limitations / Soundness / Extensions

- The owner guard is exact-type today; it is sound but does not yet keep
  base-class field fast paths active on subclasses.
- Direct field stores are opt-in/unsound. First insert, missing value,
  invalid inline values, promoted dict, and key-index mismatch still
  fall back; an existing value slot may be overwritten without CPython
  bookkeeping only when `SOAC_OPT_UNSOUND=1`.
- Class attributes and descriptors are excluded by compile-time owner
  inspection. Runtime type-version guards are the fallback if a later
  class mutation invalidates that inspection.


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
  - guards exact owner type and owner type version via the inlineable
    `soac_runtime_guard_type_version`
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
