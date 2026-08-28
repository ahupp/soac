# Type-contract enforcement goal

## Current scope: interpreter enforcement, not optimization

Source-policy amendment — 2026-08-27 (PDT): select strictness through **SOAC
source comment blocks**, with no strictness configuration file. Both settings
default to false. `# soac: package(strict_assign=true, checked_attr=true)` in
`__init__.py` supplies inherited package defaults; a module-header
`# soac: module(...)` overrides those defaults for that file alone. A
`# soac: class(checked_attr=false)` (or `true`) immediately before a class's
decorators overrides that exact declaration. Omitted keys inherit. Nested
packages override only specified keys; module and class overrides do not flow
to child modules or lexically nested classes.

`strict_assign` selects post-initialization global-binding restrictions.
`checked_attr` selects automatic eligible class participation, including
supported annotated field writes and independent class/method-mutation
protections. They are independent: checked classes may live in a module whose
globals remain mutable. Opt-out never removes an inherited or installed
contract. Unsupported framework classes still fall back before irreversible
admission; no per-class annotation is required. Function-level value checks,
optimization and benchmarks remain out of scope.

The checker resolves these rules from original source, without executing it,
and authenticates every consulted package file and its absence. Source comments
request a contract; only authenticated startup and actual runtime binding
install one. Ordinary CPython ignores comments. The retired strict future is
not a source-policy opt-in, and `[tool.soac.strict]` is rejected rather than
combined with comment rules. Republish old artifacts: schema 7, strict-contract
version 3, dialect version 2 and deployment version 3 bind the new resolution
semantics. Never reinterpret an old publication or revoke a live contract.
This supersedes the earlier project-config/future double opt-in and separate
`checked_fields` switch below. Validate both ordinary and selected modules in
the single-file integration format, with analysis/import/setup failures outside
the final-statement-only `# raise` expectation.

Tracing, profiling and monitoring scope amendment — 2026-08-25 (PDT):
**CPython-compatible observation of SOAC execution is out of scope.** This
includes `sys.settrace`, `sys.setprofile`, `sys.monitoring`, corresponding
native observation hooks, and debugger/coverage behavior built on them, for
both retained compiled and entry-interpreter execution. Neither matching
observer events nor detecting and explicitly refusing unsupported observer
configurations is a deliverable or acceptance prerequisite.

Stop extending event emulation, observer reservations, enablement interception,
pre-entry refusal gates, fallback machinery or frame reconstruction solely to
provide that compatibility or guarantee a particular unsupported-feature
error. Audit and remove or simplify dedicated machinery. Keep a shared guard
only for a concrete, independently required memory-safety, authentication or
installed-contract invariant; name that invariant rather than observer fidelity.
Otherwise valid code must not require an observer-compatibility proof to run.

SOAC observer coverage may be unavailable or incomplete; do not advertise it
as CPython-compatible. Do not globally disable or alter ordinary CPython's
observers. This exclusion does not cover ordinary source callbacks, descriptor
or class-construction hooks, exception semantics, recursion safety, GC or
required cleanup. If any callback does run, its supported Python/C operations
must still obey installed contracts. Independently required mutation barriers
and actual-object/source authentication remain in scope.

Split mixed tests and retire SOAC observer-event parity and refusal-only
requirements, preserving in-scope semantic, safety and enforcement coverage
and ordinary-CPython controls. Missing SOAC events or an absent refusal are not
completion blockers under this amendment. This supersedes conflicting observer
requirements below and in the specification; it is not implementation evidence.
SOAC's own compiler counters, diagnostic logs and internal profile collection
are separate facilities, not removed by this observer-compatibility decision.
Optimization and benchmarks remain deferred until separately requested.

Traceback and frame-inspection scope amendment — 2026-08-25 (PDT):
**SOAC traceback reconstruction and frame inspection are out of scope for
this milestone.** This applies to retained compiled and entry-interpreter
execution, not only to activations eliminated by a future optimization.
Do not require synthetic CPython frames, matching traceback frames/ancestry
or instruction positions, materialized frame locals, or CPython locals-plus
slot correspondence as prerequisites for ordinary execution or type admission.

Stop extending source-lifetime frames, source-parent links, native frame-slot
projections, or helper-owned/omitted-slot inventories solely for those features.
Audit and remove or simplify dedicated machinery; keep shared components only
for a named, independently required semantic, ownership-safety or enforcement
purpose. Moving frame reconstruction behind another native API or proof does
not bring it into scope. Source identities and ranges needed for authenticated
construction and diagnostics remain required; reconstructing execution frames
from them does not.

Preserve exception types, values/identity, chaining, propagation and handler
behavior, as well as argument binding, lexical/comprehension scoping, source
evaluation and explicit callback order, suspension/resumption, `finally`,
context managers, safe ownership, required cleanup and installed contracts.
Unsupported SOAC frame inspection may fail explicitly; do not fabricate valid-
looking frame state. This amendment does not disable or change ordinary
CPython traceback/frame behavior, nor authorize a bypass through a supported
Python operation, specialized bytecode or C API. It does not request new
tracing, profiling, monitoring or debugger support.

Split mixed compatibility tests: retire SOAC traceback/frame-inspection-only
requirements while retaining semantic, safety and contract assertions and
ordinary-CPython controls. Their absence is not an admission failure or a
completion blocker. This amendment supersedes conflicting frame/traceback
requirements below and in the specification; it is a scope decision, not
evidence that the implementation has already been simplified or validated.

Field-assignment scope clarification — 2026-08-25 (PDT): remove **all
function-level runtime type enforcement** from this implementation. This
includes parameter and return checks on SOAC-compiled functions, retained
entry-interpreter functions, CPython-executed strict functions, and generated
dataclass constructors, including deferred default-factory result checks.
The earlier proposal to restrict call checks to SOAC-compiled functions is
also superseded: function-call type enforcement belongs to a future separately
specified layer, not this milestone.

