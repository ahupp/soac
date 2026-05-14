---
title: "Unfuck Optimizer Decision Reports"
---

# Unfuck Optimizer Decision Reports

## Problem

Too much optimizer debugging currently depends on piecing together ad hoc trace
lines. In the recent generator-resume investigation we had to correlate:

- selected vs escaped plans;
- candidate counts;
- missing owner-state counts;
- alias-filter outcomes;
- late unreachable-block cleanup;
- and whether a plan appeared only after a second discovery pass.

Those are real signals, but they are not yet one coherent artifact. The result
is slow iteration and brittle reasoning.

## Goal

Produce a structured per-function optimization decision report that answers, for
each candidate:

```text
what was considered?
what facts were available?
what was selected or rejected?
why?
at which phase?
```

This should serve both:

- human debugging;
- focused regression tests that assert semantic optimizer behavior without
  matching rendered text.

## Proposed Model

Add a typed report structure owned by the planning layer:

```rust
struct OptimizationDecisionReport {
    function: FunctionId,
    candidates: Vec<CandidateDecision>,
}

struct CandidateDecision {
    id: RewriteCandidateId,
    phase: DecisionPhase,
    outcome: DecisionOutcome,
    reachable: bool,
    owner_origin: Option<TrustedOriginId>,
    owner_state: OwnerStateDisposition,
}

enum DecisionOutcome {
    Selected,
    Deferred(DeferralReason),
    Rejected(RejectionReason),
}
```

The exact fields should grow only when they answer recurring questions. The first
version does not need to describe every optimizer family in the codebase.

## Plan

1. Define rejection and deferral enums for the live trusted-resume path.

   Initial reasons should cover at least:

   - unreachable block;
   - no candidate shape;
   - candidate escaped;
   - missing owner/origin state;
   - rejected by alias lowering;
   - selected during initial planning;
   - selected only after post-normalization refresh.

2. Emit the report from trusted generator resume planning first.

   This keeps the first slice grounded in the investigation that exposed the
   pain. The report should be available in memory even when no logging sink is
   enabled.

3. Provide a compact test helper API.

   Example shape:

   ```rust
   assert_candidate_rejected_for(
       report,
       RewriteFamily::TrustedGeneratorResume,
       RejectionReason::MissingOwnerState,
   );
   ```

   The goal is to make specialization regressions easy to write without
   asserting on trace strings or rendered IR.

4. Add an optional logging/export bridge.

   The structured report may later render to:

   - tracing JSON;
   - benchmark artifacts;
   - inspector views.

   But the core abstraction should stay typed and in-process first.

5. Expand coverage to other late and fragile families.

   Good next candidates:

   - post-normalization refresh outcomes;
   - expression-context builtin consumer selection;
   - clone-scheduler decisions once the worklist exists.

6. Retire overlapping one-off counters when the report subsumes them.

   The current plan-collection and alias-filter summaries are useful, but once
   the report can aggregate the same information, redundant log-only summaries
   should shrink rather than accumulate forever.

## Challenging Parts

- The report must not become a second copy of the planner's internal state.
  Capture decisions and reasons, not every intermediate map.
- Some facts will only be meaningful when several earlier refactors land
  together, especially the separate reachable analysis view and the layered
  trusted-owner facts.
- Diagnostics should not accidentally become the source of truth. They report
  decisions; the planner still owns the actual behavior.

## Validation

- Focused trusted-resume tests assert decision outcomes through the typed report.
- The report cleanly distinguishes:

  - not found;
  - found but escaped;
  - found but missing owner state;
  - found only after post-normalization refresh.

- Existing broad traces become easier to interpret because one structured report
  explains the high-level outcome.

