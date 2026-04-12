# Value Facts And SSA Locals

## Goal

Value facts and SSA locals should be implemented as one staged pipeline change,
not as two independent systems. The shared objective is to make optimization
decisions from stable IR identity and facts, while making Python local ownership
explicit enough that refcount timing remains CPython-compatible.

The boundary should be:

```text
BlockPy passes decide names, globals, control flow, stable semantic instruction
identity, and explicit Python ownership/refcount operations.

Value facts analyze codegen-shaped BlockPy as a read-only sidecar.

Refcount lowering consumes value facts and rewrites implicit Python ownership
effects into explicit transfers, increfs, decrefs, and cleanup requirements.

JIT planning consumes explicit ownership/refcount operations and decides
Cranelift block params, cleanup edges, and stack-slot materialization.
```

## Pass Placement

The intended order is:

```text
name_binding
-> global_index
-> bb_prepared
-> bb_codegen
   - normalize strings
   - dense block labels
   - stable semantic instruction ids
-> value_facts analysis sidecar
-> ownership_effects analysis sidecar
-> optional trace/counter instrumentation
-> validate
-> JIT codegen with FactStore + SSA LocalEnv
```

Synthetic instrumentation should not change semantic instruction identity.
Counters inserted after ID assignment should point at a semantic site ID rather
than becoming semantic instructions themselves.

## Stable Instruction Identity

`InstrId` is `{ block_label, instr_index_in_block }`, so it is only unique inside
one function. Cross-function facts and counters should use:

```rust
pub struct InstrKey {
    pub function_id: FunctionId,
    pub instr_id: InstrId,
}
```

The final codegen shape should eventually make IDs present by construction:

```rust
pub struct SourceMeta {
    pub node_index: ast::AtomicNodeIndex,
    pub range: TextRange,
}

pub struct IdentifiedInstr<I> {
    pub instr_id: InstrId,
    pub source: SourceMeta,
    pub op: I,
}
```

The migration path is:

1. Add `InstrKey` and validate ID presence/uniqueness for semantic codegen IR.
2. Rename `InstrCodegen` to `InstrCodegenOp`.
3. Add `IdentifiedInstr<InstrCodegenOp>`.
4. Add an unidentified codegen module shape before ID assignment.
5. Make `assign_module_instr_ids` consume unidentified codegen IR and produce
   identified codegen IR.
6. Update JIT, counters, facts, and dumps to read IDs from the wrapper instead
   of `Meta::instr_id`.
7. Keep synthetic instrumentation tied to the semantic site IDs it observes.

This avoids an optional `Meta::instr_id` becoming another maybe-present field in
the final representation.

## Value Facts

Facts should be a sidecar, not embedded state on payload nodes:

```rust
pub enum ValueFacts {
    PyObj(PyObjFacts),
}

pub struct PyObjFacts {
    pub ty: TypeFact,
    pub truthiness: TruthinessFact,
    pub none: NoneFact,
    pub bool_singleton: BoolSingletonFact,
    pub refcount: RefcountFact,
    pub provenance: ProvenanceFact,
}

impl PyObjFacts {
    pub const fn is_none(self) -> bool;
    pub const fn is_known_not_none(self) -> bool;
    pub const fn is_truthy(self) -> Option<bool>;
    pub const fn is_immortal(self) -> bool;
    pub const fn is_exact_type(self, expected: PyExactType) -> bool;
    pub const fn is_true_singleton(self) -> bool;
    pub const fn is_false_singleton(self) -> bool;
}

pub struct FactStore {
    expr_facts: HashMap<InstrKey, ValueFacts>,
    block_entry_facts: HashMap<(FunctionId, BlockLabel), EnvFacts>,
}
```

Unknown facts should be normal, not errors. Analysis errors should mean malformed
IR, such as a missing instruction ID in a phase that requires semantic IDs.

Branch narrowing belongs in a function-level CFG analysis. For example,
`if x is None:` needs different environment facts on true and false successors,
not only a fact attached to the test expression.

Unknown calls and arbitrary Python operations should conservatively invalidate
facts about globals, attributes, class dicts, and other mutable runtime state.
The first version should preserve only immediate expression facts and exact
built-in constants across such operations.

TODO: Replace stringly typed runtime singleton names like `NONE`/`TRUE`/`FALSE` with a typed runtime-symbol enum.

## Ownership Effects

