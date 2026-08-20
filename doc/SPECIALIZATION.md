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
callable calls. User-module constructors additionally need their synthetic
constructor identity registered immediately after their class is created:
module-level calls can run before the final owner-type registration sweep.
Early registration is only eligibility evidence, not proof that eager apply
planning has selected or emitted a direct constructor edge. The optimization
currently assumes, but does not yet
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
Exact-int region inputs must also remain available in the final typed source
function after rewrite and cleanup. Ordinary typed local liveness includes
the `RegionInputSource::FunctionParam` named locals and `IndexedField`
`LocalName` receivers embedded in both hot and fallback regions of selected
exact-int branch and return sidecars. These are real semantic reads even when
they do not appear as ordinary expression loads; preserving and transporting
them across CFG edges keeps selected optimized regions valid without pruning
the optimization, fabricating values, or changing Python behavior.

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
  object result is demanded. Profiled exact-float arithmetic trees containing at
  least two eligible operations use separate, source-keyed v3 function plans,
  validated typed-root sidecars, ordered exact-type guards, unboxed `f64`
  intermediate operations, and one final Python float materialization. Store
  RHS lowering can consume return-shaped
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

Profile-mode evidence is keyed to the original lowered source function and
semantic `InstrId`. Counter definitions are established before typed JIT
planning, so the `profile` pass must preserve those original instruction IDs
and its original call graph: typed optimization rewrites, direct-call
inlining, and hot-continuation cloning are reserved for `verify`/`apply`
replay. In particular, cloning a continuation during profiling would
renumber its hot instructions without moving their source-keyed counter
definitions, leaving executed operators, calls, attributes, item accesses,
and branches incorrectly unobserved.

Skipping optimization rewrites in profile mode does not skip the ordinary
typed runtime preparation, counter instrumentation, ownership/value analysis,
generator and closure handling, or generic JIT planning. Verify and apply
still consume the collected source-keyed evidence and run their existing
validated typed rewrites, guards, and fallbacks. The mode boundary is based
on explicit `SOAC_OPT_MODE=profile`, not merely on whether a counter dump
exists.
Because profile mode retains original generator `CellRef` operations and
preserved-cell state, typed and untyped module constant collection also
preserve logical and storage names for owned cells, closure freevars, and
preserved slots. Preserved-cell unbound errors use the actual Python variable
name rather than an internal codegen label.

Profile evidence must include every counter-producing transformed module in
the process, not only modules whose CPython `m_clear` callback happens to run
before interpreter shutdown. The process `CompileSession` retains transformed
`SharedModuleState` objects for cross-module planning; a live compiler-owned
`soac.runtime` module can therefore remain un-cleared even when a benchmark
main module has already appended its own profile frame. Omitting that runtime
frame makes valid strict cross-module apply targets appear absent.

The shutdown contract is an early-registered private extension `atexit`
callback that snapshots the retained compile-session states and flushes each
active module's final counters. The explicit public Rust
`CompileSession::flush_counter_dump_outputs` API coordinates that pass;
existing module-clear flushing remains available for earlier cleanup. Each
module-state/output-path pair must be serialized at most once across both
paths, including reentrant callbacks, without holding a registry or state
mutex while building a counter record that may invoke Python. Preserve
`profile.bin` versus `verify.bin` mode separation, skip dumps in apply/none
modes, retain strict source-hash and target validation, and do not expose a
new public Python callback or module-global cache. Registering the shutdown
callback during `_soac_ext` initialization lets later user `atexit` work run
first and observers registered before importing SOAC inspect the completed
dump afterward. Exact-once is process-local: if ten independent benchmark
worker subprocesses append to one shared profile path, one main frame plus
one runtime frame from each process correctly produces twenty total frames.

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
  - Records packed exact operand-type tags: `Int = 1`, `Str = 2`, and the
    append-only `Float = 3`; an exact-float/exact-float pair is packed as
    `3 | (3 << 8) = 771`.
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
  - Count whether a guarded instance-field specialization accessed an
    inline split-dictionary value or exact object slot directly, or fell back
    to the original CPython attribute operation.

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

### Immutable Singleton Truthiness

Existing branch lowering still calls the existing exported
`dp_jit_is_true`; its private production hook now mirrors pinned CPython's
immutable singleton identities directly: pointer-identical `True` returns
**1**, while `False` and `None` return **0**. The existing null-pointer
`RuntimeError` check runs before the classifier. Every other object takes
the unchanged `PyObject_IsTrue` path, preserving `__bool__` / `__len__`
precedence, custom callbacks, subclass behavior, mutation, errors, owned
references, finalizers, and original branch-outcome counters.

The pure classifier is private and changes exactly one existing runtime
source file; there is no new exported helper, public API, mutable global,
typed IR operation, or intended generated-native-body change. There was
**no existing CPython behavior mismatch**: real stock/transformed
**Profile → Verify → Apply** truthiness integration passes both before and
after implementation. The actual exported-hook structured specialization
decision turns RED-to-GREEN; final Rust suites pass JIT **573 / 573**,
optimizer **213 / 213**, and typed IR **54 / 54**, with transformed
compatibility **16 / 16**, scoped formatting checks, and JIT Cargo
`--tests` check green. Candidate release smoke and all **80 / 120**
normally sampled / repeated workers retain every source-function ID,
native byte / block, and hidden trampoline; fixed-eight native remains
**23,163,480 bytes / 1,524,480 blocks**.

Clean three-round richards improves **27.146548 → 25.707173 ms**,
**1.055991x [1.026761, 1.076974]** raw /
**1.041139x [1.004715, 1.062791]** stock-adjusted; all three rounds win,
although five candidate outliers remain disclosed. Comprehensions improves
raw but is **0.954158x [0.935538, 0.974371]** stock-adjusted under
approximately **6.5%** faster stock; do not claim a comprehensions gain.
Matched zero-loss same-source richards profiles show C truth-call / PLT
leaves **6.072336% → 0**, while the necessary existing helper rises
**0.714157% → 4.314992%**: combined disjoint truth leaves decrease only
**6.786493% → 4.314992%**, or **2.471501 percentage points net**.
The optimization is **LANDED CANDIDATE / RETAIN** for measured richards
improvement with unchanged public API and native bodies. The authoritative
full **`just test-all` gate exits zero**: **1,233 transformed Python
nodeids / 96 isolated batches / eight workers / 96 passed / zero
failures**. Rust JIT passes **573**, optimizer **213**, typed IR **54**,
lowering **371**, and PyO3 **8**. Runtime build takes **1.679 seconds**,
Cargo **64.701 seconds**, pytest **80.356 inner / 80.374 outer seconds**,
and total test phase **145.087 seconds**; the new truthiness integration
passes in **2.85 seconds**, while the known 28-node counter-dump batch
takes **79.97 seconds**. See
`work/logs/immutable-singleton-truthiness-test-all.log`. Full-suite stock
**1.10x** remains unmet / unmeasured.


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
- Eager class-owned fields additionally consume existing `field_access`
  `generic_getattr` / `generic_setattr` Profile branch counts. A site requires
  at least eight generic observations; no new counter-dump schema is added.
  Ordinary dictionary fields still require matching owner-specific
  `type_keys`, while literal `__slots__` fields do not fabricate or require
  split-dictionary type records.
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
- A region that borrows an indexed-field value remains eligible only while its
  referenced attribute instruction still exists in the final typed function
  with a usable owner/type-version/index guard. Typed rewrites can remove,
  replace, or fail to attach that guarded `GetAttrTyped` instruction even when
  the enclosing comparison survives. Planning must then discard the dependent
  exact-int or exact-string sidecar and preserve the original generic operation;
  codegen must not recreate a missing guard or dereference an unguarded field.
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

### Eager Class-Owned Fields

Normal eager module compilation runs before its classes exist, so resolving
an owner type immediately would discard otherwise valid profiled methods. The
optimizer instead derives same-module owner provenance from the actual lowered
class namespace, its literal `__qualname__`, `MakeFunctionWithClosure` method
identity, the first local receiver, and optional literal `__slots__`.
Single-definition immutable local string constants are propagated without
name-based heuristics. Staticmethods/decorated methods, dynamic local classes,
inherited slots, unresolved owners, and cross-module operations are rejected.

The explicit public APIs are
`soac_opt::pipeline_v3::late_bound_owner_field_site_catalog`,
`soac_ir_typed::plan_v3::{LateBoundOwnerFieldSpecializationPlan,
LateBoundOwnerFieldStorage}`, and the crate-root
`soac_ir_typed::TypedLateBoundOwnerFieldPlan`. Validated plans distinguish
`SplitDict { expected_index }` from `ObjectSlot`, preserve original
function/instruction identity across same-module inlining, and assign stable,
dense module-local cell indices. Existing resolved indexed-field plans retain
precedence so previously guarded scalar regions remain valid.

`SharedModuleState` owns the stable cell array and weak-reference objects;
the real `FunctionEnvAbiHeader` contains one pointer to that array. Compiled
code reaches cells by index through the active function environment rather
than embedding process-local owner addresses or duplicating the ABI layout,
preserving precompiled-code portability. Class-created publication validates
same-module function provenance, exact generic attribute hooks, the precise
object-slot descriptor or absence of conflicting MRO bindings, and a nonzero
assigned type version. Static builtin bases in the pinned CPython can have a
null `tp_dict`, so MRO dictionaries are read through owned
`PyType_GetDict(base)` references. Depending on verified pinned CPython
layouts is intentional; because PyO3's weakref declaration is `repr(Rust)`,
generated code uses an explicit minimal C-layout `RawPyWeakRefForJit` prefix
instead. No user callbacks or user-defined descriptor invocations are used to
prime a cell.

**Guard lifetime:** every access remains valid only while the owner weakref
still points to the receiver's exact live class and its current nonzero
`tp_version_tag` equals the published version. Dead CPython weakrefs contain
`Py_None` and fail exact-owner identity. Slot loads additionally require a
non-null current slot. Split fields additionally require an unmaterialized
dictionary, valid inline storage, and a current owner `ht_cached_keys` table
of split kind whose expected index is below both inline capacity and
`dk_nentries`; the current shared-key entry must be the exact expected
interned attribute-name object. Loads also require a present value. Because
late-bound classes are intentionally not primed, an absent or mismatched
shared key falls back to the original generic store so CPython can register a
constructor's first insertion before later direct accesses. Type/property/hook
mutations, owner death or redefinition,
slot deletion, dictionary materialization/promotion, and invalid layouts
immediately take the original generic `PyObject_GetAttr` /
`PyObject_SetAttr` path. There is no timer, process-global guard table, or
strong reference extending owner lifetime.

Slot stores own the replacement before publishing it and only then decrement
the old value, so a reentrant finalizer observes the new field. Dictionary
stores reuse the existing guarded inline-values store and insertion-order
handling. Verify mode preserves original-source indexed-hit/fallback counters,
and the strengthened Profile→Verify→Apply regression covers slot/split
reads and writes, subclasses, deleted/reinserted fields, materialized/promoted
dictionaries, property replacement, finalizer ordering, weak class lifetime,
and unseeded constructor first insertion with `__static_attributes__ = ()`.
The live shared-key guard also restores actual release Apply correctness for
`deltablue` and `richards`; their single-shot cold-JIT timings are not
steady-state throughput evidence. Normally sampled fixed-eight and repeated
five-workload median geometric improvements are **1.040x** and **1.066x**;
`float` improves but `comprehensions` and `richards` regress, generated native
code grows **2.757%**, and existing scalar-region invalidations remain
unchanged. The complete `just test-all` correctness gate passes **1,217 Python
nodeids across 84 batches / 8 workers**, plus **553 JIT**, **53 typed-IR**,
**371 lowering**, **207 optimizer**, and **8 PyO3** tests; see
`work/logs/late-owner-fields-test-all.log`. The measured subset remains below
stock CPython, and the full-pyperformance **1.10x** target remains unmet.

### Polymorphic Inherited Class-Owned Fields

An inherited method's lexical class does not determine the concrete
receiver's split-dictionary layout. The optimizer therefore catalogs only
literal, same-module, transitively inherited split-dictionary classes and
self-written concrete-owner / attribute anchors. Existing Profile evidence
must identify an exact concrete owner and its observed attribute index with
at least **eight observations**; object slots, unknown/dynamic owners,
cross-module provenance, and unsupported descriptors or hooks are excluded.
Plans remain explicit and validated through the existing public
`TypedAttrAccessPlan` enum, which adds the
**`PolymorphicLateBoundOwnerFields`** variant.

Polymorphic groups reuse deterministic owner-plus-attribute cells and admit
at most **eight exact owners** per source. Actual owner-specific profiled
layouts are filtered before capping; when the lexical owner is also
profiled, it is explicitly reserved after at most **seven profiled
descendants**. An unprofiled class cannot displace a real observed owner.
The typed validator permits distinct exact owners for the same field source
but rejects duplicate owners, mixed attributes/storage, and invalid cells.
Polymorphic groups cannot be reused as exact-single-owner scalar guards.

Inherited-owner publication scans the complete concrete receiver MRO for the
original registered base function, including subclasses whose `__init__`
overrides or delegates. It publishes existing descendant cells without
expanding the global function-owner watcher/weakref registry: the declaring
base is already registered and pinned CPython `PyType_Modified` recursively
invalidates descendant type versions. Every guarded case still requires a
live weak owner, exact receiver-type identity, unchanged nonzero owner type
version, safe generic hooks/descriptors, and the correct live split-key
table, interned attribute name, field index, inline capacity, and value
presence. The receiver is evaluated once; misses take the complete original
generic getter or setter and preserve source counters and reference counts.

The genuine transformed Profile→Verify→Apply regression preserves **32
existing lexical-owner hits** while converting **128 descendant fallbacks**
into hits, for **160 indexed hits across five exact owners per field
source**. It also proves two inherited Delta descendants with unequal
indices, both reads and writes, property/hook/base/MRO mutation,
deleted/promoted/growing dictionaries, finalizer ordering, unsupported
slots, and existing scalar optimizations. Complete affected Rust libraries
pass **54 typed-IR + 210 optimizer + 561 JIT = 825 / 825 tests**; full JIT
test targets pass **561 / 561**, package-scoped formatting/checks pass, and
grouped transformed guardrails pass **78 / 78 tests in 29.41 seconds**
(**7 deselected across 10 files**).

Normally sampled fixed-eight stock score improves **0.509970x → 0.520917x**,
with robust previous-SOAC improvement **1.015700x**. Three affected/control
rounds confirm `deltablue` median **4.171529 → 3.750207 ms (1.112346x)**,
clustered 95% interval **1.097582–1.152915x**, and `richards` median
**39.759100 → 33.958922 ms (1.170800x)**, interval
**1.135034–1.219269x**; `chaos` / `comprehensions` controls are neutral.
Robust subset improvement is **1.067058x**, or **1.071040x** after
paired-stock adjustment. This benefit has a material code-size cost:
generated native code grows **23,359,400 → 24,353,560 bytes (+4.256%)**,
including **+7.835%** in `deltablue` and **+19.669%** in `richards`; typed
coverage remains **3,069 blocks / 218 functions**.

