# Optimization goal

The primary measurable goal of this line of work is to improve SOAC's
performance on the pyperformance suite. The current success target is to beat
the same stock CPython by at least 10% on the geometric mean of a benchmark set
fixed before the comparison. Define the score as
`stock elapsed time / SOAC apply elapsed time`, or equivalently
`SOAC apply throughput / stock throughput`; the target score is at least
`1.10`. A full-suite aggregate is incomplete when any comparable result is
missing or failed; an intersection aggregate may be shown only when explicitly
labeled. Report every per-benchmark ratio, execution coverage, and whether the
benchmark's meaningful hot code was actually transformed alongside the
aggregate.

Pyperformance is a proxy for progress on long-running Python programs. Direct,
representative server measurements are difficult at this phase, so the suite
provides a repeatable optimization target. The intended end result is faster
real programs, not benchmark recognition. Optimizations should therefore be
expressed as reusable semantic facts and local transformations whenever
possible.

The full pyperformance suite is the acceptance criterion, but it is too slow
for every edit. Use its independently runnable `chaos` workload as the fast
mixed-workload sanity check: its pure-Python fractal/spline implementation
exercises custom classes, mutable objects, attribute access, method calls,
nested loops, lists, branches, and integer/floating-point operations. If
`chaos` is unavailable or cannot exercise the relevant transformed hot path,
use the existing pystone benchmark instead.
Investigate and fix substantial `chaos` or pystone regressions, but an
improvement on either fast workload does not establish progress toward the
full-suite pyperformance goal. Monitor typed-IR growth and generated
native-code size; investigate material growth even when peak memory is
acceptable. Optimizer peak-memory measurements, detailed planning-time
accounting, and calculated break-even points are not currently required.

## Measurement model

Headline performance is the normally trained specialized `SOAC_OPT_MODE=apply`
pass, measured without an attached native profiler and compared with the same
vendored stock CPython. Compare the changed SOAC revision against both stock
CPython and the previous SOAC revision. Use identical inputs, affinity, clock
policy, benchmark variants, and module-selection policy, and generate fresh
profile evidence independently for each SOAC revision. A profile pass supplies
the optimization evidence required by apply. A verify pass is optional
diagnostic evidence for checking whether expected paths, guards, and fallback
counters were actually exercised; profile, verify, and unspecialized throughput
are never the headline result.

Use separate native `perf` captures, including JIT-symbol attribution, to find
and explain hot paths. Pyperformance measures the outcome; `perf`,
generated-IR inspection, and JIT code summaries explain it. Run
`just pyperformance-compare` for repeatable stock-versus-SOAC measurements and
comparison against an available prior SOAC result; it defaults to the
`chaos` fast workload, so pass the full target benchmark set when claiming
the overall goal. Final performance claims should use pyperformance's
statistical comparison across at least three independently started,
order-alternated comparisons. A delta within measured noise is inconclusive
rather than a win or regression.

For each retained performance change, report at least:

- per-benchmark changed-SOAC/stock and changed-SOAC/previous-SOAC performance;
- the fixed benchmark set, geometric-mean aggregate, and whether it meets the
  current 10% target;
- completed, missing, and failed benchmarks and material regressions;
- actual transformed hot-path coverage, including whether benchmark code,
  standard-library modules, and third-party dependencies were transformed or
  remained on stock CPython;
- material startup or compilation overhead when it explains a regression; and
- available typed-IR instruction/block counts and emitted native-code size,
  highlighting material growth or stating when instrumentation is unavailable.

Benchmark completion does not demonstrate meaningful JIT coverage. A benchmark
whose work occurs in an untransformed standard-library or third-party module is
not evidence that SOAC optimized that work; conversely, transforming a large
standard-library import graph can increase startup time without helping the
measured loop. Keep module-selection policy explicit and inspect hot-path and
generated-code evidence before drawing either conclusion. For example, the
default `chaos` configuration transforms its benchmark classes and functions
while imported standard-library `math` and `random` remain on stock CPython.

## Profile and apply phases

