---
title: "Late-bound profiled instance-field specialization"
---

# Late-bound profiled instance-field specialization

- Status: **LANDED / RETAIN; FULL CORRECTNESS GATE PASSED**.
- Pacific date: 2026-08-18 PDT.
- Baseline revision: integrated `main` change `tsrrtrqm`, commit `49d31d74`,
  including the retained profiled exact-float expression fusion.
- Baseline artifact:
  `work/pyperformance/comparison-20260818-224902-FQ1Uij/summary.json`.
- Baseline paired stock/SOAC geometric score: **0.46849285308325195x** over
  eight normally sampled workloads; the complete pyperformance target remains
  at least **1.10x** stock CPython.
- Hypothesis: retain structurally validated class-owned profiled field plans
  during eager compilation, then bind their runtime owner/layout guards after
  the real class is created. Reach deterministic module-owned guard cells via
  the existing function-environment ABI, preserve weak owner lifetime and
  type-version invalidation, and bypass generic attribute lookup only for
  exact, unmodified layouts with original-operation fallback.
- Current outcome: three fresh zero-loss native profiles establish substantial
  generic attribute-access cost, and real Apply artifacts show existing
  profiled scalar regions being discarded for missing owner guards. The
  unchanged-production standalone regression genuinely fails **1 test in
  1.45 seconds** because four profiled slot/dictionary methods all record
  **zero Verify indexed hits**, while existing Profile evidence and every
  CPython behavior guard already pass. An independent unchanged-production
  optimizer regression also genuinely fails because **19 existing generic
  field-access observations** with no split-key layout produce completely
  empty per-function profile evidence. Existing generic-operation evidence
  retention and explicit source-keyed owner-field plan/typed-sidecar APIs are
  present in the in-progress implementation, and a full
  `cargo check -p soac_jit --tests` passes in **10.29 seconds** initially and
  **3.72 seconds** with the complete guarded emitter. The independent
  optimizer evidence regression now passes **1 test / 205 filtered** after a
  **28.40-second cold rebuild**. The first complete candidate integration
  initially failed with the same **four zero Verify indexed-hit counts**,
  despite all CPython behavior checks passing. A third independent
  real-source optimizer regression now confirms the precise immediate
  boundary: the supposedly structural class-owner catalog was completely
  empty because lowered class strings pass through local temporaries. A
  general single-definition immutable-local string propagation fix now turns
  that catalog regression GREEN **1 / 207**. The next runtime attempt now
  records **33 `Point.read` and 68 aggregated `Point.write` indexed hits**
  with every slot semantic guard passing. `Record.read` instead records
  **33 indexed fallbacks** and `Record.write` records **34 indexed
  fallbacks**, each with zero generic counters and zero indexed hits. The
  precise publication blocker is now proven: pinned CPython stores static
  builtin-base type dictionaries outside `tp_dict`, so scanning
  `object.tp_dict == NULL` incorrectly rejects the valid `Record` MRO and its
  type version remains zero. Using the exported, owned
  `PyType_GetDict(base)` API resolves this pinned-CPython boundary. The full
  Profile-to-Verify-to-Apply integration initially **passes 1 test in
  1.82 seconds**; after adding split deletion, dictionary promotion/mutation,
  and class-property invalidation coverage, the strengthened regression also
  **passes 1 test in 1.93 seconds**. All four slot/split methods specialize
  while every CPython guard is preserved. A separate **33.07-second PyO3
  rebuild** preceded the initial run. Three focused typed-plan validation
  tests now **pass 3 / 3** within a full typed-IR library gate of
  **53 / 53**; the complete optimizer library **passes 207 / 207**, and the
  strengthened actual-source optimizer
  owner-admission/dense-cell regression **passes 1 / 1**, and focused JIT
  portable-ABI/source-sidecar/precedence regressions **pass 2 / 2** within a
  broader JIT field family of **25 / 25**. Existing precompiled-code
  regressions also **pass 8 / 8**. Final
  `cargo check -p soac_jit --tests` **passes in 9.59 seconds**, and scoped
  formatting plus format checks pass for all three changed crates. A broad
  transformed-runtime suite spanning ten Python files also **passes 11 tests
  in 23.73 seconds**. However, the first fixed-eight release smoke exposes a
  real Apply-only constructor bug: stock and Profile pass all eight, while
  `deltablue` loses `Variable.mark` and `richards` loses `Packet.link`.
  Existing split-value stores assume the owner's shared-key entry was already
  primed; late-bound classes intentionally are not primed. A live
  `ht_cached_keys` expected-key/index guard plus generic first-insertion
  fallback is required. A new direct-constructor regression now genuinely
  **fails 1 test in 1.16 seconds** with Verify
  `AttributeError: UnseededRecord.first`, while Profile and existing split-key
  evidence pass; the repaired live shared-key guard then makes that exact
  strengthened integration **pass 1 test in 1.74 seconds**. Corrected real
  release `deltablue` and `richards` also now **both pass Profile and Apply**.
  Their single-shot Apply measurements include the same preexisting lazy-JIT
  cold compilation seen on earlier revisions and are not throughput evidence;
  representative benchmarks and the passing full correctness gate are
  recorded below.

## Hypothesis and source evidence

`soac_opt::pipeline_v3::indexed_field_requests_from_type_key_evidence_v3`
currently recognizes constant-attribute `GetAttr`/`SetAttr` operations by
matching existing profile `type_keys`. Mechanical field decisions subsequently
pass through
`soac_opt::access_emission_v3::prepare_indexed_field_accesses_for_codegen`,
which discards a plan if its runtime owner cannot be resolved at that moment.
Normal eager module compilation happens before executing the transformed
module/class bodies, so ordinary user-defined class objects do not yet exist
when their already-profiled methods are planned.

`__slots__` classes expose a separate structural gap: a class such as
`float.Point` stores `x`, `y`, and `z` in exact member-descriptor offsets and
does not generate instance-dictionary split-key `type_keys`. Existing
`field_access` operation/counter definitions still identify those real
constant-attribute sites, so adding a new profiling schema merely to recover
literal compiler-visible slots would be unnecessary.

