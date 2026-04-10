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
-> refcount ownership lowering
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

## Refcount Lowering

Refcounting should become explicit in BlockPy before the JIT. The pass should
consume codegen-shaped BlockPy plus `FactStore` and produce a representation
where ownership is carried by IR values and release/transfer points are explicit:

```rust
enum OwnershipEffect {
    TransferOwned { value: Value },
    Incref { value: Value },
    Decref { value: Value },
    ReleaseIfOwned { value: Value },
}
```

The exact type shape can change, but the important boundary is that codegen
should no longer rediscover refcount policy from ad hoc local vectors. The JIT
may still choose how to encode the operations, fold immortal decrefs, or route
cleanup through Cranelift blocks, but semantic refcount timing belongs to the
BlockPy pass.

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
9. Introduce a BlockPy refcount ownership lowering pass that emits explicit
   ownership transfers and cleanup requirements.
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
- A first `LocalEnv` wrapper exists at the JIT per-block emission boundary. It
  still delegates to the existing parallel local-name/local-value vectors, so it
  does not change ownership or generated code yet.
- A first `FunctionLocalPlan` exists in JIT planning. It records per-block entry
  bindings from the storage layout, annotates them with available `EnvFacts`,
  and classifies known immortal locals without changing generated code.
- `LocalEnv` now carries a transient local `LocalRefKind` side table. The first
  refresh point records the current invariant that transient JIT locals are
  owned references; stack-slot-backed locals still use the old path.
- JIT specialization and counter lookup paths now require semantic instruction
  IDs for semantic codegen operations instead of silently disabling the
  optimization when an ID is missing. Synthetic test builders fill missing IDs
  explicitly, and value-fact inference ignores ID-less synthetic trace/counter
  instrumentation rather than assigning fake semantic identities.