Optimization selection must not adapt dynamically from observations made by
the optimized process. A profile process measures a workload, then a restarted
apply process uses that evidence to construct and validate optimization plans.
The cycle may be repeated deliberately. Code generation may still occur when
the apply process starts or loads a function, and optimized code may contain
entry guards, fallbacks, and supported deoptimization paths.

Profile evidence selects and prioritizes candidates; it does not prove Python
semantics. Apply must validate evidence against the relevant module, source,
function, and typed-IR assumptions that the evidence format can actually
identify. Stale or inconsistent evidence disables the affected optimization.
Counter dumps currently identify module/source content but do not record a
compiler revision or build identity, so automatic rejection of profiles from a
different SOAC compiler is not an existing guarantee. The Python source can
remain identical while a compiler change changes instruction identities or
optimization-site meaning. Generate independent profile evidence for each
revision instead of reusing profiles across compiler changes;
compiler-identity validation would require a future evidence-format and reader
change.

## Optimization structure

Analyses, decisions, and transformations should be independent components with
explicit inputs and outputs. The pipeline below describes an architectural
direction, not a claim that every fact cache, speculative overlay,
transactional commit, or prioritized worklist already exists. Improve the
current production path in the smallest sound, measurable step; do not rewrite
the optimizer merely to implement aspirational infrastructure before a concrete
optimization needs it:

```text
profile evidence + static program facts
  -> guarded candidate decisions
  -> speculative IR view when needed
  -> reusable fact analyses
  -> independent transformation plans
  -> legality and profitability of the complete bundle
  -> atomic commit
  -> mechanical code generation
```

Keep these categories distinct:

- **Policy contracts** are deployment or language restrictions that SOAC
  deliberately enforces. A proposed contract is not available to the compiler
  until its enforcement and failure behavior have been implemented and
  approved.
- **Proven facts** follow from the current typed IR and semantic analyses.
- **Guarded assumptions** depend on mutable runtime state and name the guards
  and untouched fallback that make them safe.
- **Profile observations** estimate frequency, types, targets, and
  profitability, but are not correctness facts.
- **Optimization plans** consume those inputs and describe a proposed rewrite,
  its guards, invalidations, and commit obligations.

Facts are about one typed-IR snapshot and its explicit assumptions. A
fact-producing analysis may consume semantic operation or call-effect
summaries, but it must not inspect the transformation plan that hopes to use
the fact. Resolved decisions belong in typed IR or in sidecars validated
against it. Code generation should emit those decisions mechanically rather
than rediscovering them.

Correctness eligibility and profitability are separate decisions. Profile
frequency or a large expected speedup can make an expensive legal
transformation worthwhile, but can never make an illegal transformation legal.

### Demand-driven facts and iteration

SOAC should not infer every possible fact, apply every possible
transformation, and blindly rerun the entire optimizer to a global fixed point.
Speculative inlining can create many alternate program shapes, making that
approach needlessly expensive. The default strategy is demand-driven:

1. Profile evidence and inexpensive structural scans discover and rank
   candidates.
2. A candidate requests only the facts needed to establish legality and
   estimate its value.
3. Fact providers may request prerequisite facts. Results are cached for an
   explicit IR revision, scope, and assumption set.
4. A legal and promising candidate produces a transformation plan without
   mutating the live IR.
5. On commit, the rewrite framework conservatively invalidates affected fact
   families and places only dependent candidates and newly exposed local
   opportunities back on a prioritized worklist.

Some analyses, such as dominators, reachability, or a function-wide use graph,
are naturally computed for a whole function. Demand-driven means computing an
analysis family when first needed and reusing it, not computing every scalar
fact separately. Begin with conservative function or region revisions; use
finer-grained incremental invalidation only when measurement justifies its
complexity.

Positive and negative cached results both require versioning and invalidation.
Each cached analysis result is keyed by its scope, IR revision, policy
contracts, and guarded assumptions. Fact providers declare dependencies at
analysis-family granularity; finer dependencies may be recorded when
measurement justifies them. The rewrite framework owns conservative
invalidation based on mutated IR kinds and scopes. A transformation may supply
a validated narrower change summary, but a missing or rejected summary
invalidates the enclosing region or function.

