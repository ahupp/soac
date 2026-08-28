# Strict integration test migration

## Single-file scenarios

New source-level cases can use the [single-file scenario format](STRICT_SCENARIO_TESTS.md)
under `tests/strict_scenarios/`: module sections define one analyzed project,
then `ok`/`raise` blocks run in independent authenticated processes. A `raise`
expectation covers only the final statement; module setup and preceding
statements cannot satisfy it. The format guide also records the baseline
migration inventory, separating direct candidates, required extensions and
native/tooling tests that should retain their existing purpose.

## Why the old matrix is not evidence

The initial audit found 298 delimiter cases under `tests/integration_modules`
and five under `tests/simple`. Of those, 300 validation tails declared a
`validate_module` or `validate` function that the helper never called; the
other three used top-level checks. Source bodies still ran, so this was missing
validation, not proof that every old test did nothing. The same exec-only
dispatcher and callers exist in baseline commit
`4232685e1e86c0767156a2b52e29f6461fec62eb`.

None of the 303 original sources opted into strict mode. With correct ordinary
module admission, their old in-process `soac` and `entry` variants either ran
stock code or failed obsolete runtime assumptions. Mode labels and import-hook
allow-lists are not execution evidence.

The repaired dispatcher calls one declared synchronous validator exactly once,
or executes a top-level validation tail. Test flags exist only in validation
globals, never as writes into a sealed module. The global exception-text-based
failure-to-xfail hook is removed. Legacy in-process strict modes now fail before
source execution; they are not renamed into passing stock variants.

## Explicit observation xfails — 2026-08-26 (PDT)

Keep reviewed SOAC frame inspection, traceback-shape and exact implicit-finalizer
order expectations as explicit per-case `xfail`s. Do not classify a failure by
its filename or exception text. Ordinary CPython observations, exception
propagation/chaining, explicit callback order, ownership safety, complete cleanup
and installed contracts remain normal gating tests. Split mixed tests before
marking only the excluded observation.

The five frame-only delimiter programs retain their original stock controls.
Their ten SOAC/entry variants are collected with explanatory `xfail(run=False)`
marks instead of silently omitted. These legacy variants have no authenticated
strict-admission path; their non-running xfails are documented exclusions, not
observed runtime failures or proof of admission. The frame-free capture, walrus
and exception-cleanup companions continue to run normally. Existing traceback
and locals-inspection controls that execute ordinary CPython stay unmarked.

The isolated rebind/delete finalizer-order probes run against real authenticated
SOAC and entry fixtures. Source/type ownership, entry witnesses, subprocess
success, data shape, explicit callbacks and exactly-once eventual cleanup must
pass first. Only the final comparison against the original CPython event order
raises the narrowly typed exception allowed by their xfail mark. An incidental
match reports XPASS (`strict=False`); it does not establish an exact-order
guarantee. Existing semantic and stock cleanup tests remain unchanged.

The focused verification reports 36 passes, ten non-running frame xfails and
two finalizer-order XPASS outcomes (`work/logs/excluded-observation-focused-v1.json`
and its JUnit XML). Native/runtime identity postchecks pass. The collection
regression first reproduced the silently omitted cases before the marks were
added. No full-suite rerun was performed for this test-policy change.

## Runner and first checkpoint

Use `tests._strict_integration.create_strict_project` with explicit selected
modules whose source contains production `# soac:` rules or inherits package
rules. The helper inserts neither a future import nor a strictness config file.
`StrictProject.run_case` launches a fresh native-startup-configured interpreter,
checks authenticated module readiness and the selected binding-seal state
against source path and artifact generation, optionally checks registered
native function witnesses, then runs ordinary validation after initialization.
Stock controls keep their original sources. The earlier checkpoints below
retain their historical future/configuration-era evidence.

When retaining a publication for replay, use the same repository pytest
entrypoint for publication and execution. Launcher variables such as
`LD_LIBRARY_PATH` are authenticated inputs; publishing through a different
manual launcher can correctly reject the entire replay before initialization.
Give each replay run a unique `PYTEST_DEBUG_TEMPROOT` under `work/pytest`,
rather than sharing an explicit `--basetemp`. The repository's parallel runner
launches separate pytest processes; each must obtain its own numbered base
beneath that run root. For example, from the guest repository root:

```bash
mkdir -p work/pytest
strict_replay_root="$(mktemp -d "$PWD/work/pytest/strict-replay.XXXXXX")"
PYTEST_DEBUG_TEMPROOT="$strict_replay_root" \
  PYTEST_ADDOPTS="-o tmp_path_retention_count=10000" \
  just pytest-fast tests/test_strict_import_admission.py::test_dependency_changed_after_loader_construction_blocks_later_admission
```

The increased retention count keeps completed worker bases available for the
run's evidence collection. Preserve the same environment when publishing and
replaying; never pass one common `--basetemp` to concurrent workers. The retained
final-run wrapper follows this rule by creating `work/pytest/<log-stem>` once,
setting `PYTEST_DEBUG_TEMPROOT`, and rejecting an explicit `--basetemp`.

Keep the actual numbered fixture directories (for example,
`<run-root>/pytest-of-<user>/pytest-<number>/<fixture>`), not just pytest's
`current` symlink. Other runs then have separate retention roots instead of
deleting these publications through shared `/tmp/pytest-*` cleanup. Do not
modify or move an already analyzed project: its absolute paths are signed.
Pytest marker declarations live in the root config so repository-owned replay
drivers under `work/` can import reviewed tests without unknown-marker warnings.

When pre-publishing a fixture for later replay, use the same `_pytest-run`
entrypoint for publication and execution. `just --command` is not an equivalent
launcher: the pytest recipe also sets `LD_LIBRARY_PATH`. A retained v10 batch
published through the other entrypoint correctly rejected all 52 outcomes at
startup, before any selected initializer ran. Its public authority and failure
logs remain under `work/strict-v10-keyword-cohorts/rejected-launcher`. The
successful retry re-ran the genuine checker under the replay entrypoint; it did
not copy environment values from an artifact or weaken environment validation.
Normal fixtures that publish inside pytest already use the correct boundary.

Function witnesses now require the authenticated actual public entry kind,
not merely the requested mode. Module-only cases also need evidence of the
initializer's actual entry. A Rust-only, immutable initializer observation is
wired immediately before its public call and exposed through module diagnostics;
its regression checks the unset, executing, and sealed states on a module with
no source functions. The updated runner requires this observation after the
corrected v8 extension build. The genuine initializer regression now passes
in both requested function modes, observing the same initializer path.

The initializer's explicit lowering mode is `Interpreted` in both requested
source-function modes. Its required observation is therefore exactly
`entry_interpreter`, never an accept-either check. Module-only cases have **one
observed execution path**; synchronous source-function witnesses separately
require `checked_native` or `entry_interpreter` as requested. An initial v8
test expectation incorrectly required native initialization in the compiled-
function setting. That run was interrupted after 27 expectation failures and
one pass; its log and partial per-case journal are preserved under
`work/strict-initializer-observation/initial-expectation`. No runtime policy was
changed to satisfy the correction.

The five reviewed simple cases are explicitly enumerated in
`tests/test_simple_integration_cases.py`:

| Case | Coverage | Required function witnesses |
| --- | --- | --- |
| `simple_00_empty_module` | Empty module, no visible placeholder names | None |
| `simple_10_globals` | Initial scalar and container bindings | None |
| `simple_20_operators` | Module arithmetic | None |
| `simple_30_conditionals` | Module branch and binding | None |
| `simple_40_functions` | Function call and nested call | `add`, `double` |

Only the actual strict future is added to these strict variants; their remaining
bodies and the stock files are unchanged. The genuine offline publication
passed for all five. An earlier unchanged-source publication correctly failed
because TOML inclusion alone is not strict opt-in. The five stock validators
and 14 focused harness/workflow tests pass. All ten requested-mode strict runs
also passed against the staged v7 extension in 217.27 seconds, including native
seal/source/generation checks and the two function witnesses. The first case
included checker preparation; this is correctness evidence, not a performance
measurement. A later native entry-pointer audit found eager compilation could
override the requested interpreter mode. Those results prove strict admission
but not both execution paths; a rerun with exact public-entry diagnostics is
required. That corrected v8 rerun now passes all ten outcomes, with exact
initializer observations and native/interpreter entries for `add` and `double`.
The other four simple cases remain one-path initializer/behavior coverage.

Evidence: `work/logs/strict-simple-v7-checker-publication.log` (negative),
`strict-simple-v7-checker-opt-in.log` (five published),
`strict-simple-v7-runtime.log` (ten actual strict runs),
`integration-harness-guards-after.log`, and
`integration-harness-and-stock-simple.log`.

## Source-bound runtime entry cases

The 26 behavior tests formerly using ordinary-source inspection helpers in
`crates/soac_jit/src/jit/test.rs` now live in
`tests/test_strict_entry_runtime.py`. Their existing source bodies and behavior
assertions are preserved, with explicit strict opt-in. They cover public
function entries, class/metaclass construction, generator/coroutine creation,
closures/comprehensions, imports, mutation, control flow, exception/finally
ordering, and context managers. The exact old test suffixes remain the new
case IDs. Six raw binder/frame-ownership kernels and the pure IR/deopt tests
remain in Rust; inspection-only execution helpers are removed.

