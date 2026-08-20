---
title: "Control-Flow-Aware Native Loop Safepoints"
---

# Control-flow-aware native loop safepoints

- Status: **EXCEPTION-SAFE CONTROL-FLOW CANDIDATE VALIDATED BY THE FULL
  CORRECTNESS GATE AND ONE FIXED-FOUR COMPARISON; NORMAL-ONLY INTERMEDIATE
  REJECTED; FULL PYPerformance SUITE PENDING**.
- Pacific date: **2026-08-20 PDT**.
- Change or revision: `urvkupqv`.
- Outcome: retain mandatory CPython pending-event and thread-progress safety
  while avoiding polls on control-flow edges that do not complete genuine loops.
- Strict-policy boundary: the measurements below were collected before the
  newly documented strict-only optimization policy. No benchmark in these
  comparisons authenticated or sealed a strict module, so they establish
  shared runtime correctness and generated-code shape only; they are not
  strict-versus-stock acceptance results or evidence of strict optimization.

## Hypothesis and evidence

- General-purpose opportunity: long-running transformed Python loops must
  periodically honor CPython's pending-event and thread-switch requests. A
  native busy-wait otherwise monopolizes the GIL, preventing another Python
  thread from making the progress required to terminate the loop. However,
  polling every edge whose target happens to occur earlier in a serialized
  block list treats layout as control-flow semantics and bloats ordinary
  straight-line or acyclic code.
- Existing correctness evidence: the focused native thread-progress regression
  and actual transformed `fastapi` workload genuinely failed before
  default-enabled safepoints and passed after their introduction. CPython's
  low-byte pending-event mask and a cold handler preserve the normal no-event
  fast path; disabling all polling is not a valid optimization.
- Existing fixed-four normal comparison regresses from stock geometric score
  **0.5596865226885351x** to **0.54108549x**, with official previous-SOAC
  ratio **0.9313488358x**. Stock-adjusted analysis is approximately
  **0.97468x** and remains adverse; these are fixed-subset observations, not
  a full-suite claim or proof that all workload deltas are causal.
- Existing measured Apply native bytes increase
  **19,124,400 → 21,082,920 bytes (+10.24%)** and machine blocks increase
  **1,250,920 → 1,369,490 (+9.48%)**. Optimized typed coverage stays exactly
  **2,265 blocks / 183 functions**. The unchanged typed coverage alongside
  increased native shape identifies safepoint emission as generated-code cost,
  not newly transformed Python work.
- The current generated fixed-four code contains **1,488** pending-event
  poll sites, distributed **630 / 349 / 405 / 104** across the benchmark
  functions audited. Some polls correspond only to reverse serialized block
  layout rather than actual control-flow cycles.
- Expected effect: derive the loop-closing edges from the real directed CFG in
  a validated pre-codegen sidecar; reduce false-positive polls, native bytes,
  and machine blocks while preserving at least one poll on every reachable
  reducible, irreducible, exception-mediated, or self-loop cycle. The final
  exception-safe candidate actually reduces **1,488 → 130 polls (-91.26%)**,
  native code **21,082,920 → 19,393,800 bytes (-8.0118%)**, and machine
  blocks **1,369,490 → 1,269,540 (-7.30%)**.
- One normally sampled fixed-four round improves official previous-SOAC
  throughput by **1.0280788866932424x (+2.81%)**; stock drift is approximately
  **0.43%**, leaving an approximately **2.37% stock-adjusted** improvement.
  The candidate's **0.5538868897370715x** stock score remains below both the
  pre-safepoint **0.5596865226885351x** and the full-suite **1.10x** target.
  Native bytes also remain **1.41% above** the unsafe pre-safepoint baseline;
  restoring required thread safety is not free.

## Implementation and compatibility

- Implementation shape: first select a deterministic DFS feedback-edge set
  over actual jump, conditional, and branch-table successors. For functions
  with exception edges, repeatedly search the complete normal-plus-exception
  residual CFG and add one real pollable normal edge from each remaining
  cycle until the residual graph is acyclic. Record this validated sidecar in
  the JIT function plan; codegen mechanically consults its selected edges
  instead of comparing block indexes or rediscovering graph semantics.
- CPython-visible behavior: pending calls, signal delivery, GIL handoff,
  background Python thread progress, exception propagation, and native-loop
  termination must remain intact. Keep the cheap low-byte `eval_breaker` mask
  and cold pending-handler path. Preserve the separately validated borrowed
  local-assignment and cross-edge ownership fixes that restore cyclic GC.
- Mutable assumptions and guard lifetime: CFG edges are immutable for the
  validated compiled function; no mutable Python object assumption, module
  state, speculative owner, or runtime feedback is introduced.
- Guard miss or unsupported shape: preserve existing exception and handler
  paths. A cycle consisting only of unpollable exception edges fails
  explicitly; a tampered sidecar is rejected before codegen. Layout
  permutation, irreducible cycles, overlapping exception-mediated cycles,
  self-loops, conditional arms, branch-table arms, and explicit opt-out
  remain covered.
