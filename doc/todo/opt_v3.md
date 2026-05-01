---
title: "Optimizer v3"
---

# Optimizer v3

This plan refines the `opt_v2` idea around one hard requirement:

> The full optimization result must be a data structure consumed by codegen.
> Codegen should be dumb and mechanical: emit the selected operation shapes,
> guards, fallbacks, deopt points, conversions, and ownership actions. It
> should not rediscover semantic decisions from profile counters, value facts,
> or ad hoc helper recognition.

The plan is written in two layers:

1. The model in isolation, ignoring the current implementation.
2. A follow-up migration path that incorporates existing SOAC work.

Updated direction:

- Optimizer v3 must not consume the legacy optimization plan as input. Legacy
  `mod.opt` is no longer a runtime fallback or a planner output for the normal
  optimization pipeline.
- The intended v3 pipeline is:

  ```text
  cached unoptimized BlockPy module + raw profile evidence
    -> v3 planning
    -> typed v3 plan
    -> JIT codegen consumes the selected mechanical shape
  ```

- This may temporarily reduce optimization coverage. That regression is
  acceptable because it prevents v3 from inheriting site-local legacy decisions
  and makes entanglement visible.


## Goals

- Represent optimizations as explicit alternatives over a typed semantic IR.
- Choose a region plan using facts, profile evidence, and a cost model.
- Preserve CPython-visible behavior by default.
- Make intentional behavior changes explicit in the selected plan.
- Keep codegen mechanical enough that plan validation can happen before codegen.
- Make optimization diagnostics structured: the selected plan should explain why
  it chose or declined each alternative.

Non-goals:

- Do not search for a globally optimal Python program. The search space is too
  large once Python side effects, exceptions, destructors, and dynamic dispatch
  are included.
- Do not hide optimization behavior in late Cranelift rewrites. CLIF-level
  optimization should clean up already selected plans, not choose Python
  semantics.
- Do not treat annotations or profile observations as unconditional facts.


## Implementation Status

The current implementation uses a v3 planner path. Shared plan and mechanical
emission types live in `soac_opt`, the extraction/planning pipeline lives in
`soac_opt`, and runtime `verify`/`apply` plus offline precompile build v3
decisions from raw profile evidence and cached pre-optimization BlockPy modules.
The old live bridge that reconstructed v3 artifacts from legacy `mod.opt` has
been removed, and v3 no longer consumes legacy optimization decisions or
serialized optimization-plan cache artifacts.

Implemented:

- `soac_opt::plan_v3`: full plan schema, conversion
  signatures, operation signatures, replay-safety validation, and structural
  validation.
- `soac_opt::alternatives_v3`: default catalog entries for
  generic Python add/sub/mul/bitwise/all-rich-compare, exact compact-int
  add/sub/mul/bitwise/all-rich-compare, truthiness, and materialization.
- `soac_opt::region_v3`: conservative branch/return region
  extraction that preserves evaluation order, records store targets and simple
  jump continuations for store-RHS regions, and declines unsupported blocks
  explicitly.
- `soac_opt::planner_v3`: bounded planner for
  exact-compact-`int` direct comparison branches, `a + b > 0` branches,
  `return a + b`/`return a - b`/`return a * b` arithmetic returns,
  `return a & b`/`return a | b`/`return a ^ b` bitwise returns, and
  `return a < b` comparison returns, producing hot and local-fallback
  `RegionPlan`s. It also records ordinary profiled direct-call selections from
  `call_hot_targets`, including cross-module targets present in the cached
  module set, and guarded receiver-method selections from
  `Call(GetAttr(receiver, method), ...)` profile evidence. Constant-attribute
  indexed-field selections are planned from raw `type_keys` layout evidence and
  lowered `GetAttr`/`SetAttr` sites. Exact-list item selections are planned from
  raw `getitem_hot_shapes`/`setitem_hot_shapes` evidence and lowered
  `GetItem`/`SetItem` sites.
- `soac_opt::emit_v3`: validation-gated mechanical
  emitter over selected v3 plan nodes and exits. Function-level direct-call
  selections, method-call selections, exact-list item selections, and
  indexed-field selections are also emitted as mechanical decisions.