The original metaclass case initially exposed genuine unresolved-attribute
errors for semantically framework-dynamic receivers. Checker patch 0008 keeps
those diagnostics visible as warnings and demotes every affected attribute/
call fact; it does not suppress unrelated errors or grant checked/optimization
authority. The original case has no added casts or ignores.

All **52 named requested-mode outcomes pass** on the fixed v7 extension with
the genuine 26-source publication. Each case independently creates/imports
its selected module and checks an actual native function owner and registered
metadata before its behavior assertions. To avoid repeating the complete
26-module startup authentication 52 times, these independent cases share one
subprocess per mode. Their validation locals remain separate, and each case
checks for changes to builtin bindings, import hooks, search paths, working
directory, and previously imported selected modules. Any contamination prevents
later cases from being reported as passes. This explicitly gives up per-case
process isolation, not native authentication or individual failure reporting.

Evidence: `work/logs/strict-entry-runtime-v7-batched.log` records **52/52 PASS
in 436.31 s**. The earlier unbatched run was deliberately stopped after two
passes because each new process spent roughly a minute in repeated constructor
input hashing. The bounded production follow-up is tracked separately in
`optimization-attempts/2026-08-21-strict-loader-construction-snapshot.md`.
The same eager-compilation audit invalidated the claim of distinct entry-
interpreter coverage. These passes still establish genuine strict admission and
the behavior assertions. The tests now require the actual authenticated public
vectorcall classification before and after validation: `entry_interpreter` or
`checked_native` for synchronous witnesses, and explicitly `generator_factory`
for suspended-function factories. This last classification does not by itself
prove a particular resumed-body execution path. The corrected v8 rerun passes
all **52/52** outcomes with these exact witnesses. Its 23 synchronous cases
exercise each public entry mode; the three suspended-function cases establish
factory entry and behavior, not a resumed-body mode.

## Expression and control-flow cohort

These 23 source/validation pairs have been read individually and accepted by the
actual checker with only explicit strict opt-in. They are enrolled in
`tests/test_integration_cases.py`; the first mode-authenticated runtime run is
**44/46 PASS**, with one source-binding defect reproduced in both modes.
Preserve genuine
checker diagnostics; do not add casts, suppressions, or fake runtime authority
to make a case eligible. Each positive migration must retain a stock control,
an explicit strict source, actual ownership evidence, and both execution modes.

| Case | Proposed function witnesses | Review note |
| --- | --- | --- |
| `assert_shadowing` | `trigger` | Explicit lexical `global AssertionError` permits later writes/deletion; verify the reserved absent name. |
| `bounded_loop` | `bounded_loop` | Recomputed loop guard; no namespace mutation. |
| `chained_comparison` | `value`, `probe` | Single evaluation; mutations are to a list's contents. |
| `chained_comparison_side_effects_once` | `value`, `probe` | Existing duplicate shape; do not count it as independent semantic coverage. |
| `compare_in_while` | `loop_compare` | Both comparison outcomes. |
| `float_literal_precision` | None | Module literal precision and comparison. |
| `for_else_break_minimal` | `collect_for_else_break_minimal` | Nested empty-loop else/break. |
| `for_else_continue` | `collect_for_else_continue` | Else/continue executes the outer loop. |
| `for_else_continue_minimal` | `collect_for_else_continue_minimal` | Reduced two-iteration version. |
| `for_loop_carried_local` | `run_plain`, `run_getitem` | Carried locals and subscript reads. |
| `for_loop_empty` | `run` | Empty loop body remains unexecuted. |
| `for_loop_no_else` | `exercise` | Existing future-annotations flag must be preserved. |
| `fstring_debug_conversion` | `format_debug` | Debug conversion of a string. |
| `fstring_format_shadow` | `format`, `trigger` | User global named `format` must not change f-string formatting. |
| `fstring_ifexpr_interpolation` | `pluralize` | One of the three top-level validation tails. |
| `listcomp_unbound_target` | `run` | Checker accepted; preserve the actual `UnboundLocalError` assertion. |
| `map_unpack` | None | Module assignment/unpack success, exception branch unused. |
| `map_unpacking_module` | `summarize` | Tuple result and builtin map/len calls. |
| `match_guard` | `probe` | Pattern guard with and without an iterator method. |
| `maybe_unbound_join_not_loaded` | `run` | Checker accepted the original correlated branches without suppressions. |
| `named_expr_while_not` | `walk_until_truthy` | Named expression and loop test order. |
| `slice_binding` | `collect_segments` | Nested loops, builtin bytes argument, local named `slice`. |
| `tuple_unpacking_module` | `parse_line` | ValueError branch from unpacking. |

The publication is recorded in `work/logs/strict-basic-cohort23-publication.log`
(23 modules, generation `4dc86e3be9a674fc077826e0530d29ba3cbd604459734350aef1410cdd95f7cf`).
The shared runner factors the same admission and validation checks for
single-case and batched execution. This reviewed cohort uses one process per
mode, retaining individual failures and rejecting shared-state contamination.
Stock cases still use their original source/validators. All **23 stock validators
and 25 harness tests pass** (`work/logs/strict-basic-stock-and-batch-harness-after.log`).
The harness includes pytest failure/skip/xfail and `SystemExit` outcomes: each
must be reported as an individual failure, not escape the batch or become a
pass. Shared-state corruption stops later validators; `KeyboardInterrupt`
still cancels the run. The v8 run uses a fresh genuine publication; the earlier
v7 publication is not reused across native changes.

The unchanged `match_guard` case initially failed on its second call in both native and
entry-interpreter modes with `StrictMutationError`. Pattern capture identifiers
were omitted from current-scope binding collection, so the generated capture
assignment addressed a final module binding instead of a local. This is a
source-binding bug, not an expected strict incompatibility. The original source,
drivers, checker output, and per-case journals are preserved under
`work/strict-pattern-binding/before`. The narrow pattern-binding correction and
structured local/global/nonlocal/capture tests passed in the 403-test lowerer
gate. Both actual requested modes now pass on v9. The same run's basic-cohort
journals record all 23 validators passing in each mode; only the two selected
`match_guard` cases count toward that pytest invocation's reported total.

The old `field_index_specialized_{getattr,setattr,constructor}` Rust execution
positives used ordinary, unauthenticated source plus invented profiles. Their
v9 baseline was one pass and three admission failures. They are replaced by
`field_index_profiles_never_authorize_ordinary_runtime_compilation`, covering
read/write/construction in profile/apply/verify. Existing layout-priming and
structured field-kernel checks remain. Genuine strict checked-field and guarded
load integration cases provide positive runtime coverage; the new negative
matrix's two Rust tests and all ten genuine checked-field cases now pass on
the next fixed v9 extension.

The complete corrected checkpoint is recorded in
`work/logs/strict-runtime-v8-entry-policy-cohorts.log`: **109 PASS, 2 FAIL in
576.74 s**. It comprises initializer2, entry-runtime52, basic44/46, simple10,
and dependency-mutation-after-construction1. This is correctness evidence,
including checker/startup work, not a timing comparison.

## Class, target, and lifetime cohort

Six further source/validator pairs are explicitly enrolled, with unchanged
bodies and only the real strict future added to their selected variants:

| Case | Required synchronous witnesses | Retained behavior |
| --- | --- | --- |
| `assign_target_eval_order` | `run_named_subscript`, `run_nested_subscript`, `run_attr` | RHS precedes assignment target evaluation. |
| `augassign_target_eval_order` | Same three `run_*` functions | Augmented target lookup precedes RHS and setter. |
| `chained_assignment` | None | Class aliases reference the same object; initializer-only evidence. |
| `effect_only_selected_expr_semantics` | Five effect helpers | Only the required intermediate boolean/comparison results are truth-tested. |
| `listcomp_iter_once` | `run` | Iterator protocol and comprehension result. |
| `unpack_temp_drop` | `run` | Unpack temporaries do not retain the object after explicit deletion/collection. |

Custom attribute-hook classes must retain automatic dynamic behavior without
weakening surrounding module or function admission. The six ordinary validators
pass, and the real checker accepts all six selected strict variants on v10
without casts or diagnostic suppression. Publication generation is
`9d094e1406ccd00e4d97e99784cefa9d84bb963c2ff6fdb41faa342b72288d51`;
logs are `work/logs/strict-class-lifetime-v10-stock.log` and
`strict-class-lifetime-v10-publication.log`. No strict initializer has run in
that checkpoint. All twelve requested-mode outcomes now pass on the coherent
v10 extension; `chained_assignment` still observes one initializer path, not two
function entries. Cases outside the explicit cohorts remain unreviewed.

