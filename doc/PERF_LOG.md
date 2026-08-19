---
title: "Codex Optimization Log"
---

2026-02-04: First run of transformed interpreter path

transformed interpreter
15880011868 3148 loops/s
stock cpython
967408991 1033688 loops/s
transform-only
1001108600 998892 loops/s

= 328x slower

2026-03-05: Full JIT

- Warmed in-process comparison:
  - JIT: work/logs/benchmark_jit_warm.log -> 5238 loops/s
  - Stock: work/logs/benchmark_stock_warm.log -> 824962 loops/s
= 157x slower

Vectorcall entry:

  Relative performance:

  - Stock is 95.71x faster than current JIT path
  - JIT is 1.045% of stock throughput

Use C API for operators:

  Relative:

  - JIT is 1.940% of stock throughput
  - Stock is 51.54x faster on this pystone run

2026-03-08: Remove tuple state passing between blocks:

  - JIT transformed: 23684 loops/s
  - Stock CPython: 913322 loops/s

  Relative:

  - stock is 38.56x faster
  - JIT is 2.59% of stock throughput

2026-03-25:  whole lot of cleanup, no perf work

• Current cold comparison from work/logs/benchmark-pystone-compare-20260325.log:

  - jit transformed: 30,536 loops/s
  - stock cpython: 906,698 loops/s

  Relative:

  - Stock is 29.69x faster
  - JIT transformed is 3.37% of stock throughput


2026-04-03:

changes:
  - refcounting as cranelift functions, constant pool for all strings
    - JIT/transformed: 105,083 loops/s
    - Stock CPython: 830,761 loops/s
    - transformed is about 0.126x stock, so stock is 7.9x faster.

  - 40e43654 Use Cranelift speed opt level and native ISA for JIT benchmarks
      - transformed/JIT: 91,257 loops/s
      - stock CPython: 754,886 loops/s
      - transformed is 0.121x stock, so stock is about 8.27x faster
      - timing: real 10.75, user 14.99, sys 0.83
      - log: work/logs/benchmark_opt_native_20260403.log
  - 404cbee4 Inline runtime CLIF support helpers into JIT callers
      - transformed/JIT: 119,398 loops/s
      - stock CPython: 739,834 loops/s
      - transformed is 0.161x stock, so stock is about 6.20x faster
      - timing: real 9.54, user 14.81, sys 0.76
      - log: work/logs/benchmark_opt_native_inlining_20260403.log
  - lift runtime functions to constants, immortal constants
      - transformed/JIT: 175,380 loops/s
      - stock CPython: 759,045 loops/s
      - transformed is about 0.231x stock
      - stock is about 4.33x faster
  - write through globals cache
      - transformed/JIT: 177,856 loops/s
      - stock CPython: 745,030 loops/s
      Relative performance:  transformed is about 0.239x stock stock is about 4.
  - really use vectorcall
      - transformed/JIT: 221,433 loops/s
      - stock CPython: 892,476 loops/s
      - transformed is about 0.248x stock, so stock is about 4.03x faster
  - fix read through globals cache, near 100% hitrate
      - JIT transformed: 245,183 loops/s
      - Stock CPython: 954,347 loops/s
      - So without counters enabled, the JIT is about 25.7% of stock throughput, or 3.89x slower.

2026-04-06: better specialization coverage, and constant string interning:
  - transformed/JIT profile pass: 221,542 loops/s
  - transformed/JIT specialized pass: 294,980 loops/s
  - stock CPython: 864,134 loops/s

  Headline comparison:

  - specialized transformed is about 0.341x stock
  - stock CPython is about 2.93x faster

2026-04-07: new baseline on VM
  profile pass:      118,016 loops/s
  specialized pass:  201,970 loops/s
  stock CPython:     620,991 loops/s
  hot callsites:     22

  Specialized baseline is ~32.5% of stock.

# Codex Optimization Log

Chronological log of finalized performance changes made by Codex agents.
Keep new entries succinct: what changed, which jj change id carried it, the
benchmarked throughput delta, and the headline pre/post numbers. Historical
entries may include abandoned experiments; record new positive, negative,
and inconclusive strategy history in `doc/optimization-attempts/` instead.

## 2026-07-29 - Generalize closed iterator-pipeline fusion

- jj change id: `wszrsrsw`
- summary: exact `list` and `tuple` sinks can now fuse single-use generator
  expressions through canonical single-input `map` and `filter` stages while
  preserving ordered materialization and callback exception semantics.
- throughput: `+1.54%` specialized pystone median versus the parent N-Queens
  change; the exact opaque N-Queens count path runs at `28530.838 loops/s`
  versus stock CPython's `8.457 loops/s`. The generalized ordered-list path is
  correct but remains below parity at `0.779 loops/s` versus stock's
  `7.885 loops/s`.
- pre-change benchmark: `work/bench/ruqwzllknkow`
  - apply, refcounts enabled, median: `263995 loops/s`
- post-change benchmark: `work/bench/wszrsrswkttr`
  - apply, refcounts enabled, median: `268052 loops/s`
  - no-refcount diagnostic median: `282615 loops/s`
  - total pystone code size: `206816 bytes`, `12251` machine blocks

## 2026-05-12 - Inline builtin list consumers for generators

- jj change id: `qtrtxpvp`
- summary: proven `list(generator)` calls now select the visible
  `soac.runtime.list_from_iter` helper, inline through `iter` / `next`, and keep
  trusted `resume_generator` owner arguments from falsely escaping the wrapper,
  so the generator path can lower fully into caller locals.
- throughput: `-0.25%` specialized pystone median versus the parent revision;
  total pystone JIT code size unchanged
- pre-change benchmark: `work/bench/stxxlynyvxwn_6d6da4130984`
  - apply, refcounts enabled, median: `594817 loops/s`
  - no-refcount diagnostic median: `719187 loops/s`
  - total pystone code size: `223674 bytes`, `11837` machine blocks
- post-change benchmark: `work/bench/qtrtxpvpwsrt_0b53007fa406`
  - apply, refcounts enabled, median: `593351 loops/s`
  - no-refcount diagnostic median: `732623 loops/s`
  - total pystone code size: `223674 bytes`, `11837` machine blocks

## 2026-05-01 - Land nqueens generator instantiation series

- jj change ids: `mkrvspvk`, `xxpnqpvv`, `yuonmzvz`, `xnpxqulv`, `wlpwtzzw`
- summary: The nqueens pyperformance work remaps exact-int plans across inlined
  direct calls, caches eager JIT attachment for nested functions, caches
  function instantiation templates, avoids capture dict construction for the
  common tuple-capture path, and leaves original named generators on CPython
  vectorcall.
- runtime: `7.91 s` to `104 ms` in the measured apply-mode nqueens smoke run,
  about `75.8x` faster than the starting SOAC result and about `1.11x` slower
  than stock CPython's `94.2 ms` result
- measured progression:
  - stock CPython: `nqueens-stock.json`, `94.2 ms`
  - starting SOAC apply result: `nqueens-soac-fixed.json`, `7.91 s`
  - nested-function/template cache: `nqueens-soac-template.json`, `3.63 s`
  - capture-tuple fast path: `nqueens-soac-capture-fast-apply2.json`, `2.73 s`
  - final named-generator fallback: `nqueens-soac-cpython-named-generators.json`,
    `104 ms`
- notes: The final kept result is the named-generator-only CPython vectorcall
  fallback. Broad CPython vectorcall for genexprs reached stock-like timings in
  one-off runs, but was narrowed after it broke
  `tests/test_regression_genexpr_iterator_semantics.py::test_genexpr_requires_iterator`.
  Other side experiments, including eager module direct-entry compilation,
  skipping redundant code-metadata replacement, and trusting generated capture
  order, either regressed or did not beat the kept capture/template path.

## 2026-04-27 - Land Cranelift/refcount/runtime-symbol cleanup bundle

- jj change ids: `ylklnnkt`, `owkmsrwk`, `snnysky`, `noptkkun`, `kxsstyll`
- summary: This main-bound bundle covers immortal checked-load INCREF elision,
  the normal runtime/benchmark `speed_and_size` Cranelift default,
  module-constant global-name DECREF removal, direct CPython C-API JIT symbol
  resolution, and immortal materialized-`None` results.
- throughput: `+8.01%` median in the recent compare run
- pre-change benchmark: `work/bench/rnykzppzqqur_53c05fea3df4`
  - apply, refcounts enabled, median: `247551 loops/s`
  - total pystone code size: `55082 bytes`, `3335` machine blocks
  - core pystone code size: `50862 bytes`
- post-change benchmark: `work/bench/snvutwoqpspo_0a629666c917`
  - apply, refcounts enabled, median: `267381 loops/s`
  - total pystone code size: `54919 bytes`, `3272` machine blocks
  - core pystone code size: `50691 bytes`
- notes: The post result is a benchmark child for `kxsstyll`. The net code-size
  change is small, but it removes `63` pystone machine blocks while improving
  the production refcount-enabled median.

## 2026-04-27 - Land constructor and method direct-call stack

- jj change ids: `nqsvxtt`, `onuyyrr`, `lsuosns`, `otmuxol`, `vvkssuq`,
  `rqqupmks`
- summary: Constructor and method calls are now represented as v3 direct-call
  targets, with class metadata carrying synthetic constructor entries. The final
  constructor metadata shape filters unsupported classes at type-registration
  time so the callsite does not pay the safety checks on every call.
- throughput: `+5.59%` median versus the `kxsstyll` benchmark child; `+14.05%`
  versus the earlier `rnykzppz` baseline used in the recent three-way compare
- pre-change benchmark: `work/bench/snvutwoqpspo_0a629666c917`
  - apply, refcounts enabled, median: `267381 loops/s`
  - total pystone code size: `54919 bytes`, `3272` machine blocks
  - direct-call counter hits: `1515002`
- post-change benchmark: `work/bench/wntrurtxnxqq_d133b77d9209`
  - apply, refcounts enabled, median: `282323 loops/s`
  - total pystone code size: `57258 bytes`, `3394` machine blocks
  - direct-call counter hits: `1717002`
- notes: The post benchmark artifact is the recent constructor-metadata
  benchmark child used to validate the stack that was later advanced to
  `rqqupmks` on `main`. The direct-call counter increase is the clearest
  structured signal for the pystone win; generated pystone code grew by
  `2339 bytes`.