- `soac_opt::evidence_v3`: bridge from existing
  `FunctionProfileEvidence` exact-int operator shapes and lowered integer
  module constants into v3 planner facts.
- `soac_opt::pipeline_v3`: exact-int pipeline that composes
  extraction, evidence, planning, validation, and mechanical emission.
- JIT `verify`/`apply` loading consumes raw profile evidence, validates module
  identity, and splits in-memory module-level artifacts into per-function
  mechanical artifacts.
- Offline precompile consumes raw profile evidence and cached pre-optimization
  BlockPy modules directly.
- `FunctionSpecializationInputs`: carries validated exact-int branch v3
  artifacts into the JIT build path, where codegen validates that the planned
  function identity matches the function being compiled. Same-module profiled
  direct-call decisions and ordinary-call argument plans are derived from
  mechanical v3 emission as v3-owned codegen inputs; the JIT consumes the
  emitted call plan through mechanical typed-call lowering and uses its targets
  only for direct-call function predeclaration, module-plan direct-call rewrites,
  precompile target lookup, and process-JIT batch scheduling. When a call source
  has an emitted v3 direct, constructor, or method call node, that source is
  v3-owned; guardless or early-consumed sources remain generic fallbacks instead
  of being replanned from another evidence path. Direct-call emission rejects a
  selected source that is absent from the lowered typed function.
  Method-call decisions are also derived from mechanical v3 emission as v3-owned
  codegen inputs. They carry owner type, method name, receiver
  type-version guard kind, original-call fallback kind, and an argument plan
  with the implicit receiver represented explicitly. Specialization-input
  preparation validates the emitted method plan, resolves the current owner
  type/attribute when possible, and records either prepared typed guards or an
  owned source with no guards. Preparation also validates the exact type and
  callable relocations that typed lowering will import, and process-JIT
  reservation predeclares those imports before worker codegen receives the
  function. Typed-call lowering then mechanically lowers prepared guards and
  leaves guardless v3-owned sources as their original generic call.
  Indexed-field
  decisions are also derived from mechanical v3 emission as v3-owned
  typed-attribute inputs, with exact plan/emission validation before use.
  Exact-list item decisions are derived from mechanical v3 emission as
  v3-owned item-operation inputs, with lowered `GetItem`/`SetItem` validation
  before the existing list fast-path emitter uses them.
- JIT term lowering now consumes matching exact-int direct-compare branch,
  add/compare branch, add/sub/mul/bitwise-return, and comparison-return
  artifacts by interpreting the mechanical hot region and its local generic
  fallback. Unsupported or absent v3 regions continue through the existing
  lowering path.
- JIT term lowering no longer consumes a scalar-thread sidecar. Generic typed
  locals carry unboxed scalar values through runtime block params, and
  mechanical exact-int regions pre-seed matching i64 conversion outputs from
  scalar `LocalEnv` bindings when a later branch reloads that local.

Partially migrated families are also intentionally visible:

- profiled ordinary-function direct calls are represented as v3 plan selections
  plus mechanical direct-call emissions with validated argument plans.
  Cross-module targets are represented through serialized module identities and
  resolved from the loaded module set. Module-level ordinary direct-call CFG
  rewrites are now fed only by v3 emitted inline direct-call plans. Runtime
  receiver-method and constructor calls are no longer placeholder v3 plan
  families; they stay generic unless a future plan format adds validated static
  guard inputs for them.
- indexed fields are represented as v3 plan selections plus mechanical
  indexed-field emissions. `soac_jit` now keeps the emitted access kind and
  attribute name separate and validates that they match the lowered
  `GetAttr`/`SetAttr` site while loading
  v3 artifacts, before constructing typed/codegen inputs. By-attribute layout
  availability is still shared with the existing constructor initializer fast
  path, but selected v3 indexed-field inputs must now resolve to usable
  owner-type/version guards before codegen. The final inline load/store
  emission still uses the existing typed attribute emitter rather than a
  v3-specific operation node.
- indexed globals are represented as v3 plan selections plus mechanical
  indexed-global emissions with explicit module-dict guard and original global
  fallback effects. The JIT validates emitted global name/access/index against
  lowered `NameLocation::Global(slot)` and consumes the v3 entry as the
  indexed global load/store input. Raw indexed-global profile maps remain
  serialized evidence for planning, but are no longer consumed directly by JIT lowering.
  The final helper-call builder remains in the existing global load/store
  emitters as a mechanical lowering helper for the validated v3 global node.