Python ownership should become explicit before the JIT, but BlockPy should not
insert physical `INCREF`/`DECREF` calls. The ownership-effects pass should
consume codegen-shaped BlockPy plus `FactStore` and produce a representation
where ownership is carried by IR values and release/transfer obligations are
explicit:

```rust
enum OwnershipEffect {
    ProducesOwned { value: Value },
    ProducesBorrowed { value: Value },
    RebindLocal { local: LocalLocation, new_value: Value },
    DeleteLocal { local: LocalLocation },
    TransferToSuccessor { target: BlockLabel, value: Value },
    CleanupOnFailure { live_owned: Vec<Value> },
}
```

The exact type shape can change, but the important boundary is that codegen
should no longer rediscover refcount policy from ad hoc local vectors. The JIT
lowers ownership effects to concrete calls after representation choices are
known: SSA block params versus stack-slot mirrors, borrowed helper results,
runtime constants, immortal values, and nullable-result cleanup block shape.
Semantic refcount timing belongs to the BlockPy pass; concrete refcount calls
belong to backend lowering.

The pass needs to model normal and exceptional edges. For local stores, it must
preserve CPython's order of operations by evaluating the RHS, installing the new
binding, and only then decrefing the old binding. For calls and other failing
operations, it must attach cleanup requirements to the failure edge using the
live owned values at that point.

## SSA Locals

Semantic Python local state should become an SSA environment:

```rust
enum RefKind {
    Owned,
    Borrowed,
    Immortal,
    Unbound,
}

struct LocalBinding {
    value: Option<Value>,
    ref_kind: RefKind,
}
```

Stack slots should remain backend and ABI details, not the source of truth for
normal Python locals. They are still useful for vectorcall scratch buffers,
address-taken values, frame/debug/deopt materialization, cells and closures, and
backend spills owned by Cranelift.

Progress note: ordinary CFG-edge forwarding and explicit edge slot writes now
require an explicit `LocalEnv` binding for the source value instead of falling
back to semantic stack-slot reloads. Function-entry seeding now first builds a
`LocalEnv` snapshot from stack-backed entry bindings and then uses the same
forward/incref rules as ordinary runtime edges. Multi-target terms (`if` and
`br_table`) now clone that environment per outgoing edge so one arm's release
plan cannot mutate the sibling arm during codegen.

For assignment, the RHS is fully evaluated before rebinding, and the old binding
is decref'd after the new binding is installed:

```text
new = lower(rhs)
old = env[x]
env[x] = new
DECREF(old)
```

This preserves CPython destructor timing: destructors observe the post-store
local state.

Control-flow joins should use Cranelift block params for live local bindings.
Ownership transfers along the dynamically executed edge. Values not forwarded to
the target environment are decref'd on that edge.

Potentially failing operations need cleanup continuations that know the current
SSA environment:

```text
v = call_may_fail(...)
if v == NULL:
    cleanup(env.live_owned_values())
    return NULL
```

## Block Param Facts

`ValueFacts` alone is not enough to remove LocalEnv stack mirrors. Expression
facts answer "what is this value?" The next LocalEnv step needs transport facts
for CFG edges: "what local semantics still apply to this incoming block param?"

That information should live in JIT planning beside `FunctionLocalPlan`, not in
the general expression `value_facts` pass:

```rust
pub struct BlockParamFacts {
    pub value: Option<PyObjFacts>,
    pub binding: ParamBindingFacts,
    pub provenance: ParamProvenance,
    pub ownership: LocalRefKind,
}

pub enum ParamBindingFacts {
    DefinitelyBound,
    MaybeUnbound,
    CheckedLocalValue,
}

pub enum ParamProvenance {
    ForwardedLocal(LocalLocation),
    StackSlot(LocalLocation),
    ExceptionValue,
    Constant,
    Unknown,
}
```

The important separation is:

- `PyObjFacts` describes the value itself.
- `BlockParamFacts` describes how safe it is to consume the incoming param as a
  Python local.

Recommended re-landing order:

1. Introduce `BlockParamFacts` as a planner/runtime data shape only, with no
   behavioral change.
2. Move existing planner consumers to read `binding.param_facts`.
3. Only after that, start changing LocalEnv/codegen behavior to rely on those
   facts instead of stack-slot fallback semantics.

The consumer should be able to distinguish:

- `DefinitelyBound + CheckedLocalValue`
  Raw fast-path use is allowed.