Keep static `ty` analysis and its signature facts, ordinary Python argument
binding, source/function ownership needed for authenticated construction,
independent sealed-method metadata protections, and safe cleanup. None of
those facts grants a runtime parameter or return-type guarantee. Remove
consumers that assume such a guarantee without an independent runtime guard
or protected-storage premise. This is not permission to add check elimination
or another call-enforcement mechanism.

The runtime value invariant now concerns **selected field assignments**.
Check the value at the actual protected storage write, regardless of whether
the writer is SOAC, ordinary CPython, a generated constructor, or a supported
C API. An ordinary initializer may run its preceding effects before a bad
field write fails; do not impose an earlier argument check. A factory result
is checked only when it reaches selected protected storage, and an `InitVar`
has no field check merely because it is a constructor parameter. Source-
requested slots, escaped dictionaries, pending/final type construction and
the optional storage-owned `PyTypeState` protocol remain required.

This clarification supersedes conflicting function-boundary requirements
below and in earlier acceptance tests. Migrate code, policy/artifact versions,
tests and documentation together; retain ordinary call semantics and genuine
field/safety coverage. Do not clear live required flags or revoke installed
contracts in an existing process. Old authenticated publications must not be
silently reinterpreted as the new policy. The local implementation and selected
native runtime now remove the broader call checks; combined SOAC runtime and
compatibility validation remain required. This scope change alone is not
implementation evidence.

Scope amendment — 2026-08-23 (PDT): the current goal is the complete loop from
offline `ty` type extraction, through binding authenticated contracts to actual
Python runtime types and functions, to enforcing those contracts in the
interpreter. This supersedes the earlier optimization-driven implementation
phases and performance acceptance requirements. Do not pursue optimization
until the full selected interpreter-enforcement contract is implemented and
verified against the completion criteria below. Optimization then remains
deferred until explicitly requested again; completing enforcement does not
automatically start an optimization phase.

Runtime-state representation amendment — 2026-08-25 (PDT): participating
storage carries an **optionally allocated `PyTypeState *`**, with an audited
per-object bit indicating that the extra pointer slot exists. Ordinary objects
of the same Python type retain their ordinary allocation size: do not reserve
a null pointer in every `PyObject`, dictionary, or list. The pointed-to native
state owns the runtime checking operations and resolved rules; immutable state
may be shared, and nongeneric checks need no generic bindings.

Attach the state to the actual constrained storage. An escaped instance
dictionary must retain and enforce its field policy independently of the
instance. `PyTypeObject` still owns class-wide schemas, construction and
hierarchy restrictions, but is not a substitute for storage-local state.
Use a checked internal accessor, not a per-write identity-table lookup, for
the new direct-state path. Prefer a type-aware allocation trailer over a
variable prefix before `PyObject`; preserve existing object/GC header offsets
and allocator/free pairing. Reserve and initialize the slot during supported
fresh allocation, before publication or unchecked writes become possible.
The presence bit describes allocation layout, not merely active enforcement.

