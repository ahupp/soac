---
title: "Use native fixed-length sequence unpacking"
---

# Use native fixed-length sequence unpacking

- Status: **LANDED / RETAIN**. A complete same-hardware
  fixed-eight baseline,
  decoded per-worker unpack call counts, zero-loss baseline native profiling,
  existing helper code size, and the proposed CPython-compatible operation
  boundary are established. Unchanged-production behavioral and structured
  lowerer regressions are both genuinely RED; the structured lowerer now
  passes RED→GREEN. Strengthened Profile→Apply behavioral integration also
  passes (**one test in 3.10 seconds**) after preserving trusted runtime-name
  bootstrap, native fast/cold ABI wiring, deoptimization recognition, and
  canonical CPython type-symbol storage flags. Three structured JIT tests,
  the complete **371-test lowerer suite**, and **28 transformed-runtime
  regressions in 17.69 seconds** also pass. Final combined core/lowerer/JIT
  and standalone raw-runtime checks are clean, and all scoped formatting
  checks pass. The normally sampled candidate is **1.75720874x faster than
  prior SOAC** across eight workloads, with **12.5998x `nbody`** and
  **8.6188x `spectral_norm`** gains. Repeated significant `comprehensions`,
  `deltablue`, and `fannkuch` regressions remain unresolved. The full
  correctness gate passes **1,215 Python tests across 82 batches**, with
  **550 JIT**, **371 lowering**, **202 optimizer**, and **eight PyO3** tests.
- Pacific date: 2026-08-18 PDT.
- Baseline revision: `uopnoqlm`, commit `0790d50f`; this is a docs-only
  child of the retained captured-builtins implementation and has identical
  production code to change `zolltvqv`.
- Candidate change ID: `oqprxors`.
- Outcome: SOAC currently implements ordinary fixed-length Python assignment
  unpacking by calling the transformed Python `soac.runtime.unpack` helper.
  One profiled worker records **2,400,262 calls in `nbody`** and **1,362,660
  in `spectral_norm`**, while the helper itself occupies **83,640 native
  bytes / 5,845 machine blocks** and has no useful internal profile counters.
  A compiler-owned fixed-length unpack primitive may replace this Python
  dispatch without bypassing mutable user operations. Baseline stock CPython
  is **17.20x faster on `nbody`** and **9.35x faster on `spectral_norm`**;
  a zero-loss `nbody` profile attributes **83.369% inclusive / 11.747% self
  CPU** to the generated Python unpack helper. The fixed-eight stock/SOAC
  geometric score is only **0.2811670766x**.
  An untouched-production regression fails in **0.36 seconds** because
  ordinary fixed-length language unpacking calls a monkeypatched runtime
  helper **13 times**, where stock CPython language operations require
  **zero**. The strengthened candidate passes this regression in
  **3.10 seconds** in both Profile and Apply. The fixed-eight candidate
  improves previous-SOAC geometric throughput **1.75720874x** and paired
  stock/SOAC from **0.28116708x** to **0.45968414x**, still far below the
  required **1.10x** full-suite target. Three independently repeated,
  statistically significant workload regressions remain unexplained. The
  complete `just test-all` correctness gate passes.

## Hypothesis and evidence

Ordinary Python assignment forms such as `x, y = pair`, tuple/list loop
targets, and nested fixed-arity destructuring are language operations, not
source-level calls to a mutable `soac.runtime.unpack` function. CPython
executes their sequence/iteration protocol directly and only assigns target
names after unpacking has succeeded.

Baseline SOAC lowering in
`crates/soac_lowering/src/passes/ruff_to_blockpy/stmt_lowering/assign_stmt/mod.rs::lower_unpack_target_into`
builds a tuple of Boolean target flags, calls compiler-resolved
`RuntimeName::Unpack`, materializes an intermediate tuple, and then indexes
that tuple once per assignment target. Its additional Ruff-AST
`rewrite_unpack_target` path also emits `__soac__.unpack(value, spec)`.
`soac_py/src/soac/runtime.py::unpack` then performs Python-level `iter`,
`next`, `StopIteration`, list append, flag iteration, length checks, tuple
construction, and starred-target handling. The helper must remain available
for actual user calls and starred unpacking; the retained candidate instead
introduces a distinct compiler-owned language operation.