- exact-list getitem/setitem are represented as v3 plan selections plus
  mechanical exact-list item emissions with explicit exact-list/exact-compact-int
  in-bounds guard and original item-access fallback effects. The JIT validates
  emitted access kind against lowered `GetItem`/`SetItem` sites and consumes the
  v3 entry directly as the item-specialization input, without translating it
  through hot-shape maps. Raw item hot-shape maps remain serialized profiling
  evidence for planning, but are no longer consumed directly by JIT lowering.
  The final inline list load/store builder remains in the operation-specialization
  module as a mechanical lowering helper for the validated v3 item node.

Remaining scalar cleanup:

- The compact-int region-step emitter is mechanical once a v3 region is
  selected. The previous adjacent store-RHS -> empty compare scalar-thread
  recognizer has been deleted; remaining scalar cleanup should build on the
  generic typed-local path rather than reintroducing code-shape fusions.

Branch locality and cold block hints remain layout metadata for now, not v3
semantic plan targets.

Current integration target:

- Expand v3 coverage while keeping the typed v3 plan as the source of truth;
  there is no serialized optimization-plan artifact between profiling and
  codegen.
- Continue deleting legacy optimization-plan code now that runtime consumption
  and normal offline generation are v3-only.

Typed-local scalar notes:

- Treat scalar value propagation as a generic typed-local dataflow property,
  not as a special case in codegen. Producers bind locals in `LocalEnv` with a
  concrete representation, edge planning carries that representation through
  runtime block params, and boundary operations request explicit conversion
  only when a Python object is actually required.
- Regression tests for further scalar propagation should assert structured
  typed-local facts, runtime block-param representations, and selected v3
  branch/return plans, not rendered CLIF text or one-off code shapes.


## Isolated Model

### Semantic Program

Start from a lowered program where Python evaluation order is explicit:

```text
c = BinOp(Add, a, b)
cmp = BinOp(Gt, c, 0)
truth = Truthy(cmp)
if truth:
  return True
```

Before planning, convert each optimizable area to a small linearized region:

```text
v0 = load a
v1 = load b
v2 = add(v0, v1)
v3 = const 0
v4 = gt(v2, v3)
v5 = truthy(v4)
branch v5
```

Linearization is not a semantic transform by itself. It is a planning view that
names values, preserves evaluation order, and marks the source locations where
fallback or deopt may resume. The original CFG remains the authority for Python
control flow until a validated plan chooses explicit CFG changes.

### Values And Representations

Each SSA value has one semantic identity and zero or more physical
representations available at a program point.

```rust
struct ValueState {
    semantic: SemanticValueId,
    facts: ValueFacts,
    reps: SmallVec<[AvailableRep; 3]>,
}

enum Rep {
    PyObjectOwned,
    PyObjectBorrowed,
    PyObjectImmortal,
    I32Bool01,
    I64,
}

struct AvailableRep {
    rep: Rep,
    producer: PlanNodeId,
    ownership: OwnershipState,
    facts: ValueFacts,
    cost_to_use: Cost,
}
```

Facts describe what is known about the semantic value. Representation describes
how it is currently carried.

Examples:

- A Python `bool` object is `Rep::PyObjectImmortal` with exact-bool facts.
- A branch condition after truth normalization is `Rep::I32Bool01`.
- A compact integer unboxed from a Python `int` is `Rep::I64` with range facts
  and a dependency on the guard that proved it compact.
- A Python `int` object and an unboxed `i64` can represent the same semantic
  value, but they have different ownership and materialization costs.

### Operation Alternatives

Every optimizable semantic operation exposes legal lowering alternatives:

```rust
struct LoweringAlternative {
    id: AlternativeId,
    op: SemanticOpKind,
    input_reps: SmallVec<[RepRequirement; 3]>,
    output_rep: Rep,
    required_facts: FactsPredicate,
    output_facts: FactsTransform,
    specialization_guards: SmallVec<[Guard; 3]>,
    failure_replay: FailureReplayPolicy,
    failure: FailureMode,
    cost: CostModel,
}
```

The planner selects alternatives. Codegen emits the selected alternative only.

