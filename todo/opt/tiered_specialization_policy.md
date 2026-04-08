# Tiered Specialization Policy

## Article Spark

V8 uses different thresholds for interpreter, baseline, mid-tier, and top-tier code. Optimization is
delayed until feedback is stable; if feedback changes, tier-up can be reset.

## SOAC Question

Can SOAC make specialization decisions from an explicit per-function tier policy rather than a
binary "profile pass then specialized pass" switch?

## Concrete Experiment

- Track function entry counts, loop/backedge counts where available, specialization-site stability,
  fallback-rate verification, compile latency, and generated-code size.
- Define policy states:
  - collect only
  - generic JIT
  - verified low-risk specializations
  - aggressive specializations / inlining
  - quarantine a site as unstable
- In benchmark reports, print tier decisions and reasons for top hot functions.
- Keep the current three counter modes as file/lifetime modes; layer tier policy inside a run.

## Success Signal

- Medium-hot functions get cheap specialization or generic JIT without waiting for the hottest-loop
  thresholds.
- A site with high verify fallback rate is automatically unspecialized in the next compile.
- Benchmark artifacts explain "not specialized because feedback unstable/megamorphic/cold".

## Risks

- Policy complexity can hide correctness assumptions. Each tier still needs the same sound fallback
  boundary unless explicitly behind an unsound flag.
