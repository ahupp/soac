
Your job is to make the benchmark (run via "just benchmark") as fast
as possible. Use the $analyze-pystone-perf skill to run the benchmark,
using a combination of:

* perf profiles
* verification mode counters, to see if expected optimizations are
  actually taking effect
* CLIF inspection
* your own brilliance

to find optimizations, implement them, and verify they actually improve
things.


## Guidelines

1. Changes should continue to pass tests. Don't do anything that would
   be expected to change user-visible behavior vs CPython. During
   iteration, run the focused test or repro that covers the optimized
   path; before landing non-doc changes, run the repo's full gate.

2. Changes should not be overly specific to the benchmark itself. The
   optimization should apply to general-purpose code. An outside
   observer who's never seen the pystone benchmark should be able to
   look at the change and say, "yes, that is a sensible thing to do".

3. Measure the specialized/apply pass as the transformed benchmark
   result. The profile pass is optimization input, not the headline
   throughput result.


## Optimization Loop

For each optimization attempt:

1. Establish a before/after baseline. Compare the candidate change
   against its parent with the same loop counts, same specialization
   input policy, and same benchmark-stability knobs. Report absolute
   specialized throughput, stock CPython throughput, and relative change.

2. Run verify mode before trusting perf conclusions. Expected hot
   specialization sites from the profile run should either appear in the
   verify run or have an understood reason for disappearing.

3. Collect perf for the specialized/apply pass, not the profiling pass.
   Keep benchmark, verify, perf, counter-summary, and rendered-CLIF
   artifacts in work/logs/.

4. Render specialized CLIF for the hot JIT functions. Before coding,
   connect the proposed optimization all the way through: perf hotspot,
   generated CLIF shape, relevant source shape, and the codegen/runtime
   decision that produced it.

5. Write down one falsifiable hypothesis, for example: "removing helper
   X from generated shape Y should reduce hotspot Z". Then implement the
   smallest general-purpose change that tests that hypothesis.

6. Re-run the same benchmark protocol. Report every repeated
   specialized run, the before/after absolute numbers, the relative
   change, and whether the relevant verify counters / CLIF shape changed
   as expected.

7. Update `docs/CODEX_OPT_LOG.md` before moving on. Log landed changes,
   benchmark-negative attempts, and inconclusive attempts that consumed
   meaningful investigation time. Keep entries concise: candidate
   summary, landed jj change id if any, specialized-throughput
   before/after, relative change, and the reason a candidate was
   abandoned.

8. For an abandoned candidate, also record the transferable lesson in
   this file before picking the next candidate. Include why the original
   hypothesis was incomplete: for example, the hotspot was split across
   several parents, the generated fast path expanded too much CLIF, the
   optimized helper was below the benchmark's noise floor, or the change
   optimized the consumer when the allocation / dispatch cost came from
   the producer.


## Lessons From Abandoned Attempts

### Indexed-field helper owner-type threading

After indexed field-store misses were eliminated in pystone, the remaining
`soac_runtime_store_field_indexed` self time looked like it might include
avoidable duplicate type-shape work. An attempted helper ABI change passed the
already-guarded exact owner type into the field probe/store helpers so they
could skip reloading the object's type and deriving the dict / inline-values
path from scratch.

The result was benchmark-negative: median specialized throughput changed
`416438 -> 415850 loops/s` (`-0.14%`). Generated pystone code size dropped
slightly (`257025 -> 256777` bytes), but that was below the landing threshold
and did not translate to throughput.

Lesson: successful indexed field stores are no longer dominated by owner-type
lookup alone. The remaining cost includes split-slot validation, first-insert
insertion-order maintenance, overwrite refcount traffic, and helper call shape.
Future work should either remove more of the store operation at once, such as a
full inlined/batched split-slot store for a specific storage shape, or use
typed ownership/immortal facts to reduce refcount work. Do not churn the helper
ABI just to save the duplicate type load without a clearer measured win.

### Exact-list item access after helper specialization

After the exact-list getitem/setitem helper optimization, the remaining
`exact_list_index` cost looked tempting but was already small and split
between operations. The specialized pystone perf sample attributed about
`2.28%` to `exact_list_index`, with roughly `1.00%` under
`dp_jit_pyobject_getitem` and `0.86%` under `dp_jit_pyobject_setitem`.

