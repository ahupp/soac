---
title: "Batch JIT artifact JSONL writes"
---

# Batch JIT artifact JSONL writes

- Status: landed
- Pacific date: 2026-08-18 PDT
- Baseline: retained import-time constructor specialization, recorded in
  `doc/optimization-attempts/2026-08-18-import-time-constructor-specialization.md`.
- Outcome: retained workflow improvement. Structured RED-to-GREEN proves
  nested JSONL record writes fall
  from **68 to one** while preserving consecutive valid records. A matched
  three-workload smoke verifies **58.6–268.7x** lower summary-write time,
  **3.91–39.11x** lower worker setup, unchanged complete JSONL artifacts,
  and identical emitted code. A normal-sampling workflow falls from
  **192.73 to 71.50 seconds**, with unchanged generated code and no supported
  steady-state throughput claim. Default detailed basic-block-map profiling
  now completes in **9.76 seconds** instead of timing out after more than two
  minutes. The full correctness gate passes; the **0.3693040x** paired-stock
  subset result remains below the full-suite **1.10x** objective.

## Hypothesis and evidence

SOAC emits a compact `jit-code-summary.jsonl` record whenever it commits a
compiled function. With `SOAC_JIT_BB_MAP=1`, the same helper also emits
optional `jit-bb-map.jsonl` records. In
`crates/soac_jit/src/jit/backend.rs`, `append_jit_artifact_record` opens the
artifact with `OpenOptions::append(true)` and streams
`serde_json::to_writer` directly into an unbuffered `File`; a separate
`write_all` appends the newline. Streaming JSON to an unbuffered file can
perform many small writes for every object key, nested value, or separator.
Each write crosses the Lima host/guest shared mount, so artifact serialization
can dominate JIT commit time even though the actual compiler and linker are
fast. A genuine structured regression directly confirms the amplification:
one nested JSONL object triggers **68 underlying `Write::write` calls** in
the unchanged baseline instead of the required one.

The constructor-strategy completion-only mixed smoke is recorded at
`work/pyperformance/comparison-20260818-143734-Yxp7Z4`. Its benchmark worker
directories contain both `events.jsonl` commit timing and
`jit-code-summary.jsonl` output. Summing the structured
`soac.jit_commit_detail` events yields:

| Benchmark worker | JIT commit events | Summary JSONL records across profile/apply | Total commit time | Code-summary write time | Summary share |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 67 | 134 | 2.802985 s | 2.684747 s | 95.78% |
| `deltablue` | 152 | 310 | 22.412076 s | 21.382788 s | 95.40% |
| `richards` | 104 | 206 | 5.219883 s | 4.922902 s | 94.31% |

The JSONL totals combine two distinct subprocesses per benchmark: `chaos`
contains **64 profile + 70 apply** records, `deltablue` contains **152
profile + 158 apply**, and `richards` contains **101 profile + 105 apply**.
These combined line counts are not the same measurement population as the
single `events.jsonl` commit-event counts. Baseline `deltablue` also spends
**0.974904 seconds** writing jitdump, while actual function definition takes
**0.014136 seconds** and finalization **0.016528 seconds**. The apparent
20-second setup outlier is primarily artifact I/O, not code generation or
measured Python throughput.

A separately completed ordinary-sampling constructor comparison,
`work/pyperformance/comparison-20260818-144613-sAgzx3/summary.json`, spends
**92.3 seconds** in aggregate apply-worker setup and has a **4.74-second**
maximum worker setup. Its complete three-workload workflow takes **192.73
seconds**. Setup time and measured benchmark values are distinct: normal SOAC
apply means are `chaos` **96.9252632 ms**, `deltablue` **4.6228215 ms**, and
`richards` **48.2372796 ms**. Their robust medians are respectively
**95.8330440 ms**, **4.5808291 ms**, and **44.0575340 ms**.

The expected improvement is fewer file-write calls, faster summary/BB-map
artifact emission, lower worker setup, and a cheaper full-suite optimization
workflow. Do not frame this observability-I/O change as a steady-state Python
execution specialization or claim a 10% pyperformance throughput gain from
reduced benchmark setup.

