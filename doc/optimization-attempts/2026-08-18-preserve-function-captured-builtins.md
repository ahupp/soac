---
title: "Preserve per-function captured builtins in global lookups"
---

# Preserve per-function captured builtins in global lookups

- Status: LANDED / RETAIN for CPython correctness and measured SOAC
  performance; genuine captured-builtins RED→GREEN, complete Profile→Apply
  semantics, structured inliner/deopt tests, same-resource normal sampling,
  stable finalized pystone, zero-loss native profiling, and the full
  correctness gate all pass. The exact-revision pystone artifact is refreshed
  after documentation freeze; the full-suite pyperformance goal remains
  unmet.
- Pacific date: 2026-08-18 PDT.
- Baseline: retained main change `yskqkuov`, commit `130172c2`, after
  CPython-correct synthetic-function metadata creation.
- Environment: the Ubuntu 24 VM was resized and restarted to **8 CPUs /
  12 GiB RAM**; reboot also changed the kernel from **6.8.0-124 to
  6.8.0-137**. Previous timings have different VM resources and kernel;
  compare candidates only with fresh post-resize baseline
  `comparison-20260818-175425-IXk8jU`.
- Outcome: source and zero-loss native profiling identify both a potential
  CPython semantic mismatch in captured `__builtins__` and repeated global
  fallback dictionary lookups. The verified same-resource paired-stock
  baseline is **0.2742319248x**. The focused unchanged-production
  regression fails (**1 failed in 0.36 seconds**): Profile returns
  `captured=[21, 22]` instead of CPython's `[11, 22]`. The implemented
  explicit-ABI candidate now passes warning-free
  `cargo check -p soac_jit --tests`, but its initial exact-dictionary fast
  branch exposes a genuine preexisting **SIGSEGV** after deleting a shadowed
  SOAC indexed global. Native GDB proves that external-data relocation
  compares real tombstone pointer `0xfffff7f8f2c4` against wrong JIT-local
  stub `0xffffe4a005e8`. Caller data-symbol remapping and a second
  specialized NULL→`NameError` error-propagation fix now make the entire
  Profile→Apply matrix GREEN (**1 passed in 0.79 seconds**). A structured
  regression confirms the fast helper stays inlined and its tombstone
  reference uses the caller's true external-data namespace/ID; a second
  structured regression proves deoptimization uses the captured mapping
  after globals rebinding. The complete indexed-global/deopt family passes
  **20 / 20**, runtime-inliner family **3 / 3**, and cross-strategy Python
  guardrails **9 / 9**. Warning-free final checks and scoped workspace /
  standalone formatting checks pass. Same-resource normal sampling improves
  robust four-workload median throughput **1.0527772907x**; `deltablue`
  (**1.03x**) and `richards` (**1.08x**) are statistically significant,
  while `chaos` and `comprehensions` are not. A **355.788 ms** `chaos`
  outlier reduces the mean-based geometric improvement to **1.0059561519x**.
  Native code grows **1.081%** and machine blocks **1.752%**, with unchanged
  coverage and typed IR. The paired-stock geometric ratio is only
  **0.2735422874x**, far below the full-suite **1.10x** target. Finalized
  Stable cached pystone completes at production median **1,172,977
  loops/second**, without a matched before/after baseline.
  A zero-loss
  candidate `comprehensions` native profile attributes **0.86% inclusive**
  CPU to global slow loads; historical **4.40%** was captured on different
  hardware and is directional evidence only. Closure creation remains the
  dominant cost at **28.30% inclusive**. The full gate passes **1,214 Python
  nodeids / 81 batches** plus **547 JIT / 368 lowering / 202 optimizer /
  8 PyO3 Rust tests**. The finalized performance-log entry is change
  `zolltvqv`; pystone artifact `work/bench/zolltvqvyovx` is refreshed after
  documentation freeze for the exact final revision.

## Hypothesis and evidence

CPython captures a function's effective builtins mapping once at function
creation in `PyFunctionObject.func_builtins`. Vendored
`vendor/cpython/Objects/funcobject.c::PyFunction_NewWithQualName` calls
`_PyDict_LoadBuiltinsFromGlobals(globals)` once, stores that owned result in
`op->func_builtins`, and releases it when the function is cleared. Thus:

- Rebinding `globals["__builtins__"]` after a function exists does **not**
  change that function's captured builtins mapping.
