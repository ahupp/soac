# Per-Function Feedback Vectors

## Article Spark

V8 stores inline-cache and type feedback in a per-function FeedbackVector, separate from code. This
lets the runtime update feedback cheaply and lets later compiler tiers consume stable observations.

## SOAC Question

Should SOAC maintain a compact in-memory per-function feedback vector in addition to the cross-run
counter dump format?

## Concrete Experiment

- Allocate a feedback vector alongside SOAC function metadata.
- Give hot operation sites dense feedback-slot ids during BB preparation.
- Record the common update as one relaxed increment or pointer/shape replacement; keep heavy
  serialization out of the normal update path.
- At module exit or explicit dump, translate feedback vectors to the existing rkyv counter format.
- At lazy compile time, allow a function to specialize from its already-warmed in-memory vector even
  before a second program run.

## Success Signal

- Counter-update hot path is no slower than current scalar/top-value updates.
- A single long-running process can tier up one function based on in-process feedback.
- Counter dump stays readable by existing specialization tools.

## Risks

- Duplicating "feedback" and "counters" concepts can make optimization inputs hard to reason about.
  Prefer one logical feedback schema with two storage lifetimes.