Matched zero-loss `deltablue` native profiles contain **434 baseline / 390
candidate samples across 400 replay loops**. Inclusive `PyObject_GetAttr`
falls **20.736% → 16.664%**, `GenericGetAttrWithDict` **14.288% → 9.999%**,
and inherited generic input/output ancestry falls **2.766% / 0.922% →
zero**, while `choose_method` falls **1.614% → 0.256%**. Matched `richards`
profiles contain **599 baseline / 522 candidate samples across 70 loops**;
`PyObject_GetAttr` falls **27.714% → 18.392%**,
`GenericGetAttrWithDict` **23.707% → 16.857%**, and inherited generic
holding/waiting ancestry **4.341% / 1.002% → zero**. Profile ancestry
overlaps; cold compiler frames remain present and attached replay timings
are diagnostic rather than headline benchmark results. The change is
**retained** and the authoritative `just test-all` gate passes **1,221
Python nodeids across 88 / 88 isolated file batches and eight workers**,
plus workspace Rust JIT **561**, typed IR **54**, optimizer **210**,
lowering **371**, and PyO3 **8**; see
`work/logs/inherited-owner-test-all.log`. Cargo tests take **72.359
seconds**, inner / outer pytest **93.990 / 94.003 seconds**, and the full
test phase **166.374 seconds**; the existing counter-dump batch takes
**93.06 seconds**. The full-suite stock **1.10x** objective remains unmet.

### Hot Non-Self Instance Fields

Existing same-module `SplitDict` constructor cells can also specialize hot
ordinary receiver fields outside a method's first `self` parameter.
Existing Profile `generic_getattr` / getter evidence must record at least
**eight observations**. A unique concrete same-module owner supports
existing guarded loads or stores. A polymorphic load is additionally
eligible only when its complete profile identifies **two through five
distinct exact same-module owners**, every owner has an existing matching
constructor anchor, and all owners prove the same attribute and split-field
index. Foreign owners, missing anchors, mixed indices, owner counts above
five, and polymorphic stores remain on the original generic operation.

The optimizer reuses the minimum existing matching constructor-cell index
for the exact owner, attribute, `SplitDict` storage index, original
instruction source, and load/store access kind. Existing scalar, `self`,
polymorphic inherited, and unique-owner decisions run first and retain
precedence. At most **eight distinct additional non-self source sites per
function** are selected in deterministic hottest-first order; one complete
polymorphic owner group counts as one site and never replaces an existing
unique-owner store. No new owner cell, publication path, runtime helper,
public API, or global state is introduced.

Each selected access mechanically reuses the existing exact weak-owner,
current nonzero type-version, safe hook/MRO/descriptor, shared-key identity,
inline capacity, and field-index/value guards. Receiver evaluation and
ordinary generic fallback remain singular and unchanged. Ambiguous owners,
cross-module ownership, missing anchors, cold fields, slots, deleted or
materialized/promoted dictionaries, class replacement, subclass callbacks,
descriptors, and reentrant finalizers remain guarded or generic as before.
Loads and stores through top-level locals, unrelated methods, compound
receivers, and nested eager comprehensions are admitted only with the same
validated exact owner and existing cell. For a complete same-index
polymorphic load, every existing weak-owner/type-version guard remains
independent, then successful owners converge through one matched-owner
Cranelift block parameter into **one shared live split-key guard and one
inline-values field probe** before the original generic fallback.

For the original unique-owner-only implementation, a genuine structured
production-path regression turns RED-to-GREEN,
covering exact source/index/access, unique-owner minimum-cell reuse,
cross-module ambiguity, the eight-entry cap, and existing scalar/self
precedence. The independent transformed Profile→Verify→Apply integration
also turns **1 failed / 4.43 seconds → 1 passed / 4.50 seconds**. Full
optimizer and JIT libraries pass **211 / 211** and **563 / 563**;
transformed compatibility guardrails pass **26 / 26 in 34.30 seconds**;
combined optimizer/JIT test-target checking and scoped optimizer formatting
check pass. Production changes exactly one existing optimizer file.
Normally sampled fixed-eight paired stock score improves
**0.5482172650503208x → 0.5594598880789836x**; previous-SOAC arithmetic
improvement is **1.0148678728309706x**, robust **1.00314x**, or
**1.01321x** stock-adjusted. Matched 60-versus-60 three-round robust subset
improvement is **1.03730284x**, or **1.04940010x** stock-adjusted: `chaos`
**1.056477x (95% 1.02991–1.08743x)**, `deltablue`
**1.057278x (1.01003–1.11517x)**, and `richards`
**1.072304x (1.03668–1.12246x)**. There is also a reproducible
`comprehensions` regression **0.966618x (0.94899–0.99361x)** in every
round, approximately **3.34%**; its cause is unproven. Generated native
code grows **24,353,560 → 25,033,800 bytes (+2.7932%)**; existing
virtualization is preserved.

Matched zero-loss `deltablue` profiles contain **365 → 354 samples** and
reduce generic lookup ancestry **17.534% / 4.658% → 9.603% / 2.260%**
(inclusive / self); matched `richards` profiles contain **526 → 469
samples** and reduce generic ancestry **18.632% / 6.466% → 15.989% /
3.839%**. Nested shares overlap and lazy compilation remains present. The
available `comprehensions` profile compares an older direct-generator
revision rather than the exact parent, and different compile/GC shares make
its faster diagnostic replay unsuitable for explaining or dismissing the
actual regression. The optimization is **retained**, and the authoritative
full `just test-all` gate passes **1,222 Python nodeids across 89 / 89
isolated file batches and eight workers**, plus JIT **563**, optimizer
**211**, typed IR **54**, lowering **371**, and PyO3 **8**; see
`work/logs/hot-nonself-fields-test-all.log`. Cargo tests take **66.743
seconds**, inner / outer pytest **94.592 / 94.607 seconds**, and the full
test phase **161.366 seconds**; the known counter-dump batch takes
**93.80 seconds**. The full-suite stock **1.10x** goal remains unmet.

The subsequent layout-uniform polymorphic extension changes exactly two
existing production files, `crates/soac_opt/src/pipeline_v3.rs` and
`crates/soac_jit/src/jit/mod.rs`; the existing
`crates/soac_jit/src/jit/test.rs` addition is strictly `#[cfg(test)]`-only.
Three genuine independent regressions turn RED-to-GREEN: the real
whole-production optimizer changes **0 → 5** independently anchored
exact-owner plans (**1 passed / 213 filtered / 0.08 seconds**); the real
emitted Cranelift CFG changes **5 → 1** live `ht_cached_keys` probes
(**1 passed / 573 filtered / 0.10 seconds runtime / 0.49 seconds total**);
and frozen actual stock/Profile→Verify→Apply transformed execution passes
**1 test in 2.95 seconds** while verifying original-source indexed hits
for all five owners, including an unrelated `Packet`. Existing
unique/inherited decisions, mixed layouts, more than five owners, foreign
owners, slots, hooks, MRO/property/dictionary mutations, finalizers, and
untouched fallback remain covered; no baseline CPython-visible behavior
bug is claimed.

Post-format optimizer, JIT, and typed-IR libraries pass **214 / 214**,
**574 / 574**, and **54 / 54** tests. Broad transformed compatibility
passes **16 / 16 in 37.28 seconds**, combined optimizer/JIT test-target
checking passes in **3.69 seconds**, and package-scoped formatting and
format checks pass. The optimizer's **13.81-second rebuild**, JIT's
**26.00-second compile**, and second debug-extension rebuild of
**21.57 seconds** are workflow-only build overhead, not benchmark
measurements. The dedicated new polymorphic fixture proves index **0**;
a dedicated nonzero-index fixture remains pending. Changed real
`Task.qpkt` and `Task.addPacket` / `Task.release` bodies expose `ident`
index **1** and `priority` index **2**, but this source-body evidence is
not per-site counter proof and indices **3 / 7** remain unaudited.

All eight release-smoke workloads pass with identical transformed-source
identities, **397 total JIT source rows, including adapters / 204 direct
function bodies**, **2,866 typed blocks / 204 functions**, and
**36,500 hidden trampoline bytes**; emitted native bytes change
**2,238,468 → 2,238,412**.
Normally sampled fixed-eight stock score is **0.6672361371916246x**
versus retained **0.6345791409139968x**, with official previous-SOAC
**1.076213366589749x**; all **80 actual Apply PIDs / 3,970 total JIT
source rows, including adapters / 2,040 direct function bodies** retain
exact sources and **365,000 hidden bytes**, while native bodies shrink
**23,163,480 → 23,159,960 bytes**. Single-round richards raw
**1.08277x** has an inconclusive paired-stock interval
**0.97808–1.06599x** and is not alone a definitive win.

Clean three-round comparison **`comparison-20260819-185725-iJQ74K`**
establishes repeated richards improvement **25.707173 → 23.625606 ms**:
raw **1.088106x (95% 1.069411–1.117355x)** and paired-stock-adjusted
**1.070336x (1.043181–1.107330x)**, with all three independent raw
(**1.108967x / 1.068209x / 1.081729x**) and paired
(**1.103835x / 1.039361x / 1.076220x**) rounds improving. Deltablue is
**NEUTRAL**, raw **1.007590x (0.989368–1.033818x)** / paired
**0.974161x (0.946963–1.002748x)**; unchanged-code chaos movement is an
environmental artifact, and comprehensions is neutral. Official fixed-four
stock / previous-SOAC scores are **0.5139251222980681x /
1.0654218950545014x**. All **120 actual Apply PIDs / 10,650 total JIT
source rows, including adapters / 5,490 direct function bodies** retain
exact sources, **2,265 typed blocks / 183 functions**, zero errors, and
**746,520 hidden bytes**; native code decreases
**54,697,320 → 54,686,760 bytes (-10,560)**, while machine blocks grow
**3,594,960 → 3,596,430 (+1,470)**. Six deltablue bodies shrink
**59,880 bytes**, ten richards bodies grow **49,320 bytes**, and chaos /
comprehensions bodies remain identical.

Matched zero-loss richards causal profiles contain **255 retained / 244
candidate samples** for the same worker, **100 loops / 99 Hz / disabled
block maps**. The disjoint four-symbol generic-attribute **leaf self**
total falls **9.803255% → 4.099016% (-5.704239 percentage points)**:
`_PyObject_TryGetInstanceAttribute` **5.098173% → 1.229705%**,
`_PyObject_GenericGetAttrWithDict` **3.528812% → 2.869311%**,
`PyObject_GetAttr` **0.784180% → 0%**, and its PLT **0.392090% → 0%**.
Disjoint source partitions include `Richards.run`
**4.706082% → 1.229705%**, `Task.runTask`
**1.568361% → 0.819803%**, and `Task.release`
**0.784180% → 0%**; `Task.qpkt` increases
**0.392090% → 1.229705%**, while guard work moves into JIT bodies.
Separate overlapping non-GetMethod generic ancestry changes
**14.900427% → 9.017836% (-5.882591 percentage points)**; distinct
`_PyObject_GetMethod` inclusive ancestry **7.841804% → 9.016836%**
remains a separate bottleneck. These nested ancestry shares must not be
added to disjoint leaf totals.

The authoritative full **`just test-all` gate passes**, with evidence in
**`work/logs/uniform-polymorphic-nonself-test-all.log`**: **1,234
transformed Python nodeids / 97 isolated file batches / 8 workers / 97
PASS / 0 failures**, plus Rust JIT **574**, optimizer **214**, typed IR
**54**, lowering **371**, and PyO3 **8**. Cargo compilation takes
**51.34 seconds**, Cargo tests **68.796 seconds**, inner / outer pytest
**74.030 / 74.043 seconds**, and total test phase **142.853 seconds**.
The new regression passes in **2.52 seconds**; the existing **28-node
counter-dump batch dominates at 73.32 seconds**. The optimization is a
fully validated **RETAIN / LANDING CANDIDATE**; the full-suite stock
**1.10x** objective remains unmet.

### Late-Bound Guarded Scalar Regions

The eager owner-field change alone does not recover selected exact-int scalar
regions whose receivers are arbitrary locals: top-level `chain_test` /
`projection_test` and non-`self` richards accesses have no class-method
owner-field sidecar at their original instruction. A post-planning pass now
considers **only already-selected borrowed `RegionInputSource::IndexedField`
inputs** with hot generic-field profile evidence. For a unique exact
same-module owner, attribute, and index, it reuses an existing published
`SplitDict` constructor/method owner cell; it does not expand the owner
catalog, publish additional cells, add global state, support slots, or relax
virtual-object trust. Existing ordinary indexed-field plans retain precedence.

The existing public `soac_ir_typed::TypedLateBoundOwnerFieldPlan` now exposes
the public `owner_type` and `attr_name` fields so original and inlined
instructions can validate the exact same owner/attribute independently of
the anchor's source instruction. The selected borrowed source, owner module,
owner class, attribute, expected index, and existing dense owner-cell index
must agree. A private source-keyed late-owner guard map supplements, rather
than replaces, the ordinary indexed-field map; foreign modules, ambiguous or
mismatched anchors, slots, and missing evidence remain generic.

Scalar input emission reuses the exact existing weak-owner/exact-receiver,
nonzero type-version, valid split inline-values/capacity, and current
`ht_cached_keys` split-kind/index/interned-key guards. Guard validity lasts
only for the individual guarded execution and only while all those live
conditions remain true. Owner death/rebinding, subclass hooks, class-property
mutation, dictionary materialization/promotion/deletion, or an unseeded
shared-key first insertion take the full original scalar-region fallback.

The selected entire `Truthy`/comparison/getter subtree is attached before
expression linearization and retained atomically; otherwise a hoisted getter
could run before its guard and a miss would invoke observable subclass or
descriptor hooks twice. One exact-match seeded/remapped-plan helper handles
original and same-module-inlined instructions, continuation clones, and
fixpoint/remap/late phases; unrelated generic and ordinary indexed trees keep
their normal linearization. Same-module inlining preserves callee module and
original instruction identity.

Constructor continuation cloning can retain ordinary indexed fields at new
instruction IDs while their counters remain defined at the original callee
sources. The private regular indexed-field counter map therefore credits hit,
miss, no-specialization, and fallback to the existing typed original counter
source: the observed continuation clones `#40` / `#43` correctly map to
original `#6` / `#9`. This counter repair introduces no new public API and
does not change existing constructor virtualization.