## 2026-04-21 - Fix v3 indexed-field receiver lowering

- jj change id: `ltxokuuw`
- summary: `GetAttrTyped` statement results now consume v3 emitted indexed-field
  load decisions directly, so loads used as local-store RHS values take the
  same profiled indexed fast path as expression-position loads.
- throughput: `+1.61%` median versus the previous mechanical v3 indexed-field
  change
- pre-change benchmark: apply, refcounts enabled, 1M loops x3 median
  `239058 loops/s`; core pystone code size `50323 bytes`
- post-change benchmark: apply, refcounts enabled, 1M loops x3 median
  `242905 loops/s`; core pystone code size `50667 bytes`
- indexed-field counters: `2525030` to `2929030` hits and `1010107` to
  `606107` fallbacks in a 100k-loop verify run
- result directory: `work/bench/ltxokuuwvptz`

## 2026-04-21 - Route indexed fields through optimizer v3

- jj change ids: `mnltztnt`, `ntmvkovm`
- summary: Indexed-field selections are now planned from v3 profile evidence
  and consumed as explicit v3 codegen inputs. The JIT preserves the emitted
  access kind and attribute name instead of converting v3 decisions into legacy
  per-instruction field evidence, making typed attribute annotation validate the
  selected plan shape mechanically.
- throughput: `+7.45%` median
- pre-change benchmark: apply, refcounts enabled, 1M loops x3 median
  `222490 loops/s`; core pystone code size `47394 bytes`
- post-change benchmark: apply, refcounts enabled, 1M loops x3 median
  `239058 loops/s`; core pystone code size `50323 bytes`
- indexed-field counters: `0` to `2525030` hits and `3535137` to `1010107`
  fallbacks in a 100k-loop verify run
- note: the final mechanical-consumption cleanup intentionally rejects four
  legacy-permissive indexed-field hits that were present in the immediate
  predecessor; compared with the true pre-indexed-field baseline, indexed-field
  hits still increase substantially.

## 2026-04-17 - Emit effect-only typed setattr without None materialization

- jj change id: `nlzrurom`
- summary: Typed `SetAttr` statement emission now consumes effect-only demand at
  the indexed-field producer, so fast store hits return `NoValue` instead of
  materializing and increfing an owned `None`.
- throughput: `+1.47%` median
- pre-change benchmark: apply, refcounts enabled, 1M loops x3 median
  `255234 loops/s`; total pystone code size `396565 bytes`
- post-change benchmark: apply, refcounts enabled, 1M loops x3 median
  `258974 loops/s`; total pystone code size `392208 bytes`
- refcount counters: unchanged in a 100k-loop verify run

## 2026-04-17 - Use side-effect result facts for statement discards

- jj change id: `nlmuuwvl`
- summary: Effect-only statement result discard now consumes codegen result
  facts, so legacy-shaped side-effect producers that are known to return
  immortal `None` skip unnecessary discard refcount code.
- throughput: `+0.69%` median
- pre-change benchmark:
  - apply, refcounts enabled, 1M loops x3: `186721`, `253477`,
    `255209 loops/s`
  - apply, refcounts disabled, 1M loops x3: `333205`, `327666`,
    `304328 loops/s`
  - total pystone code size: `400294 bytes`
- post-change benchmark:
  - apply, refcounts enabled, 1M loops x3: `255234`, `257154`,
    `253164 loops/s`
  - apply, refcounts disabled, 1M loops x3: `344973`, `342741`,
    `328802 loops/s`
  - total pystone code size: `396565 bytes`
- refcount counters: unchanged in a 100k-loop verify run

## 2026-04-17 - Preserve typed direct-call and module-constant ownership

- jj change id: `vwopwllm`
- summary: Guarded direct-call and constructor specialization now keep callable
  and positional operands in typed form through guard, direct, and cold fallback
  emission. Module-constant PyObject facts are also marked immortal, matching
  runtime materialization, so borrowed/immortal ownership survives into generated
  code.
