# Specialization

This document describes the runtime specializations that SOAC currently
implements for JIT codegen.

For each specialization, this covers:

- what profiling input is recorded
- what specialized code is emitted on the second pass
- current limitations, soundness boundaries, and likely extensions


## Profiling Input

The counter dump contains hot specialization input, cold key-layout
metadata, and optional verify counters for indexed storage fast paths
and applied refcount operations.
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

- `block_entry`
  - Instrumented for every lowered basic block in
    `instrument_bb_module_with_block_entry_counters`, at
    `soac-blockpy/src/passes/trace/mod.rs:107`.
  - Records scalar visit counts keyed by `(function_id, block_label)`.
  - Consumed from the binary counter dump in
    `collect_block_entry_counts_for_function`, then replayed as
    conservative cold-block hints for low-frequency non-entry blocks
    during apply/verify JIT lowering.

The cold metadata stream records dictionary-key layouts for replay:

- `module_keys`
  - Recorded in `SOAC_OPT_MODE=profile|verify`.
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

Verify-mode refcount counters are scalar per-function counters:

- `runtime_incref` / `runtime_decref`
  - Instrumented only in `SOAC_OPT_MODE=verify` by
    `instrument_bb_module_with_refcount_counters`, at
    `soac-blockpy/src/passes/trace/mod.rs`.
  - Count applied SOAC runtime refcount operations emitted through the
    JIT refcount helper path. Immortal or null values skipped by the
    runtime helper do not increment these counters.
  - These counters are diagnostic verification output; profile/apply
    modes do not emit counted refcount helper wrappers.

Normal multi-pass runs use one work directory and one mode:

- `SOAC_WORK_DIR=/path/to/dir`
- `SOAC_OPT_MODE=profile`
  - run unspecialized
  - instrument specialization-input counters
  - write `/path/to/dir/profile.bin`
- `SOAC_OPT_MODE=verify`
  - read `/path/to/dir/profile.bin`
  - apply specializations
  - instrument specialization-input counters
  - instrument applied runtime incref/decref counters
  - write `/path/to/dir/verify.bin`
- `SOAC_OPT_MODE=apply`
  - read `/path/to/dir/profile.bin`
  - apply specializations
  - emit no specialization-input counters

The JIT loads hot profile input from `$SOAC_WORK_DIR/profile.bin` in
apply/verify mode:

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


## Profiled Cold Blocks

### Counted Input

- Source input is `block_entry`.
- Every lowered basic block gets a scalar entry counter while profiling
  is enabled.
- Replay compares each non-entry block's visit count against the
  function entry block count.

### Codegen

- When `SOAC_ENABLE_PROFILED_COLD_BLOCKS=1`, apply/verify mode replays
  `block_entry` counters and marks non-entry blocks visited at most 1%
  as often as the function entry block as `cold` in Cranelift IR.
- This is a block-placement/layout hint only. It does not change Python
  semantics or skip code generation for those blocks.
- The `block_entry` counters are still recorded in profile/verify even
  when the replay hint stays disabled.

### Limits

- The current heuristic is deliberately conservative for small sample
  counts: when the function entry count is below 100, only zero-visit
  non-entry blocks can become cold.
- Missing or mismatched block-entry rows disable the hint for that
  function rather than guessing.


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
- In `apply` mode, store fast paths can be emitted for non-module-scope
  code. The helper then performs a raw store into the expected
  indexed-values slot, including null first-insert slots or tombstone
  slots that CPython would normally treat as deleted/absent.

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
- `SetAttr` sites use generic attribute set in profile/verify mode.
- In `apply` mode, constant-string `SetAttr` sites with a recorded key
  index get the same exact-owner/version guard.
- When loading those specializations, SOAC best-effort primes the owner
  type's shared-key layout from the recorded `type_keys` stream so fresh
  instances in apply/verify mode already have the expected split-key
  slots.
- After the guard, the local-runtime helper stores directly into the
  expected split-dict or inline-values slot. First inserts update the
  split-values insertion order and split-dict `ma_used` when the class
  layout has already been primed.

### Limitations / Soundness / Extensions

- The owner guard is exact-type today; it is sound but does not yet keep
  base-class field fast paths active on subclasses.
- Direct field stores remain an apply-mode behavior change. They still
  bypass CPython watcher and version bookkeeping on the raw slot-store
  path, and owner types that cannot be safely primed still fall back on
  the first store until normal CPython execution establishes the shared
  key layout.
- Class attributes and descriptors are excluded by compile-time owner
  inspection. Runtime type-version guards are the fallback if a later
  class mutation invalidates that inspection.


## Direct Function Calls

### Counted Input

- Source input is `call_hot_targets`.
- The observed value is the callee `FunctionId` recovered by
  `emit_callee_function_id_checked`, at
  `soac-jit/src/jit/mod.rs:3032`.
- This only applies to `Call` sites with:
  - no keywords
  - no starred / unpacked arguments
  - a target signature that can be bound to direct-entry parameter
    slots using positional inputs plus default sentinels

### Codegen

- The generic direct-call specialization path lives in the `Call`
  lowering branch in `emit_codegen_expr`, at
  `soac-jit/src/jit/mod.rs:4777`.
- On the hot path it:
  - computes the callee `FunctionId`
  - compares it against profiled targets
  - builds a direct argument plan for the target entry ABI, including
    null sentinels for omitted defaulted parameters
  - emits a direct Cranelift `call` to the already-compiled specialized
    runner for that function via
    `emit_direct_call_resolved_with_arg_plan`, at
    `soac-jit/src/jit/mod.rs:3515`
- On miss it falls back to normal Python vectorcall lowering.

### Limitations / Soundness / Extensions