Focused evidence includes actual transformed top-level and genuinely inlined
Profile→Verify→Apply scalar consumers, exactly-once subclass/property hooks,
deleted/materialized/promoted dictionaries, owner rebinding, unseeded first
insertion, and the preexisting indexed-field counter regression. All **67
selected transformed-runtime tests pass in 35.04 seconds** (**7 deselected**);
full Rust libraries pass **53 typed-IR + 208 optimizer + 557 JIT = 818**,
the combined test-target check passes in **3.75 seconds**, and scoped
format/check gates pass. The broad monolithic pytest process can leak handled
exception state across files; authoritative `just test-all` instead uses the
file-isolated batches in `scripts/run_pytest_parallel.py`. Aligning
`PYO3_PYTHON` / `PYO3_PYTHON_REAL` between Cargo and Just recipes avoids
unnecessary PyO3 rebuilds.

Normally profiled runs recover all four target functions (six branch regions)
in deltablue and richards. A debug-single profile with just one observation
misses a
`projection_test` source: originals `#114` / `#126` each have one smoke
observation versus eight in the normal profile, while `#365` is an
unprofiled continuation-cloned instruction whose original source is not
asserted. The three-round repeat recovers all four target functions (six
branch regions) in every one of **30 measured Apply workers per affected
workload**. Full-eight robust previous-SOAC
improvement is **1.00948x**; the repeated affected-subset robust improvement
is **1.03673x**, with `chaos` **1.06100x**, statistically neutral
`deltablue` **1.01251x** (approximate interval **0.97–1.046x**), and
`richards` **1.03725x**. Prior richards outliers distort arithmetic means,
so the apparent **1.296x** mean is not a performance claim.

Native code shrinks **23,417,280 → 23,359,400 bytes (-0.247%)** and machine
blocks shrink **1,553,260 → 1,549,290 (-0.256%)**, with typed IR unchanged
at **3,069 blocks / 218 functions**. The full-eight paired stock score is
only **0.48444263615875466x** and the repeated-subset stock score is
**0.462392x**; the prior **0.5127524704981717x** stock result comes from a
different paired cohort, and neither subset establishes full-suite stock
parity or the **1.10x** acceptance goal. The optimization is retained, and
the authoritative `just test-all` correctness gate **passes 1,218 Python
nodeids across 85 file-local batches / 8 workers**, with **85 / 85 batches
passing**, plus Rust **557 JIT**, **53 typed-IR**, **371 lowering**,
**208 optimizer**, and **8 PyO3** tests. The test phase takes **171.606
seconds**; see `work/logs/late-owner-scalar-test-all.log`. The full-suite
stock-performance target remains unmet.

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


## Source-Backed Function Materialization

Source-backed nested functions and generator expressions retain their actual
immutable CPython code objects. Each existing
`FunctionInstantiationTemplate` stores lazily initialized immutable
source-code facts in a reentry-safe `OnceLock`; the existing private
`RawPyCodeVersionPrefix.co_nfreevars` accessor identifies zero-freevar code
without rebuilding or scanning its immutable closure metadata. Zero-freevar
functions avoid constructing an unnecessary closure tuple, while captured
functions still receive fresh, independent cells.

Original-code construction passes a NULL qualified-name override so CPython
reuses the original code's canonical name and qualified-name objects.
Freshly validated positional and keyword defaults are installed directly in
their owned CPython function slots with the required INCREF; function CREATE
watchers still observe pre-initialization defaults/closure as `None`, and
initialization does not emit spurious MODIFY_DEFAULTS, MODIFY_KWDEFAULTS, or
MODIFY_QUALNAME events. Module metadata uses an exact-Unicode guard. Later
real user assignments still dispatch their required watcher events.

An additional private, positive-only template `OnceLock` can retain an
already-ready immutable compiled handle for an exact compile-session,
registered-code pointer, and code version. Every attachment rechecks
interpreted/test-force settings; both lookup and insertion require the
current live `PyFunction.func_code` to match the registered snapshot. Each
Python evaluation still creates a distinct function object, fresh closure
cells, and a distinct `FunctionEnv`; a cached handle is cloned into that
environment rather than sharing mutable Python state. Mismatched session,
code, version, forced-interpreter mode, user-mutated code/defaults, replaced
factories, or unavailable readiness preserve the ordinary guarded path.
The private `PreparedDirectEntryKey` test verifies key equality and
session/code/version inequality only; it does not itself exercise cache
retrieval, insertion, force-mode, or current-code paths. Those guards are
source-reviewed, while mutation and fallback behavior is covered by
transformed-runtime regressions. No public API is added.

The actual same-interpreter stock-versus-SOAC function watcher proves
CREATE-only initialization, canonical code/name/qualified-name identity,
original zero-freevar generator expressions, captured nested functions,
nonempty defaults, fresh object/cell identities, later genuine MODIFY events,
and synthetic factory replacement. Full JIT tests pass **559 / 559**;
grouped transformed-runtime regressions pass **31 / 31 in 15.81 seconds**;
the aligned Cargo test-target check passes in **5.04 seconds**, and scoped
format/check gates pass.

The normal fixed-eight comparison improves robust previous-SOAC throughput
**1.0426394914876491x**, with paired stock score
**0.5132971537493283x**. `comprehensions` median improves
**83.212579 → 66.395 us (1.253287x)**. An independent three-round,
four-workload repeat improves robust previous-SOAC throughput
**1.095507476732x**, or **1.092128843x** after paired-stock adjustment;
`comprehensions` median improves **83.21258 → 66.6000 us (1.249438x)**,
and all **60 candidate values** are below all **20 baseline values**.
Guardrail workloads show no reproduced material regression. All **80 Apply
workers** retain exactly **23,359,400 native bytes / 1,549,290 machine
blocks** and **3,069 typed blocks / 218 functions**.

Matched zero-loss native profiles contain **916 → 849 CPU-clock samples**.
Inclusive original source-backed creation decreases **17.573% → 7.067%**,
`co_freevars` scanning **2.401% → 0%**, and ready-entry lookup
**2.401% → 0.118%**. Native stack shares overlap; first-call compilation
and attached replay are diagnostic, not the throughput headline. Valid
function-watcher events still occur; only spurious initialization MODIFY
events disappear. The optimization is retained, but the **1.10x full-suite
stock-performance target remains unmet**. The authoritative full
`just test-all` correctness gate **passes 1,219 Python nodeids across 86
file-local batches**, with **86 / 86 batches passing**, plus **559 JIT**,
**53 typed-IR**, **208 optimizer**, **371 lowering**, and **8 PyO3** Rust
tests; see `work/logs/source-function-templates-test-all.log`. The complete
test phase takes **157.448 seconds**.

### Guarded Eager Comprehension Callable Elision

Stock CPython's PEP 709 eager list/set/dict comprehensions do not create a
`PyFunction` or emit synthetic `code.__new__` audit events. SOAC can restore
that observable behavior without moving the comprehension into its parent's
CFG: the original parent `Store`/`Delete`, control flow, cleanup roots,
ownership, profiling counters, and independently compiled child body remain
intact. Lazy generator expressions, source-authored functions, and
unsupported shapes retain their existing Python-function path.

Eligibility is limited to a compiler-owned synchronous same-module
list/set/dict child with exact generated display/name, one `_dp_iter_*`
parameter, no original source code object, at most **eight** validated
captured tuples/cells, and no kwargs/defaults/annotations/interpreted mode.
The optimizer declines a direct-call plan only for the matching actual
parent-local `Store(MakeFunctionWithClosure target)` and subsequent `Call`;
ordinary direct calls, source prefix spoofs, lazy generators, and mixed
Profile evidence remain safe. This is a semantic source/parent/child
decision, not benchmark-name or rendered-IR matching.

Runtime initialization captures the immutable original bootstrap factory
code and exact original private cache before module-body execution. Every
candidate rechecks live `sys.modules`, canonical runtime owner/type
version, bootstrap aliases, current factory/code, cache identity, exact
captured builtins, forced-interpreter state, and parent source tracing /
profiling / local/global monitoring. Factory/cache replacement, cache
subclasses or reentry, changed code/module, monitoring, custom builtin
mappings, source/lazy shapes, or interpreted mode fall back unchanged.
**Arbitrary in-place entries of the original private cache are not
checked**; only the approved original-cache identity/subclass boundary is
enforced.

One existing-template `PyCapsule` stores an `Arc` to the ready compiled
child and method definition while owning **no Python roots**. Each
invocation creates a genuine GC-tracked `METH_O` `PyCFunction` whose self
is a normally GC-traversed tuple containing that shared capsule, module
globals, builtins, and existing capture tuples. The callback uses a bounded
stack `FunctionEnvAbiHeader` with a NULL default slot, borrowed validated
closure cells, existing late-owner/deoptimization pointers, panic
containment, and normal CPython recursion / `tp_call` behavior. Thus
vectorcall, generic call, deoptimization, lifetime, cycles, and finalizer
ordering remain valid without hidden Python roots or changed parent cleanup.
No public API, global mutable state, runtime-helper symbol, generated-child
ABI, or new IR concept is added.

A genuine structured full optimizer-path regression turns RED-to-GREEN;
the actual transformed stock/SOAC watcher/audit regression also turns
RED-to-GREEN with **zero eager CREATE / zero `code.__new__`** across
Profile, Verify, and Apply. Existing SOAC failing-body finalizer order is
preserved exactly once. Two legacy canonical artifact expectations are
intentionally migrated to zero, while a forced original-cache-subclass
fallback still proves three distinct Python functions, independent closure
cells, and name/qualname metadata. Combined integrations previously pass
**3 / 3**, complete optimizer/JIT libraries pass **213 / 213** and
**568 / 568**, broader transformed tests pass **83 / 83** with **7
conventionally deselected**, and scoped formatting/combined test-target
checks pass. The final post-format integration rerun also passes
**3 / 3 in 7.55 seconds**.

The normal fixed-eight stock score is **0.589676x**, versus **0.588346x**
previously; arithmetic previous-SOAC **0.955007x** is distorted by extreme
unrelated outliers, while robust previous-SOAC geometry is **1.011265x**.
Three matched rounds improve comprehensions **52.4194 -> 49.9408 us
(1.0496318x; 95% interval [1.027465, 1.076117])**, or **1.0520710x**
after paired-stock adjustment; chaos remains neutral. Generated native
code decreases **23,293,040 -> 23,188,640 bytes** with every independently
compiled parent and child retained. Matched zero-loss profiles contain
**618 -> 692 comprehensions** and **599 -> 690 chaos** samples;
comprehensions eager-parent function ancestry falls **13.273% -> 6.074%**
while lazy generator expressions retain their original source path.
Profile shares overlap, and attached replay is confounded by kernel page
clearing. The optimization is **LANDED CANDIDATE / RETAIN**: the
authoritative full `just test-all` gate passes **1,228 Python nodeids**
across **91 / 91 isolated file batches on eight workers**, with zero
failures, plus **568 JIT**, **213 optimizer**, **371 lowering**,
**54 typed-IR**, and **8 PyO3** Rust tests; see
`work/logs/eager-comprehension-callable-test-all.log`. Cargo takes
**91.634 seconds**, pytest **81.696 seconds inner / 81.722 seconds
outer**, and the complete test phase **173.377 seconds**. The new actual
stock-parity integration passes in **9.10 seconds**; the known 28-test
counter-dump batch takes **80.81 seconds**. The full-suite **1.10x stock**
target remains unmet.

### Guarded Builtin Consumption of Source Generators

Canonical exact `METH_O` builtins `any` and `all` can consume a compiler-
owned source generator through its already compiled resume entry while
retaining the real source-generator `PyFunction`, its watcher CREATE
event, closure cells, laziness, and the actual Python generator object.
An existing constructor template prepares its canonical-owner guard once,
using the running pinned CPython's existing SOAC type-metadata accessor;
no public API, global state, runtime helper, IR concept, or new
generated-native shape is introduced.

Eligibility rechecks the canonical builtin, indexed live runtime/global
dictionaries, generator owner/type, current class/method/helper code,
local/global monitoring, tracing/profiling, reentry, and interpreted
fallback conditions. Existing vectorcall selectors first partition once
by their already-proven argument shape: `next` is considered only for
**one or two arguments**, exact `StopIteration` only for **two**, and
canonical `any` / `all` only for **one**, with null keyword arguments
and a nonnull argument buffer. Original `nargsf`, selector priority, and
ordinary fallback remain unchanged; unrelated calls skip the new
consumer selector. The iterator is acquired exactly once and its
effective next operation captured once. The existing direct compiled
resume keeps strong owned references to its three live Python arguments:
the resume function, preserved-state capsule, and mutable runtime
`NO_DEFAULT`. An existing length-checked preserved-state loader handles
shorter valid replacement capsules; on a non-`StopIteration` body error,
the current error helper is reloaded from live globals/builtins after
possible dictionary promotion, without resuming the generator twice.
Truthiness, yielded-value ownership/DECREF, short-circuiting, exceptions,
finalizers, class/method changes, and unsupported shapes retain their
existing CPython or conservative fallback behavior.

Exact normal `StopIteration` avoids the old artificial asyncio
cancellation check: stock and candidate now both produce no cancellation
callbacks, with `any=False` and `all=True`. A genuine unchanged-
production transformed stock-parity regression and an actual exported
vectorcall structured regression both turn RED-to-GREEN. Existing
limitations remain unchanged: source-parent local `PY_START` monitoring
may not fire, post-load rebinding of statically resolved builtin names is
not observed, and the previous short-circuit terminal-item retention is
preserved. Explicit dynamic consumer calls, actual runtime helper
monitoring, and at-most-once finalization remain covered.

The final frozen **705-line** transformed Profile / Verify / Apply
integration passes **1 / 1 in 5.57 seconds** after package formatting,
including live replacement of all three resume arguments, a genuine
zero-length preserved-state capsule, and runtime-globals promotion with
replacement error-helper dispatch. The full JIT library passes
**569 / 569**, broader transformed generator/runtime compatibility passes
**71 / 71 in 35.48 seconds**, package-scoped formatting and its check
pass, and the JIT test-target type check passes in **10.05 seconds**.
After argument-shape refinement, actual exported-vectorcall structured
coverage passes **1 / 1**, the new stock-parity regression plus five
retained StopIteration tests pass **6 / 6 in 11.34 seconds**, and the
final scoped format / test-target check passes in **2.76 seconds**.
The **26.46-second debug-extension restage** is workflow setup, not a
performance measurement.

Normal fixed-eight stock score is **0.6326613107877241x**, versus
retained **0.5896760656259606x**. Official previous-SOAC **1.061278x**
is outlier-sensitive; robust fixed-eight geometry is **0.999207x raw /
1.008631x stock-adjusted**. The uncontended three-round target improves
**49.926194 -> 44.872185 us (1.112631x; 95% interval
[1.096347, 1.139781])**, or **1.123761x stock-adjusted
[1.105023, 1.153696]**. Four-workload robust geometry is **1.028280x /
1.033141x stock-adjusted**. Richards recovers **1.020050x
[1.004693, 1.028789]** versus the unpartitioned candidate; versus
retained production its raw result is neutral, with a disclosed
**0.986296x stock-adjusted [0.967541, 0.993720]** residual decline.
Deltablue and chaos are paired-neutral. All **80 normal** and **120
targeted Apply workers** retain exact function rows and zero errors:
full-eight native coverage remains **23,188,640 bytes / 1,527,950
machine blocks / 2,866 typed blocks / 204 functions**, and targeted
per-round coverage remains **18,255,240 bytes / 1,201,600 machine
blocks / 2,265 typed blocks / 183 functions**.