The actual fused-float baseline contains
`typed_scalar_regions_invalidated_without_live_indexed_field_guards` events:

- `deltablue`: **20 events** across its measured worker directory;
  `chain_test` and `projection_test` each lose **two branch regions** per
  observed function event because their expected indexed-field guards are not
  available.
- `richards`: **20 events**; `HandlerTask.fn` and `WorkTask.fn` each lose
  **one branch region** per observed function event for the same reason.

These event totals span the benchmark directory's multiple worker processes;
they are not per-invocation counts. The existing correctness boundary is
appropriate: a scalar region must be rejected until its receiver field has a
real validated guard. The proposed change must make that guard available
without weakening scalar-region validation.

The untouched-production regression
`tests/test_late_bound_owner_fields.py` establishes a genuine semantic/
specialization RED: **1 failed in 1.45 seconds** at its Verify indexed-hit
assertion. Its existing Profile generic `getattr` / `setattr` rows, split-key
`Record` type-layout evidence, and absence of fabricated `Point` slot class
type/profile rows all pass. Both Profile and Verify already pass subclass,
descriptor, deleted-slot, old-value destructor, dynamic-class lifetime, and
other CPython-visible behavior checks. However, all four real methods report
zero indexed hits: **`Point.read = 0`, `Point.write = 0`, `Record.read = 0`,
and `Record.write = 0`**. This isolates the absent late-bound specialization
without labeling existing Python behavior incorrect.

An additional genuine structured optimizer RED,
`soac_opt::plan::tests::profile_evidence_preserves_hot_owner_field_sites_without_split_layout`,
fails **1 test with 205 filtered** after a **15.94-second initial Cargo
build**. Its existing source-keyed `field_access` /
`generic_getattr` observation count is **19**, but no split-dictionary
`type_keys` exists; current extraction consequently returns default empty
`FunctionProfileEvidence`. The regression proves that exact literal-slot
methods cannot become eligible until existing generic field-operation evidence
is retained independently of split-dictionary layout. After the generic rows
were retained, the exact structured test **passed 1 / 206** after a
**28.40-second cold rebuild**. Both build durations are workflow overhead,
not benchmark throughput.

The current, still-unverified implementation retains existing
`generic_getattr` / `generic_setattr` counts in the public
`soac_opt::plan::FunctionProfileEvidence::hot_field_accesses` field without
changing the counter-dump schema. It introduces explicit public
`soac_ir_typed::plan_v3::LateBoundOwnerFieldSpecializationPlan` and
`LateBoundOwnerFieldStorage` planning types, with distinct
`SplitDict { expected_index }` and `ObjectSlot` storage variants, plus the
crate-root `soac_ir_typed::TypedLateBoundOwnerFieldPlan` sidecar. These source
changes also expose the deterministic
`soac_opt::pipeline_v3::late_bound_owner_field_site_catalog` function, which
assigns dense source-keyed cell indices without depending on profile-record
iteration order. They establish the proposed explicit-plan direction, not a
passing test, working guard publication, or measured optimization.

The candidate catalog ties each owner to actual lowered class scope, the
class's literal qualified name, its actual `MakeFunctionWithClosure` method
identity, literal
slot declarations when present, and the method's first local receiver; only
actual class-namespace assignments contribute. Admission requires at least
**eight** existing generic field-access observations, avoiding code expansion
for one-off cold attributes. Split-dictionary sites still require existing
owner-specific profiled
`type_keys`; their proposed emission reuses the already guarded trusted inline
values field probe/store and original fallback. Mechanical plan emission and
validated typed sidecars are present. The first Cargo check exposed a missing
import and an attempted pre-name-binding `MakeFunction` shape that does not
exist at that stage; both source mistakes were corrected, and the combined
`cargo check -p soac_jit --tests` subsequently **passed in 10.29 seconds**
before the guarded-emitter changes and **passed again in 3.72 seconds** with
the complete emitter. Candidate behavioral tests remain pending.

