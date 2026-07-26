---
title: "Specialization"
---

# Specialization

This document describes the runtime specializations that SOAC currently
implements for JIT codegen.

For each specialization, this covers:

- what profiling input is recorded
- what specialized code is emitted on the second pass
- current limitations, soundness boundaries, and likely extensions


## Optimizer v3 Status

Optimizer v3 is the forward path for new semantic optimization work. The runtime
integration path reads raw profile evidence from `profile.bin` plus the cached
pre-optimization BlockPy module, lowers that module to `InstrTyped`, applies the
selected v3 shapes to that typed module, and then codegen mechanically lowers
the typed result. Runtime `verify`/`apply` and offline precompile both use raw
profile evidence directly; the JIT should not run legacy BlockPy optimization
rewrites or require serialized optimization-plan artifacts.

Profiled ordinary-function and guarded receiver-method direct calls from v3
planning are selected with validated argument plans and serialized target
module identities. Call plans also carry the selected call-body policy:
`DirectCall` for guarded direct-call lowering, or `Inline` when the planner
selected the early BlockPy inline path as the lower-cost body alternative.
`Inline` selection validates that the BlockPy inline-fragment builder can
construct the selected body from the cached target module, and the v3 plan is
the source of truth consumed by typed JIT planning. Method targets carry the
lowered method name as static plan data. Constructor type metadata now points
at a synthetic constructor-entry function id rather than the underlying
`__init__` id. Those entry functions are normal JIT direct-call targets with
an implicit leading type argument, so class calls can reuse the ordinary
guarded direct-call target model. Type registration only attaches that
metadata for safe default constructor shapes, so unsupported Python type-call
cases stay generic and do not enter the synthetic target.
Typed planning can also add static call plans when the callee binding is
compiler-owned rather than profile-discovered. Runtime constructor names such as
`RuntimeName::Range`, `RuntimeName::IterRange`, `RuntimeName::ClosureGenerator`,
and `RuntimeName::ClosureAsyncGenerator` resolve directly to the corresponding
`soac.runtime` constructor-entry targets, and lowered module globals that have
one module-init binding with no later in-module stores or deletes are treated as
static function targets under SOAC's strict-module assumption. Runtime
constructors such as `IterRange(...)` and compiler-generated
`ClosureGenerator(...)` calls can therefore lower as unconditional direct
callable calls; user-module constructors remain generic for now because their
type metadata is not guaranteed to be available while module initialization is
still running. The optimization currently assumes, but does not yet
runtime-enforce, the strict-module rule that outside code cannot later replace
those final globals. The static path is apply/verify-only; profile mode keeps the
original call graph so nested protocol sites still collect ordinary evidence
before later rewrites inline them.
Constant-attribute indexed-field load/store selections from `type_keys` are
also emitted as mechanical v3 indexed-field decisions; JIT validation checks
those emitted decisions against the selected plan and lowered
`GetAttr`/`SetAttr` shape before deriving typed indexed-field guard input.
They are not rewritten into legacy `FunctionProfileEvidence`. When v3 planning
contains the represented exact-int branch shape, JIT term lowering consumes the
mechanical v3 region directly;
otherwise lowering stays on the existing path.
The represented slices are exact-compact-`int` direct comparison branches such
as `a < b`, add-then-compare-to-zero branches such as `a + b > 0`,
value-producing add/sub/mul/bitwise returns such as `return a + b` or
`return a & b`, store RHS expressions such as `c = a + b`, comparison
branches against lowered integer module constants such as `c > 0`, and
value-producing comparison returns such as `return a < b` or `c = a < b`:
`operator_hot_shapes` exact-int evidence proves integer operands,
exact-string evidence proves string comparison operands, a lowered module
constant load proves the integer constant where needed, profiled indexed-global
loads can feed exact-int operator regions and exact-string comparison regions,
profiled indexed-field loads can feed branch-shaped exact-int or exact-string
regions while return/store expression regions keep ordinary `GetAttr` nodes so
the field specialization remains owned by the attribute access site,
the v3 planner emits hot checked-`i64` or exact-unicode operation regions plus
local generic Python fallback regions, materializes Python object results explicitly, and
`emit_mechanical_plan_v3` refuses invalid plans before emitting steps.

Do not expand a legacy family as the primary implementation path unless there is
a specific reason; add the v3 catalog alternative, fact bridge, planner rule,
validation, and mechanical-emitter coverage first.
The cached pre-optimization BlockPy module is intentionally not rewritten by
generic direct-call inlining or constructor scalar replacement before
v3 planning reads it; those transformations must be selected by a
plan or by a later explicitly profiled JIT path.

Current migration surface:

- Live v3 codegen: exact-int direct-compare, compare-with-integer-constant,
  and add/compare branch slices with local generic fallback, exact-int
  add/sub/mul/bitwise return-shaped expression regions with explicit PythonLong
  materialization, exact-int comparison returns with explicit Python bool
  materialization, and exact-string comparison branches or return-shaped
  expressions with exact-unicode guards and Python bool materialization when an
  object result is demanded. Store RHS lowering can consume return-shaped
  expression regions, so simple lowered code like `c = a + b; if c > 0: ...`
  can optimize the add store and the later branch as separate v3 regions.
  Profiled ordinary
  direct calls are selected by v3, emitted with serialized target identities and
  an explicit call-body policy, and embedded into `InstrTyped` during JIT
  planning. Guarded receiver-method calls are selected by v3 from the same
  `call_hot_targets` input, embedded as typed method guards, and lowered through
  the existing guarded direct method codegen shape. Constructor calls are
  selected through the same direct-call machinery when the profiled target is
  the synthetic constructor-entry function and the call can be bound without
  refreshing defaults; the entry currently preserves Python type-call semantics
  by only attaching type metadata for classes that can use direct default
  allocation. Constant-string indexed fields are selected by v3 from raw
  `type_keys`, emitted as mechanical indexed-field decisions, and consumed as
  v3-owned typed attribute inputs.
  Indexed globals are selected by v3 from raw `module_keys` plus lowered
  `NameLocation::Global(slot)` load/store sites, emitted with explicit
  module-dict guard and original-global-access fallback effects, and consumed
  as v3-owned global load/store inputs.