This representation change is in scope for the current interpreter-enforcement
milestone, beginning with its existing instance-storage checks. It is not
permission to implement generic containers/functions, tuple specialization,
closed-world hierarchy policy, new indexed field layouts, or guard elimination.
Arbitrary custom `__new__`/allocation paths are excluded from the initial new
allocation protocol; that exclusion must not weaken existing or inherited
contracts. Unsupported/pre-existing storage must not be silently moved,
retagged, copied, or made unprotected. Resolve such migration cases explicitly
while preserving the selected compatibility contract. See
[Optional storage-owned runtime type state](doc/TYPE_DRIVEN_OPTIMIZATION.md#optional-storage-owned-runtime-type-state)
for allocation, GC, freelist, construction and validation requirements. This
amendment specifies required work, not completed native implementation.

Execution-compatibility clarification — 2026-08-24 (PDT): SOAC is not required
to reproduce CPython's internal instruction schedule, transient reference
counts, or instruction-dependent timing/order of implicit reference release.
This is an approved compatibility difference, not an optimization to defer.
Preserve source-language semantics, ownership safety, required cleanup and
installed contracts; do not build bytecode-correspondence machinery solely
to match CPython's incidental execution details. See
[Approved SOAC execution-lifetime differences](#approved-soac-execution-lifetime-differences).
This clarification supersedes conflicting blanket requirements for exact
CPython reference-count and lifetime observations elsewhere in these documents.

Pending-type amendment — 2026-08-24 (PDT): bind a class contract to the actual
final decorated type. Full instance constraints may be installed after
decoration when an authenticated native pending barrier already prevents
instances before class callbacks. Reject both allocation and `__class__`
reassignment into a pending type before their effects, including through
supported C APIs and even for layout-compatible types. A fresh replacement
needs its own guard; an existing type cannot gain this guarantee retroactively.
Enable instances only after final validation and selected constraint
installation. This supersedes the earlier full-instance-contract-before-
callback requirement for this path, without relaxing selected method checks,
inheritance restrictions, dynamic-framework fallback or permanent contracts.
See [the pending-type protocol](doc/TYPE_DRIVEN_OPTIMIZATION.md#pending-types-and-final-adoption).
Implementation and compatibility validation remain required.

The required path is:

```text
Python source + resolved package/module/class comment rules
  -> offline ty analysis
  -> authenticated, versioned module/class/function contracts
  -> validation against the actual source, environment, and runtime objects
  -> native pending-type protection and final decorated-class admission
  -> selected module/class/field contract installation
  -> interpreter enforcement through supported Python and C entrypoints
```

This path must work with SOAC JIT execution disabled. It must not depend on
profile training, optimization-plan selection, check elimination, or generated
SOAC native code to establish or enforce a contract. CPython's existing
specialized bytecodes still need enforcement or a safe generic fallback: that
is correctness coverage, not a request to develop new optimizations.

### Current completion criteria

1. Export deterministic, versioned, authenticated `ty` contracts without
   executing analyzed modules. Bind source identities, checker/policy versions,
   dependencies, and the interpreter environment; reject invalid, stale,
   forged, or incomplete mandatory artifacts before user code runs.
2. Resolve logical type references to the actual Python objects. Pass explicit
   authenticated pending construction state into the real type allocator
   before class callbacks. Reject instance admission, including allocation and
   `__class__` reassignment into pending types, until the actual final decorated
   type is validated and its selected constraints are installed. Names,
   annotations, matching dictionaries, and a successful checker run are not
   runtime authority.
3. Enforce the selected module, class, field, and method-mutation policies
   across Python operations, CPython generic and specialized paths, and
   supported C APIs. Remove function-level runtime parameter/return checks
   while preserving ordinary argument binding, call behavior and independently
   required ownership/metadata protections. `checked_attr` selects class and
   supported field checks independently of `strict_assign`. Rejection must
   precede the forbidden storage mutation, not
   earlier constructor effects.
4. Preserve ordinary dataclass behavior, descriptors, dictionaries, requested
   slots, callbacks, object lifetime, and ordinary-Python interoperability
   subject to the approved strict-language and execution-lifetime differences.
   SOAC traceback/frame inspection and tracing/profiling/monitoring compatibility,
   including mandatory observer refusal, are excluded by the 2026-08-25 (PDT)
   amendments; exception semantics and ownership safety are not.
   Unsupported framework classes automatically remain dynamic before any
   irreversible class contract is installed; the surrounding strict module stays
   strict.
   Never revoke or weaken an already published contract.
5. Add end-to-end behavioral and structured contract tests covering the real
   checker, authenticated loader, actual runtime objects, and interpreter
   storage boundaries. Include ordinary unchecked-call controls, negative
   admission tests, native C API tests, warmed bytecode tests, dataclasses, and
   automatic framework fallback. Run
   `just test-all` before submitting implementation changes; documentation-only
   edits retain the normal exemption.
6. Implement the optional storage-owned type-state representation for the
   selected existing instance-storage enforcement paths. Prove that ordinary
   allocations reserve no extra pointer, protected storage uses the direct
   accessor, escaped dictionaries retain their constraints, and allocation,
   GC, failure cleanup and freelist reuse preserve both layouts. Record any
   remaining legacy-policy migration paths rather than claiming conversion
   complete while required paths still use them.

Stable indexed dictionaries, compact layouts, new virtual/direct dispatch,
inlining, trusted unchecked entries, proof propagation and check elimination,
constructor/object virtualization, and other type-driven JIT transformations
are not deliverables for this scope. Nor are pyperformance speedups, profile/
apply cycles, JIT hot-path coverage, IR/native-code size targets, or performance
log entries acceptance gates. Ordinary source-requested `__slots__` and the
metadata needed to enforce their writes remain in scope; new layout
optimizations do not. The optional type-state allocation specified above is
an explicit enforcement-representation exception, not an indexed-layout or
performance milestone.

Use CPython's ordinary closure and activation machinery for the interpreter
path. New SOAC closure layouts, suspended-state ABIs, or native binding/lifetime
recipes are not prerequisites unless a concrete enforcement or compatibility
requirement demonstrates otherwise. Such necessary work must name that
requirement and its focused regression test, not a future optimization benefit.
Matching CPython's internal reference counts, instruction-level release
schedule, traceback frames or inspectable frame layout is not such a
requirement. Audit and remove or simplify machinery
whose sole purpose is that matching; retain shared components only for an
independent, in-scope semantic, safety, interoperability or enforcement need.

Existing implementation and historical benchmark evidence are not deleted by
this scope change. Any retained execution path must still honor installed
contracts. The optimization sections below are deferred design and measurement
reference, not instructions to continue optimization or blockers on completing
this milestone. The correctness and selected compatibility policies remain
normative; optional optimization capabilities are required only when actually
installed or consumed. The detailed current phases and tests are in
`doc/TYPE_DRIVEN_OPTIMIZATION.md`.

### CPython source history — 2026-08-23 21:35 PDT

Maintain SOAC's CPython changes as individual, logically scoped commits in the
CPython repository, not as a standalone patch series applied to a pinned base
checkout. SOAC must pin the resulting CPython commit through its submodule
gitlink. A clean checkout at that commit must contain the required native
sources without a separate patch-application step. Keep regenerated bytecode
and other generated files in a separate commit at the top of the native stack,
with the regeneration commands in its commit message.

The main implementation thread should migrate the existing patch series to
this committed history without losing source changes, tests, or provenance.
Verify equivalent source contents before switching, establish reproducibility
from a clean checkout, and update source preparation,
build selection, provenance validation, relevant tests, `AGENTS.md`, and
`README.md` consistently. Do not keep standalone CPython patches as a second
maintained source of truth.

Follow-up instructions — 2026-08-23 (PDT): apply the same committed-history
model to the Ruff/`ty` changes. Vendor Ruff as a Git submodule at `vendor/ruff`
using `https://github.com/adamh-oai/ruff.git`, pin its actual committed source,
and replace the maintained checker patch-application distribution. Preserve
matched runtime Ruff dependencies, checker/exporter fingerprints, generated
lockfile separation, and fail-closed source/build validation. Unvalidated
checker experiments remain separate from the currently validated generation.

Keep both repositories' new commits local for now; do not push them. Verify
independent local checkout reproduction and explicitly record remote checkout
and CI availability as deferred until publication is separately authorized.
A local submodule pin is not a claim that a fresh remote checkout can fetch
that commit. Delete the maintained patch files only after the corresponding
committed history, local pin, and preparation/build/provenance migration have
been verified; retain historical migration evidence without maintaining two
source representations.

Perform migration in a separate staging checkout and coordinate promotion with
active native consumers; do not rewrite the live source during a build or use
new headers with an old library. Keep CPython source on the shared host/guest
mount, use persistent guest-local build directories, and retain fail-closed
source/revision/build checks. The current patch-based workflow remains the
description of the existing tooling until that coordinated migration lands;
this dated instruction supersedes it as the target architecture, not as a
claim that migration has already happened.

## Deferred optimization objective

If optimization is separately resumed, its measurable goal is to optimize
explicitly opted-in strict Python modules on the pyperformance suite. The success target
is to beat the same stock CPython by at least 10% on the geometric mean of a
benchmark set fixed before the comparison. Stock CPython runs the original
ordinary-Python workload; SOAC runs the equivalent workload with its chosen
modules explicitly opted into the strict language. Define the score as
`stock elapsed time / strict SOAC apply elapsed time`, or equivalently
`strict SOAC apply throughput / stock throughput`; the target score is at least
`1.10`. A full-suite aggregate is incomplete when any comparable result is
missing or failed; an intersection aggregate may be shown only when explicitly
labeled. Report every per-benchmark ratio, execution coverage, and whether the
benchmark's meaningful hot code was actually transformed alongside the
aggregate.

Strict modules are the sole SOAC optimization target. Their enforced language
contract can make globals, class layouts, and callable dispatch stable enough
to replace ordinary-Python speculation with proven structural facts. Ordinary
Python remains the stock comparison and interoperability boundary; optimizing,
benchmarking, or preserving a separate ordinary-SOAC execution lane is not a
project goal.

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

## Measurement model (deferred)

Headline performance is the normally trained strict
`SOAC_OPT_MODE=apply` pass, measured without an attached native profiler and
compared with the same vendored stock CPython running the original workload.
Compare the changed strict SOAC revision against stock CPython and, when
available, the previous strict SOAC revision. Use identical inputs, affinity,
clock policy, benchmark variants, algorithm, and module-selection policy;
generate fresh profile evidence independently for each strict SOAC revision.
A profile pass supplies the optimization evidence required by apply. A verify
pass is optional diagnostic evidence for checking whether expected paths,
guards, and fallback counters were actually exercised; profile, verify, and
unspecialized throughput are never the headline result.

Use separate native `perf` captures, including JIT-symbol attribution, to find
and explain hot paths. Pyperformance measures the outcome; `perf`,
generated-IR inspection, and JIT code summaries explain it. Run
`just pyperformance-compare` for repeatable stock-versus-strict-SOAC
measurements and comparison against an available prior strict SOAC result.
Extend that recipe to select and identify the explicitly opted-in source if
its current workflow does not yet support strict execution; an ordinary SOAC
run is not a substitute. The recipe defaults to the `chaos` fast workload, so
pass the full target benchmark set when claiming the overall goal. Final
performance claims should use pyperformance's
statistical comparison across at least three independently started,
order-alternated comparisons. A delta within measured noise is inconclusive
rather than a win or regression.

### Stock baseline and strict SOAC

The required comparison has exactly two language configurations:

```text
stock Python:  original ordinary-Python workload
strict SOAC:   equivalent explicitly opted-in workload
```

Report strict-SOAC/stock and, when available, changed-strict-SOAC/
previous-strict-SOAC. State when a previous strict implementation does not
exist; do not add an ordinary-SOAC baseline. List every opted-in or otherwise
modified module, the source changes, enabled language extensions, and actual
transformed hot paths. Keep workload behavior, inputs, variants, algorithm,
and module-selection policy equivalent; strict-only source changes authorize
the declared language contract, not benchmark-specific algorithm
substitutions.

Generate independent profile/apply evidence for each strict SOAC revision. Keep
source identities, strict contracts, dependency fingerprints, caches, and
measurements separate across revisions. Disclose the explicit source-policy
comments in the strict overlay, even though ordinary CPython ignores them;
preserve every other workload source byte. This
strict-versus-stock result is the primary `1.10` acceptance score.

For each retained performance change, report at least:

- the approved strict contracts and every opted-in or modified module;
- per-benchmark strict-SOAC/stock and, when available,
  changed-strict-SOAC/previous-strict-SOAC performance;
- the fixed benchmark set, strict-versus-stock geometric-mean aggregate, and
  whether the full-suite result meets the `1.10` target;
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
measured loop. A run with no authenticated, sealed strict module on its
meaningful hot path cannot establish strict optimization progress, even when
the benchmark completes. Keep module-selection policy explicit and inspect
strict-module provenance, hot-path samples, and generated-code evidence before
drawing either conclusion. For example, a strict `chaos` configuration must
opt its benchmark classes and functions into strict mode; imported
standard-library `math` and `random` can remain on stock CPython.

## Record every optimization strategy (deferred)

Maintain one tracked Markdown file per attempted optimization strategy under
`doc/optimization-attempts/`, using the Pacific-date filename convention and
template documented there. Create the file when a strategy is selected and
update that same file across baseline collection, implementation experiments,
repeated measurements, and the final decision. Do not create a new file for
each benchmark run or discard the record when experimental code is reverted.

Each strategy file must record:

- the hypothesis, expected general-purpose benefit, and hotspot or structural
  evidence motivating the strategy;
- what implementation was attempted, what changed between iterations, and the
  relevant CPython-compatibility assumptions, guards, fallback, and tests;
- the strict optimization mechanism, opted-in source changes, enforced
  contract, stock comparison, and available previous-strict-SOAC results;
- the fixed benchmark selection, measurement protocol, baseline revision or
  result, stock CPython result, previous-strict-SOAC result when available,
  and each measured candidate result or explicit reason a value is unavailable;
- benchmark completion and actual transformed project, dependency, and
  standard-library coverage, including failures and unoptimized hot paths;
- available optimized typed-IR counts, pre-optimization BlockPy size, native
  code bytes, machine-block counts, and material startup or compilation costs;
- every negative, rejected, failed, or inconclusive attempt, its quantitative
  evidence where available, and the technical reason it was not retained; and
- the current status, final verdict, transferable lesson, and next action.

Keep bulky generated artifacts under ignored `work/`; copy the essential
measurements and conclusions into the tracked strategy file so rejected work
remains understandable after artifacts disappear. `doc/PERF_LOG.md` remains a
concise summary of finalized retained performance changes; it does not replace
per-strategy attempt history.

## Profile and apply phases (deferred)

Optimization selection must not adapt dynamically from observations made by
the optimized process. A profile process measures a workload, then a restarted
apply process uses that evidence to construct and validate optimization plans.
The cycle may be repeated deliberately. Code generation may still occur when
the apply process starts or loads a function, and optimized code may contain
entry guards, fallbacks, and supported deoptimization paths.

Restrict profiling, optimization-plan selection, and optimized JIT admission
to individually authenticated strict code objects/functions executing under
their valid strict capability, or genuinely compiler-owned intrinsic
operations. Module membership, shared strict globals, and a copied future flag
are insufficient. Ordinary dynamically compiled code and annotation replay
against synthetic globals remain unoptimized even when associated with a
strict module; seal-dependent facts require an actually sealed producer. An
ordinary module executes through its stock-compatible path; calling an
optimized strict function from ordinary Python is still permitted through the
function's public boundary. Compiler-owned runtime helpers need explicit
intrinsic provenance or their own strict contract, not blanket optimization
eligibility based on an ordinary module name.

Optimized strict plans and guarded dynamic-dependency assumptions may be
invalidated or replanned. This does not permit revoking, unsealing, or silently
weakening an enforced strict module: its language contract remains valid for
the module's entire live lifetime.

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

## Optimization structure (deferred)

Analyses, decisions, and transformations should be independent components with
explicit inputs and outputs. The pipeline below describes an architectural
direction, not a claim that every fact cache, speculative overlay,
transactional commit, or prioritized worklist already exists. Improve the
current production path in the smallest sound, measurable step; do not rewrite
the optimizer merely to implement aspirational infrastructure before a concrete
optimization needs it:

```text
profile evidence + static program facts + enforced language contracts
  -> guarded or contract-proven candidate decisions
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

Strict execution adds policy facts only after the actual module, class,
function, receiver, and dependency have established their authenticated runtime
contracts. Examples include a sealed final global and its stable binding index,
a stable indexed-dictionary field or verified native member offset, a
class-default override field and its frozen default owner, a frozen callable
target, an independently assigned method/vtable slot, and a policy-checked
value. Encode them
in resolved typed IR or a validated sidecar; do not infer them from source
spelling, a single observed assignment, annotations, profile observations,
`__module__`, or an `isinstance` check against a strict base.

A final present binding is stable, but an absent name is not: append-only
strict globals can appear later and shadow a builtin. Mutable globals, mutable
ordinary dependencies, stock subclasses, unknown receivers, callable class-data
shadow fields, and unsupported descriptors still require their actual checks or
generic fallback. Frozen builtins require their own separately approved and
enforced contract; checked field values require the declaring class's resolved
`checked_attr=true` rule and complete write enforcement described below.
Dictionary presence does not
preclude protected methods or fixed indexes, and nominal type acceptance does
not imply exact representation or a participating strict receiver.

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

A rewrite that preserves the enforced strict-language contract may commit
independently. Ordinary modules and objects retain CPython behavior when
crossing a strict boundary, but do not constitute a separately optimized SOAC
language. A rewrite that exposes a compiler-owned object or operation whose
surviving behavior differs from the strict contract is compatibility-relaxed
and must be analyzed speculatively on an immutable projected view, overlay,
or clone of the IR.

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

## Generalization and benchmark specificity (deferred)

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

Strict interpreter execution and any future optimized execution must preserve
the selected strict-language contract and all CPython behavior that contract does not
deliberately change. Ordinary modules and objects retain their normal CPython
behavior as interoperability boundaries; optimizing their separate execution
is not required. Outside the selected strict-language differences and the
specifically approved relaxations below, preserve values, encounter and evaluation
order, callback count and order, exception type, value, and raising point
relative to evaluation, callbacks, mutation, and other visible effects,
cleanup, `finally` and context-manager behavior, hashing and equality,
generator completion, and interaction with supported Python and C calls.

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

The current enforcement milestone implements the already selected language
contract; it does not seek additional compatibility relaxations to unlock
performance. If optimization is separately resumed, narrowly scoped behavior
changes may be proposed under the approval process below.

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

### Selected explicitly opted-in strict-module contract

The request to implement `doc/TYPE_DRIVEN_OPTIMIZATION.md` authorizes the
strict-language contract specified here and in `doc/STRICT_MODULES.md`,
including the selected field-write and protected-name policies below.
Implementation is in progress: this approval does not claim that enforcement,
compatibility tests, or current acceptance criteria have been completed. Before
enabling or relying on any capability, implement every required boundary and validate the
actual behavior. New differences beyond this selected contract still require
explicit approval. Ordinary modules do not acquire strict semantics merely by
being transformed, imported by a strict module, or observed to have a convenient
source shape.

The selected strict contract requires:

- source-comment package/module/class rules and fail-closed authenticated
  runtime support for the selected contracts; `strict_assign` and
  `checked_attr` are independent and both default false;
- append-only module bindings when `strict_assign=true`; previously absent names
  may be added and immediately become final, while rebinding/deletion is
  permitted only for original statically declared `global NAME` bindings;
  stable indexes are an optional storage capability, not an enforcement goal;
- automatic class-capability selection from source-authenticated offline `ty`
  contracts and actual runtime construction under `checked_attr=true`, without
  manual per-class SOAC
  annotations; module membership, frozen classes, checked values, method
  dispatch, and physical instance storage remain independent;
- real ordinary instance dictionaries, including ordinary dataclasses, with
  preserved contents, identity, and supported mutation behavior; fixed field
  prefixes and dynamic overflow are deferred optional storage capabilities;
  actual source-requested slots retain their native layout and write policies,
  rather than inferred or implicit `__slots__`;
- initially absent instance overrides for eligible annotated or unannotated
  plain class-data defaults, preserving actual class bindings, visible
  dictionary contents/order, actual dictionary replacement identity, and
  exactly one owned reference per value;
- protected effective method/`ClassVar` names that reject instance attribute
  writes while permitting same-name dictionary entries which every generic,
  native, and specialized attribute/method lookup ignores; genuine declared
  fields precede inherited non-data methods, never actual data descriptors;
- the explicitly approved destruction-order difference described below only
  when a fixed-layout capability is actually installed; this does not require
  developing fixed-layout storage for the enforcement milestone;
- the separately approved SOAC execution-lifetime differences, which do not
  require fixed-layout storage or any new optimization capability;
- frozen participating classes and owned dispatch-relevant callable metadata,
  with automatic dynamic fallback for unsupported metaclasses, decorators,
  descriptors, and framework-managed classes before irreversible construction;
  optional source opt-outs are not required to keep unsupported classes usable;
- actual stdlib dataclass options and behavior, including ordinary dictionaries,
  source-requested `slots=True` replacement construction, distinct linked
  original/replacement handles, callback-visible original class objects with
  pending instance admission rejected, generated functions, descriptors,
  frozen-instance behavior, and closure-cell repair;
- independently selected instance-field write checks, disabled by default,
  and ordinary function/constructor calls on every backend: parameters,
  return values, factory outputs and `InitVar` are not runtime type-checked;
  lazy annotations, ordinary subclass interoperability and generic behavior
  remain wherever no capability applies; and
- a permanent live strict contract: forbidden mutation fails explicitly and
  never downgrades a sealed module to ordinary mutable execution.

The current milestone retains mandatory checks. For any separately resumed
optimization, only strict-to-strict operations with verified actual operands may
omit the corresponding ordinary global, owner, layout, method, or target guards. Omit an
individual value check only with a valid dominating proof; a nominal accepted
subclass does not imply strict layout or dispatch. A strict local reference to
a stock object does not freeze it. The selected contract does not authorize
checks for unsupported annotation forms, frozen process builtins, changed
integer overflow, bypassing actual data descriptors, memory leaks, unsafe
reference handling, suppressed finalizers, or source-level callback-order
changes. Implicit release timing/order may differ only under the approved
execution-lifetime and field-release policies. SOAC tracing, profiling,
monitoring and associated debugger/coverage compatibility are excluded by the
2026-08-25 (PDT) observer amendment, including any mandatory refusal guarantee.

The real type allocator must receive an explicit, interpreter-owned, single-use
construction handle bound to the authenticated module execution, lexical plan,
namespace function, actual metaclass, and transformation phase. Install the
actual requested storage and native pending-instance barrier before
`PyType_Ready`, `__set_name__`, or `__init_subclass__`; callbacks may retain the
class but cannot create instances or reassign an object's `__class__` into it.
Validate the final decorated type and install its selected constraints before
enabling instances. A mutable Python helper,
namespace attribute, matching name, or TLS value is not authority. Repeated
factory executions and dataclass replacements receive separate handles and
runtime identities. After permanent constraints are installed, a mismatch
must reject or decline only additional unpublished capabilities, never restore
unrestricted dynamic behavior. Frozen/dispatch facts appear only after final
decorator adoption and successful sealing, including for classes created after
their module has already sealed.

Exact authoritative module/class/keyword-default dictionaries require the
explicit narrowed native boundary in `doc/STRICT_MODULES.md`. Supported
mutations such as `PyDict_SetItem`, deletion/merge APIs, generic attribute
setters, and function metadata setters must reject forbidden changes before
mutation. Immutable authoritative dictionaries do not support native
`void PyDict_Clear` or equivalent non-rejectable mutation. Silently ignoring
that API, leaving a pending exception behind it, or revoking sealing is not a
solution. Python `dict.clear()` must reject before reaching it. Ordinary mutable
instance dictionaries may clear values while preserving their protected
schema. Semantics-preserving `PyFunction_SetVectorcall` changes remain
implementation state, not a mutation loophole for frozen function semantics.

Prioritize the current implementation in dependency order:

1. Make authenticated strict admission and execution work in the interpreter
   with SOAC JIT execution disabled. Do not route this path through legacy
   unverified optimization assumptions. Ordinary modules use stock-compatible
   execution and remain interoperability controls.
2. Complete offline `ty` export with matched Python 3.15/strict-dialect support,
   conservative narrowing, versioned module shards, shared policy diagnostics,
   and source/config/dependency/environment fingerprints. Preserve authenticated
   proposals through native compilation, module loading, and cache identities;
   keep checker work out of runtime imports.
3. Bind proposals to actual Python types/functions and pass each participating
   class's authenticated pending construction state into the actual type
   allocator before callbacks. Validate the final decorated type and install
   its selected constraints before instance admission. Preserve requested
   dictionaries, slots, weakrefs, and defaults. Select dynamic fallback before
   installing irreversible class restrictions.
4. Complete module/class/function and instance read/write barriers, including
   actual dictionaries, member descriptors, replacement, supported native APIs,
   and warmed CPython bytecodes/tier-specific paths. Follow the authenticated
   initialization/sealing lifecycle and publish `SEALED` only after all required
   enforcement boundaries have succeeded.
5. Enforce opt-in field writes and remove all function-level runtime type
   checks and dependent call proofs. Preserve ordinary argument/result
   behavior without introducing check elimination. Unsupported types and nonparticipating
   classes retain the documented dynamic behavior; already installed inherited
   restrictions still apply.
6. Complete the standard-dataclass adapter and framework fallback compatibility
   matrix through actual interpreter execution, including generated functions
   and source-requested replacement classes.
7. Run the end-to-end behavioral and structured contract tests and
   `just test-all`. Report supported contracts, rejected cases, dynamic fallback,
   and unresolved compatibility gaps. Performance measurements and optimized
   IR/code-generation assertions are not required for completion.

Do not retain profile-guided split-dictionary owner/version machinery merely
to optimize ordinary Python. A verified storage capability proves a field
location; presence, value type, descriptor behavior, and write policy remain
independent proof obligations. Interactions with ordinary objects use correct
generic CPython behavior unless a boundary specialization measurably improves
a strict workload and remains sound. Exact runtime value types, closures,
ownership, finalizers, recursion, generator observability, builtin mutation,
and ordinary boundary behavior do not become static merely because their
containing module is strict.

### Selected checked-value contract

The 2026-08-25 (PDT) field-assignment clarification selects storage checks only.
The 2026-08-27 (PDT) source-policy amendment selects eligible classes and their
supported annotated writes with `checked_attr=true`, independently of
`strict_assign`. Both flags default false; there is no separate checked-fields
configuration. Runtime parameter/return policy knobs and their failure policies
are removed, not retained as disabled aliases. Unsupported field types remain
dynamic; selected mandatory checks fail with `TypeError` before the bad value is stored.
The offline analyzer, runtime and cache fingerprints consume the same normalized
policy, independent of warmup, profiles, optimization level or inlining. No
per-class SOAC annotation is required.

Function annotations remain static checker facts. Calls through SOAC, the entry
interpreter, CPython and supported C APIs preserve ordinary argument binding,
defaults, variadics, body results and exceptions without adding runtime type
checks. A called function may still fail at a protected field write. Earlier
body effects and valid preceding writes are not rolled back or moved merely
to reject an argument sooner. The same rule applies to dataclass initializers;
`InitVar` and factory outputs do not create separate call-type obligations.

The normalized runtime type contract defines:

| Type form | Selected acceptance and proof boundary |
| --- | --- |
| Nominal builtin type | Genuine instances and subclasses of the supported builtin, tested from actual type identity/MRO rather than overridable Python membership hooks. |
| `int` | Includes `bool` and genuine `int` subclasses; does not prove an exact builtin or machine-width range. |
| `float` | Includes genuine `float` and `int` instances/subclasses, including `bool`, following numeric widening without conversion; does not prove exact float representation. |
| Resolved nominal class | Accepts the genuine class and ordinary subclasses under verified nominal membership; ordinary subclasses still need generic layout/method handling. |
| `None` | Accepts only `None`. |
| Fully supported union or `Optional[T]` | Accepts the alternatives in the normalized contract; `Optional[T]` includes `None`. |
| `Any`, `Unknown`, unresolved references/imports, unsupported generics/containers/protocols or annotation expressions | No mandatory check or trusted typed-value fact for that unsupported value contract; an unsupported union member makes the whole union dynamic. |

The versioned artifact must explicitly enumerate supported builtin identities
and accepted alternatives; unsupported builtin forms remain dynamic. Custom
metaclass `__instancecheck__`, ABC virtual registration, structural proxies, and
arbitrary annotation evaluation cannot establish nominal native proofs. The
runtime must not eagerly evaluate lazy annotations, type aliases, or providers
to recover offline facts. Resolved class targets must be authenticated actual
objects, not a matching `__module__`/name or an evaluated string annotation.

Generators, coroutines and async generators likewise acquire no runtime
argument, yield or return-type contract. Preserve their ordinary suspension,
await, cancellation, ownership and object-identity behavior. Writes to protected
fields remain checked regardless of which function kind performs them.

Checked fields, when selected, require a protected write contract separate from
their physical storage. Cover assignment/deletion, `setattr`,
`object.__setattr__`, member-descriptor setters, every supported dictionary
mutation and whole-dictionary attachment, deserialization, generated
constructors, CPython adaptive/tier-specific stores, supported C APIs, and
SOAC's own raw stores. Preserve missing/deleted-field errors, class defaults,
data-descriptor behavior, framework validation/coercion order, and reference
ownership. A field annotation alone proves neither presence nor exact layout,
representation, or descriptor-free access. An uncovered mutation path requires
generic/load-time guarded behavior and forbids a protected checked-field fact;
a watcher cannot substitute for rejection before mutation.

Check elimination is deferred, not part of the selected enforcement milestone.
If separately resumed, individual checks may be eliminated only when validated
typed IR has an independently valid protected-field fact or explicit runtime
guard. Static parameter/return annotations and an ordinary completed
constructor are not such proofs. Distinguish nominal acceptance from exact builtin and
participating-receiver capabilities. Recheck after effects that can invalidate
the premise, including ordinary `__class__` changes, untrusted callbacks, and
mutable closure state. Every virtual target admitted to a trusted-return path
must check or prove its actual result; a base annotation does not constrain an
unchecked override. Direct calls retain the callee's actual environment,
argument binding, ownership, exceptions, and any checks lacking such proof.

Required evidence includes focused Python/C mutation tests, warmed ordinary
callers and adaptive attribute paths, field-write error/side-effect order,
ordinary call controls, subclasses, supported/unsupported field unions,
numeric widening, lazy providers, and structured positive/negative assertions
on normalized contracts, actual type bindings, and installed runtime policies.
Optimization-plan and check-elimination assertions belong to deferred work.
Successful type checking alone is not evidence that this runtime contract holds.

### Candidate additional enforced contracts

The following contracts remain optimization directions, not facts that are true
today merely because they appear here. An agent may propose one or build an
experiment disabled by default, but new production-visible differences require
explicit approval of enforcement/failure semantics and focused validation.
Checked containers, suspension-aware generator/coroutine checks, and new
unboxed ABI/coercion semantics are not included in the selected policy above.

#### Per-strict-module frozen builtin mappings

SOAC may offer a separately opted-in strict-module contract that captures a
protected per-module builtin dictionary before creating that module's
functions. Its implementation must define snapshot timing, pre-snapshot
mutations, function capture, supported Python/C dictionary mutation paths,
and the explicit failure behavior for attempted changes to the protected
mapping. If a mutation path cannot be covered, that module cannot publish a
frozen-builtin fact.

The process-wide `builtins.__dict__` remains mutable. Ordinary Python and strict
modules without this additional opt-in continue observing their actual live
builtin mappings. The per-module snapshot does not freeze module globals,
imported aliases, user callables, class attributes, function code or defaults,
or builtin type slots. Lexical, local, and append-only module-global shadowing
retains its ordinary precedence; a newly appended strict global can still
shadow a frozen builtin. Every mutable dependency outside the protected
snapshot still requires its own proof or guard.

### Approved SOAC execution-lifetime differences

Clarification — 2026-08-24 (PDT): a SOAC function executes according to SOAC's
own IR and calling convention. Interoperation with CPython requires a sound
argument/result, ownership and exception boundary, not instruction-by-
instruction correspondence with CPython's separately compiled version of the
function.

SOAC need not match `sys.getrefcount()` results, CPython's temporary ownership
counts, borrow/duplicate/move choices, fused-opcode schedules, or the precise
timing and relative order of finalizers and weak-reference callbacks caused
by implicit reference release. Compiler-owned temporary references may keep
values alive until a SOAC-selected safe cleanup point. This permission applies
to retained SOAC compiled and entry-interpreter paths; the CPython backend
continues to use CPython's ordinary ownership and activation machinery.

Outside those permitted observations, preserve computed values,
identity/aliasing semantics, lexical and comprehension scoping, source
evaluation and explicit callback order, argument binding,
exceptions, `finally`, and context-manager behavior. Own borrowed values for
as long as they are needed and retire owned references on normal and error
paths. No leaks, indefinite retention of dead temporaries, use-after-free,
double release, suppressed required finalizers/weak-reference callbacks, or
lost resource-release effects are permitted. GC, reentrancy and resurrection
must remain safe. The 2026-08-25 (PDT) amendments exclude SOAC traceback/frame
inspection and tracing/profiling/monitoring compatibility, including mandatory
observer refusals. Do not add frame reconstruction or refusal machinery for
those features. Ordinary CPython behavior remains unchanged.

Do not require native opcode/lifetime recipes, a parallel token execution ABI,
or compiler lifecycle proofs solely to reproduce the excluded observations.
Moving that same requirement into a new native metadata API does not make it
in scope. Metadata independently needed for authenticated source/object
binding, diagnostic source locations or actual enforcement remains valid, as do
barriers covering CPython's own generic and specialized bytecodes.

Compatibility tests must distinguish required semantics and ownership safety
from the excluded exact-count/micro-order observations. Split mixed tests to
retain their meaningful checks; revise or retire assertions whose sole purpose
is matching CPython's internal schedule. Do not hide unrelated failures or
drop ordinary-Python controls for behavior that remains required.

### Approved strict-instance field-release-order relaxation

When an explicitly opted-in, fixed-layout strict instance is deallocated or
cleared by cyclic garbage collection, its occupied public, private, and
inherited fields may release their references in deterministic physical-layout
traversal order. The same exception applies to strict-layout fields physically
inherited by an ordinary or dynamic subclass; it does not grant that receiver
strict optimization facts. The resulting order need not match the field-release
order of an equivalent ordinary Python instance dictionary, whose shared-key
layout can also differ from its visible insertion order.

Consequently, `__del__` methods and weak-reference callbacks triggered by those
field values becoming unreachable may execute in a different relative order
and observe different sibling fields already cleared. This is an approved
strict-language behavior difference; no per-instance dictionary-order tracking
is required solely to reproduce stock destruction order.

Each populated field must still be cleared before its exactly-once `DECREF`,
and all finalizers, weak-reference callbacks, resource-release effects,
exceptions, cycle handling, reentrancy, and resurrection must remain valid.
The exception does not authorize skipped or duplicated callbacks, changes to
ordinary field stores/replacements/deletions, changes to the receiver's own
finalizer or weak-reference phases, independently reordered cleanup of other
objects, altered module/class/function teardown, or broader monitoring,
generator, evaluation-order, or lifetime differences.
The separately approved SOAC execution-lifetime policy above supplies its own
limited permission; this field-layout exception is not its justification.

### Approved activation-introspection relaxation

The 2026-08-25 (PDT) scope amendment above excludes SOAC traceback reconstruction
and frame inspection throughout the current milestone, whether or not an
activation was eliminated. The narrower optimization-oriented permission
below is not a reason to retain or extend that machinery now. Explicit
unsupported-operation refusals must not become a blanket refusal of ordinary
code merely because CPython frame-layout correspondence is unavailable.

The former optimization-only permission to omit eliminated activation frames
is superseded by that broader exclusion. Do not retain frame reconstruction,
frame-layout correspondence, inspection detection, or refusal/fallback
machinery solely for excluded SOAC frame observations. Ordinary CPython frame
behavior remains unchanged; incomplete SOAC inspection is not supported
compatibility.

The 2026-08-25 (PDT) observer amendment excludes tracing, profiling,
`sys.monitoring`, debugger and coverage compatibility for all retained SOAC
execution, not only eliminated activations. It does not require event parity,
observer detection, explicit refusal or compatible fallback. Preserve ordinary
CPython observers and all independently required safety/enforcement barriers;
do not represent incomplete SOAC event coverage as supported compatibility.

This relaxation does not permit changes to ordinary computed values,
evaluation or callback order, exception propagation, cleanup, collection
order, or the semantic state needed to continue execution beyond an
independently approved compatibility contract such as strict-instance
field-release order. Explicit namespace operands, lexical binding and ordinary
source operations still require their own semantics; they do not require
reconstructing an inspectable CPython frame.

### Approved eliminated-internal-object relaxation

For an object satisfying `MustEliminate`, allocation, refcount, deallocation,
GC discovery, and allocation-failure timing may differ from CPython because
the object does not exist. This does not permit dropping a reachable user
finalizer, weakref callback, resource-release effect, or other ordinary
observation. Such behavior must either make the object observable and block
elimination or be covered by a separately approved compatibility relaxation.
Ownership safety and required behavior of surviving user objects remain
mandatory, subject to the separately approved SOAC execution-lifetime policy
above; exact temporary counts and instruction-dependent release timing are
not acceptance requirements.