The evolving runtime now stores deterministic owner-field sites and dense
`LateBoundOwnerFieldCell` entries directly in `SharedModuleState`. Each cell
contains atomic weak-reference, type-version, and slot-offset values; the
module owns the weak-reference objects rather than holding strong references
to owner classes. Depending on this repository's **pinned CPython internal
layout is intentional and explicitly acceptable**; modifying vendored CPython
is also permitted if a verified implementation genuinely requires it. The
distinct unsound boundary is that the pinned **PyO3 Rust FFI declaration** for
weak references is `repr(Rust)`, not `repr(C)`, so Rust's declaration cannot
be used to infer the actual pinned CPython C-struct offsets. The approved
design instead uses one explicit minimal `#[repr(C)] RawPyWeakRefForJit`
prefix for direct weak-reference target inspection while retaining weakref
object ownership separately in module state. CPython clears a dead weakref's
target to `Py_None`, not null, so a non-null check alone cannot establish a
live owner; the exact-type/version guard must reject the cleared target. The
existing real
`FunctionEnvAbiHeader` gains one
`late_bound_owner_cells` pointer, allowing compiled functions to find their
module's cell array without embedding a process-specific owner address or
introducing a duplicate ABI-layout mirror; seven existing header fixtures were
updated rather than preserving an incompatible shadow layout. An implemented
but still-unverified class-created publication callback now checks the exact
function/module owner and canonical generic attribute hooks, accepts only an
owner's own aligned `Py_T_OBJECT_EX` member descriptor for slots, rejects
read-only slot writes and any conflicting MRO class binding for dictionary
fields, and assigns/requires a nonzero type version before release-publishing
the weak-owner cell. A now-implemented but still-unverified guarded CLIF
emitter loads dense cells through the function environment, checks weak-owner
identity and the current type version, reuses existing split-dictionary
probes/stores, falls back for missing slots, and emits slot
INCREF/store/DECREF in CPython-visible order. It preserves original-callee
indexed hit/fallback counters and rejects cross-module plans. The complete
emitter passes the **3.72-second** follow-up Cargo check; typed inline-source
behavior and end-to-end execution remain unresolved. The first complete
Profile-to-Verify-to-Apply attempt remains RED at Verify with
`Point.read = Point.write = Record.read = Record.write = 0`; existing Python
semantics still pass, so the remaining defect is missing actual
catalog/plan/publication/consumption rather than an observed semantic change.
The new structured regression
`late_bound_owner_field_catalog_finds_static_slot_and_split_methods`
independently failed because the actual lowered-source class-owner catalog was
`[]`: class qualified names and keys are forwarded through immutable local
temporaries rather than always appearing as immediate constant expressions.
A general, name-independent propagation of **single-definition immutable
`LocalLocation` string constants** restores both slot and split sites, and the
exact real-source catalog regression now **passes 1 / 207**. Temporary debug
instrumentation was removed. The subsequent real Verify pass now records
**33 `Point.read` and 68 aggregated `Point.write` indexed hits**, with subclass
overrides,
property version invalidation, deleted slots, old-value finalizer order, and
dynamic-owner lifetime all correct. However, **`Record.read = Record.write =
0`**. `Record.read` nevertheless records **33 `indexed_fallback` branches**
and `Record.write` records **34 `indexed_fallback` branches**, both with
**zero generic counters**: split plans, typed sidecars, and guarded CLIF are
active, but every runtime owner/layout probe fails. For comparison,
`Point.read` has **33 indexed hits / 4 fallbacks**, while the individual
`Point.write` store site has **34 hits / 2 fallbacks**; the earlier **68-hit**
figure aggregates multiple field sites within that method. The concrete split
publication cause is now source-proven: `_testcapi.type_get_version(Record)`
is **0** after class creation, whereas `Point` has a nonzero assigned version;
the candidate rejects the `Record` MRO because the pinned CPython's static
builtin `object` type stores its dictionary out of the object and therefore
has **`object.tp_dict == NULL`**. Vendored
`Objects/typeobject.c:530-552` describes that layout. The implemented fix uses
the exported **owned-reference `PyType_GetDict(base)` API** and releases each
temporary dictionary while preserving the no-user-callback class-binding
guard. With this correction, the complete standalone
Profile-to-Verify-to-Apply regression **passes 1 test in 1.82 seconds**: all
four `Point` / `Record` reads and writes produce indexed hits and every
existing subclass, descriptor, deletion, finalizer, and weak-lifetime check
passes. A separate **33.07-second PyO3 debug-extension rebuild** is setup
overhead, not benchmark throughput.

After that verified first GREEN, the standalone fixture was strengthened to
add split-dictionary `Record.value` deletion/reinsertion, explicit `__dict__`
materialization and mutation, non-string-key dictionary promotion/removal,
and `Record.value` property replacement/deletion/restoration invalidation.
The strengthened full Profile-to-Verify-to-Apply regression also **passes 1
test in 1.93 seconds**, preserving indexed hits for all four methods alongside
both the new split-dictionary boundaries and all earlier slot/subclass/
descriptor/finalizer/class-lifetime checks.

The complete typed-IR library suite now **passes 53 / 53**, including three
focused validation/mechanical-emission regressions that **pass 3 / 3** and
cover valid slot/split owner selections, rejection of
duplicate/incomplete owner identities, and mechanical mutation boundaries.
The strengthened actual-source optimizer catalog regression
also **passes 1 / 1**, proving valid slot/split admission, deterministic dense
cell indices, and rejection of staticmethods, dynamic owners, and inherited
slots. Two focused JIT regressions also **pass 2 / 2**:
`late_bound_owner_field_abi_is_state_relative_and_c_layout` validates actual
function-environment, owner-cell, and C-layout weakref offsets;
`late_bound_owner_field_typed_plans_preserve_sources_and_existing_indexed_precedence`
proves original function/instruction provenance, slot-store and split-load
storage/access, deterministic cell indices, and preservation of existing
resolved `IndexedField` precedence required by scalarization. Full package
The full optimizer library now **passes 207 / 207**; a broader JIT indexed/
owner-field family **passes 25 / 25**, including existing scalarization/index
guards, and precompiled-code regressions **pass 8 / 8**. The final combined
JIT/test-target Cargo check **passes in 9.59 seconds**, and package-scoped
format/format-check passes for `soac_ir_typed`, `soac_opt`, and `soac_jit`.
The broad transformed-runtime Python compatibility run also **passes 11
tests across ten files in 23.73 seconds**, covering the strengthened owner
regression plus fused floats, fixed unpacking, captured builtins, synthetic
metadata/closure caching, iteration exceptions, counter shutdown, private
class handling, and scalar cleanup. Switching from Cargo checks to pytest
also triggered a separate **45.02-second PyO3 debug-extension rebuild**;
that reproducible toolchain churn is workflow debt, not measured candidate
throughput.

### Release-workload constructor correctness failure

Frozen release candidate `zssttuox/b2db07d9` fails its first actual
fixed-eight smoke,
`work/pyperformance/comparison-20260819-002306-A5VqGp`. Stock CPython and
the transformed **Profile pass each complete all eight workloads**. The
subsequent **Apply** pass fails only `deltablue` and `richards`; their
structured worker events reveal the exact suppressed exceptions:

- `deltablue`: `AttributeError: 'Variable' object has no attribute 'mark'`.
- `richards`: `AttributeError: 'Packet' object has no attribute 'link'`.

Both attributes are first initialized in the respective class constructors.
The existing `emit_trusted_inline_values_field_store` checks only that the
dictionary is unmaterialized, its inline values are valid, and the profiled
index is within capacity; it assumes the expected key already exists in the
owner's shared-key table. Older indexed-field setup could prime that table,
whereas the new late-bound design correctly avoids user-visible class/field
priming. A constructor can therefore store into a valid numeric value slot
before the corresponding current `ht_cached_keys` entry exists; later generic
attribute lookup cannot resolve the missing key despite the raw value write.

