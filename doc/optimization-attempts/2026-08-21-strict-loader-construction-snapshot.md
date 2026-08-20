---
title: "Verify analysis inputs once per strict loader construction"
---

# Verify analysis inputs once per strict loader construction

- Status: in progress
- Pacific date: 2026-08-22 PDT
- Change or revision: shared strict-runtime implementation; not yet finalized
- Outcome: candidate freshness regression passes; three paired debug-constructor measurements improve; full-project gate and suite performance remain pending

## Hypothesis and evidence

The baseline strict loader verifies all observed analysis inputs before its
catalog loop, then repeats that filesystem sweep for every selected module via
`StrictArtifactDeployment::verified_analysis_dependencies`. A 26-module
deployment therefore reads and hashes the complete input set 27 times during
one loader construction. Shared System dependencies are independently read
again for each consumer. These are callback-free operations inside a single
`StrictArtifactLoader::from_snapshot`, not distinct module admissions.

The migrated entry-runtime suite exposed this startup cost: its first two
independent, debug-runtime subprocesses took 66.34 s and 65.77 s. Those are
workflow observations, **not a controlled optimization baseline**. The test
suite now preserves all 52 named outcomes using one subprocess per execution
requested mode, with independent modules and state-interference checks; all 52
pass. An independent entry-pointer audit subsequently found eager compilation
overrode the requested interpreter mode, so those passes do not establish two
distinct execution paths. The corrected v8 rerun now passes all 52 with exact
public-entry witnesses: 23 synchronous cases in both entry modes, and three
suspended-function cases with factory-entry evidence only.
That batching is test-only and does not change production authentication.

Hypothesis: retaining one private, verified observation snapshot only for the
callback-free constructor removes redundant content hashing as the selected
catalog grows. It should reduce startup time, not steady-state Python execution
time. Long-running workload performance remains the primary project target;
this is a bounded prerequisite/workflow improvement, not evidence of progress
toward the pyperformance geometric-mean goal.

## Implementation and compatibility

- Construct an opaque, non-serializable `VerifiedAnalysisSnapshot`, publicly
  re-exported by `soac_contracts` and tied to the immutable startup
  descriptor. Validate the descriptor and all filesystem observations once.
  Read each distinct System dependency path with the existing size bound, then
  compare its actual SHA-256, size, and historical source hash with every
  consumer's expected record.
- Deduplicate only file observations. Derive each consumer's dependency
  fingerprint independently, preserving import path, source role, actual
  selected-module policy, per-file configuration, and the checker/typeshed
  domain of vendored sources. An empty dependency list still requires all
  analysis inputs to match.
- Keep the snapshot local to `from_snapshot`; drop it before publishing the
  loader. It carries neither native object authority nor permission to reuse
  stale observations across imports.
- Construction performs Rust/OS operations only: no Python allocation, DECREF,
  user callback, or interpreter-visible publication occurs in the snapshot's
  lifetime. The filesystem observations are not an atomic filesystem snapshot;
  the old repeated scans did not provide that property either.
- Preserve fresh native, environment, source, and dependency validation in
  every later `load_verified` call. No timestamp cache, cross-import cache,
  altered directory filter, manifest-derived expectation, or fallback is added.
- Focused coverage: all **51 shared-contract tests pass**, including
  per-consumer equivalence, empty-consumer full-input checking, and
  path/role/configuration/vendored-domain rejection. A genuine dependency
  mutation test after loader construction passes against both the baseline
  runtime and the staged v8 candidate. The same-size dependency edit still
  blocks the later selected import before its body runs.

## Benchmark protocol and coverage

- Fixed startup workload: the same genuine 26 source bodies used by
  `tests/test_strict_entry_runtime.py`, selected CPython v8, and a loader-
  construction-only driver. Keep the native build, deployment, environment,
  Python support files, and input bytes identical between variants.