After the checker import-binding fix, the identical six sources were
re-published under generation
`7a2a9100b797489bb0cd770bce5347eee17358c992b259296fd059fc0b52c8ec`
without initialization. The original publication remains preserved; the new
record is `work/strict-class-lifetime-v10-import-bindings-publication.json`.
The actual run used a fresh same-pytest-environment publication, generation
`92ca1587146eadb13ab6db42563e195f8bb7431aa356ed1bfd315d398b5293de`,
recorded in `work/strict-class-lifetime-v10-pytest-publication.json`.

## Comprehension scope and protocol cohort

Twelve further original pairs are enrolled through the existing two-batch
runner. Only the strict future is added to their selected source variants;
their source bodies and validators remain unchanged.

| Case | Required synchronous witness | Retained behavior |
| --- | --- | --- |
| `comprehension_filters` | `run` | Generator filtering and short-circuit field reads. |
| `comprehension_iter_list` | `run` | Set comprehension over the constructed method-name list. |
| `comprehension_scope_shadowing` | None | Enum member lookup is not shadowed by the comprehension target. |
| `dictcomp_temp_collision` | `dict_comp_fib` | Key/value walrus assignment order and shared cells. |
| `dictcomp_temp_collision_class` | None | Dynamic Enum namespace update from a dictionary comprehension. |
| `class_comprehension` | None | Class-body comprehension result. |
| `class_scope_comprehension` | None | The original whitespace-distinct duplicate remains covered. |
| `listcomp_classcell` | `classcell_values` | Late-bound lambda cells and genuine `__class__`/`super()`. |
| `richcompare_rhs_fallback` | `run` | `NotImplemented` dispatches to the right operand and preserves its result. |
| `property_setter` | None | Property getter/setter round trip. |
| `class_private_attribute_set` | `run` | Private attribute mangling across writes and reads. |
| `list_setitem_specialization` | `set_item` | Builtin list writes and ordinary subclass overrides. |

All twelve original stock validators pass (`strict-comprehension-protocol-v10-stock.log`).
The real v10/0010 checker accepts all twelve sources without casts or
suppression, generation
`87d5e54c1f3cbbbc5b5525da516a35f8fa38c8b23c1d1b249e0b0abff635bc02`
(`strict-comprehension-protocol-v10-publication.log`). That publication did not
execute any initializer. The actual fixed-v10 run, re-published under the same
pytest entrypoint before patch0011, passes **22/24** outcomes. Both `listcomp_classcell` modes fail
before initialization because the signed lambda identity
`classcell_values.<locals>.C.<locals>.<lambda>` does not match the original
source catalog. This is a source-identity projection defect, not an accepted
strict incompatibility; its source and failure evidence remain unchanged while
the producer is corrected. The five
cases without global function witnesses provide strict admission, the observed
module-initializer entry, and behavior evidence only; running them in both
requested configurations is not proof of distinct function-entry paths. Enum
and builtin-subclass cases retain the language's actual dynamic class path;
no class eligibility or optimization claim is inferred from these validators.

The bounded identity correction is checker patch0011. It uses the same actual
semantic ancestry for lambdas and other source definitions, retaining original
byte ranges and the existing signed lexical convention. The independent native
projection now accounts for nested lambda `<locals>` and generator-expression
code scopes, with the first generator iterable still outside the suspended
body. Native matching consumes that exact projection together with the existing
source stamp, signature, and opcode ranges; no loose name fallback was added.
All 111 upstream project, 25 real CLI, and 408 lowerer tests pass. Source generation
is `d7d3a097102c34f282b933f74880e378cf25db6ad8e05666367ac22c032613cb`.

The expanded regression also exposed a separate lambda-default producer gap:
the original semantic snapshot and callable lowering both skipped nested
default expressions. A fresh genuine0010 run on the pre-fix extension reproduces
the unlowered-lambda panic before initialization (one failure, 75.19 s). Both
stages now visit defaults in the enclosing scope. Structured tests verify the
actual sibling scopes, captured cells, and enclosing-frame creation operations.
The public before fixture and input record are preserved under
`work/strict-lambda-source-identity/default-before`. On the combined v10 extension
`f21867aa4010d3744259f5651a44219dfc3530ad0f9f6e995d906453ffcbf82b`, the
unchanged comprehension/protocol cohort now passes **24/24**, including the
original `listcomp_classcell` pair. Both lambda-default regressions also pass
through their exact native/entry paths, checking creation order, separate
factory cells, and native names/layouts against ordinary controls.

On that f21867aa artifact, the other two expanded lambda cases were red: exhausted generator expressions
clear the `index` cell still owned by returned lambdas. All source/entry witnesses
and the other 21 of 30 callback observations pass in each mode. The nine failures
are three callbacks each from module, class, and factory-local class generator
expressions; their actual cells are empty, whereas the same ordinary controls
retain `2`. This is a separate runtime cell-lifetime bug, not a name-matching
failure. Evidence is in `strict-lambda-cells-v10-diagnostic.log` and its
`work/pytest/lambda-cells-v10-diagnostic` JSON outputs. The four original tests
retain all assertions.

Two validation-adapter mistakes were corrected before these semantic outcomes:
the new `run_case` strings needed dedenting and the supported
`def validate(module): ...` form. A raw validation tail runs in copied source
globals; it does not receive a variable named `module`. The initial indentation
and undefined-name failures remain recorded, and replay used the existing
unchanged publication rather than repeating analysis.

The subsequent fixed v10 extension
`1a62f777fe79a6af15047a27bee592d9c6a60f76aeec1b8d5ff10b1c36015699`
passes all four original lambda cases and both original-source class-body lambda
diagnostics. The new generator termination/lifetime pair also passes after its
ordinary validator releases a deliberately retained exception traceback; the
first two validator failures remain recorded. The checks cover exhaustion,
close, thrown-exception identity, escaped-cell lifetime, and actual source
deletion. Evidence is in `strict-v10-closure-cells-after.log` and
`strict-v10-generator-lifetime-replay.log`. The complete before/after records in
`work/strict-v10-pydantic-cells/inputs-{before,after}.json` were independently
compared and are exactly equal. These are the actual requested public function
entries; generator factory entry alone is not treated as proof of resume mode.

## Lexical class and closure cohort

The following 25 source/validator pairs have been read individually and enrolled
without changing their bodies or validators. Their strict variants only add the
real future import. All 25 stock controls and 33 harness tests pass. The first
real0011 checker publication rejects only `nested_classcell_capture` with
`unresolved-reference` for the nested function's implicit `__class__`. The
original source is valid on the selected ordinary interpreter. The full cohort
remains enrolled; this failure is not suppressed or recast as intentional
incompatibility. The failed project is retained at
`/tmp/soac-strict-class-closure-v10-kv23uvxv`, with the diagnostic in
`strict-class-closure-v10-publication.log`.

An explicitly partial **24-of-25** publication on the same frozen f21867aa
extension produces **46/48 passes**. Only `lambda_classcell` fails in each mode:
its actual source-authenticated callback has the correct `('__class__',)` layout
and requested entry, but its cell contains `None`, so it returns `None` instead
of the actual class. The ordinary control's cell contains that class. This is
distinct from the exhausted-generator empty-cell issue above. The partial
publication records the rejected 25th case, all selected source hashes, and
per-case results under `work/strict-class-closure-v10-partial`; the original
test list is unchanged. Together with the comprehension replay, the frozen run
is **70 passes / 2 failures** in 473.46 s, with byte-identical before/after native,
extension, checker, Python support, and launcher-input records. The later
1a62f777 checkpoint above fixes the two class-body lambda failures. The later
normal0014/v11 checkpoint runs all 25 original cases successfully, as recorded
below; the partial failures remain historical evidence.

Checker patch0012 resolves nested reads through the actual enclosing method,
lambda, or generator implicit-cell boundary, retaining nearer explicit bindings
and globals. Its private gate passes 114 project tests and all 10 scope test
files; normal source preparation and the CLI/test-binary builds pass for source
generation `757fa8fcd7dd7fdcbd15297a82b8625b01330eb9a7fee6378fb686f5824fbca9`.
The normal CLI gate then passes **26/26 tests** on the actual selected native v11
environment, including the new source-only publication regression and the
wrapper's post-build source/exporter fingerprint checks
(`strict-ty-0012-cli-v11-gate.log`, 220.42 s). This is checker validation, not a
fresh 25-case runtime result.

A separate genuine two-mode regression explicitly reads, replaces, deletes, and
rebinds the implicit cell and distinguishes two factory executions. Its 0011
baseline fails source-identity validation before any initializer executes
(`strict-nonlocal-classcell-before.log`). The private follow-up models a distinct
implicit-cell owner instead of inventing a namespace binding. It also exposed
an ordinary nonlocal declaration-lifetime bug: a returned closure was losing
its owner's annotation because lookup used the unreachable end-of-function
path. The unchanged invalid-assignment regression now uses reachable owner
declarations and remains blocking. Patch0013 passes 117 project tests, 63 core
tests, all 485 semantic test files, and the normal **27/27 CLI tests** on v11
(`strict-ty-0013-cli-v11-gate.log`, 224.99 s).