The added focused regression uses `UnseededRecord` with
`__static_attributes__ = ()` so CPython does not preseed its shared-key table;
direct `__init__` calls establish at least **48 generic Profile setters** and
valid existing type-key records before Verify attempts the unseeded first
insertion. Against the frozen candidate it genuinely **fails 1 test in
1.16 seconds** with `AttributeError: 'UnseededRecord' object has no attribute
'first'`, after Profile succeeds. An initial fixture iteration incorrectly
expected 48 Profile counts while constructor-entry attribution produced only
three; invoking `__init__` directly and delaying `__dict__` materialization
produced the valid, representative RED.

The proposed compatibility-preserving repair is to verify the **live owner
`ht_cached_keys` entry exists at the profiled index and matches the exact
expected key**, otherwise execute the original generic attribute store so
CPython registers first-insert keys itself. Loads must also validate the
current key/index identity before trusting a value slot. The implemented
late-bound split guard now requires a non-null live `ht_cached_keys`, exact
split-key kind, **`expected_index < dk_nentries`**, and pointer identity
between the actual interned key and expected attribute **before either a
direct load or store**. Missing or mismatched keys execute the original
generic operation, allowing CPython to register first-insert keys without
invoking user callbacks or priming a class. The exact strengthened standalone
constructor regression now **passes 1 test in 1.74 seconds** across Profile,
Verify, and Apply. Both previously failing real release workloads now also
**pass Profile and Apply**. Their debug-single Apply measurements,
approximately **345 ms for `deltablue` and 328 ms for `richards`**, are
dominated by known first-call lazy `soac.runtime.exception_matches` JIT
compilation; the integrated preceding revision already measured
**325.961 ms / 349.897 ms** under the same cold single-shot protocol, and an
earlier fixed-unpack smoke measured approximately **346 ms / 332 ms**.
An initial interpretation comparing these cold Apply values with warm Profile
values as a catastrophic regression was incorrect and is explicitly
retracted. These are correctness smoke results, not steady-state throughput;
representative candidate sampling and the passing full correctness gate are
recorded separately below.
The benchmark driver
reported only `Benchmark died`; complete exceptions were recovered from each
worker's structured `soac.module_load` event.

### Fixed-eight source census

An independently parsed AST census of the installed pyperformance sources
finds class-owned `self` attribute operations in **five** of eight workloads:

| Benchmark | Classes | `self` attribute loads | `self` attribute stores | Literal `__slots__` |
| --- | --- | --- | --- | --- |
| `chaos` | 3 | 93 | 16 | none |
| `comprehensions` | 3 | 4 | 3 | none |
| `deltablue` | 13 | 107 | 38 | none |
| `float` | 1 | 12 | 9 | `Point: ("x", "y", "z")` |
| `richards` | 14 | 40 | 47 | none |
| `fannkuch` | 0 | 0 | 0 | none |
| `nbody` | 0 | 0 | 0 | none |
| `spectral_norm` | 0 | 0 | 0 | none |

These are static AST-node counts, not measured execution counts or proof that
every method/property access is safely eligible. Some listed `self` loads are
method lookups, class attributes, descriptors, inherited fields, or dynamic
operations that must remain generic. In particular, slots-only eligibility
initially applies only to the literal `Point` class; dictionary-field
admission additionally requires matching existing `type_keys`.

### Fresh zero-loss native baseline profiles

All three captures use the integrated fused-float revision on **8 CPUs /
12 GiB / Linux 6.8.0-137**. Perf-record CPU sample counts and exported
Speedscope sample/weight counts are separate bases. Percentages below are
inclusive and overlapping; they must not be summed or interpreted as
candidate speedups.

**`float`:** `work/logs/late-owner-fields-float-baseline_*` captures
**547 cpu-clock samples with zero lost**, **8.838 MB**, and a separate
Speedscope export of **279 sampled stacks / 100,031 weights**. A **50-loop**
attached-profiler replay records **56.19716898 ms per loop**, which is
diagnostic rather than a normal pyperformance headline. Inclusive shares:

- `Point.__init__`: **20.842%**; `Point.maximize`: **20.471%**;
  `Point.normalize`: **18.099%**.
- `PyObject_GetAttr`: **18.094%**;
  `_PyObject_GenericGetAttrWithDict`: **15.535% inclusive / 10.24% self**.
- `PyObject_SetAttr`: **10.787%**;
  `_PyObject_GenericSetAttrWithDict`: **7.313% inclusive / 5.30% self**.
- Slot/member costs: `member_get` **4.569%**, `PyMember_GetOne` **2.740%**,
  and `member_set` **1.828%**.
- `_Py_Dealloc`: **11.516%**; `PyFloat_FromDouble`: **2.744%**.

**`deltablue`:** `work/logs/late-owner-fields-deltablue-baseline_*` captures
**458 cpu-clock samples with zero lost**, **7.418 MB**, and **361 Speedscope
stacks / 99,906 weights**. Its **400-loop** diagnostic replay records
**5.59792144 ms per loop**. Inclusive shares:

- `PyObject_GetAttr`: **21.617%**;
  `_PyObject_GenericGetAttrWithDict`: **16.815% inclusive / 9.83% self**.
- `PyObject_SetAttr`: **3.273%**;
  `_PyObject_GenericSetAttrWithDict`: **2.400% inclusive / 1.53% self**.
- Generated/application function-family matches: `Planner` **28.810%**,
  `BinaryConstraint` **21.175%**, `UrnaryConstraint` **16.808%**,
  `ScaleConstraint` **9.821%**, `chain_test` **45.862%**, and
  `projection_test` **28.377%**. These overlapping family/stack shares must
  not be summed.

**`richards`:** `work/logs/late-owner-fields-richards-baseline_*` captures
**735 cpu-clock samples with zero lost**, **11.813 MB**, and **424 Speedscope
stacks / 99,967 weights**. Its **70-loop** diagnostic replay records
**53.42452429 ms per loop**. Inclusive shares:

- `PyObject_GetAttr`: **24.625%**;
  `_PyObject_GenericGetAttrWithDict`: **20.816% inclusive / 12.24% self**.
- `PyObject_SetAttr`: **6.802%**;
  `_PyObject_GenericSetAttrWithDict`: **4.898% inclusive / 2.59% self**.
- Application matches: `HandlerTask.fn` **19.590%**, `WorkTask.fn` **2.721%**,
  the broad `Task` frame-name family **71.291%**, and `Packet` **11.292%**.
  Family matching and call-stack inclusion overlap.

## Proposed implementation and compatibility boundary

- Derive same-module class/method ownership structurally from lowered
  class-scope `MakeFunctionWithClosure` bindings, not a guessed qualified-name
  string,
  benchmark name, mutable global lookup, or user-visible temporary attribute.
- For ordinary instance dictionaries, retain only existing validated
  constant-attribute field plans backed by profiled `type_keys`; for literal
  `__slots__`, derive exact eligible member names structurally from immutable
  class-body slot declarations and existing source-keyed `field_access`
  operations. Do not invent a new profile record schema or treat method
  lookups as instance-data fields.
- Allocate a dense deterministic guard-cell table owned by the transformed
  module/shared state. Compiled code references a cell by deterministic index
  through its owning `FunctionEnvAbiHeader`; never embed an absolute owner
  pointer or host-specific cell address in serialized/precompiled machine
  code. Append/reuse explicit ABI offsets rather than introducing a duplicated
  private layout mirror.
- Publish each cell only after the actual compiler-proven class has been
  created. Keep only a **weak owner reference**, the verified type/version,
  field storage kind, and exact slot/key metadata; process-retained module
  state must not strongly retain classes or create module/type/function
  ownership cycles. Specializing against the pinned CPython's verified C
  layouts is allowed, but the PyO3 weakref declaration is `repr(Rust)` and
  therefore cannot supply reliable C offsets; use an explicit minimal
  `RawPyWeakRefForJit` C-layout prefix with the weakref object itself
  separately owned by module state. Reject dead weakrefs whose target is
  `Py_None` rather than null. Cells remain safely empty until publication.
- Validate owner identity and version at each call. Require exact receiver
  type and canonical CPython generic `tp_getattro` / `tp_setattro` hooks;
  validate the exact member descriptor or recorded split-dict layout without
  invoking user callbacks. Installing a property, custom descriptor,
  `__getattribute__`, `__setattr__`, subclass override, or incompatible class
  attribute must invalidate the old assumption and preserve generic Python
  behavior.
- **Guard lifetime:** a module-owned cell is valid only while its weakref
  still points to the receiver's **exact** live class and that class retains
  the captured **nonzero** `tp_version_tag`. Slot loads additionally require
  a non-null current field pointer. Split-dictionary operations additionally
  require an unmaterialized dictionary, valid inline values, a valid profiled
  index/capacity, and a present value for loads. Class death/redefinition,
  descriptor/hook replacement, version changes, slot deletion, dictionary
  materialization/promotion, or incompatible inline storage immediately use
  the original generic operation. There is no time-based validity window,
  process-global guard table, or intentional strong owner lifetime extension.
- Never prime classes or descriptors through user-visible callbacks merely to
  make a field cell eligible. Reject dynamic/redefined same-name classes,
  inherited fields, custom metaclass behavior, cross-module or ambiguous
  ownership, nonliteral slots, and unresolved/dynamic attributes initially.
  Preserve original same-module method provenance through validated inlining;
  do not bind a copied/inlined operation to its caller's unrelated owner.
- A fast load may directly read the proven exact slot or valid split-dict
  indexed value, retaining correct owned-reference semantics. Missing slots,
  deletion, promoted/materialized dictionaries, owner/version/layout mismatch,
  or unavailable cells must execute the original `PyObject_GetAttr` fallback.
- A fast store must **INCREF the replacement before swapping**, publish the
  new field value, then **DECREF the old value after publication** so its
  destructor observes the replacement. Preserve null/new-slot bookkeeping,
  descriptor/watcher behavior, active exceptions, and the original
  `PyObject_SetAttr` fallback whenever the exact safe storage contract is not
  established.
- Preserve existing `field_indexed_hit` / `field_indexed_fallback` Verify
  counters, Profile's generic execution, captured builtins, synthetic closure
  code/metadata, deterministic shutdown counter flushing, fused exact-float
  expressions, fixed unpack, and iteration-exception shadowing semantics.
- The planned standalone regression exercises ordinary split-key `Record`
  and literal-slot `Point`, expects Profile to retain generic rows and no
  fabricated Point `type_keys`, requires Verify indexed hits for reads and
  writes, and covers subclass hooks, deleted slots, replacement-value
  visibility during old-value destruction, class property invalidation,
  dynamic same-name class weakref lifetime, staticmethod boundaries, and
  same-module method usage. The unchanged-production RED is verified:
  **1 failed in 1.45 seconds**, with all four Verify indexed-hit values zero;
  candidate implementation and GREEN remain pending.

## Benchmark protocol and measurements

- Fixed same-resource baseline set:
  `chaos,comprehensions,deltablue,fannkuch,float,nbody,richards,spectral_norm`.
- Baseline:
  `work/pyperformance/comparison-20260818-224902-FQ1Uij/summary.json`; one
  normal paired round and **20 Apply values per benchmark**. Its baseline
  itself contains large unrelated `chaos` and `spectral_norm` outliers, so
  medians, paired repeated runs, significance, and unchanged-code controls
  must accompany mean comparisons.
- Candidate normal fixed-eight comparison, targeted two-round affected-workload
  comparison, and three candidate zero-loss native profiles are complete.
  The full `just test-all` correctness gate also passes.
- The eight-workload subset is not the complete pyperformance acceptance
  suite; a subset improvement cannot establish the **1.10x full-suite** goal.

