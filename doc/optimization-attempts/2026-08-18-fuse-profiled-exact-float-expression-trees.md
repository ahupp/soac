---
title: "Fuse profiled exact-float expression trees"
---

# Fuse profiled exact-float expression trees

- Status: **LANDED / RETAIN; FULL CORRECTNESS GATE PASSED**.
- Pacific date: 2026-08-18 PDT.
- Baseline revision: integrated `main` change `oqprxors`, commit `e2078ab9`,
  including the retained compiler-owned fixed-length sequence-unpack
  optimization.
- Baseline result: eight normally sampled workloads score
  **0.4596841396286201x** their paired stock CPython; the complete
  pyperformance target remains **at least 1.10x**.
- Hypothesis: select only independently profiled, exact-float expression
  trees containing **at least two** supported arithmetic operations; guard
  every source operand, perform separate IEEE `f64` operations without
  intermediate Python allocations, and box only the final source-visible
  result. This is deliberately distinct from the previous rejected
  per-operation specialization, which allocated a new `PyFloatObject` after
  every operation and regressed overall throughput.
- Current outcome: baseline source, same-resource measurements, prior
  rejection evidence, and dedicated zero-loss `nbody` and `float` native
  profiles justify an experiment. The `float` profile is dominated by slot
  attribute access rather than arithmetic; `nbody` presents the stronger
  measured fusion opportunity. An unchanged-production focused regression
  genuinely fails because none of four eligible expression trees records
  exact-float shape **771**, although its Profile-mode semantic guards pass.
  An independent source-keyed JIT structural regression genuinely failed
  because a fully profiled five-operation tree emitted **zero** native `fmul`
  instructions; it now passes with **three Fmul, two Fadd, zero Fma, one
  final float box, and six original generic fallback calls**. The strengthened
  end-to-end Profile→Apply regression also passes **1 test in 0.90 seconds**,
  covering five trees, ordered fallback, subclass/reflected/mixed operands,
  IEEE corner cases, and separate non-FMA rounding. An additional structured
  optimizer family passes **3 tests**, covering source-keyed maximal
  call/power trees, eligibility exclusions, and selected-tree-only atomic
  linearization; the complete typed-IR and optimizer Rust suites pass **49 /
  49** and **205 / 205** respectively. Broad transformed-Python guardrails
  pass **40 / 40 in 122.32 seconds**, the final combined Cargo test-target
  check passes in **7.44 seconds**, and package-scoped formatting checks pass.
  A normally sampled fixed-eight comparison improves the paired stock score
  from **0.45968414x to 0.46849285x**, but has an outlier-sensitive
  **0.90365202x** previous-SOAC mean and **0.99643138x** robust median
  geometric ratio. A separate two-round targeted repeat confirms significant
  `float` and `nbody` improvements with an eligible-workload median geometric
  ratio of **1.05912777x**. The full `just test-all` correctness gate passes
  **1,216 Python cases / 83 batches / 8 workers** plus all Rust crate suites.

## Hypothesis and evidence

Common numeric Python expressions contain nested trees such as
`x * x + y * y + z * z`. Current SOAC evaluates each generic Python numeric
operation separately: dispatch to `PyNumber_Multiply` or `PyNumber_Add`,
allocate its owned exact-float result, then consume/decrement that temporary
in the next operation. A safe fused tree can preserve the same operation
order and rounded values while eliminating intermediate boxing, decref, and
allocator traffic.

Actual installed pyperformance sources expose the general opportunity without
requiring benchmark-name recognition:

- `nbody.advance`: `dx * dx + dy * dy + dz * dz` within the force magnitude;
  its energy helper also combines squared velocity components.
- `float.Point.normalize`: `x * x + y * y + z * z` after copying attributes
  into ordinary locals.
- `chaos.GVector.linear_combination`:
  `self.x * l1 + other.x * l2`, and analogous `y`/`z` expressions. Attribute
  evaluation may be observable; eligibility must not assume these leaves
  are pure without an actual proof or a guarded single-evaluation plan.
- `spectral_norm` includes multiply/add accumulation, although in-place
  operations and user-call operands must remain generic unless their
  eligibility is explicitly represented and validated.

A source-level census over all eight fixed benchmark programs narrows the
initial approved **exact local/constant-leaf** opportunity further: `float`
contains exactly **one eligible five-operation tree**, in `Point.normalize`
at line **27**. `nbody` contains exactly **three eligible trees / 16 total
operations**: `advance` line **85** has **five** operations and
`report_energy` lines **106 / 108** have **five / six** operations. `chaos`
has no initially eligible tree because its arithmetic leaves are observable
attributes; the remaining five benchmarks have no eligible tree under this
rule. Therefore only `float` and `nbody` should initially acquire fused
Apply code, and the other six benchmarks' generated-code shapes must be
checked for unintended collateral changes.

