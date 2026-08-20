---
title: "Virtual Object Lowering"
---

# Virtual Object Lowering

## Goal

Split the current constructor-object optimization path into three distinct
stages:

1. An object virtualization pass that recognizes virtual objects and records
   explicit virtual field state.
2. A virtual-to-locals lowering pass that replaces virtual fields with ordinary
   locals and block parameters, materializing real Python objects only at
   escape and deopt boundaries.
3. Ordinary scalar and value optimization passes that operate only on normal
   locals and control flow, without knowing that some locals originally came
   from object fields.

The intended result is that object reasoning becomes one bounded analysis
problem, while the rest of the optimizer sees an ordinary scalar program.

## Previous Shape

Before this change, the typed pipeline mixed three responsibilities:

- `TypedConstructorFieldBindings` records constructor-initialized fields.
- `TypedFieldScalarState` propagates aliases and field values through the CFG.
- `TypedVirtualConstructorPlan` removes the original materialized object once
  enough uses have been rewritten.

That worked for the earlier hot-constructor cases, but it coupled object
virtualization to scalar propagation. Later scalar passes did not receive an
ordinary local program; instead, the object-specific pass already had to know
how to propagate field values through aliases, joins, and hot continuations.

The desired split is:

```text
inlined object-shaped InstrTyped
  -> object virtualization analysis
  -> virtual-to-locals lowering
  -> ordinary typed scalar/value optimization
```

## Target Contracts

### 1. Object Virtualization Analysis

Input:

- Ordinary `InstrTyped` after inlining and the earlier cleanup passes that make
  object shape visible.

Output:

- A validated sidecar plan describing:
  - which allocations are virtualizable;
  - each virtual object's stable identity;
  - field state on each block entry and outgoing edge;
  - local aliases of each virtual object;
  - required materialization boundaries;
  - exact owner-type facts that were proven from the virtualized origin.

This pass understands objects, fields, aliases, and escapes. It does not rewrite
the IR into scalar form.

### 2. Virtual-To-Locals Lowering

Input:

- The typed function plus the virtualization plan.

Output:

- Ordinary typed IR in which:
  - virtual fields are represented by normal locals;
  - loop-carried and join-carried field values are represented by block
    parameters and edge arguments;
  - exact field reads and writes on virtual receivers are rewritten into local
    loads and stores;
  - real object materialization appears only at explicit escape or deopt
    boundaries.

This is the only pass that consumes object-specific state and turns it into
ordinary scalar control flow.

### 3. Ordinary Scalar And Value Passes

Input:

- Ordinary typed IR after virtual-to-locals lowering.

Output:

- Further optimized ordinary typed IR.

These passes should reason about locals, block parameters, value facts, and
ownership, but not about virtual objects, constructor bindings, alias roots, or
field maps.

## Proposed Data Model

The new object-aware artifact should use object identities that are independent
of whichever local currently names the object.

```rust
struct TypedVirtualizationPlan {
    objects: Vec<VirtualObject>,
    block_in: HashMap<BlockLabel, VirtualState>,
    edge_out: HashMap<VirtualEdge, VirtualState>,
    boundaries: Vec<VirtualBoundary>,
}

struct VirtualObject {
    id: VirtualObjectId,
    origin: InstrId,
    owner_type: TypedAttrOwnerRef,
    fields: Vec<VirtualField>,
    materialization: VirtualMaterializationRecipe,
}

struct VirtualState {
    aliases: HashMap<LocalLocation, VirtualObjectId>,
    fields: HashMap<(VirtualObjectId, VirtualFieldId), ResolvedName>,
}
```

The important shift is from "root local plus alias repair" to a stable
`VirtualObjectId`. Deleting or rebinding the original constructor-result local
should not require re-rooting the whole analysis state.

## Object Virtualization Analysis

### Recognition

Start by reusing the current constructor-field discovery path, but require a
sound materialization recipe before declaring an object virtualizable.

Candidate requirements:

- The allocation site is known.
- The constructor body has been inlined enough to recover the object's initial
  field state.
- The object has an exact known owner type where field specialization depends
  on it.
- Reconstructing the object later does not require rerunning arbitrary
  user-visible Python effects.

Objects without a sound reconstruction recipe remain concrete.

### State Tracked

For each virtual object, track:

- aliases from locals to `VirtualObjectId`;
- current field value for each modeled field;
- exact owner-type fact;
- whether the object has escaped;
- whether each field value is known, unknown, or path-dependent;
- the materialization recipe needed if the object later escapes.

### Use Classification

Classify each use of a virtual object as one of:

- virtual-preserving:
  - alias copy;
  - exact field read;
  - exact field write;
  - exact owner-type guard that is already implied by the origin;
- materialization-required:
  - return;
  - store into globals, cells, containers, or object fields;
  - unknown call or unknown attribute access;
  - identity-sensitive use;
  - deopt or interpreter-resume boundary that requires a real Python object;
- unsupported:
  - any operation whose user-visible semantics are not yet modeled precisely.

Unsupported cases should simply end virtualization for that object rather than
falling back to shape-specific rewrites.

### CFG Merging

At each join:

- keep an object virtual only if incoming paths agree on the same
  `VirtualObjectId`;
- preserve a field as virtual when every incoming path has a compatible field
  value;
- record edge-carried values when a field remains virtual but needs a block
  parameter at the join;
- materialize or give up when incoming states are incompatible.

This replaces the earlier hidden merge behavior inside
`TypedFieldScalarState` with an explicit analysis result.

## Virtual-To-Locals Lowering

### Field Locals

Allocate one ordinary local for each live virtual field that survives analysis.
Do not allocate fields that are dead after virtualization.

Examples:

```text
IterRange.current -> %vobj0_current
IterRange.stop    -> %vobj0_stop
IterRange.step    -> %vobj0_step
```

### CFG Lowering

- Straight-line field updates become normal local stores.
- Loop-carried field values become loop-header block parameters.
- Join-carried field values become block parameters plus edge arguments.
- If a field is constant across a region, later ordinary passes can clean up the
  now-trivial local traffic.

The lowering pass should make the scalar recurrence explicit enough that the
existing typed-local and exact-int machinery can optimize it without object
knowledge.

### Operation Rewrites

Rewrite:

- `GetAttr(virtual, field)` into a read of the corresponding field local;
- `SetAttr(virtual, field, value)` into an update of the corresponding field
  local;
- exact owner-type guards implied by the virtual origin into nothing;
- alias-only stores of the virtual object into nothing unless the alias is still
  required by a later materialization boundary.

### Materialization

Materialize only where a real Python object is required.

Required boundaries include:

- returning the object;
- passing it to an unknown call;
- storing it into Python-visible state;
- executing an identity-sensitive operation;
- transferring control to a deopt or interpreter-resume path that requires the
  object in frame state.

The preferred representation is explicit. Either:

- insert ordinary typed IR that allocates and populates the object before the
  boundary; or
- if deopt reconstruction cannot be represented as ordinary IR yet, attach a
  validated materialization recipe to the deopt record keyed by instruction id.

In either case, a recipe is usable only when every value needed to rebuild the
object is still definitely bound at that boundary. If inlining cleanup has
already deleted a constructor temp needed by the recipe, keep the original
object concrete instead of attempting late materialization.

The latter is acceptable only as a validated sidecar, not as hidden codegen
behavior.

## Pipeline Ordering

The eventual typed pipeline should look roughly like:

```text
typed direct-call inlining
  -> raise/handler simplification such as StopIteration folding
  -> virtual tuple simplification
  -> profile annotations that still need original object-shaped sites
  -> object virtualization analysis
  -> virtual-to-locals lowering
  -> value-fact refresh
  -> ordinary scalar/value optimization
  -> late metadata attachment and codegen preparation
```

The current broad profile-annotation stage may need to split:

- object-shape-dependent annotations before virtualization;
- truly scalar annotations after lowering.

## Implementation Plan

### Phase 1: Add The Analysis Artifact

1. Introduce `TypedVirtualizationPlan`, `VirtualObjectId`, `VirtualState`, and
   materialization recipe types.
2. Port the current constructor-field recognition into a read-only analysis
   that produces the new sidecar without rewriting IR.
3. Add structured regression tests showing that the new analysis identifies
   the same existing constructor cases as the current path.
4. Keep the old scalarize/virtualize implementation live while the new artifact
   is being validated.

### Phase 2: Replace Local-Root Tracking

1. Move alias and field-state reasoning from `TypedFieldScalarState` onto stable
   `VirtualObjectId`s.
2. Preserve current alias behavior, including the case where the original
   constructor-result local is deleted while another alias remains live.