One baseline profiling worker per workload was decoded from the fixed-eight
comparison. Recorded exact `call_hot_targets` observations targeting
`soac.runtime.unpack` are:

| Benchmark | Calls in one decoded profiling worker | Interpretation |
| --- | --- | --- |
| `nbody` | **2,400,262** | `advance` alone accounts for **2,400,000** calls |
| `spectral_norm` | **1,362,660** | nested spectral matrix/vector loops dominate |
| `chaos` | **10,000** | general mixed-workload destructuring guardrail |
| `deltablue` | **8** | low-frequency compatibility guardrail |
| `comprehensions` | **0** | no measured unpack-helper opportunity |
| `fannkuch` | **0** | no measured unpack-helper opportunity |
| `float` | **0** | no measured unpack-helper opportunity |
| `richards` | **0** | no measured unpack-helper opportunity |

These counts belong to individual profiling workers, not an aggregate across
all normal-sampling processes. The transformed runtime profile frame has
**zero internal nonzero counter rows inside `unpack`** despite millions of
caller observations, so its large Python helper is effectively unspecialized.
Existing generated helper code is **83,640 bytes / 5,845 machine blocks**;
its elimination or reduction is a testable hypothesis, not an achieved
result.

The installed benchmark sources independently establish why this is a general
language-operation hotspot: `nbody.advance` repeatedly destructures deeply
nested body pairs such as
`(([x1, y1, z1], v1, m1), ([x2, y2, z2], v2, m2))` and body triples
`(r, [vx, vy, vz], m)`. Both `spectral_norm.part_A_times_u` and
`part_At_times_u` unpack their `(i, u)` parameter and repeatedly unpack
`(j, u_j)` from `enumerate(u)`. Eligibility depends only on compiler-owned,
fixed-arity assignment shape, never on these benchmark names or source bytes.

The independently captured baseline native profile is
`work/logs/nbody-fixed-unpack-baseline_callgraph.txt` with companion
`work/logs/nbody-fixed-unpack-baseline_speedscope.json`. Perf records
**902 `cpu-clock` samples with zero lost samples**; the separate Speedscope
export contains **322 sampled stacks / 100,044 total weights**. The replay
executes **five loops** at **0.8890191626 seconds per loop**. Capture takes
**31.06 seconds**, including a **20.23-second release rebuild**, and writes
approximately **15 MB** of perf data. Inclusive attribution is:

- Generated benchmark `advance`: **90.133%**.
- Generated `soac.runtime.unpack`: **83.369% inclusive / 11.747% self**.
- Nested generated `exception_matches`: **31.152%**.
- Nested `_validate_exception_type`: **19.069%**.
- Native `py_vectorcall_hook`: **30.825%**.

These are overlapping inclusive call-tree shares, not additive components.
They establish that the transformed `nbody` measured loop spends most of its
time executing the compiler-inserted Python unpack helper, not merely that a
benchmark completes or a helper was compiled. The independent candidate
native profile below verifies elimination of this generated helper and a
substantial reduction in nested exception/vectorcall samples.

Vendored
`vendor/cpython/Python/ceval.c::_PyEval_UnpackIterableStackRef` provides the
generic CPython iteration and cleanup contract: obtain the iterator, request
the exact required number of items, detect one extra item, preserve
underflow/overflow diagnostics, and release both partially obtained values
and the iterator on every failure. Its fixed-arity path uses
`argcntafter == -1`; starred unpacking has a distinct path and must not be
silently merged into the new primitive. A fast exact-tuple/exact-list path
can avoid Python iteration only when exact-type and length guards make the
operation equivalent; subclasses and arbitrary iterables retain normal
callbacks through the generic CPython path.

The falsifiable hypothesis is that a dedicated validated
`RuntimeName::UnpackFixed` operation can materially reduce hot Python-helper
dispatch and generated code for real numerical loops while preserving
ordinary CPython unpack semantics, mutable user calls, errors, side effects,
and reference ownership.

The root agent verified a genuine unchanged-production RED:

```text
just pytest-fast tests/test_fixed_sequence_unpacking.py -q
1 failed in 0.36s
```

