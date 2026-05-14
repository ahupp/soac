---
title: "Unfuck Trusted Owner Fact Layers"
---

# Unfuck Trusted Owner Fact Layers

## Problem

`TrustedOwnerState` has become the place where several different semantic ideas
meet:

- owner aliases;
- object origins;
- escape classification;
- runtime-name facts;
- preserved locals;
- generator resume dispatch state;
- branch/case-specific transfer facts;
- function-field and closure binding facts.

That concentration helped early work move quickly, but it now makes joins and
rejections hard to explain. When trusted generator resume planning misses a
case, the question is no longer "what single fact was lost?" It is often:

- did the alias/origin relation disappear?
- did escape classification widen?
- did resumed-dispatch bookkeeping fail to survive a merge?
- did a dead predecessor pollute the same merged state?

The recent `n_queens` work exposed that this is no longer a comfortable level of
coupling.

## Goal

Separate generic trusted-owner reasoning from generator-resume protocol
reasoning, and separate both from escape classification where practical.

The result should make it natural to ask:

```text
what owner/origin facts are true here?
what resume-protocol facts are true here?
what facts were discarded because the value escaped?
```

without spelunking one large state object.

## Proposed Model

Move toward a layered analysis product:

```rust
struct TrustedOwnershipFacts {
    owners: OwnerAliasFacts,
    escapes: EscapeFacts,
}

struct TrustedGeneratorResumeFacts {
    resume_targets: ResumeTargetFacts,
    dispatch_cases: ResumeDispatchFacts,
    preserved_bindings: ResumePreservedBindingFacts,
}

struct TrustedOptimizationFacts {
    ownership: TrustedOwnershipFacts,
    generator_resume: TrustedGeneratorResumeFacts,
}
```

The exact type names are less important than the separation of responsibility:

- generic ownership/origin transfer should not know generator resume dispatch;
- generator resume planning should query a focused sidecar rather than reverse
  engineer resume readiness out of a catch-all state lattice;
- escape classification should become an explicit reason facts are rejected,
  not an incidental consequence of a large merge.

## Plan

1. Inventory `TrustedOwnerState` by concern.

   In `crates/soac_opt/src/typed/trusted_owner.rs`, classify every field and
   query helper as belonging to one of:

   - ownership/origin;
   - escape classification;
   - runtime/callable identity;
   - generator resume protocol;
   - branch-local transfer scaffolding.

   This is the basis for a decomposition that matches real usage rather than a
   speculative clean-room redesign.

2. Introduce focused query helpers before splitting storage.

   Examples:

   ```rust
   trusted_owner_facts_for_origin(...)
   trusted_resume_facts_for_site(...)
   trusted_escape_status_for_origin(...)
   ```

   Callers in `soac_jit` should move to these helpers first. That will reduce
   blast radius when the backing storage changes.

3. Split resume-protocol bookkeeping out of the main owner-state path.

   The existing case-state maps for resume dispatch should move behind a focused
   analysis component or sub-struct. The first version can still be produced by
   the same pass if that keeps migration contained.

4. Make escape classification a first-class rejection reason.

   Today it can be difficult to tell whether a missing trusted plan means "no
   match" or "matched but escaped." The analysis API should return something
   like:

   ```rust
   enum TrustedFactLookup<T> {
       Present(T),
       Missing,
       RejectedBecauseEscaped(EscapeReason),
   }
   ```

   This also dovetails with structured decision reporting.

5. Update generator resume planning to consume the focused resume facts.

   `trusted_generator_resume_plans_from_analysis(...)` should read a specific
   resume-facts view instead of requiring broad knowledge of the full owner
   state layout.

6. Remove obsolete cross-layer helpers after the new API has landed.

   The destination is fewer helpers that combine alias, escape, and resume
   reasoning opportunistically.

## Challenging Parts

- Generator resume facts do depend on ownership facts, so the split should be a
  dependency edge, not an artificial wall.
- Some existing state merges may currently rely on being able to conservatively
  wipe several fact families together. The decomposition should preserve the
  same conservative behavior until tests justify a sharper rule.
- The state object is widely referenced from typed JIT planning, so the first
  migration should prioritize query boundaries over field reshuffling.

## Validation

- Existing focused trusted-owner and trusted-generator tests remain green.
- Add tests that separately exercise:

  - alias/origin survival across joins;
  - escape-driven rejection;
  - resume dispatch readiness after ownership facts are still intact.

- The `n_queens` regression should become easier to classify: the failure should
  identify which fact layer is absent instead of collapsing into generic
  `missing_owner_state`.