- Baseline artifact: a private byte-for-byte copy of the staged v7 debug
  extension, SHA-256
  `1ee760a986864b72125bf8ff6c77883d7f736be76eb0eba7f347afa8b04999c8`,
  at `work/strict-loader-snapshot/baseline/_soac_ext.so`. The Python support
  package is also copied; the staged extension remains untouched.
- Candidate artifact: a private copy of the staged v8 debug extension,
  SHA-256 `487191ff0a92b119a579949b84070dc61f330149a941deac56c22d89255ba9e2`,
  at `work/strict-loader-snapshot/candidate-v8/_soac_ext.so`. The baseline
  binary genuinely loads on the same selected v8 interpreter and passes the
  native startup-descriptor and complete constructor checks. Both binaries
  consume one fresh deployment of the same 26 source bodies. Three pairs use
  fresh subprocesses in baseline/candidate, candidate/baseline, baseline/candidate
  order while all other project builds and tests are paused. No symbol or
  native identity check is relaxed. These are preserved full debug extensions
  from the shared implementation, not isolated single-change release builds.
- Stock comparison: not available for this strict artifact-verification
  operation. A stock Python process does not perform the corresponding work.
- Pyperformance stock/SOAC and previous/candidate comparisons: pending; no
  suite-wide or steady-state speedup is claimed by this startup experiment.
- Coverage: the fixed deployment selects all 26 entry modules; the startup
  driver authenticates the complete generation without executing any module
  initializer, source-function body, IR lowering, or generated JIT code. It
  calls the actual public module-creation entry with an ordinary source, which
  constructs the loader before returning non-selection. Module initializers
  in the separate behavior suite intentionally use the interpreter regardless
  of requested source-function mode. The corrected 52-case suite proves strict
  admission and the public entries described above. Dependency and standard-library source
  observations are authenticated, not thereby transformed.

## Measurements

| Metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| Controlled paired 26-source loader construction wall, median of 3 | 62.141 s | 3.586 s | -94.23% |
| Constructor CPU, median of 3 | 61.631 s | 3.569 s | -94.21% |
| Whole-process wall, median of 3 | 62.214 s | 3.653 s | -94.13% |
| Single v7 constructor sanity, not a paired comparison | 62.756 s wall / 62.209 s CPU | unavailable | not claimed |
| Stock CPython elapsed / strict SOAC apply elapsed | unavailable for this operation | pending suite measurement | not claimed |
| Previous SOAC / candidate SOAC suite throughput | pending | pending | not claimed |
| Complete input sweeps per constructor (source inspection) | 27 | 1 implemented | not a timing result |
| Optimized typed-IR blocks | unavailable; startup-only change | pending | no intended IR change |
| Pre-optimization BlockPy bytes | unavailable | pending | no intended IR change |
| Apply-mode native code bytes / machine blocks | unavailable | pending | no intended generated-code change |

The three constructor wall-time pairs are **61.820568 / 3.586392 s**,
**62.268844 / 3.555907 s**, and **62.141434 / 3.649261 s** (baseline/candidate).
The median ratio is 17.33x for this debug startup operation only. The old
62.756 s v7 sanity sample is not included. Whole extension file sizes are
373,415,336 and 380,973,872 bytes, respectively; these include unrelated shared
changes and are not generated-code-size measurements. No steady-state or
pyperformance improvement is established here.

## Attempt history

### Attempt 1: constructor-local verified observations

- Baseline: `work/logs/strict-entry-runtime-v7.log` contains the two initial
  independent passes and their startup-dominated timings. The run was stopped
  deliberately rather than waiting approximately an hour for 52 processes.
- Test-only batching: `work/logs/strict-entry-runtime-v7-batched.log` records
  **52/52 PASS in 436.31 s** on the unchanged v7 extension. These are requested-
  mode passes with real strict admission; eager compilation overrode the
  interpreter-mode request, and a mode-authenticated rerun is required. This is
  not a before/after production benchmark.