During Profile, a wrapper around `soac.runtime.unpack` records **13 helper
calls** while exercising ordinary tuple/list assignment, nested unpacking,
`enumerate`, `zip`, tuple/list subclasses, and a custom iterator. Fixed
Python language unpacking should record **zero** such user-visible helper
calls. Existing tuple/list subclass callbacks already occur in correct
`[tuple, list]` order, and the custom iterator already records
`[iter, next, next, next]`; candidate fast paths must preserve those baseline
behaviors while removing only compiler-inserted helper exposure.

The same genuine behavioral RED initially passed **one test in 2.19 seconds**
and now passes its strengthened form in **3.10 seconds** in
both Profile and Apply. It verifies tuple/list and nested destructuring,
`enumerate`/`zip`, subclass/custom iterator callbacks, exact CPython arity
and noniterable errors, list snapshotting before mutating target assignment,
explicit mutable runtime-helper calls, helper `__code__` replacement,
mutable runtime globals, existing starred unpack, and forced entry. The
strengthened case additionally verifies `with`-target context-manager entry
and exit, `gc.collect()` during every custom iterator advance, immediate
partial-item `__del__` on iterator failure, and publicly observable
`runtime.unpack_fixed` rebinding that compiler-owned assignments ignore.

An independent structured lowerer regression is also genuinely RED:

```text
passes::ruff_to_blockpy::stmt_lowering::assign_stmt::test::fixed_assignment_unpack_uses_compiler_owned_arity_operation
1 failed; 368 filtered
```

The actual lowered call is `RuntimeName::Unpack(value, (TRUE, TRUE))`; the
required compiler-owned result is `RuntimeName::UnpackFixed(value, 2)`, with
an explicit integer arity rather than an allocated Boolean specification
tuple. This directly tests the production lowering decision, not rendered
BlockPy text or a throughput threshold. The structured regression now passes
genuine RED→GREEN (**one passed; 368 filtered**): both direct assignment and
`with`-target lowerers select the appended compiler-owned name plus literal
arity for unstarred targets, while starred targets retain `unpack`; existing
serialized runtime-name discriminants remain unchanged. The new runtime
attribute and native/deopt/bootstrap paths pass strengthened Profile→Apply
integration and a broader existing transformed-runtime regression selection.

Additional structured production-path validation also passes:

- JIT: **three passed; 547 filtered**, covering
  `fixed_unpack_descriptor_accepts_borrowed_object_and_unboxed_arity`,
  `fixed_unpack_primitive_recognition_requires_compiler_owned_runtime_name`,
  and `inlined_fixed_unpack_preserves_writable_cpython_type_import_identity`.
- Lowering: **two passed; 369 filtered**, proving compiler-runtime bootstrap
  does not materialize the intrinsic as a Python module constant and the
  separate `with`-target rewrite selects fixed-arity unpacking.
- Complete lowerer package suite: **371 passed; zero failed**.
- Broader transformed-runtime guardrails: **28 passed in 17.69 seconds**,
  covering strengthened fixed unpacking, existing CPython unpack/class/map
  behavior, synthetic sync/async iteration shadowing, captured builtins,
  closure code/metadata, deterministic counter shutdown, and early class
  constructor registration.
- Final combined
  `cargo check -p soac_core -p soac_lowering -p soac_jit --tests`:
  **warning-free in 5.62 seconds**. Standalone raw-runtime
  `cargo check --manifest-path crates/soac_jit_runtime/Cargo.toml`:
  **warning-free in 0.10 seconds**. Package-scoped workspace formatting/checks
  and excluded raw-runtime manifest formatting/checks all pass.
- The original direct-assignment lowerer regression remains its own genuine
  RED→GREEN, previously passing **one test; 368 filtered** before the added
  lowerer tests existed.

## Implementation and compatibility

- Append the new public crate API `RuntimeName::UnpackFixed` to the existing
  serialized runtime-name enum rather than inserting it among old variants;
  preserve previously assigned runtime-name discriminants and existing
  module/profile identities.
- Identify compiler-owned **fixed-length, unstarred** assignment unpacking
  explicitly during lowering. This is a language operation with known target
  count, not a speculative bypass of the Python-visible mutable
  `soac.runtime.unpack` function.