The fixed normal0014/v11 runtime then passes **52/52 outcomes in 265.54 s**:
these two nonlocal-cell cases and all 25 unchanged cohort cases in both
requested function modes. Exact public-entry witnesses remain mandatory for
the synchronous functions; the two functionless cases only establish the one
module-initializer path. Evidence is in `strict-checker-cells-v11-runtime.log`
and the durable `work/pytest/strict-v11-checker-cells` fixtures. The complete
`work/strict-v11-checker-cells/inputs-{before,after}.json` files are byte-identical
(SHA256 `ca85cd0a63c2e395e7d108102d29c825852d9a77ccf6eda4e9cc38a176e759f1`).

Native-valid forwarding of an outer class's initially empty cell through an
eager nested class body remains separate. The new genuine regression is
rejected by normal0014 before initialization
(`strict-eager-class-cell-before-v11.log`, 32.45 s); the structured before-test
confirms the missing outer cell owner. It distinguishes the inner body's
forwarded cell from the inner methods' own cell and checks factory isolation.
The callable-cell repair does not claim that construction-time behavior.
Patch0015 models that separate eager owner and passes 123 project tests, 63
core tests, and all 485 semantic test files. The empty-cell rule applies only
to an actual eager class body; existing lazy generator-cell behavior remains
covered. The semantic before/after logs are
`strict-eager-class-cell-checker-semantic-before.log` and
`strict-eager-class-cell-mdtests-verified.log`. After normal0015/0016 promotion,
the genuine runtime regression passes **2/2 in 47.23 s** on fixed v12 with exact
`checked_native`/`entry_interpreter` witnesses, actual native class owners,
separate outer/inner cells, and two-factory mutation isolation. Evidence is in
`strict-eager-class-cell-v12-runtime.log` and
`work/pytest/strict-eager-class-cell-v12`; complete before/after records in
`work/strict-v12-eager-cells/inputs-{before,after}.json` are byte-identical and
match the preceding v12 descriptor runtime. No lowerer/runtime workaround was
needed for this eager-cell case.

The test runner now accepts explicitly named plain-method witnesses such as
`Outer.Inner.method`. It reads only own namespaces of exact module/type objects;
it never performs inherited lookup, descriptor binding, custom metaclass access,
or evaluation. Actual native registration/owner and exact requested entry are
still checked before and after validation. This is test object selection, not
runtime authority. The two entries with no function witnesses remain one-path
initializer/admission/behavior coverage.

| Case | Required synchronous witnesses | Retained behavior |
| --- | --- | --- |
| `class_attr_default` | `Example.method` | Class-local sentinel is the method default. |
| `class_body_closure_self` | `make`, `CDLL.__init__` | A nested class body reads the enclosing instance. |
| `class_body_default_closure` | `make`, `run` | A default and method closure share the same outer sentinel. |
| `class_body_outer_local` | `build` | A class body uses the loop-local callable, not a global namesake. |
| `class_scope_capture` | `outer` | Class-body read of an enclosing cell. |
| `class_scope_inner_capture` | `outer` | The same capture shape is exercised during initialization. |
| `class_scope_inner_sees_outer_scope` | None | Nested class lookup skips the outer class's namesake. |
| `class_scope_inner_sees_outer_scope_closure` | `inner_sees_outer_scope_closure` | Nested class lookup keeps the enclosing function cell. |
| `class_method_outer_cell` | `run` | Method mutation/read of a captured list. |
| `class_method_import_shadowing` | `Example.__init__` | A method-local import is not a class attribute lookup. |
| `class_method_time_shadowing` | `Base.__init__`, `Base.time` | Global module lookup is not shadowed by a method name. |
| `class_lookup_lambda_recursion` | None | Class-body `__name__` resolves to the module; despite its name this case contains no lambda. |
| `lambda_classcell` | `classcell_lambda` | A class-body lambda captures the native class cell. |
| `lambda_qualname` | `global_function` | Two same-line lambdas preserve public names. |
| `lambda_qualname_minimal` | `global_function` | The existing equivalent name check runs during initialization. |
| `nested_class_binding` | `get_member` | Nested class identity survives attribute lookup. |
| `nested_class_closure` | `use_container`, `Container.build` | A returned nested instance shares its method's outer list. |
| `nested_class_method_shadowing` | `Outer.Inner.format_help` | Same-named outer/inner methods keep their own bindings. |
| `nested_class_nonlocal_method` | `Outer.run` | A nested method updates the enclosing method's cell. |
| `nested_class_qualname` | `Container.make` | Original `typing.Any` base retains dynamic class behavior and qualified repr; no new cast is added. |
| `nested_classcell_capture` | `exercise` | A nested function inherits its method's class cell. |
| `nested_super` | `Container.build`, `Base.probe` | A factory-created subclass uses zero-argument `super()`. |
| `nonlocal_binding` | `Example.trigger` | The original alternate nested nonlocal case stays covered. |
| `method_local_shadowing` | `Example.run` | A local with the method's name remains local. |
| `posonly_shadows_class_attr` | `make_value` | Positional-only parameters hide a class namesake; existing future annotations are preserved. |

`recursive_local_function` was reviewed but is not part of this batch: it changes
the process recursion limit, which the current contamination guard does not
observe. It needs isolated execution or an explicit recursion-limit guard;
ordinary source compatibility has not been rejected.

## Exception, closure, and cleanup cohort

The next 25 original source/validator pairs were reviewed individually. The
first real v9 publication rejects six sources with seven diagnostics, retained
in `work/logs/strict-control25-v9-publication.log` and
`work/strict-control-cohort/first-publication`. No source receives an `Any` cast,
ignore, or exception-text xfail. The 19 diagnostic-free sources publish
successfully as generation
`0dd025ab270cc9da3823803ef8dcdf116b86dbf13e3e21f6708188bf3b1a1e6d`
(`work/logs/strict-control19-v9-publication.log`). Their actual native/interpreter
run completed with 18/19 native and 16/19 entry validators passing. The same
pytest invocation selected both `match_guard` modes, for **36 PASS, 4 FAIL**
overall (`work/logs/strict-control19-match-v9-runtime.log`). Both
`with_return_context` failures expose missing binding plans for quoted nominal
annotations; required checks reject the unresolved target instead of silently
accepting it. The entry-only referrer failures exposed a positional staging
tuple, not a residual Python frame or exception cell.

The corrected entry/deopt call path uses raw vectorcall operands and native
keyword unpacking. Replaying the original two referrer validators against the
same signed deployment now passes in both actual requested modes: four
validations in two pytest batches, 22.99 s
(`work/logs/strict-entry-referrers-v9-after.log`). Native structured tests also
check absent staging-tuple referrers, exact body exception identity, keyword
string-subclass identity, invalid-key cleanup, and operand refcounts.

| Selected strict case | Required synchronous witnesses | Retained behavior |
| --- | --- | --- |
| `except_as_clears_exception` | `capture`, `count_exception_referrer_frames` | No residual frame reference to the returned exception. |
| `except_star_bind_group` | `handle` | Matching exception-group binding. |
| `except_star_group` | `handle` | Same group-binding shape with a nonempty subgroup assertion; not independent semantic coverage. |
| `try_orelse_on_exception` | `exercise` | The else clause does not run after a caught exception. |
| `closure_cell_nonlocal` | `outer` | Nonlocal assignment changes the captured cell. |
| `closure_attr` | `outer` | Returned function exposes a native closure. |
| `nonlocal_del_binding` | `outer`, `main` | Suspended nested function initializes a cell subsequently deleted by its owner. |
| `delete_nonlocal_compiles` | `outer` | Deletion through a nested nonlocal binding during initialization. |
| `with_exit_suppresses_exception` | `run` | A truthy exit method suppresses an exception. |
| `with_return_context` | `use_context`, `run` | Exit executes before a return carrying the entered object. |
| `with_extended_targets` | `unpack_starred_list` | Starred assignment of an enter result. |
| `with_special_lookup` | `run` | Context-manager special lookup bypasses instance attribute hooks. |
| `with_context_exception_leak` | `leak_check` | A suppressed exception does not retain the victim. |
| `exception_refcycle_after_except` | `run` | No extra exception referrers. |
| `exception_refcycle_args_tuple` | `run` | Equivalent referrer assertion under the original alternate local name. |
| `support_current_exception_recursion_minimal` | `exercise` | RecursionError handling during initialization. |
| `assert_raises_refcount` | `_boom`, `run` | An ordinary unittest callback preserves callable refcount. |
| `for_loop_temp_drop` | `run` | Loop/iterator temporaries do not keep a yielded object alive. |
| `coroutine_return_value` | `main`, `manual` | Event-loop and manual coroutine return propagation. |

All witnesses are synchronous. Nested generator/coroutine behavior is tested,
but no particular resumed-body mode is claimed. Some original validators only
observe results computed during module initialization; those retain initializer
behavior evidence and classify the surviving function entry, without claiming
an additional post-seal invocation that did not occur.

The other six remain explicit tests, split by their actual contract:

- `exception_cleanup_global` and `except_star_global_binding` expose a checker
  mismatch with the approved initially absent, syntactically mutable global
  contract. They remain strict cases in a separate fixture. The bounded 0009
  candidate reconciles only the exact declared-global diagnostic, preserves a
  visible warning, and exports `Unknown` with no definition/boundness proof.
  Its first four structured cases and all 102 `ty_project` tests passed
  privately. The expanded source-provenance checkpoint additionally exports
  quoted nominal leaves through ty's existing string-annotation submodel and
  distinguishes real dataclass field annotations from synthetic signature
  display types; all 105 project tests pass. Production patch 0009 has now
  passed normal source reconstruction, all 23 offline CLI tests, and a fresh
  binary build. Both original declared-global sources pass in each actual
  synchronous entry mode without weakening unrelated unresolved-name errors.
- `closure_cells`, `exception_cleanup_local`, and `exception_cleanup_deleted`
  intentionally read an unbound/deleted name. `with_enter_result_lifetime`
  calls a global the checker still infers as `None`. Their strict variants
  must continue to fail the real checker without publishing a deployment.
  Separate interoperability fixtures keep each original module ordinary and
  invoke its unchanged validator through a genuinely selected strict caller.
  Those fixtures require actual strict caller/initializer observations and
  explicitly verify that the original module/functions have no strict owner.
  They test ordinary interoperability, not transformation of the rejected source.

The complete source-provenance rerun passes **54/54 in 278.08 s**: 21 selected
strict sources in each actual synchronous entry mode, four ordinary-interop
sources in each mode, and four genuine strict-checker rejections. The original
quoted-context-manager and exception-referrer failures all pass. The separate
quoted-nominal regression passes in both modes, covering parameter/return/union
targets, pre-Ready direct self, distinct factory executions, and no provider
evaluation. Its provider-capture assertion compares against the actual ordinary
native code: a quoted method can still carry `__classdict__`, so empty captures
must not be assumed from quotation alone.

Evidence: `work/logs/strict-control25-v9-source-provenance-runtime.log`,
`strict-quoted-nominals-v9-after.log`, and
`strict-source-provenance-cli-tests.log`. Complete before/after input records
under `work/strict-source-provenance-v9` are byte-identical, including the actual
native executable/library, staged extension, Python support, checker binary,
and pinned checker sources. This is correctness evidence, not a timing claim.

A separate named-keyword audit then reproduced an observable temporary
dictionary in **both** strict function entries. Its six-case v9 baseline is
**2 PASS, 4 FAIL**: compiled cleanup matches CPython, while entry cleanup drops
the first keyword value before the second on success and native argument
errors. Ordinary CPython drops them in reverse operand order. The narrow fix
keeps plain named operands in a raw vector; unpacking calls retain their mapping
path. The expanded regression also checks mixed positional/named arguments,
callable replacement through an attribute or closure cell, and later argument
errors. All sixteen expanded post-fix outcomes now pass on the fixed v10
extension with exact synchronous entry witnesses. The exact original source,
signed publication, drivers, and failures are preserved under
`work/strict-named-keyword/v9-before`; the log is
`work/logs/strict-named-keyword-v9-before.log`.
The expanded sixteen-case fixture is accepted by the real v10/0010 checker;
its retained publication is
`work/strict-named-keyword/v10-import-bindings-publication.json`. This is an
offline preflight only. The actual run used the same-pytest-environment
publication in `work/strict-named-keyword/v10-pytest-publication.json`.

The combined fixed-v10 checkpoint is **50 PASS, 2 FAIL in 217.74 s**: named
keywords16/16, class/target/lifetime12/12, and comprehension/protocol22/24.
Its only failures are the lambda source-identity mismatch above. Actual native
executable/library, extension, checker, Python support, and launcher environment
hashes are identical before and after the run. The extension SHA-256 is
`b2cff38036825f788e18669c786a8a0d73ffad7065b3a493165b47f6e07d830f`.
Evidence is archived under `work/strict-v10-keyword-cohorts/fixed-runtime`, with
the full log at `work/logs/strict-v10-keyword-cohorts-runtime-same-env.log`.
This is correctness evidence, not a performance comparison.

The genuine foreign-call fixture exposed a separate required-boundary gap:
plain `from targets import Box` produced correct nominal signatures and
attribute proposals but no nominal-binding leaf. The checker used an IDE mode
that preserved only explicit `as` aliases. Patch0010 preserves every explicit
local import definition, leaving ordinary IDE resolution unchanged. Its
upstream109-case and real CLI24-case gates pass. Re-analysis of the exact
unchanged two-file fixture adds the two expected leaves (one each for `field`
and `call`), both referring to the existing local import range, while
attribute facts, signatures, and global catalogs remain identical. No runtime
initializer was executed. Public before evidence and the new publication are
under `work/strict-imported-nominal`; runtime nominal/capability checks still
require the actual bound class and retain their existing fail-closed behavior.

The import-hook mutation case `raise_from_import_shadow` requires a separate
isolated process review. Missing context-manager protocols and a non-exception
cause constructor also belong in explicit ordinary-interop/error coverage.
`exception_cleanup_name` has an obsolete strict validator expecting `locals()`
to be unsupported; it is not enrolled under that expectation.

## Specific defects and compatibility decisions to preserve

| Cases / path | Finding | Next action |
| --- | --- | --- |
| `yield_from_module` | Return-value assertions were unreachable inside `pytest.raises`, after the raising send. | Moved after the context; the stock validator passes (`integration-yield-from-validator-stock.log`). Both strict modes remain pending enrollment. |
| `concat_surrogates`, `fstring_surrogates`, `surrogate_unicode_escape_repr` | Existing strict-mode expectations accept U+FFFD instead of the original surrogate data. | Record as compatibility defects, not approved strict differences; preserve data or fail explicitly at a documented boundary. |
| `mutated_function_defaults`, `mutated_closure_function_defaults`, legacy `test_regression_function_mutation.py` | The old tests expect mutable callable metadata or revocation. | Keep ordinary mutation controls and add explicit strict mutation rejection; do not revive revocable capabilities. |
| `transform_temp_module`, `test_indexed_module_type.py` | Expectations include injected `runtime` or an obsolete custom module type. | Replace with the actual builtin-module/native-owner contract, not a name-only assertion. |
| `bad_syntax`, `class_annotations_mutation` | Existing matrix handles source errors around in-process import. | Separate genuine checker rejection from runtime exceptions; do not let a helper admission error satisfy either. |
| `scope_locals`, `locals_cell_contents`, named-expression/exception-name cases | Old per-case exclusions or expectations describe frame-sensitive builtin limitations. | Re-evaluate against the actual context-aware runtime; do not assume old exclusions are still necessary. |
| `test_opt_cases.py` | Its independent profile/verify subprocess runner lacks offline/native startup admission. | Migrate profile/apply execution and require real JIT evidence; passing stock behavior is insufficient. |

Framework/dataclass/typing, annotation, generator/coroutine, class/cell,
import/loader/environment, and intentional-mutation groups still need separate
review. Standard adapters cannot be replaced by blanket test opt-in, broad
xfails, or global dynamic exclusions.

The decorated-class provider regression exposed an incorrect shared source-line
rule: native class annotation providers start at the class code's first
decorator, while function providers start at the actual `def`/`async` header.
The previous structured test incorrectly expected the class header. The
corrected test fails first with `Some(5)` versus `Some(3)`, and a minimal genuine
fixture fails during import with no native `Item.__annotate__` match
(`strict-class-provider-lines-runtime-before.log`, 42.86 s). The fix selects the
offset by the original definition kind, without weakening native parent, code,
capture, or execution checks, and invalidates old BlockPy metadata with cache
generation 23. Raw v13 controls also verify decorated generic wrapper/class/
provider lines and captures; these controls compile but do not execute the
source.

That expanded replay then found a distinct generic-wrapper range mismatch
(`strict-v13-provider-binding-corrected-runtime.log`, 1 failure in 50.10 s).
Its signed declaration includes decorators, while nonempty native wrapper
positions cover the header through the body end. `TypeParameterScope` now
retains that exact parser-derived native header span separately from the full
signed declaration; the mapper still requires exact native positions rather
than a containing-range lookup. Cache generation 24 invalidates the old shape.
The focused lowerer regression passes for decorated classes, functions, and
async functions. The exact native-position kernel passes in the parent's
**689-test JIT gate**, and the genuine expanded replay passes **both modes in
48.90 s** (`strict-v13-decorated-generics-after.log`). It covers decorated
ordinary and generic classes, late factories, and generic synchronous/async
functions, checking original provider lines/freevars and VALUE/STRING replay.
Synchronous functions have exact requested entry witnesses; the async function
is witnessed as `generator_factory`, not as proof of dual coroutine-resumption
paths. The before/after input snapshots under
`work/strict-v13-decorated-generics-after/` are byte-identical: extension
`7256a81b1b3ea795bcafb6958865ff69505850b830b68ab0f45675a5d3956590`,
normal0017 checker, native v13, and unchanged Python support.

## Generated dataclass catalog