- A function created after the rebinding captures the new mapping.
- Mutating an existing captured mapping remains visible to the function.
- A non-exact dictionary/custom mapping must retain Python mapping lookup,
  callback, `KeyError`, and other exception behavior.
- Globals always take precedence over that function's captured builtins;
  missing names produce CPython-compatible `NameError`.

SOAC's current global-miss path instead re-reads `__builtins__` from the
module globals during each fallback. That can incorrectly cause old functions
to observe a later builtins rebinding and repeats dictionary/global work on
hot builtin-heavy code.

The focused regression
`just pytest-fast tests/test_captured_function_builtins.py -q` genuinely fails
against unchanged production: **1 failed in 0.36 seconds**. During Profile,
the old/new function results are `captured=[21, 22]` rather than CPython's
correct `[11, 22]`. Both existing Python `function.__builtins__` objects
already have the correct independent mapping-A/mapping-B identity; SOAC
alone incorrectly resolves the older transformed function through the later
rebound module globals mapping. This isolates a real user-visible execution
bug, not merely a suspected optimization opportunity.

Existing zero-loss native evidence identifies a material general-purpose
target:

- `chaos`: global slow-path work **10.27% inclusive**, including repeated
  globals/builtins lookup around **5.52%**.
- `comprehensions`: global slow-path work **4.40% inclusive**, including
  repeated lookup around **2.73%**.
- Shares overlap and are not additive. Provenance:
  `work/logs/chaos-synthetic-closure-cache_speedscope.json` and
  `work/logs/comprehensions-shutdown-flush-steady_speedscope.json`.

Vendored `vendor/cpython/Python/ceval.c::_PyEval_LoadGlobalStackRef` shows the
correct split: exact globals plus exact builtins use CPython's fast
dictionary-global lookup; any non-exact mapping uses
`PyMapping_GetOptionalItem(globals, name)` followed by
`PyMapping_GetOptionalItem(builtins, name)`. Vendored source also exposes
`_PyDict_LoadGlobal(globals, builtins, name)`, which hashes once and returns
an owned reference for ordinary CPython dictionaries. It was considered
then removed during diagnosis, but is **not** the cause of the actual
indexed-global crash: the inlined tombstone guard fails before either
candidate dictionary helper is reached.

However, a crucial runtime-only compatibility boundary invalidates the
initial exact-dict optimization. SOAC uses custom indexed globals with a
private `_PyDict_IndexedValueTombstone` after deletion. The first candidate
uses an exact-dictionary fast path and its focused Profile process terminates
with **SIGSEGV (-11)** after inserting then deleting a shadowing global.
Replacing `_PyDict_LoadGlobal` with `PyDict_GetItemRef` does **not** stop the
crash because neither slow helper is reached. Native GDB/JIT machine-code
disassembly proves the actual preexisting defect in external **DATA**
relocation: real `_PyDict_IndexedValueTombstone` address `0xfffff7f8f2c4`
differs from JIT-local trampoline/stub address `0xffffe4a005e8` embedded in
the inlined runtime-support guard. Its incorrect comparison accepts the
actual tombstone as a live object. `_PyDict_GetIndexedItem` correctly
returns `rc=0` and is not the cause.

The active workspace dependency is
`work/cargo-home/.../cranelift-codegen-0.130.1/src/inline.rs`, **not** the
host user's separate cached `cranelift-codegen-0.130.2`. Its
`create_global_values` clones `GlobalValueData::Symbol::User` without
translating callee user-name references into the caller's symbolic namespace,
although function references are translated. The principled SOAC fix is to
predeclare the callee's external data symbol in the caller and remap that
symbolic global-value reference before inlining, preserving the existing
fast inlined tombstone check. Its structured regression now passes and
proves both that the helper remains inlined and that the final symbolic
tombstone reference belongs to the caller's exact external DATA namespace
**1** / data ID. That remap now fixes the complete Profile
execution matrix: old/new captured mappings, live mapping mutation,
globals precedence/deletion, observable dict subclasses, custom mappings,
builtin-module normalization/mutation, `NameError`, and forced entry
interpreter all pass. Apply also passes these checks until the fixture
intentionally deletes `sentinel_builtin` from the captured mapping and
invokes the old function. Its specialized indexed-global fallback correctly
returns NULL with `NameError` set, but generated code fails to branch to
the error block and instead treats NULL as a callable. This is a second
independent preexisting `crates/soac_jit/src/jit/mod.rs` error-propagation
defect exposed by the stronger test. Adding the missing explicit
NULL→existing-error-block edge fixes it. The unchanged standalone
`tests/test_captured_function_builtins.py` now passes across both Profile
and Apply: **1 passed in 0.79 seconds**, covering old/new A/B mappings,
in-place mutation, global shadow/deletion, dict-subclass/custom mapping
callbacks, builtin-module normalization/mutation, `NameError`, and forced
entry interpreter.

