
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
   artifacts in logs/.

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