| Benchmark | Baseline paired stock mean | Baseline SOAC mean | Baseline SOAC median | Baseline maximum | Stock / SOAC | JIT functions | Candidate |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `chaos` | 29.6565830 ms | 95.8049075 ms | 67.7506635 ms | 254.2656790 ms | 0.309552x | 34 | pending |
| `comprehensions` | 13.2127568 us | 81.4481895 us | 81.9890215 us | 89.1668848 us | 0.162223x | 21 | pending |
| `deltablue` | 1.9761771 ms | 4.4544285 ms | 4.4528580 ms | 4.7748157 ms | 0.443643x | 78 | pending |
| `fannkuch` | 190.5359085 ms | 258.1756106 ms | 258.4381105 ms | 268.2722470 ms | 0.738009x | 1 | pending |
| `float` | 33.9854952 ms | 54.5013413 ms | 52.1108010 ms | 75.9883830 ms | 0.623572x | 9 | pending |
| `nbody` | 47.6133705 ms | 65.8389979 ms | 64.9692905 ms | 72.8604950 ms | 0.723179x | 8 | pending |
| `richards` | 22.0335961 ms | 39.9469926 ms | 39.3747665 ms | 42.5434360 ms | 0.551571x | 53 | pending |
| `spectral_norm` | 48.3164991 ms | 85.1424164 ms | 55.9885530 ms | 468.5579270 ms | 0.567479x | 9 | pending |

Baseline paired stock score: **0.46849285308325195x**. Baseline
previous-revision mean geometric ratio **0.9036520196614041x** was already
distorted by the `chaos` / `spectral_norm` outliers. The prior fused-float
strategy's cleaner two-round eligible-workload medians improved `float`
**1.04687x** and `nbody` **1.07153x**; any new field strategy must preserve
that retained behavior instead of attributing those earlier gains to itself.

### Measured candidate and compatibility tradeoffs

The complete same-hardware fixed-eight candidate is
`work/pyperformance/comparison-20260819-003845-YpCssx`. Its previous-SOAC
mean geometric speedup is **1.1168014648x**, but the outlier-resistant median
geometric speedup is only **1.0403088969x**. The measured-subset stock score
moves from **0.4684928531x to 0.5127524705x**; this is neither stock parity
nor progress proven across the full pyperformance suite. Prior `chaos` and
`spectral_norm` outliers inflate mean improvements, and the candidate stock
`float` measurements are themselves contaminated (median **60.77 ms**, maximum
**147.26 ms**); do not claim that SOAC beats stock on `float`.

The independently repeated two-round, five-workload comparison is
`work/pyperformance/comparison-20260819-004559-eJ14ia`. Its mean and robust
previous-SOAC geometric ratios are **1.0277321881x** and
**1.0662174681x**. Repeated median ratios are `float` **1.2692356x**,
`chaos` **1.1122241x**, `deltablue` **1.0440702x**, `comprehensions`
**0.9624948x** (a reproducible regression), and `richards` **0.9713289x**
(also a regression). The full fixed-eight `float` and `deltablue` median
improvements are **1.2796561x** and **1.0524078x**. Repeated measurements
also contain large candidate `comprehensions`, `float`, and `richards`
outliers and stock `float` / `richards` outliers; targeted robust stock score
is only **0.4068266x** despite a **0.4855680x** noisy stock mean.

Coverage and typed IR remain unchanged at **3,069 blocks / 218 functions**,
but native code grows from **22,789,000 to 23,417,280 bytes (+2.757%)** and
machine blocks from **1,512,900 to 1,553,260 (+2.668%)**. PID-matched
workload native growth is `float` **+2.95%**, `deltablue` **+3.08%**,
`richards` **+5.88%**, `chaos` **+3.84%**, and `comprehensions` **+0.94%**.
The existing scalar-region invalidation events in deltablue
`chain_test`/`projection_test` and richards `HandlerTask.fn`/`WorkTask.fn`
remain **unchanged**: this strategy does not restore scalarized regions.

All three native captures are verified zero-loss; perf-record samples and
Speedscope stack/weight counts are distinct bases. For `float`, samples
**547→458**, stacks **279→260**, generic getter **15.535%→10.044%**, and
generic setter **7.313%→3.057%**. For `deltablue`, samples **458→553**,
stacks **361→413**, generic getter **16.815%→12.296%**, and generic setter
**2.400%→1.988%**. For `richards`, samples **735→632**, stacks **424→366**,
but generic getter rises **20.816%→23.735%** and generic setter rises
**4.898%→5.061%**. Inclusive shares overlap and must not be summed; attached
single-profile replay timings diverge from normal pyperformance for delta and
richards and are not throughput headlines.

| Generated-code / profiling metric | Baseline | Candidate |
| --- | --- | --- |
| Optimized typed-IR final basic blocks | 3,069 | 3,069 |
| Optimized typed-IR function instances | 218 | 218 |
| Pre-optimization serialized BlockPy bytes | 14,398,752 | 14,398,752 |
| Apply-mode emitted native bytes | 22,789,000 | 23,417,280 (+2.757%) |
| Apply-mode machine blocks | 1,512,900 | 1,553,260 (+2.668%) |
| `float` zero-loss perf CPU samples | 547 | 458 |
| `float` Speedscope sampled stacks / weights | 279 / 100,031 | 260 / 99,948 |
| `float` generic `PyObject_GetAttr` inclusive CPU | 18.094% | 11.354% |
| `float` generic `PyObject_SetAttr` inclusive CPU | 10.787% | 4.585% |
| `deltablue` zero-loss perf CPU samples | 458 | 553 |
| `deltablue` Speedscope sampled stacks / weights | 361 / 99,906 | 413 / 100,060 |
| `deltablue` generic `PyObject_GetAttr` inclusive CPU | 21.617% | 18.446% |
| `richards` zero-loss perf CPU samples | 735 | 632 |
| `richards` Speedscope sampled stacks / weights | 424 / 99,967 | 366 / 99,920 |
| `richards` generic `PyObject_GetAttr` inclusive CPU | 24.625% | 26.584% |
| New class-owned field plans / published guard cells | none | observed; total not recorded |
| Actual Verify indexed field hits for slots/dicts | Point read/write 0; Record read/write 0 | all four methods at least 16 |