- Focused regression coverage: genuine structural RED-to-GREEN proves
  reordered acyclic polls **1 → 0**, an irreducible loop **3 → 1**, an
  exception-mediated loop **0 → 1**, and overlapping exception-mediated
  loops **0 → 2**. Pure-exception rejection, sidecar-tamper rejection,
  self-loop cold-handler masking, and explicit opt-out pass. Full JIT
  **586 / 586**, runtime configuration **10 / 10**, and broad transformed
  thread/GC/worker/generator coverage **28 / 28** pass; existing cyclic-GC
  **8 / 8** and ownership/finalizer guardrails remain GREEN.
- Final exception-safe candidate full gate: **1,357 Rust tests / 1,310
  transformed-runtime tests GREEN** across **103 isolated Python batches**.
  Runtime preparation takes **1.552 seconds**, Rust tests **64.103 seconds**,
  transformed Python tests **79.613 seconds**, and the combined test phase
  **143.728 seconds**.

## Benchmark protocol and coverage

- Fixed benchmark selection: `chaos`, `comprehensions`, `deltablue`, and
  `richards`; the complete **97-driver / 124-result** suite remains the final
  acceptance target.
- Comparison command and rounds: `just pyperformance-compare` with the fixed
  subset, independently generated Profile evidence, stock and Apply modes,
  and the immediate index-safepoint baseline. Exactly **one normally sampled
  round** completes; this is useful directional evidence but not a
  three-round confidence claim or full-suite result.
- Baseline revision or artifact: retained pre-safepoint fixed-four stock score
  **0.5596865226885351x**; current compatibility-safe safepoint candidate
  **0.54108549x**, previous-SOAC **0.9313488358x**.
- Candidate revision or artifact:
  `work/pyperformance/comparison-20260820-130325-FJHQ6Y`; immediate baseline
  `work/pyperformance/comparison-20260820-120421-u5b54J`.
- Profile evidence: independently generated candidate Profile and measured
  Apply; all four requested benchmarks complete with **10 distinct measured
  Apply worker PIDs each**. FastAPI debug-single Profile verifies progress and
  code shape, not throughput.
- Module selection: existing pyperformance benchmark-source allowlist and
  existing worker dependency policy; no broader source transformation or
  benchmark-specific substitution is authorized by this strategy.
- Completed/failed benchmarks: fixed-four comparison completes **4 / 4**.
  The original full-suite transformed Profile completed **90 / 97 drivers and
  117 / 124 results** before compatibility repairs. The three nested
  process-timing startup/classifier drivers subsequently pass **3 / 3**;
  FastAPI passes with exception-safe safepoints; actual `gc_collect` Profile
  passes after independent borrowed-reference repairs. A fresh complete
  **97 / 124** transformed run remains **PENDING**.
- Transformed benchmark/dependency modules: fixed-four measured project modules
  are `__main__` and `soac.runtime`; direct-body counts are unchanged at
  **32 / 24 / 76 / 51** for `chaos` / `comprehensions` / `deltablue` /
  `richards`. Complete third-party hot-path attribution remains **PENDING**.
- Transformed standard-library modules: **none** in the fixed-four candidate.
- Compiled functions or hot-path coverage: unchanged **183 optimized
  functions / 2,265 typed blocks**; candidate pre-optimization project
  BlockPy is **8,285,072 bytes**.

## Measurements

| Metric | Pre-safepoint baseline | Index-order safepoints | Exception-safe CFG candidate |
| --- | --- | --- | --- |
| Fixed-four stock / SOAC Apply geometric score | 0.5596865226885351x | 0.54108549x | 0.5538868897370715x |
| Official previous SOAC / candidate SOAC | n/a | 0.9313488358x | 1.0280788866932424x |
| Stock-adjusted previous-SOAC comparison | n/a | approximately 0.97468x | approximately 1.0237x; one round |
| Optimized typed-IR blocks / functions | 2,265 / 183 | 2,265 / 183 | 2,265 / 183 |
| Pre-optimization serialized BlockPy bytes | unavailable | unavailable | 8,285,072 |
| Apply-mode native code bytes | 19,124,400 | 21,082,920 (+10.24%) | 19,393,800 (-8.0118% vs previous; +1.41% vs pre-safepoint) |
| Apply-mode machine blocks | 1,250,920 | 1,369,490 (+9.48%) | 1,269,540 (-7.30% vs previous) |
| Generated pending-event poll sites | not yet audited | 1,488 | 130 (-91.26%) |
| Complete transformed pyperformance coverage | incomplete | 90 / 97 drivers; 117 / 124 results before subsequent compatibility fixes | PENDING |

## Attempt history

### Attempt 1: default-on layout-index safepoints

- Change: enable pending-event checks by default and insert a low-byte masked,
  cold-handler poll whenever a control-flow edge points to an equal or earlier
  serialized block index.
- Measurements and coverage: fixed-four score
  **0.5596865226885351x → 0.54108549x**; previous-SOAC
  **0.9313488358x**, stock-adjusted approximately **0.97468x**; native code
  **+10.24%**, machine blocks **+9.48%**, and **1,488** emitted polls with
  unchanged **2,265 / 183** typed coverage.
