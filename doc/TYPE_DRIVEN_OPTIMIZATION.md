---
title: "Type-Driven Runtime Contracts"
---

# Type-Driven Runtime Contracts

## Dated design changes

### 2026-08-25 (PDT) — tracing, profiling and monitoring out of scope

Exclude CPython-compatible tracing, profiling and monitoring of SOAC execution
from this milestone. This covers `sys.settrace`, `sys.setprofile`,
`sys.monitoring`, corresponding native observation hooks, and dependent
debugger/coverage behavior on retained compiled and entry-interpreter paths.
Neither observer-event fidelity nor correct explicit refusal of unsupported
observer configurations is required. SOAC event coverage may be absent or
incomplete; do not claim otherwise.

Stop adding observer reservations, enablement interception, event forwarding,
pre-entry rejection, compatible fallback or frame machinery solely to satisfy
an observer compatibility/refusal contract. Audit and remove or simplify that
dedicated implementation. A shared guard remains justified only by a named,
independently required ownership-safety, actual-object/source authentication or
installed-contract invariant. Observer enablement alone is not a reason to
require a new execution/admission proof or a tested refusal path.

Do not disable or alter ordinary CPython tracing/profiling/monitoring. Preserve
ordinary source semantics, explicit program callbacks, argument binding,
exceptions, comprehension scoping, suspension/resumption, recursion safety,
GC, reentrancy and required cleanup. Observer callback counts, event order and
coverage are excluded observations, not additional ordinary-callback promises.
If a callback executes, its supported operations must still enforce selected
field, module, class and method-mutation contracts. Native function/dictionary
watchers or audit hooks needed for an independent construction or mutation
invariant are not removed merely because they are callbacks.

Split mixed tests. Retire SOAC event-stream parity, observer enablement/refusal
and refusal-error-shape assertions that serve only the excluded features.
Preserve genuine construction/mutation, semantic and memory-safety regressions
through in-scope entrypoints, and retain ordinary-CPython observer controls.
Do not replace an obsolete event assertion with a new mandatory
`NotImplementedError` assertion or turn unrelated failures into expected
refusals. Re-run focused in-scope tests and `just test-all` after implementation
changes. Ordinary exception propagation and installed contracts remain required
even when no SOAC observer events are emitted.

This supersedes earlier requirements to support or explicitly refuse these
features, including the observation clauses in the execution-lifetime and
activation-introspection policies. It is a scope decision, not evidence that
the implementation has been simplified or validated. SOAC's internal compiler
counters, diagnostic logs and profile collection are separate from CPython
observer compatibility. Optimization and benchmarks remain deferred until
separately requested.

### 2026-08-25 (PDT) — traceback reconstruction and frame inspection out of scope

Exclude SOAC-specific traceback reconstruction and frame inspection from this
milestone, including retained compiled and entry-interpreter execution. This
is not restricted to optimized-away activations. Reproducing traceback frames,
their source positions/ancestry, inspectable locals or native locals-plus
layout is not an execution, admission or acceptance prerequisite. Ordinary
CPython traceback and frame behavior remains unchanged.

Stop the frame-layout correspondence work, including new helper-owned or
omitted-slot proofs whose purpose is populating native traceback/lifetime
frames. Audit and remove or simplify source-lifetime frames, source-parent
links, native slot projections and associated APIs/tests when they serve only
traceback reconstruction or inspection. Keep shared pieces only for an
explicit independent source-language, ownership-safety or installed-contract
requirement. Do not replace removed frame machinery with another compatibility
executor or frame-retention proof system. In particular, ordinary async or
tuple-target comprehensions must not be rejected merely because their SOAC
bindings do not reproduce CPython's inlined frame slots.

Keep authenticated source/code/function ownership and source ranges needed for
actual type construction, sealed-method metadata and diagnostics. Those facts
do not require a runtime traceback frame or matching execution layout.
Preserve ordinary call binding, results, exception types/identity and chaining,
propagation/handlers, lexical and comprehension scoping, evaluation and explicit
callback order, suspension/resumption, `finally`, context managers, GC safety,
reentrancy, resurrection and required cleanup. No leak, use-after-free, double
release or suppression of required resource/finalizer effects is permitted.
Selected field checks, pending/final type barriers and all other installed
contracts remain enforced through Python, specialized CPython bytecode and
supported C APIs.

Unsupported SOAC frame inspection may continue to fail explicitly; do not
invent valid-looking locals, frames or instruction positions. Do not add
tracing, profiling, monitoring or debugger support as part of this change.
Their existing refusal paths are not reasons to build frame reconstruction.
Do not alter ordinary CPython execution or inspection to accommodate SOAC.

Split mixed tests before changing expectations. Retire SOAC-only traceback
shape, frame inspection and frame-retention parity assertions; preserve the
original programs' in-scope exception, scoping, callback, cleanup and contract
coverage and ordinary-CPython controls. Do not turn an unrelated failure into
an expected refusal. Re-run focused semantic/safety tests and `just test-all`
after implementation changes. This amendment supersedes conflicting text and
acceptance requirements below; it does not claim completed implementation.
Optimization and benchmarks remain deferred until separately requested.

### 2026-08-25 (PDT) — field invariants, without function-level type enforcement

Remove all function-level runtime type enforcement in this implementation,
including SOAC-compiled, entry-interpreter and CPython function argument/return
checks, generated dataclass constructor checks, and deferred default-factory
result checks. The intermediate suggestion to limit call checks to compiled
functions is superseded as well. Any eventual call-type enforcement belongs
to a separately specified layer and is not implemented by retaining a second
current boundary path.

Field-write invariants are independent of call signatures. Preserve the actual
selected storage policy across attribute, native-slot and escaped-dictionary
writes from every supported execution engine and C API. Run ordinary dataclass
initialization and reject an incompatible value at the protected write; earlier
initialization effects are not suppressed by an argument check. `InitVar`
parameters do not create stored fields. A factory's return value has no extra
check unless it is written to selected protected storage. Ordinary functions
and generated constructors must retain normal binding and body exceptions.

Static `ty` signatures, field provenance, authenticated actual-object/source
binding, pending/final decorated-type construction, sealed-method mutation
protections and safe ownership remain independently required. Remove call-
type-only flags, policy knobs, delegates, deferred masks, activation state and
their proof consumers; preserve shared pieces only for a named independent
requirement. Static annotations and successful calls no longer establish
runtime parameter/return proofs. No optimization or check elimination is added.

This amendment supersedes contradictory function-boundary text and tests below.
The local implementation removes the broader call checks and versions the
artifact migration; matched-runtime and full compatibility validation remain
required before this migration is complete.
Never implement migration by clearing checks from an already published live
contract or silently accepting an old artifact under different semantics.

### 2026-08-25 (PDT) — optional storage-owned runtime type-state pointer