- throughput: `+3.29%` median with pinned benchmark
- pre-change benchmark:
  - specialized pass, 1M loops x3: `248185`, `261597`, `255189 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `262954`, `263587`, `265192 loops/s`
- refcount counters: unchanged in a 100k-loop verify run
- code size: `506671` to `485210 bytes` total pystone code size

## 2026-04-17 - Preserve typed attribute operand ownership

- jj change id: `snwwtlsy`
- summary: Plain positional generic typed calls now keep typed child operands
  through call emission, while typed attribute get/set fallback and indexed
  field paths release only operands materialized as owned temporaries. Typed
  attribute fallback results are checked before they can become call arguments.
- throughput: `+1.73%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `255841`, `253211`, `259156 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `260964`, `259035`, `260256 loops/s`
- refcount counters: `-101000` INCREF and `-101000` DECREF in a 100k-loop
  verify run
- code size: `506879` to `506671 bytes` total pystone code size

## 2026-04-16 - Consume planned typed call input ownership

- jj change id: `ntomvpmx`
- summary: Typed direct-call codegen now keeps the callable, receiver, and
  argument inputs as typed instructions until emission, so planned
  borrowed/immortal PyObject input ownership is consumed directly instead of
  being dropped during legacy expression lowering.
- throughput: `+0.77%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `253509`, `254773`, `258652 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `256637`, `256729`, `260574 loops/s`
- code size: unchanged at `506663 bytes` total pystone code size

# Journal of Negative Results

## 2026-04-17 - Not landed: exact-int branch comparison i32 result

- jj change id: not landed (`poorvsyp` while testing)
- summary: Tried satisfying `I32Bool01` branch demand directly from profiled
  exact-int comparison specialization. The first version deopted on
  non-compact exact ints and exposed incorrect branch-term deopt state for long
  pystone timing values; the corrected version used a cold local PyLong
  richcompare fallback. Refcount counters were unchanged, code size grew, and
  throughput regressed, so this should wait for a cleaner typed operation result
  ABI instead of adding another codegen-side special case.
- throughput: `-4.58%` median with refcounts enabled
- baseline benchmark:
  - apply, refcounts enabled, 1M loops x3 median: `261529 loops/s`
  - apply, refcounts disabled, 1M loops x3 median: `344087 loops/s`
  - total pystone code size: `392208 bytes`
- attempted benchmark:
  - apply, refcounts enabled, 1M loops x3 median: `249551 loops/s`
  - apply, refcounts disabled, 1M loops x3 median: `333610 loops/s`
  - total pystone code size: `395802 bytes`

## 2026-04-17 - Not landed: borrowed indexed-field GetAttr results

- jj change id: not landed (`rpyppuvu` while testing)
- summary: Tried planning borrowed-ok profiled indexed `GetAttr` results as
  borrowed-field candidates and skipping the direct-hit `INCREF` only when the
  receiver was already live and guard misses deopted instead of merging with an
  owned fallback. Pystone field indexed hit/fallback counts and runtime
  INCREF/DECREF counters were unchanged, so the added ownership shape did not
  reach the hot refcount path in this benchmark. Throughput regressed slightly
  and code size increased.
- throughput: `-0.66%` median with refcounts enabled
- baseline benchmark:
  - apply, refcounts enabled, 1M loops x3 median: `258974 loops/s`
  - apply, refcounts disabled, 1M loops x3 median: `346176 loops/s`
  - total pystone code size: `392208 bytes`
- attempted benchmark:
  - apply, refcounts enabled, 1M loops x3 median: `257265 loops/s`
  - apply, refcounts disabled, 1M loops x3 median: `338445 loops/s`
  - total pystone code size: `392432 bytes`

## 2026-04-17 - Not landed: typed item intrinsic and effect-only setitem emission

- jj change id: not landed (`tmxwusyv` while testing)
- summary: Tried routing typed getitem/setitem/delitem through typed intrinsic
  emission so planned operand ownership survived item specialization. A follow-up
  effect-only setitem path skipped owned `None` materialization on exact-list
  specialized store hits. Code size improved, but verify counters, including
  runtime INCREF/DECREF totals, were unchanged and pystone throughput was flat to
  slightly negative. The extra codegen complexity is not justified without a
  clearer counter or throughput win.
- throughput: `-0.31%` median with refcounts enabled
- baseline benchmark:
  - apply, refcounts enabled, 1M loops x3: `262821`, `264473`,
    `264513 loops/s`
  - apply, refcounts disabled, 1M loops x3: `351698`, `356883`,
    `359373 loops/s`
  - total pystone code size: `485210 bytes`
- attempted benchmark:
  - apply, refcounts enabled, 1M loops x3: `239322`, `263655`,
    `265600 loops/s`
  - apply, refcounts disabled, 1M loops x3: `334710`, `353713`,
    `341393 loops/s`
  - total pystone code size: `483874 bytes`

## 2026-04-16 - Not landed: typed generic positional call emission

- jj change id: not landed (`tmvssnxt` while testing)
- summary: Tried routing plain positional generic typed calls through typed
  child emission so planned borrowed/immortal input ownership would survive the
  call-emission boundary. The real refcount-enabled path regressed even after
  preserving the fixed-arity effect-only helper shape. Verify counters showed
  the attempt added `+505103` runtime INCREFs and `+505103` runtime DECREFs per
  100k-loop pystone verify run, mostly in `Proc1`, so this should wait for a
  more precise typed call emitter that does not increase ownership traffic.
- throughput: `-2.23%` median with refcounts enabled
- baseline benchmark:
  - apply, refcounts enabled, 1M loops x3: `247509`, `253836`,
    `259163 loops/s`
  - apply, refcounts disabled, 1M loops x3: `324245`, `328681`,
    `324769 loops/s`
  - total pystone code size: `506663 bytes`
- attempted benchmark:
  - apply, refcounts enabled, 1M loops x3: `248181`, `246173`,
    `260493 loops/s`
  - apply, refcounts disabled, 1M loops x3: `352532`, `353217`,
    `340215 loops/s`
  - total pystone code size: `509615 bytes`

## 2026-04-15 - Not landed: Cranelift opt-level tuning for typing import

- jj change id: not landed
- summary: Timed the slow broad-import `typing` case in `SOAC_OPT_MODE=none`
  while varying `SOAC_CRANELIFT_OPT_LEVEL`. The run imports stdlib `typing`
  through the SOAC import hook in a fresh process. Backend optimization level
  was not the dominant cost: all valid settings stayed in the same roughly
  92-95 second range, and the observed variance was larger than the opt-level
  effect. Plain `size` is not a valid Cranelift opt-level value in current
  config.
- direct fresh-process typing import:
  - `SOAC_CRANELIFT_OPT_LEVEL=none`: `93.07s`
  - `SOAC_CRANELIFT_OPT_LEVEL=speed`: `94.82s`
  - `SOAC_CRANELIFT_OPT_LEVEL=speed_and_size`: `92.44s`
- pytest wrapper check, warmed setup, `SOAC_CRANELIFT_OPT_LEVEL=none`:
  `91.30s` wall / `89.99s` pytest call and `90.75s` wall / `89.46s`
  pytest call

## 2026-04-14 - Not landed: exact narrow scalar counters with cold overflow

- jj change id: not landed (`pwvwzkwq` while testing)
- summary: Tried shrinking hot scalar counter slots from `u64` to `u16`
  while preserving exact dump totals through a cold overflow helper and
  side `u64` overflow array. Counter dumps and specialization summaries
  were byte-for-byte identical to baseline, and JIT code size did not
  change. The smaller hot slots did not offset the added wrap check and
  overflow plumbing, especially in countered modes.
- profile throughput: `-5.48%`
- verify throughput: `-9.15%`
- apply throughput: `+0.78%` median with refcounts enabled, likely noise
- baseline benchmark:
  - profile: `234169 loops/s`
  - verify: `197711 loops/s`
  - apply, refcounts enabled, 1M loops x3: `438585`, `442082`,
    `437448 loops/s`
- attempted benchmark:
  - profile: `221332 loops/s`
  - verify: `179615 loops/s`
  - apply, refcounts enabled, 1M loops x3: `442001`, `440594`,
    `444028 loops/s`

## 2026-04-14 - Not landed: u32 scalar counters with cold overflow

- jj change id: not landed (`pwvwzkwq` while testing)
- summary: Retried the narrow-counter experiment with `u32` hot slots to
  avoid practical overflow traffic while halving the hot counter array
  footprint relative to `u64`. Counter dumps, verify dumps, and
  specialization summaries again matched baseline exactly, and JIT code
  size was unchanged. The profile and verify passes still regressed,
  indicating that the hot increment sequence and extra branch dominate
  over any cache-footprint benefit for pystone.
- profile throughput: `-3.51%`
- verify throughput: `-15.96%`
- apply throughput: `+1.58%` median with refcounts enabled, likely noise
- baseline benchmark:
  - profile: `234277 loops/s`
  - verify: `207845 loops/s`
  - apply, refcounts enabled, 1M loops x3: `438998`, `430012`,
    `432882 loops/s`
- attempted benchmark:
  - profile: `226048 loops/s`
  - verify: `174680 loops/s`
  - apply, refcounts enabled, 1M loops x3: `439727`, `439593`,
    `442380 loops/s`
- storage note: the on-disk `profile.bin` and `verify.bin` counter dumps
  stayed unchanged at `3,204,400 bytes` combined because dump rows store
  reported `u64` totals and metadata. The full benchmark `counters/`
  directory was about `17 MiB`, dominated by `jit-bb-map.jsonl` and
  `jit-*.dump` artifacts rather than scalar counter values.

# cpython

## 2026-04-14 - Temporary no-refcount CPython pystone experiment

- checkout: isolated in `vendor/cpython-norefcount`; original
  `vendor/cpython` was restored and left clean
- summary: Built an optimized/LTO vendored CPython variant with temporary
  `SOAC_REFCOUNT_NOOP` patches that make normal refcount inc/dec paths no-op.
  The experiment also needed no-refcount-only workarounds for ceval-local
  decref macros, dict-key lifetime, frame ownership transfer, cyclic GC
  collection, and a couple of sysconfig build-helper paths.
- status: benchmark-usable linked interpreter, but not a clean `build_all`.
  The remaining `build_all` failures are later helper crashes in
  `generate-build-details.py` / `checksharedmods`; the Python-aware backtrace
  for `generate-build-details.py` lands in frozen importlib while importing
  `importlib.machinery`.
- throughput: `+10.7%` pystone median using `python -E -S`
- baseline `vendor/cpython`, 1M loops x3:
  `583437`, `580613`, `578021 loops/s`
- no-refcount `vendor/cpython-norefcount`, 1M loops x3:
  `624041`, `642477`, `657865 loops/s`

## 2026-04-13 - Load module constants through per-slot symbols

- jj change id: `oxkwxwwy`
- summary: Module constant loads now reference one symbol per constant slot
  instead of loading from a shared constant-table symbol plus a byte offset.
  This gives object-file loading a constant-specific relocation target while
  preserving the same specialization set.
- throughput: `+0.42%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `422870`, `418198`, `423934 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `425631`, `417725`, `424651 loops/s`

## 2026-04-13 - Remove no-op recursive leave calls

- jj change id: `kyxysmzx`
- summary: Direct-call and vectorcall-trampoline codegen no longer emits
  `dp_jit_leave_recursive_call` after the direct callee returns or after
  argument binding fails. In this vendored CPython, `Py_LeaveRecursiveCall`
  wraps a no-op; the direct-call path keeps the `Py_EnterRecursiveCall`
  C-stack guard and removes only the unpaired no-op leave call overhead.
- throughput: `+2.50%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `408266`, `418252`, `416438 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `421926`, `430101`, `426855 loops/s`

## 2026-04-13 - Not landed: thread owner type into indexed-field helpers

- jj change id: not landed
- summary: Tried passing the already-guarded exact owner type into
  `soac_runtime_probe_field_indexed` / `soac_runtime_store_field_indexed`
  so the helpers could skip reloading and re-deriving object type
  information on successful field get/store paths. The helper ABI change
  slightly reduced generated code size but did not improve specialized
  pystone throughput, so it was reverted.
- throughput: `-0.14%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `408266`, `418252`, `416438 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `421436`, `409964`, `415850 loops/s`

## 2026-04-13 - Recover direct Record constructor specialization

- jj change id: `rmsmspso`
- summary: Owner-type registration now also scans SOAC indexed module
  globals by direct indexed-dict slot lookup, so classes stored outside
  the plain module dict can register their `__init__` owner types without
  invoking module `__getattr__`. The specialized `Record.copy`
  constructor call in pystone now emits the direct
  `dp_jit_pytype_generic_alloc` / constructor-init path instead of always
  routing through `_PyObject_MakeTpCall`.
- throughput: `+27.31%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 100k loops x1: `300817 loops/s`
- post-change benchmark:
  - specialized pass, 100k loops x1: `382976 loops/s`

## 2026-04-12 - Deduplicate identical local failure cleanup blocks

- jj change id: `spxzrtuu`
- summary: Local failure cleanup lowering now reuses pending cleanup blocks
  when the concrete cleanup value list, forwarded value list, and continuation
  block are identical. An earlier arity-only sharing attempt reduced code size
  more aggressively but corrupted async cancellation cleanup, so the landed
  form keeps sharing limited to identical SSA cleanup inputs.
- throughput: `+3.68%` specialized pystone median; total pystone code size
  `-6.97%`; total pystone machine blocks `-6.06%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `275808`, `280431`, `273441 loops/s`
  - verify pass: `156370 loops/s`
  - total code size: `370500` bytes
  - total machine blocks: `22652`
- post-change benchmark:
  - specialized pass, 1M loops x3: `285947`, `279119`, `286187 loops/s`
  - verify pass: `155688 loops/s`
  - total code size: `344678` bytes
  - total machine blocks: `21280`

## 2026-04-12 - Use fixed-arity fallback calls for small guarded calls

- jj change id: `xqqowksq`
- summary: Guarded direct-call, direct-method, and constructor miss
  blocks now use the existing fixed-arity positional helper for fallback
  calls with at most three positional args. This avoids per-callsite
  vectorcall stack-slot setup on those cold miss paths while leaving the
  unspecialized generic positional-call path unchanged.
- throughput: `-0.35%` specialized pystone median in the finalized
  run; total pystone code size `-0.43%`; total pystone machine blocks
  `-0.17%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `281216`, `273692`, `276780 loops/s`
  - verify pass: `158201 loops/s`
  - total code size: `372092` bytes
  - total machine blocks: `22691`
- post-change benchmark:
  - specialized pass, 1M loops x3: `275808`, `280431`, `273441 loops/s`
  - verify pass: `156370 loops/s`
  - total code size: `370500` bytes
  - total machine blocks: `22652`

## 2026-04-12 - Share direct cleanup final return block

- jj change id: `lqtorwoq`
- summary: Direct-function lowering now shares null-cleanup blocks for
  source blocks that do not carry an active exception parameter. Blocks
  that need handled-exception state popping keep per-block cleanup. The
  shared and per-exception cleanup blocks are marked cold, and the
  benchmark summarizer now reports machine basic-block counts alongside
  code-size bytes.