## Implementation and compatibility

- Isolate the existing artifact-record serialization in a private
  `write_jit_artifact_record(&mut impl std::io::Write, &Path,
  &serde_json::Value)` helper. Serialize the complete JSON object with
  `serde_json::to_vec`, append exactly one newline byte to that in-memory
  buffer, and invoke `writer.write_all(&buffer)` once per logical record.
- Keep the existing directory creation, file open, append/create mode,
  per-record completion, stderr error reporting, and path-aware error
  messages. The file is still opened with `O_APPEND`; the serialized record
  remains immediately visible after the helper returns. Do not retain a
  process-wide `BufWriter`, defer flushing, or change artifact lifetime.
- A successful ordinary file write now submits an entire JSONL record rather
  than many serializer fragments. `write_all` may internally retry a partial
  or interrupted OS write, so do not promise impossible single-syscall
  behavior under all failure conditions; the invariant is one logical
  `write_all` invocation on one complete serialized record.
- Preserve concurrent append semantics by retaining `OpenOptions::append`.
  Writing a full record together reduces interleaving exposure compared with
  the prior fragmented serializer, but filesystem-specific cross-process
  guarantees remain those of the existing append-backed filesystem.
- Preserve JSON field names, values, ordering emitted for the existing
  `serde_json::Value`, UTF-8, escaping, trailing newline, and independent
  records. Continue to support both ordinary code-summary and optional
  detailed basic-block-map artifacts without changing their schemas.
- No Python-facing semantic change, constructor assumption, mutable type
  guard, benchmark/source recognizer, new environment variable, LLVM/Cranelift
  change, or generated-code change is intended.
- Focused structured Rust regression:
  `jit_artifact_records_are_complete_single_write_jsonl_lines`. A counting
  writer accepts nested arrays/objects and Unicode, verifies exactly one
  write per complete record, a trailing newline, valid parsed JSON, and a
  second correctly appended record. The genuine baseline RED fails because
  the first nested record invokes `write` **68 times instead of once**;
  **543 other tests are filtered**. After switching to `serde_json::to_vec`,
  adding one newline, and invoking `write_all` once, the identical regression
  passes: **one test passed, 543 filtered**. It verifies two consecutive
  nested/Unicode records, complete valid JSON, and exactly one write apiece.
  Warning-free `cargo check -p soac_jit --tests` passes in **3.65 seconds**;
  package-scoped Rust formatting and formatting checks also pass.
- Existing transformed-runtime subprocesses and real worker artifacts verify
  immediate JSONL availability and unchanged coverage. The full
  `just test-all` correctness gate passes **1,209 Python node IDs across 73
  batches** and the entire Rust workspace, including the new writer test.
- The optional `SOAC_JIT_BB_MAP=1` path uses the same record writer. Before
  this fix, the default detailed native profile timed out after more than
  **120 seconds**, with approximately **123 seconds** spent emitting map
  records. After the fix, the unchanged default
  `just pyperformance-deep-profile-from-profile ... chaos loops=12
  output_prefix=work/logs/chaos-artifact-single-write-bbmap` completes in
  **9.76 seconds**. It preserves a valid **2.9 MB `jit-bb-map.jsonl`**,
  captures **1,234 perf samples / 77.155 MB / zero lost samples**, and
  produces a **257 KB Speedscope** artifact. Full diagnostic output is in
  `work/logs/jit-artifact-single-write-bbmap-profile.log`. This proves
  large optional map rows remain complete and the existing native profiling
  workflow no longer requires disabling detailed maps.

## Benchmark protocol and coverage

- Primary comparison objective: `jit_commit_code_summary_us`, total
  `jit_commit_total_us`, profile/apply setup time, artifact record counts and
  validity, and full comparison wall time. Headline trained apply throughput
  is a regression guardrail, not the hypothesized win.
- Fixed exploratory mixed subset: `chaos,richards,deltablue`. The full
  pyperformance suite and its **1.10x** stock-CPython acceptance target remain
  separate and unachieved.
- Baseline cold smoke and exact per-worker events:
  `work/pyperformance/comparison-20260818-143734-Yxp7Z4`.