Guards are for optional specialization checks, not for required Python
semantics. If the selected implementation of `ord(x)` needs to reject a
non-unicode object or a unicode string with length other than one, that is part
of the `ord` implementation and may raise the same exception CPython would
raise. It should not be represented as a separate guard node whose miss behaves
like a speculative optimization miss.

Specialization guards protect optional fast paths. If an exact-compact-int guard
misses, the plan falls back to generic Python dispatch or deopts to a replay
point that will run generic Python dispatch.

The initial guard model can therefore stay narrow:

```rust
enum GuardKind {
    SpecializationCheck,
}
```

Failure mode is part of the alternative:

```rust
enum FailureMode {
    CannotFail,
    Raise(PythonExceptionSpec),
    FallbackToPlan {
        target: PlanNodeId,
        reason: FallbackReason,
    },
    DeoptTo {
        target: DeoptPointId,
        reason: DeoptReason,
    },
}
```

This prevents two common unsoundnesses:

- treating a required semantic check as though it were an optional profile
  guard;
- hiding why a failed specialization can deopt instead of taking a local
  fallback path.

Guard-node failure should only choose fallback or deopt. Raising belongs to the
operation implementation selected by the plan, not to a specialization guard.

### Effects And Ownership

The full plan may eventually need a detailed effect summary, but do not add a
large effect lattice before a concrete planner needs it. Start with the effect
property that is already necessary for deopt correctness: whether a failure can
be replayed without duplicating visible side effects.

```rust
struct FailureReplayPolicy {
    replay: FailureReplayKind,
    reason: ReplayReason,
}

enum FailureReplayKind {
    SafeToReplay,
    MustUseLocalFallback,
}
```

Deopt is only valid when the selected alternative explicitly records why the
deopt point can resume without replaying visible side effects. Otherwise the
alternative must use a local fallback path or decline. The reason should be
visible in the plan and diagnostics, for example:

- the guard happens before the operation and before any owned value is consumed;
- the helper contract says failure happens before side effects;
- a replay-safety scan proved all operands are reusable at the deopt point;
- the site is not replay-safe, so the selected plan uses local fallback.

A richer `EffectSpec` can be added later when a planner needs to distinguish
allocation, Python calls, DECREF/destructor behavior, or mutation observation.
Until then, avoid adding effect fields that are only speculative.

Ownership is also plan data:

```rust
struct OwnershipAction {
    value: SemanticValueId,
    action: OwnershipActionKind,
    source: OwnershipSource,
}

enum OwnershipActionKind {
    Incref,
    Decref,
    TransferOwned,
    BorrowLocal,
    MaterializeOwned,
}
```

Codegen should emit ownership actions selected by the plan, not infer cleanup
policy from local syntax after specialization.

### Conversions

Conversions are normal plan nodes with costs and validation rules:

```rust
enum ConversionKind {
    FromPythonLongCompactToI64,
    ToPythonLongOwned,
    ToPythonBoolImmortal,
    TruthinessToI32Bool01,
}
```

Examples:

- `PyObjectBorrowed exact PyLong compact -> I64` requires a specialization guard
  unless dominated facts already prove exact compact int.
- `I64 -> PyObjectOwned int` allocates or uses a CPython helper and can raise.
- `I32Bool01 -> PyObjectImmortal bool` selects `Py_True` or `Py_False` and does
  not allocate.
- `PyObject -> I32Bool01` consumes `PyObject_IsTrue`'s `-1` sentinel locally;
  the sentinel must not escape as a value representation.

Every conversion kind should have a validation entry that defines:

- allowed input representation and facts;
- required dominating facts or specialization guards;
- output representation and output facts;
- whether failure can raise, fall back, or deopt;
- ownership transfer or materialization behavior.

The validator should reject plans where a conversion does not make sense, such
as `I32Bool01 -> I64` without a defined widening conversion, compact-long
unboxing without either facts or a guard, or a fallible conversion whose failure
mode is not explicit.

### Cost Model

Cost is expected cost over observed workload, not local instruction count:

```text
expected_cost =
  hot_count * fast_path_cost
  + miss_count * miss_cost
  + deopt_count * deopt_cost
  + materialization_cost
  + ownership_cost
  + allocation_cost
  + code_size_weight * estimated_code_bytes
  + compile_cost_weight * estimated_compile_cost
```