The installed benchmark programs give a deterministic **source-derived**, not
measured-allocation, estimate of possible dynamic impact. `nbody` uses
**20,000 `advance` iterations** and five bodies, hence **ten body pairs** and
**200,000 evaluations** of the five-operation squared-distance tree per
measured benchmark invocation. If every exact-float fast path succeeds,
replacing five boxes with one could avoid up to approximately **800,000
intermediate float boxes**, plus their matching decrefs. `float` normalizes
**100,000 `Point` objects**, so its one five-operation tree could similarly
avoid up to approximately **400,000 intermediate boxes**. These are upper
bounds inferred from source loop counts, not observed candidate allocation
counters or measured throughput.

Actual baseline Apply function sizes are established by intersecting each
`jit-code-summary.jsonl` row's `process_id` with the **ten measured
`opt_mode=apply` worker PIDs** in its adjacent
`pyperformance-worker-timing.jsonl`, then selecting only
`entry_kind=direct_function_body`. Eligible `nbody.advance` is **122,460
bytes / 8,218 machine blocks**, `nbody.report_energy` **61,380 bytes / 4,126
blocks**, and `float.Point.normalize` **9,380 bytes / 662 blocks**.
Ineligible controls are `float.Point.maximize` **7,216 bytes / 478 blocks**,
`chaos.GVector.linear_combination` **13,356 bytes / 907 blocks**, and
`chaos.Spline.__call__` **92,792 bytes / 6,193 blocks**. The first summary
rows belong to Profile-mode processes, not measured Apply workers, and
default-direct-adapter rows are separate small entries; neither is a valid
before/after optimized-function comparison.

The most recent zero-loss baseline native profile is the retained
fixed-unpack `nbody` capture:

`work/logs/nbody-fixed-unpack-candidate_speedscope.json`

It contains **703 cpu-clock samples with zero lost samples**. Its separate
Speedscope export contains **137 sampled stacks / 99,972 weights**; these
counts are not interchangeable. The replay executes **50 loops at
0.07048615576 seconds each**. Important overlapping inclusive shares are:

- Generated `advance`: **76.812% inclusive / 26.181% self**.
- `_Py_Dealloc`: **18.208% inclusive**.
- `PyNumber_Multiply`: **10.96% inclusive**.
- `PyFloat_FromDouble`: **10.810% inclusive**.
- `float_mul`: **7.256% inclusive**.
- `float_dealloc`: **7.11% inclusive**.
- `float_add`: **5.833% inclusive**.
- `PyNumber_InPlaceAdd`: **5.26% inclusive**.
- `float_sub`: **3.98% inclusive**.

These shares overlap through the same call stacks and must not be added. They
identify arithmetic dispatch, temporary boxing, and deallocation as real
remaining measured-loop costs after the giant Python unpack helper has been
removed.

The dedicated fixed-main pyperformance `float` native baseline is recorded in
`work/logs/fused-float-baseline_record.txt`,
`work/logs/fused-float-baseline_callgraph.txt`, and
`work/logs/fused-float-baseline_speedscope.json`. The **11.247 MB** capture
contains **698 cpu-clock samples with zero lost samples**; its independently
generated Speedscope export contains **323 sampled stacks / 99,980 total
weights**. The attached-profiler replay runs **60 measured loops at
0.0598247231 seconds per loop**; this diagnostic timing is not a normally
sampled throughput result. Its overlapping inclusive shares are:

- `Point.normalize`: **23.647%**; `Point.__init__`: **21.918%**;
  `maximize`: **18.918%**.
- `PyObject_GetAttr`: **19.922%**; generic attribute get: **18.059%**.
- `PyObject_SetAttr`: **11.893%**; generic attribute set: **10.174%**.
- `_Py_Dealloc`: **10.744%**; `PyFloat_FromDouble`: **4.872%**.
- `binary_op1`: **9.600%**; `PyNumber_Multiply`: **4.585%**.
- `float_mul`: **3.726%**; `float_add`: **0.859%**.
- Vectorcall: **35.538%**.

These inclusive percentages also overlap and must not be summed. Attribute
access on slot-backed `Point` objects and call dispatch dominate the `float`
workload. Its arithmetic/boxing share is materially smaller than the measured
post-unpack `nbody` shares; `nbody` therefore provides the stronger
source-backed expression-fusion opportunity. Neither native profile proves a
candidate speedup.

The existing profiler currently cannot distinguish exact float operands:

- `crates/soac_opt/src/operator_specialization.rs::ExactTypeTag` contains
  only `Int = 1` and `Str = 2`.
- `crates/soac_jit/src/jit/intrinsics.rs::emit_exact_type_tag_for_value`
  checks only relocatable `PyLong_Type` and `PyUnicode_Type`, emitting zero
  for exact floats.
- Existing v3 evidence extraction recognizes only exact-int and exact-string
  binary shapes.