The actual Base/Record artifact omitted generated `__repr__` and `__eq__` despite
resolved `repr=True`/`eq=True` options. The exporter already requested those
members; the checker lacked their synthesized signatures. A focused regression
first fails on missing `Base.__repr__` (`strict-dataclass-repr-eq-before.log`).
Patch0014 passes three structured catalog cases, all 120 project tests, all 485
semantic test files, and **27/27 normal CLI tests** on v11 (227.97 s). The normal
executable was separately rebuilt through `just ty --debug-build -- --help`
(24.22 s); `work/strict-ty-0014-v11-ready.json` records its verified fingerprint,
source generation, and binary SHA. Generation follows the
actual options and live own bindings, preserving explicit definitions, lambdas,
non-callable overrides, inherited methods, and annotation-only names.

The generated parameters and return types remain **inferred**, not explicit
source annotations. Native v11 controls verify the keyword parameter shapes,
foreign-object `NotImplemented`, and a non-boolean field-comparison result
(`strict-dataclass-repr-eq-native-controls-v11.log`). No required `str`/`bool`
return check or receiver-layout authority follows from these synthetic types.
Actual trusted-transform adoption is a separate runtime boundary and is not
claimed by this checker gate.

The generator's field query excluded `dataclasses.KW_ONLY`, but the exporter's
own-annotation append pass added the marker back as an instance field. Patch0016
removes that false storage proposal using the actual dataclass-like generator
and semantic `KnownClass::KwOnly` identity. It does not guess from names or from
absence in a generated constructor. The `init=False`, interleaved `ClassVar`,
renamed marker, future-annotation, and inherited-field regressions pass; ordinary
class annotations and user types/fields named `KW_ONLY` remain intact. The
test-first failure is in `strict-kw-only-checker-before.log`; the candidate
passes 125 project tests, all 485 semantic test files, and ordinary-native v12
controls in `strict-kw-only-native-controls-v12.log`.
The normal combined0015/0016 checker then passes **28/28 CLI tests in 237.29 s**,
including both new publication behaviors. Its actual normal executable build
and post-build fingerprints also pass; `work/strict-ty-0016-v12-ready.json`
records source generation `6ee40b8ad51da55b3e0dd47dd0a4982ff40202dff60841ca13fd63215741e1fc`
and binary SHA `dcf51dc19a9d3a652b9cd28b5accb3dbf476b0b264dfa600ee13a89cc20798da`.

The schema3 baseline had no signed lexical leaf witness for a field's nominal
annotation: nominal-binding entries belonged only to source-function parameters
and returns. All six genuine v12 nominal-field cases failed class admission
(`strict-v12-nominal-field-before.log`, 69.11 s), while the real checker accepted
the unchanged source. Both baseline input snapshots are identical under
`work/strict-v12-nominal-field-before/`.

Schema4 adds an exact field annotated-assignment reference and an explicit
function-annotation or field owner for every nominal leaf. It rejects omitted
provenance keys and old schema/signature authority. The focused contract gate
passes **54 tests**; the actual checker database passes **131 project tests**
and **485 semantic test files**. Patch0017 covers global, imported, class-local,
factory-local, direct-self, inherited, and method-declared field provenance,
including source invalidation. Inference alone does not manufacture an
annotation, and multiple method declarations leave the source reference
explicitly unresolved.

The transactional-plan regression first demonstrated that a union could lose
one ambiguous alias while normalization hid the omission. Both function and
field plans now remain absent unless all required nominal leaves resolve;
builtin/numeric/None/type-object arms and `Annotated` metadata do not count as
missing nominal leaves. These are source/proposal gates, not proof of runtime
field enforcement. The normal0017 executable build passes separately (57.52 s),
then all **29/29 normal CLI tests pass in 254.30 s** on selected native v13.
The new real CLI case verifies field owners, inherited dependencies, source
invalidation, and no initializer execution. `work/strict-ty-0017-v13-ready.json`
records source generation
`9da87a7fe82d8ad233985d9ecaca28c9e2451ca9aaaa0a8a4ba47603d75c2609`
and normal binary SHA
`3cbe289b0f9eaf5df53669364c5fb11b64a0779b8a333d813880cbf69ebc041b`.
The actual v13 field checkpoint remains separate and is not implied by these
offline publication gates.

A method-only `self.payload: Target` annotation can have no native provider or
closure cell even though its source leaf resolves. Schema4 signs that source
fact without fabricating a runtime capture. The explicit construction-operand
checkpoint below supplies the first producer; its actual runtime gate remains
separate from these source facts.
Generated constructor parameters likewise need their actual field projection.
A class reference alone must not merge distinct factory executions or
reattribute an inherited field's contract to the child's namespace.

One native-valid case remains explicitly blocked: deleting an explicit method
inside the class body triggers a preexisting `invalid-method-override` diagnostic.
The same failure was reproduced with the earlier synthesizer
(`strict-dataclass-deleted-method-baseline.log`). Raw-facts coverage verifies
subsequent generation without suppressing that error or claiming admission.

## Semantic builtin base references

The actual v13 explicit-`object` class cases were rejected because schema4 could
only name source-class bases. Schema5 adds the public `BaseReference` enum to
both direct bases and logical MRO entries. Patch0018 uses actual `KnownClass`
identity for builtin aliases, preserves source references for user classes named
`object`, and leaves unsupported builtin subclasses dynamic. Logical typeshed
ABC entries are not physical CPython layout. Runtime admission separately
requires the signed builtin variant and the exact actual base object.

The isolated gates pass **57 contract tests**, **134 checker-project tests**,
and **485 semantic test files**. Exact patch replay passes with no fuzz. The
normal executable builds in 1 min 06 s, and all **30 CLI tests pass in 338.87 s**,
including real builtin-alias/source-replacement invalidation without initializer
execution. Post-build source and interpreter verification passes independently.
`work/strict-ty-0018-v13-ready.json` records source generation
`ffce7a9535667cef248837fd9c77eb2b9ad541244ac56ffe5e318b079f5d27a5`
and normal binary SHA
`d352c76b37fc13a85294174850d948e4ecc6a6d2062320cd2dd245b20c3ee7e4`.
The schema5 runtime source type-checks; actual explicit-object execution still
waits for the coordinated next native ABI build. These offline results do not
claim that runtime boundary is green.

## Dataclass fields named `self`

The unchanged genuine `Record(self: InitVar[Target], payload: Target)` fixture
was rejected by normal0018 with `duplicate signature parameter`, before runtime
execution (`strict-dataclass-edge-bindings-schema5-before.log`). The native
constructor uses `__dataclass_self__` when any dataclass field is named `self`;
the checker always used `self`. Correcting only `__init__` exposed the same
collision in the field-expanded `__replace__` proposal.

Patch0019 records stdlib generator provenance at its existing semantic producer,
preserving it through parameter updates and normalization. It uses the complete
own/inherited dataclass field table for constructor receiver selection, including
`ClassVar`, `InitVar`, and `init=False`, while excluding unannotated attributes
and semantic `KW_ONLY` markers. Custom transforms sharing native field specifiers
keep their existing policy. The stdlib replacement proposal has an unnamed
positional-only receiver and collision-free internal DTO labels; actual field
names and explicit declaration origins are unchanged. These labels do not claim
native parameter identity. The real `self`/`__dataclass_self__` constructor
conflict still follows the native rejection; the validator remains unchanged.

Private gates pass **138 unfiltered project tests**, including four new
structured cases, and **485 semantic test files**. Nine ordinary selected-v13
controls confirm native receiver and replacement signatures, including the real
duplicate-name failure. Evidence is in
`work/strict-ty-0019-isolated-ready.json` and
`work/strict-dataclass-self-receiver-native-control.json`. The tested patch SHA is
`45875e40005f30ff115cc248bc5661400b13de447e45430c5c304d3f8c83e1ef`.
Normal0019 then passes **all 31 CLI tests in 353.26 s**, with the real normal
executable built through the fingerprint-checked wrapper and reverified after
the gate. The unchanged named-self source publishes successfully, while the
real conflicting-name source still rejects without replacing its previously
published deployment. `work/strict-ty-0019-v14-ready.json` records the exact
normal executable, generation, selected v14 interpreter, and input fingerprints.

Actual named-self adapter execution subsequently passes in both checked-native
and entry-interpreter modes on the fixed v14 extension (`0fbb226...`). The
retained-publication replay is **2/2 in 10.48 s**, recorded in
`work/logs/strict-v14-named-self-retained-fixed.log`; the surrounding nominal
cohort's before/after snapshots are byte-identical. Its first run had 14 passes
and two validator-fixture failures because an ordinary duck-call test object
lacked `__post_init__`; adding that ordinary protocol method to the validator,
not changing the analyzed source or checker policy, made the two replays pass.

## Private construction cell checkpoint

The initial producer selects only cells owned by the immediate source function.
Its public core DTOs are `ClassConstructionScope`,
`ClassConstructionCaptureSlot`, and `DiscardClassConstructionCaptures`, all
explicitly re-exported from the core IR module. `MakeFunction` carries the exact
namespace function and cell operands; the catalogue validates their resolved
storage against the complete signed field leaves. Internal owner-cell storage
does not add public freevars, and generator preserved slots remain physical
storage rather than an introspection-only projection. Cache version 25 rejects
older serialized shapes.