The initial costs can be heuristic. The important requirement is structural:
every candidate has comparable cost fields, and diagnostics can show why one
candidate won.

### Region Planning

Use dynamic programming over a bounded region:

1. Enumerate available reps for inputs from facts and profile evidence.
2. Visit operations in linearized evaluation order.
3. For each operation, enumerate legal alternatives whose requirements can be
   satisfied by available reps plus explicit conversions.
4. Carry a small frontier of cheapest states per semantic value and output
   demand.
5. At region exits, choose the cheapest state satisfying the boundary demand:
   branch `I32Bool01`, return `PyObjectOwned`, generic helper argument
   `PyObjectBorrowed/Owned`, or deopt-resume environment.

Do not attempt an unbounded whole-function search at first. Handle joins by
materializing to the required boundary representation or by carrying explicit
phi-compatible reps only when both predecessors have the same proven state.

### Scalar Local Threading

Scalar local threading is a cross-region rewrite, not a late codegen peephole.
The plan may state that a scalar result produced by one region can replace a
later scalar consumer of the same local:

```text
producer:  c = exact_i64_add(a, b)
consumer:  exact_i64_compare(c, 0)
rewrite:   use producer.i64 instead of materialize(c) + load(c) + unbox(c)
```

The selected plan must make the safety boundary explicit:

- the producer and consumer region values have the same scalar representation;
- the local identity is the same resolved storage location, not only the same
  spelling;
- fallback is local and explicit, because a guard miss or overflow must execute
  the original store before any later comparison or exception path observes the
  local;
- materialization is deferred only until a Python object use requires it;
- validation rejects missing producer/consumer values, mismatched reps,
  duplicate consumers, unknown fallback regions, and missing reasons.

Live lowering now handles this through typed locals rather than a scalar-thread
plan: the producer binds `c` as an unboxed scalar on the hot path, edge planning
carries that representation as an `ExactI64` runtime block param, and the later
exact-int region pre-seeds the matching local unbox output from `LocalEnv`.


## Optimization Plan Data Structure

The output of planning should be serializable and independently validatable.

```rust
struct ModuleOptimizationPlanV3 {
    module_identity: ModuleIdentity,
    functions: Vec<FunctionOptimizationPlanV3>,
    helper_catalog_version: HelperCatalogVersion,
    cost_model_version: CostModelVersion,
}

struct FunctionOptimizationPlanV3 {
    function: FunctionIdentity,
    regions: Vec<RegionPlan>,
    direct_calls: Vec<DirectCallSpecializationPlan>,
    exact_list_items: Vec<ExactListItemSpecializationPlan>,
    indexed_fields: Vec<IndexedFieldSpecializationPlan>,
    indexed_globals: Vec<IndexedGlobalSpecializationPlan>,
    deopt_points: Vec<PlannedDeoptPoint>,
    ownership: FunctionOwnershipPlan,
    diagnostics: Vec<PlanDiagnostic>,
}

struct RegionPlan {
    region_id: RegionId,
    source: RegionSource,
    inputs: Vec<RegionInput>,
    nodes: Vec<PlanNode>,
    exits: Vec<RegionExitPlan>,
}

enum PlanNode {
    LoadInput(LoadInputNode),
    Constant(ConstantNode),
    Convert(ConvertNode),
    Guard(GuardNode),
    Operation(OperationNode),
    Materialize(MaterializeNode),
    Fallback(FallbackNode),
    Deopt(DeoptNode),
    Ownership(OwnershipNode),
}

struct OperationNode {
    semantic_op: SemanticOpId,
    alternative: AlternativeId,
    inputs: Vec<PlanValue>,
    output: Option<PlanValue>,
    failure_replay: FailureReplayPolicy,
    cost: ChosenCost,
}
```

The plan should be rich enough that codegen can perform a simple match:

```text
for region in function.regions:
  for node in region.nodes:
    emit_node(node)
```

Codegen is allowed to allocate registers, blocks, and Cranelift variables. It is
not allowed to decide:

- which specialization is legal;
- whether a guard should fallback or deopt;
- whether a result should stay unboxed or be materialized;
- whether an operation can skip Python dispatch;
- whether ownership cleanup is needed;
- whether an overflow changes behavior.