- `soac_ir_typed::PyExactType::Float` already exists, but the typed result
  inference for binary arithmetic currently first requires two exact-int
  input facts.

Append-only `ExactTypeTag::Float = 3` would produce packed exact-float/
exact-float shape `3 | (3 << 8) = 771`. Evidence must be recorded by the real
Profile-mode runtime type emitter against `PyFloat_Type`; adding a planner
rule alone cannot activate because current real float samples are shape zero.
Existing int/string tag values and profile semantics must remain unchanged.

The unchanged-production regression
`tests/test_fused_exact_float_expression.py::test_profiled_exact_float_expression_trees_preserve_python_semantics`
establishes this missing evidence directly: `just pytest-fast
tests/test_fused_exact_float_expression.py -q` reports **1 failed in 0.52
seconds** after its Profile subprocess has already passed Python semantic
checks. Four expected functions, `under_call`, `under_power`,
`inverse_distance`, and `returned_tree`, produce `covered_functions = []`
because none records exact-float/float shape **771**. Actual `under_call`
instruction **#2** records **15 shape-0** observations, instruction **#3**
records **14 shape-0** observations plus **one mixed shape-256** observation,
and instruction **#6** records **15 shape-0** observations. `under_power`
instructions **#1** and **#2** each record **12 shape-0** observations. The
fixture also covers float subclasses, reflected/mixed arithmetic, NaN,
infinity, negative zero, and observable evaluation order. Source/IR review
separately verifies that eligible five-operation float trees remain nested in
actual `nbody` and `float` BlockPy/typed expressions under power/call
boundaries. The subsequently strengthened candidate passes the complete
five-tree Profile→Apply regression **1 test in 0.90 seconds**.

The frozen integration fixture now additionally includes a precise rounding
discriminator: for `a = 1.0 + 2**-27` and `b = 1.0 - 2**-27`, Python's
separately rounded `a * b + (-1.0 * 1.0)` is **positive 0.0**, whereas an
incorrect contracted fused multiply-add produces **-2^-54**, approximately
**-5.551115123125783e-17**. The fixture asserts both the zero result and its
positive sign, and the candidate passes this check in both Profile and Apply.

The subsequently strengthened fixture also adds a **fifth** profiled tree,
`guarded_unbound`, trained **12 times** with all locals bound. On a later
invocation with the final local unbound, an earlier `RaisingFloat.__mul__`
must append its observable event and raise `ValueError` **before** that later
local can be loaded. This proves an optimized guard must fail immediately in
source order instead of loading/guarding every leaf up front. The original
unchanged-production RED covered four functions; the strengthened five-tree
candidate fixture now passes **1 test in 0.90 seconds** across Profile and
Apply, including the ordered raising-subclass/unbound-local behavior.

An independent unchanged-production structured JIT test,
`specialized_jit_opt_v3_fused_exact_float_expression_emits_single_box`, also
establishes a genuine RED. Given a source-keyed five-operation arithmetic
tree nested beneath `Pow`, with exact-float shape **771** supplied for all
five arithmetic sites, the unchanged baseline assertion failed with
`Fmul left=0 right=3`. The candidate now passes this same test and proves
**three `Fmul`**, **two `Fadd`**, **zero `Fma`**, exactly **one
`PyFloat_FromDouble`**, and **six original generic fallback calls**. This
establishes a genuine structured RED→GREEN for the approved nested tree,
not merely shape recording. The first focused Cargo compilation incurred
**27.10 seconds** of setup; it is not a runtime or benchmark result.

### Why this differs from the previously rejected strategy

The independent earlier strategy remains rejected in
`doc/optimization-attempts/2026-08-18-exact-float-arithmetic-specialization.md`.
It successfully implemented exact-float tag **3 / packed 771**, guarded each
individual `Add`/`Sub`/`Mul`, emitted one machine `f64` instruction, and then
called `PyFloat_FromDouble` **for every operation**. Its focused semantic
coverage passed, but real `chaos` throughput was effectively unchanged,
three-workload previous-SOAC geometric performance was **0.9721427x**, and
native code grew **0.317%**. That implementation was fully reverted.

Restoring the same per-operation path would repeat a measured failure: the
intermediate Python float allocations and refcount traffic remain, while
per-site exact-type guards and fallback branches expand generated code. The
new strategy is admissible only when a single validated, source-keyed tree
contains **at least two** supported arithmetic nodes and emits **one final**
`PyFloat_FromDouble` materialization. An isolated arithmetic site must remain
on its original generic path rather than reviving the rejected optimization.

## Implementation and compatibility

- Append `ExactTypeTag::Float = 3` while preserving the serialized/intended
  meanings of `Int = 1`, `Str = 2`, and packed existing binary shapes.
