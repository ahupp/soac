---
title: "Cache synthetic closure code"
---

# Cache synthetic closure code

- Status: landed
- Pacific date: 2026-08-18 PDT
- Baseline: retained general constructor specialization and batched JIT
  artifact writing; benchmark comparison
  `work/pyperformance/comparison-20260818-151012-O9YKkM/summary.json`.
- Outcome: retained. A genuine regression proves three synthetic named code objects are
  recreated where one reusable template code object should suffice. The
  candidate passes the focused semantic regression with one canonical code
  object and all 17 broader guardrails; actual normally sampled `chaos`
  improves **1.12197x by mean / 1.14361x by median**, with a robust
  three-workload geometric improvement of **1.04583x** and identical generated
  code. Independent native profiling reduces inclusive closure creation from
  **19.04% to 8.14%**. The stock-relative result remains only **0.40138x**;
  the full correctness gate passes, but full-suite acceptance remains pending.

## Hypothesis and evidence

Captured comprehensions and other synthetic nested functions are instantiated
repeatedly while executing ordinary Python loops. Each instantiation requires
a distinct Python function and fresh closure state, but an immutable named
code object with the same capture layout and function kind can be safely
shared when its compiler-owned canonical factory has not changed.

The restored detailed native profile,
`work/logs/chaos-artifact-single-write-bbmap_report.txt`, captured **1,234
`cpu-clock` samples** with **zero lost samples**. Its overlapping inclusive
call paths attribute **19.04%** to `soac_jit_make_function_with_closure`,
including **2.59%** to runtime-module import, **2.76%** to Python code-helper
invocation, and **2.11%** to `code.replace`. These percentages are
overlapping inclusive call-tree shares and must not be summed. Eliminating
repeated import/code-factory/replacement work could improve real closure-heavy
workloads without matching benchmark names, source bytes, or expected output.

The dedicated regression
`tests/test_synthetic_closure_code_cache.py::test_synthetic_closure_code_is_reused_without_hiding_runtime_mutations`
is genuinely RED on the previous implementation: three calls to the same
captured `<listcomp>` create **three `code.__new__` audit events** instead of
the expected **one**; the failure takes **0.52 seconds**. All preexisting
semantic checks already pass on the baseline: a prepatched factory runs
three times, a post-warm monkeypatch runs twice, replacing
`sys.modules["soac.runtime"]` invokes the replacement twice, canonical-helper
reentry produces `[[40, 41, 42]]`, captured values remain independent, and
original source nested-function code identity/metadata are preserved.

The cache must reduce only compiler-created synthetic code construction. It
must not reuse function objects, closure cells, default values, execution
state, user-defined factory results, or source-backed original code objects.
The reduced synthetic `code.__new__` event count is an explicitly tested
consequence of avoiding redundant compiler-owned object creation; user-visible
factory monkeypatches and runtime replacement callbacks still run normally.

## Implementation and compatibility

- Store one `OnceLock<PreparedSyntheticCode>` inside each immutable,
  compiler-owned `FunctionInstantiationTemplate`, rather than a global cache.
  The prepared value owns strong references only to the immutable named
  synthetic code and canonical helper function. Record the runtime module as
  a non-owning `usize` pointer identity; compare it against the currently
  live `sys.modules` object without ever dereferencing the saved address.
- Function templates can remain process-owned after module execution. A
  strong `soac.runtime` reference in a retained template would prevent normal
  module clearing, suppress its profile counter flush to `profile.bin`, and
  aggravate the independent missing-runtime-profile suite blocker. Preserve
  normal module lifetime with non-owning identity only.
- Only cache the synthetic ordered-capture path when no matching original
  source code exists. Preserve original named functions, generators, original
  `co_name` / `co_qualname` / `co_freevars`, and existing source code identity.
- Look up the current runtime module from `sys.modules` before falling back
  to normal import. Accept ordinary module subclasses, not only
  exact-`PyModule_Type` objects: transformed `soac.runtime` is itself a
  `PyModule` subclass. Missing or nonmodule entries retain ordinary Python
  import behavior.
- Cache only when the runtime's current `code_with_freevars` is identical to
  the canonical function in the bootstrap module's dictionary. Recheck both
  current runtime-module pointer identity and strongly owned helper identity
  on every lookup;
  prewarm monkeypatches, later monkeypatches, and `sys.modules` runtime
  replacement must immediately take the existing uncached factory path.