The checked-out vendored CPython source lacks the custom indexed-dictionary
patches in the running interpreter, so source-level
`PyDict_CheckExact` reasoning did not establish actual layout compatibility.
An exact dict is not necessarily a supported ordinary dict. Correct
external-data relocation in `crates/soac_jit/src/jit/runtime_support.rs`,
add a structured true-symbol-address regression, and preserve
tombstone-aware indexed probing, function-captured builtins, and custom
mapping semantics. Both Profile and Apply now pass; separate structured
symbol-remapping and captured-builtins deoptimization regressions also pass.

Host/guest layout inspection requires particular care: on the case-
insensitive macOS host, `vendor/cpython/python` resolves to the capitalized
`vendor/cpython/Python/` source directory, while the Linux guest has a real
lowercase `vendor/cpython/python` executable. The checked-in host CPython
source also omits indexed-dictionary customizations present in the actual
guest binary. The tombstone conclusion is grounded in the real guest
SIGSEGV, faulthandler/address-to-line evidence, and observed custom runtime
symbols, not an assumption that the host source fully describes guest
behavior.

The superficially convenient `_PyDict_LoadBuiltinsFromGlobals` must not be
called from SOAC's extension: actual ELF inspection found it is **LOCAL / not
exported** even though the source is visible. Read the existing
`PyFunctionObject.func_builtins` owned/captured mapping instead; do not add a
third unsupported CPython ABI layout mirror or rely on an unexported symbol.

Initial implementation passes warning-free
`cargo check -p soac_jit --tests`. It owns the real function's captured
`func_builtins`, derives offsets from the authoritative header while removing
the duplicate runtime mirror and **seven test-only fake mirrors**, threads
four-argument global helpers plus explicit **seven-argument cold deopt**
context, and removes the unused exported `dp_jit_load_global_obj`. Although
the actual linked CPython ELF exports `_PyDict_LoadGlobal`, removing it from
the candidate does not eliminate the indexed-deletion crash because the
actual failure is the earlier misrelocated inline tombstone comparison.
`PyMapping_GetOptionalItem` is also **GLOBAL**; the project's PyO3 git 0.28
bindings lack its wrapper, so a direct private Rust extern is required.
Both relocation and missing-error-edge corrections now pass complete
standalone Profile→Apply validation, and the structured inline-data
relocation regression passes. A separate structured deoptimization mapping
test initially hit an unrelated immortal-small-integer refcount assumption;
its fixture now uses nonimmortal large integers. Final deopt/broader
validation now passes, proving the explicit deopt path resolves the original
captured builtins despite rebound globals and retains correct nonimmortal
reference ownership. An existing indexed-global structural fixture required
the legitimate six→seven argument update for the explicit cold-deopt ABI.
The complete existing indexed-global/deopt family now passes **20 / 20**,
including the new plus existing structured deopt tests **2 / 2**. The
runtime-inliner family passes **3 / 3**, including the explicit external DATA
namespace regression **1 / 1**. Cross-strategy Python regressions pass
**9 / 9**, covering captured builtins, original/generator code, closure and
function metadata, shutdown flushing, synthetic iteration shadowing, and
constructors.

The new named structured regressions are
`inlined_runtime_global_symbols_preserve_caller_data_import_identity`
(**1 / 1**) and
`deopt_return_global_uses_captured_function_builtins_after_globals_rebind`
(**1 / 1**, **2 / 2** together with the existing deopt case). Final warning-
free Cargo checking and both scoped workspace/standalone package formatting
checks pass.

## Implementation and compatibility

- Extend the single authoritative `FunctionEnvAbiHeader` to hold an owned
  reference to the actual function's captured `func_builtins` alongside
  existing globals. Increment/decrement ownership with the function-env
  lifecycle; keep all offset consumers synchronized without introducing
  duplicate header mirrors.
- Materialize the mapping once from the already-created `PyFunctionObject`,
  never by re-reading mutable `globals["__builtins__"]` for later loads.
  Preserve old/new function distinctions across a module-level rebinding.