- Extend the actual Profile-mode operator-shape emitter to compare each
  operand's exact runtime type against relocatable `PyFloat_Type`, record
  exact-float/float packed shape **771**, and preserve existing int/string
  instrumentation.
- Generate fresh source-keyed Profile evidence for the candidate revision.
  Apply may consume validated evidence but must not specialize adaptively
  from observations made within its own optimized process.
- Build one explicit, source-keyed
  `ExactFloatExpressionSpecializationPlan` in
  `FunctionOptimizationPlanV3`, then preserve its mechanically emitted
  `TypedExactFloatExpressionPlan` sidecar on the exact nested expression
  root. Every admitted internal node must have matching exact-float profile
  evidence and a supported operation; validate the maximal complete tree
  before mechanical code generation.
- Explicit new public crate APIs are
  `soac_ir_typed::TypedExactFloatExpressionPlan`, re-exported from its crate
  `lib.rs`, and `soac_ir_typed::plan_v3::ExactFloatExpressionSpecializationPlan`
  plus `ExactFloatExpressionOperationPlan`. These names describe the
  validated function-level decision and resolved typed sidecar; their focused
  structured and Profile→Apply validations now pass.
- Traverse nested source expression containers explicitly. The existing
  generic `RegionBuilder::linearize_instr` rejects a `Call` root entirely,
  so assuming ordinary region extraction discovers a numeric call argument
  would miss real `Point.normalize` and the `under_call` regression. Select
  the eligible arithmetic subtree inside calls and unsupported `Pow` nodes
  without specializing or reordering the enclosing operation.
- The in-progress production implementation now independently collects
  nested, source-recursed fused-expression plans in **both** existing
  single-function planning wrappers and the normal whole-module planner. It
  reuses the established per-function optimization-artifact map without
  widening `FunctionPlanRequest` or `SpecializationProfile`. The append-only
  float tag, relocatable float type, and native `f64` ABI are implemented;
  `cargo check -p soac_ir_typed -p soac_opt --tests` passes warning-free in
  **6.44 seconds**. Typed sidecar annotation and the per-leaf, immediate,
  source-ordered exact-type-guarded `f64` emitter are now implemented,
  including one final `PyFloat_FromDouble` and a complete generic fallback;
  `cargo check -p soac_jit --tests` additionally passes warning-free in
  **8.20 seconds**, and the structured five-operation JIT regression is
  **GREEN**. The enhanced real Profile pass records packed shape **771** for
  all five covered functions. Its first Apply attempt failed safely during
  eager planning because selected root **#2** no longer matched the
  optimized typed tree (`background_function=consume` context). The verified
  cause was
  `soac_opt/src/typed/linearize.rs::TypedExpressionLinearizer::try_map_instr`,
  which lifts **every nested `BinOp`** before the fused sidecar attaches.
  `under_call` originally has root **#2 Add**, children **#3 Mul / #6 Mul**,
  and leaves **#4, #5, #7, #8**; after linearization root **#2** contains
  only its `Add` over synthesized temporary leaves **#12 / #13**. The
  implemented repair attaches the explicit validated sidecar immediately
  after initial typed conversion; a **three-line atomic linearizer change**
  preserves only the selected whole tree while retaining normal linearization
  of every unselected expression. The strengthened end-to-end
  `just pytest-fast tests/test_fused_exact_float_expression.py -q` now passes
  **1 test in 0.90 seconds** across both Profile and Apply; the final broad
  guardrail run reports the same case in **1.03 seconds**.
- Fuse only trees with **at least two** supported `Add`/`Sub`/`Mul` nodes.
  Reject isolated operations, unsupported operators, division/modulo/power,
  comparisons, ambiguous in-place operations, invalid or stale source IDs,
  and expressions whose effects/ownership cannot be proven.
- Evaluate source operands **exactly once** and in their original
  left-to-right order. Do not move a potentially effectful load, descriptor,
  property, indexing operation, call, exception, or finalizer across an
  earlier operation. Start with provably safe local operands unless a
  side-effect-aware staged plan explicitly preserves the original boundary.
- Stage exact-type guards in source evaluation order and abandon the full
  subtree before loading a later local if an earlier leaf fails. A later
  unbound local must not raise before an earlier float-subclass/reflected
  callback that CPython would execute first; the strengthened
  `guarded_unbound` regression requires `RaisingFloat.__mul__` to log and
  raise before the final unbound leaf is touched.
- Recheck exact `PyFloat_Type` guards at every optimized evaluation. Reject
  float subclasses and mixed operands before bypassing any user-defined
  `__add__`, `__radd__`, `__sub__`, `__rsub__`, `__mul__`, or `__rmul__`
  callback; preserve the original untouched fallback and its exceptions.
- Keep intermediate values as separate `f64` operations. Preserve Python
  expression association, per-operation IEEE rounding, NaN, infinities,
  signed zero, and ordinary exact-float overflow behavior. **Do not contract
  multiply/add into FMA**, reassociate operations, apply fast-math, or change
  signed-zero handling.