- Prepare `co_name` and `co_qualname` once in the cached code object, signal
  that its metadata is already prepared, and avoid a second per-instantiation
  `code.replace`. Allocate keyword-default dictionaries only when keyword
  defaults actually exist; preserve default semantics.
- Every instantiation still creates a distinct Python function, independent
  current capture cells and values, original globals, defaults, metadata,
  exception behavior, reference ownership, vectorcall state, and visible
  tracing/audit behavior for real user operations.
- Code preparation can invoke Python audit hooks or a monkeypatched
  bootstrap-cache dictionary and recursively reenter the same template.
  Compute the candidate code outside the `OnceLock` initialization lock,
  publish it with `set`, and reuse an already-published value after reentry
  only if both the current module address and canonical helper identity
  match. Never hold a non-reentrant initialization lock while running Python.
- Focused regression asserts canonical synthetic creation **3 to 1**,
  prepatch factory calls **3**, postpatch calls **2**, replacement-module
  calls **2**, reentrant nested results `[[40, 41, 42]]`, distinct closure
  captures, and untouched original source code identity. After the genuine
  baseline RED, the unchanged focused regression passes: **one test passed
  in 0.45 seconds**. Warning-free `cargo check -p soac_jit --tests` also
  passes. After correcting runtime-module ownership, the combined synthetic
  closure, original-code-object, function-mutation, and `code_with_freevars`
  regression suite passes **17 tests in 1.30 seconds**. Package-scoped Rust
  formatting and formatting checks pass. The representative benchmark and
  complete `just test-all` correctness gate also pass.

## Benchmark protocol and coverage

- Fixed working candidate subset: `chaos,richards,deltablue`. The complete
  pyperformance suite and **1.10x** stock-CPython acceptance goal remain
  separate and incomplete.
- Baseline comparison:
  `work/pyperformance/comparison-20260818-151012-O9YKkM/summary.json`;
  `work/logs/jit-artifact-single-write-representative.log`.
- Candidate completion smoke:
  `just pyperformance-compare chaos,richards,deltablue 1 '' --debug-single-value`.
  Cold single-value timing only establishes completion, candidate coverage,
  and artifact shape.
- Completed candidate release smoke:
  `work/pyperformance/comparison-20260818-153820-0tO6MX/summary.json`;
  full log `work/logs/synthetic-closure-cache-smoke.log`. The comparison
  completes all three workloads in **30.97 seconds**, including a release
  extension rebuild. Cold single-value apply times near **143 / 396 / 333
  milliseconds** are not representative throughput. Aggregate apply setup is
  **1.84 seconds**, while transformed function counts remain **35 / 79 / 53**.
  One-worker-per-benchmark generated code is unchanged: **2,055 typed
  blocks**, **1,567,940 native bytes**, and **103,818 machine blocks**.
  Independent profile decoding confirms all three candidate workers retain
  exactly two profile frames, including the unchanged **508,936-byte
  `soac.runtime` frame**; the non-owning module-identity fix therefore
  preserves existing runtime profile flushing.
  These single-worker native totals must not be compared directly with the
  ten-worker ordinary-sampling totals in the main metrics table.
- Candidate normal-sampling comparison:
  `just pyperformance-compare chaos,richards,deltablue 1 work/pyperformance/comparison-20260818-151012-O9YKkM`.
  Generate fresh candidate profile evidence and report both paired stock
  results and direct previous-SOAC comparisons.
- Completed normal-sampling candidate:
  `work/pyperformance/comparison-20260818-153938-uayM6V/summary.json`;
  complete output `work/logs/synthetic-closure-cache-representative.log`.
  The one-round comparison takes **68.14 seconds** and collects **20 apply
  values across ten apply workers per benchmark**. Profile setup totals
  **9.62 seconds** and apply setup totals **30.5 seconds**.
- Completed post-change detailed native profile:
  `work/logs/chaos-synthetic-closure-cache_report.txt`; capture metadata and
  timing `work/logs/synthetic-closure-cache-perf.log`. Replaying **12 loops**
  completes in **8.76 seconds** and records **1,032 `cpu-clock` samples**
  with **zero lost samples**. Compare against the **1,234-sample**, zero-loss
  pre-change detailed profile; sample counts differ and inclusive shares are
  not additive.
- The concise finalized-performance summary is tracked separately in
  `doc/PERF_LOG.md` under change ID `otwslxwo`.