- throughput: `+0.92%` specialized pystone median; total pystone code
  size `-7.66%`; total pystone machine blocks `-8.66%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `272054`, `276700`, `271604 loops/s`
  - verify pass: `168293 loops/s`
  - total code size: `402947` bytes
  - total machine blocks: `24842`
- post-change benchmark:
  - specialized pass, 1M loops x3: `274562`, `273407`, `281089 loops/s`
  - verify pass: `168352 loops/s`
  - total code size: `372092` bytes
  - total machine blocks: `22691`

## 2026-04-12 - Route direct-call null failures through the current step-null path

- jj change id: `quymqrww`
- summary: Direct-call null failures no longer build a local block that
  reloads and re-sets the current exception before branching to the
  active step-null continuation. The change also adds apply-mode
  regression coverage for direct-call and constructor exception
  propagation.
- throughput: `+1.06%` specialized pystone median after rebasing onto
  `sypsopvxttmk`; verify improved `+4.05%` and total pystone code size
  shrank by `1072` bytes
- pre-change benchmark:
  - specialized pass, 1M loops x3: `276181`, `278949`, `272400 loops/s`
  - verify pass: `160199 loops/s`
  - total code size: `404019` bytes
- post-change benchmark:
  - specialized pass, 1M loops x3: `279120`, `279927`, `274180 loops/s`
  - verify pass: `166693 loops/s`
  - total code size: `402947` bytes

## 2026-04-12 - Elide explicit error save/restore around decref cleanup

- jj change id: `omymzyom`
- summary: Removed the explicit current-exception save/restore sequence around
  owned-temp decref cleanup and routed those cleanup sites through a shared
  helper instead, relying on the runtime decref path to preserve the active
  Python exception.
- throughput: `-2.72%` specialized pystone median relative to `lrktzrpv`,
  with essentially unchanged verify throughput and a substantially smaller
  pystone JIT image
- pre-change benchmark:
  - specialized pass, 1M loops x3: `281062`, `281854`, `274855 loops/s`
  - verify pass: `161242 loops/s`
  - total code size: `447630` bytes
- post-change benchmark:
  - specialized pass, 1M loops x3: `282617`, `273427`, `271740 loops/s`
  - verify pass: `161285 loops/s`
  - total code size: `405732` bytes

## 2026-04-12 - Remove apply-mode specialization counters

- jj change id: `olrnwpvz`
- summary: Apply mode no longer lowers specialization profiling counters,
  no longer emits `dp_jit_record_top_value_sample`, and no longer logs
  specialization-runtime counter rows just because `SOAC_WORK_DIR` or
  `SOAC_LOG` is set. Profile and verify still record the same
  specialization set; only steady-state apply overhead changed.
- throughput: `+22.74%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `184066`, `187351`, `182357 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `221766`, `225928`, `228347 loops/s`

## 2026-04-11 - Disable profiled cold-block hints by default

- jj change id: `lwnxlmrx`
- summary: Kept the `block_entry` profiling pipeline and the apply/verify
  cold-block replay path, but gated the replay behind
  `SOAC_ENABLE_PROFILED_COLD_BLOCKS=1` so normal runs keep recording the
  counters without changing code layout.
- throughput: default path now matches the pre-replay baseline; the prior
  threshold experiments for the opt-in replay path stayed neutral to
  slightly negative (`166579` baseline median, `166428` at 50%, `162351`
  at 80%), so the hint now ships opt-in only

## 2026-04-11 - Mark rarely visited profiled blocks cold

- jj change id: `oxrtzwlp`
- summary: Replayed `block_entry` counters during apply/verify JIT
  lowering and marked non-entry blocks visited at most 1% as often as
  the function entry block as Cranelift `cold` blocks. This is a layout
  hint only; the short pystone validation run showed no code-size
  counter change.
- throughput: `-0.38%` median specialized pystone in a short
  100-loop validation run; treated as noise-level / neutral
- pre-change benchmark:
  - specialized pass, 100 loops x3: `158967`, `157293`, `155422 loops/s`
  - machine code size total/max: `1018394` / `120176` bytes
- post-change benchmark:
  - specialized pass, 100 loops x3: `156690`, `158210`, `153109 loops/s`
  - machine code size total/max: `1018394` / `120176` bytes

## 2026-04-08 - Inline runtime guard and indexed-field helpers

- jj change id: `kkoolpkp`
- summary: Type/version guards now inline through soac_jit_runtime, indexed
  field helpers use direct dict/inline-values access instead of
  `_PyObject_GetDictPtr`, and the opt-in unsound indexed field-store path
  reports hit/miss status instead of returning an owned temporary.
- throughput: `+4.01%` 100k default-specialized pystone; `+12.03%`
  100k opt-in unsound indexed-store pystone
- pre-change benchmark:
  - default specialized: `154514 loops/s`
  - opt-in unsound indexed stores: `142627 loops/s`
- post-change benchmark:
  - default specialized: `160710 loops/s`
  - opt-in unsound indexed stores: `159810 loops/s`
  - same-run stock CPython: about `555k loops/s`

## 2026-04-08 - Call PyLong slots directly for exact-int specialization

- jj change id: `tuyrzlpu`
- summary: Exact-`int` binary operator specialization now emits imports
  for the profiled `PyLong_Type` number slots and rich-compare slot
  instead of calling the generic Rust `dp_jit_exact_long_binary_op`
  dispatch helper. The runtime JIT symbol table binds those imports to
  CPython's `PyLong_Type.tp_as_number` / `tp_richcompare` function
  pointers at registration time.
- throughput: `+3.39%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `154501`, `155750`, `141058 loops/s`
  - stock CPython: `541272 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `158845`, `159744`, `160164 loops/s`
  - stock CPython: `550585 loops/s`

## 2026-04-08 - Reuse the direct-call entry pointer load

- jj change id: `tuyrzlpu`
- summary: Direct-call codegen now carries `FunctionEnv.direct_code_ptr`
  out of the metadata / lazy-compile check and reuses it for
  `call_indirect`, removing the duplicate direct-code-pointer load and
  null check from the fast path.
- throughput: `+1.48%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `158845`, `159744`, `160164 loops/s`
  - stock CPython: `550585 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `156111`, `162114`, `163148 loops/s`
  - stock CPython: `551829 loops/s`

## 2026-04-08 - Inline next-or-sentinel iterator progress

- jj change id: `tuyrzlpu`
- summary: Codegen now recognizes transformed iterator-progress helper
  calls and emits a native iterator helper call instead of vectorcalling
  the transformed Python runtime helper. Exhausted iterators return a
  module sentinel, and real iterator errors stay on the existing
  null-return exception path.
- throughput: `+8.06%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `156111`, `162114`, `163148 loops/s`
  - stock CPython: `551829 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `175180`, `159439`, `178348 loops/s`
  - stock CPython: `568659 loops/s`

## 2026-04-08 - Avoid KeyError allocation in global-load fallback

- jj change id: `lzqutouv`
- summary: The JIT runtime global-load fallback now probes exact dict
  globals and dict builtins with `PyDict_GetItemRef`, preserving the
  owned-reference contract without first calling mapping subscript,
  constructing `KeyError`, clearing it, and then looking in builtins.
- throughput: `+15.81%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `214770`, `217031`, `223132 loops/s`
  - stock CPython: `559279 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `252020`, `251340`, `248906 loops/s`
  - stock CPython: `552461 loops/s`

## 2026-04-08 - Fast-path exact list item helpers

- jj change id: `spsxlton`
- summary: The existing JIT getitem/setitem helpers now handle exact
  `list` with exact compact-`int` index directly: decode compact long
  indexes in Rust, normalize in-range negative indices, use direct
  `PyList_GET_ITEM` / `PyList_SET_ITEM` access, and fall back to the
  generic item protocol for mismatched, big-int, or out-of-range cases.
- throughput: `+3.12%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `308879`, `313589`, `309820 loops/s`
  - stock CPython: `551629 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `319499`, `320812`, `308803 loops/s`
  - stock CPython: `558357 loops/s`

## 2026-04-08 - Not landed: post-list-fastpath micro-optimizations

- jj change id: not landed
- summary: Tried several follow-up pystone optimizations after the
  exact-list helper change and reverted each candidate because it was
  benchmark-negative or too close to noise.
- attempts:
  - generated exact-list / compact-int getitem fast path in CLIF:
    median `319499 -> 314717 loops/s`, `-1.50%`
  - direct `ob_type` checks inside the exact-list helper: median
    `319499 -> 320632 loops/s`, `+0.35%`; treated as noise and not
    landed
  - generated singleton-truth fast path before `dp_jit_is_true`:
    median `319499 -> 307591 loops/s`, `-3.73%`
  - singleton fast path inside `dp_jit_is_true`: median
    `319499 -> 304941 loops/s`, `-4.56%`
  - branch-context richcompare-truth helper: median
    `319499 -> 306582 loops/s`, `-4.04%`

## 2026-04-09 - Profile conditional branch locality

- jj change id: `wvotvvly`
- summary: Profile/apply mode now records each conditional terminator's
  post-truthiness boolean as a `branch_outcomes` top-value counter, replays
  false-vs-true counts from `profile.bin`, and inverts false-hot specialized
  JIT branches so the hotter edge is the Cranelift true / first edge.
- throughput: `+0.15%` median versus the first parent run; repeat
  parent run was lower, so treat the measured change as benchmark noise
- pre-change benchmark:
  - specialized pass, 1M loops x3: `310384`, `315461`, `313523 loops/s`
  - repeat specialized pass, 1M loops x3: `299188`, `313244`,
    `308169 loops/s`
  - stock CPython: `550680 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `313980`, `319739`, `310154 loops/s`
  - stock CPython: `552602 loops/s`

## 2026-04-09 - Not landed: Cranelift Fast calling-convention experiments

- jj change id: not landed
- summary: Tried Cranelift `CallConv::Fast` on SOAC-internal ABIs. The
  direct-body variant changed the compiled transformed-Python body and
  matching indirect-call signatures. The runtime-helper variant changed only
  local `soac_runtime_*` CLIF helper definitions and matching local imports.
  Neither produced a benchmark-visible pystone win.
- attempts:
  - direct transformed-Python body ABI: median `313500 -> 308959 loops/s`,
    `-1.45%`
  - runtime CLIF helper ABI: median `313500 -> 312062 loops/s`, `-0.46%`
- baseline benchmark:
  - specialized pass, 1M loops x3: `313500`, `297235`, `317124 loops/s`
  - stock CPython: `544436 loops/s`
- direct-body Fast benchmark:
  - specialized pass, 1M loops x3: `308959`, `318225`, `291427 loops/s`
  - stock CPython: `523614 loops/s`
- runtime-helper Fast benchmark:
  - specialized pass, 1M loops x3: `316166`, `312062`, `309037 loops/s`
  - stock CPython: `550054 loops/s`

## 2026-04-09 - Apply-mode raw indexed stores

- jj change id: `qrutwqnr`
- summary: Apply mode now emits raw indexed stores for specialized
  module-global and split instance-field writes, bypassing CPython
  dict/object/type observer and insertion-order maintenance on guarded hits.
- throughput: `+10.29%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `250439`, `249170`, `247683 loops/s`
  - perf-context run: `247131 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `274809`, `279150`, `253522 loops/s`
  - perf-context run: `255713 loops/s`

## 2026-04-10 - Upgrade Cranelift to 0.130.1