- Materialize exactly one owned Python float for the final source-visible
  result. Reject escaped, aliased, independently observed, or intermediate
  results whose allocation/reference lifetime must remain visible. Preserve
  final allocation failure, active exceptions, operand ownership, fallback
  cleanup, tracing/monitoring boundaries, and C-extension compatibility.
  Failed exact-leaf guards must execute the original **entire-subtree**
  generic fallback rather than resuming from an unboxed partial result.
- Mutable assumptions are valid only for the guarded operands of the current
  invocation. Never cache mutable object state, descriptors, module globals,
  builtins, subclass methods, or type results across evaluations.
- Focused unchanged-production integration RED is verified: **1 failed in
  0.52 seconds**, with all four required exact-float trees absent from the
  profile despite passing Profile-mode semantic guards.
- The independent structured JIT RED is verified:
  `specialized_jit_opt_v3_fused_exact_float_expression_emits_single_box`
  originally fails because its profiled nested five-operation tree emits
  **zero Fmul instead of three**, and now passes with **three Fmul, two
  Fadd, zero Fma, exactly one `PyFloat_FromDouble`, and six preserved
  generic fallback calls**. Recording packed shape **771** alone would be
  necessary but insufficient; the passing structured test demonstrates the
  actual nested fused machine-operation shape.
- End-to-end Profile→Apply is **GREEN: 1 passed in 0.90 seconds**. The
  strengthened case proves exact-float shape **771** for five trees, nested
  `Pow`/`Call`, ordered whole-tree fallback, subclasses, reflected/mixed
  arithmetic, NaN, infinity, signed zero, positive-zero separate rounding
  without FMA, and raising-subclass evaluation before a later unbound local.
  Three additional structured optimizer tests also pass, proving maximal
  five-operation selections beneath both `Call` and `Pow`, exclusion of
  isolated/mixed/effectful leaves, and atomic selected-tree linearization
  versus five ordinary generic lifts. The complete `soac_ir_typed` suite
  passes **49 / 49**, including malformed-tree validation and reordered
  mechanical-plan rejection; the complete `soac_opt` suite passes **205 /
  205**. The single-box JIT case is independently GREEN. Broad transformed
  Python compatibility passes **40 / 40 in 122.32 seconds**, the final
  combined Cargo test-target check is warning-free in **7.44 seconds**, and
  package-scoped formatting/checking passes.

## Benchmark protocol and coverage

- Fixed exploratory benchmark set:
  `chaos,comprehensions,deltablue,fannkuch,float,nbody,richards,spectral_norm`.
  It is not the complete pyperformance acceptance suite.
- Baseline artifact:
  `work/pyperformance/comparison-20260818-214645-dkdi7r/summary.json`.
  This is the integrated fixed-unpack revision on **8 CPUs / 12 GiB /
  Linux kernel 6.8.0-137**, using **one normally sampled stock/SOAC round**
  and **20 Apply measurements per benchmark**.
- Baseline completed **8 / 8 benchmarks** in **151.29 seconds**. Every
  workload transformed `__main__` and `soac.runtime`; no standard-library
  module was transformed.
- Baseline compiled-function coverage is **34 `chaos` / 21 `comprehensions`
  / 78 `deltablue` / 1 `fannkuch` / 9 `float` / 8 `nbody` / 53 `richards`
  / 9 `spectral_norm`**. Actual generated `Point.normalize`, `advance`, and
  numeric vector methods must be inspected independently; completing a
  benchmark does not prove that a fused float tree executed.
- Use a reduced numeric subset for fast iteration only when it actually
  exercises transformed fused trees. Any final retained comparison must
  report the fixed eight, previous SOAC, paired stock, significance, robust
  medians, unrelated regressions, code-size changes, and true JIT coverage.
- Candidate release smoke:
  `work/pyperformance/comparison-20260818-224650-HZJqTT/summary.json`;
  all eight benchmarks complete, but debug-single timings include the same
  preexisting **~236–319 ms first-call runtime-helper compilation** seen in
  the prior debug smoke and are not headline throughput measurements.
- Normally sampled fixed-eight candidate:
  `work/pyperformance/comparison-20260818-224902-FQ1Uij/summary.json`;
  one round / **20 Apply values per workload**.
- Independently repeated targeted candidate:
  `work/pyperformance/comparison-20260818-225501-tc2h1K/summary.json`;
  two rounds / **40 Apply values each** for `chaos`, `fannkuch`, `float`,
  `nbody`, and `spectral_norm`, compared with the fixed-eight baseline's
  matching 20-value workloads. Its five-workload stock score must not be
  compared directly with an eight-workload score.