The runtime pins cells before helper CREATE, installs only after ordinary
creation/registration succeeds, and weakly pairs the one-use carrier with the
actual namespace function and native owner. `ConstructClass` consumes it before
callbacks; field binding reads contents after namespace execution. An explicit
finally discards unused carriers on failures. A separate private denial-only
code clone prevents a CREATE observer from executing a helper's uninitialized
closure; it grants no source identity. Ordinary and original source code are
unchanged. The preserving native CREATE shim is shared with the generated
dataclass boundary tests, not a Python callback entered with a pending C error.

The first source checkpoint passes **413 lowering tests**, including one new
structured test covering synchronous, generator, and coroutine storage;
**705 JIT tests**; and **222 optimizer tests**. Focused native kernels also pass
the stale-class-owner address-reuse regression, both may-bound cleanup cases,
and zero/captured helper denial before closure installation. Logs are
`work/logs/strict-private-class-capture-{lowering-all,jit-all,opt-all}.log`.
These are Rust/native-kernel results, not evidence that the staged extension
contains the new producer.

The actual checker accepts the new four-outcome CREATE/failed-argument/replayed
namespace family in 39.01 s with all classes Candidate and an explicit
method-owned field leaf. Its initial durable fixture is
`work/pytest/strict-v14-private-class-capture-publication`; setup-only execution
runs no strict initializer or test body. The first actual run had two passes
and two failures: the fixture had left `checked_fields` disabled, so expecting
its otherwise admitted class to reject a field write was incorrect. Owner
presence alone does not select a write predicate. The original failures and
five-seam write observations remain in
`work/logs/strict-v14-private-class-capture-runtime.log` and
`work/logs/strict-v14-private-capture-admission-paths.log`; they are not an
inheritance enforcement bug.

With the same source and explicit `supported_annotations` field policy, all
**four actual outcomes pass in 60.13 s**. The fixture now asserts the actual
published policy before running its validators. Early zero/captured helper
CREATE calls reject, ordinary public closure metadata is preserved, failed
arguments release private cells despite an escaped helper, enabled field
checks reject wrong values, and same-source/different-namespace replay rejects
and releases its unused cells. Evidence is
`work/logs/strict-v14-private-class-capture-enabled.log`; before/after snapshots
in `work/strict-v14-private-capture-enabled/` are byte-identical for extension
`fb968a9f30b66d429f540b09e3a1d197d144ff836541b534d5cc0ddaaef5284f`,
native v14, checker0019, and Python support `ef15a62b...4561`.
Class namespace/construction helpers are always
entry-interpreted by existing policy, while the source factory has an exact
checked-native or entry-interpreter witness for each requested mode.

The initial immediate-owner producer is not full lexical compatibility. The
later v15 forwarding implementation now has explicit native-closure,
private-function, and namespace-handle transports, including suspended private
slots. Existing public captures use the active native closure after binding;
private-only captures keep original cell identities without adding public
metadata. Namespace handles transfer their private cells once and clear all
edges on consumption or failed construction. This implementation still awaits
its genuine v15 runtime checkpoint; the earlier v14 passes are not evidence for
the new paths.

Policy selection is shared by the exporter DTO consumers, lowerer, and runtime.
It retains enabled field checks and genuine inheritable stdlib generated-field
declarations (including `init=False`), but excludes disabled ordinary field
checks, statically dynamic classes, and dataclass method-only annotations that
are not actual class declarations. All **58 contract tests** pass. Normal
checker0019 v15 binary
`3cfabf191d23b7550c290afb0be812843bed90d973c1d335b27898e8bf9edaf1`
contains exporter fingerprint
`8240baeddee86a71071aa09696e26a3cfe3249bc7351a70289b3e4124e00b07f`.
Its normal wrapper and **31 full CLI tests pass in 84.81 s**, with the normal
binary/fingerprint verified unchanged. Evidence is
`work/strict-ty-0019-v15-declared-fields-ready.json` and
`work/logs/strict-ty-0019-cli-v15-declared-fields.log`; the preceding `70ef...`
policy-only readiness and its signed publications remain separate evidence.

The new structural test first caught a private `CellRefForName` being treated as
an unresolved public capture. Scope collection now distinguishes an explicit
lexical-owner operand from an ordinary capture request instead of removing
public slots after the fact. The preserved failure is
`work/logs/strict-v15-private-lexical-public-layout-before.log`. The corrected
focused family passes **6 tests**, including native/private/namespace plus
generator/coroutine paths and policy/dynamic exclusions; the cache test passes
**1 test**, preserving signed owners/leaves while remapping runtime namespace
IDs through serialization. Logs are
`work/logs/strict-v15-private-lexical-public-layout-after.log` and
`work/logs/strict-v15-private-lexical-cache-remap.log`. Combined JIT test-target
type-checking passes, but no v15 runtime extension was present at this checkpoint.
The added C closure-mutation and returned/suspended private-cell lifetime cases
have ordinary CPython controls; genuine strict runtime results are pending.

## Direct-call, mutation, and original-code fixture migration

The direct-default and function-mutation files now use genuine selected strict
callers instead of the retired in-process ordinary-SOAC helper. Original ordinary
mutation sources remain ordinary dependencies with explicit negative owner
witnesses, and their original validators run against an independent stock
caller and the selected strict caller. Separate strict-source tests reject sealed
code/default/keyword-dictionary mutation. Initialization callbacks actually
execute changed code and defaults, covering restoration, retained defaults, and
retained code followed by an expected sealing failure. Known wrong-arity source
is an explicit checker negative; dynamically supplied wrong-arity arguments
still exercise the runtime binder. Omitted-default profile/apply coverage retains
the structured direct-edge counter and exact checked-native entry witness.

The two files collect **31 outcomes**. Their selected projects publish
successfully under normal0020; the ordinary controls pass all nine mutation and
three initialization scenarios on the recovered v15 interpreter. The private
capture fixture's 14 outcomes also publish successfully. These are **not runtime
passes**: `work/strict-v15-capture-mutation-preflight-0020.json` records three
setup-only publications with no initializer or test-body execution.

`test_regression_original_code_object.py` now has five genuine strict outcomes,
preserving the original nested function, generator-expression, coroutine,
async-generator, and class-helper assertions. Each of its three source bodies
is byte-identical after replacing only the initial blank line with the strict
future import. Native compilation confirms the original first lines 2, 5, and
12. New exact native-owner witnesses distinguish generator factories from
ordinary checked-native/interpreted entries; profile-mode class-helper telemetry
still rejects foreground helper JIT and requires an ordinary-method codegen
event. Its three-module publication succeeds without initialization; source
hashes and line metadata are in
`work/strict-v15-original-code-preflight-0020.json`. The first actual run and
its independently classified failures are recorded below.

## Retained generator structural coverage

Ten typed-pipeline tests previously required generator factories and suspended
activation state to disappear from next/list/tuple and diagonal-set consumers.
That expectation conflicts with retaining the actual generator resume boundary
while its handled-exception and cleanup obligations remain observable. The
tests now compare exact original and final generator target sets, local factory
materializations, matching resume plans, public factory scope, and owned
suspended-frame layout. Existing ordinary-consumer optimization assertions and
foreign preserved-slot/cell negatives remain in place; an outer generator may
still own its own preserved storage.

All ten original Python snippets and the production portion of
`typed_pipeline.rs` are unchanged. The focused `generator_activation` family
passes **10 tests in 0.17 s**, with no ignored tests. Evidence is
`work/logs/strict-v15-generator-retention-focused.log` and
`work/strict-v15-generator-retention-test-migration.json`. These are structured
compiler kernels, not strict source-admission, runtime compatibility, or
performance claims. The handled-region entry verifier regression has a
separate production fix and test-first evidence.

## Fixed v15 capture, mutation, and original-code checkpoint

The first combined actual run on normal0020, persistent native v15, and extension
`8f0ba7d506cbffee21f4395b820df3eee353674a93db9c6cee8c42b29cb25f48`
completed **28 passes and 22 failures in 384.08 s**. Runtime provenance and
test/fixture/recipe snapshots were byte-identical before and after. The durable
receipt is `work/strict-v15-forwarding-mutation-code-0020/result.json`; the
complete log and analyzed projects use the same label.

The failures are not 22 independent regressions:

| Outcomes | Classification | Required follow-up |
| --- | --- | --- |
| 18 | The common handled-state prologue generated a cleanup call using a non-dominating activation value. All 14 capture cases, two sealed-mutation cases, and two generator-code cases stopped before their substantive assertions. | Re-run every original validator after the separately tested dominance repair; do not claim capture or coroutine success from the earlier structured gates. |
| 2 | Strict runtime arity errors used non-native wording instead of CPython's aggregate missing-argument grammar. | Shared binder repair plus exact native parity and actual public-boundary replay. |
| 1 | The valid omitted-default free call returned 42 through checked-native functions, but no CLIF direct edge was selected. | Extend checked unbound-function/default planning without reopening unchecked strict targets; keep the positive direct-edge assertion. |
| 1 | The helper-JIT validator selected only per-function codegen events, while eager compilation emitted a successful batch event. | Accept the real batch event and require the exact eight ordinary-method native bodies plus zero helper bodies in the structured code inventory. |