- Completed full correctness gate:
  `work/logs/synthetic-closure-cache-test-all.log`. `just test-all` passes
  **1,210 Python node IDs across 74 batches** and the complete Rust workspace,
  including **544 `soac_jit`**, **367 `soac_lowering`**, **202 `soac_opt`**,
  and **eight PyO3-extension** tests. The test phase takes **169.537
  seconds**, total elapsed time is **171.09 seconds**, and the slowest
  counter-dump batch takes **101.02 seconds**.
- The attempted larger fixed set also included `comprehensions`, but a
  standalone baseline smoke independently fails **before any closure-cache
  candidate**. Artifact
  `work/pyperformance/comparison-20260818-152600-dmzNUX`; diagnostic log
  `work/logs/synthetic-closure-comprehensions-baseline-smoke.log`.
  Stock completes at approximately **19.3 microseconds** and profile at
  approximately **188 milliseconds**, but apply fails:

  ```text
  RuntimeError: counter dump does not contain module soac.runtime [direct_target=exception_matches id=2:27]
  ```

  Its failed baseline `profile.bin` contains exactly **one `__main__` frame
  (71,096 bytes)**. Successful `chaos`, `deltablue`, and `richards` profiles
  each contain **two frames**, including a **508,936-byte `soac.runtime`
  frame**. Profile events prove `exception_matches` was JIT-compiled in the
  failing case; its runtime execution cannot be proven from the missing
  counter frame. Transformed-module counters currently serialize only
  when the module's `m_clear` callback runs, so a retained runtime module can
  silently omit its profile frame.

  This is a preexisting cross-module runtime-profile/full-suite coverage
  blocker, not a candidate regression. Exclude `comprehensions` from this
  comparison only until its separately logged baseline failure is repaired;
  do not claim full-suite coverage or quietly redefine the acceptance target.
- Baseline transforms `__main__` and `soac.runtime` for all three working
  workloads; no standard-library or external dependency module is
  transformed. Apply compiles **35 `chaos`**, **79 `deltablue`**, and **53
  `richards`** functions, totaling **167**.
- Closure activity must be shown by actual hot-path function/capture
  construction or audit/profile evidence. Merely completing a benchmark or
  compiling a comprehension does not prove a cached closure was reused.

## Measurements

| Benchmark | Candidate paired stock mean | Previous SOAC mean | Candidate SOAC mean | Previous SOAC median | Candidate SOAC median | Previous / candidate mean | Previous / candidate median |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `chaos` | 31.1389266 ms | 94.5617740 ms | 84.2816742 ms | 95.1565940 ms | 83.2072625 ms | 1.1219731x | 1.1436092x |
| `deltablue` | 1.4868184 ms | 4.5540016 ms | 4.5131975 ms | 4.5420787 ms | 4.5196838 ms | 1.0090411x | 1.0049550x |
| `richards` | 22.9650194 ms | 44.1866514 ms | 43.2257645 ms | 42.7113685 ms | 42.9130165 ms | 1.0222295x | 0.9953010x |

The baseline paired-stock geometric ratio is **0.3693039913x**, and the
candidate paired-stock ratio is **0.4013803170x**. Both remain far below the
full-suite **1.10x** acceptance target. The candidate's direct
previous-SOAC geometric ratio is **1.0498967x** using means and
**1.0458263x** using robust medians. `chaos` improves by **12.20%** on the
mean ratio and **14.36%** on the median ratio; pyperf reports that difference
as statistically significant. The smaller `deltablue` and `richards` mean
differences are not significant; `richards` actually declines by
approximately **0.47%** on robust medians, so do not describe it as a
proven improvement. Prior VM runs had substantial outliers; report current
paired stock separately and avoid attributing stock drift to the cache.

| Generated-code / coverage metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| Optimized typed-IR final basic blocks | 2,055 | 2,055 | identical |
| Optimized typed-IR function instances | 167 | 167 | identical |
| Pre-optimization serialized BlockPy bytes | 6,311,524 | 6,311,524 | identical |
| Apply-mode native emitted bytes | 15,679,400 | 15,679,400 | identical |
| Apply-mode native machine blocks | 1,038,180 | 1,038,180 | identical |
| Canonical captured-listcomp `code.__new__` events | 3 | 1 | -2 / -66.67% |
| Synthetic closure-creation inclusive native hotspot | 19.04% | 8.14% | -10.90 percentage points |