Those choices belong in planning and validation.


## Example Plan Shape

For:

```python
def add(a, b):
    c = a + b
    if c > 0:
        return True
```

the hot exact-compact-int plan should look conceptually like:

```text
region entry:
  va_obj = input a as PyObjectBorrowed
  vb_obj = input b as PyObjectBorrowed

  guard g0: exact PyLong compact(va_obj), specialization miss -> fallback_region
  guard g1: exact PyLong compact(vb_obj), specialization miss -> fallback_region
  va_i64 = convert FromPythonLongCompactToI64(va_obj)
  vb_i64 = convert FromPythonLongCompactToI64(vb_obj)
  vc_i64 = op CheckedI64Add(va_i64, vb_i64), overflow -> fallback_region
  zero = const I64(0)
  cmp_i32 = op I64GtToBool01(vc_i64, zero)
  exit branch on cmp_i32

fallback_region:
  c_obj = op PyNumberAdd(va_obj, vb_obj)
  zero_obj = materialize PyLong(0)
  cmp_obj = op PyObjectRichCompare(c_obj, zero_obj, Py_GT)
  truth_i32 = convert TruthinessToI32Bool01(cmp_obj)
  exit branch on truth_i32
```

The optimized region does not materialize `c` or `cmp` as Python objects because
the branch exit demands `I32Bool01`. If a later Python-observable boundary needs
`c`, the plan must either materialize it or choose a different representation.


## Outline Of Work

1. Define the v3 plan schema.
   - Add Rust types for module/function/region/node plans.
   - Include guards, failure modes, effects, costs, ownership actions, and
     diagnostics.
   - Add validation that every node input is produced, every failure target is
     valid, and every region exit satisfies its demand.

2. Define the helper and alternative catalog.
   - Model generic Python operations, exact-slot helpers, compact-int machine
     operations, truthiness, and conversions.
   - Give each alternative explicit input requirements, output facts, effects,
     failure mode, and initial heuristic cost.

3. Build the linearized region view.
   - Start with straight-line expression trees feeding one branch or return.
   - Preserve evaluation order.
   - Mark replay-safe and replay-unsafe boundaries.
   - Record source `InstrId`s for diagnostics and deopt mapping.

4. Implement a bounded region planner.
   - Carry a small frontier of representation states.
   - Insert conversions explicitly.
   - Select a cheapest plan satisfying the region exit demand.
   - Emit structured diagnostics for selected and declined alternatives.

5. Keep optimizer inputs direct and in-memory.
   - The old cache artifact, standalone writer, and serialized-artifact
     inspector are gone.
   - Keep runtime and precompile on raw profile evidence plus cached
     pre-optimization BlockPy modules.

6. Make codegen consume the plan mechanically.
   - Add an emitter for v3 plan nodes.
   - Keep generic legacy emission available only as an explicit fallback node.
   - Add assertions that codegen received a complete plan for every v3 region.

7. Integrate profile evidence.
   - First consume raw per-site evidence directly from profile counters.
   - Consume raw profile evidence directly; the legacy `mod.opt` plan model has
     been deleted.
   - Then add correlated region evidence where local evidence cannot choose a
     sequence reliably.
   - Feed verify/apply hit, fallback, and deopt counts back into diagnostics.

8. Extend across CFG joins and direct calls.
   - Add conservative representation merging at joins.
   - Add direct-call variants only after single-function regions are stable.
   - Key compiled variants by function identity, rep signature, and assumptions.

9. Replace old site-local specializations incrementally.
   - Migrate exact-int operators first.
   - Then truthiness and materialization.
   - Direct calls are partially migrated through mechanical v3 typed lowering,
     including cross-module ordinary-function targets and guarded receiver
     methods. Simple ordinary-function `DirectCall` bodies and no-argument
     receiver-method `DirectCall` bodies have explicit CFG rewrites; remaining
     method/constructor guarded typed nodes still need expansion. Indexed fields
     and indexed globals are partially migrated as v3-owned codegen inputs; lift
     their actual lowering into mechanical nodes next.
   - Then getitem/setitem.
   - Keep old paths until each replacement has structured tests and diagnostics.