- Current limitations:
  - keywords are excluded
  - starred / unpacked args are excluded
  - variadic target params are excluded
  - required keyword-only target params are excluded unless they have a
    default value
- Soundness boundary:
  - this is sound as long as the `FunctionId` metadata attached to
    transformed Python functions stays correct
  - the specialization is guarded by exact callee identity, so misses
    fall back cleanly
- Natural extensions:
  - keyword argument binding for profiled call sites
  - richer support for more Python callable shapes that still map to a
    transformed function body


## Direct Method Calls

### Counted Input

- Source input is also `call_hot_targets`.
- This specialization is only considered when the call target is a
  `GetAttr`, in `direct_method_specializations_for_call_site`, at
  `soac-jit/src/jit/mod.rs:2722`.
- The profiled hot target is still the method function's `FunctionId`.
- The specialization then refines that with owner-type metadata from
  `lookup_exact_owner_types_for_method`, called from
  `direct_method_specializations_for_call_site`, at
  `soac-jit/src/jit/mod.rs:2766`.

### Codegen

- Method specialization is emitted in `emit_codegen_expr`, at
  `soac-jit/src/jit/mod.rs:4584`.
- The fast path:
  - evaluates the receiver once
  - guards exact owner type and owner type version with direct JIT
    loads from `PyObject.ob_type` and `PyTypeObject.tp_version_tag`
  - uses the descriptor function object directly
  - prepends the receiver as arg0
  - emits a direct Cranelift `call` to the compiled target function
- On miss it falls back to ordinary attribute lookup plus generic call
  lowering.

### Limitations / Soundness / Extensions

- Current limitations:
  - only constant-string `GetAttr` call targets
  - no keywords or starred / unpacked args
  - explicit args plus the implicit receiver must bind to the target's
    direct-entry parameter slots
  - only methods backed by transformed Python functions with registered
    owner-type metadata
- Soundness boundary:
  - relies on exact owner type and type-version guards
  - if owner-type invalidation misses a real semantic mutation, this
    path could become unsound
- Natural extensions:
  - keyword argument binding
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
  `soac-jit/src/jit/mod.rs:2785`, which uses
  `lookup_exact_owner_types_for_constructor`, at
  `soac-jit/src/lib.rs:1008`.

### Codegen

- Constructor specialization is emitted from the `Call` lowering path
  in `emit_codegen_expr`, at `soac-jit/src/jit/mod.rs:4775`.
- The actual constructor fast path is
  `emit_direct_constructor_resolved_with_arg_values`, at
  `soac-jit/src/jit/mod.rs:3394`.
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
  - no keywords or starred / unpacked args
  - explicit args plus the allocated instance must bind to the target's
    direct-entry parameter slots
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
  - keyword argument binding
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


## Static Runtime Builtin Primitives

### Counted Input

- There is no profile counter for the first version.
- Candidate calls are recognized statically when name binding has already
  proven the callee load is a runtime-name constant for `ord` or `chr`.
- Global-name loads are not candidates at codegen time. That avoids treating
  a shadowable module global as a builtin primitive.

### Codegen

- `ord(x)` can emit a direct call to the `soac-runtime`
  `soac_runtime_builtin_ord_i64` primitive. The primitive accepts the argument
  as a borrowed `PyObject*`, performs CPython-compatible validation internally,
  sets `PyThreadState.current_exception` on failure, and returns an `i64`.
- `len(x)` uses the same scalar-returning runtime primitive shape via
  `soac_runtime_builtin_len_i64`: borrowed `PyObject*` input, `i64` result, and
  `PyThreadState.current_exception` error reporting.
- If the consumer demands a Python object, the `i64` result is boxed through
  the existing `emit_to_python_long` coercion path. If the consumer demands an
  `i64`, the scalar value is used directly.
- `chr(ord(x))` can emit `ord_i64(x)` followed by
  `soac_runtime_builtin_chr_i64(tstate, codepoint)`, avoiding a temporary
  `PyLong` between the two builtins.
- `chr(<i64 module constant>)` can pass the constant directly to
  `soac_runtime_builtin_chr_i64` instead of first materializing a temporary
  `PyLong`.
- `chr(x)` can use the same scalar path when local value facts prove that `x`
  is an exact `PyLong`. Codegen emits the general
  `soac_runtime_pylong_as_i64_saturating` coercion before calling
  `soac_runtime_builtin_chr_i64`; huge Python integers become out-of-range
  scalar codepoints so `chr_i64` still raises CPython-compatible `ValueError`.
- Scalar-returning primitive calls check `PyThreadState.current_exception`
  immediately after the call. On error, codegen preserves the exception while
  releasing owned temporaries, then jumps to the normal Python exception path.

### Limitations / Soundness / Extensions

- The current `chr` primitive path is only selected when its argument can
  satisfy an `i64` demand: direct `ord(x)`, an `i64` module constant, or an
  expression/local with exact `PyLong` facts. Plain `chr(x)` still uses generic
  vectorcall when the argument facts are unknown.
- The runtime primitives use checked CPython C-API calls for Unicode handling.
  Direct Unicode layout reads are a later optimization.
- Error messages are not yet preserved on the checked primitive's explicit
  validation failures; the correct exception type is set. C-API failures still
  keep CPython's own error.
- Static builtin recognition relies on the existing runtime-name lowering
  behavior. That path is intentionally unsound for later module-global
  shadowing of undeclared builtin names and is marked with `BEHAVIOR_CHANGE`
  in the name-binding pass.
- Natural extensions:
  - carry `i64` results through stores and more expression forms
  - support integer literals and profiled integer-returning functions as
    inputs to `chr`
  - use the same direct-call descriptor mechanism for regular SOAC Python
    functions with typed return ABIs


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
