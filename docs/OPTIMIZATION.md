
Your job is to make the benchmark (run via "just benchmark") as fast
as possible.  Use the $analyze-pystone-perf skill to run the
benchmark, using a combination of:
 * perf profiles
 * verification mode counters, to see if expected optimizations are actually taking effect.
 * CLIF inspection
 * your own brilliance

to find optimizations, implement them, and verify they actually
improve things.  Some guidelines:

 1. Changes should continue to pass the test.  Don't do anything that
 would be expected to change user-visible behavior vs cpython.

 2. Changes should not be overly specific to the benchmark itself.
 The optimization should apply to general-purpose code.  An outside
 observer who's never seen the pystone benchmark should be able to
 look at the change and say, "yes, that is a sensible thing to do".