Baseline per-function native comparisons must match actual measured Apply
worker PIDs, not the earlier Profile rows or independent perf-replay workers:

- `float`, Apply PID **62410**: **78,472 native bytes across 18 function
  rows**; `Point.normalize` **7,984 bytes / 552 machine blocks**,
  `Point.maximize` **7,216 / 478**, and `Point.__init__` **2,416 / 169**.
- `deltablue`, Apply PID **62287**: **420,460 bytes across 156 function
  rows**; `EqualityConstraint.execute` **1,456 / 92**,
  `BinaryConstraint.output` **1,884 / 125**, and `Plan.execute`
  **9,424 / 586**.
- `richards`, Apply PID **62566**: **317,876 bytes across 105 function
  rows**; `Task.runTask` **5,532 / 340**, `Richards.run`
  **180,048 / 12,632**, and `TaskState.isTaskHoldingOrWaiting`
  **1,952 / 117**.

## Attempt history

### Attempt 1: Establish class-owned late binding as a distinct strategy

- Change: propose source-proven same-module class/method ownership,
  existing `type_keys` for dictionaries, literal `__slots__` for exact member
  descriptors, deterministic module-owned late-bound guard cells reached via
  the function-env ABI, weak owner/version validation, and generic fallback.
- Evidence: baseline score **0.46849285x**, five class-bearing workloads,
  lost scalar guards in actual `deltablue` / `richards` workers, and three
  fresh zero-loss profiles exposing generic attribute load shares of
  **18.094% / 21.617% / 24.625%** respectively for `float`, `deltablue`,
  and `richards`.
- Compatibility: exact hooks/types/descriptors and per-invocation version
  guards; ownership-safe late publication; no user callback priming; portable
  indexed cells; INCREF-before-swap-before-DECREF store ordering; original
  fallback for deletion, overrides, subclasses, dynamic ownership, and
  unsupported layouts.
- Genuine unchanged-production RED: **1 failed in 1.45 seconds**; existing
  generic Profile rows, split-key `Record` layout, absent `Point` type rows,
  and all CPython Profile/Verify semantics already pass, but
  `Point.read` / `Point.write` / `Record.read` / `Record.write` each report
  **zero Verify indexed hits**.
- Genuine optimizer structural RED-to-GREEN:
  `profile_evidence_preserves_hot_owner_field_sites_without_split_layout`
  fails **1 / 206** because **19 `field_access` / `generic_getattr` samples**
  without `type_keys` incorrectly yield default empty
  `FunctionProfileEvidence`; its first Cargo build takes **15.94 seconds**.
  After the generic evidence is retained, the same test **passes 1 / 206**
  following a **28.40-second cold rebuild**.
- In-progress, unverified implementation: retain existing generic field rows
  through public `FunctionProfileEvidence::hot_field_accesses`; add explicit
  public `LateBoundOwnerFieldSpecializationPlan`,
  `LateBoundOwnerFieldStorage`, and crate-root
  `TypedLateBoundOwnerFieldPlan` APIs; expose deterministic
  `late_bound_owner_field_site_catalog` indexing and mechanically emit the
  validated owner/storage sidecar. Class provenance uses literal class scope,
  actual namespace assignments, real `MakeFunctionWithClosure` identity, and
  its first receiver; admission requires at least **eight** generic profile
  hits, while
  split storage retains existing `type_keys` and guarded probes. Module-owned
  weak cells use one real function-environment ABI pointer. The first Cargo
  check found a missing import and an unavailable earlier-stage
  `MakeFunction` representation; after correcting those source issues,
  `cargo check -p soac_jit --tests` **passed in 10.29 seconds** before the
  guarded-emitter addition. The emitter now implements weak-owner/version
  checks, existing split probes/stores, slot null fallback and
  INCREF/store/DECREF, original-callee counters, and cross-module rejection;
  its follow-up `cargo check -p soac_jit --tests` **passes in 3.72 seconds**.
  The structured evidence regression is GREEN. The first runtime attempt had
  four zero-hit methods; after the catalog correction, slot methods produced
  real hits while split methods initially remained zero. Correcting builtin
  MRO dictionary access then makes all four methods pass.
- Independent real-source catalog RED-to-GREEN:
  `late_bound_owner_field_catalog_finds_static_slot_and_split_methods` fails
  with **`catalog = []`**, explaining all four unchanged Verify zero-hit
  results. Actual lowered classes forward constant strings through local
  temporaries; general single-definition immutable `LocalLocation`
  propagation, without name-based heuristics, makes that exact structured
  regression **pass 1 / 207**.
- Intermediate real Verify activation: `Point.read` records **33 indexed hits**
  and `Point.write` records **68 aggregated indexed hits** across its field
  sites, with all slot subclass,
  descriptor, deletion, finalizer, and dynamic-owner checks passing.
  `Record.read` and `Record.write` initially remain at **0 hits**, but produce
  **33 / 34 indexed fallbacks**, respectively, and no generic branches. Thus
  the split plans execute but their runtime guards miss. Source-backed cause:
  pinned CPython static builtin `object` keeps its dictionary outside
  `tp_dict`; the MRO scanner sees null, rejects `Record`, and leaves its type
  version **0**. Switching to the exported owned `PyType_GetDict(base)` API
  preserves the binding guard without priming user code and activates both
  split methods.
- Full genuine runtime RED-to-GREEN: standalone
  `test_eager_late_bound_slot_and_split_fields_preserve_python_semantics`
  **passes 1 / 1.82 seconds** across Profile, Verify, and Apply, with indexed
  hits for all four slot/split read/write methods and all existing
  descriptor/subclass/deletion/finalizer/class-lifetime checks passing.
  A separate **33.07-second PyO3 rebuild** is setup overhead, not a
  benchmark result.