Select an optionally allocated `PyTypeState *` on participating storage, with
a per-object presence bit and direct accessor. Ordinary objects of the same
Python type must not reserve a null pointer or allocate a state object. An
escaped instance dictionary owns its effective checking state independently
of the instance; class-wide schemas and hierarchy restrictions remain type
concerns. Prefer an audited allocation trailer, not an arbitrary negative
offset from `PyObject`. See
[Optional storage-owned runtime type state](#optional-storage-owned-runtime-type-state).

This is a required representation change for the existing interpreter-enforced
instance-storage contracts, not a claim of implementation. It supersedes the
open pointer-placement alternatives in the earlier design evaluation. Generic
container enforcement, generic function bindings, tuple specialization and
guard elimination remain deferred. The matching amendment in
[OPT_GOAL.md](../OPT_GOAL.md) makes this narrow allocation change an explicit
exception to the deferral of new layout optimizations.

### 2026-08-24 (PDT) — language semantics, not CPython execution schedules

Exact reproduction of CPython's internal reference-count and instruction-level
lifetime schedule is not a project requirement. This clarification applies to
the current enforcement milestone and retained SOAC execution paths; it is not
a deferred optimization. It supersedes conflicting blanket demands for exact
CPython lifetime observations in the earlier design and compatibility tests.
See the matching
[approved execution-lifetime policy](../OPT_GOAL.md#approved-soac-execution-lifetime-differences).

- SOAC executes its own IR. Calls between SOAC and CPython require correct
  argument binding, ownership and exception handling, not reconstruction of
  the other engine's instruction sequence.
- Do not require equal `sys.getrefcount()` results, identical temporary owners,
  the same borrow/duplicate/move schedule, or the exact timing/relative order
  of finalizers and weak-reference callbacks due to implicit reference release.
  In particular, recognizing each CPython fused store/load form is not a
  prerequisite merely to match that schedule.
- Outside those permitted observations, preserve values, identity and aliasing,
  lexical/comprehension scoping, source evaluation and explicit callback order,
  binding errors, exceptions,
  `finally`, context managers and selected type contracts. Ownership must stay
  safe on normal, exceptional and reentrant paths: no leaks, indefinite dead-
  temporary retention, use-after-free, double release, lost resource release,
  or suppression of required finalizers/weak-reference callbacks. The later
  2026-08-25 (PDT) amendments exclude SOAC traceback/frame inspection and
  tracing/profiling/monitoring compatibility, including mandatory observer
  refusal; ordinary exception and cleanup behavior remains required.
- Stop extending bytecode correspondence, native lifecycle receipts, parallel
  token-body execution or synchronization machinery solely for the excluded
  observations. Audit and remove or simplify that dedicated machinery; retain
  shared parts only when an independent in-scope requirement justifies them.
  Moving schedule matching behind a new CPython API is not the goal either.
- Keep metadata needed for authenticated actual-object/source binding and
  diagnostic source locations. CPython's specialized instructions and supported
  C APIs must still enforce installed contracts; this is distinct from making
  SOAC mirror their internal execution.
- Update tests accordingly: preserve required semantic and memory-safety
  checks and ordinary controls; split mixed tests and revise or retire only
  exact-count or instruction-dependent release-order assertions. Do not
  convert unrelated failures to expected failures or remove their coverage.

Offline analysis, pre-callback pending barriers, final decorated-type binding,
mandatory storage checks, dataclass behavior, dynamic framework fallback,
permanent contracts, local committed submodule history and the full validation
gate remain required. Optimization and benchmarks remain deferred. This is a
scope decision, not a claim that the implementation has already been simplified
or its remaining compatibility tests passed.

### 2026-08-24 (PDT) — pending types and final decorated-class binding

Bind the checked class contract to the actual final result of the decorator
chain. A provisional class escaping through a callback does not itself require
installing the final class's constraints on that provisional object.

For participating fresh types, a native pending-instance barrier may replace
full instance-constraint installation before class callbacks. Install that
barrier before callbacks can observe the type. While it is pending:

- Reject instance creation through Python and all supported native allocation
  paths, not only a direct spelling of `object.__new__(cls)`.
- Reject `__class__` reassignment whose destination is the pending type,
  **before changing the object**, even when CPython considers the layouts
  compatible. Existing objects must not enter the pending type through a
  non-allocating operation. Inherited/subtype admission must not bypass this
  prohibition either.

Run normal class transformations, validate the actual final decorated type,
and install its selected constraints before enabling instance creation. A fresh
replacement, including a dataclass slots replacement, needs its own pending
guard during construction; names, copied namespaces, and the original type's
identity do not confer that guard or the final contract. Known unsupported
framework classes remain dynamic before this participating construction path
is selected. No already published contract may be revoked.

This amends the earlier requirement to install the full instance contract
before `PyType_Ready` and callbacks. It does not authorize method mutation,
bypass existing inheritance rules, or prove invariants for an arbitrary
pre-existing class returned by a decorator. See
[Pending types and final adoption](#pending-types-and-final-adoption).
This is a specification decision, not a claim of runtime implementation or
passing compatibility tests. Optimization and benchmarks remain deferred.

### 2026-08-23 (PDT) — local committed CPython and Ruff sources

The subsequent user instructions extend the committed-history migration to
Ruff/`ty`: use a pinned `vendor/ruff` submodule with
`https://github.com/adamh-oai/ruff.git` as its origin. Replace the checker patch
distribution after verifying equivalent committed sources, coherent runtime
and offline dependencies, and checker/build fingerprints. Keep generated
lockfile changes in a separate top commit and unvalidated drafts separate
from the selected checker generation.

Do not push either native or checker commits now. Verify independent local
checkout reproduction and identify remote checkout/CI availability as deferred.
The maintained standalone patch files can be deleted after their respective
committed-history and tooling migrations are verified. Preserve historical
evidence; do not keep patches as a second maintained source of truth.

### 2026-08-23 21:35 PDT — enforcement gate and CPython commits

- Do not pursue optimization until the full selected interpreter-enforcement
  contract and compatibility matrix are complete and verified. Completing that
  milestone does not automatically resume optimization; require a separate
  explicit request. Keep optimization-only layout, dispatch, check-elimination,
  closure/ABI, and benchmark work out of the current plan.
- Maintain native CPython changes as individual logical commits in the CPython
  repository and pin the resulting commit from SOAC. Replace the maintained
  standalone CPython patch series and patch-application build prerequisite;
  do not maintain both representations as sources of truth. Keep generated
  changes in a separate top commit with regeneration commands.
- The main implementation thread owns the coordinated migration: preserve and
  compare current source contents, update preparation/build/provenance tooling
  and its tests/docs, and verify clean-checkout reproducibility before switching.
  Preserve shared-source mounting and native build/source identity checks.
  See the CPython source-history instructions in `OPT_GOAL.md`. This is an
  instruction to migrate, not a claim that the current checkout is migrated;
  the later Ruff/`ty` migration instruction above extends this decision.

### 2026-08-23 (PDT) — enforcement-only scope

The current deliverable is the full `ty` -> authenticated contracts -> actual
Python runtime types/functions -> interpreter enforcement loop, with SOAC JIT
execution disabled. This supersedes the earlier roadmap's optimization phases
and performance gates; see `OPT_GOAL.md` for the matching goal.

- Keep authentication/versioning, pre-callback construction, checked boundaries,
  complete supported mutation enforcement, dataclass compatibility, automatic
  unsupported-framework fallback, and permanent published contracts.
- Defer stable-layout optimizations, virtual/direct dispatch, unchecked trusted
  entries, proof propagation/check elimination, typed-IR optimization plans,
  profile/apply work, and performance benchmarks until separately requested.
- CPython specialized bytecodes and supported C APIs remain enforcement
  boundaries. They may use safe generic paths rather than new optimized paths.
- Use ordinary CPython closure/activation machinery for interpreter execution;
  do not expand SOAC closure layouts, suspension ABIs, or native lifetime
  recipes solely to prepare future optimizations.
- Replace optimization-plan/throughput acceptance with end-to-end behavioral
  and structured contract/binding/enforcement tests. This changes scope, not
  implementation status; preserve earlier implementation and measurement
  evidence without claiming that the current milestone is complete.

Sections explicitly marked deferred retain future design constraints, not
current work instructions. Storage/dispatch fields in illustrative full schemas
are needed only for a capability actually installed or consumed; they do not
require implementing every optional capability now. No scope change authorizes
revoking an existing contract. Optimization does not automatically resume when
the enforcement phases finish. The filename is retained to preserve links.

## Implementation status

The current scope combines field-only enforcement with the traceback/frame-
inspection and tracing/profiling/monitoring exclusions above. Historical
checked-call, frame-compatibility and observer-refusal results below are evidence
of prior work, not requirements to preserve runtime function type checks, SOAC
frame reconstruction or observer compatibility/refusal machinery.

Status: implementation in progress. This document specifies the selected target
offline-analysis artifact, actual runtime type binding, type-construction
protocol, and interpreter enforcement. It is not a claim that those
capabilities are implemented. Optimization design is retained only as deferred
reference, not as the current implementation goal.
The evidence ledger is in
`doc/optimization-attempts/2026-08-21-type-driven-strict-contracts.md`. No runtime
capability exists merely because source has annotations or passes a checker.

The offline path is implemented as `just ty`: a separately built, pinned
vendored checker exports owned semantic proposals, signs complete deterministic
module generations, and atomically publishes an out-of-band startup descriptor.
Its maintained tests exercise the real checker and the selected CPython without
executing analyzed modules. This is a bounded offline implementation, not completion
of the runtime protocol or the entire diagnostic catalogue described below.

The pinned Ruff follow-up models compiler-tail `__static_attributes__` metadata
as an ordered ordinary checker binding of `tuple[str, ...]` after the original
class suite, using the completed existing scopes to classify its namespace
target. The internal binding is not a user-authored source assignment,
declaration, class default, instance-field catalogue, or runtime/layout
authority; the exporter excludes it from those proposals. User
declarations remain governed by their existing policies. The isolated Ruff
candidate is now in local logical commits with a separate generated-lock top.
Compilation, focused checker tests, the broader upstream library/Markdown
gates, the actual native-backed wrapper, and independent fresh-checkout replay
pass. The selected local generation is now promoted. Focused class-static,
authenticated-artifact, actual type-binding and interpreter-boundary tests pass
against the matching rebuilt CPython. The complete compatibility gate remains
pending; see the evidence ledger for the exact scope and counts.

The execution-compatibility simplification removes the parallel token executor,
scalar bytecode recipes and eager-region lifecycle proofs. Retained source
metadata remains justified by actual lexical bindings, safe ownership and
native Store/CALL authentication, not a CPython release schedule or frame
inspection. Dedicated traceback/frame reconstruction, locals-plus projections,
and observer-only compatibility/refusal machinery have been removed from the
retained SOAC paths. Authenticated source identities, lexical bindings and
handled-exception state remain for their independent construction, source
semantics and cleanup requirements. Focused semantic and ordinary-CPython
controls pass; the repaired combined full gate remains pending, as recorded in
the evidence ledger. Dataclass callback bindings still read actual CPython
execution to authenticate construction operations; they do not reconstruct
SOAC frames, expose locals, or impose function argument/result type checks.
The optional storage-owned `PyTypeState` native and Rust changes are now promoted locally,
with a fresh optimized build and matching extension verified. The isolated
development/StackRef-debug gates and earlier enforcement results do not replace
the pending combined checker/runtime matrix and full gate for that representation.

## Summary

SOAC should use an offline run of Astral's `ty` to discover the logical shape
of strict Python modules, classes, functions, fields, and calls. It should feed
those facts **back into the construction of the actual runtime types**, where
CPython and participating metaclasses allocate physical storage, install
attribute policies, and bind checked types to actual Python objects.

The essential chain is:

```text
source code
    -> offline ty analysis
    -> versioned, source-authenticated module, class, and function contracts
    -> authenticated binding to actual runtime type/function identities
    -> explicit contract passed into actual type construction
    -> native pending-instance barrier before class callbacks
    -> final decorated type validated and selected policies installed
    -> Python/C module, class and protected-field mutation barriers
    -> finalized, permanently enforced runtime contract
    -> end-to-end interpreter compatibility tests
```

A checker result is a **proposal**, not a runtime fact. A logical type must
resolve to its actual authenticated runtime object, and a promised restriction
must be installed and enforced before it becomes a published runtime fact.
This does not require generating optimized code from that fact.

For example:

```python
def call(value: Bar) -> int:
    return value.baz()
```

The annotations are static checker facts, not runtime call predicates. Ordinary
binding accepts any otherwise valid argument, and the body performs Python's
actual lookup/call behavior plus any installed strict protected-name policy.
The result is not checked against `int`. If the body writes a selected checked
field, that write independently enforces its field contract. This works without
a fixed field index, method table, direct-call plan, or SOAC native-code entry.

## Relationship to the existing strict-module proposal

`doc/STRICT_MODULES.md` remains the source of truth for the strict future
feature, module lifecycle, append-only final global bindings, interoperability,
native-mutation limitations, and protected optimization capabilities.

This document defines the following refinements, now reflected in the strict
language contract and compatibility policy:

1. Strict-module membership, frozen class behavior, checked value types, method
   dispatch, and physical instance storage are separate capabilities.
2. A strict module does not imply that every class must be converted into
   dictionary-less `__slots__`.
3. A type-checker-derived class contract, rather than textual `self.x`
   heuristics alone, determines candidate fields and methods.
4. Ordinary dataclasses preserve their actual requested instance-dictionary
   behavior; `@dataclass(slots=True)` preserves its real replacement-class and
   slot behavior.
5. Nonparticipating metaclasses, decorators, and framework-managed classes
   automatically remain dynamic while their surrounding module remains strict.
6. Selected storage writes and independent guards establish only their own
   value invariants. Function annotations and successful calls supply no runtime
   argument or result guarantee.
7. The runtime never upgrades a checker prediction into a runtime fact after
   an incompatible type has already been published.

`doc/STRICT_MODULES.md` now specifies automatic capabilities instead of implicit
slots, preserves actual dataclass dictionary and replacement behavior, and
records protected-name lookup. `OPT_GOAL.md` records the selected field-write
policy and ordinary-call behavior. Those requirements must be implemented and verified before
they can authorize runtime claims or any future native assumptions. Ordinary
Python never acquires these semantics merely by being imported or transformed.

### Unsupported source literals must not become signed values

The pinned Ruff representation replaces surrogate escapes (`U+D800` through
`U+DFFF`) with U+FFFD. Until that representation is lossless, selected strict
source containing an active surrogate escape must fail explicitly before
lowering or fact publication. `soac_source` shares one exact-token validator
between those boundaries and reports the original escape's byte range; this is
an unsupported-source limitation, not Unicode-preservation support.

The checker must also validate actual second parses of string annotations and
decline exact string-literal inference from affected ordinary dependencies,
including imported literal aliases. It must never sign U+FFFD as if it were the
original surrogate. Raw literal portions, escaped backslashes, bytes, comments,
and genuine U+FFFD remain distinct controls. Ordinary native Python execution
is unchanged, including surrogate strings received by strict functions. See
the source-literal limitation in `doc/STRICT_MODULES.md` for the precise scope.

## Goals and non-goals

Goals:

- Infer supported module, class, field, and function contracts without
  requiring developers to annotate individual classes with SOAC decorators.
- Authenticate those contracts and bind their type references to the actual
  Python objects during module/function/type construction.
- Enforce every selected contract in the interpreter without SOAC JIT execution.
- Preserve normal Python lookup, binding, descriptor, validation, and callback
  behavior except for explicitly documented strict-language differences.
- Make class and module mutation restrictions visible to the same type checker
  that produced the runtime contract.
- Keep offline type analysis out of hot imports and native execution.
- Automatically fall back to dynamic behavior when a class cannot support the
  requested contract.
- Avoid runtime deoptimization or revocation of a published strict capability.
- Keep policies and actual runtime bindings explicit in authenticated metadata,
  with structured tests distinguishing proposals from installed enforcement.

Non-goals:

- Treating a Python annotation as a runtime type check.
- Deriving a CPython object offset from a type-checker field position.
- Assuming that a checked project is a closed world.
- Freezing every third-party class or replacing its metaclass.
- Running `ty` during each Python import.
- Providing full soundness against malicious native extensions that directly
  corrupt CPython object memory.
- Optimizing ordinary modules as a separate SOAC execution mode.
- Pursuing strict-module optimization: new physical-layout fast paths,
  virtual/direct calls, trusted unchecked entries, check elimination, inlining,
  profiling/apply plans, or performance/IR-size targets.
- Requiring a new SOAC closure layout, suspension ABI, or native lifetime recipe
  framework for interpreter enforcement without a concrete correctness need.
- Matching CPython's transient reference counts or fused-instruction lifetime
  schedule. These observations do not supply the concrete correctness need
  in the preceding item.

## Terminology

**Logical type fact:** a source-bound checker result such as "`Bar.baz` is an
instance method returning `int`."

**Construction contract:** an immutable, SOAC-owned description of the field,
method, inheritance, dictionary, and mutation policies that a newly created
type must install.

**Construction handle:** an interpreter-owned identity for one actual
execution of a class definition. Repeated execution of the same lexical class
definition creates distinct handles and runtime classes.

**Runtime capability:** a verified, nonforgeable runtime fact published only
after an actual object has satisfied and installed a construction contract.

**Checked value:** a value validated by a protected field write or an
independent runtime value guard. A static signature, completed call or source
ownership record supplies no parameter/return-type proof.

**Participating class:** a final runtime class constructed or explicitly
adopted through the contract protocol.

**Dynamic class:** a class in a strict module that does not possess a
particular runtime capability. It retains ordinary Python behavior for the
unsupported operations.

## Offline type analysis

### Analysis mode

The implemented offline driver runs a pinned, patched `ty` project database:

```text
just ty -- check \
    --project /absolute/project \
    --python /absolute/selected-cpython/python \
    --signing-key /absolute/build-side/signing.key \
    --output /absolute/type-facts \
    --deployment /absolute/startup-authority/deployment.json
```

This is a dedicated executable in `tools/ty`, not a new public upstream
`ty check` output format. Its maintained semantic export query is inside
`ty_python_semantic/src/types`, with access to the real class/member/decorator,
signature, dataclass, attribute, and call-binding queries. AST traversal supplies
source identities and operation owners; it is not a second annotation checker.
See `tools/ty/README.md` for the current interface and conservative limits.

SOAC's runtime uses Ruff's `ruff_python_semantic` for lexical semantics. Only the
separate offline workspace depends on `ty_python_semantic` and `ty_project`.
An external offline artifact avoids importing the checker database, project
resolver, and incremental-analysis engine into the runtime execution path.

Use one shared project-level policy instead of requiring per-class
annotations:

```toml
[tool.soac.strict]
include = ["services/**", "libraries/**"]
exclude = ["generated/**"]
default_class_policy = "automatic"
unsupported_class_policy = "dynamic"
typing_final_policy = "enforce_for_participating_classes"
checked_fields = "disabled"
field_failure = "type_error"
unsupported_value_type = "dynamic"

[tool.soac.strict.adapters]
dataclasses = "stdlib"
pydantic = "dynamic"
```

The `pydantic` setting could later name a verified cooperative adapter. Merely
listing a library never establishes a runtime capability.

Field-check settings are repository-level language policy, not per-class
annotations or optimization heuristics. The illustrated configuration leaves
instance-field value checks disabled; a project can opt into supported writes
with `checked_fields = "supported_annotations"`. Static type analysis remains
independent of runtime call behavior. Class construction, storage checks and
artifact fingerprints consume the same resolved field settings. A mandatory
field check raises its configured strict-language error; an independent
optimization guard instead takes its ordinary generic path. The removed
parameter/return-check policy keys are rejected, including disabled spellings.

### Sound analysis configuration

Optimization-producing analysis must use conservative settings. In
particular, enable and fingerprint:

```toml
[tool.ty.analysis]
strict-equality-semantics = true
strict-generic-narrowing = true

[tool.ty.environment]
python-version = "3.15"
```

The relevant `ty` documentation explicitly notes that its ordinary
equality-based narrowing can be unsound for subclasses and overridden
`__eq__`. Convenient IDE narrowing is not sufficient justification for native
code.

The runtime Ruff family and offline checker share the pinned `vendor/ruff`
committed history, based on upstream revision
`d2620d7312875790b114d821721cddf253f66423`. Its offline dialect implementation
forces these settings after per-file overrides and supplies a separate strict
future stub; it does not change ordinary `ty` defaults or rewrite source bytes.
Real checker tests cover Python 3.15 selection, syntax, stubs, and dialect
isolation. The offline driver supplies search paths from the explicitly queried
interpreter, rather than guessing an installed Python from an uninstalled
build's `sys.prefix`. Its signed publication path records consumed source,
configuration, dependency, directory-query, interpreter/library, and environment
inputs. These analysis settings and artifacts alone are not runtime enforcement
or a sealed optimization capability.

Deployment schema 2 records the selected interpreter and the source kind/path
and actual per-file checker settings for every consumed dependency. The shared
verifier derives System dependency digests and source identities from current
bytes; Vendored identities bind to the startup-pinned checker/typeshed build.
Its expectations come from protected startup configuration, never from copying
the manifest's own dependency claims. Native startup separately checks its actual
interpreter identity against the expected ABI record, including the selected
venv prefix. `PySoac_GetInterpreterPrefix()` reads the current interpreter's
native configuration, not mutable `sys.prefix` or `PyConfig_Get("prefix")`.
Two venvs sharing one executable and libpython are not interchangeable, even
when every analyzed file in the originally selected venv remains unchanged.

The exporter must preserve uncertainty. Explicit `Any`, implicit `Unknown`,
unresolved imports, checker TODO states, ignored diagnostics, dynamically
typed decorators, and substitutions of imports with `Any` must never become
concrete receiver, storage, or call-target facts.

The pinned Ruff/`ty` revision, effective per-file configuration, Python
version, platform, search paths, stubs, installed dependency versions, and
relevant ignored errors all participate in artifact validity.

Explicitly selected strict sources bind their logical module name and canonical
source file in both directions of the checker's resolver, including per-file
environments. The binding is part of the configuration fingerprint. Ambiguous
aliases and missing selected files are errors; neither a fabricated known-module
identity nor a fallback stub may stand in for the selected source. Ordinary
dependencies discovered by project globs keep normal source/stub resolution.
Selection uses the original parsed top-level strict future import, not matching
source text, and does not itself grant executable authority.

Generated framework fields also retain semantic declaration provenance. Fields
whose first declaration is the actual builtin `object` do not become
source-owned instance-field facts. This is an identity check, not a member-name
filter: a user class named `object` and genuine source overrides retain their
own declarations, while builtin `object` remains in the logical MRO.

### Artifact organization

Emit:

```text
type-facts/
    manifest.json
    modules/
        <content-address>.soac-types
        ...
```

Artifact schema 6 / strict contract 2 removes runtime function-check policy and
rejects older publications rather than reinterpreting their guarantees. It
retains the required `FieldTypeFact.annotation_origin` and
`FieldTypeFact.annotation_definition`. Its explicit,
inferred, absent, or unresolved origin is part of signed content and every
derived shard, generation, and cache identity; schema-1 through schema-5 artifacts
and signatures are rejected. The definition names the actual annotated
assignment, or is explicitly null when no unique source declaration is known.
Omitting that key is invalid even when its value would be null.
Assigning an annotated constructor argument to an unannotated
field leaves the field inferred. Real class-body or `self.field: T` declarations,
including inherited dataclass declarations, carry explicit provenance without
evaluating runtime annotation providers.

The schema also requires `ModuleTypeFacts.nominal_bindings`. Each entry identifies
one supported simple-name annotation leaf by an explicit `NominalBindingOwner`:
either a source function plus parameter index or return target, or an exact
`FieldReference` containing the declaring class, annotated assignment, and field
name. Inherited fields preserve their original declaring owner; generated
constructor signature facts project through that field instead of inventing
source function annotations. Those static facts do not install constructor,
`InitVar` or other function-call predicates. Every leaf retains its exact source range and spelling,
semantic class reference, and the
checker-resolved local binding definition and lexical scope. The exporter uses
the actual semantic name-definition query while preserving import aliases;
source traversal supplies only the annotation site and its relation to that
query. `Optional` and `Union` forms are recognized by their semantic special
form, not their source spelling. Equal normalized class types do not collapse
distinct binding leaves: two aliases can hold different runtime executions of
the same class source. Unsupported expressions and ambiguous definitions do not
receive a fabricated binding plan. Missing plans leave required nominal targets
unresolved; they cannot authorize a name-only lookup or a latest-class registry.
Plans are collected transactionally per annotation owner: if one required
nominal leaf is unresolved, no partial plan is published for that owner. A
resolved alias cannot conceal an unresolved alias after their class references
normalize to the same type. Builtin/numeric, `None`, and `type[T]` arms require
no nominal target; semantic `Annotated` metadata is not part of the value type.
Field and function annotations never borrow each other's resolved targets.
Source field identity alone still cannot distinguish repeated executions of a
factory: enforcement also needs the actual construction/field-contract owner.
The schema-6 manifest authentication domain and all derived identities include
this provenance, with no legacy default for an omitted field.

Schema 6 preserves the explicit distinction between source-class and builtin references in both
direct bases and the logical MRO. `BaseReference::Builtin` comes from the
checker's resolved `KnownClass` identity; neither a source spelling nor a
module/qualname pair substitutes for that query. Aliases of `object` retain the
builtin variant, while user classes named `object` retain their exact source
reference and digest. A complete logical MRO cannot place builtin `object`
before another entry. Logical typing/ABC entries are not physical prefix
proposals. Runtime eligibility for the builtin root requires the signed
`Builtin(Object)` variant and the exact actual `PyBaseObject_Type`; other builtin
variants do not gain layout authority or bypass dynamic participation.

The manifest identifies the checker and environment. A module shard contains
only facts for that module and its explicitly referenced external
dependencies. A deterministic binary format can be used for runtime
consumption; JSON export remains useful for diagnostics and debugging.

Ordinary unkeyed digests establish integrity only when the manifest itself is
trusted. Production artifact authority therefore requires either a signed
manifest verified against a loader-held trust anchor or an explicitly trusted,
read-only build/deployment boundary. Bind shards to an immutable generation
identifier and publish each complete snapshot atomically. A writable directory
containing a manifest, shards, and matching attacker-recomputed hashes is not
an authenticated artifact.

The artifact must support incremental updates. Rechecking one source file
should rewrite only affected module shards and dependency records, not one
enormous monorepo-wide payload.

Illustrative manifest:

```rust
struct TypeArtifactManifest {
    schema_version: u32,
    generation: ArtifactGenerationId,
    trust: ArtifactTrustProof,
    ty_revision: String,
    exporter_revision: String,
    strict_contract_version: u32,
    python_version: PythonVersion,
    python_platform: String,
    cpython_abi_fingerprint: Fingerprint,
    normalized_project_policy: Fingerprint,
    resolved_typechecker_configuration: Fingerprint,
    import_search_path: Fingerprint,
    typeshed_fingerprint: Fingerprint,
    installed_stub_fingerprint: Fingerprint,
    modules: Vec<ModuleArtifactIndex>,
}

struct ModuleArtifactIndex {
    module: ModuleContentId,
    source_digest: SourceDigest,
    effective_policy: Fingerprint,
    shard_digest: Fingerprint,
    consumed_dependencies: Vec<DependencyFingerprint>,
}
```

Use SOAC's existing `ModuleContentId`, `PersistentFunctionId`, and serialized
identity tables where possible. Extend their enclosing cache identities with
strict policy and artifact fingerprints. Preserve the existing `u64` source
identity for interoperability, but add a collision-resistant digest such as
SHA-256 for artifact authentication.

Do not persist Salsa database IDs, memory addresses, Python-visible
`__module__` strings, or pretty-printed checker types as stable authority.
Salsa definition identities are not stable across independent checker runs.

### Source-bound module facts

Illustrative module shard:

```rust
struct ModuleTypeFacts {
    module: ModuleContentId,
    source_digest: SourceDigest,
    language_policy: ModuleLanguagePolicy,
    global_bindings: Vec<GlobalBindingFact>,
    classes: Vec<ClassTypeFact>,
    functions: Vec<FunctionTypeFact>,
    nominal_bindings: Vec<NominalBindingFact>,
    attribute_sites: Vec<AttributeSiteFact>,
    call_sites: Vec<CallSiteFact>,
    diagnostics: Vec<StrictDiagnostic>,
}

struct SourceIdentity {
    module: ModuleContentId,
    lexical_qualname: String,
    source_range: SourceRange,
    definition_kind: DefinitionKind,
}

struct FieldReference {
    declaring_class: ClassReference,
    annotation_definition: SourceIdentity,
    name: String,
}

enum NominalBindingOwner {
    Function { function: SourceIdentity, annotation: AnnotationTarget },
    Field { field: FieldReference },
}

struct NominalBindingFact {
    owner: NominalBindingOwner,
    expression_range: SourceRange,
    name: String,
    class: ClassReference,
    binding: SourceIdentity,
    binding_scope: SourceIdentity,
}

struct CallSiteIdentity {
    module: ModuleContentId,
    source_digest: SourceDigest,
    enclosing_function: SourceIdentity,
    expression_range: SourceRange,
    expression_kind: CallExpressionKind,
}

struct GlobalBindingFact {
    name: String,
    mutability: GlobalMutability,
    value_type: StaticType,
    definition: Option<SourceIdentity>,
}

enum GlobalMutability {
    FinalAfterSeal,
    ExplicitlyMutable,
    LateAppendOnly,
    Unknown,
}
```

A source location identifies a particular syntactic definition; its runtime
identity is still resolved separately. A function produced by a trusted
dataclass transformation may not have its own source definition and therefore
needs explicit runtime adoption.

### Class facts

The checker exports logical class semantics:

```rust
enum BaseReference {
    Class(ClassReference),
    Builtin(BuiltinType),
}

struct ClassTypeFact {
    identity: SourceIdentity,
    bases: Vec<BaseReference>,
    metaclass: MetaclassFact,
    decorators: Vec<DecoratorFact>,
    instance_fields: Vec<FieldTypeFact>,
    methods: Vec<MethodTypeFact>,
    class_members: Vec<ClassMemberFact>,
    inheritance: InheritanceFact,
    openness: ClassOpenness,
    transform: Option<ClassTransformFact>,
}

struct InheritanceFact {
    linearized_bases: Vec<BaseReference>, // logical MRO excluding this class
    complete: bool,
}

struct FieldTypeFact {
    name: String,
    declaring_class: ClassReference,
    value_type: StaticType,
    annotation_origin: AnnotationOrigin,
    annotation_definition: Option<SourceIdentity>, // required key; explicit null is allowed
    field_kind: FieldKind,
    read_policy: FieldReadPolicy,
    write_policy: FieldWritePolicy,
    initialization: InitializationPolicy,
    default: DefaultFact,
    descriptor: DescriptorFact,
}

enum FieldKind {
    InstanceField,
    CallableInstanceField,
    ShadowableClassDefault,
    CachedDescriptorField,
    ClassVariable,
    InitOnly,
    FrameworkPrivate,
    Dynamic,
}

struct MethodTypeFact {
    name: String,
    declaring_class: ClassReference,
    binding: MethodBinding,
    signature: CallableSignature,
    declared_final: bool,
    override_policy: OverridePolicy,
    implementation: Option<SourceIdentity>,
}

enum MethodBinding {
    Instance,
    Class,
    Static,
    PropertyGetter,
    Descriptor,
}
```

These facts are not physical layouts. In particular:

- `instance_fields[3]` does not imply that a CPython dictionary stores the
  field at index `3`.
- A callable instance field does not bind `self`.
- An instance method does bind `self`.
- A class method binds the actual receiver class.
- A static method does not bind a receiver.
- A property or arbitrary descriptor may execute Python code on every read.
- `ClassVar` and dataclass `InitVar` are not ordinary instance fields.
- Pydantic aliases are not the canonical instance-dictionary field names.

Actual descriptor classification must inspect the realized runtime descriptor
and its type slots. Typeshed can deliberately model assignment support that
does not correspond to a real `tp_descr_set`; `functools.cached_property` is
one such important distinction.

### Static types

Use a normalized, structured representation:

```rust
enum StaticType {
    ExactBuiltin(BuiltinType),
    NominalClass(ClassReference),
    ExactClass(ClassReference),
    Union(Vec<StaticType>),
    Optional(Box<StaticType>),
    Callable(CallableSignature),
    Literal(LiteralValue),
    TypeVariable(TypeVariableFact),
    StructuralProtocol(ProtocolFact),
    Any,
    Unknown,
    Todo,
    Divergent,
    Unsupported {
        kind: UnsupportedTypeKind,
        reason: UnsupportedReasonCode,
    },
}
```

Normalize union order deterministically and normalize `Optional[T]` to the
same canonical encoding as `Union[T, None]`. Preserve whether subclasses are
accepted. A Python `int` annotation does not imply an exact `int` object;
`bool` is an `int` subclass. A typing `float` position can accept integers and
does not prove an exact `float` native operand.

Mutable generic contents do not become checked merely because a container has
an annotation. A `list[Bar]` value requires either a genuinely protected
container contract or per-element checks; ordinary aliased Python lists cannot
publish permanent checked-element facts.

### Call-site facts (deferred)

Call-target selection facts are future optimization input, not required for
the current artifact's storage contracts or source ownership.

```rust
struct CallSiteFact {
    identity: CallSiteIdentity,
    enclosing_function: SourceIdentity,
    receiver: Option<ReceiverTypeFact>,
    attribute_name: Option<String>,
    candidate_targets: Vec<CallableTargetFact>,
    binding: CallBindingFact,
    signature: CallableSignature,
    result_type: StaticType,
    uncertainty: CallUncertainty,
}

enum CallUncertainty {
    ExactStaticTarget,
    OpenSubclassFamily,
    FiniteUnion,
    CallableInstanceField,
    CustomDescriptor,
    StructuralProtocol,
    Dynamic,
}
```

A unique source definition is not automatically a unique executable target.
Dynamic descriptors, inheritance, monkeypatching, ordinary subclasses, and
instance shadowing remain separate runtime questions.

A call expression is not a semantic definition. Its identity uses the
original AST byte range and enclosing function instead of fabricating a
`DefinitionKind` or retaining an unstable checker database identifier.

## Feeding facts into type construction

### Normative requirement

Every selected class policy and checked-type contract must reach construction
of the **actual runtime type** through an explicit SOAC-owned contract.

The required sequence is:

```text
offline ClassTypeFact
    -> validated ClassConstructionContract
    -> per-execution ClassConstructionHandle
    -> explicit type/metaclass construction request
    -> requested storage and native pending barrier before callbacks
    -> normal __set_name__ and __init_subclass__
    -> normal decorator application
    -> validation of the actual final returned class
    -> selected constraints installed before instance admission
    -> module/class sealing
    -> immutable StrictClassCapability
```

No runtime or future optimizer may substitute "`ty` said it was a field,"
"the final class has the same `__module__`," or "a watcher saw a similar dictionary key" for this
construction and enforcement protocol.

### Existing construction boundary

The SOAC lowering path below is an existing integration point, not a
prerequisite for this milestone. Native interpreter class construction must
receive the same authenticated contract without requiring SOAC lowering or JIT
execution. Both paths must install the native pending barrier before callbacks
and the final type's selected constraints before enabling instances.

SOAC currently lowers a class to:

```python
__soac__.create_class(
    name,
    namespace_function,
    bases,
    keyword_arguments,
    requires_class_cell,
    first_line,
)
```

The helper prepares the namespace, executes the class body, and calls:

```python
cls = metaclass(name, resolved_bases, namespace, **metaclass_kwargs)
```

CPython's `type.__new__` then:

1. Determines `__slots__` and the physical object layout.
2. Allocates the actual heap type.
3. Calls `PyType_Ready`.
4. Invokes descriptor `__set_name__`.
5. Invokes inherited `__init_subclass__`.
6. Returns the class.

Callbacks can observe or publish the class and create instances. Installing a
physical layout after the existing `create_class` returns is therefore too
late.

### Explicit construction contract

Add a compiler-owned construction argument:

```python
__soac__.create_class(
    name,
    namespace_function,
    bases,
    keyword_arguments,
    requires_class_cell,
    first_line,
    class_construction_handle,
)
```

Illustrative full contract (physical-prefix and method-layout members describe
deferred optional capabilities, not additional enforcement deliverables):

```rust
struct ClassConstructionContract {
    plan: StrictClassPlanId,
    source: SourceIdentity,
    expected_bases: Vec<ExpectedBase>,
    expected_metaclass: ExpectedMetaclass,
    expected_decorators: Vec<ExpectedDecorator>,
    inherited_field_prefix: Vec<FieldLayoutEntry>,
    declared_fields: Vec<FieldLayoutEntry>,
    method_layout: Vec<MethodLayoutEntry>,
    name_policies: Vec<NameResolutionPolicy>,
    instance_storage: InstanceStoragePolicy,
    dictionary_replacement: DictReplacementPolicy,
    class_mutation: ClassMutationPolicy,
    finality: FinalityPolicy,
    checked_values: CheckedValuePolicy,
    participating_adapter: Option<AdapterIdentity>,
}

struct ClassConstructionHandle {
    module_instance: StrictModuleInstanceId,
    plan: StrictClassPlanId,
    execution_nonce: u64,
    construction_phase: ConstructionPhase,
    namespace_function: AuthenticatedFunctionIdentity,
    expected_metaclass: AuthenticatedTypeIdentity,
}

enum ConstructionPhase {
    OriginalClass,
    DataclassSlottedReplacement,
    OtherVerifiedReplacement(AdapterIdentity),
}

struct FieldLayoutEntry {
    field: StrictFieldId,
    name: InternedName,
    logical_kind: FieldKind,
    inherited_from: Option<StrictClassId>,
    frozen_default_owner: Option<StrictClassId>,
    physical_storage: RequestedFieldStorage,
    checked_type: Option<CheckedTypeId>,
}

struct MethodLayoutEntry {
    method: StrictMethodId,
    name: InternedName,
    declaring_family: StrictDispatchFamilyId,
    slot: MethodSlotId,
    binding: MethodBinding,
    signature: VerifiedCallAbi,
    shadow_policy: InstanceShadowPolicy,
    finality: MethodFinality,
}
```

The handle must be an explicit authenticated value, not a temporary class
attribute, hidden namespace entry, thread-local variable, guessed
`__module__`, or user-forgeable integer.

The actual implementation must be interpreter-owned, single-use, scoped to the
current authenticated module execution, and bound to the exact lexical class
plan, namespace function, expected metaclass, and construction phase. Consume
or expire it at the intended class-construction boundary. Reject replay,
cross-module or cross-class transfer, callback theft, concurrent reuse, and
use after the intended construction completes.

The construction call itself must resolve to a frozen compiler-owned intrinsic
or equivalently authenticated runtime entry. Calling a mutable ordinary Python
`soac.runtime.create_class` global without verifying its target would allow
untyped code to intercept or misuse the construction handle.

A class definition inside a function may run repeatedly. Each execution gets
its own construction handle, actual class identity, and runtime capability,
even when all executions share the same offline class-plan identifier.

### Installing the contract

For the builtin `type` metaclass, introduce an explicit CPython construction
entrypoint, conceptually:

```c
PyObject *
_PySOAC_TypeNewWithContract(
    PyTypeObject *metaclass,
    PyObject *name,
    PyObject *bases,
    PyObject *namespace,
    PyObject *metaclass_kwargs,
    const SoacClassConstructionContract *contract
);
```

The pending-type path uses the entrypoint to establish authenticated
construction ownership, not to guess the final decorator result. It:

1. Verifies the resolved actual bases, metaclass, namespace, and source-bound
   plan.
2. Validates actual inheritance and source-requested storage, including slots.
3. Installs the native pending-instance barrier on the actual new type.
4. Completes normal `PyType_Ready`.
5. Executes normal `__set_name__` and `__init_subclass__` in their original
   order, with instance admission still prohibited.

The pending barrier must be present before step 4. A callback may retain the
class object, but cannot create an instance or change an existing object's
`__class__` to that pending type. Constraints may be installed after decoration
as described below. Additional fixed prefixes or dispatch layouts remain
optional future capabilities, not a prerequisite for this protocol.

#### Pending types and final adoption

Pending state belongs to the actual native type and its authenticated
construction, not a Python attribute or the eventual name binding. Python
aliases, ordinary callers, specialized bytecode and supported C APIs must
observe the same barrier. In particular, layout compatibility is not permission
for `object.__setattr__(obj, "__class__", pending_type)` or an equivalent
supported C operation: reject before changing the object's type or state.

After decorators finish, validate the actual final result and bind its field,
descriptor, method and nominal requirements to the actual runtime objects.
Install every selected protection needed for allowed instances before ending
pending state. Publication must leave no callback/reentry interval in which
allocation is enabled while a required protection is absent. This transition
is not revocation of a sealed contract. Allocation need not wait for the
surrounding module's later global-sealing step once these protections exist.

A pending base must not admit instances indirectly through a subtype or base
reassignment. Reject such operations unless their complete pending/admission
path preserves the same invariant and the existing inheritance restrictions.
An arbitrary pre-existing type cannot acquire a no-earlier-instances guarantee
retroactively; retain dynamic fallback or reject an unsupported adoption.
The allocation barrier alone supplies no permission to skip selected checks
on pre-admission static/classmethod calls.

Required behavioral coverage includes construction-callback allocation
rejection, layout-compatible `__class__` reassignment rejection with unchanged
object identity/type/state, supported native allocation and assignment paths,
inherited admission, and successful creation only after final constraint
installation. Include ordinary and slotted dataclasses and dynamic-framework
controls. These are acceptance requirements, not completed validation.

### Custom metaclasses

A non-default metaclass is automatically dynamic unless it uses a verified
cooperative construction adapter.

A participating adapter must propagate the explicit pending construction state
into the actual metaclass path and, ultimately, the actual type allocation.
It must preserve:

- Metaclass selection across all bases.
- Custom `__prepare__` and namespace behavior.
- Class-body execution.
- `__classcell__` propagation.
- Metaclass `__new__` and `__init__`.
- Descriptor `__set_name__`.
- Base `__init_subclass__`.
- User-visible class identity and callback order.

Passing an unexplained keyword argument to an ordinary metaclass, substituting
the builtin `type`, or attaching compiler metadata to the class namespace is
not equivalent.

If an adapter cannot propagate the pending barrier before type publication,
the class remains dynamic. A strict module containing that class can still
publish final module globals and optimize unrelated functions.

### Decorators and replacement classes

Decorator expressions evaluate before the class body; decorators apply after
the class is constructed. A class contract must account for both phases.

A class decorator may:

- Mutate and return the existing class.
- Return a different class.
- Register the class with an external framework.
- Create instances, subclasses, descriptors, or observable callbacks.

Recognized transformations receive authenticated construction ownership through
an explicit trusted adapter. If a transformation constructs a replacement
class, the replacement's own type construction must receive its own linked
pending barrier before its callbacks. The final contract is validated against
the type actually returned, after the transformation has finished.

For `@dataclass(slots=True)`, the original class and replacement intentionally
have different physical layouts:

```text
original class:
    original pending handle + ordinary dictionary-bearing storage

replacement class:
    linked replacement pending handle + source-requested native slots
```

The original class may already have escaped through `__set_name__` or
`__init_subclass__`, but the pending barrier forbids instances at that point.
It must retain its own observed storage behavior. Never
apply the replacement's slot policy to the original class or transfer the
original class's runtime capability to the replacement.

The original provisional class and final replacement are distinct runtime
objects. Never attach the original capability to the replacement based on
matching names or copied attributes.

After successful final admission, an unselected provisional type with no
installed permanent type contract can leave pending state as dynamic. It does
not receive the replacement's contract or storage. Preserve any independently
installed function or other permanent contracts; none may be revoked.

After all decorators run:

```python
final_cls = __soac__.adopt_final_class(
    construction_handle,
    decorated_result,
)
```

This validates the actual final identity, descriptor kinds, field catalog,
metaclass, bases, method implementations, and adapter-specific invariants,
then installs the selected protections before enabling instances. It does not
retroactively invent a physical layout.

An existing already-compatible final class can receive a **weaker** post-hoc
capability when every required invariant is independently verifiable and no
earlier callback or escaped instance violates it. Such adoption must never be
described as construction-time enforcement.

### Class lifecycle

```text
PLANNED
    -> PREPARED
    -> PENDING (ordinary storage; no instance admission)
    -> DECORATED
    -> VERIFIED
    -> ENFORCED + SEALED (selected protections installed; instances enabled)
```

Before physical allocation or any irreversible strict behavior, an unsupported
class may instead be constructed as:

```text
DYNAMIC
```

Observing a pending class does not publish its final type contract. Its
ordinary physical layout remains unchanged, and pending state can end only
through the explicit final-admission or unselected-provisional disposition
above. A failed pending admission must not silently reopen instance creation.
Once selected permanent constraints are installed, an incompatible later
decorator or mutation must fail; alternatively, SOAC may decline an additional
unpublished optimization capability while retaining every installed restriction.

The actual source-requested storage is present during `PENDING`. Final native
admission must install the selected class and method mutation protections before
enabling instances, including before releasing temporary admission operands.
This applies while the containing module is still initializing as well as in a
later factory. A separate post-admission sealing call cannot close an earlier
allocation or finalizer-reentry window. The interpreter path publishes no
optional layout, direct-target, or virtual-table capability.

Module global bindings have their own later sealing boundary. Method metadata
can already be frozen while the module is initializing; its annotated calls
still have ordinary semantics and require no parameter/return target snapshot.
Module sealing does not change an active call's result behavior or reopen any
function, class or default metadata. Required field targets, including direct
Self, must be resolved by final class admission, independently of optional
guarded-site publication at module sealing.

A sealed capability cannot later be revoked to accommodate monkeypatching,
framework remapping, dictionary replacement, or changed descriptors.

## Optional storage-owned runtime type state

This section is normative for the current enforcement representation. It does
not require new generic semantics or optimized instance-field layouts.

### Implementation and remaining migration — 2026-08-25 (PDT)

The optional-state native source is committed at
`b8dcf1ca1a138253c51c8733e52e597d7db68abf` and promoted with the matching Rust
factory, predicate projections, and raw-reference-count validation. This records
source integration, not a validated selected runtime: the fresh optimized
build, matching extension, actual `ty`-authenticated integration matrix and
`just test-all` remain pending at this checkpoint. Isolated native development
and actual StackRef-debug validation do not establish those remaining joins.

The initial native guard requires a 64-bit pointer, little-endian layout and
the GIL. Linux AArch64 is the tested platform; other architectures and
threading modes have not been validated. This is not a C `long`-width claim.
The implementation uses bit 4 of the existing object flags as the allocation
marker, preserves ordinary object/GC header offsets, and freezes the allocated
inline-values extent in audited existing header padding. Matching raw Rust
reference operations modify only the low `u32` count, not the adjoining overflow
or flag fields; the marker must survive increments, decrements and last-owner
cleanup. Ordinary object and dictionary allocation sizes do not grow, and
extended dictionaries do not enter the ordinary dictionary freelist.

During the unopened Pending bind, Rust registers its storage-state factory
once. The barrier still blocks instances until final decorated-type admission.
At supported allocation, native code selects and authenticates the actual
type's immutable state using the existing `tp_cache`, a type/version receipt
and retained MRO across callback-capable preparation. Foreign caches are not
overwritten. Instance state contains separately projected dictionary rules and
actual native-slot rows. A fresh materialized dictionary receives only its
dictionary projection, with the nominal targets its own rules require and no
receiver or unrelated slot-target backedge. Direct writes use checked native
tail access, not the legacy identity table or repeated MRO discovery.

The current allocation/migration boundary is explicit:

| Storage path | Promoted implementation and remaining boundary |
| --- | --- |
| Fresh default fixed-size GC heap instance | Optional trailer for selected obligations, using the audited object/GenericNew, GenericAlloc, subtype traversal/clear/deallocation and GC-free family with an `object` solid base. |
| Fresh exact dictionary materialized from a participating instance | Owns the immutable dictionary-only projection before publication; escaped aliases keep their rules independently of the receiver. |
| No selected storage obligations | No state allocation or trailer merely because a factory was registered; independent class, method and inherited protections still apply. |
| Existing or replacement dictionary, including a dictionary subclass | Preserve its actual identity and allocation. Existing protected state remains protected; otherwise required late attachment continues through the legacy policy. No copying, moving or retroactive trailer bit. |
| Custom `__new__`, variable-size/extension allocator, mixed legacy/factory family or unversionable type | Explicitly unconverted allocation path with existing and inherited enforcement retained. No incompatible owner's metadata is passed to the Rust factory. |
| Module/function metadata, generic containers, lists/tuples and scalar/singleton objects | Not migrated by this instance-storage change; no generic or optimization capability is implied. |

Published state rules remain immutable and may be shared. Per-storage
installation, mutation and terminal flags do not rewrite the shared rules or
revoke another attachment. GC traverses the real metadata edges; allocation
failure, clear, resurrection, reuse and supported C member views still need the
combined validation below. The legacy paths listed here are not completion of
the direct-state migration requirement. See the
[runtime boundary](RUNTIME_FUNCTIONS.md) and
[module lifecycle](MODULE_LIFECYCLE.md) for the native/Rust division.

### Ownership and direct access

The object whose storage is constrained owns an optional strong reference to
native, GC-visible `PyTypeState`. The immutable state bundles the effective
write rules and native checking operations, plus resolved generic bindings
when that later feature is supported. Nongeneric states have no generic
bindings. Share equal immutable state where appropriate; do not allocate a
separate descriptor for every instance. Per-attachment lifecycle/bookkeeping
must not mutate a shared contract or revoke another object's protection.

The state belongs to the dictionary for instance-dictionary writes and to the
instance for native slot storage. Managed inline dictionaries must retain the
policy before materialization and allocate/initialize the dictionary's state
slot before exposing that dictionary. The escaped dictionary keeps its rules
even after the instance is gone; avoid a receiver backedge solely for policy
lookup. Preserve actual storage projections when slots or descriptors hide
same-name dictionary entries.

The hot storage check follows a presence-bit test and a direct pointer load,
not an object-identity side-table lookup or repeated instance/MRO policy
discovery. Resolve inherited field obligations when preparing the effective
state, without dropping distinct actual nominal bindings. `PyTypeObject`
continues to own class schemas, construction/finality state and hierarchy
rules; it is not the sole authority for per-object storage constraints.

This does not move source-function or module-binding metadata into every value.
Lack of storage state does not disable separate installed class, method-mutation,
module or inherited restrictions. Function parameters and results acquire no
runtime type check from storage state or from a static signature.

### Conditional allocation and presence bit

Conceptually, for each supported allocation family:

```text
ordinary:  [existing allocator/GC prefix][ordinary object storage]
stateful:  [existing allocator/GC prefix][ordinary object storage][PyTypeState *]
```

Only the stateful allocation reserves the final pointer. Use an audited
per-object layout bit, conceptually `HAS_TYPE_STATE_SLOT`, in an existing
appropriate flag field. A `PyTypeObject` flag alone cannot distinguish two
exact dictionaries or lists with different allocation forms. Do not grow every
object header just to add the presence flag. Check flag availability and
initialization for each supported architecture and threading build; unsupported
layouts need an explicit support decision, not an invented ABI bit.

Keep all offsets behind checked, type-aware internal allocation/access/free
helpers. Prefer a trailer to an optional preheader: retain ordinary object and
GC-header offsets rather than assuming that `self - 4` or another universal
negative offset is available. Compute the tail from the complete supported
allocation layout, including alignment and overflow checks. A generic
`sizeof(PyObject)` or base-class `tp_basicsize` calculation is not sufficient
for arbitrary subclasses, variable-size objects, or managed inline storage.

The bit means that the allocation contains a slot, not that enforcement is
currently active. Initialize the slot and its layout marker before traversal,
callbacks or publication can observe them; a null slot is allowed only in a
defined private initialization/teardown phase and must not authorize unchecked
writes to protected storage. Retain allocation-form information until freeing
or correctly reinitializing the allocation. Do not reuse GC tracking bits or
clear the marker merely because contract teardown has begun.

The extra pointer does not change the object's Python type or identity, but
does not by itself promise binary compatibility with old extension headers,
inline writers or layout mirrors. Audit supported native entrypoints and
matching allocators. Correct CPython allocation/free pairing remains required;
arbitrary raw `malloc`/`free` or direct struct writes are not a supported escape
from ownership or enforcement rules.

### Construction and migration

Pass the selected state explicitly to supported allocation, reserve its slot,
and establish checking before writes or publication require the guarantee.
Class-wide nongeneric field rules can be selected from an admitted class even
when ordinary code constructs its instance. Future generic construction needs
resolved construction-site bindings; a strict class definition or mutable
`__orig_class__` attribute does not supply missing arguments.

Initially exclude arbitrary custom `__new__`, inherited custom allocation and
unverified metaclass/extension allocation paths from this new construction
protocol. Support default allocation and explicitly audited builtin/adapter
paths. Do not invoke arbitrary `__new__` and then claim that its escaped result
was protected before return. Decide unsupported admission before irreversible
installation; retain all already installed and inherited contracts.

An existing object without the extra slot cannot be enlarged in place while
preserving its address and aliases. Do not silently copy, move or retag it to
attach state. Identify pre-existing storage and late-attachment cases in the
migration plan. Where a legacy policy is needed during migration, record that
path as unconverted and preserve enforcement; it is not completion of the
direct-state requirement or a second canonical copy of the same policy.
Do not change an approved interoperability behavior just to fit the trailer.
Automatic generic adoption/conversion is not introduced by this amendment.

Ordinary interpreter execution, warmed specialized bytecodes and supported C
APIs must honor attached state. Execution-engine fallback cannot turn it off.
The presence bit is not a proof that a particular expected contract matches.

### Allocation-family and lifecycle requirements

| Storage family | Required representation or future extension point |
|---|---|
| Exact instance dictionary | Optional tail pointer; ordinary dictionaries retain ordinary size. Escaped aliases use the same dictionary-owned policy. |
| User instance with native slots or inline dictionary storage | Audited optional extension of its complete allocation; materialization transfers/projects obligations without an unprotected interval. |
| Exact list, when generic enforcement is added | Pointer on the list body, not its resizable item buffer; item-buffer growth must preserve the state. |
| Exact tuple, when generic evidence is added | Aligned tail after the variable-length item array, outside logical tuple length; requires variable-size allocation/access support. |
| Scalars and shared singleton objects | No new pointer merely to participate in a nominal check; do not attach interpreter-local state to shared singletons. |

GC traversal must include retained contract/type edges. Ordinary and extended
allocations must not be mixed by freelists; initially bypassing freelists for
extended objects is acceptable. Audit failure cleanup, deallocation,
resurrection and reuse. Future tuple support must also address GC untracking,
private resize and the shared empty tuple; tuple immutability does not remove
metadata lifetime obligations. Tuple/list generic support is not required for
this milestone.

Acceptance tests for the current migration must verify:

- Ordinary and protected objects of the same Python type use the correct
  allocation form; ordinary allocations reserve no state-pointer slot.
- A missing bit never causes a tail read; the direct-state path does not query
  the identity table. Shared states do not couple attachment lifetimes.
- Attribute writes and escaped-dictionary writes enforce the same effective
  obligations, including after receiver destruction and dictionary
  materialization, through ordinary, specialized and supported native paths.
- OOM, rejected initialization, cyclic GC, resurrection and repeated
  allocation/free do not leak state, confuse freelist sizes, expose unchecked
  storage or dereference a stale pointer.
- Unsupported allocation and pre-existing-storage paths retain their documented
  semantics and installed protections; no fake late-installation guarantee.

Validate behavior with SOAC JIT execution disabled. This amendment claims no
measured speedup and does not authorize benchmarks or check elimination.

## Physical instance layout

This section retains the design for deferred optional field-layout capabilities;
the preceding runtime type-state representation is separately in scope.
Preserving actual dictionaries, source-requested slots, descriptor behavior,
replacement identity, and checked writes is required now; constructing new
stable indexes or exposing raw-load plans is not. If an existing fixed-layout
capability is retained, its invariants still apply.

### Storage capabilities

Keep storage independent from class freezing and method dispatch:

```rust
enum InstanceStorageCapability {
    GenericPython,
    StableIndexedDictionary {
        layout: StrictLayoutId,
        fields: Vec<IndexedField>,
        replacement: DictReplacementPolicy,
    },
    NativeObjectSlots {
        layout: StrictLayoutId,
        fields: Vec<NativeObjectField>,
    },
    AdapterOwned {
        adapter: AdapterIdentity,
        layout: VerifiedAdapterLayout,
    },
}
```

An ordinary dataclass can therefore have frozen methods without losing its real
instance dictionary. A Pydantic class can have a generic instance-storage
capability even when some independently verified class behavior is usable.

### Stable indexed dictionaries

For ordinary dictionary-bearing classes, the preferred representation is a
real, mutable, Python-visible instance dictionary with a protected,
SOAC-owned fixed field prefix:

```text
type-owned schema:
    first  -> 0
    second -> 1
    third  -> 2

instance storage:
    values[0] = first value or UNSET
    values[1] = second value or UNSET
    values[2] = third value or UNSET
    overflow  = dynamically added names and non-string dictionary keys
```

SOAC, not `ty`, allocates the physical indices during type construction.

Required invariants:

- Inherited fields preserve their actual superclass positions.
- Participating subclasses extend the same prefix without renumbering it.
- Multiple inheritance either has a verified compatible prefix or remains
  dynamic.
- Reserving a field index does not make it a visible dictionary key.
- An `UNSET` field remains absent from `vars(instance)`, dictionary length,
  insertion order, iteration, copying, and serialization.
- Deletion restores `UNSET` and preserves the ordinary missing-attribute
  behavior.
- Reinsertion preserves ordinary visible insertion-order behavior.
- Dynamic names and arbitrary supported dictionary keys go into overflow
  without invalidating the fixed prefix.
- Overflow growth, hash-table rehash, and dictionary materialization never
  renumber fixed fields.
- A movable values array is safe only when generated code reloads its current
  base after operations that can move it.
- There is exactly one authoritative owned reference per field value.
- GC traversal, clearing, weak references, reference counts, watchers, and
  public dictionary behavior remain coherent.

Stock CPython split dictionaries do not currently provide these invariants:
growth, promotion, replacement, and materialization can alter or invalidate
their internal representation. This proposal requires an explicit protected
instance-dictionary implementation or an equivalent verified adapter.

### Native object slots

For classes that already genuinely request `__slots__`, use verified actual
member-descriptor offsets:

```text
field address = instance address + verified PyMemberDef offset
```

Use the final actual class rather than an offline assumption. A slotted
subclass can still inherit a real instance dictionary from a base.

Hidden native storage must be represented in the type's GC traversal and
clearing metadata. Do not store separately owned copies in both a hidden slot
and the visible instance dictionary.

The pending protocol separates the logical dictionary-field catalog from the
native object-member catalog. CPython creates source-requested members in the
ordinary solid-base layout. Rust reserves offset cells beforehand and binds
them against the actual final type's `tp_members` at admission, while instances
remain prohibited. The immediate-contract native API instead validates its
full policy before Ready; it must not leave an earlier classcell/GC allocation
window. Native member setters and supported `PyMember_SetOne`
views select policy by physical offset, not by caller-controlled field spelling;
adaptive slot stores use the same barrier. Ordinary subclasses inherit physical
write obligations without gaining strict receiver or dispatch authority.
For optional-state allocations, reject member views overlapping the state
pointer, its layout-marker field or the saved inline-allocation extent before
running a value converter. Those internal allocation fields are not writable
Python members. This targeted defense does not make arbitrary header views,
raw pointer writes or `Py_SET_TYPE` part of the supported member API.

A dictionary-bearing slotted type may have an explicit hidden dictionary entry
with the same spelling as a member. It is a separate Python value, not a mirror
of the slot. An inherited dictionary-prefix obligation remains enforced on that
entry; a native-member-only requirement does not silently become a check on
an unrelated hidden dictionary value. Storage owners retain a dictionary-field
selection for each actual check owner, not just a merged set of field names.
Replacement construction selects its own storage routing without rewriting
the original's storage. Native pending direct-self predicates remain unresolved
until the actual selected final type binds them once; inherited predicates and
each inherited dictionary selection retain their original declaring targets.
Optional native reads require an actual
sealed construction and its canonical visible member descriptor. NULL members
use original attribute lookup so unbound/deleted-field errors remain intact.

The original actual type is also bound by a callback-free weakref in its owner,
not merely by an address and Rust execution token. Public native constructors
can receive an exposed owner; they cannot make a new type match that immutable
weak witness after its original dies. Owner-only callbacks and optional field
and method capabilities verify this witness without retaining the class.

For recognized dataclass replacements, the native handle has an explicit
replacement mode and no namespace-function operand. It consumes the exact live
`_add_slots` producer view once. Original and replacement classes share declaring
source provenance, but have distinct construction witnesses and physical
catalogs; the replacement never rewrites the original class-dictionary witness
or rebinds its immutable nominal targets. Native and authenticated runtime tests
exercise source-requested members, dataclass replacements, hybrid inherited
dictionaries, failed applications, and collection of both classes. Those
focused results are distinct from the still-pending combined compatibility
matrix and full project gate; the earlier fixed-prefix tests alone were not
evidence for native slots.

### Class defaults

Preserve ordinary class-default behavior:

```python
class Point:
    x = 1


point = Point()
assert vars(point) == {}
assert point.x == 1
```

Do not eagerly insert `x` into `point.__dict__`. That would change dictionary
contents, deletion behavior, pickling, constructor observations, and object
ownership.

The offline class contract must classify an eligible annotated or unannotated
plain class value as `ShadowableClassDefault` and reserve its instance-prefix
position during actual type construction. Exclude explicit `ClassVar` names,
methods, and descriptors from this classification. Record the frozen default
owner separately; the index is not available unless this construction entry
actually exists.

For a genuine frozen plain class-data default, a fixed-index load can use:

```text
candidate = instance.values[X_INDEX]
result = select(candidate is not UNSET, candidate, FROZEN_CLASS_DEFAULT)
```

The default must come from the actual receiver class's frozen MRO. This
optimization is invalid for data descriptors, arbitrary properties, cached
descriptors, and custom attribute hooks.

### Dictionary replacement

`instance.__dict__ = replacement` is a real dictionary-identity change.
Native code, including Pydantic's validation path, can perform the equivalent
operation through `PyObject_GenericSetAttr`.

Available policies are:

```rust
enum DictReplacementPolicy {
    Reject,
    NormalizeReplacementInPlace,
    AdapterMaintainsStableLayout,
    Generic,
}
```

`NormalizeReplacementInPlace` must preserve:

```python
replacement = {}
instance.__dict__ = replacement
assert instance.__dict__ is replacement
```

It must validate or reorganize the actual replacement dictionary before it is
published, keep the fixed prefix valid, and preserve retained references to
both the old and new dictionaries.

Copying the replacement into the old dictionary is not equivalent. A strict
class without a verified replacement policy cannot expose an unconditional
fixed instance index.

### Initialization and deletion

A type-checker declaration does not prove that a field has already been
initialized:

```python
class Bar:
    baz: int
```

An access may need an `UNSET` check, an ordinary class-default fallback, or an
`AttributeError`. The check can be removed only after a verified constructor,
dominating store, checked-field invariant, or other valid dataflow fact proves
presence.

Deletion remains legal unless the strict contract explicitly prohibits it.
A native access cannot blindly dereference a null field pointer.

## Attribute resolution and descriptor behavior

### Runtime member classification

Classify the realized final MRO entry, not a printed type, `callable(value)`,
or a typeshed declaration:

```rust
enum RuntimeMemberKind {
    PlainClassValue,
    PythonInstanceMethod,
    StaticMethod,
    ClassMethod,
    DataDescriptor,
    NonDataDescriptor,
    DeclaredInstanceField,
    CachedDescriptor,
    Dynamic,
}
```

CPython normally resolves reads in this order:

```text
data descriptor
    -> populated instance dictionary
    -> non-data descriptor
    -> plain class value
```

The real discriminator is the descriptor type's `tp_descr_get` and
`tp_descr_set` slots. A property without a user-defined setter is still a data
descriptor because its descriptor setter exists and raises.

Descriptors with mutable descriptor classes or mutable descriptor behavior
cannot supply permanent precedence facts unless those descriptor dependencies
are also frozen.

The initial builtin-descriptor producer selects exactly one canonical
`staticmethod`, `classmethod`, or getter-only `property` application directly
to the compiler-recorded newly created function. The descriptor expression is
evaluated before defaults and function construction, as in ordinary Python.
Runtime selection checks the actual immutable builtin and records the function,
code, owner, and namespace execution in a native birth record. The record does
not add a strong reference to the function or code beyond ordinary ownership.
The runtime witness also fixes the original record's non-reused native birth
ID immediately after construction, before another callback. An exposed witness
reused by a supported C constructor must not authorize its different birth;
object addresses alone cannot distinguish reuse. The ID does not substitute for
the function, source, execution, or owner checks.
After complete copied-namespace admission, pre-Ready adoption seals the actual
descriptor before callbacks. Input, copied, and adopted namespace validation
are distinct phases; a not-yet-adopted birth is not a seal.

Borrowing a descriptor from another execution of the same source, constructing
a wrapper around an existing function, rebinding the actual decorator, or
using a decorator chain does not grant this authority. Such unsupported classes
decline before installation. An already adopted descriptor keeps its original
contract when borrowed by ordinary code. A getter-only property remains a data
descriptor with ordinary read-only errors, not a protected instance-method name
or a physical field slot.

### Name-specific strict policies

Keep name resolution and assignment policy separate:

```rust
struct NameResolutionPolicy {
    name: InternedName,
    owner: NameOwner,
    read: NameReadPolicy,
    attribute_write: AttributeWritePolicy,
    dictionary_write: DictionaryWritePolicy,
}

enum NameOwner {
    DeclaredInstanceField,
    ShadowableClassDefault,
    ProtectedInstanceMethod,
    ProtectedClassMethod,
    ProtectedStaticMethod,
    ProtectedClassVariable,
    DataDescriptor,
    CachedDescriptor,
    Dynamic,
}

enum NameReadPolicy {
    OrdinaryDescriptorPrecedence,
    InstanceFieldBeforeInheritedNonDataMethod,
    InstanceFieldThenFrozenClassDefault,
    ProtectedClassMemberIgnoresInstanceDictionary,
    Dynamic,
}

enum AttributeWritePolicy {
    CheckedInstanceField,
    OrdinaryInstanceField,
    ShadowableClassDefault,
    InvokeDataDescriptor,
    RejectProtectedClassMember,
    Dynamic,
}

enum DictionaryWritePolicy {
    CheckedDeclaredField,
    AllowShadowableClassDefault,
    AllowIgnoredProtectedName,
    AllowDescriptorBacking,
    Dynamic,
}
```

For protected method or `ClassVar` names:

```python
instance.method = replacement                  # raises
setattr(instance, "method", replacement)     # raises
object.__setattr__(instance, "method", replacement)  # raises

instance.__dict__["method"] = replacement    # may succeed
assert instance.method is not replacement      # protected lookup ignores it
```

The final behavior is an explicit strict-language deviation from normal
non-data-descriptor shadowing. It avoids requiring every raw dictionary
insertion to reject a method-name key.

The policy must be implemented in every actual attribute-read path, not only
in SOAC-generated code. CPython's generic attribute getter, method-lookup
helpers, specialized attribute bytecodes, and native C attribute APIs must
agree.

In particular, `object.__getattribute__(instance, name)`,
`PyObject_GenericGetAttr`, both CPython method-lookup helpers, and specialized
`LOAD_ATTR`/method-call opcodes must all return the protected class method
rather than the ignored colliding dictionary value. A Python-level
`__getattribute__` override is not a sufficient enforcement boundary.

### Declared fields take precedence over inherited methods

The class contract must distinguish:

```python
class Parent:
    def schema(self):
        ...


class Child(Parent):
    schema: str
```

If `schema` is a genuine declared instance field, it is not an optimizable
inherited method on `Child`. Its populated instance value must remain visible.

This distinction is necessary for dataclasses and Pydantic models that
legitimately define fields whose names collide with inherited methods.

Field precedence never bypasses a real data descriptor. A getter or setter
with observable behavior must still execute.

### Cached properties

Standard `functools.cached_property` is a non-data descriptor that reads and
writes an actual instance dictionary.

The contract may support it by:

1. Reserving its exact cache name as a declared shadowable dictionary field.
2. Running the actual descriptor on cache misses.
3. Loading the reserved populated field on cache hits when all descriptor
   dependencies are valid.
4. Preserving assignment, deletion, and recomputation behavior.

Alternatively, an explicitly recognized replacement descriptor can become a
data descriptor that performs its own dictionary lookup. Such replacement is
observable through descriptor identity, descriptor type, setter behavior, and
calls on every cache hit; it is not silently equivalent to the standard
library descriptor.

Arbitrary non-data descriptors cannot all be treated as cached properties.
Framework field descriptors and method-producing descriptors have different
semantics and require their own validated contracts or generic lookup.

### Descriptor-owned fields

Dataclasses, Pydantic, and ORMs may install data descriptors whose getters,
setters, warnings, validation, or instrumentation are observable.

A field present in the checker catalog is eligible for a raw field load only
when the actual final descriptor and hooks authorize that access.

Otherwise:

```text
logical field is known
    + descriptor has observable behavior
    -> descriptor call, not raw indexed load
```

## Method dispatch

Method tables and virtual/direct code-generation paths below are deferred.
The current milestone still enforces method finality, protected-name lookup,
ordinary descriptor binding, and ordinary public calls, including overrides and
ordinary subclasses. Generic interpreter dispatch is sufficient.

### Method layout

Method slots are class-family-owned positions:

```rust
struct DispatchFamily {
    id: StrictDispatchFamilyId,
    methods: Vec<DispatchMethod>,
}

struct DispatchMethod {
    name: InternedName,
    slot: MethodSlotId,
    binding: MethodBinding,
    abi: VerifiedCallAbi,
    override_policy: OverridePolicy,
}

struct ClassDispatchTable {
    owner: StrictClassId,
    family: StrictDispatchFamilyId,
    entries: Vec<DispatchTarget>,
}

struct DispatchTarget {
    function: FrozenFunctionCapability,
    environment: FunctionEnvironmentId,
    entry: NativeEntry,
}
```

A participating subclass inherits its parent's slot positions and replaces the
target entry for an allowed override.

Multiple inheritance needs an explicit common-family, interface-table, or
adjustment policy. Declaring the same method spelling in two unrelated bases
does not automatically prove that their physical method-table positions agree.

### Virtual dispatch

For:

```python
class Bar:
    def baz(self) -> int:
        return 1


class Child(Bar):
    def baz(self) -> int:
        return 2
```

the checker proposes:

```text
receiver family = Bar
member kind = bound instance method
signature = () -> int
override policy = open
```

The runtime contract establishes:

```text
BAZ_METHOD_SLOT = 4
Bar.method_table[4] = Bar_baz
Child.method_table[4] = Child_baz
```

Native code can emit:

```text
table = load_dispatch_table(value)
target = table[4]
return target.entry(target.environment, value)
```

Required invariants:

- The receiver belongs to the participating dispatch family.
- Every participating override uses the same binding mode and verified ABI,
  or an explicit adapter thunk.
- The effective MRO and owner remain valid.
- Protected instance methods cannot be shadowed during attribute lookup.
- Class and method mutation cannot replace a sealed target.
- Unknown ordinary subclasses use the generic boundary.

The receiver-family proof must inspect the actual `Py_TYPE(value)` and its
authenticated installed class capability. A user-visible
`isinstance(value, Bar)` result is insufficient for native layout admission:
custom `__instancecheck__`, ABC virtual subclasses, structural protocols, and
proxies can report membership without possessing the required physical layout
or dispatch table.

A Python typing signature does not itself specify a machine calling
convention. Each dispatch entry carries an actual verified native ABI and
the callee's own globals, builtins, defaults, closure cells, and ownership
environment.

### Direct dispatch

Emit a fixed direct call when all of the following hold:

1. The actual receiver has an authenticated strict receiver/family capability,
   or an independently verified exact runtime type.
2. The actual receiver type is exact, or the method is runtime-enforced final
   across every receiver admitted to this direct path.
3. The class binding and relevant MRO cannot change.
4. Instance lookup cannot shadow the method.
5. The exact function's code and consumed call metadata are protected.
6. The actual descriptor binding matches the call.
7. The callee environment, argument-binding plan, and ABI are known.

```text
return BAR_BAZ_ENTRY(BAR_BAZ_ENVIRONMENT, value)
```

The optimizer may inline only after satisfying the additional existing
ownership, evaluation-order, recursion, cleanup, and exception contracts.
CPython observer compatibility is outside the current milestone under the
2026-08-25 (PDT) amendment; this deferred design does not restore that obligation.

`typing.final` is a checker declaration, not a runtime barrier. It becomes a
direct-dispatch fact only when class construction rejects prohibited
subclasses or method overrides.

### Method evaluation order

Python resolves the callable before evaluating argument expressions:

```python
value.baz(change_value())
```

The resolved method must be captured before `change_value()` runs. A method
plan cannot move descriptor lookup or target selection after argument
evaluation unless the construction contract proves doing so unobservable.

The same rule applies to exceptions, bound receivers, class methods, static
methods, finalizers, and nested calls.

### Ordinary subclasses

An ordinary Python subclass is not automatically part of a strict dispatch
family:

```python
class OrdinaryChild(StrictBar):
    def baz(self):
        return "different"
```

Options are:

- Runtime-enforced finality rejects subclass creation.
- A participating strict subclass receives the same inherited dispatch
  contract.
- An ordinary subclass remains ordinary and enters a generic receiver path.

Static knowledge of all subclasses currently present in a repository does not
close the Python world to dynamically imported or generated subclasses.

## Checked runtime types

### Separate shape and value contracts

Class shape and value typing are independent:

```text
logical shape contract:
    Bar has an instance field named count

checked value contract:
    every observable value stored in Bar.count is an int accepted
    by the declared runtime type policy
```

The first identifies a member policy; it does not promise an indexed load.
The second requires checks through all supported writes. Neither requires
optimizing later reads or eliminating checks in this milestone.

Do not infer the second from the first.

### Public function boundaries

Runtime function-level type checks are removed by the 2026-08-25 (PDT)
clarification. Source and generated functions use their ordinary argument
binding, body, result and exception semantics. A function annotated to return
`int` does not acquire a runtime return check, and an annotated parameter does
not reject an otherwise valid Python call merely because its value has a
different type. This applies to SOAC, the retained entry interpreter, CPython,
and calls through supported C APIs; no alternate checked-entry path remains.

Preserve positional-only and keyword-only arguments, defaults, variadics,
descriptor-bound receiver placement, ordinary binding errors, evaluation order,
closure identity, exception handling and required cleanup. These behaviors do
not depend on a function-level type-check plan. An initializer may execute
earlier effects before reaching a protected write. A selected field rejects
an incompatible stored value at that write, not at the call boundary.

Authenticated source/function ownership, native creation records, structural
signature matching for source identity, and independently required frozen-method
metadata are separate concerns. Retain them where needed for actual type
construction and module/class integrity, without treating them as parameter or
return-type proofs. Ordinary return completion can also finalize nested
definitions; removing a return predicate must not discard that lifecycle work.

Static signatures remain available to `ty` and ordinary source analysis.
Neither an annotation nor successful execution establishes a trusted runtime
argument/result fact. Existing guarded execution paths must retain their
independent guards or decline a capability whose old justification depended on
a removed check. Future function-type enforcement requires a separate design.

### Supported runtime field checks

The initial checked subset should be deliberately small:

```rust
enum CheckedType {
    ExactBuiltin(BuiltinType),
    NominalBuiltin {
        builtin: BuiltinType,
        allow_subclasses: bool,
    },
    NumericWidening {
        target: BuiltinType,
        accepted: Vec<BuiltinType>,
    },
    NominalClass {
        class: StrictClassId,
        allow_subclasses: bool,
    },
    Union(Vec<CheckedType>),
    Optional(Box<CheckedType>),
    None,
}
```

Define each check's exact behavior:

- Exact class versus nominal subclass acceptance.
- `None` and union acceptance.
- `bool` as a subclass of `int` when nominal semantics permit it.
- Typing numeric widening, including integer values accepted in a `float`
  position.
- Changes to `__class__`.
- Custom metaclass `__instancecheck__`.
- Virtual subclass registration.
- Forward references that are not resolved before sealing.

A nominal field `Bar` check must accept an ordinary genuine `Bar` subclass
unless the selected strict-language policy explicitly rejects it. Passing the
field check establishes nominal acceptance at that write, not a
`ParticipatingStrictReceiver` fact. Native field offsets and method-table
access require a second independent capability check; accepted ordinary
subclasses continue through the generic path.

Likewise, accepting an `int` in a `float`-annotated position does not prove an
exact native `float` representation. Exact unboxing requires a separate exact
type proof or an explicitly defined coercion.

Protocols, `Any`, unresolved generics, arbitrary mutable containers,
user-defined `__instancecheck__`, and unsupported annotation expressions do
not produce an unchecked native proof.

Runtime enforcement must never eagerly evaluate lazy annotation providers or
execute annotation expressions solely to recover facts already supplied by
the offline artifact.

A direct self-class reference whose signed binding is the owning class
definition denotes that particular construction. In the pending protocol its
required target is reserved but unresolved during callbacks and decoration,
then bound once to the authenticated final type before instance admission.
An unresolved reservation must not fall through to an older same-named global
or annotation cell. A slots replacement binds this requirement to its own
independently guarded final identity, not the provisional original. This narrow
structural rule does not apply to aliases or other classes with equal source
identities; they require their actual signed lexical operands.

Selected field alias leaves use their authenticated declaration operand: the
actual source globals, original provider's validated capture, or explicit
class-dictionary capture. No provider is evaluated. Construction resolves the
targets required by the field policy before enabling instances; later cell
writes do not revoke or rebind those installed targets. This freezes the field
check's target, not the cell or its future contents, and supplies no live-cell
stability, receiver-layout, or check-elimination proof. Optional guarded field
and method requests retain only the actual targets needed by their independent
capability checks. There are no per-call parameter/return target snapshots.

The actual lexical target may be an ordinary imported type or an automatically
dynamic framework type. Native nominal membership does not require a strict
class owner. The runtime checks the actual native type and subtype relationship
without invoking a metaclass's `__instancecheck__`, virtual subclass registry,
or a value's `__class__` property. The target is retained, but its mutable MRO
is not assumed stable. Optional field and method publication separately requires
a sealed actual class with the matching authenticated class identity and source
digest; a nominally accepted framework type receives no such capability.

A class-dictionary capture is mutable too. Before taking its binding value,
the runtime must prove it is the actual dictionary copied into this particular
class execution, using the live native class policy and the private execution
owner. Matching source identity, cell identity, names, or dictionary contents
alone is insufficient. A substituted cell value remains ordinary annotation
state but cannot authorize a required class-scoped check. The coordinate
witness itself owns no Python references and must become terminal when its
actual class binding dies.

Every selected field retains its own annotation-leaf bindings. A union
can accept several actual class executions even when normalization gives them
one source class reference, but every required leaf must resolve. An unresolved
leaf cannot silently remove an alternative from the contract. Mutable-cell
premises used by any separate optimization must still be revalidated after
callbacks; declaration-bound field targets do not establish such premises.

An inferred return type is not a reusable runtime fact. Native operations need
their own guard or other independently validated premise, including when a
callee or virtual override has a declared return annotation.

### Checked fields

A mandatory checked field has a protected write contract over its actual
source-requested storage. A separate fixed-location load capability additionally
requires a stable physical location; mandatory checks do not require an indexed
dictionary or an optimized read.

The write contract must cover all supported ways to change the value:

```python
obj.field = value
setattr(obj, "field", value)
object.__setattr__(obj, "field", value)
obj.__dict__["field"] = value
obj.__dict__.update({"field": value})
```

It must also cover supported native APIs, whole-dictionary attachment,
deserialization, framework adapters, generated constructor bodies, and
SOAC's own raw indexed stores.

If a supported mutation can store an unchecked value, subsequent code must
perform a load-time check or treat that field as dynamically typed.

A Pydantic adapter must preserve the library's validation/coercion order. It
cannot reject raw input before the model validator has performed a conversion
that ordinary Pydantic accepts.

Runtime field policies retain their actual construction identity independently
of the normalized logical layout. Two factory executions can have equal source
field/type references but different nominal targets. Inheritance must preserve
both actual policies; merging their static type descriptions does not permit
dropping either write requirement. Diamond inheritance may reuse the same
actual policy once. Ordinary subclasses inherit the mandatory policies without
acquiring strict receiver or dispatch authority.

For a field nominal leaf, construction reads only its authenticated lexical
operand: the original class annotation provider's validated capture, the actual
source class namespace, or the actual source module globals. A direct-self
leaf reserves a GC edge during preparation, remains unresolved through pending
callbacks and decoration, and binds once to the authenticated selected final
type before admission enables instances.
No provider is evaluated. In particular, a method-local `self.field: T` may have
no runtime capture at all; a source spelling or `Field.type` is not a substitute.
Such a class needs an explicit authenticated construction operand or must decline
before installing a contract.

An escaped instance dictionary retains the actual type targets required to
enforce future writes, but not its former receiver or the class/module policy
merely for lookup. A selected direct-self field necessarily retains that type;
the edge and resulting cycles must be visible to GC. A field write does not
freeze its referent's ordinary `__class__` or MRO. Consequently, a nominal write
check alone cannot justify a persistent load proof after effects that can change
membership; a subsequent boundary or operation must recheck unless a separate
stable premise has been proved.

### Check elimination (deferred)

This section constrains future work only. The current interpreter milestone
performs selected storage checks and requires no new proof propagation or check
elimination. Existing paths must not bypass a selected contract. Future call-
type enforcement is unspecified and supplies no premise to the current system.

Elide an individual check only when validated typed IR contains a dominating
proof:

```rust
enum CheckedValueProof {
    NominalTypeAccepted(CheckedTypeId),
    VerifiedReceiverCapability {
        class: StrictClassId,
        family: StrictDispatchFamilyId,
        layout: Option<StrictLayoutId>,
    },
    CheckedProtectedField(StrictFieldId),
    VerifiedExactBuiltin(BuiltinType),
    DominatingExplicitTypeGuard(CheckedTypeId),
}
```

An annotation or completed constructor/function call supplies none of these
proofs. A protected-storage read or an explicit guard can support only the
property it actually establishes. Direct field/virtual dispatch additionally
requires a `VerifiedReceiverCapability`; an ordinary subclass can satisfy a
nominal field check while still taking the generic method path.

The proof must survive only across operations allowed by its contract. If
`__class__` reassignment, unchecked dictionary mutation, mutable closure
contents, or an untrusted callback can invalidate the premise, restore a check
or use the generic path.

Checker inference, a parameter annotation, profiling evidence, and the name
of a variable are never sufficient proofs.

## Runtime mutation enforcement

### Enforcement principle

Every published runtime contract must identify the mechanism that prevents it
from becoming false.

```rust
struct VerifiedSemanticFact {
    subject: RuntimeObjectIdentity,
    fact: SemanticFact,
    enforcement: EnforcementCapability,
}
```

If a fact has no complete supported enforcement path, it is not published.

### Enforcement matrix

| Invariant | Runtime enforcement |
|---|---|
| Final module global | Protected module dictionary and all supported global-store APIs |
| Frozen class member | Type mutation barrier plus protected actual class dictionary |
| Stable MRO | Protected bases, subclass policy, and participating hierarchy |
| Protected instance method | Attribute setter rejection plus lookup that ignores colliding dictionary entries |
| Checked instance field | Checked owner-aware instance-dictionary or member-slot writes |
| Fixed instance offset (deferred optional capability) | Construction-installed layout plus replacement/growth policy |
| Frozen function code | Function metadata setters and supported C function APIs |
| Stable descriptor precedence | Frozen verified descriptor type and relevant descriptor behavior |
| Final method | Type-construction override rejection and immutable owner binding |

### Attribute assignment

Overriding a Python `__setattr__` method is not enough:

```python
object.__setattr__(instance, "method", replacement)
```

can directly invoke CPython's generic setter and bypass the class's Python
override.

Enforce participating receiver policies inside the shared underlying
attribute-assignment implementation before:

- A descriptor setter is invoked.
- An inline managed-dictionary value is written.
- A materialized dictionary entry is changed.
- A native member slot is updated.
- A protected method or class variable is shadowed.

Preserve legitimate descriptor setters and custom framework hooks when the
construction adapter explicitly admits them.

### Dictionary mutation

For checked field values and protected module/class dictionaries, enforce
policy at the authoritative dictionary mutation layer, not only through a
wrapper or dictionary watcher.

Cover supported operations including:

```python
mapping[key] = value
del mapping[key]
mapping.update(...)
mapping.setdefault(...)
mapping.pop(...)
mapping.popitem()
mapping.clear()
mapping |= other
```

and the corresponding supported C entrypoints:

```c
PyDict_SetItem(...)
PyDict_SetItemString(...)
PyDict_DelItem(...)
PyDict_DelItemString(...)
PyDict_Merge(...)
PyObject_GenericSetAttr(...)
```

Bulk operations must validate all policy-relevant writes before exposing an
invalid state. Reentrant mappings, key hashing/equality, dictionary aliases,
shared dictionary owners, and non-string keys require explicit compatible
handling or conservative rejection.

A canonical stored `str` subclass is still a Unicode field name. Its selected
value predicate applies to insertion, replacement, bulk writes and initial
dictionary admission just as for an exact `str`. Read its Unicode payload
without calling `str()`, hashing or equality again after the native lookup.
This does not normalize arbitrary non-string overflow keys into field names
or grant a no-alias/read-value proof.

Existing CPython dictionary watchers are observability mechanisms. They do not
provide a reliable rejecting pre-mutation barrier; watcher errors can be
reported as unraisable. They cannot establish these invariants.

### Adaptive interpreter operations

CPython's warmed-up bytecode interpreter has specialized attribute operations
that can directly access instance values or member slots without calling
`PyObject_GenericSetAttr` or the ordinary dictionary API.

For participating types, update or disable every incompatible specialized
path, including:

```text
STORE_ATTR_INSTANCE_VALUE
STORE_ATTR_WITH_HINT
STORE_ATTR_SLOT
LOAD_ATTR_INSTANCE_VALUE
LOAD_ATTR_WITH_HINT
LOAD_ATTR_SLOT
specialized method lookup and call operations
generated executor/tier-specific equivalents
```

Both `_PyObject_GetMethod` variants must use the same protected-name policy
as generic attribute reads. A Python program that warms a specialization must
not bypass checked field writes, protected method lookup, descriptor behavior,
or actual layout validation merely because it stopped calling the generic
helper.

### Whole-dictionary replacement

Protect the actual shared replacement seam:

```text
Python __dict__ assignment
    -> generic descriptor setter
    -> _PyObject_SetDict / managed-dictionary attachment
```

This seam must:

1. Identify whether the receiver has an active strict layout.
2. Reject, normalize, or delegate the actual replacement under its declared
   policy.
3. Validate checked field values and protected-name behavior.
4. Preserve incoming dictionary identity when replacement is allowed.
5. Publish the new layout and ownership before any observer can see it.

Protecting only a custom `tp_setattro` misses native callers that directly
invoke `PyObject_GenericSetAttr`.

### Class mutation

After sealing, enforce the actual type and class-dictionary boundary:

```python
Bar.baz = replacement
del Bar.baz
setattr(Bar, "baz", replacement)
Bar.__bases__ = (AnotherBase,)
```

All must reject when they violate the frozen class contract.

Protect direct access to the authoritative type dictionary as well. A type
attribute setter alone cannot prevent an extension from obtaining the actual
dictionary and calling a supported dictionary mutation API.

Participating types must also reject incompatible instance `__class__`
reassignment before it invalidates stored field indices or method tables.

### Frozen function targets

Sealing a class binding does not freeze the referenced function object:

```python
Bar.baz.__code__ = other.__code__
```

Protect every function property consumed by the actual direct-call plan:

- Function identity.
- Exact code object.
- Relevant default and keyword-default bindings.
- Keyword-default dictionary contents when consumed.
- An authenticated SOAC-owned private entry/trampoline when direct code
  generation consumes it.
- Closure-cell identities.
- Actual globals and builtins mappings.

Do not treat CPython's public vectorcall implementation pointer as immutable
function semantics. `PyFunction_SetVectorcall` returns `void`, and SOAC itself
uses compatible vectorcall replacement. A semantics-preserving pointer update
remains permitted implementation state; direct native plans use their own
authenticated frozen body/trampoline. Behavior-changing replacement outside
the supported native contract cannot become an approved mutation loophole.

Patch or reject `PyFunction_SetDefaults`, `PyFunction_SetKwDefaults`, and
`PyFunction_SetClosure` when they would change frozen call semantics.
`PyCell_Set` remains legal for ordinary mutable closure contents; it requires
protection only when a separate optimization explicitly claims immutable cell
contents.

Closure-cell contents and mutable default objects remain dynamic unless a
separate contract explicitly protects them.

For undecorated source function definitions, the retained SOAC producer emits
`CompleteFunctionDefinition`, exported by `soac_core::block_py`, after metadata
and type-parameter setup and before the source binding. The CPython consumer
instead uses the original `definition_store` callback, authenticating the actual
native code, instruction ordinal, and supported value operand without requiring
SOAC IR. In both paths, an eligible free function born during module
initialization freezes at module sealing; in an already sealed module it is
adopted at its definition boundary. A class-owned definition remains pending for
actual class adoption; an independent function assigned into a class is still a
free function. This classification uses immediate lexical source ownership, not
the final class member catalogue or a qualified-name guess.

The current completion producer does not adopt arbitrary decorator results or
retain the original function across a decorator call. Supporting a recognized
identity-preserving adapter requires its own actual construction proof.
Function annotation leaves do not create required runtime targets at adoption.
Actual targets needed by selected fields must instead resolve under the field
construction protocol before instances are admitted. Optional guarded-site
targets can decline publication without changing ordinary call behavior.
Neither function completion nor a successful call authorizes an argument/result
type proof or physical layout. See the evidence ledger for the combined
implementation's validation status.

A function generated with `exec` during a trusted transformation may be
adopted at sealing when its actual ownership and environment are validated.
Originating from `exec` is not, by itself, disqualifying.

Do not freeze unrelated shared functions merely because a strict class
references them. Dataclasses install some shared standard-library helper
functions, while generated `__init__` functions are distinct objects.

### The unsupported native boundary

No design can simultaneously guarantee:

1. An externally exposed authoritative exact mutable CPython dictionary.
2. Permanent dictionary immutability.
3. Arbitrary unrestricted native mutation through all existing C APIs.

In particular, `PyDict_Clear` returns `void` and has no ordinary error channel
with which a patched implementation can reject clearing an immutable
namespace.

The strict runtime must explicitly choose one:

- Restrict the supported native C API contract for protected dictionaries.
- Prevent unsupported extensions from receiving the authoritative dictionary.
- Store authority outside the externally writable dictionary.
- Avoid publishing the optimization capability.

For a mutable **instance** dictionary, `clear` can be supported when it
removes visible entries while preserving the hidden fixed schema and checked
storage machinery. The unsatisfied case is clearing a namespace whose existing
bindings are promised permanently immutable.

Direct native memory corruption and writes that bypass all supported CPython
APIs are outside the supported threat model.

## Framework and decorator compatibility

### Ordinary classes

For a source-authenticated ordinary class with the builtin `type` metaclass:

1. Consume its checker-derived field and method contract.
2. Construct the class through the explicit protected type entrypoint.
3. Preserve a real instance dictionary unless the source requested slots.
4. At final class adoption, bind selected class/Self predicates and freeze
   mandatory class, method, and default metadata before enabling instances.
5. Finish still-unbound module nominal leaves at that module's authenticated
   seal; calls meanwhile use an actual post-binder snapshot. Do not rebind
   established targets or reopen frozen metadata.
6. Publish optional capabilities only after their separate prerequisites hold.
   The CPython backend publishes no optional layout, direct-target, or
   virtual-table capabilities.

No per-class annotation is required.

### Standard dataclasses

`ty` can determine genuine dataclass fields, defaults, inheritance, callable
fields, `ClassVar`, and `InitVar` candidates.

The trusted dataclass adapter must:

- Preserve the exact user-requested `slots`, `frozen`, `weakref_slot`,
  `kw_only`, ordering, and hash options.
- Preserve `default_factory` evaluation and `__post_init__`.
- Preserve descriptors and generated assignment semantics.
- Preserve class defaults without adding synthetic visible dictionary keys.
- Reconcile the actual final real fields rather than treating pseudo-fields as
  instance storage.
- Preserve `cached_property` behavior on dictionary-bearing instances.
- Authenticate and, when appropriate, adopt generated Python function objects.

The adapter separates invocation-scoped helper attestation from generated
function ownership. An existing Python stdlib helper can participate only
after its complete code and nested constants, instruction sites, flags,
binding layout, actual globals/builtins, defaults, closure values, and native
entry match independently verified implementation evidence. Structural checks
must use exact native types and callback-free reads, never Python equality or
overridable attribute access. An equivalent `FunctionType` copy installed
before preparation can qualify under that full graph check; matching a name
or code object alone cannot. This grants only a role in one actual class-site
invocation, not strict-source ownership, JIT eligibility, or permission to
freeze shared stdlib functions. Active parent validation uses the frame's
captured executed code, and effect-sensitive identities are rechecked after
allocating validation and before each privileged transition.

The independent body evidence is a pair of native-build frozen code recipes
for the selected `dataclasses` and `reprlib` sources, authenticated by the
selected native library. Decoding a recipe never executes its module or roots
Python code persistently. Comparison may project only the explicit stable
filename; constants, layout, code, and call sites still match structurally.
Neither mutable `__file__` nor an analyzed typeshed stub authenticates a runtime
stdlib body. Actual globals, builtins, defaults, sentinel objects, and helper
classes are separate environment witnesses and must be validated as such.

The privileged-edge manifest resolves exact source locations to unique real
CALL/CALL_KW/CALL_FUNCTION_EX offsets in that attested code. Opcode numbers
come from the configured native build header, not Python's mutable `dis` or
`opcode` modules. A LOAD or cache instruction sharing the expression's span
does not select a role. The native call boundary still checks the actual
callee, executed parent code, and bound role operands.

The generated repr decorator is a distinct case: its expression has exactly
two ordered CALLs at one source span. The actual authenticated factory code
must contain that unique pair, with different callee/operand roles and the
repr implementation's recorded birth between them. Missing, extra, or
ambiguous calls reject the projection. This does not authorize general
bytecode-pattern recognition or calls from a code-identical copied factory.

Fresh compilation is not sufficient body evidence: tracing can replace a
builder's already-bound `body` or `name`. SOURCE and final installation must
match deterministic, role-owned generation fragments derived from signed
fields/options and verified actual Field/default operands. The immutable
transcript is then bound to the exact exec text and resulting code tree.
The native bridge compiles that text once. A second Rust-side compilation
would issue an extra observable audit event, so the callback consumes the
actual native compiler result and its callback-free weak code tree instead.
Unmodeled fragments decline before binding or fail explicitly afterward;
they never acquire an install role merely by traversing a trusted helper.

Fresh decorator closures, exec-created factories, and generated methods need
the separate native creation record installed before CREATE watchers. That
record binds the actual function to its invocation, producer frame/site, code,
and role. Copies cannot inherit it. Unknown initial callable/option graphs
decline before class protection; incompatible transitions after binding fail
explicitly without revocation. Completed or pre-bind-declined invocations
retain no extra class, builder, or globals lifetime through escaped records.
The Created transition consumes a preallocated one-use function slot. Native
creation authority is installed before observers can discover the function;
it authenticates its birth and role, not its parameter or return types.
Failed publication tombstones the native record and releases the initialized
function normally; it must not directly free an object with escaped weak or
strong references. An escaped failed function remains terminal.

Fresh per-method annotation providers and fresh repr implementations are
adopted individually when their native birth role and exact relationship to
the generated method have been authenticated. Their callable metadata can
then be sealed without granting source/JIT/check capabilities. This is not a
transitive freeze of shared helpers, user factories, code objects, or other
closure referents, and it does not execute annotation providers.

Generated methods retain ordinary Python argument binding, bytecode, results
and exceptions. Their static signatures remain checker facts, not runtime
parameter/return contracts. There is no required constructor-entry delegate,
supplied/deferred mask or factory-result value-check site. Explicit arguments,
ordinary defaults and factory sentinels follow the stdlib's original behavior.

Factories run once at their original evaluation point and assignment order.
Their result reaches the ordinary attribute or native-slot write path, where
a selected field policy checks the actual stored value. With field checks
disabled, or when a generated initializer is explicitly called on an ordinary
foreign receiver, its annotation does not introduce a type error. An `InitVar`
is an ordinary argument forwarded to `__post_init__`, not instance storage.

Only selected field-write predicates require actual declaration-bound nominal
targets. Minimal GC-owned snapshots retain the target types needed by those
fields, not constructor-only or `InitVar` annotations. Inherited checked fields
retain their original declaring owners; unchecked base fields are not
retroactively upgraded by a child constructor. Neither mutable `Field.type`,
an evaluated annotation result nor a logical class reference alone supplies
runtime authority. Direct-self field targets remain reserved during pending
callbacks and bind once to the actual selected final class before admission.

Slot replacement requires separate original/replacement layout owners. Stock
dataclasses share generated/source methods between those types and repair
existing `__class__` cells, including the initializer's annotation provider;
they do not replace that provider's metadata. Those ordinary cell writes
cannot retarget already committed field-check targets. The linked pending
protocol avoids retargeting: original and replacement remain guarded, and only
the actual selected final type fills the still-unbound self predicates. This
applies to selected self-typed fields, not method parameters, receivers,
returns or `InitVar` arguments. Inherited base-self fields retain their original
declaring target. Both source consumers use this linked pending
protocol. The earlier retained early-enforced slots decline is historical, not
the current source construction path. Independent unsupported graph or retained
projection refusals still apply; no installed contract may be revoked.

The native slots bridge consumes the already evaluated metaclass, name, bases,
copied namespace and original class at its exact attested opcode. It prepares
one distinct replacement handle; it neither retains the original namespace
function nor impersonates `_add_slots` as that source function. The independent
slot projection compares inherited metadata with actual native object-field
catalogs, then validates copied operands, slot names and doc values without
Python equality, iteration or attribute callbacks. The original method birth
records are reused by identity, not consumed again or reassigned to the new
type. Frozen pickle helpers remain ordinary shared stdlib functions with
protected class bindings, not new source/JIT or generated-check owners.

Native interpreter completion runs after the actual Apply activation and its
consumed operands retire. It prepares and revalidates member evidence for the
actual selected result without requiring the original type to remain alive.
The exact replacement construction and native member births supply authority,
not a recovered original address or a copied namespace alone. Final source
publication admits that result before disposing any surviving unselected
provisional type through the same resolved native lineage. Weak inventories do
not keep either type alive and never select a final result on their own.

The retained early-enforced path currently publishes both original/replacement
member owners and finalizes their already protected classes separately. That
older path is not evidence of the pending interpreter handoff or its lifetime
guarantees. In either path, failed application removes only its exact pending
records and must not revoke an installed native type contract.

Generated bodies need no checked-frame activation, checked vectorcall delegate,
or factory-value bridge. Supported `PyFunction_SetVectorcall` changes retain
ordinary call behavior; storage checks remain on the actual attribute, slot
and dictionary writes. Source creation and transformation records still
authenticate construction independently of the generated body's call path.

For:

```python
@dataclass
class Point:
    x: int
    y: int = 1
```

use stable dictionary-prefix storage when that capability is installed.

For:

```python
@dataclass(slots=True)
class Point:
    x: int
    y: int = 1
```

inspect and authorize the actual final replacement type's member descriptors.
The adapter must pass a linked replacement-specific native-slot construction
contract into that replacement's actual type allocation. Preserve the
original class's separate dictionary-bearing contract, as well as both the
original and replacement callback sequences and any escaped original
instances.

`frozen=True` concerns instance assignment, not class immutability. Preserve
the generated frozen-instance setter rather than replacing it with raw writes.

### Pydantic

For a Pydantic model, checker facts can identify logical model fields and
method signatures, but the default runtime class remains dynamic because:

- `ModelMetaclass` is not the builtin `type` metaclass.
- Its field registry is produced after its internal call to `type.__new__`.
- Validation can replace the entire real instance dictionary.
- Attribute assignment can invoke validation and coercion.
- Forward-reference completion and rebuilding can mutate the class later.
- A declared field can shadow an inherited method.
- Cached and deprecated descriptors have observable dictionary/descriptor
  behavior.

A future verified Pydantic adapter could participate by:

1. Supplying the offline canonical field contract to `ModelMetaclass` before
   the actual superclass type allocation.
2. Preserving the metaclass's custom namespace and callbacks.
3. Installing compatible owner-aware physical storage before
   `PyType_Ready`.
4. Comparing the final `__pydantic_fields__` catalog against the contract.
5. Preserving field aliases, `extra="allow"`, `__pydantic_extra__`,
   `__pydantic_private__`, `model_fields_set`, `model_post_init`, computed
   fields, descriptors, assignment handlers, validators, and inherited-field
   collisions.
6. Supporting exact-dictionary replacement under a genuinely compatible
   normalization or adapter-owned layout policy.
7. Completing and freezing all incompatible class rebuilds before publishing
   frozen capabilities, or keeping those classes dynamic.

Without all required adapter guarantees, Pydantic fields and methods use the
ordinary Python path. The surrounding strict module still benefits from
independently enforceable module and function capabilities.

### Other frameworks

Django field descriptors, SQLAlchemy instrumentation, attrs transformers, and
custom class decorators cannot be classified solely by their metaclass:

- Some framework classes use the builtin `type` metaclass.
- Some decorators instrument an otherwise ordinary class after creation.
- Some mappings and reverse relationships mutate classes after module import.
- Some non-data descriptors rely on ordinary instance-dictionary shadowing.

Automatic policy therefore excludes unrecognized decorators, unsafe bases,
unverified descriptor owners, framework instrumentation markers, and
noncooperating metaclasses before selecting an irreversible strict layout.

Unknown classes remain dynamic without requiring annotations throughout the
repository.

## Extending ty for strict semantics

### Shared language policy

The checker and runtime consume the same project-level strict policy and
source future-feature marker:

```python
from __future__ import strict
```

The checker must understand that strict-module semantics apply only where the
runtime loader can authenticate and enforce the matching module contract.

Ordinary modules and unsupported dynamic classes do not silently inherit
strict restrictions.

### Module binding diagnostics

Teach `ty` the strict module lifecycle:

```text
initialization:
    ordinary initial binding and rebinding are allowed

sealed:
    existing final bindings cannot be rebound or deleted
    lexically declared mutable globals remain writable
    supported previously absent names may be appended once
```

Examples:

```python
from __future__ import strict

LIMIT = 1
LIMIT = 2  # allowed: still executing module initialization


def invalid() -> None:
    globals()["LIMIT"] = 3  # strict-final-global-rebind


def allowed() -> None:
    global mutable_count
    mutable_count += 1
```

Resolve direct writes, `global` statements, imported module attribute writes,
`globals()` aliases, `module.__dict__`, known `function.__globals__`, and
constant-key dictionary updates when their targets are known.

Unknown aliases still require runtime enforcement. Lack of a diagnostic is
never proof that an arbitrary effect is safe.

### Class mutation diagnostics

For a participating strict class:

```python
class Bar:
    def baz(self) -> int:
        return 1


def invalid() -> None:
    Bar.baz = replacement  # strict-class-mutation
    del Bar.baz            # strict-class-mutation
    Bar.new_member = 1     # strict-class-mutation
```

Class-body assignments, recognized decorators, and approved initialization
callbacks remain legal before class sealing.

The checker must distinguish:

- Runtime-frozen participating classes.
- Automatically dynamic framework classes.
- Known initialization-only code.
- General functions that can execute after sealing.
- Cross-module accesses to an already sealed strict owner.

### Instance assignment and shadow diagnostics

The checker consumes the same name policies installed at type construction:

```python
class Bar:
    count: int

    def baz(self) -> int:
        return self.count


def invalid(value: Bar) -> None:
    value.baz = lambda: 3  # strict-instance-method-shadow
    value.count = "wrong"  # strict-incompatible-field-write if checked
```

The `strict-incompatible-field-write` diagnostic applies only when `count`
has an explicitly enabled runtime `CheckedInstanceField` contract. Without
that capability, a normal checker may still report an ordinary static type
mismatch, but SOAC must not claim that the runtime field value is protected.

Add explicit handling for:

- Inherited method shadowing.
- Final-method shadowing through an instance.
- `ClassVar` instance writes.
- Undeclared fields only when the runtime class has a closed-field policy.
- Declared fields intentionally overriding inherited non-data methods.
- Compatible dataclass and framework-generated fields.
- Literal `setattr` and `object.__setattr__` calls.
- Recognizable `vars(instance)` and `instance.__dict__` aliases.
- Replacement of a strict fixed-layout instance dictionary.

Dictionary insertion under a protected method name is not itself invalid if
the runtime policy intentionally permits the entry but ignores it during
attribute lookup.

Current `ty` coverage does not fully diagnose inherited-method shadowing by
instance assignments. Strict-mode rules must close that gap rather than
assuming the existing general-purpose checker already proves it.

### Finality and inheritance diagnostics

Ordinary `typing.final` is advisory in Python. This proposal upgrades it to
runtime-enforced finality only when the shared project policy explicitly
selects `typing_final_policy = "enforce_for_participating_classes"` and the
actual class constructor installs the corresponding barrier. An explicit
existing strict-runtime finality declaration can provide the same capability.

Under that policy, rules must reject:

```python
@final
class Base:
    ...


class Child(Base):  # strict-final-class-subclass
    ...
```

and:

```python
class Base:
    @final
    def method(self) -> None:
        ...


class Child(Base):
    def method(self) -> None:  # strict-final-method-override
        ...
```

The shared CPython type-construction and base-reassignment paths must enforce
the same restrictions for dynamically created subclasses, ordinary class
statements, custom metaclasses, `type(...)` calls, and supported native type
factories. The participating SOAC constructor alone is insufficient because
ordinary subclasses do not pass through it. If the shared barrier cannot cover
a particular creation path, final-method dispatch requires an exact receiver
proof instead.

Incompatible physical inheritance, method ABI changes, descriptor-kind
transitions, and unsafe mutable base classes also produce contract errors or
automatic dynamic classification.

### Diagnostic catalog

Suggested rule identifiers:

```text
strict-final-global-rebind
strict-final-global-delete
strict-class-mutation
strict-final-class-subclass
strict-final-method-override
strict-instance-method-shadow
strict-classvar-instance-write
strict-undeclared-field
strict-incompatible-field-write
strict-incompatible-override
strict-dict-replacement
strict-unsupported-metaclass
strict-unsupported-decorator
strict-unsupported-descriptor
strict-layout-inheritance-conflict
strict-unchecked-dynamic-type
strict-construction-contract-mismatch
```

Definitive violations prevent publication of a strict class contract.
Uncertainty conservatively removes the affected capability or leaves the
class dynamic according to project policy.

Suppressing a checker diagnostic cannot make a runtime invariant true.
Suppressed, unresolved, or dynamically typed regions must not silently retain
the corresponding optimization capability.

The offline exporter implements one explicit framework fallback for ty's
structured `unresolved-attribute` diagnostic. For a locally classified dynamic
framework receiver or an actual user-defined metaclass, it retains the
diagnostic as a visible `strict-unchecked-dynamic-type` warning and replaces
affected attribute values and consuming attribute/call proposals with
Unknown/dynamic facts. Classification uses the actual inferred receiver and
resolved class/MRO identities, not diagnostic prose or lexical containment.
An arbitrary mutable base, `Any`, or an ignored region is not sufficient.
Ordinary ty diagnostics are unchanged; candidate-class attribute mistakes,
unresolved names, incompatible declared writes, and strict finality/mutation
violations still block publication. This fallback supplies no runtime class,
checked-value, layout, or direct-entry capability.

### Class-transform support

Extend `ty` or its exporter to expose structured facts for:

- Ordinary class annotations and bound methods.
- Standard `@dataclass`, including inherited fields and generated methods.
- `ClassVar`, `InitVar`, keyword-only fields, defaults, and default factories.
- Recognized `dataclass_transform` participants.
- Framework field aliases and canonical runtime storage names.
- Custom or unsupported metaclasses.
- Descriptor results and callable instance fields.
- Final methods, final classes, and override relationships.

`dataclass_transform` is a static typing promise, not proof that a framework
actually constructs the predicted layout. The participating runtime adapter
must still verify the actual final class and field catalog.

### Checker implementation boundaries

Make the changes in the checker's structured semantic pipeline rather than
postprocessing formatted diagnostics or matching source text:

1. Ruff's future-feature parser recognizes the authenticated SOAC strict
   dialect.
2. The vendored `__future__` typing stub exposes the feature in that dialect.
3. `ty_project` builds the complete configured project/environment database.
4. `ty_python_semantic` class-definition inference classifies strict owners,
   metaclasses, decorators, inherited members, and logical fields.
5. Its existing dataclass/`dataclass_transform` call-binding logic supplies
   transform-aware logical field and generated-signature facts.
6. Its assignment/deletion/attribute inference checks the same lifecycle,
   protected-name, finality, and checked-value policy installed by SOAC.
7. Its diagnostic registry publishes the new strict rule identifiers.
8. A structured exporter emits deterministic source-bound shards directly
   from those semantic facts.

The relevant upstream source areas are `ty_project/src/db.rs`,
`ty_python_semantic/src/types/infer/builder.rs`,
`ty_python_semantic/src/types/call/bind.rs`,
`ty_python_semantic/src/types/diagnostic.rs`, and
`ruff_python_parser/src/semantic_errors.rs`. The exact revisions of all
Ruff-family crates must remain compatible.

The current maintained export path implements the two final-global rules,
class mutation, instance-method shadowing, ClassVar instance writes,
final-class subclassing, final-method overrides, incompatible overrides, and
policy-gated incompatible field writes. These nine rules use actual checker
symbol/member/call queries and the shared resolved language policy. The broader
catalogue remains a runtime/checker integration target: arbitrary aliases,
native callbacks, physical undeclared-field restrictions, and unsupported
framework construction do not gain static enforcement claims. Ordinary `ty`
checking remains isolated from the explicit SOAC export path.

## Typed IR and optimization plans (deferred)

These are future consumers of enforced contracts. The current milestone does
not implement or require these plans, optimized code generation, or associated
optimization-shape acceptance tests. Authenticated runtime policy metadata and
actual object bindings remain required independently of any IR consumer.

### Validated capabilities

Represent runtime facts explicitly:

```rust
enum RuntimeSemanticCapability {
    FinalGlobal {
        module: StrictModuleInstanceId,
        binding: GlobalBindingId,
    },
    StableInstanceField {
        class: StrictClassId,
        field: StrictFieldId,
        storage: VerifiedFieldStorage,
    },
    CheckedInstanceField {
        class: StrictClassId,
        field: StrictFieldId,
        value: CheckedTypeId,
    },
    VirtualMethod {
        family: StrictDispatchFamilyId,
        slot: MethodSlotId,
        abi: VerifiedCallAbi,
    },
    DirectMethod {
        class: StrictClassId,
        method: StrictMethodId,
        target: FrozenFunctionCapability,
    },
    CheckedSignature {
        function: FrozenFunctionCapability,
        signature: CheckedSignatureId,
    },
}
```

The offline artifact proposes potential capabilities; authenticated runtime
construction and sealing publish the actual capabilities.

### Selected operations

Extend the validated optimization plan with operation shapes such as:

```rust
enum TypedAttributeAccessPlan {
    Generic,
    StableIndexedField {
        class: StrictClassId,
        field: StrictFieldId,
        index: u32,
        presence: FieldPresencePolicy,
    },
    NativeObjectSlot {
        class: StrictClassId,
        field: StrictFieldId,
        offset: u32,
        presence: FieldPresencePolicy,
    },
    DescriptorCall {
        owner: StrictClassId,
        descriptor: VerifiedDescriptorId,
    },
}

enum TypedMethodCallPlan {
    Generic,
    CallableInstanceField {
        field: TypedAttributeAccessPlan,
        abi: DynamicPythonCallAbi,
    },
    Virtual {
        family: StrictDispatchFamilyId,
        slot: MethodSlotId,
        receiver: VerifiedReceiverCapability,
        arguments: VerifiedCallArgumentPlan,
        argument_proofs: Vec<CheckedValueProof>,
        abi: VerifiedCallAbi,
    },
    Direct {
        target: FrozenFunctionCapability,
        receiver: VerifiedReceiverCapability,
        arguments: VerifiedCallArgumentPlan,
        argument_proofs: Vec<CheckedValueProof>,
        abi: VerifiedCallAbi,
    },
}
```

Codegen consumes the selected plan mechanically. It does not rerun type
inference, inspect mutable Python dictionaries, infer offsets from annotation
order, or decide that a likely method target is frozen.

Existing SOAC guarded indexed-field and direct-call representations provide
useful integration points, but they do not currently provide verified strict
virtual dispatch or this checked construction contract.

### Guarded fallback

A strict function may accept an ordinary receiver without a checked-signature
contract:

```text
if receiver_has_verified_Bar_capability(value):
    return strict_dispatch(value)
else:
    return ordinary_python_call(value, "baz")
```

The guard is located before the dependent operation and must preserve
evaluation order. It is a normal explicit fast/slow dispatch choice, not
runtime revocation of a published strict module.

Within an already checked strict-to-strict region, a dominating receiver proof
can eliminate the guard.

## Artifact loading, cache identity, and publication

### Loader

Before lowering or importing an opted-in module:

1. Read the actual source bytes.
2. Resolve its canonical module identity.
3. Authenticate the matching offline shard and source digest.
4. Validate Python version, platform, CPython ABI, checker revision, policy,
   and relevant dependency fingerprints.
5. Reject stale or incomplete strict artifacts before executing user code,
   unless the configured fallback still authenticates the strict future and
   fully enforces every promised strict module/global contract while merely
   withholding optional class/type optimization capabilities.
6. Attach authenticated class and function contracts to the actual interpreter
   module/code construction context; a SOAC IR consumer is optional future work.
7. Pass construction handles through the actual class-definition path.

The source future feature and class facts must be captured before existing
lowering rewrites remove annotated assignments or future imports.

A missing type-fact shard can never downgrade a `from __future__ import
strict` module into ordinary unprotected execution. If a mandatory checked,
frozen, or physical-layout policy cannot be constructed without the artifact,
the module fails closed before any user instruction.

Do not import or execute otherwise unnecessary modules merely to validate
their cached dependency fingerprints.

### Runtime publication ordering

Already installed policies must apply during module initialization and
reentrant callbacks. For classes constructed during initialization, no
seal-dependent capability may be consumed before:

```text
module initialization
    + final decorators
    + final class adoption
    + class/function freezing
    + module SEALED
```

Circular imports, reentrant class callbacks, and functions invoked during
module initialization use safe interpreter behavior while honoring every
already installed field policy and mutation restriction.

Any retained eager SOAC compilation path may produce generic code, but cannot
consume nonexistent sealed class/function capabilities. Such compilation is
not required to demonstrate this interpreter milestone.

For a factory-defined class created after module sealing, use the final
class's separate verified/sealed lifecycle instead of waiting for another
module seal.

### Cache fingerprints

Include at least:

```text
module source hash
strict future-feature state
offline artifact schema
ty revision and exporter revision
effective project and per-module policy
Python version, platform, and CPython ABI
resolved stubs and dependency fingerprints
class source identities
actual verified class/metaclass/decorator identities
actual installed storage and inherited write policies
physical field prefixes and method-family slots, only if installed
frozen function/code/signature identities
selected checked-field policies
every consumed external strict capability
```

Identical Python source under a different checker configuration, class
contract, adapter, or CPython build must not reuse incompatible enforcement
metadata. The same rule applies to native plans if optimization is resumed.

## Implementation phases

These phases implement interpreter enforcement only. They supersede the prior
physical-layout/dispatch and measurement phases; there is no automatic follow-on
optimization phase.

### Phase 0: contract and policy alignment

Update `doc/STRICT_MODULES.md` and `OPT_GOAL.md` to agree on:

- Automatic capability-based class classification.
- Real dictionary-bearing strict classes.
- Preserving actual standard-dataclass behavior.
- Selected field-write semantics without function-level runtime type checks.
- Excluding SOAC traceback reconstruction and frame inspection without losing
  exception semantics, safe cleanup or authenticated construction.
- Excluding SOAC tracing/profiling/monitoring compatibility and mandatory
  observer refusals, without weakening independent safety/enforcement barriers.
- Protected-method lookup and dictionary-shadow behavior.
- The explicit supported native C API boundary.

No implementation may rely on incompatible assumptions remaining elsewhere
in the documented strict contract.

The offline analyzer must also understand the strict dialect. Extend the
matched Ruff parser's recognized future-feature set, `ty`'s module policy,
and the corresponding `__future__` typing stub to accept
`from __future__ import strict` only in the authenticated SOAC analysis mode.
The currently pinned parser otherwise rejects unknown future features before
strict diagnostics or artifact export can run. Preserve ordinary Ruff/CPython
diagnostics outside that mode and include the dialect/parser version in the
artifact fingerprint.

Upgrade or backport Python 3.15 checker/parser/typeshed support and the
conservative narrowing options as part of the same coordinated toolchain
change.

### Phase 1: offline facts

Implemented for the supported semantic subset by the standalone `tools/ty`
driver and its pinned vendored checker commits: deterministic module shards, stable
source identities, conservative per-file checker configuration,
source/config/dependency fingerprints, logical class fields, method binding,
inheritance/finality, standard dataclass facts, and the nine rules above.
Cross-module plain strict bases use the same semantic classifier and recursive
MRO queries as local proposals, including scoped suppression and source-change
invalidation. Known dynamic bases propagate dynamic participation. Foreign
dataclass/transform bases still require an explicit per-file adapter-policy
context before they can become candidates; an importer's policy is not reused
as the defining file's authority. None of these proposals replaces runtime
matching of actual protected base objects.
Publication reuses unchanged content-addressed shards; persistent incremental
checker databases and complete ecosystem adapters are not implemented.

First export source facts and compare them with runtime observations; do not
generate native assumptions from the initial artifact.

### Phase 2: explicit type construction

Preserve offline contracts through authenticated native compilation/loading
and bind their type references to actual Python objects.

Pass explicit authenticated construction handles through the interpreter class
path. Implement the protected CPython type-construction entrypoint and install
the native pending-instance barrier before `PyType_Ready` and callbacks.
Validate the actual final decorated type and install selected
name/value/mutation policies before instance admission. Preserve actual
source-requested dictionaries and slots; new fixed-prefix layouts and method
tables are not required.

Support ordinary builtin-metaclass classes first. Unknown metaclasses and
decorators automatically remain dynamic.

### Phase 3: runtime barriers

Implement module/type/function protections, attribute read/write policies,
owner-aware instance-dictionary mutation, whole-dictionary replacement rules,
`__class__` protections, and the chosen supported C API boundary.

Tests must prove behavior through both Python syntax and actual native C API
entrypoints, including warmed specialized bytecodes, before publishing the
corresponding contract. Generic fallback is sufficient when it enforces the
same policy.

### Phase 4: checked fields and ordinary calls

Implement opt-in checked-field writes for the supported normalized type subset
and the optional storage-owned state representation. Remove all function-level
runtime type enforcement and its policy/proof consumers. Preserve ordinary
binding, body results, exceptions and cleanup through SOAC, the entry
interpreter, CPython and supported C calls. Dataclass generation does not
introduce parameter or factory-result checks. Field checks apply at the actual
storage mutation; check elimination remains deferred.

### Phase 5: recognized transforms and compatibility

Add an authenticated standard-dataclass adapter, including ordinary and
`slots=True` forms, replacement classes, generated functions, and inherited
layouts.

Test automatic dynamic fallback for unsupported Pydantic, Django, SQLAlchemy,
and other framework classes without per-class SOAC annotations. Developing new
framework-specific optimization adapters is not required. No adapter may
publish stronger policies without actual construction/validation support.

### Phase 6: end-to-end interpreter acceptance

Run source through the real offline checker and startup-authenticated loader,
construct actual runtime objects, and exercise the acceptance matrix below
with SOAC JIT execution disabled. Record the source/artifact/runtime identities,
installed policies, expected rejection, dynamic fallback, and observed Python/C
behavior. Include structured positive/negative assertions on contracts and
actual bindings, not rendered IR or expected optimized instruction shapes.

Run `just test-all` before submitting implementation changes and report any
remaining compatibility gaps. Neither pyperformance measurements nor optimized
native-code/JIT-coverage evidence is a completion requirement. Existing
performance results remain historical evidence, not evidence of enforcement.

## Acceptance tests

The primary end-to-end tests execute the actual strict interpreter path with
SOAC JIT execution disabled; flags or synthetic metadata alone are not strict
authority. Native specialized paths must be exercised where supported, even
though new optimization work is deferred. Preserve ordinary-Python controls
for behavior not intentionally changed by the strict contract.
The execution-lifetime clarification also applies to retained SOAC paths:
exact transient counts and CPython instruction-dependent release order are not
acceptance gates. Keep checks for ownership safety, required cleanup, correct
scoping, source-level effects and the installed contracts. A mismatch limited
to the excluded observations is not a reason to add another bytecode decoder
or native ownership-recipe framework.

SOAC traceback reconstruction, native frame-slot correspondence, frame
inspection and frame-retention parity are likewise outside acceptance scope.
No new helper-owned/omitted-slot proof is required just to populate a native
traceback frame. Preserve ordinary exception propagation/chaining, semantic
bindings, suspension and cleanup tests independently of those observations;
ordinary CPython traceback/frame controls retain their normal behavior.

SOAC tracing/profiling/monitoring event fidelity, observer enablement/refusal
and refusal-error-shape assertions are not acceptance gates either. Do not
replace excluded observer behavior with new mandatory rejection tests. Keep
mixed tests' ordinary computation, construction, mutation, exception and
cleanup assertions, using in-scope entrypoints where needed; ordinary CPython
observer controls and installed-contract checks remain valid.

### Artifact integrity

Cover:

- Same source with different strict policy.
- Changed `ty` revision, Python version, platform, and CPython ABI.
- Changed project configuration, per-file override, search path, and stub.
- Changed consumed dependency.
- `Any`, `Unknown`, ignored errors, unresolved imports, and unsafe narrowing.
- Stable content-derived source identities across independent checker runs.
- Forged, untrusted, replayed, or partially published artifact generations.
- Incremental module-shard updates.
- Repeated execution of one lexical class definition.

### Type construction

Cover:

- Native pending protection and actual requested storage present during
  descriptor `__set_name__` and inherited `__init_subclass__`.
- Callback attempts to create instances rejected while pending; retaining a
  class reference remains allowed.
- Layout-compatible `__class__` reassignment into a pending type rejected
  before object changes, through Python and supported native paths.
- Actual final decorated type validated and selected field/type/name policies
  installed before instance creation is enabled.
- Metaclass `__prepare__`, `__new__`, and `__init__` behavior.
- Rejected or dynamic noncooperating metaclasses.
- Recognized decorator mutation.
- Decorators returning a replacement class.
- Repeated factory-created runtime classes from one source location.
- A factory-created class whose module had already sealed.
- A decorator mismatch after the provisional class escaped through a callback.
- Replayed, stolen, cross-module, or expired construction handles.
- Mismatched actual base, descriptor, decorator, or field catalog.
- No temporary namespace attributes or thread-local compiler metadata.

### Attribute behavior

Cover:

- Inherited field checks and actual source-requested storage.
- Multiple-inheritance policy conflicts and safe automatic fallback.
- Missing, initialized, deleted, and reinserted fields.
- Class-data defaults without synthetic dictionary entries.
- Subclass-specific class defaults.
- Ordinary instance-dictionary insertion order and non-string keys.
- Dictionary growth without losing installed write/name policies.
- `__dict__` replacement and retained dictionary identity.
- Protected method lookup with an ignored same-name dictionary entry.
- Identical protected lookup through `object.__getattribute__`,
  `PyObject_GenericGetAttr`, both method helpers, and specialized bytecodes.
- `setattr` and `object.__setattr__` rejecting protected method names.
- Declared fields overriding inherited non-data methods.
- Data-descriptor getter and setter precedence.
- Read-only properties and descriptor-raised exceptions.
- Mutable descriptor-type transitions.
- Standard `cached_property` miss, hit, assignment, deletion, and recompute.

### Python and native mutation paths

Cover:

- Module/class attribute assignment and deletion.
- `globals()`, module `__dict__`, and function `__globals__`.
- Instance dictionary item assignment and every supported bulk operation.
- `PyDict_SetItem`, deletion, merge, and supported direct raw stores.
- `PyType_GetDict` followed by supported direct class-dictionary mutation.
- `PyObject_GenericSetAttr` and `object.__setattr__`.
- Warmed adaptive `STORE_ATTR`, `LOAD_ATTR`, and method-call bytecodes.
- Managed `__dict__` replacement.
- `__bases__` and `__class__` assignment.
- Function `__code__`, defaults, and keyword defaults.
- `PyFunction_SetDefaults`, `PyFunction_SetKwDefaults`,
  `PyFunction_SetClosure`, and in-place keyword default dictionary mutation.
- Compatible `PyFunction_SetVectorcall` replacement and behavior-changing
  unsupported replacements.
- Documented unsupported immutable-dictionary `PyDict_Clear` behavior.
- Reentrant finalizers, custom key equality, weak references, and GC.

### Ordinary function calls and dispatch

Cover:

- Annotation-mismatched arguments, defaults, variadic values and successful
  results retaining ordinary Python behavior, through SOAC, entry-interpreter,
  CPython and supported C callers.
- Ordinary missing/duplicate/unexpected-argument errors, including correct
  precedence before body execution.
- Body exceptions, `finally`, caller handlers, closure/default identity and
  required cleanup without argument/return check activations.
- An annotated method executing its earlier effects before a protected field
  write rejects, with the old stored value unchanged.
- Ordinary generated constructor, `InitVar`, explicit factory-sentinel and
  factory-result behavior when no selected protected storage is written.
- The same constructor rejecting only at an actual checked dictionary/native
  slot write, including after factory effects and through escaped dictionaries.
- Source/creation ownership and sealed method metadata without value-type
  authority; copied metadata must not impersonate a construction owner.
- Method versus callable-field binding; instance, static and class methods;
  ordinary override dispatch and nonparticipating subclasses.
- Runtime-enforced final classes and methods, and method lookup before argument
  evaluation.
- No parameter/return proof or check-elimination nomination remaining merely
  because an annotation or authenticated function identity exists.
- No seal-dependent capability used before module/class sealing.

### Dataclasses and frameworks

Cover:

- Ordinary dataclass fields, defaults, defaults factories, and class defaults.
- `ClassVar`, `InitVar`, keyword-only fields, and inherited fields.
- Actual user-requested `slots=True` replacement identity and callbacks.
- Slotted subclasses inheriting a real instance dictionary.
- `frozen=True` assignment rejection and generated initialization.
- Dataclass descriptor-typed fields.
- Dataclass-generated function adoption and shared foreign helpers.
- Ordinary standard-library `cached_property` behavior.
- Pydantic model fields colliding with inherited methods.
- Pydantic cached/computed/deprecated descriptor behavior.
- Pydantic validation, coercion, and dictionary replacement.
- Deferred Pydantic class rebuild.
- Django and SQLAlchemy automatic dynamic fallback.

Every test should distinguish a checker prediction, the authenticated
construction contract, the realized runtime object, and installed enforcement.
A checker fact or a correctly shaped artifact without actual interpreter
enforcement does not prove correctness. Optimized plans, direct/virtual calls,
check-elimination decisions, and benchmark improvements are not acceptance
requirements for this milestone.

## Source references

Repository integration points:

- `crates/soac_lowering/src/driver.rs`: the retained SOAC path rewrites annotated
  assignments and future imports early; preserve source-bound facts first.
- `crates/soac_lowering/src/passes/ast_to_ast/rewrite_class_def/mod.rs`:
  retained lowered `__soac__.create_class` invocation.
- `soac_py/src/soac/runtime.py`: retained class namespace preparation and
  metaclass invocation.
- `crates/soac_jit/src/strict_interpreter.rs`: authenticated CPython module
  construction and original-code execution, without SOAC lowering or JIT.
- `crates/soac_jit/src/strict_interpreter/call_join.rs`: actual native call
  operands for class construction and supported transformations.
- `crates/soac_jit/src/strict_interpreter/callbacks.rs`: authenticated original
  function births and definition-boundary completion.
- `crates/soac_jit/src/strict_class.rs`: shared pending/final-admission binding
  for the native interpreter and retained SOAC source consumers.
- `vendor/cpython/Python/bltinmodule.c`: native `__build_class__` namespace and
  metaclass path.
- `vendor/cpython/Objects/typeobject.c`: actual type allocation,
  `PyType_Ready`, callback order, type mutation, and dictionary replacement.
- `vendor/cpython/Objects/object.c`: generic attribute lookup and assignment.
- `vendor/cpython/Objects/dictobject.c`: authoritative dictionary mutation
  and managed-dictionary behavior.
- `vendor/cpython/Python/bytecodes.c`: adaptive attribute loads, stores,
  method lookup, and specialized operation bypasses.
- `vendor/cpython/Lib/dataclasses.py`: generated functions and
  `slots=True` replacement classes.
- `crates/soac_core/src/block_py/name_gen.rs`: existing persistent module and
  function identities.
- `crates/soac_ir_typed/src/typed.rs`: current guarded direct-call plans.
- `crates/soac_ir_typed/src/plan_v3.rs`: current indexed-field plans and
  serialized module identities.
- `doc/STRICT_MODULES.md`: strict module lifecycle and interoperability.
- `OPT_GOAL.md`: current enforcement-only goal, approved compatibility policy,
  and deferred optimization reference.

Primary upstream references:

- [ty configuration](https://docs.astral.sh/ty/reference/configuration/)
- [ty command-line reference](https://docs.astral.sh/ty/reference/cli/)
- [ty typing FAQ](https://docs.astral.sh/ty/reference/typing-faq/)
- [Python dataclasses documentation](https://docs.python.org/3/library/dataclasses.html)
- [Python descriptor guide](https://docs.python.org/3/howto/descriptor.html)
- [Pydantic model documentation](https://docs.pydantic.dev/latest/concepts/models/)