- Keep direct source calls to `soac.runtime.unpack`, monkeypatched runtime
  functions, user-defined `unpack` names, rebound globals/builtins, and
  starred targets on their current semantics. Do not introduce a
  benchmark-name, source-fingerprint, helper-identity, or exact-output
  special case.
- For exact tuples and exact lists only, check the requested length before
  transferring any items; preserve their item order, owned references,
  mutation safety, and CPython underflow/overflow exceptions. Recheck exact
  type and current length on every invocation; do not cache mutable object
  state, type assumptions, or user-visible helper bindings across calls.
- For list/tuple subclasses, custom sequences, iterators, generators, and
  other iterables, use CPython-compatible generic unpacking. Preserve
  `__iter__`, `__next__`, length/size behavior, reentry, user exceptions,
  finalizer timing, and partial-item cleanup. Subclass overrides must never
  be bypassed merely because a base C layout is compatible.
- The approved production shape is one raw exact-tuple/list fast helper,
  `soac_runtime_unpack_fixed(tstate, iterable, arity)`, plus
  **one registered Rust cold generic helper**, shared by ordinary JIT,
  forced-entry, and deoptimization paths:
  `dp_jit_unpack_fixed_slow(tstate, iterable, arity)`. The direct ABI borrows
  the iterable, unboxes its immutable integer arity, and returns an owned
  tuple or null with the existing Python error. Generic extraction uses an
  independently owned vector of CPython stack references before publishing
  the final result. Do **not** use `PyTuple_New` as temporary scratch storage:
  its returned tuple can be GC-tracked before arbitrary iterator callbacks,
  exposing a partially initialized or otherwise unsafe Python container.
- The raw fast helper is admitted to the existing bounded local-runtime
  inliner, subject to its shared **128-Cranelift-instruction** limit. This
  changes no admission rule for unrelated user functions.
- Initial runtime integration exposed a preexisting external-data declaration
  inconsistency: the inliner imported every symbol as read-only, but
  `PyTuple_Type` and `PyList_Type` already have canonical writable
  declarations. The first attempt failed with
  `reserved JIT declaration snapshot data PyTuple_Type storage flags mismatch`.
  The general fix uses `cpython_type_symbol_from_name` to preserve writable
  CPython type imports without changing tombstones or other data flags.
- `_PyEval_UnpackIterableStackRef` writes its owned stack references in
  reverse order using `*--sp`; reverse the independent stack-reference
  vector before passing it to exported
  `_PyTuple_FromStackRefStealOnSuccess`. `_PyStackRef` embeds tag bits for
  immortals and is not a raw `PyObject *`. The tuple converter steals
  references only on successful allocation; converter failure must close
  all still-owned references exactly once, whereas the CPython unpack helper
  already cleans its partially produced references on its own failure.
- Match CPython user-visible errors for noniterable inputs, insufficient
  values, excess values, custom iterator exceptions, and target assignment
  order. Never expose partially assigned targets when unpacking itself
  fails.
- Preserve starred/extended unpacking through the existing helper or
  independently proven generic fallback. Preserve nested assignment targets,
  `for` destructuring, `with` targets, attribute/subscript target side
  effects, and evaluation order.
- Keep the decision available across Profile, Apply, Verify, deoptimization,
  forced-entry execution, and runtime/bootstrap initialization. A compiler-
  owned fixed operation must not require `soac.runtime` to be fully imported
  before bootstrap or expose inconsistent interpreter/JIT behavior.
- `soac.runtime.bb_trace_enter` itself destructures `(name, value)` before
  the newly defined `unpack_fixed` appears later in its source. Name binding
  now preserves `RuntimeName::UnpackFixed` alongside `RuntimeName::Globals`
  in both `ModuleConstantExtractor` and `RuntimeNameGlobalNameRewriter`, so
  the compiler-owned operation is neither recursively materialized as an
  uninitialized runtime attribute nor rewritten to a mutable global.
  Direct-ABI recognition accepts only explicit runtime-name locations or
  validated runtime-name constants, never arbitrary user globals with the
  same spelling. The entry/deoptimization interpreter also recognizes the
  trusted operation before ordinary callable lookup. The visible Python
  `runtime.unpack_fixed` fallback delegates to mutable `unpack`; compiler
  operations use the native primitive instead.
- Maintain explicit reference ownership, exception cleanup, user mutation,
  and source-independent eligibility. Do not repeat the rejected eager-
  comprehension strategy's broad cleanup-root changes or name heuristics.