- Not currently a v3 semantic-plan target: branch locality and cold block
  layout hints. These remain layout metadata unless a future CFG-placement plan
  needs to represent them.


## Profiling Input

The counter dump contains hot specialization input, cold key-layout
metadata, and optional verify counters for indexed storage fast paths
and applied refcount operations.
The hot streams consumed by the current replay path are:

- `call_hot_targets`
  - Instrumented for `Call` expressions with no keywords and only
    positional arguments in
    `instrument_bb_module_with_call_target_counters`, at
    `crates/soac_lowering/src/passes/trace/mod.rs:235`.
  - Records observed packed `FunctionId` values via the runtime
    heavy-hitter counter path.
  - Consumed from the binary counter dump in
    `collect_call_target_specializations_for_function`, at
    `crates/soac_jit/src/counter_dump.rs:759`.

- `operator_hot_shapes`
  - Instrumented for candidate `BinOp` and `UnaryOp` expressions in
    `instrument_bb_module_with_call_target_counters`, at
    `crates/soac_lowering/src/passes/trace/mod.rs:235`.
  - Records packed operand shape tags, currently exact-type tags only.
  - Consumed from the binary counter dump in
    `collect_operator_specializations_for_function`, at
    `crates/soac_jit/src/counter_dump.rs:789`.

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
    `crates/soac_lowering/src/passes/trace/mod.rs:112`.
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
    `collect_module_key_layouts`, at `crates/soac_jit/src/counter_dump.rs`.

- `type_keys`
  - Recorded from the vendored CPython split-key insertion watcher for
    transformed classes created while key-layout profiling is enabled.
  - Contains `(module.qualname, key, split_keys_index)` entries.
  - Consumed from the binary counter dump with
    `collect_type_key_layouts`, at `crates/soac_jit/src/counter_dump.rs`.

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
    `crates/soac_lowering/src/passes/trace/mod.rs:136`.
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
  - emit no specialization-input counter dump
  - keep in-process `deopt_entry_guard_miss` counters so apply-mode event
    logs can report which planned source point entered `dp_jit_deopt_resume`

The JIT loads hot profile input from `$SOAC_WORK_DIR/profile.bin` in
apply/verify mode:

- `load_call_target_specializations`, at
  `crates/soac_jit/src/jit/mod.rs:1966`
- `load_operator_specializations`, at
  `crates/soac_jit/src/jit/mod.rs:2057`
- `load_branch_preferences`, at `crates/soac_jit/src/jit/mod.rs`


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
- In v3 planning, the optimizer consumes raw
  `module_keys` plus lowered `NameLocation::Global(slot)` load/store sites and
  emits matching indexed-global selections into the v3 plan.
- In profile and verify modes, and in apply mode for modules without an
  original source-backed named generator, the transformed module creates an
  indexed Unicode dictionary whose key table matches the lowered module
  global-name table. An apply-mode module containing an original source-backed
  named generator instead retains the ordinary Unicode-key dictionary created
  by `PyModule_FromDefAndSpec`; SOAC installs only its module metadata. It does
  not construct or promote an indexed dictionary, and indexed-global
  selections use their existing guard fallback or deopt path.
- In verify mode, each global load/store also gets
  `global_indexed_hit` and `global_indexed_fallback` scalar counters.

### Codegen

- Direct-name global loads/stores use the expected lowered global index.
- In v3 mode, the selected global load/store must come from the in-memory v3
  plan. JIT validation rejects any indexed-global emission whose name, access
  kind, or expected index does not match the lowered `NameLocation::Global`
  instruction.
- Without a v3 indexed-global emission, global loads/stores use the generic
  global helper path.
- The emitted fast path calls a local-runtime helper with the globals
  dict, constant key object, and expected index.
- The helper guards that the globals dict still has an indexed-unicode
  keys object and an indexed-values block large enough for the compiled
  slot, then reads or writes that slot.
- In `profile` mode, load guard misses, tombstones, and absent values
  increment the fallback counter when enabled and execute the existing
  global load slow path so counter collection stays local and non-deopting.
- In `verify`/`apply` mode, load guard misses for planned deopt points
  branch to a cold `dp_jit_deopt_resume` continuation instead of emitting
  the local slow global-load fallback. Loads nested inside another body or term
  expression may reuse the enclosing instruction/term boundary when a
  conservative evaluation-order scan proves that deopting there cannot replay
  an earlier side effect; otherwise they keep the local slow path.
- Indexed-global loads selected as v3 exact-operator or exact-comparison region
  inputs use the same module-dict guard, but a hot miss branches to that
  region's local generic fallback rather than to an instruction-level load
  fallback.
- In `profile` mode, store guard misses or store failures still
  increment the fallback counter when enabled and execute the existing
  global store slow path.
- In `verify`/`apply` mode, store guard misses for planned deopt
  points use a cold `dp_jit_deopt_resume` continuation when the stored
  value operand is safe to replay. Otherwise codegen keeps the existing
  global store slow path.
- In `verify`/`apply` mode, store fast paths can be emitted for
  non-module-scope code. The helper then performs a raw store into the expected
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
- In v3 planning, the optimizer consumes raw
  `type_keys` plus lowered constant-attribute `GetAttr`/`SetAttr` sites and
  emits matching indexed-field selections into the v3 plan.
- Codegen resolves a recorded owner name to the currently imported type,
  then rejects the specialization if a class binding/descriptor for that
  attribute is present.
- In verify mode, each `GetAttr`/`SetAttr` also gets
  `field_indexed_hit` and `field_indexed_fallback` scalar counters.
- Verify mode preserves profiled indexed field accesses when their counters
  are active instead of removing them through constructor scalar replacement.
  Profile mode without selected accesses and apply mode without field counters
  retain their normal object virtualization.

### Codegen