- Strengthened end-to-end runtime GREEN: the expanded full
  Profile-to-Verify-to-Apply fixture **passes 1 / 1.93 seconds**, adding
  `Record.value` deletion/reinsertion,
  materialized/mutated/non-string-key-promoted `__dict__`, and class-property
  replacement/deletion/restoration/version invalidation while preserving all
  four indexed-hit methods and prior CPython guardrails.
- Full typed-IR library GREEN: **53 / 53**, including focused typed-plan
  validation/mechanical GREEN **3 / 3** valid slot/split
  selections, duplicate/incomplete rejection, and mechanical mutation
  boundaries. The
  strengthened actual-source optimizer owner-admission/dense-index regression
  also **passes 1 / 1**, rejecting staticmethod, dynamic, and inherited
  owners. Focused JIT regressions also **pass 2 / 2**, proving portable
  `FunctionEnv`/owner-cell/C-weakref ABI offsets, original function/source,
  slot-store/split-load access and storage, dense indices, and existing
  resolved indexed-field precedence. Full-package and broad-suite outcomes
  are recorded below.
- Additional complete/guardrail Rust gates GREEN: full optimizer library
  **207 / 207**, existing and new JIT field/scalar guard family **25 / 25**,
  and precompiled-code regressions **8 / 8**; final JIT/test-target Cargo
  check **9.59 seconds** and scoped formatting/format-check pass for all
  three changed crates. The cross-strategy transformed-runtime suite also
  **passes 11 tests / 10 files in 23.73 seconds**. Cargo-to-pytest extension
  rebuild adds **45.02 seconds** of workflow overhead.
- First real fixed-eight release smoke RED:
  `comparison-20260819-002306-A5VqGp` completes stock/Profile **8 / 8**, but
  Apply loses constructor-created `Variable.mark` and `Packet.link`. Existing
  split inline-value first stores assume a primed shared-key entry, which the
  late-bound design intentionally does not provide. Planned correction:
  validate live `ht_cached_keys` expected key/index and generically insert
  missing keys.
- Genuine focused constructor RED: `UnseededRecord` sets
  `__static_attributes__ = ()`, establishes at least **48 existing generic
  Profile setter observations**, and then **fails 1 / 1.16 seconds** in
  Verify with missing `UnseededRecord.first`. An initial fixture miscount of
  **3 versus 48** exposed constructor-entry counter attribution; direct
  `__init__` calls and delayed dictionary materialization corrected the
  reproducer.
- Focused constructor RED-to-GREEN: the late split fast path now checks live
  cached-key split kind, expected index **below `dk_nentries`**, and exact
  interned-key pointer before trusting loads/stores; missing keys fall back
  to CPython's generic insertion with no callbacks or priming. The same
  strengthened Profile-to-Verify-to-Apply regression **passes 1 / 1.74
  seconds**, and both previously failing real release `deltablue` /
  `richards` workers now pass Profile and Apply.
- Cold-smoke interpretation correction: repaired debug-single Apply values
  **345.31 ms / 327.81 ms** are comparable to prior integrated **325.961 ms /
  349.897 ms** and older **346 ms / 332 ms**; all include first-call lazy
  `exception_matches` compilation. PID-matched worker events attribute
  **318.309 ms** of the deltablue value (about **92%**) and **266.677 ms**
  of the richards value (about **81%**) to that cold compilation. The initial
  catastrophic interpretation compared incompatible cold Apply and warm
  Profile values and is retracted. These cold values are not normal
  throughput evidence; representative comparisons are recorded separately.
- Post-repair focused validation GREEN: JIT owner ABI/source regressions
  **2 / 2**, existing/new indexed-field family **25 / 25**,
  `cargo check -p soac_jit --tests` **6.15 seconds**, and scoped JIT
  formatting/format-check. Earlier complete typed-IR **53 / 53**, optimizer
  **207 / 207**, precompiled **8 / 8**, and ten-file transformed-runtime
  Python **11 / 11** remain valid. Normal benchmarking and the full
  post-repair `just test-all` gate also pass.
- Complete post-repair correctness gate GREEN:
  `work/logs/late-owner-fields-test-all.log` records **1,217 Python nodeids
  across 84 batches / 8 workers, all passing**, plus Rust **soac_jit 553**,
  **soac_ir_typed 53**, **soac_lowering 371**, **soac_opt 207**, and
  **PyO3 8**. Runtime preparation takes **25.342 seconds**, Rust tests
  **83.475 seconds**, pytest **94.452 seconds** internally / **94.468
  seconds** externally, and the complete test phase **177.960 seconds**.
  The existing counter-dump batch alone takes **94.11 seconds**.
- Result: **LANDED / RETAIN**. The genuine
  unchanged-production integration RED, both structured RED-to-GREEN tests,
  and focused Profile-to-Verify-to-Apply GREEN are verified, including broad
  Rust/Python compatibility and precompiled-code guardrails. Real release
  Apply workloads nevertheless expose a genuine unprimed constructor-key
  correctness failure, independently reproduced by a focused Verify RED and
  repaired to focused GREEN with a live shared-key identity guard. Both
  formerly failing real release workloads also now pass Profile and Apply;
  representative candidate comparisons and the full `just test-all` gate
  are complete.

## Verdict and next action

- Verdict: **LANDED / RETAIN; FULL CORRECTNESS GATE PASSED**. Focused semantic
  regressions pass, the repaired real release workloads complete, and normal
  fixed-eight / repeated five-workload median ratios improve **1.0403x /
  1.0662x**. Explicit tradeoffs remain: `comprehensions` and `richards`
  regress, native code grows **2.757%**, and scalar invalidations remain
  unchanged. The measured stock score is only **0.5128x** on this subset;
  neither stock parity nor the full-suite **1.10x** goal is established.
- Transferable lesson: a profile-selected guard cannot be used until its
  owner type exists, and a strong process-retained owner reference can change
  Python shutdown/lifetime semantics. Preserve both explicit ownership and
  original descriptor behavior rather than weakening guard validation.
- Next action: investigate the unchanged scalar-region invalidations, reproducible
  `comprehensions` / `richards` regressions, and generated-code growth.
