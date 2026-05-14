---
title: "Unfuck Reachable Analysis View"
---

# Unfuck Reachable Analysis View

## Problem

The current optimizer sometimes needs dead control flow to stop polluting an
analysis, but removing those blocks earlier changes the rewrite pipeline itself.
The recent unreachable-block experiment around trusted generator resume planning
showed the tension:

- ignoring dead resume blocks looked semantically attractive;
- mutating the function earlier made the broader `n_queens` regression much
  slower or unstable;
- the experiment had to be reverted.

That suggests the optimizer is missing a useful middle layer: analyses want a
reachable CFG view, while rewrite sequencing still wants the original mutable
function until a later cleanup point.

## Goal

Give analyses a non-mutating reachable-block view so they can ignore dead blocks
without changing rewrite timing.

The rule should become:

```text
analysis may read a reachable view
rewrites mutate the function only at explicit rewrite points
physical CFG pruning stays a separate cleanup operation
```

## Proposed Model

Add a reusable helper for typed functions:

```rust
struct ReachableTypedCfgView {
    entry: BlockLabel,
    reachable_blocks: BTreeSet<BlockLabel>,
}

impl ReachableTypedCfgView {
    fn contains(&self, block: BlockLabel) -> bool;
    fn iter_blocks<'a>(
        &'a self,
        function: &'a TypedFunction,
    ) -> impl Iterator<Item = &'a TypedBlock>;
}
```

The exact API can vary, but the key property is that analyses can ask "which
blocks matter?" without calling a mutating prune helper.

## Plan

1. Identify typed analyses currently sensitive to dead blocks.

   Start with:

   - trusted-owner analysis;
   - trusted generator resume plan collection;
   - any late alias or consumer analysis that merges states across all physical
     predecessors rather than reachable predecessors.

2. Add a reusable reachability helper for typed CFGs.

   It should compute reachability from the real entry block using current CFG
   successor semantics and expose a small read-only API.

3. Thread the reachable view into trusted-owner analysis first.

   This is the analysis that directly motivated the plan. It should:

   - initialize only reachable blocks;
   - ignore unreachable predecessors at joins;
   - avoid manufacturing default state for dead blocks merely because they
     exist in the function body.

4. Teach generator resume plan collection to honor the same view.

   A resume site in a dead block should not contribute:

   - a missing-owner-state count;
   - an escaped-plan count;
   - or a refresh candidate.

5. Keep existing mutating prune helpers where they are still semantically useful.

   The late cleanup point may still want to physically delete dead blocks. This
   plan does not replace that cleanup. It removes the temptation to change prune
   timing just to make analysis results sane.

6. Add tests that distinguish "ignored by analysis" from "physically removed."

   Examples:

   - a dead resume block remains in the typed function before cleanup;
   - trusted-owner analysis does not report it;
   - a later prune still removes it when the cleanup phase runs.

## Challenging Parts

- Some existing diagnostics probably count facts over all physical blocks. Those
  counters will need clear semantics once reachability is introduced.
- Branch-table and resumed-dispatch edges must be represented correctly in the
  reachability traversal; otherwise the analysis view could become accidentally
  unsound.
- The first patch should resist the urge to refactor every traversal in the
  optimizer. Start with the few analyses that caused the regression.

## Validation

- Focused tests around dead trusted-resume sites.
- The broad `n_queens` regression should no longer require mutation-timing
  experiments just to diagnose missing trusted-owner state.
- Logs should distinguish:

  - physical dead blocks present in the function;
  - reachable blocks considered by an analysis.