- Constant-string `GetAttr` sites with a recorded key index get a
  guard on exact owner type and owner type version.
- With a v3 indexed-field plan, those recorded key indexes come from validated
  mechanical indexed-field emissions.
- V3 indexed-field emissions remain separate from legacy per-instruction field
  evidence inside `soac_jit`: input preparation validates that the emitted
  access kind and attribute name match the lowered `GetAttr`/`SetAttr`
  instruction, so typed annotation and codegen receive an already shape-checked
  v3 field input instead of silently treating it as another profiled field
  candidate. If a v3 indexed-field input cannot resolve a usable
  owner-type/version guard in the current compile context, typed lowering keeps
  the original generic attribute operation as the local fallback for that site.
  By-attribute layout availability is still shared with the existing constructor
  initializer fast path until that family has its own v3 plan node.
- After codegen-to-typed lowering, these sites are represented as
  `GetAttrTyped` / `SetAttrTyped` operations annotated with a profiled
  indexed-field access plan. The typed plan carries the selected
  owner/type-version/index guard chain so codegen consumes the explicit
  plan instead of reselecting guards from the raw counter tables.
  Generic typed attribute operations still lower directly to normal
  CPython attribute access.
- When a profiled indexed-field load is used as a v3 exact-int or exact-string
  branch-region input, codegen reuses the typed `GetAttr` guard chain for the
  borrowed field value and jumps to the region's local generic fallback on a
  guard or inline-values miss. Return/store expression regions intentionally do
  not consume indexed-field loads as region inputs yet.
- After that guard, loads check the object's CPython inline-values block for
  the expected key at the recorded index and return an owned reference to the
  value slot.
- Missing values, invalidated inline-values blocks, materialized dicts,
  promoted/combined dicts, key-index mismatch, type guard miss, or
  type-version miss increment the fallback counter when enabled and execute
  normal CPython attribute lookup.
- In `verify`/`apply` mode, load guard misses for planned deopt points
  use a cold `dp_jit_deopt_resume` continuation when the receiver and
  attribute-key operands are safe to replay. If the receiver expression
  could have side effects, codegen keeps the local CPython attribute
  lookup fallback instead of deopting before the `GetAttr`.
- `SetAttr` sites use generic attribute set in profile mode.
- In `verify`/`apply` mode, constant-string `SetAttr` sites with a
  recorded key index get the same exact-owner/version guard.
- When loading those specializations, SOAC best-effort primes the owner
  type's shared-key layout from the recorded `type_keys` stream so fresh
  instances in apply/verify mode already have the expected split-key
  slots.
- After the guard, codegen stores directly into the expected inline-values slot.
  First inserts update the split-values insertion order when the class layout
  has already been primed.
- In `verify`/`apply` mode, store guard misses or inline-values misses for
  planned deopt points use a cold `dp_jit_deopt_resume` continuation
  when the receiver, attribute key, and replacement operands are safe to
  replay. Otherwise codegen keeps the local CPython attribute-set
  fallback.

### Limitations / Soundness / Extensions

- The owner guard is exact-type today; it is sound but does not yet keep
  base-class field fast paths active on subclasses.
- Direct field stores remain a verify/apply-mode behavior change. They still
  bypass CPython watcher and version bookkeeping on the raw slot-store
  path, and owner types that cannot be safely primed still fall back on
  the first store until normal CPython execution establishes the shared
  key layout.
- Class attributes and descriptors are excluded by compile-time owner
  inspection. Runtime type-version guards are the fallback if a later
  class mutation invalidates that inspection.


## Direct Function Calls

In v3, the optimizer reads raw `call_hot_targets` evidence,
resolves ordinary-function targets from the cached module set, records those
selections in the v3 plan, and validates that selected inline bodies can be
constructed from the cached target module. The JIT loads the plan, lowers the
cached pre-optimization module to `InstrTyped`, embeds the selected direct-call
or inline shape in typed IR, then builds value facts, locals, refcount
ownership, and deopt resume tables from the typed result. It does not build
owner-attribute guard maps or rewrite method calls from profile evidence.

Current live v3 direct-call support covers ordinary function targets with
validated positional/default argument plans and synthetic constructor-entry
targets whose argument plan does not require default refresh. Runtime-guarded
receiver-method plans are intentionally disabled for now; if such plans appear
in v3 plan/emission data, specialization-input preparation rejects them because
their owner/type guard payload is not yet a static mechanical JIT input.

### Counted Input

- Source input is `call_hot_targets`.
- The observed value is the callee `FunctionId` recovered by
  `emit_callee_function_id_checked`, at
  `crates/soac_jit/src/jit/mod.rs:3032`.
- This only applies to `Call` sites with:
  - no keywords
  - no starred / unpacked arguments
  - a target signature that can be bound to direct-entry parameter
    slots using positional inputs plus default sentinels

### Codegen

- Runtime `verify`/`apply` reads raw profile evidence and remaps selected
  persistent function ids to the live module id before typed planning.
- Batch scheduling builds the typed module from the cached pre-opt module plus
  the in-memory validated v3 plan. Precompile uses the same raw evidence and
  cached pre-opt module shape, so codegen sees the selected operation shapes
  directly in `InstrTyped` in both runtime and precompile flows.
- Residual call nodes that were not selected by the plan use the normal generic
  typed call lowering.
- Inline-winning call sites keep the generic fallback continuation in
  `InstrTyped` so deopt replay has the original call shape. When the exposed
  typed direct-call guard miss is replay-safe and refcount emission is enabled,
  JIT codegen routes the miss arm to `dp_jit_deopt_resume` and suppresses local
  machine code for fallback blocks whose only incoming machine edges deopt.

### Explicit Typed Representation

- Direct-call specialization is represented on `InstrTyped`, not on
  `InstrBlockPy`. The typed rewrite evaluates the callable and arguments once,
  emits typed guard tests for hot targets, emits typed direct-call or inline
  bodies for selected hot arms, and leaves an explicit generic typed fallback
  for the original call shape.