Track profile/apply setup separately from measured steady-state throughput.
The previous three-workload normal workflow takes **71.50 seconds**, with
**9.01 seconds** aggregate profile setup and **33.4 seconds** aggregate
apply setup. The candidate workflow takes **68.14 seconds**, with **9.62
seconds** profile setup and **30.5 seconds** apply setup; its main measured
win is actual warmed `chaos` execution rather than a code-size change.
In fact, `chaos` apply-worker setup rises from approximately **0.919 to
1.065 seconds** even while measured execution improves, confirming the
headline effect cannot be explained by faster setup.

The independent candidate native profile provides direct causal evidence for
the measured `chaos` improvement. Inclusive
`soac_jit_make_function_with_closure` falls from **19.04%** in the
1,234-sample baseline to **8.14%** in the 1,032-sample candidate, with zero
lost samples in both captures. Baseline `PyImport_Import` (**2.59%**) and
`code_replace` (**2.11%**) no longer appear above the candidate report's
display threshold; absence from a truncated report does not prove literally
zero calls or zero samples. Remaining candidate hotspots include generic
attribute access at **13.57% inclusive** and `load_global_slow` at
**10.27% inclusive**. These identify separate future optimization candidates,
not additional validated gains from this strategy.

## Attempt history

### Attempt 1: Cache immutable code per canonical synthetic template

- Change: resolve current runtime module without repeated full import and
  cache a named synthetic code object in its immutable function template when
  the canonical compiler-owned factory is active.
- Measurements and coverage: detailed native perf has **1,234 samples / zero
  lost**, with **19.04%** inclusive closure creation and smaller nested
  import/helper/`code.replace` costs. The baseline focused test creates three
  named code objects where one should suffice. The candidate release smoke
  completes all three benchmarks with unchanged **35 / 79 / 53** functions,
  **2,055** typed blocks, **1,567,940** native bytes, and **103,818** machine
  blocks. Normal sampling verifies **1.12197x** mean and **1.14361x** median
  `chaos` improvement, a **1.04583x** robust mixed-subset geometric mean,
  and exactly unchanged typed/native generated code. Independent zero-loss
  native profiling confirms inclusive closure creation falls from **19.04%
  to 8.14%**.
- Compatibility and tests: the focused baseline fails in **0.52 seconds**
  solely on **3 vs. 1** synthetic audit events; pre/post helper monkeypatch,
  runtime module replacement, canonical-helper reentry, independent captured
  values, and original source code identity already pass. The candidate
  passes the unchanged integration in **0.45 seconds**, reducing canonical
  synthetic creation from **three to one** while retaining every mutation,
  reentry, and identity guard. A warning-free JIT Rust test-target check also
  passes. A broader **17-test** closure/generator run first passed the initial
  implementation, but that result predated a necessary ownership correction:
  strongly retaining the runtime module could suppress module-clear profile
  flushing. The corrected cache retains only code/helper and compares a
  never-dereferenced `usize` module identity; it checks both module and helper
  after reentry. The corrected combined synthetic-closure, original-code,
  function-mutation, and freevar regression suite then passes **17 tests in
  1.30 seconds**; warning-free Rust checks and scoped formatting checks pass.
  The full `just test-all` correctness gate passes **1,210 Python cases / 74
  batches** and all Rust crate suites.
- Result: retained after significant measured `chaos` improvement, unchanged
  generated code, native hotspot reduction, and the complete passing gate.
  `comprehensions` remains independently blocked at baseline by missing
  `soac.runtime` profile evidence and is not counted as successful closure
  optimization coverage.

## Verdict and next action

- Verdict: **LANDED / RETAIN**. The corrected non-owning cache passes **17 focused
  closure/function guardrails**, preserves runtime profile-frame flushing,
  and produces a significant **1.12197x** `chaos` mean improvement with
  **1.04583x** robust mixed-subset geometric improvement and no generated-code
  growth. Native closure overhead falls from **19.04% to 8.14%**. The
  complete correctness gate passes. The stock-relative result is only
  **0.40138x**; complete suite coverage, the independent `comprehensions`
  blocker, and the **1.10x** full-suite performance objective remain
  unresolved.
- Transferable lesson: code reuse is distinct from function/cell reuse, and
  runtime/factory identity plus reentrancy must be checked before caching
  compiler-generated Python objects.
- Next action: retain the validated closure-code optimization and repair
  missing `soac.runtime` profile evidence as a separate full-suite strategy.