- A genuine baseline behavior RED is established before production edits;
  prove direct source mutability, exact tuple/list behavior, custom
  iterator callbacks, subclass fallback, starred fallback, error/finalizer
  paths, and deopt/forced-entry/bootstrap coverage. The new source regression is
  `tests/test_fixed_sequence_unpacking.py::test_fixed_sequence_unpacking_is_a_cpython_language_operation`;
  it fails against unchanged production (**one failed in 0.36 seconds**)
  because fixed unpack operations invoke the runtime wrapper **13 times**.
  The initial end-to-end implementation passed (**one test in 2.19
  seconds**); the strengthened case now passes (**one test in 3.10
  seconds**) in both Profile and Apply, including actual `with`-target
  enter/exit, repeated GC inside iteration, immediate partial-value cleanup,
  and source-observable `runtime.unpack_fixed` rebinding.
  The separate structured
  `fixed_assignment_unpack_uses_compiler_owned_arity_operation` regression
  also genuinely fails (**one failed; 368 filtered**) because lowering still
  chooses `RuntimeName::Unpack` and a Boolean tuple; it subsequently passes
  (**one passed; 368 filtered**) after direct and `with` lowerers select the
  appended `UnpackFixed(value, literal_arity)` and preserve starred fallback.
- `doc/RUNTIME_FUNCTIONS.md` tracks the new exported raw
  `soac_runtime_unpack_fixed`, registered cold `dp_jit_unpack_fixed_slow`,
  and Python-visible `soac.runtime.unpack_fixed` helpers while retaining
  existing mutable `unpack`. `doc/SPECIALIZATION.md` documents the trusted
  language-operation provenance, direct ABI, exact-type guard boundaries,
  bootstrap/deoptimization behavior, and strengthened integration coverage.

## Benchmark protocol and coverage

- Fixed comparison set:
  `chaos,comprehensions,deltablue,fannkuch,float,nbody,richards,spectral_norm`.
  The full pyperformance suite, not this subset, remains the acceptance
  target: at least **1.10x** the same stock CPython geometric mean.
- Authoritative same-resource pre-candidate baseline:
  `work/pyperformance/comparison-20260818-194002-hXpGuV/summary.json`;
  all **8 / 8 workloads completed** in **515.48 seconds** on **8 CPUs /
  12 GiB / Linux kernel 6.8.0-137**. Generate fresh independent Profile
  evidence for the candidate; use the same worker/sampling and module policy.
- Baseline transformed modules are `__main__` and `soac.runtime` for every
  workload; **no standard-library module is transformed**. Compiled
  benchmark-function counts are **35 `chaos` / 21 `comprehensions` /
  79 `deltablue` / 1 `fannkuch` / 9 `float` / 9 `nbody` / 53 `richards` /
  10 `spectral_norm`**. Completion alone is not proof of transformed hot
  coverage; decoded `advance` and spectral call targets establish actual
  compiler-owned unpack activity in their benchmark loops.
- Baseline and candidate normally sampled Apply each use **20 measurements
  per benchmark**. The same-hardware candidate artifact is
  `work/pyperformance/comparison-20260818-214645-dkdi7r/summary.json`; all
  eight workloads complete in **151.29 seconds**, versus **515.48 seconds**
  for the previous revision. The preceding release debug-single smoke,
  `comparison-20260818-214519-YREh05`, completed in **33.76 seconds**
  including a **21.45-second release rebuild**; its cold values are not
  throughput evidence.
- Candidate compiled-function coverage is **34 `chaos` / 21
  `comprehensions` / 78 `deltablue` / 1 `fannkuch` / 9 `float` / 8 `nbody`
  / 53 `richards` / 9 `spectral_norm`**. The old Python `unpack` body is
  absent from exactly the four workloads previously using it: `chaos`,
  `deltablue`, `nbody`, and `spectral_norm`. Project modules remain
  `__main__` and `soac.runtime`; no standard-library module is transformed.
- Baseline and candidate each execute **80 Apply workers**. Aggregate Apply
  setup decreases **69.278 seconds to 55.110 seconds** and aggregate
  measured-worker time **39.059 seconds to 13.733 seconds**; the primary
  claim is measured steady-state numerical-loop improvement, not startup.