- When no v3 plan owns the site, the original generic call remains in
  `InstrTyped`, except for the explicit static runtime-name targets described
  above.
- Runtime `range` resolves to CPython's original `builtins.range`, preserving
  its exact type, indexing, slicing, comparison, and iterator behavior.
  Exact CPython range iterators use the existing guarded raw range-iteration
  path; the internal `IterRange` class remains available for independently
  selected runtime specializations.
- The rewrite consumes profiled targets that match the ordinary direct-call /
  typed inliner shape: positional-only-or-normal parameters, no keywords, no
  starred args, and positional inputs that can bind through the direct-entry
  argument plan. Omitted trailing/defaulted parameters are passed as default
  sentinels and resolved by the callee's default-resolving direct entry.
  Ordinary stack-only functions that create and return generator instances may
  inline. The generator callable's executable blocks are its resume body, not
  its public factory, so a body with wrapper-owned preserved activation state
  must not be inlined as an ordinary call; true lexical `freevars` or `cellvars`
  also keep the callee out of the typed inliner. Generator-instance
  planning recognizes both strict module-global generator names and locally
  proven generator function values such as nested generator helpers carried
  through local slots. Source-backed named generators retain CPython's original
  generator vectorcall in ordinary and apply modes when their original code
  objects are available. Production compilation and source-backed CLIF/
  InstrTyped inspection use the same original-code matcher, so native perf
  annotations reflect this actual generator decision instead of reconstructing
  an unrelated transformed generator body. Source-backed named generators also
  receive their source code's CPython `MAKE_FUNCTION` version so native calls
  specialize and later defaults or closure mutations retain normal CPython
  invalidation.
  Both ordinary typed direct calls and inlined Python function bodies retain a
  typed runtime-function identity guard and a cold generic fallback. The guard
  also compares the live function code, positional defaults, and keyword
  defaults against owned snapshots captured in ABI-preserving, append-only JIT
  metadata fields. Replacing a transformed function's `__code__` rejects its
  stale direct entry or inline body; its shared vectorcall then restores and
  invokes the original CPython vectorcall without dropping live JIT metadata
  owned by active frames. Replacing positional or keyword defaults refreshes
  the existing function environment on the next call rather than installing a
  process-wide watcher for ordinary functions. Because `__kwdefaults__` is a
  mutable dictionary, a function with non-null keyword-only defaults stays on
  that vectorcall path and rereads its keyword-only default slots before each
  call; pointer identity alone cannot validate an in-place value replacement or
  deletion. Compiler-owned runtime classes
  retain exact-owner specialization metadata without activating that watcher;
  the watcher is installed lazily only when a mutable source-defined class
  requires method and type-version invalidation.
  Direct calls to explicitly resolved compiler runtime names instead receive a
  trusted-runtime inline decision and retain their guard-free hot path; this
  exception does not apply to ordinary module-global Python functions.
  Profile and verify modes use the SOAC generator-factory
  vectorcall instead, so their resume bodies record the call-target and
  operation-shape evidence consumed by later typed planning. Generator
  expressions always keep the transformed factory: their iterator validation,
  closure bindings, and eager direct entries must not be replaced by the
  named-generator fast path. Eager scheduling and static generator-instance
  planning apply the same per-owner source-code and specialization-mode
  decision. In apply mode, a source-backed named generator retains its normal
  CPython public call instead of having a transformed wrapper and consumer loop
  recreated inside its caller. Both source-backed and transformed named
  generators retain their explicit direct-entry metadata; counter-recording
  modes, generated generators, and generator expressions remain eligible for
  their normal typed plans. Apply-mode modules containing source-backed named
  generators retain the ordinary Unicode-key dictionary created by
  `PyModule_FromDefAndSpec`; SOAC installs only its module metadata. This
  lets the native generator specialize global and builtin loads while preserving
  `function.__globals__ is module.__dict__`, global rebinding and deletion,
  and the generic fallback of indexed JIT accesses. Profile and verify modes
  retain their original indexed dictionaries. After trusted
  generator-resume inlining, typed
  planning
  can replace a nonescaping generator wrapper with explicit caller locals for
  preserved activation slots, initializing those locals from the original
  public-call arguments and runtime slot defaults. When a generator requires
  the transformed factory, it uses the SOAC generator-factory vectorcall path
  consistently so its preserved activation storage model remains explicit. The
  transformed factory resolves `make_generator_instance` through the existing
  module-owned runtime-name cache instead of importing and resolving the
  runtime helper on every generator creation. Generator, coroutine, and
  asynchronous-generator wrappers reuse the source function's actual immutable
  code object when its name, qualified name, and generator-kind flags agree;
  otherwise the existing synthetic code-template fallback preserves behavior.
  The first slice only lowers generators whose preserved state has no preserved cell
  slots and whose wrapper has no remaining observable uses after the resume body
  is inlined; synthetic alias/setup temps introduced while inlining trusted
  `next`/`send` paths are consumed with the wrapper.
  Exact generator-instance evidence also propagates through
  `iter(generator_function(...))`: the identity-iterator transfer retains the
  proven generator owner, instance origin, and resume function without
  promoting ambiguous aliases or ordinary iterator return values.
  The trusted `next(gen)` runtime-protocol inline uses that same erasability
  proof up front. If later observable uses such as `gen.throw(...)` remain, the
  protocol call stays generic instead of exposing wrapper-only fields before
  generator-state lowering can prove the wrapper is erasable.
  Immediate consumption by `list`, `set`, or `tuple` is not by itself such a
  proof for a source-backed generator. A native generator's suspended frame and
  owning `PyGenObject` remain observable through `frame.f_generator`, tracing
  and monitoring callbacks, `sys._current_frames()`, traceback construction,
  and close/finalization after an exceptional consumer exit. Those observers
  can become active after consumption has begun. Because the current runtime
  cannot materialize an equivalent native generator and suspended frame at a
  yield or safepoint, production apply mode does not attach generator-instance
  or consumer-fusion plans to source-backed named generators. The structured
  scalar-replacement tests exercise transformed generators whose wrapper and
  activation storage are compiler-owned; they do not establish that a native
  `PyGenObject` may be removed.
  Calls to builtin `list`, `set`, or `tuple` with a proven nonescaping generator
  instance can also carry an explicit typed builtin-implementation plan. That
  plan keeps the observed callable as the original builtin, but selects the
  visible `soac.runtime.list_from_iter`, `soac.runtime.set_from_iter`, or
  `soac.runtime.tuple_from_iter` helper as a fallback-free inline body so the
  generator consumer loop can be exposed to later typed rewrites without
  pretending that the builtin object itself is the helper target. The tuple
  helper keeps its own visible `iter`/`next` loop around a list accumulator before
  final tuple materialization, so tuple(genexpr) can reuse the same
  generator-consumer lowering rather than falling back to CPython iteration.
  The current slice only selects that body when the paired generator resume
  target stays within the bounded generator-inline budget, so large resume state
  machines keep the ordinary builtin path instead of flattening a
  disproportionate amount of control flow into the caller.
  Cloned generator-resume state remains owned by its particular inline
  instance: cloned activation slots and alias evidence cannot be merged across
  sibling generators, and an ordinary caller must not retain unresolved
  generator-preserved storage after rewriting.
  Calls to the resolved compiler-owned `resume_generator` runtime primitive
  can use its explicit five-borrowed-argument C ABI instead of Python
  vectorcall. The primitive returns an owned object, propagates the current
  Python exception on failure, and is never selected for an independently
  rebound Python name. The Python-facing generator-resume entry also uses
  CPython's native fastcall ABI for exactly five positional arguments; unusual
  argument shapes retain the original Python-facing validation and exception
  behavior.
  Direct-call and constructor-init inlining use deterministic projected
  cumulative block and instruction budgets. Generator-resume candidates are
  prioritized, and continuation cloning consumes the same remaining CFG
  budget instead of applying an independent limit to each call site. Late
  builtin consumers are admitted one at a time so the trusted iterator
  protocol and generator-resume decisions can be refreshed before processing
  another consumer. Optional late builtin and protocol inlining stops at the
  measured remaining budget while mandatory generator-resume cleanup
  continues. Virtual-field join trampolines consume the remaining block
  budget in deterministic edge order.
  Constructor-entry targets currently require all user arguments to be explicit
  because their type-stored metadata does not yet refresh `__init__` defaults.