- Baseline ordinary-sampling stock/profile/apply comparison:
  `work/pyperformance/comparison-20260818-144613-sAgzx3/summary.json`;
  output log `work/logs/import-time-constructor-representative.log`.
- Candidate completion-only comparison:
  `just pyperformance-compare chaos,richards,deltablue 1 '' --debug-single-value`.
  Its cold measured values cannot establish steady-state throughput, but its
  worker setup, commit-event breakdown, valid JSONL records, and coverage can
  directly test this workflow hypothesis.
- Completed matched candidate smoke:
  `work/pyperformance/comparison-20260818-150834-CWHJy7/summary.json`;
  full output is in `work/logs/jit-artifact-single-write-smoke.log`. It
  completes in **30.55 seconds**, including an **18.05-second** release
  extension rebuild. Its **1.04-second total profile setup** and
  **1.89-second total apply setup** are separate from cold single-value
  benchmark timings.
- Candidate ordinary-sampling regression comparison, if needed:
  `just pyperformance-compare chaos,richards,deltablue 1 work/pyperformance/comparison-20260818-144613-sAgzx3`.
- Completed normal-sampling candidate:
  `work/pyperformance/comparison-20260818-151012-O9YKkM/summary.json`;
  complete output is in `work/logs/jit-artifact-single-write-representative.log`.
  It uses the same three workloads, normal worker/sample shape, ten apply
  workers per benchmark, and prior constructor-specialized SOAC baseline.
- Completed full correctness gate:
  `work/logs/jit-artifact-single-write-test-all.log`. `just test-all` passes
  **1,209 Python node IDs across 73 batches**, **544 `soac_jit`**, **367
  `soac_lowering`**, **202 `soac_opt`**, and **eight PyO3-extension** tests.
  Cargo tests take **71.722 seconds**, pytest takes **104.708 seconds**, the
  complete test phase takes **176.445 seconds**, and total elapsed time is
  **200.69 seconds**. The slowest counter-dump batch takes **103.13
  seconds**.
- Each baseline benchmark transforms its own `__main__` benchmark module and
  `soac.runtime`; no standard-library or external dependency module is
  transformed. Apply compiles **35 `chaos`**, **79 `deltablue`**, and
  **53 `richards`** functions, totaling **167**. The candidate smoke
  completes all three with identical module/function coverage and no
  transformed standard-library modules.
- Use consistent worker counts for aggregate code-size comparisons. The
  writer change should not alter optimized typed blocks, serialized BlockPy,
  native emitted code, native machine blocks, or selected constructor direct
  edges.

## Measurements

| Workflow metric | Baseline | Candidate | Change |
| --- | --- | --- | --- |
| Structured nested-record underlying write calls | 68 | 1 | -67 / -98.53% |
| `deltablue` commit events / summary records | 152 / 310 | 151 / 310 | one fewer event; exact artifact lines preserved |
| `deltablue` total JIT commit | 22.412076 s | 0.307316 s | 72.93x less time |
| `deltablue` code-summary writes | 21.382788 s | 0.079588 s | 268.67x less time |
| `richards` total commit / code-summary writes | 5.219883 s / 4.922902 s | 0.231921 s / 0.084009 s | 22.51x / 58.60x less time |
| `chaos` total commit / code-summary writes | 2.802985 s / 2.684747 s | 0.171871 s / 0.041681 s | 16.31x / 64.41x less time |
| Ordinary-sampling aggregate profile setup | 59.9 s | 9.01 s | 6.65x less / -84.96% |
| Ordinary-sampling aggregate apply setup | 92.3 s | 33.4 s | 2.76x less / -63.81% |
| Median ordinary-sampling apply-worker setup | 2.93 s | 0.981 s | 2.99x less |
| Maximum ordinary-sampling apply-worker setup | 4.74 s | 4.76 s | +0.02 s; isolated outlier |
| Ordinary-sampling mixed comparison wall time | 192.73 s | 71.50 s | 2.70x less / -62.90% |
| Default detailed basic-block-map profile | timeout after >120 s | 9.76 s | completes with 1,234 samples |