The 28 actual passes retain ordinary-target mutation interoperability, negative
ordinary-owner witnesses, selected native/interpreted callers, pre-seal
replacement execution/restoration, retained-default behavior, explicit sealing
failure for retained replacement code, ordinary original-code metadata, the
entry-interpreter omitted default, and the genuine static wrong-arity rejection.

The binder repair shares one native-name/error formatter and the existing
current-default binder, without a native ABI, checker, or alternate strict-entry
change. Its native-linked unit compares **17 invalid call shapes** with actual
CPython Unicode error objects, including aggregate positional/keyword-only
arguments, default ranges, explicit keyword-only counts, and an actual qualified
name containing a lone surrogate. It also checks output-reference cleanup.
The focused parity test and two existing exact-positional binder tests pass;
`cargo check -p soac_jit --tests` passes. Evidence is
`work/strict-v15-argument-binding-gate-retry.json`. Both original wrong-arity
cases and both new 17-shape native-parity cases also pass in the actual
after-checkpoint below.

The isolated default-call diagnostic
`work/strict-v15-default-edge-diagnostic/result.json` preserves the original
signed source and confirms checked-native execution, a real profile/apply pass,
two compiled direct bodies, and **zero selected direct edges**. It is a
diagnostic pass, not a passing optimization assertion. The source-selected
checked planner currently handles method calls only; the normal strict-module
unchecked-target exclusion remains correct and unchanged.

The next immutable extension is
`023028063f4c6750fc29cbbfbf690b4de134864294dc9c0c900afa27f39ac0ac`,
built after **420 lowerer and 716 JIT tests passed**. The selected 23-outcome
actual replay completed **17 passes and six failures in 224.83 s**; runtime and
test/fixture/recipe snapshots were byte-identical. Its selection is
`work/strict-v15-compatibility-after-selection.json`, and its receipt is
`work/strict-v15-compatibility-capture-binder-after/result.json`.

The 17 passes include early CREATE rejection, exact namespace-birth matching,
escaped namespace-handle cleanup, allowed pre-seal public-closure replacement,
ordinary private-cell forwarding/lifetime, sealed mutation rejection, all four
binding cases, and the helper inventory. The remaining six failures reached
their substantive assertions: two generator and two coroutine cases retained
private targets through an already-closed wrapper; two generator-expression
cases did not preserve the original `gi_code` identity. They remain distinct
from the repaired common prologue failure and the unimplemented free-call
direct-edge selection.

A retained-project diagnostic in both entry modes proved that the closed
wrapper's `_resume_function` was the only remaining referrer to the source
function, while native preserved-state edges were already cleared. Removing
only that wrapper edge released the target without deleting the wrapper or its
code metadata. This **two-pass diagnostic is not an after-test substitute**;
its evidence is `work/strict-v15-closed-private-capture-diagnostic/result.json`.

The Python runtime now releases that terminal wrapper edge outside its
transport-exception handler, preserving the original exception context and
shared source-cell ownership. The four unchanged generator/coroutine validators
then passed **4/4 in 60.91 s**, with fresh genuine publication and byte-identical
runtime/test snapshots. The extension, native interpreter, and checker were
unchanged; the new Python-support aggregate is
`42deb0379265651b79fe35bb94ac8239d4c4bbf3953fca126c27801ac7a63429`.
Evidence is `work/strict-v15-closed-private-capture-after/result.json`.
The subsequent comment-only lint explanation preserves the semantic AST and
changes support to
`289cd4142bc113293b02d35c566b20d209e6d93509274c9d7884fbd56cedb914`.
On coordinated extension
`c8620129aae0647c819f4b8da9edee4faf1bb0cb6fa1592e422588df825c88ed`,
all **12 additional cases pass in 168.05 s**: generator, coroutine, and
async-generator shared-frame ownership, plus close/completion finalizer
contexts compared with ordinary native controls in both entry modes.
Runtime and test snapshots are byte-identical in
`work/strict-v15-terminal-owner-expanded-after/result.json`.
The separate generator-expression identity and checked free-call optimization
gaps are not claimed fixed by these checkpoints.

## Full-gate workflow evidence

Offline checker test execution and normal-executable readiness are separate
boundaries. Use `just ty --debug-build -- --help` to build and run the normal
binary through the fingerprint-checked wrapper. Without the separator,
`--help` only prints wrapper help and performs no build. The production fixture
helper already uses the separated form. The 0013/0014 journal caught and
corrected this distinction before releasing consumers: real CLI tests had run,
but wrapper-help success was not evidence of a rebuilt normal binary.

The scoped formatting recipes now route `soac_ty` to its separate manifest,
just as they already did for the standalone raw runtime. Both
`just fmt-rust soac_ty` and the combined contracts/checker format check passed;
the offline package no longer requires a manual Cargo-format workaround.

The 0020 lock refresh exposed a separate workflow defect: `cargo generate-lockfile`
upgraded `is-macro` 0.3.7 to 0.3.8, `log` 0.4.33 to 0.4.34, and `uuid` 1.24.1
to 1.25.0 while adding the shared source validator. The package-identity
preflight stopped before building the checker. The original lock was restored
from its hash-verified archive, and `run_ty --update-lockfile` now uses
`cargo update --workspace`. The actual retry added only `soac_source` and kept
every existing package identity. Failed lock/diff/log evidence is preserved as
`work/logs/strict-ty-0020-generate-lockfile-before.*`; the corrected delta is
`work/logs/strict-ty-0020-lockfile.diff`. Wrapper regression cases cover both
successful and failed Cargo exit statuses; the full Python toolchain family
passes **27 tests in 1.39 s**. The normal0020 executable
`05644b448f67b0d78dd20d1efcd994dd3d7d092c6dfcc743d3d7d631e1fe1b1d`
embeds exporter fingerprint
`d7578fc95bc39d7660857a31c95440d2d52eaa4dcaa9dfb8966ae475c8822106`
and passes **all 34 CLI tests in 85.36 s** against the recovered persistent v15
interpreter. Both identities are unchanged after the gate; the final receipt is
`work/strict-ty-0020-v15-ready.json`.

The first 0020 CLI launch omitted the selected `CPYTHON_BIN`: 12 pure tests
passed, while 22 native-dependent cases stopped at their environment preflight.
The preserved setup failure is
`work/logs/strict-ty-0020-cli-missing-selected-environment.log`. The corrected
launch uses `just --command` and the runner checks that `CPYTHON_BIN` resolves to
the actual venv base interpreter before Cargo. This environment export does not
replace a bare `python3` command: raw Python controls must name
`.venv/bin/python` explicitly. Two ordinary-control launches under system Python
3.12 were retained as setup failures and repeated successfully on selected 3.15;
neither is evidence of a compiler regression.

A printed `CheckerError` category is not itself a blocking severity. The old
module-protocol batch failed on `support_import_internalcapi`'s unresolved import;
the simultaneously printed `typing_import` name warning was incorrectly enrolled
as an individual rejection. A direct comparison of archived normal0019 and
normal0020 against the identical retained project succeeds on both and publishes
identical unsuppressed `mismatched-type-name` diagnostics with severity `warning`.
The signed differential evidence is
`work/strict-v15-typing-import-differential/result.json`. This source belongs in
the admitted module-only cohort with its warning visible, not in a waived or
suppressed-error category.

The 0019 private runner initially used the `soac_` project filter. Its 88 passing
tests did not constitute the 138-test full project gate; the unfiltered rerun
passed with zero filtered tests, and packaging checks the recorded counts.
An explicit upstream Rust toolchain override also missed the cached root-stable
artifacts and rebuilt dependencies. Reuse the actual cached toolchain and record
its identity, rather than inferring it from a dependency's toolchain file.

Checked-field tests must opt into their intended field policy and inspect the
published policy before interpreting write behavior. The default test-project
configuration enables supported parameter/return checks but disables field
checks. The private-capture fixture initially checked only class participation
and nominal-leaf presence, which led to an incorrect enforcement diagnosis;
the exported `language_policy` immediately distinguished the fixture mismatch.

The first v6 full gate stopped after workspace Rust tests: 513 passed and 142
failed, mostly mutex-poison cascades from the first invalid ordinary runtime
fixture. Raw runtime tests and pytest did **not** run. The phase function had
re-enabled shell `errexit` in its caller, aborting status collection.

The phase body now uses a subshell. A behavioral test executes the actual
public `just test-all` and private phase recipe with controlled external test
commands. All five status scenarios verify that every test phase is attempted
serially and the exact first nonzero status is returned. The historical 142
results are not 142 independently diagnosed bugs, and that interrupted run is
not a full-gate pass.