3. Add explicit per-block and per-edge state output.
4. Keep CFG support conservative at first; unsupported merges should decline
   virtualization rather than infer too much.

### Phase 3: Lower Straight-Line Cases

1. Implement virtual-to-locals lowering for one-object, no-join regions.
2. Rewrite field reads and writes to ordinary locals.
3. Remove now-redundant object guards and alias traffic in those regions.
4. Prove that the simplest current constructor hot paths no longer require the
   old field-scalarization machinery.

### Phase 4: Lower CFG-Carried State

1. Add block-parameter synthesis for loop headers and joins.
2. Carry field locals on edges where values differ by predecessor, splitting
   non-jump predecessors through jump trampolines when the original edge shape
   cannot carry block arguments directly.
3. Preserve ordinary scalar facts across the new block parameters.
4. Re-run the pure-Python range forcing case and verify that iterator fields are
   now normal loop-carried locals rather than hidden object state.

### Phase 5: Add Materialization Boundaries

1. Add explicit materialization for ordinary escapes.
2. Integrate deopt/interpreter-resume reconstruction.
3. Validate ownership, refcount timing, and cleanup behavior at each boundary.
4. Add regression tests for mixed virtual/concrete paths and cold deopt exits.

### Phase 6: Delete The Old Coupled Path

1. Remove `TypedFieldScalarState`.
2. Remove `TypedVirtualConstructorPlan`.
3. Remove the current scalarize-then-virtualize pair from the typed pipeline.
4. Keep only:
   - object virtualization analysis;
   - virtual-to-locals lowering;
   - ordinary scalar/value passes.

## Regression Coverage

Add structured tests for:

- straight-line constructor field reads becoming locals;
- alias survival after the original constructor-result local is rebound or
  deleted;
- loop-carried fields becoming block parameters;
- joins with equal incoming field state remaining virtual;
- joins with incompatible field state forcing materialization or declining
  virtualization;
- exact owner-type guard removal on a virtual object;
- deopt edges reconstructing the right Python object state;
- escaping objects remaining concrete;
- the pure-Python range case lowering to scalar loop-carried locals with no hot
  field traffic remaining.

These tests should assert typed structure and selected plans, not rendered text.

## Risks And Hard Parts

### Materialization Correctness

The optimizer must never reconstruct an object by rerunning arbitrary Python
effects. Virtualization is only valid when the object has a side-effect-free,
validated reconstruction recipe.

### Deopt Semantics

If interpreter resume needs the object in live frame state, lowering must make
that object real before transfer or provide a validated reconstruction recipe to
the deopt machinery. Hiding that work in late codegen would recreate the same
architectural problem in another layer.

### CFG Growth

Representing virtual fields as block parameters can increase local and edge
arity. Keep only live fields, rely on ordinary dead-code cleanup afterward, and
measure the code-size effect before broadening coverage.

### Ownership And Refcount Timing

Field locals may still carry owned Python values. Lowering must preserve
CPython-visible ownership transitions and cleanup timing even when the container
object itself disappears on the hot path.

### Partial Virtualization

Some objects will be virtual on the hot path but require materialization on a
cold escape path. The design must support that mixed case directly instead of
forcing an all-or-nothing decision for the whole function.

## Acceptance Criteria

The migration is complete when:

- object-specific reasoning is confined to the virtualization analysis and
  virtual-to-locals lowering stages;
- ordinary scalar/value passes no longer consume constructor bindings, alias
  roots, or virtual-object state;
- the pure-Python range forcing case becomes a normal scalar loop after
  lowering;
- existing constructor hot paths still optimize correctly;
- materialization at escape and deopt boundaries is explicit and covered by
  structured tests;
- the old coupled scalarize/virtualize path has been removed.

## Current Limitations

- The dedicated fully-virtual path now exists for trusted, non-escaping
  allocations, and typed-v3 can expose the known runtime `range` allocation
  path through static direct-call selection. Other statically-known runtime
  targets still need the same explicit treatment before they can benefit from
  this path.
- Exception-edge materialization is still conservative. If a virtual object
  would need to cross an exception edge, the analysis keeps that path concrete
  rather than attempting late reconstruction there.
- Deopt reconstruction currently uses explicit pre-boundary materialization in
  ordinary typed IR. There is not yet a separate deopt-record reconstruction
  sidecar because the ordinary IR form is sufficient for the supported cases.