- Production implementation is written; shared-contract tests pass 51/51
  (`work/logs/strict-loader-snapshot-contracts.log`). The v8 native-linked
  strict Rust gate passes 81 tests. The genuine post-construction mutation
  regression passes on the candidate in the corrected 111-case checkpoint;
  that checkpoint's only two failures are an unrelated match-pattern scope
  bug, retained in the migration report.
- The first private baseline probe correctly rejected a replay environment
  mismatch: `just --command` alone does not prepend the selected library path
  as `_pytest-run` does. The measurement runner now reproduces that existing
  recipe's environment and checks all observed environment hashes before
  launching. It does not rewrite the descriptor or weaken runtime validation.
  This rejected run supplies no timing result.
- The corrected private-binary probe authenticates the original 26-source
  deployment and returns ordinary non-selection through the actual public
  module-creation entry. The one v7 sample measures **62.756 s constructor wall
  time / 62.209 s CPU**, with **62.820 s whole-process wall time**. Evidence:
  `work/logs/strict-loader-snapshot-baseline-environment.log` and
  `work/strict-loader-snapshot/baseline-sanity-environment/measurements.json`.
  This verifies the isolation protocol; it is not a speedup measurement.
- The new real-checker/native-startup regression passes against the baseline
  extension: a first selected import seals, a consumed ordinary dependency is
  edited without changing its size, and the next selected import is rejected
  before its body runs (**1/1 PASS**, 48.91 s including checker setup,
  `work/logs/strict-loader-snapshot-mutation-baseline.log`). Candidate rerun
  passes in `work/logs/strict-runtime-v8-entry-policy-cohorts.log` (11.88 s
  including its setup, not a timing comparison).
- Controlled paired timing passes for all six independent processes. Evidence:
  `work/logs/strict-loader-snapshot-v8-paired.log` and
  `work/strict-loader-snapshot/v8-paired/{measurements,summary}.json`.
  Per-process logs preserve the selected binary hash, native ABI, deployment
  generation, and startup descriptor SHA-256
  `fb612676d37bf3297c0cca8e6f3f3e7bd5dbcb353db74fc6682706345a34dee9`.
- Fixture preservation issue: pytest cleanup removed the original temporary
  v7 deployment before the cross-generation comparison was prepared. The
  private baseline binary, Python support, descriptor identity metadata, and
  unpaired sanity logs are preserved, but that temporary deployment is no
  longer replayable. The fresh comparison must analyze the same reviewed 26
  source bodies once on the selected native build and give that one deployment
  to both binaries. Keep benchmark fixtures outside pytest's retention
  directory and archive their bytes and original path identities under ignored
  `work/`; do not substitute the earlier unpaired timing for a missing
  controlled baseline.
- Fresh v8 publication succeeds for all 26 sources at
  `/tmp/soac-strict-loader-snapshot-v8-01a02587/authority/deployment.json`,
  generation `ed810221e0aa840883ba34ad816f58a2b532e622265f11262fda47fd2d7d3fdf`,
  ABI `21bcaa04d098c2c909fc85d583a3c11b65f108495051036b6d58524a187ae08a`.
  Source digests and archive paths are in
  `work/strict-loader-snapshot/v8-comparison.json`; the complete source,
  authority, and artifact tree is archived under `v8-comparison-archive` in
  the same directory. Its absolute guest paths are preserved, not rewritten:
  an archive copy is not itself relocatable runtime authority. The descriptor
  has 67 file inputs totaling 37,539,889 bytes, four directory inputs,
  20 missing inputs, and 1,509 consumed-dependency records. It does not observe
  the staged extension file. This establishes the common workload; the later
  quiet-window measurements above independently establish compatibility and
  constructor timing for both private binaries.

## Verdict and next action

The bounded snapshot preserves the tested later-admission freshness boundary
and materially reduces this measured debug startup operation. Keep it in the
shared candidate, subject to the full-project gate and final integration.
The required stock/previous/candidate pyperformance comparisons remain pending;
do not extrapolate steady-state or suite-wide performance from this result.