- Thread explicit captured builtins into the raw global helper ABI as
  `(globals, builtins, name, expected_index)`. Existing direct indexed-global
  hits remain fast and return owned results; misses use the supplied
  function-specific builtins mapping.
- Remove the exported `dp_jit_load_global_obj` helper entirely. Repo-wide
  source analysis finds no remaining caller or import: only its exported
  wrapper, legacy hook/panic stub, symbol registry, and documentation
  inventory refer to it. The project does not retain backward-compatibility
  helpers with no production consumers; document the removed exported
  helper and the surviving four-argument contracts explicitly.
- Correct preexisting external **DATA** relocation in
  `crates/soac_jit/src/jit/runtime_support.rs`: predeclare callee data
  symbols in the caller and remap `GlobalValueData::Symbol::User` name
  references before the pinned Cranelift **0.130.1** inliner clones them.
  The inlined guard must compare the true tombstone address, not a JIT-local
  trampoline. Add a structured relocation regression and preserve the fast
  inline path, tombstone-aware probing, captured builtins, mapping callbacks,
  owned references, errors, global precedence, and `NameError`.
- Branch specialized indexed-global fallback NULL results to the existing
  error block, preserving their `NameError` instead of joining NULL into a
  later callable/vectorcall path.
- Use safe `PyDict_GetItemRef` for the exact-dictionary fallback and
  `PyMapping_GetOptionalItem` for mapping subclasses/custom mappings;
  `_PyDict_LoadGlobal` is not needed.
- For dict subclasses or custom mappings, use
  `PyMapping_GetOptionalItem` in CPython order: globals first, then the
  captured builtins mapping. Preserve `__missing__`/`__getitem__` callbacks,
  mapping mutation, exception propagation, and never replace custom lookup
  with an exact-dict shortcut.
- Thread the same captured context through generated JIT entries, cold/deopt
  global loads, explicit seven-argument deopt resume, closure/generator
  resumed execution, background compilation, and any direct global-load
  helper paths; do not silently retain an inconsistent three-argument
  fallback.
- Preserve source-independent optimization, stdlib/user global shadowing,
  function identity/code/default mutation, runtime module lifecycle, and
  profile/apply semantics. Add no new public API, environment variable,
  source fingerprint, global cache, or unsupported CPython symbol.
- A genuine baseline regression already proves old/new function capture is
  wrong (**1 failed in 0.36 seconds**). Preserve coverage of functions
  created after rebinding, live mutation of both captured mappings, custom
  mapping callbacks/failures, and globals precedence.
- Update `doc/RUNTIME_FUNCTIONS.md` for both surviving explicit
  four-argument global helpers and remove the dead exported
  `dp_jit_load_global_obj` inventory entry. The helper removal is an
  intentional exported-surface change, not a hidden compatibility fallback.

## Benchmark protocol and coverage

- Fixed general-purpose exploratory set:
  `chaos,richards,deltablue,comprehensions`; full acceptance remains the
  complete pyperformance suite at **1.10x stock CPython**.
- Historical pre-resize comparison:
  `work/pyperformance/comparison-20260818-173820-fI0KHb/summary.json`;
  paired-stock ratio **0.2475497620x**, contaminated by severe `chaos` /
  `richards` outliers. The earlier
  `work/pyperformance/comparison-20260818-165506-0dnfty/summary.json` has
  ratio **0.2780522558x** and cleaner robust medians. Both are historical
  context only; neither is a same-environment post-resize baseline.
- Completed post-resize baseline on **8 CPUs / 12 GiB / kernel
  6.8.0-137**, before any production edit:
  `work/pyperformance/comparison-20260818-175425-IXk8jU/summary.json`;
  `work/logs/captured-builtins-post-resize-baseline.log`. The fixed-four
  comparison takes **91.94 seconds**, with paired-stock geometric ratio
  **0.2742319248437325x**. This is the sole valid candidate baseline.
- Candidate completion smoke after genuine semantic GREEN:
  `just pyperformance-compare chaos,richards,deltablue,comprehensions 1 '' --debug-single-value`.
  Cold/single-value timings prove completion only, never throughput.