Matched zero-loss comprehensions profiles contain **692 retained / 547
unpartitioned / 570 final samples**: old builtin `any` and iterator
frames disappear, the source-generator body remains, and canonical
guard inclusive/self decreases **3.656% / 1.462% -> 1.579% / 0.527%**
after argument partitioning. Immediate matched Richards profiles contain
**432 -> 568 samples**, with vectorcall inclusive/self
**13.89% / 5.09% -> 10.56% / 3.35%**; nested samples overlap and
inlined frames limit attribution. The first full-gate attempt encounters
one preexisting brittle Rust collision-identity test: legitimate
CPython open-address probing produces `[false, false]` rather than the
test's exact `[false]`; its shared test mutex then causes **112
secondary failures, 113 total**. A narrow `#[cfg(test)]`-only correction
in existing `crates/soac_jit/src/function_instantiation.rs` now requires
nonempty, exclusively false identities for both GENERAL dictionaries and
dictionary subclasses, retaining fresh-key identity and exception
checks. The exact focused test passes **1 / 1**, and package formatting
passes. This is a third existing Rust file for **tests only**; the two
generator runtime production files are unchanged. The corrected
authoritative full `just test-all` retry **exits zero**: **1,229 Python
nodeids across 92 / 92 isolated batches on eight workers**, with zero
failed batches, plus **569 JIT**, **213 optimizer**, **371 lowering**,
**54 typed-IR**, and **8 PyO3** Rust tests. Runtime build takes
**32.538 seconds**, Cargo tests **72.456 seconds**, pytest
**89.188 seconds inner / 89.206 seconds outer**, and the complete test
phase **161.678 seconds**. The new actual generator parity integration
passes in **7.28 seconds**; the known 28-test counter-dump batch takes
**88.28 seconds**. See
`work/logs/guarded-generator-builtin-consumption-test-all.log`. The
optimization is **LANDED CANDIDATE / RETAIN**; the fixed-eight stock
score **0.6326613107877241x** remains below the full-suite **1.10x
stock** goal.

A later same-strategy callable-kind refinement changes exactly the
existing `crates/soac_jit/src/jit/specialized_helpers.rs` production
file. The actual exported-vectorcall path first masks positional arity
and rejects shapes other than **one or two** arguments, then partitions
exact callable metadata into four existing route families: exact Python
functions with **two** arguments retain only the `StopIteration`
matcher; exact C functions with **`METH_FASTCALL` and the full immutable
name `next`** may reach canonical `next(iterator[, default])`; exact C
functions with **`METH_O` and an `a`-prefixed name** may reach canonical
one-argument `any` / `all` without initializing the `next` cache; all
other shapes use the original live CPython
`_PyObject_VectorcallTstate` fallback. Compiler-created exact C
`<eager comprehension>`, builtin `len` / `iter`, bound methods,
ordinary one-argument Python functions, custom callables, keywords, and
unsupported arities therefore skip impossible selectors. The existing
heavy guarded-generator consumer is **`#[inline(never)]`**, protecting
the ordinary vectorcall-hook frame from its cold machinery: actual
AArch64 release frames are **816 bytes retained / 1,216 bytes rejected
first candidate / 848 bytes refined**, with standalone consumer restored
and `next` reinlined. Original thread-state/null
behavior, vectorcall offset flags, keyword handling, callable mutation,
source-generator ownership, callbacks, monitoring, exceptions, and
finalizers remain intact.

The same refinement fixes an independently proven existing CPython
compatibility bug: a fresh process with `builtins.next = len` previously
cached `len` as though it were canonical `next`, making
`len(iter(range(3)))` incorrectly return **0** instead of raising stock
**`TypeError`**. Existing cache initialization now accepts only an exact
`PyCFunction` with immutable C method name **`next`**, exact
**`METH_FASTCALL`**, and the owning exact builtins module / its current
dictionary. Missing or replaced values leave the existing `OnceLock`
uninitialized, perform no INCREF, and use ordinary CPython dispatch;
restoring canonical `next` retries successfully. No runtime helper,
public API, mutable global, typed-IR operation, or generated source /
native-body change is introduced.

Three genuine unchanged-behavior REDs establish the user-visible cache
bug, actual callable classifier gap, and fresh-cache canonicalization
failure; a later actual-production structured RED constructs a real
exact C `<eager comprehension>` and exposes its impossible builtin
selection. Final refined production-consumed structured regressions
turn RED-to-GREEN **2 / 2**, including eager / `len` / `iter` exclusion.
Actual fresh-cache semantics, stock versus transformed
**Profile → Verify → Apply**, retained `StopIteration` mutation /
observer controls, and guarded `any` / `all` pass **8 / 8 in 14.58
seconds**, including the genuine CPython `TypeError` correction. Fresh
complete Rust JIT **577 / 577**, optimizer **214 / 214**, and typed IR
**54 / 54** pass; broad actual transformed compatibility passes
**24 / 24 in 45.69 seconds**. Package formatting is complete, and fresh
final **`cargo check -p soac_opt -p soac_jit --tests`** plus scoped
**`just fmt-rust-check soac_jit`** both pass. Refined smoke preserves
**397 total JIT source rows / 204 direct-function-body rows**, exactly
**2,238,412 native bytes / 147,769 machine blocks / 38,108 hidden
trampoline bytes**. Fixed-eight stock is **0.6555584208465822x**, with
outlier-contaminated official previous **0.9850631879265838x**; robust
`chaos`, comprehensions, deltablue, and richards estimates are near
neutral. Its **80 Apply workers / 3,970 total JIT source rows including
adapters / 2,040 direct-function-body rows** retain **23,159,960 native
bytes / 1,524,970 machine blocks / 381,080 hidden trampoline bytes**.

The clean three-round target reports stock **0.5358039397819471x**
versus retained **0.525149227454957x**, previous-SOAC
**1.0132710404047143x**. `chaos` improves **40.003514 → 38.9389 ms**,
raw **1.027341x [0.98765, 1.04542]** and significantly stock-paired
**1.050575x [1.00704, 1.06769]**; comprehensions, deltablue, and
richards remain statistically neutral. All **120 measured Apply
workers / 10,650 total JIT source rows including adapters / 5,490
direct-function-body rows** retain **54,686,760 native bytes / 3,596,430
machine blocks / 777,240 hidden trampoline bytes / 2,265 typed blocks /
183 functions**, with **zero errors**. Matched zero-loss profiles show
richards helper self **5.753704% → 4.385509%**, neutral deltablue
**6.178208% → 6.25%**, and increased comprehensions helper self
**1.862341% → 3.672922%** despite neutral throughput; nested frames
must not be summed. The authoritative full **`just test-all`** gate
passes; see **`work/logs/vectorcall-callable-kind-test-all.log`**.
Exactly **1,237 transformed Python nodeids / 99 isolated batches / eight
workers** complete **99 PASS / zero failures**. Rust JIT passes **577 /
577 in 14.65 seconds**, optimizer **214 / 214 in 0.56 seconds**, typed
IR **54 / 54**, lowering **371 / 371 in 0.47 seconds**, and PyO3
extension **8 / 8 in 0.11 seconds**. Runtime build takes **1.680
seconds**, Cargo **56.927 seconds**, pytest **74.331 seconds inner /
74.345 seconds outer**, and the complete test phase **131.284 seconds**;
the two new integration nodeids pass in **3.89 seconds**, while the
28-node counter shard takes **73.83 seconds**. Status is **FULLY
VALIDATED RETAIN LANDING CANDIDATE**, justified by the genuine
CPython-visible `next` correctness fix plus paired `chaos` improvement
without significant repeated regressions. Full-suite stock **1.10x**
remains unmet, and no universal speedup or already-landed status is
claimed.

### Template-Aware Function Registration

Source-backed function instantiation already owns an immutable
`Arc<FunctionInstantiationTemplate>`. Private registration and metadata
creation propagate that existing `Arc` instead of rediscovering the shared
module, function, template, and eager entry through repeated maps and
runtime-ID hashes. Initialized original-code presence, including
`Some(None)`, is reused; the `Arc`-owned module name is borrowed rather
than allocating a per-function string. Every function still receives fresh
metadata, independent closure cells, and a distinct boxed `FunctionEnv`;
the `#[repr(C)]` layout and ABI offsets remain unchanged.

The existing template caches only a successful vectorcall trampoline for
the exact compile session and parameter arity. The underlying process
trampoline cache additionally partitions the same arity by immutable
exact-positional eligibility, so keyword-only or variadic functions never
reuse an exact-positional trampoline. Engine initialization occurs
outside the template `OnceLock`, with no `get_or_init` around reentrant or
fallible engine work. The unchanged public registration fallback,
force-interpreter mode, generator convention, and current function/code/
default guards remain intact. No public API, runtime helper, global state,
or IR change is added. All **80 measured fixed-eight workers** preserve
exactly **23,293,040 native bytes / 1,533,550 machine blocks** and
**2,866 typed blocks / 204 functions**.

A genuine structured RED-to-GREEN regression uses two real lowered nested
CPython closures, proves shared existing-template identity and compatible
trampoline reuse, invokes the unchanged public registration path, rejects
actual alternate-session and alternate-arity trampolines, and preserves
distinct environments and captures **3** versus **9**. Full JIT library
and all test targets each pass **567 / 567**; transformed guardrails pass
**50 / 50 in 35.45 seconds**, and scoped formatting/all-target checks
pass. Normal fixed-eight stock score improves
**0.5782047994x -> 0.6028454470x**, with previous-SOAC arithmetic
**1.02674228x** and robust **1.026794x**. A matched three-round repeat
improves comprehensions **1.03462367x [1.01672, 1.06397]**, or
**1.05486663x stock-adjusted**; raw four-workload geometry is neutral at
**0.999617x**, with raw delta/richards slowdowns becoming neutral after
paired-stock adjustment. Thus isolated control drift is disclosed rather
than attributed to the optimization.

Matched zero-loss profiles, **743 -> 707 samples** at **50,000 loops /
199 Hz**, show duplicated template lookup inside metadata
**0.8078% -> 0%** and metadata hashing **0.6730% -> 0%**; inclusive
factory ancestry decreases **21.547% -> 17.529%**. Profile shares overlap,
and attached replay is diagnostic only. The optimization is retained after
the authoritative full `just test-all` gate passes **1,227 Python nodeids
across 90 / 90 isolated batches**, with **567 JIT**, **212 optimizer**,
**54 typed-IR**, **371 lowering**, and **8 PyO3** Rust tests passing; see
`work/logs/template-aware-registration-test-all.log`. Cargo takes
**68.197 seconds**, pytest **78.846 seconds inner / 78.862 seconds outer**,
and the full test phase **147.071 seconds**. The full-suite **1.10x stock**
target remains unmet.

### Guarded Indexed Runtime Factory Lookup

Synthetic-code instantiation can reuse its existing `PreparedSyntheticCode`
and interned factory key after the original factory lookup, audit, and
reentry behavior complete. The actual runtime module is a mutable heap
`_soac_ext.IndexedModuleType`, not exact `PyModule_Type`. Its cached proof
requires the exact canonical module type, unchanged nonzero type version,
direct immutable `PyModule_Type` base, inherited module getter, and no
factory-name descriptor in the heap owner's own class dictionary. Static
base-type `tp_dict` cannot be used for this proof because pinned CPython
stores static builtin dictionaries outside those type objects.

Every invocation revalidates module identity, exact owner/type version,
getter, and current exact non-GENERAL dictionary. For the runtime's custom
indexed **`dk_kind == 3`** dictionary, an existing present live slot and
interned key permit a direct borrowed read followed by the required
`INCREF`. Missing keys, GENERAL dictionaries, custom module subclasses,
module `__getattr__`, class properties or `__getattribute__`, type-version
changes, factory/module replacement, and watcher-free raw slot mutation
retain the original fresh-name `dp.getattr(...)` fallback and propagated
exceptions. No public API, global state, runtime helper, or IR change is
added. All **80 fixed-eight Apply workers** preserve exactly
**23,293,040 native bytes / 1,533,550 machine blocks** and
**2,866 typed blocks / 204 functions**.

A genuine actual-production regression turns RED-to-GREEN on a lowered
comprehension, canonical heap module, custom indexed dictionary, real
cached owner/nonzero version, and single factory invocation. Its
adversarial controls prove raw-slot replacement with balanced references,
fresh-name module/subclass hooks, GENERAL-key collision identity and
propagated errors, and mutable canonical heap-class property/getter
invalidation. Complete JIT library and all test targets each pass
**568 / 568**, broad transformed compatibility passes **33 / 33 in 34.20
seconds**, and scoped formatting/all-target checks pass. Normal fixed-eight
stock score declines **0.6028454470x -> 0.5883463026x**, with official
previous-SOAC geometry **0.9969491582x**; the apparent float regression
cannot execute this optimization. A matched three-round comparison instead
finds significant comprehensions improvement
**1.0528748x [1.022367, 1.074919]** and chaos improvement
**1.0482367x [1.026554, 1.065812]**, with delta/richards paired-neutral
and subset robust geometry **1.0403375x / 1.0307875x stock-adjusted**.

Matched zero-loss comprehensions profiles contain **707 -> 618 samples**;
synthetic-code factory ancestry decreases **3.2516% -> 1.7809%**, while
fresh Unicode-name allocation **1.415% -> 0%** and module getter ancestry
**1.1306% -> 0%** disappear. Descendant shares overlap their parent and
are not additive; changing GC share **17.97% -> 14.40%** confounds
diagnostic replay. The retained optimization passes the authoritative full
`just test-all` gate with **1,227 Python nodeids / 90 isolated batches /
8 workers / 0 failed**, plus **568 JIT**, **212 optimizer**, **54
typed-IR**, **371 lowering**, and **8 PyO3** Rust tests; see
`work/logs/guarded-indexed-runtime-factory-test-all.log`. Cargo takes
**61.431 seconds**, pytest **76.810 seconds inner / 76.825 seconds
outer**, and the complete test phase **138.270 seconds**. The full-suite
**1.10x stock** target remains unmet.

### Interned Trusted Runtime Lookup Keys

An existing source-backed function-instantiation template may lazily own
exactly **three fallibly interned immutable Unicode lookup keys**. The keys
replace temporary C-string conversion only in trusted runtime dictionary
lookups; templates retain no runtime module, module dictionary, bootstrap,
or factory value. Current `sys.modules`, runtime module, bootstrap, and
factory bindings are still checked on every call, and fallible interning
occurs outside the relevant lock.