- In apply/verify mode, JIT module planning consumes the cached pre-opt module
  plus raw profile evidence. The v3 planner makes the call-body decisions; typed
  planning applies those decisions to `InstrTyped`, and codegen mechanically
  emits the resulting typed shapes.
- Inline-winning direct calls assign fresh caller instruction ids to cloned
  callee operations and remap callee-owned operation plans, currently exact-list
  `GetItem`/`SetItem`, before typed access-plan annotation.
- Profile-selected inline body decisions stay local to the profiled caller by
  default. When a callee body is itself inlined, typed planning remaps
  mechanical callee sidecars and structurally static inline targets; only small
  nested profile-selected bodies are propagated transitively. Larger nested
  callees need a fresh call-body decision in their new context instead of being
  cloned recursively into every caller.
- JIT codegen has direct boolean lowering for typed direct-call guard tests, so
  those guards do not round-trip through Python truthiness. Replay/deopt support
  consumes the typed metadata selected by the plan rather than legacy codegen
  direct-call operations.

### Limitations / Soundness / Extensions

- Current limitations:
  - keywords are excluded
  - starred / unpacked args are excluded
  - method targets require a constant lowered attribute name and resolvable
    owner-type metadata
  - variadic target params are excluded
  - required keyword-only target params are excluded unless they have a
    default value
  - constructor-entry targets are declined when the selected argument plan
    needs default sentinels, because type metadata does not yet observe later
    `__init__.__defaults__` / `__kwdefaults__` mutation
  - constructor-entry direct allocation currently requires simple positional
    arguments and constructor metadata attached to a safe default allocation
    shape; custom `__new__`, abstract classes, custom metaclasses, non-generic
    allocation, keywords, and starred arguments stay on the generic class-call
    path
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

Direct method calls use `call_hot_targets` evidence for lowered calls whose
callable is a constant-name `GetAttr`, such as `record.copy()`. When the hot
target is a transformed function and the receiver plus positional arguments can
bind through a validated direct-entry argument plan, v3 records the selected
target, method name, and argument plan. During typed JIT planning, the JIT
resolves owner type metadata for that function and method name, predeclares the
owner type and owner-attribute callable relocation, and embeds typed method
guards into the call site.

Synthetic runtime protocol calls such as lowered `iter(x)` and `next(x)` use the
same evidence channel, but sample the receiver's resolved `__iter__` or
`__next__` function id instead of the builtin helper object itself. That keeps
the later direct-call decision tied to the Python method body that can actually
be inlined. When strict-module static targeting proves a local still carries a
trusted runtime owner type, typed planning can treat that one protocol callsite
as unconditional too; nested trusted constructors discovered through the direct
inline keep propagating that owner fact. The later virtual-object lowering pass
still decides separately whether the object itself can be erased.

Codegen then emits the guarded method path: evaluate the receiver, check the
receiver type/version, direct-call the selected function with the receiver as
implicit argument `0`, and fall back to the original generic method call on
guard miss. The path does not currently inline method bodies or specialize
keyword/starred method calls.


## Type Constructors

Constructor calls reuse `call_hot_targets` evidence, but the target identity is
the heap type's synthetic `__soac_constructor_entry__` function, distinct from
the `__init__` function id. These entry functions are present in the BlockPy
module, and eligible type metadata stores a JIT environment for the entry. If
the runtime type shape is not eligible for direct default allocation, SOAC
leaves the heap type metadata empty so profile evidence keeps the class call
generic.

