---
title: "Unfuck Candidate Worklist Scheduler"
---

# Unfuck Candidate Worklist Scheduler

## Problem

The typed rewrite loop currently gets work done by repeatedly walking functions,
discovering candidates, rewriting, splitting continuations, revisiting late
cases, and rediscovering more candidates. This is flexible, but it makes the
system hard to reason about because "new work" is often derived from mutated
function shape rather than from an explicit dependency graph.

The recent thread exposed two concrete symptoms:

- clone churn after broad reopen behavior;
- uncertainty over whether a candidate is genuinely new, newly eligible, or
  simply a shape-shifted rediscovery of work already considered.

## Goal

Replace more of the broad rediscovery loop with an explicit candidate worklist
whose entries have stable identities and visible lifecycle transitions.

The optimizer should be able to say:

```text
candidate X was discovered
candidate X was rejected because Y
rewrite Z invalidated candidate X
candidate X was refreshed and accepted
candidate X was already consumed; do not clone it again
```

## Proposed Model

Introduce stable candidate metadata:

```rust
struct RewriteCandidateId {
    function: FunctionId,
    source_instr: InstrId,
    family: RewriteFamily,
    lineage: CandidateLineage,
}

enum RewriteFamily {
    TrustedGeneratorResume,
    BuiltinConsumer,
    RuntimeProtocolConsumer,
    PostInlineHotContinuation,
}

enum CandidateStatus {
    Discovered,
    Deferred(DeferralReason),
    Rejected(RejectionReason),
    Applied,
    Invalidated(InvalidationReason),
}
```

This does not require every rewrite family to migrate at once. The first slice
should target the families involved in the current late-inline/nqueens behavior.

## Plan

1. Inventory the current repeated discovery loops.

   In `typed_pipeline.rs`, map where these families are currently:

   - initially collected;
   - revisited after other rewrites;
   - suppressed to prevent duplicate work;
   - responsible for hot-continuation splitting.

2. Define stable candidate identity for trusted generator resume work first.

   The identity should survive:

   - localized CFG splitting;
   - late normalization;
   - and any temporary block relabeling that does not change the semantic source
     site.

3. Add a small worklist engine around one candidate family.

   The first implementation can be intentionally narrow:

   - seed trusted resume candidates;
   - process them;
   - record rejection or application;
   - explicitly enqueue refresh requests only when the documented
     post-normalization boundary says they may change.

4. Replace ad hoc suppression for that family.

   Once a stable candidate lifecycle exists, remove family-local clone/reopen
   bookkeeping that was only compensating for repeated whole-function scans.

5. Extend the same pattern to expression-context builtin consumers and other
   late typed consumers.

   These families are already strong candidates for explicit scheduling because
   they become eligible due to predictable normalization or linearization
   events.

6. Add clone-churn accounting.

   Record:

   - number of candidate instances discovered;
   - number of lineage-equivalent rediscoveries prevented;
   - number of clones emitted per semantic candidate.

   This gives a direct way to prove the scheduler is reducing churn rather than
   merely renaming the old behavior.

## Challenging Parts

- Stable identity is the hard part. If identity is tied too closely to current
  block shape, the scheduler will reproduce the same fragility under different
  names.
- Some rewrites genuinely produce new semantic opportunities. The worklist must
  allow that without degenerating into hidden global rescans.
- The migration should not force every existing rewrite family into the new
  scheduler at once; that would be a risky all-or-nothing refactor.

## Validation

- A focused regression around the earlier clone-churn shape.
- Counts showing fewer rediscovered-equivalent candidates after the first
  migrated family lands.
- Existing trusted generator resume and `n_queens` regressions remain green.