- The full `just test-all` correctness gate passes:
  `work/logs/fixed-unpack-test-all.log`, **1,215 Python node IDs in 82
  passing batches across eight workers**; Rust **550 JIT / 371 lowering /
  202 optimizer / eight PyO3** tests all pass. Wall time is **211.57
  seconds**, including **25.388 seconds** of test-runtime preparation,
  **92.902 seconds** of Cargo tests, **93.120 seconds** of Python tests,
  and **186.038 seconds** in the combined test phase. The known slow
  counter-dump batch takes **92.38 seconds**. The complete pyperformance
  suite and its required **1.10x** stock score remain unachieved.

## Measurements

| Benchmark | Previous stock mean | Previous SOAC mean | Previous SOAC median | Candidate stock mean | Candidate SOAC mean | Candidate SOAC median | Previous / candidate | Candidate stock / SOAC | Previous comparison |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `chaos` | 31.7881268 ms | 64.5569748 ms | 63.9865815 ms | 29.9041604 ms | 65.1022591 ms | 64.9763890 ms | 0.991624x | 0.459341x | not significant |
| `comprehensions` | 8.1164533 us | 77.5630500 us | 77.1133052 us | 7.9957358 us | 82.7257059 us | 82.3167505 us | 0.937593x | 0.096654x | significantly slower |
| `deltablue` | 1.6114451 ms | 3.9715783 ms | 3.9610381 ms | 1.4343003 ms | 4.3866803 ms | 4.3759648 ms | 0.905372x | 0.326967x | significantly slower |
| `fannkuch` | 199.1649016 ms | 240.2776810 ms | 238.5309770 ms | 183.8070806 ms | 246.8134926 ms | 246.2322300 ms | 0.973519x | 0.744721x | significantly slower |
| `float` | 34.9068753 ms | 52.2848398 ms | 51.1992745 ms | 31.1533249 ms | 53.7412227 ms | 53.5592030 ms | 0.972900x | 0.579691x | not significant |
| `nbody` | 51.4768049 ms | 885.5229094 ms | 885.2678085 ms | 48.3650634 ms | 70.2807075 ms | 69.8007170 ms | **12.599801x** | 0.688170x | significantly faster |
| `richards` | 22.4372471 ms | 41.3079500 ms | 40.9855660 ms | 20.9250762 ms | 39.3422979 ms | 39.1320370 ms | 1.049963x | 0.531872x | significantly faster |
| `spectral_norm` | 51.7913849 ms | 484.4061489 ms | 476.9289910 ms | 48.8525484 ms | 56.2033022 ms | 54.9930745 ms | **8.618820x** | 0.869211x | significantly faster |

The previous-SOAC/candidate mean geometric improvement is
**1.7572087385766595x**; the independent median-based geometric improvement
is **1.7527693819838497x**. Paired stock/SOAC improves from
**0.281167076589324x** to **0.4596841396286201x**. Every individual
workload remains slower than its same-run stock CPython, and the full-suite
**1.10x** acceptance target is not met.

The initial comparison marks `comprehensions`, `deltablue`, and `fannkuch`
significantly slower; `nbody`, `spectral_norm`, and `richards` significantly
faster; and `chaos`/`float` not significant. A separate targeted,
normally sampled comparison,
`work/pyperformance/comparison-20260818-215155-DzLGcE/summary.json`,
**independently confirms all three significant regressions**:

| Benchmark | Previous SOAC mean | Targeted candidate mean | Previous / targeted candidate | Targeted median comparison |
| --- | --- | --- | --- | --- |
| `comprehensions` | 77.5630500 us | 81.9595060 us | 0.946358x; 1.06x slower | 77.1133052 us to 81.5214424 us |
| `deltablue` | 3.9715783 ms | 4.3721252 ms | 0.908386x; 1.10x slower | 3.9610381 ms to 4.3317974 ms |
| `fannkuch` | 240.2776810 ms | 261.9344800 ms | 0.917320x; 1.09x slower | 238.5309770 ms to 253.7158105 ms |