The fast path requires an **exact dictionary** whose pinned CPython keys
layout is Unicode-only or split. Existing PyO3 `ffi::PyDictObject.ma_keys`
and one private C-layout keys-prefix mirror expose the key kind. GENERAL-key
dictionaries and dict subclasses retain the original
`PyDict_GetItemString` path: colliding custom `__eq__` callbacks still
observe fresh lookup-key identity, raised errors are swallowed/reported via
the existing unraisable behavior, and no pending exception escapes. The
separate module `dp.getattr("code_with_freevars")` remains unchanged so
custom module `__getattribute__` / `__getattr__` lookup identity stays
observable. Existing template-owned Python object lifetime remains in force;
cross-interpreter or session-reset guarantees are not newly established.

A genuine real-template/production-import regression turns RED-to-GREEN,
proving three interned identities, successive live runtime-module and
factory replacement, and no strong module retention. An adversarial
GENERAL-dictionary/subclass collision-key identity and exception test also
passes. Complete JIT library and all test targets each pass **565 / 565**;
grouped transformed runtime tests pass **21 / 21 across 12 files in 17.48
seconds**, and aligned test-target / scoped-format checks pass. Production
changes only `crates/soac_jit/src/lib.rs` and
`crates/soac_jit/src/function_instantiation.rs`; it adds no public API,
runtime helper, generated-code path, or mutable global cache.

Normally sampled fixed-eight results show an official previous-SOAC
arithmetic **regression, 0.9899057912132601x**, and stock score decreases
**0.5594598880789836x → 0.5558386711560767x**; robust full-eight ratio is
only **1.003392x**. Matched **60-versus-60** targeted samples confirm
`comprehensions` **1.067068x**, clustered 95% interval
**1.035739–1.094314x**, or stock-adjusted **1.049671x**, interval
**1.012618–1.096761x**. Robust affected/control subset is **1.024243x**, or
**1.013272x** stock-adjusted; adjusted `richards` remains borderline at
**0.957491x (0.917952–1.000002x)**. All generated native bytes, machine
blocks, and function bodies are unchanged; these caveats preclude any
full-suite speedup claim.

Matched zero-loss **50,000-loop / 199 Hz** comprehensions profiles contain
**782 → 738 samples**. `PyDict_GetItemString` ancestry falls
**4.860160% → 0%** (import **3.453272% → 0%**, bootstrap
**1.151091% → 0%**); a previously sampled unrelated **0.255798%** site has
unchanged source. Unicode allocation/decode falls **2.1743% → 0.4071%** and
descendant deallocation **2.6859% → 0.5429%**, but samples overlap and
must not be summed. Existing dict lookup remains, while cold compilation
**5.2439% → 5.5643%** and GC **14.3217% → 16.3884%** persist; profiler
replay is diagnostic, not a performance headline. The candidate is
**retained**, and the authoritative full `just test-all` correctness gate
passes **1,222 Python nodeids across 89 / 89 file-local batches and eight
workers**, plus JIT **565**, optimizer **211**, typed IR **54**, lowering
**371**, and PyO3 **8**; see
`work/logs/interned-runtime-keys-test-all.log`. Cargo tests take
**58.487 seconds**, inner / outer pytest **95.296 / 95.311 seconds**, and
the complete test phase **153.809 seconds**; the known counter-dump batch
takes **94.53 seconds**. The full-suite stock **1.10x** goal remains unmet.

## Direct Generator-Instance Preserved State

Canonical original-code generator expressions can avoid the interpreted
Python state-preservation bridge when an existing private trusted-helper
plan proves the exact original helper metadata, code identity, and compile
session. Interned cached-key dictionary probes check the currently live
runtime helper globals and generator class without allocating or invoking
user code. Any helper rebinding, replacement before its first call, modified
class/type, unexpected original code, mismatched session, or unavailable
trusted entry uses the unchanged Python helper path.

The direct path builds the existing preserved-state capsule from raw owned
object slots and unboxed `i64` scalar slots without temporary Python tuples.
An RAII builder releases acquired values on partial failure; capsule
destruction retains normal generator ownership, traversal, and cleanup.
The actual original `PyFunction` name/qualified-name objects are passed to
the real `ClosureGenerator`, restoring stock CPython identity for both
plain and captured original generators. If a live function was renamed and
no longer matches its immutable code names, the prior compiler-name fallback
is preserved.

Each evaluation still receives a fresh function, independent closure cells,
and a fresh lazy generator; `send`, `throw`, `close`, finalizer behavior,
current runtime globals, and helper/factory mutation remain observable.
Pinned CPython thread/code prefixes check tracing, profiling, global
`sys.monitoring`, and code-local `sys.monitoring`; any active observer
forces the complete original interpreted path. The selected scope is only
trusted canonical source-backed generator expressions; ordinary named
generators, coroutines, async generators, and unrelated dynamic factories
are not admitted as special cases. No public API is added.

Genuine regressions verify direct owned-object/unboxed-scalar capsule
initialization plus abandoned-builder cleanup and stock generator name /
qualified-name identity. Both turn RED-to-GREEN; the complete JIT library
and full Cargo test targets each pass **560 / 560**. Grouped transformed
generator, source-watcher, mutation, async, and previous-optimization
checks report **35 passed / 1 preexisting expected xfail**; the aligned
Cargo `--tests` check and scoped formatting/format checks also pass. The
production change is frozen to four existing files and adds no public API.

Normally sampled fixed-eight pyperformance shows robust previous-SOAC
improvement **1.006422x** and paired stock score **0.5099697650277614x**;
`comprehensions` improves **66.3955 → 63.9919 us (1.037561x)**. Three
independent affected/guardrail rounds confirm **66.3955 → 62.1283 us
(1.068684x)** for comprehensions, clustered 95% interval
**1.032661–1.107134x**, or **1.121045x** after paired-stock adjustment.
The affected/guardrail subset overall is **1.005639x** robust and
**1.021786x** stock-adjusted. Nbody and spectral have no eligible original
generator expression and show no statistically established regression;
their generated code remains unchanged. Across all eight workloads,
generated code remains exactly **23,359,400 native bytes / 1,549,290
machine blocks**, with unchanged **3,069 typed blocks / 218 functions**.

Matched zero-loss native profiles contain **849 baseline / 844 candidate
samples**; the old interpreted helper bridge drops **15.425% → 0%**, the
state-preservation PyO3 bridge **4.003% → 0%**, and factory ancestry
**18.133% → 16.357%**. Stack samples overlap: candidate slot initialization
**10.432%** includes **7.826%** periodic GC, leaving approximately
**2.606%** non-GC initialization; productive factory ancestry after
excluding GC falls **12.127% → 8.531%**. The old initializer may compile
while profiling but has no handle in measured Apply execution. This change
is **retained**; the stock **1.10x** acceptance target remains unmet. The
authoritative full `just test-all` gate passes **1,220 Python nodeids across
87 / 87 isolated batches and eight workers**, plus Rust JIT **560**, typed
IR **53**, lowering **371**, optimizer **208**, and PyO3 **8** tests; see
`work/logs/direct-generator-state-test-all.log`. Runtime test build takes
**20.539 seconds**, Cargo tests **60.073 seconds**, inner / outer pytest
**93.912 / 93.926 seconds**, and the complete test phase **154.011 seconds**.
The existing counter-dump batch takes **93.25 seconds**.

### GC-Visible Packed State and Guarded Direct Generator Allocation

A subsequent generator-state iteration fixes a genuine observable CPython
cycle-collection difference: an iterator captured through a transformed
generator's previously untracked preserved-state capsule stayed alive and
its finalizer did not execute, while stock CPython collected the same
cycle. Both existing public and compiler-owned preserved-state
construction paths now create GC-tracked capsules using the pinned
existing `_PyCapsule_SetTraverse` export.

The private **24-byte** preserved state owns one checked `Vec` allocation
containing contiguous raw `u64` values and a packed, potentially
multiword immutable object-kind bitmap. Traversal visits only marked
object and cell slots, never scalar values that happen to resemble
pointers. Clearing sets the **object payload slot to null before its
decrement**, preserves the immutable kind bitmap for later stores, and
holds no Rust borrow across a potentially reentrant finalizer. Overflow /
bounds checks, owned-reference cleanup, repeated clear/destructor calls,
resurrection, and both existing construction paths remain sound.

For the exact canonical generator class, the existing factory can use
`GenericAlloc` and initialize **eight checked slots** directly. Admission
checks the live class/type version, original initializer function and code,
initializer vectorcall, allocator / `__new__`, descriptors and hooks,
recursion, source-function watcher, and active local/global monitoring,
tracing, or profiling. A monitored initializer is observable in Apply
mode even though the retained Profile-mode baseline emits no initializer
`PY_START`; direct allocation falls back whenever the existing callback
must remain visible. Live initializer/class/code/slot mutation,
replacement subclasses, forced interpretation, dynamic hooks, and unsafe
layouts retain the complete original class-call path. Real generator
identity, laziness, captures, `send` / `throw` / `close`, and finalizer
ordering remain unchanged. This changes no public API, global, runtime
helper inventory, IR operation, or native direct-function body. The
existing successful generator event gains one **new**
`constructor_path='direct_slots'` / `'python_class'` field; no new event or
helper is introduced.

A genuine unchanged-production real stock/transformed cycle regression
turns **RED → GREEN: 1 passed in 5.93 seconds** across
**Profile → Verify → Apply**, proving weak-reference collection and
exactly-once finalization, GC tracking, **70** mixed preserved slots, live
constructor/source-monitoring mutation guards, counters, and unchanged
native coverage. An independent pinned-CPython structured regression also
turns RED-to-GREEN across **130** mixed object / cell / scalar slots,
bitmap boundaries **63 / 64 / 127 / 128**, visitor early-stop **37**,
payload clear-before-decrement, and idempotent clear. Post-format complete
JIT library and all test targets each pass **571 / 571**; the transformed
compatibility matrix passes **52 / 52 across 20 files in 30.07 seconds**
with no xfails. Scoped package formatting / formatting check and the
aligned JIT test-target Cargo check pass.

The **570-sample** zero-loss comprehensions profile used to identify the
factory hotspot comes from earlier comparison **131748**, before the
retained exact-positional-trampoline revision; it is qualitative source
evidence, not a matched profile of immediate baseline **141233**.
Comprehensions was neutral across that intervening revision. A subsequent
**557-sample** zero-loss candidate profile eliminates historical
initializer **9.477% → 0%**, interpreted init eval **6.493% → 0%**, and
reduces direct-factory ancestry **18.776% → 8.259%**. Correct GC tracking
adds capsule traversal **0% → 6.820% inclusive / 3.049% self**, with GC
ancestry **14.040% → 23.868%**; source-function instantiation remains
visible **10.873% → 12.210%**. These historical/candidate profiles span
different revisions, inclusive frames overlap, and their percentages are
not additive or causal throughput evidence.

Fixed-eight stock score is **0.6249286764762751x** versus retained
**0.6146084338507914x**; previous-SOAC arithmetic is
**1.0003747535524583x**, robust **1.004336x / 1.012863x stock-adjusted**.
The apparent initial **1.037913x** comprehensions improvement does **not**
reproduce: the authoritative three-round target is
**44.923821 → 45.000817 us (0.998289x; 95% 0.979545–1.019565)** /
paired-stock **1.001340x [0.982678, 1.026006]**. Robust four-workload
geometry is **1.007762x / 1.002211x stock-adjusted**; richards' raw
increase is neutral after stock adjustment. All **80** fixed-eight workers
retain exactly **23,188,640 native bytes / 1,527,950 machine blocks** and
**365,000** hidden trampoline bytes; all **120** repeated workers retain
**54,765,720 native bytes / 3,604,800 blocks** and **746,520** hidden
trampoline bytes, with zero errors.

This iteration is **LANDED CANDIDATE / RETAIN FOR ACTUAL CPYTHON GC /
FINALIZER CORRECTNESS WITH NEUTRAL REPEATED PERFORMANCE**, not for an
asserted speedup. Its authoritative full `just test-all` gate **PASSES
1,231 Python nodeids / 94 isolated batches / 8 workers**, with **94 passed
/ 0 failed**; see
`work/logs/direct-generator-instance-state-compact-test-all.log`.
Workspace Rust JIT **571**, lowering **371**, optimizer **213**, typed IR
**54**, and PyO3 **8** all pass. Runtime build takes **1.592 seconds**,
Cargo tests **65.950 seconds**, inner / outer parallel pytest
**79.423 / 79.439 seconds**, and the complete test phase **145.402
seconds**. The new real stock-parity regression passes in **8.31
seconds**; the existing 28-test counter-dump batch takes **79.41
seconds**. Full-suite stock **1.10x** remains unmet.

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

### Immediate Zero-Argument and Positional Method Dispatch

An immediate source method invocation can use pinned CPython's existing
`_PyObject_GetMethod` protocol without allocating the visible, GC-tracked
bound-method wrapper produced by ordinary generic attribute lookup. This
fixes a real CPython-visible difference: inside inherited `Child.observe()`,
stock sees no live bound `MethodType`, while the prior transformed path
observed its unnecessary wrapper through GC referrers. A separately stored
bound method remains visible and is never elided.

Before existing typed rewrites and linearization, a private source-grounded
sidecar records each original getter / immediate-call instruction pair and
preserves their original field/getter and call instrumentation-counter IDs.
After optimization, the typed pipeline recovers only a proven same-block
method invocation with **zero or at most two simple `Load` positional
arguments**, a uniquely defined/consumed temporary, and an existing
`Local`, closure `Cell`, or `Preserved` receiver. Escaped aliases,
standalone stored methods, multiple uses, cross-block values, global/class
receivers, more than two arguments, effectful argument expressions,
keywords, starred arguments, and source `super` paths remain on their
original path. Pinned
CPython 3.15 does not support the earlier host-interpreter assumption that
`super` must expose a wrapper; only its original result and behavior are
preserved.

Profile-driven hot-continuation cloning can preserve the same private
compiler temporary while assigning new getter / call instruction IDs. The
final recovery therefore groups candidates by the exact full compiler-private
`ResolvedName`, rejects any CFG `BlockArg` transport, and requires every
store / load / delete of that name to belong to isolated, adjacent
same-block getter / call pairs. Exactly one pair must retain both original
source instruction IDs. Every continuation clone must match the anchored
receiver, constant attribute, ordered simple positional arguments, and
`Generic` access; recovery is all-or-none for the complete group and
removes only its proven per-block store / delete. Original and cloned getter
and call counter IDs remain correct. Aliases, block-edge transport,
unmatched arguments, and all prior receiver / ownership exclusions remain
on the ordinary path.

An admitted pair uses the already-existing typed `GuardedMethodCall`
operation with **empty guards**; it does not enable currently disabled
runtime-guarded receiver-method plans or add a new public IR variant. JIT
emission mechanically imports the existing pinned-CPython
`_PyObject_GetMethod` symbol and follows its actual method/non-method/null
result ABI. CPython remains responsible for inherited lookup, live class /
MRO mutation, instance shadows, properties, data/non-data descriptors,
custom `__getattribute__` / `__getattr__`, `staticmethod`, `classmethod`,
missing attributes, and raised errors. Existing getter branch and call
counters, single receiver evaluation, owned callable / receiver references,
and method lookup **before positional argument evaluation** preserve
original CPython order. The ordinary non-method descriptor branch releases
its owned receiver before evaluating arguments; argument-prefix errors
clean up **previous owned arguments → conditionally owned receiver → owned
callable**, preserving the original exception. Finalizers, branch/call
counters, and fallback semantics remain unchanged. No public API, runtime
helper, global mutable state, or new IR operation is introduced.