- Completed release smoke:
  `work/pyperformance/comparison-20260818-185017-kePJ2j/summary.json`;
  **25.49 seconds** elapsed, including **17.33 seconds** release build.
  All four workloads complete with unchanged **35 / 21 / 79 / 53** compiled
  functions, `__main__` plus `soac.runtime`, no transformed stdlib, and
  unchanged **2,541 typed blocks / 193 functions**. One-worker native code
  rises from **1,867,424 to 1,887,608 bytes** (**+1.081%**) and machine
  blocks from **123,614 to 125,780** (**+1.752%**); monitor both against
  any measured throughput. Cold single values near 143 ms / 471
  microseconds / 375 ms / 319 ms are not valid headline timings.
- Completed same-resource candidate normal comparison:
  `work/pyperformance/comparison-20260818-185154-TACah3/summary.json`;
  `work/logs/captured-builtins-normal.log`; **101.38 seconds** elapsed on
  the same **8 CPUs / 12 GiB / kernel 6.8.0-137** as the baseline. Its
  Apply setup totals **49.9 seconds** across **40 workers**, with
  **959.2 ms median / 5.11 seconds maximum**. Both comparison artifacts
  have **20 measured values per benchmark**, unchanged transformed modules,
  and identical optimized typed IR.
- Completed candidate native `comprehensions` profile:
  `work/logs/comprehensions-captured-builtins_callgraph.txt` and its
  Speedscope companion; **199 Hz / 30,000 loops**, **34.53 seconds** total,
  **8.094 seconds** measured replay, **91,638,956-byte perf.data**, and
  **zero lost samples**. The perf report rounds its sample count to **1K**;
  Speedscope contains **877 sampled stacks / 100,229 total weights**, which
  are distinct reporting bases. Global slow loads account for **0.86%
  inclusive** versus historical **4.40%** on different VM hardware/kernel;
  the historical comparison is directional, not a controlled before/after.
  Closure creation remains **28.30% inclusive**, shared function
  instantiation **24.79%**, vectorcall registration **6.52%**, and
  `PyString::new` **6.09% aggregate / 4.58% under closure creation**.
  Inclusive shares overlap and must not be added.
- Finalized secondary pystone sanity artifact:
  `work/bench/zolltvqvyovx`; a stable cached run at measured commit
  `a181e40` completes in **14.84 seconds**. Production refcounts-enabled
  runs are **1,146,988 / 1,172,977 / 1,183,274 loops/second**, with headline
  median **1,172,977 loops/second**. Profile and Verify record **565,077**
  and **707,370 loops/second**; the unsound refcounts-disabled diagnostic
  median is **1,116,300 loops/second** and is not a production result.
  Generated pystone output contains **239,660 native bytes / 15,764 machine
  blocks**. A discarded first attempt took **157.84 seconds** and produced
  unstable runs **196,724 / 161,583 / 461,952 loops/second** (median
  **196,724**) while paying a roughly **61-second first release-inspector
  build** plus a recreated venv; it is not representative. The guest also
  initially lacked `jj`; installing verified official `jj` **0.44**
  unblocked the required finalized-revision snapshot. The artifact is
  refreshed after documentation freeze against the exact final revision.
  No matched prior pystone result exists, so make no before/after claim.
- Final full correctness gate: `just test-all` passes;
  `work/logs/captured-builtins-test-all.log`. All **1,214 Python nodeids /
  81 batches across eight workers** pass, together with **547 `soac_jit` /
  368 `soac_lowering` / 202 `soac_opt` / 8 `soac_pyo3` Rust tests**.
  Cargo takes **76.482 seconds**, pytest **108.505 seconds**, the slow
  counter-dump batch **108.35 seconds**, the complete test phase
  **185.003 seconds**, and total wall time **206.94 seconds**.
- Finalized performance entry: `doc/PERF_LOG.md`, change `zolltvqv`;
  pystone uses full change ID `zolltvqvyovx`, with its exact-final-revision
  artifact refreshed after documentation freeze.
- Existing transformed modules are `__main__` plus `soac.runtime`, with no
  transformed standard library; historical compiled counts are
  **35 `chaos` / 21 `comprehensions` / 79 `deltablue` / 53 `richards`**.

## Measurements