- Compatibility and tests: native thread-progress and actual FastAPI failures
  genuinely turn GREEN; disabling the mandatory pending-event behavior is
  rejected. Existing full gate was GREEN before this next optimization.
- Result: **RETAINED FOR CORRECTNESS; PERFORMANCE REGRESSION DISCLOSED**.
- Reason: reverse block-list order is not a proof of a directed CFG cycle and
  emits unnecessary safepoints on acyclic edges.

### Attempt 2: normal-only true-CFG loop-closing edge sidecar

- Change: select deterministic DFS ancestor-closing edges over only normal
  jump/conditional/branch-table CFG edges and validate that plan before
  mechanical codegen.
- Measurements and coverage: genuine structured RED-to-GREEN gives reordered
  acyclic polls **1 → 0** and irreducible-loop polls **3 → 1**. FastAPI
  generated polls fall **116 → 5** and native bytes
  **340,744 → 314,092**; its real Profile run completes.
- Compatibility and tests: independently discovered that an exception edge
  can close a cycle whose normal-only graph is acyclic. Genuine unchanged
  normal-only candidate RED yields **0 expected-at-least-1** polls for a
  mixed exception cycle and **0 expected-2** for overlapping cycles.
- Result: **REJECTED AS CPYTHON-UNSAFE; DO NOT OMIT EXCEPTION EDGES**.
- Reason: ordinary DFS backedges alone do not form a feedback-edge set for the
  executable normal-plus-exception control-flow graph.

### Attempt 3: validated exception-safe residual-CFG feedback edges

- Change: preserve the normal DFS plan, then repeatedly inspect the complete
  normal-plus-exception residual CFG with selected poll edges removed. Add a
  genuine normal edge from each remaining cycle until the residual graph is
  acyclic. Reject cycles containing only unpollable exception edges.
- Measurements and coverage: genuine mixed-cycle RED-to-GREEN **0 → 1** and
  overlapping-cycle **0 → 2** preserve the original acyclic **1 → 0** and
  irreducible **3 → 1** wins. Real FastAPI polls become **116 → 8**;
  native bytes **340,744 → 315,476 (-7.415%)**. The required exception
  safety adds **3 polls / 1,384 bytes** over the rejected normal-only
  intermediate.
- Fixed-four normally sampled one-round Apply result is stock score
  **0.5538868897370715x** and previous-SOAC
  **1.0280788866932424x (+2.81%)**, approximately **+2.37%** after
  **0.43%** stock drift. Per-workload previous-SOAC ratios are `chaos`
  **1.0380606738761173x**, `comprehensions` **1.0281193152962615x**,
  `deltablue` **1.0356616338651687x**, and `richards`
  **1.0106984886491859x**. One round cannot establish statistical
  significance or a full-suite improvement.
- Actual measured Apply JITDUMP poll counts are `chaos` **630 → 42**,
  `comprehensions` **349 → 30**, `deltablue` **405 → 46**, and `richards`
  **104 → 12**, total **1,488 → 130 (-91.26%)**. Representative hot bodies
  improve `write_ppm` **81 → 4**, `make_some_widgets` **128 → 11**,
  `projection_test` **98 → 8**, `Richards.run` **39 → 6**, and `WorkTask.fn`
  **38 → 3**. All **10 measured Apply workers per benchmark** have complete
  matching emitted function coverage.
- Aggregate normal Apply native code drops
  **21,082,920 → 19,393,800 bytes (-8.0118%)** and machine blocks drop
  **1,369,490 → 1,269,540 (-7.30%)**; typed coverage remains
  **2,265 blocks / 183 functions**. Project modules are `__main__` and
  `soac.runtime`; standard-library transformed count is **zero**.
- Compatibility and tests: JIT **586 / 586**, config **10 / 10**, transformed
  thread/GC/classifier/generator regressions **28 / 28**, package check and
  format, actual FastAPI Profile, and actual `gc_collect` Profile all pass.
  Tampered plan and exception-only cycles reject explicitly.
- Result: **FULL CORRECTNESS GATE GREEN; FIXED-FOUR CANDIDATE VALIDATED;
  FULL SUITE PENDING**.
- Reason: deleting selected normal poll edges makes the complete executable
  CFG acyclic; therefore every possible normal-or-exception-mediated cycle
  crosses at least one retained pending-event safepoint.

## Verdict and next action

- Verdict: retain exception-safe graph-derived safepoints as a validated
  candidate: required thread progress and GC compatibility remain intact,
  one fixed-four round improves the prior safe revision, and native shape is
  materially smaller. The complete correctness gate passes **1,357 Rust**
  and **1,310 transformed Python** tests. The complete **97-driver /
  124-result** acceptance comparison remains **PENDING**.
- Transferable lesson: serialized block layout cannot establish loop
  semantics, and normal CFG edges alone omit exception-mediated cycles.
  Preserve both negative intermediate outcomes and derive a validated
  feedback-edge set over the executable graph.
- Next action: unblock remaining complete pyperformance drivers/dependencies,
  collect a three-round full-suite Apply
  comparison with actual hot-path coverage, and evaluate regressions. The
  **1.10x stock** goal remains unmet.