- jj change id: `vyqwvlks`
- summary: Upgraded the Cranelift dependency family from `0.125` to
  `0.130.1`, aligned the direct `gimli` dependency with Cranelift's
  unwind types, and kept the regenerated snapshot formatting changes.
- throughput: `+1.94%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `273439`, `267436`, `275214 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `278749`, `281497`, `276426 loops/s`

## 2026-04-10 - Use process-JIT direct calls for SOAC function targets

- jj change id: `wwosynst`
- summary: Process-JIT batches now predeclare reachable SOAC functions and
  emit CLIF direct `call`s for supported direct edges. Unsupported edges use
  the generic Python call fallback, and warmed direct-context lookups avoid
  cloning the lowered `BlockPyFunction` after compilation.
- throughput: `+179.55%` median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `89790`, `86473`, `86037 loops/s`
  - perf-context run: `83929 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `247158`, `240873`, `241731 loops/s`
  - perf-context run: `227895 loops/s`

## 2026-04-10 - Remove direct-entry tracing from generated code

- jj change id: `oxqvnxtl`
- summary: Removed the generated direct-entry trace helper import and calls,
  the runtime symbol binding, and the helper that checked the process
  environment on every direct JIT entry.
- throughput: `+25.43%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `90588`, `89972`, `92372 loops/s`
  - perf-context run: `88371 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `114328`, `107795`, `113621 loops/s`
  - perf-context run: `95269 loops/s`

## 2026-04-10 - Enable direct field-index specialization

- jj change id: `vytxokyr`
- summary: Same-module direct-function compilation now receives module globals,
  so apply/verify mode can resolve profiled split-dict owner layouts and emit
  field-indexed instance load/store fast paths. Also removed the leftover
  `SOAC_BIND_TRACE` argument-binding debug path.
- throughput: `+6.20%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `85166`, `114419`, `115642 loops/s`
  - perf-context run: `107731 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `115465`, `123032`, `121511 loops/s`
  - perf-context run: `117328 loops/s`

## 2026-04-10 - Pass thread state to indexed field stores

- jj change id: `xuwyyrwr`
- summary: The indexed field-store runtime helper now receives the generated
  function's existing `PyThreadState` pointer and uses it when decrefing a
  replaced field value, avoiding one helper-local TLS lookup on successful raw
  field stores.
- throughput: `+3.58%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `115465`, `123032`, `121511 loops/s`
  - perf-context run: `117328 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `131076`, `121536`, `125865 loops/s`
  - perf-context run: `116351 loops/s`

## 2026-04-10 - Not landed: split exact-int truth helper

- jj change id: not landed (`voqqtors`)
- summary: Tried routing exact-`int` `not` / internal truth unary
  specialization through a new helper returning raw `nb_bool` as `i32`, then
  materializing `Py_True` / `Py_False` through the typed bool path. The
  specialization set did not change, and the removed object-returning exact-long
  unary helper path was too small in pystone to justify the extra split.
- throughput: `-1.01%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `126977`, `126471`, `123701 loops/s`
  - perf-context run: `125578 loops/s`
- attempted benchmark:
  - specialized pass, 1M loops x3: `124219`, `127264`, `125195 loops/s`
  - perf-context run: `120165 loops/s`

## 2026-04-11 - Thread tstate through hot JIT helpers

- jj change id: `sosmzxqw`
- summary: Threaded the existing `PyThreadState` parameter through the hot JIT
  helper paths, including keyword and unpacked-call helper lowering, so those
  paths stop doing helper-local thread-state/TLS lookups. The specialized and
  verify specialization sets stayed identical across the before/after runs.
- throughput: `+69.50%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `101666`, `107664`, `103292 loops/s`
  - perf-context run: `100400 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `175063`, `176168`, `175076 loops/s`
  - perf-context run: `172766 loops/s`

## 2026-04-12 - Honor effect-only demand for LocalEnv stores

- jj change id: `xykryxmq`
- summary: Statement-position LocalEnv store/delete producers now return
  `NoValue` when the result is not consumed, avoiding owned-`None`
  materialization for those producer paths. Against the verify-refcount-counter
  base, specialization sets and verify hit/fallback counters stayed unchanged.
- throughput: `+9.32%` specialized pystone median; code size `-2.90%`;
  applied refcount ops unchanged
- pre-change benchmark:
  - specialized pass, 1M loops x3: `170756`, `172461`, `166913 loops/s`
  - pystone JIT code bytes: `933661`
  - pystone verify refcount ops: `20956626`
- post-change benchmark:
  - specialized pass, 1M loops x3: `185930`, `189987`, `186673 loops/s`
  - pystone JIT code bytes: `906622`
  - pystone verify refcount ops: `20956626`

## 2026-04-12 - Reduce JIT LocalEnv stack mirrors

- jj change id: `mnplvqtw`
- summary: Direct-entry params and cleanup-only locals now travel through
  planned block params where possible, and the JIT allocates physical stack
  slots only for remaining stack-backed paths. This removes entry
  store/load roundtrips and avoids preserving stack mirrors only for
  representation compatibility.
- throughput: `+26.62%` specialized pystone median
- pre-change benchmark:
  - specialized pass, 1M loops x3: `221766`, `225928`, `228347 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `280740`, `287558`, `286068 loops/s`

## 2026-04-12 - Share local failure cleanup blocks by logical locals

- jj change id: `onzmnylr`
- summary: Cleanup-null failure paths now share cleanup blocks when they release
  the same ordered logical locals, while passing the current SSA values as
  block params. Generator/coroutine closure bindings use the same logical-local
  cleanup key as ordinary closure bindings.
- throughput: `-1.65%` specialized pystone median; code size `-9.79%`;
  machine blocks `-7.96%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `284414`, `291437`, `278717 loops/s`
  - pystone JIT code bytes: `344486`
  - pystone machine blocks: `21281`
- post-change benchmark:
  - specialized pass, 1M loops x3: `279717`, `277360`, `281715 loops/s`
  - pystone JIT code bytes: `310747`
  - pystone machine blocks: `19587`

## 2026-04-13 - Remove redundant refcount null guards

- jj change id: `rxmtwtlm`
- summary: JIT refcount release/acquire sites now call the runtime refcount
  helper directly and rely on the helper's own null/immortal checks, avoiding a
  duplicate caller-side branch pair that survived runtime helper inlining.
- throughput: `+5.55%` specialized pystone median; code size `-13.78%`;
  machine blocks `-12.60%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `290599`, `284774`, `285709 loops/s`
  - pystone JIT code bytes: `310747`
  - pystone machine blocks: `19587`
- post-change benchmark:
  - specialized pass, 1M loops x3: `303904`, `297016`, `301574 loops/s`
  - pystone JIT code bytes: `267926`
  - pystone machine blocks: `17119`

## 2026-04-13 - Hoist JIT runtime constant loads

- jj change id: `uunnzzqx`
- summary: Runtime constants loaded from the module constant table now happen
  once in the direct-entry setup and are reused across generated blocks,
  instead of reloading `None`, bools, deleted-sentinel, and empty-tuple for
  every block.
- throughput: `-0.03%` specialized pystone median; code size `-2.35%`;
  machine blocks `-0.71%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `306741`, `306534`, `308409 loops/s`
  - pystone JIT code bytes: `267926`
  - pystone machine blocks: `17119`
- post-change benchmark:
  - specialized pass, 1M loops x3: `302549`, `307871`, `306656 loops/s`
  - pystone JIT code bytes: `261630`
  - pystone machine blocks: `16998`

## 2026-04-13 - Optimize vectorcall argument binding

- jj change id: `rvqxpmyw`
- summary: Direct vectorcall argument binding now uses a precomputed function
  binding plan and writes owned argument values directly into the trampoline's
  output buffer, avoiding per-call `bound_args`, `assigned`, and positional
  parameter-index vector allocations.
- throughput: `+12.54%` specialized pystone median; code size unchanged;
  machine blocks unchanged
- pre-change benchmark:
  - specialized pass, 1M loops x3: `305154`, `311978`, `307370 loops/s`
  - pystone JIT code bytes: `261630`
  - pystone machine blocks: `16998`
- post-change benchmark:
  - specialized pass, 1M loops x3: `336900`, `349622`, `345921 loops/s`
  - pystone JIT code bytes: `261630`
  - pystone machine blocks: `16998`

## 2026-04-13 - Call imported JIT helpers directly

- jj change id: `zwsqzyuv`
- summary: Imported helper calls now keep the Cranelift `Linkage::Import`
  declaration directly instead of defining a local helper-pointer data slot and
  trampoline function per imported symbol. This removes the extra trampoline
  call/return hop, but leaves Cranelift's far external target materialization at
  each call site.
- throughput: `+9.62%` specialized pystone median; code size `+1.37%`;
  machine blocks `+0.02%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `319541`, `347491`, `338516 loops/s`
  - pystone JIT code bytes: `174333`
  - pystone machine blocks: `11257`
- post-change benchmark:
  - specialized pass, 1M loops x3: `373208`, `366708`, `371067 loops/s`
  - pystone JIT code bytes: `176723`
  - pystone machine blocks: `11259`

## 2026-04-13 - Outline decref dealloc preservation

- jj change id: `pmmnxlnu`
- summary: The exception-preserving refcount-zero dealloc path now lives in a
  non-inlined generated runtime CLIF helper and runtime-CLIF calls to generated
  runtime functions are remapped as local JIT functions, removing duplicated
  dealloc-preservation code from inlined decref/store fast paths.
- throughput: `+0.22%` specialized pystone median; code size `-5.44%`;
  machine blocks `+0.03%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `411595`, `406691`, `405697 loops/s`
  - pystone JIT code bytes: `271819`
  - pystone machine blocks: `17385`
- post-change benchmark:
  - specialized pass, 1M loops x3: `417360`, `407568`, `405675 loops/s`
  - pystone JIT code bytes: `257025`
  - pystone machine blocks: `17390`

## 2026-04-13 - Keep scalar builtin chains unboxed

- jj change id: `tlxlmktz`
- summary: Bounded I64 demand now propagates through Add/Sub, letting hot
  `chr(ord(x) + 1)` codegen keep the `ord` result unboxed until the `chr`
  runtime primitive consumes it instead of boxing through PyLong and generic
  Python addition.