| Benchmark | Historical SOAC median before VM resize | New 8-CPU baseline | Candidate stock / SOAC mean / SOAC median | Same-environment previous / candidate |
| --- | --- | --- | --- | --- |
| `chaos` | 80.9583570 ms | stock 30.3165924 ms; SOAC mean 78.1517575 ms; median 77.4602805 ms | stock 29.7052543 ms; SOAC mean 87.5356763 ms; median 72.0015015 ms | mean 0.892799x; median **1.075815x**; not significant |
| `comprehensions` | 88.8434829 microseconds | stock 7.9933752 microseconds; SOAC mean 90.0923449 microseconds; median 89.1362441 microseconds | stock 7.8906353 microseconds; SOAC mean 87.8145440 microseconds; median 85.5721665 microseconds | mean 1.025939x; median **1.041650x**; not significant |
| `deltablue` | 4.5496985 ms | stock 1.4686011 ms; SOAC mean 4.5785340 ms; median 4.5561716 ms | stock 1.4968931 ms; SOAC mean 4.4302400 ms; median 4.3779513 ms | mean **1.033473x**, significant; median **1.040709x** |
| `richards` | 44.5541300 ms | stock 22.4378025 ms; SOAC mean 43.7995481 ms; median 42.3916908 ms | stock 22.0023144 ms; SOAC mean 40.4881292 ms; median 40.2460413 ms | mean **1.081787x**, significant; median **1.053313x** |

Historical figures are **not comparable across the VM resize or
6.8.0-124→6.8.0-137 kernel change**. Compare candidates only with the
verified 8-CPU / 12-GiB baseline. The robust geometric improvement across
all four same-resource SOAC medians is **1.0527772907x**. Mean-based SOAC
improvement is only **1.0059561519x** because candidate `chaos` includes a
**355.788 ms** outlier; do not substitute its arithmetic mean for its
**72.0015015 ms** median or claim that its improvement is statistically
significant. Pyperf reports only `deltablue` and `richards` significant.
The baseline and candidate stock-relative geometric ratios are respectively
**0.2742319248x** and **0.2735422874x**; the full-suite **1.10x** goal is
not achieved.

| Guardrail | Historical pre-resize | New baseline | Candidate |
| --- | --- | --- | --- |
| `chaos` global slow-path inclusive CPU | 10.27% | pending | pending |
| `chaos` repeated builtin lookup inclusive CPU | 5.52% | pending | pending |
| `comprehensions` global slow-path inclusive CPU | 4.40% | unavailable on same hardware | 0.86%; cross-hardware comparison directional only |
| `comprehensions` repeated builtin lookup inclusive CPU | 2.73% | pending | pending |
| Optimized typed-IR final basic blocks | 2,541 | 2,541 | 2,541; unchanged |
| Optimized typed-IR function instances | 193 | 193 | 193; unchanged |
| Pre-optimization serialized BlockPy bytes | 8,171,456 | 8,171,456 | 8,171,456; unchanged |
| Apply-mode native emitted bytes | 18,674,240 | 18,674,240 | 18,876,080; **+1.081%** |
| Apply-mode native machine blocks | 1,236,140 | 1,236,140 | 1,257,800; **+1.752%** |

## Attempt history

### Attempt 1: Carry captured builtins through global load ABI

- Change: own each Python function's actual captured builtins in its existing
  function environment, then use one explicit four-argument global helper
  with tombstone-aware indexed globals, safe exact-dict lookup, and
  semantics-preserving custom-mapping branches;
  remove the proven-unused exported `dp_jit_load_global_obj` helper.
- Evidence: modern CPython captures `func_builtins` at function creation;
  existing SOAC repeatedly reloads mutable module `__builtins__`. Historical
  zero-loss profiles show global fallback up to **10.27% inclusive**.
- Compatibility: old/new functions across rebinding, mutations of captured
  mappings, custom callbacks/errors, direct indexed global hits, cold deopt,
  owned references, and CPython `NameError` all require focused coverage.