The synthetic entry is now a normal JIT function with an implicit leading type
argument. Direct-call rewriting guards the callee as the exact heap type, loads
the entry metadata from the type, prepends the type object to the direct-entry
argument list, and falls back to the original generic class call on guard miss.
The entry body still lowers from a simple `constructor_call(cls, *args, **kwargs)`
IR shape, but JIT codegen recognizes that helper inside constructor entries and
emits `PyType_GenericAlloc`, an internal direct call to the selected `__init__`,
and `dp_jit_finish_constructor_init`. The per-call allocation-shape check is not
in the synthetic target; registration checks the realized `PyTypeObject` once
and does not attach constructor metadata for custom `__new__`, metaclasses,
abstract classes, or non-generic allocation.

Constructor initializer inlining is represented in `InstrTyped` metadata, not
rediscovered by codegen. After a constructor-entry direct call is inlined, the
typed pipeline can inline the selected `__init__` body into the hot constructor
path when the initializer can bind without closure storage, block parameters,
exception edges, or jump arguments and all returns are known `None`. The
remaining `constructor_call` is marked as
`InlinedConstructorEntryWithInlinedInitBody`, so JIT codegen emits
allocation-only `PyType_GenericAlloc`; the explicit inlined `SetAttr` and raise
paths preserve initializer side effects. Packed `*args` parameters are
materialized once into a typed temp before the inlined body rather than
substituted repeatedly at every `args` use.

Cross-module initializer body inlining currently requires every non-local load
in the callee to be representable in the caller module. Runtime names and
constants can be remapped, but ordinary globals from another module still need
an explicit external-module global representation; the inliner rejects those
instead of treating them as caller globals.

After typed direct-call inlining, constructor field scalarization can also plan a
private hot-path virtual constructor. The rewrite is only selected for a cloned
hot continuation where the constructed local no longer has an identity use: field
loads have been replaced with scalar locals, scalarized field stores can be
dropped, cleanup deletes can be removed, and exact-type direct-call guards on
the same virtual object can be proven redundant. The generic fallback path keeps
the original materialized object behavior. Ordinary profiled constructor
virtualization still requires indexed-field access plans before field loads and
stores are rewritten, but fully trusted static runtime constructors can consume
their bound constant fields directly in the fully-virtual path. That lets
  compiler-generated runtime wrappers such as `ClosureGenerator` lower their hot
  state to locals before indexed-field replay would otherwise make those fields
  visible.

Field-state planning retains bindings only for constructor instructions that
actually occur in the typed function. A real constructor can still expose
scalar fields before an explicit materialization boundary even when its object
cannot be removed. Once the object escapes through a global or nonlocal store,
its aliases and field facts are invalidated; subsequent accesses retain their
observable Python behavior.

## Operation Specializations

SOAC keeps operation-level specializations in
`crates/soac_jit/src/jit/operation_specializations.rs` instead of spreading guarded
fast paths through generic opcode lowering. The first implementations are
concrete rather than framework-driven: `GetItem` emits exact-list/exact-int and
exact-tuple/exact-int arms, `SetItem` emits an exact-list/exact-int arm, and all
three share generic fallback paths.

Exact-sequence item sites are selected by the existing v3 exact-list item plan
family; the family name is retained so typed sidecars, mechanical emission, and
inlined-site remapping remain shared. Each plan records its lowered
`GetItem`/`SetItem` source, access kind, exact-list or exact-tuple shape, the
corresponding exact-sequence/exact-compact-int in-bounds guard, and the original
item-access fallback. Tuple plans are valid for `GetItem` only. The JIT consumes
the selected typed item plan directly and emits either the list slot pointer or
the tuple's inline item array; legacy replayed item-shape maps are not consulted
by codegen. The generic `dp_jit_pyobject_getitem` and
`dp_jit_pyobject_setitem` helpers intentionally do not contain sequence fast
paths; unplanned item access goes through the CPython item APIs.

### Exact-List `GetItem`

### Counted Input

- Source input is `getitem_hot_shapes`.
- The initial shape tag records exact `list` receiver with an exact `int` index.
  Zero means "no specialized arm selected" and is ignored during profile
  replay.
- Verify mode also records `getitem_specialized_hit` and
  `getitem_specialized_fallback` scalar counters for selected arms.

### Codegen

- `GetItem` lowering routes through the operation-specialization module.
- Without counters or v3 exact-list item emissions, `GetItem` stays on the generic
  `PyObject_GetItem` path.
- With `getitem_hot_shapes` counters, profile/verify mode records the dispatch
  shape after evaluating operands. The validated exact-list item emission in
  the v3 plan selects the specialized arm directly.
- The first specialized arm guards:
  - the object is exactly `PyList_Type`
  - the index object is exactly `PyLong_Type`
  - the normalized integer index is in bounds
- On hit, codegen loads `PyListObject.ob_item[index]` directly. If typed lowering
  has already proven the index as a scalar `i64`, the direct arm consumes that
  scalar without first materializing a `PyLong`; otherwise it guards and unboxes
  the compact exact-int object input. The result path still INCREFs the borrowed
  list element and returns the owned result expected by the current legacy
  `GetItem` lowering path.
- On miss, codegen falls back to `PyObject_GetItem` through the generic helper
  path, which performs no exact-list rediscovery, and records
  `getitem_specialized_fallback` in verify mode.

### Limitations / Soundness / Extensions

- Current limitations:
  - exact-list reads select the exact-list/exact-int dispatch shape; exact-tuple
    reads use a separate shape and guard described below
  - guard misses use the generic fallback path for now; deopt is deliberately
    deferred until operation-specialization sites can guarantee a non-null
    runtime deopt table in all apply/verify entry paths
  - the current legacy path adapts the direct borrowed list element to owned
    immediately; typed-result consumers should eventually preserve borrowed
    ownership when their `ResultDemand` allows it
  - only compact exact integers are unboxed directly; non-compact exact integers
    fall back to the generic C API path
- Soundness boundary:
  - receiver and index exact types are guarded before direct layout reads
  - non-list objects, list subclasses, non-exact-int indices, and out-of-bounds
    indices fall back before direct list memory access