- throughput: `+1.84%` specialized pystone median; code size `-0.41%`;
  machine blocks `-0.26%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `421455`, `421742`, `420594 loops/s`
  - pystone JIT code bytes: `257353`
  - pystone machine blocks: `17384`
- post-change benchmark:
  - specialized pass, 1M loops x3: `425734`, `429207`, `432758 loops/s`
  - pystone JIT code bytes: `256306`
  - pystone machine blocks: `17339`

## 2026-04-14 - Use direct object symbols for module constants

- jj change id: `xmxmkuxl`
- summary: Immutable module constants now use direct object-address symbols
  instead of slot-address symbols containing `PyObject*` values, removing the
  extra symbol-value load while preserving symbolic relocation.
- throughput: `+2.23%` specialized pystone median; code size `-2.12%`;
  machine blocks `-1.14%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `424374`, `432790`, `427096 loops/s`
  - pystone JIT code bytes: `257638`
  - pystone machine blocks: `17415`
- post-change benchmark:
  - specialized pass, 1M loops x3: `443631`, `436627`, `436614 loops/s`
  - pystone JIT code bytes: `252190`
  - pystone machine blocks: `17216`

## 2026-04-14 - Split default-resolving direct calls

- jj change id: `rrvownsp`
- summary: Core direct-call JIT entries now assume formal argument slots are
  already well-formed, while vectorcall/defaulted direct edges use a separate
  default-resolving adapter that preserves dynamic `__defaults__` /
  `__kwdefaults__` updates.
- throughput: `+0.45%` specialized pystone median; code size `-0.16%`;
  machine blocks `-1.19%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `438783`, `436726`, `436053 loops/s`
  - pystone JIT code bytes: `252190`
  - pystone machine blocks: `17216`
- post-change benchmark:
  - specialized pass, 1M loops x3: `438677`, `436415`, `441002 loops/s`
  - pystone JIT code bytes: `251784`
  - pystone machine blocks: `17011`

## 2026-04-15 - Specialize exact-list getitem

- jj change id: `xnzntwux`
- summary: `GetItem` now records profiled receiver/index shapes and replays an
  exact-`list`/compact-exact-`int` arm that directly bounds-checks and loads
  `PyListObject.ob_item[index]`, with generic fallback for guard misses.
- throughput: `+1.28%` specialized pystone median; code size `+1.23%`;
  machine blocks `+1.34%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `437687`, `358042`, `457180 loops/s`
  - pystone JIT code bytes: `270780`
  - pystone machine blocks: `16898`
- post-change benchmark:
  - specialized pass, 1M loops x3: `443282`, `441606`, `448104 loops/s`
  - pystone JIT code bytes: `274113`
  - pystone machine blocks: `17125`

## 2026-04-15 - Specialize exact-list setitem

- jj change id: `lwnzpkpv`
- summary: `SetItem` now records profiled receiver/index shapes and replays an
  exact-`list`/compact-exact-`int` arm that directly bounds-checks and stores
  through `PyListObject.ob_item[index]`; specialization guard-miss fallback
  blocks are marked cold.
- throughput: `+0.41%` specialized pystone median; code size `+1.40%`;
  machine blocks `+1.35%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `443282`, `441606`, `448104 loops/s`
  - pystone JIT code bytes: `274113`
  - pystone machine blocks: `17125`
- post-change benchmark:
  - specialized pass, 1M loops x3: `445109`, `450530`, `440886 loops/s`
  - pystone JIT code bytes: `277954`
  - pystone machine blocks: `17356`

## 2026-04-16 - Interpret import-time module scaffolding

- jj change id: `myxywzml`
- summary: BlockPy functions now carry an execution-mode tag. Runtime JIT paths
  skip functions tagged for interpretation, and import-time module/class
  scaffolding (`_dp_module_init`, `_dp_class_ns_*`, and `_dp_define_class_*`)
  runs through the interpreter instead of paying release JIT codegen during
  broad import-hook loading.
- load time: typing slow import release pytest time `4.72s -> 1.86s`; direct
  release wall time `4.80s -> 1.83s`.
- pre-change benchmark:
  - release slow typing import pytest: `1 passed in 4.72s`, `4.95s` wall
  - direct release slow typing import: `4.72s`, `4.80s` wall
- post-change benchmark:
  - release slow typing import pytest: `1 passed in 1.86s`, `2.07s` wall
  - direct release slow typing import: `1.87s`, `1.83s` wall

## 2026-04-16 - Fixed-point range scalarization and exact-int compact ops

- jj change id: `ssnxnmuy`
- summary: Direct-call inlining and constructor scalar replacement now run to a
  small fixed point before value facts/JIT planning. Direct runtime-helper calls
  preserve result facts, and fact-proven exact-int arithmetic/comparison lowers
  to compact `PyLong` machine-int fast paths with cold deopt-on-miss in
  apply/verify mode.
- throughput: `-10.65%` specialized pystone median; no-refcount diagnostic
  `+4.74%`
- pre-change benchmark:
  - specialized pass, 1M loops x1: `259050 loops/s`
  - no-refcount diagnostic, 1M loops x1: `334356 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x1: `231468 loops/s`
  - no-refcount diagnostic, 1M loops x1: `350192 loops/s`

## 2026-04-17 - Specialize exact-int if comparisons

- jj change id: `mysmommr`
- summary: profiled exact-int comparisons used directly as `if` conditions now
  lower to a scalar `I32Bool01` truth value instead of materializing a Python
  bool and calling generic truthiness. Guard-miss deopt is used only when the
  reachable continuation locals are materialized; otherwise the local generic
  fallback remains in place.
- throughput: `+1.10%` specialized pystone median; no-refcount diagnostic
  `+3.02%`; code size `-5.64%`
- pre-change benchmark:
  - specialized pass, 1M loops x5: `255528`, `241373`, `239204`, `249795`,
    `257029 loops/s`
  - no-refcount diagnostic, 1M loops x5: `345733`, `340184`, `264129`,
    `342075`, `337931 loops/s`
  - pystone JIT code bytes: `392208`
- post-change benchmark:
  - specialized pass, 1M loops x5: `252544`, `243623`, `249279`, `260897`,
    `259273 loops/s`
  - no-refcount diagnostic, 1M loops x5: `345491`, `351148`, `349593`,
    `350474`, `353850 loops/s`
  - pystone JIT code bytes: `370090`

## 2026-04-17 - Profile-shape compact exact-int ops

- jj change id: `vwwpmtkw`
- summary: profiled exact-`int` add/sub/mul and comparisons now guard exact
  compact `PyLong` operands, run the arithmetic or comparison on extracted
  machine integers, and deopt to the generic continuation on guard miss.