Both zero-argument regressions turn RED-to-GREEN, then expanded genuine
same-strategy positional regressions independently turn RED-to-GREEN: the
whole-production typed-pipeline / source-counter test verifies zero,
one, and two arguments plus builtin/captured receivers across
**Profile / Verify / Apply**; frozen transformed four-way stock parity
passes **1 / 1 in 4.82 seconds**, preserving stored wrappers, descriptors,
MRO/shadows, `super`, lookup-before-unbound-error, finalizers, counters,
and native controls. A first complete positional JIT run exposed one
preexisting brittle GENERAL-dictionary collision assertion; legal
randomized hash probing caused approximately **111** secondary failures
through its poisoned shared Python test mutex. The durable correction
changes only an existing **`#[cfg(test)]`** assertion in
`function_instantiation.rs` to require multiple observations that are all
fresh identities; this is a fourth **test-only** path, while production
remains exactly three existing files.

An actual captured-cell nested comprehension then exposed an Apply-only
continuation-clone correctness gap: stock and Profile observed no wrapper,
while the previous Apply implementation observed one. Both genuine
regressions turn RED-to-GREEN after the bounded, same-file clone recovery:
the whole-production structured test proves exactly **one** selected node
in Profile and **two** original-plus-clone nodes in Verify and Apply;
the frozen real transformed stock-parity regression passes **1 / 1 in
7.78 seconds**, preserving every existing descriptor, evaluation, finalizer,
counter, and native-body control.

Fresh final **post-clone** Rust JIT **572 / 572**, optimizer **213 / 213**, and typed IR
**54 / 54** suites pass; grouped transformed checks pass **15 / 15**, and
package-scoped formatting / format check plus JIT `--tests` Cargo check
pass. The earlier zero-argument-only three-round deltablue result
**1.077933x [1.057344, 1.095900]** is valid for that historical
implementation only; its fixed-eight comparison **155216** is permanently
discarded because broad external contention corrupts unchanged controls.
The pre-clone positional comprehensions result was a genuine **0.93660x
[0.91529, 0.95561] regression**. Final post-clone release smoke and all
**30** repeated comprehensions workers confirm the actual nested hot Apply
body changes **12,552 bytes / 816 blocks → 12,644 / 823** while every
source function and hidden trampoline remains present. Nevertheless, clean
three-round comprehensions remains **neutral: 0.998325x [0.987093,
1.019031]**; an apparent **1.081145x** stock-adjusted result merely
coincides with **8.30%** stock drift and is not a causal gain.

The same clean comparison improves deltablue **2.92860 → 2.61845 ms**,
**1.118447x [1.09207, 1.13869]** versus retained SOAC and a further
**1.037585x [1.01464, 1.05759]** versus the already-improved zero-argument
implementation. Richards is **1.019530x [1.00573, 1.03815]**, subject to
paired-stock drift; chaos is neutral after adjustment. Final fixed-eight
ordinary native code decreases **23,188,640 → 23,163,480 bytes** with
unchanged **365,000 bytes** of hidden trampolines. Its official previous
score **0.9683515036210124x** is corrupted by host outliers; the clean
four-workload official score is **1.0194276621869476x** previous /
**0.49747399350945193x** stock. Matched zero-loss comprehensions profiles
(**292 → 292** samples) show builtin-wrapper union **3.7663% → 2.7402%**,
offset by `_PyObject_GetMethod` **3.0825%** inclusive / **1.0278%** self;
overlapping inclusive ancestry is not additive. Deltablue wrapper union
declines **3.1698% → 0.4067%**, with **199 → 99 Hz** sampling precision
caveat; small richards profiles do not prove wrapper elimination. Multiple
actual CPython-visible parity fixes and repeated deltablue improvement
support **LANDED CANDIDATE / RETAIN**. The authoritative full
**`just test-all` gate exits zero**, passing **1,232 Python nodeids across
95 / 95 isolated batches and eight workers**, with **zero failures**.
Rust suites pass JIT **572**, optimizer **213**, lowering **371**, typed
IR **54**, and PyO3 **8**. Runtime build takes **1.951 seconds**, Cargo
**89.061 seconds**, pytest **80.428 inner / 80.445 outer seconds**, and
the full test phase **169.519 seconds**; the known 28-node counter batch
takes **80.52 seconds**, and the method-parity integration passes in
**6.03 seconds**. See
`work/logs/immediate-method-call-dispatch-test-all.log`. The optimization
changes exactly three existing production runtime paths plus one existing
**`#[cfg(test)]`-only** collision assertion; the full-suite stock
**1.10x** goal remains unmet and full-suite performance is unmeasured.

### Exact Positional Argument Binding

The existing immutable `DirectArgBindingPlan` can directly bind a fully
supplied, exact-arity positional call when positional parameters map
identically to entry slots and the call has no keywords. The vectorcall
argument count masks its offset bit before comparison. Keyword-only
parameters, `*args`, `**kwargs`, missing/excess arguments, or any keyword
tuple remain on the unchanged generic binder; fully supplied positional
defaults need no default insertion.

The admitted path skips generic output zeroing and default scanning while
acquiring exactly one owned reference per supplied argument. Normal entry
cleanup still decrements each owned value. Zero-arity calls accept the
existing null argument/output buffer representation. A malformed null
argument after an acquired prefix decrements **only that written prefix**,
never touches uninitialized trailing slots, and preserves existing null-output
versus null-arguments error ordering and exact exceptions. Existing current
function/code/default refresh and all generic fallback behavior remain in
force; the path does not bypass mutable `__code__`, `__defaults__`,
`__kwdefaults__`, interpreted entry, or compiled-handle guards.

A genuine structured regression turns RED-to-GREEN across nine lowered
callable shapes, including zero-arity, positional-only, fully supplied
defaults, closure/generator, vectorcall offset, and unsupported argument
controls. An independent pinned-CPython FFI test constructs real compiled
function metadata and proves exact list-reference ownership,
`[object, NULL, object]` prefix-only cleanup, untouched sentinel tail,
zero-arity null buffers, and original error precedence. Complete JIT Rust
library and all-target suites each pass **563 / 563**, aligned test-target
checking and scoped formatting/checks pass, and transformed compatibility
guardrails pass **95 tests / 2 expected existing xfails / 7 deselected
across 16 files in 30.42 seconds**. Production changes exactly one existing
file and adds no public API, helper, template, or global state.

Normally sampled fixed-eight paired-stock score improves
**0.520917130452074x → 0.5482172650503208x**; previous-SOAC arithmetic
improvement is **1.05714472883199x**, robust **1.059214x**, and
stock-adjusted robust **1.052378x**. A targeted **60-versus-60 sample**
three-round comparison confirms robust subset improvement **1.05567184x**
(**1.02805729x** stock-adjusted): `deltablue`
**3.750207 → 3.529319 ms (1.06258668x; 95% 1.01126–1.08508x)**,
`richards` **33.958922 → 31.815431 ms (1.06737267x; 95%
1.01524–1.09177x)**, and `comprehensions`
**63.84438 → 60.14894 us (1.06143821x; 95% 1.03246–1.07916x)**.
`chaos` is neutral after paired-stock adjustment. Generated native code
remains exactly **24,353,560 bytes / 1,608,670 machine blocks** and typed
coverage **3,069 blocks / 218 functions**.

Matched zero-loss delta profiles contain **390 → 365 samples across 400
loops**; binder inclusive/self ancestry falls **7.181% / 6.925% → 2.740% /
2.740%**, outer binder ancestry **9.489% → 4.658%**, and `memset`
**0.513% → 0%**. Matched richards profiles contain **522 → 526 samples
across 70 loops**; binder inclusive/self ancestry falls **8.424% / 7.466%
→ 3.231% / 3.231%**, outer ancestry **10.150% → 5.892%**, and `memset`
**0.958% → 0%**. Cold compiler ancestry persists (delta approximately
**13.33% → 13.4%**, richards **8.822% → 9.314%**); inclusive stack shares
overlap and attached profiles are diagnostic, not benchmark headlines. The
optimization is **retained**, and the authoritative full `just test-all`
gate passes **1,221 Python nodeids across 88 / 88 isolated file batches
and eight workers**, plus Rust JIT **563**, typed IR **54**, optimizer
**210**, lowering **371**, and PyO3 **8**; see
`work/logs/exact-positional-binder-test-all.log`. Cargo tests take
**58.807 seconds**, inner / outer pytest **92.972 / 92.986 seconds**, and
the complete test phase **151.807 seconds**; the known counter-dump batch
takes **92.18 seconds**. The full-suite stock **1.10x** goal remains unmet.

The subsequent generated-trampoline iteration reuses the same immutable
`DirectArgBindingPlan::binds_exact_positional(requested_arity, NULL)`
decision, capped at **eight** parameters. The existing process cache is
keyed by **`(arity, exact_positional_eligible)`**; its original generic
engine method and trampoline remain intact, while an eligible private
sibling receives a distinct generated symbol. Thus same-arity
keyword-only, `*args`, `**kwargs`, and above-cap targets cannot accidentally
share an exact trampoline. A default-capable exact function remains
eligible and installs the exact trampoline; a call omitting a default takes
the embedded generic-binder / default-adapter fallback arm within that same
trampoline rather than installing a separate generic trampoline.

The exact trampoline mechanically checks the current function/code/default
and keyword-default snapshots, masks the vectorcall offset flag, requires
the original expected argument count and null keyword tuple, and validates
each argument pointer. Existing `RefcountLowering` acquires one
immortal-safe owned reference per supplied argument. Because every
positional parameter is proven supplied, it enters the existing
`FUNCTION_ENV_DIRECT_CODE_PTR_OFFSET` **core** entry without passing
through the default adapter. Any failed guard retains the original Rust
binder and `FUNCTION_ENV_DEFAULT_DIRECT_CODE_PTR_OFFSET` default entry;
existing recursion checks, mutable-function refresh, prefix cleanup,
error ordering, decrements, and finalizer behavior remain unchanged. No
public API, runtime helper, global, IR operation, ABI offset, or generated
benchmark direct-function body is added or changed.

Both genuine unchanged-production regressions turn RED-to-GREEN: one real
lowered-template / compile-session / process-cache Rust test partitions
actual installed trampoline pointers and preserves all generic/cap
controls; a frozen transformed **Profile → Verify → Apply** integration
also verifies exported `PyVectorcall_Function` pointers, defaults/code
mutations, offset vectorcall, ownership/finalizers, and existing native
bodies. Final post-format JIT library and all test targets each pass
**570 / 570**; the frozen integration passes **1 / 1 in 1.58 seconds**,
and the expanded transformed matrix passes **35 tests / 1 known expected
xfail across 13 files in 21.87 seconds**. Scoped package formatting,
formatting check, and JIT test-target Cargo check pass.

Generated vectorcall trampolines are absent from ordinary benchmark-body
native summaries, so their code-size tradeoff must be independently
checked in measured-worker `jitdump`: retained per-workload baselines are
richards **5,824 bytes**, chaos and deltablue **5,236 bytes each**, and
comprehensions **3,276 bytes**. All **80** normal Apply workers preserve
their **3,970** ordinary direct-function / adapter rows and exactly
**23,188,640 native bytes / 1,527,950 machine blocks / 2,866 typed blocks
/ 204 functions**, but hidden trampoline code really grows
**287,200 → 365,000 bytes (+27.09%; approximately +0.335% of ordinary
native code)**. Only the exact shape is added; generic trampolines are not
duplicated. All **120** repeated target workers similarly preserve
**54,765,720 ordinary native bytes / 3,604,800 machine blocks** across
three rounds, while hidden trampolines grow **587,160 → 746,520 bytes**.

Normally sampled fixed-eight stock score declines
**0.6326613107877241x → 0.6146084338507914x**; previous-SOAC arithmetic is
**1.0388446426221598x**, robust **1.036288x**, but stock-adjusted robust
**0.995015x** amid substantial float-stock drift. Thus these aggregate
results do not establish a stock improvement. Clean repeated target
measurements instead support retention: richards
**29.844901 → 28.280179 ms (1.055329x; 95% 1.046396–1.074325)** /
stock-adjusted **1.060877x**, and deltablue
**3.176625 → 2.928699 ms (1.084654x; 95% 1.069272–1.099800)** /
stock-adjusted **1.084153x**. Chaos and comprehensions are neutral; robust
four-workload geometry is **1.036295x / 1.030463x stock-adjusted**.

Matched same-source zero-loss richards profiles (**568 → 395 samples / 70
loops**) eliminate prior binder ancestry **7.92182% → 0%** and separate
default-adapter self **2.11249% → 0%**; direct-wrapper self rises
**6.51350% → 8.86146%** from actual inline work. The
**10.03431-percentage-point** gross sum is not a speedup prediction and
inclusive shares overlap. A **276-sample / 400-loop** candidate delta
profile retains rare generic binder ancestry **0.72435%**, but its older
baseline is a different revision and cannot establish causality.

The generated-trampoline iteration is **LANDED CANDIDATE / RETAIN** with
explicit hidden-code cost. Its own authoritative full `just test-all`
gate **passes 1,230 Python nodeids / 93 isolated batches / 8 workers**, with
**93 passed / 0 failed**; see
`work/logs/exact-positional-trampoline-test-all.log`. Rust JIT **570**,
lowering **371**, optimizer **213**, typed IR **54**, and PyO3 **8** all
pass. Cargo takes **67.263 seconds**, inner / outer parallel pytest
**77.522 / 77.537 seconds**, and the complete test phase **144.812
seconds**; the new transformed regression passes in **2.03 seconds**, and
the existing 28-test counter-dump batch takes **77.38 seconds**. This
validates the subsequent iteration independently of the earlier retained
binder's historical gate. The full-suite stock **1.10x** goal remains
unmet.

### Native Recursion Guards for Shared Vectorcall

Existing exact-positional and generic vectorcall trampolines both acquire
the current live thread state before entering a public CPython native
recursion checker. Retained lossless `richards` call profiles attribute
**2.459410%** to exact-trampoline recursion checks; an earlier
`deltablue` profile attributes **7.316147%** to exact trampoline checks.
These profile shares prioritize the opportunity; they are not promised
speedups or runtime semantic proofs.