An attempted generated getitem-only fast path was benchmark-negative:
median specialized throughput changed `319499 -> 314717 loops/s`
(`-1.50%`). The rendered fast path replaced the helper call on the
getitem hit path, but added exact-type guards, compact-long decoding,
negative-index normalization, bounds checks, direct item loads, incref /
decref work, hit/result/fallback merge blocks, and still kept the
generic helper on the miss path. That CLIF/body expansion outweighed the
small part of the helper that belonged to getitem.

A direct `ob_type` check inside the helper was inconclusive: median
specialized throughput changed `319499 -> 320632 loops/s` (`+0.35%`),
which is in the observed run-to-run noise for this benchmark.

Lesson: before inlining a helper, separate self time from each parent
operation and estimate the part that the candidate actually removes.
For list subscripts, prefer a broader plan that covers both load and
store or removes allocation/refcount traffic around the subscript.
Treat one or two type-check instructions inside an already-small helper
as below the landing threshold unless repeated benchmarks show a clear
win.

### Truth tests after rich comparisons

The profile showed `dp_jit_is_true` at about `1.43%` self, but
`PyObject_IsTrue` itself was only about `0.62%` self. For pystone's
branch conditions, a larger cost is the producer side: rich comparisons
allocate / return the `True` or `False` singleton as an owned
`PyObject *`; then control-flow lowering calls `dp_jit_is_true` and
decrefs that bool.

A generated singleton-truth fast path before `dp_jit_is_true` was
benchmark-negative: median specialized throughput changed
`319499 -> 307591 loops/s` (`-3.73%`). In rendered CLIF, each optimized
truth test grew into pointer comparisons against `True`, `False`, and
`None`, several extra `brif`s, dedicated true/false/fallback blocks, and
a result merge; the fallback `dp_jit_is_true` call remained present.

A singleton fast path inside `dp_jit_is_true` was also negative: median
specialized throughput changed `319499 -> 304941 loops/s` (`-4.56%`).
It only targeted the already-small consumer helper and did not remove
the `PyObject_RichCompare` call, bool-object result, decref, or the
helper call frame itself.

A sound branch-context helper that combined `PyObject_RichCompare`,
truth conversion, and compare-result decref was also negative: median
specialized throughput changed `319499 -> 306582 loops/s` (`-4.04%`).
It removed the `PyObject_RichCompare` / `dp_jit_is_true` pair from
rendered Proc0 CLIF, but still produced the owned compare result inside
an out-of-line Rust helper and still paid generic rich-compare dispatch.

Lesson: optimize branch-producing comparisons as comparisons, not as
owned-bool objects followed by generic truth. A better hypothesis would
lower branch-context exact-int, identity, or Unicode/string comparisons
to an `i32` / CLIF condition directly, or call an appropriate
rich-compare-bool style helper, while preserving the owned-object
compare result when Python code actually consumes that bool.

### Exact-int truth helper splitting

An attempted split of exact-`int` `not` / internal truth unary operations into a
new `nb_bool`-returning helper was benchmark-negative: median specialized
throughput changed `126471 -> 125195 loops/s` (`-1.01%`). The specialization set
did not change, and the baseline exact-long unary helper path was only about
`0.05%` in the perf symbol report.

Lesson: do not split tiny object-returning helpers into separate out-of-line
truth helpers as a standalone optimization. For typed truth to pay off, it needs
to remove a larger producer/consumer chain or stay in generated code through a
consumer that actually wants machine truth, such as branch lowering or
result-demand-aware statement lowering.


## Candidate Backlog

Use fresh benchmark + verify + perf data to rank work; do not blindly
follow this list. Current likely optimization families include:

* global-load specialization from profiled module-key layouts
* attribute-load specialization from profiled type-key layouts
* refcount reduction on values that remain live within one JIT function
* broader exact-int operator lowering, with generic PyNumber fallback
* direct-call and small-function inlining when call targets are verified
  hot and stable
* guard elision when ownership, module mutability, or type versioning
  gives a concrete soundness argument