| Steady-state and generated-code guardrail | Previous baseline | Candidate | Change |
| --- | --- | --- | --- |
| `chaos` SOAC median | 95.8330440 ms | 95.1565940 ms | 1.00711x; throughput guardrail |
| `deltablue` SOAC median | 4.5808291 ms | 4.5420787 ms | 1.00853x; throughput guardrail |
| `richards` SOAC median | 44.0575340 ms | 42.7113685 ms | 1.03152x; throughput guardrail |
| Paired-stock geometric speedup | 0.3567255x | 0.3693040x | below 1.10x goal |
| Optimized typed-IR final basic blocks | 2,055 | 2,055 | identical |
| Optimized typed-IR function instances | 167 | 167 | identical |
| Pre-optimization serialized BlockPy bytes | 6,311,524 | 6,311,524 | identical |
| Apply-mode native emitted bytes | 15,679,400 | 15,679,400 | identical |
| Apply-mode native machine blocks | 1,038,180 | 1,038,180 | identical |

The normal-sampling candidate SOAC means are `chaos` **94.5617740 ms**,
`deltablue` **4.5540016 ms**, and `richards` **44.1866514 ms**; paired stock
means are respectively **30.2450146 ms**, **1.4563872 ms**, and
**21.7581136 ms**. Its previous-SOAC mean-based geometric ratio is
**1.0433796x** and its robust median-based ratio is **1.0156580x**. Only the
`chaos` mean difference is significant in the pyperf comparison;
`deltablue` and `richards` are hidden as statistically insignificant. Since
the writer does not change any generated-code tuple or Python execution
semantics, do not attribute a throughput improvement to these noisy
differences. The measured win is worker setup and end-to-end workflow time.

Normal-sampling matched apply-worker measurements confirm that the smoke
result is not solely a single shared-mount outlier:

| Benchmark | Previous average apply setup | Candidate average apply setup | Previous average summary write | Candidate average summary write | Summary speedup |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 2.4091 s | 0.9194 s | 35.2088 ms | 0.6207 ms | 56.73x |
| `deltablue` | 3.8154 s | 0.9884 s | 34.5719 ms | 0.5628 ms | 61.43x |
| `richards` | 2.9114 s | 1.0039 s | 35.0450 ms | 1.1006 ms | 31.84x |

Average main-process commit time falls from **1.3320 to 0.0962 seconds**
for `chaos`, **2.7748 to 0.1663 seconds** for `deltablue`, and **1.9096 to
0.1177 seconds** for `richards`. With ten apply workers each, candidate
apply JSONL rows are exactly **700**, **1,580**, and **1,050** respectively;
the corresponding body/adapter counts are **350 + 350**, **790 + 790**, and
**530 + 520**. All JSON lines parse and every generated
`(entry_kind, function_qualname, code_size, machine_block_count)` remains
identical to its previous-SOAC counterpart.

The first five measured candidate rows compare the same three-workload
`--debug-single-value` configuration before and after the writer change;
they do not compare a debug smoke against a normal-sampling baseline. The
previous `deltablue` timing includes an unusually severe shared-mount outlier,
so its **268.67x** summary-write reduction is an observed same-mode outcome,
not a guaranteed typical speedup. `richards` and `chaos` independently show
**58.60x** and **64.41x** lower summary-write time without relying on that
one `deltablue` outlier.

Matched apply-worker setup and main-process commit timings are:

| Benchmark | Previous apply setup | Candidate apply setup | Setup improvement | Previous main commit | Candidate main commit |
| --- | --- | --- | --- | --- | --- |
| `chaos` | 3.127305 s | 0.799890 s | 3.91x | 1.351802 s | 0.102641 s |
| `deltablue` | 20.530796 s | 0.524946 s | 39.11x | 19.922434 s | 0.166376 s |
| `richards` | 4.990303 s | 0.562710 s | 8.87x | 3.743515 s | 0.150456 s |

The less outlier-sensitive previous ordinary-sampling per-worker setup
references are **2.4091 seconds** for `chaos`, **3.8154 seconds** for
`deltablue`, and **2.9114 seconds** for `richards`. These are a different
sampling mode and must be labeled separately from the matched-smoke ratios.
The candidate's average summary-write time per apply record is approximately
**622.1 microseconds** for `chaos`, **527.1 microseconds** for `deltablue`,
and **807.8 microseconds** for `richards`.