The existing shared vectorcall emitter now uses its current thread state,
the real Cranelift native frame pointer, and private pinned
`#[repr(C)]` layout facts: `PyThreadState.base_frame` offset **80**,
embedded interpreter-frame size **88**, and frame-relative native soft
limit offset **104**. For 64-bit `aarch64` / `x86_64`, exactly **two
trusted pinned loads** and **one hot unsigned conditional branch** check
the conservative maximum-margin band
**`[soft - 65536, soft + 32768)`** with wrapping unsigned
**`(frame_pointer - soft + 65536) <u 98304`**. The universal
**32,768-byte** margin covers both the pinned release margin **16,384**
and documented debug/ASAN/TSAN margin **32,768** without changing the
hot-path branch or instruction count.

Pinned CPython attaches the nonnull live thread state only after its
nonnull embedded base frame and nonzero native stack limits have been
initialized; existing generated direct callees already rely on these
attached-vectorcall invariants. The final emitter therefore has **no
redundant null-state, null-base, or zero-limit presence branches**. Its
single range branch sends an in-band native frame to the original
**cold** `dp_jit_enter_recursive_call` → `Py_EnterRecursiveCall` path
with unchanged TLS lookup, native stack check, failure, exception, and
wrong-fiber handling. The initial **`UINTPTR_MAX`** soft-limit sentinel
may skip only when the pinned public helper would also return zero.
Unsupported architectures retain the unconditional public helper;
arbitrary CPython layout changes are not dynamically detected. The
current thread state is never cached across calls or threads, and no
public helper, API, typed IR, or ordinary direct-function body is added.
Only hidden exact/generic vectorcall trampoline code changes: ordinary
targeted source bodies remain exactly **54,686,760 bytes / 3,596,430
blocks**, while hidden trampolines grow retained **746,520 → 777,240
bytes**, below rejected first **789,720 bytes**. Normal fixed-eight
source bodies remain **23,159,960 bytes / 1,524,970 blocks**; hidden
trampolines change retained **365,000 → rejected 386,920 → final 381,080
bytes**.

A genuine production-used Cranelift structured regression turns
RED-to-GREEN, proving the real frame-pointer read, pinned offsets, two
trusted loads, the final **98,304-byte** unsigned danger interval, and
exactly **one cold / zero hot** original recursion-helper calls. A
subsequent genuine refinement RED-to-GREEN reduces actual production
hot conditional branches **4 → 1** while preserving the same interval,
loads, layout, and cold helper. Actual
stock versus transformed **Profile → Verify → Apply** recursion
compatibility passes **1 / 1 in 1.60 seconds** across exact and generic
calls, bounded `RecursionError`, three live thread states, ctypes
callbacks, profiling, finalizers, and generated native bodies. Complete
JIT / optimizer / typed-IR libraries pass **575 / 575**, **214 / 214**,
and **54 / 54**; broad transformed coverage passes **17 / 17 in 40.06
seconds**, and combined test-target / scoped formatting checks pass.
Production changes exactly existing `runtime_context.rs` and
`vectorcall.rs`; the existing JIT regression is `#[cfg(test)]`-only.
The preceding four-branch implementation was rejected after a genuine
three-round `chaos` regression: raw **0.970913x**, stock-adjusted
**0.975258x**. Final clean targeted three-round `deltablue` improves
**2.468877 → 2.318359 ms**, raw **1.064924x [1.030268, 1.086195]** /
paired **1.073872x [1.036578, 1.107536]**; `richards` improves
**23.625606 → 21.780125 ms**, raw **1.084732x [1.056110, 1.098710]** /
paired **1.072466x [1.040196, 1.096731]**. `chaos` is neutral, raw
**1.000030x [0.985100, 1.032427]** / paired
**0.974080x [0.958827, 1.007537]**, and improves
**1.029990x [1.017646, 1.065572]** over the rejected first iteration;
`comprehensions` is neutral. Official targeted stock / previous SOAC are
**0.525149227454957x / 1.0374660673409746x**; normal fixed-eight are
**0.6694448241941483x / 1.0016222298324013x**. All **120 targeted Apply
PIDs / 10,650 total JIT source rows including adapters / 5,490 actual
direct-function bodies** preserve source identity, native bytes / blocks,
and typed coverage, with **100,206 INFO / zero errors**.

Matched lossless rejected-first / refined `deltablue` native captures
contain **176 / 178 samples**, the same **600 loops / 99 Hz**, and zero
lost samples. The targeted public recursion helper has zero samples in
both; exact trampoline **self** declines **5.11333% → 2.80919%** after
the three redundant branches are removed. Separately attributed
exact-trampoline live-thread-state TLS acquisition declines
**2.272591% → 0.561837%** but remains; the older-revision retained
profile observed **6.096123%**. True matched retained / refined
`richards` lossless captures have **244 / 226 samples**, the same **100
loops / 99 Hz**, and zero loss: strict exact-trampoline public recursion
helper **2.459410% → zero**, trampoline self
**10.245541% → 7.081328%**, and live-thread-state TLS
**1.639606% → 0.884416%**, still present. Unrelated refined
`RichCompare` recursion contributes **0.442208%**. Limited samples and
overlapping stack ancestry prohibit adding these shares.

The authoritative full `just test-all` gate independently passes;
complete evidence is
`work/logs/inline-native-recursion-stack-guard-test-all.log`. Exactly
**1,235 transformed pytest nodeids / 98 isolated batches / 8 workers**
complete **98 passed / zero failed**. Rust JIT passes **575 in 21.50
seconds**, optimizer **214 in 0.70 seconds**, typed IR **54 in 0.01
seconds**, lowering **371 in 1.54 seconds**, and the PyO3 extension
**8 in 0.14 seconds**. Runtime build is **1.571 seconds**, test-target
compilation **36.23 seconds**, complete Cargo tests **62.855 seconds**,
inner / outer parallel pytest **79.523 / 79.538 seconds**, and total
test phase **142.405 seconds**. The new native-recursion integration
passes in **2.20 seconds**, the prior uniform-field integration in
**2.59 seconds**; one 28-node counter shard takes **78.87 seconds**.
Status is **FULLY VALIDATED / RETAIN LANDING CANDIDATE**, not yet
landed. No baseline CPython-visible bug is claimed; full-suite stock
**1.10x** remains unmet and unmeasured.

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
  recreated inside its caller. General closed-pipeline fusion does not attach
  executable generator-instance metadata to those calls: the current sidecar
  changes factory codegen and is not a guard. No exact-source,
  benchmark-specific, or opaque-fusion exception changes this policy. Both
  source-backed and transformed named generators retain their explicit
  direct-entry metadata; counter-recording
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
  Immediate consumption by `list`, `set`, or `tuple` is not by itself proof that
  a source-backed named-generator activation may be erased. Its native
  `PyGenObject` and suspended frame are observable through `frame.f_generator`,
  tracing and monitoring, `sys._current_frames()`, traceback construction, and
  exceptional cleanup. Source-backed generator function objects and escaping,
  aliased, or immediately materialized calls therefore retain CPython's public
  vectorcall in the generalized path. Admitting one safely will require a
  pre-activation guard over the exact function binding, code, defaults, and
  closure dependencies, with guard failure resuming the untouched original
  expression. It must also reject the activation before rewriting when
  argument/state binding or the whole ownership graph cannot be lowered.
  Calls to builtin `list` or `tuple` with a proven nonescaping generator
  instance can also carry an explicit typed builtin-implementation plan. That
  plan keeps the observed callable as the original builtin, but selects the
  visible `soac.runtime.list_from_iter` or
  `soac.runtime.tuple_from_iter` helper as a fallback-free inline body so the
  generator consumer loop can be exposed to later typed rewrites without
  pretending that the builtin object itself is the helper target. The tuple
  helper keeps its own visible `iter`/`next` loop around a list accumulator before
  final tuple materialization, so tuple(genexpr) can reuse the same
  generator-consumer lowering rather than falling back to CPython iteration.
  `soac.runtime.set_from_iter` remains a callable runtime helper, but production
  planning deliberately leaves exact builtin `set` construction on CPython's
  native path. Inlining the current Python helper expands `result.add(item)`
  into too much method-call and exception control flow to be compact or
  profitable; a future fused set sink should lower directly to checked
  `PySet_Add`-shaped IR/runtime support.
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

## Closed Iterator-Pipeline Fusion

Closed iterator-pipeline fusion removes proven generator, `map`, and `filter`
intermediates when a single-use chain is consumed immediately by an exact
builtin `list` or `tuple`. Eligible producers include compiler-owned
generator expressions and the compiler-owned `map`/`filter` workers created by
the selected stages. Source-backed named generators stay native in this
generalized pass. The sink remains a real Python collection and may escape;
the original stage algorithm executes and materializes the exact sink from
the values that reach it. No benchmark-specific algorithm substitution or
source-identity exception participates in this generic path.

The specialization records no new profiling counters. Typed planning derives a
single-use def/use chain from the verify/apply typed module plus exact static
targets.
Each `map` or `filter` stage must be the canonical builtin with exactly two
positional arguments, and its iterable input must have one of those proven
generator identities. The chain must terminate at an exact `list` or `tuple`
consumer. A callback or predicate remains an ordinary fallible
per-element call; it does not receive the iterator, so only the iterable edge
is treated as internal, nonescaping consumption.

Planning stages the visible runtime implementations
`map_from_iter`/`filter_from_iter` and
`list_from_iter`/`tuple_from_iter` one source at a time. After
each rewrite, the typed inline fixpoint refreshes owner evidence, exposes the
next wrapper, inlines eligible worker or producer resumes, and scalar-replaces
their preserved activation slots with caller locals. External runtime callees
use the same public generator layout as local callees during state lowering.
Codegen therefore receives ordinary typed loops, callback calls, and collection
operations; it does not rediscover the pipeline or emit a pipeline-specific
runtime patch.

The fused order remains source-next, map callback, filter callback or truth
test, then sink insertion for each item. Construction and argument evaluation
order, eager outer `iter()` acquisition, and list and tuple encounter order
remain visible. `StopIteration` from a map callback, filter callback, or filter
truth test terminates that stage and returns the partial sink, matching the
builtin iterators. Other callback exceptions propagate once. `StopIteration`
raised inside the fused generator-expression producer still follows PEP 479.
Exact builtin `set` consumers remain native, including their hashing, equality,
insertion-error, and length-hint behavior.

Current eligibility deliberately excludes aliases or multiple uses of an
intermediate, escaping iterator results, keyword or starred arguments,
multi-input `map`, arbitrary source iterators, exact builtin `set` and other
sinks, and source-backed named-generator activations. A source name that does
not resolve to SOAC's canonical runtime builtin also prevents selection.
Excluded cases retain their ordinary unfused path; in apply mode a
source-backed named generator retains its native CPython call, while generated
or transformed iterators retain SOAC's ordinary wrapper path. Supporting
source-backed producers requires a guarded whole-graph fallback; closed
ownership alone cannot protect against decorators, global or `__code__`
rebinding, and positional or keyword-default mutation.

Individual resume targets are currently limited to 64 blocks and 512
recursively counted typed instructions. Optional inline admission stops near a
cumulative caller cap of 384 blocks and 4096 typed body instructions; consumer
admission reserves its immediate resume/protocol follow-up and admits at most
one builtin-implementation source per fixpoint pass. These are typed CFG growth
limits, not native code-size limits. A stage that is too large remains unfused,
but an already-started rewrite may require resume/state cleanup beyond the
optional cap; the current process is not a whole-graph transaction. Deep nested
graphs can therefore be only partially fused and grow substantial exception,
ownership, and protocol control flow. Source-backed N-Queens generators remain
on their ordinary native path; no benchmark-specific replacement bypasses
those generator or fusion boundaries.

Fully fused intermediates intentionally relax CPython compatibility for
back-door generator and frame introspection. Their generator-expression and
`map`/`filter` objects, suspended frames, and frame ancestry do not exist:
`gi_frame`, `frame.f_generator`, `sys._current_frames()`, tracing, profiling,
monitoring, GC inspection, and equivalent object/frame observation cannot see
those eliminated activations. Their allocation, reference-count,
destruction/finalizer, recursion-check, and eval-breaker timing can
consequently differ. The materializer helpers grow a list by append, and
`tuple_from_iter` uses that list before constructing the final tuple, so peak
memory, allocation/`MemoryError`, and cleanup timing can differ from CPython's
builtin length-hint paths. Pipeline admission does not treat callbacks as
iterator escapes or erase their effects. Callback calls preserve ordinary
result and exception semantics and may be independently specialized under the
direct-call policy above.

## Opaque Fused Iteration

Production does not admit benchmark-specific opaque iteration. The previous
exact-source N-Queens substitution, pinned source fixtures, apply-mode opaque
root replacement, specialized scalar JIT helper registration, and source-
specific indexed module-dictionary exception have been removed. Historical
performance-log entries describing that experiment are not current runtime
behavior or valid full-suite performance evidence.

Source-backed named generators, including the actual N-Queens workload, retain
their original CPython generator vectorcall and ordinary apply-mode module
globals. Their source generator bodies run, real solution tuples are produced
in encounter order, and active Python tracing continues to observe producer
call and line callbacks. Production does not silently erase those source-backed
generator activations, bypass observers, or replace the program with a
benchmark-selected algorithm.

The source-independent closed iterator-pipeline fusion described above remains
available for its documented compiler-owned shapes. Generic optimization-plan
or IR representations and runtime helper ABI may remain without establishing a
production admission. Any future whole-graph fusion or domain specialization
must use source-independent semantic facts, preserve visible Python effects
unless an explicitly approved compatibility policy applies, and retain a valid
untouched fallback. Exact source bytes, benchmark names, harness behavior, or
precomputed benchmark results are never production eligibility conditions.


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

For source-defined classes, constructor identity must become available at
class creation, not only after the entire transformed module finishes
initializing. The existing `soac.runtime.create_class` callback already sees
the realized class and its trusted transformed namespace function; that
function can supply the owning module's SOAC context to the existing
owner-type registration path. Registering the safe synthetic constructor
identity before returning the class lets profile-mode module-level calls
record that identity in `call_hot_targets`.
`_soac_ext.profile_watch_type_key_layout(cls, namespace_fn)` first retains the
existing split-key watcher and then calls
the public `soac_jit::register_created_owner_type_from_namespace` API, which
reads trusted SOAC namespace-function metadata and reuses ordinary owner-type
registration. Untransformed namespace functions remain a no-op. The final
module-wide registration sweep and owner-mutation invalidation remain
necessary and are preserved.

The early path must reject custom metaclasses and unsupported allocation
shapes using raw CPython type slots before inspecting `__module__` or calling
any Python-visible operation. A metaclass can override `__getattribute__`,
and an early `owner.__module__` lookup would otherwise run user code before
the class name is assigned in its containing module. Same-module eligibility
must also avoid exotic descriptor or equality callbacks. Eligibility must
cover the complete recursively visited owner-type graph: a safe outer class
can contain a nested class with a custom metaclass, and recursively
registering that nested type would expose the same premature lookup. A
cycle-safe `owner_type_supports_early_registration` preflight traverses raw
type dictionaries and verifies heap type, exact built-in metaclass, generic
allocation, `object.__new__`, and exact-Unicode module identity before any
Python callback. It defers the entire unsafe graph to the unchanged late
registration sweep after class assignment.