PID-matched Apply code summaries show all 21 `comprehensions` function bodies
unchanged except a **four-byte** runtime validator difference; `fannkuch`
generated code size and structured attribution are unchanged. Actual
machine-code relocation bytes can differ across processes, including within
the same revision. The only changed `deltablue` benchmark body,
`chain_test`, shrinks **44,956 to 44,184 bytes**, and its obsolete
**83,640-byte** unpack body disappears. Thus the regressions are real and
reproduced but are not explained by generated-code expansion; their cause
remains unproven and must not be dismissed as noise.

| Generated-code / hotspot metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| Optimized typed-IR final basic blocks | 3,389 | 3,069 | **-9.4423%** |
| Optimized typed-IR function instances | 222 | 218 | **-1.8018%** |
| Pre-optimization serialized BlockPy bytes | 14,378,912 | 14,398,752 | +0.1380% |
| Apply-mode emitted native bytes | 26,752,920 | 22,882,360 | **-14.4678%** |
| Apply-mode native machine blocks | 1,782,930 | 1,520,050 | **-14.7443%** |
| `soac.runtime.unpack` native helper body | 83,640 bytes / 5,845 blocks | absent in all four previous users | eliminated |
| `nbody` unpack calls in one baseline Profile worker | 2,400,262 | helper absent in Profile and Apply | old Python dispatch eliminated |
| `spectral_norm` unpack calls in one baseline Profile worker | 1,362,660 | helper absent in Profile and Apply | old Python dispatch eliminated |
| `nbody` generated unpack inclusive CPU | 83.369% | absent from sampled stacks | eliminated |
| `nbody` generated unpack self CPU | 11.747% | absent from sampled stacks | eliminated |
| Nested `exception_matches` inclusive CPU | 31.152% | 4.688% | -26.464 percentage points |
| Nested `_validate_exception_type` inclusive CPU | 19.069% | 1.847% | -17.222 percentage points |
| `py_vectorcall_hook` inclusive CPU | 30.825% | 7.676% | -23.149 percentage points |

The baseline native profile records **902 cpu-clock samples with zero loss**;
the candidate records **703 cpu-clock samples with zero loss**. Their separate
Speedscope exports contain **322 sampled stacks / 100,044 weights** and
**137 sampled stacks / 99,972 weights**, respectively; these are distinct
sample bases. Baseline replay runs **five loops at 0.8890191626 seconds**
each; candidate replay runs **50 loops at 0.0704861558 seconds** each.
Candidate capture completes in **7.89 seconds** without a rebuild.

With the Python unpack helper absent, remaining candidate `nbody` hotspots are
generated `advance` (**76.812% inclusive / 26.181% self**),
`PyFloat_FromDouble` (**10.810%**), object deallocation (**18.208%**),
float multiplication (**7.256%**), and addition (**5.833%**). Inclusive
shares overlap and must not be added.

Actual Apply benchmark functions also shrink: `nbody.advance` decreases
**140,576 to 122,460 bytes**; `report_energy` **78,108 to 61,380 bytes**;
`offset_momentum` **34,956 to 28,600 bytes**; and spectral
`part_A_times_u` / `part_At_times_u` decrease **20,512 to 17,472 bytes** /
**20,528 to 17,488 bytes**. Existing native caller bodies, not only a
benchmark wrapper, therefore execute the new fixed-unpack operation.

## Attempt history

### Attempt 1: Add a compiler-owned fixed-arity unpack operation

- Change: append public `RuntimeName::UnpackFixed`, selected only for
  compiler-generated nonstarred fixed-arity assignment. Exact tuple/list
  paths avoid the Python helper; subclass/custom iterable paths preserve
  CPython's generic iteration and errors.
- Evidence: decoded per-worker runtime-helper targets number **2,400,262
  in `nbody`**, **1,362,660 in `spectral_norm`**, **10,000 in `chaos`**, and
  **8 in `deltablue`**. Existing helper native output is **83,640 bytes /
  5,845 machine blocks** with no nonzero helper-internal profile counters.
  A zero-loss native profile (**902 perf samples; 322 Speedscope stacks**)
  attributes **83.369% inclusive / 11.747% self** CPU to generated
  `soac.runtime.unpack` under **90.133% inclusive** benchmark `advance`.
- Compatibility: mutable source-level runtime helpers, starred unpacking,
  subclass/custom iterator callbacks, mutation, exceptions, exact item
  ownership, partial cleanup, target ordering, deopt, forced entry, and
  runtime bootstrap must all remain correct.