Analyses are observationally pure: they may populate fact caches, diagnostics,
and metrics, but must not mutate typed IR or optimization decisions. Query or
candidate order may affect which profitable optimizations fit a resource
budget, but must not affect Python-visible semantics or the soundness of facts
consumed by a selected optimization; identical optimization decisions and
generated code are not required.

Fact dependency cycles are solved to a sound fixed point within the smallest
relevant analysis family or region. Fixed-point analyses publish only sound
results. If a resource limit prevents convergence, the result is conservative
or `Unknown`, and dependent optimizations decline. Proposal fingerprints,
code-size and compile-time budgets, and bounded local rounds limit
transformation exploration and prevent duplicate work and non-monotonic
cycles; they never authorize consuming an incomplete analysis result.
Whole-function or whole-program saturation remains appropriate when
measurement shows that it pays for itself; it is not the default.

### Speculation and atomic commit

A semantics-preserving rewrite may commit independently. A rewrite that exposes
a compiler-owned object or operation whose surviving behavior differs from
CPython is compatibility-relaxed and must be analyzed speculatively on an
immutable projected view, overlay, or clone of the IR.

When one tentative transformation exposes facts needed by another, the
optimizer applies the first plan only to that speculative view, runs the
ordinary fact providers against the projected IR revision, and composes any
downstream plans there. The live IR remains unchanged until the complete bundle
is accepted.

Such a proposal declares `MustEliminate` for every non-equivalent temporary
object or operation. The complete dependent bundle--for example guarded target
selection, constructor or method inlining, ownership proof, virtual-object
lowering, and consumer materialization--is committed atomically only after all
legality, profitability, and `MustEliminate` obligations are discharged. Each
proposal names its base IR revision and assumption set. Immediately before
commit, SOAC revalidates that base and every static obligation, applies the
bundle to a transaction or clone, validates the resulting typed IR, and only
then replaces the live function. A mismatch discards or replans the proposal.
Facts derived from a speculative view are conditional on that proposal and
must not leak into the live fact cache if the proposal is rejected.

Commit also requires proof that every required runtime guard can be emitted,
dominates all effects that rely on its assumption, and reaches the untouched
fallback on failure. At runtime, successful guards select the optimized bundle;
failed guards select the fallback before dependent visible effects.

Each guarded assumption must state how long it remains valid. Entry guards are
insufficient when a Python callback, C extension, reentrant operation, monkey
patch, concurrency boundary, or other mutation can invalidate the assumption
after entry. The optimization must prove stability for the entire dependent
interval, invalidate optimized code, or revalidate before each unsafe use. If
invalidation occurs after visible effects begin, fallback is permitted only
when the exact semantic continuation can be reconstructed without replaying or
dropping those effects; otherwise the optimization must be rejected.

Whether an object is observable is an analysis fact, not a property hard-coded
into a particular optimization. Returning or yielding it, storing it into
escaping state, passing it to an unknown Python or C call, printing or
formatting it, taking its `repr`, checking its type, `isinstance`, or identity,
creating a weak reference, pickling it, or using otherwise observable protocol
operations cause the `MustEliminate` obligation to fail unless the observation
is preserved exactly. For example, `print(map(...))` must retain an observable,
CPython-compatible `map` object unless SOAC can preserve that observation
exactly; otherwise the complete dependent compatibility-relaxed bundle is
rejected. Unrelated, independently valid optimizations remain eligible.

Object-use facts should model allocation origins, aliases, capture and
ownership edges, returned aliases, identity observations, escape and
materialization boundaries, fields or activation slots, and call effects such
as borrow, capture, returned alias, or escape. Nonescape is a property of the
relevant ownership component, not merely a boolean on one allocation. A
`map`/`filter` stage captures its iterable, while its callback is an ordinary
fallible call that does not receive the iterator.

When no trusted call-effect summary exists, every object passed to the call is
treated as potentially escaping or observably used.

Virtual-object lowering consumes these facts independently. Ordinary
instances, builtin iterator wrappers, and generator activations should share
the same origin, alias, capture, identity, and escape model where possible.
Generator protocol analysis may add resume and preserved-slot facts, but must
not prove nonescape by naming instructions that a proposed rewrite intends to
delete.