10. Benchmark and document kept performance changes.
   - Use focused specialization tests before full benchmarks.
   - For accepted performance changes, run the repo benchmark workflow and log
     the result in the optimization log.


## Detailed First Step

The first implementation step should be schema-only plus validation. It should
not change generated code.

Deliverable:

- A new optimizer-plan module, for example `crates/soac_opt/src/plan_v3.rs`
  or a new shared crate module if the existing crate boundaries make that
  cleaner.
- Data types for:
  - `ModuleOptimizationPlanV3`
  - `FunctionOptimizationPlanV3`
  - `RegionPlan`
  - `PlanNode`
  - `PlanValue`
  - `Rep`
  - `ValueFacts` references or a small v3 fact wrapper
  - `GuardKind`
  - `GuardFailure`
  - `FailureMode`
  - `FailureReplayPolicy`
  - `Cost`
  - `PlanDiagnostic`
- A validator for structural invariants.
- Unit tests that construct small plans by hand and validate success/failure.

Concrete scope:

1. Add representation and value IDs.

   ```rust
   #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
   pub struct PlanValueId(pub u32);

   #[derive(Clone, Copy, Debug, Eq, PartialEq)]
   pub enum Rep {
       PyObjectOwned,
       PyObjectBorrowed,
       PyObjectImmortal,
       I32Bool01,
       I64,
   }

   #[derive(Clone, Debug, Eq, PartialEq)]
   pub struct PlanValue {
       pub id: PlanValueId,
       pub rep: Rep,
   }
   ```

2. Add node forms without committing to every future operation.

   ```rust
   pub enum PlanNode {
       Input { output: PlanValue },
       Constant { output: PlanValue, constant: PlannedConstant },
       Convert { input: PlanValue, output: PlanValue, kind: ConversionKind },
       Guard { inputs: Vec<PlanValue>, guard: GuardSpec, failure: GuardFailure },
       Operation { inputs: Vec<PlanValue>, output: Option<PlanValue>, op: PlannedOp },
       Materialize { input: PlanValue, output: PlanValue, kind: MaterializeKind },
       Fallback { target: FallbackTarget },
       Deopt { target: DeoptPointId },
       Ownership { action: OwnershipAction },
   }

   pub enum GuardFailure {
       FallbackToPlan { target: PlanNodeId, reason: FallbackReason },
       DeoptTo { target: DeoptPointId, reason: DeoptReason },
   }
   ```

3. Add minimal failure-replay metadata.

   ```rust
   pub struct FailureReplayPolicy {
       pub replay: FailureReplayKind,
       pub reason: ReplayReason,
   }

   pub enum FailureReplayKind {
       SafeToReplay,
       MustUseLocalFallback,
   }
   ```

   Do not add a full `EffectSpec` until a concrete planner needs the extra
   dimensions. The initial schema only needs enough information to validate
   whether a failed specialization may deopt or must use local fallback.

4. Add validation rules.

   - Node outputs must be unique within a region.
   - Node inputs must refer to prior outputs or declared region inputs.
   - Region exits must refer to available values.
   - A branch exit must use `I32Bool01`.
   - A return exit must use `PyObjectOwned`.
   - Guard failure targets must exist.
   - Deopt nodes must reference declared deopt points.
   - `DeoptTo` failure is invalid unless the alternative records
     `FailureReplayKind::SafeToReplay` and a non-empty replay reason.
   - `FallbackToPlan` failure is required when the replay policy is
     `MustUseLocalFallback`.
   - Conversion nodes must match a declared conversion rule for input rep,
     output rep, required facts or guards, failure mode, and ownership effect.
   - `Ownership` actions must reference available values.

5. Add tests.

   - A valid compact-int branch plan validates.
   - A branch on `PyObjectOwned` fails.
   - A conversion using an undefined input fails.
   - A conversion with mismatched input/output reps fails.
   - A duplicate output value fails.
   - A deopt failure without a replay-safe reason fails.
   - A return using `I64` without materialization fails.

6. Add rendering only for diagnostics, not as behavior.

   A compact debug renderer is useful for failed tests and plan dumps, but tests
   should assert structured validation errors rather than exact rendered text.

Out of scope for the first step:

- No profile integration.
- No codegen emission.
- No migration of existing exact-int operator specialization.
- No new benchmark claim.
- No behavior change.