An eager apply compiler may plan lowered functions before the module body has
created the actual Python class. The synthetic constructor function and its
persistent ID can still be known from the lowered module, but the heap-type
identity and constructor metadata are only available later at execution.
Thus early registration alone does not prove that apply selected a direct
edge: require structured planning or actual `soac_jit_direct_edges` evidence
before claiming constructor specialization. The focused profile-to-apply
integration verifies a real eager-mode direct edge for a safe source-defined
class, and a `chaos` apply run independently observes new direct edges in
`GVector.linear_combination`, `GVector.__mul__`, and `GVector.__add__`.
Generated-code growth and steady-state throughput must still be evaluated
separately. Unsupported top-level or nested class shapes retain
zero constructor metadata and their original Python `__new__`, metaclass
`__call__`, `__init__`, evaluation order, side effects, and exceptions.

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
- Exact type tags include `ExactTypeTag::Int = 1`, `ExactTypeTag::Str = 2`,
  and `ExactTypeTag::Float = 3`. The float tag is consumed by the separate
  exact-float expression-tree specialization below; exact-int regions retain
  their existing tag and semantics.
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
  - additional `str`, `bytes`, `bool`, and mixed-type operator shapes; exact
    float arithmetic is handled separately as a multi-operation expression tree
  - richer shape encodings
  - specialization for more operators that are currently generic


## Profiled Exact-Float Expression Trees

### Counted Input and Planning

- Source input is the existing `operator_hot_shapes` Profile evidence, keyed to
  the original lowered function and each semantic arithmetic `InstrId`.
- Exact `PyFloat_Type` operands use append-only `ExactTypeTag::Float = 3`; an
  exact-float/exact-float binary observation is packed as **771**. Existing
  exact-int and exact-string tags remain unchanged. Profile mode records the
  unspecialized original operation graph; Apply consumes only that completed
  source-keyed evidence.
- A valid `ExactFloatExpressionSpecializationPlan` selects one maximal tree of
  **at least two** `Add`, `Sub`, or `Mul` operations. Every internal operation
  must have exact-float shape **771**; every leaf must be a direct resolved
  local or immutable module-constant load. The function plan retains the root
  source, all operations and their original order/kinds, and all leaf source
  identities.
- Normal whole-module planning and the existing single-function paths both
  recurse through enclosing expression containers, so an eligible arithmetic
  subtree remains selectable beneath an otherwise generic call or power.
  The collector chooses maximal trees; source-keyed validation rejects stale
  roots, changed operation/leaf order, unsupported leaf effects, and
  overlapping selections.
- The resolved `TypedExactFloatExpressionPlan` sidecar is attached immediately
  after initial typed conversion. The existing typed expression linearizer
  preserves that explicitly selected complete tree as one atomic expression;
  unselected expressions retain their ordinary linearization. This keeps
  enclosing `Call`/`Pow` boundaries unchanged and prevents nested operations
  from being replaced by temporary loads before mechanical code generation.
- Explicit public crate APIs are the crate-root
  `soac_ir_typed::TypedExactFloatExpressionPlan` re-export and
  `soac_ir_typed::plan_v3::{ExactFloatExpressionSpecializationPlan,
  ExactFloatExpressionOperationPlan}`.

### Codegen

- Codegen consumes only the validated typed-root sidecar. It loads and checks
  each leaf against relocatable `PyFloat_Type` **immediately and in the
  original left-to-right evaluation order**; a mismatch branches at once to a
  cold clone of the complete original generic subtree.
- Guarded exact floats are read as `f64`; selected operations emit separate
  `fadd`, `fsub`, and `fmul` instructions in the original expression tree and
  association. The hot path calls `PyFloat_FromDouble` **once**, for the final
  observable expression result. A five-operation sum-of-squares therefore
  emits three `fmul`, two `fadd`, no `fma`, and one Python-float allocation;
  its complete generic arithmetic and enclosing power remain available.
- Exact-type checks execute on every optimized invocation. Failed guards do
  not carry assumptions between calls, skip subclass/reflected callbacks,
  inspect a later unbound local before an earlier callback, or resume from a
  partially evaluated unboxed subtree.

### Limitations / Soundness / Extensions

- Only profiled exact-float trees with at least two `Add`/`Sub`/`Mul` nodes
  and direct local/constant leaves are eligible. Isolated operations, in-place
  arithmetic, divisions, modulo, powers, comparisons, float subclasses,
  mixed-type values, attribute/index/call leaves, and missing/stale evidence
  retain their original generic behavior; an enclosing call/power may still
  contain a selected arithmetic child.
- Preserve source evaluation order, single evaluation, full-subtree generic
  fallback, Python ownership and exception behavior, NaN/infinity, signed zero,
  and the IEEE rounding of each separate operation. **Never contract a
  multiply/add into FMA, reassociate operations, or enable fast-math.**
- Focused structured codegen and transformed Profile→Apply regressions pass,
  including nested call/power trees, exact shape **771**, one final box,
  subclass/reflected fallback, a rounding case that distinguishes FMA from
  separate operations, and a raising subclass before a later unbound local.
  Three additional structured optimizer cases verify maximal call/power tree
  selection, unsupported-shape/effectful-leaf rejection, and one atomic
  selected-tree lift versus five ordinary generic lifts. The full typed-IR
  suite passes **49 / 49**, including malformed-tree and reordered-plan
  rejection; the full optimizer suite passes **205 / 205**. Expanded
  transformed-Python guardrails pass **40 / 40 in 122.32 seconds**, and the
  combined Cargo test-target check plus scoped formatting checks pass.
  Two repeated normally sampled rounds significantly improve the eligible
  `float` and `nbody` workload medians **1.047x** and **1.072x**, while the
  eight-workload median geometric result is approximately unchanged
  (**0.996x**) and includes unrelated outliers. The complete correctness
  gate passes **1,216 Python nodeids across 83 batches / 8 workers**, plus
  all Rust suites; see `work/logs/fused-float-test-all.log`.


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


## Compiler-Owned Fixed-Length Sequence Unpacking

### Counted Input

- Fixed-length assignment unpacking is a Python language operation, not a
  profiled call to a mutable runtime function. No `call_hot_targets`, type
  observation, workload name, or other profile evidence is required.
- Lowering selects the append-only `RuntimeName::UnpackFixed` plus an immutable
  literal target count for compiler-generated, nonstarred assignment and
  `with` targets. Starred targets keep the existing `RuntimeName::Unpack`
  operation.
- Only an explicitly resolved runtime-name location or validated runtime-name
  constant qualifies. Same-named user globals and ordinary source calls to
  Python runtime helpers are never treated as compiler-owned primitives.

### Codegen

- Profile, verify, and apply-mode JIT code can consume the explicit
  `RuntimeName::UnpackFixed(value, arity)` operation through its direct C ABI:
  `soac_runtime_unpack_fixed(tstate, borrowed_value, unboxed_arity)` returns
  an owned Python tuple or null with the current Python exception preserved.
- The raw helper is eligible for the existing bounded local-runtime inliner;
  its current shared admission limit is 128 Cranelift instructions.
- Imported CPython type symbols retain their canonical writable-data flags
  during inlining; unrelated external data keeps its existing declarations.
- The local raw runtime checks exact type and target count. A matching exact
  tuple receives a new owned reference; a matching exact list is snapshotted
  into an owned tuple before assignment-target side effects can mutate it.
  These guards are checked on every invocation; no mutable object, type,
  runtime-helper binding, or source-module value is cached across calls.
- Wrong arity, tuple/list subclasses, and arbitrary iterables use one
  registered Rust cold helper, `dp_jit_unpack_fixed_slow`. It invokes the
  linked CPython fixed-unpack implementation with an independently owned
  stack-reference buffer, reverses the resulting stack order, and publishes
  the result through CPython's steal-on-success tuple constructor.
- Name binding preserves `UnpackFixed` as an explicit intrinsic, like
  `RuntimeName::Globals`, instead of materializing it as a module constant or
  rewriting it to a module global. This also avoids recursively loading a
  not-yet-defined runtime attribute during transformed-runtime bootstrap.
- The entry/deoptimization interpreter recognizes the same trusted
  `RuntimeName::UnpackFixed` operation directly and uses the shared cold
  helper without invoking a mutable Python runtime attribute.

### Limitations / Soundness / Extensions

- Exact-type guards never bypass tuple/list subclass overrides. Generic
  iteration preserves CPython's callback order, exact underflow/overflow
  errors, exception propagation, and partial-item cleanup.
- A Python tuple is never used as temporary unpack scratch: arbitrary
  iterator callbacks could observe or collect its partially initialized,
  GC-tracked state. Tagged stack references remain privately owned until
  successful tuple publication; allocation failure releases each item once.
- Starred unpacking and explicit Python calls to `soac.runtime.unpack` keep
  their existing mutable behavior. Rebinding or replacing Python helper code
  does not change compiler-owned language unpacking.
- The complete lowerer suite, structured direct-ABI/provenance/writable-type
  tests, compiler-bootstrap and `with`-target lowering tests, and strengthened
  Profile/Apply/forced-entry behavior pass. The integration additionally
  verifies context-manager exit, reentrant garbage collection, immediate
  partial-item cleanup, and publicly observable `unpack_fixed` rebinding.
  A broader 28-case transformed-runtime regression selection also passes;
  combined workspace/raw-runtime checks and scoped formatting also pass.
  An eight-workload comparison improves previous-SOAC geometric throughput
  1.757x, led by 12.60x `nbody` and 8.62x `spectral_norm`; three separately
  reproduced significant `comprehensions`, `deltablue`, and `fannkuch`
  regressions remain unexplained. Full-suite stock parity is not achieved,
  while the full `just test-all` correctness gate passes.


## Compiler-Owned Iterator Exhaustion Exceptions

Compiler-generated synchronous and asynchronous iteration handlers bind their
exception types to the private runtime names `__soac__.StopIteration` and
`__soac__.StopAsyncIteration`. This covers both synthetic comprehension
rewrites and ordinary `for` / `async for` lowering. Compiler-owned iteration
must not accidentally resolve a user module's lexical `StopIteration` or
`StopAsyncIteration` binding: replacing either name must not change loop
termination.

The rule applies only to synthetic handlers. User-authored
`except StopIteration:` and `except StopAsyncIteration:` continue to resolve
ordinary lexical globals and therefore retain their existing Python-visible
shadowing behavior. Lowered runtime-name provenance also remains explicit
for downstream planning; this change does not add a new helper, alter
optimizer direct-call selection, or bypass mutable runtime-helper globals.


## Guarded Canonical StopIteration Matching

Profiled direct-call planning declines only a source-proven compiler-owned
`RuntimeName::ExceptionMatches` invocation whose exception type is the
compiler-owned runtime `StopIteration`, including resolved constant-pool
runtime aliases. Ordinary local handlers, callbacks, `ValueError`, and
other direct-call targets remain unchanged. The declined matcher reaches the
existing generic vectorcall hook mechanically; no new IR operation,
runtime helper, owner catalog, public API, or process-global cache is added.
The previous unsound process-global static matcher cache is removed.

An existing private helper-instantiation template owns a zero-allocation
pointer/index/session metadata cache. It records **seven custom-indexed
runtime-global dependency slots and four exact combined-Unicode builtin
entries**, not new Python keys or strong module references. At every use,
the guard rechecks live globals dictionary / `ma_keys` / `ma_values`, exact
original key identity, slot index, value pointer, null/tombstone absence,
and current builtin table/value. This detects both replacement of an
existing raw indexed `isinstance` slot and insertion into a previously
absent `issubclass` slot even when watcher/version/`ma_used` metadata does
not change. Canonical original registered helper and validator functions,
their code objects, compile session, globals mappings, and expected current
dependency values must all still match.

Both helper and validator are checked for tracing, profiling, global
monitoring, and code-local monitoring; any active observer takes the full
original Python path. The native match accepts **only an exact
`StopIteration` instance**. All subclasses, spoofed or raising `__class__`,
exception class objects, nonmatching exceptions, code/module/helper/builtin
replacement, and descriptor or callback changes fall back, preserving the
original helper's early `isinstance(exc, RecursionError)` visibility.
Existing watcher-bypassing runtime stores therefore cannot silently erase
observable callbacks.

The genuine structured production planner regression turns RED-to-GREEN
while preserving three unrelated direct targets. The genuine transformed
integration improves **5 failed / 3.78 seconds → 5 passed / 4.87 seconds**,
fixing four preexisting user-visible helper/validator mutation callback
bugs and proving actual nested Profile→Verify→Apply direct-edge removal;
an unrelated direct edge remains. Real pinned-CPython FFI regression proves
unchanged indexed dict versions/usage and unchanged builtin `ma_keys` still
invalidate live guards with balanced owned references. Full optimizer/JIT
suites pass **212 / 212** and **566 / 566**; transformed compatibility
guardrails pass **37 / 37 across 16 files in 26.92 seconds**; package-scoped
format checks and combined all-target Cargo checks pass. Exactly three
production files change; no public API or helper is added.

Normally sampled fixed-eight paired-stock score improves
**0.5558386711560767x → 0.5782047994439117x**; previous-SOAC mean
improvement is **1.0350348551699229x**, robust **1.02855x**. Matched
60-versus-60 three-round chaos improves **1.120774x**, clustered 95%
interval **1.07971–1.14830x**, or **1.121950x** stock-adjusted; deltablue
improves **1.038626x**. Robust subset improvement is **1.045955x**, or
**1.063296x** stock-adjusted. Normal native code shrinks
**25,033,800 → 23,293,040 bytes (-6.9536%)**: only compiler-owned matcher
and validator helper bodies disappear; no user function coverage is lost.

Matched zero-loss **70-loop / 199 Hz** chaos profiles record **806 → 639
raw samples** and **415 → 349 aggregated stacks**. Matcher ancestry falls
**10.421% → 0%**, nested validator **5.334% → 0%**,
`builtin_isinstance` **2.357% → 0%**, `object_isinstance`
**1.861% → 0%**, and runtime-global slow lookup **3.225% → 0%**. The new
live guard accounts for **2.035% inclusive**, with overlapping **0.939%**
slot checks and **0.312%** monitoring. Existing eager factory remains
**9.428% → 10.010%**; profile shares overlap and attached replay is
diagnostic only. The optimization is **retained**, and the authoritative
full `just test-all` correctness gate passes **1,227 Python nodeids across
90 / 90 isolated file batches and eight workers**, plus JIT **566**,
optimizer **212**, typed IR **54**, lowering **371**, and PyO3 **8**; see
`work/logs/live-guarded-stop-iteration-test-all.log`. Cargo tests take
**70.160 seconds**, inner / outer pytest **74.533 / 74.546 seconds**, and
the complete test phase **144.721 seconds**; the known counter-dump batch
takes **74.13 seconds**. The full-suite stock **1.10x** goal remains unmet.

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
