# Value Facts And SSA Locals

## Goal

Value facts and SSA locals should be implemented as one staged pipeline change,
not as two independent systems. The shared objective is to make optimization
decisions from stable IR identity and facts, while making Python local ownership
explicit enough that refcount timing remains CPython-compatible.

The boundary should be:

```text
BlockPy passes decide names, globals, control flow, and stable semantic
instruction identity.

Value facts analyze codegen-shaped BlockPy as a read-only sidecar.

JIT planning decides SSA ownership, Cranelift block params, cleanup edges, and
stack-slot materialization.
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
8. Add a JIT-local planning phase for SSA ownership decisions.
9. Convert straight-line local load/store to SSA ownership.
10. Move block params to ownership-carrying SSA values.
11. Replace stack-slot cleanup with environment cleanup at returns and failure
    edges.
12. Add the ownership verifier and only then expand fact-driven codegen.

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
- JIT codegen now computes the read-only `FactStore` once per lowered module and
  exposes expression fact lookup through `JitEmitCtx`. Generated code does not
  use the facts yet.
- A first `LocalEnv` wrapper exists at the JIT per-block emission boundary. It
  still delegates to the existing parallel local-name/local-value vectors, so it
  does not change ownership or generated code yet.
- A first `FunctionLocalPlan` exists in JIT planning. It records per-block entry
  bindings from the storage layout, annotates them with available `EnvFacts`,
  and classifies known immortal locals without changing generated code.