- Candidate `nbody` native profile:
  `work/logs/fused-float-nbody-candidate_{record,callgraph,speedscope}.*`;
  matched prior profile `work/logs/nbody-fixed-unpack-candidate_*`. Attached
  profiling timings are diagnostic, not normally sampled throughput.
- Full `just test-all` correctness gate: **PASS**;
  `work/logs/fused-float-test-all.log` records **1,216 Python nodeids / 83
  passing batches / 8 workers**, plus **551 `soac_jit`**, **49
  `soac_ir_typed`**, **371 `soac_lowering`**, **205 `soac_opt`**, and **8
  PyO3** Rust tests. Cargo takes **108.032 seconds**, inner pytest
  **104.131 seconds** / outer pytest **104.146 seconds**, and the combined
  test phase **212.190 seconds**; the slow counter batch takes **103.39
  seconds** and logged runtime preparation **1.966 seconds**.

## Measurements

| Benchmark | Baseline paired stock mean | Baseline SOAC mean | Baseline SOAC median | Baseline stock / SOAC | Compiled functions | Fixed-eight candidate mean / median |
| --- | --- | --- | --- | --- | --- | --- |
| `chaos` | 29.9041604 ms | 65.1022591 ms | 64.9763890 ms | 0.459341x | 34 | 95.8049075 ms / 67.7506635 ms |
| `comprehensions` | 7.9957358 us | 82.7257059 us | 82.3167505 us | 0.096654x | 21 | 81.4481895 us / 81.9890215 us |
| `deltablue` | 1.4343003 ms | 4.3866803 ms | 4.3759648 ms | 0.326967x | 78 | 4.4544285 ms / 4.4528580 ms |
| `fannkuch` | 183.8070806 ms | 246.8134926 ms | 246.2322300 ms | 0.744721x | 1 | 258.1756106 ms / 258.4381105 ms |
| `float` | 31.1533249 ms | 53.7412227 ms | 53.5592030 ms | 0.579691x | 9 | 54.5013413 ms / 52.1108010 ms |
| `nbody` | 48.3650634 ms | 70.2807075 ms | 69.8007170 ms | 0.688170x | 8 | 65.8389979 ms / 64.9692905 ms |
| `richards` | 20.9250762 ms | 39.3422979 ms | 39.1320370 ms | 0.531872x | 53 | 39.9469926 ms / 39.3747665 ms |
| `spectral_norm` | 48.8525484 ms | 56.2033022 ms | 54.9930745 ms | 0.869211x | 9 | 85.1424164 ms / 55.9885530 ms |

The fixed-eight paired stock/SOAC score moves from
**0.4596841396286201x** to **0.46849285308325195x**, still far below the
complete-suite **1.10x** requirement. Its previous-SOAC arithmetic-mean
geometric ratio is only **0.9036520196614041x**; the outlier-resistant
per-workload median geometric ratio is **0.9964313839664793x**. Candidate
`chaos` contains a **254.266 ms** outlier and `spectral_norm` a
**468.558 ms** outlier; `chaos` and `fannkuch` are reported significantly
slower, `nbody` significantly faster, and the other five workloads are not
significantly different in this first comparison. Every generated direct
function body in all six ineligible workloads has unchanged generated code
size and structured summary, but that does not prove the measured unrelated
regressions are impossible or identify their cause.

The independent two-round targeted repeat has a five-workload previous-SOAC
mean geometric ratio of **0.9623003673635678x** and robust median geometric
ratio of **1.006477989173264x**. The only eligible workloads both improve
significantly: `float` mean **53.7412227 → 51.9292064 ms (1.03489397x)**
and median **53.5592030 → 51.1612905 ms (1.04686966x)**; `nbody` mean
**70.2807075 → 67.3662522 ms (1.04326284x)** and median
**69.8007170 → 65.1412055 ms (1.07152940x)**. Their eligible-only median
geometric ratio is **1.0591277663935188x**. The targeted stock score
**0.6993471005669075x** covers only five workloads and is not comparable to
either eight-workload stock score.

