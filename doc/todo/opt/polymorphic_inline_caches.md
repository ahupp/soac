# Polymorphic Inline Caches

## Article Spark

V8 treats monomorphic sites as ideal, polymorphic sites as a short check chain, and megamorphic sites
as not worth locally specializing.

## SOAC Question

Can SOAC specialize common 2-4 way call/field/global/operator sites instead of consuming only one
top value or falling back completely?

## Concrete Experiment

- Extend specialization selection to retain a bounded ordered list of observed shapes per site:
  owner type + version/key-layout index for fields, module key-layout for globals, callee function
  id/type for calls, exact operand type tuple for operators.
- Emit a linear fast-path chain for sites whose cumulative top-N coverage is high and N is below a
  small threshold.
- Treat sites above the threshold as megamorphic: do not emit large chains; keep generic code.
- Add verify counters that break out each arm plus the final fallback.

## Success Signal

- Benchmarks with known two-shape workloads hit both specialized arms and avoid slow-path helper
  calls.
- Pystone does not grow significantly in code size when sites remain monomorphic.
- Profiles identify megamorphic sites as intentionally generic rather than failed specialization.

## Risks

- Python descriptor and class mutation rules make field/method polymorphism more complicated than a
  plain object shape check.
- Code size can grow quickly if the threshold or coverage heuristic is too generous.