## Separable Workstreams

These workstreams are intended to be assigned to independent agents with low
merge-conflict risk. Each workstream should own a narrow file set.

### A. Plan Schema And Validation

Ownership:

- `crates/soac_opt/src/plan_v3.rs`
- module export wiring
- unit tests colocated with the new module

Output:

- Core v3 data types.
- Structural validator.
- Hand-built tests.

Conflict risk:

- Low if other agents do not edit the new v3 schema file.

### B. Alternative Catalog

Ownership:

- a new file such as `crates/soac_opt/src/alternatives_v3.rs`
- catalog-only tests

Output:

- `LoweringAlternative`
- `ConversionKind`
- initial exact-int add/compare/truthiness alternatives
- initial heuristic costs

Conflict risk:

- Low after the schema lands. This work depends on the schema types but can stay
  in a separate file.

### C. Linearized Region Builder

Ownership:

- a new file such as `crates/soac_lowering/src/passes/optimization_regions.rs`
- focused tests in `soac_lowering`

Output:

- Straight-line region extraction for expression trees feeding branch/return.
- Evaluation-order-preserving region inputs and exits.
- Source `InstrId` mapping.

Conflict risk:

- Medium. This touches lowering pass exports, but can avoid existing codegen and
  JIT files.

### D. Planner Prototype

Ownership:

- a new file such as `crates/soac_opt/src/planner_v3.rs`
- tests that use hand-built regions and the alternative catalog

Output:

- Bounded dynamic-programming planner.
- Explicit conversion insertion.
- Chosen-plan diagnostics.

Conflict risk:

- Low after schema and catalog are stable. It should not edit codegen.

### E. Mechanical Codegen Emitter

Ownership:

- a new file such as `crates/soac_jit/src/jit/plan_v3_emitter.rs`
- minimal wiring in `crates/soac_jit/src/jit/mod.rs`

Output:

- Emitter for a tiny subset of plan nodes.
- Assertions that all semantic decisions are present in the plan.
- No fallback to local decision-making except through explicit `Fallback` nodes.

Conflict risk:

- Medium. This touches JIT wiring and should start only after schema validation
  and a small planner output are stable.

### F. Profile Evidence

Ownership:

- counter dump/profile modules
- new tests around profile decoding

Output:

- Existing per-site evidence converted into planner evidence.
- Later correlated region evidence format.

Conflict risk:

- Medium. Counter formats are shared; this should be staged after schema work so
  it can target stable evidence structs.

### G. Documentation And Diagnostics

Ownership:

- `doc/SPECIALIZATION.md`
- planner diagnostics renderer
- targeted tests for diagnostics structures

Output:

- Documentation for what evidence is recorded, what plan shape is emitted, and
  where codegen remains mechanical.

Conflict risk:

- Low to medium. It should follow implementation milestones rather than race
  ahead of them.


## Incorporating Existing SOAC Work

The current implementation already contains useful pieces. The v3 migration
should absorb them rather than restart.

- Existing typed result demands are the first boundary-demand system. Branch
  tests demand `I32Bool01`; returns demand owned `PyObject`.
- Existing `SoacValue` and integer facts are close to the desired codegen value
  representation. The v3 plan should decide when those values appear.
- The old legacy optimization plan was a per-instruction precursor. V3 extends
  that idea from instruction replacement to region plans with explicit nodes,
  and the precursor model has now been removed from production code.
- Existing exact-int operator specialization is the first migration target
  because it already contains the core decision that v3 wants to move out of
  codegen.
- Existing direct-call CFG rewrites prove that SOAC can validate and replan after
  explicit CFG changes. V3 should use the same principle, but with a plan data
  structure as the source of truth.

Suggested migration order:

1. Land the v3 schema and validator with no behavior change.
2. Build a hand-authored v3 plan for the compact-int branch example and render it
   in a diagnostic path only.
3. Add the region builder for the same shape.
4. Add a planner that selects the same hand-authored shape from alternatives.
5. Add a mechanical emitter behind an off-by-default flag for that exact shape.
6. Compare emitted code against the existing exact-int path.
7. Replace the old exact-int path only when the v3 plan can express the same
   guards, fallbacks, deopt behavior, ownership, and diagnostics.