- Tests/measurements: same-resource post-resize/kernel baseline established
  in **91.94 seconds** with **0.2742319248x** paired-stock score; genuine
  Profile old/new mapping RED (**1 failed in 0.36 seconds**, `[21, 22]`
  instead of `[11, 22]`). Candidate explicit-ABI implementation passes
  warning-free `cargo check -p soac_jit --tests`, but first focused runtime
  validation crashes **SIGSEGV (-11)** after indexed-global deletion. Native
  GDB proves actual tombstone `0xfffff7f8f2c4` versus incorrect JIT-local
  relocated data address `0xffffe4a005e8`; slow CPython dictionary helpers
  were never reached. Caller symbol remapping fixes the complete Profile
  matrix, while an additional generated NULL→error edge fixes Apply's
  missing captured-builtin `NameError`. Complete unchanged Profile→Apply
  regression is GREEN (**1 passed in 0.79 seconds**), and structured
  Cranelift regression proves preserved inlining plus caller-owned external
  DATA namespace 1 / data ID. A second structured deopt regression passes
  with a captured mapping after globals rebinding and nonimmortal reference
  ownership; the existing structural fixture was updated from old six-arg
  to new seven-arg cold deopt. Indexed-global/deopt family **20 / 20**,
  structured deopt pair **2 / 2**, inliner family **3 / 3**, and
  cross-strategy Python guardrails **9 / 9** pass. Final warning-free Cargo
  check plus scoped workspace/standalone formatting checks also pass.
  Same-resource four-workload normal sampling shows robust median geometric
  improvement **1.0527772907x**, including significant `deltablue`
  **1.033473x** and `richards` **1.081787x** mean speedups. `chaos` and
  `comprehensions` are not significant; a **355.788 ms** `chaos` outlier
  limits the mean geometric result to **1.0059561519x**. Coverage and typed
  IR remain unchanged, with **+1.081% native bytes / +1.752% machine
  blocks**. Stable cached pystone completes at production median
  **1,172,977 loops/second** in **14.84 seconds**; its discarded
  first-build-contaminated preliminary median was **196,724**, and no
  matched prior pystone result exists. Zero-loss
  candidate native profiling finds global slow loads at **0.86% inclusive**
  and closure creation still dominant at **28.30% inclusive**; the old
  **4.40%** global-load share was captured under different VM hardware and
  is directional only. The full `just test-all` gate passes **1,214 Python
  nodeids / 81 batches** plus **547 JIT / 368 lowering / 202 optimizer /
  8 PyO3 Rust tests** in **206.94 seconds** total.
- Result: LANDED / RETAIN; focused and expanded CPython-correct semantics
  are GREEN, normal sampling shows two statistically significant workload
  wins and a positive robust-median trend, zero-loss profiling verifies the
  remaining hotspot distribution, and the complete gate passes.

## Verdict and next action

- Verdict: LANDED / RETAIN for CPython correctness and measured SOAC
  performance. Source/profile evidence, a clean same-resource
  baseline, and a genuine user-visible old/new function-capture RED support
  a semantics-first strategy; the candidate compiles cleanly, but initial
  runtime validation exposes a reproducible indexed-global-deletion
  **SIGSEGV** caused by GDB-confirmed incorrect external-data relocation.
  Caller-symbol remapping plus a specialized NULL→`NameError` edge makes
  complete Profile→Apply semantics GREEN (**1 passed in 0.79 seconds**).
  Both structured inliner relocation and captured-builtins deopt
  regressions pass, with **20 indexed/deopt**, **3 inliner**, and **9
  cross-strategy Python** guardrails GREEN. Final checking/scoped formatting
  pass; same-resource normal sampling improves robust median geometric
  throughput **1.0527772907x**, with significant `deltablue` and `richards`
  gains only. The noisy arithmetic-mean geometric result is
  **1.0059561519x**, native code grows **1.081%**, and the stock-relative
  score remains just **0.2735422874x**. Stable cached pystone completes at
  production median **1,172,977 loops/second**; no matched previous result
  supports a pystone speedup claim. The
  zero-loss candidate profile finds global slow loads at **0.86% inclusive**
  and closure creation at **28.30% inclusive**. The complete gate passes
  **1,214 Python nodeids / 81 batches**, **547 JIT / 368 lowering /
  202 optimizer / 8 PyO3 Rust tests**, with **185.003 seconds** test phase
  and **206.94 seconds** total. `doc/PERF_LOG.md` records finalized change
  `zolltvqv`; stable cached pystone remains only a correctness/regression
  guardrail, with artifact `work/bench/zolltvqvyovx` refreshed after
  documentation freeze against the exact final revision.
- Transferable lesson: CPython binds builtins to each function, not to the
  module's future `__builtins__`; source-visible symbols may not be exported,
  and an exported exact-dict helper may still be incompatible with SOAC's
  running custom indexed-dictionary layout.
- Next action: pursue the remaining closure/generator-factory hotspots as a
  separate documented strategy; compare only against the verified **8-CPU /
  12-GiB / kernel-6.8.0-137** environment and never represent this
  fixed-four subset as the complete pyperformance acceptance target.
