---
title: "Unfuck Post-Normalization Refresh"
---

# Unfuck Post-Normalization Refresh

## Problem

The typed rewrite pipeline currently learns that some candidates are valid only
after later rewrites have changed the function shape. The clearest example is
trusted generator resume planning around late `StopIteration` normalization:

- initial candidate discovery runs before the function has reached its final
  late-inline shape;
- later control-flow normalization makes additional resume work visible;
- a second discovery pass revisits the function and opportunistically finds the
  newly eligible work.

That behavior is semantically reasonable, but the current structure makes it
hard to distinguish:

- a candidate that never existed;
- a candidate that existed but was rejected;
- a candidate that only became valid after normalization;
- a candidate whose eligibility changed because unrelated CFG cleanup happened
  nearby.

This is the main source of "why did this reopen?" confusion in the recent
`n_queens` work.

## Goal

Replace implicit whole-function rediscovery with an explicit post-normalization
refresh contract.

The pipeline should say, in code and in diagnostics:

```text
linearize expressions
derive initial semantic facts
plan initial candidates
apply selected rewrites
normalize late control protocol
refresh only candidate families invalidated or unlocked by normalization
lower the selected state
```

## Proposed Model

Add a small typed phase boundary around late normalization:

```rust
struct PostNormalizationRefreshInput {
    function: FunctionId,
    changed_blocks: BTreeSet<BlockLabel>,
    invalidated_families: BTreeSet<RefreshFamily>,
}

enum RefreshFamily {
    TrustedGeneratorResume,
    RuntimeProtocolConsumer,
    BuiltinConsumer,
}
```

The refresh phase should be narrow by default:

- it may recompute the minimum semantic facts needed by the listed families;
- it may revisit only blocks touched by the normalization or explicitly marked
  as dependent on them;
- it should not silently rerun every late typed rewrite family.

## Plan

1. Document the current typed rewrite phase order in
   `crates/soac_jit/src/jit/typed_pipeline.rs`.

   The first pass should answer:

   - where expression linearization happens;
   - where initial trusted inline work is prepared;
   - where late `StopIteration` normalization occurs;
   - which rewrite families are rediscovered afterward.

2. Introduce an explicit late-normalization result object.

   The current normalization helper should return a summary such as:

   ```rust
   struct LateNormalizationResult {
       changed: bool,
       touched_blocks: BTreeSet<BlockLabel>,
       refresh_families: BTreeSet<RefreshFamily>,
   }
   ```

   Even if the first version only records `TrustedGeneratorResume`, the data
   flow should be explicit from the start.

3. Split "late refresh" from "ordinary candidate discovery".

   Keep the existing initial planning path intact, but move the current
   post-normalization generator-resume revisit behind a dedicated helper such
   as:

   ```rust
   refresh_typed_candidates_after_late_normalization(...)
   ```

   This helper should consume `LateNormalizationResult` and make the refresh
   contract visible at the callsite.

4. Restrict the first refresh implementation to trusted generator resume work.

   This keeps the migration small and anchored to the live `n_queens` problem.
   Other late families should only enter the refresh enum when there is evidence
   that normalization genuinely unlocks them.

5. Add structured diagnostics for the refresh boundary.

   Emit counts for:

   - normalization-changed functions;
   - refresh families requested;
   - candidates found only during refresh;
   - candidates that were present earlier but changed disposition after refresh.

   These should eventually feed the structured optimization decision report
   described in `unfuck_optimizer_decision_reports.md`.

6. Add targeted tests around the boundary.

   Prefer small structural tests that prove:

   - a trusted generator resume candidate can appear only after normalization;
   - the refresh phase picks it up;
   - unrelated candidate families are not rescanned.

   Keep the broad `n_queens` regression as the end-to-end guard, not the first
   iteration loop.

## Challenging Parts

- The current rewrite loop is not yet organized around explicit phase outputs,
  so the first patch may need to thread a result object through a fairly large
  function before any semantic cleanup lands.
- Some late rewrite families may already be relying on accidental rediscovery.
  The migration should expose those dependencies with diagnostics rather than
  preserving them invisibly.
- The refresh boundary must not turn into a second unbounded planner loop under a
  cleaner name.

## Validation

- Focused unit or integration coverage for post-normalization trusted resume
  eligibility.
- The existing `n_queens` cross-module regression remains green.
- Trace output should make it obvious whether a resume candidate came from the
  initial planning phase or the post-normalization refresh phase.