All candidate JSONL lines parse successfully. Exact combined profile/apply
line counts remain **134 `chaos`**, **310 `deltablue`**, and **206
`richards`**. Apply-only record counts are respectively **70** (35 body + 35
adapter), **158** (79 + 79), and **105** (53 + 52). For every benchmark,
every `(entry_kind, function_qualname, code_size, machine_block_count)` tuple
matches the baseline exactly. Comparable one-worker-per-benchmark aggregate
generated code remains **2,055 optimized typed blocks**, **167 functions**,
**1,567,940 native bytes**, and **103,818 machine blocks**. Those per-worker
smoke aggregates must not be compared directly with the ten-worker ordinary
baseline totals in the guardrail table.

Prior VM scheduling and shared-mount contention created major benchmark
outliers. Compare distributions, robust medians, explicit worker timing, and
structured commit events; do not confuse reduced JSONL serialization or cold
setup with an improvement in normally sampled Python throughput.

## Attempt history

### Attempt 1: Serialize each JSONL record before its append

- Change: replace fragmented unbuffered `serde_json::to_writer` plus a
  separate newline write with one complete serialized JSONL buffer and one
  logical `write_all`, preserving file-append and immediate-visibility
  behavior.
- Measurements and coverage: previous worker artifacts prove summary writes
  consumed **21.382788 / 22.412076 seconds (95.40%)** of one `deltablue`
  commit stream, with analogous **94–96%** summary overhead for `chaos` and
  `richards`. The matched candidate smoke reduces summary time by
  **58.60–268.67x** and apply setup by **3.91–39.11x**, while preserving
  every artifact row and emitted-code tuple. Normal sampling confirms
  **31.84–61.43x** lower per-record summary time, **192.73 to 71.50
  seconds** end-to-end, and **92.3 to 33.4 seconds** aggregate apply setup.
  Generated IR/native code and all JSONL tuples are identical. The optional
  detailed basic-block-map profile, previously timing out after more than
  120 seconds, now completes in **9.76 seconds** with **1,234 samples**, a
  valid **2.9 MB** map, and zero lost samples. The full gate passes
  **1,209 Python cases / 73 batches** and all Rust crate suites.
- Compatibility and tests: a structured counting-writer test verifies one
  complete write, valid JSONL, Unicode/nested values, and consecutive
  records. The genuine baseline RED fails after **68 underlying writes**
  for one record, with **543 other tests filtered**. The candidate GREEN
  passes **one test / 543 filtered**, proving one write per complete record
  and two consecutive valid nested/Unicode JSONL lines. Warning-free
  `cargo check -p soac_jit --tests` passes in **3.65 seconds**, and scoped
  package formatting / formatting checks pass.
- Result: retained after the complete correctness gate. This is an
  observability/startup workflow optimization with confirmed
  artifact-I/O/setup improvement; it does not itself establish faster
  generated Python code.

## Verdict and next action

- Verdict: **LANDED / RETAIN** as a general artifact-writing and developer
  workflow improvement. A genuine structured RED-to-GREEN proves **68 writes
  become one** per nested record with valid consecutive JSONL output. A
  matched smoke and normal-sampling comparison verify much lower worker setup
  and summary-write time, identical artifact records, and unchanged generated
  code. The representative workflow is **2.70x faster**, and default
  detailed-map native profiling now completes in **9.76 seconds** instead of
  timing out. The full correctness gate passes. This is not a claimed
  steady-state Python optimization: the paired-stock geometric ratio remains
  **0.3693040x**, and the full-suite **1.10x** objective is outstanding.
- Transferable lesson: when VM-mounted compiler artifacts dominate commit
  events, first distinguish serialization write-call patterns from actual
  definition/finalization and from measured benchmark execution.
- Next action: retain the validated artifact-writing, benchmark-workflow, and
  native-profiling improvements; use the faster optimization loop to
  investigate Python throughput strategies toward the full-suite **1.10x**
  objective.