- `MaybeUnbound`
  The consumer must preserve `UnboundLocalError` / deleted-name semantics rather
  than treating the value as an ordinary `PyObject*`.
- ownership still independently controls incref/decref policy.

This keeps checking at the consumption boundary instead of forcing the edge
forwarder to erase maybe-unbound state into a bare pointer-valued block arg.

Suggested placement:

```text
value_facts
-> ownership_effects
-> local liveness / must-bound analysis
-> block-param planning
   - choose runtime block params
   - attach BlockParamFacts
-> JIT codegen consumes BlockParamFacts
```

`compute_function_local_must_bound_ins(...)` is already the beginning of this
story. `BlockParamFacts` is the generalization from a yes/no local property to
per-edge, per-param transport semantics.

## Removing Stack Mirrors

It should be possible to remove stack-mirrored storage for normal Python locals,
but that is an architectural follow-on from SSA locals rather than a local
cleanup. The current split is:

- `LocalEnv` is the semantic local environment used by JIT codegen.
- `StackSlots` is also used as a second storage location for many Python locals.
- non-local runtime bookkeeping such as handled-exception state should continue
  using backend stack slots.

The stack-mirror removal target is therefore:

- keep stack slots for backend/runtime bookkeeping
- remove stack slots as a second home for ordinary Python locals

The work required is:

1. Make `LocalEnv` the only storage for ordinary Python locals.
   `load_location`, `load_name`, `store_location`, and `delete_location`
   should stop depending on mirrored stack slots for local semantics.

2. Carry all live-in locals through explicit block params.
   JIT planning must stop relying on stack slots as the fallback transport across
   CFG edges. Every live local that may be read in a successor block should be
   represented in the successor environment.

3. Introduce per-block-param transport facts.
   The consumer needs to know whether an incoming block param is:
   - definitely bound
   - maybe unbound
   - already checked for deleted/unbound/null
   - owned, borrowed, or immortal
   - forwarded from a local binding versus synthesized from another source

4. Remove edge-time stack-slot repair.
   Exception dispatch and ordinary block jumps currently still repair or
   materialize local state through stack slots on some paths. That must become
   explicit block-param transport instead.

Current finding: internal temp locals such as `_dp_tmp_*` still reappear as
semantic stack-slot loads in loop bodies even when the JIT planning layer shows
runtime block-param transport. First-store local-only for slot-backed locals is
still unsafe until those residual block-entry/edge paths stop reading the
semantic stack slots as the source of truth.

5. Move cleanup fully onto ownership/refcount plans.
   Generic stack-slot-wide cleanup should not be the source of truth for normal
   local lifetime. Planned ownership actions should drive decref behavior.

6. Keep exception/runtime scratch state separate.
   Slots like handled-exception tracking are still fine as backend state; they
   are not part of the "remove local stack mirrors" goal.

The practical migration order should be:

1. Add `BlockParamFacts`.
2. Make live-in locals explicit in block-param planning.
3. Teach local loads to consume block-param facts instead of inferring semantics
   from `StackMirror` versus `LocalOnly`.
4. Remove stack-slot fallbacks and edge slot-writes for normal locals.
5. Delete local stack mirrors once tests and benchmarks hold.

The recent `_dp_iter_*` bug is the concrete proof that this work is necessary:
the JIT lost loop-carried local state because some locals were still expected to
be recoverable from stack slots after earlier stores had left them only in the
semantic local environment.

Add an ownership verifier before broad rewrites. It should check that every
owned value is released or transferred exactly once on every normal and
exceptional path.

## Implementation Order

1. Stabilize instruction identity with `InstrKey` and codegen-ID validation.
2. Clarify synthetic instrumentation semantics around semantic site IDs.
3. Add a static `FactStore` sidecar and minimal fact types.
4. Infer non-CFG local facts for literals, `None`, booleans, runtime constants,
   and simple exact built-in constants.
5. Add branch transfer/narrowing for `IfTerm.test`.
6. Thread read-only `FactStore` into JIT codegen.
7. Introduce `LocalEnv` in JIT codegen without changing generated behavior.
8. Add a temporary JIT-local planning phase for SSA ownership observations.
9. Introduce a BlockPy ownership-effects pass that records semantic ownership
   transfers and cleanup requirements without emitting physical refcount calls.
10. Convert straight-line local load/store to SSA ownership in the BlockPy pass.
11. Move block params to ownership-carrying SSA values.
12. Replace stack-slot cleanup with explicit environment cleanup at returns and failure
    edges.