- throughput: `+3.93%` specialized pystone median; no-refcount diagnostic
  `+0.08%`; code size `+1.21%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `206655`, `250581`, `260174 loops/s`
  - no-refcount diagnostic, 1M loops x3: `341530`, `343662`, `340408 loops/s`
  - pystone JIT code bytes: `370090`
- post-change benchmark:
  - specialized pass, 1M loops x3: `260428`, `260904`, `256758 loops/s`
  - no-refcount diagnostic, 1M loops x3: `344871`, `341789`, `335818 loops/s`
  - pystone JIT code bytes: `374554`

## 2026-04-18 - Replay planned field layouts by index

- jj change id: `noupmxss`
- summary: planned indexed-field layout priming now replays owner-type keys in
  profiled `expected_index` order instead of `HashMap` iteration order. The bad
  order changed `pystone.Record` split-dict key indexes during verify/apply, so
  indexed-field guards missed even though the optimization plan contained the
  right field decisions.
- throughput: `+385.38%` specialized pystone median; no-refcount diagnostic
  `+14.55%`
- pre-change benchmark:
  - specialized pass, 1M loops x3: `50959`, `53673`, `116568 loops/s`
  - no-refcount diagnostic, 1M loops x3: `284496`, `310252`, `299339 loops/s`
- post-change benchmark:
  - specialized pass, 1M loops x3: `260255`, `262657`, `260516 loops/s`
  - no-refcount diagnostic, 1M loops x3: `342902`, `345126`, `342299 loops/s`

## 2026-04-22 - Plan v3 guarded constructor calls

- jj change id: `otplqsxp`
- summary: optimizer v3 now writes guarded constructor-call selections into
  its v3 plan, and the JIT consumes those emitted constructor-call nodes as
  typed-call inputs while preserving the existing constructor allocation/init
  lowering shape.
- throughput: `-0.18%` specialized pystone median; no-refcount diagnostic
  `+2.49%`; code size unchanged
- pre-change benchmark:
  - specialized pass, 1M loops x3: `169294`, `166294`, `170687 loops/s`
  - no-refcount diagnostic, 1M loops x3: `180244`, `175495`, `171471 loops/s`
  - pystone JIT code bytes: `56776`
- post-change benchmark:
  - specialized pass, 1M loops x3: `171222`, `168987`, `159633 loops/s`
  - no-refcount diagnostic, 1M loops x3: `178648`, `179869`, `181974 loops/s`
  - pystone JIT code bytes: `56776`

## 2026-04-22 - Move v3 direct-call rewrites before typed lowering

- jj change id: `xqnzznpu`
- summary: v3 ordinary direct-call store rewrites now run in the profiled
  BlockPy module plan even when the function also has unrelated source-keyed v3
  decisions. Direct-call emissions consumed by that rewrite are filtered out of
  late typed JIT lowering so the selected static plan is not replayed twice.
- throughput: `+1.38%` specialized pystone median; no-refcount diagnostic
  `-0.60%`; code size unchanged
- pre-change benchmark:
  - specialized pass, 1M loops x3: `95238`, `93548`, `95157 loops/s`
  - no-refcount diagnostic, 1M loops x3: `171801`, `168587`, `172543 loops/s`
  - pystone JIT code bytes: `64879`
- post-change benchmark:
  - specialized pass, 1M loops x3: `95367`, `96469`, `96669 loops/s`
  - no-refcount diagnostic, 1M loops x3: `172970`, `170712`, `170764 loops/s`
  - pystone JIT code bytes: `64879`

## 2026-04-22 - Feed v3 method calls into early call rewrites

- jj change id: `mmqxxtum`
- summary: v3 method and constructor call emissions now participate in the
  existing profiled BlockPy no-arg method/runtime-iter rewrite path when that
  rewrite can consume the lowered call site. V3 method/constructor sources
  consumed by module planning are filtered out of late typed JIT lowering.
- throughput: `+0.03%` specialized pystone median; no-refcount diagnostic
  `-3.46%`; code size unchanged
- pre-change benchmark:
  - specialized pass, 1M loops x3: `95367`, `96469`, `96669 loops/s`
  - no-refcount diagnostic, 1M loops x3: `172970`, `170712`, `170764 loops/s`
  - pystone JIT code bytes: `64879`
- post-change benchmark:
  - specialized pass, 1M loops x3: `95203`, `96496`, `96721 loops/s`
  - no-refcount diagnostic, 1M loops x3: `173418`, `164855`, `155884 loops/s`
  - pystone JIT code bytes: `64879`

## 2026-04-29 - Track cleanup-root slot state for refcount cleanup

- jj change id: `wywwtlrs`
- summary: cleanup roots now carry planned physical stack-slot state through
  JIT local planning, so first materializations and proven-empty overwrites skip
  conservative previous-slot DECREF scaffolding. Normal returns with identical
  cleanup-root state share one exit cleanup block.
- throughput: `+5.35%` specialized pystone median; no-refcount diagnostic
  `-0.93%`
- pre-change benchmark: `work/bench/tlwwozxkzrmn_ca6570685879`
  - apply, refcounts enabled, median: `569875 loops/s`
  - no-refcount diagnostic median: `760332 loops/s`
  - total pystone code size: `66689 bytes`, `3918` machine blocks
  - core pystone code size: `63343 bytes`
- post-change benchmark: `work/bench/wywwtlrsvrxn`
  - apply, refcounts enabled, median: `600370 loops/s`
  - no-refcount diagnostic median: `753238 loops/s`
  - total pystone code size: `65169 bytes`, `3918` machine blocks
  - core pystone code size: `61913 bytes`
- notes: verify counters are unchanged, including `runtime_decref=8163218` and
  `runtime_incref=6957036`; the win comes from emitted cleanup shape rather
  than a changed number of executed refcount operations.

## 2026-05-01 - Keep scalar locals out of owned cleanup roots

- jj change id: `kvmsrnls`
- summary: scalar-local exact-i64 values now keep cleanup-root slots empty
  until a deopt, fallback, or generic boundary requires PyObject
  materialization. The merge also removes the old scalar-thread branch special
  case and resolves `BlockParamRole::Value` through generator resume dispatch.
- throughput: `+8.27%` specialized pystone median versus a same-window
  current-main baseline; no-refcount diagnostic `+0.13%`; total pystone JIT
  code size `-3.53%`
- pre-change benchmark: `work/bench/wsurmszspnmr_4a2e726b336f`
  - apply, refcounts enabled, median: `527726 loops/s`
  - no-refcount diagnostic median: `621519 loops/s`
  - total pystone code size: `68854 bytes`, `4025` machine blocks
  - core pystone code size: `65327 bytes`
- post-change benchmark: `work/bench/kvmsrnlslooy`
  - apply, refcounts enabled, median: `571371 loops/s`
  - no-refcount diagnostic median: `622344 loops/s`
  - total pystone code size: `66422 bytes`, `3994` machine blocks
  - core pystone code size: `62987 bytes`
- notes: verify counters drop from `runtime_decref=8163218` to `5848196` and
  `runtime_incref=6957036` to `4642014`; remaining hot refcount sites are still
  dominated by real stack exit sweeps and boxed integer-result overwrites such
  as `IntLoc3`, `IntLoc1`, and `IntLoc2`.

## 2026-05-05 - Preserve constructor scalar state across deopting guards

- jj change id: `tplpstrplxvztmuulvyrxxomxrxxyyol`
- summary: typed constructor field scalarization now treats the fallback edge
  of a `guard_miss_deopt` direct-call guard as non-continuing, so hot
  constructor field state survives across the guarded call shape without
  cloning more of the CFG.
- throughput: `+1.96%` specialized pystone median versus the previous
  same-stack smoke run; total pystone JIT code size `-1.95%`
- pre-change benchmark: `work/bench/tplpstrplxvz_df0a5747f616`
  - apply, refcounts enabled, median: `392575 loops/s`
  - no-refcount diagnostic median: n/a
  - total pystone code size: `447523 bytes`, `24590` machine blocks
  - core pystone code size: `435267 bytes`
- post-change benchmark: `work/bench/tplpstrplxvz_da785f209429`
  - apply, refcounts enabled, median: `400252 loops/s`
  - no-refcount diagnostic median: n/a
  - total pystone code size: `438779 bytes`, `24590` machine blocks
  - core pystone code size: `426523 bytes`

## 2026-07-26 - Bring native N-Queens to practical CPython parity

- jj change id: `mzvswpyykkts`
- summary: specialize tuple accesses, generator calls, and field reads; retain
  source-backed generator globals, native vectorcall, CPython call versions,
  owned mutation snapshots, and lazily watched mutable method owners.
- native N-Queens, CPU 16, 200 loops, 92 solutions: SOAC `8.203 runs/s`, stock
  CPython `8.503 runs/s`; SOAC is `3.53%` slower.
- loss-checked Linux `perf`, 160 loops, 92 solutions: SOAC `8.212 runs/s`, stock
  CPython `8.326 runs/s`; SOAC is `1.37%` slower, with zero lost samples.
- repeated loss-checked Linux `perf` after production-faithful generator
  inspection, 160 loops, 92 solutions: SOAC `8.251 runs/s`, stock CPython
  `8.453 runs/s`; SOAC is `2.39%` slower, with zero lost samples in both runs.
- repeated loss-checked Linux `perf` after guarding ordinary direct calls and
  inlined functions against code mutation, 160 loops, 92 solutions: SOAC
  `8.268 runs/s`, stock CPython `8.453 runs/s`; SOAC is `2.19%` slower, with
  zero lost samples in both runs. The remaining global function-watcher
  dispatch accounts for `0.75%` of sampled SOAC execution.
- loss-checked Linux `perf` after restoring explicitly trusted,
  guard-free runtime inlining, 160 loops, 92 solutions: SOAC `8.118 runs/s`,
  stock CPython `8.512 runs/s`; SOAC is `4.63%` slower, with zero lost samples
  in both runs. Global function-watcher dispatch accounts for `0.82%` of
  sampled SOAC execution; CPython parity remains unachieved.
- CPU-pinned release measurement without `perf` sampling, 240 loops, 92
  solutions: SOAC `8.248 runs/s`, stock CPython `8.459 runs/s`; SOAC is
  `2.49%` slower. The direct-call code guard now retains an append-only
  per-function code snapshot as preparation for reducing watcher overhead.
- loss-checked Linux `perf` with the ABI-preserving per-function code guard,
  160 loops, 92 solutions: SOAC `8.256 runs/s`, stock CPython `8.476 runs/s`;
  SOAC is `2.60%` slower, with zero lost samples in both runs. The exact
  function guard validates the live CPython code pointer before entering
  optimized direct or inlined function bodies.
- loss-checked Linux `perf` after removing the eager process-wide function
  watcher, 160 loops, 92 solutions: SOAC `8.320 runs/s`, stock CPython
  `8.479 runs/s`; SOAC is `1.88%` slower, with zero lost samples in both
  runs. Ordinary functions use owned code/default snapshots and cold native
  vectorcall fallback; compiler-owned runtime classes retain indexed owner
  metadata, while mutable source classes enable the watcher lazily.
- repeated CPU-pinned production measurements without `perf` sampling,
  240 loops each, 92 solutions: SOAC `8.425`, `8.431`, and `8.450 runs/s`;
  stock CPython `8.410`, `8.468`, and `8.533 runs/s`. Median throughput is
  `8.431` versus `8.468 runs/s`, `0.44%` below stock and within native
  N-Queens parity; one stock-first comparison is `0.25%` faster than stock.
- final loss-free Linux `perf` comparison at 49 Hz, CPU 16, 240 loops, 92
  solutions: SOAC `8.173 runs/s`, stock CPython `8.193 runs/s`; SOAC is
  `0.24%` below stock. Both runs retained deep DWARF call graphs and lost zero
  samples or chunks.
- alternating same-process comparison using two real imported modules on CPU
  16, ten 20-loop samples per mode: median SOAC throughput is `8.3206 runs/s`
  versus stock CPython `8.3173 runs/s`, `0.04%` above stock. This controls for
  the host-frequency and run-order drift visible in separate native processes.
- production apply intentionally acts on zero of the two source-backed named
  generator bodies and zero of their consumer-fusion opportunities. A native
  `PyGenObject` can escape through frame, tracing, monitoring, traceback, and
  finalization observers even when ordinary dataflow shows immediate builtin
  consumption; SOAC does not yet have suspended-frame materialization for a
  correct mid-loop deopt. The mechanical transformed-generator rewrite remains
  covered and bounded, but native N-Queens parity comes from retaining CPython's
  generator execution rather than enabling that unsound production path.
- SOAC before/after: `0.279 runs/s` to `8.431 runs/s`, `30.22x` faster.
- finalized production pystone profile, verify, and refcounts-enabled apply
  medians for the exact recorded revision are in `work/bench/mzvswpyykkts`.

## 2026-07-28 - Fuse the pinned N-Queens generator graph

- jj change id: `ruqwzllknkowzyounptxkkqlzvupuzvx`
- summary: validate the complete six-producer N-Queens graph and exact source,
  then replace its nonescaping Count or Discard sink with guarded scalar
  bit-mask search while retaining the original graph as cold fallback.
- production pyperformance N-Queens on CPU 16: stock CPython `118 ms`, SOAC
  apply `34.8 us`; SOAC is `3393.97x` faster.
- loss-checked `cpu-clock` perf slice on CPU 16: stock CPython `8.402 runs/s`,
  SOAC apply `28527.743 runs/s`; SOAC is `3395.35x` faster, with zero lost
  samples in both runs.
- finalized pystone apply median versus parent `main`: `263838` to `263995`
  loops/s (`+0.06%`); total generated code remains `206088 bytes`. The
  no-refcount diagnostic median is `254609 loops/s`.
- finalized pystone artifact: `work/bench/ruqwzllknkow`.

## 2026-08-18 - Preserve scalar-region indexed-field guard dependencies

- jj change id: `tmtwnonn`
- summary: discard exact scalar-region plans when their borrowed indexed-field
  input lacks a matching surviving typed owner/layout guard; `richards` and
  `deltablue` now complete specialized apply instead of failing.
- `chaos` SOAC apply: `215.22 ms` to `192.72 ms` (`1.117x` faster, exploratory
  previous baseline); candidate stock CPython: `29.54 ms`, or `0.153x` stock.
- newly completed exploratory runs: `richards` `381.46 ms` versus stock
  `22.10 ms` (`17.26x` slower); `deltablue` `484.26 ms` versus stock `1.47 ms`
  (`328.69x` slower).

## 2026-08-18 - Register constructor targets during class creation

- jj change id: `ntkwxnmn`
- summary: register safe transformed classes before returning from class
  creation so profile evidence selects existing guarded constructor calls.
- pyperformance SOAC apply medians: `chaos` `100.61 ms` to `95.83 ms`
  (`1.050x`), `deltablue` `4.69 ms` to `4.58 ms` (`1.024x`), and `richards`
  `43.20 ms` to `44.06 ms` (`0.981x`); mixed-workload geometric improvement
  `1.018x`. The candidate remains `0.357x` stock CPython across that subset.

## 2026-08-18 - Reuse prepared synthetic closure code

- jj change id: `otwslxwo`
- summary: reuse each synthetic closure's immutable named code object while
  preserving fresh captures, original source code, runtime mutation, and
  reentrant initialization.
- pyperformance SOAC apply: `chaos` `94.56 ms` to `84.28 ms` (`1.122x`
  faster, statistically significant); three-workload geometric improvement
  `1.050x`, with unchanged generated code and a `0.401x` stock-CPython score.

## 2026-08-18 - Preserve function-captured builtins in global lookup

- jj change id: `zolltvqv`
- summary: reuse each function's captured builtins for JIT global misses while
  preserving custom mappings, indexed-dictionary deletion, and missing names.
- pyperformance SOAC apply: `deltablue` `4.58 ms` to `4.43 ms` (`1.033x`) and
  `richards` `43.80 ms` to `40.49 ms` (`1.082x`), both statistically
  significant; four-workload median geometric improvement `1.053x`. The
  mean-based comparison is `1.006x` after a large `chaos` outlier; the
  candidate remains `0.274x` stock CPython across the measured subset.

## 2026-08-18 - Use compiler-owned fixed-length sequence unpacking

- jj change id: `oqprxors`
- summary: replace compiler-generated fixed-arity Python unpack calls with a
  guarded native language operation while preserving generic iteration and
  mutable user-visible helpers.
- eight-workload previous-SOAC geometric improvement `1.757x`; paired stock
  score `0.281x` to `0.460x`. `nbody` improves `885.5 ms` to `70.3 ms`
  (`12.60x`) and `spectral_norm` `484.4 ms` to `56.2 ms` (`8.62x`).
  Repeated `comprehensions`, `deltablue`, and `fannkuch` regressions remain
  unresolved.

## 2026-08-18 - Fuse profiled exact-float expression trees

- jj change id: `tsrrtrqm`
- summary: execute validated multi-operation exact-float trees as guarded
  native arithmetic with one final Python-float allocation.
- repeated significant SOAC median improvements: `float` `53.56 ms` to
  `51.16 ms` (`1.047x`) and `nbody` `69.80 ms` to `65.14 ms` (`1.072x`);
  eligible-workload geometric improvement `1.059x`. The noisy eight-workload
  mean/median ratios are `0.904x` / `0.996x`; paired stock score moves from
  `0.460x` to `0.468x`.

## 2026-08-19 - Specialize late-bound profiled instance fields

- jj change id: `zssttuox`
- summary: guard exact slotted and split-dictionary instance fields through
  weak, versioned owner cells populated after class creation.
- eight-workload stock score `0.468x` to `0.513x`; previous-SOAC mean/median
  improvements `1.117x` / `1.040x`, with `float` median `1.280x`. Repeated
  affected-workload median improvement `1.066x`; `comprehensions` and
  `richards` regress, and generated native code grows `2.757%`.

## 2026-08-19 - Recover guarded late-owner scalar regions

- jj change id: `nzlwkyzw`
- summary: reuse existing weak owner-field guards in selected scalar regions
  while preserving inlined original-source specialization counters.
- eight-workload paired stock score `0.484x`; previous-SOAC robust median
  improvement `1.009x`. Three-round affected-workload robust improvement
  `1.037x`; generated native code shrinks `0.247%`.

## 2026-08-19 - Reuse source-backed function materialization

- jj change id: `xtnupnyk`
- summary: reuse immutable original-function metadata and ready-entry keys
  while restoring CPython function-watcher initialization behavior.
- eight-workload stock score `0.513x`; previous-SOAC robust improvement
  `1.043x`. Three-round affected-workload robust improvement `1.096x`, led
  by `comprehensions` `83.21 us` to `66.60 us` (`1.249x`); generated native
  code remains unchanged.

## 2026-08-19 - Initialize trusted generator state directly

- jj change id: `okqlrmxm`
- summary: avoid interpreted generator-state tuple bridging and restore
  canonical generator-name identity.
- eight-workload stock score `0.510x`; previous-SOAC robust improvement
  `1.006x`. Repeated `comprehensions` improves `66.40 us` to `62.13 us`
  (`1.069x`); generated native code remains unchanged.

## 2026-08-19 - Specialize polymorphic inherited instance fields

- jj change id: `nvvlrumm`
- summary: guard inherited split-dictionary fields by exact profiled owner
  and preserve existing lexical-owner specializations.
- eight-workload stock score `0.521x`; previous-SOAC robust improvement
  `1.016x`. Three-round robust improvement `1.067x`, led by `deltablue`
  `1.112x` and `richards` `1.171x`; generated native code grows `4.256%`.

## 2026-08-19 - Fast-path exact positional argument binding

- jj change id: `nnyqlvvy`
- summary: reuse direct argument plans for fully supplied positional calls.
- eight-workload stock score `0.548217x` versus prior `0.520917x`;
  previous-SOAC improvement `1.05714x`, targeted robust `1.05567x`.
  `deltablue` improves `3.750 ms` to `3.529 ms` and `richards` `33.959 ms`
  to `31.815 ms`; generated native code remains unchanged.

## 2026-08-19 - Specialize hot non-self instance fields

- jj change id: `mztqqkor`
- summary: reuse existing exact-owner constructor cells for hot non-self
  loads and stores.
- eight-workload stock score `0.559460x` versus prior `0.548217x`;
  previous-SOAC robust improvement `1.003x`, three-round targeted `1.037x`.
  `deltablue` improves `1.057x` and `richards` `1.072x`; `comprehensions`
  regresses `3.34%` and generated native code grows `2.79%`.

## 2026-08-19 - Reuse interned runtime lookup keys

- jj change id: `wtyxsxpv`
- summary: reuse guarded interned Unicode keys for live runtime lookups.
- eight-workload stock score `0.555839x` versus prior `0.559460x`; mean
  previous-SOAC ratio `0.990x`, robust `1.003x`. Repeated `comprehensions`
  improves `1.067x` (normal median `63.29 us` to `60.68 us`); generated
  native code remains unchanged.

## 2026-08-19 - Guard canonical StopIteration matcher dependencies

- jj change id: `rllxmowx`
- summary: replace unsafe matcher direct calls with exact-exception,
  live-dependency-guarded runtime dispatch.
- eight-workload stock score `0.578205x` versus prior `0.555839x`;
  previous-SOAC improvement `1.035x`. Repeated `chaos` improves `1.121x`
  and `deltablue` `1.039x`; generated native code shrinks `6.95%`.

## 2026-08-19 - Reuse known function-instantiation templates

- jj change id: `kvpzmtlp`
- summary: propagate existing function templates through registration and
  reuse session/arity-matched vectorcall trampolines.
- eight-workload stock score `0.602845x` versus prior `0.578205x`;
  previous-SOAC improvement `1.02674x`. Repeated `comprehensions`
  improves `1.035x`; generated native code remains exactly `23,293,040 B`.

## 2026-08-19 - Guard indexed runtime factory lookups

- jj change id: `zwkrytkq`
- summary: read live indexed runtime-factory slots under exact module and
  mutable-type guards while retaining observable attribute fallbacks.
- eight-workload stock score declines `0.602845x` to `0.588346x`;
  previous-SOAC ratio `0.996949x`. Repeated `comprehensions` improves
  `1.053x` and `chaos` `1.048x`; native code remains `23,293,040 B`.

## 2026-08-19 - Elide eager comprehension function objects

- jj change id: `wnlnpkrp`
- summary: use guarded compiled-child callables for eager comprehensions,
  restoring stock function-watcher and code-audit parity.
- eight-workload stock score `0.589676x` versus prior `0.588346x`;
  robust previous-SOAC `1.01127x`. Repeated `comprehensions` improves
  `1.04963x` (`52.42 us` to `49.94 us`); native code decreases
  `23,293,040 B` to `23,188,640 B`.

## 2026-08-19 - Guard canonical generator builtin consumption

- jj change id: `lnxvnnml`
- summary: consume source generators through guarded compiled resume,
  restore stock exhaustion behavior, and partition call dispatch.
- eight-workload stock score `0.632661x` versus prior `0.589676x`;
  official previous `1.06128x` is outlier-sensitive, with robust previous
  `0.99921x`. Repeated `comprehensions` improves `1.11263x`
  (`49.93 us` to `44.87 us`); native code remains `23,188,640 B`.

## 2026-08-19 - Inline exact positional vectorcall binding

- jj change id: `rukzksko`
- summary: partition exact-arity trampolines and enter validated core
  function bodies directly.
- eight-workload stock score `0.614608x` versus prior `0.632661x`;
  previous-SOAC `1.03884x`. Repeated `deltablue` improves `1.08465x`
  (`3.177 ms` to `2.929 ms`) and `richards` `1.05533x` (`29.845 ms` to
  `28.280 ms`); body native remains `23,188,640 B`, with hidden
  trampolines increasing `287,200 B` to `365,000 B`.