Producer virtualization and consumer materialization are independent plans.
Eliminating an iterator or generator activation does not by itself authorize
replacing `list`, `tuple`, `set`, or `dict`; each consumer must preserve its own
ordering, callback, hashing, equality, replacement, exception, and cleanup
behavior unless an explicit compatibility policy says otherwise.

A guard miss must select the untouched implementation before any optimized
visible effect. Failure after effects have begun is allowed only when SOAC can
reconstruct the exact semantic continuation without replaying callbacks,
consumption, or other effects. Otherwise the optimization must not be selected.

## Generalization and benchmark specificity

The pyperformance suite determines priorities and measures results; benchmarks
must not be recognizable inputs to production optimization decisions.
Production eligibility must not depend on a benchmark file name, function
name, harness behavior, precomputed output, exact source bytes, or semantically
irrelevant constants used as a benchmark fingerprint. Semantically relevant
literal values remain valid optimization inputs.

Exact-source gates may be useful as temporary soundness scaffolding for an
experiment, but they may not remain as production eligibility and require a
documented replacement path. Benchmark-specific experimental substitutions
are disabled in the headline pyperformance score and reported only as
diagnostics.
Separately approved domain specializations must recognize domain semantics
rather than benchmark identity. Neither category demonstrates that a general
compiler mechanism works. Eliminating generator frames, for example, does not
justify replacing permutation enumeration with a specialized N-Queens bit-mask
search; that requires an independent semantic equivalence argument.

Classify optimization evidence as:

- **Benchmark-specific:** admission depends on benchmark identity, pinned
  source, or a replacement algorithm tailored to that program.
- **Domain-specific:** a semantic recognizer and equivalence argument apply to
  a problem domain independently of benchmark identity, but do not constitute
  a general compiler mechanism.
- **Mechanism-specific:** a focused slice demonstrates one reusable compiler
  mechanism.
- **General:** source-independent semantic facts select the transformation
  across multiple structurally different programs.

Claims of general progress should include multiple source shapes and an
opportunity census: candidates discovered, eligible, applied, rejected, and
the structured reason for each rejection. Also verify that the intended calls,
allocations, and virtual objects were actually eliminated.

## Correctness and safety

Unless an exception is explicitly approved below, optimized execution must
preserve CPython user-visible behavior, including values, encounter and
evaluation order, callback count and order, exception type, value, and raising
point relative to evaluation, callbacks, mutation, and other visible effects,
cleanup, `finally` and context-manager behavior, hashing and equality, generator
completion, and interaction with Python and C calls.

Correctness validation must compare complete observable results, not only a
proxy count or discarded value. For N-Queens this includes every materialized
solution tuple in encounter order, in addition to the total count. Generalized
iterator and consumer optimizations also require focused differential tests for
aliases, escaping objects, callbacks, exceptions, partial consumption,
shadowed names, mutated dependencies, guard failure, and untouched fallback.

No Python-accessible input or violated optimization premise may cause undefined
behavior, memory corruption, use-after-free, an interpreter-invariant
violation, native crash or abort, data race, or silent incorrect result. No
violated premise or compiler behavior may introduce a hang, deadlock, or
nontermination absent from the untouched CPython path. Failure has one of three
outcomes:

- an unsupported shape or unprofitable proposal is rejected, leaving the
  original path selected; a failed runtime guard executes that original path;
- violation of an explicitly enforced program contract raises its documented
  Python exception; or
- stale or invalid compiler metadata causes the optimized artifact to be
  rejected before execution.

## Compatibility policy

Compatibility relaxations are a first-class optimization tool. Agents should
actively and enthusiastically identify and propose narrowly scoped CPython
behavior changes when they plausibly unlock material performance. Do not
self-censor a useful proposal merely because it is incompatible.

Rarity and monorepo evidence inform a proposal's value, but do not by themselves
authorize it. Only explicit user approval authorizes a new production-visible
compatibility change. A new category must be approved and documented before
the optimizer relies on it; unapproved experiments must remain disabled by
default and excluded from headline results. A proposal should state:

- the blocked optimization and expected benefit;
- discovered and eligible opportunity counts, or an explicit statement that
  opportunity size is not yet measurable;
- affected behavior and an estimate of real-program prevalence;
- how the new contract is enforced or detected;
- the exception, fallback, or migration behavior on violation;
- which ordinary semantics remain preserved; and
- focused tests and performance evidence.

Every intentional divergence is recorded here as an approved policy and in
`doc/SPECIALIZATION.md` for the concrete specialization. The specialization
entry records its scope, proof and guards, preserved and changed behavior,
guard-miss behavior, and differential tests.

### Candidate enforced contracts

The following contracts are optimization directions, not facts that are true
today merely because they appear in this document. An agent may propose a
contract or build an experiment that is disabled by default, but may not enable
or rely on production-visible compatibility changes until the user explicitly
approves their enforcement and failure semantics and focused tests validate
them.

#### Runtime-enforced type annotations

SOAC may enforce selected type annotations as runtime program contracts. For
example, after enforcing `Foo.field: str`, optimized code may assume that a
present `field` contains an allowed `str` value. A violating value must raise a
documented Python exception at the enforced read, write, or other mutation
boundary before it reaches unsafe specialized code.

The proposal must define exact-type versus `isinstance` behavior, optional and
union types, subclasses, `Any`, missing or deleted attributes, descriptors,
and alternate mutation paths such as `object.__setattr__`, instance
dictionaries, serialization, and C extensions. An annotation alone does not
prove attribute existence, initialization, exact layout, or descriptor-free
access. Before publishing a typed-field fact, SOAC must prove that every
relevant Python and C mutation path is enforced; an uncovered path requires a
guard or makes the optimization ineligible.

#### Builtin namespace immutability

SOAC may enforce a contract that additions, replacements, and deletions in
`builtins.__dict__` are unsupported after a defined freeze point. The
implementation must define that point, the treatment of pre-freeze mutations,
whether direct C-API dictionary mutation is intercepted or guarded, and
whether an attempted post-freeze mutation raises, disables affected optimized
code, or fails in another explicit way. If a mutation path cannot be covered,
the contract is unavailable.

This contract does not freeze module globals, imported aliases, user
callables, class attributes, function code or defaults, or builtin type slots.
Lexical, local, and module-global shadowing must retain ordinary Python name
binding. A mutable dependency not covered by the enforced contract still
requires proof or a guard.

### Approved activation-introspection relaxation

SOAC may omit activation machinery eliminated by optimization and is not
required to materialize eliminated generator/frame objects, frame ancestry,
frame locals, traceback frames, or GC-discoverable activations solely for
back-door observation. An attempt to inspect unsupported activation state must
fail explicitly or select a compatible fallback; it must never silently return
incorrect or incomplete data as though the operation succeeded.

Tracing, profiling, `sys.monitoring`, debugger, and coverage support may likewise
be unsupported for eliminated activations, but attempts to enable or use those
features must fail explicitly, decline the optimization, or fall back to
compatible execution. SOAC must not silently omit callbacks or events that an
enabled feature is entitled to receive. Once callbacks can run, their count,
order, state mutations, exceptions, and other visible effects retain ordinary
CPython semantics.

This relaxation does not permit changes to ordinary computed values,
evaluation or callback order, exception propagation, cleanup, collection
order, or the semantic state needed to continue execution. If `locals()` or a
similar operation is unsupported in an optimized activation, optimization must
be declined or the operation must fail explicitly rather than return silently
incorrect data.

### Approved eliminated-internal-object relaxation

For an object satisfying `MustEliminate`, allocation, refcount, deallocation,
GC discovery, and allocation-failure timing may differ from CPython because
the object does not exist. This does not permit dropping a reachable user
finalizer, weakref callback, resource-release effect, or other ordinary
observation. Such behavior must either make the object observable and block
elimination or be covered by a separately approved compatibility relaxation.
Reference-count and lifetime effects on surviving user objects remain ordinary
observations and must be preserved unless separately approved.