| Generated-code / profiling metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| Optimized typed-IR final basic blocks | 3,069 | 3,069 | unchanged |
| Optimized typed-IR function instances | 218 | 218 | unchanged |
| Pre-optimization serialized BlockPy bytes | 14,398,752 | 14,398,752 | unchanged |
| Apply-mode emitted native bytes | 22,882,360 | 22,789,000 | -0.4080% |
| Apply-mode machine blocks | 1,520,050 | 1,512,900 | -0.4704% |
| `nbody` zero-loss perf cpu-clock samples | 703 | 647 | both zero loss |
| `nbody` Speedscope sampled stacks / weights | 137 / 99,972 | 134 / 100,029 | distinct basis |
| `nbody` `PyFloat_FromDouble` inclusive CPU | 10.810% | 8.039% | -2.771 percentage points |
| `nbody` `PyNumber_Add` inclusive CPU | 2.988% | 0.000% | absent from candidate samples |
| `nbody` `binary_op1` inclusive CPU | 21.622% | 18.391% | -3.231 percentage points |
| `nbody` exact `_Py_Dealloc` inclusive CPU | 18.208% | 22.715% | increased; overlapping/noisy |
| `nbody` `float_mul` inclusive CPU | 7.256% | 6.334% | -0.922 percentage points |
| `nbody` `float_add` inclusive CPU | 5.833% | 3.555% | -2.278 percentage points |
| `float` baseline zero-loss cpu-clock samples | 698 | not captured | no matched candidate profile |
| `float` Speedscope sampled stacks / weights | 323 / 99,980 | not captured | no matched candidate profile |
| `float` `PyObject_GetAttr` inclusive CPU | 19.922% | not captured | baseline only |
| `float` `PyObject_SetAttr` inclusive CPU | 11.893% | not captured | baseline only |
| `float` `PyFloat_FromDouble` inclusive CPU | 4.872% | not captured | baseline only |
| `float` `float_mul` / `float_add` inclusive CPU | 3.726% / 0.859% | not captured | baseline only |
| Actual Profile exact-float shape observations | unsupported / zero tag | 771 in five focused functions | focused GREEN |
| Planned fused-tree intermediate Python boxes | existing box per operation | one final box for five-op structured tree | focused GREEN |

The matched `nbody` zero-loss profiler replay runs **50 loops** before and
after; observed per-loop time changes **70.48615576 → 64.81006962 ms
(1.08758x)**. These are attached-profiler diagnostic values, not the normal
benchmark headline. Inclusive samples overlap. The exact `_Py_Dealloc` frame
increases **18.208% → 22.715%**; counting the baseline's separate
`_Py_Dealloc.localalias.lto_priv.0` frame instead gives a deallocation-family
baseline of **20.912%**, so neither percentage should be misrepresented as
proof that absolute deallocation work decreased.

PID-matched Apply direct-body comparisons prove exactly three changed
functions: `Point.normalize` **9,380 → 7,984 bytes / 662 → 552 blocks**,
`advance` **122,460 → 119,856 bytes / 8,218 → 8,031 blocks**, and
`report_energy` **61,380 → 56,044 bytes / 4,126 → 3,708 blocks**. Every
direct body in `chaos` (**34**), `comprehensions` (**21**), `deltablue`
(**78**), `fannkuch` (**1**), `richards` (**53**), and `spectral_norm`
(**9**) retains the same generated size and structured summary. Process IDs
are matched to measured Apply worker timing; Profile rows and default-entry
adapters are excluded.

## Attempt history

### Attempt 1: Define a materially different fused-tree strategy

- Change: propose append-only exact-float Profile tag **3 / packed 771** and
  source-keyed typed plans covering only nested trees with **at least two**
  exact `Add`/`Sub`/`Mul` nodes and **one final result box**.
- Baseline: integrated main `oqprxors/e2078ab9`, same-hardware fixed-eight
  stock score **0.45968414x**, `float` **53.7412 ms**, `nbody`
  **70.2807 ms**, **3,069 typed blocks / 218 functions**,
  **22,882,360 native bytes**, and **1,520,050 machine blocks**.
- Evidence: existing `nbody` native profile records **703 cpu-clock samples
  with zero loss**, identifying `PyFloat_FromDouble` **10.810%**,
  `_Py_Dealloc` **18.208%**, `float_mul` **7.256%**, and `float_add`
  **5.833%** as overlapping remaining costs. A dedicated `float` profile
  independently records **698 cpu-clock samples with zero loss** and
  **323 Speedscope stacks / 99,980 weights**; attribute get/set costs
  **19.922% / 11.893%** exceed float allocation **4.872%** and arithmetic
  `float_mul` **3.726%** / `float_add` **0.859%**. `nbody` therefore
  presents the stronger arithmetic-fusion opportunity.
- Previous failure: per-operation boxing yielded **0.9721427x** prior-SOAC
  geometric throughput and **0.317% native-code growth** despite passing
  exact-float semantics. Do not reintroduce that rejected implementation.
- Genuine baseline RED: **1 failed in 0.52 seconds**; the Profile subprocess
  passes semantic guardrails, but `under_call`, `under_power`,
  `inverse_distance`, and `returned_tree` all lack required exact-float shape
  **771**. Representative actual evidence is `under_call` **#2: 15 x shape
  0**, **#3: 14 x shape 0 plus 1 x mixed shape 256**, **#6: 15 x shape 0**,
  and `under_power` **#1/#2: 12 x shape 0 each**.
- Independent genuine structural RED:
  `specialized_jit_opt_v3_fused_exact_float_expression_emits_single_box`
  initially fails `Fmul left=0 right=3` for an actual five-operation subtree
  beneath `Pow` with all five shape-**771** evidence samples. The same test
  is now **GREEN**, proving **3 Fmul + 2 Fadd + 0 Fma + 1 final float box +
  6 generic fallback calls**; the initial focused Cargo compile cost
  **27.10 seconds**.