13. Add the ownership verifier and only then expand fact-driven codegen.

## Current Status

Started:

- `InstrKey` exists as `(FunctionId, InstrId)`.
- Codegen-shaped semantic instructions are validated for ID presence and
  uniqueness immediately after ID assignment.
- `CounterSite::Runtime` treats `instr_id` as the semantic site being observed;
  synthetic instrumentation instructions may still have no semantic ID.
- The codegen instruction enum has been split by name into `InstrCodegenOp`,
  with `InstrCodegen` retained as a compatibility alias. The next step is to
  replace the alias with an identified wrapper.
- `CodegenUnidentifiedModuleShape` now represents normalized codegen IR before
  ID assignment. `assign_module_instr_ids` consumes that shape and returns
  `CodegenModuleShape`, making the pre-ID/post-ID pass boundary explicit.
- `constant_none` has been split out of the base `Instr` trait so future
  identified codegen instructions are not required to synthesize fake semantic
  IDs.
- A nominal `IdentifiedInstr<I>` wrapper exists, but is not wired into
  `CodegenModuleShape` yet. The remaining mechanical step is to make
  `InstrCodegenOp` generic over its child instruction type instead of being
  self-recursive.
- A first `FactStore` sidecar exists and records expression facts keyed by
  `InstrKey`. Initial inference covers runtime `None`/bool singletons, module
  constant loads, and exact builtin types for string/bytes/int/float literal
  module constants, including truthiness for those literals.
- `PyObjFacts` is split into independent type, truthiness, none-identity,
  bool-singleton, refcount, and provenance dimensions. This keeps queries like
  `is_none`, `is_known_not_none`, `is_immortal`, and `is_exact_type` independent
  instead of forcing unrelated facts through one aggregate enum.
- `EnvFacts` can now carry per-local facts at block entries. The first branch
  transfer narrows `if x is None` / `if x is not None` successor entries to
  `x: None` and `x: not None` respectively.
- Branch transfer also narrows representable bool-singleton identity tests on
  the exact edge where identity is known, such as `if x is True` and the false
  edge of `if x is not False`.
- Block-entry facts are computed by a forward function-level transfer over the
  CFG. Straight-line local stores set facts for the target local, local loads
  can copy known facts to another local, and local deletes remove facts.
- A temporary JIT-side `LocalRefKind` cleanup consumer is being added so cleanup
  paths stop assuming every transient local is owned by construction. This is
  intentionally a bridge to the BlockPy refcount lowering pass, not the final
  home for refcount policy.
- JIT codegen now computes the read-only `FactStore` once per lowered module and
  exposes expression fact lookup through `JitEmitCtx`. Generated code does not
  use the facts yet.
- A first `LocalEnv` wrapper exists at the JIT per-block emission boundary.
  It now owns typed `LocalEnvEntry { location, name, value, ref_kind, storage }`
  state directly, and the older pre-`LocalEnv` expression emitter has been
  removed. JIT local codegen now has one transient-local value space.
- `LocalEnv` now tracks semantic Python locals by typed `LocalLocation`
  throughout load/store/delete lowering instead of carrying a parallel
  scratch-name representation.
- Runtime block params now enter `LocalEnv` as owned `LocalLocation` bindings at
  block entry. They are still mirrored into stack slots until failure cleanup
  can consume `LocalEnv` directly.
- `LocalEnvEntry` records whether a value is local-only or mirrored into a
  stack slot, so explicit cleanup can avoid double-decrefing transitional
  stack-slot mirrors.
- Stack-mirrored runtime block params are represented as borrowed `LocalEnv`
  entries after cloning into the stack slot. Forwarding borrowed locals to a
  successor now emits the required INCREF, so stack-slot cleanup owns the
  mirrored reference and `LocalEnv` does not leak an extra block-param owner.
- JIT terminal lowering now consults the current `LocalEnv` entries after
  terminal expression emission. Successor block-param forwarding and terminal
  cleanup no longer rely on a stale ref-kind snapshot taken at block entry.
- A first `FunctionLocalPlan` exists in JIT planning. It records per-block entry
  bindings from the storage layout, annotates them with available `EnvFacts`,
  and classifies known immortal locals without changing generated code.