- Natural extensions:
  - add a separate list-compatible subclass arm guarded by mapping slot identity
  - add richer operation dispatch tags for string, dict, and custom slots
  - emit borrowed `EmitResult` for typed consumers instead of forcing an owned
    legacy result
  - add a matching exact-string getitem arm

### Exact-Tuple `GetItem`

### Counted Input

- Source input is `getitem_hot_shapes`.
- Shape tag `2` records an exact `tuple` receiver with an exact `int` index;
  the exact-list shape remains tag `1`, and zero still means no specialized
  dispatch shape.
- The v3 planner represents the tuple read in the existing exact-list item plan
  family using `ExactTupleExactInt` and the distinct
  `ExactTupleExactCompactIntInBounds` guard.
- Verify mode reuses `getitem_specialized_hit` and
  `getitem_specialized_fallback` counters.

### Codegen

- Exact tuple reads use the same explicit v3-plan annotation, direct and inlined
  instruction-ID remapping, compact-int unboxing, negative-index normalization,
  bounds checks, and cold generic fallback as exact-list reads.
- The receiver is guarded against the relocatable `PyTuple_Type` symbol; tuple
  subclasses do not enter the direct arm.
- The pinned CPython `PyTupleObject` contains a cached `ob_hash` between its
  variable-sized object header and `ob_item`. Codegen uses the corresponding
  `RawPyTupleObject` layout before directly loading
  `ob_item[normalized_index]`.
- A direct hit INCREFs the borrowed tuple element and returns the owned value
  required by ordinary `GetItem` lowering. When the index is already a proven
  scalar `i64`, the direct arm avoids materializing an intermediate `PyLong`.
- A guard miss uses the original `PyObject_GetItem` helper, preserving tuple
  subclass `__getitem__`, `__index__`, out-of-bounds exceptions, and ownership.

### Limitations / Soundness / Extensions

- Only exact tuples and compact exact-int indices use the direct arm; bools,
  `int` subclasses, non-compact integers, tuple subclasses, non-integer index
  objects, and invalid indices retain generic CPython semantics.
- Tuples are immutable: tuple `SetItem` is never selected as a v3 plan, even
  when tuple shape evidence occurs at a generic store site.
- Direct reads return an owned element rather than propagating borrowed result
  demand; extending borrowed typed consumption remains future work.

### Exact-List `SetItem`

### Counted Input

- Source input is `setitem_hot_shapes`.
- The shape tag records exact `list` receiver with an exact `int` index. The
  replacement value does not participate in the shape because list slots accept
  any Python object.
- Verify mode also records `setitem_specialized_hit` and
  `setitem_specialized_fallback` scalar counters for selected arms.

### Codegen

- `SetItem` lowering routes through the operation-specialization module.
- Without counters or v3 exact-list item emissions, `SetItem` stays on the generic
  `PyObject_SetItem` helper path.
- With `setitem_hot_shapes` counters, profile/verify mode records the dispatch
  shape after evaluating operands. The validated exact-list item emission in
  the v3 plan selects the specialized arm directly.
- The specialized arm guards:
  - the object is exactly `PyList_Type`
  - the index object is exactly `PyLong_Type`
  - the index is compact and normalizes in bounds
  - the replacement value is non-null
- On hit, codegen INCREFs the replacement, stores it directly to
  `PyListObject.ob_item[index]`, DECREFs the old slot value, and returns owned
  `None`.
- On miss, codegen falls back to `PyObject_SetItem` through the generic helper
  path, which performs no exact-list rediscovery, and records
  `setitem_specialized_fallback` in verify mode.

### Limitations / Soundness / Extensions

- Current limitations:
  - only the exact-list/exact-int dispatch shape is recorded
  - guard misses use the generic fallback path for now, matching `GetItem`
  - only compact exact integers are unboxed directly; non-compact exact integers
    fall back to the generic C API path
- Soundness boundary:
  - receiver and index exact types are guarded before direct layout writes
  - non-list objects, list subclasses, non-exact-int indices, out-of-bounds
    indices, and null replacements fall back before direct list memory access
- Natural extensions:
  - add a separate list-compatible subclass arm guarded by assignment slot
    identity
  - preserve effect-only result demand so successful stores do not materialize
    owned `None` when the value is unused

## Exact-Int Operators

### Counted Input

- Source input is `operator_hot_shapes`.
- Candidate operators are the v3-supported binary `BinOp` nodes:
  - `Add`, `Sub`, `Mul`
  - `And`, `Or`, `Xor`
  - `Eq`, `Ne`, `Lt`, `Le`, `Gt`, `Ge`
- Candidate detection is in
  `instrument_bb_module_with_call_target_counters` in
  `crates/soac_lowering/src/passes/trace/mod.rs`.
- Shapes are packed exact-type tags defined in
  `crates/soac_opt/src/operator_specialization.rs`.
- Exact type tags currently include `ExactTypeTag::Int` and
  `ExactTypeTag::Str`.
- Unary operators, division, modulo, shifts, power, identity/contains tests,
  matmul, and in-place variants are not counted for this specialization and use
  generic lowering unless v3 grows explicit alternatives for them.

### Codegen

- v3 planning emits mechanical hot regions plus local fallback regions.
- The hot path guards that both operands are compact exact `PyLong` objects,
  unboxes them as machine `i64`, and then emits the selected operation:
  - checked machine add/sub/mul with PythonLong materialization when the result
    is demanded as an object
  - machine bitwise and/or/xor with PythonLong materialization when needed
  - direct integer comparisons with branch or Python bool materialization based
    on result demand
- Operand inputs may be locals/cells, module constants where applicable, or
  profiled indexed module-global loads. Hot indexed-global inputs borrow
  directly from the guarded module-dict slot; local fallbacks reload the global
  with the normal owned global-load path before running generic Python
  operation lowering.
- On type/compactness/overflow miss, codegen runs the local generic fallback
  region emitted from the original Python operation shape. When the fallback is
  replay-safe, guard-miss lowering can instead target a cold
  `dp_jit_deopt_resume` continuation.