- Implementation milestone: the normal module planner and both existing
  single-function wrappers now collect the same nested, source-keyed tree
  decisions through the existing per-function artifact map. The request and
  profile types remain unchanged; typed/optimizer crates pass a warning-free
  `cargo check -p soac_ir_typed -p soac_opt --tests` in **6.44 seconds**.
  The JIT crate also passes `cargo check -p soac_jit --tests` in **8.20
  seconds**, and its typed annotation/immediate per-leaf guarded single-box
  emission now passes the structured regression. The first real Apply
  attempt safely rejected selected root **#2** because
  `TypedExpressionLinearizer::try_map_instr` replaced its children **#3/#6**
  with temporary leaves **#12/#13**. Immediate validated annotation plus a
  three-line selected-tree-only linearizer preservation fix resolves the
  mismatch; strengthened Profile→Apply now passes **1 test in 0.90
  seconds** across five shape-**771** functions and all semantic guards.
- Additional structured optimizer validation passes **3 / 3 tests**:
  source-keyed maximal five-operation selection beneath both `Call` and
  `Pow`; rejection of isolated operations, mixed shapes, and effectful
  leaves; and one atomic lift for a selected tree versus five unchanged
  generic lifts. The full typed-IR suite passes **49 / 49**, including
  malformed-tree and reordered mechanical-plan rejection; the full optimizer
  suite passes **205 / 205**. Broad transformed Python guards pass **40 /
  40 in 122.32 seconds**, including fixed unpack, captured builtins, closure
  cache/metadata, deterministic shutdown counters, native fallback,
  iteration-shadow correctness, and constructor registration. The final
  combined Cargo test-target check passes warning-free in **7.44 seconds**;
  package-scoped formatting and formatting checks both pass.
- Workflow issue: alternating plain Cargo invocations and `pytest-fast` can
  trigger repeated **20–30-second PyO3 rebuilds** despite unchanged sources.
  Preserve compatible extension/build settings or add a project-native cache
  diagnostic to avoid contaminating focused iteration time.
- Workflow issue: the two-round targeted comparison completed every stock,
  Profile, and Apply worker but its final summarizer rejected three extra
  benchmarks present in the eight-workload baseline; the outer `tee` pipeline
  initially masked the failure. Filtering the existing baseline to the exact
  five-workload selector and rerunning only the summarizer recovered every
  result without repeating benchmark work. The recipe should prefilter prior
  suites by selector and propagate pipeline failures with `pipefail`.
- Compatibility: source-keyed validation, exact runtime type guards,
  subclass/reflected fallback, exactly-once ordered operand evaluation,
  explicit ownership, rounded separate IEEE operations, **no FMA**, and one
  final owned Python float; unsupported or observable trees remain generic.
- Result: **LANDED / RETAIN**. Dedicated `float`/`nbody` native baselines and
  genuine unchanged-production integration/structured REDs are complete;
  both the structured JIT case and enhanced Profile→Apply semantic case are
  GREEN, with all **40** expanded Python guardrails and both Rust package
  suites passing. The fixed eight is inconclusive overall, but independent
  paired repeats significantly improve both eligible workloads, shrink their
  generated code, and reduce observed `PyFloat_FromDouble` sample share.
  The full `just test-all` gate passes **1,216 Python nodeids / 83 batches**
  and all Rust suites. **Retain the verified focused performance and
  correctness improvement without claiming a full-suite speedup.**

## Verdict and next action

- Verdict: **LANDED / RETAIN**. The explicit nested-tree
  implementation passes focused structural and end-to-end regressions plus
  **40 broad Python**, **49 typed-IR**, and **205 optimizer** tests. Two
  independent paired rounds significantly improve both eligible workloads:
  `float` median **1.04687x**, `nbody` median **1.07153x**, and their joint
  median geometric ratio **1.05913x**, with **0.408% less generated native
  code**. The complete fixed-eight result remains noisy/inconclusive
  (**0.99643x median geometric**, **0.90365x mean geometric**) and the stock
  score **0.46849x** remains far short of **1.10x**. Full `just test-all`
  passes **1,216 Python nodeids / 83 batches**, **551 JIT**, **49 typed-IR**,
  **371 lowerer**, **205 optimizer**, and **8 PyO3** tests.
- Transferable lesson: exact-type evidence and a fast scalar instruction are
  insufficient when each operation still allocates a Python result. A fused
  plan must eliminate intermediate boxing while preserving the original
  expression's guards, evaluation boundaries, and IEEE results.
- Next action: retain and integrate the fully validated candidate; continue
  pursuing the incomplete **1.10x complete-suite** stock-CPython goal with
  separate source-grounded strategies.