- Tests/measurements: complete fixed-eight same-hardware baseline,
  **0.2811670766x** stock, **515.48 seconds**, **3,389 typed blocks / 222
  functions**, **26,752,920 native bytes**, and **1,782,930 machine
  blocks**. Baseline native perf is complete with zero lost samples, and the
  genuine focused RED fails in **0.36 seconds** with **13** unexpected
  runtime-helper calls. A second genuine structured lowerer RED fails
  (**one failed; 368 filtered**) on `Unpack(value, (TRUE, TRUE))` instead of
  compiler-owned `UnpackFixed(value, 2)`, then passes (**one passed; 368
  filtered**) with direct/with-target conversion and untouched starred
  fallback. Existing subclass/iterator callback ordering passes.
  The approved exact fast helper / one Rust cold helper uses independent
  stack-reference storage; GC-tracked tuple scratch is rejected. Source now
  preserves the compiler-owned runtime name through both name-binding
  bootstrap stages, recognizes it only with trusted runtime provenance,
  and dispatches it directly in the entry/deoptimization interpreter.
  Initial integration exposed read-only redeclaration of the writable
  `PyTuple_Type` import; the inliner now recognizes canonical CPython type
  data without changing unrelated data flags. The behavioral regression then
  initially passes (**one test in 2.19 seconds**) in both Profile and Apply.
  Its stronger version then passes (**one test in 3.10 seconds**), including
  context-manager targets, repeated GC, immediate partial-item destruction,
  and user-visible rebinding of the new Python helper.
  Three structured JIT descriptor/trusted-provenance/writable-type tests
  pass (**three passed; 547 filtered**), and two additional lowerer
  bootstrap/`with`-target tests pass (**two passed; 369 filtered**).
  The complete `soac_lowering` suite passes **371 tests with zero failures**.
  Existing transformed-runtime guardrails also pass **28 tests in 17.69
  seconds**, including prior unpacking, class/map, iteration-shadow,
  captured-builtins, closure, shutdown-flush, and constructor behavior.
  Initial `cargo check -p soac_jit --tests` passed in **10.64 seconds**;
  final combined core/lowerer/JIT test-target checking passes warning-free
  in **5.62 seconds**, standalone raw-runtime checking in **0.10 seconds**,
  and package-scoped plus excluded raw-runtime formatting/checks all pass.
  Normally sampled candidate performance is **1.75720874x faster across
  eight workloads**, led by **12.5998x `nbody`** and **8.6188x
  `spectral_norm`**; native bytes decrease **14.4678%**. A separate repeat
  confirms significant `comprehensions`, `deltablue`, and `fannkuch`
  regressions despite unchanged or smaller generated code. The full
  correctness gate passes **1,215 Python node IDs / 82 batches**, plus
  **550 JIT / 371 lowering / 202 optimizer / eight PyO3** Rust tests.
- Result: **LANDED / RETAIN**. General compiler-owned
  fixed unpacking preserves strengthened CPython behavior and materially
  improves the measured mixed workload; three reproducible performance
  regressions require follow-up. Full-suite acceptance is not achieved.

## Verdict and next action

- Verdict: **LANDED / RETAIN**. The same-hardware
  eight-workload candidate improves prior-SOAC mean geometric throughput
  **1.75720874x**, with **12.5998x `nbody`**, **8.6188x `spectral_norm`**,
  and **14.4678% less native code**. Strengthened CPython compatibility and
  focused existing regressions pass. Independently reproduced
  `comprehensions`, `deltablue`, and `fannkuch` slowdowns remain unresolved;
  the stock score **0.45968414x** is still below the **1.10x** complete-suite
  target. The full **1,215-node Python / 550-test JIT / 371-test lowering**
  correctness gate passes.
- Transferable lesson: source-visible assignment unpacking is a language
  operation, while direct user calls to `soac.runtime.unpack` remain mutable
  Python behavior; an optimization must preserve that boundary and CPython's
  exact iterator/error/ownership semantics.
- Next action: investigate the reproduced three-workload regressions and
  remaining float/allocation hotspots. Continue toward complete-suite
  stock-CPython parity; the fixed-eight result is not the acceptance target.