### Limitations / Soundness / Extensions

- Current limitations:
  - only exact `int`/`int`
  - no mixed-type shapes
  - unsupported operator kinds always use generic lowering
- Soundness boundary:
  - compact-long machine-code specialization guards exact runtime `PyLong`
    layout and compact representation before direct memory access
  - unsupported or mismatched shapes either deopt to the generic
    continuation or fall back to generic lowering
- Natural extensions:
  - `float`, `str`, `bytes`, `bool`, and mixed-type shapes
  - richer shape encodings
  - specialization for more operators that are currently generic


## Exact Comparisons

### Counted Input

- There is no separate comparison counter kind.
- Comparisons reuse the binary operator machinery, because comparison
  operations are represented in the `BinOpKind` space for this stage.
- The same `operator_hot_shapes` counter input drives them.

### Codegen

- Comparison specialization is emitted from v3 operator regions.
- If the profiled shape is exact `int`/`int`, comparisons such as
  `Eq`, `Ne`, `Lt`, `Le`, `Gt`, and `Ge` guard compact exact `PyLong` layout
  and emit a direct integer comparison instead of generic
  `PyObject_RichCompare` lowering.
- If the profiled shape is exact `str`/`str`, the hot path guards both operands
  against exact `PyUnicode_Type`. `I32Bool01` compare-to-bool lowering first
  handles pointer-equal operands and compact ASCII one-character operands
  directly. Longer compact ASCII operands call a SOAC runtime helper that
  performs lexicographic byte comparison before the general exact-Unicode
  fallback to `PyUnicode_Compare`. This avoids allocation of the intermediate
  Python bool and the follow-on
  truthiness helper; when an object result is demanded, the scalar comparison
  result is materialized as the Python bool singleton. Operand inputs may be
  locals/cells, module string constants, or profiled indexed module-global
  loads. The hot indexed-global input borrows directly from the guarded
  module-dict slot; the local fallback reloads the global with the normal owned
  global-load path before running generic Python comparison.
- When the comparison is consumed by `I32Bool01` demand, such as an
  `if` condition, the profiled exact-int path guards compact exact `PyLong`
  layout, unboxes both operands, emits a direct integer comparison, and
  branches on the scalar result without calling `PyObject_RichCompare` or
  `dp_jit_is_true`.
- When the comparison is consumed as a Python object, compact exact `PyLong`
  operands still use a direct integer comparison and then materialize the
  boolean singleton without calling `PyObject_RichCompare`.
- On guard miss, comparison specialization uses the same deopt-or-fallback
  behavior as exact-int binary operators.
- A guard miss only uses `dp_jit_deopt_resume` when the continuation is
  replay-safe and every local that can be read by the reachable continuation
  tail is definitely materialized by the planned resume entry. If not, codegen
  keeps the local generic fallback path so unbound/null block-param state cannot
  be mis-reconstructed by the deopt interpreter.

### Limitations / Soundness / Extensions

- Current limitations:
  - exact `int`/`int`, and exact `str`/`str` for branch-context
    compare-to-bool plans or return-shaped Python bool results
  - non-compact `PyLong` values guard-miss instead of using bigint-specific
    fast-path code
  - no bytes/tuple/list comparison specialization
- Soundness boundary:
  - exact-shape guarded, otherwise deopt to the generic continuation or
    generic fallback
  - exact-string comparisons require exact `PyUnicode_Type`, so subclasses and
    non-string objects run the generic fallback before Python-visible dispatch
- Natural extensions:
  - exact `str` comparisons in more expression shapes beyond v3 return-shaped
    regions
  - exact `float` comparisons
  - container comparisons where CPython semantics are stable enough to
    encode directly


## Static Runtime Builtin Primitives

### Counted Input

- There is no profile counter for the first version.
- Candidate calls are recognized statically when name binding has already
  proven the callee load is a runtime-name constant for `ord`, `chr`, `len`, or
  `iter`.
- Global-name loads are not candidates at codegen time. That avoids treating
  a shadowable module global as a builtin primitive.

### Codegen

- `ord(x)` can emit a direct call to the `soac_jit_runtime`
  `soac_runtime_builtin_ord_i64` primitive. The primitive accepts the argument
  as a borrowed `PyObject*`, performs CPython-compatible validation internally,
  sets `PyThreadState.current_exception` on failure, and returns an `i64`.
- `len(x)` uses the same scalar-returning runtime primitive shape via
  `soac_runtime_builtin_len_i64`: borrowed `PyObject*` input, `i64` result, and
  `PyThreadState.current_exception` error reporting.
- `iter(x)` can emit a direct call to `soac_runtime_builtin_iter_object`, which
  accepts a borrowed `PyObject*` and returns the owned result of
  `PyObject_GetIter(x)`.
- If the consumer demands a Python object, the `i64` result is boxed through
  the existing `emit_to_python_long` coercion path. If the consumer demands an
  `i64`, the scalar value is used directly.
- `chr(ord(x))` can emit `ord_i64(x)` followed by
  `soac_runtime_builtin_chr_i64(tstate, codepoint)`, avoiding a temporary
  `PyLong` between the two builtins.
- `chr(<i64 module constant>)` can pass the constant directly to
  `soac_runtime_builtin_chr_i64` instead of first materializing a temporary
  `PyLong`.
- When `Add`, `Sub`, or `Mul` is emitted to satisfy an `i64`/index demand,
  codegen uses Cranelift signed overflow-checking arithmetic and branches to
  `OverflowError` on overflow. **BEHAVIOR_CHANGE:** optimized SOAC raises on
  overflow in this scalar path instead of falling back to CPython's
  arbitrary-precision `int`.
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
- constructor calls with keywords, starred arguments, unresolved owner metadata,
  or argument plans that need default refresh
- most non-`int` operator shapes

Those are the main expansion areas if we want the specialization system
to cover more of the remaining Python/runtime overhead.