- JIT specialization and counter lookup paths now require semantic instruction
  IDs for semantic codegen operations instead of silently disabling the
  optimization when an ID is missing. Synthetic test builders fill missing IDs
  explicitly, and value-fact inference ignores ID-less synthetic trace/counter
  instrumentation rather than assigning fake semantic identities.
- A first BlockPy ownership-effects sidecar exists and is computed after
  `value_facts` in the lowering driver. It records local rebind, delete, and
  cleanup ownership effects from codegen-shaped BlockPy, including stores of
  the runtime `DELETED` sentinel and immortal local facts. The Rust types still
  use the transitional `RefcountPlan` names internally, but the pass entrypoint
  and timing label are now `ownership_effects`.
- The ownership-effects plan has a verifier that replays local ownership through each
  codegen block and validates store/delete transitions plus edge and return
  cleanup actions before instrumentation or JIT codegen can consume the module.
- JIT planning now computes the same verified per-function ownership plan beside
  `FunctionLocalPlan` and makes it available through `JitEmitCtx`.
- JIT edge-release consumption has started. Normal CFG edges and exception
  dispatch edges now consume verified ownership-plan releases by clearing stack
  slots only when target liveness/params do not preserve the local. No-handler
  failure sites now route current `LocalEnv` local-only owners through per-site
  cleanup blocks before entering the existing whole-stack cleanup path.
- The current JIT refcount-plan consistency check no longer reports edge
  release gaps for stack-slot-backed locals. The remaining transition is to move
  semantic Python local ownership out of function-wide stack slots and into an
  edge-transfer-aware SSA/`LocalEnv` representation for handled exception
  dispatch and other failure paths.
- JIT physical stack slots now use `NULL` for unbound or released local state
  instead of the runtime `DELETED` sentinel. Stack-slot loads raise the deleted
  name error before exposing `NULL` as a Python value, and stack-slot
  INCREF/DECREF operations skip `NULL`.
- JIT return and successful explicit-raise terminals now also consume terminal
  ownership-plan releases. Returns clear planned stack slots to `NULL` and no
  longer scan every stack slot; explicit-raise terminals still route through
  shared failure cleanup that skips already-`NULL` slots.
- `LocalEnv` store lowering now installs the new binding before releasing any
  previous local-only owner, preserving CPython destructor-visible rebinding
  order.
- Planned stack-slot entry bindings are now also materialized into `LocalEnv`
  at block entry as borrowed stack mirrors. Normal block-body loads and
  forwarding paths can consult `LocalEnv` first instead of rediscovering those
  locals only through stack-slot fallback loads.
- Ordinary local-location loads now require a `LocalEnv` binding and no longer
  fall back to raw stack-slot loads. The remaining stack-slot fallback surface
  is limited to name-based helper and exceptional-edge transport paths.
- Local deletes now leave an explicit unbound `LocalEnv` binding instead of
  dropping the entry outright, so same-block post-delete loads keep deleted-name
  semantics without reintroducing raw stack-slot fallback for ordinary locals.
- Exception-dispatch slot writes and runtime target-arg forwarding now require
  their named sources to be present in the forwarded block-param set instead of
  reloading those semantic locals from stack slots in the dispatch block.
- Name-based `LocalEnv` loads used by owned-cell/local helper paths now also
  require an explicit `LocalEnv` binding instead of consulting semantic stack
  slots as a fallback source of truth.
- JIT local planning now has a module-level side table keyed by `FunctionId`.
  Specialized codegen, inspector debug output, and focused PyO3 tests consume
  that precomputed plan instead of recomputing per-function LocalEnv state inside
  the Cranelift builder.
- The module-level JIT local plan now has a renderer in the planning module, and
  inspector pipeline output includes it as a `jit_local_plan` step. This makes
  block params, stack-slot seeds, and edge transports inspectable without relying
  on ad hoc codegen-only debug structures.
- Required physical stack slots for JIT local state are now derived by
  `PlannedJitFunctionLocals` itself. Codegen allocates the requested slots but no
  longer recomputes that requirement from scattered plan fragments.
- Block-entry `LocalEnv` construction now goes through one materialization helper
  that takes the `PlannedJitFunctionLocals` and block index, instead of
  open-coding separate runtime-param and stack-seed binding calls in the main
  Cranelift block loop.
- The JIT local plan now also contains explicit per-block `LocalEnv`
  materialization entries. Each entry says whether the value comes from a
  runtime block param or a stack-slot load, and codegen lowers that source list
  instead of interpreting runtime params and stack-slot seeds independently.
